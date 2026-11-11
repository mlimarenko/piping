import { describe, expect, it } from "vitest";

import type { ConnectionTarget } from "@/shared/api/types";
import type { Uuid } from "@/shared/lib/ids";

import {
  selectedHostsSignature,
  selectedHostsToStaticTargetSetMembers,
  staticTargetSetMembersToHosts,
  toPlaybookLaunchTarget,
  toPlaybookStaticTargetSetMember,
  toSelectedHost,
  type PlaybookTargetSetMember,
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

  it("preserves missing source identity instead of inventing a device ID from the target ID", () => {
    const host = connectionTarget({
      deviceId: undefined,
    });

    expect(toPlaybookLaunchTarget(host)).toMatchObject({
      sourceKind: "device",
      sourceId: null,
    });
    expect(toPlaybookStaticTargetSetMember(host, 0)).toMatchObject({
      source_kind: "device",
      source_id: null,
    });
  });

  it("rehydrates persisted members in sort order and keeps unknown members selectable", () => {
    const first = connectionTarget();
    const second = connectionTarget({
      id: uuid("44444444-4444-4444-4444-444444444444"),
      name: "cash-2",
      deviceId: uuid("55555555-5555-5555-5555-555555555555"),
    });
    const members: PlaybookTargetSetMember[] = [
      { target_kind: "connection_target", target_id: second.id, source_kind: "device", source_id: second.deviceId!, sort_order: 1 },
      { target_kind: "connection_target", target_id: first.id, source_kind: "device", source_id: first.deviceId!, sort_order: 0 },
      { target_kind: "connection_target", target_id: "missing", source_kind: "virtual_machine", source_id: "missing-source", sort_order: 2 },
    ];

    expect(staticTargetSetMembersToHosts(members, [first, second])).toEqual([
      expect.objectContaining({ id: first.id, name: "cash-1" }),
      expect.objectContaining({ id: second.id, name: "cash-2" }),
      expect.objectContaining({ id: "missing", name: "missing", sourceKind: "virtual_machine", sourceId: "missing-source" }),
    ]);
  });

  it("serializes the selection with a stable sort order and ignores display-only fields in dirty checks", () => {
    const first = toSelectedHost(connectionTarget());
    const second = toSelectedHost(connectionTarget({
      id: uuid("44444444-4444-4444-4444-444444444444"),
      deviceId: uuid("55555555-5555-5555-5555-555555555555"),
    }));
    const renamed = { ...first, name: "renamed", subtitle: "changed" };

    expect(selectedHostsToStaticTargetSetMembers([first, second])).toEqual([
      { target_kind: "connection_target", target_id: first.id, source_kind: "device", source_id: first.sourceId, sort_order: 0 },
      { target_kind: "connection_target", target_id: second.id, source_kind: "device", source_id: second.sourceId, sort_order: 1 },
    ]);
    expect(selectedHostsSignature([first])).toBe(selectedHostsSignature([renamed]));
  });

});
