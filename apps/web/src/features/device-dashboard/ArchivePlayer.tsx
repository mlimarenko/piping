import { useMemo, useRef, useState, type RefObject } from "react";
import { useTranslation } from "react-i18next";
import { AlertTriangle, Download, Play } from "lucide-react";

import { API_BASE_URL } from "@/shared/api/client";
import { cn } from "@/shared/lib/utils";
import { Badge } from "@/shared/ui-primitives/badge";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import FieldLabel from "@/shared/ui-shared/FieldLabel";
import type { DeviceChannel } from "@/features/device-dashboard/api";
import {
  clampToRetention,
  formatDuration,
  playbackUrl,
  toLocalInput,
  windowHasFootage,
  windowProblem,
  windowProblemMessage,
  WINDOW_PRESETS,
  type ArchiveWindow,
  type RecordedSpan,
} from "@/features/device-dashboard/archiveWindow";
import RecordingTimeline from "@/features/device-dashboard/RecordingTimeline";

interface ArchivePlayerProps {
  targetId: string;
  /** Channels to choose from; falls back to a single unnamed channel. */
  channels: readonly DeviceChannel[];
  /** Retained footage bounds, when the device reports them. */
  retention: Readonly<{ oldest: string | null; newest: string | null }>;
  /** Recorded spans, used to warn about windows with no footage. */
  spans: readonly RecordedSpan[];
  /** Whether the caller may also download the selected window. */
  canExport: boolean;
  /** Called with the current window when the operator asks to export it. */
  onExport?: (channel: number, startMs: number, endMs: number) => void;
  className?: string;
  /** False when the surrounding tab is hidden, so the video releases its slot. */
  active?: boolean;
}

/** One archive window handed to the `<video>` element. */
interface PlayingWindow {
  /**
   * Pinned when playback starts: reading live state would make changing the
   * camera dropdown silently re-request the archive as a render side effect.
   */
  channel: number;
  startMs: number;
  endMs: number;
  /** Set after the browser failed to decode the faithful copy. */
  transcode: boolean;
}

/**
 * Watch recorded footage for a chosen channel and period.
 *
 * The backend streams a fragmented MP4 that plays progressively as it arrives,
 * so this is a plain `<video>` rather than a second media pipeline. Seeking
 * within the loaded window is native; moving outside it means requesting a new
 * window, which is what the controls here do.
 *
 * Nothing is requested until the operator presses play: an archive request costs
 * a device session, so it must never be a side effect of opening a tab.
 */
function ArchivePlayer({
  targetId,
  channels,
  retention,
  spans,
  canExport,
  onExport,
  className,
  active = true,
}: Readonly<ArchivePlayerProps>) {
  const { t } = useTranslation();
  const videoRef = useRef<HTMLVideoElement>(null);

  const [channel, setChannel] = useState(() => channels[0]?.index ?? 1);
  const [startInput, setStartInput] = useState(() => {
    const end = retention.newest ? new Date(retention.newest) : new Date();
    return toLocalInput(new Date(end.getTime() - 60_000));
  });
  const [endInput, setEndInput] = useState(() =>
    toLocalInput(retention.newest ? new Date(retention.newest) : new Date()),
  );
  /** The window currently being played; `null` until the operator starts one. */
  const [playing, setPlaying] = useState<PlayingWindow | null>(null);
  const [failed, setFailed] = useState(false);
  // A hidden tab must not keep a device session open. Derived rather than
  // cleared from an effect, so going inactive drops the <video> in the same
  // render instead of after a cascading one.
  const visible = active ? playing : null;

  const requested = useMemo(() => {
    const startMs = new Date(startInput).getTime();
    const endMs = new Date(endInput).getTime();
    return { startMs, endMs };
  }, [startInput, endInput]);
  const problem = windowProblem(requested.startMs, requested.endMs);
  const hasFootage = problem ? true : windowHasFootage(requested, spans);
  const durationSeconds = (requested.endMs - requested.startMs) / 1000;

  const selectWindow = (window: ArchiveWindow) => {
    setStartInput(toLocalInput(new Date(window.startMs)));
    setEndInput(toLocalInput(new Date(window.endMs)));
  };

  const applyPreset = (seconds: number) => {
    const endMs = Number.isFinite(requested.endMs) ? requested.endMs : Date.now();
    const next = clampToRetention({ startMs: endMs - seconds * 1000, endMs }, retention);
    setStartInput(toLocalInput(new Date(next.startMs)));
    setEndInput(toLocalInput(new Date(next.endMs)));
  };

  const startPlayback = () => {
    if (problem) return;
    const window = clampToRetention(requested, retention);
    setFailed(false);
    setPlaying({ channel, startMs: window.startMs, endMs: window.endMs, transcode: false });
  };

  /**
   * The browser could not decode the stream.
   *
   * These recorders commonly write H.265, which most browsers refuse, so the
   * first failure is not a dead end: retry once asking the server to re-encode.
   * Transcoding costs real CPU, which is why it is never the first attempt. A
   * second failure is a genuine one and surfaces the export escape hatch.
   */
  const handleDecodeFailure = () => {
    if (visible && !visible.transcode) {
      setPlaying({ ...visible, transcode: true });
      return;
    }
    setFailed(true);
  };

  const problemMessage = windowProblemMessage(problem, t);

  return (
    <div className={cn("space-y-4", className)}>
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-4">
        {channels.length > 1 && (
          <div className="space-y-1">
            <FieldLabel htmlFor="archive-channel">
              {t("deviceDashboard.exportChannel", "Camera")}
            </FieldLabel>
            <select
              id="archive-channel"
              className="h-9 w-full rounded-md border border-border bg-background px-2 text-sm"
              value={channel}
              onChange={(event) => setChannel(Number(event.target.value))}
            >
              {channels.map((entry) => (
                <option key={entry.index} value={entry.index}>
                  {entry.name?.trim() ||
                    `${t("deviceDashboard.channelShort", "Ch")} ${entry.index}`}
                </option>
              ))}
            </select>
          </div>
        )}
        <div className="space-y-1">
          <FieldLabel htmlFor="archive-start">
            {t("deviceDashboard.exportFrom", "From")}
          </FieldLabel>
          <Input
            id="archive-start"
            type="datetime-local"
            className="h-9 text-sm"
            value={startInput}
            onChange={(event) => setStartInput(event.target.value)}
          />
        </div>
        <div className="space-y-1">
          <FieldLabel htmlFor="archive-end">
            {t("deviceDashboard.exportTo", "To")}
          </FieldLabel>
          <Input
            id="archive-end"
            type="datetime-local"
            className="h-9 text-sm"
            value={endInput}
            onChange={(event) => setEndInput(event.target.value)}
          />
        </div>
        <div className="flex flex-wrap items-end gap-2">
          <Button
            type="button"
            size="sm"
            className="gap-1.5"
            disabled={problem !== null}
            onClick={startPlayback}
          >
            <Play className="h-4 w-4" />
            {t("deviceDashboard.watch", "Watch")}
          </Button>
          {canExport && (
            <Button
              type="button"
              size="sm"
              variant="outline"
              className="gap-1.5"
              disabled={problem !== null}
              onClick={() => onExport?.(channel, requested.startMs, requested.endMs)}
            >
              <Download className="h-4 w-4" />
              {t("deviceDashboard.exportSelected", "Export selected range")}
            </Button>
          )}
        </div>
      </div>

      <div className="flex flex-wrap items-center gap-2">
        <span className="text-xs text-muted-foreground">
          {t("deviceDashboard.quickWindow", "Quick window")}
        </span>
        {WINDOW_PRESETS.map((seconds) => (
          <Button
            key={seconds}
            type="button"
            size="sm"
            variant="outline"
            className="h-8 px-2 text-sm"
            onClick={() => applyPreset(seconds)}
          >
            {t("deviceDashboard.lastMinutes", {
              defaultValue: "Last {{count}} min",
              count: Math.round(seconds / 60),
            })}
          </Button>
        ))}
        {problem === null && (
          <Badge variant="status-neutral" className="ml-auto text-xs">
            {formatDuration(durationSeconds)}
          </Badge>
        )}
      </div>

      <RecordingTimeline
        targetId={targetId}
        channel={channel}
        selected={problem ? null : requested}
        onSelectSpan={selectWindow}
      />

      {problemMessage && (
        <p role="alert" className="text-xs text-destructive">
          {problemMessage}
        </p>
      )}
      {!problemMessage && !hasFootage && (
        <p className="flex items-center gap-1.5 text-xs text-muted-foreground">
          <AlertTriangle className="h-3.5 w-3.5" />
          {t(
            "deviceDashboard.windowHasNoFootage",
            "The device reports no recording in this window.",
          )}
        </p>
      )}

      <div className="overflow-hidden rounded-lg border border-border bg-black">
        <ArchiveViewport
          targetId={targetId}
          playing={visible}
          failed={failed}
          canExport={canExport}
          videoRef={videoRef}
          onExport={onExport}
          onDecodeFailure={handleDecodeFailure}
        />
      </div>
    </div>
  );
}

interface ArchiveViewportProps {
  targetId: string;
  /** The window being played, or `null` when nothing has been started. */
  playing: PlayingWindow | null;
  /** Set once the browser could not decode even the re-encoded stream. */
  failed: boolean;
  canExport: boolean;
  videoRef: RefObject<HTMLVideoElement | null>;
  onExport?: (channel: number, startMs: number, endMs: number) => void;
  onDecodeFailure: () => void;
}

/**
 * The black pane below the controls: idle placeholder, the video, or the
 * "this browser cannot decode it" escape hatch.
 *
 * Split out of the player so each state is a named, flat branch — the three
 * cases used to be one nested conditional inside the JSX, where the failure
 * pane was the hardest state to find.
 */
function ArchiveViewport({
  targetId,
  playing,
  failed,
  canExport,
  videoRef,
  onExport,
  onDecodeFailure,
}: Readonly<ArchiveViewportProps>) {
  const { t } = useTranslation();

  if (!playing) {
    return (
      <div className="flex min-h-56 items-center justify-center p-6 text-center text-xs text-muted-foreground">
        {t("deviceDashboard.pickAWindow", "Pick a period and press Watch to load recorded video.")}
      </div>
    );
  }

  if (failed) {
    return (
      <div className="flex min-h-56 flex-col items-center justify-center gap-3 p-6 text-center">
        <p className="text-xs text-muted-foreground">
          {t(
            "deviceDashboard.playbackUnsupported",
            "This browser cannot play the recording's format. Export the clip to view it locally.",
          )}
        </p>
        {canExport && (
          <Button
            type="button"
            size="sm"
            variant="outline"
            className="gap-1.5"
            onClick={() => onExport?.(playing.channel, playing.startMs, playing.endMs)}
          >
            <Download className="h-4 w-4" />
            {t("deviceDashboard.exportSelected", "Export selected range")}
          </Button>
        )}
      </div>
    );
  }

  return (
    <video
      ref={videoRef}
      key={`${playing.channel}-${playing.startMs}-${playing.endMs}-${playing.transcode}`}
      src={playbackUrl(
        API_BASE_URL,
        targetId,
        playing.channel,
        playing.startMs,
        playing.endMs,
        playing.transcode,
      )}
      controls
      autoPlay
      className="max-h-[60vh] w-full"
      onError={onDecodeFailure}
    >
      <track kind="captions" />
    </video>
  );
}

export default ArchivePlayer;
