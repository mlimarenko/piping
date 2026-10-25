import { queryOptions, skipToken, type QueryClient } from "@tanstack/react-query";
import type { InventoryEntityKey, Paginated, Resource } from "@/shared/api/types";
import type { EntityPickerOption } from "@/shared/forms/EntityPicker";
import { listAllResources } from "./api";

const completeResourceList = (items: Resource[]): Paginated<Resource> => ({
  items, total: items.length, limit: items.length, offset: 0,
});

/** Shared FK options retain the inventory envelope and include every page. */
export const inventoryResourceOptionsQuery = (entity: InventoryEntityKey | null) => queryOptions({
  queryKey: ["inventory", "resources", entity, "options"],
  queryFn: entity ? async () => completeResourceList(await listAllResources(entity)) : skipToken,
  staleTime: 30_000,
});

export async function cacheCreatedInventoryResource(client: QueryClient, entity: InventoryEntityKey, created: Resource): Promise<void> {
  const { queryKey } = inventoryResourceOptionsQuery(entity);
  // An older list response must not overwrite a just-persisted option.
  await client.cancelQueries({ queryKey });
  const previous = client.getQueryData(queryKey);
  const hadCompleteList = previous !== undefined
    && previous.offset === 0 && previous.items.length === previous.total
    && !client.getQueryState(queryKey)?.isInvalidated;
  client.setQueryData(queryKey, (current) => completeResourceList([
    ...(current?.items ?? []).filter((item) => item.id !== created.id), created,
  ]));
  if (!hadCompleteList) {
    // A created-only envelope is immediately selectable, not a complete list.
    // Keep it stale until a fresh full read succeeds; read failures must not
    // turn the successful creation into an error or remove its cached option.
    void client.invalidateQueries({ queryKey }, { throwOnError: false });
  }
}

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
