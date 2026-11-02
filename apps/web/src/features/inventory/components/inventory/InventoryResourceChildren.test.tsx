import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes, useNavigate, useParams } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ReactNode } from "react";
import type { InventoryEntityKey } from "@/shared/api/types";
import type * as Client from "@/shared/api/client";
import type * as InventoryApi from "@/features/inventory/api";
import type * as SettingsApi from "@/shared/settings/api";
import type { TableColumnVisibilityValue } from "@/shared/settings/registry";
import { AllPermissions } from "@/test/permissions";
import "@/shared/i18n";
import { PermissionsContext } from "@/shared/auth/permissionsContext";
import InventoryResourceDetailPage from "@/features/inventory/InventoryResourceDetailPage";
import EntityResourceEditorPage from "@/features/inventory/EntityResourceEditorPage";
import InventoryResourceChildren from "./InventoryResourceChildren";
import { inventoryResourceChildRelations, listInventoryResourceChildren } from "./inventoryResourceChildLists";

const api = vi.hoisted(() => ({ get: vi.fn(), post: vi.fn(), put: vi.fn(), patch: vi.fn(), remove: vi.fn(), detail: vi.fn(), resource: vi.fn(), update: vi.fn() }));
const settings = vi.hoisted(() => ({ read: vi.fn(), write: vi.fn() }));
vi.mock("@/shared/settings/api", async original => ({ ...await original<typeof SettingsApi>(), fetchEffectiveSetting: settings.read, upsertUserSetting: settings.write }));
vi.mock("@/shared/api/client", async original => {
  const actual = await original<typeof Client>();
  return { ...actual, apiClient: { ...actual.apiClient, GET: api.get, POST: api.post, PUT: api.put, PATCH: api.patch, DELETE: api.remove } };
});
vi.mock("@/features/inventory/api", async original => ({ ...await original<typeof InventoryApi>(), getResourceWithRaw: api.detail, getResource: api.resource, updateResource: api.update }));
// The unchanged rendered-context channel is covered by first-slice tests; this
// integration exercises the new child consumer through the real detail owner.
vi.mock("@/features/inventory/components/config-contexts/RenderedConfigContextPanel", () => ({ default: () => null }));

type Request = { params: { query: Record<string, string | number | boolean | undefined> } };
const routes = {
  interfaces: "/api/v1/inventory/interfaces", modules: "/api/v1/inventory/modules",
  vm: "/api/v1/inventory/vm-interfaces", ips: "/api/v1/inventory/ip-addresses",
};
const contentTypes = [{ id: "ct-interface", token: "dcim.interface" }, { id: "ct-vm-interface", token: "virtualization.vminterface" }];
function page(items: Record<string, unknown>[], total = items.length, offset = 0) {
  return { data: { items, total, limit: 50, offset } };
}
function client() { return new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: 30_000 } } }); }
function mount(children: ReactNode, path = "/inventory/resources/Devices/device-1?origin=list#selected") {
  const queryClient = client();
  return render(<QueryClientProvider client={queryClient}><AllPermissions><MemoryRouter initialEntries={[path]}>{children}</MemoryRouter></AllPermissions></QueryClientProvider>);
}
function children(entity: InventoryEntityKey, id = "parent-1") {
  return <InventoryResourceChildren entity={entity} parent={{ id, name: "Selected parent", type: "parent" }} />;
}
let columnPreferences: TableColumnVisibilityValue = {};
beforeEach(() => {
  Object.values(api).forEach(mock => mock.mockReset());
  columnPreferences = {};
  settings.read.mockReset(); settings.write.mockReset();
  settings.read.mockImplementation((key: string) => {
    if (key !== "table.column_visibility") throw new Error(`Unexpected setting ${key}`);
    return Promise.resolve({ key, value: structuredClone(columnPreferences), source: "user" });
  });
  settings.write.mockImplementation((key: string, value: TableColumnVisibilityValue) => {
    if (key !== "table.column_visibility") throw new Error(`Unexpected setting write ${key}`);
    columnPreferences = structuredClone(value);
    return Promise.resolve();
  });
  api.get.mockImplementation((path: string) => {
    if (path === "/api/v1/inventory/content-types") return Promise.resolve({ data: contentTypes });
    if (path === routes.interfaces) return Promise.resolve(page([{ id: "interface-1", name: "eth0", description: "Device port" }]));
    if (path === routes.modules) return Promise.resolve(page([{ id: "module-1", name: "Module", description: "Installed module" }]));
    if (path === routes.vm) return Promise.resolve(page([{ id: "vmi-1", name: "ens3", description: "VM port" }]));
    if (path === routes.ips) return Promise.resolve(page([{ id: "ip-1", address: "192.0.2.10/24", status: "active" }]));
    throw new Error(`Unexpected read route ${path}`);
  });
  api.detail.mockImplementation((entity: InventoryEntityKey, id: string) => Promise.resolve({ resource: { id, name: entity === "Devices" ? "Router" : entity === "Virtual Machines" ? "Test VM" : entity === "Interfaces" ? "eth0" : entity === "VM Interfaces" ? "ens3" : "192.0.2.10/24", type: "resource" }, raw: {} }));
});

describe("metadata-derived read-only resource children", () => {
  it.each([
    ["Devices", routes.interfaces, "device_id"],
    ["Devices", routes.modules, "device_id"],
    ["Virtual Machines", routes.vm, "virtual_machine_id"],
    ["Interfaces", routes.ips, "assigned_object_id"],
    ["VM Interfaces", routes.ips, "assigned_object_id"],
  ] as const)("pins %s children on %s using %s", async (entity, route, filter) => {
    mount(children(entity), `/inventory/resources/${encodeURIComponent(entity)}/parent-1?${filter}=foreign&assigned_object_type_id=foreign`);
    await screen.findByRole("link", { name: entity.includes("Interface") ? "192.0.2.10/24" : entity === "Virtual Machines" ? "ens3" : "eth0" });
    const calls = api.get.mock.calls as [string, Request][];
    const request = calls.find(([path]) => path === route)?.[1];
    expect(request?.params.query).toMatchObject({ [filter]: "parent-1", offset: 0, limit: 50 });
    if (entity.includes("Interface")) expect(request?.params.query).toHaveProperty("assigned_object_type_id", entity === "Interfaces" ? "ct-interface" : "ct-vm-interface");
    expect(request?.params.query).not.toHaveProperty("interface_id");
    expect(screen.getAllByText("Selected parent · parent-1").length).toBeGreaterThan(0);
    expect(screen.queryByRole("button", { name: /Create|Delete|Edit|Save|Filter|Sort/i })).toBeNull();
    expect(api.post).not.toHaveBeenCalled(); expect(api.put).not.toHaveBeenCalled(); expect(api.patch).not.toHaveBeenCalled(); expect(api.remove).not.toHaveBeenCalled();
  });

  it("pins parent and polymorphic type after hostile composed parameters and preserves the server envelope", async () => {
    const spec = inventoryResourceChildRelations("Interfaces")[0]!;
    api.get.mockResolvedValue(page([{ id: "ip-1", address: "192.0.2.10/24" }], 127, 50));
    const result = await listInventoryResourceChildren(spec, "interface-1", { assigned_object_id: "foreign", assigned_object_type_id: "foreign-type", limit: 50, offset: 50 }, "ct-interface");
    expect(api.get).toHaveBeenCalledWith(routes.ips, { params: { query: { assigned_object_id: "interface-1", assigned_object_type_id: "ct-interface", limit: 50, offset: 50 } } });
    expect(result).toMatchObject({ total: 127, limit: 50, offset: 50, items: [{ id: "ip-1" }] });
  });

  it("uses server totals and offset for Next page without offering unsupported sorting", async () => {
    api.get.mockImplementation((_path: string, request: Request) => Promise.resolve(page([{ id: `vmi-${request.params.query.offset}`, name: `port-${request.params.query.offset}` }], 127, Number(request.params.query.offset))));
    mount(children("Virtual Machines"));
    await screen.findByRole("link", { name: "port-0" });
    expect(screen.getByText(/1.*50.*127/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Next page" }));
    expect(await screen.findByRole("link", { name: "port-50" })).toBeInTheDocument();
    expect(api.get).toHaveBeenLastCalledWith(routes.vm, { params: { query: expect.objectContaining({ virtual_machine_id: "parent-1", offset: 50, limit: 50 }) } });
    expect(screen.getByText(/51.*100.*127/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /sort/i })).toBeNull();
  });

  it("resets pagination on parent switch and never resurrects late previous-parent rows", async () => {
    let finishOld!: (value: ReturnType<typeof page>) => void;
    api.get.mockImplementation((_path: string, request: Request) => {
      const query = request.params.query;
      if (query.virtual_machine_id === "first" && query.offset === 50) return new Promise(resolve => { finishOld = resolve; });
      return Promise.resolve(page([{ id: String(query.virtual_machine_id), name: `${query.virtual_machine_id} port` }], query.virtual_machine_id === "first" ? 127 : 1));
    });
    function Parent() { const { id = "" } = useParams(); return children("Virtual Machines", id); }
    function Navigation() { const navigate = useNavigate(); return <button onClick={() => void navigate("/inventory/resources/Virtual%20Machines/second")}>Switch parent</button>; }
    mount(<><Navigation /><Routes><Route path="/inventory/resources/:entity/:id" element={<Parent />} /></Routes></>, "/inventory/resources/Virtual%20Machines/first");
    await screen.findByRole("link", { name: "first port" });
    fireEvent.click(screen.getByRole("button", { name: "Next page" }));
    await waitFor(() => expect(api.get).toHaveBeenCalledWith(routes.vm, { params: { query: expect.objectContaining({ virtual_machine_id: "first", offset: 50 }) } }));
    fireEvent.click(screen.getByRole("button", { name: "Switch parent" }));
    await screen.findByRole("link", { name: "second port" });
    expect(api.get).toHaveBeenLastCalledWith(routes.vm, { params: { query: expect.objectContaining({ virtual_machine_id: "second", offset: 0 }) } });
    await act(async () => { finishOld(page([{ id: "late", name: "late first-parent port" }], 127, 50)); await Promise.resolve(); });
    expect(screen.queryByRole("link", { name: "late first-parent port" })).toBeNull();
    expect(screen.queryByRole("link", { name: "first port" })).toBeNull();
    expect(screen.getByRole("link", { name: "second port" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Previous page" })).toBeDisabled();
  });

  it("distinguishes failed load from empty and recovers through Retry", async () => {
    api.get.mockRejectedValueOnce(new Error("private child diagnostic"));
    mount(children("Virtual Machines"));
    expect(await screen.findByRole("button", { name: /retry|try again/i })).toBeInTheDocument();
    expect(screen.queryByText("private child diagnostic")).toBeNull();
    expect(screen.queryByText(/0.*of.*0/)).toBeNull();
    api.get.mockResolvedValue(page([]));
    fireEvent.click(screen.getByRole("button", { name: /retry|try again/i }));
    expect(await screen.findByText(/No .* yet/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Next page" })).toBeDisabled();
  });

  it("fails closed when polymorphic type metadata is unavailable rather than issuing an untyped IP query", async () => {
    api.get.mockResolvedValue({ data: [] });
    mount(children("Interfaces"));
    await screen.findByRole("button", { name: /retry|try again/i });
    expect(api.get).toHaveBeenCalledTimes(1);
    expect(api.get).toHaveBeenCalledWith("/api/v1/inventory/content-types");
  });

  it.each([
    { entity: "Devices", id: "device-1", name: "Router", child: "Interfaces", childId: "interface-1", port: "eth0" },
    { entity: "Virtual Machines", id: "vm-1", name: "Test VM", child: "VM Interfaces", childId: "vmi-1", port: "ens3" },
  ])("drills $entity to interface to IP and Back using only read links", async ({ entity, id, name, child, childId, port: portName }) => {
    const parent = `/inventory/resources/${encodeURIComponent(entity)}/${id}?origin=list#selected`;
    mount(<Routes><Route path="/inventory/resources/:entity/:id" element={<InventoryResourceDetailPage />} /></Routes>, parent);
    const port = await screen.findByRole("link", { name: portName });
    expect(port).toHaveAttribute("href", `/inventory/resources/${encodeURIComponent(child)}/${childId}?return=${encodeURIComponent(parent)}`);
    fireEvent.click(port);
    await screen.findByRole("heading", { level: 1, name: portName });
    const ip = await screen.findByRole("link", { name: "192.0.2.10/24" });
    fireEvent.click(ip);
    await screen.findByRole("heading", { level: 1, name: "192.0.2.10/24" });
    fireEvent.click(screen.getByRole("link", { name: "Back" }));
    await screen.findByRole("heading", { level: 1, name: portName });
    fireEvent.click(screen.getByRole("link", { name: "Back" }));
    await screen.findByRole("heading", { level: 1, name });
    expect(api.detail).toHaveBeenCalledWith("IP Addresses", "ip-1");
    expect(api.post).not.toHaveBeenCalled(); expect(api.put).not.toHaveBeenCalled(); expect(api.patch).not.toHaveBeenCalled(); expect(api.remove).not.toHaveBeenCalled();
    expect(within(screen.getByRole("region", { name: new RegExp(`^${child}$`, "i") })).getByText(`${name} · ${id}`)).toBeInTheDocument();
  });

  it("does not mount children or read data without READ INVENTORY", () => {
    render(<QueryClientProvider client={client()}><PermissionsContext.Provider value={{ roles: [], isAdmin: false, can: () => false, canAny: () => false, hasRole: () => false }}><MemoryRouter>{children("Devices")}</MemoryRouter></PermissionsContext.Provider></QueryClientProvider>);
    expect(screen.queryByRole("table")).toBeNull();
    expect(api.get).not.toHaveBeenCalled();
    expect(settings.read).not.toHaveBeenCalled();
  });

  it("retains relation column preferences across parent records without colliding with sibling or standalone inventory tables", async () => {
    columnPreferences = { "inventory.Interfaces": { hidden: ["description"] } };
    function Parent() {
      const { entity, id = "" } = useParams();
      return children(entity === "Devices" ? "Devices" : "Virtual Machines", id);
    }
    function Navigation() {
      const navigate = useNavigate();
      return <><button onClick={() => void navigate("/inventory/resources/Devices/second")}>Next parent record</button><button onClick={() => void navigate("/inventory/resources/Virtual%20Machines/vm-1")}>VM parent</button></>;
    }
    mount(<><Navigation /><Routes><Route path="/inventory/resources/:entity/:id" element={<Parent />} /></Routes></>, "/inventory/resources/Devices/first");
    const interfaces = screen.getByRole("region", { name: "Interfaces" });
    const modules = screen.getByRole("region", { name: "Modules" });
    await within(interfaces).findByText("Device port");
    const interfaceColumns = within(interfaces).getByRole("button", { name: "Columns" });
    const moduleColumns = within(modules).getByRole("button", { name: "Columns" });
    expect(interfaceColumns).toHaveAttribute("data-column-settings-table-id", "inventory.children.Devices.device.interfaces");
    expect(moduleColumns.getAttribute("data-column-settings-table-id")).not.toBe(interfaceColumns.getAttribute("data-column-settings-table-id"));
    fireEvent.click(interfaceColumns);
    expect(screen.getByRole("menuitemcheckbox", { name: /Name/ })).toBeDisabled();
    fireEvent.click(screen.getByRole("menuitemcheckbox", { name: "Description" }));
    fireEvent.keyDown(document, { key: "Escape" });
    await waitFor(() => expect(settings.write).toHaveBeenCalledWith("table.column_visibility", expect.objectContaining({
      "inventory.children.Devices.device.interfaces": { hidden: ["description"] },
      "inventory.Interfaces": { hidden: ["description"] },
    })));
    expect(within(interfaces).queryByText("Device port")).toBeNull();
    expect(within(modules).getByText("Installed module")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Next parent record" }));
    const next = screen.getByRole("region", { name: "Interfaces" });
    await within(next).findByText("Selected parent · second");
    expect(within(next).getByRole("button", { name: "Columns" })).toHaveAttribute("data-column-settings-table-id", "inventory.children.Devices.device.interfaces");
    expect(within(next).queryByRole("columnheader", { name: "Description" })).toBeNull();
    expect(within(next).getByRole("columnheader", { name: /Name/ })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "VM parent" }));
    const vm = await screen.findByRole("region", { name: /VM interfaces/i });
    expect(await within(vm).findByText("VM port")).toBeInTheDocument();
    const vmColumns = within(vm).getByRole("button", { name: "Columns" });
    expect(vmColumns.getAttribute("data-column-settings-table-id")).not.toBe("inventory.children.Devices.device.interfaces");
    expect(vmColumns.getAttribute("data-column-settings-table-id")).not.toBe(moduleColumns.getAttribute("data-column-settings-table-id"));
    expect(api.post).not.toHaveBeenCalled(); expect(api.put).not.toHaveBeenCalled(); expect(api.patch).not.toHaveBeenCalled(); expect(api.remove).not.toHaveBeenCalled();
  });

  it("refreshes child changes after real editor save and parent return within production 30-second freshness", async () => {
    let description = "Original child description";
    const read: (path: string, request?: Request) => unknown = api.get.getMockImplementation()!;
    api.get.mockImplementation((path: string, request?: Request) => {
      if (path === routes.interfaces) return Promise.resolve(page([{ id: "interface-1", name: "eth0", device_id: "device-1", description }]));
      if (path === "/api/v1/inventory/devices") return Promise.resolve(page([{ id: "device-1", name: "Router" }]));
      return read(path, request);
    });
    api.resource.mockImplementation(() => Promise.resolve({ id: "interface-1", name: "eth0", type: "interface", deviceId: "device-1", description }));
    api.update.mockImplementation((_entity: InventoryEntityKey, _id: string, changes: { description: string }) => {
      description = changes.description;
      return Promise.resolve({ id: "interface-1", name: "eth0", type: "interface", deviceId: "device-1", description });
    });
    mount(<Routes>
      <Route path="/inventory/resources/:entity/:id" element={<InventoryResourceDetailPage />} />
      <Route path="/inventory/resources/:entity/:id/edit" element={<EntityResourceEditorPage />} />
    </Routes>);
    await screen.findByText("Original child description");
    fireEvent.click(screen.getByRole("link", { name: "eth0" }));
    fireEvent.click(await screen.findByRole("link", { name: "Edit" }));
    fireEvent.change(await screen.findByRole("textbox", { name: "Description" }), { target: { value: "Saved child description" } });
    fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));
    await screen.findByRole("heading", { level: 1, name: "Router" });
    expect(await screen.findByText("Saved child description")).toBeInTheDocument();
    expect(api.get.mock.calls.filter(([path]) => path === routes.interfaces)).toHaveLength(2);
    expect(api.update).toHaveBeenCalledWith("Interfaces", "interface-1", expect.objectContaining({ description: "Saved child description" }));
    expect(screen.queryByText("Original child description")).toBeNull();
  });
});
