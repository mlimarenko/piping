import { memo } from "react";
import { useTranslation } from "react-i18next";
import { CheckCircle2, Circle, Loader2, XCircle } from "lucide-react";

import { cn } from "@/shared/lib/utils";
import { type PlaybookRun, type PlaybookRunEvent } from "@/features/playbooks/api";
import { runStatusLabelKey } from "@/features/playbooks/runPhase";
import { parseAnsiLine } from "@/features/playbooks/runOutput";
import { classifyEvent, keySegmentsByOffset, scopedEventMessage } from "./runEventText";

// ---------------------------------------------------------------------------
// Output-stream primitives — ansible-native lines, end marker, empty state
// ---------------------------------------------------------------------------

export const AnsibleEventLine = memo(function AnsibleEventLine({
  event,
  message,
  selectedHost,
  selectedRunId,
}: {
  event: PlaybookRunEvent;
  message?: string;
  selectedHost?: string | null;
  selectedRunId?: string | null;
}) {
  const { colorClass, prefix, isHeader } = classifyEvent(event);
  const renderedMessage = message ?? scopedEventMessage(event, selectedHost, selectedRunId);
  // Raw console lines carry ansible's own ANSI colouring — render it verbatim as
  // coloured spans. Mapped/clean lines have no escapes and keep the semantic
  // classifyEvent colour. Memoised so growing the log doesn't re-parse every line.
  const segments = parseAnsiLine(renderedMessage);
  const colored = segments.length > 1 || segments[0]?.className !== "";
  // Key by cumulative character offset (content-derived, stable across renders)
  // rather than the array index, which the reconciler treats as positional.
  const keyedSegments = keySegmentsByOffset(segments);
  return (
    <div
      className={cn(
        "flex max-w-full gap-1 overflow-hidden font-mono text-sm leading-relaxed",
        !colored && colorClass,
        isHeader && "mt-3 border-t border-border/20 pt-2",
      )}
    >
      {prefix ? <span className="shrink-0 select-none">{prefix}</span> : null}
      <span className="min-w-0 whitespace-pre-wrap break-words [overflow-wrap:anywhere]">
        {/* Always render parsed segments: their text is escape-stripped, so a
            line that carries ANSI but no colour (only resets, or an all-escape
            separator) can never leak literal `[0;31m` codes. Coloured segments
            carry ANSI classes; uncoloured ones inherit the semantic
            `classifyEvent` colour from the container above. */}
        {keyedSegments.map(({ key, segment }) => (
          <span key={key} className={segment.className || undefined}>
            {segment.text}
          </span>
        ))}
      </span>
    </div>
  );
});

export function OutputEndMarker({
  status,
  canceled: canceledOverride = false,
}: {
  status: PlaybookRun["status"];
  canceled?: boolean;
}) {
  const { t } = useTranslation();
  const failed = status === "failed" || status === "timed_out";
  const canceled = !failed && (canceledOverride || status === "canceled");
  let state: "failed" | "canceled" | "succeeded" = "succeeded";
  if (failed) state = "failed";
  else if (canceled) state = "canceled";
  let toneClass = "text-muted-foreground";
  if (failed) toneClass = "text-status-negative/80";
  else if (canceled) toneClass = "text-status-warning/80";
  let stateIcon = <CheckCircle2 className="h-3 w-3 shrink-0" />;
  if (failed) stateIcon = <XCircle className="h-3 w-3 shrink-0" />;
  else if (canceled) stateIcon = <Circle className="h-3 w-3 shrink-0" />;
  return (
    <div
      data-testid="run-output-end-marker"
      data-state={state}
      className={cn(
        "mt-3 flex items-center gap-2 border-t border-border pt-2 text-xs",
        toneClass,
      )}
    >
      {stateIcon}
      <span>
        {t("playbooks.runDetail.outputFinished", {
          status: t(canceled ? runStatusLabelKey("canceled") : runStatusLabelKey(status)),
        })}
      </span>
    </div>
  );
}

export type RunGroupOutputAggregateDisplay = {
  doneCount: number;
  totalCount: number;
  failedCount: number;
  canceledCount: number;
  anyCanceled: boolean;
  allActiveStopping: boolean;
};

export function EmptyOutputState({
  spinning,
  message,
  aggregateDisplay,
}: Readonly<{
  spinning: boolean;
  message: string;
  aggregateDisplay?: RunGroupOutputAggregateDisplay | null;
}>) {
  const { t } = useTranslation();
  return (
    <div className="flex flex-col items-center justify-center gap-2 py-6 text-center text-sm text-muted-foreground">
      <div className="flex items-center justify-center gap-2">
        {spinning ? <Loader2 className="h-3 w-3 animate-spin" /> : null}
        <span>{message}</span>
      </div>
      {aggregateDisplay ? (
        <div className="flex flex-wrap items-center justify-center gap-x-2 gap-y-1 text-xs">
          <span>
            {t("playbooks.runDetail.outputEmptyProgress", {
              done: aggregateDisplay.doneCount,
              total: aggregateDisplay.totalCount,
            })}
          </span>
          {aggregateDisplay.failedCount > 0 ? (
            <span className="text-status-negative">
              {t("playbooks.runDetail.failedCount", { count: aggregateDisplay.failedCount })}
            </span>
          ) : null}
          {aggregateDisplay.canceledCount > 0 ? (
            <span className="text-status-warning">
              {t("playbooks.runDetail.canceledCount", {
                count: aggregateDisplay.canceledCount,
              })}
            </span>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
