import { apiClient, unwrap } from "@/shared/api/client";
import type { components } from "@/shared/api/schema.gen";

export type PlaybookCatalogItem = components["schemas"]["PlaybookCatalogItem"];
export type PlaybookConnection = components["schemas"]["PlaybookConnection"];
export type PlaybookRisk = components["schemas"]["PlaybookRisk"];
export type PlaybookStatus = components["schemas"]["PlaybookStatus"];
export type PlaybookAssignmentScopeKind =
  components["schemas"]["PlaybookAssignmentScopeKind"];
export type ListPlaybooksResponse = components["schemas"]["ListPlaybooksResponse"];
export type CreatePlaybookRequest = components["schemas"]["CreatePlaybookRequest"];
export type UpdatePlaybookRequest = components["schemas"]["UpdatePlaybookRequest"];
export type PlaybookAssignment = components["schemas"]["PlaybookAssignment"];
export type ListPlaybookAssignmentsResponse =
  components["schemas"]["ListPlaybookAssignmentsResponse"];
export type CreatePlaybookAssignmentRequest =
  components["schemas"]["CreatePlaybookAssignmentRequest"];
export type UpdatePlaybookAssignmentRequest =
  components["schemas"]["UpdatePlaybookAssignmentRequest"];
export type PlaybookTargetSet = components["schemas"]["PlaybookTargetSet"];
export type ListPlaybookTargetSetsResponse =
  components["schemas"]["ListPlaybookTargetSetsResponse"];
export type CreateTargetSetRequest = components["schemas"]["CreateTargetSetRequest"];
export type PlaybookTargetSetPreview = components["schemas"]["PlaybookTargetSetPreview"];
export type PlaybookTargetPreviewItem = components["schemas"]["PlaybookTargetPreviewItem"];
export type CreateRunGroupRequest = components["schemas"]["CreateRunGroupRequest"];
export type PlaybookRunGroupCreated = components["schemas"]["PlaybookRunGroupCreated"];
export type PlaybookRunGroup = components["schemas"]["PlaybookRunGroup"];
export type PlaybookRun = components["schemas"]["PlaybookRun"];
export type PlaybookRunStatus = components["schemas"]["PlaybookRunStatus"];
export type PlaybookRunEvent = components["schemas"]["PlaybookRunEvent"];
export type PlaybookRunEventKind = components["schemas"]["PlaybookRunEventKind"];
export type PlaybookArtifact = components["schemas"]["PlaybookArtifact"];
export type PlaybookSchedule = components["schemas"]["PlaybookSchedule"];
export type ListPlaybookSchedulesResponse =
  components["schemas"]["ListPlaybookSchedulesResponse"];
export type CreatePlaybookScheduleRequest =
  components["schemas"]["CreatePlaybookScheduleRequest"];
export type UpdatePlaybookScheduleRequest =
  components["schemas"]["UpdatePlaybookScheduleRequest"];
export type PlaybookServicePrincipal =
  components["schemas"]["PlaybookServicePrincipal"];
export type ListPlaybookServicePrincipalsResponse =
  components["schemas"]["ListPlaybookServicePrincipalsResponse"];
export type CreatePlaybookServicePrincipalRequest =
  components["schemas"]["CreatePlaybookServicePrincipalRequest"];
export type RevokePlaybookServicePrincipalRequest =
  components["schemas"]["RevokePlaybookServicePrincipalRequest"];
export type PlaybookVersion = components["schemas"]["PlaybookVersion"];
export type ListPlaybookVersionsResponse =
  components["schemas"]["ListPlaybookVersionsResponse"];
export type CreatePlaybookVersionRequest =
  components["schemas"]["CreatePlaybookVersionRequest"];
export type PlaybookRunGroupDetail = components["schemas"]["PlaybookRunGroupDetail"];
export type ListPlaybookRunGroupsResponse =
  components["schemas"]["ListPlaybookRunGroupsResponse"];
export type ListPlaybookRunsResponse = components["schemas"]["ListPlaybookRunsResponse"];
export type ListPlaybookRunEventsResponse =
  components["schemas"]["ListPlaybookRunEventsResponse"];
export type ListPlaybookRunArtifactsResponse =
  components["schemas"]["ListPlaybookRunArtifactsResponse"];
export type PlaybookScheduleConcurrencyPolicy =
  components["schemas"]["PlaybookScheduleConcurrencyPolicy"];
export type PlaybookScheduleMisfirePolicy =
  components["schemas"]["PlaybookScheduleMisfirePolicy"];
export type PlaybookTargetSetMode = components["schemas"]["PlaybookTargetSetMode"];
export type StaticTargetMemberRequest =
  components["schemas"]["StaticTargetMemberRequest"];
export type PlaybookTargetAssignments =
  components["schemas"]["PlaybookTargetAssignments"];
export type PlaybookTargetAssignment =
  components["schemas"]["PlaybookTargetAssignment"];
export type PlaybookExecutionEnvironment =
  components["schemas"]["PlaybookExecutionEnvironment"];
export type ListExecutionEnvironmentsResponse =
  components["schemas"]["ListExecutionEnvironmentsResponse"];
export type CreateExecutionEnvironmentRequest =
  components["schemas"]["CreateExecutionEnvironmentRequest"];
export type PlaybookProject = components["schemas"]["PlaybookProject"];
export type ListProjectsResponse = components["schemas"]["ListProjectsResponse"];
export type CreateProjectRequest = components["schemas"]["CreateProjectRequest"];
export type ProjectFileResponse = components["schemas"]["ProjectFileResponse"];
export type ProjectFilesResponse = components["schemas"]["ProjectFilesResponse"];
export type UploadProjectFileRequest =
  components["schemas"]["UploadProjectFileRequest"];
export type PlaybookJobTemplate = components["schemas"]["PlaybookJobTemplate"];
export type ListJobTemplatesResponse =
  components["schemas"]["ListJobTemplatesResponse"];
export type CreateJobTemplateRequest =
  components["schemas"]["CreateJobTemplateRequest"];

export const TARGET_PLAYBOOK_ASSIGNMENTS_LIMIT = 20;

export async function listPlaybooks(
  params: { limit?: number; offset?: number } = {},
): Promise<ListPlaybooksResponse> {
  return unwrap(
    await apiClient.GET("/api/v1/playbooks", {
      params: { query: { limit: params.limit ?? 100, offset: params.offset ?? 0 } },
    }),
  );
}

export async function createPlaybook(
  body: CreatePlaybookRequest,
): Promise<PlaybookCatalogItem> {
  return unwrap(await apiClient.POST("/api/v1/playbooks", { body }));
}

export async function updatePlaybook(
  id: string,
  body: UpdatePlaybookRequest,
): Promise<PlaybookCatalogItem> {
  return unwrap(
    await apiClient.PATCH("/api/v1/playbooks/{id}", {
      params: { path: { id } },
      body,
    }),
  );
}

export async function deletePlaybook(id: string): Promise<void> {
  await apiClient.DELETE("/api/v1/playbooks/{id}", {
    params: { path: { id } },
  });
}

export async function getPlaybook(id: string): Promise<PlaybookCatalogItem> {
  return unwrap(
    await apiClient.GET("/api/v1/playbooks/{id}", {
      params: { path: { id } },
    }),
  );
}

export async function listPlaybookVersions(
  id: string,
  params: { limit?: number; offset?: number } = {},
): Promise<ListPlaybookVersionsResponse> {
  return unwrap(
    await apiClient.GET("/api/v1/playbooks/{id}/versions", {
      params: {
        path: { id },
        query: { limit: params.limit ?? 100, offset: params.offset ?? 0 },
      },
    }),
  );
}

export async function createPlaybookVersion(
  id: string,
  body: CreatePlaybookVersionRequest,
): Promise<PlaybookVersion> {
  return unwrap(
    await apiClient.POST("/api/v1/playbooks/{id}/versions", {
      params: { path: { id } },
      body,
    }),
  );
}

export async function listPlaybookAssignments(
  params: {
    playbook_id?: string;
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
          playbook_id: params.playbook_id,
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

export async function createPlaybookRunGroup(
  playbookId: string,
  body: CreateRunGroupRequest,
): Promise<PlaybookRunGroupCreated> {
  return unwrap(
    await apiClient.POST("/api/v1/playbooks/{id}/runs", {
      params: { path: { id: playbookId } },
      body,
    }),
  );
}

export async function launchJobTemplate(
  id: string,
  body: CreateRunGroupRequest,
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
): Promise<PlaybookTargetAssignments> {
  return unwrap(
    await apiClient.GET("/api/v1/playbooks/targets/{target_id}/assignments", {
      params: {
        path: { target_id: targetId },
        query: { limit },
      },
    }),
  );
}

export async function listPlaybookSchedules(
  params: {
    playbook_id?: string;
    target_set_id?: string;
    concurrency_policy?: string;
    enabled?: string;
    limit?: number;
    offset?: number;
    sort?: string;
    order?: "asc" | "desc";
  } = {},
): Promise<ListPlaybookSchedulesResponse> {
  return unwrap(
    await apiClient.GET("/api/v1/playbooks/schedules", {
      params: {
        query: {
          playbook_id: params.playbook_id,
          target_set_id: params.target_set_id,
          concurrency_policy: params.concurrency_policy,
          enabled: params.enabled,
          limit: params.limit ?? 100,
          offset: params.offset ?? 0,
          sort: params.sort,
          order: params.order,
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
  body: CreatePlaybookScheduleRequest,
): Promise<PlaybookSchedule> {
  return unwrap(await apiClient.POST("/api/v1/playbooks/schedules", { body }));
}

export async function updatePlaybookSchedule(
  id: string,
  body: UpdatePlaybookScheduleRequest,
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

export async function listPlaybookServicePrincipals(
  params: {
    status?: string;
    limit?: number;
    offset?: number;
    sort?: string;
    order?: "asc" | "desc";
  } = {},
): Promise<ListPlaybookServicePrincipalsResponse> {
  return unwrap(
    await apiClient.GET("/api/v1/playbooks/service-principals", {
      params: {
        query: {
          status: params.status,
          limit: params.limit ?? 100,
          offset: params.offset ?? 0,
          sort: params.sort,
          order: params.order,
        },
      },
    }),
  );
}

export async function createPlaybookServicePrincipal(
  body: CreatePlaybookServicePrincipalRequest,
): Promise<PlaybookServicePrincipal> {
  return unwrap(
    await apiClient.POST("/api/v1/playbooks/service-principals", { body }),
  );
}

export async function getPlaybookServicePrincipal(
  id: string,
): Promise<PlaybookServicePrincipal> {
  return unwrap(
    await apiClient.GET("/api/v1/playbooks/service-principals/{id}", {
      params: { path: { id } },
    }),
  );
}

export async function revokePlaybookServicePrincipal(
  id: string,
  body: RevokePlaybookServicePrincipalRequest = {},
): Promise<PlaybookServicePrincipal> {
  return unwrap(
    await apiClient.POST("/api/v1/playbooks/service-principals/{id}/revoke", {
      params: { path: { id } },
      body,
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
): Promise<PlaybookProject> {
  return unwrap(await apiClient.POST("/api/v1/playbooks/projects", { body }));
}

export async function getProject(id: string): Promise<PlaybookProject> {
  return unwrap(
    await apiClient.GET("/api/v1/playbooks/projects/{id}", {
      params: { path: { id } },
    }),
  );
}

/** Stored files of a manual project; always empty for SCM-backed projects. */
export async function getProjectFiles(id: string): Promise<ProjectFilesResponse> {
  return unwrap(
    await apiClient.GET("/api/v1/playbooks/projects/{id}/files", {
      params: { path: { id } },
    }),
  );
}

/**
 * Replace the whole stored file set of a manual project. The backend rewrites
 * `metadata.project_files` from `files`; this is the only writer.
 */
export async function uploadProjectFiles(
  id: string,
  files: UploadProjectFileRequest[],
): Promise<PlaybookProject> {
  return unwrap(
    await apiClient.POST("/api/v1/playbooks/projects/{id}/files", {
      params: { path: { id } },
      body: { files },
    }),
  );
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
): Promise<PlaybookJobTemplate> {
  return unwrap(await apiClient.POST("/api/v1/playbooks/job-templates", { body }));
}

export async function getJobTemplate(id: string): Promise<PlaybookJobTemplate> {
  return unwrap(
    await apiClient.GET("/api/v1/playbooks/job-templates/{id}", {
      params: { path: { id } },
    }),
  );
}

export async function listPlaybookRuns(
  params: {
    playbookId?: string | null;
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
          playbook_id: params.playbookId ?? undefined,
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
    playbook_id?: string | null;
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
          playbook_id: params.playbook_id ?? undefined,
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
