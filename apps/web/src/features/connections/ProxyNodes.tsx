import { useMemo, useState, useRef } from "react";
import { Link, useNavigate } from "react-router";
import { useTranslation, Trans } from "react-i18next";
import { useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { useApiMutation } from "@/shared/api/hooks";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { useServerTable } from "@/shared/ui-shared/useServerTable";
import PageChrome from "@/shared/ui-shared/PageChrome";
import RowActions from "@/shared/ui-shared/RowActions";
import { DropdownMenuItem } from "@/shared/ui-primitives/dropdown-menu";
import {
  deleteProxyNodeAssignment,
  deleteProxyNode,
  listProxyNodeAssignments,
  listProxyNodes,
  rotateProxyNodeSecret,
  updateProxyNode,
  type CreatedProxyNodeResponse,
} from "@/features/connections/proxies-api";
import { listResources } from "@/features/inventory/api";
import {
  CONNCONF_SCOPE_TYPES,
  type ConnConfScopeType,
} from "@/features/connections/connection-config/api";
import { SCOPE_ENTITY, SCOPE_I18N_KEY } from "@/features/connections/connection-config/scopeCatalog";
import type { ProxyNode as BackendProxyNode, ProxyNodeAssignment as BackendProxyNodeAssignment } from "@/shared/api/types";
import type { components } from "@/shared/api/schema.gen";
import type { ApiError } from "@/shared/api/client";
import DataTable, {
  type DataTableFilterOption,
} from "@/shared/ui-shared/DataTable";
import { boolFilterOptions } from "@/shared/ui-shared/dataTableLocal";
import {
  PROXY_ASSIGNMENTS_TABLE_CONFIG,
  PROXY_NODES_TABLE_CONFIG,
} from "@/features/connections/proxyTableConfig";
import StatusChip from "@/shared/ui-shared/StatusChip";
import EmptyState from "@/shared/ui-shared/EmptyState";
import EntityRef from "@/shared/ui-shared/EntityRef";
import { Button } from "@/shared/ui-primitives/button";
import SearchInput from "@/shared/ui-shared/SearchInput";
import { Switch } from "@/shared/ui-primitives/switch";
import { AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from "@/shared/ui-primitives/alert-dialog";
import { Plus, Pencil, Trash2, Radio, Route, Globe, KeyRound } from "lucide-react";
import { notify } from "@/shared/lib/notifications";
import ProxyNodeBootstrapDialog from "@/features/connections/ProxyNodeBootstrapDialog";
import {
  PROXY_ASSIGNMENT_NEW_ROUTE,
  PROXY_NODE_NEW_ROUTE,
  proxyAssignmentEditHref,
  proxyNodeHref,
  proxyNodeEditHref,
} from "@/features/connections/proxyRecordRoutes";

type ProxyNode = BackendProxyNode;
type ProxyNodeAssignment = BackendProxyNodeAssignment;
type UpdateProxyNodeRequest = components["schemas"]["UpdateProxyNodeRequest"];

const assignmentDefaultPageSize = 10;

const ProxyNodes = () => {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const [deleteTarget, setDeleteTarget] = useState<ProxyNode | null>(null);
  const [bootstrapDialog, setBootstrapDialog] =
    useState<CreatedProxyNodeResponse | null>(null);
  const [assignmentDeleteTarget, setAssignmentDeleteTarget] =
    useState<ProxyNodeAssignment | null>(null);
  const actionTriggers = useRef(new Map<string, HTMLButtonElement>());
  const deleteInvoker = useRef<HTMLButtonElement | null>(null);
  const bootstrapInvoker = useRef<HTMLButtonElement | null>(null);

  // View-but-not-edit: hide proxy-node mutation affordances the caller can't use.
  // Assignment create/delete also gate on PROXY_NODE CREATE/DELETE.
  const { can } = usePermissions();
  const canCreate = can("CREATE", "PROXY_NODE");
  const canUpdate = can("UPDATE", "PROXY_NODE");
  const canDelete = can("DELETE", "PROXY_NODE");

  const nodeTable = useServerTable<ProxyNode, NonNullable<Parameters<typeof listProxyNodes>[0]>>({
    ...PROXY_NODES_TABLE_CONFIG,
    queryKey: ["proxy-nodes"],
    listFn: listProxyNodes,
    keepPreviousData: true,
  });
  const { search, setSearch, page, setPage, pageSize, setPageSize,
    sortState: sanitizedSortState, columnFilters: sanitizedNodeFilters,
    query: proxyQuery } = nodeTable;
  const proxyOptionsQuery = useQuery({
    queryKey: ["proxy-nodes", "options"],
    queryFn: () => listProxyNodes({ limit: 1000, offset: 0 }),
    staleTime: 60_000,
  });
  const assignmentTable = useServerTable<ProxyNodeAssignment, NonNullable<Parameters<typeof listProxyNodeAssignments>[0]>>({
    ...PROXY_ASSIGNMENTS_TABLE_CONFIG,
    queryKey: ["proxy-node-assignments"],
    listFn: ({ limit, offset, sort, order, proxy_node_id, scope_type, enabled }) =>
      listProxyNodeAssignments({ limit, offset, sort, order, proxy_node_id, scope_type, enabled }),
    pageSizeDefault: assignmentDefaultPageSize,
    keepPreviousData: true,
  });
  const { page: assignmentPage, setPage: setAssignmentPage,
    pageSize: assignmentPageSize, setPageSize: setAssignmentPageSize,
    sortState: sanitizedAssignmentSortState, columnFilters: sanitizedAssignmentFilters,
    query: assignmentsQuery } = assignmentTable;
  const nodes = proxyQuery.data?.items ?? [];
  const proxyOptions = proxyOptionsQuery.data?.items ?? nodes;
  const total = proxyQuery.data?.total ?? 0;
  const assignments = assignmentsQuery.data?.items ?? [];
  const assignmentTotal = assignmentsQuery.data?.total ?? 0;

  const targetableScopes = useMemo(
    () => CONNCONF_SCOPE_TYPES.filter((s) => s !== "global"),
    [],
  );
  const scopeQueries = useQueries({
    queries: targetableScopes.map((scope) => ({
      queryKey: ["inventory", "proxy-assignment-scope", SCOPE_ENTITY[scope]],
      queryFn: () => listResources(SCOPE_ENTITY[scope], { limit: 1000, offset: 0 }),
      staleTime: 60_000,
      enabled: assignments.length > 0,
    })),
  });
  const optionsByScope = useMemo(() => {
    const map = {} as Record<Exclude<ConnConfScopeType, "global">, { id: string; name: string }[]>;
    targetableScopes.forEach((scope, index) => {
      map[scope] = (scopeQueries[index]?.data?.items ?? []).map((r) => ({
        id: r.id,
        name: r.name,
      }));
    });
    return map;
  }, [targetableScopes, scopeQueries]);

  // Assignment column-filter options: proxy (FK -> proxy node list) + scope
  // (static scope-type enum). Values forward as comma-joined `IN` filters.
  const proxyFilterOptions = useMemo<DataTableFilterOption[]>(
    () => proxyOptions.map((node) => ({ value: node.id, label: node.name })),
    [proxyOptions],
  );
  const scopeFilterOptions = useMemo<DataTableFilterOption[]>(
    () =>
      CONNCONF_SCOPE_TYPES.map((scope) => ({
        value: scope,
        label: t(`connectionConfig.scope.${SCOPE_I18N_KEY[scope]}`),
      })),
    [t],
  );
  const enabledFilterOptions = useMemo<DataTableFilterOption[]>(
    () =>
      boolFilterOptions({
        trueLabel: t("enabled"),
        falseLabel: t("disabled"),
      }),
    [t],
  );


  const invalidate = () => qc.invalidateQueries({ queryKey: ["proxy-nodes"] });
  const invalidateAssignments = () =>
    qc.invalidateQueries({ queryKey: ["proxy-node-assignments"] });

  const updateMutation = useApiMutation({
    mutationFn: ({ id, body }: { id: string; body: UpdateProxyNodeRequest }) =>
      updateProxyNode(id, body),
    onSuccess: (updated) => {
      void invalidate();
      notify("success", t("proxyNodes.nodeUpdated", { name: updated.name }));
    },
    onError: (err: ApiError) => notify("error", err.message || t("unknownError")),
  });

  const deleteMutation = useApiMutation({
    mutationFn: (id: string) => deleteProxyNode(id),
    onSuccess: () => {
      void invalidate();
      notify("success", t("proxyNodes.nodeDeleted"));
      setDeleteTarget(null);
    },
    onError: (err: ApiError) => notify("error", err.message || t("unknownError")),
  });

  const rotateSecretMutation = useApiMutation({
    mutationFn: (id: string) => rotateProxyNodeSecret(id),
    onSuccess: (created) => {
      void invalidate();
      notify("success", t("proxyNodes.secretRotated", { name: created.node.name }));
      setBootstrapDialog(created);
    },
    onError: (err: ApiError) => notify("error", err.message || t("unknownError")),
  });

  const deleteAssignmentMutation = useApiMutation({
    mutationFn: (id: string) => deleteProxyNodeAssignment(id),
    onSuccess: () => {
      void invalidateAssignments();
      notify("success", t("proxyNodes.assignmentDeleted"));
      setAssignmentDeleteTarget(null);
    },
    onError: (err: ApiError) => notify("error", err.message || t("unknownError")),
  });

  const handleDelete = () => {
    if (!deleteTarget) return;
    deleteMutation.mutate(deleteTarget.id);
  };

  const handleAssignmentDelete = () => {
    if (!assignmentDeleteTarget) return;
    deleteAssignmentMutation.mutate(assignmentDeleteTarget.id);
  };

  const toggleEnabled = (n: ProxyNode) => {
    // UpdateProxyNodeRequest requires name/slug/base_url/enabled — send the whole
    // node, not just the toggled flag, or the PUT 422s on the missing fields.
    updateMutation.mutate({
      id: n.id,
      body: { name: n.name, slug: n.slug, base_url: n.base_url, enabled: !n.enabled },
    });
  };

  const assignmentScopeTarget = (
    type: string,
    id: string | null | undefined,
  ): { id: string; name?: string } | null => {
    if (type === "global" || !id) return null;
    const scope = type as Exclude<ConnConfScopeType, "global">;
    return {
      id,
      name: optionsByScope[scope]?.find((option) => option.id === id)?.name,
    };
  };

  const formatSecretRotatedAt = (value?: string | null) => {
    if (!value) return t("proxyNodes.neverRotated");
    const date = new Date(value);
    if (Number.isNaN(date.getTime())) return value;
    return date.toLocaleString();
  };

  const columns = [
    { key: "name", label: t("name"), sortable: true, render: (r: ProxyNode) => (
      <div className="flex items-center gap-2">
        <div className={`w-2 h-2 rounded-full ${r.enabled ? "bg-status-positive animate-pulse" : "bg-muted-foreground"}`} />
        <Link to={proxyNodeHref(r.id)} className="truncate font-medium text-primary hover:underline">{r.name}</Link>
      </div>
    )},
    { key: "slug", label: t("proxyNodes.slug"), sortable: true, render: (r: ProxyNode) => <span className="font-mono text-[11px] text-muted-foreground">{r.slug}</span> },
    { key: "baseUrl", label: t("proxyNodes.baseUrl"), sortable: true, render: (r: ProxyNode) => <span className="font-mono text-[11px] text-muted-foreground">{r.base_url}</span> },
    { key: "secret", label: t("proxyNodes.secret"), render: (r: ProxyNode) => (
      <div className="flex min-w-0 flex-col">
        <span className="text-xs font-medium">{t("proxyNodes.secretVersion", { version: r.secret_version ?? 0 })}</span>
        <span className="truncate text-xs text-muted-foreground">{formatSecretRotatedAt(r.secret_rotated_at)}</span>
      </div>
    )},
    { key: "enabled", label: t("status"), sortable: true, filterOptions: enabledFilterOptions, render: (r: ProxyNode) => (
      <div className="flex items-center gap-2">
        {canUpdate && <Switch aria-label={`${t("enabled")}: ${r.name}`} checked={r.enabled} onCheckedChange={() => toggleEnabled(r)} disabled={updateMutation.isPending} />}
        <StatusChip status={r.enabled ? "positive" : "neutral"} label={r.enabled ? t("enabled") : t("disabled")} />
      </div>
    )},
    {
      key: "actions", label: "", excludeFromSettings: true, render: (r: ProxyNode) => canUpdate || canDelete ? (
        <RowActions label={`${t("actions")}: ${r.name}`} ref={node => {
          if (node) actionTriggers.current.set(`node:${r.id}`, node);
          else actionTriggers.current.delete(`node:${r.id}`);
        }}>
          {canUpdate && <DropdownMenuItem onSelect={() => {
            bootstrapInvoker.current = actionTriggers.current.get(`node:${r.id}`) ?? null;
            rotateSecretMutation.mutate(r.id);
          }} disabled={rotateSecretMutation.isPending}><KeyRound aria-hidden="true" />{t("proxyNodes.rotateSecret")}</DropdownMenuItem>}
          {canUpdate && <DropdownMenuItem asChild><Link to={proxyNodeEditHref(r.id)}><Pencil aria-hidden="true" />{t("edit")}</Link></DropdownMenuItem>}
          {canDelete && <DropdownMenuItem variant="destructive" onSelect={() => {
            deleteInvoker.current = actionTriggers.current.get(`node:${r.id}`) ?? null;
            setDeleteTarget(r);
          }}><Trash2 aria-hidden="true" />{t("delete")}</DropdownMenuItem>}
        </RowActions>
      ) : null
    },
  ];

  const assignmentColumns = [
    { key: "proxy", label: t("proxyNodes.assignmentProxy"), sortable: true, filterOptions: proxyFilterOptions, render: (r: ProxyNodeAssignment) => (
      <div className="flex min-w-0 flex-col">
        <span className="truncate text-sm font-medium">{r.proxy_node_name ?? r.proxy_node_id}</span>
        <span className="truncate font-mono text-[11px] text-muted-foreground">{r.proxy_node_slug ?? r.proxy_node_id}</span>
      </div>
    )},
    { key: "scope_type", label: t("proxyNodes.assignmentScope"), sortable: true, filterOptions: scopeFilterOptions, render: (r: ProxyNodeAssignment) => {
      const isGlobal = r.scope_type === "global";
      const typeLabel = t(
        `connectionConfig.scope.${SCOPE_I18N_KEY[r.scope_type as ConnConfScopeType] ?? "global"}`,
      );
      const target = assignmentScopeTarget(r.scope_type, r.scope_id);
      return (
        <div className="flex min-w-0 items-center gap-2">
          <span className="inline-flex shrink-0 items-center gap-1 rounded-md bg-primary/15 px-2 py-0.5 text-xs font-semibold text-primary">
            {isGlobal ? <Globe className="h-3 w-3" /> : <span className="h-1.5 w-1.5 rounded-full bg-primary" />}
            {typeLabel}
          </span>
          <span className="min-w-0 truncate text-xs">
            {isGlobal
              ? t("connectionConfig.scope.appliesAll")
              : <EntityRef name={target?.name} id={target?.id} />}
          </span>
        </div>
      );
    }},
    { key: "weight", label: t("proxyNodes.assignmentWeight"), sortable: true, render: (r: ProxyNodeAssignment) => <span className="font-mono text-[11px]">{r.weight}</span> },
    { key: "enabled", label: t("status"), sortable: true, filterOptions: enabledFilterOptions, render: (r: ProxyNodeAssignment) => (
      <StatusChip status={r.enabled ? "positive" : "neutral"} label={r.enabled ? t("enabled") : t("disabled")} />
    )},
    {
      key: "actions", label: "", excludeFromSettings: true, render: (r: ProxyNodeAssignment) => canUpdate || canDelete ? (
        <RowActions label={`${t("actions")}: ${r.proxy_node_name ?? r.proxy_node_id} · ${r.scope_type === "global"
          ? t("connectionConfig.scope.appliesAll")
          : assignmentScopeTarget(r.scope_type, r.scope_id)?.name ?? r.scope_id ?? r.id}`} ref={node => {
          if (node) actionTriggers.current.set(`assignment:${r.id}`, node);
          else actionTriggers.current.delete(`assignment:${r.id}`);
        }}>
          {canUpdate && <DropdownMenuItem asChild><Link to={proxyAssignmentEditHref(r.id)}><Pencil aria-hidden="true" />{t("edit")}</Link></DropdownMenuItem>}
          {canDelete && <DropdownMenuItem variant="destructive" onSelect={() => {
            deleteInvoker.current = actionTriggers.current.get(`assignment:${r.id}`) ?? null;
            setAssignmentDeleteTarget(r);
          }}><Trash2 aria-hidden="true" />{t("delete")}</DropdownMenuItem>}
        </RowActions>
      ) : null
    },
  ];

  return (
    <div className="flex min-w-0 flex-col gap-4">
      <PageChrome
        title={t("proxyNodes.title")}
        icon={Radio}
        description={t("proxyNodes.description")}
        badge={total > 0 ? String(total) : undefined}
        actions={canCreate ? <>
          <Button asChild size="sm" className="gap-2 text-sm"><Link to={PROXY_NODE_NEW_ROUTE}><Plus className="size-4" /> {t("proxyNodes.addNode")}</Link></Button>
          <Button asChild variant="outline" size="sm" className="gap-2 text-sm"><Link to={PROXY_ASSIGNMENT_NEW_ROUTE}><Plus className="size-4" />{t("proxyNodes.addAssignment")}</Link></Button>
        </> : undefined}
      />
      <DataTable
        tableId="connections.proxies"
        columns={columns}
        data={nodes}
        getRowKey={r => r.id}
        loading={proxyQuery.isPending}
        error={proxyQuery.isError ? proxyQuery.error : undefined}
        onRetry={() => proxyQuery.refetch()}
        sortState={sanitizedSortState}
        onSortChange={nodeTable.setSortState}
        columnFilters={sanitizedNodeFilters}
        onColumnFilterChange={nodeTable.setColumnFilter}
        pagination={{
          page,
          pageSize,
          total,
          onPageChange: setPage,
          onPageSizeChange: setPageSize,
        }}
        emptyState={<EmptyState icon={Radio} title={t("proxyNodes.noNodes")} description={t("proxyNodes.noNodesDescription")} actionLabel={canCreate ? t("proxyNodes.addNode") : undefined} onAction={canCreate ? () => { void navigate(PROXY_NODE_NEW_ROUTE); } : undefined} />}
        toolbar={
          <SearchInput value={search} onValueChange={setSearch} placeholder={t("proxyNodes.searchPlaceholder")} />
        }
      />

      <DataTable
        tableId="connections.proxyAssignments"
        columns={assignmentColumns}
        data={assignments}
        getRowKey={r => r.id}
        loading={assignmentsQuery.isPending}
        error={assignmentsQuery.isError ? assignmentsQuery.error : undefined}
        onRetry={() => assignmentsQuery.refetch()}
        sortState={sanitizedAssignmentSortState}
        onSortChange={assignmentTable.setSortState}
        columnFilters={sanitizedAssignmentFilters}
        onColumnFilterChange={assignmentTable.setColumnFilter}
        pagination={{
          page: assignmentPage,
          pageSize: assignmentPageSize,
          total: assignmentTotal,
          onPageChange: setAssignmentPage,
          onPageSizeChange: setAssignmentPageSize,
        }}
        emptyState={
          <EmptyState
            className="py-10 sm:py-16"
            icon={Route}
            title={t("proxyNodes.noAssignments")}
            description={nodes.length === 0 ? t("proxyNodes.noAssignmentsNeedNode") : t("proxyNodes.noAssignmentsDescription")}
            actionLabel={nodes.length === 0 || !canCreate ? undefined : t("proxyNodes.addAssignment")}
            onAction={nodes.length === 0 || !canCreate ? undefined : () => { void navigate(PROXY_ASSIGNMENT_NEW_ROUTE); }}
          />
        }
        toolbar={
          <div className="flex w-full items-center justify-between gap-2">
            <div className="min-w-0">
              <h2 className="text-sm font-semibold text-foreground">{t("proxyNodes.assignmentsTitle")}</h2>
              <p className="text-xs text-muted-foreground">{t("proxyNodes.assignmentsDescription")}</p>
            </div>
          </div>
        }
      />

      <ProxyNodeBootstrapDialog bootstrap={bootstrapDialog} onClose={() => {
        setBootstrapDialog(null);
        requestAnimationFrame(() => bootstrapInvoker.current?.focus());
      }} />

      <AlertDialog open={!!deleteTarget} onOpenChange={open => !open && setDeleteTarget(null)}>
        <AlertDialogContent className="surface" onCloseAutoFocus={event => {
          event.preventDefault();
          deleteInvoker.current?.focus();
        }}>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("proxyNodes.deleteTitle")}</AlertDialogTitle>
            <AlertDialogDescription>
              <Trans
                i18nKey="proxyNodes.deleteDescription"
                values={{ name: deleteTarget?.name ?? "" }}
                components={{ 1: <strong /> }}
              />
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("cancel")}</AlertDialogCancel>
            <AlertDialogAction onClick={handleDelete} className="bg-destructive text-destructive-foreground hover:bg-destructive/90">{t("delete")}</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <AlertDialog open={!!assignmentDeleteTarget} onOpenChange={open => !open && setAssignmentDeleteTarget(null)}>
        <AlertDialogContent className="surface" onCloseAutoFocus={event => {
          event.preventDefault();
          deleteInvoker.current?.focus();
        }}>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("proxyNodes.deleteAssignmentTitle")}</AlertDialogTitle>
            <AlertDialogDescription>{t("proxyNodes.deleteAssignmentDescription")}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("cancel")}</AlertDialogCancel>
            <AlertDialogAction onClick={handleAssignmentDelete} className="bg-destructive text-destructive-foreground hover:bg-destructive/90">{t("delete")}</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
};

export default ProxyNodes;
