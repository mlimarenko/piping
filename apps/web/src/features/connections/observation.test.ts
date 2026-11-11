import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  normalizeObservationHandoff,
  observationHandoffFor,
  observationWebsocketUrl,
  observationWindowPath,
  openObservationWindow,
  readObservationHandoff,
  removeObservationHandoff,
  writeObservationHandoff,
} from "./observation";
import type { ActiveSession } from "@/shared/api/types";

const sessionId = "session-id";
const observeCapability = {
  allowed: true,
  reason: null,
  websocket_url: `/api/v1/ws/observe/${sessionId}`,
  transport: "terminal" as const,
};

const session = {
  id: sessionId,
  hostId: "target-id",
  hostName: "Target",
  userDisplayName: "Operator",
  protocol: "ssh",
  status: "active",
  connectedAt: "2026-10-05T00:00:00Z",
  lastActivity: "2026-10-05T00:00:00Z",
} as ActiveSession;

beforeEach(() => {
  removeObservationHandoff(sessionId);
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("observation capability", () => {
  it("maps an allowed capability to the same-origin observe socket", () => {
    expect(observationWebsocketUrl(observeCapability, sessionId, "ssh")).toBe(
      "ws://localhost:3000/api/v1/ws/observe/session-id",
    );
  });

  it("rejects a socket for another session, origin, or transport", () => {
    expect(observationWebsocketUrl(observeCapability, "other-session", "ssh")).toBeNull();
    expect(
      observationWebsocketUrl(
        { ...observeCapability, websocket_url: "wss://attacker.example/api/v1/ws/observe/session-id" },
        sessionId,
        "ssh",
      ),
    ).toBeNull();
    expect(
      observationWebsocketUrl(
        { ...observeCapability, transport: "guacamole" },
        sessionId,
        "ssh",
      ),
    ).toBeNull();
  });

  it("rejects denied capabilities and client-only CCTV sessions", () => {
    expect(
      observationWebsocketUrl({ ...observeCapability, allowed: false }, sessionId, "ssh"),
    ).toBeNull();
    expect(observationWebsocketUrl(observeCapability, sessionId, "cctv")).toBeNull();
  });
});

describe("observation handoff", () => {
  it("builds a snapshot from the selected session and its validated socket", () => {
    expect(observationHandoffFor(session, "ws://localhost:3000/api/v1/ws/observe/session-id"))
      .toEqual({
        sessionId,
        hostId: "target-id",
        hostName: "Target",
        userLabel: "Operator",
        protocol: "ssh",
        websocketUrl: "ws://localhost:3000/api/v1/ws/observe/session-id",
      });
    expect(observationHandoffFor({ ...session, protocol: "cctv" }, "ws://localhost:3000/x")).toBeNull();
  });

  it("persists the snapshot for reloads and drops a tampered socket", () => {
    writeObservationHandoff({
      sessionId,
      hostId: "target-id",
      hostName: "Target",
      protocol: "ssh",
      websocketUrl: "ws://localhost:3000/api/v1/ws/observe/session-id",
    });
    expect(readObservationHandoff(sessionId)).toMatchObject({
      sessionId,
      protocol: "ssh",
      websocketUrl: "ws://localhost:3000/api/v1/ws/observe/session-id",
    });
    expect(
      normalizeObservationHandoff({
        sessionId,
        hostId: "target-id",
        hostName: "Target",
        protocol: "ssh",
        websocketUrl: "wss://attacker.example/api/v1/ws/observe/session-id",
      }),
    ).toBeNull();
    expect(readObservationHandoff("other-session")).toBeNull();
  });

  it("opens the dedicated route and clears the snapshot when a popup is blocked", () => {
    const open = vi.spyOn(window, "open").mockReturnValue({} as Window);
    expect(
      openObservationWindow({
        sessionId,
        hostId: "target-id",
        hostName: "Target",
        protocol: "ssh",
        websocketUrl: "ws://localhost:3000/api/v1/ws/observe/session-id",
      }),
    ).toBe(true);
    expect(open).toHaveBeenCalledWith(observationWindowPath(sessionId), "_blank");
    expect(readObservationHandoff(sessionId)).not.toBeNull();

    open.mockReturnValue(null);
    expect(
      openObservationWindow({
        sessionId,
        hostId: "target-id",
        hostName: "Target",
        protocol: "ssh",
        websocketUrl: "ws://localhost:3000/api/v1/ws/observe/session-id",
      }),
    ).toBe(false);
    expect(readObservationHandoff(sessionId)).toBeNull();
  });
});
