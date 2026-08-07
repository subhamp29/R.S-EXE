import { useState, useEffect, useCallback, useRef } from 'react';
import * as api from '../lib/api';

const POLL_INTERVAL_MS = 4000;
const SAFETY_TIMEOUT_MS = 15000; // Force loading=false after 15s even if promises hang

export function useSystemInfo() {
  const [systemInfo, setSystemInfo] = useState(null);
  const [gpus, setGpus] = useState([]);
  const [hypervisor, setHypervisor] = useState(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(null);
  const [lastUpdated, setLastUpdated] = useState(null);

  // In-flight guard: prevents overlapping poll calls
  const isFetching = useRef(false);
  const safetyTimerRef = useRef(null);

  const fetchSystemInfo = useCallback(async () => {
    try {
      const result = await api.getSystemInfo();
      if (result.ok) {
        setSystemInfo(result.output);
      } else {
        setError(prev => prev || result.error);
      }
    } catch (e) {
      setError(prev => prev || String(e));
    }
  }, []);

  const fetchStaticData = useCallback(async () => {
    setError(null);

    const [gpuResult, hvResult] = await Promise.all([
      api.detectGpus(),
      api.checkHypervisor(),
    ]);

    if (gpuResult.ok) {
      setGpus(Array.isArray(gpuResult.output) ? gpuResult.output : []);
    } else {
      setError(prev => prev || gpuResult.error);
    }

    if (hvResult.ok) {
      setHypervisor(hvResult.output);
    } else {
      setError(prev => prev || hvResult.error);
    }

    setLastUpdated(new Date());
  }, []);

  // Initial fetch: system info + static data
  const fetchAll = useCallback(async () => {
    if (isFetching.current) return;
    isFetching.current = true;

    // Safety net: if promises hang forever, force loading=false
    if (safetyTimerRef.current) clearTimeout(safetyTimerRef.current);
    safetyTimerRef.current = setTimeout(() => {
      if (isFetching.current) {
        isFetching.current = false;
        setLoading(false);
      }
    }, SAFETY_TIMEOUT_MS);

    try {
      await fetchSystemInfo();
      await fetchStaticData();
    } finally {
      isFetching.current = false;
      setLoading(false);
      if (safetyTimerRef.current) clearTimeout(safetyTimerRef.current);
    }
  }, [fetchSystemInfo, fetchStaticData]);

  // Manual refresh for static data (GPU, hypervisor) only
  const refetchStatic = useCallback(async () => {
    if (isFetching.current) return;
    isFetching.current = true;

    // Safety net
    if (safetyTimerRef.current) clearTimeout(safetyTimerRef.current);
    safetyTimerRef.current = setTimeout(() => {
      if (isFetching.current) {
        isFetching.current = false;
      }
    }, SAFETY_TIMEOUT_MS);

    try {
      await fetchStaticData();
    } finally {
      isFetching.current = false;
      if (safetyTimerRef.current) clearTimeout(safetyTimerRef.current);
    }
  }, [fetchStaticData]);

  // Poll only dynamic data (system info / RAM) on interval
  useEffect(() => {
    fetchAll();
    const timer = setInterval(async () => {
      if (isFetching.current) return;
      isFetching.current = true;
      try {
        await fetchSystemInfo();
      } finally {
        isFetching.current = false;
      }
    }, POLL_INTERVAL_MS);

    return () => {
      clearInterval(timer);
      if (safetyTimerRef.current) clearTimeout(safetyTimerRef.current);
    };
  }, [fetchAll, fetchSystemInfo]);

  return { systemInfo, gpus, hypervisor, loading, error, lastUpdated, refetch: refetchStatic };
}
