import { fireEvent, render, screen, within } from "@testing-library/react";
import "@/shared/i18n";
import { beforeEach, describe, expect, it, vi } from "vitest";

import ConnectionsPanel from "./ConnectionsPanel";
import type { LiveSession } from "@/features/connections/ConnectionsWorkspace";
import type { PlaybookTab } from "@/features/playbooks/playbookTabs";
import { playbookRunGroupHref } from "@/features/playbooks/playbookNavigation";

vi.mock("@/features/connections/components/connections/LiveSessionViewer", () => ({
  default: ({ sessionId }: { sessionId: string }) => <div data-testid={`viewer-${sessionId}`} />,
}));
vi.mock("@/features/connections/components/connections/SessionHealthBar", () => ({
  default: () => <div data-testid="session-health-bar" />,
}));
vi.mock("@/features/playbooks/PlaybookTabViewer", () => ({
  default: ({ tab }: { tab: PlaybookTab }) => <div data-testid={`playbook-viewer-${tab.id}`} />,
}));

class MockResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}

const session: LiveSession = {
  id: "ssh-1",
  hostId: "host-1",
  hostName: "spb-hv5",
  protocol: "ssh",
  status: "connected",
  websocketUrl: "/api/v1/ws/ssh/ssh-1",
  token: "token",
};

function playbookTab(overrides: Partial<PlaybookTab> = {}): PlaybookTab {
  return {
    id: "group-1",
    groupId: "group-1",
    playbookName: "Update inventory",
    hostSummary: "3 hosts",
    phase: "running",
    target: { kind: "group", groupId: "group-1" },
    startedAt: "2026-06-29T00:00:00Z",
    ...overrides,
  };
}

function baseProps() {
  return {
    sessions: [session],
    activeSessionId: session.id,
    onSelectSession: vi.fn(),
    onCloseSession: vi.fn(),
    onCloseAllSessions: vi.fn(),
    onDetachSession: vi.fn(),
    onReconnectSession: vi.fn(),
    onSessionStatusChange: vi.fn(),
    expanded: true,
    onToggleExpand: vi.fn(),
    dockPosition: "bottom" as const,
    onChangeDock: vi.fn(),
    panelSize: 320,
    onPanelResize: vi.fn(),
  };
}

describe("ConnectionsPanel playbook tabs", () => {
  beforeEach(() => {
    globalThis.ResizeObserver = MockResizeObserver;
    window.ResizeObserver = MockResizeObserver;
  });

  it("renders session tabs and playbook tabs in the same strip", () => {
    render(
      <ConnectionsPanel
        {...baseProps()}
        playbookTabs={[playbookTab()]}
        activePlaybookTabId={null}
        onSelectPlaybookTab={vi.fn()}
        onClosePlaybookTab={vi.fn()}
      />,
    );
    expect(within(screen.getByRole("tablist")).getByText(/spb-hv5/)).toBeInTheDocument();
    expect(screen.getByText(/Update inventory/)).toBeInTheDocument();
  });

  it("shows the playbook viewer while preserving the hidden session viewer", () => {
    render(
      <ConnectionsPanel
        {...baseProps()}
        activeSessionId={null}
        playbookTabs={[playbookTab()]}
        activePlaybookTabId="group-1"
        onSelectPlaybookTab={vi.fn()}
        onClosePlaybookTab={vi.fn()}
      />,
    );
    expect(screen.getByTestId("playbook-viewer-group-1")).toBeInTheDocument();
    expect(screen.getByTestId(`viewer-${session.id}`).closest("[data-session-pane-viewer]")).toHaveAttribute("aria-hidden", "true");
  });

  it("selects the playbook tab on click", () => {
    const onSelectPlaybookTab = vi.fn();
    render(
      <ConnectionsPanel
        {...baseProps()}
        playbookTabs={[playbookTab()]}
        activePlaybookTabId={null}
        onSelectPlaybookTab={onSelectPlaybookTab}
        onClosePlaybookTab={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByText(/Update inventory/));
    expect(onSelectPlaybookTab).toHaveBeenCalledWith("group-1");
  });

  it("closes the playbook tab locally without touching the session close callback", () => {
    const onClosePlaybookTab = vi.fn();
    const onCloseSession = vi.fn();
    render(
      <ConnectionsPanel
        {...baseProps()}
        onCloseSession={onCloseSession}
        playbookTabs={[playbookTab()]}
        activePlaybookTabId="group-1"
        onSelectPlaybookTab={vi.fn()}
        onClosePlaybookTab={onClosePlaybookTab}
      />,
    );
    fireEvent.click(screen.getByLabelText("Close tab"));
    expect(onClosePlaybookTab).toHaveBeenCalledWith("group-1");
    expect(onCloseSession).not.toHaveBeenCalled();
  });

  it("deep-links to the run group's Runs-tab entry from the playbook tab's detach button", () => {
    const openSpy = vi.spyOn(window, "open").mockReturnValue(null);
    const tab = playbookTab();
    render(
      <ConnectionsPanel
        {...baseProps()}
        playbookTabs={[tab]}
        activePlaybookTabId="group-1"
        onSelectPlaybookTab={vi.fn()}
        onClosePlaybookTab={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByLabelText("Open runs in a new tab"));
    expect(openSpy).toHaveBeenCalledWith(playbookRunGroupHref(tab.groupId, tab.playbookName), "_blank");
    openSpy.mockRestore();
  });

  it("renders the divider only when both a session and a playbook tab are open", () => {
    const { rerender } = render(
      <ConnectionsPanel
        {...baseProps()}
        playbookTabs={[playbookTab()]}
        activePlaybookTabId={null}
        onSelectPlaybookTab={vi.fn()}
        onClosePlaybookTab={vi.fn()}
      />,
    );
    expect(screen.getByTestId("tab-kind-divider")).toBeInTheDocument();

    rerender(
      <ConnectionsPanel
        {...baseProps()}
        playbookTabs={[]}
        activePlaybookTabId={null}
        onSelectPlaybookTab={vi.fn()}
        onClosePlaybookTab={vi.fn()}
      />,
    );
    expect(screen.queryByTestId("tab-kind-divider")).toBeNull();

    rerender(
      <ConnectionsPanel
        {...baseProps()}
        sessions={[]}
        activeSessionId={null}
        playbookTabs={[playbookTab()]}
        activePlaybookTabId="group-1"
        onSelectPlaybookTab={vi.fn()}
        onClosePlaybookTab={vi.fn()}
      />,
    );
    expect(screen.queryByTestId("tab-kind-divider")).toBeNull();
  });

  it("renders with only playbook tabs open (no sessions) — the flagship post-launch/reload state", () => {
    render(
      <ConnectionsPanel
        {...baseProps()}
        sessions={[]}
        activeSessionId={null}
        playbookTabs={[playbookTab()]}
        activePlaybookTabId="group-1"
        onSelectPlaybookTab={vi.fn()}
        onClosePlaybookTab={vi.fn()}
      />,
    );
    expect(screen.getByText(/Update inventory/)).toBeInTheDocument();
    expect(screen.getByTestId("playbook-viewer-group-1")).toBeInTheDocument();
    expect(screen.queryByTestId("session-health-bar")).toBeNull();
    expect(screen.queryByTestId(`viewer-${session.id}`)).toBeNull();
    // The session-only close-all-sessions control must not show with zero sessions.
    expect(screen.queryByLabelText("Close all sessions")).toBeNull();
  });

  it("only shows the playbook when both active identities are set, preserving session mounts", () => {
    // ConnectionsWorkspace is responsible for keeping activeSessionId and
    // activePlaybookTabId mutually exclusive; this test locks in the
    // component-level backstop so a wiring regression upstream can never
    // show the session grid alongside the playbook — the playbook tab wins.
    render(
      <ConnectionsPanel
        {...baseProps()}
        activeSessionId={session.id}
        playbookTabs={[playbookTab()]}
        activePlaybookTabId="group-1"
        onSelectPlaybookTab={vi.fn()}
        onClosePlaybookTab={vi.fn()}
      />,
    );
    expect(screen.getByTestId("playbook-viewer-group-1")).toBeInTheDocument();
    expect(screen.getByTestId(`viewer-${session.id}`).closest("[data-session-pane-viewer]")).toHaveAttribute("aria-hidden", "true");
    expect(screen.queryByTestId("session-health-bar")).toBeNull();
    expect(screen.getByTestId(`viewer-${session.id}`).closest("[inert]")).toBeInTheDocument();
    // The session-specific toolbar (reconnect button) must not render either —
    // it would otherwise control a session that isn't the visible viewer.
    expect(screen.queryByLabelText("Reconnect session")).toBeNull();
  });
});
