import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type * as ConnectionsApi from "@/features/connections/api";
import LiveSessionViewer from "./LiveSessionViewer";

const api = vi.hoisted(() => ({ getSessionClientCredentials: vi.fn(), closeSession: vi.fn(), terminateSession: vi.fn() }));
vi.mock("@/demo/env", () => ({ IS_DEMO: false }));
vi.mock("@/features/connections/api", async original => ({ ...await original<typeof ConnectionsApi>(), ...api }));
vi.mock("@/features/connections/components/connections/LiveTerminal", () => ({
  default: (props: { websocketUrl: string; readOnly?: boolean; onOpen?: () => void; onClose?: (code?: number, reason?: string) => void }) => (
    <div data-testid="ssh-terminal" data-readonly={String(props.readOnly)} data-url={props.websocketUrl}>
      <button onClick={props.onOpen}>Attach</button>
      <button onClick={() => props.onClose?.(1000, "observer_lease_expired")}>Expire lease</button>
      <button onClick={() => props.onClose?.(1000, "detach")}>Clean close</button>
      <button onClick={() => props.onClose?.(1008, "policy_denied")}>Deny</button>
      <button onClick={() => props.onClose?.(1006, "observer_lease_expired")}>Drop with lease reason</button>
    </div>
  ),
}));
vi.mock("@/features/connections/components/connections/LiveRDP", () => ({
  default: (props: { websocketUrl: string; readOnly: boolean; onOpen?: () => void; onClose?: (clean: boolean, code?: number, reason?: string) => void; onWireClose?: (code: number, reason: string) => void }) => (
    <div data-testid="guacamole-display" data-readonly={String(props.readOnly)} data-url={props.websocketUrl}>
      <button onClick={props.onOpen}>Paint</button>
      <button onClick={() => props.onClose?.(true)}>Disconnect</button>
      {/* The adapter forwards the tunnel socket's raw close pair. */}
      <button onClick={() => props.onWireClose?.(1000, "observer_lease_expired")}>Expire lease</button>
      <button onClick={() => props.onWireClose?.(1008, "observer_lease_expired")}>Deny with lease reason</button>
      <button onClick={() => props.onWireClose?.(1006, "observer_lease_expired")}>Drop with lease reason</button>
      <button onClick={() => props.onWireClose?.(1000, "detach")}>Close with other reason</button>
      <button onClick={() => props.onWireClose?.(Number.NaN, "observer_lease_expired")}>Expire with unknown code</button>
      <button onClick={() => props.onClose?.(false, 771, "Session closed")}>Tunnel error</button>
      <button onClick={() => props.onClose?.(false, 1000, "observer_lease_expired")}>Status mimics expiry</button>
    </div>
  ),
}));

describe("shared desktop renderer", () => {
  it("routes operator VNC to Guacamole and reports display and disconnect events without requesting credentials", async () => {
    const onOpen = vi.fn(); const onClose = vi.fn();
    render(<LiveSessionViewer sessionId="session-1" protocol="vnc" websocketUrl="/api/v1/ws/vnc/session-1" token="operator" isActive onOpen={onOpen} onClose={onClose} />);
    const display = await screen.findByTestId("guacamole-display");
    expect(display).toHaveAttribute("data-url", "/api/v1/ws/vnc/session-1");
    expect(display).toHaveAttribute("data-readonly", "false");
    fireEvent.click(screen.getByRole("button", {name: "Paint"}));
    fireEvent.click(screen.getByRole("button", {name: "Disconnect"}));
    expect(onOpen).toHaveBeenCalledOnce();
    expect(onClose).toHaveBeenCalledWith(true, undefined, undefined);
    expect(api.getSessionClientCredentials).not.toHaveBeenCalled();
    expect(api.terminateSession).not.toHaveBeenCalled();
  });

  it("routes VNC observation to the same display in read-only mode without terminating its owner", async () => {
    const { unmount } = render(<LiveSessionViewer sessionId="session-1" protocol="vnc" websocketUrl="/api/v1/ws/observe/session-1" token="" isActive readOnly />);
    const display = await screen.findByTestId("guacamole-display");
    expect(display).toHaveAttribute("data-url", "/api/v1/ws/observe/session-1");
    expect(display).toHaveAttribute("data-readonly", "true");
    unmount();
    expect(api.getSessionClientCredentials).not.toHaveBeenCalled();
    expect(api.closeSession).not.toHaveBeenCalled();
    expect(api.terminateSession).not.toHaveBeenCalled();
  });
});

describe("observation lease renewal", () => {
  const observeUrl = "/api/v1/ws/observe/session-1";

  /** Advance past any scheduled renewal with fake timers and flush React. */
  function settleRenewalDelay() {
    act(() => { vi.advanceTimersByTime(1_000); });
  }

  /** Let the installed fake clock show a lease that actually served the viewer. */
  function serveLease() {
    act(() => { vi.advanceTimersByTime(60_000); });
  }

  it("re-handshakes the same observe route when the SSH lease expires", async () => {
    const onOpen = vi.fn();
    const onClose = vi.fn();
    render(<LiveSessionViewer sessionId="session-1" protocol="ssh" websocketUrl={observeUrl} token="" isActive readOnly onOpen={onOpen} onClose={onClose} />);
    const first = await screen.findByTestId("ssh-terminal");
    expect(first).toHaveAttribute("data-url", observeUrl);
    fireEvent.click(within(first).getByRole("button", { name: "Attach" }));
    vi.useFakeTimers();
    try {
      serveLease();
      fireEvent.click(within(first).getByRole("button", { name: "Expire lease" }));
      // The caller still sees the exact close; the renewal is a fresh handshake.
      expect(onClose).toHaveBeenCalledWith(true, 1000, "observer_lease_expired");
      settleRenewalDelay();
      const renewed = screen.getByTestId("ssh-terminal");
      expect(renewed).not.toBe(first);
      expect(renewed).toHaveAttribute("data-url", observeUrl);
      expect(renewed).toHaveAttribute("data-readonly", "true");
      fireEvent.click(within(renewed).getByRole("button", { name: "Attach" }));
      expect(onOpen).toHaveBeenCalledTimes(2);
    } finally {
      vi.useRealTimers();
    }
    expect(api.getSessionClientCredentials).not.toHaveBeenCalled();
    expect(api.terminateSession).not.toHaveBeenCalled();
  });

  it("re-handshakes a Guacamole observation that only carries the lease reason", async () => {
    render(<LiveSessionViewer sessionId="session-1" protocol="vnc" websocketUrl={observeUrl} token="" isActive readOnly />);
    const first = await screen.findByTestId("guacamole-display");
    fireEvent.click(within(first).getByRole("button", { name: "Paint" }));
    vi.useFakeTimers();
    try {
      serveLease();
      fireEvent.click(within(first).getByRole("button", { name: "Expire lease" }));
      settleRenewalDelay();
      const renewed = screen.getByTestId("guacamole-display");
      expect(renewed).not.toBe(first);
      expect(renewed).toHaveAttribute("data-url", observeUrl);
    } finally {
      vi.useRealTimers();
    }
  });

  it.each([
    ["an ordinary clean SSH close", "ssh-terminal", "Clean close"],
    ["an SSH policy denial", "ssh-terminal", "Deny"],
    ["a transport drop carrying the lease reason", "ssh-terminal", "Drop with lease reason"],
    ["an ordinary Guacamole disconnect", "guacamole-display", "Disconnect"],
    ["a Guacamole tunnel error", "guacamole-display", "Tunnel error"],
    ["a Guacamole status mimicking the wire expiry", "guacamole-display", "Status mimics expiry"],
    ["a Guacamole policy denial carrying the lease reason", "guacamole-display", "Deny with lease reason"],
    ["a Guacamole transport drop carrying the lease reason", "guacamole-display", "Drop with lease reason"],
    ["a Guacamole clean close with another reason", "guacamole-display", "Close with other reason"],
    ["a Guacamole close whose code the transport lost", "guacamole-display", "Expire with unknown code"],
  ])("keeps %s terminal", async (_label, testId, action) => {
    render(<LiveSessionViewer sessionId="session-1" protocol={testId === "ssh-terminal" ? "ssh" : "rdp"} websocketUrl={observeUrl} token="" isActive readOnly />);
    const first = await screen.findByTestId(testId);
    fireEvent.click(within(first).getByRole("button", { name: testId === "ssh-terminal" ? "Attach" : "Paint" }));
    vi.useFakeTimers();
    try {
      serveLease();
      fireEvent.click(within(first).getByRole("button", { name: action }));
      settleRenewalDelay();
      expect(screen.getByTestId(testId)).toBe(first);
    } finally {
      vi.useRealTimers();
    }
  });

  it("does not renew a fresh handshake that expires before it serves the viewer", async () => {
    render(<LiveSessionViewer sessionId="session-1" protocol="ssh" websocketUrl={observeUrl} token="" isActive readOnly />);
    const first = await screen.findByTestId("ssh-terminal");
    fireEvent.click(within(first).getByRole("button", { name: "Attach" }));
    vi.useFakeTimers();
    let renewed: HTMLElement;
    try {
      serveLease();
      fireEvent.click(within(first).getByRole("button", { name: "Expire lease" }));
      settleRenewalDelay();
      renewed = screen.getByTestId("ssh-terminal");
      expect(renewed).not.toBe(first);
      // The replacement never attached, so another expiry has nothing to renew.
      fireEvent.click(within(renewed).getByRole("button", { name: "Expire lease" }));
      settleRenewalDelay();
      expect(screen.getByTestId("ssh-terminal")).toBe(renewed);
    } finally {
      vi.useRealTimers();
    }
  });

  it("cancels a pending renewal when the observation closes", async () => {
    const clearTimeout = vi.spyOn(window, "clearTimeout");
    const view = render(<LiveSessionViewer sessionId="session-1" protocol="ssh" websocketUrl={observeUrl} token="" isActive readOnly />);
    const first = await screen.findByTestId("ssh-terminal");
    fireEvent.click(within(first).getByRole("button", { name: "Attach" }));
    fireEvent.click(within(first).getByRole("button", { name: "Expire lease" }));
    view.unmount();
    expect(clearTimeout).toHaveBeenCalled();
    expect(screen.queryByTestId("ssh-terminal")).toBeNull();
    clearTimeout.mockRestore();
  });

  it("keeps an operator viewer's close terminal", async () => {
    const onClose = vi.fn();
    render(<LiveSessionViewer sessionId="session-1" protocol="ssh" websocketUrl="/api/v1/ws/ssh/session-1" token="tok" isActive onClose={onClose} />);
    const first = await screen.findByTestId("ssh-terminal");
    fireEvent.click(within(first).getByRole("button", { name: "Attach" }));
    vi.useFakeTimers();
    try {
      fireEvent.click(within(first).getByRole("button", { name: "Expire lease" }));
      expect(onClose).toHaveBeenCalledWith(true, 1000, "observer_lease_expired");
      settleRenewalDelay();
      expect(screen.getByTestId("ssh-terminal")).toBe(first);
    } finally {
      vi.useRealTimers();
    }
  });
});
