import type { AutomationSecret } from "@/features/automation-secrets/api";
import type { PlaybookTargetPreviewItem, PlaybookTargetSetPreview } from "./api";

/**
 * One launch composition is previewed and checked up to this many hosts.
 *
 * This is a UI contract, not the backend run limit: the launch API still accepts
 * a prepared `target_limit` of 50,000 (`PLAYBOOK_RUN_MAX_TARGET_LIMIT`), and the
 * preview endpoint clamps `limit` to 500 for the same reason. A larger saved set
 * is shown to the operator as "narrow the selector" instead of queueing a run
 * whose composition nobody reviewed.
 */
export const LAUNCH_TARGET_LIMIT = 500;

/** Hosts rendered inline before the operator opens the full preview. */
export const LAUNCH_TARGET_SAMPLE = 5;

/** A page-owned query reduced to the three states a dialog must tell apart. */
export type LaunchResource<T> =
  | { status: "loading" }
  | { status: "failed" }
  | { status: "ready"; data: T };

/**
 * Reduce a query result to {@link LaunchResource}. An unresolved query is
 * `loading`, never an empty composition.
 */
export function launchResourceFromQuery<T>(query: { isError: boolean; data: T | undefined }): LaunchResource<T> {
  if (query.isError) return { status: "failed" };
  return query.data === undefined ? { status: "loading" } : { status: "ready", data: query.data };
}

export type LaunchBlockerCode =
  | "no-permission"
  | "target-set-loading"
  | "target-set-failed"
  | "empty-target-set"
  | "empty-selection"
  | "fixed-targets-unsupported"
  | "target-set-over-limit"
  | "unbound-address"
  | "secrets-loading"
  | "secrets-failed"
  | "missing-secrets";

export interface LaunchBlocker {
  code: LaunchBlockerCode;
  /** Affected hosts or secrets, when the blocker is countable. */
  count?: number;
}

/** The saved set composition the operator reviews before queueing the run. */
export interface LaunchComposition {
  targetSetId: string;
  name: string;
  mode: PlaybookTargetSetPreview["target_set"]["mode"];
  total: number;
  sample: PlaybookTargetPreviewItem[];
  /** Resolved hosts without a bound address; they cannot be reached over SSH. */
  unboundAddressCount: number;
}

export interface LaunchReadinessInput {
  /** The saved job resolves with at least one entrypoint and its project. */
  jobReady: boolean;
  /** Execute on the job template, including native controller execution. */
  canLaunch: boolean;
  /** Managed inventory target set; `null` for native inventory. */
  targetSetId: string | null;
  /** The run is narrowed to an explicit host selection instead of the saved set. */
  adHocTargets: boolean;
  fixedTargetCount: number;
  /** Caller-owned preview of {@link targetSetId}; `undefined` fails closed. */
  preview?: LaunchResource<PlaybookTargetSetPreview>;
  /** Declared `ENV_NAME -> secret id` references of the saved job. */
  secretRefs: Record<string, string>;
  /** Caller-owned named-secret list; `undefined` fails closed while refs exist. */
  secrets?: LaunchResource<AutomationSecret[]>;
}

export interface LaunchReadiness {
  launchable: boolean;
  blockers: LaunchBlocker[];
  composition: LaunchComposition | null;
}

/** A host without a bound address cannot be reached, so it cannot be launched. */
export function hasBoundAddress(item: PlaybookTargetPreviewItem): boolean {
  return item.address_bound && Boolean(item.address?.trim());
}

function compositionFrom(preview: PlaybookTargetSetPreview): LaunchComposition {
  return {
    targetSetId: preview.target_set.id,
    name: preview.target_set.name,
    mode: preview.target_set.mode,
    total: preview.total,
    sample: preview.items.slice(0, LAUNCH_TARGET_SAMPLE),
    unboundAddressCount: preview.items.filter((item) => !hasBoundAddress(item)).length,
  };
}

/** Referenced secrets that are absent from the visible list or hold no value. */
export function unavailableSecretCount(
  secretRefs: Record<string, string>,
  secrets: readonly AutomationSecret[],
): number {
  const byId = new Map(secrets.map((secret) => [secret.id, secret]));
  let missing = 0;
  for (const secretId of Object.values(secretRefs)) {
    const secret = byId.get(secretId);
    if (!secret || !secret.has_value) missing += 1;
  }
  return missing;
}

/**
 * Decide whether the saved job can be queued and what composition the operator
 * is about to run against.
 *
 * A loading or failed preview/secrets query blocks the launch: an unavailable
 * composition is never reported as an empty one, and a missing answer is not
 * permission to queue.
 */
export function evaluateLaunchReadiness(input: LaunchReadinessInput): LaunchReadiness {
  const blockers: LaunchBlocker[] = [];
  const needsComposition = input.targetSetId !== null && !input.adHocTargets;
  let composition: LaunchComposition | null = null;

  if (!input.canLaunch) blockers.push({ code: "no-permission" });

  if (needsComposition) {
    if (input.preview === undefined || input.preview.status === "loading") {
      blockers.push({ code: "target-set-loading" });
    } else if (input.preview.status === "failed") {
      blockers.push({ code: "target-set-failed" });
    } else {
      composition = compositionFrom(input.preview.data);
      if (composition.total === 0) {
        blockers.push({ code: "empty-target-set" });
      } else if (composition.total > LAUNCH_TARGET_LIMIT) {
        blockers.push({ code: "target-set-over-limit", count: composition.total });
      }
      if (composition.unboundAddressCount > 0) {
        blockers.push({ code: "unbound-address", count: composition.unboundAddressCount });
      }
    }
  } else if (input.adHocTargets) {
    // An explicit host selection narrows a saved managed inventory; native
    // inventory resolves its own host list and cannot be narrowed.
    if (input.targetSetId === null) {
      blockers.push({ code: "fixed-targets-unsupported" });
    } else if (input.fixedTargetCount === 0) {
      blockers.push({ code: "empty-selection" });
    }
    if (input.fixedTargetCount > LAUNCH_TARGET_LIMIT) {
      blockers.push({ code: "target-set-over-limit", count: input.fixedTargetCount });
    }
  }

  const hasSecretRefs = Object.keys(input.secretRefs).length > 0;
  if (hasSecretRefs) {
    if (input.secrets === undefined || input.secrets.status === "loading") {
      blockers.push({ code: "secrets-loading" });
    } else if (input.secrets.status === "failed") {
      blockers.push({ code: "secrets-failed" });
    } else {
      const missing = unavailableSecretCount(input.secretRefs, input.secrets.data);
      if (missing > 0) blockers.push({ code: "missing-secrets", count: missing });
    }
  }

  return {
    launchable: input.jobReady && blockers.length === 0,
    blockers,
    composition,
  };
}
