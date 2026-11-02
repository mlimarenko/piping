import { useTranslation } from "react-i18next";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Loader2, Square } from "lucide-react";

import { cancelPlaybookRun, type PlaybookRun } from "@/features/playbooks/api";
import {
  isTerminalStatus,
  isStopping,
  statusPhase,
} from "@/features/playbooks/runPhase";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { PhaseGlyph } from "./RunStatusBar";
import { useLiveRun } from "./useLiveRun";

// ---------------------------------------------------------------------------
// RunRow
// ---------------------------------------------------------------------------

export function RunRow({
  run,
  panelActive,
  pollPerRun,
  groupId,
  groupedExecution,
  onStopped,
}: {
  run: PlaybookRun;
  panelActive: boolean;
  pollPerRun: boolean;
  /** Group id to invalidate when per-host stop succeeds. */
  groupId: string | null;
  groupedExecution: boolean;
  /** Called after a successful per-host stop so the parent can re-derive allActiveStopping. */
  onStopped?: () => void;
}) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const { can } = usePermissions();
  const live = useLiveRun(run, panelActive, pollPerRun);
  const phase = statusPhase(live.status);
  const failed = live.status === "failed" || live.status === "timed_out";
  const canceled = live.status === "canceled";
  const terminal = isTerminalStatus(live.status);
  const stopping = isStopping(live.status, live.cancel_requested_at);
  // Mirrors the backend cancel grant ("you can stop what you can run" —
  // Execute×Playbook in `authorize_cancel_for_playbook`). The per-playbook
  // owner short-circuit is a per-object decision and stays server-side.
  const canCancel = can("EXECUTE", "PLAYBOOK");

  const cancelMutation = useMutation({
    mutationFn: () => cancelPlaybookRun(live.id),
    onSuccess: () => {
      if (groupId) {
        void queryClient.invalidateQueries({ queryKey: ["playbooks", "run-group", groupId] });
      }
      void queryClient.invalidateQueries({ queryKey: ["playbooks", "run", live.id] });
      void queryClient.invalidateQueries({ queryKey: ["playbooks", "run-groups"] });
      onStopped?.();
    },
  });

  // A grouped Ansible run is one native ansible-playbook process; stopping one
  // host would abort every other host. The header-level group stop is the only
  // supported control for that execution model.
  const showStopButton =
    canCancel && !groupedExecution && !terminal && !stopping && !cancelMutation.isPending;
  // Show stopping badge when: cancel_requested_at set but not yet terminal.
  const showStoppingBadge = stopping || cancelMutation.isPending;

  return (
    <div className="group flex min-h-9 w-full items-center gap-2 rounded-md px-2 py-2 transition-colors hover:bg-accent">
      {/* Status display area — per-host state comes from run rows, not output parsing */}
      <div className="flex min-w-0 flex-1 items-center gap-2 text-left">
        <PhaseGlyph
          phase={phase}
          failed={failed}
          canceled={canceled}
          className="h-3 w-3 shrink-0"
        />
        <span className="min-w-0 flex-1">
          <span
            className="block truncate text-sm font-medium text-foreground"
            title={live.error_message ?? undefined}
          >
            {live.target_name ?? "—"}
          </span>
          <span className="block truncate text-xs text-muted-foreground">
            {live.target_address ?? "—"}
          </span>
        </span>
      </div>
      {/* Stopping badge (replaces stop button once cancel requested) */}
      {showStoppingBadge ? (
        <span className="flex shrink-0 items-center gap-2 rounded-md border border-status-warning bg-card px-2 py-1 text-xs font-medium text-status-warning">
          <Loader2 className="h-2.5 w-2.5 animate-spin" />
          {t("playbooks.runStatus.stopping")}
        </span>
      ) : null}
      {/* Per-host stop button */}
      {showStopButton ? (
        <button
          type="button"
          aria-label={t("playbooks.runDetail.stop")}
          onClick={(e) => {
            e.stopPropagation();
            cancelMutation.mutate();
          }}
          disabled={cancelMutation.isPending}
          className="flex size-9 shrink-0 items-center justify-center rounded-md text-status-negative transition-colors hover:bg-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-40"
        >
          <Square className="h-3 w-3 fill-current" />
        </button>
      ) : null}
    </div>
  );
}
