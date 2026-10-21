import { act, cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "@/shared/i18n";
import { PermissionsContext } from "@/shared/auth/permissionsContext";
import type { ProxyNode } from "@/shared/api/types";
import type * as ProxiesApi from "./proxies-api";
import type * as InventoryApi from "@/features/inventory/api";
import ProxyNodes from "./ProxyNodes";
import ProxyNodeDetail from "./ProxyNodeDetail";
import ProxyNodeEditor from "./ProxyNodeEditor";
import ProxyAssignmentEditor from "./ProxyAssignmentEditor";

const api = vi.hoisted(() => ({
  setPageSize: vi.fn(),
  listProxyNodes: vi.fn(),
  listProxyNodeAssignments: vi.fn(),
  updateProxyNode: vi.fn(),
  createProxyNode: vi.fn(),
  updateProxyNodeAssignment: vi.fn(),
  createProxyNodeAssignment: vi.fn(),
  deleteProxyNode: vi.fn(),
  deleteProxyNodeAssignment: vi.fn(),
  rotateProxyNodeSecret: vi.fn(),
}));
vi.mock("@/shared/hooks/useTablePageSize", async () => {
  const {useState} = await import("react");
  return {useTablePageSize: () => {
    const [value, setValue] = useState(25);
    return {value, setValue: (size: number) => { api.setPageSize(size); setValue(size); }};
  }};
});

vi.mock("./proxies-api", async (original) => ({
  ...(await original<typeof ProxiesApi>()),
  listProxyNodes: api.listProxyNodes,
  listProxyNodeAssignments: api.listProxyNodeAssignments,
  updateProxyNode: api.updateProxyNode,
  createProxyNode: api.createProxyNode,
  updateProxyNodeAssignment: api.updateProxyNodeAssignment,
  createProxyNodeAssignment: api.createProxyNodeAssignment,
  deleteProxyNode: api.deleteProxyNode,
  deleteProxyNodeAssignment: api.deleteProxyNodeAssignment,
  rotateProxyNodeSecret: api.rotateProxyNodeSecret,
}));

vi.mock("@/features/inventory/api", async (original) => ({
  ...(await original<typeof InventoryApi>()),
  listResources: vi.fn().mockResolvedValue({ items: [], total: 0, limit: 25, offset: 0 }),
}));

const node = {
  id: "p1",
  name: "edge-proxy",
  slug: "edge",
  base_url: "https://proxy.example.test",
  enabled: true,
  secret_version: 1,
  secret_rotated_at: null,
} as unknown as ProxyNode;

function Location() {
  const location = useLocation();
  return (
    <output data-testid="location">
      {location.pathname}
      {location.search}
    </output>
  );
}

function renderPages(path: string, readOnly = false) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <PermissionsContext.Provider value={{ roles: [], isAdmin: !readOnly, can: action => !readOnly || action === "READ", canAny: () => !readOnly, hasRole: () => false }}>
        <MemoryRouter initialEntries={[path]}>
          <Routes>
            <Route path="/connections/proxies" element={<ProxyNodes />} />
            <Route path="/connections/proxies/new" element={<ProxyNodeEditor />} />
            <Route path="/connections/proxies/:id/edit" element={<ProxyNodeEditor />} />
            <Route path="/connections/proxies/assignments/new" element={<ProxyAssignmentEditor />} />
            <Route path="/connections/proxies/assignments/:id/edit" element={<ProxyAssignmentEditor />} />
            <Route path="/connections/proxies/:id" element={<ProxyNodeDetail />} />
          </Routes>
          <Location />
        </MemoryRouter>
      </PermissionsContext.Provider>
    </QueryClientProvider>,
  );
}

afterEach(() => {
  // Drain Radix focus restoration before the test document is replaced.
  vi.useFakeTimers(); cleanup(); vi.runOnlyPendingTimers(); vi.useRealTimers();
});

describe("proxy pages", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    api.listProxyNodes.mockResolvedValue({ items: [node], total: 1, limit: 25, offset: 0 });
    api.listProxyNodeAssignments.mockResolvedValue({ items: [], total: 0, limit: 10, offset: 0 });
    api.deleteProxyNode.mockResolvedValue(undefined);
    api.deleteProxyNodeAssignment.mockResolvedValue(undefined);
  });

  it("canonical proxy creation actions live in the header and menu navigation does not open a node", async () => {
    renderPages("/connections/proxies");
    const header = within(screen.getByRole("banner"));
    expect(header.getByRole("link", { name: "Add Node" })).toHaveAttribute("href", "/connections/proxies/new");
    expect(header.getByRole("link", { name: "Add assignment" })).toHaveAttribute("href", "/connections/proxies/assignments/new");
    const trigger = await screen.findByRole("button", { name: "Actions: edge-proxy" });
    trigger.focus();
    fireEvent.keyDown(trigger, { key: "ArrowDown" });
    const rotate = await screen.findByRole("menuitem", { name: "Rotate secret" });
    await waitFor(() => expect(rotate).toHaveFocus());
    fireEvent.keyDown(rotate, { key: "ArrowDown" });
    const edit = screen.getByRole("menuitem", { name: "Edit" });
    await waitFor(() => expect(edit).toHaveFocus());
    expect(edit).toHaveAttribute("href", "/connections/proxies/p1/edit");
    fireEvent.keyDown(edit, { key: "Escape" });
    await waitFor(() => expect(trigger).toHaveFocus());
    expect(screen.getByTestId("location")).toHaveTextContent(/^\/connections\/proxies$/);
    expect(api.rotateProxyNodeSecret).not.toHaveBeenCalled();
    expect(api.deleteProxyNode).not.toHaveBeenCalled();
  });

  it("keeps node and assignment pagination, mapped sorts and CSV filters independent", async () => {
    api.listProxyNodes.mockResolvedValue({items: [node], total: 100, limit: 25, offset: 0});
    api.listProxyNodeAssignments.mockResolvedValue({items: [{id: "a1", proxy_node_id: "p1", proxy_node_name: "edge-proxy", scope_type: "global", scope_id: null, weight: 4, enabled: true}], total: 100});
    renderPages("/connections/proxies");
    await screen.findByRole("button", {name: "Actions: edge-proxy"});
    expect(api.listProxyNodes).toHaveBeenCalledWith({limit: 1000, offset: 0});
    expect(api.listProxyNodeAssignments).toHaveBeenLastCalledWith(expect.objectContaining({limit: 10, offset: 0}));
    fireEvent.click(screen.getAllByRole("button", {name: "Next page"})[0]!);
    fireEvent.click(screen.getAllByRole("button", {name: "Next page"})[1]!);
    await waitFor(() => expect(api.listProxyNodes).toHaveBeenLastCalledWith(expect.objectContaining({limit: 25, offset: 25})));
    expect(api.listProxyNodeAssignments).toHaveBeenLastCalledWith(expect.objectContaining({limit: 10, offset: 10}));
    const assignmentsCount = api.listProxyNodeAssignments.mock.calls.length;
    fireEvent.click(document.querySelector('[data-column-sort-key="baseUrl"]')!);
    await waitFor(() => expect(api.listProxyNodes).toHaveBeenLastCalledWith(expect.objectContaining({sort: "base_url", offset: 0})));
    expect(api.listProxyNodeAssignments).toHaveBeenCalledTimes(assignmentsCount);
    fireEvent.click(document.querySelector('[data-column-sort-key="proxy"]')!);
    await waitFor(() => expect(api.listProxyNodeAssignments).toHaveBeenLastCalledWith(expect.objectContaining({sort: "proxy_node_id", offset: 0, limit: 10})));
    fireEvent.click(document.querySelector('[data-column-filter-key="scope_type"]')!);
    fireEvent.click(await screen.findByRole("button", {name: "Global"}));
    await waitFor(() => expect(api.listProxyNodeAssignments).toHaveBeenLastCalledWith(expect.objectContaining({scope_type: "global", offset: 0})));
    expect(api.listProxyNodes.mock.lastCall?.[0]).not.toHaveProperty("scope_type");
    fireEvent.change(screen.getAllByRole("combobox", {name: "Rows per page"})[0]!, {target: {value: "50"}});
    expect(api.setPageSize).toHaveBeenCalledWith(50);
    fireEvent.change(screen.getAllByRole("combobox", {name: "Rows per page"})[1]!, {target: {value: "50"}});
    await waitFor(() => expect(api.listProxyNodeAssignments).toHaveBeenLastCalledWith(expect.objectContaining({limit: 50, offset: 0})));
    expect(api.setPageSize).toHaveBeenCalledTimes(1);
  });

  it("debounces only node search for 250ms without changing assignments or the options query", async () => {
    renderPages("/connections/proxies");
    await screen.findByRole("button", {name: "Actions: edge-proxy"});
    vi.useFakeTimers();
    const count = api.listProxyNodes.mock.calls.length;
    const assignmentsCount = api.listProxyNodeAssignments.mock.calls.length;
    fireEvent.change(screen.getByRole("textbox", {name: "Search nodes..."}), {target: {value: "edge"}});
    await act(async () => { await vi.advanceTimersByTimeAsync(249); });
    expect(api.listProxyNodes).toHaveBeenCalledTimes(count);
    await act(async () => { await vi.advanceTimersByTimeAsync(1); });
    expect(api.listProxyNodes).toHaveBeenLastCalledWith(expect.objectContaining({search: "edge", offset: 0, limit: 25}));
    expect(api.listProxyNodeAssignments).toHaveBeenCalledTimes(assignmentsCount);
    expect(api.listProxyNodes.mock.calls.filter(([params]) => params.limit === 1000)).toHaveLength(1);
    vi.useRealTimers();
  });

  it.each(["node", "assignment"])("canonical %s delete keeps confirmation, cancellation focus and mutation identity", async kind => {
    api.listProxyNodeAssignments.mockResolvedValue({ items: [{ id: "a1", proxy_node_id: "p1", proxy_node_name: "edge-proxy", scope_type: "global", scope_id: null, weight: 4, enabled: true }], total: 1 });
    renderPages("/connections/proxies");
    const trigger = await screen.findByRole("button", { name: kind === "node" ? "Actions: edge-proxy" : "Actions: edge-proxy · Applies to all" });
    fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false });
    const edit = await screen.findByRole("menuitem", { name: "Edit" });
    expect(edit).toHaveAttribute("href", kind === "node" ? "/connections/proxies/p1/edit" : "/connections/proxies/assignments/a1/edit");
    fireEvent.click(screen.getByRole("menuitem", { name: "Delete" }));
    const remove = kind === "node" ? api.deleteProxyNode : api.deleteProxyNodeAssignment;
    expect(remove).not.toHaveBeenCalled();
    fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(trigger).toHaveFocus());
    expect(remove).not.toHaveBeenCalled();
    fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false });
    fireEvent.click(await screen.findByRole("menuitem", { name: "Delete" }));
    fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(remove).toHaveBeenCalledWith(kind === "node" ? "p1" : "a1"));
  });

  it("canonical proxy detail actions cancel back to the invoker", async () => {
    renderPages("/connections/proxies/p1");
    const trigger = await screen.findByRole("button", { name: "Actions: edge-proxy" });
    expect(within(screen.getByRole("banner")).getByRole("link", { name: "Back to proxy nodes" })).toHaveAttribute("href", "/connections/proxies");
    fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false });
    expect((await screen.findByRole("menuitem", { name: "Edit" }))).toHaveAttribute("href", "/connections/proxies/p1/edit");
    fireEvent.click(screen.getByRole("menuitem", { name: "Delete" }));
    fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(trigger).toHaveFocus());
    expect(api.deleteProxyNode).not.toHaveBeenCalled();
  });

  it("canonical secret rotation targets its node and returns from bootstrap to the menu invoker", async () => {
    api.rotateProxyNodeSecret.mockResolvedValue({ node, bootstrap: { node_id: "p1", node_secret: "", env: {}, docker_run_command: "" } });
    renderPages("/connections/proxies");
    const trigger = await screen.findByRole("button", { name: "Actions: edge-proxy" });
    fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false });
    fireEvent.click(await screen.findByRole("menuitem", { name: "Rotate secret" }));
    const dialog = await screen.findByRole("dialog");
    expect(api.rotateProxyNodeSecret).toHaveBeenCalledWith("p1");
    fireEvent.click(within(dialog).getByRole("button", { name: "Done" }));
    await waitFor(() => expect(trigger).toHaveFocus());
    expect(screen.getByTestId("location")).toHaveTextContent(/^\/connections\/proxies$/);
  });

  it.each(["node", "assignment"])("canonical %s edit activates its routed editor from the keyboard menu", async kind => {
    api.listProxyNodeAssignments.mockResolvedValue({ items: [{ id: "a1", proxy_node_id: "p1", proxy_node_name: "edge-proxy", scope_type: "global", scope_id: null, weight: 4, enabled: true }], total: 1 });
    renderPages("/connections/proxies");
    const trigger = await screen.findByRole("button", { name: kind === "node" ? "Actions: edge-proxy" : "Actions: edge-proxy · Applies to all" });
    trigger.focus(); fireEvent.keyDown(trigger, { key: "ArrowDown" });
    const edit = await screen.findByRole("menuitem", { name: "Edit" });
    edit.focus(); fireEvent.keyDown(edit, { key: "Enter" });
    await waitFor(() => expect(screen.getByTestId("location")).toHaveTextContent(kind === "node" ? "/connections/proxies/p1/edit" : "/connections/proxies/assignments/a1/edit"));
    if (kind === "node") expect(await screen.findByRole("textbox", { name: /^Name/ })).toHaveValue("edge-proxy");
    else expect(await screen.findByRole("spinbutton", { name: "Weight" })).toHaveValue(4);
  });

  it("canonical read-only proxy pages have no mutation trigger and retain focusable back navigation", async () => {
    renderPages("/connections/proxies/p1", true);
    await screen.findByRole("heading", { name: "edge-proxy" });
    expect(screen.queryByRole("button", { name: /^Actions:/ })).not.toBeInTheDocument();
    const back = within(screen.getByRole("banner")).getByRole("link", { name: "Back to proxy nodes" });
    back.focus(); expect(back).toHaveFocus(); fireEvent.click(back);
    await screen.findByRole("link", { name: "edge-proxy" });
    expect(screen.queryByRole("button", { name: /^Actions:/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Add Node" })).not.toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Add assignment" })).not.toBeInTheDocument();
  });

  it.each([
    ["detail", "loading"], ["detail", "error"], ["detail", "missing"],
    ["edit", "loading"], ["edit", "error"], ["edit", "missing"],
    ["create", "loading"], ["create", "error"],
  ])("keeps the node %s page heading and exit while %s", async (mode, state) => {
    let resolve!: (value: unknown) => void;
    const deferred = new Promise(value => { resolve = value; });
    if (state === "loading") api.listProxyNodes.mockReturnValue(deferred);
    else if (state === "error") api.listProxyNodes.mockRejectedValue(new Error("fixture unavailable"));
    else api.listProxyNodes.mockResolvedValue({ items: [], total: 0, limit: 25, offset: 0 });
    renderPages(mode === "create" ? "/connections/proxies/new" : `/connections/proxies/missing${mode === "edit" ? "/edit" : ""}`);
    if (state === "loading") await waitFor(() => expect(api.listProxyNodes).toHaveBeenCalled());
    else await screen.findByText(i18n.t(state === "error" ? "proxyNodes.loadFailed" : "proxyNodes.notFound"));
    const title = i18n.t(mode === "detail" ? "proxyNodes.title" : mode === "edit" ? "proxyNodes.editNode" : "proxyNodes.addNodeTitle");
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(screen.getByRole("heading", { level: 1, name: title })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^(Save|Save changes|Add Node)$/i })).not.toBeInTheDocument();
    if (state === "error") expect(screen.getByRole("button", { name: i18n.t("retry") })).toBeEnabled();
    if (mode === "detail") fireEvent.click(screen.getByRole("link", { name: i18n.t("proxyNodes.backToList") }));
    else {
      const back = screen.getByRole("button", { name: i18n.t(mode === "edit" ? "proxyNodes.backToNodes" : "proxyNodes.backToList") });
      expect(back).toBeEnabled();
      fireEvent.click(state === "error" ? back : screen.getByRole("button", { name: i18n.t("cancel") }));
    }
    await waitFor(() => expect(screen.getByTestId("location")).toHaveTextContent(/^\/connections\/proxies$/));
    await act(async () => { resolve({ items: [], total: 0, limit: 25, offset: 0 }); await deferred; });
  });

  it.each(["empty", "missing", "error"])("keeps an assignment page heading and exit in the %s state", async state => {
    if (state === "empty") api.listProxyNodes.mockResolvedValue({ items: [], total: 0, limit: 25, offset: 0 });
    if (state === "error") api.listProxyNodes.mockRejectedValue(new Error("fixture unavailable"));
    renderPages(state === "missing" ? "/connections/proxies/assignments/missing/edit" : "/connections/proxies/assignments/new");
    const message = state === "empty" ? "Add a proxy node before creating routing assignments"
      : state === "missing" ? "Proxy node not found" : "Failed to load proxy nodes";
    await screen.findByText(message);
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    fireEvent.click(screen.getByRole("button", { name: /^Cancel$/ }));
    await waitFor(() => expect(screen.getByTestId("location")).toHaveTextContent(/^\/connections\/proxies$/));
  });

  it("opens the node detail page from the list and exposes the routed create entry", async () => {
    renderPages("/connections/proxies");

    expect(screen.getByRole("link", { name: "Add Node" })).toHaveAttribute(
      "href",
      "/connections/proxies/new",
    );

    fireEvent.click(await screen.findByRole("link", { name: "edge-proxy" }));

    expect(await screen.findByRole("heading", { name: "edge-proxy" })).toBeInTheDocument();
    expect(screen.getByTestId("location")).toHaveTextContent("/connections/proxies/p1");
  });

  it("pages, sorts and filters the node and assignment catalogues independently", async () => {
    api.listProxyNodes.mockResolvedValue({items: [node], total: 100, limit: 25, offset: 0});
    api.listProxyNodeAssignments.mockResolvedValue({items: [{id: "a1", proxy_node_id: "p1", scope_type: "global", scope_id: null, weight: 4, enabled: true}], total: 100, limit: 10, offset: 0});
    renderPages("/connections/proxies");
    const nodeLink = await screen.findByRole("link", {name: "edge-proxy"});
    const nodes = within(nodeLink.closest('[data-table-id="connections.proxies"]') as HTMLElement);
    const assignments = within(screen.getByRole("heading", {name: "Routing assignments"}).closest('[data-table-id="connections.proxyAssignments"]') as HTMLElement);
    fireEvent.change(nodes.getByRole("combobox", {name: "Rows per page"}), {target: {value: "50"}});
    fireEvent.click(nodes.getByRole("button", {name: "Next page"}));
    await waitFor(() => expect(api.listProxyNodes).toHaveBeenCalledWith(expect.objectContaining({offset: 50})));
    expect(api.listProxyNodeAssignments).toHaveBeenLastCalledWith(expect.objectContaining({offset: 0}));
    fireEvent.click(assignments.getByRole("button", {name: "Next page"}));
    await waitFor(() => expect(api.listProxyNodeAssignments).toHaveBeenLastCalledWith(expect.objectContaining({offset: 10})));
    fireEvent.click(nodes.getByRole("button", {name: "Name"}));
    await waitFor(() => expect(api.listProxyNodes).toHaveBeenLastCalledWith(expect.objectContaining({sort: "name", order: "asc", offset: 0})));
    expect(api.listProxyNodeAssignments).toHaveBeenLastCalledWith(expect.objectContaining({offset: 10}));
    fireEvent.click(assignments.getByRole("button", {name: "Weight"}));
    await waitFor(() => expect(api.listProxyNodeAssignments).toHaveBeenLastCalledWith(expect.objectContaining({sort: "weight", order: "asc", offset: 0})));
    fireEvent.click(nodes.getByRole("button", {name: "Filter: Status"}));
    fireEvent.click(await screen.findByRole("button", {name: "Enabled"}));
    await waitFor(() => expect(api.listProxyNodes).toHaveBeenLastCalledWith(expect.objectContaining({enabled: "true"})));
    expect(api.listProxyNodeAssignments).toHaveBeenLastCalledWith(expect.objectContaining({enabled: undefined, sort: "weight"}));
    fireEvent.click(nodes.getByRole("button", {name: "Filter: Status"}));
    fireEvent.click(assignments.getByRole("button", {name: "Filter: Scope"}));
    fireEvent.click(await screen.findByRole("button", {name: "Global"}));
    await waitFor(() => expect(api.listProxyNodeAssignments).toHaveBeenLastCalledWith(expect.objectContaining({scope_type: "global", sort: "weight"})));
    expect(api.listProxyNodes).toHaveBeenLastCalledWith(expect.objectContaining({enabled: "true", sort: "name"}));
  });

  it("locks node fields during a deferred update, preserves failure draft and guards cancel", async () => {
    let rejectSave!: (error: Error) => void;
    api.updateProxyNode.mockImplementation(() => new Promise((_resolve, reject) => { rejectSave = reject; }));
    renderPages("/connections/proxies/p1/edit");
    const name = await screen.findByRole("textbox", {name: /^Name/});
    fireEvent.change(name, {target: {value: "Edited proxy"}});
    fireEvent.click(screen.getByRole("button", {name: "Save Changes"}));
    await waitFor(() => expect(api.updateProxyNode).toHaveBeenCalledWith("p1", {
      name: "Edited proxy", slug: "edge", base_url: node.base_url, enabled: true,
    }));
    for (const input of screen.getAllByRole("textbox")) expect(input).toBeDisabled();
    expect(screen.getByRole("switch", {name: "Enabled"})).toBeDisabled();
    rejectSave(new Error("save failed"));
    await waitFor(() => expect(name).not.toBeDisabled());
    expect(name).toHaveValue("Edited proxy");
    fireEvent.click(screen.getByRole("button", {name: "Cancel"}));
    fireEvent.click(within(screen.getByRole("alertdialog")).getByRole("button", {name: "Continue editing"}));
    expect(name).toHaveValue("Edited proxy");
    fireEvent.click(screen.getByRole("button", {name: "Cancel"}));
    fireEvent.click(within(screen.getByRole("alertdialog")).getByRole("button", {name: "Discard"}));
    await waitFor(() => expect(screen.getByTestId("location")).toHaveTextContent("/connections/proxies"));
  });

  it("locks every assignment surface while saving and keeps scope/weight after failure", async () => {
    api.listProxyNodeAssignments.mockResolvedValue({items: [{id: "a1", proxy_node_id: "p1", scope_type: "global", scope_id: null, weight: 4, enabled: true}], total: 1});
    let rejectSave!: (error: Error) => void;
    api.updateProxyNodeAssignment.mockImplementation(() => new Promise((_resolve, reject) => { rejectSave = reject; }));
    renderPages("/connections/proxies/assignments/a1/edit");
    const weight = await screen.findByRole("spinbutton", {name: "Weight"});
    fireEvent.change(weight, {target: {value: "12"}});
    fireEvent.click(screen.getByRole("button", {name: "Save Changes"}));
    await waitFor(() => expect(api.updateProxyNodeAssignment).toHaveBeenCalledWith("a1", {
      proxy_node_id: "p1", scope_type: "global", scope_id: null, weight: 12, enabled: true,
    }));
    expect(weight).toBeDisabled();
    for (const picker of screen.getAllByRole("combobox")) expect(picker).toBeDisabled();
    expect(screen.getByRole("switch", {name: "Enabled"})).toBeDisabled();
    rejectSave(new Error("save failed"));
    await waitFor(() => expect(weight).not.toBeDisabled());
    expect(weight).toHaveValue(12);
  });
});
