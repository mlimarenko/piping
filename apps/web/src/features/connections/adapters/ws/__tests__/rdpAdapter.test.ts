import { describe, expect, it, vi, afterEach } from "vitest";

interface MockStream {
  sendAck: ReturnType<typeof vi.fn>;
  stringReader?: {
    emitText(text: string): void;
    end(): void;
  };
}

const guacMocks = vi.hoisted(() => {
  class MockStringReader {
    public ontext: ((text: string) => void) | null = null;
    public onend: (() => void) | null = null;

    constructor(stream: MockStream) {
      stream.stringReader = this;
    }

    emitText(text: string): void {
      this.ontext?.(text);
    }

    end(): void {
      this.onend?.();
    }
  }

  class MockStringWriter {
    public static instances: MockStringWriter[] = [];
    public onack: ((status: { isError(): boolean }) => void) | null = null;
    public sendText = vi.fn();
    public sendEnd = vi.fn();

    constructor(public stream: unknown) {
      MockStringWriter.instances.push(this);
    }
  }

  class MockClient {
    public static instances: MockClient[] = [];
    public static State = {
      CONNECTED: 3,
      DISCONNECTED: 5,
    };
    public onfilesystem: unknown = null;
    public onclipboard: ((stream: MockStream, mimetype: string) => void) | null = null;
    public onerror: unknown = null;
    public onstatechange: unknown = null;
    public connect = vi.fn();
    public disconnect = vi.fn(() => { this.tunnel.sendMessage("disconnect"); this.tunnel.disconnect(); });
    public createClipboardStream = vi.fn((mimetype: string) => ({ mimetype }));
    public sendSize = vi.fn();
    public sendKeyEvent = vi.fn();
    public sendMouseState = vi.fn();
    public sendTouchState = vi.fn();
    private displayEl = document.createElement("div");
    public display = {
      oncursor: null as null | ((canvas: HTMLCanvasElement | null, x: number, y: number) => void),
      getElement: () => this.displayEl,
    };

    constructor(private tunnel: MockWebSocketTunnel) {
      MockClient.instances.push(this);
    }

    getDisplay() {
      return this.display;
    }
  }

  class MockWebSocketTunnel {
    public static instances: MockWebSocketTunnel[] = [];
    public onwsclose: ((code: number, reason: string) => void) | null = null;
    public disconnect = vi.fn();
    public rawSend = vi.fn();
    public sendMessage = this.rawSend;
    constructor(public url: string) { MockWebSocketTunnel.instances.push(this); }
  }

  class MockKeyboard {
    public onkeydown: unknown = null;
    public onkeyup: unknown = null;
    public reset = vi.fn();
  }

  class MockMouse {
    public onEach = vi.fn();
  }

  class MockTouch {
    public onEach = vi.fn();
  }

  return {
    MockClient,
    MockKeyboard,
    MockStringReader,
    MockMouse,
    MockStringWriter,
    MockTouch,
    MockWebSocketTunnel,
  };
});

vi.mock("guacamole-common-js", () => ({
  default: {
    WebSocketTunnel: guacMocks.MockWebSocketTunnel,
    Client: guacMocks.MockClient,
    Keyboard: guacMocks.MockKeyboard,
    Mouse: guacMocks.MockMouse,
    Touch: guacMocks.MockTouch,
    StringReader: guacMocks.MockStringReader,
    StringWriter: guacMocks.MockStringWriter,
    Object: {
      ROOT_STREAM: "/",
      STREAM_INDEX_MIMETYPE: "application/vnd.glyptodon.guacamole.stream-index+json",
    },
    ArrayBufferWriter: {
      DEFAULT_BLOB_LENGTH: 6048,
    },
    Status: {
      Code: {
        SUCCESS: 0,
        UNSUPPORTED: 0x0100,
      },
    },
  },
}));

import { attachRdpViewer } from "../rdpAdapter";

describe("attachRdpViewer clipboard", () => {
  afterEach(() => {
    vi.clearAllMocks();
    guacMocks.MockClient.instances = [];
    guacMocks.MockStringWriter.instances = [];
    guacMocks.MockWebSocketTunnel.instances = [];
  });

  it("observation exposes no input, clipboard, filesystem or remote resize and closes only the tunnel", () => {
    const onClipboardControllerChange = vi.fn();
    const onFilesystem = vi.fn();
    const adapter = attachRdpViewer({ container: document.createElement("div"), url: "/api/v1/ws/observe/session-1", token: "", readOnly: true, width: 800, height: 600, onClipboardControllerChange, onFilesystem });
    const client = guacMocks.MockClient.instances[0]!;
    const tunnel = guacMocks.MockWebSocketTunnel.instances[0]!;
    // The client still owns the handshake; the adapter only subscribes.
    expect(client.connect).toHaveBeenCalledWith("");
    expect(client.onfilesystem).toBeNull();
    expect(client.onclipboard).toBeNull();
    expect(tunnel.onwsclose).toBeNull();
    expect(onClipboardControllerChange).not.toHaveBeenCalled();
    adapter.resize(800, 600);
    expect(client.sendSize).not.toHaveBeenCalled();
    tunnel.sendMessage("key", 1, 65);
    tunnel.sendMessage("size", 800, 600);
    tunnel.sendMessage("clipboard", 1, "text/plain");
    tunnel.sendMessage("", "ping", 12345);
    tunnel.sendMessage("", "ping", Infinity);
    tunnel.sendMessage("", "ping", NaN);
    tunnel.sendMessage("", "ping", "12345");
    tunnel.sendMessage("", "ping", 12345, "extra");
    tunnel.sendMessage("", "disconnect", 12345);
    tunnel.sendMessage("");
    tunnel.sendMessage("sync", 1);
    expect(tunnel.rawSend).toHaveBeenCalledTimes(2);
    expect(tunnel.rawSend).toHaveBeenCalledWith("", "ping", 12345);
    expect(tunnel.rawSend).toHaveBeenCalledWith("sync", 1);
    adapter.dispose();
    expect(client.disconnect).toHaveBeenCalledOnce();
    expect(tunnel.disconnect).toHaveBeenCalledOnce();
    expect(tunnel.rawSend).toHaveBeenCalledTimes(2);
    expect(onFilesystem).not.toHaveBeenCalled();
  });

  it("forwards the tunnel's raw close pair to an observation and stops after dispose", () => {
    const onWireClose = vi.fn();
    const adapter = attachRdpViewer({
      container: document.createElement("div"),
      url: "/api/v1/ws/observe/session-1",
      token: "",
      readOnly: true,
      onWireClose,
    });
    const client = guacMocks.MockClient.instances[0]!;
    const tunnel = guacMocks.MockWebSocketTunnel.instances[0]!;
    expect(client.connect).toHaveBeenCalledWith("");

    tunnel.onwsclose?.(1000, "observer_lease_expired");
    expect(onWireClose).toHaveBeenCalledExactlyOnceWith(1000, "observer_lease_expired");

    // A 1008 with the same text stays a distinct pair at the adapter boundary.
    tunnel.onwsclose?.(1008, "observer_lease_expired");
    expect(onWireClose).toHaveBeenLastCalledWith(1008, "observer_lease_expired");

    adapter.dispose();
    tunnel.onwsclose?.(1000, "observer_lease_expired");
    expect(onWireClose).toHaveBeenCalledTimes(2);
  });

  it("exposes a controller that sends text through a Guacamole clipboard stream", () => {
    const onClipboardControllerChange = vi.fn();

    const adapter = attachRdpViewer({
      container: document.createElement("div"),
      url: "/api/v1/ws/rdp/session-1",
      token: "token",
      onClipboardControllerChange,
    });

    const controller = onClipboardControllerChange.mock.calls[0]?.[0];
    controller.sendText("local text");

    const client = guacMocks.MockClient.instances[0];
    const writer = guacMocks.MockStringWriter.instances[0];
    expect(client?.createClipboardStream).toHaveBeenCalledWith("text/plain");
    expect(writer?.sendText).toHaveBeenCalledWith("local text");
    expect(writer?.sendEnd).toHaveBeenCalledOnce();

    adapter.dispose();
    expect(onClipboardControllerChange).toHaveBeenLastCalledWith(null);
  });

  it("syncs browser clipboard and sends remote paste on Ctrl+V", async () => {
    const originalClipboard = Object.getOwnPropertyDescriptor(navigator, "clipboard");
    const readText = vi.fn().mockResolvedValue("shortcut clipboard");
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { readText },
    });

    const container = document.createElement("div");
    container.tabIndex = 0;
    document.body.append(container);
    container.focus();
    const adapter = attachRdpViewer({
      container,
      url: "/api/v1/ws/rdp/session-1",
      token: "token",
    });

    try {
      const event = new KeyboardEvent("keydown", {
        key: "v",
        ctrlKey: true,
        bubbles: true,
        cancelable: true,
      });
      container.dispatchEvent(event);

      expect(event.defaultPrevented).toBe(false);
      await vi.waitFor(() => {
        const writer = guacMocks.MockStringWriter.instances[0];
        expect(writer?.sendText).toHaveBeenCalledWith("shortcut clipboard");
      });

      const client = guacMocks.MockClient.instances[0];
      expect(client?.sendKeyEvent.mock.calls.slice(-4)).toEqual([
        [1, 0xffe3],
        [1, 0x0076],
        [0, 0x0076],
        [0, 0xffe3],
      ]);
    } finally {
      adapter.dispose();
      container.remove();
      if (originalClipboard) Object.defineProperty(navigator, "clipboard", originalClipboard);
      else Reflect.deleteProperty(navigator, "clipboard");
    }
  });

  it("keeps Guacamole display layers above the viewer background and hides the browser cursor", () => {
    const container = document.createElement("div");

    attachRdpViewer({
      container,
      url: "/api/v1/ws/rdp/session-1",
      token: "token",
    });

    const displayEl = container.firstElementChild as HTMLElement | null;
    expect(displayEl).not.toBeNull();
    expect(displayEl?.style.position).toBe("relative");
    expect(displayEl?.style.zIndex).toBe("0");
    expect(displayEl?.style.isolation).toBe("isolate");
    expect(displayEl?.style.cursor).toBe("none");
  });

  it("sends dynamic viewer size changes to guacamole", () => {
    const adapter = attachRdpViewer({
      container: document.createElement("div"),
      url: "/api/v1/ws/rdp/session-1",
      token: "token",
    });

    adapter.resize(1366, 768);

    const client = guacMocks.MockClient.instances[0];
    expect(client?.sendSize).toHaveBeenCalledWith(1366, 768);
  });

  it("reads inbound text/plain clipboard streams with StringReader", () => {
    const onRemoteClipboardText = vi.fn();
    attachRdpViewer({
      container: document.createElement("div"),
      url: "/api/v1/ws/rdp/session-1",
      token: "token",
      onRemoteClipboardText,
    });

    const client = guacMocks.MockClient.instances[0];
    const stream: MockStream = { sendAck: vi.fn() };
    client?.onclipboard?.(stream, "text/plain");
    stream.stringReader?.emitText("remote ");
    stream.stringReader?.emitText("clipboard");
    stream.stringReader?.end();

    expect(onRemoteClipboardText).toHaveBeenCalledWith("remote clipboard");
  });

  it("rejects unsupported inbound clipboard MIME types", () => {
    attachRdpViewer({
      container: document.createElement("div"),
      url: "/api/v1/ws/rdp/session-1",
      token: "token",
    });

    const client = guacMocks.MockClient.instances[0];
    const stream: MockStream = { sendAck: vi.fn() };
    client?.onclipboard?.(stream, "image/png");

    expect(stream.sendAck).toHaveBeenCalledWith("Unsupported clipboard type", 0x0100);
  });
});

describe("attachRdpViewer cursor fallback", () => {
  function cursorCanvas(width: number, height: number, visible = false): HTMLCanvasElement {
    const canvas = document.createElement("canvas");
    canvas.width = width;
    canvas.height = height;
    const data = new Uint8ClampedArray(width * height * 4);
    if (visible && data.length > 0) data[data.length - 1] = 255;
    vi.spyOn(canvas, "getContext").mockReturnValue({
      getImageData: () => ({ data }),
    } as unknown as CanvasRenderingContext2D);
    return canvas;
  }

  afterEach(() => {
    vi.clearAllMocks();
    guacMocks.MockClient.instances = [];
    guacMocks.MockWebSocketTunnel.instances = [];
  });

  it("shows a local marker until the VNC session provides its own cursor", () => {
    const container = document.createElement("div");
    attachRdpViewer({ container, url: "/api/v1/ws/vnc/session-1", token: "", width: 800, height: 600, fallbackCursor: true });
    const el = container.firstElementChild as HTMLElement;
    expect(el.style.cursor).toContain("data:image/svg+xml");
    const display = guacMocks.MockClient.instances[0]!.display;
    display.oncursor?.(cursorCanvas(24, 24, true), 0, 0);
    expect(el.style.cursor).toBe("none");
    // The session hiding its client-side cursor again must bring the marker back.
    display.oncursor?.(cursorCanvas(0, 0), 0, 0);
    expect(el.style.cursor).toContain("data:image/svg+xml");
  });

  it("keeps the VNC pointer visible when a reconnected server sends a transparent cursor image", () => {
    const container = document.createElement("div");
    const old = attachRdpViewer({ container, url: "/api/v1/ws/vnc/first", token: "", width: 1280, height: 800, fallbackCursor: true });
    const oldDisplay = guacMocks.MockClient.instances[0]!.display;
    oldDisplay.oncursor?.(cursorCanvas(24, 24, true), 4, 6);
    old.dispose();
    attachRdpViewer({ container, url: "/api/v1/ws/vnc/reconnected", token: "", width: 800, height: 600, fallbackCursor: true });
    const display = guacMocks.MockClient.instances[1]!.display;
    const element = container.firstElementChild as HTMLElement;
    display.oncursor?.(cursorCanvas(24, 24), 0, 0);
    expect(element.style.cursor).toContain("data:image/svg+xml");
    display.oncursor?.(cursorCanvas(24, 24, true), 4, 6);
    expect(element.style.cursor).toBe("none");
  });

  it("releases the old VNC cursor callback when the connection is disposed", () => {
    const adapter = attachRdpViewer({ container: document.createElement("div"), url: "/api/v1/ws/vnc/first", token: "", fallbackCursor: true });
    const display = guacMocks.MockClient.instances[0]!.display;
    expect(display.oncursor).not.toBeNull();
    adapter.dispose();
    expect(display.oncursor).toBeNull();
  });

  it("retains the VNC marker when cursor pixels cannot be read", () => {
    const container = document.createElement("div");
    attachRdpViewer({ container, url: "/api/v1/ws/vnc/first", token: "", fallbackCursor: true });
    const canvas = cursorCanvas(24, 24, true);
    vi.spyOn(canvas, "getContext").mockImplementation(() => { throw new DOMException("Unreadable cursor", "SecurityError"); });
    guacMocks.MockClient.instances[0]!.display.oncursor?.(canvas, 0, 0);
    expect((container.firstElementChild as HTMLElement).style.cursor).toContain("data:image/svg+xml");
  });

  it("keeps the local cursor hidden when the protocol always provides one", () => {
    const container = document.createElement("div");
    attachRdpViewer({ container, url: "/api/v1/ws/rdp/session-1", token: "", width: 800, height: 600 });
    const el = container.firstElementChild as HTMLElement;
    expect(el.style.cursor).toBe("none");
    expect(guacMocks.MockClient.instances[0]!.display.oncursor).toBeNull();
  });
});
