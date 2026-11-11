import { describe, expect, it } from "vitest";

import type {
  LatestMetricItem,
  MonitoringMetricDescriptor,
} from "@/features/observability/api";
import { metricCardStatus } from "./metricDisplay";

function descriptor(
  valueType: string,
  unit: string | null = null,
  options: unknown = {},
): MonitoringMetricDescriptor {
  return {
    name: "metric",
    label: "Metric",
    query: "metric",
    unit,
    value_type: valueType,
    widget_type: "stat",
    description: "Metric",
    source_type: "prometheus",
    query_type: "promql",
    options,
    show_in_list: false,
    list_render: "auto",
  };
}

function latest(value: string, valueType: string): LatestMetricItem {
  return {
    query: "metric",
    executed_query: "metric",
    name: "metric",
    label: "Metric",
    unit: null,
    value_type: valueType,
    widget_type: "stat",
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

describe("metricCardStatus", () => {
  it("maps state metrics to normal and critical", () => {
    const metric = descriptor("state", "state");

    expect(metricCardStatus(metric, latest("1", "state"), "up")).toBe("normal");
    expect(metricCardStatus(metric, latest("0", "state"), "down")).toBe("critical");
  });

  it("uses auto thresholds for percentages", () => {
    const metric = descriptor("percent", "%");

    expect(metricCardStatus(metric, latest("42", "percent"), "42%")).toBe("normal");
    expect(metricCardStatus(metric, latest("85", "percent"), "85%")).toBe("warning");
    expect(metricCardStatus(metric, latest("95", "percent"), "95%")).toBe("critical");
  });

  it("keeps automatic state coloring when status mode is auto", () => {
    const metric = descriptor("state", "state", { display: { status: { mode: "auto" } } });

    expect(metricCardStatus(metric, latest("1", "state"), "up")).toBe("normal");
    expect(metricCardStatus(metric, latest("0", "state"), "down")).toBe("critical");
  });

  it("keeps automatic percent thresholds when status mode is auto", () => {
    const metric = descriptor("percent", "%", { display: { status: { mode: "auto" } } });

    expect(metricCardStatus(metric, latest("85", "percent"), "85%")).toBe("warning");
    expect(metricCardStatus(metric, latest("95", "percent"), "95%")).toBe("critical");
  });

  it("uses custom numeric thresholds and lower-is-worse direction", () => {
    const metric = descriptor("number", null, {
      display: {
        status: {
          mode: "numeric",
          direction: "lower_is_worse",
          warning: 20,
          critical: 10,
        },
      },
    });

    expect(metricCardStatus(metric, latest("30", "number"), "30")).toBe("normal");
    expect(metricCardStatus(metric, latest("15", "number"), "15")).toBe("warning");
    expect(metricCardStatus(metric, latest("5", "number"), "5")).toBe("critical");
  });

  it("allows status coloring to be disabled", () => {
    const metric = descriptor("percent", "%", { display: { status: { mode: "none" } } });

    expect(metricCardStatus(metric, latest("95", "percent"), "95%")).toBe("neutral");
  });
});
