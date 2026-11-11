//! REST handlers for Linux playbook catalog management.

use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use piping_contracts::playbook_runner::{
    AcquireAuth, AcquireProjectRequest, RUNNER_CONTRACT_VERSION, RunnerConsoleBatch,
    SnapshotContent, validate_console_batch,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::api::access_control_helpers::build_policy_evaluation_service;
use crate::api::connection_authz_attributes::connection_target_authz_attributes_for;
use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::api::rest::routes::validation::{deserialize_nullable_patch, require_bulk_items};
use crate::app::AppState;
use crate::app::playbook_secret_gate::RecordSshDelegation;
use crate::contexts::access_control::domain::{Action, Principal, ResourceType};
use crate::contexts::audit::domain::safe_projection::project_metadata;
use crate::contexts::audit::domain::{
    AUDIT_ACTOR_KIND_PLAYBOOK_JOB_TOKEN, AuditContext, AuditCorrelation, AuditDecision,
    AuditDelegatedPrincipal, AuditDelegatedPrincipalKind, AuditEmitter, AuditEmitterKind,
    AuditEvent, AuditOutcome,
};
use crate::contexts::playbooks::application::playbook_service::PlaybookRunGroupEventListFilter;
use crate::contexts::playbooks::application::{
    CompleteRunGroupBatchInput, CompleteRunGroupBatchItemInput, CompleteRunInput,
    CreateArtifactInput, CreateAssignmentInput, CreateExecutionEnvironmentInput,
    CreateJobTemplateInput, CreateProjectInput, CreateRunEventInput, CreateRunGroupEventInput,
    CreateScheduleInput, CreateTargetSetInput, LaunchJobTemplateInput, PlaybookRunCompletion,
    PlaybookRunGroupCompletionBatch, PlaybookRunGroupDetail, PlaybookTargetAssignments,
    RunStreamFrame, StaticTargetMemberInput, UpdateAssignmentInput,
    UpdateExecutionEnvironmentInput, UpdateJobTemplateInput, UpdateProjectInput,
    UpdateScheduleInput, parse_job_token, selector_keys,
};
use crate::contexts::playbooks::domain::{
    BuiltinJobLookup, PlaybookArtifact, PlaybookAssignment, PlaybookAssignmentScopeKind,
    PlaybookExecutionEnvironment, PlaybookRun, PlaybookRunEvent, PlaybookRunEventKind,
    PlaybookRunGroup, PlaybookRunStatus, PlaybookTargetSelectorPreview, PlaybookTargetSet,
    PlaybookTargetSetListItem, PlaybookTargetSetMode, PlaybookTargetSetPreview,
    automation_audit_state, automation_execution_outcome, automation_lifecycle_event,
    parse_assignment_scope_kind, parse_run_status, parse_target_set_mode,
};
use crate::contexts::playbooks::infrastructure::persistence::{
    AppendRunGroupEventBatchOutcome, PLAYBOOK_ASSIGNMENT_COLUMNS, PLAYBOOK_RUN_COLUMNS,
    PLAYBOOK_RUN_GROUP_COLUMNS, PLAYBOOK_TARGET_SET_COLUMNS, PlaybookAssignmentListFilter,
    PlaybookRunListFilter, PlaybookScheduleListFilter, PlaybookTargetSetListFilter,
};
use crate::contexts::secrets::application::{SecretStore, resolve_secret_visibility};
use crate::contexts::secrets::domain::SECRET_KIND_NAMED;
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
pub struct ListPlaybookAssignmentsQuery {
    /// Comma-separated job template IDs; matches any (`IN`).
    pub job_template_id: Option<String>,
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
    /// Case-insensitive name search; empty or whitespace means no filter.
    pub search: Option<String>,
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

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct ListRunGroupConsoleQuery {
    pub from_sequence: Option<u64>,
    pub limit: Option<u32>,
    pub dispatch_id: Option<Uuid>,
}

/// OpenAPI representation of the shared console wire types.
#[derive(Debug, utoipa::ToSchema)]
pub enum ConsoleStreamSchema {
    #[serde(rename = "stdout")]
    Stdout,
    #[serde(rename = "stderr")]
    Stderr,
}

#[derive(Debug, utoipa::ToSchema)]
pub struct ConsoleChunkSchema {
    pub sequence: u64,
    pub stream: ConsoleStreamSchema,
    pub bytes: Vec<u8>,
}

#[derive(Debug, utoipa::ToSchema)]
pub struct RunnerConsoleBatchSchema {
    pub dispatch_id: Uuid,
    pub dispatch_attempt: i32,
    pub chunks: Vec<ConsoleChunkSchema>,
}

#[derive(Debug, utoipa::ToSchema)]
pub struct ConsoleAckSchema {
    pub dispatch_id: Uuid,
    pub dispatch_attempt: i32,
    pub next_sequence: u64,
}

#[derive(Debug, utoipa::ToSchema)]
pub struct ConsolePageSchema {
    pub dispatch_id: Option<Uuid>,
    pub dispatch_attempt: i32,
    pub chunks: Vec<ConsoleChunkSchema>,
    pub next_sequence: u64,
    pub complete: bool,
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
    pub source: crate::contexts::playbooks::domain::ProjectSource,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SyncProjectRequest {
    pub base_revision_id: Option<Uuid>,
    pub execution_environment_id: Uuid,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateJobTemplateRequest {
    pub name: String,
    pub project_id: Uuid,
    pub workdir: String,
    pub playbook_paths: Vec<String>,
    pub revision: crate::contexts::playbooks::domain::RevisionSelection,
    pub inventory: crate::contexts::playbooks::domain::InventorySelection,
    #[serde(default)]
    pub vars_schema: crate::contexts::playbooks::domain::VarsSchemaSource,
    pub options: crate::contexts::playbooks::domain::LaunchOptions,
    pub execution_environment_id: Uuid,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LaunchJobTemplateRequest {
    pub metadata: Option<Value>,
    /// Narrow the resolved managed inventory to these target IDs. Empty,
    /// duplicate, foreign IDs and selection with native inventory are rejected.
    pub target_ids: Option<Vec<Uuid>>,
    pub overrides: Option<crate::contexts::playbooks::domain::ParameterOverrides>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateScheduleRequest {
    pub template_id: Uuid,
    pub targets: crate::contexts::playbooks::domain::ScheduleTargets,
    pub overrides: Option<crate::contexts::playbooks::domain::ParameterOverrides>,
    pub name: String,
    pub cron_expression: String,
    pub timezone: Option<String>,
    pub misfire_policy: Option<crate::contexts::playbooks::domain::PlaybookScheduleMisfirePolicy>,
    pub concurrency_policy:
        Option<crate::contexts::playbooks::domain::PlaybookScheduleConcurrencyPolicy>,
    pub enabled: Option<bool>,
    pub next_run_at: Option<DateTime<Utc>>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateScheduleRequest {
    pub targets: Option<crate::contexts::playbooks::domain::ScheduleTargets>,
    pub overrides: Option<crate::contexts::playbooks::domain::ParameterOverrides>,
    pub name: Option<String>,
    pub cron_expression: Option<String>,
    pub timezone: Option<String>,
    pub misfire_policy: Option<crate::contexts::playbooks::domain::PlaybookScheduleMisfirePolicy>,
    pub concurrency_policy:
        Option<crate::contexts::playbooks::domain::PlaybookScheduleConcurrencyPolicy>,
    pub enabled: Option<bool>,
    pub next_run_at: Option<DateTime<Utc>>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct ListSchedulesQuery {
    pub template_id: Option<String>,
    pub enabled: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ListSchedulesResponse {
    pub items: Vec<crate::contexts::playbooks::domain::PlaybookSchedule>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateExecutionEnvironmentRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "deserialize_nullable_patch")]
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

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateProjectRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "deserialize_nullable_patch")]
    pub description: Option<Option<String>>,
    #[serde(default)]
    pub source: Option<crate::contexts::playbooks::domain::ProjectSource>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateJobTemplateRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub project_id: Option<Uuid>,
    #[serde(default)]
    pub workdir: Option<String>,
    #[serde(default)]
    pub playbook_paths: Option<Vec<String>>,
    #[serde(default)]
    pub revision: Option<crate::contexts::playbooks::domain::RevisionSelection>,
    #[serde(default)]
    pub inventory: Option<crate::contexts::playbooks::domain::InventorySelection>,
    #[serde(default)]
    pub vars_schema: Option<crate::contexts::playbooks::domain::VarsSchemaSource>,
    #[serde(default)]
    pub options: Option<crate::contexts::playbooks::domain::LaunchOptions>,
    #[serde(default)]
    pub execution_environment_id: Option<Uuid>,
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

/// Body of the unsaved-selector preview. `selector` and `limit` carry the same
/// meaning they carry when the set is saved: an omitted selector is the empty
/// object, and an omitted limit is one preview page.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct PreviewTargetSelectorRequest {
    pub selector: Option<Value>,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct TargetSetFacetsQuery {
    pub kind: crate::contexts::playbooks::domain::TargetSetFacetKind,
    pub selector: Option<String>,
    pub search: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreatePlaybookAssignmentRequest {
    pub job_template_id: Uuid,
    pub target_set_id: Uuid,
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
    pub job_template_id: Option<Uuid>,
    pub target_set_id: Option<Uuid>,
    #[serde(default, deserialize_with = "deserialize_nullable_patch")]
    pub name: Option<Option<String>>,
    pub scope_kind: Option<PlaybookAssignmentScopeKind>,
    #[serde(default, deserialize_with = "deserialize_nullable_patch")]
    pub scope_id: Option<Option<Uuid>>,
    pub priority: Option<i32>,
    pub enabled: Option<bool>,
    pub visible: Option<bool>,
    pub executable: Option<bool>,
    pub quick_action_enabled: Option<bool>,
    pub default_input: Option<Value>,
    pub default_metadata: Option<Value>,
    #[serde(default, deserialize_with = "deserialize_nullable_patch")]
    pub default_timeout_seconds: Option<Option<i32>>,
    #[serde(default, deserialize_with = "deserialize_nullable_patch")]
    pub default_target_limit: Option<Option<i32>>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct ListPlaybookRunsQuery {
    /// Comma-separated template IDs; matches any (`IN`).
    pub template_id: Option<String>,
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
    /// Comma-separated template IDs; matches any (`IN`).
    pub template_id: Option<String>,
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
    pub dispatch_id: Uuid,
    pub dispatch_attempt: i32,
    pub lease_owner: String,
    pub console_next_sequence: u64,
    pub group_status: PlaybookRunStatus,
    pub exit_code: Option<i32>,
    pub error_message: Option<String>,
    pub runs: Vec<CompleteRunGroupBatchItemRequest>,
}

impl CompleteRunGroupBatchRequest {
    pub(crate) fn validate(&self) -> Result<(), AppError> {
        // The group field is the process/lifecycle verdict and has no
        // `Unreported` member; a host run may be unreported.
        if !matches!(
            self.group_status,
            PlaybookRunStatus::Succeeded
                | PlaybookRunStatus::Failed
                | PlaybookRunStatus::Canceled
                | PlaybookRunStatus::TimedOut
        ) || self.runs.iter().any(|run| !run.status.is_terminal())
        {
            return Err(AppError::Validation(
                "completion statuses must be terminal".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct TargetSetPreviewQuery {
    pub limit: Option<i64>,
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
    pub items: Vec<crate::contexts::playbooks::domain::Project>,
    pub total: u64,
    pub limit: u32,
    pub offset: u32,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ListJobTemplatesResponse {
    pub items: Vec<crate::contexts::playbooks::domain::JobTemplate>,
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
    pub items: Vec<PlaybookTargetSetListItem>,
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

fn automation_audit_context(
    event_type: &str,
    target_type: &str,
    target_id: Option<Uuid>,
    metadata: &Value,
    request_id: Option<Uuid>,
    token_id: Option<Uuid>,
) -> AuditContext {
    let uuid_field = |key: &str| {
        metadata
            .get(key)
            .and_then(Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok())
    };
    let state = metadata
        .get("status")
        .and_then(Value::as_str)
        .and_then(|status| parse_run_status(status).ok())
        .or_else(|| {
            (event_type == "playbook.run_group.created").then_some(PlaybookRunStatus::Queued)
        });
    let outcome = if event_type.ends_with(".completed") {
        state.map_or(AuditOutcome::Unknown, automation_execution_outcome)
    } else if event_type.ends_with(".cancel_requested") {
        AuditOutcome::Succeeded
    } else if state.is_some() {
        AuditOutcome::Unknown
    } else {
        AuditOutcome::Succeeded
    };
    AuditContext {
        outcome,
        decision: Some(AuditDecision::Allowed),
        state: state.map(automation_audit_state),
        delegated_principal: token_id.map(|id| AuditDelegatedPrincipal {
            kind: AuditDelegatedPrincipalKind::PlaybookJobToken,
            id,
        }),
        emitter: Some(AuditEmitter {
            kind: AuditEmitterKind::Api,
            id: None,
        }),
        correlation: AuditCorrelation {
            request_id,
            run_id: uuid_field("run_id").or_else(|| {
                (target_type == "playbook_run")
                    .then_some(target_id)
                    .flatten()
            }),
            run_group_id: uuid_field("run_group_id").or_else(|| {
                (target_type == "playbook_run_group")
                    .then_some(target_id)
                    .flatten()
            }),
            ..Default::default()
        },
        ..Default::default()
    }
}

async fn record_playbook_audit(
    state: &AppState,
    request_context: &RequestContext,
    event_type: &str,
    target_type: &str,
    target_id: Option<Uuid>,
    metadata: Value,
) -> Result<(), AppError> {
    let context = automation_audit_context(
        event_type,
        target_type,
        target_id,
        &metadata,
        Uuid::parse_str(&request_context.correlation_id).ok(),
        None,
    );
    let metadata = project_metadata(event_type, &metadata);
    state
        .services
        .audit
        .record_event(
            AuditEvent {
                id: Uuid::now_v7(),
                event_type: event_type.to_string(),
                actor_id: request_actor_id(request_context),
                actor_kind: request_context.audit_actor_kind().to_string(),
                target_type: Some(target_type.to_string()),
                target_id,
                metadata,
                created_at: Utc::now(),
            }
            .with_context(context)?,
        )
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
    // One actor class for both bindings; the run-group binding is carried by
    // the `run_group_id` metadata below and lands in `correlation.run_group_id`.
    let token_id = parse_job_token(raw_token).map(|(token_id, _)| token_id);
    let context = automation_audit_context(
        event_type,
        target_type,
        target_id,
        &metadata,
        None,
        token_id,
    );
    let metadata = project_metadata(event_type, &metadata);
    let event = AuditEvent {
        id: Uuid::now_v7(),
        event_type: event_type.to_string(),
        actor_id: None,
        actor_kind: AUDIT_ACTOR_KIND_PLAYBOOK_JOB_TOKEN.to_string(),
        target_type: Some(target_type.to_string()),
        target_id,
        metadata,
        created_at: Utc::now(),
    }
    .with_context(context);
    let event = match event {
        Ok(event) => event,
        Err(error) => {
            log_error_event(
                "playbooks.callback.audit_failed",
                "playbook_callback_audit_error",
                error.to_string(),
                EventFields::default()
                    .with_operation("playbook_callback_audit")
                    .with_status("failed"),
            );
            return;
        }
    };
    let recorded = state.services.audit.record_event(event).await;
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

/// Gate binding a named secret to a playbook project's SCM credential.
///
/// A project resolves the bound secret into git checkout material, so binding
/// is disclosure-equivalent and requires the canonical per-secret
/// `Secret × SecretVariable` authority. Check authorization before loading the
/// secret so an unauthorized caller cannot distinguish an existing secret from
/// an absent one.
fn project_source_secret_id(
    source: &crate::contexts::playbooks::domain::ProjectSource,
) -> Option<Uuid> {
    use crate::contexts::playbooks::domain::{ProjectAuth, ProjectSource};
    match source {
        ProjectSource::Git {
            auth: ProjectAuth::HttpsBasic { secret_id, .. } | ProjectAuth::HttpsBearer { secret_id },
            ..
        } => Some(*secret_id),
        _ => None,
    }
}

fn project_source_kind(source: &crate::contexts::playbooks::domain::ProjectSource) -> &'static str {
    match source {
        crate::contexts::playbooks::domain::ProjectSource::Editable => "editable",
        crate::contexts::playbooks::domain::ProjectSource::Git { .. } => "git",
        crate::contexts::playbooks::domain::ProjectSource::System { .. } => "system",
    }
}

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

async fn ensure_project_revision_access(
    state: &AppState,
    context: &RequestContext,
    action: Action,
    resource_id: Option<&str>,
) -> Result<(), AppError> {
    state
        .services
        .policy_evaluation
        .ensure_access(
            context,
            action,
            ResourceType::PlaybookProjectRevision,
            resource_id,
        )
        .await
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ChangeProjectRevisionRequest {
    pub base_revision_id: Uuid,
    pub changes: Vec<crate::contexts::playbooks::domain::RevisionChange>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ForkProjectRequest {
    pub revision_id: Uuid,
    pub name: String,
}

#[utoipa::path(get, path = "/api/v1/playbooks/projects/{id}/revisions/{revision_id}", tag = "playbooks",
    params(("id" = Uuid, Path), ("revision_id" = Uuid, Path)),
    responses((status = 200, body = crate::contexts::playbooks::domain::ProjectRevision), (status = 404)))]
pub async fn get_project_revision(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path((id, revision_id)): Path<(Uuid, Uuid)>,
) -> Response {
    let context = request_context(&request_id, auth_context, &headers);
    if let Err(error) = context.require_authenticated_user_id() {
        return handle_app_error(error);
    }
    if let Err(error) =
        ensure_project_access(&state, &context, Action::Read, Some(&id.to_string())).await
    {
        return handle_app_error(error);
    }
    if let Err(error) = ensure_project_revision_access(
        &state,
        &context,
        Action::Read,
        Some(&revision_id.to_string()),
    )
    .await
    {
        return handle_app_error(error);
    }
    match state
        .services
        .playbooks
        .get_project_revision(id, revision_id)
        .await
    {
        Ok(Some(revision)) => Json(revision).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Project revision not found".into())),
        Err(error) => handle_app_error(error),
    }
}

#[utoipa::path(post, path = "/api/v1/playbooks/projects/{id}/revisions", tag = "playbooks",
    params(("id" = Uuid, Path)), request_body = ChangeProjectRevisionRequest,
    responses((status = 201, body = crate::contexts::playbooks::domain::ProjectRevision), (status = 409)))]
pub async fn change_project_revision(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    EnvelopeJson(payload): EnvelopeJson<ChangeProjectRevisionRequest>,
) -> Response {
    let context = request_context(&request_id, auth_context, &headers);
    let user_id = match context.require_authenticated_user_id() {
        Ok(id) => id,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) =
        ensure_project_access(&state, &context, Action::Update, Some(&id.to_string())).await
    {
        return handle_app_error(error);
    }
    if let Err(error) = ensure_project_revision_access(&state, &context, Action::Create, None).await
    {
        return handle_app_error(error);
    }
    match state
        .services
        .playbooks
        .change_project_revision(id, payload.base_revision_id, payload.changes, Some(user_id))
        .await
    {
        Ok(revision) => {
            if let Err(error) = record_playbook_audit(
                &state,
                &context,
                "playbook.project_revision.created",
                "playbook_project_revision",
                Some(revision.revision_id),
                json!({
                    "action": "create",
                    "reason_code": "project_revision_created",
                    "summary": "playbook project revision created",
                    "affected_count": 1,
                    "resource_type": "playbook_project_revision",
                    "resource_id": revision.revision_id.to_string(),
                    "project_id": id.to_string(),
                    "revision_id": revision.revision_id.to_string(),
                    "base_revision_id": payload.base_revision_id.to_string(),
                    "tree_sha256": revision.tree_sha256.clone(),
                }),
            )
            .await
            {
                return handle_app_error(error);
            }
            (StatusCode::CREATED, Json(revision)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

#[utoipa::path(post, path = "/api/v1/playbooks/projects/{id}/fork", tag = "playbooks",
    params(("id" = Uuid, Path)), request_body = ForkProjectRequest,
    responses((status = 201, body = crate::contexts::playbooks::domain::Project)))]
pub async fn fork_project(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    EnvelopeJson(payload): EnvelopeJson<ForkProjectRequest>,
) -> Response {
    let context = request_context(&request_id, auth_context, &headers);
    let user_id = match context.require_authenticated_user_id() {
        Ok(id) => id,
        Err(error) => return handle_app_error(error),
    };
    for (action, resource_id) in [(Action::Read, Some(id.to_string())), (Action::Create, None)] {
        if let Err(error) =
            ensure_project_access(&state, &context, action, resource_id.as_deref()).await
        {
            return handle_app_error(error);
        }
    }
    if let Err(error) = ensure_project_revision_access(
        &state,
        &context,
        Action::Read,
        Some(&payload.revision_id.to_string()),
    )
    .await
    {
        return handle_app_error(error);
    }
    match state
        .services
        .playbooks
        .fork_project_revision(id, payload.revision_id, payload.name, Some(user_id))
        .await
    {
        Ok(project) => {
            if let Err(error) = record_playbook_audit(
                &state,
                &context,
                "playbook.project.forked",
                "playbook_project",
                Some(project.id),
                json!({
                    "action": "fork",
                    "reason_code": "project_forked",
                    "summary": "playbook project forked",
                    "affected_count": 1,
                    "resource_type": "playbook_project",
                    "resource_id": project.id.to_string(),
                    "project_id": project.id.to_string(),
                    "source_project_id": id.to_string(),
                    "source_revision_id": payload.revision_id.to_string(),
                    "name": project.name.clone(),
                }),
            )
            .await
            {
                return handle_app_error(error);
            }
            (StatusCode::CREATED, Json(project)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
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
    responses((status = 200, description = "Project", body = crate::contexts::playbooks::domain::Project), (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse))
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
        (status = 201, description = "Project created", body = crate::contexts::playbooks::domain::Project),
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
    if let Err(error) = ensure_project_credential_use(
        &state,
        &request_context,
        project_source_secret_id(&payload.source),
    )
    .await
    {
        return handle_app_error(error);
    }
    let input = CreateProjectInput {
        name: payload.name,
        description: payload.description,
        source: payload.source,
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
                    "source_kind": project_source_kind(&item.source),
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
        (status = 200, description = "Project updated", body = crate::contexts::playbooks::domain::Project),
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
    let existing = match state.services.playbooks.get_project(id).await {
        Ok(Some(existing)) => existing,
        Ok(None) => {
            return handle_app_error(AppError::NotFound(format!(
                "Playbook project '{id}' not found"
            )));
        }
        Err(error) => return handle_app_error(error),
    };
    // Authorize the EFFECTIVE credential, not only a submitted one. An absent
    // `source` keeps the stored binding, so a caller with only project Update
    // must still be allowed to use that secret.
    let effective_source = payload.source.as_ref().unwrap_or(&existing.source);
    if let Err(error) = ensure_project_credential_use(
        &state,
        &request_context,
        project_source_secret_id(effective_source),
    )
    .await
    {
        return handle_app_error(error);
    }
    let input = UpdateProjectInput {
        name: payload.name,
        description: payload.description,
        source: payload.source,
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
                    "source_kind": project_source_kind(&item.source),
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

/// Get one builtin catalog job by slug with its truthful availability.
#[utoipa::path(
    get,
    path = "/api/v1/playbooks/catalog/{slug}",
    tag = "playbooks",
    params(("slug" = String, Path, description = "Catalog slug")),
    responses(
        (status = 200, description = "Builtin job lookup", body = BuiltinJobLookup),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn get_builtin_job_template(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(slug): Path<String>,
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
    match state.services.playbooks.resolve_builtin_job(&slug).await {
        Ok(lookup) => Json(lookup).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Get a job template.
#[utoipa::path(
    get,
    path = "/api/v1/playbooks/job-templates/{id}",
    tag = "playbooks",
    params(("id" = Uuid, Path, description = "Job template ID")),
    responses((status = 200, description = "Job template", body = crate::contexts::playbooks::domain::JobTemplate), (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse))
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

/// Get the effective launch parameter schema of a job template.
#[utoipa::path(
    get,
    path = "/api/v1/playbooks/job-templates/{id}/vars-schema",
    tag = "playbooks",
    params(("id" = Uuid, Path, description = "Job template ID")),
    responses((status = 200, description = "Effective parameter schema", body = crate::contexts::playbooks::application::JobTemplateVarsSchema), (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse))
)]
pub async fn get_job_template_vars_schema(
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
    match state
        .services
        .playbooks
        .get_job_template_vars_schema(id)
        .await
    {
        Ok(Some(item)) => Json(item).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound(format!(
            "Playbook job template '{id}' not found"
        ))),
        Err(error) => handle_app_error(error),
    }
}

/// Authorize the related resources a job template request references (project,
/// managed target set, execution environment) before persisting.
async fn ensure_job_template_related_access(
    state: &AppState,
    request_context: &RequestContext,
    project_id: Option<Uuid>,
    inventory: Option<&crate::contexts::playbooks::domain::InventorySelection>,
    execution_environment_id: Option<Uuid>,
) -> Result<(), AppError> {
    if let Some(project_id) = project_id {
        ensure_project_access(
            state,
            request_context,
            Action::Read,
            Some(&project_id.to_string()),
        )
        .await?;
    }
    if let Some(crate::contexts::playbooks::domain::InventorySelection::Managed { target_set_id }) =
        inventory
    {
        ensure_target_set_access(
            state,
            request_context,
            Action::Read,
            Some(&target_set_id.to_string()),
        )
        .await?;
    }
    if let Some(execution_environment_id) = execution_environment_id {
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

/// Create a job template.
#[utoipa::path(
    post,
    path = "/api/v1/playbooks/job-templates",
    tag = "playbooks",
    request_body = CreateJobTemplateRequest,
    responses(
        (status = 201, description = "Job template created", body = crate::contexts::playbooks::domain::JobTemplate),
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
    if let Err(error) = ensure_job_template_related_access(
        &state,
        &request_context,
        Some(payload.project_id),
        Some(&payload.inventory),
        Some(payload.execution_environment_id),
    )
    .await
    {
        return handle_app_error(error);
    }
    if let Err(error) = ensure_job_template_credential_visibility(
        &state,
        &request_context,
        !payload.options.secret_refs.is_empty(),
    )
    .await
    {
        return handle_app_error(error);
    }
    let input = CreateJobTemplateInput {
        template: crate::contexts::playbooks::domain::JobTemplate {
            id: Uuid::nil(),
            name: payload.name,
            project_id: payload.project_id,
            workdir: payload.workdir,
            playbook_paths: payload.playbook_paths,
            revision: payload.revision,
            inventory: payload.inventory,
            vars_schema: payload.vars_schema,
            options: payload.options,
            execution_environment_id: payload.execution_environment_id,
        },
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
                    "name": item.name.clone(),
                    "project_id": item.project_id.to_string(),
                    "execution_environment_id": item.execution_environment_id.to_string(),
                    "secret_ref_count": item.options.secret_refs.len(),
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
        (status = 200, description = "Job template updated", body = crate::contexts::playbooks::domain::JobTemplate),
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
    let existing = match state.services.playbooks.get_job_template(id).await {
        Ok(Some(existing)) => existing,
        Ok(None) => {
            return handle_app_error(AppError::NotFound(format!(
                "Playbook job template '{id}' not found"
            )));
        }
        Err(error) => return handle_app_error(error),
    };
    let project_id = payload.project_id.unwrap_or(existing.project_id);
    let inventory = payload.inventory.as_ref().unwrap_or(&existing.inventory);
    let execution_environment_id = payload
        .execution_environment_id
        .unwrap_or(existing.execution_environment_id);
    if let Err(error) = ensure_job_template_related_access(
        &state,
        &request_context,
        Some(project_id),
        Some(inventory),
        Some(execution_environment_id),
    )
    .await
    {
        return handle_app_error(error);
    }
    let needs_secret_visibility = payload
        .options
        .as_ref()
        .map(|options| !options.secret_refs.is_empty())
        .unwrap_or(!existing.options.secret_refs.is_empty());
    if let Err(error) =
        ensure_job_template_credential_visibility(&state, &request_context, needs_secret_visibility)
            .await
    {
        return handle_app_error(error);
    }
    let input = UpdateJobTemplateInput {
        name: payload.name,
        project_id: payload.project_id,
        workdir: payload.workdir,
        playbook_paths: payload.playbook_paths,
        revision: payload.revision,
        inventory: payload.inventory,
        vars_schema: payload.vars_schema,
        options: payload.options,
        execution_environment_id: payload.execution_environment_id,
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
                    "project_id": item.project_id.to_string(),
                    "execution_environment_id": item.execution_environment_id.to_string(),
                    "secret_ref_count": item.options.secret_refs.len(),
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
        (status = 409, description = "Deletion blocked by dependent resources", body = crate::api::rest::error_mapping::ApiErrorResponse),
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

/// Only authorization refusals are denials; validation and infrastructure errors are not.
fn denied_launch_audit(
    context: &RequestContext,
    template_id: Uuid,
    error: &AppError,
) -> Result<Option<AuditEvent>, AppError> {
    if !matches!(error, AppError::Forbidden(_)) {
        return Ok(None);
    }
    AuditEvent {
        id: Uuid::now_v7(),
        event_type: "playbook.job_template.launch_denied".into(),
        actor_id: context.user_id,
        actor_kind: context.audit_actor_kind().into(),
        target_type: Some("playbook_job_template".into()),
        target_id: Some(template_id),
        metadata: project_metadata("playbook.job_template.launch_denied", &json!({})),
        created_at: Utc::now(),
    }
    .with_context(AuditContext {
        outcome: AuditOutcome::Denied,
        decision: Some(AuditDecision::Denied),
        emitter: Some(AuditEmitter {
            kind: AuditEmitterKind::Api,
            id: None,
        }),
        correlation: AuditCorrelation {
            request_id: Uuid::parse_str(&context.correlation_id).ok(),
            ..Default::default()
        },
        ..Default::default()
    })
    .map(Some)
}

async fn launch_error_response(
    state: &AppState,
    context: &RequestContext,
    template_id: Uuid,
    error: AppError,
) -> Response {
    match denied_launch_audit(context, template_id, &error) {
        Ok(Some(event)) => {
            if let Err(audit_error) = state.services.audit.record_event(event).await {
                return handle_app_error(audit_error);
            }
        }
        Ok(None) => {}
        Err(audit_error) => return handle_app_error(audit_error),
    }
    handle_app_error(error)
}

/// Launch a job template with its pinned configuration.
#[utoipa::path(post, path = "/api/v1/playbooks/job-templates/{id}/launch", tag = "playbooks",
    params(("id" = Uuid, Path)), request_body = LaunchJobTemplateRequest,
    responses((status = 201, body = crate::contexts::playbooks::application::playbook_service::PlaybookRunGroupCreated), (status = 403, description = "Access denied")))]
pub async fn launch_job_template(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    EnvelopeJson(payload): EnvelopeJson<LaunchJobTemplateRequest>,
) -> Response {
    let context = request_context(&request_id, auth_context, &headers);
    let user_id = match context.require_authenticated_user_id() {
        Ok(id) => id,
        Err(e) => return handle_app_error(e),
    };
    if let Err(e) =
        ensure_job_template_access(&state, &context, Action::Execute, Some(&id.to_string())).await
    {
        return launch_error_response(&state, &context, id, e).await;
    }
    match state
        .services
        .playbooks
        .launch_job_template(
            id,
            LaunchJobTemplateInput {
                metadata: payload.metadata,
                target_ids: payload.target_ids,
                overrides: payload.overrides,
            },
            user_id,
            &context,
        )
        .await
    {
        Ok(created) => {
            let event = match automation_lifecycle_event(
                "playbook.run_group.created",
                &created.run_group,
                None,
                AuditOutcome::Unknown,
                AuditEmitterKind::Api,
                created.run_group.created_at,
            ) {
                Ok(event) => event,
                Err(error) => return handle_app_error(error),
            };
            let mut audit_context = event.audit_context();
            audit_context.decision = Some(AuditDecision::Allowed);
            let event = match event.with_context(audit_context) {
                Ok(event) => event,
                Err(error) => return handle_app_error(error),
            };
            if let Err(error) = state.services.audit.record_event(event).await {
                return handle_app_error(error);
            }
            (StatusCode::CREATED, Json(created)).into_response()
        }
        Err(e) => launch_error_response(&state, &context, id, e).await,
    }
}

async fn ensure_schedule_access(
    state: &AppState,
    context: &RequestContext,
    action: Action,
    id: Option<&str>,
) -> Result<(), AppError> {
    crate::api::permission_helpers::ensure_access_with_resource(
        state,
        context,
        action,
        ResourceType::PlaybookSchedule,
        id,
    )
    .await
}

/// A schedule records the saved target set its selection was copied from, so
/// the caller must be allowed to read that set.
async fn ensure_schedule_target_set_access(
    state: &AppState,
    context: &RequestContext,
    targets: &crate::contexts::playbooks::domain::ScheduleTargets,
) -> Result<(), AppError> {
    let target_set_id = match targets {
        crate::contexts::playbooks::domain::ScheduleTargets::Fixed { target_set_id, .. }
        | crate::contexts::playbooks::domain::ScheduleTargets::Dynamic { target_set_id, .. } => {
            *target_set_id
        }
    };
    if let Some(target_set_id) = target_set_id {
        ensure_target_set_access(
            state,
            context,
            Action::Read,
            Some(&target_set_id.to_string()),
        )
        .await?;
    }
    Ok(())
}

/// List schedules.
#[utoipa::path(get, path = "/api/v1/playbooks/schedules", tag = "playbooks", params(ListSchedulesQuery), responses((status = 200, body = ListSchedulesResponse)))]
pub async fn list_schedules(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ListSchedulesQuery>,
) -> Response {
    let context = request_context(&request_id, auth_context, &headers);
    if let Err(e) = context.require_authenticated_user_id() {
        return handle_app_error(e);
    }
    if let Err(e) = ensure_schedule_access(&state, &context, Action::Read, None).await {
        return handle_app_error(e);
    }
    let template_id = match parse_csv_uuids(query.template_id.as_deref(), "template_id") {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };
    let enabled = match parse_csv_bools(query.enabled.as_deref()) {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };
    let filter = PlaybookScheduleListFilter {
        template_id,
        enabled,
        ..Default::default()
    };
    let limit = query.limit.unwrap_or(100).clamp(1, 500);
    let offset = query.offset.unwrap_or(0).max(0);
    match state
        .services
        .playbooks
        .list_schedules(&filter, limit, offset)
        .await
    {
        Ok((items, total)) => Json(ListSchedulesResponse {
            items,
            total,
            limit,
            offset,
        })
        .into_response(),
        Err(e) => handle_app_error(e),
    }
}

/// Retrieve a schedule.
#[utoipa::path(get, path = "/api/v1/playbooks/schedules/{id}", tag = "playbooks", params(("id" = Uuid, Path)), responses((status = 200, body = crate::contexts::playbooks::domain::PlaybookSchedule)))]
pub async fn get_schedule(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let context = request_context(&request_id, auth_context, &headers);
    if let Err(e) = context.require_authenticated_user_id() {
        return handle_app_error(e);
    }
    if let Err(e) =
        ensure_schedule_access(&state, &context, Action::Read, Some(&id.to_string())).await
    {
        return handle_app_error(e);
    }
    match state.services.playbooks.get_schedule(id).await {
        Ok(Some(item)) => Json(item).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound(format!("Schedule '{id}' not found"))),
        Err(e) => handle_app_error(e),
    }
}

/// Create a schedule owned by the authenticated user.
#[utoipa::path(post, path = "/api/v1/playbooks/schedules", tag = "playbooks", request_body = CreateScheduleRequest, responses((status = 201, body = crate::contexts::playbooks::domain::PlaybookSchedule)))]
pub async fn create_schedule(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    EnvelopeJson(p): EnvelopeJson<CreateScheduleRequest>,
) -> Response {
    let context = request_context(&request_id, auth_context, &headers);
    let user = match context.require_authenticated_user_id() {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };
    if let Err(e) = ensure_schedule_access(&state, &context, Action::Create, None).await {
        return handle_app_error(e);
    }
    if let Err(e) = ensure_job_template_access(
        &state,
        &context,
        Action::Execute,
        Some(&p.template_id.to_string()),
    )
    .await
    {
        return handle_app_error(e);
    }
    if let Err(e) = ensure_schedule_target_set_access(&state, &context, &p.targets).await {
        return handle_app_error(e);
    }
    let input = CreateScheduleInput {
        template_id: p.template_id,
        targets: p.targets,
        overrides: p.overrides,
        name: p.name,
        cron_expression: p.cron_expression,
        timezone: p.timezone,
        misfire_policy: p.misfire_policy,
        concurrency_policy: p.concurrency_policy,
        enabled: p.enabled,
        next_run_at: p.next_run_at,
        metadata: p.metadata,
    };
    match state
        .services
        .playbooks
        .create_schedule(input, Some(user))
        .await
    {
        Ok(item) => (StatusCode::CREATED, Json(item)).into_response(),
        Err(e) => handle_app_error(e),
    }
}

/// Update a schedule without changing its owner.
#[utoipa::path(patch, path = "/api/v1/playbooks/schedules/{id}", tag = "playbooks", params(("id" = Uuid, Path)), request_body = UpdateScheduleRequest, responses((status = 200, body = crate::contexts::playbooks::domain::PlaybookSchedule)))]
pub async fn update_schedule(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    EnvelopeJson(p): EnvelopeJson<UpdateScheduleRequest>,
) -> Response {
    let context = request_context(&request_id, auth_context, &headers);
    let user = match context.require_authenticated_user_id() {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };
    if let Err(e) =
        ensure_schedule_access(&state, &context, Action::Update, Some(&id.to_string())).await
    {
        return handle_app_error(e);
    }
    let existing = match state.services.playbooks.get_schedule(id).await {
        Ok(Some(v)) => v,
        Ok(None) => {
            return handle_app_error(AppError::NotFound(format!("Schedule '{id}' not found")));
        }
        Err(e) => return handle_app_error(e),
    };
    // The service retains the original owner on update. Do not let another
    // user change the job while the scheduler would attribute it to that user.
    if existing.owner_user_id != user {
        return handle_app_error(AppError::Forbidden(
            "Only the schedule owner can update this schedule".to_string(),
        ));
    }
    if let Err(e) = ensure_job_template_access(
        &state,
        &context,
        Action::Execute,
        Some(&existing.template_id.to_string()),
    )
    .await
    {
        return handle_app_error(e);
    }
    if let Some(targets) = p.targets.as_ref() {
        if let Err(e) = ensure_schedule_target_set_access(&state, &context, targets).await {
            return handle_app_error(e);
        }
    }
    let input = UpdateScheduleInput {
        targets: p.targets,
        overrides: p.overrides,
        name: p.name,
        cron_expression: p.cron_expression,
        timezone: p.timezone,
        misfire_policy: p.misfire_policy,
        concurrency_policy: p.concurrency_policy,
        enabled: p.enabled,
        next_run_at: p.next_run_at,
        metadata: p.metadata,
    };
    match state
        .services
        .playbooks
        .update_schedule(id, input, Some(user))
        .await
    {
        Ok(Some(item)) => Json(item).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound(format!("Schedule '{id}' not found"))),
        Err(e) => handle_app_error(e),
    }
}

/// Delete a schedule.
#[utoipa::path(delete, path = "/api/v1/playbooks/schedules/{id}", tag = "playbooks", params(("id" = Uuid, Path)), responses((status = 204, description = "Deleted")))]
pub async fn delete_schedule(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let context = request_context(&request_id, auth_context, &headers);
    if let Err(e) = context.require_authenticated_user_id() {
        return handle_app_error(e);
    }
    if let Err(e) =
        ensure_schedule_access(&state, &context, Action::Delete, Some(&id.to_string())).await
    {
        return handle_app_error(e);
    }
    match state.services.playbooks.delete_schedule(id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => handle_app_error(AppError::NotFound(format!("Schedule '{id}' not found"))),
        Err(e) => handle_app_error(e),
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
        .create_artifact_with_job_token(
            run_id,
            raw_token.as_str(),
            input,
            crate::contexts::playbooks::domain::JobTokenAudience::Supervisor,
            Utc::now(),
        )
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

/// Append an ordered batch of console bytes using the group callback token.
#[utoipa::path(
    post,
    path = "/api/v1/playbooks/run-groups/{group_id}/console/batch",
    tag = "playbooks",
    params(("group_id" = Uuid, Path, description = "Playbook run group ID")),
    request_body = RunnerConsoleBatchSchema,
    responses(
        (status = 200, description = "Durable console prefix", body = ConsoleAckSchema),
        (status = 400, description = "Invalid console batch", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Invalid or expired callback token", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Callback token scope denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 409, description = "Console identity or prefix conflict", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_run_group_console_batch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(group_id): Path<Uuid>,
    EnvelopeJson(payload): EnvelopeJson<RunnerConsoleBatch>,
) -> Response {
    let raw_token = match bearer_token(&headers) {
        Ok(token) => token,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = validate_console_batch(&payload) {
        return handle_app_error(AppError::Validation(error));
    }
    match state
        .services
        .playbooks
        .append_console_batch_with_group_callback_token(group_id, raw_token.as_str(), payload)
        .await
    {
        Ok(ack) => Json(ack).into_response(),
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
        (status = 409, description = "Dispatch or console prefix conflict", body = crate::api::rest::error_mapping::ApiErrorResponse),
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
        dispatch_id: payload.dispatch_id,
        dispatch_attempt: payload.dispatch_attempt,
        lease_owner: payload.lease_owner,
        console_next_sequence: payload.console_next_sequence,
        group_status: payload.group_status,
        exit_code: payload.exit_code,
        error_message: payload.error_message,
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
    if let Err(error) = request_context.require_authenticated_user_id() {
        return handle_app_error(error);
    }
    if let Err(error) = ensure_run_access(
        &state,
        &request_context,
        Action::Update,
        Some(&run_id.to_string()),
    )
    .await
    {
        return handle_app_error(error);
    }
    let run = match state.services.playbooks.get_run(run_id).await {
        Ok(Some(run)) => run,
        Ok(None) => {
            return handle_app_error(AppError::NotFound(format!(
                "Playbook run '{run_id}' not found"
            )));
        }
        Err(error) => return handle_app_error(error),
    };
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
            "template_id": run_group.template_id.to_string(),
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
    if let Err(error) = request_context.require_authenticated_user_id() {
        return handle_app_error(error);
    }
    if let Err(error) = ensure_run_group_access(
        &state,
        &request_context,
        Action::Update,
        Some(&group_id.to_string()),
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
            "template_id": updated.run_group.template_id.to_string(),
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

/// Replay a durable console page for an authorized user.
#[utoipa::path(
    get,
    path = "/api/v1/playbooks/run-groups/{group_id}/console",
    tag = "playbooks",
    params(ListRunGroupConsoleQuery, ("group_id" = Uuid, Path, description = "Playbook run group ID")),
    responses(
        (status = 200, description = "Console byte page", body = ConsolePageSchema),
        (status = 400, description = "Invalid query", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 409, description = "Dispatch identity mismatch", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_run_group_console(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(group_id): Path<Uuid>,
    Query(query): Query<ListRunGroupConsoleQuery>,
) -> Response {
    let request_context = request_context(&request_id, auth_context, &headers);
    if let Err(error) = request_context.require_authenticated_user_id() {
        return handle_app_error(error);
    }
    let resource_id = group_id.to_string();
    if let Err(error) =
        ensure_run_group_access(&state, &request_context, Action::Read, Some(&resource_id)).await
    {
        return handle_app_error(error);
    }
    match state
        .services
        .playbooks
        .list_console(
            group_id,
            query.from_sequence.unwrap_or(0),
            i64::from(query.limit.unwrap_or(64).clamp(1, 64)),
            query.dispatch_id,
        )
        .await
    {
        Ok(page) => Json(page).into_response(),
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
    let filter = PlaybookTargetSetListFilter {
        mode,
        sort,
        search: query.search,
    };
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
                    "mode": preview.target_set.mode,
                    "selector_keys": selector_keys(&preview.target_set.selector),
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
        Ok(outcome) => {
            let target_set = outcome.target_set;
            if let Err(error) = record_playbook_audit(
                &state,
                &request_context,
                "playbook.target_set.updated",
                "playbook_target_set",
                Some(target_set.id),
                json!({
                    "action": "update",
                    "mode": target_set.mode,
                    "changed_selector_keys": outcome.changed_selector_keys,
                    "mode_changed": outcome.mode_changed,
                    "name_changed": outcome.name_changed,
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
        (status = 409, description = "Target set is used by job templates, assignments or schedules (DELETE_BLOCKED with blocker tables, counts, and examples)", body = crate::api::rest::error_mapping::ApiErrorResponse),
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

#[utoipa::path(
    get,
    path = "/api/v1/playbooks/target-sets/facets",
    tag = "playbooks",
    params(TargetSetFacetsQuery),
    responses(
        (status = 200, description = "Inventory facet identifiers and matching host counts", body = crate::contexts::playbooks::domain::TargetSetFacetPage),
        (status = 400, description = "Invalid facet query", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_target_set_facets(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Query(query): Query<TargetSetFacetsQuery>,
) -> Response {
    let context = request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_target_set_access(&state, &context, Action::Read, None).await {
        return handle_app_error(error);
    }
    let selector = match query
        .selector
        .as_deref()
        .map(serde_json::from_str::<Value>)
        .transpose()
    {
        Ok(selector) => selector,
        Err(_) => {
            return handle_app_error(AppError::Validation(
                "Facet selector must be valid JSON".into(),
            ));
        }
    };
    match state
        .services
        .playbooks
        .list_target_set_facets(
            query.kind,
            selector,
            query.search,
            query.limit.unwrap_or(50),
            query.offset.unwrap_or(0),
        )
        .await
    {
        Ok(page) => Json(page).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Preview a target selector that has not been saved as a target set.
#[utoipa::path(
    post,
    path = "/api/v1/playbooks/target-sets/preview",
    tag = "playbooks",
    request_body = PreviewTargetSelectorRequest,
    responses(
        (status = 200, description = "Resolved selector preview; nothing was saved", body = PlaybookTargetSelectorPreview),
        (status = 400, description = "Invalid selector or limit", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn preview_target_selector(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<PreviewTargetSelectorRequest>,
) -> Response {
    let request_context = request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_target_set_access(&state, &request_context, Action::Read, None).await
    {
        return handle_app_error(error);
    }
    match state
        .services
        .playbooks
        .preview_target_selector(payload.selector, payload.limit.unwrap_or(100))
        .await
    {
        Ok(preview) => Json(preview).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Acquire a Git project in the isolated runner and import its immutable revision.
#[utoipa::path(
    post,
    path = "/api/v1/playbooks/projects/{id}/sync",
    tag = "playbooks",
    params(("id" = Uuid, Path, description = "Project ID")),
    request_body = SyncProjectRequest,
    responses(
        (status = 201, description = "Acquired project revision", body = crate::contexts::playbooks::domain::ProjectRevision),
        (status = 400, description = "Invalid source, disabled or unpinned image, or unsupported SSH record authentication", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Project, environment, controller or secret delegation denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Project or execution environment not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 409, description = "Project or base revision changed during acquisition", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 502, description = "Runner acquisition failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn sync_project(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    EnvelopeJson(payload): EnvelopeJson<SyncProjectRequest>,
) -> Response {
    use crate::contexts::playbooks::domain::{
        ProjectAuth, ProjectEntry, ProjectEntryContent, ProjectSource,
    };

    let context = request_context(&request_id, auth_context, &headers);
    let user_id = match context.require_authenticated_user_id() {
        Ok(user_id) => user_id,
        Err(error) => return handle_app_error(error),
    };
    for action in [Action::Update, Action::ExecuteNative] {
        if let Err(error) =
            ensure_project_access(&state, &context, action, Some(&id.to_string())).await
        {
            return handle_app_error(error);
        }
    }
    if let Err(error) = ensure_execution_environment_access(
        &state,
        &context,
        Action::Read,
        Some(&payload.execution_environment_id.to_string()),
    )
    .await
    {
        return handle_app_error(error);
    }
    let project = match state.services.playbooks.get_project(id).await {
        Ok(Some(project)) => project,
        Ok(None) => {
            return handle_app_error(AppError::NotFound(format!("Project '{id}' not found")));
        }
        Err(error) => return handle_app_error(error),
    };
    let environment = match state
        .services
        .playbooks
        .get_execution_environment(payload.execution_environment_id)
        .await
    {
        Ok(Some(environment)) => environment,
        Ok(None) => {
            return handle_app_error(AppError::NotFound(
                "Execution environment not found".to_string(),
            ));
        }
        Err(error) => return handle_app_error(error),
    };
    if !environment.enabled {
        return handle_app_error(AppError::Validation(
            "Execution environment is disabled".to_string(),
        ));
    }
    let (image, image_digest) =
        match crate::contexts::playbooks::application::playbook_service::split_pinned_image(
            &environment.image,
        ) {
            Ok(image) => image,
            Err(error) => return handle_app_error(error),
        };
    let (url, reference, source_auth) = match project.source {
        ProjectSource::Git {
            url,
            reference,
            auth,
        } => (url, reference, auth),
        ProjectSource::Editable | ProjectSource::System { .. } => {
            return handle_app_error(AppError::Validation(
                "Only Git projects can be synced".to_string(),
            ));
        }
    };
    let auth = match source_auth {
        ProjectAuth::Anonymous => AcquireAuth::Anonymous,
        ProjectAuth::SshRecord { ssh_record_id } => {
            let Some(user_id) = context.user_id else {
                return handle_app_error(AppError::Forbidden(
                    "Git credential delegation requires an authenticated user".into(),
                ));
            };
            let principal = Principal::user(user_id);
            let connection = match state
                .services
                .connection
                .resolve_delegated_ssh_record(
                    ssh_record_id,
                    Some(user_id),
                    crate::contexts::connections::application::connection_service::SshRecordPurpose::EndpointBound,
                    &RecordSshDelegation::for_launch(
                        &state.services.access_control,
                        &principal,
                        Some(&context),
                    ),
                )
                .await
            {
                Ok(connection) => connection,
                Err(error) => return handle_app_error(error),
            };
            // The acquisition boundary checks URL host, port and explicit user
            // against this exact endpoint before creating a Git sandbox.
            AcquireAuth::SshKey { connection }
        }
        ProjectAuth::HttpsBasic {
            username,
            secret_id,
        } => {
            let password = match acquire_project_secret(&state, &context, secret_id).await {
                Ok(value) => value,
                Err(error) => return handle_app_error(error),
            };
            AcquireAuth::HttpsBasic { username, password }
        }
        ProjectAuth::HttpsBearer { secret_id } => {
            let token = match acquire_project_secret(&state, &context, secret_id).await {
                Ok(value) => value,
                Err(error) => return handle_app_error(error),
            };
            AcquireAuth::HttpsBearer { token }
        }
    };
    let acquisition_id = Uuid::now_v7();
    let request = AcquireProjectRequest {
        protocol_version: RUNNER_CONTRACT_VERSION,
        acquisition_id,
        url,
        reference,
        auth,
        image,
        image_digest,
    };
    let client =
        match crate::contexts::playbooks::infrastructure::AnsibleRunnerClient::from_settings(
            &state.settings,
        ) {
            Ok(client) => client,
            Err(_) => {
                return handle_app_error(AppError::External(
                    "Project acquisition is unavailable".to_string(),
                ));
            }
        };
    let receipt = match client.acquire_project(&request).await {
        Ok(receipt) => receipt,
        Err(crate::contexts::playbooks::infrastructure::ansible_runner_client::ProjectAcquisitionError::Failure(code)) => {
            return crate::api::rest::error_mapping::build_error_response(
                StatusCode::BAD_GATEWAY,
                "EXTERNAL_ERROR",
                "Project acquisition failed",
                code.as_str(),
            );
        }
        // Never expose untrusted response bodies or transport diagnostics.
        Err(_) => {
            return handle_app_error(AppError::External("Project acquisition failed".to_string()));
        }
    };
    let entries = receipt
        .entries
        .into_iter()
        .map(|entry| ProjectEntry {
            path: entry.path,
            content: match entry.content {
                SnapshotContent::File { bytes, executable } => {
                    ProjectEntryContent::File { bytes, executable }
                }
                SnapshotContent::Symlink { target } => ProjectEntryContent::Symlink { target },
            },
        })
        .collect();
    match state
        .services
        .playbooks
        .import_project_revision(
            id,
            payload.base_revision_id,
            project.updated_at,
            receipt.source_commit_oid,
            entries,
            Some(user_id),
        )
        .await
    {
        Ok(revision) => {
            if let Err(error) = record_playbook_audit(&state, &context, "playbook.project.synced", "playbook_project", Some(id), json!({
                "action": "sync", "project_id": id, "acquisition_id": acquisition_id, "revision_id": revision.revision_id,
            })).await {
                return handle_app_error(error);
            }
            (StatusCode::CREATED, Json(revision)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

async fn acquire_project_secret(
    state: &AppState,
    context: &RequestContext,
    secret_id: Uuid,
) -> Result<String, AppError> {
    let resource_id = format!("secret_variable:{secret_id}");
    for action in [Action::Execute, Action::DelegateCredentials] {
        build_policy_evaluation_service(state)
            .ensure_access(
                context,
                action,
                ResourceType::SecretVariable,
                Some(&resource_id),
            )
            .await?;
    }
    let secret = state
        .repositories
        .secrets
        .get_variable_by_id(secret_id)
        .await?
        .ok_or_else(|| {
            AppError::Validation("Project credential is not an existing named secret".to_string())
        })?;
    if secret.kind != SECRET_KIND_NAMED || secret.is_protected {
        return Err(AppError::Forbidden(
            "Project credential cannot be used for acquisition".to_string(),
        ));
    }
    state.services.secret_store.get_secret(secret_id).await
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
    let job_template_id = match parse_csv_uuids(query.job_template_id.as_deref(), "job_template_id")
    {
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
        job_template_id,
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
    let playbook_resource_id = payload.job_template_id.to_string();
    if let Err(error) = ensure_job_template_access(
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
    let input = CreateAssignmentInput {
        job_template_id: payload.job_template_id,
        target_set_id: payload.target_set_id,
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
                    "job_template_id": assignment.job_template_id.to_string(),
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
    if let Some(job_template_id) = payload.job_template_id {
        let playbook_resource_id = job_template_id.to_string();
        if let Err(error) = ensure_job_template_access(
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
    let input = UpdateAssignmentInput {
        job_template_id: payload.job_template_id,
        target_set_id: payload.target_set_id,
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
                    "job_template_id": assignment.job_template_id.to_string(),
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

/// List playbook runs for history, filterable by template and/or status.
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
    let template_id = match parse_csv_uuids(query.template_id.as_deref(), "template_id") {
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
        template_id,
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

/// List playbook run groups for history, filterable by template and/or status.
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
    let template_id = match parse_csv_uuids(query.template_id.as_deref(), "template_id") {
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
        template_id,
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
        CompleteRunGroupBatchRequest, CreateRunGroupEventsBatchRequest, bearer_token,
        ensure_single_run_cancel_allowed, parse_csv_assignment_scope_kinds, parse_csv_bools,
        parse_csv_run_statuses, parse_csv_target_set_modes,
    };
    use crate::contexts::playbooks::domain::{
        PlaybookAssignmentScopeKind, PlaybookRunStatus, PlaybookTargetSetMode,
    };
    use crate::errors::AppError;
    use axum::http::{HeaderMap, HeaderValue, header::AUTHORIZATION};
    use serde_json::json;
    use uuid::Uuid;

    #[test]
    fn nullable_automation_patch_preserves_omission_clear_and_value() {
        use super::{
            UpdateExecutionEnvironmentRequest, UpdatePlaybookAssignmentRequest,
            UpdateProjectRequest,
        };

        let omitted: UpdateProjectRequest = serde_json::from_value(json!({})).unwrap();
        let cleared: UpdateProjectRequest =
            serde_json::from_value(json!({"description": null})).unwrap();
        let set: UpdateProjectRequest =
            serde_json::from_value(json!({"description": "project"})).unwrap();
        assert_eq!(omitted.description, None);
        assert_eq!(cleared.description, Some(None));
        assert_eq!(set.description, Some(Some("project".into())));

        let omitted: UpdateExecutionEnvironmentRequest = serde_json::from_value(json!({})).unwrap();
        let cleared: UpdateExecutionEnvironmentRequest =
            serde_json::from_value(json!({"description": null})).unwrap();
        let set: UpdateExecutionEnvironmentRequest =
            serde_json::from_value(json!({"description": "environment"})).unwrap();
        assert_eq!(omitted.description, None);
        assert_eq!(cleared.description, Some(None));
        assert_eq!(set.description, Some(Some("environment".into())));

        let omitted: UpdatePlaybookAssignmentRequest = serde_json::from_value(json!({})).unwrap();
        let cleared: UpdatePlaybookAssignmentRequest = serde_json::from_value(json!({
            "name": null, "scope_id": null, "default_timeout_seconds": null, "default_target_limit": null
        })).unwrap();
        let scope_id = Uuid::new_v4();
        let set: UpdatePlaybookAssignmentRequest = serde_json::from_value(json!({
            "name": "assignment", "scope_id": scope_id, "default_timeout_seconds": 30, "default_target_limit": 2
        })).unwrap();
        assert_eq!(
            (
                omitted.name,
                omitted.scope_id,
                omitted.default_timeout_seconds,
                omitted.default_target_limit
            ),
            (None, None, None, None)
        );
        assert_eq!(
            (
                cleared.name,
                cleared.scope_id,
                cleared.default_timeout_seconds,
                cleared.default_target_limit
            ),
            (Some(None), Some(None), Some(None), Some(None))
        );
        assert_eq!(
            (
                set.name,
                set.scope_id,
                set.default_timeout_seconds,
                set.default_target_limit
            ),
            (
                Some(Some("assignment".into())),
                Some(Some(scope_id)),
                Some(Some(30)),
                Some(Some(2))
            )
        );
    }

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

    #[test]
    fn launch_denial_audit_preserves_user_and_pat_identity_without_error_payloads() {
        use crate::contexts::audit::domain::{AuditDecision, AuditEmitterKind, AuditOutcome};
        use crate::contexts::shared::request_context::RequestContext;
        let user_id = Uuid::new_v4();
        let request_id = Uuid::new_v4();
        let template_id = Uuid::new_v4();
        for method in ["session", "api_token"] {
            let mut context = RequestContext::new(
                Some(user_id),
                method.into(),
                None,
                request_id.to_string(),
                Default::default(),
            );
            if method == "api_token" {
                context.api_token_id = Some(Uuid::new_v4().to_string());
            }
            let event = super::denied_launch_audit(
                &context,
                template_id,
                &AppError::Forbidden("private policy diagnostic".into()),
            )
            .unwrap()
            .unwrap();
            let audit = event.audit_context();
            assert_eq!(event.actor_id, Some(user_id));
            assert_eq!(event.actor_kind, context.audit_actor_kind());
            assert_eq!(event.target_id, Some(template_id));
            assert_eq!(audit.outcome, AuditOutcome::Denied);
            assert_eq!(audit.decision, Some(AuditDecision::Denied));
            assert_eq!(audit.emitter.unwrap().kind, AuditEmitterKind::Api);
            assert_eq!(audit.correlation.request_id, Some(request_id));
            assert_eq!(audit.correlation.run_group_id, None);
            assert_eq!(audit.correlation.causation_id, None);
            assert!(audit.state.is_none());
            assert!(audit.delegated_principal.is_none());
            assert!(
                !event
                    .metadata
                    .to_string()
                    .contains("private policy diagnostic")
            );
            for error in [
                AppError::Validation("invalid input".into()),
                AppError::Database("offline".into()),
                AppError::Config("missing authorizer".into()),
                AppError::NotFound("template".into()),
                AppError::Conflict("retry".into()),
                AppError::Auth("unauthenticated".into()),
            ] {
                assert!(
                    super::denied_launch_audit(&context, template_id, &error)
                        .unwrap()
                        .is_none()
                );
            }
        }
    }

    #[test]
    fn automation_audit_distinguishes_queued_and_terminal_outcomes() {
        use crate::contexts::audit::domain::{
            AuditAutomationState, AuditLifecycleState, AuditOutcome,
        };
        let group = Uuid::new_v4();
        let request = Uuid::new_v4();
        let token = Uuid::new_v4();
        let queued = super::automation_audit_context(
            "playbook.run_group.created",
            "playbook_run_group",
            Some(group),
            &json!({}),
            Some(request),
            None,
        );
        assert_eq!(queued.outcome, AuditOutcome::Unknown);
        assert_eq!(
            queued.state,
            Some(AuditLifecycleState::Automation(
                AuditAutomationState::Queued
            ))
        );
        assert_eq!(queued.correlation.request_id, Some(request));
        assert_eq!(queued.correlation.run_group_id, Some(group));
        let failed = super::automation_audit_context(
            "playbook.run_group.completed",
            "playbook_run_group",
            Some(group),
            &json!({"status":"failed", "password":"never store"}),
            None,
            Some(token),
        );
        assert_eq!(failed.outcome, AuditOutcome::Failed);
        assert_eq!(
            failed.state,
            Some(AuditLifecycleState::Automation(
                AuditAutomationState::Failed
            ))
        );
        assert_eq!(failed.delegated_principal.unwrap().id, token);
        let cancel_request = super::automation_audit_context(
            "playbook.run_group.cancel_requested",
            "playbook_run_group",
            Some(group),
            &json!({"status": "running"}),
            Some(request),
            None,
        );
        assert_eq!(cancel_request.outcome, AuditOutcome::Succeeded);
        assert_eq!(
            cancel_request.state,
            Some(AuditLifecycleState::Automation(
                AuditAutomationState::Running
            ))
        );
        assert!(
            super::project_metadata(
                "playbook.run_group.completed",
                &json!({"password":"never store"})
            )
            .get("password")
            .is_none()
        );
    }

    #[test]
    fn manual_launch_selection_rejects_retired_overrides_and_invalid_ids() {
        let target_id = Uuid::new_v4();
        let request: super::LaunchJobTemplateRequest =
            serde_json::from_value(json!({"target_ids": [target_id]})).unwrap();
        assert_eq!(request.target_ids, Some(vec![target_id]));
        let normal: super::LaunchJobTemplateRequest = serde_json::from_value(json!({})).unwrap();
        assert!(normal.target_ids.is_none());
        for invalid in [
            json!({"target_ids": ["host.example"]}),
            json!({"target_set_id": target_id}),
            json!({"target_limit": 1}),
            json!({"target_ids": [target_id], "target_set_id": target_id}),
        ] {
            assert!(serde_json::from_value::<super::LaunchJobTemplateRequest>(invalid).is_err());
        }
    }

    #[test]
    fn event_batch_rejects_empty_collection_with_a_400() {
        use axum::http::StatusCode;

        use crate::api::rest::error_mapping::map_app_error_to_http;

        let events = serde_json::from_value::<CreateRunGroupEventsBatchRequest>(
            json!({"batch_id": Uuid::nil(), "events": []}),
        )
        .expect("deserialize events");
        assert_eq!(
            map_app_error_to_http(events.validate().unwrap_err()).status(),
            StatusCode::BAD_REQUEST
        );
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
            parse_csv_run_statuses(Some(" running, TIMED_OUT, running ")).ok(),
            Some(vec![
                PlaybookRunStatus::Running,
                PlaybookRunStatus::TimedOut,
            ])
        );
        assert_eq!(
            parse_csv_run_statuses(Some(" UNREPORTED ")).ok(),
            Some(vec![PlaybookRunStatus::Unreported])
        );
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
            PlaybookRunStatus::Unreported,
        ] {
            assert!(ensure_single_run_cancel_allowed(50_000, status).is_ok());
        }
    }

    #[test]
    fn completion_accepts_no_host_failure_but_rejects_nonterminal_status() {
        let mut body = json!({
            "dispatch_id": Uuid::new_v4(), "dispatch_attempt": 1,
            "lease_owner": "worker-1", "console_next_sequence": 0,
            "group_status": "failed", "exit_code": 1,
            "error_message": "failed before hosts", "runs": []
        });
        let completion: CompleteRunGroupBatchRequest =
            serde_json::from_value(body.clone()).unwrap();
        assert!(completion.validate().is_ok());
        assert_eq!(completion.group_status, PlaybookRunStatus::Failed);
        body["group_status"] = json!("running");
        let nonterminal: CompleteRunGroupBatchRequest =
            serde_json::from_value(body.clone()).unwrap();
        assert!(nonterminal.validate().is_err());
        // A succeeded process may report hosts whose structured result never
        // arrived; an unknown status string still fails at the boundary.
        body["group_status"] = json!("succeeded");
        body["runs"] = json!([{
            "run_id": Uuid::new_v4(), "status": "unreported",
            "exit_code": null, "error_message": "no structured result"
        }]);
        let unreported: CompleteRunGroupBatchRequest =
            serde_json::from_value(body.clone()).unwrap();
        assert!(unreported.validate().is_ok());
        assert_eq!(unreported.group_status, PlaybookRunStatus::Succeeded);
        assert_eq!(unreported.runs[0].status, PlaybookRunStatus::Unreported);
        body["runs"][0]["status"] = json!("exploded");
        assert!(serde_json::from_value::<CompleteRunGroupBatchRequest>(body.clone()).is_err());
        body["runs"][0]["status"] = json!("unreported");
        body["group_status"] = json!("unreported");
        let unreported_group: CompleteRunGroupBatchRequest =
            serde_json::from_value(body.clone()).unwrap();
        assert!(
            unreported_group.validate().is_err(),
            "a group process verdict cannot be unreported"
        );
        body["group_status"] = json!("exploded");
        assert!(serde_json::from_value::<CompleteRunGroupBatchRequest>(body).is_err());
    }
}
