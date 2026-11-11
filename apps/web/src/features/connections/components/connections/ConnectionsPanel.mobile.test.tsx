import { useState } from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import ConnectionsPanel from "./ConnectionsPanel";
import type { PlaybookTab } from "@/features/playbooks/playbookTabs";

vi.mock("./LiveSessionViewer", () => ({
  default: ({ sessionId, isActive }: { sessionId: string; isActive: boolean }) =>
    <input aria-label={`Terminal ${sessionId}`} data-active={isActive} defaultValue="scrollback" />,
}));
vi.mock("./SessionHealthBar", () => ({ default: () => null }));
vi.mock("@/features/playbooks/PlaybookTabViewer", () => ({ default: () => <p>Playbook output</p> }));

const props = () => ({
  mobile: true,
  sessions: [{ id: "ssh-1", hostId: "host-1", hostName: "mobile-host", protocol: "ssh" as const,
    status: "connected" as const, websocketUrl: "/api/v1/ws/ssh/ssh-1", token: "fixture" }],
  activeSessionId: "ssh-1", onSelectSession: vi.fn(), onCloseSession: vi.fn(),
  onCloseAllSessions: vi.fn(), onDetachSession: vi.fn(), onReconnectSession: vi.fn(),
  onSessionStatusChange: vi.fn(), expanded: true, onToggleExpand: vi.fn(),
  dockPosition: "bottom" as const, onChangeDock: vi.fn(), panelSize: 120, onPanelResize: vi.fn(),
  playbookTabs: [], activePlaybookTabId: null, onSelectPlaybookTab: vi.fn(), onClosePlaybookTab: vi.fn(),
});
const openMenu = (name: string) => fireEvent.pointerDown(screen.getByRole("button", { name }), { button: 0, ctrlKey: false });

describe("mobile session workspace", () => {
  beforeEach(() => {
    globalThis.ResizeObserver = class { observe() {} unobserve() {} disconnect() {} };
  });

  it("uses the screen rather than the persisted desktop dock height", () => {
    const { container } = render(<ConnectionsPanel {...props()} />);
    expect(container.firstElementChild).toHaveClass("fixed");
    expect(container.firstElementChild).not.toHaveStyle({ height: "120px" });
  });

  it("ignores the persisted right dock on touch devices", () => {
    const { container } = render(<ConnectionsPanel {...props()} dockPosition="right" />);
    expect(container.firstElementChild).toHaveClass("flex-col");
    expect(container.firstElementChild).not.toHaveClass("flex-row");
  });

  it("keeps identity compact and exposes the complete session name in the switcher", () => {
    const callbacks = props();
    const hostName = "production-terminal-with-an-unabridged-hostname.example.test";
    render(<ConnectionsPanel {...callbacks} sessions={[{ ...callbacks.sessions[0]!, hostName }]} />);
    expect(screen.queryByRole("tablist")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Close session" })).not.toBeInTheDocument();
    openMenu("Switch session or playbook");
    const selected = screen.getByRole("menuitemradio", { name: new RegExp(hostName) });
    expect(selected).toHaveAttribute("aria-checked", "true");
    expect(selected).toHaveTextContent("SSH");
    fireEvent.click(selected);
    expect(callbacks.onSelectSession).toHaveBeenCalledExactlyOnceWith("ssh-1");
  });

  it("shows permitted details and close only on request through the shared tools", () => {
    const callbacks = { ...props(), onInspectSession: vi.fn() };
    const { rerender } = render(<ConnectionsPanel {...callbacks} />);
    expect(screen.queryByRole("button", { name: "Details" })).not.toBeInTheDocument();
    openMenu("Session and panel actions");
    fireEvent.click(screen.getByRole("menuitem", { name: "Details" }));
    expect(callbacks.onInspectSession).toHaveBeenCalledExactlyOnceWith(callbacks.sessions[0]);
    openMenu("Session and panel actions");
    fireEvent.click(screen.getByRole("menuitem", { name: "Close session" }));
    expect(callbacks.onCloseSession).toHaveBeenCalledExactlyOnceWith("ssh-1");
    rerender(<ConnectionsPanel {...callbacks} canInspectSession={() => false} />);
    openMenu("Session and panel actions");
    expect(screen.queryByRole("menuitem", { name: "Details" })).not.toBeInTheDocument();
  });

  it("switches sessions and playbooks without replacing retained viewers or drafts", () => {
    const callbacks = props();
    const sessions = [...callbacks.sessions, { ...callbacks.sessions[0]!, id: "ssh-2", hostName: "second-host", websocketUrl: "/api/v1/ws/ssh/ssh-2" }];
    const tab: PlaybookTab = { id: "playbook-1", groupId: "group-1", playbookName: "Inventory update",
      hostSummary: "2 hosts", phase: "running", target: { kind: "group", groupId: "group-1" }, startedAt: "2026-11-05T00:00:00Z" };
    function Fixture() {
      const [selected, select] = useState<string | null>("ssh-1");
      const [playbook, selectPlaybook] = useState<string | null>(null);
      return <ConnectionsPanel {...callbacks} sessions={sessions} activeSessionId={selected} activePlaybookTabId={playbook}
        playbookTabs={[tab]} onSelectSession={id => { select(id); selectPlaybook(null); }}
        onSelectPlaybookTab={id => { select(null); selectPlaybook(id); }} />;
    }
    render(<Fixture />);
    const terminal = screen.getByRole("textbox", { name: "Terminal ssh-1" });
    fireEvent.change(terminal, { target: { value: "kept draft" } });
    openMenu("Switch session or playbook");
    fireEvent.click(screen.getByRole("menuitemradio", { name: /second-host/ }));
    const second = screen.getByRole("textbox", { name: "Terminal ssh-2" });
    expect(second).toHaveAttribute("data-active", "true");
    expect(terminal).toHaveAttribute("data-active", "false");
    expect(terminal.closest("[inert]")).not.toBeNull();
    openMenu("Switch session or playbook");
    fireEvent.click(screen.getByRole("menuitemradio", { name: /Inventory update/ }));
    expect(screen.getByText("Playbook output")).toBeInTheDocument();
    expect(second).toHaveAttribute("data-active", "false");
    fireEvent.click(screen.getByRole("button", { name: "Close tab" }));
    expect(callbacks.onClosePlaybookTab).toHaveBeenCalledExactlyOnceWith(tab.id);
    expect(callbacks.onCloseSession).not.toHaveBeenCalled();
    openMenu("Switch session or playbook");
    fireEvent.click(screen.getByRole("menuitemradio", { name: /mobile-host/ }));
    expect(screen.getByRole("textbox", { name: "Terminal ssh-1" })).toBe(terminal);
    expect(terminal).toHaveValue("kept draft");
  });

  it("returns to targets without closing or remounting the terminal", () => {
    const callbacks = props();
    const { rerender } = render(<ConnectionsPanel {...callbacks} />);
    const terminal = screen.getByRole("textbox", { name: "Terminal ssh-1" });
    fireEvent.change(terminal, { target: { value: "kept buffer" } });
    fireEvent.click(screen.getByRole("button", { name: "Back to connections" }));
    expect(callbacks.onToggleExpand).toHaveBeenCalledOnce();
    expect(callbacks.onCloseSession).not.toHaveBeenCalled();
    rerender(<ConnectionsPanel {...callbacks} visible={false} />);
    rerender(<ConnectionsPanel {...callbacks} visible />);
    expect(screen.getByRole("textbox", { name: "Terminal ssh-1" })).toBe(terminal);
    expect(terminal).toHaveValue("kept buffer");
  });
});
