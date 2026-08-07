//! Backend commands for Android Virtual Device (AVD) management.
//!
//! All blocking subprocess work uses `tokio::process::Command` (never a
//! synchronous `std::process::Command` inside an async fn) so the window never
//! freezes. Running-state is tracked in a process-global `Mutex<HashMap>`,
//! which `delete_avd` consults to refuse deleting an AVD that is still running.
use crate::commands::CommandResult;
use crate::commands::paths;
use crate::commands::sdk::{avdmanager_command, stream_child_output, EVT_LOG, LogLine};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{LazyLock, Mutex};
use tauri::Emitter;
use tauri::Window;
use tokio::io::AsyncWriteExt;

/// Process-global record of which AVDs are marked as running this session.
/// Emulators launched outside R.S EXE (e.g. from Android Studio) are not
/// tracked here; this captures the running state for anything we launch.
pub static RUNNING_AVDS: LazyLock<Mutex<HashMap<String, bool>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A parsed AVD definition.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AvdInfo {
    pub name: String,
    pub path: String,
    pub target: String,
    pub api_level: Option<u32>,
    pub ram: Option<String>,
    pub cores: Option<String>,
    pub gpu_mode: Option<String>,
    pub running: bool,
    /// Phase 4 — Speed Mode persisted setting.
    pub speed_mode: Option<bool>,
    /// Phase 7 — Disabled hardware toggles persisted in config.ini.
    pub no_camera: Option<bool>,
    pub no_gps: Option<bool>,
    pub no_bluetooth: Option<bool>,
}

/// Input for `create_avd`. Every numeric field is validated on the backend;
/// a clear `CommandResult::fail` is returned rather than panicking.
#[derive(Debug, Deserialize)]
pub struct CreateAvdInput {
    pub name: String,
    pub system_image: String,
    pub ram: u64,      // MB
    pub cores: u32,
    pub storage: u64,  // MB
    pub gpu_mode: String,
    pub resolution: Option<String>,
    pub dpi: Option<u32>,
    /// Phase 7 — Disable unused emulated hardware by default.
    /// Defaults to true (disabled) for resource-constrained hardware.
    pub no_camera: bool,
    pub no_gps: bool,
    pub no_bluetooth: bool,
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// List every AVD found under `avd_dir()`, parsing each `config.ini`.
///
/// Each AVD on disk is represented by a `<name>.ini` file **and** a `<name>.avd`
/// directory. Without deduplication both are enumerated, producing duplicate
/// entries. We track seen names in a set so each AVD appears exactly once.
#[tauri::command]
pub fn list_avds() -> CommandResult<Vec<AvdInfo>> {
    let dir = paths::avd_dir();
    eprintln!("[DEBUG] list_avds: scanning {}", dir.display());

    let mut infos = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        // No AVD dir yet — perfectly valid; just return empty.
        eprintln!("[DEBUG] list_avds: avd_dir does not exist");
        return CommandResult::success(infos);
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let file_name = match path.file_name().map(|n| n.to_string_lossy().into_owned()) {
            Some(n) => n,
            None => continue,
        };

        // AVDs appear as either `<name>.ini` or `<name>.avd` (folder).
        if file_name.ends_with(".ini") {
            let name = file_name[..file_name.len() - 4].to_string();
            if !seen.insert(name.clone()) {
                eprintln!("[DEBUG] list_avds: skipping duplicate entry for '{}'", name);
                continue;
            }
            if let Some(info) = parse_avd(&dir, &name, Some(&path)) {
                infos.push(info);
            }
        } else if file_name.ends_with(".avd") && path.is_dir() {
            let name = file_name[..file_name.len() - 4].to_string();
            if !seen.insert(name.clone()) {
                eprintln!("[DEBUG] list_avds: skipping duplicate entry for '{}'", name);
                continue;
            }
            if let Some(info) = parse_avd(&dir, &name, None) {
                infos.push(info);
            }
        }
    }

    infos.sort_by(|a, b| a.name.cmp(&b.name));
    eprintln!("[DEBUG] list_avds: found {} unique AVD(s)", infos.len());
    CommandResult::success(infos)
}

/// Launch the Android emulator for a given AVD.
///
/// Spawns `emulator -avd <name>` as a fully detached process so it survives
/// the Tauri app exiting. The AVD is marked as running in the process-global
/// `RUNNING_AVDS` map so `delete_avd` will refuse to delete it while it's up.
#[tauri::command]
pub fn start_avd(name: String) -> CommandResult<bool> {
    eprintln!("[DEBUG] start_avd invoked: name='{}'", name);

    if name.trim().is_empty() {
        return CommandResult::fail("AVD name cannot be empty".to_string());
    }

    let emu = paths::emulator_binary_path();
    if !emu.is_file() {
        return CommandResult::fail(format!(
            "emulator binary not found at {}. Install the 'emulator' SDK package first.",
            emu.display()
        ));
    }

    let avd_dir = paths::avd_dir();
    let ini_path = avd_dir.join(format!("{}.ini", name));
    let avd_folder = avd_dir.join(format!("{}.avd", name));
    if !ini_path.exists() && !avd_folder.exists() {
        return CommandResult::fail(format!(
            "AVD '{}' not found. Create it first via the Devices page.",
            name
        ));
    }

    // Mark as running so delete_avd refuses to delete it while emulator is up.
    mark_avd_running(&name, true);

    // Build env: JAVA_HOME, ANDROID_HOME, ANDROID_SDK_ROOT, and extended PATH
    // (java bin + existing + emulator lib dirs for DLL resolution).
    let mut env_pairs = paths::java_env_pairs();
    // Prepend emulator lib dirs to PATH so emulator.exe finds its DLLs.
    let emu_lib64 = paths::emulator_dir().join("lib64");
    let emu_lib = paths::emulator_dir().join("lib");
    let existing_path = std::env::var("PATH").unwrap_or_default();
    let ext_path = format!(
        "{}{}{}{}{}",
        emu_lib64.to_string_lossy(),
        std::path::MAIN_SEPARATOR,
        emu_lib.to_string_lossy(),
        std::path::MAIN_SEPARATOR,
        existing_path
    );
    // Find and replace the PATH entry we set in java_env_pairs.
    for (k, v) in env_pairs.iter_mut() {
        if *k == "PATH" {
            *v = ext_path.clone();
            break;
        }
    }

    let mut cmd = std::process::Command::new(&emu);
    cmd.envs(env_pairs);
    cmd.env("ANDROID_AVD_HOME", avd_dir.to_string_lossy().into_owned());
    cmd.arg("-avd").arg(&name)
        .arg("-gpu")
        .arg("host")
        .arg("-no-snapshot-load");

    // On Windows, use DETACHED_PROCESS so the emulator survives app exit.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x00000008); // CREATE_NO_WINDOW is 0x08000000; DETACHED_PROCESS is 0x00000008
    }

    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    match cmd.spawn() {
        Ok(child) => {
            let pid = child.id();
            eprintln!("[DEBUG] start_avd: emulator spawned, pid={}", pid);
            // Intentionally forget the child handle — emulator runs independently.
            std::mem::forget(child);
            CommandResult::success(true)
        }
        Err(e) => {
            eprintln!("[DEBUG] start_avd: FAILED to spawn emulator: {}", e);
            mark_avd_running(&name, false);
            CommandResult::fail(format!("Failed to launch emulator: {e}"))
        }
    }
}

/// Validate inputs and run `avdmanager create avd`, then write tuning into the
/// freshly created `config.ini`.
#[tauri::command]
pub async fn create_avd(window: Window, input: CreateAvdInput) -> CommandResult<bool> {
    eprintln!(
        "[DEBUG] create_avd invoked: name='{}' system_image='{}' ram={} cores={} storage={} gpu_mode='{}' resolution={:?} dpi={:?}",
        input.name, input.system_image, input.ram, input.cores, input.storage, input.gpu_mode, input.resolution, input.dpi
    );
    // ---- Backend-side validation (never panics) -----------------------
    if input.name.trim().is_empty() {
        return CommandResult::fail("AVD name cannot be empty".to_string());
    }
    if !is_valid_avd_name(&input.name) {
        return CommandResult::fail(format!(
            "AVD name '{}' contains invalid characters (only letters, digits, spaces, '.', '_', '-' are allowed)",
            input.name
        ));
    }
    if input.system_image.trim().is_empty() {
        return CommandResult::fail("A system image must be selected".to_string());
    }
    // RAM bounds (MB).
    if input.ram < 128 || input.ram > 32768 {
        return CommandResult::fail(format!(
            "RAM must be between 128 and 32768 MB (got {} MB)",
            input.ram
        ));
    }
    // CPU cores bounds.
    if input.cores < 1 || input.cores > 32 {
        return CommandResult::fail(format!(
            "CPU cores must be between 1 and 32 (got {})",
            input.cores
        ));
    }
    // Storage bounds (MB).
    if input.storage < 128 || input.storage > 262144 {
        return CommandResult::fail(format!(
            "Storage must be between 128 and 262144 MB (got {} MB)",
            input.storage
        ));
    }
    // GPU mode whitelist.
    match input.gpu_mode.as_str() {
        "host" | "host-only" | "swiftshader_indirect" | "swiftshader_host" | "software" | "none" => {}
        other => {
            return CommandResult::fail(format!(
                "Invalid GPU mode '{}'. Allowed: host, host-only, swiftshader_indirect, swiftshader_host, software, none",
                other
            ));
        }
    }
    if let Some(dpi) = input.dpi {
        if dpi < 80 || dpi > 640 {
            return CommandResult::fail(format!("DPI must be between 80 and 640 (got {})", dpi));
        }
    }

    // ---- Already exists? ----------------------------------------------
    let avd_dir = paths::avd_dir();
    let avd_folder = avd_dir.join(format!("{}.avd", input.name));
    let ini_path = avd_dir.join(format!("{}.ini", input.name));
    if avd_folder.exists() || ini_path.exists() {
        return CommandResult::fail(format!(
            "An AVD named '{}' already exists. Choose a different name.",
            input.name
        ));
    }

    let _ = window.emit(EVT_LOG, LogLine {
        stage: format!("create:{}", input.name),
        line: format!("Creating AVD '{}' with image {}", input.name, input.system_image),
    });

    // ---- Run `avdmanager create avd` ----------------------------------
    // We pass a fixed device definition (pixel_4) so avdmanager doesn't prompt.
    let mut args: Vec<&str> = vec![
        "create",
        "avd",
        "-n",
        &input.name,
        "-k",
        &input.system_image,
        "-d",
        "pixel_4",
        "--abi",
    ];
    let abi = sdk_abi(&input.system_image);
    let abi_owned = abi.clone();
    args.push(&abi_owned);
    if input.gpu_mode == "none" || input.gpu_mode == "software" {
        args.push("--no-snapshot");
    }
    args.push("--force");

    let mut cmd = match avdmanager_command(&args) {
        Ok(c) => c,
        Err(e) => {
            return CommandResult::fail(format!(
                "Cannot run avdmanager: {e}. Is cmdline-tools installed?"
            ))
        }
    };
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return CommandResult::fail(format!("Failed to spawn avdmanager: {e}")),
    };
    // Pre-pipe "y" so avdmanager never blocks on an interactive prompt.
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(b"y\ny\ny\ny\ny\n").await;
    }
    let output = stream_child_output(&mut child, &window, &format!("create:{}", input.name)).await;
    let status = match child.wait().await {
        Ok(s) => s,
        Err(e) => return CommandResult::fail(format!("avdmanager exited abnormally: {e}\n--- output ---\n{output}")),
    };
    if !status.success() {
        return CommandResult::fail(format!(
            "avdmanager create avd failed with status {} (is the system image installed?) — output:\n{}",
            status, output
        ));
    }

    // ---- Write tuning into config.ini -----------------------------------
    // AVD created successfully; now tune ram/cores/storage/gpu into config.ini.
    let cfg_path = avd_folder.join("config.ini");
    eprintln!("[DEBUG] create_avd: writing config.ini at {}", cfg_path.display());
    if let Err(e) = write_avd_config(&cfg_path, &input) {
        return CommandResult::fail(format!(
            "AVD folder created, but failed to write config.ini: {e}"
        ));
    }

    let _ = window.emit(
        crate::commands::sdk::EVT_DONE,
        serde_json::json!({
            "component": format!("create:{}", input.name),
            "ok": true,
            "message": format!("AVD '{}' created", input.name),
        }),
    );
    CommandResult::success(true)
}

/// Delete an AVD. Refuses if the AVD is currently marked as running.
#[tauri::command]
pub fn delete_avd(name: String) -> CommandResult<bool> {
    eprintln!("[DEBUG] delete_avd invoked: name='{}'", name);
    if name.trim().is_empty() {
        return CommandResult::fail("AVD name cannot be empty".to_string());
    }

    {
        let map = match RUNNING_AVDS.lock() {
            Ok(g) => g,
            Err(e) => {
                return CommandResult::fail(format!("running-state lock poisoned: {e}"));
            }
        };
        if map.get(&name).copied().unwrap_or(false) {
            eprintln!("[DEBUG] delete_avd: REFUSED — '{}' is marked as running", name);
            return CommandResult::fail(format!(
                "Cannot delete '{}': it is currently running. Stop the emulator first.",
                name
            ));
        }
    }

    let avd_dir = paths::avd_dir();
    let folder = avd_dir.join(format!("{}.avd", name));
    let ini = avd_dir.join(format!("{}.ini", name));

    eprintln!("[DEBUG] delete_avd: folder={} ini={}", folder.display(), ini.display());

    let exists = folder.exists() || ini.exists();
    if !exists {
        eprintln!("[DEBUG] delete_avd: AVD '{}' not found on disk", name);
        return CommandResult::fail(format!("AVD '{}' not found on disk", name));
    }

    if folder.exists() {
        eprintln!("[DEBUG] delete_avd: removing folder {}", folder.display());
        if let Err(e) = std::fs::remove_dir_all(&folder) {
            eprintln!("[DEBUG] delete_avd: FAILED to remove folder: {}", e);
            return CommandResult::fail(format!(
                "Failed to remove AVD folder {}: {e}",
                folder.display()
            ));
        }
    }
    if ini.exists() {
        eprintln!("[DEBUG] delete_avd: removing ini {}", ini.display());
        if let Err(e) = std::fs::remove_file(&ini) {
            eprintln!("[DEBUG] delete_avd: FAILED to remove ini: {}", e);
            return CommandResult::fail(format!(
                "Failed to remove AVD ini {}: {e}",
                ini.display()
            ));
        }
    }

    // Also clear any leftover entry in the running map.
    let _ = RUNNING_AVDS.lock().map(|mut m| {
        m.remove(&name);
    });

    eprintln!("[DEBUG] delete_avd: success — '{}' deleted", name);
    CommandResult::success(true)
}

// ---------------------------------------------------------------------------
// Parsing / helpers
// ---------------------------------------------------------------------------

/// Mark an AVD as running (used by the (future) launch command). Public so the
/// emulator-launch code in a later phase can update this.
#[allow(dead_code)]
pub fn mark_avd_running(name: &str, running: bool) {
    if let Ok(mut m) = RUNNING_AVDS.lock() {
        m.insert(name.to_string(), running);
    }
}

/// Parse an AVD given its ini path (and optionally the `.avd` folder).
fn parse_avd(dir: &Path, name: &str, ini_path: Option<&Path>) -> Option<AvdInfo> {
    // Resolve the .avd folder: from the ini's `path=` value, or `<name>.avd`.
    let folder = (|| {
        if let Some(ini) = ini_path {
            if let Ok(text) = std::fs::read_to_string(ini) {
                let p = parse_ini_value(&text, "path");
                if !p.is_empty() {
                    return Some(PathBuf::from(p));
                }
            }
        }
        let candidate = dir.join(format!("{}.avd", name));
        if candidate.is_dir() {
            Some(candidate)
        } else {
            None
        }
    })()?;

    let config = folder.join("config.ini");
    let target = read_ini(&config, "target")
        .or_else(|| ini_path.and_then(|p| read_ini(p, "target")))
        .unwrap_or_default();
    let api_level = if target.is_empty() { None } else { extract_api_level(&target) };
    let ram = read_ini(&config, "hw.ram.size");
    let cores = read_ini(&config, "hw.cpu.ncore");
    let gpu_mode = read_ini(&config, "hw.gpu.mode");
    let speed_mode = read_ini(&config, "kb.speedmode").and_then(|v| {
        match v.to_lowercase().as_str() {
            "true" | "1" | "yes" => Some(true),
            "false" | "0" | "no" => Some(false),
            _ => None,
        }
    });

    // Phase 7 — Read disabled hardware toggles from config.ini.
    let no_camera = read_ini(&config, "kb.no_camera").and_then(|v| {
        match v.to_lowercase().as_str() {
            "true" | "1" | "yes" => Some(true),
            "false" | "0" | "no" => Some(false),
            _ => None,
        }
    });
    let no_gps = read_ini(&config, "kb.no_gps").and_then(|v| {
        match v.to_lowercase().as_str() {
            "true" | "1" | "yes" => Some(true),
            "false" | "0" | "no" => Some(false),
            _ => None,
        }
    });
    let no_bluetooth = read_ini(&config, "kb.no_bluetooth").and_then(|v| {
        match v.to_lowercase().as_str() {
            "true" | "1" | "yes" => Some(true),
            "false" | "0" | "no" => Some(false),
            _ => None,
        }
    });

    // Resolve the AVD name: prefer `name=` from config.ini, fall back to
    // `AvdId=` (the key avdmanager actually writes), then to the filename.
    let resolved_name = read_ini(&config, "name")
        .filter(|s| !s.is_empty())
        .or_else(|| read_ini(&config, "AvdId"))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| name.to_string());

    let running = RUNNING_AVDS
        .lock()
        .map(|m| m.get(&resolved_name).copied().unwrap_or(false))
        .unwrap_or(false);

    Some(AvdInfo {
        name: resolved_name,
        path: folder.to_string_lossy().into_owned(),
        target,
        api_level,
        ram,
        cores,
        gpu_mode,
        running,
        speed_mode,
        no_camera,
        no_gps,
        no_bluetooth,
    })
}

fn extract_api_level(target: &str) -> Option<u32> {
    // sdkmanager uses "android-34" style targets, but config.ini may
    // contain additional text after the API level (e.g. "android-34 (Google APIs)").
    // Strip the "android-" prefix then extract only the leading digits.
    let t = target.trim_start_matches("android-");
    let numeric: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
    numeric.parse::<u32>().ok()
}

fn read_ini(path: &Path, key: &str) -> Option<String> {
    let contents = std::fs::read_to_string(path).ok()?;
    let val = parse_ini_value(&contents, key);
    if val.is_empty() { None } else { Some(val) }
}

/// Minimal ini reader: returns the value for the first `key=value` line.
fn parse_ini_value(text: &str, key: &str) -> String {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            if k.trim() == key {
                return v.trim().to_string();
            }
        }
    }
    String::new()
}

/// Derive an ABI from the system-image package id by taking the last
/// semicolon-delimited segment — this is always the ABI.
/// e.g. `system-images;android-10;default;x86`       -> `x86`
///      `system-images;android-34;google_apis;x86_64` -> `x86_64`
///      `system-images;android-34;google_apis;arm64-v8a` -> `arm64-v8a`
fn sdk_abi(system_image: &str) -> String {
    system_image
        .split(';')
        .last()
        .filter(|s| !s.is_empty())
        .unwrap_or("x86_64")
        .to_string()
}

fn is_valid_avd_name(name: &str) -> bool {
    let n = name.trim();
    if n.is_empty() {
        return false;
    }
    n.chars().all(|c| c.is_alphanumeric() || c == ' ' || c == '.' || c == '_' || c == '-')
}

/// Append tuning keys into the freshly created config.ini. Uses line-based
/// editing so existing keys are replaced and unknown keys preserved.
fn write_avd_config(path: &Path, input: &CreateAvdInput) -> Result<(), String> {
    let contents = std::fs::read_to_string(path)
        .map_err(|e| format!("read existing config.ini: {e}"))?;

    let mut out: Vec<String> = Vec::new();
    let replace_keys = [
        "hw.ram.size",
        "hw.cpu.ncore",
        "disk.dataPartition.size",
        "hw.gpu.mode",
        "ro.sf.dpi",
        "hw.lcd.width",
        "hw.lcd.height",
    ];

    for line in contents.lines() {
        if let Some((k, _)) = line.split_once('=') {
            if replace_keys.contains(&k.trim()) {
                continue; // re-emit below with tuned values
            }
        }
        out.push(line.to_string());
    }

    push_kv(&mut out, "hw.ram.size", &format!("{}MB", input.ram));
    push_kv(&mut out, "hw.cpu.ncore", &input.cores.to_string());
    push_kv(&mut out, "disk.dataPartition.size", &format!("{}M", input.storage));
    push_kv(&mut out, "hw.gpu.mode", &input.gpu_mode);
    if let Some(dpi) = input.dpi {
        push_kv(&mut out, "ro.sf.dpi", &dpi.to_string());
    }
    if let Some(res) = &input.resolution {
        push_kv(&mut out, "hw.lcd.width", res.split('x').next().unwrap_or("0"));
        push_kv(&mut out, "hw.lcd.height", res.split('x').nth(1).unwrap_or("0"));
    }
    // Phase 7 — Disabled hardware toggles (default true = disabled for new AVDs).
    push_kv(&mut out, "kb.no_camera", if input.no_camera { "true" } else { "false" });
    push_kv(&mut out, "kb.no_gps", if input.no_gps { "true" } else { "false" });
    push_kv(&mut out, "kb.no_bluetooth", if input.no_bluetooth { "true" } else { "false" });

    let mut text = out.join("\n");
    text.push('\n');
    std::fs::write(path, text).map_err(|e| format!("write config.ini: {e}"))?;
    Ok(())
}

fn push_kv(out: &mut Vec<String>, key: &str, value: &str) {
    out.push(format!("{}={}", key, value));
}

// ---------------------------------------------------------------------------
// AVD config tuning (live edit)
// ---------------------------------------------------------------------------

/// Input for `update_avd_config`. Only provided fields are written; omitted
/// fields leave the existing config.ini value untouched.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct AvdTuningOptions {
    pub ram: Option<u64>,      // MB
    pub cores: Option<u32>,
    pub heap_size: Option<u32>, // MB (hw.heap.size)
    pub gpu_mode: Option<String>,
    pub resolution: Option<String>, // "WxH"
    pub dpi: Option<u32>,
    pub wipe_user_data: Option<bool>,
    /// Phase 4 — Speed Mode: post-boot GPU composition optimization.
    /// Persisted as `kb.speedmode=true/false` in config.ini.
    pub speed_mode: Option<bool>,
    /// Phase 7 — Disable unused emulated hardware.
    /// Persisted as `kb.no_camera`, `kb.no_gps`, `kb.no_bluetooth` in config.ini.
    /// Default true (disabled) for new AVDs.
    pub no_camera: Option<bool>,
    pub no_gps: Option<bool>,
    pub no_bluetooth: Option<bool>,
}

/// Live-edit an existing AVD's `config.ini`.
///
/// Refuses to edit while the AVD is currently running (checking the
/// backend-authoritative `RUNNING_AVDS` map). Returns a clear error telling
/// the user to stop the device first.
///
/// Validates values against safe ranges, applying profile-specific clamps for
/// Wear OS, Android TV, and Automotive devices.
#[tauri::command]
pub fn update_avd_config(name: String, options: AvdTuningOptions) -> CommandResult<bool> {
    // Log the EXACT raw options received from the frontend before any processing.
    eprintln!(
        "[DEBUG] update_avd_config: RAW options received from frontend — name='{}' ram={:?} cores={:?} heap_size={:?} gpu_mode={:?} resolution={:?} dpi={:?} wipe_user_data={:?}",
        name,
        options.ram,
        options.cores,
        options.heap_size,
        options.gpu_mode,
        options.resolution,
        options.dpi,
        options.wipe_user_data,
    );

    if name.trim().is_empty() {
        return CommandResult::fail("AVD name cannot be empty".to_string());
    }

    // Refuse to edit while running.
    {
        let map = match RUNNING_AVDS.lock() {
            Ok(g) => g,
            Err(e) => return CommandResult::fail(format!("running-state lock poisoned: {e}")),
        };
        if map.get(&name).copied().unwrap_or(false) {
            return CommandResult::fail(format!(
                "Cannot edit '{}': it is currently running. Stop the emulator first, then retry.",
                name
            ));
        }
    }

    let avd_dir = paths::avd_dir();
    let avd_folder = avd_dir.join(format!("{}.avd", name));
    let cfg_path = avd_folder.join("config.ini");

    if !cfg_path.exists() {
        return CommandResult::fail(format!(
            "config.ini not found for '{}' at {}",
            name,
            cfg_path.display()
        ));
    }

    // Read existing config to detect the device profile for clamping.
    let existing = match std::fs::read_to_string(&cfg_path) {
        Ok(t) => t,
        Err(e) => return CommandResult::fail(format!("read config.ini: {e}")),
    };
    let profile = detect_avd_profile(&existing);

    // Log which profile was detected and what clamp range is being applied.
    let (ram_min, ram_max, heap_min, heap_max, cores_min, cores_max) = profile_limits(profile);
    eprintln!(
        "[DEBUG] update_avd_config: profile detected for '{}' = {:?}, RAM clamp range = {}..{}, heap = {}..{}, cores = {}..{}",
        name, profile, ram_min, ram_max, heap_min, heap_max, cores_min, cores_max
    );

    // Validate and apply clamps per profile.
    let mut ram = options.ram;
    let mut heap = options.heap_size;
    let mut cores = options.cores;

    if let Some(ref mut r) = ram {
        let clamped = (*r).clamp(ram_min, ram_max);
        if *r != clamped {
            eprintln!(
                "[DEBUG] update_avd_config: CLAMPING RAM {} -> {} for profile {:?} (range {}..{})",
                r, clamped, profile, ram_min, ram_max
            );
            *r = clamped;
        } else {
            eprintln!(
                "[DEBUG] update_avd_config: RAM {} is within range {}..{} for profile {:?}, no clamping needed",
                r, ram_min, ram_max, profile
            );
        }
    }
    if let Some(ref mut h) = heap {
        let clamped = (*h).clamp(heap_min, heap_max);
        if *h != clamped {
            eprintln!(
                "[DEBUG] update_avd_config: CLAMPING heap {} -> {} for profile {:?} (range {}..{})",
                h, clamped, profile, heap_min, heap_max
            );
            *h = clamped;
        }
    }
    if let Some(ref mut c) = cores {
        let clamped = (*c).clamp(cores_min, cores_max);
        if *c != clamped {
            eprintln!(
                "[DEBUG] update_avd_config: CLAMPING cores {} -> {} for profile {:?} (range {}..{})",
                c, clamped, profile, cores_min, cores_max
            );
            *c = clamped;
        }
    }

    // Log the final values that will be written.
    eprintln!(
        "[DEBUG] update_avd_config: final values to write — ram={:?} heap={:?} cores={:?}",
        ram, heap, cores
    );

    // Build the write plan: keys to add/replace.
    let mut replace_keys = Vec::new();
    if ram.is_some() { replace_keys.push("hw.ram.size"); }
    if cores.is_some() { replace_keys.push("hw.cpu.ncore"); }
    if heap.is_some() { replace_keys.push("hw.heap.size"); }
    if options.gpu_mode.is_some() { replace_keys.push("hw.gpu.mode"); }
    if options.resolution.is_some() {
        replace_keys.push("hw.lcd.width");
        replace_keys.push("hw.lcd.height");
    }
    if options.dpi.is_some() { replace_keys.push("ro.sf.dpi"); }
    if options.wipe_user_data == Some(true) {
        replace_keys.push("disk.dataPartition.size");
    }
    if options.speed_mode.is_some() {
        replace_keys.push("kb.speedmode");
    }
    if options.no_camera.is_some() {
        replace_keys.push("kb.no_camera");
    }
    if options.no_gps.is_some() {
        replace_keys.push("kb.no_gps");
    }
    if options.no_bluetooth.is_some() {
        replace_keys.push("kb.no_bluetooth");
    }

    let mut out: Vec<String> = Vec::new();
    for line in existing.lines() {
        if let Some((k, _)) = line.split_once('=') {
            if replace_keys.contains(&k.trim()) {
                continue;
            }
        }
        out.push(line.to_string());
    }

    if let Some(r) = ram {
        push_kv(&mut out, "hw.ram.size", &format!("{}MB", r));
    }
    if let Some(c) = cores {
        push_kv(&mut out, "hw.cpu.ncore", &c.to_string());
    }
    if let Some(h) = heap {
        push_kv(&mut out, "hw.heap.size", &format!("{}MB", h));
    }
    if let Some(ref gpu) = options.gpu_mode {
        // Validate GPU mode against the known safe list.
        match gpu.as_str() {
            "host" | "host-only" | "swiftshader_indirect" | "swiftshader_host" | "software" | "none" => {}
            other => {
                return CommandResult::fail(format!(
                    "Invalid GPU mode '{}'. Allowed: host, host-only, swiftshader_indirect, swiftshader_host, software, none",
                    other
                ));
            }
        }
        push_kv(&mut out, "hw.gpu.mode", gpu);
    }
    if let Some(ref res) = options.resolution {
        let parts: Vec<&str> = res.split('x').collect();
        if parts.len() == 2 {
            if let (Ok(w), Ok(h)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) {
                push_kv(&mut out, "hw.lcd.width", &w.to_string());
                push_kv(&mut out, "hw.lcd.height", &h.to_string());
            }
        }
    }
    if let Some(dpi) = options.dpi {
        if dpi < 80 || dpi > 640 {
            return CommandResult::fail(format!("DPI must be between 80 and 640 (got {})", dpi));
        }
        push_kv(&mut out, "ro.sf.dpi", &dpi.to_string());
    }
    if options.wipe_user_data == Some(true) {
        // Reset data partition to a fresh 2 GB.
        push_kv(&mut out, "disk.dataPartition.size", "2048M");
    }
    if let Some(sm) = options.speed_mode {
        push_kv(&mut out, "kb.speedmode", if sm { "true" } else { "false" });
    }
    if let Some(nc) = options.no_camera {
        push_kv(&mut out, "kb.no_camera", if nc { "true" } else { "false" });
    }
    if let Some(ng) = options.no_gps {
        push_kv(&mut out, "kb.no_gps", if ng { "true" } else { "false" });
    }
    if let Some(nb) = options.no_bluetooth {
        push_kv(&mut out, "kb.no_bluetooth", if nb { "true" } else { "false" });
    }

    eprintln!(
        "[DEBUG] update_avd_config: writing config.ini for '{}' with keys: ram={:?} cores={:?} heap={:?} gpu_mode={:?} resolution={:?} dpi={:?} wipe_user_data={:?} speed_mode={:?} no_camera={:?} no_gps={:?} no_bluetooth={:?}",
        name, ram, cores, heap, options.gpu_mode, options.resolution, options.dpi, options.wipe_user_data, options.speed_mode, options.no_camera, options.no_gps, options.no_bluetooth
    );

    let mut text = out.join("\n");
    text.push('\n');
    if let Err(e) = std::fs::write(&cfg_path, text) {
        return CommandResult::fail(format!("write config.ini: {e}"));
    }

    // Verify what was actually written by reading back hw.ram.size.
    if let Ok(contents) = std::fs::read_to_string(&cfg_path) {
        for line in contents.lines() {
            if let Some((k, v)) = line.split_once('=') {
                if k.trim() == "hw.ram.size" {
                    eprintln!("[DEBUG] update_avd_config: verified config.ini hw.ram.size={} for '{}'", v.trim(), name);
                }
            }
        }
    }

    eprintln!("[DEBUG] update_avd_config: wrote config.ini for '{}'", name);
    CommandResult::success(true)
}

// ---------------------------------------------------------------------------
// Profile detection & clamps
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvdProfile {
    Phone,
    WearOs,
    Tv,
    Automotive,
}

/// Heuristic profile detection from config.ini contents.
///
/// We inspect `target` and `device` / `hw.device.name` for well-known substrings.
pub fn detect_avd_profile(config_text: &str) -> AvdProfile {
    let lower = config_text.to_lowercase();
    // Wear OS device names commonly contain "wear" or "round".
    if lower.contains("wear") || lower.contains("round") {
        eprintln!("[DEBUG] detect_avd_profile: matched WearOs (wear/round)");
        return AvdProfile::WearOs;
    }
    // Android TV targets / devices.
    if lower.contains("android-tv") || lower.contains("television") || lower.contains("tv ") {
        eprintln!("[DEBUG] detect_avd_profile: matched Tv (android-tv/television/tv )");
        return AvdProfile::Tv;
    }
    // Automotive targets — only match the specific "automotive" keyword,
    // NOT the broad "car" substring which false-positives on "sdcard", "sdCard", etc.
    if lower.contains("automotive") {
        eprintln!("[DEBUG] detect_avd_profile: matched Automotive via 'automotive' substring");
        return AvdProfile::Automotive;
    }
    eprintln!("[DEBUG] detect_avd_profile: no special profile matched, defaulting to Phone");
    AvdProfile::Phone
}

/// Safe (min, max) ranges for RAM (MB), heap (MB), and cores per profile.
///
/// These values are ported from the reference commands.rs implementation and
/// reflect the practical limits for each form factor.
fn profile_limits(profile: AvdProfile) -> (u64, u64, u32, u32, u32, u32) {
    match profile {
        AvdProfile::Phone => (512, 16384, 64, 512, 1, 16),
        AvdProfile::WearOs => (256, 2048, 32, 128, 1, 4),
        AvdProfile::Tv => (2048, 8192, 128, 256, 2, 8),
        AvdProfile::Automotive => (4096, 16384, 256, 512, 2, 12),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    #[test]
    fn validates_avd_name_and_fields() {
        assert!(is_valid_avd_name("Pixel_4"));
        assert!(is_valid_avd_name("Nexus 5X"));
        assert!(!is_valid_avd_name("bad/name"));
        assert!(!is_valid_avd_name(""));
        assert!(!is_valid_avd_name(" "));
        assert!(is_valid_avd_name("api-34.1"));

        assert_eq!(sdk_abi("system-images;android-34;google_apis;x86_64"), "x86_64");
        assert_eq!(sdk_abi("system-images;android-10;default;x86"), "x86");
        assert_eq!(sdk_abi("system-images;android-34;google_apis;arm64-v8a"), "arm64-v8a");
    }

    #[test]
    fn writes_tuned_config_ini_preserving_others() {
        let dir = std::env::temp_dir().join(format!("droidbay-avd-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("config.ini");
        {
            let mut f = std::fs::File::create(&cfg).unwrap();
            writeln!(f, "AvdId=test").unwrap();
            writeln!(f, "name=test").unwrap();
            writeln!(f, "target=android-34").unwrap();
            writeln!(f, "hw.ram.size=2048MB").unwrap();
            writeln!(f, "PlayStore.enabled=false").unwrap();
            writeln!(f, "hw.gpu.mode=host").unwrap();
        }

        let input = CreateAvdInput {
            name: "test".into(),
            system_image: "system-images;android-34;google_apis;x86_64".into(),
            ram: 4096,
            cores: 4,
            storage: 8192,
            gpu_mode: "host".into(),
            resolution: Some("1080x1920".into()),
            dpi: Some(420),
        };
        let err = write_avd_config(&cfg, &input);
        assert!(err.is_ok(), "write failed: {:?}", err);

        let text = std::fs::read_to_string(&cfg).unwrap();
        assert!(text.contains("hw.ram.size=4096MB"));
        assert!(text.contains("hw.cpu.ncore=4"));
        assert!(text.contains("disk.dataPartition.size=8192M"));
        assert!(text.contains("hw.gpu.mode=host"));
        assert!(text.contains("ro.sf.dpi=420"));
        assert!(text.contains("hw.lcd.width=1080"));
        assert!(text.contains("hw.lcd.height=1920"));
        // Untouched keys are preserved.
        assert!(text.contains("AvdId=test"));
        assert!(text.contains("PlayStore.enabled=false"));
        // Old value replaced, not duplicated.
        assert!(!text.contains("2048MB"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn update_avd_config_refuses_running_avd() {
        use crate::commands::avd::RUNNING_AVDS;
        if let Ok(mut guard) = RUNNING_AVDS.lock() {
            guard.insert("running_avd".into(), true);
        }

        let result = update_avd_config(
            "running_avd".into(),
            AvdTuningOptions { ram: Some(2048), ..Default::default() },
        );
        assert!(!result.ok);
        assert!(result.error.unwrap().contains("currently running"));

        if let Ok(mut guard) = RUNNING_AVDS.lock() {
            guard.remove("running_avd");
        }
    }

    #[test]
    fn profile_clamps_applied() {
        // Wear OS should clamp RAM to 256-2048.
        let cfg = "\n[hw]\nname=WearOSRound\n";
        let profile = detect_avd_profile(cfg);
        assert_eq!(profile, AvdProfile::WearOs);
        let (min, max, _, _, _, _) = profile_limits(profile);
        assert_eq!(min, 256);
        assert_eq!(max, 2048);

        // Android TV target should clamp to TV ranges.
        let cfg = "\ntarget=android-tv-33\n";
        let profile = detect_avd_profile(cfg);
        assert_eq!(profile, AvdProfile::Tv);

        // Automotive target.
        let cfg = "\ntarget=android-automotive-34\n";
        let profile = detect_avd_profile(cfg);
        assert_eq!(profile, AvdProfile::Automotive);

        // Regression: "sdcard" contains "car" but must NOT be detected as Automotive.
        let cfg = "\nhw.sdCard=yes\nsdcard.size=512 MB\nhw.device.name=pixel_4\n";
        let profile = detect_avd_profile(cfg);
        assert_eq!(profile, AvdProfile::Phone);
    }
}
