import { useEffect, useMemo, useRef, useState } from "react";
import { Link, useLocation, useSearchParams } from "react-router";
import { useTranslation } from "react-i18next";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Pencil, Plus, Radio, Trash2 } from "lucide-react";
import { useApiMutation } from "@/shared/api/hooks";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { useTablePageSize } from "@/shared/hooks/useTablePageSize";
import DataTable, { type Column } from "@/shared/ui-shared/DataTable";
import EmptyState from "@/shared/ui-shared/EmptyState";
import RowActions from "@/shared/ui-shared/RowActions";
import ConfirmDeleteDialog from "@/shared/ui-shared/ConfirmDeleteDialog";
import MultiSelect from "@/shared/ui-shared/MultiSelect";
import FormSelect from "@/shared/ui-shared/FormSelect";
import { DropdownMenuItem } from "@/shared/ui-primitives/dropdown-menu";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import { deleteExporterAssignment, listAllExporterAssignments, type ExporterAssignmentItem } from "./api";
import { EXPORTER_ASSIGNMENTS_KEY } from "./keys";
import { useScopeEntities } from "./useScopeEntities";
import { EXPORTER_ASSIGNMENTS_BASE, exporterListContext, observabilityEditHref, observabilityNewHref, observabilityRecordHref } from "./observabilityRoutes";
import { EXPORTER_ASSIGNMENTS_TABLE_CONFIG } from "./tableConfig";
import { MONITORING_SCOPE_I18N_KEY, type MonitoringScopeType } from "./scope";
import { sanitizeDataTableColumnFilters, sanitizeDataTableSortState } from "@/shared/ui-shared/tableState";

export default function ServiceDiscoveryScreen() {
  const { t } = useTranslation();
  const { can } = usePermissions();
  const qc = useQueryClient();
  const location = useLocation();
  const { search } = location;
  const state: unknown = location.state;
  const focusOnReturn = typeof state === "object" && state !== null && "focusExporterList" in state && state.focusExporterList === true;
  const [params, setParams] = useSearchParams();
  const pageSize = useTablePageSize();
  const query = useQuery({ queryKey: EXPORTER_ASSIGNMENTS_KEY, queryFn: () => listAllExporterAssignments() });
  const entities = useScopeEntities(t("observability.workflow.allTargets", { defaultValue: "All matching targets" }), { includeConnectionTargets: query.data?.some(row => row.scope_type === "connection_target") });
  const context = exporterListContext(search);
  const href = (row: ExporterAssignmentItem) => observabilityRecordHref(EXPORTER_ASSIGNMENTS_BASE, row.id) + context;
  const sort = sanitizeDataTableSortState(params.get("sort") ? { key: params.get("sort")!, dir: params.get("order") === "desc" ? "desc" : "asc" } : null, EXPORTER_ASSIGNMENTS_TABLE_CONFIG.sortKeys);
  const filters = sanitizeDataTableColumnFilters(Object.fromEntries(EXPORTER_ASSIGNMENTS_TABLE_CONFIG.filterColumns.map(({ colKey }) => [colKey, params.get(colKey)?.split(",") ?? []])), EXPORTER_ASSIGNMENTS_TABLE_CONFIG.filterColumns.map(c => c.colKey));
  const update = (values: Record<string, string | null>, reset = true, replace = false) => setParams(current => {
    const next = new URLSearchParams(current);
    for (const [key, value] of Object.entries(values)) { if (value) next.set(key, value); else next.delete(key); }
    if (reset) next.delete("page");
    return next;
  }, { replace });
  const assignments = useMemo(() => query.data ?? [], [query.data]);
  const value = (row: ExporterAssignmentItem, key: string): string | number => {
    switch (key) {
      case "exporter": return row.exporter_name;
      case "scope_id": return entities.resolveName(row.scope_type, row.scope_id);
      case "enabled": return String(row.enabled);
      case "weight": return row.weight;
      case "job": return row.job ?? "";
      default: return row.scope_type;
    }
  };
  const term = (params.get("search") ?? "").trim().toLocaleLowerCase();
  const rows = assignments.filter(row => {
    if (term && ![row.name, row.exporter_name, row.exporter_slug, row.scope_id, entities.resolveName(row.scope_type, row.scope_id), row.job].some(v => v?.toLocaleLowerCase().includes(term))) return false;
    return Object.entries(filters).every(([key, selected]) => !selected.length || selected.includes(key === "exporter" ? row.exporter_id ?? row.exporter_slug : key === "scope_id" ? row.scope_id ?? "" : String(value(row, key))));
  }).sort((a, b) => {
    const first = sort ? value(a, sort.key) : a.name || a.exporter_name;
    const second = sort ? value(b, sort.key) : b.name || b.exporter_name;
    const compared = typeof first === "number" && typeof second === "number" ? first - second : String(first).localeCompare(String(second));
    return (sort?.dir === "desc" ? -compared : compared) || a.id.localeCompare(b.id);
  });
  const requestedPage = /^\d+$/.test(params.get("page") ?? "") ? Math.max(0, Number(params.get("page")) - 1) : 0;
  const page = Math.min(requestedPage, Math.max(0, Math.ceil(rows.length / pageSize.value) - 1));
  useEffect(() => {
    if (!query.isSuccess || requestedPage === page) return;
    setParams(current => { const next = new URLSearchParams(current); if (page) next.set("page", String(page + 1)); else next.delete("page"); return next; }, { replace: true });
  }, [query.isSuccess, requestedPage, page, setParams]);
  const [deleting, setDeleting] = useState<ExporterAssignmentItem | null>(null);
  const focus = useRef<HTMLButtonElement | null>(null);
  const rowTriggers = useRef(new Map<string, HTMLButtonElement>());
  const searchInput = useRef<HTMLInputElement>(null);
  const mutation = useApiMutation({
    mutationFn: (id: string) => {
      if (!can("DELETE", "MONITORING")) throw new Error("Permission removed");
      return deleteExporterAssignment(id);
    },
    onSuccess: (_, id) => {
      qc.setQueryData<ExporterAssignmentItem[]>(EXPORTER_ASSIGNMENTS_KEY, current => current?.filter(row => row.id !== id));
      void qc.invalidateQueries({ queryKey: EXPORTER_ASSIGNMENTS_KEY });
      void qc.invalidateQueries({ queryKey: ["observability", "service-discovery-preview"] });
      setDeleting(null);
    },
  });
  const options = (key: string) => Array.from(new Map(assignments.map(row => {
    const id = key === "exporter" ? row.exporter_id ?? row.exporter_slug : key === "scope_id" ? row.scope_id ?? "" : String(value(row, key));
    return [id, { value: id, label: key === "enabled" ? t(row.enabled ? "enabled" : "disabled") : key === "scope_type" ? t(`observability.scope.${MONITORING_SCOPE_I18N_KEY[row.scope_type as MonitoringScopeType]}`, { defaultValue: row.scope_type }) : String(value(row, key)) }];
  })).values()).filter(option => option.value);
  const columns: Column<ExporterAssignmentItem>[] = [
    { key: "scope_id", label: t("observability.workflow.assignmentTarget", { defaultValue: "Assignment and target" }), filterOptions: options("scope_id"), render: row => {
      const destination = entities.resolveName(row.scope_type, row.scope_id);
      return <div className="min-w-0 whitespace-normal break-words">
        <Link aria-label={row.name || destination} className="flex min-w-0 flex-col underline-offset-2 hover:underline focus-visible:ring-2 focus-visible:ring-ring" to={href(row)}>
          {row.name && <span title={row.name} className="order-2 line-clamp-1 text-xs text-muted-foreground sm:order-1 sm:line-clamp-none sm:text-sm sm:font-medium sm:text-primary">{row.name}</span>}
          <span title={destination} className="order-1 line-clamp-2 font-medium text-primary sm:order-2 sm:line-clamp-none sm:text-xs sm:font-normal sm:text-muted-foreground">{destination}</span>
        </Link>
        <span title={`${row.exporter_name} (${row.exporter_slug})`} className="line-clamp-1 text-xs text-muted-foreground sm:hidden">{row.exporter_name}</span>
      </div>;
    } },
    { key: "exporter", label: t("observability.sd.exporter"), className: "hidden sm:table-cell", filterOptions: options("exporter"), render: row => <div className="whitespace-normal break-words">{row.exporter_name}<div className="text-xs text-muted-foreground">{row.exporter_slug}</div></div> },
    { key: "scope_type", label: t("observability.scope.typeLabel"), filterOptions: options("scope_type"), className: "hidden lg:table-cell", render: row => t(`observability.scope.${MONITORING_SCOPE_I18N_KEY[row.scope_type as MonitoringScopeType]}`, { defaultValue: row.scope_type }) },
    { key: "job", label: t("observability.workflow.scrape", { defaultValue: "Scrape configuration" }), className: "hidden md:table-cell", render: row => <span className="whitespace-normal break-all">{[row.target, row.job, row.scheme, row.port, row.metrics_path].filter(v => v != null && v !== "").join(" · ") || t("observability.workflow.profileDefaults", { defaultValue: "From profile" })}</span> },
    { key: "weight", label: t("observability.sd.weight"), className: "hidden lg:table-cell", render: row => row.weight },
    { key: "enabled", label: t("observability.sd.enabled"), filterOptions: options("enabled"), render: row => <><span className="sm:hidden" aria-label={t(row.enabled ? "enabled" : "disabled")}>{t(row.enabled ? "observability.on" : "observability.off")}</span><span className="hidden sm:inline">{t(row.enabled ? "enabled" : "disabled")}</span></> },
    { key: "actions", label: "", excludeFromSettings: true, render: row => <RowActions label={`${t("actions")}: ${row.name || row.exporter_name}`} ref={node => { if (node) rowTriggers.current.set(row.id, node); else rowTriggers.current.delete(row.id); }}>
      <DropdownMenuItem asChild><Link to={href(row)}>{t("observability.workflow.details", { defaultValue: "Details" })}</Link></DropdownMenuItem>
      {can("UPDATE", "MONITORING") && <DropdownMenuItem asChild><Link to={observabilityEditHref(EXPORTER_ASSIGNMENTS_BASE, row.id) + context}><Pencil className="size-4" />{t("edit")}</Link></DropdownMenuItem>}
      {can("DELETE", "MONITORING") && <DropdownMenuItem variant="destructive" onSelect={() => { focus.current = rowTriggers.current.get(row.id) ?? null; mutation.reset(); setDeleting(row); }}><Trash2 className="size-4" />{t("delete")}</DropdownMenuItem>}
    </RowActions> },
  ];
  return <div className="flex min-h-0 min-w-0 flex-1 flex-col gap-4">
    <div className="flex flex-wrap items-center justify-between gap-4">
      <Input ref={searchInput} autoFocus={focusOnReturn} className="w-full sm:max-w-sm" aria-label={t("search")} placeholder={t("search")} value={params.get("search") ?? ""} onChange={event => update({ search: event.target.value }, true, true)} />
      {can("CREATE", "MONITORING") && <Button asChild><Link to={observabilityNewHref(EXPORTER_ASSIGNMENTS_BASE) + context}><Plus className="size-4" />{t("observability.sd.add")}</Link></Button>}
    </div>
    <details className="sm:hidden">
      <summary className="min-h-9 cursor-pointer rounded-md py-2 text-sm font-medium focus-visible:ring-2 focus-visible:ring-ring">{t("observability.workflow.filtersSort", { defaultValue: "Filters and sorting" })}</summary>
      <div className="grid gap-4 py-2">
        <MultiSelect aria-label={t("observability.sd.exporter")} placeholder={t("observability.sd.exporter")} items={options("exporter").map(option => ({ id: option.value, label: option.label }))} value={filters.exporter ?? []} onValueChange={values => update({ exporter: values.join(",") })} />
        <FormSelect label={t("observability.workflow.sort", { defaultValue: "Sort" })} value={sort ? `${sort.key}:${sort.dir}` : "default"} onValueChange={next => { const [key = "default", order] = next.split(":"); update({ sort: key === "default" ? null : key, order: order ?? null }); }} options={[
          { value: "default", label: t("observability.sd.schemeDefault") },
          ...columns.filter(column => column.key !== "actions").flatMap(column => (["asc", "desc"] as const).map(dir => ({ value: `${column.key}:${dir}`, label: `${column.label} · ${t(`observability.workflow.${dir}`)}` }))),
        ]} />
      </div>
    </details>
    {query.isPending && <output aria-live="polite">{t("loading")}</output>}
    <DataTable tableId="observability.exporters" columns={columns} data={rows.slice(page * pageSize.value, (page + 1) * pageSize.value)} getRowKey={row => row.id}
      loading={query.isPending} error={query.isError ? t("observability.entityLoadFailed") : undefined} onRetry={() => query.refetch()}
      className="min-h-0 flex-1 max-sm:[&_table]:table-fixed max-sm:[&_th]:px-1 max-sm:[&_td]:px-1 max-sm:[&_th>div]:flex-wrap max-sm:[&_th_[data-column-sort-trigger]>span]:whitespace-normal max-sm:[&_th:first-child]:w-[64%] max-sm:[&_th:nth-last-child(2)]:w-[23%] max-sm:[&_th:last-child]:w-[13%]" sortState={sort} onSortChange={next => update({ sort: next?.key ?? null, order: next?.dir ?? null })}
      columnFilters={filters} onColumnFilterChange={(key, values) => update({ [key]: values.join(",") })}
      pagination={query.data ? { page, pageSize: pageSize.value, total: rows.length, onPageChange: next => update({ page: next ? String(next + 1) : null }, false), onPageSizeChange: next => { pageSize.setValue(next); update({ page: null }); } } : undefined}
      emptyState={<EmptyState icon={Radio} title={t("observability.sd.empty")} description={t("observability.sd.emptyHint")} />} />
    <ConfirmDeleteDialog open={!!deleting} onOpenChange={open => { if (!open && !mutation.isPending) setDeleting(null); }} title={t("delete")} description={<>{deleting?.name || deleting?.exporter_name} ({deleting?.id}){mutation.isError && <span role="alert" className="block text-destructive">{t("observability.entityDeleteFailed")}</span>}</>} cancelLabel={t("cancel")} confirmLabel={t("delete")} cancelDisabled={mutation.isPending} onCloseAutoFocus={event => { event.preventDefault(); (focus.current?.isConnected ? focus.current : searchInput.current)?.focus(); }} confirmProps={{ disabled: mutation.isPending || !can("DELETE", "MONITORING"), "aria-busy": mutation.isPending, onClick: event => { event.preventDefault(); if (deleting && !mutation.isPending && can("DELETE", "MONITORING")) mutation.mutate(deleting.id); } }} />
    {mutation.isError && <p role="alert" className="text-destructive">{t("observability.entityDeleteFailed")}</p>}
  </div>;
}
