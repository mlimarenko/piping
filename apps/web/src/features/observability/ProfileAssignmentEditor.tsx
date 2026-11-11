import { useState, type ReactNode } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useLocation, useNavigate, useParams } from "react-router";
import { useTranslation } from "react-i18next";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { useApiMutation } from "@/shared/api/hooks";
import { Input } from "@/shared/ui-primitives/input";
import { Button } from "@/shared/ui-primitives/button";
import { Switch } from "@/shared/ui-primitives/switch";
import FormSelect from "@/shared/ui-shared/FormSelect";
import FormField, { FormFieldControl } from "@/shared/ui-shared/FormField";
import EntityEditorPage from "@/shared/ui-shared/EntityEditorPage";
import PageState from "@/shared/ui-shared/PageState";
import { createProfileAssignment, updateProfileAssignment, listAllProfileAssignments, listAllProfiles, type ProfileAssignment, type MonitoringProfile, type ProfileAssignmentRequest } from "./api";
import { PROFILE_ASSIGNMENTS_KEY, PROFILES_KEY, invalidateMonitoringProfileCaches } from "./keys";
import { PROFILE_ASSIGNMENTS_BASE, profileAssignmentsHref, profileListContext, observabilityRecordHref } from "./observabilityRoutes";
import { useScopeEntities, type ScopeEntities } from "./useScopeEntities";
import { type MonitoringScopeType } from "./scope";
import ScopeFields from "./ScopeFields";
import ProfilePreviewPanel from "./ProfilePreviewPanel";

export function ProfileAssignmentForm({ editing, profiles, entities, refreshError }: { editing: ProfileAssignment | null; profiles: MonitoringProfile[]; entities: ScopeEntities; refreshError?: ReactNode }) {
  const { t } = useTranslation();
  const { can } = usePermissions();
  const qc = useQueryClient();
  const navigate = useNavigate();
  const { search } = useLocation();
  const allowed = can(editing ? "UPDATE" : "CREATE", "MONITORING");
  const [baseline] = useState(() => ({ profile_id: editing?.profile_id ?? new URLSearchParams(search).get("profile_id") ?? "", scope_type: editing?.scope_type ?? "global", scope_id: editing?.scope_id ?? "", target: editing?.target ?? "", weight: String(editing?.weight ?? 0), enabled: editing?.enabled ?? true }));
  const [draft, setDraft] = useState(baseline);
  const [previewAccepted, setPreviewAccepted] = useState(false);
  const mutation = useApiMutation({
    mutationFn: (body: ProfileAssignmentRequest) => {
      if (!can(editing ? "UPDATE" : "CREATE", "MONITORING")) throw new Error("Permission removed");
      return editing ? updateProfileAssignment(editing.id, body) : createProfileAssignment(body);
    },
    onSuccess: saved => {
      qc.setQueryData<ProfileAssignment[]>(PROFILE_ASSIGNMENTS_KEY, current => current ? [...current.filter(assignment => assignment.id !== saved.id), saved] : [saved]);
      void invalidateMonitoringProfileCaches(qc);
      void navigate(observabilityRecordHref(PROFILE_ASSIGNMENTS_BASE, saved.id) + profileListContext(search));
    },
  });
  const set = <K extends keyof typeof draft>(key: K, value: typeof draft[K]) => {
    setDraft(current => ({ ...current, [key]: value }));
    setPreviewAccepted(false);
  };
  const back = () => { if (!mutation.isPending) void navigate(profileAssignmentsHref(search)); };
  const valid = !!draft.profile_id && (draft.scope_type === "global" || !!draft.scope_id) && /^-?\d+$/.test(draft.weight) && Number.isInteger(Number(draft.weight)) && Number(draft.weight) >= -2147483648 && Number(draft.weight) <= 2147483647;
  const submit = () => {
    if (!allowed || !valid || !previewAccepted || mutation.isPending) return;
    mutation.mutate({ ...draft, scope_id: draft.scope_type === "global" ? null : draft.scope_id, target: draft.target.trim() || null, weight: Number(draft.weight) });
  };
  const options = profiles.map(profile => ({ value: profile.id, label: profile.name }));
  if (editing && !options.some(option => option.value === editing.profile_id)) options.push({ value: editing.profile_id, label: editing.profile_name });
  return <EntityEditorPage scope="routed" title={t(editing ? "observability.profiles.editAssignment" : "observability.profiles.newAssignment")} description={t("observability.profiles.assignmentHint")}
    onBack={back} backLabel={t("back")} onCancel={back} cancelLabel={t("cancel")} onSave={submit} saveLabel={t(editing ? "save" : "create")} saving={mutation.isPending} saveDisabled={!allowed || !valid || !previewAccepted || mutation.isPending}
    dirty={JSON.stringify(draft) !== JSON.stringify(baseline)} dirtyMessage={t("editor.unsavedChanges")} error={mutation.isError || refreshError ? <>{mutation.isError && t("observability.workflow.saveFailed")}{refreshError}</> : undefined}>
    <fieldset disabled={mutation.isPending || !allowed} className="grid min-w-0 gap-4">
      <FormSelect label={t("observability.profiles.profile")} value={draft.profile_id} onValueChange={value => set("profile_id", value)} options={options} required />
      <ScopeFields scopeType={draft.scope_type as MonitoringScopeType} scopeId={draft.scope_id} onScopeTypeChange={value => { set("scope_type", value); set("scope_id", ""); }} onScopeIdChange={value => set("scope_id", value)} entities={entities} />
      <FormField htmlFor="assignment-target" label={t("observability.sd.target")}><FormFieldControl><Input id="assignment-target" value={draft.target} onChange={event => set("target", event.target.value)} /></FormFieldControl></FormField>
      <FormField htmlFor="assignment-weight" label={t("observability.profiles.weight")}><FormFieldControl><Input id="assignment-weight" type="number" step="1" min={-2147483648} max={2147483647} value={draft.weight} onChange={event => set("weight", event.target.value)} /></FormFieldControl></FormField>
      <FormField htmlFor="assignment-enabled" label={t("enabled")}><FormFieldControl><Switch id="assignment-enabled" checked={draft.enabled} onCheckedChange={value => set("enabled", value)} /></FormFieldControl></FormField>
    </fieldset>
    {valid && <ProfilePreviewPanel profileId={draft.profile_id} scopeType={draft.scope_type} scopeId={draft.scope_id} target={draft.target} accepted={previewAccepted} onAcceptedChange={setPreviewAccepted} />}
  </EntityEditorPage>;
}

export default function ProfileAssignmentEditor() {
  const { id } = useParams();
  const { t } = useTranslation();
  const navigate = useNavigate();
  const { search } = useLocation();
  const profiles = useQuery({ queryKey: PROFILES_KEY, queryFn: () => listAllProfiles() });
  const assignments = useQuery({ queryKey: PROFILE_ASSIGNMENTS_KEY, queryFn: () => listAllProfileAssignments(), enabled: !!id });
  const entities = useScopeEntities(t("observability.scope.global"), { includeConnectionTargets: true });
  const retry = () => { void profiles.refetch(); if (id) void assignments.refetch(); };
  const statePage = (content: ReactNode) => <EntityEditorPage scope="routed" title={t(id ? "observability.profiles.editAssignment" : "observability.profiles.newAssignment")} onBack={() => void navigate(profileAssignmentsHref(search))} backLabel={t("back")} cancelLabel={t("cancel")}>{content}</EntityEditorPage>;
  if (profiles.isPending || (id && assignments.isPending)) return statePage(<PageState kind="loading" title={t("loading")} />);
  if ((profiles.isError && !profiles.data) || (id && assignments.isError && !assignments.data)) return statePage(<PageState kind="error" title={t("observability.entityLoadFailed")} action={<Button onClick={retry}>{t("retry")}</Button>} />);
  const editing = id ? assignments.data?.find(assignment => assignment.id === id) : null;
  if (id && !editing) return statePage(<PageState kind="empty" title={t("observability.entityNotFound")} />);
  return <ProfileAssignmentForm key={id ?? "new"} editing={editing ?? null} profiles={profiles.data ?? []} entities={entities} refreshError={profiles.isError || assignments.isError ? <div role="alert">{t("observability.entityLoadFailed")}<Button variant="outline" onClick={retry}>{t("retry")}</Button></div> : undefined} />;
}
