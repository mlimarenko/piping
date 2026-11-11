import type { ConnectionTarget } from "@/shared/api/types";
import type { TreeNode } from "./TreeNodeInspector";
import { AUTO_PREFERRED_ORDER, type TreeConfig, type TreeGroupKey } from "./treeConfig";

export interface GroupNode {
  key: string;
  label: string;
  groupKey: TreeGroupKey;
  hosts: ConnectionTarget[];
  children: GroupNode[];
}

function readField(host: ConnectionTarget, key: TreeGroupKey): string {
  const value = host[key];
  return typeof value === "string" && value ? value : "";
}

export function treeGroupOrder(hosts: ConnectionTarget[], config: TreeConfig): TreeGroupKey[] {
  return config.mode === "custom" ? config.order : AUTO_PREFERRED_ORDER.filter((key) => hosts.some((host) => readField(host, key)));
}

export function buildGroups(hosts: ConnectionTarget[], order: TreeGroupKey[], includeUnknown: boolean, pathPrefix = ""): GroupNode[] {
  const [head, ...rest] = order;
  if (head === undefined) return [];
  const bucket = new Map<string, ConnectionTarget[]>();
  for (const host of hosts) {
    const raw = readField(host, head);
    const label = raw || "—";
    if (!raw && !includeUnknown) continue;
    const list = bucket.get(label) ?? [];
    list.push(host);
    bucket.set(label, list);
  }
  return Array.from(bucket.keys()).sort((a, b) => a.localeCompare(b)).map((label) => {
    const groupHosts = bucket.get(label) ?? [];
    const key = `${pathPrefix}${head}=${label}|`;
    return { key, label, groupKey: head, hosts: groupHosts, children: buildGroups(groupHosts, rest, includeUnknown, key) };
  });
}

export function treeGroupHostId(host: ConnectionTarget, type: TreeGroupKey): string | undefined {
  return host[`${type}Id`];
}

export function resolveTreeGroup(hosts: ConnectionTarget[], config: TreeConfig, type: TreeGroupKey, path: string, anchor: string, entityId: string | null): TreeNode | null {
  const candidates: GroupNode[] = [];
  const visit = (groups: GroupNode[]) => {
    for (const group of groups) {
      if (group.groupKey === type && (entityId
        ? group.hosts.some((host) => treeGroupHostId(host, type) === entityId)
        : group.key === path && group.hosts.some((host) => host.id === anchor))) candidates.push(group);
      visit(group.children);
    }
  };
  visit(buildGroups(hosts, treeGroupOrder(hosts, config), config.includeUnknown));
  const anchored = candidates.filter((group) => group.hosts.some((host) => host.id === anchor && (!entityId || treeGroupHostId(host, type) === entityId)));
  const group = candidates.length === 1 ? candidates[0] : anchored.length === 1 ? anchored[0] : undefined;
  return group ? { type: group.groupKey, id: group.key, name: group.label, hosts: group.hosts } : null;
}
