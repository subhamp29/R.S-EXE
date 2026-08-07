import { useSystemInfo } from '../hooks/useSystemInfo';
import Card from './Card';
import StatusBadge from './StatusBadge';

function formatBytes(bytes) {
  if (bytes == null || isNaN(bytes)) return '—';
  const gb = bytes / (1024 ** 3);
  return `${gb.toFixed(1)} GB`;
}

function ProgressBar({ used, total }) {
  if (!total || total === 0) return null;
  const pct = Math.min(100, Math.max(0, (used / total) * 100));
  return (
    <div className="progress-track">
      <div className="progress-fill" style={{ width: `${pct}%` }} />
      <span className="progress-label">{pct.toFixed(0)}% used</span>
    </div>
  );
}

export default function Dashboard() {
  const { systemInfo, gpus, hypervisor, loading, error, lastUpdated, refetch } = useSystemInfo();

  const handleRefreshStatic = async () => {
    await refetch();
  };

  return (
    <div className="dashboard">
      <div className="dashboard-header">
        <h1 className="dashboard-heading">Dashboard</h1>
        <span className="dashboard-subtitle">
          {loading
            ? 'Fetching system data…'
            : lastUpdated
              ? `Last updated ${lastUpdated.toLocaleTimeString()}`
              : 'System data ready'}
        </span>
      </div>

      {error && (
        <div className="error-banner" role="alert">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <circle cx="12" cy="12" r="10" /><line x1="12" y1="8" x2="12" y2="12" /><line x1="12" y1="16" x2="12.01" y2="16" />
          </svg>
          <span>{error}</span>
        </div>
      )}

      <div className="dashboard-grid">
        {/* RAM */}
        <Card title="Memory (RAM)" className="card-ram card-primary">
          {loading && !systemInfo ? (
            <div className="skeleton-line" />
          ) : systemInfo ? (
            <>
              <div className="stat-row">
                <span className="stat-label">Total</span>
                <span className="stat-value">{formatBytes(systemInfo.total_ram_bytes)}</span>
              </div>
              <div className="stat-row">
                <span className="stat-label">Free</span>
                <span className="stat-value">{formatBytes(systemInfo.free_ram_bytes)}</span>
              </div>
              <ProgressBar used={systemInfo.total_ram_bytes - systemInfo.free_ram_bytes} total={systemInfo.total_ram_bytes} />
            </>
          ) : (
            <p className="empty-text">No data available</p>
          )}
        </Card>

        {/* CPU */}
        <Card title="Processor">
          {loading && !systemInfo ? (
            <div className="skeleton-line" />
          ) : systemInfo ? (
            <>
              <p className="stat-value stat-value-lg">{systemInfo.cpu_model || 'Unknown'}</p>
              <div className="stat-row">
                <span className="stat-label">Cores</span>
                <span className="stat-value">{systemInfo.cpu_cores ?? '—'}</span>
              </div>
              <div className="stat-row">
                <span className="stat-label">Arch</span>
                <span className="stat-value">{systemInfo.architecture || '—'}</span>
              </div>
            </>
          ) : (
            <p className="empty-text">No data available</p>
          )}
        </Card>

        {/* GPU */}
        <Card title="Graphics">
          <div className="card-actions">
            <button className="btn-refresh" onClick={handleRefreshStatic} title="Refresh GPU detection">
              ↻ Refresh
            </button>
          </div>
          {loading && gpus.length === 0 ? (
            <div className="skeleton-line" />
          ) : gpus.length > 0 ? (
            <ul className="gpu-list">
              {gpus.map((gpu, idx) => (
                  <li key={idx} className="gpu-item">
                    <div className="gpu-name">{gpu.name}</div>
                    <div className="gpu-meta">
                      {gpu.is_dedicated || gpu.vram_bytes > 0 ? (
                        <span className="stat-label">{formatBytes(gpu.vram_bytes)} VRAM</span>
                      ) : (
                        <span className="stat-label">Shared with system memory</span>
                      )}
                      <StatusBadge type={gpu.is_dedicated ? 'dedicated' : 'integrated'}>
                        {gpu.is_dedicated ? 'Dedicated' : 'Integrated'}
                      </StatusBadge>
                  </div>
                </li>
              ))}
            </ul>
          ) : (
            <p className="empty-text">No GPUs detected</p>
          )}
        </Card>

        {/* Hypervisor */}
        <Card title="Hypervisor">
          <div className="card-actions">
            <button className="btn-refresh" onClick={handleRefreshStatic} title="Refresh hypervisor status">
              ↻ Refresh
            </button>
          </div>
          {loading && hypervisor === null ? (
            <div className="skeleton-line" />
          ) : hypervisor !== null ? (
            <div className="hypervisor-row">
              <span className={`status-dot ${hypervisor.is_active ? 'status-dot-on' : 'status-dot-off'}`} />
              <span className="stat-value">{hypervisor.is_active ? 'Active' : 'Inactive'}</span>
              <StatusBadge type={hypervisor.is_active ? 'success' : 'muted'}>
                {hypervisor.is_active ? 'Enabled' : 'Disabled'}
              </StatusBadge>
            </div>
          ) : (
            <p className="empty-text">No data available</p>
          )}
        </Card>
      </div>
    </div>
  );
}
