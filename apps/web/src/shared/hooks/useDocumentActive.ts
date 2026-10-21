import { useSyncExternalStore } from "react";

/**
 * Whether THIS window is the one the user is actually looking at — focused and
 * visible. Live MJPEG streams are `multipart/x-mixed-replace` `<img>` requests
 * that hold a browser connection open forever; the browser caps ~6 connections
 * per origin ACROSS all tabs/windows. So a detached CCTV wall (a grid of live
 * streams in its own `window.open` window) can consume the whole pool and starve
 * the main app window's navigation requests — they queue until a stream closes.
 *
 * Gating a stream on this flag lets a background window release its connections
 * (the stream `<img>` unmounts → connection closes) so the focused window gets
 * them back, then resume when the user returns. `focus`/`blur` — not just the
 * Page Visibility API — is what distinguishes two side-by-side top-level windows
 * (Page Visibility only flips on tab-switch/minimize, not cross-window focus).
 *
 * Design: start **optimistically active** and flip to inactive only on an
 * OBSERVED `blur`/`hidden` event. `document.hasFocus()` is deliberately NOT read
 * at init — it returns a spurious `false` in some environments (jsdom, a window
 * that hasn't received its first focus event yet), which would wrongly gate the
 * first stream. A freshly focused/opened window is active until proven otherwise.
 */
const hasDom = typeof window !== "undefined" && typeof document !== "undefined";

let focused = true;
let visible = hasDom ? document.visibilityState !== "hidden" : true;
let blurTimer: ReturnType<typeof setTimeout> | undefined;
const listeners = new Set<() => void>();
const BLUR_GRACE_MS = 200;

function snapshot(): boolean {
  return focused && visible;
}

function emit(): void {
  for (const listener of listeners) {
    listener();
  }
}

function onFocus(): void {
  if (blurTimer !== undefined) {
    clearTimeout(blurTimer);
    blurTimer = undefined;
  }
  if (!focused) {
    focused = true;
    emit();
  }
}

function onBlur(): void {
  if (blurTimer !== undefined) clearTimeout(blurTimer);
  blurTimer = setTimeout(() => {
    blurTimer = undefined;
    if (focused) {
      focused = false;
      emit();
    }
  }, BLUR_GRACE_MS);
}

function onPageHide(): void {
  if (blurTimer !== undefined) clearTimeout(blurTimer);
  blurTimer = undefined;
  if (focused) {
    focused = false;
    emit();
  }
}

function onVisibility(): void {
  const next = document.visibilityState !== "hidden";
  if (!next && blurTimer !== undefined) {
    clearTimeout(blurTimer);
    blurTimer = undefined;
  }
  if (next !== visible) {
    visible = next;
    emit();
  }
}

function subscribe(listener: () => void): () => void {
  if (!hasDom) {
    return () => {};
  }
  if (listeners.size === 0) {
    window.addEventListener("focus", onFocus);
    window.addEventListener("blur", onBlur);
    window.addEventListener("pageshow", onFocus);
    window.addEventListener("pagehide", onPageHide);
    document.addEventListener("visibilitychange", onVisibility);
  }
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0) {
      window.removeEventListener("focus", onFocus);
      window.removeEventListener("blur", onBlur);
      window.removeEventListener("pageshow", onFocus);
      window.removeEventListener("pagehide", onPageHide);
      document.removeEventListener("visibilitychange", onVisibility);
    }
  };
}

/**
 * Reactively tracks whether this window is focused AND visible. SSR-safe and
 * optimistic (active until an event proves otherwise).
 */
export function useDocumentActive(): boolean {
  return useSyncExternalStore(subscribe, snapshot, () => true);
}
