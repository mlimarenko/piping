import { act, cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { useState } from "react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import type * as ConnectionsApiModule from "@/features/connections/api";
import type * as PlaybooksApiModule from "@/features/playbooks/api";
import type * as SettingsModule from "@/shared/settings";
import type * as InventoryApiModule from "@/features/inventory/api";
import type * as TreeConfigModule from "@/features/connections/components/connections/treeConfig";
import type * as SessionViewportModule from "./hooks/useSessionViewport";

import ConnectionsWorkspace, { type LiveSession } from "./ConnectionsWorkspace";
import { CONNECTION_TARGETS_QUERY_KEY } from "./api";
import { TooltipProvider } from "@/shared/ui-primitives/tooltip";

const mocks = vi.hoisted(() => ({
  listConnectionTargets: vi.fn(),
  openSession: vi.fn(),
  closeSession: vi.fn(),
  getConnectionLaunchOptions: vi.fn(),
  listPublicConnectionFilterPresets: vi.fn(),
  listPlaybooks: vi.fn(),
  useSetting: vi.fn(),
  workspaceFilters: ((): Record<string, string[]> => ({}))(),
  mobileSession: false,
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
  createPublicConnectionFilterPreset: vi.fn(),
  deletePublicConnectionFilterPreset: vi.fn(),
}));

vi.mock("@/features/playbooks/api", async () => {
  const actual = await vi.importActual<typeof PlaybooksApiModule>(
    "@/features/playbooks/api",
  );
  return {
    ...actual,
    listPlaybooks: mocks.listPlaybooks,
    getPlaybookRunGroup: vi.fn(),
  };
});

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

vi.mock("@/shared/hooks/use-mobile", () => ({ useIsMobile: () => false }));

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

vi.mock("@/features/connections/components/connections/ConnectionsFilters", () => ({
  default: () => <div data-testid="connections-filters" />,
}));

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

function renderWorkspace(filters: Record<string, string[]> = {}, visible = true) {
  mocks.workspaceFilters = filters;
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  clients.add(client);
  const view = render(
    <QueryClientProvider client={client}>
      <MemoryRouter>
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
    mocks.listPlaybooks.mockReset();
    mocks.useSetting.mockReset();
    mocks.mobileSession = false;
    mocks.sessionTabs = [];
    mocks.viewMode = "tree";
    mocks.canInventory = true;
    mocks.canReadInventory = true;
    mocks.listConnectionTargets.mockResolvedValue({ items: [], total: 0, limit: 0, offset: 0 });
    mocks.listPublicConnectionFilterPresets.mockResolvedValue([]);
    mocks.listPlaybooks.mockResolvedValue({ items: [] });
    mocks.useSetting.mockImplementation((key: string, fallback: unknown) => ({
      value:
        key === "connections.workspace.filters"
          ? { viewMode: mocks.viewMode, filters: mocks.workspaceFilters }
          : fallback,
      setValue: vi.fn(),
      isLoading: false,
    }));
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

  it("does not mount or fetch the persisted map without Inventory read permission", async () => {
    mocks.viewMode = "map";
    mocks.canReadInventory = false;
    mocks.mapSites.mockClear();
    mocks.listConnectionTargets.mockResolvedValue({ items: [{ id: "router-fixture", name: "router-1", targetKind: "device" }], total: 1, limit: 100, offset: 0 });
    renderWorkspace();
    expect(await screen.findByText(/Map requires read access to Inventory/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Map" })).toBeNull();
    expect(mocks.mapSites).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Cards" }));
    expect(screen.getByRole("button", { name: "Cards" })).toHaveAttribute("aria-pressed", "true");
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
