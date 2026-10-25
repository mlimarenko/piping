import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import type { InventoryTemplate } from "./templateTypes";
import TemplateEditorPreview from "./TemplateEditorPreview";

const api = vi.hoisted(() => ({ previewTemplateRun: vi.fn() }));
vi.mock("@/features/inventory/api", () => api);

const template: InventoryTemplate = {
  id: "saved-template", name: "Sites", slug: "sites", status: "draft", description: "", updatedAt: "",
  variables: [{ key: "name", label: "Site name", value_type: "string", required: true, default_value: "Berlin" }],
  steps: [{ step_key: "site", title: "Create site", resource_type: "site", operation: "create", depends_on_step_keys: [],
    payload_template: { name: "{{name}}" }, on_conflict_policy: "fail" }],
};

describe("TemplateEditorPreview", () => {
  it("previews the current unsaved draft even when it has a persisted ID", () => {
    const view = render(<TemplateEditorPreview template={template} />);
    expect(screen.getByText("Berlin")).toBeInTheDocument();
    view.rerender(<TemplateEditorPreview template={{ ...template,
      variables: [{ ...template.variables[0]!, default_value: "Paris" }],
      steps: [{ ...template.steps[0]!, title: "Edited step", payload_template: { name: "New {{name}}" } }],
    }} />);
    expect(screen.getByText("Edited step")).toBeInTheDocument();
    expect(screen.getByText("New Paris")).toBeInTheDocument();
    expect(screen.queryByText("Berlin")).not.toBeInTheDocument();
    expect(api.previewTemplateRun).not.toHaveBeenCalled();
  });

  it("identifies incomplete inputs and keeps placeholders instead of fabricating null names", () => {
    render(<TemplateEditorPreview template={{ ...template, variables: [{ ...template.variables[0]!, default_value: null }] }} />);
    expect(screen.getByRole("status")).toHaveTextContent("Site name");
    expect(screen.getByText("{{name}}")).toBeInTheDocument();
    expect(screen.queryByText("null")).not.toBeInTheDocument();
  });

  it("reports missing collection row inputs without claiming that an existing repeated step is absent", () => {
    render(<TemplateEditorPreview template={{ ...template,
      variables: [{ key: "rows", label: "Rows", value_type: "json", default_value: [{}], validation_rules: {
        fields: [{ key: "name", label: "Name", value_type: "string", required: true }],
      } }],
      steps: [{ ...template.steps[0]!, payload_template: { name: "{{row.name}}" }, repeat: { for_each: "{{rows}}", item_token: "row" } }],
    }} />);
    expect(screen.getByRole("status")).toHaveTextContent("Rows · row 1 · Name");
    expect(screen.queryByText("Add a step to preview what will be created")).not.toBeInTheDocument();
    expect(screen.getByText("No objects to preview with the current input values.")).toBeInTheDocument();
    expect(api.previewTemplateRun).not.toHaveBeenCalled();
  });
});
