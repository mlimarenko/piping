import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

let visibilityState: DocumentVisibilityState;

async function renderDocumentActive() {
  const { useDocumentActive } = await import("./useDocumentActive");
  return renderHook(() => useDocumentActive());
}

describe("useDocumentActive", () => {
  beforeEach(() => {
    vi.resetModules();
    vi.useFakeTimers();
    visibilityState = "visible";
    vi.spyOn(document, "visibilityState", "get").mockImplementation(() => visibilityState);
  });

  afterEach(() => {
    cleanup();
    vi.clearAllTimers();
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("keeps the document active during the blur grace period, then deactivates it", async () => {
    const hook = await renderDocumentActive();

    expect(hook.result.current).toBe(true);

    act(() => window.dispatchEvent(new Event("blur")));
    act(() => vi.advanceTimersByTime(199));
    expect(hook.result.current).toBe(true);

    act(() => vi.advanceTimersByTime(1));
    expect(hook.result.current).toBe(false);
  });

  it("cancels a pending blur when focus returns during the grace period", async () => {
    const hook = await renderDocumentActive();

    act(() => window.dispatchEvent(new Event("blur")));
    act(() => vi.advanceTimersByTime(100));
    act(() => window.dispatchEvent(new Event("focus")));
    act(() => vi.advanceTimersByTime(100));

    expect(hook.result.current).toBe(true);
  });

  it("restarts the grace period when another blur event arrives", async () => {
    const hook = await renderDocumentActive();

    act(() => window.dispatchEvent(new Event("blur")));
    act(() => vi.advanceTimersByTime(150));
    act(() => window.dispatchEvent(new Event("blur")));
    act(() => vi.advanceTimersByTime(199));
    expect(hook.result.current).toBe(true);

    act(() => vi.advanceTimersByTime(1));
    expect(hook.result.current).toBe(false);
  });

  it("deactivates immediately on pagehide and reactivates on pageshow", async () => {
    const hook = await renderDocumentActive();

    act(() => window.dispatchEvent(new Event("blur")));
    act(() => window.dispatchEvent(new Event("pagehide")));
    expect(hook.result.current).toBe(false);

    act(() => window.dispatchEvent(new Event("pageshow")));
    act(() => vi.advanceTimersByTime(200));
    expect(hook.result.current).toBe(true);
  });

  it("stays inactive while hidden and reactivates when visibility returns", async () => {
    const hook = await renderDocumentActive();

    act(() => window.dispatchEvent(new Event("blur")));
    visibilityState = "hidden";
    act(() => document.dispatchEvent(new Event("visibilitychange")));
    expect(hook.result.current).toBe(false);

    act(() => vi.advanceTimersByTime(200));
    visibilityState = "visible";
    act(() => document.dispatchEvent(new Event("visibilitychange")));
    expect(hook.result.current).toBe(true);
  });

  it("shares global listeners and removes them only after the last subscriber unmounts", async () => {
    const addWindowListener = vi.spyOn(window, "addEventListener");
    const removeWindowListener = vi.spyOn(window, "removeEventListener");
    const first = await renderDocumentActive();
    const second = await renderDocumentActive();

    expect(addWindowListener.mock.calls.filter(([event]) => event === "blur")).toHaveLength(1);

    first.unmount();
    expect(removeWindowListener.mock.calls.filter(([event]) => event === "blur")).toHaveLength(0);

    second.unmount();
    expect(removeWindowListener.mock.calls.filter(([event]) => event === "blur")).toHaveLength(1);
  });
});
