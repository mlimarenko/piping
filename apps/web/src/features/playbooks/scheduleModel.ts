import { isUuid } from "@/shared/lib/ids";
import { z } from "zod";
import type { CreateScheduleRequest, PlaybookSchedule, PlaybookTargetSetPreview, ResolvedTarget, ScheduleTargets, UpdateScheduleRequest } from "./api";
import { emptyScheduleForm, type ScheduleForm } from "./playbookFormModels";
import { parseParameterValues } from "./varsSchema";

export const scheduleDraftErrorDefaults = {
  invalid: "Complete all required fields and provide a valid cron and timezone.",
  targets: "The target selection is invalid. Check the JSON and the composition.",
  parameters: "The parameter overrides must be a JSON object.",
} as const;
export type ScheduleDraftError = keyof typeof scheduleDraftErrorDefaults;

export type ScheduleTargetsError = "jsonSyntax" | "selectorShape" | "fixedShape" | "emptyFixed";
export const scheduleTargetsErrorDefaults: Record<ScheduleTargetsError, string> = {
  jsonSyntax: "The targets are not valid JSON.",
  selectorShape: "The dynamic selector must be a JSON object.",
  fixedShape: "The fixed snapshot must contain resolved host identities.",
  emptyFixed: "The fixed snapshot has no hosts.",
};

export function fixedTargetsFromPreview(preview: PlaybookTargetSetPreview): ResolvedTarget[] {
  if (preview.truncated || preview.items.length !== preview.total || !preview.items.length || preview.items.some((item) => !item.name.trim() || !isUuid(item.target_id) || !isUuid(item.source_id)) || new Set(preview.items.map((item) => item.target_id)).size !== preview.items.length) throw new Error("Incomplete or invalid target preview");
  return preview.items.map((item) => ({ hostname: item.name, target_id: item.target_id, source_id: item.source_id }));
}

function resolvedTarget(value: unknown): value is ResolvedTarget {
  if (!value || typeof value !== "object" || !("hostname" in value) || typeof value.hostname !== "string" || !value.hostname.trim()) return false;
  if ("target_id" in value && value.target_id != null && !isUuid(value.target_id)) return false;
  if ("source_id" in value && value.source_id != null && !isUuid(value.source_id)) return false;
  return ("target_id" in value && isUuid(value.target_id)) || ("source_id" in value && isUuid(value.source_id));
}

/** Provenance of a copied selection: cleared whenever the draft is edited by hand. */
function targetSetReference(form: ScheduleForm): { target_set_id?: string } {
  return isUuid(form.targetSetId) ? { target_set_id: form.targetSetId } : {};
}

/** Field-level reason the target selection cannot be saved, or `null` when it is valid. */
export function scheduleTargetsError(form: ScheduleForm): ScheduleTargetsError | null {
  if (!form.targetsJson.trim()) return form.selectionMode === "dynamic" ? "selectorShape" : "emptyFixed";
  let raw: unknown;
  try {
    raw = JSON.parse(form.targetsJson);
  } catch {
    return "jsonSyntax";
  }
  if (form.selectionMode === "dynamic") return raw && typeof raw === "object" && !Array.isArray(raw) ? null : "selectorShape";
  if (!Array.isArray(raw) || !raw.length) return "emptyFixed";
  return raw.every(resolvedTarget) ? null : "fixedShape";
}

export function scheduleTargets(form: ScheduleForm): ScheduleTargets {
  const error = scheduleTargetsError(form);
  if (error) throw new Error(scheduleTargetsErrorDefaults[error]);
  const raw: unknown = JSON.parse(form.targetsJson);
  if (form.selectionMode === "dynamic") return { kind: "dynamic", selector: raw as Record<string, unknown>, ...targetSetReference(form) };
  return { kind: "fixed", targets: raw as ResolvedTarget[], ...targetSetReference(form) };
}

function validTimezone(timezone: string): boolean {
  try {
    new Intl.DateTimeFormat("en", { timeZone: timezone.trim() });
    return true;
  } catch {
    return false;
  }
}

export function scheduleRequest(form: ScheduleForm): CreateScheduleRequest {
  if (!form.name.trim() || !form.jobTemplateId || form.cronExpression.trim().split(/\s+/).length !== 5 || !validTimezone(form.timezone)) throw new Error("Complete all required fields");
  return { name: form.name.trim(), template_id: form.jobTemplateId, targets: scheduleTargets(form), cron_expression: form.cronExpression.trim(), timezone: form.timezone.trim(), misfire_policy: form.misfirePolicy, concurrency_policy: form.concurrencyPolicy, enabled: form.enabled, overrides: { extra_vars: parseParameterValues(form.extraVars) } };
}

export function updateScheduleRequest(form: ScheduleForm): UpdateScheduleRequest {
  const { name, targets, cron_expression, timezone, misfire_policy, concurrency_policy, enabled, overrides } = scheduleRequest(form);
  return { name, targets, cron_expression, timezone, misfire_policy, concurrency_policy, enabled, overrides };
}

export function scheduleForm(schedule: PlaybookSchedule): ScheduleForm {
  return { ...emptyScheduleForm(), name: schedule.name, jobTemplateId: schedule.template_id, selectionMode: schedule.targets.kind === "fixed" ? "static" : "dynamic", targetSetId: schedule.targets.target_set_id ?? "", targetsJson: JSON.stringify(schedule.targets.kind === "fixed" ? schedule.targets.targets : schedule.targets.selector, null, 2), cronExpression: schedule.cron_expression, timezone: schedule.timezone, concurrencyPolicy: schedule.concurrency_policy, misfirePolicy: schedule.misfire_policy, enabled: schedule.enabled, extraVars: JSON.stringify(schedule.overrides?.extra_vars ?? {}, null, 2) };
}

/** The frozen host list of a fixed snapshot, or `null` for a dynamic draft or invalid JSON. */
export function fixedSnapshotTargets(form: ScheduleForm): ResolvedTarget[] | null {
  if (form.selectionMode !== "static") return null;
  try {
    const targets = scheduleTargets(form);
    return targets.kind === "fixed" ? targets.targets : null;
  } catch {
    return null;
  }
}

export interface ScheduleSnapshotDiff {
  added: ResolvedTarget[];
  removed: ResolvedTarget[];
}

function snapshotTargetKey(target: ResolvedTarget): string {
  return target.target_id ?? `hostname:${target.hostname}`;
}

/** Added and removed hosts between the saved snapshot and the current set composition. */
export function snapshotDiff(current: readonly ResolvedTarget[], next: readonly ResolvedTarget[]): ScheduleSnapshotDiff {
  const currentKeys = new Set(current.map(snapshotTargetKey));
  const nextKeys = new Set(next.map(snapshotTargetKey));
  return {
    added: next.filter((target) => !currentKeys.has(snapshotTargetKey(target))),
    removed: current.filter((target) => !nextKeys.has(snapshotTargetKey(target))),
  };
}

export function validateScheduleDraft(form: ScheduleForm): ScheduleDraftError[] {
  const errors: ScheduleDraftError[] = [];
  try { parseParameterValues(form.extraVars); } catch { errors.push("parameters"); }
  if (scheduleTargetsError(form)) errors.push("targets");
  if (!form.name.trim() || !form.jobTemplateId || form.cronExpression.trim().split(/\s+/).length !== 5 || !validTimezone(form.timezone)) errors.push("invalid");
  return errors;
}

const scheduleFormSchema = z.object({
  name: z.string(),
  jobTemplateId: z.string(),
  selectionMode: z.enum(["static", "dynamic"]),
  targetSetId: z.string(),
  targetsJson: z.string(),
  cronExpression: z.string(),
  timezone: z.string(),
  extraVars: z.string(),
  concurrencyPolicy: z.enum(["skip_if_running", "queue_pending", "replace_pending"]),
  misfirePolicy: z.enum(["fire_once"]),
  enabled: z.boolean(),
});

/** Draft carried through the target-set create flow: the edited form and its dirty baseline. */
const scheduleReturnFlowDraftSchema = z.object({
  form: scheduleFormSchema,
  baseline: scheduleFormSchema,
});

export type ScheduleReturnFlowDraft = z.infer<typeof scheduleReturnFlowDraftSchema>;

export function scheduleReturnFlowDraft(value: unknown): ScheduleReturnFlowDraft | null {
  const parsed = scheduleReturnFlowDraftSchema.safeParse(value);
  return parsed.success ? parsed.data : null;
}
