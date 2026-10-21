import { describe, expect, it } from "vitest";

import type { ConnectionTarget } from "@/shared/api/types";
import type { Uuid } from "@/shared/lib/ids";

import {
  toPlaybookLaunchTarget,
  toPlaybookStaticTargetSetMember,
  toSelectedHost,
} from "./hostSelection";

function uuid(value: string): Uuid {
  return value as Uuid;
}

function connectionTarget(overrides: Partial<ConnectionTarget> = {}): ConnectionTarget {
  return {
    id: uuid("11111111-1111-1111-1111-111111111111"),
    name: "cash-1",
    targetKind: "device",
    role: "Cash",
    site: "Site 1",
    primaryIp: "10.0.0.10",
    deviceId: uuid("22222222-2222-2222-2222-222222222222"),
    ...overrides,
  };
}

describe("playbook host selection projections", () => {
  it("projects connection targets into launch dialog summaries with backing source identity", () => {
    const host = connectionTarget({
      targetKind: "virtual_machine",
      deviceId: undefined,
      vmId: uuid("33333333-3333-3333-3333-333333333333"),
    });

    expect(toPlaybookLaunchTarget(host, "Site 1 · VM · 10.0.0.10")).toEqual({
      id: "11111111-1111-1111-1111-111111111111",
      name: "cash-1",
      subtitle: "Site 1 · VM · 10.0.0.10",
      sourceKind: "virtual_machine",
      sourceId: "33333333-3333-3333-3333-333333333333",
    });
  });

  it("uses the same source identity for selected hosts and static target-set members", () => {
    const host = connectionTarget();

    expect(toSelectedHost(host)).toMatchObject({
      id: "11111111-1111-1111-1111-111111111111",
      sourceKind: "device",
      sourceId: "22222222-2222-2222-2222-222222222222",
    });
    expect(toPlaybookStaticTargetSetMember(host, 7)).toEqual({
      target_kind: "connection_target",
      target_id: "11111111-1111-1111-1111-111111111111",
      source_kind: "device",
      source_id: "22222222-2222-2222-2222-222222222222",
      sort_order: 7,
    });
  });

  it("keeps missing source identity visible in summaries but falls back for static members", () => {
    const host = connectionTarget({
      deviceId: undefined,
    });

    expect(toPlaybookLaunchTarget(host)).toMatchObject({
      sourceKind: "device",
      sourceId: null,
    });
    expect(toPlaybookStaticTargetSetMember(host, 0)).toMatchObject({
      source_kind: "device",
      source_id: "11111111-1111-1111-1111-111111111111",
    });
  });
});
