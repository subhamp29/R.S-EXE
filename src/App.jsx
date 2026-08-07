import { useState, useEffect } from 'react';
import TitleBar from './components/TitleBar';
import Sidebar from './components/Sidebar';
import Dashboard from './components/Dashboard';
import SdkManager from './components/SdkManager';
import Devices from './components/Devices';
import Settings from './components/Settings';
import ErrorBoundary from './components/ErrorBoundary';
import { SdkInstallProvider } from './contexts/SdkInstallContext';
import { listen } from '@tauri-apps/api/event';

export default function App() {
  const [active, setActive] = useState('dashboard');

  // Listen for the single-instance event — when a second launch attempt is
  // detected, the backend emits this so the frontend can show a brief toast.
  useEffect(() => {
    let unlisten = null;
    let cancelled = false;
    listen('single-instance', (e) => {
      if (cancelled) return;
      const msg = e.payload || 'R.S EXE is already running';
      // Show a brief inline toast rather than a blocking alert.
      const toast = document.createElement('div');
      toast.className = 'single-instance-toast';
      toast.textContent = msg;
      document.body.appendChild(toast);
      setTimeout(() => toast.remove(), 3000);
    })
      .then((fn) => { if (!cancelled) unlisten = fn; })
      .catch((err) => console.error('[App] Failed to listen for single-instance:', err));
    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, []);

  const renderPage = () => {
    switch (active) {
      case 'dashboard':
        return <Dashboard />;
      case 'devices':
        return <Devices onNavigate={setActive} />;
      case 'sdk':
        return <SdkManager onNavigate={setActive} />;
      case 'settings':
        return <Settings />;
      default:
        return <Dashboard />;
    }
  };

  return (
    <SdkInstallProvider>
      <div className="app-root">
        <ErrorBoundary>
          <TitleBar />
          <div className="app-body">
            <Sidebar active={active} onNavigate={setActive} />
            <main className="app-content">
              {renderPage()}
            </main>
          </div>
        </ErrorBoundary>
      </div>
    </SdkInstallProvider>
  );
}
