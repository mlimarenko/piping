import type { TFunction } from "i18next";
import type { ConnectionTarget } from "@/shared/api/types";
import type { DataTableFilterOption } from "@/shared/ui-shared/DataTable";
import { inventoryResourceStatusLabel } from "@/features/inventory/resourceStatus";

export type ConnectionTargetsTableView = "mobile" | "desktop";

export type ConnectionTargetsTableColumnKey =
  | "name"
  | "status"
  | "targetKind"
  | "region"
  | "site"
  | "tenant"
  | "role"
  | "platform"
  | "deviceType"
  | "primaryIp"
  | "actions";

type ConnectionTargetsFilterSource =
  | "status"
  | "targetKind"
  | "region"
  | "site"
  | "tenant"
  | "role"
  | "platform"
  | "deviceType";

interface ConnectionTargetsFilterContract {
  param: string;
  source: ConnectionTargetsFilterSource;
}

export interface ConnectionTargetsColumnContract {
  key: ConnectionTargetsTableColumnKey;
  sortable: boolean;
  sortParam?: string;
  filter?: ConnectionTargetsFilterContract;
  excludeFromSettings?: boolean;
}

interface ConnectionTargetsTableContract {
  tableIds: Record<ConnectionTargetsTableView, string>;
  viewColumns: Record<ConnectionTargetsTableView, ConnectionTargetsTableColumnKey[]>;
  columns: Record<ConnectionTargetsTableColumnKey, ConnectionTargetsColumnContract>;
}

const CONNECTION_TARGET_STATUS_VALUES = [
  "active",
  "planned",
  "staged",
  "decommissioning",
  "offline",
] as const;

const CONNECTION_TARGET_KIND_VALUES = [
  "device",
  "virtual_machine",
] as const;

export const CONNECTION_TARGETS_TABLE_CONFIG: ConnectionTargetsTableContract = {
  tableIds: {
    mobile: "connections.targets.mobile",
    desktop: "connections.targets.desktop",
  },
  viewColumns: {
    mobile: ["name", "status", "actions"],
    desktop: [
      "name",
      "status",
      "targetKind",
      "region",
      "site",
      "tenant",
      "role",
      "platform",
      "deviceType",
      "primaryIp",
      "actions",
    ],
  },
  columns: {
    name: { key: "name", sortable: true, sortParam: "name" },
    status: {
      key: "status",
      sortable: true,
      sortParam: "status",
      filter: { param: "status", source: "status" },
    },
    targetKind: {
      key: "targetKind",
      sortable: true,
      sortParam: "targetKind",
      filter: { param: "target_kind", source: "targetKind" },
    },
    region: {
      key: "region",
      sortable: true,
      sortParam: "region",
      filter: { param: "region_id", source: "region" },
    },
    site: {
      key: "site",
      sortable: true,
      sortParam: "site",
      filter: { param: "site_id", source: "site" },
    },
    tenant: {
      key: "tenant",
      sortable: true,
      sortParam: "tenant",
      filter: { param: "tenant_id", source: "tenant" },
    },
    role: {
      key: "role",
      sortable: true,
      sortParam: "role",
      filter: { param: "role_id", source: "role" },
    },
    platform: {
      key: "platform",
      sortable: true,
      sortParam: "platform",
      filter: { param: "platform_id", source: "platform" },
    },
    deviceType: {
      key: "deviceType",
      sortable: true,
      sortParam: "deviceType",
      filter: { param: "device_type_id", source: "deviceType" },
    },
    primaryIp: {
      key: "primaryIp",
      sortable: true,
      sortParam: "primaryIp",
    },
    actions: {
      key: "actions",
      sortable: false,
      excludeFromSettings: true,
    },
  },
};

export function connectionTargetsTableId(view: ConnectionTargetsTableView): string {
  return CONNECTION_TARGETS_TABLE_CONFIG.tableIds[view];
}

export function connectionTargetsViewColumnKeys(
  view: ConnectionTargetsTableView,
): ConnectionTargetsTableColumnKey[] {
  return CONNECTION_TARGETS_TABLE_CONFIG.viewColumns[view];
}

export function connectionTargetsSortParam(
  key: string,
): string | undefined {
  return CONNECTION_TARGETS_TABLE_CONFIG.columns[key as ConnectionTargetsTableColumnKey]?.sortParam;
}

export function connectionTargetsFilterParam(
  key: string,
): string | undefined {
  return CONNECTION_TARGETS_TABLE_CONFIG.columns[key as ConnectionTargetsTableColumnKey]?.filter?.param;
}

function uniqueIdFilterOptions<T>(
  rows: T[],
  idOf: (row: T) => string | null | undefined,
  labelOf: (row: T) => string | null | undefined,
): DataTableFilterOption[] {
  const counts = new Map<string, { label: string; count: number }>();
  for (const row of rows) {
    const id = idOf(row);
    const label = labelOf(row);
    if (!id || !label) continue;
    const current = counts.get(id);
    counts.set(id, { label, count: (current?.count ?? 0) + 1 });
  }
  return Array.from(counts.entries())
    .map(([value, option]) => ({ value, label: option.label, count: option.count }))
    .sort((left, right) => left.label.localeCompare(right.label));
}

export function buildConnectionTargetFilterOptions(
  hosts: ConnectionTarget[],
  t: TFunction,
): Partial<Record<ConnectionTargetsTableColumnKey, DataTableFilterOption[]>> {
  return {
    status: CONNECTION_TARGET_STATUS_VALUES.map((status) => ({
      value: status,
      label: inventoryResourceStatusLabel(t, status),
      count: hosts.filter((host) => host.status === status).length,
    })),
    targetKind: CONNECTION_TARGET_KIND_VALUES.map((kind) => ({
      value: kind,
      label:
        kind === "device"
          ? t("connections.targetKind.device", "Device")
          : t("connections.targetKind.virtualMachine", "Virtual machine"),
      count: hosts.filter((host) => host.targetKind === kind).length,
    })),
    region: uniqueIdFilterOptions(hosts, (host) => host.regionId, (host) => host.region),
    site: uniqueIdFilterOptions(hosts, (host) => host.siteId, (host) => host.site),
    tenant: uniqueIdFilterOptions(hosts, (host) => host.tenantId, (host) => host.tenant),
    role: uniqueIdFilterOptions(hosts, (host) => host.roleId, (host) => host.role),
    platform: uniqueIdFilterOptions(hosts, (host) => host.platformId, (host) => host.platform),
    deviceType: uniqueIdFilterOptions(
      hosts,
      (host) => host.deviceTypeId,
      (host) => host.deviceType,
    ),
  };
}
