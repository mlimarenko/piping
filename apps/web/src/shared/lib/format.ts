/** Shared formatting utilities */

/**
 * Format an ISO timestamp as a human-readable relative time string.
 * Returns "—" when the input is null/undefined/empty.
 */
export function formatRelativeTime(iso: string | null | undefined): string {
  if (!iso) return "—";
  const diff = Date.now() - new Date(iso).getTime();
  if (diff < 60_000) return "just now";
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)}m ago`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)}h ago`;
  return `${Math.floor(diff / 86_400_000)}d ago`;
}

/**
 * Return a Tailwind class string for an HTTP status code badge.
 * Returns the neutral surface-chip class when status is absent.
 */
export function httpStatusColor(status: number | null | undefined): string {
  if (!status) return "surface-chip";
  if (status >= 200 && status < 300)
    return "bg-status-positive/10 text-status-positive border border-status-positive/15";
  if (status >= 400 && status < 500)
    return "bg-status-warning/10 text-status-warning border border-status-warning/15";
  return "bg-status-negative/10 text-status-negative border border-status-negative/15";
}
