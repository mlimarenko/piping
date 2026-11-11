import type { ConnectionTarget } from "@/shared/api/types";
import { trackProductEvent } from "@/shared/lib/telemetry";
import { ApiError } from "@/shared/api/client";
import { isUuid } from "@/shared/lib/ids";
import { launchJobTemplate } from "./api";
import type { Extravars } from "./varsSchema";

/** The server narrows its prepared managed inventory; rejection never retries with saved targets. */
export async function launchJobOnTargets(
  jobTemplateId: string,
  targets: readonly ConnectionTarget[],
  source: string,
  extraVars?: Extravars,
) {
  const targetIds = targets.map((target) => target.id);
  if (!jobTemplateId || targetIds.length === 0 || targetIds.some((id) => !isUuid(id) || id === "00000000-0000-0000-0000-000000000000") || new Set(targetIds.map((id) => id.toLowerCase())).size !== targetIds.length) {
    throw new ApiError({ status: 400, code: "VALIDATION_ERROR", message: "A job and a nonempty selection of unique, valid target IDs are required." });
  }
  return launchJobTemplate(jobTemplateId, {
    target_ids: targetIds,
    metadata: { source, target_count: targets.length },
    ...(extraVars ? { overrides: { extra_vars: extraVars } } : {}),
  }).then((created) => {
    trackProductEvent("playbook_run_started");
    return created;
  });
}
