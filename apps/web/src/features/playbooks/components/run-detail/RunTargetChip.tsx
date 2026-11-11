import { useTranslation } from "react-i18next";

import { type PlaybookRun } from "@/features/playbooks/api";
import { statusPhase } from "@/features/playbooks/runPhase";
import { PhaseGlyph } from "./RunStatusBar";
import { useLiveRun } from "./useLiveRun";

// ---------------------------------------------------------------------------
// RunTargetChip — compact wrapping-chip variant of RunRow for the target cloud
// (used in compact mode so a large fan-out doesn't push the log off-screen)
// ---------------------------------------------------------------------------

export function RunTargetChip({
  run,
  panelActive,
  pollPerRun,
}: {
  run: PlaybookRun;
  panelActive: boolean;
  pollPerRun: boolean;
}) {
  const { t } = useTranslation();
  const live = useLiveRun(run, panelActive, pollPerRun);
  const phase = statusPhase(live.status);
  const failed = live.status === "failed" || live.status === "timed_out";
  const canceled = live.status === "canceled";
  const unreported = live.status === "unreported";
  const name = live.target_name ?? "—";
  const address = live.target_address ?? "";
  const title = address ? `${name} · ${address}` : name;
  return (
    <div
      title={unreported ? `${title} · ${t("playbooks.runStatus.unreported")}` : title}
      className="flex max-w-[12rem] items-center gap-2 rounded-md border border-border bg-card px-2 py-1"
    >
      <PhaseGlyph
        phase={phase}
        failed={failed}
        canceled={canceled}
        unreported={unreported}
        className="h-2.5 w-2.5 shrink-0"
      />
      <span className="min-w-0 truncate text-xs font-medium text-foreground">{name}</span>
    </div>
  );
}
