import { act, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { type PropsWithChildren, useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { getHostLabels } from "./api";
import { useHostLabels } from "./useHostLabels";

vi.mock("./api", () => ({ getHostLabels: vi.fn() }));

const clients: QueryClient[] = [];
function Wrapper({ children }: PropsWithChildren) {
  const [client] = useState(() => {
    const created = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
    clients.push(created);
    return created;
  });
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}

function response(ids: string[]) {
  return { hosts: ids.map((host_id) => ({ host_id, metrics: [], alerts: [], updated_at: null, stale: false })) };
}

afterEach(() => {
  for (const client of clients.splice(0)) client.clear();
  vi.resetAllMocks();
});

describe("useHostLabels request budget", () => {
  it("keeps successful batches when one batch fails and releases its request slot", async () => {
    vi.mocked(getHostLabels).mockImplementation((ids) =>
      ids[0] === "host-0" ? Promise.reject(new Error("Labels unavailable")) : Promise.resolve(response(ids)),
    );
    const ids = Array.from({ length: 151 }, (_, index) => `host-${index}`);
    const { result } = renderHook(() => useHostLabels(ids), { wrapper: Wrapper });
    await waitFor(() => expect(result.current.isLoading).toBe(false));
    expect(result.current.byHost.size).toBe(101);
    expect(result.current.byHost.has("host-150")).toBe(true);
  });

  it("fetches every host in a large inventory with at most two active requests", async () => {
    let active = 0;
    let peak = 0;
    vi.mocked(getHostLabels).mockImplementation(async (ids) => {
      active += 1;
      peak = Math.max(peak, active);
      await new Promise<void>((resolve) => queueMicrotask(resolve));
      active -= 1;
      return response(ids);
    });
    const ids = Array.from({ length: 4218 }, (_, index) => `host-${index}`);
    const { result } = renderHook(() => useHostLabels(ids), { wrapper: Wrapper });
    await waitFor(() => expect(result.current.byHost.size).toBe(ids.length));
    expect(peak).toBeLessThanOrEqual(2);
    expect(vi.mocked(getHostLabels).mock.calls.every(([batch]) => batch.length <= 50)).toBe(true);
  });

  it("does not silently drop hosts past the former 5000-id cap", async () => {
    vi.mocked(getHostLabels).mockImplementation((ids) => Promise.resolve(response(ids)));
    const ids = Array.from({ length: 5001 }, (_, index) => `host-${index}`);
    const { result } = renderHook(() => useHostLabels(ids), { wrapper: Wrapper });
    await waitFor(() => expect(result.current.byHost.size).toBe(ids.length));
    expect(result.current.byHost.has("host-5000")).toBe(true);
  });

  it("does not fetch empty or duplicate ids", async () => {
    vi.mocked(getHostLabels).mockImplementation((ids) => Promise.resolve(response(ids)));
    const { result } = renderHook(() => useHostLabels(["", "host-1", "host-1", "host-2"]), { wrapper: Wrapper });
    await waitFor(() => expect(result.current.byHost.size).toBe(2));
    expect(getHostLabels).toHaveBeenCalledTimes(1);
    expect(vi.mocked(getHostLabels).mock.calls[0]?.[0]).toEqual(["host-1", "host-2"]);
  });

  it("cancels obsolete viewport requests before starting labels for the next window", async () => {
    const pendingSignals: AbortSignal[] = [];
    vi.mocked(getHostLabels).mockImplementation((ids, signal) => {
      if (ids[0] === "next-host") return Promise.resolve(response(ids));
      if (!signal) return new Promise(() => undefined);
      pendingSignals.push(signal);
      return new Promise((_resolve, reject) => {
        signal.addEventListener("abort", () => reject(new DOMException("Cancelled", "AbortError")), { once: true });
      });
    });
    const ids = Array.from({ length: 200 }, (_, index) => `old-${index}`);
    const { result, rerender } = renderHook(({ visible }) => useHostLabels(visible), {
      wrapper: Wrapper, initialProps: { visible: ids },
    });
    await waitFor(() => expect(pendingSignals).toHaveLength(2));
    act(() => rerender({ visible: ["next-host"] }));
    await waitFor(() => expect(result.current.byHost.has("next-host")).toBe(true));
    expect(pendingSignals.every((signal) => signal.aborted)).toBe(true);
    expect(result.current.byHost.size).toBe(1);
  });
});
