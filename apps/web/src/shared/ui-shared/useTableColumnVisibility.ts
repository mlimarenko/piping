import { useCallback, useMemo } from "react";
import { useSetting } from "@/shared/settings/hooks";
import {
  defaultHiddenColumnsFor,
  normalizeTableColumnVisibility,
  type TableColumnVisibilityValue,
} from "@/shared/settings/registry";

const SETTING_KEY = "table.column_visibility";
const DEBOUNCE_MS = 500;

export interface UseTableColumnVisibilityResult {
  hiddenColumns: Set<string>;
  setHidden: (next: Set<string>) => void;
  reset: () => void;
  source: ReturnType<typeof useSetting<TableColumnVisibilityValue>>["source"];
}

/**
 * Per-user, per-table column visibility backed by the shared settings store.
 *
 * The setting stores HIDDEN column keys per table (`{ [tableId]: { hidden } }`),
 * so columns added to a table later default to visible. When a table has NO
 * stored entry, the per-table default-hidden policy
 * (`defaultHiddenColumnsFor`) applies, so a wide table can ship a sensible
 * visible subset while keeping every column toggleable. A stored entry — even
 * `{ hidden: [] }` (the user explicitly revealed everything) — fully overrides
 * the default; `reset()` deletes the entry so the default re-applies.
 *
 * When `tableId` is empty the hook is inert: the underlying query is disabled,
 * `hiddenColumns` is empty, and the mutators are no-ops. This lets `DataTable`
 * call the hook unconditionally (Rules of Hooks) while only wiring real behavior
 * for tables that opt in with a `tableId`.
 */
export function useTableColumnVisibility(tableId: string): UseTableColumnVisibilityResult {
  const enabled = tableId.length > 0;
  const setting = useSetting<TableColumnVisibilityValue>(SETTING_KEY, {}, {
    debounceMs: DEBOUNCE_MS,
    enabled,
    normalize: normalizeTableColumnVisibility,
  });

  const { value, setValue, resetValue } = setting;

  const hiddenColumns = useMemo(() => {
    if (!enabled) return new Set<string>();
    // A stored entry (including `{ hidden: [] }`) is an explicit user override
    // and wins outright; only when the table has no entry does the per-table
    // default-hidden policy apply.
    if (tableId in value) return new Set(value[tableId]?.hidden ?? []);
    return new Set(defaultHiddenColumnsFor(tableId));
  }, [enabled, tableId, value]);

  const setHidden = useCallback(
    (next: Set<string>) => {
      if (!enabled) return;
      setValue((prev) => ({ ...prev, [tableId]: { hidden: Array.from(next) } }));
    },
    [enabled, setValue, tableId],
  );

  const reset = useCallback(() => {
    if (!enabled) return;
    if (!(tableId in value)) return;
    const next = { ...value };
    delete next[tableId];
    if (Object.keys(next).length === 0) {
      // No table-specific overrides remain: drop the whole user override so the
      // instance/global default re-applies instead of persisting `{}`.
      resetValue();
    } else {
      setValue(next);
    }
  }, [enabled, resetValue, setValue, tableId, value]);

  return { hiddenColumns, setHidden, reset, source: setting.source };
}
