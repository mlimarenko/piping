import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useLocation, useNavigate, useSearchParams } from "react-router";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  ArrowLeft,
  Box,
  CalendarClock,
  ClipboardList,
  Database,
  GitBranch,
  History,
  KeyRound,
  Layers3,
  Play,
  Plus,
  RefreshCw,
  Route,
  Rocket,
} from "lucide-react";

import PageChrome, { PageActions } from "@/shared/ui-shared/PageChrome";
import SurfaceCard, { SurfaceCardContent, SurfaceCardHeader } from "@/shared/ui-shared/SurfaceCard";
import type { SectionTabItem } from "@/shared/ui-shared/SectionTabs";
import SectionLinks from "@/shared/ui-shared/SectionLinks";
import PageState from "@/shared/ui-shared/PageState";
import PaneResizer from "@/shared/ui-shared/PaneResizer";
import {
  PLAYBOOK_ASSIGNMENTS_TABLE_CONFIG,
  PLAYBOOK_RUN_GROUPS_TABLE_CONFIG,
  PLAYBOOK_SCHEDULES_TABLE_CONFIG,
  PLAYBOOK_SERVICE_PRINCIPALS_TABLE_CONFIG,
  PLAYBOOK_TARGET_SETS_TABLE_CONFIG,
} from "@/features/playbooks/tableConfig";
import { useServerTable, type ServerTableListParams } from "@/shared/ui-shared/useServerTable";
import { Button } from "@/shared/ui-primitives/button";
import { useApiMutation } from "@/shared/api/hooks";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { useSetting } from "@/shared/settings";
import type { ApiError } from "@/shared/api/client";
import { notify } from "@/shared/lib/notifications";
import { assertUuid } from "@/shared/lib/ids";
import { cn } from "@/shared/lib/utils";
import { listConnectionTargets, CONNECTION_TARGETS_QUERY_KEY } from "@/features/connections/api";
import {
  createPlaybookAssignment,
  createPlaybookRunGroup,
  createPlaybookTargetSet,
  deletePlaybook,
  deletePlaybookAssignment,
  deletePlaybookSchedule,
  deletePlaybookTargetSet,
  listPlaybookAssignments,
  listExecutionEnvironments,
  listJobTemplates,
  listPlaybooks,
  listProjects,
  listPlaybookRunGroups,
  listPlaybookSchedules,
  listPlaybookServicePrincipals,
  listPlaybookTargetSets,
  launchJobTemplate,
  previewPlaybookTargetSet,
  revokePlaybookServicePrincipal,
  updatePlaybookAssignment,
  updatePlaybook,
  updatePlaybookSchedule,
  type CreatePlaybookAssignmentRequest,
  type CreateRunGroupRequest,
  type PlaybookExecutionEnvironment,
  type PlaybookJobTemplate,
  type PlaybookAssignment,
  type PlaybookCatalogItem,
  type PlaybookProject,
  type PlaybookRunGroup,
  type PlaybookRunGroupCreated,
  type PlaybookSchedule,
  type PlaybookServicePrincipal,
  type PlaybookTargetSet,
  type UpdatePlaybookAssignmentRequest,
} from "@/features/playbooks/api";
import PlaybookLaunchDialog, {
  type PlaybookLaunchIntent,
} from "@/features/playbooks/PlaybookLaunchDialog";
import { DeletePlaybookDialog } from "@/features/playbooks/DeletePlaybookDialog";
import { RevokeServicePrincipalDialog } from "@/features/playbooks/RevokeServicePrincipalDialog";
import { AssignmentDialog } from "@/features/playbooks/AssignmentDialog";
import { DeleteTargetSetDialog } from "@/features/playbooks/DeleteTargetSetDialog";
import { DeleteAssignmentDialog } from "@/features/playbooks/DeleteAssignmentDialog";
import { DeleteScheduleDialog } from "@/features/playbooks/DeleteScheduleDialog";
import { usePlaybookScopeEntities } from "@/features/playbooks/usePlaybookScopeEntities";
import type { SelectedHost } from "@/features/playbooks/hostSelection";
import {
  RunDetailPanel,
  type RunDetailTarget,
} from "@/features/playbooks/RunDetailPanel";
import {
  clampRunDetailWidth,
  isWideRunsLayout,
  maxRunDetailWidth,
  resolveRunDetailWidth,
  RUNS_DETAIL_COMPACT_MAX_WIDTH,
  RUNS_DETAIL_WIDTH_DEFAULT,
  RUNS_DETAIL_WIDTH_SETTING_KEY,
  type RunsDetailWidthSetting,
} from "@/features/playbooks/runDetailLayout";
import { isTerminalStatus } from "@/features/playbooks/runPhase";
import {
  PLAYBOOK_RUN_GROUP_NAME_PARAM,
  PLAYBOOK_RUN_GROUP_PARAM,
  PLAYBOOK_TAB_ROUTES,
  playbooksTabFromPath,
  type PlaybooksTab,
} from "@/features/playbooks/playbookNavigation";
import {
  asJsonObject,
  targetSetLabel,
} from "@/features/playbooks/playbookPageFormat";
import { type AssignmentForm } from "@/features/playbooks/playbookFormModels";
import {
  executionEnvironmentEditorNewHref,
  jobTemplateEditorNewHref,
  jobTemplateEditorNewForExecutionEnvironmentHref,
  jobTemplateEditorNewForProjectHref,
  playbookEditorCopyHref,
  playbookEditorEditHref,
  playbookEditorNewHref,
  projectEditorNewHref,
  scheduleEditorEditHref,
  scheduleEditorNewHref,
  servicePrincipalEditorNewHref,
  targetSetEditorEditHref,
  targetSetEditorNewHref,
  targetSetEditorNewForAssignmentHref,
} from "@/features/playbooks/playbookEditorRoutes";
import {
  PlaybookTabPanel,
  TargetPreview,
} from "@/features/playbooks/PlaybookPanelUi";
import { CatalogPanel } from "@/features/playbooks/CatalogPanel";
import { AssignmentsPanel } from "@/features/playbooks/AssignmentsPanel";
import { TargetSetsPanel } from "@/features/playbooks/TargetSetsPanel";
import { SchedulesPanel } from "@/features/playbooks/SchedulesPanel";
import { ServicePrincipalsPanel } from "@/features/playbooks/ServicePrincipalsPanel";
import { RunsPanel } from "@/features/playbooks/RunsPanel";
import { ExecutionEnvironmentsPanel } from "@/features/playbooks/ExecutionEnvironmentsPanel";
import { ProjectsPanel } from "@/features/playbooks/ProjectsPanel";
import { JobTemplatesPanel } from "@/features/playbooks/JobTemplatesPanel";
import { usePlaybookTable } from "./usePlaybookTable";
import { playbookDetailHref } from "./playbookDetailRoutes";
import { consumeAssignmentDraft, readAssignmentDraft, startAssignmentDraft } from "./assignmentDraft";

// Server-list params the shared `useServerTable` composes for each collection.
// The feature list function is the authority on the domain filter keys; the
// shared shape only adds the transport fields (`search`, `sort`, `order`,
// `limit`, `offset`) every server list already shares.
type AssignmentListParams = Parameters<typeof listPlaybookAssignments>[0] & ServerTableListParams;
type TargetSetListParams = Parameters<typeof listPlaybookTargetSets>[0] & ServerTableListParams;
type ScheduleListParams = Parameters<typeof listPlaybookSchedules>[0] & ServerTableListParams;
type ServicePrincipalListParams = Parameters<typeof listPlaybookServicePrincipals>[0] & ServerTableListParams;
type RunGroupListParams = Parameters<typeof listPlaybookRunGroups>[0] & ServerTableListParams;

// The unified "Run playbook" dialog picks a playbook + a targeting strategy
// (an existing target set, or an ad-hoc list of hosts) before handing off to
// the shared review dialog. It replaces the old per-row "Run default" /
// "Run on hosts" split with a single Run entry point.
type RunTargetMode = "target-set" | "hosts";

const emptyAssignmentForm = (): AssignmentForm => ({
  playbookId: "",
  targetSetId: "",
  name: "",
  scopeKind: "global",
  scopeId: "",
  priority: "100",
  enabled: true,
  visible: true,
  executable: true,
  quickActionEnabled: false,
});

type HeaderCreateAction = { label: string; onClick: () => void };

function headerCreateActionIfAllowed(
  allowed: boolean,
  action: HeaderCreateAction,
): HeaderCreateAction | null {
  return allowed ? action : null;
}

// The seven setup surfaces reachable from the sidebar's single "Setup" entry.
// They keep their /automation/* routes; the in-page pill row switches between
// them so the sidebar stays at three automation entries.
const SETUP_TABS: ReadonlySet<PlaybooksTab> = new Set([
  "projects",
  "job-templates",
  "execution-environments",
  "assignments",
  "target-sets",
  "schedules",
  "service-principals",
]);

const PlaybooksPage = ({ initialTab = "catalog", parent }: { initialTab?: PlaybooksTab; parent?: PlaybookCatalogItem } = {}) => {
  const { t } = useTranslation();
  // View-but-not-edit: hide mutation affordances the caller can't use; the
  // server still enforces. Run/launch maps to EXECUTE on the playbook itself.
  const { can } = usePermissions();
  const canCreatePlaybook = can("CREATE", "PLAYBOOK");
  const canExecutePlaybook = can("EXECUTE", "PLAYBOOK");
  const canCreateExecutionEnvironment = can("CREATE", "PLAYBOOK_EXECUTION_ENVIRONMENT");
  const canCreateProject = can("CREATE", "PLAYBOOK_PROJECT");
  const canCreateJobTemplate = can("CREATE", "PLAYBOOK_JOB_TEMPLATE");
  const canCreateAssignment = can("CREATE", "PLAYBOOK_ASSIGNMENT");
  const canCreateTargetSet = can("CREATE", "PLAYBOOK_TARGET_SET");
  const canCreateSchedule = can("CREATE", "PLAYBOOK_SCHEDULE");
  const canCreateServicePrincipal = can("CREATE", "PLAYBOOK_SERVICE_PRINCIPAL");
  const queryClient = useQueryClient();
  const location = useLocation();
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();
  const parentId = parent?.playbook.id;
  const tab = parent ? initialTab : playbooksTabFromPath(location.pathname, initialTab);
  const setTab = useCallback((nextTab: PlaybooksTab) => {
    if (parentId && nextTab === "runs") {
      void navigate(playbookDetailHref(parentId, "runs", location.search));
      return;
    }
    const nextPath = PLAYBOOK_TAB_ROUTES[nextTab];
    if (location.pathname !== nextPath) {
      void navigate(nextPath);
    }
  }, [location.pathname, location.search, navigate, parentId]);
  // Pill row for the setup surfaces; entries the user cannot read are hidden,
  // mirroring the capability gates the sidebar applied when each surface had
  // its own nav item.
  const setupTabOptions = useMemo<SectionTabItem<PlaybooksTab>[]>(() => {
    const entries: Array<{ value: PlaybooksTab; labelKey: string; gated: boolean }> = [
      { value: "projects", labelKey: "navItems.projects", gated: can("READ", "PLAYBOOK_PROJECT") },
      { value: "job-templates", labelKey: "navItems.jobTemplates", gated: can("READ", "PLAYBOOK_JOB_TEMPLATE") },
      { value: "execution-environments", labelKey: "navItems.executionEnvironments", gated: can("READ", "PLAYBOOK_EXECUTION_ENVIRONMENT") },
      { value: "assignments", labelKey: "navItems.assignments", gated: can("READ", "PLAYBOOK_ASSIGNMENT") },
      { value: "target-sets", labelKey: "navItems.targetSets", gated: can("READ", "PLAYBOOK_TARGET_SET") },
      { value: "schedules", labelKey: "navItems.schedules", gated: can("READ", "PLAYBOOK_SCHEDULE") },
      { value: "service-principals", labelKey: "navItems.servicePrincipals", gated: can("READ", "PLAYBOOK_SERVICE_PRINCIPAL") },
    ];
    return entries
      .filter((entry) => entry.gated)
      .map((entry) => ({ value: entry.value, label: t(entry.labelKey) }));
  }, [can, t]);
  // Delete-playbook confirmation + service-principal revoke confirmation.
  const [deletePlaybookItem, setDeletePlaybookItem] = useState<PlaybookCatalogItem | null>(null);
  const [revokeServicePrincipal, setRevokeServicePrincipal] =
    useState<PlaybookServicePrincipal | null>(null);
  const draft = readAssignmentDraft(location.state);
  const returnedDraft = draft && (draft.editingId ? can("UPDATE", "PLAYBOOK_ASSIGNMENT") : canCreateAssignment) && (!parentId || draft.form.playbookId === parentId) ? draft : null;
  const [assignmentDialogOpen, setAssignmentDialogOpen] = useState(Boolean(returnedDraft));
  const [assignmentForm, setAssignmentForm] = useState<AssignmentForm>(() => returnedDraft?.form ?? emptyAssignmentForm());
  const [editingAssignmentId, setEditingAssignmentId] = useState<string | null>(returnedDraft?.editingId ?? null);
  const clearAssignmentDraft = () => {
    consumeAssignmentDraft(location.state);
    if (draft) void navigate(location.pathname + location.search, { replace: true, state: null });
  };
  const [assignmentDeleteId, setAssignmentDeleteId] = useState<string | null>(null);
  const [targetSetDeleteId, setTargetSetDeleteId] = useState<string | null>(null);

  const [previewTargetSetId, setPreviewTargetSetId] = useState<string | null>(null);
  const [launchIntent, setLaunchIntent] = useState<PlaybookLaunchIntent | null>(null);

  // Unified run flow. The shared launch dialog owns the final review and,
  // when opened from this page, also owns playbook + target source selection.
  const [runDialogOpen, setRunDialogOpen] = useState(false);
  const [runDialogPlaybookId, setRunDialogPlaybookId] = useState("");
  const [runTargetMode, setRunTargetMode] = useState<RunTargetMode>("target-set");
  const [runTargetSetId, setRunTargetSetId] = useState("");
  const [runHostSelection, setRunHostSelection] = useState<SelectedHost[]>([]);

  // The active run-detail target: either a just-launched group (seeded with its
  // runs) or a history group drilled into by id.
  const [unscopedRunDetail, setRunDetail] = useState<
    { target: RunDetailTarget; playbookName: string } | null
  >(null);
  const scopedGroupId = searchParams.get(PLAYBOOK_RUN_GROUP_PARAM);
  const runDetail = parent
    ? scopedGroupId ? { target: { kind: "group" as const, groupId: scopedGroupId }, playbookName: parent.playbook.name } : null
    : unscopedRunDetail;

  // Container-width-driven master-detail layout for the Runs tab (wide: side
  // panel + PaneResizer; narrow: drill-in with a back button). Measured via
  // ResizeObserver rather than the viewport because the nav sidebar steals
  // width from the tab content — see runDetailLayout.ts for the pure math.
  // Default (1200) assumes a wide desktop container until the observer
  // reports the real size, matching the wide/simultaneous layout existing
  // tests already exercise.
  const runsContainerObserverRef = useRef<ResizeObserver | null>(null);
  const [runsContainerWidth, setRunsContainerWidth] = useState(1200);
  const runsContainerRef = useCallback((node: HTMLDivElement | null) => {
    runsContainerObserverRef.current?.disconnect();
    runsContainerObserverRef.current = null;
    if (!node || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver((entries) => {
      for (const entry of entries) {
        setRunsContainerWidth(entry.contentRect.width);
      }
    });
    observer.observe(node);
    runsContainerObserverRef.current = observer;
  }, []);
  const runsWideMode = isWideRunsLayout(runsContainerWidth);
  const { value: runsDetailWidthSetting, setValue: setRunsDetailWidthSetting } =
    useSetting<RunsDetailWidthSetting>(
      RUNS_DETAIL_WIDTH_SETTING_KEY,
      RUNS_DETAIL_WIDTH_DEFAULT,
      { debounceMs: 300 },
    );
  const runsDetailMaxWidth = maxRunDetailWidth(runsContainerWidth);
  const resolvedRunsDetailWidth = resolveRunDetailWidth(
    runsDetailWidthSetting.width,
    runsContainerWidth,
  );
  const handleRunsDetailResize = useCallback(
    (nextWidth: number) => {
      setRunsDetailWidthSetting({ width: clampRunDetailWidth(nextWidth, runsContainerWidth) });
    },
    [runsContainerWidth, setRunsDetailWidthSetting],
  );

  const [scheduleDeleteId, setScheduleDeleteId] = useState<string | null>(null);
  const rowDialogInvoker = useRef<HTMLElement | null>(null);
  const rowDialogOpen = Boolean(deletePlaybookItem || revokeServicePrincipal || assignmentDeleteId || targetSetDeleteId || scheduleDeleteId);
  const openRowDialog = (open: () => void) => {
    rowDialogInvoker.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    open();
  };
  useEffect(() => {
    if (rowDialogOpen || !rowDialogInvoker.current) return;
    const invoker = rowDialogInvoker.current;
    rowDialogInvoker.current = null;
    const timer = window.setTimeout(() => {
      if (invoker.isConnected) invoker.focus();
    }, 0);
    return () => window.clearTimeout(timer);
  }, [rowDialogOpen]);

  // The catalog is the name source for every surface that renders a playbook
  // (`playbookById` in the job-template, assignment, schedule and run tables) and
  // the option list for the assignment, schedule, job-template and launch
  // dialogs. Only the two tabs that never name a playbook can skip it.
  const catalogQuery = useQuery({
    queryKey: ["playbooks", "catalog"],
    queryFn: () => listPlaybooks({ limit: 100, offset: 0 }),
    enabled: !parent && can("READ", "PLAYBOOK") && tab !== "target-sets" && tab !== "service-principals",
  });
  // Own panel, plus the environment-name column on projects/job templates and
  // the environment picker in the project and job-template dialogs. The job
  // template dialog is reachable from all three tabs and seeds its form from
  // `executionEnvironments[0]` at open time, so the data must already be warm.
  const executionEnvironmentsQuery = useQuery({
    queryKey: ["playbooks", "execution-environments"],
    queryFn: () => listExecutionEnvironments({ limit: 100, offset: 0 }),
    enabled:
      tab === "execution-environments" || tab === "projects" || tab === "job-templates",
  });
  // Own panel, plus the project-name column on job templates and the project
  // picker/seed in the job-template dialog (openable from all three tabs).
  const projectsQuery = useQuery({
    queryKey: ["playbooks", "projects"],
    queryFn: () => listProjects({ limit: 100, offset: 0 }),
    enabled:
      tab === "projects" || tab === "job-templates" || tab === "execution-environments",
  });
  const jobTemplatesQuery = useQuery({
    queryKey: ["playbooks", "job-templates"],
    queryFn: () => listJobTemplates({ limit: 100, offset: 0 }),
    enabled: tab === "job-templates",
  });
  const assignmentsTable = usePlaybookTable<PlaybookAssignment, AssignmentListParams>({
    listFn: (params) => listPlaybookAssignments(params),
    queryKey: ["playbooks", "assignments"],
    sortKeys: PLAYBOOK_ASSIGNMENTS_TABLE_CONFIG.sortKeys,
    filterColumns: PLAYBOOK_ASSIGNMENTS_TABLE_CONFIG.filterColumns,
    pageSizeDefault: 100,
    keepPreviousData: true,
    enabled: tab === "assignments" && can("READ", "PLAYBOOK_ASSIGNMENT"),
  }, parentId, "assignments");
  const targetSetsTable = useServerTable<PlaybookTargetSet, TargetSetListParams>({
    listFn: (params) => listPlaybookTargetSets(params),
    queryKey: ["playbooks", "target-sets"],
    sortKeys: PLAYBOOK_TARGET_SETS_TABLE_CONFIG.sortKeys,
    filterColumns: PLAYBOOK_TARGET_SETS_TABLE_CONFIG.filterColumns,
    pageSizeDefault: 100,
    keepPreviousData: true,
    enabled: tab === "target-sets",
  });
  // Unfiltered corpus. Distinct from `targetSetsQuery`: this one resolves
  // `targetSetById` for the target-set name shown on catalog, job-template,
  // assignment and schedule rows, and it is the option list (and form seed) for
  // every picker. Needed on every tab except Service principals — including
  // Target sets itself, where the delete confirmation resolves its name through
  // `targetSetById`.
  const allTargetSetsQuery = useQuery({
    queryKey: ["playbooks", "target-sets", "all"],
    queryFn: () => listPlaybookTargetSets({ limit: 100, offset: 0 }),
    enabled: tab !== "service-principals" && can("READ", "PLAYBOOK_TARGET_SET"),
  });
  const schedulesTable = usePlaybookTable<PlaybookSchedule, ScheduleListParams>({
    listFn: (params) => listPlaybookSchedules(params),
    queryKey: ["playbooks", "schedules"],
    sortKeys: PLAYBOOK_SCHEDULES_TABLE_CONFIG.sortKeys,
    filterColumns: PLAYBOOK_SCHEDULES_TABLE_CONFIG.filterColumns,
    pageSizeDefault: 100,
    keepPreviousData: true,
    enabled: tab === "schedules" && can("READ", "PLAYBOOK_SCHEDULE"),
  }, parentId, "schedules");
  const servicePrincipalsTable = useServerTable<PlaybookServicePrincipal, ServicePrincipalListParams>({
    listFn: (params) => listPlaybookServicePrincipals(params),
    queryKey: ["playbooks", "service-principals"],
    sortKeys: PLAYBOOK_SERVICE_PRINCIPALS_TABLE_CONFIG.sortKeys,
    filterColumns: PLAYBOOK_SERVICE_PRINCIPALS_TABLE_CONFIG.filterColumns,
    pageSizeDefault: 100,
    keepPreviousData: true,
    // Own panel, plus the principal-name column and the principal picker in the
    // schedule dialog.
    enabled: (tab === "service-principals" || tab === "schedules") && can("READ", "PLAYBOOK_SERVICE_PRINCIPAL"),
  });
  const runGroupsTable = usePlaybookTable<PlaybookRunGroup, RunGroupListParams>({
    listFn: (params) => listPlaybookRunGroups(params),
    queryKey: ["playbooks", "run-groups"],
    sortKeys: PLAYBOOK_RUN_GROUPS_TABLE_CONFIG.sortKeys,
    filterColumns: PLAYBOOK_RUN_GROUPS_TABLE_CONFIG.filterColumns,
    pageSizeDefault: 100,
    keepPreviousData: true,
    // Runs tab only. `runGroups` feeds the RunsPanel and the runs badge, and
    // that badge lives inside `case "runs":` of the header switch — nothing
    // reads this data from another tab. The cross-page runs dock has its own
    // query key ("active-dock"), so gating here does not blind it.
    enabled: tab === "runs" && can("READ", "PLAYBOOK_RUN_GROUP"),
    // Refresh every 5 s while any visible group is non-terminal so the table
    // stays current without a manual reload. Stops polling once all are done.
    refetchInterval: (query) => {
      const items = query.state.data?.items ?? [];
      const anyActive = items.some((group) => !isTerminalStatus(group.status));
      return anyActive ? 5000 : false;
    },
  }, parentId, "runs");
  const previewQuery = useQuery({
    queryKey: ["playbooks", "target-set-preview", previewTargetSetId],
    queryFn: () => previewPlaybookTargetSet(previewTargetSetId ?? "", 50),
    enabled: previewTargetSetId !== null && can("READ", "PLAYBOOK_TARGET_SET"),
  });
  // Connection inventory powers the dynamic selector facet options and the
  // multi-host / static picker. Shares the workspace's canonical cache entry.
  const connectionTargetsQuery = useQuery({
    queryKey: CONNECTION_TARGETS_QUERY_KEY,
    queryFn: () => listConnectionTargets(),
    enabled: runDialogOpen && runTargetMode === "hosts",
    staleTime: 60_000,
  });

  const playbooks = useMemo(() => parent ? [parent] : catalogQuery.data?.items ?? [], [catalogQuery.data, parent]);
  const executionEnvironments = useMemo(
    () => executionEnvironmentsQuery.data?.items ?? [],
    [executionEnvironmentsQuery.data],
  );
  const projects = useMemo(() => projectsQuery.data?.items ?? [], [projectsQuery.data]);
  const jobTemplates = useMemo(
    () => jobTemplatesQuery.data?.items ?? [],
    [jobTemplatesQuery.data],
  );
  const assignments = assignmentsTable.query.data?.items ?? [];
  const targetSets = useMemo(
    () => targetSetsTable.query.data?.items ?? [],
    [targetSetsTable.query.data],
  );
  const allTargetSets = useMemo(
    () => allTargetSetsQuery.data?.items ?? [],
    [allTargetSetsQuery.data],
  );
  const schedules = schedulesTable.query.data?.items ?? [];
  const servicePrincipals = useMemo(
    () => servicePrincipalsTable.query.data?.items ?? [],
    [servicePrincipalsTable.query.data],
  );
  const runGroups = runGroupsTable.query.data?.items ?? [];
  const connectionTargets = useMemo(
    () => connectionTargetsQuery.data?.items ?? [],
    [connectionTargetsQuery.data],
  );
  const runnablePlaybooks = useMemo(
    () =>
      playbooks.filter(
        (item) => item.playbook.status === "active" && Boolean(item.current_version),
      ),
    [playbooks],
  );
  const playbookById = useMemo(
    () => new Map(playbooks.map((item) => [item.playbook.id, item])),
    [playbooks],
  );
  const executionEnvironmentById = useMemo(
    () =>
      new Map(
        executionEnvironments.map((environment) => [environment.id, environment]),
      ),
    [executionEnvironments],
  );
  const projectById = useMemo(
    () => new Map(projects.map((project) => [project.id, project])),
    [projects],
  );
  const targetSetById = useMemo(
    () => new Map(allTargetSets.map((targetSet) => [targetSet.id, targetSet])),
    [allTargetSets],
  );
  const pickInitialRunTargetSetId = useCallback(
    (playbook: PlaybookCatalogItem | null | undefined) => {
      const defaultId = playbook?.playbook.default_target_set_id ?? "";
      if (defaultId && targetSetById.has(defaultId)) return defaultId;
      return allTargetSets.length === 1 ? (allTargetSets[0]?.id ?? "") : "";
    },
    [allTargetSets, targetSetById],
  );
  const servicePrincipalById = useMemo(
    () => new Map(servicePrincipals.map((principal) => [principal.id, principal])),
    [servicePrincipals],
  );
  // Resolves assignment scope_id → entity name (and powers the scope picker)
  // without ever surfacing a raw UUID.
  const scopeEntities = usePlaybookScopeEntities(
    t("playbooks.page.scope.global"),
    tab === "assignments" || assignmentDialogOpen,
  );
  // Live host-count preview for the target set chosen in the assignment dialog.
  const assignmentPreviewQuery = useQuery({
    queryKey: ["playbooks", "assignment-preview", assignmentForm.targetSetId],
    queryFn: () => previewPlaybookTargetSet(assignmentForm.targetSetId, 1),
    enabled: assignmentDialogOpen && Boolean(assignmentForm.targetSetId) && can("READ", "PLAYBOOK_TARGET_SET"),
    staleTime: 30_000,
  });
  // No target set is choosable yet. Stays false once the assignment form already
  // holds a target set (e.g. just created inline) even before the list refetches,
  // so the selector isn't disabled with a hidden selection.
  const assignmentTargetSetMissing =
    allTargetSets.length === 0 && !assignmentForm.targetSetId;

  const invalidatePlaybooks = () =>
    Promise.all([
      queryClient.invalidateQueries({ queryKey: ["playbooks", "catalog"] }),
      queryClient.invalidateQueries({ queryKey: ["playbooks", "execution-environments"] }),
      queryClient.invalidateQueries({ queryKey: ["playbooks", "projects"] }),
      queryClient.invalidateQueries({ queryKey: ["playbooks", "job-templates"] }),
      queryClient.invalidateQueries({ queryKey: ["playbooks", "assignments"] }),
      queryClient.invalidateQueries({ queryKey: ["playbooks", "target-sets"] }),
      queryClient.invalidateQueries({ queryKey: ["playbooks", "schedules"] }),
      queryClient.invalidateQueries({ queryKey: ["playbooks", "service-principals"] }),
      queryClient.invalidateQueries({ queryKey: ["playbooks", "run-groups"] }),
    ]);

  const archivePlaybookMutation = useApiMutation({
    mutationFn: (id: string) => updatePlaybook(id, { status: "archived" }),
    onSuccess: (updated) => {
      void invalidatePlaybooks();
      notify(
        "success",
        t("playbooks.page.toasts.playbookArchived", { name: updated.playbook.name }),
      );
    },
    onError: (err: ApiError) => notify("error", err.message || t("unknownError")),
  });

  const deletePlaybookMutation = useApiMutation({
    mutationFn: (id: string) => deletePlaybook(id),
    onSuccess: () => {
      void invalidatePlaybooks();
      setDeletePlaybookItem(null);
      notify("success", t("playbooks.page.toasts.playbookDeleted"));
    },
    onError: (err: ApiError) => {
      // RESTRICT FKs on run history make the playbook undeletable; the backend
      // returns 409 — steer the operator to archive instead.
      if (err.status === 409) {
        notify("error", t("playbooks.page.errors.playbookHasRunHistory"));
      } else {
        notify("error", err.message || t("unknownError"));
      }
      setDeletePlaybookItem(null);
    },
  });

  const revokeServicePrincipalMutation = useApiMutation({
    mutationFn: (id: string) => revokePlaybookServicePrincipal(id),
    onSuccess: (principal) => {
      void queryClient.invalidateQueries({
        queryKey: ["playbooks", "service-principals"],
      });
      setRevokeServicePrincipal(null);
      notify(
        "success",
        t("playbooks.page.toasts.servicePrincipalRevoked", { name: principal.name }),
      );
    },
    onError: (err: ApiError) => notify("error", err.message || t("unknownError")),
  });

  const deleteTargetSetMutation = useApiMutation<void, string>({
    mutationFn: (id: string) => deletePlaybookTargetSet(id),
    onSuccess: (_data, deletedId) => {
      void invalidatePlaybooks();
      setTargetSetDeleteId(null);
      setPreviewTargetSetId((current) => (current === deletedId ? null : current));
      notify("success", t("playbooks.page.toasts.targetSetDeleted"));
    },
    onError: (err: ApiError) =>
      notify(
        "error",
        err.status === 409
          ? err.message || t("playbooks.page.errors.targetSetDeleteBlocked")
          : err.message || t("unknownError"),
      ),
  });

  const createAssignmentMutation = useApiMutation({
    mutationFn: (body: CreatePlaybookAssignmentRequest) => createPlaybookAssignment(body),
    onSuccess: () => {
      void invalidatePlaybooks();
      setAssignmentDialogOpen(false);
      clearAssignmentDraft();
      notify("success", t("playbooks.page.toasts.assignmentCreated"));
    },
    onError: (err: ApiError) => notify("error", err.message || t("unknownError")),
  });

  const updateAssignmentMutation = useApiMutation({
    mutationFn: (vars: { id: string; body: UpdatePlaybookAssignmentRequest }) =>
      updatePlaybookAssignment(vars.id, vars.body),
    onSuccess: () => {
      void invalidatePlaybooks();
      setAssignmentDialogOpen(false);
      clearAssignmentDraft();
      setEditingAssignmentId(null);
      notify("success", t("playbooks.page.toasts.assignmentUpdated"));
    },
    onError: (err: ApiError) => notify("error", err.message || t("unknownError")),
  });

  const deleteAssignmentMutation = useApiMutation({
    mutationFn: (id: string) => deletePlaybookAssignment(id),
    onSuccess: () => {
      void invalidatePlaybooks();
      setAssignmentDeleteId(null);
      notify("success", t("playbooks.page.toasts.assignmentDeleted"));
    },
    onError: (err: ApiError) => notify("error", err.message || t("unknownError")),
  });

  const toggleScheduleMutation = useApiMutation({
    mutationFn: (vars: { id: string; enabled: boolean }) =>
      updatePlaybookSchedule(vars.id, { enabled: vars.enabled }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["playbooks", "schedules"] });
    },
    onError: (err: ApiError) => notify("error", err.message || t("unknownError")),
  });

  const deleteScheduleMutation = useApiMutation({
    mutationFn: (id: string) => deletePlaybookSchedule(id),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["playbooks", "schedules"] });
      setScheduleDeleteId(null);
      notify("success", t("playbooks.page.toasts.scheduleDeleted"));
    },
    onError: (err: ApiError) => notify("error", err.message || t("unknownError")),
  });

  const openRunGroupDetail = useCallback(
    (groupId: string, playbookName: string) => {
      if (parentId) {
        const next = new URLSearchParams(location.search);
        next.set(PLAYBOOK_RUN_GROUP_PARAM, groupId);
        next.set(PLAYBOOK_RUN_GROUP_NAME_PARAM, playbookName);
        void navigate(playbookDetailHref(parentId, "runs", next.toString()));
        return;
      }
      setRunDetail({ target: { kind: "group", groupId }, playbookName });
      setTab("runs");
    },
    [setRunDetail, setTab, parentId, location.search, navigate],
  );

  // Deep-link: `/automation/runs?group=<id>&name=<playbookName>` opens that
  // group's RunDetailPanel on the Runs tab. Used by the Connections playbook
  // tab's "open in a new tab" button (`playbookRunGroupHref`), which already
  // knows the playbook name and so does not need a second catalog round-trip
  // here. Applied once per id, then stripped from the URL — mirrors the host
  // deep-link in ConnectionsWorkspace (`CONNECTIONS_HOST_PARAM`).
  const runGroupDeepLinkAppliedRef = useRef<string | null>(null);
  useEffect(() => {
    if (parentId) return;
    const groupId = searchParams.get(PLAYBOOK_RUN_GROUP_PARAM);
    if (!groupId || runGroupDeepLinkAppliedRef.current === groupId) return;
    runGroupDeepLinkAppliedRef.current = groupId;
    const playbookName =
      searchParams.get(PLAYBOOK_RUN_GROUP_NAME_PARAM) || t("playbooks.runDetail.unknownPlaybook");
    openRunGroupDetail(groupId, playbookName);
    setSearchParams(
      (prev) => {
        const next = new URLSearchParams(prev);
        next.delete(PLAYBOOK_RUN_GROUP_PARAM);
        next.delete(PLAYBOOK_RUN_GROUP_NAME_PARAM);
        return next;
      },
      { replace: true },
    );
  }, [openRunGroupDetail, searchParams, setSearchParams, t, parentId]);
  const closeRunDetail = () => {
    setRunDetail(null);
    if (parentId) setSearchParams((previous) => {
      const next = new URLSearchParams(previous);
      next.delete(PLAYBOOK_RUN_GROUP_PARAM);
      next.delete(PLAYBOOK_RUN_GROUP_NAME_PARAM);
      return next;
    });
  };

  const recordRun = (created: PlaybookRunGroupCreated, playbookName: string) => {
    if (parentId) {
      queryClient.setQueryData(["playbooks", "run-group", created.run_group.id], { run_group: created.run_group, runs: created.runs });
      openRunGroupDetail(created.run_group.id, playbookName);
      return;
    }
    // Open the live detail seeded with the just-returned runs so it renders
    // immediately; each run then refreshes through its own GET.
    setRunDetail({
      target: { kind: "seed", runs: created.runs, groupId: created.run_group.id },
      playbookName,
    });
    setTab("runs");
  };

  const runMutation = useApiMutation({
    mutationFn: async (intent: PlaybookLaunchIntent) => {
      const playbook = intent.playbook;
      const playbookId = playbook?.playbook.id;
      if (!playbookId && !intent.jobTemplateId) {
        throw new Error(t("playbooks.page.errors.playbookRequired"));
      }
      // Ad-hoc (multi-host) runs arrive with a static host list carrying their
      // source identity: materialize a static target set first, then queue
      // against it. This routes bulk runs through the same confirmation dialog
      // (review + dry-run + timeout + input) as catalog and assignment runs.
      let targetSetId = intent.targetSetId ?? null;
      if (!targetSetId && intent.staticTargets && intent.staticTargets.length > 0) {
        const created = await createPlaybookTargetSet({
          name: t("playbooks.page.runDialog.targetSetName", {
            name: playbook?.playbook.name ?? "playbook",
            count: intent.staticTargets.length,
          }),
          mode: "static",
          members: intent.staticTargets.map((host) => ({
            target_id: host.id,
            target_kind: "connection_target",
            source_id: host.sourceId ?? null,
            source_kind: host.sourceKind ?? "device",
          })),
        });
        targetSetId = created.target_set.id;
      }
      const body: CreateRunGroupRequest = {
        ...(targetSetId ? { target_set_id: targetSetId } : {}),
        trigger_kind: intent.triggerKind ?? "manual",
        ...(intent.targetLimit ? { target_limit: intent.targetLimit } : {}),
        ...(intent.metadata ? { metadata: intent.metadata } : {}),
      };
      if (intent.jobTemplateId) {
        return launchJobTemplate(intent.jobTemplateId, body);
      }
      if (!playbookId) throw new Error(t("playbooks.page.errors.playbookRequired"));
      return createPlaybookRunGroup(playbookId, body);
    },
    onSuccess: (created, intent) => {
      void queryClient.invalidateQueries({ queryKey: ["playbooks"] });
      const playbookName =
        intent.launchDisplayName ??
        intent.playbook?.playbook.name ??
        t("playbooks.runDetail.unknownPlaybook");
      setLaunchIntent(null);
      setRunDialogOpen(false);
      recordRun(created, playbookName);
      notify(
        "success",
        t("playbooks.page.toasts.runQueued", { count: created.run_group.total_runs }),
      );
    },
    onError: (err: ApiError) => notify("error", err.message || t("unknownError")),
  });

  const refreshAll = () => {
    void invalidatePlaybooks();
    if (previewTargetSetId !== null) {
      void queryClient.invalidateQueries({
        queryKey: ["playbooks", "target-set-preview", previewTargetSetId],
      });
    }
  };

  const openCreatePlaybook = () => {
    void navigate(playbookEditorNewHref());
  };

  const openExecutionEnvironmentDialog = () => {
    void navigate(executionEnvironmentEditorNewHref());
  };

  const openProjectDialog = () => {
    void navigate(projectEditorNewHref());
  };

  const openJobTemplateDialog = () => {
    void navigate(jobTemplateEditorNewHref());
  };

  const openJobTemplateForExecutionEnvironment = (
    environment: PlaybookExecutionEnvironment,
  ) => {
    void navigate(jobTemplateEditorNewForExecutionEnvironmentHref(environment.id));
  };

  const openJobTemplateForProject = (project: PlaybookProject) => {
    void navigate(jobTemplateEditorNewForProjectHref(project.id));
  };

  const openEditPlaybook = (item: PlaybookCatalogItem) => {
    void navigate(playbookEditorEditHref(item.playbook.id));
  };

  const openVersionHistory = (item: PlaybookCatalogItem) => {
    void navigate(playbookDetailHref(item.playbook.id, "versions", new URLSearchParams({ returnTo: location.pathname + location.search }).toString()));
  };

  const openPlaybookTemplateDraft = (item: PlaybookCatalogItem) => {
    void navigate(playbookEditorCopyHref(item.playbook.id));
  };

  const openTargetSetDialog = () => {
    void navigate(targetSetEditorNewHref());
  };

  const openEditTargetSetDialog = (targetSet: PlaybookTargetSet) => {
    void navigate(targetSetEditorEditHref(targetSet.id));
  };

  const openAssignmentDialog = (assignment?: Partial<AssignmentForm>) => {
    setEditingAssignmentId(null);
    setAssignmentForm({ ...emptyAssignmentForm(), ...assignment });
    setAssignmentDialogOpen(true);
  };

  const openEditAssignmentDialog = (assignment: PlaybookAssignment) => {
    setEditingAssignmentId(assignment.id);
    setAssignmentForm({
      playbookId: assignment.playbook_id,
      targetSetId: assignment.target_set_id,
      name: assignment.name ?? "",
      scopeKind: assignment.scope_kind,
      scopeId: assignment.scope_id ?? "",
      priority: String(assignment.priority),
      enabled: assignment.enabled,
      visible: assignment.visible,
      executable: assignment.executable,
      quickActionEnabled: assignment.quick_action_enabled,
    });
    setAssignmentDialogOpen(true);
  };

  const handleAssignmentDialogOpenChange = (open: boolean) => {
    setAssignmentDialogOpen(open);
    if (!open) {
      setEditingAssignmentId(null);
      clearAssignmentDraft();
    }
  };

  // Create a target set from inside the assignment dialog: navigate to the
  // target-set route and return to the assignments tab afterward.
  const openTargetSetDialogForAssignment = () => {
    let state: ReturnType<typeof startAssignmentDraft>;
    try {
      state = startAssignmentDraft({
        returnHref: location.pathname + location.search,
        editingId: editingAssignmentId,
        form: assignmentForm,
      }, location.state);
    } catch {
      // Do not leave the form if the sole flow owner could not persist its non-secret metadata.
      notify("error", t("unknownError"));
      return;
    }
    // Store the same draft on the origin history entry so native Back also restores it.
    void navigate(location.pathname + location.search, { replace: true, state });
    void navigate(targetSetEditorNewForAssignmentHref(), { state });
  };

  const selectRunPlaybook = (playbookId: string) => {
    const playbook = playbookById.get(playbookId);
    const targetSetId = pickInitialRunTargetSetId(playbook);
    setRunDialogPlaybookId(playbookId);
    setRunTargetSetId(targetSetId);
    setRunTargetMode("target-set");
    setRunHostSelection([]);
  };

  // Open the canonical launch dialog. When launched from a catalog row the
  // playbook and its default target set are preloaded, so the common path is
  // one confirmation away. The header action still allows changing both.
  const openRunDialog = (playbookId?: string) => {
    const resolvedId = playbookId ?? runnablePlaybooks[0]?.playbook.id ?? "";
    const playbook = playbookById.get(resolvedId);
    const defaultTargetSetId = pickInitialRunTargetSetId(playbook);
    const defaultTargetSet = defaultTargetSetId ? targetSetById.get(defaultTargetSetId) : undefined;
    setRunDialogPlaybookId(resolvedId);
    setRunTargetSetId(defaultTargetSetId);
    setRunTargetMode("target-set");
    setRunHostSelection([]);
    setRunDialogOpen(true);
    setLaunchIntent({
      playbook: playbook ?? null,
      targetSetId: defaultTargetSetId || null,
      previewTargetSetId: defaultTargetSetId || null,
      targetSetName: defaultTargetSet ? targetSetLabel(defaultTargetSet) : null,
      targetSetMode: defaultTargetSet?.mode ?? null,
      triggerKind: "manual",
      metadata: { source: "playbooks.catalog_default" },
    });
  };

  const openScheduleDialog = () => {
    const href = scheduleEditorNewHref();
    void navigate(parentId ? `${href}?${new URLSearchParams({ playbook: parentId, returnTo: location.pathname + location.search })}` : href);
  };

  const openServicePrincipalDialog = () => {
    void navigate(servicePrincipalEditorNewHref());
  };

  const openScheduleEditDialog = (schedule: PlaybookSchedule) => {
    const href = scheduleEditorEditHref(schedule.id);
    void navigate(parent ? `${href}?${new URLSearchParams({ returnTo: location.pathname + location.search })}` : href);
  };

  const scheduleToDelete = scheduleDeleteId
    ? schedules.find((schedule) => schedule.id === scheduleDeleteId) ?? null
    : null;
  const assignmentToDelete = assignmentDeleteId
    ? assignments.find((assignment) => assignment.id === assignmentDeleteId) ?? null
    : null;
  const targetSetToDelete = targetSetDeleteId
    ? targetSetById.get(targetSetDeleteId) ?? null
    : null;
  const effectiveRunTargetSetId =
    runDialogOpen && runTargetMode === "target-set" && !runTargetSetId
      ? pickInitialRunTargetSetId(playbookById.get(runDialogPlaybookId))
      : runTargetSetId;
  const assignmentToDeleteName =
    assignmentToDelete?.name ??
    (assignmentToDelete
      ? playbookById.get(assignmentToDelete.playbook_id)?.playbook.name ?? assignmentToDelete.id
      : "");
  const targetSetToDeleteName = targetSetToDelete?.name ?? "";

  const buildAssignmentPayload = (): CreatePlaybookAssignmentRequest | null => {
    if (!assignmentForm.playbookId || !assignmentForm.targetSetId) {
      notify("error", t("playbooks.page.errors.assignmentRequired"));
      return null;
    }
    let scopeId: string | null = null;
    if (assignmentForm.scopeKind !== "global") {
      if (!assignmentForm.scopeId) {
        notify("error", t("playbooks.page.errors.scopeTargetRequired"));
        return null;
      }
      // Guard against ever binding a display name into the id field; degrade to
      // a readable error rather than letting the event-handler throw dead-end.
      try {
        scopeId = assertUuid(assignmentForm.scopeId, "scope_id");
      } catch {
        notify("error", t("playbooks.page.errors.scopeTargetRequired"));
        return null;
      }
    }
    return {
      playbook_id: assignmentForm.playbookId,
      target_set_id: assignmentForm.targetSetId,
      name: assignmentForm.name.trim() || null,
      scope_kind: assignmentForm.scopeKind,
      scope_id: scopeId,
      priority: Number.parseInt(assignmentForm.priority, 10) || 100,
      enabled: assignmentForm.enabled,
      visible: assignmentForm.visible,
      executable: assignmentForm.executable,
      quick_action_enabled: assignmentForm.quickActionEnabled,
    };
  };

  const submitAssignment = () => {
    const payload = buildAssignmentPayload();
    if (!payload) return;
    if (editingAssignmentId) {
      updateAssignmentMutation.mutate({ id: editingAssignmentId, body: payload });
      return;
    }
    createAssignmentMutation.mutate(payload);
  };

  const runAssignment = (assignment: PlaybookAssignment) => {
    const playbook = playbookById.get(assignment.playbook_id);
    if (!playbook) {
      notify("error", t("playbooks.page.errors.playbookRequired"));
      return;
    }
    const targetSet = targetSetById.get(assignment.target_set_id);
    setLaunchIntent({
      playbook,
      targetSetId: assignment.target_set_id,
      previewTargetSetId: assignment.target_set_id,
      targetSetName: targetSet ? targetSetLabel(targetSet) : null,
      targetSetMode: targetSet?.mode ?? null,
      triggerKind: "manual",
      metadata: {
        source: "playbooks.assignment",
        assignment_id: assignment.id,
      },
    });
  };

  const runScheduleNow = (schedule: PlaybookSchedule) => {
    const playbook = playbookById.get(schedule.playbook_id);
    if (!playbook) {
      notify("error", t("playbooks.page.errors.playbookRequired"));
      return;
    }
    const targetSet = targetSetById.get(schedule.target_set_id);
    setLaunchIntent({
      playbook,
      targetSetId: schedule.target_set_id,
      previewTargetSetId: schedule.target_set_id,
      targetSetName: targetSet ? targetSetLabel(targetSet) : null,
      targetSetMode: targetSet?.mode ?? null,
      triggerKind: "manual",
      metadata: {
        source: "playbooks.schedule_run_now",
        schedule_id: schedule.id,
      },
    });
  };

  const runJobTemplate = (template: PlaybookJobTemplate) => {
    if (!template.enabled) {
      notify("error", t("playbooks.page.errors.jobTemplateDisabled"));
      return;
    }
    const templateLaunchOptions = asJsonObject(template.launch_options);
    const templateExtraVars = asJsonObject(template.extra_vars);
    if (!template.playbook_id) {
      const targetSetId = template.inventory_target_set_id ?? null;
      if (!targetSetId) {
        notify("error", t("playbooks.page.errors.jobTemplateTargetSetRequired"));
        return;
      }
      const targetSet = targetSetById.get(targetSetId);
      runMutation.mutate({
        playbook: null,
        jobTemplateId: template.id,
        launchDisplayName: template.name,
        targetSetId,
        previewTargetSetId: targetSetId,
        targetSetName: targetSet ? targetSetLabel(targetSet) : null,
        targetSetMode: targetSet?.mode ?? null,
        triggerKind: "manual",
        metadata: {
          source: "playbooks.job_template",
          job_template_id: template.id,
          job_template_name: template.name,
          launch_options: {
            ...templateLaunchOptions,
            extravars: {
              ...asJsonObject(templateLaunchOptions.extravars),
              ...templateExtraVars,
            },
          },
        },
      });
      return;
    }
    const playbook = playbookById.get(template.playbook_id);
    if (!playbook) {
      notify("error", t("playbooks.page.errors.playbookRequired"));
      return;
    }
    const targetSetId =
      template.inventory_target_set_id ?? playbook.playbook.default_target_set_id ?? null;
    const targetSet = targetSetId ? targetSetById.get(targetSetId) : undefined;
    setLaunchIntent({
      playbook,
      jobTemplateId: template.id,
      launchDisplayName: template.name,
      targetSetId,
      previewTargetSetId: targetSetId,
      targetSetName: targetSet ? targetSetLabel(targetSet) : null,
      targetSetMode: targetSet?.mode ?? null,
      triggerKind: "manual",
      metadata: {
        source: "playbooks.job_template",
        job_template_id: template.id,
        job_template_name: template.name,
        launch_options: {
          ...templateLaunchOptions,
          extravars: {
            ...asJsonObject(templateLaunchOptions.extravars),
            ...templateExtraVars,
          },
        },
      },
    });
  };

  const pageHeaderMeta = useMemo(() => {
    switch (tab) {
      case "projects":
        return {
          icon: GitBranch,
          title: t("playbooks.page.projects.title"),
          description: t("playbooks.page.projects.description"),
          badge: t("playbooks.page.badges.projects", { count: projects.length }),
        };
      case "job-templates":
        return {
          icon: Rocket,
          title: t("playbooks.page.jobTemplates.title"),
          description: t("playbooks.page.jobTemplates.description"),
          badge: t("playbooks.page.badges.jobTemplates", {
            count: jobTemplates.length,
          }),
        };
      case "execution-environments":
        return {
          icon: Box,
          title: t("playbooks.page.executionEnvironments.title"),
          description: t("playbooks.page.executionEnvironments.description"),
          badge: t("playbooks.page.badges.executionEnvironments", {
            count: executionEnvironments.length,
          }),
        };
      case "assignments":
        return {
          icon: Route,
          title: t("playbooks.page.assignments.title"),
          description: t("playbooks.page.assignments.description"),
          badge: t("playbooks.page.badges.assignments", {
            count: assignmentsTable.query.data?.total ?? 0,
          }),
        };
      case "target-sets":
        return {
          icon: Layers3,
          title: t("playbooks.page.targetSets.title"),
          description: t("playbooks.page.targetSets.description"),
          badge: t("playbooks.page.badges.targetSets", {
            count: targetSetsTable.query.data?.total ?? 0,
          }),
        };
      case "schedules":
        return {
          icon: CalendarClock,
          title: t("playbooks.page.schedules.title"),
          description: t("playbooks.page.schedules.description"),
          badge: t("playbooks.page.badges.schedules", {
            count: schedulesTable.query.data?.total ?? 0,
          }),
        };
      case "service-principals":
        return {
          icon: KeyRound,
          title: t("playbooks.page.servicePrincipals.title"),
          description: t("playbooks.page.servicePrincipals.description"),
          badge: t("playbooks.page.badges.servicePrincipals", {
            count: servicePrincipalsTable.query.data?.total ?? 0,
          }),
        };
      case "runs":
        return {
          icon: History,
          title: t("playbooks.page.runs.title"),
          description: t("playbooks.page.runs.description"),
          badge: t("playbooks.page.badges.runs", {
            count: runGroupsTable.query.data?.total ?? 0,
          }),
        };
      case "catalog":
      default:
        return {
          icon: ClipboardList,
          title: t("playbooks.page.title"),
          description: t("playbooks.page.description"),
          badge: t("playbooks.page.badges.catalog", { count: playbooks.length }),
        };
    }
  }, [
    assignmentsTable.query.data?.total,
    executionEnvironments.length,
    jobTemplates.length,
    playbooks.length,
    projects.length,
    runGroupsTable.query.data?.total,
    schedulesTable.query.data?.total,
    servicePrincipalsTable.query.data?.total,
    t,
    tab,
    targetSetsTable.query.data?.total,
  ]);

  const selectedRunGroupId =
    runDetail?.target.kind === "group"
      ? runDetail.target.groupId
      : (runDetail?.target.groupId ?? null);
  const canRunFromHeader =
    canExecutePlaybook &&
    runnablePlaybooks.length > 0 &&
    (tab === "catalog" || tab === "runs");
  const headerCreateAction = ((): HeaderCreateAction | null => {
    switch (tab) {
      case "catalog":
        return headerCreateActionIfAllowed(canCreatePlaybook, {
          label: t("playbooks.page.actions.newPlaybook"),
          onClick: openCreatePlaybook,
        });
      case "projects":
        return headerCreateActionIfAllowed(canCreateProject, {
          label: t("playbooks.page.actions.newProject"),
          onClick: openProjectDialog,
        });
      case "job-templates":
        return headerCreateActionIfAllowed(canCreateJobTemplate, {
          label: t("playbooks.page.actions.newJobTemplate"),
          onClick: openJobTemplateDialog,
        });
      case "execution-environments":
        return headerCreateActionIfAllowed(canCreateExecutionEnvironment, {
          label: t("playbooks.page.actions.newExecutionEnvironment"),
          onClick: openExecutionEnvironmentDialog,
        });
      case "assignments":
        return headerCreateActionIfAllowed(canCreateAssignment, {
          label: t("playbooks.page.actions.newAssignment"),
          onClick: () => openAssignmentDialog(parentId ? { playbookId: parentId } : undefined),
        });
      case "target-sets":
        return headerCreateActionIfAllowed(canCreateTargetSet, {
          label: t("playbooks.page.actions.newTargetSet"),
          onClick: openTargetSetDialog,
        });
      case "schedules":
        return headerCreateActionIfAllowed(canCreateSchedule, {
          label: t("playbooks.page.actions.newSchedule"),
          onClick: openScheduleDialog,
        });
      case "service-principals":
        return headerCreateActionIfAllowed(canCreateServicePrincipal, {
          label: t("playbooks.page.actions.newServicePrincipal"),
          onClick: openServicePrincipalDialog,
        });
      case "runs":
      default:
        return null;
    }
  })();

  return (
    <div className={cn("flex min-w-0 flex-col gap-4", tab === "runs" && "h-full min-h-0")}>
      {!parent && <PageChrome
        icon={pageHeaderMeta.icon}
        title={pageHeaderMeta.title}
        description={pageHeaderMeta.description}
        badge={pageHeaderMeta.badge}
      />}
      <PageActions>
            <Button variant="outline" onClick={refreshAll}>
              <RefreshCw className="h-3.5 w-3.5" />
              {t("refresh")}
            </Button>
            {headerCreateAction && (!parent || tab === "assignments" || tab === "schedules") && (
              <Button
                variant={canRunFromHeader ? "outline" : "default"}
                onClick={headerCreateAction.onClick}
              >
                <Plus className="h-3.5 w-3.5" />
                {headerCreateAction.label}
              </Button>
            )}
            {canRunFromHeader ? (
              <Button onClick={() => openRunDialog()}>
                <Play className="h-3.5 w-3.5" />
                {t("playbooks.page.actions.runPlaybook")}
              </Button>
            ) : null}
      </PageActions>

      <div className="flex min-h-0 flex-1 flex-col gap-3">
        {!parent && SETUP_TABS.has(tab) ? (
          <SectionLinks label={t("navItems.automationSetup")} activeValue={tab} items={setupTabOptions.map((item) => ({ ...item, to: PLAYBOOK_TAB_ROUTES[item.value] }))} />
        ) : null}
        <PlaybookTabPanel active={tab === "catalog"}>
          <CatalogPanel
            playbooks={playbooks}
            loading={catalogQuery.isPending}
            targetSetById={targetSetById}
            onRun={(item) => openRunDialog(item.playbook.id)}
            onAssign={(item) => openAssignmentDialog({ playbookId: item.playbook.id })}
            onEdit={openEditPlaybook}
            onVersionHistory={openVersionHistory}
            onUseAsTemplate={openPlaybookTemplateDraft}
            onArchive={(item) => archivePlaybookMutation.mutate(item.playbook.id)}
            onDelete={(item) => openRowDialog(() => setDeletePlaybookItem(item))}
          />
        </PlaybookTabPanel>

        <PlaybookTabPanel active={tab === "projects"}>
          <ProjectsPanel
            projects={projects}
            loading={projectsQuery.isPending}
            error={projectsQuery.isError ? projectsQuery.error : undefined}
            onRetry={() => projectsQuery.refetch()}
            executionEnvironmentById={executionEnvironmentById}
            onCreateJobTemplate={openJobTemplateForProject}
          />
        </PlaybookTabPanel>

        <PlaybookTabPanel active={tab === "job-templates"}>
          <JobTemplatesPanel
            jobTemplates={jobTemplates}
            loading={jobTemplatesQuery.isPending}
            error={jobTemplatesQuery.isError ? jobTemplatesQuery.error : undefined}
            onRetry={() => jobTemplatesQuery.refetch()}
            playbookById={playbookById}
            projectById={projectById}
            targetSetById={targetSetById}
            executionEnvironmentById={executionEnvironmentById}
            onRun={runJobTemplate}
            runPending={runMutation.isPending}
          />
        </PlaybookTabPanel>

        <PlaybookTabPanel active={tab === "execution-environments"}>
          <ExecutionEnvironmentsPanel
            executionEnvironments={executionEnvironments}
            loading={executionEnvironmentsQuery.isPending}
            error={executionEnvironmentsQuery.isError ? executionEnvironmentsQuery.error : undefined}
            onRetry={() => executionEnvironmentsQuery.refetch()}
            onCreateJobTemplate={openJobTemplateForExecutionEnvironment}
          />
        </PlaybookTabPanel>

        <PlaybookTabPanel active={tab === "assignments"}>
          <AssignmentsPanel
            parentScoped={Boolean(parent)}
            assignments={assignments}
            loading={assignmentsTable.query.isPending}
            error={assignmentsTable.query.isError ? assignmentsTable.query.error : undefined}
            onRetry={assignmentsTable.refresh}
            sortState={assignmentsTable.sortState}
            onSortChange={assignmentsTable.setSortState}
            columnFilters={assignmentsTable.columnFilters}
            onColumnFilterChange={assignmentsTable.setColumnFilter}
            pagination={{
              page: assignmentsTable.page,
              pageSize: assignmentsTable.pageSize,
              total: assignmentsTable.query.data?.total ?? 0,
              onPageChange: assignmentsTable.setPage,
              onPageSizeChange: assignmentsTable.setPageSize,
            }}
            playbookById={playbookById}
            targetSetById={targetSetById}
            scopeEntities={scopeEntities}
            onEdit={openEditAssignmentDialog}
            onDelete={(assignment) => openRowDialog(() => setAssignmentDeleteId(assignment.id))}
            onPreview={(assignment) => {
              if (!can("READ", "PLAYBOOK_TARGET_SET")) return;
              setPreviewTargetSetId(assignment.target_set_id);
              if (!parent) setTab("target-sets");
            }}
            onRun={runAssignment}
            runPending={runMutation.isPending}
          />
          {parent && can("READ", "PLAYBOOK_TARGET_SET") && previewTargetSetId && (previewQuery.isError ? <PageState kind="error" title={t("unknownError")} /> : previewQuery.data ? <TargetPreview preview={previewQuery.data} /> : <PageState kind="loading" title={t("loading")} />)}
        </PlaybookTabPanel>

        <PlaybookTabPanel active={tab === "target-sets"}>
          <div className="grid min-h-0 gap-4 xl:grid-cols-[minmax(0,1fr)_minmax(20rem,28rem)]">
            <TargetSetsPanel
              targetSets={targetSets}
              loading={targetSetsTable.query.isPending}
              error={targetSetsTable.query.isError ? targetSetsTable.query.error : undefined}
              onRetry={targetSetsTable.refresh}
              selectedId={previewTargetSetId}
              sortState={targetSetsTable.sortState}
              onSortChange={targetSetsTable.setSortState}
              columnFilters={targetSetsTable.columnFilters}
              onColumnFilterChange={targetSetsTable.setColumnFilter}
              pagination={{
                page: targetSetsTable.page,
                pageSize: targetSetsTable.pageSize,
                total: targetSetsTable.query.data?.total ?? 0,
                onPageChange: targetSetsTable.setPage,
                onPageSizeChange: targetSetsTable.setPageSize,
              }}
              onEdit={openEditTargetSetDialog}
              onDelete={(targetSet) => openRowDialog(() => setTargetSetDeleteId(targetSet.id))}
              onSelect={(targetSet) => setPreviewTargetSetId(targetSet.id)}
            />

            <SurfaceCard className="min-h-0 overflow-hidden">
              <SurfaceCardHeader
                title={<span className="flex items-center gap-2"><Database aria-hidden="true" className="size-4 text-primary" />{t("playbooks.page.preview.title")}</span>}
                description={t("playbooks.page.preview.description")}
              />
              <SurfaceCardContent className="flex flex-col gap-3">
                {previewTargetSetId === null ? (
                  <PageState kind="empty" title={t("playbooks.page.preview.empty")} />
                ) : previewQuery.isPending ? (
                  <PageState kind="loading" title={t("loading")} />
                ) : previewQuery.data ? (
                  <TargetPreview preview={previewQuery.data} />
                ) : (
                  <PageState kind="error" title={t("playbooks.page.preview.failed")} />
                )}
              </SurfaceCardContent>
            </SurfaceCard>
          </div>
        </PlaybookTabPanel>

        <PlaybookTabPanel active={tab === "schedules"}>
          <SchedulesPanel
            parentScoped={Boolean(parent)}
            schedules={schedules}
            loading={schedulesTable.query.isPending}
            error={schedulesTable.query.isError ? schedulesTable.query.error : undefined}
            onRetry={schedulesTable.refresh}
            sortState={schedulesTable.sortState}
            onSortChange={schedulesTable.setSortState}
            columnFilters={schedulesTable.columnFilters}
            onColumnFilterChange={schedulesTable.setColumnFilter}
            pagination={{
              page: schedulesTable.page,
              pageSize: schedulesTable.pageSize,
              total: schedulesTable.query.data?.total ?? 0,
              onPageChange: schedulesTable.setPage,
              onPageSizeChange: schedulesTable.setPageSize,
            }}
            playbookById={playbookById}
            targetSetById={targetSetById}
            servicePrincipalById={servicePrincipalById}
            onEdit={openScheduleEditDialog}
            onToggle={(schedule, enabled) =>
              toggleScheduleMutation.mutate({ id: schedule.id, enabled })
            }
            onDelete={(schedule) => openRowDialog(() => setScheduleDeleteId(schedule.id))}
            togglingId={
              toggleScheduleMutation.isPending
                ? toggleScheduleMutation.variables?.id ?? null
                : null
            }
            onRunNow={runScheduleNow}
            runNowPending={runMutation.isPending}
          />
        </PlaybookTabPanel>

        <PlaybookTabPanel active={tab === "service-principals"}>
          <ServicePrincipalsPanel
            servicePrincipals={servicePrincipals}
            loading={servicePrincipalsTable.query.isPending}
            error={servicePrincipalsTable.query.isError ? servicePrincipalsTable.query.error : undefined}
            onRetry={servicePrincipalsTable.refresh}
            sortState={servicePrincipalsTable.sortState}
            onSortChange={servicePrincipalsTable.setSortState}
            columnFilters={servicePrincipalsTable.columnFilters}
            onColumnFilterChange={servicePrincipalsTable.setColumnFilter}
            pagination={{
              page: servicePrincipalsTable.page,
              pageSize: servicePrincipalsTable.pageSize,
              total: servicePrincipalsTable.query.data?.total ?? 0,
              onPageChange: servicePrincipalsTable.setPage,
              onPageSizeChange: servicePrincipalsTable.setPageSize,
            }}
            onRevoke={(principal) => openRowDialog(() => setRevokeServicePrincipal(principal))}
          />
        </PlaybookTabPanel>

        <PlaybookTabPanel active={tab === "runs"}>
          <div ref={runsContainerRef} className="flex h-full min-h-0 flex-col gap-3">
            {!runsWideMode && runDetail !== null ? (
              <Button
                variant="outline"
                size="sm"
                className="w-fit gap-2"
                onClick={closeRunDetail}
              >
                <ArrowLeft className="h-3.5 w-3.5" />
                {t("playbooks.runDetail.backToRuns")}
              </Button>
            ) : null}
            <div
              className={cn(
                "flex min-h-0 flex-1",
                runsWideMode ? "flex-row" : "flex-col",
              )}
            >
              {runsWideMode || runDetail === null ? (
                <div
                  className={cn(
                    "flex min-h-0 flex-1 flex-col",
                    runsWideMode && "min-w-[360px]",
                  )}
                >
                  <RunsPanel
                    parentScoped={Boolean(parent)}
                    runGroups={runGroups}
                    loading={runGroupsTable.query.isPending}
                    error={runGroupsTable.query.isError ? runGroupsTable.query.error : undefined}
                    onRetry={runGroupsTable.refresh}
                    playbooks={playbooks}
                    playbookById={playbookById}
                    selectedGroupId={selectedRunGroupId}
                    sortState={runGroupsTable.sortState}
                    onSortChange={runGroupsTable.setSortState}
                    columnFilters={runGroupsTable.columnFilters}
                    onColumnFilterChange={runGroupsTable.setColumnFilter}
                    pagination={{
                      page: runGroupsTable.page,
                      pageSize: runGroupsTable.pageSize,
                      total: runGroupsTable.query.data?.total ?? 0,
                      onPageChange: runGroupsTable.setPage,
                      onPageSizeChange: runGroupsTable.setPageSize,
                    }}
                    onOpenGroup={(groupId, playbookName) =>
                      openRunGroupDetail(groupId, playbookName)
                    }
                  />
                </div>
              ) : null}
              {runDetail !== null && runsWideMode ? (
                <PaneResizer
                  orientation="horizontal"
                  size={resolvedRunsDetailWidth}
                  min={380}
                  max={runsDetailMaxWidth}
                  onResize={handleRunsDetailResize}
                  className="mx-1"
                />
              ) : null}
              {runDetail !== null ? (
                <div
                  className={cn("flex min-h-0 flex-col", runsWideMode ? "shrink-0" : "flex-1")}
                  style={runsWideMode ? { width: resolvedRunsDetailWidth } : undefined}
                >
                  <RunDetailPanel
                    target={runDetail.target}
                    playbookName={runDetail.playbookName}
                    compact={!runsWideMode || resolvedRunsDetailWidth < RUNS_DETAIL_COMPACT_MAX_WIDTH}
                    onClose={closeRunDetail}
                  />
                </div>
              ) : null}
            </div>
          </div>
        </PlaybookTabPanel>
      </div>

      {/* Delete playbook confirmation */}
      <DeletePlaybookDialog
        item={deletePlaybookItem}
        pending={deletePlaybookMutation.isPending}
        onOpenChange={(open) => {
          if (!open) setDeletePlaybookItem(null);
        }}
        onConfirm={(item) => deletePlaybookMutation.mutate(item.playbook.id)}
      />

      {/* Revoke service principal confirmation */}
      <RevokeServicePrincipalDialog
        principal={revokeServicePrincipal}
        pending={revokeServicePrincipalMutation.isPending}
        onOpenChange={(open) => {
          if (!open) setRevokeServicePrincipal(null);
        }}
        onConfirm={(principal) => revokeServicePrincipalMutation.mutate(principal.id)}
      />

      {/* Create / edit assignment */}
      <AssignmentDialog
        open={assignmentDialogOpen}
        editing={editingAssignmentId !== null}
        form={assignmentForm}
        onFormChange={setAssignmentForm}
        playbooks={playbooks}
        allTargetSets={allTargetSets}
        targetSetMissing={assignmentTargetSetMissing}
        previewPending={assignmentPreviewQuery.isPending}
        previewError={assignmentPreviewQuery.isError}
        previewTotal={assignmentPreviewQuery.data?.total ?? 0}
        canCreateTargetSet={canCreateTargetSet}
        canReadTargetSet={can("READ", "PLAYBOOK_TARGET_SET")}
        onCreateTargetSet={openTargetSetDialogForAssignment}
        scopeEntities={scopeEntities}
        onOpenChange={handleAssignmentDialogOpenChange}
        onSubmit={submitAssignment}
        submitting={createAssignmentMutation.isPending || updateAssignmentMutation.isPending}
      />

      {/* Delete target-set confirmation */}
      <DeleteTargetSetDialog
        open={targetSetDeleteId !== null}
        name={targetSetToDeleteName}
        pending={deleteTargetSetMutation.isPending}
        onOpenChange={(open) => {
          if (!open) setTargetSetDeleteId(null);
        }}
        onConfirm={() => {
          if (targetSetDeleteId) deleteTargetSetMutation.mutate(targetSetDeleteId);
        }}
      />

      {/* Delete assignment confirmation */}
      <DeleteAssignmentDialog
        open={assignmentDeleteId !== null}
        name={assignmentToDeleteName}
        pending={deleteAssignmentMutation.isPending}
        onOpenChange={(open) => {
          if (!open) setAssignmentDeleteId(null);
        }}
        onConfirm={() => {
          if (assignmentDeleteId) deleteAssignmentMutation.mutate(assignmentDeleteId);
        }}
      />

      <PlaybookLaunchDialog
        open={launchIntent !== null}
        intent={launchIntent}
        onOpenChange={(open) => {
          if (!open) {
            setLaunchIntent(null);
            setRunDialogOpen(false);
          }
        }}
        onConfirm={(intent) => runMutation.mutate(intent)}
        isSubmitting={runMutation.isPending}
        playbookOptions={runDialogOpen ? runnablePlaybooks : undefined}
        selectedPlaybookId={runDialogOpen ? runDialogPlaybookId : undefined}
        onSelectedPlaybookIdChange={runDialogOpen ? selectRunPlaybook : undefined}
        playbooksLoading={catalogQuery.isPending}
        targeting={
          runDialogOpen
            ? {
                mode: runTargetMode,
                onModeChange: setRunTargetMode,
                targetSets: allTargetSets,
                selectedTargetSetId: effectiveRunTargetSetId,
                onSelectedTargetSetIdChange: setRunTargetSetId,
                hostOptions: connectionTargets,
                selectedHosts: runHostSelection,
                onSelectedHostsChange: setRunHostSelection,
                hostsLoading: connectionTargetsQuery.isPending,
              }
            : undefined
        }
      />

      {/* Delete schedule confirmation */}
      <DeleteScheduleDialog
        open={scheduleDeleteId !== null}
        name={scheduleToDelete?.name ?? ""}
        pending={deleteScheduleMutation.isPending}
        onOpenChange={(open) => {
          if (!open) setScheduleDeleteId(null);
        }}
        onConfirm={() => {
          if (scheduleDeleteId) deleteScheduleMutation.mutate(scheduleDeleteId);
        }}
      />


    </div>
  );
};

export default PlaybooksPage;
