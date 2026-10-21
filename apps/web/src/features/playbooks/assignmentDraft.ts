import { z } from "zod";
import { isUuid } from "@/shared/lib/ids";

const assignmentDraftSchema = z.object({
  returnHref: z.string().refine((href) => href === "/automation/assignments" || href.startsWith("/automation/assignments?") || /^\/playbooks\/[0-9a-f-]+\?/.test(href)),
  editingId: z.string().refine(isUuid).nullable(),
  form: z.object({
    playbookId: z.string(), targetSetId: z.string(), name: z.string(),
    scopeKind: z.enum(["global", "region", "site", "tenant", "role", "platform", "cluster", "connection_target"]),
    scopeId: z.string(), priority: z.string(), enabled: z.boolean(), visible: z.boolean(),
    executable: z.boolean(), quickActionEnabled: z.boolean(),
  }),
});
const flowReference = z.object({ assignmentFlowId: z.string().refine(isUuid) });
const storageKey = (id: string) => `piping.assignment-flow:${id}`;

/** One session-scoped owner; history entries carry a reference, never draft snapshots.
 * The schema contains only assignment metadata, no secret or execution payload.
 * Removing this owner consumes every old history reference, including after reload.
 */
export function startAssignmentDraft(draft: unknown, state?: unknown) {
  const current = flowReference.safeParse(state);
  const id = current.success && readAssignmentDraft(state) ? current.data.assignmentFlowId : crypto.randomUUID();
  window.sessionStorage.setItem(storageKey(id), JSON.stringify(assignmentDraftSchema.parse(draft)));
  return { assignmentFlowId: id };
}

export function isAssignmentDraftReference(state: unknown) {
  return flowReference.safeParse(state).success;
}

/** A return/result may update a live flow, never create a replacement for a consumed one. */
export function updateAssignmentDraftTarget(state: unknown, targetSetId?: string) {
  const reference = flowReference.safeParse(state);
  if (!reference.success) return null;
  const draft = readAssignmentDraft(state);
  if (!draft) return null;
  const next = assignmentDraftSchema.parse({
    ...draft,
    form: { ...draft.form, targetSetId: targetSetId ?? draft.form.targetSetId },
  });
  window.sessionStorage.setItem(storageKey(reference.data.assignmentFlowId), JSON.stringify(next));
  return { state: reference.data, returnHref: next.returnHref };
}

export function readAssignmentDraft(state: unknown) {
  const reference = flowReference.safeParse(state);
  if (!reference.success) return null;
  try {
    const stored = window.sessionStorage.getItem(storageKey(reference.data.assignmentFlowId));
    if (!stored) return null;
    const result = assignmentDraftSchema.safeParse(JSON.parse(stored));
    return result.success ? result.data : null;
  } catch {
    // Unavailable/corrupt session storage must fail closed, never reopen a stale history snapshot.
    return null;
  }
}

export function consumeAssignmentDraft(state: unknown) {
  const reference = flowReference.safeParse(state);
  if (reference.success) window.sessionStorage.removeItem(storageKey(reference.data.assignmentFlowId));
}
