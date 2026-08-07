import { useState, useEffect, useRef, useCallback } from 'react';
import { listen } from '@tauri-apps/api/event';
import * as api from '../lib/api';

const MAX_BOOT_LOGS = 2000;

/**
 * Backend-authoritative emulator control hook.
 *
 * Survives navigation by syncing running state from the backend on mount and
 * listening for live boot-log events. This mirrors the pattern used by
 * `useSdkStatus` for install state.
 */
export function useEmulatorControl() {
  const [runningAvds, setRunningAvds] = useState([]);
  const [booting, setBooting] = useState({});
  const [logs, setLogs] = useState([]);
  const [error, setError] = useState(null);

  const isRefreshing = useRef(false);

  const appendLog = useCallback((entry) => {
    setLogs((prev) => {
      const next = [...prev, entry];
      if (next.length > MAX_BOOT_LOGS) next.shift();
      return next;
    });
  }, []);

  const clearLogs = useCallback(() => setLogs([]), []);

  // Sync running state from backend. This is the single source of truth.
  const refreshRunning = useCallback(async () => {
    if (isRefreshing.current) return;
    isRefreshing.current = true;
    try {
      const res = await api.getRunningAvds();
      if (res.ok) {
        const names = new Set(res.output);
        setRunningAvds((prev) => {
          // Merge backend state with local booting flags so we don't lose
          // in-flight boot status during a refresh.
          const next = prev.filter((a) => booting[a.name] && !names.has(a.name));
          for (const name of names) {
            if (!next.some((a) => a.name === name)) {
              next.push({ name, running: true });
            }
          }
          return next;
        });
      } else {
        setError(res.error);
      }
    } catch (e) {
      setError(String(e));
    } finally {
      isRefreshing.current = false;
    }
  }, [booting]);

  // Boot an AVD. Returns immediately; actual boot runs in the background.
  const bootAvd = useCallback(
    async (name, options = {}) => {
      setBooting((prev) => ({ ...prev, [name]: true }));
      setError(null);
      appendLog({ stage: `boot:${name}`, line: `Requesting boot for '${name}'…` });
      const merged = { no_snapshot: true, ...options };
      const res = await api.bootAvd(name, merged);
      if (res.ok) {
        appendLog({ stage: `boot:${name}`, line: `Boot command accepted for '${name}'` });
        // Optimistically mark as running; backend will confirm via events.
        setRunningAvds((prev) => {
          if (prev.some((a) => a.name === name)) return prev;
          return [...prev, { name, running: true }];
        });
        // Refresh after a short delay to confirm backend state.
        setTimeout(() => refreshRunning(), 3000);
      } else {
        appendLog({ stage: `boot:${name}`, line: `Boot failed: ${res.error}` });
        setError(res.error);
        setBooting((prev) => ({ ...prev, [name]: false }));
      }
      return res;
    },
    [appendLog, refreshRunning]
  );

  // Stop an AVD.
  const stopAvd = useCallback(
    async (name) => {
      setBooting((prev) => ({ ...prev, [name]: true }));
      setError(null);
      appendLog({ stage: `stop:${name}`, line: `Requesting stop for '${name}'…` });
      const res = await api.stopAvd(name);
      if (res.ok) {
        appendLog({ stage: `stop:${name}`, line: `Stop command accepted for '${name}'` });
        setRunningAvds((prev) => prev.filter((a) => a.name !== name));
        setBooting((prev) => {
          const next = { ...prev };
          delete next[name];
          return next;
        });
        setTimeout(() => refreshRunning(), 1000);
      } else {
        appendLog({ stage: `stop:${name}`, line: `Stop failed: ${res.error}` });
        setError(res.error);
        setBooting((prev) => ({ ...prev, [name]: false }));
      }
      return res;
    },
    [appendLog, refreshRunning]
  );

  const forceStopAvd = useCallback(
    async (name) => {
      setBooting((prev) => ({ ...prev, [name]: true }));
      setError(null);
      appendLog({ stage: `stop:${name}`, line: `Force-stopping '${name}'…` });
      const res = await api.forceStopAvd(name);
      if (res.ok) {
        appendLog({ stage: `stop:${name}`, line: `Force stop completed for '${name}'` });
        setRunningAvds((prev) => prev.filter((a) => a.name !== name));
        setBooting((prev) => {
          const next = { ...prev };
          delete next[name];
          return next;
        });
        setTimeout(() => refreshRunning(), 1000);
      } else {
        appendLog({ stage: `stop:${name}`, line: `Force stop failed: ${res.error}` });
        setError(res.error);
        setBooting((prev) => ({ ...prev, [name]: false }));
      }
      return res;
    },
    [appendLog, refreshRunning]
  );

  // Initial sync + global event listeners.
  useEffect(() => {
    refreshRunning();

    // Listen for boot log lines from the backend.
    // NOTE: `listen()` returns a Promise<UnlistenFn>. We never `await` the
    // returned promise into a variable and then call `.then` on it — that was
    // the root cause of the black-screen crash (the stored value would be the
    // resolved unlisten *function*, which has no `.then`). Instead we use the
    // robust "cancelled flag + store resolved fn" pattern so cleanup is correct
    // regardless of whether the promise has resolved yet.
    let bootLogUnsub = null;
    let bootLogCancelled = false;
    listen('sdk-log', (e) => {
      const p = e.payload;
      if (typeof p === 'object' && p && p.stage && p.line) {
        appendLog(p);
      }
    })
      .then((fn) => {
        if (bootLogCancelled) {
          // Effect already cleaned up before the promise resolved — unlisten the
          // soon-to-be-orphaned listener immediately so it can't fire.
          fn();
        } else {
          bootLogUnsub = fn;
        }
      })
      .catch((err) => {
        console.error('[useEmulatorControl] Failed to listen for sdk-log:', err);
      });

    // Listen for boot done events so we can clear booting flags.
    let bootDoneUnsub = null;
    let bootDoneCancelled = false;
    listen('sdk-install-done', (e) => {
      const p = e.payload;
      if (p && p.component && p.component.startsWith('boot:')) {
        const avdName = p.component.slice(5);
        setBooting((prev) => {
          const next = { ...prev };
          delete next[avdName];
          return next;
        });
        if (!p.ok) {
          setRunningAvds((prev) => prev.filter((a) => a.name !== avdName));
          appendLog({ stage: p.component, line: `Boot error: ${p.message}` });
        }
      }
    })
      .then((fn) => {
        if (bootDoneCancelled) {
          fn();
        } else {
          bootDoneUnsub = fn;
        }
      })
      .catch((err) => {
        console.error('[useEmulatorControl] Failed to listen for sdk-install-done:', err);
      });

    return () => {
      bootLogCancelled = true;
      if (bootLogUnsub) bootLogUnsub();
      bootDoneCancelled = true;
      if (bootDoneUnsub) bootDoneUnsub();
    };
  }, [appendLog, refreshRunning]);

  // Helper: is a specific AVD running?
  const isRunning = useCallback(
    (name) => runningAvds.some((a) => a.name === name),
    [runningAvds]
  );

  // Optimize all user-installed apps on a running AVD via AOT compilation.
  const optimizeInstalledApps = useCallback(
    async (name) => {
      setError(null);
      appendLog({ stage: `optimize:${name}`, line: `Requesting app optimization for '${name}'…` });
      const res = await api.optimizeInstalledApps(name);
      if (!res.ok) {
        appendLog({ stage: `optimize:${name}`, line: `Optimization failed: ${res.error}` });
        setError(res.error);
      }
      return res;
    },
    [appendLog]
  );

  // Install an APK on a running AVD.
  const installApk = useCallback(
    async (avdName, apkPath) => {
      setError(null);
      appendLog({ stage: `apk:${avdName}`, line: `Installing APK on '${avdName}'…` });
      const res = await api.installApk(avdName, apkPath);
      if (!res.ok) {
        appendLog({ stage: `apk:${avdName}`, line: `APK install failed: ${res.error}` });
        setError(res.error);
      } else {
        appendLog({ stage: `apk:${avdName}`, line: `APK installed successfully on '${avdName}'` });
      }
      return res;
    },
    [appendLog]
  );

  // List installed apps on a running AVD.
  const listInstalledApps = useCallback(
    async (avdName) => {
      setError(null);
      const res = await api.listInstalledApps(avdName);
      if (!res.ok) {
        appendLog({ stage: `apps:${avdName}`, line: `Failed to list apps: ${res.error}` });
        setError(res.error);
      }
      return res;
    },
    [appendLog]
  );

  // Uninstall an app from a running AVD.
  const uninstallApp = useCallback(
    async (avdName, packageName) => {
      setError(null);
      appendLog({ stage: `uninstall:${avdName}`, line: `Uninstalling '${packageName}' from '${avdName}'…` });
      const res = await api.uninstallApp(avdName, packageName);
      if (!res.ok) {
        appendLog({ stage: `uninstall:${avdName}`, line: `Uninstall failed: ${res.error}` });
        setError(res.error);
      } else {
        appendLog({ stage: `uninstall:${avdName}`, line: `Uninstalled '${packageName}' from '${avdName}'` });
      }
      return res;
    },
    [appendLog]
  );

  // Launch an installed app on a running AVD.
  const launchApp = useCallback(
    async (avdName, packageName) => {
      setError(null);
      appendLog({ stage: `launch:${avdName}`, line: `Launching '${packageName}' on '${avdName}'…` });
      const res = await api.launchApp(avdName, packageName);
      if (!res.ok) {
        appendLog({ stage: `launch:${avdName}`, line: `Launch failed: ${res.error}` });
        setError(res.error);
      } else {
        appendLog({ stage: `launch:${avdName}`, line: `Launched '${packageName}' on '${avdName}'` });
      }
      return res;
    },
    [appendLog]
  );

  // Capture a screenshot on a running AVD.
  const captureScreenshot = useCallback(
    async (avdName) => {
      setError(null);
      appendLog({ stage: `screenshot:${avdName}`, line: `Capturing screenshot on '${avdName}'…` });
      const res = await api.captureScreenshot(avdName);
      if (!res.ok) {
        appendLog({ stage: `screenshot:${avdName}`, line: `Screenshot failed: ${res.error}` });
        setError(res.error);
      } else if (res.output) {
        appendLog({ stage: `screenshot:${avdName}`, line: `Screenshot saved to: ${res.output}` });
      }
      return res;
    },
    [appendLog]
  );

  return {
    runningAvds,
    booting,
    logs,
    error,
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
  };
}
