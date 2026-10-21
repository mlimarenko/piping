import { describe, expect, it } from "vitest";
import {
  CONNECTION_TARGETS_TABLE_CONFIG,
  buildConnectionTargetFilterOptions,
  connectionTargetsFilterParam,
  connectionTargetsSortParam,
  connectionTargetsTableId,
  connectionTargetsViewColumnKeys,
} from "./tableConfig";
import type { ConnectionTarget } from "@/shared/api/types";

describe("connection targets table config", () => {
  it("keeps view column keys unique and registered", () => {
    for (const view of ["mobile", "desktop"] as const) {
      const keys = connectionTargetsViewColumnKeys(view);
      expect(new Set(keys).size).toBe(keys.length);
      for (const key of keys) {
        expect(CONNECTION_TARGETS_TABLE_CONFIG.columns[key]).toBeDefined();
      }
    }
  });

  it("uses distinct table ids for responsive schemas", () => {
    expect(connectionTargetsTableId("mobile")).toBe("connections.targets.mobile");
    expect(connectionTargetsTableId("desktop")).toBe("connections.targets.desktop");
    expect(connectionTargetsTableId("mobile")).not.toBe(connectionTargetsTableId("desktop"));
  });

  it("keeps every sortable rendered column mapped to a backend sort param", () => {
    for (const view of ["mobile", "desktop"] as const) {
      for (const key of connectionTargetsViewColumnKeys(view)) {
        const column = CONNECTION_TARGETS_TABLE_CONFIG.columns[key];
        if (!column.sortable) continue;
        expect(connectionTargetsSortParam(key)).toBeTruthy();
      }
    }
    expect(connectionTargetsSortParam("primaryIp")).toBe("primaryIp");
  });

  it("keeps every filterable rendered column mapped to a unique backend filter param", () => {
    const params = new Map<string, string>();
    for (const key of connectionTargetsViewColumnKeys("desktop")) {
      const param = connectionTargetsFilterParam(key);
      if (!param) continue;
      expect(params.has(param), `${key} reuses backend filter param ${param}`).toBe(false);
      params.set(param, key);
    }
    expect(params.get("target_kind")).toBe("targetKind");
    expect(params.get("device_type_id")).toBe("deviceType");
  });

  it("builds stable filter options for all configured filter sources", () => {
    const hosts: ConnectionTarget[] = [
      {
        id: "00000000-0000-0000-0000-000000000001" as ConnectionTarget["id"],
        name: "ares",
        targetKind: "device",
        status: "active",
        region: "North",
        regionId: "10000000-0000-0000-0000-000000000001" as ConnectionTarget["regionId"],
        site: "HQ",
        siteId: "20000000-0000-0000-0000-000000000001" as ConnectionTarget["siteId"],
        tenant: "Grad",
        tenantId: "30000000-0000-0000-0000-000000000001" as ConnectionTarget["tenantId"],
        role: "Cashier",
        roleId: "40000000-0000-0000-0000-000000000001" as ConnectionTarget["roleId"],
        platform: "Ubuntu",
        platformId: "50000000-0000-0000-0000-000000000001" as ConnectionTarget["platformId"],
        deviceType: "POS",
        deviceTypeId: "60000000-0000-0000-0000-000000000001" as ConnectionTarget["deviceTypeId"],
      },
      {
        id: "00000000-0000-0000-0000-000000000002" as ConnectionTarget["id"],
        name: "hermes",
        targetKind: "virtual_machine",
        status: "active",
        site: "HQ",
        siteId: "20000000-0000-0000-0000-000000000001" as ConnectionTarget["siteId"],
      },
    ];

    const options = buildConnectionTargetFilterOptions(
      hosts,
      ((key: string, defaultOrOptions?: string | { defaultValue?: string }) =>
        typeof defaultOrOptions === "string"
          ? defaultOrOptions
          : defaultOrOptions?.defaultValue ?? key) as never,
    );

    expect(options.status?.find((option) => option.value === "active")?.count).toBe(2);
    expect(options.targetKind?.map((option) => option.value)).toEqual(["device", "virtual_machine"]);
    const firstHost = hosts[0];
    expect(firstHost).toBeDefined();
    const firstSiteId = firstHost?.siteId;
    expect(options.site?.find((option) => option.value === firstSiteId)?.count).toBe(2);
  });
});
