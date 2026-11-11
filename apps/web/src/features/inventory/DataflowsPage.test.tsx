import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes, useLocation, useNavigate } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { changeAppLanguage } from "@/shared/i18n";
import { AllPermissions } from "@/test/permissions";
import { PermissionsContext } from "@/shared/auth/permissionsContext";
import DataflowsPage from "./DataflowsPage";
import en from "@/shared/i18n/locales/en";
import { parseDataflowPorts } from "./dataflowPorts";
import type { Dataflow, DataflowEndpointResolution } from "./dataflows-api";

const TAG_CORE = "tag-core";
const TAG_CAMPUS = "tag-campus";

const api = vi.hoisted(() => ({
  listDataflows: vi.fn(), updateDataflow: vi.fn(), deleteDataflow: vi.fn(), bulkDataflows: vi.fn(),
  getDataflowPolicy: vi.fn(), listAllResources: vi.fn(), resolveDataflowEndpoint: vi.fn(),
  listApplications: vi.fn(), listDataflowGroups: vi.fn(),
}));
vi.mock("./dataflows-api", () => ({
  listDataflows: api.listDataflows, updateDataflow: api.updateDataflow, deleteDataflow: api.deleteDataflow,
  bulkDataflows: api.bulkDataflows, getDataflowPolicy: api.getDataflowPolicy, resolveDataflowEndpoint: api.resolveDataflowEndpoint,
  dataflowEndpoints: (flow: { sources: unknown[]; destinations: unknown[] }) => [...flow.sources, ...flow.destinations],
}));
vi.mock("./dataflowCatalog-api", async original => ({ ...(await original()), listApplications: api.listApplications, listDataflowGroups: api.listDataflowGroups }));
vi.mock("./api", () => ({ listAllResources: api.listAllResources }));

const endpoint = (object_id: string, label: string) => ({ object_type: "dcim.device" as const, object_id, label, address_mode: "default" as const, ipaddresses: [] });

const flow: Dataflow = {
  id: "flow-1", name: "Web to database", description: "Application traffic", comments: "Change request 42",
  status: "active", effective_status: "active", blocked_by_group_id: null,
  transport: "tcp", destination_ports: [443], source_ports: null, icmp_types: null,
  allow_cross_vrf: false, application_id: "app-web", group_id: "group-prod",
  owner_id: null, tenant_id: null, tags: [TAG_CORE], inherited_tags: [TAG_CAMPUS], effective_tags: [TAG_CORE, TAG_CAMPUS], custom_field_data: {},
  sources: [endpoint("host-1", "Web host")], destinations: [endpoint("host-2", "Database host")],
};

const second: Dataflow = {
  ...flow, id: "flow-2", name: "Database replication", application_id: "app-data", group_id: null,
  transport: "udp", destination_ports: null, tags: [], inherited_tags: [TAG_CAMPUS], effective_tags: [TAG_CAMPUS],
  sources: [endpoint("host-2", "Database host")], destinations: [endpoint("host-3", "Replica host")],
};

const multi: Dataflow = {
  ...flow, id: "flow-3", name: "Fleet access", status: "active", effective_status: "disabled", blocked_by_group_id: "group-paused",
  sources: [endpoint("host-1", "Web host"), endpoint("host-4", "Application host"), endpoint("host-5", "Worker host")],
};

const host = { object_type: "dcim.device", object_id: "host-1", label: "Web host" };
const policy = {
  host, apply_ready: true, diagnostics: [],
  rules: [{ direction: "egress", family: "ipv4", transport: "tcp", source: "192.0.2.1/32", destination: "192.0.2.2/32", destination_ports: [443], vrf_id: null, dataflow_ids: ["flow-1"] }],
};

function NavigationProbe() {
  const location = useLocation();
  const navigate = useNavigate();
  return <><span data-testid="current-url">{location.pathname}{location.search}{location.hash}</span><button data-testid="browser-back" onClick={() => navigate(-1)}>Browser back</button><button data-testid="browser-forward" onClick={() => navigate(1)}>Browser forward</button></>;
}

function mount(path = "/inventory/dataflows", readOnly = false) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const content = <MemoryRouter initialEntries={[path]}><Routes>
    <Route path="/inventory/dataflows" element={<DataflowsPage />} />
    <Route path="/inventory/dataflows/new" element={<div>New dataflow editor</div>} />
    <Route path="/inventory/dataflows/:id/edit" element={<div>Edit dataflow editor</div>} />
  </Routes><NavigationProbe /></MemoryRouter>;
  const view = render(<QueryClientProvider client={client}>{readOnly ? <PermissionsContext.Provider value={{ roles: [], isAdmin: false, can: action => action === "READ", canAny: () => false, hasRole: () => false }}>{content}</PermissionsContext.Provider> : <AllPermissions>{content}</AllPermissions>}</QueryClientProvider>);
  return { ...view, client };
}

async function choose(label: string, name: string) {
  const picker = await screen.findByRole("combobox", { name: label });
  await waitFor(() => expect(picker).toBeEnabled());
  fireEvent.click(picker);
  fireEvent.click(await screen.findByRole("option", { name: new RegExp(name) }));
}

async function openPolicy() {
  fireEvent.click(screen.getByRole("button", { name: "Export JSON" }));
  await screen.findByRole("dialog", { name: "Host firewall policy" });
}

async function flowAction(name: string) {
  fireEvent.keyDown(screen.getByRole("button", { name: "Actions for Web to database" }), { key: "Enter" });
  fireEvent.click(await screen.findByRole("menuitem", { name }));
}

function selectRows(count: number) {
  const checkboxes = screen.getAllByRole("checkbox").slice(1);
  expect(checkboxes).toHaveLength(count);
  for (const checkbox of checkboxes) fireEvent.click(checkbox);
  return checkboxes;
}

beforeEach(() => {
  vi.clearAllMocks();
  api.listDataflows.mockResolvedValue([flow]);
  api.updateDataflow.mockResolvedValue(flow);
  api.deleteDataflow.mockResolvedValue(undefined);
  api.bulkDataflows.mockResolvedValue({ action: "set_status", affected: [{ id: "flow-1" }, { id: "flow-2" }] });
  api.listApplications.mockResolvedValue([
    { id: "app-web", name: "Web platform" }, { id: "app-data", name: "Core data" },
  ]);
  api.listDataflowGroups.mockResolvedValue([
    { id: "group-prod", name: "Production" }, { id: "group-paused", name: "Paused legacy" },
  ]);
  api.listAllResources.mockImplementation((entity: string) => Promise.resolve(entity === "Tags"
    ? [{ id: TAG_CORE, name: "Core network", type: "tag" }, { id: TAG_CAMPUS, name: "Campus", type: "tag" }]
    : entity === "Devices" ? [{ id: "host-1", name: "Web host", type: "device" }, { id: "host-2", name: "Database host", type: "device" }] : []));
  api.getDataflowPolicy.mockResolvedValue(policy);
  api.resolveDataflowEndpoint.mockImplementation((value: DataflowEndpointResolution["endpoint"]) => Promise.resolve({
    endpoint: value, members: [], addresses: [{ cidr: "192.0.2.1/32", vrf_id: null }], diagnostics: [],
    ipaddresses: [{ id: "ip-1", address: "192.0.2.1/32", vrf_id: null }], default_ipaddresses: ["ip-1"],
  } satisfies DataflowEndpointResolution));
});

describe("dataflow catalog list", () => {
  it("opens the compact filter disclosure and keeps active filters in the URL", async () => {
    mount();
    await screen.findByRole("table");
    fireEvent.click(screen.getByRole("button", { name: en.dataflows.filters }));
    const panel = await screen.findByRole("dialog", { name: en.dataflows.filters });
    fireEvent.change(within(panel).getByRole("combobox", { name: en.dataflows.applicationFilter }), { target: { value: "app-web" } });
    expect(screen.getByTestId("current-url")).toHaveTextContent("application=app-web");
    expect(screen.getByRole("button", { name: en.dataflows.filtersWithCount.replace("{{count}}", "1") })).toBeInTheDocument();
    fireEvent.click(within(panel).getByRole("button", { name: en.dataflows.resetFilters }));
    expect(screen.getByTestId("current-url")).not.toHaveTextContent("application=app-web");
  });

  it("shows catalog navigation, default columns and one effective status per row", async () => {
    mount();
    const table = await screen.findByRole("table");
    expect(screen.getByRole("navigation", { name: en.dataflows.catalogNavLabel })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: en.dataflows.catalogFlows })).toHaveAttribute("aria-current", "page");
    for (const name of [en.name, en.dataflows.source, en.dataflows.destination, en.dataflows.transport, en.dataflows.effectiveStatus]) {
      expect(screen.getByRole("columnheader", { name })).toBeInTheDocument();
    }
    expect(within(table).getByText("Web platform · Production")).toBeInTheDocument();
    expect(within(table).getByText("Web host")).toBeInTheDocument();
    expect(within(table).getByText("Database host")).toBeInTheDocument();
    expect(within(table).getByText("TCP")).toBeInTheDocument();
    expect(within(table).getByText("443")).toBeInTheDocument();
    expect(within(table).getAllByText("Active")).toHaveLength(1);
    expect(within(table).queryByText(TAG_CORE)).not.toBeInTheDocument();
    expect(within(table).queryByText("dcim.device")).not.toBeInTheDocument();
  });

  it("bounds multi-endpoint sides, shows effective status and the blocking ancestor", async () => {
    api.listDataflows.mockResolvedValue([multi]);
    mount();
    await screen.findByRole("table");
    expect(screen.getByText("Web host +2")).toBeInTheDocument();
    expect(screen.getByText("Database host")).toBeInTheDocument();
    expect(screen.getByTitle("Blocked by group: Paused legacy")).toBeInTheDocument();
    const table = screen.getByRole("table");
    expect(within(table).queryByText("Active")).not.toBeInTheDocument();
    expect(within(table).getByText("Disabled")).toBeInTheDocument();
  });

  it("filters client-side by application, group, transport, effective status and effective tag while preserving the URL", async () => {
    api.listDataflows.mockResolvedValue([flow, second, { ...multi, inherited_tags: [], effective_tags: [TAG_CORE] }]);
    mount();
    await screen.findByRole("table");
    expect(screen.getByText("Web to database")).toBeInTheDocument();
    expect(screen.getByText("Database replication")).toBeInTheDocument();
    expect(screen.getByText("Fleet access")).toBeInTheDocument();

    fireEvent.change(screen.getByRole("combobox", { name: en.dataflows.applicationFilter }), { target: { value: "app-data" } });
    await waitFor(() => expect(screen.getByText("Database replication")).toBeInTheDocument());
    expect(screen.queryByText("Web to database")).not.toBeInTheDocument();
    expect(screen.getByTestId("current-url")).toHaveTextContent("application=app-data");

    fireEvent.change(screen.getByRole("combobox", { name: en.dataflows.applicationFilter }), { target: { value: "" } });
    fireEvent.change(screen.getByRole("combobox", { name: en.dataflows.groupFilter }), { target: { value: "group-prod" } });
    await waitFor(() => expect(screen.getByText("Web to database")).toBeInTheDocument());
    expect(screen.queryByText("Database replication")).not.toBeInTheDocument();
    expect(screen.getByTestId("current-url")).toHaveTextContent("group=group-prod");

    fireEvent.change(screen.getByRole("combobox", { name: en.dataflows.groupFilter }), { target: { value: "" } });
    fireEvent.change(screen.getByRole("combobox", { name: en.dataflows.transportFilter }), { target: { value: "udp" } });
    await waitFor(() => expect(screen.getByText("Database replication")).toBeInTheDocument());
    expect(screen.queryByText("Web to database")).not.toBeInTheDocument();
    expect(screen.getByTestId("current-url")).toHaveTextContent("transport=udp");

    fireEvent.change(screen.getByRole("combobox", { name: en.dataflows.transportFilter }), { target: { value: "" } });
    fireEvent.change(screen.getByRole("combobox", { name: en.dataflows.effectiveStatusFilter }), { target: { value: "disabled" } });
    await waitFor(() => expect(screen.getByText("Fleet access")).toBeInTheDocument());
    expect(screen.queryByText("Web to database")).not.toBeInTheDocument();
    expect(screen.getByTestId("current-url")).toHaveTextContent("status=disabled");

    fireEvent.change(screen.getByRole("combobox", { name: en.dataflows.effectiveStatusFilter }), { target: { value: "all" } });
    fireEvent.change(screen.getByRole("combobox", { name: en.dataflows.tagFilter }), { target: { value: TAG_CAMPUS } });
    await waitFor(() => expect(screen.getByText("Web to database")).toBeInTheDocument());
    expect(screen.getByText("Database replication")).toBeInTheDocument();
    expect(screen.queryByText("Fleet access")).not.toBeInTheDocument();
    expect(screen.getByTestId("current-url")).toHaveTextContent(`tag=${TAG_CAMPUS}`);
  });

  it("keeps tag selector scope in the exact endpoint filter and URL", async () => {
    const tagged = (scope: "dcim.device" | "virtualization.virtualmachine") => ({
      object_type: "extras.tag" as const, object_id: "tag-shared", label: "Shared tag", address_mode: "default" as const,
      ipaddresses: [], tag_object_type: scope,
    });
    const deviceFlow: Dataflow = { ...flow, id: "tag-device", name: "Device tag flow", sources: [tagged("dcim.device")] };
    const vmFlow: Dataflow = { ...flow, id: "tag-vm", name: "VM tag flow", sources: [tagged("virtualization.virtualmachine")] };
    api.listDataflows.mockResolvedValue([deviceFlow, vmFlow]);
    const first = mount("/inventory/dataflows?endpointType=extras.tag&endpointId=tag-shared&endpointTagObjectType=dcim.device");
    await screen.findByText("Device tag flow");
    expect(screen.queryByText("VM tag flow")).not.toBeInTheDocument();
    expect(screen.getByTestId("current-url")).toHaveTextContent("endpointTagObjectType=dcim.device");
    fireEvent.click(screen.getByRole("button", { name: en.dataflows.clearEndpointFilter }));
    await screen.findByText("VM tag flow");
    first.unmount();
    mount("/inventory/dataflows?endpointType=extras.tag&endpointId=tag-shared");
    expect(await screen.findByText("Device tag flow")).toBeInTheDocument();
    expect(screen.getByText("VM tag flow")).toBeInTheDocument();
  });

  it("filters group descendants inclusively", async () => {
    api.listDataflowGroups.mockResolvedValue([
      { id: "group-prod", name: "Production" },
      { id: "group-edge", name: "Edge", parent_id: "group-prod" },
      { id: "group-other", name: "Other" },
    ]);
    api.listDataflows.mockResolvedValue([
      flow,
      { ...flow, id: "child-flow", name: "Edge child flow", group_id: "group-edge" },
      { ...flow, id: "other-flow", name: "Other flow", group_id: "group-other" },
    ]);
    mount("/inventory/dataflows?group=group-prod");
    await screen.findByText("Web to database");
    expect(screen.getByText("Edge child flow")).toBeInTheDocument();
    expect(screen.queryByText("Other flow")).not.toBeInTheDocument();
  });

  it("reports unknown filter values without rewriting the URL", async () => {
    mount("/inventory/dataflows?application=missing&transport=typo");
    expect(await screen.findByText(en.dataflows.invalidFilter)).toBeInTheDocument();
    expect(screen.getByTestId("current-url")).toHaveTextContent("application=missing&transport=typo");
    fireEvent.click(screen.getAllByRole("button", { name: en.dataflows.resetViewFilters })[0]!);
    await screen.findByRole("table");
    expect(screen.getByTestId("current-url")).not.toHaveTextContent("application=missing");
  });

  it("keeps search and exact endpoint filters client-side", async () => {
    api.listDataflows.mockResolvedValue([flow, second]);
    const first = mount();
    await screen.findByRole("table");
    fireEvent.change(screen.getByRole("textbox", { name: en.dataflows.search }), { target: { value: "Replica" } });
    await waitFor(() => expect(screen.getByText("Database replication")).toBeInTheDocument());
    expect(screen.queryByText("Web to database")).not.toBeInTheDocument();
    fireEvent.change(screen.getByRole("textbox", { name: en.dataflows.search }), { target: { value: "" } });
    await waitFor(() => expect(screen.getByText("Web to database")).toBeInTheDocument());
    first.unmount();
    mount("/inventory/dataflows?endpointType=dcim.device&endpointId=host-1");
    await waitFor(() => expect(screen.getByText("Web to database")).toBeInTheDocument());
    expect(screen.queryByText("Database replication")).not.toBeInTheDocument();
  });
});

describe("dataflow catalog bulk and row actions", () => {
  it("performs an atomic bulk status update and clears the selection on success", async () => {
    api.listDataflows.mockResolvedValue([flow, second]);
    mount();
    await screen.findByRole("table");
    selectRows(2);
    const bar = screen.getByRole("region", { name: "2 selected" });
    fireEvent.click(within(bar).getByRole("button", { name: en.dataflows.bulkActivate }));
    await waitFor(() => expect(api.bulkDataflows).toHaveBeenCalledWith(expect.objectContaining({
      action: "set_status", status: "active", ids: expect.arrayContaining(["flow-1", "flow-2"]),
    })));
    expect(await screen.findByRole("status")).toHaveTextContent("Updated flows: 2.");
    await waitFor(() => expect(screen.queryByRole("region", { name: "2 selected" })).not.toBeInTheDocument());
    await waitFor(() => expect(api.listDataflows).toHaveBeenCalledTimes(2));
  });

  it("keeps rows selected and the move draft after a failed bulk action", async () => {
    api.listDataflows.mockResolvedValue([flow, second]);
    api.bulkDataflows.mockRejectedValue(new Error("raw backend detail"));
    mount();
    await screen.findByRole("table");
    const checkboxes = selectRows(2);
    const bar = screen.getByRole("region", { name: "2 selected" });
    fireEvent.click(within(bar).getByRole("button", { name: en.dataflows.bulkMove }));
    const dialog = await screen.findByRole("dialog", { name: en.dataflows.bulkMoveTitle });
    fireEvent.change(within(dialog).getByRole("combobox", { name: en.dataflows.bulkGroup }), { target: { value: "group-prod" } });
    fireEvent.click(within(dialog).getByRole("button", { name: en.dataflows.bulkMove }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(en.dataflows.bulkFailed);
    expect(screen.queryByText("raw backend detail")).not.toBeInTheDocument();
    expect(within(dialog).getByRole("combobox", { name: en.dataflows.bulkGroup })).toHaveValue("group-prod");
    fireEvent.click(within(dialog).getByRole("button", { name: en.cancel }));
    expect(screen.getByRole("region", { name: "2 selected" })).toBeInTheDocument();
    for (const checkbox of checkboxes) expect(checkbox).toHaveAttribute("aria-checked", "true");
  });

  it("moves flows and clears group or application explicitly with null", async () => {
    api.listDataflows.mockResolvedValue([flow, second]);
    api.bulkDataflows.mockResolvedValue({ action: "move", affected: [{ id: "flow-1" }, { id: "flow-2" }] });
    mount();
    await screen.findByRole("table");
    selectRows(2);
    fireEvent.click(within(screen.getByRole("region", { name: "2 selected" })).getByRole("button", { name: en.dataflows.bulkMove }));
    const dialog = await screen.findByRole("dialog", { name: en.dataflows.bulkMoveTitle });
    fireEvent.change(within(dialog).getByRole("combobox", { name: en.dataflows.bulkGroup }), { target: { value: "__none__" } });
    fireEvent.change(within(dialog).getByRole("combobox", { name: en.dataflows.bulkApplication }), { target: { value: "app-data" } });
    fireEvent.click(within(dialog).getByRole("button", { name: en.dataflows.bulkMove }));
    await waitFor(() => expect(api.bulkDataflows).toHaveBeenCalledWith(expect.objectContaining({
      action: "move", group_id: null, application_id: "app-data", ids: expect.arrayContaining(["flow-1", "flow-2"]),
    })));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: en.dataflows.bulkMoveTitle })).not.toBeInTheDocument());
  });

  it("opens a prefilled draft for Clone without sending a clone request", async () => {
    mount();
    await screen.findByRole("table");
    await flowAction(en.dataflows.cloneFlow);
    expect(await screen.findByText("New dataflow editor")).toBeInTheDocument();
    expect(screen.getByTestId("current-url")).toHaveTextContent("/inventory/dataflows/new?clone=flow-1");
    expect(api.updateDataflow).not.toHaveBeenCalled();
    expect(api.bulkDataflows).not.toHaveBeenCalled();
  });

  it("keeps single disable and confirmed delete actions", async () => {
    mount();
    await screen.findByRole("table");
    await flowAction(en.dataflows.disable);
    await waitFor(() => expect(api.updateDataflow).toHaveBeenCalledWith("flow-1", { status: "disabled" }));
    await flowAction("Delete");
    expect(api.deleteDataflow).not.toHaveBeenCalled();
    fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(api.deleteDataflow).toHaveBeenCalledWith("flow-1", expect.anything()));
  });

  it("hides selection and mutating actions for a read-only user", async () => {
    mount("/inventory/dataflows", true);
    await screen.findByRole("table");
    expect(screen.queryAllByRole("checkbox")).toHaveLength(0);
    expect(screen.queryByRole("link", { name: en.dataflows.newTitle })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Actions for/ })).not.toBeInTheDocument();
  });
});

describe("dataflow detail, focus and history", () => {
  it("shows readable endpoints, application, group, effective status, blocking group and tag labels without raw identity rows", async () => {
    api.listDataflows.mockResolvedValue([multi]);
    mount();
    await screen.findByRole("table");
    fireEvent.click(screen.getByRole("button", { name: "Inspect Fleet access" }));
    const dialog = await screen.findByRole("dialog", { name: "Fleet access" });
    expect(within(dialog).getByText("Web platform")).toBeInTheDocument();
    expect(within(dialog).getByText("Production")).toBeInTheDocument();
    expect(within(dialog).getByText(en.dataflows.blockedByGroup.replace("{{name}}", "Paused legacy"))).toBeInTheDocument();
    expect(await within(dialog).findByText("Core network")).toBeInTheDocument();
    expect(within(dialog).getByText("Campus")).toBeInTheDocument();
    expect(within(dialog).getByRole("heading", { name: en.dataflows.directTags })).toBeInTheDocument();
    expect(within(dialog).getByRole("heading", { name: en.dataflows.inheritedTags })).toBeInTheDocument();
    expect(within(dialog).getByText("Change request 42")).toBeVisible();
    expect(within(dialog).getByRole("link", { name: en.dataflows.policyTitle })).toHaveAttribute("href", expect.stringContaining("policy=1"));
    expect(within(dialog).queryByText(/host-1|dcim\.device|default/)).not.toBeInTheDocument();
    expect(within(dialog).getByText("Disabled")).toBeInTheDocument();
    for (const name of ["Worker host", "Application host"]) expect(within(dialog).getByRole("heading", { name })).toBeInTheDocument();
  });

  it("opens and closes with focus restoration and restores the dialog through browser back", async () => {
    mount();
    const opener = await screen.findByRole("button", { name: "Inspect Web to database" });
    fireEvent.click(opener);
    expect(await screen.findByRole("dialog", { name: flow.name })).toBeInTheDocument();
    expect(screen.getByTestId("current-url")).toHaveTextContent("flow=flow-1");
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: flow.name })).not.toBeInTheDocument());
    await waitFor(() => expect(opener).toHaveFocus());
    fireEvent.click(screen.getByTestId("browser-back"));
    expect(await screen.findByRole("dialog", { name: flow.name })).toBeInTheDocument();
    fireEvent.click(screen.getByTestId("browser-back"));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: flow.name })).not.toBeInTheDocument());
  });

  it("switches views with links and preserves query, hash and browser history", async () => {
    mount("/inventory/dataflows?q=Web&status=active&endpointType=dcim.device&endpointId=host-1&other=keep#context");
    await screen.findByRole("table");
    const topology = screen.getByRole("link", { name: en.dataflows.topologyView });
    expect(topology).toHaveAttribute("href", expect.stringContaining("other=keep&view=topology#context"));
    fireEvent.click(topology);
    expect(topology).toHaveAttribute("aria-current", "page");
    expect(await screen.findByRole("region", { name: "Scrollable connection topology" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Browser back" }));
    expect(await screen.findByRole("table")).toBeInTheDocument();
    expect(screen.getByTestId("current-url")).toHaveTextContent("other=keep#context");
    fireEvent.click(screen.getByRole("button", { name: "Browser forward" }));
    expect(await screen.findByRole("link", { name: en.dataflows.topologyView })).toHaveAttribute("aria-current", "page");
  });

  it("handles unknown deep links and preserves unrelated list filters", async () => {
    mount("/inventory/dataflows?flow=missing&q=not-visible");
    expect(await screen.findByText(en.dataflows.selectionUnavailable)).toBeInTheDocument();
    expect(screen.getByTestId("current-url")).toHaveTextContent("flow=missing&q=not-visible");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });
});

describe("dataflow policy panel", () => {
  it("restricts host policy types to Device and VM", async () => {
    mount();
    await openPolicy();
    fireEvent.keyDown(screen.getByRole("combobox", { name: "Host type" }), { key: "ArrowDown" });
    expect(await screen.findByRole("option", { name: "Device" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "Virtual machine" })).toBeInTheDocument();
    expect(screen.getAllByRole("option")).toHaveLength(2);
    expect(api.resolveDataflowEndpoint).not.toHaveBeenCalled();
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
    expect(await screen.findByRole("dialog", { name: "Host firewall policy" })).toBeInTheDocument();
    await screen.findByText("Egress");
    const ruleTable = screen.getByRole("table");
    expect(within(ruleTable).getByText("Any protocol")).toBeInTheDocument();
    expect(within(ruleTable).getAllByText("Any ports")).toHaveLength(2);
    expect(within(ruleTable).getByRole("columnheader", { name: "Source ports" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Export JSON" })).toBeEnabled();
  });
});

describe("dataflow utilities", () => {
  it("bounds distinct ports and accepts both endpoints of port domain", () => {
    expect(parseDataflowPorts("65535,1,1")).toEqual([1, 65535]);
    expect(parseDataflowPorts(Array.from({ length: 257 }, (_, i) => String(i + 1)).join(","))).toBeNull();
  });

  it("renders Russian catalog labels", async () => {
    await changeAppLanguage("ru");
    const view = mount();
    try {
      expect(await screen.findByRole("heading", { name: "Потоки данных" })).toBeInTheDocument();
      expect(screen.getByRole("navigation", { name: "Каталог потоков" })).toBeInTheDocument();
      expect(screen.getByRole("combobox", { name: "Действующий статус" })).toBeInTheDocument();
    } finally {
      view.unmount();
      await act(() => changeAppLanguage("en"));
    }
  });
});
