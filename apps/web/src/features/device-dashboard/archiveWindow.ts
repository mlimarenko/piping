/**
 * Time-window arithmetic shared by the archive player and the export dialog.
 *
 * Kept free of React and of the API client so the rules an operator relies on —
 * what counts as a valid window, which retained footage a request lands in — are
 * testable on their own.
 */

import type { TFunction } from "i18next";

/** Backend default for `PIPING_CCTV__MAX_EXPORT_WINDOW_SECS`. */
export const MAX_WINDOW_SECONDS = 2 * 60 * 60;

/** Quick-pick window lengths, in seconds. */
export const WINDOW_PRESETS = [60, 5 * 60, 15 * 60, 30 * 60] as const;

/** One contiguous stretch of recorded footage, as the device reports it. */
export interface RecordedSpan {
  start: string;
  end: string;
}

/** A resolved archive window, in epoch milliseconds. */
export interface ArchiveWindow {
  startMs: number;
  endMs: number;
}

/** Why a requested window cannot be played or exported. */
export type WindowProblem = "invalid" | "inverted" | "too-long";

/**
 * Validate a window the operator picked.
 *
 * Returns `null` when the window is usable. The checks mirror the backend's own
 * so an impossible request is refused before it costs a media slot.
 */
export function windowProblem(startMs: number, endMs: number): WindowProblem | null {
  if (!Number.isFinite(startMs) || !Number.isFinite(endMs)) return "invalid";
  if (endMs <= startMs) return "inverted";
  if ((endMs - startMs) / 1000 > MAX_WINDOW_SECONDS) return "too-long";
  return null;
}

/**
 * Clamp a window into the retained footage, when the device reports its extent.
 *
 * Playing outside the retained range yields an empty stream and an opaque
 * failure; clamping turns that into the nearest window that can actually
 * contain footage. Bounds the device does not report are left untouched.
 */
export function clampToRetention(
  window: ArchiveWindow,
  retention: Readonly<{ oldest: string | null; newest: string | null }>,
): ArchiveWindow {
  const oldestMs = retention.oldest ? Date.parse(retention.oldest) : Number.NaN;
  const newestMs = retention.newest ? Date.parse(retention.newest) : Number.NaN;
  let { startMs, endMs } = window;
  if (Number.isFinite(oldestMs) && startMs < oldestMs) {
    const span = endMs - startMs;
    startMs = oldestMs;
    endMs = startMs + span;
  }
  if (Number.isFinite(newestMs) && endMs > newestMs) {
    const span = endMs - startMs;
    endMs = newestMs;
    // `Math.max` would keep a start that is now AFTER the clamped end — the case
    // where the whole request sits past the newest footage — producing an
    // inverted window that reads as valid. Anchor the start to the span instead.
    startMs = endMs - span;
  }
  return { startMs, endMs };
}

/**
 * Whether any recorded span overlaps the window.
 *
 * Used to warn before a request the device is certain to answer emptily. An
 * empty span list means the device did not enumerate them, which is not
 * evidence of absence — so it counts as "may contain footage".
 */
export function windowHasFootage(window: ArchiveWindow, spans: readonly RecordedSpan[]): boolean {
  if (spans.length === 0) return true;
  return spans.some((span) => {
    const spanStart = Date.parse(span.start);
    const spanEnd = Date.parse(span.end);
    if (!Number.isFinite(spanStart) || !Number.isFinite(spanEnd)) return true;
    return spanStart < window.endMs && spanEnd > window.startMs;
  });
}

/** The window a span represents, capped to the maximum request length. */
export function windowFromSpan(span: RecordedSpan): ArchiveWindow {
  const startMs = Date.parse(span.start);
  const spanEndMs = Date.parse(span.end);
  const endMs = Math.min(spanEndMs, startMs + MAX_WINDOW_SECONDS * 1000);
  return { startMs, endMs };
}

/**
 * Operator-facing explanation for a rejected window, or `null` when the window
 * is usable.
 *
 * Lives beside {@link windowProblem} rather than in the player so the rule and
 * the sentence describing it stay in one place, and so the mapping is covered
 * by the same React-free tests as the rule itself.
 */
export function windowProblemMessage(problem: WindowProblem | null, t: TFunction): string | null {
  switch (problem) {
    case "inverted":
      return t("deviceDashboard.exportEndAfterStart", "End must be after start.");
    case "too-long":
      return t("deviceDashboard.exportTooLong", {
        defaultValue: "The window is longer than {{hours}} hours; pick a shorter range.",
        hours: MAX_WINDOW_SECONDS / 3600,
      });
    case "invalid":
      return t("deviceDashboard.exportInvalidRange", "Enter a valid time range.");
    default:
      return null;
  }
}

/** Local-midnight bounds (epoch ms, `[startMs, endMs)`) for the day containing `ms`. */
export function dayBounds(ms: number): ArchiveWindow {
  const date = new Date(ms);
  const startMs = new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();
  return { startMs, endMs: startMs + 24 * 60 * 60 * 1000 };
}

/** One recorded span positioned within a day, as a `0..1` fraction of the day's length. */
export interface TimelineSegment {
  span: RecordedSpan;
  /** Where the span (clipped to the day) starts, as a fraction of the day. */
  startFraction: number;
  /** Where the span (clipped to the day) ends, as a fraction of the day. */
  endFraction: number;
}

/**
 * Position each span that overlaps `[dayStartMs, dayEndMs)` as a day-relative
 * fraction, for rendering on a fixed-width timeline bar. A span already
 * unparseable is dropped rather than surfacing a NaN-positioned bar; a span
 * that only partially overlaps the day is clipped to it.
 */
export function segmentsForDay(
  spans: readonly RecordedSpan[],
  dayStartMs: number,
  dayEndMs: number,
): TimelineSegment[] {
  const dayLength = dayEndMs - dayStartMs;
  const segments: TimelineSegment[] = [];
  for (const span of spans) {
    const spanStartMs = Date.parse(span.start);
    const spanEndMs = Date.parse(span.end);
    if (!Number.isFinite(spanStartMs) || !Number.isFinite(spanEndMs)) continue;
    const clippedStartMs = Math.max(spanStartMs, dayStartMs);
    const clippedEndMs = Math.min(spanEndMs, dayEndMs);
    if (clippedEndMs <= clippedStartMs) continue;
    segments.push({
      span,
      startFraction: (clippedStartMs - dayStartMs) / dayLength,
      endFraction: (clippedEndMs - dayStartMs) / dayLength,
    });
  }
  return segments;
}

/** `Date` → the `YYYY-MM-DDTHH:mm` a `datetime-local` input expects (local). */
export function toLocalInput(date: Date): string {
  const pad = (value: number) => String(value).padStart(2, "0");
  return (
    `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}` +
    `T${pad(date.getHours())}:${pad(date.getMinutes())}`
  );
}

/** Human duration for a window, e.g. `1 h 05 m` or `45 s`. */
export function formatDuration(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds <= 0) return "—";
  const total = Math.round(seconds);
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const secs = total % 60;
  if (hours > 0) return `${hours}:${String(minutes).padStart(2, "0")}:${String(secs).padStart(2, "0")}`;
  if (minutes > 0) return `${minutes}:${String(secs).padStart(2, "0")}`;
  return `${secs}s`;
}

/**
 * Playback URL for one window; the session cookie authenticates the request.
 *
 * Lives beside the window arithmetic rather than in the player component so the
 * component file exports only components.
 */
export function playbackUrl(
  apiBaseUrl: string,
  targetId: string,
  channel: number,
  startMs: number,
  endMs: number,
  /** Ask the server to re-encode so any browser can decode the stream. */
  transcode = false,
): string {
  const params = new URLSearchParams({
    channel: String(channel),
    start: new Date(startMs).toISOString(),
    end: new Date(endMs).toISOString(),
  });
  if (transcode) params.set("transcode", "true");
  return `${apiBaseUrl}/api/v1/cctv/${encodeURIComponent(targetId)}/playback?${params.toString()}`;
}
