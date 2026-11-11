import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  computeReconnectDelayMs,
  openSessionSocket,
  RECONNECT_GIVE_UP_REASON,
} from "../sessionSocket";

/**
 * In-memory WebSocket double. Tracks constructor calls across a whole test
 * so we can assert backoff timing, subprotocol negotiation, and abort
 * behavior without hitting a real server.
 */
class MockWebSocket {
  static CONNECTING = 0;
  static OPEN = 1;
  static CLOSING = 2;
  static CLOSED = 3;
  static instances: MockWebSocket[] = [];

  public readyState = MockWebSocket.CONNECTING;
  public binaryType: BinaryType = "blob";
  public sent: Array<string | ArrayBufferLike | Blob | ArrayBufferView> = [];
  public url: string;
  public protocols: string | string[] | undefined;

  private listeners = new Map<string, Set<(ev: Event) => void>>();

  constructor(url: string, protocols?: string | string[]) {
    this.url = url;
    this.protocols = protocols;
    MockWebSocket.instances.push(this);
  }

  addEventListener(type: string, cb: (ev: Event) => void): void {
    let set = this.listeners.get(type);
    if (!set) {
      set = new Set();
      this.listeners.set(type, set);
    }
    set.add(cb);
  }

  removeEventListener(type: string, cb: (ev: Event) => void): void {
    this.listeners.get(type)?.delete(cb);
  }

  dispatch(type: string, ev: Event): void {
    this.listeners.get(type)?.forEach((cb) => cb(ev));
  }

  send(data: string | ArrayBufferLike | Blob | ArrayBufferView): void {
    this.sent.push(data);
  }

  close(code?: number, reason?: string): void {
    this.readyState = MockWebSocket.CLOSED;
    const ev = new CloseEvent("close", { code: code ?? 1000, reason: reason ?? "" });
    this.dispatch("close", ev);
  }

  // Helpers for tests:
  fireOpen(): void {
    this.readyState = MockWebSocket.OPEN;
    this.dispatch("open", new Event("open"));
  }

  fireCloseFromServer(code = 1006, reason = "abnormal"): void {
    this.readyState = MockWebSocket.CLOSED;
    this.dispatch("close", new CloseEvent("close", { code, reason }));
  }
}

describe("computeReconnectDelayMs", () => {
  it("doubles the delay per attempt, capped at maxDelayMs", () => {
    expect(computeReconnectDelayMs(1, 500, 30_000)).toBe(500);
    expect(computeReconnectDelayMs(2, 500, 30_000)).toBe(1000);
    expect(computeReconnectDelayMs(3, 500, 30_000)).toBe(2000);
    expect(computeReconnectDelayMs(4, 500, 30_000)).toBe(4000);
    expect(computeReconnectDelayMs(10, 500, 30_000)).toBe(30_000);
  });
});

describe("openSessionSocket", () => {
  beforeEach(() => {
    MockWebSocket.instances = [];
    vi.stubGlobal("WebSocket", MockWebSocket);
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("passes url and protocols through to the WebSocket constructor", () => {
    openSessionSocket({
      url: "wss://host/api/v1/ws/ssh/abc?token=x",
      protocols: ["guacamole"],
    });
    expect(MockWebSocket.instances).toHaveLength(1);
    expect(MockWebSocket.instances[0]!.url).toBe("wss://host/api/v1/ws/ssh/abc?token=x");
    expect(MockWebSocket.instances[0]!.protocols).toEqual(["guacamole"]);
    expect(MockWebSocket.instances[0]!.binaryType).toBe("arraybuffer");
  });

  it("reconnects with exponential backoff between attempts", () => {
    openSessionSocket({
      url: "wss://host/ws",
      reconnect: { baseDelayMs: 500, maxAttempts: 3 },
    });

    // Initial attempt.
    expect(MockWebSocket.instances).toHaveLength(1);
    MockWebSocket.instances[0]!.fireCloseFromServer();

    // Attempt 2 is scheduled at baseDelayMs (500ms).
    vi.advanceTimersByTime(499);
    expect(MockWebSocket.instances).toHaveLength(1);
    vi.advanceTimersByTime(1);
    expect(MockWebSocket.instances).toHaveLength(2);

    MockWebSocket.instances[1]!.fireCloseFromServer();

    // Attempt 3 waits baseDelayMs * 2 (1000ms).
    vi.advanceTimersByTime(999);
    expect(MockWebSocket.instances).toHaveLength(2);
    vi.advanceTimersByTime(1);
    expect(MockWebSocket.instances).toHaveLength(3);

    MockWebSocket.instances[2]!.fireCloseFromServer();

    // Attempt 4 waits baseDelayMs * 4 (2000ms).
    vi.advanceTimersByTime(1999);
    expect(MockWebSocket.instances).toHaveLength(3);
    vi.advanceTimersByTime(1);
    expect(MockWebSocket.instances).toHaveLength(4);

    // maxAttempts reached — no further reconnects.
    MockWebSocket.instances[3]!.fireCloseFromServer();
    vi.advanceTimersByTime(60_000);
    expect(MockWebSocket.instances).toHaveLength(4);
  });

  it("emits a reconnect event on each retry", () => {
    const reconnects = vi.fn();
    const sock = openSessionSocket({
      url: "wss://host/ws",
      reconnect: { baseDelayMs: 500, maxAttempts: 3 },
    });
    sock.addEventListener("reconnect", reconnects);

    MockWebSocket.instances[0]!.fireCloseFromServer();
    vi.advanceTimersByTime(500);
    expect(reconnects).toHaveBeenCalledTimes(1);

    MockWebSocket.instances[1]!.fireCloseFromServer();
    vi.advanceTimersByTime(1000);
    expect(reconnects).toHaveBeenCalledTimes(2);
  });

  it("resets the attempt counter after a successful open", () => {
    openSessionSocket({
      url: "wss://host/ws",
      reconnect: { baseDelayMs: 500, maxAttempts: 3 },
    });

    // First attempt succeeds.
    MockWebSocket.instances[0]!.fireOpen();

    // Drop → attempt 2 scheduled at baseDelayMs (500ms), not at the
    // whatever-it-was-before-the-open delay.
    MockWebSocket.instances[0]!.fireCloseFromServer();
    vi.advanceTimersByTime(500);
    expect(MockWebSocket.instances).toHaveLength(2);

    // Second attempt opens, then drops — counter reset, so next retry
    // is again baseDelayMs.
    MockWebSocket.instances[1]!.fireOpen();
    MockWebSocket.instances[1]!.fireCloseFromServer();
    vi.advanceTimersByTime(500);
    expect(MockWebSocket.instances).toHaveLength(3);
  });

  it("skips reconnect when shouldReconnect returns false", () => {
    openSessionSocket({
      url: "wss://host/ws",
      reconnect: { baseDelayMs: 500, maxAttempts: 3 },
      shouldReconnect: (ev) => ev.code !== 1000,
    });

    MockWebSocket.instances[0]!.fireOpen();
    MockWebSocket.instances[0]!.fireCloseFromServer(1000, "");
    vi.advanceTimersByTime(10_000);
    expect(MockWebSocket.instances).toHaveLength(1);
  });

  it("reconnects when shouldReconnect allows the close", () => {
    openSessionSocket({
      url: "wss://host/ws",
      reconnect: { baseDelayMs: 500, maxAttempts: 3 },
      shouldReconnect: (ev) => ev.reason === "detached_alive",
    });

    MockWebSocket.instances[0]!.fireOpen();
    MockWebSocket.instances[0]!.fireCloseFromServer(1000, "detached_alive");
    vi.advanceTimersByTime(499);
    expect(MockWebSocket.instances).toHaveLength(1);
    vi.advanceTimersByTime(1);
    expect(MockWebSocket.instances).toHaveLength(2);
  });

  it("AbortSignal stops pending reconnects", () => {
    const ctrl = new AbortController();
    openSessionSocket({
      url: "wss://host/ws",
      reconnect: { baseDelayMs: 500, maxAttempts: 8 },
      signal: ctrl.signal,
    });

    MockWebSocket.instances[0]!.fireCloseFromServer();
    // A reconnect is scheduled but hasn't fired yet.
    ctrl.abort();
    vi.advanceTimersByTime(10_000);
    expect(MockWebSocket.instances).toHaveLength(1);
  });

  it("AbortSignal closes the open socket synchronously", () => {
    const ctrl = new AbortController();
    const sock = openSessionSocket({
      url: "wss://host/ws",
      signal: ctrl.signal,
    });
    MockWebSocket.instances[0]!.fireOpen();
    expect(sock.readyState).toBe("open");

    ctrl.abort();
    expect(sock.readyState).toBe("closed");
    expect(MockWebSocket.instances[0]!.readyState).toBe(MockWebSocket.CLOSED);
  });

  it("signal already aborted before open does not open a socket", () => {
    const ctrl = new AbortController();
    ctrl.abort();
    const sock = openSessionSocket({ url: "wss://host/ws", signal: ctrl.signal });
    expect(MockWebSocket.instances).toHaveLength(0);
    expect(sock.readyState).toBe("closed");
  });

  it("close() cancels pending reconnect and marks the socket closed", () => {
    const sock = openSessionSocket({
      url: "wss://host/ws",
      reconnect: { baseDelayMs: 500, maxAttempts: 8 },
    });
    MockWebSocket.instances[0]!.fireCloseFromServer();
    sock.close();
    vi.advanceTimersByTime(10_000);
    expect(MockWebSocket.instances).toHaveLength(1);
    expect(sock.readyState).toBe("closed");
  });

  it("send throws when the socket is not open", () => {
    const sock = openSessionSocket({ url: "wss://host/ws" });
    // Initial state is CONNECTING — send must throw rather than silently
    // buffer, because Guacamole/xterm handshakes are ordering-sensitive.
    expect(() => sock.send("hello")).toThrow(/cannot send/);
  });

  it("send passes payload through once the socket is open", () => {
    const sock = openSessionSocket({ url: "wss://host/ws" });
    MockWebSocket.instances[0]!.fireOpen();
    sock.send("hello");
    const buf = new Uint8Array([1, 2, 3]).buffer;
    sock.send(buf);
    expect(MockWebSocket.instances[0]!.sent).toEqual(["hello", buf]);
  });

  it("forwards message events to subscribers", () => {
    const onMessage = vi.fn();
    const sock = openSessionSocket({ url: "wss://host/ws" });
    const unsub = sock.addEventListener("message", onMessage);
    MockWebSocket.instances[0]!.fireOpen();
    const ev = new MessageEvent("message", { data: "hi" });
    MockWebSocket.instances[0]!.dispatch("message", ev);
    expect(onMessage).toHaveBeenCalledWith(ev);
    unsub();
    MockWebSocket.instances[0]!.dispatch("message", ev);
    expect(onMessage).toHaveBeenCalledTimes(1);
  });

  it("stops retrying and emits a terminal give-up close at the deadline", () => {
    const closes: CloseEvent[] = [];
    const sock = openSessionSocket({
      url: "wss://host/ws",
      reconnect: { baseDelayMs: 500, maxDelayMs: 3000, deadlineMs: 60_000 },
    });
    sock.addEventListener("close", (ev) => closes.push(ev as CloseEvent));

    // Initial socket never opens; drive it through repeated drops until the
    // 60s wall-clock deadline is hit. Each failed attempt fires a per-attempt
    // close, then schedules the next retry.
    let guard = 0;
    while (sock.readyState !== "closed" && guard < 100) {
      guard += 1;
      const latest = MockWebSocket.instances[MockWebSocket.instances.length - 1]!;
      latest.fireCloseFromServer();
      // Let the next scheduled reconnect (if any) fire.
      vi.advanceTimersByTime(3000);
    }

    const giveUp = closes.find((c) => c.reason === RECONNECT_GIVE_UP_REASON);
    expect(giveUp).toBeDefined();
    expect(giveUp!.code).toBe(1006);
    expect(sock.readyState).toBe("closed");

    // Past the deadline no further sockets are created even as time advances.
    const created = MockWebSocket.instances.length;
    vi.advanceTimersByTime(120_000);
    expect(MockWebSocket.instances).toHaveLength(created);
  });

  it("emits the give-up close only once", () => {
    const closes: CloseEvent[] = [];
    const sock = openSessionSocket({
      url: "wss://host/ws",
      reconnect: { baseDelayMs: 500, maxDelayMs: 3000, deadlineMs: 10_000 },
    });
    sock.addEventListener("close", (ev) => closes.push(ev as CloseEvent));

    let guard = 0;
    while (sock.readyState !== "closed" && guard < 100) {
      guard += 1;
      const latest = MockWebSocket.instances[MockWebSocket.instances.length - 1]!;
      latest.fireCloseFromServer();
      vi.advanceTimersByTime(3000);
    }

    const giveUps = closes.filter((c) => c.reason === RECONNECT_GIVE_UP_REASON);
    expect(giveUps).toHaveLength(1);
  });

  it("resets the deadline window after a successful open", () => {
    const sock = openSessionSocket({
      url: "wss://host/ws",
      reconnect: { baseDelayMs: 500, maxDelayMs: 1000, deadlineMs: 5000 },
    });

    // Burn ~4s of the window via a drop + retry, then open successfully.
    MockWebSocket.instances[0]!.fireCloseFromServer();
    vi.advanceTimersByTime(4000);
    const reconnected = MockWebSocket.instances[MockWebSocket.instances.length - 1]!;
    reconnected.fireOpen();

    // A fresh drop now should get the FULL 5s budget again (window reset on
    // open), so a single drop does not immediately give up.
    reconnected.fireCloseFromServer();
    vi.advanceTimersByTime(500);
    // Still reconnecting: a new socket was scheduled, not a give-up.
    expect(sock.readyState).not.toBe("closed");
  });
});
