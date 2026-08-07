//! Single source of truth for every filesystem path Droidbay touches.
//!
//! `sdk.rs` and `avd.rs` both import from here so path logic is never
//! duplicated. Every helper returns a concrete [`PathBuf`] built from the
//! environment with a sensible, documented default.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use crate::commands::CommandResult;

/// The root Android SDK directory.
///
/// Resolution order:
/// 1. `DROIDBAY_ANDROID_SDK` env (explicit override for testing/packaging)
/// 2. `ANDROID_HOME` env (honored by every Android tool on the planet)
/// 3. User-configured SDK override (Settings page)
/// 4. `LOCALAPPDATA\Android\Sdk` (the Android Studio default on Windows)
pub fn sdk_base() -> PathBuf {
    if let Ok(p) = std::env::var("DROIDBAY_ANDROID_SDK") {
        return PathBuf::from(p);
    }
    if let Ok(p) = std::env::var("ANDROID_HOME") {
        return PathBuf::from(p);
    }
    if let Some(p) = app_settings().and_then(|s| s.sdk_override).filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    #[cfg(windows)]
    {
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            return PathBuf::from(local).join("Android").join("Sdk");
        }
    }
    // Linux / macOS fallback.
    let home = std::env::var("HOME")
        .unwrap_or_else(|_| "HOME".to_string());
    PathBuf::from(home).join("Android").join("Sdk")
}

/// Canonical SDK root (alias kept for readability at call sites).
#[allow(dead_code)]
pub fn sdk_dir() -> PathBuf {
    sdk_base()
}

/// Where the JDK lives (`sdk/jdk`).
pub fn jdk_dir() -> PathBuf {
    if let Some(p) = app_settings().and_then(|s| s.jdk_override).filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    sdk_base().join("jdk")
}

/// Where cmdline-tools `latest` lives (`sdk/cmdline-tools/latest`).
pub fn cmdline_dir() -> PathBuf {
    sdk_base().join("cmdline-tools").join("latest")
}

/// Directory holding `.avd` folders and `*.ini` files (`~/.android/avd`
/// or `ANDROID_AVD_HOME`).
pub fn avd_dir() -> PathBuf {
    if let Ok(p) = std::env::var("ANDROID_AVD_HOME") {
        return PathBuf::from(p);
    }
    if let Ok(home) = std::env::var("USERPROFILE") {
        return PathBuf::from(home).join(".android").join("avd");
    }
    // Unix-style fallback.
    std::env::var("HOME")
        .map(|h| PathBuf::from(h).join(".android").join("avd"))
        .unwrap_or_else(|_| PathBuf::from(".android").join("avd"))
}

/// Where the `emulator` binary package lives (`sdk/emulator`).
pub fn emulator_dir() -> PathBuf {
    sdk_base().join("emulator")
}

/// Absolute path to the `emulator` binary (`emulator.exe` on Windows).
pub fn emulator_binary_path() -> std::path::PathBuf {
    emulator_dir().join(if cfg!(windows) { "emulator.exe" } else { "emulator" })
}

/// `sdk/platform-tools` — used for `adb` and install-status checks.
pub fn platform_tools_dir() -> PathBuf {
    sdk_base().join("platform-tools")
}

// ---------------------------------------------------------------------------
// Existence helpers — these are the *real* source of truth for install state.
// ---------------------------------------------------------------------------

/// JDK is present if `bin/java` (or `bin/java.exe`) exists under the JDK dir.
pub fn jdk_installed() -> bool {
    let java = jdk_dir().join("bin").join(java_exe_name());
    java.is_file()
}

/// cmdline-tools is present if `bin/sdkmanager` exists under the cmdline dir.
pub fn cmdline_installed() -> bool {
    sdkmanager_path().is_file()
}

/// platform-tools is present if `platform-tools/` (or `adb`) exists.
pub fn platform_tools_installed() -> bool {
    platform_tools_dir().join(adb_exe_name()).is_file()
}

/// emulator is present if `emulator/emulator.exe` (or `emulator`) exists.
pub fn emulator_installed() -> bool {
    emulator_binary_path().is_file()
}

/// The SDK licenses are considered accepted when at least one file exists
/// inside the `licenses/` directory of the SDK root.
pub fn licenses_accepted() -> bool {
    let lic_dir = sdk_base().join("licenses");
    if !lic_dir.is_dir() {
        return false;
    }
    // Count at least one license file (non-directory entry) inside.
    std::fs::read_dir(&lic_dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .any(|e| e.file_type().map(|ft| ft.is_file()).unwrap_or(false))
        })
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// App-level settings persistence
// ---------------------------------------------------------------------------

/// App-level settings persisted in a JSON file next to the app's data directory.
/// This is separate from any AVD config and survives app restarts.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct AppSettings {
    /// Optional override for the Android SDK root path. When set, `sdk_base()`
    /// returns this path (after env vars).
    pub sdk_override: Option<String>,
    /// Optional override for the JDK root path. When set, `jdk_dir()` returns
    /// this path.
    pub jdk_override: Option<String>,
    /// Where to save screenshots captured from running AVDs.
    pub screenshot_dir: Option<String>,
}

static APP_SETTINGS: LazyLock<Mutex<Option<AppSettings>>> = LazyLock::new(|| Mutex::new(None));

/// Path to the app-level settings JSON file.
fn app_settings_path() -> PathBuf {
    #[cfg(windows)]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return PathBuf::from(appdata).join("R.S EXE").join("settings.json");
        }
    }
    // Unix fallback: ~/.config/rs-exe/settings.json
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config").join("rs-exe").join("settings.json")
}

/// Load app settings from disk. Cached in memory after the first load.
pub fn app_settings() -> Option<AppSettings> {
    let mut guard = APP_SETTINGS.lock().ok()?;
    if guard.is_none() {
        let path = app_settings_path();
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(s) = serde_json::from_str::<AppSettings>(&text) {
                *guard = Some(s);
            }
        }
    }
    guard.clone()
}

/// Save app settings to disk and update the in-memory cache.
pub fn save_app_settings(settings: AppSettings) -> Result<(), String> {
    let path = app_settings_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create settings dir: {e}"))?;
    }
    let json = serde_json::to_string_pretty(&settings).map_err(|e| format!("serialize settings: {e}"))?;
    std::fs::write(&path, json).map_err(|e| format!("write settings: {e}"))?;
    if let Ok(mut guard) = APP_SETTINGS.lock() {
        *guard = Some(settings);
    }
    Ok(())
}

/// Clear all app-level settings (reset to defaults).
pub fn clear_app_settings() -> Result<(), String> {
    let path = app_settings_path();
    let _ = std::fs::remove_file(&path);
    if let Ok(mut guard) = APP_SETTINGS.lock() {
        *guard = None;
    }
    Ok(())
}

/// Absolute path to the `sdkmanager` launcher script/batch.
pub fn sdkmanager_path() -> std::path::PathBuf {
    cmdline_dir().join("bin").join(SDKMANAGER_LAUNCHER_NAME)
}

/// Absolute path to the `avdmanager` launcher script/batch.
pub fn avdmanager_path() -> std::path::PathBuf {
    cmdline_dir().join("bin").join(AVDMANAGER_LAUNCHER_NAME)
}

/// Where screenshots captured from running AVDs should be saved.
///
/// Resolution order:
/// 1. User-configured screenshot dir (Settings page)
/// 2. `<app data>/R.S EXE/screenshots/` (default app data location)
pub fn screenshots_dir() -> PathBuf {
    if let Some(p) = app_settings().and_then(|s| s.screenshot_dir.clone()).filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    #[cfg(windows)]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return PathBuf::from(appdata).join("R.S EXE").join("screenshots");
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config").join("rs-exe").join("screenshots")
}

/// Build the environment block a child process needs to find java + the SDK.
/// Returns (env_name, env_value) pairs suitable for `Command::envs`.
pub fn java_env_pairs() -> Vec<(&'static str, String)> {
    let jdk = jdk_dir();
    let mut pairs = Vec::with_capacity(4);
    pairs.push(("JAVA_HOME", jdk.to_string_lossy().into_owned()));
    let java_bin = jdk.join("bin");
    // Prepend the JDK bin to PATH so sdkmanager/avdmanager find `java`.
    let existing = std::env::vars_os()
        .find(|(k, _)| k == "PATH")
        .map(|(_, v)| v.into_string().unwrap_or_default())
        .unwrap_or_default();
    let new_path = if existing.is_empty() {
        java_bin.to_string_lossy().into_owned()
    } else {
        format!("{}\u{0};{}", java_bin.to_string_lossy(), existing)
    };
    pairs.push(("PATH", new_path));
    pairs.push(("ANDROID_HOME", sdk_base().to_string_lossy().into_owned()));
    pairs.push(
        ("ANDROID_SDK_ROOT", sdk_base().to_string_lossy().into_owned()),
    );
    pairs
}

/// Returns `java.exe` on Windows and `java` elsewhere.
fn java_exe_name() -> &'static str {
    if cfg!(windows) { "java.exe" } else { "java" }
}

fn adb_exe_name() -> &'static str {
    if cfg!(windows) { "adb.exe" } else { "adb" }
}

/// The sdkmanager/avdmanager launcher filename differs by platform.
const SDKMANAGER_LAUNCHER_NAME: &str = if cfg!(windows) { "sdkmanager.bat" } else { "sdkmanager" };
const AVDMANAGER_LAUNCHER_NAME: &str = if cfg!(windows) { "avdmanager.bat" } else { "avdmanager" };

// ---------------------------------------------------------------------------
// Phase 7 — Expose SDK path to the frontend for disk-space checks
// ---------------------------------------------------------------------------

/// Returns the Android SDK root path as a string so the frontend can
/// run a disk-space check before SDK installs and AVD creation.
#[tauri::command]
pub fn get_sdk_path() -> CommandResult<String> {
    CommandResult::success(sdk_base().to_string_lossy().into_owned())
}
