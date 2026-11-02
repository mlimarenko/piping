import { useEffect, useState } from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import ConnectionsPanel from "./ConnectionsPanel";
import type { LiveSession } from "../../ConnectionsWorkspace";
import { defaultSessionPaneLayout, splitSessionPane, placePaneSession, normalizeSessionPaneLayout, type SessionPaneLayout } from "./sessionPaneLayout";

const { mounts, unmounts } = vi.hoisted(() => ({ mounts: vi.fn(), unmounts: vi.fn() }));
vi.mock("./LiveSessionViewer", () => ({
  default: function MockSessionViewer({ sessionId, protocol, isActive, isVisible }: { sessionId: string; protocol: string; isActive: boolean; isVisible: boolean }) {
    useEffect(() => { mounts(sessionId); return () => unmounts(sessionId); }, [sessionId]);
    return <input data-testid={`viewer-${sessionId}`} data-active={String(isActive)} data-visible={String(isVisible)} aria-label={`${protocol} fixture`} defaultValue={`buffer-${sessionId}`} />;
  },
}));
vi.mock("./SessionHealthBar", () => ({ default: () => null }));
vi.mock("@/features/playbooks/PlaybookTabViewer", () => ({ default: () => <p>Playbook fixture</p> }));

const sessions: LiveSession[] = (["ssh", "rdp", "vnc", "cctv"] as const).map(protocol => ({
  id: protocol, hostId: `host-${protocol}`, hostName: `${protocol}-host`, protocol,
  status: "connected", websocketUrl: `/api/v1/ws/${protocol}/${protocol}`, token: "fixture",
}));
function props() {
  return { sessions, activeSessionId: "ssh", onSelectSession: vi.fn(), onCloseSession: vi.fn(), onCloseAllSessions: vi.fn(),
    onDetachSession: vi.fn(), onReconnectSession: vi.fn(), onSessionStatusChange: vi.fn(), expanded: true, onToggleExpand: vi.fn(),
    dockPosition: "bottom" as const, panelSize: 500, onPanelResize: vi.fn(), onChangeDock: vi.fn(),
    playbookTabs: [], activePlaybookTabId: null, onSelectPlaybookTab: vi.fn(), onClosePlaybookTab: vi.fn() };
}
function StatefulPanel({ mobile = false }: { mobile?: boolean }) {
  const [selected, setSelected] = useState<string | null>("ssh");
  return <ConnectionsPanel {...props()} mobile={mobile} activeSessionId={selected} onSelectSession={setSelected} />;
}
const choose = (number: number, id: string) => fireEvent.change(screen.getByRole("combobox", { name: `Session in pane ${number}` }), { target: { value: id } });
const openActions = (number: number) => fireEvent.pointerDown(screen.getByRole("button", { name: `Pane ${number} actions` }), { button: 0, ctrlKey: false });
const split = (number: number, orientation: "side-by-side" | "stacked") => {
  openActions(number);
  fireEvent.click(screen.getByRole("menuitem", { name: `Split pane ${number} ${orientation}` }));
};

describe("split connection panel", () => {
  it("blocks standalone splitting with an accessible reason without changing the saved group", () => {
    let saved: SessionPaneLayout = { ...placePaneSession(splitSessionPane(defaultSessionPaneLayout("ssh"), "horizontal"), "pane-2", "rdp") };
    if (saved.root.kind === "split") saved = { ...saved, root: { ...saved.root, ratio: 0.4 } };
    const original = saved;
    const writes = vi.fn();
    function GroupFixture() {
      const [active, select] = useState<string | null>("vnc");
      return <ConnectionsPanel {...props()} activeSessionId={active} onSelectSession={select} paneLayout={saved} onPaneLayoutChange={writes} />;
    }
    render(<GroupFixture />);
    const actions = screen.getByRole("button", { name: "Pane 1 actions" });
    expect(actions).toHaveAccessibleDescription(/To change the layout, return to Work group/);
    openActions(1);
    const blocked = screen.getByRole("menuitem", { name: "Split pane 1 side-by-side" });
    expect(blocked).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByRole("menuitem", { name: "Split pane 1 stacked" })).toHaveAttribute("aria-disabled", "true");
    fireEvent.click(blocked);
    expect(writes).not.toHaveBeenCalled();
    fireEvent.keyDown(screen.getByRole("menu"), { key: "Escape" });
    fireEvent.click(screen.getByRole("tab", { name: "Work group · 2" }));
    expect(screen.getByRole("combobox", { name: "Session in pane 1" })).toHaveValue("ssh");
    expect(screen.getByRole("combobox", { name: "Session in pane 2" })).toHaveValue("rdp");
    expect(screen.getByRole("separator", { name: "Resize session panes" })).toHaveAttribute("aria-valuenow", "40");
    openActions(1);
    expect(screen.getByRole("menuitem", { name: "Split pane 1 side-by-side" })).not.toHaveAttribute("aria-disabled", "true");
    expect(saved).toBe(original);
  });
  it("persists group-member rail focus and maximize before standalone navigation and rehydration", () => {
    let saved: SessionPaneLayout = { ...placePaneSession(splitSessionPane(defaultSessionPaneLayout("ssh"), "horizontal"), "pane-2", "rdp"), maximizedPaneId: "pane-2" };
    const writes = vi.fn();
    function PreferenceFixture({ initialActive = "rdp", initialExpanded = false }: { initialActive?: string; initialExpanded?: boolean }) {
      const [active, select] = useState<string | null>(initialActive);
      const [expanded, expand] = useState(initialExpanded);
      const [preference, persist] = useState(saved);
      return <ConnectionsPanel {...props()} dockPosition="right" expanded={expanded} onToggleExpand={() => expand(value => !value)}
        activeSessionId={active} onSelectSession={select} paneLayout={normalizeSessionPaneLayout(preference)}
        onPaneLayoutChange={next => { writes(next); saved = next; persist(next); }} />;
    }
    const { unmount } = render(<PreferenceFixture />);
    fireEvent.click(screen.getAllByRole("tab")[0]!);
    expect(screen.getByTestId("viewer-ssh")).toHaveAttribute("data-active", "true");
    fireEvent.click(screen.getByRole("tab", { name: /VNC.*vnc-host/ }));
    expect(saved.focusedPaneId).toBe("pane-1");
    expect(saved.maximizedPaneId).toBe("pane-1");
    expect(saved.root).toMatchObject({ first: { sessionId: "ssh" }, second: { sessionId: "rdp" } });
    expect(writes).toHaveBeenCalledTimes(1);
    unmount();
    render(<PreferenceFixture initialActive="vnc" initialExpanded />);
    expect(screen.getByTestId("viewer-vnc")).toHaveAttribute("data-active", "true");
    fireEvent.click(screen.getByRole("tab", { name: "Work group · 2" }));
    expect(screen.getByTestId("viewer-ssh")).toHaveAttribute("data-active", "true");
    expect(screen.getByTestId("viewer-rdp")).toHaveAttribute("data-visible", "false");
    expect(writes).toHaveBeenCalledTimes(1);
  });
  it("resizes the right-docked panel with the keyboard without remounting sessions", () => {
    function ResizablePanel() {
      const [size, setSize] = useState(900);
      return <ConnectionsPanel {...props()} dockPosition="right" panelSize={size} panelMaxSize={1200} onPanelResize={setSize} />;
    }
    render(<ResizablePanel />);
    const viewer = screen.getByTestId("viewer-ssh");
    fireEvent.change(viewer, { target: { value: "Retained terminal buffer" } });
    const resize = screen.getByRole("slider", { name: "Session panel size" });
    fireEvent.keyDown(resize, { key: "Home" });
    expect(resize).toHaveAttribute("aria-valuenow", "320");
    fireEvent.keyDown(resize, { key: "ArrowLeft" });
    expect(resize).toHaveAttribute("aria-valuenow", "340");
    expect(screen.getByTestId("viewer-ssh")).toBe(viewer);
    expect(viewer).toHaveValue("Retained terminal buffer");
    expect(unmounts).not.toHaveBeenCalled();
  });
  beforeEach(() => {
    mounts.mockClear(); unmounts.mockClear();
    globalThis.ResizeObserver = class { observe() {} unobserve() {} disconnect() {} };
  });

  it("shows a launched session alone without filling a persisted empty split", () => {
    const persisted = splitSessionPane(defaultSessionPaneLayout("ended-session"), "vertical");
    const persist = vi.fn();
    render(<ConnectionsPanel {...props()} sessions={[sessions[0]!]} paneLayout={persisted} onPaneLayoutChange={persist} />);
    expect(screen.getByTestId("viewer-ssh")).toHaveAttribute("data-visible", "true");
    expect(screen.getByTestId("viewer-ssh")).toHaveAttribute("data-active", "true");
    expect(screen.queryByRole("combobox", { name: "Session in pane 2" })).toBeNull();
    expect(persist).not.toHaveBeenCalled();
  });

  it("persists an externally selected session despite freshly normalized preference objects", () => {
    function PreferenceFixture() {
      const [active, setActive] = useState<string | null>("ssh");
      const [, refresh] = useState(0);
      const [persisted, persist] = useState(() => placePaneSession(splitSessionPane(defaultSessionPaneLayout("ssh"), "horizontal"), "pane-2", "rdp"));
      return <><button onClick={() => setActive("vnc")}>Launch VNC</button><button onClick={() => refresh(n => n + 1)}>Refresh parent</button>
        <ConnectionsPanel {...props()} activeSessionId={active} onSelectSession={setActive} paneLayout={normalizeSessionPaneLayout(persisted)} onPaneLayoutChange={persist} /></>;
    }
    render(<PreferenceFixture />);
    fireEvent.click(screen.getByRole("button", { name: "Launch VNC" }));
    expect(screen.getByTestId("viewer-vnc")).toHaveAttribute("data-active", "true");
    fireEvent.click(screen.getByRole("button", { name: "Refresh parent" }));
    expect(screen.getByTestId("viewer-vnc")).toHaveAttribute("data-active", "true");
    expect(screen.getByTestId("viewer-rdp")).toHaveAttribute("data-visible", "false");
    fireEvent.click(screen.getByRole("tab", { name: "Work group · 2" }));
    expect(screen.getByRole("combobox", { name: "Session in pane 1" })).toHaveValue("ssh");
    expect(screen.getByRole("combobox", { name: "Session in pane 2" })).toHaveValue("rdp");
    expect(screen.getByTestId("viewer-rdp")).toHaveAttribute("data-active", "true");
    expect(screen.getByTestId("viewer-vnc")).toHaveAttribute("data-visible", "false");
  });

  it("keeps all mixed-protocol viewer nodes and buffers through split, resize, swap, maximize and removal", () => {
    render(<StatefulPanel />);
    const nodes = sessions.map(s => screen.getByTestId(`viewer-${s.id}`));
    fireEvent.change(nodes[0]!, { target: { value: "draft survives" } });
    split(1, "side-by-side");
    choose(2, "rdp");
    split(2, "stacked");
    choose(3, "vnc");
    split(3, "side-by-side");
    choose(4, "cctv");
    for (const node of nodes) expect(node).toHaveAttribute("data-visible", "true");
    expect(nodes.filter(node => node.getAttribute("data-active") === "true")).toHaveLength(1);
    openActions(4);
    expect(screen.getByRole("menuitem", { name: "Split pane 4 stacked" })).toHaveAttribute("aria-disabled", "true");
    fireEvent.keyDown(screen.getByRole("menu"), { key: "Escape" });
    const separator = screen.getAllByRole("separator").find(node => node.getAttribute("aria-label") === "Resize session panes")!;
    fireEvent.keyDown(separator, { key: "End" });
    expect(separator).toHaveAttribute("aria-valuenow", "80");
    choose(4, "ssh");
    expect(screen.getByTestId("viewer-ssh")).toHaveAttribute("data-active", "true");
    fireEvent.click(screen.getAllByRole("button", { name: "Maximize pane" })[3]!);
    expect(nodes.filter(node => node.getAttribute("data-visible") === "true")).toHaveLength(1);
    fireEvent.click(screen.getByRole("button", { name: "Restore panes" }));
    openActions(4);
    fireEvent.click(screen.getByRole("menuitem", { name: "Remove pane (keep session)" }));
    sessions.forEach((session, index) => expect(screen.getByTestId(`viewer-${session.id}`)).toBe(nodes[index]));
    expect(nodes[0]).toHaveValue("draft survives");
    expect(mounts).toHaveBeenCalledTimes(4);
    expect(unmounts).not.toHaveBeenCalled();
  });

  it("preserves mixed viewer identity and focused pane across desktop, phone and desktop", () => {
    let layout = splitSessionPane(defaultSessionPaneLayout("ssh"), "horizontal");
    layout = placePaneSession(layout, "pane-2", "vnc");
    layout = splitSessionPane(layout, "vertical");
    layout = placePaneSession(layout, "pane-3", "cctv");
    const callbacks = { ...props(), paneLayout: layout, activeSessionId: "cctv", onPaneLayoutChange: vi.fn() };
    const { rerender } = render(<ConnectionsPanel {...callbacks} />);
    const nodes = sessions.map(s => screen.getByTestId(`viewer-${s.id}`));
    const separators = screen.getAllByRole("separator", { name: "Resize session panes" });
    expect(separators.map(node => node.getAttribute("aria-orientation"))).toEqual(["vertical", "horizontal"]);
    rerender(<ConnectionsPanel {...callbacks} mobile />);
    expect(screen.queryByRole("separator", { name: "Resize session panes" })).toBeNull();
    expect(callbacks.onPaneLayoutChange).not.toHaveBeenCalled();
    openActions(3);
    expect(screen.queryByRole("menuitem", { name: "Split pane 3 side-by-side" })).toBeNull();
    expect(screen.getByRole("menuitem", { name: "Split pane 3 stacked" })).not.toHaveAttribute("aria-disabled", "true");
    fireEvent.keyDown(screen.getByRole("menu"), { key: "Escape" });
    const persistedAfterMenu = callbacks.onPaneLayoutChange.mock.calls.length;
    expect(nodes[3]).toHaveAttribute("data-active", "true");
    rerender(<ConnectionsPanel {...callbacks} />);
    expect(screen.getAllByRole("separator", { name: "Resize session panes" }).map(node => node.getAttribute("aria-valuenow"))).toEqual(["50", "50"]);
    sessions.forEach((session, index) => expect(screen.getByTestId(`viewer-${session.id}`)).toBe(nodes[index]));
    expect(mounts).toHaveBeenCalledTimes(4);
    expect(unmounts).not.toHaveBeenCalled();
    expect(callbacks.onPaneLayoutChange).toHaveBeenCalledTimes(persistedAfterMenu);
  });

  it("supports explicit mobile stacked panes and back without ending any session", () => {
    const callbacks = props();
    const { rerender } = render(<ConnectionsPanel {...callbacks} mobile />);
    split(1, "stacked");
    choose(2, "rdp");
    const nodes = sessions.map(s => screen.getByTestId(`viewer-${s.id}`));
    expect(nodes[0]).toHaveAttribute("data-visible", "true");
    expect(nodes[1]).toHaveAttribute("data-visible", "true");
    fireEvent.click(screen.getByRole("button", { name: "Back to connections" }));
    rerender(<ConnectionsPanel {...callbacks} mobile visible={false} />);
    expect(nodes.every(node => node.getAttribute("data-active") === "false")).toBe(true);
    rerender(<ConnectionsPanel {...callbacks} mobile />);
    sessions.forEach((session, index) => expect(screen.getByTestId(`viewer-${session.id}`)).toBe(nodes[index]));
    expect(callbacks.onCloseSession).not.toHaveBeenCalled();
    expect(unmounts).not.toHaveBeenCalled();
  });

  it("labels the split as a group without changing its name with pane focus", () => {
    render(<StatefulPanel />);
    split(1, "stacked");
    choose(2, "rdp");
    expect(screen.getByRole("tab", { name: "Work group · 2" })).toBeInTheDocument();
    fireEvent.focus(screen.getByRole("combobox", { name: "Session in pane 1" }));
    expect(screen.getByRole("tab", { name: "Work group · 2" })).toHaveAttribute("aria-selected", "true");
    expect(screen.queryByRole("toolbar", { name: "Session layout" })).not.toBeInTheDocument();
    openActions(1);
    expect(screen.getByRole("menuitem", { name: "Split pane 1 side-by-side" })).toBeInTheDocument();
  });

  it("keeps mixed viewer identity on playbook selection and focuses a different pane without broadcasting input", () => {
    const callbacks = props();
    const { rerender } = render(<ConnectionsPanel {...callbacks} />);
    split(1, "stacked");
    choose(2, "rdp");
    const ssh = screen.getByTestId("viewer-ssh"), rdp = screen.getByTestId("viewer-rdp");
    expect(rdp).toHaveAttribute("data-active", "true");
    expect(ssh.closest("[inert]")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Focus SSH · ssh-host" }));
    expect(ssh).toHaveAttribute("data-active", "true");
    expect(rdp).toHaveAttribute("data-active", "false");
    const tab = { id: "group", groupId: "group", playbookName: "Run", hostSummary: "host", phase: "running" as const, target: { kind: "group" as const, groupId: "group" }, startedAt: "2026-10-02T00:00:00Z" };
    rerender(<ConnectionsPanel {...callbacks} activeSessionId={null} playbookTabs={[tab]} activePlaybookTabId="group" />);
    expect(screen.getByText("Playbook fixture")).toBeInTheDocument();
    expect(ssh).toHaveAttribute("data-visible", "false");
    expect(rdp).toHaveAttribute("data-visible", "false");
    rerender(<ConnectionsPanel {...callbacks} />);
    expect(screen.getByTestId("viewer-ssh")).toBe(ssh);
    expect(screen.getByTestId("viewer-rdp")).toBe(rdp);
    expect(unmounts).not.toHaveBeenCalled();
  });

  it("removes pane separately from closing session, and reconciles multi-ID detach", () => {
    const callbacks = props();
    const { rerender } = render(<ConnectionsPanel {...callbacks} />);
    split(1, "stacked");
    choose(2, "rdp");
    openActions(2);
    fireEvent.click(screen.getByRole("menuitem", { name: "Remove pane (keep session)" }));
    expect(callbacks.onCloseSession).not.toHaveBeenCalled();
    expect(screen.getByTestId("viewer-rdp")).toBeInTheDocument();
    rerender(<ConnectionsPanel {...callbacks} sessions={sessions.slice(2)} activeSessionId="vnc" />);
    expect(screen.queryByTestId("viewer-ssh")).not.toBeInTheDocument();
    expect(screen.queryByTestId("viewer-rdp")).not.toBeInTheDocument();
    expect(screen.getByTestId("viewer-vnc")).toHaveAttribute("data-active", "true");
  });

  it("shows protocol names alongside icons, not indistinguishable desktop glyphs", () => {
    render(<StatefulPanel />);
    for (const label of ["SSH", "RDP", "VNC", "CCTV"]) expect(screen.getByText(label)).toBeInTheDocument();
    const rdp = screen.getByText("RDP").querySelector("svg"), vnc = screen.getByText("VNC").querySelector("svg");
    expect(rdp?.getAttribute("class")).not.toBe(vnc?.getAttribute("class"));
  });
});
