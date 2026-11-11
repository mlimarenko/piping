import { Link, useNavigate, useParams } from "react-router";
import { useTranslation } from "react-i18next";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Layers, Pencil, Plus, Trash2 } from "lucide-react";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { useApiMutation } from "@/shared/api/hooks";
import EntityDetail from "@/shared/ui-shared/EntityDetail";
import PageState from "@/shared/ui-shared/PageState";
import { Button } from "@/shared/ui-primitives/button";
import { getProfile, deleteProfile, listAllProfileAssignments } from "./api";
import { PROFILES_KEY, PROFILE_ASSIGNMENTS_KEY } from "./keys";
import { PROFILES_BASE } from "./ProfilesScreen";
import { PROFILE_ASSIGNMENTS_BASE } from "./observabilityRoutes";
import { useScopeEntities } from "./useScopeEntities";

export default function ProfileDetail() {
  const { id } = useParams();
  const { t } = useTranslation();
  const { can } = usePermissions();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const profile = useQuery({ queryKey: [...PROFILES_KEY, id], queryFn: () => getProfile(id!) });
  const assignments = useQuery({ queryKey: [...PROFILE_ASSIGNMENTS_KEY, { profile_id: id }], queryFn: () => listAllProfileAssignments({ profile_id: id }) });
  const entities = useScopeEntities(t("observability.scope.global"), { includeConnectionTargets: true });
  const mutation = useApiMutation({ mutationFn: () => {
    if (!id || !can("DELETE", "MONITORING")) throw new Error("Permission removed");
    return deleteProfile(id);
  }, onSuccess: () => { void qc.invalidateQueries({ queryKey: PROFILES_KEY }); void navigate("/observability?tab=profiles"); } });
  if (profile.isPending) return <PageState kind="loading" title={t("loading")} />;
  if (profile.isError) return <PageState kind="error" title={t("observability.entityLoadFailed")} action={<Button onClick={() => void profile.refetch()}>{t("retry")}</Button>} />;
  const item = profile.data;
  return <EntityDetail icon={Layers} title={item.name} description={item.description ?? item.slug} backHref="/observability?tab=profiles" error={mutation.isError ? t("observability.entityDeleteFailed") : undefined}
    fields={[
      { label: "Slug", value: item.slug },
      { label: t("enabled"), value: t(item.enabled ? "enabled" : "disabled") },
      { label: t("delete"), value: t(item.assignment_count > 0 ? "observability.profiles.assignedDeleteHint" : "observability.profiles.deleteHint") },
      { label: t("observability.profiles.items"), value: <ul className="grid gap-2">{item.items.map(entry => <li key={entry.id} className="min-w-0 break-words"><span className="font-medium">{entry.exporter_name}</span><span className="block font-mono text-xs">{entry.effective_job} · {entry.port ?? t("observability.profiles.exporterDefaults")} · {entry.metrics_path ?? t("observability.profiles.exporterDefaults")}</span></li>)}</ul> },
      ...([ ["metric_set_id", "metricSet", "/observability/sets"], ["query_source_id", "querySource", "/observability/sources"], ["alert_source_id", "alertSource", "/observability/alert-sources"] ] as const).map(([field, label, base]) => ({ label: t(`observability.profiles.${label}`), value: item[field] ? <Link className="break-all text-primary hover:underline" to={`${base}/${item[field]}`}>{item[field]}</Link> : "—" })),
      { label: t("observability.profiles.assignments"), value: assignments.isPending ? t("loading") : assignments.isError ? <div role="alert">{t("observability.entityLoadFailed")}<Button onClick={() => void assignments.refetch()}>{t("retry")}</Button></div> : <ul className="grid gap-2">{assignments.data?.length ? assignments.data.map(assignment => <li key={assignment.id}><Link className="break-words text-primary hover:underline" to={`${PROFILE_ASSIGNMENTS_BASE}/${assignment.id}`}>{entities.resolveName(assignment.scope_type, assignment.scope_id)} · {t(assignment.enabled ? "enabled" : "disabled")}</Link></li>) : <li>{t("observability.profiles.noAssignments")}</li>}</ul> },
    ]}
    actions={[
      { key: "apply", label: t("observability.profiles.newAssignment"), icon: Plus, allowed: can("CREATE", "MONITORING"), disabled: mutation.isPending, onClick: () => void navigate(`${PROFILE_ASSIGNMENTS_BASE}/new?profile_id=${encodeURIComponent(item.id)}`) },
      { key: "edit", label: t("edit"), icon: Pencil, allowed: can("UPDATE", "MONITORING"), disabled: mutation.isPending, onClick: () => void navigate(`${PROFILES_BASE}/${id}/edit`) },
      { key: "delete", label: t("delete"), icon: Trash2, allowed: can("DELETE", "MONITORING"), disabled: mutation.isPending || item.assignment_count > 0 || !assignments.isSuccess || !!assignments.data?.length, confirm: { title: t("delete"), description: t("observability.profiles.deleteHint"), onConfirm: () => mutation.mutate() } },
    ]}
  />;
}
