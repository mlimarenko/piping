import { describe, expect, it } from "vitest";

import { buildFilterParams } from "@/shared/ui-shared/tableState";
import {
  PLAYBOOK_ASSIGNMENTS_TABLE_CONFIG,
  PLAYBOOK_RUN_GROUPS_TABLE_CONFIG,
  PLAYBOOK_SCHEDULES_TABLE_CONFIG,
  PLAYBOOK_SERVICE_PRINCIPALS_TABLE_CONFIG,
  PLAYBOOK_TARGET_SETS_TABLE_CONFIG,
} from "./tableConfig";

describe("playbooks table config", () => {
  it("keeps assignment filters declarative", () => {
    expect(PLAYBOOK_ASSIGNMENTS_TABLE_CONFIG.sortKeys).toContain("quick_action_enabled");
    expect(
      buildFilterParams(
        {
          playbook: ["pb-1"],
          quick_action_enabled: ["true"],
        },
        PLAYBOOK_ASSIGNMENTS_TABLE_CONFIG.filterColumns,
      ),
    ).toEqual({
      playbook_id: "pb-1",
      quick_action_enabled: "true",
    });
  });

  it("keeps the remaining playbook tables on declarative filter params", () => {
    expect(
      buildFilterParams(
        { mode: ["dynamic"] },
        PLAYBOOK_TARGET_SETS_TABLE_CONFIG.filterColumns,
      ),
    ).toEqual({ mode: "dynamic" });
    expect(
      buildFilterParams(
        {
          playbook: ["pb-1"],
          target_set: ["ts-1"],
          concurrency: ["queue_pending"],
          enabled: ["false"],
        },
        PLAYBOOK_SCHEDULES_TABLE_CONFIG.filterColumns,
      ),
    ).toEqual({
      playbook_id: "pb-1",
      target_set_id: "ts-1",
      concurrency_policy: "queue_pending",
      enabled: "false",
    });
    expect(
      buildFilterParams(
        { status: ["active"] },
        PLAYBOOK_SERVICE_PRINCIPALS_TABLE_CONFIG.filterColumns,
      ),
    ).toEqual({ status: "active" });
    expect(
      buildFilterParams(
        {
          playbook: ["pb-1"],
          status: ["running"],
        },
        PLAYBOOK_RUN_GROUPS_TABLE_CONFIG.filterColumns,
      ),
    ).toEqual({
      playbook_id: "pb-1",
      status: "running",
    });
  });
});
