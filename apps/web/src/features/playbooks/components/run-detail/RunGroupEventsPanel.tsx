import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import { ArrowDown, Terminal, XCircle } from "lucide-react";

import { Badge } from "@/shared/ui-primitives/badge";
import { type PlaybookRun, type PlaybookRunEvent } from "@/features/playbooks/api";
import {
  isTerminalStatus,
  runStatusLabelKey,
  runStatusVariant,
  statusPhase,
} from "@/features/playbooks/runPhase";
import {
  consoleOutputEvents,
  formatRunOutputEvent,
} from "@/features/playbooks/runOutput";
import { useAutoScroll } from "@/features/playbooks/useAutoScroll";
import {
  AnsibleEventLine,
  EmptyOutputState,
  OutputEndMarker,
  type RunGroupOutputAggregateDisplay,
} from "./RunOutputView";
import { PhaseGlyph } from "./RunStatusBar";
import { latestReadableStatusMessage } from "./runEventText";
import { useAccumulatedRunGroupEvents } from "./useAccumulatedRunGroupEvents";

// The group console is a single stream: one grouped launch is ONE native
// `ansible-playbook` process, so its stdout is shown exactly as Ansible printed
// it. There is deliberately no per-host output filter — splitting an interleaved
// Ansible console by host cannot be done faithfully from text, and hiding lines
// breaks the 1:1 output guarantee. Per-host STATE (status, stop, recap) lives in
// the targets rail, which is fed by run rows rather than output parsing.
export function RunGroupEventsPanel({
  groupId,
  groupStatus,
  aggregateDisplay,
  panelActive,
}: {
  groupId: string;
  groupStatus: PlaybookRun["status"] | null;
  aggregateDisplay?: RunGroupOutputAggregateDisplay | null;
  panelActive: boolean;
}) {
  const { t } = useTranslation();
  const effectiveStatus = groupStatus ?? "queued";
  const phase = statusPhase(effectiveStatus);
  const terminal = isTerminalStatus(effectiveStatus);
  const failed = effectiveStatus === "failed" || effectiveStatus === "timed_out";
  const canceled =
    !failed &&
    (effectiveStatus === "canceled" ||
      (terminal && aggregateDisplay?.anyCanceled === true));
  const aggregateIsStopping = aggregateDisplay?.allActiveStopping === true;
  let emptyOutputMessage = t("playbooks.runDetail.noEvents");
  if (canceled) emptyOutputMessage = t("playbooks.runDetail.outputEmptyCanceled");
  else if (aggregateIsStopping) emptyOutputMessage = t("playbooks.runDetail.outputEmptyStopping");
  else if (!terminal) emptyOutputMessage = t("playbooks.runDetail.outputEmptyRunning");
  const {
    events,
    isPending: eventsPending,
    isError: eventsError,
  } = useAccumulatedRunGroupEvents(groupId, panelActive, terminal);
  const outputEvents = useMemo(() => consoleOutputEvents(events), [events]);
  const renderedOutputEvents = useMemo(
    () =>
      outputEvents
        .map((event) => ({
          event,
          message: formatRunOutputEvent(event),
        }))
        .filter(
          (item): item is { event: PlaybookRunEvent; message: string } =>
            item.message !== null,
        ),
    [outputEvents],
  );
  const statusMessage = useMemo(() => latestReadableStatusMessage(events), [events]);
  const showSpinner =
    renderedOutputEvents.length === 0 &&
    (eventsPending || (!terminal && !eventsError));

  const { containerRef, userScrolled, onScroll, scrollToBottom } =
    useAutoScroll<HTMLDivElement>(renderedOutputEvents.length);

  return (
    <div className="flex min-h-0 flex-1 flex-col space-y-2">
      <div className="flex flex-wrap items-center gap-2 px-0.5">
        {failed ? (
          <XCircle className="h-3.5 w-3.5 shrink-0 text-status-negative" />
        ) : (
          <PhaseGlyph phase={phase} failed={failed} canceled={canceled} className="h-3.5 w-3.5" />
        )}
        {aggregateIsStopping ? (
          <Badge variant="status-staging" className="rounded-md">
            {t("playbooks.runStatus.stopping")}
          </Badge>
        ) : (
          <Badge variant={runStatusVariant(effectiveStatus)} className="rounded-md">
            {t(runStatusLabelKey(effectiveStatus))}
          </Badge>
        )}
        {statusMessage ? (
          <span className="min-w-0 flex-1 break-words text-xs text-muted-foreground [overflow-wrap:anywhere]">
            {statusMessage}
          </span>
        ) : null}
      </div>

      <div className="relative flex min-h-0 flex-1 flex-col rounded-md border border-border bg-background">
        <div className="flex items-center gap-1.5 border-b border-border/30 px-2.5 py-1.5">
          <Terminal className="h-3 w-3 text-muted-foreground" />
          <span className="text-sm font-medium text-foreground">
            {t("playbooks.runDetail.output")}
          </span>
        </div>
        <div
          ref={containerRef}
          className="min-h-0 flex-1 overflow-x-hidden overflow-y-auto p-2"
          onScroll={onScroll}
        >
          {eventsError ? (
            <p className="rounded-md border border-status-negative bg-card px-3 py-2 text-sm text-status-negative">
              {t("playbooks.runDetail.outputLoadFailed")}
            </p>
          ) : showSpinner ? (
            <EmptyOutputState
              spinning
              message={emptyOutputMessage}
              aggregateDisplay={aggregateDisplay ?? null}
            />
          ) : renderedOutputEvents.length === 0 ? (
            <EmptyOutputState
              spinning={false}
              message={emptyOutputMessage}
              aggregateDisplay={aggregateDisplay ?? null}
            />
          ) : (
            renderedOutputEvents.map(({ event, message }) => (
              <AnsibleEventLine key={event.id} event={event} message={message} />
            ))
          )}
          {terminal ? <OutputEndMarker status={effectiveStatus} canceled={canceled} /> : null}
        </div>
        {userScrolled ? (
          <button
            type="button"
            onClick={scrollToBottom}
            className="absolute bottom-3 right-3 flex min-h-9 items-center gap-2 rounded-md border border-border bg-card px-3 py-2 text-sm text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          >
            <ArrowDown className="h-3 w-3" />
            {t("playbooks.runDetail.scrollToBottom")}
          </button>
        ) : null}
      </div>
    </div>
  );
}
