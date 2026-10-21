import { useMemo } from "react";
import { useQueries, useQuery } from "@tanstack/react-query";
import {
  countDevicesForCluster,
  countDevicesForSite,
  countDevicesForTenant,
  countLocationsForSite,
  countRacksForSite,
  countSitesForRegion,
  countVmsForCluster,
  countVmsForTenant,
  getResourceWithRaw,
  listDeviceInterfaces,
  listContactAssignmentsByObject,
  listDevicesForCluster,
  listDevicesForSite,
  listDevicesForTenant,
  listIpAddressesForDevice,
  listIpAddressesForVirtualMachine,
  listLocationsForSite,
  listRacksForSite,
  listResources,
  listSitesForRegion,
  listVmInterfaces,
  listVmsForCluster,
  listVmsForTenant,
} from "@/features/inventory/api";
import { inventoryInspectorFields } from "@/features/inventory/components/inventory/inventoryInspectorFields";
import type { InventoryEntityKey, Resource } from "@/shared/api/types";
import type { TreeNodeType } from "./TreeNodeInspector";

/** Cache key for the shared contact-assignments roster (list + detach share it). */
const CONTACT_ASSIGNMENTS_KEY = ["inspector", "entityList", "Contact Assignments"] as const;

/**
 * Data layer for the connections TreeNodeInspector. Every hook is gated by an
 * `enabled` flag so nothing fetches until a node is selected and (for group
 * nodes) its entity UUID is resolved. Lists are cached aggressively because the
 * same name→id maps and contact rosters are reused across node selections.
 */

const FIVE_MINUTES = 5 * 60 * 1000;

/** Group node type → the inventory entity whose detail/list backs it. */
export const GROUP_ENTITY: Record<Exclude<TreeNodeType, "host">, InventoryEntityKey> = {
  site: "Sites",
  region: "Regions",
  tenant: "Tenants",
  role: "Device Roles",
  platform: "Platforms",
  deviceType: "Device Types",
  cluster: "Clusters",
};

/**
 * Tree node types whose backing entity is a NetBox-contactable model.
 * NetBox attaches contacts to Site/SiteGroup/Region/Location/Rack/Manufacturer/
 * Device/VirtualDeviceContext/PowerPanel/Tenant/Cluster/ClusterGroup/
 * VirtualMachine/Circuit/Provider/ProviderAccount/ProviderNetwork. Of those,
 * only the models that surface as tree node types here are eligible: host
 * (Device/VirtualMachine), site, region, tenant, cluster. The remaining node
 * types — role (DeviceRole), platform (Platform), deviceType (DeviceType) — are
 * not contactable models, so they are intentionally excluded.
 */
const CONTACT_CAPABLE: ReadonlySet<TreeNodeType> = new Set<TreeNodeType>([
  "host",
  "site",
  "region",
  "tenant",
  "cluster",
]);

export function entitySupportsContacts(type: TreeNodeType): boolean {
  return CONTACT_CAPABLE.has(type);
}

/** Cached full list for a single inventory entity (used for name→id + detail). */
export function useEntityList(entity: InventoryEntityKey | null) {
  return useQuery({
    queryKey: ["inspector", "entityList", entity],
    queryFn: () => listResources(entity as InventoryEntityKey),
    enabled: entity != null,
    staleTime: FIVE_MINUTES,
  });
}

/**
 * id → display-name map across the cached lists of an entity's related FK tabs.
 *
 * Group-node detail responses (`GET /:id`) carry only the FK *ids* (region_id,
 * tenant_id, site_group_id, …), never resolved names, so the curated `related`
 * inspector rows would otherwise render empty and collapse under "show empty
 * fields". This resolves those names from the same `["inspector","entityList",
 * tab]` lists used elsewhere (shared cache — no extra fetch), keyed by the
 * globally-unique entity UUID. `omitKeys` skips related fields the caller renders
 * elsewhere (e.g. the host overview), so no list is fetched for a tab that is
 * never displayed. Returns an empty map until the lists resolve; rows fill in
 * progressively as each cached list lands.
 */
export function useRelatedEntityNames(
  entity: InventoryEntityKey | null,
  omitKeys: ReadonlySet<keyof Resource> | undefined,
  enabled: boolean,
): Map<string, string> {
  const relatedTabs = useMemo<InventoryEntityKey[]>(() => {
    if (!entity) return [];
    const tabs = new Set<InventoryEntityKey>();
    for (const field of inventoryInspectorFields(entity)) {
      if (field.kind !== "related" || !field.relatedTab || !field.idKey) continue;
      if (omitKeys?.has(field.key)) continue;
      tabs.add(field.relatedTab);
    }
    return [...tabs];
  }, [entity, omitKeys]);

  const results = useQueries({
    queries: relatedTabs.map((tab) => ({
      queryKey: ["inspector", "entityList", tab],
      queryFn: () => listResources(tab),
      enabled: enabled && entity != null,
      staleTime: FIVE_MINUTES,
    })),
  });

  return useMemo(() => {
    const nameById = new Map<string, string>();
    for (const result of results) {
      for (const item of result.data?.items ?? []) {
        nameById.set(item.id, item.name);
      }
    }
    return nameById;
  }, [results]);
}

/**
 * Single entity detail by UUID (e.g. a host's device or a resolved group).
 * Returns BOTH the mapped `Resource` (curated rows, resolved FK names) and the
 * raw backend record (the inspector's full-field completeness pass) in one fetch.
 */
export function useEntityDetail(
  entity: InventoryEntityKey | null,
  id: string | undefined,
  enabled = true,
) {
  return useQuery({
    queryKey: ["inspector", "entityDetail", entity, id],
    queryFn: () => getResourceWithRaw(entity as InventoryEntityKey, id as string),
    enabled: enabled && entity != null && Boolean(id),
    staleTime: FIVE_MINUTES,
  });
}

/**
 * Resolve a group node's entity UUID. Prefers an FK id already carried by one of
 * the grouped hosts (cheap, exact); otherwise falls back to a name→id lookup in
 * the cached entity list. Returns `undefined` when ambiguous/unavailable so the
 * caller degrades gracefully (skip the detail section).
 */
export function useResolvedGroupId(
  type: Exclude<TreeNodeType, "host">,
  name: string,
  fkIdFromHost: string | undefined,
  enabled: boolean,
): { id: string | undefined; loading: boolean } {
  const needsList = enabled && !fkIdFromHost;
  const listQuery = useEntityList(needsList ? GROUP_ENTITY[type] : null);

  const id = useMemo(() => {
    if (!enabled) return undefined;
    if (fkIdFromHost) return fkIdFromHost;
    const items = listQuery.data?.items;
    if (!items) return undefined;
    const matches = items.filter((r: Resource) => r.name === name);
    // Only resolve when the name maps to exactly one entity; ambiguous → skip.
    const [only] = matches;
    return matches.length === 1 && only ? only.id : undefined;
  }, [enabled, fkIdFromHost, listQuery.data, name]);

  return { id, loading: needsList && listQuery.isLoading };
}

export interface InspectorContact {
  /** The contact-assignment UUID (this row's identity; NOT a link target). */
  id: string;
  name: string;
  role?: string;
  title?: string;
  email?: string;
  phone?: string;
  address?: string;
  link?: string;
  description?: string;
  groupName?: string;
  priority?: string;
  /** The assigned Contact's UUID — link target for the contact name. */
  contactId?: string;
  /** The assigned Contact Role's UUID — link target for the role label. */
  roleId?: string;
}

export function useContactCatalog(
  enabled: boolean,
): { contacts: Resource[]; roles: Resource[]; loading: boolean } {
  const contactsQuery = useQuery({
    queryKey: ["inspector", "entityList", "Contacts"],
    queryFn: () => listResources("Contacts"),
    enabled,
    staleTime: FIVE_MINUTES,
  });
  const rolesQuery = useQuery({
    queryKey: ["inspector", "entityList", "Contact Roles"],
    queryFn: () => listResources("Contact Roles"),
    enabled,
    staleTime: FIVE_MINUTES,
  });

  return {
    contacts: contactsQuery.data?.items ?? [],
    roles: rolesQuery.data?.items ?? [],
    loading: enabled && (contactsQuery.isLoading || rolesQuery.isLoading),
  };
}

/**
 * Contacts assigned to a given entity UUID. Built from the existing contacts,
 * contact-assignments and contact-roles list endpoints (no contacts-summary
 * route exists). `object_id` on an assignment is the assigned entity's UUID, so
 * a single equality filter is enough — content_type need not be matched.
 */
export interface EntityContactsResult {
  contacts: InspectorContact[];
  loading: boolean;
  /** Eager assignment count for the badge (`undefined` until the query resolves). */
  count: number | undefined;
  /** True while the eager count is still unresolved (drives the badge placeholder). */
  countLoading: boolean;
}

/**
 * Contacts assigned to a given entity UUID. The per-object ASSIGNMENTS roster is
 * fetched EAGERLY (`countEnabled` = inspector card open) so the Contacts count
 * badge fills in without expanding the section — the count is just the assignment
 * roster length, which needs no catalog. The contacts/roles CATALOG (used only to
 * resolve display names for the expanded rows) stays LAZY (`listEnabled` =
 * section/dialog open) so opening a card never fetches the full Contacts +
 * Contact Roles lists just to show a number.
 */
export function useEntityContacts(
  entityUuid: string | undefined,
  listEnabled: boolean,
  countEnabled: boolean,
): EntityContactsResult {
  const hasEntity = Boolean(entityUuid);
  const countActive = countEnabled && hasEntity;
  const listActive = listEnabled && hasEntity;
  const assignmentsQuery = useQuery({
    // Per-object key (the typed `object_id` filter scopes the fetch); shares the
    // CONTACT_ASSIGNMENTS_KEY prefix so detach/attach prefix-invalidation refreshes it.
    queryKey: [...CONTACT_ASSIGNMENTS_KEY, entityUuid],
    queryFn: () => listContactAssignmentsByObject(entityUuid as string),
    enabled: countActive || listActive,
    staleTime: FIVE_MINUTES,
  });
  const catalog = useContactCatalog(listActive);

  const contacts = useMemo<InspectorContact[]>(() => {
    if (!entityUuid) return [];
    const assignments = assignmentsQuery.data ?? [];
    const contactById = new Map(
      catalog.contacts.map((c) => [c.id, c]),
    );
    const roleNameById = new Map(
      catalog.roles.map((r) => [r.id, r.name]),
    );
    // The typed `object_id` filter already scopes the fetch to this entity, so
    // no client-side objectId filter is needed.
    return assignments
      .map((a) => {
        const contact = a.contactId ? contactById.get(a.contactId) : undefined;
        return {
          id: a.id,
          name: contact?.name ?? a.contactName ?? a.contactId ?? a.id,
          role: a.roleId ? roleNameById.get(a.roleId) : undefined,
          title: contact?.title,
          email: contact?.email,
          phone: contact?.phone,
          address: contact?.address,
          link: contact?.link,
          description: contact?.description,
          groupName: contact?.groupName,
          priority: a.priority,
          contactId: a.contactId,
          roleId: a.roleId,
        };
      });
  }, [entityUuid, assignmentsQuery.data, catalog.contacts, catalog.roles]);

  // Badge count is the assignment roster length (no catalog needed); loading is
  // data-presence (NOT `isLoading`) so a disabled query never flashes "0".
  const count = assignmentsQuery.data?.length;

  return {
    contacts,
    loading:
      listActive &&
      (assignmentsQuery.isLoading || catalog.loading),
    count,
    countLoading: countActive && count === undefined,
  };
}

/**
 * A host's network interfaces + IP addresses. Both are inherently small per host,
 * so the FULL lists are fetched EAGERLY (`enabled` = inspector card open, NOT
 * section expansion): the Interfaces and IP Addresses count badges fill in from
 * `items.length` without a click, and the rows are already loaded when the user
 * expands either section (bonus). The IP Addresses count is derived from the same
 * fetch (addresses grouped by interface), so eager interfaces ⇒ eager IP count.
 *
 * `interfacesLoaded` / `addressesLoaded` are data-presence flags (NOT `isLoading`)
 * so the count badges show a placeholder instead of a premature "0".
 */
export function useHostNetworkInterfaces(
  targetKind: "device" | "virtual_machine" | undefined,
  entityId: string | undefined,
  enabled: boolean,
): {
  interfaces: Resource[];
  addressesByInterfaceId: Map<string, Resource[]>;
  loading: boolean;
  interfacesLoaded: boolean;
  addressesLoaded: boolean;
} {
  const active = enabled && Boolean(targetKind) && Boolean(entityId);
  const interfacesQuery = useQuery({
    queryKey: ["inspector", "networkInterfaces", targetKind, entityId],
    queryFn: () =>
      targetKind === "virtual_machine"
        ? listVmInterfaces(entityId as string)
        : listDeviceInterfaces(entityId as string),
    enabled: active,
    staleTime: FIVE_MINUTES,
  });
  const addressesQuery = useQuery({
    queryKey: ["inspector", "networkAddresses", targetKind, entityId],
    queryFn: () =>
      targetKind === "virtual_machine"
        ? listIpAddressesForVirtualMachine(entityId as string)
        : listIpAddressesForDevice(entityId as string),
    enabled: active,
    staleTime: FIVE_MINUTES,
  });

  const addressesByInterfaceId = useMemo(() => {
    const grouped = new Map<string, Resource[]>();
    for (const address of addressesQuery.data ?? []) {
      if (!address.interfaceId) continue;
      const next = grouped.get(address.interfaceId) ?? [];
      next.push(address);
      grouped.set(address.interfaceId, next);
    }
    return grouped;
  }, [addressesQuery.data]);

  return {
    interfaces: interfacesQuery.data ?? [],
    addressesByInterfaceId,
    loading: active && (interfacesQuery.isLoading || addressesQuery.isLoading),
    interfacesLoaded: interfacesQuery.data !== undefined,
    addressesLoaded: addressesQuery.data !== undefined,
  };
}

/** Shape returned by every parent-scoped relation hook (RelatedEntitySection). */
export interface RelationResult {
  items: Resource[];
  loading: boolean;
  error: Error | null;
  /**
   * Eager child count for the badge (envelope `total`, fetched with `limit=1`).
   * `undefined` until the count query resolves so the badge can show a loading
   * placeholder instead of a premature "0".
   */
  count: number | undefined;
  /** True while the eager count is still unresolved (drives the badge placeholder). */
  countLoading: boolean;
}

/**
 * A parent-scoped child collection for the inspector's RelatedEntitySection
 * (e.g. devices at a site, VMs in a cluster). Two independent queries, both gated
 * on a resolved parent UUID:
 *  - the eager COUNT (`countEnabled` = inspector card open) fetches only the
 *    envelope `total` so the badge fills in without expanding the section;
 *  - the lazy LIST (`listEnabled` = section expanded) fetches the rows.
 * `relation` namespaces the cache keys so two relations of the same entity
 * (e.g. Cluster→Devices vs Site→Devices) never collide.
 */
function useParentRelation(
  relation: string,
  parentId: string | undefined,
  fetcher: (parentId: string) => Promise<Resource[]>,
  counter: (parentId: string) => Promise<number>,
  listEnabled: boolean,
  countEnabled: boolean,
): RelationResult {
  const hasParent = Boolean(parentId);
  const listActive = listEnabled && hasParent;
  const countActive = countEnabled && hasParent;
  const listQuery = useQuery({
    queryKey: ["inspector", "relation", relation, parentId],
    queryFn: () => fetcher(parentId as string),
    enabled: listActive,
    staleTime: FIVE_MINUTES,
  });
  const countQuery = useQuery({
    queryKey: ["inspector", "relationCount", relation, parentId],
    queryFn: () => counter(parentId as string),
    enabled: countActive,
    staleTime: FIVE_MINUTES,
  });
  // Badge loading is data-presence, NOT `isLoading`: a disabled query reports
  // `isLoading: false` so keying off it would flash "0" before the count fires.
  const count = countQuery.data;
  return {
    items: listQuery.data ?? [],
    loading: listActive && listQuery.isLoading,
    error: listQuery.error,
    count,
    countLoading: countActive && count === undefined,
  };
}

export function useSiteDevices(
  siteId: string | undefined,
  listEnabled: boolean,
  countEnabled: boolean,
): RelationResult {
  return useParentRelation("site-devices", siteId, listDevicesForSite, countDevicesForSite, listEnabled, countEnabled);
}

export function useSiteRacks(
  siteId: string | undefined,
  listEnabled: boolean,
  countEnabled: boolean,
): RelationResult {
  return useParentRelation("site-racks", siteId, listRacksForSite, countRacksForSite, listEnabled, countEnabled);
}

export function useSiteLocations(
  siteId: string | undefined,
  listEnabled: boolean,
  countEnabled: boolean,
): RelationResult {
  return useParentRelation("site-locations", siteId, listLocationsForSite, countLocationsForSite, listEnabled, countEnabled);
}

export function useRegionSites(
  regionId: string | undefined,
  listEnabled: boolean,
  countEnabled: boolean,
): RelationResult {
  return useParentRelation("region-sites", regionId, listSitesForRegion, countSitesForRegion, listEnabled, countEnabled);
}

export function useTenantDevices(
  tenantId: string | undefined,
  listEnabled: boolean,
  countEnabled: boolean,
): RelationResult {
  return useParentRelation("tenant-devices", tenantId, listDevicesForTenant, countDevicesForTenant, listEnabled, countEnabled);
}

export function useTenantVms(
  tenantId: string | undefined,
  listEnabled: boolean,
  countEnabled: boolean,
): RelationResult {
  return useParentRelation("tenant-vms", tenantId, listVmsForTenant, countVmsForTenant, listEnabled, countEnabled);
}

export function useClusterDevices(
  clusterId: string | undefined,
  listEnabled: boolean,
  countEnabled: boolean,
): RelationResult {
  return useParentRelation("cluster-devices", clusterId, listDevicesForCluster, countDevicesForCluster, listEnabled, countEnabled);
}

export function useClusterVms(
  clusterId: string | undefined,
  listEnabled: boolean,
  countEnabled: boolean,
): RelationResult {
  return useParentRelation("cluster-vms", clusterId, listVmsForCluster, countVmsForCluster, listEnabled, countEnabled);
}
