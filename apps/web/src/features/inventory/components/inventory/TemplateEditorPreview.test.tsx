import { act, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import type { InventoryTemplate } from "./templateTypes";
import TemplateEditorPreview from "./TemplateEditorPreview";

const api = vi.hoisted(() => ({ previewTemplateRun: vi.fn(), listAllResources: vi.fn() }));
vi.mock("@/features/inventory/api", () => api);

const template: InventoryTemplate = {
  id: "saved-template", name: "Sites", slug: "sites", status: "draft", description: "", updatedAt: "",
  variables: [{ key: "name", label: "Site name", value_type: "string", required: true, default_value: "Berlin" }],
  steps: [{ step_key: "site", title: "Create site", resource_type: "site", operation: "create", depends_on_step_keys: [],
    payload_template: { name: "{{name}}" }, on_conflict_policy: "fail" }],
};

const referenceDraft = (rules: Record<string, unknown>, value?: unknown): InventoryTemplate => ({ ...template,
  variables: [{ key: "role", label: "Role", value_type: "resource_ref", required: true, default_value: value, validation_rules: { resource: "device_role", ...rules } }],
  steps: [{ ...template.steps[0]!, resource_type: "device", payload_template: { role_id: "{{role}}" } }],
});

const renderPreview = (ui: React.ReactElement) => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(ui, { wrapper: ({ children }) => <QueryClientProvider client={client}>{children}</QueryClientProvider> });
};

describe("TemplateEditorPreview", () => {
  it("resolves configured reference defaults in the unsaved draft", async () => {
    const id = "00000000-0000-4000-8000-000000000001";
    api.listAllResources.mockResolvedValue([{ id, name: "Core", type: "device_role" }]);
    const draft: InventoryTemplate = { ...template,
      variables: [{ key: "role", label: "Role", value_type: "resource_ref", required: true, validation_rules: { resource: "device_role", default_name: "Core" } }],
      steps: [{ ...template.steps[0]!, resource_type: "device", payload_template: { role_id: "{{role}}" } }],
    };
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<QueryClientProvider client={client}><TemplateEditorPreview template={draft} /></QueryClientProvider>);
    expect(await screen.findByText(new RegExp(id), {}, { timeout: 1500 })).toBeInTheDocument();
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it.each([
    ["ambiguous", [{ id: "00000000-0000-4000-8000-000000000001", name: "Core", type: "device_role" }, { id: "00000000-0000-4000-8000-000000000002", name: "Core", type: "device_role" }]],
    ["missing", []],
  ])("marks %s configured references missing", async (_label, options) => {
    api.listAllResources.mockResolvedValue(options);
    renderPreview(<TemplateEditorPreview template={referenceDraft({ default_name: "Core" })} />);
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Role"), { timeout: 1500 });
    expect(screen.getByText("{{role}}")).toBeInTheDocument();
  });

  it("does not replace an unavailable explicit identity with a matching name", async () => {
    api.listAllResources.mockResolvedValue([{ id: "00000000-0000-4000-8000-000000000002", name: "Core", type: "device_role" }]);
    renderPreview(<TemplateEditorPreview template={referenceDraft({ default_id: "00000000-0000-4000-8000-000000000001", default_name: "Core" })} />);
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Role"), { timeout: 1500 });
    expect(screen.getByText("{{role}}")).toBeInTheDocument();
  });

  it.each([
    ["text default", "Core"],
    ["structured default", { resource: "device_role", id: "00000000-0000-4000-8000-000000000001" }],
  ])("resolves a %s", async (_label, value) => {
    api.listAllResources.mockResolvedValue([{ id: "00000000-0000-4000-8000-000000000001", name: "Core", type: "device_role" }]);
    renderPreview(<TemplateEditorPreview template={referenceDraft({}, value)} />);
    expect(await screen.findByText(/00000000-0000-4000-8000-000000000001/, {}, { timeout: 1500 })).toBeInTheDocument();
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it("marks an invalid explicit identity missing", async () => {
    api.listAllResources.mockResolvedValue([{ id: "00000000-0000-4000-8000-000000000001", name: "Core", type: "device_role" }]);
    renderPreview(<TemplateEditorPreview template={referenceDraft({ default_id: "not-a-uuid", default_name: "Core" })} />);
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Role"), { timeout: 1500 });
    expect(screen.getByText("{{role}}")).toBeInTheDocument();
  });

  it("keeps name-bound scalar text without inventing an identity", () => {
    api.listAllResources.mockRejectedValue(new Error("unavailable"));
    const draft = referenceDraft({}, "Core");
    draft.variables[0]!.value_type = "string";
    draft.steps[0]!.payload_template = { role: "{{role}}" };
    renderPreview(<TemplateEditorPreview template={draft} />);
    expect(screen.getByText("Core")).toBeInTheDocument();
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it("shows loading, then a lookup failure as missing", async () => {
    let reject!: (error: Error) => void;
    api.listAllResources.mockReturnValue(new Promise((_resolve, fail) => { reject = fail; }));
    renderPreview(<TemplateEditorPreview template={referenceDraft({ default_name: "Core" })} />);
    expect(screen.getByRole("status")).toHaveTextContent("Loading...");
    await act(async () => { reject(new Error("unavailable")); await Promise.resolve(); });
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Role"), { timeout: 1500 });
  });

  it("does not report an empty optional reference when lookup fails", async () => {
    api.listAllResources.mockRejectedValue(new Error("unavailable"));
    const draft = referenceDraft({});
    draft.variables[0]!.required = false;
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<QueryClientProvider client={client}><TemplateEditorPreview template={draft} /></QueryClientProvider>);
    await waitFor(() => expect(client.getQueryState(["inventory", "resources", "Device Roles", "options"])?.status).toBe("error"), { timeout: 1500 });
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it("reports an optional configured reference when lookup fails", async () => {
    api.listAllResources.mockRejectedValue(new Error("unavailable"));
    const draft = referenceDraft({ default_name: "Core" });
    draft.variables[0]!.required = false;
    renderPreview(<TemplateEditorPreview template={draft} />);
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Role"), { timeout: 1500 });
    expect(screen.getByText("{{role}}")).toBeInTheDocument();
  });

  it("uses the latest draft when an earlier lookup completes", async () => {
    let resolve!: (value: unknown[]) => void;
    api.listAllResources.mockReturnValue(new Promise(done => { resolve = done; }));
    const view = renderPreview(<TemplateEditorPreview template={referenceDraft({ default_name: "Core" })} />);
    view.rerender(<TemplateEditorPreview template={referenceDraft({ default_name: "Edge" })} />);
    await act(async () => { resolve([{ id: "00000000-0000-4000-8000-000000000001", name: "Core", type: "device_role" }, { id: "00000000-0000-4000-8000-000000000002", name: "Edge", type: "device_role" }]); await Promise.resolve(); });
    expect(await screen.findByText(/00000000-0000-4000-8000-000000000002/, {}, { timeout: 1500 })).toBeInTheDocument();
    expect(screen.queryByText(/00000000-0000-4000-8000-000000000001/)).not.toBeInTheDocument();
  });
  it("previews the current unsaved draft even when it has a persisted ID", () => {
    const view = renderPreview(<TemplateEditorPreview template={template} />);
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
    renderPreview(<TemplateEditorPreview template={{ ...template, variables: [{ ...template.variables[0]!, default_value: null }] }} />);
    expect(screen.getByRole("status")).toHaveTextContent("Site name");
    expect(screen.getByText("{{name}}")).toBeInTheDocument();
    expect(screen.queryByText("null")).not.toBeInTheDocument();
  });

  it("reports missing collection row inputs without claiming that an existing repeated step is absent", () => {
    renderPreview(<TemplateEditorPreview template={{ ...template,
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
