/**
 * Yields long enough for the WebView to commit + paint the current frame before
 * the caller runs a heavy synchronous burst (e.g. the noVNC RFB construction).
 *
 * Two animation frames let the desktop WebView paint the "connecting" state
 * before that burst blocks the main thread. The timeout is a safety net, not a
 * nicety: requestAnimationFrame is paused while the window is occluded or
 * minimized, so without it a backgrounded VNC connect would stall here
 * indefinitely and never attach.
 */
export function waitForViewerPaint(): Promise<void> {
  return new Promise((resolve) => {
    if (typeof window === "undefined" || typeof window.requestAnimationFrame !== "function") {
      globalThis.setTimeout(resolve, 0);
      return;
    }

    let settled = false;
    const done = () => {
      if (settled) return;
      settled = true;
      resolve();
    };

    window.requestAnimationFrame(() => {
      window.requestAnimationFrame(done);
    });
    globalThis.setTimeout(done, 100);
  });
}
