import type { useTranslation } from "react-i18next";

import { type RunPhase } from "@/features/playbooks/runPhase";

// ---------------------------------------------------------------------------
// Phase → badge variant helpers
// ---------------------------------------------------------------------------

export function phaseVariant(
  phase: RunPhase,
  anyFailed: boolean,
  anyCanceled: boolean,
  anyUnreported = false,
) {
  if (phase === "finished") {
    if (anyFailed) return "status-failed" as const;
    if (anyCanceled) return "status-staging" as const;
    if (anyUnreported) return "status-neutral" as const;
    return "status-active" as const;
  }
  if (phase === "running") return "status-staging" as const;
  return "secondary" as const;
}

export function phaseLabel(
  phase: RunPhase,
  anyFailed: boolean,
  anyCanceled: boolean,
  anyUnreported: boolean,
  t: ReturnType<typeof useTranslation>["t"],
) {
  if (phase === "running") return t("playbooks.runDetail.phase.running");
  if (phase === "finished") {
    if (anyFailed) return t("playbooks.runDetail.phase.failed");
    if (anyCanceled) return t("playbooks.runStatus.canceled");
    if (anyUnreported) return t("playbooks.runStatus.unreported");
    return t("playbooks.runDetail.phase.succeeded");
  }
  return t("playbooks.runDetail.phase.pending");
}

// ---------------------------------------------------------------------------
// Elapsed — seconds → "1m 5s"; the ticking hook stays with the panel
// ---------------------------------------------------------------------------

export function formatElapsed(seconds: number): string {
  const m = Math.floor(seconds / 60);
  const s = seconds % 60;
  return m > 0 ? `${m}m ${s}s` : `${s}s`;
}
