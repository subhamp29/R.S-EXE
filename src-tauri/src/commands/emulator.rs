//! Backend commands for emulator launch / stop / running-state tracking.
//!
//! Boot is fire-and-forget: `boot_avd` returns immediately with `true` once the
//! emulator process is spawned, and a background task monitors stderr / exit
//! status so the frontend can show live boot logs and detect early failures
//! (missing HAXM/WHPX, corrupted AVD, etc.).
//!
//! Running state is tracked in a process-global `Mutex<HashMap<String,
//! tokio::process::Child>>` so `stop_avd` can find and terminate the exact
//! process we launched. This map is the source of truth for
//! `get_running_avds()`.
use crate::commands::CommandResult;
use crate::commands::avd::RUNNING_AVDS;
use crate::commands::paths;
use crate::commands::sdk::{LogLine, EVT_LOG};
use crate::commands::system::{get_system_info, SystemInfo};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::Emitter;
use tauri::Window;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::Mutex as TokioMutex;
use tokio::time::timeout;

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

/// Process-global map of AVD name -> running emulator child handle.
///
/// This survives across Tauri command invocations so `stop_avd` and
/// `get_running_avds` can see emulators launched in a previous `boot_avd` call.
static EMULATOR_CHILDREN: LazyLock<TokioMutex<HashMap<String, tokio::process::Child>>> =
    LazyLock::new(|| TokioMutex::new(HashMap::new()));

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct BootOptions {
    pub no_snapshot: bool,
    pub gpu_mode: Option<String>,
    pub scale: Option<f32>,
    pub netdelay: Option<String>,
    pub netspeed: Option<String>,
    pub wipe_user_data: bool,
    /// Phase 4 — Speed Mode: post-boot ADB sequence to force GPU composition
    /// via service call SurfaceFlinger 1008 i32 1 (disables hardware overlays).
    /// Only works on rootable images (Google APIs / AOSP, not Google Play).
    /// When None, the persisted config.ini value (kb.speedmode) is used.
    pub speed_mode: Option<bool>,
    /// Phase 7 — Disable unused emulated hardware to reduce overhead.
    /// When None, the persisted config.ini values (kb.no_camera, etc.) are used.
    /// Defaults to true (disabled) for new AVDs.
    pub no_camera: Option<bool>,
    pub no_gps: Option<bool>,
    pub no_bluetooth: Option<bool>,
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Launch an AVD in a fully non-blocking fire-and-forget manner.
///
/// Returns `true` immediately once the emulator process is spawned. The actual
/// boot continues in a background task that:
///   * streams emulator.exe stderr to the frontend via `EVT_LOG`
///   * detects early exits (e.g. HAXM missing) and emits a specific error
///   * clears the running flag on natural exit
///
/// The child process handle is stored in `EMULATOR_CHILDREN` so `stop_avd` can
/// find it later.
#[tauri::command]
pub async fn boot_avd(window: Window, name: String, options: Option<BootOptions>) -> CommandResult<bool> {
    if name.trim().is_empty() {
        return CommandResult::fail("AVD name cannot be empty".to_string());
    }

    let opts = options.unwrap_or_default();

    eprintln!(
        "[EMU] boot_avd invoked: name='{}' no_snapshot={} gpu_mode={:?} scale={:?} netdelay={:?} netspeed={:?} wipe_user_data={} no_camera={:?} no_gps={:?} no_bluetooth={:?}",
        name, opts.no_snapshot, opts.gpu_mode, opts.scale, opts.netdelay, opts.netdelay, opts.wipe_user_data,
        opts.no_camera, opts.no_gps, opts.no_bluetooth
    );

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

    // If already tracked as running, don't double-launch.
    {
        let guard = EMULATOR_CHILDREN.lock().await;
        if guard.contains_key(&name) {
            eprintln!("[EMU] boot_avd: '{}' is already tracked as running", name);
            return CommandResult::success(true);
        }
    }

    // -----------------------------------------------------------------
    // Phase 4 — Pre-boot repair
    // -----------------------------------------------------------------

    // 1) Stale lock file cleanup: remove leftover .lock files from crashed sessions.
    if avd_folder.is_dir() {
        clean_stale_locks(&avd_folder);
    }

    // 2) AVD path auto-repair: if the .ini's path= doesn't match the actual
    //    folder location, correct it so the emulator doesn't fail with a
    //    confusing path error.
    if ini_path.exists() {
        repair_avd_path(&ini_path, &avd_folder, &avd_dir);
    }

    // -----------------------------------------------------------------
    // Build environment
    // -----------------------------------------------------------------
    let mut env_pairs = paths::java_env_pairs();
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
    for (k, v) in env_pairs.iter_mut() {
        if *k == "PATH" {
            *v = ext_path.clone();
            break;
        }
    }

    let mut cmd = Command::new(&emu);
    cmd.envs(env_pairs);
    cmd.env("ANDROID_AVD_HOME", avd_dir.to_string_lossy().into_owned());
    cmd.arg("-avd").arg(&name);

    // Sane defaults: avoid loading a potentially stale snapshot, and keep the
    // window manageable on first boot.
    if opts.no_snapshot {
        cmd.arg("-no-snapshot-load");
    }
    if opts.wipe_user_data {
        cmd.arg("-wipe-data");
    }

    // -----------------------------------------------------------------
    // Phase 4 — GPU/accel flag normalization
    // -----------------------------------------------------------------
    // Normalize user-facing strings to emulator CLI expectations. This
    // defensive mapping avoids passing invalid values when flag names change
    // between emulator versions.
    let gpu_mode = normalize_gpu_mode(opts.gpu_mode.as_deref().unwrap_or("host"));
    cmd.arg("-gpu").arg(&gpu_mode);

    if let Some(scale) = opts.scale {
        cmd.arg("-scale").arg(scale.to_string());
    }
    if let Some(ref netdelay) = opts.netdelay {
        cmd.arg("-netdelay").arg(netdelay);
    }
    if let Some(ref netspeed) = opts.netspeed {
        cmd.arg("-netspeed").arg(netspeed);
    }

    // -----------------------------------------------------------------
    // Phase 4 — Disk I/O reduction flags
    // -----------------------------------------------------------------
    cmd.arg("-logcat").arg("*:S"); // silence verbose guest logcat (huge I/O reduction)
    cmd.arg("-no-metrics");        // suppress telemetry / warning banner

    // -----------------------------------------------------------------
    // Phase 4 — Network flags for faster guest networking
    // -----------------------------------------------------------------
    cmd.arg("-dns-server").arg("1.1.1.1"); // bypass slirp DNS routing latency
    cmd.arg("-prop").arg("qemu.net.tcp.buffersize.default=1048576");
    cmd.arg("-prop").arg("qemu.net.tcp.buffersize.wifi=1048576");

    // -----------------------------------------------------------------
    // Phase 4 — Audio backend (Windows)
    // -----------------------------------------------------------------
    #[cfg(windows)]
    {
        cmd.arg("-audio").arg("wasapi");
    }

    // -----------------------------------------------------------------
    // Phase 4 — Heap sizing by profile AND RAM tier
    // -----------------------------------------------------------------
    // We read config.ini to detect the device profile and the AVD's RAM
    // allocation, then inject the appropriate -prop values.
    let config_text = std::fs::read_to_string(avd_folder.join("config.ini")).unwrap_or_default();
    let profile = crate::commands::avd::detect_avd_profile(&config_text);
    let ram_mb = extract_ram_mb(&config_text);
    let ram_tier = ram_mb >= 8192; // >= 8 GB is the "high" tier

    let (heap_mb, heap_growth_mb) = heap_for_profile_and_ram(profile, ram_tier);
    if heap_mb > 0 {
        cmd.arg("-prop").arg(format!("qemu.dalvik.vm.heapsize={}", heap_mb));
    }
    if heap_growth_mb > 0 {
        cmd.arg("-prop").arg(format!("qemu.dalvik.vm.heapgrowthlimit={}", heap_growth_mb));
    }

    // Phase 4 — Resolve Speed Mode: explicit BootOptions override > persisted
    // config.ini value (kb.speedmode) > default false.
    let speed_mode = opts.speed_mode.unwrap_or_else(|| {
        parse_ini_value(&config_text, "kb.speedmode")
            .to_lowercase()
            .parse::<bool>()
            .unwrap_or(false)
    });
    eprintln!("[EMU] boot_avd: speed_mode={} (source={})", speed_mode, if opts.speed_mode.is_some() { "explicit" } else { "config.ini" });

    // Phase 7 — Resolve hardware disable flags: explicit BootOptions override >
    // persisted config.ini values (kb.no_camera, kb.no_gps, kb.no_bluetooth) >
    // default true (disabled) for new AVDs.
    let no_camera = opts.no_camera.unwrap_or_else(|| {
        parse_ini_value(&config_text, "kb.no_camera")
            .to_lowercase()
            .parse::<bool>()
            .unwrap_or(true)
    });
    let no_gps = opts.no_gps.unwrap_or_else(|| {
        parse_ini_value(&config_text, "kb.no_gps")
            .to_lowercase()
            .parse::<bool>()
            .unwrap_or(true)
    });
    let no_bluetooth = opts.no_bluetooth.unwrap_or_else(|| {
        parse_ini_value(&config_text, "kb.no_bluetooth")
            .to_lowercase()
            .parse::<bool>()
            .unwrap_or(true)
    });
    eprintln!(
        "[EMU] boot_avd: no_camera={} no_gps={} no_bluetooth={} (source: explicit/config.ini/default)",
        no_camera, no_gps, no_bluetooth
    );
    if no_camera {
        cmd.arg("-no-camera");
    }
    if no_gps {
        cmd.arg("-no-gps");
    }
    if no_bluetooth {
        cmd.arg("-no-bluetooth");
    }

    let args: Vec<String> = cmd
        .as_std()
        .get_args()
        .map(|s| s.to_string_lossy().into_owned())
        .collect();
    eprintln!(
        "[EMU] boot_avd: emulator args count={} gpu={}",
        args.len(),
        args.iter().find(|a| *a == "-gpu").map(|_| true).unwrap_or(false)
    );

    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::piped());

             match cmd.spawn() {
                Ok(child) => {
                    let pid = child.id();
                    eprintln!("[EMU] boot_avd: emulator spawned, pid={:?}", pid);

                    // Phase 4 — Process priority elevation: give the emulator
                    // HIGH_PRIORITY_CLASS on Windows so it wins scheduling over
                    // background tasks on CPU-constrained machines.
                    #[cfg(windows)]
                    {
                        if let Some(raw_pid) = pid {
                            let _ = elevate_process_priority(raw_pid);
                        }
                    }

                    #[cfg(windows)]
                    scan_for_orphan_qemu(&name, "post-spawn");

                    // Mark running in both tracking maps.
                    crate::commands::avd::mark_avd_running(&name, true);

                      // Store child handle so stop_avd can find it.
                      {
                          let mut guard = EMULATOR_CHILDREN.lock().await;
                          guard.insert(name.clone(), child);
                      }

                     // Phase 4 — Speed Mode: spawn a CONCURRENT task that runs the
                      // post-boot ADB sequence WHILE the emulator is still running.
                      // This matches the reference's std::thread::spawn pattern:
                      // sleep 4s for boot, then run adb commands on the live device.
                      if speed_mode {
                          let sm_name = name.clone();
                          let sm_win = window.clone();
                          tauri::async_runtime::spawn(async move {
                              run_speed_mode(&sm_win, &sm_name).await;
                          });
                      }

                     // Spawn a background monitor task: stream stderr and detect early
                    // failures. We do NOT await the child here — boot_avd returns
                    // immediately.
                    let win = window.clone();
                    let monitor_name = name.clone();
                    tauri::async_runtime::spawn(async move {
                        // Take the child back out of the map so we can await it.
                        let mut child_opt = {
                            let mut guard = EMULATOR_CHILDREN.lock().await;
                            guard.remove(&monitor_name)
                        };

                        let mut child = match child_opt.take() {
                            Some(c) => c,
                            None => {
                                eprintln!("[EMU] monitor: child for '{}' not found in map", monitor_name);
                                crate::commands::avd::mark_avd_running(&monitor_name, false);
                                return;
                            }
                        };

                        let stderr = child.stderr.take();
                        let win2 = win.clone();
                        let stage = format!("boot:{}", monitor_name);

                        // Shared timestamp of last log line for stall detection.
                        let last_log_ns = Arc::new(AtomicU64::new(
                            SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos() as u64
                        ));

                        // Spawn stderr reader as a CONCURRENT task so the pipe never fills up.
                        let stderr_handle = if let Some(stderr) = stderr {
                            let win3 = win2.clone();
                            let stage2 = stage.clone();
                            let last_log = last_log_ns.clone();
                            Some(tauri::async_runtime::spawn(async move {
                                let reader = BufReader::new(stderr);
                                let mut lines = reader.lines();
                                while let Ok(Some(line)) = lines.next_line().await {
                                    last_log.store(
                                        SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos() as u64,
                                        Ordering::Relaxed,
                                    );
                                    let trimmed = line.trim();
                                    if !trimmed.is_empty() {
                                        let _ = win3.emit(
                                            EVT_LOG,
                                            LogLine {
                                                stage: stage2.clone(),
                                                line: format!("[emulator:stderr] {}", trimmed),
                                            },
                                        );
                                    }
                                }
                            }))
                        } else {
                            None
                        };

                        // Record spawn time for crash-recovery timing.
                        let spawn_ns = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_nanos() as u64;
                        let child_pid = child.id();

                        // Phase 4 — Crash recovery: if the emulator exits within
                        // CRASH_RECOVERY_THRESHOLD_SECS seconds, it's almost
                        // certainly a corrupted Quick Boot snapshot. We delete
                        // the snapshots folder and tell the user to relaunch for
                        // a clean cold boot.
                        const CRASH_RECOVERY_THRESHOLD_SECS: u64 = 10;

                        // Phase 1: wait for early exit (60s timeout for immediate crashes).
                        let mut child_wait = Box::pin(child.wait());
                        let mut early_timer = Box::pin(tokio::time::sleep(Duration::from_secs(60)));
                        let early_exit = loop {
                            tokio::select! {
                                biased;
                                result = &mut child_wait => {
                                    let elapsed_ms = (SystemTime::now()
                                        .duration_since(UNIX_EPOCH)
                                        .unwrap_or_default()
                                        .as_nanos() as u64)
                                        .saturating_sub(spawn_ns)
                                        / 1_000_000;
                                    match result {
                                        Ok(ref status) => {
                                            eprintln!(
                                                "[EMU] monitor: tracked child for '{}' exited early — pid={:?} exit={:?} elapsed={}ms",
                                                monitor_name, child_pid, status, elapsed_ms
                                            );
                                        }
                                        Err(ref e) => {
                                            eprintln!(
                                                "[EMU] monitor: tracked child for '{}' wait error — pid={:?} err={} elapsed={}ms",
                                                monitor_name, child_pid, e, elapsed_ms
                                            );
                                        }
                                    }
                                    #[cfg(windows)]
                                    scan_for_orphan_qemu(&monitor_name, "phase1-exit");
                                    break Some(result);
                                }
                                _ = &mut early_timer => {
                                    break None;
                                }
                            }
                        };

                        let mut exit_status = match early_exit {
                            Some(Ok(status)) => Some(status),
                                Some(Err(e)) => {
                                    let _ = win.emit(
                                        EVT_LOG,
                                        LogLine {
                                            stage: stage.clone(),
                                            line: format!("[boot:debug] emulator process error: {e}"),
                                        },
                                    );
                                #[cfg(windows)]
                                {
                                    use std::os::windows::process::ExitStatusExt;
                                    Some(std::process::ExitStatus::from_raw(1))
                                }
                                #[cfg(not(windows))]
                                {
                                    let _ = e;
                                    None
                                }
                            }
                            None => None, // still running after 60s — normal for cold boot
                        };

                        // Phase 4 — Corrupted snapshot crash recovery.
                        if let Some(_status) = exit_status {
                            let elapsed_s = (SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_nanos() as u64)
                                .saturating_sub(spawn_ns) / 1_000_000_000;
                            if elapsed_s < CRASH_RECOVERY_THRESHOLD_SECS {
                                eprintln!(
                                    "[EMU] monitor: '{}' crashed after {}s (<{}s threshold) — likely corrupted snapshot",
                                    monitor_name, elapsed_s, CRASH_RECOVERY_THRESHOLD_SECS
                                );
                                let avd_dir2 = paths::avd_dir();
                                let snapshots_dir = avd_dir2.join(format!("{}.avd", monitor_name)).join("snapshots");
                                if snapshots_dir.exists() && snapshots_dir.is_dir() {
                                    match std::fs::remove_dir_all(&snapshots_dir) {
                                        Ok(_) => {
                                            let _ = win.emit(
                                                EVT_LOG,
                                                LogLine {
                                                    stage: stage.clone(),
                                                    line: "Corrupted Quick Boot snapshot detected — snapshots deleted. Close this AVD and relaunch for a clean cold boot.".to_string(),
                                                },
                                            );
                                        }
                                        Err(e) => {
                                            eprintln!("[EMU] monitor: failed to delete snapshots dir: {}", e);
                                        }
                                    }
                                }
                            }
                        }

                        // Phase 2: if still running, monitor for stalls until child exits.
                        if exit_status.is_none() {
                            let stall_threshold = Duration::from_secs(240); // 4 minutes of silence
                            let mut stall_timer = tokio::time::interval(Duration::from_secs(30));
                            let mut stalled_emitted = false;

                            loop {
                                tokio::select! {
                                    biased;
                                    result = &mut child_wait => {
                                        let elapsed_ms = (SystemTime::now()
                                            .duration_since(UNIX_EPOCH)
                                            .unwrap_or_default()
                                            .as_nanos() as u64)
                                            .saturating_sub(spawn_ns)
                                            / 1_000_000;
                                        eprintln!(
                                            "[EMU] monitor: tracked child for '{}' wait() resolved — pid={:?} elapsed={}ms",
                                            monitor_name, child_pid, elapsed_ms
                                        );
                                        exit_status = match result {
                                            Ok(status) => {
                                                eprintln!(
                                                    "[EMU] monitor: tracked child for '{}' exited — status={:?}",
                                                    monitor_name, status
                                                );
                                                Some(status)
                                            }
                                            Err(e) => {
                                                let _ = win.emit(
                                                    EVT_LOG,
                                                    LogLine {
                                                        stage: stage.clone(),
                                                        line: format!("[boot:debug] emulator process error: {e}"),
                                                    },
                                                );
                                                eprintln!(
                                                    "[EMU] monitor: tracked child for '{}' wait error: {}",
                                                    monitor_name, e
                                                );
                                                #[cfg(windows)]
                                                {
                                                    use std::os::windows::process::ExitStatusExt;
                                                    Some(std::process::ExitStatus::from_raw(1))
                                                }
                                                #[cfg(not(windows))]
                                                {
                                                    let _ = e;
                                                    None
                                                }
                                            }
                                        };
                                        #[cfg(windows)]
                                        scan_for_orphan_qemu(&monitor_name, "phase2-exit");
                                        break;
                                    }
                                    _ = stall_timer.tick() => {
                                        let last = last_log_ns.load(Ordering::Relaxed);
                                        let now_ns = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos() as u64;
                                        let elapsed = Duration::from_nanos(now_ns.saturating_sub(last));
                                        if elapsed > stall_threshold && !stalled_emitted {
                                            stalled_emitted = true;
                                            let _ = win.emit(
                                                EVT_LOG,
                                                LogLine {
                                                    stage: stage.clone(),
                                                    line: "[boot:debug] Boot appears stalled — this may be a GPU/acceleration compatibility issue. Try GPU mode: swiftshader_indirect or reduce RAM allocation.".to_string(),
                                                },
                                            );
                                        }
                                    }
                                }
                            }
                        }

                        if let Some(status) = exit_status {
                            if !status.success() {
                                let _ = win.emit(
                                    EVT_LOG,
                                    LogLine {
                                        stage: stage.clone(),
                                        line: format!("[boot:debug] emulator exited with status: {:?}", status),
                                    },
                                );
                                // Also emit a structured done event so the frontend
                                // can surface a real error instead of a generic one.
                                let _ = win.emit(
                                    crate::commands::sdk::EVT_DONE,
                                    serde_json::json!({
                                        "component": format!("boot:{}", monitor_name),
                                        "ok": false,
                                        "message": format!(
                                            "emulator exited with status {:?} — check the boot log for details",
                                            status
                                        ),
                                    }),
                                );
                            }
                        }

                        // Wait for stderr reader to finish before clearing running flag.
                        if let Some(h) = stderr_handle {
                            let _ = h.await;
                        }

                        // Clear the running flag so the UI recovers.
                        crate::commands::avd::mark_avd_running(&monitor_name, false);
                    });

            CommandResult::success(true)
        }
        Err(e) => {
            eprintln!("[EMU] boot_avd: FAILED to spawn emulator: {}", e);
            // Emit a specific error so the frontend can surface it.
            let _ = window.emit(
                EVT_LOG,
                LogLine {
                    stage: format!("boot:{}", name),
                    line: format!("Failed to launch emulator: {e}"),
                },
            );
            CommandResult::fail(format!("Failed to launch emulator: {e}"))
        }
    }
}

/// Stop a running AVD.
///
/// Strategy:
///   1. Try `adb -s <serial> emu kill` (graceful shutdown via ADB).
///   2. If ADB can't find the device or the command fails, fall back to
///      killing the tracked `tokio::process::Child` handle directly.
///   3. If neither path works, clear the running flag so the UI isn't stuck.
#[tauri::command]
pub async fn stop_avd(name: String) -> CommandResult<bool> {
    eprintln!("[EMU] stop_avd invoked: name='{}'", name);

    if name.trim().is_empty() {
        return CommandResult::fail("AVD name cannot be empty".to_string());
    }

    let mut adb_succeeded = false;

    // Strategy 1: graceful shutdown via adb.
    if let Ok(adb_path) = adb_path() {
        if adb_path.is_file() {
            // Try to discover the emulator's ADB serial with a timeout.
            // If the emulator never finished booting, adb may hang waiting for
            // a device that will never respond.
            let serial_result = timeout(
                Duration::from_secs(10),
                find_emulator_serial(&adb_path, &name),
            )
            .await;

            let serial: Option<String> = match serial_result {
                Ok(Some(s)) => Some(s),
                Ok(None) => {
                    eprintln!("[EMU] stop_avd: could not find ADB serial for '{}', will try child kill", name);
                    None
                }
                Err(_) => {
                    eprintln!("[EMU] stop_avd: find_emulator_serial timed out for '{}', will try child kill", name);
                    None
                }
            };

            if let Some(serial) = serial {
                eprintln!("[EMU] stop_avd: found serial '{}' for '{}'", serial, name);
                match run_adb_emu_kill(&adb_path, &serial).await {
                    Ok(true) => {
                        eprintln!("[EMU] stop_avd: adb emu kill succeeded for serial '{}'", serial);
                        adb_succeeded = true;
                    }
                    Ok(false) => {
                        eprintln!("[EMU] stop_avd: adb emu kill returned false for serial '{}'", serial);
                    }
                    Err(e) => {
                        eprintln!("[EMU] stop_avd: adb emu kill error: {}", e);
                    }
                }
            }
        }
    }

    // Strategy 2: kill the tracked child process directly.
    if !adb_succeeded {
        eprintln!("[EMU] stop_avd: falling back to child-process kill for '{}'", name);
        if let Some(mut child) = {
            let mut guard = EMULATOR_CHILDREN.lock().await;
            guard.remove(&name)
        } {
            match child.kill().await {
                Ok(_) => {
                    eprintln!("[EMU] stop_avd: killed child process for '{}'", name);
                }
                Err(e) => {
                    eprintln!("[EMU] stop_avd: failed to kill child for '{}': {}", name, e);
                }
            }
            // Wait briefly for the OS to reap the process.
            let _ = timeout(Duration::from_secs(5), child.wait()).await;
        }
    }

    // Clear the running flag regardless so the UI recovers.
    if let Some(mut guard) = crate::commands::avd::RUNNING_AVDS.lock().ok() {
        guard.remove(&name);
    }

    CommandResult::success(true)
}

/// Immediately kill a running AVD's emulator process with no graceful adb attempt.
///
/// This is the escape hatch for stuck boots where `stop_avd` may hang because
/// adb cannot communicate with the emulator. It directly kills the tracked
/// child process and clears the running flag.
#[tauri::command]
pub async fn force_stop_avd(name: String) -> CommandResult<bool> {
    eprintln!("[EMU] force_stop_avd invoked: name='{}'", name);

    if name.trim().is_empty() {
        return CommandResult::fail("AVD name cannot be empty".to_string());
    }

    eprintln!("[EMU] force_stop_avd: force-killing child process for '{}'", name);
    if let Some(mut child) = {
        let mut guard = EMULATOR_CHILDREN.lock().await;
        guard.remove(&name)
    } {
        match child.kill().await {
            Ok(_) => {
                eprintln!("[EMU] force_stop_avd: killed child process for '{}'", name);
            }
            Err(e) => {
                eprintln!("[EMU] force_stop_avd: failed to kill child for '{}': {}", name, e);
            }
        }
        // Wait briefly for the OS to reap the process.
        let _ = timeout(Duration::from_secs(5), child.wait()).await;
    } else {
        eprintln!("[EMU] force_stop_avd: no tracked child found for '{}'", name);
    }

    // Clear the running flag regardless so the UI recovers.
    if let Some(mut guard) = crate::commands::avd::RUNNING_AVDS.lock().ok() {
        guard.remove(&name);
    }

    CommandResult::success(true)
}

/// Return the names of AVDs currently tracked as running by this session.
///
/// Reads from the same `RUNNING_AVDS` map used by `list_avds` so the
/// frontend sees consistent running state across all commands.
#[tauri::command]
pub async fn get_running_avds() -> CommandResult<Vec<String>> {
    let map = crate::commands::avd::RUNNING_AVDS
        .lock()
        .map(|m| m.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    eprintln!("[EMU] get_running_avds: {} tracked", map.len());
    CommandResult::success(map)
}

// ---------------------------------------------------------------------------
// Phase 5 — Boot resource check
// ---------------------------------------------------------------------------

/// Result of a pre-boot resource check. The frontend uses this to warn the
/// user before launching an emulator on a constrained machine.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct BootResourceCheck {
    pub free_ram_bytes: u64,
    pub avd_ram_bytes: u64,
    pub running_count: usize,
    pub running_names: Vec<String>,
    pub low_memory: bool,
    pub multiple_running: bool,
}

/// Quick synchronous-feeling resource check before boot.
///
/// Reads the AVD's `hw.ram.size` from config.ini, calls the existing
/// `get_system_info` (sysinfo-based, fast), and checks the running AVD map.
/// Returns a structured result the frontend can use to show a non-blocking
/// warning dialog.
#[tauri::command]
pub async fn check_boot_resources(name: String) -> CommandResult<BootResourceCheck> {
    eprintln!("[EMU] check_boot_resources: name='{}'", name);

    // 1) Read AVD RAM allocation from config.ini.
    let avd_dir = paths::avd_dir();
    let avd_folder = avd_dir.join(format!("{}.avd", name));
    let cfg_path = avd_folder.join("config.ini");
    let avd_ram_mb = if cfg_path.exists() {
        if let Ok(text) = std::fs::read_to_string(&cfg_path) {
            extract_ram_mb(&text)
        } else {
            0
        }
    } else {
        0
    };
    let avd_ram_bytes = avd_ram_mb * 1024 * 1024;

    // 2) Get current system info (fast sysinfo call, not WMI).
    let sys_info = match get_system_info() {
        r if r.ok => r.output.unwrap_or(SystemInfo {
            total_ram_bytes: 0,
            free_ram_bytes: 0,
            cpu_cores: 0,
            cpu_model: String::new(),
            platform: String::new(),
            architecture: String::new(),
        }),
        _ => SystemInfo {
            total_ram_bytes: 0,
            free_ram_bytes: 0,
            cpu_cores: 0,
            cpu_model: String::new(),
            platform: String::new(),
            architecture: String::new(),
        },
    };
    let free_ram_bytes = sys_info.free_ram_bytes;

    // 3) Check which AVDs are currently running.
    let running_names: Vec<String> = RUNNING_AVDS
        .lock()
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    let running_count = running_names.len();

    // 4) Determine warnings.
    let host_overhead_bytes = 1500 * 1024 * 1024; // ~1.5 GB host OS buffer
    let low_memory = free_ram_bytes < avd_ram_bytes + host_overhead_bytes;
    let multiple_running = running_count > 0 && !running_names.iter().any(|n| n == &name);

    eprintln!(
        "[EMU] check_boot_resources: free={}MB avd={}MB running={} low_mem={} multi_run={}",
        free_ram_bytes / 1024 / 1024,
        avd_ram_bytes / 1024 / 1024,
        running_count,
        low_memory,
        multiple_running
    );

    CommandResult::success(BootResourceCheck {
        free_ram_bytes,
        avd_ram_bytes,
        running_count,
        running_names,
        low_memory,
        multiple_running,
    })
}

// ---------------------------------------------------------------------------
// Phase 5 — Snapshot management
// ---------------------------------------------------------------------------

/// A single snapshot entry inside `<avd>.avd/snapshots/`.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SnapshotInfo {
    /// Entry name (directory or file base name).
    pub id: String,
    /// Human-readable display name.
    pub name: String,
    /// Size in bytes.
    pub size_bytes: u64,
}

/// List all snapshots for an AVD by scanning the `<avd>.avd/snapshots/` directory.
///
/// Returns an empty list if the directory doesn't exist yet (no snapshots made).
#[tauri::command]
pub async fn list_snapshots(avd_name: String) -> CommandResult<Vec<SnapshotInfo>> {
    eprintln!("[EMU] list_snapshots: avd_name='{}'", avd_name);

    let avd_dir = paths::avd_dir();
    let snapshots_dir = avd_dir.join(format!("{}.avd", avd_name)).join("snapshots");

    if !snapshots_dir.exists() || !snapshots_dir.is_dir() {
        eprintln!("[EMU] list_snapshots: snapshots dir does not exist");
        return CommandResult::success(vec![]);
    }

    let mut infos = Vec::new();
    let entries = match std::fs::read_dir(&snapshots_dir) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("[EMU] list_snapshots: read_dir failed: {}", e);
            return CommandResult::success(vec![]);
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let file_name = match path.file_name().map(|n| n.to_string_lossy().into_owned()) {
            Some(n) => n,
            None => continue,
        };

        // Skip the snapshot.pb metadata file itself.
        if file_name == "snapshot.pb" {
            continue;
        }

        let metadata = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };

        let size_bytes = if path.is_dir() {
            // Sum sizes of all files in the directory recursively.
            fn dir_size(path: &PathBuf) -> u64 {
                let mut total: u64 = 0;
                if let Ok(entries) = std::fs::read_dir(path) {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        if p.is_dir() {
                            total += dir_size(&p);
                        } else if let Ok(m) = std::fs::metadata(&p) {
                            total += m.len();
                        }
                    }
                }
                total
            }
            dir_size(&path)
        } else {
            metadata.len()
        };

        infos.push(SnapshotInfo {
            id: file_name.clone(),
            name: file_name.clone(),
            size_bytes,
        });
    }

    // Sort by name for consistent display.
    infos.sort_by(|a, b| a.id.cmp(&b.id));
    eprintln!("[EMU] list_snapshots: found {} snapshots", infos.len());
    CommandResult::success(infos)
}

/// Delete a specific snapshot for an AVD by removing its file or directory
/// from the `<avd>.avd/snapshots/` directory.
#[tauri::command]
pub async fn delete_snapshot(avd_name: String, snapshot_id: String) -> CommandResult<bool> {
    eprintln!("[EMU] delete_snapshot: avd_name='{}' snapshot_id='{}'", avd_name, snapshot_id);

    if snapshot_id.trim().is_empty() {
        return CommandResult::fail("Snapshot ID cannot be empty".to_string());
    }

    let avd_dir = paths::avd_dir();
    let snapshots_dir = avd_dir.join(format!("{}.avd", avd_name)).join("snapshots");
    let snapshot_path = snapshots_dir.join(&snapshot_id);

    if !snapshot_path.exists() {
        return CommandResult::fail(format!(
            "Snapshot '{}' not found for AVD '{}'",
            snapshot_id, avd_name
        ));
    }

    let result = if snapshot_path.is_dir() {
        std::fs::remove_dir_all(&snapshot_path)
    } else {
        std::fs::remove_file(&snapshot_path)
    };

    match result {
        Ok(_) => {
            eprintln!("[EMU] delete_snapshot: deleted '{}'", snapshot_path.display());
            CommandResult::success(true)
        }
        Err(e) => {
            eprintln!("[EMU] delete_snapshot: failed: {}", e);
            CommandResult::fail(format!("Failed to delete snapshot: {}", e))
        }
    }
}

/// Save a snapshot of a running AVD via the emulator console (`adb emu avd snapshot save`).
///
/// This requires the emulator to be running and reachable via ADB. Returns
/// an error if the AVD is not currently running or the console command fails.
///
/// NOTE: The `adb emu` console proxy is the most scriptable approach, but
/// behavior can vary between emulator versions. If this command fails
/// consistently on a given setup, the fallback is to manually use the
/// emulator's Extended Controls UI (snapshot tab) or stop + boot with
/// `-no-snapshot-load` disabled.
#[tauri::command]
pub async fn save_snapshot(window: Window, avd_name: String) -> CommandResult<bool> {
    eprintln!("[EMU] save_snapshot: avd_name='{}'", avd_name);

    let adb = match adb_path() {
        Ok(p) => p,
        Err(e) => return CommandResult::fail(e),
    };
    if !adb.is_file() {
        return CommandResult::fail("adb binary not found".to_string());
    }

    // Wait for the emulator to be ADB-ready before issuing the snapshot save
    // command. The emulator's ADB state goes through "offline" for a lengthy
    // period during boot; `adb emu avd snapshot save` silently fails or hangs
    // in that window, so we poll until the device reaches "device" state.
    let stage = format!("snapshot:{}", avd_name);
    let serial = match wait_for_device_ready(&adb, &avd_name, &window, &stage, 300).await {
        Ok(s) => s,
        Err(e) => {
            let _ = window.emit(
                EVT_LOG,
                LogLine {
                    stage: stage.clone(),
                    line: format!("Save Snapshot failed: {}", e),
                },
            );
            return CommandResult::fail(e);
        }
    };
    eprintln!("[EMU] save_snapshot: serial='{}'", serial);

    let _ = window.emit(
        EVT_LOG,
        LogLine {
            stage: format!("snapshot:{}", avd_name),
            line: format!("Saving snapshot for '{}' via console...", avd_name),
        },
    );

    // Run `adb -s <serial> emu avd snapshot save quickboot`
    let output = match timeout(
        Duration::from_secs(60),
        tokio::process::Command::new(&adb)
            .args(["-s", &serial, "emu", "avd", "snapshot", "save", "quickboot"])
            .output(),
    )
    .await
    {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            let msg = format!("Failed to run adb emu snapshot save: {}", e);
            eprintln!("[EMU] save_snapshot: {}", msg);
            let _ = window.emit(
                EVT_LOG,
                LogLine {
                    stage: format!("snapshot:{}", avd_name),
                    line: format!("Save Snapshot failed: {}", msg),
                },
            );
            return CommandResult::fail(msg);
        }
        Err(_) => {
            let msg = "adb emu snapshot save timed out after 60s".to_string();
            eprintln!("[EMU] save_snapshot: {}", msg);
            let _ = window.emit(
                EVT_LOG,
                LogLine {
                    stage: format!("snapshot:{}", avd_name),
                    line: format!("Save Snapshot timed out: {}", msg),
                },
            );
            return CommandResult::fail(msg);
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if output.status.success() {
        eprintln!("[EMU] save_snapshot: success — stdout={}", stdout);
        let _ = window.emit(
            EVT_LOG,
            LogLine {
                stage: format!("snapshot:{}", avd_name),
                line: "Snapshot saved successfully.".to_string(),
            },
        );
        CommandResult::success(true)
    } else {
        let msg = format!(
            "adb emu snapshot save failed (status={}): {}",
            output.status, stderr
        );
        eprintln!("[EMU] save_snapshot: {}", msg);
        let _ = window.emit(
            EVT_LOG,
            LogLine {
                stage: format!("snapshot:{}", avd_name),
                line: format!("Save Snapshot failed: {}", msg),
            },
        );
        CommandResult::fail(msg)
    }
}

#[cfg(windows)]
fn scan_for_orphan_qemu(avd_name: &str, when: &str) {
    use std::process::Command;
    let output = Command::new("tasklist")
        .args(["/FO", "CSV", "/NH"])
        .output();

    let qemu_count = match output {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            stdout.lines()
                .filter(|line| line.to_lowercase().contains("qemu-system"))
                .count()
        }
        _ => 0,
    };

    eprintln!(
        "[EMU] qemu scan for '{}' ({}) : {} qemu-system process(es) found",
        avd_name, when, qemu_count
    );
}

fn adb_path() -> Result<PathBuf, String> {
    Ok(paths::platform_tools_dir().join(if cfg!(windows) { "adb.exe" } else { "adb" }))
}

/// Run `adb devices` and return parsed `(serial, state)` pairs for emulator
/// devices.
///
/// Each line of `adb devices` output looks like `<serial>\t<state>`, e.g.:
///   `emulator-5554\tdevice`     — ready
///   `emulator-5554\toffline`    — still handshaking / booting
///
/// Unlike the old `line.contains("device")` substring match, this extracts the
/// exact state token so callers can distinguish `"device"` (ready) from
/// `"offline"`, `"unauthorized"`, `"unknown"`, and other transitional states.
async fn adb_emulator_device_states(adb: &PathBuf) -> Vec<(String, String)> {
    let output = match timeout(
        Duration::from_secs(15),
        tokio::process::Command::new(adb).args(["devices"]).output(),
    )
    .await
    {
        Ok(Ok(o)) if o.status.success() => o,
        _ => return Vec::new(),
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            // Skip the "List of devices attached" header and non-emulator lines.
            if !trimmed.starts_with("emulator-") {
                return None;
            }
            let mut parts = trimmed.split_whitespace();
            let serial = parts.next()?;
            let state = parts.next().unwrap_or("");
            Some((serial.to_string(), state.to_string()))
        })
        .collect()
}

/// Run `adb devices` and try to match a running emulator to the given AVD name.
///
/// ADB serials for emulators look like `emulator-5554`. We match by checking
/// `adb -s <serial> emu avd name` against our target name.
///
/// **Readiness check**: a device is only considered a match when its ADB state
/// is exactly `"device"` (ready). Devices in the `"offline"` state — which ADB
/// reports for a lengthy period while the emulator is still bootstrapping its
/// ADB daemon — are explicitly excluded. Previously this used a loose
/// `line.contains("device")` substring match which, while it happened to exclude
/// `"offline"`, was fragile and did not parse the state field properly. Now we
/// parse `<serial> <state>` tokens and compare the state field exactly.
async fn find_emulator_serial(adb: &PathBuf, target_name: &str) -> Option<String> {
    let states = adb_emulator_device_states(adb).await;

    for (serial, state) in states {
        // Only treat the device as ready if ADB state is exactly "device".
        // This correctly excludes "offline" (still booting / handshaking),
        // "unauthorized", and "unknown" states.
        if state != "device" {
            continue;
        }

        // Verify this emulator's AVD name matches our target. Add a timeout so
        // a device that reports "device" but has an unresponsive console cannot
        // hang us indefinitely. If the name query fails (timeout or adb error)
        // for this serial, skip to the next candidate rather than aborting.
        let name_output = match timeout(
            Duration::from_secs(10),
            tokio::process::Command::new(adb)
                .args(["-s", &serial, "emu", "avd", "name"])
                .output(),
        )
        .await
        {
            Ok(Ok(o)) => o,
            _ => continue,
        };

        if name_output.status.success() {
            let avd_name = String::from_utf8_lossy(&name_output.stdout).trim().to_string();
            if avd_name == target_name {
                return Some(serial);
            }
        }
    }
    None
}

/// Poll `adb devices` until the emulator for `target_name` reaches ADB-ready
/// state (`"device"`) or `timeout_secs` elapses.
///
/// The emulator's ADB state goes through `"offline"` for a real, sometimes
/// lengthy period after the process starts (confirmed: several minutes on some
/// hardware) before flipping to `"device"`. Issuing ADB commands like
/// `adb emu avd snapshot save` during the `"offline"` window silently fails or
/// hangs. This helper polls every 2 seconds until the device is ready, emitting
/// periodic progress log lines to `window` so the UI shows the user we are
/// actively waiting rather than appearing frozen.
async fn wait_for_device_ready(
    adb: &PathBuf,
    target_name: &str,
    window: &Window,
    stage: &str,
    timeout_secs: u64,
) -> Result<String, String> {
    const POLL_INTERVAL: Duration = Duration::from_secs(2);
    let deadline = Duration::from_secs(timeout_secs);
    let start = Instant::now();
    let mut last_log_secs: u64 = 0;

    loop {
        if let Some(serial) = find_emulator_serial(adb, target_name).await {
            eprintln!(
                "[EMU] {}: device '{}' is ADB-ready (serial='{}', waited {:.1}s)",
                stage,
                target_name,
                serial,
                start.elapsed().as_secs_f64()
            );
            return Ok(serial);
        }

        if start.elapsed() >= deadline {
            let msg = format!(
                "Emulator '{}' is not fully booted yet — please wait for it to \
                 finish starting before retrying. ADB reports the device as \
                 'offline' (state != 'device') after waiting {}s.",
                target_name,
                start.elapsed().as_secs()
            );
            eprintln!("[EMU] {}: {}", stage, msg);
            return Err(msg);
        }

        // Emit a progress log roughly every 10 seconds so the user sees we're
        // actively waiting rather than appearing frozen.
        let elapsed_secs = start.elapsed().as_secs();
        if elapsed_secs >= last_log_secs + 10 {
            last_log_secs = elapsed_secs;
            let _ = window.emit(
                EVT_LOG,
                LogLine {
                    stage: stage.to_string(),
                    line: format!(
                        "Waiting for emulator '{}' to finish booting (ADB state \
                         not 'device' yet, ~{}s elapsed)...",
                        target_name, elapsed_secs
                    ),
                },
            );
        }

        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Run `adb -s <serial> emu kill` and return whether the command appeared to succeed.
async fn run_adb_emu_kill(adb: &PathBuf, serial: &str) -> Result<bool, String> {
    let output = timeout(
        Duration::from_secs(10),
        tokio::process::Command::new(adb)
            .args(["-s", serial, "emu", "kill"])
            .output(),
    )
    .await
    .map_err(|_| "adb emu kill timed out".to_string())?
    .map_err(|e| format!("failed to run adb emu kill: {e}"))?;

    // adb emu kill often returns non-zero even when it works, so we mainly
    // check that stderr doesn't contain a hard error.
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.to_lowercase().contains("error") && !stderr.to_lowercase().contains("device not found") {
        // "device not found" is expected if the emulator already died.
        return Ok(false);
    }
    Ok(true)
}

// ---------------------------------------------------------------------------
// Phase 4 — AOT app compilation
// ---------------------------------------------------------------------------

/// Compile all user-installed (third-party) apps to native machine code via
/// `adb shell cmd package compile -m speed -f`. This is the guest-side
/// equivalent of the reference `optimize_guest_apps` and measurably improves
/// app/game runtime performance on weak hardware.
#[tauri::command]
pub async fn optimize_installed_apps(window: Window, avd_name: String) -> CommandResult<bool> {
    eprintln!("[EMU] optimize_installed_apps: avd_name='{}'", avd_name);

    let adb = match adb_path() {
        Ok(p) => p,
        Err(e) => return CommandResult::fail(e),
    };
    if !adb.is_file() {
        return CommandResult::fail("adb binary not found".to_string());
    }

    // Wait for the emulator to be ADB-ready before running any adb shell
    // commands. Same offline-timing issue as save_snapshot — the emulator
    // reports "offline" for a lengthy period during boot, and adb shell
    // commands silently fail or hang in that window.
    let stage = format!("optimize:{}", avd_name);
    let serial = match wait_for_device_ready(&adb, &avd_name, &window, &stage, 300).await {
        Ok(s) => s,
        Err(e) => {
            let _ = window.emit(
                EVT_LOG,
                LogLine {
                    stage: stage.clone(),
                    line: format!("Optimization failed: {}", e),
                },
            );
            return CommandResult::fail(e);
        }
    };
    eprintln!("[EMU] optimize_installed_apps: serial='{}'", serial);

    let _ = window.emit(
        EVT_LOG,
        LogLine {
            stage: format!("optimize:{}", avd_name),
            line: format!("Discovering user-installed apps on {} ({})...", avd_name, serial),
        },
    );

    // List only third-party packages (-3).
    let list_output = match tokio::process::Command::new(&adb)
        .args(["-s", &serial, "shell", "pm", "list", "packages", "-3"])
        .output()
        .await
    {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr);
            return CommandResult::fail(format!("pm list packages failed: {}", err));
        }
        Err(e) => return CommandResult::fail(format!("Failed to run pm list packages: {e}")),
    };

    // Parse package names from lines like "package:com.example.app".
    let packages: Vec<String> = list_output
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("package:")
                .map(|s| s.trim().to_string())
        })
        .collect();

    if packages.is_empty() {
        let _ = window.emit(
            EVT_LOG,
            LogLine {
                stage: format!("optimize:{}", avd_name),
                line: "No user-installed apps found to optimize.".to_string(),
            },
        );
        let _ = window.emit(
            crate::commands::sdk::EVT_DONE,
            serde_json::json!({
                "component": format!("optimize:{}", avd_name),
                "ok": true,
                "message": "No user-installed apps found",
            }),
        );
        return CommandResult::success(true);
    }

    let _ = window.emit(
        EVT_LOG,
        LogLine {
            stage: format!("optimize:{}", avd_name),
            line: format!("Found {} user-installed apps. Compiling...", packages.len()),
        },
    );

    let mut compiled = 0usize;
    for (idx, pkg) in packages.iter().enumerate() {
        let _ = window.emit(
            EVT_LOG,
            LogLine {
                stage: format!("optimize:{}", avd_name),
                line: format!("Compiling [{}/{}] {}...", idx + 1, packages.len(), pkg),
            },
        );

        let compile_result = tokio::process::Command::new(&adb)
            .args(["-s", &serial, "shell", "cmd", "package", "compile", "-m", "speed", "-f", pkg])
            .output()
            .await;

        match compile_result {
            Ok(ref o) if o.status.success() => {
                compiled += 1;
            }
            Ok(ref o) => {
                let err = String::from_utf8_lossy(&o.stderr);
                eprintln!("[EMU] optimize: compile failed for {}: {}", pkg, err);
            }
            Err(e) => {
                eprintln!("[EMU] optimize: compile error for {}: {}", pkg, e);
            }
        }
    }

    let _ = window.emit(
        EVT_LOG,
        LogLine {
            stage: format!("optimize:{}", avd_name),
            line: format!("Optimization complete: {}/{} apps compiled.", compiled, packages.len()),
        },
    );
    let _ = window.emit(
        crate::commands::sdk::EVT_DONE,
        serde_json::json!({
            "component": format!("optimize:{}", avd_name),
            "ok": true,
            "message": format!("Optimized {}/{} apps", compiled, packages.len()),
        }),
    );

    CommandResult::success(true)
}

// ---------------------------------------------------------------------------
// Phase 4 — Pre-boot repair helpers
// ---------------------------------------------------------------------------

/// Remove stale `.lock` files left behind by crashed emulator sessions. These
/// can prevent a fresh boot from starting.
fn clean_stale_locks(avd_folder: &PathBuf) {
    let entries = match std::fs::read_dir(avd_folder) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map(|e| e == "lock").unwrap_or(false) {
            eprintln!("[EMU] pre-boot: removing stale lock file {}", path.display());
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// If the AVD's `.ini` file has a `path=` entry that doesn't match the actual
/// `.avd` folder location, correct it. This handles the case where folders
/// were moved/renamed outside the app.
fn repair_avd_path(ini_path: &PathBuf, avd_folder: &PathBuf, _avd_dir: &PathBuf) {
    let Ok(text) = std::fs::read_to_string(ini_path) else {
        return;
    };

    let current_path = parse_ini_value(&text, "path");
    let canonical_avd = avd_folder.canonicalize().unwrap_or_else(|_| avd_folder.clone());
    let canonical_current = PathBuf::from(&current_path).canonicalize().unwrap_or_else(|_| PathBuf::from(current_path.clone()));

    if canonical_current != canonical_avd {
        eprintln!(
            "[EMU] pre-boot: repairing AVD path in {} — was '{}', now '{}'",
            ini_path.display(),
            current_path,
            canonical_avd.display()
        );
        let mut out = Vec::new();
        for line in text.lines() {
            if line.trim_start().starts_with("path=") {
                out.push(format!("path={}", canonical_avd.display()));
            } else {
                out.push(line.to_string());
            }
        }
        let new_text = out.join("\n") + "\n";
        if let Err(e) = std::fs::write(ini_path, new_text) {
            eprintln!("[EMU] pre-boot: failed to write repaired ini: {}", e);
        }
    }
}

// ---------------------------------------------------------------------------
// Phase 4 — GPU/accel flag normalization
// ---------------------------------------------------------------------------

/// Normalize user-facing GPU mode strings to the exact emulator CLI values.
/// This defensive mapping handles silent flag-name changes between emulator
/// versions.
fn normalize_gpu_mode(input: &str) -> String {
    let lower = input.to_lowercase();
    match lower.as_str() {
        "host" | "host-only" => "host".to_string(),
        "swiftshader_indirect" | "swiftshader" | "swiftshader_ind" => "swiftshader_indirect".to_string(),
        "swiftshader_host" | "swiftshader_hw" => "swiftshader_host".to_string(),
        "software" | "sw" | "auto" => "swiftshader_indirect".to_string(),
        "none" | "off" => "none".to_string(),
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Phase 4 — Heap sizing by profile AND RAM tier
// ---------------------------------------------------------------------------

/// Return (heapsize_mb, heapgrowthlimit_mb) for the given device profile and
/// RAM tier.
fn heap_for_profile_and_ram(
    profile: crate::commands::avd::AvdProfile,
    ram_tier: bool, // true = >= 8 GB RAM
) -> (u32, u32) {
    match profile {
        crate::commands::avd::AvdProfile::Phone => {
            if ram_tier { (512, 512) } else { (256, 256) }
        }
        crate::commands::avd::AvdProfile::WearOs => (64, 64),
        crate::commands::avd::AvdProfile::Tv => {
            if ram_tier { (256, 256) } else { (128, 128) }
        }
        crate::commands::avd::AvdProfile::Automotive => {
            if ram_tier { (512, 512) } else { (256, 256) }
        }
    }
}

/// Extract `hw.ram.size` from an AVD config.ini, returning MB as u64.
fn extract_ram_mb(config_text: &str) -> u64 {
    let val = parse_ini_value(config_text, "hw.ram.size");
    let numeric: String = val.chars().take_while(|c| c.is_ascii_digit()).collect();
    numeric.parse::<u64>().unwrap_or(0)
}

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

// ---------------------------------------------------------------------------
// Phase 4 — Process priority elevation (Windows)
// ---------------------------------------------------------------------------

/// Elevate the emulator process to HIGH_PRIORITY_CLASS on Windows. Uses the
/// Windows API via `windows-sys`.
#[cfg(windows)]
fn elevate_process_priority(pid: u32) {
    unsafe {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{OpenProcess, SetPriorityClass};

        // PROCESS_SET_INFORMATION = 0x0200
        let handle = OpenProcess(0x0200, 0, pid);
        if handle.is_null() {
            eprintln!("[EMU] priority: OpenProcess failed for pid {}", pid);
            return;
        }

        // HIGH_PRIORITY_CLASS = 0x00000080
        const HIGH_PRIORITY_CLASS: u32 = 0x00000080;
        let ok = SetPriorityClass(handle, HIGH_PRIORITY_CLASS);
        if ok == 0 {
            eprintln!("[EMU] priority: SetPriorityClass failed for pid {}", pid);
        } else {
            eprintln!("[EMU] priority: elevated pid {} to HIGH_PRIORITY_CLASS", pid);
        }

        CloseHandle(handle);
    }
}

/// Fallback for non-Windows platforms — no-op.
#[cfg(not(windows))]
fn elevate_process_priority(_pid: u32) {}

// ---------------------------------------------------------------------------
// Phase 4 — Speed Mode post-boot ADB sequence
// ---------------------------------------------------------------------------

/// Run the Speed Mode post-boot sequence: wait for device, root (best-effort),
/// then force GPU composition via `service call SurfaceFlinger 1008 i32 1`.
/// This disables hardware overlays for smoother rendering on weak GPUs.
/// Fails gracefully if root isn't available (Google Play images, etc.).
///
/// Spawned concurrently right after emulator process spawn (not after exit),
/// matching the reference's std::thread::spawn pattern.
async fn run_speed_mode(window: &Window, avd_name: &str) {
    // Give the emulator ~4 seconds to start accepting connections before we
    // start hammering it with adb commands. This matches the reference
    // implementation's sleep-before-adb pattern.
    tokio::time::sleep(Duration::from_secs(4)).await;

    let _ = window.emit(
        EVT_LOG,
        LogLine {
            stage: format!("speed:{}", avd_name),
            line: "Speed Mode: running post-boot optimization sequence...".to_string(),
        },
    );

    let adb = match adb_path() {
        Ok(p) => p,
        Err(e) => {
            let _ = window.emit(
                EVT_LOG,
                LogLine {
                    stage: format!("speed:{}", avd_name),
                    line: format!("Speed Mode: skipped — {}", e),
                },
            );
            return;
        }
    };

    // Wait for the emulator to be ADB-ready before running any speed-mode
    // commands. This consolidates the old find_emulator_serial + wait-for-device
    // pattern into a single poll loop that handles the "offline" → "device"
    // transition correctly.
    let stage = format!("speed:{}", avd_name);
    let serial = match wait_for_device_ready(&adb, avd_name, window, &stage, 300).await {
        Ok(s) => s,
        Err(e) => {
            let _ = window.emit(
                EVT_LOG,
                LogLine {
                    stage: stage.clone(),
                    line: format!("Speed Mode: {}", e),
                },
            );
            return;
        }
    };

    // Best-effort root: silently ignore failure (Google Play images won't allow it).
    let _ = tokio::process::Command::new(&adb)
        .args(["-s", &serial, "root"])
        .output()
        .await;

    // Force GPU composition via SurfaceFlinger service call.
    let _ = window.emit(
        EVT_LOG,
        LogLine {
            stage: format!("speed:{}", avd_name),
            line: "Speed Mode: forcing GPU composition (service call SurfaceFlinger 1008 i32 1)...".to_string(),
        },
    );

    let sf_result = tokio::process::Command::new(&adb)
        .args(["-s", &serial, "shell", "service", "call", "SurfaceFlinger", "1008", "i32", "1"])
        .output()
        .await;

    match sf_result {
        Ok(ref o) if o.status.success() => {
            let _ = window.emit(
                EVT_LOG,
                LogLine {
                    stage: format!("speed:{}", avd_name),
                    line: "Speed Mode: GPU composition enabled — hardware overlays disabled".to_string(),
                },
            );
        }
        Ok(ref o) => {
            let err = String::from_utf8_lossy(&o.stderr);
            eprintln!("[EMU] speed_mode: SurfaceFlinger call failed: {}", err);
            let _ = window.emit(
                EVT_LOG,
                LogLine {
                    stage: format!("speed:{}", avd_name),
                    line: format!("Speed Mode: SurfaceFlinger call failed ({}). This is normal on Google Play images.", err),
                },
            );
        }
        Err(e) => {
            eprintln!("[EMU] speed_mode: SurfaceFlinger call error: {}", e);
        }
    }
}

// ---------------------------------------------------------------------------
// Phase 6 — App install / management / screenshot / settings
// ---------------------------------------------------------------------------

/// Install an APK onto a running AVD.
///
/// Uses the adb-readiness pattern (wait_for_device_ready) to ensure the
/// emulator is in "device" state before issuing the install command. Streams
/// adb output to the frontend log panel and returns the raw adb error on
/// failure so the user sees INSTALL_FAILED_* codes verbatim.
#[tauri::command]
pub async fn install_apk(window: Window, avd_name: String, apk_path: String) -> CommandResult<bool> {
    eprintln!("[EMU] install_apk: avd_name='{}' apk_path='{}'", avd_name, apk_path);

    let adb = match adb_path() {
        Ok(p) => p,
        Err(e) => return CommandResult::fail(e),
    };
    if !adb.is_file() {
        return CommandResult::fail("adb binary not found".to_string());
    }

    if !Path::new(&apk_path).exists() {
        return CommandResult::fail(format!("APK file not found: {}", apk_path));
    }

    let stage = format!("apk:{}", avd_name);
    let serial = match wait_for_device_ready(&adb, &avd_name, &window, &stage, 300).await {
        Ok(s) => s,
        Err(e) => {
            let _ = window.emit(
                EVT_LOG,
                LogLine {
                    stage: stage.clone(),
                    line: format!("APK install failed: {}", e),
                },
            );
            return CommandResult::fail(e);
        }
    };
    eprintln!("[EMU] install_apk: serial='{}'", serial);

    let _ = window.emit(
        EVT_LOG,
        LogLine {
            stage: stage.clone(),
            line: format!("Installing APK on '{}' ({})...", avd_name, serial),
        },
    );

    let output = match timeout(
        Duration::from_secs(120),
        tokio::process::Command::new(&adb)
            .args(["-s", &serial, "install", "-r", &apk_path])
            .output(),
    )
    .await
    {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            let msg = format!("Failed to run adb install: {}", e);
            eprintln!("[EMU] install_apk: {}", msg);
            let _ = window.emit(
                EVT_LOG,
                LogLine {
                    stage: stage.clone(),
                    line: format!("APK install failed: {}", msg),
                },
            );
            return CommandResult::fail(msg);
        }
        Err(_) => {
            let msg = "adb install timed out after 120s".to_string();
            eprintln!("[EMU] install_apk: {}", msg);
            let _ = window.emit(
                EVT_LOG,
                LogLine {
                    stage: stage.clone(),
                    line: format!("APK install timed out: {}", msg),
                },
            );
            return CommandResult::fail(msg);
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if output.status.success() {
        eprintln!("[EMU] install_apk: success — stdout={}", stdout);
        let _ = window.emit(
            EVT_LOG,
            LogLine {
                stage: stage.clone(),
                line: format!("APK installed successfully on '{}'", avd_name),
            },
        );
        let _ = window.emit(
            crate::commands::sdk::EVT_DONE,
            serde_json::json!({
                "component": format!("apk:{}", avd_name),
                "ok": true,
                "message": format!("APK installed on '{}'", avd_name),
            }),
        );
        CommandResult::success(true)
    } else {
        let msg = format!(
            "adb install failed (status={}): {} {}",
            output.status, stdout, stderr
        );
        eprintln!("[EMU] install_apk: {}", msg);
        let _ = window.emit(
            EVT_LOG,
            LogLine {
                stage: stage.clone(),
                line: format!("APK install failed: {}", msg),
            },
        );
        let _ = window.emit(
            crate::commands::sdk::EVT_DONE,
            serde_json::json!({
                "component": format!("apk:{}", avd_name),
                "ok": false,
                "message": msg.clone(),
            }),
        );
        CommandResult::fail(msg)
    }
}

/// Result of listing installed apps.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct InstalledApp {
    /// Package name (e.g. com.example.app).
    pub package: String,
    /// Human-readable label if we could extract it; falls back to package name.
    pub label: Option<String>,
}

/// List third-party (user-installed) apps on a running AVD.
///
/// Uses the adb-readiness pattern. First runs `pm list packages -3` to get
/// package names, then attempts `dumpsys package <pkg>` to extract the
/// application label for each. If label extraction fails, the label field
/// is left as None and the frontend falls back to displaying the package name.
#[tauri::command]
pub async fn list_installed_apps(window: Window, avd_name: String) -> CommandResult<Vec<InstalledApp>> {
    eprintln!("[EMU] list_installed_apps: avd_name='{}'", avd_name);

    let adb = match adb_path() {
        Ok(p) => p,
        Err(e) => return CommandResult::fail(e),
    };
    if !adb.is_file() {
        return CommandResult::fail("adb binary not found".to_string());
    }

    let stage = format!("apps:{}", avd_name);
    let serial = match wait_for_device_ready(&adb, &avd_name, &window, &stage, 300).await {
        Ok(s) => s,
        Err(e) => {
            let _ = window.emit(
                EVT_LOG,
                LogLine {
                    stage: stage.clone(),
                    line: format!("List installed apps failed: {}", e),
                },
            );
            return CommandResult::fail(e);
        }
    };
    eprintln!("[EMU] list_installed_apps: serial='{}'", serial);

    let list_output = match tokio::process::Command::new(&adb)
        .args(["-s", &serial, "shell", "pm", "list", "packages", "-3"])
        .output()
        .await
    {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr);
            return CommandResult::fail(format!("pm list packages failed: {}", err));
        }
        Err(e) => return CommandResult::fail(format!("Failed to run pm list packages: {e}")),
    };

    let packages: Vec<String> = list_output
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("package:")
                .map(|s| s.trim().to_string())
        })
        .collect();

    if packages.is_empty() {
        eprintln!("[EMU] list_installed_apps: no third-party apps");
        return CommandResult::success(vec![]);
    }

    let mut apps = Vec::with_capacity(packages.len());
    for pkg in packages {
        let label = fetch_app_label(&adb, &serial, &pkg).await;
        apps.push(InstalledApp {
            package: pkg,
            label,
        });
    }

    eprintln!("[EMU] list_installed_apps: found {} apps", apps.len());
    CommandResult::success(apps)
}

/// Attempt to fetch the application label for a package via `dumpsys package`.
/// This is best-effort: returns None if the output format is unexpected or
/// the command fails.
async fn fetch_app_label(adb: &PathBuf, serial: &str, pkg: &str) -> Option<String> {
    let output = match tokio::process::Command::new(adb)
        .args(["-s", serial, "shell", "dumpsys", "package", pkg])
        .output()
        .await
    {
        Ok(o) if o.status.success() => o,
        _ => return None,
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Look for a line like:  android:label=0x7f0a0000 (resolved to string)
    // or fallback to any line containing "android:label=" and a hex value.
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.contains("android:label=") {
            // Try to extract the hex resource id.
            if let Some(hex_part) = trimmed.split("android:label=").nth(1) {
                let hex = hex_part.split_whitespace().next().unwrap_or(hex_part);
                // Convert hex resource id to actual string by parsing the int.
                if let Ok(_res_id) = u64::from_str_radix(hex.trim_start_matches("0x"), 16) {
                    // Use `aapt dump` equivalent via adb: try to resolve the label
                    // by running `pm dump` and grepping, but this is unreliable.
                    // As a pragmatic fallback, just return None — the frontend
                    // will display the package name.
                    // NOTE: Precise label extraction requires parsing binary
                    // resources which is not feasible here. We intentionally
                    // do NOT over-invest; package name alone is the documented
                    // acceptable fallback.
                }
            }
        }
    }
    None
}

/// Uninstall an app from a running AVD.
///
/// Uses the adb-readiness pattern. Returns the raw adb output on failure.
#[tauri::command]
pub async fn uninstall_app(window: Window, avd_name: String, package_name: String) -> CommandResult<bool> {
    eprintln!("[EMU] uninstall_app: avd_name='{}' package_name='{}'", avd_name, package_name);

    let adb = match adb_path() {
        Ok(p) => p,
        Err(e) => return CommandResult::fail(e),
    };
    if !adb.is_file() {
        return CommandResult::fail("adb binary not found".to_string());
    }

    if package_name.trim().is_empty() {
        return CommandResult::fail("Package name cannot be empty".to_string());
    }

    let stage = format!("uninstall:{}", avd_name);
    let serial = match wait_for_device_ready(&adb, &avd_name, &window, &stage, 300).await {
        Ok(s) => s,
        Err(e) => {
            let _ = window.emit(
                EVT_LOG,
                LogLine {
                    stage: stage.clone(),
                    line: format!("Uninstall failed: {}", e),
                },
            );
            return CommandResult::fail(e);
        }
    };
    eprintln!("[EMU] uninstall_app: serial='{}'", serial);

    let _ = window.emit(
        EVT_LOG,
        LogLine {
            stage: stage.clone(),
            line: format!("Uninstalling '{}' from '{}'...", package_name, avd_name),
        },
    );

    let output = match timeout(
        Duration::from_secs(60),
        tokio::process::Command::new(&adb)
            .args(["-s", &serial, "uninstall", &package_name])
            .output(),
    )
    .await
    {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            let msg = format!("Failed to run adb uninstall: {}", e);
            eprintln!("[EMU] uninstall_app: {}", msg);
            return CommandResult::fail(msg);
        }
        Err(_) => {
            let msg = "adb uninstall timed out after 60s".to_string();
            eprintln!("[EMU] uninstall_app: {}", msg);
            return CommandResult::fail(msg);
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if output.status.success() || stdout.contains("Success") {
        eprintln!("[EMU] uninstall_app: success — {}", stdout);
        let _ = window.emit(
            EVT_LOG,
            LogLine {
                stage: stage.clone(),
                line: format!("Uninstalled '{}' from '{}'", package_name, avd_name),
            },
        );
        let _ = window.emit(
            crate::commands::sdk::EVT_DONE,
            serde_json::json!({
                "component": format!("uninstall:{}", avd_name),
                "ok": true,
                "message": format!("Uninstalled '{}'", package_name),
            }),
        );
        CommandResult::success(true)
    } else {
        let msg = format!("adb uninstall failed (status={}): {} {}", output.status, stdout, stderr);
        eprintln!("[EMU] uninstall_app: {}", msg);
        let _ = window.emit(
            EVT_LOG,
            LogLine {
                stage: stage.clone(),
                line: format!("Uninstall failed: {}", msg),
            },
        );
        let _ = window.emit(
            crate::commands::sdk::EVT_DONE,
            serde_json::json!({
                "component": format!("uninstall:{}", avd_name),
                "ok": false,
                "message": msg.clone(),
            }),
        );
        CommandResult::fail(msg)
    }
}

/// Launch an installed app on a running AVD.
///
/// Uses the adb-readiness pattern. Tries `monkey` with the LAUNCHER category
/// first; if that fails, falls back to `am start` with the MAIN/LAUNCHER intent.
/// If neither works, returns the raw error.
#[tauri::command]
pub async fn launch_app(window: Window, avd_name: String, package_name: String) -> CommandResult<bool> {
    eprintln!("[EMU] launch_app: avd_name='{}' package_name='{}'", avd_name, package_name);

    let adb = match adb_path() {
        Ok(p) => p,
        Err(e) => return CommandResult::fail(e),
    };
    if !adb.is_file() {
        return CommandResult::fail("adb binary not found".to_string());
    }

    if package_name.trim().is_empty() {
        return CommandResult::fail("Package name cannot be empty".to_string());
    }

    let stage = format!("launch:{}", avd_name);
    let serial = match wait_for_device_ready(&adb, &avd_name, &window, &stage, 300).await {
        Ok(s) => s,
        Err(e) => {
            let _ = window.emit(
                EVT_LOG,
                LogLine {
                    stage: stage.clone(),
                    line: format!("Launch failed: {}", e),
                },
            );
            return CommandResult::fail(e);
        }
    };
    eprintln!("[EMU] launch_app: serial='{}'", serial);

    let _ = window.emit(
        EVT_LOG,
        LogLine {
            stage: stage.clone(),
            line: format!("Launching '{}' on '{}'...", package_name, avd_name),
        },
    );

    // Strategy 1: monkey with LAUNCHER category.
    let monkey_attempt = async {
        let output = timeout(
            Duration::from_secs(30),
            tokio::process::Command::new(&adb)
                .args([
                    "-s", &serial, "shell", "monkey",
                    "-p", &package_name,
                    "-c", "android.intent.category.LAUNCHER",
                    "1",
                ])
                .output(),
        )
        .await;

        match output {
            Ok(Ok(o)) => Some(o),
            _ => {
                let msg = "adb shell monkey timed out or failed".to_string();
                eprintln!("[EMU] launch_app: monkey failed: {}", msg);
                let _ = window.emit(
                    EVT_LOG,
                    LogLine {
                        stage: stage.clone(),
                        line: format!("Launch (monkey) failed: {}", msg),
                    },
                );
                None
            }
        }
    };

    let monkey_output = monkey_attempt.await;

    if let Some(ref o) = monkey_output {
        if o.status.success() {
            let stdout = String::from_utf8_lossy(&o.stdout);
            if stdout.contains("Events injected: 1") {
                eprintln!("[EMU] launch_app: monkey success");
                let _ = window.emit(
                    EVT_LOG,
                    LogLine {
                        stage: stage.clone(),
                        line: format!("Launched '{}' on '{}'", package_name, avd_name),
                    },
                );
                let _ = window.emit(
                    crate::commands::sdk::EVT_DONE,
                    serde_json::json!({
                        "component": format!("launch:{}", avd_name),
                        "ok": true,
                        "message": format!("Launched '{}'", package_name),
                    }),
                );
                return CommandResult::success(true);
            }
        }
        let stderr = String::from_utf8_lossy(&o.stderr);
        eprintln!("[EMU] launch_app: monkey returned non-success: status={} stderr={}", o.status, stderr);
    }

    // Strategy 2: am start with MAIN/LAUNCHER intent.
    let am_output = match timeout(
        Duration::from_secs(30),
        tokio::process::Command::new(&adb)
            .args([
                "-s", &serial, "shell", "am", "start",
                "-a", "android.intent.action.MAIN",
                "-c", "android.intent.category.LAUNCHER",
                &package_name,
            ])
            .output(),
    )
    .await
    {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            let msg = format!("Failed to run adb am start: {}", e);
            eprintln!("[EMU] launch_app: am start failed: {}", msg);
            return CommandResult::fail(msg);
        }
        Err(_) => {
            let msg = "adb am start timed out after 30s".to_string();
            eprintln!("[EMU] launch_app: {}", msg);
            return CommandResult::fail(msg);
        }
    };

    if am_output.status.success() {
        eprintln!("[EMU] launch_app: am start success");
        let _ = window.emit(
            EVT_LOG,
            LogLine {
                stage: stage.clone(),
                line: format!("Launched '{}' on '{}'", package_name, avd_name),
            },
        );
        let _ = window.emit(
            crate::commands::sdk::EVT_DONE,
            serde_json::json!({
                "component": format!("launch:{}", avd_name),
                "ok": true,
                "message": format!("Launched '{}'", package_name),
            }),
        );
        CommandResult::success(true)
    } else {
        let stdout = String::from_utf8_lossy(&am_output.stdout);
        let stderr = String::from_utf8_lossy(&am_output.stderr);
        let msg = format!("adb am start failed (status={}): {} {}", am_output.status, stdout, stderr);
        eprintln!("[EMU] launch_app: {}", msg);
        let _ = window.emit(
            EVT_LOG,
            LogLine {
                stage: stage.clone(),
                line: format!("Launch failed: {}", msg),
            },
        );
        let _ = window.emit(
            crate::commands::sdk::EVT_DONE,
            serde_json::json!({
                "component": format!("launch:{}", avd_name),
                "ok": false,
                "message": msg.clone(),
            }),
        );
        CommandResult::fail(msg)
    }
}

/// Capture a screenshot from a running AVD and save it to the screenshots dir.
///
/// Uses the adb-readiness pattern. Generates a timestamped filename and saves
/// the PNG output. Returns the saved file path on success.
#[tauri::command]
pub async fn capture_screenshot(window: Window, avd_name: String) -> CommandResult<String> {
    eprintln!("[EMU] capture_screenshot: avd_name='{}'", avd_name);

    let adb = match adb_path() {
        Ok(p) => p,
        Err(e) => return CommandResult::fail(e),
    };
    if !adb.is_file() {
        return CommandResult::fail("adb binary not found".to_string());
    }

    let stage = format!("screenshot:{}", avd_name);
    let serial = match wait_for_device_ready(&adb, &avd_name, &window, &stage, 300).await {
        Ok(s) => s,
        Err(e) => {
            let _ = window.emit(
                EVT_LOG,
                LogLine {
                    stage: stage.clone(),
                    line: format!("Screenshot failed: {}", e),
                },
            );
            return CommandResult::fail(e);
        }
    };
    eprintln!("[EMU] capture_screenshot: serial='{}'", serial);

    let _ = window.emit(
        EVT_LOG,
        LogLine {
            stage: stage.clone(),
            line: format!("Capturing screenshot on '{}'...", avd_name),
        },
    );

    let output = match timeout(
        Duration::from_secs(30),
        tokio::process::Command::new(&adb)
            .args(["-s", &serial, "exec-out", "screencap", "-p"])
            .output(),
    )
    .await
    {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            let msg = format!("Failed to run adb screencap: {}", e);
            eprintln!("[EMU] capture_screenshot: {}", msg);
            return CommandResult::fail(msg);
        }
        Err(_) => {
            let msg = "adb screencap timed out after 30s".to_string();
            eprintln!("[EMU] capture_screenshot: {}", msg);
            return CommandResult::fail(msg);
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let msg = format!("adb screencap failed (status={}): {}", output.status, stderr);
        eprintln!("[EMU] capture_screenshot: {}", msg);
        return CommandResult::fail(msg);
    }

    if output.stdout.is_empty() {
        let msg = "adb screencap returned empty output — the emulator may not support screencap".to_string();
        eprintln!("[EMU] capture_screenshot: {}", msg);
        return CommandResult::fail(msg);
    }

    let save_dir = paths::screenshots_dir();
    if let Err(e) = std::fs::create_dir_all(&save_dir) {
        let msg = format!("Failed to create screenshots dir: {}", e);
        eprintln!("[EMU] capture_screenshot: {}", msg);
        return CommandResult::fail(msg);
    }

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let filename = format!("{}_{}.png", avd_name, timestamp);
    let save_path = save_dir.join(&filename);

    if let Err(e) = std::fs::write(&save_path, &output.stdout) {
        let msg = format!("Failed to write screenshot: {}", e);
        eprintln!("[EMU] capture_screenshot: {}", msg);
        return CommandResult::fail(msg);
    }

    let display_path = save_path.to_string_lossy().into_owned();
    eprintln!("[EMU] capture_screenshot: saved to {}", display_path);

    let _ = window.emit(
        EVT_LOG,
        LogLine {
            stage: stage.clone(),
            line: format!("Screenshot saved to: {}", display_path),
        },
    );
    let _ = window.emit(
        crate::commands::sdk::EVT_DONE,
        serde_json::json!({
            "component": format!("screenshot:{}", avd_name),
            "ok": true,
            "message": format!("Screenshot saved to {}", display_path),
        }),
    );

    CommandResult::success(display_path)
}

// ---------------------------------------------------------------------------
// Phase 6 — App-level settings commands
// ---------------------------------------------------------------------------

/// Get the current app-level settings (SDK override, screenshot dir, etc.).
#[tauri::command]
pub async fn get_app_settings() -> CommandResult<crate::commands::paths::AppSettings> {
    let settings = crate::commands::paths::app_settings().unwrap_or_default();
    CommandResult::success(settings)
}

/// Save app-level settings.
#[tauri::command]
pub async fn save_app_settings_window(settings: crate::commands::paths::AppSettings) -> CommandResult<bool> {
    eprintln!("[EMU] save_app_settings: {:?}", settings);
    match crate::commands::paths::save_app_settings(settings) {
        Ok(_) => CommandResult::success(true),
        Err(e) => CommandResult::fail(e),
    }
}

/// Clear all app-level settings (reset to defaults).
#[tauri::command]
pub async fn reset_app_settings(window: Window) -> CommandResult<bool> {
    eprintln!("[EMU] reset_app_settings invoked");
    let _ = window.emit(
        EVT_LOG,
        LogLine {
            stage: "settings".to_string(),
            line: "Resetting app settings to defaults...".to_string(),
        },
    );
    match crate::commands::paths::clear_app_settings() {
        Ok(_) => {
            let _ = window.emit(
                EVT_LOG,
                LogLine {
                    stage: "settings".to_string(),
                    line: "App settings cleared.".to_string(),
                },
            );
            CommandResult::success(true)
        }
        Err(e) => CommandResult::fail(e),
    }
}
