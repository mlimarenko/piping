import { screen, waitFor, fireEvent, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider, useQuery } from "@tanstack/react-query";
import { CONNECTION_TARGETS_QUERY_KEY } from "@/features/connections/api";
import { MemoryRouter, Route, Routes } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import { AllPermissions } from "@/test/permissions";
import EntityResourceEditorPage from "./EntityResourceEditorPage";
import EntityFormBody from "@/shared/forms/EntityFormBody";
import { ApiError } from "@/shared/api/client";
import { inventoryErrorMessage } from "./inventoryErrors";

const apiMocks = vi.hoisted(() => ({
  getResource: vi.fn(),
  createResource: vi.fn(),
  updateResource: vi.fn(),
  listAllResources: vi.fn(),
  listChildResourcesByRouteQuery: vi.fn(),
  notify: vi.fn(),
}));

vi.mock("@/shared/lib/notifications", () => ({ notify: apiMocks.notify }));

vi.mock("@/features/inventory/api", async (original) => ({
  ...(await original()),
  getResource: apiMocks.getResource,
  getResourceWithRaw: async () => ({ resource: await apiMocks.getResource(), raw: {} }),
  createResource: apiMocks.createResource,
  updateResource: apiMocks.updateResource,
  listAllResources: apiMocks.listAllResources,
  listChildResourcesByRouteQuery: apiMocks.listChildResourcesByRouteQuery,
}));

const RETURN = "/inventory/resources?entity=Sites";

vi.mock("@/features/inventory/custom-fields-api", async (original) => ({
  ...(await original()),
  listCustomFields: vi.fn(() => Promise.resolve({ items: [], total: 0, limit: 100, offset: 0 })),
  listContentTypes: vi.fn(() => Promise.resolve(["dcim.site", "dcim.device", "virtualization.virtualmachine", "dcim.moduletypeprofile"].map((token, index) => ({
    id: `scope-${index}`, token, app_label: token.split(".")[0], model: token.split(".")[1], label: token, supported_for_custom_fields: true,
  })))),
}));

function renderEditor(initialEntry: string) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <AllPermissions>
        <MemoryRouter initialEntries={[initialEntry]}>
          <Routes>
            <Route path="/inventory/resources/:entity/new" element={<EntityResourceEditorPage />} />
            <Route path="/inventory/resources/:entity/:id/edit" element={<EntityResourceEditorPage />} />
            <Route path="/inventory/resources" element={<div>list-probe</div>} />
          </Routes>
        </MemoryRouter>
      </AllPermissions>
    </QueryClientProvider>,
  );
}

describe("EntityResourceEditorPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    apiMocks.listAllResources.mockResolvedValue([]);
    apiMocks.listChildResourcesByRouteQuery.mockResolvedValue({ rows: [], total: 0, limit: 50, offset: 0 });
  });

  it("retains bay edits after a rejected write and maps internal errors to safe localized text", async () => {
    const error = new ApiError({ status: 409, message: "private SQL detail", reasonCode: "MODULE_BAY_OCCUPIED" });
    apiMocks.getResource.mockResolvedValue({ id: "bay", name: "Slot", type: "module_bay", deviceId: "d", enabled: "false", moduleBayTypes: [] });
    apiMocks.updateResource.mockRejectedValue(error);
    renderEditor("/inventory/resources/Module%20Bays/bay/edit");
    await screen.findByRole("heading", { level: 1, name: "Slot" });
    fireEvent.change(screen.getByRole("textbox", { name: /^Name/ }), { target: { value: "Retained slot" } });
    fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));
    await waitFor(() => expect(apiMocks.updateResource).toHaveBeenCalledWith("Module Bays", "bay", { name: "Retained slot" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Save Changes" })).toBeEnabled());
    expect(screen.getByRole("textbox", { name: /^Name/ })).toHaveValue("Retained slot");
    expect(screen.queryByText("private SQL detail")).not.toBeInTheDocument();
    expect(inventoryErrorMessage(key => key, error)).toBe("inventory.resources.saveErrors.moduleBayOccupied");
    expect(inventoryErrorMessage(key => key, new ApiError({ status: 500, message: "private SQL detail" }))).toBe("unknownError");
  });

  it("keeps the parent and failed child drafts after parent PUT success and child POST409, then retries before navigating", async () => {
    const parent = { id: "module", name: "Module hardware", type: "module", deviceId: "device", moduleTypeId: "mt", moduleBayId: "bay", description: "Original" };
    apiMocks.getResource.mockResolvedValue(parent);
    apiMocks.updateResource.mockResolvedValue({ ...parent, description: "Retained parent edit" });
    apiMocks.createResource.mockRejectedValueOnce(new ApiError({ status: 409, message: "private child SQL" }))
      .mockResolvedValueOnce({ id: "child", name: "Retained bay", type: "module_bay", deviceId: "device", moduleId: "module" });
    renderEditor("/inventory/resources/Modules/module/edit");
    await screen.findByRole("heading", { level: 1, name: "Module hardware" });
    fireEvent.change(screen.getByRole("textbox", { name: "Description" }), { target: { value: "Retained parent edit" } });
    fireEvent.click(await screen.findByRole("button", { name: "Add Module bays" }));
    const child = screen.getByRole("listitem");
    fireEvent.click(within(child).getByRole("button", { name: "Expand" }));
    fireEvent.change(within(child).getByRole("textbox", { name: /^Name/ }), { target: { value: "Retained bay" } });
    fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));
    await waitFor(() => expect(apiMocks.createResource).toHaveBeenCalledTimes(1));
    expect(apiMocks.updateResource).toHaveBeenCalledWith("Modules", "module", expect.objectContaining({ description: "Retained parent edit" }));
    await screen.findByRole("button", { name: "Retry" });
    expect(screen.queryByText("list-probe")).not.toBeInTheDocument();
    expect(within(child).getByRole("textbox", { name: /^Name/ })).toHaveValue("Retained bay");
    expect(screen.getAllByRole("textbox", { name: "Description" })[0]).toHaveValue("Retained parent edit");
    expect(screen.queryByText("private child SQL")).not.toBeInTheDocument();
    expect(apiMocks.notify).not.toHaveBeenCalledWith("success", expect.anything());
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));
    await screen.findByText("list-probe");
    expect(apiMocks.createResource).toHaveBeenCalledTimes(2);
    expect(apiMocks.createResource).toHaveBeenLastCalledWith("Module Bays", expect.objectContaining({ name: "Retained bay", deviceId: "device", moduleId: "module" }));
    expect(apiMocks.notify).toHaveBeenCalledWith("success", expect.stringContaining("Module hardware"));
  });

  it("keeps one editor and heading while loading and after the resource arrives", async () => {
    let finish!: (value: unknown) => void;
    apiMocks.getResource.mockReturnValue(new Promise(resolve => { finish = resolve; }));
    renderEditor("/inventory/resources/Sites/site-1/edit");
    expect(screen.getByRole("status")).toHaveTextContent("Loading");
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(screen.getAllByTestId("entity-editor-page")).toHaveLength(1);
    expect(screen.queryByRole("button", { name: "Save Changes" })).not.toBeInTheDocument();
    finish({ id: "site-1", name: "Site A", type: "site" });
    await screen.findByRole("heading", { level: 1, name: "Site A" });
    expect(screen.getAllByTestId("entity-editor-page")).toHaveLength(1);
    expect(screen.getByRole("button", { name: "Save Changes" })).toBeInTheDocument();
  });

  it("Cooling nested template choices inherit the hardware parent before a child is saved", async () => {
    apiMocks.getResource.mockResolvedValue({ id: "mt", name: "Cooling hardware", type: "module_type", manufacturerId: "manufacturer" });
    apiMocks.listAllResources.mockImplementation(entity => Promise.resolve(entity === "Cooling Intake Templates" ? [
      { id: "same", name: "Same hardware inlet", type: "cooling_intake_template", moduleTypeId: "mt" },
      { id: "other", name: "Other hardware inlet", type: "cooling_intake_template", moduleTypeId: "other" },
    ] : []));
    renderEditor("/inventory/resources/Module%20Types/mt/edit");
    fireEvent.click(await screen.findByRole("button", { name: "Add Cooling Outflow Templates" }));
    const child = screen.getByRole("listitem");
    fireEvent.click(within(child).getByRole("button", { name: "Expand" }));
    fireEvent.click(within(child).getByRole("combobox", { name: "Cooling intake" }));
    expect(await screen.findByRole("option", { name: /Same hardware inlet/ })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: /Other hardware inlet/ })).not.toBeInTheDocument();
  });

  it.each(["error", "missing"])("keeps %s in its shell and retries without exposing a write action", async state => {
    if (state === "error") apiMocks.getResource.mockRejectedValueOnce(new Error("private backend detail"));
    else apiMocks.getResource.mockResolvedValueOnce(null);
    apiMocks.getResource.mockResolvedValue({ id: "site-1", name: "Recovered site", type: "site" });
    renderEditor("/inventory/resources/Sites/site-1/edit");
    expect(await screen.findByRole("alert")).toHaveTextContent("Failed to load this resource.");
    expect(screen.queryByText("private backend detail")).not.toBeInTheDocument();
    expect(screen.getAllByTestId("entity-editor-page")).toHaveLength(1);
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(screen.queryByRole("button", { name: "Save Changes" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /retry/i }));
    await screen.findByRole("heading", { level: 1, name: "Recovered site" });
    expect(screen.getAllByTestId("entity-editor-page")).toHaveLength(1);
  });

  it("owns the invalid-entity shell and preserves the explicit return destination", async () => {
    renderEditor(`/inventory/resources/invalid/new?return=${encodeURIComponent(RETURN)}`);
    expect(screen.getByRole("alert")).toBeInTheDocument();
    expect(screen.getAllByTestId("entity-editor-page")).toHaveLength(1);
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(apiMocks.getResource).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(await screen.findByText("list-probe")).toBeInTheDocument();
  });

  it("retains explicit related-field clearing and hides it for disabled or empty values", () => {
    const onField = vi.fn();
    const props = { entity: "Owners" as const, draft: { groupId: "group-1" }, onField,
      loadOptions: () => [{ id: "group-1", name: "Operations" }], onRequestCreate: vi.fn(), visibleKeys: new Set(["groupId"]) };
    const view = render(<AllPermissions><EntityFormBody {...props} /></AllPermissions>);
    fireEvent.click(screen.getByRole("button", { name: "Clear Owner group" }));
    expect(onField).toHaveBeenCalledWith("groupId", "");
    view.rerender(<AllPermissions><EntityFormBody {...props} disabledKeys={new Set(["groupId"])} /></AllPermissions>);
    expect(screen.queryByRole("button", { name: "Clear Owner group" })).not.toBeInTheDocument();
    view.rerender(<AllPermissions><EntityFormBody {...props} draft={{ groupId: "" }} /></AllPermissions>);
    expect(screen.queryByRole("button", { name: "Clear Owner group" })).not.toBeInTheDocument();
  });

  it("edits a wireless LAN, clears its group, and sends only changed fields", async () => {
    apiMocks.getResource.mockResolvedValue({ id: "lan", name: "Campus", ssid: "Campus", type: "wireless_lan", status: "active", groupId: "group", authPsk: "", description: "Preserve me" });
    apiMocks.listAllResources.mockImplementation((entity: string) => Promise.resolve(entity === "Wireless LAN Groups" ? [{ id: "group", name: "Campus group", type: "wireless_lan_group" }] : []));
    apiMocks.updateResource.mockResolvedValue({ id: "lan", name: "Staff", ssid: "Staff", type: "wireless_lan" });
    renderEditor("/inventory/resources/Wireless%20LANs/lan/edit");
    await screen.findByRole("heading", { level: 1, name: "Campus" });
    expect(screen.queryByRole("textbox", { name: /^Name/ })).not.toBeInTheDocument();
    expect(screen.getByLabelText("Pre-shared key")).toHaveAttribute("type", "password");
    fireEvent.change(screen.getByRole("textbox", { name: /^SSID/ }), { target: { value: "Staff" } });
    fireEvent.click(screen.getByRole("button", { name: "Clear Wireless LAN group" }));
    fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));
    await waitFor(() => expect(apiMocks.updateResource).toHaveBeenCalledWith("Wireless LANs", "lan", { ssid: "Staff", groupId: null }));
  });

  it("blocks invalid wireless link distance before calling the API", async () => {
    apiMocks.getResource.mockResolvedValue({ id: "link", name: "Bridge", ssid: "Bridge", type: "wireless_link", status: "connected", interfaceAId: "a", interfaceBId: "b", distance: "10", distanceUnit: "m" });
    renderEditor("/inventory/resources/Wireless%20Links/link/edit");
    await screen.findByRole("heading", { level: 1, name: "Bridge" });
    fireEvent.change(screen.getByRole("spinbutton", { name: "Distance" }), { target: { value: "-1" } });
    fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));
    expect(await screen.findByText("Enter a distance between 0 and 999999.99.")).toBeInTheDocument();
    expect(apiMocks.updateResource).not.toHaveBeenCalled();
  });

  it("edits service address bindings with real named multi-select options and clears them explicitly", async () => {
    apiMocks.getResource.mockResolvedValue({ id: "service-1", name: "HTTPS", type: "service", portMappings: ["tcp/443"], serviceIpAddressIds: ["ip-1"], parentObject: { objectType: "dcim.device", objectId: "device-1", label: "Web" } });
    apiMocks.listAllResources.mockImplementation((entity: string) => Promise.resolve(entity === "IP Addresses" ? [{ id: "ip-1", name: "192.0.2.10/32", type: "ip address" }] : []));
    apiMocks.updateResource.mockResolvedValue({ id: "service-1", name: "HTTPS", type: "service", serviceIpAddressIds: [] });
    renderEditor("/inventory/resources/Services/service-1/edit");
    expect(await screen.findByRole("heading", { level: 1, name: "HTTPS" })).toBeInTheDocument();
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    const remove = await screen.findByRole("button", { name: "Remove 192.0.2.10/32" });
    expect(screen.getByText(/Optional explicit service bindings/)).toBeInTheDocument();
    fireEvent.click(remove);
    fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));
    await waitFor(() => expect(apiMocks.updateResource).toHaveBeenCalledWith("Services", "service-1", expect.objectContaining({ serviceIpAddressIds: [] })));
  });

  it("creates a new entity and returns to the originating list URL", async () => {
    apiMocks.createResource.mockResolvedValue({ id: "new", name: "Site A", type: "sites" });

    renderEditor(`/inventory/resources/Sites/new?return=${encodeURIComponent(RETURN)}`);

    expect(await screen.findByRole("heading", { level: 1 })).toHaveTextContent(/create/i);
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    fireEvent.change(screen.getByRole("textbox", { name: "Name" }), { target: { value: "Site A" } });
    await screen.findByText("No custom fields apply to this entity type.");
    fireEvent.click(screen.getByRole("button", { name: "Create" }));

    await waitFor(() =>
      expect(apiMocks.createResource).toHaveBeenCalledWith(
        "Sites",
        expect.objectContaining({ name: "Site A" }),
      ),
    );
    expect(await screen.findByText("list-probe")).toBeInTheDocument();
  });

  it("validates template mappings in the shared form and submits a canonical array", async () => {
    apiMocks.createResource.mockResolvedValue({ id: "template-1", name: "Mixed", type: "service_template", portMappings: ["tcp/80", "udp/53", "sctp/123"] });
    renderEditor("/inventory/resources/Service%20Templates/new");
    const input = await screen.findByRole("textbox", { name: "Port mappings" });
    expect(input).toHaveAttribute("placeholder", "tcp/80, udp/53, sctp/123");
    expect(screen.queryByRole("textbox", { name: "Protocol" })).not.toBeInTheDocument();
    expect(screen.queryByRole("textbox", { name: "Ports" })).not.toBeInTheDocument();
    fireEvent.change(screen.getByRole("textbox", { name: "Name" }), { target: { value: "Mixed" } });
    fireEvent.change(input, { target: { value: "TCP/080, tcp/80" } });
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    expect(await screen.findByText(/Enter one or more distinct protocol\/port mappings/)).toBeInTheDocument();
    expect(apiMocks.createResource).not.toHaveBeenCalled();
    fireEvent.change(input, { target: { value: "TCP/080, udp/53, SCTP/0123" } });
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(apiMocks.createResource).toHaveBeenCalledWith("Service Templates", expect.objectContaining({ name: "Mixed", portMappings: ["tcp/80", "udp/53", "sctp/123"] })));
  });

  it.each(["Devices", "Virtual Machines"])("refreshes the real target observer after creating %s and returning", async entity => {
    apiMocks.listAllResources.mockImplementation((kind: string) => Promise.resolve(kind === "Sites" ? [{ id: "site-fixture", name: "Fixture site" }] : []));
    let targets: string[] = [];
    const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
    client.setQueryData(CONNECTION_TARGETS_QUERY_KEY, targets);
    apiMocks.createResource.mockRejectedValueOnce(new Error("Rejected create fixture")).mockImplementation(() => {
      targets = ["Created target"];
      return Promise.resolve({ id: "target-1", name: "Created target" });
    });
    function TargetsObserver() {
      const query = useQuery({ queryKey: CONNECTION_TARGETS_QUERY_KEY, queryFn: () => Promise.resolve(targets) });
      return <div>{query.data?.join(",") || "No observer targets"}</div>;
    }
    render(<QueryClientProvider client={client}><AllPermissions>
      <TargetsObserver />
      <MemoryRouter initialEntries={[`/inventory/resources/${encodeURIComponent(entity)}/new?return=%2Fconnections`]}>
        <Routes>
          <Route path="/inventory/resources/:entity/new" element={<EntityResourceEditorPage />} />
          <Route path="/connections" element={<div>connections-return-probe</div>} />
        </Routes>
      </MemoryRouter>
    </AllPermissions></QueryClientProvider>);
    fireEvent.change(await screen.findByRole("textbox", { name: "Name" }), { target: { value: "Created target" } });
    if (entity === "Devices") {
      fireEvent.click(screen.getByRole("combobox", { name: "Site" }));
      fireEvent.click(await screen.findByText("Fixture site"));
    }
    expect(screen.getByRole("button", { name: "Back to connections" })).toBeInTheDocument();
    await screen.findByText("No custom fields apply to this entity type.");
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(apiMocks.createResource).toHaveBeenCalledTimes(1));
    expect(client.getQueryState(CONNECTION_TARGETS_QUERY_KEY)?.isInvalidated).toBe(false);
    expect(screen.getByText("No observer targets")).toBeInTheDocument();
    await waitFor(() => expect(screen.getByRole("button", { name: "Create" })).not.toBeDisabled());
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    expect(await screen.findByText("connections-return-probe")).toBeInTheDocument();
    expect(await screen.findByText("Created target")).toBeInTheDocument();
    expect(client.getQueryData(CONNECTION_TARGETS_QUERY_KEY)).toEqual(["Created target"]);
    client.clear();
  });

  it("loads an entity by id and saves it in place", async () => {
    apiMocks.getResource.mockResolvedValue({ id: "site-1", name: "Old name", type: "sites" });
    apiMocks.updateResource.mockResolvedValue({ id: "site-1", name: "New name", type: "sites" });

    renderEditor(`/inventory/resources/Sites/site-1/edit?return=${encodeURIComponent(RETURN)}`);

    const name = await screen.findByRole("textbox", { name: "Name" });
    expect(name).toHaveValue("Old name");
    expect(apiMocks.getResource).toHaveBeenCalledWith("Sites", "site-1");

    fireEvent.change(name, { target: { value: "New name" } });
    fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));

    await waitFor(() =>
      expect(apiMocks.updateResource).toHaveBeenCalledWith(
        "Sites",
        "site-1",
        expect.objectContaining({ name: "New name" }),
      ),
    );
    expect(await screen.findByText("list-probe")).toBeInTheDocument();
  });

  it.each(["Devices", "Virtual Machines"])("refreshes cached target variants only after a successful %s update", async entity => {
    let targetName = "Old target";
    apiMocks.getResource.mockResolvedValue({ id: "target-1", name: "Old target", siteId: "site-fixture" });
    apiMocks.updateResource.mockRejectedValueOnce(new Error("Rejected fixture mutation"))
      .mockImplementationOnce(() => {
        targetName = "New target";
        return Promise.resolve({ id: "target-1", name: targetName });
      });
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const rootKey = CONNECTION_TARGETS_QUERY_KEY;
    const tableKey = [...rootKey, "table", { search: "target" }];
    client.setQueryData(rootKey, { items: ["Old target"] });
    client.setQueryData(tableKey, { items: ["Old target"] });
    function UpdatedTargetsObserver() {
      const query = useQuery({ queryKey: rootKey, staleTime: Infinity,
        queryFn: () => Promise.resolve({ items: [targetName] }) });
      return <div>Observed target: {query.data?.items.join(",")}</div>;
    }
    render(<QueryClientProvider client={client}><AllPermissions>
      <UpdatedTargetsObserver />
      <MemoryRouter initialEntries={[`/inventory/resources/${encodeURIComponent(entity)}/target-1/edit?return=%2Fconnections`]}>
        <Routes>
          <Route path="/inventory/resources/:entity/:id/edit" element={<EntityResourceEditorPage />} />
          <Route path="/connections" element={<div>connections-update-return</div>} />
        </Routes>
      </MemoryRouter>
    </AllPermissions></QueryClientProvider>);
    fireEvent.change(await screen.findByRole("textbox", { name: "Name" }), { target: { value: "New target" } });
    fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));
    await waitFor(() => expect(apiMocks.updateResource).toHaveBeenCalledTimes(1));
    expect(client.getQueryState(rootKey)?.isInvalidated).toBe(false);
    expect(client.getQueryState(tableKey)?.isInvalidated).toBe(false);
    expect(screen.getByText("Observed target: Old target")).toBeInTheDocument();
    await waitFor(() => expect(screen.getByRole("button", { name: "Save Changes" })).not.toBeDisabled());
    fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));
    expect(await screen.findByText("connections-update-return")).toBeInTheDocument();
    expect(await screen.findByText("Observed target: New target")).toBeInTheDocument();
    expect(client.getQueryState(tableKey)?.isInvalidated).toBe(true);
    client.clear();
  });

  it("warns before discarding a dirty create and returns on confirm", async () => {
    renderEditor(`/inventory/resources/Sites/new?return=${encodeURIComponent(RETURN)}`);

    fireEvent.change(await screen.findByRole("textbox", { name: "Name" }), {
      target: { value: "Half filled" },
    });
    // Back triggers the discard confirmation while dirty.
    fireEvent.click(screen.getByRole("button", { name: /back to list/i }));
    const alert = await screen.findByRole("alertdialog");
    fireEvent.click(within(alert).getByRole("button", { name: /discard/i }));
    expect(await screen.findByText("list-probe")).toBeInTheDocument();
  });

  it("drops the cached edit-detail after save so a reopen re-fetches fresh data", async () => {
    apiMocks.getResource.mockResolvedValue({ id: "site-1", name: "Old name", type: "sites" });
    apiMocks.updateResource.mockResolvedValue({ id: "site-1", name: "New name", type: "sites" });
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const removeSpy = vi.spyOn(client, "removeQueries");
    render(
      <QueryClientProvider client={client}>
        <AllPermissions>
          <MemoryRouter
            initialEntries={[`/inventory/resources/Sites/site-1/edit?return=${encodeURIComponent(RETURN)}`]}
          >
            <Routes>
              <Route path="/inventory/resources/:entity/:id/edit" element={<EntityResourceEditorPage />} />
              <Route path="/inventory/resources" element={<div>list-probe</div>} />
            </Routes>
          </MemoryRouter>
        </AllPermissions>
      </QueryClientProvider>,
    );

    fireEvent.change(await screen.findByRole("textbox", { name: "Name" }), {
      target: { value: "New name" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));

    await waitFor(() => expect(apiMocks.updateResource).toHaveBeenCalled());
    expect(removeSpy).toHaveBeenCalledWith({
      queryKey: ["inventory", "resources", "Sites", "edit-detail", "site-1"],
    });
  });

  it("creates a module profile through the shared routed editor with schema validation", async () => {
    apiMocks.createResource.mockResolvedValue({ id: "p", name: "Fiber", type: "module_type_profile" });
    renderEditor(`/inventory/resources/Module%20Type%20Profiles/new?return=${encodeURIComponent(RETURN)}`);
    fireEvent.change(await screen.findByRole("textbox", { name: "Name" }), { target: { value: "Fiber" } });
    fireEvent.change(screen.getByRole("textbox", { name: "Schema" }), { target: { value: "[]" } });
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    expect(await screen.findByText("Enter a JSON object or null for the profile schema")).toBeInTheDocument();
    expect(apiMocks.createResource).not.toHaveBeenCalled();
    fireEvent.change(screen.getByRole("textbox", { name: "Schema" }), { target: { value: '{"properties":{}}' } });
    await screen.findByText("No custom fields apply to this entity type.");
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(apiMocks.createResource).toHaveBeenCalledWith("Module Type Profiles", expect.objectContaining({ name: "Fiber", schemaSummary: '{"properties":{}}' })));
  });

  it("patches only changed module profile fields and permits explicit schema clearing", async () => {
    apiMocks.getResource.mockResolvedValue({ id: "p", name: "Fiber", type: "module_type_profile", schemaSummary: '{"properties":{}}' });
    apiMocks.updateResource.mockResolvedValue({ id: "p", name: "Fiber", type: "module_type_profile" });
    renderEditor("/inventory/resources/Module%20Type%20Profiles/p/edit");
    fireEvent.change(await screen.findByRole("textbox", { name: "Schema" }), { target: { value: "" } });
    fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));
    await waitFor(() => expect(apiMocks.updateResource).toHaveBeenCalledWith("Module Type Profiles", "p", { schemaSummary: null }));
  });

  it("edits ModuleType attributes without rewriting an omitted profile or other hardware fields", async () => {
    apiMocks.getResource.mockResolvedValue({ id: "mt", name: "Fiber", model: "Fiber", type: "module_type", manufacturerId: "m", profileId: "p", attributeDataSummary: '{"ports":4}' });
    apiMocks.updateResource.mockResolvedValue({ id: "mt", name: "Fiber", type: "module_type" });
    renderEditor("/inventory/resources/Module%20Types/mt/edit");
    fireEvent.change(await screen.findByRole("textbox", { name: "Module attributes (JSON)" }), { target: { value: "{" } });
    fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));
    expect(await screen.findByText("Enter valid JSON for the module attributes")).toBeInTheDocument();
    expect(apiMocks.updateResource).not.toHaveBeenCalled();
    fireEvent.change(screen.getByRole("textbox", { name: "Module attributes (JSON)" }), { target: { value: '{"ports":8}' } });
    fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));
    await waitFor(() => expect(apiMocks.updateResource).toHaveBeenCalledWith("Module Types", "mt", { attributeDataSummary: '{"ports":8}' }));
  });

  it("creates a Tag through the shared routed editor", async () => {
    apiMocks.createResource.mockResolvedValue({ id: "tag-1", name: "Core", type: "tag", slug: "core", color: "#155dfc", weight: "0" });
    renderEditor(`/inventory/resources/Tags/new?return=${encodeURIComponent(RETURN)}`);
    fireEvent.change(await screen.findByRole("textbox", { name: "Name" }), { target: { value: "Core" } });
    fireEvent.change(screen.getByRole("textbox", { name: "Slug" }), { target: { value: "core" } });
    fireEvent.change(screen.getByRole("textbox", { name: "Color" }), { target: { value: "#155dfc" } });
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(apiMocks.createResource).toHaveBeenCalledWith("Tags", expect.objectContaining({ name: "Core", slug: "core", color: "#155dfc", weight: "0", description: "" })));
    expect(await screen.findByText("list-probe")).toBeInTheDocument();
  });

  it("updates a Tag through the shared routed editor, preserving unedited catalog attributes", async () => {
    apiMocks.getResource.mockResolvedValue({ id: "tag-1", name: "Core", type: "tag", slug: "core", color: "155dfc", weight: "0", description: "Network" });
    apiMocks.updateResource.mockResolvedValue({ id: "tag-1", name: "Renamed", type: "tag" });
    renderEditor(`/inventory/resources/Tags/tag-1/edit?return=${encodeURIComponent(RETURN)}`);
    fireEvent.change(await screen.findByRole("textbox", { name: "Name" }), { target: { value: "Renamed" } });
    fireEvent.change(screen.getByRole("textbox", { name: "Description" }), { target: { value: "" } });
    fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));
    await waitFor(() => expect(apiMocks.updateResource).toHaveBeenCalledWith("Tags", "tag-1", { name: "Renamed", description: "" }));
    expect(await screen.findByText("list-probe")).toBeInTheDocument();
  });

  it("does not mark an untouched nested create frame as dirty and pops without a prompt", async () => {
    renderEditor(`/inventory/resources/Sites/new?return=${encodeURIComponent(RETURN)}`);

    expect(await screen.findByRole("heading", { level: 1 })).toHaveTextContent(/create/i);
    // Open an inline child create frame but type nothing into it.
    fireEvent.click(screen.getAllByRole("button", { name: /^Create new /i })[0]!);
    expect(await screen.findByRole("heading", { name: /Create Region/i })).toBeInTheDocument();

    // A blank nested frame is not unsaved work.
    expect(screen.queryByText(/You have unsaved changes/)).not.toBeInTheDocument();

    // Back pops the local frame without a discard prompt.
    fireEvent.click(screen.getByRole("button", { name: "Back" }));
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    expect(await screen.findByRole("heading", { name: /Create Site/i })).toBeInTheDocument();
  });

  it("keeps a dirty root guarded while a blank inline-create frame is active", async () => {
    apiMocks.getResource.mockResolvedValue({ id: "site-1", name: "Old name", type: "sites" });
    renderEditor(`/inventory/resources/Sites/site-1/edit?return=${encodeURIComponent(RETURN)}`);

    fireEvent.change(await screen.findByRole("textbox", { name: "Name" }), {
      target: { value: "New name" },
    });
    fireEvent.click(screen.getAllByRole("button", { name: /^Create new /i })[0]!);
    expect(await screen.findByRole("heading", { name: /Create Region/i })).toBeInTheDocument();

    // The dirty root stays visible and still guards a full page unload.
    expect(screen.getByText(/You have unsaved changes/)).toBeInTheDocument();
    const guard = vi.fn();
    const event = new Event("beforeunload", { cancelable: true });
    event.preventDefault = guard;
    window.dispatchEvent(event);
    expect(guard).toHaveBeenCalled();

    // The breadcrumb leaves the root, so it stays guarded too.
    fireEvent.click(screen.getByRole("button", { name: "Site" }));
    const alert = await screen.findByRole("alertdialog");
    fireEvent.click(within(alert).getByRole("button", { name: /discard/i }));
    expect(await screen.findByText("list-probe")).toBeInTheDocument();
  });
});
