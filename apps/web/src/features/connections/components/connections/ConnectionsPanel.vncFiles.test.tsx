import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useState } from "react";
import "@/shared/i18n";
import { beforeEach, describe, expect, it, vi } from "vitest";

import ConnectionsPanel from "./ConnectionsPanel";
import type { LiveSession } from "@/features/connections/ConnectionsWorkspace";

const fileBrowserSessions = vi.hoisted(() => ({
  ssh: [] as Array<{ id: string; protocol: LiveSession["protocol"] }>,
}));

vi.mock("@/features/connections/components/connections/LiveSessionViewer", () => ({
  default: ({ sessionId }: { sessionId: string }) => (
    <div data-testid={`viewer-${sessionId}`} />
  ),
}));

vi.mock("@/features/connections/components/connections/SessionHealthBar", () => ({
  default: () => <div data-testid="session-health-bar" />,
}));

vi.mock("@/features/connections/components/connections/SshFileBrowserPanel", () => ({
  default: ({ session }: { session: LiveSession }) => {
    fileBrowserSessions.ssh.push({ id: session.id, protocol: session.protocol });
    return (
      <div
        data-testid="ssh-file-browser"
        data-session-id={session.id}
        data-protocol={session.protocol}
      />
    );
  },
}));

vi.mock("@/features/connections/components/connections/RdpFileBrowserPanel", () => ({
  default: () => <div data-testid="rdp-file-browser-panel" />,
}));

class MockResizeObserver {
  static instances: MockResizeObserver[] = [];
  element: Element | null = null;
  constructor(readonly callback: ResizeObserverCallback) { MockResizeObserver.instances.push(this); }
  observe(element: Element) { this.element = element; }
  unobserve() {}
  disconnect = vi.fn();
}

const sessions: LiveSession[] = [
  {
    id: "vnc-1",
    hostId: "host-1",
    hostName: "ares",
    protocol: "vnc",
    status: "connected",
    websocketUrl: "/api/v1/ws/vnc/vnc-1",
    token: "token",
    username: "alice",
  },
  {
    id: "ssh-1",
    hostId: "host-1",
    hostName: "ares",
    protocol: "ssh",
    status: "connected",
    websocketUrl: "/api/v1/ws/ssh/ssh-1",
    token: "token",
    username: "bob",
  },
  {
    id: "ssh-alice",
    hostId: "host-1",
    hostName: "ares",
    protocol: "ssh",
    status: "connected",
    websocketUrl: "/api/v1/ws/ssh/ssh-alice",
    token: "token",
    username: "alice",
  },
  {
    id: "ssh-other",
    hostId: "host-2",
    hostName: "other",
    protocol: "ssh",
    status: "connected",
    websocketUrl: "/api/v1/ws/ssh/ssh-other",
    token: "token",
  },
];

function Harness() {
  const [activeSessionId, setActiveSessionId] = useState<string | null>("vnc-1");
  return (
    <ConnectionsPanel
      sessions={sessions}
      activeSessionId={activeSessionId}
      onSelectSession={setActiveSessionId}
      onCloseSession={() => {}}
      onCloseAllSessions={() => {}}
      onDetachSession={() => {}}
      onReconnectSession={() => {}}
      onSessionStatusChange={() => {}}
      playbookTabs={[]}
      activePlaybookTabId={null}
      onSelectPlaybookTab={() => {}}
      onClosePlaybookTab={() => {}}
      expanded
      onToggleExpand={() => {}}
      dockPosition="bottom"
      onChangeDock={() => {}}
      panelSize={520}
      onPanelResize={() => {}}
    />
  );
}

describe("ConnectionsPanel VNC Files (SSH)", () => {
  beforeEach(() => {
    MockResizeObserver.instances = [];
    globalThis.ResizeObserver = MockResizeObserver;
    window.ResizeObserver = MockResizeObserver;
    fileBrowserSessions.ssh = [];
  });

  it("keeps grid measurement active after opening a session file browser", async () => {
    let width = 640;
    const measure = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(() => ({ x: 0, y: 0, top: 0, left: 0, bottom: 520, right: width, width, height: 520, toJSON: () => ({}) }));
    try {
      render(<Harness />);
      const observer = MockResizeObserver.instances.find(instance => instance.element?.hasAttribute("data-session-pane-grid"))!;
      fireEvent.pointerDown(screen.getByRole("button", { name: "Pane 1 actions" }), { button: 0, ctrlKey: false });
      fireEvent.click(screen.getByRole("menuitem", { name: "Split pane 1 side-by-side" }));
      fireEvent.focus(screen.getByRole("combobox", { name: "Session in pane 1" }));
      fireEvent.pointerDown(screen.getByRole("button", { name: "Session tools" }), { button: 0, ctrlKey: false });
      fireEvent.click(screen.getByText("Files (SSH)").closest('[role="menuitem"]')!);
      await screen.findByTestId("ssh-file-browser");
      expect(observer.disconnect).not.toHaveBeenCalled();
      // Files selects the matching SSH session as a standalone canvas, not as
      // a replacement for the VNC group pane. Measure its full-canvas minimum.
      width = 200;
      act(() => observer.callback([], observer));
      fireEvent.pointerDown(screen.getByRole("button", { name: "Pane 1 actions" }), { button: 0, ctrlKey: false });
      expect(screen.getByRole("menuitem", { name: "Split pane 1 side-by-side" })).toHaveAttribute("aria-disabled", "true");
    } finally { measure.mockRestore(); }
  });

  it("switches from VNC to the matching same-host SSH session before opening files", async () => {
    render(<Harness />);

    fireEvent.pointerDown(screen.getByRole("button", { name: "Session tools" }), {
      button: 0,
      ctrlKey: false,
    });
    fireEvent.click(screen.getByText("Files (SSH)").closest('[role="menuitem"]')!);

    await waitFor(() => {
      expect(screen.getByTestId("ssh-file-browser")).toHaveAttribute("data-session-id", "ssh-alice");
    });

    expect(screen.getByTestId("ssh-file-browser")).toHaveAttribute("data-protocol", "ssh");
    expect(fileBrowserSessions.ssh).toEqual(
      expect.arrayContaining([{ id: "ssh-alice", protocol: "ssh" }]),
    );
    expect(fileBrowserSessions.ssh).not.toEqual(
      expect.arrayContaining([{ id: "vnc-1", protocol: "vnc" }]),
    );
  });
});
