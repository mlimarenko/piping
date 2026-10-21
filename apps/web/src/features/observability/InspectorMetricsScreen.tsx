import { type ReactNode, useMemo, useRef, useState } from "react";
import { useNavigate, useSearchParams } from "react-router";
import { useTranslation } from "react-i18next";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  BellRing,
  Cable,
  Database,
  Globe,
  Layers,
  ListChecks,
  Pencil,
  Plus,
  Sparkles,
  Trash2,
  type LucideIcon,
} from "lucide-react";

import { useApiMutation } from "@/shared/api/hooks";
import type { ApiError } from "@/shared/api/client";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { cn } from "@/shared/lib/utils";
import { notify } from "@/shared/lib/notifications";
import DataTable, { type Column } from "@/shared/ui-shared/DataTable";
import { PageActions } from "@/shared/ui-shared/PageChrome";
import RowActions from "@/shared/ui-shared/RowActions";
import { DropdownMenuItem } from "@/shared/ui-primitives/dropdown-menu";
import EmptyState from "@/shared/ui-shared/EmptyState";
import ConfirmDeleteDialog from "@/shared/ui-shared/ConfirmDeleteDialog";
import SectionTabs from "@/shared/ui-shared/SectionTabs";
import { Badge } from "@/shared/ui-primitives/badge";
import { Button } from "@/shared/ui-primitives/button";
import { Tabs, TabsContent } from "@/shared/ui-primitives/tabs";
import {
  createAlertSourceAssignment,
  createMetricSetAssignment,
  createQuerySourceAssignment,
  deleteAlertSource,
  deleteAlertSourceAssignment,
  deleteMetricQuery,
  deleteMetricSet,
  deleteMetricSetAssignment,
  deleteQuerySource,
  deleteQuerySourceAssignment,
  listAlertSourceAssignments,
  listAlertSources,
  listMetricQueries,
  listMetricSetAssignments,
  listMetricSets,
  listQuerySourceAssignments,
  listQuerySources,
  updateAlertSourceAssignment,
  updateMetricSetAssignment,
  updateQuerySourceAssignment,
  type AlertSourceAssignmentItem,
  type AlertSourceItem,
  type MetricQueryItem,
  type MetricSetAssignmentItem,
  type MetricSetDetail,
  type QuerySourceAssignmentItem,
  type QuerySourceItem,
} from "@/features/observability/api";
import ExporterPresetApplyDialog from "./ExporterPresetApplyDialog";
import SimpleAssignmentDialog, {
  type AssignmentDraft,
} from "./SimpleAssignmentDialog";
import InspectorPreviewPanel from "./InspectorPreviewPanel";
import { useScopeEntities } from "./useScopeEntities";
import {
  MONITORING_SCOPE_I18N_KEY,
  MONITORING_SCOPE_LABEL,
  type MonitoringScopeType,
} from "./scope";
import {
  ALERT_SOURCE_ASSIGNMENTS_KEY,
  ALERT_SOURCES_KEY,
  METRIC_QUERIES_KEY,
  METRIC_SET_ASSIGNMENTS_KEY as SET_ASSIGN_KEY,
  METRIC_SETS_KEY,
  QUERY_SOURCE_ASSIGNMENTS_KEY as QS_ASSIGN_KEY,
  QUERY_SOURCES_KEY,
  invalidateInspectorMetricCaches,
} from "./keys";
import { metricStatusMode } from "./metricDisplay";
import {
  INSPECTOR_VIEWS,
  OBSERVABILITY_ENTITY_ROUTES,
  observabilityEditHref,
  observabilityNewHref,
  observabilityRecordHref,
} from "./observabilityRoutes";

const QUERY_SOURCES = OBSERVABILITY_ENTITY_ROUTES.querySources;
const METRIC_QUERIES = OBSERVABILITY_ENTITY_ROUTES.metricQueries;
const METRIC_SETS = OBSERVABILITY_ENTITY_ROUTES.metricSets;
const ALERT_SOURCES = OBSERVABILITY_ENTITY_ROUTES.alertSources;

const EMPTY_ASSIGNMENT: AssignmentDraft = {
  targetId: "",
  scopeType: "global",
  scopeId: "",
  weight: "0",
  enabled: true,
};

type InspectorWorkbenchView = "profiles" | "cards" | "groups" | "sources" | "alerts";

type InspectorProfileRow = {
  key: string;
  scopeType: string;
  scopeId: string | null;
  sources: QuerySourceAssignmentItem[];
  sets: MetricSetAssignmentItem[];
};

function InspectorTableSummary({
  icon: Icon,
  title,
  description,
  action,
}: {
  icon: LucideIcon;
  title: string;
  description: string;
  action?: ReactNode;
}) {
  return (
    <>
      <div className="min-w-0">
        <h2 className="flex items-center gap-2 text-sm font-semibold text-foreground"><Icon className="size-4 text-primary" aria-hidden="true" />{title}</h2>
        <p className="max-w-2xl text-xs leading-4 text-muted-foreground">{description}</p>
      </div>
      {action && <PageActions>{action}</PageActions>}
    </>
  );
}

/**
 * Screen ② — "what to show in inspectors". Manages the inspector-side
 * configuration: data sources, metric queries, metric sets (with their items),
 * and the scope/host assignments that bind sources and sets. No exporters here.
 */
const InspectorMetricsScreen = () => {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const globalLabel = t("observability.scope.global", { defaultValue: "Global" });
  const entities = useScopeEntities(globalLabel);

  // View-but-not-edit: hide create/edit/delete affordances when the caller
  // lacks the matching MONITORING capability; inspecting/reading stays visible.
  const { can } = usePermissions();
  const canCreate = can("CREATE", "MONITORING");
  const canUpdate = can("UPDATE", "MONITORING");
  const canDelete = can("DELETE", "MONITORING");
  const [deleteTarget, setDeleteTarget] = useState<{ label: string; confirm: () => void } | null>(null);
  const rowTriggers = useRef(new Map<string, HTMLButtonElement>());
  const returnFocus = useRef<HTMLButtonElement | null>(null);

  const navigate = useNavigate();
  const [params, setParams] = useSearchParams();
  const viewParam = params.get("view");
  const workbenchView: InspectorWorkbenchView =
    viewParam && (INSPECTOR_VIEWS as readonly string[]).includes(viewParam)
      ? (viewParam as InspectorWorkbenchView)
      : "profiles";
  const setWorkbenchView = (next: InspectorWorkbenchView) => {
    setParams(
      (prev) => {
        const nextParams = new URLSearchParams(prev);
        nextParams.set("view", next);
        return nextParams;
      },
      { replace: true },
    );
  };
  const [qsAssignDialog, setQsAssignDialog] = useState<{
    open: boolean;
    editing: QuerySourceAssignmentItem | null;
  }>({ open: false, editing: null });
  const [setAssignDialog, setSetAssignDialog] = useState<{
    open: boolean;
    editing: MetricSetAssignmentItem | null;
  }>({ open: false, editing: null });
  const [alertAssignDialog, setAlertAssignDialog] = useState<{
    open: boolean;
    editing: AlertSourceAssignmentItem | null;
  }>({ open: false, editing: null });
  const [presetDialogOpen, setPresetDialogOpen] = useState(false);

  // ── Queries ────────────────────────────────────────────
  const sourcesQuery = useQuery({ queryKey: QUERY_SOURCES_KEY, queryFn: () => listQuerySources() });
  const queriesQuery = useQuery({ queryKey: METRIC_QUERIES_KEY, queryFn: () => listMetricQueries() });
  const setsQuery = useQuery({ queryKey: METRIC_SETS_KEY, queryFn: () => listMetricSets() });
  const qsAssignQuery = useQuery({
    queryKey: QS_ASSIGN_KEY,
    queryFn: () => listQuerySourceAssignments(),
  });
  const setAssignQuery = useQuery({
    queryKey: SET_ASSIGN_KEY,
    queryFn: () => listMetricSetAssignments(),
  });
  const alertSourcesQuery = useQuery({
    queryKey: ALERT_SOURCES_KEY,
    queryFn: () => listAlertSources(),
  });
  const alertAssignQuery = useQuery({
    queryKey: ALERT_SOURCE_ASSIGNMENTS_KEY,
    queryFn: () => listAlertSourceAssignments(),
  });

  const sources = useMemo(() => sourcesQuery.data?.items ?? [], [sourcesQuery.data]);
  const alertSources = useMemo(() => alertSourcesQuery.data?.items ?? [], [alertSourcesQuery.data]);
  // Set of metric_query ids that already belong to at least one metric set.
  // Used by the "List by default" pill on the metric_cards table so the user
  // sees which queries float on their own thanks to the default-show fallback.
  const queriesInAnySet = useMemo(() => {
    const ids = new Set<string>();
    for (const setDetail of setsQuery.data?.items ?? []) {
      for (const item of setDetail.items ?? []) {
        if (item.metric_query_id) ids.add(item.metric_query_id);
      }
    }
    return ids;
  }, [setsQuery.data]);
  const alertAssignments = useMemo(
    () => alertAssignQuery.data?.items ?? [],
    [alertAssignQuery.data],
  );
  const metricQueries = useMemo(() => queriesQuery.data?.items ?? [], [queriesQuery.data]);
  const sets = useMemo(() => setsQuery.data?.items ?? [], [setsQuery.data]);
  const qsAssignments = useMemo(() => qsAssignQuery.data?.items ?? [], [qsAssignQuery.data]);
  const setAssignments = useMemo(() => setAssignQuery.data?.items ?? [], [setAssignQuery.data]);
  const profileRows = useMemo(() => {
    const rows = new Map<string, InspectorProfileRow>();

    const ensure = (scopeType: string, scopeId: string | null | undefined) => {
      const normalizedScopeId = scopeId ?? null;
      const key = `${scopeType}:${normalizedScopeId ?? ""}`;
      const existing = rows.get(key);
      if (existing) return existing;
      const next = {
        key,
        scopeType,
        scopeId: normalizedScopeId,
        sources: [],
        sets: [],
      };
      rows.set(key, next);
      return next;
    };

    qsAssignments.forEach((assignment) => {
      ensure(assignment.scope_type, assignment.scope_id).sources.push(assignment);
    });
    setAssignments.forEach((assignment) => {
      ensure(assignment.scope_type, assignment.scope_id).sets.push(assignment);
    });

    const scopeRank = (scopeType: string) => {
      const order = ["global", "connection_target", "device", "vm", "site", "tenant", "role"];
      const index = order.indexOf(scopeType);
      return index === -1 ? order.length : index;
    };

    return Array.from(rows.values()).sort((a, b) => {
      const rankDelta = scopeRank(a.scopeType) - scopeRank(b.scopeType);
      if (rankDelta !== 0) return rankDelta;
      return a.key.localeCompare(b.key);
    });
  }, [qsAssignments, setAssignments]);

  // ── Delete mutations ───────────────────────────────────
  const deleteSourceMutation = useApiMutation({
    mutationFn: (id: string) => deleteQuerySource(id),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: QUERY_SOURCES_KEY });
      invalidateInspectorMetricCaches(qc);
    },
    onError: (err: ApiError) => notify("error", err.message),
  });
  const deleteQueryMutation = useApiMutation({
    mutationFn: (id: string) => deleteMetricQuery(id),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: METRIC_QUERIES_KEY });
      invalidateInspectorMetricCaches(qc);
    },
    onError: (err: ApiError) => notify("error", err.message),
  });
  const deleteSetMutation = useApiMutation({
    mutationFn: (id: string) => deleteMetricSet(id),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: METRIC_SETS_KEY });
      invalidateInspectorMetricCaches(qc);
    },
    onError: (err: ApiError) => notify("error", err.message),
  });
  const deleteQsAssignMutation = useApiMutation({
    mutationFn: (id: string) => deleteQuerySourceAssignment(id),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: QS_ASSIGN_KEY });
      invalidateInspectorMetricCaches(qc);
    },
    onError: (err: ApiError) => notify("error", err.message),
  });
  const deleteSetAssignMutation = useApiMutation({
    mutationFn: (id: string) => deleteMetricSetAssignment(id),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: SET_ASSIGN_KEY });
      invalidateInspectorMetricCaches(qc);
    },
    onError: (err: ApiError) => notify("error", err.message),
  });
  const deleteAlertSourceMutation = useApiMutation({
    mutationFn: (id: string) => deleteAlertSource(id),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ALERT_SOURCES_KEY });
    },
    onError: (err: ApiError) => notify("error", err.message),
  });
  const deleteAlertAssignMutation = useApiMutation({
    mutationFn: (id: string) => deleteAlertSourceAssignment(id),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ALERT_SOURCE_ASSIGNMENTS_KEY });
    },
    onError: (err: ApiError) => notify("error", err.message),
  });

  // ── Assignment save mutations ──────────────────────────
  const qsAssignMutation = useApiMutation({
    mutationFn: (draft: AssignmentDraft) => {
      const body = {
        query_source_id: draft.targetId,
        scope_type: draft.scopeType,
        scope_id: draft.scopeType === "global" ? null : draft.scopeId || null,
        weight: Number.parseInt(draft.weight, 10) || 0,
        enabled: draft.enabled,
      };
      return qsAssignDialog.editing
        ? updateQuerySourceAssignment(qsAssignDialog.editing.id, body)
        : createQuerySourceAssignment(body);
    },
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: QS_ASSIGN_KEY });
      invalidateInspectorMetricCaches(qc);
      notify("success", t("observability.assign.saved", { defaultValue: "Assignment saved" }));
    },
    onError: (err: ApiError) => notify("error", err.message),
  });
  const setAssignMutation = useApiMutation({
    mutationFn: (draft: AssignmentDraft) => {
      const body = {
        metric_set_id: draft.targetId,
        scope_type: draft.scopeType,
        scope_id: draft.scopeType === "global" ? null : draft.scopeId || null,
        weight: Number.parseInt(draft.weight, 10) || 0,
        enabled: draft.enabled,
      };
      return setAssignDialog.editing
        ? updateMetricSetAssignment(setAssignDialog.editing.id, body)
        : createMetricSetAssignment(body);
    },
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: SET_ASSIGN_KEY });
      invalidateInspectorMetricCaches(qc);
      notify("success", t("observability.assign.saved", { defaultValue: "Assignment saved" }));
    },
    onError: (err: ApiError) => notify("error", err.message),
  });
  const alertAssignMutation = useApiMutation({
    mutationFn: (draft: AssignmentDraft) => {
      const body = {
        alert_source_id: draft.targetId,
        scope_type: draft.scopeType,
        scope_id: draft.scopeType === "global" ? null : draft.scopeId || null,
        weight: Number.parseInt(draft.weight, 10) || 0,
        enabled: draft.enabled,
      };
      return alertAssignDialog.editing
        ? updateAlertSourceAssignment(alertAssignDialog.editing.id, body)
        : createAlertSourceAssignment(body);
    },
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ALERT_SOURCE_ASSIGNMENTS_KEY });
      notify("success", t("observability.assign.saved", { defaultValue: "Assignment saved" }));
    },
    onError: (err: ApiError) => notify("error", err.message),
  });

  const scopeLabel = (scopeType: string) => {
    const type = scopeType as MonitoringScopeType;
    return t(`observability.scope.${MONITORING_SCOPE_I18N_KEY[type] ?? "global"}`, {
      defaultValue: MONITORING_SCOPE_LABEL[type] ?? scopeType,
    });
  };

  const scopeChip = (scopeType: string, scopeId: string | null | undefined) => {
    const type = scopeType as MonitoringScopeType;
    const isGlobal = type === "global";
    const label = scopeLabel(scopeType);
    return (
      <span className="inline-flex items-center gap-1.5">
        <span className="inline-flex items-center gap-1 rounded-md bg-accent px-1.5 py-0.5 text-xs font-semibold text-primary">
          {isGlobal ? <Globe className="h-3 w-3" /> : <span className="h-1.5 w-1.5 rounded-full bg-primary" />}
          {label}
        </span>
        {!isGlobal && (
          <span className="truncate text-xs text-muted-foreground">
            {entities.resolveName(scopeType, scopeId)}
          </span>
        )}
      </span>
    );
  };

  const rowActions = (label: string, onEdit: (() => void) | undefined, onDelete: () => void, deleting: boolean) => (canUpdate && onEdit) || canDelete ? (
    <RowActions label={`${t("actions")}: ${label}`} ref={(node) => { if (node) rowTriggers.current.set(label, node); else rowTriggers.current.delete(label); }}>
      {canUpdate && onEdit && (
        <DropdownMenuItem
          onSelect={onEdit}
        >
          <Pencil className="h-3.5 w-3.5" />
          {t("edit")}
        </DropdownMenuItem>
      )}
      {canDelete && (
        <DropdownMenuItem variant="destructive"
          disabled={deleting}
          onSelect={() => { returnFocus.current = rowTriggers.current.get(label) ?? null; setDeleteTarget({ label, confirm: onDelete }); }}
        >
          <Trash2 className="h-3.5 w-3.5" />
          {t("delete")}
        </DropdownMenuItem>
      )}
    </RowActions>
  ) : null;

  const profileName = (row: InspectorProfileRow) => {
    if (row.scopeType === "global") {
      return t("observability.workbench.globalProfile", { defaultValue: "Default inspector" });
    }
    const resolvedName = entities.resolveName(row.scopeType, row.scopeId);
    if (resolvedName && resolvedName !== "…") return resolvedName;
    return t("observability.workbench.scopedProfile", {
      scope: scopeLabel(row.scopeType),
      defaultValue: "{{scope}} profile",
    });
  };

  const renderProfileStatus = (row: InspectorProfileRow) => {
    if (row.sources.length === 0 || row.sets.length === 0) {
      return (
        <Badge variant="status-staging" className="rounded-md text-xs">
          {t("observability.workbench.needsSetup", { defaultValue: "Needs setup" })}
        </Badge>
      );
    }
    if ([...row.sources, ...row.sets].some((item) => !item.enabled)) {
      return (
        <Badge variant="status-neutral" className="rounded-md text-xs">
          {t("observability.workbench.partlyOff", { defaultValue: "Partly off" })}
        </Badge>
      );
    }
    return (
      <Badge variant="status-active" className="rounded-md text-xs">
        {t("observability.workbench.ready", { defaultValue: "Ready" })}
      </Badge>
    );
  };

  function renderRuleStack<T extends { id: string; enabled: boolean; weight: number }>({
    items,
    emptyLabel,
    getName,
    onEdit,
    onDelete,
    deleting,
  }: {
    items: T[];
    emptyLabel: string;
    getName: (item: T) => string;
    onEdit: (item: T) => void;
    onDelete: (item: T) => void;
    deleting: boolean;
  }) {
    if (items.length === 0) {
      return (
        <div className="rounded-md border border-dashed border-border bg-background px-2.5 py-2 text-xs text-muted-foreground">
          {emptyLabel}
        </div>
      );
    }

    return (
      <div className="space-y-1.5">
        {items.map((item) => (
          <div
            key={item.id}
            className={cn(
              "flex items-center gap-2 rounded-md border border-border bg-background px-2.5 py-1.5",
              !item.enabled && "opacity-60",
            )}
          >
            <div className="min-w-0 flex-1">
              <p className="break-words text-sm font-medium text-foreground">{getName(item)}</p>
              <div className="mt-1 flex flex-wrap items-center gap-1.5">
                <span className="font-mono text-xs text-muted-foreground tabular-nums">
                  {t("observability.workbench.weight", { defaultValue: "weight" })}: {item.weight}
                </span>
                {!item.enabled && (
                  <Badge variant="outline" className="rounded-md px-1.5 text-xs text-muted-foreground">
                    {t("observability.off", { defaultValue: "Off" })}
                  </Badge>
                )}
              </div>
            </div>
             {rowActions(`${getName(item)} (${item.id})`, () => onEdit(item), () => onDelete(item), deleting)}
          </div>
        ))}
      </div>
    );
  }

  const profileStatusSortValue = (row: InspectorProfileRow) => {
    if (row.sources.length === 0 || row.sets.length === 0) return 2;
    if ([...row.sources, ...row.sets].some((item) => !item.enabled)) return 1;
    return 0;
  };

  const profilesColumns: Column<InspectorProfileRow>[] = [
    {
      key: "profile",
      label: t("observability.workbench.profileColumn", { defaultValue: "Profile" }),
      width: "210px",
      className: "whitespace-normal align-top",
      getSortValue: (row) => profileName(row),
      render: (row) => (
        <div className="min-w-0">
          <p className="truncate text-sm font-semibold text-foreground">{profileName(row)}</p>
          <div className="mt-1 min-w-0">{scopeChip(row.scopeType, row.scopeId)}</div>
        </div>
      ),
    },
    {
      key: "sources",
      label: t("observability.workbench.sourceColumn", { defaultValue: "Sources" }),
      width: "30%",
      className: "whitespace-normal align-top",
      sortable: false,
      render: (row) =>
        renderRuleStack({
          items: row.sources,
          emptyLabel: t("observability.workbench.noSource", { defaultValue: "No source assigned" }),
          getName: (assignment) => assignment.query_source_name,
          onEdit: (assignment) => setQsAssignDialog({ open: true, editing: assignment }),
          onDelete: (assignment) => deleteQsAssignMutation.mutate(assignment.id),
          deleting: deleteQsAssignMutation.isPending,
        }),
    },
    {
      key: "groups",
      label: t("observability.workbench.groupColumn", { defaultValue: "Metric groups" }),
      width: "30%",
      className: "whitespace-normal align-top",
      sortable: false,
      render: (row) =>
        renderRuleStack({
          items: row.sets,
          emptyLabel: t("observability.workbench.noGroup", { defaultValue: "No metric group assigned" }),
          getName: (assignment) => assignment.metric_set_name,
          onEdit: (assignment) => setSetAssignDialog({ open: true, editing: assignment }),
          onDelete: (assignment) => deleteSetAssignMutation.mutate(assignment.id),
          deleting: deleteSetAssignMutation.isPending,
        }),
    },
    {
      key: "status",
      label: t("observability.workbench.statusColumn", { defaultValue: "Status" }),
      width: "120px",
      getSortValue: profileStatusSortValue,
      render: renderProfileStatus,
    },
  ];

  const metricCardColumns: Column<MetricQueryItem>[] = [
    {
      key: "metric",
      label: t("observability.metric.title", { defaultValue: "Metric" }),
      className: "whitespace-normal",
      getSortValue: (row) => row.label || row.name,
      render: (row) => {
        const showDefaultListBadge =
          row.default_show_in_list && !queriesInAnySet.has(row.id);
        return (
          <div className="min-w-[220px] max-w-[520px]">
            <div className="flex flex-wrap items-center gap-1.5">
              <p className="truncate text-sm font-medium text-foreground">
                {row.label || row.name}
              </p>
              {showDefaultListBadge && (
                <Badge
                  variant="outline"
                  className="rounded-md border-border bg-muted px-1.5 text-xs font-normal text-muted-foreground"
                  title={t("observability.metric.defaultInListHint", {
                    defaultValue:
                      "This query is not part of any metric set, so the list-view fallback shows it because default_show_in_list is on.",
                  })}
                >
                  {t("observability.metric.defaultInList", {
                    defaultValue: "List by default",
                  })}
                </Badge>
              )}
            </div>
            <code className="block truncate text-xs text-muted-foreground">{row.query}</code>
          </div>
        );
      },
    },
    {
      key: "source_type",
      label: t("observability.source.type", { defaultValue: "Source" }),
      width: "120px",
      render: (row) => (
        <Badge variant="secondary" className="rounded-md text-xs">
          {row.source_type}
        </Badge>
      ),
    },
    {
      key: "unit",
      label: t("observability.metric.unit", { defaultValue: "Unit" }),
      width: "90px",
      render: (row) => row.unit ? (
        <Badge variant="outline" className="rounded-md text-xs">
          {row.unit}
        </Badge>
      ) : (
        <span className="text-xs text-muted-foreground">—</span>
      ),
    },
    {
      key: "mode",
      label: t("observability.metric.mode", { defaultValue: "Mode" }),
      width: "110px",
      getSortValue: (row) => metricStatusMode(row.options),
      render: (row) => (
        <Badge variant="outline" className="rounded-md text-xs">
          {metricStatusMode(row.options)}
        </Badge>
      ),
    },
    {
      key: "actions",
      label: "",
      width: "78px",
      excludeFromSettings: true,
      sortable: false,
      render: (row) =>
        rowActions(
          `${row.name} (${row.id})`,
          () => { void navigate(observabilityEditHref(METRIC_QUERIES.base, row.id)); },
          () => deleteQueryMutation.mutate(row.id),
          deleteQueryMutation.isPending,
        ),
    },
  ];

  const metricGroupColumns: Column<MetricSetDetail>[] = [
    {
      key: "name",
      label: t("observability.set.title", { defaultValue: "Metric set" }),
      render: (row) => (
        <div className="min-w-0">
          <p className="truncate text-sm font-medium text-foreground">{row.name}</p>
          <p className="truncate text-xs text-muted-foreground">
            {t("observability.set.itemCount", {
              count: row.items.length,
              defaultValue: `${row.items.length} metrics`,
            })}
          </p>
        </div>
      ),
    },
    {
      key: "items",
      label: t("observability.set.metrics", { defaultValue: "Metrics" }),
      width: "110px",
      getSortValue: (row) => row.items.length,
      render: (row) => (
        <span className="text-xs tabular-nums text-foreground">{row.items.length}</span>
      ),
    },
    {
      key: "enabled",
      label: t("observability.sd.enabled", { defaultValue: "Enabled" }),
      width: "110px",
      getSortValue: (row) => (row.enabled ? "1" : "0"),
      render: (row) => (
        <Badge
          variant={row.enabled ? "status-active" : "outline"}
          className="rounded-md text-xs"
        >
          {row.enabled
            ? t("observability.on", { defaultValue: "On" })
            : t("observability.off", { defaultValue: "Off" })}
        </Badge>
      ),
    },
    {
      key: "actions",
      label: "",
      width: "78px",
      excludeFromSettings: true,
      sortable: false,
      render: (row) =>
        rowActions(
          `${row.name} (${row.id})`,
          () => { void navigate(observabilityEditHref(METRIC_SETS.base, row.id)); },
          () => deleteSetMutation.mutate(row.id),
          deleteSetMutation.isPending,
        ),
    },
  ];

  const sourceColumns: Column<QuerySourceItem>[] = [
    {
      key: "name",
      label: t("observability.source.title", { defaultValue: "Data source" }),
      className: "whitespace-normal",
      render: (row) => (
        <div className="min-w-[220px] max-w-[520px]">
          <p className="truncate text-sm font-medium text-foreground">{row.name}</p>
          <p className="truncate text-xs text-muted-foreground">{row.query_url}</p>
        </div>
      ),
    },
    {
      key: "source_type",
      label: t("observability.source.type", { defaultValue: "Type" }),
      width: "130px",
      render: (row) => (
        <Badge variant="secondary" className="rounded-md text-xs">
          {row.source_type}
        </Badge>
      ),
    },
    {
      key: "enabled",
      label: t("observability.sd.enabled", { defaultValue: "Enabled" }),
      width: "110px",
      getSortValue: (row) => (row.enabled ? "1" : "0"),
      render: (row) => (
        <Badge
          variant={row.enabled ? "status-active" : "outline"}
          className="rounded-md text-xs"
        >
          {row.enabled
            ? t("observability.on", { defaultValue: "On" })
            : t("observability.off", { defaultValue: "Off" })}
        </Badge>
      ),
    },
    {
      key: "actions",
      label: "",
      width: "78px",
      excludeFromSettings: true,
      sortable: false,
      render: (row) =>
        rowActions(
          `${row.name} (${row.id})`,
          () => { void navigate(observabilityEditHref(QUERY_SOURCES.base, row.id)); },
          () => deleteSourceMutation.mutate(row.id),
          deleteSourceMutation.isPending,
        ),
    },
  ];

  const alertSourceColumns: Column<AlertSourceItem>[] = [
    {
      key: "name",
      label: t("observability.alertSource.title", { defaultValue: "Alert source" }),
      className: "whitespace-normal",
      render: (row) => (
        <div className="min-w-[220px] max-w-[520px]">
          <p className="truncate text-sm font-medium text-foreground">{row.name}</p>
          <p className="truncate text-xs text-muted-foreground">{row.query_url}</p>
        </div>
      ),
    },
    {
      key: "source_type",
      label: t("observability.source.type", { defaultValue: "Type" }),
      width: "130px",
      render: (row) => (
        <Badge variant="secondary" className="rounded-md text-xs">
          {row.source_type}
        </Badge>
      ),
    },
    {
      key: "enabled",
      label: t("observability.sd.enabled", { defaultValue: "Enabled" }),
      width: "110px",
      getSortValue: (row) => (row.enabled ? "1" : "0"),
      render: (row) => (
        <Badge
          variant={row.enabled ? "status-active" : "outline"}
          className="rounded-md text-xs"
        >
          {row.enabled
            ? t("observability.on", { defaultValue: "On" })
            : t("observability.off", { defaultValue: "Off" })}
        </Badge>
      ),
    },
    {
      key: "actions",
      label: "",
      width: "78px",
      excludeFromSettings: true,
      sortable: false,
      render: (row) =>
        rowActions(
          `${row.name} (${row.id})`,
          () => { void navigate(observabilityEditHref(ALERT_SOURCES.base, row.id)); },
          () => deleteAlertSourceMutation.mutate(row.id),
          deleteAlertSourceMutation.isPending,
        ),
    },
  ];

  const alertAssignmentColumns: Column<AlertSourceAssignmentItem>[] = [
    {
      key: "alert_source",
      label: t("observability.alertSource.title", { defaultValue: "Alert source" }),
      className: "whitespace-normal",
      getSortValue: (row) => row.alert_source_name,
      render: (row) => (
        <p className="truncate text-sm font-medium text-foreground">{row.alert_source_name}</p>
      ),
    },
    {
      key: "scope",
      label: t("observability.scope.typeLabel", { defaultValue: "Scope" }),
      className: "whitespace-normal",
      sortable: false,
      render: (row) => scopeChip(row.scope_type, row.scope_id),
    },
    {
      key: "weight",
      label: t("observability.workbench.weight", { defaultValue: "weight" }),
      width: "90px",
      getSortValue: (row) => row.weight,
      render: (row) => (
        <span className="font-mono text-xs tabular-nums text-muted-foreground">{row.weight}</span>
      ),
    },
    {
      key: "enabled",
      label: t("observability.sd.enabled", { defaultValue: "Enabled" }),
      width: "110px",
      getSortValue: (row) => (row.enabled ? "1" : "0"),
      render: (row) => (
        <Badge
          variant={row.enabled ? "status-active" : "outline"}
          className="rounded-md text-xs"
        >
          {row.enabled
            ? t("observability.on", { defaultValue: "On" })
            : t("observability.off", { defaultValue: "Off" })}
        </Badge>
      ),
    },
    {
      key: "actions",
      label: "",
      width: "78px",
      excludeFromSettings: true,
      sortable: false,
      render: (row) =>
        rowActions(
          `${row.alert_source_name} (${row.id})`,
          () => setAlertAssignDialog({ open: true, editing: row }),
          () => deleteAlertAssignMutation.mutate(row.id),
          deleteAlertAssignMutation.isPending,
        ),
    },
  ];

  const renderProfilesView = () => (
    <DataTable<InspectorProfileRow>
      tableId="observability.inspector.profiles"
      columns={profilesColumns}
      data={profileRows}
      getRowKey={(row) => row.key}
      loading={qsAssignQuery.isPending || setAssignQuery.isPending}
      error={qsAssignQuery.error ?? setAssignQuery.error ?? undefined}
      onRetry={() => {
        void qsAssignQuery.refetch();
        void setAssignQuery.refetch();
      }}
      className="min-h-[460px]"
      toolbar={
        <InspectorTableSummary
          icon={Cable}
          title={t("observability.workbench.profilesTitle", { defaultValue: "Inspector profiles" })}
          description={t("observability.workbench.profilesHint", {
            defaultValue:
              "Choose which data source and metric groups are used for each scope. Values are read live from external monitoring sources.",
          })}
          action={
            canCreate ? (
              <>
                <Button variant="outline" className="gap-1.5" onClick={() => setQsAssignDialog({ open: true, editing: null })}>
                  <Plus className="h-3.5 w-3.5" />
                  {t("observability.workbench.assignSource", { defaultValue: "Assign source" })}
                </Button>
                <Button className="gap-1.5" onClick={() => setSetAssignDialog({ open: true, editing: null })}>
                  <Plus className="h-3.5 w-3.5" />
                  {t("observability.workbench.assignGroup", { defaultValue: "Assign group" })}
                </Button>
              </>
            ) : undefined
          }
        />
      }
      emptyState={
        <EmptyState
          icon={Cable}
          title={t("observability.workbench.noProfiles", { defaultValue: "No inspector profiles" })}
          description={t("observability.workbench.noProfilesHint", {
            defaultValue: "Bind a data source and a metric group to a scope to define what the inspector shows.",
          })}
          className="min-h-[260px] py-8"
          actions={canCreate && (
            <>
              <Button variant="outline" className="gap-1.5" onClick={() => setQsAssignDialog({ open: true, editing: null })}>
                <Plus className="h-3.5 w-3.5" />
                {t("observability.workbench.assignSource", { defaultValue: "Assign source" })}
              </Button>
              <Button className="gap-1.5" onClick={() => setSetAssignDialog({ open: true, editing: null })}>
                <Plus className="h-3.5 w-3.5" />
                {t("observability.workbench.assignGroup", { defaultValue: "Assign group" })}
              </Button>
            </>
          )}
        />
      }
    />
  );

  const renderMetricCardsView = () => (
    <DataTable<MetricQueryItem>
      tableId="observability.inspector.metric_cards"
      columns={metricCardColumns}
      data={metricQueries}
      getRowKey={(row) => row.id}
      onRowClick={(row) => { void navigate(observabilityRecordHref(METRIC_QUERIES.base, row.id)); }}
      loading={queriesQuery.isPending}
      error={queriesQuery.error ?? undefined}
      onRetry={() => queriesQuery.refetch()}
      className="min-h-[460px]"
      toolbar={
        <InspectorTableSummary
          icon={ListChecks}
          title={t("observability.workbench.cardsTitle", { defaultValue: "Metric cards" })}
          description={t("observability.workbench.cardsHint", {
            defaultValue: "Reusable metric queries that can be shown in inspector profiles through metric groups.",
          })}
          action={
            canCreate ? (
              <Button className="gap-1.5" onClick={() => { void navigate(observabilityNewHref(METRIC_QUERIES.base)); }}>
                <Plus className="h-3.5 w-3.5" />
                {t("observability.metric.add", { defaultValue: "New metric" })}
              </Button>
            ) : undefined
          }
        />
      }
      emptyState={
        <EmptyState
          icon={ListChecks}
          title={t("observability.metric.empty", { defaultValue: "No metric queries" })}
          description={t("observability.metric.emptyHint", {
            defaultValue: "Define queries, then group them into sets.",
          })}
          className="min-h-[260px] py-8"
          actions={canCreate && (
            <Button className="gap-1.5" onClick={() => { void navigate(observabilityNewHref(METRIC_QUERIES.base)); }}>
              <Plus className="h-3.5 w-3.5" />
              {t("observability.metric.add", { defaultValue: "New metric" })}
            </Button>
          )}
        />
      }
    />
  );

  const renderMetricGroupsView = () => (
    <DataTable<MetricSetDetail>
      tableId="observability.inspector.metric_groups"
      columns={metricGroupColumns}
      data={sets}
      getRowKey={(row) => row.id}
      onRowClick={(row) => { void navigate(observabilityRecordHref(METRIC_SETS.base, row.id)); }}
      loading={setsQuery.isPending}
      error={setsQuery.error ?? undefined}
      onRetry={() => setsQuery.refetch()}
      className="min-h-[460px]"
      toolbar={
        <InspectorTableSummary
          icon={Layers}
          title={t("observability.workbench.groupsTitle", { defaultValue: "Metric groups" })}
          description={t("observability.workbench.groupsHint", {
            defaultValue:
              "Named sets of cards. Profiles attach these groups to scopes, so the inspector knows what to show.",
          })}
          action={
            canCreate ? (
              <Button className="gap-1.5" onClick={() => { void navigate(observabilityNewHref(METRIC_SETS.base)); }}>
                <Plus className="h-3.5 w-3.5" />
                {t("observability.set.add", { defaultValue: "New set" })}
              </Button>
            ) : undefined
          }
        />
      }
      emptyState={
        <EmptyState
          icon={Layers}
          title={t("observability.set.empty", { defaultValue: "No metric sets" })}
          description={t("observability.set.emptyHint", {
            defaultValue: "Group metric queries into a reusable set.",
          })}
          className="min-h-[260px] py-8"
          actions={canCreate && (
            <Button className="gap-1.5" onClick={() => { void navigate(observabilityNewHref(METRIC_SETS.base)); }}>
              <Plus className="h-3.5 w-3.5" />
              {t("observability.set.add", { defaultValue: "New set" })}
            </Button>
          )}
        />
      }
    />
  );

  const renderSourcesView = () => (
    <DataTable<QuerySourceItem>
      tableId="observability.inspector.sources"
      columns={sourceColumns}
      data={sources}
      getRowKey={(row) => row.id}
      onRowClick={(row) => { void navigate(observabilityRecordHref(QUERY_SOURCES.base, row.id)); }}
      loading={sourcesQuery.isPending}
      error={sourcesQuery.error ?? undefined}
      onRetry={() => sourcesQuery.refetch()}
      className="min-h-[460px]"
      toolbar={
        <InspectorTableSummary
          icon={Database}
          title={t("observability.workbench.sourcesTitle", { defaultValue: "Data sources" })}
          description={t("observability.workbench.sourcesHint", {
            defaultValue: "External monitoring systems Piping queries for current inspector values.",
          })}
          action={
            canCreate ? (
              <>
                <Button

                  variant="outline"
                  className="gap-1.5"
                  onClick={() => setPresetDialogOpen(true)}
                >
                  <Sparkles className="h-3.5 w-3.5" />
                  {t("observability.preset.applyAction", { defaultValue: "Apply preset…" })}
                </Button>
                <Button className="gap-1.5" onClick={() => { void navigate(observabilityNewHref(QUERY_SOURCES.base)); }}>
                  <Plus className="h-3.5 w-3.5" />
                  {t("observability.source.add", { defaultValue: "New source" })}
                </Button>
              </>
            ) : undefined
          }
        />
      }
      emptyState={
        <EmptyState
          icon={Database}
          title={t("observability.source.empty", { defaultValue: "No data sources" })}
          description={t("observability.source.emptyHint", {
            defaultValue: "Add one so inspectors can read metric values.",
          })}
          className="min-h-[260px] py-8"
          actions={canCreate && (
            <Button className="gap-1.5" onClick={() => { void navigate(observabilityNewHref(QUERY_SOURCES.base)); }}>
              <Plus className="h-3.5 w-3.5" />
              {t("observability.source.add", { defaultValue: "New source" })}
            </Button>
          )}
        />
      }
    />
  );

  const renderAlertsView = () => (
    <div className="flex min-h-0 flex-col gap-4">
      <DataTable<AlertSourceItem>
        tableId="observability.inspector.alert_sources"
        columns={alertSourceColumns}
        data={alertSources}
        getRowKey={(row) => row.id}
        onRowClick={(row) => { void navigate(observabilityRecordHref(ALERT_SOURCES.base, row.id)); }}
        loading={alertSourcesQuery.isPending}
        error={alertSourcesQuery.error ?? undefined}
        onRetry={() => alertSourcesQuery.refetch()}
        className="min-h-[220px]"
        toolbar={
          <InspectorTableSummary
            icon={BellRing}
            title={t("observability.workbench.alertSourcesTitle", { defaultValue: "Alert sources" })}
            description={t("observability.workbench.alertSourcesHint", {
              defaultValue:
                "External alerting backends Piping pulls active alerts from (Alertmanager, vmalert, Zabbix).",
            })}
            action={
              canCreate ? (
                <Button className="gap-1.5" onClick={() => { void navigate(observabilityNewHref(ALERT_SOURCES.base)); }}>
                  <Plus className="h-3.5 w-3.5" />
                  {t("observability.alertSource.add", { defaultValue: "New alert source" })}
                </Button>
              ) : undefined
            }
          />
        }
        emptyState={
          <EmptyState
            icon={BellRing}
            title={t("observability.alertSource.empty", { defaultValue: "No alert sources" })}
            description={t("observability.alertSource.emptyHint", {
              defaultValue: "Add one so hosts can surface active alerts in lists.",
            })}
            className="min-h-[260px] py-8"
            actions={canCreate && (
              <Button className="gap-1.5" onClick={() => { void navigate(observabilityNewHref(ALERT_SOURCES.base)); }}>
                <Plus className="h-3.5 w-3.5" />
                {t("observability.alertSource.add", { defaultValue: "New alert source" })}
              </Button>
            )}
          />
        }
      />
      <DataTable<AlertSourceAssignmentItem>
        tableId="observability.inspector.alert_assignments"
        columns={alertAssignmentColumns}
        data={alertAssignments}
        getRowKey={(row) => row.id}
        loading={alertAssignQuery.isPending}
        error={alertAssignQuery.error ?? undefined}
        onRetry={() => alertAssignQuery.refetch()}
        className="min-h-[220px]"
        toolbar={
          <InspectorTableSummary
            icon={Cable}
            title={t("observability.assign.alertTitle", { defaultValue: "Alert source assignments" })}
            description={t("observability.assign.alertHint", {
              defaultValue: "Bind an alert source to a scope or connection target.",
            })}
            action={
              canCreate ? (
                <Button variant="outline" className="gap-1.5" onClick={() => setAlertAssignDialog({ open: true, editing: null })}>
                  <Plus className="h-3.5 w-3.5" />
                  {t("observability.assign.assignAlert", { defaultValue: "Assign alert source" })}
                </Button>
              ) : undefined
            }
          />
        }
        emptyState={
          <EmptyState
            icon={Cable}
            title={t("observability.assign.alertEmpty", { defaultValue: "No alert assignments" })}
            description={t("observability.assign.alertEmptyHint", {
              defaultValue: "Bind an alert source to a scope to start correlating alerts to hosts.",
            })}
            className="min-h-[260px] py-8"
            actions={canCreate && (
              <Button variant="outline" className="gap-1.5" onClick={() => setAlertAssignDialog({ open: true, editing: null })}>
                <Plus className="h-3.5 w-3.5" />
                {t("observability.assign.assignAlert", { defaultValue: "Assign alert source" })}
              </Button>
            )}
          />
        }
      />
    </div>
  );

  const workbenchTabs = [
    {
      value: "profiles",
      label: t("observability.workbench.profilesTab", { defaultValue: "Inspector profiles" }),
      icon: Cable,
    },
    {
      value: "cards",
      label: t("observability.workbench.cardsTab", { defaultValue: "Metric cards" }),
      icon: ListChecks,
    },
    {
      value: "groups",
      label: t("observability.workbench.groupsTab", { defaultValue: "Metric groups" }),
      icon: Layers,
    },
    {
      value: "sources",
      label: t("observability.workbench.sourcesTab", { defaultValue: "Data sources" }),
      icon: Database,
    },
    {
      value: "alerts",
      label: t("observability.workbench.alertsTab", { defaultValue: "Alert sources" }),
      icon: BellRing,
    },
  ] satisfies { value: InspectorWorkbenchView; label: string; icon: LucideIcon }[];

  return (
    <div className="grid min-w-0 gap-4 xl:min-h-0 xl:flex-1 xl:grid-cols-[minmax(0,1fr)_420px]">
      <Tabs
        value={workbenchView}
        onValueChange={(value) => setWorkbenchView(value as InspectorWorkbenchView)}
        className="flex min-h-0 min-w-0 flex-col gap-3"
      >
        <SectionTabs items={workbenchTabs} />
        <TabsContent value="profiles" className="mt-0 min-h-0 flex-1">
          {renderProfilesView()}
        </TabsContent>
        <TabsContent value="cards" className="mt-0 min-h-0 flex-1">
          {renderMetricCardsView()}
        </TabsContent>
        <TabsContent value="groups" className="mt-0 min-h-0 flex-1">
          {renderMetricGroupsView()}
        </TabsContent>
        <TabsContent value="sources" className="mt-0 min-h-0 flex-1">
          {renderSourcesView()}
        </TabsContent>
        <TabsContent value="alerts" className="mt-0 min-h-0 flex-1">
          {renderAlertsView()}
        </TabsContent>
      </Tabs>
      <ConfirmDeleteDialog
        open={!!deleteTarget}
        onOpenChange={(open) => { if (!open) setDeleteTarget(null); }}
        onCloseAutoFocus={(event) => { event.preventDefault(); returnFocus.current?.focus(); }}
        title={t("delete")}
        description={deleteTarget?.label}
        cancelLabel={t("cancel")}
        confirmLabel={t("delete")}
        confirmProps={{ disabled: !canDelete, onClick: () => { if (canDelete) deleteTarget?.confirm(); } }}
      />

      <InspectorPreviewPanel />

      {/* ── Dialogs ── */}
      <SimpleAssignmentDialog
        open={qsAssignDialog.open}
        onOpenChange={(open) => setQsAssignDialog((s) => ({ ...s, open }))}
        isEditing={!!qsAssignDialog.editing}
        title={
          qsAssignDialog.editing
            ? t("observability.assign.editSource", { defaultValue: "Edit source assignment" })
            : t("observability.assign.newSource", { defaultValue: "New source assignment" })
        }
        description={t("observability.assign.sourceHint", {
          defaultValue: "Bind a data source to a scope or connection target.",
        })}
        targetLabel={t("observability.source.title", { defaultValue: "Data source" })}
        targetOptions={sources.map((s) => ({ id: s.id, name: s.name }))}
        entities={entities}
        pending={qsAssignMutation.isPending}
        onSubmit={(draft) => { if (!qsAssignMutation.isPending) qsAssignMutation.mutate(draft, { onSuccess: () => setQsAssignDialog({ open: false, editing: null }) }); }}
        initial={
          qsAssignDialog.editing
            ? {
                targetId: qsAssignDialog.editing.query_source_id,
                scopeType: qsAssignDialog.editing.scope_type as MonitoringScopeType,
                scopeId: qsAssignDialog.editing.scope_id ?? "",
                weight: String(qsAssignDialog.editing.weight),
                enabled: qsAssignDialog.editing.enabled,
              }
            : EMPTY_ASSIGNMENT
        }
      />
      <SimpleAssignmentDialog
        open={setAssignDialog.open}
        onOpenChange={(open) => setSetAssignDialog((s) => ({ ...s, open }))}
        isEditing={!!setAssignDialog.editing}
        title={
          setAssignDialog.editing
            ? t("observability.assign.editSet", { defaultValue: "Edit set assignment" })
            : t("observability.assign.newSet", { defaultValue: "New set assignment" })
        }
        description={t("observability.assign.setHint", {
          defaultValue: "Bind a metric set to a scope or connection target.",
        })}
        targetLabel={t("observability.set.title", { defaultValue: "Metric set" })}
        targetOptions={sets.map((s) => ({ id: s.id, name: s.name }))}
        entities={entities}
        pending={setAssignMutation.isPending}
        onSubmit={(draft) => { if (!setAssignMutation.isPending) setAssignMutation.mutate(draft, { onSuccess: () => setSetAssignDialog({ open: false, editing: null }) }); }}
        initial={
          setAssignDialog.editing
            ? {
                targetId: setAssignDialog.editing.metric_set_id,
                scopeType: setAssignDialog.editing.scope_type as MonitoringScopeType,
                scopeId: setAssignDialog.editing.scope_id ?? "",
                weight: String(setAssignDialog.editing.weight),
                enabled: setAssignDialog.editing.enabled,
              }
            : EMPTY_ASSIGNMENT
        }
      />
      <ExporterPresetApplyDialog
        open={presetDialogOpen}
        onOpenChange={setPresetDialogOpen}
        sources={sources}
        entities={entities}
      />
      <SimpleAssignmentDialog
        open={alertAssignDialog.open}
        onOpenChange={(open) => setAlertAssignDialog((s) => ({ ...s, open }))}
        isEditing={!!alertAssignDialog.editing}
        title={
          alertAssignDialog.editing
            ? t("observability.assign.editAlert", { defaultValue: "Edit alert assignment" })
            : t("observability.assign.newAlert", { defaultValue: "New alert assignment" })
        }
        description={t("observability.assign.alertHint", {
          defaultValue: "Bind an alert source to a scope or connection target.",
        })}
        targetLabel={t("observability.alertSource.title", { defaultValue: "Alert source" })}
        targetOptions={alertSources.map((s) => ({ id: s.id, name: s.name }))}
        entities={entities}
        pending={alertAssignMutation.isPending}
        onSubmit={(draft) => { if (!alertAssignMutation.isPending) alertAssignMutation.mutate(draft, { onSuccess: () => setAlertAssignDialog({ open: false, editing: null }) }); }}
        initial={
          alertAssignDialog.editing
            ? {
                targetId: alertAssignDialog.editing.alert_source_id,
                scopeType: alertAssignDialog.editing.scope_type as MonitoringScopeType,
                scopeId: alertAssignDialog.editing.scope_id ?? "",
                weight: String(alertAssignDialog.editing.weight),
                enabled: alertAssignDialog.editing.enabled,
              }
            : EMPTY_ASSIGNMENT
        }
      />
    </div>
  );
};

export default InspectorMetricsScreen;
