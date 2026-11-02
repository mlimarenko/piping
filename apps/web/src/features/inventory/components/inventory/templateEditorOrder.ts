import type { TemplateStep } from "@/shared/api/types";

export interface TemplateStepOrderViolation {
  consumerIndex: number;
  consumerKey: string;
  producerKey: string;
  reason: "producerAfterConsumer" | "missingProducer" | "selfReference";
}

export interface TemplateStepMoveResult<T extends TemplateStep = TemplateStep> {
  steps: T[];
  violations: TemplateStepOrderViolation[];
  blockingViolation: TemplateStepOrderViolation | null;
}

function producerReferences(step: TemplateStep): Set<string> {
  const keys = new Set(step.depends_on_step_keys);
  const visit = (value: unknown): void => {
    if (typeof value === "string") {
      for (const token of value.matchAll(/\{\{([^{}]*)\}\}/g)) {
        // Only step output tokens, not prose mentioning steps or expressions.
        const body = token[1];
        if (body === undefined) continue;
        const reference = /^steps\.([^\s.{}]+)\.(?:(?:current|\d+)\.)?ids?$/.exec(body.trim());
        const producerKey = reference?.[1];
        if (producerKey !== undefined) keys.add(producerKey);
      }
    } else if (Array.isArray(value)) {
      value.forEach(visit);
    } else if (value && typeof value === "object") {
      Object.values(value).forEach(visit);
    }
  };
  visit(step.payload_template);
  visit(step.repeat);
  return keys;
}

/** Report unresolved references as well as order errors; never normalize editable keys. */
export function analyzeTemplateStepOrder(steps: readonly TemplateStep[]): TemplateStepOrderViolation[] {
  const firstProducerIndex = new Map<string, number>();
  steps.forEach((step, index) => {
    // Duplicate-key validation belongs to the schema. Use the first producer
    // here so an unfinished duplicate edit cannot crash or overwrite an earlier row.
    if (!firstProducerIndex.has(step.step_key)) firstProducerIndex.set(step.step_key, index);
  });
  return steps.flatMap((step, consumerIndex) => {
    const violations: TemplateStepOrderViolation[] = [];
    for (const producerKey of producerReferences(step)) {
      const producerIndex = firstProducerIndex.get(producerKey);
      const reason = producerKey === step.step_key
        ? "selfReference"
        : producerIndex === undefined
          ? "missingProducer"
          : producerIndex > consumerIndex
            ? "producerAfterConsumer"
            : null;
      if (reason) violations.push({ consumerIndex, consumerKey: step.step_key, producerKey, reason });
    }
    return violations;
  });
}

/** Reject only new ordering pairs, allowing existing invalid templates to be repaired. */
export function moveTemplateStep<T extends TemplateStep>(steps: T[], from: number, to: number): TemplateStepMoveResult<T> {
  const violations = analyzeTemplateStepOrder(steps);
  if (!Number.isInteger(from) || !Number.isInteger(to) || from < 0 || to < 0 || from >= steps.length || to >= steps.length || from === to) {
    return { steps, violations, blockingViolation: null };
  }

  const rows = steps.map((step, originalIndex) => ({ step, originalIndex }));
  const moved = rows.splice(from, 1)[0];
  // The bounds above guarantee a row for ordinary arrays; keep the boundary
  // fail-safe if an unusual sparse input does not supply one.
  if (moved === undefined) return { steps, violations, blockingViolation: null };
  rows.splice(to, 0, moved);
  const proposed = rows.map(({ step }) => step);
  const proposedViolations = analyzeTemplateStepOrder(proposed);
  const existingPairs = new Set(violations
    .filter((violation) => violation.reason === "producerAfterConsumer")
    .map((violation) => JSON.stringify([violation.consumerIndex, violation.producerKey])));
  const blockingViolation = proposedViolations.find((violation) =>
    violation.reason === "producerAfterConsumer"
    && !existingPairs.has(JSON.stringify([rows[violation.consumerIndex]?.originalIndex, violation.producerKey]))) ?? null;

  return blockingViolation
    ? { steps, violations, blockingViolation }
    : { steps: proposed, violations: proposedViolations, blockingViolation: null };
}
