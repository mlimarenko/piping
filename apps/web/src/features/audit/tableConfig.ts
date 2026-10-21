import { buildDataTableFilterParams, type DataTableFilterParamConfig } from "@/shared/ui-shared/tableState";

/**
 * Sort/filter metadata for the audit-log {@link DataTable}.
 *
 * The shared table owns the sort/filter UX (header-click cycle + the
 * `ColumnFilterMenu` funnel popover). This module is the single consumer
 * contract: it maps each rendered column `key` to the backend sort key and
 * declares which columns carry a multi-value checkbox filter and the exact
 * query dimension those values feed.
 *
 * Single source of truth: the backend `list_audit_events` allowlists the sort
 * keys `created_at, event_type, actor, target_type, result, summary` and
 * accepts comma-separated `IN` filters on `event_type`, `target_type`, `result`
 * (see `apps/api/.../audit` + `schema.gen.ts`). The audit columns render under
 * UI-friendly keys (`timestamp`, `action`, `target`), so each is mapped to its
 * backend sort key here; the three filterable columns route to the
 * backend query parameter that column drives.
 */

export type AuditFilterDimension = "eventTypes" | "targetTypes" | "results";
type AuditBackendFilterParam = "event_type" | "target_type" | "result";

interface AuditTableConfig {
  sortKeys: string[];
  sortKeyByColumn: Record<string, string>;
  filterColumns: DataTableFilterParamConfig[];
  filterDimensionByColumn: Record<string, AuditFilterDimension>;
}

export const AUDIT_TABLE_CONFIG: AuditTableConfig = {
  sortKeys: ["timestamp", "actor", "action", "target", "result", "summary"],
  sortKeyByColumn: {
    timestamp: "created_at",
    actor: "actor",
    action: "event_type",
    target: "target_type",
    result: "result",
    summary: "summary",
  },
  filterColumns: [
    { colKey: "action", param: "event_type" },
    { colKey: "target", param: "target_type" },
    { colKey: "result", param: "result" },
  ],
  filterDimensionByColumn: {
    action: "eventTypes",
    target: "targetTypes",
    result: "results",
  },
};

export const AUDIT_FILTER_DIMENSION_BY_COLUMN = AUDIT_TABLE_CONFIG.filterDimensionByColumn;

/** Column keys that get a sort affordance (every other column opts out). */
export const AUDIT_SORTABLE_COLUMN_KEYS = new Set(AUDIT_TABLE_CONFIG.sortKeys);

export function auditSortParamForColumn(columnKey: string | null | undefined): string | undefined {
  if (!columnKey) return undefined;
  return AUDIT_TABLE_CONFIG.sortKeyByColumn[columnKey];
}

export function buildAuditFilterParams(
  columnFilters: Record<string, string[]>,
): Partial<Record<AuditFilterDimension, string[]>> {
  const backendParams = buildDataTableFilterParams(
    columnFilters,
    AUDIT_TABLE_CONFIG.filterColumns,
  ) as Partial<Record<AuditBackendFilterParam, string[]>>;

  return {
    ...(backendParams.event_type ? { eventTypes: backendParams.event_type } : {}),
    ...(backendParams.target_type ? { targetTypes: backendParams.target_type } : {}),
    ...(backendParams.result ? { results: backendParams.result } : {}),
  };
}
