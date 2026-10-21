import { fireEvent, screen, waitFor } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { PermissionsContext } from "@/shared/auth/permissionsContext";
import "@/shared/i18n";
import TemplateEditorPage from "./TemplateEditorPage";
import { TooltipProvider } from "@/shared/ui-primitives/tooltip";

const api = vi.hoisted(() => ({ getTemplate: vi.fn(), createTemplate: vi.fn(), updateTemplate: vi.fn(), previewTemplateRun: vi.fn() }));
vi.mock("@/features/inventory/api", async (original) => ({ ...(await original()), ...api }));

function renderEditor(path: string, writable: boolean) {
  return render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
    <PermissionsContext.Provider value={{ roles: [], isAdmin: false, can: action => action === "READ" || writable, canAny: () => writable, hasRole: () => false }}>
      <TooltipProvider><MemoryRouter initialEntries={[path]}><Routes>
        <Route path="/inventory/templates/new" element={<TemplateEditorPage />} />
        <Route path="/inventory/templates/:id/edit" element={<TemplateEditorPage />} />
        <Route path="/inventory/templates" element={<div>template-list</div>} />
      </Routes></MemoryRouter></TooltipProvider>
    </PermissionsContext.Provider>
  </QueryClientProvider>);
}

describe("TemplateEditorPage capabilities", () => {
  it("keeps one standard-width shell from pending to loaded", async () => {
    let finish!: (value: unknown) => void;
    api.getTemplate.mockReturnValue(new Promise(resolve => { finish = resolve; }));
    renderEditor("/inventory/templates/tpl-1/edit", true);
    expect(screen.getByRole("status")).toHaveTextContent("Loading");
    expect(screen.getAllByTestId("entity-editor-page")).toHaveLength(1);
    expect(screen.getByTestId("entity-editor-page")).toHaveClass("max-w-[1280px]");
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    finish({ id: "tpl-1", name: "Loaded template", slug: "loaded", status: "draft", description: "", updatedAt: "", variables: [], steps: [] });
    await screen.findByRole("heading", { level: 1, name: "Loaded template" });
    expect(screen.getAllByTestId("entity-editor-page")).toHaveLength(1);
    expect(screen.getByTestId("entity-editor-page")).toHaveClass("max-w-[1280px]");
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
});
