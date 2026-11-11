import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const guacMocks = vi.hoisted(() => {
  class MockMouseState {
    constructor(
      public x: number,
      public y: number,
      public left: boolean,
      public middle: boolean,
      public right: boolean,
      public up: boolean,
      public down: boolean,
    ) {}
  }

  class MockTouchState {
    id = 0;
    radiusX = 0;
    radiusY = 0;
    angle = 0;
    force = 1;

    constructor(template?: Partial<MockTouchState>) {
      Object.assign(this, template);
    }
  }

  class MockKeyboard {
    static instances: MockKeyboard[] = [];
    element: HTMLDocument | HTMLElement | undefined;
    onkeydown: ((keysym: number) => boolean) | null = null;
    onkeyup: ((keysym: number) => void) | null = null;
    reset = vi.fn();

    constructor(element?: HTMLDocument | HTMLElement) {
      this.element = element;
      MockKeyboard.instances.push(this);
    }
  }

  class MockMouse {
    static State = MockMouseState;
    static instances: MockMouse[] = [];
    handlers: Array<{ types: string[]; cb: (event: { state: MockMouseState }) => void }> = [];

    constructor() {
      MockMouse.instances.push(this);
    }

    onEach(types: string[], cb: (event: { state: MockMouseState }) => void) {
      this.handlers.push({ types, cb });
    }

    emit(type: string, state: MockMouseState) {
      this.handlers
        .filter((handler) => handler.types.includes(type))
        .forEach((handler) => handler.cb({ state }));
    }
  }

  class MockTouch {
    static State = MockTouchState;
    static instances: MockTouch[] = [];
    handlers: Array<{ types: string[]; cb: (event: { state: MockTouchState }) => void }> = [];

    constructor() {
      MockTouch.instances.push(this);
    }

    onEach(types: string[], cb: (event: { state: MockTouchState }) => void) {
      this.handlers.push({ types, cb });
    }

    emit(type: string, state: MockTouchState) {
      this.handlers
        .filter((handler) => handler.types.includes(type))
        .forEach((handler) => handler.cb({ state }));
    }
  }

  return {
    MockKeyboard,
    MockMouse,
    MockMouseState,
    MockTouch,
    MockTouchState,
  };
});

vi.mock("guacamole-common-js", () => ({
  default: {
    Keyboard: guacMocks.MockKeyboard,
    Mouse: guacMocks.MockMouse,
    Touch: guacMocks.MockTouch,
  },
}));

import { attachGuacInputs, buildGuacTunnelConnection } from "../guacShared";

function makeClient() {
  const displayEl = document.createElement("div");
  return {
    client: {
      getDisplay: () => ({ getElement: () => displayEl }),
      sendKeyEvent: vi.fn(),
      sendMouseState: vi.fn(),
      sendTouchState: vi.fn(),
    },
    displayEl,
  };
}

describe("buildGuacTunnelConnection", () => {
  it("keeps WebSocketTunnel URL query-free and moves token into connect data", () => {
    const result = buildGuacTunnelConnection("wss://edge.example/api/v1/ws/rdp/1?trace=abc", "secret");

    const tunnelUrl = new URL(result.tunnelUrl);
    expect(tunnelUrl.origin).toBe("wss://edge.example");
    expect(tunnelUrl.pathname).toBe("/api/v1/ws/rdp/1");
    expect(tunnelUrl.search).toBe("");
    expect(result.connectData.get("trace")).toBe("abc");
    expect(result.connectData.get("token")).toBe("secret");
  });

  it("resolves relative tunnel URLs directly to the browser WebSocket origin", () => {
    const result = buildGuacTunnelConnection("/api/v1/ws/rdp/session-id", "token");
    const tunnelUrl = new URL(result.tunnelUrl);

    expect(tunnelUrl.protocol).toMatch(/^wss?:$/);
    expect(tunnelUrl.host).toBe(window.location.host);
    expect(tunnelUrl.pathname).toBe("/api/v1/ws/rdp/session-id");
    expect(result.connectData.get("token")).toBe("token");
  });
});

describe("attachGuacInputs", () => {
  beforeEach(() => {
    guacMocks.MockKeyboard.instances.length = 0;
    guacMocks.MockMouse.instances.length = 0;
    guacMocks.MockTouch.instances.length = 0;
    vi.useFakeTimers();
    vi.spyOn(window, "requestAnimationFrame").mockImplementation((cb) =>
      window.setTimeout(() => cb(0), 16),
    );
    vi.spyOn(window, "cancelAnimationFrame").mockImplementation((id) => {
      window.clearTimeout(id);
    });
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("coalesces mouse moves to the latest state per animation frame", () => {
    const { client } = makeClient();
    attachGuacInputs(client as never, document.createElement("div"));
    const mouse = guacMocks.MockMouse.instances[0]!;

    mouse.emit("mousemove", new guacMocks.MockMouseState(1, 1, false, false, false, false, false));
    mouse.emit("mousemove", new guacMocks.MockMouseState(2, 3, false, false, false, false, false));
    expect(client.sendMouseState).not.toHaveBeenCalled();

    vi.advanceTimersByTime(16);

    expect(client.sendMouseState).toHaveBeenCalledTimes(1);
    expect(client.sendMouseState.mock.calls[0]![0]).toMatchObject({ x: 2, y: 3 });
  });

  it("scopes keyboard events to the viewer container", () => {
    const { client } = makeClient();
    const container = document.createElement("div");
    attachGuacInputs(client as never, container);

    expect(guacMocks.MockKeyboard.instances[0]!.element).toBe(container);
  });

  it("releases viewer keyboard state when pointer leaves the viewer", () => {
    const { client } = makeClient();
    const container = document.createElement("div");
    document.body.append(container);
    attachGuacInputs(client as never, container);
    container.focus();

    container.dispatchEvent(new MouseEvent("mouseleave"));

    expect(guacMocks.MockKeyboard.instances[0]!.reset).toHaveBeenCalled();
    expect(document.activeElement).not.toBe(container);
  });

  it("keeps mouse down/up immediate and ordered after a pending move", () => {
    const { client } = makeClient();
    attachGuacInputs(client as never, document.createElement("div"));
    const mouse = guacMocks.MockMouse.instances[0]!;

    mouse.emit("mousemove", new guacMocks.MockMouseState(4, 5, false, false, false, false, false));
    mouse.emit("mousedown", new guacMocks.MockMouseState(4, 5, true, false, false, false, false));

    expect(client.sendMouseState).toHaveBeenCalledTimes(2);
    expect(client.sendMouseState.mock.calls[0]![0]).toMatchObject({ x: 4, y: 5, left: false });
    expect(client.sendMouseState.mock.calls[1]![0]).toMatchObject({ x: 4, y: 5, left: true });

    vi.advanceTimersByTime(16);
    expect(client.sendMouseState).toHaveBeenCalledTimes(2);
  });

  it("coalesces touch moves while keeping touch start/end immediate", () => {
    const { client } = makeClient();
    attachGuacInputs(client as never, document.createElement("div"));
    const touch = guacMocks.MockTouch.instances[0]!;

    touch.emit("touchmove", new guacMocks.MockTouchState({ id: 1, force: 0.5 }));
    touch.emit("touchmove", new guacMocks.MockTouchState({ id: 1, force: 0.7 }));
    expect(client.sendTouchState).not.toHaveBeenCalled();

    touch.emit("touchend", new guacMocks.MockTouchState({ id: 1, force: 0 }));

    expect(client.sendTouchState).toHaveBeenCalledTimes(2);
    expect(client.sendTouchState.mock.calls[0]![0]).toMatchObject({ id: 1, force: 0.7 });
    expect(client.sendTouchState.mock.calls[1]![0]).toMatchObject({ id: 1, force: 0 });
  });
});
