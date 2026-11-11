import { afterEach, describe, expect, it, vi } from "vitest";
import Guacamole from "guacamole-common-js";

import { observeTunnelWireClose, type WireCloseObservable } from "../guacShared";

/**
 * Minimal WebSocket double for the real Guacamole tunnel: the vendor only needs
 * the constructor, the `on*` handlers it assigns, and `close()`.
 */
class FakeWebSocket extends EventTarget {
  public static instances: FakeWebSocket[] = [];
  public onopen: ((event: Event) => void) | null = null;
  public onclose: ((event: CloseEvent) => void) | null = null;
  public onmessage: ((event: MessageEvent) => void) | null = null;
  public close = vi.fn();

  constructor(public url: string, public protocols?: string | string[]) {
    super();
    FakeWebSocket.instances.push(this);
  }

  /** Deliver a server close the way the browser does. */
  deliverClose(code: number, reason: string): void {
    this.onclose?.(new CloseEvent("close", { code, reason }));
  }
}

function realTunnel() {
  vi.stubGlobal("WebSocket", FakeWebSocket);
  const statuses: Array<{ code: number; message?: string }> = [];
  const states: number[] = [];
  const tunnel = new Guacamole.WebSocketTunnel("wss://host/api/v1/ws/observe/session-1");
  tunnel.onerror = (status) => statuses.push(status);
  tunnel.onstatechange = (state) => states.push(state);
  return { tunnel, statuses, states };
}

describe("patched Guacamole wire close", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    FakeWebSocket.instances = [];
  });

  it("keeps a denial code that the Guacamole status collapses, without taking over the socket", () => {
    const { tunnel, statuses, states } = realTunnel();
    const wireClose = vi.fn();
    observeTunnelWireClose(tunnel, wireClose);
    const stubbedSocket = globalThis.WebSocket;

    tunnel.connect("token=");
    expect(FakeWebSocket.instances).toHaveLength(1);
    // Product code subscribes; it never swaps the socket constructor.
    expect(globalThis.WebSocket).toBe(stubbedSocket);

    FakeWebSocket.instances[0]!.deliverClose(1008, "observer_lease_expired");

    // The vendor still owns status/error/state handling.
    expect(statuses).toHaveLength(1);
    expect(statuses[0]!.message).toBe("observer_lease_expired");
    expect(Number.isNaN(statuses[0]!.code)).toBe(true);
    expect(states).toContain(Guacamole.Tunnel.State.CLOSED);
    expect(FakeWebSocket.instances[0]!.close).toHaveBeenCalled();
    // ...and the patched tunnel hands over the pair the status lost.
    expect(wireClose).toHaveBeenCalledExactlyOnceWith(1008, "observer_lease_expired");
  });

  it("keeps the lease code for the same reason text", () => {
    const { tunnel, statuses } = realTunnel();
    const wireClose = vi.fn();
    observeTunnelWireClose(tunnel, wireClose);

    tunnel.connect("token=");
    FakeWebSocket.instances[0]!.deliverClose(1000, "observer_lease_expired");

    // Identical Guacamole status for both codes; only the wire pair differs.
    expect(statuses[0]!.message).toBe("observer_lease_expired");
    expect(Number.isNaN(statuses[0]!.code)).toBe(true);
    expect(wireClose).toHaveBeenCalledExactlyOnceWith(1000, "observer_lease_expired");
  });

  it("leaves an unobserved tunnel on its ordinary Guacamole lifecycle", () => {
    const { tunnel, statuses, states } = realTunnel();

    observeTunnelWireClose(tunnel);
    expect((tunnel as WireCloseObservable).onwsclose).toBeNull();

    tunnel.connect("token=");
    FakeWebSocket.instances[0]!.deliverClose(1000, "observer_lease_expired");

    expect(statuses).toHaveLength(1);
    expect(states).toContain(Guacamole.Tunnel.State.CLOSED);
  });
});
