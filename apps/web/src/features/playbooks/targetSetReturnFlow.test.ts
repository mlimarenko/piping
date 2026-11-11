import { beforeEach, describe, expect, it } from "vitest";
import {
  consumeTargetSetReturnFlow,
  readTargetSetReturnDestination,
  readTargetSetReturnFlow,
  startTargetSetReturnFlow,
  TARGET_SET_RETURN_LABELS,
  updateTargetSetReturnFlowTarget,
} from "./targetSetReturnFlow";

type Draft = { name: string; targetSetId: string };

const draft: Draft = { name: "Unsaved metadata", targetSetId: "old-target" };
const returnHref = "/playbooks/settings/assignments?assignments.enabled=false";
const labelKey = TARGET_SET_RETURN_LABELS.job;

// The consumer owns the draft shape; the flow only keeps it alive and hands it back through this parser.
function parseDraft(value: unknown): Draft | null {
  const candidate = value as { name?: unknown; targetSetId?: unknown } | null;
  return typeof candidate?.name === "string" && typeof candidate?.targetSetId === "string"
    ? { name: candidate.name, targetSetId: candidate.targetSetId }
    : null;
}

describe("single target-set return-flow owner", () => {
  beforeEach(() => window.sessionStorage.clear());

  it("resolves every serialized history reference to one updated owner and consumes them across remount/reload", () => {
    const origin = startTargetSetReturnFlow(returnHref, draft, { labelKey });
    expect(Object.keys(origin)).toEqual(["targetSetReturnFlow"]);
    const targetEntry = JSON.parse(JSON.stringify(origin));
    const returnedEntry = updateTargetSetReturnFlowTarget(targetEntry, "new-target")?.state;
    expect(returnedEntry).toEqual(origin);
    expect(readTargetSetReturnFlow(JSON.parse(JSON.stringify(origin)), parseDraft)?.targetSetId).toBe("new-target");
    consumeTargetSetReturnFlow(returnedEntry);
    expect(readTargetSetReturnFlow(origin, parseDraft)).toBeNull();
    expect(readTargetSetReturnFlow(targetEntry, parseDraft)).toBeNull();
    expect(readTargetSetReturnFlow(JSON.parse(JSON.stringify(returnedEntry)), parseDraft)).toBeNull();
  });

  it("returns only the draft the consumer whitelists and rejects malformed, corrupt and external inputs", () => {
    const input = { ...draft, unexpectedExecutionPayload: "not-consumer-draft" };
    const reference = startTargetSetReturnFlow(returnHref, input, { labelKey });
    expect(readTargetSetReturnFlow(reference, parseDraft)).toEqual({ returnHref, targetSetId: "", draft });
    expect(readTargetSetReturnFlow({ targetSetReturnFlow: draft }, parseDraft)).toBeNull();
    expect(readTargetSetReturnFlow({ targetSetReturnFlow: { flowId: "not-an-id" } }, parseDraft)).toBeNull();
    expect(readTargetSetReturnFlow({ assignmentFlowId: "11111111-1111-4111-8111-111111111111" }, parseDraft)).toBeNull();
    window.sessionStorage.setItem(`piping.target-set-return-flow:${reference.targetSetReturnFlow.flowId}`, "not-json");
    expect(readTargetSetReturnFlow(reference, parseDraft)).toBeNull();
    expect(() => startTargetSetReturnFlow("https://foreign.example", draft)).toThrow();
    expect(() => startTargetSetReturnFlow("/automation/assignments", draft)).toThrow();
    expect(() => startTargetSetReturnFlow("//foreign.example/playbooks", draft)).toThrow();
    expect(() => startTargetSetReturnFlow("/playbooks?tab=assignments", draft)).not.toThrow();
  });

  it("offers the list a return destination only when the flow carries its label", () => {
    const withLabel = startTargetSetReturnFlow(returnHref, draft, { labelKey });
    expect(readTargetSetReturnDestination(withLabel)).toEqual({ returnHref, labelKey });
    const withoutLabel = startTargetSetReturnFlow(returnHref, draft);
    expect(readTargetSetReturnDestination(withoutLabel)).toBeNull();
    expect(readTargetSetReturnFlow(withoutLabel, parseDraft)?.draft).toEqual(draft);
  });

  it("keeps the consumer draft alive across the create page and returns the created set", () => {
    const reference = startTargetSetReturnFlow(returnHref, draft, { labelKey });
    const returned = updateTargetSetReturnFlowTarget(reference, "created-target");
    expect(returned?.returnHref).toBe(returnHref);
    expect(readTargetSetReturnFlow(returned?.state, parseDraft)).toEqual({ returnHref, targetSetId: "created-target", draft });
    expect(updateTargetSetReturnFlowTarget(reference)?.returnHref).toBe(returnHref);
    expect(readTargetSetReturnFlow(reference, parseDraft)?.targetSetId).toBe("created-target");
    expect(updateTargetSetReturnFlowTarget(null, "late-target")).toBeNull();
    consumeTargetSetReturnFlow(reference);
  });

  it("updates only the current live owner and cannot replace consumed or unknown references", () => {
    const state = startTargetSetReturnFlow(returnHref, draft, { labelKey });
    expect(updateTargetSetReturnFlowTarget(state, "created-target")?.state).toEqual(state);
    expect(readTargetSetReturnFlow(state, parseDraft)?.targetSetId).toBe("created-target");
    consumeTargetSetReturnFlow(state);
    expect(updateTargetSetReturnFlowTarget(state, "late-target")).toBeNull();
    expect(readTargetSetReturnFlow(state, parseDraft)).toBeNull();
    expect(updateTargetSetReturnFlowTarget(null, "late-target")).toBeNull();
  });

  it("reuses a live flow id and starts a distinct one after the flow is consumed", () => {
    const first = startTargetSetReturnFlow(returnHref, draft, { labelKey });
    const second = startTargetSetReturnFlow(returnHref, { ...draft, name: "Second attempt" }, { labelKey, state: first });
    expect(second.targetSetReturnFlow.flowId).toBe(first.targetSetReturnFlow.flowId);
    expect(readTargetSetReturnFlow(second, parseDraft)?.draft.name).toBe("Second attempt");
    consumeTargetSetReturnFlow(first);
    const third = startTargetSetReturnFlow(returnHref, draft, { labelKey, state: first });
    expect(third.targetSetReturnFlow.flowId).not.toBe(first.targetSetReturnFlow.flowId);
  });
});
