import { writeBrowserClipboardText } from "@/shared/lib/clipboard";

export { writeBrowserClipboardText };

export interface RemoteClipboardController {
  sendText(text: string): void;
}

export type RemoteClipboardTextHandler = (text: string) => void;

export interface RemoteClipboardPasteOptions {
  readClipboardText?: () => Promise<string>;
  onPasteShortcut?: () => void | Promise<void>;
  onPasteFallback?: () => void | Promise<void>;
  pasteShortcutDelayMs?: number;
}

const NATIVE_PASTE_CAPTURE_TIMEOUT_MS = 750;
const ASYNC_CLIPBOARD_FALLBACK_DELAY_MS = 75;

export function normalizeClipboardText(text: string): string | null {
  const normalized = text.replace(/\r\n|\r/g, "\n");
  return normalized.length > 0 ? normalized : null;
}

export function readClipboardEventText(event: ClipboardEvent): string | null {
  return normalizeClipboardText(event.clipboardData?.getData("text/plain") ?? "");
}

export function shouldIgnoreClipboardPasteTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  return Boolean(
    target.closest(
      "input, textarea, select, [contenteditable=''], [contenteditable='true'], [role='textbox']",
    ),
  );
}

function isRemoteClipboardPasteShortcut(event: KeyboardEvent): boolean {
  if (event.defaultPrevented || event.repeat || event.isComposing) return false;
  if (event.altKey || event.shiftKey) return false;
  if (!event.ctrlKey && !event.metaKey) return false;
  return event.key.toLowerCase() === "v" || event.code === "KeyV";
}

function isPasteShortcutKeyup(event: KeyboardEvent): boolean {
  return event.key.toLowerCase() === "v" || event.code === "KeyV";
}

function eventBelongsToElement(element: HTMLElement, event: Event): boolean {
  if (shouldIgnoreClipboardPasteTarget(event.target)) return false;
  if (event.target instanceof Node && element.contains(event.target)) return true;
  const activeElement = element.ownerDocument.activeElement;
  return activeElement instanceof Node && element.contains(activeElement);
}

function getBrowserClipboardReader(
  readClipboardText?: () => Promise<string>,
): (() => Promise<string>) | null {
  if (readClipboardText) return readClipboardText;
  if (typeof navigator === "undefined") return null;
  const clipboard = navigator.clipboard;
  if (!clipboard?.readText) return null;
  return () => clipboard.readText();
}

export function attachRemoteClipboardPaste(
  element: HTMLElement,
  controller: RemoteClipboardController,
  options: RemoteClipboardPasteOptions = {},
): { dispose(): void } {
  let suppressPasteShortcutKeyup = false;
  let nativePasteCapture: { dispose(): void } | null = null;

  const sendRemotePaste = (text: string | null) => {
    if (!text) return;
    controller.sendText(text);
    if (!options.onPasteShortcut) return;
    const delayMs = Math.max(0, options.pasteShortcutDelayMs ?? 0);
    if (delayMs === 0) {
      void options.onPasteShortcut();
      return;
    }
    element.ownerDocument.defaultView?.setTimeout(() => {
      void options.onPasteShortcut?.();
    }, delayMs);
  };

  const focusElement = (target: Element | null) => {
    if (target instanceof HTMLElement) {
      target.focus({ preventScroll: true });
    }
  };

  const startNativePasteCapture = () => {
    const doc = element.ownerDocument;
    const view = doc.defaultView;
    const body = doc.body;
    if (!view || !body) return false;

    nativePasteCapture?.dispose();
    const previousActiveElement = doc.activeElement;
    const capture = doc.createElement("textarea");
    capture.setAttribute("aria-hidden", "true");
    capture.tabIndex = -1;
    capture.autocapitalize = "off";
    capture.autocomplete = "off";
    capture.spellcheck = false;
    capture.style.position = "fixed";
    capture.style.left = "0";
    capture.style.top = "0";
    capture.style.width = "1px";
    capture.style.height = "1px";
    capture.style.opacity = "0";
    capture.style.pointerEvents = "none";
    capture.style.zIndex = "-1";

    let disposed = false;
    let textHandled = false;
    const cleanup = (fallback = false) => {
      if (disposed) return;
      disposed = true;
      view.clearTimeout(timeout);
      view.clearTimeout(asyncFallbackTimer);
      capture.removeEventListener("paste", handleCapturePaste);
      capture.remove();
      if (doc.activeElement === capture) {
        focusElement(previousActiveElement);
      }
      if (nativePasteCapture?.dispose === cleanup) {
        nativePasteCapture = null;
      }
      if (fallback && !textHandled) {
        void options.onPasteFallback?.();
      }
    };

    const handleCapturePaste = (event: ClipboardEvent) => {
      const text = readClipboardEventText(event);
      event.preventDefault();
      textHandled = text !== null;
      cleanup();
      sendRemotePaste(text);
    };

    const readText = getBrowserClipboardReader(options.readClipboardText);
    const asyncFallbackTimer = view.setTimeout(() => {
      if (!readText || disposed) return;
      void readText()
        .then((clipboardText) => {
          if (disposed) return;
          const text = normalizeClipboardText(clipboardText);
          textHandled = text !== null;
          cleanup();
          sendRemotePaste(text);
        })
        .catch(() => undefined);
    }, ASYNC_CLIPBOARD_FALLBACK_DELAY_MS);
    const timeout = view.setTimeout(() => cleanup(true), NATIVE_PASTE_CAPTURE_TIMEOUT_MS);

    capture.addEventListener("paste", handleCapturePaste);
    body.append(capture);
    capture.focus({ preventScroll: true });
    capture.select();
    nativePasteCapture = { dispose: cleanup };
    return true;
  };

  const handlePaste = (event: ClipboardEvent) => {
    if (event.defaultPrevented) return;
    if (shouldIgnoreClipboardPasteTarget(event.target)) return;
    if (!eventBelongsToElement(element, event)) return;
    const text = readClipboardEventText(event);
    if (!text) return;
    sendRemotePaste(text);
    event.preventDefault();
  };

  const handleKeyDown = (event: KeyboardEvent) => {
    if (!options.onPasteShortcut) return;
    if (!isRemoteClipboardPasteShortcut(event)) return;
    if (!eventBelongsToElement(element, event)) return;

    suppressPasteShortcutKeyup = true;
    event.stopPropagation();
    event.stopImmediatePropagation();
    if (!startNativePasteCapture()) {
      event.preventDefault();
      const readText = getBrowserClipboardReader(options.readClipboardText);
      void readText?.()
        .then((clipboardText) => {
          const text = normalizeClipboardText(clipboardText);
          if (text) sendRemotePaste(text);
          else void options.onPasteFallback?.();
        })
        .catch(() => {
          void options.onPasteFallback?.();
        });
    }
  };

  const handleKeyUp = (event: KeyboardEvent) => {
    if (!suppressPasteShortcutKeyup) return;
    if (!isPasteShortcutKeyup(event)) return;
    suppressPasteShortcutKeyup = false;
    event.preventDefault();
    event.stopPropagation();
    event.stopImmediatePropagation();
  };

  const keyboardEventTarget = element.ownerDocument;
  element.addEventListener("paste", handlePaste);
  keyboardEventTarget.addEventListener("paste", handlePaste, true);
  keyboardEventTarget.addEventListener("keydown", handleKeyDown, true);
  keyboardEventTarget.addEventListener("keyup", handleKeyUp, true);
  return {
    dispose(): void {
      nativePasteCapture?.dispose();
      element.removeEventListener("paste", handlePaste);
      keyboardEventTarget.removeEventListener("paste", handlePaste, true);
      keyboardEventTarget.removeEventListener("keydown", handleKeyDown, true);
      keyboardEventTarget.removeEventListener("keyup", handleKeyUp, true);
    },
  };
}
