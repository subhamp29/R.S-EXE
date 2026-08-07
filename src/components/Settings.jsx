import { useState, useEffect } from 'react';
import Card from './Card';
import * as api from '../lib/api';

export default function Settings() {
  const [settings, setSettings] = useState({
    sdk_override: '',
    jdk_override: '',
    screenshot_dir: '',
  });
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState(null);
  const [resetConfirm, setResetConfirm] = useState(false);

  useEffect(() => {
    let cancelled = false;
    api.getAppSettings()
      .then((r) => {
        if (cancelled) return;
        if (r.ok && r.output) {
          setSettings({
            sdk_override: r.output.sdk_override || '',
            jdk_override: r.output.jdk_override || '',
            screenshot_dir: r.output.screenshot_dir || '',
          });
        }
        setLoading(false);
      })
      .catch((e) => {
        if (cancelled) return;
        setError(String(e));
        setLoading(false);
      });
    return () => { cancelled = true; };
  }, []);

  const handleChange = (field) => (e) => {
    setSettings((prev) => ({ ...prev, [field]: e.target.value }));
  };

  const handleSave = async () => {
    setSaving(true);
    setError(null);
    const res = await api.saveAppSettings(settings);
    if (!res.ok) {
      setError(res.error);
    }
    setSaving(false);
  };

  const handleReset = async () => {
    setResetConfirm(false);
    const res = await api.resetAppSettings();
    if (res.ok) {
      setSettings({ sdk_override: '', jdk_override: '', screenshot_dir: '' });
      setError(null);
    } else {
      setError(res.error);
    }
  };

  const handlePickScreenshotDir = async () => {
    try {
      const { open } = await import('@tauri-apps/plugin-dialog');
      const selected = await open({
        directory: true,
        title: 'Choose Screenshot Save Location',
      });
      if (selected && typeof selected === 'string') {
        setSettings((prev) => ({ ...prev, screenshot_dir: selected }));
      }
    } catch (e) {
      console.error('[Settings] Directory picker failed:', e);
    }
  };

  const handlePickSdkDir = async () => {
    try {
      const { open } = await import('@tauri-apps/plugin-dialog');
      const selected = await open({
        directory: true,
        title: 'Choose Android SDK Location',
      });
      if (selected && typeof selected === 'string') {
        setSettings((prev) => ({ ...prev, sdk_override: selected }));
      }
    } catch (e) {
      console.error('[Settings] SDK dir picker failed:', e);
    }
  };

  const handlePickJdkDir = async () => {
    try {
      const { open } = await import('@tauri-apps/plugin-dialog');
      const selected = await open({
        directory: true,
        title: 'Choose JDK Location',
      });
      if (selected && typeof selected === 'string') {
        setSettings((prev) => ({ ...prev, jdk_override: selected }));
      }
    } catch (e) {
      console.error('[Settings] JDK dir picker failed:', e);
    }
  };

  if (loading) {
    return (
      <div className="settings-page">
        <Card title="Settings">
          <p className="text-muted">Loading settings…</p>
        </Card>
      </div>
    );
  }

  return (
    <div className="settings-page">
      <div className="settings-header">
        <h1 className="dashboard-heading">Settings</h1>
      </div>

      {error && (
        <div className="error-banner" role="alert">
          <span>{error}</span>
        </div>
      )}

      <div className="settings-grid">
        <Card title="Paths" className="settings-card">
          <div className="settings-field">
            <label>Android SDK Path Override</label>
            <div className="settings-path-row">
              <input
                className="input-text"
                value={settings.sdk_override}
                onChange={handleChange('sdk_override')}
                placeholder="e.g. D:\\Android\\Sdk"
              />
              <button className="btn-ghost btn-sm" onClick={handlePickSdkDir} type="button">Browse</button>
            </div>
            <small className="text-muted">
              Leave blank to use the default managed location. Useful if you already have Android Studio installed elsewhere.
            </small>
          </div>

          <div className="settings-field">
            <label>JDK Path Override</label>
            <div className="settings-path-row">
              <input
                className="input-text"
                value={settings.jdk_override}
                onChange={handleChange('jdk_override')}
                placeholder="e.g. D:\\Android\\Sdk\\jdk"
              />
              <button className="btn-ghost btn-sm" onClick={handlePickJdkDir} type="button">Browse</button>
            </div>
            <small className="text-muted">
              Leave blank to use <code>sdk/jdk</code> relative to the SDK path.
            </small>
          </div>

          <div className="settings-field">
            <label>Screenshot Save Location</label>
            <div className="settings-path-row">
              <input
                className="input-text"
                value={settings.screenshot_dir}
                onChange={handleChange('screenshot_dir')}
                placeholder="Default app data folder"
              />
              <button className="btn-ghost btn-sm" onClick={handlePickScreenshotDir} type="button">Browse</button>
            </div>
            <small className="text-muted">
              Where screenshots captured from running AVDs are saved.
            </small>
          </div>
        </Card>

        <Card title="About" className="settings-card">
          <div className="settings-about">
            <div className="settings-about-name">R.S EXE</div>
            <div className="settings-about-version">Version 0.1.0</div>
            <p className="text-muted" style={{ marginTop: 8 }}>
              Desktop Android Virtual Device (AVD) Manager.
              Built with Tauri, React, and the Android SDK tools.
            </p>
            <p className="text-muted" style={{ marginTop: 4, fontSize: '0.85em' }}>
              Created by Subham Mahapatra
            </p>
          </div>
        </Card>
      </div>

      <div className="settings-actions">
        <button className="btn-accent" onClick={handleSave} disabled={saving}>
          {saving ? 'Saving…' : 'Save Settings'}
        </button>
        <button
          className="btn-danger"
          onClick={() => setResetConfirm(true)}
          type="button"
        >
          Reset to Defaults
        </button>
      </div>

      {resetConfirm && (
        <div className="modal-backdrop" onClick={() => setResetConfirm(false)}>
          <div className="modal" onClick={(e) => e.stopPropagation()}>
            <div className="modal-header">
              <h2 className="dashboard-heading">Reset Settings</h2>
              <button className="btn-ghost" onClick={() => setResetConfirm(false)}>✕</button>
            </div>
            <div className="modal-body">
              <p>
                This will clear all R.S EXE app-level settings (SDK overrides,
                screenshot location, etc.) back to their defaults. AVD data is
                not affected.
              </p>
              <p style={{ color: 'var(--color-danger)', fontWeight: 500 }}>
                This cannot be undone.
              </p>
            </div>
            <div className="modal-footer">
              <button className="btn-ghost" onClick={() => setResetConfirm(false)}>Cancel</button>
              <button className="btn-danger" onClick={handleReset}>Reset</button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}