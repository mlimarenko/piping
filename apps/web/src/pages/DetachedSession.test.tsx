import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import "@/shared/i18n";
import { beforeEach, describe, expect, it, vi } from "vitest";

import DetachedSession from "./DetachedSession";
import {
  readDetachedHandoff,
  writeDetachedHandoff,
} from "@/features/connections/detachedHandoff";

const apiMocks = vi.hoisted(() => ({
  resumeSession: vi.fn(),
  openSession: vi.fn(),
  closeSession: vi.fn(),
  getConnectionLaunchOptions: vi.fn(),
}));

const viewerCalls = vi.hoisted(() => ({
  sessions: [] as Array<{ sessionId: string; protocol: string; websocketUrl: string; token: string }>,
}));

const healthCalls = vi.hoisted(() => ({
  onConnectionLost: [] as Array<(() => void) | undefined>,
}));

vi.mock("@/features/connections/api", () => ({
  resumeSession: apiMocks.resumeSession,
  openSession: apiMocks.openSession,
  closeSession: apiMocks.closeSession,
  getConnectionLaunchOptions: apiMocks.getConnectionLaunchOptions,
}));

vi.mock("@/features/connections/components/connections/LiveSessionViewer", () => ({
  default: ({
    sessionId,
    protocol,
    websocketUrl,
    token,
    onReconnect,
  }: {
    sessionId: string;
    protocol: string;
    websocketUrl: string;
    token: string;
    onReconnect?: () => void;
  }) => {
    viewerCalls.sessions.push({ sessionId, protocol, websocketUrl, token });
    return <div data-testid="live-session-viewer" data-session-id={sessionId}><button onClick={onReconnect}>Retry viewer</button></div>;
  },
}));

vi.mock("@/features/connections/components/connections/SessionHealthBar", () => ({
  default: ({ onConnectionLost }: { onConnectionLost?: () => void }) => {
    healthCalls.onConnectionLost.push(onConnectionLost);
    return (
      <button
        type="button"
        data-testid="session-health-bar"
        onClick={() => onConnectionLost?.()}
      >
        health
      </button>
    );
  },
}));

function renderDetached(sessionId: string) {
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><MemoryRouter initialEntries={[`/connections/detached/${sessionId}`]}>
      <Routes>
        <Route path="/connections/detached/:connectionId" element={<DetachedSession />} />
      </Routes>
    </MemoryRouter></QueryClientProvider>,
  );
}

describe("DetachedSession", () => {
  beforeEach(() => {
    window.localStorage.clear();
    delete window.__PIPING_DETACHED_HANDOFF__;
    apiMocks.resumeSession.mockReset();
    apiMocks.openSession.mockReset();
    apiMocks.closeSession.mockReset().mockResolvedValue(undefined);
    apiMocks.getConnectionLaunchOptions.mockReset();
    viewerCalls.sessions = [];
    healthCalls.onConnectionLost = [];
  });

  it("opens a fresh detached handoff without first resuming the session", async () => {
    writeDetachedHandoff("session-1", {
      sessionId: "session-1",
      hostId: "host-1",
      hostName: "cash-1",
      protocol: "vnc",
      websocketUrl: "/api/v1/ws/vnc/session-1",
      token: "token-1",
    });

    renderDetached("session-1");

    expect(await screen.findByTestId("live-session-viewer")).toHaveAttribute(
      "data-session-id",
      "session-1",
    );
    await waitFor(() => {
      expect(apiMocks.resumeSession).not.toHaveBeenCalled();
    });
    expect(viewerCalls.sessions.at(-1)).toMatchObject({
      sessionId: "session-1",
      protocol: "vnc",
      websocketUrl: "/api/v1/ws/vnc/session-1",
      token: "token-1",
    });
  });

  it("manual Retry fresh-launches the current saved account and replaces the route/handoff without resume", async () => {
    writeDetachedHandoff("old-retry", { sessionId: "old-retry", hostId: "host-a", hostName: "SSH A", protocol: "ssh", websocketUrl: "/ssh/old", token: "fixture" });
    apiMocks.getConnectionLaunchOptions.mockResolvedValue({ protocols: [{ protocol: "ssh", accounts: [{ entryId: "current-account", kind: "shared", label: "Operator", username: "qa", requiresInput: false }] }] });
    apiMocks.openSession.mockResolvedValue({ session: { id: "new-retry", hostId: "host-a", protocol: "ssh" }, proxy: { websocketUrl: "/ssh/new", token: "fixture" } });
    renderDetached("old-retry");
    await screen.findByTestId("live-session-viewer");
    fireEvent.click(screen.getByRole("button", { name: "Retry viewer" }));
    await waitFor(() => expect(screen.getByTestId("live-session-viewer")).toHaveAttribute("data-session-id", "new-retry"));
    expect(apiMocks.openSession).toHaveBeenCalledExactlyOnceWith({ hostId: "host-a", protocol: "ssh", entryId: "current-account" });
    expect(apiMocks.resumeSession).not.toHaveBeenCalled();
    expect(apiMocks.closeSession).toHaveBeenCalledExactlyOnceWith("old-retry");
    expect(readDetachedHandoff("old-retry")).toBeNull();
    expect(readDetachedHandoff("new-retry")?.sessionId).toBe("new-retry");
  });

  it("falls back to backend resume when the handoff is missing", async () => {
    apiMocks.resumeSession.mockResolvedValueOnce({
      session: {
        id: "session-2",
        hostId: "host-2",
        hostName: "operator-2",
        protocol: "ssh",
      },
      proxy: {
        websocketUrl: "/api/v1/ws/ssh/session-2",
        token: "token-2",
      },
    });

    renderDetached("session-2");

    expect(await screen.findByTestId("live-session-viewer")).toHaveAttribute(
      "data-session-id",
      "session-2",
    );
    expect(apiMocks.resumeSession).toHaveBeenCalledWith("session-2");
    expect(viewerCalls.sessions.at(-1)).toMatchObject({
      sessionId: "session-2",
      protocol: "ssh",
      websocketUrl: "/api/v1/ws/ssh/session-2",
      token: "token-2",
    });
  });

  it("uses the desktop-injected handoff before reading backend state", async () => {
    window.__PIPING_DETACHED_HANDOFF__ = {
      sessionId: "session-3",
      hostId: "host-3",
      hostName: "cash-3",
      protocol: "ssh",
      websocketUrl: "/api/v1/ws/ssh/session-3",
      token: "token-3",
      username: "root",
    };

    renderDetached("session-3");

    expect(await screen.findByTestId("live-session-viewer")).toHaveAttribute(
      "data-session-id",
      "session-3",
    );
    await waitFor(() => {
      expect(apiMocks.resumeSession).not.toHaveBeenCalled();
    });
    expect(window.__PIPING_DETACHED_HANDOFF__).toBeUndefined();
    expect(readDetachedHandoff("session-3")).toMatchObject({
      sessionId: "session-3",
      protocol: "ssh",
      websocketUrl: "/api/v1/ws/ssh/session-3",
      token: "token-3",
      username: "root",
    });
    expect(viewerCalls.sessions.at(-1)).toMatchObject({
      sessionId: "session-3",
      protocol: "ssh",
      websocketUrl: "/api/v1/ws/ssh/session-3",
      token: "token-3",
    });
  });

  it("shows a reconnect overlay when visual-session health reports a lost RDP route", async () => {
    writeDetachedHandoff("session-rdp", {
      sessionId: "session-rdp",
      hostId: "host-rdp",
      hostName: "spb-hv5",
      protocol: "rdp",
      websocketUrl: "/api/v1/ws/rdp/session-rdp",
      token: "token-rdp",
    });

    renderDetached("session-rdp");

    expect(await screen.findByTestId("live-session-viewer")).toBeInTheDocument();
    expect(healthCalls.onConnectionLost.at(-1)).toEqual(expect.any(Function));

    fireEvent.click(screen.getByTestId("session-health-bar"));

    expect(await screen.findByText("Connection lost")).toBeInTheDocument();
    expect(screen.getByText("Reconnect session")).toBeInTheDocument();
  });

  it("renders a cctv console from its stored handoff without touching the session API", async () => {
    writeDetachedHandoff("cctv:target-9", {
      sessionId: "cctv:target-9",
      hostId: "target-9",
      hostName: "Регистратор 1-1",
      protocol: "cctv",
      websocketUrl: "",
      token: "",
    });

    renderDetached("cctv:target-9");

    expect(await screen.findByTestId("live-session-viewer")).toBeInTheDocument();
    expect(viewerCalls.sessions.at(-1)).toMatchObject({
      sessionId: "cctv:target-9",
      protocol: "cctv",
    });
    expect(apiMocks.resumeSession).not.toHaveBeenCalled();
    // No server session exists for a client-only console, so the server-session
    // health bar must not mount and poll `cctv:<targetId>` forever.
    expect(screen.queryByTestId("session-health-bar")).not.toBeInTheDocument();
  });

  it("synthesizes a cctv handoff from the route id when storage is empty", async () => {
    renderDetached("cctv:target-42");

    expect(await screen.findByTestId("live-session-viewer")).toBeInTheDocument();
    expect(viewerCalls.sessions.at(-1)).toMatchObject({
      sessionId: "cctv:target-42",
      protocol: "cctv",
    });
    expect(apiMocks.resumeSession).not.toHaveBeenCalled();
    expect(readDetachedHandoff("cctv:target-42")).toMatchObject({
      hostId: "target-42",
      protocol: "cctv",
    });
  });
});
