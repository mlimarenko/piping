import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

/**
 * Unit tests for the SSH reconnect state machine in {@link attachSshTerminal}.
 *
 * We mock xterm, its addons, and the WebSocket so we can drive the adapter
 * through drop → silent → visible → failed transitions with fake timers and
 * assert: input blocking, the reset-on-reconnect (not on first open), and the
 * status callback sequence.
 */

// --- xterm / addon mocks -----------------------------------------------------
// Defined via vi.hoisted so the (hoisted) vi.mock factories can reference them
// while the test body can still inspect captured instances.

interface MockTerminalLike {
  options: { disableStdin?: boolean; fontSize?: number; fontFamily?: string };
  cols: number;
  rows: number;
  reset: ReturnType<typeof vi.fn>;
  dispose: ReturnType<typeof vi.fn>;
  write: ReturnType<typeof vi.fn>;
  focus: ReturnType<typeof vi.fn>;
  blur: ReturnType<typeof vi.fn>;
  resize: ReturnType<typeof vi.fn>;
  modes: { applicationCursorKeysMode: boolean };
  textarea: HTMLTextAreaElement;
  fireData(data: string): void;
  fireBinary(data: string): void;
}

const { termInstances, fit, MockTerminal } = vi.hoisted(() => {
  const instances: MockTerminalLike[] = [];
  const fit = vi.fn();
  class Term {
    public options: { disableStdin?: boolean; fontSize?: number; fontFamily?: string } = { fontSize: 13 };
    public cols = 80;
    public rows = 24;
    public reset = vi.fn();
    public dispose = vi.fn();
    public focus = vi.fn();
    public blur = vi.fn();
    public resize = vi.fn();
    public open = vi.fn();
    public loadAddon = vi.fn();
    public onResize = vi.fn(() => ({ dispose: vi.fn() }));
    public write = vi.fn();
    public modes = { applicationCursorKeysMode: false };
    public textarea = document.createElement("textarea");
    private dataHandlers = new Set<(data: string) => void>();
    private binaryHandlers = new Set<(data: string) => void>();

    public onData = vi.fn((cb: (data: string) => void) => {
      this.dataHandlers.add(cb);
      return { dispose: vi.fn(() => this.dataHandlers.delete(cb)) };
    });
    public onBinary = vi.fn((cb: (data: string) => void) => {
      this.binaryHandlers.add(cb);
      return { dispose: vi.fn(() => this.binaryHandlers.delete(cb)) };
    });

    constructor() {
      instances.push(this);
    }

    public fireData(data: string): void {
      this.dataHandlers.forEach((cb) => cb(data));
    }

    public fireBinary(data: string): void {
      this.binaryHandlers.forEach((cb) => cb(data));
    }
  }
  return { termInstances: instances, fit, MockTerminal: Term };
});

vi.mock("@xterm/xterm/css/xterm.css", () => ({}));
vi.mock("@xterm/xterm", () => ({ Terminal: MockTerminal }));
vi.mock("@xterm/addon-fit", () => ({
  FitAddon: class {
    fit = fit;
  },
}));
import { attachSshTerminal, type SshConnectionStatus } from "../sshAdapter";

// --- WebSocket mock ----------------------------------------------------------

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

  close(): void {
    this.readyState = MockWebSocket.CLOSED;
  }

  fireOpen(): void {
    this.readyState = MockWebSocket.OPEN;
    this.dispatch("open", new Event("open"));
  }

  fireClose(code = 1006, reason = "abnormal"): void {
    this.readyState = MockWebSocket.CLOSED;
    this.dispatch("close", new CloseEvent("close", { code, reason }));
  }

  fireMessage(data: string | ArrayBuffer): void {
    this.dispatch("message", new MessageEvent("message", { data }));
  }
}

function latestSocket(): MockWebSocket {
  return MockWebSocket.instances[MockWebSocket.instances.length - 1]!;
}

function makeContainer(): HTMLElement {
  return document.createElement("div");
}

describe("attachSshTerminal reconnect state machine", () => {
  beforeEach(() => {
    termInstances.length = 0;
    MockWebSocket.instances = [];
    vi.stubGlobal("WebSocket", MockWebSocket);
    vi.useFakeTimers();
  });

  afterEach(() => {
    Reflect.deleteProperty(document, "fonts");
    vi.useRealTimers();
    vi.unstubAllGlobals();
    vi.clearAllMocks();
  });

  it("observation never sends terminal input or resize and never reconnects", () => {
    const adapter = attachSshTerminal({ container: makeContainer(), url: "ws://localhost/api/v1/ws/observe/session-1", token: "", readOnly: true });
    const socket = latestSocket();
    socket.fireOpen();
    socket.fireMessage('__piping_ws__:{"type":"observer_resize","cols":120,"rows":42}');
    expect(termInstances[0]!.resize).toHaveBeenCalledWith(120, 42);
    socket.fireMessage('__piping_ws__:{"type":"observer_resize","cols":999999,"rows":42}');
    expect(termInstances[0]!.resize).toHaveBeenCalledTimes(1);
    termInstances[0]!.fireData("malicious input");
    termInstances[0]!.fireBinary("abc");
    expect(adapter.sendText("command")).toBe(false);
    expect(adapter.sendKey("enter")).toBe(false);
    adapter.fit();
    vi.advanceTimersByTime(1000);
    expect(socket.sent).toEqual([]);
    expect(fit).not.toHaveBeenCalled();
    expect(socket.url).not.toContain("token=");
    socket.fireClose();
    vi.advanceTimersByTime(5000);
    expect(MockWebSocket.instances).toHaveLength(1);
    adapter.dispose();
  });

  it("does not apply observer dimensions to an operator terminal", () => {
    const adapter = attachSshTerminal({ container: makeContainer(), url: "ws://localhost/api/v1/ws/ssh/session-1", token: "operator" });
    latestSocket().fireOpen();
    latestSocket().fireMessage('__piping_ws__:{"type":"observer_resize","cols":120,"rows":42}');
    expect(termInstances[0]!.resize).not.toHaveBeenCalled();
    adapter.dispose();
  });

  it("uses the resolved mono stack and refits after the font loads without delaying the socket", async () => {
    let resolveFont!: (faces: FontFace[]) => void;
    const load = vi.fn(() => new Promise<FontFace[]>((resolve) => { resolveFont = resolve; }));
    Object.defineProperty(document, "fonts", { configurable: true, value: { load } });
    const container = makeContainer();
    container.style.setProperty("--font-family-mono", '"Medeo Mono", monospace');
    const adapter = attachSshTerminal({ container, url: "wss://host/api/v1/ws/ssh/abc", token: "tok" });
    const terminal = termInstances[0]!;
    expect(load).toHaveBeenCalledWith('13px "Medeo Mono", monospace');
    expect(terminal.options.fontFamily).toBeUndefined();
    latestSocket().fireOpen();
    latestSocket().fireMessage("hello");
    expect(terminal.write).toHaveBeenCalled();
    const fitsBeforeLoad = fit.mock.calls.length;
    resolveFont([]);
    await vi.waitFor(() => expect(terminal.options.fontFamily).toBe('"Medeo Mono", monospace'));
    expect(fit.mock.calls.length).toBeGreaterThan(fitsBeforeLoad);
    expect(MockWebSocket.instances).toHaveLength(1);
    adapter.dispose();
  });

  it("applies an already loaded font asynchronously", async () => {
    Object.defineProperty(document, "fonts", { configurable: true, value: { load: vi.fn().mockResolvedValue([]) } });
    const container = makeContainer();
    container.style.setProperty("--font-family-mono", '"Medeo Mono", monospace');
    const adapter = attachSshTerminal({ container, url: "wss://host/api/v1/ws/ssh/abc", token: "tok" });
    await Promise.resolve();
    expect(termInstances[0]!.options.fontFamily).toBe('"Medeo Mono", monospace');
    adapter.dispose();
  });

  it("keeps the fallback usable if the font fails", async () => {
    Object.defineProperty(document, "fonts", { configurable: true, value: { load: vi.fn().mockRejectedValue(new Error("font unavailable")) } });
    const container = makeContainer();
    container.style.setProperty("--font-family-mono", '"Medeo Mono", monospace');
    const adapter = attachSshTerminal({ container, url: "wss://host/api/v1/ws/ssh/abc", token: "tok" });
    await Promise.resolve();
    await Promise.resolve();
    latestSocket().fireOpen();
    latestSocket().fireMessage("hello");
    expect(termInstances[0]!.options.fontFamily).toBeUndefined();
    expect(termInstances[0]!.write).toHaveBeenCalled();
    adapter.dispose();
  });

  it("does not update or refit a disposed terminal when the font arrives", async () => {
    let resolveFont!: (faces: FontFace[]) => void;
    Object.defineProperty(document, "fonts", { configurable: true, value: { load: () => new Promise<FontFace[]>((resolve) => { resolveFont = resolve; }) } });
    const container = makeContainer();
    container.style.setProperty("--font-family-mono", '"Medeo Mono", monospace');
    const adapter = attachSshTerminal({ container, url: "wss://host/api/v1/ws/ssh/abc", token: "tok" });
    adapter.dispose();
    const fitsAfterDispose = fit.mock.calls.length;
    resolveFont([]);
    await Promise.resolve();
    expect(termInstances[0]!.options.fontFamily).toBeUndefined();
    expect(fit.mock.calls.length).toBe(fitsAfterDispose);
  });

  it("turns a one-finger vertical swipe into xterm wheel scrolling, not shell input", () => {
    const container = makeContainer();
    const wheel = vi.fn();
    container.addEventListener("wheel", wheel);
    const adapter = attachSshTerminal({ container, url: "wss://host/api/v1/ws/ssh/abc", token: "tok" });
    latestSocket().fireOpen();
    const point = (y: number) => ({ identifier: 1, clientX: 80, clientY: y });
    container.dispatchEvent(new TouchEvent("touchstart", { touches: [point(200) as Touch], bubbles: true }));
    const move = new TouchEvent("touchmove", { touches: [point(260) as Touch], bubbles: true, cancelable: true });
    container.dispatchEvent(move);
    expect(wheel).toHaveBeenCalledOnce();
    expect(wheel.mock.calls[0]?.[0]).toMatchObject({ deltaY: -60, clientX: 80, clientY: 260 });
    expect(move.defaultPrevented).toBe(true);
    expect(latestSocket().sent.filter((data) => typeof data === "string" && !data.startsWith("__piping_ws__:"))).toEqual([]);
    adapter.dispose();
  });

  it("configures the native IME input without shell autocorrection or iOS focus zoom", () => {
    const adapter = attachSshTerminal({ container: makeContainer(), url: "wss://host/api/v1/ws/ssh/abc", token: "tok" });
    expect(termInstances[0]?.textarea).toMatchObject({ inputMode: "text", autocapitalize: "none", autocomplete: "off", spellcheck: false });
    expect(termInstances[0]?.textarea.getAttribute("autocorrect")).toBe("off");
    expect(termInstances[0]?.textarea.style.fontSize).toBe("16px");
    adapter.dispose();
  });

  it("sends touch keys only while live and respects application cursor mode without forcing keyboard focus", () => {
    const adapter = attachSshTerminal({ container: makeContainer(), url: "wss://host/api/v1/ws/ssh/abc", token: "tok" });
    expect(adapter.sendKey("enter")).toBe(false);
    latestSocket().fireOpen();
    adapter.sendKey("enter"); adapter.sendKey("tab"); adapter.sendKey("escape"); adapter.sendKey("interrupt");
    adapter.sendKey("up");
    const term = termInstances[0]!;
    term.modes.applicationCursorKeysMode = true;
    adapter.sendKey("left"); adapter.sendKey("pageDown");
    expect(latestSocket().sent).toEqual(["\r", "\t", "\x1b", "\x03", "\x1b[A", "\x1bOD", "\x1b[6~"]);
    expect(term.focus).not.toHaveBeenCalled();
    latestSocket().fireClose(1006);
    expect(adapter.sendKey("interrupt")).toBe(false);
    adapter.dispose();
    expect(adapter.sendKey("enter")).toBe(false);
  });

  it("gates virtual chords while preserving native data and binary input", () => {
    const adapter = attachSshTerminal({ container: makeContainer(), url: "wss://host/api/v1/ws/ssh/abc", token: "tok" });
    const socket = latestSocket();
    const term = termInstances[0]!;
    expect(adapter.sendKey("f1")).toBe(false);
    socket.fireOpen();
    expect(adapter.sendKey("home", { alt: true })).toBe(true);
    expect(adapter.sendKey({ character: "xy" }, { ctrl: true })).toBe(false);
    term.fireData("漢字\x1b[200~paste\x1b[201~");
    term.fireBinary("\xff");
    expect(socket.sent).toEqual(["\x1b[1;3H", "漢字\x1b[200~paste\x1b[201~", new Uint8Array([255])]);
    socket.readyState = MockWebSocket.CLOSED;
    expect(adapter.sendKey("f2")).toBe(false);
    adapter.dispose();
    expect(adapter.sendKey("f3")).toBe(false);
    expect(socket.sent).toHaveLength(3);
  });

  it("fits a resized container and sends the resulting PTY dimensions", () => {
    let resize!: ResizeObserverCallback;
    const observe = vi.fn();
    const disconnect = vi.fn();
    const originalObserver = globalThis.ResizeObserver;
    globalThis.ResizeObserver = class {
      constructor(callback: ResizeObserverCallback) { resize = callback; }
      observe = observe;
      disconnect = disconnect;
      unobserve = vi.fn();
    };
    try {
    const container = makeContainer();
    const adapter = attachSshTerminal({ container, url: "wss://host/api/v1/ws/ssh/abc", token: "tok" });
    const socket = latestSocket();
    socket.fireOpen();
    expect(observe).toHaveBeenCalledWith(container);
    const term = termInstances[0]!;
    const fitsBefore = fit.mock.calls.length;
    fit.mockImplementationOnce(() => { term.cols = 100; term.rows = 30; });
    resize([], {} as ResizeObserver);
    expect(fit).toHaveBeenCalledTimes(fitsBefore + 1);
    vi.advanceTimersByTime(90);
    expect(socket.sent).toContain('__piping_ws__:{"type":"resize","cols":100,"rows":30}');
    adapter.dispose();
    expect(disconnect).toHaveBeenCalledOnce();
    } finally {
      globalThis.ResizeObserver = originalObserver;
    }
  });

  it("transitions live → silent → visible → failed across the reconnect window", () => {
    const statuses: SshConnectionStatus[] = [];
    const adapter = attachSshTerminal({
      container: makeContainer(),
      url: "wss://host/api/v1/ws/ssh/abc",
      token: "tok",
      onStatus: (s) => statuses.push(s),
    });

    // First open: already in the initial `live` state, so no transition is
    // emitted (setStatus dedupes identical states).
    latestSocket().fireOpen();
    expect(statuses).toEqual([]);

    // Drop → reconnecting-silent, no banner/input-block yet.
    latestSocket().fireClose(1006, "abnormal");
    expect(statuses).toEqual(["reconnecting-silent"]);
    const term = termInstances[0]!;
    expect(term.options.disableStdin).toBe(true);

    // At 10s still down → reconnecting-visible, input blocked.
    vi.advanceTimersByTime(10_000);
    expect(statuses).toContain("reconnecting-visible");
    expect(term.options.disableStdin).toBe(true);

    // Keep failing through the 60s deadline → failed.
    let guard = 0;
    while (!statuses.includes("failed") && guard < 100) {
      guard += 1;
      latestSocket().fireClose(1006, "abnormal");
      vi.advanceTimersByTime(3000);
    }
    expect(statuses).toContain("failed");
    expect(term.options.disableStdin).toBe(true);

    adapter.dispose();
  });

  it("does NOT reset on the first open, but DOES reset on a reconnect open", () => {
    const adapter = attachSshTerminal({
      container: makeContainer(),
      url: "wss://host/api/v1/ws/ssh/abc",
      token: "tok",
    });
    const term = termInstances[0]!;

    // First open: no reset (nothing to redraw).
    latestSocket().fireOpen();
    expect(term.reset).not.toHaveBeenCalled();

    // Drop, then reconnect within the window.
    latestSocket().fireClose(1006, "abnormal");
    vi.advanceTimersByTime(500);
    const reconnected = latestSocket();
    reconnected.fireOpen();

    // Reconnect open resets exactly once, before the replayed bytes.
    expect(term.reset).toHaveBeenCalledTimes(1);

    adapter.dispose();
  });

  it("returns to live and re-enables input on reconnect before the deadline", () => {
    const statuses: SshConnectionStatus[] = [];
    const adapter = attachSshTerminal({
      container: makeContainer(),
      url: "wss://host/api/v1/ws/ssh/abc",
      token: "tok",
      onStatus: (s) => statuses.push(s),
    });
    const term = termInstances[0]!;

    latestSocket().fireOpen();
    latestSocket().fireClose(1006, "abnormal");
    // Escalate into the visible (blocked) phase.
    vi.advanceTimersByTime(10_000);
    expect(term.options.disableStdin).toBe(true);

    // Reconnect succeeds → back to live, input re-enabled.
    vi.advanceTimersByTime(500);
    latestSocket().fireOpen();
    expect(statuses[statuses.length - 1]).toBe("live");
    expect(term.options.disableStdin).toBe(false);

    adapter.dispose();
  });

  it("calls onClose only on the terminal give-up, not on per-attempt drops", () => {
    const onClose = vi.fn();
    const adapter = attachSshTerminal({
      container: makeContainer(),
      url: "wss://host/api/v1/ws/ssh/abc",
      token: "tok",
      onClose,
    });

    latestSocket().fireOpen();
    latestSocket().fireClose(1006, "abnormal");
    // Several per-attempt drops within the window must NOT call onClose.
    vi.advanceTimersByTime(500);
    latestSocket().fireClose(1006, "abnormal");
    vi.advanceTimersByTime(1000);
    expect(onClose).not.toHaveBeenCalled();

    // Push past the 60s deadline → onClose fires once with the give-up close.
    let guard = 0;
    while (onClose.mock.calls.length === 0 && guard < 100) {
      guard += 1;
      latestSocket().fireClose(1006, "abnormal");
      vi.advanceTimersByTime(3000);
    }
    expect(onClose).toHaveBeenCalledTimes(1);

    adapter.dispose();
  });

  it("a clean 1000 close goes straight to failed and calls onClose", () => {
    const statuses: SshConnectionStatus[] = [];
    const onClose = vi.fn();
    const adapter = attachSshTerminal({
      container: makeContainer(),
      url: "wss://host/api/v1/ws/ssh/abc",
      token: "tok",
      onStatus: (s) => statuses.push(s),
      onClose,
    });

    latestSocket().fireOpen();
    latestSocket().fireClose(1000, "");
    expect(statuses[statuses.length - 1]).toBe("failed");
    expect(onClose).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(10_000);
    expect(MockWebSocket.instances).toHaveLength(1);

    adapter.dispose();
  });

  it("a target-unavailable close is terminal and does not reconnect", () => {
    const statuses: SshConnectionStatus[] = [];
    const onClose = vi.fn();
    const adapter = attachSshTerminal({
      container: makeContainer(),
      url: "wss://host/api/v1/ws/ssh/abc",
      token: "tok",
      onStatus: (s) => statuses.push(s),
      onClose,
    });

    latestSocket().fireOpen();
    latestSocket().fireClose(1013, "Target connection timed out");

    expect(statuses[statuses.length - 1]).toBe("failed");
    expect(onClose).toHaveBeenCalledWith(1013, "Target connection timed out");
    vi.advanceTimersByTime(10_000);
    expect(MockWebSocket.instances).toHaveLength(1);

    adapter.dispose();
  });

  it("a 1000 'detached_alive' close reconnects instead of failing", () => {
    const statuses: SshConnectionStatus[] = [];
    const onClose = vi.fn();
    const adapter = attachSshTerminal({
      container: makeContainer(),
      url: "wss://host/api/v1/ws/ssh/abc",
      token: "tok",
      onStatus: (s) => statuses.push(s),
      onClose,
    });

    latestSocket().fireOpen();
    // Backend signals "client link dropped, SSH session still alive".
    latestSocket().fireClose(1000, "detached_alive");
    // Must NOT terminate — it enters the reconnect window and does not call onClose.
    expect(statuses[statuses.length - 1]).toBe("reconnecting-silent");
    expect(onClose).not.toHaveBeenCalled();
    vi.advanceTimersByTime(500);
    expect(MockWebSocket.instances).toHaveLength(2);

    adapter.dispose();
  });

  it("reports open only after the backend ready control frame or terminal payload", () => {
    const onOpen = vi.fn();
    const adapter = attachSshTerminal({
      container: makeContainer(),
      url: "wss://host/api/v1/ws/ssh/abc",
      token: "tok",
      onOpen,
    });
    const term = termInstances[0]!;

    latestSocket().fireOpen();
    expect(onOpen).not.toHaveBeenCalled();

    latestSocket().fireMessage("__piping_ws__:{\"type\":\"ready\"}");
    expect(onOpen).toHaveBeenCalledTimes(1);
    expect(term.write).not.toHaveBeenCalled();

    latestSocket().fireMessage("prompt$ ");
    expect(onOpen).toHaveBeenCalledTimes(1);
    expect(term.write).toHaveBeenLastCalledWith("prompt$ ");

    adapter.dispose();
  });

  it("uses the first terminal payload as readiness for older backends", () => {
    const onOpen = vi.fn();
    const adapter = attachSshTerminal({
      container: makeContainer(),
      url: "wss://host/api/v1/ws/ssh/abc",
      token: "tok",
      onOpen,
    });

    latestSocket().fireOpen();
    latestSocket().fireMessage("prompt$ ");

    expect(onOpen).toHaveBeenCalledTimes(1);

    adapter.dispose();
  });

  it("keeps terminal output attached after a per-attempt close and reconnect", () => {
    const adapter = attachSshTerminal({
      container: makeContainer(),
      url: "wss://host/api/v1/ws/ssh/abc",
      token: "tok",
    });
    const term = termInstances[0]!;

    latestSocket().fireOpen();
    latestSocket().fireMessage("before\r\n");
    expect(term.write).toHaveBeenLastCalledWith("before\r\n");

    latestSocket().fireClose(1006, "abnormal");
    vi.advanceTimersByTime(500);
    const reconnected = latestSocket();
    reconnected.fireOpen();
    reconnected.fireMessage("after\r\n");

    expect(term.reset).toHaveBeenCalledTimes(1);
    expect(term.write).toHaveBeenLastCalledWith("after\r\n");

    adapter.dispose();
  });

  it("synthesizes a 1008 close when the startup PTY shows an SSH auth failure", () => {
    const statuses: SshConnectionStatus[] = [];
    const onClose = vi.fn();
    const adapter = attachSshTerminal({
      container: makeContainer(),
      url: "wss://host/api/v1/ws/ssh/abc",
      token: "tok",
      onStatus: (s) => statuses.push(s),
      onClose,
    });

    latestSocket().fireOpen();
    // Backend forwards the upstream ssh "Permission denied" banner but never
    // sends a close frame (some sshd auth-method combos leave the PTY open).
    latestSocket().fireMessage(
      "testuser@10.0.0.1: Permission denied (publickey,password,keyboard-interactive).\r\n",
    );

    expect(onClose).toHaveBeenCalledTimes(1);
    expect(onClose).toHaveBeenCalledWith(1008, "SSH authentication failed");
    expect(statuses[statuses.length - 1]).toBe("failed");

    adapter.dispose();
  });

  it("does NOT misfire on a bare 'Permission denied' in ordinary output", () => {
    const onClose = vi.fn();
    const adapter = attachSshTerminal({
      container: makeContainer(),
      url: "wss://host/api/v1/ws/ssh/abc",
      token: "tok",
      onClose,
    });

    latestSocket().fireOpen();
    // A real prompt, then the user cats a logfile that happens to contain the
    // phrase — no method list, so it must NOT be treated as a connect failure.
    latestSocket().fireMessage("user@host:~$ ");
    latestSocket().fireMessage("sudo: 3 incorrect password attempts\r\nPermission denied\r\n");

    expect(onClose).not.toHaveBeenCalled();

    adapter.dispose();
  });

  it("stops auth detection after the first keystroke", () => {
    const onClose = vi.fn();
    const adapter = attachSshTerminal({
      container: makeContainer(),
      url: "wss://host/api/v1/ws/ssh/abc",
      token: "tok",
      onClose,
    });
    const term = termInstances[0]!;

    latestSocket().fireOpen();
    // User types; from now on a method-list banner is ordinary remote output
    // (e.g. ssh-ing onward from inside the session) and must not synthesize.
    term.fireData("ssh other-host\r");
    latestSocket().fireMessage(
      "me@other-host: Permission denied (publickey,password,keyboard-interactive).\r\n",
    );

    expect(onClose).not.toHaveBeenCalled();

    adapter.dispose();
  });

  it("filters backend control frames and sends stdin only while live", () => {
    const adapter = attachSshTerminal({
      container: makeContainer(),
      url: "wss://host/api/v1/ws/ssh/abc",
      token: "tok",
    });
    const term = termInstances[0]!;

    latestSocket().fireOpen();
    term.fireData("a");
    expect(latestSocket().sent).toEqual(["a"]);

    latestSocket().fireMessage("__piping_ws__:{\"type\":\"resize\"}");
    expect(term.write).not.toHaveBeenCalled();

    latestSocket().fireClose(1006, "abnormal");
    expect(term.options.disableStdin).toBe(true);
    expect(() => term.fireData("b")).not.toThrow();
    expect(latestSocket().sent).toEqual(["a"]);

    adapter.dispose();
  });

  it("blurs the hidden terminal input without disposing the session", () => {
    const adapter = attachSshTerminal({ container: makeContainer(), url: "wss://host/api/v1/ws/ssh/abc", token: "tok" });
    latestSocket().fireOpen();
    const term = termInstances[0]!;

    adapter.blur();
    expect(term.blur).toHaveBeenCalledOnce();
    // Blur is a keyboard concern, not a transport one.
    expect(term.dispose).not.toHaveBeenCalled();
    expect(MockWebSocket.instances).toHaveLength(1);

    adapter.dispose();
  });

  it("clamps setFontSize to the bounded range and re-fits the PTY", () => {
    const adapter = attachSshTerminal({ container: makeContainer(), url: "wss://host/api/v1/ws/ssh/abc", token: "tok" });
    latestSocket().fireOpen();
    const term = termInstances[0]!;
    // setFontSize must not disconnect to change metrics.
    expect(adapter.setFontSize(18)).toBe(18);
    expect(term.options.fontSize).toBe(18);

    expect(adapter.setFontSize(999)).toBe(24);
    expect(term.options.fontSize).toBe(24);
    expect(adapter.setFontSize(-4)).toBe(10);
    expect(term.options.fontSize).toBe(10);
    expect(adapter.getFontSize()).toBe(10);

    expect(MockWebSocket.instances).toHaveLength(1);
    expect(term.dispose).not.toHaveBeenCalled();

    adapter.dispose();
  });

  it("gates soft-keyboard focus: a tap does not summon the keyboard while disabled", () => {
    const container = makeContainer();
    const adapter = attachSshTerminal({ container, url: "wss://host/api/v1/ws/ssh/abc", token: "tok" });
    latestSocket().fireOpen();
    const term = termInstances[0]!;

    // Command mode: soft input off, tap must NOT focus (no OS keyboard).
    adapter.setSoftKeyboardEnabled(false);
    expect(term.textarea.inputMode).toBe("none");
    const point = { identifier: 1, clientX: 80, clientY: 200 };
    container.dispatchEvent(new TouchEvent("touchstart", { touches: [point as Touch], bubbles: true }));
    container.dispatchEvent(new TouchEvent("touchend", { touches: [], bubbles: true }));
    expect(term.focus).not.toHaveBeenCalled();

    // Direct input mode: soft input back on, tap focuses the live PTY.
    expect(adapter.sendText("echo composed\r")).toBe(true);
    expect(latestSocket().sent).toContain("echo composed\r");
    expect(term.focus).not.toHaveBeenCalled();
    adapter.setSoftKeyboardEnabled(true);
    expect(term.textarea.inputMode).toBe("text");
    container.dispatchEvent(new TouchEvent("touchstart", { touches: [point as Touch], bubbles: true }));
    container.dispatchEvent(new TouchEvent("touchend", { touches: [], bubbles: true }));
    expect(term.focus).toHaveBeenCalledOnce();

    adapter.dispose();
  });
});
