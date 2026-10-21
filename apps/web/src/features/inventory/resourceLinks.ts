import type { InventoryEntityKey } from "@/shared/api/types";
import {
  CATEGORIZED_ENTITIES,
  CABLING_WORKSTATION_RAIL_KEY,
  CONFIG_CONTEXTS_RAIL_KEY,
  IPAM_WORKSPACE_RAIL_KEY,
  type RailEntity,
} from "./components/inventory/inventoryCategories";

/**
 * Sole builder/parser for the inventory resource editor routes:
 *
 *   /inventory/resources                       (list; `?entity=<key>` selects)
 *   /inventory/resources/<Entity>/new          (create)
 *   /inventory/resources/<Entity>/<uuid>/edit  (edit)
 *
 * `entity` is always validated against {@link CATEGORIZED_ENTITIES}: entities
 * that exist in the API but are not surfaced as navigable inventory tabs yield
 * `null` and callers render a plain value. There is no legacy query deep-link:
 * the path routes are the only editor entry points.
 */

export const INVENTORY_RESOURCES_PATH = "/inventory/resources";

/** Persisted read-only detail is separate from the authoring/inspector links. */
export function inventoryResourceDetailHref(entity: string | null | undefined, resourceId: string | null | undefined, returnTo?: string): string | null {
  if (!resourceId || !isInventoryEntityKey(entity)) return null;
  const base = `${INVENTORY_RESOURCES_PATH}/${encodeURIComponent(entity)}/${encodeURIComponent(resourceId)}`;
  return returnTo ? `${base}?return=${encodeURIComponent(returnTo)}` : base;
}

export const INVENTORY_PARAM = {
  entity: "entity",
} as const;

const CATEGORIZED_SET: ReadonlySet<string> = new Set<string>(CATEGORIZED_ENTITIES);

/** Narrow an arbitrary string to a navigable inventory entity key. */
export function isInventoryEntityKey(
  value: string | null | undefined,
): value is InventoryEntityKey {
  return value != null && CATEGORIZED_SET.has(value);
}

/**
 * Narrow an arbitrary string to a navigable rail entry — a real inventory
 * entity OR a pseudo-entity workspace (config layers / IPAM / cabling). The
 * pseudo-entities own no rows and have no editor route, so they are only ever
 * selected on the list.
 */
export function isRailEntity(
  value: string | null | undefined,
): value is RailEntity {
  return (
    isInventoryEntityKey(value) ||
    value === CONFIG_CONTEXTS_RAIL_KEY ||
    value === IPAM_WORKSPACE_RAIL_KEY ||
    value === CABLING_WORKSTATION_RAIL_KEY
  );
}

/**
 * Canonical href to an inventory resource's edit form. Returns `null` when the
 * id is missing or the entity is not a navigable (categorized) entity, so a
 * dangling FK or a non-surfaced relatedTab degrades to a non-link.
 */
export function inventoryResourceHref(
  entity: string | undefined | null,
  resourceId: string | undefined | null,
): string | null {
  if (!resourceId || !isInventoryEntityKey(entity)) return null;
  // Callers that need a specific return target append `?return=`; otherwise the
  // editor returns to the list.
  return inventoryResourceEditPath(entity, resourceId);
}

/** Canonical full-page route for creating a navigable inventory resource. */
export function inventoryResourceNewPath(
  entity: InventoryEntityKey,
  returnTo?: string,
): string {
  const base = `${INVENTORY_RESOURCES_PATH}/${encodeURIComponent(entity)}/new`;
  return returnTo ? `${base}?return=${encodeURIComponent(returnTo)}` : base;
}

/** Canonical full-page route for editing a navigable inventory resource. */
export function inventoryResourceEditPath(
  entity: InventoryEntityKey,
  resourceId: string,
  returnTo?: string,
): string {
  const base = `${INVENTORY_RESOURCES_PATH}/${encodeURIComponent(entity)}/${encodeURIComponent(resourceId)}/edit`;
  return returnTo ? `${base}?return=${encodeURIComponent(returnTo)}` : base;
}

/** Parse the active rail entry (`?entity=<key>`) off the list URL. */
export function parseInventoryRailEntity(
  search: URLSearchParams,
): RailEntity | undefined {
  const raw = search.get(INVENTORY_PARAM.entity);
  return isRailEntity(raw) ? raw : undefined;
}

/**
 * New params that select a rail entry — used on manual tab/category switches
 * and the list's "open entity" shortcuts. Accepts the pseudo-entity workspaces.
 */
export function inventoryEntityParams(entity: RailEntity): URLSearchParams {
  const next = new URLSearchParams();
  next.set(INVENTORY_PARAM.entity, entity);
  return next;
}
