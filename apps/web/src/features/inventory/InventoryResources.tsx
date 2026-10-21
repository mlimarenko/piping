import { startTransition, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation, Trans } from "react-i18next";
import type { TFunction } from "i18next";
import { useLocation, useNavigate, useSearchParams } from "react-router";
import { useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { useApiMutation } from "@/shared/api/hooks";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { RequireCapability } from "@/shared/auth/permissions";
import IdentitySelection from "@/features/access-control/components/access/IdentitySelection";
import { CONNECTION_TARGETS_QUERY_KEY } from "@/features/connections/api";
import { useTablePageSize } from "@/shared/hooks/useTablePageSize";
import PageChrome from "@/shared/ui-shared/PageChrome";
import PageLayout from "@/shared/ui-shared/PageLayout";
import RowActions from "@/shared/ui-shared/RowActions";
import { useInventoryRowActionFocus } from "./useInventoryRowActionFocus";
import { DropdownMenuItem } from "@/shared/ui-primitives/dropdown-menu";
import DataTable, { type DataTableSortState, type DataTableFilterOption } from "@/shared/ui-shared/DataTable";
import {
  sanitizeDataTableColumnFilters,
  sanitizeDataTableSortState,
  updateDataTableColumnFilters,
} from "@/shared/ui-shared/tableState";
import StatusChip from "@/shared/ui-shared/StatusChip";
import EmptyState from "@/shared/ui-shared/EmptyState";
import { Button } from "@/shared/ui-primitives/button";
import SearchInput from "@/shared/ui-shared/SearchInput";
import SectionLinks from "@/shared/ui-shared/SectionLinks";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/shared/ui-primitives/alert-dialog";
import {
  Eye,
  Plus,
  Trash2,
  Pencil,
  Database,
  RefreshCw,
  Loader2,
  Layers,
  Cable,
  Network,
  CircleAlert,
  ArrowUpRight,
} from "lucide-react";
import {
  TooltipProvider,
} from "@/shared/ui-primitives/tooltip";
import { notify } from "@/shared/lib/notifications";
import {
  type Resource,
  tabConfig,
  fieldsByTab,
  inventoryEntityIsReadOnly,
} from "@/features/inventory/components/inventory/inventoryTypes";
import {
  inventoryCategories,
  CATEGORIZED_ENTITIES,
  firstEntityOf,
  categoryOf,
  isConfigContextsRailKey,
  isIpamWorkspaceRailKey,
  isCablingWorkstationRailKey,
  isPseudoInventoryRailKey,
  DEFAULT_CATEGORY,
  type RailEntity,
} from "@/features/inventory/components/inventory/inventoryCategories";
import ConfigContexts from "@/features/inventory/ConfigContexts";
import IpamWorkspace from "@/features/inventory/components/ipam-workspace/IpamWorkspace";
import CablingWorkstation from "@/features/inventory/components/cabling-workstation/CablingWorkstation";
import {
  parseInventoryRailEntity,
  inventoryEntityParams,
  inventoryResourceNewPath,
  inventoryResourceEditPath,
  inventoryResourceDetailHref,
} from "@/features/inventory/resourceLinks";
import { inventoryResourceStatusLabel, inventoryResourceStatusTone } from "@/features/inventory/resourceStatus";
import { useHostLabels } from "@/features/observability/useHostLabels";
import { HostObservabilityCluster } from "@/features/observability/HostObservabilityHoverCard";
import BulkEditDialog, {
  type BulkEditPatch,
} from "@/features/inventory/components/inventory/BulkEditDialog";
import EntityCreateEditor from "@/shared/forms/EntityCreateEditor";
import type { EntityPickerOption } from "@/shared/forms/EntityPicker";
import {
  bulkUpdateResources,
  createResource,
  deleteResource,
  listAllResources,
  listResources,
  listResourcesForTable,
} from "@/features/inventory/api";
import { inventoryResourceOption } from "@/features/inventory/resourceOptions";
import {
  EMPTY_CONFIG,
  customFieldContentTypeTokenFor,
  inventoryColumnKeyForField,
  inventoryCustomFieldColumnKey,
  inventoryCustomFieldDataColumns,
  inventoryFieldRendersAsDataColumn,
  inventoryIdentityColumnLabel,
  inventoryIdentityFieldForColumns,
  inventoryNameColumnKey,
  inventoryTableConfigFor,
  inventoryTableConfigWithCustomFields,
  type InventoryFilterColumn,
} from "@/features/inventory/tableConfig";
import {
  listChoiceSets,
  listContentTypes,
  listCustomFields,
} from "@/features/inventory/custom-fields-api";
import {
  inventoryEntityHasStatus,
  type InventoryEntityKey,
  type ObjectRefProjection,
} from "@/shared/api/types";
import {
  fallbackBlockerLabel,
  parseDeleteBlockedDetails,
  type DeleteBlockedDetails,
  type DeleteBlockedGroup,
} from "@/features/inventory/deleteBlockers";
import { inventoryEntityIconFor } from "@/shared/entity-icon-utils";
import { EntityIcon } from "@/shared/entity-icons";
import { ApiError } from "@/shared/api/client";

/** Icon/singular lookup keyed by entity, derived from the shared tabConfig. */
const tabConfigByKey = new Map<InventoryEntityKey, (typeof tabConfig)[number]>(
  tabConfig.map((cfg) => [cfg.key, cfg]),
);

const sameEntityList = (
  a: InventoryEntityKey[],
  b: InventoryEntityKey[],
): boolean => a.length === b.length && a.every((entity, idx) => entity === b[idx]);

const fkEntitiesForColumn = (
  column: Extract<InventoryFilterColumn, { type: "fk" }>,
): InventoryEntityKey[] => (Array.isArray(column.fkEntity) ? column.fkEntity : [column.fkEntity]);

const labelForFkFilterOption = (
  entity: InventoryEntityKey,
  resource: Resource,
  t: TFunction,
  qualifySource: boolean,
): string => {
  const source = qualifySource
    ? t(`inventory.resources.entities.${entity}.one`, { defaultValue: entity })
    : null;
  const withSource = (label: string) => (source ? `${source} · ${label}` : label);

  if (entity === "Interfaces") {
    return withSource(
      [resource.name, resource.deviceName].filter((part): part is string => Boolean(part)).join(" · "),
    );
  }
  if (entity === "VM Interfaces") {
    return withSource(
      [resource.name, resource.virtualMachineName].filter((part): part is string => Boolean(part)).join(" · "),
    );
  }
  return withSource(resource.name);
};

const clampPercent = (value: number): number => Math.max(0, Math.min(100, value));

interface BulkDeleteFailure {
  resourceId: string;
  resourceName: string;
  errorMessage: string;
  details: DeleteBlockedDetails | null;
}

interface BulkEditFailure {
  resourceId: string;
  resourceName: string;
  reason: string;
}

const DeleteBlockedPanel = ({
  details,
  blockerGroupLabel,
  onOpenRelated,
}: {
  details: DeleteBlockedDetails;
  blockerGroupLabel: (group: DeleteBlockedGroup) => string;
  onOpenRelated: (entity: InventoryEntityKey, resourceId: string) => void;
}) => {
  const { t } = useTranslation();

  return (
    <div className="space-y-3">
      <div className="rounded-lg border border-warning/30 bg-warning/10 p-3 text-sm text-foreground">
        <div className="flex items-start gap-2">
          <CircleAlert className="mt-0.5 h-4 w-4 shrink-0 text-warning" />
          <div className="space-y-1">
            <p className="font-medium">
              {t("inventory.resources.deleteBlockedSummary", {
                count: details.totalCount,
              })}
            </p>
            <p className="text-xs text-muted-foreground">
              {t("inventory.resources.deleteBlockedTypeSummary", {
                count: details.blockers.length,
              })}
            </p>
            <p className="text-xs text-muted-foreground">
              {t("inventory.resources.deleteBlockedHint")}
            </p>
          </div>
        </div>
      </div>

      <div className="space-y-2">
        {details.blockers.map((group) => (
          <div key={group.table} className="rounded-lg border border-border bg-accent/10 p-3">
            <div className="flex items-center justify-between gap-3">
              <div className="min-w-0">
                <p className="truncate text-sm font-medium">{blockerGroupLabel(group)}</p>
                <p className="text-xs text-muted-foreground">
                  {t("inventory.resources.deleteBlockedCount", { count: group.count })}
                </p>
              </div>
            </div>

            {group.examples.length > 0 ? (
              <div className="mt-2 space-y-1">
                {group.examples.map((example, index) => (
                  <div
                    key={`${group.table}:${example.id ?? index}:${example.label}`}
                    className="flex items-center justify-between gap-2 rounded-md border border-border bg-background px-2.5 py-2 text-sm"
                  >
                    <span className="truncate">{example.label}</span>
                    {group.entityKey && example.id ? (
                      <Button
                        type="button"
                        variant="ghost"
                        size="sm"
                        className="h-9 shrink-0 gap-1 px-2 text-sm"
                        aria-label={t("inventory.resources.openRelatedResource", {
                          name: example.label,
                        })}
                        onClick={() => onOpenRelated(group.entityKey!, example.id!)}
                      >
                        <ArrowUpRight className="h-3.5 w-3.5" />
                        {t("open")}
                      </Button>
                    ) : null}
                  </div>
                ))}
                {group.count > group.examples.length ? (
                  <p className="px-1 text-xs text-muted-foreground">
                    {t("inventory.resources.deleteBlockedRemaining", {
                      count: group.count - group.examples.length,
                    })}
                  </p>
                ) : null}
              </div>
            ) : null}
          </div>
        ))}
      </div>
    </div>
  );
};

const PrefixUtilizationCell = ({
  resource,
  labelUsedOfTotal,
  labelUsedOnly,
  labelFree,
  labelMarked,
}: {
  resource: Resource;
  labelUsedOfTotal: (used: number, total: number) => string;
  labelUsedOnly: (used: number) => string;
  labelFree: (free: number) => string;
  labelMarked: string;
}) => {
  const used = resource.prefixUtilizedAddresses ?? 0;
  const usable = resource.prefixUsableAddresses;
  const percent =
    typeof resource.prefixUtilizationPercent === "number"
      ? clampPercent(resource.prefixUtilizationPercent)
      : resource.markUtilized === "true" && typeof usable === "number" && usable > 0
        ? 100
        : 0;
  const free = typeof usable === "number" ? Math.max(usable - used, 0) : undefined;
  const percentLabel = `${Math.round(percent)}%`;
  const meterTone =
    percent >= 90
      ? "bg-status-negative"
      : percent >= 70
        ? "bg-status-warning"
        : percent > 0
          ? "bg-primary"
          : "bg-muted-foreground/40";

  return (
    <div className="min-w-[11rem] max-w-[15rem] space-y-1">
      <div className="flex items-center justify-between gap-2 text-xs">
        <span className="truncate font-medium text-foreground">
          {typeof usable === "number" ? labelUsedOfTotal(used, usable) : labelUsedOnly(used)}
        </span>
        <span className="shrink-0 tabular-nums text-muted-foreground">{percentLabel}</span>
      </div>
      <div className="h-1.5 overflow-hidden rounded-full bg-muted">
        <div
          className={`h-full rounded-full ${meterTone}`}
          style={{ width: `${percent}%` }}
        />
      </div>
      <div className="flex items-center justify-between gap-2 text-xs text-muted-foreground">
        <span className="truncate">
          {resource.markUtilized === "true" ? labelMarked : free != null ? labelFree(free) : "—"}
        </span>
      </div>
    </div>
  );
};

const renderCustomFieldValue = (value: unknown, notSetLabel: string) => {
  if (value === null || value === undefined || value === "") {
    return <span className="text-muted-foreground italic text-xs">{notSetLabel}</span>;
  }
  if (typeof value === "boolean") {
    return <span className="text-muted-foreground">{value ? "True" : "False"}</span>;
  }
  if (Array.isArray(value)) {
    if (value.length === 0) {
      return <span className="text-muted-foreground italic text-xs">{notSetLabel}</span>;
    }
    return <span className="text-muted-foreground">{value.map(String).join(", ")}</span>;
  }
  if (typeof value === "object") {
    return <span className="text-muted-foreground">{JSON.stringify(value)}</span>;
  }
  if (typeof value === "string" || typeof value === "number" || typeof value === "bigint") {
    return <span className="text-muted-foreground">{value.toString()}</span>;
  }
  return <span className="text-muted-foreground">{notSetLabel}</span>;
};

const InventoryResourcesBody = ({ activeTab, onFallbackTabChange }: {
  activeTab: RailEntity;
  onFallbackTabChange: (tab: RailEntity) => void;
}) => {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const { triggerProps, restoreFocus } = useInventoryRowActionFocus();
  // View-but-not-edit: a READ-only inventory role sees the list/filters but no
  // create/edit/delete/bulk-mutate affordances — hidden, not 403-on-click.
  const { can } = usePermissions();
  const canCreate = can("CREATE", "INVENTORY");
  const canRead = can("READ", "INVENTORY");
  const canUpdate = can("UPDATE", "INVENTORY");
  const canDelete = can("DELETE", "INVENTORY");
  const navigate = useNavigate();
  const location = useLocation();
  const activeCategory = categoryOf(activeTab);
  // Pseudo-entities render bespoke panes, not the standard
  // entity table/form flow; many hooks below are gated on this.
  const isConfigContextsTab = isConfigContextsRailKey(activeTab);
  const isIpamWorkspaceTab = isIpamWorkspaceRailKey(activeTab);
  const isCablingWorkstationTab = isCablingWorkstationRailKey(activeTab);
  // The active entity as a *real* inventory key, or null for the pseudo-entity.
  // Standard CRUD queries/columns key off this so the special key never reaches
  // `listResources` / `ENTITY_API` (which are indexed by `InventoryEntityKey`).
  const activeEntity: InventoryEntityKey | null = isPseudoInventoryRailKey(activeTab)
    ? null
    : activeTab;
  // A read-only entity (no backend write routes) shows no create/edit/delete
  // affordances even for a caller holding the inventory write capability.
  const activeEntityIsReadOnly = activeEntity !== null && inventoryEntityIsReadOnly(activeEntity);
  const canWriteActiveEntity = activeEntity !== null && !activeEntityIsReadOnly;
  const canCreateActiveEntity = canCreate && canWriteActiveEntity;
  const [search, setSearch] = useState("");
  const searchInput = useRef<HTMLInputElement>(null);
  const resetFocusPending = useRef(false);
  const [selectedKeys, setSelectedKeys] = useState<Set<string>>(new Set());

  // Server-pagination state for the active tab.
  const [page, setPage] = useState(0);
  const { value: pageSize, setValue: setPageSize } = useTablePageSize();

  // Backend-driven sort + multi-value column filters for the active tab. Sort is
  // a single allowlisted column key + direction; filters map a table column key
  // to its selected values. Both are forwarded to the table-fetch path and reset
  // (with the page) when the active entity changes.
  const [sortState, setSortState] = useState<DataTableSortState | null>(null);
  const [columnFilters, setColumnFilters] = useState<Record<string, string[]>>({});

  // Debounced search value threaded into the server query (avoid a request per keystroke).
  const [debouncedSearch, setDebouncedSearch] = useState(search);
  const debounceTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => {
    if (debounceTimer.current !== null) clearTimeout(debounceTimer.current);
    debounceTimer.current = setTimeout(() => setDebouncedSearch(search), 250);
    return () => {
      if (debounceTimer.current !== null) clearTimeout(debounceTimer.current);
    };
  }, [search]);

  const [deleteTarget, setDeleteTarget] = useState<Resource | null>(null);
  const [deleteBlockedDetails, setDeleteBlockedDetails] = useState<DeleteBlockedDetails | null>(null);
  const [bulkDeleteOpen, setBulkDeleteOpen] = useState(false);
  const [bulkDeleteFailures, setBulkDeleteFailures] = useState<BulkDeleteFailure[] | null>(null);
  const [bulkEditOpen, setBulkEditOpen] = useState(false);
  const [bulkEditFailures, setBulkEditFailures] = useState<BulkEditFailure[] | null>(null);
  const [bulkRelatedEntities, setBulkRelatedEntities] = useState<InventoryEntityKey[]>([]);
  const [bulkCreateRelatedEntities, setBulkCreateRelatedEntities] = useState<InventoryEntityKey[]>([]);
  // Recursive create launched from inside the bulk-edit dialog: roots a separate
  // EntityCreateEditor at the related entity without disturbing the page.
  const [bulkCreateEntity, setBulkCreateEntity] = useState<InventoryEntityKey | null>(null);

  // Reset tab-scoped UI when the URL-derived active entity changes. This guarded
  // render-time reset follows React's "adjust state while rendering" pattern and
  // avoids cascading effects.
  const [lastActiveTab, setLastActiveTab] = useState(activeTab);
  if (lastActiveTab !== activeTab) {
    setLastActiveTab(activeTab);
    setSearch("");
    setDebouncedSearch("");
    setPage(0);
    setSortState(null);
    setColumnFilters({});
    setSelectedKeys(new Set());
  }

  const handleBulkRelatedEntitiesChange = useCallback((entities: InventoryEntityKey[]) => {
    setBulkRelatedEntities((prev) => (sameEntityList(prev, entities) ? prev : entities));
  }, []);

  const handleBulkCreateRelatedEntitiesChange = useCallback((entities: InventoryEntityKey[]) => {
    setBulkCreateRelatedEntities((prev) => (sameEntityList(prev, entities) ? prev : entities));
  }, []);

  // Navigation counts use envelope totals without downloading each collection.
  const countQueries = useQueries({
    queries: CATEGORIZED_ENTITIES.map((entity) => ({
      queryKey: ["inventory", "resources", entity, "count"],
      queryFn: () => listResources(entity, { limit: 1 }),
    })),
  });
  const countByTab = useMemo(() => {
    return new Map(CATEGORIZED_ENTITIES.map((entity, idx) => [entity, countQueries[idx]]));
  }, [countQueries]);

  // Deferred option lists for only the related entities visible in the active
  // form(s). This avoids opening one inventory form and fanning out a full-list
  // request for every resource type. Query keys stay under the per-entity prefix
  // so create/delete invalidations still refresh affected pickers.
  const optionEntities = useMemo(() => {
    const entities = new Set<InventoryEntityKey>();
    if (bulkEditOpen) bulkRelatedEntities.forEach((entity) => entities.add(entity));
    if (bulkCreateEntity) bulkCreateRelatedEntities.forEach((entity) => entities.add(entity));
    return Array.from(entities);
  }, [
    bulkCreateEntity,
    bulkCreateRelatedEntities,
    bulkEditOpen,
    bulkRelatedEntities,
  ]);

  const optionQueries = useQueries({
    queries: optionEntities.map((entity) => ({
      queryKey: ["inventory", "resources", entity, "options"],
      queryFn: () => listResources(entity),
    })),
  });
  const byTab = useMemo(() => {
    const map: Partial<Record<InventoryEntityKey, Resource[]>> = {};
    optionEntities.forEach((entity, idx) => {
      map[entity] = optionQueries[idx]?.data?.items ?? [];
    });
    return map;
  }, [optionEntities, optionQueries]);

  const optionQueryStateByEntity = useMemo(() => {
    const map = new Map<InventoryEntityKey, { isPending: boolean }>();
    optionEntities.forEach((entity, idx) => {
      map.set(entity, { isPending: optionQueries[idx]?.isPending ?? false });
    });
    return map;
  }, [optionEntities, optionQueries]);

  const countBadge = (entities: readonly RailEntity[]) => {
    const queries = entities.filter((entity) => !isPseudoInventoryRailKey(entity))
      .map((entity) => countByTab.get(entity));
    if (queries.length === 0) return undefined;
    if (queries.every((query) => query?.data !== undefined)) {
      return queries.reduce((total, query) => total + (query?.data?.total ?? 0), 0);
    }
    const unavailable = queries.some((query) => query?.isError);
    const label = t(unavailable ? "unknownError" : "loading");
    return <span aria-label={label} title={label}>{unavailable ? "—" : "…"}</span>;
  };

  // Per-entity sort/filter capability (devices fully wired; others empty →
  // sort/filter UI degrades to off). Drives which columns are sortable, which
  // expose a filter menu, and the column-key → backend filter-param mapping.
  // The config-layers pseudo-entity has no table (it renders the embedded
  // ConfigContexts pane), so it carries no sort/filter config.
  const baseTableConfig = activeEntity ? inventoryTableConfigFor(activeEntity) : EMPTY_CONFIG;
  const customFieldContentTypeToken = customFieldContentTypeTokenFor(activeEntity);
  const contentTypesQuery = useQuery({
    queryKey: ["inventory", "content-types", "custom-field-table"],
    queryFn: () => listContentTypes(),
    enabled: Boolean(customFieldContentTypeToken),
    staleTime: 60_000,
  });
  const contentTypes = useMemo(() => contentTypesQuery.data ?? [], [contentTypesQuery.data]);
  const activeCustomFieldContentType = useMemo(
    () => contentTypes.find((type) => type.token === customFieldContentTypeToken),
    [contentTypes, customFieldContentTypeToken],
  );
  const customFieldsQuery = useQuery({
    queryKey: ["custom-fields", "inventory-table", activeCustomFieldContentType?.id ?? null],
    queryFn: () =>
      listCustomFields({
        limit: 1000,
        offset: 0,
        object_type_id: activeCustomFieldContentType?.id,
        sort: "weight",
        order: "asc",
      }),
    enabled: Boolean(activeCustomFieldContentType?.id),
    staleTime: 60_000,
  });
  const customFieldDefinitions = useMemo(
    () => customFieldsQuery.data?.items ?? [],
    [customFieldsQuery.data],
  );
  const customFieldChoiceSetIds = useMemo(
    () =>
      new Set(
        customFieldDefinitions
          .map((definition) => definition.choice_set_id)
          .filter((id): id is string => Boolean(id)),
      ),
    [customFieldDefinitions],
  );
  const choiceSetsQuery = useQuery({
    queryKey: ["custom-field-choice-sets", "inventory-table"],
    queryFn: () => listChoiceSets({ limit: 1000, offset: 0, sort: "name", order: "asc" }),
    enabled: customFieldChoiceSetIds.size > 0,
    staleTime: 60_000,
  });
  const choiceSets = useMemo(() => choiceSetsQuery.data?.items ?? [], [choiceSetsQuery.data]);
  const tableConfig = useMemo(
    () =>
      inventoryTableConfigWithCustomFields(
        baseTableConfig,
        activeEntity,
        customFieldDefinitions,
        choiceSets,
        contentTypes,
      ),
    [activeEntity, baseTableConfig, choiceSets, contentTypes, customFieldDefinitions],
  );
  const sortableKeySet = useMemo(() => new Set(tableConfig.sortKeys), [tableConfig.sortKeys]);
  const filterColumnByKey = useMemo(() => {
    const map = new Map<string, InventoryFilterColumn>();
    tableConfig.filterColumns.forEach((col) => map.set(col.colKey, col));
    return map;
  }, [tableConfig.filterColumns]);

  // FK filter columns whose option lists are fetched from a sibling entity.
  // Enabled lazily — only for the active entity's configured FK filter columns —
  // so a tab with no FK filters fans out no option requests.
  const fkFilterColumns = useMemo(
    () =>
      tableConfig.filterColumns.filter(
        (col): col is Extract<InventoryFilterColumn, { type: "fk" }> => col.type === "fk",
      ),
    [tableConfig.filterColumns],
  );

  const fkOptionEntities = useMemo(
    () => Array.from(new Set(fkFilterColumns.flatMap((col) => fkEntitiesForColumn(col)))),
    [fkFilterColumns],
  );

  const fkOptionQueries = useQueries({
    queries: fkOptionEntities.map((entity) => ({
      queryKey: ["inventory", "resources", entity, "filterOptions"],
      queryFn: () => listAllResources(entity),
      staleTime: 60_000,
    })),
  });

  const fkRowsByEntity = useMemo(() => {
    const map = new Map<InventoryEntityKey, Resource[]>();
    fkOptionEntities.forEach((entity, idx) => {
      map.set(entity, fkOptionQueries[idx]?.data ?? []);
    });
    return map;
  }, [fkOptionEntities, fkOptionQueries]);

  // colKey → resolved `{value,label}[]` filter options. Enum columns map their
  // static i18n-keyed values; FK columns map the fetched entity list to
  // `{value:id, label:name}` (popover search handles high cardinality).
  const filterOptionsByKey = useMemo(() => {
    const map = new Map<string, DataTableFilterOption[]>();
    tableConfig.filterColumns.forEach((col) => {
      if (col.type === "enum") {
        map.set(
          col.colKey,
          col.enumValues.map((v) => ({ value: v.value, label: t(v.label, { defaultValue: v.value }) })),
        );
      }
    });
    fkFilterColumns.forEach((col) => {
      const options = new Map<string, DataTableFilterOption>();
      const fkEntities = fkEntitiesForColumn(col);
      const qualifySource = fkEntities.length > 1;
      fkEntities.forEach((entity) => {
        const rows = fkRowsByEntity.get(entity) ?? [];
        rows.forEach((resource) => {
          const optionKey = qualifySource ? `${entity}:${resource.id}` : resource.id;
          options.set(optionKey, {
            key: optionKey,
            value: resource.id,
            label: labelForFkFilterOption(entity, resource, t, qualifySource),
          });
        });
      });
      map.set(col.colKey, Array.from(options.values()));
    });
    return map;
  }, [fkFilterColumns, fkRowsByEntity, t, tableConfig.filterColumns]);
  const currentTableSortKeys = useMemo(() => tableConfig.sortKeys, [tableConfig.sortKeys]);
  const currentTableFilterKeys = useMemo(
    () => tableConfig.filterColumns.map((column) => column.colKey),
    [tableConfig.filterColumns],
  );
  const sanitizedSortState = useMemo(
    () => sanitizeDataTableSortState(sortState, currentTableSortKeys),
    [currentTableSortKeys, sortState],
  );
  const sanitizedColumnFilters = useMemo(
    () => sanitizeDataTableColumnFilters(columnFilters, currentTableFilterKeys),
    [columnFilters, currentTableFilterKeys],
  );

  // Active filters keyed by backend param (column key → param), comma-joined by
  // the table-fetch path. Only configured filter columns are forwarded.
  const activeFilterParams = useMemo(() => {
    const params: Record<string, string[]> = {};
    for (const [colKey, values] of Object.entries(sanitizedColumnFilters)) {
      if (values.length === 0) continue;
      const param = filterColumnByKey.get(colKey)?.param;
      if (param) params[param] = values;
    }
    return params;
  }, [filterColumnByKey, sanitizedColumnFilters]);

  const hasActiveQuery = Boolean(debouncedSearch) || Object.keys(activeFilterParams).length > 0;
  const resetActiveQuery = () => {
    resetFocusPending.current = true;
    if (debounceTimer.current !== null) clearTimeout(debounceTimer.current);
    setSearch("");
    setDebouncedSearch("");
    setColumnFilters({});
    setPage(0);
  };

  useEffect(() => {
    if (resetFocusPending.current && searchInput.current) {
      searchInput.current.focus();
      resetFocusPending.current = false;
    }
  }, [search, debouncedSearch, columnFilters]);

  // Separate server-paginated query for the active tab's visible rows.
  // Keyed on entity + page + pageSize + debouncedSearch + sort + filters so it
  // refetches only when those dimensions actually change.
  const sortParam = sanitizedSortState
    ? { key: sanitizedSortState.key, dir: sanitizedSortState.dir }
    : null;
  const activePageQuery = useQuery({
    queryKey: [
      "inventory",
      "resources",
      activeTab,
      { page, pageSize, search: debouncedSearch, sort: sortParam, filters: activeFilterParams },
    ],
    queryFn: () =>
      listResourcesForTable(activeEntity as InventoryEntityKey, {
        limit: pageSize,
        offset: page * pageSize,
        search: debouncedSearch || undefined,
        sort: sanitizedSortState?.key,
        order: sanitizedSortState?.dir,
        filters: activeFilterParams,
      }),
    // The config-layers pseudo-entity has no `listResources` row source.
    enabled: activeEntity !== null,
    placeholderData: (prev) => prev,
  });

  // FK-carrying list endpoints return fully-hydrated rows (site_name,
  // tenant_name, …) so no client-side lookup/enrichment is needed.
  const activePageData = activePageQuery.data;
  const activeData = useMemo(() => activePageData?.items ?? [], [activePageData]);
  const activeTotal = activePageData?.total ?? 0;
  const activeDataById = useMemo(
    () => new Map(activeData.map((resource) => [resource.id, resource])),
    [activeData],
  );

  // No client-side filtering — search is passed to the server query.
  const filtered = activeData;

  // Health/metrics column is host-scoped only. The observability cache is keyed
  // `obs:host:{id}` where the id is the connection-target/device id — i.e.
  // `dcim_device.id` for Devices and the VM id for Virtual Machines. The
  // inventory `Resource.id` on those two tabs IS exactly that id (the list
  // endpoints are `/inventory/devices` and `/inventory/virtual_machines`), so a
  // row id maps 1:1 to a host id. For every other entity (site, rack, IP, …) the
  // row id is NOT a host id, so the column is omitted entirely rather than
  // rendering wrong labels. One batched cache fetch drives all visible rows; the
  // hook is called unconditionally with an empty id set on non-host tabs.
  const isHostEntity = activeEntity === "Devices" || activeEntity === "Virtual Machines";
  const hostLabelIds = useMemo(
    () => (isHostEntity ? filtered.map((resource) => resource.id) : []),
    [isHostEntity, filtered],
  );
  const { byHost: hostLabelsByHost } = useHostLabels(hostLabelIds);

  const currentCategory =
    inventoryCategories.find((c) => c.key === activeCategory) ?? DEFAULT_CATEGORY;
  const currentCategoryLabel = t(currentCategory.labelKey);
  const currentCategoryHint = t(`inventory.resources.hints.categories.${currentCategory.key}`, {
    defaultValue: t("inventory.resources.description"),
  });
  const activeEntityLabel = isConfigContextsTab
    ? t("configContexts.title")
    : isIpamWorkspaceTab
      ? t("inventory.ipamWorkspace.title")
      : isCablingWorkstationTab
        ? t("inventory.cablingWorkstation.title")
      : t(`inventory.resources.entities.${activeTab}.other`, {
          defaultValue: activeTab,
        });
  const activeEntityHint = t(`inventory.resources.hints.entities.${activeTab}`, {
    defaultValue: "",
  });
  const contextHint = activeEntityHint
    ? t("inventory.resources.hints.entityContext", {
        category: currentCategoryLabel,
        entity: activeEntityLabel,
        hint: activeEntityHint,
      })
    : t("inventory.resources.hints.categoryContext", {
        category: currentCategoryLabel,
        hint: currentCategoryHint,
      });

  const invalidateActive = () => {
    void qc.invalidateQueries({ queryKey: ["inventory", "resources", activeTab] });
    void qc.invalidateQueries({ queryKey: CONNECTION_TARGETS_QUERY_KEY });
  };

  // Standard-entity delete mutation; row actions drive it, so `activeEntity` is
  // non-null whenever it fires (the cast re-applies the real-key type).
  const deleteMutation = useApiMutation({
    mutationFn: (id: string) => deleteResource(activeEntity as InventoryEntityKey, id),
    onSuccess: () => {
      void invalidateActive();
      notify("success", t("inventory.resources.deleted"));
      setDeleteBlockedDetails(null);
      setDeleteTarget(null);
    },
    onError: (err: ApiError) => {
      const blockedDetails = parseDeleteBlockedDetails(err);
      if (blockedDetails) {
        setDeleteBlockedDetails(blockedDetails);
        notify("error", t("inventory.resources.deleteBlockedToast"));
        return;
      }
      notify("error", err.status === 409 ? t("inventory.resources.deleteBlockedToast") : err.message || t("unknownError"));
    },
  });

  const resetEntityQuery = (entity: RailEntity) => {
    onFallbackTabChange(entity);
    if (debounceTimer.current !== null) clearTimeout(debounceTimer.current);
    setSearch("");
    setDebouncedSearch("");
    setPage(0);
    setSortState(null);
    setColumnFilters({});
    setSelectedKeys(new Set());
  };

  const handlePageChange = (newPage: number) => {
    setPage(newPage);
    // Clear selections on page change — cross-page bulk ops are not supported.
    setSelectedKeys(new Set());
  };

  // Sort/filter change resets to the first page and clears selections (the
  // visible row set changes, so a kept selection could span an unseen page).
  const handleSortChange = (next: DataTableSortState | null) => {
    setSortState(next);
    setPage(0);
    setSelectedKeys(new Set());
  };

  const handleColumnFilterChange = (key: string, values: string[]) => {
    setColumnFilters((prev) => updateDataTableColumnFilters(prev, key, values));
    setPage(0);
    setSelectedKeys(new Set());
  };

  const handlePageSizeChange = (size: number) => {
    setPageSize(size);
    setPage(0);
    setSelectedKeys(new Set());
  };

  const openCreate = () => {
    // The config-layers pane owns its own create flow; the standard create
    // page is never opened for it.
    if (activeEntity === null) return;
    // Canonical full-page editor. `return` is the current list URL so Back/Cancel
    // restores the active tab/filters the operator came from.
    const returnTo = `${location.pathname}${location.search}`;
    startTransition(() => {
      void navigate(inventoryResourceNewPath(activeEntity, returnTo));
    });
  };
  const openEdit = (r: Resource) => {
    if (activeEntity === null) return;
    const returnTo = `${location.pathname}${location.search}`;
    startTransition(() => {
      void navigate(inventoryResourceEditPath(activeEntity, r.id, returnTo));
    });
  };
  const openDetail = (r: Resource) => {
    const href = inventoryResourceDetailHref(activeEntity, r.id, `${location.pathname}${location.search}${location.hash}`);
    if (href) void navigate(href);
  };

  // Selectable options for a related entity in the bulk-edit pickers.
  const loadOptions = (entity: InventoryEntityKey): EntityPickerOption[] =>
    (byTab[entity] ?? []).map((r) => inventoryResourceOption(entity, r));

  const isOptionsLoading = (entity: InventoryEntityKey): boolean =>
    optionQueryStateByEntity.get(entity)?.isPending ?? false;

  // Persists a resource (root or nested) via the same API path the user would
  // hit by picking that tab manually. Invalidates the affected tab's query so
  // the new row is selectable in parent pickers; returns the persisted Resource
  // (with backend UUID) or null on failure.
  const createEntity = async (
    entity: InventoryEntityKey,
    resource: Resource,
  ): Promise<Resource | null> => {
    try {
      const { id: _omit, ...payload } = resource;
      const created = await createResource(entity, payload);
      void qc.invalidateQueries({ queryKey: ["inventory", "resources", entity] });
      return created;
    } catch (err) {
      const message = err instanceof Error ? err.message : t("unknownError");
      notify("error", message);
      return null;
    }
  };

  const handleDelete = () => {
    if (!deleteTarget) return;
    setDeleteBlockedDetails(null);
    deleteMutation.mutate(deleteTarget.id);
  };

  const closeDeleteDialog = () => {
    setDeleteTarget(null);
    setDeleteBlockedDetails(null);
  };

  const openDeleteDialog = (resource: Resource) => {
    setDeleteBlockedDetails(null);
    setDeleteTarget(resource);
  };

  const closeBulkDeleteFailures = () => {
    setBulkDeleteFailures(null);
  };

  const closeBulkEditFailures = () => {
    setBulkEditFailures(null);
  };

  const openRelatedBlocker = useCallback(
    (entity: InventoryEntityKey, resourceId: string) => {
      closeDeleteDialog();
      closeBulkDeleteFailures();
      const returnTo = `${location.pathname}${location.search}`;
      void navigate(inventoryResourceEditPath(entity, resourceId, returnTo));
    },
    [location.pathname, location.search, navigate],
  );

  const blockerGroupLabel = (group: DeleteBlockedGroup): string =>
    group.entityKey
      ? t(`inventory.resources.entities.${group.entityKey}.other`, {
          defaultValue: group.entityKey,
        })
      : fallbackBlockerLabel(group.table);

  const executeBulkDelete = async () => {
    setBulkDeleteOpen(false);
    const ids = Array.from(selectedKeys);
    const failures: BulkDeleteFailure[] = [];
    let deletedCount = 0;
    await Promise.all(
      ids.map(async (id) => {
        try {
          await deleteResource(activeEntity as InventoryEntityKey, id);
          deletedCount += 1;
        } catch (error) {
          let errorMessage = error instanceof Error ? error.message : t("unknownError");
          if (error instanceof ApiError && error.status === 409) {
            errorMessage = t("inventory.resources.deleteBlockedToast");
          }
          failures.push({
            resourceId: id,
            resourceName: activeDataById.get(id)?.name ?? id,
            errorMessage,
            details: parseDeleteBlockedDetails(error),
          });
        }
      }),
    );
    if (deletedCount > 0) {
      notify("success", t("inventory.resources.bulkDeleteSuccess", { count: deletedCount }));
    }
    if (failures.length > 0) {
      const failure = failures[0];
      if (failures.length === 1 && ids.length === 1 && failure?.details) {
        setDeleteTarget(
          activeDataById.get(failure.resourceId) ?? {
            id: failure.resourceId,
            name: failure.resourceName,
            type: activeEntity?.toLowerCase() ?? "resource",
          },
        );
        setDeleteBlockedDetails(failure.details);
        notify("error", t("inventory.resources.deleteBlockedToast"));
        setSelectedKeys(new Set([failure.resourceId]));
        return;
      }
      setBulkDeleteFailures(failures);
      const blockedFailureCount = failures.filter((failure) => failure.details).length;
      notify(
        "error",
        blockedFailureCount === failures.length
          ? t("inventory.resources.bulkDeleteBlockedToast", { count: failures.length })
          : t("inventory.resources.bulkDeleteFailed", { failed: failures.length, total: ids.length }),
      );
      setSelectedKeys(new Set(failures.map((failure) => failure.resourceId)));
    } else {
      setSelectedKeys(new Set());
    }
    void invalidateActive();
  };

  // Rows backing the bulk-edit dialog's "mixed values" hints. Selections are
  // cleared on page/page-size changes, so the active page is the selected-row
  // source of truth without requiring a full active-entity list query.
  const selectedRows = activeData.filter((r) => selectedKeys.has(r.id));

  const handleBulkEdit = async (patch: BulkEditPatch) => {
    setBulkEditOpen(false);
    const ids = Array.from(selectedKeys);
    const { updated, failed } = await bulkUpdateResources(activeEntity as InventoryEntityKey, ids, patch);
    if (failed.length > 0) {
      setBulkEditFailures(
        failed.map((failure) => ({
          resourceId: failure.id,
          resourceName: activeDataById.get(failure.id)?.name ?? failure.id,
          reason: failure.reason,
        })),
      );
      notify(
        "error",
        t("inventory.resources.bulkEditPartial", {
          updated: updated.length,
          total: ids.length,
        }),
      );
    } else {
      setBulkEditFailures(null);
      notify("success", t("inventory.resources.bulkEditSuccess", { count: updated.length }));
    }
    void invalidateActive();
    setSelectedKeys(new Set(failed.map((failure) => failure.id)));
  };

  const handleRefresh = () => {
    // Invalidates all ["inventory", "resources", ...] queries including paginated ones.
    void qc.invalidateQueries({ queryKey: ["inventory", "resources"] });
    void qc.invalidateQueries({ queryKey: CONNECTION_TARGETS_QUERY_KEY });
  };

  // Columns: adaptive to active tab's fields config. Every tabular field
  // (excluding multiline `textarea` and the icon picker, which have no
  // single-cell representation) becomes a column so the column-settings picker
  // exposes the entity's FULL attribute set. The long-tail columns start hidden
  // via the per-entity default-hidden policy (`table.column_visibility`), not by
  // truncating the column list here. Empty for the config-layers pane (it
  // renders a bespoke list, not this table).
  const extraFields = activeEntity ? fieldsByTab[activeEntity] || [] : [];
  const activeEntityHasStatus = activeEntity ? inventoryEntityHasStatus(activeEntity) : false;
  const specialColumnKeys = baseTableConfig.specialColumnKeys ?? [];

  // Sort/filter attributes for a column, keyed by its (possibly overridden)
  // column key. `sortable` is explicit (DataTable defaults columns to sortable;
  // unsupported columns must opt out so controlled sort never offers them).
  // `filterOptions` is set only when the config marks the column filterable AND
  // its options are resolved (FK lists may still be loading → empty array hides
  // the menu, which is the correct degraded state).
  const columnMeta = (key: string): { sortable: boolean; filterOptions?: DataTableFilterOption[] } => {
    const filterOptions = filterColumnByKey.has(key)
      ? filterOptionsByKey.get(key) ?? []
      : undefined;
    return { sortable: sortableKeySet.has(key), filterOptions };
  };

  // Nameless entities (device types) render a non-`name` field in the name
  // column and sort it under that backend key; honor the `name` override here so
  // the primary column becomes sortable. Entities without a `name` override keep
  // the literal `name` key (Devices is unchanged).
  const nameColKey = activeEntity ? inventoryNameColumnKey(activeEntity) : "name";
  const identityField = activeEntity ? inventoryIdentityFieldForColumns(activeEntity, extraFields) : undefined;
  const nameColumnLabel = activeEntity
    ? identityField
      ? t(identityField.label, { defaultValue: identityField.label })
      : t(inventoryIdentityColumnLabel(activeEntity), { defaultValue: inventoryIdentityColumnLabel(activeEntity) })
    : t("name");
  const visibleFields = activeEntity
    ? extraFields.filter((field) => inventoryFieldRendersAsDataColumn(activeEntity, field))
    : [];
  const customFieldColumns = useMemo(
    () => inventoryCustomFieldDataColumns(customFieldDefinitions),
    [customFieldDefinitions],
  );

  const columns = [
    {
      key: nameColKey,
      label: nameColumnLabel,
      ...columnMeta(nameColKey),
      render: (r: Resource) => (
        <div className="flex items-center gap-2">
          <EntityIcon
            fallback={inventoryEntityIconFor(activeEntity ?? "Sites")}
            iconName={r.iconName}
            customDataUrl={r.iconDataUrl}
            className="size-9 rounded-md bg-accent/20 text-primary"
            iconClassName="h-3.5 w-3.5"
          />
          {r.status && (
            <div
              className={`w-1.5 h-1.5 rounded-full ${
                r.status === "active"
                  ? "bg-status-positive"
                  : r.status === "planned"
                  ? "bg-status-info"
                  : r.status === "decommissioning"
                  ? "bg-status-negative"
                  : "bg-status-warning"
              }`}
            />
          )}
          <span className="font-medium text-foreground">{r.name}</span>
        </div>
      ),
    },
    ...(activeEntityHasStatus ? [{
      key: "status",
      label: t("status"),
      ...columnMeta("status"),
      render: (r: Resource) => (
        r.status ? (
          <StatusChip
            status={inventoryResourceStatusTone(r.status)}
            label={inventoryResourceStatusLabel(t, r.status)}
          />
        ) : (
          <span className="text-muted-foreground">—</span>
        )
      ),
    }] : []),
    ...(specialColumnKeys.includes("utilization") ? [{
      key: "utilization",
      label: t("inventory.resources.fields.utilization"),
      width: "15rem",
      ...columnMeta("utilization"),
      render: (r: Resource) => (
        <PrefixUtilizationCell
          resource={r}
          labelUsedOfTotal={(used, total) =>
            t("inventory.resources.prefixUtilizationUsedOfTotal", { used, total })
          }
          labelUsedOnly={(used) =>
            t("inventory.resources.prefixUtilizationUsedOnly", { used })
          }
          labelFree={(free) =>
            t("inventory.resources.prefixUtilizationFree", { free })
          }
          labelMarked={t("inventory.resources.prefixUtilizationMarked")}
        />
      ),
    }] : []),
    ...(specialColumnKeys.includes("portMappings") ? [{
      key: "portMappings",
      label: t("inventory.resources.fields.portMappings"),
      width: "24rem",
      sortable: false,
      render: (r: Resource) => (
        <span className="block max-w-[24rem] truncate font-mono text-xs" title={r.portMappings}>
          {r.portMappings || t("inventory.resources.form.notSet")}
        </span>
      ),
    }] : []),
    ...visibleFields.map((f) => {
      // The table column key may differ from the form-field key so it matches
      // the backend sort key (devices: `deviceType` -> `device_type`). The
      // renderer below still closes over the original `f.key`, so cells read the
      // unchanged `Resource` field while sort/filter use the backend key.
      const colKey = activeEntity ? inventoryColumnKeyForField(activeEntity, f) : f.key;
      return {
      key: colKey,
      ...columnMeta(colKey),
      // Field labels are i18n keys for the newer entities and plain strings for
      // the original tabs; defaultValue lets both render correctly.
      label: t(f.label, { defaultValue: f.label }),
      // `Resource` is a local UI type; rendering a dynamic per-field column is
      // structural index access on the local type, not a backend-contract bypass.
      render: (r: Resource) => {
        if (f.type === "identity-multiple" && f.identityKind) {
          return <IdentitySelection kind={f.identityKind} label={t(f.label)} value={(r[f.key] as string[] | undefined) ?? []} />;
        }
        if (f.type === "related" && f.relatedTab && f.idKey) {
          const id = r[f.idKey] as string | undefined;
          const name = fkRowsByEntity.get(f.relatedTab)?.find((row) => row.id === id)?.name;
          if (name) return <span className="text-muted-foreground">{name}</span>;
        }
        if (f.type === "objectRef") {
          // The cell shows the materialized ObjectRef label; a dangling target
          // (label=null but ref present) reads "Deleted object", an absent ref
          // reads "—".
          const ref = (r as unknown as Record<string, ObjectRefProjection | null | undefined>)[f.key];
          if (!ref) {
            return <span className="text-muted-foreground italic text-xs">{t("inventory.resources.form.notSet")}</span>;
          }
          return (
            <span className="text-muted-foreground">
              {ref.label ?? t("inventory.resources.deletedObject")}
            </span>
          );
        }
        return (r as unknown as Record<string, string>)[f.key]
          ? <span className="text-muted-foreground">{(r as unknown as Record<string, string>)[f.key]}</span>
          : <span className="text-muted-foreground italic text-xs">{t("inventory.resources.form.notSet")}</span>;
      },
      };
    }),
    ...customFieldColumns.map((definition) => {
      const colKey = inventoryCustomFieldColumnKey(definition);
      return {
        key: colKey,
        label: definition.label,
        ...columnMeta(colKey),
        render: (r: Resource) =>
          renderCustomFieldValue(
            r.customFields?.[definition.name],
            t("inventory.resources.form.notSet"),
          ),
      };
    }),
    // Host-scoped health/metrics indicator (Devices / Virtual Machines only).
    // Empty header + excludeFromSettings keep it a utility column like `actions`,
    // so it never enters the column-settings picker or breaks header parity. Each
    // row's cluster renders nothing when the host has no cached labels/alerts.
    ...(specialColumnKeys.includes("observability") ? [{
      key: "observability",
      label: "",
      width: "12rem",
      excludeFromSettings: true,
      sortable: false,
      render: (r: Resource) => (
        <div className="flex min-w-0 items-center">
          <HostObservabilityCluster entry={hostLabelsByHost.get(r.id)} />
        </div>
      ),
    }] : []),
    // Read-only schema is a normal configurable data column, not a form field.
    ...(specialColumnKeys.includes("schemaSummary") ? [{
      key: "schemaSummary",
      label: t("inventory.resources.fields.schema"),
      width: "24rem",
      sortable: false,
      render: (r: Resource) => (
        r.schemaSummary ? (
          <span
            className="block max-w-[24rem] truncate font-mono text-xs text-muted-foreground"
            title={r.schemaSummary}
            aria-label={`${t("inventory.resources.fields.schema")}: ${r.schemaSummary}`}
          >
            {r.schemaSummary}
          </span>
        ) : (
          <span className="text-muted-foreground italic text-xs">
            {t("inventory.resources.form.notSet")}
          </span>
        )
      ),
    }] : []),
    {
      key: "actions",
      label: "",
      excludeFromSettings: true,
      render: (r: Resource) => (
        (canRead || (canWriteActiveEntity && (canUpdate || canDelete))) ? <RowActions label={`${t("actions")}: ${r.name}`} {...triggerProps(r.id)}>
          {canRead && (
            <DropdownMenuItem onSelect={() => openDetail(r)}><Eye className="size-4" />{t("view")}</DropdownMenuItem>
          )}
          {canUpdate && canWriteActiveEntity && (
            <DropdownMenuItem onSelect={() => openEdit(r)}><Pencil className="size-4" />{t("edit")}</DropdownMenuItem>
          )}
          {canDelete && canWriteActiveEntity && (
            <DropdownMenuItem variant="destructive" onSelect={() => openDeleteDialog(r)}><Trash2 className="size-4" />{t("delete")}</DropdownMenuItem>
          )}
        </RowActions> : null
      ),
    },
  ];

  return (
    <div className="flex min-w-0 flex-col gap-4">
      <PageChrome
        title={isConfigContextsTab ? t("configContexts.title") : isIpamWorkspaceTab ? t("inventory.ipamWorkspace.navLabel") : isCablingWorkstationTab ? t("inventory.cablingWorkstation.navLabel") : activeEntity ? t(`inventory.resources.entities.${activeEntity}.other`, { defaultValue: activeEntity }) : t("inventory.resources.title")}
        icon={Database}
        description={contextHint}
        badge={activeEntity ? `${activeTotal} ${t("items")}` : undefined}
        actions={
          activeEntity ? <div className="flex gap-2">
            {canUpdate && canWriteActiveEntity && selectedKeys.size > 0 && (
              <Button
                variant="outline"
                size="sm"
                className="gap-2"
                onClick={() => setBulkEditOpen(true)}
              >
                <Pencil className="h-3 w-3" /> {t("inventory.resources.bulkEdit", { count: selectedKeys.size })}
              </Button>
            )}
            {canDelete && canWriteActiveEntity && selectedKeys.size > 0 && (
              <Button
                variant="outline"
                size="sm"
                className="gap-2 text-destructive border-destructive/30"
                onClick={() => setBulkDeleteOpen(true)}
              >
                <Trash2 className="h-3 w-3" /> {t("inventory.resources.bulkDelete", { count: selectedKeys.size })}
              </Button>
            )}
            <Button
              variant="outline"
              size="sm"
              className="gap-2"
              onClick={handleRefresh}
            >
              <RefreshCw className="h-3 w-3" /> {t("refresh")}
            </Button>
            {canCreateActiveEntity && (
              <Button className="gap-2" onClick={openCreate}>
                <Plus className="h-3 w-3" /> {t("create")}
              </Button>
            )}
          </div> : undefined
        }
      />

      <div className="flex min-w-0 flex-col gap-4">
        <TooltipProvider delayDuration={200}>
        <div className="flex min-h-0 min-w-0 flex-1 flex-col gap-4">
          <div role="group" aria-label={t("inventory.resources.title")} className="flex min-w-0 flex-col gap-2">
          <SectionLinks
            label={t("inventory.resources.title")}
            activeValue={activeCategory}
            wrap
            onNavigate={(category) => resetEntityQuery(firstEntityOf(category))}
            items={inventoryCategories.map(category => ({
              value: category.key,
              label: t(category.labelKey),
              to: { search: `?${inventoryEntityParams(firstEntityOf(category.key))}` },
              icon: category.icon,
              badge: countBadge(category.entities),
            }))}
          />
          <SectionLinks<RailEntity>
            label={t(currentCategory.labelKey)}
            activeValue={activeTab}
            variant="secondary"
            wrap
            onNavigate={resetEntityQuery}
            items={currentCategory.entities.map((entity) => {
              const isConfigLayers = isConfigContextsRailKey(entity);
              const isIpamWorkspace = isIpamWorkspaceRailKey(entity);
              const isCablingWorkstation = isCablingWorkstationRailKey(entity);
              const isPseudo = isConfigLayers || isIpamWorkspace || isCablingWorkstation;
              const label = isConfigLayers ? t("configContexts.title")
                : isIpamWorkspace ? t("inventory.ipamWorkspace.navLabel")
                : isCablingWorkstation ? t("inventory.cablingWorkstation.navLabel")
                : t(`inventory.resources.entities.${entity}.other`, { defaultValue: entity });
              return { value: entity, label,
                to: { search: `?${inventoryEntityParams(entity)}` },
                badge: isPseudo ? undefined : countBadge([entity]),
                icon: isConfigLayers ? Layers : isIpamWorkspace ? Network : isCablingWorkstation ? Cable : undefined };
            })}
          />
          {activeEntity && <SearchInput
            ref={searchInput}
            containerClassName="min-w-0 w-full"
            value={search}
            onValueChange={(value) => { setSearch(value); setPage(0); }}
            placeholder={t("inventory.resources.searchPlaceholder", { entity: t(`inventory.resources.entities.${activeTab}.other`, { defaultValue: activeTab }) })}
          />}
          </div>

          <div className="m-0 flex min-h-0 flex-1 flex-col">
          {isConfigContextsTab ? (
              <ConfigContexts />
          ) : isIpamWorkspaceTab ? (
              <IpamWorkspace />
          ) : isCablingWorkstationTab ? (
              <CablingWorkstation />
          ) : (
          <DataTable
            className="min-h-0"
            columns={columns}
            data={filtered}
            getRowKey={(r) => r.id}
            selectable
            selectedKeys={selectedKeys}
            onSelectionChange={setSelectedKeys}
            loading={activePageQuery.isPending}
            error={activePageQuery.isError ? activePageQuery.error : undefined}
            onRetry={() => activePageQuery.refetch()}
            sortState={sanitizedSortState}
            onSortChange={handleSortChange}
            columnFilters={sanitizedColumnFilters}
            onColumnFilterChange={handleColumnFilterChange}
            tableId={`inventory.${activeTab}`}
            // Identity + status are core columns and stay present in the picker;
            // `actions` is non-hideable by DataTable default. `nameColKey` honors
            // the nameless-entity override (e.g. Device Types -> `model`).
            nonHideableKeys={[nameColKey, "status", "actions"]}
            pagination={{
              page,
              pageSize,
              total: activeTotal,
              onPageChange: handlePageChange,
              onPageSizeChange: handlePageSizeChange,
            }}
            emptyState={
              activePageQuery.isPending ? (
                <div className="flex items-center justify-center py-8 text-xs text-muted-foreground">
                  <Loader2 className="h-4 w-4 animate-spin mr-2" />
                  {t("inventory.resources.loadingEntities", {
                    entity: t(`inventory.resources.entities.${activeTab}.other`, { defaultValue: activeTab }),
                  })}
                </div>
              ) : activePageQuery.isError ? (
                <div className="py-8 text-center text-xs text-destructive">
                  {t("inventory.resources.failedToLoad", {
                    entity: t(`inventory.resources.entities.${activeTab}.other`, { defaultValue: activeTab }),
                    error: (activePageQuery.error as { message?: string })?.message ?? t("unknownError"),
                  })}
                </div>
              ) : (
                <div className="flex flex-col items-center">
                <EmptyState
                  icon={Database}
                  className={hasActiveQuery ? "pb-4" : undefined}
                  title={
                    hasActiveQuery
                      ? t("noResults")
                      : t("inventory.resources.noEntity", {
                          entity: t(`inventory.resources.entities.${activeTab}.other`, { defaultValue: activeTab }),
                        })
                  }
                  description={
                    hasActiveQuery
                      ? t("inventory.resources.tryAdjusting")
                      : !canCreateActiveEntity ? t("inventory.resources.noAvailableRecords")
                      : t("inventory.resources.createEntry", {
                          entity: t(`inventory.resources.entities.${activeTab}.one`, { defaultValue: activeTab }),
                        })
                  }
                  actionLabel={
                    hasActiveQuery || !canCreateActiveEntity
                      ? undefined
                      : t("inventory.resources.addEntity", {
                          entity: t(`inventory.resources.entities.${activeTab}.one`, {
                            defaultValue: tabConfigByKey.get(activeEntity ?? "Sites")?.singular ?? activeTab,
                          }),
                        })
                  }
                  onAction={hasActiveQuery || !canCreateActiveEntity ? undefined : openCreate}
                />
                {hasActiveQuery ? <Button type="button" variant="outline" size="sm" className="mb-12" onClick={resetActiveQuery}>{t("inventory.resources.resetQuery")}</Button> : null}
                </div>
              )
            }
          />
          )}
          </div>
        </div>
        </TooltipProvider>
      </div>

      {activeEntity !== null && (
        <BulkEditDialog
          open={bulkEditOpen}
          onOpenChange={setBulkEditOpen}
          entity={activeEntity}
          selectedRows={selectedRows}
          loadOptions={loadOptions}
          onVisibleRelatedEntitiesChange={handleBulkRelatedEntitiesChange}
          isOptionsLoading={isOptionsLoading}
          onApply={(patch) => void handleBulkEdit(patch)}
          onCreateRelated={(related) => setBulkCreateEntity(related)}
        />
      )}

      {/* Recursive create launched from the bulk-edit dialog's pickers. */}
      {bulkCreateEntity && (
        <EntityCreateEditor
          open={!!bulkCreateEntity}
          onOpenChange={(v) => !v && setBulkCreateEntity(null)}
          rootEntity={bulkCreateEntity}
          createEntity={createEntity}
          loadOptions={loadOptions}
          onVisibleRelatedEntitiesChange={handleBulkCreateRelatedEntitiesChange}
          isOptionsLoading={isOptionsLoading}
          onRootCreated={() => setBulkCreateEntity(null)}
        />
      )}

      <AlertDialog open={bulkDeleteOpen} onOpenChange={(open) => !open && setBulkDeleteOpen(false)}>
        <AlertDialogContent className="surface">
          <AlertDialogHeader>
            <AlertDialogTitle>{t("inventory.resources.bulkDeleteConfirmTitle")}</AlertDialogTitle>
            <AlertDialogDescription>
              {t("inventory.resources.bulkDeleteConfirmDescription", { count: selectedKeys.size })}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("cancel")}</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => void executeBulkDelete()}
              className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
            >
              {t("inventory.resources.bulkDeleteConfirmAction", { count: selectedKeys.size })}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <AlertDialog open={!!deleteTarget} onOpenChange={(open) => !open && closeDeleteDialog()}>
        <AlertDialogContent className="surface" onCloseAutoFocus={event => { event.preventDefault(); restoreFocus(); }}>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {deleteBlockedDetails
                ? t("inventory.resources.deleteBlockedTitle")
                : t("inventory.resources.deleteResource")}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {deleteBlockedDetails ? (
                t("inventory.resources.deleteBlockedDescription", {
                  name: deleteTarget?.name ?? "",
                })
              ) : (
                <Trans
                  i18nKey="inventory.resources.deleteDescription"
                  values={{ name: deleteTarget?.name ?? "" }}
                  components={{ 1: <strong /> }}
                />
              )}
            </AlertDialogDescription>
          </AlertDialogHeader>
          {deleteBlockedDetails && (
            <DeleteBlockedPanel
              details={deleteBlockedDetails}
              blockerGroupLabel={blockerGroupLabel}
              onOpenRelated={openRelatedBlocker}
            />
          )}
          <AlertDialogFooter>
            <AlertDialogCancel>{deleteBlockedDetails ? t("close") : t("cancel")}</AlertDialogCancel>
            {!deleteBlockedDetails && (
              <Button
                type="button"
                onClick={handleDelete}
                disabled={deleteMutation.isPending}
                className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
              >
                {t("delete")}
              </Button>
            )}
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <AlertDialog open={!!bulkDeleteFailures?.length} onOpenChange={(open) => !open && closeBulkDeleteFailures()}>
        <AlertDialogContent className="surface max-w-2xl">
          <AlertDialogHeader>
            <AlertDialogTitle>{t("inventory.resources.bulkDeleteBlockedTitle")}</AlertDialogTitle>
            <AlertDialogDescription>
              {t("inventory.resources.bulkDeleteBlockedDescription", {
                count: bulkDeleteFailures?.length ?? 0,
              })}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <div className="max-h-[60vh] space-y-3 overflow-y-auto pr-1">
            {bulkDeleteFailures?.map((failure) => (
              <div key={failure.resourceId} className="rounded-lg border border-border bg-accent/10 p-3">
                <div className="space-y-1">
                  <p className="text-sm font-medium">{failure.resourceName}</p>
                  <p className="text-xs text-muted-foreground">{failure.errorMessage}</p>
                </div>
                {failure.details ? (
                  <div className="mt-3">
                    <DeleteBlockedPanel
                      details={failure.details}
                      blockerGroupLabel={blockerGroupLabel}
                      onOpenRelated={openRelatedBlocker}
                    />
                  </div>
                ) : null}
              </div>
            ))}
          </div>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("close")}</AlertDialogCancel>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <AlertDialog open={!!bulkEditFailures?.length} onOpenChange={(open) => !open && closeBulkEditFailures()}>
        <AlertDialogContent className="surface max-w-2xl">
          <AlertDialogHeader>
            <AlertDialogTitle>{t("inventory.resources.bulkEditFailedTitle")}</AlertDialogTitle>
            <AlertDialogDescription>
              {t("inventory.resources.bulkEditFailedDescription", {
                count: bulkEditFailures?.length ?? 0,
              })}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <div className="max-h-[60vh] space-y-2 overflow-y-auto pr-1">
            {bulkEditFailures?.map((failure) => (
              <div key={failure.resourceId} className="rounded-lg border border-border bg-accent/10 p-3">
                <p className="text-sm font-medium">{failure.resourceName}</p>
                <p className="mt-1 text-xs text-muted-foreground">{failure.reason}</p>
              </div>
            ))}
          </div>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("close")}</AlertDialogCancel>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
};

const InventoryResources = () => {
  const { can } = usePermissions();
  const canRead = can("READ", "INVENTORY");
  const [searchParams] = useSearchParams();
  const [fallbackTab, setFallbackTab] = useState<RailEntity>("Sites");
  const activeTab = parseInventoryRailEntity(searchParams) ?? fallbackTab;
  return <PageLayout width={canRead && isIpamWorkspaceRailKey(activeTab) ? "full" : "standard"} fill>
    <RequireCapability check={() => canRead}>
      <InventoryResourcesBody activeTab={activeTab} onFallbackTabChange={setFallbackTab} />
    </RequireCapability>
  </PageLayout>;
};

export default InventoryResources;
