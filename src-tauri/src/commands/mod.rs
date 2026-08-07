pub mod result;
pub mod system;
pub mod window;
pub mod paths;
pub mod sdk;
pub mod avd;
pub mod emulator;

pub use result::CommandResult;

pub use system::{get_system_info, detect_gpus, check_hypervisor, get_app_version, check_disk_space};
pub use window::{window_minimize, window_maximize, window_close};

pub use paths::{
    sdk_base, sdk_dir, jdk_dir, cmdline_dir, avd_dir, emulator_dir, emulator_binary_path,
    platform_tools_dir, jdk_installed, cmdline_installed, platform_tools_installed,
    emulator_installed, licenses_accepted, app_settings, save_app_settings, clear_app_settings,
    sdkmanager_path, avdmanager_path, screenshots_dir, java_env_pairs, get_sdk_path,
};

pub use sdk::{
    check_install_status, get_install_progress, install_jdk, install_cmdline_tools, accept_licenses,
    fetch_sdk_packages, install_package, uninstall_package,
};
pub use avd::{list_avds, create_avd, delete_avd, update_avd_config, start_avd};
pub use emulator::{
    boot_avd, stop_avd, force_stop_avd, get_running_avds, optimize_installed_apps,
    check_boot_resources, list_snapshots, delete_snapshot, save_snapshot,
    install_apk, list_installed_apps, uninstall_app, launch_app, capture_screenshot,
    get_app_settings, save_app_settings_window, reset_app_settings,
};
