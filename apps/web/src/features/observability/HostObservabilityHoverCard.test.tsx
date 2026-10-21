import type { ReactNode } from "react";
import { render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import "@/shared/i18n";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type * as ApiModule from "./api";
import type { MetricHistoryResponse } from "./api";

const apiMock = vi.hoisted(() => ({
  getMetricHistory: vi.fn(),
}));

vi.mock("@/features/observability/api", async () => {
  const actual = await vi.importActual<typeof ApiModule>("@/features/observability/api");
  return {
    ...actual,
    getMetricHistory: apiMock.getMetricHistory,
  };
});

// Radix's HoverCard hides its content until a pointer/focus gesture lands on
// the trigger, which JSDOM does not produce. The hover card module exports
// `HoverMetricCard` precisely so we can mount it directly and assert the real
// component wiring — `useMetricHistorySeries(host_id, query)` →
// `MetricWidgetCard variant="card"` with the `series.length >= 2` gate and a
// translated `sparklineLabel`.

import { HostObservabilityCluster, HoverMetricCard } from "./HostObservabilityHoverCard";
import type { HostLabelEntry, HostLabelMetric } from "./api";

function promValue(raw: string): unknown {
  return {
    status: "success",
    data: { resultType: "vector", result: [{ metric: {}, value: [1700000000, raw] }] },
  };
}

function historyResponse(points: unknown[]): MetricHistoryResponse {
  return { configured: true, query: "node_load1", series: [{ labels: {}, points }] };
}

const HOST_ID = "11111111-1111-1111-1111-111111111111";

function labelMetric(over: Partial<HostLabelMetric> = {}): HostLabelMetric {
  return {
    query: "node_load1",
    value: promValue("0.5"),
    label: "Load",
    // `stat` widget — without backfill this card has no sparkline. The mini
    // trend strip is the load-bearing assertion here.
    widget_type: "stat",
    unit: null,
    value_type: null,
    list_render: "value",
    options: {},
    semantic: null,
    ...over,
  };
}

function hostEntry(metrics: HostLabelMetric[]): HostLabelEntry {
  return {
    host_id: HOST_ID,
    stale: false,
    metrics,
    alerts: [],
  };
}

function Wrapper({ children }: { children: ReactNode }) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}

describe("HostObservabilityHoverCard sparkline backfill", () => {
  beforeEach(() => {
    apiMock.getMetricHistory.mockReset();
  });

  it("keeps semantic availability out of the regular metric strip", () => {
    render(
      <Wrapper>
        <HostObservabilityCluster
          entry={hostEntry([
            labelMetric({
              query: "up{job=\"node\"}",
              label: "Up",
              value: promValue("1"),
              value_type: "state",
              semantic: "availability",
            }),
            labelMetric({ query: "node_load1", label: "Load" }),
          ])}
        />
      </Wrapper>,
    );

    expect(screen.queryByLabelText(/^Up:/)).not.toBeInTheDocument();
    expect(screen.getByLabelText(/^Load:/)).toBeInTheDocument();
  });

  it("queries metric history per metric and renders the mini trend strip beneath a stat card", async () => {
    apiMock.getMetricHistory.mockResolvedValue(
      historyResponse([
        [1700000000, "0.2"],
        [1700000060, "0.4"],
        [1700000120, "0.6"],
      ]),
    );

    render(
      <Wrapper>
        <HoverMetricCard hostId={HOST_ID} metric={labelMetric()} stale={false} />
      </Wrapper>,
    );

    // The hook must call the history endpoint with the host_id and query
    // from the cached label metric.
    await waitFor(() => expect(apiMock.getMetricHistory).toHaveBeenCalled());
    const [targetId, query] = apiMock.getMetricHistory.mock.calls[0] as [string, string];
    expect(targetId).toBe(HOST_ID);
    expect(query).toBe("node_load1");

    // The mini trend strip mounts beneath the value pill — even for a `stat`
    // widget type that has no built-in sparkline body. The label is the
    // translated `observability.metric.recentValues`, so the test asserts the
    // i18n thread-through too.
    expect(await screen.findByTestId("metric-mini-trend")).toBeInTheDocument();
    expect(screen.getByTestId("metric-mini-trend")).toHaveAttribute(
      "aria-label",
      "Recent values",
    );
  });

  it("silently degrades to a value-only card when the history endpoint errors", async () => {
    apiMock.getMetricHistory.mockRejectedValue(new Error("upstream down"));

    render(
      <Wrapper>
        <HoverMetricCard hostId={HOST_ID} metric={labelMetric()} stale={false} />
      </Wrapper>,
    );

    // The metric label still renders so the card stays useful even without a
    // backfilled trend.
    expect(screen.getByText(/Load/)).toBeInTheDocument();

    // No error chip is added in the hover card path — `metric-mini-trend`
    // must not appear when history could not be loaded.
    await waitFor(() => expect(apiMock.getMetricHistory).toHaveBeenCalled());
    expect(screen.queryByTestId("metric-mini-trend")).not.toBeInTheDocument();
  });

  it("skips the trend strip when the history response carries fewer than two points", async () => {
    apiMock.getMetricHistory.mockResolvedValue(
      historyResponse([[1700000000, "0.5"]]),
    );

    render(
      <Wrapper>
        <HoverMetricCard hostId={HOST_ID} metric={labelMetric()} stale={false} />
      </Wrapper>,
    );

    await waitFor(() => expect(apiMock.getMetricHistory).toHaveBeenCalled());
    expect(screen.queryByTestId("metric-mini-trend")).not.toBeInTheDocument();
  });
});
