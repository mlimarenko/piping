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

/** Format a timestamp using the application's selected language. */
export function formatAbsoluteTime(iso: string | null | undefined, locale: string): string {
  if (!iso) return "—";
  const time = new Date(iso);
  return Number.isFinite(time.getTime()) ? new Intl.DateTimeFormat(locale, { dateStyle: "short", timeStyle: "medium" }).format(time) : "—";
}

/** Format a finite duration with at most two localized units. */
export function formatDuration(ms: number, locale: string): string {
  if (!Number.isFinite(ms)) return "—";
  const seconds = Math.floor(Math.max(0, ms) / 1000);
  const minutes = Math.floor(seconds / 60);
  const hours = Math.floor(minutes / 60);
  const unit = (value: number, unit: "hour" | "minute" | "second") =>
    new Intl.NumberFormat(locale, { style: "unit", unit, unitDisplay: "short" }).format(value);
  if (hours > 0) return `${unit(hours, "hour")} ${unit(minutes % 60, "minute")}`;
  if (minutes > 0) return `${unit(minutes, "minute")} ${unit(seconds % 60, "second")}`;
  return unit(seconds, "second");
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
