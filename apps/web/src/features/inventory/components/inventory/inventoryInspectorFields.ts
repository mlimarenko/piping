import type { InventoryEntityKey, Resource } from "@/shared/api/types";
import { inventoryEntityHasStatus } from "@/shared/api/types";
import { fieldsByTab, type FieldConfig } from "./inventoryTypes";

/**
 * Inventory-owned, presentation-agnostic inspector metadata. Derives the read-
 * only "Details" rows for an entity directly from the single-source-of-truth
 * form metadata (`fieldsByTab`) plus `inventoryEntityHasStatus`. The connections
 * `TreeNodeInspector` consumes this instead of owning its own per-entity
 * field/related-link matrix, so the inspector and the inventory CRUD form can
 * never drift on which fields exist or which ones are FK links.
 *
 * Field-kind mapping from {@link FieldConfig.type}:
 *  - `related`           → linkable row (carries `relatedTab` + `idKey`).
 *  - `textarea`          → expandable long-text row.
 *  - `text|select|number|color` → short scalar row (never a link).
 *  - `icon|objectRef`    → not surfaced in the read-only inspector.
 */

export type InspectorFieldKind = "scalar" | "related" | "textarea";

export interface InspectorFieldMeta {
  /** `Resource` value key read for display (the human-readable column). */
  key: keyof Resource;
  /** i18n key (or literal label) shared with the CRUD form. */
  label: string;
  kind: InspectorFieldKind;
  /** For `related` rows: the entity its edit deep-link points at. */
  relatedTab?: InventoryEntityKey;
  /** For `related` rows: the FK id key on `Resource` carrying the link target. */
  idKey?: keyof Resource;
}

function kindOf(field: FieldConfig): InspectorFieldKind | null {
  switch (field.type) {
    case "related":
      return "related";
    case "textarea":
      return "textarea";
    case "text":
    case "select":
    case "number":
    case "color":
      return "scalar";
    // `icon`/`objectRef` have no single scalar inspector cell; `nested-children`
    // is a host-rendered inline editor, not a read-only inspector row.
    case "icon":
    case "identity-multiple":
    case "related-multiple":
    case "objectRef":
    case "nested-children":
      return null;
  }
}

/**
 * Read-only inspector field metadata for an inventory entity, in form order.
 * Returns an empty array for entities with no form fields.
 */
export function inventoryInspectorFields(entity: InventoryEntityKey): InspectorFieldMeta[] {
  const fields = fieldsByTab[entity] ?? [];
  const metas: InspectorFieldMeta[] = [];
  for (const field of fields) {
    const kind = kindOf(field);
    if (!kind) continue;
    metas.push({
      key: field.key,
      label: field.label,
      kind,
      relatedTab: kind === "related" ? field.relatedTab : undefined,
      idKey: kind === "related" ? field.idKey : undefined,
    });
  }
  return metas;
}

/** Whether the entity carries a status field worth a dedicated inspector row. */
export function inventoryInspectorHasStatus(entity: InventoryEntityKey): boolean {
  return inventoryEntityHasStatus(entity);
}
