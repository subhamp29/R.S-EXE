//! Backend commands for Android SDK management.
//!
//! Every long-running operation (download, extraction, sdkmanager invocation) is
//! non-blocking: downloads stream over async `reqwest`; extraction of the
//! synchronous `zip` crate runs inside `tokio::task::spawn_blocking` and forwards
//! per-entry progress back through an mpsc channel; subprocess output is read
//! as raw bytes with manual \r/\n splitting so that sdkmanager progress-bar
//! ticks (which use \r to overwrite the same line) are delivered to the
//! frontend in real time.
//!
//! Install tasks are spawned with `tauri::async_runtime::spawn` so they run to
//! completion regardless of whether the originating frontend `invoke()` caller
//! is still listening. Progress and logs are also mirrored into a shared
//! `InstallState` (managed via `tauri::State`) so the frontend can recover
//! after navigation / remount by querying `get_install_progress`.
use crate::commands::CommandResult;
use crate::commands::paths;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use tauri::Emitter;
use tauri::Window;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Real install state of the SDK pieces we care about.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SdkStatus {
    pub jdk: bool,
    pub cmdline_tools: bool,
    pub platform_tools: bool,
    pub emulator: bool,
}

/// A single package entry from `sdkmanager --list`.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SdkPackage {
    pub id: String,
    pub name: String,
    pub desc: String,
    pub version: String,
    pub installed: bool,
    pub category: String,
}

/// Progress payload emitted during downloads / extractions.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct InstallProgress {
    pub component: String,
    pub percent: u32,
    pub message: String,
    pub bytes_done: u64,
    pub bytes_total: Option<u64>,
}

/// Log line payload for streamed subprocess output.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LogLine {
    pub stage: String,
    pub line: String,
}

// ---------------------------------------------------------------------------
// Shared install state (survives component unmount / navigation)
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct ComponentInstallState {
    pub installing: bool,
    pub progress: Option<InstallProgress>,
    pub done: Option<serde_json::Value>,
    pub logs: Vec<LogLine>,
    pub error: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct InstallState {
    pub jdk: ComponentInstallState,
    pub cmdline_tools: ComponentInstallState,
    pub platform_tools: ComponentInstallState,
    pub emulator: ComponentInstallState,
    pub licenses: ComponentInstallState,
    // package-id -> state (for install/uninstall of arbitrary packages)
    pub packages: std::collections::HashMap<String, ComponentInstallState>,
}

impl InstallState {
    fn get_mut(&mut self, component: &str) -> &mut ComponentInstallState {
        match component {
            "jdk" => &mut self.jdk,
            "cmdline-tools" => &mut self.cmdline_tools,
            "platform-tools" => &mut self.platform_tools,
            "emulator" => &mut self.emulator,
            "licenses" => &mut self.licenses,
            other => self.packages.entry(other.to_string()).or_default(),
        }
    }
}

// We use a `pub use` in commands/mod.rs so main.rs can initialize it.
pub type SharedInstallState = Arc<Mutex<InstallState>>;

static INSTALL_STATE: OnceLock<SharedInstallState> = OnceLock::new();

pub fn get_shared_install_state() -> SharedInstallState {
    INSTALL_STATE
        .get_or_init(|| Arc::new(Mutex::new(InstallState::default())))
        .clone()
}

// ---------------------------------------------------------------------------
// Event names (must stay in sync with the frontend)
// ---------------------------------------------------------------------------

pub const EVT_PROGRESS: &str = "sdk-install-progress";
pub const EVT_DONE: &str = "sdk-install-done";
pub const EVT_LOG: &str = "sdk-log";

// ---------------------------------------------------------------------------
// Download URLs
// ---------------------------------------------------------------------------
//
// JDK: we use the Adoptium (Eclipse Temurin) API.
//   Why Temurin over Microsoft OpenJDK:
//     * api.adoptium.net is purpose-built for programmatic download and returns
//       a direct binary stream with a reliable Content-Length, which lets us
//       emit genuine byte-progress events.
//     * Temurin is the community-standard JDK used by Android tooling and
//       Google's own documentation.
//     * Microsoft OpenJDK publishes to versioned, mutable download.microsoft.com
//       paths that change per release and lack a stable "latest" redirect,
//       making them fragile for automation.

fn jdk_download_url() -> String {
    let (os, arch) = native_target();
    format!(
        "https://api.adoptium.net/v3/binary/latest/17/ga/{os}/{arch}/jdk/hotspot/normal/eclipse"
    )
}

const CMDLINE_TOOLS_URL_WIN: &str =
    "https://dl.google.com/android/repository/commandlinetools-win-11076708_latest.zip";

fn cmdline_tools_url() -> String {
    let (os, arch) = native_target();
    if cfg!(windows) {
        CMDLINE_TOOLS_URL_WIN.to_string()
    } else {
        format!(
            "https://dl.google.com/android/repository/commandlinetools-{os}-{arch}-11076708_latest.zip"
        )
    }
}

fn native_target() -> (String, String) {
    let os = if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "mac"
    };
    let arch = if cfg!(target_arch = "x86_64") {
        "x64"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x64"
    };
    (os.to_string(), arch.to_string())
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Real install state by probing the actual files on disk.
#[tauri::command]
pub fn check_install_status() -> CommandResult<SdkStatus> {
    eprintln!(
        "[DEBUG] check_install_status: jdk={} cmdline={} platform-tools={} emulator={}",
        paths::jdk_installed(),
        paths::cmdline_installed(),
        paths::platform_tools_installed(),
        paths::emulator_installed()
    );
    CommandResult::success(SdkStatus {
        jdk: paths::jdk_installed(),
        cmdline_tools: paths::cmdline_installed(),
        platform_tools: paths::platform_tools_installed(),
        emulator: paths::emulator_installed(),
    })
}

/// Return the shared install state so the frontend can recover after navigation.
#[tauri::command]
pub fn get_install_progress() -> CommandResult<InstallState> {
    let state = get_shared_install_state();
    let guard = state.lock().unwrap();
    eprintln!("[DEBUG] get_install_progress: jdk={} cmdline={} platform-tools={} emulator={}",
        guard.jdk.installing, guard.cmdline_tools.installing, guard.platform_tools.installing, guard.emulator.installing);
    CommandResult::success(guard.clone())
}

/// Download a JDK (Temurin) and extract it into `jdk_dir()`.
/// Returns immediately with `started: true`; the actual work continues in a
/// fire-and-forget task so it survives frontend disconnection.
#[tauri::command]
pub async fn install_jdk(window: Window) -> CommandResult<bool> {
    eprintln!("[SDK] install_jdk command RECEIVED");
    let component = "jdk".to_string();
    if paths::jdk_installed() {
        eprintln!("[SDK] install_jdk: JDK already installed on disk, returning early");
        let _ = window.emit(EVT_DONE, done_payload(&component, true, "JDK already installed"));
        return CommandResult::success(true);
    }

    // Idempotency: if an install is already in flight, don't start another.
    let state = get_shared_install_state();
    {
        let guard = state.lock().unwrap();
        if guard.jdk.installing {
            eprintln!("[SDK] install_jdk: already in progress, skipping duplicate");
            return CommandResult::success(true);
        }
    }

    let url = jdk_download_url();
    let zip_path = cache_path("jdk-download.zip");
    let win = window.clone();

    eprintln!("[SDK] install_jdk: spawning fire-and-forget task, url={}", url);

    tauri::async_runtime::spawn(async move {
        eprintln!("[SDK] install_jdk task STARTED");
        {
            let mut guard = state.lock().unwrap();
            guard.get_mut(&component).installing = true;
            guard.get_mut(&component).error = None;
            guard.get_mut(&component).logs.clear();
            guard.get_mut(&component).progress = None;
            guard.get_mut(&component).done = None;
        } // guard dropped here, before any await

        let _ = win.emit(EVT_LOG, LogLine { stage: component.clone(), line: format!("Downloading JDK from {}", url) });
        let _ = win.emit(EVT_PROGRESS, progress(&component, 0, "Starting JDK download…"));
        let result = download_with_progress(&url, &zip_path, &win, &component).await;

        match result {
            Ok(()) => {
                let _ = win.emit(EVT_LOG, LogLine { stage: component.clone(), line: "JDK downloaded; extracting".into() });
                let _ = win.emit(EVT_PROGRESS, progress(&component, 0, "Extracting JDK…"));
                match extract_and_consolidate(&zip_path, paths::jdk_dir(), &win, &component).await {
                    Ok(()) => {
                        let _ = std::fs::remove_file(&zip_path).ok();
                        let _ = win.emit(EVT_DONE, done_payload(&component, true, "JDK installed"));
                        eprintln!("[SDK] install_jdk: SUCCESS");
                        let mut guard = state.lock().unwrap();
                        guard.get_mut(&component).installing = false;
                        guard.get_mut(&component).done = Some(done_payload(&component, true, "JDK installed"));
                        guard.get_mut(&component).progress = None;
                    }
                    Err(e) => {
                        eprintln!("[SDK] install_jdk: extraction FAILED: {}", e);
                        let _ = win.emit(EVT_DONE, done_payload(&component, false, &e));
                        let mut guard = state.lock().unwrap();
                        guard.get_mut(&component).installing = false;
                        guard.get_mut(&component).error = Some(e.clone());
                        guard.get_mut(&component).done = Some(done_payload(&component, false, &e));
                    }
                }
            }
            Err(e) => {
                eprintln!("[SDK] install_jdk: download FAILED: {}", e);
                let _ = win.emit(EVT_DONE, done_payload(&component, false, &e));
                let mut guard = state.lock().unwrap();
                guard.get_mut(&component).installing = false;
                guard.get_mut(&component).error = Some(e.clone());
                guard.get_mut(&component).done = Some(done_payload(&component, false, &e));
            }
        }
    });

    eprintln!("[SDK] install_jdk: returned started=true to frontend");
    CommandResult::success(true)
}

/// Download Android cmdline-tools and extract into `cmdline_dir()`.
/// Returns immediately with `started: true`; actual work continues in a
/// fire-and-forget task.
#[tauri::command]
pub async fn install_cmdline_tools(window: Window) -> CommandResult<bool> {
    eprintln!("[SDK] install_cmdline_tools command RECEIVED");
    let component = "cmdline-tools".to_string();
    if paths::cmdline_installed() {
        eprintln!("[SDK] install_cmdline_tools: already installed on disk, returning early");
        let _ = window
            .emit(EVT_DONE, done_payload(&component, true, "cmdline-tools already installed"));
        return CommandResult::success(true);
    }

    let state = get_shared_install_state();
    {
        let guard = state.lock().unwrap();
        if guard.cmdline_tools.installing {
            eprintln!("[SDK] install_cmdline_tools: already in progress, skipping duplicate");
            return CommandResult::success(true);
        }
    }

    let url = cmdline_tools_url();
    let zip_path = cache_path("cmdline-download.zip");
    let win = window.clone();

    eprintln!("[SDK] install_cmdline_tools: spawning fire-and-forget task, url={}", url);

    tauri::async_runtime::spawn(async move {
        eprintln!("[SDK] install_cmdline_tools task STARTED");
        {
            let mut guard = state.lock().unwrap();
            guard.get_mut(&component).installing = true;
            guard.get_mut(&component).error = None;
            guard.get_mut(&component).logs.clear();
            guard.get_mut(&component).progress = None;
            guard.get_mut(&component).done = None;
        }

        let _ = win.emit(EVT_LOG, LogLine { stage: component.clone(), line: format!("Downloading cmdline-tools from {}", url) });
        let _ = win.emit(EVT_PROGRESS, progress(&component, 0, "Starting cmdline-tools download…"));
        let result = download_with_progress(&url, &zip_path, &win, &component).await;

        match result {
            Ok(()) => {
                let _ = win.emit(EVT_LOG, LogLine { stage: component.clone(), line: "cmdline-tools downloaded; extracting".into() });
                let _ = win.emit(EVT_PROGRESS, progress(&component, 0, "Extracting cmdline-tools…"));
                match extract_and_consolidate(&zip_path, paths::cmdline_dir(), &win, &component).await {
                    Ok(()) => {
                        let _ = std::fs::remove_file(&zip_path).ok();
                        let _ = win.emit(EVT_DONE, done_payload(&component, true, "cmdline-tools installed"));
                        eprintln!("[SDK] install_cmdline_tools: SUCCESS");
                        let mut guard = state.lock().unwrap();
                        guard.get_mut(&component).installing = false;
                        guard.get_mut(&component).done = Some(done_payload(&component, true, "cmdline-tools installed"));
                        guard.get_mut(&component).progress = None;
                    }
                    Err(e) => {
                        eprintln!("[SDK] install_cmdline_tools: extraction FAILED: {}", e);
                        let _ = win.emit(EVT_DONE, done_payload(&component, false, &e));
                        let mut guard = state.lock().unwrap();
                        guard.get_mut(&component).installing = false;
                        guard.get_mut(&component).error = Some(e.clone());
                        guard.get_mut(&component).done = Some(done_payload(&component, false, &e));
                    }
                }
            }
            Err(e) => {
                eprintln!("[SDK] install_cmdline_tools: download FAILED: {}", e);
                let _ = win.emit(EVT_DONE, done_payload(&component, false, &e));
                let mut guard = state.lock().unwrap();
                guard.get_mut(&component).installing = false;
                guard.get_mut(&component).error = Some(e.clone());
                guard.get_mut(&component).done = Some(done_payload(&component, false, &e));
            }
        }
    });

    eprintln!("[SDK] install_cmdline_tools: returned started=true to frontend");
    CommandResult::success(true)
}

/// Run `sdkmanager --licenses` and auto-answer "y" to every prompt.
/// Spawned as fire-and-forget; returns immediately with `started: true`.
#[tauri::command]
pub async fn accept_licenses(window: Window) -> CommandResult<bool> {
    let component = "licenses".to_string();
    let state = get_shared_install_state();

    // Idempotency check BEFORE spawning any process — prevents orphaned children
    // when called concurrently.
    {
        let guard = state.lock().unwrap();
        if guard.licenses.installing {
            eprintln!("[SDK] accept_licenses: already in progress, skipping duplicate");
            return CommandResult::success(true);
        }
    }

    let mut cmd = match sdkmanager_command(&["--licenses"]) {
        Ok(c) => c,
        Err(e) => return CommandResult::fail(e),
    };
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let msg = format!("Failed to launch sdkmanager for licenses: {e}");
            eprintln!("[SDK] accept_licenses: {}", msg);
            let _ = window.emit(EVT_DONE, done_payload(&component, false, &msg));
            let mut guard = state.lock().unwrap();
            guard.get_mut(&component).installing = false;
            guard.get_mut(&component).error = Some(msg.clone());
            guard.get_mut(&component).done = Some(done_payload(&component, false, &msg));
            return CommandResult::fail(msg);
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin
            .write_all(b"y\ny\ny\ny\ny\ny\ny\ny\ny\ny\n")
            .await;
    }

    let win = window.clone();
    let component_for_log = component.clone();

    eprintln!("[SDK] accept_licenses: spawning fire-and-forget task");

    tauri::async_runtime::spawn(async move {
        {
            let mut guard = state.lock().unwrap();
            guard.get_mut(&component_for_log).installing = true;
            guard.get_mut(&component_for_log).error = None;
            guard.get_mut(&component_for_log).logs.clear();
        }

        let output = stream_child_output(&mut child, &win, &component_for_log).await;

        let status = match child.wait().await {
            Ok(s) => s,
            Err(e) => {
                let msg = format!(
                    "sdkmanager --licenses exited abnormally: {e}\n--- sdkmanager output ---\n{output}"
                );
                eprintln!("[SDK] accept_licenses: {}", msg);
                let _ = win.emit(EVT_DONE, done_payload(&component_for_log, false, &msg));
                let mut guard = state.lock().unwrap();
                guard.get_mut(&component_for_log).installing = false;
                guard.get_mut(&component_for_log).error = Some(msg.clone());
                guard.get_mut(&component_for_log).done = Some(done_payload(&component_for_log, false, &msg));
                return;
            }
        };

        let ok = status.success();
        let msg = if ok {
            "Licenses accepted".to_string()
        } else {
            format!(
                "sdkmanager --licenses -> {}\n--- sdkmanager output ---\n{}",
                status, output
            )
        };
        eprintln!("[SDK] accept_licenses: ok={} msg={}", ok, msg);
        let _ = win.emit(EVT_DONE, done_payload(&component_for_log, ok, &msg));
        let mut guard = state.lock().unwrap();
        guard.get_mut(&component_for_log).installing = false;
        if !ok {
            guard.get_mut(&component_for_log).error = Some(msg.clone());
        }
        guard.get_mut(&component_for_log).done = Some(done_payload(&component_for_log, ok, &msg));
    });

    eprintln!("[SDK] accept_licenses: returned started=true to frontend");
    CommandResult::success(true)
}

/// Parse `sdkmanager --list` into categorized package entries.
#[tauri::command]
pub async fn fetch_sdk_packages(window: Window) -> CommandResult<Vec<SdkPackage>> {
    let mut cmd = match sdkmanager_command(&["--list"]) {
        Ok(c) => c,
        Err(e) => return CommandResult::fail(e),
    };
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return CommandResult::fail(format!(
                "Failed to launch sdkmanager --list: {e}"
            ))
        }
    };

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let win = window.clone();

    // Read stdout (capturing it for parsing) and stderr concurrently to avoid
    // pipe-buffer deadlock.
    let win2 = win.clone();
    let t_out = read_crlf_lines(stdout, move |trimmed| {
        let _ = win2.emit(
            EVT_LOG,
            LogLine {
                stage: "list".into(),
                line: trimmed.to_string(),
            },
        );
    });
    let t_err = stream_lines(stderr, &window, "list");

    let (combined, stderr_output) = tokio::join!(t_out, t_err);

    let status = match child.wait().await {
        Ok(s) => s,
        Err(e) => {
            return CommandResult::fail(format!(
                "sdkmanager --list exited abnormally: {e}\n--- sdkmanager output ---\n{}{}",
                combined, stderr_output
            ))
        }
    };
    if !status.success() {
        let detail = if stderr_output.trim().is_empty() {
            combined.trim_end().to_string()
        } else {
            format!("{}{}", combined.trim_end(), stderr_output.trim_end())
        };
        return CommandResult::fail(format!(
            "sdkmanager --list exited with {} — output:\n{}",
            status, detail
        ));
    }

    CommandResult::success(parse_sdkmanager_list(&combined))
}

/// Install a package via `sdkmanager --install <id>` with live log stream.
/// Spawned as fire-and-forget; returns immediately with `started: true`.
#[tauri::command]
pub async fn install_package(
    window: Window,
    package: String,
) -> CommandResult<bool> {
    eprintln!("[SDK] install_package command RECEIVED for {}", package);
    if package.trim().is_empty() {
        return CommandResult::fail("Package id is empty".to_string());
    }
    // Safety net: refuse to install packages when the SDK licenses have not
    // been accepted.  sdkmanager would just fail with a cryptic exit code 1
    // ("license not accepted"), so we fail fast with a clear message instead.
    if !paths::licenses_accepted() {
        return CommandResult::fail(format!(
            "SDK licenses have not been accepted. \
             Run \"Accept Licenses\" (sdkmanager --licenses) first, then retry \
             installing {}. See {} for the license files.",
            package,
            paths::sdk_base().join("licenses").display()
        ));
    }
    let mut cmd = match sdkmanager_command(&["--install", &package]) {
        Ok(c) => c,
        Err(e) => return CommandResult::fail(e),
    };
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return CommandResult::fail(format!(
                "Failed to launch sdkmanager install: {e}"
            ))
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(b"y\ny\ny\ny\ny\n").await;
    }

    let win = window.clone();
    let pkg_for_log = package.clone();
    let key = format!("install:{}", package);
    let state = get_shared_install_state();

    {
        let guard = state.lock().unwrap();
        if guard.packages.get(&key).map(|c| c.installing).unwrap_or(false) {
            eprintln!("[SDK] install_package {}: already in progress, skipping duplicate", package);
            return CommandResult::success(true);
        }
    }

    eprintln!("[SDK] install_package: spawning fire-and-forget task for {}", package);

    tauri::async_runtime::spawn(async move {
        {
            let mut guard = state.lock().unwrap();
            guard.get_mut(&key).installing = true;
            guard.get_mut(&key).error = None;
            guard.get_mut(&key).logs.clear();
        }

        let output = stream_child_output(&mut child, &win, &format!("install:{}", pkg_for_log)).await;
        let status = match child.wait().await {
            Ok(s) => s,
            Err(e) => {
                let msg = format!(
                    "install exited abnormally: {e}\n--- sdkmanager output ---\n{output}"
                );
                eprintln!("[SDK] install_package {}: {}", pkg_for_log, msg);
                let _ = win.emit(EVT_DONE, done_payload(&key, false, &msg));
                let mut guard = state.lock().unwrap();
                guard.get_mut(&key).installing = false;
                guard.get_mut(&key).error = Some(msg);
                return;
            }
        };
        let ok = status.success();
        let msg = if ok {
            format!("sdkmanager install {} succeeded", pkg_for_log)
        } else {
            format!(
                "sdkmanager install {} -> {}\n--- sdkmanager output ---\n{}",
                pkg_for_log, status, output
            )
        };
        eprintln!("[SDK] install_package {}: ok={} msg={}", pkg_for_log, ok, msg);
        let _ = win.emit(EVT_DONE, done_payload(&key, ok, &msg));
        let mut guard = state.lock().unwrap();
        guard.get_mut(&key).installing = false;
        if !ok {
            guard.get_mut(&key).error = Some(msg.clone());
        }
        guard.get_mut(&key).done = Some(done_payload(&key, ok, &msg));
    });

    eprintln!("[SDK] install_package: returned started=true to frontend for {}", package);
    CommandResult::success(true)
}

/// Uninstall a package via `sdkmanager --uninstall <id>` with live log stream.
/// Spawned as fire-and-forget; returns immediately with `started: true`.
#[tauri::command]
pub async fn uninstall_package(window: Window, package: String) -> CommandResult<bool> {
    eprintln!("[SDK] uninstall_package command RECEIVED for {}", package);
    if package.trim().is_empty() {
        return CommandResult::fail("Package id is empty".to_string());
    }
    let mut cmd = match sdkmanager_command(&["--uninstall", &package]) {
        Ok(c) => c,
        Err(e) => return CommandResult::fail(e),
    };
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return CommandResult::fail(format!(
                "Failed to launch sdkmanager uninstall: {e}"
            ))
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(b"y\ny\ny\ny\ny\n").await;
    }

    let win = window.clone();
    let pkg_for_log = package.clone();
    let key = format!("uninstall:{}", package);
    let state = get_shared_install_state();

    {
        let guard = state.lock().unwrap();
        if guard.packages.get(&key).map(|c| c.installing).unwrap_or(false) {
            eprintln!("[SDK] uninstall_package {}: already in progress, skipping duplicate", package);
            return CommandResult::success(true);
        }
    }

    eprintln!("[SDK] uninstall_package: spawning fire-and-forget task for {}", package);

    tauri::async_runtime::spawn(async move {
        {
            let mut guard = state.lock().unwrap();
            guard.get_mut(&key).installing = true;
            guard.get_mut(&key).error = None;
            guard.get_mut(&key).logs.clear();
        }

        let output = stream_child_output(&mut child, &win, &format!("uninstall:{}", pkg_for_log)).await;
        let status = match child.wait().await {
            Ok(s) => s,
            Err(e) => {
                let msg = format!(
                    "uninstall exited abnormally: {e}\n--- sdkmanager output ---\n{output}"
                );
                eprintln!("[SDK] uninstall_package {}: {}", pkg_for_log, msg);
                let _ = win.emit(EVT_DONE, done_payload(&key, false, &msg));
                let mut guard = state.lock().unwrap();
                guard.get_mut(&key).installing = false;
                guard.get_mut(&key).error = Some(msg);
                return;
            }
        };
        let ok = status.success();
        let msg = if ok {
            format!("sdkmanager uninstall {} succeeded", pkg_for_log)
        } else {
            format!(
                "sdkmanager uninstall {} -> {}\n--- sdkmanager output ---\n{}",
                pkg_for_log, status, output
            )
        };
        eprintln!("[SDK] uninstall_package {}: ok={} msg={}", pkg_for_log, ok, msg);
        let _ = win.emit(EVT_DONE, done_payload(&key, ok, &msg));
        let mut guard = state.lock().unwrap();
        guard.get_mut(&key).installing = false;
        if !ok {
            guard.get_mut(&key).error = Some(msg.clone());
        }
        guard.get_mut(&key).done = Some(done_payload(&key, ok, &msg));
    });

    eprintln!("[SDK] uninstall_package: returned started=true to frontend for {}", package);
    CommandResult::success(true)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn done_payload(component: &str, ok: bool, message: &str) -> serde_json::Value {
    serde_json::json!({ "component": component, "ok": ok, "message": message })
}

fn progress(component: &str, percent: u32, message: &str) -> InstallProgress {
    InstallProgress {
        component: component.to_string(),
        percent,
        message: message.to_string(),
        bytes_done: 0,
        bytes_total: None,
    }
}

/// Local cache folder for in-flight downloads.
fn cache_path(name: &str) -> PathBuf {
    let dir = paths::sdk_base().join(".cache");
    let _ = std::fs::create_dir_all(&dir);
    dir.join(name)
}

/// Stream a remote URL to `dest`, emitting byte-progress events.
async fn download_with_progress(
    url: &str,
    dest: &Path,
    window: &Window,
    component: &str,
) -> Result<(), String> {
    eprintln!("[SDK] download_with_progress START: component={} url={} dest={}", component, url, dest.display());

    let client = reqwest::Client::builder()
        .user_agent("R.S EXE/0.1")
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {e}"))?;

    eprintln!("[SDK] download_with_progress: sending request to {}", url);

    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| {
            eprintln!("[SDK] download_with_progress: request FAILED: {}", e);
            format!("Download request failed: {e}")
        })?;

    eprintln!("[SDK] download_with_progress: response status={} content_length={:?}", resp.status(), resp.content_length());

    if !resp.status().is_success() {
        let err = format!(
            "Download failed with HTTP {} for {}",
            resp.status(),
            url
        );
        eprintln!("[SDK] download_with_progress: {}", err);
        return Err(err);
    }

    let total = resp.content_length();
    let mut file = tokio::fs::File::create(dest)
        .await
        .map_err(|e| {
            eprintln!("[SDK] download_with_progress: failed to create file {}: {}", dest.display(), e);
            format!("Failed to create download target {}: {e}", dest.display())
        })?;

    let mut stream = resp.bytes_stream();
    let mut done: u64 = 0;
    let mut chunks = 0u64;
    let mut last_logged_pct: u32 = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| {
            eprintln!("[SDK] download_with_progress: stream error after {} bytes: {}", done, e);
            format!("Download stream error: {e}")
        })?;
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("Failed writing download: {e}"))?;
        done += chunk.len() as u64;
        chunks += 1;
        if chunks % 50 == 0 {
            eprintln!("[SDK] download_with_progress: {} bytes downloaded ({} chunks)", done, chunks);
        }
        let pct = total
            .map(|t| if t > 0 { ((done * 100) / t).min(100) as u32 } else { 0 })
            .unwrap_or(0);
        // Log every percentage-point change so we can see if bytes are still arriving
        // or if the download has stalled at a particular percentage.
        if pct != last_logged_pct {
            eprintln!("[SDK] download_with_progress: {}% ({} bytes / {:?})", pct, done, total);
            last_logged_pct = pct;
        }
        let _ = window.emit(
            EVT_PROGRESS,
            InstallProgress {
                component: component.to_string(),
                percent: pct,
                message: format!(
                    "Downloading {} — {}/{} bytes",
                    component,
                    done,
                    total.map(|t| t.to_string()).unwrap_or_else(|| "?".into())
                ),
                bytes_done: done,
                bytes_total: total,
            },
        );
    }
    eprintln!("[SDK] download_with_progress COMPLETE: {} bytes in {} chunks", done, chunks);
    file.flush().await.ok();
    Ok(())
}

/// Extract a zip archive into a temp dir, emit per-entry progress, then move
/// the archive's single top-level folder (if any) into `dest` so the on-disk
/// layout matches what the Android tools expect.
async fn extract_and_consolidate(
    zip_path: &Path,
    dest: PathBuf,
    window: &Window,
    component: &str,
) -> Result<(), String> {
    eprintln!("[SDK] extract_and_consolidate START: component={} zip={} dest={}", component, zip_path.display(), dest.display());

    let dest_parent = dest
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let tmp = dest_parent
        .join(format!(".{}.extract", dest.file_name().unwrap().to_string_lossy()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)
        .map_err(|e| format!("Failed to create temp extract dir: {e}"))?;

    let zip_path = zip_path.to_path_buf();
    let comp = component.to_string();
    let (tx, mut rx) = mpsc::unbounded_channel::<InstallProgress>();
    let tx_clone = tx.clone();

    let handle = tokio::task::spawn_blocking(move || -> Result<(), String> {
        eprintln!("[SDK] extract_and_consolidate: spawn_blocking started for {}", comp);
        let r = extract_zip_sync(&zip_path, &tmp, &tx_clone);
        if r.is_ok() {
            let _ = consolidate_dir(&tmp, &dest);
        }
        let _ = tx_clone.send(InstallProgress {
            component: comp.clone(),
            percent: 100,
            message: format!("Extracted {} to {}", comp, dest.display()),
            bytes_done: 0,
            bytes_total: None,
        });
        r
    });

    let mut prog_count = 0u32;
    while let Some(prog) = rx.recv().await {
        prog_count += 1;
        if prog_count % 20 == 0 {
            eprintln!("[SDK] extract_and_consolidate: progress event #{} for {}: {}%", prog_count, component, prog.percent);
        }
        let _ = window.emit(EVT_PROGRESS, &prog);
    }
    eprintln!("[SDK] extract_and_consolidate: received {} progress events for {}", prog_count, component);

    let blocking_result = handle
        .await
        .map_err(|e| {
            eprintln!("[SDK] extract_and_consolidate: spawn_blocking PANICKED: {}", e);
            format!("Extraction task panicked: {e}")
        })?;

    blocking_result?;

    eprintln!("[SDK] extract_and_consolidate COMPLETE: component={}", component);
    Ok(())
}

/// Synchronous zip extraction with per-entry progress sent over `tx`.
/// Runs inside `spawn_blocking`; progress is pushed through the channel so the
/// async runtime (and the UI) stays responsive.
fn extract_zip_sync(
    zip_path: &Path,
    dest: &Path,
    tx: &mpsc::UnboundedSender<InstallProgress>,
) -> Result<(), String> {
    let file = std::fs::File::open(zip_path)
        .map_err(|e| format!("Failed to open zip {}: {e}", zip_path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| format!("Failed to read zip archive: {e}"))?;

    let total_entries = archive.len();
    // First pass: total uncompressed bytes for a real percentage.
    let mut total_bytes: u64 = 0;
    for i in 0..total_entries {
        if let Ok(e) = archive.by_index(i) {
            total_bytes = total_bytes.saturating_add(e.size());
        }
    }

    std::fs::create_dir_all(dest)
        .map_err(|e| format!("Failed to create dest {:?}: {e}", dest))?;

    let mut done_bytes: u64 = 0;
    for i in 0..total_entries {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("zip entry {i} error: {e}"))?;

        let out_path = match entry.enclosed_name() {
            Some(p) => dest.join(p),
            None => {
                // Path escapes the archive root; skip for safety.
                continue;
            }
        };

        if entry.is_dir() {
            std::fs::create_dir_all(&out_path)
                .map_err(|e| format!("mkdir {:?}: {e}", out_path))?;
        } else {
            if let Some(p) = out_path.parent() {
                std::fs::create_dir_all(p)
                    .map_err(|e| format!("mkdir parent {:?}: {e}", p))?;
            }
            let mut outfile = std::fs::File::create(&out_path)
                .map_err(|e| format!("create file {:?}: {e}", out_path))?;
            let mut buf = [0u8; 64 * 1024];
            loop {
                let n = entry
                    .read(&mut buf)
                    .map_err(|e| format!("read zip entry {i}: {e}"))?;
                if n == 0 {
                    break;
                }
                outfile
                    .write_all(&buf[..n])
                    .map_err(|e| format!("write {:?}: {e}", out_path))?;
                done_bytes += n as u64;
            }
            outfile
                .flush()
                .map_err(|e| format!("flush {:?}: {e}", out_path))?;
        }

        let pct = if total_bytes > 0 {
            ((done_bytes * 100) / total_bytes).min(100) as u32
        } else {
            0
        };
        let _ = tx.send(InstallProgress {
            component: "extraction".to_string(),
            percent: pct,
            message: format!(
                "Extracting {}/{} — {}",
                i + 1,
                total_entries,
                entry.name()
            ),
            bytes_done: done_bytes,
            bytes_total: Some(total_bytes),
        });
    }
    Ok(())
}

/// Move the archive's top-level item(s) from `tmp` into `dest`. If the archive
/// contained a single top-level folder, it is renamed to `dest`; otherwise every
/// top-level entry is moved in.
fn consolidate_dir(tmp: &Path, dest: &Path) -> Result<(), String> {
    let entries: Vec<PathBuf> = std::fs::read_dir(tmp)
        .map_err(|e| format!("Failed to read temp dir: {e}"))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();

    if entries.is_empty() {
        return Err(format!(
            "Archive appeared empty (no top-level entries in {})",
            tmp.display()
        ));
    }

    if dest.exists() {
        if dest.is_dir() {
            let _ = std::fs::remove_dir_all(dest);
        } else {
            let _ = std::fs::remove_file(dest);
        }
    }

    if entries.len() == 1 && entries[0].is_dir() {
        std::fs::rename(&entries[0], dest).map_err(|e| {
            format!(
                "Failed to move extracted folder to {}: {e}",
                dest.display()
            )
        })?;
    } else {
        std::fs::create_dir_all(dest)
            .map_err(|e| format!("Failed to create {}: {e}", dest.display()))?;
        for entry in &entries {
            let target = dest.join(entry.file_name().unwrap());
            if target.exists() {
                if target.is_dir() {
                    let _ = std::fs::remove_dir_all(&target);
                } else {
                    let _ = std::fs::remove_file(&target);
                }
            }
            std::fs::rename(entry, &target).map_err(|e| {
                format!(
                    "Failed to move {} into {}: {e}",
                    entry.display(),
                    dest.display()
                )
            })?;
        }
    }

    let _ = std::fs::remove_dir_all(tmp);
    Ok(())
}

/// Build a `sdkmanager` (or `avdmanager`) child command with JAVA_HOME /
/// ANDROID_HOME set so the tools find java and the SDK.
fn sdkmanager_command(args: &[&str]) -> Result<Command, String> {
    let sm = paths::sdkmanager_path();
    if !sm.is_file() {
        return Err(format!(
            "sdkmanager not found at {}. Install cmdline-tools first.",
            sm.display()
        ));
    }

    let mut cmd = Command::new(&sm);
    for a in args {
        cmd.arg(a);
    }

    for (k, v) in paths::java_env_pairs() {
        eprintln!("[SDK] sdkmanager_command: env {}={}", k, v);
        cmd.env(k, v);
    }
    cmd.kill_on_drop(true);
    Ok(cmd)
}

/// Read raw bytes from an optional `AsyncRead` stream and split on **both**
/// `\r` and `\n` as line boundaries.  Accepts `Option<R>` so callers can pass
/// `child.stdout.take()` / `child.stderr.take()` directly without a separate
/// `if let Some` guard.
///
/// `on_line` is called for each non-empty decoded line.  The full accumulated
/// output (with `\n` separators) is returned so callers can include it in
/// error messages.
async fn read_crlf_lines<R: tokio::io::AsyncRead + Unpin>(
    reader: Option<R>,
    mut on_line: impl FnMut(&str),
) -> String {
    use tokio::io::AsyncReadExt;
    let Some(reader) = reader else {
        return String::new();
    };
    let mut reader = tokio::io::BufReader::new(reader);
    let mut buf = [0u8; 4096];
    let mut accumulator: Vec<u8> = Vec::new();
    let mut full_output = String::new();

    loop {
        let n = match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };

        for &byte in &buf[..n] {
            if byte == b'\n' || byte == b'\r' {
                if !accumulator.is_empty() {
                    if let Ok(line) = std::str::from_utf8(&accumulator) {
                        let trimmed = line.trim();
                        if !trimmed.is_empty() {
                            on_line(trimmed);
                            full_output.push_str(trimmed);
                            full_output.push('\n');
                        }
                    }
                    accumulator.clear();
                }
            } else {
                accumulator.push(byte);
            }
        }
    }

    // Flush any trailing data that didn't end with a terminator.
    if !accumulator.is_empty() {
        if let Ok(line) = std::str::from_utf8(&accumulator) {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                on_line(trimmed);
                full_output.push_str(trimmed);
                full_output.push('\n');
            }
        }
    }

    full_output
}

/// Build an `avdmanager` child command with the java/sdk env configured.
pub(crate) fn avdmanager_command(args: &[&str]) -> Result<Command, String> {
    let am = paths::avdmanager_path();
    if !am.is_file() {
        return Err(format!(
            "avdmanager not found at {}. Install cmdline-tools first.",
            am.display()
        ));
    }

    let mut cmd = Command::new(&am);
    for a in args {
        cmd.arg(a);
    }

    for (k, v) in paths::java_env_pairs() {
        cmd.env(k, v);
    }
    cmd.kill_on_drop(true);
    Ok(cmd)
}

/// Try to parse a download-progress percentage from a sdkmanager output line.
/// First tries the bracket-style format (`[=======>    ] 45%`), then falls back
/// to a generic `NN%` extractor so that \r-terminated progress ticks that may
/// have been split mid-line are still recognised.
fn parse_progress_percent(line: &str) -> Option<u32> {
    let line = line.trim();
    // Bracket-style progress: [=======>    ] 45% or [=======>    ] 45% Fetching...
    if let Some(bracket_end) = line.find(']') {
        let after = line[bracket_end + 1..].trim();
        if let Some(pct_idx) = after.find('%') {
            let before_pct = after[..pct_idx].trim();
            if let Some(pct_str) = before_pct.split_whitespace().last() {
                if let Ok(pct) = pct_str.parse::<u32>() {
                    return Some(pct.min(100));
                }
            }
        }
    }
    // Generic fallback: any occurrence of "NN%" in the line.
    extract_percentage(line)
}

/// Generic percentage extractor: finds the first `%` in the line and extracts
/// the integer immediately preceding it.  Handles formats like `45%`, ` 45%`,
/// `45% Fetching...` etc.
fn extract_percentage(line: &str) -> Option<u32> {
    if let Some(pos) = line.find('%') {
        let mut start = pos;
        while start > 0 {
            let prev = start - 1;
            if let Some(c) = line.chars().nth(prev) {
                if c.is_ascii_digit() || c == ' ' {
                    start = prev;
                } else {
                    break;
                }
            } else {
                break;
            }
        }
        line[start..pos].trim().parse::<u32>().ok()
    } else {
        None
    }
}

/// Map a stream stage string to a component name for progress events.
/// Strips `install:` / `uninstall:` prefixes so that e.g.
/// `install:platform-tools` maps to `platform-tools`.
fn stage_to_component(stage: &str) -> String {
    if let Some(stripped) = stage.strip_prefix("install:") {
        stripped.to_string()
    } else if let Some(stripped) = stage.strip_prefix("uninstall:") {
        stripped.to_string()
    } else {
        stage.to_string()
    }
}

/// Read both stdout and stderr of a child to EOF concurrently, emitting each
/// line to the frontend via `EVT_LOG` **and** returning the full captured
/// output so callers can include it in error messages instead of just an
/// opaque exit code.
pub(crate) async fn stream_child_output(
    child: &mut tokio::process::Child,
    window: &Window,
    stage: &str,
) -> String {
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let w = window.clone();
    let s = stage.to_string();
    let component = stage_to_component(stage);
    // Track the last emitted percent per component so that repeated sdkmanager
    // output lines (e.g. multiple "3%" lines) don't emit duplicate progress
    // events that make the bar appear stuck.
    let last_pct: std::sync::Arc<std::sync::Mutex<Option<u32>>> =
        std::sync::Arc::new(std::sync::Mutex::new(None));
    let last_pct_t1 = last_pct.clone();
    let last_pct_t2 = last_pct.clone();

    let emit_progress = |window: &Window, component: &str, pct: u32, last_pct: &std::sync::Arc<std::sync::Mutex<Option<u32>>>| {
        let mut guard = last_pct.lock().unwrap();
        if *guard == Some(pct) {
            return;
        }
        *guard = Some(pct);
        drop(guard);
        let _ = window.emit(
            EVT_PROGRESS,
            InstallProgress {
                component: component.to_string(),
                percent: pct,
                message: format!("{} download {}%", component, pct),
                bytes_done: 0,
                bytes_total: None,
            },
        );
    };

    let t1 = async {
        let w2 = w.clone();
        let s2 = s.clone();
        let component2 = component.clone();
        read_crlf_lines(stdout, move |trimmed| {
            let _ = w2.emit(
                EVT_LOG,
                LogLine {
                    stage: s2.clone(),
                    line: trimmed.to_string(),
                },
            );
            if let Some(pct) = parse_progress_percent(trimmed) {
                emit_progress(&w2, &component2, pct, &last_pct_t1);
            }
        })
        .await
    };

    let t2 = async {
        let w2 = w.clone();
        let s2 = s.clone();
        let component2 = component.clone();
        read_crlf_lines(stderr, move |trimmed| {
            let _ = w2.emit(
                EVT_LOG,
                LogLine {
                    stage: s2.clone(),
                    line: trimmed.to_string(),
                },
            );
            if let Some(pct) = parse_progress_percent(trimmed) {
                emit_progress(&w2, &component2, pct, &last_pct_t2);
            }
        })
        .await
    };

    let (out, err) = tokio::join!(t1, t2);

    // Concatenate stdout + stderr preserving order for a single readable blob.
    if out.is_empty() && err.is_empty() {
        String::new()
    } else if out.is_empty() {
        err
    } else if err.is_empty() {
        out
    } else {
        format!("{}{}", out, err)
    }
}

/// Stream an optional reader's lines, emit each as `EVT_LOG`, parse
/// download-progress percentages and emit `EVT_PROGRESS` (deduplicated
/// so the same percent on consecutive lines doesn't reset the bar),
/// and return the full captured text so callers can include it in
/// error messages.
pub(crate) async fn stream_lines<R: tokio::io::AsyncRead + Unpin>(
    reader: Option<R>,
    window: &Window,
    stage: &str,
) -> String {
    let w = window.clone();
    let s = stage.to_string();
    read_crlf_lines(reader, move |trimmed| {
        let _ = w.emit(
            EVT_LOG,
            LogLine {
                stage: s.clone(),
                line: trimmed.to_string(),
            },
        );
    })
    .await
}

// ---------------------------------------------------------------------------
// `sdkmanager --list` parser
// ---------------------------------------------------------------------------

fn parse_sdkmanager_list(text: &str) -> Vec<SdkPackage> {
    let mut installed: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut available: Vec<SdkPackage> = Vec::new();

    enum Section {
        None,
        Installed,
        Available,
        Updates,
    }
    let mut section = Section::None;

    for raw in text.lines() {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let lower = trimmed.to_ascii_lowercase();

        if lower.starts_with("installed packages") {
            section = Section::Installed;
            continue;
        }
        if lower.starts_with("available packages") {
            section = Section::Available;
            continue;
        }
        if lower.starts_with("available updates") {
            section = Section::Updates;
            continue;
        }
        // Skip banners / column headers.
        if trimmed.chars().all(|c| c == '-' || c == '=') {
            continue;
        }
        if lower.starts_with("path") && (lower.contains("version") || lower.contains("description")) {
            continue;
        }

        match section {
            Section::None => {}
            Section::Installed => {
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                if let Some(id) = parts.first() {
                    installed.insert((*id).to_string());
                }
            }
            Section::Available => {
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                if parts.is_empty() {
                    continue;
                }
                // Package ids use ';' as a separator but never contain spaces,
                // so split_whitespace is safe.
                let id = parts[0].to_string();
                let version = parts
                    .get(1)
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                let desc = parts
                    .get(2)
                    .map(|_| parts[2..].join(" "))
                    .unwrap_or_default();
                available.push(SdkPackage {
                    id: id.clone(),
                    name: id.clone(),
                    desc: desc.clone(),
                    version,
                    installed: installed.contains(&id),
                    category: categorize(&id),
                });
            }
            Section::Updates => {}
        }
    }

    // Deduplicate by id, keeping the first occurrence.
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut packages: Vec<SdkPackage> = Vec::with_capacity(available.len());
    for p in available {
        if seen.insert(p.id.clone()) {
            packages.push(p);
        }
    }

    // -------------------------------------------------------------------------
    // Curated system-images allowlist
    // -------------------------------------------------------------------------
    // Droidbay exposes exactly 5 curated system images through the UI.
    // sdkmanager itself is NOT restricted — this only controls what is listed
    // and installable from the app.
    //
    // Fallback substitution: if a primary ID is not present in the repository
    // listing, the closest available fallback is used instead so we still get
    // up to 5 images where possible.
    // NOTE: android-28;google_apis;x86_64 may be deprecated in newer repos; if
    // unavailable, fallback to android-27;google_apis;x86_64 or
    // android-27;google_apis;x86 is attempted.

    #[derive(Debug, Clone, Copy)]
    struct CuratedEntry {
        primary: &'static str,
        fallbacks: &'static [&'static str],
    }

    const CURATED_ENTRIES: &[CuratedEntry] = &[
        CuratedEntry {
            // 1. Android TV (API 33) — prefer x86_64, fall back to x86
            primary: "system-images;android-33;android-tv;x86_64",
            fallbacks: &["system-images;android-33;android-tv;x86"],
        },
        CuratedEntry {
            // 2. Wear OS (API 30) — prefer x86, fall back to x86_64 if available
            primary: "system-images;android-30;android-wear;x86",
            fallbacks: &["system-images;android-30;android-wear;x86_64"],
        },
        CuratedEntry {
            // 3. Android 11 (API 30) phone/tablet, google_apis + x86_64
            primary: "system-images;android-30;google_apis;x86_64",
            fallbacks: &["system-images;android-30;google_apis;x86"],
        },
        CuratedEntry {
            // 4. Android 10 (API 29) phone/tablet, google_apis + x86_64
            primary: "system-images;android-29;google_apis;x86_64",
            fallbacks: &["system-images;android-29;google_apis;x86"],
        },
        CuratedEntry {
            // 5. Android 9 (API 28) phone/tablet, google_apis + x86_64
            // If this exact ID is removed upstream, fallback to API 27.
            primary: "system-images;android-28;google_apis;x86_64",
            fallbacks: &[
                "system-images;android-28;google_apis;x86",
                "system-images;android-27;google_apis;x86_64",
                "system-images;android-27;google_apis;x86",
            ],
        },
    ];

    let available_ids: std::collections::HashSet<String> =
        packages.iter().map(|p| p.id.clone()).collect();

    let mut allowed_system_ids: std::collections::HashSet<String> =
        std::collections::HashSet::new();

    for entry in CURATED_ENTRIES {
        if available_ids.contains(entry.primary) {
            allowed_system_ids.insert(entry.primary.to_string());
        } else {
            for &fallback in entry.fallbacks {
                if available_ids.contains(fallback) {
                    allowed_system_ids.insert(fallback.to_string());
                    eprintln!(
                        "[SDK] Curated system-image substitution: '{}' not found, using '{}'",
                        entry.primary, fallback
                    );
                    break;
                }
            }
        }
    }

    packages.retain(|p| {
        if p.id.starts_with("system-images") {
            allowed_system_ids.contains(&p.id)
        } else {
            true
        }
    });

    packages
}

fn categorize(id: &str) -> String {
    let idl = id.to_lowercase();
    if idl.starts_with("system-images") {
        "system-images".to_string()
    } else if idl.starts_with("platform-tools") {
        "platform-tools".to_string()
    } else if idl.starts_with("build-tools") {
        "build-tools".to_string()
    } else if idl.starts_with("platforms") {
        "platforms".to_string()
    } else if idl == "emulator" || idl.starts_with("emulator") {
        "emulator".to_string()
    } else if idl.starts_with("ndk") {
        "ndk".to_string()
    } else {
        "other".to_string()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_available_and_installed_packages() {
        let sample = "\
Loading package index...
Done. 3 packages available.
Installed packages
----------
Package Path        Version
platform-tools       1.0
build-tools;34.0.0   34.0.0

Available Packages
----------
Package Path               Version      Description
platform-tools             35.0.0       Android SDK Platform-...
build-tools;34.0.0         34.0.0       Android SDK Build-Tools 34
system-images;android-30;google_apis;x86_64 30      Android SDK...
emulator                   32.0.0       Android Emulator

Available Updates
----------
";
        let pkgs = parse_sdkmanager_list(sample);
        assert!(pkgs.iter().any(|p| p.id == "platform-tools" && p.installed));
        assert!(pkgs.iter().any(|p| p.id == "build-tools;34.0.0" && p.installed));
        // Curated filter keeps only the 5 allowed system-images; android-30
        // google_apis x86_64 is in the allowlist, non-curated images are dropped.
        assert!(pkgs.iter().any(|p| p.id == "system-images;android-30;google_apis;x86_64"));
        assert!(pkgs.iter().any(|p| p.id == "emulator"));
        // Non-curated system-images must be absent.
        assert!(pkgs.iter().all(|p| !p.id.starts_with("system-images") || p.id == "system-images;android-30;google_apis;x86_64"));
        // No duplicates.
        let mut ids: Vec<String> = pkgs.iter().map(|p| p.id.clone()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), pkgs.len());
    }

    #[test]
    fn categorize_packages() {
        assert_eq!(categorize("system-images;android-34;google_apis;x86_64"), "system-images");
        assert_eq!(categorize("platform-tools"), "platform-tools");
        assert_eq!(categorize("build-tools;34.0.0"), "build-tools");
        assert_eq!(categorize("platforms;android-34"), "platforms");
        assert_eq!(categorize("emulator"), "emulator");
        assert_eq!(categorize("ndk;25.1.8937393"), "ndk");
        assert_eq!(categorize("linter"), "other");
    }

    #[test]
    fn curated_filter_keeps_only_allowed_system_images() {
        // Sample with a mix of curated and non-curated system-images.
        let sample = "\
Loading package index...
Done.
Installed packages
----------
Package Path        Version
platform-tools       1.0

Available Packages
----------
Package Path                          Version      Description
platform-tools                        35.0.0       Android SDK Platform-...
system-images;android-33;android-tv;x86_64 33      Android TV ...
system-images;android-34;google_apis;x86_64 34      Android SDK...
system-images;android-30;android-wear;x86     30      Wear OS...
system-images;android-30;google_apis;x86_64 30      Android SDK...
system-images;android-29;google_apis;x86_64 29      Android SDK...
system-images;android-28;google_apis;x86_64 28      Android SDK...
system-images;android-35;default;x86_64     35      Android SDK...
emulator                              32.0.0      Android Emulator

Available Updates
----------
";
        let pkgs = parse_sdkmanager_list(sample);
        let sys_ids: Vec<String> = pkgs.iter().filter(|p| p.id.starts_with("system-images")).map(|p| p.id.clone()).collect();
        // Exactly 5 curated system-images should remain.
        assert_eq!(sys_ids.len(), 5, "Expected 5 curated system-images, got: {:?}", sys_ids);
        assert!(sys_ids.contains(&"system-images;android-33;android-tv;x86_64".to_string()));
        assert!(sys_ids.contains(&"system-images;android-30;android-wear;x86".to_string()));
        assert!(sys_ids.contains(&"system-images;android-30;google_apis;x86_64".to_string()));
        assert!(sys_ids.contains(&"system-images;android-29;google_apis;x86_64".to_string()));
        assert!(sys_ids.contains(&"system-images;android-28;google_apis;x86_64".to_string()));
        // Non-curated images must be absent.
        assert!(pkgs.iter().all(|p| p.id != "system-images;android-34;google_apis;x86_64"));
        assert!(pkgs.iter().all(|p| p.id != "system-images;android-35;default;x86_64"));
        // Non-system-images must still be present.
        assert!(pkgs.iter().any(|p| p.id == "platform-tools"));
        assert!(pkgs.iter().any(|p| p.id == "emulator"));
    }

    #[test]
    fn curated_filter_substitutes_fallback_when_primary_missing() {
        // Primary TV image is missing, but fallback x86 is available.
        let sample = "\
Loading package index...
Done.
Installed packages
----------
Package Path        Version

Available Packages
----------
Package Path                          Version      Description
system-images;android-33;android-tv;x86     33      Android TV x86...
system-images;android-30;google_apis;x86_64 30      Android SDK...
system-images;android-29;google_apis;x86_64 29      Android SDK...
system-images;android-28;google_apis;x86_64 28      Android SDK...
emulator                              32.0.0      Android Emulator

Available Updates
----------
";
        let pkgs = parse_sdkmanager_list(sample);
        let sys_ids: Vec<String> = pkgs.iter().filter(|p| p.id.starts_with("system-images")).map(|p| p.id.clone()).collect();
        assert_eq!(sys_ids.len(), 4, "Expected 4 curated system-images, got: {:?}", sys_ids);
        // Fallback should have been substituted for the missing TV x86_64.
        assert!(sys_ids.contains(&"system-images;android-33;android-tv;x86".to_string()));
        // The missing primary x86_64 should NOT appear.
        assert!(pkgs.iter().all(|p| p.id != "system-images;android-33;android-tv;x86_64"));
    }

    #[test]
    fn parse_progress_percent_handles_sdkmanager_format() {
        // Bracket-style with trailing text after % (the actual sdkmanager format).
        assert_eq!(parse_progress_percent("[=                                      ] 4% Fetch remote repository..."), Some(4));
        assert_eq!(parse_progress_percent("[=======================================] 100% Computing updates..."), Some(100));
        assert_eq!(parse_progress_percent("[=========                              ] 25% Loading local repository..."), Some(25));
        // Bracket-style without trailing text (also valid).
        assert_eq!(parse_progress_percent("[=======>    ] 45%"), Some(45));
        // Generic fallback: any "NN%" pattern is matched so that \r-split
        // progress ticks (which may lack brackets) still produce a percentage.
        assert_eq!(parse_progress_percent("Downloading 45%"), Some(45));
        assert_eq!(parse_progress_percent("45%"), Some(45));
        // No percentage at all — should not match.
        assert_eq!(parse_progress_percent("[=======>    ] done"), None);
        assert_eq!(parse_progress_percent("Download complete"), None);
    }

    #[test]
    fn extract_percentage_generic() {
        assert_eq!(extract_percentage("45%"), Some(45));
        assert_eq!(extract_percentage(" 45% "), Some(45));
        assert_eq!(extract_percentage("45% Fetching..."), Some(45));
        assert_eq!(extract_percentage("100%"), Some(100));
        assert_eq!(extract_percentage("no percent here"), None);
        assert_eq!(extract_percentage(""), None);
    }
}
