import type { InventoryEntityKey } from "@/shared/api/types";
import { MONITORING_SCOPE_TYPES, type MonitoringScopeType } from "./api";

export { MONITORING_SCOPE_TYPES };
export type { MonitoringScopeType };

/** Scope layers that resolve a concrete `scope_id` against an inventory entity. */
export const MONITORING_TARGET_SCOPES = MONITORING_SCOPE_TYPES.filter(
  (scope): scope is Exclude<MonitoringScopeType, "global"> => scope !== "global",
);

/**
 * Maps inventory-backed monitoring scopes to the entity their `scope_id` points
 * at. `global` carries no target; `connection_target` resolves through the
 * connection target workspace.
 */
export const MONITORING_SCOPE_ENTITY: Record<
  Exclude<MonitoringScopeType, "global" | "connection_target">,
  InventoryEntityKey
> = {
  site: "Sites",
  tenant: "Tenants",
  role: "Device Roles",
};

/** i18n key suffix for each scope label, under `observability.scope.*`. */
export const MONITORING_SCOPE_I18N_KEY: Record<MonitoringScopeType, string> = {
  global: "global",
  site: "site",
  tenant: "tenant",
  role: "role",
  connection_target: "connection_target",
};

/** Human-readable default labels (used as i18n `defaultValue` fallbacks). */
export const MONITORING_SCOPE_LABEL: Record<MonitoringScopeType, string> = {
  global: "Global",
  site: "Site",
  tenant: "Tenant",
  role: "Role",
  connection_target: "Connection target",
};
