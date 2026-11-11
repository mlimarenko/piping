import type { CreatePlaybookAssignmentRequest, PlaybookAssignment, PlaybookAssignmentScopeKind } from "./api";
import { isUuid } from "@/shared/lib/ids";
import { z } from "zod";

export interface AssignmentForm {
  jobTemplateId: string;
  targetSetId: string;
  name: string;
  scopeKind: PlaybookAssignmentScopeKind;
  scopeId: string;
  priority: string;
  enabled: boolean;
  visible: boolean;
  executable: boolean;
  quickActionEnabled: boolean;
}

export function assignmentForm(item?: PlaybookAssignment): AssignmentForm {
  return {
    jobTemplateId: item?.job_template_id ?? "", targetSetId: item?.target_set_id ?? "",
    name: item?.name ?? "", scopeKind: item?.scope_kind ?? "global", scopeId: item?.scope_id ?? "",
    priority: String(item?.priority ?? 0), enabled: item?.enabled ?? true, visible: item?.visible ?? true,
    executable: item?.executable ?? true, quickActionEnabled: item?.quick_action_enabled ?? false,
  };
}

export function assignmentRequest(form: AssignmentForm): CreatePlaybookAssignmentRequest {
  const priority = Number(form.priority);
  if (!isUuid(form.jobTemplateId) || !isUuid(form.targetSetId) || (form.scopeKind !== "global" && !isUuid(form.scopeId)) || !/^-?\d+$/.test(form.priority) || !Number.isInteger(priority) || priority < -2147483648 || priority > 2147483647) {
    throw new Error("Select a job, target set and scope, and enter a valid priority.");
  }
  return { job_template_id: form.jobTemplateId, target_set_id: form.targetSetId, name: form.name.trim() || null,
    scope_kind: form.scopeKind, scope_id: form.scopeKind === "global" ? null : form.scopeId,
    priority, enabled: form.enabled, visible: form.visible, executable: form.executable, quick_action_enabled: form.quickActionEnabled };
}

const assignmentFormSchema = z.object({
  jobTemplateId: z.string(),
  targetSetId: z.string(),
  name: z.string(),
  scopeKind: z.enum(["global", "region", "site", "tenant", "role", "platform", "cluster", "connection_target"]),
  scopeId: z.string(),
  priority: z.string(),
  enabled: z.boolean(),
  visible: z.boolean(),
  executable: z.boolean(),
  quickActionEnabled: z.boolean(),
});

/** Draft carried through the target-set create flow; assignment metadata only, no secret or run payload. */
const assignmentReturnFlowDraftSchema = z.object({
  editingId: z.string().refine(isUuid).nullable(),
  form: assignmentFormSchema,
});

export type AssignmentReturnFlowDraft = z.infer<typeof assignmentReturnFlowDraftSchema>;

export function assignmentReturnFlowDraft(value: unknown): AssignmentReturnFlowDraft | null {
  const parsed = assignmentReturnFlowDraftSchema.safeParse(value);
  return parsed.success ? parsed.data : null;
}
