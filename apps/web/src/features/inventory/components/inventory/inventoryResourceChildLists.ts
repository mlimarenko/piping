import type { InventoryEntityKey, Paginated, Resource } from "@/shared/api/types";
import type { paths } from "@/shared/api/schema.gen";
import type { RelationSpec } from "@/shared/nested-editor/descriptorFromSpec";
import type { ServerTableListParams } from "@/shared/ui-shared/useServerTable";
import { listChildResourcesByRouteQuery } from "@/features/inventory/api";
import { relationshipsByTab } from "./inventoryTypes";

/** A nested relation's child becomes a top-level read collection when visiting
 * that intermediate entity. The writable tree remains owned by NestedEditorPanel. */
export function inventoryResourceChildRelations(entity: InventoryEntityKey): RelationSpec[] {
  const relations = new Map<string, RelationSpec>();
  for (const spec of relationshipsByTab[entity] ?? []) relations.set(spec.key, spec);
  const visit = (spec: RelationSpec) => {
    if (spec.childEntity === entity && spec.child) relations.set(spec.child.key, spec.child);
    if (spec.child) visit(spec.child);
  };
  for (const specs of Object.values(relationshipsByTab)) specs?.forEach(visit);
  return [...relations.values()];
}

/** Column preferences belong to the relation projection, not a parent record
 * UUID or the wider standalone inventory table for the same child entity. */
export function inventoryResourceChildTableId(entity: InventoryEntityKey, relation: RelationSpec): string {
  return `inventory.children.${entity}.${relation.key}`;
}

export type ChildListParams = ServerTableListParams & Record<string, string | number | boolean | undefined>;

/** Preserve the server envelope and pin scope after composed table parameters. */
export async function listInventoryResourceChildren(spec: RelationSpec, parentId: string, params: ChildListParams, assignedTypeId?: string): Promise<Paginated<Resource>> {
  if (!parentId || (spec.listFilterKey === "assigned_object_id" && !assignedTypeId)) throw new Error("Missing inventory relation scope");
  const query = { ...params, limit: params.limit ?? 50, offset: params.offset ?? 0,
    [spec.listFilterKey]: parentId,
    ...(spec.listFilterKey === "assigned_object_id" ? { assigned_object_type_id: assignedTypeId } : {}) };
  const page = await listChildResourcesByRouteQuery(spec.childEntity, spec.listRoute as keyof paths, query);
  return { items: page.rows, total: page.total, limit: page.limit, offset: page.offset };
}
