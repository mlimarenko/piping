import { fireEvent, screen, waitFor } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { PermissionsContext } from "@/shared/auth/permissionsContext";
import "@/shared/i18n";
import DataflowEditorPage from "./DataflowEditorPage";

const api = vi.hoisted(() => ({
  getDataflow: vi.fn(), createDataflow: vi.fn(), updateDataflow: vi.fn(), resolveDataflowEndpoint: vi.fn(),
  listApplications: vi.fn(), listDataflowGroups: vi.fn(), listAddressGroups: vi.fn(), listAllResources: vi.fn(),
}));
vi.mock("./dataflows-api", async original => ({
  ...(await original()),
  getDataflow: api.getDataflow, createDataflow: api.createDataflow, updateDataflow: api.updateDataflow,
  resolveDataflowEndpoint: api.resolveDataflowEndpoint,
}));
vi.mock("./dataflowCatalog-api", async original => ({
  ...(await original()),
  listApplications: api.listApplications, listDataflowGroups: api.listDataflowGroups, listAddressGroups: api.listAddressGroups,
}));
vi.mock("./api", () => ({ listAllResources: api.listAllResources }));

const device = (object_id: string, label: string) => ({ object_type: "dcim.device", object_id, address_mode: "default", ipaddresses: [], label });
const baseFlow = (overrides: Record<string, unknown> = {}) => ({
  id: "flow-1", name: "Recovered flow", description: "", comments: "", status: "draft", transport: "any",
  destination_ports: null, source_ports: null, icmp_types: null, allow_cross_vrf: false,
  application_id: null, group_id: null, owner_id: null, tenant_id: null, tags: [], custom_field_data: {},
  sources: [device("dev-1", "Web host")], destinations: [device("dev-2", "Database host")],
  ...overrides,
});

function mount(path = "/inventory/dataflows/flow-1/edit", writable = true) {
  return render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
    <PermissionsContext.Provider value={{ roles: [], isAdmin: false, can: action => action === "READ" || writable, canAny: () => writable, hasRole: () => false }}>
      <MemoryRouter initialEntries={[path]}><Routes>
        <Route path="/inventory/dataflows/:id/edit" element={<DataflowEditorPage />} />
        <Route path="/inventory/dataflows/new" element={<DataflowEditorPage />} />
        <Route path="/inventory/dataflows" element={<div>dataflow-list</div>} />
      </Routes></MemoryRouter>
    </PermissionsContext.Provider>
  </QueryClientProvider>);
}

async function choose(name: string, option: string) {
  const trigger = screen.getByRole("combobox", { name });
  await waitFor(() => expect(trigger).toBeEnabled());
  fireEvent.click(trigger);
  fireEvent.click(await screen.findByRole("option", { name: option }));
}

async function chooseEntity(name: string, option: string) {
  const trigger = screen.getByRole("combobox", { name });
  await waitFor(() => expect(trigger).toBeEnabled());
  fireEvent.click(trigger);
  fireEvent.click(await screen.findByRole("option", { name: option }));
}

function expectShell() {
  expect(screen.getAllByTestId("entity-editor-page")).toHaveLength(1);
  expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
}

describe("DataflowEditorPage state ownership", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    api.listAllResources.mockImplementation(async (entity: string) => entity === "Devices" ? [
      { id: "dev-1", name: "Web host", type: "device" },
      { id: "dev-2", name: "Database host", type: "device" },
      { id: "dev-3", name: "Edge host", type: "device" },
    ] : []);
    api.resolveDataflowEndpoint.mockResolvedValue({
      addresses: [], default_ipaddresses: [], diagnostics: [], ipaddresses: [], members: [],
      endpoint: { object_type: "dcim.device", object_id: "dev-1", address_mode: "default", ipaddresses: [] },
    });
    api.listApplications.mockResolvedValue([{ id: "app-1", name: "Billing" }, { id: "app-2", name: "Search" }]);
    api.listDataflowGroups.mockResolvedValue([{ id: "grp-1", name: "Edge" }, { id: "grp-2", name: "Core" }]);
    api.listAddressGroups.mockResolvedValue([]);
    api.createDataflow.mockResolvedValue({});
    api.updateDataflow.mockResolvedValue({});
  });

  it("keeps one editor and heading from pending to loaded", async () => {
    let finish!: (value: unknown) => void;
    api.getDataflow.mockReturnValue(new Promise(resolve => { finish = resolve; }));
    mount();
    expect(screen.getByRole("status")).toHaveTextContent("Loading");
    expectShell();
    finish(baseFlow());
    await screen.findByRole("textbox", { name: "Name (optional)" });
    expectShell();
    expect(screen.getByRole("button", { name: "Save Changes" })).toBeInTheDocument();
  });

  it("owns error and retry without granting write access", async () => {
    api.getDataflow.mockRejectedValueOnce(new Error("private backend detail"));
    api.getDataflow.mockResolvedValue(baseFlow());
    mount(undefined, false);
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not load dataflows");
    expect(screen.queryByText("private backend detail")).not.toBeInTheDocument();
    expectShell();
    expect(screen.queryByRole("button", { name: "Save Changes" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /retry/i }));
    await screen.findByRole("textbox", { name: "Name (optional)" });
    expect(screen.getByRole("textbox", { name: "Name (optional)" })).toBeDisabled();
    expect(screen.queryByRole("button", { name: "Save Changes" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(await screen.findByText("dataflow-list")).toBeInTheDocument();
  });

  it("creates with endpoint arrays and blocks an invalid side", async () => {
    mount("/inventory/dataflows/new");
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    expect(await screen.findAllByText("Select both a source and a destination.")).toHaveLength(2);
    expect(api.createDataflow).not.toHaveBeenCalled();
    await chooseEntity("Source 1", "Web host");
    await chooseEntity("Destination 1", "Database host");
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await vi.waitFor(() => expect(api.createDataflow).toHaveBeenCalledOnce());
    const payload = api.createDataflow.mock.calls[0]![0];
    expect(payload.sources).toEqual([{ object_type: "dcim.device", object_id: "dev-1", address_mode: "default", ipaddresses: [] }]);
    expect(payload.destinations).toEqual([{ object_type: "dcim.device", object_id: "dev-2", address_mode: "default", ipaddresses: [] }]);
    expect(payload).toMatchObject({
      status: "draft", transport: "any", destination_ports: null, source_ports: null, icmp_types: null,
      application_id: null, group_id: null, comments: "",
    });
  });

  it("adds and removes endpoints while preserving side order", async () => {
    mount("/inventory/dataflows/new");
    await chooseEntity("Source 1", "Web host");
    await chooseEntity("Destination 1", "Database host");
    fireEvent.click(screen.getByRole("button", { name: "Add source" }));
    await chooseEntity("Source 2", "Edge host");
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await vi.waitFor(() => expect(api.createDataflow).toHaveBeenCalledOnce());
    expect(api.createDataflow.mock.calls[0]![0].sources.map((row: { object_id: string }) => row.object_id)).toEqual(["dev-1", "dev-3"]);
  });

  it("keeps a fresh form neutral and shows the empty-side error only after save", async () => {
    mount("/inventory/dataflows/new");
    expect(screen.queryByText("Add at least one object.")).not.toBeInTheDocument();
    expect(screen.queryByText("Select both a source and a destination.")).not.toBeInTheDocument();
    await chooseEntity("Source 1", "Web host");
    await chooseEntity("Destination 1", "Database host");
    fireEvent.click(screen.getByRole("button", { name: "Remove object Source 1" }));
    expect(screen.queryByText("Add at least one object.")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    expect(screen.getByText("Add at least one object.")).toBeInTheDocument();
    expect(api.createDataflow).not.toHaveBeenCalled();
  });

  it("normalizes destination and source port ranges and displays TCP + UDP", async () => {
    api.getDataflow.mockResolvedValue(baseFlow({ transport: "tcp" }));
    mount();
    await screen.findByRole("textbox", { name: "Name (optional)" });
    await choose("Transport", "TCP + UDP");
    expect(screen.getByRole("combobox", { name: "Transport" })).toHaveTextContent("TCP + UDP");
    await choose("Destination ports mode", "Selected ports");
    fireEvent.change(screen.getByRole("textbox", { name: "Destination ports" }), { target: { value: "8443, 8000-8001" } });
    await choose("Source ports mode", "Selected ports");
    fireEvent.change(screen.getByRole("textbox", { name: "Source ports" }), { target: { value: "53, 53, 1000-1001" } });
    fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));
    await vi.waitFor(() => expect(api.updateDataflow).toHaveBeenCalledOnce());
    expect(api.updateDataflow.mock.calls[0]![1]).toMatchObject({
      transport: "tcp_udp", destination_ports: [8000, 8001, 8443], source_ports: [53, 1000, 1001],
    });
  });

  it("hides ports for ICMP and validates ICMP types before save", async () => {
    api.getDataflow.mockResolvedValue(baseFlow());
    mount();
    await screen.findByRole("textbox", { name: "Name (optional)" });
    await choose("Transport", "ICMP");
    expect(screen.queryByRole("combobox", { name: "Destination ports mode" })).not.toBeInTheDocument();
    expect(screen.queryByRole("combobox", { name: "Source ports mode" })).not.toBeInTheDocument();
    await choose("ICMP types mode", "Selected ICMP types");
    fireEvent.change(screen.getByRole("textbox", { name: "ICMP types" }), { target: { value: "256" } });
    fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));
    expect(screen.getByRole("alert")).toHaveTextContent("Enter unique ICMP types from 0 to 255.");
    expect(api.updateDataflow).not.toHaveBeenCalled();
    fireEvent.change(screen.getByRole("textbox", { name: "ICMP types" }), { target: { value: "8, 0, 8" } });
    fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));
    await vi.waitFor(() => expect(api.updateDataflow).toHaveBeenCalledOnce());
    expect(api.updateDataflow.mock.calls[0]![1]).toMatchObject({
      transport: "icmp", destination_ports: null, source_ports: null, icmp_types: [0, 8],
    });
  });

  it("prefills a clone as a new draft and saves with POST", async () => {
    api.getDataflow.mockResolvedValue(baseFlow({
      status: "active", transport: "tcp", destination_ports: [443], source_ports: [1024], comments: "ops",
      application_id: "app-1", group_id: "grp-1", tags: ["tag-1"], custom_field_data: { rack: "A1" },
      sources: [device("dev-1", "Web host"), device("dev-3", "Edge host")],
    }));
    mount("/inventory/dataflows/new?clone=flow-1");
    expect(await screen.findByRole("textbox", { name: "Name (optional)" })).toHaveValue("Recovered flow (copy)");
    expect(screen.getByRole("combobox", { name: "Status" })).toHaveTextContent("Draft");
    expect(screen.getByRole("button", { name: "Create" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await vi.waitFor(() => expect(api.createDataflow).toHaveBeenCalledOnce());
    expect(api.updateDataflow).not.toHaveBeenCalled();
    const payload = api.createDataflow.mock.calls[0]![0];
    expect(payload).not.toHaveProperty("id");
    expect(payload).toMatchObject({
      name: "Recovered flow (copy)", status: "draft", transport: "tcp", destination_ports: [443], source_ports: [1024],
      comments: "ops", application_id: "app-1", group_id: "grp-1", tags: ["tag-1"], custom_field_data: { rack: "A1" },
    });
    expect(payload.sources.map((row: { object_id: string }) => row.object_id)).toEqual(["dev-1", "dev-3"]);
  });

  it("keeps application and group metadata on update", async () => {
    api.getDataflow.mockResolvedValue(baseFlow({ application_id: "app-1", group_id: "grp-1" }));
    mount();
    expect(await screen.findByRole("textbox", { name: "Name (optional)" })).toBeInTheDocument();
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Application" })).toHaveTextContent("Billing"));
    expect(screen.getByRole("combobox", { name: "Group" })).toHaveTextContent("Edge");
    await choose("Application", "Search");
    fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));
    await vi.waitFor(() => expect(api.updateDataflow).toHaveBeenCalledOnce());
    expect(api.updateDataflow.mock.calls[0]![1]).toMatchObject({ application_id: "app-2", group_id: "grp-1" });
  });

  it("keeps the create shell and guards dirty cancellation", async () => {
    mount("/inventory/dataflows/new");
    expectShell();
    expect(api.getDataflow).not.toHaveBeenCalled();
    fireEvent.change(screen.getByRole("textbox", { name: "Name (optional)" }), { target: { value: "Unsaved" } });
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await screen.findByRole("alertdialog");
    fireEvent.click(screen.getByRole("button", { name: /Continue editing/i }));
    expect(screen.getByRole("textbox", { name: "Name (optional)" })).toHaveValue("Unsaved");
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    fireEvent.click(await screen.findByRole("button", { name: /^Discard/i }));
    expect(await screen.findByText("dataflow-list")).toBeInTheDocument();
  });

  it("reports a safe save error", async () => {
    api.getDataflow.mockResolvedValue(baseFlow());
    api.updateDataflow.mockRejectedValueOnce(new Error("private backend detail"));
    mount();
    await screen.findByRole("textbox", { name: "Name (optional)" });
    fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(/Could not save the dataflow/);
    expect(screen.queryByText("private backend detail")).not.toBeInTheDocument();
  });
});
