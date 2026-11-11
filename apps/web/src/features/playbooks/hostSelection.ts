import type { ConnectionTarget } from "@/shared/api/types";

export type PlaybookConnectionTargetSourceKind = ConnectionTarget["targetKind"];

/** A connection target picked for an ad-hoc / static-target-set playbook run. */
export interface SelectedHost {
  id: string;
  name: string;
  subtitle: string;
  /** `device` | `virtual_machine` — doubles as the static member source_kind. */
  sourceKind: PlaybookConnectionTargetSourceKind;
  /** UUID of the backing device / VM source row, if known. */
  sourceId: string | null;
}

export interface PlaybookLaunchTargetProjection {
  id: string;
  name: string;
  subtitle?: string | null;
  sourceKind: PlaybookConnectionTargetSourceKind;
  sourceId: string | null;
}

export interface PlaybookStaticTargetSetMember {
  target_kind: "connection_target";
  target_id: string;
  source_kind: PlaybookConnectionTargetSourceKind;
  source_id: string | null;
  sort_order: number;
}

/** Member shape returned by the target-set detail endpoint. */
export interface PlaybookTargetSetMember {
  target_kind: string;
  target_id: string;
  source_kind: string;
  source_id: string;
  sort_order: number;
}

function connectionTargetSourceId(target: ConnectionTarget): string | null {
  return (target.targetKind === "device" ? target.deviceId : target.vmId) ?? null;
}

function defaultConnectionTargetSubtitle(target: ConnectionTarget): string {
  return [target.role, target.site, target.primaryIp].filter(Boolean).join(" · ");
}

/** Project a hydrated connection target into a {@link SelectedHost}. */
export function toSelectedHost(target: ConnectionTarget, subtitle?: string): SelectedHost {
  return {
    id: target.id,
    name: target.name,
    subtitle: subtitle ?? defaultConnectionTargetSubtitle(target),
    sourceKind: target.targetKind,
    sourceId: connectionTargetSourceId(target),
  };
}

/**
 * Project a connection target into launch-dialog metadata. Connection workspace
 * bulk and quick actions both use this so the confirmation UI and the later
 * static target-set materialization describe the same backing source.
 */
export function toPlaybookLaunchTarget(
  target: ConnectionTarget,
  subtitle?: string,
): PlaybookLaunchTargetProjection {
  return {
    id: target.id,
    name: target.name,
    subtitle: subtitle ?? defaultConnectionTargetSubtitle(target),
    sourceKind: target.targetKind,
    sourceId: connectionTargetSourceId(target),
  };
}

/** Static target-set member for connection-originated ad-hoc playbook runs. */
export function toPlaybookStaticTargetSetMember(
  target: ConnectionTarget,
  sortOrder: number,
): PlaybookStaticTargetSetMember {
  return {
    target_kind: "connection_target",
    target_id: target.id,
    source_kind: target.targetKind,
    source_id: connectionTargetSourceId(target),
    sort_order: sortOrder,
  };
}

/** Build the editor selection from persisted target-set members. */
export function staticTargetSetMembersToHosts(
  members: readonly PlaybookTargetSetMember[] | null | undefined,
  targets: readonly ConnectionTarget[],
): SelectedHost[] {
  const byId = new Map<string, ConnectionTarget>(
    targets.map((target) => [target.id, target]),
  );
  return [...(members ?? [])]
    .sort((left, right) => left.sort_order - right.sort_order)
    .map((member) => {
      const target = byId.get(member.target_id);
      if (target) return toSelectedHost(target);
      return {
        id: member.target_id,
        name: member.target_id,
        subtitle: "",
        sourceKind: member.source_kind as PlaybookConnectionTargetSourceKind,
        sourceId: member.source_id,
      };
    });
}

/** Serialize the editor selection back into the atomic PUT member list. */
export function selectedHostsToStaticTargetSetMembers(
  hosts: readonly SelectedHost[],
): PlaybookStaticTargetSetMember[] {
  return hosts.map((host, sortOrder) => ({
    target_kind: "connection_target",
    target_id: host.id,
    source_kind: host.sourceKind,
    source_id: host.sourceId,
    sort_order: sortOrder,
  }));
}

/** Stable dirty-check projection: names and subtitles never affect persistence. */
export function selectedHostsSignature(hosts: readonly SelectedHost[]): string {
  return JSON.stringify(hosts.map((host) => [host.id, host.sourceKind, host.sourceId]));
}
