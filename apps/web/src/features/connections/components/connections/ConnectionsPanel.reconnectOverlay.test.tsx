import { fireEvent, render, screen } from "@testing-library/react";
import "@/shared/i18n";
import { beforeEach, describe, expect, it, vi } from "vitest";

import ConnectionsPanel from "./ConnectionsPanel";
import type { LiveSession } from "@/features/connections/ConnectionsWorkspace";

// The lazy viewer and the health bar both reach for browser/network APIs that are
// irrelevant to the recovery-overlay logic under test, so stub them out.
vi.mock("@/features/connections/components/connections/LiveSessionViewer", () => ({
  default: ({ sessionId }: { sessionId: string }) => <div data-testid={`viewer-${sessionId}`} />,
}));
vi.mock("@/features/connections/components/connections/SessionHealthBar", () => ({
  default: () => <div data-testid="session-health-bar" />,
}));

class MockResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}

function baseSession(overrides: Partial<LiveSession>): LiveSession {
  return {
    id: "s",
    hostId: "host-1",
    hostName: "spb-hv5",
    protocol: "rdp",
    status: "connected",
    // A live session keeps its url/token after a drop (only `idle` clears them),
    // so `isSessionWsConnectable` stays true and the "restoring" spinner does not
    // shadow the recovery overlay.
    websocketUrl: "/api/v1/ws/rdp/s",
    token: "token",
    username: "alice",
    ...overrides,
  };
}

function renderPanel(session: LiveSession, onReconnectSession = vi.fn()) {
  render(
    <ConnectionsPanel
      sessions={[session]}
      activeSessionId={session.id}
      onSelectSession={() => {}}
      onCloseSession={() => {}}
      onCloseAllSessions={() => {}}
      onDetachSession={() => {}}
      onReconnectSession={onReconnectSession}
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
    />,
  );
  return { onReconnectSession };
}

describe("ConnectionsPanel recovery overlay", () => {
  beforeEach(() => {
    globalThis.ResizeObserver = MockResizeObserver;
    window.ResizeObserver = MockResizeObserver;
  });

  it("shows a Connection lost overlay with a reconnect button for a failed RDP session", () => {
    // `failed` is what the half-open health-driven detector flips to.
    const { onReconnectSession } = renderPanel(baseSession({ id: "rdp-1", protocol: "rdp", status: "failed" }));

    expect(screen.getByText("Connection lost")).toBeInTheDocument();
    // The overlay button carries the visible label; the toolbar button is icon-only.
    const reconnect = screen.getByText("Reconnect session");
    fireEvent.click(reconnect);
    expect(onReconnectSession).toHaveBeenCalledTimes(1);
    expect(onReconnectSession.mock.calls[0]![0]).toMatchObject({ id: "rdp-1" });
  });

  it("shows the overlay for a failed VNC session too", () => {
    renderPanel(baseSession({ id: "vnc-1", protocol: "vnc", status: "failed", websocketUrl: "/api/v1/ws/vnc/vnc-1" }));
    expect(screen.getByText("Connection lost")).toBeInTheDocument();
  });

  it("labels a clean RDP disconnect as Session ended but still offers reconnect", () => {
    // A clean WebSocket close (code 1000) maps to `disconnected`; the recovery
    // overlay still appears so the operator can reconnect, but it must not cry
    // "Connection lost" for an orderly end.
    renderPanel(baseSession({ id: "rdp-clean", protocol: "rdp", status: "disconnected" }));
    expect(screen.getByText("Session ended")).toBeInTheDocument();
    expect(screen.getByText("Reconnect session")).toBeInTheDocument();
    expect(screen.queryByText("Connection lost")).toBeNull();
  });

  it("does not show the panel overlay for a dropped SSH session (it has its own in-viewer overlay)", () => {
    renderPanel(baseSession({ id: "ssh-1", protocol: "ssh", status: "failed", websocketUrl: "/api/v1/ws/ssh/ssh-1" }));
    expect(screen.queryByText("Connection lost")).toBeNull();
    expect(screen.queryByText("Session ended")).toBeNull();
    // No visible-text reconnect button in the canvas area (only the icon-only toolbar control).
    expect(screen.queryByText("Reconnect session")).toBeNull();
  });

  it("keeps the Session ended overlay for a restored idle session of any protocol", () => {
    renderPanel(baseSession({ id: "ssh-idle", protocol: "ssh", status: "idle", websocketUrl: "", token: "" }));
    expect(screen.getByText("Session ended")).toBeInTheDocument();
    expect(screen.getByText("Reconnect session")).toBeInTheDocument();
  });

  it("does not show a recovery overlay while the session is healthy", () => {
    renderPanel(baseSession({ id: "rdp-ok", protocol: "rdp", status: "connected" }));
    expect(screen.queryByText("Connection lost")).toBeNull();
    expect(screen.queryByText("Session ended")).toBeNull();
  });
});
