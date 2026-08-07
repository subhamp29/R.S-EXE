import { Component } from 'react';

// Top-level safety net. Catches render-phase errors (and class-component
// lifecycle/unmount errors) anywhere in the tree below it so the user lands on a
// recoverable screen with a Reload button instead of a silent black screen.
//
// NOTE: React error boundaries do NOT catch errors thrown from `useEffect`
// cleanup (unmount) callbacks — those are exactly what crashed <Devices> here and
// is why the real fix lives in useEmulatorControl.js. This boundary is a
// defense-in-depth net for *future* render-time regressions.
export default class ErrorBoundary extends Component {
  constructor(props) {
    super(props);
    this.state = { hasError: false, error: null };
  }

  static getDerivedStateFromError(error) {
    return { hasError: true, error };
  }

  componentDidCatch(error, errorInfo) {
    // Log full details to the DevTools console for diagnosis.
    console.error('[ErrorBoundary] Uncaught error in app tree:', error, errorInfo);
  }

  handleReload = () => {
    // Reload the webview (served from the dev server in `tauri dev`, or the
    // bundled dist in production). Restarts React + effects cleanly.
    window.location.reload();
  };

  render() {
    if (this.state.hasError) {
      return (
        <div className="error-boundary">
          <div className="error-boundary__card">
            <div className="error-boundary__icon" role="img" aria-label="Error">
              <svg width="32" height="32" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <circle cx="12" cy="12" r="10" />
                <line x1="12" y1="8" x2="12" y2="12" />
                <line x1="12" y1="16" x2="12.01" y2="16" />
              </svg>
            </div>
            <h1 className="dashboard-heading">Something went wrong</h1>
            <p className="error-boundary__text">
              R.S EXE hit an unexpected error while rendering. Reload the application to continue.
            </p>
            <button className="btn-accent" onClick={this.handleReload}>
              Reload application
            </button>
          </div>
        </div>
      );
    }
    return this.props.children;
  }
}
