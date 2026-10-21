import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Camera, Film, Grid2x2, Square, Video } from "lucide-react";
import { cn } from "@/shared/lib/utils";
import SurfaceCard from "@/shared/ui-shared/SurfaceCard";
import { Skeleton } from "@/shared/ui-primitives/skeleton";
import DeviceSnapshotView from "@/features/connections/components/connections/DeviceSnapshotView";
import DeviceLiveView from "@/features/connections/components/connections/DeviceLiveView";
import CctvLiveGrid from "./CctvLiveGrid";
import {
  defaultCells,
  loadLiveViewSettings,
  resizeCells,
  saveLiveViewSettings,
  type GridLayout,
  type LiveViewSettings,
  type LiveViewSource,
} from "./liveGridSettings";
import PtzControl from "./PtzControl";
import { getDeviceChannels, type DeviceCapabilities, type DeviceChannel } from "./api";
import { retryOn5xx } from "@/shared/api/hooks";

// A deterministic config error (no credentials / unsupported) never becomes
// readable by retrying; only genuine 5xx are worth another attempt.
/**
 * Capability-driven camera control: a channel selector over the device's live
 * view. The channel list degrades to the best signal the device offers —
 * `list_channels` (names) where the family implements it, else the `info`
 * channel count as bare numbers, else a single default view.
 *
 * The picture follows the selected channel; the whole panel is one card so it
 * reads as the "switch cameras and watch" surface the operator wants.
 */
function CameraControl({
  targetId,
  capabilities,
  infoChannelCount,
  className,
  active = true,
}: Readonly<{
  targetId: string;
  capabilities?: DeviceCapabilities;
  infoChannelCount?: number | null;
  className?: string;
  /** Host-surface active flag (false when the CCTV session tab is backgrounded
   * in the same window); threaded to the live grid/view to pause its streams. */
  active?: boolean;
}>) {
  const { t } = useTranslation();
  const [selected, setSelected] = useState<number>(1);
  // View settings (single/grid, stream/stills, grid shape) persist per target.
  const [settings, setSettings] = useState<LiveViewSettings>(() =>
    loadLiveViewSettings(targetId),
  );
  const updateSettings = (patch: Partial<LiveViewSettings>) => {
    setSettings((previous) => {
      const next = { ...previous, ...patch };
      saveLiveViewSettings(targetId, next);
      return next;
    });
  };

  const channelsQuery = useQuery({
    queryKey: ["device", targetId, "channels"],
    queryFn: () => getDeviceChannels(targetId),
    enabled: !!targetId && capabilities?.channels === true,
    retry: retryOn5xx,
  });

  const channels = useMemo(
    () => resolveChannels(channelsQuery.data?.channels, infoChannelCount),
    [channelsQuery.data, infoChannelCount],
  );

  const loadingChannels = capabilities?.channels === true && channelsQuery.isPending;
  const view = deriveViewState(capabilities, channels, settings, selected);

  return (
    // Small split panes scroll controls rather than squeezing the picture to a strip.
    <SurfaceCard className={cn("flex min-h-0 max-h-full flex-col overflow-y-auto p-4", className)}>
      <LiveViewHeader
        channelCount={channels.length}
        showChannelCount={view.showSelector}
        canStream={view.canStream}
        source={view.source}
        gridAvailable={view.gridAvailable}
        isGrid={view.isGrid}
        layout={settings.layout}
        onToggleSource={() =>
          updateSettings({ source: view.source === "stream" ? "stills" : "stream" })
        }
        onToggleMode={() => updateSettings({ mode: view.isGrid ? "single" : "grid" })}
        onSelectLayout={(layout) =>
          updateSettings({ layout, cells: resizeCells(settings.cells, layout) })
        }
      />

      {loadingChannels ? (
        <Skeleton className="mb-2 h-8 w-full" />
      ) : (
        !view.isGrid &&
        view.showSelector && (
          <ChannelSelector channels={channels} selected={selected} onSelect={setSelected} />
        )
      )}

      <LiveViewport
        targetId={targetId}
        isGrid={view.isGrid}
        source={view.source}
        layout={settings.layout}
        cells={settings.cells}
        channels={channels}
        showSelector={view.showSelector}
        selected={selected}
        active={active}
        onCellChannelChange={(cellIndex, channel) => {
          const cells = settings.cells.slice();
          cells[cellIndex] = channel;
          updateSettings({ cells });
        }}
        onFocusChannel={(channel) => {
          setSelected(channel);
          updateSettings({ mode: "single" });
        }}
      />

      {view.showPtz && (
        <div className="mt-3 border-t border-border pt-3">
          <PtzControl
            key={`${targetId}:${view.activeChannel}`}
            targetId={targetId}
            channel={view.activeChannel}
          />
        </div>
      )}
      {view.ptzEligible && view.isGrid && (
        <div className="mt-3 border-t border-border pt-3 text-xs text-muted-foreground">
          {t(
            "deviceDashboard.ptzGridHint",
            "Switch to single view to control PTZ — grid mode has no single camera to pan.",
          )}
        </div>
      )}
    </SurfaceCard>
  );
}

/**
 * Everything the panel decides per render from capabilities, the discovered
 * channels, the persisted view settings and the current selection: which
 * controls appear, which viewport variant renders and whether PTZ is usable.
 */
interface LiveViewState {
  /** More than one channel — show the selector and the channel count badge. */
  showSelector: boolean;
  /** The channel the single view (and PTZ) is bound to. */
  activeChannel: number;
  /** Device, permission and per-channel discovery all confirm PTZ. */
  ptzEligible: boolean;
  canStream: boolean;
  /** Effective source — a non-streaming device is forced to stills. */
  source: LiveViewSource;
  gridAvailable: boolean;
  isGrid: boolean;
  /** PTZ pad actually rendered (eligible AND meaningfully bound). */
  showPtz: boolean;
}

/**
 * Pure resolver for the panel's derived flags, kept out of the component so
 * the capability/permission/discovery rules read as one linear ruleset
 * instead of being interleaved with rendering.
 */
function deriveViewState(
  capabilities: DeviceCapabilities | undefined,
  channels: DeviceChannel[],
  settings: LiveViewSettings,
  selected: number,
): LiveViewState {
  const showSelector = channels.length > 1;
  const activeChannel = showSelector ? selected : 1;
  const selectedChannel = channels.find((channel) => channel.index === activeChannel);
  // Target-level PTZ means only that the driver can transport commands. Render
  // controls only after read-only discovery confirms support for this channel.
  const deviceHasPtz = capabilities?.ptz === true;
  const mayControlPtz = capabilities?.permissions.ptz === true;
  const channelHasPtz = selectedChannel?.ptz_support === "supported";
  const ptzEligible = deviceHasPtz && mayControlPtz && channelHasPtz;
  const canStream = capabilities?.streaming === true;
  const source = canStream ? settings.source : "stills";
  const gridAvailable = channels.length > 1;
  const isGrid = gridAvailable && settings.mode === "grid";
  // In grid mode `selected` is frozen at whatever the single view last showed
  // and has no relationship to any of the (possibly several, possibly none
  // matching) cells on screen — binding PTZ to it would let an operator pan a
  // camera they cannot see with no indication which one moved. Hide the pad
  // instead of showing a misleadingly-bound one; a hint explains why.
  const showPtz = ptzEligible && !isGrid;
  return {
    showSelector,
    activeChannel,
    ptzEligible,
    canStream,
    source,
    gridAvailable,
    isGrid,
    showPtz,
  };
}

/**
 * Header row of the panel: title plus the view controls (stream/stills
 * toggle, single/grid toggle, grid size). Presentational — the parent owns
 * the settings, so this stays a plain mapping from flags to controls.
 */
function LiveViewHeader({
  channelCount,
  showChannelCount,
  canStream,
  source,
  gridAvailable,
  isGrid,
  layout,
  onToggleSource,
  onToggleMode,
  onSelectLayout,
}: Readonly<{
  channelCount: number;
  showChannelCount: boolean;
  canStream: boolean;
  source: LiveViewSource;
  gridAvailable: boolean;
  isGrid: boolean;
  layout: GridLayout;
  onToggleSource: () => void;
  onToggleMode: () => void;
  onSelectLayout: (layout: GridLayout) => void;
}>) {
  const { t } = useTranslation();
  return (
    <div className="mb-3 flex flex-wrap items-center gap-2">
      <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-md bg-accent text-primary">
        <Video className="h-4 w-4" />
      </div>
      <h2 className="text-sm font-semibold text-foreground">
        {t("deviceDashboard.liveView", "Live view")}
      </h2>
      <div className="ml-auto flex min-w-0 flex-wrap items-center gap-1.5">
        {showChannelCount && (
          <span className="mr-1 text-xs text-muted-foreground">
            {t("deviceDashboard.channelCount", "{{count}} channels", { count: channelCount })}
          </span>
        )}
        {canStream && (
          <ViewToggle
            active={source === "stream"}
            onClick={onToggleSource}
            icon={source === "stream" ? <Film className="h-3 w-3" /> : <Camera className="h-3 w-3" />}
            label={
              source === "stream"
                ? t("connections.video.sourceStream", "Stream")
                : t("connections.video.sourceStills", "Stills")
            }
            title={t("connections.video.sourceToggle", "Switch between live stream and stills")}
          />
        )}
        {gridAvailable && (
          <ViewToggle
            active={isGrid}
            onClick={onToggleMode}
            icon={isGrid ? <Grid2x2 className="h-3 w-3" /> : <Square className="h-3 w-3" />}
            label={
              isGrid
                ? t("connections.video.viewGrid", "Grid")
                : t("connections.video.viewSingle", "Single")
            }
            title={t("connections.video.viewToggle", "Switch between single channel and grid")}
          />
        )}
        {isGrid && (
          <select
            value={layout}
            aria-label={t("connections.video.gridLayout", "Grid size")}
            onChange={(event) => onSelectLayout(Number(event.target.value) as GridLayout)}
            className="h-8 rounded-md border border-border bg-background px-2 text-sm text-foreground"
          >
            <option value={2}>2×2</option>
            <option value={3}>3×3</option>
            <option value={4}>4×4</option>
          </select>
        )}
      </div>
    </div>
  );
}

/**
 * The picture area of the panel — exactly one of the three mutually exclusive
 * variants (grid of cells, single live stream, single stills view), so the
 * either/or/or choice lives here as guard clauses instead of nested ternaries
 * in the parent's JSX.
 */
function LiveViewport({
  targetId,
  isGrid,
  source,
  layout,
  cells,
  channels,
  showSelector,
  selected,
  active,
  onCellChannelChange,
  onFocusChannel,
}: Readonly<{
  targetId: string;
  isGrid: boolean;
  source: LiveViewSource;
  layout: GridLayout;
  cells: readonly number[];
  channels: DeviceChannel[];
  showSelector: boolean;
  selected: number;
  active: boolean;
  onCellChannelChange: (cellIndex: number, channel: number) => void;
  onFocusChannel: (channel: number) => void;
}>) {
  if (isGrid) {
    return (
      <CctvLiveGrid
        targetId={targetId}
        layout={layout}
        cells={cells.length ? cells : defaultCells(layout)}
        source={source}
        channels={channels}
        onCellChannelChange={onCellChannelChange}
        onFocusChannel={onFocusChannel}
        active={active}
      />
    );
  }
  // Remounts on a channel switch (React's own prescribed pattern for
  // "reset state when a prop changes"): without this, a previous
  // channel's error overlay/aspect-ratio/one-shot-retry state leaks
  // into the newly selected channel until its own load/error fires —
  // and a spent retry from channel A silently means channel B gets
  // zero auto-retries of its own.
  const viewKey = showSelector ? selected : "default";
  const channel = showSelector ? selected : undefined;
  if (source === "stream") {
    return (
      <DeviceLiveView
        key={viewKey}
        targetId={targetId}
        channel={channel}
        className="min-h-40 shrink-0"
        active={active}
      />
    );
  }
  return (
    <DeviceSnapshotView
      key={viewKey}
      targetId={targetId}
      channel={channel}
      className="min-h-40 shrink-0"
      active={active}
    />
  );
}

/** Small pill toggle used in the live-view header controls. */
function ViewToggle({
  active,
  onClick,
  icon,
  label,
  title,
}: Readonly<{
  active: boolean;
  onClick: () => void;
  icon: React.ReactNode;
  label: string;
  title: string;
}>) {
  return (
    <button
      type="button"
      title={title}
      aria-pressed={active}
      onClick={onClick}
      className={cn(
        "flex h-8 items-center gap-1 rounded-md border px-2 text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
        active
          ? "border-primary/50 bg-accent text-primary"
          : "border-input/30 bg-background text-muted-foreground hover:border-input/60 hover:text-foreground",
      )}
    >
      {icon}
      {label}
    </button>
  );
}

/**
 * Horizontal, scrollable row of channel chips. The number is the device-native
 * channel; the name (when the device reports one) is the accessible label and
 * tooltip so a matrix of cameras stays scannable.
 */
function ChannelSelector({
  channels,
  selected,
  onSelect,
}: Readonly<{
  channels: DeviceChannel[];
  selected: number;
  onSelect: (index: number) => void;
}>) {
  const { t } = useTranslation();
  return (
    <div
      className="mb-3 flex gap-1.5 overflow-x-auto stable-scrollbar-gutter pb-1"
      role="tablist"
      aria-label={t("deviceDashboard.channels", "Channels")}
    >
      {channels.map((channel) => {
        const isActive = channel.index === selected;
        const label = channel.name?.trim()
          ? `${channel.index} · ${channel.name.trim()}`
          : `${t("deviceDashboard.channelShort", "Ch")} ${channel.index}`;
        return (
          <button
            key={channel.index}
            type="button"
            role="tab"
            aria-selected={isActive}
            title={channel.name?.trim() || label}
            onClick={() => onSelect(channel.index)}
            className={cn(
              "h-8 shrink-0 rounded-md border px-2.5 text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
              isActive
                ? "border-primary/50 bg-accent text-primary"
                : "border-input/30 bg-background text-muted-foreground hover:text-foreground hover:border-input/60",
            )}
          >
            {label}
          </button>
        );
      })}
    </div>
  );
}

/**
 * The channel list to render, most-informative first: the device's own channel
 * list (with names), else a synthesized `1..count` from the info readout, else
 * empty (the caller shows the single default view).
 */
function resolveChannels(
  reported: DeviceChannel[] | undefined,
  infoCount: number | null | undefined,
): DeviceChannel[] {
  if (reported && reported.length > 0) return reported;
  if (infoCount && infoCount > 1) {
    return Array.from({ length: infoCount }, (_unused, i) => ({
      index: i + 1,
      name: null,
      enabled: null,
      ptz_support: "unknown" as const,
      recording: null,
    }));
  }
  return [];
}

export default CameraControl;
