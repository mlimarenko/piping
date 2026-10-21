import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import type { DataTableFilterOption, DataTableSortState } from "@/shared/ui-shared/DataTable";
import type { AccessFilterContext, AccessTableConfig } from "@/features/access-control/tableConfig";
import {
  sanitizeDataTableColumnFilters,
  sanitizeDataTableSortState,
  updateDataTableColumnFilters,
} from "@/shared/ui-shared/tableState";

/**
 * Shared sort + per-column filter controls for an access-control list table.
 *
 * Every access table holds the same state shape (controlled `sortState` +
 * `columnFilters`), maps the same column-key -> backend sort key, and forwards
 * the same comma-joined `IN` filter params; this hook owns that boilerplate so
 * each table component stays a thin renderer. Mirrors the inventory pattern
 * (`inventoryTableConfigFor` + the `columnMeta`/param derivation in
 * `InventoryResources`), factored because access has five tables instead of one.
 *
 * FK option lists are resolved by the caller (they come from `useAccessData()`,
 * which differs per table) and passed in as `fkOptions` keyed by column key.
 */
export interface AccessTableControls {
  sortState: DataTableSortState | null;
  onSortChange: (next: DataTableSortState | null) => void;
  columnFilters: Record<string, string[]>;
  onColumnFilterChange: (key: string, values: string[]) => void;
  /** Backend sort key for the active sort (column key mapped via overrides), or undefined. */
  sortParam: string | undefined;
  orderParam: "asc" | "desc" | undefined;
  /** Backend filter param -> selected values (column key mapped to its `param`). */
  filterParams: Record<string, string[]>;
  /** Sort/filter attributes for a column, keyed by its (possibly overridden) key. */
  columnMeta: (key: string) => { sortable: boolean; filterOptions?: DataTableFilterOption[] };
}

export function useAccessTableControls(
  config: AccessTableConfig,
  options: {
    /** Called whenever sort/filter changes so the table can reset to page 0. */
    onReset: () => void;
    /** colKey -> resolved FK options (only for `type: "fk"` columns). */
    fkOptions?: Record<string, DataTableFilterOption[]>;
    /** Dynamic context used by custom filter routers (for example grants.subject). */
    filterContext?: AccessFilterContext;
  },
): AccessTableControls {
  const { t } = useTranslation();
  const { filterContext, onReset, fkOptions } = options;
  const [sortState, setSortState] = useState<DataTableSortState | null>(null);
  const [columnFilters, setColumnFilters] = useState<Record<string, string[]>>({});

  const sortableKeySet = useMemo(() => new Set(config.sortKeys), [config.sortKeys]);
  const filterableKeys = useMemo(
    () => config.filterColumns.map((column) => column.colKey),
    [config.filterColumns],
  );
  const filterColumnByKey = useMemo(() => {
    const map = new Map(config.filterColumns.map((col) => [col.colKey, col]));
    return map;
  }, [config.filterColumns]);
  const sanitizedSortState = useMemo(
    () => sanitizeDataTableSortState(sortState, sortableKeySet),
    [sortState, sortableKeySet],
  );
  const sanitizedColumnFilters = useMemo(
    () => sanitizeDataTableColumnFilters(columnFilters, filterableKeys),
    [columnFilters, filterableKeys],
  );

  // colKey -> resolved `{value,label}[]`. Enum columns map their static
  // i18n-keyed values; FK columns take the caller-resolved option list.
  const filterOptionsByKey = useMemo(() => {
    const map = new Map<string, DataTableFilterOption[]>();
    for (const col of config.filterColumns) {
      if (col.type === "enum") {
        map.set(
          col.colKey,
          col.enumValues.map((v) => ({ value: v.value, label: t(v.label, { defaultValue: v.value }) })),
        );
      } else {
        map.set(col.colKey, fkOptions?.[col.colKey] ?? []);
      }
    }
    return map;
  }, [config.filterColumns, fkOptions, t]);

  const onSortChange = (next: DataTableSortState | null) => {
    setSortState(next);
    onReset();
  };

  const onColumnFilterChange = (key: string, values: string[]) => {
    setColumnFilters((prev) => updateDataTableColumnFilters(prev, key, values));
    onReset();
  };

  // Active filters keyed by backend param (column key -> `param`). Empty value
  // lists are dropped so an inactive filter sends no param.
  const filterParams = useMemo(() => {
    const params: Record<string, string[]> = {};
    for (const [colKey, values] of Object.entries(sanitizedColumnFilters)) {
      if (values.length === 0) continue;
      const column = filterColumnByKey.get(colKey);
      if (!column) continue;
      if (column.type === "fk" && column.buildParams) {
        Object.assign(params, column.buildParams(values, filterContext ?? {}));
        continue;
      }
      if (column.param) params[column.param] = values;
    }
    return params;
  }, [filterColumnByKey, filterContext, sanitizedColumnFilters]);

  const columnMeta = (key: string): { sortable: boolean; filterOptions?: DataTableFilterOption[] } => ({
    sortable: sortableKeySet.has(key),
    filterOptions: filterColumnByKey.has(key) ? filterOptionsByKey.get(key) ?? [] : undefined,
  });

  return {
    sortState: sanitizedSortState,
    onSortChange,
    columnFilters: sanitizedColumnFilters,
    onColumnFilterChange,
    sortParam: sanitizedSortState?.key,
    orderParam: sanitizedSortState?.dir,
    filterParams,
    columnMeta,
  };
}
