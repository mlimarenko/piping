import { describe, expect, it } from "vitest";
import { fieldsByTab } from "@/features/inventory/components/inventory/inventoryTypes";
import {
  inventoryColumnKeyForField,
  inventoryFieldRendersAsDataColumn,
  inventoryIdentityColumnLabel,
  inventoryNameColumnKey,
  inventoryTableConfigFor,
} from "@/features/inventory/tableConfig";
import {
  TABLE_DEFAULT_HIDDEN_COLUMNS,
  defaultHiddenColumnsFor,
} from "@/shared/settings/registry";
import { inventoryEntityHasStatus, type InventoryEntityKey } from "@/shared/api/types";

/**
 * Reconstruct the data-column keys the inventory table renders for an entity,
 * mirroring `InventoryResources` column construction: every non-textarea /
 * non-icon field that does not already render as the primary identity column
 * becomes a data column, and `columnKeyOverrides` is applied so the key matches
 * the backend sort/filter contract.
 */
function reconstructDataColumnKeys(entity: InventoryEntityKey): string[] {
  const fields = fieldsByTab[entity] ?? [];
  return fields
    .filter((f) => inventoryFieldRendersAsDataColumn(entity, f))
    .map((f) => inventoryColumnKeyForField(entity, f));
}

function reconstructVisibleTableColumnKeys(entity: InventoryEntityKey): string[] {
  const keys = [inventoryNameColumnKey(entity)];
  if (inventoryEntityHasStatus(entity)) keys.push("status");
  if (entity === "Prefixes") keys.push("utilization");
  keys.push(...reconstructDataColumnKeys(entity));
  return keys;
}

function reconstructVisibleTableColumnLabels(entity: InventoryEntityKey): string[] {
  const fields = fieldsByTab[entity] ?? [];
  const labels = [inventoryIdentityColumnLabel(entity)];
  if (inventoryEntityHasStatus(entity)) labels.push("status");
  if (entity === "Prefixes") labels.push("inventory.resources.fields.utilization");
  labels.push(
    ...fields
      .filter((f) => inventoryFieldRendersAsDataColumn(entity, f))
      .map((f) => f.label),
  );
  return labels;
}

function duplicateKeys(keys: string[]): string[] {
  const seen = new Set<string>();
  const duplicates = new Set<string>();
  for (const key of keys) {
    if (seen.has(key)) duplicates.add(key);
    else seen.add(key);
  }
  return Array.from(duplicates);
}

const INVENTORY_TABLE_IDS_WITH_DEFAULT_HIDDEN_COLUMNS = Object.keys(
  TABLE_DEFAULT_HIDDEN_COLUMNS,
).filter((tableId) => tableId.startsWith("inventory."));

describe("inventory default-hidden column policy", () => {
  // The map keys are `inventory.<entity>`; assert every default-hidden key is a
  // real rendered column key for that entity. A drifted key (e.g. forgetting a
  // `columnKeyOverrides` rename) would silently show the column instead of
  // hiding it — this pins the contract so it fails CI instead.
  it.each(INVENTORY_TABLE_IDS_WITH_DEFAULT_HIDDEN_COLUMNS)(
    "%s hides only keys that are real rendered columns",
    (tableId) => {
      const entity = tableId.replace(/^inventory\./, "") as InventoryEntityKey;
      const columnKeys = new Set(reconstructDataColumnKeys(entity));
      for (const hiddenKey of defaultHiddenColumnsFor(tableId)) {
        expect(columnKeys.has(hiddenKey), `${tableId}: "${hiddenKey}"`).toBe(true);
      }
    },
  );

  it("keeps a sensible visible window (never hides every data column)", () => {
    for (const tableId of INVENTORY_TABLE_IDS_WITH_DEFAULT_HIDDEN_COLUMNS) {
      const entity = tableId.replace(/^inventory\./, "") as InventoryEntityKey;
      const columnKeys = reconstructDataColumnKeys(entity);
      const hidden = new Set(defaultHiddenColumnsFor(tableId));
      const visibleData = columnKeys.filter((k) => !hidden.has(k));
      expect(visibleData.length, tableId).toBeGreaterThan(0);
    }
  });

  it("returns an empty default for tables without a policy", () => {
    expect(defaultHiddenColumnsFor("inventory.Clusters")).toEqual([]);
    expect(defaultHiddenColumnsFor("some.other.table")).toEqual([]);
  });

  // The full-attribute-set change dropped the `slice(0, N)` cap for EVERY
  // entity, not just the sort/filter-configured ones. Without a matching
  // default-hidden entry a wide entity (Rack Types, Circuits, …) would render
  // its full field set by default. Assert every entity's default-VISIBLE data
  // column count stays within the historical window so no table is absurdly
  // wide on first open; this is the check that catches a missing map entry.
  const DEFAULT_VISIBLE_WINDOW = (entity: string) => (entity === "Prefixes" ? 3 : 4);

  it.each(Object.keys(fieldsByTab))(
    "%s keeps its default-visible data columns within the window",
    (entity) => {
      const columnKeys = reconstructDataColumnKeys(entity as InventoryEntityKey);
      const hidden = new Set(defaultHiddenColumnsFor(`inventory.${entity}`));
      const filterable = new Set(
        inventoryTableConfigFor(entity as InventoryEntityKey).filterColumns.map(
          (column) => column.colKey,
        ),
      );
      const visibleData = columnKeys.filter((k) => !hidden.has(k) && !filterable.has(k));
      expect(visibleData.length, entity).toBeLessThanOrEqual(DEFAULT_VISIBLE_WINDOW(entity));
    },
  );

  it.each(Object.keys(fieldsByTab))(
    "%s never hides filterable data columns by default",
    (entity) => {
      const key = entity as InventoryEntityKey;
      const hidden = new Set(defaultHiddenColumnsFor(`inventory.${entity}`));
      for (const filterColumn of inventoryTableConfigFor(key).filterColumns) {
        expect(hidden.has(filterColumn.colKey), `${entity}: ${filterColumn.colKey}`).toBe(false);
      }
    },
  );

  it.each(Object.keys(fieldsByTab))(
    "%s renders each visible table column key once",
    (entity) => {
      const columnKeys = reconstructVisibleTableColumnKeys(entity as InventoryEntityKey);

      expect(duplicateKeys(columnKeys), entity).toEqual([]);
    },
  );

  it.each(Object.keys(fieldsByTab))(
    "%s renders each settings-managed column label once",
    (entity) => {
      const columnLabels = reconstructVisibleTableColumnLabels(entity as InventoryEntityKey);

      expect(duplicateKeys(columnLabels), entity).toEqual([]);
    },
  );

  it.each(Object.keys(fieldsByTab))(
    "%s makes every rendered data column sortable when table support exists",
    (entity) => {
      const key = entity as InventoryEntityKey;
      const config = inventoryTableConfigFor(key);
      if (config.sortKeys.length === 0) return;

      const sortable = new Set(config.sortKeys);
      expect(sortable.has(inventoryNameColumnKey(key)), `${entity}: name`).toBe(true);
      for (const columnKey of reconstructDataColumnKeys(key)) {
        if (config.unsortableKeys?.includes(columnKey)) continue;
        expect(sortable.has(columnKey), `${entity}: ${columnKey}`).toBe(true);
      }
    },
  );

  it.each(Object.keys(fieldsByTab))(
    "%s has explicit sort coverage for every visible table column, or a documented gap",
    (entity) => {
      const key = entity as InventoryEntityKey;
      const config = inventoryTableConfigFor(key);
      if (config.sortKeys.length === 0) return;

      const sortable = new Set(config.sortKeys);
      const documentedSortGaps = new Set((config.unsortableKeys ?? []).map((column) => `${entity}:${column}`));

      for (const columnKey of reconstructVisibleTableColumnKeys(key)) {
        const gapKey = `${entity}:${columnKey}`;
        if (documentedSortGaps.has(gapKey)) continue;
        expect(sortable.has(columnKey), gapKey).toBe(true);
      }
    },
  );

  it.each(Object.keys(fieldsByTab))(
    "%s exposes filters on every visible FK-like table column",
    (entity) => {
      const key = entity as InventoryEntityKey;
      const config = inventoryTableConfigFor(key);
      if (config.sortKeys.length === 0) return;

      const filterable = new Set(config.filterColumns.map((column) => column.colKey));
      for (const field of fieldsByTab[key] ?? []) {
        if (field.type !== "related" && field.type !== "select" && field.type !== "objectRef") {
          continue;
        }
        if (!inventoryFieldRendersAsDataColumn(key, field)) continue;

        const columnKey = inventoryColumnKeyForField(key, field);
        expect(filterable.has(columnKey), `${entity}: ${columnKey}`).toBe(true);
      }
    },
  );
});
