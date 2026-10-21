import { useLocation, useNavigate, useParams } from "react-router";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Copy, Pencil, Radio, Trash2 } from "lucide-react";
import { useApiMutation } from "@/shared/api/hooks";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { notify } from "@/shared/lib/notifications";
import EntityDetail from "@/shared/ui-shared/EntityDetail";
import PageState from "@/shared/ui-shared/PageState";
import EmptyState from "@/shared/ui-shared/EmptyState";
import { useUnsavedChangesGuard } from "@/shared/ui-shared/useUnsavedChangesGuard";
import { Button } from "@/shared/ui-primitives/button";
import { deleteExporterAssignment, listAllExporterAssignments, type ExporterAssignmentItem } from "./api";
import { EXPORTER_ASSIGNMENTS_KEY } from "./keys";
import { useScopeEntities } from "./useScopeEntities";
import { MONITORING_SCOPE_I18N_KEY, type MonitoringScopeType } from "./scope";
import { EXPORTER_ASSIGNMENTS_BASE, exporterAssignmentsHref, exporterListContext, observabilityEditHref } from "./observabilityRoutes";

export default function ExporterAssignmentDetail() {
  const { id } = useParams();
  const { search } = useLocation();
  const { t } = useTranslation();
  const { can } = usePermissions();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const listHref = exporterAssignmentsHref(search);
  const query = useQuery({ queryKey: EXPORTER_ASSIGNMENTS_KEY, queryFn: () => listAllExporterAssignments() });
  const entities = useScopeEntities(t("observability.workflow.allTargets", { defaultValue: "All matching targets" }), { includeConnectionTargets: query.data?.find(row => row.id === id)?.scope_type === "connection_target" });
  const mutation = useApiMutation({
    mutationFn: (recordId: string) => { if (!can("DELETE", "MONITORING")) throw new Error("Permission removed"); return deleteExporterAssignment(recordId); },
    onSuccess: (_, recordId) => {
      qc.setQueryData<ExporterAssignmentItem[]>(EXPORTER_ASSIGNMENTS_KEY, current => current?.filter(row => row.id !== recordId));
      void qc.invalidateQueries({ queryKey: EXPORTER_ASSIGNMENTS_KEY });
      void qc.invalidateQueries({ queryKey: ["observability", "service-discovery-preview"] });
      void navigate(listHref, { state: { focusExporterList: true } });
    },
  });
  useUnsavedChangesGuard(false, mutation.isPending);
  if (query.isPending || query.isError || !query.data.some(row => row.id === id)) return <PageState icon={Radio} title={t("observability.title")} backHref={listHref}>
    {query.isPending ? <output>{t("loading")}</output> : <EmptyState icon={Radio} title={t(query.isError ? "observability.entityLoadFailed" : "observability.entityNotFound")} actionLabel={query.isError ? t("retry") : undefined} onAction={() => { void query.refetch(); }} />}
  </PageState>;
  const item = query.data.find(row => row.id === id)!;
  const copyValue = (value: string) => <div className="flex min-w-0 items-start gap-2"><code className="min-w-0 break-all">{value}</code><Button size="icon-sm" variant="ghost" aria-label={`${t("copy")}: ${value}`} onClick={() => { void navigator.clipboard.writeText(value).then(() => notify("success", t("copied")), () => notify("error", t("observability.workflow.copyFailed", { defaultValue: "Could not copy. Select and copy the value manually." }))); }}><Copy className="size-4" /></Button></div>;
  return <div aria-busy={mutation.isPending} className={mutation.isPending ? "pointer-events-none" : undefined}><EntityDetail icon={Radio} title={item.name || item.exporter_name} description={item.exporter_slug} backHref={listHref}
    error={mutation.isError ? t("observability.entityDeleteFailed") : undefined}
    fields={[
      { label: "ID", value: copyValue(item.id) },
      { label: t("observability.scope.typeLabel"), value: t(`observability.scope.${MONITORING_SCOPE_I18N_KEY[item.scope_type as MonitoringScopeType]}`, { defaultValue: item.scope_type }) },
      { label: t("observability.scope.targetLabel"), value: <div className="grid gap-2"><span className="break-words">{entities.resolveName(item.scope_type, item.scope_id)}</span>{item.scope_id && copyValue(item.scope_id)}</div> },
      { label: t("observability.sd.exporter"), value: item.exporter_name },
      { label: t("observability.workflow.profileSlug", { defaultValue: "Profile slug" }), value: item.exporter_slug },
      { label: t("observability.workflow.profileId", { defaultValue: "Profile ID" }), value: item.exporter_id ? copyValue(item.exporter_id) : "—" },
      { label: t("observability.sd.weight"), value: item.weight },
      { label: t("observability.sd.enabled"), value: t(item.enabled ? "enabled" : "disabled") },
      ...(["target", "job", "scheme", "port", "metrics_path"] as const).map(key => ({ label: t(`observability.sd.${key === "metrics_path" ? "metricsPath" : key}`), value: item[key] == null ? t("observability.workflow.profileDefaults", { defaultValue: "From profile" }) : <span className="break-all">{String(item[key])}</span> })),
      { label: t("observability.sd.labels"), value: <pre className="max-w-full overflow-auto whitespace-pre-wrap break-all">{JSON.stringify(item.labels, null, 2)}</pre> },
    ]}
    actions={[
      { key: "edit", label: t("edit"), icon: Pencil, allowed: can("UPDATE", "MONITORING"), onClick: () => { if (!mutation.isPending) void navigate(observabilityEditHref(EXPORTER_ASSIGNMENTS_BASE, item.id) + exporterListContext(search)); }, disabled: mutation.isPending },
      { key: "delete", label: t("delete"), icon: Trash2, variant: "outline", allowed: can("DELETE", "MONITORING"), disabled: mutation.isPending, confirm: { title: t("delete"), description: `${item.name || item.exporter_name} (${item.id})`, onConfirm: () => { if (can("DELETE", "MONITORING") && !mutation.isPending) mutation.mutate(item.id); } } },
    ]} /></div>;
}
