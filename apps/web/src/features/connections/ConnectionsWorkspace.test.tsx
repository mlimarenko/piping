import { act, cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { useState } from "react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, useLocation, useNavigate } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import type * as ConnectionsApiModule from "@/features/connections/api";
import type * as PlaybooksApiModule from "@/features/playbooks/api";
import type * as SettingsModule from "@/shared/settings";
import type * as InventoryApiModule from "@/features/inventory/api";
import type * as TreeConfigModule from "@/features/connections/components/connections/treeConfig";
import type * as SessionViewportModule from "./hooks/useSessionViewport";
import type { ComponentProps } from "react";
import type ConnectionsFilters from "@/features/connections/components/connections/ConnectionsFilters";

import ConnectionsWorkspace, { type LiveSession } from "./ConnectionsWorkspace";
import { CONNECTION_TARGETS_QUERY_KEY } from "./api";
import { TooltipProvider } from "@/shared/ui-primitives/tooltip";

const mocks = vi.hoisted(() => ({
  listConnectionTargets: vi.fn(),
  openSession: vi.fn(),
  closeSession: vi.fn(),
  getConnectionLaunchOptions: vi.fn(),
  listPublicConnectionFilterPresets: vi.fn(),
  getTargetPlaybookAssignments: vi.fn(),
  launchJobTemplate: vi.fn(),
  notify: vi.fn(),
  createPublicConnectionFilterPreset: vi.fn(),
  realFilters: false,
  listPlaybooks: vi.fn(),
  useSetting: vi.fn(),
  workspaceFilters: ((): Record<string, string[]> => ({}))(),
  mobileSession: false,
  mobile: false,
  sessionTabs: [] as LiveSession[],
  viewMode: "tree",
  canInventory: true,
  canReadInventory: true,
  mapSites: vi.fn(),
}));

vi.mock("@/features/inventory/api", async importOriginal => ({ ...await importOriginal<typeof InventoryApiModule>(), listAllResources: mocks.mapSites }));

vi.mock("@/shared/auth/permissionsContext", () => ({
  usePermissions: () => ({ can: (action: string) => action === "READ" ? mocks.canReadInventory : mocks.canInventory }),
}));

vi.mock("@/features/connections/api", async () => {
  const actual = await vi.importActual<typeof ConnectionsApiModule>(
    "@/features/connections/api",
  );
  return { ...actual, listConnectionTargets: mocks.listConnectionTargets, openSession: mocks.openSession, closeSession: mocks.closeSession, getConnectionLaunchOptions: mocks.getConnectionLaunchOptions };
});

vi.mock("@/features/connections/filter-presets-api", () => ({
  listPublicConnectionFilterPresets: mocks.listPublicConnectionFilterPresets,
  createPublicConnectionFilterPreset: mocks.createPublicConnectionFilterPreset,
  deletePublicConnectionFilterPreset: vi.fn(),
}));

vi.mock("@/features/playbooks/api", async () => {
  const actual = await vi.importActual<typeof PlaybooksApiModule>(
    "@/features/playbooks/api",
  );
  return {
    ...actual,
    getTargetPlaybookAssignments: mocks.getTargetPlaybookAssignments,
    launchJobTemplate: mocks.launchJobTemplate,
    getPlaybookRunGroup: vi.fn(),
  };
});

vi.mock("@/shared/lib/notifications", () => ({ notify: mocks.notify }));

vi.mock("@/shared/settings", async () => {
  const actual = await vi.importActual<typeof SettingsModule>("@/shared/settings");
  return {
    ...actual,
    useSetting: mocks.useSetting,
    readSessionSetting: (key: string, fallback: unknown) => key === "connections.workspace.sessionTabs" ? mocks.sessionTabs : fallback,
    readRetiredSessionJson: () => null,
    removeSessionSetting: vi.fn(),
    writeSessionSetting: vi.fn(),
    readBrowserLocalSetting: (_key: string, fallback: unknown) => fallback,
    writeBrowserLocalSetting: vi.fn(),
  };
});

vi.mock("@/shared/hooks/use-mobile", () => ({ useIsMobile: () => mocks.mobile }));

vi.mock("@/features/connections/hooks/useSessionViewport", async (importOriginal) => ({
  ...await importOriginal<typeof SessionViewportModule>(),
  useSessionTouchLayout: () => mocks.mobileSession,
}));
vi.mock("@/features/connections/components/connections/LiveSessionViewer", () => ({
  default: ({ sessionId, onReconnect }: { sessionId: string; onReconnect?: () => void }) => <div data-testid={`viewer-${sessionId}`}><input aria-label="Preserved SSH viewer" defaultValue="scrollback" /><button onClick={onReconnect}>Retry {sessionId}</button></div>,
}));
vi.mock("@/features/connections/components/connections/SessionHealthBar", () => ({ default: () => null }));

vi.mock("@/app/desktop/runtime", () => ({
  getTauriInvoke: () => null,
  isPipingDesktopShell: () => false,
}));

vi.mock("@/features/connections/components/connections/treeConfig", async () => {
  const actual = await vi.importActual<typeof TreeConfigModule>(
    "@/features/connections/components/connections/treeConfig",
  );
  return {
    ...actual,
    useTreeConfig: () => ({
      config: actual.DEFAULT_TREE_CONFIG,
      setConfig: vi.fn(),
      isLoading: false,
    }),
  };
});

vi.mock("@/features/observability/useHostLabels", () => ({
  useHostLabels: () => ({ byHost: new Map() }),
}));

vi.mock("@/features/connections/components/connections/ConnectionsFilters", async (importOriginal) => {
  const actual = await importOriginal<{ default: typeof ConnectionsFilters }>();
  return { default: (props: ComponentProps<typeof ConnectionsFilters>) => mocks.realFilters
    ? <actual.default {...props} /> : <div data-testid="connections-filters" /> };
});

vi.mock("@/features/playbooks/PlaybookRunsDock", () => ({ default: () => null }));
vi.mock("@/features/playbooks/PlaybookLaunchDialog", () => ({ default: () => null }));
vi.mock("@/features/connections/components/connections/TreeSettingsDialog", () => ({
  default: () => null,
}));
const clients = new Set<QueryClient>();

afterEach(async () => {
  cleanup();
  await act(async () => {
    await Promise.all([...clients].map((client) => client.cancelQueries()));
    clients.forEach((client) => client.clear());
    clients.clear();
  });
});

function renderWorkspace(filters: Record<string, string[]> = {}, visible = true, initialEntry = "/connections") {
  mocks.workspaceFilters = filters;
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  clients.add(client);
  const view = render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[initialEntry]}>
        <TooltipProvider>
        <ConnectionsWorkspace visible={visible} />
        </TooltipProvider>
      </MemoryRouter>
    </QueryClientProvider>,
  );
  return { ...view, client };
}

describe("ConnectionsWorkspace filter toggle accessibility", () => {
  beforeEach(() => {
    mocks.listConnectionTargets.mockReset();
    mocks.openSession.mockReset();
    mocks.closeSession.mockReset();
    mocks.getConnectionLaunchOptions.mockReset();
    mocks.closeSession.mockResolvedValue(undefined);
    mocks.listPublicConnectionFilterPresets.mockReset();
    mocks.getTargetPlaybookAssignments.mockReset();
    mocks.getTargetPlaybookAssignments.mockResolvedValue({ items: [] });
    mocks.launchJobTemplate.mockReset();
    mocks.notify.mockReset();
    mocks.createPublicConnectionFilterPreset.mockReset();
    mocks.realFilters = false;
    mocks.listPlaybooks.mockReset();
    mocks.useSetting.mockReset();
    mocks.mobileSession = false;
    mocks.mobile = false;
    mocks.sessionTabs = [];
    mocks.viewMode = "tree";
    mocks.canInventory = true;
    mocks.canReadInventory = true;
    mocks.listConnectionTargets.mockResolvedValue({ items: [], total: 0, limit: 0, offset: 0 });
    mocks.listPublicConnectionFilterPresets.mockResolvedValue([]);
    mocks.useSetting.mockImplementation((key: string, fallback: unknown) => ({
      value:
        key === "connections.workspace.filters"
          ? { viewMode: mocks.viewMode, filters: mocks.workspaceFilters }
          : fallback,
      setValue: vi.fn(),
      isLoading: false,
    }));
  });

  it("dismisses mobile details with the canonical sheet Escape", async () => {
    mocks.mobile = true;
    mocks.viewMode = "cards";
    mocks.listConnectionTargets.mockResolvedValue({ items: [{ id: "router-fixture", name: "router-1", targetKind: "device" }], total: 1, limit: 100, offset: 0 });
    renderWorkspace({}, true, "/connections?host=router-fixture");
    const inspector = await screen.findByRole("dialog", { name: "router-1" });
    fireEvent.keyDown(inspector, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "router-1" })).not.toBeInTheDocument());
  }, 20000);

  const targetA = { id: "host-a", name: "router-a", targetKind: "device" };
  const targetB = { id: "host-b", name: "router-b", targetKind: "device" };

  it("reveals URL hosts after loading and on back/forward without chasing refreshes", async () => {
    const scroll = vi.spyOn(Element.prototype, "scrollIntoView");
    const result = { items: [targetA, targetB], total: 2, limit: 100, offset: 0 };
    let resolveHosts!: (value: typeof result) => void;
    const pendingHosts = new Promise<typeof result>(resolve => { resolveHosts = resolve; });
    mocks.listConnectionTargets.mockReturnValue(pendingHosts);
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    clients.add(client);
    function Fixture() {
      const navigate = useNavigate();
      return <>
        <button onClick={() => void navigate("/connections?host=host-b")}>Other host</button>
        <button onClick={() => void navigate("/connections?host=host-b", { replace: true })}>Same host link</button>
        <button onClick={() => void navigate(-1)}>Previous link</button>
        <button onClick={() => void navigate(1)}>Next link</button>
        <ConnectionsWorkspace />
      </>;
    }
    render(<QueryClientProvider client={client}><MemoryRouter initialEntries={["/connections?host=host-a"]}><TooltipProvider><Fixture /></TooltipProvider></MemoryRouter></QueryClientProvider>);
    expect(scroll).not.toHaveBeenCalled();
    const revealed = () => {
      const element = scroll.mock.contexts.at(-1);
      return element instanceof Element ? element.textContent : null;
    };
    await act(async () => { resolveHosts(result); await pendingHosts; });
    await waitFor(() => expect(revealed()).toContain("router-a"));
    const other = screen.getByRole("button", { name: "Other host" });
    fireEvent.click(other);
    await waitFor(() => expect(revealed()).toContain("router-b"));
    const beforeSameLink = scroll.mock.calls.length;
    fireEvent.click(screen.getByRole("button", { name: "Same host link" }));
    await waitFor(() => expect(scroll).toHaveBeenCalledTimes(beforeSameLink + 1));
    fireEvent.click(screen.getByRole("button", { name: "Previous link" }));
    await waitFor(() => expect(revealed()).toContain("router-a"));
    fireEvent.click(screen.getByRole("button", { name: "Next link" }));
    await waitFor(() => expect(revealed()).toContain("router-b"));
    const calls = scroll.mock.calls.length;
    mocks.listConnectionTargets.mockResolvedValue({ ...result, items: [targetA, { ...targetB, primaryIp: "192.0.2.8" }] });
    await act(async () => { await client.invalidateQueries({ queryKey: CONNECTION_TARGETS_QUERY_KEY }); });
    expect(scroll).toHaveBeenCalledTimes(calls);
    scroll.mockRestore();
  }, 20000);

  it("restores a renamed group by UUID after editor return and refresh, but not an ordinary return", async () => {
    mocks.mobile = true;
    mocks.workspaceFilters = { site: ["DC-1"] };
    const siteId = "20000000-0000-0000-0000-000000000001";
    const targets = (site: string) => ({ items: [{ ...targetA, site, siteId }], total: 1, limit: 100, offset: 0 });
    mocks.listConnectionTargets.mockResolvedValue(targets("DC-1"));
    const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
    clients.add(client);
    function Fixture() {
      const location = useLocation();
      const navigate = useNavigate();
      const visible = location.pathname === "/connections";
      const returnTo = new URLSearchParams(location.search).get("return") ?? "/connections";
      return <>
        <output data-testid="lifecycle-url">{location.pathname}{location.search}</output>
        <button onClick={() => void navigate("/other")}>Leave workspace</button>
        <button onClick={() => void navigate("/connections")}>Ordinary return</button>
        <button onClick={() => void navigate("/connections?group=invalid")}>Invalid identity</button>
        <button onClick={() => void navigate("/connections?host=missing")}>Missing identity</button>
        <button onClick={() => void navigate(`/editor?return=${encodeURIComponent(location.pathname + location.search)}`)}>Open editor</button>
        <button onClick={() => void navigate(returnTo)}>Save or cancel</button>
        <ConnectionsWorkspace visible={visible} />
      </>;
    }
    const mount = (entry: string) => render(<QueryClientProvider client={client}><MemoryRouter initialEntries={[entry]}><TooltipProvider><Fixture /></TooltipProvider></MemoryRouter></QueryClientProvider>);
    const view = mount("/connections?fixture=kept");
    fireEvent.click(await screen.findByRole("button", { name: /^DC-1/ }));
    await screen.findByRole("dialog", { name: "DC-1" });
    const source = screen.getByTestId("lifecycle-url").textContent ?? "";
    expect(source).toContain("fixture=kept");
    expect(new URL(source, "http://localhost").searchParams.get("group")).toBeTruthy();
    fireEvent.click(screen.getByText("Ordinary return"));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "DC-1" })).not.toBeInTheDocument());
    expect(screen.getByTestId("lifecycle-url")).toHaveTextContent("/connections");
    fireEvent.click(await screen.findByRole("button", { name: /^DC-1/ }));
    await screen.findByRole("dialog", { name: "DC-1" });
    fireEvent.click(screen.getByText("Invalid identity"));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "DC-1" })).not.toBeInTheDocument());
    fireEvent.click(await screen.findByRole("button", { name: /^DC-1/ }));
    await screen.findByRole("dialog", { name: "DC-1" });
    fireEvent.click(screen.getByText("Missing identity"));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "DC-1" })).not.toBeInTheDocument());
    fireEvent.click(await screen.findByRole("button", { name: /^DC-1/ }));
    await screen.findByRole("dialog", { name: "DC-1" });
    fireEvent.click(screen.getByText("Open editor"));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "DC-1" })).not.toBeInTheDocument());
    act(() => { client.setQueryData(CONNECTION_TARGETS_QUERY_KEY, targets("DC-renamed")); });
    fireEvent.click(screen.getByRole("button", { name: "Save or cancel" }));
    await screen.findByRole("dialog", { name: "DC-renamed" });
    expect(screen.getByText("No targets match your search or filters")).toBeInTheDocument();
    fireEvent.click(screen.getByText("Leave workspace"));
    fireEvent.click(screen.getByRole("button", { name: "Ordinary return" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "DC-renamed" })).not.toBeInTheDocument());
    view.unmount();
    mocks.listConnectionTargets.mockResolvedValue(targets("DC-renamed"));
    mount(source);
    await screen.findByRole("dialog", { name: "DC-renamed" });
  }, 20000);
  const sessionA: LiveSession = { id: "session-a", hostId: "host-a", hostName: "router-a", protocol: "ssh", status: "connected", websocketUrl: "/api/v1/ws/ssh/session-a", token: "fixture" };
  const sessionB: LiveSession = { ...sessionA, id: "session-b", hostId: "host-b", hostName: "router-b", websocketUrl: "/api/v1/ws/ssh/session-b" };
  function configureInspector(sessions: LiveSession[] = []) {
    mocks.viewMode = "table";
    mocks.mobile = true;
    mocks.sessionTabs = sessions;
    mocks.listConnectionTargets.mockResolvedValue({ items: [targetA, targetB], total: 2, limit: 100, offset: 0 });
    mocks.useSetting.mockImplementation(function useReactiveSetting(key: string, fallback: unknown) {
      const initial = key === "connections.workspace.filters" ? { viewMode: "table", filters: {} }
        : key === "connections.workspace.panel" && sessions.length ? { showPanel: true, expanded: true, dockPosition: "bottom", sizes: { bottom: 320, right: 640 }, activeSession: { sessionId: sessions[0]!.id, hostId: sessions[0]!.hostId, protocol: sessions[0]!.protocol } }
          : fallback;
      const [value, setValue] = useState(initial);
      return { value, setValue, isLoading: false };
    });
  }
  it("closes a host card on same-path navigation without closing its live viewer", async () => {
    configureInspector([sessionA]);
    const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
    clients.add(client);
    function Fixture() {
      const navigate = useNavigate();
      return <><button onClick={() => void navigate("/connections")}>Sidebar Connections</button><ConnectionsWorkspace /></>;
    }
    render(<QueryClientProvider client={client}><MemoryRouter initialEntries={["/connections?host=host-a"]}><TooltipProvider><Fixture /></TooltipProvider></MemoryRouter></QueryClientProvider>);
    const viewer = await screen.findByTestId("viewer-session-a");
    await screen.findByRole("dialog", { name: "router-a" });
    fireEvent.click(screen.getByText("Sidebar Connections"));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "router-a" })).not.toBeInTheDocument());
    expect(screen.getByTestId("viewer-session-a")).toBe(viewer);
    expect(mocks.closeSession).not.toHaveBeenCalled();
  });
  async function openConnectionSection() {
    const dialog = await screen.findByRole("dialog", { name: "router-a" });
    const toggle = within(dialog).getByRole("button", { name: "Connections" });
    if (toggle.getAttribute("aria-expanded") === "false") fireEvent.click(toggle);
    return dialog;
  }

  it("warms table connection actions on row intent before opening the inspector", async () => {
    configureInspector();
    renderWorkspace();
    const name = await screen.findByText("router-a", { selector: "td *" });
    expect(mocks.getConnectionLaunchOptions).not.toHaveBeenCalled();
    fireEvent.pointerEnter(name.closest("tr")!);
    await waitFor(() => expect(mocks.getConnectionLaunchOptions).toHaveBeenCalledOnce());
    expect(screen.queryByRole("dialog", { name: "router-a" })).not.toBeInTheDocument();
    expect(mocks.openSession).not.toHaveBeenCalled();
  });

  it("launches the inspected host once and reports an out-of-set rejection without fallback", async () => {
    configureInspector();
    const hostId = "11111111-1111-4111-8111-111111111111";
    mocks.listConnectionTargets.mockResolvedValue({ items: [{ ...targetA, id: hostId }], total: 1, limit: 100, offset: 0 });
    mocks.getTargetPlaybookAssignments.mockResolvedValue({ items: [{ assignment: { id: "assignment", name: "Collect", job_template_id: "job", enabled: true, executable: true }, job_template: { id: "job", name: "Collect", inventory: { kind: "managed", target_set_id: "set" } } }] });
    const { ApiError } = await import("@/shared/api/client");
    mocks.launchJobTemplate.mockRejectedValue(new ApiError({ status: 422, code: "VALIDATION_ERROR", message: "outside inventory" }));
    renderWorkspace({}, true, `/connections?host=${hostId}`);
    const dialog = await screen.findByRole("dialog", { name: "router-a" });
    const section = await within(dialog).findByRole("button", { name: "Jobs" });
    expect(section).toHaveAttribute("aria-expanded", "false");
    expect(mocks.getTargetPlaybookAssignments).not.toHaveBeenCalled();
    fireEvent.click(section);
    fireEvent.click(await within(dialog).findByRole("button", { name: "Collect" }));
    await waitFor(() => expect(mocks.launchJobTemplate).toHaveBeenCalledOnce());
    expect(mocks.launchJobTemplate).toHaveBeenCalledWith("job", { target_ids: [hostId], metadata: { source: "connections.host_assignment", target_count: 1 } });
    await waitFor(() => expect(mocks.notify).toHaveBeenCalledWith("error", expect.stringMatching(/selected|inventory/i), { source: "playbooks" }));
    expect(mocks.launchJobTemplate).toHaveBeenCalledOnce();
  }, 15000);

  it("closes details after a new session succeeds, retains the selected row and preserves existing viewers", async () => {
    configureInspector([sessionB]);
    mocks.getConnectionLaunchOptions.mockResolvedValue({ protocols: [{ protocol: "ssh", accounts: [{ entryId: "saved", kind: "shared", label: "Operator", username: "qa", requiresInput: false }] }] });
    mocks.openSession.mockResolvedValue({ session: { id: "session-new", hostId: "host-a", protocol: "ssh" }, proxy: { websocketUrl: "/api/v1/ws/ssh/session-new", token: "fixture" } });
    renderWorkspace({}, true, "/connections?host=host-a");
    const existingViewer = await screen.findByTestId("viewer-session-b");
    const dialog = await openConnectionSection();
    fireEvent.click(await within(dialog).findByRole("button", { name: "SSH Operator" }));
    await screen.findByTestId("viewer-session-new");
    expect(screen.queryByRole("dialog", { name: "router-a" })).not.toBeInTheDocument();
    expect(screen.getByTestId("viewer-session-b")).toBe(existingViewer);
    expect(screen.getByText("router-a", { selector: "td *" }).closest("tr")).toHaveClass("bg-primary/6");
  }, 15000);

  it("launches an unplaced map target directly without opening details or replacing an existing viewer", async () => {
    mocks.viewMode = "map";
    mocks.sessionTabs = [sessionB];
    mocks.mapSites.mockResolvedValue([]);
    mocks.listConnectionTargets.mockResolvedValue({ items: [targetA, targetB], total: 2, limit: 100, offset: 0 });
    mocks.getConnectionLaunchOptions.mockResolvedValue({ protocols: [{ protocol: "ssh", accounts: [{ entryId: "saved", kind: "shared", label: "Operator", username: "qa", requiresInput: false }] }] });
    mocks.openSession.mockResolvedValue({ session: { id: "session-new", hostId: "host-a", protocol: "ssh" }, proxy: { websocketUrl: "/api/v1/ws/ssh/session-new", token: "fixture" } });
    renderWorkspace();
    fireEvent.click(await screen.findByRole("button", { name: "Unplaced targets (2)" }));
    const action = screen.getByTestId("quick-access-host-a");
    fireEvent.pointerEnter(action);
    fireEvent.pointerDown(action, { button: 0, ctrlKey: false });
    fireEvent.click(await screen.findByRole("menuitem", { name: /SSH.*Operator/ }));
    expect(screen.queryByRole("complementary", { name: "router-a" })).not.toBeInTheDocument();
    expect(screen.queryByRole("dialog", { name: "router-a" })).not.toBeInTheDocument();
    await screen.findByTestId("viewer-session-new");
    expect(mocks.openSession).toHaveBeenCalled();
    expect(mocks.closeSession).not.toHaveBeenCalled();
  }, 15000);

  it("foregrounds an existing tab and hides details without disconnecting", async () => {
    configureInspector([sessionA, sessionB]);
    mocks.mobile = false;
    renderWorkspace({}, true, "/connections?host=host-a");
    await screen.findByRole("complementary", { name: "router-a" });
    const viewer = screen.getByTestId("viewer-session-b");
    fireEvent.click(screen.getByRole("tab", { name: /router-b/ }));
    await waitFor(() => expect(screen.queryByRole("complementary", { name: "router-a" })).not.toBeInTheDocument());
    expect(screen.getByTestId("viewer-session-b")).toBe(viewer);
    expect(mocks.openSession).not.toHaveBeenCalled();
    expect(mocks.closeSession).not.toHaveBeenCalled();
  });

  it("preserves existing sessions when options fail and launches the authoritative account after retry", async () => {
    configureInspector([sessionA, sessionB]);
    mocks.getConnectionLaunchOptions.mockRejectedValue(new Error("launch options unavailable"));
    mocks.openSession.mockResolvedValue({ session: { id: "session-new", hostId: "host-a", protocol: "ssh" }, proxy: { websocketUrl: "/api/v1/ws/ssh/session-new", token: "fixture" } });
    renderWorkspace({}, true, "/connections?host=host-a");
    const viewer = await screen.findByTestId("viewer-session-a");
    const otherViewer = screen.getByTestId("viewer-session-b");
    const dialog = await openConnectionSection();
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Unable to load connection options.");
    expect(within(dialog).queryByRole("button", { name: /^SSH/ })).not.toBeInTheDocument();
    expect(mocks.openSession).not.toHaveBeenCalled();
    expect(mocks.closeSession).not.toHaveBeenCalled();
    expect(screen.getByTestId("viewer-session-a")).toBe(viewer);
    expect(screen.getByTestId("viewer-session-b")).toBe(otherViewer);

    mocks.getConnectionLaunchOptions.mockResolvedValue({ protocols: [{ protocol: "ssh", accounts: [{ entryId: "saved", kind: "shared", label: "Operator", username: "qa", requiresInput: false }] }] });
    fireEvent.click(within(dialog).getByRole("button", { name: "Retry" }));
    fireEvent.click(await within(dialog).findByRole("button", { name: "SSH Operator" }));
    await screen.findByTestId("viewer-session-new");
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "router-a" })).not.toBeInTheDocument());
    expect(screen.getByTestId("viewer-session-a")).toBe(viewer);
    expect(screen.getByTestId("viewer-session-b")).toBe(otherViewer);
    expect(mocks.openSession).toHaveBeenCalledOnce();
    expect(mocks.openSession).toHaveBeenCalledWith({ hostId: "host-a", protocol: "ssh", entryId: "saved" });
    expect(mocks.closeSession).not.toHaveBeenCalled();
  }, 15000);

  it("opens the active session's target instead of a different selected catalogue row", async () => {
    configureInspector([sessionB]);
    renderWorkspace({}, true, "/connections?host=host-a");
    const dialog = await screen.findByRole("dialog", { name: "router-a" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Close" }));
    fireEvent.pointerDown(within(screen.getByTestId("session-workspace")).getByRole("button", { name: "Session and panel actions" }), { button: 0, ctrlKey: false });
    fireEvent.click(screen.getByRole("menuitem", { name: "Details" }));
    expect(await screen.findByRole("dialog", { name: "router-b" })).toBeInTheDocument();
    expect(mocks.closeSession).not.toHaveBeenCalled();
  });

  it("keeps selection after close and does not reopen a consumed deep link on a catalogue refresh", async () => {
    configureInspector();
    const { client } = renderWorkspace({}, true, "/connections?host=host-a");
    const dialog = await screen.findByRole("dialog", { name: "router-a" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Close" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "router-a" })).not.toBeInTheDocument());
    await act(async () => { await client.invalidateQueries({ queryKey: CONNECTION_TARGETS_QUERY_KEY }); });
    expect(screen.queryByRole("dialog", { name: "router-a" })).not.toBeInTheDocument();
    expect(screen.getByText("router-a", { selector: "td *" }).closest("tr")).toHaveClass("bg-primary/6");
  });

  it("leaves details open when a credential prompt is canceled or a connection fails", async () => {
    configureInspector();
    mocks.getConnectionLaunchOptions.mockResolvedValue({ protocols: [{ protocol: "ssh", accounts: [{ entryId: "input", kind: "shared", label: "Operator", username: "qa", requiresInput: true }] }] });
    mocks.openSession.mockRejectedValue(new Error("connection failed"));
    renderWorkspace({}, true, "/connections?host=host-a");
    const inspector = await openConnectionSection();
    fireEvent.click(await within(inspector).findByRole("button", { name: "SSH Operator" }));
    const password = await screen.findByLabelText(/Password/);
    const prompt = password.closest('[role="dialog"]')!;
    fireEvent.click(within(prompt as HTMLElement).getByRole("button", { name: "Cancel" }));
    expect(await screen.findByRole("dialog", { name: "router-a" })).toBeInTheDocument();
    fireEvent.click(within(screen.getByRole("dialog", { name: "router-a" })).getByRole("button", { name: "SSH Operator" }));
    const nextPassword = await screen.findByLabelText(/Password/);
    fireEvent.change(nextPassword, { target: { value: "test-input" } });
    fireEvent.click(within(nextPassword.closest('[role="dialog"]') as HTMLElement).getByRole("button", { name: /Connect/ }));
    await waitFor(() => expect(mocks.openSession).toHaveBeenCalledOnce());
    expect(await within(nextPassword.closest('[role="dialog"]') as HTMLElement).findByRole("alert")).toBeInTheDocument();
    expect(inspector).toBeInTheDocument();
  }, 15000);

  it.each(["tree", "cards", "table"])("owns one shared canvas for ordinary %s browsing and retains its search across view changes", async mode => {
    mocks.viewMode = mode;
    const { container } = renderWorkspace();
    const search = screen.getByRole("textbox");
    expect(search.closest('[data-slot="page-layout"]')).not.toBeNull();
    expect(container.querySelectorAll('[data-slot="page-layout"]')).toHaveLength(1);
    fireEvent.change(search, { target: { value: "retained query" } });
    fireEvent.click(screen.getByRole("button", { name: mode === "cards" ? "Table" : "Cards" }));
    expect(screen.getByRole("textbox")).toHaveValue("retained query");
    expect(container.querySelectorAll('[data-slot="page-layout"]')).toHaveLength(1);
    await screen.findByText("No targets match your search or filters");
  });

  it("reports whether the filter panel is collapsed or expanded", () => {
    renderWorkspace();

    const toggle = screen.getByRole("button", { name: "Filters" });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByTestId("connections-filters")).toBeNull();

    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByTestId("connections-filters")).toBeInTheDocument();

    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByTestId("connections-filters")).toBeNull();
  });

  it("keeps a failed public preset save retryable through the workspace callback", async () => {
    mocks.realFilters = true;
    mocks.listConnectionTargets.mockResolvedValue({ items: [{ id: "host-a", name: "router-a", targetKind: "device", status: "active" }], total: 1, limit: 100, offset: 0 });
    mocks.createPublicConnectionFilterPreset.mockRejectedValueOnce(new Error("server failure")).mockResolvedValueOnce({});
    renderWorkspace();
    await screen.findByText("router-a");
    fireEvent.click(screen.getByRole("button", { name: "Filters" }));
    fireEvent.click(screen.getByRole("button", { name: "active" }));
    fireEvent.click(screen.getByRole("button", { name: "Save filter" }));
    const dialog = screen.getByRole("dialog", { name: "Save filter" });
    fireEvent.change(within(dialog).getByLabelText(/Filter name/), { target: { value: "Team" } });
    fireEvent.click(within(dialog).getByLabelText("Public"));
    fireEvent.click(within(dialog).getByRole("button", { name: "Save filter" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Could not save filter");
    expect(within(dialog).getByLabelText(/Filter name/)).toHaveValue("Team");
    expect(within(dialog).getByLabelText("Public")).toBeChecked();
    expect(within(dialog).getByText("1 selected")).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button", { name: "Save filter" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Save filter" })).not.toBeInTheDocument());
    expect(mocks.createPublicConnectionFilterPreset).toHaveBeenCalledTimes(2);
    expect(mocks.createPublicConnectionFilterPreset.mock.calls[1]![0]).toEqual({ name: "Team", filters: expect.objectContaining({ status: ["active"] }) });
  }, 20000);

  it("paginates the filtered table through the shared table controls", async () => {
    mocks.viewMode = "table";
    const items = Array.from({ length: 51 }, (_, index) => ({ id: `host-${index}`, name: `host-${index}`, targetKind: "device" }));
    mocks.listConnectionTargets.mockResolvedValue({ items, total: items.length, limit: 100, offset: 0 });
    renderWorkspace();
    expect(await screen.findByText("host-0")).toBeInTheDocument();
    expect(screen.queryByText("host-50")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Next page" }));
    expect(screen.getByText("host-50")).toBeInTheDocument();
    expect(screen.queryByText("host-0")).toBeNull();
  });

  it("uses the saved page size and persists a change across remounts", async () => {
    mocks.viewMode = "table";
    let savedPageSize = 100;
    mocks.useSetting.mockImplementation(function useReactiveSetting(key: string, fallback: unknown) {
      const [pageSize, setPageSize] = useState(savedPageSize);
      if (key === "table.page_size") {
        return {
          value: pageSize,
          setValue: (value: number) => { savedPageSize = value; setPageSize(value); },
          isLoading: false,
        };
      }
      return {
        value: key === "connections.workspace.filters"
          ? { viewMode: mocks.viewMode, filters: mocks.workspaceFilters }
          : fallback,
        setValue: vi.fn(),
        isLoading: false,
      };
    });
    const items = Array.from({ length: 101 }, (_, index) => ({ id: `host-${index}`, name: `host-${index}`, targetKind: "device" }));
    mocks.listConnectionTargets.mockResolvedValue({ items, total: items.length, limit: 101, offset: 0 });

    const first = renderWorkspace();
    expect(await screen.findByText("1–100 of 101")).toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "Rows per page" })).toHaveValue("100");
    fireEvent.click(screen.getByRole("button", { name: "Next page" }));
    expect(screen.getByText("101–101 of 101")).toBeInTheDocument();
    fireEvent.change(screen.getByRole("combobox", { name: "Rows per page" }), { target: { value: "50" } });
    expect(screen.getByText("1–50 of 101")).toBeInTheDocument();
    first.unmount();

    renderWorkspace();
    expect(await screen.findByText("1–50 of 101")).toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "Rows per page" })).toHaveValue("50");
  });

  it("retains bulk checks across pages and selects all filtered results, not only the page", async () => {
    mocks.viewMode = "table";
    const items = Array.from({ length: 70 }, (_, index) => ({ id: `host-${index}`, name: `${index < 60 ? "match" : "other"}-${index}`, targetKind: "device" }));
    mocks.listConnectionTargets.mockImplementation((params?: { search?: string }) => {
      const search = params?.search;
      const result = search ? items.filter((item) => item.name.includes(search)) : items;
      return Promise.resolve({ items: result, total: result.length, limit: 100, offset: 0 });
    });
    renderWorkspace();
    await screen.findByText("match-0");
    fireEvent.pointerDown(screen.getByRole("button", { name: "Catalogue view and actions" }), { button: 0, ctrlKey: false });
    fireEvent.click(await screen.findByRole("menuitem", { name: "Select hosts for job launch" }));
    fireEvent.click(screen.getByRole("checkbox", { name: "Select row host-0" }));
    fireEvent.click(screen.getByRole("button", { name: "Next page" }));
    fireEvent.click(screen.getByRole("checkbox", { name: "Select row host-50" }));
    expect(screen.getByText("2 selected")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Previous page" }));
    expect(screen.getByRole("checkbox", { name: "Select row host-0" })).toBeChecked();
    fireEvent.change(screen.getByRole("textbox"), { target: { value: "match" } });
    await waitFor(() => expect(mocks.listConnectionTargets).toHaveBeenCalledWith(expect.objectContaining({ search: "match" })));
    await screen.findByText("1–50 of 60");
    fireEvent.click(screen.getByRole("button", { name: "Select visible" }));
    expect(await screen.findByText("60 selected")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Next page" }));
    expect(screen.getByRole("checkbox", { name: "Select row host-59" })).toBeChecked();
  }, 20000);

  it("slices the server-sorted result after sorting the entire table", async () => {
    mocks.viewMode = "table";
    const items = Array.from({ length: 51 }, (_, index) => ({ id: `host-${index}`, name: `host-${String(index).padStart(2, "0")}`, targetKind: "device" }));
    mocks.listConnectionTargets.mockImplementation((params?: { sort?: string }) => Promise.resolve({ items: params?.sort ? [...items].reverse() : items, total: items.length, limit: 100, offset: 0 }));
    renderWorkspace();
    await screen.findByText("host-00");
    fireEvent.click(screen.getByRole("button", { name: /Name/i }));
    await waitFor(() => expect(mocks.listConnectionTargets).toHaveBeenCalledWith(expect.objectContaining({ sort: "name" })));
    expect(await screen.findByText("host-50")).toBeInTheDocument();
    expect(screen.queryByText("host-00")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Next page" }));
    expect(screen.getByText("host-00")).toBeInTheDocument();
  });

  it("does not mount or fetch the persisted map without Inventory read permission", async () => {
    mocks.viewMode = "map";
    mocks.canReadInventory = false;
    mocks.mapSites.mockClear();
    mocks.listConnectionTargets.mockResolvedValue({ items: [{ id: "router-fixture", name: "router-1", targetKind: "device" }], total: 1, limit: 100, offset: 0 });
    renderWorkspace();
    expect(await screen.findByText(/Map requires read access to Inventory/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Map" })).toBeNull();
    expect(mocks.mapSites).not.toHaveBeenCalled();
    expect(screen.getByRole("textbox").closest('[data-slot="page-layout"]')).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Cards" }));
    expect(screen.getByRole("button", { name: "Cards" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("textbox").closest('[data-slot="page-layout"]')).not.toBeNull();
  });

  it.each(["tree", "cards", "table"])("shows a safe target-query error and retries the failed catalogue source in %s", async mode => {
    mocks.viewMode = mode;
    mocks.listConnectionTargets.mockRejectedValueOnce(new Error("private backend detail"));
    renderWorkspace();
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not load connection targets");
    expect(screen.queryByText("private backend detail")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByText("No connection targets available")).toBeInTheDocument();
    expect(mocks.listConnectionTargets).toHaveBeenCalledTimes(2);
  });

  it.each(["tree", "cards", "table"])("announces initial loading without showing empty setup in %s", async mode => {
    mocks.viewMode = mode;
    mocks.listConnectionTargets.mockImplementation(() => new Promise(() => {}));
    renderWorkspace();
    expect(await screen.findByRole("status")).toHaveTextContent("Loading connection targets");
    expect(screen.queryByText("No connection targets available")).toBeNull();
  });

  it.each(["tree", "cards", "table"])("offers the existing device and VM editors when empty in %s", async mode => {
    mocks.viewMode = mode;
    renderWorkspace();
    expect(await screen.findByRole("link", { name: "Add device" })).toHaveAttribute("href", expect.stringContaining("return=%2Fconnections"));
    expect(screen.getByRole("link", { name: "Add virtual machine" })).toHaveAttribute("href", expect.stringContaining("Virtual%20Machines"));
  });

  it.each(["tree", "cards", "table"])("normalizes whitespace search without hiding accessible targets in %s", async mode => {
    const height = vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(600);
    const width = vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockReturnValue(900);
    mocks.viewMode = mode;
    mocks.listConnectionTargets.mockResolvedValue({ items: [{ id: "router-fixture", name: "router-1", targetKind: "device" }], total: 1, limit: 100, offset: 0 });
    const { client } = renderWorkspace();
    expect((await screen.findAllByText("router-1")).length).toBeGreaterThan(0);
    const search = screen.getByRole("textbox");
    vi.useFakeTimers();
    try {
      for (const value of [" ", " router-1 "]) {
        fireEvent.change(search, { target: { value } });
        await act(async () => { await vi.advanceTimersByTimeAsync(200); });
        await act(async () => { await vi.advanceTimersByTimeAsync(1); });
        expect(screen.getAllByText("router-1").length).toBeGreaterThan(0);
        expect(search).toHaveValue(value);
        expect(screen.queryByText("No connection targets available")).toBeNull();
        expect(screen.queryByRole("link", { name: "Add device" })).toBeNull();
        expect(screen.queryByText("No targets match your search or filters")).toBeNull();
        if (value === " ") expect(mocks.listConnectionTargets).toHaveBeenCalledTimes(1);
      }
      if (mode === "table") {
        expect(mocks.listConnectionTargets.mock.calls[1]?.[0]).toEqual(expect.objectContaining({ search: "router-1" }));
        const key = client.getQueryCache().findAll({ queryKey: [...CONNECTION_TARGETS_QUERY_KEY, "table"] })
          .find(query => query.state.status === "success")?.queryKey;
        expect(key?.[3]).toEqual(expect.objectContaining({ search: "router-1" }));
      }
    } finally {
      vi.useRealTimers();
      height.mockRestore();
      width.mockRestore();
    }
  });

  it.each(["READ", "CREATE"])("explains missing inventory %s permission without inactive create controls", async action => {
    if (action === "READ") mocks.canReadInventory = false;
    else mocks.canInventory = false;
    renderWorkspace();
    expect(await screen.findByText(/You need permission to read and create/)).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Add device" })).toBeNull();
  });

  it("recognizes facet-only no matches and resets facets with search focus", async () => {
    renderWorkspace({ status: ["offline"] });
    expect(await screen.findByText("No targets match your search or filters")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Reset search and filters" }));
    expect(await screen.findByText("No connection targets available")).toBeInTheDocument();
    expect(screen.getByRole("textbox")).toHaveFocus();
    expect(screen.getByRole("button", { name: "Filters" })).toBeInTheDocument();
  });

  it("retains the mounted viewer during a failed background catalogue refresh", async () => {
    mocks.viewMode = "table";
    mocks.listConnectionTargets.mockResolvedValue({ items: [{ id: "host-fixture", name: "Cached target", targetKind: "device" }], total: 1, limit: 100, offset: 0 });
    mocks.sessionTabs = [{ id: "session-fixture", hostId: "host-fixture", hostName: "Fixture host",
      protocol: "ssh", status: "connected", websocketUrl: "/api/v1/ws/ssh/session-fixture", token: "fixture" }];
    const { client } = renderWorkspace();
    const viewer = await screen.findByRole("textbox", { name: "Preserved SSH viewer" });
    expect(viewer.closest('[data-slot="page-layout"]')).toBeNull();
    await screen.findByText("Cached target");
    mocks.listConnectionTargets.mockRejectedValue(new Error("private stale failure"));
    await act(async () => { await client.invalidateQueries({ queryKey: CONNECTION_TARGETS_QUERY_KEY }); });
    expect(await screen.findByRole("alert")).toHaveTextContent("Previously loaded targets are shown");
    expect(screen.queryByText("No connection targets available")).toBeNull();
    expect(screen.getByText("Cached target")).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Preserved SSH viewer" })).toBe(viewer);
    expect(screen.queryByText("private stale failure")).toBeNull();
  });

  it("retries the failed filtered table source without refetching a successful base query", async () => {
    mocks.viewMode = "table";
    const { client } = renderWorkspace();
    await screen.findByText("No connection targets available");
    mocks.listConnectionTargets.mockRejectedValueOnce(new Error("filtered table denied"))
      .mockResolvedValue({ items: [], total: 0, limit: 0, offset: 0 });
    const search = screen.getByRole("textbox");
    fireEvent.change(search, { target: { value: "absent" } });
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not load connection targets");
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByText("No targets match your search or filters")).toBeInTheDocument();
    expect(mocks.listConnectionTargets).toHaveBeenCalledTimes(3);
    expect(mocks.listConnectionTargets.mock.calls[2]?.[0]).toEqual(expect.objectContaining({ search: "absent" }));
    expect(client.getQueryState(CONNECTION_TARGETS_QUERY_KEY)?.status).toBe("success");
    fireEvent.click(screen.getByRole("button", { name: "Reset search and filters" }));
    expect(search).toHaveFocus();
    expect(search).toHaveValue("");
  });

  it.each([true, false])("canonical workspace title registers only while visible=%s", visible => {
    renderWorkspace({}, visible);
    const title = within(screen.getByRole("banner")).queryByRole("heading", { level: 1, name: "Connections Workspace" });
    if (visible) expect(title).toBeInTheDocument();
    else expect(title).not.toBeInTheDocument();
  });

  it("returns from mobile SSH to targets and resumes the same viewer", async () => {
    mocks.mobileSession = true;
    mocks.sessionTabs = [{ id: "session-fixture", hostId: "host-fixture", hostName: "Fixture host",
      protocol: "ssh", status: "connected", websocketUrl: "/api/v1/ws/ssh/session-fixture", token: "fixture" }];
    const panel = { showPanel: true, expanded: true, dockPosition: "bottom",
      sizes: { bottom: 140, right: 320 },
      activeSession: { sessionId: "session-fixture", hostId: "host-fixture", protocol: "ssh" } };
    mocks.useSetting.mockImplementation((key: string, fallback: unknown) => ({
      value: key === "connections.workspace.panel" ? panel : fallback,
      setValue: vi.fn(), isLoading: false,
    }));
    renderWorkspace();
    const viewer = await screen.findByRole("textbox", { name: "Preserved SSH viewer" });
    const workspace = screen.getByTestId("session-workspace");
    expect(workspace).not.toHaveClass("hidden");
    fireEvent.click(screen.getByRole("button", { name: "Back to connections" }));
    expect(workspace).toHaveClass("hidden");
    fireEvent.click(screen.getByRole("button", { name: /^(Show )?1 open tab$/ }));
    expect(workspace).not.toHaveClass("hidden");
    expect(screen.getByRole("textbox", { name: "Preserved SSH viewer" })).toBe(viewer);
  });

  it("Retry collects fresh credentials, cancels without mutation, and replaces only the original pane and tab", async () => {
    const a = { id: "session-a", hostId: "host-a", hostName: "SSH A", protocol: "ssh" as const, status: "failed" as const, websocketUrl: "/ssh/a", token: "fixture" };
    const b = { ...a, id: "session-b", hostId: "host-b", hostName: "SSH B", status: "connected" as const };
    const vnc = { ...b, id: "session-vnc", hostId: "host-vnc", hostName: "VNC", protocol: "vnc" as const };
    mocks.sessionTabs = [a, b, vnc];
    const panel = { showPanel: true, expanded: true, dockPosition: "right", sizes: { bottom: 320, right: 640 }, activeSession: { sessionId: b.id, hostId: b.hostId, protocol: b.protocol },
      paneLayout: { root: { kind: "split", id: "split-2", direction: "horizontal", ratio: 0.4,
        first: { kind: "pane", id: "pane-1", sessionId: a.id }, second: { kind: "pane", id: "pane-2", sessionId: b.id } }, focusedPaneId: "pane-2", maximizedPaneId: null } };
    mocks.useSetting.mockImplementation(function useReactiveSetting(key: string, fallback: unknown) {
      const [value, setValue] = useState(key === "connections.workspace.panel" ? panel : fallback);
      return { value, setValue, isLoading: false };
    });
    mocks.getConnectionLaunchOptions.mockResolvedValue({ protocols: [{ protocol: "ssh", accounts: [{ entryId: "current-entry", kind: "shared", label: "Operator", username: "qa", requiresInput: true }] }] });
    mocks.openSession.mockResolvedValue({ session: { id: "session-new", hostId: a.hostId, protocol: "ssh" }, proxy: { websocketUrl: "/ssh/new", token: "fixture" } });
    renderWorkspace();
    const viewerB = await screen.findByTestId("viewer-session-b");
    const viewerVnc = screen.getByTestId("viewer-session-vnc");
    fireEvent.click(screen.getByRole("button", { name: "Retry session-a" }));
    await screen.findByLabelText(/Password/);
    const firstDialog = screen.getByRole("dialog");
    expect(mocks.openSession).not.toHaveBeenCalled();
    expect(mocks.closeSession).not.toHaveBeenCalled();
    fireEvent.click(within(firstDialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(screen.getByTestId("viewer-session-a")).toBeInTheDocument();
    expect(mocks.closeSession).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Retry session-a" }));
    await screen.findByLabelText(/Password/);
    const dialog = screen.getByRole("dialog");
    const password = crypto.randomUUID().slice(0, 8);
    fireEvent.change(within(dialog).getByLabelText(/Password/), { target: { value: password } });
    fireEvent.click(within(dialog).getByRole("button", { name: /Connect/ }));
    await screen.findByTestId("viewer-session-new");
    const request = mocks.openSession.mock.calls[0]?.[0] as { entryId?: string; credentials?: { password?: string } };
    expect(request.entryId).toBe("current-entry");
    expect(request.credentials?.password === password).toBe(true);
    expect(mocks.closeSession).toHaveBeenCalledExactlyOnceWith(a.id);
    expect(screen.getByRole("combobox", { name: "Session in pane 1" })).toHaveValue("session-new");
    expect(screen.getByRole("combobox", { name: "Session in pane 2" })).toHaveValue(b.id);
    expect(screen.getByTestId("viewer-session-b")).toBe(viewerB);
    expect(screen.getByTestId("viewer-session-vnc")).toBe(viewerVnc);
    const tabs = screen.getAllByRole("tab");
    expect(tabs).toHaveLength(2);
    expect(tabs[0]).toHaveAccessibleName("Work group · 2");
    expect(tabs[1]).toHaveAccessibleName(/VNC/);
    const paneOne = screen.getByRole("combobox", { name: "Session in pane 1" });
    expect(within(paneOne).getAllByRole("option").map(option => (option as HTMLOptionElement).value)).toEqual(["", "session-new", b.id, vnc.id]);
    mocks.getConnectionLaunchOptions.mockResolvedValue({ protocols: [{ protocol: "ssh", accounts: [{ entryId: "saved-entry", kind: "shared", label: "Operator", username: "qa", requiresInput: false }] }] });
    let resolveOpen!: (value: unknown) => void;
    mocks.openSession.mockReturnValueOnce(new Promise(resolve => { resolveOpen = resolve; }));
    fireEvent.click(screen.getByRole("button", { name: "Retry session-new" }));
    await waitFor(() => expect(mocks.openSession).toHaveBeenCalledTimes(2));
    fireEvent.change(screen.getByRole("combobox", { name: "Session in pane 1", hidden: true }), { target: { value: "" } });
    const tab = screen.getByRole("tab", { name: /SSH A/, hidden: true });
    fireEvent.click(within(tab.parentElement!).getByRole("button", { name: "Close session", hidden: true }));
    await act(async () => { resolveOpen({ session: { id: "orphan", hostId: a.hostId, protocol: "ssh" }, proxy: { websocketUrl: "/ssh/orphan", token: "fixture" } }); await Promise.resolve(); });
    await waitFor(() => expect(mocks.closeSession).toHaveBeenCalledWith("orphan"));
    expect(screen.queryByTestId("viewer-orphan")).not.toBeInTheDocument();
    expect(screen.queryByTestId("viewer-session-new")).not.toBeInTheDocument();
    expect(screen.getByTestId("viewer-session-b")).toBe(viewerB);
  }, 15_000);

  it("does not restore a preferred session over an empty work group", async () => {
    mocks.sessionTabs = [{ id: "session-fixture", hostId: "host-fixture", hostName: "Fixture host",
      protocol: "ssh", status: "connected", websocketUrl: "/api/v1/ws/ssh/session-fixture", token: "fixture" }];
    const panel = { showPanel: true, expanded: true, dockPosition: "right", sizes: { bottom: 320, right: 480 }, activeSession: null,
      paneLayout: { root: { kind: "split", id: "split-2", direction: "horizontal", ratio: 0.4,
        first: { kind: "pane", id: "pane-1", sessionId: null }, second: { kind: "pane", id: "pane-2", sessionId: null } },
        focusedPaneId: "pane-2", maximizedPaneId: null } };
    mocks.useSetting.mockImplementation((key: string, fallback: unknown) => ({
      value: key === "connections.workspace.panel" ? panel : fallback, setValue: vi.fn(), isLoading: false,
    }));
    renderWorkspace();
    expect(await screen.findByRole("tab", { name: "Work group · 0" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("combobox", { name: "Session in pane 1" })).toHaveValue("");
    expect(screen.getByRole("combobox", { name: "Session in pane 2" })).toHaveValue("");
  });

  it("announces the active-filter count as part of the toggle purpose", () => {
    renderWorkspace({ status: ["online"], site: ["dc-1"] });

    const toggle = screen.getByRole("button", { name: "Filters (2 active)" });
    expect(screen.queryByRole("button", { name: "2" })).toBeNull();
    expect(within(toggle).getByText("2")).toHaveAttribute("aria-hidden", "true");
  });

  it("caps only the visual badge while announcing the exact active-filter count", () => {
    renderWorkspace({ status: Array.from({ length: 101 }, (_, index) => `status-${index}`) });

    const toggle = screen.getByRole("button", { name: "Filters (101 active)" });
    expect(within(toggle).getByText("99+")).toHaveAttribute("aria-hidden", "true");
  });
});
