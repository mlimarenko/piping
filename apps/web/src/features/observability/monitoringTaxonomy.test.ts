import { describe, expect, it } from "vitest";
import {
  defaultQueryTypeForSourceType,
  isMetricQueryTypeCompatible,
  queryTypesForSourceType,
} from "./monitoringTaxonomy";

describe("monitoring taxonomy", () => {
  it("treats Prometheus and VictoriaMetrics as PromQL-compatible sources", () => {
    expect(queryTypesForSourceType("prometheus")).toEqual(["promql"]);
    expect(queryTypesForSourceType("victoriametrics")).toEqual(["promql"]);
    expect(defaultQueryTypeForSourceType("victoriametrics")).toBe("promql");
    expect(isMetricQueryTypeCompatible("prometheus", "promql")).toBe(true);
    expect(isMetricQueryTypeCompatible("victoriametrics", "promql")).toBe(true);
  });

  it("keeps Zabbix item selectors separate from PromQL", () => {
    expect(queryTypesForSourceType("zabbix")).toEqual([
      "zabbix_item_key",
      "zabbix_item_name",
      "zabbix_itemid",
    ]);
    expect(defaultQueryTypeForSourceType("zabbix")).toBe("zabbix_item_key");
    expect(isMetricQueryTypeCompatible("zabbix", "zabbix_item_key")).toBe(true);
    expect(isMetricQueryTypeCompatible("zabbix", "promql")).toBe(false);
    expect(isMetricQueryTypeCompatible("prometheus", "zabbix_item_key")).toBe(false);
  });
});
