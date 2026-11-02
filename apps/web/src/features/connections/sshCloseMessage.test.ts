import { describe, expect, it } from "vitest";

import {
  SSH_AUTH_DENIED_PTY_RE,
  SSH_AUTH_FAILURE_CODE,
  SSH_AUTH_FAILURE_REASON,
  sshCloseMessage,
} from "./sshCloseMessage";

describe("sshCloseMessage", () => {
  it("classifies auth failures (backend reason and synthesized reason)", () => {
    for (const reason of [
      "SSH authentication failed",
      "Permission denied (password)",
      "authentication failed",
      "credentials rejected",
    ]) {
      expect(sshCloseMessage(1008, reason)).toEqual({
        titleKey: "connections.authFailed",
        descriptionKey: "connections.authFailedDescription",
      });
    }
  });

  it("classifies the network failure family from the backend 1013 reasons", () => {
    for (const reason of [
      "Target refused connection",
      "Target is unreachable",
      "Target connection timed out",
    ]) {
      expect(sshCloseMessage(1013, reason)).toEqual({
        titleKey: "connections.hostUnreachable",
        descriptionKey: "connections.hostUnreachableDescription",
      });
    }
  });

  it("classifies an upstream mid-session loss", () => {
    expect(sshCloseMessage(1013, "Upstream connection lost")).toEqual({
      titleKey: "connections.upstreamLost",
      descriptionKey: "connections.upstreamLostDescription",
    });
  });

  it("auth wins over the network bucket when both phrases could match", () => {
    // "Permission denied" must never be mis-bucketed as a network error.
    expect(sshCloseMessage(1008, "Permission denied (keyboard-interactive)").titleKey).toBe(
      "connections.authFailed",
    );
  });

  it("falls back to network/generic when no reason text is present", () => {
    expect(sshCloseMessage(1006, undefined).titleKey).toBe("connections.networkError");
    expect(sshCloseMessage(undefined, "").titleKey).toBe("connections.networkError");
    expect(sshCloseMessage(4001, "").titleKey).toBe("connections.connectionFailed");
  });
});

describe("SSH_AUTH_DENIED_PTY_RE", () => {
  it("matches the OpenSSH method-list auth banner", () => {
    expect(
      SSH_AUTH_DENIED_PTY_RE.test(
        "testuser@10.0.0.1: Permission denied (publickey,password,keyboard-interactive).",
      ),
    ).toBe(true);
    expect(SSH_AUTH_DENIED_PTY_RE.test("Permission denied (password).")).toBe(true);
    expect(
      SSH_AUTH_DENIED_PTY_RE.test("Permission denied (keyboard-interactive)."),
    ).toBe(true);
  });

  it("does NOT match a bare 'Permission denied' in ordinary output", () => {
    expect(SSH_AUTH_DENIED_PTY_RE.test("Permission denied")).toBe(false);
    expect(SSH_AUTH_DENIED_PTY_RE.test("rm: cannot remove 'x': Permission denied")).toBe(false);
    // A method list without an interactive method is not the connect banner.
    expect(SSH_AUTH_DENIED_PTY_RE.test("Permission denied (publickey).")).toBe(false);
  });

  it("exposes the synthesized close constants matching the backend contract", () => {
    expect(SSH_AUTH_FAILURE_CODE).toBe(1008);
    expect(SSH_AUTH_FAILURE_REASON).toBe("SSH authentication failed");
  });
});
