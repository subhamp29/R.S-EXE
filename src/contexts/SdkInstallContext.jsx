import { createContext, useContext, useState, useCallback, useEffect, useRef } from 'react';
import { listen } from '@tauri-apps/api/event';
import * as api from '../lib/api';

const SdkInstallContext = createContext(null);

export function SdkInstallProvider({ children }) {
  const [logs, setLogs] = useState([]);
  const [progress, setProgress] = useState({});
  const [done, setDone] = useState({});
  const [installing, setInstalling] = useState({});
  // Track the last time each component's progress changed, for stall detection.
  const lastProgressTick = useRef({});

  const appendLog = useCallback((entry) => {
    setLogs((prev) => {
      // Collapse consecutive identical lines to reduce firehose noise.
      const last = prev[prev.length - 1];
      if (last && last.stage === entry.stage && last.line === entry.line) {
        return prev;
      }
      const next = [...prev, entry];
      if (next.length > 2000) next.shift();
      return next;
    });
  }, []);

  const clearLogs = useCallback(() => setLogs([]), []);

  // Sync from backend shared state on mount / remount.
  const syncFromBackend = useCallback(async () => {
    try {
      const res = await api.getInstallProgress();
      if (res.ok && res.output) {
        const data = res.output;
        const newInstalling = {};
        const newProgress = {};
        const newDone = {};
        const newLogs = [];

        const walk = (prefix, comp) => {
          if (comp.installing) newInstalling[prefix] = true;
          if (comp.progress) newProgress[prefix] = comp.progress;
          if (comp.done) newDone[prefix] = comp.done;
          if (comp.logs) newLogs.push(...comp.logs.map(l => ({ ...l })));
        };

        walk('jdk', data.jdk);
        walk('cmdline-tools', data.cmdline_tools);
        walk('platform-tools', data.platform_tools);
        walk('emulator', data.emulator);
        walk('licenses', data.licenses);

        for (const [key, comp] of Object.entries(data.packages || {})) {
          walk(key, comp);
        }

        setInstalling(newInstalling);
        setProgress(newProgress);
        setDone(newDone);
        if (newLogs.length > 0) {
          setLogs((prev) => {
            const merged = [...prev, ...newLogs];
            // deduplicate by stage+line
            const seen = new Set();
            return merged.filter(l => {
              const k = `${l.stage}\x00${l.line}`;
              if (seen.has(k)) return false;
              seen.add(k);
              return true;
            }).slice(-2000);
          });
        }
      }
    } catch (e) {
      console.error('[SdkInstallContext] syncFromBackend failed:', e);
    }
  }, []);

  useEffect(() => {
    syncFromBackend();
  }, [syncFromBackend]);

  // Global Tauri event listeners (live for app lifetime, not scoped to SdkManager).
  useEffect(() => {
    const events = ['sdk-install-progress', 'sdk-install-done', 'sdk-log'];
    // `listen()` returns a Promise<UnlistenFn>. We collect the resolved unlisten
    // fns and use a single `active` flag so that, if the effect cleans up before
    // a listener's promise resolves, we unlisten the orphaned listener right
    // away instead of leaking it. This also avoids calling `.then` on a resolved
    // function (the black-screen crash root cause).
    const unsubs = [];
    let active = true;

    for (const evt of events) {
      listen(evt, (e) => {
        const p = e.payload;
        if (evt === 'sdk-install-progress') {
          const key = p.component;
          const prev = lastProgressTick.current[key];
          const now = Date.now();
          if (prev && prev.pct === p.percent && now - prev.time > 60000) {
            console.warn(
              `[SdkInstallContext] STALL DETECTED: ${key} stuck at ${p.percent}% for ${Math.round((now - prev.time) / 1000)}s`
            );
          }
          lastProgressTick.current[key] = { pct: p.percent, time: now };
          setProgress((prev) => ({ ...prev, [key]: p }));
        } else if (evt === 'sdk-install-done') {
          setDone((prev) => ({ ...prev, [p.component]: p }));
          setInstalling((prev) => {
            const next = { ...prev };
            // handle both plain keys and "install:xxx"/"uninstall:xxx" keys
            const keysToClear = [p.component];
            if (p.component.startsWith('install:')) keysToClear.push(p.component);
            if (p.component.startsWith('uninstall:')) keysToClear.push(p.component);
            for (const k of keysToClear) delete next[k];
            return next;
          });
          appendLog({ stage: p.component, line: `[${p.component}] ${p.message}` });
        } else if (evt === 'sdk-log') {
          appendLog(p);
        }
      })
        .then((fn) => {
          if (active) {
            unsubs.push(fn);
          } else {
            // Cleaned up before the promise resolved — unlisten the orphan.
            fn();
          }
        })
        .catch((err) => {
          console.error(`[SdkInstallContext] Failed to listen for ${evt}:`, err);
        });
    }

    return () => {
      active = false;
      for (const fn of unsubs) {
        try {
          fn().catch(console.error);
        } catch (err) {
          console.error('[SdkInstallContext] unlisten error:', err);
        }
      }
    };
  }, [appendLog]);

  const markInstalling = useCallback((key, on) => {
    setInstalling((prev) => {
      const next = { ...prev };
      if (on) next[key] = true;
      else delete next[key];
      return next;
    });
  }, []);

  const value = {
    logs,
    progress,
    done,
    installing,
    appendLog,
    clearLogs,
    syncFromBackend,
    markInstalling,
  };

  return (
    <SdkInstallContext.Provider value={value}>
      {children}
    </SdkInstallContext.Provider>
  );
}

export function useSdkInstall() {
  const ctx = useContext(SdkInstallContext);
  if (!ctx) {
    // Fallback so the hook doesn't crash if used outside the provider.
    return {
      logs: [],
      progress: {},
      done: {},
      installing: {},
      appendLog: () => {},
      clearLogs: () => {},
      syncFromBackend: async () => {},
      markInstalling: () => {},
    };
  }
  return ctx;
}
