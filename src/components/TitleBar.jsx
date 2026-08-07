import { windowMinimize, windowMaximize, windowClose } from '../lib/api';

export default function TitleBar() {
  return (
    <div className="titlebar">
      <div className="titlebar-drag-region">
        <div className="titlebar-brand">
          <svg className="titlebar-logo" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <rect x="5" y="2" width="14" height="20" rx="2" ry="2" />
            <line x1="12" y1="18" x2="12" y2="18" />
          </svg>
          <span className="titlebar-name">R.S EXE</span>
        </div>
      </div>
      <div className="titlebar-controls">
        <button className="titlebar-btn" onClick={windowMinimize} aria-label="Minimize" title="Minimize">
          <svg width="12" height="12" viewBox="0 0 12 12">
            <rect y="5" width="12" height="2" fill="currentColor" />
          </svg>
        </button>
        <button className="titlebar-btn" onClick={windowMaximize} aria-label="Maximize" title="Maximize / Restore">
          <svg width="12" height="12" viewBox="0 0 12 12">
            <rect x="1" y="1" width="10" height="10" fill="none" stroke="currentColor" strokeWidth="1.5" />
          </svg>
        </button>
        <button className="titlebar-btn titlebar-btn-close" onClick={windowClose} aria-label="Close" title="Close">
          <svg width="12" height="12" viewBox="0 0 12 12">
            <line x1="2" y1="2" x2="10" y2="10" stroke="currentColor" strokeWidth="1.5" />
            <line x1="10" y1="2" x2="2" y2="10" stroke="currentColor" strokeWidth="1.5" />
          </svg>
        </button>
      </div>
    </div>
  );
}
