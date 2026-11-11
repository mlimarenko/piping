import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import en from "@/shared/i18n/locales/en";
import { AllPermissions } from "@/test/permissions";
import { PermissionsContext } from "@/shared/auth/permissionsContext";
import PageChrome from "@/shared/ui-shared/PageChrome";
import CustomFields from "./CustomFields";
import ConfigContexts from "./ConfigContexts";
import SyncMappingList from "./SyncMappingList";
import ConfigContextScopeBindings from "./components/config-contexts/ConfigContextScopeBindings";
import type * as customFieldsApi from "./custom-fields-api";
import type * as configContextsApi from "./config-contexts-api";
import type * as syncMappingApi from "./sync-mapping-api";
import type * as inventoryApi from "./api";

const mocks = vi.hoisted(() => ({ fields: vi.fn(), choices: vi.fn(), contexts: vi.fn(), bindings: vi.fn(), mappings: vi.fn(), removeField: vi.fn(), removeChoice: vi.fn(), removeContext: vi.fn(), removeMapping: vi.fn(), removeBinding: vi.fn() }));
vi.mock("./custom-fields-api", async original => ({ ...await original<typeof customFieldsApi>(), listCustomFields: mocks.fields, listChoiceSets: mocks.choices, listContentTypes: () => Promise.resolve([]), deleteCustomField: mocks.removeField, deleteChoiceSet: mocks.removeChoice }));
vi.mock("./config-contexts-api", async original => ({ ...await original<typeof configContextsApi>(), listContexts: mocks.contexts, listBindings: mocks.bindings, deleteContext: mocks.removeContext, deleteBinding: mocks.removeBinding }));
vi.mock("./sync-mapping-api", async original => ({ ...await original<typeof syncMappingApi>(), listMappingConfigurations: mocks.mappings, deleteMappingConfiguration: mocks.removeMapping }));
vi.mock("./api", async original => ({ ...await original<typeof inventoryApi>(), listAllResources: () => Promise.resolve([{ id: "site", name: "Site A", type: "site" }]) }));

function mount(children: ReactNode, readOnly = false) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const permissions = { roles: [], isAdmin: false, can: (action: string) => action === "READ", canAny: () => true, hasRole: () => false };
  const content = <MemoryRouter>{children}</MemoryRouter>;
  return render(<QueryClientProvider client={client}>{readOnly ? <PermissionsContext.Provider value={permissions}>{content}</PermissionsContext.Provider> : <AllPermissions>{content}</AllPermissions>}</QueryClientProvider>);
}

beforeEach(() => {
  vi.clearAllMocks();
  mocks.fields.mockResolvedValue({ items: [{ id: "field", name: "asset_code", label: "Asset code", type: "text", object_types: [], weight: 100, required: false, is_secret: false }], total: 1 });
  mocks.choices.mockResolvedValue({ items: [{ id: "choices", name: "Locations", extra_choices: [] }], total: 1 });
  mocks.contexts.mockResolvedValue({ items: [{ id: "context", name: "Routing", description: "", weight: 100, active: true, data: "{}" }], total: 1 });
  mocks.bindings.mockResolvedValue({ items: [], total: 0 });
  mocks.mappings.mockResolvedValue({ items: [{ id: "mapping", logical_id: "logical", name: "Device mapping", version: 1, is_latest: true, direction: "pull", provider_id: "provider" }], total: 1 });
});

describe("Inventory secondary row actions", () => {
  it.each([
    { name: "Asset code", page: <CustomFields />, remove: mocks.removeField },
    { name: "Routing", page: <><PageChrome title="Config Contexts" /><ConfigContexts /></>, remove: mocks.removeContext },
    { name: "Device mapping", page: <SyncMappingList />, remove: mocks.removeMapping },
  ])("returns to $name after cancelling delete and retains edit permission", async ({ name, page, remove }) => {
    mount(page);
    const trigger = await screen.findByRole("button", { name: `Actions: ${name}` });
    trigger.focus();
    fireEvent.keyDown(trigger, { key: "Enter" });
    expect(await screen.findByRole("menuitem", { name: "Edit" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("menuitem", { name: "Delete" }));
    fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(trigger).toHaveFocus());
    expect(remove).not.toHaveBeenCalled();
  });

  it.each([
    { name: "Asset code", page: <CustomFields /> },
    { name: "Routing", page: <><PageChrome title="Config Contexts" /><ConfigContexts /></> },
    { name: "Device mapping", page: <SyncMappingList /> },
  ])("has no empty menu for a reader of $name", async ({ name, page }) => {
    mount(page, true);
    await screen.findByText(name);
    expect(screen.queryByRole("button", { name: `Actions: ${name}` })).not.toBeInTheDocument();
  });

  it("lets the config-context child contribute creation without replacing the parent's title", async () => {
    mount(<><PageChrome title="Config Contexts" /><ConfigContexts /></>);
    await screen.findByText("Routing");
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    const header = screen.getByRole("banner");
    fireEvent.click(within(header).getByRole("button", { name: "New Context" }));
    expect(await screen.findByRole("heading", { level: 2, name: "Create Config Context" })).toBeInTheDocument();
  });

  it("starts contexts inactive and accepts NetBox priority overrides within the integer range", async () => {
    mount(<><PageChrome title="Config Contexts" /><ConfigContexts /></>);
    await screen.findByText("Routing");
    fireEvent.click(within(screen.getByRole("banner")).getByRole("button", { name: "New Context" }));
    const weight = await screen.findByRole("spinbutton", { name: "Weight" });
    expect(weight).toHaveValue(1000);
    expect(screen.getByRole("switch")).not.toBeChecked();
    fireEvent.change(screen.getByRole("textbox", { name: /Name/ }), { target: { value: "Tagged override" } });
    const create = screen.getByRole("button", { name: "Create" });
    for (const value of ["-1", "32768", "1.5"]) {
      fireEvent.change(weight, { target: { value } });
      expect(create).toBeDisabled();
      expect(weight).toHaveAttribute("aria-invalid", "true");
    }
    for (const value of ["0", "2000", "32767"]) {
      fireEvent.change(weight, { target: { value } });
      expect(create).toBeEnabled();
      expect(weight).toHaveAttribute("aria-invalid", "false");
    }
  });

  it("rejects JSON arrays, null and scalars as context layers before submission", async () => {
    mount(<><PageChrome title="Config Contexts" /><ConfigContexts /></>);
    await screen.findByText("Routing");
    fireEvent.click(within(screen.getByRole("banner")).getByRole("button", { name: "New Context" }));
    fireEvent.change(await screen.findByRole("textbox", { name: /Name/ }), { target: { value: "Tagged override" } });
    const data = screen.getByRole("textbox", { name: "Context Data (JSON)" });
    const create = screen.getByRole("button", { name: "Create" });
    for (const value of ["[]", "null", "1", '"text"']) {
      fireEvent.change(data, { target: { value } });
      expect(create).toBeDisabled();
      expect(screen.getByText(en.configContexts.invalidJson)).toBeVisible();
    }
    fireEvent.change(data, { target: { value: '{"ntp":{"servers":["192.0.2.1"]}}' } });
    expect(create).toBeEnabled();
  });

  it.each([
    { name: "Asset code", page: <CustomFields /> },
    { name: "Routing", page: <><PageChrome title="Config Contexts" /><ConfigContexts /></> },
  ])("returns to the $name row trigger after leaving its embedded editor", async ({ name, page }) => {
    mount(page);
    const trigger = await screen.findByRole("button", { name: `Actions: ${name}` });
    fireEvent.keyDown(trigger, { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitem", { name: "Edit" }));
    fireEvent.click(await screen.findByRole("button", { name: "Back" }));
    await waitFor(() => expect(trigger).toHaveFocus());
  });

  it("restores creation focus only after its global invoker is no longer inert", async () => {
    mount(<CustomFields />);
    await screen.findByText("Asset code");
    const invoker = within(screen.getByRole("banner")).getByRole("button", { name: "New Field" });
    const nativeFocus = invoker.focus.bind(invoker);
    const interactiveFocusAttempts: boolean[] = [];
    const focus = vi.spyOn(invoker, "focus").mockImplementation(() => {
      // jsdom does not enforce inert; record whether a real browser could focus.
      interactiveFocusAttempts.push(invoker.closest("[inert]") === null);
      nativeFocus();
    });
    invoker.focus();
    interactiveFocusAttempts.length = 0;
    fireEvent.click(invoker);
    expect(invoker.closest("[inert]")).not.toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(interactiveFocusAttempts).toContain(true);
    expect(invoker).toHaveFocus();
    focus.mockRestore();
  });

  it("keeps choice-set removal in its row menu and restores focus on cancel", async () => {
    mocks.fields.mockResolvedValue({ items: [], total: 0 });
    mount(<CustomFields />);
    fireEvent.keyDown(screen.getByRole("tab", { name: /Choice Sets/ }), { key: "Enter" });
    const trigger = await screen.findByRole("button", { name: "Actions: Locations" });
    fireEvent.keyDown(trigger, { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitem", { name: "Delete" }));
    fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(trigger).toHaveFocus());
    expect(mocks.removeChoice).not.toHaveBeenCalled();
  });

  it("confirms removing the last binding of an active context before making it global", async () => {
    mocks.bindings.mockResolvedValue({ items: [{ id: "binding", context_id: "context", scope_type: "site", scope_id: "site" }], total: 1 });
    mount(<ConfigContextScopeBindings contextId="context" />);
    const trigger = await screen.findByRole("button", { name: /Actions:.*Site A/ });
    fireEvent.keyDown(trigger, { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitem", { name: "Remove" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(mocks.removeBinding).not.toHaveBeenCalled();
    fireEvent.click(within(dialog).getByRole("button", { name: "Remove and apply globally" }));
    await waitFor(() => expect(mocks.removeBinding).toHaveBeenCalledWith("binding"));
  });

  it("keeps the last-binding confirmation for a saved-active context with an unsaved inactive draft", async () => {
    mocks.bindings.mockResolvedValue({ items: [{ id: "binding", context_id: "context", scope_type: "site", scope_id: "site" }], total: 1 });
    mount(<><PageChrome title="Config Contexts" /><ConfigContexts /></>);
    const contextTrigger = await screen.findByRole("button", { name: "Actions: Routing" });
    fireEvent.keyDown(contextTrigger, { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitem", { name: "Edit" }));

    fireEvent.click(await screen.findByRole("switch", { name: "active" }));
    expect(screen.getByRole("switch", { name: "inactive" })).toBeInTheDocument();

    const bindingTrigger = await screen.findByRole("button", { name: /Actions:.*Site A/ });
    fireEvent.keyDown(bindingTrigger, { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitem", { name: "Remove" }));
    expect(await screen.findByRole("alertdialog")).toBeInTheDocument();
    expect(mocks.removeBinding).not.toHaveBeenCalled();

    fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Remove and apply globally" }));
    await waitFor(() => expect(mocks.removeBinding).toHaveBeenCalledWith("binding"));
  });

  it("does not claim global application for a saved-inactive context with an unsaved active draft", async () => {
    mocks.contexts.mockResolvedValue({ items: [{ id: "context", name: "Routing", description: "", weight: 100, active: false, data: "{}" }], total: 1 });
    mocks.bindings.mockResolvedValue({ items: [], total: 0 });
    mount(<><PageChrome title="Config Contexts" /><ConfigContexts /></>);
    const contextTrigger = await screen.findByRole("button", { name: "Actions: Routing" });
    fireEvent.keyDown(contextTrigger, { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitem", { name: "Edit" }));

    expect(await screen.findByText(en.configContexts.scope.globalWhenActive)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("switch", { name: "inactive" }));
    expect(screen.getByRole("switch", { name: "active" })).toBeInTheDocument();
    expect(screen.getByText(en.configContexts.scope.globalWhenActive)).toBeInTheDocument();
    expect(screen.queryByText(en.configContexts.scope.appliesGlobally)).not.toBeInTheDocument();
  });

  it("does not expose binding mutations to a config-context reader", async () => {
    mocks.bindings.mockResolvedValue({ items: [{ id: "binding", context_id: "context", scope_type: "site", scope_id: "site" }], total: 1 });
    mount(<ConfigContextScopeBindings contextId="context" />, true);
    await screen.findByText("Site A");
    expect(screen.queryByRole("button", { name: /Actions:/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Add" })).not.toBeInTheDocument();
  });
});
