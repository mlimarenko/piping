import { useCallback, useMemo, useState } from "react";
import { useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  AlertTriangle,
  ChevronRight,
  GitBranch,
  Layers,
  ListTree,
  Loader2,
  Network,
  Plus,
  RefreshCw,
  Sparkles,
} from "lucide-react";
import {
  createResource,
  listIpAddressesForPrefix,
  listIpRangesForPrefix,
  listPrefixTree,
  previewNextAvailablePrefix,
  type PrefixPlanningPreview,
  type PrefixTreeNode,
} from "@/features/inventory/api";
import { inventoryResourceOption, inventoryResourceOptionsQuery } from "@/features/inventory/resourceOptions";
import type { InventoryEntityKey, Resource } from "@/shared/api/types";
import { usePermissions } from "@/shared/auth/permissionsContext";
import EntityCreateEditor from "@/shared/forms/EntityCreateEditor";
import type { EntityPickerOption } from "@/shared/forms/EntityPicker";
import type { EntityFormValues } from "@/shared/forms/entityForm";
import { relatedEntitiesForEntity } from "@/shared/forms/entityRelatedEntities";
import { IPAM_WORKSPACE_PREFIXES_TABLE_CONFIG } from "./ipamWorkspaceTableConfig";
import DataTable, { type Column } from "@/shared/ui-shared/DataTable";
import EmptyState from "@/shared/ui-shared/EmptyState";
import { PageActions } from "@/shared/ui-shared/PageChrome";
import StatusChip from "@/shared/ui-shared/StatusChip";
import SearchInput from "@/shared/ui-shared/SearchInput";
import FormSelect from "@/shared/ui-shared/FormSelect";
import SectionTabs from "@/shared/ui-shared/SectionTabs";
import { Tabs, TabsContent } from "@/shared/ui-primitives/tabs";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import { Skeleton } from "@/shared/ui-primitives/skeleton";
import { notify } from "@/shared/lib/notifications";
import { cn } from "@/shared/lib/utils";
import { useDebouncedValue } from "@/shared/hooks/useDebouncedValue";

type WorkspaceTab = "prefixes" | "ip-addresses" | "ip-ranges";
type MobilePane = "browse" | "list" | "inspect";

const ALL_STATUSES = "__all_statuses__";
const ALL_FAMILIES = "__all_families__";
const TREE_LIMIT = 300;
const DEFAULT_WORKSPACE_PAGE_SIZE = 50;
const WORKSPACE_PAGE_SIZE_OPTIONS = [25, 50, 100, 250] as const;
const EMPTY_PREFIX_TREE_NODES: PrefixTreeNode[] = [];
const EMPTY_RESOURCE_ROWS: Resource[] = [];
const PREFIX_ENTITY: InventoryEntityKey = "Prefixes";

function sameEntityList(a: InventoryEntityKey[], b: InventoryEntityKey[]): boolean {
  return a.length === b.length && a.every((value, index) => value === b[index]);
}

function clampPercent(value: number): number {
  return Math.min(100, Math.max(0, value));
}

function utilizationLabel(node: PrefixTreeNode, notCalculated: string): string {
  if (node.utilization_percent === null || node.utilization_percent === undefined) {
    return notCalculated;
  }
  return `${node.utilization_percent.toFixed(1)}%`;
}

function utilizationPercent(node: PrefixTreeNode): number {
  if (typeof node.utilization_percent === "number") {
    return clampPercent(node.utilization_percent);
  }
  return node.mark_utilized ? 100 : 0;
}

function utilizationTone(node: PrefixTreeNode): {
  barClassName: string;
  chipClassName: string;
  dotClassName: string;
} {
  const percent = utilizationPercent(node);
  if (node.mark_utilized || percent >= 90) {
    return {
      barClassName: "bg-status-negative",
      chipClassName: "text-status-negative",
      dotClassName: "bg-status-negative",
    };
  }
  if (percent >= 70) {
    return {
      barClassName: "bg-status-warning",
      chipClassName: "text-status-warning",
      dotClassName: "bg-status-warning",
    };
  }
  if (percent > 0) {
    return {
      barClassName: "bg-status-positive",
      chipClassName: "text-status-positive",
      dotClassName: "bg-status-positive",
    };
  }
  return {
    barClassName: "bg-muted-foreground/40",
    chipClassName: "text-muted-foreground",
    dotClassName: "bg-muted-foreground/50",
  };
}

function selectedPrefixLength(prefix?: string): number {
  const raw = prefix?.split("/")[1];
  const parsed = raw ? Number.parseInt(raw, 10) : NaN;
  if (!Number.isFinite(parsed)) return 24;
  return Math.min(32, parsed + 1);
}

function planningTone(status: string): "ok" | "warn" | "muted" {
  if (status === "available") return "ok";
  if (status === "invalid" || status === "exhausted" || status === "window_exhausted") return "warn";
  return "muted";
}

function prefixStatusTone(status: string): "positive" | "warning" | "negative" | "neutral" {
  if (status === "active") return "positive";
  if (status === "reserved" || status === "container") return "warning";
  if (status === "deprecated") return "negative";
  return "neutral";
}

function prefixCreateInitialValues(
  selectedNode: PrefixTreeNode | null,
  candidatePrefix?: string,
): Partial<EntityFormValues> {
  const values: Partial<EntityFormValues> = { status: "active" };
  if (candidatePrefix) values.prefix = candidatePrefix;
  if (selectedNode?.site_id) values.siteId = selectedNode.site_id;
  if (selectedNode?.vrf_id) values.vrfId = selectedNode.vrf_id;
  if (selectedNode?.tenant_id) values.tenantId = selectedNode.tenant_id;
  return values;
}

function resourceStatusTone(status?: string): "positive" | "warning" | "negative" | "neutral" {
  if (status === "active") return "positive";
  if (status === "reserved" || status === "planned" || status === "staged") return "warning";
  if (status === "deprecated" || status === "offline" || status === "decommissioning") {
    return "negative";
  }
  return "neutral";
}

const IpamWorkspace = () => {
  const { t } = useTranslation();
  const { can } = usePermissions();
  const queryClient = useQueryClient();
  const canCreatePrefix = can("CREATE", "INVENTORY");
  const [search, setSearch] = useState("");
  const [rootPrefix, setRootPrefix] = useState("");
  const [family, setFamily] = useState<"all" | "4" | "6">("all");
  const [status, setStatus] = useState("all");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [workspaceTab, setWorkspaceTab] = useState<WorkspaceTab>("prefixes");
  const [mobilePane, setMobilePane] = useState<MobilePane>("browse");
  const [ipAddressPage, setIpAddressPage] = useState(0);
  const [ipAddressPageSize, setIpAddressPageSize] = useState(DEFAULT_WORKSPACE_PAGE_SIZE);
  const [ipRangePage, setIpRangePage] = useState(0);
  const [ipRangePageSize, setIpRangePageSize] = useState(DEFAULT_WORKSPACE_PAGE_SIZE);
  const [previewDraft, setPreviewDraft] = useState<{ prefixId: string | null; length: number }>(() => ({
    prefixId: null,
    length: 24,
  }));
  const [previewRequest, setPreviewRequest] = useState<{ id: string; length: number } | null>(null);
  const [createDialogOpen, setCreateDialogOpen] = useState(false);
  const [createInitialValues, setCreateInitialValues] = useState<Partial<EntityFormValues>>({});
  const [createRelatedEntities, setCreateRelatedEntities] = useState<InventoryEntityKey[]>(() =>
    relatedEntitiesForEntity(PREFIX_ENTITY),
  );

  // Both free-text filters are debounced: they used to sit directly in the
  // query key, so every keystroke was a request. The sibling inventory screen
  // already debounced; these two were simply missed.
  const debouncedSearch = useDebouncedValue(search);
  const debouncedRootPrefix = useDebouncedValue(rootPrefix);

  const treeQuery = useQuery({
    queryKey: [
      "inventory",
      "ipam-workspace",
      "prefix-tree",
      debouncedSearch,
      debouncedRootPrefix,
      family,
      status,
    ],
    queryFn: () =>
      listPrefixTree({
        search: debouncedSearch.trim() || undefined,
        root_prefix: debouncedRootPrefix.trim() || undefined,
        family: family === "all" ? undefined : Number(family) === 4 ? 4 : 6,
        status: status === "all" ? undefined : status,
        limit: TREE_LIMIT,
      }),
  });

  const nodes = treeQuery.data?.nodes ?? EMPTY_PREFIX_TREE_NODES;
  const explicitSelectedNode = selectedId ? nodes.find((node) => node.id === selectedId) ?? null : null;
  const selectedNode = explicitSelectedNode ?? (selectedId === null ? nodes[0] ?? null : null);
  const selectionOutsideFilter = selectedId !== null && !explicitSelectedNode;
  const nodeById = useMemo(() => new Map(nodes.map((node) => [node.id, node])), [nodes]);

  const [collapsedIds, setCollapsedIds] = useState<Set<string>>(() => new Set());
  const activePreviewLength =
    selectedNode && previewDraft.prefixId === selectedNode.id
      ? previewDraft.length
      : selectedPrefixLength(selectedNode?.prefix);

  const visibleNodes = useMemo(
    () =>
      nodes.filter((node) => {
        let parentId = node.parent_id;
        const visited = new Set<string>();
        while (parentId) {
          const parent = nodeById.get(parentId);
          if (!parent || visited.has(parent.id)) return true;
          if (collapsedIds.has(parent.id)) return false;
          visited.add(parent.id);
          parentId = parent.parent_id;
        }
        return true;
      }),
    [collapsedIds, nodeById, nodes],
  );

  const childPrefixes = useMemo(() => {
    if (!selectedNode) return nodes.filter((node) => !node.parent_id);
    return nodes.filter((node) => node.parent_id === selectedNode.id);
  }, [nodes, selectedNode]);

  const ipAddressesQuery = useQuery({
    queryKey: [
      "inventory",
      "ipam-workspace",
      "ip-addresses",
      selectedNode?.id,
      selectedNode?.prefix,
      selectedNode?.vrf_id,
      ipAddressPage,
      ipAddressPageSize,
    ],
    enabled: workspaceTab === "ip-addresses" && selectedNode !== null,
    queryFn: () => {
      if (!selectedNode) throw new Error("missing selected prefix");
      return listIpAddressesForPrefix(selectedNode.prefix, {
        limit: ipAddressPageSize,
        offset: ipAddressPage * ipAddressPageSize,
        vrfId: selectedNode.vrf_id,
      });
    },
  });

  const ipRangesQuery = useQuery({
    queryKey: [
      "inventory",
      "ipam-workspace",
      "ip-ranges",
      selectedNode?.id,
      selectedNode?.prefix,
      selectedNode?.vrf_id,
      ipRangePage,
      ipRangePageSize,
    ],
    enabled: workspaceTab === "ip-ranges" && selectedNode !== null,
    queryFn: () => {
      if (!selectedNode) throw new Error("missing selected prefix");
      return listIpRangesForPrefix(selectedNode.prefix, {
        limit: ipRangePageSize,
        offset: ipRangePage * ipRangePageSize,
        vrfId: selectedNode.vrf_id,
      });
    },
  });

  const ipAddresses = ipAddressesQuery.data?.items ?? EMPTY_RESOURCE_ROWS;
  const ipRanges = ipRangesQuery.data?.items ?? EMPTY_RESOURCE_ROWS;

  const previewQuery = useQuery<PrefixPlanningPreview>({
    queryKey: ["inventory", "ipam-workspace", "planning-preview", previewRequest],
    enabled: previewRequest !== null,
    queryFn: async () => {
      if (!previewRequest) throw new Error("missing preview request");
      return previewNextAvailablePrefix(previewRequest.id, previewRequest.length);
    },
  });

  const createOptionQueries = useQueries({
    queries: createRelatedEntities.map((entity) => ({
      ...inventoryResourceOptionsQuery(entity),
      enabled: createDialogOpen,
    })),
  });

  const createOptionsByEntity = useMemo(() => {
    const map: Partial<Record<InventoryEntityKey, Resource[]>> = {};
    createRelatedEntities.forEach((entity, idx) => {
      map[entity] = createOptionQueries[idx]?.data?.items ?? [];
    });
    return map;
  }, [createOptionQueries, createRelatedEntities]);

  const createOptionStateByEntity = useMemo(() => {
    const map = new Map<InventoryEntityKey, { isPending: boolean }>();
    createRelatedEntities.forEach((entity, idx) => {
      map.set(entity, { isPending: createOptionQueries[idx]?.isPending ?? false });
    });
    return map;
  }, [createOptionQueries, createRelatedEntities]);

  const loadCreateOptions = useCallback(
    (entity: InventoryEntityKey): EntityPickerOption[] =>
      (createOptionsByEntity[entity] ?? []).map((resource) => inventoryResourceOption(entity, resource)),
    [createOptionsByEntity],
  );

  const isCreateOptionsLoading = useCallback(
    (entity: InventoryEntityKey): boolean =>
      createDialogOpen && (createOptionStateByEntity.get(entity)?.isPending ?? false),
    [createDialogOpen, createOptionStateByEntity],
  );

  const handleVisibleCreateRelatedEntitiesChange = useCallback((entities: InventoryEntityKey[]) => {
    setCreateRelatedEntities((current) => (sameEntityList(current, entities) ? current : entities));
  }, []);

  const prefixColumns = useMemo<Column<PrefixTreeNode>[]>(
    () => [
      {
        key: "prefix",
        label: t("inventory.ipamWorkspace.columns.prefix"),
        render: (node) => (
          <div className="min-w-0">
            <div className="truncate font-medium">{node.prefix}</div>
            <div className="truncate text-xs text-muted-foreground">
              {node.vrf_name ?? t("inventory.ipamWorkspace.labels.globalVrf")}
            </div>
          </div>
        ),
      },
      {
        key: "status",
        label: t("status"),
        render: (node) => <StatusChip status={prefixStatusTone(node.status)} label={node.status} />,
      },
      {
        key: "scope",
        label: t("inventory.ipamWorkspace.columns.scope"),
        render: (node) => (
          <span className="block max-w-52 truncate text-xs text-muted-foreground">
            {[node.site_name, node.tenant_name].filter(Boolean).join(" / ") ||
              t("inventory.ipamWorkspace.labels.unscoped")}
          </span>
        ),
      },
      {
        key: "utilization",
        label: t("inventory.ipamWorkspace.columns.utilization"),
        render: (node) => {
          const percent = utilizationPercent(node);
          const tone = utilizationTone(node);
          const label = utilizationLabel(node, t("inventory.ipamWorkspace.labels.notCalculated"));
          return (
            <div className="min-w-28">
              <div className="flex items-center justify-between gap-2 text-xs">
                <span className={cn("tabular-nums", tone.chipClassName)}>{label}</span>
                {node.mark_utilized ? (
                  <span className="text-xs uppercase text-status-negative">
                    {t("inventory.ipamWorkspace.labels.markUtilized")}
                  </span>
                ) : null}
              </div>
              <div className="mt-1 h-1.5 overflow-hidden rounded-full bg-muted">
                <div
                  className={cn("h-full rounded-full", tone.barClassName)}
                  role="progressbar"
                  aria-label={t("inventory.ipamWorkspace.tree.utilizationBarLabel", { prefix: node.prefix })}
                  aria-valuemin={0}
                  aria-valuemax={100}
                  aria-valuenow={percent}
                  style={{ width: `${percent}%` }}
                />
              </div>
            </div>
          );
        },
      },
      {
        key: "children",
        label: t("inventory.ipamWorkspace.columns.children"),
        render: (node) => <span className="text-xs tabular-nums">{node.child_count}</span>,
      },
    ],
    [t],
  );

  const ipAddressColumns = useMemo<Column<Resource>[]>(
    () => [
      {
        key: "address",
        label: t("inventory.ipamWorkspace.ipAddresses.columns.address"),
        render: (row) => (
          <div className="min-w-0">
            <div className="truncate font-medium">{row.prefix ?? row.name}</div>
            {row.dnsName ? (
              <div className="truncate text-xs text-muted-foreground">{row.dnsName}</div>
            ) : null}
          </div>
        ),
      },
      {
        key: "status",
        label: t("status"),
        render: (row) =>
          row.status ? <StatusChip status={resourceStatusTone(row.status)} label={row.status} /> : null,
      },
      {
        key: "assignment",
        label: t("inventory.ipamWorkspace.ipAddresses.columns.assignment"),
        render: (row) => (
          <span className="block max-w-48 truncate text-xs text-muted-foreground">
            {row.interfaceName ?? row.interfaceId ?? t("inventory.ipamWorkspace.labels.unassigned")}
          </span>
        ),
      },
      {
        key: "role",
        label: t("inventory.ipamWorkspace.ipAddresses.columns.role"),
        render: (row) => (
          <span className="block max-w-40 truncate text-xs text-muted-foreground">
            {row.ipamRole || t("inventory.ipamWorkspace.labels.none")}
          </span>
        ),
      },
      {
        key: "vrf",
        label: t("inventory.ipamWorkspace.ipAddresses.columns.vrf"),
        render: (row) => (
          <span className="block max-w-40 truncate text-xs text-muted-foreground">
            {row.vrfName ?? t("inventory.ipamWorkspace.labels.globalVrf")}
          </span>
        ),
      },
    ],
    [t],
  );

  const ipRangeColumns = useMemo<Column<Resource>[]>(
    () => [
      {
        key: "range",
        label: t("inventory.ipamWorkspace.ipRanges.columns.range"),
        render: (row) => (
          <div className="min-w-0">
            <div className="truncate font-medium">
              {row.startAddress} - {row.endAddress}
            </div>
            {row.description ? (
              <div className="truncate text-xs text-muted-foreground">{row.description}</div>
            ) : null}
          </div>
        ),
      },
      {
        key: "status",
        label: t("status"),
        render: (row) =>
          row.status ? <StatusChip status={resourceStatusTone(row.status)} label={row.status} /> : null,
      },
      {
        key: "size",
        label: t("inventory.ipamWorkspace.ipRanges.columns.size"),
        render: (row) => <span className="text-xs tabular-nums">{row.size ?? ""}</span>,
      },
      {
        key: "vrf",
        label: t("inventory.ipamWorkspace.ipRanges.columns.vrf"),
        render: (row) => (
          <span className="block max-w-40 truncate text-xs text-muted-foreground">
            {row.vrfName ?? t("inventory.ipamWorkspace.labels.globalVrf")}
          </span>
        ),
      },
      {
        key: "tenant",
        label: t("inventory.ipamWorkspace.ipRanges.columns.tenant"),
        render: (row) => (
          <span className="block max-w-40 truncate text-xs text-muted-foreground">
            {row.tenant ?? t("inventory.ipamWorkspace.labels.unscoped")}
          </span>
        ),
      },
    ],
    [t],
  );

  const resetScopedPages = () => {
    setIpAddressPage(0);
    setIpRangePage(0);
  };

  const resetFilters = () => {
    setSearch("");
    setRootPrefix("");
    setFamily("all");
    setStatus("all");
    resetScopedPages();
  };

  const openCreatePrefix = (candidatePrefix?: string) => {
    setCreateInitialValues(prefixCreateInitialValues(selectedNode, candidatePrefix));
    setCreateRelatedEntities(relatedEntitiesForEntity(PREFIX_ENTITY));
    setCreateDialogOpen(true);
  };

  const createWorkspaceEntity = async (
    entity: InventoryEntityKey,
    resource: Resource,
  ): Promise<Resource | null> => {
    try {
      const { id: _omit, ...payload } = resource;
      const created = await createResource(entity, payload);
      void queryClient.invalidateQueries({ queryKey: ["inventory", "resources", entity] });
      void queryClient.invalidateQueries({ queryKey: ["inventory", "resources", entity, "count"] });
      void queryClient.invalidateQueries({ queryKey: ["inventory", "ipam-workspace"] });
      return created;
    } catch (err) {
      notify("error", err instanceof Error ? err.message : t("unknownError"));
      return null;
    }
  };

  const handlePrefixCreated = (created: Resource) => {
    setSelectedId(created.id);
    setWorkspaceTab("prefixes");
    setMobilePane("list");
    resetScopedPages();
    setPreviewRequest(null);
    notify("success", t("inventory.resources.created", { name: created.name }));
    void treeQuery.refetch();
  };

  const selectNode = (node: PrefixTreeNode) => {
    const ancestorIds: string[] = [];
    let parentId = node.parent_id;
    while (parentId) {
      const parent = nodeById.get(parentId);
      if (!parent) break;
      ancestorIds.push(parent.id);
      parentId = parent.parent_id;
    }
    if (ancestorIds.length > 0) {
      setCollapsedIds((current) => {
        const next = new Set(current);
        for (const id of ancestorIds) {
          next.delete(id);
        }
        return next;
      });
    }
    setSelectedId(node.id);
    setPreviewDraft({ prefixId: node.id, length: selectedPrefixLength(node.prefix) });
    setMobilePane("inspect");
    resetScopedPages();
  };

  const toggleExpanded = (node: PrefixTreeNode) => {
    if (node.child_count === 0) return;
    setCollapsedIds((current) => {
      const next = new Set(current);
      if (next.has(node.id)) {
        next.delete(node.id);
      } else {
        next.add(node.id);
      }
      return next;
    });
  };

  const activePlanning =
    selectedNode && previewQuery.data?.parent_id === selectedNode.id ? previewQuery.data : null;
  const planningStatusTone = activePlanning ? planningTone(activePlanning.status) : "muted";

  return (
    <>
      <div className="flex min-w-0 flex-col gap-4">
        <PageActions>
            <div className="flex flex-wrap items-center justify-end gap-2">
              <span className="text-xs text-muted-foreground">{nodes.length < (treeQuery.data?.total ?? 0)
                ? t("inventory.ipamWorkspace.nodeCountTruncated", { shown: nodes.length, total: treeQuery.data?.total ?? 0 })
                : t("inventory.ipamWorkspace.nodeCount", { count: treeQuery.data?.total ?? 0 })}</span>
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="h-9 gap-1.5"
                onClick={() => treeQuery.refetch()}
              >
                <RefreshCw className={cn("h-3.5 w-3.5", treeQuery.isFetching && "animate-spin")} />
                {t("refresh")}
              </Button>
              {canCreatePrefix ? (
                <Button
                  type="button"
                  size="sm"
                  className="h-9 gap-1.5"
                  onClick={() => openCreatePrefix()}
                >
                  <Plus className="h-3.5 w-3.5" />
                  {t("inventory.ipamWorkspace.actions.createPrefix")}
                </Button>
              ) : null}
            </div>
        </PageActions>

      <div className="grid grid-cols-2 gap-2 lg:flex lg:flex-row lg:items-center">
          <SearchInput
            value={search}
            onValueChange={(value) => {
              setSearch(value);
              resetScopedPages();
            }}
            placeholder={t("inventory.ipamWorkspace.filters.search")}
            containerClassName="col-span-2 lg:col-span-1"
          />
        <Input
          value={rootPrefix}
          onChange={(event) => {
            setRootPrefix(event.target.value);
            resetScopedPages();
          }}
          placeholder={t("inventory.ipamWorkspace.filters.rootPrefix")}
          className="col-span-2 h-9 lg:col-span-1 lg:w-48"
        />
        <FormSelect
          value={family === "all" ? ALL_FAMILIES : family}
          onValueChange={(value) => {
            setFamily(value === ALL_FAMILIES ? "all" : value === "4" ? "4" : "6");
            resetScopedPages();
          }}
          className="lg:w-32"
          aria-label={t("inventory.ipamWorkspace.filters.allFamilies")}
          options={[
            { value: ALL_FAMILIES, label: t("inventory.ipamWorkspace.filters.allFamilies") },
            { value: "4", label: "IPv4" }, { value: "6", label: "IPv6" },
          ]}
        />
        <FormSelect
          value={status === "all" ? ALL_STATUSES : status}
          onValueChange={(value) => {
            setStatus(value === ALL_STATUSES ? "all" : value);
            resetScopedPages();
          }}
          className="lg:w-40"
          aria-label={t("inventory.ipamWorkspace.filters.allStatuses")}
          options={[
            { value: ALL_STATUSES, label: t("inventory.ipamWorkspace.filters.allStatuses") },
            ...["container", "active", "reserved", "deprecated"].map((value) => ({ value, label: value })),
          ]}
        />
        <Button type="button" variant="ghost" size="sm" className="col-span-2 h-9 lg:col-span-1" onClick={resetFilters}>
          {t("inventory.ipamWorkspace.filters.reset")}
        </Button>
      </div>

      {/* `contents`: the mobile pane tabs own their real mounted `tabpanel`s
          below, but this wrapper adds no box so the outer grid/flex chain keeps
          the exact geometry it had when the panes were plain grid children. An
          inactive wrapper is hidden below the same `lg` threshold: a
          force-mounted tabpanel whose child is only CSS-hidden would still be
          exposed as an empty tabpanel in the accessibility tree. At `lg` every
          pane is visible again, so the wrappers return to `contents`. */}
      <Tabs value={mobilePane} onValueChange={(value) => setMobilePane(value as MobilePane)} className="contents">
        <SectionTabs aria-label={t("inventory.ipamWorkspace.navLabel")} wrapperClassName="lg:hidden" items={(["browse", "list", "inspect"] as const).map((pane) => ({
          value: pane, label: t(`inventory.ipamWorkspace.mobile.${pane}`),
        }))} />

      {selectionOutsideFilter ? (
        <div className="rounded-lg border border-warning/30 bg-warning/10 px-3 py-2 text-xs text-warning-foreground">
          {t("inventory.ipamWorkspace.selectionOutsideFilter")}
        </div>
      ) : null}

      <div className="grid min-w-0 grid-cols-1 gap-4 lg:grid-cols-[minmax(220px,280px)_minmax(0,1fr)_minmax(220px,260px)]">
        <TabsContent value="browse" forceMount tabIndex={-1} className={mobilePane === "browse" ? "contents" : "hidden lg:contents"}>
        <section
          className={cn(
            "min-w-0 flex-col gap-4",
            mobilePane !== "browse" ? "hidden lg:flex" : "flex",
          )}
        >
          <div className="mb-2 flex items-center justify-between px-1">
            <h2 className="text-sm font-semibold">{t("inventory.ipamWorkspace.tree.title")}</h2>
            {treeQuery.isFetching ? <Loader2 className="h-3.5 w-3.5 animate-spin text-muted-foreground" /> : null}
          </div>
          {treeQuery.isPending ? (
            <div className="space-y-2">
              {Array.from({ length: 8 }).map((_, index) => (
                <Skeleton key={index} className="h-9 rounded-lg" />
              ))}
            </div>
          ) : treeQuery.isError ? (
            <EmptyState
              icon={AlertTriangle}
              title={t("inventory.ipamWorkspace.errorTitle")}
              description={treeQuery.error instanceof Error ? treeQuery.error.message : undefined}
              actionLabel={t("retry")}
              onAction={() => treeQuery.refetch()}
            />
          ) : nodes.length === 0 ? (
            <EmptyState
              icon={ListTree}
              title={search || rootPrefix || status !== "all" || family !== "all" ? t("inventory.ipamWorkspace.noMatchTitle") : t("inventory.ipamWorkspace.emptyTitle")}
              description={t("inventory.ipamWorkspace.emptyDescription")}
              actionLabel={t("inventory.ipamWorkspace.filters.reset")}
              onAction={resetFilters}
            />
          ) : (
            <div
              className="max-h-[32rem] space-y-1 overflow-y-auto overscroll-contain pr-1 stable-scrollbar-gutter"
              role="tree"
              aria-label={t("inventory.ipamWorkspace.tree.title")}
            >
              {visibleNodes.map((node) => {
                const hasChildren = node.child_count > 0;
                const expanded = !collapsedIds.has(node.id);
                const percent = utilizationPercent(node);
                const tone = utilizationTone(node);
                const label = utilizationLabel(node, t("inventory.ipamWorkspace.labels.notCalculated"));
                return (
                  <div
                    key={node.node_id}
                    role="treeitem"
                    aria-expanded={hasChildren ? expanded : undefined}
                    className={cn(
                      "flex min-h-[58px] w-full items-center gap-2 rounded-lg px-2 py-1.5 text-left text-sm hover:bg-accent/50",
                      selectedNode?.id === node.id && "bg-primary/10 text-primary",
                    )}
                    style={{ paddingLeft: `${Math.min(28, node.depth * 12 + 8)}px` }}
                  >
                    {hasChildren ? (
                      <Button variant="ghost" size="icon"
                        type="button"
                        className="size-9 shrink-0 text-muted-foreground hover:bg-background hover:text-foreground"
                        aria-label={t(
                          expanded
                            ? "inventory.ipamWorkspace.tree.collapse"
                            : "inventory.ipamWorkspace.tree.expand",
                          {
                            prefix: node.prefix,
                          },
                        )}
                        onClick={() => toggleExpanded(node)}
                      >
                        <ChevronRight
                          className={cn("h-3.5 w-3.5 transition-transform", expanded && "rotate-90")}
                        />
                      </Button>
                    ) : (
                      <span
                        className={cn("flex h-6 w-6 shrink-0 items-center justify-center rounded-md border border-border bg-background", tone.chipClassName)}
                        title={t("inventory.ipamWorkspace.tree.leaf")}
                        aria-hidden="true"
                      >
                        <span className={cn("h-2 w-2 rounded-full", tone.dotClassName)} />
                      </span>
                    )}
                    <Button variant="ghost" type="button" className="h-auto min-h-9 min-w-0 flex-1 flex-col items-stretch gap-1 whitespace-normal px-0 text-left" onClick={() => selectNode(node)}>
                      <div className="flex min-w-0 items-center justify-between gap-2">
                        <span className="truncate font-medium">{node.prefix}</span>
                        <span className={cn("shrink-0 text-xs font-medium tabular-nums", tone.chipClassName)}>
                          {label}
                        </span>
                      </div>
                      <div className="mt-0.5 flex min-w-0 items-center justify-between gap-2 text-xs text-muted-foreground">
                        <span className="truncate">
                          {t("inventory.ipamWorkspace.tree.children", {
                            count: node.child_count,
                          })}
                        </span>
                        {node.mark_utilized ? (
                          <span className="shrink-0 text-xs uppercase text-status-negative">
                            {t("inventory.ipamWorkspace.labels.markUtilized")}
                          </span>
                        ) : null}
                      </div>
                      <div className="mt-1.5 h-1.5 overflow-hidden rounded-full bg-muted">
                        <div
                          className={cn("h-full rounded-full", tone.barClassName)}
                          role="progressbar"
                          aria-label={t("inventory.ipamWorkspace.tree.utilizationBarLabel", { prefix: node.prefix })}
                          aria-valuemin={0}
                          aria-valuemax={100}
                          aria-valuenow={percent}
                          style={{ width: `${percent}%` }}
                        />
                      </div>
                    </Button>
                  </div>
                );
              })}
            </div>
          )}
        </section>
        </TabsContent>

        <TabsContent value="list" forceMount tabIndex={-1} className={mobilePane === "list" ? "contents" : "hidden lg:contents"}>
        <section
          className={cn(
            "min-h-0 flex-col overflow-hidden",
            mobilePane !== "list" ? "hidden lg:flex" : "flex",
          )}
        >
          <Tabs value={workspaceTab} onValueChange={(value) => {
            setWorkspaceTab(value as WorkspaceTab);
            resetScopedPages();
          }} className="flex min-h-0 flex-1 flex-col">
            <SectionTabs aria-label={t("inventory.ipamWorkspace.navLabel")} wrapperClassName="mb-2 shrink-0" items={(["prefixes", "ip-addresses", "ip-ranges"] as const).map((tab) => ({
              value: tab, label: t(`inventory.ipamWorkspace.tabs.${tab}`),
            }))} />
            <TabsContent value="prefixes" tabIndex={-1} className="contents">
            <DataTable
              tableId="inventory.ipamWorkspace.prefixes"
              className="min-h-0 flex-1"
              columns={prefixColumns}
              data={childPrefixes}
              getRowKey={(node) => node.id}
              selectedRow={selectedNode}
              onRowClick={selectNode}
              density="compact"
              loading={treeQuery.isPending}
              error={treeQuery.isError ? treeQuery.error : undefined}
              onRetry={() => treeQuery.refetch()}
              nonHideableKeys={[...IPAM_WORKSPACE_PREFIXES_TABLE_CONFIG.sortKeys]}
              emptyState={
                <EmptyState
                  icon={GitBranch}
                  title={t("inventory.ipamWorkspace.table.emptyTitle")}
                  description={t("inventory.ipamWorkspace.table.emptyDescription")}
                />
              }
            />
            </TabsContent>
            <TabsContent value="ip-addresses" tabIndex={-1} className="contents">
            <DataTable
              className="min-h-0 flex-1"
              columns={ipAddressColumns}
              data={ipAddresses}
              getRowKey={(row) => row.id}
              density="compact"
              loading={selectedNode !== null && (ipAddressesQuery.isPending || ipAddressesQuery.isFetching)}
              error={selectedNode !== null ? ipAddressesQuery.error : null}
              onRetry={() => ipAddressesQuery.refetch()}
              pagination={{
                page: ipAddressPage,
                pageSize: ipAddressPageSize,
                total: ipAddressesQuery.data?.total ?? 0,
                pageSizeOptions: WORKSPACE_PAGE_SIZE_OPTIONS,
                onPageChange: setIpAddressPage,
                onPageSizeChange: (size) => {
                  setIpAddressPageSize(size);
                  setIpAddressPage(0);
                },
              }}
              emptyState={
                <EmptyState
                  icon={Network}
                  title={t("inventory.ipamWorkspace.ipAddresses.emptyTitle")}
                  description={t("inventory.ipamWorkspace.ipAddresses.emptyDescription", {
                    prefix: selectedNode?.prefix ?? "",
                  })}
                />
              }
            />
            </TabsContent>
            <TabsContent value="ip-ranges" tabIndex={-1} className="contents">
            <DataTable
              className="min-h-0 flex-1"
              columns={ipRangeColumns}
              data={ipRanges}
              getRowKey={(row) => row.id}
              density="compact"
              loading={selectedNode !== null && (ipRangesQuery.isPending || ipRangesQuery.isFetching)}
              error={selectedNode !== null ? ipRangesQuery.error : null}
              onRetry={() => ipRangesQuery.refetch()}
              pagination={{
                page: ipRangePage,
                pageSize: ipRangePageSize,
                total: ipRangesQuery.data?.total ?? 0,
                pageSizeOptions: WORKSPACE_PAGE_SIZE_OPTIONS,
                onPageChange: setIpRangePage,
                onPageSizeChange: (size) => {
                  setIpRangePageSize(size);
                  setIpRangePage(0);
                },
              }}
              emptyState={
                <EmptyState
                  icon={Layers}
                  title={t("inventory.ipamWorkspace.ipRanges.emptyTitle")}
                  description={t("inventory.ipamWorkspace.ipRanges.emptyDescription", {
                    prefix: selectedNode?.prefix ?? "",
                  })}
                />
              }
            />
            </TabsContent>
          </Tabs>
        </section>
        </TabsContent>

        <TabsContent value="inspect" forceMount tabIndex={-1} className={mobilePane === "inspect" ? "contents" : "hidden lg:contents"}>
        <aside
          className={cn(
            "min-w-0 flex-col",
            mobilePane !== "inspect" ? "hidden lg:flex" : "flex",
          )}
        >
          {selectedNode ? (
            <div className="space-y-4">
              <div className="space-y-1">
                <div className="flex items-center justify-between gap-2">
                  <h2 className="truncate font-mono text-sm font-semibold">{selectedNode.prefix}</h2>
                  <StatusChip
                    status={prefixStatusTone(selectedNode.status)}
                    label={selectedNode.status}
                  />
                </div>
                <p className="text-xs text-muted-foreground">
                  {selectedNode.breadcrumbs.join(" / ")}
                </p>
              </div>

              <div className="rounded-lg border border-border bg-background p-3">
                <div className="mb-2 flex items-center justify-between gap-2 text-xs">
                  <span className="text-muted-foreground">{t("inventory.ipamWorkspace.columns.utilization")}</span>
                  <span className={cn("font-medium tabular-nums", utilizationTone(selectedNode).chipClassName)}>
                    {utilizationLabel(selectedNode, t("inventory.ipamWorkspace.labels.notCalculated"))}
                  </span>
                </div>
                <div className="h-2 overflow-hidden rounded-full bg-muted">
                  <div
                    className={cn("h-full rounded-full", utilizationTone(selectedNode).barClassName)}
                    role="progressbar"
                    aria-label={t("inventory.ipamWorkspace.tree.utilizationBarLabel", { prefix: selectedNode.prefix })}
                    aria-valuemin={0}
                    aria-valuemax={100}
                    aria-valuenow={utilizationPercent(selectedNode)}
                    style={{ width: `${utilizationPercent(selectedNode)}%` }}
                  />
                </div>
              </div>

              <dl className="grid grid-cols-2 gap-3 text-xs">
                <div>
                  <dt className="text-muted-foreground">{t("inventory.ipamWorkspace.inspector.family")}</dt>
                  <dd className="font-medium">IPv{selectedNode.family}</dd>
                </div>
                <div>
                  <dt className="text-muted-foreground">{t("inventory.ipamWorkspace.columns.children")}</dt>
                  <dd className="font-medium tabular-nums">{selectedNode.child_count}</dd>
                </div>
                <div>
                  <dt className="text-muted-foreground">{t("inventory.ipamWorkspace.labels.markUtilized")}</dt>
                  <dd className="font-medium">{selectedNode.mark_utilized ? t("yes") : t("no")}</dd>
                </div>
              </dl>

              <div className="rounded-lg border border-border bg-background p-3">
                <h3 className="mb-2 text-sm font-semibold">{t("inventory.ipamWorkspace.planning.title")}</h3>
                <div className="flex gap-2">
                  <Input
                    type="number"
                    min={selectedNode.family === 4 ? selectedPrefixLength(selectedNode.prefix) : 1}
                    max={selectedNode.family === 4 ? 32 : 128}
                    value={activePreviewLength}
                    onChange={(event) => {
                      const parsed = Number.parseInt(event.target.value, 10);
                      setPreviewDraft({
                        prefixId: selectedNode.id,
                        length: Number.isFinite(parsed) ? parsed : selectedPrefixLength(selectedNode.prefix),
                      });
                    }}
                    className="h-9"
                    aria-label={t("inventory.ipamWorkspace.planning.prefixLength")}
                  />
                  <Button
                    type="button"
                    size="sm"
                    className="h-9 shrink-0 gap-1.5"
                    onClick={() => setPreviewRequest({ id: selectedNode.id, length: activePreviewLength })}
                  >
                    {previewQuery.isFetching ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <Sparkles className="h-3.5 w-3.5" />}
                    {t("inventory.ipamWorkspace.planning.preview")}
                  </Button>
                </div>
                {activePlanning ? (
                  <div
                    className={cn(
                      "mt-3 rounded-md border px-2.5 py-2 text-xs",
                      planningStatusTone === "ok" && "border-success/30 bg-success/10",
                      planningStatusTone === "warn" && "border-warning/30 bg-warning/10",
                      planningStatusTone === "muted" && "border-border bg-muted",
                    )}
                  >
                    <div className="font-medium">
                      {activePlanning.candidate_prefix ??
                        t(`inventory.ipamWorkspace.planning.status.${activePlanning.status}`, {
                          defaultValue: activePlanning.status,
                        })}
                    </div>
                    {activePlanning.reason ? (
                      <div className="mt-1 text-muted-foreground">
                        {t(`inventory.ipamWorkspace.planning.reason.${activePlanning.reason}`, {
                          defaultValue: activePlanning.reason,
                        })}
                      </div>
                    ) : null}
                    {activePlanning.candidate_prefix && canCreatePrefix ? (
                      <Button
                        type="button"
                        variant="outline"
                        size="sm"
                        className="mt-2 h-9 w-full gap-1.5"
                        onClick={() => openCreatePrefix(activePlanning.candidate_prefix ?? undefined)}
                      >
                        <Plus className="h-3.5 w-3.5" />
                        {t("inventory.ipamWorkspace.planning.createCandidate")}
                      </Button>
                    ) : null}
                  </div>
                ) : null}
              </div>
            </div>
          ) : (
            <EmptyState
              icon={Network}
              title={t("inventory.ipamWorkspace.inspector.emptyTitle")}
              description={t("inventory.ipamWorkspace.inspector.emptyDescription")}
            />
          )}
        </aside>
        </TabsContent>
      </div>
      </Tabs>
    </div>
      {canCreatePrefix ? (
        <EntityCreateEditor
          open={createDialogOpen}
          onOpenChange={setCreateDialogOpen}
          rootEntity={PREFIX_ENTITY}
          initialValues={createInitialValues}
          createEntity={createWorkspaceEntity}
          loadOptions={loadCreateOptions}
          isOptionsLoading={isCreateOptionsLoading}
          onVisibleRelatedEntitiesChange={handleVisibleCreateRelatedEntitiesChange}
          onRootCreated={handlePrefixCreated}
        />
      ) : null}
    </>
  );
};

export default IpamWorkspace;
