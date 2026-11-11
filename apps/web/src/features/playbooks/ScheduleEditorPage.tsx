import { useEffect, useRef, useState } from "react";
import { useLocation, useNavigate, useParams, useSearchParams } from "react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { isUuid } from "@/shared/lib/ids";
import { usePermissions } from "@/shared/auth/permissionsContext";
import PageState from "@/shared/ui-shared/PageState";
import { Button } from "@/shared/ui-primitives/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/shared/ui-primitives/dialog";
import EntityEditorPage from "@/shared/ui-shared/EntityEditorPage";
import { createPlaybookSchedule, getSchedule, listJobTemplates, listPlaybookTargetSets, previewPlaybookTargetSet, updatePlaybookSchedule, type JobTemplate, type PlaybookSchedule, type PlaybookTargetSetListItem, type ResolvedTarget } from "./api";
import { ScheduleEditor } from "./ScheduleEditor";
import { emptyScheduleForm, type ScheduleForm } from "./playbookFormModels";
import { fixedSnapshotTargets, fixedTargetsFromPreview, scheduleForm, scheduleRequest, scheduleReturnFlowDraft, snapshotDiff, updateScheduleRequest, type ScheduleSnapshotDiff } from "./scheduleModel";
import { playbookReturnHref } from "./playbookDetailRoutes";
import { PLAYBOOK_TAB_ROUTES } from "./playbookNavigation";
import { targetSetEditorNewHref } from "./playbookEditorRoutes";
import { consumeTargetSetReturnFlow, readTargetSetReturnFlow, startTargetSetReturnFlow, TARGET_SET_RETURN_LABELS } from "./targetSetReturnFlow";
import type { TargetSetPreviewState } from "./TargetSetPicker";
import { useJobVarsSchema } from "./useJobVarsSchema";
import { launchResourceFromQuery } from "./launchReadiness";

function SnapshotChangeList({ title, items }: Readonly<{ title: string; items: ResolvedTarget[] }>) {
  if (!items.length) return null;
  return <div className="min-w-0">
    <p className="text-sm font-medium text-foreground">{title}</p>
    <ul className="mt-1 list-disc space-y-0.5 pl-5 text-sm text-muted-foreground">
      {items.map((item) => <li key={item.target_id ?? item.hostname} className="break-words">{item.hostname}</li>)}
    </ul>
  </div>;
}

function ScheduleFormPage({ initial, jobId, jobs, targets, back, referenceError }: Readonly<{ initial?: PlaybookSchedule; jobId: string; jobs: JobTemplate[]; targets: PlaybookTargetSetListItem[]; back: () => void; referenceError?: string }>) {
  const { t } = useTranslation();
  const { can } = usePermissions();
  const client = useQueryClient();
  const location = useLocation();
  const navigate = useNavigate();
  // One-shot: the target-set editor returns here with a live flow reference.
  // Restore its draft, then consume it so no later history entry revives it.
  const [returnFlow] = useState(() => readTargetSetReturnFlow(location.state, scheduleReturnFlowDraft));
  const [form, setForm] = useState<ScheduleForm>(() => returnFlow
    ? { ...returnFlow.draft.form, targetSetId: returnFlow.targetSetId || returnFlow.draft.form.targetSetId }
    : initial ? scheduleForm(initial) : { ...emptyScheduleForm(), jobTemplateId: jobId });
  const [baseline] = useState<ScheduleForm>(() => returnFlow ? returnFlow.draft.baseline : form);
  const varsSchema = useJobVarsSchema(jobs.find((job) => job.id === form.jobTemplateId));
  const [error, setError] = useState("");
  const [resolving, setResolving] = useState(false);
  const [snapshot, setSnapshot] = useState<{ target: PlaybookTargetSetListItem; diff: ScheduleSnapshotDiff; targets: ResolvedTarget[] } | null>(null);
  const [snapshotPending, setSnapshotPending] = useState(false);
  const selection = useRef(0);
  const mutation = useMutation({ retry: false, mutationFn: async () => initial ? updatePlaybookSchedule(initial.id, updateScheduleRequest(form)) : createPlaybookSchedule(scheduleRequest(form)), onSuccess: () => { void client.invalidateQueries({ queryKey: ["playbooks", "schedules"] }); back(); }, onError: () => setError(t("playbooks.scheduleDraft.saveFailed", { defaultValue: "Could not save the schedule. Check your input and permissions, then retry." })) });
  const allowed = can(initial ? "UPDATE" : "CREATE", "PLAYBOOK_SCHEDULE") && can("EXECUTE", "PLAYBOOK_JOB_TEMPLATE");
  const canReadTargetSet = can("READ", "PLAYBOOK_TARGET_SET");
  // The list already carries the resolved count from the same server path as the
  // preview; the schedule must not fetch the composition twice.
  const selectedTargetSet = targets.find((item) => item.id === form.targetSetId);
  const targetSetPreview: TargetSetPreviewState = !form.targetSetId
    ? { kind: "idle" }
    : !canReadTargetSet ? { kind: "denied" }
    : selectedTargetSet?.resolved_count == null ? { kind: "unknown" }
    : { kind: "ready", total: selectedTargetSet.resolved_count };
  const returnHref = `${location.pathname}${location.search}`;
  const startReturnFlow = () => startTargetSetReturnFlow(
    returnHref,
    { form, baseline },
    { labelKey: TARGET_SET_RETURN_LABELS.schedule, state: location.state },
  );
  const createTargetSet = () => {
    if (!can("CREATE", "PLAYBOOK_TARGET_SET")) return;
    void navigate(targetSetEditorNewHref(), { state: startReturnFlow() });
  };
  const openTargetSetList = () => {
    if (!canReadTargetSet) return;
    void navigate(PLAYBOOK_TAB_ROUTES["target-sets"], { state: startReturnFlow() });
  };
  useEffect(() => { consumeTargetSetReturnFlow(location.state); }, [location.state]);
  async function selectTargetSet(id: string) {
    const target = targets.find((item) => item.id === id);
    if (!target) return;
    const request = ++selection.current;
    setError("");
    if (target.mode === "dynamic") {
      setForm((prev) => ({ ...prev, targetSetId: id, targetsJson: JSON.stringify(target.selector, null, 2) }));
      return;
    }
    setResolving(true);
    try {
      const preview = await previewPlaybookTargetSet(id, 1000);
      const resolved = fixedTargetsFromPreview(preview);
      if (request === selection.current) setForm((prev) => ({ ...prev, targetSetId: id, targetsJson: JSON.stringify(resolved, null, 2) }));
    } catch { if (request === selection.current) setError(t("playbooks.scheduleDraft.previewFailed", { defaultValue: "Could not resolve a complete target set. Fixed targets were not changed." })); }
    finally { if (request === selection.current) setResolving(false); }
  }
  async function refreshSnapshot() {
    const target = targets.find((item) => item.id === form.targetSetId);
    if (!target) return;
    setError("");
    setSnapshotPending(true);
    try {
      const preview = await previewPlaybookTargetSet(target.id, 1000);
      if (!preview.items.length) { setError(t("playbooks.scheduleDraft.snapshotEmpty", { defaultValue: "The target set has no hosts. The snapshot was not changed." })); return; }
      const resolved = fixedTargetsFromPreview(preview);
      setSnapshot({ target, diff: snapshotDiff(fixedSnapshotTargets(form) ?? [], resolved), targets: resolved });
    } catch { setError(t("playbooks.scheduleDraft.snapshotFailed", { defaultValue: "Could not resolve the current target set composition. The snapshot was not changed." })); }
    finally { setSnapshotPending(false); }
  }
  function applySnapshot() {
    if (!snapshot) return;
    const targetsJson = JSON.stringify(snapshot.targets, null, 2);
    setForm((prev) => ({ ...prev, targetsJson }));
    setSnapshot(null);
  }
  return <>
    <ScheduleEditor varsSchema={launchResourceFromQuery(varsSchema)} form={form} jobTemplates={jobs} targetSets={targets} onFormChange={setForm} onBack={back} submitting={mutation.isPending || resolving} editing={Boolean(initial)} dirty={JSON.stringify(form) !== JSON.stringify(baseline)} referenceError={error || referenceError || (!allowed ? t("playbooks.permissionDenied", { defaultValue: "You do not have permission to save this schedule." }) : undefined)} onSave={allowed ? () => { setError(""); mutation.mutate(); } : undefined} onSelectTargetSet={(id) => { void selectTargetSet(id); }} onRefreshSnapshot={form.selectionMode === "static" ? () => { void refreshSnapshot(); } : undefined} snapshotPending={snapshotPending} targetSetPreview={targetSetPreview} onCreateTargetSet={can("CREATE", "PLAYBOOK_TARGET_SET") ? createTargetSet : undefined} onBrowseTargetSets={canReadTargetSet ? openTargetSetList : undefined} />
    <Dialog open={Boolean(snapshot)} onOpenChange={(open) => { if (!open && !snapshotPending) setSnapshot(null); }}>
      <DialogContent className="max-h-[90dvh] overflow-y-auto sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>{t("playbooks.scheduleDraft.snapshotDialogTitle", { defaultValue: "Update snapshot from set" })}</DialogTitle>
          <DialogDescription>{snapshot ? t("playbooks.scheduleDraft.snapshotDialogDescription", { name: snapshot.target.name, defaultValue: "The current composition of “{{name}}” will replace the saved snapshot." }) : ""}</DialogDescription>
        </DialogHeader>
        {snapshot && <div className="min-w-0 space-y-3">
          <p className="text-sm text-muted-foreground">{t("playbooks.scheduleDraft.snapshotResult", { count: snapshot.targets.length, defaultValue: "New composition: {{count}} hosts." })}</p>
          {!snapshot.diff.added.length && !snapshot.diff.removed.length && <p className="text-sm text-muted-foreground">{t("playbooks.scheduleDraft.snapshotNoChanges", { defaultValue: "The composition is unchanged." })}</p>}
          <SnapshotChangeList title={t("playbooks.scheduleDraft.snapshotAdded", { defaultValue: "Added" })} items={snapshot.diff.added} />
          <SnapshotChangeList title={t("playbooks.scheduleDraft.snapshotRemoved", { defaultValue: "Removed" })} items={snapshot.diff.removed} />
        </div>}
        <DialogFooter>
          <Button variant="outline" onClick={() => setSnapshot(null)}>{t("cancel")}</Button>
          <Button disabled={!snapshot || snapshotPending} onClick={applySnapshot}>{t("playbooks.scheduleDraft.snapshotApply", { defaultValue: "Update snapshot" })}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  </>;
}

export default function ScheduleEditorPage() {
  const { id } = useParams();
  const navigate = useNavigate();
  const [params] = useSearchParams();
  const { t } = useTranslation();
  const back = () => { void navigate(params.has("returnTo") ? playbookReturnHref(params.get("returnTo")) : PLAYBOOK_TAB_ROUTES.schedules); };
  const jobs = useQuery({ queryKey: ["playbooks", "job-templates"], queryFn: () => listJobTemplates({ limit: 100, offset: 0 }) });
  const targets = useQuery({ queryKey: ["playbooks", "target-sets", "all"], queryFn: () => listPlaybookTargetSets({ limit: 100, offset: 0 }) });
  const schedule = useQuery({ queryKey: ["playbooks", "schedules", id], queryFn: () => getSchedule(id ?? ""), enabled: Boolean(id) });
  const queries = [jobs, targets, ...(id ? [schedule] : [])];
  const failure = queries.find((query) => query.isError && !query.data);
  if (failure || queries.some((query) => query.isLoading)) return <EntityEditorPage scope="routed" title={t("playbooks.page.dialogs.schedule.title")} onBack={back} backLabel={t("back")}><PageState kind={failure ? "error" : "loading"} description={failure?.error instanceof Error ? failure.error.message : undefined} action={failure ? <Button onClick={() => { void failure.refetch(); }}>{t("retry")}</Button> : undefined} /></EntityEditorPage>;
  const initialJobId = params.get("job_template_id");
  const refreshFailure = queries.find((query) => query.isError);
  return <ScheduleFormPage key={id ?? "new"} initial={schedule.data} jobId={isUuid(initialJobId) ? initialJobId : ""} jobs={jobs.data?.items ?? []} targets={targets.data?.items ?? []} back={back} referenceError={refreshFailure?.error instanceof Error ? refreshFailure.error.message : undefined} />;
}
