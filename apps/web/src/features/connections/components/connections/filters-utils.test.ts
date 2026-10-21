import { describe, expect, it } from "vitest";
import {
  applyFilters,
  matchesTargetSearch,
  cloneFilterState,
  countActiveFilters,
  deserializeFilterState,
  emptyFilterState,
  filterStateEquals,
  isSerializableFilterStateEmpty,
  normalizeFilterPresets,
  serializeFilterState,
  type FilterState,
} from "./filters-utils";
import type { ConnectionTarget } from "@/shared/api/types";
import { testUuid } from "@/test/ids";

describe("connection filter utils", () => {
  it.each(["name", "region", "site", "tenant", "role", "platform", "deviceType", "cluster", "primaryIp"])("matches %s consistently for Workspace and location search", field => {
    const target: ConnectionTarget = { id: testUuid("search-target"), name: "Other", targetKind: "device", [field]: "Unique Context" };
    expect(matchesTargetSearch(target, " unique CONTEXT ")).toBe(true);
    expect(matchesTargetSearch(target, "unmatched")).toBe(false);
    expect(matchesTargetSearch(target, "  ")).toBe(true);
  });

  it("counts active filter values", () => {
    const state = emptyFilterState();
    state.status.add("active");
    state.region.add("Moscow");
    state.region.add("Saint Petersburg");

    expect(countActiveFilters(state)).toBe(3);
  });

  it("serializes and deserializes stable filter state", () => {
    const state = emptyFilterState();
    state.site.add("Warehouse B");
    state.site.add("Warehouse A");
    state.role.add("edge");
    state.targetKind.add("device");

    const serialized = serializeFilterState(state);

    expect(serialized).toEqual({
      targetKind: ["device"],
      role: ["edge"],
      site: ["Warehouse A", "Warehouse B"],
    });
    expect(deserializeFilterState(serialized).site.has("Warehouse A")).toBe(true);
    expect(countActiveFilters(deserializeFilterState(serialized))).toBe(4);
  });

  it("normalizes saved filter presets and removes empty presets", () => {
    const presets = normalizeFilterPresets(
      [
        {
          id: "one",
          name: "Active Moscow",
          filters: { status: ["active", "active"], region: [" Moscow "] },
          createdAt: "2026-06-02T00:00:00.000Z",
          updatedAt: "2026-06-02T00:00:00.000Z",
        },
        {
          id: "empty",
          name: "Empty",
          filters: {},
        },
      ],
      "private",
    );

    expect(presets).toHaveLength(1);
    expect(presets[0]!).toMatchObject({
      id: "one",
      name: "Active Moscow",
      visibility: "private",
      filters: { status: ["active"], region: ["Moscow"] },
    });
    expect(isSerializableFilterStateEmpty(presets[0]!.filters)).toBe(false);
  });

  it("applies derived host filters", () => {
    const hosts: ConnectionTarget[] = [
      {
        id: testUuid("device-1"),
        name: "web-prod-01",
        targetKind: "device",
        status: "active",
        primaryIp: "10.20.0.11",
        addressBound: true,
      },
      {
        id: testUuid("vm-1"),
        name: "app-node-02",
        targetKind: "virtual_machine",
        status: "active",
        primaryIp: "10.20.1.32",
        addressBound: false,
      },
      {
        id: testUuid("device-2"),
        name: "orphan-router",
        targetKind: "device",
        status: "offline",
      },
    ];
    const selected = emptyFilterState();
    selected.targetKind.add("virtual_machine");
    selected.ipSegment.add("10.20.1.0/24");
    selected.addressBinding.add("unbound");

    expect(applyFilters([...hosts], selected).map((host) => host.name)).toEqual(["app-node-02"]);

    selected.targetKind.clear();
    selected.ipSegment.clear();
    selected.addressBinding.clear();
    selected.addressState.add("missing_address");

    expect(applyFilters([...hosts], selected).map((host) => host.name)).toEqual(["orphan-router"]);
  });
});

describe("filter draft helpers", () => {
  it("clones every group so editing the draft never mutates the applied state", () => {
    const applied = emptyFilterState();
    applied.status.add("online");
    applied.site.add("dc-1");

    const draft = cloneFilterState(applied);
    draft.status.add("offline");
    draft.site.delete("dc-1");

    expect([...applied.status]).toEqual(["online"]);
    expect([...applied.site]).toEqual(["dc-1"]);
    expect([...draft.status].sort()).toEqual(["offline", "online"]);
    expect([...draft.site]).toEqual([]);
  });

  it("compares selections across every group regardless of insertion order", () => {
    const left: FilterState = emptyFilterState();
    left.region.add("eu");
    left.region.add("us");
    const right: FilterState = emptyFilterState();
    right.region.add("us");
    right.region.add("eu");

    expect(filterStateEquals(left, right)).toBe(true);

    right.region.add("ap");
    expect(filterStateEquals(left, right)).toBe(false);
  });

  it("treats an unedited clone as equal so Apply can be a no-op", () => {
    const applied: FilterState = emptyFilterState();
    applied.role.add("gateway");
    expect(filterStateEquals(applied, cloneFilterState(applied))).toBe(true);
  });
});
