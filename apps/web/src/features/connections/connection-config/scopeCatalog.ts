import type { InventoryEntityKey } from "@/shared/api/types";
import type { ConnConfScopeType } from "./api";

/**
 * Maps each scope layer to the inventory entity it targets. `global` has no
 * target (applies to everything). All other layers resolve their `scope_id`
 * against the listed inventory entity.
 */
export const SCOPE_ENTITY: Record<
  Exclude<ConnConfScopeType, "global">,
  InventoryEntityKey
> = {
  region: "Regions",
  site_group: "Site Groups",
  site: "Sites",
  location: "Locations",
  rack: "Racks",
  tenant: "Tenants",
  cluster: "Clusters",
  role: "Device Roles",
  device_type: "Device Types",
  platform: "Platforms",
  device: "Devices",
  vm: "Virtual Machines",
};

/** i18n key suffix for each scope label, under `connectionConfig.scope.*`. */
export const SCOPE_I18N_KEY: Record<ConnConfScopeType, string> = {
  global: "global",
  region: "region",
  site_group: "siteGroup",
  site: "site",
  location: "location",
  rack: "rack",
  tenant: "tenant",
  cluster: "cluster",
  role: "role",
  device_type: "deviceType",
  platform: "platform",
  device: "device",
  vm: "vm",
};
