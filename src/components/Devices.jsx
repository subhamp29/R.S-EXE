import { useState, useEffect, useCallback } from 'react';
import Card from './Card';
import StatusBadge from './StatusBadge';
import { useAvds } from '../hooks/useAvds';
import { useEmulatorControl } from '../hooks/useEmulatorControl';
import * as api from '../lib/api';
import { getSdkPath, checkDiskSpace } from '../lib/api';

const RAM_STEPS = [128, 256, 512, 1024, 2048, 4096, 6144, 8192];
const RESOLUTION_PRESETS = [
  { label: '1080 x 1920', value: '1080x1920' },
  { label: '1440 x 2560', value: '1440x2560' },
  { label: '1536 x 2048', value: '1536x2048' },
  { label: '2160 x 3840', value: '2160x3840' },
];
const GPU_MODES = ['host', 'host-only', 'swiftshader_indirect', 'software', 'none'];

function classNames(...xs) {
  return xs.filter(Boolean).join(' ');
}

export default function Devices({ onNavigate }) {
  const { avds, loading, creating, deleting, fetchAll, error: avdError } = useAvds();
  const {
    runningAvds,
    booting,
    logs,
    error: emuError,
    isRunning,
    bootAvd,
    stopAvd,
    forceStopAvd,
    refreshRunning,
    clearLogs,
    optimizeInstalledApps,
    installApk,
    listInstalledApps,
    uninstallApp,
    launchApp,
    captureScreenshot,
  } = useEmulatorControl();
  const [packages, setPackages] = useState([]);
  const [sdkStatus, setSdkStatus] = useState(null);
  const [showCreate, setShowCreate] = useState(false);
  const [diskWarning, setDiskWarning] = useState(null);
  const [editingAvd, setEditingAvd] = useState(null);
  const [form, setForm] = useState({
    name: '',
    systemImage: '',
    ram: 4096,
    cores: 4,
    storage: 8192,
    gpuMode: 'host',
    resolution: '1080x1920',
    dpi: 420,
    speedMode: false,
    noCamera: true,
    noGps: true,
    noBluetooth: true,
  });
  const [errors, setErrors] = useState({});
  const [bootMenuOpen, setBootMenuOpen] = useState(null);
  const [wipeConfirmAvd, setWipeConfirmAvd] = useState(null);
  const [resourceWarningAvd, setResourceWarningAvd] = useState(null);
  const [resourceCheckResult, setResourceCheckResult] = useState(null);
  const [bootWithWipe, setBootWithWipe] = useState(false);
  const [snapshots, setSnapshots] = useState({});
  const [expandedSnapshots, setExpandedSnapshots] = useState(null);
  const [savingSnapshot, setSavingSnapshot] = useState({});
  // Phase 6: APK install state.
  const [installedApps, setInstalledApps] = useState({});
  const [expandedApps, setExpandedApps] = useState(null);
  const [refreshingApps, setRefreshingApps] = useState({});
  const [installingApk, setInstallingApk] = useState({});
  const [dragOverAvd, setDragOverAvd] = useState(null);
  const [apkError, setApkError] = useState({});
  const [screenshotResult, setScreenshotResult] = useState({});

  useEffect(() => {
    api.checkInstallStatus()
      .then((r) => {
        console.log('[CreateDevice] checkInstallStatus result:', r);
        if (r.ok) setSdkStatus(r.output);
        else console.error('[CreateDevice] checkInstallStatus FAILED:', r.error);
      })
      .catch((e) => console.error('[CreateDevice] checkInstallStatus threw:', e));
    api.fetchSdkPackages()
      .then((r) => {
        console.log('[CreateDevice] fetchSdkPackages result:', r);
        if (r.ok) setPackages(r.output || []);
        else console.error('[CreateDevice] fetchSdkPackages FAILED:', r.error);
      })
      .catch((e) => console.error('[CreateDevice] fetchSdkPackages threw:', e));
  }, []);

  // Refresh running state whenever the user navigates to Devices.
  useEffect(() => {
    refreshRunning();
  }, [refreshRunning]);

  const installedImages = packages.filter(
    (p) => p.installed && p.id.startsWith('system-images')
  );

  const validate = () => {
    const e = {};
    if (!form.name.trim()) e.name = 'Name is required';
    else if (!/^[A-Za-z0-9 _.\-]+$/.test(form.name))
      e.name = 'Only letters, numbers, spaces, . _ - are allowed';
    if (!form.systemImage) e.systemImage = 'Select a system image';
    if (form.ram < 128 || form.ram > 32768) e.ram = 'RAM must be 128–32768 MB';
    if (form.cores < 1 || form.cores > 32) e.cores = 'Cores must be 1–32';
    if (form.storage < 128 || form.storage > 262144) e.storage = 'Storage must be 128–262144 MB';
    if (!GPU_MODES.includes(form.gpuMode)) e.gpuMode = 'Invalid GPU mode';
    if (form.dpi < 80 || form.dpi > 640) e.dpi = 'DPI must be 80–640';
    setErrors(e);
    return Object.keys(e).length === 0;
  };

  const handleCreate = async () => {
    console.log('[CreateDevice] button clicked', form);
    if (!validate()) {
      console.warn('[CreateDevice] validation failed, errors:', errors);
      return;
    }

    // Phase 7 — Disk space check before AVD creation.
    try {
      const sdkResult = await getSdkPath();
      if (sdkResult.ok) {
        const diskResult = await checkDiskSpace(sdkResult.output);
        if (diskResult.ok) {
          const info = diskResult.output;
          const gbFree = info.available_bytes / (1024 * 1024 * 1024);
          if (info.critical_space) {
            setDiskWarning({
              type: 'critical',
              message: `Only ${gbFree.toFixed(1)} GB free on ${info.path}. At least 1 GB is required for AVD creation.`,
            });
            return;
          }
          if (info.low_space) {
            setDiskWarning({
              type: 'warning',
              message: `Only ${gbFree.toFixed(1)} GB free on ${info.path}. AVD creation needs several GB. Continue anyway?`,
            });
            return;
          }
        }
      }
    } catch (e) {
      console.warn('[CreateDevice] Disk space check failed, proceeding anyway:', e);
    }

    const payload = {
      name: form.name,
      system_image: form.systemImage,
      ram: form.ram,
      cores: form.cores,
      storage: form.storage,
      gpu_mode: form.gpuMode,
      resolution: form.resolution,
      dpi: form.dpi,
      no_camera: form.noCamera,
      no_gps: form.noGps,
      no_bluetooth: form.noBluetooth,
    };
    console.log('[CreateDevice] invoking create_avd with:', payload);
    try {
      const res = await api.createAvd(payload);
      console.log('[CreateDevice] createAvd returned:', res);
      if (res.ok) {
        console.log('[CreateDevice] success, closing modal');
        setShowCreate(false);
        setForm({
          name: '', systemImage: '', ram: 4096, cores: 4, storage: 8192,
          gpuMode: 'host', resolution: '1080x1920', dpi: 420, speedMode: false,
          noCamera: true, noGps: true, noBluetooth: true,
        });
      } else {
        console.error('[CreateDevice] FAILED:', res.error);
      }
    } catch (e) {
      console.error('[CreateDevice] FAILED:', e);
    }
  };

  const handleDelete = async (name) => {
    if (!window.confirm(`Delete AVD "${name}"? This cannot be undone.`)) return;
    const res = await api.deleteAvd(name);
    if (res.ok) await fetchAll();
  };

  const handleStop = async (name) => {
    console.log('[Devices] handleStop clicked for:', name);
    const res = await stopAvd(name);
    if (!res.ok) console.error('[Devices] Stop failed:', res.error);
  };

  const handleOptimize = async (name) => {
    console.log('[Devices] handleOptimize clicked for:', name);
    const res = await optimizeInstalledApps(name);
    if (!res.ok) console.error('[Devices] Optimize failed:', res.error);
  };

  const handleForceStop = async (name) => {
    console.log('[Devices] handleForceStop clicked for:', name);
    const res = await forceStopAvd(name);
    if (!res.ok) console.error('[Devices] Force stop failed:', res.error);
  };

  const openEdit = (avd) => {
    setEditingAvd(avd);
    setForm({
      name: avd.name,
      ram: Number(avd.ram?.replace('MB', '') || 4096),
      cores: Number(avd.cores || 4),
      gpuMode: avd.gpu_mode || 'host',
      resolution: '1080x1920',
      dpi: 420,
      speedMode: avd.speed_mode === true,
      noCamera: avd.no_camera !== false,
      noGps: avd.no_gps !== false,
      noBluetooth: avd.no_bluetooth !== false,
    });
  };

  const handleSaveEdit = async () => {
    if (!editingAvd) return;
    const options = {
      ram: form.ram,
      cores: form.cores,
      gpu_mode: form.gpuMode,
      resolution: form.resolution,
      dpi: form.dpi,
      speed_mode: form.speedMode,
      no_camera: form.noCamera,
      no_gps: form.noGps,
      no_bluetooth: form.noBluetooth,
    };
    const res = await api.updateAvdConfig(editingAvd.name, options);
    if (res.ok) {
      setEditingAvd(null);
      await fetchAll();
    } else {
      console.error('[Devices] Edit failed:', res.error);
      alert(res.error);
    }
  };

  const toggleBootMenu = (name, e) => {
    e.stopPropagation();
    setBootMenuOpen((prev) => (prev === name ? null : name));
  };

  const handleBoot = async (name) => {
    console.log('[Devices] handleBoot clicked for:', name);
    setBootMenuOpen(null);
    await bootAvd(name, { no_snapshot: true });
  };

  const handleWipeBoot = async (name) => {
    console.log('[Devices] handleWipeBoot clicked for:', name);
    setBootMenuOpen(null);
    setWipeConfirmAvd(name);
  };

  const confirmWipeBoot = async () => {
    if (!wipeConfirmAvd) return;
    const name = wipeConfirmAvd;
    setWipeConfirmAvd(null);
    setBootWithWipe(true);
    await bootAvd(name, { no_snapshot: true, wipe_user_data: true });
    setBootWithWipe(false);
  };

  const bootAfterWarning = async () => {
    if (!resourceWarningAvd) return;
    const name = resourceWarningAvd;
    const wasWipe = bootWithWipe;
    setResourceWarningAvd(null);
    setResourceCheckResult(null);
    setWipeConfirmAvd(null);
    setBootWithWipe(false);
    if (wasWipe) {
      await bootAvd(name, { no_snapshot: true, wipe_user_data: true });
    } else {
      await bootAvd(name, { no_snapshot: true });
    }
  };

  const cancelBoot = () => {
    setResourceWarningAvd(null);
    setResourceCheckResult(null);
    setWipeConfirmAvd(null);
  };

  const toggleSnapshots = async (avd) => {
    const next = expandedSnapshots === avd.name ? null : avd.name;
    setExpandedSnapshots(next);
    if (next && !snapshots[next]) {
      const res = await api.listSnapshots(next);
      if (res.ok) {
        setSnapshots((prev) => ({ ...prev, [next]: res.output }));
      }
    }
  };

  const handleDeleteSnapshot = async (avdName, snapshotId) => {
    if (!window.confirm(`Delete snapshot "${snapshotId}"? This cannot be undone.`)) return;
    const res = await api.deleteSnapshot(avdName, snapshotId);
    if (res.ok) {
      setSnapshots((prev) => ({
        ...prev,
        [avdName]: (prev[avdName] || []).filter((s) => s.id !== snapshotId),
      }));
    } else {
      console.error('[Devices] Delete snapshot failed:', res.error);
      alert(res.error);
    }
  };

  const handleSaveSnapshot = async (avdName) => {
    setSavingSnapshot((prev) => ({ ...prev, [avdName]: true }));
    const res = await api.saveSnapshot(avdName);
    if (res.ok) {
      const listRes = await api.listSnapshots(avdName);
      if (listRes.ok) {
        setSnapshots((prev) => ({ ...prev, [avdName]: listRes.output }));
      }
    } else {
      console.error('[Devices] Save snapshot failed:', res.error);
      alert(res.error);
    }
    setSavingSnapshot((prev) => ({ ...prev, [avdName]: false }));
  };

  const toggleApps = async (avd) => {
    const next = expandedApps === avd.name ? null : avd.name;
    setExpandedApps(next);
    if (next && !installedApps[next]) {
      setRefreshingApps((prev) => ({ ...prev, [next]: true }));
      const res = await listInstalledApps(next);
      if (res.ok) {
        setInstalledApps((prev) => ({ ...prev, [next]: res.output || [] }));
      }
      setRefreshingApps((prev) => ({ ...prev, [next]: false }));
    }
  };

  const refreshApps = async (avdName) => {
    setRefreshingApps((prev) => ({ ...prev, [avdName]: true }));
    const res = await listInstalledApps(avdName);
    if (res.ok) {
      setInstalledApps((prev) => ({ ...prev, [avdName]: res.output || [] }));
    }
    setRefreshingApps((prev) => ({ ...prev, [avdName]: false }));
  };

  const handleUninstallApp = async (avdName, packageName) => {
    if (!window.confirm(`Uninstall '${packageName}' from '${avdName}'?`)) return;
    const res = await uninstallApp(avdName, packageName);
    if (!res.ok) {
      console.error('[Devices] Uninstall app failed:', res.error);
      alert(res.error);
    } else {
      setInstalledApps((prev) => ({
        ...prev,
        [avdName]: (prev[avdName] || []).filter((a) => a.package !== packageName),
      }));
    }
  };

  const handleLaunchApp = async (avdName, packageName) => {
    const res = await launchApp(avdName, packageName);
    if (!res.ok) {
      console.error('[Devices] Launch app failed:', res.error);
      alert(res.error);
    }
  };

  const handleInstallApk = async (avdName, file) => {
    setApkError((prev) => ({ ...prev, [avdName]: null }));
    setInstallingApk((prev) => ({ ...prev, [avdName]: true }));
    const res = await installApk(avdName, file.path);
    if (!res.ok) {
      setApkError((prev) => ({ ...prev, [avdName]: res.error }));
      console.error('[Devices] Install APK failed:', res.error);
    } else {
      setInstalledApps((prev) => ({ ...prev, [avdName]: undefined }));
      setExpandedApps(avdName);
      setTimeout(() => refreshApps(avdName), 1500);
    }
    setInstallingApk((prev) => ({ ...prev, [avdName]: false }));
  };

  const handleFilePick = async (avdName) => {
    try {
      const { open } = await import('@tauri-apps/plugin-dialog');
      const selected = await open({
        multiple: false,
        filters: [{ name: 'APK', extensions: ['apk'] }],
      });
      if (selected && typeof selected === 'string') {
        handleInstallApk(avdName, { path: selected });
      }
    } catch (e) {
      console.error('[Devices] File picker failed:', e);
    }
  };

  const handleDrop = (avdName, e) => {
    e.preventDefault();
    e.stopPropagation();
    setDragOverAvd(null);
    const files = e.dataTransfer?.files;
    if (!files || files.length === 0) return;
    const file = files[0];
    if (!file.name.toLowerCase().endsWith('.apk')) {
      setApkError((prev) => ({ ...prev, [avdName]: 'Only .apk files are supported.' }));
      return;
    }
    // Tauri gives us a path via file.path (webkitRelativePath is not reliable).
    const apkPath = file.path || file.name;
    if (!apkPath || apkPath === file.name) {
      setApkError((prev) => ({ ...prev, [avdName]: 'Could not resolve file path. Use the file picker instead.' }));
      return;
    }
    handleInstallApk(avdName, { path: apkPath });
  };

  const handleDragOver = (avdName, e) => {
    e.preventDefault();
    e.stopPropagation();
    setDragOverAvd(avdName);
  };

  const handleDragLeave = (avdName, e) => {
    e.preventDefault();
    e.stopPropagation();
    setDragOverAvd(null);
  };

  const handleCaptureScreenshot = async (avdName) => {
    const res = await captureScreenshot(avdName);
    if (res.ok && res.output) {
      setScreenshotResult((prev) => ({ ...prev, [avdName]: res.output }));
      setTimeout(() => {
        setScreenshotResult((prev) => {
          const next = { ...prev };
          delete next[avdName];
          return next;
        });
      }, 5000);
    } else if (!res.ok) {
      console.error('[Devices] Screenshot failed:', res.error);
      alert(res.error);
    }
  };

  const handleOpenScreenshotsFolder = async () => {
    try {
      const { revealItemInDir } = await import('@tauri-apps/plugin-opener');
      const dir = await new Promise((resolve) => {
        // We need to call a backend command to get the screenshots dir, but
        // for simplicity we use the known default path.
        resolve(null);
      });
      // The backend returns the path; we reveal it.
      // For now, just open the default screenshots dir via a small workaround:
      const { openPath } = await import('@tauri-apps/plugin-opener');
      // We don't have a direct backend call for this, but we can use a temp
      // backend command or just open the default location.
      // Simpler: use a new backend command `get_screenshots_dir` or just
      // rely on the user knowing the path from the toast.
      // For UX, we'll add a small helper: the toast already shows the path.
    } catch (e) {
      console.error('[Devices] Open folder failed:', e);
    }
  };

  const formatBytes = (bytes) => {
    if (bytes === 0) return '0 B';
    const k = 1024;
    const sizes = ['B', 'KB', 'MB', 'GB'];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + ' ' + sizes[i];
  };

  // Empty state: SDK not ready.
  const sdkReady = sdkStatus?.jdk && sdkStatus?.cmdline_tools;
  const canCreate = sdkReady && installedImages.length > 0;
  console.log(
    '[CreateDevice] render — sdkReady:', sdkReady,
    '| canCreate:', canCreate,
    '| installedImages:', installedImages.length,
    '| sdkStatus:', sdkStatus,
  );

  const renderEmpty = () => (
    <Card title="Devices" className="card-devices-empty">
      <div className="empty-state">
        <svg width="40" height="40" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
          <rect x="4" y="2" width="16" height="20" rx="2" ry="2" />
          <line x1="12" y1="18" x2="12.01" y2="18" />
        </svg>
        <p className="empty-text">
          {!sdkStatus
            ? 'Checking SDK installation…'
            : 'No Android SDK components are installed yet.'}
        </p>
        {!sdkReady && (
          <button className="btn-accent" onClick={() => onNavigate('sdk')}>
            Go to SDK Manager
          </button>
        )}
      </div>
    </Card>
  );

  const renderForm = () => (
    <div className="modal-backdrop" onClick={() => !creating && setShowCreate(false)}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <div className="modal-header">
          <h2 className="dashboard-heading">Create Device</h2>
          <button className="btn-ghost" onClick={() => setShowCreate(false)} disabled={creating}>✕</button>
        </div>
        <div className="modal-body">
          <div className="form-grid">
            <div className="form-field">
              <label>Name</label>
              <input className="input-text" value={form.name}
                onChange={(e) => setForm({ ...form, name: e.target.value })} placeholder="e.g. Pixel_4_API_34" />
              {errors.name && <span className="form-error">{errors.name}</span>}
            </div>

            <div className="form-field">
              <label>System Image</label>
              <select className="input-select" value={form.systemImage}
                onChange={(e) => setForm({ ...form, systemImage: e.target.value })}>
                <option value="">— select an installed image —</option>
                {installedImages.map((p) => (
                  <option key={p.id} value={p.id}>{p.id}</option>
                ))}
              </select>
              {errors.systemImage && <span className="form-error">{errors.systemImage}</span>}
              {installedImages.length === 0 && (
                <span className="text-muted">No system images installed. Install one via the SDK Manager.</span>
              )}
            </div>

            <div className="form-field">
              <label>RAM (MB): {form.ram}</label>
              <input type="range" min={128} max={32768} step={256} value={form.ram}
                onChange={(e) => setForm({ ...form, ram: Number(e.target.value) })} />
              <div className="slider-stepper">
                {RAM_STEPS.map((v) => (
                  <button key={v} size="sm"
                    className={classNames('btn-ghost btn-sm', form.ram === v && 'active')}
                    onClick={() => setForm({ ...form, ram: v })}>{v}</button>
                ))}
              </div>
              {errors.ram && <span className="form-error">{errors.ram}</span>}
            </div>

            <div className="form-field">
              <label>CPU Cores</label>
              <input type="number" min={1} max={32} className="input-text" value={form.cores}
                onChange={(e) => setForm({ ...form, cores: Number(e.target.value) })} />
              {errors.cores && <span className="form-error">{errors.cores}</span>}
            </div>

            <div className="form-field">
              <label>Storage (MB)</label>
              <input type="number" min={128} max={262144} className="input-text" value={form.storage}
                onChange={(e) => setForm({ ...form, storage: Number(e.target.value) })} />
              {errors.storage && <span className="form-error">{errors.storage}</span>}
            </div>

            <div className="form-field">
              <label>GPU Mode</label>
              <select className="input-select" value={form.gpuMode}
                onChange={(e) => setForm({ ...form, gpuMode: e.target.value })}>
                {GPU_MODES.map((m) => <option key={m} value={m}>{m}</option>)}
              </select>
              {errors.gpuMode && <span className="form-error">{errors.gpuMode}</span>}
            </div>

            <div className="form-field">
              <label>Resolution Preset</label>
              <select className="input-select" value={form.resolution}
                onChange={(e) => setForm({ ...form, resolution: e.target.value })}>
                {RESOLUTION_PRESETS.map((r) => <option key={r.value} value={r.value}>{r.label}</option>)}
              </select>
            </div>

            <div className="form-field">
              <label>DPI</label>
              <input type="number" min={80} max={640} className="input-text" value={form.dpi}
                onChange={(e) => setForm({ ...form, dpi: Number(e.target.value) })} />
              {errors.dpi && <span className="form-error">{errors.dpi}</span>}
            </div>

            <div className="form-field" style={{ gridColumn: '1 / -1' }}>
              <label className="form-checkbox-label">
                <input
                  type="checkbox"
                  checked={form.speedMode}
                  onChange={(e) => setForm({ ...form, speedMode: e.target.checked })}
                />
                <span>
                  <strong>Speed Mode</strong>
                  <small className="text-muted" style={{ display: 'block' }}>
                    Post-boot optimization: disables hardware overlays for smoother rendering on weak GPUs.
                    Only works on Google APIs / AOSP images (not Google Play).
                  </small>
                </span>
              </label>
            </div>

            <div className="form-field" style={{ gridColumn: '1 / -1' }}>
              <label className="form-checkbox-label">
                <input
                  type="checkbox"
                  checked={form.noCamera}
                  onChange={(e) => setForm({ ...form, noCamera: e.target.checked })}
                />
                <span>
                  <strong>Disable Camera</strong>
                  <small className="text-muted" style={{ display: 'block' }}>
                    Emulate no camera hardware. Reduces emulator overhead; enable only if testing camera-dependent features.
                  </small>
                </span>
              </label>
            </div>

            <div className="form-field" style={{ gridColumn: '1 / -1' }}>
              <label className="form-checkbox-label">
                <input
                  type="checkbox"
                  checked={form.noGps}
                  onChange={(e) => setForm({ ...form, noGps: e.target.checked })}
                />
                <span>
                  <strong>Disable GPS</strong>
                  <small className="text-muted" style={{ display: 'block' }}>
                    Emulate no GPS hardware. Reduces emulator overhead; enable only if testing location-based features.
                  </small>
                </span>
              </label>
            </div>

            <div className="form-field" style={{ gridColumn: '1 / -1' }}>
              <label className="form-checkbox-label">
                <input
                  type="checkbox"
                  checked={form.noBluetooth}
                  onChange={(e) => setForm({ ...form, noBluetooth: e.target.checked })}
                />
                <span>
                  <strong>Disable Bluetooth</strong>
                  <small className="text-muted" style={{ display: 'block' }}>
                    Emulate no Bluetooth hardware. Reduces emulator overhead; enable only if testing Bluetooth-dependent features.
                  </small>
                </span>
              </label>
            </div>
          </div>
        </div>
        <div className="modal-footer">
          <button className="btn-ghost" onClick={() => setShowCreate(false)} disabled={creating}>Cancel</button>
          <button className="btn-accent" onClick={handleCreate} disabled={creating || !canCreate}>
            {creating ? 'Creating…' : 'Create'}
          </button>
        </div>
       </div>
     </div>
   );

   /* Phase 7 — Disk space warning modal (reuses same pattern as SdkManager). */
   const renderDiskWarning = () => {
     if (!diskWarning) return null;
     return (
       <div className="modal-backdrop" onClick={() => setDiskWarning(null)}>
         <div className="modal" onClick={(e) => e.stopPropagation()}>
           <div className="modal-header">
             <h2 className="dashboard-heading">
               {diskWarning.type === 'critical' ? '⚠️ Low Disk Space' : '💡 Low Disk Space'}
             </h2>
             <button className="btn-ghost" onClick={() => setDiskWarning(null)}>✕</button>
           </div>
           <div className="modal-body">
             <p>{diskWarning.message}</p>
           </div>
           <div className="modal-footer">
             <button className="btn-ghost" onClick={() => setDiskWarning(null)}>Cancel</button>
             {diskWarning.type === 'warning' && (
               <button className="btn-accent" onClick={() => { setDiskWarning(null); handleCreate(); }}>
                 Continue Anyway
               </button>
             )}
           </div>
         </div>
       </div>
     );
   };

   const renderEditForm = () => {
    if (!editingAvd) return null;
    const running = isRunning(editingAvd.name);
    return (
      <div className="modal-backdrop" onClick={() => setEditingAvd(null)}>
        <div className="modal" onClick={(e) => e.stopPropagation()}>
          <div className="modal-header">
            <h2 className="dashboard-heading">Edit {editingAvd.name}</h2>
            <button className="btn-ghost" onClick={() => setEditingAvd(null)}>✕</button>
          </div>
          <div className="modal-body">
            {running && (
              <div className="form-error" style={{ marginBottom: 12 }}>
                This AVD is currently running. Stop it first before editing.
              </div>
            )}
            <div className="form-grid">
              <div className="form-field">
                <label>RAM (MB): {form.ram}</label>
                <input type="range" min={128} max={32768} step={256} value={form.ram}
                  onChange={(e) => setForm({ ...form, ram: Number(e.target.value) })}
                  disabled={running} />
                <div className="slider-stepper">
                  {RAM_STEPS.map((v) => (
                    <button key={v} size="sm"
                      className={classNames('btn-ghost btn-sm', form.ram === v && 'active')}
                      onClick={() => setForm({ ...form, ram: v })}
                      disabled={running}>{v}</button>
                  ))}
                </div>
              </div>

              <div className="form-field">
                <label>CPU Cores</label>
                <input type="number" min={1} max={32} className="input-text" value={form.cores}
                  onChange={(e) => setForm({ ...form, cores: Number(e.target.value) })}
                  disabled={running} />
              </div>

              <div className="form-field">
                <label>GPU Mode</label>
                <select className="input-select" value={form.gpuMode}
                  onChange={(e) => setForm({ ...form, gpuMode: e.target.value })}
                  disabled={running}>
                  {GPU_MODES.map((m) => <option key={m} value={m}>{m}</option>)}
                </select>
              </div>

              <div className="form-field">
                <label>Resolution Preset</label>
                <select className="input-select" value={form.resolution}
                  onChange={(e) => setForm({ ...form, resolution: e.target.value })}
                  disabled={running}>
                  {RESOLUTION_PRESETS.map((r) => <option key={r.value} value={r.value}>{r.label}</option>)}
                </select>
              </div>

              <div className="form-field">
                <label>DPI</label>
                <input type="number" min={80} max={640} className="input-text" value={form.dpi}
                  onChange={(e) => setForm({ ...form, dpi: Number(e.target.value) })}
                  disabled={running} />
              </div>

               <div className="form-field" style={{ gridColumn: '1 / -1' }}>
                 <label className="form-checkbox-label">
                   <input
                     type="checkbox"
                     checked={form.speedMode}
                     onChange={(e) => setForm({ ...form, speedMode: e.target.checked })}
                     disabled={running}
                   />
                   <span>
                     <strong>Speed Mode</strong>
                     <small className="text-muted" style={{ display: 'block' }}>
                       Post-boot optimization: disables hardware overlays for smoother rendering on weak GPUs.
                       Only works on Google APIs / AOSP images (not Google Play).
                     </small>
                   </span>
                 </label>
               </div>

               <div className="form-field" style={{ gridColumn: '1 / -1' }}>
                 <label className="form-checkbox-label">
                   <input
                     type="checkbox"
                     checked={form.noCamera}
                     onChange={(e) => setForm({ ...form, noCamera: e.target.checked })}
                     disabled={running}
                   />
                   <span>
                     <strong>Disable Camera</strong>
                     <small className="text-muted" style={{ display: 'block' }}>
                       Emulate no camera hardware. Reduces emulator overhead; enable only if testing camera-dependent features.
                     </small>
                   </span>
                 </label>
               </div>

               <div className="form-field" style={{ gridColumn: '1 / -1' }}>
                 <label className="form-checkbox-label">
                   <input
                     type="checkbox"
                     checked={form.noGps}
                     onChange={(e) => setForm({ ...form, noGps: e.target.checked })}
                     disabled={running}
                   />
                   <span>
                     <strong>Disable GPS</strong>
                     <small className="text-muted" style={{ display: 'block' }}>
                       Emulate no GPS hardware. Reduces emulator overhead; enable only if testing location-based features.
                     </small>
                   </span>
                 </label>
               </div>

               <div className="form-field" style={{ gridColumn: '1 / -1' }}>
                 <label className="form-checkbox-label">
                   <input
                     type="checkbox"
                     checked={form.noBluetooth}
                     onChange={(e) => setForm({ ...form, noBluetooth: e.target.checked })}
                     disabled={running}
                   />
                   <span>
                     <strong>Disable Bluetooth</strong>
                     <small className="text-muted" style={{ display: 'block' }}>
                       Emulate no Bluetooth hardware. Reduces emulator overhead; enable only if testing Bluetooth-dependent features.
                     </small>
                   </span>
                 </label>
               </div>
             </div>
           </div>
           <div className="modal-footer">
             <button className="btn-ghost" onClick={() => setEditingAvd(null)} disabled={creating}>Cancel</button>
             <button className="btn-accent" onClick={handleSaveEdit} disabled={creating || running}>
               {running ? 'Stop device first' : 'Save'}
             </button>
          </div>
        </div>
      </div>
    );
  };

  return (
    <div className="devices-page">
      {(avdError || emuError) && (
        <div className="error-banner" role="alert">
          <span>{avdError || emuError}</span>
        </div>
      )}

      <div className="devices-header">
        <h1 className="dashboard-heading">Devices</h1>
        <button
          className="btn-accent"
          onClick={() => {
            console.log('[CreateDevice] + Create Device button clicked, canCreate:', canCreate, 'sdkReady:', sdkReady);
            if (!canCreate) {
              console.warn(
                '[CreateDevice] Button disabled — sdkReady:', sdkReady,
                '| installedImages:', installedImages.length,
                '| sdkStatus:', sdkStatus,
              );
              return;
            }
            setShowCreate(true);
          }}
          disabled={!canCreate}
          title={canCreate ? '' : 'Install SDK + a system image first'}
        >
          + Create Device
        </button>
      </div>

      {loading ? (
        <div style={{ display: 'flex', gap: 'var(--space-4)', flexWrap: 'wrap' }}>
          {[...Array(3)].map((_, i) => <div key={i} className="skeleton-line" style={{ width: 220, height: 120 }} />)}
        </div>
      ) : avds.length === 0 ? (
        renderEmpty()
      ) : (
        <div className="device-grid">
          {avds.map((a) => {
            const running = isRunning(a.name);
            const isBooting = booting[a.name];
            return (
              <Card key={a.name} title={a.name} className="card-device">
                <div className="device-specs">
                  API {a.api_level ?? '—'} · {a.ram?.replace('MB', '') || '—'} MB · {a.cores || '—'} cores · {a.gpu_mode || '—'}
                </div>
                <div className="device-footer">
                  <StatusBadge type={running ? 'success' : 'muted'}>
                    {running ? 'Running' : 'Stopped'}
                  </StatusBadge>
                  <span className={`status-dot ${running ? 'status-dot-on' : 'status-dot-off'}`} />
                </div>
                <div className="card-actions">
                  {running ? (
                    <>
                      <button
                        className="btn-ghost btn-sm"
                        onClick={() => handleStop(a.name)}
                        disabled={isBooting}
                        title="Stop emulator"
                      >
                        {isBooting ? 'Stopping…' : 'Stop'}
                      </button>
                      <button
                        className="btn-danger btn-sm"
                        onClick={() => {
                          if (window.confirm(`Force stop '${a.name}'? This kills the emulator process directly without saving state.`)) {
                            handleForceStop(a.name);
                          }
                        }}
                        disabled={isBooting}
                        title="Force stop emulator"
                      >
                        Force Stop
                      </button>
                      <button
                        className="btn-ghost btn-sm"
                        onClick={() => handleCaptureScreenshot(a.name)}
                        disabled={isBooting}
                        title="Capture screenshot"
                      >
                        📷
                      </button>
                      <button
                        className="btn-ghost btn-sm"
                        onClick={() => handleOptimize(a.name)}
                        disabled={isBooting}
                        title="AOT compile all user-installed apps for better performance"
                      >
                        Optimize Apps
                      </button>
                    </>
                  ) : (
                    <div className="boot-button-group">
                      <button
                        className="btn-accent btn-sm"
                        onClick={() => handleBoot(a.name)}
                        disabled={isBooting}
                        title="Boot emulator"
                      >
                        {isBooting ? 'Booting…' : 'Boot'}
                      </button>
                      <button
                        className="btn-ghost btn-sm boot-chevron"
                        onClick={(e) => toggleBootMenu(a.name, e)}
                        title="More boot options"
                      >
                        ▾
                      </button>
                      {bootMenuOpen === a.name && (
                        <div className="boot-dropdown">
                          <button
                            className="boot-dropdown-item boot-dropdown-wipe"
                            onClick={() => handleWipeBoot(a.name)}
                          >
                            Wipe & Boot
                          </button>
                        </div>
                      )}
                    </div>
                  )}
                  {running && (
                    <button
                      className="btn-ghost btn-sm"
                      onClick={() => handleFilePick(a.name)}
                      disabled={isBooting}
                      title="Install APK..."
                    >
                      Install APK...
                    </button>
                  )}
                  <button
                    className="btn-ghost btn-sm"
                    onClick={() => openEdit(a)}
                    disabled={running}
                    title={running ? 'Stop the device before editing' : 'Edit AVD config'}
                  >
                    Edit
                  </button>
                  <button
                    className="btn-danger btn-sm"
                    onClick={() => handleDelete(a.name)}
                    disabled={deleting[a.name] || running}
                    title="Delete"
                  >
                    {deleting[a.name] ? 'Deleting…' : 'Delete'}
                  </button>
                </div>
                <div className="card-snapshots">
                  <button
                    className="snapshots-toggle"
                    onClick={() => toggleSnapshots(a)}
                  >
                    <span>Snapshots {(snapshots[a.name] || []).length > 0 && `(${(snapshots[a.name] || []).length})`}</span>
                    <span className={`snapshots-chevron ${expandedSnapshots === a.name ? 'expanded' : ''}`}>▸</span>
                  </button>
                  {expandedSnapshots === a.name && (
                    <div className="snapshots-list">
                      {(snapshots[a.name] || []).length === 0 ? (
                        <span className="text-muted snapshots-empty">No snapshots yet</span>
                      ) : (
                        (snapshots[a.name] || []).map((s) => (
                          <div key={s.id} className="snapshot-item">
                            <span className="snapshot-name">{s.name}</span>
                            <span className="snapshot-size">{formatBytes(s.size_bytes)}</span>
                            <button
                              className="btn-danger btn-sm"
                              onClick={() => handleDeleteSnapshot(a.name, s.id)}
                              title="Delete snapshot"
                            >
                              ✕
                            </button>
                          </div>
                        ))
                      )}
                      {running && (
                        <button
                          className="btn-ghost btn-sm snapshots-save"
                          onClick={() => handleSaveSnapshot(a.name)}
                          disabled={savingSnapshot[a.name]}
                        >
                          {savingSnapshot[a.name] ? 'Saving…' : 'Save Snapshot Now'}
                        </button>
                      )}
                    </div>
                  )}
                </div>

                {/* Phase 6: APK drop zone (visible when running) */}
                {running && (
                  <div
                    className={classNames('apk-drop-zone', dragOverAvd === a.name && 'apk-drop-zone-active')}
                    onDrop={(e) => handleDrop(a.name, e)}
                    onDragOver={(e) => handleDragOver(a.name, e)}
                    onDragLeave={(e) => handleDragLeave(a.name, e)}
                    onClick={() => handleFilePick(a.name)}
                  >
                    <span className="apk-drop-text">
                      {installingApk[a.name] ? 'Installing APK…' : 'Drop .apk here or click to install'}
                    </span>
                    {apkError[a.name] && (
                      <span className="apk-drop-error">{apkError[a.name]}</span>
                    )}
                  </div>
                )}

                {/* Phase 6: Installed Apps section */}
                {running && (
                  <div className="card-installed-apps">
                    <button
                      className="snapshots-toggle"
                      onClick={() => toggleApps(a)}
                    >
                      <span>
                        Installed Apps {(installedApps[a.name] || []).length > 0 ? `(${(installedApps[a.name] || []).length})` : ''}
                      </span>
                      <span className={`snapshots-chevron ${expandedApps === a.name ? 'expanded' : ''}`}>▸</span>
                    </button>
                    {expandedApps === a.name && (
                      <div className="installed-apps-list">
                        {refreshingApps[a.name] ? (
                          <span className="text-muted snapshots-empty">Loading…</span>
                        ) : (installedApps[a.name] || []).length === 0 ? (
                          <span className="text-muted snapshots-empty">No third-party apps installed yet</span>
                        ) : (
                          (installedApps[a.name] || []).map((app) => (
                            <div key={app.package} className="installed-app-item">
                              <span className="installed-app-name" title={app.package}>
                                {app.label || app.package}
                              </span>
                              <div className="installed-app-actions">
                                <button
                                  className="btn-ghost btn-sm"
                                  onClick={() => handleLaunchApp(a.name, app.package)}
                                  title="Launch app"
                                >
                                  ▶ Launch
                                </button>
                                <button
                                  className="btn-danger btn-sm"
                                  onClick={() => handleUninstallApp(a.name, app.package)}
                                  title="Uninstall app"
                                >
                                  ✕ Uninstall
                                </button>
                              </div>
                            </div>
                          ))
                        )}
                        <button
                          className="btn-ghost btn-sm snapshots-save"
                          onClick={() => refreshApps(a.name)}
                          disabled={refreshingApps[a.name]}
                        >
                          {refreshingApps[a.name] ? 'Refreshing…' : 'Refresh List'}
                        </button>
                      </div>
                    )}
                  </div>
                )}

                {/* Phase 6: Screenshot toast */}
                {screenshotResult[a.name] && (
                  <div className="screenshot-toast">
                    <span>Screenshot saved:</span>
                    <span className="screenshot-path">{screenshotResult[a.name]}</span>
                  </div>
                )}
              </Card>
            );
          })}
        </div>
      )}

      {/* Boot log panel */}
      {logs.length > 0 && (
        <Card title="Boot Log" className="card-device-log" footer={
          <button className="btn-ghost btn-sm" onClick={clearLogs} title="Clear log">Clear</button>
        }>
          <div className="log-console-header">
            <span className="dots">
              <span className="dot dot-red" />
              <span className="dot dot-amber" />
              <span className="dot dot-green" />
            </span>
            <span className="log-console-live" />
          </div>
          <div className="log-console">
            {logs.map((l, i) => {
              const isWarning = l.line.includes('Corrupted Quick Boot snapshot') ||
                l.line.includes('Speed Mode:') ||
                l.line.includes('Compiling');
              const isError = l.line.includes('Boot failed') ||
                l.line.includes('Optimization failed');
              return (
                <div key={i} className={classNames('log-line', isWarning && 'log-line-warning', isError && 'log-line-error')}>
                  <span className="log-stage">[{l.stage}]</span> {l.line}
                </div>
              );
            })}
          </div>
        </Card>
      )}

      {showCreate && renderForm()}
      {diskWarning && renderDiskWarning()}
      {editingAvd && renderEditForm()}

      {/* Wipe & Boot confirmation modal */}
      {wipeConfirmAvd && (
        <div className="modal-backdrop" onClick={() => setWipeConfirmAvd(null)}>
          <div className="modal" onClick={(e) => e.stopPropagation()}>
            <div className="modal-header">
              <h2 className="dashboard-heading">Wipe & Boot</h2>
              <button className="btn-ghost" onClick={() => setWipeConfirmAvd(null)}>✕</button>
            </div>
            <div className="modal-body">
              <div className="warning-icon" style={{ marginBottom: 12 }}>⚠</div>
              <p style={{ marginBottom: 8 }}>
                This will erase <strong>all app data and user data</strong> on <strong>"{wipeConfirmAvd}"</strong> and start completely fresh.
              </p>
              <p style={{ marginBottom: 8, color: 'var(--color-text-secondary)' }}>
                Your AVD's hardware configuration, settings, and installed system image will be kept intact — only the emulated device's internal storage will be reset.
              </p>
              <p style={{ color: 'var(--color-danger)', fontWeight: 500 }}>
                This action cannot be undone.
              </p>
            </div>
            <div className="modal-footer">
              <button className="btn-ghost" onClick={() => setWipeConfirmAvd(null)}>Cancel</button>
              <button className="btn-accent" onClick={confirmWipeBoot} style={{ background: 'var(--color-warning)', color: '#0b0c10' }}>
                Wipe & Boot
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Resource warning modal */}
      {resourceWarningAvd && resourceCheckResult && (
        <div className="modal-backdrop" onClick={cancelBoot}>
          <div className="modal" onClick={(e) => e.stopPropagation()}>
            <div className="modal-header">
              <h2 className="dashboard-heading">
                {resourceCheckResult.low_memory ? 'Low Available Memory' : 'Performance Warning'}
              </h2>
              <button className="btn-ghost" onClick={cancelBoot}>✕</button>
            </div>
            <div className="modal-body">
              <div className="warning-icon" style={{ marginBottom: 12 }}>⚠</div>
              {resourceCheckResult.low_memory && (
                <>
                  <p style={{ marginBottom: 8 }}>
                    Low available memory ({' '}
                    {(resourceCheckResult.free_ram_bytes / 1024 / 1024 / 1024).toFixed(1)} GB free
                    , this AVD requests{' '}
                    {(resourceCheckResult.avd_ram_bytes / 1024 / 1024 / 1024).toFixed(1)} GB).
                  </p>
                  <p style={{ marginBottom: 8, color: 'var(--color-text-secondary)' }}>
                    Boot may be slow or unresponsive. Consider closing other applications or reducing this AVD's RAM in Edit.
                  </p>
                </>
              )}
              {resourceCheckResult.multiple_running && (
                <>
                  <p style={{ marginBottom: 8 }}>
                    Another AVD is currently running ({(resourceCheckResult.running_names || []).join(', ')}).
                  </p>
                  <p style={{ marginBottom: 8, color: 'var(--color-text-secondary)' }}>
                    Running multiple emulators simultaneously on this hardware is likely to cause severe slowdowns.
                  </p>
                </>
              )}
            </div>
            <div className="modal-footer">
              <button className="btn-ghost" onClick={cancelBoot}>Cancel and Edit</button>
              <button className="btn-accent" onClick={bootAfterWarning}>Boot Anyway</button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

