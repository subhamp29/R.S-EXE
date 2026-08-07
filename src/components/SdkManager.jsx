import { useState } from 'react';
import { useSdkStatus } from '../hooks/useSdkStatus';
import Card from './Card';
import { getSdkPath, checkDiskSpace } from '../lib/api';

// ---------------------------------------------------------------------------
// Curated system-image allowlist (mirrors the Rust-side curation filter)
// ---------------------------------------------------------------------------
// Only these system-image package IDs are shown in the SDK Packages panel and
// installable from the UI. All others are hidden. The Rust backend already
// filters the data source, but we keep this JS-side check as a safety net.
const CURATED_SYSTEM_IMAGE_IDS = new Set([
  // 1. Android TV (API 33, x86_64 primary, x86 fallback)
  'system-images;android-33;android-tv;x86_64',
  'system-images;android-33;android-tv;x86',
  // 2. Wear OS (API 30, x86 primary, x86_64 fallback)
  'system-images;android-30;android-wear;x86',
  'system-images;android-30;android-wear;x86_64',
  // 3–5. Standard phone/tablet, API ≤ 30, google_apis + x86_64 (with x86 fallbacks)
  'system-images;android-30;google_apis;x86_64',
  'system-images;android-30;google_apis;x86',
  'system-images;android-29;google_apis;x86_64',
  'system-images;android-29;google_apis;x86',
  'system-images;android-28;google_apis;x86_64',
  'system-images;android-28;google_apis;x86',
  // API 28 fallbacks (if removed upstream)
  'system-images;android-27;google_apis;x86_64',
  'system-images;android-27;google_apis;x86',
]);

function isAllowedSystemImage(id) {
  return !id.startsWith('system-images') || CURATED_SYSTEM_IMAGE_IDS.has(id);
}

function StatusCheck({ label, installed, installing, percent }) {
  const showProgress = installing && percent != null && percent > 0;
  // Show progress badge also when install is not actively "installing" but
  // progress is stuck at < 100 % — this prevents a stale "Installed" badge
  // when a partial download left files on disk but never completed.
  const showStuckProgress = !installing && percent != null && percent > 0 && percent < 100;
  const effectiveInstalled = installed && !showProgress && !showStuckProgress;
  return (
    <div className="sdk-status-row">
      <span className={`sdk-check ${effectiveInstalled ? 'sdk-check-ok' : 'sdk-check-missing'}`}>
        {effectiveInstalled ? (
          <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round">
            <polyline points="20 6 9 17 4 12" />
          </svg>
        ) : (
          <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round">
            <line x1="18" y1="6" x2="6" y2="18" /><line x1="6" y1="6" x2="18" y2="18" />
          </svg>
        )}
      </span>
      <span className={`sdk-check-label ${effectiveInstalled ? 'sdk-check-ok' : 'sdk-check-missing'}`}>
        {label}
      </span>
      {effectiveInstalled ? (
        <span className="status-badge status-badge-success">Installed</span>
      ) : showProgress ? (
        <span className="status-badge status-badge-progress">{percent}%</span>
      ) : showStuckProgress ? (
        <span className="status-badge status-badge-progress">{percent}%</span>
      ) : (
        <span className="status-badge status-badge-muted">Missing</span>
      )}
    </div>
  );
}

function ProgressBar({ progress }) {
  if (!progress || progress.percent === 0) return null;
  return (
    <div className="sdk-progress">
      <div className="stat-row">
        <span className="stat-label">{progress.component}</span>
        <span className="stat-value">{progress.percent}%</span>
      </div>
      <div className="progress-track">
        <div className="progress-fill" style={{ width: `${progress.percent}%` }} />
      </div>
      <span className="progress-label">{progress.message}</span>
    </div>
  );
}

export default function SdkManager() {
  const {
    status,
    packages,
    loading,
    error,
    logs,
    progress,
    installing,
    installAllDisabled,
    fetchAll,
    installAll,
    installJdk,
    installCmdlineTools,
    acceptLicenses,
    installPackage,
    uninstallPackage,
    clearLogs,
  } = useSdkStatus();

  const [search, setSearch] = useState('');
  const [statusExpanded, setStatusExpanded] = useState(false);
  const [diskWarning, setDiskWarning] = useState(null);

  const handleInstallAll = async (force = false) => {
    console.log('[SdkManager] Install All CLICKED, installAllDisabled=', installAllDisabled, 'status=', status);

    // Phase 7 — Disk space check before SDK install (skip if force=true).
    if (!force) {
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
                message: `Only ${gbFree.toFixed(1)} GB free on ${info.path}. At least 1 GB is required for SDK installs.`,
              });
              return;
            }
            if (info.low_space) {
              setDiskWarning({
                type: 'warning',
                message: `Only ${gbFree.toFixed(1)} GB free on ${info.path}. SDK installs need several GB. Continue anyway?`,
              });
              return;
            }
          }
        }
      } catch (e) {
        console.warn('[SdkManager] Disk space check failed, proceeding anyway:', e);
      }
    }

    try {
      const result = await installAll();
      console.log('[SdkManager] installAll resolved:', result);
    } catch (e) {
      console.error('[SdkManager] installAll threw:', e);
    }
  };

  // Derived display state — purely presentational, no logic change.
  const hasMissing = status && (!status.jdk || !status.cmdline_tools || !status.platform_tools || !status.emulator);
  const anyInstalling = Object.values(installing).some(Boolean);

  return (
    <div className="sdk-page">
      <div className="sdk-header">
        <h1 className="dashboard-heading">SDK Manager</h1>
        <button className="btn-refresh" onClick={() => { fetchAll(); clearLogs(); }} title="Refresh status & packages">
          ↻ Refresh
        </button>
      </div>

      {error && (
        <div className="error-banner" role="alert">
          <span>{error}</span>
        </div>
      )}

      {/* Phase 7 — Disk space warning modal (reuses same pattern as Phase 5 resource warning). */}
      {diskWarning && (
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
                <button className="btn-accent" onClick={() => { setDiskWarning(null); handleInstallAll(true); }}>
                  Continue Anyway
                </button>
              )}
            </div>
          </div>
        </div>
      )}

      <div className="sdk-grid">
         {/* ---- Status section ---- */}
         <Card className={`card-sdk-status card-primary ${!hasMissing ? '' : 'card-urgent'}`}>
           {loading && status === null ? (
             <div className="skeleton-line" />
           ) : status ? (
             <>
               {!hasMissing ? (
                 <div className="sdk-status-compact">
                   <span className="sdk-compact-text">All SDK components installed ✓</span>
                   <button
                     className="btn-ghost btn-sm"
                     onClick={() => setStatusExpanded((v) => !v)}
                   >
                     {statusExpanded ? 'Hide Details' : 'Details'}
                   </button>
                 </div>
               ) : (
                 <div className="sdk-status-header">
                   <h3 className="sdk-status-title">Install Status</h3>
                   <button
                     className="btn-refresh-sm"
                     onClick={() => fetchAll()}
                     title="Refresh install status"
                   >
                     ↻ Refresh
                   </button>
                 </div>
               )}
               <div className={`sdk-status-details ${!hasMissing && !statusExpanded ? 'sdk-status-details-collapsed' : ''}`}>
                 <StatusCheck label="JDK (Temurin 17)" installed={status.jdk} installing={installing.jdk} percent={progress.jdk?.percent} />
                 <StatusCheck label="Android cmdline-tools" installed={status.cmdline_tools} installing={installing['cmdline-tools']} percent={progress['cmdline-tools']?.percent} />
                 <StatusCheck label="Android platform-tools" installed={status.platform_tools} installing={installing['platform-tools'] || installing['install:platform-tools']} percent={progress['platform-tools']?.percent} />
                 <StatusCheck label="Android Emulator" installed={status.emulator} installing={installing['emulator'] || installing['install:emulator']} percent={progress['emulator']?.percent} />
               </div>
                {(hasMissing || statusExpanded) && (
                  <div className="sdk-actions">
                    <button
                      className="btn-accent"
                      onClick={handleInstallAll}
                      disabled={installAllDisabled}
                    >
                      {installAllDisabled ? 'Installing…' : 'Install All'}
                    </button>
                    {!status.jdk && (
                      <button className="btn-ghost" onClick={() => { console.log('[SdkManager] Install JDK clicked'); installJdk(); }} disabled={!!installing.jdk}>
                        {installing.jdk ? 'Installing JDK…' : 'Install JDK'}
                      </button>
                    )}
                    {!status.cmdline_tools && (
                      <button className="btn-ghost" onClick={() => { console.log('[SdkManager] Install cmdline-tools clicked'); installCmdlineTools(); }} disabled={installing['cmdline-tools']}>
                        {installing['cmdline-tools'] ? 'Installing…' : 'Install cmdline-tools'}
                      </button>
                    )}
                    {!status.platform_tools && (
                      <button className="btn-ghost" onClick={() => { console.log('[SdkManager] Install platform-tools clicked'); installPackage('platform-tools'); }} disabled={!!installing['install:platform-tools'] || !!installing.platform_tools}>
                        {(installing['install:platform-tools'] || installing.platform_tools) ? 'Installing…' : 'Install platform-tools'}
                      </button>
                    )}
                    {!status.emulator && (
                      <button className="btn-ghost" onClick={() => { console.log('[SdkManager] Install emulator clicked'); installEmulator(); }} disabled={!!installing.emulator}>
                        {installing.emulator ? 'Installing…' : 'Install emulator'}
                      </button>
                    )}
                    {status.cmdline_tools && (
                      <button className="btn-ghost" onClick={acceptLicenses} disabled={installing.licenses}>
                        {installing.licenses ? 'Accepting…' : 'Accept Licenses'}
                      </button>
                    )}
                  </div>
                )}
             </>
           ) : (
             <p className="empty-text">Unable to determine install status.</p>
           )}

          <ProgressBar progress={progress.jdk} />
          <ProgressBar progress={progress['cmdline-tools']} />
          <ProgressBar progress={progress['platform-tools']} />
          <ProgressBar progress={progress['emulator']} />
          {packages.filter((p) => p.installed).map((p) => (
            <ProgressBar key={p.id} progress={progress[`install:${p.id}`] || progress[`uninstall:${p.id}`]} />
          ))}
        </Card>

        {/* ---- Live log console ---- */}
        <Card title="Install Log" className="card-sdk-log">
          <div className="log-console-header">
            <span className="dots">
              <span className="dot dot-red" />
              <span className="dot dot-amber" />
              <span className="dot dot-green" />
            </span>
            {anyInstalling && <span className="log-console-live" />}
          </div>
          <div className="log-console">
            {logs.length === 0 ? (
              <p className="empty-text">No activity yet. Run an install to see live output.</p>
            ) : (
              logs.map((l, i) => (
                <div key={i} className="log-line">
                  <span className="log-stage">[{l.stage}]</span> {l.line}
                </div>
              ))
            )}
          </div>
        </Card>
      </div>

      {/* ---- Package browser ---- */}
      <Card title="SDK Packages" className="card-sdk-packages">
        <div className="sdk-search">
          <input
            type="text"
            className="input-search"
            placeholder="Search packages (e.g. platform-tools, system-images)…"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
          />
        </div>

        {packages.length === 0 ? (
          status && status.cmdline_tools ? (
            <p className="empty-text">No packages loaded. The cmdline-tools are installed but <code>sdkmanager</code> may not be running correctly — verify the JDK is working (JAVA_HOME) and click Refresh, or reinstall cmdline-tools.</p>
          ) : (
            <p className="empty-text">No packages. Make sure cmdline-tools are installed and click Refresh.</p>
          )
        ) : (
          <div className="sdk-package-list">
            {packages.filter((p) => {
              const q = search.toLowerCase();
              const hay = `${p.id} ${p.desc} ${p.version}`.toLowerCase();
              const matchesSearch = !q || hay.includes(q);
              const isAllowed = isAllowedSystemImage(p.id);
              return matchesSearch && isAllowed;
            }).map((p) => {
              const installingKey = p.installed ? `uninstall:${p.id}` : p.id;
              const busy = installing[installingKey] || installing[p.id] || installing[`install:${p.id}`] || installing[`uninstall:${p.id}`];
              return (
                 <div key={p.id} className="sdk-package-row">
                   <div className="sdk-package-info">
                     <div className="sdk-package-name">{p.desc || p.name}</div>
                     <div className="sdk-package-meta">
                       <span className="sdk-package-version">v{p.version || '—'}</span>
                       <span className="sdk-package-category">{p.category}</span>
                     </div>
                   </div>
                   <div className="sdk-package-actions">
                    {p.installed ? (
                      <button
                        className="btn-danger"
                        onClick={() => uninstallPackage(p.id)}
                        disabled={busy}
                        title="Uninstall"
                      >
                        {busy ? 'Removing…' : 'Uninstall'}
                      </button>
                    ) : (
                      <button
                        className="btn-accent"
                        onClick={() => installPackage(p.id)}
                        disabled={busy}
                        title="Install"
                      >
                        {busy ? 'Installing…' : 'Install'}
                      </button>
                    )}
                  </div>
                </div>
              );
            })}
          </div>
        )}
      </Card>
    </div>
  );
}
