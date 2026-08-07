import { useState, useEffect, useRef } from 'react';
import { listen } from '@tauri-apps/api/event';
import * as api from '../lib/api';
import { useSdkInstall } from '../contexts/SdkInstallContext';

function ok(output) {
  return { ok: true, error: null, output };
}

function fail(error) {
  return { ok: false, error, output: null };
}

// Mirrors useSystemInfo: on-demand fetch with an in-flight guard, plus Tauri
// event listeners that feed the live log console and per-component progress.
// NOTE: logs / progress / done / installing are now provided by
// SdkInstallContext so they survive component unmount during tab switches.
export function useSdkStatus() {
  const [status, setStatus] = useState(null);
  const [packages, setPackages] = useState([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(null);

  const {
    logs,
    progress,
    done,
    installing,
    appendLog,
    clearLogs,
    syncFromBackend,
    markInstalling,
  } = useSdkInstall();

  const isFetching = useRef(false);

  const fetchAll = async () => {
    if (isFetching.current) return;
    isFetching.current = true;
    try {
      const [sRes, pRes] = await Promise.all([
        api.checkInstallStatus(),
        api.fetchSdkPackages(),
      ]);
      if (sRes.ok) setStatus(sRes.output);
      else setError(sRes.error);
      if (pRes.ok) setPackages(pRes.output || []);
    } catch (e) {
      setError(String(e));
    } finally {
      isFetching.current = false;
      setLoading(false);
    }
  };

  // Initial load + sync from backend shared state so we catch any in-flight
  // installs that started while the user was on another tab.
  useEffect(() => {
    fetchAll();
    syncFromBackend();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Auto-refresh install status whenever any install completes so the
  // Install Status panel flips from Missing → Installed without a manual
  // page refresh or Refresh button click.
  useEffect(() => {
    // `listen()` returns a Promise<UnlistenFn>. Use the robust "cancelled flag
    // + store resolved fn" pattern so cleanup works whether or not the promise
    // has resolved yet (avoids the `.then`-on-a-function crash seen elsewhere).
    let unlisten = null;
    let cancelled = false;
    listen('sdk-install-done', () => {
      fetchAll();
    })
      .then((fn) => {
        if (cancelled) {
          fn();
        } else {
          unlisten = fn;
        }
      })
      .catch((err) => {
        console.error('[useSdkStatus] Failed to listen for sdk-install-done:', err);
      });
    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, [fetchAll]);

  const installJdk = async () => {
    if (installing.jdk) return ok(true);
    markInstalling('jdk', true);
    appendLog({ stage: 'install-all', line: 'Starting JDK install…' });
    const res = await api.installJdk();
    if (!res.ok) {
      appendLog({ stage: 'jdk', line: res.error });
    }
    markInstalling('jdk', false);
    return res;
  };

  const installCmdlineTools = async () => {
    if (installing['cmdline-tools']) return ok(true);
    markInstalling('cmdline-tools', true);
    appendLog({ stage: 'install-all', line: 'Starting cmdline-tools install…' });
    const res = await api.installCmdlineTools();
    if (!res.ok) appendLog({ stage: 'cmdline-tools', line: res.error });
    markInstalling('cmdline-tools', false);
    return res;
  };

  const acceptLicenses = async () => {
    if (installing.licenses) return ok(true);
    markInstalling('licenses', true);
    appendLog({ stage: 'install-all', line: 'Accepting licenses…' });
    const res = await api.acceptLicenses();
    if (!res.ok) appendLog({ stage: 'licenses', line: res.error });
    markInstalling('licenses', false);
    return res;
  };

   const installEmulator = async () => {
     if (installing.emulator) return ok(true);
     markInstalling('emulator', true);
     appendLog({ stage: 'install-all', line: 'Starting emulator install…' });
     const res = await api.installPackage('emulator');
     if (!res.ok) appendLog({ stage: 'emulator', line: res.error });
     markInstalling('emulator', false);
     return res;
   };

   const installPackage = async (pkg) => {
     const key = `install:${pkg}`;
     if (installing[key]) return ok(true);
     markInstalling(key, true);
     appendLog({ stage: 'install-all', line: `Starting install of ${pkg}…` });
     const res = await api.installPackage(pkg);
     if (!res.ok) appendLog({ stage: key, line: res.error });
     markInstalling(key, false);
     return res;
   };

  const uninstallPackage = async (pkg) => {
    const key = `uninstall:${pkg}`;
    if (installing[key]) return ok(true);
    markInstalling(key, true);
    const res = await api.uninstallPackage(pkg);
    if (!res.ok) appendLog({ stage: key, line: res.error });
    markInstalling(key, false);
    return res;
  };

  // "Install All" runs the missing pieces in sequence: JDK -> cmdline-tools ->
  // licenses -> platform-tools. We re-check status from the backend at each
  // gate (not the hook's possibly-stale `status`) so the sequence stays correct.
  const recheck = async () => (await api.checkInstallStatus()).output;

  const waitForInstall = async (componentKey, timeoutMs = 600000) => {
    const start = Date.now();
    while (Date.now() - start < timeoutMs) {
      const res = await api.getInstallProgress();
      if (res.ok && res.output) {
        const data = res.output;
        let comp = null;
        if (componentKey === 'jdk') comp = data.jdk;
        else if (componentKey === 'cmdline-tools') comp = data.cmdline_tools;
        else if (componentKey === 'platform-tools') comp = data.platform_tools;
        else if (componentKey === 'licenses') comp = data.licenses;
        else if (data.packages) comp = data.packages[componentKey];

        if (comp) {
         if (!comp.installing && comp.done) {
           if (comp.done.ok) {
             return ok(true);
           }
           // done exists but ok=false — the component failed. Return a
           // fail() so callers can distinguish success from failure
           // (previously returned ok(false) which masked the failure).
           return fail(comp.done.message || comp.error || 'Install failed');
         }
         if (comp.error) {
           return fail(comp.error);
         }
        }
      }
      await new Promise((r) => setTimeout(r, 1500));
    }
    return fail('Timeout waiting for install');
  };

   const installAll = async () => {
     appendLog({ stage: 'install-all', line: 'Starting Install All…' });
     let cur = await recheck();

     // Phase 6 — Parallelize independent JDK + cmdline-tools installs.
     // Both are independent of each other and can download concurrently,
     // cutting total install time roughly in half.
     const jdkMissing = !cur?.jdk && !installing.jdk;
     const cmdlineMissing = !cur?.cmdline_tools && !installing['cmdline-tools'];

     if (jdkMissing && cmdlineMissing) {
       // Both missing — launch in parallel.
       appendLog({ stage: 'install-all', line: 'Starting JDK + cmdline-tools in parallel…' });
       const [jdkRes, cmdlineRes] = await Promise.all([installJdk(), installCmdlineTools()]);

       if (!jdkRes.ok) {
         appendLog({ stage: 'install-all', line: `JDK install failed: ${jdkRes.error}` });
         return;
       }
       if (!cmdlineRes.ok) {
         appendLog({ stage: 'install-all', line: `cmdline-tools install failed: ${cmdlineRes.error}` });
         return;
       }

       // Wait for both to finish.
       const [jdkWait, cmdlineWait] = await Promise.all([
         waitForInstall('jdk'),
         waitForInstall('cmdline-tools'),
       ]);
       if (!jdkWait.ok) {
         appendLog({ stage: 'install-all', line: `JDK install failed: ${jdkWait.error}` });
         return;
       }
       if (!cmdlineWait.ok) {
         appendLog({ stage: 'install-all', line: `cmdline-tools install failed: ${cmdlineWait.error}` });
         return;
       }
       cur = await recheck();
     } else if (jdkMissing) {
       const r = await installJdk();
       if (!r.ok) {
         appendLog({ stage: 'install-all', line: `JDK install failed: ${r.error}` });
         return;
       }
       const w = await waitForInstall('jdk');
       if (!w.ok) {
         appendLog({ stage: 'install-all', line: `JDK install failed: ${w.error}` });
         return;
       }
       cur = await recheck();
     } else if (cmdlineMissing) {
       const r = await installCmdlineTools();
       if (!r.ok) {
         appendLog({ stage: 'install-all', line: `cmdline-tools install failed: ${r.error}` });
         return;
       }
       const w = await waitForInstall('cmdline-tools');
       if (!w.ok) {
         appendLog({ stage: 'install-all', line: `cmdline-tools install failed: ${w.error}` });
         return;
       }
       cur = await recheck();
     }

     // Accept licenses once cmdline-tools are present (idempotent).
     // This is a HARD gate: platform-tools (and any other package) cannot be
     // installed until licenses are accepted, otherwise sdkmanager exits with a
     // cryptic exit code 1.
     if (!installing.licenses) {
      const r = await acceptLicenses();
      if (!r.ok) {
        appendLog({ stage: 'install-all', line: `License acceptance failed: ${r.error}` });
        return;
      }
      const w = await waitForInstall('licenses');
      if (!w.ok) {
        appendLog({ stage: 'install-all', line: `License acceptance failed: ${w.error}` });
        return;
      }
      cur = await recheck();
    }
    if (!cur?.platform_tools && !installing['install:platform-tools'] && !installing['platform-tools']) {
      const r = await installPackage('platform-tools');
      if (!r.ok) {
        appendLog({ stage: 'install-all', line: `platform-tools install failed: ${r.error}` });
        return;
      }
      const w = await waitForInstall('install:platform-tools');
      if (!w.ok) {
        appendLog({ stage: 'install-all', line: `platform-tools install failed: ${w.error}` });
        return;
      }
    }
    // Install the emulator package so AVDs can actually boot.
    if (!cur?.emulator && !installing['install:emulator'] && !installing['emulator']) {
      const r = await installPackage('emulator');
      if (!r.ok) {
        appendLog({ stage: 'install-all', line: `emulator install failed: ${r.error}` });
        return;
      }
      const w = await waitForInstall('install:emulator');
      if (!w.ok) {
        appendLog({ stage: 'install-all', line: `emulator install failed: ${w.error}` });
        return;
      }
    }
    appendLog({ stage: 'install-all', line: 'Install All complete.' });
    await fetchAll();
  };

   const installAllDisabled = installing.jdk || installing['cmdline-tools'] || installing.licenses || installing['platform-tools'] || installing['install:platform-tools'] || installing['emulator'] || installing['install:emulator'];

   return {
     status,
     packages,
     loading,
     error,
     logs,
     progress,
     done,
     installing,
     installAllDisabled,
     fetchAll,
     installAll,
     installJdk,
     installCmdlineTools,
     installEmulator,
     acceptLicenses,
     installPackage,
     uninstallPackage,
     clearLogs,
   };
}
