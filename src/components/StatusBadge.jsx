export default function StatusBadge({ type, children }) {
  return <span className={`status-badge status-badge-${type}`}>{children}</span>;
}
