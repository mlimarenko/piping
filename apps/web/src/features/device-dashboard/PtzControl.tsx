import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  ChevronUp,
  Move,
  ZoomIn,
  ZoomOut,
} from "lucide-react";
import { cn } from "@/shared/lib/utils";
import { sendPtzCommand, type PtzCommand } from "./api";

/** A single directional/zoom nudge as a normalized velocity vector. */
interface PtzMove {
  pan: number;
  tilt: number;
  zoom: number;
}

const NUDGES: ReadonlyArray<{ key: string; label: string; move: PtzMove; icon: typeof Move }> = [
  { key: "up", label: "Up", move: { pan: 0, tilt: 1, zoom: 0 }, icon: ChevronUp },
  { key: "left", label: "Left", move: { pan: -1, tilt: 0, zoom: 0 }, icon: ChevronLeft },
  { key: "right", label: "Right", move: { pan: 1, tilt: 0, zoom: 0 }, icon: ChevronRight },
  { key: "down", label: "Down", move: { pan: 0, tilt: -1, zoom: 0 }, icon: ChevronDown },
];

const ZOOMS: ReadonlyArray<{ key: string; label: string; move: PtzMove; icon: typeof Move }> = [
  { key: "zoom-in", label: "Zoom in", move: { pan: 0, tilt: 0, zoom: 1 }, icon: ZoomIn },
  { key: "zoom-out", label: "Zoom out", move: { pan: 0, tilt: 0, zoom: -1 }, icon: ZoomOut },
];

/**
 * Press-and-hold PTZ pad: pointer-down starts a continuous move on that axis,
 * pointer-up (or leaving the button) halts it with a stop. Rendered only when
 * the device reports the `ptz` capability AND the caller holds the `ptz` write
 * permission — an unpermitted operator never sees the pad, so it never triggers
 * a 403.
 */
function PtzControl({
  targetId,
  channel,
}: Readonly<{
  targetId: string;
  channel: number;
}>) {
  const { t } = useTranslation();
  const [failed, setFailed] = useState(false);
  // Guards a stop against firing when no move is in flight (e.g. pointer-leave
  // without a prior press), and coalesces overlapping presses.
  const moving = useRef(false);
  const commandChain = useRef(Promise.resolve());

  const queueCommand = useCallback(
    (command: PtzCommand) => {
      commandChain.current = commandChain.current
        .catch(() => undefined)
        .then(() => sendPtzCommand(targetId, command))
        .catch(() => setFailed(true));
    },
    [targetId],
  );

  const startMove = useCallback(
    (move: PtzMove) => {
      moving.current = true;
      const command: PtzCommand = { kind: "move", channel, ...move };
      queueCommand(command);
    },
    [channel, queueCommand],
  );

  const stopMove = useCallback(() => {
    if (!moving.current) return;
    moving.current = false;
    const command: PtzCommand = { kind: "stop", channel };
    queueCommand(command);
  }, [channel, queueCommand]);

  useEffect(() => stopMove, [stopMove]);

  // Keyboard press-and-hold mirrors the pointer semantics: Enter/Space DOWN
  // starts the move, UP stops it — a native button's synthesized `click` (one
  // event on key-up) has no "hold" concept, so it's replaced entirely here
  // rather than layered on top of it (hence `preventDefault` on both).
  // `event.repeat` guards against the OS's key-repeat re-firing `startMove` on
  // every auto-repeat tick while the key stays held.
  const holdHandlers = (move: PtzMove) => ({
    onPointerDown: () => startMove(move),
    onPointerUp: stopMove,
    onPointerLeave: stopMove,
    onPointerCancel: stopMove,
    // Mirrors `onPointerLeave`: if focus moves away mid-hold (tab away, window
    // blur) the motor must not keep running with no way left to stop it.
    onBlur: stopMove,
    onKeyDown: (event: React.KeyboardEvent<HTMLButtonElement>) => {
      if (event.key !== "Enter" && event.key !== " ") return;
      event.preventDefault();
      if (event.repeat) return;
      startMove(move);
    },
    onKeyUp: (event: React.KeyboardEvent<HTMLButtonElement>) => {
      if (event.key !== "Enter" && event.key !== " ") return;
      event.preventDefault();
      stopMove();
    },
  });

  return (
    <div className="flex flex-wrap items-center gap-3">
      <span className="inline-flex items-center gap-1 text-sm font-medium text-muted-foreground">
        <Move className="size-4" />
        {t("deviceDashboard.ptz", "PTZ")}
      </span>

      {/* Directional pad: a 3×3 grid with the cross populated. */}
      <div className="grid grid-cols-3 grid-rows-3 gap-1">
        {NUDGES.map((nudge) => (
          <PtzButton
            key={nudge.key}
            label={t(`deviceDashboard.ptz${nudge.label.replace(/\s/g, "")}`, nudge.label)}
            icon={nudge.icon}
            className={cn(
              nudge.key === "up" && "col-start-2 row-start-1",
              nudge.key === "left" && "col-start-1 row-start-2",
              nudge.key === "right" && "col-start-3 row-start-2",
              nudge.key === "down" && "col-start-2 row-start-3",
            )}
            {...holdHandlers(nudge.move)}
          />
        ))}
      </div>

      {/* Zoom cluster. */}
      <div className="flex gap-1">
        {ZOOMS.map((zoom) => (
          <PtzButton
            key={zoom.key}
            label={t(`deviceDashboard.ptz${zoom.label.replace(/\s/g, "")}`, zoom.label)}
            icon={zoom.icon}
            {...holdHandlers(zoom.move)}
          />
        ))}
      </div>

      {failed && (
        <span role="status" aria-live="polite" className="text-xs text-status-negative">
          {t("deviceDashboard.ptzFailed", "PTZ command failed")}
        </span>
      )}
    </div>
  );
}

function PtzButton({
  label,
  icon: Icon,
  className,
  ...handlers
}: Readonly<{
  label: string;
  icon: typeof Move;
  className?: string;
  onPointerDown: () => void;
  onPointerUp: () => void;
  onPointerLeave: () => void;
  onPointerCancel: () => void;
  onBlur: () => void;
  onKeyDown: (event: React.KeyboardEvent<HTMLButtonElement>) => void;
  onKeyUp: (event: React.KeyboardEvent<HTMLButtonElement>) => void;
}>) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      className={cn(
        "flex h-8 w-8 items-center justify-center rounded-md border border-border bg-background text-foreground transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
        "hover:border-primary/50 hover:bg-accent active:bg-accent",
        className,
      )}
      {...handlers}
    >
      <Icon className="h-4 w-4" />
    </button>
  );
}

export default PtzControl;
