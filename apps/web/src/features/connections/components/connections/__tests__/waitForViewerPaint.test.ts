import { afterEach, describe, expect, it, vi } from "vitest";

import { waitForViewerPaint } from "../viewerPaint";

describe("waitForViewerPaint", () => {
  afterEach(() => {
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it("resolves on the timeout safety net when requestAnimationFrame never fires", async () => {
    // Simulates an occluded/minimized WebView: rAF callbacks are paused, so the
    // double-rAF would otherwise stall the VNC connect indefinitely.
    vi.useFakeTimers();
    vi.spyOn(window, "requestAnimationFrame").mockImplementation(() => 1);

    const settled = vi.fn();
    const promise = waitForViewerPaint().then(settled);

    expect(settled).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(100);
    await promise;

    expect(settled).toHaveBeenCalledOnce();
  });

  it("resolves once even when both frames and the timeout fire", async () => {
    vi.useFakeTimers();
    const resolve = vi.fn();
    const promise = waitForViewerPaint().then(resolve);

    // Flush queued animation frames and the safety-net timer together.
    await vi.runAllTimersAsync();
    await promise;

    expect(resolve).toHaveBeenCalledOnce();
  });
});
