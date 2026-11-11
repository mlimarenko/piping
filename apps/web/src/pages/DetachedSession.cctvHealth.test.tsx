import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router";
import "@/shared/i18n";
import { beforeEach, describe, expect, it, vi } from "vitest";

import DetachedSession from "./DetachedSession";
import { writeDetachedHandoff } from "@/features/connections/detachedHandoff";

// The real SessionHealthBar is intentionally left unmocked here: this suite
// proves the detached page never issues `GET sessions/{id}/health` for a
// client-only `cctv:<targetId>` console, which the component-level mocks in
// DetachedSession.test.tsx cannot observe.
const apiMocks = vi.hoisted(() => ({
  resumeSession: vi.fn(),
  closeSession: vi.fn(),
  getSessionHealth: vi.fn(),
}));

vi.mock("@/features/connections/api", () => ({
  resumeSession: apiMocks.resumeSession,
  closeSession: apiMocks.closeSession,
  getSessionHealth: apiMocks.getSessionHealth,
}));

vi.mock("@/features/connections/components/connections/LiveSessionViewer", () => ({
  default: () => <div data-testid="live-session-viewer" />,
}));

function renderDetached(sessionId: string) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <MemoryRouter initialEntries={[`/connections/detached/${sessionId}`]}>
        <Routes>
          <Route path="/connections/detached/:connectionId" element={<DetachedSession />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("DetachedSession session-health polling", () => {
  beforeEach(() => {
    window.localStorage.clear();
    delete window.__PIPING_DETACHED_HANDOFF__;
    apiMocks.resumeSession.mockReset();
    apiMocks.closeSession.mockReset();
    apiMocks.getSessionHealth.mockReset();
    // Never settle: if the bar mounts, it would poll this pseudo-id forever.
    apiMocks.getSessionHealth.mockImplementation(() => new Promise(() => {}));
  });

  it("never asks the session API for health of a client-only cctv console", async () => {
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
    expect(screen.queryByTestId("session-health-bar")).not.toBeInTheDocument();
    await waitFor(() => {
      expect(apiMocks.getSessionHealth).not.toHaveBeenCalled();
    });
  });

  it("still polls server session health for a server-backed protocol", async () => {
    writeDetachedHandoff("session-ssh", {
      sessionId: "session-ssh",
      hostId: "host-ssh",
      hostName: "cash-1",
      protocol: "ssh",
      websocketUrl: "/api/v1/ws/ssh/session-ssh",
      token: "token-ssh",
    });

    renderDetached("session-ssh");

    expect(await screen.findByTestId("session-health-bar")).toBeInTheDocument();
    await waitFor(() => {
      expect(apiMocks.getSessionHealth).toHaveBeenCalledWith("session-ssh");
    });
  });
});
