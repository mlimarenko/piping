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

const apiMocks = vi.hoisted(() => ({
  getResource: vi.fn(),
  createResource: vi.fn(),
  updateResource: vi.fn(),
  listAllResources: vi.fn(),
}));

vi.mock("@/features/inventory/api", async (original) => ({
  ...(await original()),
  getResource: apiMocks.getResource,
  getResourceWithRaw: async () => ({ resource: await apiMocks.getResource(), raw: {} }),
  createResource: apiMocks.createResource,
  updateResource: apiMocks.updateResource,
  listAllResources: apiMocks.listAllResources,
}));

const RETURN = "/inventory/resources?entity=Sites";

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
  });

  it("owns one form-width shell while loading and after the resource arrives", async () => {
    let finish!: (value: unknown) => void;
    apiMocks.getResource.mockReturnValue(new Promise(resolve => { finish = resolve; }));
    renderEditor("/inventory/resources/Sites/site-1/edit");
    expect(screen.getByRole("status")).toHaveTextContent("Loading");
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(screen.getAllByTestId("entity-editor-page")).toHaveLength(1);
    expect(screen.getByTestId("entity-editor-page")).toHaveClass("max-w-[960px]");
    finish({ id: "site-1", name: "Site A", type: "site" });
    await screen.findByRole("heading", { level: 1, name: "Site A" });
    expect(screen.getAllByTestId("entity-editor-page")).toHaveLength(1);
    expect(screen.getByTestId("entity-editor-page")).toHaveClass("max-w-[960px]");
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

  it("edits service address bindings with real named multi-select options and clears them explicitly", async () => {
    apiMocks.getResource.mockResolvedValue({ id: "service-1", name: "HTTPS", type: "service", serviceIpAddressIds: ["ip-1"], parentObject: { objectType: "dcim.device", objectId: "device-1", label: "Web" } });
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
    fireEvent.click(screen.getByRole("button", { name: "Create" }));

    await waitFor(() =>
      expect(apiMocks.createResource).toHaveBeenCalledWith(
        "Sites",
        expect.objectContaining({ name: "Site A" }),
      ),
    );
    expect(await screen.findByText("list-probe")).toBeInTheDocument();
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

  it.each(["Tags", "Module Type Profiles"] as const)(
    "withholds writes for the read-only entity %s on the routed edit route",
    async (entity) => {
      apiMocks.getResource.mockResolvedValue({ id: "ro-1", name: "Read only", type: entity });

      renderEditor(
        `/inventory/resources/${encodeURIComponent(entity)}/ro-1/edit?return=${encodeURIComponent(RETURN)}`,
      );

      expect(await screen.findByRole("heading", { level: 1, name: "Read only" })).toBeInTheDocument();
      expect(screen.queryByRole("textbox")).toBeNull();
      expect(screen.queryByRole("button", { name: "Save Changes" })).not.toBeInTheDocument();
      expect(apiMocks.updateResource).not.toHaveBeenCalled();
    },
  );

  it("withholds create for a read-only entity on the routed new route", async () => {
    renderEditor(`/inventory/resources/Tags/new?return=${encodeURIComponent(RETURN)}`);

    expect(await screen.findByRole("heading", { level: 1 })).toHaveTextContent(/create/i);
    expect(screen.queryByRole("button", { name: "Create" })).not.toBeInTheDocument();
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
