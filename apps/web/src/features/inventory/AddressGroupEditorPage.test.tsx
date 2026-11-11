import { fireEvent, screen, waitFor } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AllPermissions } from "@/test/permissions";
import AddressGroupEditorPage from "./AddressGroupEditorPage";

const MEMBER_LIMIT = 64;

const catalog = vi.hoisted(() => ({
  listApplicationRoles: vi.fn(), createApplicationRole: vi.fn(), updateApplicationRole: vi.fn(), deleteApplicationRole: vi.fn(),
  listApplications: vi.fn(), getApplication: vi.fn(), createApplication: vi.fn(), updateApplication: vi.fn(), deleteApplication: vi.fn(),
  listDataflowGroups: vi.fn(), getDataflowGroup: vi.fn(), createDataflowGroup: vi.fn(), updateDataflowGroup: vi.fn(), deleteDataflowGroup: vi.fn(),
  listAddressGroups: vi.fn(), getAddressGroup: vi.fn(), createAddressGroup: vi.fn(), updateAddressGroup: vi.fn(), deleteAddressGroup: vi.fn(),
  bulkDataflows: vi.fn(),
}));
const api = vi.hoisted(() => ({ listAllResources: vi.fn() }));
const flowsApi = vi.hoisted(() => ({ resolveDataflowEndpoint: vi.fn() }));
vi.mock("./dataflowCatalog-api", () => catalog);
vi.mock("./api", () => api);
vi.mock("./dataflows-api", () => flowsApi);

const office = {
  id: "ag-office", name: "Office", description: "HQ", comments: "", owner_id: null, tenant_id: null, tags: [],
  members: [
    { object_type: "dcim.device", object_id: "device-1", label: "Host", address_mode: "default", ipaddresses: [] },
    { object_type: "ipam.prefix", object_id: "prefix-1", label: "LAN", address_mode: "default", ipaddresses: [] },
  ],
};

function member(index: number) {
  return { object_type: "dcim.device", object_id: `device-${index}`, label: `Host ${index}`, address_mode: "default", ipaddresses: [] };
}

function mount(path: string) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  render(<QueryClientProvider client={client}><AllPermissions><MemoryRouter initialEntries={[path]}><Routes>
    <Route path="/inventory/dataflows/address-groups" element={<div>Address groups list</div>} />
    <Route path="/inventory/dataflows/address-groups/new" element={<AddressGroupEditorPage />} />
    <Route path="/inventory/dataflows/address-groups/:id/edit" element={<AddressGroupEditorPage />} />
  </Routes></MemoryRouter></AllPermissions></QueryClientProvider>);
}

beforeEach(() => {
  vi.clearAllMocks();
  catalog.getAddressGroup.mockResolvedValue(office);
  catalog.updateAddressGroup.mockResolvedValue(office);
  catalog.createAddressGroup.mockResolvedValue(office);
  catalog.listAddressGroups.mockResolvedValue([{ id: "ag-other", name: "Other", members: [] }]);
  api.listAllResources.mockResolvedValue([
    { id: "device-1", name: "Host" }, { id: "prefix-1", name: "LAN" },
    ...Array.from({ length: MEMBER_LIMIT }, (_, index) => ({ id: `device-${index}`, name: `Host ${index}` })),
  ]);
  flowsApi.resolveDataflowEndpoint.mockResolvedValue({
    endpoint: { object_type: "dcim.device", object_id: "device-1", address_mode: "default", ipaddresses: [] },
    members: [], addresses: [], diagnostics: [], ipaddresses: [], default_ipaddresses: [],
  });
});

describe("address group editor", () => {
  it("removes a member and keeps the remaining member order in the payload", async () => {
    mount("/inventory/dataflows/address-groups/ag-office/edit");
    expect(await screen.findByRole("heading", { level: 1, name: "Address group — Office" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Remove object: Object 1" }));
    fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));
    await waitFor(() => expect(catalog.updateAddressGroup).toHaveBeenCalledWith("ag-office", expect.objectContaining({
      members: [{ object_type: "ipam.prefix", object_id: "prefix-1", address_mode: "default", ipaddresses: [] }],
    })));
    expect(await screen.findByText("Address groups list")).toBeInTheDocument();
  });

  it("adds a member to the end of the list", async () => {
    mount("/inventory/dataflows/address-groups/ag-office/edit");
    await screen.findByRole("heading", { level: 1, name: "Address group — Office" });
    expect(screen.getByText("2 / 64")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Add object" }));
    expect(screen.getByText("3 / 64")).toBeInTheDocument();
    expect(screen.getByRole("group", { name: "Object 3" })).toBeInTheDocument();
  });

  it("stops adding at the 64 member limit", async () => {
    catalog.getAddressGroup.mockResolvedValue({ ...office, members: Array.from({ length: MEMBER_LIMIT }, (_, index) => member(index)) });
    mount("/inventory/dataflows/address-groups/ag-office/edit");
    await screen.findByRole("heading", { level: 1, name: "Address group — Office" });
    expect(await screen.findByText("64 / 64")).toBeInTheDocument();
    const add = screen.getByRole("button", { name: "Add object" });
    expect(add).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Remove object: Object 64" }));
    await waitFor(() => expect(screen.getByText("63 / 64")).toBeInTheDocument());
    expect(screen.getByRole("button", { name: "Add object" })).toBeEnabled();
  });

  it("omits address groups from member type choices", async () => {
    mount("/inventory/dataflows/address-groups/new");
    const type = await screen.findByRole("combobox", { name: "Object 1 type" });
    fireEvent.click(type);
    const labels = (await screen.findAllByRole("option")).map(option => option.textContent);
    expect(labels).not.toContain("Address group");
    expect(labels).toContain("Device");
    expect(labels).toContain("IP range");
  });

  it("rejects a nested address group member before saving", async () => {
    catalog.getAddressGroup.mockResolvedValue({
      ...office,
      members: [{ object_type: "inventory.addressgroup", object_id: "ag-other", label: "Other", address_mode: "default", ipaddresses: [] }],
    });
    mount("/inventory/dataflows/address-groups/ag-office/edit");
    await screen.findByRole("heading", { level: 1, name: "Address group — Office" });
    fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("An address group cannot contain another address group.");
    expect(catalog.updateAddressGroup).not.toHaveBeenCalled();
  });

  it("blocks saving an incomplete member on create", () => {
    mount("/inventory/dataflows/address-groups/new");
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    expect(screen.getAllByRole("alert").map(node => node.textContent).join(" ")).toContain("Select both a source and a destination.");
    expect(catalog.createAddressGroup).not.toHaveBeenCalled();
  });
});
