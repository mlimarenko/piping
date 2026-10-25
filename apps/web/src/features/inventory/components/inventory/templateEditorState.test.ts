import { describe, expect, it } from "vitest";
import { createTemplateEditorDraft, templateFromEditorDraft, moveEditorRow, applyAutoBoundReferences } from "./templateEditorState";
import type { InventoryTemplate } from "./templateTypes";

const initial: InventoryTemplate = {
  id: "saved", name: "Draft", slug: "draft", status: "draft", description: "", updatedAt: "",
  variables: [
    { key: "", label: "", value_type: "string", default_value: { resource: "site", id: "00000000-0000-4000-8000-000000000001" } },
    { key: "", label: "", value_type: "string", default_value: "Plain default" },
  ],
  steps: [],
};

describe("template editor draft identity", () => {
  it("moves values, open state and structured-default intent together, independent of editable keys", () => {
    const draft = createTemplateEditorDraft(initial);
    draft.variables[0]!.open = true;
    draft.variables[0]!.value.default_value = "{";
    const moved = { ...draft, variables: moveEditorRow(draft.variables, 0, 1) };
    expect(moved.variables.map(row => row.editorId)).toEqual([draft.variables[1]!.editorId, draft.variables[0]!.editorId]);
    expect(moved.variables[1]).toMatchObject({ open: true, structuredDefault: true, value: { default_value: "{" } });
    expect(moved.variables[0]).toMatchObject({ open: false, structuredDefault: false, value: { default_value: "Plain default" } });
    const serialized = templateFromEditorDraft(moved);
    expect(serialized.variables).toEqual([initial.variables[1], { ...initial.variables[0], default_value: "{" }]);
    expect(serialized.variables.every(value => !("editorId" in value) && !("open" in value) && !("structuredDefault" in value))).toBe(true);
    expect(initial.variables[0]!.default_value).toEqual({ resource: "site", id: "00000000-0000-4000-8000-000000000001" });
  });

  it("adopts appended auto-bound variables without replacing the identity or intent of existing rows", () => {
    const draft = createTemplateEditorDraft(initial);
    const bound = applyAutoBoundReferences(draft, {
      variables: [...templateFromEditorDraft(draft).variables, { key: "site_id", label: "Site", value_type: "resource_ref" }], steps: [],
    });
    expect(bound.variables.slice(0, 2).map(row => row.editorId)).toEqual(draft.variables.map(row => row.editorId));
    expect(bound.variables[2]!.editorId).not.toBe(draft.variables[0]!.editorId);
    expect(templateFromEditorDraft({ ...draft, variables: draft.variables.map(row => ({ ...row, open: true })) })).toEqual(initial);
  });
});
