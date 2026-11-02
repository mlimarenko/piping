import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import type {
  LatestMetricItem,
  MonitoringMetricDescriptor,
} from "@/features/observability/api";
import MetricWidgetCard from "./MetricWidgetCard";

function descriptor(overrides: Partial<MonitoringMetricDescriptor> = {}): MonitoringMetricDescriptor {
  return {
    name: "network_rx",
    label: "Network RX",
    query: "rate(node_network_receive_bytes_total[5m])",
    unit: "B/s",
    value_type: "number",
    widget_type: "sparkline",
    description: "Receive throughput",
    source_type: "prometheus",
    query_type: "promql",
    options: {},
    show_in_list: false,
    list_render: "auto",
    ...overrides,
  };
}

function latest(value: string): LatestMetricItem {
  return {
    query: "rate(node_network_receive_bytes_total[5m])",
    executed_query: "rate(node_network_receive_bytes_total[5m])",
    name: "network_rx",
    label: "Network RX",
    unit: "B/s",
    value_type: "number",
    widget_type: "sparkline",
    source_type: "prometheus",
    status: "success",
    value: {
      status: "success",
      data: {
        resultType: "vector",
        result: [{ metric: {}, value: [1, value] }],
      },
    },
    error: null,
  };
}

describe("MetricWidgetCard", () => {
  it("keeps error status legible on an opaque neutral surface", () => {
    const { container } = render(
      <MetricWidgetCard
        metric={descriptor()}
        latest={{ ...latest("0"), status: "error", error: "Unavailable" }}
      />,
    );

    expect(container.firstElementChild).toHaveClass("bg-card", "border-border");
    expect(container.firstElementChild?.className).not.toMatch(/gradient|shadow/);
    expect(container.querySelector(".bg-status-negative")).toBeInTheDocument();
    expect(screen.getByText("Unavailable")).toBeInTheDocument();
  });

  it("renders optional sparkline history with the compact current value", () => {
    const { container } = render(
      <MetricWidgetCard
        metric={descriptor()}
        latest={latest("2048")}
        series={[
          { timestamp: 1, value: 1024 },
          { timestamp: 2, value: 2048 },
          { timestamp: 3, value: 1536 },
        ]}
      />,
    );

    expect(screen.getByText("Network RX")).toBeInTheDocument();
    expect(screen.getByText("2 KiB/s")).toBeInTheDocument();
    expect(screen.getByRole("img", { name: "Recent values" })).toBeInTheDocument();
    expect(container.querySelector("[data-testid='metric-sparkline-line']")).toHaveAttribute(
      "d",
      expect.stringContaining("L"),
    );
    expect(screen.queryByText("Instant value only")).not.toBeInTheDocument();
  });
});
