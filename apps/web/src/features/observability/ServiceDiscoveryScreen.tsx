import { useEffect, useMemo, useRef, useState } from "react";
import { Link, useLocation, useSearchParams } from "react-router";
import { useTranslation } from "react-i18next";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Copy, Pencil, Plus, Radio, Trash2 } from "lucide-react";
import { useApiMutation } from "@/shared/api/hooks";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { useTablePageSize } from "@/shared/hooks/useTablePageSize";
import DataTable, { type Column } from "@/shared/ui-shared/DataTable";
import EmptyState from "@/shared/ui-shared/EmptyState";
import RowActions from "@/shared/ui-shared/RowActions";
import ConfirmDeleteDialog from "@/shared/ui-shared/ConfirmDeleteDialog";
import MultiSelect from "@/shared/ui-shared/MultiSelect";
import FormSelect from "@/shared/ui-shared/FormSelect";
import FormField from "@/shared/ui-shared/FormField";
import { DropdownMenuItem } from "@/shared/ui-primitives/dropdown-menu";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/shared/ui-primitives/dialog";
import { Button } from "@/shared/ui-primitives/button";
import SearchInput from "@/shared/ui-shared/SearchInput";
import { createProfileAssignment, deleteProfileAssignment, listAllProfileAssignments, listAllProfiles, type ProfileAssignment, type MonitoringProfile } from "./api";
import { PROFILE_ASSIGNMENTS_KEY, PROFILES_KEY, invalidateMonitoringProfileCaches } from "./keys";
import ScopeFields from "./ScopeFields";
import { useScopeEntities } from "./useScopeEntities";
import { PROFILE_ASSIGNMENTS_BASE, profileListContext, observabilityEditHref, observabilityRecordHref } from "./observabilityRoutes";
import { PROFILE_ASSIGNMENTS_TABLE_CONFIG } from "./tableConfig";
import { MONITORING_SCOPE_I18N_KEY, type MonitoringScopeType } from "./scope";
import { sanitizeDataTableColumnFilters, sanitizeDataTableSortState } from "@/shared/ui-shared/tableState";

interface AssignmentScopeDraft {
  scopeType: MonitoringScopeType;
  scopeId: string;
}

/** Global scope carries no entity id; every other layer needs a concrete one. */
function scopeBody({ scopeType, scopeId }: AssignmentScopeDraft) {
  return {
    scope_type: scopeType,
    scope_id: scopeType === "global" ? null : scopeId || null,
  };
}

interface BulkCreateAssignmentsDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

/**
 * Creates one profile assignment per selected catalog profile in a single
 * scope. The selection IS the request: profiles that are missing from the
 * catalog or rejected by the backend are reported, never silently skipped.
 */
function BulkCreateAssignmentsDialog({ open, onOpenChange }: BulkCreateAssignmentsDialogProps) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const [scopeType, setScopeType] = useState<MonitoringScopeType>("global");
  const [scopeId, setScopeId] = useState("");
  const [profileIds, setExporterIds] = useState<string[]>([]);
  const [failedSlugs, setFailedSlugs] = useState<string[]>([]);
  const formKey = open ? "open" : "closed";
  const [prevFormKey, setPrevFormKey] = useState(formKey);
  if (prevFormKey !== formKey) {
    setPrevFormKey(formKey);
    setScopeType("global");
    setScopeId("");
    setExporterIds([]);
    setFailedSlugs([]);
  }

  const entities = useScopeEntities(t("observability.workflow.allTargets", { defaultValue: "All matching targets" }), { includeConnectionTargets: open && scopeType === "connection_target" });
  const catalog = useQuery({ queryKey: PROFILES_KEY, queryFn: () => listAllProfiles(), enabled: open, staleTime: 60_000 });
  const profiles = useMemo(() => catalog.data ?? [], [catalog.data]);
  const selectedProfiles = useMemo(() => profiles.filter(profile => profileIds.includes(profile.id)), [profiles, profileIds]);
  const unknownIds = profileIds.filter(id => !profiles.some(profile => profile.id === id));

  const mutation = useApiMutation({
    mutationFn: async (input: { scope: AssignmentScopeDraft; profiles: MonitoringProfile[] }) => {
      const failed: string[] = [];
      for (const profile of input.profiles) {
        try {
          await createProfileAssignment({
            profile_id: profile.id,
            enabled: true,
            ...scopeBody(input.scope),
          });
        } catch {
          failed.push(profile.slug);
        }
      }
      return { failed };
    },
    onSettled: () => {
      void invalidateMonitoringProfileCaches(qc);
    },
  });

  const canSubmit =
    profileIds.length > 0 && unknownIds.length === 0 && (scopeType === "global" || !!scopeId) &&
    !catalog.isPending && !mutation.isPending;

  return <Dialog open={open} onOpenChange={next => { if (!mutation.isPending) onOpenChange(next); }}>
    <DialogContent className="max-w-lg" description={t("observability.sd.bulkCreateHint")}>
      <DialogHeader><DialogTitle>{t("observability.sd.bulkCreateTitle")}</DialogTitle></DialogHeader>
      <fieldset disabled={mutation.isPending} className="grid min-w-0 gap-4">
        <ScopeFields
          disabled={mutation.isPending}
          scopeType={scopeType}
          scopeId={scopeId}
          onScopeTypeChange={setScopeType}
          onScopeIdChange={setScopeId}
          entities={entities}
        />
        <FormField
          label={t("observability.sd.bulkCreateProfiles")}
          help={catalog.isPending
            ? t("loading", { defaultValue: "Loading…" })
            : catalog.isError
              ? t("observability.entityLoadFailed")
              : t("observability.sd.bulkCreateProfilesHint")}
        >
          <MultiSelect
            aria-label={t("observability.sd.bulkCreateProfiles")}
            placeholder={t("observability.sd.profilePlaceholder")}
            items={profiles.map(profile => ({ id: profile.id, label: profile.name }))}
            value={profileIds}
            onValueChange={setExporterIds}
          />
        </FormField>
        {unknownIds.length > 0 && <p role="alert" className="text-sm text-destructive">{t("observability.sd.bulkCreateProfilesMissing")}</p>}
        {failedSlugs.length > 0 && <p role="alert" className="text-sm text-destructive">{t("observability.sd.bulkCreateFailed", { count: failedSlugs.length, profiles: failedSlugs.join(", ") })}</p>}
      </fieldset>
      <DialogFooter>
        <Button variant="outline" disabled={mutation.isPending} onClick={() => onOpenChange(false)}>{t("cancel")}</Button>
        <Button
          disabled={!canSubmit}
          aria-busy={mutation.isPending}
          onClick={() => {
            if (!canSubmit) return;
            setFailedSlugs([]);
            mutation.mutate({ scope: { scopeType, scopeId }, profiles: selectedProfiles }, {
              onSuccess: result => {
                if (result.failed.length) { setFailedSlugs(result.failed); return; }
                setExporterIds([]);
                onOpenChange(false);
              },
            });
          }}
        >
          {mutation.isPending
            ? t("observability.sd.bulkCreatePending")
            : t("observability.sd.bulkCreateSubmit", { count: profileIds.length })}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>;
}

interface CloneAssignmentDialogProps {
  open: boolean;
  source: ProfileAssignment | null;
  onOpenChange: (open: boolean) => void;
}

/**
 * Copies an assignment to another scope, keeping its scrape overrides. The
 * copy is a new assignment; the original is untouched.
 */
function CloneAssignmentDialog({ open, source, onOpenChange }: CloneAssignmentDialogProps) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const initialScopeType = (source?.scope_type as MonitoringScopeType | undefined) ?? "global";
  const [scopeType, setScopeType] = useState<MonitoringScopeType>(initialScopeType);
  const [scopeId, setScopeId] = useState("");
  const formKey = `${open ? "open" : "closed"}:${source?.id ?? ""}`;
  const [prevFormKey, setPrevFormKey] = useState(formKey);
  if (prevFormKey !== formKey) {
    setPrevFormKey(formKey);
    setScopeType(initialScopeType);
    setScopeId("");
  }

  const entities = useScopeEntities(t("observability.workflow.allTargets", { defaultValue: "All matching targets" }), {
    includeConnectionTargets: open && (scopeType === "connection_target" || source?.scope_type === "connection_target"),
  });

  const sameScope = !!source && scopeType === source.scope_type &&
    (scopeType === "global" || scopeId === (source.scope_id ?? ""));

  const mutation = useApiMutation({
    mutationFn: (scope: AssignmentScopeDraft) => {
      if (!source) throw new Error("No assignment to clone");
      return createProfileAssignment({
        profile_id: source.profile_id,
        enabled: source.enabled,
        target: source.target ?? null,
        weight: source.weight,
        ...scopeBody(scope),
      });
    },
    onSuccess: () => {
      void invalidateMonitoringProfileCaches(qc);
      onOpenChange(false);
    },
  });

  const canSubmit = !!source && (scopeType === "global" || !!scopeId) && !sameScope && !mutation.isPending;

  return <Dialog open={open} onOpenChange={next => { if (!mutation.isPending) onOpenChange(next); }}>
    <DialogContent className="max-w-lg" description={t("observability.sd.cloneDescription")}>
      <DialogHeader><DialogTitle>{t("observability.sd.cloneTitle")}</DialogTitle></DialogHeader>
      {source && <div className="min-w-0 rounded-md border border-border/60 p-3 text-sm">
        <p className="truncate font-medium">{source.profile_name}</p>
        <p className="truncate text-muted-foreground">{source.profile_name} ({source.profile_slug}) · {entities.resolveName(source.scope_type, source.scope_id)}</p>
      </div>}
      <fieldset disabled={mutation.isPending} className="grid min-w-0 gap-4">
        <ScopeFields
          disabled={mutation.isPending}
          scopeType={scopeType}
          scopeId={scopeId}
          onScopeTypeChange={setScopeType}
          onScopeIdChange={setScopeId}
          entities={entities}
        />
        {sameScope && <p className="text-sm text-muted-foreground">{t("observability.sd.cloneSameScope")}</p>}
        {mutation.isError && <p role="alert" className="text-sm text-destructive">{t("observability.sd.cloneFailed")}</p>}
      </fieldset>
      <DialogFooter>
        <Button variant="outline" disabled={mutation.isPending} onClick={() => onOpenChange(false)}>{t("cancel")}</Button>
        <Button
          disabled={!canSubmit}
          aria-busy={mutation.isPending}
          onClick={() => { if (canSubmit) mutation.mutate({ scopeType, scopeId }); }}
        >
          {mutation.isPending ? t("observability.sd.clonePending") : t("observability.sd.cloneSubmit")}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>;
}

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
  const query = useQuery({ queryKey: PROFILE_ASSIGNMENTS_KEY, queryFn: () => listAllProfileAssignments() });
  const entities = useScopeEntities(t("observability.workflow.allTargets", { defaultValue: "All matching targets" }), { includeConnectionTargets: query.data?.some(row => row.scope_type === "connection_target") });
  const context = profileListContext(search);
  const href = (row: ProfileAssignment) => observabilityRecordHref(PROFILE_ASSIGNMENTS_BASE, row.id) + context;
  const sort = sanitizeDataTableSortState(params.get("sort") ? { key: params.get("sort")!, dir: params.get("order") === "desc" ? "desc" : "asc" } : null, PROFILE_ASSIGNMENTS_TABLE_CONFIG.sortKeys);
  const filters = sanitizeDataTableColumnFilters(Object.fromEntries(PROFILE_ASSIGNMENTS_TABLE_CONFIG.filterColumns.map(({ colKey }) => [colKey, params.get(colKey)?.split(",") ?? []])), PROFILE_ASSIGNMENTS_TABLE_CONFIG.filterColumns.map(c => c.colKey));
  const update = (values: Record<string, string | null>, reset = true, replace = false) => setParams(current => {
    const next = new URLSearchParams(current);
    for (const [key, value] of Object.entries(values)) { if (value) next.set(key, value); else next.delete(key); }
    if (reset) next.delete("page");
    return next;
  }, { replace });
  const assignments = useMemo(() => query.data ?? [], [query.data]);
  const value = (row: ProfileAssignment, key: string): string | number => {
    switch (key) {
      case "profile": return row.profile_name;
      case "scope_id": return entities.resolveName(row.scope_type, row.scope_id);
      case "enabled": return String(row.enabled);
      case "target": return row.target ?? "";
      default: return row.scope_type;
    }
  };
  const term = (params.get("search") ?? "").trim().toLocaleLowerCase();
  const rows = assignments.filter(row => {
    if (term && ![row.profile_name, row.profile_slug, row.scope_id, entities.resolveName(row.scope_type, row.scope_id), row.target].some(v => v?.toLocaleLowerCase().includes(term))) return false;
    return Object.entries(filters).every(([key, selected]) => !selected.length || selected.includes(key === "profile" ? row.profile_id ?? row.profile_slug : key === "scope_id" ? row.scope_id ?? "" : String(value(row, key))));
  }).sort((a, b) => {
    const first = sort ? value(a, sort.key) : a.profile_name;
    const second = sort ? value(b, sort.key) : b.profile_name;
    const compared = typeof first === "number" && typeof second === "number" ? first - second : String(first).localeCompare(String(second));
    return (sort?.dir === "desc" ? -compared : compared) || a.id.localeCompare(b.id);
  });
  const requestedPage = /^\d+$/.test(params.get("page") ?? "") ? Math.max(0, Number(params.get("page")) - 1) : 0;
  const page = Math.min(requestedPage, Math.max(0, Math.ceil(rows.length / pageSize.value) - 1));
  useEffect(() => {
    if (!query.isSuccess || requestedPage === page) return;
    setParams(current => { const next = new URLSearchParams(current); if (page) next.set("page", String(page + 1)); else next.delete("page"); return next; }, { replace: true });
  }, [query.isSuccess, requestedPage, page, setParams]);
  const [deleting, setDeleting] = useState<ProfileAssignment | null>(null);
  const [creating, setCreating] = useState(false);
  const [cloning, setCloning] = useState<ProfileAssignment | null>(null);
  const focus = useRef<HTMLButtonElement | null>(null);
  const rowTriggers = useRef(new Map<string, HTMLButtonElement>());
  const searchInput = useRef<HTMLInputElement>(null);
  const mutation = useApiMutation({
    mutationFn: (id: string) => {
      if (!can("DELETE", "MONITORING")) throw new Error("Permission removed");
      return deleteProfileAssignment(id);
    },
    onSuccess: (_, id) => {
      qc.setQueryData<ProfileAssignment[]>(PROFILE_ASSIGNMENTS_KEY, current => current?.filter(row => row.id !== id));
      void invalidateMonitoringProfileCaches(qc);
      setDeleting(null);
    },
  });
  const options = (key: string) => Array.from(new Map(assignments.map(row => {
    const id = key === "profile" ? row.profile_id ?? row.profile_slug : key === "scope_id" ? row.scope_id ?? "" : String(value(row, key));
    return [id, { value: id, label: key === "enabled" ? t(row.enabled ? "enabled" : "disabled") : key === "scope_type" ? t(`observability.scope.${MONITORING_SCOPE_I18N_KEY[row.scope_type as MonitoringScopeType]}`, { defaultValue: row.scope_type }) : String(value(row, key)) }];
  })).values()).filter(option => option.value);
  const columns: Column<ProfileAssignment>[] = [
    { key: "scope_id", label: t("observability.workflow.assignmentTarget", { defaultValue: "Assignment and target" }), filterOptions: options("scope_id"), render: row => {
      const destination = entities.resolveName(row.scope_type, row.scope_id);
      return <div className="min-w-0 whitespace-normal break-words">
        <Link aria-label={destination} className="flex min-w-0 flex-col underline-offset-2 hover:underline focus-visible:ring-2 focus-visible:ring-ring" to={href(row)}>
          <span title={destination} className="order-1 line-clamp-2 font-medium text-primary sm:order-2 sm:line-clamp-none sm:text-xs sm:font-normal sm:text-muted-foreground">{destination}</span>
        </Link>
        <span title={`${row.profile_name} (${row.profile_slug})`} className="line-clamp-1 text-xs text-muted-foreground sm:hidden">{row.profile_name}</span>
      </div>;
    } },
    { key: "profile", label: t("observability.sd.profile"), className: "hidden sm:table-cell", filterOptions: options("profile"), render: row => <div className="whitespace-normal break-words">{row.profile_name}<div className="text-xs text-muted-foreground">{row.profile_slug}</div></div> },
    { key: "scope_type", label: t("observability.scope.typeLabel"), filterOptions: options("scope_type"), className: "hidden lg:table-cell", render: row => t(`observability.scope.${MONITORING_SCOPE_I18N_KEY[row.scope_type as MonitoringScopeType]}`, { defaultValue: row.scope_type }) },
    { key: "target", label: t("observability.sd.target"), className: "hidden md:table-cell", render: row => <span className="whitespace-normal break-all">{row.target || t("observability.workflow.profileDefaults", { defaultValue: "From profile" })}</span> },
    { key: "enabled", label: t("observability.sd.enabled"), filterOptions: options("enabled"), render: row => <><span className="sm:hidden" aria-label={t(row.enabled ? "enabled" : "disabled")}>{t(row.enabled ? "observability.on" : "observability.off")}</span><span className="hidden sm:inline">{t(row.enabled ? "enabled" : "disabled")}</span></> },
    { key: "actions", label: "", excludeFromSettings: true, render: row => <RowActions label={`${t("actions")}: ${row.profile_name}`} ref={node => { if (node) rowTriggers.current.set(row.id, node); else rowTriggers.current.delete(row.id); }}>
      <DropdownMenuItem asChild><Link to={href(row)}>{t("observability.workflow.details", { defaultValue: "Details" })}</Link></DropdownMenuItem>
      {can("CREATE", "MONITORING") && <DropdownMenuItem onSelect={() => setCloning(row)}><Copy className="size-4" />{t("observability.sd.cloneAction")}</DropdownMenuItem>}
      {can("UPDATE", "MONITORING") && <DropdownMenuItem asChild><Link to={observabilityEditHref(PROFILE_ASSIGNMENTS_BASE, row.id) + context}><Pencil className="size-4" />{t("edit")}</Link></DropdownMenuItem>}
      {can("DELETE", "MONITORING") && <DropdownMenuItem variant="destructive" onSelect={() => { focus.current = rowTriggers.current.get(row.id) ?? null; mutation.reset(); setDeleting(row); }}><Trash2 className="size-4" />{t("delete")}</DropdownMenuItem>}
    </RowActions> },
  ];
  return <div className="flex min-h-0 min-w-0 flex-1 flex-col gap-4">
    <details className="sm:hidden">
      <summary className="min-h-9 cursor-pointer rounded-md py-2 text-sm font-medium focus-visible:ring-2 focus-visible:ring-ring">{t("observability.workflow.filtersSort", { defaultValue: "Filters and sorting" })}</summary>
      <div className="grid gap-4 py-2">
        <MultiSelect aria-label={t("observability.sd.profile")} placeholder={t("observability.sd.profile")} items={options("profile").map(option => ({ id: option.value, label: option.label }))} value={filters.profile ?? []} onValueChange={values => update({ profile: values.join(",") })} />
        <FormSelect label={t("observability.workflow.sort", { defaultValue: "Sort" })} value={sort ? `${sort.key}:${sort.dir}` : "default"} onValueChange={next => { const [key = "default", order] = next.split(":"); update({ sort: key === "default" ? null : key, order: order ?? null }); }} options={[
          { value: "default", label: t("observability.sd.schemeDefault") },
          ...columns.filter(column => column.key !== "actions").flatMap(column => (["asc", "desc"] as const).map(dir => ({ value: `${column.key}:${dir}`, label: `${column.label} · ${t(`observability.workflow.${dir}`)}` }))),
        ]} />
      </div>
    </details>
    <DataTable tableId="observability.profiles" columns={columns} data={rows.slice(page * pageSize.value, (page + 1) * pageSize.value)} getRowKey={row => row.id}
      toolbar={<>
        <SearchInput ref={searchInput} autoFocus={focusOnReturn} placeholder={t("search")} value={params.get("search") ?? ""} onValueChange={search => update({ search }, true, true)} />
        {can("CREATE", "MONITORING") && <><Button asChild><Link to={`${PROFILE_ASSIGNMENTS_BASE}/new${context}`}><Plus className="size-4" />{t("observability.profiles.newAssignment")}</Link></Button><Button variant="outline" onClick={() => setCreating(true)}>{t("observability.sd.bulkCreateTitle")}</Button></>}
      </>}
      loading={query.isPending} error={query.isError ? t("observability.entityLoadFailed") : undefined} onRetry={() => query.refetch()}
      className="min-h-0 flex-1 max-sm:[&_table]:table-fixed max-sm:[&_th]:px-1 max-sm:[&_td]:px-1 max-sm:[&_th>div]:flex-wrap max-sm:[&_th_[data-column-sort-trigger]>span]:whitespace-normal max-sm:[&_th:first-child]:w-[64%] max-sm:[&_th:nth-last-child(2)]:w-[23%] max-sm:[&_th:last-child]:w-[13%]" sortState={sort} onSortChange={next => update({ sort: next?.key ?? null, order: next?.dir ?? null })}
      columnFilters={filters} onColumnFilterChange={(key, values) => update({ [key]: values.join(",") })}
      pagination={query.data ? { page, pageSize: pageSize.value, total: rows.length, onPageChange: next => update({ page: next ? String(next + 1) : null }, false), onPageSizeChange: next => { pageSize.setValue(next); update({ page: null }); } } : undefined}
      emptyState={<EmptyState icon={Radio} title={t("observability.sd.empty")} description={t("observability.sd.emptyHint")} />} />
    <ConfirmDeleteDialog open={!!deleting} onOpenChange={open => { if (!open && !mutation.isPending) setDeleting(null); }} title={t("delete")} description={<>{deleting?.profile_name} ({deleting?.id}){mutation.isError && <span role="alert" className="block text-destructive">{t("observability.entityDeleteFailed")}</span>}</>} cancelLabel={t("cancel")} confirmLabel={t("delete")} cancelDisabled={mutation.isPending} onCloseAutoFocus={event => { event.preventDefault(); (focus.current?.isConnected ? focus.current : searchInput.current)?.focus(); }} confirmProps={{ disabled: mutation.isPending || !can("DELETE", "MONITORING"), "aria-busy": mutation.isPending, onClick: event => { event.preventDefault(); if (deleting && !mutation.isPending && can("DELETE", "MONITORING")) mutation.mutate(deleting.id); } }} />
    {mutation.isError && <p role="alert" className="text-destructive">{t("observability.entityDeleteFailed")}</p>}
    <BulkCreateAssignmentsDialog open={creating} onOpenChange={setCreating} />
    <CloneAssignmentDialog open={!!cloning} source={cloning} onOpenChange={open => { if (!open) setCloning(null); }} />
  </div>;
}
