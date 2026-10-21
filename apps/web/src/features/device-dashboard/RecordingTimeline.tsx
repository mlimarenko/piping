import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { ChevronLeft, ChevronRight } from "lucide-react";

import { Button } from "@/shared/ui-primitives/button";
import { Skeleton } from "@/shared/ui-primitives/skeleton";
import { searchDeviceRecordings } from "@/features/device-dashboard/api";
import {
  dayBounds,
  segmentsForDay,
  windowFromSpan,
  type ArchiveWindow,
  type TimelineSegment,
} from "@/features/device-dashboard/archiveWindow";

const HOUR_MARKS = [0, 6, 12, 18, 24] as const;
/**
 * Floor on a segment's rendered width, in percent of the day. A one-minute span
 * is 0.07% of a day — below a pixel — so without a floor real footage would be
 * invisible and unclickable.
 */
const MIN_SEGMENT_WIDTH_PERCENT = 0.4;

interface RecordingTimelineProps {
  targetId: string;
  channel: number;
  /** The operator's currently requested window (start/end inputs), for the marker overlay. */
  selected: ArchiveWindow | null;
  /** Called with the clicked span's window (already capped to the export/playback limit). */
  onSelectSpan: (window: ArchiveWindow) => void;
}

/**
 * A one-day, per-channel recording timeline backed by `search_recordings`.
 *
 * Unlike the device-wide `/recording` aggregate, this is scoped to exactly the
 * channel the operator is watching, so switching cameras shows that camera's
 * actual footage rather than a fleet-wide union. The window is a fixed
 * calendar day — day navigation instead of a single 30-day bar keeps segments
 * wide enough to see and click.
 */
function RecordingTimeline({
  targetId,
  channel,
  selected,
  onSelectSpan,
}: Readonly<RecordingTimelineProps>) {
  const { t } = useTranslation();
  const [dayAnchorMs, setDayAnchorMs] = useState(() => Date.now());
  const { startMs: dayStartMs, endMs: dayEndMs } = useMemo(
    () => dayBounds(dayAnchorMs),
    [dayAnchorMs],
  );

  const query = useQuery({
    queryKey: [
      "device",
      targetId,
      "recordings",
      channel,
      new Date(dayStartMs).toISOString(),
    ],
    queryFn: () =>
      searchDeviceRecordings(targetId, {
        channel,
        startIso: new Date(dayStartMs).toISOString(),
        endIso: new Date(dayEndMs).toISOString(),
      }),
    retry: false,
  });

  const segments = useMemo(
    () => segmentsForDay(query.data?.spans ?? [], dayStartMs, dayEndMs),
    [query.data, dayStartMs, dayEndMs],
  );

  const selectedRange = useMemo(() => {
    if (!selected) return null;
    const startFraction = (selected.startMs - dayStartMs) / (dayEndMs - dayStartMs);
    const endFraction = (selected.endMs - dayStartMs) / (dayEndMs - dayStartMs);
    if (endFraction <= 0 || startFraction >= 1) return null;
    return {
      startFraction: Math.max(startFraction, 0),
      endFraction: Math.min(endFraction, 1),
    };
  }, [selected, dayStartMs, dayEndMs]);

  const isToday = dayEndMs > Date.now();

  return (
    <div className="space-y-1.5">
      <div className="flex items-center justify-between">
        <Button
          type="button"
          size="sm"
          variant="ghost"
          className="h-9 gap-1 px-2 text-sm"
          onClick={() => setDayAnchorMs(dayStartMs - 1)}
        >
          <ChevronLeft className="h-3 w-3" />
          {t("deviceDashboard.timelinePrevDay", "Prev day")}
        </Button>
        <span className="text-sm font-medium text-muted-foreground">
          {new Date(dayStartMs).toLocaleDateString(undefined, {
            weekday: "short",
            month: "short",
            day: "numeric",
          })}
        </span>
        <Button
          type="button"
          size="sm"
          variant="ghost"
          className="h-9 gap-1 px-2 text-sm"
          disabled={isToday}
          onClick={() => setDayAnchorMs(dayEndMs + 1)}
        >
          {t("deviceDashboard.timelineNextDay", "Next day")}
          <ChevronRight className="h-3 w-3" />
        </Button>
      </div>

      <TimelineBar
        isPending={query.isPending}
        isError={query.isError}
        segments={segments}
        selectedRange={selectedRange}
        onSelectSpan={onSelectSpan}
      />
      <div className="flex justify-between text-xs text-muted-foreground">
        {HOUR_MARKS.map((hour) => (
          <span key={hour}>{String(hour).padStart(2, "0")}:00</span>
        ))}
      </div>
    </div>
  );
}

/** Where the operator's requested window sits on the day, as `0..1` fractions. */
interface SelectedRange {
  startFraction: number;
  endFraction: number;
}

interface TimelineBarProps {
  isPending: boolean;
  isError: boolean;
  segments: readonly TimelineSegment[];
  selectedRange: SelectedRange | null;
  onSelectSpan: (window: ArchiveWindow) => void;
}

/** Percent width for a fraction span, floored so a very short one stays visible. */
function barWidth(startFraction: number, endFraction: number): string {
  return `${Math.max((endFraction - startFraction) * 100, MIN_SEGMENT_WIDTH_PERCENT)}%`;
}

/**
 * The day's footage bar: one clickable block per recorded span, with the
 * requested window outlined over them.
 *
 * A `<fieldset>` rather than a div with `role="group"`: this is a real group of
 * controls, so the native element carries the grouping without an ARIA
 * override, and the legend names it for screen readers.
 */
function TimelineBar({
  isPending,
  isError,
  segments,
  selectedRange,
  onSelectSpan,
}: Readonly<TimelineBarProps>) {
  const { t } = useTranslation();

  if (isPending) {
    return <Skeleton className="h-9 w-full rounded" />;
  }

  if (isError) {
    return (
      <p className="text-xs text-muted-foreground">
        {t(
          "deviceDashboard.timelineUnavailable",
          "Recording timeline unavailable for this channel.",
        )}
      </p>
    );
  }

  return (
    <fieldset className="relative h-9 w-full overflow-hidden rounded border border-border bg-muted">
      <legend className="sr-only">
        {t("deviceDashboard.timelineForDay", "Recorded footage for the selected day")}
      </legend>
      {segments.map((segment) => (
        <button
          key={`${segment.span.start}-${segment.span.end}`}
          type="button"
          className="absolute inset-y-0 bg-primary transition-colors hover:bg-primary/90"
          style={{
            left: `${segment.startFraction * 100}%`,
            width: barWidth(segment.startFraction, segment.endFraction),
          }}
          title={`${new Date(segment.span.start).toLocaleTimeString()} → ${new Date(segment.span.end).toLocaleTimeString()}`}
          onClick={() => onSelectSpan(windowFromSpan(segment.span))}
        />
      ))}
      {selectedRange && (
        <div
          className="pointer-events-none absolute inset-y-0 border-x-2 border-status-warning"
          style={{
            left: `${selectedRange.startFraction * 100}%`,
            width: barWidth(selectedRange.startFraction, selectedRange.endFraction),
          }}
        />
      )}
      {segments.length === 0 && (
        <div className="flex h-full items-center justify-center text-xs text-muted-foreground">
          {t("deviceDashboard.timelineNoFootage", "No recorded footage this day")}
        </div>
      )}
    </fieldset>
  );
}

export default RecordingTimeline;
