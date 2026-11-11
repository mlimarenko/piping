import { useMemo, useState } from "react";
import { useLocation, useNavigate } from "react-router";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { useApiMutation } from "@/shared/api/hooks";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { useServerTable, type ServerTableListParams } from "@/shared/ui-shared/useServerTable";
import { Button } from "@/shared/ui-primitives/button";
import { AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from "@/shared/ui-primitives/alert-dialog";
import PageState from "@/shared/ui-shared/PageState";
import { createPlaybookAssignment, updatePlaybookAssignment, deletePlaybookAssignment, listPlaybookAssignments, listJobTemplates, listPlaybookTargetSets, previewPlaybookTargetSet, type PlaybookAssignment } from "./api";
import { assignmentReturnFlowDraft, assignmentForm, assignmentRequest } from "./assignmentFormModel";
import { consumeTargetSetReturnFlow, isTargetSetReturnFlowReference, readTargetSetReturnFlow, startTargetSetReturnFlow } from "./targetSetReturnFlow";
import { targetSetEditorNewHref } from "./playbookEditorRoutes";
import { AssignmentDialog } from "./AssignmentDialog";
import { AssignmentsPanel } from "./AssignmentsPanel";
import { TargetPreview } from "./PlaybookPanelUi";
import { PLAYBOOK_ASSIGNMENTS_TABLE_CONFIG } from "./tableConfig";
import { usePlaybookScopeEntities } from "./usePlaybookScopeEntities";
import { useTargetSetPreview } from "./useTargetSetPreview";

type Params = Parameters<typeof listPlaybookAssignments>[0] & ServerTableListParams;

export function AssignmentsSettings() {
  const { t } = useTranslation();
  const { can } = usePermissions();
  const client = useQueryClient();
  const location = useLocation();
  const navigate = useNavigate();
  const [editingId, setEditingId] = useState<string | null>(null);
  const [open, setOpen] = useState(false);
  const [form, setForm] = useState(assignmentForm);
  const [error, setError] = useState<string>();
  const [deleting, setDeleting] = useState<PlaybookAssignment | null>(null);
  const [previewId, setPreviewId] = useState<string | null>(null);

  // One-shot: the target-set editor returns here with a live flow reference.
  // Reopen the dialog with the saved form and the set the user just created,
  // then consume the reference so no later history entry restores it again.
  const [restoredFlow, setRestoredFlow] = useState<string | null>(null);
  if (restoredFlow !== location.key && isTargetSetReturnFlowReference(location.state)) {
    setRestoredFlow(location.key);
    const flow = readTargetSetReturnFlow(location.state, assignmentReturnFlowDraft);
    if (flow) {
      consumeTargetSetReturnFlow(location.state);
      setEditingId(flow.draft.editingId);
      setForm({ ...flow.draft.form, targetSetId: flow.targetSetId || flow.draft.form.targetSetId });
      setError(undefined);
      setOpen(true);
    }
  }
  const assignments = useServerTable<PlaybookAssignment, Params>({ listFn: listPlaybookAssignments, queryKey: ["playbooks", "assignments"], ...PLAYBOOK_ASSIGNMENTS_TABLE_CONFIG, pageSizeDefault: 100 });
  const jobs = useQuery({ queryKey: ["playbooks", "job-templates"], queryFn: () => listJobTemplates({ limit: 100, offset: 0 }), enabled: can("READ", "PLAYBOOK_JOB_TEMPLATE") });
  const targets = useQuery({ queryKey: ["playbooks", "target-sets", "assignment"], queryFn: () => listPlaybookTargetSets({ limit: 100, offset: 0 }), enabled: can("READ", "PLAYBOOK_TARGET_SET") });
  const targetSetPreview = useTargetSetPreview(open ? form.targetSetId : "", can("READ", "PLAYBOOK_TARGET_SET"));
  const preview = useQuery({ queryKey: ["playbooks", "target-set-preview", previewId], queryFn: () => previewPlaybookTargetSet(previewId ?? "", 50), enabled: Boolean(previewId) && can("READ", "PLAYBOOK_TARGET_SET") });
  const scopes = usePlaybookScopeEntities(t("playbooks.page.scope.global"), open || (assignments.query.data?.items.length ?? 0) > 0);
  const jobsById = useMemo(() => new Map((jobs.data?.items ?? []).map(item => [item.id, item])), [jobs.data]);
  const targetsById = useMemo(() => new Map((targets.data?.items ?? []).map(item => [item.id, item])), [targets.data]);
  const refresh = () => { void client.invalidateQueries({ queryKey: ["playbooks", "assignments"] }); };
  const save = useApiMutation({
    mutationFn: (body: ReturnType<typeof assignmentRequest>) => editingId ? updatePlaybookAssignment(editingId, body) : createPlaybookAssignment(body),
    onSuccess: (saved, body) => {
      refresh();
      if (saved.job_template_id !== body.job_template_id || saved.target_set_id !== body.target_set_id || (saved.name || null) !== (body.name || null) || saved.scope_kind !== body.scope_kind || (saved.scope_id || null) !== (body.scope_id || null) || saved.priority !== body.priority || saved.enabled !== body.enabled || saved.visible !== body.visible || saved.executable !== body.executable || saved.quick_action_enabled !== body.quick_action_enabled) {
        setError(t("playbooks.assignment.notSaved", { defaultValue: "The server did not apply all changes. Review the saved assignment before retrying." }));
        return;
      }
      setOpen(false); setError(undefined);
    },
    onError: () => setError(t("unknownError")),
  });
  const remove = useApiMutation({ mutationFn: (id: string) => deletePlaybookAssignment(id), onSuccess: () => { setDeleting(null); setError(undefined); refresh(); }, onError: () => setError(t("unknownError")) });
  const start = (item?: PlaybookAssignment) => { setEditingId(item?.id ?? null); setForm(assignmentForm(item)); setError(undefined); setOpen(true); };

  const createTargetSet = () => {
    if (!can("CREATE", "PLAYBOOK_TARGET_SET")) return;
    const state = startTargetSetReturnFlow(`${location.pathname}${location.search}`, { editingId, form }, { state: location.state });
    void navigate(targetSetEditorNewHref(), { state });
  };
  const referencesUnavailable = !can("READ", "PLAYBOOK_JOB_TEMPLATE") || !can("READ", "PLAYBOOK_TARGET_SET") || jobs.isError || targets.isError;
  const submit = () => {
    if (referencesUnavailable) { setError(t("playbooks.assignment.referencesUnavailable", { defaultValue: "Job templates and target sets must be readable to edit assignments." })); return; }
    try { setError(undefined); save.mutate(assignmentRequest(form)); } catch { setError(t("playbooks.assignment.invalid", { defaultValue: "Select a job, target set and scope, and enter a valid priority." })); }
  };
  return <div className="flex min-h-0 flex-1 flex-col gap-4">
    {can("CREATE", "PLAYBOOK_ASSIGNMENT") && <div><Button onClick={() => start()} disabled={referencesUnavailable || jobs.isPending || targets.isPending}>{t("create")}</Button></div>}
    {referencesUnavailable && <p role="alert" className="text-sm text-destructive">{t("playbooks.assignment.referencesUnavailable", { defaultValue: "Job templates and target sets must be readable to edit assignments." })}</p>}
    <p className="text-sm text-muted-foreground">{t("playbooks.assignment.hostLaunch", { defaultValue: "Launch assigned jobs for an individual host from Connections. The job's saved managed inventory must contain that host." })}</p>
    {!open && !deleting && error && <p role="alert" className="text-sm text-destructive">{error}</p>}
    <AssignmentsPanel assignments={assignments.query.data?.items ?? []} loading={assignments.query.isPending} error={assignments.query.isError ? assignments.query.error : undefined} onRetry={assignments.refresh} sortState={assignments.sortState} onSortChange={assignments.setSortState} columnFilters={assignments.columnFilters} onColumnFilterChange={assignments.setColumnFilter} pagination={{ page: assignments.page, pageSize: assignments.pageSize, total: assignments.query.data?.total ?? 0, onPageChange: assignments.setPage, onPageSizeChange: assignments.setPageSize }} playbookById={jobsById} targetSetById={targetsById} scopeEntities={scopes} onEdit={item => start(item)} onDelete={setDeleting} onPreview={item => setPreviewId(item.target_set_id)} runPending={false} emptyState={<PageState kind="empty" title={t("playbooks.page.empty.assignments")} description={t("playbooks.page.emptyHints.assignments")} action={can("CREATE", "PLAYBOOK_ASSIGNMENT") ? <Button onClick={() => start()} disabled={referencesUnavailable || jobs.isPending || targets.isPending}>{t("create")}</Button> : undefined} />} />
    {previewId && !open && (preview.isPending ? <PageState kind="loading" title={t("loading")} /> : preview.data ? <TargetPreview preview={preview.data} /> : <PageState kind="error" title={t("playbooks.page.preview.failed")} />)}
    <AssignmentDialog open={open} editing={editingId !== null} form={form} onFormChange={setForm} playbooks={jobs.data?.items ?? []} allTargetSets={targets.data?.items ?? []} targetSetMissing={referencesUnavailable} targetSetPreview={targetSetPreview} onCreateTargetSet={can("CREATE", "PLAYBOOK_TARGET_SET") ? createTargetSet : undefined} scopeEntities={scopes} onOpenChange={setOpen} onSubmit={submit} submitting={save.isPending} error={error} />
    <AlertDialog open={Boolean(deleting)} onOpenChange={value => { if (!value && !remove.isPending) setDeleting(null); }}><AlertDialogContent><AlertDialogHeader><AlertDialogTitle>{t("delete")}</AlertDialogTitle><AlertDialogDescription>{t("playbooks.assignment.deleteConfirm", { defaultValue: "Delete assignment {{name}}?", name: deleting?.name || deleting?.id })}</AlertDialogDescription></AlertDialogHeader>{error && <p role="alert" className="text-sm text-destructive">{error}</p>}<AlertDialogFooter><AlertDialogCancel disabled={remove.isPending}>{t("cancel")}</AlertDialogCancel><AlertDialogAction disabled={remove.isPending} onClick={event => { event.preventDefault(); if (deleting) remove.mutate(deleting.id); }}>{t("delete")}</AlertDialogAction></AlertDialogFooter></AlertDialogContent></AlertDialog>
  </div>;
}
