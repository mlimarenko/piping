import { useMemo, useState } from "react";
import { useInspectorSections } from "./useInspectorSections";
import { useTranslation } from "react-i18next";
import { useQuery } from "@tanstack/react-query";
import { Link, useLocation, useNavigate } from "react-router";
import InspectorFieldLabel from "@/shared/ui-shared/InspectorFieldLabel";
import { Button } from "@/shared/ui-primitives/button";
import {
  X, Building2, Server, Network, Map as MapIcon, Tag, Users, Cpu,
  HardDrive, Terminal, Layers,
  Gauge, Info, Contact as ContactIcon, Loader2,
  Palette, Factory, FileText, AlertTriangle, ClipboardList,
  Mail, Phone, MapPin, Link2, BriefcaseBusiness,
  ChevronRight, ChevronDown, ExternalLink, Pencil,

} from "lucide-react";
import { cn } from "@/shared/lib/utils";
import { notify } from "@/shared/lib/notifications";
import { writeBrowserClipboardText } from "@/shared/lib/clipboard";
import { connectionTargetFallbackIcon, treeGroupIconFor } from "@/shared/entity-icon-utils";
import { EntityIcon } from "@/shared/entity-icons";
import HostMetricsDialog from "@/features/observability/HostMetricsDialog";
import HostMetricsPreviewSection from "@/features/observability/HostMetricsPreviewSection";
import HostPlaybooksPreviewSection from "@/features/playbooks/HostPlaybooksPreviewSection";
import type { PlaybookTargetAssignment } from "@/features/playbooks/api";
import type { ConnectionTarget, InventoryEntityKey, Resource } from "@/shared/api/types";
import {
  inventoryResourceHref,
  inventoryResourceEditPath,
} from "@/features/inventory/resourceLinks";
import { connectionsHostUrl } from "@/features/connections/workspaceLinks";
import { inventoryResourceStatusLabel } from "@/features/inventory/resourceStatus";
import {
  inventoryInspectorFields,
  inventoryInspectorHasStatus,
} from "@/features/inventory/components/inventory/inventoryInspectorFields";
import {
  curatedSnakeKeys,
  customFieldRows,
  genericInspectorRows,
  type CustomFieldRow,
} from "@/features/inventory/components/inventory/inventoryRawFields";
import {
  revealEntityCustomFields,
  type CustomFieldEntityKind,
} from "@/features/inventory/custom-fields-api";
import type { TreeGroupKey } from "./treeConfig";
import ConnectButtons from "./ConnectButtons";
import type { AdHocCredentialsContext } from "./AdHocCredentialsDialog";
import { useDeferredInspectorQueries } from "./useDeferredInspectorQueries";
import {
  GROUP_ENTITY,
  entitySupportsContacts,
  useClusterDevices,
  useClusterVms,
  useEntityContacts,
  useEntityDetail,
  useHostNetworkInterfaces,
  useRegionSites,
  useRelatedEntityNames,
  useResolvedGroupId,
  useSiteDevices,
  useSiteLocations,
  useSiteRacks,
  useTenantDevices,
  useTenantVms,
  type InspectorContact,
} from "./inspectorData";
import type { OpenableProtocol } from "./launchPresentation";
import InspectorSection from "./InspectorSection";
import RenderedConfigContextPanel from "@/features/inventory/components/config-contexts/RenderedConfigContextPanel";
import { usePermissions } from "@/shared/auth/permissionsContext";
import RelatedEntitySection, { CopyableDetailRow, RelatedEntityRow } from "./RelatedEntitySection";
import { InspectorCopyButton, InspectorFieldRow } from "./InspectorFieldRow";

export type TreeNodeType = TreeGroupKey | "host";

export interface TreeNode {
  type: TreeNodeType;
  id: string;
  name: string;
  host?: ConnectionTarget;
  hosts?: ConnectionTarget[];
  meta?: Record<string, string>;
}

interface Props {
  node: TreeNode;
  onClose: () => void;
  onConnect?: (host: ConnectionTarget, protocol: OpenableProtocol, entryId?: string, username?: string) => void;
  /** Opens the ad-hoc credentials dialog (mounted at the workspace level). */
  onOpenAdHoc?: (context: AdHocCredentialsContext) => void;
  /** When true, render full-width as a row below the list instead of a fixed-width side column */
  stacked?: boolean;
  /**
   * Explicit column width in px. When set, the inspector fills that width
   * instead of its default fixed `md:w-80`, so the workspace owns resizing.
   */
  width?: number;
  /** Called when the user selects a playbook from a host node. The workspace owns confirmation and launch. */
  onPlaybookQuickAction?: (host: ConnectionTarget, item: PlaybookTargetAssignment) => void;
}

/** Per-field icon for metadata-driven group detail rows; falls back per kind. */
const FIELD_ICONS: Partial<Record<keyof Resource, React.ElementType>> = {
  region: MapIcon,
  parentName: MapIcon,
  site: Building2,
  tenant: Users,
  groupName: Layers,
  siteGroupName: Layers,
  clusterTypeName: Layers,
  clusterGroupName: Layers,
  manufacturerName: Factory,
  napalmDriver: Server,
  color: Palette,
  facility: Factory,
  timeZone: Info,
  physicalAddress: MapIcon,
  shippingAddress: MapIcon,
  latitude: MapIcon,
  longitude: MapIcon,
  description: FileText,
  comments: FileText,
  position: HardDrive,
  face: HardDrive,
  airflow: Network,
  vcpus: Cpu,
  memory: Server,
  disk: HardDrive,
};

const FIELD_HELP_KEY: Readonly<Record<string, string>> = {
  airflow: "connections.inspector.fieldHelp.airflow",
  asns: "connections.inspector.fieldHelp.asns",
  assetTag: "connections.inspector.fieldHelp.assetTag",
  asset_tag: "connections.inspector.fieldHelp.assetTag",
  cluster: "connections.inspector.fieldHelp.cluster",
  cluster_group: "connections.inspector.fieldHelp.clusterGroup",
  clusterGroupName: "connections.inspector.fieldHelp.clusterGroup",
  cluster_type: "connections.inspector.fieldHelp.clusterType",
  clusterTypeName: "connections.inspector.fieldHelp.clusterType",
  color: "connections.inspector.fieldHelp.color",
  comments: "connections.inspector.fieldHelp.comments",
  created_at: "connections.inspector.fieldHelp.created",
  custom_fields: "connections.inspector.fieldHelp.customFields",
  description: "connections.inspector.fieldHelp.description",
  deviceType: "connections.inspector.fieldHelp.type",
  external_id: "connections.inspector.fieldHelp.externalId",
  face: "connections.inspector.fieldHelp.face",
  facility: "connections.inspector.fieldHelp.facility",
  groupName: "connections.inspector.fieldHelp.group",
  id: "connections.inspector.fieldHelp.id",
  latitude: "connections.inspector.fieldHelp.latitude",
  longitude: "connections.inspector.fieldHelp.longitude",
  manufacturer: "connections.inspector.fieldHelp.manufacturer",
  manufacturerName: "connections.inspector.fieldHelp.manufacturer",
  napalm_driver: "connections.inspector.fieldHelp.napalmDriver",
  name: "connections.inspector.fieldHelp.name",
  napalmDriver: "connections.inspector.fieldHelp.napalmDriver",
  parentName: "connections.inspector.fieldHelp.parent",
  physical_address: "connections.inspector.fieldHelp.physicalAddress",
  physicalAddress: "connections.inspector.fieldHelp.physicalAddress",
  piping_synced_at: "connections.inspector.fieldHelp.pipingSyncedAt",
  platform: "connections.inspector.fieldHelp.platform",
  position: "connections.inspector.fieldHelp.position",
  primary_ip: "connections.inspector.fieldHelp.primaryIp",
  primaryIp: "connections.inspector.fieldHelp.primaryIp",
  region: "connections.inspector.fieldHelp.region",
  role: "connections.inspector.fieldHelp.role",
  serial: "connections.inspector.fieldHelp.serial",
  serial_number: "connections.inspector.fieldHelp.serial",
  shipping_address: "connections.inspector.fieldHelp.shippingAddress",
  shippingAddress: "connections.inspector.fieldHelp.shippingAddress",
  site: "connections.inspector.fieldHelp.site",
  siteGroupName: "connections.inspector.fieldHelp.siteGroup",
  slug: "connections.inspector.fieldHelp.slug",
  status: "connections.inspector.fieldHelp.status",
  tags: "connections.inspector.fieldHelp.tags",
  tenant: "connections.inspector.fieldHelp.tenant",
  time_zone: "connections.inspector.fieldHelp.timeZone",
  timeZone: "connections.inspector.fieldHelp.timeZone",
  type: "connections.inspector.fieldHelp.type",
  updated: "connections.inspector.fieldHelp.updated",
  updated_at: "connections.inspector.fieldHelp.updated",
};

interface HostOverviewField {
  key: keyof ConnectionTarget;
  labelKey: string;
  icon: React.ElementType;
  idKey?: keyof ConnectionTarget;
  relatedTab?: InventoryEntityKey;
  mono?: boolean;
}

const HOST_OVERVIEW_FIELDS: Record<ConnectionTarget["targetKind"], HostOverviewField[]> = {
  device: [
    { key: "role", idKey: "roleId", relatedTab: "Device Roles", labelKey: "connections.columns.role", icon: Cpu },
    { key: "platform", idKey: "platformId", relatedTab: "Platforms", labelKey: "connections.columns.platform", icon: Layers },
    { key: "deviceType", idKey: "deviceTypeId", relatedTab: "Device Types", labelKey: "connections.columns.type", icon: HardDrive },
    { key: "region", idKey: "regionId", relatedTab: "Regions", labelKey: "connections.columns.region", icon: MapIcon },
    { key: "site", idKey: "siteId", relatedTab: "Sites", labelKey: "connections.columns.site", icon: Building2 },
    { key: "tenant", idKey: "tenantId", relatedTab: "Tenants", labelKey: "connections.columns.tenant", icon: Users },
  ],
  virtual_machine: [
    { key: "role", idKey: "roleId", relatedTab: "Device Roles", labelKey: "connections.columns.role", icon: Cpu },
    { key: "platform", idKey: "platformId", relatedTab: "Platforms", labelKey: "connections.columns.platform", icon: Layers },
    { key: "cluster", idKey: "clusterId", relatedTab: "Clusters", labelKey: "connections.filtersPanel.cluster", icon: Layers },
    { key: "region", idKey: "regionId", relatedTab: "Regions", labelKey: "connections.columns.region", icon: MapIcon },
    { key: "site", idKey: "siteId", relatedTab: "Sites", labelKey: "connections.columns.site", icon: Building2 },
    { key: "tenant", idKey: "tenantId", relatedTab: "Tenants", labelKey: "connections.columns.tenant", icon: Users },
  ],
};

const HOST_DETAIL_OMIT_KEYS: ReadonlySet<keyof Resource> = new Set<keyof Resource>([
  "role",
  "platform",
  "deviceType",
  "region",
  "site",
  "tenant",
  "cluster",
  "primaryIp",
]);

function isRecord(value: unknown): value is Record<string, unknown> {
  return value != null && typeof value === "object" && !Array.isArray(value);
}

function customFieldRevealKindForEntity(entity: InventoryEntityKey | null): CustomFieldEntityKind | null {
  switch (entity) {
    case "Sites":
      return "sites";
    case "Devices":
      return "devices";
    case "Regions":
      return "regions";
    case "Clusters":
      return "clusters";
    case "Tenants":
      return "tenants";
    case "Virtual Machines":
      return "virtual-machines";
    default:
      return null;
  }
}

function hasCustomFieldPayload(
  raw?: Record<string, unknown>,
  fallback?: Record<string, unknown> | null,
): boolean {
  return [
    raw?.custom_fields,
    raw?.custom_field_data,
    fallback,
  ].some((candidate) => isRecord(candidate) && Object.keys(candidate).length > 0);
}

function mergeRevealedCustomFields(
  raw: Record<string, unknown> | undefined,
  fallback: Record<string, unknown> | null | undefined,
  revealed: Record<string, unknown> | undefined,
): { raw: Record<string, unknown> | undefined; fallback: Record<string, unknown> | null | undefined } {
  if (!revealed || Object.keys(revealed).length === 0) return { raw, fallback };

  if (raw && isRecord(raw.custom_fields)) {
    return {
      raw: { ...raw, custom_fields: { ...raw.custom_fields, ...revealed } },
      fallback,
    };
  }

  if (raw && isRecord(raw.custom_field_data)) {
    return {
      raw: { ...raw, custom_field_data: { ...raw.custom_field_data, ...revealed } },
      fallback,
    };
  }

  if (isRecord(fallback)) {
    return { raw, fallback: { ...fallback, ...revealed } };
  }

  return { raw, fallback };
}

const TreeNodeInspector = ({
  node,
  onClose,
  onConnect,
  onOpenAdHoc,
  stacked = false,
  width,
  onPlaybookQuickAction,
}: Props) => {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const location = useLocation();
  const { can } = usePermissions();
  // Hide the whole Quick Connect section (header included) from a
  // READ-without-CONNECT role so it never reads as an empty/orphaned section.
  // CONNECT is held by every seed role; this only affects custom roles.
  const canConnect = can("CONNECT", "CONNECTION_TARGET");
  const { expandedSections, toggleSection: toggleExpandedSection } = useInspectorSections();
  const [showMetrics, setShowMetrics] = useState(false);
  const [hasConfiguredMetrics, setHasConfiguredMetrics] = useState(false);
  const [hasEligiblePlaybooks, setHasEligiblePlaybooks] = useState(false);
  const [detailsTouched, setDetailsTouched] = useState(false);

  const toggleSection = (key: string) => {
    if (key === "details") setDetailsTouched(true);
    toggleExpandedSection(key);
  };

  // Reset the metrics panel when the inspected node changes. Done during render
  // via a key sentinel (React's recommended alternative to a setState-in-effect)
  // so switching nodes never triggers a cascading re-render.
  const nodeKey = `${node.type}:${node.id}:${node.host?.id ?? ""}`;
  const [prevNodeKey, setPrevNodeKey] = useState(nodeKey);
  if (prevNodeKey !== nodeKey) {
    setPrevNodeKey(nodeKey);
    setHasConfiguredMetrics(false);
    setHasEligiblePlaybooks(false);
    setShowMetrics(false);
    setDetailsTouched(false);
  }
  const deferredQueriesEnabled = useDeferredInspectorQueries(nodeKey);
  const detailsFetchEnabled = deferredQueriesEnabled || detailsTouched;

  const hosts = node.hosts || (node.host ? [node.host] : []);
  const isHost = node.type === "host";
  const headerIconHost =
    node.type === "host"
      ? node.host
      : node.type === "role"
        ? hosts.find((host) => host.roleIconDataUrl || host.roleIconName)
        : undefined;
  const headerFallbackIcon =
    node.type === "host" && node.host
      ? connectionTargetFallbackIcon(node.host)
      : treeGroupIconFor(node.type as TreeGroupKey);

  // ── Detail/contacts data layer ──
  // ConnectionTarget nodes fetch their device/VM detail by UUID (targetKind-
  // aware); group nodes resolve an entity UUID first (FK from a host, else
  // name→id from the cached list).
  const groupType = isHost ? null : (node.type as Exclude<typeof node.type, "host">);
  const hostTargetKind = node.host?.targetKind;
  // Host detail entity is targetKind-aware: device → Devices/deviceId,
  // virtual_machine → Virtual Machines/vmId; the bare host id is a fallback.
  const hostEntity: InventoryEntityKey | null = isHost
    ? hostTargetKind === "virtual_machine" ? "Virtual Machines" : "Devices"
    : null;
  const hostEntityId = isHost
    ? (hostTargetKind === "virtual_machine" ? node.host?.vmId : node.host?.deviceId) ?? node.host?.id
    : undefined;

  const contactsSectionOpen =
    expandedSections.includes("contacts") && entitySupportsContacts(node.type);
  const groupResolutionEnabled = !isHost && (detailsFetchEnabled || contactsSectionOpen);
  const groupFkFromHost = groupType
    ? hostGroupFk(hosts[0], groupType)
    : undefined;
  const groupResolved = useResolvedGroupId(
    groupType ?? "site",
    node.name,
    groupFkFromHost,
    groupResolutionEnabled,
  );
  const entityUuid = isHost ? hostEntityId : groupResolved.id;
  const detailEntity = isHost ? hostEntity : groupType ? GROUP_ENTITY[groupType] : null;
  const detailQuery = useEntityDetail(detailEntity, entityUuid, detailsFetchEnabled);
  const detail = detailQuery.data?.resource;
  const detailRaw = detailQuery.data?.raw;
  const customFieldEntityKind = customFieldRevealKindForEntity(detailEntity);
  const customFieldsSectionOpen = expandedSections.includes("custom-fields");
  const detailCustomFields = (detail as { customFields?: Record<string, unknown> | null } | undefined)?.customFields;
  const revealedCustomFieldsQuery = useQuery({
    queryKey: ["inspector", "customFieldsReveal", customFieldEntityKind, entityUuid],
    queryFn: () => revealEntityCustomFields(customFieldEntityKind as CustomFieldEntityKind, entityUuid as string),
    enabled:
      customFieldsSectionOpen &&
      Boolean(customFieldEntityKind) &&
      Boolean(entityUuid) &&
      hasCustomFieldPayload(detailRaw, detailCustomFields),
    retry: false,
    staleTime: 5 * 60 * 1000,
  });
  const mergedCustomFields = useMemo(
    () => mergeRevealedCustomFields(detailRaw, detailCustomFields, revealedCustomFieldsQuery.data),
    [detailRaw, detailCustomFields, revealedCustomFieldsQuery.data],
  );
  const customFields = useMemo(() => {
    if (!detail) return [];
    return customFieldRows(mergedCustomFields.raw, mergedCustomFields.fallback);
  }, [detail, mergedCustomFields]);
  const detailsLoading = !detailsFetchEnabled || detailQuery.isLoading || groupResolved.loading;
  const detailsUnavailable =
    detailsFetchEnabled &&
    Boolean(entityUuid) &&
    !detail &&
    !detailQuery.isLoading &&
    !groupResolved.loading;
  // The metadata-driven Details section renders the entity's full field set
  // (populated rows inline, empties collapsed), so any resolved detail has rows.
  const detailsHasRows = Boolean(detail);
  const showDetailsSection =
    Boolean(detailEntity) &&
    (detailsLoading || detailsUnavailable || detailsHasRows || isHost);

  // The node's own inventory edit deep-link (secondary affordance on the header).
  const selfHref = inventoryResourceHref(detailEntity ?? undefined, entityUuid);
  // Editing opens the canonical full-page inventory editor and returns to this
  // exact workspace URL; the inspector has no in-workspace edit overlay.
  const openEdit = (entity: InventoryEntityKey, id: string) => {
    const returnTo = `${location.pathname}${location.search}`;
    void navigate(inventoryResourceEditPath(entity, id, returnTo));
  };
  const canUpdateInventory = can("UPDATE", "INVENTORY");
  const canEditNode = Boolean(detailEntity) && Boolean(entityUuid) && canUpdateInventory;
  // Clicking a related-entity row navigates to that entity's canonical editor;
  // without UPDATE the rows keep their inventory deep-link.
  const onEditRelated = canUpdateInventory
    ? (entity: InventoryEntityKey, id: string) => openEdit(entity, id)
    : undefined;
  // Shareable deep-link to this host in the connections workspace (selects +
  // opens its inspector). Host nodes only; keyed on the ConnectionTarget id.
  const hostLinkId = node.type === "host" ? node.host?.id : undefined;
  const copyHostLink = hostLinkId
    ? () => {
        void writeBrowserClipboardText(connectionsHostUrl(hostLinkId)).then(
          () => notify("success", t("connections.inspector.linkCopied")),
          () => notifyClipboardWriteUnavailable(t),
        );
      }
    : undefined;
  const nodeTypeLabel =
    node.type === "host"
      ? t(`connections.filtersPanel.targetKinds.${hostTargetKind}`, { defaultValue: t("connections.filtersPanel.targetKind") })
      : detailEntity
        ? t(`inventory.resources.entities.${detailEntity}.one`, { defaultValue: node.type })
        : node.type;
  const subtitle = node.type === "host"
    ? nodeTypeLabel
    : `${nodeTypeLabel} · ${t("connections.hostsCount", { count: hosts.length })}`;

  // Contacts: assignments joined to this entity, plus compact attach/create actions.
  const contactTarget = attachTarget(node);
  const contactObjectId = isHost ? contactTarget?.objectId : entityUuid;
  // Contacts: lazy LIST (section open) hydrates the rows; eager COUNT (card-open,
  // deferred) fetches just the per-object assignment roster so the badge shows the
  // real number without expanding. Read-only — no attach/detach from the inspector.
  const {
    contacts,
    loading: contactsLoading,
    count: contactsCount,
    countLoading: contactsCountLoading,
  } = useEntityContacts(contactObjectId, contactsSectionOpen, deferredQueriesEnabled);

  // ── Relations registry data layer ──
  // A fixed superset of relation hooks is called UNCONDITIONALLY (rules-of-hooks)
  // — each is `enabled`-gated on its section being expanded AND the parent UUID
  // being resolved, so a hook only fetches for the node type that renders it.
  const sectionOpen = (key: string) => expandedSections.includes(key);
  // Network interfaces + IP addresses are inherently small, so the full lists are
  // fetched EAGERLY on card-open (deferred post-paint, non-blocking): the count
  // badges fill in from `items.length` without a click, and the rows are already
  // loaded when either section is expanded. The fetch backs BOTH the Interfaces
  // section and the derived IP Addresses section.
  const networkEager = isHost && deferredQueriesEnabled;
  const network = useHostNetworkInterfaces(hostTargetKind, hostEntityId, networkEager);
  // Orphan IP addresses: grouped under an interface id that is NOT in the host's
  // interface list (e.g. an interface filtered out or not yet loaded). The nested
  // Interfaces section renders each interface's own IPs inline; these leftovers get
  // a small fallback group so they are never silently dropped.
  // (Addresses with no interfaceId at all are already dropped by the data hook.)
  const orphanIpAddresses = useMemo(() => {
    const knownInterfaceIds = new Set(network.interfaces.map((iface) => iface.id));
    const orphans: Resource[] = [];
    for (const [interfaceId, addresses] of network.addressesByInterfaceId) {
      if (knownInterfaceIds.has(interfaceId)) continue;
      orphans.push(...addresses);
    }
    return orphans;
  }, [network.interfaces, network.addressesByInterfaceId]);

  const siteId = groupType === "site" ? entityUuid : undefined;
  const regionId = groupType === "region" ? entityUuid : undefined;
  const tenantId = groupType === "tenant" ? entityUuid : undefined;
  const clusterId = groupType === "cluster" ? entityUuid : undefined;

  // Each relation: lazy LIST (enabled on section expand) drives the rows; eager
  // COUNT (enabled on card-open + resolved parent UUID, deferred post-paint) fills
  // the badge without a click. The parent-UUID gate lives inside each hook.
  const siteDevices = useSiteDevices(siteId, sectionOpen("site-devices"), deferredQueriesEnabled);
  const siteRacks = useSiteRacks(siteId, sectionOpen("site-racks"), deferredQueriesEnabled);
  const siteLocations = useSiteLocations(siteId, sectionOpen("site-locations"), deferredQueriesEnabled);
  const regionSites = useRegionSites(regionId, sectionOpen("region-sites"), deferredQueriesEnabled);
  const tenantDevices = useTenantDevices(tenantId, sectionOpen("tenant-devices"), deferredQueriesEnabled);
  const tenantVms = useTenantVms(tenantId, sectionOpen("tenant-vms"), deferredQueriesEnabled);
  const clusterDevices = useClusterDevices(clusterId, sectionOpen("cluster-devices"), deferredQueriesEnabled);
  const clusterVms = useClusterVms(clusterId, sectionOpen("cluster-vms"), deferredQueriesEnabled);

  const groupMetaEntries = node.type === "host" ? [] : Object.entries(node.meta ?? {});
  const showOverviewSection = (node.type === "host" && Boolean(node.host)) || groupMetaEntries.length > 0;

  // Interface entity for the host interfaces section + add dialog.
  const interfaceEntity: InventoryEntityKey =
    hostTargetKind === "virtual_machine" ? "VM Interfaces" : "Interfaces";
  const interfacesTitle =
    hostTargetKind === "virtual_machine"
      ? t("connections.inspector.relations.vmInterfaces")
      : t("connections.inspector.relations.networkInterfaces");

  return (
    <>
      <section
        className={cn(
          "@container flex min-h-0 flex-1 flex-col overflow-hidden bg-background text-sm",
          stacked
            ? "w-full h-full min-h-0"
            : "w-full md:w-80",
        )}
        style={!stacked && width ? { width } : undefined}
      >
      {/* Identity shares the scroll viewport so long names cannot displace details. */}
      <div className="min-h-0 flex-1 overflow-y-auto">
      <div className="shrink-0 p-3 border-b border-border/30">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2 min-w-0">
            <div className="h-9 w-9 rounded-md surface-inset flex items-center justify-center shrink-0">
              <EntityIcon
                fallback={headerFallbackIcon}
                iconName={headerIconHost?.roleIconName}
                customDataUrl={headerIconHost?.roleIconDataUrl}
                className="h-3.5 w-3.5 text-primary"
                iconClassName="h-3.5 w-3.5"
              />
            </div>
            <div className="min-w-0">
              <div className="flex items-center gap-1 min-w-0">
                {canEditNode ? (
                  // Primary header action: navigate to the canonical full-page
                  // inventory editor for this node.
                  <button
                    type="button"
                    onClick={() => detailEntity && entityUuid && openEdit(detailEntity, entityUuid)}
                    className="group/edit flex min-w-0 items-center gap-1 text-left text-sm font-semibold text-foreground hover:text-primary"
                    title={t("connections.inspector.openEdit")}
                  >
                    <span className="[overflow-wrap:anywhere] group-hover/edit:underline">{node.name}</span>
                    <Pencil className="size-4 shrink-0" aria-hidden="true" />
                  </button>
                ) : selfHref ? (
                  <Link
                    to={selfHref}
                    className="block text-sm font-semibold text-foreground [overflow-wrap:anywhere] hover:text-primary hover:underline"
                    title={t("connections.inspector.openInInventory")}
                  >
                    {node.name}
                  </Link>
                ) : (
                  <h2 className="text-sm font-semibold text-foreground [overflow-wrap:anywhere]">{node.name}</h2>
                )}
                {/* Secondary affordance: keep the inventory deep-link available as
                    a small icon when the primary click edits in-workspace. */}
                {canEditNode && selfHref && (
                  <Link
                    to={selfHref}
                    className="inline-flex h-6 w-6 shrink-0 items-center justify-center rounded-sm text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                    aria-label={t("connections.inspector.openInInventory")}
                    title={t("connections.inspector.openInInventory")}
                  >
                    <ExternalLink className="h-3 w-3" aria-hidden="true" />
                  </Link>
                )}
                {/* Shareable workspace deep-link to this host. */}
                {copyHostLink && (
                  <button
                    type="button"
                    onClick={copyHostLink}
                    className="inline-flex h-6 w-6 shrink-0 items-center justify-center rounded-sm text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                    aria-label={t("connections.inspector.copyLink")}
                    title={t("connections.inspector.copyLink")}
                  >
                    <Link2 className="h-3 w-3" aria-hidden="true" />
                  </button>
                )}
              </div>
              <p className="text-xs text-muted-foreground [overflow-wrap:anywhere]">{subtitle}</p>
            </div>
          </div>
          <Button variant="ghost" size="icon-sm" className="shrink-0" onClick={onClose} aria-label={t("close")}>
            <X className="h-4 w-4" />
          </Button>
        </div>
      </div>

      <div>
        {node.type === "host" && node.host && (
          <HostMetricsPreviewSection
            host={node.host}
            kind={node.host.targetKind === "virtual_machine" ? "vm" : "device"}
            visible={false}
            enabled={deferredQueriesEnabled}
            onConfiguredMetricsChange={setHasConfiguredMetrics}
          />
        )}
        {node.type === "host" && node.host && (
          <HostPlaybooksPreviewSection
            host={node.host}
            visible={false}
            enabled={deferredQueriesEnabled}
            onAvailabilityChange={setHasEligiblePlaybooks}
          />
        )}

        {/* ConnectionTarget-specific: anti-SSRF binding warning — surfaced prominently
            because connect will be rejected by the backend for these hosts. */}
        {node.type === "host" && node.host?.addressBound === false && (
          <div className="m-3 flex items-start gap-2 rounded-md border border-status-warning/30 bg-status-warning/10 px-2.5 py-2 text-xs leading-snug text-status-warning">
            <AlertTriangle className="h-3.5 w-3.5 shrink-0 mt-px" />
            <span>{t("connections.noBoundAddress")}</span>
          </div>
        )}

        {/* ConnectionTarget-specific: Quick Connect */}
        {node.type === "host" && node.host && onConnect && canConnect && (
          <Section title={t("connections.inspector.openConnection")} icon={Terminal} sectionKey="connect" expanded={expandedSections} onToggle={toggleSection}>
            <ConnectButtons host={node.host} onConnect={onConnect} onOpenAdHoc={onOpenAdHoc} />
          </Section>
        )}

        {node.type === "host" && node.host && hasEligiblePlaybooks && (
          <Section title={t("playbooks.inspector.title")} icon={ClipboardList} sectionKey="playbooks" expanded={expandedSections} onToggle={toggleSection}>
            <HostPlaybooksPreviewSection
              host={node.host}
              enabled={deferredQueriesEnabled}
              layout="buttons"
              onAvailabilityChange={setHasEligiblePlaybooks}
              onLaunch={(item) => {
                if (node.host) onPlaybookQuickAction?.(node.host, item);
              }}
            />
          </Section>
        )}

        {/* Overview: synchronous host data only. Group nodes no longer render
            self/aggregate rows; entity facts live in the metadata-driven
            Details section. */}
        {showOverviewSection && (
          <Section title={t("connections.inspector.info")} icon={Cpu} sectionKey="overview" expanded={expandedSections} onToggle={toggleSection}>
            {node.type === "host" && node.host ? (
              <HostOverview host={node.host} onEdit={onEditRelated} t={t} />
            ) : (
              <>
                {groupMetaEntries.map(([k, v]) => (
                  <InspectorFieldRow key={k} icon={Tag} label={k} help={inspectorFieldHelp(t, k)} value={v} t={t} />
                ))}
              </>
            )}
          </Section>
        )}

        {/* Details: the entity's own fields, fetched lazily by UUID. */}
        {showDetailsSection && (
          <Section title={t("connections.inspector.details")} icon={Info} sectionKey="details" expanded={expandedSections} onToggle={toggleSection}>
            {detailsLoading ? (
              <LoadingRow t={t} />
            ) : isHost ? (
              detail ? (
                <HostDetailFields targetKind={hostTargetKind} detail={detail} raw={detailRaw} onEdit={onEditRelated} t={t} />
              ) : (
                <MutedRow text={t("connections.inspector.detailsUnavailable")} />
              )
            ) : detail ? (
              <GroupDetailFields entity={detailEntity as InventoryEntityKey} detail={detail} raw={detailRaw} onEdit={onEditRelated} t={t} />
            ) : (
              <MutedRow text={t("connections.inspector.detailsUnavailable")} />
            )}
          </Section>
        )}

        {customFields.length > 0 && (
          <Section
            title={t("customFields.entity.section")}
            icon={Tag}
            sectionKey="custom-fields"
            expanded={expandedSections}
            onToggle={toggleSection}
          >
            <CustomFieldRows rows={customFields} t={t} />
          </Section>
        )}

        {/* Config context — rendered, inheritance-resolved (devices/VMs only).
            Collapsed by default so it only fetches when opened. */}
        {(detailEntity === "Devices" || detailEntity === "Virtual Machines") && entityUuid && (
          <Section
            title={t("configContexts.rendered.section")}
            icon={Layers}
            sectionKey="config-context"
            expanded={expandedSections}
            onToggle={toggleSection}
          >
            <RenderedConfigContextPanel
              kind={detailEntity === "Virtual Machines" ? "virtual-machine" : "device"}
              entityId={entityUuid}
              showHeader={false}
            />
          </Section>
        )}

        {/* Relations registry — standardised RelatedEntitySection per node type,
            rendered below the node's own full fields. Contacts (all contactable
            nodes) and host Interfaces have a working "+"; every parent-scoped
            list is read-only (no quick-create dialog). */}

        {/* Host (device/VM): nested Interfaces (each expands to its own IP
            addresses, mirroring the edit form) → optional orphan IP group →
            Contacts. */}
        {isHost && (
          <>
            <RelatedEntitySection
              title={interfacesTitle}
              icon={Network}
              sectionKey="interfaces"
              expandedSections={expandedSections}
              onToggle={toggleSection}
              items={network.interfaces}
              loading={network.loading}
              count={network.interfaces.length}
              countLoading={networkEager && !network.interfacesLoaded}
              renderRow={interfaceRow(interfaceEntity, network.addressesByInterfaceId, onEditRelated, t)}
              emptyText={t("connections.inspector.relations.noInterfaces")}
              t={t}
            />
            {/* Orphan IPs (interface id not in the interface list) keep a small
                fallback group so they are never silently dropped. */}
            {orphanIpAddresses.length > 0 && (
              <RelatedEntitySection
                title={t("connections.inspector.relations.otherIpAddresses")}
                icon={Network}
                sectionKey="ip-addresses-other"
                expandedSections={expandedSections}
                onToggle={toggleSection}
                items={orphanIpAddresses}
                loading={network.loading}
                count={orphanIpAddresses.length}
                countLoading={networkEager && !network.addressesLoaded}
                renderRow={ipAddressRow(onEditRelated, t)}
                emptyText={t("connections.inspector.relations.noIpAddresses")}
                t={t}
              />
            )}
          </>
        )}

        {/* Site: Devices → Racks → Locations (read-only lists). */}
        {groupType === "site" && (
          <>
            <RelatedEntitySection
              title={t("connections.inspector.relations.devices")}
              icon={Server}
              sectionKey="site-devices"
              expandedSections={expandedSections}
              onToggle={toggleSection}
              items={siteDevices.items}
              loading={siteDevices.loading}
              error={siteDevices.error}
              count={siteDevices.count}
              countLoading={siteDevices.countLoading}
              renderRow={deviceRow(onEditRelated, t)}
              emptyText={t("connections.inspector.relations.noDevices")}
              t={t}
            />
            <RelatedEntitySection
              title={t("connections.inspector.relations.racks")}
              icon={HardDrive}
              sectionKey="site-racks"
              expandedSections={expandedSections}
              onToggle={toggleSection}
              items={siteRacks.items}
              loading={siteRacks.loading}
              error={siteRacks.error}
              count={siteRacks.count}
              countLoading={siteRacks.countLoading}
              renderRow={rackRow(onEditRelated, t)}
              emptyText={t("connections.inspector.relations.noRacks")}
              t={t}
            />
            <RelatedEntitySection
              title={t("connections.inspector.relations.locations")}
              icon={MapIcon}
              sectionKey="site-locations"
              expandedSections={expandedSections}
              onToggle={toggleSection}
              items={siteLocations.items}
              loading={siteLocations.loading}
              error={siteLocations.error}
              count={siteLocations.count}
              countLoading={siteLocations.countLoading}
              renderRow={locationRow(onEditRelated, t)}
              emptyText={t("connections.inspector.relations.noLocations")}
              t={t}
            />
          </>
        )}

        {/* Region: Sites (read-only list). */}
        {groupType === "region" && (
          <RelatedEntitySection
            title={t("connections.inspector.relations.sites")}
            icon={Building2}
            sectionKey="region-sites"
            expandedSections={expandedSections}
            onToggle={toggleSection}
            items={regionSites.items}
            loading={regionSites.loading}
            error={regionSites.error}
            count={regionSites.count}
            countLoading={regionSites.countLoading}
            renderRow={siteRow(onEditRelated, t)}
            emptyText={t("connections.inspector.relations.noSites")}
            t={t}
          />
        )}

        {/* Tenant: Devices → Virtual Machines (read-only lists). */}
        {groupType === "tenant" && (
          <>
            <RelatedEntitySection
              title={t("connections.inspector.relations.devices")}
              icon={Server}
              sectionKey="tenant-devices"
              expandedSections={expandedSections}
              onToggle={toggleSection}
              items={tenantDevices.items}
              loading={tenantDevices.loading}
              error={tenantDevices.error}
              count={tenantDevices.count}
              countLoading={tenantDevices.countLoading}
              renderRow={deviceRow(onEditRelated, t)}
              emptyText={t("connections.inspector.relations.noDevices")}
              t={t}
            />
            <RelatedEntitySection
              title={t("connections.inspector.relations.virtualMachines")}
              icon={Cpu}
              sectionKey="tenant-vms"
              expandedSections={expandedSections}
              onToggle={toggleSection}
              items={tenantVms.items}
              loading={tenantVms.loading}
              error={tenantVms.error}
              count={tenantVms.count}
              countLoading={tenantVms.countLoading}
              renderRow={vmRow(onEditRelated, t)}
              emptyText={t("connections.inspector.relations.noVirtualMachines")}
              t={t}
            />
          </>
        )}

        {/* Cluster: Devices → Virtual Machines (read-only lists). */}
        {groupType === "cluster" && (
          <>
            <RelatedEntitySection
              title={t("connections.inspector.relations.devices")}
              icon={Server}
              sectionKey="cluster-devices"
              expandedSections={expandedSections}
              onToggle={toggleSection}
              items={clusterDevices.items}
              loading={clusterDevices.loading}
              error={clusterDevices.error}
              count={clusterDevices.count}
              countLoading={clusterDevices.countLoading}
              renderRow={deviceRow(onEditRelated, t)}
              emptyText={t("connections.inspector.relations.noDevices")}
              t={t}
            />
            <RelatedEntitySection
              title={t("connections.inspector.relations.virtualMachines")}
              icon={Cpu}
              sectionKey="cluster-vms"
              expandedSections={expandedSections}
              onToggle={toggleSection}
              items={clusterVms.items}
              loading={clusterVms.loading}
              error={clusterVms.error}
              count={clusterVms.count}
              countLoading={clusterVms.countLoading}
              renderRow={vmRow(onEditRelated, t)}
              emptyText={t("connections.inspector.relations.noVirtualMachines")}
              t={t}
            />
          </>
        )}

        {/* Contacts: assignments joined to contacts, filtered by entity UUID —
            read-only list (clicking a contact opens its edit form). */}
        {entitySupportsContacts(node.type) && (
          <RelatedEntitySection
            title={t("connections.inspector.relations.contacts")}
            icon={ContactIcon}
            sectionKey="contacts"
            expandedSections={expandedSections}
            onToggle={toggleSection}
            items={contacts}
            count={contactsCount}
            countLoading={contactsCountLoading}
            loading={contactsLoading || (!entityUuid && groupResolved.loading)}
            renderRow={contactRow(onEditRelated, t)}
            emptyText={t("connections.inspector.relations.noContacts")}
            t={t}
          />
        )}

        {node.type === "host" && node.host && hasConfiguredMetrics && (
          <Section title={t("observability.selectedMetrics")} icon={Gauge} sectionKey="metrics" expanded={expandedSections} onToggle={toggleSection}>
            <HostMetricsPreviewSection
              host={node.host}
              kind={node.host.targetKind === "virtual_machine" ? "vm" : "device"}
              onConfiguredMetricsChange={setHasConfiguredMetrics}
            />
          </Section>
        )}
      </div>

      </div>
      {/* CCTV devices open their console from the connect panel (the `CCTV`
          launch button), like SSH/RDP/VNC — no separate footer entry, so the
          device has one consistent open flow. */}
      {node.type === "host" && node.host && hasConfiguredMetrics && (
        <div className="flex gap-1 border-t border-border/30 p-2">
          <Button
            variant="ghost"
            size="sm"
            className="h-9 flex-1 gap-2 text-sm"
            onClick={() => setShowMetrics(true)}
          >
            <Gauge className="h-3 w-3" />
            {t("observability.metrics")}
          </Button>
        </div>
      )}
      </section>

      {node.type === "host" && node.host && hasConfiguredMetrics && (
        <HostMetricsDialog host={node.host} open={showMetrics} onOpenChange={setShowMetrics} />
      )}
    </>
  );
};

/* ── Sub-components ── */

type Translate = (k: string, opts?: Record<string, unknown>) => string;

function inspectorFieldHelp(t: Translate, key: string): string | undefined {
  const helpKey = FIELD_HELP_KEY[key];
  return helpKey ? t(helpKey) : undefined;
}

/** Local alias for the shared collapsible section shell (single source of truth
 * in {@link InspectorSection}); the inspector's many call sites keep using
 * `<Section …>`. */
const Section = InspectorSection;

/** Multiline long-text row (textarea fields) with a show all / hide toggle. */
const ExpandableTextRow = ({ icon: Icon, label, help, value, t }: {
  icon: React.ElementType; label: string; help?: string; value: string; t: Translate;
}) => {
  const [expanded, setExpanded] = useState(false);
  const isLong = value.length > 120 || value.includes("\n");
  return (
    <div className="group/field text-xs py-0.5">
      <div className="mb-0.5 flex items-start justify-between gap-2">
        <div className="min-w-0 text-muted-foreground">
          <InspectorFieldLabel icon={Icon} label={label} help={help} iconClassName="h-2.5 w-2.5" />
        </div>
        <InspectorCopyButton label={label} value={value} t={t} className="shrink-0" />
      </div>
      <p className={cn("text-foreground whitespace-pre-wrap break-words", !expanded && isLong && "line-clamp-3")}>
        {value}
      </p>
      {isLong && (
        <button
          type="button"
          onClick={() => setExpanded((v) => !v)}
          className="mt-0.5 text-xs text-primary hover:underline"
        >
          {expanded ? t("connections.inspector.hide") : t("connections.inspector.showAll")}
        </button>
      )}
    </div>
  );
};

const LoadingRow = ({ t }: { t: (k: string) => string }) => (
  <div className="flex items-center gap-2 text-xs text-muted-foreground py-1">
    <Loader2 className="h-3 w-3 animate-spin" />
    <span>{t("loading")}</span>
  </div>
);

const MutedRow = ({ text }: { text: string }) => (
  <p className="text-xs text-muted-foreground py-0.5">{text}</p>
);

function notifyClipboardWriteUnavailable(t: Translate) {
  notify("warning", t("connections.clipboard.receiveUnavailable"), {
    description: t("connections.clipboard.receiveFailedDescription"),
    source: "connections",
  });
}

const CustomFieldRows = ({ rows, t }: { rows: CustomFieldRow[]; t: Translate }) => (
  <div className="space-y-0.5">
    {rows.map((row) => (
      <CopyableInspectorFieldRow key={row.key} row={row} t={t} />
    ))}
  </div>
);

const CopyableInspectorFieldRow = ({ row, t }: { row: CustomFieldRow; t: Translate }) => (
  <div
    className="group/field grid grid-cols-[minmax(0,42%)_minmax(0,1fr)_36px] items-start gap-1.5 py-1 text-sm"
    data-custom-field-row
  >
    <div className="min-w-0 text-muted-foreground">
      <InspectorFieldLabel label={row.label} />
    </div>
    <span className="min-w-0 cursor-text select-text whitespace-pre-wrap [overflow-wrap:anywhere] text-right text-foreground">
      {row.value}
    </span>
    <InspectorCopyButton label={row.label} value={row.value} t={t} />
  </div>
);

/* ── Relation row factories ──
 * Thin functions producing a single RelatedEntityRow per item. Secondary fields
 * are plain muted text; the whole row links to the entity's edit form. */

const statusLabel = (t: Translate, status?: string): string | undefined =>
  status ? inventoryResourceStatusLabel(t, status) || undefined : undefined;

/** Optional handler that opens a related entity in the in-workspace edit form;
 *  undefined for read-only callers (rows then fall back to the inventory link). */
type OnEditRow = ((entity: InventoryEntityKey, id: string) => void) | undefined;

const contactRow = (onEdit: OnEditRow, t: Translate) => (c: InspectorContact) => {
  const contactId = c.contactId;
  return (
    <RelatedEntityRow
      key={c.id}
      icon={ContactIcon}
      label={c.name}
      secondary1={c.role}
      secondary2={c.priority ? `P${c.priority}` : undefined}
      href={inventoryResourceHref("Contacts", contactId)}
      onActivate={onEdit && contactId ? () => onEdit("Contacts", contactId) : undefined}
      linkScope="label"
      ariaLabel={`${t("connections.inspector.relations.contacts")}: ${c.name}`}
      details={contactDetails(c, t)}
    />
  );
};

const CONTACT_DETAIL_FIELDS: Array<{
  key: keyof Pick<InspectorContact, "title" | "phone" | "email" | "address" | "link" | "description" | "groupName">;
  icon: React.ElementType;
  textClassName?: string;
}> = [
  { key: "title", icon: BriefcaseBusiness },
  { key: "phone", icon: Phone, textClassName: "break-all" },
  { key: "email", icon: Mail, textClassName: "break-all" },
  { key: "address", icon: MapPin },
  { key: "link", icon: Link2, textClassName: "break-all" },
  { key: "description", icon: FileText, textClassName: "line-clamp-2" },
  { key: "groupName", icon: Users },
];

interface ContactDetailItem {
  key: keyof Pick<InspectorContact, "title" | "phone" | "email" | "address" | "link" | "description" | "groupName">;
  icon: React.ElementType;
  value: string;
  textClassName?: string;
}

const contactDetails = (contact: InspectorContact, t: Translate): React.ReactNode | undefined => {
  const details: ContactDetailItem[] = [];
  for (const { key, icon, textClassName } of CONTACT_DETAIL_FIELDS) {
    const rawValue = contact[key];
    if (typeof rawValue !== "string") continue;
    const value = rawValue.trim();
    if (!value) continue;
    details.push({ key, icon, textClassName, value });
  }

  if (details.length === 0) {
    return undefined;
  }

  return details.map(({ key, icon: Icon, value, textClassName }) => (
    <CopyableDetailRow
      key={key}
      icon={Icon}
      value={value}
      textClassName={textClassName}
      copyLabel={t("copy")}
      copiedLabel={t("copied")}
      onCopyUnavailable={() => notifyClipboardWriteUnavailable(t)}
    />
  ));
};

const interfaceRow = (
  entity: InventoryEntityKey,
  addressesByInterfaceId: Map<string, Resource[]>,
  onEdit: OnEditRow,
  t: Translate,
) => (iface: Resource) => (
  <InterfaceRow
    key={iface.id}
    entity={entity}
    iface={iface}
    addresses={addressesByInterfaceId.get(iface.id) ?? []}
    onEdit={onEdit}
    t={t}
  />
);

/**
 * Read-only master-detail interface row: the interface name keeps its inventory
 * deep-link (`linkScope="label"`), and a chevron toggle reveals the interface's
 * own IP addresses indented beneath it — mirroring the edit form's nested layout.
 * An interface with no IPs renders no chevron (nothing to expand).
 */
const InterfaceRow = ({ entity, iface, addresses, onEdit, t }: {
  entity: InventoryEntityKey;
  iface: Resource;
  addresses: Resource[];
  onEdit: OnEditRow;
  t: Translate;
}) => {
  const [expanded, setExpanded] = useState(false);
  const hasAddresses = addresses.length > 0;
  const secondary = [
    [iface.interfaceType, iface.mode].filter(Boolean).join(" · ") || undefined,
    iface.enabled != null
      ? iface.enabled === "false"
        ? t("disabled")
        : t("enabled")
      : undefined,
  ]
    .filter(Boolean)
    .join(" · ");

  return (
    <div className="border-b border-border/10 last:border-0">
      <div className="flex min-w-0 items-center gap-1">
        {hasAddresses ? (
          <button
            type="button"
            onClick={() => setExpanded((v) => !v)}
            className="inline-flex h-6 w-6 shrink-0 items-center justify-center rounded-sm text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
            aria-expanded={expanded}
            aria-label={
              expanded
                ? t("connections.inspector.collapseInterfaceAddresses")
                : t("connections.inspector.expandInterfaceAddresses")
            }
            title={
              expanded
                ? t("connections.inspector.collapseInterfaceAddresses")
                : t("connections.inspector.expandInterfaceAddresses")
            }
          >
            {expanded ? <ChevronDown className="h-3 w-3" /> : <ChevronRight className="h-3 w-3" />}
          </button>
        ) : (
          <span className="inline-flex h-6 w-6 shrink-0" aria-hidden="true" />
        )}
        <div className="min-w-0 flex-1">
          <RelatedEntityRow
            icon={Network}
            label={iface.name}
            secondary1={secondary || undefined}
            href={inventoryResourceHref(entity, iface.id)}
            onActivate={onEdit ? () => onEdit(entity, iface.id) : undefined}
            linkScope="label"
            ariaLabel={`${iface.name}`}
          />
        </div>
      </div>
      {hasAddresses && expanded && (
        <div className="ml-6 border-l border-border/20 pl-2">
          {addresses.map((addr) => ipAddressRow(onEdit, t)(addr))}
        </div>
      )}
    </div>
  );
};

const ipAddressRow = (onEdit: OnEditRow, t: Translate) => (addr: Resource) => (
  <RelatedEntityRow
    key={addr.id}
    icon={Network}
    label={addr.prefix ?? addr.primaryIp ?? addr.name}
    secondary1={statusLabel(t, addr.status)}
    href={inventoryResourceHref("IP Addresses", addr.id)}
    onActivate={onEdit ? () => onEdit("IP Addresses", addr.id) : undefined}
  />
);

const deviceRow = (onEdit: OnEditRow, t: Translate) => (d: Resource) => (
  <RelatedEntityRow
    key={d.id}
    icon={Server}
    label={d.name}
    secondary1={d.role ?? d.deviceType ?? undefined}
    secondary2={statusLabel(t, d.status)}
    href={inventoryResourceHref("Devices", d.id)}
    onActivate={onEdit ? () => onEdit("Devices", d.id) : undefined}
  />
);

const vmRow = (onEdit: OnEditRow, t: Translate) => (vm: Resource) => (
  <RelatedEntityRow
    key={vm.id}
    icon={Cpu}
    label={vm.name}
    secondary1={vm.role ?? undefined}
    secondary2={statusLabel(t, vm.status)}
    href={inventoryResourceHref("Virtual Machines", vm.id)}
    onActivate={onEdit ? () => onEdit("Virtual Machines", vm.id) : undefined}
  />
);

const rackRow = (onEdit: OnEditRow, t: Translate) => (rack: Resource) => (
  <RelatedEntityRow
    key={rack.id}
    icon={HardDrive}
    label={rack.name}
    secondary1={rack.role ?? undefined}
    secondary2={statusLabel(t, rack.status)}
    href={inventoryResourceHref("Racks", rack.id)}
    onActivate={onEdit ? () => onEdit("Racks", rack.id) : undefined}
  />
);

const locationRow = (onEdit: OnEditRow, t: Translate) => (loc: Resource) => (
  <RelatedEntityRow
    key={loc.id}
    icon={MapIcon}
    label={loc.name}
    secondary1={statusLabel(t, loc.status)}
    href={inventoryResourceHref("Locations", loc.id)}
    onActivate={onEdit ? () => onEdit("Locations", loc.id) : undefined}
  />
);

const siteRow = (onEdit: OnEditRow, t: Translate) => (site: Resource) => (
  <RelatedEntityRow
    key={site.id}
    icon={Building2}
    label={site.name}
    secondary1={site.region ?? undefined}
    secondary2={statusLabel(t, site.status)}
    href={inventoryResourceHref("Sites", site.id)}
    onActivate={onEdit ? () => onEdit("Sites", site.id) : undefined}
  />
);

function hostDetailEntity(
  targetKind: ConnectionTarget["targetKind"] | undefined,
): InventoryEntityKey {
  return targetKind === "virtual_machine" ? "Virtual Machines" : "Devices";
}

/** Synchronous host overview from the ConnectionTarget. The field set is
 * targetKind-specific, so device-only and VM-only rows cannot leak across. */
const HostOverview = ({ host, onEdit, t }: { host: ConnectionTarget; onEdit?: OnEditRow; t: Translate }) => {
  const fields = HOST_OVERVIEW_FIELDS[host.targetKind] ?? HOST_OVERVIEW_FIELDS.device;
  return (
    <>
      {fields.map((field) => {
        const raw = host[field.key];
        const value = typeof raw === "string" ? raw : undefined;
        if (!value) return null;
        const idRaw = field.idKey ? host[field.idKey] : undefined;
        const id = typeof idRaw === "string" ? idRaw : undefined;
        const href = field.relatedTab ? inventoryResourceHref(field.relatedTab, id) : null;
        const onActivate =
          onEdit && field.relatedTab && id ? () => onEdit(field.relatedTab as InventoryEntityKey, id) : undefined;
        return (
          <InspectorFieldRow
            key={field.key}
            icon={field.icon}
            label={t(field.labelKey)}
            help={inspectorFieldHelp(t, String(field.key))}
            value={value}
            mono={field.mono}
            href={href}
            onActivate={onActivate}
            t={t}
          />
        );
      })}
    </>
  );
};

const HostDetailFields = ({ targetKind, detail, raw, onEdit, t }: {
  targetKind: ConnectionTarget["targetKind"] | undefined;
  detail: Resource;
  raw?: Record<string, unknown>;
  onEdit?: OnEditRow;
  t: Translate;
}) => {
  const entity = hostDetailEntity(targetKind);
  const rows: React.ReactNode[] = [];
  if (detail.primaryIp) {
    const ipId = detail.primaryIp4Id ?? detail.primaryIp6Id;
    rows.push(
      <InspectorFieldRow
        key="primaryIp"
        icon={Network}
        label={t("inventory.resources.fields.primaryIp")}
        help={inspectorFieldHelp(t, "primaryIp")}
        value={detail.primaryIp}
        mono
        href={inventoryResourceHref("IP Addresses", ipId)}
        onActivate={onEdit && ipId ? () => onEdit("IP Addresses", ipId) : undefined}
        t={t}
      />,
    );
  }
  return (
    <>
      {rows}
      <GroupDetailFields
        entity={entity}
        detail={detail}
        raw={raw}
        host
        omitKeys={HOST_DETAIL_OMIT_KEYS}
        emptyFallback={false}
        onEdit={onEdit}
        t={t}
      />
    </>
  );
};

/** One normalized inspector row (curated or generic), pre-split by emptiness. */
interface DisplayRow {
  key: string;
  icon: React.ElementType;
  label: string;
  help?: string;
  value: string;
  /** "scalar" → single line (optionally linked); "textarea" → expandable text. */
  kind: "scalar" | "textarea";
  href?: string | null;
  /** Related-entity target for in-workspace edit (FK rows). Paired with `href`
   *  as the read-only deep-link fallback. */
  editEntity?: InventoryEntityKey;
  editId?: string;
  isEmpty: boolean;
}

function renderDisplayRow(row: DisplayRow, onEdit: OnEditRow, t: Translate): React.ReactNode {
  if (row.isEmpty) {
    return <InspectorFieldRow key={row.key} icon={row.icon} label={row.label} help={row.help} value="—" t={t} />;
  }
  if (row.kind === "textarea") {
    return <ExpandableTextRow key={row.key} icon={row.icon} label={row.label} help={row.help} value={row.value} t={t} />;
  }
  const onActivate =
    onEdit && row.editEntity && row.editId ? () => onEdit(row.editEntity as InventoryEntityKey, row.editId as string) : undefined;
  return (
    <InspectorFieldRow
      key={row.key}
      icon={row.icon}
      label={row.label}
      help={row.help}
      value={row.value}
      href={row.href}
      onActivate={onActivate}
      t={t}
    />
  );
}

/**
 * Metadata-driven detail rows showing the entity's FULL field set. Curated rows
 * (form-derived: resolved FK names, i18n labels, textarea handling) come from
 * {@link inventoryInspectorFields}; every remaining backend field (slug,
 * external_id, sync timestamps, tags, asns, custom fields, …) comes from
 * {@link genericInspectorRows} over the raw record, de-duplicated against the
 * curated keys via {@link curatedSnakeKeys}. Populated rows render inline; empty
 * fields collapse behind a toggle so "all fields" is complete without a wall of
 * blanks. The inspector owns presentation only.
 */
const GroupDetailFields = ({ entity, detail, raw, host = false, omitKeys, emptyFallback = true, onEdit, t }: {
  entity: InventoryEntityKey;
  detail: Resource;
  raw?: Record<string, unknown>;
  host?: boolean;
  omitKeys?: ReadonlySet<keyof Resource>;
  emptyFallback?: boolean;
  onEdit?: OnEditRow;
  t: Translate;
}) => {
  const [showEmpty, setShowEmpty] = useState(false);
  // Detail endpoints (`GET /:id`) return only FK ids for related fields, so the
  // curated `related` rows below resolve their display name from the cached
  // related-entity lists (shared cache, omits host-overview fields).
  const relatedNameById = useRelatedEntityNames(entity, omitKeys, true);
  const rows: DisplayRow[] = [];

  if (inventoryInspectorHasStatus(entity) && detail.status) {
    rows.push({
      key: "status",
      icon: Info,
      label: t("connections.columns.status"),
      help: inspectorFieldHelp(t, "status"),
      value: inventoryResourceStatusLabel(t, detail.status),
      kind: "scalar",
      isEmpty: false,
    });
  }

  // Curated rows: resolved FK names + i18n labels + textarea handling.
  for (const field of inventoryInspectorFields(entity)) {
    if (omitKeys?.has(field.key)) continue;
    const rawValue = detail[field.key];
    const idRaw = field.idKey ? detail[field.idKey] : undefined;
    const idStr = typeof idRaw === "string" ? idRaw : undefined;
    let value =
      typeof rawValue === "string"
        ? rawValue
        : typeof rawValue === "number" || typeof rawValue === "boolean"
          ? String(rawValue)
          : "";
    // The detail carries only the FK id for related fields; resolve its display
    // name from the cached related-entity list so the row renders as a named
    // link instead of collapsing under "show empty fields".
    if (field.kind === "related" && value === "" && idStr) {
      value = relatedNameById.get(idStr) ?? "";
    }
    rows.push({
      key: field.key,
      icon: FIELD_ICONS[field.key] ?? (field.kind === "textarea" ? FileText : Info),
      label: t(field.label, { defaultValue: field.label }),
      help: inspectorFieldHelp(t, String(field.key)),
      value,
      kind: field.kind === "textarea" ? "textarea" : "scalar",
      href: field.kind === "related" ? inventoryResourceHref(field.relatedTab, idStr) : null,
      editEntity: field.kind === "related" ? field.relatedTab : undefined,
      editId: field.kind === "related" ? idStr : undefined,
      isEmpty: value === "",
    });
  }

  // Completeness rows: every remaining backend field, deduped against curated.
  if (raw) {
    const rendered = curatedSnakeKeys(entity, host);
    for (const row of genericInspectorRows(raw, rendered)) {
      rows.push({
        key: `raw:${row.key}`,
        icon: FIELD_ICONS[row.key as keyof Resource] ?? (row.kind === "longtext" ? FileText : Info),
        label: row.i18nKey ? t(row.i18nKey, { defaultValue: row.label }) : row.label,
        help: inspectorFieldHelp(t, row.key),
        value: row.value,
        kind: row.kind === "longtext" ? "textarea" : "scalar",
        href: row.kind === "link" ? inventoryResourceHref(row.relatedTab, row.linkId) : null,
        editEntity: row.kind === "link" ? row.relatedTab : undefined,
        editId: row.kind === "link" ? row.linkId : undefined,
        isEmpty: row.isEmpty,
      });
    }
  }

  if (rows.length === 0) {
    return emptyFallback ? <MutedRow text={t("connections.inspector.detailsUnavailable")} /> : null;
  }

  const populated = rows.filter((row) => !row.isEmpty);
  const empties = rows.filter((row) => row.isEmpty);

  return (
    <>
      {populated.map((row) => renderDisplayRow(row, onEdit, t))}
      {empties.length > 0 && (
        <>
          <button
            type="button"
            onClick={() => setShowEmpty((value) => !value)}
            className="mt-1 text-xs text-primary hover:underline"
          >
            {showEmpty
              ? t("connections.inspector.hideEmptyFields")
              : t("connections.inspector.showEmptyFields", { count: empties.length })}
          </button>
          {showEmpty && empties.map((row) => renderDisplayRow(row, onEdit, t))}
        </>
      )}
    </>
  );
};

/**
 * Canonical NetBox `app_label.model` token for a contactable node, paired with
 * the target entity UUID from the SAME source so a VM never gets a device id (or
 * vice versa). Returns `undefined` for non-contactable nodes or when the entity
 * UUID is not yet resolved. For a host the device/VM split is taken from
 * `targetKind` (`virtual_machine` → `virtualization.virtualmachine` + `vmId`,
 * else `dcim.device` + `deviceId`); the bare host id is a last-resort fallback.
 */
function attachTarget(
  node: TreeNode,
): { objectType: string; objectId?: string } | undefined {
  if (node.type === "host") {
    const host = node.host;
    if (!host) return undefined;
    if (host.targetKind === "virtual_machine") {
      const objectId = host.vmId ?? host.id;
      return objectId ? { objectType: "virtualization.virtualmachine", objectId } : undefined;
    }
    const objectId = host.deviceId ?? host.id;
    return objectId ? { objectType: "dcim.device", objectId } : undefined;
  }
  const objectType = GROUP_OBJECT_TYPE[node.type];
  return objectType ? { objectType } : undefined;
}

/** Group node type → canonical NetBox content-type token (contactable types only). */
const GROUP_OBJECT_TYPE: Partial<Record<Exclude<TreeNodeType, "host">, string>> = {
  site: "dcim.site",
  region: "dcim.region",
  tenant: "tenancy.tenant",
  cluster: "virtualization.cluster",
};

/** The host FK UUID for a given group type, if the host carries it. */
function hostGroupFk(host: ConnectionTarget | undefined, type: Exclude<TreeNodeType, "host">): string | undefined {
  if (!host) return undefined;
  switch (type) {
    case "site": return host.siteId;
    case "region": return host.regionId;
    case "tenant": return host.tenantId;
    case "role": return host.roleId;
    case "platform": return host.platformId;
    case "deviceType": return host.deviceTypeId;
    case "cluster": return host.clusterId;
    default: return undefined;
  }
}

export default TreeNodeInspector;
