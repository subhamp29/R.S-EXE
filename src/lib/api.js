import { invoke } from '@tauri-apps/api/core';

function ok(output) {
  return { ok: true, error: null, output };
}

function fail(error) {
  return { ok: false, error, output: null };
}

export async function getSystemInfo() {
  try {
    const result = await invoke('get_system_info');
    if (result && result.ok && result.output) return ok(result.output);
    return fail(result && result.error ? result.error : 'Unknown error');
  } catch (e) {
    return fail(String(e));
  }
}

export async function detectGpus() {
  try {
    const result = await invoke('detect_gpus');
    if (result && result.ok && result.output) return ok(result.output);
    return fail(result && result.error ? result.error : 'Unknown error');
  } catch (e) {
    return fail(String(e));
  }
}

export async function checkHypervisor() {
  try {
    const result = await invoke('check_hypervisor');
    if (result && result.ok && result.output) return ok(result.output);
    return fail(result && result.error ? result.error : 'Unknown error');
  } catch (e) {
    return fail(String(e));
  }
}

export async function getAppVersion() {
  try {
    const result = await invoke('get_app_version');
    if (result && result.ok && result.output) return ok(result.output);
    return fail(result && result.error ? result.error : 'Unknown error');
  } catch (e) {
    return fail(String(e));
  }
}

export async function windowMinimize() {
  try {
    const result = await invoke('window_minimize');
    if (result && result.ok) return ok(result.output ?? true);
    return fail(result && result.error ? result.error : 'Unknown error');
  } catch (e) {
    return fail(String(e));
  }
}

export async function windowMaximize() {
  try {
    const result = await invoke('window_maximize');
    if (result && result.ok) return ok(result.output ?? true);
    return fail(result && result.error ? result.error : 'Unknown error');
  } catch (e) {
    return fail(String(e));
  }
}

export async function windowClose() {
  try {
    const result = await invoke('window_close');
    if (result && result.ok) return ok(result.output ?? true);
    return fail(result && result.error ? result.error : 'Unknown error');
  } catch (e) {
    return fail(String(e));
  }
}

// ---------------------------------------------------------------------------
// SDK management
// ---------------------------------------------------------------------------

export async function checkInstallStatus() {
  try {
    const result = await invoke('check_install_status');
    if (result && result.ok && result.output) return ok(result.output);
    return fail(result && result.error ? result.error : 'Unknown error');
  } catch (e) {
    return fail(String(e));
  }
}

export async function getInstallProgress() {
  try {
    const result = await invoke('get_install_progress');
    if (result && result.ok && result.output) return ok(result.output);
    return fail(result && result.error ? result.error : 'Unknown error');
  } catch (e) {
    return fail(String(e));
  }
}

export async function installJdk() {
  try {
    const result = await invoke('install_jdk');
    if (result && result.ok) return ok(result.output ?? true);
    return fail(result && result.error ? result.error : 'Unknown error');
  } catch (e) {
    return fail(String(e));
  }
}

export async function installCmdlineTools() {
  try {
    const result = await invoke('install_cmdline_tools');
    if (result && result.ok) return ok(result.output ?? true);
    return fail(result && result.error ? result.error : 'Unknown error');
  } catch (e) {
    return fail(String(e));
  }
}

export async function acceptLicenses() {
  try {
    const result = await invoke('accept_licenses');
    if (result && result.ok) return ok(result.output ?? true);
    return fail(result && result.error ? result.error : 'Unknown error');
  } catch (e) {
    return fail(String(e));
  }
}

export async function fetchSdkPackages() {
  try {
    const result = await invoke('fetch_sdk_packages');
    if (result && result.ok) return ok(result.output || []);
    return fail(result && result.error ? result.error : 'Unknown error');
  } catch (e) {
    return fail(String(e));
  }
}

export async function installPackage(pkg) {
  try {
    const result = await invoke('install_package', { package: pkg });
    if (result && result.ok) return ok(result.output ?? true);
    return fail(result && result.error ? result.error : 'Unknown error');
  } catch (e) {
    return fail(String(e));
  }
}

export async function uninstallPackage(pkg) {
  try {
    const result = await invoke('uninstall_package', { package: pkg });
    if (result && result.ok) return ok(result.output ?? true);
    return fail(result && result.error ? result.error : 'Unknown error');
  } catch (e) {
    return fail(String(e));
  }
}

// ---------------------------------------------------------------------------
// AVD management
// ---------------------------------------------------------------------------

export async function listAvds() {
  try {
    const result = await invoke('list_avds');
    if (result && result.ok) return ok(result.output || []);
    return fail(result && result.error ? result.error : 'Unknown error');
  } catch (e) {
    return fail(String(e));
  }
}

export async function createAvd(input) {
  try {
    const result = await invoke('create_avd', { input });
    if (result && result.ok) return ok(result.output ?? true);
    return fail(result && result.error ? result.error : 'Unknown error');
  } catch (e) {
    return fail(String(e));
  }
}

export async function deleteAvd(name) {
  try {
    const result = await invoke('delete_avd', { name });
    if (result && result.ok) return ok(result.output ?? true);
    return fail(result && result.error ? result.error : 'Unknown error');
  } catch (e) {
    return fail(String(e));
  }
}

export async function startAvd(name) {
  try {
    const result = await invoke('start_avd', { name });
    if (result && result.ok) return ok(result.output ?? true);
    return fail(result && result.error ? result.error : 'Unknown error');
  } catch (e) {
    return fail(String(e));
  }
}

export async function stopAvd(name) {
  try {
    const result = await invoke('stop_avd', { name });
    if (result && result.ok) return ok(result.output ?? true);
    return fail(result && result.error ? result.error : 'Unknown error');
  } catch (e) {
    return fail(String(e));
  }
}

export async function forceStopAvd(name) {
  try {
    const result = await invoke('force_stop_avd', { name });
    if (result && result.ok) return ok(result.output ?? true);
    return fail(result && result.error ? result.error : 'Unknown error');
  } catch (e) {
    return fail(String(e));
  }
}

export async function bootAvd(name, options) {
  try {
    const result = await invoke('boot_avd', { name, options: options ?? null });
    if (result && result.ok) return ok(result.output ?? true);
    return fail(result && result.error ? result.error : 'Unknown error');
  } catch (e) {
    return fail(String(e));
  }
}

export async function getRunningAvds() {
  try {
    const result = await invoke('get_running_avds');
    if (result && result.ok) return ok(result.output || []);
    return fail(result && result.error ? result.error : 'Unknown error');
  } catch (e) {
    return fail(String(e));
  }
}

export async function optimizeInstalledApps(avdName) {
  try {
    const result = await invoke('optimize_installed_apps', { avdName });
    if (result && result.ok) return ok(result.output ?? true);
    return fail(result && result.error ? result.error : 'Unknown error');
  } catch (e) {
    return fail(String(e));
  }
}

export async function updateAvdConfig(name, options) {
  try {
    const result = await invoke('update_avd_config', { name, options });
    if (result && result.ok) return ok(result.output ?? true);
    return fail(result && result.error ? result.error : 'Unknown error');
  } catch (e) {
    return fail(String(e));
  }
}

// ---------------------------------------------------------------------------
// Phase 5 — Resource check & snapshots
// ---------------------------------------------------------------------------

export async function checkBootResources(avdName) {
  try {
    const result = await invoke('check_boot_resources', { name: avdName });
    if (result && result.ok && result.output) return ok(result.output);
    return fail(result && result.error ? result.error : 'Unknown error');
  } catch (e) {
    return fail(String(e));
  }
}

export async function listSnapshots(avdName) {
  try {
    const result = await invoke('list_snapshots', { avdName });
    if (result && result.ok && result.output) return ok(result.output || []);
    return fail(result && result.error ? result.error : 'Unknown error');
  } catch (e) {
    return fail(String(e));
  }
}

export async function getSdkPath() {
  return invoke('get_sdk_path');
}

export async function checkDiskSpace(path) {
  try {
    const result = await invoke('check_disk_space', { path });
    if (result && result.ok && result.output) return ok(result.output);
    return fail(result && result.error ? result.error : 'Unknown error');
  } catch (e) {
    return fail(String(e));
  }
}

export async function deleteSnapshot(avdName, snapshotId) {
  try {
    const result = await invoke('delete_snapshot', { avdName, snapshotId });
    if (result && result.ok) return ok(result.output ?? true);
    return fail(result && result.error ? result.error : 'Unknown error');
  } catch (e) {
    return fail(String(e));
  }
}

export async function saveSnapshot(avdName) {
  try {
    const result = await invoke('save_snapshot', { avdName });
    if (result && result.ok) return ok(result.output ?? true);
    return fail(result && result.error ? result.error : 'Unknown error');
  } catch (e) {
    return fail(String(e));
  }
}

// ---------------------------------------------------------------------------
// Phase 6 — APK install / app management / screenshot / settings
// ---------------------------------------------------------------------------

export async function installApk(avdName, apkPath) {
  try {
    const result = await invoke('install_apk', { avdName, apkPath });
    if (result && result.ok) return ok(result.output ?? true);
    return fail(result && result.error ? result.error : 'Unknown error');
  } catch (e) {
    return fail(String(e));
  }
}

export async function listInstalledApps(avdName) {
  try {
    const result = await invoke('list_installed_apps', { avdName });
    if (result && result.ok) return ok(result.output || []);
    return fail(result && result.error ? result.error : 'Unknown error');
  } catch (e) {
    return fail(String(e));
  }
}

export async function uninstallApp(avdName, packageName) {
  try {
    const result = await invoke('uninstall_app', { avdName, packageName });
    if (result && result.ok) return ok(result.output ?? true);
    return fail(result && result.error ? result.error : 'Unknown error');
  } catch (e) {
    return fail(String(e));
  }
}

export async function launchApp(avdName, packageName) {
  try {
    const result = await invoke('launch_app', { avdName, packageName });
    if (result && result.ok) return ok(result.output ?? true);
    return fail(result && result.error ? result.error : 'Unknown error');
  } catch (e) {
    return fail(String(e));
  }
}

export async function captureScreenshot(avdName) {
  try {
    const result = await invoke('capture_screenshot', { avdName });
    if (result && result.ok) return ok(result.output ?? null);
    return fail(result && result.error ? result.error : 'Unknown error');
  } catch (e) {
    return fail(String(e));
  }
}

export async function getAppSettings() {
  try {
    const result = await invoke('get_app_settings');
    if (result && result.ok) return ok(result.output ?? null);
    return fail(result && result.error ? result.error : 'Unknown error');
  } catch (e) {
    return fail(String(e));
  }
}

export async function saveAppSettings(settings) {
  try {
    const result = await invoke('save_app_settings_window', { settings });
    if (result && result.ok) return ok(result.output ?? true);
    return fail(result && result.error ? result.error : 'Unknown error');
  } catch (e) {
    return fail(String(e));
  }
}

export async function resetAppSettings() {
  try {
    const result = await invoke('reset_app_settings');
    if (result && result.ok) return ok(result.output ?? true);
    return fail(result && result.error ? result.error : 'Unknown error');
  } catch (e) {
    return fail(String(e));
  }
}
