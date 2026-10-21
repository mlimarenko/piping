import { describe, expect, it } from "vitest";

import type { LatestMetricItem, MonitoringMetricDescriptor } from "@/features/observability/api";
import {
  groupMetricsByCategory,
  metricHasValue,
  metricValueSummary,
  UNCATEGORIZED_CATEGORY_KEY,
} from "./metricHelpers";

function descriptor(over: Partial<MonitoringMetricDescriptor>): MonitoringMetricDescriptor {
  return {
    name: "m",
    label: "M",
    query: "q",
    unit: null,
    value_type: "number",
    widget_type: "stat",
    source_type: "prometheus",
    query_type: "promql",
    options: {},
    show_in_list: false,
    list_render: "auto",
    ...over,
  };
}

describe("groupMetricsByCategory", () => {
  it("buckets named categories in first-seen order and pins uncategorized last", () => {
    const groups = groupMetricsByCategory(
      [
        descriptor({ name: "a", category: "Compute" }),
        descriptor({ name: "b", category: null }),
        descriptor({ name: "c", category: "Compute" }),
        descriptor({ name: "d", category: "Network" }),
        descriptor({ name: "e" }),
      ],
      "Other",
    );

    expect(groups).toHaveLength(3);
    const [compute, network, uncategorized] = groups;
    expect(compute).toMatchObject({ key: "Compute", title: "Compute" });
    expect(compute?.metrics.map((m) => m.name)).toEqual(["a", "c"]);
    expect(network).toMatchObject({ key: "Network", title: "Network" });
    expect(network?.metrics.map((m) => m.name)).toEqual(["d"]);
    expect(uncategorized).toMatchObject({ key: UNCATEGORIZED_CATEGORY_KEY, title: "Other" });
    expect(uncategorized?.metrics.map((m) => m.name)).toEqual(["b", "e"]);
  });

  it("returns a single uncategorized bucket when no metric carries a category", () => {
    const groups = groupMetricsByCategory(
      [descriptor({ name: "a" }), descriptor({ name: "b", category: "   " })],
      "Other",
    );
    expect(groups).toHaveLength(1);
    expect(groups[0]?.key).toBe(UNCATEGORIZED_CATEGORY_KEY);
  });
});

function prometheusLatest(value: string | null, overrides: Partial<LatestMetricItem> = {}): LatestMetricItem {
  return {
    query: "metric",
    executed_query: "metric",
    name: "metric",
    label: "Metric",
    unit: null,
    value_type: "number",
    widget_type: "stat",
    source_type: "prometheus",
    status: "success",
    value: {
      status: "success",
      data: {
        resultType: "vector",
        result: value === null ? [] : [{ metric: {}, value: [1, value] }],
      },
    },
    error: null,
    ...overrides,
  };
}

describe("metric helpers", () => {
  it("formats byte rates with human-readable units", () => {
    const item = prometheusLatest("547705.7");

    expect(metricHasValue(item)).toBe(true);
    expect(metricValueSummary(item, { value_type: "number", unit: "B/s" })).toBe("535 KiB/s");
  });

  it("formats duration units when the query value type is automatic", () => {
    const item = prometheusLatest("3358800", { value_type: "auto", unit: "s" });

    expect(metricValueSummary(item)).toBe("38d 21h");
  });

  it("treats empty Prometheus vectors as missing data", () => {
    const item = prometheusLatest(null);

    expect(metricHasValue(item)).toBe(false);
    expect(metricValueSummary(item, { value_type: "number", unit: "B/s" })).toBe("—");
  });
});
