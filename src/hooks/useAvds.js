import { useState, useEffect, useRef } from 'react';
import { listen } from '@tauri-apps/api/event';
import * as api from '../lib/api';

// Mirrors useSystemInfo: on-demand load of AVDs with an in-flight guard, plus a
// Tauri event listener so the live log console (shared SDK events) works while
// create/delete run.
export function useAvds() {
  const [avds, setAvds] = useState([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(null);
  const [logs, setLogs] = useState([]);
  const [creating, setCreating] = useState(false);
  const [deleting, setDeleting] = useState({});
  const [booting, setBooting] = useState({});

  const isFetching = useRef(false);

  const appendLog = (entry) =>
    setLogs((prev) => {
      const next = [...prev, entry];
      if (next.length > 2000) next.shift();
      return next;
    });

  const fetchAll = async () => {
    if (isFetching.current) return;
    isFetching.current = true;
    try {
      const res = await api.listAvds();
      if (res.ok) setAvds(res.output || []);
      else setError(res.error);
    } catch (e) {
      setError(String(e));
    } finally {
      isFetching.current = false;
      setLoading(false);
    }
  };

  useEffect(() => {
    fetchAll();
  }, []);

  // Listen for shared sdk-log events so create/delete activity is visible.
  useEffect(() => {
    // `listen()` returns a Promise<UnlistenFn>. Use the robust "cancelled flag
    // + store resolved fn" pattern so cleanup is correct whether or not the
    // promise has resolved (same fix applied to useEmulatorControl/useSdkStatus).
    let unlisten = null;
    let cancelled = false;
    listen('sdk-log', (e) => appendLog(e.payload))
      .then((fn) => {
        if (cancelled) {
          fn();
        } else {
          unlisten = fn;
        }
      })
      .catch((err) => {
        console.error('[useAvds] Failed to listen for sdk-log:', err);
      });
    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, []);

  const createAvd = async (input) => {
    setCreating(true);
    setError(null);
    const res = await api.createAvd(input);
    if (res.ok) {
      appendLog({ stage: `create:${input.name}`, line: `AVD '${input.name}' created successfully` });
      await fetchAll();
    } else {
      appendLog({ stage: `create:${input.name}`, line: res.error });
      setError(res.error);
    }
    setCreating(false);
    return res;
  };

   const deleteAvd = async (name) => {
    setDeleting((prev) => ({ ...prev, [name]: true }));
    setError(null);
    const res = await api.deleteAvd(name);
    if (res.ok) {
      appendLog({ stage: `delete:${name}`, line: `AVD '${name}' deleted` });
      setAvds((prev) => prev.filter((a) => a.name !== name));
    } else {
      appendLog({ stage: `delete:${name}`, line: res.error });
      setError(res.error);
    }
    setDeleting((prev) => ({ ...prev, [name]: false }));
    return res;
  };

  const startAvd = async (name) => {
    setBooting((prev) => ({ ...prev, [name]: true }));
    setError(null);
    const res = await api.startAvd(name);
    if (res.ok) {
      appendLog({ stage: `boot:${name}`, line: `AVD '${name}' boot started` });
      // Mark as running optimistically in the local list.
      setAvds((prev) =>
        prev.map((a) => (a.name === name ? { ...a, running: true } : a))
      );
    } else {
      appendLog({ stage: `boot:${name}`, line: res.error });
      setError(res.error);
    }
    setBooting((prev) => ({ ...prev, [name]: false }));
    return res;
  };

  const stopAvd = async (name) => {
    setBooting((prev) => ({ ...prev, [name]: true }));
    setError(null);
    const res = await api.stopAvd(name);
    if (res.ok) {
      appendLog({ stage: `stop:${name}`, line: `AVD '${name}' stopped` });
      setAvds((prev) =>
        prev.map((a) => (a.name === name ? { ...a, running: false } : a))
      );
    } else {
      appendLog({ stage: `stop:${name}`, line: res.error });
      setError(res.error);
    }
    setBooting((prev) => ({ ...prev, [name]: false }));
    return res;
  };

   return {
    avds,
    loading,
    error,
    logs,
    creating,
    deleting,
    booting,
    fetchAll,
    createAvd,
    deleteAvd,
    startAvd,
    stopAvd,
  };
}
