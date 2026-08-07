#![allow(dependency_on_unit_never_type_fallback)]
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
mod commands;
use commands::{
    get_app_version, get_system_info, check_hypervisor, detect_gpus,
    window_close, window_maximize, window_minimize,
    check_install_status, get_install_progress, install_jdk, install_cmdline_tools, accept_licenses,
    fetch_sdk_packages, install_package, uninstall_package,
    list_avds, create_avd, delete_avd, update_avd_config, start_avd, stop_avd,
    boot_avd, force_stop_avd, get_running_avds, optimize_installed_apps,
    check_boot_resources, list_snapshots, delete_snapshot, save_snapshot,
    install_apk, list_installed_apps, uninstall_app, launch_app, capture_screenshot,
    get_app_settings, save_app_settings_window, reset_app_settings,
    check_disk_space, get_sdk_path,
};
use tauri::Manager;
use tauri::Emitter;

fn main() {
    tauri::Builder::default()
        // Single-instance lock: register first so it intercepts second launches
        // before any other plugin or command runs.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            // Focus the existing main window so the user sees the app come to foreground.
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_focus();
            }
            // Notify the frontend so it can show a brief "already running" message.
            let _ = app.emit("single-instance", "R.S EXE is already running");
            // Exit this second instance cleanly — no second window, no resource contention.
            std::process::exit(0);
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            get_system_info,
            detect_gpus,
            check_hypervisor,
            get_app_version,
            window_minimize,
            window_maximize,
            window_close,
            // SDK management
            check_install_status,
            get_install_progress,
            install_jdk,
            install_cmdline_tools,
            accept_licenses,
            fetch_sdk_packages,
            install_package,
            uninstall_package,
            // AVD management
            list_avds,
            create_avd,
            delete_avd,
            update_avd_config,
            start_avd,
            stop_avd,
            // Emulator control
            boot_avd,
            force_stop_avd,
            get_running_avds,
            optimize_installed_apps,
            // Phase 5
            check_boot_resources,
            list_snapshots,
            delete_snapshot,
            save_snapshot,
            // Phase 6
            install_apk,
            list_installed_apps,
            uninstall_app,
            launch_app,
            capture_screenshot,
            get_app_settings,
            save_app_settings_window,
            reset_app_settings,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

