import { useTranslation } from "react-i18next";
import { cn } from "@/shared/lib/utils";
import DeviceLiveView from "@/features/connections/components/connections/DeviceLiveView";
import DeviceSnapshotView from "@/features/connections/components/connections/DeviceSnapshotView";
import type { DeviceChannel } from "./api";
import type { GridLayout, LiveViewSource } from "./liveGridSettings";

/** Grid cells run a reduced stream shape so a 4×4 wall stays within the
 * backend's transcode budget (single view uses the defaults). */
const GRID_STREAM_FPS = 3;
const GRID_STREAM_MAX_WIDTH = 640;
/** Stills mode refreshes grid cells on a slow cadence. */
const GRID_STILLS_INTERVAL_MS = 5000;
const GRID_CELL_KEYS = Array.from({ length: 16 }, (_, index) => `grid-cell-${index + 1}`);

/**
 * A configurable N×N wall of live channel views for one recorder.
 *
 * Every cell carries its own channel picker (top-left) and a click on the
 * picture promotes that channel to the single-channel view — the same
 * "glance at the wall, focus one camera" flow every vendor NVR UI has.
 */
function CctvLiveGrid({
  targetId,
  layout,
  cells,
  source,
  channels,
  onCellChannelChange,
  onFocusChannel,
  className,
  active = true,
}: Readonly<{
  targetId: string;
  layout: GridLayout;
  cells: readonly number[];
  source: LiveViewSource;
  /** Known channels (for the per-cell picker); may be empty. */
  channels: readonly DeviceChannel[];
  onCellChannelChange: (cellIndex: number, channel: number) => void;
  onFocusChannel: (channel: number) => void;
  className?: string;
  /** Host-surface active flag — pauses every cell's stream/poll when the tab is
   * backgrounded (same-window) so it stops starving the connection pool. */
  active?: boolean;
}>) {
  const { t } = useTranslation();
  const known = channels.length > 0 ? channels : null;

  return (
    <div
      className={cn(
        // `items-start`: cells follow each camera's own aspect ratio, so a
        // portrait channel next to a landscape one is not stretched to the row's
        // tallest cell (which would distort it).
        "grid items-start gap-1.5",
        layout === 2 && "grid-cols-2",
        layout === 3 && "grid-cols-3",
        layout === 4 && "grid-cols-4",
        className,
      )}
    >
      {cells.map((channel, cellIndex) => {
        const picker = (
          <select
            value={channel}
            aria-label={t("connections.video.gridCellChannel", "Cell channel")}
            onClick={(event) => event.stopPropagation()}
            onChange={(event) => onCellChannelChange(cellIndex, Number(event.target.value))}
            className="absolute left-1 top-1 z-10 h-9 rounded-md border border-border bg-background px-2 text-sm text-foreground"
          >
            {(known ?? cells.map((value) => ({ index: value, name: null }))).map((option) => (
              <option key={option.index} value={option.index}>
                {option.name?.trim()
                  ? `${option.index} · ${option.name.trim()}`
                  : `${t("deviceDashboard.channelShort", "Ch")} ${option.index}`}
              </option>
            ))}
          </select>
        );
        return (
          // A plain (non-interactive) container, NOT a `<button>`: the picker
          // `<select>` and, inside `DeviceLiveView`, a Reconnect `<button>` are
          // both real interactive descendants, and nesting them inside another
          // button is invalid HTML and unreachable/ambiguous for a screen reader
          // (nested-interactive elements). The "click the cell to zoom" affordance
          // is instead a SIBLING full-cover button (`z-0`, below the picker's own
          // `z-10`) — clicking anywhere on the picture reaches it (both video
          // views render their `<img>` as `pointer-events-none` for exactly this),
          // while clicking the picker or Reconnect still reaches THEM first.
          <div key={GRID_CELL_KEYS[cellIndex]} className="relative">
            <button
              type="button"
              className="absolute inset-0 z-0 cursor-zoom-in"
              title={t("connections.video.gridFocusHint", "Open this channel full-size")}
              aria-label={t("connections.video.gridFocusHint", "Open this channel full-size")}
              onClick={() => onFocusChannel(channel)}
            />
            {source === "stream" ? (
              <DeviceLiveView
                // Remounts when THIS cell's channel changes (the per-cell
                // picker), so a previous channel's error/retry/aspect state
                // never leaks into the newly picked one. See the single-view
                // `CameraControl` for the identical reasoning.
                key={channel}
                targetId={targetId}
                channel={channel}
                fps={GRID_STREAM_FPS}
                maxWidth={GRID_STREAM_MAX_WIDTH}
                overlay={picker}
                active={active}
              />
            ) : (
              <div className="relative">
                <DeviceSnapshotView
                  key={channel}
                  targetId={targetId}
                  channel={channel}
                  intervalMs={GRID_STILLS_INTERVAL_MS}
                  autoRefreshDefault
                  hideControls
                  active={active}
                />
                {picker}
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}

export default CctvLiveGrid;
