use crate::contexts::playbooks::domain::{
    JobTemplate, JobTokenAudience, LaunchSpec, ParameterOverrides, PlaybookAssignment,
    PlaybookAssignmentScopeKind, PlaybookCancelOutcome, PlaybookJobToken, PlaybookRun,
    PlaybookRunEvent, PlaybookRunEventKind, PlaybookRunGroup, PlaybookRunStatus,
    PlaybookRunTriggerKind, PlaybookSchedule, PlaybookScheduleConcurrencyPolicy,
    PlaybookScheduleMisfirePolicy, PlaybookTargetPreviewItem, PlaybookTargetSetMode,
    ScheduleTargets,
};
use serde_json::Value;
use uuid::Uuid;

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
    pub source: crate::contexts::playbooks::domain::ProjectSource,
    pub user_id: Option<Uuid>,
}

#[derive(Debug, Clone)]
pub struct CreateJobTemplateRecord {
    pub template: JobTemplate,
    pub user_id: Option<Uuid>,
}

/// One builtin job definition written by the startup catalog loader.
#[derive(Debug, Clone, PartialEq)]
pub struct SeedSystemTemplateRecord {
    pub id: Uuid,
    pub definition: crate::contexts::playbooks::domain::CatalogTemplateDefinition,
    /// Seed value for a new row. An existing row keeps its own instance-owned
    /// environment, so a re-seed may carry `None` when no default is configured.
    pub execution_environment_id: Option<Uuid>,
}

/// One atomic builtin catalog write: the single system project, one immutable
/// revision per catalog tree and the job definitions of its system entries.
#[derive(Debug, Clone)]
pub struct SeedSystemCatalogRecord {
    /// The catalog source key carried by `ProjectSource::System`.
    pub source_key: String,
    pub project_id: Uuid,
    pub project_name: String,
    pub project_description: Option<String>,
    pub revision_id: Uuid,
    pub tree_sha256: String,
    pub entries: Vec<crate::contexts::playbooks::domain::ProjectEntry>,
    pub templates: Vec<SeedSystemTemplateRecord>,
}

/// What one catalog seed changed. A repeated seed with unchanged inputs reports
/// zero created and zero updated rows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SeededSystemCatalog {
    pub project_created: bool,
    pub revision_created: bool,
    pub templates_created: u32,
    pub templates_updated: u32,
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
    /// `None` preserves the stored static members. `Some` replaces them
    /// atomically, which keeps a mode switch from silently dropping the list.
    pub members: Option<Vec<StaticTargetMemberRecord>>,
    pub user_id: Option<Uuid>,
}

#[derive(Debug, Clone)]
pub struct CreateAssignmentRecord {
    pub job_template_id: Uuid,
    pub target_set_id: Uuid,
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
    pub job_template_id: Option<Uuid>,
    pub target_set_id: Option<Uuid>,
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
    pub template_id: Uuid,
    pub targets: ScheduleTargets,
    pub overrides: ParameterOverrides,
    pub owner_user_id: Uuid,
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
    pub targets: Option<ScheduleTargets>,
    pub overrides: Option<ParameterOverrides>,
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
pub struct RunGroupWithRuns {
    pub run_group: PlaybookRunGroup,
    pub runs: Vec<PlaybookRun>,
}

#[derive(Debug, Clone)]
pub struct TargetAssignmentRecord {
    pub assignment: PlaybookAssignment,
    pub template: JobTemplate,
}

#[derive(Debug, Clone)]
pub struct TargetAssignmentsRecord {
    pub target: PlaybookTargetPreviewItem,
    pub items: Vec<TargetAssignmentRecord>,
}

#[derive(Debug, Clone)]
pub struct CreateRunGroupRecord {
    pub launch_spec: LaunchSpec,
    pub schedule_id: Option<Uuid>,
    pub scheduled_for: Option<chrono::DateTime<chrono::Utc>>,
    pub trigger_kind: PlaybookRunTriggerKind,
    pub requested_by_user_id: Option<Uuid>,
    pub service_principal_id: Option<Uuid>,
    pub metadata: Value,
    /// Server-authored C3 secret-use authorization snapshot (env → {secret_id,
    /// key}, no values). The repository writes this as the ONLY author of the
    /// `secret_env_authorization` key in `run_group.metadata`, stripping any
    /// caller-supplied value first so it cannot be forged through launch metadata.
    pub secret_env_authorization: Option<Value>,
    /// SSH record contexts authorized against the source principal's original
    /// request ceiling. This is never supplied by run-group metadata.
    pub ssh_record_contexts: Vec<Uuid>,
}

#[derive(Debug, Clone)]
pub struct CreatedRunGroupRecord {
    pub run_group: PlaybookRunGroup,
    pub runs: Vec<PlaybookRun>,
}

#[derive(Debug, Clone)]
pub struct CreateJobTokenRecord {
    pub id: Uuid,
    pub run_id: Uuid,
    pub token_hash: String,
    pub token_prefix: String,
    pub scopes: Value,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    /// The surface this token is minted for. Persisted into `metadata.audience`
    /// by the repository, so it can never be omitted at a creation site.
    pub audience: JobTokenAudience,
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
pub struct StartLeasedRunsRecord {
    pub run_group_id: Uuid,
    pub run_ids: Vec<Uuid>,
    pub lease_owner: String,
    pub dispatch_id: Uuid,
    pub dispatch_attempt: i32,
    pub now: chrono::DateTime<chrono::Utc>,
    pub job_tokens: Vec<CreateJobTokenRecord>,
    pub sandbox_job_tokens: Vec<CreateJobTokenRecord>,
    pub group_callback_token: CreateRunGroupCallbackTokenRecord,
}

#[derive(Debug, Clone)]
pub struct StartedRunRecord {
    pub run: PlaybookRun,
    pub job_token: PlaybookJobToken,
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
    /// Authoritative terminal group status. Zero-host launches have no run rows
    /// to roll up, so the caller's group verdict is the only terminal signal.
    pub group_status: PlaybookRunStatus,
    pub exit_code: Option<i32>,
    pub error_message: Option<String>,
    /// Supervisor receipt. Present on the callback path, which requires it to
    /// match the durable dispatch identity and seal the console prefix.
    pub dispatch_id: Option<Uuid>,
    pub dispatch_attempt: Option<i32>,
    /// Final durable console chunk count. Present on the callback path; zero is
    /// valid and seals an empty stream.
    pub console_next_sequence: Option<u64>,
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
