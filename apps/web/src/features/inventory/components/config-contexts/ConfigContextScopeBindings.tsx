import { useCallback, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { Plus, Trash2, Loader2, AlertTriangle } from "lucide-react";
import FormSection from "@/shared/ui-shared/FormSection";
import RowActions from "@/shared/ui-shared/RowActions";
import { useInventoryRowActionFocus } from "@/features/inventory/useInventoryRowActionFocus";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { DropdownMenuItem } from "@/shared/ui-primitives/dropdown-menu";
import { useApiMutation } from "@/shared/api/hooks";
import type { ApiError } from "@/shared/api/client";
import { notify } from "@/shared/lib/notifications";
import { assertUuid } from "@/shared/lib/ids";
import type { InventoryEntityKey } from "@/shared/api/types";
import { listResources } from "@/features/inventory/api";
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

/**
 * The config-context scope types the backend resolver actually honors for a
 * device/VM rendered context, mapped to the inventory entity that supplies the
 * target options. This is the *honored* set, NOT every token the create
 * endpoint accepts: the resolver (`config_context_repository.list_for_scope_
 * hierarchy`) only pushes `device | vm | site | tenant | role | cluster |
 * tenant_group` into a device/VM scope set — a binding on any other token is
 * accepted but never matches, i.e. a silent no-op. We therefore only offer the
 * ones that take effect. Ordered low→high precedence to match resolution order.
 */
const SCOPE_TYPE_ENTITY: ReadonlyArray<readonly [string, InventoryEntityKey]> = [
  ["tenant_group", "Tenant Groups"],
  ["tenant", "Tenants"],
  ["site", "Sites"],
  ["cluster", "Clusters"],
  ["role", "Device Roles"],
  ["device", "Devices"],
  ["vm", "Virtual Machines"],
];

const ENTITY_BY_SCOPE_TYPE = new Map<string, InventoryEntityKey>(SCOPE_TYPE_ENTITY);
const HONORED_SCOPE_TYPES = SCOPE_TYPE_ENTITY.map(([token]) => token);

interface ConfigContextScopeBindingsProps {
  /** The config context whose bindings are managed. */
  contextId: string;
}

/**
 * "Scope assignments" for a config context: what the context attaches to. A
 * context with no bindings applies nowhere (surfaced as an explicit empty
 * state). Add a binding by picking a scope type then the target entity (bound
 * by id via {@link EntitySelect}, asserted as a UUID before POST); remove a
 * binding inline. All mutations refetch through TanStack Query so the list and
 * the rendered-context panels stay in sync.
 */
const ConfigContextScopeBindings = ({ contextId }: ConfigContextScopeBindingsProps) => {
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
    queries: neededEntities.map((entity) => ({
      queryKey: ["inventory", "resources", entity, "options"],
      queryFn: () => listResources(entity),
    })),
  });

  // entity → (id → name) for display + the add picker.
  const nameByEntity = useMemo(() => {
    const map = new Map<InventoryEntityKey, Map<string, string>>();
    neededEntities.forEach((entity, idx) => {
      const inner = new Map<string, string>();
      for (const r of optionQueries[idx]?.data?.items ?? []) inner.set(r.id, r.name);
      map.set(entity, inner);
    });
    return map;
  }, [neededEntities, optionQueries]);

  const resolveName = useCallback((scopeType: string, scopeId: string): string | undefined => {
    const entity = ENTITY_BY_SCOPE_TYPE.get(scopeType);
    if (!entity) return undefined;
    return nameByEntity.get(entity)?.get(scopeId);
  }, [nameByEntity]);

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

  const invalidate = () => {
    void qc.invalidateQueries({ queryKey: ["config-context-bindings", contextId] });
    // The rendered config context on any device/VM detail may now differ.
    void qc.invalidateQueries({ queryKey: ["inventory", "config-context"] });
  };

  const createMutation = useApiMutation({
    mutationFn: (body: { scope_type: string; scope_id: string }) =>
      createBinding({ context_id: contextId, ...body }),
    onSuccess: () => {
      invalidate();
      setAddScopeType("");
      setAddScopeId("");
      notify("success", t("configContexts.scope.added"));
    },
    onError: (err: ApiError) => notify("error", err.message || t("unknownError")),
  });

  const deleteMutation = useApiMutation({
    mutationFn: (id: string) => deleteBinding(id),
    onSuccess: () => {
      invalidate();
      notify("success", t("configContexts.scope.removed"));
    },
    onError: (err: ApiError) => notify("error", err.message || t("unknownError")),
  });

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
      HONORED_SCOPE_TYPES.map((token) => ({
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
          if (name) return <span className="text-xs text-foreground">{name}</span>;
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
            <DropdownMenuItem variant="destructive" disabled={deleteMutation.isPending} onSelect={() => deleteMutation.mutate(binding.id)}>
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
      scopeTypeFilterOptions,
      scopeTypeLabel,
      t,
    ],
  );

  return (
    <FormSection title={t("configContexts.scope.section")} description={t("configContexts.scope.sectionHint")} contentClassName="flex flex-col gap-4">

      {/* Current bindings */}
      {bindingsQuery.isPending ? (
        <div className="flex items-center justify-center py-4">
          <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
        </div>
      ) : !bindingsQuery.isError && bindings.length === 0 && !hasActiveFilters ? (
        <div className="flex items-start gap-2 rounded-lg border border-status-warning/30 bg-status-warning/5 px-3 py-2">
          <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0 text-status-warning" />
          <p className="text-xs text-muted-foreground leading-snug">
            {t("configContexts.scope.appliesNowhere")}
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
          options={HONORED_SCOPE_TYPES.map((token) => ({
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
    </FormSection>
  );
};

export default ConfigContextScopeBindings;
