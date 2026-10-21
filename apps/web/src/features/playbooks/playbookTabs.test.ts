import { describe, expect, it, vi } from "vitest";
import type { TFunction } from "i18next";

import { ApiError } from "@/shared/api/client";
import type { PlaybookRun, PlaybookRunGroupDetail, PlaybookRunStatus } from "@/features/playbooks/api";
import type { RunDetailTarget } from "@/features/playbooks/RunDetailPanel";
import {
  buildMixedTabOrder,
  buildPlaybookTab,
  EMPTY_PERSISTED_PLAYBOOK_TABS,
  normalizePersistedPlaybookTabs,
  playbookHostSummaryFromCount,
  playbookHostSummaryFromRuns,
  playbookStatusDotClassName,
  restorePersistedPlaybookTabs,
  toPersistedPlaybookTabs,
  type PersistedPlaybookTabs,
  type PlaybookTab,
} from "@/features/playbooks/playbookTabs";

// A stand-in for react-i18next's `t`: returns the key with interpolated `count`
// so assertions can check both the key path and the value without a full
// i18next instance.
const t = ((key: string, opts?: { count?: number }) =>
  opts?.count !== undefined ? `${key}:${opts.count}` : key) as unknown as TFunction;

function run(overrides: Partial<PlaybookRun> = {}): PlaybookRun {
  return {
    attempt: 1,
    credential_account_kind: "none",
    credential_merge_policy: "merge",
    credential_resolution: {},
    id: "run-1",
    metadata: {},
    playbook_id: "playbook-1",
    playbook_version_id: "version-1",
    queued_at: "2026-06-29T00:00:00Z",
    run_group_id: "group-1",
    source_id: "host-1",
    source_kind: "device",
    status: "running",
    target_address_bound: true,
    target_id: "host-1",
    target_kind: "device",
    timeout_seconds: 900,
    ...overrides,
  };
}

function groupDetail(status: PlaybookRunStatus, overrides: Partial<PlaybookRunGroupDetail["run_group"]> = {}): PlaybookRunGroupDetail {
  return {
    run_group: {
      canceled_runs: 0,
      created_at: "2026-06-29T00:00:00Z",
      failed_runs: 0,
      id: "group-1",
      metadata: {},
      playbook_id: "playbook-1",
      playbook_version_id: "version-1",
      status,
      succeeded_runs: 0,
      timeout_runs: 0,
      total_runs: 1,
      dispatch_id: null,
      dispatch_attempt: 0,
      trigger_kind: "manual",
      updated_at: "2026-06-29T00:00:00Z",
      ...overrides,
    },
    runs: [],
  };
}

describe("normalizePersistedPlaybookTabs / toPersistedPlaybookTabs", () => {
  it("round-trips a valid persisted shape", () => {
    const persisted: PersistedPlaybookTabs = {
      version: 1,
      tabs: [
        {
          id: "group-1",
          groupId: "group-1",
          playbookName: "Update inventory",
          hostSummary: "3 hosts",
          startedAt: "2026-06-29T00:00:00Z",
        },
      ],
    };
    expect(normalizePersistedPlaybookTabs(persisted)).toEqual(persisted);
  });

  it("drops entries missing a required string field", () => {
    const raw = {
      version: 1,
      tabs: [
        { id: "group-1", groupId: "group-1", playbookName: "Update inventory", hostSummary: "", startedAt: "2026-06-29T00:00:00Z" },
        { id: "group-2", groupId: "group-2", playbookName: "Update inventory", hostSummary: "1 host", startedAt: "2026-06-29T00:00:00Z" },
      ],
    };
    expect(normalizePersistedPlaybookTabs(raw)).toEqual({
      version: 1,
      tabs: [
        { id: "group-2", groupId: "group-2", playbookName: "Update inventory", hostSummary: "1 host", startedAt: "2026-06-29T00:00:00Z" },
      ],
    });
  });

  it("rejects a bad shape (wrong version, non-array tabs, non-object, null, string)", () => {
    expect(normalizePersistedPlaybookTabs({ version: 2, tabs: [] })).toEqual(EMPTY_PERSISTED_PLAYBOOK_TABS);
    expect(normalizePersistedPlaybookTabs({ version: 1, tabs: "not-an-array" })).toEqual(EMPTY_PERSISTED_PLAYBOOK_TABS);
    expect(normalizePersistedPlaybookTabs(null)).toEqual(EMPTY_PERSISTED_PLAYBOOK_TABS);
    expect(normalizePersistedPlaybookTabs("garbage")).toEqual(EMPTY_PERSISTED_PLAYBOOK_TABS);
    expect(normalizePersistedPlaybookTabs(undefined)).toEqual(EMPTY_PERSISTED_PLAYBOOK_TABS);
  });

  it("drops tabs with no restorable group id when projecting to the persisted shape", () => {
    const tabs: PlaybookTab[] = [
      {
        id: "seed:abc",
        groupId: "",
        playbookName: "Ad-hoc",
        hostSummary: "1 host",
        phase: "queued",
        target: { kind: "seed", runs: [] },
        startedAt: "2026-06-29T00:00:00Z",
      },
      {
        id: "group-9",
        groupId: "group-9",
        playbookName: "Update inventory",
        hostSummary: "2 hosts",
        phase: "running",
        target: { kind: "group", groupId: "group-9" },
        startedAt: "2026-06-29T00:00:00Z",
      },
    ];
    expect(toPersistedPlaybookTabs(tabs)).toEqual({
      version: 1,
      tabs: [
        { id: "group-9", groupId: "group-9", playbookName: "Update inventory", hostSummary: "2 hosts", startedAt: "2026-06-29T00:00:00Z" },
      ],
    });
  });
});

describe("playbookHostSummaryFromRuns / playbookHostSummaryFromCount", () => {
  it("uses the single target's name for a single run", () => {
    expect(playbookHostSummaryFromRuns([run({ target_name: "Касса 603-1" })], t)).toBe("Касса 603-1");
  });

  it("falls back to target_address, then a generic fallback key, when the name is missing", () => {
    expect(playbookHostSummaryFromRuns([run({ target_name: null, target_address: "10.0.0.5" })], t)).toBe("10.0.0.5");
    expect(playbookHostSummaryFromRuns([run({ target_name: null, target_address: null })], t)).toBe(
      "playbookTab.singleHostFallback",
    );
  });

  it("uses an N-hosts summary for more than one run", () => {
    expect(playbookHostSummaryFromRuns([run(), run({ id: "run-2" })], t)).toBe("playbookTab.hostsCount:2");
  });

  it("derives the same N-hosts summary from a bare count", () => {
    expect(playbookHostSummaryFromCount(5, t)).toBe("playbookTab.hostsCount:5");
  });
});

describe("buildPlaybookTab", () => {
  it("builds a tab from a seed target with the group id and a queued phase", () => {
    const target: RunDetailTarget = { kind: "seed", runs: [run({ target_name: "spb-hv5" })], groupId: "group-1" };
    const tab = buildPlaybookTab(target, "Update inventory", t);
    expect(tab.id).toBe("group-1");
    expect(tab.groupId).toBe("group-1");
    expect(tab.playbookName).toBe("Update inventory");
    expect(tab.hostSummary).toBe("spb-hv5");
    expect(tab.phase).toBe("queued");
    expect(tab.target).toBe(target);
    expect(typeof tab.startedAt).toBe("string");
  });

  it("synthesizes a local seed id when the target carries no group id", () => {
    const target: RunDetailTarget = { kind: "seed", runs: [run()] };
    const tab = buildPlaybookTab(target, "Ad-hoc", t);
    expect(tab.groupId).toBe("");
    expect(tab.id.startsWith("seed:")).toBe(true);
  });
});

describe("restorePersistedPlaybookTabs", () => {
  const persisted: PersistedPlaybookTabs = {
    version: 1,
    tabs: [
      { id: "group-running", groupId: "group-running", playbookName: "Update inventory", hostSummary: "1 host", startedAt: "2026-06-29T00:00:00Z" },
      { id: "group-succeeded", groupId: "group-succeeded", playbookName: "Update inventory", hostSummary: "1 host", startedAt: "2026-06-29T00:00:00Z" },
      { id: "group-missing", groupId: "group-missing", playbookName: "Update inventory", hostSummary: "1 host", startedAt: "2026-06-29T00:00:00Z" },
    ],
  };

  it("keeps only the non-terminal groups and drops a definite 404", async () => {
    const fetchGroup = vi.fn((groupId: string) => {
      if (groupId === "group-running") return Promise.resolve(groupDetail("running"));
      if (groupId === "group-succeeded") return Promise.resolve(groupDetail("succeeded"));
      return Promise.reject(new ApiError({ status: 404, message: "not found" }));
    });

    const restored = await restorePersistedPlaybookTabs(persisted, fetchGroup);

    expect(fetchGroup).toHaveBeenCalledTimes(3);
    expect(restored).toHaveLength(1);
    expect(restored[0]).toMatchObject({
      id: "group-running",
      groupId: "group-running",
      phase: "running",
      target: { kind: "group", groupId: "group-running" },
    });
  });

  it("restores nothing when every persisted group is already terminal", async () => {
    const fetchGroup = vi.fn(() => Promise.resolve(groupDetail("failed")));
    const restored = await restorePersistedPlaybookTabs(persisted, fetchGroup);
    expect(restored).toEqual([]);
  });

  it("keeps the tab on a transient failure (network/5xx) instead of erasing it", async () => {
    const fetchGroup = vi.fn((groupId: string) => {
      if (groupId === "group-running") return Promise.resolve(groupDetail("running"));
      if (groupId === "group-succeeded") return Promise.reject(new ApiError({ status: 503, message: "unavailable" }));
      return Promise.reject(new TypeError("Failed to fetch"));
    });

    const restored = await restorePersistedPlaybookTabs(persisted, fetchGroup);

    // Only the definite-404 case (there is none here) would be dropped; both
    // the 503 and the network-level failure keep their persisted tab with a
    // best-effort "running" phase pending their own live query.
    expect(restored.map((tab) => tab.id).sort()).toEqual([
      "group-missing",
      "group-running",
      "group-succeeded",
    ]);
    const keptOnError = restored.find((tab) => tab.id === "group-succeeded");
    expect(keptOnError).toMatchObject({ phase: "running", target: { kind: "group", groupId: "group-succeeded" } });
  });
});

describe("buildMixedTabOrder", () => {
  it("orders session tabs first, then playbook tabs", () => {
    const order = buildMixedTabOrder(["s1", "s2"], ["p1", "p2"]);
    expect(order).toEqual([
      { kind: "session", id: "s1" },
      { kind: "session", id: "s2" },
      { kind: "playbook", id: "p1" },
      { kind: "playbook", id: "p2" },
    ]);
  });

  it("handles an empty side of either kind", () => {
    expect(buildMixedTabOrder([], ["p1"])).toEqual([{ kind: "playbook", id: "p1" }]);
    expect(buildMixedTabOrder(["s1"], [])).toEqual([{ kind: "session", id: "s1" }]);
    expect(buildMixedTabOrder([], [])).toEqual([]);
  });
});

describe("playbookStatusDotClassName", () => {
  it("maps each status bucket to a distinct tone class", () => {
    expect(playbookStatusDotClassName("queued")).toContain("positive");
    expect(playbookStatusDotClassName("running")).toContain("positive");
    expect(playbookStatusDotClassName("succeeded")).not.toContain("positive");
    expect(playbookStatusDotClassName("failed")).toContain("negative");
    expect(playbookStatusDotClassName("timed_out")).toContain("negative");
    expect(playbookStatusDotClassName("canceled")).toContain("warning");
  });
});
