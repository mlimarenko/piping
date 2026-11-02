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
