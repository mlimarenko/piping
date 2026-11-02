import type { useTranslation } from "react-i18next";
import { CheckCircle2, Circle, Loader2, Square, X, XCircle } from "lucide-react";

import { Badge } from "@/shared/ui-primitives/badge";
import { cn } from "@/shared/lib/utils";
import { type RunPhase } from "@/features/playbooks/runPhase";
import { formatElapsed, phaseLabel, phaseVariant } from "./runStatus";

// ---------------------------------------------------------------------------
// RunStatusBar — sticky top strip (owns close, stop, phase, counters, elapsed)
// ---------------------------------------------------------------------------

export function PhaseGlyph({
  phase,
  failed,
  canceled,
  className,
}: {
  phase: RunPhase;
  failed?: boolean;
  canceled?: boolean;
  className?: string;
}) {
  if (phase === "finished") {
    if (failed) return <XCircle className={cn("shrink-0 text-status-negative", className)} />;
    if (canceled) return <Circle className={cn("shrink-0 text-status-warning", className)} />;
    return <CheckCircle2 className={cn("shrink-0 text-status-positive", className)} />;
  }
  if (phase === "running") {
    return <Loader2 className={cn("shrink-0 animate-spin text-status-warning", className)} />;
  }
  return <Circle className={cn("shrink-0 text-muted-foreground", className)} />;
}

export function RunStatusBar({
  playbookName,
  phase,
  anyFailed,
  anyCanceled,
  failedCount,
  doneCount,
  totalCount,
  elapsed,
  isRunning,
  onStop,
  stopping,
  allStopping,
  onClose,
  t,
}: {
  playbookName: string;
  phase: RunPhase;
  anyFailed: boolean;
  anyCanceled: boolean;
  failedCount: number;
  doneCount: number;
  totalCount: number;
  elapsed: number;
  isRunning: boolean;
  onStop?: () => void;
  stopping?: boolean;
  /** True when every active run has cancel_requested_at set — show stopping state in header. */
  allStopping?: boolean;
  onClose?: () => void;
  t: ReturnType<typeof useTranslation>["t"];
}) {
  const isStoppingAll = allStopping === true;
  return (
    <div className="flex h-11 shrink-0 items-center gap-3 border-b border-border/30 px-4">
      <PhaseGlyph
        phase={phase}
        failed={anyFailed}
        canceled={anyCanceled}
        className="h-4 w-4 shrink-0"
      />
      <Badge
        variant={phaseVariant(phase, anyFailed, anyCanceled)}
        className="shrink-0 rounded-md"
      >
        {phaseLabel(phase, anyFailed, anyCanceled, t)}
      </Badge>
      <span className="min-w-0 flex-1 truncate text-sm font-medium text-foreground">
        {playbookName}
      </span>
      {isRunning ? (
        <span className="shrink-0 text-xs tabular-nums text-muted-foreground">
          {formatElapsed(elapsed)}
        </span>
      ) : null}
      <span className="shrink-0 text-xs text-muted-foreground">
        {doneCount}/{totalCount}
        {failedCount > 0 ? (
          <>
            <span className="mx-1 text-muted-foreground/60" aria-hidden>
              {" · "}
            </span>
            <span className="text-status-negative">
              {t("playbooks.runDetail.failedCount", { count: failedCount })}
            </span>
          </>
        ) : null}
      </span>
      {isRunning && onStop ? (
        <button
          type="button"
          onClick={isStoppingAll ? undefined : onStop}
          disabled={stopping || isStoppingAll}
          className="flex min-h-9 shrink-0 items-center gap-2 rounded-md border border-status-negative bg-card px-3 py-2 text-sm font-medium text-status-negative transition-colors hover:bg-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-60"
        >
          {(stopping ?? false) || isStoppingAll ? (
            <Loader2 className="h-3 w-3 animate-spin" />
          ) : (
            <Square className="h-3 w-3 fill-current" />
          )}
          {isStoppingAll
            ? t("playbooks.runStatus.stopping")
            : t("playbooks.runDetail.stop")}
        </button>
      ) : null}
      {onClose ? (
        <button
          type="button"
          onClick={onClose}
          className="ml-1 flex size-9 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          aria-label={t("close", { defaultValue: "Close" })}
        >
          <X className="h-4 w-4" />
        </button>
      ) : null}
    </div>
  );
}
