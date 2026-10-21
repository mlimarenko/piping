import { afterEach, describe, expect, it, vi } from "vitest";

const rfbMocks = vi.hoisted(() => {
  class MockRFB {
    public static instances: MockRFB[] = [];
    private scaleViewportValue = false;
    private resizeSessionValue = false;
    public scaleViewportSet = vi.fn();
    public resizeSessionSet = vi.fn();
    public clipboardPasteFrom = vi.fn();
    public focus = vi.fn();
    public viewOnly = false;
    public sentKey = vi.fn();
    public sendKey = this.sentKey;
    public languageSwitchKeyDown = vi.fn();
    private listeners = new Map<string, EventListener>();
    private readonly noVncKeyboardListener: EventListener;
    public disconnect = vi.fn(() => {
      this.target.ownerDocument.removeEventListener("keydown", this.noVncKeyboardListener, true);
    });
    public addEventListener = vi.fn((type: string, cb: EventListener) => {
      this.listeners.set(type, cb);
    });
    public removeEventListener = vi.fn((type: string, cb: EventListener) => {
      if (this.listeners.get(type) === cb) {
        this.listeners.delete(type);
      }
    });

    constructor(
      public target: HTMLElement,
      public url: string,
      public options?: Record<string, unknown>,
    ) {
      MockRFB.instances.push(this);
      target.append(document.createElement("canvas"));
      this.noVncKeyboardListener = (event) => {
        const keyboardEvent = event as KeyboardEvent;
        if (
          keyboardEvent.shiftKey &&
          (keyboardEvent.altKey || keyboardEvent.ctrlKey) &&
          keyboardEvent.code.startsWith("Shift")
        ) {
          this.languageSwitchKeyDown(keyboardEvent.code);
        }
        if (!keyboardEvent.ctrlKey || keyboardEvent.code !== "KeyV") return;
        this.sendKey(0xffe3, "ControlLeft", true);
        this.sendKey(0x0076, "KeyV", true);
        this.sendKey(0x0076, "KeyV", false);
        this.sendKey(0xffe3, "ControlLeft", false);
      };
      target.ownerDocument.addEventListener("keydown", this.noVncKeyboardListener, true);
    }

    get scaleViewport(): boolean {
      return this.scaleViewportValue;
    }

    set scaleViewport(value: boolean) {
      this.scaleViewportValue = value;
      this.scaleViewportSet(value);
    }

    get resizeSession(): boolean {
      return this.resizeSessionValue;
    }

    set resizeSession(value: boolean) {
      this.resizeSessionValue = value;
      this.resizeSessionSet(value);
    }

    emit(type: string, detail: unknown): void {
      this.listeners.get(type)?.(new CustomEvent(type, { detail }));
    }
  }

  return { MockRFB };
});

vi.mock("@novnc/novnc", () => ({
  default: rfbMocks.MockRFB,
}));

import { attachVncViewer } from "../vncAdapter";

const remoteModifierReleaseCalls = [
  [0xffe3, "ControlLeft", false],
  [0xffe4, "ControlRight", false],
  [0xffe1, "ShiftLeft", false],
  [0xffe2, "ShiftRight", false],
  [0xffe9, "AltLeft", false],
  [0xffea, "AltRight", false],
  [0xffeb, "MetaLeft", false],
  [0xffec, "MetaRight", false],
];

describe("attachVncViewer", () => {
  afterEach(() => {
    vi.useRealTimers();
    vi.clearAllMocks();
    rfbMocks.MockRFB.instances = [];
  });

  it("releases held modifier and ordinary keys before disabling input, without reconnecting", async () => {
    const adapter = await attachVncViewer({ container: document.createElement("div"), url: "/api/v1/ws/vnc/session-1", token: "fixture" });
    const rfb = rfbMocks.MockRFB.instances[0]!;
    rfb.sendKey(0xffe1, "ShiftLeft", true);
    rfb.sendKey(0x61, "KeyA", true);
    rfb.sentKey.mockClear();
    adapter.setInputEnabled(false);
    expect(rfb.sentKey.mock.calls).toEqual([[0xffe1, "ShiftLeft", false], [0x61, "KeyA", false]]);
    expect(rfb.viewOnly).toBe(true);
    rfb.sentKey.mockClear();
    rfb.sendKey(0x62, "KeyB", true);
    adapter.focus();
    expect(rfb.sentKey).not.toHaveBeenCalled();
    expect(rfb.focus).not.toHaveBeenCalled();
    expect(rfb.disconnect).not.toHaveBeenCalled();
    adapter.setInputEnabled(true);
    rfb.sendKey(0x62, "KeyB", true);
    expect(rfb.sentKey).toHaveBeenCalledWith(0x62, "KeyB", true);
    adapter.dispose();
    expect(rfb.sentKey).toHaveBeenLastCalledWith(0x62, "KeyB", false);
  });

  it("cancels queued clipboard input when a pane becomes inactive", async () => {
    vi.useFakeTimers();
    const container = document.createElement("div");
    document.body.append(container);
    const controllerChanged = vi.fn();
    const adapter = await attachVncViewer({ container, url: "/api/v1/ws/vnc/session-1", token: "fixture", onClipboardControllerChange: controllerChanged });
    const rfb = rfbMocks.MockRFB.instances[0]!;
    const controller = controllerChanged.mock.calls[0]![0];
    controller.sendText("fixture clipboard");
    container.dispatchEvent(new KeyboardEvent("keydown", { key: "v", code: "KeyV", ctrlKey: true, bubbles: true, cancelable: true }));
    adapter.setInputEnabled(false);
    rfb.sentKey.mockClear();
    rfb.clipboardPasteFrom.mockClear();
    controller.sendText("must not send");
    vi.runAllTimers();
    expect(rfb.sentKey).not.toHaveBeenCalled();
    expect(rfb.clipboardPasteFrom).not.toHaveBeenCalled();
    adapter.dispose();
    container.remove();
  });

  it("enables noVNC resize modes and disposes the RFB instance", async () => {
    const container = document.createElement("div");
    container.innerHTML = "<span>old</span>";

    const adapter = await attachVncViewer({
      container,
      url: "/api/v1/ws/vnc/session-1?existing=1",
      token: "session-token",
      password: "secret",
    });

    const rfb = rfbMocks.MockRFB.instances[0];
    const url = new URL(rfb?.url ?? "");
    expect(url.pathname).toBe("/api/v1/ws/vnc/session-1");
    expect(url.searchParams.get("existing")).toBe("1");
    expect(url.searchParams.get("token")).toBe("session-token");
    expect(rfb?.options).toMatchObject({
      credentials: { password: "secret" },
      wsProtocols: ["binary", "base64"],
    });
    expect(rfb?.scaleViewport).toBe(true);
    expect(rfb?.resizeSession).toBe(true);
    expect((container.firstElementChild as HTMLElement | null)?.style.overflow).toBe("hidden");
    expect(container).toHaveAttribute("tabindex", "0");
    expect(container.querySelector("span")).toBeNull();
    expect(container.querySelector("canvas")).not.toBeNull();

    adapter.dispose();

    expect(rfb?.disconnect).toHaveBeenCalledOnce();
    expect(container.innerHTML).toBe("");
  });

  it("requests a noVNC viewport refresh when resized", async () => {
    const adapter = await attachVncViewer({
      container: document.createElement("div"),
      url: "/api/v1/ws/vnc/session-1",
      token: "session-token",
    });

    const rfb = rfbMocks.MockRFB.instances[0];
    adapter.resize();

    expect(rfb?.scaleViewportSet).toHaveBeenCalledTimes(2);
    expect(rfb?.scaleViewportSet).toHaveBeenLastCalledWith(true);
    expect(rfb?.resizeSessionSet).toHaveBeenCalledTimes(2);
    expect(rfb?.resizeSessionSet).toHaveBeenLastCalledWith(true);
  });

  it("focuses the noVNC RFB session instead of the wrapper element", async () => {
    const container = document.createElement("div");
    const adapter = await attachVncViewer({
      container,
      url: "/api/v1/ws/vnc/session-1",
      token: "session-token",
    });

    adapter.focus();
    const rfb = rfbMocks.MockRFB.instances[0];
    expect(rfb?.focus).toHaveBeenCalledWith({ preventScroll: true });

    container.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    expect(rfb?.focus).toHaveBeenCalledTimes(2);
    adapter.dispose();
  });

  it("forwards focused Ctrl+C shortcuts as remote VNC key events", async () => {
    const container = document.createElement("div");
    document.body.append(container);
    const adapter = await attachVncViewer({
      container,
      url: "/api/v1/ws/vnc/session-1",
      token: "session-token",
    });

    try {
      const event = new KeyboardEvent("keydown", {
        key: "c",
        code: "KeyC",
        ctrlKey: true,
        bubbles: true,
        cancelable: true,
      });
      container.dispatchEvent(event);

      expect(event.defaultPrevented).toBe(true);
      const rfb = rfbMocks.MockRFB.instances[0];
      expect(rfb?.sentKey.mock.calls).toEqual([
        [0xffe3, "ControlLeft", true],
        [0x0063, "KeyC", true],
        [0x0063, "KeyC", false],
        [0xffe3, "ControlLeft", false],
      ]);
    } finally {
      adapter.dispose();
      container.remove();
    }
  });

  it("forwards Ctrl+C by physical key code on non-Latin layouts", async () => {
    const container = document.createElement("div");
    document.body.append(container);
    const adapter = await attachVncViewer({
      container,
      url: "/api/v1/ws/vnc/session-1",
      token: "session-token",
    });

    try {
      const event = new KeyboardEvent("keydown", {
        key: "с",
        code: "KeyC",
        ctrlKey: true,
        bubbles: true,
        cancelable: true,
      });
      container.dispatchEvent(event);

      expect(event.defaultPrevented).toBe(true);
      const rfb = rfbMocks.MockRFB.instances[0];
      expect(rfb?.sentKey.mock.calls).toEqual([
        [0xffe3, "ControlLeft", true],
        [0x0063, "KeyC", true],
        [0x0063, "KeyC", false],
        [0xffe3, "ControlLeft", false],
      ]);
    } finally {
      adapter.dispose();
      container.remove();
    }
  });

  it("does not intercept Alt+Shift or Ctrl+Shift language-switch shortcuts", async () => {
    const container = document.createElement("div");
    document.body.append(container);
    const adapter = await attachVncViewer({
      container,
      url: "/api/v1/ws/vnc/session-1",
      token: "session-token",
    });

    try {
      const altShift = new KeyboardEvent("keydown", {
        key: "Shift",
        code: "ShiftLeft",
        altKey: true,
        shiftKey: true,
        bubbles: true,
        cancelable: true,
      });
      container.dispatchEvent(altShift);

      const ctrlShift = new KeyboardEvent("keydown", {
        key: "Shift",
        code: "ShiftRight",
        ctrlKey: true,
        shiftKey: true,
        bubbles: true,
        cancelable: true,
      });
      container.dispatchEvent(ctrlShift);

      const rfb = rfbMocks.MockRFB.instances[0];
      expect(altShift.defaultPrevented).toBe(false);
      expect(ctrlShift.defaultPrevented).toBe(false);
      expect(rfb?.languageSwitchKeyDown.mock.calls).toEqual([["ShiftLeft"], ["ShiftRight"]]);
    } finally {
      adapter.dispose();
      container.remove();
    }
  });

  it("syncs browser clipboard and sends one remote paste shortcut without typing text", async () => {
    vi.useFakeTimers();
    const originalClipboard = Object.getOwnPropertyDescriptor(navigator, "clipboard");
    const readText = vi.fn().mockResolvedValue("http://localhost:9080/connections");
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { readText },
    });

    const container = document.createElement("div");
    document.body.append(container);
    const adapter = await attachVncViewer({
      container,
      url: "/api/v1/ws/vnc/session-1",
      token: "session-token",
    });

    try {
      const event = new KeyboardEvent("keydown", {
        key: "v",
        code: "KeyV",
        ctrlKey: true,
        bubbles: true,
        cancelable: true,
      });
      container.dispatchEvent(event);

      expect(event.defaultPrevented).toBe(false);

      const rfb = rfbMocks.MockRFB.instances[0];
      await vi.advanceTimersByTimeAsync(75);
      expect(rfb?.clipboardPasteFrom).toHaveBeenCalledTimes(1);
      expect(rfb?.clipboardPasteFrom).toHaveBeenLastCalledWith(
        "http://localhost:9080/connections",
      );
      expect(rfb?.sentKey).not.toHaveBeenCalled();

      await vi.advanceTimersByTimeAsync(450);
      expect(rfb?.clipboardPasteFrom).toHaveBeenCalledTimes(3);
      expect(rfb?.sentKey).not.toHaveBeenCalled();

      await vi.advanceTimersByTimeAsync(200);
      expect(rfb?.sentKey.mock.calls).toEqual([
        ...remoteModifierReleaseCalls,
        [0xffe3, "ControlLeft", true],
        [0x0076, "KeyV", true],
        [0x0076, "KeyV", false],
        [0xffe3, "ControlLeft", false],
      ]);
    } finally {
      adapter.dispose();
      container.remove();
      if (originalClipboard) Object.defineProperty(navigator, "clipboard", originalClipboard);
      else Reflect.deleteProperty(navigator, "clipboard");
    }
  });

  it("syncs localized clipboard text without typing it as keysyms", async () => {
    vi.useFakeTimers();
    const originalClipboard = Object.getOwnPropertyDescriptor(navigator, "clipboard");
    const readText = vi.fn().mockResolvedValue("я");
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { readText },
    });

    const container = document.createElement("div");
    document.body.append(container);
    const adapter = await attachVncViewer({
      container,
      url: "/api/v1/ws/vnc/session-1",
      token: "session-token",
    });

    try {
      const event = new KeyboardEvent("keydown", {
        key: "м",
        code: "KeyV",
        ctrlKey: true,
        bubbles: true,
        cancelable: true,
      });
      container.dispatchEvent(event);

      const rfb = rfbMocks.MockRFB.instances[0];
      await vi.advanceTimersByTimeAsync(75);
      expect(rfb?.clipboardPasteFrom).toHaveBeenCalledTimes(1);
      expect(rfb?.clipboardPasteFrom).toHaveBeenLastCalledWith("я");
      expect(rfb?.sentKey).not.toHaveBeenCalled();

      await vi.advanceTimersByTimeAsync(650);
      expect(rfb?.sentKey.mock.calls).toEqual([
        ...remoteModifierReleaseCalls,
        [0xffe3, "ControlLeft", true],
        [0x0076, "KeyV", true],
        [0x0076, "KeyV", false],
        [0xffe3, "ControlLeft", false],
      ]);
    } finally {
      adapter.dispose();
      container.remove();
      if (originalClipboard) Object.defineProperty(navigator, "clipboard", originalClipboard);
      else Reflect.deleteProperty(navigator, "clipboard");
    }
  });

  it("syncs browser clipboard on Cmd+V", async () => {
    vi.useFakeTimers();
    const originalClipboard = Object.getOwnPropertyDescriptor(navigator, "clipboard");
    const readText = vi.fn().mockResolvedValue("mac paste");
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { readText },
    });

    const container = document.createElement("div");
    document.body.append(container);
    const adapter = await attachVncViewer({
      container,
      url: "/api/v1/ws/vnc/session-1",
      token: "session-token",
    });

    try {
      const event = new KeyboardEvent("keydown", {
        key: "v",
        code: "KeyV",
        metaKey: true,
        bubbles: true,
        cancelable: true,
      });
      container.dispatchEvent(event);

      const rfb = rfbMocks.MockRFB.instances[0];
      await vi.advanceTimersByTimeAsync(75);
      expect(rfb?.clipboardPasteFrom).toHaveBeenCalledWith("mac paste");
      expect(rfb?.sentKey).not.toHaveBeenCalled();

      await vi.advanceTimersByTimeAsync(650);
      expect(rfb?.sentKey.mock.calls).toEqual([
        ...remoteModifierReleaseCalls,
        [0xffe3, "ControlLeft", true],
        [0x0076, "KeyV", true],
        [0x0076, "KeyV", false],
        [0xffe3, "ControlLeft", false],
      ]);
    } finally {
      adapter.dispose();
      container.remove();
      if (originalClipboard) Object.defineProperty(navigator, "clipboard", originalClipboard);
      else Reflect.deleteProperty(navigator, "clipboard");
    }
  });

  it("resyncs clipboard before delayed remote paste shortcut", async () => {
    vi.useFakeTimers();
    const originalClipboard = Object.getOwnPropertyDescriptor(navigator, "clipboard");
    const readText = vi.fn().mockResolvedValue("unsupported\u0001paste");
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { readText },
    });

    const container = document.createElement("div");
    document.body.append(container);
    const adapter = await attachVncViewer({
      container,
      url: "/api/v1/ws/vnc/session-1",
      token: "session-token",
    });

    try {
      const event = new KeyboardEvent("keydown", {
        key: "v",
        code: "KeyV",
        ctrlKey: true,
        bubbles: true,
        cancelable: true,
      });
      container.dispatchEvent(event);

      const rfb = rfbMocks.MockRFB.instances[0];
      await vi.advanceTimersByTimeAsync(75);
      expect(rfb?.clipboardPasteFrom).toHaveBeenCalledTimes(1);
      expect(rfb?.sentKey).not.toHaveBeenCalled();

      await vi.advanceTimersByTimeAsync(450);
      expect(rfb?.clipboardPasteFrom).toHaveBeenCalledTimes(3);
      expect(rfb?.sentKey).not.toHaveBeenCalled();

      await vi.advanceTimersByTimeAsync(200);
      expect(rfb?.sentKey.mock.calls).toEqual([
        ...remoteModifierReleaseCalls,
        [0xffe3, "ControlLeft", true],
        [0x0076, "KeyV", true],
        [0x0076, "KeyV", false],
        [0xffe3, "ControlLeft", false],
      ]);
    } finally {
      adapter.dispose();
      container.remove();
      if (originalClipboard) Object.defineProperty(navigator, "clipboard", originalClipboard);
      else Reflect.deleteProperty(navigator, "clipboard");
    }
  });

  it("falls back to remote Ctrl+V when browser clipboard is unavailable", async () => {
    vi.useFakeTimers();
    const originalClipboard = Object.getOwnPropertyDescriptor(navigator, "clipboard");
    Reflect.deleteProperty(navigator, "clipboard");

    const container = document.createElement("div");
    document.body.append(container);
    const adapter = await attachVncViewer({
      container,
      url: "/api/v1/ws/vnc/session-1",
      token: "session-token",
    });

    try {
      const event = new KeyboardEvent("keydown", {
        key: "v",
        code: "KeyV",
        ctrlKey: true,
        bubbles: true,
        cancelable: true,
      });
      container.dispatchEvent(event);

      const rfb = rfbMocks.MockRFB.instances[0];
      expect(rfb?.sentKey).not.toHaveBeenCalled();
      expect(rfb?.clipboardPasteFrom).not.toHaveBeenCalled();

      await vi.advanceTimersByTimeAsync(750);
      expect(rfb?.sentKey.mock.calls).toEqual([
        ...remoteModifierReleaseCalls,
        [0xffe3, "ControlLeft", true],
        [0x0076, "KeyV", true],
        [0x0076, "KeyV", false],
        [0xffe3, "ControlLeft", false],
      ]);
    } finally {
      adapter.dispose();
      container.remove();
      if (originalClipboard) Object.defineProperty(navigator, "clipboard", originalClipboard);
    }
  });

  it("exposes a clipboard controller backed by noVNC clipboardPasteFrom", async () => {
    const onClipboardControllerChange = vi.fn();

    const adapter = await attachVncViewer({
      container: document.createElement("div"),
      url: "/api/v1/ws/vnc/session-1",
      token: "session-token",
      onClipboardControllerChange,
    });

    const controller = onClipboardControllerChange.mock.calls[0]?.[0];
    controller.sendText("local clipboard");

    const rfb = rfbMocks.MockRFB.instances[0];
    expect(rfb?.clipboardPasteFrom).toHaveBeenCalledWith("local clipboard");

    adapter.dispose();
    expect(onClipboardControllerChange).toHaveBeenLastCalledWith(null);
  });

  it("forwards remote noVNC clipboard text", async () => {
    const onRemoteClipboardText = vi.fn();

    await attachVncViewer({
      container: document.createElement("div"),
      url: "/api/v1/ws/vnc/session-1",
      token: "session-token",
      onRemoteClipboardText,
    });

    const rfb = rfbMocks.MockRFB.instances[0];
    rfb?.emit("clipboard", { text: "remote clipboard" });

    expect(onRemoteClipboardText).toHaveBeenCalledWith("remote clipboard");
  });
});
