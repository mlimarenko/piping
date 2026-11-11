import { Link, useLocation, useNavigate, useParams } from "react-router";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { ArrowLeft, Copy, Pencil, Radio, Trash2 } from "lucide-react";
import { useApiMutation } from "@/shared/api/hooks";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { notify } from "@/shared/lib/notifications";
import EntityDetail from "@/shared/ui-shared/EntityDetail";
import PageState from "@/shared/ui-shared/PageState";
import PageChrome from "@/shared/ui-shared/PageChrome";
import { useUnsavedChangesGuard } from "@/shared/ui-shared/useUnsavedChangesGuard";
import { Button } from "@/shared/ui-primitives/button";
import { deleteProfileAssignment, listAllProfileAssignments, type ProfileAssignment } from "./api";
import { PROFILE_ASSIGNMENTS_KEY, invalidateMonitoringProfileCaches } from "./keys";
import { useScopeEntities } from "./useScopeEntities";
import { MONITORING_SCOPE_I18N_KEY, type MonitoringScopeType } from "./scope";
import { PROFILE_ASSIGNMENTS_BASE, profileAssignmentsHref, profileListContext, observabilityEditHref } from "./observabilityRoutes";

export default function ProfileAssignmentDetail() {
  const { id } = useParams();
  const { search } = useLocation();
  const { t } = useTranslation();
  const { can } = usePermissions();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const listHref = profileAssignmentsHref(search);
  const query = useQuery({ queryKey: PROFILE_ASSIGNMENTS_KEY, queryFn: () => listAllProfileAssignments() });
  const entities = useScopeEntities(t("observability.workflow.allTargets", { defaultValue: "All matching targets" }), { includeConnectionTargets: query.data?.find(row => row.id === id)?.scope_type === "connection_target" });
  const mutation = useApiMutation({
    mutationFn: (recordId: string) => { if (!can("DELETE", "MONITORING")) throw new Error("Permission removed"); return deleteProfileAssignment(recordId); },
    onSuccess: (_, recordId) => {
      qc.setQueryData<ProfileAssignment[]>(PROFILE_ASSIGNMENTS_KEY, current => current?.filter(row => row.id !== recordId));
      void invalidateMonitoringProfileCaches(qc);
      void navigate(listHref, { state: { focusExporterList: true } });
    },
  });
  useUnsavedChangesGuard(false, mutation.isPending);
  if (query.isPending || query.isError || !query.data.some(row => row.id === id)) return <>
    <PageChrome icon={Radio} title={t("observability.title")} actions={<Button asChild variant="ghost"><Link to={listHref}><ArrowLeft className="size-4" />{t("back")}</Link></Button>} />
    <PageState kind={query.isPending ? "loading" : query.isError ? "error" : "empty"} title={t(query.isPending ? "loading" : query.isError ? "observability.entityLoadFailed" : "observability.entityNotFound")} action={query.isError ? <Button variant="outline" onClick={() => { void query.refetch(); }}>{t("retry")}</Button> : undefined} />
  </>;
  const item = query.data.find(row => row.id === id)!;
  const copyValue = (value: string) => <div className="flex min-w-0 items-start gap-2"><code className="min-w-0 break-all">{value}</code><Button size="icon-sm" variant="ghost" aria-label={`${t("copy")}: ${value}`} onClick={() => { void navigator.clipboard.writeText(value).then(() => notify("success", t("copied")), () => notify("error", t("observability.workflow.copyFailed", { defaultValue: "Could not copy. Select and copy the value manually." }))); }}><Copy className="size-4" /></Button></div>;
  return <div aria-busy={mutation.isPending} className={mutation.isPending ? "pointer-events-none" : undefined}><EntityDetail icon={Radio} title={item.profile_name} description={item.profile_slug} backHref={listHref}
    error={mutation.isError ? t("observability.entityDeleteFailed") : undefined}
    fields={[
      { label: "ID", value: copyValue(item.id) },
      { label: t("observability.scope.typeLabel"), value: t(`observability.scope.${MONITORING_SCOPE_I18N_KEY[item.scope_type as MonitoringScopeType]}`, { defaultValue: item.scope_type }) },
      { label: t("observability.scope.targetLabel"), value: <div className="grid gap-2"><span className="break-words">{entities.resolveName(item.scope_type, item.scope_id)}</span>{item.scope_id && copyValue(item.scope_id)}</div> },
      { label: t("observability.sd.profile"), value: item.profile_name },
      { label: t("observability.workflow.profileSlug", { defaultValue: "Profile slug" }), value: item.profile_slug },
      { label: t("observability.workflow.profileId", { defaultValue: "Profile ID" }), value: item.profile_id ? copyValue(item.profile_id) : "—" },
      { label: t("observability.sd.enabled"), value: t(item.enabled ? "enabled" : "disabled") },
       { label: t("observability.sd.target"), value: item.target ?? t("observability.workflow.profileDefaults") },
       { label: t("observability.profiles.weight"), value: String(item.weight) },
    ]}
    actions={[
      { key: "edit", label: t("edit"), icon: Pencil, allowed: can("UPDATE", "MONITORING"), onClick: () => { if (!mutation.isPending) void navigate(observabilityEditHref(PROFILE_ASSIGNMENTS_BASE, item.id) + profileListContext(search)); }, disabled: mutation.isPending },
      { key: "delete", label: t("delete"), icon: Trash2, variant: "outline", allowed: can("DELETE", "MONITORING"), disabled: mutation.isPending, confirm: { title: t("delete"), description: t("observability.profiles.deleteAssignmentHint", { name: item.profile_name }), onConfirm: () => { if (can("DELETE", "MONITORING") && !mutation.isPending) mutation.mutate(item.id); } } },
    ]} /></div>;
}
