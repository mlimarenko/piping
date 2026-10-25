import { describe, expect, it } from "vitest";

import type { TemplateStep } from "@/shared/api/types";

import { analyzeTemplateStepOrder, moveTemplateStep } from "./templateEditorOrder";

function step(key: string, overrides: Partial<TemplateStep> = {}): TemplateStep {
  return {
    step_key: key,
    title: key,
    resource_type: "site",
    operation: "create",
    depends_on_step_keys: [],
    payload_template: {},
    on_conflict_policy: "fail",
    ...overrides,
  };
}

describe("template editor sequential ordering", () => {
  it("moves independent rows without changing rows, references, repeat or sort_order", () => {
    const producer = step("source", { sort_order: 7 });
    const consumer = step("consumer", {
      depends_on_step_keys: ["source"],
      payload_template: { nested: [{ id: "{{steps.source.current.id}}" }] },
      repeat: { for_each: "{{steps.source.ids}}" },
    });
    const steps = [producer, consumer, step("independent")];
    const before = structuredClone(steps);
    steps.forEach(Object.freeze);
    Object.freeze(steps);
    const result = moveTemplateStep(steps, 2, 0);
    expect(result.blockingViolation).toBeNull();
    expect(result.violations).toEqual([]);
    expect(result.steps).toEqual([steps[2], producer, consumer]);
    expect(result.steps[1]).toBe(producer);
    expect(result.steps[2]).toBe(consumer);
    expect(result.steps[2]?.payload_template).toBe(consumer.payload_template);
    expect(result.steps[2]?.repeat).toBe(consumer.repeat);
    expect(steps).toEqual(before);
  });

  it.each([[0, 1], [1, 0]])("rejects crossing an explicit dependency (%i → %i)", (from, to) => {
    const steps = [step("source"), step("consumer", { depends_on_step_keys: ["source"] })];
    const result = moveTemplateStep(steps, from, to);
    expect(result.steps).toBe(steps);
    expect(result.blockingViolation).toEqual({ consumerIndex: 0, consumerKey: "consumer", producerKey: "source", reason: "producerAfterConsumer" });
    expect(result.violations).toEqual([]);
  });

  it.each(["id", "ids", "12.id", "12.ids", "current.id", "current.ids"])("finds nested payload token %s", (suffix) => {
    const steps = [step("source"), step("consumer", {
      payload_template: { collection: [{ nested: `prefix {{steps.source.${suffix}}} suffix` }] },
    })];
    expect(moveTemplateStep(steps, 0, 1).blockingViolation?.producerKey).toBe("source");
  });

  it.each([
    { for_each: "{{steps.source.ids}}", item_token: "item" },
    { count: "{{ steps.source.id }}", start: 1 },
    { for_each: [{ nested: ["{{steps.source.current.id}}"] }] },
  ])("finds recursive repeat references %j", (repeat) => {
    const steps = [step("source"), step("consumer", { repeat })];
    expect(moveTemplateStep(steps, 1, 0).blockingViolation?.producerKey).toBe("source");
  });

  it("ignores prose, non-step tokens and unsupported expressions", () => {
    const consumer = step("consumer", { payload_template: {
      text: ["steps.source.id", "{{item.steps.source.id}}", "{{steps.source.id + 1}}", "{{steps.source.name}}", "{{steps.source.id", "source.id}}"],
    } });
    expect(analyzeTemplateStepOrder([consumer, step("source")])).toEqual([]);
  });

  it("allows preserving or repairing existing violations but not trading them for new pairs", () => {
    const a = step("a", { depends_on_step_keys: ["b"] });
    const b = step("b");
    const c = step("c", { depends_on_step_keys: ["b"] });
    const steps = [a, b, c, step("independent")];
    const preserved = moveTemplateStep(steps, 3, 0);
    expect(preserved.blockingViolation).toBeNull();
    expect(preserved.violations).toMatchObject([{ consumerIndex: 1, producerKey: "b" }]);
    const repaired = moveTemplateStep(steps, 1, 0);
    expect(repaired.violations).toEqual([]);
    expect(repaired.steps).toEqual([b, a, c, steps[3]]);
    const traded = moveTemplateStep(steps, 1, 2);
    expect(traded.steps).toBe(steps);
    expect(traded.blockingViolation?.consumerKey).toBe("c");
  });

  it("reports cycles, self references and missing producers without letting them block independent repairs", () => {
    const steps = [
      step("a", { depends_on_step_keys: ["b", "missing", "a"], payload_template: { id: "{{steps.unknown.id}}" } }),
      step("b", { depends_on_step_keys: ["a"] }),
      step("independent"),
    ];
    expect(analyzeTemplateStepOrder(steps)).toEqual([
      { consumerIndex: 0, consumerKey: "a", producerKey: "b", reason: "producerAfterConsumer" },
      { consumerIndex: 0, consumerKey: "a", producerKey: "missing", reason: "missingProducer" },
      { consumerIndex: 0, consumerKey: "a", producerKey: "a", reason: "selfReference" },
      { consumerIndex: 0, consumerKey: "a", producerKey: "unknown", reason: "missingProducer" },
    ]);
    expect(moveTemplateStep(steps, 2, 0).blockingViolation).toBeNull();
    expect(moveTemplateStep(steps, 0, 1).blockingViolation?.consumerKey).toBe("b");
  });

  it("deduplicates reference sources and preserves exact editable keys", () => {
    const steps = [step("consumer", {
      depends_on_step_keys: ["Source-X", "Source-X"],
      payload_template: { id: "{{steps.Source-X.id}}" },
      repeat: { for_each: "{{steps.Source-X.ids}}" },
    }), step("Source-X")];
    expect(analyzeTemplateStepOrder(steps)).toEqual([
      { consumerIndex: 0, consumerKey: "consumer", producerKey: "Source-X", reason: "producerAfterConsumer" },
    ]);
  });

  it("handles empty and duplicate editable keys without collapsing consumer rows", () => {
    expect(analyzeTemplateStepOrder([])).toEqual([]);
    const steps = [step(""), step("same"), step("same", { depends_on_step_keys: ["source"] }), step("source")];
    expect(moveTemplateStep(steps, 2, 0).blockingViolation).toBeNull();
    expect(moveTemplateStep(steps, 3, 0).violations).toEqual([]);
    expect(analyzeTemplateStepOrder([step("same"), step("same"), step("consumer", { depends_on_step_keys: ["same"] })])).toEqual([]);
    const duplicates = [
      step("same", { depends_on_step_keys: ["source"] }),
      step("source"),
      step("same", { depends_on_step_keys: ["source"] }),
    ];
    // An existing violation on one duplicate must not excuse a new one on another.
    expect(moveTemplateStep(duplicates, 1, 2).blockingViolation?.consumerIndex).toBe(1);
    expect(moveTemplateStep(duplicates, 1, 2).steps).toBe(duplicates);
  });

  it.each([[0, 0], [-1, 0], [0, -1], [0, 2], [2, 0], [0.5, 0], [NaN, 0], [0, Infinity]])("treats invalid or unchanged move %j → %j as a no-op", (from, to) => {
    const steps = [step("a"), step("b")];
    const result = moveTemplateStep(steps, from, to);
    expect(result.steps).toBe(steps);
    expect(result.blockingViolation).toBeNull();
  });
});
