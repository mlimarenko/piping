import { fieldsByTab } from "@/features/inventory/components/inventory/inventoryTypes";
import type { InventoryEntityKey } from "@/shared/api/types";

export function relatedEntitiesForEntity(
  entity: InventoryEntityKey,
  visibleKeys?: Set<string>,
): InventoryEntityKey[] {
  const fields = (fieldsByTab[entity] ?? []).filter((field) => {
    if (!visibleKeys) return true;
    return visibleKeys.has(field.idKey ?? field.key);
  });

  return Array.from(
    new Set<InventoryEntityKey>([
      ...fields
        .filter((field) => (field.type === "related" || field.type === "related-multiple") && field.relatedTab)
        .map((field) => field.relatedTab as InventoryEntityKey),
      ...fields
        .filter((field) => field.type === "objectRef")
        .flatMap((field) => (field.objectTypes ?? []).map((objectType) => objectType.relatedTab)),
    ]),
  );
}
