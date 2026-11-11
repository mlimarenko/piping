import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import "@/shared/i18n";
import { AllPermissions } from "@/test/permissions";
import type { ActiveSession } from "@/shared/api/types";
import SessionDetailPanel from "./SessionDetailPanel";

const writeText = vi.fn().mockResolvedValue(undefined);
beforeEach(() => {
  vi.clearAllMocks();
  Object.defineProperty(navigator, "clipboard", {value: {writeText}, configurable: true});
});

function renderPanel(observeReason?: string) {
  const session = {id: "session-full-id", hostId: "target-full-id", userId: "user-full-id", protocol: "ssh", status: "active",
    connectedAt: "2026-10-05T00:00:00Z", lastActivity: "2026-10-05T00:00:00Z"} as unknown as ActiveSession;
  return render(
    <AllPermissions>
      <SessionDetailPanel
        session={session}
        onDisconnect={() => {}}
        onTerminate={() => {}}
        observeReason={observeReason}
      />
    </AllPermissions>,
  );
}

it("explains a denied observation grant without exposing the backend reason code", () => {
  renderPanel("permission_denied");
  expect(screen.getByText("You do not have permission to observe this session.")).toBeInTheDocument();
  expect(screen.queryByText(/permission_denied/)).not.toBeInTheDocument();
});

it("explains that an ended session can no longer be observed", () => {
  renderPanel("session_ended");
  expect(screen.getByText("This session has ended and can no longer be observed.")).toBeInTheDocument();
});

it("explains when the session runtime is unavailable", () => {
  renderPanel("runtime_unavailable");
  expect(screen.getByText("The session runtime is unavailable, so observation cannot start.")).toBeInTheDocument();
});

it("falls back to the generic label for an unknown reason and when none is given", () => {
  const { unmount } = renderPanel("future_unknown_reason");
  expect(screen.getByText("Observation is unavailable for this session.")).toBeInTheDocument();
  expect(screen.queryByText(/future_unknown_reason/)).not.toBeInTheDocument();

  unmount();
  renderPanel();
  expect(screen.getByText("Observation is unavailable for this session.")).toBeInTheDocument();
});

it("copies full identifiers while rendering the selected session and routing lifecycle actions by session id", () => {
  const session = {id: "session-full-id", hostId: "target-full-id", userId: "user-full-id", protocol: "ssh", status: "active",
    connectedAt: "2026-10-05T00:00:00Z", lastActivity: "2026-10-05T00:00:00Z"} as unknown as ActiveSession;
  const onDisconnect = vi.fn();
  const onTerminate = vi.fn();
  render(<AllPermissions><SessionDetailPanel session={session} onDisconnect={onDisconnect} onTerminate={onTerminate} /></AllPermissions>);
  for (const [label, id] of [["Copy Session ID", session.id], ["Copy Target ID", session.hostId], ["Copy User ID", session.userId]]) {
    fireEvent.click(screen.getByRole("button", {name: label}));
    expect(writeText).toHaveBeenLastCalledWith(id);
  }
  fireEvent.click(screen.getByRole("button", {name: "Disconnect"}));
  fireEvent.click(screen.getByRole("button", {name: "Terminate"}));
  expect(onDisconnect).toHaveBeenCalledWith(session.id);
  expect(onTerminate).toHaveBeenCalledWith(session.id);
});

it("shows the read-only preview under the actions and swaps Observe for Close viewer while observing", () => {
  const session = {id: "session-full-id", hostId: "target-full-id", userId: "user-full-id", protocol: "vnc", status: "active",
    connectedAt: "2026-10-05T00:00:00Z", lastActivity: "2026-10-05T00:00:00Z"} as unknown as ActiveSession;
  render(
    <AllPermissions>
      <SessionDetailPanel
        session={session}
        onDisconnect={() => {}}
        onTerminate={() => {}}
        onObserve={() => {}}
        observing
        onCloseObservation={() => {}}
        onObserveInWindow={() => {}}
        preview={<div data-testid="observation-preview">viewer</div>}
      />
    </AllPermissions>,
  );

  expect(screen.getByTestId("observation-preview")).toBeInTheDocument();
  expect(screen.getByRole("button", {name: "Close viewer"})).toBeInTheDocument();
  expect(screen.queryByRole("button", {name: "Observe"})).not.toBeInTheDocument();
  expect(screen.getByRole("button", {name: "Open in new window"})).toBeInTheDocument();
});
