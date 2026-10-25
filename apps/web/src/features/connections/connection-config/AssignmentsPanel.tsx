import { useCallback, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { Plus, Pencil, Trash2, Globe, AlertCircle, Info, ServerCog } from "lucide-react";
import { useApiMutation } from "@/shared/api/hooks";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { listResources } from "@/features/inventory/api";
import type { ApiError } from "@/shared/api/client";
import { notify } from "@/shared/lib/notifications";
import DataTable, {
  type Column,
  type DataTableFilterOption,
  type DataTableSortState,
} from "@/shared/ui-shared/DataTable";
import { boolFilterOptions } from "@/shared/ui-shared/dataTableLocal";
import PageState from "@/shared/ui-shared/PageState";
import EmptyState from "@/shared/ui-shared/EmptyState";
import EntityRef from "@/shared/ui-shared/EntityRef";
import StatusChip from "@/shared/ui-shared/StatusChip";
import RowActions from "@/shared/ui-shared/RowActions";
import { DropdownMenuItem } from "@/shared/ui-primitives/dropdown-menu";
import {
  sanitizeDataTableColumnFilters,
  sanitizeDataTableSortState,
  updateDataTableColumnFilters,
  mapDataTableSortKey,
} from "@/shared/ui-shared/tableState";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import {
  CONNCONF_SCOPE_TYPES,
  createAssignment,
  deleteAssignment,
  listAssignments,
  updateAssignment,
  type ConnConfAssignment,
  type ConnConfScopeType,
} from "./api";
import { SCOPE_ENTITY, SCOPE_I18N_KEY } from "./scopeCatalog";
import AssignmentDialog from "./AssignmentDialog";
import AssignToTargetsDialog from "./AssignToTargetsDialog";
import {
  buildConnectionConfigAssignmentFilterParams,
  CONNECTION_CONFIG_ASSIGNMENTS_TABLE_CONFIG,
} from "./tableConfig";

function formatDateTime(value?: string | null): string {
  if (!value) return "—";
  return new Date(value).toLocaleString();
}

const AssignmentsPanel = ({
  recordId,
}: {
  recordId: string;
}) => {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const { can } = usePermissions();
  const canCreate = can("CREATE", "CONFIG_CONTEXT");
  const canUpdate = can("UPDATE", "CONFIG_CONTEXT");
  const canDelete = can("DELETE", "CONFIG_CONTEXT");

  // The add/edit form lives in a focused modal so it never competes with the
  // record editor. `dialogTarget` is null in create mode, the assignment in
  // edit mode; `dialogOpen` drives the modal independently.
  const [dialogOpen, setDialogOpen] = useState(false);
  const [bulkDialogOpen, setBulkDialogOpen] = useState(false);
  const [dialogTarget, setDialogTarget] = useState<ConnConfAssignment | null>(null);
  const actionTriggers = useRef(new Map<string, HTMLButtonElement>());
  const [sortState, setSortState] = useState<DataTableSortState | null>(null);
  const [columnFilters, setColumnFilters] = useState<Record<string, string[]>>({});

  const sanitizedSortState = useMemo(
    () =>
      sanitizeDataTableSortState(
        sortState,
        CONNECTION_CONFIG_ASSIGNMENTS_TABLE_CONFIG.sortKeys,
      ),
    [sortState],
  );
  const sanitizedColumnFilters = useMemo(
    () =>
      sanitizeDataTableColumnFilters(
        columnFilters,
        CONNECTION_CONFIG_ASSIGNMENTS_TABLE_CONFIG.filterColumns.map((column) => column.colKey),
      ),
    [columnFilters],
  );
  const assignmentFilterParams = useMemo(
    () => buildConnectionConfigAssignmentFilterParams(sanitizedColumnFilters),
    [sanitizedColumnFilters],
  );

  const assignmentsQuery = useQuery({
    queryKey: [
      "connection-config",
      "assignments",
      recordId,
      sanitizedSortState,
      assignmentFilterParams,
    ],
    queryFn: () =>
      listAssignments({
        record_id: recordId,
        sort: mapDataTableSortKey(
          sanitizedSortState,
          CONNECTION_CONFIG_ASSIGNMENTS_TABLE_CONFIG.sortKeyByColumn,
        ),
        order: sanitizedSortState?.dir,
        scope_type: assignmentFilterParams.scope_type,
        scope_id: assignmentFilterParams.scope_id,
        enabled: assignmentFilterParams.enabled,
      }),
  });
  const assignments = useMemo(
    () => assignmentsQuery.data ?? [],
    [assignmentsQuery.data],
  );

  // Close the dialog whenever the selected record changes, so a stale editor
  // never carries over. Done during render (React's recommended pattern).
  const [prevRecordId, setPrevRecordId] = useState(recordId);
  if (prevRecordId !== recordId) {
    setPrevRecordId(recordId);
    setDialogOpen(false);
    setBulkDialogOpen(false);
    setDialogTarget(null);
    setSortState(null);
    setColumnFilters({});
  }

  // Eagerly load every scope entity so scope_id → name resolves in the list.
  const targetableScopes = CONNCONF_SCOPE_TYPES.filter((s) => s !== "global");
  const targetQueries = useQueries({
    queries: targetableScopes.map((scope) => ({
      queryKey: ["inventory", "connconf-scope", SCOPE_ENTITY[scope]],
      queryFn: () => listResources(SCOPE_ENTITY[scope], { limit: 1000, offset: 0 }),
      staleTime: 60_000,
    })),
  });
  const optionsByScope = useMemo(() => {
    const map = {} as Record<Exclude<ConnConfScopeType, "global">, { id: string; name: string }[]>;
    targetableScopes.forEach((scope, index) => {
      map[scope] = (targetQueries[index]?.data?.items ?? []).map((r) => ({
        id: r.id,
        name: r.name,
      }));
    });
    return map;
  }, [targetableScopes, targetQueries]);

  const scopeResolved = useCallback(
    (
      type: string,
      id: string | null | undefined,
    ): { name: string | undefined; id: string } | null => {
      if (type === "global") return null;
      if (!id) return null;
      const scope = type as Exclude<ConnConfScopeType, "global">;
      const opts = optionsByScope[scope] ?? [];
      const match = opts.find((o) => o.id === id)?.name;
      // While still loading, treat as unresolved (EntityRef will show short-id)
      return { name: match, id };
    },
    [optionsByScope],
  );
  const scopeFilterOptions = useMemo<DataTableFilterOption[]>(
    () =>
      targetableScopes.flatMap((scope) =>
        (optionsByScope[scope] ?? []).map((option) => ({
          value: option.id,
          label: `${t(`connectionConfig.scope.${SCOPE_I18N_KEY[scope]}`)} · ${option.name}`,
        })),
      ),
    [optionsByScope, t, targetableScopes],
  );
  const filterOptionsByKey = useMemo<Record<string, DataTableFilterOption[]>>(
    () => ({
      scopeType: CONNCONF_SCOPE_TYPES.map((scope) => ({
        value: scope,
        label:
          scope === "global"
            ? t("connectionConfig.scope.global")
            : t(`connectionConfig.scope.${SCOPE_I18N_KEY[scope]}`),
      })),
      scope: scopeFilterOptions,
      enabled: boolFilterOptions({
        trueLabel: t("enabled"),
        falseLabel: t("disabled"),
      }),
    }),
    [scopeFilterOptions, t],
  );

  const invalidate = () =>
    qc.invalidateQueries({ queryKey: ["connection-config", "assignments", recordId] });

  const applyGlobalMutation = useApiMutation({
    mutationFn: () =>
      createAssignment({
        record_id: recordId,
        scope_type: "global",
        scope_id: null,
        weight: 0,
      }),
    onSuccess: () => {
      void invalidate();
      notify("success", t("connectionConfig.assignments.added"));
    },
    onError: (err: ApiError) =>
      notify("error", err.message || t("connectionConfig.assignments.addFailed")),
  });

  const reweightMutation = useApiMutation({
    mutationFn: ({ id, value }: { id: string; value: number }) =>
      updateAssignment(id, { weight: value }),
    onSuccess: () => void invalidate(),
    onError: (err: ApiError) => notify("error", err.message),
  });

  const removeMutation = useApiMutation({
    mutationFn: (id: string) => deleteAssignment(id),
    onSuccess: () => {
      void invalidate();
      notify("success", t("connectionConfig.assignments.removed"));
    },
    onError: (err: ApiError) => notify("error", err.message),
  });

  const openCreate = useCallback(() => {
    setDialogTarget(null);
    setDialogOpen(true);
  }, []);

  const openEdit = useCallback((assignment: ConnConfAssignment) => {
    setDialogTarget(assignment);
    setDialogOpen(true);
  }, []);

  const columns = useMemo<Column<ConnConfAssignment>[]>(
    () => [
      {
        key: "scopeType",
        label: t("connectionConfig.assignments.scopeType"),
        sortable: true,
        filterable: true,
        filterOptions: filterOptionsByKey.scopeType,
        render: (assignment) => {
          const isGlobal = assignment.scope_type === "global";
          return (
            <span className="inline-flex items-center gap-1 rounded-md bg-primary/15 px-2 py-0.5 text-xs font-semibold text-primary">
              {isGlobal ? <Globe className="h-3 w-3" /> : <span className="h-1.5 w-1.5 rounded-full bg-primary" />}
              {t(
                `connectionConfig.scope.${SCOPE_I18N_KEY[assignment.scope_type as ConnConfScopeType] ?? "global"}`,
              )}
            </span>
          );
        },
      },
      {
        key: "scope",
        label: t("connectionConfig.assignments.target"),
        sortable: true,
        filterable: true,
        filterOptions: filterOptionsByKey.scope,
        render: (assignment) => {
          if (assignment.scope_type === "global") {
            return t("connectionConfig.scope.appliesAll");
          }
          const resolved = scopeResolved(assignment.scope_type, assignment.scope_id);
          return resolved ? <EntityRef name={resolved.name} id={resolved.id} /> : "—";
        },
      },
      {
        key: "weight",
        label: t("connectionConfig.assignments.weight"),
        sortable: true,
        render: (assignment) => (
          <Input
            key={`${assignment.id}:${assignment.weight}`}
            type="number"
            aria-label={t("connectionConfig.assignments.weight")}
            className="h-8 w-20 text-xs text-center font-mono"
            defaultValue={assignment.weight}
            disabled={!canUpdate || reweightMutation.isPending}
            onBlur={(event) => {
              const value = Number(event.target.value) || 0;
              if (value !== assignment.weight) {
                reweightMutation.mutate({ id: assignment.id, value });
              }
            }}
          />
        ),
      },
      {
        key: "enabled",
        label: t("enabled"),
        sortable: true,
        filterable: true,
        filterOptions: filterOptionsByKey.enabled,
        render: (assignment) => (
          <StatusChip
            status={assignment.enabled ? "positive" : "neutral"}
            label={assignment.enabled ? t("enabled") : t("disabled")}
          />
        ),
      },
      {
        key: "createdAt",
        label: t("createdAt", { defaultValue: "Created" }),
        sortable: true,
        render: (assignment) => formatDateTime(assignment.created_at),
      },
      {
        key: "actions",
        label: "",
        excludeFromSettings: true,
        className: "w-12",
        render: (assignment) => canUpdate || canDelete ? (
          <RowActions
            label={`${t("actions")}: ${assignment.scope_type === "global"
              ? t("connectionConfig.scope.appliesAll")
              : scopeResolved(assignment.scope_type, assignment.scope_id)?.name ?? assignment.scope_id ?? assignment.id}`}
            ref={(node) => {
              if (node) actionTriggers.current.set(assignment.id, node);
              else actionTriggers.current.delete(assignment.id);
            }}
          >
            {canUpdate && (
              <DropdownMenuItem onSelect={() => openEdit(assignment)}>
                <Pencil aria-hidden="true" />{t("edit")}
              </DropdownMenuItem>
            )}
            {canDelete && (
              <DropdownMenuItem variant="destructive" onSelect={() => removeMutation.mutate(assignment.id)}>
                <Trash2 aria-hidden="true" />{t("connectionConfig.assignments.remove")}
              </DropdownMenuItem>
            )}
          </RowActions>
        ) : null,
      },
    ],
    [canDelete, canUpdate, filterOptionsByKey.enabled, filterOptionsByKey.scope, filterOptionsByKey.scopeType, openEdit, reweightMutation, removeMutation, scopeResolved, t],
  );

  const hasActiveFilters = Object.keys(sanitizedColumnFilters).length > 0;
  const isEmpty = !assignmentsQuery.isPending && !assignmentsQuery.isError && assignments.length === 0;
  const showGuidedEmpty = isEmpty && !hasActiveFilters;

  return (
    <section aria-label={t("connectionConfig.assignments.title")} className="grid min-w-0 gap-4">
      <p className="text-sm text-muted-foreground">{t("connectionConfig.assignments.whatIsHint")}</p>

      <div className="flex flex-col gap-4">
        {/* EMPTY STATE — guided primary path. The record does nothing yet, so
            lead the user: one-click global, or open the assignment dialog. */}
        {showGuidedEmpty && (
          <div className="rounded-lg border border-primary/30 bg-primary/5 px-3 py-3">
            <div className="flex items-start gap-2">
              <Info className="mt-0.5 h-3.5 w-3.5 shrink-0 text-primary" aria-hidden />
              <div className="min-w-0 flex-1">
                <p className="text-xs font-medium text-foreground">
                  {t("connectionConfig.assignments.notAppliedTitle")}
                </p>
                  <p className="mt-0.5 text-xs leading-relaxed text-muted-foreground">
                    {t("connectionConfig.assignments.notAppliedHint")}
                  </p>
                <div className="mt-2.5 flex flex-wrap items-center gap-2">
                  {canCreate && (
                    <Button
                      size="sm"
                      className="h-9 gap-1 text-xs"
                      disabled={applyGlobalMutation.isPending}
                      onClick={() => applyGlobalMutation.mutate()}
                    >
                      <Globe className="h-3 w-3" />
                      {t("connectionConfig.assignments.applyGlobally")}
                    </Button>
                  )}
                  {canCreate && (
                    <Button
                      variant="outline"
                      size="sm"
                      className="h-9 gap-1 text-xs"
                      onClick={() => setBulkDialogOpen(true)}
                    >
                      <ServerCog className="h-3 w-3" />
                      {t("connectionConfig.bulkAssign.open")}
                    </Button>
                  )}
                  {canCreate && (
                    <Button
                      variant="outline"
                      size="sm"
                      className="h-9 gap-1 text-xs"
                      onClick={openCreate}
                    >
                      <Plus className="h-3 w-3" />
                      {t("connectionConfig.assignments.pickScopeToggle")}
                    </Button>
                  )}
                </div>
              </div>
            </div>
          </div>
        )}

        {/* NON-EMPTY STATE — the existing bindings are the focus; adding opens
            a dedicated modal. */}
        {!assignmentsQuery.isError && (!showGuidedEmpty || assignmentsQuery.isPending) && (
          <>
            <DataTable
              tableId="connection-config.assignments"
              data={assignments}
              columns={columns}
              getRowKey={(assignment) => assignment.id}
              sortState={sanitizedSortState}
              onSortChange={setSortState}
              columnFilters={sanitizedColumnFilters}
              onColumnFilterChange={(key, values) => {
                setColumnFilters((prev) => updateDataTableColumnFilters(prev, key, values));
              }}
              loading={assignmentsQuery.isPending}
              error={assignmentsQuery.error}
              onRetry={() => assignmentsQuery.refetch()}
              toolbar={
                <div
                  className="flex flex-wrap items-center justify-between gap-2"
                >
                  <p
                    className="text-sm font-medium text-muted-foreground"
                  >
                    {t("connectionConfig.assignments.currentHeading")}
                  </p>
                  <div className="flex flex-wrap items-center justify-end gap-2">
                    {canCreate && (
                      <Button
                        variant="outline"
                        size="sm"
                        className="h-8 gap-1 text-xs"
                        disabled={applyGlobalMutation.isPending}
                        onClick={() => applyGlobalMutation.mutate()}
                      >
                        <Globe className="h-3 w-3" />
                        {t("connectionConfig.assignments.applyGlobally")}
                      </Button>
                    )}
                    {canCreate && (
                      <Button
                        variant="outline"
                        size="sm"
                        className="h-8 gap-1 text-xs"
                        onClick={() => setBulkDialogOpen(true)}
                      >
                        <ServerCog className="h-3 w-3" />
                        {t("connectionConfig.bulkAssign.open")}
                      </Button>
                    )}
                    {canCreate && (
                      <Button
                        size="sm"
                        className="h-8 gap-1 text-xs"
                        onClick={openCreate}
                      >
                        <Plus className="h-3 w-3" />
                        {t("connectionConfig.assignments.addToggle")}
                      </Button>
                    )}
                  </div>
                </div>
              }
              emptyState={
                <EmptyState
                  icon={AlertCircle}
                  title={t("connectionConfig.assignments.empty")}
                />
              }
            />

          </>
        )}

        {assignmentsQuery.isError ? (
          <PageState kind="error" title={t("connectionConfig.assignments.loadError")} />
        ) : null}
      </div>

      <AssignmentDialog
        open={dialogOpen}
        onOpenChange={(open) => {
          setDialogOpen(open);
          if (!open && dialogTarget) {
            const trigger = actionTriggers.current.get(dialogTarget.id);
            requestAnimationFrame(() => trigger?.focus());
          }
        }}
        recordId={recordId}
        assignment={dialogTarget}
      />
      <AssignToTargetsDialog
        open={bulkDialogOpen}
        onOpenChange={setBulkDialogOpen}
        recordId={recordId}
      />
    </section>
  );
};

export default AssignmentsPanel;
