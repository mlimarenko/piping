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
    public disconnect = vi.fn();
    public createClipboardStream = vi.fn((mimetype: string) => ({ mimetype }));
    public sendSize = vi.fn();
    public sendKeyEvent = vi.fn();
    public sendMouseState = vi.fn();
    public sendTouchState = vi.fn();
    private displayEl = document.createElement("div");

    constructor() {
      MockClient.instances.push(this);
    }

    getDisplay() {
      return {
        getElement: () => this.displayEl,
      };
    }
  }

  class MockWebSocketTunnel {
    constructor(public url: string) {}
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
