import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

/**
 * The Sentry SDK is a lazy chunk (`./observability-sentry`) fetched only after
 * the first render, so there is always a window where an error can happen with
 * no reporter installed. These tests pin the contract that makes that window
 * safe: nothing thrown before the SDK arrives is lost, nothing is reported
 * twice, and a build without a DSN leaks neither listeners nor memory.
 */

const startSentry = vi.hoisted(() => vi.fn());
vi.mock("./observability-sentry", () => ({ startSentry }));

const DSN = "https://public@example.invalid/1";

type Listener = (event: Event) => void;

/**
 * Capture the facade's global listeners instead of dispatching real events on
 * `window`: a genuine `error` event in jsdom is reported as an uncaught
 * exception by the test runner, which would fail the run even though the
 * handler behaved correctly. Invoking the registered handler exercises exactly
 * the code under test.
 */
function trackListeners() {
  const listeners = new Map<string, Set<Listener>>();
  vi.spyOn(window, "addEventListener").mockImplementation(((
    type: string,
    listener: Listener,
  ) => {
    const set = listeners.get(type) ?? new Set<Listener>();
    set.add(listener);
    listeners.set(type, set);
  }) as typeof window.addEventListener);
  vi.spyOn(window, "removeEventListener").mockImplementation(((
    type: string,
    listener: Listener,
  ) => {
    listeners.get(type)?.delete(listener);
  }) as typeof window.removeEventListener);

  return {
    count: (type: string) => listeners.get(type)?.size ?? 0,
    emit(type: string, payload: Record<string, unknown>) {
      const event = Object.assign(new Event(type), payload);
      for (const listener of listeners.get(type) ?? []) listener(event);
    },
  };
}

/** Fresh module state per test — the facade keeps its buffer in module scope. */
async function loadFacade() {
  vi.resetModules();
  return import("./observability");
}

function makeBridge() {
  return { capture: vi.fn(), tagRequestId: vi.fn() };
}

/** jsdom has no requestIdleCallback, so the facade falls back to setTimeout(0). */
async function flushIdle() {
  await vi.advanceTimersByTimeAsync(0);
}

describe("observability facade", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    startSentry.mockReset();
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("replays errors buffered before the SDK loads, in order", async () => {
    const bridge = makeBridge();
    startSentry.mockReturnValue(bridge);
    const globals = trackListeners();
    const { initObservability, loadObservability, captureException } = await loadFacade();

    initObservability(DSN);

    const early = new Error("early boom");
    captureException(early, { tags: { errorBoundary: "connections" } });
    globals.emit("error", { error: new Error("window boom") });
    expect(bridge.capture).not.toHaveBeenCalled();

    loadObservability();
    await flushIdle();

    expect(startSentry).toHaveBeenCalledTimes(1);
    expect(bridge.capture).toHaveBeenCalledTimes(2);
    expect(bridge.capture.mock.calls[0]?.[0]).toBe(early);
    expect(bridge.capture.mock.calls[0]?.[1]).toEqual({
      tags: { errorBoundary: "connections" },
    });
    expect((bridge.capture.mock.calls[1]?.[0] as Error).message).toBe("window boom");
  });

  it("buffers unhandled rejections too", async () => {
    const bridge = makeBridge();
    startSentry.mockReturnValue(bridge);
    const globals = trackListeners();
    const { initObservability, loadObservability } = await loadFacade();

    initObservability(DSN);
    const reason = new Error("rejected");
    globals.emit("unhandledrejection", { reason });

    loadObservability();
    await flushIdle();

    expect(bridge.capture).toHaveBeenCalledExactlyOnceWith(reason, {
      tags: { earlyCapture: "unhandledrejection" },
    });
  });

  it("does not double-report an error that both a boundary and window see", async () => {
    const bridge = makeBridge();
    startSentry.mockReturnValue(bridge);
    const globals = trackListeners();
    const { initObservability, loadObservability, captureException } = await loadFacade();

    initObservability(DSN);
    const error = new Error("same object");
    captureException(error);
    globals.emit("error", { error });

    loadObservability();
    await flushIdle();

    expect(bridge.capture).toHaveBeenCalledTimes(1);
  });

  it("forwards straight through once active and drops the early listeners", async () => {
    const bridge = makeBridge();
    startSentry.mockReturnValue(bridge);
    const globals = trackListeners();
    const { initObservability, loadObservability, captureException } = await loadFacade();

    initObservability(DSN);
    expect(globals.count("error")).toBe(1);
    expect(globals.count("unhandledrejection")).toBe(1);

    loadObservability();
    await flushIdle();

    // Sentry installs its own global handlers, so ours must be gone.
    expect(globals.count("error")).toBe(0);
    expect(globals.count("unhandledrejection")).toBe(0);

    captureException(new Error("late"));
    expect(bridge.capture).toHaveBeenCalledTimes(1);
  });

  it("applies a request id captured before the SDK loaded", async () => {
    const bridge = makeBridge();
    startSentry.mockReturnValue(bridge);
    trackListeners();
    const { initObservability, loadObservability, tagRequestId } = await loadFacade();

    initObservability(DSN);
    tagRequestId("req-early");
    expect(bridge.tagRequestId).not.toHaveBeenCalled();

    loadObservability();
    await flushIdle();

    expect(bridge.tagRequestId).toHaveBeenCalledExactlyOnceWith("req-early");
  });

  it("stays inert without a DSN: no SDK fetch, no listeners left behind", async () => {
    const globals = trackListeners();
    const { initObservability, loadObservability, captureException } = await loadFacade();

    initObservability(undefined);
    captureException(new Error("dropped"));
    loadObservability();
    await flushIdle();

    expect(startSentry).not.toHaveBeenCalled();
    expect(globals.count("error")).toBe(0);
    expect(globals.count("unhandledrejection")).toBe(0);
  });

  it("shuts down cleanly when the SDK chunk fails to start", async () => {
    startSentry.mockImplementation(() => {
      throw new Error("chunk load failed");
    });
    const globals = trackListeners();
    const { initObservability, loadObservability, captureException } = await loadFacade();

    initObservability(DSN);
    captureException(new Error("early"));
    loadObservability();
    await flushIdle();

    expect(globals.count("error")).toBe(0);
    expect(() => captureException(new Error("later"))).not.toThrow();
  });
});
