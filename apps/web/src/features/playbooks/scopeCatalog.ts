import type { InventoryEntityKey } from "@/shared/api/types";
import type { PlaybookAssignmentScopeKind } from "@/features/playbooks/api";

/** Scope kinds that resolve a concrete `scope_id` (everything except `global`). */
export const PLAYBOOK_TARGET_SCOPE_KINDS: Exclude<PlaybookAssignmentScopeKind, "global">[] = [
  "connection_target",
  "role",
  "site",
  "region",
  "tenant",
  "cluster",
  "platform",
];

/**
 * Maps each inventory-backed playbook scope kind to the inventory entity its
 * `scope_id` points at. `global` carries no target (applies everywhere) and
 * `connection_target` resolves through the connection-target workspace rather
 * than an inventory facet, so neither appears here.
 */
export const PLAYBOOK_SCOPE_ENTITY: Record<
  Exclude<PlaybookAssignmentScopeKind, "global" | "connection_target">,
  InventoryEntityKey
> = {
  role: "Device Roles",
  site: "Sites",
  region: "Regions",
  tenant: "Tenants",
  cluster: "Clusters",
  platform: "Platforms",
};
