import { useCallback, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { Plus, Trash2, Loader2, Globe } from "lucide-react";
import RowActions from "@/shared/ui-shared/RowActions";
import { useInventoryRowActionFocus } from "@/features/inventory/useInventoryRowActionFocus";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { DropdownMenuItem } from "@/shared/ui-primitives/dropdown-menu";
import { useApiMutation } from "@/shared/api/hooks";
import type { ApiError } from "@/shared/api/client";
import { notify } from "@/shared/lib/notifications";
import { assertUuid } from "@/shared/lib/ids";
import type { InventoryEntityKey, Resource } from "@/shared/api/types";
import { inventoryResourceOptionsQuery } from "@/features/inventory/resourceOptions";
import {
  listBindings,
  createBinding,
  deleteBinding,
  type ConfigContextBinding,
} from "@/features/inventory/config-contexts-api";
import {
  CONFIG_CONTEXT_BINDINGS_TABLE_CONFIG,
  buildConfigContextBindingsFilterParams,
} from "@/features/inventory/configContextBindingsTableConfig";
import { Button } from "@/shared/ui-primitives/button";
import FormField from "@/shared/ui-shared/FormField";
import FormSelect from "@/shared/ui-shared/FormSelect";
import EntitySelect, { type EntityOption } from "@/shared/ui-shared/EntitySelect";
import DataTable, { type Column } from "@/shared/ui-shared/DataTable";
import {
  sanitizeDataTableColumnFilters,
  sanitizeDataTableSortState,
  updateDataTableColumnFilters,
} from "@/shared/ui-shared/tableState";
import EntityOptionLabel from "@/shared/ui-shared/EntityOptionLabel";
import {
  AlertDialog, AlertDialogAction, AlertDialogCancel,
  AlertDialogContent, AlertDialogDescription, AlertDialogFooter,
  AlertDialogHeader, AlertDialogTitle,
} from "@/shared/ui-primitives/alert-dialog";

/**
 * NetBox config-context scope types offered by the binding picker, each mapped
 * to the inventory catalogue that supplies its target options. Resolution
 * matches every assigned category (AND) and any target inside one category
 * (OR); a context with no bindings is global. Contributing contexts are ordered
 * by weight and name, and the device/VM local data still wins.
 *
 * `connection_target` stays internal: it has no inventory catalogue, so it is
 * not offered here.
 */
const SCOPE_TYPE_ENTITY: ReadonlyArray<readonly [string, InventoryEntityKey]> = [
  ["region", "Regions"],
  ["site_group", "Site Groups"],
  ["site", "Sites"],
  ["location", "Locations"],
  ["device_type", "Device Types"],
  ["role", "Device Roles"],
  ["platform", "Platforms"],
  ["cluster_type", "Cluster Types"],
  ["cluster_group", "Cluster Groups"],
  ["cluster", "Clusters"],
  ["tenant_group", "Tenant Groups"],
  ["tenant", "Tenants"],
  ["tag", "Tags"],
  ["device", "Devices"],
  ["vm", "Virtual Machines"],
];

const ENTITY_BY_SCOPE_TYPE = new Map<string, InventoryEntityKey>(SCOPE_TYPE_ENTITY);
const OFFERED_SCOPE_TYPES = SCOPE_TYPE_ENTITY.map(([token]) => token);

interface ConfigContextScopeBindingsProps {
  /** The config context whose bindings are managed. */
  contextId: string;
  /**
   * Whether the edited context is persisted as active. Bindings are saved
   * immediately, independently of the editor's Save, so this must be the
   * stored state — never the unsaved draft switch. Active with no bindings
   * means the context applies globally, so the empty state and the
   * last-binding removal must say so instead of hiding the effect.
   */
  contextActive?: boolean;
}

/**
 * "Scope assignments" for a config context: what the context attaches to.
 * Targets inside one scope type are combined with OR, different scope types
 * must all match (AND). A context with no bindings is global — for an active
 * context that is an explicit warning, never a silent state. Add a binding by
 * picking a scope type then the target entity (bound by id via
 * {@link EntitySelect}, asserted as a UUID before POST); remove a binding
 * inline, confirming when the removal is the active context's last one. All
 * mutations refetch through TanStack Query so the list and the rendered-context
 * panels stay in sync.
 */
const ConfigContextScopeBindings = ({ contextId, contextActive = true }: ConfigContextScopeBindingsProps) => {
  const { triggerProps } = useInventoryRowActionFocus();
  const { can } = usePermissions();
  const canUpdate = can("UPDATE", "CONFIG_CONTEXT");
  const { t } = useTranslation();
  const qc = useQueryClient();
  const [sortState, setSortState] = useState<{ key: string; dir: "asc" | "desc" } | null>(null);
  const [columnFilters, setColumnFilters] = useState<Record<string, string[]>>({});
  const bindingSortKeys = CONFIG_CONTEXT_BINDINGS_TABLE_CONFIG.sortKeys;
  const bindingFilterKeys = useMemo(
    () => CONFIG_CONTEXT_BINDINGS_TABLE_CONFIG.filterColumns.map((column) => column.colKey),
    [],
  );
  const sanitizedSortState = useMemo(
    () => sanitizeDataTableSortState(sortState, bindingSortKeys),
    [bindingSortKeys, sortState],
  );
  const sanitizedColumnFilters = useMemo(
    () => sanitizeDataTableColumnFilters(columnFilters, bindingFilterKeys),
    [bindingFilterKeys, columnFilters],
  );
  const bindingFilterParams = useMemo(
    () => buildConfigContextBindingsFilterParams(sanitizedColumnFilters),
    [sanitizedColumnFilters],
  );
  const hasActiveFilters = useMemo(
    () => Object.values(sanitizedColumnFilters).some((values) => values.length > 0),
    [sanitizedColumnFilters],
  );
  const bindingQueryParams = useMemo(
    () => ({
      sort: sanitizedSortState?.key,
      order: sanitizedSortState?.dir,
      ...bindingFilterParams,
    }),
    [bindingFilterParams, sanitizedSortState],
  );

  const bindingsQuery = useQuery({
    queryKey: ["config-context-bindings", contextId, bindingQueryParams],
    queryFn: () =>
      listBindings(contextId, {
        sort: bindingQueryParams.sort,
        order: bindingQueryParams.order,
        scopeType: bindingFilterParams.scope_type,
      }),
  });
  const bindings = useMemo(() => bindingsQuery.data?.items ?? [], [bindingsQuery.data]);

  // Resolve scope target names: fetch the option list only for the scope-type
  // entities actually present in the current bindings plus the one being added.
  const [addScopeType, setAddScopeType] = useState<string>("");
  const [addScopeId, setAddScopeId] = useState<string>("");

  const neededEntities = useMemo(() => {
    const set = new Set<InventoryEntityKey>();
    for (const b of bindings) {
      const entity = ENTITY_BY_SCOPE_TYPE.get(b.scope_type);
      if (entity) set.add(entity);
    }
    const addEntity = ENTITY_BY_SCOPE_TYPE.get(addScopeType);
    if (addEntity) set.add(addEntity);
    return Array.from(set);
  }, [bindings, addScopeType]);

  const optionQueries = useQueries({
    queries: neededEntities.map(inventoryResourceOptionsQuery),
  });

  // entity → (id → resource) for display + the add picker.
  const resourceByEntity = useMemo(() => {
    const map = new Map<InventoryEntityKey, Map<string, Resource>>();
    neededEntities.forEach((entity, idx) => {
      const inner = new Map<string, Resource>();
      for (const r of optionQueries[idx]?.data?.items ?? []) inner.set(r.id, r);
      map.set(entity, inner);
    });
    return map;
  }, [neededEntities, optionQueries]);

  const resolveResource = useCallback((scopeType: string, scopeId: string): Resource | undefined => {
    const entity = ENTITY_BY_SCOPE_TYPE.get(scopeType);
    if (!entity) return undefined;
    return resourceByEntity.get(entity)?.get(scopeId);
  }, [resourceByEntity]);
  const resolveName = useCallback(
    (scopeType: string, scopeId: string): string | undefined =>
      resolveResource(scopeType, scopeId)?.name,
    [resolveResource],
  );

  const addEntity = ENTITY_BY_SCOPE_TYPE.get(addScopeType);
  const addEntityIdx = addEntity ? neededEntities.indexOf(addEntity) : -1;
  const addOptionsLoading = addEntityIdx >= 0 && (optionQueries[addEntityIdx]?.isPending ?? false);
  const addOptions: EntityOption[] = useMemo(() => {
    if (addEntityIdx < 0) return [];
    return (optionQueries[addEntityIdx]?.data?.items ?? []).map((r) => ({
      id: r.id,
      label: r.name,
    }));
  }, [addEntityIdx, optionQueries]);

  const refreshBindings = useCallback(async () => {
    // Deleting a binding is a two-step operation: the DELETE itself and the
    // authoritative list refetch that supplies the next total. Keep the
    // mutation pending until the refetch settles so a second removal cannot
    // read the pre-delete total. A failed refetch leaves the bindings query in
    // its error state: the rows are hidden and removals stay blocked until a
    // successful retry makes the count authoritative again.
    try {
      await qc.invalidateQueries({ queryKey: ["config-context-bindings", contextId] });
    } catch {
      // Fail closed: the query error state owns the retry, not a stale count.
    }
    // The rendered config context on any device/VM detail may now differ.
    void qc.invalidateQueries({ queryKey: ["inventory", "config-context"] });
  }, [contextId, qc]);

  const createMutation = useApiMutation({
    mutationFn: (body: { scope_type: string; scope_id: string }) =>
      createBinding({ context_id: contextId, ...body }),
    onSuccess: async () => {
      await refreshBindings();
      setAddScopeType("");
      setAddScopeId("");
      notify("success", t("configContexts.scope.added"));
    },
    onError: (err: ApiError) => notify("error", err.message || t("unknownError")),
  });

  const deleteMutation = useApiMutation({
    mutationFn: (id: string) => deleteBinding(id),
    onSuccess: async () => {
      await refreshBindings();
      notify("success", t("configContexts.scope.removed"));
    },
    onError: (err: ApiError) => notify("error", err.message || t("unknownError")),
  });

  // The table query is the only source of the binding total. During a fetch
  // or after a failed fetch it is not authoritative, so removals stay
  // conservative instead of trusting a stale count.
  const countTrusted =
    !hasActiveFilters && !bindingsQuery.isFetching && !bindingsQuery.isError;
  const totalBindings = bindingsQuery.data?.total ?? 0;
  const [deleteRequest, setDeleteRequest] = useState<{
    binding: ConfigContextBinding;
    lastBinding: boolean;
  } | null>(null);

  const requestRemoval = useCallback((binding: ConfigContextBinding) => {
    if (!contextActive) {
      deleteMutation.mutate(binding.id);
      return;
    }
    if (countTrusted && totalBindings > 1) {
      deleteMutation.mutate(binding.id);
      return;
    }
    // The count is authoritative and equals one, or it is not authoritative
    // at all (filtered/fetching/error): never remove an active context's
    // possible last binding without the global-change confirmation.
    setDeleteRequest({
      binding,
      lastBinding: countTrusted && totalBindings === 1,
    });
  }, [contextActive, countTrusted, totalBindings, deleteMutation]);

  const handleAdd = () => {
    if (!canUpdate) return;
    if (!addScopeType || !addScopeId) return;
    // The id is bound from EntitySelect option ids (entity UUIDs); assert at the
    // write boundary so a non-id never reaches the backend as `scope_id`.
    const scopeId = assertUuid(addScopeId, "scope_id");
    createMutation.mutate({ scope_type: addScopeType, scope_id: scopeId });
  };

  const scopeTypeLabel = useCallback(
    (token: string): string => t(`configContexts.scope.types.${token}`, { defaultValue: token }),
    [t],
  );
  const scopeTypeFilterOptions = useMemo(
    () =>
      OFFERED_SCOPE_TYPES.map((token) => ({
        value: token,
        label: scopeTypeLabel(token),
      })),
    [scopeTypeLabel],
  );

  const bindingColumns: Column<ConfigContextBinding>[] = useMemo(
    () => [
      {
        key: "scope_type",
        label: t("configContexts.scope.scopeType"),
        sortable: true,
        filterable: true,
        filterOptions: scopeTypeFilterOptions,
        getSortValue: (binding) => scopeTypeLabel(binding.scope_type),
        render: (binding) => (
          <span className="surface-chip rounded px-1.5 py-0.5 text-xs font-medium uppercase tracking-wide text-muted-foreground">
            {scopeTypeLabel(binding.scope_type)}
          </span>
        ),
      },
      {
        key: "scope_id",
        label: t("configContexts.scope.target"),
        sortable: true,
        getSortValue: (binding) =>
          resolveName(binding.scope_type, binding.scope_id) ?? binding.scope_id,
        render: (binding) => {
          const known = ENTITY_BY_SCOPE_TYPE.has(binding.scope_type);
          const name = resolveName(binding.scope_type, binding.scope_id);
          const entity = ENTITY_BY_SCOPE_TYPE.get(binding.scope_type);
          const resolving =
            known && entity
              ? (optionQueries[neededEntities.indexOf(entity)]?.isPending ?? false)
              : false;
          if (!known) {
            return (
              <span className="font-mono text-xs text-muted-foreground">
                {binding.scope_id}
              </span>
            );
          }
          if (resolving) return <span className="text-muted-foreground">{t("loading")}</span>;
          if (name) {
            const resource = resolveResource(binding.scope_type, binding.scope_id);
            return (
              <span className="text-xs text-foreground">
                <EntityOptionLabel
                  label={name}
                  color={binding.scope_type === "tag" ? resource?.color : undefined}
                />
              </span>
            );
          }
          return (
            <span className="italic text-muted-foreground">
              {t("inventory.resources.deletedObject")}
            </span>
          );
        },
      },
      {
        key: "actions",
        label: "",
        excludeFromSettings: true,
        className: "text-right",
        sortable: false,
        render: (binding) => canUpdate ? (
          <RowActions label={`${t("actions")}: ${scopeTypeLabel(binding.scope_type)} · ${resolveName(binding.scope_type, binding.scope_id) || binding.scope_id}`} {...triggerProps(binding.id)}>
            <DropdownMenuItem
              variant="destructive"
              disabled={deleteMutation.isPending}
              onSelect={() => requestRemoval(binding)}
            >
              <Trash2 className="size-4" />{t("remove")}
            </DropdownMenuItem>
          </RowActions>
        ) : null,
      },
    ],
    [
      deleteMutation,
      canUpdate,
      triggerProps,
      neededEntities,
      optionQueries,
      resolveName,
      resolveResource,
      requestRemoval,
      scopeTypeFilterOptions,
      scopeTypeLabel,
      t,
    ],
  );

  return (
    <section aria-label={t("configContexts.scope.section")} className="flex min-w-0 flex-col gap-4">
      <div className="flex flex-col gap-1">
        <h2 className="text-base font-semibold">{t("configContexts.scope.section")}</h2>
        <p className="text-sm text-muted-foreground">{t("configContexts.scope.sectionHint")}</p>
      </div>

      {/* Current bindings */}
      {bindingsQuery.isPending ? (
        <div className="flex items-center justify-center py-4">
          <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
        </div>
      ) : !bindingsQuery.isError && bindings.length === 0 && !hasActiveFilters ? (
        <div className={contextActive
          ? "flex items-start gap-2 rounded-lg border border-status-warning/30 bg-status-warning/5 px-3 py-2"
          : "flex items-start gap-2 rounded-lg border border-border bg-accent/10 px-3 py-2"}>
          <Globe className={contextActive
            ? "mt-0.5 h-3.5 w-3.5 shrink-0 text-status-warning"
            : "mt-0.5 h-3.5 w-3.5 shrink-0 text-muted-foreground"} />
          <p className="text-xs text-muted-foreground leading-snug">
            {contextActive
              ? t("configContexts.scope.appliesGlobally")
              : t("configContexts.scope.globalWhenActive")}
          </p>
        </div>
      ) : (
        <DataTable
          tableId="inventory.config-context-bindings"
          columns={bindingColumns}
          data={bindings}
          getRowKey={(binding) => binding.id}
          error={bindingsQuery.isError ? bindingsQuery.error : undefined}
          onRetry={() => bindingsQuery.refetch()}
          density="compact"
          virtualizeRows={false}
          sortState={sanitizedSortState}
          onSortChange={setSortState}
          columnFilters={sanitizedColumnFilters}
          onColumnFilterChange={(key, values) =>
            setColumnFilters((current) => updateDataTableColumnFilters(current, key, values))
          }
          emptyState={
            <div className="px-3 py-6 text-center text-xs text-muted-foreground">
              {t("noResults")}
            </div>
          }
        />
      )}

      {/* Add binding */}
      {canUpdate && <div className="grid grid-cols-1 gap-2 sm:grid-cols-[150px_1fr_auto] sm:items-end">
        <FormSelect
          label={t("configContexts.scope.scopeType")}
          labelClassName="text-sm"
          value={addScopeType}
          onValueChange={(v) => {
            setAddScopeType(v);
            setAddScopeId("");
          }}
          options={OFFERED_SCOPE_TYPES.map((token) => ({
            value: token,
            label: scopeTypeLabel(token),
          }))}
          placeholder={t("select")}
          className="space-y-1"
        />
        <FormField
          label={t("configContexts.scope.target")}
          labelClassName="text-sm"
          className="space-y-1"
        >
          <EntitySelect
            items={addOptions}
            value={addScopeId}
            onValueChange={setAddScopeId}
            placeholder={
              addOptionsLoading ? t("loading") : t("configContexts.scope.targetPlaceholder")
            }
            searchPlaceholder={t("search")}
            disabled={!addScopeType || addOptionsLoading}
            aria-label={t("configContexts.scope.target")}
          />
        </FormField>
        <Button
          type="button"
          size="sm"
          className="h-8 gap-1.5"
          disabled={!addScopeType || !addScopeId || createMutation.isPending}
          onClick={handleAdd}
        >
          {createMutation.isPending ? (
            <Loader2 className="h-3 w-3 animate-spin" />
          ) : (
            <Plus className="h-3 w-3" />
          )}
          {t("add")}
        </Button>
      </div>}

      {deleteRequest && (
        <AlertDialog open onOpenChange={(open) => { if (!open) setDeleteRequest(null); }}>
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>
                {t(deleteRequest.lastBinding
                  ? "configContexts.scope.removeLastTitle"
                  : "configContexts.scope.removeUncertainTitle")}
              </AlertDialogTitle>
              <AlertDialogDescription>
                {t(deleteRequest.lastBinding
                  ? "configContexts.scope.removeLastDescription"
                  : "configContexts.scope.removeUncertainDescription")}
              </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel>{t("cancel")}</AlertDialogCancel>
              <AlertDialogAction
                className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
                disabled={deleteMutation.isPending}
                onClick={() => {
                  deleteMutation.mutate(deleteRequest.binding.id);
                  setDeleteRequest(null);
                }}
              >
                {t(deleteRequest.lastBinding
                  ? "configContexts.scope.removeLastConfirm"
                  : "configContexts.scope.removeUncertainConfirm")}
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      )}
    </section>
  );
};

export default ConfigContextScopeBindings;
