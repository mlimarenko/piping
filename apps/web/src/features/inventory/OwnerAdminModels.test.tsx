import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { Link, MemoryRouter, Route, Routes } from "react-router";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import { AllPermissions } from "@/test/permissions";
import { PermissionsContext } from "@/shared/auth/permissionsContext";
import EntityCreateEditor from "@/shared/forms/EntityCreateEditor";
import EntityResourceEditorPage from "./EntityResourceEditorPage";
import InventoryResources from "./InventoryResources";
import InventoryResourceDetailPage from "./InventoryResourceDetailPage";
import { CATEGORIZED_ENTITIES, CONFIG_CONTEXTS_RAIL_KEY, CABLING_WORKSTATION_RAIL_KEY, IPAM_WORKSPACE_RAIL_KEY } from "./components/inventory/inventoryCategories";
import { inventoryResourceDetailHref, inventoryResourceHref } from "./resourceLinks";
import { fieldsByTab, inventoryModelCapabilities } from "./components/inventory/inventoryTypes";
import { buildEntityDraft, validateEntityDraft } from "@/shared/forms/entityForm";
import type { InventoryEntityKey, Resource } from "@/shared/api/types";
import { ApiError } from "@/shared/api/client";
import type * as inventoryApi from "./api";
import type * as directoryApi from "@/features/access-control/api";
import IdentitySelection from "@/features/access-control/components/access/IdentitySelection";
import type * as CustomFieldsApi from "./custom-fields-api";
import type * as ConfigContextsApi from "./config-contexts-api";

const mocks = vi.hoisted(() => ({ get: vi.fn(), raw: vi.fn(), children: vi.fn(), definitions: vi.fn(), reveal: vi.fn(), context: vi.fn(), list: vi.fn(), all: vi.fn(), create: vi.fn(), update: vi.fn(), delete: vi.fn(), users: vi.fn(), groups: vi.fn() }));
vi.mock("./api", async (original) => ({ ...await original<typeof inventoryApi>(), getResource: mocks.get, getResourceWithRaw: mocks.raw, listChildResourcesByRouteQuery: mocks.children, listResources: mocks.list, listResourcesForTable: mocks.list, listAllResources: mocks.all, createResource: mocks.create, updateResource: mocks.update, deleteResource: mocks.delete }));
vi.mock("./custom-fields-api", async original => ({ ...await original<typeof CustomFieldsApi>(), listCustomFields: mocks.definitions, listChoiceSets: () => Promise.resolve({ items: [] }), revealEntityCustomFields: mocks.reveal }));
vi.mock("./config-contexts-api", async original => ({ ...await original<typeof ConfigContextsApi>(), getRenderedConfigContext: mocks.context }));
vi.mock("@/features/access-control/api", async (original) => ({ ...await original<typeof directoryApi>(), listAllUsers: mocks.users, listAllGroups: mocks.groups }));

const owner: Resource = { id: "owner-1", name: "Operations", type: "owner", description: "Infrastructure", groupId: "group-1", users: ["user-1"], userGroups: ["identity-group-1"] };
const loadOptions = () => [{ id: "group-1", name: "Infrastructure" }];
const reader = { roles: [], isAdmin: false, can: (action: string) => action === "READ", canAny: () => true, hasRole: () => false };
function mount(children: ReactNode, readOnly = false, client = new QueryClient({ defaultOptions: { queries: { retry: false } } })) {
  return render(<QueryClientProvider client={client}>{readOnly ? <PermissionsContext.Provider value={reader}>{children}</PermissionsContext.Provider> : <AllPermissions>{children}</AllPermissions>}</QueryClientProvider>);
}

/** Mount the canonical full-page editor at its edit route for an entity. */
function mountOwnerEditor(
   entity: InventoryEntityKey,
  editing: Resource,
  readOnly = false,
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } }),
) {
  const path = `/inventory/resources/${encodeURIComponent(entity)}/${editing.id}/edit`;
  const tree = (
    <MemoryRouter initialEntries={[path]}>
      <Routes>
        <Route path="/inventory/resources/:entity/:id/edit" element={<EntityResourceEditorPage />} />
        <Route path="/inventory/resources" element={<div>owner-list</div>} />
      </Routes>
    </MemoryRouter>
  );
  return render(
    <QueryClientProvider client={client}>
      {readOnly ? (
        <PermissionsContext.Provider value={reader}>{tree}</PermissionsContext.Provider>
      ) : (
        <AllPermissions>{tree}</AllPermissions>
      )}
    </QueryClientProvider>,
  );
}
beforeEach(() => {
  vi.clearAllMocks();
  mocks.users.mockResolvedValue([{ id: "user-1", username: "alice", email: "alice@example.test" }]);
  mocks.groups.mockResolvedValue([{ id: "identity-group-1", name: "Network team" }]);
  mocks.get.mockResolvedValue(owner);
  mocks.raw.mockResolvedValue({ resource: owner, raw: { id: owner.id, name: owner.name } });
  mocks.children.mockResolvedValue({ rows: [], total: 0, limit: 50, offset: 0 });
  mocks.definitions.mockResolvedValue({ items: [] });
  mocks.list.mockImplementation((entity: InventoryEntityKey) => Promise.resolve({ items: entity === "Owners" ? [owner] : [], total: entity === "Owners" ? 1 : 0, limit: 25, offset: 0 }));
  mocks.all.mockResolvedValue([{ id: "group-1", name: "Infrastructure", type: "owner_group" }]);
  mocks.delete.mockResolvedValue(undefined);
});

function mountDetail(path = "/inventory/resources/Owners/owner-1", readOnly = true) {
  return mount(<MemoryRouter initialEntries={[path]}><Routes>
    <Route path="/inventory/resources/:entity/:id" element={<InventoryResourceDetailPage />} />
    <Route path="/inventory/resources" element={<p>Returned to inventory</p>} />
  </Routes></MemoryRouter>, readOnly);
}

describe("persisted inventory detail", () => {
  it("review regression: save then View refetches saved detail within production freshness", async () => {
    let stored = { ...owner, description: "Original persisted description" };
    mocks.get.mockImplementation(() => Promise.resolve({ ...stored }));
    mocks.raw.mockImplementation(() => Promise.resolve({ resource: { ...stored }, raw: { id: stored.id, name: stored.name, description: stored.description } }));
    mocks.update.mockImplementation((_entity: InventoryEntityKey, _id: string, changes: Partial<Resource>) => {
      stored = { ...stored, ...changes };
      return Promise.resolve({ ...stored });
    });
    const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: 30_000 } } });
    mount(<MemoryRouter initialEntries={["/inventory/resources/Owners/owner-1"]}><Routes>
      <Route path="/inventory/resources/:entity/:id" element={<InventoryResourceDetailPage />} />
      <Route path="/inventory/resources/:entity/:id/edit" element={<EntityResourceEditorPage />} />
      <Route path="/inventory/resources" element={<Link to="/inventory/resources/Owners/owner-1">View saved owner</Link>} />
    </Routes></MemoryRouter>, false, client);
    await screen.findByText("Original persisted description");
    expect(mocks.raw).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole("link", { name: "Edit" }));
    const description = await screen.findByRole("textbox", { name: "Description" });
    fireEvent.change(description, { target: { value: "Saved by the editor" } });
    fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));
    fireEvent.click(await screen.findByRole("link", { name: "View saved owner" }));
    await screen.findByRole("heading", { level: 1, name: "Operations" });
    await waitFor(() => expect(mocks.raw).toHaveBeenCalledTimes(2), { timeout: 500 });
    expect(await screen.findByText("Saved by the editor")).toBeInTheDocument();
    expect(mocks.update).toHaveBeenCalledWith("Owners", "owner-1", { description: "Saved by the editor" });
    expect(mocks.create).not.toHaveBeenCalled();
    expect(mocks.delete).not.toHaveBeenCalled();
  });

  it.each([404, 500])("review regression: terminal %s retains a non-loading heading and safe Back without authoring", async status => {
    mocks.raw.mockRejectedValue(new ApiError({ status, message: "private terminal diagnostic" }));
    mountDetail("/inventory/resources/Owners/owner-1?return=https%3A%2F%2Foutside.test");
    await screen.findByRole("alert");
    const headings = screen.getAllByRole("heading", { level: 1 });
    expect(headings).toHaveLength(1);
    expect(headings[0]).toHaveTextContent("Owner");
    expect(headings[0]).not.toHaveTextContent(/loading/i);
    expect(screen.getByRole("link", { name: "Back" })).toHaveAttribute("href", "/inventory/resources?entity=Owners");
    expect(screen.queryByRole("link", { name: "Edit" })).toBeNull();
    expect(screen.queryByRole("button", { name: /Save/ })).toBeNull();
    expect(screen.queryByText("private terminal diagnostic")).toBeNull();
  });

  it("review regression: pending detail keeps its entity heading and Back", async () => {
    mocks.raw.mockReturnValue(new Promise(() => {}));
    mountDetail();
    await screen.findByRole("status");
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Owner");
    expect(screen.getByRole("link", { name: "Back" })).toHaveAttribute("href", "/inventory/resources?entity=Owners");
  });

  it("loads only the selected item, keeps directory/unresolved identity values and has no authoring controls", async () => {
    mocks.raw.mockResolvedValue({ resource: { ...owner, users: ["user-1", "unresolved-user"], groupId: "unresolved-owner-group", groupName: "" }, raw: { id: owner.id, name: owner.name, response_only: "Saved only on server", count: 0, enabled: false, unexplained_id: "11111111-1111-4111-8111-111111111111" } });
    mountDetail();
    expect(await screen.findByText("alice, unresolved-user")).toBeInTheDocument();
    expect(await screen.findByText("Network team")).toBeInTheDocument();
    expect(screen.getByText("unresolved-owner-group")).toBeInTheDocument();
    expect(screen.getByText("Saved only on server")).toBeInTheDocument();
    expect(screen.getByText("0")).toBeInTheDocument();
    expect(screen.getByText("false")).toBeInTheDocument();
    expect(screen.getByText("11111111-1111-4111-8111-111111111111")).toBeInTheDocument();
    expect(mocks.raw).toHaveBeenCalledWith("Owners", "owner-1");
    expect(mocks.list).not.toHaveBeenCalled();
    expect(mocks.all).not.toHaveBeenCalled();
    expect(screen.queryByRole("textbox")).toBeNull();
    expect(screen.queryByRole("combobox")).toBeNull();
    expect(screen.queryByRole("link", { name: "Edit" })).toBeNull();
    expect(mocks.update).not.toHaveBeenCalled();
    expect(mocks.create).not.toHaveBeenCalled();
    expect(mocks.delete).not.toHaveBeenCalled();
  });

  it("renders service parent references, address arrays and complete response-only schema", async () => {
    mocks.raw.mockResolvedValue({ resource: { id: "service-1", name: "HTTPS", type: "service", parentObject: { objectType: "dcim.device", objectId: "device-1", label: "Router" }, serviceIpAddressIds: ["ip-1", "ip-2"], ports: "443, 8443" }, raw: { schema: { custom: { nested: "full schema value" } }, comments: "long description\n".repeat(20) } });
    mountDetail("/inventory/resources/Services/service-1");
    expect(await screen.findByRole("link", { name: "Router (dcim.device)" })).toHaveAttribute("href", "/inventory/resources/Devices/device-1");
    expect(screen.getByRole("link", { name: "ip-1" })).toHaveAttribute("href", "/inventory/resources/IP%20Addresses/ip-1");
    expect(screen.getByRole("link", { name: "ip-2" })).toBeInTheDocument();
    expect(screen.getByText(/full schema value/)).toBeInTheDocument();
    expect(screen.getByText(/long description/).textContent).toBe("long description\n".repeat(20));
  });

  it("keeps backend read-only records readable even with write grants, without Edit", async () => {
    mocks.raw.mockResolvedValue({ resource: { id: "profile-1", name: "Rack profile", type: "module_type_profile" }, raw: { description: "Response-only profile description", schema: { required: ["size"], properties: { size: { type: "number" } } } } });
    mountDetail("/inventory/resources/Module%20Type%20Profiles/profile-1", false);
    await screen.findByRole("heading", { level: 1, name: "Rack profile" });
    expect(screen.getByText(/Required:/)).toBeInTheDocument();
    expect(screen.getByText("Response-only profile description")).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Edit" })).toBeNull();
    expect(screen.queryByRole("textbox")).toBeNull();
  });

  it("offers a separately gated authoring link and restores a safe return URL", async () => {
    mountDetail(`/inventory/resources/Owners/owner-1?return=${encodeURIComponent("/inventory/resources?entity=Owners&search=Ops#table")}`, false);
    expect(await screen.findByRole("link", { name: "Edit" })).toHaveAttribute("href", `/inventory/resources/Owners/owner-1/edit?return=${encodeURIComponent("/inventory/resources?entity=Owners&search=Ops#table")}`);
    fireEvent.click(screen.getByRole("link", { name: "Back" }));
    expect(await screen.findByText("Returned to inventory")).toBeInTheDocument();
    expect(inventoryResourceHref("Owners", "owner-1")).toBe("/inventory/resources/Owners/owner-1/edit");
  });

  it("distinguishes loading, safe failure/retry and missing data", async () => {
    mocks.raw.mockRejectedValueOnce(new ApiError({ status: 500, message: "private backend diagnostic" }));
    mountDetail();
    expect(screen.getByRole("status")).toBeInTheDocument();
    expect(await screen.findByRole("alert")).toHaveTextContent("Failed to load this resource");
    expect(screen.queryByText("private backend diagnostic")).toBeNull();
    mocks.raw.mockResolvedValue({ resource: owner, raw: {} });
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByRole("heading", { level: 1, name: "Operations" })).toBeInTheDocument();
  });

  it("renders 404 without retrying a missing record and rejects external return destinations", async () => {
    mocks.raw.mockRejectedValue(new ApiError({ status: 404, message: "private absent" }));
    mountDetail("/inventory/resources/Owners/owner-1?return=https%3A%2F%2Foutside.test");
    await screen.findByRole("alert");
    expect(screen.queryByRole("button", { name: "Retry" })).toBeNull();
    expect(screen.getByRole("link", { name: "Back" })).toHaveAttribute("href", "/inventory/resources?entity=Owners");
    expect(screen.queryByText("private absent")).toBeNull();
  });

  it.each([CONFIG_CONTEXTS_RAIL_KEY, IPAM_WORKSPACE_RAIL_KEY, CABLING_WORKSTATION_RAIL_KEY, "not-an-entity"])("fails closed for non-resource %s", async entity => {
    mountDetail(`/inventory/resources/${encodeURIComponent(entity)}/row-1`);
    expect(await screen.findByRole("alert")).toBeInTheDocument();
    expect(mocks.raw).not.toHaveBeenCalled();
    expect(inventoryResourceDetailHref(entity, "row-1")).toBeNull();
  });

  it("shows typed visible custom values but never raw hidden/unknown/secret data or authoring auxiliaries", async () => {
    mocks.definitions.mockResolvedValue({ items: [
      { id: "1", name: "flag", label: "Saved flag", field_type: "boolean", ui_visible: "always", weight: 0, is_secret: false },
      { id: "2", name: "number", label: "Saved zero", field_type: "integer", ui_visible: "if-set", weight: 1, is_secret: false },
      { id: "3", name: "hidden", label: "Hidden field", field_type: "text", ui_visible: "hidden", weight: 2, is_secret: false },
      { id: "4", name: "secret", label: "Protected field", field_type: "text", ui_visible: "always", weight: 3, is_secret: true },
      { id: "5", name: "unset", label: "Unset field", field_type: "text", ui_visible: "if-set", weight: 4, is_secret: false },
    ] });
    mocks.raw.mockResolvedValue({ resource: { id: "site-1", name: "Site", type: "site" }, raw: { custom_fields: { flag: false, number: 0, hidden: "hidden value", secret: "untrusted plaintext", unknown: "unknown value", unset: null }, local_context_data: { site_flag: false } } });
    mountDetail("/inventory/resources/Sites/site-1");
    expect(await screen.findByText("Saved flag")).toBeInTheDocument();
    expect(screen.getByText("false")).toBeInTheDocument();
    expect(screen.getByText("0")).toBeInTheDocument();
    expect(screen.getByText("****")).toBeInTheDocument();
    expect(screen.queryByText(/hidden value|untrusted plaintext|unknown value|Unset field/)).toBeNull();
    expect(screen.queryByRole("textbox")).toBeNull();
    expect(screen.queryByRole("switch")).toBeNull();
    expect(mocks.reveal).not.toHaveBeenCalled();
    expect(screen.getByText(/Site Flag: false/)).toBeInTheDocument();
    mocks.reveal.mockRejectedValue(new ApiError({ status: 403, message: "private reveal detail" }));
    fireEvent.click(screen.getByRole("button", { name: "Reveal" }));
    await screen.findByRole("alert");
    expect(screen.queryByText("private reveal detail")).toBeNull();
    expect(screen.getByText("****")).toBeInTheDocument();
    expect(mocks.update).not.toHaveBeenCalled();
    mocks.reveal.mockResolvedValue({ secret: "authorized reveal value", hidden: "never display another field" });
    fireEvent.click(screen.getByRole("button", { name: "Reveal" }));
    expect(await screen.findByText("authorized reveal value")).toBeInTheDocument();
    expect(mocks.reveal).toHaveBeenLastCalledWith("sites", "site-1");
    expect(screen.queryByText("never display another field")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Hide" }));
    expect(screen.queryByText("authorized reveal value")).toBeNull();
  });

  it("replaces read-only /edit custom/local-context authoring auxiliaries with the saved read body", async () => {
    mocks.raw.mockResolvedValue({ resource: { id: "site-1", name: "Site", type: "site" }, raw: { local_context_data: { saved: true }, custom_fields: { unknown: "must not fall back to raw" } } });
    mountOwnerEditor("Sites", { id: "site-1", name: "Site", type: "site" }, true);
    await screen.findByText("Saved: true");
    await screen.findByRole("heading", { level: 1, name: "Site" });
    expect(screen.queryByRole("textbox")).toBeNull();
    expect(screen.queryByRole("switch")).toBeNull();
    expect(screen.queryByRole("combobox")).toBeNull();
    expect(screen.queryByText("must not fall back to raw")).toBeNull();
    expect(screen.getByText("Saved: true")).toBeInTheDocument();
    expect(mocks.get).not.toHaveBeenCalled();
    expect(mocks.update).not.toHaveBeenCalled();
  });

  it("reads definition pages to completion before exposing custom fields", async () => {
    mocks.definitions.mockImplementation(({ offset }: { offset: number }) => Promise.resolve({
      items: offset === 0 ? [{ id: "hidden", name: "hidden", label: "Not visible", field_type: "text", ui_visible: "hidden", weight: 0, is_secret: false }]
        : [{ id: "last", name: "last", label: "Last page field", field_type: "json", ui_visible: "always", weight: 1, is_secret: false }], total: 2, limit: 1, offset,
    }));
    mocks.raw.mockResolvedValue({ resource: { id: "site-1", name: "Site", type: "site" }, raw: { custom_fields: { hidden: "must remain hidden", last: { value: false } } } });
    mountDetail("/inventory/resources/Sites/site-1");
    expect(await screen.findByText("Last page field")).toBeInTheDocument();
    expect(screen.getByText("Value: false")).toBeInTheDocument();
    expect(screen.queryByText("must remain hidden")).toBeNull();
    expect(mocks.definitions).toHaveBeenNthCalledWith(2, { limit: 100, offset: 1 });
  });

  it("retains saved local context and the canonical rendered config-context consumer", async () => {
    mocks.context.mockResolvedValue({ rendered: { merged: "inherited configuration" }, sources: [], local_context_data: { saved: 0 } });
    mocks.raw.mockResolvedValue({ resource: { id: "device-1", name: "Router", type: "device" }, raw: { local_context_data: { saved: 0 } } });
    mountDetail("/inventory/resources/Devices/device-1");
    expect(await screen.findByText(/inherited configuration/)).toBeInTheDocument();
    expect(screen.getByText("Saved: 0")).toBeInTheDocument();
    expect(mocks.context).toHaveBeenCalledWith("device", "device-1");
    expect(screen.queryByRole("textbox")).toBeNull();
  });
});

describe("Owner AdminModel native forms", () => {
  it("preserves peer ownership changes when the operator edits only description", async () => {
    mocks.update.mockResolvedValue(owner);
    mountOwnerEditor("Owners", owner);
    fireEvent.change(await screen.findByRole("textbox", { name: "Description" }), { target: { value: "Operator changed description" } });
    fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));
    // Only the changed field is sent — peer ownership/identity values are not resent.
    await waitFor(() =>
      expect(mocks.update).toHaveBeenCalledWith("Owners", owner.id, { description: "Operator changed description" }),
    );
  });

  it("does not offer a shared Clear action without an explicit persistence contract", () => {
    mountOwnerEditor("Platforms", { id: "p", name: "Platform", type: "platform", manufacturerId: "m" });
    expect(screen.queryByRole("button", { name: /Clear Manufacturer/i })).not.toBeInTheDocument();
  });

  it.each(["user", "group"] as const)("refreshes %s options through canonical directory invalidation", async (kind) => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const list = kind === "user" ? mocks.users : mocks.groups;
    const directory = kind === "user" ? "users" : "groups";
    const raw = kind === "user" ? { id: "new", username: "New directory entry", email: "new@example.test" } : { id: "new", name: "New directory entry" };
    client.setQueryData(["access", directory], [raw]);
    mount(<IdentitySelection kind={kind} label="Directory" value={[]} onChange={vi.fn()} />, false, client);
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Directory" })).not.toBeDisabled());
    expect(client.getQueryCache().findAll({ queryKey: ["access", directory] })).toHaveLength(2);
    list.mockResolvedValue([raw]);
    await act(async () => { await client.invalidateQueries({ queryKey: ["access", directory] }); });
    await waitFor(() => expect(list).toHaveBeenCalledTimes(2));
    fireEvent.click(screen.getByRole("combobox", { name: "Directory" }));
    expect(await screen.findByRole("option", { name: "New directory entry" })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: /alice|Network team/ })).not.toBeInTheDocument();
    expect(client.getQueryData(["access", directory])).toEqual([raw]);
  });
  it.each([
    { kind: "user", id: "user-1", label: "alice" },
    { kind: "group", id: "identity-group-1", label: "Network team" },
  ] as const)("displays canonical $kind labels while preserving unresolved assignments", async ({ kind, id, label }) => {
    mount(<IdentitySelection kind={kind} label="Directory" value={[id, "unresolved-id"]} />);
    expect(await screen.findByText(`${label}, unresolved-id`)).toBeInTheDocument();
    expect(screen.queryByText(/\[object Object\]/)).not.toBeInTheDocument();
  });

  it("displays an empty assignment as an em dash", () => {
    mount(<IdentitySelection kind="user" label="Directory" value={[]} />);
    expect(screen.getByText("—")).toBeInTheDocument();
  });
  it.each(["Owners", "Owner Groups"] as const)("pins %s fields, limits and unsupported capabilities", (entity) => {
    expect(inventoryModelCapabilities(entity)).toEqual({ tags: false, custom_fields: false, change_dates: false });
    const keys = (fieldsByTab[entity] ?? []).map((field) => field.key);
    for (const key of ["tags", "customFields", "createdAt", "updatedAt", "status", "slug"]) expect(keys).not.toContain(key);
    if (entity === "Owner Groups") expect(keys).toEqual(["description"]);
    const draft = { ...buildEntityDraft(entity), name: "Ops", description: "x".repeat(201) };
    expect(validateEntityDraft(entity, draft, (key) => key)).toHaveProperty("description");
  });

  it("creates an owner with group and identity UUID selections through the native create dialog", async () => {
    const create = vi.fn((_entity: InventoryEntityKey, resource: Resource) => Promise.resolve({ ...resource, id: "created" }));
    mount(<EntityCreateEditor open onOpenChange={vi.fn()} rootEntity="Owners" createEntity={create} loadOptions={loadOptions} onRootCreated={vi.fn()} />);
    fireEvent.change(screen.getByRole("textbox", { name: "Name" }), { target: { value: "Ops" } });
    fireEvent.click(screen.getByRole("combobox", { name: "Owner group" }));
    fireEvent.click(await screen.findByRole("option", { name: /Infrastructure/ }));
    const users = screen.getByRole("combobox", { name: "Users" });
    await waitFor(() => expect(users).not.toBeDisabled());
    fireEvent.click(users);
    fireEvent.click(await screen.findByRole("option", { name: "alice" }));
    fireEvent.click(users);
    fireEvent.click(screen.getByRole("combobox", { name: "Groups" }));
    fireEvent.click(await screen.findByRole("option", { name: "Network team" }));
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(create).toHaveBeenCalled());
    expect(create.mock.calls[0]).toEqual(["Owners", expect.objectContaining({ name: "Ops", groupId: "group-1", users: ["user-1"], userGroups: ["identity-group-1"], description: "" })]);
  });

  it("edits an owner and clears group, both identity collections and description explicitly", async () => {
    mocks.update.mockResolvedValue(owner);
    mountOwnerEditor("Owners", owner);
    await screen.findByText("alice");
    fireEvent.click(screen.getByRole("button", { name: "Clear Owner group" }));
    fireEvent.click(screen.getByRole("button", { name: "Remove alice" }));
    fireEvent.click(screen.getByRole("button", { name: "Remove Network team" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Description" }), { target: { value: "" } });
    fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));
    await waitFor(() =>
      expect(mocks.update).toHaveBeenCalledWith(
        "Owners",
        owner.id,
        expect.objectContaining({ groupId: null, users: [], userGroups: [], description: "" }),
      ),
    );
    expect(screen.queryByText("Custom fields")).not.toBeInTheDocument();
  });

  it("creates and edits a flat owner group with no identity or AdminModel fields", async () => {
    const create = vi.fn((_entity: InventoryEntityKey, resource: Resource) => Promise.resolve({ ...resource, id: "created" }));
    const view = mount(<EntityCreateEditor open onOpenChange={vi.fn()} rootEntity="Owner Groups" createEntity={create} loadOptions={loadOptions} onRootCreated={vi.fn()} />);
    fireEvent.change(screen.getByRole("textbox", { name: "Name" }), { target: { value: "Core operations" } });
    fireEvent.change(screen.getByRole("textbox", { name: "Description" }), { target: { value: "Infrastructure" } });
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(create).toHaveBeenCalledWith("Owner Groups", expect.objectContaining({ name: "Core operations", description: "Infrastructure" })));
    view.unmount();
    mocks.get.mockResolvedValue({ id: "g", name: "Core operations", description: "Infrastructure", type: "owner_group" });
    mocks.update.mockResolvedValue({ id: "g", name: "Operations", description: "", type: "owner_group" });
    mountOwnerEditor("Owner Groups", { id: "g", name: "Core operations", description: "Infrastructure", type: "owner_group" });
    fireEvent.change(await screen.findByRole("textbox", { name: "Name" }), { target: { value: "Operations" } });
    fireEvent.change(screen.getByRole("textbox", { name: "Description" }), { target: { value: "" } });
    fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));
    await waitFor(() =>
      expect(mocks.update).toHaveBeenCalledWith(
        "Owner Groups",
        "g",
        expect.objectContaining({ name: "Operations", description: "" }),
      ),
    );
    expect(mocks.update.mock.calls[0]?.[2]).not.toHaveProperty("users");
  });

  it("preserves assignments when directory resolution fails and shows only safe feedback", async () => {
    mocks.users.mockRejectedValue(new Error("raw directory failure"));
    mocks.groups.mockRejectedValue(new Error("raw directory failure"));
    mocks.update.mockResolvedValue(owner);
    mountOwnerEditor("Owners", owner);
    await waitFor(() => expect(screen.getAllByRole("status")).toHaveLength(2));
    for (const status of screen.getAllByRole("status")) expect(status.tagName).toBe("OUTPUT");
    expect(screen.getByRole("combobox", { name: "Users" })).toBeDisabled();
    expect(screen.getByRole("combobox", { name: "Groups" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));
    // Unchanged (disabled) identity fields are not resent.
    await waitFor(() => expect(mocks.update).toHaveBeenCalledWith("Owners", owner.id, {}));
    expect(screen.queryByText("raw directory failure")).not.toBeInTheDocument();
  });

  it.each(["Owners", "Owner Groups"] as const)("renders %s read-only detail without save or inline creation", async (entity) => {
    mocks.get.mockResolvedValue(owner);
    mountOwnerEditor(entity, owner, true);
    await screen.findByRole("heading", { level: 1, name: "Operations" });
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Save Changes" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Create new/ })).not.toBeInTheDocument();
    expect(screen.getByText("Infrastructure")).toBeInTheDocument();
    if (entity === "Owners") expect(await screen.findByText("alice")).toBeInTheDocument();
    else expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
  });

  it.each(CATEGORIZED_ENTITIES)("makes real registry %s View reachable independently of write support", async entity => {
    mocks.list.mockResolvedValue({ items: [{ id: "imported-1", name: "Imported record", type: "tag" }], total: 1, limit: 25, offset: 0 });
    mocks.raw.mockResolvedValue({ resource: { id: "imported-1", name: "Imported record", type: entity }, raw: { id: "imported-1", name: "Imported record" } });
    mount(<MemoryRouter initialEntries={[`/inventory/resources?entity=${encodeURIComponent(entity)}`]}><Routes>
      <Route path="/inventory/resources" element={<InventoryResources />} />
      <Route path="/inventory/resources/:entity/:id" element={<InventoryResourceDetailPage />} />
    </Routes></MemoryRouter>, true);
    await screen.findByText("Imported record");
    fireEvent.keyDown(screen.getByRole("button", { name: "Actions: Imported record" }), { key: "Enter" });
    const view = await screen.findByRole("menuitem", { name: "View" });
    expect(view).toBeInTheDocument();
    expect(screen.queryByRole("menuitem", { name: "Edit" })).toBeNull();
    expect(screen.queryByRole("menuitem", { name: "Delete" })).toBeNull();
    expect(inventoryResourceDetailHref(entity, "imported-1")).toBe(`/inventory/resources/${encodeURIComponent(entity)}/imported-1`);
    fireEvent.click(view);
    expect(await screen.findByRole("heading", { level: 1, name: "Imported record" })).toBeInTheDocument();
    expect(mocks.raw).toHaveBeenCalledWith(entity, "imported-1");
  });

  it("offers read-only native list/detail with no write affordances", async () => {
    mount(
      <MemoryRouter initialEntries={["/inventory/resources?entity=Owners"]}>
        <Routes>
          <Route path="/inventory/resources" element={<InventoryResources />} />
          <Route
            path="/inventory/resources/:entity/:id"
            element={<div>owner-detail-page</div>}
          />
        </Routes>
      </MemoryRouter>,
      true,
    );
    await screen.findByText("Operations");
    expect(screen.queryByRole("button", { name: "Create" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Edit" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Delete" })).not.toBeInTheDocument();
    // View owns persisted detail; inspector/edit links remain authoring links.
    fireEvent.keyDown(screen.getByRole("button", { name: "Actions: Operations" }), { key: "Enter" });
    expect(screen.queryByRole("menuitem", { name: "Edit" })).not.toBeInTheDocument();
    expect(screen.queryByRole("menuitem", { name: "Delete" })).not.toBeInTheDocument();
    fireEvent.click(await screen.findByRole("menuitem", { name: "View" }));
    expect(await screen.findByText("owner-detail-page")).toBeInTheDocument();
  });

  it.each(["Owners", "Owner Groups"] as const)("renders %s deletion references as localized conflicts, never the raw backend error", async (entity) => {
    mocks.list.mockResolvedValue({ items: [owner], total: 1, limit: 25, offset: 0 });
    mocks.delete.mockRejectedValue(new ApiError({ message: "SQL internal owner reference", code: "CONFLICT", status: 409, reasonCode: "DELETE_BLOCKED", details: { total_count: 1, blockers: [{ table: "dcim_device", entity_key: "Devices", count: 1, examples: [{ id: "device-1", label: "Router" }] }] } }));
    mount(<MemoryRouter initialEntries={[`/inventory/resources?entity=${entity}`]}><InventoryResources /></MemoryRouter>);
    await screen.findByText("Operations");
    fireEvent.keyDown(screen.getByRole("button", { name: "Actions: Operations" }), { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitem", { name: "Delete" }));
    fireEvent.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Delete" }));
    await screen.findByText("Router");
    expect(screen.queryByText("SQL internal owner reference")).not.toBeInTheDocument();
    expect(within(screen.getByRole("alertdialog")).queryByRole("button", { name: "Delete" })).not.toBeInTheDocument();
  });
});
