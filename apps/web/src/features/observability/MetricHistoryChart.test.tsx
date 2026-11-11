import { render, screen } from "@testing-library/react";
import "@/shared/i18n";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { MonitoringMetricDescriptor } from "@/features/observability/api";
import type { UseMetricHistoryResult } from "@/features/observability/useMetricHistory";
import MetricHistoryChart from "./MetricHistoryChart";

const historyMock = vi.hoisted(() => ({
  useMetricHistory: vi.fn(),
}));

vi.mock("@/features/observability/useMetricHistory", () => ({
  useMetricHistory: historyMock.useMetricHistory,
}));

function descriptor(over: Partial<MonitoringMetricDescriptor> = {}): MonitoringMetricDescriptor {
  return {
    name: "cpu",
    label: "CPU",
    query: "cpu_usage",
    unit: "%",
    value_type: "percent",
    widget_type: "sparkline",
    description: null,
    source_type: "prometheus",
    query_type: "promql",
    options: {},
    show_in_list: false,
    list_render: "auto",
    ...over,
  };
}

function historyResult(over: Partial<UseMetricHistoryResult> = {}): UseMetricHistoryResult {
  return {
    points: [],
    isPending: false,
    isError: false,
    error: null,
    ...over,
  };
}

describe("MetricHistoryChart", () => {
  beforeEach(() => {
    historyMock.useMetricHistory.mockReset();
  });

  it("renders an empty state when there are no points", () => {
    historyMock.useMetricHistory.mockReturnValue(historyResult());
    render(<MetricHistoryChart targetId="t1" metric={descriptor()} />);

    expect(screen.getByText("No history in the selected range.")).toBeInTheDocument();
    expect(screen.queryByTestId("metric-history-line")).not.toBeInTheDocument();
  });

  it("draws the line and current value when points are present", () => {
    historyMock.useMetricHistory.mockReturnValue(
      historyResult({
        points: [
          { timestamp: 1700000000, value: 20 },
          { timestamp: 1700000060, value: 80 },
          { timestamp: 1700000120, value: 50 },
        ],
      }),
    );
    const { container } = render(<MetricHistoryChart targetId="t1" metric={descriptor()} />);

    expect(container.querySelector("[data-testid='metric-history-line']")).toHaveAttribute(
      "d",
      expect.stringContaining("L"),
    );
    expect(screen.getByText("50%")).toBeInTheDocument();
    expect(screen.getByRole("img", { name: "CPU history" })).toBeInTheDocument();
  });
});
