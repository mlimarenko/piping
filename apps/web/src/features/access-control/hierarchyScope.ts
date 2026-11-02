import { Boxes, Cpu, Globe, Layers, MapPin, Tag, type LucideIcon } from "lucide-react";
import type { InventoryEntityKey } from "@/shared/api/types";

/**
 * Inventory-hierarchy scope registry — the ONE frontend place that knows which
 * policy attribute keys are "hierarchy levels" and where their entities come
 * from. The policy editor's Область tabs, its chips, the id→name resolution
 * for constraint values and the attribute-conditions key split all derive from
 * these two tables.
 *
 * # Extension guide — adding a new hierarchy level (e.g. `rack_id`)
 *
 * The backend side comes first — see the extension guide in
 * `apps/api/src/contexts/access_control/domain/vocabulary.rs` (the key must be
 * declared in `constraint_attribute_keys` and wired through the per-target
 * checks and row visibility there,
 * else the tab stays disabled and a saved policy would never match). Then:
 *
 * 1. Map the key to its inventory entity in {@link ID_KEY_ENTITY} — this is
 *    what feeds the checkbox list (exhaustive `listAllResources` load) and
 *    resolves stored UUIDs back to display names.
 * 2. Add a tab entry to {@link HIERARCHY_SCOPE_DEFS} (order = hierarchy
 *    top-down; pick a distinct lucide icon).
 * 3. Add `accessControl.policies.constraintKeyLabels.<key>` to BOTH locales
 *    (`shared/i18n/locales/{ru,en}.ts`) — the tab label, the chip sublabel and
 *    the preview all read this one dictionary.
 *
 * Skipping any step turns `hierarchyScope.spec.ts` red. Nothing else needs
 * touching: tab gating, checkbox OR/AND semantics, chips, preview grouping and
 * serialization are generic over these tables.
 */

/** Entity source per id-valued constraint key: which inventory entity list
 *  supplies the pickable values (and resolves ids back to names). */
export const ID_KEY_ENTITY: Record<string, InventoryEntityKey> = {
  role_id: "Device Roles",
  site_id: "Sites",
  region_id: "Regions",
  tenant_id: "Tenants",
  platform_id: "Platforms",
  cluster_id: "Clusters",
};

export const ID_CONSTRAINT_KEYS = Object.keys(ID_KEY_ENTITY);

/** The Область tabs for inventory-hierarchy scoping (top-down). Each is a
 *  friendly checkbox view over the attribute constraint of the same key — the
 *  one mechanism the backend actually enforces (write-time key validation,
 *  per-target ABAC, row-level list visibility). Several checked values of one
 *  level OR together; different levels AND together — so checkbox combinations
 *  can never author a self-contradictory rule. */
export const HIERARCHY_SCOPE_DEFS = [
  { key: "region_id", icon: Globe },
  { key: "site_id", icon: MapPin },
  { key: "tenant_id", icon: Layers },
  { key: "cluster_id", icon: Boxes },
  { key: "role_id", icon: Tag },
  { key: "platform_id", icon: Cpu },
] as const satisfies readonly { key: string; icon: LucideIcon }[];

export type HierarchyKey = (typeof HIERARCHY_SCOPE_DEFS)[number]["key"];

export const HIERARCHY_KEYS: readonly string[] = HIERARCHY_SCOPE_DEFS.map((def) => def.key);
