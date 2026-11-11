import type { ReactNode } from "react";
import { renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { MetricHistoryResponse } from "@/features/observability/api";
import {
  mapHistorySeriesToPoints,
  useMetricHistory,
  useMetricHistorySeries,
} from "@/features/observability/useMetricHistory";

const apiMock = vi.hoisted(() => ({
  getMetricHistory: vi.fn(),
}));

vi.mock("@/features/observability/api", () => ({
  getMetricHistory: apiMock.getMetricHistory,
}));

function historyResponse(points: unknown[]): MetricHistoryResponse {
  return {
    configured: true,
    query: "up",
    series: [{ labels: {}, points }],
  };
}

function wrapper({ children }: { children: ReactNode }) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}

describe("mapHistorySeriesToPoints", () => {
  it("maps the first series' tuples into numeric sparkline points", () => {
    const points = mapHistorySeriesToPoints([
      { labels: {}, points: [[1700000000, "1.5"], [1700000060, "2.5"]] },
      { labels: {}, points: [[1700000000, "9"]] },
    ]);
    expect(points).toEqual([
      { timestamp: 1700000000, value: 1.5 },
      { timestamp: 1700000060, value: 2.5 },
    ]);
  });

  it("drops malformed or non-numeric entries", () => {
    const points = mapHistorySeriesToPoints([
      {
        labels: {},
        points: [
          [1700000000, "1"],
          [1700000060, "NaN"],
          [1700000120],
          "bogus",
          [1700000180, "3"],
        ],
      },
    ]);
    expect(points).toEqual([
      { timestamp: 1700000000, value: 1 },
      { timestamp: 1700000180, value: 3 },
    ]);
  });

  it("returns an empty array when there is no series", () => {
    expect(mapHistorySeriesToPoints(undefined)).toEqual([]);
    expect(mapHistorySeriesToPoints([])).toEqual([]);
  });
});

describe("useMetricHistory", () => {
  beforeEach(() => {
    apiMock.getMetricHistory.mockReset();
  });

  it("fetches history and exposes mapped points", async () => {
    apiMock.getMetricHistory.mockResolvedValue(
      historyResponse([
        [1700000000, "10"],
        [1700000060, "20"],
      ]),
    );

    const { result } = renderHook(
      () => useMetricHistory("target-1", "up", { rangeSeconds: 3600, step: 60 }),
      { wrapper },
    );

    await waitFor(() => expect(result.current.isPending).toBe(false));

    expect(result.current.points).toEqual([
      { timestamp: 1700000000, value: 10 },
      { timestamp: 1700000060, value: 20 },
    ]);
    expect(apiMock.getMetricHistory).toHaveBeenCalledTimes(1);
    const call = apiMock.getMetricHistory.mock.calls[0] ?? [];
    const [targetId, query, start, end, step] = call as [string, string, number, number, number?];
    expect(targetId).toBe("target-1");
    expect(query).toBe("up");
    expect(step).toBe(60);
    expect(end - start).toBe(3600);
  });

  it("does not fetch when disabled or missing identifiers", () => {
    renderHook(
      () => useMetricHistory("target-1", "up", { rangeSeconds: 3600, enabled: false }),
      { wrapper },
    );
    renderHook(() => useMetricHistory(undefined, "up", { rangeSeconds: 3600 }), { wrapper });
    renderHook(() => useMetricHistory("target-1", undefined, { rangeSeconds: 3600 }), { wrapper });

    expect(apiMock.getMetricHistory).not.toHaveBeenCalled();
  });
});

describe("useMetricHistorySeries", () => {
  beforeEach(() => {
    apiMock.getMetricHistory.mockReset();
  });

  it("backfills the last hour at 60s step and exposes just the points", async () => {
    apiMock.getMetricHistory.mockResolvedValue(
      historyResponse([
        [1700000000, "0.1"],
        [1700000060, "0.2"],
        [1700000120, "0.3"],
      ]),
    );

    const { result } = renderHook(
      () => useMetricHistorySeries("host-9", "rate(node_cpu_seconds_total[5m])"),
      { wrapper },
    );

    await waitFor(() => expect(result.current.length).toBeGreaterThan(0));
    const [targetId, query, start, end, step] = apiMock.getMetricHistory.mock.calls[0] as [
      string,
      string,
      number,
      number,
      number?,
    ];
    expect(targetId).toBe("host-9");
    expect(query).toBe("rate(node_cpu_seconds_total[5m])");
    expect(end - start).toBe(3600);
    expect(step).toBe(60);
    expect(result.current).toEqual([
      { timestamp: 1700000000, value: 0.1 },
      { timestamp: 1700000060, value: 0.2 },
      { timestamp: 1700000120, value: 0.3 },
    ]);
  });
});
