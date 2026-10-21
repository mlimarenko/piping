use crate::contexts::playbooks::domain::{
    PlaybookAssignmentScopeKind, PlaybookCancelOutcome, PlaybookConnection, PlaybookJobToken,
    PlaybookLanguage, PlaybookRisk, PlaybookRun, PlaybookRunEvent, PlaybookRunEventKind,
    PlaybookRunGroup, PlaybookRunStatus, PlaybookRunTriggerKind, PlaybookRuntime, PlaybookSchedule,
    PlaybookScheduleConcurrencyPolicy, PlaybookScheduleMisfirePolicy, PlaybookStatus,
    PlaybookTargetSet, PlaybookTargetSetMode, PlaybookVersion,
};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct CreatePlaybookRecord {
    pub slug: String,
    pub name: String,
    pub description: Option<String>,
    pub status: PlaybookStatus,
    pub runtime: PlaybookRuntime,
    pub risk: PlaybookRisk,
    pub owner_user_id: Option<Uuid>,
    pub category: String,
    pub vars_schema: Option<Value>,
    pub connection_default: PlaybookConnection,
    pub source_kind: String,
    pub language: PlaybookLanguage,
    pub entrypoint: Option<String>,
    pub content: String,
    pub content_sha256: String,
    pub input_schema: Value,
    pub output_schema: Value,
    pub token_policy: Value,
    pub timeout_seconds: i32,
    pub changelog: Option<String>,
    pub metadata: Value,
    pub user_id: Option<Uuid>,
}

#[derive(Debug, Clone)]
pub struct CreatePlaybookVersionRecord {
    pub playbook_id: Uuid,
    pub language: PlaybookLanguage,
    pub runtime: PlaybookRuntime,
    pub entrypoint: Option<String>,
    pub content: String,
    pub content_sha256: String,
    pub input_schema: Value,
    pub output_schema: Value,
    pub token_policy: Value,
    pub timeout_seconds: i32,
    pub changelog: Option<String>,
    pub metadata: Value,
    pub user_id: Option<Uuid>,
}

#[derive(Debug, Clone)]
pub struct CreateExecutionEnvironmentRecord {
    pub name: String,
    pub description: Option<String>,
    pub image: String,
    pub pull_policy: String,
    pub runner_group: String,
    pub enabled: bool,
    pub env: Value,
    pub settings: Value,
    pub metadata: Value,
    pub user_id: Option<Uuid>,
}

#[derive(Debug, Clone)]
pub struct CreateProjectRecord {
    pub name: String,
    pub description: Option<String>,
    pub scm_type: String,
    pub scm_url: Option<String>,
    pub scm_branch: Option<String>,
    pub scm_ref: Option<String>,
    pub playbook_base_path: Option<String>,
    pub credential_id: Option<Uuid>,
    pub default_execution_environment_id: Option<Uuid>,
    pub sync_policy: String,
    pub metadata: Value,
    pub user_id: Option<Uuid>,
}

#[derive(Debug, Clone)]
pub struct CreateJobTemplateRecord {
    pub name: String,
    pub description: Option<String>,
    pub playbook_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub playbook_path: Option<String>,
    pub inventory_target_set_id: Option<Uuid>,
    pub execution_environment_id: Option<Uuid>,
    pub credential_refs: Value,
    pub extra_vars: Value,
    pub launch_options: Value,
    pub ask_variables_on_launch: bool,
    pub ask_limit_on_launch: bool,
    pub ask_tags_on_launch: bool,
    pub ask_skip_tags_on_launch: bool,
    pub ask_credential_on_launch: bool,
    pub ask_execution_environment_on_launch: bool,
    pub enabled: bool,
    pub metadata: Value,
    pub user_id: Option<Uuid>,
}

#[derive(Debug, Clone)]
pub struct UpdatePlaybookRecord {
    pub id: Uuid,
    pub slug: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub status: Option<PlaybookStatus>,
    pub runtime: Option<PlaybookRuntime>,
    pub risk: Option<PlaybookRisk>,
    pub owner_user_id: Option<Option<Uuid>>,
    pub default_target_set_id: Option<Option<Uuid>>,
    pub category: Option<String>,
    /// `Some(None)` clears the schema; `Some(Some(_))` sets it; `None` leaves it.
    pub vars_schema: Option<Option<Value>>,
    pub connection_default: Option<PlaybookConnection>,
    pub metadata: Option<Value>,
    pub user_id: Option<Uuid>,
}

#[derive(Debug, Clone)]
pub struct StaticTargetMemberRecord {
    pub target_kind: String,
    pub target_id: Uuid,
    pub source_kind: String,
    pub source_id: Uuid,
    pub sort_order: i32,
}

#[derive(Debug, Clone)]
pub struct CreateTargetSetRecord {
    pub name: String,
    pub mode: PlaybookTargetSetMode,
    pub selector: Value,
    pub members: Vec<StaticTargetMemberRecord>,
    pub user_id: Option<Uuid>,
}

#[derive(Debug, Clone)]
pub struct UpdateTargetSetRecord {
    pub id: Uuid,
    pub name: String,
    pub mode: PlaybookTargetSetMode,
    pub entity_kind: String,
    pub selector: Value,
    pub user_id: Option<Uuid>,
}

#[derive(Debug, Clone)]
pub struct CreateAssignmentRecord {
    pub playbook_id: Uuid,
    pub target_set_id: Uuid,
    pub default_version_id: Option<Uuid>,
    pub name: Option<String>,
    pub scope_kind: PlaybookAssignmentScopeKind,
    pub scope_id: Option<Uuid>,
    pub priority: i32,
    pub enabled: bool,
    pub visible: bool,
    pub executable: bool,
    pub quick_action_enabled: bool,
    pub default_input: Value,
    pub default_metadata: Value,
    pub default_timeout_seconds: Option<i32>,
    pub default_target_limit: Option<i32>,
    pub metadata: Value,
    pub user_id: Option<Uuid>,
}

#[derive(Debug, Clone)]
pub struct UpdateAssignmentRecord {
    pub id: Uuid,
    pub playbook_id: Option<Uuid>,
    pub target_set_id: Option<Uuid>,
    pub default_version_id: Option<Option<Uuid>>,
    pub name: Option<Option<String>>,
    pub scope_kind: PlaybookAssignmentScopeKind,
    pub scope_id: Option<Uuid>,
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
    pub user_id: Option<Uuid>,
}

#[derive(Debug, Clone)]
pub struct CreateScheduleRecord {
    pub playbook_id: Uuid,
    pub target_set_id: Uuid,
    pub service_principal_id: Uuid,
    pub name: String,
    pub cron_expression: String,
    pub timezone: String,
    pub misfire_policy: PlaybookScheduleMisfirePolicy,
    pub concurrency_policy: PlaybookScheduleConcurrencyPolicy,
    pub enabled: bool,
    pub next_run_at: Option<chrono::DateTime<chrono::Utc>>,
    pub metadata: Value,
    pub user_id: Option<Uuid>,
}

#[derive(Debug, Clone)]
pub struct UpdateScheduleRecord {
    pub id: Uuid,
    pub target_set_id: Option<Uuid>,
    pub name: Option<String>,
    pub cron_expression: Option<String>,
    pub timezone: Option<String>,
    pub misfire_policy: Option<PlaybookScheduleMisfirePolicy>,
    pub concurrency_policy: Option<PlaybookScheduleConcurrencyPolicy>,
    /// When `Some`, toggle the enabled flag. Enabling requires `next_run_at` to
    /// be supplied (or already present) so the `chk_playbook_schedule_enabled_due`
    /// constraint holds; disabling clears the lease triple atomically.
    pub enabled: Option<bool>,
    pub next_run_at: Option<chrono::DateTime<chrono::Utc>>,
    pub metadata: Option<Value>,
    pub user_id: Option<Uuid>,
}

#[derive(Debug, Clone)]
pub struct CreateServicePrincipalRecord {
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
    pub metadata: Value,
    pub user_id: Option<Uuid>,
}

#[derive(Debug, Clone)]
pub struct RevokeServicePrincipalRecord {
    pub id: Uuid,
    pub revocation_reason: Option<String>,
    pub user_id: Option<Uuid>,
    pub now: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone)]
pub struct RunGroupWithRuns {
    pub run_group: PlaybookRunGroup,
    pub runs: Vec<PlaybookRun>,
}

#[derive(Debug, Clone)]
pub struct CreateRunGroupRecord {
    pub playbook_id: Uuid,
    pub target_set_id: Uuid,
    pub schedule_id: Option<Uuid>,
    pub scheduled_for: Option<chrono::DateTime<chrono::Utc>>,
    pub trigger_kind: PlaybookRunTriggerKind,
    pub requested_by_user_id: Option<Uuid>,
    pub service_principal_id: Option<Uuid>,
    pub metadata: Value,
    pub target_limit: i64,
    /// Server-authored C3 secret-use authorization snapshot (env → {secret_id,
    /// key}, no values). The repository writes this as the ONLY author of the
    /// `secret_env_authorization` key in `run_group.metadata`, stripping any
    /// caller-supplied value first so it cannot be forged through launch metadata.
    pub secret_env_authorization: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct CreatedRunGroupRecord {
    pub run_group: PlaybookRunGroup,
    pub runs: Vec<PlaybookRun>,
    pub target_set: PlaybookTargetSet,
}

#[derive(Debug, Clone)]
pub struct CreateJobTokenRecord {
    pub id: Uuid,
    pub run_id: Uuid,
    pub token_hash: String,
    pub token_prefix: String,
    pub scopes: Value,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub metadata: Value,
}

#[derive(Debug, Clone)]
pub struct CreateRunGroupCallbackTokenRecord {
    pub id: Uuid,
    pub token_hash: String,
    pub token_prefix: String,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub metadata: Value,
}

#[derive(Debug, Clone)]
pub struct CallbackTokenAuth {
    pub token_id: Uuid,
    pub token_hash: String,
    pub now: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone)]
pub struct CreateArtifactRecord {
    pub run_id: Uuid,
    pub artifact_type: String,
    pub name: String,
    pub content_type: String,
    pub content_json: Option<Value>,
    pub content_text: Option<String>,
    pub content_sha256: String,
    pub content_size_bytes: i64,
}

#[derive(Debug, Clone)]
pub struct CreateRunEventRecord {
    pub run_id: Uuid,
    pub host: Option<String>,
    pub event_kind: PlaybookRunEventKind,
    pub event_type: String,
    pub message: Option<String>,
    pub payload: Value,
}

#[derive(Debug, Clone)]
pub struct CreateRunGroupEventRecord {
    pub run_group_id: Uuid,
    pub run_id: Option<Uuid>,
    pub host: Option<String>,
    pub event_kind: PlaybookRunEventKind,
    pub event_type: String,
    pub message: Option<String>,
    pub payload: Value,
}

#[derive(Debug, Clone)]
pub enum AppendRunGroupEventBatchOutcome {
    Created(Vec<PlaybookRunEvent>),
    AlreadyIngested,
}

#[derive(Debug, Clone)]
pub struct LeaseQueuedRunsRecord {
    pub lease_owner: String,
    pub limit: i64,
    pub max_concurrent_groups: i64,
    pub max_concurrent_runs_per_group: i64,
    pub now: chrono::DateTime<chrono::Utc>,
    pub lease_expires_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone)]
pub struct ScheduleOccurrenceAdvanceRecord {
    pub schedule_id: Uuid,
    pub scheduled_for: chrono::DateTime<chrono::Utc>,
    pub next_run_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone)]
pub struct LeaseScheduleOccurrencesRecord {
    pub lease_owner: String,
    pub limit: i64,
    pub lease_ttl_seconds: i64,
    pub advances: Vec<ScheduleOccurrenceAdvanceRecord>,
}

#[derive(Debug, Clone)]
pub struct DueScheduleRecord {
    pub schedule: PlaybookSchedule,
    pub database_now: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone)]
pub struct LeasedScheduleOccurrenceRecord {
    pub schedule: PlaybookSchedule,
    pub scheduled_for: chrono::DateTime<chrono::Utc>,
    pub fired_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone)]
pub struct ReleaseScheduleOccurrenceRecord {
    pub schedule_id: Uuid,
    pub lease_owner: String,
    pub scheduled_for: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone)]
pub struct StartLeasedRunRecord {
    pub run_id: Uuid,
    pub lease_owner: String,
    pub dispatch_id: Uuid,
    pub dispatch_attempt: i32,
    pub now: chrono::DateTime<chrono::Utc>,
    pub job_token: CreateJobTokenRecord,
}

#[derive(Debug, Clone)]
pub struct StartLeasedRunsRecord {
    pub run_ids: Vec<Uuid>,
    pub lease_owner: String,
    pub dispatch_id: Uuid,
    pub dispatch_attempt: i32,
    pub now: chrono::DateTime<chrono::Utc>,
    pub job_tokens: Vec<CreateJobTokenRecord>,
    pub group_callback_token: CreateRunGroupCallbackTokenRecord,
}

#[derive(Debug, Clone)]
pub struct StartedRunRecord {
    pub run: PlaybookRun,
    pub version: PlaybookVersion,
    pub job_token: PlaybookJobToken,
}

#[derive(Debug, Clone)]
pub struct MarkRunRunningRecord {
    pub run_id: Uuid,
    pub lease_owner: String,
    pub now: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone)]
pub struct MarkRunsRunningRecord {
    pub run_ids: Vec<Uuid>,
    pub lease_owner: String,
    pub now: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone)]
pub struct SetActiveRunTimeoutsRecord {
    pub run_ids: Vec<Uuid>,
    pub lease_owner: String,
    pub timeout_seconds: i32,
    pub lease_expires_at: chrono::DateTime<chrono::Utc>,
    pub now: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone)]
pub struct RequeueStartedRunsRecord {
    pub run_ids: Vec<Uuid>,
    pub lease_owner: String,
    /// The parked lease expiry; the run becomes leaseable again once this
    /// passes (the normal expired-lease reclaim in `lease_queued_runs`).
    pub retry_at: chrono::DateTime<chrono::Utc>,
    pub reason: String,
    pub now: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone)]
pub struct FinalizeRunGroupCancelRecord {
    pub run_group_id: Uuid,
    pub dispatch_id: Uuid,
    pub outcome: PlaybookCancelOutcome,
    pub now: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone)]
pub struct FinalizedRunGroupCancelRecord {
    pub run_group: PlaybookRunGroup,
    pub canceled_runs: Vec<PlaybookRun>,
}

#[derive(Debug, Clone)]
pub struct CompleteRunRecord {
    pub run_id: Uuid,
    pub lease_owner: String,
    pub status: PlaybookRunStatus,
    pub exit_code: Option<i32>,
    pub error_message: Option<String>,
    pub finished_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone)]
pub struct CompleteRunBatchItemRecord {
    pub run_id: Uuid,
    pub status: PlaybookRunStatus,
    pub exit_code: Option<i32>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CompleteRunGroupBatchRecord {
    pub run_group_id: Uuid,
    pub lease_owner: String,
    pub runs: Vec<CompleteRunBatchItemRecord>,
    /// `known_hosts` lines the runner observed for the first time during this
    /// completion. Persisted after the completion transaction commits so a
    /// rolled-back completion never pins a key.
    pub new_host_keys: Vec<String>,
    pub finished_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone)]
pub struct CompletedRunRecord {
    pub run: PlaybookRun,
    pub run_group: PlaybookRunGroup,
    /// `false` when the run was already terminal on arrival (a late/duplicate
    /// sidecar callback) and `complete_run` returned the existing state as an
    /// idempotent no-op — no event was appended, no group rollup ran, and the
    /// caller must NOT publish a completion frame or re-audit. `true` when this
    /// call performed the terminal transition.
    pub newly_completed: bool,
}

#[derive(Debug, Clone)]
pub struct CompletedRunGroupBatchRecord {
    pub run_group: PlaybookRunGroup,
    pub newly_completed_runs: Vec<PlaybookRun>,
}

#[derive(Debug, Clone)]
pub struct TimeoutStuckRunsRecord {
    pub now: chrono::DateTime<chrono::Utc>,
    pub limit: i64,
}

#[derive(Debug, Clone)]
pub struct TimedOutRunsRecord {
    pub runs: Vec<PlaybookRun>,
    pub run_groups: Vec<PlaybookRunGroup>,
}
