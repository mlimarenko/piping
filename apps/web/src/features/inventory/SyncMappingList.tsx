import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useApiMutation } from "@/shared/api/hooks";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { useLocation, useNavigate, useParams } from "react-router";
import { Plus, Layers, Activity, Pencil, Trash2 } from "lucide-react";
import { notify } from "@/shared/lib/notifications";

import PageChrome from "@/shared/ui-shared/PageChrome";
import RowActions from "@/shared/ui-shared/RowActions";
import { useInventoryRowActionFocus } from "./useInventoryRowActionFocus";
import { DropdownMenuItem } from "@/shared/ui-primitives/dropdown-menu";
import DataTable from "@/shared/ui-shared/DataTable";
import { Button } from "@/shared/ui-primitives/button";
import { Badge } from "@/shared/ui-primitives/badge";
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
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/shared/ui-primitives/popover";
import { useTablePageSize } from "@/shared/hooks/useTablePageSize";
import EmptyState from "@/shared/ui-shared/EmptyState";
import SearchInput from "@/shared/ui-shared/SearchInput";
import type { DataTableSortState } from "@/shared/ui-shared/DataTable";
import {
  sanitizeDataTableColumnFilters,
  sanitizeDataTableSortState,
  updateDataTableColumnFilters,
} from "@/shared/ui-shared/tableState";
import {
  buildSyncMappingsFilterParams,
  SYNC_MAPPINGS_TABLE_CONFIG,
} from "@/features/inventory/syncMappingsTableConfig";
import type { MappingConfiguration } from "@/shared/api/types";
import {
  listMappingConfigurations,
  deleteMappingConfiguration,
} from "@/features/inventory/sync-mapping-api";
import SectionLinks from "@/shared/ui-shared/SectionLinks";
import { syncSectionLinks } from "./syncSections";
/**
 * List page: all MappingConfigurations grouped by logical_id with version chips.
 * Routes: /inventory/sync/mappings and /inventory/sync/:providerId/mappings
 *
 * Pagination: client-side over grouped array. The backend returns raw
 * MappingConfiguration rows; grouping (by logical_id) happens on the
 * frontend in groupConfigs(). Server-side limit/offset would split groups
 * across pages, so client-side pagination over groups is the correct choice.
 * The list is small per-provider (typically < 50 logical configs).
 */

interface GroupedConfig {
  logical_id: string;
  name: string;
  latest: MappingConfiguration;
  olderVersions: MappingConfiguration[];
  provider_id: string;
}

function groupConfigs(items: MappingConfiguration[]): GroupedConfig[] {
  const byLogical = new Map<string, MappingConfiguration[]>();
  for (const item of items) {
    const group = byLogical.get(item.logical_id) ?? [];
    group.push(item);
    byLogical.set(item.logical_id, group);
  }

  return Array.from(byLogical.entries()).map(([logical_id, versions]) => {
    const sorted = [...versions].sort((a, b) => b.version - a.version);
    // byLogical only stores non-empty arrays (group.push runs before .set), so sorted[0] exists.
    const latest = sorted.find((v) => v.is_latest) ?? sorted[0]!;
    const olderVersions = sorted.filter((v) => v.id !== latest.id);
    return {
      logical_id,
      name: latest.name,
      latest,
      olderVersions,
      provider_id: latest.provider_id,
    };
  });
}

const SyncMappingList = () => {
  const location = useLocation();
  const { t } = useTranslation();
  const navigate = useNavigate();
  const { providerId } = useParams<{ providerId?: string }>();
  const qc = useQueryClient();
  const { triggerProps, restoreFocus } = useInventoryRowActionFocus();
  // View-but-not-edit: hide create/edit/delete affordances the caller can't use.
  const { can } = usePermissions();
  const canCreate = can("CREATE", "INVENTORY");
  const canUpdate = can("UPDATE", "INVENTORY");
  const canDelete = can("DELETE", "INVENTORY");
  const [deleteTarget, setDeleteTarget] = useState<GroupedConfig | null>(null);
  const [sortState, setSortState] = useState<DataTableSortState | null>(null);
  const [columnFilters, setColumnFilters] = useState<Record<string, string[]>>({});
  const sanitizedSortState = sanitizeDataTableSortState(
    sortState,
    SYNC_MAPPINGS_TABLE_CONFIG.sortKeys,
  );
  const sanitizedColumnFilters = sanitizeDataTableColumnFilters(
    columnFilters,
    SYNC_MAPPINGS_TABLE_CONFIG.filterColumns.map((column) => column.colKey),
  );
  const filterParams = buildSyncMappingsFilterParams(sanitizedColumnFilters);
  const [search, setSearch] = useState("");
  const [debouncedSearch, setDebouncedSearch] = useState("");
  const searchTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  // Client-side pagination state
  const [page, setPage] = useState(0);
  const { value: pageSize, setValue: setPageSize } = useTablePageSize();

  useEffect(() => {
    if (searchTimer.current !== null) clearTimeout(searchTimer.current);
    searchTimer.current = setTimeout(() => {
      setDebouncedSearch(search.trim());
      setPage(0);
    }, 250);
    return () => {
      if (searchTimer.current !== null) clearTimeout(searchTimer.current);
    };
  }, [search]);

  const query = useQuery({
    queryKey: [
      "sync",
      "mapping-configurations",
      providerId,
      { search: debouncedSearch, sort: sanitizedSortState, filters: sanitizedColumnFilters },
    ],
    queryFn: () =>
      listMappingConfigurations({
        provider_id: providerId,
        search: debouncedSearch || undefined,
        direction: filterParams.direction,
        sort: sanitizedSortState?.key as
          | "name"
          | "versions"
          | "direction"
          | "provider_id"
          | undefined,
        order: sanitizedSortState?.dir,
        limit: 500,
      }),
  });

  const deleteMutation = useApiMutation({
    mutationFn: (id: string) => deleteMappingConfiguration(id),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ["sync", "mapping-configurations"] });
      notify("success", t("sync.mapping.deleted", "Configuration deleted"));
      setDeleteTarget(null);
    },
    onError: () => notify("error", t("sync.mapping.deleteFailed", "Delete failed")),
  });

  const handleDelete = () => {
    if (!deleteTarget || !canDelete || deleteMutation.isPending) return;
    deleteMutation.mutate(deleteTarget.latest.id);
  };

  const navToEditor = (id: string, mode: "edit" | "readonly" | "create") => {
    if (mode === "create") {
      const base = providerId
        ? `/inventory/sync/${encodeURIComponent(providerId)}/mappings/new`
        : "/inventory/sync/mappings/new";
      void navigate({ pathname: base, search: location.search, hash: location.hash });
    } else {
      void navigate({ pathname: `/inventory/sync/mappings/${id}/${mode}`, search: location.search, hash: location.hash });
    }
  };

  const directionLabel = (direction: string) => {
    if (direction === "pull") return t("sync.mapping.directionPull", "Pull");
    if (direction === "push") return t("sync.mapping.directionPush", "Push");
    if (direction === "bidirectional") return t("sync.mapping.directionBidirectional", "Bidirectional");
    return direction;
  };

  const items = query.data?.items ?? [];
  const groups = groupConfigs(items);
  const directionFilterOptions = ["pull", "push", "bidirectional"].map((direction) => ({
    value: direction,
    label: directionLabel(direction),
    count: groups.filter((group) => group.latest.direction === direction).length,
  }));
  const columns = [
    {
      key: "name",
      label: t("sync.mapping.nameColumn", "Name"),
      getSortValue: (group: GroupedConfig) => group.name,
      render: (group: GroupedConfig) => (
        <span className="text-sm font-medium text-foreground">{group.name}</span>
      ),
    },
    {
      key: "versions",
      label: t("sync.mapping.versionsLabel", "Versions"),
      getSortValue: (group: GroupedConfig) => group.latest.version,
      render: (group: GroupedConfig) => (
        <div className="flex items-center gap-1 flex-wrap">
          <span className="surface-chip px-2 py-0.5 rounded-full text-xs font-medium text-primary">
            v{group.latest.version} ({t("sync.mapping.latestBadge", "latest")})
          </span>
          {group.olderVersions.slice(0, 2).map((v) => (
            <Button variant="ghost"
              type="button"
              key={v.id}
              onClick={(e) => {
                e.stopPropagation();
                navToEditor(v.id, "readonly");
              }}
              className="surface-chip px-2 py-0.5 rounded-full text-xs font-medium text-muted-foreground hover:text-foreground transition-colors"
            >
              v{v.version}
            </Button>
          ))}
          {group.olderVersions.length > 2 && (
            <Popover>
              <PopoverTrigger asChild>
                <Button variant="ghost"
                  type="button"
                  onClick={(e) => e.stopPropagation()}
                  aria-label={t("sync.mapping.moreVersionsLabel", {
                    count: group.olderVersions.length - 2,
                    defaultValue: "{{count}} older versions",
                  })}
                  className="surface-chip px-2 py-0.5 rounded-full text-xs font-medium text-muted-foreground hover:text-foreground transition-colors"
                >
                  {t("sync.mapping.moreVersions", {
                    count: group.olderVersions.length - 2,
                    defaultValue: "+{{count}} more",
                  })}
                </Button>
              </PopoverTrigger>
              <PopoverContent className="w-64 p-3 space-y-1.5">
                <p className="text-xs font-semibold text-foreground mb-2">
                  {t("sync.mapping.olderVersionsTitle", "Older versions")}
                </p>
                {group.olderVersions.slice(2).map((v) => (
                  <div key={v.id} className="flex items-center justify-between gap-2 text-xs">
                    <div className="text-muted-foreground min-w-0">
                      <span className="font-medium">v{v.version}</span>
                      {" — "}
                      {new Date(v.created_at).toLocaleDateString()}
                      {v.created_by && ` · ${v.created_by}`}
                    </div>
                    <Button
                      variant="ghost"
                      size="sm"
                      className="h-9 text-sm px-2 shrink-0"
                      onClick={() => navToEditor(v.id, "readonly")}
                    >
                      {t("view", "View")}
                    </Button>
                  </div>
                ))}
              </PopoverContent>
            </Popover>
          )}
          <InFlightBadge count={group.latest.in_flight_job_count} />
        </div>
      ),
    },
    {
      key: "direction",
      label: t("sync.mapping.directionLabel", "Direction"),
      filterOptions: directionFilterOptions,
      getSortValue: (group: GroupedConfig) => group.latest.direction,
      render: (group: GroupedConfig) => (
        <span className="text-xs text-muted-foreground">
          {directionLabel(group.latest.direction)}
        </span>
      ),
    },
    {
      key: "provider_id",
      label: t("sync.mapping.providerLabel", "Provider"),
      getSortValue: (group: GroupedConfig) => group.provider_id,
      render: (group: GroupedConfig) => (
        <span className="truncate font-mono text-[11px] text-muted-foreground">{group.provider_id}</span>
      ),
    },
    {
      key: "actions",
      label: "",
      excludeFromSettings: true,
      render: (group: GroupedConfig) => canUpdate || canDelete ? (
        <RowActions label={`${t("actions")}: ${group.name}`} {...triggerProps(group.logical_id)}>
          {canUpdate && <DropdownMenuItem onSelect={() => navToEditor(group.latest.id, "edit")}><Pencil className="size-4" />{t("edit")}</DropdownMenuItem>}
          {canDelete && <DropdownMenuItem variant="destructive" onSelect={() => setDeleteTarget(group)}><Trash2 className="size-4" />{t("delete")}</DropdownMenuItem>}
        </RowActions>
      ) : null,
    },
  ];

  // Slice groups for the current page
  const pagedGroups = groups.slice(page * pageSize, (page + 1) * pageSize);

  return (
    <div className="flex min-w-0 flex-col gap-6">
      <PageChrome
        title={t("inventory.sync.title")}
        icon={Activity}
        description={t("inventory.sync.description")}
        actions={
          canCreate ? (
            <Button
              size="sm"
              className="h-9 text-sm gap-1"
              onClick={() => navToEditor("", "create")}
            >
              <Plus className="h-3 w-3" />
              {t("sync.mapping.newConfig", "New Configuration")}
            </Button>
          ) : undefined
        }
      />

      <SectionLinks label={t("inventory.sync.sectionsLabel")} activeValue="mappings"
        items={syncSectionLinks(t, { mappings: groups.length }, location, "mappings", providerId)} />

      <DataTable
        tableId="inventory.syncMappings"
        className="max-h-[65dvh] min-h-0"
        columns={columns}
        data={pagedGroups}
        getRowKey={(g) => g.logical_id}
        onRowClick={(group) => navToEditor(group.latest.id, "readonly")}
        loading={query.isLoading}
        error={query.isError ? query.error : undefined}
        onRetry={() => query.refetch()}
        sortState={sanitizedSortState}
        onSortChange={(next) => {
          setSortState(next);
          setPage(0);
        }}
        columnFilters={sanitizedColumnFilters}
        onColumnFilterChange={(key, values) => {
          setColumnFilters((current) => updateDataTableColumnFilters(current, key, values));
          setPage(0);
        }}
        pagination={{
          page,
          pageSize,
          total: groups.length,
          onPageChange: (p) => setPage(p),
          onPageSizeChange: (s) => { setPageSize(s); setPage(0); },
        }}
        emptyState={
          <EmptyState
            icon={Layers}
            title={t("sync.mapping.noConfigurations", "No mapping configurations yet.")}
            description={t(
              "sync.mapping.noConfigurationsDescription",
              "Create your first configuration to define how fields sync.",
            )}
            actionLabel={canCreate ? t("sync.mapping.newConfig", "New Configuration") : undefined}
            onAction={canCreate ? () => navToEditor("", "create") : undefined}
          />
        }
        toolbar={
          <SearchInput
            value={search}
            onValueChange={setSearch}
            placeholder={t("sync.mapping.searchPlaceholder", "Search configurations…")}
          />
        }
      />

      <AlertDialog open={!!deleteTarget} onOpenChange={(open) => !open && setDeleteTarget(null)}>
        <AlertDialogContent className="surface" onCloseAutoFocus={event => { event.preventDefault(); restoreFocus(); }}>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("sync.mapping.deleteTitle", "Delete configuration")}</AlertDialogTitle>
            <AlertDialogDescription>
              {t("sync.mapping.confirmDelete", "Delete this configuration?")}
              {" "}{deleteTarget?.name}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("cancel")}</AlertDialogCancel>
            <AlertDialogAction disabled={!canDelete || deleteMutation.isPending} onClick={handleDelete} className="bg-destructive text-destructive-foreground hover:bg-destructive/90">
              {t("delete")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
};

// ── InFlightBadge ─────────────────────────────────────────────────────────────

interface InFlightBadgeProps {
  count: number;
}

/**
 * Truthful, non-interactive in-flight count. There is no runs/jobs route and no
 * list-jobs API, so this must never navigate — a clickable badge would land the
 * user on a NotFound page.
 */
const InFlightBadge = ({ count }: InFlightBadgeProps) => {
  const { t } = useTranslation();
  if (!count) return null;

  return (
    <span
      role="status"
      title={t("sync.mapping.inFlightJobsLabel", { count, defaultValue: "{{count}} in-flight jobs" })}
      className="inline-flex items-center gap-1"
    >
      <Badge
        variant="secondary"
        className="h-4 gap-0.5 px-1.5 text-xs font-semibold bg-status-info/15 text-status-info border-status-info/30"
      >
        <Activity className="h-2.5 w-2.5" />
        {count} {t("sync.mapping.inFlightSuffix", "in-flight")}
      </Badge>
    </span>
  );
};

export default SyncMappingList;
