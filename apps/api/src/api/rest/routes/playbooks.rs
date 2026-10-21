//! REST handlers for Linux playbook catalog management.

use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::api::access_control_helpers::build_policy_evaluation_service;
use crate::api::connection_authz_attributes::{
    batch_connection_target_is_allowed, connection_target_authz_attributes_for,
    resolve_connection_target_facets_batch,
};
use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::api::rest::routes::validation::require_bulk_items;
use crate::app::AppState;
use crate::contexts::access_control::domain::{
    Action, Policy, PolicyDecision, ResourceType, compile_connection_target_visibility,
    evaluate_policies_with_metadata,
};
use crate::contexts::audit::domain::AuditEvent;
use crate::contexts::playbooks::application::playbook_service::PlaybookRunGroupEventListFilter;
use crate::contexts::playbooks::application::{
    CompleteRunGroupBatchInput, CompleteRunGroupBatchItemInput, CompleteRunInput,
    CreateArtifactInput, CreateAssignmentInput, CreateExecutionEnvironmentInput,
    CreateJobTemplateInput, CreatePlaybookInput, CreatePlaybookVersionInput, CreateProjectInput,
    CreateRunEventInput, CreateRunGroupEventInput, CreateRunGroupInput, CreateScheduleInput,
    CreateServicePrincipalInput, CreateTargetSetInput, LaunchJobTemplateInput, PlaybookCatalogItem,
    PlaybookRunCompletion, PlaybookRunGroupCompletionBatch, PlaybookRunGroupCreated,
    PlaybookRunGroupDetail, PlaybookTargetAssignments, RevokeServicePrincipalInput,
    RunAuthorizationTargets, RunStreamFrame, StaticTargetMemberInput, UpdateAssignmentInput,
    UpdateExecutionEnvironmentInput, UpdateJobTemplateInput, UpdatePlaybookInput,
    UpdateProjectInput, UpdateScheduleInput, explicit_run_credential_reference,
    job_template_credential_references, parse_job_token, runtime_policy_from_token_policy,
    secret_use_allowed, secret_use_pat_allows,
};
use crate::contexts::playbooks::domain::{
    PlaybookArtifact, PlaybookAssignment, PlaybookAssignmentScopeKind, PlaybookConnection,
    PlaybookExecutionEnvironment, PlaybookJobTemplate, PlaybookLanguage, PlaybookProject,
    PlaybookRisk, PlaybookRun, PlaybookRunEvent, PlaybookRunEventKind, PlaybookRunGroup,
    PlaybookRunStatus, PlaybookRunTriggerKind, PlaybookRuntime, PlaybookSchedule,
    PlaybookScheduleConcurrencyPolicy, PlaybookScheduleMisfirePolicy, PlaybookServicePrincipal,
    PlaybookStatus, PlaybookTargetSet, PlaybookTargetSetMode, PlaybookTargetSetPreview,
    PlaybookVersion, parse_assignment_scope_kind, parse_run_status,
    parse_schedule_concurrency_policy, parse_service_principal_status, parse_target_set_mode,
};
use crate::contexts::playbooks::infrastructure::AnsibleProjectFile;
use crate::contexts::playbooks::infrastructure::persistence::{
    AppendRunGroupEventBatchOutcome, PLAYBOOK_ASSIGNMENT_COLUMNS, PLAYBOOK_RUN_COLUMNS,
    PLAYBOOK_RUN_GROUP_COLUMNS, PLAYBOOK_SCHEDULE_COLUMNS, PLAYBOOK_SERVICE_PRINCIPAL_COLUMNS,
    PLAYBOOK_TARGET_SET_COLUMNS, PlaybookAssignmentListFilter, PlaybookRunListFilter,
    PlaybookScheduleListFilter, PlaybookServicePrincipalListFilter, PlaybookTargetSetListFilter,
};
use crate::contexts::secrets::application::{
    PLAYBOOK_VERSION_RUNTIME_ENV_CONSUMER_TYPE, SecretStore, resolve_secret_visibility,
};
use crate::contexts::secrets::domain::{SECRET_KIND_NAMED, SecretUsageBinding};
use crate::contexts::secrets::infrastructure::SecretRepository;
use crate::contexts::shared::list_query::{SortSpec, parse_csv_uuids};
use crate::contexts::shared::pagination::normalize_page;
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;
use crate::observability::{EventFields, log_error_event};

/// Member-run cap when fanning a run-group cancel out to its runs. Matches the
/// run fan-out ceiling so every member of even the largest group is stamped.
const PLAYBOOK_RUN_CANCEL_MEMBER_LIMIT: i64 = 50_000;

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct ListPlaybooksQuery {
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct ListPlaybookVersionsQuery {
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct ListPlaybookAssignmentsQuery {
    /// Comma-separated playbook IDs; matches any (`IN`).
    pub playbook_id: Option<String>,
    /// Comma-separated target-set IDs; matches any (`IN`).
    pub target_set_id: Option<String>,
    /// Comma-separated assignment scope kinds; matches any (`IN`).
    pub scope_kind: Option<String>,
    /// Comma-separated booleans; matches any (`IN`).
    pub enabled: Option<String>,
    /// Comma-separated booleans; matches any (`IN`).
    pub quick_action_enabled: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    /// Logical sort column key (allowlisted; unknown keys are rejected).
    pub sort: Option<String>,
    /// Sort direction: `asc` (default) or `desc`.
    pub order: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct ListPlaybookTargetSetsQuery {
    /// Comma-separated target-set modes (`dynamic`/`static`); matches any (`IN`).
    pub mode: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    /// Logical sort column key (allowlisted; unknown keys are rejected).
    pub sort: Option<String>,
    /// Sort direction: `asc` (default) or `desc`.
    pub order: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct ListPlaybookSchedulesQuery {
    /// Comma-separated playbook IDs; matches any (`IN`).
    pub playbook_id: Option<String>,
    /// Comma-separated target-set IDs; matches any (`IN`).
    pub target_set_id: Option<String>,
    /// Comma-separated concurrency policies; matches any (`IN`).
    pub concurrency_policy: Option<String>,
    /// Comma-separated enabled flags (`true`/`false`); matches any (`IN`).
    pub enabled: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    /// Logical sort column key (allowlisted; unknown keys are rejected).
    pub sort: Option<String>,
    /// Sort direction: `asc` (default) or `desc`.
    pub order: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct ListPlaybookTargetAssignmentsQuery {
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct ListPlaybookRunItemsQuery {
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct ListPlaybookRunGroupEventsQuery {
    pub run_id: Option<Uuid>,
    pub host: Option<String>,
    pub after_sequence: Option<i64>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreatePlaybookRequest {
    pub slug: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub status: Option<PlaybookStatus>,
    pub runtime: Option<PlaybookRuntime>,
    pub risk: Option<PlaybookRisk>,
    pub owner_user_id: Option<Uuid>,
    pub category: Option<String>,
    pub vars_schema: Option<Value>,
    pub connection_default: Option<PlaybookConnection>,
    pub language: Option<PlaybookLanguage>,
    pub entrypoint: Option<String>,
    pub content: String,
    pub input_schema: Option<Value>,
    pub output_schema: Option<Value>,
    pub token_policy: Option<Value>,
    pub timeout_seconds: Option<i32>,
    pub changelog: Option<String>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreatePlaybookVersionRequest {
    pub language: Option<PlaybookLanguage>,
    pub runtime: Option<PlaybookRuntime>,
    pub entrypoint: Option<String>,
    pub content: String,
    pub input_schema: Option<Value>,
    pub output_schema: Option<Value>,
    pub token_policy: Option<Value>,
    pub timeout_seconds: Option<i32>,
    pub changelog: Option<String>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateExecutionEnvironmentRequest {
    pub name: String,
    pub description: Option<String>,
    pub image: String,
    pub pull_policy: Option<String>,
    pub runner_group: Option<String>,
    pub enabled: Option<bool>,
    pub env: Option<Value>,
    pub settings: Option<Value>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateProjectRequest {
    pub name: String,
    pub description: Option<String>,
    pub scm_type: Option<String>,
    pub scm_url: Option<String>,
    pub scm_branch: Option<String>,
    pub scm_ref: Option<String>,
    pub playbook_base_path: Option<String>,
    pub credential_id: Option<Uuid>,
    pub default_execution_environment_id: Option<Uuid>,
    pub sync_policy: Option<String>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateJobTemplateRequest {
    pub name: String,
    pub description: Option<String>,
    pub playbook_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub playbook_path: Option<String>,
    pub inventory_target_set_id: Option<Uuid>,
    pub execution_environment_id: Option<Uuid>,
    pub credential_refs: Option<Value>,
    pub extra_vars: Option<Value>,
    pub launch_options: Option<Value>,
    pub ask_variables_on_launch: Option<bool>,
    pub ask_limit_on_launch: Option<bool>,
    pub ask_tags_on_launch: Option<bool>,
    pub ask_skip_tags_on_launch: Option<bool>,
    pub ask_credential_on_launch: Option<bool>,
    pub ask_execution_environment_on_launch: Option<bool>,
    pub enabled: Option<bool>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateExecutionEnvironmentRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<Option<String>>,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub pull_policy: Option<String>,
    #[serde(default)]
    pub runner_group: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub env: Option<Value>,
    #[serde(default)]
    pub settings: Option<Value>,
    #[serde(default)]
    pub metadata: Option<Value>,
}

/// Deserialize a nullable patch field so an absent key and an explicit JSON
/// `null` stay distinguishable.
///
/// A plain `Option<Option<T>>` collapses both cases to `None`: serde treats a
/// present `null` as the outer `None`. With `#[serde(default)]` the field is
/// then `None` whether the client omitted it or sent `null`, so an advertised
/// nullable "clear" (`Some(None)`) was unreachable. Calling this on the field
/// only when the key is present returns `Some(inner)`, keeping `None` for an
/// absent key.
fn deserialize_nullable_patch<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Some(Option::<T>::deserialize(deserializer)?))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateProjectRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<Option<String>>,
    #[serde(default)]
    pub scm_url: Option<Option<String>>,
    #[serde(default)]
    pub scm_branch: Option<Option<String>>,
    #[serde(default)]
    pub scm_ref: Option<Option<String>>,
    #[serde(default)]
    pub playbook_base_path: Option<Option<String>>,
    // Only this field distinguishes an absent key from an explicit `null`: the
    // secret binding is the one field here whose clearing is both advertised by
    // the contract and load-bearing, since a stale binding keeps its secret
    // undeletable. Widening the same treatment to the neighbouring fields would
    // silently turn a `null` they currently ignore into a wipe, which belongs to
    // whoever owns those fields, not to PWA-53.
    #[serde(default, deserialize_with = "deserialize_nullable_patch")]
    pub credential_id: Option<Option<Uuid>>,
    #[serde(default)]
    pub default_execution_environment_id: Option<Option<Uuid>>,
    #[serde(default)]
    pub sync_policy: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateJobTemplateRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<Option<String>>,
    #[serde(default)]
    pub inventory_target_set_id: Option<Option<Uuid>>,
    #[serde(default)]
    pub execution_environment_id: Option<Option<Uuid>>,
    #[serde(default)]
    pub credential_refs: Option<Value>,
    #[serde(default)]
    pub extra_vars: Option<Value>,
    #[serde(default)]
    pub launch_options: Option<Value>,
    #[serde(default)]
    pub ask_variables_on_launch: Option<bool>,
    #[serde(default)]
    pub ask_limit_on_launch: Option<bool>,
    #[serde(default)]
    pub ask_tags_on_launch: Option<bool>,
    #[serde(default)]
    pub ask_skip_tags_on_launch: Option<bool>,
    #[serde(default)]
    pub ask_credential_on_launch: Option<bool>,
    #[serde(default)]
    pub ask_execution_environment_on_launch: Option<bool>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub metadata: Option<Value>,
}

/// One manual-project file in an upload payload. Paths are relative POSIX paths;
/// absolute paths, parent traversal, `ansible.cfg` and the runner-reserved
/// `.piping_keys/` directory are rejected server-side.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UploadProjectFileRequest {
    pub path: String,
    pub content: String,
    #[serde(default)]
    pub executable: Option<bool>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UploadProjectFilesRequest {
    #[schema(min_items = 1)]
    pub files: Vec<UploadProjectFileRequest>,
}

impl UploadProjectFilesRequest {
    /// A project-files upload must carry at least one file. An empty set is a
    /// client mistake (a no-op overwrite), so it is a 400.
    pub(crate) fn validate(&self) -> Result<(), AppError> {
        require_bulk_items(&self.files)?;
        Ok(())
    }
}

/// One stored manual-project file, read back from `metadata.project_files`.
/// Mirrors `UploadProjectFileRequest` on the read path; `executable` is always
/// a concrete boolean because it is persisted with a default.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ProjectFileResponse {
    pub path: String,
    pub content: String,
    pub executable: bool,
}

/// Response body of `GET /projects/{id}/files`.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ProjectFilesResponse {
    pub files: Vec<ProjectFileResponse>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdatePlaybookRequest {
    pub slug: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub status: Option<PlaybookStatus>,
    pub runtime: Option<PlaybookRuntime>,
    pub risk: Option<PlaybookRisk>,
    pub owner_user_id: Option<Option<Uuid>>,
    pub default_target_set_id: Option<Option<Uuid>>,
    pub category: Option<String>,
    pub vars_schema: Option<Option<Value>>,
    pub connection_default: Option<PlaybookConnection>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct StaticTargetMemberRequest {
    pub target_kind: String,
    pub target_id: Uuid,
    pub source_kind: String,
    pub source_id: Option<Uuid>,
    pub sort_order: Option<i32>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateTargetSetRequest {
    pub name: String,
    pub mode: PlaybookTargetSetMode,
    pub selector: Option<Value>,
    pub members: Option<Vec<StaticTargetMemberRequest>>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreatePlaybookAssignmentRequest {
    pub playbook_id: Uuid,
    pub target_set_id: Uuid,
    pub default_version_id: Option<Uuid>,
    pub name: Option<String>,
    pub scope_kind: PlaybookAssignmentScopeKind,
    pub scope_id: Option<Uuid>,
    pub priority: Option<i32>,
    pub enabled: Option<bool>,
    pub visible: Option<bool>,
    pub executable: Option<bool>,
    pub quick_action_enabled: Option<bool>,
    pub default_input: Option<Value>,
    pub default_metadata: Option<Value>,
    pub default_timeout_seconds: Option<i32>,
    pub default_target_limit: Option<i32>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdatePlaybookAssignmentRequest {
    pub playbook_id: Option<Uuid>,
    pub target_set_id: Option<Uuid>,
    pub default_version_id: Option<Option<Uuid>>,
    pub name: Option<Option<String>>,
    pub scope_kind: Option<PlaybookAssignmentScopeKind>,
    pub scope_id: Option<Option<Uuid>>,
    pub priority: Option<i32>,
    pub enabled: Option<bool>,
    pub visible: Option<bool>,
    pub executable: Option<bool>,
    pub quick_action_enabled: Option<bool>,
    pub default_input: Option<Value>,
    pub default_metadata: Option<Value>,
    pub default_timeout_seconds: Option<Option<i32>>,
    pub default_target_limit: Option<Option<i32>>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreatePlaybookScheduleRequest {
    pub playbook_id: Uuid,
    pub target_set_id: Uuid,
    pub service_principal_id: Uuid,
    pub name: String,
    pub cron_expression: String,
    pub timezone: Option<String>,
    pub misfire_policy: Option<PlaybookScheduleMisfirePolicy>,
    pub concurrency_policy: Option<PlaybookScheduleConcurrencyPolicy>,
    pub enabled: Option<bool>,
    pub next_run_at: Option<DateTime<Utc>>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdatePlaybookScheduleRequest {
    pub target_set_id: Option<Uuid>,
    pub name: Option<String>,
    pub cron_expression: Option<String>,
    pub timezone: Option<String>,
    pub misfire_policy: Option<PlaybookScheduleMisfirePolicy>,
    pub concurrency_policy: Option<PlaybookScheduleConcurrencyPolicy>,
    pub enabled: Option<bool>,
    pub next_run_at: Option<DateTime<Utc>>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreatePlaybookServicePrincipalRequest {
    pub name: String,
    pub slug: Option<String>,
    pub description: Option<String>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct RevokePlaybookServicePrincipalRequest {
    pub revocation_reason: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct ListPlaybookServicePrincipalsQuery {
    /// Comma-separated statuses; matches any (`IN`).
    pub status: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    /// Logical sort column key (allowlisted; unknown keys are rejected).
    pub sort: Option<String>,
    /// Sort direction: `asc` (default) or `desc`.
    pub order: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct ListPlaybookRunsQuery {
    /// Comma-separated playbook IDs; matches any (`IN`).
    pub playbook_id: Option<String>,
    /// Comma-separated run statuses; matches any (`IN`).
    pub status: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    /// Logical sort column key (allowlisted; unknown keys are rejected).
    pub sort: Option<String>,
    /// Sort direction: `asc` (default) or `desc`.
    pub order: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct ListPlaybookRunGroupsQuery {
    /// Comma-separated playbook IDs; matches any (`IN`).
    pub playbook_id: Option<String>,
    /// Comma-separated run statuses; matches any (`IN`).
    pub status: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    /// Logical sort column key (allowlisted; unknown keys are rejected).
    pub sort: Option<String>,
    /// Sort direction: `asc` (default) or `desc`.
    pub order: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct GetPlaybookRunGroupQuery {
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateRunGroupRequest {
    pub target_set_id: Option<Uuid>,
    pub trigger_kind: Option<PlaybookRunTriggerKind>,
    pub target_limit: Option<i64>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateArtifactRequest {
    pub artifact_type: String,
    pub name: String,
    pub content_type: Option<String>,
    pub content_json: Option<Value>,
    pub content_text: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateRunEventRequest {
    pub event_kind: PlaybookRunEventKind,
    pub event_type: String,
    pub message: Option<String>,
    pub payload: Option<Value>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateRunGroupEventRequest {
    pub run_id: Option<Uuid>,
    pub host: Option<String>,
    pub event_kind: PlaybookRunEventKind,
    pub event_type: String,
    pub message: Option<String>,
    pub payload: Option<Value>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateRunGroupEventsBatchRequest {
    /// Runner-assigned idempotency key: the backend ingests a given
    /// `(run_group_id, batch_id)` at most once so a retried delivery appends no
    /// duplicate events.
    pub batch_id: Uuid,
    #[schema(min_items = 1)]
    pub events: Vec<CreateRunGroupEventRequest>,
}

impl CreateRunGroupEventsBatchRequest {
    /// An event batch must carry at least one event; an empty batch is a 400
    /// rather than an idempotent-looking no-op `200`.
    pub(crate) fn validate(&self) -> Result<(), AppError> {
        require_bulk_items(&self.events)?;
        Ok(())
    }
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CompleteRunRequest {
    pub lease_owner: String,
    pub status: PlaybookRunStatus,
    pub exit_code: Option<i32>,
    pub error_message: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CompleteRunGroupBatchItemRequest {
    pub run_id: Uuid,
    pub status: PlaybookRunStatus,
    pub exit_code: Option<i32>,
    pub error_message: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CompleteRunGroupBatchRequest {
    pub lease_owner: String,
    #[schema(min_items = 1)]
    pub runs: Vec<CompleteRunGroupBatchItemRequest>,
    /// `known_hosts` lines the runner observed for the first time during this
    /// job (trust-on-first-use harvest). Absent when the runner learned no new
    /// keys.
    #[serde(default)]
    pub new_host_keys: Vec<String>,
}

impl CompleteRunGroupBatchRequest {
    /// A completion batch must carry at least one run; an empty batch is a 400
    /// rather than a no-op `200`.
    pub(crate) fn validate(&self) -> Result<(), AppError> {
        require_bulk_items(&self.runs)?;
        Ok(())
    }
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct TargetSetPreviewQuery {
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ListPlaybooksResponse {
    pub items: Vec<PlaybookCatalogItem>,
    pub total: u64,
    pub limit: u32,
    pub offset: u32,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ListPlaybookVersionsResponse {
    pub items: Vec<PlaybookVersion>,
    pub total: u64,
    pub limit: u32,
    pub offset: u32,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ListExecutionEnvironmentsResponse {
    pub items: Vec<PlaybookExecutionEnvironment>,
    pub total: u64,
    pub limit: u32,
    pub offset: u32,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ListProjectsResponse {
    pub items: Vec<PlaybookProject>,
    pub total: u64,
    pub limit: u32,
    pub offset: u32,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ListJobTemplatesResponse {
    pub items: Vec<PlaybookJobTemplate>,
    pub total: u64,
    pub limit: u32,
    pub offset: u32,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ListPlaybookAssignmentsResponse {
    pub items: Vec<PlaybookAssignment>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ListPlaybookTargetSetsResponse {
    pub items: Vec<PlaybookTargetSet>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ListPlaybookSchedulesResponse {
    pub items: Vec<PlaybookSchedule>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ListPlaybookServicePrincipalsResponse {
    pub items: Vec<PlaybookServicePrincipal>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ListPlaybookRunsResponse {
    pub items: Vec<PlaybookRun>,
    pub total: u64,
    pub limit: u32,
    pub offset: u32,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ListPlaybookRunGroupsResponse {
    pub items: Vec<PlaybookRunGroup>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ListPlaybookRunArtifactsResponse {
    pub items: Vec<PlaybookArtifact>,
    pub total: u64,
    pub limit: u32,
    pub offset: u32,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ListPlaybookRunEventsResponse {
    pub items: Vec<PlaybookRunEvent>,
    pub total: u64,
    pub limit: u32,
    pub offset: u32,
}

/// Request body for the duplicate playbook endpoint.
/// All fields are optional; omitting `name` generates a default "(copy)" suffix.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct DuplicatePlaybookRequest {
    pub name: Option<String>,
}

fn request_context(
    request_id: &RequestId,
    auth_context: Option<Extension<AuthContext>>,
    headers: &HeaderMap,
) -> RequestContext {
    build_request_context(request_id, auth_context, headers)
}

fn bearer_token(headers: &HeaderMap) -> Result<String, AppError> {
    let value = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| AppError::Auth("TOKEN_INVALID".to_string()))?;
    let (scheme, token) = value
        .split_once(' ')
        .ok_or_else(|| AppError::Auth("TOKEN_INVALID".to_string()))?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return Err(AppError::Auth("TOKEN_INVALID".to_string()));
    }
    let token = token.trim();
    if token.is_empty() {
        return Err(AppError::Auth("TOKEN_INVALID".to_string()));
    }
    Ok(token.to_string())
}

fn request_actor_id(ctx: &RequestContext) -> Option<Uuid> {
    ctx.user_id
}

fn request_audit_metadata(ctx: &RequestContext) -> Value {
    json!({
        "actor_kind": "user",
        "auth_method": ctx.auth_method,
        "api_token_id": ctx.api_token_id,
        "request_ip": ctx.request_ip,
        "correlation_id": ctx.correlation_id,
    })
}

fn merge_metadata(base: Value, extra: Value) -> Value {
    let mut base = base.as_object().cloned().unwrap_or_default();
    if let Some(extra) = extra.as_object() {
        for (key, value) in extra {
            base.insert(key.clone(), value.clone());
        }
    }
    Value::Object(base)
}

async fn record_playbook_audit(
    state: &AppState,
    request_context: &RequestContext,
    event_type: &str,
    target_type: &str,
    target_id: Option<Uuid>,
    metadata: Value,
) -> Result<(), AppError> {
    let metadata = merge_metadata(request_audit_metadata(request_context), metadata);
    state
        .services
        .audit
        .record_event(AuditEvent {
            id: Uuid::now_v7(),
            event_type: event_type.to_string(),
            actor_id: request_actor_id(request_context),
            actor_kind: "user".to_string(),
            target_type: Some(target_type.to_string()),
            target_id,
            metadata,
            created_at: Utc::now(),
        })
        .await
}

/// Record an audit row for a sidecar job-token callback. Best-effort by
/// design: every caller runs AFTER the callback payload was durably persisted,
/// so surfacing an audit-write failure as a 5xx would only make the sidecar
/// retry an already-applied mutation and duplicate run events/completions. The
/// failure is logged for operators instead of failing the callback.
async fn record_playbook_job_token_audit(
    state: &AppState,
    event_type: &str,
    target_type: &str,
    target_id: Option<Uuid>,
    raw_token: &str,
    metadata: Value,
) {
    let token_id = parse_job_token(raw_token).map(|(token_id, _)| token_id);
    let metadata = merge_metadata(
        json!({
            "actor_kind": "playbook_job_token",
            "auth_method": "playbook_job_token",
            "job_token_id": token_id.map(|value| value.to_string()),
        }),
        metadata,
    );
    let recorded = state
        .services
        .audit
        .record_event(AuditEvent {
            id: Uuid::now_v7(),
            event_type: event_type.to_string(),
            // A playbook job token is NOT an identity_user, so it must not
            // populate the actor_id FK (audit_event.actor_id -> identity_user.id).
            // Writing token_id here FK-violates and 400s the entire sidecar
            // callback, silently dropping the run's ansible events. The token
            // identity is captured via actor_kind + metadata.job_token_id above.
            actor_id: None,
            actor_kind: "playbook_job_token".to_string(),
            target_type: Some(target_type.to_string()),
            target_id,
            metadata,
            created_at: Utc::now(),
        })
        .await;
    if let Err(error) = recorded {
        log_error_event(
            "playbooks.callback.audit_failed",
            "playbook_callback_audit_error",
            error.to_string(),
            EventFields::default()
                .with_operation("playbook_callback_audit")
                .with_status("failed")
                .with_context("audit_event_type", event_type.to_string()),
        );
    }
}

async fn ensure_access(
    state: &AppState,
    request_context: &RequestContext,
    action: Action,
    resource_id: Option<&str>,
) -> Result<(), AppError> {
    crate::api::permission_helpers::ensure_access_with_resource(
        state,
        request_context,
        action,
        ResourceType::Playbook,
        resource_id,
    )
    .await
}

/// Canonical resource id for a specific playbook (`playbook:{uuid}`).
fn playbook_resource_id(playbook_id: Uuid) -> String {
    format!("playbook:{playbook_id}")
}

/// Canonical resource id for a playbook category (`playbook:cat:{category}`).
fn playbook_category_resource_id(category: &str) -> String {
    format!("playbook:cat:{category}")
}

/// Parse a comma-separated list of booleans into a deduplicated vector.
/// Empty/whitespace entries are skipped; anything but `true`/`false`
/// (case-insensitive) is a client error.
fn parse_csv_bools(raw: Option<&str>) -> Result<Vec<bool>, AppError> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let mut out: Vec<bool> = Vec::new();
    for token in raw.split(',') {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value = match trimmed.to_ascii_lowercase().as_str() {
            "true" => true,
            "false" => false,
            _ => {
                return Err(AppError::Validation(format!(
                    "Invalid enabled value {trimmed:?}: expected true or false"
                )));
            }
        };
        if !out.contains(&value) {
            out.push(value);
        }
    }
    Ok(out)
}

/// Parse a comma-separated list of assignment scope kinds into a deduplicated
/// vector. Empty/whitespace entries are skipped; invalid kinds are client errors.
fn parse_csv_assignment_scope_kinds(
    raw: Option<&str>,
) -> Result<Vec<PlaybookAssignmentScopeKind>, AppError> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let mut out: Vec<PlaybookAssignmentScopeKind> = Vec::new();
    for token in raw.split(',') {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            continue;
        }
        let kind = parse_assignment_scope_kind(trimmed)
            .map_err(|_| AppError::Validation(format!("Invalid scope_kind {trimmed:?}")))?;
        if !out.contains(&kind) {
            out.push(kind);
        }
    }
    Ok(out)
}

/// Parse a comma-separated list of target-set modes into a deduplicated vector.
/// Empty/whitespace entries are skipped; an invalid mode is a client error.
fn parse_csv_target_set_modes(raw: Option<&str>) -> Result<Vec<PlaybookTargetSetMode>, AppError> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let mut out: Vec<PlaybookTargetSetMode> = Vec::new();
    for token in raw.split(',') {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mode = parse_target_set_mode(trimmed)
            .map_err(|_| AppError::Validation(format!("Invalid mode {trimmed:?}")))?;
        if !out.contains(&mode) {
            out.push(mode);
        }
    }
    Ok(out)
}

/// Parse a comma-separated list of schedule concurrency policies.
fn parse_csv_schedule_concurrency_policies(
    raw: Option<&str>,
) -> Result<Vec<PlaybookScheduleConcurrencyPolicy>, AppError> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let mut out: Vec<PlaybookScheduleConcurrencyPolicy> = Vec::new();
    for token in raw.split(',') {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            continue;
        }
        let policy = parse_schedule_concurrency_policy(trimmed)
            .map_err(|_| AppError::Validation(format!("Invalid concurrency_policy {trimmed:?}")))?;
        if !out.contains(&policy) {
            out.push(policy);
        }
    }
    Ok(out)
}

/// Parse a comma-separated list of run statuses into a deduplicated vector.
/// Empty/whitespace entries are skipped; an invalid status is a client error.
fn parse_csv_run_statuses(raw: Option<&str>) -> Result<Vec<PlaybookRunStatus>, AppError> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let mut out: Vec<PlaybookRunStatus> = Vec::new();
    for token in raw.split(',') {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            continue;
        }
        let status = parse_run_status(trimmed)
            .map_err(|_| AppError::Validation(format!("Invalid status {trimmed:?}")))?;
        if !out.contains(&status) {
            out.push(status);
        }
    }
    Ok(out)
}

/// Parse a comma-separated list of service-principal statuses into a
/// deduplicated vector. Empty/whitespace entries are skipped; an invalid status
/// is a client error.
fn parse_csv_service_principal_statuses(
    raw: Option<&str>,
) -> Result<Vec<crate::contexts::playbooks::domain::PlaybookServicePrincipalStatus>, AppError> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for token in raw.split(',') {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            continue;
        }
        let status = parse_service_principal_status(trimmed)
            .map_err(|_| AppError::Validation(format!("Invalid status {trimmed:?}")))?;
        if !out.contains(&status) {
            out.push(status);
        }
    }
    Ok(out)
}

/// Enforce the composite right to RUN a playbook on the target fan-out
/// (legs (a) general run right + (b) this-playbook-OR-its-category), with an
/// ownership short-circuit and full deny-override.
///
/// This is the playbook portion of the 3-way AND ("general run right" AND
/// "this playbook OR its category" AND "host access"). The per-host
/// `ConnectionTarget` leg (c) is enforced separately and unconditionally by the
/// caller (`create_run_group`) and MUST NOT be folded in here — the ownership
/// short-circuit narrows ONLY the per-playbook leg, never host access.
///
/// Evaluation order (deny-override preserved across the OR legs):
/// 1. DENY VETO FIRST: an explicit `Deny` matching EITHER candidate resource id
///    (`playbook:{id}` or `playbook:cat:{category}`) vetoes the run, even if the
///    other candidate (or ownership) would allow. This is the engine's
///    deny-override semantics extended over the OR. We detect an *explicit* deny
///    (vs the no-match default-deny) via `check_access_with_trace`'s `matched_deny`
///    — `ensure_access` alone cannot distinguish the two (both surface as
///    `PolicyDecision::Deny`).
/// 2. OWNERSHIP: if no explicit deny vetoes, the owner may run their own playbook
///    (per-playbook leg only). The PAT token-scope ceiling is applied FIRST so an
///    owner-authenticated API token cannot exceed its declared scope (the allow
///    legs get this gate from `ensure_access`; the owner path returns before them,
///    so it is enforced explicitly here).
/// 3. ALLOW LEGS: otherwise allow when EITHER `Execute`/`Playbook` on
///    `playbook:{id}` (a `resources=["*"]` general policy matches this, satisfying
///    leg (a)) OR `Execute`/`Playbook` on `playbook:cat:{category}` is allowed.
///    These legs go through `ensure_access` so the PAT token-scope gate
///    (`api_token_scope_allows`) is preserved — `check_access_with_trace` does NOT
///    apply that gate, so it is used ONLY for the deny veto, never to grant.
async fn ensure_playbook_run_grant(
    state: &AppState,
    request_context: &RequestContext,
    caller: Uuid,
    playbook_id: Uuid,
    category: &str,
    owner_user_id: Option<Uuid>,
) -> Result<(), AppError> {
    let id_resource = playbook_resource_id(playbook_id);
    let category_resource = playbook_category_resource_id(category);

    // (1) Deny veto: an explicit Deny on either candidate id wins over every Allow
    // leg AND over ownership. `matched_deny` is non-empty only for an explicit Deny
    // policy match (the no-match default-deny leaves it empty), so this vetoes the
    // "category Allow but explicit playbook Deny => denied" case without rejecting
    // a legitimate category-only grant.
    for candidate in [id_resource.as_str(), category_resource.as_str()] {
        let trace = state
            .services
            .access_control
            .check_access_with_trace(
                request_context,
                Action::Execute,
                ResourceType::Playbook,
                Some(candidate),
                None,
            )
            .await?;
        if !trace.matched_deny.is_empty() {
            return Err(AppError::Forbidden(
                "playbook run grant: explicit deny on playbook or category".to_string(),
            ));
        }
    }

    // PAT token-scope ceiling: the ownership short-circuit below must NOT bypass the
    // caller's API-token scope. `ensure_access` applies this gate on its allow legs,
    // but the owner path returns before reaching them — so enforce it here too, with
    // the same semantics (`api_token_scope_allows` is a no-op for non-token auth).
    if request_context.auth_method == "api_token"
        && !request_context.allows_api_token_scope_for_resource(
            "execute",
            ResourceType::Playbook.as_str(),
            Some(id_resource.as_str()),
        )
    {
        return Err(AppError::Forbidden(
            "TOKEN_SCOPE_DENIED:token scope does not permit this action".to_string(),
        ));
    }

    // (2) Ownership short-circuit: an owner can run their own playbook (per-playbook
    // leg only — host access is still enforced by the caller). Reached only when no
    // explicit deny vetoed above and the token scope permits the action.
    if owner_user_id == Some(caller) {
        return Ok(());
    }

    // (3) Allow legs via `ensure_access` (preserves the PAT token-scope gate). Leg
    // (a)+(b)-specific: Execute on the canonical playbook id (a `*`-resourced
    // general policy matches this, covering the general run right and the per-id
    // grant in one check). Then the leg (b)-category fallback.
    if ensure_access(
        state,
        request_context,
        Action::Execute,
        Some(id_resource.as_str()),
    )
    .await
    .is_ok()
    {
        return Ok(());
    }
    ensure_access(
        state,
        request_context,
        Action::Execute,
        Some(category_resource.as_str()),
    )
    .await
}

/// One preflight row: whether the caller may export a declared secret into a run.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct SecretPreflightEntry {
    pub env_name: String,
    pub key: String,
    /// `None` when the declared key resolves to no secret (broken declaration).
    pub secret_id: Option<Uuid>,
    /// Whether a value is stored for the secret (UX signal; not an authz input).
    pub has_value: bool,
    /// Whether this caller may export the secret into a run.
    pub allowed: bool,
}

/// Preflight response: per-secret verdicts plus the launch-blocking aggregate.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct SecretPreflightResponse {
    pub secrets: Vec<SecretPreflightEntry>,
    pub all_allowed: bool,
}

/// The runtime secrets (`ENV_NAME → secret_key`) an effective playbook version
/// declares, read from the pinned/effective version's `token_policy`. Empty when
/// the playbook has no version or declares no runtime secrets.
fn runtime_secret_env_for_version(
    version: Option<&PlaybookVersion>,
) -> Result<std::collections::BTreeMap<String, String>, AppError> {
    match version {
        Some(version) => Ok(runtime_policy_from_token_policy(&version.token_policy)?.secret_env),
        None => Ok(std::collections::BTreeMap::new()),
    }
}

/// Author-time validation of a version's declared runtime secrets:
/// every `token_policy.runtime.secrets.env` key MUST resolve to an existing,
/// non-`is_protected` secret. Fail-closed — a broken or protected reference is
/// rejected at save time, not deferred to launch. The author is NOT required to
/// hold the launch grant here (the hard gate is C3 at launch; an author without
/// the grant simply cannot run their own playbook).
async fn validate_version_runtime_secret_refs(
    state: &AppState,
    token_policy: Option<&Value>,
) -> Result<(), AppError> {
    let Some(token_policy) = token_policy else {
        return Ok(());
    };
    let secret_env = runtime_policy_from_token_policy(token_policy)?.secret_env;
    for (env_name, key) in &secret_env {
        let secret = state
            .repositories
            .secrets
            .get_variable_by_key(key)
            .await?
            .ok_or_else(|| {
                AppError::Validation(format!(
                    "Playbook runtime secret `{key}` (env {env_name}) does not exist"
                ))
            })?;
        if secret.is_protected {
            return Err(AppError::Validation(format!(
                "Playbook runtime secret `{key}` (env {env_name}) is protected and cannot be exported to a run environment"
            )));
        }
    }
    Ok(())
}

/// Project a persisted version's runtime-secret declarations into
/// `secret_usage_binding` so admins get where-used visibility and named
/// secrets get delete-protection. Best-effort AFTER a successful create — the
/// refs were already validated pre-create by
/// `validate_version_runtime_secret_refs`.
async fn project_version_runtime_secret_usage(
    state: &AppState,
    playbook_id: Uuid,
    version_id: Uuid,
    token_policy: Option<&Value>,
) -> Result<(), AppError> {
    let Some(token_policy) = token_policy else {
        return Ok(());
    };
    let secret_env = runtime_policy_from_token_policy(token_policy)?.secret_env;
    for (env_name, key) in &secret_env {
        let Some(secret) = state.repositories.secrets.get_variable_by_key(key).await? else {
            continue;
        };
        state
            .repositories
            .secrets
            .upsert_usage_binding(SecretUsageBinding {
                id: Uuid::now_v7(),
                consumer_type: PLAYBOOK_VERSION_RUNTIME_ENV_CONSUMER_TYPE.to_string(),
                consumer_id: version_id,
                field_key: env_name.clone(),
                owner_scope_type: "playbook".to_string(),
                owner_scope_id: playbook_id,
                secret_id: secret.id,
            })
            .await?;
    }
    Ok(())
}

/// Gate **C3** for a manual REST launch: delegates to the shared app-layer
/// orchestrator (`crate::app::playbook_secret_gate`) so the manual path and the
/// scheduled-fire path reach the same verdict through one implementation.
///
/// `subject` is the canonical principal that will own the run
/// (`user:{uuid}` / `service_principal:{uuid}`), recorded in the snapshot so the
/// run worker can re-check the same principal at start. On success returns the
/// server-authored authorization snapshot (env → {secret_id, key}; NO values)
/// for `run_group.metadata.secret_env_authorization`, or `None` when no secrets
/// are declared.
pub(crate) async fn ensure_secret_use_grants(
    state: &AppState,
    request_context: &RequestContext,
    secret_env: &std::collections::BTreeMap<String, String>,
    subject: &str,
) -> Result<Option<Value>, AppError> {
    crate::app::playbook_secret_gate::ensure_secret_use_grants(
        &state.services.access_control,
        &state.repositories.secrets,
        crate::app::playbook_secret_gate::SecretGateCaller::Request {
            request_context,
            subject,
        },
        secret_env,
    )
    .await
}

/// Dry-run of C3: per-secret verdicts for the launch dialog, using the
/// SAME `secret_use_allowed` core as `ensure_secret_use_grants` so a preflight
/// verdict equals the launch verdict by construction. Never mutates state and
/// never emits denied-audit.
async fn secret_use_preflight(
    state: &AppState,
    request_context: &RequestContext,
    secret_env: &std::collections::BTreeMap<String, String>,
) -> Result<SecretPreflightResponse, AppError> {
    let policies = state
        .services
        .access_control
        .resolve_policies_for_request(request_context)
        .await?;
    let context = state
        .services
        .access_control
        .constraint_context_for_request(request_context, ResourceType::SecretVariable, None)?;

    let mut entries = Vec::with_capacity(secret_env.len());
    for (env_name, key) in secret_env {
        let secret = state.repositories.secrets.get_variable_by_key(key).await?;
        let (secret_id, has_value, allowed) = match secret {
            Some(secret) if secret.is_protected => (Some(secret.id), false, false),
            Some(secret) => {
                let has_value = match state.services.secret_store.get_secret(secret.id).await {
                    Ok(_) => true,
                    Err(AppError::NotFound(_)) => false,
                    Err(error) => return Err(error),
                };
                let allowed =
                    secret_use_allowed(&policies, &context, secret.id, key, |candidate| {
                        secret_use_pat_allows(request_context, candidate)
                    });
                (Some(secret.id), has_value, allowed)
            }
            None => (None, false, false),
        };
        entries.push(SecretPreflightEntry {
            env_name: env_name.clone(),
            key: key.clone(),
            secret_id,
            has_value,
            allowed,
        });
    }
    let all_allowed = entries.iter().all(|entry| entry.allowed);
    Ok(SecretPreflightResponse {
        secrets: entries,
        all_allowed,
    })
}

/// PAT token-scope ceiling for the C2 credential-visibility gate. The
/// shared `resolve_secret_visibility` runs `check_access` directly, which
/// BYPASSES the PAT ceiling; a scoped API token could otherwise use an explicit
/// credential its scope never granted. Each launch-path C2 site gates
/// `resolve_secret_visibility` behind this so a token cannot exceed its declared
/// `Secret × SecretVariable` scope. (The shared checker stays untouched —
/// `connection_config` depends on it.)
fn credential_use_scope_allowed(request_context: &RequestContext) -> bool {
    !matches!(
        request_context.auth_method.as_str(),
        "api_token" | "playbook_job_token"
    ) || request_context.allows_api_token_scope_for_resource(
        "secret",
        ResourceType::SecretVariable.as_str(),
        None,
    )
}

/// Gate binding a named secret to a playbook project's SCM credential.
///
/// A project resolves the bound secret into git checkout material, so binding
/// is disclosure-equivalent and requires the canonical per-secret
/// `Secret × SecretVariable` authority. Check authorization before loading the
/// secret so an unauthorized caller cannot distinguish an existing secret from
/// an absent one.
async fn ensure_project_credential_use(
    state: &AppState,
    request_context: &RequestContext,
    credential_id: Option<Uuid>,
) -> Result<(), AppError> {
    let Some(secret_id) = credential_id else {
        return Ok(());
    };

    // Match the secret-value routes exactly: the canonical prefixed resource id
    // preserves per-secret allow/deny decisions and the PAT scope ceiling.
    let resource_id = format!("secret_variable:{secret_id}");
    build_policy_evaluation_service(state)
        .ensure_access(
            request_context,
            Action::Secret,
            ResourceType::SecretVariable,
            Some(&resource_id),
        )
        .await?;

    let secret = state
        .repositories
        .secrets
        .get_variable_by_id(secret_id)
        .await?
        .ok_or_else(|| {
            AppError::Validation("credential_id must identify an existing named secret".to_string())
        })?;
    if secret.kind != SECRET_KIND_NAMED {
        return Err(AppError::Validation(
            "credential_id must identify an existing named secret".to_string(),
        ));
    }
    if secret.is_protected {
        return Err(AppError::Forbidden(
            "Protected named secrets cannot be used by playbook projects".to_string(),
        ));
    }

    Ok(())
}

async fn ensure_target_set_access(
    state: &AppState,
    request_context: &RequestContext,
    action: Action,
    resource_id: Option<&str>,
) -> Result<(), AppError> {
    state
        .services
        .policy_evaluation
        .ensure_access(
            request_context,
            action,
            ResourceType::PlaybookTargetSet,
            resource_id,
        )
        .await
}

async fn ensure_version_access(
    state: &AppState,
    request_context: &RequestContext,
    action: Action,
    resource_id: Option<&str>,
) -> Result<(), AppError> {
    state
        .services
        .policy_evaluation
        .ensure_access(
            request_context,
            action,
            ResourceType::PlaybookVersion,
            resource_id,
        )
        .await
}

async fn ensure_assignment_access(
    state: &AppState,
    request_context: &RequestContext,
    action: Action,
    resource_id: Option<&str>,
) -> Result<(), AppError> {
    state
        .services
        .policy_evaluation
        .ensure_access(
            request_context,
            action,
            ResourceType::PlaybookAssignment,
            resource_id,
        )
        .await
}

async fn ensure_connection_target_access(
    state: &AppState,
    request_context: &RequestContext,
    action: Action,
    target_id: Uuid,
) -> Result<(), AppError> {
    // Attribute-aware authorization for single-target playbook APIs. Launch
    // fan-out uses `ensure_authorization_targets_access`, which batches facets
    // and policy resolution for the entire target set.
    let attributes = connection_target_authz_attributes_for(state, target_id, None).await?;
    // Canonical `connection_target:{id}` form like every other per-target site,
    // so canonically-authored DENY policies match the execute path too. Bare-id
    // patterns keep matching (the matcher strips the prefix for them), so
    // existing bare-id policies are unaffected.
    let resource_id = format!("connection_target:{target_id}");
    state
        .services
        .policy_evaluation
        .ensure_access_with_attributes(
            request_context,
            action,
            ResourceType::ConnectionTarget,
            Some(resource_id.as_str()),
            Some(attributes),
        )
        .await
}

async fn ensure_schedule_access(
    state: &AppState,
    request_context: &RequestContext,
    action: Action,
    resource_id: Option<&str>,
) -> Result<(), AppError> {
    state
        .services
        .policy_evaluation
        .ensure_access(
            request_context,
            action,
            ResourceType::PlaybookSchedule,
            resource_id,
        )
        .await
}

async fn ensure_service_principal_access(
    state: &AppState,
    request_context: &RequestContext,
    action: Action,
    resource_id: Option<&str>,
) -> Result<(), AppError> {
    state
        .services
        .policy_evaluation
        .ensure_access(
            request_context,
            action,
            ResourceType::PlaybookServicePrincipal,
            resource_id,
        )
        .await
}

async fn ensure_execution_environment_access(
    state: &AppState,
    request_context: &RequestContext,
    action: Action,
    resource_id: Option<&str>,
) -> Result<(), AppError> {
    state
        .services
        .policy_evaluation
        .ensure_access(
            request_context,
            action,
            ResourceType::PlaybookExecutionEnvironment,
            resource_id,
        )
        .await
}

async fn ensure_project_access(
    state: &AppState,
    request_context: &RequestContext,
    action: Action,
    resource_id: Option<&str>,
) -> Result<(), AppError> {
    state
        .services
        .policy_evaluation
        .ensure_access(
            request_context,
            action,
            ResourceType::PlaybookProject,
            resource_id,
        )
        .await
}

async fn ensure_job_template_access(
    state: &AppState,
    request_context: &RequestContext,
    action: Action,
    resource_id: Option<&str>,
) -> Result<(), AppError> {
    state
        .services
        .policy_evaluation
        .ensure_access(
            request_context,
            action,
            ResourceType::PlaybookJobTemplate,
            resource_id,
        )
        .await
}

async fn ensure_run_access(
    state: &AppState,
    request_context: &RequestContext,
    action: Action,
    resource_id: Option<&str>,
) -> Result<(), AppError> {
    state
        .services
        .policy_evaluation
        .ensure_access(
            request_context,
            action,
            ResourceType::PlaybookRun,
            resource_id,
        )
        .await
}

async fn ensure_run_group_access(
    state: &AppState,
    request_context: &RequestContext,
    action: Action,
    resource_id: Option<&str>,
) -> Result<(), AppError> {
    state
        .services
        .policy_evaluation
        .ensure_access(
            request_context,
            action,
            ResourceType::PlaybookRunGroup,
            resource_id,
        )
        .await
}

/// List playbooks.
#[utoipa::path(
    get,
    path = "/api/v1/playbooks",
    tag = "playbooks",
    params(ListPlaybooksQuery),
    responses(
        (status = 200, description = "Playbook catalog", body = ListPlaybooksResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_playbooks(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ListPlaybooksQuery>,
) -> Response {
    let request_context = request_context(&request_id, auth_context, &headers);
    if let Err(error) = request_context.require_authenticated_user_id() {
        return handle_app_error(error);
    }
    if let Err(error) = ensure_access(&state, &request_context, Action::Read, None).await {
        return handle_app_error(error);
    }
    let (limit, offset) = normalize_page(query.limit, query.offset);
    match state.services.playbooks.list_catalog(limit, offset).await {
        Ok(page) => Json(ListPlaybooksResponse {
            items: page.items,
            total: page.total,
            limit: page.limit,
            offset: page.offset,
        })
        .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Get a playbook.
#[utoipa::path(
    get,
    path = "/api/v1/playbooks/{id}",
    tag = "playbooks",
    params(("id" = Uuid, Path, description = "Playbook ID")),
    responses(
        (status = 200, description = "Playbook", body = PlaybookCatalogItem),
        (status = 404, description = "Playbook not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_playbook(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let request_context = request_context(&request_id, auth_context, &headers);
    if let Err(error) = request_context.require_authenticated_user_id() {
        return handle_app_error(error);
    }
    let resource_id = id.to_string();
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Read,
        Some(resource_id.as_str()),
    )
    .await
    {
        return handle_app_error(error);
    }
    match state.services.playbooks.get_catalog_item(id).await {
        Ok(Some(item)) => Json(item).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound(format!("Playbook '{id}' not found"))),
        Err(error) => handle_app_error(error),
    }
}

/// List execution environments.
#[utoipa::path(
    get,
    path = "/api/v1/playbooks/execution-environments",
    tag = "playbooks",
    params(ListPlaybooksQuery),
    responses(
        (status = 200, description = "Execution environments", body = ListExecutionEnvironmentsResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn list_execution_environments(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ListPlaybooksQuery>,
) -> Response {
    let request_context = request_context(&request_id, auth_context, &headers);
    if let Err(error) = request_context.require_authenticated_user_id() {
        return handle_app_error(error);
    }
    if let Err(error) =
        ensure_execution_environment_access(&state, &request_context, Action::Read, None).await
    {
        return handle_app_error(error);
    }
    let (limit, offset) = normalize_page(query.limit, query.offset);
    match state
        .services
        .playbooks
        .list_execution_environments(limit, offset)
        .await
    {
        Ok(page) => Json(ListExecutionEnvironmentsResponse {
            items: page.items,
            total: page.total,
            limit: page.limit,
            offset: page.offset,
        })
        .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Get an execution environment.
#[utoipa::path(
    get,
    path = "/api/v1/playbooks/execution-environments/{id}",
    tag = "playbooks",
    params(("id" = Uuid, Path, description = "Execution environment ID")),
    responses((status = 200, description = "Execution environment", body = PlaybookExecutionEnvironment), (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse))
)]
pub async fn get_execution_environment(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let request_context = request_context(&request_id, auth_context, &headers);
    if let Err(error) = request_context.require_authenticated_user_id() {
        return handle_app_error(error);
    }
    let resource_id = id.to_string();
    if let Err(error) = ensure_execution_environment_access(
        &state,
        &request_context,
        Action::Read,
        Some(&resource_id),
    )
    .await
    {
        return handle_app_error(error);
    }
    match state.services.playbooks.get_execution_environment(id).await {
        Ok(Some(item)) => Json(item).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound(format!(
            "Playbook execution environment '{id}' not found"
        ))),
        Err(error) => handle_app_error(error),
    }
}

/// Create an execution environment.
#[utoipa::path(
    post,
    path = "/api/v1/playbooks/execution-environments",
    tag = "playbooks",
    request_body = CreateExecutionEnvironmentRequest,
    responses(
        (status = 201, description = "Execution environment created", body = PlaybookExecutionEnvironment),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn create_execution_environment(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<CreateExecutionEnvironmentRequest>,
) -> Response {
    let request_context = request_context(&request_id, Some(auth_context), &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(user_id) => user_id,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) =
        ensure_execution_environment_access(&state, &request_context, Action::Create, None).await
    {
        return handle_app_error(error);
    }
    let input = CreateExecutionEnvironmentInput {
        name: payload.name,
        description: payload.description,
        image: payload.image,
        pull_policy: payload.pull_policy,
        runner_group: payload.runner_group,
        enabled: payload.enabled,
        env: payload.env,
        settings: payload.settings,
        metadata: payload.metadata,
    };
    match state
        .services
        .playbooks
        .create_execution_environment(input, Some(user_id))
        .await
    {
        Ok(item) => {
            if let Err(error) = record_playbook_audit(
                &state,
                &request_context,
                "playbook.execution_environment.created",
                "playbook_execution_environment",
                Some(item.id),
                json!({
                    "action": "create",
                    "reason_code": "execution_environment_created",
                    "summary": "playbook execution environment created",
                    "affected_count": 1,
                    "resource_type": "playbook_execution_environment",
                    "resource_id": item.id.to_string(),
                    "execution_environment_id": item.id.to_string(),
                    "name": item.name.clone(),
                    "image": item.image.clone(),
                    "enabled": item.enabled,
                }),
            )
            .await
            {
                return handle_app_error(error);
            }
            (StatusCode::CREATED, Json(item)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Update an execution environment.
#[utoipa::path(
    patch,
    path = "/api/v1/playbooks/execution-environments/{id}",
    tag = "playbooks",
    params(("id" = Uuid, Path, description = "Execution environment ID")),
    request_body = UpdateExecutionEnvironmentRequest,
    responses(
        (status = 200, description = "Execution environment updated", body = PlaybookExecutionEnvironment),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn update_execution_environment(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    EnvelopeJson(payload): EnvelopeJson<UpdateExecutionEnvironmentRequest>,
) -> Response {
    let request_context = request_context(&request_id, Some(auth_context), &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(user_id) => user_id,
        Err(error) => return handle_app_error(error),
    };
    let resource_id = id.to_string();
    if let Err(error) = ensure_execution_environment_access(
        &state,
        &request_context,
        Action::Update,
        Some(&resource_id),
    )
    .await
    {
        return handle_app_error(error);
    }
    let input = UpdateExecutionEnvironmentInput {
        name: payload.name,
        description: payload.description,
        image: payload.image,
        pull_policy: payload.pull_policy,
        runner_group: payload.runner_group,
        enabled: payload.enabled,
        env: payload.env,
        settings: payload.settings,
        metadata: payload.metadata,
    };
    match state
        .services
        .playbooks
        .update_execution_environment(id, input, Some(user_id))
        .await
    {
        Ok(Some(item)) => {
            if let Err(error) = record_playbook_audit(
                &state,
                &request_context,
                "playbook.execution_environment.updated",
                "playbook_execution_environment",
                Some(item.id),
                json!({
                    "action": "update",
                    "reason_code": "execution_environment_updated",
                    "summary": "playbook execution environment updated",
                    "affected_count": 1,
                    "resource_type": "playbook_execution_environment",
                    "resource_id": item.id.to_string(),
                    "execution_environment_id": item.id.to_string(),
                    "name": item.name.clone(),
                    "enabled": item.enabled,
                }),
            )
            .await
            {
                return handle_app_error(error);
            }
            Json(item).into_response()
        }
        Ok(None) => handle_app_error(AppError::NotFound(format!(
            "Playbook execution environment '{id}' not found"
        ))),
        Err(error) => handle_app_error(error),
    }
}

/// Delete an execution environment.
#[utoipa::path(
    delete,
    path = "/api/v1/playbooks/execution-environments/{id}",
    tag = "playbooks",
    params(("id" = Uuid, Path, description = "Execution environment ID")),
    responses(
        (status = 204, description = "Execution environment deleted"),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 409, description = "Execution environment is still referenced", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn delete_execution_environment(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let request_context = request_context(&request_id, auth_context, &headers);
    if let Err(error) = request_context.require_authenticated_user_id() {
        return handle_app_error(error);
    }
    let resource_id = id.to_string();
    if let Err(error) = ensure_execution_environment_access(
        &state,
        &request_context,
        Action::Delete,
        Some(&resource_id),
    )
    .await
    {
        return handle_app_error(error);
    }
    match state
        .services
        .playbooks
        .delete_execution_environment(id)
        .await
    {
        Ok(true) => {
            if let Err(error) = record_playbook_audit(
                &state,
                &request_context,
                "playbook.execution_environment.deleted",
                "playbook_execution_environment",
                Some(id),
                json!({
                    "action": "delete",
                    "reason_code": "execution_environment_deleted",
                    "summary": "playbook execution environment deleted",
                    "affected_count": 1,
                    "resource_type": "playbook_execution_environment",
                    "resource_id": id.to_string(),
                    "execution_environment_id": id.to_string(),
                    "deleted": true,
                }),
            )
            .await
            {
                return handle_app_error(error);
            }
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => handle_app_error(AppError::NotFound(format!(
            "Playbook execution environment '{id}' not found"
        ))),
        Err(error) => handle_app_error(error),
    }
}

/// List projects.
#[utoipa::path(
    get,
    path = "/api/v1/playbooks/projects",
    tag = "playbooks",
    params(ListPlaybooksQuery),
    responses(
        (status = 200, description = "Projects", body = ListProjectsResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn list_projects(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ListPlaybooksQuery>,
) -> Response {
    let request_context = request_context(&request_id, auth_context, &headers);
    if let Err(error) = request_context.require_authenticated_user_id() {
        return handle_app_error(error);
    }
    if let Err(error) = ensure_project_access(&state, &request_context, Action::Read, None).await {
        return handle_app_error(error);
    }
    let (limit, offset) = normalize_page(query.limit, query.offset);
    match state.services.playbooks.list_projects(limit, offset).await {
        Ok(page) => Json(ListProjectsResponse {
            items: page.items,
            total: page.total,
            limit: page.limit,
            offset: page.offset,
        })
        .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Get a project.
#[utoipa::path(
    get,
    path = "/api/v1/playbooks/projects/{id}",
    tag = "playbooks",
    params(("id" = Uuid, Path, description = "Project ID")),
    responses((status = 200, description = "Project", body = PlaybookProject), (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse))
)]
pub async fn get_project(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let request_context = request_context(&request_id, auth_context, &headers);
    if let Err(error) = request_context.require_authenticated_user_id() {
        return handle_app_error(error);
    }
    let resource_id = id.to_string();
    if let Err(error) =
        ensure_project_access(&state, &request_context, Action::Read, Some(&resource_id)).await
    {
        return handle_app_error(error);
    }
    match state.services.playbooks.get_project(id).await {
        Ok(Some(item)) => Json(item).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound(format!(
            "Playbook project '{id}' not found"
        ))),
        Err(error) => handle_app_error(error),
    }
}

/// Create a project.
#[utoipa::path(
    post,
    path = "/api/v1/playbooks/projects",
    tag = "playbooks",
    request_body = CreateProjectRequest,
    responses(
        (status = 201, description = "Project created", body = PlaybookProject),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn create_project(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<CreateProjectRequest>,
) -> Response {
    let request_context = request_context(&request_id, Some(auth_context), &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(user_id) => user_id,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = ensure_project_access(&state, &request_context, Action::Create, None).await
    {
        return handle_app_error(error);
    }
    if let Some(execution_environment_id) = payload.default_execution_environment_id {
        let resource_id = execution_environment_id.to_string();
        if let Err(error) = ensure_execution_environment_access(
            &state,
            &request_context,
            Action::Read,
            Some(&resource_id),
        )
        .await
        {
            return handle_app_error(error);
        }
    }
    if let Err(error) =
        ensure_project_credential_use(&state, &request_context, payload.credential_id).await
    {
        return handle_app_error(error);
    }
    let input = CreateProjectInput {
        name: payload.name,
        description: payload.description,
        scm_type: payload.scm_type,
        scm_url: payload.scm_url,
        scm_branch: payload.scm_branch,
        scm_ref: payload.scm_ref,
        playbook_base_path: payload.playbook_base_path,
        credential_id: payload.credential_id,
        default_execution_environment_id: payload.default_execution_environment_id,
        sync_policy: payload.sync_policy,
        metadata: payload.metadata,
    };
    match state
        .services
        .playbooks
        .create_project(input, Some(user_id))
        .await
    {
        Ok(item) => {
            if let Err(error) = record_playbook_audit(
                &state,
                &request_context,
                "playbook.project.created",
                "playbook_project",
                Some(item.id),
                json!({
                    "action": "create",
                    "reason_code": "project_created",
                    "summary": "playbook project created",
                    "affected_count": 1,
                    "resource_type": "playbook_project",
                    "resource_id": item.id.to_string(),
                    "project_id": item.id.to_string(),
                    "name": item.name.clone(),
                    "scm_type": item.scm_type.clone(),
                    "sync_policy": item.sync_policy.clone(),
                }),
            )
            .await
            {
                return handle_app_error(error);
            }
            (StatusCode::CREATED, Json(item)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Update a project.
#[utoipa::path(
    patch,
    path = "/api/v1/playbooks/projects/{id}",
    tag = "playbooks",
    params(("id" = Uuid, Path, description = "Project ID")),
    request_body = UpdateProjectRequest,
    responses(
        (status = 200, description = "Project updated", body = PlaybookProject),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn update_project(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    EnvelopeJson(payload): EnvelopeJson<UpdateProjectRequest>,
) -> Response {
    let request_context = request_context(&request_id, Some(auth_context), &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(user_id) => user_id,
        Err(error) => return handle_app_error(error),
    };
    let resource_id = id.to_string();
    if let Err(error) =
        ensure_project_access(&state, &request_context, Action::Update, Some(&resource_id)).await
    {
        return handle_app_error(error);
    }
    if let Some(Some(execution_environment_id)) = payload.default_execution_environment_id {
        let ee_resource_id = execution_environment_id.to_string();
        if let Err(error) = ensure_execution_environment_access(
            &state,
            &request_context,
            Action::Read,
            Some(&ee_resource_id),
        )
        .await
        {
            return handle_app_error(error);
        }
    }
    // Authorize the EFFECTIVE credential, not only a submitted one. When the
    // project already owns a binding and this PATCH does not explicitly clear
    // it, the binding survives the update: `payload.credential_id.flatten()`
    // yields `None` for an absent field and the gate would return `Ok` without
    // checking anything, letting a caller with only `PLAYBOOK_PROJECT:Update`
    // redirect `scm_url` to a host they control and expose the still-bound
    // secret on the next checkout.
    let effective_credential_id = match payload.credential_id {
        Some(submitted) => submitted,
        None => match state.services.playbooks.get_project(id).await {
            Ok(Some(existing)) => existing.credential_id,
            Ok(None) => None,
            Err(error) => return handle_app_error(error),
        },
    };
    if let Err(error) =
        ensure_project_credential_use(&state, &request_context, effective_credential_id).await
    {
        return handle_app_error(error);
    }
    let input = UpdateProjectInput {
        name: payload.name,
        description: payload.description,
        scm_url: payload.scm_url,
        scm_branch: payload.scm_branch,
        scm_ref: payload.scm_ref,
        playbook_base_path: payload.playbook_base_path,
        credential_id: payload.credential_id,
        default_execution_environment_id: payload.default_execution_environment_id,
        sync_policy: payload.sync_policy,
    };
    match state
        .services
        .playbooks
        .update_project(id, input, Some(user_id))
        .await
    {
        Ok(Some(item)) => {
            if let Err(error) = record_playbook_audit(
                &state,
                &request_context,
                "playbook.project.updated",
                "playbook_project",
                Some(item.id),
                json!({
                    "action": "update",
                    "reason_code": "project_updated",
                    "summary": "playbook project updated",
                    "affected_count": 1,
                    "resource_type": "playbook_project",
                    "resource_id": item.id.to_string(),
                    "project_id": item.id.to_string(),
                    "name": item.name.clone(),
                    "scm_type": item.scm_type.clone(),
                }),
            )
            .await
            {
                return handle_app_error(error);
            }
            Json(item).into_response()
        }
        Ok(None) => handle_app_error(AppError::NotFound(format!(
            "Playbook project '{id}' not found"
        ))),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a project.
#[utoipa::path(
    delete,
    path = "/api/v1/playbooks/projects/{id}",
    tag = "playbooks",
    params(("id" = Uuid, Path, description = "Project ID")),
    responses(
        (status = 204, description = "Project deleted"),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 409, description = "Project still has job templates", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn delete_project(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let request_context = request_context(&request_id, auth_context, &headers);
    if let Err(error) = request_context.require_authenticated_user_id() {
        return handle_app_error(error);
    }
    let resource_id = id.to_string();
    if let Err(error) =
        ensure_project_access(&state, &request_context, Action::Delete, Some(&resource_id)).await
    {
        return handle_app_error(error);
    }
    match state.services.playbooks.delete_project(id).await {
        Ok(true) => {
            if let Err(error) = record_playbook_audit(
                &state,
                &request_context,
                "playbook.project.deleted",
                "playbook_project",
                Some(id),
                json!({
                    "action": "delete",
                    "reason_code": "project_deleted",
                    "summary": "playbook project deleted",
                    "affected_count": 1,
                    "resource_type": "playbook_project",
                    "resource_id": id.to_string(),
                    "project_id": id.to_string(),
                    "deleted": true,
                }),
            )
            .await
            {
                return handle_app_error(error);
            }
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => handle_app_error(AppError::NotFound(format!(
            "Playbook project '{id}' not found"
        ))),
        Err(error) => handle_app_error(error),
    }
}

/// Upload files into a project.
#[utoipa::path(
    post,
    path = "/api/v1/playbooks/projects/{id}/files",
    tag = "playbooks",
    params(("id" = Uuid, Path, description = "Project ID")),
    request_body = UploadProjectFilesRequest,
    responses(
        (status = 200, description = "Manual project files stored", body = PlaybookProject),
        (status = 400, description = "Invalid file set or non-manual project", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn upload_project_files(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    EnvelopeJson(payload): EnvelopeJson<UploadProjectFilesRequest>,
) -> Response {
    if let Err(error) = payload.validate() {
        return handle_app_error(error);
    }
    let request_context = request_context(&request_id, Some(auth_context), &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(user_id) => user_id,
        Err(error) => return handle_app_error(error),
    };
    let resource_id = id.to_string();
    if let Err(error) =
        ensure_project_access(&state, &request_context, Action::Update, Some(&resource_id)).await
    {
        return handle_app_error(error);
    }
    let files: Vec<AnsibleProjectFile> = payload
        .files
        .into_iter()
        .map(|file| AnsibleProjectFile {
            path: file.path,
            content: file.content,
            executable: file.executable.unwrap_or(false),
        })
        .collect();
    let file_count = files.len();
    match state
        .services
        .playbooks
        .set_project_files(id, files, Some(user_id))
        .await
    {
        Ok(Some(item)) => {
            if let Err(error) = record_playbook_audit(
                &state,
                &request_context,
                "playbook.project.files_uploaded",
                "playbook_project",
                Some(item.id),
                json!({
                    "action": "update",
                    "reason_code": "project_files_uploaded",
                    "summary": "playbook project files uploaded",
                    "affected_count": 1,
                    "resource_type": "playbook_project",
                    "resource_id": item.id.to_string(),
                    "project_id": item.id.to_string(),
                    "file_count": file_count,
                }),
            )
            .await
            {
                return handle_app_error(error);
            }
            Json(item).into_response()
        }
        Ok(None) => handle_app_error(AppError::NotFound(format!(
            "Playbook project '{id}' not found"
        ))),
        Err(error) => handle_app_error(error),
    }
}

/// Get the stored files of a project.
#[utoipa::path(
    get,
    path = "/api/v1/playbooks/projects/{id}/files",
    tag = "playbooks",
    params(("id" = Uuid, Path, description = "Project ID")),
    responses(
        (status = 200, description = "Stored project files", body = ProjectFilesResponse),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn get_project_files(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let request_context = request_context(&request_id, auth_context, &headers);
    if let Err(error) = request_context.require_authenticated_user_id() {
        return handle_app_error(error);
    }
    let resource_id = id.to_string();
    if let Err(error) =
        ensure_project_access(&state, &request_context, Action::Read, Some(&resource_id)).await
    {
        return handle_app_error(error);
    }
    match state.services.playbooks.get_project_files(id).await {
        Ok(Some(files)) => Json(ProjectFilesResponse {
            files: files
                .into_iter()
                .map(|file| ProjectFileResponse {
                    path: file.path,
                    content: file.content,
                    executable: file.executable,
                })
                .collect(),
        })
        .into_response(),
        Ok(None) => handle_app_error(AppError::NotFound(format!(
            "Playbook project '{id}' not found"
        ))),
        Err(error) => handle_app_error(error),
    }
}

/// List job templates.
#[utoipa::path(
    get,
    path = "/api/v1/playbooks/job-templates",
    tag = "playbooks",
    params(ListPlaybooksQuery),
    responses(
        (status = 200, description = "Job templates", body = ListJobTemplatesResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn list_job_templates(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ListPlaybooksQuery>,
) -> Response {
    let request_context = request_context(&request_id, auth_context, &headers);
    if let Err(error) = request_context.require_authenticated_user_id() {
        return handle_app_error(error);
    }
    if let Err(error) =
        ensure_job_template_access(&state, &request_context, Action::Read, None).await
    {
        return handle_app_error(error);
    }
    let (limit, offset) = normalize_page(query.limit, query.offset);
    match state
        .services
        .playbooks
        .list_job_templates(limit, offset)
        .await
    {
        Ok(page) => Json(ListJobTemplatesResponse {
            items: page.items,
            total: page.total,
            limit: page.limit,
            offset: page.offset,
        })
        .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Get a job template.
#[utoipa::path(
    get,
    path = "/api/v1/playbooks/job-templates/{id}",
    tag = "playbooks",
    params(("id" = Uuid, Path, description = "Job template ID")),
    responses((status = 200, description = "Job template", body = PlaybookJobTemplate), (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse))
)]
pub async fn get_job_template(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let request_context = request_context(&request_id, auth_context, &headers);
    if let Err(error) = request_context.require_authenticated_user_id() {
        return handle_app_error(error);
    }
    let resource_id = id.to_string();
    if let Err(error) =
        ensure_job_template_access(&state, &request_context, Action::Read, Some(&resource_id)).await
    {
        return handle_app_error(error);
    }
    match state.services.playbooks.get_job_template(id).await {
        Ok(Some(item)) => Json(item).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound(format!(
            "Playbook job template '{id}' not found"
        ))),
        Err(error) => handle_app_error(error),
    }
}

/// Authorize the related resources a create request references (playbook,
/// project, target set, execution environment) before persisting.
async fn ensure_create_job_template_related_access(
    state: &AppState,
    request_context: &RequestContext,
    payload: &CreateJobTemplateRequest,
) -> Result<(), AppError> {
    if let Some(playbook_id) = payload.playbook_id {
        ensure_access(
            state,
            request_context,
            Action::Read,
            Some(&playbook_id.to_string()),
        )
        .await?;
    }
    if let Some(project_id) = payload.project_id {
        ensure_project_access(
            state,
            request_context,
            Action::Read,
            Some(&project_id.to_string()),
        )
        .await?;
    }
    if let Some(target_set_id) = payload.inventory_target_set_id {
        ensure_target_set_access(
            state,
            request_context,
            Action::Read,
            Some(&target_set_id.to_string()),
        )
        .await?;
    }
    if let Some(execution_environment_id) = payload.execution_environment_id {
        ensure_execution_environment_access(
            state,
            request_context,
            Action::Read,
            Some(&execution_environment_id.to_string()),
        )
        .await?;
    }
    Ok(())
}

/// Gate the caller's ability to reference managed job-template credentials.
/// `needs_visibility` is the caller's "does this request reference credentials"
/// verdict; when set, the caller must be able to view secrets.
async fn ensure_job_template_credential_visibility(
    state: &AppState,
    request_context: &RequestContext,
    needs_visibility: bool,
) -> Result<(), AppError> {
    if !needs_visibility {
        return Ok(());
    }
    let can_use_credential = resolve_secret_visibility(
        Some(state.services.access_control.as_ref()),
        Some(request_context),
    )
    .await?;
    if !can_use_credential {
        return Err(AppError::Forbidden(
            "Not authorized to use referenced job template credentials".to_string(),
        ));
    }
    Ok(())
}

/// Authorize the resolved run fan-out: the target set plus every concrete
/// connection target the run will touch.
async fn ensure_authorization_targets_access(
    state: &AppState,
    request_context: &RequestContext,
    authorization_targets: &RunAuthorizationTargets,
) -> Result<(), AppError> {
    let policies = state
        .services
        .access_control
        .resolve_policies_for_request(request_context)
        .await?;

    let target_set_resource_id = authorization_targets.target_set_id.to_string();
    ensure_resolved_read_access(
        request_context,
        &policies,
        &state
            .services
            .access_control
            .constraint_context_for_request(
                request_context,
                ResourceType::PlaybookTargetSet,
                Some(target_set_resource_id.as_str()),
            )?,
        ResourceType::PlaybookTargetSet,
        Some(target_set_resource_id.as_str()),
    )?;

    let facets =
        resolve_connection_target_facets_batch(state, authorization_targets.target_ids.as_slice())
            .await?;
    let connection_target_context = state
        .services
        .access_control
        .constraint_context_for_request(request_context, ResourceType::ConnectionTarget, None)?;
    let predicate = compile_connection_target_visibility(&policies, &connection_target_context);

    for target_id in &authorization_targets.target_ids {
        let resource_id = format!("connection_target:{target_id}");
        ensure_read_token_scope(
            request_context,
            ResourceType::ConnectionTarget,
            Some(resource_id.as_str()),
        )?;
        if !batch_connection_target_is_allowed(&predicate, *target_id, facets.get(target_id)) {
            return Err(AppError::Forbidden("Access denied".to_string()));
        }
    }
    Ok(())
}

/// Apply the non-policy ceilings that `PolicyEvaluationService` normally wraps
/// around a decision. This remains per resource because PAT scopes may name one
/// connection target even though the effective policy set is request-wide.
fn ensure_read_token_scope(
    request_context: &RequestContext,
    resource_type: ResourceType,
    resource_id: Option<&str>,
) -> Result<(), AppError> {
    if matches!(
        request_context.auth_method.as_str(),
        "api_token" | "playbook_job_token"
    ) && !request_context.allows_api_token_scope_for_resource(
        "read",
        resource_type.as_str(),
        resource_id,
    ) {
        return Err(AppError::Forbidden(
            "TOKEN_SCOPE_DENIED:token scope does not permit this action".to_string(),
        ));
    }
    if request_context.auth_method == "playbook_job_token" {
        return Err(AppError::Forbidden(
            "TOKEN_SCOPE_DENIED:playbook job token is limited to inventory".to_string(),
        ));
    }
    Ok(())
}

fn ensure_resolved_read_access(
    request_context: &RequestContext,
    policies: &[Policy],
    context: &Value,
    resource_type: ResourceType,
    resource_id: Option<&str>,
) -> Result<(), AppError> {
    ensure_read_token_scope(request_context, resource_type.clone(), resource_id)?;
    let decision = evaluate_policies_with_metadata(
        policies,
        Action::Read,
        resource_type,
        resource_id,
        context,
    )
    .decision;
    if decision == PolicyDecision::Deny {
        return Err(AppError::Forbidden("Access denied".to_string()));
    }
    Ok(())
}

/// Gate credential use at launch. Mirrors the create/update credential gate but
/// is ceiling-aware (C2): a scoped PAT cannot exceed its declared scope, and the
/// template's launch options and the request metadata may both name an explicit
/// credential reference that also demands secret visibility.
async fn ensure_launch_credential_visibility(
    state: &AppState,
    request_context: &RequestContext,
    template: &PlaybookJobTemplate,
    payload_metadata: Option<&Value>,
) -> Result<(), AppError> {
    let needs_secret_visibility = launch_references_credentials(
        &template.credential_refs,
        &template.launch_options,
        payload_metadata,
    )?;
    if !needs_secret_visibility {
        return Ok(());
    }
    // C2 (ceiling-aware): gate `resolve_secret_visibility` behind the PAT
    // token-scope ceiling so a scoped token cannot exceed its declared scope.
    let can_use_credential = if !credential_use_scope_allowed(request_context) {
        false
    } else {
        resolve_secret_visibility(
            Some(state.services.access_control.as_ref()),
            Some(request_context),
        )
        .await?
    };
    if !can_use_credential {
        return Err(AppError::Forbidden(
            "Not authorized to use referenced job template credentials".to_string(),
        ));
    }
    Ok(())
}

/// Decide whether a launch references a managed credential through any of the
/// three supported channels. Kept separate from the authorization lookup so
/// malformed references and request overrides can be tested without a database.
fn launch_references_credentials(
    template_credential_refs: &Value,
    template_launch_options: &Value,
    payload_metadata: Option<&Value>,
) -> Result<bool, AppError> {
    let template_launch_metadata = json!({ "launch_options": template_launch_options.clone() });
    let credential_refs = job_template_credential_references(Some(template_credential_refs))?;
    Ok(!credential_refs.is_empty()
        || explicit_run_credential_reference(Some(&template_launch_metadata)).is_some()
        || explicit_run_credential_reference(payload_metadata).is_some())
}

/// Resolve the launch inventory with request input taking precedence over the
/// template default. Both launch paths must authorize the same effective set.
fn resolve_launch_target_set_id(
    requested_target_set_id: Option<Uuid>,
    template_target_set_id: Option<Uuid>,
) -> Option<Uuid> {
    requested_target_set_id.or(template_target_set_id)
}

/// Create a job template.
#[utoipa::path(
    post,
    path = "/api/v1/playbooks/job-templates",
    tag = "playbooks",
    request_body = CreateJobTemplateRequest,
    responses(
        (status = 201, description = "Job template created", body = PlaybookJobTemplate),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn create_job_template(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<CreateJobTemplateRequest>,
) -> Response {
    let request_context = request_context(&request_id, Some(auth_context), &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(user_id) => user_id,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) =
        ensure_job_template_access(&state, &request_context, Action::Create, None).await
    {
        return handle_app_error(error);
    }
    if let Err(error) =
        ensure_create_job_template_related_access(&state, &request_context, &payload).await
    {
        return handle_app_error(error);
    }
    let credential_refs = match job_template_credential_references(payload.credential_refs.as_ref())
    {
        Ok(refs) => refs,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = ensure_job_template_credential_visibility(
        &state,
        &request_context,
        !credential_refs.is_empty(),
    )
    .await
    {
        return handle_app_error(error);
    }
    let input = CreateJobTemplateInput {
        name: payload.name,
        description: payload.description,
        playbook_id: payload.playbook_id,
        project_id: payload.project_id,
        playbook_path: payload.playbook_path,
        inventory_target_set_id: payload.inventory_target_set_id,
        execution_environment_id: payload.execution_environment_id,
        credential_refs: payload.credential_refs,
        extra_vars: payload.extra_vars,
        launch_options: payload.launch_options,
        ask_variables_on_launch: payload.ask_variables_on_launch,
        ask_limit_on_launch: payload.ask_limit_on_launch,
        ask_tags_on_launch: payload.ask_tags_on_launch,
        ask_skip_tags_on_launch: payload.ask_skip_tags_on_launch,
        ask_credential_on_launch: payload.ask_credential_on_launch,
        ask_execution_environment_on_launch: payload.ask_execution_environment_on_launch,
        enabled: payload.enabled,
        metadata: payload.metadata,
    };
    match state
        .services
        .playbooks
        .create_job_template(input, Some(user_id))
        .await
    {
        Ok(item) => {
            if let Err(error) = record_playbook_audit(
                &state,
                &request_context,
                "playbook.job_template.created",
                "playbook_job_template",
                Some(item.id),
                json!({
                    "action": "create",
                    "reason_code": "job_template_created",
                    "summary": "playbook job template created",
                    "affected_count": 1,
                    "resource_type": "playbook_job_template",
                    "resource_id": item.id.to_string(),
                    "job_template_id": item.id.to_string(),
                    "playbook_id": item.playbook_id.map(|id| id.to_string()),
                    "project_id": item.project_id.map(|id| id.to_string()),
                    "target_set_id": item.inventory_target_set_id.map(|id| id.to_string()),
                    "execution_environment_id": item.execution_environment_id.map(|id| id.to_string()),
                    "credential_ref_count": credential_refs.len(),
                    "enabled": item.enabled,
                }),
            )
            .await
            {
                return handle_app_error(error);
            }
            (StatusCode::CREATED, Json(item)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Update a job template.
#[utoipa::path(
    patch,
    path = "/api/v1/playbooks/job-templates/{id}",
    tag = "playbooks",
    params(("id" = Uuid, Path, description = "Job template ID")),
    request_body = UpdateJobTemplateRequest,
    responses(
        (status = 200, description = "Job template updated", body = PlaybookJobTemplate),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn update_job_template(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    EnvelopeJson(payload): EnvelopeJson<UpdateJobTemplateRequest>,
) -> Response {
    let request_context = request_context(&request_id, Some(auth_context), &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(user_id) => user_id,
        Err(error) => return handle_app_error(error),
    };
    let resource_id = id.to_string();
    if let Err(error) =
        ensure_job_template_access(&state, &request_context, Action::Update, Some(&resource_id))
            .await
    {
        return handle_app_error(error);
    }
    if let Some(Some(target_set_id)) = payload.inventory_target_set_id {
        let target_resource_id = target_set_id.to_string();
        if let Err(error) = ensure_target_set_access(
            &state,
            &request_context,
            Action::Read,
            Some(&target_resource_id),
        )
        .await
        {
            return handle_app_error(error);
        }
    }
    if let Some(Some(execution_environment_id)) = payload.execution_environment_id {
        let ee_resource_id = execution_environment_id.to_string();
        if let Err(error) = ensure_execution_environment_access(
            &state,
            &request_context,
            Action::Read,
            Some(&ee_resource_id),
        )
        .await
        {
            return handle_app_error(error);
        }
    }
    let credential_refs = match job_template_credential_references(payload.credential_refs.as_ref())
    {
        Ok(refs) => refs,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = ensure_job_template_credential_visibility(
        &state,
        &request_context,
        !credential_refs.is_empty(),
    )
    .await
    {
        return handle_app_error(error);
    }
    let input = UpdateJobTemplateInput {
        name: payload.name,
        description: payload.description,
        inventory_target_set_id: payload.inventory_target_set_id,
        execution_environment_id: payload.execution_environment_id,
        credential_refs: payload.credential_refs,
        extra_vars: payload.extra_vars,
        launch_options: payload.launch_options,
        ask_variables_on_launch: payload.ask_variables_on_launch,
        ask_limit_on_launch: payload.ask_limit_on_launch,
        ask_tags_on_launch: payload.ask_tags_on_launch,
        ask_skip_tags_on_launch: payload.ask_skip_tags_on_launch,
        ask_credential_on_launch: payload.ask_credential_on_launch,
        ask_execution_environment_on_launch: payload.ask_execution_environment_on_launch,
        enabled: payload.enabled,
        metadata: payload.metadata,
    };
    match state
        .services
        .playbooks
        .update_job_template(id, input, Some(user_id))
        .await
    {
        Ok(Some(item)) => {
            if let Err(error) = record_playbook_audit(
                &state,
                &request_context,
                "playbook.job_template.updated",
                "playbook_job_template",
                Some(item.id),
                json!({
                    "action": "update",
                    "reason_code": "job_template_updated",
                    "summary": "playbook job template updated",
                    "affected_count": 1,
                    "resource_type": "playbook_job_template",
                    "resource_id": item.id.to_string(),
                    "job_template_id": item.id.to_string(),
                    "name": item.name.clone(),
                    "enabled": item.enabled,
                }),
            )
            .await
            {
                return handle_app_error(error);
            }
            Json(item).into_response()
        }
        Ok(None) => handle_app_error(AppError::NotFound(format!(
            "Playbook job template '{id}' not found"
        ))),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a job template.
#[utoipa::path(
    delete,
    path = "/api/v1/playbooks/job-templates/{id}",
    tag = "playbooks",
    params(("id" = Uuid, Path, description = "Job template ID")),
    responses(
        (status = 204, description = "Job template deleted"),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn delete_job_template(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let request_context = request_context(&request_id, auth_context, &headers);
    if let Err(error) = request_context.require_authenticated_user_id() {
        return handle_app_error(error);
    }
    let resource_id = id.to_string();
    if let Err(error) =
        ensure_job_template_access(&state, &request_context, Action::Delete, Some(&resource_id))
            .await
    {
        return handle_app_error(error);
    }
    match state.services.playbooks.delete_job_template(id).await {
        Ok(true) => {
            if let Err(error) = record_playbook_audit(
                &state,
                &request_context,
                "playbook.job_template.deleted",
                "playbook_job_template",
                Some(id),
                json!({
                    "action": "delete",
                    "reason_code": "job_template_deleted",
                    "summary": "playbook job template deleted",
                    "affected_count": 1,
                    "resource_type": "playbook_job_template",
                    "resource_id": id.to_string(),
                    "job_template_id": id.to_string(),
                    "deleted": true,
                }),
            )
            .await
            {
                return handle_app_error(error);
            }
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => handle_app_error(AppError::NotFound(format!(
            "Playbook job template '{id}' not found"
        ))),
        Err(error) => handle_app_error(error),
    }
}

/// Assemble the audit-metadata payload for a queued job-template launch.
/// Project-backed templates (no playbook id) additionally record the ansible
/// project they execute.
fn job_template_launch_audit_details(
    template: &PlaybookJobTemplate,
    created: &PlaybookRunGroupCreated,
    requested_target_limit: Option<i64>,
) -> Value {
    let details = json!({
        "action": "launch",
        "reason_code": "job_template_launch_requested",
        "summary": "job template run group queued",
        "affected_count": created.runs.len(),
        "resource_type": "playbook_job_template",
        "resource_id": template.id.to_string(),
        "job_template_id": template.id.to_string(),
        "job_template_name": template.name,
        "playbook_id": created.run_group.playbook_id.to_string(),
        "run_group_id": created.run_group.id.to_string(),
        "target_set_id": created.target_set.id.to_string(),
        "trigger_kind": created.run_group.trigger_kind,
        "run_count": created.runs.len(),
        "target_count": created.runs.len(),
        "requested_target_limit": requested_target_limit,
        "status": created.run_group.status,
    });
    if template.playbook_id.is_some() {
        return details;
    }
    merge_metadata(
        details,
        json!({ "project_id": template.project_id.map(|id| id.to_string()) }),
    )
}

/// Queue the run group for an already-authorized job-template launch, record
/// the launch audit event, and shape the `202 Accepted` response.
async fn queue_job_template_launch(
    state: &AppState,
    request_context: &RequestContext,
    template: &PlaybookJobTemplate,
    user_id: Uuid,
    secret_env_authorization: Option<Value>,
    payload: CreateRunGroupRequest,
) -> Response {
    let requested_target_limit = payload.target_limit;
    let input = LaunchJobTemplateInput {
        target_set_id: payload.target_set_id,
        trigger_kind: payload.trigger_kind,
        target_limit: payload.target_limit,
        metadata: payload.metadata,
    };
    match state
        .services
        .playbooks
        .launch_job_template(template.id, input, Some(user_id), secret_env_authorization)
        .await
    {
        Ok(created) => {
            if let Err(error) = record_playbook_audit(
                state,
                request_context,
                "playbook.job_template.launched",
                "playbook_job_template",
                Some(template.id),
                job_template_launch_audit_details(template, &created, requested_target_limit),
            )
            .await
            {
                return handle_app_error(error);
            }
            (StatusCode::ACCEPTED, Json(created)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Handle the project-backed job-template launch path (a template with no
/// `playbook_id` runs an ansible project rather than a piping playbook
/// version). Performs its own project/target/credential RBAC before queuing
/// the run: there is no `token_policy.runtime.secrets.env` for a project-backed
/// template, so this path has no C3 secret gate to enforce.
async fn handle_project_backed_job_template_launch(
    state: &AppState,
    request_context: &RequestContext,
    template: &PlaybookJobTemplate,
    user_id: Uuid,
    payload: CreateRunGroupRequest,
) -> Response {
    if let Err(error) = ensure_access(state, request_context, Action::Execute, None).await {
        return handle_app_error(error);
    }
    if let Some(project_id) = template.project_id {
        let project_resource_id = project_id.to_string();
        if let Err(error) = ensure_project_access(
            state,
            request_context,
            Action::Read,
            Some(&project_resource_id),
        )
        .await
        {
            return handle_app_error(error);
        }
    }
    let requested_target_set_id =
        resolve_launch_target_set_id(payload.target_set_id, template.inventory_target_set_id);
    let authorization_targets = match state
        .services
        .playbooks
        .resolve_target_set_authorization_targets(requested_target_set_id, payload.target_limit)
        .await
    {
        Ok(targets) => targets,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) =
        ensure_authorization_targets_access(state, request_context, &authorization_targets).await
    {
        return handle_app_error(error);
    }

    if let Err(error) = ensure_launch_credential_visibility(
        state,
        request_context,
        template,
        payload.metadata.as_ref(),
    )
    .await
    {
        return handle_app_error(error);
    }
    // Project-backed templates run an ansible project, not a piping playbook
    // version, so there is no `token_policy.runtime.secrets.env` and hence no
    // C3 secret authorization to snapshot.
    queue_job_template_launch(state, request_context, template, user_id, None, payload).await
}

/// Queue a run group from a job template.
#[utoipa::path(
    post,
    path = "/api/v1/playbooks/job-templates/{id}/launch",
    tag = "playbooks",
    params(("id" = Uuid, Path, description = "Job template ID")),
    request_body = CreateRunGroupRequest,
    responses(
        (status = 202, description = "Job template run group queued", body = PlaybookRunGroupCreated),
        (status = 400, description = "Invalid launch payload", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Job template, playbook or target set not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn launch_job_template(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    EnvelopeJson(payload): EnvelopeJson<CreateRunGroupRequest>,
) -> Response {
    let request_context = request_context(&request_id, Some(auth_context), &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(user_id) => user_id,
        Err(error) => return handle_app_error(error),
    };
    let resource_id = id.to_string();
    if let Err(error) =
        ensure_job_template_access(&state, &request_context, Action::Read, Some(&resource_id)).await
    {
        return handle_app_error(error);
    }
    let template = match state.services.playbooks.get_job_template(id).await {
        Ok(Some(template)) => template,
        Ok(None) => {
            return handle_app_error(AppError::NotFound(format!(
                "Playbook job template '{id}' not found"
            )));
        }
        Err(error) => return handle_app_error(error),
    };

    let Some(playbook_id) = template.playbook_id else {
        return handle_project_backed_job_template_launch(
            &state,
            &request_context,
            &template,
            user_id,
            payload,
        )
        .await;
    };

    let catalog_item = match state.services.playbooks.get_catalog_item(playbook_id).await {
        Ok(Some(item)) => item,
        Ok(None) => {
            return handle_app_error(AppError::NotFound(format!(
                "Playbook '{playbook_id}' not found"
            )));
        }
        Err(error) => return handle_app_error(error),
    };
    let playbook = &catalog_item.playbook;
    if let Err(error) = ensure_playbook_run_grant(
        &state,
        &request_context,
        user_id,
        playbook_id,
        playbook.category.as_str(),
        playbook.owner_user_id,
    )
    .await
    {
        return handle_app_error(error);
    }

    let requested_target_set_id =
        resolve_launch_target_set_id(payload.target_set_id, template.inventory_target_set_id);
    let authorization_targets = match state
        .services
        .playbooks
        .resolve_manual_run_authorization_targets(
            playbook_id,
            requested_target_set_id,
            payload.target_limit,
        )
        .await
    {
        Ok(targets) => targets,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) =
        ensure_authorization_targets_access(&state, &request_context, &authorization_targets).await
    {
        return handle_app_error(error);
    }

    if let Err(error) = ensure_launch_credential_visibility(
        &state,
        &request_context,
        &template,
        payload.metadata.as_ref(),
    )
    .await
    {
        return handle_app_error(error);
    }

    // SECURITY (C3): gate every runtime secret the effective version exports and
    // capture the server-authored authorization snapshot for `run_group.metadata`.
    let secret_env = match runtime_secret_env_for_version(catalog_item.current_version.as_ref()) {
        Ok(secret_env) => secret_env,
        Err(error) => return handle_app_error(error),
    };
    let secret_env_authorization = match ensure_secret_use_grants(
        &state,
        &request_context,
        &secret_env,
        &format!("user:{user_id}"),
    )
    .await
    {
        Ok(snapshot) => snapshot,
        Err(error) => return handle_app_error(error),
    };

    queue_job_template_launch(
        &state,
        &request_context,
        &template,
        user_id,
        secret_env_authorization,
        payload,
    )
    .await
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct SecretsPreflightQuery {
    /// Version to preflight; defaults to the playbook's current version (the one
    /// a launch would pin), so a preflight taken with no `version_id` equals the
    /// launch verdict.
    pub version_id: Option<Uuid>,
}

/// Check the secret grants a playbook version needs before launch.
///
/// Read-only; the server still enforces the grants when a run starts.
#[utoipa::path(
    get,
    path = "/api/v1/playbooks/{id}/secrets-preflight",
    tag = "playbooks",
    params(
        ("id" = Uuid, Path, description = "Playbook ID"),
        SecretsPreflightQuery,
    ),
    responses(
        (status = 200, description = "Per-secret verdicts for the caller", body = SecretPreflightResponse),
        (status = 404, description = "Playbook or version not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn playbook_secrets_preflight(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(query): Query<SecretsPreflightQuery>,
) -> Response {
    let request_context = request_context(&request_id, auth_context, &headers);
    // Reading the caller's own launch preconditions is a Read on the playbook —
    // the same gate as `GET /playbooks/{id}` and its static MCP catalog entry.
    let resource_id = playbook_resource_id(id);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Read,
        Some(resource_id.as_str()),
    )
    .await
    {
        return handle_app_error(error);
    }
    let catalog_item = match state.services.playbooks.get_catalog_item(id).await {
        Ok(Some(item)) => item,
        Ok(None) => {
            return handle_app_error(AppError::NotFound(format!("Playbook '{id}' not found")));
        }
        Err(error) => return handle_app_error(error),
    };
    // Resolve the version whose runtime secrets we preflight: an explicit
    // `version_id` (UX may inspect a specific revision), else the current version
    // that a launch would pin.
    let version = match query.version_id {
        Some(version_id) => {
            let versions = match state
                .services
                .playbooks
                .list_playbook_versions(id, 500, 0)
                .await
            {
                Ok(versions) => versions,
                Err(error) => return handle_app_error(error),
            };
            match versions
                .items
                .into_iter()
                .find(|version| version.id == version_id)
            {
                Some(version) => Some(version),
                None => {
                    return handle_app_error(AppError::NotFound(format!(
                        "Playbook version '{version_id}' not found"
                    )));
                }
            }
        }
        None => catalog_item.current_version,
    };
    let secret_env = match runtime_secret_env_for_version(version.as_ref()) {
        Ok(secret_env) => secret_env,
        Err(error) => return handle_app_error(error),
    };
    match secret_use_preflight(&state, &request_context, &secret_env).await {
        Ok(response) => Json(response).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Create a playbook.
#[utoipa::path(
    post,
    path = "/api/v1/playbooks",
    tag = "playbooks",
    request_body = CreatePlaybookRequest,
    responses(
        (status = 201, description = "Playbook created", body = PlaybookCatalogItem),
        (status = 400, description = "Invalid playbook payload", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_playbook(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<CreatePlaybookRequest>,
) -> Response {
    let request_context = request_context(&request_id, Some(auth_context), &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(user_id) => user_id,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = ensure_access(&state, &request_context, Action::Create, None).await {
        return handle_app_error(error);
    }
    // Author-time: reject a playbook whose initial version's declared
    // runtime secrets point at a missing or protected secret before persisting.
    if let Err(error) =
        validate_version_runtime_secret_refs(&state, payload.token_policy.as_ref()).await
    {
        return handle_app_error(error);
    }
    let input = CreatePlaybookInput {
        slug: payload.slug,
        name: payload.name,
        description: payload.description,
        status: payload.status,
        runtime: payload.runtime,
        risk: payload.risk,
        owner_user_id: payload.owner_user_id,
        category: payload.category,
        vars_schema: payload.vars_schema,
        connection_default: payload.connection_default,
        language: payload.language,
        entrypoint: payload.entrypoint,
        content: payload.content,
        input_schema: payload.input_schema,
        output_schema: payload.output_schema,
        token_policy: payload.token_policy,
        timeout_seconds: payload.timeout_seconds,
        changelog: payload.changelog,
        metadata: payload.metadata,
    };
    match state
        .services
        .playbooks
        .create_playbook(input, Some(user_id))
        .await
    {
        Ok(item) => {
            if let Some(version) = item.current_version.as_ref()
                && let Err(error) = project_version_runtime_secret_usage(
                    &state,
                    item.playbook.id,
                    version.id,
                    Some(&version.token_policy),
                )
                .await
            {
                return handle_app_error(error);
            }
            if let Err(error) = record_playbook_audit(
                &state,
                &request_context,
                "playbook.created",
                "playbook",
                Some(item.playbook.id),
                json!({
                    "action": "create",
                    "reason_code": "user_requested_create",
                    "summary": "playbook catalog entry created",
                    "affected_count": 1,
                    "resource_type": "playbook",
                    "resource_id": item.playbook.id.to_string(),
                    "playbook_id": item.playbook.id.to_string(),
                    "slug": item.playbook.slug,
                    "status": item.playbook.status,
                    "runtime": item.playbook.runtime,
                    "risk": item.playbook.risk,
                    "category": item.playbook.category,
                    "current_version_id": item.playbook.current_version_id.map(|value| value.to_string()),
                }),
            )
            .await
            {
                return handle_app_error(error);
            }
            (StatusCode::CREATED, Json(item)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// List immutable versions for a playbook.
#[utoipa::path(
    get,
    path = "/api/v1/playbooks/{id}/versions",
    tag = "playbooks",
    params(
        ListPlaybookVersionsQuery,
        ("id" = Uuid, Path, description = "Playbook ID")
    ),
    responses(
        (status = 200, description = "Playbook versions", body = ListPlaybookVersionsResponse),
        (status = 404, description = "Playbook not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_playbook_versions(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(query): Query<ListPlaybookVersionsQuery>,
) -> Response {
    let request_context = request_context(&request_id, auth_context, &headers);
    if let Err(error) = request_context.require_authenticated_user_id() {
        return handle_app_error(error);
    }
    let resource_id = id.to_string();
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Read,
        Some(resource_id.as_str()),
    )
    .await
    {
        return handle_app_error(error);
    }
    let (limit, offset) = normalize_page(query.limit, query.offset);
    match state
        .services
        .playbooks
        .list_playbook_versions(id, limit, offset)
        .await
    {
        Ok(page) => Json(ListPlaybookVersionsResponse {
            items: page.items,
            total: page.total,
            limit: page.limit,
            offset: page.offset,
        })
        .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Create a new immutable version for a playbook and publish it as current.
#[utoipa::path(
    post,
    path = "/api/v1/playbooks/{id}/versions",
    tag = "playbooks",
    params(("id" = Uuid, Path, description = "Playbook ID")),
    request_body = CreatePlaybookVersionRequest,
    responses(
        (status = 201, description = "Playbook version created", body = PlaybookVersion),
        (status = 400, description = "Invalid playbook version payload", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Playbook not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_playbook_version(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    EnvelopeJson(payload): EnvelopeJson<CreatePlaybookVersionRequest>,
) -> Response {
    let request_context = request_context(&request_id, Some(auth_context), &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(user_id) => user_id,
        Err(error) => return handle_app_error(error),
    };
    let resource_id = id.to_string();
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Update,
        Some(resource_id.as_str()),
    )
    .await
    {
        return handle_app_error(error);
    }
    // Author-time: reject a version whose declared runtime secrets point at
    // a missing or protected secret BEFORE persisting it.
    if let Err(error) =
        validate_version_runtime_secret_refs(&state, payload.token_policy.as_ref()).await
    {
        return handle_app_error(error);
    }
    let input = CreatePlaybookVersionInput {
        language: payload.language,
        runtime: payload.runtime,
        entrypoint: payload.entrypoint,
        content: payload.content,
        input_schema: payload.input_schema,
        output_schema: payload.output_schema,
        token_policy: payload.token_policy,
        timeout_seconds: payload.timeout_seconds,
        changelog: payload.changelog,
        metadata: payload.metadata,
    };
    match state
        .services
        .playbooks
        .create_playbook_version(id, input, Some(user_id))
        .await
    {
        Ok(version) => {
            if let Err(error) = project_version_runtime_secret_usage(
                &state,
                version.playbook_id,
                version.id,
                Some(&version.token_policy),
            )
            .await
            {
                return handle_app_error(error);
            }
            if let Err(error) = record_playbook_audit(
                &state,
                &request_context,
                "playbook.version.created",
                "playbook_version",
                Some(version.id),
                json!({
                    "action": "create_version",
                    "reason_code": "user_published_version",
                    "summary": "playbook version created",
                    "affected_count": 1,
                    "resource_type": "playbook_version",
                    "resource_id": version.id.to_string(),
                    "playbook_id": version.playbook_id.to_string(),
                    "version_id": version.id.to_string(),
                    "version": version.version,
                    "runtime": version.runtime,
                    "language": version.language,
                    "content_sha256": version.content_sha256,
                    "timeout_seconds": version.timeout_seconds,
                }),
            )
            .await
            {
                return handle_app_error(error);
            }
            (StatusCode::CREATED, Json(version)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Update a playbook.
#[utoipa::path(
    patch,
    path = "/api/v1/playbooks/{id}",
    tag = "playbooks",
    params(("id" = Uuid, Path, description = "Playbook ID")),
    request_body = UpdatePlaybookRequest,
    responses(
        (status = 200, description = "Playbook updated", body = PlaybookCatalogItem),
        (status = 404, description = "Playbook not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_playbook(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    EnvelopeJson(payload): EnvelopeJson<UpdatePlaybookRequest>,
) -> Response {
    let request_context = request_context(&request_id, Some(auth_context), &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(user_id) => user_id,
        Err(error) => return handle_app_error(error),
    };
    let resource_id = id.to_string();
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Update,
        Some(resource_id.as_str()),
    )
    .await
    {
        return handle_app_error(error);
    }
    let input = UpdatePlaybookInput {
        slug: payload.slug,
        name: payload.name,
        description: payload.description,
        status: payload.status,
        runtime: payload.runtime,
        risk: payload.risk,
        owner_user_id: payload.owner_user_id,
        default_target_set_id: payload.default_target_set_id,
        category: payload.category,
        vars_schema: payload.vars_schema,
        connection_default: payload.connection_default,
        metadata: payload.metadata,
    };
    match state
        .services
        .playbooks
        .update_playbook(id, input, Some(user_id))
        .await
    {
        Ok(Some(item)) => {
            if let Err(error) = record_playbook_audit(
                &state,
                &request_context,
                "playbook.updated",
                "playbook",
                Some(item.playbook.id),
                json!({
                    "action": "update",
                    "reason_code": "user_requested_update",
                    "summary": "playbook catalog entry updated",
                    "affected_count": 1,
                    "resource_type": "playbook",
                    "resource_id": item.playbook.id.to_string(),
                    "playbook_id": item.playbook.id.to_string(),
                    "slug": item.playbook.slug,
                    "status": item.playbook.status,
                    "runtime": item.playbook.runtime,
                    "risk": item.playbook.risk,
                    "category": item.playbook.category,
                    "current_version_id": item.playbook.current_version_id.map(|value| value.to_string()),
                }),
            )
            .await
            {
                return handle_app_error(error);
            }
            Json(item).into_response()
        }
        Ok(None) => handle_app_error(AppError::NotFound(format!("Playbook '{id}' not found"))),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a playbook.
#[utoipa::path(
    delete,
    path = "/api/v1/playbooks/{id}",
    tag = "playbooks",
    params(("id" = Uuid, Path, description = "Playbook ID")),
    responses(
        (status = 204, description = "Playbook deleted"),
        (status = 404, description = "Playbook not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 409, description = "Playbook has run history (DELETE_BLOCKED with blocker tables, counts, and examples)", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_playbook(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let request_context = request_context(&request_id, auth_context, &headers);
    if let Err(error) = request_context.require_authenticated_user_id() {
        return handle_app_error(error);
    }
    let resource_id = id.to_string();
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Delete,
        Some(resource_id.as_str()),
    )
    .await
    {
        return handle_app_error(error);
    }
    match state.services.playbooks.delete_playbook(id).await {
        Ok(true) => {
            if let Err(error) = record_playbook_audit(
                &state,
                &request_context,
                "playbook.deleted",
                "playbook",
                Some(id),
                json!({
                    "action": "delete",
                    "reason_code": "user_requested_delete",
                    "summary": "playbook catalog entry deleted",
                    "affected_count": 1,
                    "resource_type": "playbook",
                    "resource_id": id.to_string(),
                    "playbook_id": id.to_string(),
                    "deleted": true,
                }),
            )
            .await
            {
                return handle_app_error(error);
            }
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => handle_app_error(AppError::NotFound(format!("Playbook '{id}' not found"))),
        Err(error) => handle_app_error(error),
    }
}

/// Load a playbook catalog item or fail with the canonical not-found error.
async fn load_playbook_catalog_item(
    state: &AppState,
    playbook_id: Uuid,
) -> Result<PlaybookCatalogItem, AppError> {
    match state.services.playbooks.get_catalog_item(playbook_id).await {
        Ok(Some(item)) => Ok(item),
        Ok(None) => Err(AppError::NotFound(format!(
            "Playbook '{playbook_id}' not found"
        ))),
        Err(error) => Err(error),
    }
}

/// Authorize an explicit credential reference carried by a manual run payload.
async fn ensure_run_credential_use_allowed(
    state: &AppState,
    request_context: &RequestContext,
    metadata: Option<&Value>,
) -> Result<(), AppError> {
    // SECURITY (C2): if the run group references an explicit credential account /
    // shared-credential-set, the subject must be allowed to use it before creation
    // (the persisted decision is resolved by the worker at run start without
    // re-checking). Mirror the connection credential path's secret-visibility gate.
    let Some(credential_reference) = explicit_run_credential_reference(metadata) else {
        return Ok(());
    };
    // C2 (ceiling-aware): gate `resolve_secret_visibility` behind the PAT
    // token-scope ceiling so a scoped token cannot exceed its declared scope.
    let can_use_credential = if !credential_use_scope_allowed(request_context) {
        false
    } else {
        resolve_secret_visibility(
            Some(state.services.access_control.as_ref()),
            Some(request_context),
        )
        .await?
    };
    if !can_use_credential {
        return Err(AppError::Forbidden(format!(
            "Not authorized to use referenced credential {} ({})",
            credential_reference.account_id, credential_reference.account_kind
        )));
    }
    Ok(())
}

/// Authorize every secret the effective version exports into the run env (C3).
async fn authorize_run_secret_env(
    state: &AppState,
    request_context: &RequestContext,
    version: Option<&PlaybookVersion>,
    user_id: Uuid,
) -> Result<Option<Value>, AppError> {
    // SECURITY (C3): every secret the effective version exports into the run env
    // requires an Execute × SecretVariable grant for the requesting subject. The
    // returned snapshot is server-authored into `run_group.metadata` (no values).
    let secret_env = runtime_secret_env_for_version(version)?;
    ensure_secret_use_grants(
        state,
        request_context,
        &secret_env,
        &format!("user:{user_id}"),
    )
    .await
}

/// Queue a playbook run group.
#[utoipa::path(
    post,
    path = "/api/v1/playbooks/{id}/runs",
    tag = "playbooks",
    params(("id" = Uuid, Path, description = "Playbook ID")),
    request_body = CreateRunGroupRequest,
    responses(
        (status = 202, description = "Playbook run group queued", body = PlaybookRunGroupCreated),
        (status = 400, description = "Invalid run payload", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Playbook or target set not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_run_group(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    EnvelopeJson(payload): EnvelopeJson<CreateRunGroupRequest>,
) -> Response {
    let request_context = request_context(&request_id, Some(auth_context), &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(user_id) => user_id,
        Err(error) => return handle_app_error(error),
    };
    // Composite run-authz (legs a+b): the caller needs the general run right AND a
    // grant on THIS playbook OR its category (or to own it). Fetch the playbook so
    // category + owner are available without inline SQL (handlers don't run SQL).
    let catalog_item = match load_playbook_catalog_item(&state, id).await {
        Ok(item) => item,
        Err(error) => return handle_app_error(error),
    };
    let playbook = &catalog_item.playbook;
    if let Err(error) = ensure_playbook_run_grant(
        &state,
        &request_context,
        user_id,
        id,
        playbook.category.as_str(),
        playbook.owner_user_id,
    )
    .await
    {
        return handle_app_error(error);
    }
    // SECURITY (C1): resolve the concrete fan-out the same way
    // `create_run_group` will (effective target set incl. the playbook's default,
    // and the same target_limit), then authorize the subject against the resolved
    // target set AND every resolved host before any run rows are persisted. The
    // (future) worker trusts these decisions, so they must be enforced at creation
    // time and fail-closed: a single unauthorized host rejects the whole request.
    let authorization_targets = match state
        .services
        .playbooks
        .resolve_manual_run_authorization_targets(id, payload.target_set_id, payload.target_limit)
        .await
    {
        Ok(targets) => targets,
        Err(error) => return handle_app_error(error),
    };
    // C1: authorize the effective target set and every resolved host through
    // the one batched implementation used by both manual and template launches.
    if let Err(error) =
        ensure_authorization_targets_access(&state, &request_context, &authorization_targets).await
    {
        return handle_app_error(error);
    }
    // SECURITY (C2): explicit credential references must be authorized before
    // creation; C3 authorizes every secret the effective version exports.
    if let Err(error) =
        ensure_run_credential_use_allowed(&state, &request_context, payload.metadata.as_ref()).await
    {
        return handle_app_error(error);
    }
    let secret_env_authorization = match authorize_run_secret_env(
        &state,
        &request_context,
        catalog_item.current_version.as_ref(),
        user_id,
    )
    .await
    {
        Ok(snapshot) => snapshot,
        Err(error) => return handle_app_error(error),
    };
    let input = CreateRunGroupInput {
        target_set_id: payload.target_set_id,
        schedule_id: None,
        scheduled_for: None,
        trigger_kind: payload.trigger_kind,
        target_limit: payload.target_limit,
        metadata: payload.metadata,
        secret_env_authorization,
    };
    match state
        .services
        .playbooks
        .create_manual_run_group(id, input, Some(user_id))
        .await
    {
        Ok(created) => {
            if let Err(error) = record_playbook_audit(
                &state,
                &request_context,
                "playbook.run_group.queued",
                "playbook_run_group",
                Some(created.run_group.id),
                json!({
                    "action": "run",
                    "reason_code": "manual_run_requested",
                    "summary": "playbook run group queued",
                    "affected_count": created.runs.len(),
                    "resource_type": "playbook_run_group",
                    "resource_id": created.run_group.id.to_string(),
                    "playbook_id": created.run_group.playbook_id.to_string(),
                    "run_group_id": created.run_group.id.to_string(),
                    "target_set_id": created.target_set.id.to_string(),
                    "trigger_kind": created.run_group.trigger_kind,
                    "run_count": created.runs.len(),
                    "target_count": created.runs.len(),
                    "requested_target_limit": payload.target_limit,
                    "status": created.run_group.status,
                }),
            )
            .await
            {
                return handle_app_error(error);
            }
            (StatusCode::ACCEPTED, Json(created)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Create a playbook run artifact through a run-bound job token.
#[utoipa::path(
    post,
    path = "/api/v1/playbooks/runs/{run_id}/artifacts",
    tag = "playbooks",
    params(("run_id" = Uuid, Path, description = "Playbook run ID")),
    request_body = CreateArtifactRequest,
    responses(
        (status = 201, description = "Artifact created", body = PlaybookArtifact),
        (status = 400, description = "Invalid artifact payload", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Invalid or expired job token", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Job token scope denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 429, description = "Callback quota exceeded", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_run_artifact(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(run_id): Path<Uuid>,
    EnvelopeJson(payload): EnvelopeJson<CreateArtifactRequest>,
) -> Response {
    let raw_token = match bearer_token(&headers) {
        Ok(token) => token,
        Err(error) => return handle_app_error(error),
    };
    let input = CreateArtifactInput {
        artifact_type: payload.artifact_type,
        name: payload.name,
        content_type: payload.content_type,
        content_json: payload.content_json,
        content_text: payload.content_text,
    };
    match state
        .services
        .playbooks
        .create_artifact_with_job_token(run_id, raw_token.as_str(), input, Utc::now())
        .await
    {
        Ok(artifact) => {
            record_playbook_job_token_audit(
                &state,
                "playbook.artifact.created",
                "playbook_artifact",
                Some(artifact.id),
                raw_token.as_str(),
                json!({
                    "action": "callback_artifact_create",
                    "reason_code": "job_token_artifact_callback",
                    "summary": "playbook artifact callback accepted",
                    "affected_count": 1,
                    "resource_type": "playbook_artifact",
                    "resource_id": artifact.id.to_string(),
                    "run_id": artifact.run_id.to_string(),
                    "artifact_id": artifact.id.to_string(),
                    "artifact_type": artifact.artifact_type,
                    "name": artifact.name,
                    "content_type": artifact.content_type,
                    "content_size_bytes": artifact.content_size_bytes,
                    "has_content_json": artifact.content_json.is_some(),
                    "has_content_text": artifact.content_text.is_some(),
                }),
            )
            .await;
            (StatusCode::CREATED, Json(artifact)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Append a playbook run event through a run-bound job token.
#[utoipa::path(
    post,
    path = "/api/v1/playbooks/runs/{run_id}/events",
    tag = "playbooks",
    params(("run_id" = Uuid, Path, description = "Playbook run ID")),
    request_body = CreateRunEventRequest,
    responses(
        (status = 201, description = "Run event appended", body = PlaybookRunEvent),
        (status = 400, description = "Invalid event payload", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Invalid or expired job token", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Job token scope denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 429, description = "Callback quota exceeded", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_run_event(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(run_id): Path<Uuid>,
    EnvelopeJson(payload): EnvelopeJson<CreateRunEventRequest>,
) -> Response {
    let raw_token = match bearer_token(&headers) {
        Ok(token) => token,
        Err(error) => return handle_app_error(error),
    };
    let input = CreateRunEventInput {
        event_kind: payload.event_kind,
        event_type: payload.event_type,
        message: payload.message,
        payload: payload.payload,
    };
    match state
        .services
        .playbooks
        .append_run_event_with_job_token(run_id, raw_token.as_str(), input, Utc::now())
        .await
    {
        Ok(event) => {
            record_playbook_job_token_audit(
                &state,
                "playbook.run_event.appended",
                "playbook_run_event",
                Some(event.id),
                raw_token.as_str(),
                json!({
                    "action": "callback_event_append",
                    "reason_code": "job_token_event_callback",
                    "summary": "playbook run event callback accepted",
                    "affected_count": 1,
                    "resource_type": "playbook_run_event",
                    "resource_id": event.id.to_string(),
                    "run_id": event.run_id.map(|run_id| run_id.to_string()),
                    "event_id": event.id.to_string(),
                    "event_kind": event.event_kind,
                    "event_type": event.event_type,
                    "has_message": event.message.is_some(),
                    "has_payload": !event.payload.is_null(),
                }),
            )
            .await;
            // Fan the persisted event out to any live run-stream subscribers.
            // Best-effort: a failed send (no subscribers) must not fail the
            // callback — the row is durable and replayed on the next connect.
            publish_run_event_to_live_streams(&state, &event).await;
            (StatusCode::CREATED, Json(event)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Append a playbook run-group event through its dedicated group callback token.
#[utoipa::path(
    post,
    path = "/api/v1/playbooks/run-groups/{group_id}/events",
    tag = "playbooks",
    params(("group_id" = Uuid, Path, description = "Playbook run group ID")),
    request_body = CreateRunGroupEventRequest,
    responses(
        (status = 201, description = "Run-group event appended", body = PlaybookRunEvent),
        (status = 400, description = "Invalid event payload", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Invalid or expired job token", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Job token scope denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 429, description = "Callback quota exceeded", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_run_group_event(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(group_id): Path<Uuid>,
    EnvelopeJson(payload): EnvelopeJson<CreateRunGroupEventRequest>,
) -> Response {
    let raw_token = match bearer_token(&headers) {
        Ok(token) => token,
        Err(error) => return handle_app_error(error),
    };
    let input = CreateRunGroupEventInput {
        run_id: payload.run_id,
        host: payload.host,
        event_kind: payload.event_kind,
        event_type: payload.event_type,
        message: payload.message,
        payload: payload.payload,
    };
    match state
        .services
        .playbooks
        .append_run_group_event_with_group_callback_token(
            group_id,
            raw_token.as_str(),
            input,
            Utc::now(),
        )
        .await
    {
        Ok(event) => {
            record_playbook_job_token_audit(
                &state,
                "playbook.run_group_event.appended",
                "playbook_run_event",
                Some(event.id),
                raw_token.as_str(),
                json!({
                    "action": "callback_group_event_append",
                    "reason_code": "job_token_event_callback",
                    "summary": "playbook run-group event callback accepted",
                    "affected_count": 1,
                    "resource_type": "playbook_run_event",
                    "resource_id": event.id.to_string(),
                    "run_group_id": event.run_group_id.to_string(),
                    "run_id": event.run_id.map(|run_id| run_id.to_string()),
                    "host": event.host,
                    "event_id": event.id.to_string(),
                    "event_kind": event.event_kind,
                    "event_type": event.event_type,
                    "has_message": event.message.is_some(),
                    "has_payload": !event.payload.is_null(),
                }),
            )
            .await;
            publish_run_event_to_live_streams(&state, &event).await;
            (StatusCode::CREATED, Json(event)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Append multiple run-group events through its dedicated group callback token.
#[utoipa::path(
    post,
    path = "/api/v1/playbooks/run-groups/{group_id}/events/batch",
    tag = "playbooks",
    params(("group_id" = Uuid, Path, description = "Playbook run group ID")),
    request_body = CreateRunGroupEventsBatchRequest,
    responses(
        (status = 200, description = "Event batch already ingested", body = Vec<PlaybookRunEvent>),
        (status = 201, description = "Run-group events appended", body = Vec<PlaybookRunEvent>),
        (status = 400, description = "Invalid event payload", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Invalid or expired job token", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Job token scope denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 429, description = "Callback quota exceeded", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_run_group_events_batch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(group_id): Path<Uuid>,
    EnvelopeJson(payload): EnvelopeJson<CreateRunGroupEventsBatchRequest>,
) -> Response {
    if let Err(error) = payload.validate() {
        return handle_app_error(error);
    }
    let raw_token = match bearer_token(&headers) {
        Ok(token) => token,
        Err(error) => return handle_app_error(error),
    };
    let batch_id = payload.batch_id;
    let inputs = payload
        .events
        .into_iter()
        .map(|event| CreateRunGroupEventInput {
            run_id: event.run_id,
            host: event.host,
            event_kind: event.event_kind,
            event_type: event.event_type,
            message: event.message,
            payload: event.payload,
        })
        .collect();
    match state
        .services
        .playbooks
        .append_run_group_events_with_group_callback_token(
            group_id,
            batch_id,
            raw_token.as_str(),
            inputs,
            Utc::now(),
        )
        .await
    {
        Ok(AppendRunGroupEventBatchOutcome::Created(events)) => {
            let affected_count = events.len();
            if affected_count > 0 {
                record_playbook_job_token_audit(
                    &state,
                    "playbook.run_group_events.appended",
                    "playbook_run_event",
                    events.first().map(|event| event.id),
                    raw_token.as_str(),
                    json!({
                        "action": "callback_group_events_append",
                        "reason_code": "job_token_event_callback",
                        "summary": "playbook run-group event batch callback accepted",
                        "affected_count": affected_count,
                        "resource_type": "playbook_run_event",
                        "run_group_id": group_id.to_string(),
                        "first_event_id": events.first().map(|event| event.id.to_string()),
                        "last_event_id": events.last().map(|event| event.id.to_string()),
                    }),
                )
                .await;
            }
            for event in &events {
                publish_run_event_to_live_streams(&state, event).await;
            }
            (StatusCode::CREATED, Json(events)).into_response()
        }
        Ok(AppendRunGroupEventBatchOutcome::AlreadyIngested) => {
            (StatusCode::OK, Json(Vec::<PlaybookRunEvent>::new())).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Complete multiple playbook runs from one grouped runner callback.
#[utoipa::path(
    post,
    path = "/api/v1/playbooks/run-groups/{group_id}/completion-batch",
    tag = "playbooks",
    params(("group_id" = Uuid, Path, description = "Playbook run group ID")),
    request_body = CompleteRunGroupBatchRequest,
    responses(
        (status = 200, description = "Run group completions accepted", body = PlaybookRunGroupCompletionBatch),
        (status = 400, description = "Invalid completion payload", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Invalid or expired job token", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Job token scope denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 429, description = "Callback quota exceeded", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn complete_run_group_batch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(group_id): Path<Uuid>,
    EnvelopeJson(payload): EnvelopeJson<CompleteRunGroupBatchRequest>,
) -> Response {
    if let Err(error) = payload.validate() {
        return handle_app_error(error);
    }
    let raw_token = match bearer_token(&headers) {
        Ok(token) => token,
        Err(error) => return handle_app_error(error),
    };
    let input = CompleteRunGroupBatchInput {
        lease_owner: payload.lease_owner,
        runs: payload
            .runs
            .into_iter()
            .map(|run| CompleteRunGroupBatchItemInput {
                run_id: run.run_id,
                status: run.status,
                exit_code: run.exit_code,
                error_message: run.error_message,
            })
            .collect(),
        new_host_keys: payload.new_host_keys,
    };
    match state
        .services
        .playbooks
        .complete_run_group_with_group_callback_token(
            group_id,
            raw_token.as_str(),
            input,
            Utc::now(),
        )
        .await
    {
        Ok(completion) => {
            let affected_count = completion.completed_runs.len();
            if affected_count > 0 {
                record_playbook_job_token_audit(
                    &state,
                    "playbook.run_group.completed",
                    "playbook_run_group",
                    Some(group_id),
                    raw_token.as_str(),
                    json!({
                        "action": "callback_run_group_complete",
                        "reason_code": "job_token_completion_callback",
                        "summary": "playbook run-group completion batch accepted",
                        "affected_count": affected_count,
                        "resource_type": "playbook_run_group",
                        "resource_id": group_id.to_string(),
                        "run_group_id": group_id.to_string(),
                        "status": completion.run_group.status,
                    }),
                )
                .await;
            }

            for run in &completion.completed_runs {
                state.run_event_broadcaster.publish(
                    run.id,
                    RunStreamFrame::Completion {
                        run_id: run.id,
                        status: run.status.as_str().to_string(),
                        exit_code: run.exit_code,
                        error_message: run.error_message.clone(),
                    },
                );
            }

            Json(completion).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Complete a playbook run through a run-bound job token.
#[utoipa::path(
    post,
    path = "/api/v1/playbooks/runs/{run_id}/completion",
    tag = "playbooks",
    params(("run_id" = Uuid, Path, description = "Playbook run ID")),
    request_body = CompleteRunRequest,
    responses(
        (status = 200, description = "Run completed", body = PlaybookRunCompletion),
        (status = 400, description = "Invalid completion payload", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Invalid or expired job token", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Job token scope denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 429, description = "Callback quota exceeded", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn complete_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(run_id): Path<Uuid>,
    EnvelopeJson(payload): EnvelopeJson<CompleteRunRequest>,
) -> Response {
    let raw_token = match bearer_token(&headers) {
        Ok(token) => token,
        Err(error) => return handle_app_error(error),
    };
    let input = CompleteRunInput {
        run_id,
        lease_owner: payload.lease_owner,
        status: payload.status,
        exit_code: payload.exit_code,
        error_message: payload.error_message,
    };
    match state
        .services
        .playbooks
        .complete_run_with_job_token(run_id, raw_token.as_str(), input, Utc::now())
        .await
    {
        // `newly_completed` is `false` when the run was already terminal on
        // arrival (a late/duplicate callback, e.g. one racing an operator
        // cancel). The repository returned the existing state as an idempotent
        // no-op, so we must NOT re-audit a "completed" event nor re-publish a
        // completion frame (which would mislabel a `Canceled` run or strand a
        // double-close on the socket). Return the current state verbatim.
        Ok((completion, newly_completed)) => {
            if !newly_completed {
                return Json(completion).into_response();
            }
            record_playbook_job_token_audit(
                &state,
                "playbook.run.completed",
                "playbook_run",
                Some(completion.run.id),
                raw_token.as_str(),
                json!({
                    "action": "callback_run_complete",
                    "reason_code": "job_token_completion_callback",
                    "summary": "playbook run completion callback accepted",
                    "affected_count": 1,
                    "resource_type": "playbook_run",
                    "resource_id": completion.run.id.to_string(),
                    "run_id": completion.run.id.to_string(),
                    "run_group_id": completion.run.run_group_id.to_string(),
                    "playbook_id": completion.run.playbook_id.to_string(),
                    "target_id": completion.run.target_id.to_string(),
                    "target_kind": completion.run.target_kind,
                    "status": completion.run.status,
                    "exit_code": completion.run.exit_code,
                    "has_error_message": completion.run.error_message.is_some(),
                }),
            )
            .await;
            // Signal terminality to live run-stream subscribers so they can
            // close the socket. Best-effort: no subscribers is fine.
            state.run_event_broadcaster.publish(
                completion.run.id,
                RunStreamFrame::Completion {
                    run_id: completion.run.id,
                    status: completion.run.status.as_str().to_string(),
                    exit_code: completion.run.exit_code,
                    error_message: completion.run.error_message.clone(),
                },
            );
            Json(completion).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Authorize a cancel request against the run's playbook using the same
/// composite run grant that gates launching it ("you can stop what you can
/// run"): fetch the playbook for category + owner, then `ensure_playbook_run_grant`.
async fn authorize_cancel_for_playbook(
    state: &AppState,
    request_context: &RequestContext,
    caller: Uuid,
    playbook_id: Uuid,
) -> Result<(), AppError> {
    let playbook = match state
        .services
        .playbooks
        .get_catalog_item(playbook_id)
        .await?
    {
        Some(item) => item.playbook,
        None => return Err(AppError::NotFound("Playbook not found".to_string())),
    };
    ensure_playbook_run_grant(
        state,
        request_context,
        caller,
        playbook_id,
        playbook.category.as_str(),
        playbook.owner_user_id,
    )
    .await
}

/// Best-effort live fan-out for a persisted run event.
///
/// Normal run events publish to their one run stream. Group-level events
/// (`run_id = NULL`) are durable once and mirrored to every member run stream so
/// an already-open per-target UI sees PLAY/TASK/raw controller output without
/// waiting for a REST replay.
async fn publish_run_event_to_live_streams(state: &AppState, event: &PlaybookRunEvent) {
    if let Some(run_id) = event.run_id {
        state
            .run_event_broadcaster
            .publish(run_id, RunStreamFrame::event_for_run(event, run_id));
        return;
    }

    let group = match state
        .services
        .playbooks
        .get_run_group_with_runs(event.run_group_id, PLAYBOOK_RUN_CANCEL_MEMBER_LIMIT)
        .await
    {
        Ok(Some(group)) => group,
        Ok(None) => return,
        Err(error) => {
            log_error_event(
                "playbook.run_event.live_publish_failed",
                "playbook_run_event_publish_error",
                error.to_string(),
                EventFields::default()
                    .with_operation("playbook_run_event_live_publish")
                    .with_resource("playbook_run_group", event.run_group_id.to_string()),
            );
            return;
        }
    };
    for run in &group.runs {
        state
            .run_event_broadcaster
            .publish(run.id, RunStreamFrame::event_for_run(event, run.id));
    }
}

/// Publish a `canceled` completion frame to any live run-stream subscriber for
/// each run this request just terminalized (`status == Canceled`), mirroring the
/// completion-callback publish in `complete_run`. This lets a subscribed socket
/// close immediately and the FE's STOP reflect instantly instead of waiting on a
/// sidecar callback that may never arrive. Best-effort: a run with no subscriber
/// is a no-op (the terminal state is durable and replayed on the next connect).
fn publish_cancel_completions(state: &AppState, runs: &[PlaybookRun]) {
    for run in runs
        .iter()
        .filter(|run| run.status == PlaybookRunStatus::Canceled)
    {
        state.run_event_broadcaster.publish(
            run.id,
            RunStreamFrame::Completion {
                run_id: run.id,
                status: PlaybookRunStatus::Canceled.as_str().to_string(),
                exit_code: None,
                error_message: Some("canceled by operator".to_string()),
            },
        );
    }
}

/// A non-terminal member of a multi-run Ansible execution cannot be canceled
/// independently because the controller dispatch is shared by the whole group.
/// Terminal members remain accepted so cancellation retries stay idempotent.
fn ensure_single_run_cancel_allowed(
    total_runs: i32,
    status: PlaybookRunStatus,
) -> Result<(), AppError> {
    if total_runs > 1 && !status.is_terminal() {
        return Err(AppError::Conflict(
            "Grouped Ansible executions can only be canceled as a whole run group".to_string(),
        ));
    }
    Ok(())
}

/// Request cancellation of a single playbook run.
#[utoipa::path(
    post,
    path = "/api/v1/playbooks/runs/{run_id}/cancel",
    tag = "playbooks",
    params(("run_id" = Uuid, Path, description = "Playbook run ID")),
    responses(
        (status = 200, description = "Run cancellation requested", body = PlaybookRun),
        (status = 404, description = "Run not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 409, description = "Run belongs to a grouped Ansible execution", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn cancel_run(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(run_id): Path<Uuid>,
) -> Response {
    let request_context = request_context(&request_id, auth_context, &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(user_id) => user_id,
        Err(error) => return handle_app_error(error),
    };
    // Load the run first so we can authorize against its playbook (no inline SQL
    // — fetch through the playbook service).
    let run = match state.services.playbooks.get_run(run_id).await {
        Ok(Some(run)) => run,
        Ok(None) => {
            return handle_app_error(AppError::NotFound(format!(
                "Playbook run '{run_id}' not found"
            )));
        }
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) =
        authorize_cancel_for_playbook(&state, &request_context, user_id, run.playbook_id).await
    {
        return handle_app_error(error);
    }
    let run_group = match state
        .services
        .playbooks
        .get_run_group_with_runs(run.run_group_id, 1)
        .await
    {
        Ok(Some(detail)) => detail.run_group,
        Ok(None) => {
            return handle_app_error(AppError::NotFound(format!(
                "Playbook run group '{}' not found",
                run.run_group_id
            )));
        }
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = ensure_single_run_cancel_allowed(run_group.total_runs, run.status) {
        return handle_app_error(error);
    }
    // Queued/leased work terminalizes here because it was never dispatched.
    // Dispatched work only persists the durable command; the cancel worker owns
    // terminalization after runner confirmation or deadline expiry.
    let updated = match state.services.playbooks.request_run_cancel(run_id).await {
        Ok(Some(run)) => run,
        Ok(None) => {
            return handle_app_error(AppError::NotFound(format!(
                "Playbook run '{run_id}' not found"
            )));
        }
        Err(error) => return handle_app_error(error),
    };
    // Only the undispatched path is terminal at request time. The worker or the
    // normal runner completion callback publishes dispatched terminal states.
    publish_cancel_completions(&state, std::slice::from_ref(&updated));
    if let Err(error) = record_playbook_audit(
        &state,
        &request_context,
        "playbook.run.cancel_requested",
        "playbook_run",
        Some(updated.id),
        json!({
            "action": "cancel",
            "reason_code": "manual_cancel_requested",
            "summary": "playbook run cancellation requested",
            "affected_count": 1,
            "resource_type": "playbook_run",
            "resource_id": updated.id.to_string(),
            "run_id": updated.id.to_string(),
            "run_group_id": updated.run_group_id.to_string(),
            "playbook_id": updated.playbook_id.to_string(),
            "status": updated.status,
            "dispatch_id": run_group.dispatch_id,
            "cancel_requested_at": updated.cancel_requested_at,
            "cancel_outcome": updated.cancel_outcome,
        }),
    )
    .await
    {
        return handle_app_error(error);
    }
    Json(updated).into_response()
}

/// Request cancellation of every active run in a playbook run group.
#[utoipa::path(
    post,
    path = "/api/v1/playbooks/run-groups/{group_id}/cancel",
    tag = "playbooks",
    params(("group_id" = Uuid, Path, description = "Playbook run group ID")),
    responses(
        (status = 200, description = "Run group cancellation requested", body = PlaybookRunGroupDetail),
        (status = 404, description = "Run group not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn cancel_run_group(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(group_id): Path<Uuid>,
) -> Response {
    let request_context = request_context(&request_id, auth_context, &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(user_id) => user_id,
        Err(error) => return handle_app_error(error),
    };
    // Load the group to authorize against its playbook before stamping members.
    let group = match state
        .services
        .playbooks
        .get_run_group_with_runs(group_id, PLAYBOOK_RUN_CANCEL_MEMBER_LIMIT)
        .await
    {
        Ok(Some(detail)) => detail,
        Ok(None) => {
            return handle_app_error(AppError::NotFound(format!(
                "Playbook run group '{group_id}' not found"
            )));
        }
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = authorize_cancel_for_playbook(
        &state,
        &request_context,
        user_id,
        group.run_group.playbook_id,
    )
    .await
    {
        return handle_app_error(error);
    }
    let updated = match state
        .services
        .playbooks
        .request_run_group_cancel(group_id, PLAYBOOK_RUN_CANCEL_MEMBER_LIMIT)
        .await
    {
        Ok(Some(detail)) => detail,
        Ok(None) => {
            return handle_app_error(AppError::NotFound(format!(
                "Playbook run group '{group_id}' not found"
            )));
        }
        Err(error) => return handle_app_error(error),
    };
    // Only an undispatched group is terminal at request time.
    publish_cancel_completions(&state, &updated.runs);
    if let Err(error) = record_playbook_audit(
        &state,
        &request_context,
        "playbook.run_group.cancel_requested",
        "playbook_run_group",
        Some(updated.run_group.id),
        json!({
            "action": "cancel",
            "reason_code": "manual_cancel_requested",
            "summary": "playbook run group cancellation requested",
            "affected_count": updated.runs.len(),
            "resource_type": "playbook_run_group",
            "resource_id": updated.run_group.id.to_string(),
            "run_group_id": updated.run_group.id.to_string(),
            "playbook_id": updated.run_group.playbook_id.to_string(),
            "status": updated.run_group.status,
            "dispatch_id": updated.run_group.dispatch_id,
            "cancel_requested_at": updated.run_group.cancel_requested_at,
            "cancel_deadline_at": updated.run_group.cancel_deadline_at,
            "cancel_outcome": updated.run_group.cancel_outcome,
        }),
    )
    .await
    {
        return handle_app_error(error);
    }
    Json(updated).into_response()
}

/// List playbook schedules.
#[utoipa::path(
    get,
    path = "/api/v1/playbooks/schedules",
    tag = "playbooks",
    params(ListPlaybookSchedulesQuery),
    responses(
        (status = 200, description = "Playbook schedules", body = ListPlaybookSchedulesResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_schedules(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ListPlaybookSchedulesQuery>,
) -> Response {
    let request_context = request_context(&request_id, auth_context, &headers);
    if let Err(error) = request_context.require_authenticated_user_id() {
        return handle_app_error(error);
    }
    if let Err(error) = ensure_schedule_access(&state, &request_context, Action::Read, None).await {
        return handle_app_error(error);
    }
    let enabled = match parse_csv_bools(query.enabled.as_deref()) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let playbook_id = match parse_csv_uuids(query.playbook_id.as_deref(), "playbook_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let target_set_id = match parse_csv_uuids(query.target_set_id.as_deref(), "target_set_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let concurrency_policy =
        match parse_csv_schedule_concurrency_policies(query.concurrency_policy.as_deref()) {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        PLAYBOOK_SCHEDULE_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let filter = PlaybookScheduleListFilter {
        playbook_id,
        target_set_id,
        concurrency_policy,
        enabled,
        sort,
    };
    let limit = query.limit.unwrap_or(100);
    let offset = query.offset.unwrap_or(0);
    match state
        .services
        .playbooks
        .list_schedules(&filter, limit, offset)
        .await
    {
        Ok((items, total)) => Json(ListPlaybookSchedulesResponse {
            items,
            total,
            limit,
            offset,
        })
        .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Get a playbook schedule.
#[utoipa::path(
    get,
    path = "/api/v1/playbooks/schedules/{id}",
    tag = "playbooks",
    params(("id" = Uuid, Path, description = "Schedule ID")),
    responses(
        (status = 200, description = "Playbook schedule", body = PlaybookSchedule),
        (status = 404, description = "Schedule not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_schedule(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let request_context = request_context(&request_id, auth_context, &headers);
    if let Err(error) = request_context.require_authenticated_user_id() {
        return handle_app_error(error);
    }
    let resource_id = id.to_string();
    if let Err(error) =
        ensure_schedule_access(&state, &request_context, Action::Read, Some(&resource_id)).await
    {
        return handle_app_error(error);
    }
    match state.services.playbooks.get_schedule(id).await {
        Ok(Some(schedule)) => Json(schedule).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound(format!(
            "Playbook schedule '{id}' not found"
        ))),
        Err(error) => handle_app_error(error),
    }
}

/// Create a playbook schedule.
#[utoipa::path(
    post,
    path = "/api/v1/playbooks/schedules",
    tag = "playbooks",
    request_body = CreatePlaybookScheduleRequest,
    responses(
        (status = 201, description = "Playbook schedule created", body = PlaybookSchedule),
        (status = 400, description = "Invalid schedule payload", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Playbook, target set, or service principal not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_schedule(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<CreatePlaybookScheduleRequest>,
) -> Response {
    let request_context = request_context(&request_id, Some(auth_context), &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(user_id) => user_id,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = ensure_schedule_access(&state, &request_context, Action::Create, None).await
    {
        return handle_app_error(error);
    }
    // Composite run-authz parity with manual runs: the schedule author needs the
    // general run right AND a grant on THIS playbook OR its category (or to own
    // it), not merely the service-principal Execute right. Fetch the playbook so
    // category + owner are available without inline SQL (handlers don't run SQL).
    let scheduled_item = match state
        .services
        .playbooks
        .get_catalog_item(payload.playbook_id)
        .await
    {
        Ok(Some(item)) => item,
        Ok(None) => {
            return handle_app_error(AppError::NotFound(format!(
                "Playbook '{}' not found",
                payload.playbook_id
            )));
        }
        Err(error) => return handle_app_error(error),
    };
    let scheduled_playbook = &scheduled_item.playbook;
    if let Err(error) = ensure_playbook_run_grant(
        &state,
        &request_context,
        user_id,
        payload.playbook_id,
        scheduled_playbook.category.as_str(),
        scheduled_playbook.owner_user_id,
    )
    .await
    {
        return handle_app_error(error);
    }
    // SECURITY (C3): the schedule author must hold the Execute × SecretVariable
    // grant for every secret the effective version exports, at CREATE time — a
    // schedule cannot be armed to run automation with secrets its creator cannot
    // use. The per-fire gate is applied separately when the schedule worker
    // calls `create_scheduled_run_group`: the per-fire gate clears the service
    // principal's `Execute × SecretVariable` rights and authors the snapshot
    // under that service principal.
    let scheduled_secret_env =
        match runtime_secret_env_for_version(scheduled_item.current_version.as_ref()) {
            Ok(secret_env) => secret_env,
            Err(error) => return handle_app_error(error),
        };
    if let Err(error) = ensure_secret_use_grants(
        &state,
        &request_context,
        &scheduled_secret_env,
        &format!("user:{user_id}"),
    )
    .await
    {
        return handle_app_error(error);
    }
    let target_set_resource_id = payload.target_set_id.to_string();
    if let Err(error) = ensure_target_set_access(
        &state,
        &request_context,
        Action::Read,
        Some(target_set_resource_id.as_str()),
    )
    .await
    {
        return handle_app_error(error);
    }
    let service_principal_resource_id = payload.service_principal_id.to_string();
    if let Err(error) = ensure_service_principal_access(
        &state,
        &request_context,
        Action::Execute,
        Some(service_principal_resource_id.as_str()),
    )
    .await
    {
        return handle_app_error(error);
    }
    let input = CreateScheduleInput {
        playbook_id: payload.playbook_id,
        target_set_id: payload.target_set_id,
        service_principal_id: payload.service_principal_id,
        name: payload.name,
        cron_expression: payload.cron_expression,
        timezone: payload.timezone,
        misfire_policy: payload.misfire_policy,
        concurrency_policy: payload.concurrency_policy,
        enabled: payload.enabled,
        next_run_at: payload.next_run_at,
        metadata: payload.metadata,
    };
    match state
        .services
        .playbooks
        .create_schedule(input, Some(user_id))
        .await
    {
        Ok(schedule) => {
            if let Err(error) = record_playbook_audit(
                &state,
                &request_context,
                "playbook.schedule.created",
                "playbook_schedule",
                Some(schedule.id),
                json!({
                    "action": "create",
                    "reason_code": "user_requested_create",
                    "summary": "playbook schedule created",
                    "affected_count": 1,
                    "resource_type": "playbook_schedule",
                    "resource_id": schedule.id.to_string(),
                    "schedule_id": schedule.id.to_string(),
                    "playbook_id": schedule.playbook_id.to_string(),
                    "target_set_id": schedule.target_set_id.to_string(),
                    "service_principal_id": schedule.service_principal_id.to_string(),
                    "enabled": schedule.enabled,
                    "next_run_at": schedule.next_run_at,
                }),
            )
            .await
            {
                return handle_app_error(error);
            }
            (StatusCode::CREATED, Json(schedule)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// List artifacts collected for a playbook run.
#[utoipa::path(
    get,
    path = "/api/v1/playbooks/runs/{run_id}/artifacts",
    tag = "playbooks",
    params(
        ListPlaybookRunItemsQuery,
        ("run_id" = Uuid, Path, description = "Playbook run ID"),
    ),
    responses(
        (status = 200, description = "Playbook run artifacts", body = ListPlaybookRunArtifactsResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_run_artifacts(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(run_id): Path<Uuid>,
    Query(query): Query<ListPlaybookRunItemsQuery>,
) -> Response {
    let request_context = request_context(&request_id, auth_context, &headers);
    if let Err(error) = request_context.require_authenticated_user_id() {
        return handle_app_error(error);
    }
    let resource_id = run_id.to_string();
    if let Err(error) =
        ensure_run_access(&state, &request_context, Action::Read, Some(&resource_id)).await
    {
        return handle_app_error(error);
    }
    let (limit, offset) = normalize_page(query.limit, query.offset);
    match state
        .services
        .playbooks
        .list_run_artifacts(run_id, limit, offset)
        .await
    {
        Ok(page) => Json(ListPlaybookRunArtifactsResponse {
            items: page.items,
            total: page.total,
            limit: page.limit,
            offset: page.offset,
        })
        .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// List events collected for a playbook run.
#[utoipa::path(
    get,
    path = "/api/v1/playbooks/runs/{run_id}/events",
    tag = "playbooks",
    params(
        ListPlaybookRunItemsQuery,
        ("run_id" = Uuid, Path, description = "Playbook run ID"),
    ),
    responses(
        (status = 200, description = "Playbook run events", body = ListPlaybookRunEventsResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_run_events(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(run_id): Path<Uuid>,
    Query(query): Query<ListPlaybookRunItemsQuery>,
) -> Response {
    let request_context = request_context(&request_id, auth_context, &headers);
    if let Err(error) = request_context.require_authenticated_user_id() {
        return handle_app_error(error);
    }
    let resource_id = run_id.to_string();
    if let Err(error) =
        ensure_run_access(&state, &request_context, Action::Read, Some(&resource_id)).await
    {
        return handle_app_error(error);
    }
    let (limit, offset) = normalize_page(query.limit, query.offset);
    match state
        .services
        .playbooks
        .list_run_events(run_id, limit, offset)
        .await
    {
        Ok(page) => Json(ListPlaybookRunEventsResponse {
            items: page.items,
            total: page.total,
            limit: page.limit,
            offset: page.offset,
        })
        .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// List events collected for a playbook run group.
#[utoipa::path(
    get,
    path = "/api/v1/playbooks/run-groups/{group_id}/events",
    tag = "playbooks",
    params(
        ListPlaybookRunGroupEventsQuery,
        ("group_id" = Uuid, Path, description = "Playbook run group ID"),
    ),
    responses(
        (status = 200, description = "Playbook run-group events", body = ListPlaybookRunEventsResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_run_group_events(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(group_id): Path<Uuid>,
    Query(query): Query<ListPlaybookRunGroupEventsQuery>,
) -> Response {
    let request_context = request_context(&request_id, auth_context, &headers);
    if let Err(error) = request_context.require_authenticated_user_id() {
        return handle_app_error(error);
    }
    let resource_id = group_id.to_string();
    if let Err(error) = ensure_run_group_access(
        &state,
        &request_context,
        Action::Read,
        Some(resource_id.as_str()),
    )
    .await
    {
        return handle_app_error(error);
    }
    let (limit, offset) = normalize_page(query.limit, query.offset);
    match state
        .services
        .playbooks
        .list_run_group_events(
            group_id,
            PlaybookRunGroupEventListFilter {
                run_id: query.run_id,
                host: query.host,
                after_sequence: query.after_sequence,
            },
            limit,
            offset,
        )
        .await
    {
        Ok(page) => Json(ListPlaybookRunEventsResponse {
            items: page.items,
            total: page.total,
            limit: page.limit,
            offset: page.offset,
        })
        .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// List playbook target sets.
#[utoipa::path(
    get,
    path = "/api/v1/playbooks/target-sets",
    tag = "playbooks",
    params(ListPlaybookTargetSetsQuery),
    responses(
        (status = 200, description = "Playbook target sets", body = ListPlaybookTargetSetsResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_target_sets(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ListPlaybookTargetSetsQuery>,
) -> Response {
    let request_context = request_context(&request_id, auth_context, &headers);
    if let Err(error) = request_context.require_authenticated_user_id() {
        return handle_app_error(error);
    }
    if let Err(error) = ensure_target_set_access(&state, &request_context, Action::Read, None).await
    {
        return handle_app_error(error);
    }
    let mode = match parse_csv_target_set_modes(query.mode.as_deref()) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        PLAYBOOK_TARGET_SET_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let filter = PlaybookTargetSetListFilter { mode, sort };
    let limit = query.limit.unwrap_or(100);
    let offset = query.offset.unwrap_or(0);
    match state
        .services
        .playbooks
        .list_target_sets(&filter, limit, offset)
        .await
    {
        Ok((items, total)) => Json(ListPlaybookTargetSetsResponse {
            items,
            total,
            limit,
            offset,
        })
        .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Get a playbook target set without resolving targets.
#[utoipa::path(
    get,
    path = "/api/v1/playbooks/target-sets/{id}",
    tag = "playbooks",
    params(("id" = Uuid, Path, description = "Target set ID")),
    responses(
        (status = 200, description = "Playbook target set", body = PlaybookTargetSet),
        (status = 404, description = "Target set not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_target_set(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let request_context = request_context(&request_id, auth_context, &headers);
    if let Err(error) = request_context.require_authenticated_user_id() {
        return handle_app_error(error);
    }
    let resource_id = id.to_string();
    if let Err(error) =
        ensure_target_set_access(&state, &request_context, Action::Read, Some(&resource_id)).await
    {
        return handle_app_error(error);
    }
    match state.services.playbooks.get_target_set(id).await {
        Ok(Some(target_set)) => Json(target_set).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound(format!(
            "Playbook target set '{id}' not found"
        ))),
        Err(error) => handle_app_error(error),
    }
}

/// Create a playbook target set.
#[utoipa::path(
    post,
    path = "/api/v1/playbooks/target-sets",
    tag = "playbooks",
    request_body = CreateTargetSetRequest,
    responses(
        (status = 201, description = "Target set created with preview", body = PlaybookTargetSetPreview),
        (status = 400, description = "Invalid target set payload", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_target_set(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<CreateTargetSetRequest>,
) -> Response {
    let request_context = request_context(&request_id, Some(auth_context), &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(user_id) => user_id,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) =
        ensure_target_set_access(&state, &request_context, Action::Create, None).await
    {
        return handle_app_error(error);
    }
    let input = CreateTargetSetInput {
        name: payload.name,
        mode: payload.mode,
        selector: payload.selector,
        members: payload.members.map(|members| {
            members
                .into_iter()
                .map(|member| StaticTargetMemberInput {
                    target_kind: member.target_kind,
                    target_id: member.target_id,
                    source_kind: member.source_kind,
                    source_id: member.source_id,
                    sort_order: member.sort_order,
                })
                .collect()
        }),
    };
    match state
        .services
        .playbooks
        .create_target_set(input, Some(user_id))
        .await
    {
        Ok(preview) => {
            if let Err(error) = record_playbook_audit(
                &state,
                &request_context,
                "playbook.target_set.created",
                "playbook_target_set",
                Some(preview.target_set.id),
                json!({
                    "action": "create",
                    "reason_code": "user_requested_create",
                    "summary": "playbook target set created",
                    "affected_count": 1,
                    "resource_type": "playbook_target_set",
                    "resource_id": preview.target_set.id.to_string(),
                    "target_set_id": preview.target_set.id.to_string(),
                    "mode": preview.target_set.mode,
                    "preview_count": preview.items.len(),
                    "preview_total": preview.total,
                    "preview_truncated": preview.truncated,
                    "resolved_target_count": preview.items.len(),
                }),
            )
            .await
            {
                return handle_app_error(error);
            }
            (StatusCode::CREATED, Json(preview)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Update an existing playbook target set.
#[utoipa::path(
    put,
    path = "/api/v1/playbooks/target-sets/{id}",
    tag = "playbooks",
    params(("id" = Uuid, Path, description = "Target set ID")),
    request_body = CreateTargetSetRequest,
    responses(
        (status = 200, description = "Target set updated", body = PlaybookTargetSet),
        (status = 400, description = "Invalid target set payload", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Target set not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_target_set(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    EnvelopeJson(payload): EnvelopeJson<CreateTargetSetRequest>,
) -> Response {
    let request_context = request_context(&request_id, Some(auth_context), &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(user_id) => user_id,
        Err(error) => return handle_app_error(error),
    };
    let resource_id = id.to_string();
    if let Err(error) =
        ensure_target_set_access(&state, &request_context, Action::Update, Some(&resource_id)).await
    {
        return handle_app_error(error);
    }
    let input = CreateTargetSetInput {
        name: payload.name,
        mode: payload.mode,
        selector: payload.selector,
        members: payload.members.map(|members| {
            members
                .into_iter()
                .map(|member| StaticTargetMemberInput {
                    target_kind: member.target_kind,
                    target_id: member.target_id,
                    source_kind: member.source_kind,
                    source_id: member.source_id,
                    sort_order: member.sort_order,
                })
                .collect()
        }),
    };
    match state
        .services
        .playbooks
        .update_target_set(id, input, Some(user_id))
        .await
    {
        Ok(target_set) => {
            if let Err(error) = record_playbook_audit(
                &state,
                &request_context,
                "playbook.target_set.updated",
                "playbook_target_set",
                Some(target_set.id),
                json!({
                    "action": "update",
                    "reason_code": "user_requested_update",
                    "summary": "playbook target set updated",
                    "affected_count": 1,
                    "resource_type": "playbook_target_set",
                    "resource_id": target_set.id.to_string(),
                    "target_set_id": target_set.id.to_string(),
                    "mode": target_set.mode,
                }),
            )
            .await
            {
                return handle_app_error(error);
            }
            Json(target_set).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Delete an existing playbook target set.
#[utoipa::path(
    delete,
    path = "/api/v1/playbooks/target-sets/{id}",
    tag = "playbooks",
    params(("id" = Uuid, Path, description = "Target set ID")),
    responses(
        (status = 204, description = "Target set deleted"),
        (status = 404, description = "Target set not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 409, description = "Target set is used by schedules (DELETE_BLOCKED with blocker tables, counts, and examples)", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_target_set(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let request_context = request_context(&request_id, auth_context, &headers);
    if let Err(error) = request_context.require_authenticated_user_id() {
        return handle_app_error(error);
    }
    let resource_id = id.to_string();
    if let Err(error) =
        ensure_target_set_access(&state, &request_context, Action::Delete, Some(&resource_id)).await
    {
        return handle_app_error(error);
    }
    match state.services.playbooks.delete_target_set(id).await {
        Ok(true) => {
            if let Err(error) = record_playbook_audit(
                &state,
                &request_context,
                "playbook.target_set.deleted",
                "playbook_target_set",
                Some(id),
                json!({
                    "action": "delete",
                    "reason_code": "user_requested_delete",
                    "summary": "playbook target set deleted",
                    "affected_count": 1,
                    "resource_type": "playbook_target_set",
                    "resource_id": id.to_string(),
                    "target_set_id": id.to_string(),
                    "deleted": true,
                }),
            )
            .await
            {
                return handle_app_error(error);
            }
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => handle_app_error(AppError::NotFound(format!(
            "Playbook target set '{id}' not found"
        ))),
        Err(error) => handle_app_error(error),
    }
}

/// Preview a playbook target set.
#[utoipa::path(
    get,
    path = "/api/v1/playbooks/target-sets/{id}/preview",
    tag = "playbooks",
    params(
        TargetSetPreviewQuery,
        ("id" = Uuid, Path, description = "Target set ID")
    ),
    responses(
        (status = 200, description = "Resolved target set preview", body = PlaybookTargetSetPreview),
        (status = 404, description = "Target set not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn preview_target_set(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(query): Query<TargetSetPreviewQuery>,
) -> Response {
    let request_context = request_context(&request_id, auth_context, &headers);
    if let Err(error) = request_context.require_authenticated_user_id() {
        return handle_app_error(error);
    }
    let resource_id = id.to_string();
    if let Err(error) =
        ensure_target_set_access(&state, &request_context, Action::Read, Some(&resource_id)).await
    {
        return handle_app_error(error);
    }
    match state
        .services
        .playbooks
        .preview_target_set(id, query.limit.unwrap_or(100))
        .await
    {
        Ok(Some(preview)) => Json(preview).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound(format!(
            "Playbook target set '{id}' not found"
        ))),
        Err(error) => handle_app_error(error),
    }
}

/// List playbook assignments.
#[utoipa::path(
    get,
    path = "/api/v1/playbooks/assignments",
    tag = "playbooks",
    params(ListPlaybookAssignmentsQuery),
    responses(
        (status = 200, description = "Playbook assignments", body = ListPlaybookAssignmentsResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_assignments(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ListPlaybookAssignmentsQuery>,
) -> Response {
    let request_context = request_context(&request_id, auth_context, &headers);
    if let Err(error) = request_context.require_authenticated_user_id() {
        return handle_app_error(error);
    }
    if let Err(error) = ensure_assignment_access(&state, &request_context, Action::Read, None).await
    {
        return handle_app_error(error);
    }
    let playbook_id = match parse_csv_uuids(query.playbook_id.as_deref(), "playbook_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let target_set_id = match parse_csv_uuids(query.target_set_id.as_deref(), "target_set_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let scope_kind = match parse_csv_assignment_scope_kinds(query.scope_kind.as_deref()) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let enabled = match parse_csv_bools(query.enabled.as_deref()) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let quick_action_enabled = match parse_csv_bools(query.quick_action_enabled.as_deref()) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        PLAYBOOK_ASSIGNMENT_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let filter = PlaybookAssignmentListFilter {
        playbook_id,
        target_set_id,
        scope_kind,
        enabled,
        quick_action_enabled,
        sort,
    };
    let limit = query.limit.unwrap_or(100);
    let offset = query.offset.unwrap_or(0);
    match state
        .services
        .playbooks
        .list_assignments(&filter, limit, offset)
        .await
    {
        Ok((items, total)) => Json(ListPlaybookAssignmentsResponse {
            items,
            total,
            limit,
            offset,
        })
        .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// List playbook assignments eligible for a selected connection target.
#[utoipa::path(
    get,
    path = "/api/v1/playbooks/targets/{target_id}/assignments",
    tag = "playbooks",
    params(
        ListPlaybookTargetAssignmentsQuery,
        ("target_id" = Uuid, Path, description = "Connection target ID"),
    ),
    responses(
        (status = 200, description = "Eligible playbook assignments for the target", body = PlaybookTargetAssignments),
        (status = 404, description = "Connection target not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_target_assignments(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(target_id): Path<Uuid>,
    Query(query): Query<ListPlaybookTargetAssignmentsQuery>,
) -> Response {
    let request_context = request_context(&request_id, auth_context, &headers);
    if let Err(error) = request_context.require_authenticated_user_id() {
        return handle_app_error(error);
    }
    if let Err(error) = ensure_assignment_access(&state, &request_context, Action::Read, None).await
    {
        return handle_app_error(error);
    }
    if let Err(error) =
        ensure_connection_target_access(&state, &request_context, Action::Read, target_id).await
    {
        return handle_app_error(error);
    }
    match state
        .services
        .playbooks
        .list_target_assignments(target_id, query.limit.unwrap_or(20))
        .await
    {
        Ok(Some(response)) => Json(response).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound(
            "Connection target not found".to_string(),
        )),
        Err(error) => handle_app_error(error),
    }
}

/// Create a playbook assignment.
#[utoipa::path(
    post,
    path = "/api/v1/playbooks/assignments",
    tag = "playbooks",
    request_body = CreatePlaybookAssignmentRequest,
    responses(
        (status = 201, description = "Playbook assignment created", body = PlaybookAssignment),
        (status = 400, description = "Invalid assignment payload", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Playbook, target set, or default version not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_assignment(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<CreatePlaybookAssignmentRequest>,
) -> Response {
    let request_context = request_context(&request_id, Some(auth_context), &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(user_id) => user_id,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) =
        ensure_assignment_access(&state, &request_context, Action::Create, None).await
    {
        return handle_app_error(error);
    }
    let playbook_resource_id = payload.playbook_id.to_string();
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Read,
        Some(playbook_resource_id.as_str()),
    )
    .await
    {
        return handle_app_error(error);
    }
    let target_set_resource_id = payload.target_set_id.to_string();
    if let Err(error) = ensure_target_set_access(
        &state,
        &request_context,
        Action::Read,
        Some(target_set_resource_id.as_str()),
    )
    .await
    {
        return handle_app_error(error);
    }
    if let Some(default_version_id) = payload.default_version_id {
        let version_resource_id = default_version_id.to_string();
        if let Err(error) = ensure_version_access(
            &state,
            &request_context,
            Action::Read,
            Some(version_resource_id.as_str()),
        )
        .await
        {
            return handle_app_error(error);
        }
    }
    let input = CreateAssignmentInput {
        playbook_id: payload.playbook_id,
        target_set_id: payload.target_set_id,
        default_version_id: payload.default_version_id,
        name: payload.name,
        scope_kind: payload.scope_kind,
        scope_id: payload.scope_id,
        priority: payload.priority,
        enabled: payload.enabled,
        visible: payload.visible,
        executable: payload.executable,
        quick_action_enabled: payload.quick_action_enabled,
        default_input: payload.default_input,
        default_metadata: payload.default_metadata,
        default_timeout_seconds: payload.default_timeout_seconds,
        default_target_limit: payload.default_target_limit,
        metadata: payload.metadata,
    };
    match state
        .services
        .playbooks
        .create_assignment(input, Some(user_id))
        .await
    {
        Ok(assignment) => {
            if let Err(error) = record_playbook_audit(
                &state,
                &request_context,
                "playbook.assignment.created",
                "playbook_assignment",
                Some(assignment.id),
                json!({
                    "action": "create",
                    "reason_code": "user_requested_create",
                    "summary": "playbook assignment created",
                    "affected_count": 1,
                    "resource_type": "playbook_assignment",
                    "resource_id": assignment.id.to_string(),
                    "assignment_id": assignment.id.to_string(),
                    "playbook_id": assignment.playbook_id.to_string(),
                    "target_set_id": assignment.target_set_id.to_string(),
                    "scope_kind": assignment.scope_kind,
                    "scope_id": assignment.scope_id.map(|value| value.to_string()),
                    "visible": assignment.visible,
                    "executable": assignment.executable,
                    "quick_action_enabled": assignment.quick_action_enabled,
                    "enabled": assignment.enabled,
                }),
            )
            .await
            {
                return handle_app_error(error);
            }
            (StatusCode::CREATED, Json(assignment)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Get a playbook assignment.
#[utoipa::path(
    get,
    path = "/api/v1/playbooks/assignments/{id}",
    tag = "playbooks",
    params(("id" = Uuid, Path, description = "Assignment ID")),
    responses(
        (status = 200, description = "Playbook assignment", body = PlaybookAssignment),
        (status = 404, description = "Assignment not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_assignment(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let request_context = request_context(&request_id, auth_context, &headers);
    if let Err(error) = request_context.require_authenticated_user_id() {
        return handle_app_error(error);
    }
    let resource_id = id.to_string();
    if let Err(error) =
        ensure_assignment_access(&state, &request_context, Action::Read, Some(&resource_id)).await
    {
        return handle_app_error(error);
    }
    match state.services.playbooks.get_assignment(id).await {
        Ok(Some(assignment)) => Json(assignment).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound(format!(
            "Playbook assignment '{id}' not found"
        ))),
        Err(error) => handle_app_error(error),
    }
}

/// Update a playbook assignment.
#[utoipa::path(
    patch,
    path = "/api/v1/playbooks/assignments/{id}",
    tag = "playbooks",
    params(("id" = Uuid, Path, description = "Assignment ID")),
    request_body = UpdatePlaybookAssignmentRequest,
    responses(
        (status = 200, description = "Playbook assignment updated", body = PlaybookAssignment),
        (status = 400, description = "Invalid assignment payload", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Assignment, playbook, target set, or default version not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_assignment(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    EnvelopeJson(payload): EnvelopeJson<UpdatePlaybookAssignmentRequest>,
) -> Response {
    let request_context = request_context(&request_id, Some(auth_context), &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(user_id) => user_id,
        Err(error) => return handle_app_error(error),
    };
    let resource_id = id.to_string();
    if let Err(error) =
        ensure_assignment_access(&state, &request_context, Action::Update, Some(&resource_id)).await
    {
        return handle_app_error(error);
    }
    if let Some(playbook_id) = payload.playbook_id {
        let playbook_resource_id = playbook_id.to_string();
        if let Err(error) = ensure_access(
            &state,
            &request_context,
            Action::Read,
            Some(playbook_resource_id.as_str()),
        )
        .await
        {
            return handle_app_error(error);
        }
    }
    if let Some(target_set_id) = payload.target_set_id {
        let target_set_resource_id = target_set_id.to_string();
        if let Err(error) = ensure_target_set_access(
            &state,
            &request_context,
            Action::Read,
            Some(target_set_resource_id.as_str()),
        )
        .await
        {
            return handle_app_error(error);
        }
    }
    if let Some(Some(default_version_id)) = payload.default_version_id {
        let version_resource_id = default_version_id.to_string();
        if let Err(error) = ensure_version_access(
            &state,
            &request_context,
            Action::Read,
            Some(version_resource_id.as_str()),
        )
        .await
        {
            return handle_app_error(error);
        }
    }
    let input = UpdateAssignmentInput {
        playbook_id: payload.playbook_id,
        target_set_id: payload.target_set_id,
        default_version_id: payload.default_version_id,
        name: payload.name,
        scope_kind: payload.scope_kind,
        scope_id: payload.scope_id,
        priority: payload.priority,
        enabled: payload.enabled,
        visible: payload.visible,
        executable: payload.executable,
        quick_action_enabled: payload.quick_action_enabled,
        default_input: payload.default_input,
        default_metadata: payload.default_metadata,
        default_timeout_seconds: payload.default_timeout_seconds,
        default_target_limit: payload.default_target_limit,
        metadata: payload.metadata,
    };
    match state
        .services
        .playbooks
        .update_assignment(id, input, Some(user_id))
        .await
    {
        Ok(Some(assignment)) => {
            if let Err(error) = record_playbook_audit(
                &state,
                &request_context,
                "playbook.assignment.updated",
                "playbook_assignment",
                Some(assignment.id),
                json!({
                    "action": "update",
                    "reason_code": "user_requested_update",
                    "summary": "playbook assignment updated",
                    "affected_count": 1,
                    "resource_type": "playbook_assignment",
                    "resource_id": assignment.id.to_string(),
                    "assignment_id": assignment.id.to_string(),
                    "playbook_id": assignment.playbook_id.to_string(),
                    "target_set_id": assignment.target_set_id.to_string(),
                    "scope_kind": assignment.scope_kind,
                    "scope_id": assignment.scope_id.map(|value| value.to_string()),
                    "visible": assignment.visible,
                    "executable": assignment.executable,
                    "quick_action_enabled": assignment.quick_action_enabled,
                    "enabled": assignment.enabled,
                }),
            )
            .await
            {
                return handle_app_error(error);
            }
            Json(assignment).into_response()
        }
        Ok(None) => handle_app_error(AppError::NotFound(format!(
            "Playbook assignment '{id}' not found"
        ))),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a playbook assignment.
#[utoipa::path(
    delete,
    path = "/api/v1/playbooks/assignments/{id}",
    tag = "playbooks",
    params(("id" = Uuid, Path, description = "Assignment ID")),
    responses(
        (status = 204, description = "Playbook assignment deleted"),
        (status = 404, description = "Assignment not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_assignment(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let request_context = request_context(&request_id, auth_context, &headers);
    if let Err(error) = request_context.require_authenticated_user_id() {
        return handle_app_error(error);
    }
    let resource_id = id.to_string();
    if let Err(error) =
        ensure_assignment_access(&state, &request_context, Action::Delete, Some(&resource_id)).await
    {
        return handle_app_error(error);
    }
    match state.services.playbooks.delete_assignment(id).await {
        Ok(true) => {
            if let Err(error) = record_playbook_audit(
                &state,
                &request_context,
                "playbook.assignment.deleted",
                "playbook_assignment",
                Some(id),
                json!({
                    "action": "delete",
                    "reason_code": "user_requested_delete",
                    "summary": "playbook assignment deleted",
                    "affected_count": 1,
                    "resource_type": "playbook_assignment",
                    "resource_id": id.to_string(),
                    "assignment_id": id.to_string(),
                    "deleted": true,
                }),
            )
            .await
            {
                return handle_app_error(error);
            }
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => handle_app_error(AppError::NotFound(format!(
            "Playbook assignment '{id}' not found"
        ))),
        Err(error) => handle_app_error(error),
    }
}

/// Update a playbook schedule.
#[utoipa::path(
    patch,
    path = "/api/v1/playbooks/schedules/{id}",
    tag = "playbooks",
    params(("id" = Uuid, Path, description = "Schedule ID")),
    request_body = UpdatePlaybookScheduleRequest,
    responses(
        (status = 200, description = "Playbook schedule updated", body = PlaybookSchedule),
        (status = 400, description = "Invalid schedule payload", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Schedule or target set not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_schedule(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    EnvelopeJson(payload): EnvelopeJson<UpdatePlaybookScheduleRequest>,
) -> Response {
    let request_context = request_context(&request_id, Some(auth_context), &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(user_id) => user_id,
        Err(error) => return handle_app_error(error),
    };
    let resource_id = id.to_string();
    if let Err(error) =
        ensure_schedule_access(&state, &request_context, Action::Update, Some(&resource_id)).await
    {
        return handle_app_error(error);
    }
    // A re-targeted schedule must clear Read on the new target set, mirroring the
    // create path's referenced-resource authorization.
    if let Some(target_set_id) = payload.target_set_id {
        let target_set_resource_id = target_set_id.to_string();
        if let Err(error) = ensure_target_set_access(
            &state,
            &request_context,
            Action::Read,
            Some(target_set_resource_id.as_str()),
        )
        .await
        {
            return handle_app_error(error);
        }
    }
    let input = UpdateScheduleInput {
        target_set_id: payload.target_set_id,
        name: payload.name,
        cron_expression: payload.cron_expression,
        timezone: payload.timezone,
        misfire_policy: payload.misfire_policy,
        concurrency_policy: payload.concurrency_policy,
        enabled: payload.enabled,
        next_run_at: payload.next_run_at,
        metadata: payload.metadata,
    };
    match state
        .services
        .playbooks
        .update_schedule(id, input, Some(user_id))
        .await
    {
        Ok(Some(schedule)) => {
            if let Err(error) = record_playbook_audit(
                &state,
                &request_context,
                "playbook.schedule.updated",
                "playbook_schedule",
                Some(schedule.id),
                json!({
                    "action": "update",
                    "reason_code": "user_requested_update",
                    "summary": "playbook schedule updated",
                    "affected_count": 1,
                    "resource_type": "playbook_schedule",
                    "resource_id": schedule.id.to_string(),
                    "schedule_id": schedule.id.to_string(),
                    "playbook_id": schedule.playbook_id.to_string(),
                    "target_set_id": schedule.target_set_id.to_string(),
                    "enabled": schedule.enabled,
                    "next_run_at": schedule.next_run_at,
                }),
            )
            .await
            {
                return handle_app_error(error);
            }
            Json(schedule).into_response()
        }
        Ok(None) => handle_app_error(AppError::NotFound(format!(
            "Playbook schedule '{id}' not found"
        ))),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a playbook schedule.
#[utoipa::path(
    delete,
    path = "/api/v1/playbooks/schedules/{id}",
    tag = "playbooks",
    params(("id" = Uuid, Path, description = "Schedule ID")),
    responses(
        (status = 204, description = "Playbook schedule deleted"),
        (status = 404, description = "Schedule not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_schedule(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let request_context = request_context(&request_id, auth_context, &headers);
    if let Err(error) = request_context.require_authenticated_user_id() {
        return handle_app_error(error);
    }
    let resource_id = id.to_string();
    if let Err(error) =
        ensure_schedule_access(&state, &request_context, Action::Delete, Some(&resource_id)).await
    {
        return handle_app_error(error);
    }
    match state.services.playbooks.delete_schedule(id).await {
        Ok(true) => {
            if let Err(error) = record_playbook_audit(
                &state,
                &request_context,
                "playbook.schedule.deleted",
                "playbook_schedule",
                Some(id),
                json!({
                    "action": "delete",
                    "reason_code": "user_requested_delete",
                    "summary": "playbook schedule deleted",
                    "affected_count": 1,
                    "resource_type": "playbook_schedule",
                    "resource_id": id.to_string(),
                    "schedule_id": id.to_string(),
                    "deleted": true,
                }),
            )
            .await
            {
                return handle_app_error(error);
            }
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => handle_app_error(AppError::NotFound(format!(
            "Playbook schedule '{id}' not found"
        ))),
        Err(error) => handle_app_error(error),
    }
}

/// List playbook service principals.
#[utoipa::path(
    get,
    path = "/api/v1/playbooks/service-principals",
    tag = "playbooks",
    params(ListPlaybookServicePrincipalsQuery),
    responses(
        (status = 200, description = "Playbook service principals", body = ListPlaybookServicePrincipalsResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_service_principals(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ListPlaybookServicePrincipalsQuery>,
) -> Response {
    let request_context = request_context(&request_id, auth_context, &headers);
    if let Err(error) = request_context.require_authenticated_user_id() {
        return handle_app_error(error);
    }
    if let Err(error) =
        ensure_service_principal_access(&state, &request_context, Action::Read, None).await
    {
        return handle_app_error(error);
    }
    let status = match parse_csv_service_principal_statuses(query.status.as_deref()) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        PLAYBOOK_SERVICE_PRINCIPAL_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let filter = PlaybookServicePrincipalListFilter { status, sort };
    let limit = query.limit.unwrap_or(100);
    let offset = query.offset.unwrap_or(0);
    match state
        .services
        .playbooks
        .list_service_principals(&filter, limit, offset)
        .await
    {
        Ok((items, total)) => Json(ListPlaybookServicePrincipalsResponse {
            items,
            total,
            limit,
            offset,
        })
        .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Get a playbook service principal.
#[utoipa::path(
    get,
    path = "/api/v1/playbooks/service-principals/{id}",
    tag = "playbooks",
    params(("id" = Uuid, Path, description = "Service principal ID")),
    responses(
        (status = 200, description = "Playbook service principal", body = PlaybookServicePrincipal),
        (status = 404, description = "Service principal not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_service_principal(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let request_context = request_context(&request_id, auth_context, &headers);
    if let Err(error) = request_context.require_authenticated_user_id() {
        return handle_app_error(error);
    }
    let resource_id = id.to_string();
    if let Err(error) =
        ensure_service_principal_access(&state, &request_context, Action::Read, Some(&resource_id))
            .await
    {
        return handle_app_error(error);
    }
    match state.services.playbooks.get_service_principal(id).await {
        Ok(Some(principal)) => Json(principal).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound(format!(
            "Playbook service principal '{id}' not found"
        ))),
        Err(error) => handle_app_error(error),
    }
}

/// Create a playbook service principal.
#[utoipa::path(
    post,
    path = "/api/v1/playbooks/service-principals",
    tag = "playbooks",
    request_body = CreatePlaybookServicePrincipalRequest,
    responses(
        (status = 201, description = "Playbook service principal created", body = PlaybookServicePrincipal),
        (status = 400, description = "Invalid service principal payload", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_service_principal(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<CreatePlaybookServicePrincipalRequest>,
) -> Response {
    let request_context = request_context(&request_id, Some(auth_context), &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(user_id) => user_id,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) =
        ensure_service_principal_access(&state, &request_context, Action::Create, None).await
    {
        return handle_app_error(error);
    }
    let input = CreateServicePrincipalInput {
        name: payload.name,
        slug: payload.slug,
        description: payload.description,
        metadata: payload.metadata,
    };
    match state
        .services
        .playbooks
        .create_service_principal(input, Some(user_id))
        .await
    {
        Ok(principal) => {
            if let Err(error) = record_playbook_audit(
                &state,
                &request_context,
                "playbook.service_principal.created",
                "playbook_service_principal",
                Some(principal.id),
                json!({
                    "action": "create",
                    "reason_code": "user_requested_create",
                    "summary": "playbook service principal created",
                    "affected_count": 1,
                    "resource_type": "playbook_service_principal",
                    "resource_id": principal.id.to_string(),
                    "service_principal_id": principal.id.to_string(),
                    "slug": principal.slug,
                    "status": principal.status,
                }),
            )
            .await
            {
                return handle_app_error(error);
            }
            (StatusCode::CREATED, Json(principal)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Revoke a playbook service principal.
#[utoipa::path(
    post,
    path = "/api/v1/playbooks/service-principals/{id}/revoke",
    tag = "playbooks",
    params(("id" = Uuid, Path, description = "Service principal ID")),
    request_body = RevokePlaybookServicePrincipalRequest,
    responses(
        (status = 200, description = "Playbook service principal revoked", body = PlaybookServicePrincipal),
        (status = 400, description = "Invalid revoke payload", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Service principal not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn revoke_service_principal(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    EnvelopeJson(payload): EnvelopeJson<RevokePlaybookServicePrincipalRequest>,
) -> Response {
    let request_context = request_context(&request_id, Some(auth_context), &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(user_id) => user_id,
        Err(error) => return handle_app_error(error),
    };
    let resource_id = id.to_string();
    if let Err(error) = ensure_service_principal_access(
        &state,
        &request_context,
        Action::Delete,
        Some(&resource_id),
    )
    .await
    {
        return handle_app_error(error);
    }
    let input = RevokeServicePrincipalInput {
        revocation_reason: payload.revocation_reason,
    };
    match state
        .services
        .playbooks
        .revoke_service_principal(id, input, Some(user_id), Utc::now())
        .await
    {
        Ok(Some(principal)) => {
            if let Err(error) = record_playbook_audit(
                &state,
                &request_context,
                "playbook.service_principal.revoked",
                "playbook_service_principal",
                Some(principal.id),
                json!({
                    "action": "revoke",
                    "reason_code": "user_requested_revoke",
                    "summary": "playbook service principal revoked",
                    "affected_count": 1,
                    "resource_type": "playbook_service_principal",
                    "resource_id": principal.id.to_string(),
                    "service_principal_id": principal.id.to_string(),
                    "slug": principal.slug,
                    "status": principal.status,
                    "has_revocation_reason": principal.revocation_reason.is_some(),
                }),
            )
            .await
            {
                return handle_app_error(error);
            }
            Json(principal).into_response()
        }
        Ok(None) => handle_app_error(AppError::NotFound(format!(
            "Playbook service principal '{id}' not found"
        ))),
        Err(error) => handle_app_error(error),
    }
}

/// List playbook runs for history, filterable by playbook and/or status.
#[utoipa::path(
    get,
    path = "/api/v1/playbooks/runs",
    tag = "playbooks",
    params(ListPlaybookRunsQuery),
    responses(
        (status = 200, description = "Playbook runs", body = ListPlaybookRunsResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_runs(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ListPlaybookRunsQuery>,
) -> Response {
    let request_context = request_context(&request_id, auth_context, &headers);
    if let Err(error) = request_context.require_authenticated_user_id() {
        return handle_app_error(error);
    }
    if let Err(error) = ensure_run_access(&state, &request_context, Action::Read, None).await {
        return handle_app_error(error);
    }
    let playbook_id = match parse_csv_uuids(query.playbook_id.as_deref(), "playbook_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let status = match parse_csv_run_statuses(query.status.as_deref()) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        PLAYBOOK_RUN_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let filter = PlaybookRunListFilter {
        playbook_id,
        status,
        sort,
    };
    let (limit, offset) = normalize_page(query.limit, query.offset);
    match state
        .services
        .playbooks
        .list_runs(&filter, limit, offset)
        .await
    {
        Ok(page) => Json(ListPlaybookRunsResponse {
            items: page.items,
            total: page.total,
            limit: page.limit,
            offset: page.offset,
        })
        .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// List playbook run groups for history, filterable by playbook and/or status.
#[utoipa::path(
    get,
    path = "/api/v1/playbooks/run-groups",
    tag = "playbooks",
    params(ListPlaybookRunGroupsQuery),
    responses(
        (status = 200, description = "Playbook run groups", body = ListPlaybookRunGroupsResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_run_groups(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ListPlaybookRunGroupsQuery>,
) -> Response {
    let request_context = request_context(&request_id, auth_context, &headers);
    if let Err(error) = request_context.require_authenticated_user_id() {
        return handle_app_error(error);
    }
    if let Err(error) = ensure_run_group_access(&state, &request_context, Action::Read, None).await
    {
        return handle_app_error(error);
    }
    let playbook_id = match parse_csv_uuids(query.playbook_id.as_deref(), "playbook_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let status = match parse_csv_run_statuses(query.status.as_deref()) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        PLAYBOOK_RUN_GROUP_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let filter = PlaybookRunListFilter {
        playbook_id,
        status,
        sort,
    };
    let limit = query.limit.unwrap_or(100);
    let offset = query.offset.unwrap_or(0);
    match state
        .services
        .playbooks
        .list_run_groups(&filter, limit, offset)
        .await
    {
        Ok((items, total)) => Json(ListPlaybookRunGroupsResponse {
            items,
            total,
            limit,
            offset,
        })
        .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Get a single playbook run with current live status.
#[utoipa::path(
    get,
    path = "/api/v1/playbooks/runs/{run_id}",
    tag = "playbooks",
    params(("run_id" = Uuid, Path, description = "Playbook run ID")),
    responses(
        (status = 200, description = "Playbook run", body = PlaybookRun),
        (status = 404, description = "Run not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_run(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(run_id): Path<Uuid>,
) -> Response {
    let request_context = request_context(&request_id, auth_context, &headers);
    if let Err(error) = request_context.require_authenticated_user_id() {
        return handle_app_error(error);
    }
    let resource_id = run_id.to_string();
    if let Err(error) =
        ensure_run_access(&state, &request_context, Action::Read, Some(&resource_id)).await
    {
        return handle_app_error(error);
    }
    match state.services.playbooks.get_run(run_id).await {
        Ok(Some(run)) => Json(run).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound(format!(
            "Playbook run '{run_id}' not found"
        ))),
        Err(error) => handle_app_error(error),
    }
}

/// Duplicate a playbook, creating an independent copy owned by the caller.
#[utoipa::path(
    post,
    path = "/api/v1/playbooks/{id}/duplicate",
    tag = "playbooks",
    params(("id" = Uuid, Path, description = "Playbook ID to duplicate")),
    request_body = DuplicatePlaybookRequest,
    responses(
        (status = 201, description = "Playbook duplicated", body = PlaybookCatalogItem),
        (status = 404, description = "Playbook not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn duplicate_playbook(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    EnvelopeJson(payload): EnvelopeJson<DuplicatePlaybookRequest>,
) -> Response {
    let request_context = request_context(&request_id, Some(auth_context), &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(user_id) => user_id,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = ensure_access(&state, &request_context, Action::Create, None).await {
        return handle_app_error(error);
    }
    match state
        .services
        .playbooks
        .duplicate_playbook(id, payload.name, Some(user_id))
        .await
    {
        Ok(item) => {
            // The copy inherits the source version's `token_policy` (and with it
            // `runtime.secrets.env`), so its version 1 must be projected into
            // `secret_usage_binding` like any other authored version — otherwise
            // the declared secrets under-count usage and stay deletable while
            // this live playbook still references them.
            if let Some(version) = item.current_version.as_ref()
                && let Err(error) = project_version_runtime_secret_usage(
                    &state,
                    item.playbook.id,
                    version.id,
                    Some(&version.token_policy),
                )
                .await
            {
                return handle_app_error(error);
            }
            if let Err(error) = record_playbook_audit(
                &state,
                &request_context,
                "playbook.duplicated",
                "playbook",
                Some(item.playbook.id),
                json!({
                    "action": "duplicate",
                    "reason_code": "user_requested_duplicate",
                    "summary": "playbook duplicated from source",
                    "affected_count": 1,
                    "resource_type": "playbook",
                    "resource_id": item.playbook.id.to_string(),
                    "playbook_id": item.playbook.id.to_string(),
                    "source_playbook_id": id.to_string(),
                    "slug": item.playbook.slug,
                    "status": item.playbook.status,
                }),
            )
            .await
            {
                return handle_app_error(error);
            }
            (StatusCode::CREATED, Json(item)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Get a playbook run group with its per-target runs.
#[utoipa::path(
    get,
    path = "/api/v1/playbooks/run-groups/{id}",
    tag = "playbooks",
    params(
        GetPlaybookRunGroupQuery,
        ("id" = Uuid, Path, description = "Run group ID"),
    ),
    responses(
        (status = 200, description = "Playbook run group with runs", body = PlaybookRunGroupDetail),
        (status = 404, description = "Run group not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_run_group(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(query): Query<GetPlaybookRunGroupQuery>,
) -> Response {
    let request_context = request_context(&request_id, auth_context, &headers);
    if let Err(error) = request_context.require_authenticated_user_id() {
        return handle_app_error(error);
    }
    let resource_id = id.to_string();
    if let Err(error) = ensure_run_group_access(
        &state,
        &request_context,
        Action::Read,
        Some(resource_id.as_str()),
    )
    .await
    {
        return handle_app_error(error);
    }
    match state
        .services
        .playbooks
        .get_run_group_with_runs(id, query.limit.unwrap_or(500))
        .await
    {
        Ok(Some(detail)) => Json(detail).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound(format!(
            "Playbook run group '{id}' not found"
        ))),
        Err(error) => handle_app_error(error),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CompleteRunGroupBatchRequest, CreateRunGroupEventsBatchRequest, UploadProjectFilesRequest,
        bearer_token, credential_use_scope_allowed, ensure_single_run_cancel_allowed,
        launch_references_credentials, parse_csv_assignment_scope_kinds, parse_csv_bools,
        parse_csv_run_statuses, parse_csv_schedule_concurrency_policies,
        parse_csv_service_principal_statuses, parse_csv_target_set_modes,
        resolve_launch_target_set_id,
    };
    use crate::contexts::playbooks::domain::{
        PlaybookAssignmentScopeKind, PlaybookRunStatus, PlaybookScheduleConcurrencyPolicy,
        PlaybookServicePrincipalStatus, PlaybookTargetSetMode,
    };
    use crate::contexts::shared::request_context::{ApiTokenScopeContext, RequestContext};
    use crate::errors::AppError;
    use axum::http::{HeaderMap, HeaderValue, header::AUTHORIZATION};
    use serde_json::json;
    use uuid::Uuid;

    fn assert_validation_error_contains<T: std::fmt::Debug>(
        result: Result<T, AppError>,
        needle: &str,
    ) {
        assert!(
            matches!(
                &result,
                Err(AppError::Validation(message)) if message.contains(needle)
            ),
            "unexpected result: {result:?}"
        );
    }

    fn request_context(
        auth_method: &str,
        api_token_scopes: Vec<ApiTokenScopeContext>,
    ) -> RequestContext {
        RequestContext::new(
            Some(Uuid::nil()),
            auth_method.to_string(),
            None,
            "playbooks-test".to_string(),
            crate::contexts::shared::request_context::RequestAuth {
                oidc_groups: Vec::new(),
                api_token_scopes,
                api_token_scope_resource_override: None,
            },
        )
    }

    #[test]
    fn batch_requests_reject_empty_collections_with_a_400() {
        use axum::http::StatusCode;

        use crate::api::rest::error_mapping::map_app_error_to_http;

        let files = serde_json::from_value::<UploadProjectFilesRequest>(json!({"files": []}))
            .expect("deserialize files");
        let events = serde_json::from_value::<CreateRunGroupEventsBatchRequest>(
            json!({"batch_id": Uuid::nil(), "events": []}),
        )
        .expect("deserialize events");
        let runs = serde_json::from_value::<CompleteRunGroupBatchRequest>(
            json!({"lease_owner": "runner", "runs": []}),
        )
        .expect("deserialize runs");

        for validated in [files.validate(), events.validate(), runs.validate()] {
            assert_eq!(
                map_app_error_to_http(validated.unwrap_err()).status(),
                StatusCode::BAD_REQUEST,
                "an empty playbook batch must answer 400, not 200",
            );
        }
    }

    #[test]
    fn bearer_token_enforces_case_insensitive_scheme_and_rejects_invalid_shapes() {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("bEaReR    sidecar-token   "),
        );

        let token = bearer_token(&headers);

        assert!(matches!(token.as_deref(), Ok("sidecar-token")));
        let missing = HeaderMap::new();
        let mut wrong_scheme = HeaderMap::new();
        wrong_scheme.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Basic sidecar-token"),
        );
        let mut empty_token = HeaderMap::new();
        empty_token.insert(AUTHORIZATION, HeaderValue::from_static("Bearer    "));

        for result in [
            bearer_token(&missing),
            bearer_token(&wrong_scheme),
            bearer_token(&empty_token),
        ] {
            assert!(matches!(
                result,
                Err(AppError::Auth(message)) if message == "TOKEN_INVALID"
            ));
        }
    }

    #[test]
    fn csv_playbook_filter_parsers_normalize_deduplicate_and_reject_invalid_values() {
        let parsed = parse_csv_bools(Some(" false, TRUE, false, , true "));

        assert_eq!(parsed.ok(), Some(vec![false, true]));
        let parsed = parse_csv_bools(Some("true, enabled, false"));
        assert_validation_error_contains(parsed, "enabled");
        assert_eq!(
            parse_csv_assignment_scope_kinds(Some(" ROLE, site, role ")).ok(),
            Some(vec![
                PlaybookAssignmentScopeKind::Role,
                PlaybookAssignmentScopeKind::Site,
            ])
        );
        assert_eq!(
            parse_csv_target_set_modes(Some(" STATIC, dynamic, static ")).ok(),
            Some(vec![
                PlaybookTargetSetMode::Static,
                PlaybookTargetSetMode::Dynamic,
            ])
        );
        assert_eq!(
            parse_csv_schedule_concurrency_policies(Some(
                " queue_pending, SKIP_IF_RUNNING, queue_pending "
            ))
            .ok(),
            Some(vec![
                PlaybookScheduleConcurrencyPolicy::QueuePending,
                PlaybookScheduleConcurrencyPolicy::SkipIfRunning,
            ])
        );
        assert_eq!(
            parse_csv_run_statuses(Some(" running, TIMED_OUT, running ")).ok(),
            Some(vec![
                PlaybookRunStatus::Running,
                PlaybookRunStatus::TimedOut,
            ])
        );
        assert_eq!(
            parse_csv_service_principal_statuses(Some(" disabled, ACTIVE, disabled ")).ok(),
            Some(vec![
                PlaybookServicePrincipalStatus::Disabled,
                PlaybookServicePrincipalStatus::Active,
            ])
        );
    }

    #[test]
    fn job_template_launch_validation_enforces_targets_credentials_and_token_ceiling() {
        let requested = Uuid::from_u128(1);
        let template_default = Uuid::from_u128(2);

        assert_eq!(
            resolve_launch_target_set_id(Some(requested), Some(template_default)),
            Some(requested)
        );
        assert_eq!(
            resolve_launch_target_set_id(None, Some(template_default)),
            Some(template_default)
        );
        assert_eq!(resolve_launch_target_set_id(None, None), None);
        let account_id = Uuid::from_u128(7).to_string();
        let credential_refs = json!([{
            "account_kind": "connection_config",
            "account_id": account_id,
        }]);
        let launch_options = json!({
            "credential_choice": "connection_config",
            "credential_account_id": account_id,
        });
        let payload_metadata = json!({ "launch_options": launch_options.clone() });

        assert_eq!(
            launch_references_credentials(&credential_refs, &json!({}), None).ok(),
            Some(true)
        );
        assert_eq!(
            launch_references_credentials(&json!([]), &launch_options, None).ok(),
            Some(true)
        );
        assert_eq!(
            launch_references_credentials(&json!([]), &json!({}), Some(&payload_metadata),).ok(),
            Some(true)
        );
        assert_eq!(
            launch_references_credentials(&json!([]), &json!({}), None).ok(),
            Some(false)
        );
        let malformed = json!([{
            "account_kind": "connection_config",
            "account_id": "not-a-uuid",
        }]);

        let result = launch_references_credentials(&malformed, &json!({}), None);

        assert_validation_error_contains(result, "account_id must be a UUID");
        let secret_scope = ApiTokenScopeContext {
            action: "secret".to_string(),
            resource_type: "secret_variable".to_string(),
            resource_pattern: "*".to_string(),
        };
        let read_only_scope = ApiTokenScopeContext {
            action: "read".to_string(),
            ..secret_scope.clone()
        };

        assert!(credential_use_scope_allowed(&request_context(
            "session",
            Vec::new()
        )));
        assert!(credential_use_scope_allowed(&request_context(
            "api_token",
            vec![secret_scope]
        )));
        assert!(!credential_use_scope_allowed(&request_context(
            "api_token",
            vec![read_only_scope]
        )));
    }

    #[test]
    fn single_run_cancellation_enforces_group_boundary_and_idempotent_retries() {
        let result = ensure_single_run_cancel_allowed(2, PlaybookRunStatus::Running);

        assert!(matches!(
            result,
            Err(AppError::Conflict(message))
                if message.contains("can only be canceled as a whole run group")
        ));
        for status in [
            PlaybookRunStatus::Queued,
            PlaybookRunStatus::Leased,
            PlaybookRunStatus::Dispatching,
            PlaybookRunStatus::Running,
        ] {
            assert!(ensure_single_run_cancel_allowed(1, status).is_ok());
        }
        for status in [
            PlaybookRunStatus::Succeeded,
            PlaybookRunStatus::Failed,
            PlaybookRunStatus::Canceled,
            PlaybookRunStatus::TimedOut,
        ] {
            assert!(ensure_single_run_cancel_allowed(50_000, status).is_ok());
        }
    }

    #[test]
    fn complete_run_group_batch_request_defaults_absent_new_host_keys_and_keeps_reported_ones() {
        let without_new_host_keys: CompleteRunGroupBatchRequest = serde_json::from_value(json!({
            "lease_owner": "worker-1",
            "runs": [{
                "run_id": "00000000-0000-0000-0000-000000000001",
                "status": "succeeded",
                "exit_code": 0,
                "error_message": null
            }]
        }))
        .expect("a body without new_host_keys must still deserialize");
        assert!(
            without_new_host_keys.new_host_keys.is_empty(),
            "an absent new_host_keys must default to empty, not reject the callback"
        );

        let harvested: CompleteRunGroupBatchRequest = serde_json::from_value(json!({
            "lease_owner": "worker-1",
            "runs": [{
                "run_id": "00000000-0000-0000-0000-000000000001",
                "status": "succeeded",
                "exit_code": 0,
                "error_message": null
            }],
            "new_host_keys": ["host.example.org ssh-ed25519 AAAASEED"]
        }))
        .expect("a body with new_host_keys deserializes");
        assert_eq!(
            harvested.new_host_keys,
            vec!["host.example.org ssh-ed25519 AAAASEED".to_string()],
            "the runner-reported keys must survive deserialization"
        );
    }
}
