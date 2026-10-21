import { act, fireEvent, screen, waitFor } from "@testing-library/react";
import { useState } from "react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes, useNavigate } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AllPermissions } from "@/test/permissions";
import { PermissionsContext, type PermissionsValue } from "@/shared/auth/permissionsContext";
import { changeAppLanguage } from "@/shared/i18n";
import en from "@/shared/i18n/locales/en";
import type { MappingConfiguration } from "@/shared/api/types";
import { ApiError } from "@/shared/api/client";
import SyncMappingEditor from "./SyncMappingEditor";

const api = vi.hoisted(() => ({ get: vi.fn(), update: vi.fn(), create: vi.fn(), providers: vi.fn(), operators: vi.fn(), suggest: vi.fn() }));
vi.mock("./sync-api", async importOriginal => ({ ...await importOriginal<Record<string, unknown>>(), listProviders: api.providers }));
vi.mock("./sync-mapping-api", async importOriginal => ({
  ...await importOriginal<Record<string, unknown>>(),
  getMappingConfiguration: api.get, updateMappingConfiguration: api.update, createMappingConfiguration: api.create,
  listOperators: api.operators,
  suggestReverse: api.suggest,
}));
vi.mock("@/shared/lib/notifications", () => ({ notify: vi.fn() }));

const config: MappingConfiguration = {
  id: "mapping-1", logical_id: "logical-1", name: "Inventory pull", version: 1,
  is_latest: true, provider_id: "provider-1", target: { kind: "local_inventory" }, status: "active",
  direction: "pull", conflict_mode: "netbox_wins", entity_mappings: [], created_at: "2026-01-01T00:00:00Z", in_flight_job_count: 0,
};

const configured: MappingConfiguration = { ...config, entity_mappings: [{
  id: "entity-1", source_entity: "dcim.devices", target_entity: "Devices", enabled: true,
  field_mappings: [{ id: "field-1", source_field: "name", target_field: "name", enabled: true,
    forward_transform: { operator: "trim", args: { field: "name" } }, reverse_transform: null }],
}] };

function renderEditor(path = "/inventory/sync/mappings/mapping-1/edit", permissions?: PermissionsValue) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<QueryClientProvider client={client}><AllPermissions>
    <MemoryRouter initialEntries={["/inventory/sync/mappings", path]} initialIndex={1}>
      <PermissionsContext.Provider value={permissions ?? { can: () => true, canAny: () => true, hasRole: () => true, roles: [], isAdmin: true }}>
      <Routes>
        <Route path="/inventory/sync/mappings" element={<h1>Mapping list</h1>} />
        <Route path="/inventory/sync/:providerId/mappings" element={<h1>Mapping list</h1>} />
        <Route path="/inventory/sync/mappings/new" element={<SyncMappingEditor />} />
        <Route path="/inventory/sync/mappings/:id/:mode" element={<SyncMappingEditor />} />
      </Routes>
      </PermissionsContext.Provider>
    </MemoryRouter>
  </AllPermissions></QueryClientProvider>);
}

beforeEach(async () => {
  vi.clearAllMocks();
  await changeAppLanguage("en");
  api.get.mockResolvedValue(config);
  api.providers.mockResolvedValue({ items: [], total: 0 });
  api.update.mockResolvedValue({ ...config, id: "mapping-2", version: 2 });
  api.operators.mockResolvedValue([]);
});

describe("SyncMappingEditor workbench", () => {
  it.each([false, true])("exposes only persisted entity settings (readonly: %s)", async readonly => {
    const saved: MappingConfiguration = { ...configured, id: "mapping-2", version: 2,
      entity_mappings: configured.entity_mappings.map(entity => ({ ...entity, enabled: false })) };
    api.get.mockResolvedValueOnce(configured).mockResolvedValue(saved);
    api.update.mockResolvedValue(saved);
    renderEditor(`/inventory/sync/mappings/mapping-1/${readonly ? "readonly" : "edit"}`);
    await screen.findByRole("heading", { name: "Inventory pull v1" });
    fireEvent.click(screen.getByRole("treeitem", { name: /name/ }));
    fireEvent.keyDown(screen.getByRole("tab", { name: en.sync.mapping.tabSettings }), { key: "Enter" });
    const enabled = screen.getByRole("switch", { name: en.sync.mapping.entityEnabled });
    expect(enabled).toBeChecked();
    expect(screen.getAllByRole("switch")).toHaveLength(1);
    if (readonly) {
      expect(enabled).toBeDisabled();
      expect(api.update).not.toHaveBeenCalled();
    } else {
      fireEvent.click(enabled);
      expect(enabled).not.toBeChecked();
      fireEvent.click(screen.getByRole("button", { name: /save as v2/i }));
      await waitFor(() => expect(api.update).toHaveBeenCalledTimes(1));
      expect(api.update).toHaveBeenCalledWith(configured.id, {
        name: configured.name, direction: configured.direction, conflict_mode: configured.conflict_mode,
        entity_mappings: saved.entity_mappings,
      });
      for (const entity of api.update.mock.calls[0]?.[1].entity_mappings ?? []) {
        for (const key of ["auto_sync_enabled", "dry_run_mode", "verbose_logging"]) expect(entity).not.toHaveProperty(key);
      }
      await screen.findByRole("heading", { name: "Inventory pull v2" });
      expect(screen.getByRole("switch", { name: en.sync.mapping.entityEnabled })).not.toBeChecked();
    }
  });

  function expectStandardOwner() {
    const owners = document.querySelectorAll('[data-slot="page-layout"], [data-slot="entity-editor-page"]');
    expect(owners).toHaveLength(1);
    expect(owners[0]).toHaveClass("max-w-[1280px]");
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
  }

  it("discards the complete authoring draft on return to immutable detail and reopening edit", async () => {
    api.get.mockResolvedValue(configured);
    renderEditor("/inventory/sync/mappings/mapping-1/readonly");
    await screen.findByRole("heading", { name: "Inventory pull v1" });
    fireEvent.click(screen.getByRole("link", { name: en.edit }));
    fireEvent.change(await screen.findByRole("textbox", { name: /configuration name/i }), { target: { value: "Unsaved name" } });
    fireEvent.click(screen.getByRole("combobox", { name: /direction/i }));
    fireEvent.click(screen.getByRole("option", { name: en.sync.mapping.directionPush }));
    fireEvent.click(screen.getByRole("combobox", { name: /conflict mode/i }));
    fireEvent.click(screen.getByRole("option", { name: en.sync.mapping.conflictLocalWins }));
    fireEvent.click(screen.getByRole("treeitem", { name: /name/ }));
    fireEvent.change(screen.getByRole("textbox", { name: /field path/i }), { target: { value: "unsaved.field" } });
    fireEvent.click(screen.getByRole("button", { name: en.cancel }));
    await screen.findByRole("alertdialog");
    fireEvent.click(screen.getByRole("button", { name: en.entityEditor.discard }));
    await screen.findByRole("link", { name: en.edit });
    expect(screen.getByRole("heading", { name: "Inventory pull v1" })).toBeInTheDocument();
    expect(screen.getByText(en.sync.mapping.directionPull, { selector: "dd" })).toBeInTheDocument();
    expect(screen.getByText(en.sync.mapping.conflictRemoteWins, { selector: "dd" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("treeitem", { name: /name/ }));
    expect(screen.getByRole("textbox", { name: /field path/i })).toHaveValue("name");
    fireEvent.click(screen.getByRole("link", { name: en.edit }));
    expect(await screen.findByRole("textbox", { name: /configuration name/i })).toHaveValue("Inventory pull");
    expect(screen.getByRole("combobox", { name: /direction/i })).toHaveTextContent(en.sync.mapping.directionPull);
    expect(screen.getByRole("combobox", { name: /conflict mode/i })).toHaveTextContent(en.sync.mapping.conflictRemoteWins);
    fireEvent.click(screen.getByRole("treeitem", { name: /name/ }));
    expect(screen.getByRole("textbox", { name: /field path/i })).toHaveValue("name");
    expect(api.update).not.toHaveBeenCalled();
  });

  it("does not expose an authoring draft after permission loss or restore it when permission returns", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    function PermissionHarness() {
      const [allowed, setAllowed] = useState(true);
      return <PermissionsContext.Provider value={{ can: () => allowed, canAny: () => allowed, hasRole: () => false, roles: [], isAdmin: allowed }}>
        <button onClick={() => setAllowed(value => !value)}>Toggle permission</button>
        <SyncMappingEditor />
      </PermissionsContext.Provider>;
    }
    render(<QueryClientProvider client={client}><MemoryRouter initialEntries={["/inventory/sync/mappings/mapping-1/edit"]}>
      <Routes><Route path="/inventory/sync/mappings/:id/:mode" element={<PermissionHarness />} /></Routes>
    </MemoryRouter></QueryClientProvider>);
    fireEvent.change(await screen.findByRole("textbox", { name: /configuration name/i }), { target: { value: "Unauthorized draft" } });
    fireEvent.click(screen.getByRole("button", { name: "Toggle permission" }));
    expect(await screen.findByRole("heading", { name: "Inventory pull v1" })).toBeInTheDocument();
    expect(screen.queryByRole("textbox", { name: /configuration name/i })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Toggle permission" }));
    expect(await screen.findByRole("textbox", { name: /configuration name/i })).toHaveValue("Inventory pull");
  });

  it("resets draft for another mapping id without wiping edits on a same-id query refresh", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    api.get.mockImplementation((id: string) => Promise.resolve(id === "mapping-2" ? { ...config, id, name: "Other mapping" } : config));
    function RouteHarness() {
      const navigate = useNavigate();
      return <><button onClick={() => navigate("/inventory/sync/mappings/mapping-2/edit")}>Other mapping</button><SyncMappingEditor /></>;
    }
    render(<QueryClientProvider client={client}><AllPermissions><MemoryRouter initialEntries={["/inventory/sync/mappings/mapping-1/edit"]}>
      <Routes><Route path="/inventory/sync/mappings/:id/:mode" element={<RouteHarness />} /></Routes>
    </MemoryRouter></AllPermissions></QueryClientProvider>);
    fireEvent.change(await screen.findByRole("textbox", { name: /configuration name/i }), { target: { value: "Keep valid edits" } });
    act(() => client.setQueryData(["sync", "mapping-configuration", "mapping-1"], { ...config, name: "Refreshed server name" }));
    expect(screen.getByRole("textbox", { name: /configuration name/i })).toHaveValue("Keep valid edits");
    fireEvent.click(screen.getByRole("button", { name: "Other mapping" }));
    expect(await screen.findByRole("heading", { name: "Other mapping v1" })).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: /configuration name/i })).toHaveValue("Other mapping");
    expect(api.update).not.toHaveBeenCalled();
  });

  it("clears a discarded draft even when history returns to the same authoring identity", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<QueryClientProvider client={client}><AllPermissions>
      <MemoryRouter initialEntries={["/inventory/sync/mappings/mapping-1/edit?context=list", "/inventory/sync/mappings/mapping-1/edit?context=detail"]} initialIndex={1}>
        <Routes><Route path="/inventory/sync/mappings/:id/:mode" element={<SyncMappingEditor />} /></Routes>
      </MemoryRouter>
    </AllPermissions></QueryClientProvider>);
    fireEvent.change(await screen.findByRole("textbox", { name: /configuration name/i }), { target: { value: "Discard this draft" } });
    fireEvent.click(screen.getByRole("button", { name: en.cancel }));
    await screen.findByRole("alertdialog");
    fireEvent.click(screen.getByRole("button", { name: en.entityEditor.discard }));
    await waitFor(() => expect(screen.getByRole("textbox", { name: /configuration name/i })).toHaveValue("Inventory pull"));
    expect(api.update).not.toHaveBeenCalled();
  });

  it("owns standard geometry and back navigation while configuration is pending", () => {
    api.get.mockReturnValue(new Promise(() => {}));
    renderEditor();
    expectStandardOwner();
    expect(screen.queryByRole("button", { name: /save as/i })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: en.sync.mapping.backToList }));
    expect(screen.getByRole("heading", { name: "Mapping list" })).toBeInTheDocument();
  });

  it("retains one standard owner through load failure, retry and loaded editor", async () => {
    api.get.mockRejectedValueOnce(new Error("private backend detail")).mockResolvedValue(config);
    renderEditor();
    await screen.findByText(en.sync.mapping.loadConfigError);
    expectStandardOwner();
    expect(screen.queryByText("private backend detail")).not.toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: en.sync.mapping.backToList })).toHaveLength(1);
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    await screen.findByRole("textbox", { name: /configuration name/i });
    expectStandardOwner();
  });

  it.each(["404", "empty"])("owns standard geometry for a missing configuration (%s)", async (kind) => {
    if (kind === "404") api.get.mockRejectedValue(new ApiError({ status: 404, message: "private missing detail" }));
    else api.get.mockResolvedValue(null);
    renderEditor();
    await screen.findByRole("alert");
    expectStandardOwner();
    expect(screen.queryByRole("textbox", { name: /configuration name/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /save as/i })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: en.sync.mapping.backToList }));
    expect(screen.getByRole("heading", { name: "Mapping list" })).toBeInTheDocument();
  });

  it("shows immutable mapping identity as definitions, with a read-only field tree", async () => {
    api.get.mockResolvedValue({ ...configured, is_latest: false });
    renderEditor("/inventory/sync/mappings/mapping-1/readonly?context=inventory#fields");
    await screen.findByRole("heading", { level: 1, name: "Inventory pull v1" });
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expectStandardOwner();
    expect(screen.getByText("provider-1", { selector: "dd" })).toBeInTheDocument();
    expect(screen.getByText(en.sync.mapping.targetLocalInventory, { selector: "dd" })).toBeInTheDocument();
    expect(screen.queryByRole("textbox", { name: /configuration name/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /save as/i })).not.toBeInTheDocument();
    expect(screen.getByRole("link", { name: en.sync.mapping.editLatest })).toHaveAttribute("href", "/inventory/sync/mappings/mapping-1/edit?context=inventory#fields");
    expect(screen.getByRole("link", { name: en.sync.mapping.backToList })).toHaveAttribute("href", "/inventory/sync/provider-1/mappings?context=inventory#fields");
    fireEvent.click(screen.getByRole("treeitem", { name: /name/ }));
    expect(await screen.findByRole("tab", { name: en.sync.mapping.tabPreview })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: en.sync.mapping.addEntity })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("link", { name: en.sync.mapping.backToList }));
    expect(await screen.findByRole("heading", { name: "Mapping list" })).toBeInTheDocument();
    expect(api.update).not.toHaveBeenCalled();
  });

  it("uses read-only detail for an edit URL without update permission", async () => {
    renderEditor(undefined, { can: () => false, canAny: () => false, hasRole: () => false, roles: [], isAdmin: false });
    await screen.findByRole("heading", { level: 1, name: "Inventory pull v1" });
    expect(screen.getByText("provider-1", { selector: "dd" })).toBeInTheDocument();
    expect(screen.queryByRole("textbox", { name: /configuration name/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: en.sync.mapping.addEntity })).not.toBeInTheDocument();
    expect(screen.queryByRole("link", { name: en.sync.mapping.editLatest })).not.toBeInTheDocument();
    expect(api.update).not.toHaveBeenCalled();
  });

  it("exposes configuration and field sections with one page heading", async () => {
    renderEditor();
    await screen.findByRole("heading", { level: 1, name: "Inventory pull v1" });
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(screen.getByRole("heading", { level: 2, name: en.sync.mapping.stepConfig })).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 2, name: en.sync.mapping.stepFields })).toBeInTheDocument();
  });

  it("allows cancel on a clean editor and confirms discard on a dirty editor", async () => {
    renderEditor();
    const name = await screen.findByRole("textbox", { name: /configuration name/i });
    fireEvent.change(name, { target: { value: "Changed mapping" } });
    fireEvent.click(screen.getByRole("button", { name: en.cancel }));
    expect(await screen.findByRole("alertdialog")).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Mapping list" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: en.entityEditor.continueEditing }));
    expect(name).toHaveValue("Changed mapping");
    fireEvent.change(name, { target: { value: config.name } });
    fireEvent.click(screen.getByRole("button", { name: en.cancel }));
    expect(await screen.findByRole("heading", { name: "Mapping list" })).toBeInTheDocument();
    expect(api.update).not.toHaveBeenCalled();
  });

  it("disables invalid saves and submits the versioned payload once", async () => {
    renderEditor();
    const name = await screen.findByRole("textbox", { name: /configuration name/i });
    fireEvent.change(name, { target: { value: " " } });
    expect(screen.getByRole("button", { name: /save as v2/i })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: /save as v2/i }));
    expect(api.update).not.toHaveBeenCalled();
    fireEvent.change(name, { target: { value: "Updated mapping" } });
    fireEvent.click(screen.getByRole("button", { name: /save as v2/i }));
    await waitFor(() => expect(api.update).toHaveBeenCalledTimes(1));
    expect(api.update).toHaveBeenCalledWith(config.id, {
      name: "Updated mapping", direction: "pull", conflict_mode: "netbox_wins", entity_mappings: [],
    });
  });

  it("requires a provider on the create route", () => {
    renderEditor("/inventory/sync/mappings/new");
    expect(screen.getByRole("button", { name: en.save })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: en.save }));
    expect(api.create).not.toHaveBeenCalled();
  });

  it("guards non-name changes on the create route", async () => {
    renderEditor("/inventory/sync/mappings/new");
    fireEvent.click(screen.getByRole("combobox", { name: /direction/i }));
    fireEvent.click(screen.getByRole("option", { name: en.sync.mapping.directionPush }));
    fireEvent.click(screen.getByRole("button", { name: en.cancel }));
    expect(await screen.findByRole("alertdialog")).toBeInTheDocument();
    expect(api.create).not.toHaveBeenCalled();
  });

  it("keeps the pending save disabled and does not submit twice", async () => {
    const saved = { ...config, name: "Pending mapping", id: "mapping-2", version: 2 };
    api.get.mockResolvedValueOnce(config).mockResolvedValue(saved);
    let resolveSave!: (saved: MappingConfiguration) => void;
    api.update.mockReturnValue(new Promise<MappingConfiguration>(resolve => { resolveSave = resolve; }));
    renderEditor();
    const name = await screen.findByRole("textbox", { name: /configuration name/i });
    fireEvent.change(name, { target: { value: "Pending mapping" } });
    fireEvent.click(screen.getByRole("button", { name: /save as v2/i }));
    await waitFor(() => expect(api.update).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(screen.getByRole("button", { name: /save as v2/i })).toBeDisabled());
    fireEvent.click(screen.getByRole("button", { name: /save as v2/i }));
    expect(api.update).toHaveBeenCalledTimes(1);
    act(() => { resolveSave(saved); });
    expect(await screen.findByRole("heading", { level: 1, name: "Pending mapping v2" }, { timeout: 1000 })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /save as v3/i })).toBeEnabled();
    fireEvent.change(screen.getByRole("textbox", { name: /configuration name/i }), { target: { value: "Next edit" } });
    fireEvent.click(screen.getByRole("button", { name: en.cancel }));
    expect(await screen.findByRole("alertdialog")).toBeInTheDocument();
  });

  it("locks every draft editing surface before a deferred save resolves and unlocks the saved version", async () => {
    const current = configured;
    const saved = { ...current, name: "Snapshot A", id: "mapping-2", version: 2 };
    api.get.mockResolvedValueOnce(current).mockResolvedValue(saved);
    let resolveSave!: (saved: MappingConfiguration) => void;
    api.update.mockReturnValue(new Promise<MappingConfiguration>(resolve => { resolveSave = resolve; }));
    renderEditor();
    const name = await screen.findByRole("textbox", { name: /configuration name/i });
    fireEvent.click(screen.getByRole("treeitem", { name: /name/ }));
    const operator = screen.getByRole("combobox", { name: /operator.*forward/i });
    const field = screen.getByRole("textbox", { name: /field path/i });
    fireEvent.change(name, { target: { value: "Snapshot A" } });
    fireEvent.click(screen.getByRole("button", { name: /save as v2/i }));
    await waitFor(() => expect(api.update).toHaveBeenCalledTimes(1));
    expect(name).toBeDisabled();
    expect(operator).toBeDisabled();
    expect(field).toBeDisabled();
    expect(screen.getByRole("combobox", { name: /direction/i })).toBeDisabled();
    expect(screen.queryByRole("button", { name: /add entity/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /add field/i })).not.toBeInTheDocument();
    fireEvent.change(name, { target: { value: "Discarded B" } });
    fireEvent.change(field, { target: { value: "discarded.field" } });
    fireEvent.click(operator);
    expect(name).toHaveValue("Snapshot A");
    expect(field).toHaveValue("name");
    expect(operator).toHaveAttribute("aria-expanded", "false");
    const settingsTab = screen.getByRole("tab", { name: en.sync.mapping.tabSettings });
    fireEvent.keyDown(settingsTab, { key: "Enter" });
    for (const control of screen.getAllByRole("switch")) expect(control).toBeDisabled();
    expect(api.update).toHaveBeenCalledWith(current.id, {
      name: "Snapshot A", direction: "pull", conflict_mode: "netbox_wins", entity_mappings: current.entity_mappings,
    });
    act(() => { resolveSave(saved); });
    await screen.findByRole("heading", { name: "Snapshot A v2" });
    const savedName = screen.getByRole("textbox", { name: /configuration name/i });
    expect(savedName).toBeEnabled();
    for (const control of screen.getAllByRole("switch")) expect(control).toBeEnabled();
    expect(screen.getByRole("button", { name: /add entity/i })).toBeEnabled();
    expect(screen.getByRole("button", { name: /save as v3/i })).toBeEnabled();
    fireEvent.change(savedName, { target: { value: "Next edit" } });
    fireEvent.click(screen.getByRole("button", { name: en.cancel }));
    expect(await screen.findByRole("alertdialog")).toBeInTheDocument();
  });

  it("unlocks after a rejected save without discarding the dirty draft or bypassing validation", async () => {
    let rejectSave!: (error: Error) => void;
    api.update.mockReturnValue(new Promise<MappingConfiguration>((_resolve, reject) => { rejectSave = reject; }));
    renderEditor();
    const name = await screen.findByRole("textbox", { name: /configuration name/i });
    fireEvent.change(name, { target: { value: "Retained draft" } });
    fireEvent.click(screen.getByRole("button", { name: /save as v2/i }));
    await waitFor(() => expect(api.update).toHaveBeenCalledTimes(1));
    expect(name).toBeDisabled();
    act(() => { rejectSave(new Error("Save rejected")); });
    await waitFor(() => expect(name).toBeEnabled());
    expect(name).toHaveValue("Retained draft");
    fireEvent.click(screen.getByRole("button", { name: en.cancel }));
    expect(await screen.findByRole("alertdialog")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: en.entityEditor.continueEditing }));
    fireEvent.change(name, { target: { value: " " } });
    expect(screen.getByRole("button", { name: /save as v2/i })).toBeDisabled();
    expect(api.update).toHaveBeenCalledTimes(1);
  });

  it("fences an operator suggestion callback started before the pending save", async () => {
    const saved = { ...configured, id: "mapping-2", version: 2 };
    api.get.mockResolvedValueOnce(configured).mockResolvedValue(saved);
    let resolveSave!: (saved: MappingConfiguration) => void;
    let resolveSuggestion!: (result: { reverse_transform: { operator: "identity"; args: { field: string } } }) => void;
    api.update.mockReturnValue(new Promise<MappingConfiguration>(resolve => { resolveSave = resolve; }));
    api.suggest.mockReturnValue(new Promise(resolve => { resolveSuggestion = resolve; }));
    renderEditor();
    await screen.findByRole("textbox", { name: /configuration name/i });
    fireEvent.click(screen.getByRole("treeitem", { name: /name/ }));
    fireEvent.click(screen.getByRole("button", { name: en.sync.mapping.suggestReverse }));
    await waitFor(() => expect(api.suggest).toHaveBeenCalledTimes(1));
    fireEvent.click(screen.getByRole("button", { name: /save as v2/i }));
    await waitFor(() => expect(api.update).toHaveBeenCalledTimes(1));
    await act(async () => {
      resolveSuggestion({ reverse_transform: { operator: "identity", args: { field: "late.field" } } });
      await api.suggest.mock.results[0]?.value;
    });
    expect(screen.queryByRole("combobox", { name: /operator.*reverse/i })).not.toBeInTheDocument();
    expect(api.update).toHaveBeenCalledWith(configured.id, {
      name: configured.name, direction: "pull", conflict_mode: "netbox_wins", entity_mappings: configured.entity_mappings,
    });
    act(() => { resolveSave(saved); });
    await screen.findByRole("heading", { name: "Inventory pull v2" });
    expect(screen.queryByRole("button", { name: en.sync.mapping.clearReverse })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: en.sync.mapping.suggestReverse })).toBeEnabled();
    fireEvent.click(screen.getByRole("button", { name: en.cancel }));
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
  });

  it.each([false, true])("rejects a deferred reverse suggestion after same-identity discard (expression unmounted: %s)", async unmountExpression => {
    api.get.mockResolvedValue(configured);
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<QueryClientProvider client={client}><AllPermissions>
      <MemoryRouter initialEntries={["/inventory/sync/mappings/mapping-1/edit?context=list", "/inventory/sync/mappings/mapping-1/edit?context=detail"]} initialIndex={1}>
        <Routes><Route path="/inventory/sync/mappings/:id/:mode" element={<SyncMappingEditor />} /></Routes>
      </MemoryRouter>
    </AllPermissions></QueryClientProvider>);
    let resolveSuggestion!: (result: { reverse_transform: { operator: "identity"; args: { field: string } } }) => void;
    const suggestion = new Promise(resolve => { resolveSuggestion = resolve; });
    api.suggest.mockReturnValueOnce(suggestion);
    const name = await screen.findByRole("textbox", { name: /configuration name/i });
    fireEvent.click(screen.getByRole("treeitem", { name: /name/ }));
    fireEvent.click(screen.getByRole("button", { name: en.sync.mapping.suggestReverse }));
    await waitFor(() => expect(api.suggest).toHaveBeenCalledTimes(1));
    if (unmountExpression) fireEvent.keyDown(screen.getByRole("tab", { name: en.sync.mapping.tabSettings }), { key: "Enter" });
    fireEvent.change(name, { target: { value: "Discarded edit" } });
    fireEvent.click(screen.getByRole("button", { name: en.cancel }));
    await screen.findByRole("alertdialog");
    fireEvent.click(screen.getByRole("button", { name: en.entityEditor.discard }));
    expect(screen.getByRole("textbox", { name: /configuration name/i })).toHaveValue(configured.name);
    await act(async () => {
      resolveSuggestion({ reverse_transform: { operator: "identity", args: { field: "discarded.result" } } });
      await suggestion;
    });
    if (unmountExpression) fireEvent.keyDown(screen.getByRole("tab", { name: en.sync.mapping.tabExpression }), { key: "Enter" });
    expect(screen.queryByRole("button", { name: en.sync.mapping.clearReverse })).not.toBeInTheDocument();
    await waitFor(() => expect(screen.getByRole("button", { name: en.sync.mapping.suggestReverse })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: en.cancel }));
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    api.suggest.mockResolvedValue({ reverse_transform: { operator: "identity", args: { field: "fresh.result" } } });
    fireEvent.click(screen.getByRole("button", { name: en.sync.mapping.suggestReverse }));
    await screen.findByRole("button", { name: en.sync.mapping.clearReverse });
    fireEvent.click(screen.getByRole("button", { name: /save as v2/i }));
    await waitFor(() => expect(api.update).toHaveBeenCalledTimes(1));
    expect(api.update.mock.calls[0]?.[1]).toMatchObject({ name: configured.name, entity_mappings: [{
      field_mappings: [{ reverse_transform: { operator: "identity", args: { field: "fresh.result" } } }],
    }] });
  });

  it.each([["success", false], ["success", true], ["failure", false], ["failure", true]] as const)("rejects a stale authoring callback after save %s (expression unmounted: %s)", async (outcome, unmountExpression) => {
    const saved = { ...configured, name: "Saved A", id: "mapping-2", version: 2 };
    api.get.mockResolvedValueOnce(configured).mockResolvedValue(outcome === "success" ? saved : configured);
    let resolveSave!: (saved: MappingConfiguration) => void;
    let rejectSave!: (error: Error) => void;
    let resolveSuggestion!: (result: { reverse_transform: { operator: "identity"; args: { field: string } } }) => void;
    api.update.mockReturnValueOnce(new Promise<MappingConfiguration>((resolve, reject) => { resolveSave = resolve; rejectSave = reject; }));
    const suggestion = new Promise(resolve => { resolveSuggestion = resolve; });
    api.suggest.mockReturnValue(suggestion);
    renderEditor();
    const name = await screen.findByRole("textbox", { name: /configuration name/i });
    fireEvent.click(screen.getByRole("treeitem", { name: /name/ }));
    fireEvent.click(screen.getByRole("button", { name: en.sync.mapping.suggestReverse }));
    await waitFor(() => expect(api.suggest).toHaveBeenCalledTimes(1));
    if (unmountExpression) {
      fireEvent.keyDown(screen.getByRole("tab", { name: en.sync.mapping.tabSettings }), { key: "Enter" });
      expect(screen.queryByRole("combobox", { name: /operator.*forward/i })).not.toBeInTheDocument();
    }
    fireEvent.change(name, { target: { value: "Saved A" } });
    fireEvent.click(screen.getByRole("button", { name: /save as v2/i }));
    await waitFor(() => expect(api.update).toHaveBeenCalledTimes(1));
    if (outcome === "success") {
      act(() => { resolveSave(saved); });
      await screen.findByRole("heading", { name: "Saved A v2" });
    } else {
      act(() => { rejectSave(new Error("Save rejected")); });
    }
    await waitFor(() => expect(name).toBeEnabled());
    await act(async () => {
      resolveSuggestion({ reverse_transform: { operator: "identity", args: { field: "stale.field" } } });
      await suggestion;
    });
    expect(screen.getByRole("heading", { name: outcome === "success" ? "Saved A v2" : "Saved A v1" })).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: /configuration name/i })).toHaveValue("Saved A");
    fireEvent.keyDown(screen.getByRole("tab", { name: en.sync.mapping.tabExpression }), { key: "Enter" });
    expect(screen.queryByRole("button", { name: en.sync.mapping.clearReverse })).not.toBeInTheDocument();
    await waitFor(() => expect(screen.getByRole("button", { name: en.sync.mapping.suggestReverse })).toBeEnabled());
    api.suggest.mockResolvedValue({ reverse_transform: { operator: "identity", args: { field: "current.field" } } });
    fireEvent.click(screen.getByRole("button", { name: en.sync.mapping.suggestReverse }));
    fireEvent.click(await screen.findByRole("button", { name: en.sync.mapping.clearReverse }));
    expect(screen.getByRole("textbox", { name: /configuration name/i })).toHaveValue("Saved A");
    const nextVersion = outcome === "success" ? 3 : 2;
    const next = { ...saved, id: "mapping-next", version: nextVersion };
    api.update.mockResolvedValue(next);
    api.get.mockResolvedValue(next);
    fireEvent.click(screen.getByRole("button", { name: new RegExp(`save as v${nextVersion}`, "i") }));
    await waitFor(() => expect(api.update).toHaveBeenCalledTimes(2));
    expect(api.update).toHaveBeenLastCalledWith(outcome === "success" ? saved.id : configured.id, {
      name: "Saved A", direction: "pull", conflict_mode: "netbox_wins", entity_mappings: configured.entity_mappings,
    });
    await screen.findByRole("heading", { name: `Saved A v${nextVersion}` });
  });

  it("uses the same pending lock for creation and unlocks the created version", async () => {
    api.providers.mockResolvedValue({ items: [{ id: "provider-1", providerId: "netbox", name: "Provider", config: {}, enabled: true, secretFields: [] }], total: 1 });
    const saved = { ...config, name: "Created A" };
    api.get.mockResolvedValue(saved);
    let resolveSave!: (saved: MappingConfiguration) => void;
    api.create.mockReturnValue(new Promise<MappingConfiguration>(resolve => { resolveSave = resolve; }));
    renderEditor("/inventory/sync/mappings/new");
    const name = screen.getByRole("textbox", { name: /configuration name/i });
    await waitFor(() => expect(screen.getByRole("button", { name: en.save })).toBeEnabled());
    fireEvent.change(name, { target: { value: "Created A" } });
    fireEvent.click(screen.getByRole("button", { name: en.save }));
    await waitFor(() => expect(api.create).toHaveBeenCalledTimes(1));
    expect(name).toBeDisabled();
    expect(screen.getByRole("combobox", { name: /provider/i })).toBeDisabled();
    fireEvent.change(name, { target: { value: "Attempted B" } });
    expect(name).toHaveValue("Created A");
    expect(api.create).toHaveBeenCalledWith({ logical_id: expect.any(String), name: "Created A", provider_id: "provider-1",
      target: { kind: "local_inventory" }, direction: "pull", conflict_mode: "netbox_wins", entity_mappings: [] });
    act(() => { resolveSave(saved); });
    await screen.findByRole("heading", { name: "Created A v1" });
    expect(name).toBeEnabled();
    expect(screen.getByRole("button", { name: /save as v2/i })).toBeEnabled();
  });

  it.each(["identity", "lookup", "trim"] as const)("locks the already-open %s argument/picker surface during saving", async operator => {
    const current: MappingConfiguration = { ...configured, entity_mappings: configured.entity_mappings.map(entity => ({
      ...entity, field_mappings: entity.field_mappings.map(field => ({ ...field,
        forward_transform: { operator, args: operator === "lookup" ? { key_field: "name", table: [] } : { field: "name" } },
      })),
    })) };
    const saved = { ...current, id: "mapping-2", version: 2 };
    api.get.mockResolvedValueOnce(current).mockResolvedValue(saved);
    let resolveSave!: (saved: MappingConfiguration) => void;
    api.update.mockReturnValue(new Promise<MappingConfiguration>(resolve => { resolveSave = resolve; }));
    renderEditor();
    await screen.findByRole("textbox", { name: /configuration name/i });
    fireEvent.click(screen.getByRole("treeitem", { name: /name/ }));
    if (operator === "lookup") {
      fireEvent.click(screen.getByRole("button", { name: en.sync.mapping.args.pasteCsv }));
      expect(screen.getByPlaceholderText(/active,active/)).toBeEnabled();
    } else {
      fireEvent.click(screen.getByRole("combobox", { name: operator === "identity" ? /field path/i : /operator.*forward/i }));
      expect(screen.getByPlaceholderText(operator === "identity" ? en.sync.mapping.args.fieldSuggestions : en.sync.mapping.operatorPickerPlaceholder)).toBeInTheDocument();
    }
    fireEvent.click(screen.getByRole("button", { name: /save as v2/i }));
    await waitFor(() => expect(api.update).toHaveBeenCalledTimes(1));
    if (operator === "lookup") {
      expect(screen.getByPlaceholderText(/active,active/)).toBeDisabled();
      expect(screen.getByRole("button", { name: en.sync.mapping.args.applyCSV })).toBeDisabled();
    } else {
      expect(screen.queryByPlaceholderText(operator === "identity" ? en.sync.mapping.args.fieldSuggestions : en.sync.mapping.operatorPickerPlaceholder)).not.toBeInTheDocument();
      expect(screen.getByRole("combobox", { name: operator === "identity" ? /field path/i : /operator.*forward/i })).toBeDisabled();
    }
    act(() => { resolveSave(saved); });
    await screen.findByRole("heading", { name: "Inventory pull v2" });
    if (operator === "lookup") {
      fireEvent.click(screen.getByRole("button", { name: en.sync.mapping.args.pasteCsv }));
      expect(screen.getByPlaceholderText(/active,active/)).toBeEnabled();
      expect(screen.getByRole("button", { name: en.sync.mapping.args.applyCSV })).toBeEnabled();
    } else {
      expect(screen.getByRole("combobox", { name: operator === "identity" ? /field path/i : /operator.*forward/i })).toBeEnabled();
    }
  });

  it("offers no save on archived versions and shows immutable configuration values", async () => {
    api.get.mockResolvedValue({ ...config, is_latest: false });
    renderEditor("/inventory/sync/mappings/mapping-1/readonly");
    await screen.findByRole("heading", { level: 1, name: "Inventory pull v1" });
    expect(screen.getByText("provider-1", { selector: "dd" })).toBeInTheDocument();
    expect(screen.queryByRole("textbox", { name: /configuration name/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /save as/i })).not.toBeInTheDocument();
    expect(screen.getByRole("link", { name: en.sync.mapping.editLatest })).toBeInTheDocument();
  });
});
