/**
 * Shared helpers for Guacamole-backed viewers. Keeps `rdpAdapter.ts` thin and
 * avoids duplicate input wiring when additional Guacamole protocols are added.
 */

import Guacamole from "guacamole-common-js";

import type { RdpFilesystem } from "./rdpFilesystem";
import type { RemoteClipboardController, RemoteClipboardTextHandler } from "./remoteClipboard";

export interface GuacAdapterOptions {
  container: HTMLElement;
  /** Absolute `wss://` URL from the backend, including any existing query. */
  url: string;
  /** Per-session token issued by `POST /api/v1/connections/sessions`. */
  token: string;
  readOnly?: boolean;
  width?: number;
  height?: number;
  onOpen?: () => void;
  onClose?: (code?: number, reason?: string) => void;
  /**
   * Fired when the remote framebuffer changes size. The viewer uses it to
   * re-fit the display to the panel after a remote resize or a VNC server's
   * initial size instruction.
   */
  onDisplayResize?: (width: number, height: number) => void;
  /**
   * The tunnel's raw WebSocket close pair, before Guacamole folds it into a
   * status code. Only the observer lease contract needs it: the vendor keeps
   * just the reason when a close carries one, so the code must be captured at
   * the transport.
   */
  onWireClose?: (code: number, reason: string) => void;
  onClipboardControllerChange?: (controller: RemoteClipboardController | null) => void;
  onRemoteClipboardText?: RemoteClipboardTextHandler;
  /**
   * Invoked when the session exposes a redirected drive (RDP only, when the
   * backend launches with `enable-drive`). The provided filesystem can browse,
   * download, and upload files on that drive.
   */
  onFilesystem?: (fs: RdpFilesystem) => void;
  /**
   * VNC only: the session may never send a client-side cursor, leaving the
   * pointer invisible while the display forces `cursor: none`. Show a local
   * marker until a non-empty cursor image arrives (and again if it disappears).
   */
  fallbackCursor?: boolean;
}

export interface GuacAdapter {
  dispose(): void;
  resize(w: number, h: number): void;
  /** Scale the rendered framebuffer without changing its remote resolution. */
  setDisplayScale(scale: number): void;
  /** Current remote framebuffer size; zero before the first `size` instruction. */
  getDisplaySize(): { width: number; height: number };
}

function cloneMouseState(state: Guacamole.Mouse.State): Guacamole.Mouse.State {
  return new Guacamole.Mouse.State(
    state.x,
    state.y,
    state.left,
    state.middle,
    state.right,
    state.up,
    state.down,
  );
}

function cloneTouchState(state: Guacamole.Touch.State): Guacamole.Touch.State {
  return new Guacamole.Touch.State(state);
}

/**
 * Build the tuple expected by Guacamole.WebSocketTunnel.
 *
 * guacamole-common-js appends `?${connectData}` itself when `client.connect()`
 * is called. Putting the token directly in `tunnelUrl` would produce
 * `/api/v1/ws/rdp/id?token=abc?width=...`, so all query parameters must live in
 * `connectData`.
 */
export function buildGuacTunnelConnection(
  url: string,
  token: string,
): { tunnelUrl: string; connectData: URLSearchParams } {
  const parsed = /^wss?:\/\//i.test(url)
    ? new URL(url)
    : new URL(
      url,
      typeof window !== "undefined"
        ? `${window.location.protocol === "https:" ? "wss:" : "ws:"}//${window.location.host}`
        : "ws://localhost",
    );
  const connectData = new URLSearchParams(parsed.search);
  if (token) connectData.set("token", token);
  parsed.search = "";
  return { tunnelUrl: parsed.toString(), connectData };
}

/**
 * Connect a Guacamole tunnel and hand back its raw WebSocket close pair.
 *
 * `Guacamole.WebSocketTunnel` collapses any close that carries a reason into
 * `Status(parseInt(reason), reason)`, so the wire code is lost before the
 * client sees it. The tracked package patch
 * (`patches/guacamole-common-js@1.5.0.patch`) exposes the original pair on
 * `tunnel.onwsclose`, called from the tunnel's own `onclose` handler before the
 * status mapping. The connection itself still runs through
 * `Guacamole.Client.connect`; this only subscribes the adapter.
 */
export interface WireCloseObservable {
  onwsclose?: ((code: number, reason: string) => void) | null;
}

/** Subscribe to the tunnel's raw WebSocket close pair, if a caller needs it. */
export function observeTunnelWireClose(
  tunnel: Guacamole.WebSocketTunnel,
  onWireClose?: (code: number, reason: string) => void,
): void {
  if (!onWireClose) return;
  (tunnel as Guacamole.WebSocketTunnel & WireCloseObservable).onwsclose = onWireClose;
}

/** Wires keyboard, mouse, and touch inputs into a Guacamole client. */
export function attachGuacInputs(
  client: Guacamole.Client,
  container: HTMLElement,
): { dispose(): void } {
  const display = client.getDisplay();
  const displayEl = display.getElement();

  // Keep keyboard ownership scoped to the viewer. A document-level keyboard
  // steals input from workspace search/forms after an RDP session is opened,
  // especially inside the Tauri desktop shell.
  const keyboard = new Guacamole.Keyboard(container);
  keyboard.onkeydown = (keysym) => {
    client.sendKeyEvent(1, keysym);
    return false;
  };
  keyboard.onkeyup = (keysym) => {
    client.sendKeyEvent(0, keysym);
  };

  const mouse = new Guacamole.Mouse(displayEl);
  let pendingMouseMove: Guacamole.Mouse.State | null = null;
  let mouseMoveRaf: number | undefined;

  const flushMouseMove = () => {
    const state = pendingMouseMove;
    pendingMouseMove = null;
    if (state) client.sendMouseState(state, true);
  };

  const queueMouseMove = (state: Guacamole.Mouse.State) => {
    pendingMouseMove = cloneMouseState(state);
    if (typeof window === "undefined") {
      flushMouseMove();
      return;
    }
    if (mouseMoveRaf !== undefined) return;
    mouseMoveRaf = window.requestAnimationFrame(() => {
      mouseMoveRaf = undefined;
      flushMouseMove();
    });
  };

  // @types/guacamole-common-js types onEach with the DOM TargetListener signature,
  // but the runtime emits Guacamole.Mouse.Event / Touch.Event. Cast through unknown
  // since the lib's own JS clearly emits its own Event subclasses.
  mouse.onEach(["mousedown", "mouseup"], ((e: Guacamole.Mouse.Event) => {
    flushMouseMove();
    client.sendMouseState(e.state, true);
  }) as unknown as Parameters<typeof mouse.onEach>[1]);
  mouse.onEach(["mousemove"], ((e: Guacamole.Mouse.Event) => {
    queueMouseMove(e.state);
  }) as unknown as Parameters<typeof mouse.onEach>[1]);

  const touch = new Guacamole.Touch(displayEl);
  let pendingTouchMove: Guacamole.Touch.State | null = null;
  let touchMoveRaf: number | undefined;

  const flushTouchMove = () => {
    const state = pendingTouchMove;
    pendingTouchMove = null;
    if (state) client.sendTouchState(state, true);
  };

  const queueTouchMove = (state: Guacamole.Touch.State) => {
    pendingTouchMove = cloneTouchState(state);
    if (typeof window === "undefined") {
      flushTouchMove();
      return;
    }
    if (touchMoveRaf !== undefined) return;
    touchMoveRaf = window.requestAnimationFrame(() => {
      touchMoveRaf = undefined;
      flushTouchMove();
    });
  };

  touch.onEach(["touchstart", "touchend"], ((e: Guacamole.Touch.Event) => {
    flushTouchMove();
    client.sendTouchState(e.state, true);
  }) as unknown as Parameters<typeof touch.onEach>[1]);
  touch.onEach(["touchmove"], ((e: Guacamole.Touch.Event) => {
    queueTouchMove(e.state);
  }) as unknown as Parameters<typeof touch.onEach>[1]);

  // The element should accept keyboard focus so that click-to-type works.
  if (!container.hasAttribute("tabindex")) {
    container.setAttribute("tabindex", "0");
  }

  const focusContainer = () => {
    container.focus({ preventScroll: true });
  };
  const releaseKeyboardFocus = () => {
    try {
      keyboard.reset();
    } catch {
      // ignore
    }
    if (container.ownerDocument.activeElement === container) {
      container.blur();
    }
  };
  displayEl.addEventListener("mousedown", focusContainer, true);
  displayEl.addEventListener("touchstart", focusContainer, true);
  container.addEventListener("mouseleave", releaseKeyboardFocus);
  container.addEventListener("blur", releaseKeyboardFocus);

  return {
    dispose(): void {
      displayEl.removeEventListener("mousedown", focusContainer, true);
      displayEl.removeEventListener("touchstart", focusContainer, true);
      container.removeEventListener("mouseleave", releaseKeyboardFocus);
      container.removeEventListener("blur", releaseKeyboardFocus);
      keyboard.onkeydown = null;
      keyboard.onkeyup = null;
      // Guacamole's Keyboard listens on the document; reset() releases any
      // held keys and fires keyup so the server doesn't think shift is
      // still down after the viewer closes.
      try {
        keyboard.reset();
      } catch {
        // ignore
      }
      if (typeof window !== "undefined") {
        if (mouseMoveRaf !== undefined) window.cancelAnimationFrame(mouseMoveRaf);
        if (touchMoveRaf !== undefined) window.cancelAnimationFrame(touchMoveRaf);
      }
      mouseMoveRaf = undefined;
      touchMoveRaf = undefined;
      pendingMouseMove = null;
      pendingTouchMove = null;
    },
  };
}
