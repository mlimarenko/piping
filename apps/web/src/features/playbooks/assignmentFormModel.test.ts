import { describe, expect, it } from "vitest";
import { assignmentForm, assignmentRequest } from "./assignmentFormModel";

describe("assignment form", () => {
  it("uses the generated job assignment request and rejects invalid boundary input", () => {
    const form = { ...assignmentForm(), jobTemplateId: "11111111-1111-4111-8111-111111111111", targetSetId: "22222222-2222-4222-8222-222222222222", priority: "-4" };
    expect(assignmentRequest(form)).toMatchObject({ job_template_id: form.jobTemplateId, target_set_id: form.targetSetId, scope_kind: "global", scope_id: null, priority: -4 });
    expect(() => assignmentRequest({ ...form, priority: "3.5" })).toThrow();
    expect(() => assignmentRequest({ ...form, scopeKind: "site", scopeId: "" })).toThrow();
    expect(() => assignmentRequest({ ...form, jobTemplateId: "not-a-uuid" })).toThrow();
  });
});
