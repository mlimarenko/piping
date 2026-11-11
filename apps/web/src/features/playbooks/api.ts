import { ApiError, apiClient, unwrap } from "@/shared/api/client";
import type { components } from "@/shared/api/schema.gen";

export type PlaybookAssignmentScopeKind = components["schemas"]["PlaybookAssignmentScopeKind"];
export type PlaybookAssignment = components["schemas"]["PlaybookAssignment"];
export type ListPlaybookAssignmentsResponse = components["schemas"]["ListPlaybookAssignmentsResponse"];
export type CreatePlaybookAssignmentRequest = components["schemas"]["CreatePlaybookAssignmentRequest"];
export type UpdatePlaybookAssignmentRequest = components["schemas"]["UpdatePlaybookAssignmentRequest"];
export type PlaybookTargetSet = components["schemas"]["PlaybookTargetSet"];
export type PlaybookTargetSetListItem = components["schemas"]["PlaybookTargetSetListItem"];
export type PlaybookTargetSetUsage = components["schemas"]["PlaybookTargetSetUsage"];
export type PlaybookTargetSetUsageRef = components["schemas"]["PlaybookTargetSetUsageRef"];
export type ListPlaybookTargetSetsResponse = components["schemas"]["ListPlaybookTargetSetsResponse"];
export type CreateTargetSetRequest = components["schemas"]["CreateTargetSetRequest"];
export type PlaybookTargetSetPreview = components["schemas"]["PlaybookTargetSetPreview"];
export type PlaybookTargetSelectorPreview = components["schemas"]["PlaybookTargetSelectorPreview"];

export async function previewTargetSelector(selector: Record<string, unknown>, signal?: AbortSignal): Promise<PlaybookTargetSelectorPreview> {
  return unwrap(await apiClient.POST("/api/v1/playbooks/target-sets/preview", {
    body: { selector, limit: 50 }, signal,
  }));
}
export type TargetSetFacet = components["schemas"]["TargetSetFacet"];
export type TargetSetFacetKind = components["schemas"]["TargetSetFacetKind"];
export type TargetSetFacetPage = components["schemas"]["TargetSetFacetPage"];
export type PlaybookTargetPreviewItem = components["schemas"]["PlaybookTargetPreviewItem"];
export type PlaybookRunGroupCreated = components["schemas"]["PlaybookRunGroupCreated"];
export type PlaybookRunGroup = components["schemas"]["PlaybookRunGroup"];
export type PlaybookRun = components["schemas"]["PlaybookRun"];
export type PlaybookRunStatus = components["schemas"]["PlaybookRunStatus"];
export type PlaybookRunEvent = components["schemas"]["PlaybookRunEvent"];
export type PlaybookRunEventKind = components["schemas"]["PlaybookRunEventKind"];
export type PlaybookArtifact = components["schemas"]["PlaybookArtifact"];
export type PlaybookSchedule = components["schemas"]["PlaybookSchedule"];
export type ListSchedulesResponse = components["schemas"]["ListSchedulesResponse"];
export type CreateScheduleRequest = components["schemas"]["CreateScheduleRequest"];
export type UpdateScheduleRequest = components["schemas"]["UpdateScheduleRequest"];
export type PlaybookRunGroupDetail = components["schemas"]["PlaybookRunGroupDetail"];
export type ListPlaybookRunGroupsResponse = components["schemas"]["ListPlaybookRunGroupsResponse"];
export type ListPlaybookRunsResponse = components["schemas"]["ListPlaybookRunsResponse"];
export type ListPlaybookRunEventsResponse = components["schemas"]["ListPlaybookRunEventsResponse"];
export type ListPlaybookRunArtifactsResponse = components["schemas"]["ListPlaybookRunArtifactsResponse"];
export type PlaybookScheduleConcurrencyPolicy = components["schemas"]["PlaybookScheduleConcurrencyPolicy"];
export type PlaybookScheduleMisfirePolicy = components["schemas"]["PlaybookScheduleMisfirePolicy"];
export type PlaybookTargetSetMode = components["schemas"]["PlaybookTargetSetMode"];
export type StaticTargetMemberRequest = components["schemas"]["StaticTargetMemberRequest"];
export type PlaybookTargetAssignments = components["schemas"]["PlaybookTargetAssignments"];
export type PlaybookTargetAssignment = components["schemas"]["PlaybookTargetAssignment"];
export type JobTemplateVarsSchema = components["schemas"]["JobTemplateVarsSchema"];
export type VarsSchemaSource = components["schemas"]["VarsSchemaSource"];

export async function getJobTemplateVarsSchema(id: string): Promise<JobTemplateVarsSchema> {
  return unwrap(await apiClient.GET("/api/v1/playbooks/job-templates/{id}/vars-schema", { params: { path: { id } } }));
}
export type PlaybookExecutionEnvironment = components["schemas"]["PlaybookExecutionEnvironment"];
export type ListExecutionEnvironmentsResponse = components["schemas"]["ListExecutionEnvironmentsResponse"];
export type CreateExecutionEnvironmentRequest = components["schemas"]["CreateExecutionEnvironmentRequest"];
export type UpdateExecutionEnvironmentRequest = components["schemas"]["UpdateExecutionEnvironmentRequest"];
export type Project = components["schemas"]["Project"];
export type ProjectRevision = components["schemas"]["ProjectRevision"];
export type ProjectEntry = components["schemas"]["ProjectEntry"];
export type ProjectEntryContent = components["schemas"]["ProjectEntryContent"];
export type ProjectSource = components["schemas"]["ProjectSource"];
export type ProjectAuth = components["schemas"]["ProjectAuth"];
export type RevisionChange = components["schemas"]["RevisionChange"];
export type ListProjectsResponse = components["schemas"]["ListProjectsResponse"];
export type CreateProjectRequest = components["schemas"]["CreateProjectRequest"];
export type UpdateProjectRequest = components["schemas"]["UpdateProjectRequest"];
export type ChangeProjectRevisionRequest = components["schemas"]["ChangeProjectRevisionRequest"];
export type ForkProjectRequest = components["schemas"]["ForkProjectRequest"];
export type SyncProjectRequest = components["schemas"]["SyncProjectRequest"];
export type JobTemplate = components["schemas"]["JobTemplate"];
export type RevisionSelection = components["schemas"]["RevisionSelection"];
export type InventorySelection = components["schemas"]["InventorySelection"];
export type LaunchOptions = components["schemas"]["LaunchOptions"];
export type ScheduleTargets = components["schemas"]["ScheduleTargets"];
export type ResolvedTarget = components["schemas"]["ResolvedTarget"];
export type ListJobTemplatesResponse = components["schemas"]["ListJobTemplatesResponse"];
export type CreateJobTemplateRequest = components["schemas"]["CreateJobTemplateRequest"];
export type UpdateJobTemplateRequest = components["schemas"]["UpdateJobTemplateRequest"];
export type LaunchJobTemplateRequest = components["schemas"]["LaunchJobTemplateRequest"];
export type ConsolePage = components["schemas"]["ConsolePageSchema"];

export const TARGET_PLAYBOOK_ASSIGNMENTS_LIMIT = 20;

export async function listPlaybookAssignments(
  params: {
    job_template_id?: string;
    target_set_id?: string;
    scope_kind?: string;
    enabled?: string;
    quick_action_enabled?: string;
    limit?: number;
    offset?: number;
    sort?: string;
    order?: "asc" | "desc";
  } = {},
): Promise<ListPlaybookAssignmentsResponse> {
  return unwrap(
    await apiClient.GET("/api/v1/playbooks/assignments", {
      params: {
        query: {
          job_template_id: params.job_template_id,
          target_set_id: params.target_set_id,
          scope_kind: params.scope_kind,
          enabled: params.enabled,
          quick_action_enabled: params.quick_action_enabled,
          limit: params.limit ?? 100,
          offset: params.offset ?? 0,
          sort: params.sort,
          order: params.order,
        },
      },
    }),
  );
}

export async function createPlaybookAssignment(
  body: CreatePlaybookAssignmentRequest,
): Promise<PlaybookAssignment> {
  return unwrap(await apiClient.POST("/api/v1/playbooks/assignments", { body }));
}

export async function getPlaybookAssignment(id: string): Promise<PlaybookAssignment> {
  return unwrap(
    await apiClient.GET("/api/v1/playbooks/assignments/{id}", {
      params: { path: { id } },
    }),
  );
}

export async function updatePlaybookAssignment(
  id: string,
  body: UpdatePlaybookAssignmentRequest,
): Promise<PlaybookAssignment> {
  return unwrap(
    await apiClient.PATCH("/api/v1/playbooks/assignments/{id}", {
      params: { path: { id } },
      body,
    }),
  );
}

export async function deletePlaybookAssignment(id: string): Promise<void> {
  await apiClient.DELETE("/api/v1/playbooks/assignments/{id}", {
    params: { path: { id } },
  });
}

export async function listPlaybookTargetSets(
  params: {
    mode?: string;
    search?: string;
    limit?: number;
    offset?: number;
    sort?: string;
    order?: "asc" | "desc";
  } = {},
): Promise<ListPlaybookTargetSetsResponse> {
  return unwrap(
    await apiClient.GET("/api/v1/playbooks/target-sets", {
      params: {
        query: {
          mode: params.mode,
          search: params.search,
          limit: params.limit ?? 100,
          offset: params.offset ?? 0,
          sort: params.sort,
          order: params.order,
        },
      },
    }),
  );
}

export async function createPlaybookTargetSet(
  body: CreateTargetSetRequest,
): Promise<PlaybookTargetSetPreview> {
  return unwrap(await apiClient.POST("/api/v1/playbooks/target-sets", { body }));
}

export async function updateTargetSet(
  id: string,
  body: CreateTargetSetRequest,
): Promise<PlaybookTargetSet> {
  return unwrap(
    await apiClient.PUT("/api/v1/playbooks/target-sets/{id}", {
      params: { path: { id } },
      body,
    }),
  );
}

export async function deletePlaybookTargetSet(id: string): Promise<void> {
  await apiClient.DELETE("/api/v1/playbooks/target-sets/{id}", {
    params: { path: { id } },
  });
}

export async function getTargetSet(id: string): Promise<PlaybookTargetSet> {
  return unwrap(
    await apiClient.GET("/api/v1/playbooks/target-sets/{id}", {
      params: { path: { id } },
    }),
  );
}

export async function previewPlaybookTargetSet(
  id: string,
  limit = 50,
): Promise<PlaybookTargetSetPreview> {
  return unwrap(
    await apiClient.GET("/api/v1/playbooks/target-sets/{id}/preview", {
      params: { path: { id }, query: { limit } },
    }),
  );
}

export async function listTargetSetFacets(
  kind: TargetSetFacetKind,
  selector: Record<string, unknown>,
  search: string,
  offset = 0,
  limit = 50,
): Promise<TargetSetFacetPage> {
  return unwrap(await apiClient.GET("/api/v1/playbooks/target-sets/facets", {
    params: { query: { kind, selector: JSON.stringify(selector), search, limit, offset } },
  }));
}

export async function launchJobTemplate(
  id: string,
  body: LaunchJobTemplateRequest = {},
): Promise<PlaybookRunGroupCreated> {
  return unwrap(
    await apiClient.POST("/api/v1/playbooks/job-templates/{id}/launch", {
      params: { path: { id } },
      body,
    }),
  );
}

export async function getTargetPlaybookAssignments(
  targetId: string,
  limit = TARGET_PLAYBOOK_ASSIGNMENTS_LIMIT,
  signal?: AbortSignal,
): Promise<PlaybookTargetAssignments> {
  return unwrap(
    await apiClient.GET("/api/v1/playbooks/targets/{target_id}/assignments", {
      params: {
        path: { target_id: targetId },
        query: { limit },
      },
      signal,
    }),
  );
}

export async function listPlaybookSchedules(
  params: {
    template_id?: string;
    enabled?: string;
    limit?: number;
    offset?: number;
  } = {},
): Promise<ListSchedulesResponse> {
  return unwrap(
    await apiClient.GET("/api/v1/playbooks/schedules", {
      params: {
        query: {
          template_id: params.template_id,
          enabled: params.enabled,
          limit: params.limit ?? 100,
          offset: params.offset ?? 0,
        },
      },
    }),
  );
}

export async function listPlaybookRunEvents(
  runId: string,
  params: { limit?: number; offset?: number } = {},
): Promise<ListPlaybookRunEventsResponse> {
  return unwrap(
    await apiClient.GET("/api/v1/playbooks/runs/{run_id}/events", {
      params: {
        path: { run_id: runId },
        query: { limit: params.limit ?? 200, offset: params.offset ?? 0 },
      },
    }),
  );
}

export async function listPlaybookRunGroupEvents(
  groupId: string,
  params: {
    runId?: string | null;
    host?: string | null;
    afterSequence?: number | null;
    limit?: number;
    offset?: number;
  } = {},
): Promise<ListPlaybookRunEventsResponse> {
  return unwrap(
    await apiClient.GET("/api/v1/playbooks/run-groups/{group_id}/events", {
      params: {
        path: { group_id: groupId },
        query: {
          run_id: params.runId ?? undefined,
          host: params.host ?? undefined,
          after_sequence: params.afterSequence ?? undefined,
          limit: params.limit ?? 1000,
          offset: params.offset ?? 0,
        },
      },
    }),
  );
}

export async function listPlaybookRunArtifacts(
  runId: string,
  params: { limit?: number; offset?: number } = {},
): Promise<ListPlaybookRunArtifactsResponse> {
  return unwrap(
    await apiClient.GET("/api/v1/playbooks/runs/{run_id}/artifacts", {
      params: {
        path: { run_id: runId },
        query: { limit: params.limit ?? 100, offset: params.offset ?? 0 },
      },
    }),
  );
}

export async function createPlaybookSchedule(
  body: CreateScheduleRequest,
): Promise<PlaybookSchedule> {
  return unwrap(await apiClient.POST("/api/v1/playbooks/schedules", { body }));
}

export async function updatePlaybookSchedule(
  id: string,
  body: UpdateScheduleRequest,
): Promise<PlaybookSchedule> {
  return unwrap(
    await apiClient.PATCH("/api/v1/playbooks/schedules/{id}", {
      params: { path: { id } },
      body,
    }),
  );
}

export async function deletePlaybookSchedule(id: string): Promise<void> {
  await apiClient.DELETE("/api/v1/playbooks/schedules/{id}", {
    params: { path: { id } },
  });
}

export async function getSchedule(id: string): Promise<PlaybookSchedule> {
  return unwrap(
    await apiClient.GET("/api/v1/playbooks/schedules/{id}", {
      params: { path: { id } },
    }),
  );
}

export async function listExecutionEnvironments(
  params: { limit?: number; offset?: number } = {},
): Promise<ListExecutionEnvironmentsResponse> {
  return unwrap(
    await apiClient.GET("/api/v1/playbooks/execution-environments", {
      params: { query: { limit: params.limit ?? 100, offset: params.offset ?? 0 } },
    }),
  );
}

export async function createExecutionEnvironment(
  body: CreateExecutionEnvironmentRequest,
): Promise<PlaybookExecutionEnvironment> {
  return unwrap(
    await apiClient.POST("/api/v1/playbooks/execution-environments", { body }),
  );
}

export async function getExecutionEnvironment(
  id: string,
): Promise<PlaybookExecutionEnvironment> {
  return unwrap(
    await apiClient.GET("/api/v1/playbooks/execution-environments/{id}", {
      params: { path: { id } },
    }),
  );
}

export async function updateExecutionEnvironment(id: string, body: UpdateExecutionEnvironmentRequest): Promise<PlaybookExecutionEnvironment> {
  return unwrap(await apiClient.PATCH("/api/v1/playbooks/execution-environments/{id}", { params: { path: { id } }, body }));
}

export async function deleteExecutionEnvironment(id: string): Promise<void> {
  await apiClient.DELETE("/api/v1/playbooks/execution-environments/{id}", { params: { path: { id } } });
}

export async function listProjects(
  params: { limit?: number; offset?: number } = {},
): Promise<ListProjectsResponse> {
  return unwrap(
    await apiClient.GET("/api/v1/playbooks/projects", {
      params: { query: { limit: params.limit ?? 100, offset: params.offset ?? 0 } },
    }),
  );
}

export async function createProject(
  body: CreateProjectRequest,
): Promise<Project> {
  return unwrap(await apiClient.POST("/api/v1/playbooks/projects", { body }));
}

export async function getProject(id: string): Promise<Project> {
  return unwrap(
    await apiClient.GET("/api/v1/playbooks/projects/{id}", {
      params: { path: { id } },
    }),
  );
}

export async function updateProject(id: string, body: UpdateProjectRequest): Promise<Project> {
  return unwrap(await apiClient.PATCH("/api/v1/playbooks/projects/{id}", { params: { path: { id } }, body }));
}

export async function deleteProject(id: string): Promise<void> {
  await apiClient.DELETE("/api/v1/playbooks/projects/{id}", { params: { path: { id } } });
}

export async function getProjectRevision(id: string, revisionId: string): Promise<ProjectRevision> {
  return unwrap(
    await apiClient.GET("/api/v1/playbooks/projects/{id}/revisions/{revision_id}", {
      params: { path: { id, revision_id: revisionId } },
    }),
  );
}

export async function changeProjectRevision(id: string, body: ChangeProjectRevisionRequest): Promise<ProjectRevision> {
  return unwrap(await apiClient.POST("/api/v1/playbooks/projects/{id}/revisions", { params: { path: { id } }, body }));
}

export async function forkProject(id: string, body: ForkProjectRequest): Promise<Project> {
  return unwrap(await apiClient.POST("/api/v1/playbooks/projects/{id}/fork", { params: { path: { id } }, body }));
}

export async function syncProject(id: string, body: SyncProjectRequest): Promise<ProjectRevision> {
  return unwrap(await apiClient.POST("/api/v1/playbooks/projects/{id}/sync", { params: { path: { id } }, body }));
}

export async function listJobTemplates(
  params: { limit?: number; offset?: number } = {},
): Promise<ListJobTemplatesResponse> {
  return unwrap(
    await apiClient.GET("/api/v1/playbooks/job-templates", {
      params: { query: { limit: params.limit ?? 100, offset: params.offset ?? 0 } },
    }),
  );
}

export async function createJobTemplate(
  body: CreateJobTemplateRequest,
): Promise<JobTemplate> {
  return unwrap(await apiClient.POST("/api/v1/playbooks/job-templates", { body }));
}

export async function getJobTemplate(id: string): Promise<JobTemplate> {
  return unwrap(
    await apiClient.GET("/api/v1/playbooks/job-templates/{id}", {
      params: { path: { id } },
    }),
  );
}

export async function updateJobTemplate(id: string, body: UpdateJobTemplateRequest): Promise<JobTemplate> {
  return unwrap(await apiClient.PATCH("/api/v1/playbooks/job-templates/{id}", { params: { path: { id } }, body }));
}

export async function deleteJobTemplate(id: string): Promise<void> {
  await apiClient.DELETE("/api/v1/playbooks/job-templates/{id}", { params: { path: { id } } });
}

export async function listPlaybookRuns(
  params: {
    template_id?: string | null;
    status?: string | null;
    limit?: number;
    offset?: number;
    sort?: string;
    order?: "asc" | "desc";
  } = {},
): Promise<ListPlaybookRunsResponse> {
  return unwrap(
    await apiClient.GET("/api/v1/playbooks/runs", {
      params: {
        query: {
          template_id: params.template_id ?? undefined,
          status: params.status ?? undefined,
          limit: params.limit ?? 100,
          offset: params.offset ?? 0,
          sort: params.sort,
          order: params.order,
        },
      },
    }),
  );
}

export async function listPlaybookRunGroups(
  params: {
    template_id?: string | null;
    status?: string | null;
    limit?: number;
    offset?: number;
    sort?: string;
    order?: "asc" | "desc";
  } = {},
): Promise<ListPlaybookRunGroupsResponse> {
  return unwrap(
    await apiClient.GET("/api/v1/playbooks/run-groups", {
      params: {
        query: {
          template_id: params.template_id ?? undefined,
          status: params.status ?? undefined,
          limit: params.limit ?? 100,
          offset: params.offset ?? 0,
          sort: params.sort,
          order: params.order,
        },
      },
    }),
  );
}

export async function getPlaybookRun(runId: string): Promise<PlaybookRun> {
  return unwrap(
    await apiClient.GET("/api/v1/playbooks/runs/{run_id}", {
      params: { path: { run_id: runId } },
    }),
  );
}

export async function getPlaybookRunGroup(
  id: string,
  limit = 50_000,
): Promise<PlaybookRunGroupDetail> {
  return unwrap(
    await apiClient.GET("/api/v1/playbooks/run-groups/{id}", {
      params: { path: { id }, query: { limit } },
    }),
  );
}

/** Inclusive durable replay. The console model also validates the byte page at runtime. */
export async function getRunGroupConsole(
  groupId: string,
  options: { fromSequence: number; dispatchId?: string; signal: AbortSignal },
): Promise<ConsolePage> {
  if (!groupId || !Number.isSafeInteger(options.fromSequence) || options.fromSequence < 0 || options.dispatchId === "") {
    throw new ApiError({ status: 0, message: "Invalid console replay request" });
  }
  return unwrap(await apiClient.GET("/api/v1/playbooks/run-groups/{group_id}/console", {
    params: { path: { group_id: groupId }, query: { from_sequence: options.fromSequence, limit: 64, dispatch_id: options.dispatchId } },
    signal: options.signal,
  }));
}

/**
 * Request cancellation of a single run. Dispatched work remains nonterminal
 * until the runner confirms it stopped or the cancellation deadline expires.
 */
export async function cancelPlaybookRun(runId: string): Promise<PlaybookRun> {
  return unwrap(
    await apiClient.POST("/api/v1/playbooks/runs/{run_id}/cancel", {
      params: { path: { run_id: runId } },
    }),
  );
}

/** Request cancellation of every active run in a run group ("STOP all"). */
export async function cancelPlaybookRunGroup(
  groupId: string,
): Promise<PlaybookRunGroupDetail> {
  return unwrap(
    await apiClient.POST("/api/v1/playbooks/run-groups/{group_id}/cancel", {
      params: { path: { group_id: groupId } },
    }),
  );
}

/**
 * Same-origin `ws(s)://` URL for a run's live event stream
 * (`GET /api/v1/ws/playbooks/runs/{run_id}`). Cookie-authed like the SSH
 * terminal socket — the browser attaches the session cookie automatically, so
 * no token query is needed.
 */
export function playbookRunStreamUrl(runId: string): string {
  const { protocol, host } = window.location;
  const wsProtocol = protocol === "https:" ? "wss:" : "ws:";
  return `${wsProtocol}//${host}/api/v1/ws/playbooks/runs/${encodeURIComponent(runId)}`;
}
