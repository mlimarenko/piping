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
  displayEl.style.cursor = "none";
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
    width,
    height,
    onOpen,
    onClose,
    onClipboardControllerChange,
    onRemoteClipboardText,
    onFilesystem,
  } = opts;

  const { tunnelUrl, connectData } = buildGuacTunnelConnection(url, token);
  const tunnel = new Guacamole.WebSocketTunnel(tunnelUrl);
  const client = new Guacamole.Client(tunnel);
  let disposed = false;
  let closeReported = false;

  // Mount display before connecting so the first draw has somewhere to go.
  const displayEl = client.getDisplay().getElement();
  keepGuacDisplayLayersAboveViewerBackground(displayEl);
  container.innerHTML = "";
  container.appendChild(displayEl);

  // Redirected drive (RDPDR): guacd emits a `filesystem` object when the
  // session is launched with `enable-drive`. Wrap it for the file browser.
  client.onfilesystem = (driveObject, driveName) => {
    if (disposed) return;
    onFilesystem?.(createRdpFilesystem(driveObject, driveName));
  };

  const clipboardController: RemoteClipboardController = {
    sendText(text: string): void {
      sendClipboardText(client, text);
    },
  };
  const clipboardPaste = attachRemoteClipboardPaste(container, clipboardController, {
    onPasteShortcut: () => sendRemotePasteShortcut(client),
  });
  const inputs = attachGuacInputs(client, container);
  onClipboardControllerChange?.(clipboardController);

  client.onclipboard = (stream, mimetype) => {
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

  if (typeof width === "number") connectData.set("width", String(width));
  if (typeof height === "number") connectData.set("height", String(height));
  client.connect(connectData.toString());

  return {
    dispose(): void {
      disposed = true;
      client.onerror = null;
      client.onstatechange = null;
      client.onclipboard = null;
      onClipboardControllerChange?.(null);
      clipboardPaste.dispose();
      inputs.dispose();
      try {
        client.disconnect();
      } catch {
        // ignore: disconnect() can throw if the tunnel never opened.
      }
      container.innerHTML = "";
    },
    resize(w: number, h: number): void {
      client.sendSize(w, h);
    },
  };
}
