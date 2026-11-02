import { describe, expect, it } from "vitest";
import { consumeAssignmentDraft, readAssignmentDraft, startAssignmentDraft, updateAssignmentDraftTarget } from "./assignmentDraft";

const draft = { returnHref: "/automation/assignments?enabled=true", editingId: null, form: {
  playbookId: "11111111-1111-4111-8111-111111111111", targetSetId: "old-target", name: "Unsaved metadata", scopeKind: "global" as const, scopeId: "", priority: "75", enabled: true, visible: true, executable: false, quickActionEnabled: false,
} };
describe("single assignment-flow owner", () => {
  it("resolves all serialized history references to one updated owner and consumes them across remount/reload", () => {
    const origin = startAssignmentDraft(draft);
    expect(Object.keys(origin)).toEqual(["assignmentFlowId"]);
    const targetEntry = JSON.parse(JSON.stringify(origin));
    const returnedEntry = updateAssignmentDraftTarget(targetEntry, "new-target")?.state;
    expect(returnedEntry).toEqual(origin);
    expect(readAssignmentDraft(JSON.parse(JSON.stringify(origin)))?.form.targetSetId).toBe("new-target");
    consumeAssignmentDraft(returnedEntry);
    expect(readAssignmentDraft(origin)).toBeNull();
    expect(readAssignmentDraft(targetEntry)).toBeNull();
    expect(readAssignmentDraft(JSON.parse(JSON.stringify(returnedEntry)))).toBeNull();
  });
  it("whitelists assignment metadata and rejects snapshot, malformed, corrupt and external-return inputs", () => {
    const input = { ...draft, unexpectedExecutionPayload: "not-assignment-metadata" };
    const reference = startAssignmentDraft(input);
    expect(readAssignmentDraft(reference)).toEqual(draft);
    expect(window.sessionStorage.getItem(`piping.assignment-flow:${reference.assignmentFlowId}`)).not.toContain("not-assignment-metadata");
    expect(readAssignmentDraft({ assignmentDraft: draft })).toBeNull();
    expect(readAssignmentDraft({ assignmentFlowId: "not-an-id" })).toBeNull();
    window.sessionStorage.setItem(`piping.assignment-flow:${reference.assignmentFlowId}`, "not-json");
    expect(readAssignmentDraft(reference)).toBeNull();
    expect(() => startAssignmentDraft({ ...draft, returnHref: "https://foreign.example" })).toThrow();
    consumeAssignmentDraft(reference);
  });
  it("retains the existing connection-target scope in the assignment contract", () => {
    const reference = startAssignmentDraft({ ...draft, form: { ...draft.form, scopeKind: "connection_target", scopeId: "target-id" } });
    expect(readAssignmentDraft(reference)?.form.scopeKind).toBe("connection_target");
    consumeAssignmentDraft(reference);
  });
  it("updates only the current live owner and cannot replace consumed or unknown references", () => {
    const state = startAssignmentDraft(draft);
    expect(updateAssignmentDraftTarget(state, "created-target")?.state).toEqual(state);
    expect(readAssignmentDraft(state)?.form.targetSetId).toBe("created-target");
    expect(updateAssignmentDraftTarget(state)?.state).toEqual(state);
    expect(readAssignmentDraft(state)?.form.targetSetId).toBe("created-target");
    consumeAssignmentDraft(state);
    expect(updateAssignmentDraftTarget(state, "late-target")).toBeNull();
    expect(readAssignmentDraft(state)).toBeNull();
    expect(updateAssignmentDraftTarget(null, "late-target")).toBeNull();
  });
});
