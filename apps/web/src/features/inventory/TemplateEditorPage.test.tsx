import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { PermissionsContext } from "@/shared/auth/permissionsContext";
import "@/shared/i18n";
import TemplateEditorPage from "./TemplateEditorPage";
import { TooltipProvider } from "@/shared/ui-primitives/tooltip";
import type { InventoryTemplate } from "@/shared/api/types";

const api = vi.hoisted(() => ({ getTemplate: vi.fn(), createTemplate: vi.fn(), updateTemplate: vi.fn(), previewTemplateRun: vi.fn() }));
vi.mock("@/features/inventory/api", async (original) => ({ ...(await original()), ...api }));
const notifications = vi.hoisted(() => ({ notify: vi.fn() }));
vi.mock("@/shared/lib/notifications", () => notifications);

const selectTab = (tab: HTMLElement) => fireEvent.mouseDown(tab, { button: 0, ctrlKey: false });

function renderEditor(path: string, writable: boolean) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const view = render(<QueryClientProvider client={client}>
    <PermissionsContext.Provider value={{ roles: [], isAdmin: false, can: action => action === "READ" || writable, canAny: () => writable, hasRole: () => false }}>
      <TooltipProvider><MemoryRouter initialEntries={[path]}><Routes>
        <Route path="/inventory/templates/new" element={<TemplateEditorPage />} />
        <Route path="/inventory/templates/:id/edit" element={<TemplateEditorPage />} />
        <Route path="/inventory/templates" element={<div>template-list</div>} />
      </Routes></MemoryRouter></TooltipProvider>
    </PermissionsContext.Provider>
  </QueryClientProvider>);
  return { ...view, client };
}

describe("TemplateEditorPage capabilities", () => {
  it("keeps one editor and heading from pending to loaded", async () => {
    let finish!: (value: unknown) => void;
    api.getTemplate.mockReturnValue(new Promise(resolve => { finish = resolve; }));
    renderEditor("/inventory/templates/tpl-1/edit", true);
    expect(screen.getByRole("status")).toHaveTextContent("Loading");
    expect(screen.getAllByTestId("entity-editor-page")).toHaveLength(1);
    expect(screen.queryByRole("button", { name: "Save Changes" })).not.toBeInTheDocument();
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    finish({ id: "tpl-1", name: "Loaded template", slug: "loaded", status: "draft", description: "", updatedAt: "", variables: [], steps: [] });
    await screen.findByRole("heading", { level: 1, name: "Loaded template" });
    expect(screen.getAllByTestId("entity-editor-page")).toHaveLength(1);
    expect(screen.getByRole("button", { name: "Save Changes" })).toBeInTheDocument();
  });

  it.each(["error", "missing"])("owns %s chrome with retry and preserves read-only access after recovery", async state => {
    if (state === "error") api.getTemplate.mockRejectedValueOnce(new Error("private backend detail"));
    else api.getTemplate.mockResolvedValueOnce(null);
    api.getTemplate.mockResolvedValue({ id: "tpl-1", name: "Recovered template", slug: "loaded", status: "draft", description: "", updatedAt: "", variables: [], steps: [] });
    renderEditor("/inventory/templates/tpl-1/edit", false);
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not load this template");
    expect(screen.queryByText("private backend detail")).not.toBeInTheDocument();
    expect(screen.getAllByTestId("entity-editor-page")).toHaveLength(1);
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(screen.queryByRole("button", { name: "Save Changes" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /retry/i }));
    await screen.findByRole("heading", { level: 1, name: "Recovered template" });
    expect(screen.getAllByTestId("entity-editor-page")).toHaveLength(1);
    expect(screen.queryByRole("button", { name: "Save Changes" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(await screen.findByText("template-list")).toBeInTheDocument();
  });

  it.each(["edit", "clear", "malformed", "invalid UUID", "wrong target", "repair", "literal"])("edits string-typed reference defaults using their existing representation: %s", async (action) => {
    const selected = { resource: "device_role", id: "00000000-0000-4000-8000-000000000001" };
    const edited = { ...selected, id: "00000000-0000-4000-8000-000000000002" };
    const literal = JSON.stringify(selected);
    api.getTemplate.mockResolvedValue({ id: "tpl-1", name: "Roles", slug: "roles", status: "draft", description: "", updatedAt: "",
      variables: [{ key: "role", label: "Role choice", value_type: "string", default_value: action === "literal" ? literal : selected }],
      steps: [{ step_key: "device", title: "Device", resource_type: "device", operation: "create", depends_on_step_keys: [], payload_template: { name: "Switch", site_id: "site-id", role: "{{role}}", role_id: "role-id", device_type_id: "type-id" }, on_conflict_policy: "fail" }] });
    api.updateTemplate.mockResolvedValue({});
    renderEditor("/inventory/templates/tpl-1/edit", true);
    selectTab(await screen.findByRole("tab", { name: /^Variables/ }));
    fireEvent.click(await screen.findByRole("button", { name: /role.*Role choice/i }));
    const input = screen.getByRole("textbox", { name: "Default" });
    if (action === "repair") {
      fireEvent.change(input, { target: { value: "{" } });
      fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));
      await waitFor(() => expect(input).toHaveAttribute("aria-invalid", "true"));
      expect(api.updateTemplate).not.toHaveBeenCalled();
    }
    const next = action === "invalid UUID" ? { ...edited, id: "not-a-uuid" }
      : action === "wrong target" ? { ...edited, resource: "device_type" } : edited;
    fireEvent.change(input, { target: { value: action === "clear" ? "" : action === "malformed" ? "{" : JSON.stringify(next) } });
    fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));
    if (["malformed", "invalid UUID", "wrong target"].includes(action)) {
      await waitFor(() => expect(input).toHaveAttribute("aria-invalid", "true"));
      expect(api.updateTemplate).not.toHaveBeenCalled();
      return;
    }
    await waitFor(() => expect(api.updateTemplate).toHaveBeenCalledWith("tpl-1", expect.objectContaining({ variables: [expect.objectContaining({
      value_type: "string", default_value: action === "clear" ? undefined : action === "literal" ? JSON.stringify(edited) : edited,
    })] })));
  });

  it.each(["preserve", "edit", "clear", "invalid", "wrong target", "extra fields", "malformed JSON"])("round-trips structured reference defaults on editor save: %s", async (action) => {
    const selected = { resource: "device_type", id: "00000000-0000-4000-8000-000000000001" };
    const edited = { ...selected, id: "00000000-0000-4000-8000-000000000002" };
    api.getTemplate.mockResolvedValue({ id: "tpl-1", name: "Types", slug: "types", status: "draft", description: "", updatedAt: "",
      variables: [{ key: "type", label: "Type choice", value_type: "resource_ref", default_value: selected, validation_rules: { resource: "device_type" } }],
      steps: [{ step_key: "device", title: "Device", resource_type: "device", operation: "create", depends_on_step_keys: [], payload_template: { name: "Switch", site_id: "site-id", role_id: "role-id", device_type_id: "{{type}}" }, on_conflict_policy: "fail" }] });
    api.updateTemplate.mockResolvedValue({});
    renderEditor("/inventory/templates/tpl-1/edit", true);
    selectTab(await screen.findByRole("tab", { name: /^Variables/ }));
    fireEvent.click(await screen.findByRole("button", { name: /type.*Type choice/i }));
    const input = screen.getByRole("textbox", { name: "Default" });
    expect(input).toHaveValue(JSON.stringify(selected));
    if (action === "preserve") {
      fireEvent.change(screen.getByRole("textbox", { name: "Help text" }), { target: { value: "Edited help" } });
    } else {
      const next = action === "edit" ? edited : action === "wrong target" ? { ...selected, resource: "device_role" }
        : action === "extra fields" ? { ...selected, name: "Extra" } : { ...selected, id: "not-a-uuid" };
      fireEvent.change(input, { target: { value: action === "clear" ? "" : action === "malformed JSON" ? "{" : JSON.stringify(next) } });
    }
    fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));
    if (!["preserve", "edit", "clear"].includes(action)) {
      await waitFor(() => expect(input).toHaveAttribute("aria-invalid", "true"));
      expect(api.updateTemplate).not.toHaveBeenCalled();
      return;
    }
    await waitFor(() => expect(api.updateTemplate).toHaveBeenCalledWith("tpl-1", expect.objectContaining({ variables: [expect.objectContaining({
      default_value: action === "clear" ? undefined : action === "edit" ? edited : selected,
    })] })));
  });

  it("derives a bound reference kind read-only and reconciles its saved declaration", async () => {
    api.getTemplate.mockResolvedValue({ id: "tpl-1", name: "Roles", slug: "roles", status: "draft", description: "", updatedAt: "", variables: [{ key: "role", label: "Role choice", value_type: "resource_ref", validation_rules: { resource: "site" } }], steps: [{ step_key: "device", title: "Device", resource_type: "device", operation: "create", depends_on_step_keys: [], payload_template: { name: "Switch", site_id: "site-id", role_id: "{{role}}", device_type_id: "type-id" }, on_conflict_policy: "fail" }] });
    api.updateTemplate.mockResolvedValue({});
    renderEditor("/inventory/templates/tpl-1/edit", true);
    selectTab(await screen.findByRole("tab", { name: /^Variables/ }));
    fireEvent.click(await screen.findByRole("button", { name: /role.*Role choice/i }));
    const selector = screen.getByRole("combobox", { name: "Inventory object" });
    expect(selector).toBeDisabled();
    expect(selector).toHaveTextContent("Device Role");
    fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));
    await waitFor(() => expect(api.updateTemplate).toHaveBeenCalledWith("tpl-1", expect.objectContaining({ variables: [expect.objectContaining({ validation_rules: { resource: "device_role" } })] })));
  });

  beforeEach(() => {
    vi.clearAllMocks();
    api.previewTemplateRun.mockResolvedValue({ steps: [] });
    api.getTemplate.mockResolvedValue({ id: "tpl-1", name: "Network", slug: "network", status: "draft", description: "", variables: [], steps: [], updatedAt: "" });
  });

  it.each(["/inventory/templates/new", "/inventory/templates/tpl-1/edit"])("omits save for a READ-only viewer at %s", async path => {
    renderEditor(path, false);
    await screen.findByRole("heading", { level: 1 });
    expect(screen.queryByRole("button", { name: /Save Changes|Create Template/i })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /Back to/i }));
    expect(await screen.findByText("template-list")).toBeInTheDocument();
    expect(api.createTemplate).not.toHaveBeenCalled();
    expect(api.updateTemplate).not.toHaveBeenCalled();
  });

  it("retains validation and saving for a writer", async () => {
    renderEditor("/inventory/templates/new", true);
    fireEvent.click(await screen.findByRole("button", { name: /Create Template/i }));
    await waitFor(() => expect(screen.getByRole("textbox", { name: /Name/ })).toHaveAttribute("aria-invalid", "true"));
    expect(screen.getByRole("textbox", { name: /Name/ })).toHaveAccessibleDescription("Name is required");
    fireEvent.change(screen.getByRole("textbox", { name: /Name/ }), { target: { value: "Network" } });
    expect(screen.getByRole("textbox", { name: /Name/ })).not.toHaveAttribute("aria-describedby");
    expect(api.createTemplate).not.toHaveBeenCalled();
  });

  it("focuses the invalid name input, not the label help button, on client-side submit", async () => {
    const raf = vi.spyOn(window, "requestAnimationFrame").mockImplementation((callback) => {
      callback(0);
      return 1;
    });
    try {
      renderEditor("/inventory/templates/new", true);
      fireEvent.click(await screen.findByRole("button", { name: /Create Template/i }));
      await waitFor(() => {
        expect(document.activeElement).toBe(document.getElementById("meta-name"));
      });
    } finally {
      raf.mockRestore();
    }
  });

  it("guards dirty Back and lets the author continue or discard", async () => {
    renderEditor("/inventory/templates/new", true);
    fireEvent.change(screen.getByRole("textbox", { name: /Name/ }), { target: { value: "Draft" } });
    fireEvent.click(screen.getByRole("button", { name: /Back to/i }));
    expect(await screen.findByRole("alertdialog")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /Continue editing/i }));
    expect(screen.getByRole("textbox", { name: /Name/ })).toHaveValue("Draft");
    fireEvent.click(screen.getByRole("button", { name: /Back to/i }));
    fireEvent.click(screen.getByRole("button", { name: /^Discard/i }));
    expect(await screen.findByText("template-list")).toBeInTheDocument();
    expect(api.createTemplate).not.toHaveBeenCalled();
  });

  it("saves a valid edited template and returns to its list", async () => {
    const template = { id: "tpl-1", name: "Network", slug: "network", status: "draft", description: "", variables: [], updatedAt: "", steps: [{
      step_key: "site", title: "Create site", resource_type: "site", operation: "create", depends_on_step_keys: [], payload_template: { name: "Edge", slug: "edge" }, on_conflict_policy: "fail",
    }] };
    api.getTemplate.mockResolvedValue(template);
    api.updateTemplate.mockResolvedValue({ ...template, name: "Edited" });
    renderEditor("/inventory/templates/tpl-1/edit", true);
    fireEvent.change(await screen.findByRole("textbox", { name: /Name/ }), { target: { value: "Edited" } });
    fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));
    await waitFor(() => expect(api.updateTemplate).toHaveBeenCalledWith("tpl-1", expect.objectContaining({ name: "Edited", slug: "network", steps: [expect.objectContaining(template.steps[0])] })));
    expect(await screen.findByText("template-list")).toBeInTheDocument();
  });

  const orderedTemplate = (): InventoryTemplate => ({
    id: "tpl-1", name: "Ordered draft", slug: "ordered", status: "draft", description: "", updatedAt: "",
    variables: [
      { key: "choice", label: "Choice", value_type: "resource_ref", validation_rules: { resource: "site" }, default_value: { resource: "site", id: "00000000-0000-4000-8000-000000000001" } },
      { key: "notes", label: "Notes", value_type: "string", default_value: "Keep notes" },
    ],
    steps: [
      { step_key: "first", title: "First site", resource_type: "site", operation: "create", depends_on_step_keys: [], payload_template: { name: "First" }, on_conflict_policy: "fail" },
      { step_key: "second", title: "Second site", resource_type: "site", operation: "create", depends_on_step_keys: [], payload_template: { name: "Second" }, on_conflict_policy: "fail" },
    ],
  });

  it("shows one inventory-style panel, keeps edits across tabs, and previews the unsaved draft", async () => {
    api.getTemplate.mockResolvedValue(orderedTemplate());
    renderEditor("/inventory/templates/tpl-1/edit", true);
    await screen.findByRole("tab", { name: "Details" });
    expect(screen.getAllByRole("tabpanel")).toHaveLength(1);
    expect(screen.queryByRole("button", { name: /choice.*Choice/i })).not.toBeInTheDocument();
    fireEvent.change(screen.getByRole("textbox", { name: "Description" }), { target: { value: "Unsaved details" } });
    selectTab(screen.getByRole("tab", { name: /^Steps/ }));
    fireEvent.click(screen.getByRole("button", { name: /First site.*Site/i }));
    fireEvent.change(screen.getByRole("textbox", { name: "Step name" }), { target: { value: "Edited first step" } });
    selectTab(screen.getByRole("tab", { name: "Preview" }));
    expect(screen.getAllByRole("tabpanel")).toHaveLength(1);
    expect(screen.getByText("Edited first step")).toBeInTheDocument();
    expect(api.previewTemplateRun).not.toHaveBeenCalled();
    selectTab(screen.getByRole("tab", { name: "Details" }));
    expect(screen.getByRole("textbox", { name: "Description" })).toHaveValue("Unsaved details");
  });

  it("does not mark opening tabs or rows as an unsaved document change", async () => {
    api.getTemplate.mockResolvedValue(orderedTemplate());
    renderEditor("/inventory/templates/tpl-1/edit", true);
    selectTab(await screen.findByRole("tab", { name: /^Variables/ }));
    fireEvent.click(screen.getByRole("button", { name: /notes.*Notes/ }));
    selectTab(screen.getByRole("tab", { name: /^Steps/ }));
    expect(screen.queryByText("You have unsaved changes")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /Back to/ }));
    expect(await screen.findByText("template-list")).toBeInTheDocument();
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
  });

  it("moves structured defaults and open rows atomically and saves both custom orders without UI metadata", async () => {
    const template = orderedTemplate();
    const edited = { resource: "site", id: "00000000-0000-4000-8000-000000000002" };
    api.getTemplate.mockResolvedValue(template);
    api.updateTemplate.mockResolvedValue(template);
    renderEditor("/inventory/templates/tpl-1/edit", true);
    selectTab(await screen.findByRole("tab", { name: /^Variables/ }));
    fireEvent.click(screen.getByRole("button", { name: /choice.*Choice/i }));
    fireEvent.change(screen.getByRole("textbox", { name: "Default" }), { target: { value: "{" } });
    fireEvent.click(screen.getByRole("button", { name: "Move down: Choice" }));
    expect(screen.getByRole("textbox", { name: "Default" })).toHaveValue("{");
    fireEvent.change(screen.getByRole("textbox", { name: "Default" }), { target: { value: JSON.stringify(edited) } });
    const rows = within(screen.getByRole("list", { name: "Variables" })).getAllByRole("listitem");
    expect(rows[0]).toHaveTextContent("Notes");
    expect(rows[1]).toHaveTextContent("Choice");
    selectTab(screen.getByRole("tab", { name: /^Steps/ }));
    fireEvent.click(screen.getByRole("button", { name: "Move down: First site" }));
    fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));
    await waitFor(() => expect(api.updateTemplate).toHaveBeenCalledWith("tpl-1", expect.objectContaining({
      variables: [expect.objectContaining({ key: "notes", default_value: "Keep notes", sort_order: 0 }), expect.objectContaining({ key: "choice", default_value: edited, sort_order: 1 })],
      steps: [expect.objectContaining({ step_key: "second", sort_order: 0 }), expect.objectContaining({ step_key: "first", sort_order: 1 })],
    })));
    const payload = api.updateTemplate.mock.calls[0]?.[1] as InventoryTemplate;
    expect(payload.variables.every(value => !("editorId" in value) && !("structuredDefault" in value))).toBe(true);
  });

  it("rejects a move across a producer and keeps its references and order unchanged", async () => {
    const template = orderedTemplate();
    template.steps[1]!.depends_on_step_keys = ["first"];
    template.steps[1]!.payload_template = { name: "Second", region_id: "{{steps.first.id}}" };
    api.getTemplate.mockResolvedValue(template);
    api.updateTemplate.mockResolvedValue(template);
    renderEditor("/inventory/templates/tpl-1/edit", true);
    selectTab(await screen.findByRole("tab", { name: /^Steps/ }));
    fireEvent.click(screen.getByRole("button", { name: "Move up: Second site" }));
    expect(screen.getByRole("alert")).toHaveTextContent("second must stay after first");
    const rows = within(screen.getByRole("list", { name: "Steps" })).getAllByRole("listitem");
    expect(rows[0]).toHaveTextContent("First site");
    fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));
    await waitFor(() => expect(api.updateTemplate).toHaveBeenCalledWith("tpl-1", expect.objectContaining({ steps: [
      expect.objectContaining({ step_key: "first" }),
      expect.objectContaining({ step_key: "second", depends_on_step_keys: ["first"], payload_template: template.steps[1]!.payload_template }),
    ] })));
  });

  it("opens the hidden invalid panel and focuses its input after Save", async () => {
    const template = orderedTemplate();
    template.variables[0]!.key = "invalid key";
    api.getTemplate.mockResolvedValue(template);
    renderEditor("/inventory/templates/tpl-1/edit", true);
    fireEvent.click(await screen.findByRole("button", { name: "Save Changes" }));
    await waitFor(() => expect(screen.getByRole("tab", { name: /^Variables/ })).toHaveAttribute("aria-selected", "true"));
    await waitFor(() => expect(document.getElementById("var-0-key-input")).toHaveFocus());
    expect(api.updateTemplate).not.toHaveBeenCalled();
  });

  it("lets read-only users inspect tabs and rows without changing values or order", async () => {
    api.getTemplate.mockResolvedValue(orderedTemplate());
    renderEditor("/inventory/templates/tpl-1/edit", false);
    expect(await screen.findByRole("textbox", { name: /Name/ })).toBeDisabled();
    selectTab(screen.getByRole("tab", { name: /^Variables/ }));
    expect(screen.getByRole("button", { name: "Move down: Choice" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: /choice.*Choice/i }));
    expect(screen.getByRole("textbox", { name: "Default" })).toBeDisabled();
    expect(api.updateTemplate).not.toHaveBeenCalled();
  });

  it("keeps the edit-session draft when the same template refetches", async () => {
    const template = orderedTemplate();
    api.getTemplate.mockResolvedValue(template);
    const { client } = renderEditor("/inventory/templates/tpl-1/edit", true);
    fireEvent.change(await screen.findByRole("textbox", { name: "Description" }), { target: { value: "My unsaved description" } });
    await act(async () => {
      client.setQueryData(["inventory", "template", "tpl-1"], { ...template, name: "Refetched name" });
      await Promise.resolve();
    });
    expect(screen.getByRole("textbox", { name: "Description" })).toHaveValue("My unsaved description");
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Ordered draft");
    fireEvent.click(screen.getByRole("button", { name: /Back to/ }));
    expect(await screen.findByRole("alertdialog")).toBeInTheDocument();
  });

  it("blocks edits during save and retains the draft with a safe error after failure", async () => {
    api.getTemplate.mockResolvedValue(orderedTemplate());
    let reject!: (reason: Error) => void;
    api.updateTemplate.mockImplementationOnce(() => new Promise((_, fail) => { reject = fail; }));
    renderEditor("/inventory/templates/tpl-1/edit", true);
    selectTab(await screen.findByRole("tab", { name: /^Variables/ }));
    fireEvent.click(screen.getByRole("button", { name: /notes.*Notes/ }));
    fireEvent.change(screen.getByRole("textbox", { name: "Default" }), { target: { value: "Unsaved notes" } });
    fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Move up: Notes" })).toBeDisabled());
    expect(screen.getByRole("textbox", { name: "Default" })).toBeDisabled();
    await act(async () => {
      reject(new Error("internal diagnostic text"));
      await Promise.resolve();
    });
    await waitFor(() => expect(screen.getByRole("button", { name: "Move up: Notes" })).toBeEnabled());
    expect(screen.getByRole("textbox", { name: "Default" })).toHaveValue("Unsaved notes");
    expect(notifications.notify).toHaveBeenCalledWith("error", expect.stringContaining("Your changes are still here"));
    expect(screen.queryByText("internal diagnostic text")).not.toBeInTheDocument();
  });
});
