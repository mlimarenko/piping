import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes, useLocation, useNavigate } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { changeAppLanguage } from "@/shared/i18n";
import { AllPermissions } from "@/test/permissions";
import { PermissionsContext } from "@/shared/auth/permissionsContext";
import DataflowsPage from "./DataflowsPage";
import DataflowEditorPage from "./DataflowEditorPage";
import en from "@/shared/i18n/locales/en";
import { parseDataflowPorts } from "./dataflowPorts";
import type { Dataflow, DataflowEndpointResolution } from "./dataflows-api";

const api = vi.hoisted(() => ({ listDataflows: vi.fn(), getDataflow: vi.fn(), createDataflow: vi.fn(), updateDataflow: vi.fn(), deleteDataflow: vi.fn(), getDataflowPolicy: vi.fn(), listAllResources: vi.fn(), resolveDataflowEndpoint: vi.fn() }));
vi.mock("./dataflows-api", () => api);
vi.mock("./api", () => ({ listAllResources: api.listAllResources }));

const flow: Dataflow = {
  id: "flow-1", name: "Web to database", description: "Application traffic", status: "active", transport: "tcp", destination_ports: [443],
  source: { object_type: "dcim.device", object_id: "host-1", label: "Web host", address_mode: "default", ipaddresses: [] },
  destination: { object_type: "dcim.device", object_id: "host-2", label: "Database host", address_mode: "default", ipaddresses: [] },
  allow_cross_vrf: false,
};
const host = { object_type: "dcim.device", object_id: "host-1", label: "Web host" };
const policy = {
  host, apply_ready: true, diagnostics: [],
  rules: [{ direction: "egress", family: "ipv4", transport: "tcp", source: "192.0.2.1/32", destination: "192.0.2.2/32", destination_ports: [443], vrf_id: null, dataflow_ids: ["flow-1"] }],
};

function NavigationProbe() {
  const location = useLocation();
  const navigate = useNavigate();
  return <><span data-testid="current-url">{location.pathname}{location.search}</span><button onClick={() => navigate(-1)}>Browser back</button></>;
}

function mount(path = "/inventory/dataflows", readOnly = false) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const content = <MemoryRouter initialEntries={[path]}><Routes>
    <Route path="/inventory/dataflows" element={<DataflowsPage />} />
    <Route path="/inventory/dataflows/new" element={<DataflowEditorPage />} />
    <Route path="/inventory/dataflows/:id/edit" element={<DataflowEditorPage />} />
  </Routes><NavigationProbe /></MemoryRouter>;
  const view = render(<QueryClientProvider client={client}>{readOnly ? <PermissionsContext.Provider value={{ roles: [], isAdmin: false, can: action => action === "READ", canAny: () => false, hasRole: () => false }}>{content}</PermissionsContext.Provider> : <AllPermissions>{content}</AllPermissions>}</QueryClientProvider>);
  return { ...view, client };
}

async function choose(label: string, name: string) {
  const picker = await screen.findByRole("combobox", { name: label  });
  await waitFor(() => expect(picker).toBeEnabled());
  fireEvent.click(picker);
  fireEvent.click(await screen.findByRole("option", { name: new RegExp(name) }));
}

async function select(label: string, option: string) {
  fireEvent.keyDown(screen.getByRole("combobox", { name: label }), { key: "ArrowDown" });
  fireEvent.click(await screen.findByRole("option", { name: option }));
}

async function openPolicy() {
  fireEvent.click(screen.getByRole("button", { name: "Export JSON" }));
  await screen.findByRole("dialog", { name: "Host firewall policy" });
}

async function flowAction(name: string) {
  fireEvent.keyDown(screen.getByRole("button", { name: "Actions for Web to database" }), { key: "Enter" });
  fireEvent.click(await screen.findByRole("menuitem", { name }));
}

beforeEach(() => {
  vi.clearAllMocks();
  api.listDataflows.mockResolvedValue([flow]);
  api.getDataflow.mockResolvedValue(flow);
  api.createDataflow.mockResolvedValue(flow);
  api.updateDataflow.mockResolvedValue(flow);
  api.deleteDataflow.mockResolvedValue(undefined);
  api.listAllResources.mockResolvedValue([{ id: "host-1", name: "Web host", type: "device" }, { id: "host-2", name: "Database host", type: "device" }]);
  api.getDataflowPolicy.mockResolvedValue(policy);
  api.resolveDataflowEndpoint.mockResolvedValue({ endpoint: flow.source, members: [], addresses: [{ cidr: "192.0.2.1/32", vrf_id: null }], diagnostics: [], ipaddresses: [{ id: "ip-1", address: "192.0.2.1/32", vrf_id: null }, { id: "ip-2", address: "2001:db8::1/128", vrf_id: null }], default_ipaddresses: ["ip-1"] } satisfies DataflowEndpointResolution);
});

describe("dataflow form", () => {
  it.each(["empty", "another-row", "error", "loading"])("restores search when a query removes the dataflow opener (%s)", async state => {
    const { client } = mount();
    const opener = await screen.findByRole("button", { name: "Inspect Web to database" });
    fireEvent.click(opener);
    await screen.findByRole("dialog", { name: flow.name });
    if (state === "error") api.listDataflows.mockRejectedValue(new Error("query failed"));
    else if (state === "loading") api.listDataflows.mockReturnValue(new Promise(() => {}));
    else api.listDataflows.mockResolvedValue(state === "empty" ? [] : [{ ...flow, id: "flow-2", name: "Other flow" }]);
    await act(async () => {
      if (state === "loading") void client.resetQueries({ queryKey: ["inventory", "dataflows", "list"] });
      else await client.refetchQueries({ queryKey: ["inventory", "dataflows", "list"] });
    });
    await waitFor(() => expect(opener).not.toBeInTheDocument());
    const dialog = screen.queryByRole("dialog", { name: flow.name });
    if (dialog) fireEvent.keyDown(dialog, { key: "Escape" });
    await waitFor(() => expect(screen.getByPlaceholderText("Search names or endpoints…")).toHaveFocus());
    client.clear();
  });
  it("groups endpoints, traffic and metadata while keeping inventory previews optional", async () => {
    mount("/inventory/dataflows/flow-1/edit");
    await screen.findByRole("heading", { level: 1, name: "Edit dataflow — Web to database" });
    for (const name of ["Endpoints", "Traffic", "Details"]) {
      expect(screen.getByRole("heading", { level: 2, name })).toBeInTheDocument();
    }
    const source = screen.getByRole("group", { name: "Source" });
    const preview = await within(source).findByText("No known host members for this endpoint.");
    expect(preview).not.toBeVisible();
    fireEvent.click(within(source).getByText("Inventory membership preview"));
    expect(preview).toBeVisible();
    fireEvent.click(within(source).getByText("Inventory membership preview"));
    expect(preview).not.toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));
    await waitFor(() => expect(api.updateDataflow).toHaveBeenCalledWith("flow-1", expect.objectContaining({
      source: { object_type: "dcim.device", object_id: "host-1", address_mode: "default", ipaddresses: [] },
    })));
  });

  it("focuses the first incomplete endpoint and does not submit an invalid form", () => {
    mount("/inventory/dataflows/new");
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    expect(screen.getByRole("combobox", { name: "Source type" })).toHaveFocus();
    expect(screen.getByRole("alert")).toHaveTextContent("Select both a source and a destination.");
    expect(api.createDataflow).not.toHaveBeenCalled();
  });

  it("reveals and focuses invalid advanced description rather than hiding its validation", async () => {
    mount("/inventory/dataflows/flow-1/edit");
    await screen.findByRole("textbox", { name: "Name (optional)" });
    fireEvent.click(screen.getByText("Advanced", { selector: "summary" }));
    const description = screen.getByRole("textbox", { name: "Description" });
    fireEvent.change(description, { target: { value: "x".repeat(4001) } });
    fireEvent.click(screen.getByText("Advanced", { selector: "summary" }));
    expect(description).not.toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));
    expect(description).toBeVisible();
    expect(description).toHaveFocus();
    expect(api.updateDataflow).not.toHaveBeenCalled();
  });

  it.each(["All Piping hosts", "Internet / any IP", "IP address", "Prefix"])("keeps %s on its fixed address mode", async type => {
    mount("/inventory/dataflows/new");
    await select("Source type", type);
    expect(screen.queryByRole("combobox", { name: "Source addresses" })).not.toBeInTheDocument();
  });
  it("guards navigation after changing an inventory group and preserves it when continuing", async () => {
    mount("/inventory/dataflows/new");
    await select("Source type", "Region");
    fireEvent.click(screen.getByRole("button", { name: "Back to dataflows" }));
    expect(await screen.findByRole("alertdialog", { name: "Leave without saving?" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Continue editing" }));
    expect(screen.getByRole("combobox", { name: "Source type" })).toHaveTextContent("Region");
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    fireEvent.click(await screen.findByRole("button", { name: "Discard" }));
    expect(await screen.findByRole("heading", { name: "Dataflows" })).toBeInTheDocument();
    expect(api.createDataflow).not.toHaveBeenCalled();
  });
  it("previews a group through the resolver and sends only its selector without inferring traffic", async () => {
    mount("/inventory/dataflows/new");
    await select("Source type", "Region");
    await choose("Source", "Web host");
    await select("Destination type", "Tenant");
    await choose("Destination", "Database host");
    await waitFor(() => expect(api.resolveDataflowEndpoint).toHaveBeenCalledWith({ object_type: "dcim.region", object_id: "host-1", address_mode: "default", ipaddresses: [] }));
    expect(api.listAllResources).toHaveBeenCalledWith("Regions");
    expect(api.listAllResources).toHaveBeenCalledWith("Tenants");
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(api.createDataflow).toHaveBeenCalledWith(expect.objectContaining({ transport: "any", destination_ports: null,
      source: { object_type: "dcim.region", object_id: "host-1", address_mode: "default", ipaddresses: [] }, destination: { object_type: "tenancy.tenant", object_id: "host-2", address_mode: "default", ipaddresses: [] } })));
  });

  it("restricts host policy types to Device and VM", async () => {
    mount();
    await openPolicy();
    fireEvent.keyDown(screen.getByRole("combobox", { name: "Host type" }), { key: "ArrowDown" });
    expect(await screen.findByRole("option", { name: "Device" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "Virtual machine" })).toBeInTheDocument();
    expect(screen.getAllByRole("option")).toHaveLength(2);
    expect(api.resolveDataflowEndpoint).not.toHaveBeenCalled();
  });

  it("rejects more than 32 explicit IP selections before save", async () => {
    api.getDataflow.mockResolvedValue({ ...flow, source: { ...flow.source, address_mode: "selected", ipaddresses: Array.from({ length: 33 }, (_, i) => `ip-${i}`) } });
    mount("/inventory/dataflows/flow-1/edit");
    fireEvent.click(await screen.findByRole("button", { name: "Save Changes" }));
    expect(screen.getByText("Choose no more than 32 IP addresses for one endpoint.")).toBeInTheDocument();
    expect(api.updateDataflow).not.toHaveBeenCalled();
  });
  it.each([
    ["Device", en.inventory.resources.hints.entities.Devices],
    ["Virtual machine", en.inventory.resources.hints.entities["Virtual Machines"]],
    ["Service", en.inventory.resources.hints.entities.Services],
    ["IP address", en.inventory.resources.hints.entities["IP Addresses"]],
    ["Prefix", en.inventory.resources.hints.entities.Prefixes],
    ["Region", en.inventory.resources.hints.entities.Regions],
    ["Site", en.inventory.resources.hints.entities.Sites],
    ["Site group", en.inventory.resources.hints.entities["Site Groups"]],
    ["Device role", en.inventory.resources.hints.entities["Device Roles"]],
    ["Tenant", en.inventory.resources.hints.entities.Tenants],
    ["Tenant group", en.inventory.resources.hints.entities["Tenant Groups"]],
    ["Platform", en.inventory.resources.hints.entities.Platforms],
    ["Manufacturer", en.inventory.resources.hints.entities.Manufacturers],
    ["Device type", en.inventory.resources.hints.entities["Device Types"]],
    ["Rack", en.inventory.resources.hints.entities.Racks],
    ["Location", en.inventory.resources.hints.entities.Locations],
    ["Cluster", en.inventory.resources.hints.entities.Clusters],
    ["Cluster group", en.inventory.resources.hints.entities["Cluster Groups"]],
    ["Cluster type", en.inventory.resources.hints.entities["Cluster Types"]],
    ["All Piping hosts", en.dataflows.allHint],
    ["Internet / any IP", en.dataflows.internetHint],
  ])("explains the selected %s endpoint and describes its type selector", async (type, explanation) => {
    mount("/inventory/dataflows/new");
    if (type !== "Device") await select("Source type", type);
    const source = screen.getByRole("group", { name: "Source" });
    const description = within(source).getByText(explanation);
    expect(within(source).getByRole("combobox", { name: "Source type" })).toHaveAttribute("aria-describedby", description.id);
  });

  it("uses canonical address options/defaults and deduplicates identical endpoints", async () => {
    mount("/inventory/dataflows/new");
    await choose("Source", "Web host");
    await choose("Destination", "Web host");
    expect(await screen.findAllByText("Default addresses: 192.0.2.1/32")).toHaveLength(2);
    expect(api.resolveDataflowEndpoint).toHaveBeenCalledOnce();
    expect(api.resolveDataflowEndpoint).toHaveBeenCalledWith({ object_type: "dcim.device", object_id: "host-1", address_mode: "default", ipaddresses: [] });
    expect(api.listAllResources).toHaveBeenCalledWith("Devices");
    expect(api.listAllResources).not.toHaveBeenCalledWith("Interfaces");
    expect(api.listAllResources).not.toHaveBeenCalledWith("IP Addresses");
  });
  it("creates with only chosen source/destination and visible defaults", async () => {
    mount("/inventory/dataflows/new");
    await choose("Source", "Web host");
    await choose("Destination", "Database host");
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(api.createDataflow).toHaveBeenCalledWith(expect.objectContaining({ name: undefined, status: "draft", transport: "any", destination_ports: null, allow_cross_vrf: false })));
  });

  it.each([["Internet / any IP", "internet"], ["All Piping hosts", "all"]])("creates %s to host without builtin UUID or address picker", async (label, type) => {
    mount("/inventory/dataflows/new");
    await select("Source type", label);
    expect(screen.queryByRole("combobox", { name: "Source" })).not.toBeInTheDocument();
    expect(screen.queryByRole("combobox", { name: "Source addresses" })).not.toBeInTheDocument();
    await choose("Destination", "Database host");
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(api.createDataflow).toHaveBeenCalledWith(expect.objectContaining({ transport: "any", destination_ports: null, source: { object_type: type, object_id: null, address_mode: "default", ipaddresses: [] } })));
  });

  it("selects several IPs and resets selections on address mode changes", async () => {
    mount("/inventory/dataflows/new");
    await choose("Source", "Web host");
    await choose("Destination", "Database host");
    await select("Source addresses", "Selected addresses");
    const picker = screen.getByRole("combobox", { name: "Source selected IP addresses" });
    await waitFor(() => expect(picker).toBeEnabled());
    fireEvent.click(picker);
    fireEvent.click(await screen.findByRole("option", { name: "192.0.2.1/32" }));
    fireEvent.click(screen.getByRole("option", { name: "2001:db8::1/128" }));
    fireEvent.keyDown(window, { key: "Escape" });
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(api.createDataflow).toHaveBeenCalledWith(expect.objectContaining({ source: { object_type: "dcim.device", object_id: "host-1", address_mode: "selected", ipaddresses: ["ip-1", "ip-2"] } })));
  });

  it("switches selected addresses and ports back to defaults without retaining stale IDs or ports", async () => {
    api.getDataflow.mockResolvedValue({ ...flow, source: { ...flow.source, address_mode: "selected", ipaddresses: ["ip-1"] } });
    mount("/inventory/dataflows/flow-1/edit");
    await screen.findByRole("textbox", { name: "Name (optional)" });
    await select("Source addresses", "All object addresses");
    await select("Destination ports", "Any ports");
    fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));
    await waitFor(() => expect(api.updateDataflow).toHaveBeenCalledWith("flow-1", expect.objectContaining({ destination_ports: null, source: { object_type: "dcim.device", object_id: "host-1", address_mode: "all", ipaddresses: [] } })));
  });

  it("switches a selected endpoint to Internet and clears its object identity and selected addresses", async () => {
    api.getDataflow.mockResolvedValue({ ...flow, source: { ...flow.source, address_mode: "selected", ipaddresses: ["ip-1"] } });
    mount("/inventory/dataflows/flow-1/edit");
    await screen.findByRole("textbox", { name: "Name (optional)" });
    await select("Source type", "Internet / any IP");
    fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));
    await waitFor(() => expect(api.updateDataflow).toHaveBeenCalledWith("flow-1", expect.objectContaining({ source: { object_type: "internet", object_id: null, address_mode: "default", ipaddresses: [] } })));
  });

  it("requires chosen addresses only in selected mode and clears that constraint when returning to default", async () => {
    mount("/inventory/dataflows/new");
    await choose("Source", "Web host");
    await choose("Destination", "Database host");
    await select("Source addresses", "Selected addresses");
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    expect(screen.getAllByText("Choose at least one IP address or use default/all addresses.").length).toBeGreaterThan(0);
    expect(api.createDataflow).not.toHaveBeenCalled();
    await select("Source addresses", "Default addresses");
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(api.createDataflow).toHaveBeenCalledWith(expect.objectContaining({ source: { object_type: "dcim.device", object_id: "host-1", address_mode: "default", ipaddresses: [] } })));
  });
  it("renders Russian labels and explicit validation errors", async () => {
    await changeAppLanguage("ru");
    const view = mount("/inventory/dataflows/new");
    try {
      expect(screen.getByRole("heading", { name: "Новый поток" })).toBeInTheDocument();
      fireEvent.click(screen.getByRole("button", { name: "Создать" }));
      expect(screen.getByText("Выберите источник и назначение.")).toBeInTheDocument();
      expect(screen.getByRole("combobox", { name: "Порты назначения" })).toHaveTextContent("Любые порты");
    } finally {
      view.unmount();
      await changeAppLanguage("en");
    }
  });
  it("submits explicit sorted distinct ports and defaults to draft", async () => {
    mount("/inventory/dataflows/new");
    fireEvent.change(screen.getByRole("textbox", { name: "Name (optional)" }), { target: { value: " HTTPS " } });
    await select("Destination ports", "Selected ports");
    fireEvent.change(screen.getByRole("textbox", { name: "Selected ports" }), { target: { value: "8443, 443,443" } });
    await choose("Source", "Web host");
    await choose("Destination", "Database host");
    fireEvent.click(screen.getByRole("button", { name: "Create"  }));
    await waitFor(() => expect(api.createDataflow).toHaveBeenCalledWith(expect.objectContaining({ name: "HTTPS", status: "draft", transport: "any", destination_ports: [443, 8443], source: { object_type: "dcim.device", object_id: "host-1", address_mode: "default", ipaddresses: [] }, destination: { object_type: "dcim.device", object_id: "host-2", address_mode: "default", ipaddresses: [] } })));
    expect(await screen.findByRole("heading", { name: "Dataflows" })).toBeInTheDocument();
  });

  it.each(["", "0", "65536", "1.5", "80,", "80-90", "any"])("rejects invalid selected ports %j without sending", async ports => {
    mount("/inventory/dataflows/new");
    await select("Destination ports", "Selected ports");
    fireEvent.change(screen.getByRole("textbox", { name: "Selected ports" }), { target: { value: ports } });
    fireEvent.click(screen.getByRole("button", { name: "Create"  }));
    expect(screen.getByText(/Enter 1–256 distinct integer ports/)).toBeInTheDocument();
    expect(screen.getByText("Select both a source and a destination.")).toBeInTheDocument();
    expect(api.createDataflow).not.toHaveBeenCalled();
  });

  it("loads and edits without sending response labels", async () => {
    mount("/inventory/dataflows/flow-1/edit");
    fireEvent.change(await screen.findByRole("textbox", { name: "Name (optional)" }), { target: { value: "Updated" } });
    fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));
    await waitFor(() => expect(api.updateDataflow).toHaveBeenCalledWith("flow-1", expect.objectContaining({ name: "Updated", source: { object_type: "dcim.device", object_id: "host-1", address_mode: "default", ipaddresses: [] }, destination_ports: [443] })));
  });

  it("shows safe error copy and keeps failed draft", async () => {
    api.updateDataflow.mockRejectedValue(new Error("SQL private internal detail"));
    mount("/inventory/dataflows/flow-1/edit");
    fireEvent.click(await screen.findByRole("button", { name: "Save Changes" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not save the dataflow");
    expect(screen.queryByText(/SQL private/)).not.toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Name (optional)" })).toHaveValue(flow.name);
  });

  it("preserves update permission gates on direct navigation", async () => {
    mount("/inventory/dataflows/flow-1/edit", true);
    expect(await screen.findByRole("textbox", { name: "Name (optional)" })).toBeDisabled();
    expect(screen.queryByRole("button", { name: "Save Changes" })).not.toBeInTheDocument();
  });

  it("bounds distinct ports and accepts both endpoints of port domain", () => {
    expect(parseDataflowPorts("65535,1,1")).toEqual([1, 65535]);
    expect(parseDataflowPorts(Array.from({ length: 257 }, (_, i) => String(i + 1)).join(","))).toBeNull();
  });

  it("uses named service endpoints without inferring listener ports", async () => {
    api.listAllResources.mockImplementation((entity: string) => Promise.resolve(entity === "Services" ? [{ id: "service-1", name: "DNS listener", type: "service", ports: "53", protocol: "udp", serviceIpAddressIds: [] }] : [{ id: "host-1", name: "Web host", type: "device" }]));
    mount("/inventory/dataflows/new");
    fireEvent.change(screen.getByRole("textbox", { name: "Name (optional)" }), { target: { value: "Explicit service intent" } });
    await choose("Source", "Web host");
    fireEvent.keyDown(screen.getByRole("combobox", { name: "Destination type" }), { key: "ArrowDown" });
    fireEvent.click(await screen.findByRole("option", { name: "Service" }));
    await choose("Destination", "DNS listener");
    expect(screen.getByRole("combobox", { name: "Destination ports" })).toHaveTextContent("Any ports");
    await select("Destination ports", "Selected ports");
    fireEvent.change(screen.getByRole("textbox", { name: "Selected ports" }), { target: { value: "8443" } });
    fireEvent.keyDown(screen.getByRole("combobox", { name: "Transport" }), { key: "ArrowDown" });
    fireEvent.click(await screen.findByRole("option", { name: "SCTP" }));
    fireEvent.keyDown(screen.getByRole("combobox", { name: "Status" }), { key: "ArrowDown" });
    fireEvent.click(await screen.findByRole("option", { name: "Active" }));
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(api.createDataflow).toHaveBeenCalledWith(expect.objectContaining({ status: "active", transport: "sctp", destination_ports: [8443], destination: { object_type: "ipam.service", object_id: "service-1", address_mode: "default", ipaddresses: [] } })));
  });
});

describe("dataflow workspace", () => {
  it("shows disable failures inside the selected flow dialog", async () => {
    api.updateDataflow.mockRejectedValue(new Error("PRIVATE API DETAIL"));
    mount("/inventory/dataflows?view=topology&flow=flow-1");
    const dialog = await screen.findByRole("dialog", { name: flow.name });
    await flowAction("Disable");
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Could not change this dataflow.");
    expect(screen.queryByText("PRIVATE API DETAIL")).not.toBeInTheDocument();
  });
  it("switches views with keyboard tabs, inspects graph connections and restores URL selection on browser back", async () => {
    mount();
    await screen.findByRole("button", { name: "Inspect Web to database" });
    const topology = screen.getByRole("tab", { name: "Topology" });
    fireEvent.mouseDown(topology, { button: 0, ctrlKey: false });
    expect(await screen.findByRole("region", { name: "Scrollable connection topology" })).toBeInTheDocument();
    expect(screen.getByTestId("current-url")).toHaveTextContent("view=topology");
    expect(api.resolveDataflowEndpoint).not.toHaveBeenCalled();
    const node = screen.getByRole("button", { name: "Highlight connections for Web host" });
    node.focus();
    expect(node).toHaveFocus();
    fireEvent.click(node);
    expect(node).toHaveAttribute("aria-pressed", "true");
    fireEvent.click(screen.getByRole("button", { name: "Inspect Web to database" }));
    expect(await screen.findByRole("dialog", { name: flow.name })).toBeInTheDocument();
    expect(screen.getByTestId("current-url")).toHaveTextContent("flow=flow-1");
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: flow.name })).not.toBeInTheDocument());
    expect(screen.getByRole("button", { name: "Inspect Web to database" })).toHaveFocus();
    fireEvent.click(screen.getByRole("button", { name: "Browser back" }));
    expect(await screen.findByRole("dialog", { name: flow.name })).toBeInTheDocument();
  });

  it("exposes the list and topology views as real tabpanels switched by keyboard and by pointer", async () => {
    mount();
    await screen.findByRole("button", { name: "Inspect Web to database" });
    expect(screen.getByRole("tablist", { name: "Connection view" })).toBeInTheDocument();
    expect(screen.getByRole("tabpanel", { name: "List" })).toBeInTheDocument();
    fireEvent.keyDown(screen.getByRole("tab", { name: "Topology" }), { key: "Enter" });
    expect(screen.getByRole("tab", { name: "Topology" })).toHaveAttribute("aria-selected", "true");
    expect(await screen.findByRole("tabpanel", { name: "Topology" })).toBeInTheDocument();
    expect(await screen.findByRole("region", { name: "Scrollable connection topology" })).toBeInTheDocument();
    expect(screen.getByTestId("current-url")).toHaveTextContent("view=topology");
    fireEvent.mouseDown(screen.getByRole("tab", { name: "List" }), { button: 0, ctrlKey: false });
    expect(await screen.findByRole("tabpanel", { name: "List" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Inspect Web to database" })).toBeInTheDocument();
  });

  it("opens a deep-linked flow despite unrelated list filters and uses the canonical backend resolver", async () => {
    mount("/inventory/dataflows?view=topology&flow=flow-1&q=not-visible", true);
    expect(await screen.findByRole("dialog", { name: flow.name })).toBeInTheDocument();
    await waitFor(() => expect(api.resolveDataflowEndpoint).toHaveBeenCalledTimes(2));
    expect(api.resolveDataflowEndpoint).toHaveBeenCalledWith({ object_type: "dcim.device", object_id: "host-1", address_mode: "default", ipaddresses: [] });
    expect(screen.queryByRole("button", { name: "Actions for Web to database" })).not.toBeInTheDocument();
  });

  it("filters exact endpoints only, preserves map context while switching, and can clear it", async () => {
    api.listDataflows.mockResolvedValue([flow, { ...flow, id: "scope", name: "Region scope", source: { ...flow.source, object_type: "dcim.region", object_id: "region" } , destination: { ...flow.destination, object_id: "other" } }]);
    mount("/inventory/dataflows?view=topology&endpointType=dcim.device&endpointId=host-1", true);
    expect(await screen.findByRole("button", { name: "Inspect Web to database" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Inspect Region scope" })).not.toBeInTheDocument();
    expect(screen.getByText(/Exact source or destination matches only/)).toBeInTheDocument();
    expect(api.resolveDataflowEndpoint).not.toHaveBeenCalled();
    fireEvent.keyDown(screen.getByRole("tab", { name: "List" }), { key: "Enter" });
    expect(screen.getByRole("list", { name: "Directed connections" })).toBeInTheDocument();
    expect(screen.getByTestId("current-url")).toHaveTextContent("endpointId=host-1");
    fireEvent.click(screen.getByRole("button", { name: "Show all endpoints" }));
    expect(await screen.findByRole("button", { name: "Inspect Region scope" })).toBeInTheDocument();
  });

  it("handles unknown deep links and invalid endpoint filters without guessing", async () => {
    mount("/inventory/dataflows?view=topology&flow=missing&endpointType=unknown&endpointId=host-1");
    expect(await screen.findByText("This flow is unavailable or no longer visible.")).toBeInTheDocument();
    expect(screen.getByText("Invalid endpoint filter")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Inspect Web to database" })).not.toBeInTheDocument();
    expect(api.resolveDataflowEndpoint).not.toHaveBeenCalled();
  });

  it("keeps the complete list for large graphs and narrows all modes by status", async () => {
    api.listDataflows.mockResolvedValue(Array.from({ length: 85 }, (_, index) => ({ ...flow, id: `flow-${index}`, name: `Connection ${index}`, status: index === 84 ? "draft" : "active" })));
    mount("/inventory/dataflows?view=topology");
    expect(await screen.findByRole("list", { name: "Directed connections" })).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: /^Inspect Connection/ })).toHaveLength(85);
    expect(screen.getByRole("button", { name: "Inspect Connection 84" })).toBeInTheDocument();
    expect(screen.getByText(/More than 80 connections match/)).toBeInTheDocument();
    fireEvent.change(screen.getByRole("combobox", { name: "Status" }), { target: { value: "draft" } });
    expect(await screen.findByRole("region", { name: "Scrollable connection topology" })).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: /^Inspect Connection/ })).toHaveLength(1);
  });
  it("filters by named endpoints", async () => {
    mount();
    await screen.findByRole("button", { name: "Inspect Web to database" });
    fireEvent.change(screen.getByRole("textbox", { name: "Search names or endpoints…" }), { target: { value: "Database" } });
    expect(screen.getByRole("button", { name: "Inspect Web to database" })).toBeInTheDocument();
    fireEvent.change(screen.getByRole("textbox", { name: "Search names or endpoints…" }), { target: { value: "not-a-host" } });
    expect(screen.queryByText(flow.name)).not.toBeInTheDocument();
  });
  it("disables a flow and confirms deletion before sending", async () => {
    mount();
    await screen.findByRole("button", { name: "Inspect Web to database" });
    await flowAction("Disable");
    await waitFor(() => expect(api.updateDataflow).toHaveBeenCalledWith("flow-1", { status: "disabled" }));
    await flowAction("Delete");
    expect(api.deleteDataflow).not.toHaveBeenCalled();
    fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Delete"  }));
    await waitFor(() => expect(api.deleteDataflow).toHaveBeenCalledWith("flow-1", expect.anything()));
  });

  it("hides mutating actions for a read-only user", async () => {
    mount("/inventory/dataflows", true);
    await screen.findByRole("button", { name: "Inspect Web to database" });
    expect(screen.queryByRole("link", { name: "New dataflow" })).not.toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Edit"  })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Disable" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Delete"  })).not.toBeInTheDocument();
  });

  it("fetches readable policy and exports only complete current-host data", async () => {
    const createUrl = vi.fn(() => "blob:policy");
    const revoke = vi.fn();
    vi.stubGlobal("URL", class extends URL { static override createObjectURL = createUrl; static override revokeObjectURL = revoke; });
    const click = vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => undefined);
    mount();
    await openPolicy();
    expect(screen.getByRole("button", { name: "Export JSON" })).toBeDisabled();
    await choose("Host", "Web host");
    fireEvent.click(screen.getByRole("button", { name: "Fetch policy" }));
    await waitFor(() => expect(api.getDataflowPolicy).toHaveBeenCalledWith({ object_type: "dcim.device", object_id: "host-1" }));
    expect(await screen.findByText("Egress")).toBeInTheDocument();
    expect(screen.getByText("192.0.2.2/32")).toBeInTheDocument();
    expect(screen.getByText("flow-1")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Export JSON" }));
    expect(createUrl).toHaveBeenCalledWith(expect.any(Blob));
    expect(click).toHaveBeenCalledOnce();
    expect(revoke).toHaveBeenCalledWith("blob:policy");
    await choose("Host", "Database host");
    expect(screen.getByRole("button", { name: "Export JSON" })).toBeDisabled();
    expect(screen.queryByText("192.0.2.2/32")).not.toBeInTheDocument();
    click.mockRestore();
    vi.unstubAllGlobals();
  });

  it("allows inspecting blocked addresses but prevents rule display and export", async () => {
    api.getDataflowPolicy.mockResolvedValue({ ...policy, apply_ready: false, diagnostics: [{ code: "endpoint_unresolved", message: "Endpoint has no usable address", dataflow_id: "flow-1" }] });
    mount();
    await openPolicy();
    await choose("Host", "Web host");
    fireEvent.click(screen.getByRole("button", { name: "Fetch policy" }));
    expect(await screen.findByText(/An endpoint could not be resolved safely/)).toBeInTheDocument();
    expect(screen.getByText(/Policy blocked/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Export JSON" })).toBeDisabled();
    expect(screen.queryByText("192.0.2.2/32")).not.toBeInTheDocument();
  });

  it("does not export an old complete result after a refetch fails", async () => {
    mount();
    await openPolicy();
    await choose("Host", "Web host");
    fireEvent.click(screen.getByRole("button", { name: "Fetch policy" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Export JSON" })).toBeEnabled());
    api.getDataflowPolicy.mockRejectedValue(new Error("backend raw detail"));
    fireEvent.click(screen.getByRole("button", { name: "Fetch policy" }));
    expect(await screen.findByText(/Could not load host policy/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Export JSON" })).toBeDisabled();
    expect(screen.queryByText("backend raw detail")).not.toBeInTheDocument();
  });

  it("shows complete empty policy caveats and sanitizes unknown diagnostic messages", async () => {
    api.getDataflowPolicy.mockResolvedValue({ host, apply_ready: true, rules: [], diagnostics: [{ code: "no_relevant_flows", message: "No relevant active intent", dataflow_id: null }, { code: "unrecognized", message: "raw private details", dataflow_id: null }] });
    mount();
    await openPolicy();
    await choose("Host", "Web host");
    fireEvent.click(screen.getByRole("button", { name: "Fetch policy" }));
    expect(await screen.findByText("No active rules. This does not specify allow/deny defaults.")).toBeInTheDocument();
    expect(screen.getByText(/No relevant active flows/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Export JSON" })).toBeEnabled();
    expect(screen.queryByText("raw private details")).not.toBeInTheDocument();
  });

  it("renders nullable rule ports and wildcard protocol safely", async () => {
    api.getDataflowPolicy.mockResolvedValue({ ...policy, rules: [{ ...policy.rules[0], transport: "any", destination_ports: null }] });
    mount();
    await openPolicy();
    await choose("Host", "Web host");
    fireEvent.click(screen.getByRole("button", { name: "Fetch policy" }));
    expect(await screen.findByText("Any protocol")).toBeInTheDocument();
    expect(screen.getByText("Any ports")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Export JSON" })).toBeEnabled();
  });

  it("explains excluded All members by stable code without treating the policy as blocked", async () => {
    api.getDataflowPolicy.mockResolvedValue({ ...policy, diagnostics: [{ code: "inventory_excluded", message: "raw private details", dataflow_id: "flow-1" }] });
    mount();
    await openPolicy();
    await choose("Host", "Web host");
    fireEvent.click(screen.getByRole("button", { name: "Fetch policy" }));
    expect(await screen.findByText(/Inventory members without eligible addresses/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Export JSON" })).toBeEnabled();
    expect(screen.queryByText("raw private details")).not.toBeInTheDocument();
  });

  it("shows each connection once and loads details and policy only on request", async () => {
    mount();
    const inspect = await screen.findByRole("button", { name: "Inspect Web to database" });
    expect(screen.getAllByText(flow.name)).toHaveLength(1);
    expect(screen.queryByText(flow.description)).not.toBeInTheDocument();
    expect(screen.queryByRole("table")).not.toBeInTheDocument();
    expect(screen.queryByRole("combobox", { name: "Host" })).not.toBeInTheDocument();
    expect(api.resolveDataflowEndpoint).not.toHaveBeenCalled();
    expect(api.listAllResources).not.toHaveBeenCalled();
    fireEvent.click(inspect);
    const detail = await screen.findByRole("dialog", { name: flow.name });
    await waitFor(() => expect(api.resolveDataflowEndpoint).toHaveBeenCalledTimes(2));
    expect(within(detail).getByText(flow.description)).not.toBeVisible();
    fireEvent.click(within(detail).getByText("Description and evidence"));
    expect(within(detail).getByText(flow.description)).toBeVisible();
    fireEvent.click(within(detail).getByRole("button", { name: "Close" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    await waitFor(() => expect(inspect).toHaveFocus());
    await openPolicy();
    await choose("Host", "Web host");
    fireEvent.click(screen.getByRole("button", { name: "Fetch policy" }));
    await screen.findByText("Egress");
    expect(screen.getAllByRole("table")).toHaveLength(1);
  });
});
