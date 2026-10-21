use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::errors::AppError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlaybookStatus {
    Draft,
    Active,
    Archived,
}

impl PlaybookStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Active => "active",
            Self::Archived => "archived",
        }
    }
}

impl fmt::Display for PlaybookStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlaybookLanguage {
    Yaml,
}

impl PlaybookLanguage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Yaml => "yaml",
        }
    }
}

impl fmt::Display for PlaybookLanguage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlaybookRuntime {
    Ansible,
}

impl PlaybookRuntime {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ansible => "ansible",
        }
    }
}

impl fmt::Display for PlaybookRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Per-host transport the Ansible runner uses to reach a target. Orthogonal to
/// `PlaybookRuntime` (always `ansible`): a single playbook can target SSH (Linux)
/// or WinRM (Windows) hosts. `connection_default` on a playbook picks the default
/// transport; the inventory builder maps it to `ansible_connection`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlaybookConnection {
    #[default]
    Ssh,
    Winrm,
    Local,
}

impl PlaybookConnection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ssh => "ssh",
            Self::Winrm => "winrm",
            Self::Local => "local",
        }
    }
}

impl fmt::Display for PlaybookConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlaybookRisk {
    Low,
    Medium,
    High,
    Critical,
}

impl PlaybookRisk {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Critical => "critical",
        }
    }
}

impl fmt::Display for PlaybookRisk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Playbook {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub description: Option<String>,
    pub status: PlaybookStatus,
    pub runtime: PlaybookRuntime,
    pub risk: PlaybookRisk,
    pub owner_user_id: Option<Uuid>,
    pub category: String,
    pub current_version_id: Option<Uuid>,
    pub default_target_set_id: Option<Uuid>,
    /// Provenance of the playbook content. `in_app` playbooks are editable
    /// through the API; non-`in_app` playbooks (`git` mirrors, `official`
    /// catalog entries and built-in `system` playbooks) are read-only and
    /// reject mutating operations at the service boundary.
    pub source_kind: String,
    /// Typed launch-form schema (JSON Schema object) for the playbook's
    /// extravars. `None` means the playbook declares no typed variables.
    pub vars_schema: Option<Value>,
    /// Default per-host transport (ssh/winrm) for runs of this playbook.
    pub connection_default: PlaybookConnection,
    /// Capability flag: when `true` (and `source_kind == 'system'`), the run
    /// worker exposes a live-run-scoped job token and base URL to the visible
    /// playbook process so it can call normal inventory REST endpoints.
    /// Duplicate / in_app copies always inherit `false` — never copy this
    /// capability.
    pub inventory_rest_enabled: bool,
    pub created_by_user_id: Option<Uuid>,
    pub updated_by_user_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PlaybookVersion {
    pub id: Uuid,
    pub playbook_id: Uuid,
    pub version: i32,
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
    pub created_by_user_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PlaybookExecutionEnvironment {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub image: String,
    pub pull_policy: String,
    pub runner_group: String,
    pub enabled: bool,
    pub env: Value,
    pub settings: Value,
    pub metadata: Value,
    pub created_by_user_id: Option<Uuid>,
    pub updated_by_user_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PlaybookProject {
    pub id: Uuid,
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
    pub sync_status: String,
    pub last_sync_revision: Option<String>,
    pub last_sync_error: Option<String>,
    pub last_synced_at: Option<DateTime<Utc>>,
    pub metadata: Value,
    pub created_by_user_id: Option<Uuid>,
    pub updated_by_user_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PlaybookJobTemplate {
    pub id: Uuid,
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
    pub created_by_user_id: Option<Uuid>,
    pub updated_by_user_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlaybookTargetSetMode {
    Dynamic,
    Static,
}

impl PlaybookTargetSetMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dynamic => "dynamic",
            Self::Static => "static",
        }
    }
}

impl fmt::Display for PlaybookTargetSetMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PlaybookTargetSet {
    pub id: Uuid,
    pub name: String,
    pub mode: PlaybookTargetSetMode,
    pub entity_kind: String,
    pub selector: Value,
    pub created_by_user_id: Option<Uuid>,
    pub updated_by_user_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlaybookAssignmentScopeKind {
    Global,
    ConnectionTarget,
    Role,
    Site,
    Region,
    Tenant,
    Cluster,
    Platform,
}

impl PlaybookAssignmentScopeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::ConnectionTarget => "connection_target",
            Self::Role => "role",
            Self::Site => "site",
            Self::Region => "region",
            Self::Tenant => "tenant",
            Self::Cluster => "cluster",
            Self::Platform => "platform",
        }
    }
}

impl fmt::Display for PlaybookAssignmentScopeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PlaybookAssignment {
    pub id: Uuid,
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
    pub created_by_user_id: Option<Uuid>,
    pub updated_by_user_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub metadata: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlaybookServicePrincipalStatus {
    Active,
    Disabled,
    Revoked,
}

impl PlaybookServicePrincipalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Disabled => "disabled",
            Self::Revoked => "revoked",
        }
    }
}

impl fmt::Display for PlaybookServicePrincipalStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PlaybookServicePrincipal {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
    pub status: PlaybookServicePrincipalStatus,
    pub created_by_user_id: Option<Uuid>,
    pub updated_by_user_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub revoked_by_user_id: Option<Uuid>,
    pub revocation_reason: Option<String>,
    pub metadata: Value,
}

/// Misfire policy for scheduled playbook runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlaybookScheduleMisfirePolicy {
    FireOnce,
}

impl PlaybookScheduleMisfirePolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FireOnce => "fire_once",
        }
    }
}

impl fmt::Display for PlaybookScheduleMisfirePolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlaybookScheduleConcurrencyPolicy {
    SkipIfRunning,
    QueuePending,
    ReplacePending,
}

impl PlaybookScheduleConcurrencyPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SkipIfRunning => "skip_if_running",
            Self::QueuePending => "queue_pending",
            Self::ReplacePending => "replace_pending",
        }
    }
}

impl fmt::Display for PlaybookScheduleConcurrencyPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PlaybookSchedule {
    pub id: Uuid,
    pub playbook_id: Uuid,
    pub target_set_id: Uuid,
    pub service_principal_id: Uuid,
    pub name: String,
    pub cron_expression: String,
    pub timezone: String,
    pub misfire_policy: PlaybookScheduleMisfirePolicy,
    pub concurrency_policy: PlaybookScheduleConcurrencyPolicy,
    pub enabled: bool,
    pub next_run_at: Option<DateTime<Utc>>,
    pub last_run_at: Option<DateTime<Utc>>,
    pub created_by_user_id: Option<Uuid>,
    pub updated_by_user_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PlaybookTargetPreviewItem {
    pub target_kind: String,
    pub target_id: Uuid,
    pub source_kind: String,
    pub source_id: Uuid,
    pub name: String,
    pub address: Option<String>,
    pub address_bound: bool,
    pub status: Option<String>,
    pub site_id: Option<Uuid>,
    pub site_name: Option<String>,
    pub region_id: Option<Uuid>,
    pub region_name: Option<String>,
    pub tenant_id: Option<Uuid>,
    pub tenant_name: Option<String>,
    pub role_id: Option<Uuid>,
    pub role_name: Option<String>,
    pub platform_id: Option<Uuid>,
    pub platform_name: Option<String>,
    pub device_type_id: Option<Uuid>,
    pub device_type_name: Option<String>,
    pub cluster_id: Option<Uuid>,
    pub cluster_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PlaybookTargetSetPreview {
    pub target_set: PlaybookTargetSet,
    pub items: Vec<PlaybookTargetPreviewItem>,
    pub total: i64,
    pub limit: i64,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlaybookRunStatus {
    Queued,
    Leased,
    Dispatching,
    Running,
    Succeeded,
    Failed,
    Canceled,
    TimedOut,
}

/// How an operator cancellation reached a terminal state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlaybookCancelOutcome {
    /// The group was still queued or leased and had never been dispatched.
    NotDispatched,
    /// The runner receipt proved the dispatched process had stopped.
    RunnerConfirmed,
    /// The cancellation deadline elapsed without runner confirmation.
    DeadlineExpired,
}

impl PlaybookCancelOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotDispatched => "not_dispatched",
            Self::RunnerConfirmed => "runner_confirmed",
            Self::DeadlineExpired => "deadline_expired",
        }
    }
}

impl PlaybookRunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Leased => "leased",
            Self::Dispatching => "dispatching",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
            Self::TimedOut => "timed_out",
        }
    }

    /// True while the run may be executing in the sidecar. Queued runs are not
    /// yet dispatched and terminal runs are
    /// already done, so neither is "active" — but both are still cancellable
    /// (see [`Self::is_terminal`] for the inverse used by the cancel/idempotency
    /// paths).
    pub fn is_active(self) -> bool {
        matches!(self, Self::Leased | Self::Dispatching | Self::Running)
    }

    /// True once the run has reached a final state and can no longer transition.
    /// Dispatched cancellation reaches this set only after runner confirmation
    /// or deadline expiry; completion callbacks remain idempotent afterward.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Canceled | Self::TimedOut
        )
    }
}

impl fmt::Display for PlaybookRunStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlaybookRunTriggerKind {
    Manual,
    Schedule,
    Api,
    Mcp,
    Bulk,
}

impl PlaybookRunTriggerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Schedule => "schedule",
            Self::Api => "api",
            Self::Mcp => "mcp",
            Self::Bulk => "bulk",
        }
    }
}

impl fmt::Display for PlaybookRunTriggerKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlaybookRunEventKind {
    Status,
    Stdout,
    Stderr,
    System,
    Result,
}

impl PlaybookRunEventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
            Self::System => "system",
            Self::Result => "result",
        }
    }
}

impl fmt::Display for PlaybookRunEventKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PlaybookRunGroup {
    pub id: Uuid,
    pub playbook_id: Uuid,
    pub playbook_version_id: Uuid,
    pub target_set_id: Option<Uuid>,
    pub schedule_id: Option<Uuid>,
    pub scheduled_for: Option<DateTime<Utc>>,
    pub requested_by_user_id: Option<Uuid>,
    pub service_principal_id: Option<Uuid>,
    pub trigger_kind: PlaybookRunTriggerKind,
    pub status: PlaybookRunStatus,
    pub total_runs: i32,
    pub succeeded_runs: i32,
    pub failed_runs: i32,
    pub canceled_runs: i32,
    pub timeout_runs: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub lease_owner: Option<String>,
    pub lease_expires_at: Option<DateTime<Utc>>,
    pub leased_at: Option<DateTime<Utc>>,
    pub dispatch_id: Option<Uuid>,
    pub dispatch_attempt: i32,
    pub cancel_requested_at: Option<DateTime<Utc>>,
    pub cancel_deadline_at: Option<DateTime<Utc>>,
    pub cancel_outcome: Option<PlaybookCancelOutcome>,
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PlaybookRun {
    pub id: Uuid,
    pub run_group_id: Uuid,
    pub playbook_id: Uuid,
    pub playbook_version_id: Uuid,
    pub target_kind: String,
    pub target_id: Uuid,
    pub source_kind: String,
    pub source_id: Uuid,
    pub target_name: Option<String>,
    pub target_address: Option<String>,
    pub target_address_bound: bool,
    pub credential_account_kind: String,
    pub credential_account_id: Option<Uuid>,
    pub credential_account_entry_id: Option<String>,
    pub credential_merge_policy: String,
    pub credential_resolution: Value,
    pub status: PlaybookRunStatus,
    pub cancel_requested_at: Option<DateTime<Utc>>,
    pub cancel_outcome: Option<PlaybookCancelOutcome>,
    pub lease_owner: Option<String>,
    pub lease_expires_at: Option<DateTime<Utc>>,
    pub queued_at: DateTime<Utc>,
    pub leased_at: Option<DateTime<Utc>>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub exit_code: Option<i32>,
    pub error_message: Option<String>,
    pub attempt: i32,
    pub timeout_seconds: i32,
    pub metadata: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlaybookJobTokenStatus {
    Active,
    Used,
    Revoked,
    Expired,
}

impl PlaybookJobTokenStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Used => "used",
            Self::Revoked => "revoked",
            Self::Expired => "expired",
        }
    }
}

impl fmt::Display for PlaybookJobTokenStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PlaybookJobToken {
    pub id: Uuid,
    pub run_id: Uuid,
    pub target_kind: String,
    pub target_id: Uuid,
    pub source_kind: String,
    pub source_id: Uuid,
    pub token_prefix: String,
    pub status: PlaybookJobTokenStatus,
    pub scopes: Value,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PlaybookRunEvent {
    pub id: Uuid,
    pub run_group_id: Uuid,
    pub run_id: Option<Uuid>,
    pub host: Option<String>,
    pub event_sequence: i64,
    pub event_kind: PlaybookRunEventKind,
    pub event_type: String,
    pub message: Option<String>,
    pub payload: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PlaybookArtifact {
    pub id: Uuid,
    pub run_id: Uuid,
    pub artifact_type: String,
    pub name: String,
    pub content_type: String,
    pub content_json: Option<Value>,
    pub content_text: Option<String>,
    pub content_sha256: String,
    pub content_size_bytes: i64,
    pub created_at: DateTime<Utc>,
}

pub fn parse_playbook_status(value: &str) -> Result<PlaybookStatus, AppError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "draft" => Ok(PlaybookStatus::Draft),
        "active" => Ok(PlaybookStatus::Active),
        "archived" => Ok(PlaybookStatus::Archived),
        other => Err(AppError::Database(format!(
            "Unknown playbook status: {other}"
        ))),
    }
}

pub fn parse_playbook_language(value: &str) -> Result<PlaybookLanguage, AppError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "yaml" => Ok(PlaybookLanguage::Yaml),
        other => Err(AppError::Database(format!(
            "Unknown playbook language: {other}"
        ))),
    }
}

pub fn parse_playbook_runtime(value: &str) -> Result<PlaybookRuntime, AppError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "ansible" => Ok(PlaybookRuntime::Ansible),
        other => Err(AppError::Database(format!(
            "Unknown playbook runtime: {other}"
        ))),
    }
}

pub fn parse_playbook_connection(value: &str) -> Result<PlaybookConnection, AppError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "ssh" => Ok(PlaybookConnection::Ssh),
        "winrm" => Ok(PlaybookConnection::Winrm),
        "local" => Ok(PlaybookConnection::Local),
        other => Err(AppError::Database(format!(
            "Unknown playbook connection: {other}"
        ))),
    }
}

pub fn parse_playbook_risk(value: &str) -> Result<PlaybookRisk, AppError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "low" => Ok(PlaybookRisk::Low),
        "medium" => Ok(PlaybookRisk::Medium),
        "high" => Ok(PlaybookRisk::High),
        "critical" => Ok(PlaybookRisk::Critical),
        other => Err(AppError::Database(format!(
            "Unknown playbook risk: {other}"
        ))),
    }
}

pub fn parse_target_set_mode(value: &str) -> Result<PlaybookTargetSetMode, AppError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "dynamic" => Ok(PlaybookTargetSetMode::Dynamic),
        "static" => Ok(PlaybookTargetSetMode::Static),
        other => Err(AppError::Database(format!(
            "Unknown playbook target set mode: {other}"
        ))),
    }
}

pub fn parse_assignment_scope_kind(value: &str) -> Result<PlaybookAssignmentScopeKind, AppError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "global" => Ok(PlaybookAssignmentScopeKind::Global),
        "connection_target" => Ok(PlaybookAssignmentScopeKind::ConnectionTarget),
        "role" => Ok(PlaybookAssignmentScopeKind::Role),
        "site" => Ok(PlaybookAssignmentScopeKind::Site),
        "region" => Ok(PlaybookAssignmentScopeKind::Region),
        "tenant" => Ok(PlaybookAssignmentScopeKind::Tenant),
        "cluster" => Ok(PlaybookAssignmentScopeKind::Cluster),
        "platform" => Ok(PlaybookAssignmentScopeKind::Platform),
        other => Err(AppError::Database(format!(
            "Unknown playbook assignment scope kind: {other}"
        ))),
    }
}

pub fn parse_service_principal_status(
    value: &str,
) -> Result<PlaybookServicePrincipalStatus, AppError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "active" => Ok(PlaybookServicePrincipalStatus::Active),
        "disabled" => Ok(PlaybookServicePrincipalStatus::Disabled),
        "revoked" => Ok(PlaybookServicePrincipalStatus::Revoked),
        other => Err(AppError::Database(format!(
            "Unknown playbook service principal status: {other}"
        ))),
    }
}

pub fn parse_schedule_misfire_policy(
    value: &str,
) -> Result<PlaybookScheduleMisfirePolicy, AppError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "fire_once" => Ok(PlaybookScheduleMisfirePolicy::FireOnce),
        // `skip_missed` / `catch_up` are not implemented for the first release;
        // mapping them onto `fire_once` would silently mislead callers, so they
        // are rejected loudly at the boundary instead of advertising fake support.
        "skip_missed" | "catch_up" => Err(AppError::Validation(format!(
            "playbook schedule misfire policy `{}` is not supported; only `fire_once` is available",
            value.trim()
        ))),
        other => Err(AppError::Validation(format!(
            "Unknown playbook schedule misfire policy: {other}"
        ))),
    }
}

pub fn parse_schedule_concurrency_policy(
    value: &str,
) -> Result<PlaybookScheduleConcurrencyPolicy, AppError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "skip_if_running" => Ok(PlaybookScheduleConcurrencyPolicy::SkipIfRunning),
        "queue_pending" => Ok(PlaybookScheduleConcurrencyPolicy::QueuePending),
        "replace_pending" => Ok(PlaybookScheduleConcurrencyPolicy::ReplacePending),
        other => Err(AppError::Database(format!(
            "Unknown playbook schedule concurrency policy: {other}"
        ))),
    }
}

pub fn parse_run_status(value: &str) -> Result<PlaybookRunStatus, AppError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "queued" => Ok(PlaybookRunStatus::Queued),
        "leased" => Ok(PlaybookRunStatus::Leased),
        "dispatching" => Ok(PlaybookRunStatus::Dispatching),
        "running" => Ok(PlaybookRunStatus::Running),
        "succeeded" => Ok(PlaybookRunStatus::Succeeded),
        "failed" => Ok(PlaybookRunStatus::Failed),
        "canceled" => Ok(PlaybookRunStatus::Canceled),
        "timed_out" => Ok(PlaybookRunStatus::TimedOut),
        other => Err(AppError::Database(format!(
            "Unknown playbook run status: {other}"
        ))),
    }
}

pub fn parse_cancel_outcome(value: &str) -> Result<PlaybookCancelOutcome, AppError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "not_dispatched" => Ok(PlaybookCancelOutcome::NotDispatched),
        "runner_confirmed" => Ok(PlaybookCancelOutcome::RunnerConfirmed),
        "deadline_expired" => Ok(PlaybookCancelOutcome::DeadlineExpired),
        other => Err(AppError::Database(format!(
            "Unknown playbook cancel outcome: {other}"
        ))),
    }
}

pub fn parse_run_trigger_kind(value: &str) -> Result<PlaybookRunTriggerKind, AppError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "manual" => Ok(PlaybookRunTriggerKind::Manual),
        "schedule" => Ok(PlaybookRunTriggerKind::Schedule),
        "api" => Ok(PlaybookRunTriggerKind::Api),
        "mcp" => Ok(PlaybookRunTriggerKind::Mcp),
        "bulk" => Ok(PlaybookRunTriggerKind::Bulk),
        other => Err(AppError::Database(format!(
            "Unknown playbook run trigger kind: {other}"
        ))),
    }
}

pub fn parse_run_event_kind(value: &str) -> Result<PlaybookRunEventKind, AppError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "status" => Ok(PlaybookRunEventKind::Status),
        "stdout" => Ok(PlaybookRunEventKind::Stdout),
        "stderr" => Ok(PlaybookRunEventKind::Stderr),
        "system" => Ok(PlaybookRunEventKind::System),
        "result" => Ok(PlaybookRunEventKind::Result),
        other => Err(AppError::Database(format!(
            "Unknown playbook run event kind: {other}"
        ))),
    }
}

pub fn parse_job_token_status(value: &str) -> Result<PlaybookJobTokenStatus, AppError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "active" => Ok(PlaybookJobTokenStatus::Active),
        "used" => Ok(PlaybookJobTokenStatus::Used),
        "revoked" => Ok(PlaybookJobTokenStatus::Revoked),
        "expired" => Ok(PlaybookJobTokenStatus::Expired),
        other => Err(AppError::Database(format!(
            "Unknown playbook job token status: {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PlaybookAssignmentScopeKind, PlaybookConnection, PlaybookJobTokenStatus, PlaybookLanguage,
        PlaybookRisk, PlaybookRunEventKind, PlaybookRunStatus, PlaybookRunTriggerKind,
        PlaybookRuntime, PlaybookScheduleConcurrencyPolicy, PlaybookScheduleMisfirePolicy,
        PlaybookServicePrincipalStatus, PlaybookStatus, PlaybookTargetSetMode,
        parse_assignment_scope_kind, parse_job_token_status, parse_playbook_connection,
        parse_playbook_language, parse_playbook_risk, parse_playbook_runtime,
        parse_playbook_status, parse_run_event_kind, parse_run_status, parse_run_trigger_kind,
        parse_schedule_concurrency_policy, parse_schedule_misfire_policy,
        parse_service_principal_status, parse_target_set_mode,
    };

    #[test]
    fn parse_playbook_status_accepts_canonical_values() {
        assert_eq!(
            parse_playbook_status("draft").unwrap(),
            PlaybookStatus::Draft
        );
        assert_eq!(
            parse_playbook_status("ACTIVE").unwrap(),
            PlaybookStatus::Active
        );
        assert!(parse_playbook_status("running").is_err());
    }

    #[test]
    fn playbook_language_accepts_yaml_only() {
        assert_eq!(
            parse_playbook_language("yaml").unwrap(),
            PlaybookLanguage::Yaml
        );
        assert_eq!(
            parse_playbook_language("YAML").unwrap(),
            PlaybookLanguage::Yaml
        );
        assert!(parse_playbook_language("shell").is_err());
        assert!(parse_playbook_language("powershell").is_err());
    }

    #[test]
    fn playbook_runtime_accepts_ansible_only() {
        assert_eq!(
            parse_playbook_runtime("ansible").unwrap(),
            PlaybookRuntime::Ansible
        );
        assert_eq!(
            parse_playbook_runtime("ANSIBLE").unwrap(),
            PlaybookRuntime::Ansible
        );
        assert!(parse_playbook_runtime("linux_ssh").is_err());
        assert!(parse_playbook_runtime("shell").is_err());
        assert!(parse_playbook_runtime("winrm").is_err());
    }

    #[test]
    fn playbook_connection_accepts_ssh_winrm_and_local() {
        assert_eq!(
            parse_playbook_connection("ssh").unwrap(),
            PlaybookConnection::Ssh
        );
        assert_eq!(
            parse_playbook_connection("WINRM").unwrap(),
            PlaybookConnection::Winrm
        );
        assert_eq!(
            parse_playbook_connection("local").unwrap(),
            PlaybookConnection::Local
        );
        assert_eq!(PlaybookConnection::default(), PlaybookConnection::Ssh);
        assert!(parse_playbook_connection("telnet").is_err());
    }

    #[test]
    fn parse_playbook_risk_accepts_canonical_values() {
        assert_eq!(parse_playbook_risk("low").unwrap(), PlaybookRisk::Low);
        assert_eq!(parse_playbook_risk("medium").unwrap(), PlaybookRisk::Medium);
        assert_eq!(parse_playbook_risk("high").unwrap(), PlaybookRisk::High);
        assert_eq!(
            parse_playbook_risk("critical").unwrap(),
            PlaybookRisk::Critical
        );
        assert!(parse_playbook_risk("unknown").is_err());
    }

    #[test]
    fn parse_target_set_mode_accepts_explicit_modes() {
        assert_eq!(
            parse_target_set_mode("dynamic").unwrap(),
            PlaybookTargetSetMode::Dynamic
        );
        assert_eq!(
            parse_target_set_mode("STATIC").unwrap(),
            PlaybookTargetSetMode::Static
        );
        assert!(parse_target_set_mode("implicit").is_err());
    }

    #[test]
    fn parse_assignment_scope_kind_accepts_hierarchy_scopes() {
        assert_eq!(
            parse_assignment_scope_kind("global").unwrap(),
            PlaybookAssignmentScopeKind::Global
        );
        assert_eq!(
            parse_assignment_scope_kind("connection_target").unwrap(),
            PlaybookAssignmentScopeKind::ConnectionTarget
        );
        assert_eq!(
            parse_assignment_scope_kind("ROLE").unwrap(),
            PlaybookAssignmentScopeKind::Role
        );
        assert_eq!(
            parse_assignment_scope_kind("site").unwrap(),
            PlaybookAssignmentScopeKind::Site
        );
        assert_eq!(
            parse_assignment_scope_kind("region").unwrap(),
            PlaybookAssignmentScopeKind::Region
        );
        assert_eq!(
            parse_assignment_scope_kind("tenant").unwrap(),
            PlaybookAssignmentScopeKind::Tenant
        );
        assert_eq!(
            parse_assignment_scope_kind("cluster").unwrap(),
            PlaybookAssignmentScopeKind::Cluster
        );
        assert_eq!(
            parse_assignment_scope_kind("platform").unwrap(),
            PlaybookAssignmentScopeKind::Platform
        );
        assert!(parse_assignment_scope_kind("device").is_err());
        assert!(parse_assignment_scope_kind("virtual_machine").is_err());
    }

    #[test]
    fn parse_service_principal_status_accepts_lifecycle_values() {
        assert_eq!(
            parse_service_principal_status("active").unwrap(),
            PlaybookServicePrincipalStatus::Active
        );
        assert_eq!(
            parse_service_principal_status("DISABLED").unwrap(),
            PlaybookServicePrincipalStatus::Disabled
        );
        assert_eq!(
            parse_service_principal_status("revoked").unwrap(),
            PlaybookServicePrincipalStatus::Revoked
        );
        assert!(parse_service_principal_status("expired").is_err());
    }

    #[test]
    fn parse_run_status_and_trigger_kind_accept_canonical_values() {
        assert_eq!(
            parse_run_status("queued").unwrap(),
            PlaybookRunStatus::Queued
        );
        assert_eq!(
            parse_run_status("DISPATCHING").unwrap(),
            PlaybookRunStatus::Dispatching
        );
        assert!(parse_run_status("cancel_requested").is_err());
        assert_eq!(
            parse_run_trigger_kind("manual").unwrap(),
            PlaybookRunTriggerKind::Manual
        );
        assert_eq!(
            parse_run_trigger_kind("api").unwrap(),
            PlaybookRunTriggerKind::Api
        );
        assert_eq!(
            parse_run_trigger_kind("schedule").unwrap(),
            PlaybookRunTriggerKind::Schedule
        );
        assert_eq!(
            parse_run_trigger_kind("MCP").unwrap(),
            PlaybookRunTriggerKind::Mcp
        );
        assert_eq!(
            parse_run_trigger_kind("bulk").unwrap(),
            PlaybookRunTriggerKind::Bulk
        );
        assert!(parse_run_trigger_kind("webhook").is_err());
    }

    #[test]
    fn parse_schedule_policies_accept_canonical_values() {
        assert_eq!(
            parse_schedule_misfire_policy("FIRE_ONCE").unwrap(),
            PlaybookScheduleMisfirePolicy::FireOnce
        );
        // First release: only `fire_once` is implemented. The policy names that
        // would alias `fire_once` are rejected at the boundary.
        assert!(parse_schedule_misfire_policy("skip_missed").is_err());
        assert!(parse_schedule_misfire_policy("catch_up").is_err());
        assert!(parse_schedule_misfire_policy("run_all").is_err());

        assert_eq!(
            parse_schedule_concurrency_policy("skip_if_running").unwrap(),
            PlaybookScheduleConcurrencyPolicy::SkipIfRunning
        );
        assert_eq!(
            parse_schedule_concurrency_policy("QUEUE_PENDING").unwrap(),
            PlaybookScheduleConcurrencyPolicy::QueuePending
        );
        assert_eq!(
            parse_schedule_concurrency_policy("replace_pending").unwrap(),
            PlaybookScheduleConcurrencyPolicy::ReplacePending
        );
        assert!(parse_schedule_concurrency_policy("parallel").is_err());
    }

    #[test]
    fn parse_run_event_kind_accepts_canonical_values() {
        assert_eq!(
            parse_run_event_kind("status").unwrap(),
            PlaybookRunEventKind::Status
        );
        assert_eq!(
            parse_run_event_kind("STDOUT").unwrap(),
            PlaybookRunEventKind::Stdout
        );
        assert_eq!(
            parse_run_event_kind("stderr").unwrap(),
            PlaybookRunEventKind::Stderr
        );
        assert_eq!(
            parse_run_event_kind("system").unwrap(),
            PlaybookRunEventKind::System
        );
        assert_eq!(
            parse_run_event_kind("result").unwrap(),
            PlaybookRunEventKind::Result
        );
        assert!(parse_run_event_kind("debug").is_err());
    }

    #[test]
    fn parse_job_token_status_accepts_canonical_values() {
        assert_eq!(
            parse_job_token_status("active").unwrap(),
            PlaybookJobTokenStatus::Active
        );
        assert_eq!(
            parse_job_token_status("EXPIRED").unwrap(),
            PlaybookJobTokenStatus::Expired
        );
        assert!(parse_job_token_status("disabled").is_err());
    }
}
