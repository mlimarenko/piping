import { describe, expect, it } from "vitest";

import { buildFilterParams } from "@/shared/ui-shared/tableState";
import {
  PLAYBOOK_ASSIGNMENTS_TABLE_CONFIG,
  PLAYBOOK_RUN_GROUPS_TABLE_CONFIG,
  PLAYBOOK_SCHEDULES_TABLE_CONFIG,
  PLAYBOOK_TARGET_SETS_TABLE_CONFIG,
} from "./tableConfig";

describe("playbooks table config", () => {
  it("keeps assignment filters declarative", () => {
    expect(PLAYBOOK_ASSIGNMENTS_TABLE_CONFIG.sortKeys).toContain("quick_action_enabled");
    expect(
      buildFilterParams(
        {
          job_template: ["pb-1"],
          quick_action_enabled: ["true"],
        },
        PLAYBOOK_ASSIGNMENTS_TABLE_CONFIG.filterColumns,
      ),
    ).toEqual({
      job_template_id: "pb-1",
      quick_action_enabled: "true",
    });
  });

  it("does not send retired catalog filters from persisted run and schedule table state", () => {
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
      enabled: "false",
    });
    expect(
      buildFilterParams(
        {
          playbook: ["pb-1"],
          status: ["running"],
        },
        PLAYBOOK_RUN_GROUPS_TABLE_CONFIG.filterColumns,
      ),
    ).toEqual({
      status: "running",
    });
  });
});
