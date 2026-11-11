import { useEffect, useState } from "react";
import { useLocation, useNavigate, useParams, useSearchParams } from "react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { isUuid } from "@/shared/lib/ids";
import { usePermissions } from "@/shared/auth/permissionsContext";
import PageState from "@/shared/ui-shared/PageState";
import EntityEditorPage from "@/shared/ui-shared/EntityEditorPage";
import { Button } from "@/shared/ui-primitives/button";
import { AUTOMATION_SECRETS_QUERY_KEY, listAutomationSecrets } from "@/features/automation-secrets/api";
import { createJobTemplate, getJobTemplate, getProjectRevision, listExecutionEnvironments, listPlaybookTargetSets, listProjects, updateJobTemplate, type JobTemplate, type PlaybookTargetSetListItem, type Project, type PlaybookExecutionEnvironment } from "./api";
import { JobTemplateEditor } from "./JobTemplateEditor";
import { emptyJobTemplateForm, type JobTemplateForm } from "./playbookFormModels";
import { jobTemplateForm, jobTemplateRequest, jobTemplateReturnFlowDraft } from "./jobTemplateModel";
import { projectRevisionQueryKey } from "./projectFilesModel";
import { PLAYBOOK_TAB_ROUTES } from "./playbookNavigation";
import { targetSetEditorNewHref } from "./playbookEditorRoutes";
import { consumeTargetSetReturnFlow, readTargetSetReturnFlow, startTargetSetReturnFlow, TARGET_SET_RETURN_LABELS } from "./targetSetReturnFlow";
import { useTargetSetPreview } from "./useTargetSetPreview";

function JobForm({ initial, projectId, projects, targets, environments, back }: Readonly<{ initial?: JobTemplate; projectId: string; projects: Project[]; targets: PlaybookTargetSetListItem[]; environments: PlaybookExecutionEnvironment[]; back: () => void }>) {
  const { t } = useTranslation();
  const { can } = usePermissions();
  const client = useQueryClient();
  const location = useLocation();
  const navigate = useNavigate();
  // One-shot: the target-set editor returns here with a live flow reference.
  // Restore its draft, then consume it so no later history entry revives it.
  const [returnFlow] = useState(() => readTargetSetReturnFlow(location.state, jobTemplateReturnFlowDraft));
  const [form, setForm] = useState<JobTemplateForm>(() => returnFlow
    ? { ...returnFlow.draft.form, targetSetId: returnFlow.targetSetId || returnFlow.draft.form.targetSetId }
    : initial ? jobTemplateForm(initial) : { ...emptyJobTemplateForm(), projectId });
  const [baseline] = useState<JobTemplateForm>(() => returnFlow ? returnFlow.draft.baseline : form);
  const [error, setError] = useState("");
  useEffect(() => { consumeTargetSetReturnFlow(location.state); }, [location.state]);
  const selectedProject = projects.find((project) => project.id === form.projectId);
  const revisionId = form.revisionPolicy === "fixed" ? form.revision : selectedProject?.current_revision_id ?? "";
  const files = useQuery({
    queryKey: projectRevisionQueryKey(selectedProject?.id ?? "", revisionId),
    queryFn: () => getProjectRevision(selectedProject!.id, revisionId),
    enabled: Boolean(selectedProject && revisionId),
  });
  const canReadSecrets = can("READ", "SECRET_VARIABLE");
  const secrets = useQuery({ queryKey: AUTOMATION_SECRETS_QUERY_KEY, queryFn: listAutomationSecrets, enabled: canReadSecrets });
  const mutation = useMutation({ retry: false, mutationFn: async () => initial ? updateJobTemplate(initial.id, jobTemplateRequest(form)) : createJobTemplate(jobTemplateRequest(form)), onSuccess: () => { void client.invalidateQueries({ queryKey: ["playbooks", "job-templates"] }); back(); }, onError: () => setError(t("playbooks.jobDraft.saveFailed", { defaultValue: "Could not save the job. Check your input and permissions, then retry." })) });
  const allowed = can(initial ? "UPDATE" : "CREATE", "PLAYBOOK_JOB_TEMPLATE");
  const canReadTargetSet = can("READ", "PLAYBOOK_TARGET_SET");
  const targetSetPreview = useTargetSetPreview(form.targetSetId, canReadTargetSet);
  const returnHref = `${location.pathname}${location.search}`;
  const startReturnFlow = () => startTargetSetReturnFlow(
    returnHref,
    { form, baseline },
    { labelKey: TARGET_SET_RETURN_LABELS.job, state: location.state },
  );
  const createTargetSet = () => {
    if (!can("CREATE", "PLAYBOOK_TARGET_SET")) return;
    void navigate(targetSetEditorNewHref(), { state: startReturnFlow() });
  };
  const openTargetSetList = () => {
    if (!canReadTargetSet) return;
    void navigate(PLAYBOOK_TAB_ROUTES["target-sets"], { state: startReturnFlow() });
  };
  return <JobTemplateEditor
    form={form}
    projects={projects}
    targetSets={targets}
    executionEnvironments={environments}
    entrypointFiles={(files.data?.entries ?? []).filter((entry) => entry.content.kind === "file").map((entry) => entry.path)}
    projectEntries={files.data?.entries ?? []}
    entrypointFilesLoading={files.isLoading}
    entrypointFilesError={files.isError ? t("playbooks.jobDraft.filesLoadFailed", { defaultValue: "Project files could not be loaded. Retry or use advanced options." }) : undefined}
    projectRevisionMissing={Boolean(selectedProject && !revisionId)}
    secrets={secrets.data ?? []}
    secretsLoading={secrets.isLoading}
    secretsError={!canReadSecrets
      ? t("playbooks.jobDraft.noSecretReadAccess", { defaultValue: "You do not have permission to read secrets. Use advanced options to enter secret IDs." })
      : secrets.isError ? t("playbooks.jobDraft.secretsLoadFailed", { defaultValue: "Secrets could not be loaded. Retry or use advanced options." }) : undefined}
    onFormChange={setForm}
    onBack={back}
    submitting={mutation.isPending}
    editing={Boolean(initial)}
    dirty={JSON.stringify(form) !== JSON.stringify(baseline)}
    referenceError={error || (!allowed ? t("playbooks.permissionDenied", { defaultValue: "You do not have permission to save this job." }) : undefined)}
    onSave={allowed ? () => { setError(""); mutation.mutate(); } : undefined}
    targetSetPreview={targetSetPreview}
    onCreateTargetSet={can("CREATE", "PLAYBOOK_TARGET_SET") ? createTargetSet : undefined}
    onBrowseTargetSets={canReadTargetSet ? openTargetSetList : undefined}
  />;
}

export default function JobTemplateEditorPage() {
  const navigate = useNavigate();
  const { jobId } = useParams();
  const [params] = useSearchParams();
  const { t } = useTranslation();
  const back = () => { void navigate(PLAYBOOK_TAB_ROUTES["job-templates"]); };
  const projects = useQuery({ queryKey: ["playbooks", "projects"], queryFn: () => listProjects({ limit: 100, offset: 0 }) });
  const targets = useQuery({ queryKey: ["playbooks", "target-sets", "all"], queryFn: () => listPlaybookTargetSets({ limit: 100, offset: 0 }) });
  const environments = useQuery({ queryKey: ["playbooks", "execution-environments"], queryFn: () => listExecutionEnvironments({ limit: 100, offset: 0 }) });
  const job = useQuery({ queryKey: ["playbooks", "job-templates", jobId], queryFn: () => getJobTemplate(jobId ?? ""), enabled: Boolean(jobId) });
  const queries = [projects, targets, environments, ...(jobId ? [job] : [])];
  const failure = queries.find((query) => query.isError && !query.data);
  if (failure || queries.some((query) => query.isLoading)) return <EntityEditorPage scope="routed" title={t("playbooks.page.dialogs.jobTemplate.title")} onBack={back} backLabel={t("back")}><PageState kind={failure ? "error" : "loading"} description={failure?.error instanceof Error ? failure.error.message : undefined} action={failure ? <Button onClick={() => { void failure.refetch(); }}>{t("retry")}</Button> : undefined} /></EntityEditorPage>;
  const projectId = params.get("project_id");
  return <JobForm key={jobId ?? "new"} initial={job.data} projectId={isUuid(projectId) ? projectId : ""} projects={projects.data?.items ?? []} targets={targets.data?.items ?? []} environments={environments.data?.items ?? []} back={back} />;
}
