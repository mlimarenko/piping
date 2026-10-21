import type { DataTableSortState } from "./DataTable";

export type DataTableFilterParamValue = string | string[];
export type DataTableSortKeyMap = Record<string, string>;

export interface DataTableFilterParamConfig {
  colKey: string;
  param: string;
  serialize?: (values: string[]) => DataTableFilterParamValue | undefined;
}

/**
 * Declarative query mapping for one server-driven list: the sort keys the
 * backend accepts and the column→param filters it understands. Owned by the
 * feature (the backend contract lives there); shared only as a shape so
 * `useServerTable` and the feature config files share one vocabulary.
 */
export interface DataTableQueryConfig {
  sortKeys: readonly string[];
  filterColumns: readonly DataTableFilterParamConfig[];
}

export function sanitizeDataTableSortState(
  sortState: DataTableSortState | null,
  allowedKeys: Iterable<string>,
): DataTableSortState | null {
  if (!sortState) return null;
  const allowed = new Set(allowedKeys);
  return allowed.has(sortState.key) ? sortState : null;
}

export function mapDataTableSortKey(
  sortState: DataTableSortState | null,
  sortKeyByColumn?: DataTableSortKeyMap,
): string | undefined {
  if (!sortState) return undefined;
  return sortKeyByColumn?.[sortState.key] ?? sortState.key;
}

export function sanitizeDataTableColumnFilters(
  columnFilters: Record<string, string[]>,
  allowedKeys: Iterable<string>,
): Record<string, string[]> {
  const allowed = new Set(allowedKeys);
  const sanitized: Record<string, string[]> = {};
  for (const [key, values] of Object.entries(columnFilters)) {
    if (!allowed.has(key)) continue;
    const normalized = Array.from(
      new Set(values.filter((value) => typeof value === "string" && value.length > 0)),
    );
    if (normalized.length > 0) sanitized[key] = normalized;
  }
  return sanitized;
}

export function updateDataTableColumnFilters(
  current: Record<string, string[]>,
  key: string,
  values: string[],
): Record<string, string[]> {
  if (values.length === 0) {
    if (!(key in current)) return current;
    const next = { ...current };
    delete next[key];
    return next;
  }
  return { ...current, [key]: values };
}

export function serializeDataTableFilterValuesCsv(values: string[]): string | undefined {
  return values.length > 0 ? values.join(",") : undefined;
}

/**
 * Server-list filter params as CSV strings only. The 4 inventory list tables
 * (`templates`, `template-runs`, `custom-field definitions`, `choice-sets`) all
 * serialize their filters to comma-joined strings, so the array-valued entries
 * `buildDataTableFilterParams` can emit are dropped here rather than repeated
 * per feature. Used by `useServerTable`.
 */
export function buildFilterParams(
  columnFilters: Record<string, string[]>,
  filterColumns: Iterable<DataTableFilterParamConfig>,
): Record<string, string> {
  return Object.fromEntries(
    Object.entries(buildDataTableFilterParams(columnFilters, filterColumns)).filter(
      (entry): entry is [string, string] => typeof entry[1] === "string",
    ),
  );
}

export function buildDataTableFilterParams(
  columnFilters: Record<string, string[]>,
  filterColumns: Iterable<DataTableFilterParamConfig>,
): Record<string, DataTableFilterParamValue> {
  const filterColumnByKey = new Map(
    Array.from(filterColumns, (column) => [column.colKey, column] as const),
  );
  const params: Record<string, DataTableFilterParamValue> = {};

  for (const [colKey, values] of Object.entries(columnFilters)) {
    if (values.length === 0) continue;
    const column = filterColumnByKey.get(colKey);
    if (!column) continue;

    const serialized = column.serialize ? column.serialize(values) : values;
    if (
      serialized === undefined ||
      (Array.isArray(serialized) && serialized.length === 0) ||
      (typeof serialized === "string" && serialized.length === 0)
    ) {
      continue;
    }

    params[column.param] = serialized;
  }

  return params;
}
