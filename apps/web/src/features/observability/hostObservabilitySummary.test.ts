import { describe, expect, it } from "vitest";

import type {
  LatestMetricItem,
  MonitoringMetricDescriptor,
} from "@/features/observability/api";
import { summarizeHostObservability } from "./hostObservabilitySummary";

function descriptor(
  name: string,
  valueType: string,
  unit: string | null = null,
  options: unknown = {},
): MonitoringMetricDescriptor {
  return {
    name,
    label: name,
    query: name,
    unit,
    value_type: valueType,
    widget_type: "stat",
    description: null,
    source_type: "prometheus",
    query_type: "promql",
    options,
    show_in_list: false,
    list_render: "auto",
  };
}

function latest(query: string, value: string, valueType = "number"): LatestMetricItem {
  return {
    query,
    executed_query: query,
    name: query,
    label: query,
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

function failed(query: string): LatestMetricItem {
  return {
    ...latest(query, "0"),
    status: "error",
    value: null,
    error: "source unavailable",
  };
}

describe("summarizeHostObservability", () => {
  it("returns normal when all visible metrics have healthy values", () => {
    const metric = descriptor("up", "state", "state");

    const summary = summarizeHostObservability(
      [metric],
      new Map([["up", latest("up", "1", "state")]]),
      false,
    );

    expect(summary).toMatchObject({
      state: "normal",
      ok: 1,
      warning: 0,
      critical: 0,
      unknown: 0,
      total: 1,
    });
  });

  it("prioritizes critical over warning and unknown", () => {
    const metrics = [
      descriptor("cpu", "percent", "%"),
      descriptor("memory", "percent", "%"),
      descriptor("disk", "percent", "%"),
    ];

    const summary = summarizeHostObservability(
      metrics,
      new Map([
        ["cpu", latest("cpu", "91", "percent")],
        ["memory", latest("memory", "85", "percent")],
      ]),
      false,
    );

    expect(summary).toMatchObject({
      state: "critical",
      critical: 1,
      warning: 1,
      unknown: 1,
      ok: 0,
      total: 3,
    });
  });

  it("treats latest query errors as critical", () => {
    const metric = descriptor("up", "state", "state");

    const summary = summarizeHostObservability([metric], new Map([["up", failed("up")]]), true);

    expect(summary).toMatchObject({
      state: "critical",
      critical: 1,
      stale: true,
    });
  });
});
