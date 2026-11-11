import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useLocation, useNavigate, useSearchParams, Link } from "react-router";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowLeft, ClipboardList, Plus, RefreshCw } from "lucide-react";
import PageChrome, { PageActions } from "@/shared/ui-shared/PageChrome";
import SectionLinks from "@/shared/ui-shared/SectionLinks";
import PageState from "@/shared/ui-shared/PageState";
import SearchInput from "@/shared/ui-shared/SearchInput";
import SurfaceCard, { SurfaceCardContent, SurfaceCardHeader } from "@/shared/ui-shared/SurfaceCard";
import { Button } from "@/shared/ui-primitives/button";
import { useServerTable, type ServerTableListParams } from "@/shared/ui-shared/useServerTable";
import { useApiMutation } from "@/shared/api/hooks";
import type { ApiError } from "@/shared/api/client";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { notify } from "@/shared/lib/notifications";
import {
  deletePlaybookSchedule, deletePlaybookTargetSet, launchJobTemplate,
  listExecutionEnvironments, listJobTemplates, listProjects, listPlaybookRunGroups,
  listPlaybookSchedules, listPlaybookTargetSets,
  previewPlaybookTargetSet, updatePlaybookSchedule,
  type PlaybookRunGroup, type PlaybookSchedule,
  type PlaybookTargetSetListItem,
} from "./api";
import { PLAYBOOK_RUN_GROUPS_TABLE_CONFIG, PLAYBOOK_SCHEDULES_TABLE_CONFIG, PLAYBOOK_TARGET_SETS_TABLE_CONFIG } from "./tableConfig";
import { PLAYBOOK_RUN_GROUP_NAME_PARAM, PLAYBOOK_RUN_GROUP_PARAM, PLAYBOOK_TAB_ROUTES, PLAYBOOK_TARGET_SET_PARAM, isPlaybookSettingsTab, playbookRunGroupHref, playbooksTabFromPath, type PlaybooksTab } from "./playbookNavigation";
import { readTargetSetReturnDestination } from "./targetSetReturnFlow";import { executionEnvironmentEditorNewHref, jobTemplateEditorNewHref, jobTemplateEditorNewForExecutionEnvironmentHref, projectEditorNewHref, scheduleEditorEditHref, scheduleEditorNewHref, targetSetEditorEditHref, targetSetEditorNewForDuplicateHref, targetSetEditorNewForTemplateHref, targetSetEditorNewHref } from "./playbookEditorRoutes";
import { ProjectsPanel } from "./ProjectsPanel";
import { JobTemplatesPanel } from "./JobTemplatesPanel";
import { SchedulesPanel } from "./SchedulesPanel";
import { RunsPanel } from "./RunsPanel";
import { ExecutionEnvironmentsPanel } from "./ExecutionEnvironmentsPanel";
import { TargetSetsPanel } from "./TargetSetsPanel";
import { TARGET_SET_TEMPLATES } from "./targetSetTemplates";
import { AssignmentsSettings } from "./AssignmentsSettings";
import { SettingsOverview } from "./SettingsOverview";
import { TargetPreview } from "./PlaybookPanelUi";
import { DeleteTargetSetDialog } from "./DeleteTargetSetDialog";
import { DeleteScheduleDialog } from "./DeleteScheduleDialog";
import PlaybookLaunchDialog, { type PlaybookLaunchIntent } from "./PlaybookLaunchDialog";
import { useJobVarsSchema } from "./useJobVarsSchema";
import { PLAYBOOK_TAB_DOCS } from "./playbookDocs";
import { automationDocUrl } from "@/shared/lib/automationDocs";
import { LAUNCH_TARGET_LIMIT, launchResourceFromQuery } from "./launchReadiness";
import { AUTOMATION_SECRETS_QUERY_KEY, listAutomationSecrets } from "@/features/automation-secrets/api";
import { RunDetailPanel } from "./RunDetailPanel";
import { isTerminalStatus } from "./runPhase";

type TargetParams = Parameters<typeof listPlaybookTargetSets>[0] & ServerTableListParams;
type ScheduleParams = Parameters<typeof listPlaybookSchedules>[0] & ServerTableListParams;
type RunParams = Parameters<typeof listPlaybookRunGroups>[0] & ServerTableListParams;

export default function PlaybooksPage({ initialTab = "projects" }: { initialTab?: PlaybooksTab } = {}) {
  const { t } = useTranslation();
  const { can } = usePermissions();
  const location = useLocation();
  const navigate = useNavigate();
  const [params, setParams] = useSearchParams();
  const client = useQueryClient();
  const tab = playbooksTabFromPath(location.pathname, initialTab);
  const targetSetReturn = tab === "target-sets" && can("READ", "PLAYBOOK_TARGET_SET") ? readTargetSetReturnDestination(location.state) : null;
  // Forward only a live flow: a consumed or foreign reference must not travel into create.
  const targetSetReturnState: unknown = targetSetReturn ? (location.state as unknown) : undefined;
  const [jobId, setJobId] = useState<string | null>(null);
  const linkedGroup = tab === "runs" ? params.get(PLAYBOOK_RUN_GROUP_PARAM) : null;
  const detail = linkedGroup ? { target: { kind: "group" as const, groupId: linkedGroup }, name: params.get(PLAYBOOK_RUN_GROUP_NAME_PARAM) || t("playbooks.nativeConsole.run") } : null;
  const clearDetail = () => {
    if (linkedGroup) setParams(previous => { const next = new URLSearchParams(previous); next.delete(PLAYBOOK_RUN_GROUP_PARAM); next.delete(PLAYBOOK_RUN_GROUP_NAME_PARAM); return next; }, { replace: true });
  };
  const openGroup = (groupId: string, name: string) => setParams(previous => {
    const next = new URLSearchParams(previous);
    next.set(PLAYBOOK_RUN_GROUP_PARAM, groupId);
    next.set(PLAYBOOK_RUN_GROUP_NAME_PARAM, name);
    return next;
  });
  const [targetId, setTargetId] = useState<string | null>(() => params.get(PLAYBOOK_TARGET_SET_PARAM));
  const [deleteTarget, setDeleteTarget] = useState<PlaybookTargetSetListItem | null>(null);
  const [deleteSchedule, setDeleteSchedule] = useState<PlaybookSchedule | null>(null);
  const settingsLinks = [
    { value: "execution-environments" as const, label: t("navItems.executionEnvironments"), allowed: can("READ", "PLAYBOOK_EXECUTION_ENVIRONMENT") },
    { value: "target-sets" as const, label: t("navItems.targetSets"), allowed: can("READ", "PLAYBOOK_TARGET_SET") },
    { value: "assignments" as const, label: t("playbooks.page.assignments.title", { defaultValue: "Assignments" }), allowed: can("READ", "PLAYBOOK_ASSIGNMENT") },
  ].filter(item => item.allowed);
  const primaryLinks = [
    { value: "projects" as PlaybooksTab, label: t("navItems.projects"), allowed: can("READ", "PLAYBOOK_PROJECT") },
    { value: "job-templates" as PlaybooksTab, label: t("navItems.jobTemplates"), allowed: can("READ", "PLAYBOOK_JOB_TEMPLATE") },
    { value: "runs" as PlaybooksTab, label: t("navItems.runs"), allowed: can("READ", "PLAYBOOK_RUN_GROUP") },
    { value: "schedules" as PlaybooksTab, label: t("navItems.schedules"), allowed: can("READ", "PLAYBOOK_SCHEDULE") },
    ...(settingsLinks.length > 0 ? [{ value: "settings" as PlaybooksTab, label: t("playbooks.page.settings", { defaultValue: "Settings" }), allowed: true }] : []),
  ].filter(item => item.allowed);
  const environments = useQuery({ queryKey: ["playbooks", "execution-environments"], queryFn: () => listExecutionEnvironments({ limit: 100, offset: 0 }), enabled: can("READ", "PLAYBOOK_EXECUTION_ENVIRONMENT") && (tab === "projects" || tab === "job-templates" || tab === "execution-environments") });
  const projects = useQuery({ queryKey: ["playbooks", "projects"], queryFn: () => listProjects({ limit: 100, offset: 0 }), enabled: can("READ", "PLAYBOOK_PROJECT") && (tab === "projects" || tab === "job-templates") });
  const jobs = useQuery({ queryKey: ["playbooks", "job-templates"], queryFn: () => listJobTemplates({ limit: 100, offset: 0 }), enabled: tab === "job-templates" && can("READ", "PLAYBOOK_JOB_TEMPLATE") });
  const targets = useServerTable<PlaybookTargetSetListItem, TargetParams>({ listFn: listPlaybookTargetSets, queryKey: ["playbooks", "target-sets"], ...PLAYBOOK_TARGET_SETS_TABLE_CONFIG, pageSizeDefault: 100, enabled: tab === "target-sets" });
  const schedules = useServerTable<PlaybookSchedule, ScheduleParams>({ listFn: listPlaybookSchedules, queryKey: ["playbooks", "schedules"], ...PLAYBOOK_SCHEDULES_TABLE_CONFIG, pageSizeDefault: 100, enabled: tab === "schedules" });
  const allTargets = useQuery({ queryKey: ["playbooks", "target-sets", "all"], queryFn: () => listPlaybookTargetSets({ limit: 100, offset: 0 }), enabled: (tab === "job-templates" || tab === "schedules") && can("READ", "PLAYBOOK_TARGET_SET") });
  const runs = useServerTable<PlaybookRunGroup, RunParams>({ listFn: listPlaybookRunGroups, queryKey: ["playbooks", "run-groups"], ...PLAYBOOK_RUN_GROUPS_TABLE_CONFIG, pageSizeDefault: 100, enabled: tab === "runs", refetchInterval: query => query.state.data?.items.some(group => !isTerminalStatus(group.status)) ? 5000 : false });
  const preview = useQuery({ queryKey: ["playbooks", "target-set-preview", targetId], queryFn: () => previewPlaybookTargetSet(targetId ?? "", 50), enabled: targetId !== null && can("READ", "PLAYBOOK_TARGET_SET") });
  // Launch composition: the same preview endpoint the target-set list uses, at
  // the agreed UI limit, so the dialog can validate every host it will queue.
  const launchJob = jobs.data?.items.find(item => item.id === jobId);
  const launchVarsSchema = useJobVarsSchema(launchJob, jobId !== null);
  const launchTargetSetId = launchJob?.inventory.kind === "managed" ? launchJob.inventory.target_set_id : null;
  const launchPreview = useQuery({ queryKey: ["playbooks", "launch-preview", launchTargetSetId], queryFn: () => previewPlaybookTargetSet(launchTargetSetId ?? "", LAUNCH_TARGET_LIMIT), enabled: launchTargetSetId !== null });
  const launchSecrets = useQuery({ queryKey: AUTOMATION_SECRETS_QUERY_KEY, queryFn: listAutomationSecrets, enabled: jobId !== null });
  const environmentMap = useMemo(() => new Map((environments.data?.items ?? []).map(item => [item.id, item])), [environments.data]);
  const projectMap = useMemo(() => new Map((projects.data?.items ?? []).map(item => [item.id, item])), [projects.data]);
  const targetMap = useMemo(() => new Map((allTargets.data?.items ?? []).map(item => [item.id, item])), [allTargets.data]);
  const refresh = () => { void client.invalidateQueries({ queryKey: ["playbooks"] }); };
  const fail = (error: ApiError) => notify("error", error.message || t("unknownError"));
  const launch = useApiMutation({ mutationFn: (intent: PlaybookLaunchIntent) => launchJobTemplate(intent.jobTemplateId, intent.extraVars ? { overrides: { extra_vars: intent.extraVars } } : {}), onSuccess: (created, intent) => { const name = jobs.data?.items.find(item => item.id === intent.jobTemplateId)?.name ?? t("unknown"); setJobId(null); refresh(); void navigate(playbookRunGroupHref(created.run_group.id, name)); }, onError: fail });
  const removeTarget = useApiMutation({ mutationFn: deletePlaybookTargetSet, onSuccess: () => { setDeleteTarget(null); setTargetId(null); refresh(); }, onError: fail });
  const removeSchedule = useApiMutation({ mutationFn: deletePlaybookSchedule, onSuccess: () => { setDeleteSchedule(null); refresh(); }, onError: fail });
  const toggleSchedule = useApiMutation({ mutationFn: ({ id, enabled }: { id: string; enabled: boolean }) => updatePlaybookSchedule(id, { enabled }), onSuccess: refresh, onError: fail });
  const titles: Record<PlaybooksTab, string> = {
    projects: t("playbooks.page.projects.title"),
    "job-templates": t("playbooks.page.jobTemplates.title"), runs: t("playbooks.page.runs.title"), schedules: t("playbooks.page.schedules.title"),
    "execution-environments": t("playbooks.page.executionEnvironments.title"), "target-sets": t("playbooks.page.targetSets.title"), assignments: t("playbooks.page.assignments.title", { defaultValue: "Assignments" }),
    settings: t("playbooks.page.settings", { defaultValue: "Settings" }),
  };
  const create = {
    projects: ["PLAYBOOK_PROJECT", projectEditorNewHref], "job-templates": ["PLAYBOOK_JOB_TEMPLATE", jobTemplateEditorNewHref],
    schedules: ["PLAYBOOK_SCHEDULE", scheduleEditorNewHref], "execution-environments": ["PLAYBOOK_EXECUTION_ENVIRONMENT", executionEnvironmentEditorNewHref],
    "target-sets": ["PLAYBOOK_TARGET_SET", targetSetEditorNewHref],
  } as const;
  const descriptions: Record<PlaybooksTab, string> = {
    projects: t("playbooks.page.projects.description"),
    "job-templates": t("playbooks.page.jobTemplates.description"),
    runs: t("playbooks.page.runs.description"),
    schedules: t("playbooks.page.schedules.description"),
    "execution-environments": t("playbooks.page.executionEnvironments.description"),
    "target-sets": t("playbooks.page.targetSets.description"),
    assignments: t("playbooks.page.assignments.description"),
    settings: t("playbooks.page.help.settingsOverview"),
  };
  // Relations name what a section depends on, so the operator can follow the
  // chain instead of guessing where a target set or identity comes from.
  const related: Record<PlaybooksTab, PlaybooksTab[]> = {
    projects: ["job-templates"],
    "job-templates": ["projects", "target-sets", "execution-environments", "runs"],
    runs: ["job-templates"],
    schedules: ["job-templates", "target-sets"],
    "execution-environments": ["job-templates"],
    "target-sets": ["job-templates", "schedules", "assignments"],
    assignments: ["job-templates", "target-sets"],
    settings: [],
  };
  const docs = automationDocUrl(PLAYBOOK_TAB_DOCS[tab].slug, PLAYBOOK_TAB_DOCS[tab].hash);
  const sectionHelp = <div className="flex max-w-prose flex-col gap-2">
    <p>{descriptions[tab]}</p>
    {related[tab].length > 0 && <p className="text-muted-foreground">{t("playbooks.page.help.relations", { sections: related[tab].map(entry => titles[entry]).join(", ") })}</p>}
    <a className="inline-flex items-center gap-1 font-medium underline underline-offset-2" href={docs} target="_blank" rel="noopener noreferrer">{t("playbooks.page.help.docs")}<span className="sr-only">{t("playbooks.page.help.docsNewTab")}</span></a>
  </div>;
  const emptyKeys = { projects: "projects", "job-templates": "jobTemplates", runs: "runs", schedules: "schedules", "execution-environments": "executionEnvironments", } as const;
  const emptyState = (kind: keyof typeof emptyKeys) => {
    const creation = kind in create ? create[kind as keyof typeof create] : null;
    return <PageState kind="empty" title={t(`playbooks.page.empty.${emptyKeys[kind]}`)} description={t(`playbooks.page.emptyHints.${emptyKeys[kind]}`)} action={creation && can("CREATE", creation[0]) ? <Button onClick={() => void navigate(creation[1]())}><Plus className="size-4" />{t("create")}</Button> : undefined} />;
  };
  const createAction = tab in create ? create[tab as keyof typeof create] : null;
  const settingsSection = isPlaybookSettingsTab(tab);
  return <div className="flex min-h-0 min-w-0 flex-1 flex-col gap-4">
    <PageChrome icon={ClipboardList} title={titles[tab]} description={sectionHelp} descriptionLabel={t("playbooks.page.help.action")} />
    <PageActions><Button variant="outline" onClick={refresh}><RefreshCw className="size-4" />{t("refresh")}</Button>{createAction && can("CREATE", createAction[0]) && <Button onClick={() => void navigate(createAction[1](), { state: targetSetReturnState })}><Plus className="size-4" />{t("create")}</Button>}</PageActions>
    <SectionLinks label={t("playbooks.page.sectionsLabel")} activeValue={settingsSection ? "settings" : tab} items={primaryLinks.map(item => ({ ...item, to: PLAYBOOK_TAB_ROUTES[item.value] }))} />
    {settingsSection && tab !== "settings" && <div className="flex min-w-0 flex-wrap items-center justify-between gap-3">
      <SectionLinks label={t("navItems.automationSetup")} className="min-w-0" activeValue={tab} items={settingsLinks.map(item => ({ ...item, to: PLAYBOOK_TAB_ROUTES[item.value] }))} />
      {targetSetReturn && <Button asChild variant="outline"><Link to={targetSetReturn.returnHref} state={targetSetReturnState}><ArrowLeft className="size-4" />{t(targetSetReturn.labelKey)}</Link></Button>}
    </div>}
    {tab === "settings" && <SettingsOverview />}
    {tab === "projects" && <ProjectsPanel projects={projects.data?.items ?? []} loading={projects.isPending} error={projects.isError ? projects.error : undefined} onRetry={() => void projects.refetch()} executionEnvironmentById={environmentMap} onCreateJobTemplate={() => void navigate(jobTemplateEditorNewHref())} emptyState={emptyState("projects")} />}
    {tab === "job-templates" && <JobTemplatesPanel jobTemplates={jobs.data?.items ?? []} loading={jobs.isPending} error={jobs.isError ? jobs.error : undefined} onRetry={() => void jobs.refetch()} projectById={projectMap} targetSetById={targetMap} executionEnvironmentById={environmentMap} onRun={item => setJobId(item.id)} runPending={launch.isPending} emptyState={emptyState("job-templates")} />}
    {tab === "execution-environments" && <ExecutionEnvironmentsPanel executionEnvironments={environments.data?.items ?? []} loading={environments.isPending} error={environments.isError ? environments.error : undefined} onRetry={() => void environments.refetch()} onCreateJobTemplate={item => void navigate(jobTemplateEditorNewForExecutionEnvironmentHref(item.id))} emptyState={emptyState("execution-environments")} />}
    {tab === "assignments" && <AssignmentsSettings />}
    {tab === "target-sets" && <div className="grid min-h-0 gap-4 2xl:grid-cols-[minmax(0,1fr)_minmax(18rem,24rem)]"><TargetSetsPanel targetSets={targets.query.data?.items ?? []} loading={targets.query.isPending} error={targets.query.isError ? targets.query.error : undefined} onRetry={targets.refresh} selectedId={targetId} sortState={targets.sortState} onSortChange={targets.setSortState} columnFilters={targets.columnFilters} onColumnFilterChange={targets.setColumnFilter} pagination={{ page: targets.page, pageSize: targets.pageSize, total: targets.query.data?.total ?? 0, onPageChange: targets.setPage, onPageSizeChange: targets.setPageSize }} onEdit={item => void navigate(targetSetEditorEditHref(item.id))} onDelete={setDeleteTarget} onDuplicate={item => void navigate(targetSetEditorNewForDuplicateHref(item.id))} onSelect={item => setTargetId(item.id)} toolbar={<SearchInput value={targets.search} onValueChange={targets.setSearch} placeholder={t("playbooks.page.targetSets.searchPlaceholder")} />} emptyState={targets.search.trim() ? <PageState kind="empty" title={t("playbooks.page.targetSets.noSearchResults")} /> : can("CREATE", "PLAYBOOK_TARGET_SET") ? <PageState kind="empty" title={t("playbooks.page.empty.targetSets")} description={t("playbooks.page.targetSets.emptyDescription")} action={<Button onClick={() => void navigate(targetSetEditorNewHref(), { state: targetSetReturnState })}><Plus className="size-4" />{t("create")}</Button>}><div className="flex flex-wrap items-center justify-center gap-2">{TARGET_SET_TEMPLATES.map(template => <Button key={template.key} variant="outline" size="sm" onClick={() => void navigate(targetSetEditorNewForTemplateHref(template.key), { state: targetSetReturnState })}>{t(`playbooks.page.targetSetTemplates.${template.key}.title`)}</Button>)}</div></PageState> : <PageState kind="empty" title={t("playbooks.page.empty.targetSets")} />} /><SurfaceCard><SurfaceCardHeader title={t("playbooks.page.preview.title")} /><SurfaceCardContent>{targetId === null ? <PageState kind="empty" title={t("playbooks.page.preview.empty")} /> : preview.isPending ? <PageState kind="loading" title={t("loading")} /> : preview.data ? <TargetPreview preview={preview.data} /> : <PageState kind="error" title={t("playbooks.page.preview.failed")} />}</SurfaceCardContent></SurfaceCard></div>}
    {tab === "schedules" && <SchedulesPanel schedules={schedules.query.data?.items ?? []} loading={schedules.query.isPending} error={schedules.query.isError ? schedules.query.error : undefined} onRetry={schedules.refresh} sortState={schedules.sortState} onSortChange={schedules.setSortState} columnFilters={schedules.columnFilters} onColumnFilterChange={schedules.setColumnFilter} pagination={{ page: schedules.page, pageSize: schedules.pageSize, total: schedules.query.data?.total ?? 0, onPageChange: schedules.setPage, onPageSizeChange: schedules.setPageSize }} targetSetById={targetMap} onEdit={item => void navigate(scheduleEditorEditHref(item.id))} onToggle={(item, enabled) => toggleSchedule.mutate({ id: item.id, enabled })} onDelete={setDeleteSchedule} togglingId={toggleSchedule.isPending ? toggleSchedule.variables?.id ?? null : null} emptyState={emptyState("schedules")} />}
    {tab === "runs" && <div className={detail ? "grid min-h-0 flex-1 gap-3 lg:grid-cols-[minmax(0,1fr)_minmax(0,1fr)]" : "min-h-0 flex-1"}><RunsPanel runGroups={runs.query.data?.items ?? []} loading={runs.query.isPending} error={runs.query.isError ? runs.query.error : undefined} onRetry={runs.refresh} selectedGroupId={detail?.target.groupId} sortState={runs.sortState} onSortChange={runs.setSortState} columnFilters={runs.columnFilters} onColumnFilterChange={runs.setColumnFilter} pagination={{ page: runs.page, pageSize: runs.pageSize, total: runs.query.data?.total ?? 0, onPageChange: runs.setPage, onPageSizeChange: runs.setPageSize }} onOpenGroup={openGroup} emptyState={emptyState("runs")} />{detail && <RunDetailPanel target={detail.target} playbookName={detail.name} onClose={clearDetail} />}</div>}
    <PlaybookLaunchDialog varsSchema={launchResourceFromQuery(launchVarsSchema)} open={jobId !== null} intent={jobId ? { jobTemplateId: jobId } : null} onOpenChange={open => { if (!open) setJobId(null); }} onConfirm={intent => launch.mutate(intent)} jobTemplates={jobs.data?.items ?? []} projects={projects.data?.items ?? []} onSelectedJobTemplateIdChange={setJobId} loading={jobs.isPending || (can("READ", "PLAYBOOK_PROJECT") && projects.isPending)} preview={launchTargetSetId === null ? undefined : launchResourceFromQuery(launchPreview)} secrets={launchResourceFromQuery(launchSecrets)} onRetryComposition={() => { void launchPreview.refetch(); void launchSecrets.refetch(); }} referenceError={!can("READ", "PLAYBOOK_PROJECT") ? t("playbooks.launchDraft.projectPermission", { defaultValue: "Project read permission is required to launch a job." }) : projects.isError ? t("playbooks.launchDraft.projectsUnavailable", { defaultValue: "Projects could not be loaded." }) : undefined} onRetry={projects.isError ? () => { void projects.refetch(); } : undefined} isSubmitting={launch.isPending} canLaunch={can("EXECUTE", "PLAYBOOK_JOB_TEMPLATE") && can("EXECUTE_NATIVE", "PLAYBOOK_JOB_TEMPLATE")} />
    <DeleteTargetSetDialog open={deleteTarget !== null} name={deleteTarget?.name ?? ""} usage={deleteTarget?.usage} error={removeTarget.error} pending={removeTarget.isPending} onOpenChange={open => { if (!open) { setDeleteTarget(null); removeTarget.reset(); } }} onConfirm={() => { if (deleteTarget) removeTarget.mutate(deleteTarget.id); }} />
    <DeleteScheduleDialog open={deleteSchedule !== null} name={deleteSchedule?.name ?? ""} pending={removeSchedule.isPending} onOpenChange={open => { if (!open) setDeleteSchedule(null); }} onConfirm={() => { if (deleteSchedule) removeSchedule.mutate(deleteSchedule.id); }} />
  </div>;
}
