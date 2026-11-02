import type { InventoryTemplate, TemplateStep, TemplateVariable } from "@/shared/api/types";
import { isTemplateSelection } from "./templateTypes";

export interface TemplateVariableRow {
  editorId: string;
  value: TemplateVariable;
  open: boolean;
  structuredDefault: boolean;
}

export interface TemplateStepRow {
  editorId: string;
  value: TemplateStep;
  open: boolean;
}

export interface TemplateEditorDraft {
  details: Omit<InventoryTemplate, "variables" | "steps">;
  variables: TemplateVariableRow[];
  steps: TemplateStepRow[];
}

export const emptyTemplate = (): InventoryTemplate => ({
  id: "", name: "", slug: "", status: "draft", description: "", variables: [], steps: [], updatedAt: "",
});

export const variableRow = (value: TemplateVariable, open = false): TemplateVariableRow => ({
  editorId: crypto.randomUUID(), value, open,
  structuredDefault: value.value_type !== "json" && isTemplateSelection(value.default_value),
});

export const stepRow = (value: TemplateStep, open = false): TemplateStepRow => ({
  editorId: crypto.randomUUID(), value, open,
});

export function createTemplateEditorDraft(initial: InventoryTemplate): TemplateEditorDraft {
  const { variables, steps, ...details } = structuredClone(initial);
  return { details, variables: variables.map(value => variableRow(value)), steps: steps.map(value => stepRow(value)) };
}

export function templateFromEditorDraft(draft: TemplateEditorDraft): InventoryTemplate {
  return { ...draft.details, variables: draft.variables.map(row => row.value), steps: draft.steps.map(row => row.value) };
}

/** Auto-binding updates step values and only appends variables; row identity stays session-owned. */
export function applyAutoBoundReferences(
  draft: TemplateEditorDraft,
  bound: { variables: TemplateVariable[]; steps: TemplateStep[] },
): TemplateEditorDraft {
  return {
    ...draft,
    variables: bound.variables.map((value, index) => draft.variables[index]
      ? { ...draft.variables[index], value } : variableRow(value)),
    steps: draft.steps.map((row, index) => ({ ...row, value: bound.steps[index] ?? row.value })),
  };
}

export function moveEditorRow<T>(rows: readonly T[], from: number, to: number): T[] {
  if (from === to || !Number.isInteger(from) || !Number.isInteger(to) || from < 0 || to < 0 || from >= rows.length || to >= rows.length) return [...rows];
  const next = [...rows];
  const [row] = next.splice(from, 1);
  next.splice(to, 0, row!);
  return next;
}
