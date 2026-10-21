/**
 * VNC viewer built on noVNC's RFB class. Owns the canvas and the WebSocket
 * through the noVNC internals.
 *
 * The backend relays raw VNC/RFB TCP bytes over a WebSocket at
 * `/api/v1/ws/vnc/{id}?token={token}`. noVNC connects to that endpoint
 * with `Sec-WebSocket-Protocol: binary` (selected from `["binary", "base64"]`).
 * Guacamole is NOT used for VNC — the relay is raw RFB protocol.
 */

// noVNC is a static dependency of the lazy LiveVNC chunk so it loads under the
// Suspense fallback, matching how rdpAdapter imports guacamole and sshAdapter
// imports xterm. Deferring it here pushes parse/execute work into the connect
// path, which is visible as startup jank in the desktop WebView.
import RFB from "@novnc/novnc";

import {
  attachRemoteClipboardPaste,
  type RemoteClipboardController,
  type RemoteClipboardTextHandler,
} from "./remoteClipboard";

export interface VncAdapterOptions {
  container: HTMLElement;
  /** Absolute `wss://` URL from the backend, including any existing query. */
  url: string;
  /** Per-session token issued by `POST /api/v1/connections/sessions`. */
  token: string;
  /** VNC password resolved from the session's client-credentials endpoint. */
  password?: string;
  /** Called when the RFB connection is established. */
  onOpen?: () => void;
  /** Called when the connection closes; clean=true means normal shutdown. */
  onClose?: (clean: boolean, reason?: string) => void;
  onClipboardControllerChange?: (controller: RemoteClipboardController | null) => void;
  onRemoteClipboardText?: RemoteClipboardTextHandler;
}

export interface VncAdapter {
  dispose(): void;
  focus(): void;
  resize(): void;
  setInputEnabled(enabled: boolean): void;
}

const VNC_KEYSYM_CONTROL_L = 0xffe3;
const VNC_KEYSYM_CONTROL_R = 0xffe4;
const VNC_KEYSYM_SHIFT_L = 0xffe1;
const VNC_KEYSYM_SHIFT_R = 0xffe2;
const VNC_KEYSYM_ALT_L = 0xffe9;
const VNC_KEYSYM_ALT_R = 0xffea;
const VNC_KEYSYM_SUPER_L = 0xffeb;
const VNC_KEYSYM_SUPER_R = 0xffec;
const VNC_KEYSYM_V = 0x0076;
const VNC_CLIPBOARD_RESYNC_DELAYS_MS = [150, 450] as const;
const VNC_PASTE_SHORTCUT_DELAY_MS = 650;

function appendToken(url: string, token: string): string {
  const base = typeof window !== "undefined" ? window.location.origin : "http://localhost";
  const parsed = new URL(url, base);
  parsed.searchParams.set("token", token);
  return parsed.toString();
}

function shouldIgnoreVncShortcutTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  return Boolean(
    target.closest(
      "input, textarea, select, [contenteditable=''], [contenteditable='true'], [role='textbox']",
    ),
  );
}

function eventBelongsToElement(element: HTMLElement, event: Event): boolean {
  if (shouldIgnoreVncShortcutTarget(event.target)) return false;
  if (event.target instanceof Node && element.contains(event.target)) return true;
  const activeElement = element.ownerDocument.activeElement;
  return activeElement instanceof Node && element.contains(activeElement);
}

function keyboardEventToControlShortcutKeysym(event: KeyboardEvent): number | null {
  if (/^Key[A-Z]$/.test(event.code)) {
    return event.code.charCodeAt(3) + 0x20;
  }
  if (event.key.length !== 1) return null;
  const codepoint = event.key.toLowerCase().codePointAt(0);
  if (codepoint === undefined || codepoint < 0x20 || codepoint > 0x7e) return null;
  return codepoint;
}

function isRemoteControlShortcut(event: KeyboardEvent): boolean {
  if (event.defaultPrevented || event.repeat || event.isComposing) return false;
  if (event.altKey) return false;
  if (!event.ctrlKey && !event.metaKey) return false;
  if (!event.shiftKey && (event.key.toLowerCase() === "v" || event.code === "KeyV")) return false;
  return keyboardEventToControlShortcutKeysym(event) !== null;
}

function sendRemoteControlShortcut(rfb: RFB, event: KeyboardEvent): void {
  const keysym = keyboardEventToControlShortcutKeysym(event);
  if (keysym === null) return;
  const code = event.code || `Key${event.key.toUpperCase()}`;
  rfb.sendKey(VNC_KEYSYM_CONTROL_L, "ControlLeft", true);
  rfb.sendKey(keysym, code, true);
  rfb.sendKey(keysym, code, false);
  rfb.sendKey(VNC_KEYSYM_CONTROL_L, "ControlLeft", false);
}

function sendRemotePasteShortcut(rfb: RFB): void {
  releaseRemoteModifiers(rfb);
  rfb.sendKey(VNC_KEYSYM_CONTROL_L, "ControlLeft", true);
  rfb.sendKey(VNC_KEYSYM_V, "KeyV", true);
  rfb.sendKey(VNC_KEYSYM_V, "KeyV", false);
  rfb.sendKey(VNC_KEYSYM_CONTROL_L, "ControlLeft", false);
}

function releaseRemoteModifiers(rfb: RFB): void {
  rfb.sendKey(VNC_KEYSYM_CONTROL_L, "ControlLeft", false);
  rfb.sendKey(VNC_KEYSYM_CONTROL_R, "ControlRight", false);
  rfb.sendKey(VNC_KEYSYM_SHIFT_L, "ShiftLeft", false);
  rfb.sendKey(VNC_KEYSYM_SHIFT_R, "ShiftRight", false);
  rfb.sendKey(VNC_KEYSYM_ALT_L, "AltLeft", false);
  rfb.sendKey(VNC_KEYSYM_ALT_R, "AltRight", false);
  rfb.sendKey(VNC_KEYSYM_SUPER_L, "MetaLeft", false);
  rfb.sendKey(VNC_KEYSYM_SUPER_R, "MetaRight", false);
}

function attachVncControlShortcutBridge(
  element: HTMLElement,
  rfb: RFB,
): { dispose(): void } {
  const handleKeyDown = (event: KeyboardEvent) => {
    if (rfb.viewOnly) return;
    if (!isRemoteControlShortcut(event)) return;
    if (!eventBelongsToElement(element, event)) return;
    event.preventDefault();
    event.stopPropagation();
    event.stopImmediatePropagation();
    sendRemoteControlShortcut(rfb, event);
  };

  const handleKeyUp = (event: KeyboardEvent) => {
    if (rfb.viewOnly) return;
    if (!isRemoteControlShortcut(event)) return;
    if (!eventBelongsToElement(element, event)) return;
    event.preventDefault();
    event.stopPropagation();
    event.stopImmediatePropagation();
  };

  const keyboardEventTarget = element.ownerDocument;
  keyboardEventTarget.addEventListener("keydown", handleKeyDown, true);
  keyboardEventTarget.addEventListener("keyup", handleKeyUp, true);
  return {
    dispose(): void {
      keyboardEventTarget.removeEventListener("keydown", handleKeyDown, true);
      keyboardEventTarget.removeEventListener("keyup", handleKeyUp, true);
    },
  };
}

/** Release every transmitted key before noVNC's viewOnly setter suppresses key-up. */
function trackVncPressedKeys(rfb: RFB): () => void {
  const pressed = new Map<string, number>();
  const sendKey = rfb.sendKey.bind(rfb);
  rfb.sendKey = (keysym, code, down) => {
    if (rfb.viewOnly) return;
    if (down === true) pressed.set(code, keysym);
    if (down === false) pressed.delete(code);
    sendKey(keysym, code, down);
  };
  return () => {
    for (const [code, keysym] of pressed) rfb.sendKey(keysym, code, false);
  };
}

// RFB's constructor accepts a container element; it creates its own internal
// canvas and appends it into the provided target div. The Promise return type is
// retained for callers even though attachment is now synchronous (noVNC is a
// static import), so LiveVNC and the adapter tests stay unchanged.
export function attachVncViewer(opts: VncAdapterOptions): Promise<VncAdapter> {
  const {
    container,
    url,
    token,
    password,
    onOpen,
    onClose,
    onClipboardControllerChange,
    onRemoteClipboardText,
  } = opts;

  // Clear the container; RFB will append its own internal screen/canvas.
  container.innerHTML = "";
  if (!container.hasAttribute("tabindex")) {
    container.setAttribute("tabindex", "0");
  }

  // RFB is constructed after the paste interceptor is registered; that order
  // keeps our Ctrl+V bridge ahead of noVNC's keyboard handlers. The closures
  // below bind the const declared at its construction site — they only run
  // after construction, so the temporal-dead-zone order is safe.
  let lastClipboardText: string | null = null;
  let pasteShortcutTimers: number[] = [];
  const focusViewer = () => {
    if (rfb.viewOnly) return;
    rfb.focus({ preventScroll: true });
  };
  const clearPasteShortcutTimers = () => {
    const view = container.ownerDocument.defaultView;
    for (const timer of pasteShortcutTimers) {
      view?.clearTimeout(timer);
    }
    pasteShortcutTimers = [];
  };
  const schedulePasteShortcut = () => {
    if (rfb.viewOnly) return;
    const text = lastClipboardText;
    const view = container.ownerDocument.defaultView;
    if (!text || !view) return;

    clearPasteShortcutTimers();
    focusViewer();
    // noVNC direct keysyms are affected by the remote keyboard state/layout
    // (CapsLock, Shift, national layouts). Use RFB clipboard sync instead and
    // only send the remote paste shortcut after resyncing ClientCutText.
    for (const delayMs of VNC_CLIPBOARD_RESYNC_DELAYS_MS) {
      pasteShortcutTimers.push(
        view.setTimeout(() => {
          if (lastClipboardText === text) {
            rfb.clipboardPasteFrom(text);
          }
        }, delayMs),
      );
    }
    pasteShortcutTimers.push(
      view.setTimeout(() => {
        if (lastClipboardText !== text) return;
        focusViewer();
        sendRemotePasteShortcut(rfb);
      }, VNC_PASTE_SHORTCUT_DELAY_MS),
    );
  };

  const clipboardController: RemoteClipboardController = {
    sendText(text: string): void {
      if (rfb.viewOnly) return;
      lastClipboardText = text;
      rfb.clipboardPasteFrom(text);
    },
  };
  // Register paste interception before constructing noVNC's RFB. noVNC also
  // hooks keyboard events, and if it sees Ctrl+V first it can paste the remote
  // machine's stale clipboard before our ClientCutText sync reaches the server.
  const clipboardPaste = attachRemoteClipboardPaste(container, clipboardController, {
    onPasteShortcut: schedulePasteShortcut,
    onPasteFallback: () => {
      focusViewer();
      sendRemotePasteShortcut(rfb);
    },
  });

  const wsUrl = appendToken(url, token);

  const rfb = new RFB(container, wsUrl, {
    credentials: password ? { password } : {},
    wsProtocols: ["binary", "base64"],
  });
  const releasePressedKeys = trackVncPressedKeys(rfb);
  const setInputEnabled = (enabled: boolean) => {
    if (!enabled) {
      clearPasteShortcutTimers();
      lastClipboardText = null;
      releasePressedKeys();
    }
    rfb.viewOnly = !enabled;
  };

  rfb.scaleViewport = true;
  rfb.resizeSession = true;

  const screen = container.firstElementChild;
  if (screen instanceof HTMLElement) {
    screen.style.overflow = "hidden";
    screen.style.minWidth = "0";
    screen.style.minHeight = "0";
  }

  const requestResize = () => {
    rfb.scaleViewport = true;
    rfb.resizeSession = true;
  };
  const shortcutBridge = attachVncControlShortcutBridge(container, rfb);
  container.addEventListener("mousedown", focusViewer, true);
  container.addEventListener("touchstart", focusViewer, true);
  onClipboardControllerChange?.(clipboardController);

  const handleConnect = () => onOpen?.();
  const handleDisconnect = (ev: Event) => {
    const detail = (ev as CustomEvent<{ clean: boolean; reason?: string }>).detail;
    onClose?.(detail?.clean ?? false, detail?.reason);
  };
  const handleClipboard = (ev: Event) => {
    const detail = (ev as CustomEvent<{ text?: unknown }>).detail;
    if (typeof detail?.text === "string" && detail.text.length > 0) {
      onRemoteClipboardText?.(detail.text);
    }
  };

  rfb.addEventListener("connect", handleConnect);
  rfb.addEventListener("disconnect", handleDisconnect);
  rfb.addEventListener("clipboard", handleClipboard);

  return Promise.resolve({
    setInputEnabled,
    dispose(): void {
      setInputEnabled(false);
      rfb.removeEventListener("connect", handleConnect);
      rfb.removeEventListener("disconnect", handleDisconnect);
      rfb.removeEventListener("clipboard", handleClipboard);
      onClipboardControllerChange?.(null);
      container.removeEventListener("mousedown", focusViewer, true);
      container.removeEventListener("touchstart", focusViewer, true);
      shortcutBridge.dispose();
      clipboardPaste.dispose();
      clearPasteShortcutTimers();
      try {
        rfb.disconnect();
      } catch {
        // ignore
      }
      // noVNC removes its own screen element on disconnect; clear any residual.
      container.innerHTML = "";
    },
    focus(): void {
      focusViewer();
    },
    resize(): void {
      requestResize();
    },
  });
}
