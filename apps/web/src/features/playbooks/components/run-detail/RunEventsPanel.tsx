import { useMemo, useRef, useState } from "react";
import { useCollectionPreviewFocus } from "@/shared/ui-shared/useCollectionPreviewFocus";
import { useTranslation } from "react-i18next";
import { useQuery } from "@tanstack/react-query";
import { ArrowDown, FileText, Terminal, XCircle } from "lucide-react";

import { Badge } from "@/shared/ui-primitives/badge";
import {
  listPlaybookRunArtifacts,
  listPlaybookRunEvents,
  type PlaybookArtifact,
  type PlaybookRun,
  type PlaybookRunEvent,
} from "@/features/playbooks/api";
import {
  isTerminalStatus,
  isStopping,
  runStatusLabelKey,
  runStatusVariant,
  statusPhase,
} from "@/features/playbooks/runPhase";
import {
  consoleOutputEvents,
  formatRunOutputEvent,
} from "@/features/playbooks/runOutput";
import { useAutoScroll } from "@/features/playbooks/useAutoScroll";
import { usePlaybookRunStream } from "@/features/playbooks/usePlaybookRunStream";
import { ArtifactViewer } from "./ArtifactViewer";
import {
  AnsibleEventLine,
  EmptyOutputState,
  OutputEndMarker,
} from "./RunOutputView";
import { PhaseGlyph } from "./RunStatusBar";
import { formatArtifactTypeLabel, formatSize } from "./artifactFormat";
import { latestReadableStatusMessage } from "./runEventText";
import { useLiveRun } from "./useLiveRun";

// ---------------------------------------------------------------------------
// RunEventsPanel — event stream with ansible-native colouring and auto-scroll
// ---------------------------------------------------------------------------

export function RunEventsPanel({
  run,
  panelActive,
  pollPerRun,
}: {
  run: PlaybookRun;
  panelActive: boolean;
  pollPerRun: boolean;
}) {
  const { t } = useTranslation();
  const [viewingArtifact, setViewingArtifact] = useState<PlaybookArtifact | null>(null);
  const outputAnchor = useRef<HTMLDivElement>(null);
  const artifactFocus = useCollectionPreviewFocus(outputAnchor, panelActive);

  const seedTerminal = isTerminalStatus(run.status);
  const live = useLiveRun(run, panelActive, pollPerRun);

  // Live WS stream while the run is non-terminal.
  const stream = usePlaybookRunStream(run.id, panelActive && !seedTerminal);

  const effectiveStatus = stream.status ?? live.status;
  const phase = statusPhase(effectiveStatus);
  const terminal = isTerminalStatus(effectiveStatus);
  const failed = effectiveStatus === "failed" || effectiveStatus === "timed_out";
  const canceled = effectiveStatus === "canceled";
  // Stopping: cancel_requested_at is available from the polled live run data.
  const runIsStopping = isStopping(effectiveStatus, live.cancel_requested_at);
  const effectiveExitCode = stream.exitCode ?? live.exit_code ?? null;
  const effectiveError = stream.errorMessage ?? live.error_message ?? null;

  const eventsQuery = useQuery({
    queryKey: ["playbooks", "run-events", run.id],
    queryFn: () => listPlaybookRunEvents(run.id, { limit: 500 }),
    enabled: panelActive && !stream.isLive,
    refetchInterval: terminal ? false : 2500,
  });
  const polledEvents = useMemo(() => eventsQuery.data?.items ?? [], [eventsQuery.data]);
  const events = useMemo(
    () => (stream.events.length > 0 ? stream.events : polledEvents),
    [stream.events, polledEvents],
  );
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
    (stream.isLive ||
      (!terminal && !eventsQuery.isError) ||
      (!stream.isLive && eventsQuery.isPending));
  let emptyOutputMessage = t("playbooks.runDetail.noEvents");
  if (canceled) emptyOutputMessage = t("playbooks.runDetail.outputEmptyCanceled");
  else if (runIsStopping) emptyOutputMessage = t("playbooks.runDetail.outputEmptyStopping");
  else if (!terminal) emptyOutputMessage = t("playbooks.runDetail.outputEmptyRunning");

  const artifactsQuery = useQuery({
    queryKey: ["playbooks", "run-artifacts", run.id],
    queryFn: () => listPlaybookRunArtifacts(run.id, { limit: 50 }),
    enabled: panelActive,
    refetchInterval: terminal ? false : 5000,
  });
  const artifacts = artifactsQuery.data?.items ?? [];

  const { containerRef, userScrolled, onScroll, scrollToBottom } =
    useAutoScroll<HTMLDivElement>(renderedOutputEvents.length);

  return (
    <div ref={outputAnchor} role="region" aria-label={t("playbooks.runDetail.output")} tabIndex={-1} className="flex min-h-0 flex-1 flex-col space-y-2 rounded-md focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
      {/* Per-run status row */}
      <div className="flex flex-wrap items-center gap-2 px-0.5">
        {failed ? (
          <XCircle className="h-3.5 w-3.5 shrink-0 text-status-negative" />
        ) : (
          <PhaseGlyph phase={phase} failed={failed} canceled={canceled} className="h-3.5 w-3.5" />
        )}
        {runIsStopping ? (
          <Badge variant="status-staging" className="rounded-md">
            {t("playbooks.runStatus.stopping")}
          </Badge>
        ) : (
          <Badge
            variant={runStatusVariant(effectiveStatus)}
            className="rounded-md"
          >
            {t(runStatusLabelKey(effectiveStatus))}
          </Badge>
        )}
        {stream.isLive && !terminal ? (
          <span className="flex shrink-0 items-center gap-2 text-xs font-medium text-status-positive">
            <span className="h-1.5 w-1.5 animate-pulse rounded-full bg-status-positive" />
            {t("playbooks.runDetail.live")}
          </span>
        ) : null}
        {effectiveExitCode !== null ? (
          <Badge variant="secondary" className="rounded-md font-mono">
            {t("playbooks.runDetail.exitCode", { code: effectiveExitCode })}
          </Badge>
        ) : null}
        {statusMessage ? (
          <span className="min-w-0 flex-1 break-words text-xs text-muted-foreground [overflow-wrap:anywhere]">
            {statusMessage}
          </span>
        ) : null}
      </div>

      {effectiveError ? (
        <p className="rounded-md border border-status-negative bg-card px-3 py-2 text-sm text-status-negative">
          {effectiveError}
        </p>
      ) : null}

      {/* Output stream */}
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
          {showSpinner ? (
            <EmptyOutputState spinning message={emptyOutputMessage} />
          ) : renderedOutputEvents.length === 0 ? (
            <EmptyOutputState spinning={false} message={emptyOutputMessage} />
          ) : (
            renderedOutputEvents.map(({ event, message }) => (
              <AnsibleEventLine
                key={event.event_sequence}
                event={event}
                message={message}
              />
            ))
          )}
          {terminal ? <OutputEndMarker status={effectiveStatus} /> : null}
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

      {/* Artifacts — each row opens an inline content viewer */}
      {artifacts.length > 0 ? (
        <div className="rounded-md border border-border bg-background">
          <div className="flex items-center gap-1.5 border-b border-border/30 px-2.5 py-1.5">
            <FileText className="h-3 w-3 text-muted-foreground" />
            <span className="text-sm font-medium text-foreground">
              {t("playbooks.runDetail.artifacts", { count: artifacts.length })}
            </span>
          </div>
          <div className="divide-y divide-border/25">
            {artifacts.map((artifact) => (
              <button
                key={artifact.id}
                type="button"
                onClick={event => { artifactFocus.capture(event.currentTarget); setViewingArtifact(artifact); }}
                className="flex min-h-9 w-full items-center justify-between gap-2 px-3 py-2 text-left transition-colors hover:bg-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
              >
                <span className="truncate text-sm font-medium text-foreground">
                  {artifact.name}
                </span>
                <div className="flex shrink-0 items-center gap-1.5">
                  <span className="text-xs text-muted-foreground">
                    {formatSize(artifact.content_size_bytes)}
                  </span>
                  <span className="rounded-md bg-muted px-2 py-1 text-xs text-muted-foreground">
                    {formatArtifactTypeLabel(artifact.artifact_type, t)}
                  </span>
                </div>
              </button>
            ))}
          </div>
        </div>
      ) : null}

      {/* Artifact content viewer (modal dialog on top of the non-modal sheet) */}
      {viewingArtifact !== null ? (
        <ArtifactViewer
          artifact={viewingArtifact}
          onClose={() => setViewingArtifact(null)}
          onCloseAutoFocus={artifactFocus.onCloseAutoFocus}
        />
      ) : null}
    </div>
  );
}
