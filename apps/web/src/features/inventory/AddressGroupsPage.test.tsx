import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes, useLocation, useNavigate } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AllPermissions } from "@/test/permissions";
import { PermissionsContext } from "@/shared/auth/permissionsContext";
import { ApiError } from "@/shared/api/client";
import AddressGroupsPage from "./AddressGroupsPage";

const catalog = vi.hoisted(() => ({
  listApplicationRoles: vi.fn(), createApplicationRole: vi.fn(), updateApplicationRole: vi.fn(), deleteApplicationRole: vi.fn(),
  listApplications: vi.fn(), getApplication: vi.fn(), createApplication: vi.fn(), updateApplication: vi.fn(), deleteApplication: vi.fn(),
  listDataflowGroups: vi.fn(), getDataflowGroup: vi.fn(), createDataflowGroup: vi.fn(), updateDataflowGroup: vi.fn(), deleteDataflowGroup: vi.fn(),
  listAddressGroups: vi.fn(), getAddressGroup: vi.fn(), createAddressGroup: vi.fn(), updateAddressGroup: vi.fn(), deleteAddressGroup: vi.fn(),
  bulkDataflows: vi.fn(),
}));
const flowsApi = vi.hoisted(() => ({ resolveDataflowEndpoint: vi.fn() }));
vi.mock("./dataflowCatalog-api", () => catalog);
vi.mock("./dataflows-api", () => flowsApi);

const office = {
  id: "ag-office", name: "Office", description: "HQ ranges", comments: "Managed by the network team", owner_id: null, tenant_id: null, tags: [],
  members: [
    { object_type: "dcim.device", object_id: "device-1", label: "Host", address_mode: "default", ipaddresses: [] },
    { object_type: "ipam.prefix", object_id: "prefix-1", label: "LAN", address_mode: "default", ipaddresses: [] },
  ],
};
const branch = { ...office, id: "ag-branch", name: "Branch", description: "", members: [office.members[0]] };

function Probe() {
  const location = useLocation();
  const navigate = useNavigate();
  return <>
    <span data-testid="url">{location.search}</span>
    <button type="button" data-testid="browser-back" onClick={() => void navigate(-1)}>History back</button>
  </>;
}

function mount(path = "/inventory/dataflows/address-groups", readOnly = false) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const content = <MemoryRouter initialEntries={[path]}><Routes>
    <Route path="/inventory/dataflows/address-groups" element={<AddressGroupsPage />} />
  </Routes><Probe /></MemoryRouter>;
  render(<QueryClientProvider client={client}>
    {readOnly
      ? <PermissionsContext.Provider value={{ roles: [], isAdmin: false, can: action => action === "READ", canAny: () => false, hasRole: () => false }}>{content}</PermissionsContext.Provider>
      : <AllPermissions>{content}</AllPermissions>}
  </QueryClientProvider>);
}

async function openDetail(name: string) {
  const opener = await screen.findByRole("button", { name: `Inspect ${name}` });
  fireEvent.click(opener);
  const dialog = await screen.findByRole("dialog", { name });
  return { opener, dialog };
}

beforeEach(() => {
  vi.clearAllMocks();
  catalog.listAddressGroups.mockResolvedValue([branch, office]);
  catalog.deleteAddressGroup.mockResolvedValue(undefined);
  flowsApi.resolveDataflowEndpoint.mockResolvedValue({
    endpoint: { object_type: "dcim.device", object_id: "device-1", address_mode: "default", ipaddresses: [], label: "Web host" },
    addresses: [{ cidr: "10.0.0.10/32", vrf_id: null }],
    default_ipaddresses: ["ip-1"],
    diagnostics: [],
    ipaddresses: [],
    members: [{ host: { object_type: "dcim.device", object_id: "device-1", label: "Web host" }, addresses: [{ cidr: "10.0.0.10/32", vrf_id: null }], diagnostics: [] }],
  });
});

describe("address groups catalog", () => {
  it("lists groups with their member counts and the active catalog tab", async () => {
    mount();
    await screen.findByRole("button", { name: "Inspect Office" });
    expect(screen.getAllByRole("button", { name: /^Inspect / }).map(node => node.textContent)).toEqual(["Branch", "Office"]);
    const officeRow = screen.getByRole("button", { name: "Inspect Office" }).closest("tr") as HTMLElement;
    const branchRow = screen.getByRole("button", { name: "Inspect Branch" }).closest("tr") as HTMLElement;
    expect(within(officeRow).getByText("2")).toBeInTheDocument();
    expect(within(branchRow).getByText("1")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Address groups" })).toHaveAttribute("aria-current", "page");
  });

  it("omits empty metadata columns", async () => {
    mount();
    await screen.findByRole("button", { name: "Inspect Office" });
    expect(screen.getAllByRole("columnheader").map(header => header.textContent)).not.toContain("Tags");
  });

  it("shows metadata columns when a visible row carries them", async () => {
    catalog.listAddressGroups.mockResolvedValue([{ ...office, owner_id: "owner-1", tenant_id: "tenant-1", tags: ["tag-1"] }]);
    mount();
    await screen.findByRole("button", { name: "Inspect Office" });
    expect(screen.getAllByRole("columnheader").map(header => header.textContent)).toEqual(expect.arrayContaining(["Owner", "Tenant", "Tags"]));
  });

  it("keeps search in the URL", async () => {
    mount();
    await screen.findByRole("button", { name: "Inspect Office" });
    fireEvent.change(screen.getByPlaceholderText("Search"), { target: { value: "office" } });
    await waitFor(() => expect(screen.getByTestId("url")).toHaveTextContent("?q=office"));
    expect(screen.queryByRole("button", { name: "Inspect Branch" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Inspect Office" })).toBeInTheDocument();
  });

  it("keeps the read action for a long name", async () => {
    const longName = "Branch office ranges for the northern region including every temporary construction site";
    catalog.listAddressGroups.mockResolvedValue([{ ...office, name: longName }]);
    mount();
    const opener = await screen.findByRole("button", { name: `Inspect ${longName}` });
    expect(opener).toBeVisible();
    expect(screen.getByRole("button", { name: `Actions: ${longName}` })).toBeInTheDocument();
  });

  it("lets a read-only user inspect metadata and resolver members without write controls", async () => {
    mount("/inventory/dataflows/address-groups", true);
    const { dialog } = await openDetail("Office");
    expect(within(dialog).getByText("HQ ranges")).toBeInTheDocument();
    expect(within(dialog).getByText("Managed by the network team")).toBeInTheDocument();
    expect(within(dialog).getByText("Objects in this group: 2")).toBeInTheDocument();
    expect(within(dialog).getByRole("heading", { name: "Object 1" })).toBeInTheDocument();
    expect(within(dialog).getByRole("heading", { name: "Object 2" })).toBeInTheDocument();
    expect((await within(dialog).findAllByText(/10\.0\.0\.10\/32/)).length).toBeGreaterThan(0);
    expect(within(dialog).queryByRole("link", { name: "Edit" })).not.toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: "Delete" })).not.toBeInTheDocument();
  });

  it("links directly to the flows that use this address group as an endpoint", async () => {
    mount();
    const { dialog } = await openDetail("Office");
    expect(within(dialog).getByRole("link", { name: "Flows" }))
      .toHaveAttribute("href", "/inventory/dataflows?endpointType=inventory.addressgroup&endpointId=ag-office");
  });

  it("closes with browser back and restores name focus while keeping the search", async () => {
    mount("/inventory/dataflows/address-groups?q=off");
    const { opener } = await openDetail("Office");
    expect(screen.getByTestId("url")).toHaveTextContent("selected=ag-office");
    fireEvent.click(screen.getByTestId("browser-back"));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Office" })).not.toBeInTheDocument());
    await waitFor(() => expect(opener).toHaveFocus());
    expect(screen.getByPlaceholderText("Search")).toHaveValue("off");
    expect(screen.getByTestId("url")).toHaveTextContent("q=off");
    expect(screen.getByTestId("url")).not.toHaveTextContent("selected");
  });

  it("keeps search and the open detail when a referenced delete is blocked", async () => {
    catalog.deleteAddressGroup.mockRejectedValue(new ApiError({ status: 409, message: "selectors", reasonCode: "DELETE_BLOCKED" }));
    mount("/inventory/dataflows/address-groups?q=off");
    const { dialog } = await openDetail("Office");
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    const confirm = await within(dialog).findByRole("group", { name: "Delete" });
    fireEvent.click(within(confirm).getByRole("button", { name: "Delete" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Delete is blocked by related records");
    expect(catalog.deleteAddressGroup).toHaveBeenCalledWith("ag-office", expect.anything());
    expect(screen.getByRole("dialog", { name: "Office" })).toBeInTheDocument();
    expect(screen.getByPlaceholderText("Search")).toHaveValue("off");
    expect(screen.getByTestId("url")).toHaveTextContent("q=off");
    expect(screen.getByTestId("url")).toHaveTextContent("selected=ag-office");
  });

  it("reports a referenced address group delete as blocked from the row actions", async () => {
    catalog.deleteAddressGroup.mockRejectedValue(new ApiError({ status: 409, message: "selectors", reasonCode: "DELETE_BLOCKED" }));
    mount();
    await screen.findByRole("button", { name: "Inspect Office" });
    fireEvent.keyDown(screen.getByRole("button", { name: "Actions: Office" }), { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitem", { name: "Delete" }));
    const dialog = await screen.findByRole("alertdialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Delete is blocked by related records");
    expect(catalog.deleteAddressGroup).toHaveBeenCalledWith("ag-office", expect.anything());
  });
});
