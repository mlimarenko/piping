import { z } from "zod";
import { isUuid } from "@/shared/lib/ids";
import { isTargetSetReturnHref } from "./playbookEditorRoutes";

/**
 * Session-scoped owner of every "leave an editor for the target-set surface and
 * come back" path: opening the target-set list, and creating a set from that
 * list or directly from the editor.
 *
 * History carries only a flow reference; the consumer draft lives once in
 * session storage under that flow id. A consumed flow resolves to nothing, so a
 * stale or replayed history entry cannot revive an old draft.
 */
export const TARGET_SET_RETURN_FLOW_STATE_KEY = "targetSetReturnFlow";

const STORAGE_PREFIX = "piping.target-set-return-flow";

export const TARGET_SET_RETURN_LABELS = {
  job: "playbooks.targetSetReturn.toJob",
  schedule: "playbooks.targetSetReturn.toSchedule",
  inventory: "playbooks.targetSetReturn.toInventory",
} as const;

export type TargetSetReturnLabelKey = (typeof TARGET_SET_RETURN_LABELS)[keyof typeof TARGET_SET_RETURN_LABELS];

const labelKeys = Object.values(TARGET_SET_RETURN_LABELS) as [TargetSetReturnLabelKey, ...TargetSetReturnLabelKey[]];

const referenceSchema = z.object({
  [TARGET_SET_RETURN_FLOW_STATE_KEY]: z.object({ flowId: z.string().refine(isUuid) }),
});

const envelopeSchema = z.object({
  returnHref: z.string().max(2000).refine(isTargetSetReturnHref),
  labelKey: z.enum(labelKeys).optional(),
  targetSetId: z.string(),
  draft: z.unknown(),
  initialSelector: z.record(z.string(), z.unknown()).optional(),
});

// The draft shape belongs to its consumer; the envelope owns the return
// destination, its label and the id of the set the editor created.
type FlowEnvelope = z.infer<typeof envelopeSchema>;
type FlowReference = { targetSetReturnFlow: { flowId: string } };

export type TargetSetReturnFlow<T> = {
  returnHref: string;
  targetSetId: string;
  draft: T;
};

export type TargetSetReturnDestination = {
  returnHref: string;
  labelKey: TargetSetReturnLabelKey;
};

export type StartTargetSetReturnFlowOptions = {
  /** Present when the target-set list can offer a labelled return action. */
  labelKey?: TargetSetReturnLabelKey;
  /** The history state this navigation replaces, so one draft keeps one owner. */
  state?: unknown;
  initialSelector?: Record<string, unknown>;
};

function storageKey(flowId: string): string {
  return `${STORAGE_PREFIX}:${flowId}`;
}

function readReference(state: unknown): FlowReference | null {
  const parsed = referenceSchema.safeParse(state);
  return parsed.success ? parsed.data : null;
}

function readEnvelope(state: unknown): FlowEnvelope | null {
  const reference = readReference(state);
  if (!reference) return null;
  try {
    const stored = window.sessionStorage.getItem(storageKey(reference[TARGET_SET_RETURN_FLOW_STATE_KEY].flowId));
    if (!stored) return null;
    const parsed = envelopeSchema.safeParse(JSON.parse(stored));
    return parsed.success ? parsed.data : null;
  } catch {
    // Unavailable or corrupt session storage must fail closed, never reopen a stale draft.
    return null;
  }
}

/** Starts the flow, reusing a live flow id so one draft never gets two owners. */
export function startTargetSetReturnFlow(
  returnHref: string,
  draft: unknown,
  { labelKey, state, initialSelector }: StartTargetSetReturnFlowOptions = {},
): FlowReference {
  if (!isTargetSetReturnHref(returnHref)) {
    throw new Error("Invalid target-set return destination.");
  }
  const reference = readReference(state);
  const flowId =
    reference && readEnvelope(reference)
      ? reference[TARGET_SET_RETURN_FLOW_STATE_KEY].flowId
      : crypto.randomUUID();
  const envelope = envelopeSchema.parse({ returnHref, labelKey, targetSetId: "", draft, initialSelector });
  window.sessionStorage.setItem(storageKey(flowId), JSON.stringify(envelope));
  return { [TARGET_SET_RETURN_FLOW_STATE_KEY]: { flowId } };
}

export function isTargetSetReturnFlowReference(state: unknown): boolean {
  return readReference(state) !== null;
}

export function readTargetSetInitialSelector(state: unknown): Record<string, unknown> | undefined {
  return readEnvelope(state)?.initialSelector;
}

/** The live flow and its draft, parsed by the consumer, or null for unknown, consumed or corrupt state. */
export function readTargetSetReturnFlow<T>(
  state: unknown,
  parseDraft: (draft: unknown) => T | null,
): TargetSetReturnFlow<T> | null {
  const envelope = readEnvelope(state);
  if (!envelope) return null;
  const draft = parseDraft(envelope.draft);
  return draft === null
    ? null
    : { returnHref: envelope.returnHref, targetSetId: envelope.targetSetId, draft };
}

/** Return destination for the target-set list, which never needs the consumer draft. */
export function readTargetSetReturnDestination(state: unknown): TargetSetReturnDestination | null {
  const envelope = readEnvelope(state);
  return envelope?.labelKey ? { returnHref: envelope.returnHref, labelKey: envelope.labelKey } : null;
}

/** Records the created set on a live flow; a consumed reference is never replaced. */
export function updateTargetSetReturnFlowTarget(
  state: unknown,
  targetSetId?: string,
): { state: FlowReference; returnHref: string } | null {
  const reference = readReference(state);
  if (!reference) return null;
  const envelope = readEnvelope(reference);
  if (!envelope) return null;
  const next = envelopeSchema.parse({
    ...envelope,
    targetSetId: targetSetId ?? envelope.targetSetId,
  });
  window.sessionStorage.setItem(
    storageKey(reference[TARGET_SET_RETURN_FLOW_STATE_KEY].flowId),
    JSON.stringify(next),
  );
  return { state: reference, returnHref: next.returnHref };
}

export function consumeTargetSetReturnFlow(state: unknown): void {
  const reference = readReference(state);
  if (reference) {
    window.sessionStorage.removeItem(storageKey(reference[TARGET_SET_RETURN_FLOW_STATE_KEY].flowId));
  }
}
