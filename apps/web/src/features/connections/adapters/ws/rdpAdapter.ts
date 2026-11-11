/**
 * RDP viewer using guacamole-common-js. Routes through the backend's
 * `/api/v1/ws/rdp/{session_id}` endpoint, which speaks the Guacamole protocol over a
 * WebSocket with subprotocol "guacamole" (see apps/api/src/proxy/router.rs).
 *
 * The guac client owns its own tunnel and display; this adapter wires the
 * tunnel's URL (with query token), mounts the generated DOM element into
 * the caller's container, and plugs in keyboard/mouse/touch inputs.
 */

import Guacamole from "guacamole-common-js";

import {
  attachRemoteClipboardPaste,
  type RemoteClipboardController,
} from "./remoteClipboard";
import {
  attachGuacInputs,
  buildGuacTunnelConnection,
  observeTunnelWireClose,
  type GuacAdapter,
  type GuacAdapterOptions,
} from "./guacShared";
import { createRdpFilesystem } from "./rdpFilesystem";

export type { GuacAdapter, GuacAdapterOptions } from "./guacShared";

const GUAC_KEYSYM_CONTROL_L = 0xffe3;
const GUAC_KEYSYM_V = 0x0076;

function keepGuacDisplayLayersAboveViewerBackground(displayEl: HTMLElement): void {
  displayEl.style.position = "relative";
  displayEl.style.zIndex = "0";
  displayEl.style.isolation = "isolate";
  // The viewer container is the scroll/fit viewport; `margin: auto` centres a
  // scaled display and collapses to zero when the 1:1 display overflows.
  displayEl.style.margin = "auto";
  displayEl.style.flex = "0 0 auto";
}

// A white cross with a dark outline stays visible on both light and dark
// desktops; the native crosshair is the fallback if the data URI is blocked.
const VNC_FALLBACK_CURSOR = `url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='16' height='16'%3E%3Cpath d='M8 2v4M8 10v4M2 8h4M10 8h4' stroke='black' stroke-width='3' stroke-linecap='round'/%3E%3Cpath d='M8 2.5v3M8 10.5v3M2.5 8h3M10.5 8h3' stroke='white' stroke-width='1' stroke-linecap='round'/%3E%3C/svg%3E") 8 8, crosshair`;

function hasVisibleCursorPixels(canvas: HTMLCanvasElement | null): boolean {
  if (!canvas || canvas.width === 0 || canvas.height === 0) return false;
  try {
    const pixels = canvas.getContext("2d")?.getImageData(0, 0, canvas.width, canvas.height).data;
    if (!pixels) return false;
    for (let alpha = 3; alpha < pixels.length; alpha += 4) {
      if (pixels[alpha]! > 0) return true;
    }
  } catch {
    // Keep the local marker if the remote cursor cannot be inspected.
  }
  return false;
}

/**
 * The display hides the OS pointer unconditionally because Guacamole draws the
 * remote cursor itself. A VNC server that never sends a cursor image leaves
 * that layer empty, so the pointer vanishes entirely; fall back to a local
 * marker until the session provides one and whenever it hides it again.
 */
function installGuacCursor(
  display: Guacamole.Display,
  displayEl: HTMLElement,
  fallback: boolean,
): void {
  const apply = (hasRemoteCursor: boolean) => {
    displayEl.style.cursor = hasRemoteCursor || !fallback ? "none" : VNC_FALLBACK_CURSOR;
  };
  apply(false);
  if (!fallback) return;
  display.oncursor = (canvas) => apply(hasVisibleCursorPixels(canvas));
}

function sendClipboardText(client: Guacamole.Client, text: string): void {
  const stream = client.createClipboardStream("text/plain");
  const writer = new Guacamole.StringWriter(stream);
  writer.onack = (status) => {
    if (status.isError()) {
      try {
        writer.sendEnd();
      } catch {
        // ignore: the stream may already be closed by guacd.
      }
    }
  };
  writer.sendText(text);
  writer.sendEnd();
}

function sendRemotePasteShortcut(client: Guacamole.Client): void {
  client.sendKeyEvent(1, GUAC_KEYSYM_CONTROL_L);
  client.sendKeyEvent(1, GUAC_KEYSYM_V);
  client.sendKeyEvent(0, GUAC_KEYSYM_V);
  client.sendKeyEvent(0, GUAC_KEYSYM_CONTROL_L);
}

export function attachRdpViewer(opts: GuacAdapterOptions): GuacAdapter {
  const {
    container,
    url,
    token,
    readOnly = false,
    width,
    height,
    onOpen,
    onClose,
    onWireClose,
    onDisplayResize,
    onClipboardControllerChange,
    onRemoteClipboardText,
    onFilesystem,
    fallbackCursor = false,
  } = opts;

  const { tunnelUrl, connectData } = buildGuacTunnelConnection(url, token);
  const tunnel = new Guacamole.WebSocketTunnel(tunnelUrl);
  if (readOnly) {
    const send = tunnel.sendMessage.bind(tunnel);
    tunnel.sendMessage = (opcode: unknown, ...args: unknown[]) => {
      // Internal pings are echoed by the observation endpoint, not sent to guacd.
      const ping = opcode === "" && args.length === 2 && args[0] === "ping" && typeof args[1] === "number" && Number.isFinite(args[1]);
      if (opcode === "sync" || opcode === "ack" || ping) send(opcode, ...args);
    };
  }
  const client = new Guacamole.Client(tunnel);
  const display = client.getDisplay();
  let disposed = false;
  let closeReported = false;

  display.onresize = (width, height) => {
    if (!disposed) onDisplayResize?.(width, height);
  };

  // Mount display before connecting so the first draw has somewhere to go.
  const displayEl = display.getElement();
  keepGuacDisplayLayersAboveViewerBackground(displayEl);
  installGuacCursor(client.getDisplay(), displayEl, fallbackCursor);
  container.innerHTML = "";
  container.appendChild(displayEl);

  // Redirected drive (RDPDR): guacd emits a `filesystem` object when the
  // session is launched with `enable-drive`. Wrap it for the file browser.
  if (!readOnly) client.onfilesystem = (driveObject, driveName) => {
    if (disposed) return;
    onFilesystem?.(createRdpFilesystem(driveObject, driveName));
  };

  const clipboardController: RemoteClipboardController = {
    sendText(text: string): void {
      sendClipboardText(client, text);
    },
  };
  const clipboardPaste = readOnly ? null : attachRemoteClipboardPaste(container, clipboardController, {
    onPasteShortcut: () => sendRemotePasteShortcut(client),
  });
  const inputs = readOnly ? null : attachGuacInputs(client, container);
  if (!readOnly) onClipboardControllerChange?.(clipboardController);

  if (!readOnly) client.onclipboard = (stream, mimetype) => {
    if (mimetype !== "text/plain") {
      stream.sendAck("Unsupported clipboard type", Guacamole.Status.Code.UNSUPPORTED);
      return;
    }
    let text = "";
    const reader = new Guacamole.StringReader(stream);
    reader.ontext = (chunk) => {
      text += chunk;
    };
    reader.onend = () => {
      if (text.length > 0) onRemoteClipboardText?.(text);
    };
  };

  client.onerror = (status) => {
    closeReported = true;
    onClose?.(status.code, status.message);
  };

  client.onstatechange = (state) => {
    if (disposed) return;
    if (state === Guacamole.Client.State.CONNECTED) {
      onOpen?.();
      return;
    }
    if (state === Guacamole.Client.State.DISCONNECTED && !closeReported) {
      closeReported = true;
      onClose?.(Guacamole.Status.Code.SUCCESS);
    }
  };

  if (!readOnly && typeof width === "number") connectData.set("width", String(width));
  if (!readOnly && typeof height === "number") connectData.set("height", String(height));
  // Forward the tunnel's raw close pair; `onClose` keeps the Guacamole status
  // semantics the operator UI already renders.
  observeTunnelWireClose(tunnel, onWireClose && ((code, reason) => { if (!disposed) onWireClose(code, reason); }));
  client.connect(connectData.toString());

  return {
    dispose(): void {
      disposed = true;
      display.onresize = null;
      display.oncursor = null;
      client.onerror = null;
      client.onstatechange = null;
      client.onclipboard = null;
      if (!readOnly) onClipboardControllerChange?.(null);
      clipboardPaste?.dispose();
      inputs?.dispose();
      try {
        client.disconnect();
      } catch {
        // ignore: disconnect() can throw if the tunnel never opened.
      }
      container.innerHTML = "";
    },
    resize(w: number, h: number): void {
      if (!readOnly) client.sendSize(w, h);
    },
    setDisplayScale(scale: number): void {
      display.scale(scale);
    },
    getDisplaySize(): { width: number; height: number } {
      return { width: display.getWidth(), height: display.getHeight() };
    },
  };
}
