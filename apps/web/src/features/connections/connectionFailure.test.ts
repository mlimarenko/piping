import { describe, expect, it } from "vitest";
import { ApiError } from "@/shared/api/client";
import {
  connectionFailureFromApiError,
  connectionFailureFromWsClose,
} from "@/features/connections/connectionFailure";

describe("connectionFailure adapters", () => {
  it.each([
    ["CONNECTION_AUTHENTICATION_FAILED", "authentication"],
    ["CONNECTION_CREDENTIALS_REQUIRED", "configuration"],
    ["CONNECTION_UNREACHABLE", "unreachable"],
    ["CONNECTION_TIMEOUT", "timeout"],
    ["CONNECTION_UNSUPPORTED", "unsupported"],
    ["CONNECTION_BUSY", "busy"],
    ["CONNECTION_FAILED", "unknown"],
  ] as const)("maps ApiError reasonCode %s without exposing its message", (reasonCode, kind) => {
    const failure = connectionFailureFromApiError(
      new ApiError({ status: 502, reasonCode, message: "PRIVATE BACKEND DETAIL" }),
    );

    expect(failure.kind).toBe(kind);
    expect(failure).not.toHaveProperty("message");
  });

  it("does not infer a REST failure from HTTP status without a canonical reason code", () => {
    expect(
      connectionFailureFromApiError(new ApiError({ status: 401, message: "secret" })).kind,
    ).toBe("unknown");
    expect(
      connectionFailureFromApiError(new ApiError({ status: 429, message: "secret" })).kind,
    ).toBe("unknown");
    expect(
      connectionFailureFromApiError(
        new ApiError({ status: 502, reasonCode: "UNRECOGNIZED_REASON", message: "secret" }),
      ).kind,
    ).toBe("unknown");
    expect(connectionFailureFromApiError(new Error("secret")).kind).toBe("unknown");
  });

  it.each([
    [1008, "SSH authentication failed", "authentication"],
    [1013, "Target connection timed out", "timeout"],
    [1013, "Target refused connection", "unreachable"],
    [1013, "Upstream connection lost", "disconnected"],
    [1006, "PRIVATE BACKEND DETAIL", "network"],
  ] as const)("maps WS close %s / %s to %s", (code, reason, kind) => {
    const failure = connectionFailureFromWsClose(code, reason);

    expect(failure.kind).toBe(kind);
    expect(failure).not.toHaveProperty("message");
  });
});
