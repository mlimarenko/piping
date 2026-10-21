import type { InventoryEntityKey, Resource } from "@/shared/api/types";
import type { EntityPickerOption } from "@/shared/forms/EntityPicker";

export interface InventoryResourceOption extends EntityPickerOption {
  slug?: string;
}

const compactOptionParts = (...parts: Array<string | undefined | null>): string | undefined => {
  const unique = new Set<string>();
  const compacted = parts
    .map((part) => part?.trim())
    .filter((part): part is string => Boolean(part))
    .filter((part) => {
      const normalized = part.toLocaleLowerCase();
      if (unique.has(normalized)) return false;
      unique.add(normalized);
      return true;
    });
  return compacted.length > 0 ? compacted.join(" · ") : undefined;
};

export const inventoryOptionSubtitleFor = (
  entity: InventoryEntityKey,
  resource: Resource,
): string | undefined => {
  switch (entity) {
    case "Device Types":
      return compactOptionParts(resource.manufacturerName, resource.manufacturerId);
    case "Interfaces":
      return compactOptionParts(
        resource.deviceName,
        resource.site,
        resource.interfaceType,
        resource.primaryIp,
        resource.status,
      );
    case "VM Interfaces":
      return compactOptionParts(
        resource.virtualMachineName,
        resource.site,
        resource.interfaceType,
        resource.primaryIp,
        resource.status,
      );
    case "Devices":
      return compactOptionParts(resource.site, resource.role, resource.primaryIp);
    case "Virtual Machines":
      return compactOptionParts(resource.site, resource.cluster, resource.role, resource.primaryIp);
    case "Racks":
      return compactOptionParts(resource.site, resource.facilityId, resource.tenant);
    case "Prefixes":
      return compactOptionParts(resource.site, resource.vrf, resource.tenant);
    case "IP Addresses":
      return compactOptionParts(resource.dnsName, resource.primaryIp, resource.site, resource.vrf);
    case "VLANs":
      return compactOptionParts(resource.vid, resource.site, resource.tenant);
    case "Clusters":
      return compactOptionParts(resource.site, resource.clusterTypeName, resource.clusterGroupName);
    case "Services":
      return compactOptionParts(resource.parentObject?.label, resource.protocol, resource.ports);
    default:
      return compactOptionParts(resource.site, resource.tenant, resource.region, resource.description);
  }
};

export const inventoryOptionSearchTextFor = (resource: Resource): string | undefined =>
  compactOptionParts(
    resource.deviceName,
    resource.virtualMachineName,
    resource.site,
    resource.tenant,
    resource.region,
    resource.role,
    resource.platform,
    resource.deviceType,
    resource.manufacturerName,
    resource.manufacturerId,
    resource.primaryIp,
    resource.cluster,
    resource.description,
    resource.dnsName,
    resource.vrf,
    resource.vid,
    resource.parentObject?.label,
  );

export const inventoryResourceOption = (
  entity: InventoryEntityKey,
  resource: Resource,
): InventoryResourceOption => ({
  id: resource.id,
  name: resource.name,
  slug: resource.slug,
  subtitle: inventoryOptionSubtitleFor(entity, resource),
  searchText: inventoryOptionSearchTextFor(resource),
});
