use std::collections::{BTreeMap, HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Duration, Utc};
use openssl::rand::rand_bytes;
use openssl::sha::sha256;
use saffron::Cron;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use url::Url;
use uuid::Uuid;

use crate::contexts::access_control::application::AccessControlService;
use crate::contexts::access_control::domain::{Action, PolicyDecision, Principal, ResourceType};
use crate::contexts::access_control::infrastructure::persistence::{
    PolicyRepository, RoleRepository, UserRepository,
};
use crate::contexts::playbooks::domain::{
    Playbook, PlaybookArtifact, PlaybookAssignment, PlaybookAssignmentScopeKind,
    PlaybookCancelOutcome, PlaybookConnection, PlaybookExecutionEnvironment, PlaybookJobTemplate,
    PlaybookJobToken, PlaybookLanguage, PlaybookProject, PlaybookRisk, PlaybookRun,
    PlaybookRunEvent, PlaybookRunEventKind, PlaybookRunGroup, PlaybookRunStatus,
    PlaybookRunTriggerKind, PlaybookRuntime, PlaybookSchedule, PlaybookScheduleConcurrencyPolicy,
    PlaybookScheduleMisfirePolicy, PlaybookServicePrincipal, PlaybookServicePrincipalStatus,
    PlaybookStatus, PlaybookTargetPreviewItem, PlaybookTargetSet, PlaybookTargetSetMode,
    PlaybookTargetSetPreview, PlaybookVersion,
};
use crate::contexts::playbooks::infrastructure::persistence::{
    AppendRunGroupEventBatchOutcome, CallbackTokenAuth, CompleteRunBatchItemRecord,
    CompleteRunGroupBatchRecord, CompleteRunRecord, CreateArtifactRecord, CreateAssignmentRecord,
    CreateExecutionEnvironmentRecord, CreateJobTemplateRecord, CreateJobTokenRecord,
    CreatePlaybookRecord, CreatePlaybookVersionRecord, CreateProjectRecord, CreateRunEventRecord,
    CreateRunGroupCallbackTokenRecord, CreateRunGroupEventRecord, CreateRunGroupRecord,
    CreateScheduleRecord, CreateServicePrincipalRecord, CreateTargetSetRecord, DueScheduleRecord,
    FinalizeRunGroupCancelRecord, FinalizedRunGroupCancelRecord, LeaseQueuedRunsRecord,
    LeaseScheduleOccurrencesRecord, LeasedScheduleOccurrenceRecord, MarkRunRunningRecord,
    MarkRunsRunningRecord, PlaybookAssignmentListFilter, PlaybookRepository, PlaybookRunListFilter,
    PlaybookScheduleListFilter, PlaybookServicePrincipalListFilter, PlaybookTargetSetListFilter,
    ReleaseScheduleOccurrenceRecord, RequeueStartedRunsRecord, RevokeServicePrincipalRecord,
    RunGroupWithRuns, ScheduleOccurrenceAdvanceRecord, SetActiveRunTimeoutsRecord,
    StartLeasedRunRecord, StartLeasedRunsRecord, StaticTargetMemberRecord, TimeoutStuckRunsRecord,
    UpdateAssignmentRecord, UpdatePlaybookRecord, UpdateScheduleRecord, UpdateTargetSetRecord,
};
use crate::contexts::playbooks::infrastructure::{
    AnsibleProjectFile, stored_project_files, validate_manual_project_files,
};
use crate::contexts::shared::pagination::PageResult;
use crate::errors::AppError;
use crate::observability::metrics;

const PLAYBOOK_NAME_MAX_CHARS: usize = 200;
const PLAYBOOK_TEMPLATE_NAME_MAX_CHARS: usize = 200;
const PLAYBOOK_DESCRIPTION_MAX_CHARS: usize = 4_000;
const PLAYBOOK_CONTENT_MAX_BYTES: usize = 1_048_576;
const PLAYBOOK_ENTRYPOINT_MAX_CHARS: usize = 240;
const PLAYBOOK_PROJECT_PATH_MAX_CHARS: usize = 240;
const PLAYBOOK_CONTAINER_IMAGE_MAX_CHARS: usize = 512;
const PLAYBOOK_RUNNER_GROUP_MAX_CHARS: usize = 120;
const PLAYBOOK_RUN_DEFAULT_TARGET_LIMIT: i64 = 50_000;
const PLAYBOOK_RUN_MAX_TARGET_LIMIT: i64 = 50_000;
const PLAYBOOK_ARTIFACT_NAME_MAX_CHARS: usize = 240;
const PLAYBOOK_ARTIFACT_TYPE_MAX_CHARS: usize = 120;
const PLAYBOOK_ARTIFACT_TEXT_MAX_BYTES: usize = 2_097_152;
const PLAYBOOK_CATEGORY_MAX_CHARS: usize = 64;
const PLAYBOOK_JOB_TOKEN_PREFIX: &str = "pjt";
const PLAYBOOK_JOB_TOKEN_SECRET_BYTES: usize = 32;
const PLAYBOOK_JOB_TOKEN_DEFAULT_TTL_SECONDS: i64 = 3_600;
const PLAYBOOK_JOB_TOKEN_MAX_TTL_SECONDS: i64 = 86_400;
const PLAYBOOK_ARTIFACT_CREATE_SCOPE: &str = "artifact:create";
const PLAYBOOK_INVENTORY_READ_SCOPE: &str = "inventory:read";
const PLAYBOOK_INVENTORY_CREATE_SCOPE: &str = "inventory:create";
const PLAYBOOK_INVENTORY_UPDATE_SCOPE: &str = "inventory:update";
const PLAYBOOK_LEASE_DEFAULT_LIMIT: i64 = 25;
const PLAYBOOK_LEASE_MAX_LIMIT: i64 = 250;
const PLAYBOOK_RUN_LEASE_MAX_LIMIT: i64 = PLAYBOOK_RUN_MAX_TARGET_LIMIT;
const PLAYBOOK_LEASE_DEFAULT_TTL_SECONDS: i64 = 300;
const PLAYBOOK_LEASE_MAX_TTL_SECONDS: i64 = 3_600;
const PLAYBOOK_LEASE_OWNER_MAX_CHARS: usize = 120;
const PLAYBOOK_RUN_ERROR_MAX_CHARS: usize = 8_000;
const PLAYBOOK_CANCEL_DEADLINE_SECONDS: i64 = 60;
const PLAYBOOK_RUN_EVENT_TYPE_MAX_CHARS: usize = 120;
const PLAYBOOK_RUN_EVENT_MESSAGE_MAX_CHARS: usize = 8_000;
/// Raw `console_line` passthrough events carry the verbatim `ansible-playbook`
/// stdout/stderr line; they keep the 1:1 log promise far past the derived-event
/// cap and are only clipped at a pathological length (64 KiB).
const PLAYBOOK_RUN_EVENT_RAW_MESSAGE_MAX_CHARS: usize = 65_536;
/// Marker appended to an event message clipped by the ingest cap, so the log
/// shows the truncation instead of silently losing the tail.
const PLAYBOOK_RUN_EVENT_TRUNCATION_MARKER: &str = "…[truncated]";
/// Replacement for the values of plaintext-credential-looking keys inside
/// runner event payloads.
const PLAYBOOK_RUN_EVENT_SECRET_MASK: &str = "***";
/// Event type of the raw 1:1 ansible stdout/stderr passthrough lines emitted by
/// the runner sidecar.
const PLAYBOOK_RUN_EVENT_RAW_CONSOLE_EVENT_TYPE: &str = "console_line";
const PLAYBOOK_RUN_GROUP_EVENT_BATCH_MAX_ITEMS: usize = 1_000;
const PLAYBOOK_RUN_GROUP_COMPLETION_BATCH_MAX_ITEMS: usize = 50_000;
const PLAYBOOK_VERSION_DEFAULT_TIMEOUT_SECONDS: i32 = 1_800;
const PLAYBOOK_VERSION_MAX_TIMEOUT_SECONDS: i32 = 86_400;
const PLAYBOOK_JOB_TOKEN_MAX_SCOPES: usize = 16;
const PLAYBOOK_RUNTIME_SECRET_ENV_MAX_ITEMS: usize = 32;
const PLAYBOOK_RUNTIME_SECRET_ENV_NAME_MAX_CHARS: usize = 128;
const PLAYBOOK_RUNTIME_SECRET_KEY_MAX_CHARS: usize = 200;
const PLAYBOOK_SCHEDULE_CRON_MAX_CHARS: usize = 200;
const PLAYBOOK_SCHEDULE_TIMEZONE_MAX_CHARS: usize = 120;
const PLAYBOOK_SERVICE_PRINCIPAL_REVOCATION_REASON_MAX_CHARS: usize = 2_000;
const PLAYBOOK_JOB_TOKEN_ALLOWED_SCOPES: &[&str] = &[
    "artifact:create",
    "event:append",
    "status:update",
    "inventory:read",
    "inventory:create",
    "inventory:update",
];
/// Provenance value for playbooks authored and editable in-app. Any other
/// `source_kind` (a `git` mirror or a built-in `system` playbook) is read-only at
/// the service boundary.
const PLAYBOOK_SOURCE_KIND_IN_APP: &str = "in_app";
const PLAYBOOK_SOURCE_KIND_GIT: &str = "git";

/// Provenance value for built-in playbooks seeded by migration (e.g. the
/// auto-update inventory schedule). Like `git`, `system` playbooks are read-only:
/// they are not `in_app`, so `ensure_playbook_editable` rejects every mutation.
const PLAYBOOK_SOURCE_KIND_SYSTEM: &str = "system";
const PROJECT_LAUNCH_PLACEHOLDER_PLAYBOOK: &str = r#"---
- name: Project-backed job template launch
  hosts: all
  gather_facts: false
  tasks:
    - name: Project bundle is materialized by the runner
      ansible.builtin.debug:
        msg: Project-backed playbook content is supplied through project_files at launch time.
"#;

#[derive(Debug, Clone, Deserialize)]
pub struct CreatePlaybookInput {
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

#[derive(Debug, Clone, Deserialize)]
pub struct CreatePlaybookVersionInput {
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

#[derive(Debug, Clone, Deserialize)]
pub struct CreateExecutionEnvironmentInput {
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

#[derive(Debug, Clone, Deserialize)]
pub struct CreateProjectInput {
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

#[derive(Debug, Clone, Deserialize)]
pub struct CreateJobTemplateInput {
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

/// Partial update for an execution environment. Absent fields (`None`) keep the
/// stored value; `Some(None)` clears a nullable field.
#[derive(Debug, Clone, Deserialize)]
pub struct UpdateExecutionEnvironmentInput {
    pub name: Option<String>,
    pub description: Option<Option<String>>,
    pub image: Option<String>,
    pub pull_policy: Option<String>,
    pub runner_group: Option<String>,
    pub enabled: Option<bool>,
    pub env: Option<Value>,
    pub settings: Option<Value>,
    pub metadata: Option<Value>,
}

/// Partial update for a playbook project. `scm_type` and `metadata` are
/// intentionally not patchable here: `scm_type` changes the whole launch
/// semantics, and `metadata` carries `project_files` which must only be replaced
/// through the dedicated upload endpoint. For `credential_id`,
/// `Some(Some(id))` binds the named secret, `Some(None)` clears it, and `None`
/// leaves it unchanged.
#[derive(Debug, Clone, Deserialize)]
pub struct UpdateProjectInput {
    pub name: Option<String>,
    pub description: Option<Option<String>>,
    pub scm_url: Option<Option<String>>,
    pub scm_branch: Option<Option<String>>,
    pub scm_ref: Option<Option<String>>,
    pub playbook_base_path: Option<Option<String>>,
    pub credential_id: Option<Option<Uuid>>,
    pub default_execution_environment_id: Option<Option<Uuid>>,
    pub sync_policy: Option<String>,
}

/// Partial update for a job template. The source binding
/// (`playbook_id` / `project_id` / `playbook_path`) is immutable here because it
/// is governed by a table CHECK constraint; recreate the template to change it.
#[derive(Debug, Clone, Deserialize)]
pub struct UpdateJobTemplateInput {
    pub name: Option<String>,
    pub description: Option<Option<String>>,
    pub inventory_target_set_id: Option<Option<Uuid>>,
    pub execution_environment_id: Option<Option<Uuid>>,
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

#[derive(Debug, Clone, Deserialize)]
pub struct UpdatePlaybookInput {
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
}

#[derive(Debug, Clone, Deserialize)]
pub struct StaticTargetMemberInput {
    pub target_kind: String,
    pub target_id: Uuid,
    pub source_kind: String,
    pub source_id: Option<Uuid>,
    pub sort_order: Option<i32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateTargetSetInput {
    pub name: String,
    pub mode: PlaybookTargetSetMode,
    pub selector: Option<Value>,
    pub members: Option<Vec<StaticTargetMemberInput>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateAssignmentInput {
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

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateAssignmentInput {
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

#[derive(Debug, Clone, Deserialize)]
pub struct CreateRunGroupInput {
    pub target_set_id: Option<Uuid>,
    pub schedule_id: Option<Uuid>,
    #[serde(skip)]
    pub scheduled_for: Option<DateTime<Utc>>,
    pub trigger_kind: Option<PlaybookRunTriggerKind>,
    pub target_limit: Option<i64>,
    pub metadata: Option<Value>,
    /// Server-authored C3 secret-use authorization snapshot (env → {secret_id,
    /// key}, no values). For a manual run the REST layer sets it after
    /// `ensure_secret_use_grants` clears; for a scheduled fire
    /// `create_scheduled_run_group` AUTHORS it (overriding any input) after the
    /// service-principal gate clears. `#[serde(skip)]` guarantees a client body
    /// can never forge it. The repository is the sole author of this key in
    /// `run_group.metadata` (it strips any caller-supplied
    /// `secret_env_authorization` before inserting this).
    #[serde(skip)]
    pub secret_env_authorization: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LaunchJobTemplateInput {
    pub target_set_id: Option<Uuid>,
    pub trigger_kind: Option<PlaybookRunTriggerKind>,
    pub target_limit: Option<i64>,
    pub metadata: Option<Value>,
}

/// Concrete authorization surface for a manual run group: the effective target
/// set and every host the run group will fan execution onto. The REST layer must
/// clear the requesting subject against all of these BEFORE creation (fail-closed).
#[derive(Debug, Clone)]
pub struct RunAuthorizationTargets {
    pub target_set_id: Uuid,
    pub target_ids: Vec<Uuid>,
}

/// An explicit credential account / shared-credential-set referenced by a run
/// group's `launch_options`. When present, the requesting subject must be
/// authorized to use it (secret visibility) BEFORE the run group is created,
/// because the persisted decision is trusted by the worker at run start.
/// `auto`/`none` choices reference no concrete credential and yield `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunCredentialReference {
    pub account_kind: String,
    pub account_id: Uuid,
}

/// Extract the explicit credential account / shared-credential-set a run group's
/// `launch_options` references, if any.
///
/// SECURITY: the run-group create path persists this reference with
/// `credential_resolution: resolve_at_worker_start`, so the worker trusts it
/// without re-checking. The REST layer therefore MUST authorize the requesting
/// subject against the returned reference (secret visibility) before creating the
/// run group. `auto`/`none` reference no concrete credential and return `None`.
/// This mirrors only the explicit-reference subset of the repository's
/// credential-decision parsing; the repository remains the authority for the full
/// normalization that is persisted.
pub fn explicit_run_credential_reference(
    metadata: Option<&Value>,
) -> Option<RunCredentialReference> {
    let launch_options = metadata?.get("launch_options").and_then(Value::as_object)?;
    let choice = launch_options
        .get("credential_choice")
        .and_then(Value::as_str)
        .unwrap_or("auto")
        .trim()
        .to_ascii_lowercase();
    let account_id = launch_options
        .get("credential_account_id")
        .or_else(|| launch_options.get("account_id"))
        .or_else(|| launch_options.get("credential_set_id"))
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value.trim()).ok())?;
    let account_kind = match choice.as_str() {
        "connection_config" | "shared_credential_set" => choice,
        "selected" | "explicit" => launch_options
            .get("credential_account_kind")
            .or_else(|| launch_options.get("account_kind"))
            .and_then(Value::as_str)
            .map(|value| value.trim().to_ascii_lowercase())?,
        _ => return None,
    };
    if !matches!(
        account_kind.as_str(),
        "connection_config" | "shared_credential_set"
    ) {
        return None;
    }
    Some(RunCredentialReference {
        account_kind,
        account_id,
    })
}

/// Parse the credential references carried by a job template.
///
/// Job templates may store references to already-managed credential objects, but
/// never inline usernames, passwords, private keys, or env material. The accepted
/// shape is intentionally narrow until the full AWX-style credential model is
/// first-class:
///
/// `{ "account_kind": "connection_config" | "shared_credential_set",
///    "account_id": "<uuid>", "purpose"?: "machine" | "vault" | "scm" }`
pub fn job_template_credential_references(
    credential_refs: Option<&Value>,
) -> Result<Vec<RunCredentialReference>, AppError> {
    parse_job_template_credential_refs(credential_refs)
        .map(|refs| refs.into_iter().map(|item| item.reference).collect())
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateScheduleInput {
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

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateScheduleInput {
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

#[derive(Debug, Clone, Deserialize)]
pub struct CreateServicePrincipalInput {
    pub name: String,
    pub slug: Option<String>,
    pub description: Option<String>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RevokeServicePrincipalInput {
    pub revocation_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CreateJobTokenInput {
    pub run_id: Uuid,
    pub expires_in_seconds: Option<i64>,
    pub scopes: Option<Vec<String>>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateArtifactInput {
    pub artifact_type: String,
    pub name: String,
    pub content_type: Option<String>,
    pub content_json: Option<Value>,
    pub content_text: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateRunEventInput {
    pub event_kind: PlaybookRunEventKind,
    pub event_type: String,
    pub message: Option<String>,
    pub payload: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateRunGroupEventInput {
    pub run_id: Option<Uuid>,
    pub host: Option<String>,
    pub event_kind: PlaybookRunEventKind,
    pub event_type: String,
    pub message: Option<String>,
    pub payload: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct LeaseQueuedRunsInput {
    pub lease_owner: String,
    pub limit: Option<i64>,
    pub lease_ttl_seconds: Option<i64>,
    pub max_concurrent_groups: Option<i64>,
    pub max_concurrent_runs_per_group: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct LeaseDueSchedulesInput {
    pub lease_owner: String,
    pub limit: Option<i64>,
    pub lease_ttl_seconds: Option<i64>,
    pub advances: Vec<ScheduleOccurrenceAdvance>,
}

#[derive(Debug, Clone)]
pub struct ScheduleOccurrenceAdvance {
    pub schedule_id: Uuid,
    pub scheduled_for: DateTime<Utc>,
    pub next_run_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct ReleaseScheduleOccurrenceInput {
    pub schedule_id: Uuid,
    pub scheduled_for: DateTime<Utc>,
    pub lease_owner: String,
}

#[derive(Debug, Clone)]
pub struct StartLeasedRunInput {
    pub run_id: Uuid,
    pub lease_owner: String,
    pub dispatch_id: Uuid,
    pub dispatch_attempt: i32,
    pub job_token_expires_in_seconds: Option<i64>,
    pub job_token_scopes: Option<Vec<String>>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct StartLeasedRunsInput {
    pub run_ids: Vec<Uuid>,
    pub lease_owner: String,
    pub dispatch_id: Uuid,
    pub dispatch_attempt: i32,
    pub job_token_expires_in_seconds: Option<i64>,
    pub job_token_scopes: Option<Vec<String>>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct MarkRunRunningInput {
    pub run_id: Uuid,
    pub lease_owner: String,
}

#[derive(Debug, Clone)]
pub struct MarkRunsRunningInput {
    pub run_ids: Vec<Uuid>,
    pub lease_owner: String,
}

#[derive(Debug, Clone)]
pub struct SetActiveRunTimeoutsInput {
    pub run_ids: Vec<Uuid>,
    pub lease_owner: String,
    pub timeout_seconds: i32,
}

#[derive(Debug, Clone)]
pub struct RequeueStartedRunsInput {
    pub run_ids: Vec<Uuid>,
    pub lease_owner: String,
    /// Backoff before the parked lease expires and the run becomes leaseable
    /// again by the normal `lease_queued_runs` reclaim path.
    pub retry_delay_seconds: i64,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct CompleteRunInput {
    pub run_id: Uuid,
    pub lease_owner: String,
    pub status: PlaybookRunStatus,
    pub exit_code: Option<i32>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CompleteRunGroupBatchItemInput {
    pub run_id: Uuid,
    pub status: PlaybookRunStatus,
    pub exit_code: Option<i32>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CompleteRunGroupBatchInput {
    pub lease_owner: String,
    pub runs: Vec<CompleteRunGroupBatchItemInput>,
    /// `known_hosts` lines the runner observed for the first time during this
    /// completion (TOFU harvest), persisted through the host key pin store.
    pub new_host_keys: Vec<String>,
}

/// Optional narrowing for one page of playbook run-group events.
///
/// Groups the three independent filters so `list_run_group_events` stays under
/// the argument-count limit; the host is normalized by the service before the
/// repository call.
#[derive(Debug, Clone, Default)]
pub struct PlaybookRunGroupEventListFilter {
    /// Restrict to a single run inside the group.
    pub run_id: Option<Uuid>,
    /// Restrict to a single host.
    pub host: Option<String>,
    /// Return only events whose `event_sequence` is greater than this.
    pub after_sequence: Option<i64>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct PlaybookCatalogItem {
    pub playbook: Playbook,
    pub current_version: Option<PlaybookVersion>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct PlaybookTargetAssignment {
    pub assignment: PlaybookAssignment,
    pub playbook: PlaybookCatalogItem,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct PlaybookTargetAssignments {
    pub target: PlaybookTargetPreviewItem,
    pub items: Vec<PlaybookTargetAssignment>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct PlaybookRunGroupCreated {
    pub run_group: PlaybookRunGroup,
    pub target_set: PlaybookTargetSet,
    pub runs: Vec<PlaybookRun>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct PlaybookRunGroupDetail {
    pub run_group: PlaybookRunGroup,
    pub runs: Vec<PlaybookRun>,
}

#[derive(Debug, Clone)]
pub struct CreatedPlaybookJobToken {
    pub token: PlaybookJobToken,
    pub raw_token: String,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct LeasedPlaybookRuns {
    pub lease_owner: String,
    pub lease_expires_at: DateTime<Utc>,
    pub runs: Vec<PlaybookRun>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct DuePlaybookSchedule {
    pub schedule: PlaybookSchedule,
    pub database_now: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct LeasedPlaybookScheduleOccurrence {
    pub schedule: PlaybookSchedule,
    pub scheduled_for: DateTime<Utc>,
    pub fired_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct LeasedPlaybookScheduleOccurrences {
    pub lease_owner: String,
    pub occurrences: Vec<LeasedPlaybookScheduleOccurrence>,
}

#[derive(Debug, Clone)]
pub struct PlaybookRunExecution {
    pub run: PlaybookRun,
    pub version: PlaybookVersion,
    pub job_token: PlaybookJobToken,
    pub raw_job_token: String,
}

#[derive(Debug, Clone)]
pub struct PlaybookRunGroupExecution {
    pub executions: Vec<PlaybookRunExecution>,
    pub raw_group_callback_token: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlaybookRuntimePolicy {
    pub expose_piping_api: bool,
    pub inventory_scopes: Vec<String>,
    pub secret_env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct PlaybookRunCompletion {
    pub run: PlaybookRun,
    pub run_group: PlaybookRunGroup,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct PlaybookRunGroupCompletionBatch {
    pub run_group: PlaybookRunGroup,
    pub completed_runs: Vec<PlaybookRun>,
}

#[derive(Debug, Clone, Default)]
pub struct TimedOutPlaybookRuns {
    pub runs: Vec<PlaybookRun>,
    pub run_groups: Vec<PlaybookRunGroup>,
}

pub struct PlaybookService<R>
where
    R: PlaybookRepository,
{
    repo: Arc<R>,
    service_principal_authorizer: Option<Arc<dyn PlaybookServicePrincipalAuthorizer>>,
    scheduled_run_secret_authorizer: Option<Arc<dyn PlaybookScheduledRunSecretAuthorizer>>,
}

impl<R> PlaybookService<R>
where
    R: PlaybookRepository,
{
    pub fn new(repo: Arc<R>) -> Self {
        Self {
            repo,
            service_principal_authorizer: None,
            scheduled_run_secret_authorizer: None,
        }
    }

    pub fn new_with_service_principal_authorizer(
        repo: Arc<R>,
        service_principal_authorizer: Arc<dyn PlaybookServicePrincipalAuthorizer>,
    ) -> Self {
        Self {
            repo,
            service_principal_authorizer: Some(service_principal_authorizer),
            scheduled_run_secret_authorizer: None,
        }
    }

    /// Wire the C3 authoring gate for scheduled fires. Without it, a scheduled
    /// fire of a secret-bearing playbook fails closed.
    pub fn with_scheduled_run_secret_authorizer(
        mut self,
        authorizer: Arc<dyn PlaybookScheduledRunSecretAuthorizer>,
    ) -> Self {
        self.scheduled_run_secret_authorizer = Some(authorizer);
        self
    }

    pub async fn list_catalog(
        &self,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<PlaybookCatalogItem>, AppError> {
        self.repo.list_catalog(limit, offset).await
    }

    pub async fn get_catalog_item(
        &self,
        id: Uuid,
    ) -> Result<Option<PlaybookCatalogItem>, AppError> {
        self.repo.get_catalog_item(id).await
    }

    pub async fn create_playbook(
        &self,
        input: CreatePlaybookInput,
        user_id: Option<Uuid>,
    ) -> Result<PlaybookCatalogItem, AppError> {
        let name = normalize_name(input.name)?;
        let slug = normalize_playbook_slug(input.slug.as_deref().unwrap_or(name.as_str()))?;
        let description =
            normalize_optional_text(input.description, PLAYBOOK_DESCRIPTION_MAX_CHARS)?;
        let category = normalize_category(input.category)?;
        let metadata = normalize_metadata(input.metadata)?;
        let content = normalize_content(input.content)?;
        let content_sha256 = hex_sha256(content.as_bytes());
        let input_schema = normalize_json_object(input.input_schema, "input_schema")?;
        let output_schema = normalize_json_object(input.output_schema, "output_schema")?;
        let token_policy = normalize_token_policy(input.token_policy)?;
        let timeout_seconds = normalize_version_timeout(input.timeout_seconds)?;
        let entrypoint = normalize_optional_text(input.entrypoint, PLAYBOOK_ENTRYPOINT_MAX_CHARS)?;
        let language = input.language.unwrap_or(PlaybookLanguage::Yaml);
        let status = input.status.unwrap_or(PlaybookStatus::Draft);
        let runtime = input.runtime.unwrap_or(PlaybookRuntime::Ansible);
        let risk = input.risk.unwrap_or(PlaybookRisk::Low);
        let record = CreatePlaybookRecord {
            slug,
            name,
            description,
            status,
            runtime,
            risk,
            owner_user_id: input.owner_user_id,
            category,
            vars_schema: normalize_optional_vars_schema(input.vars_schema)?,
            connection_default: input.connection_default.unwrap_or_default(),
            source_kind: PLAYBOOK_SOURCE_KIND_IN_APP.to_string(),
            language,
            entrypoint,
            content,
            content_sha256,
            input_schema,
            output_schema,
            token_policy,
            timeout_seconds,
            changelog: normalize_optional_text(input.changelog, PLAYBOOK_DESCRIPTION_MAX_CHARS)?,
            metadata,
            user_id,
        };
        self.repo.create_playbook(record).await
    }

    /// Duplicate any playbook (including system/git) into a new `in_app` copy.
    ///
    /// RBAC: same `Action::Create` gate as `create_playbook` — checked by the
    /// handler before calling here. The duplicate always has:
    /// - `source_kind = 'in_app'` (editable from the start)
    /// - playbook-level `inventory_rest_enabled = false`; copied playbooks do
    ///   not inherit the built-in system inventory REST capability
    /// - `metadata.system` and `metadata.factory` stripped
    /// - version 1 = body/vars_schema/defaults/timeout from current version
    /// - no schedules, assignments, or service-principal links
    pub async fn duplicate_playbook(
        &self,
        playbook_id: Uuid,
        name_override: Option<String>,
        user_id: Option<Uuid>,
    ) -> Result<PlaybookCatalogItem, AppError> {
        let Some(item) = self.repo.get_catalog_item(playbook_id).await? else {
            return Err(AppError::NotFound("Playbook not found".to_string()));
        };
        let source = &item.playbook;
        let source_version = item.current_version.as_ref().ok_or_else(|| {
            AppError::Conflict("Playbook has no current version to duplicate".to_string())
        })?;

        let new_name = name_override
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("{} (copy)", source.name));
        let new_name = normalize_name(new_name)?;

        // Generate a unique slug with collision suffixes.
        let base_slug = normalize_playbook_slug(new_name.as_str()).unwrap_or_else(|_| {
            // Fallback: truncate source slug to 120 chars and append -copy.
            let base = &source.slug[..source.slug.len().min(120)];
            format!("{base}-copy")
        });
        let slug = self.unique_slug(&base_slug).await?;

        // Strip system/factory metadata keys, keep the rest.
        let clean_meta = strip_system_metadata(&source.metadata);

        let record = CreatePlaybookRecord {
            slug,
            name: new_name,
            description: source.description.clone(),
            status: source.status,
            runtime: source_version.runtime,
            risk: source.risk,
            owner_user_id: user_id,
            category: source.category.clone(),
            vars_schema: source.vars_schema.clone(),
            connection_default: source.connection_default,
            source_kind: PLAYBOOK_SOURCE_KIND_IN_APP.to_string(),
            language: source_version.language,
            entrypoint: source_version.entrypoint.clone(),
            content: source_version.content.clone(),
            content_sha256: source_version.content_sha256.clone(),
            input_schema: source_version.input_schema.clone(),
            output_schema: source_version.output_schema.clone(),
            token_policy: source_version.token_policy.clone(),
            timeout_seconds: source_version.timeout_seconds,
            changelog: Some("Duplicated from source playbook".to_string()),
            metadata: clean_meta,
            user_id,
        };
        self.repo.create_playbook(record).await
    }

    /// Try `base_slug`, then `base_slug-2`, `base_slug-3`, … up to suffix 10.
    /// Returns the first slug that does not exist in the `playbook` table.
    async fn unique_slug(&self, base: &str) -> Result<String, AppError> {
        // First try the base slug.
        let catalog = self.repo.list_catalog(1_000, 0).await?.items;
        let existing_slugs: HashSet<&str> = catalog
            .iter()
            .map(|item| item.playbook.slug.as_str())
            .collect();

        if !existing_slugs.contains(base) {
            return Ok(base.to_string());
        }
        for n in 2u32..=10 {
            let candidate = format!("{base}-{n}");
            if !existing_slugs.contains(candidate.as_str()) {
                return Ok(candidate);
            }
        }
        // Fall back to UUID suffix for guaranteed uniqueness.
        Ok(format!("{base}-{}", Uuid::now_v7().simple()))
    }

    pub async fn list_playbook_versions(
        &self,
        playbook_id: Uuid,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<PlaybookVersion>, AppError> {
        if self.repo.get_catalog_item(playbook_id).await?.is_none() {
            return Err(AppError::NotFound("Playbook not found".to_string()));
        }
        self.repo.list_versions(playbook_id, limit, offset).await
    }

    /// Reject mutating operations on playbooks that are not authored in-app.
    ///
    /// Returns the playbook when it exists and is editable (`source_kind ==
    /// 'in_app'`); a 404 when it does not exist; and a 409 Conflict when it is a
    /// `git` mirror or a built-in `system` playbook and therefore read-only. Every
    /// in-app mutation path (`update_playbook`, `create_playbook_version`,
    /// `delete_playbook`) funnels through here so non-`in_app` playbooks fail
    /// closed by construction.
    async fn ensure_playbook_editable(&self, id: Uuid) -> Result<Playbook, AppError> {
        let Some(catalog_item) = self.repo.get_catalog_item(id).await? else {
            return Err(AppError::NotFound("Playbook not found".to_string()));
        };
        if catalog_item.playbook.source_kind != PLAYBOOK_SOURCE_KIND_IN_APP {
            let message = if catalog_item.playbook.source_kind == PLAYBOOK_SOURCE_KIND_SYSTEM {
                "playbook is a built-in system playbook and is read-only"
            } else {
                "playbook is managed by an external source and is read-only"
            };
            return Err(AppError::Conflict(message.to_string()));
        }
        Ok(catalog_item.playbook)
    }

    pub async fn create_playbook_version(
        &self,
        playbook_id: Uuid,
        input: CreatePlaybookVersionInput,
        user_id: Option<Uuid>,
    ) -> Result<PlaybookVersion, AppError> {
        self.ensure_playbook_editable(playbook_id).await?;
        let content = normalize_content(input.content)?;
        let content_sha256 = hex_sha256(content.as_bytes());
        let record = CreatePlaybookVersionRecord {
            playbook_id,
            language: input.language.unwrap_or(PlaybookLanguage::Yaml),
            runtime: input.runtime.unwrap_or(PlaybookRuntime::Ansible),
            entrypoint: normalize_optional_text(input.entrypoint, PLAYBOOK_ENTRYPOINT_MAX_CHARS)?,
            content,
            content_sha256,
            input_schema: normalize_json_object(input.input_schema, "input_schema")?,
            output_schema: normalize_json_object(input.output_schema, "output_schema")?,
            token_policy: normalize_token_policy(input.token_policy)?,
            timeout_seconds: normalize_version_timeout(input.timeout_seconds)?,
            changelog: normalize_optional_text(input.changelog, PLAYBOOK_DESCRIPTION_MAX_CHARS)?,
            metadata: normalize_metadata(input.metadata)?,
            user_id,
        };
        self.repo
            .create_version(record)
            .await?
            .ok_or_else(|| AppError::NotFound("Playbook not found".to_string()))
    }

    pub async fn update_playbook(
        &self,
        id: Uuid,
        input: UpdatePlaybookInput,
        user_id: Option<Uuid>,
    ) -> Result<Option<PlaybookCatalogItem>, AppError> {
        self.ensure_playbook_editable(id).await?;
        // When a default target set is being assigned, validate the FK up front
        // so a missing target set surfaces as a clean 404 instead of a generic
        // constraint violation. `Some(None)` clears the default and needs no
        // lookup; `None` leaves it unchanged.
        if let Some(Some(target_set_id)) = input.default_target_set_id
            && self.repo.get_target_set(target_set_id).await?.is_none()
        {
            return Err(AppError::NotFound("Target set not found".to_string()));
        }
        let record = UpdatePlaybookRecord {
            id,
            slug: input
                .slug
                .as_deref()
                .map(normalize_playbook_slug)
                .transpose()?,
            name: input.name.map(normalize_name).transpose()?,
            description: input
                .description
                .map(Some)
                .map(|value| normalize_optional_text(value, PLAYBOOK_DESCRIPTION_MAX_CHARS))
                .transpose()?
                .flatten(),
            status: input.status,
            runtime: input.runtime,
            risk: input.risk,
            owner_user_id: input.owner_user_id,
            default_target_set_id: input.default_target_set_id,
            category: input
                .category
                .map(Some)
                .map(normalize_category)
                .transpose()?,
            vars_schema: input
                .vars_schema
                .map(normalize_optional_vars_schema)
                .transpose()?,
            connection_default: input.connection_default,
            metadata: input.metadata.map(validate_metadata).transpose()?,
            user_id,
        };
        self.repo.update_playbook(record).await
    }

    pub async fn delete_playbook(&self, id: Uuid) -> Result<bool, AppError> {
        self.ensure_playbook_editable(id).await?;
        self.repo.delete_playbook(id).await
    }

    pub async fn list_execution_environments(
        &self,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<PlaybookExecutionEnvironment>, AppError> {
        self.repo.list_execution_environments(limit, offset).await
    }

    pub async fn get_execution_environment(
        &self,
        id: Uuid,
    ) -> Result<Option<PlaybookExecutionEnvironment>, AppError> {
        self.repo.get_execution_environment(id).await
    }

    pub async fn create_execution_environment(
        &self,
        input: CreateExecutionEnvironmentInput,
        user_id: Option<Uuid>,
    ) -> Result<PlaybookExecutionEnvironment, AppError> {
        let record = CreateExecutionEnvironmentRecord {
            name: normalize_required_text(input.name, "name", PLAYBOOK_TEMPLATE_NAME_MAX_CHARS)?,
            description: normalize_optional_text(
                input.description,
                PLAYBOOK_DESCRIPTION_MAX_CHARS,
            )?,
            image: normalize_required_text(
                input.image,
                "image",
                PLAYBOOK_CONTAINER_IMAGE_MAX_CHARS,
            )?,
            pull_policy: normalize_choice(
                input.pull_policy.as_deref().unwrap_or("if_not_present"),
                &["always", "if_not_present", "never"],
                "pull_policy",
            )?,
            runner_group: normalize_required_text(
                input.runner_group.unwrap_or_else(|| "default".to_string()),
                "runner_group",
                PLAYBOOK_RUNNER_GROUP_MAX_CHARS,
            )?,
            enabled: input.enabled.unwrap_or(true),
            env: normalize_json_object(input.env, "env")?,
            settings: normalize_json_object(input.settings, "settings")?,
            metadata: normalize_metadata(input.metadata)?,
            user_id,
        };
        self.repo.create_execution_environment(record).await
    }

    pub async fn list_projects(
        &self,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<PlaybookProject>, AppError> {
        self.repo.list_projects(limit, offset).await
    }

    pub async fn get_project(&self, id: Uuid) -> Result<Option<PlaybookProject>, AppError> {
        self.repo.get_project(id).await
    }

    pub async fn create_project(
        &self,
        input: CreateProjectInput,
        user_id: Option<Uuid>,
    ) -> Result<PlaybookProject, AppError> {
        let scm_type = normalize_choice(
            input.scm_type.as_deref().unwrap_or("manual"),
            &["manual", "git"],
            "scm_type",
        )?;
        // SCM credentials only apply to a git checkout; a manual project has no
        // remote to authenticate against. The reference is resolved to concrete
        // secret material lazily at launch time (see workers `resolve_scm_credential`).
        if input.credential_id.is_some() && scm_type != "git" {
            return Err(AppError::Validation(
                "credential_id is only supported for git playbook projects".to_string(),
            ));
        }
        let scm_url = normalize_project_scm_url(input.scm_url, &scm_type)?;
        if scm_type == "git" && scm_url.is_none() {
            return Err(AppError::Validation(
                "scm_url is required for git projects".to_string(),
            ));
        }
        if scm_type == "manual" && scm_url.is_some() {
            return Err(AppError::Validation(
                "manual projects must not set scm_url".to_string(),
            ));
        }
        let record = CreateProjectRecord {
            name: normalize_required_text(input.name, "name", PLAYBOOK_TEMPLATE_NAME_MAX_CHARS)?,
            description: normalize_optional_text(
                input.description,
                PLAYBOOK_DESCRIPTION_MAX_CHARS,
            )?,
            scm_type,
            scm_url,
            scm_branch: normalize_optional_text(input.scm_branch, PLAYBOOK_ENTRYPOINT_MAX_CHARS)?,
            scm_ref: normalize_optional_text(input.scm_ref, PLAYBOOK_ENTRYPOINT_MAX_CHARS)?,
            playbook_base_path: normalize_project_path(
                input.playbook_base_path,
                "playbook_base_path",
            )?,
            credential_id: input.credential_id,
            default_execution_environment_id: input.default_execution_environment_id,
            sync_policy: normalize_choice(
                input.sync_policy.as_deref().unwrap_or("manual"),
                &["manual", "on_launch", "scheduled"],
                "sync_policy",
            )?,
            metadata: normalize_metadata(input.metadata)?,
            user_id,
        };
        self.repo.create_project(record).await
    }

    pub async fn list_job_templates(
        &self,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<PlaybookJobTemplate>, AppError> {
        self.repo.list_job_templates(limit, offset).await
    }

    pub async fn get_job_template(
        &self,
        id: Uuid,
    ) -> Result<Option<PlaybookJobTemplate>, AppError> {
        self.repo.get_job_template(id).await
    }

    pub async fn create_job_template(
        &self,
        input: CreateJobTemplateInput,
        user_id: Option<Uuid>,
    ) -> Result<PlaybookJobTemplate, AppError> {
        let playbook_path = normalize_project_path(input.playbook_path, "playbook_path")?;
        match (input.playbook_id, input.project_id, playbook_path.as_ref()) {
            (Some(_), None, None) | (None, Some(_), Some(_)) => {}
            _ => {
                return Err(AppError::Validation(
                    "job template source must be either playbook_id or project_id + playbook_path"
                        .to_string(),
                ));
            }
        }
        let record = CreateJobTemplateRecord {
            name: normalize_required_text(input.name, "name", PLAYBOOK_TEMPLATE_NAME_MAX_CHARS)?,
            description: normalize_optional_text(
                input.description,
                PLAYBOOK_DESCRIPTION_MAX_CHARS,
            )?,
            playbook_id: input.playbook_id,
            project_id: input.project_id,
            playbook_path,
            inventory_target_set_id: input.inventory_target_set_id,
            execution_environment_id: input.execution_environment_id,
            credential_refs: normalize_job_template_credential_refs(input.credential_refs)?,
            extra_vars: normalize_json_object(input.extra_vars, "extra_vars")?,
            launch_options: normalize_json_object(input.launch_options, "launch_options")?,
            ask_variables_on_launch: input.ask_variables_on_launch.unwrap_or(false),
            ask_limit_on_launch: input.ask_limit_on_launch.unwrap_or(false),
            ask_tags_on_launch: input.ask_tags_on_launch.unwrap_or(false),
            ask_skip_tags_on_launch: input.ask_skip_tags_on_launch.unwrap_or(false),
            ask_credential_on_launch: input.ask_credential_on_launch.unwrap_or(false),
            ask_execution_environment_on_launch: input
                .ask_execution_environment_on_launch
                .unwrap_or(false),
            enabled: input.enabled.unwrap_or(true),
            metadata: normalize_metadata(input.metadata)?,
            user_id,
        };
        self.repo.create_job_template(record).await
    }

    pub async fn update_execution_environment(
        &self,
        id: Uuid,
        input: UpdateExecutionEnvironmentInput,
        user_id: Option<Uuid>,
    ) -> Result<Option<PlaybookExecutionEnvironment>, AppError> {
        let Some(existing) = self.repo.get_execution_environment(id).await? else {
            return Ok(None);
        };
        let description = match input.description {
            Some(value) => value,
            None => existing.description,
        };
        let record = CreateExecutionEnvironmentRecord {
            name: normalize_required_text(
                input.name.unwrap_or(existing.name),
                "name",
                PLAYBOOK_TEMPLATE_NAME_MAX_CHARS,
            )?,
            description: normalize_optional_text(description, PLAYBOOK_DESCRIPTION_MAX_CHARS)?,
            image: normalize_required_text(
                input.image.unwrap_or(existing.image),
                "image",
                PLAYBOOK_CONTAINER_IMAGE_MAX_CHARS,
            )?,
            pull_policy: normalize_choice(
                input
                    .pull_policy
                    .as_deref()
                    .unwrap_or(&existing.pull_policy),
                &["always", "if_not_present", "never"],
                "pull_policy",
            )?,
            runner_group: normalize_required_text(
                input.runner_group.unwrap_or(existing.runner_group),
                "runner_group",
                PLAYBOOK_RUNNER_GROUP_MAX_CHARS,
            )?,
            enabled: input.enabled.unwrap_or(existing.enabled),
            env: normalize_json_object(Some(input.env.unwrap_or(existing.env)), "env")?,
            settings: normalize_json_object(
                Some(input.settings.unwrap_or(existing.settings)),
                "settings",
            )?,
            metadata: normalize_metadata(Some(input.metadata.unwrap_or(existing.metadata)))?,
            user_id,
        };
        self.repo.update_execution_environment(id, record).await
    }

    /// Refuses to delete an execution environment that is still referenced by a
    /// project or job template; the FK is `ON DELETE SET NULL`, so without this
    /// guard the delete would silently strip live references.
    pub async fn delete_execution_environment(&self, id: Uuid) -> Result<bool, AppError> {
        if self.repo.count_execution_environment_references(id).await? > 0 {
            return Err(AppError::Conflict(
                "execution environment is still referenced by a project or job template"
                    .to_string(),
            ));
        }
        self.repo.delete_execution_environment(id).await
    }

    pub async fn update_project(
        &self,
        id: Uuid,
        input: UpdateProjectInput,
        user_id: Option<Uuid>,
    ) -> Result<Option<PlaybookProject>, AppError> {
        let Some(existing) = self.repo.get_project(id).await? else {
            return Ok(None);
        };
        let scm_type = existing.scm_type.clone();
        let scm_url_raw = match input.scm_url {
            Some(value) => value,
            None => existing.scm_url,
        };
        let scm_url = normalize_project_scm_url(scm_url_raw, &scm_type)?;
        if scm_type == "git" && scm_url.is_none() {
            return Err(AppError::Validation(
                "scm_url is required for git projects".to_string(),
            ));
        }
        if scm_type == "manual" && scm_url.is_some() {
            return Err(AppError::Validation(
                "manual projects must not set scm_url".to_string(),
            ));
        }
        let description = match input.description {
            Some(value) => value,
            None => existing.description,
        };
        let scm_branch = match input.scm_branch {
            Some(value) => value,
            None => existing.scm_branch,
        };
        let scm_ref = match input.scm_ref {
            Some(value) => value,
            None => existing.scm_ref,
        };
        let playbook_base_path = match input.playbook_base_path {
            Some(value) => value,
            None => existing.playbook_base_path,
        };
        let default_execution_environment_id = match input.default_execution_environment_id {
            Some(value) => value,
            None => existing.default_execution_environment_id,
        };
        let credential_id = match input.credential_id {
            Some(value) => value,
            None => existing.credential_id,
        };
        // SCM credentials only apply to a git checkout, exactly as on create.
        if credential_id.is_some() && scm_type != "git" {
            return Err(AppError::Validation(
                "credential_id is only supported for git playbook projects".to_string(),
            ));
        }
        let record = CreateProjectRecord {
            name: normalize_required_text(
                input.name.unwrap_or(existing.name),
                "name",
                PLAYBOOK_TEMPLATE_NAME_MAX_CHARS,
            )?,
            description: normalize_optional_text(description, PLAYBOOK_DESCRIPTION_MAX_CHARS)?,
            scm_type,
            scm_url,
            scm_branch: normalize_optional_text(scm_branch, PLAYBOOK_ENTRYPOINT_MAX_CHARS)?,
            scm_ref: normalize_optional_text(scm_ref, PLAYBOOK_ENTRYPOINT_MAX_CHARS)?,
            playbook_base_path: normalize_project_path(playbook_base_path, "playbook_base_path")?,
            credential_id,
            default_execution_environment_id,
            sync_policy: normalize_choice(
                input
                    .sync_policy
                    .as_deref()
                    .unwrap_or(&existing.sync_policy),
                &["manual", "on_launch", "scheduled"],
                "sync_policy",
            )?,
            // Preserved verbatim: metadata is only mutated through the dedicated
            // file-upload path so `project_files` is never clobbered by a PATCH.
            metadata: existing.metadata,
            user_id,
        };
        self.repo.update_project(id, record).await
    }

    /// Replaces the stored manual-project file set. Rejects git projects (their
    /// files come from the checkout) and reuses the exact launch-time bundle
    /// guards (relative paths, no traversal, no `ansible.cfg`, size ceilings).
    pub async fn set_project_files(
        &self,
        id: Uuid,
        files: Vec<AnsibleProjectFile>,
        user_id: Option<Uuid>,
    ) -> Result<Option<PlaybookProject>, AppError> {
        let Some(existing) = self.repo.get_project(id).await? else {
            return Ok(None);
        };
        if existing.scm_type != "manual" {
            return Err(AppError::Validation(
                "file upload is only supported for manual playbook projects".to_string(),
            ));
        }
        let validated = validate_manual_project_files(files)?;
        let project_files = serde_json::to_value(validated).map_err(|error| {
            AppError::Internal(format!("failed to serialize project files: {error}"))
        })?;
        self.repo
            .set_project_files(id, project_files, user_id)
            .await
    }

    /// Reads the stored manual-project file set for
    /// `GET /projects/{id}/files`. Returns `None` only when the project does not
    /// exist. Both manual and git projects answer with their stored
    /// `metadata.project_files`, which is an empty list when nothing was ever
    /// uploaded — the read path never invents a 4xx for a git project.
    pub async fn get_project_files(
        &self,
        id: Uuid,
    ) -> Result<Option<Vec<AnsibleProjectFile>>, AppError> {
        let Some(existing) = self.repo.get_project(id).await? else {
            return Ok(None);
        };
        Ok(Some(stored_project_files(&existing.metadata)?))
    }

    pub async fn update_job_template(
        &self,
        id: Uuid,
        input: UpdateJobTemplateInput,
        user_id: Option<Uuid>,
    ) -> Result<Option<PlaybookJobTemplate>, AppError> {
        let Some(existing) = self.repo.get_job_template(id).await? else {
            return Ok(None);
        };
        let description = match input.description {
            Some(value) => value,
            None => existing.description,
        };
        let inventory_target_set_id = match input.inventory_target_set_id {
            Some(value) => value,
            None => existing.inventory_target_set_id,
        };
        let execution_environment_id = match input.execution_environment_id {
            Some(value) => value,
            None => existing.execution_environment_id,
        };
        let credential_refs = match input.credential_refs {
            Some(value) => normalize_job_template_credential_refs(Some(value))?,
            None => existing.credential_refs,
        };
        let extra_vars = match input.extra_vars {
            Some(value) => normalize_json_object(Some(value), "extra_vars")?,
            None => existing.extra_vars,
        };
        let launch_options = match input.launch_options {
            Some(value) => normalize_json_object(Some(value), "launch_options")?,
            None => existing.launch_options,
        };
        let record = CreateJobTemplateRecord {
            name: normalize_required_text(
                input.name.unwrap_or(existing.name),
                "name",
                PLAYBOOK_TEMPLATE_NAME_MAX_CHARS,
            )?,
            description: normalize_optional_text(description, PLAYBOOK_DESCRIPTION_MAX_CHARS)?,
            // Source binding is immutable (governed by a table CHECK constraint).
            playbook_id: existing.playbook_id,
            project_id: existing.project_id,
            playbook_path: existing.playbook_path,
            inventory_target_set_id,
            execution_environment_id,
            credential_refs,
            extra_vars,
            launch_options,
            ask_variables_on_launch: input
                .ask_variables_on_launch
                .unwrap_or(existing.ask_variables_on_launch),
            ask_limit_on_launch: input
                .ask_limit_on_launch
                .unwrap_or(existing.ask_limit_on_launch),
            ask_tags_on_launch: input
                .ask_tags_on_launch
                .unwrap_or(existing.ask_tags_on_launch),
            ask_skip_tags_on_launch: input
                .ask_skip_tags_on_launch
                .unwrap_or(existing.ask_skip_tags_on_launch),
            ask_credential_on_launch: input
                .ask_credential_on_launch
                .unwrap_or(existing.ask_credential_on_launch),
            ask_execution_environment_on_launch: input
                .ask_execution_environment_on_launch
                .unwrap_or(existing.ask_execution_environment_on_launch),
            enabled: input.enabled.unwrap_or(existing.enabled),
            metadata: match input.metadata {
                Some(value) => normalize_metadata(Some(value))?,
                None => existing.metadata,
            },
            user_id,
        };
        self.repo.update_job_template(id, record).await
    }

    pub async fn delete_job_template(&self, id: Uuid) -> Result<bool, AppError> {
        self.repo.delete_job_template(id).await
    }

    /// Refuses to delete a project that still owns job templates; the FK is
    /// `ON DELETE CASCADE`, so without this guard the delete would silently take
    /// every dependent job template with it.
    pub async fn delete_project(&self, id: Uuid) -> Result<bool, AppError> {
        if self.repo.count_job_templates_for_project(id).await? > 0 {
            return Err(AppError::Conflict(
                "project still has job templates; delete or retarget them first".to_string(),
            ));
        }
        self.repo.delete_project(id).await
    }

    pub async fn launch_job_template(
        &self,
        template_id: Uuid,
        input: LaunchJobTemplateInput,
        user_id: Option<Uuid>,
        secret_env_authorization: Option<Value>,
    ) -> Result<PlaybookRunGroupCreated, AppError> {
        let Some(template) = self.repo.get_job_template(template_id).await? else {
            return Err(AppError::NotFound("Job template not found".to_string()));
        };
        if !template.enabled {
            return Err(AppError::Validation("Job template is disabled".to_string()));
        }
        let metadata = job_template_launch_metadata(&template, input.metadata)?;
        let playbook_id = if let Some(playbook_id) = template.playbook_id {
            playbook_id
        } else {
            if input
                .target_set_id
                .or(template.inventory_target_set_id)
                .is_none()
            {
                return Err(AppError::Validation(
                    "target_set_id is required for project-backed job templates".to_string(),
                ));
            }
            self.create_project_job_template_snapshot(&template, user_id)
                .await?
                .playbook
                .id
        };
        let run_input = CreateRunGroupInput {
            target_set_id: input.target_set_id.or(template.inventory_target_set_id),
            schedule_id: None,
            scheduled_for: None,
            trigger_kind: input.trigger_kind,
            target_limit: input.target_limit,
            metadata: Some(metadata),
            secret_env_authorization,
        };
        self.create_manual_run_group(playbook_id, run_input, user_id)
            .await
    }

    async fn create_project_job_template_snapshot(
        &self,
        template: &PlaybookJobTemplate,
        user_id: Option<Uuid>,
    ) -> Result<PlaybookCatalogItem, AppError> {
        let project_id = template.project_id.ok_or_else(|| {
            AppError::Validation("Project-backed job template is missing project_id".to_string())
        })?;
        let playbook_path = template.playbook_path.as_deref().ok_or_else(|| {
            AppError::Validation("Project-backed job template is missing playbook_path".to_string())
        })?;
        let Some(project) = self.repo.get_project(project_id).await? else {
            return Err(AppError::NotFound("Playbook project not found".to_string()));
        };
        let entrypoint =
            join_project_playbook_path(project.playbook_base_path.as_deref(), playbook_path)?;
        let content = PROJECT_LAUNCH_PLACEHOLDER_PLAYBOOK.to_string();
        let metadata = json!({
            "internal": true,
            "job_template_snapshot": true,
            "source": "playbooks.job_template.project_snapshot",
            "job_template_id": template.id.to_string(),
            "job_template_name": template.name.clone(),
            "project_id": project.id.to_string(),
            "project_name": project.name,
            "project_scm_type": project.scm_type,
            "project_playbook_path": playbook_path,
            "project_playbook_entrypoint": entrypoint.clone(),
        });
        let record = CreatePlaybookRecord {
            slug: format!("jt-{}-{}", template.id.simple(), Uuid::now_v7().simple()),
            name: template.name.clone(),
            description: template.description.clone(),
            status: PlaybookStatus::Active,
            runtime: PlaybookRuntime::Ansible,
            risk: PlaybookRisk::Low,
            owner_user_id: user_id,
            category: "automation".to_string(),
            vars_schema: None,
            connection_default: PlaybookConnection::Ssh,
            source_kind: PLAYBOOK_SOURCE_KIND_GIT.to_string(),
            language: PlaybookLanguage::Yaml,
            entrypoint: Some(entrypoint),
            content_sha256: hex_sha256(content.as_bytes()),
            content,
            input_schema: json!({}),
            output_schema: json!({}),
            token_policy: json!({ "scopes": ["artifact:create"], "ttl_seconds": 3600 }),
            timeout_seconds: PLAYBOOK_VERSION_DEFAULT_TIMEOUT_SECONDS,
            changelog: Some("Created for project-backed job template launch".to_string()),
            metadata,
            user_id,
        };
        self.repo.create_playbook(record).await
    }

    pub async fn create_target_set(
        &self,
        input: CreateTargetSetInput,
        user_id: Option<Uuid>,
    ) -> Result<PlaybookTargetSetPreview, AppError> {
        let name = normalize_name(input.name)?;
        let selector = normalize_selector(input.selector)?;
        let members = input
            .members
            .unwrap_or_default()
            .into_iter()
            .map(normalize_static_member)
            .collect::<Result<Vec<_>, _>>()?;
        if input.mode == PlaybookTargetSetMode::Static && members.is_empty() {
            return Err(AppError::Validation(
                "Static target set requires at least one member".to_string(),
            ));
        }
        let record = CreateTargetSetRecord {
            name,
            mode: input.mode,
            selector,
            members,
            user_id,
        };
        self.repo.create_target_set(record, 100).await
    }

    pub async fn update_target_set(
        &self,
        id: Uuid,
        input: CreateTargetSetInput,
        user_id: Option<Uuid>,
    ) -> Result<PlaybookTargetSet, AppError> {
        let name = normalize_name(input.name)?;
        let selector = normalize_selector(input.selector)?;
        let record = UpdateTargetSetRecord {
            id,
            name,
            mode: input.mode,
            entity_kind: "connection_target".to_string(),
            selector,
            user_id,
        };
        self.repo
            .update_target_set(record)
            .await?
            .ok_or_else(|| AppError::NotFound("Target set not found".to_string()))
    }

    pub async fn delete_target_set(&self, id: Uuid) -> Result<bool, AppError> {
        self.repo.delete_target_set(id).await
    }

    pub async fn prune_deleted_role_from_target_sets(
        &self,
        role_id: Uuid,
        role_name: &str,
    ) -> Result<u64, AppError> {
        self.repo
            .prune_deleted_role_from_target_sets(role_id, role_name)
            .await
    }

    pub async fn list_target_sets(
        &self,
        filter: &PlaybookTargetSetListFilter,
        limit: i64,
        offset: i64,
    ) -> Result<(Vec<PlaybookTargetSet>, i64), AppError> {
        self.repo
            .list_target_sets(filter, limit.clamp(1, 500), offset.max(0))
            .await
    }

    pub async fn get_target_set(&self, id: Uuid) -> Result<Option<PlaybookTargetSet>, AppError> {
        self.repo.get_target_set(id).await
    }

    pub async fn preview_target_set(
        &self,
        id: Uuid,
        limit: i64,
    ) -> Result<Option<PlaybookTargetSetPreview>, AppError> {
        self.repo.preview_target_set(id, limit.clamp(1, 500)).await
    }

    async fn ensure_quick_action_playbook_is_low_risk(
        &self,
        playbook_id: Uuid,
        quick_action_enabled: bool,
    ) -> Result<(), AppError> {
        if !quick_action_enabled {
            return Ok(());
        }
        let Some(playbook) = self.repo.get_catalog_item(playbook_id).await? else {
            return Err(AppError::NotFound("Playbook not found".to_string()));
        };
        if playbook.playbook.risk != PlaybookRisk::Low {
            return Err(AppError::Validation(
                "quick actions require a low-risk playbook".to_string(),
            ));
        }
        Ok(())
    }

    pub async fn create_assignment(
        &self,
        input: CreateAssignmentInput,
        user_id: Option<Uuid>,
    ) -> Result<PlaybookAssignment, AppError> {
        let quick_action_enabled = input.quick_action_enabled.unwrap_or(false);
        self.ensure_quick_action_playbook_is_low_risk(input.playbook_id, quick_action_enabled)
            .await?;
        let (scope_kind, scope_id) = normalize_assignment_scope(input.scope_kind, input.scope_id)?;
        let record = CreateAssignmentRecord {
            playbook_id: input.playbook_id,
            target_set_id: input.target_set_id,
            default_version_id: input.default_version_id,
            name: input
                .name
                .map(|value| normalize_required_text(value, "name", PLAYBOOK_NAME_MAX_CHARS))
                .transpose()?,
            scope_kind,
            scope_id,
            priority: input.priority.unwrap_or(0),
            enabled: input.enabled.unwrap_or(true),
            visible: input.visible.unwrap_or(true),
            executable: input.executable.unwrap_or(true),
            quick_action_enabled,
            default_input: normalize_json_object(input.default_input, "default_input")?,
            default_metadata: normalize_json_object(input.default_metadata, "default_metadata")?,
            default_timeout_seconds: normalize_optional_range(
                input.default_timeout_seconds,
                "default_timeout_seconds",
                1,
                PLAYBOOK_VERSION_MAX_TIMEOUT_SECONDS,
            )?,
            default_target_limit: normalize_optional_range(
                input.default_target_limit,
                "default_target_limit",
                1,
                PLAYBOOK_RUN_MAX_TARGET_LIMIT as i32,
            )?,
            metadata: normalize_metadata(input.metadata)?,
            user_id,
        };
        self.repo.create_assignment(record).await
    }

    pub async fn list_assignments(
        &self,
        filter: &PlaybookAssignmentListFilter,
        limit: i64,
        offset: i64,
    ) -> Result<(Vec<PlaybookAssignment>, i64), AppError> {
        self.repo
            .list_assignments(filter, limit.clamp(1, 500), offset.max(0))
            .await
    }

    pub async fn get_assignment(&self, id: Uuid) -> Result<Option<PlaybookAssignment>, AppError> {
        self.repo.get_assignment(id).await
    }

    pub async fn update_assignment(
        &self,
        id: Uuid,
        input: UpdateAssignmentInput,
        user_id: Option<Uuid>,
    ) -> Result<Option<PlaybookAssignment>, AppError> {
        let Some(existing) = self.repo.get_assignment(id).await? else {
            return Ok(None);
        };

        let scope_kind = input.scope_kind.unwrap_or(existing.scope_kind);
        let requested_scope_id = input.scope_id;
        let scope_id = match scope_kind {
            PlaybookAssignmentScopeKind::Global => {
                if matches!(requested_scope_id, Some(Some(_))) {
                    return Err(AppError::Validation(
                        "Global playbook assignments must not set scope_id".to_string(),
                    ));
                }
                None
            }
            _ => match requested_scope_id {
                Some(scope_id) => scope_id,
                None if input.scope_kind.is_some() && scope_kind != existing.scope_kind => {
                    return Err(AppError::Validation(
                        "scope_id is required when changing assignment scope kind".to_string(),
                    ));
                }
                None => existing.scope_id,
            },
        };
        let (scope_kind, scope_id) = normalize_assignment_scope(scope_kind, scope_id)?;

        let playbook_changed = input
            .playbook_id
            .is_some_and(|playbook_id| playbook_id != existing.playbook_id);
        let effective_playbook_id = input.playbook_id.unwrap_or(existing.playbook_id);
        let quick_action_enabled = input
            .quick_action_enabled
            .unwrap_or(existing.quick_action_enabled);
        self.ensure_quick_action_playbook_is_low_risk(effective_playbook_id, quick_action_enabled)
            .await?;
        let default_version_id = if playbook_changed && input.default_version_id.is_none() {
            Some(None)
        } else {
            input.default_version_id
        };

        let record = UpdateAssignmentRecord {
            id,
            playbook_id: input.playbook_id,
            target_set_id: input.target_set_id,
            default_version_id,
            name: input
                .name
                .map(|value| {
                    value
                        .map(|text| normalize_required_text(text, "name", PLAYBOOK_NAME_MAX_CHARS))
                        .transpose()
                })
                .transpose()?,
            scope_kind,
            scope_id,
            priority: input.priority,
            enabled: input.enabled,
            visible: input.visible,
            executable: input.executable,
            quick_action_enabled: input.quick_action_enabled,
            default_input: input
                .default_input
                .map(|value| normalize_json_object(Some(value), "default_input"))
                .transpose()?,
            default_metadata: input
                .default_metadata
                .map(|value| normalize_json_object(Some(value), "default_metadata"))
                .transpose()?,
            default_timeout_seconds: input
                .default_timeout_seconds
                .map(|value| {
                    value
                        .map(|seconds| {
                            normalize_optional_range(
                                Some(seconds),
                                "default_timeout_seconds",
                                1,
                                PLAYBOOK_VERSION_MAX_TIMEOUT_SECONDS,
                            )
                        })
                        .transpose()
                        .map(Option::flatten)
                })
                .transpose()?,
            default_target_limit: input
                .default_target_limit
                .map(|value| {
                    value
                        .map(|limit| {
                            normalize_optional_range(
                                Some(limit),
                                "default_target_limit",
                                1,
                                PLAYBOOK_RUN_MAX_TARGET_LIMIT as i32,
                            )
                        })
                        .transpose()
                        .map(Option::flatten)
                })
                .transpose()?,
            metadata: input.metadata.map(validate_metadata).transpose()?,
            user_id,
        };
        self.repo.update_assignment(record).await?.map_or_else(
            || {
                Err(AppError::NotFound(
                    "Playbook, target set, or default version not found".to_string(),
                ))
            },
            |assignment| Ok(Some(assignment)),
        )
    }

    pub async fn delete_assignment(&self, id: Uuid) -> Result<bool, AppError> {
        self.repo.delete_assignment(id).await
    }

    pub async fn list_target_assignments(
        &self,
        target_id: Uuid,
        limit: i64,
    ) -> Result<Option<PlaybookTargetAssignments>, AppError> {
        self.repo
            .list_target_assignments(target_id, limit.clamp(1, 100))
            .await
    }

    pub async fn create_schedule(
        &self,
        input: CreateScheduleInput,
        user_id: Option<Uuid>,
    ) -> Result<PlaybookSchedule, AppError> {
        let enabled = input.enabled.unwrap_or(true);
        if enabled && input.next_run_at.is_none() {
            return Err(AppError::Validation(
                "next_run_at is required for enabled playbook schedules".to_string(),
            ));
        }
        let record = CreateScheduleRecord {
            playbook_id: input.playbook_id,
            target_set_id: input.target_set_id,
            service_principal_id: input.service_principal_id,
            name: normalize_name(input.name)?,
            cron_expression: normalize_cron_expression(input.cron_expression)?,
            timezone: normalize_timezone(input.timezone)?,
            misfire_policy: input
                .misfire_policy
                .unwrap_or(PlaybookScheduleMisfirePolicy::FireOnce),
            concurrency_policy: input
                .concurrency_policy
                .unwrap_or(PlaybookScheduleConcurrencyPolicy::SkipIfRunning),
            enabled,
            next_run_at: input.next_run_at,
            metadata: normalize_metadata(input.metadata)?,
            user_id,
        };
        self.repo.create_schedule(record).await
    }

    pub async fn get_schedule(&self, id: Uuid) -> Result<Option<PlaybookSchedule>, AppError> {
        self.repo.get_schedule(id).await
    }

    pub async fn list_schedules(
        &self,
        filter: &PlaybookScheduleListFilter,
        limit: i64,
        offset: i64,
    ) -> Result<(Vec<PlaybookSchedule>, i64), AppError> {
        self.repo
            .list_schedules(filter, limit.clamp(1, 500), offset.max(0))
            .await
    }

    pub async fn update_schedule(
        &self,
        id: Uuid,
        input: UpdateScheduleInput,
        user_id: Option<Uuid>,
    ) -> Result<Option<PlaybookSchedule>, AppError> {
        // The schedule must exist before we can validate the resulting enabled
        // state against its (possibly unchanged) `next_run_at`.
        let Some(existing) = self.repo.get_schedule(id).await? else {
            return Ok(None);
        };
        // Re-validate cron with saffron and timezone with chrono-tz on update,
        // exactly as the create path does, so a schedule that updates is always
        // runnable by `compute_next_run_at`.
        let cron_expression = input
            .cron_expression
            .map(normalize_cron_expression)
            .transpose()?;
        let timezone = input
            .timezone
            .map(|value| normalize_timezone(Some(value)))
            .transpose()?;
        let name = input.name.map(normalize_name).transpose()?;
        let metadata = input.metadata.map(validate_metadata).transpose()?;
        // `chk_playbook_schedule_enabled_due`: a schedule that ends up enabled must
        // have a non-null `next_run_at`. Reject enabling without a usable cursor
        // (either supplied in this request or already present on the row).
        if input.enabled == Some(true)
            && input.next_run_at.is_none()
            && existing.next_run_at.is_none()
        {
            return Err(AppError::Validation(
                "next_run_at is required to enable a playbook schedule".to_string(),
            ));
        }
        let record = UpdateScheduleRecord {
            id,
            target_set_id: input.target_set_id,
            name,
            cron_expression,
            timezone,
            misfire_policy: input.misfire_policy,
            concurrency_policy: input.concurrency_policy,
            enabled: input.enabled,
            next_run_at: input.next_run_at,
            metadata,
            user_id,
        };
        self.repo.update_schedule(record).await
    }

    pub async fn delete_schedule(&self, id: Uuid) -> Result<bool, AppError> {
        self.repo.delete_schedule(id).await
    }

    pub async fn list_service_principals(
        &self,
        filter: &PlaybookServicePrincipalListFilter,
        limit: i64,
        offset: i64,
    ) -> Result<(Vec<PlaybookServicePrincipal>, i64), AppError> {
        self.repo
            .list_service_principals(filter, limit.clamp(1, 500), offset.max(0))
            .await
    }

    pub async fn get_service_principal(
        &self,
        id: Uuid,
    ) -> Result<Option<PlaybookServicePrincipal>, AppError> {
        self.repo.get_service_principal(id).await
    }

    pub async fn create_service_principal(
        &self,
        input: CreateServicePrincipalInput,
        user_id: Option<Uuid>,
    ) -> Result<PlaybookServicePrincipal, AppError> {
        let name = normalize_name(input.name)?;
        let slug =
            normalize_service_principal_slug(input.slug.as_deref().unwrap_or(name.as_str()))?;
        let description =
            normalize_optional_text(input.description, PLAYBOOK_DESCRIPTION_MAX_CHARS)?;
        let metadata = normalize_metadata(input.metadata)?;
        let record = CreateServicePrincipalRecord {
            name,
            slug,
            description,
            metadata,
            user_id,
        };
        self.repo.create_service_principal(record).await
    }

    pub async fn revoke_service_principal(
        &self,
        id: Uuid,
        input: RevokeServicePrincipalInput,
        user_id: Option<Uuid>,
        now: DateTime<Utc>,
    ) -> Result<Option<PlaybookServicePrincipal>, AppError> {
        let revocation_reason = normalize_optional_text(
            input.revocation_reason,
            PLAYBOOK_SERVICE_PRINCIPAL_REVOCATION_REASON_MAX_CHARS,
        )?;
        self.repo
            .revoke_service_principal(RevokeServicePrincipalRecord {
                id,
                revocation_reason,
                user_id,
                now,
            })
            .await
    }

    pub async fn get_run(&self, run_id: Uuid) -> Result<Option<PlaybookRun>, AppError> {
        self.repo.get_run(run_id).await
    }

    pub async fn list_runs(
        &self,
        filter: &PlaybookRunListFilter,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<PlaybookRun>, AppError> {
        self.repo.list_runs(filter, limit, offset).await
    }

    pub async fn list_run_groups(
        &self,
        filter: &PlaybookRunListFilter,
        limit: i64,
        offset: i64,
    ) -> Result<(Vec<PlaybookRunGroup>, i64), AppError> {
        self.repo
            .list_run_groups(filter, limit.clamp(1, 500), offset.max(0))
            .await
    }

    pub async fn get_run_group_with_runs(
        &self,
        run_group_id: Uuid,
        limit: i64,
    ) -> Result<Option<PlaybookRunGroupDetail>, AppError> {
        let detail = self
            .repo
            .get_run_group_with_runs(run_group_id, limit.clamp(1, PLAYBOOK_RUN_MAX_TARGET_LIMIT))
            .await?;
        Ok(detail
            .map(|RunGroupWithRuns { run_group, runs }| PlaybookRunGroupDetail { run_group, runs }))
    }

    /// Persist a cancellation request. Undispatched queued/leased work is
    /// terminalized immediately; dispatched work remains active until the
    /// cancel worker records runner confirmation or deadline expiry.
    pub async fn request_run_cancel(&self, run_id: Uuid) -> Result<Option<PlaybookRun>, AppError> {
        let now = Utc::now();
        self.repo
            .request_run_cancel(
                run_id,
                now,
                now + Duration::seconds(PLAYBOOK_CANCEL_DEADLINE_SECONDS),
            )
            .await
    }

    /// Persist one durable group cancellation command and return its current
    /// receipt. Repeated requests preserve the original deadline and outcome.
    pub async fn request_run_group_cancel(
        &self,
        run_group_id: Uuid,
        limit: i64,
    ) -> Result<Option<PlaybookRunGroupDetail>, AppError> {
        let now = Utc::now();
        let detail = self
            .repo
            .request_run_group_cancel(
                run_group_id,
                limit.clamp(1, PLAYBOOK_RUN_MAX_TARGET_LIMIT),
                now,
                now + Duration::seconds(PLAYBOOK_CANCEL_DEADLINE_SECONDS),
            )
            .await?;
        Ok(detail
            .map(|RunGroupWithRuns { run_group, runs }| PlaybookRunGroupDetail { run_group, runs }))
    }

    pub async fn list_pending_run_group_cancels(
        &self,
        limit: i64,
    ) -> Result<Vec<PlaybookRunGroup>, AppError> {
        self.repo
            .list_pending_run_group_cancels(limit.clamp(1, PLAYBOOK_LEASE_MAX_LIMIT))
            .await
    }

    pub async fn finalize_run_group_cancel(
        &self,
        run_group_id: Uuid,
        dispatch_id: Uuid,
        outcome: PlaybookCancelOutcome,
        now: DateTime<Utc>,
    ) -> Result<Option<FinalizedRunGroupCancelRecord>, AppError> {
        self.repo
            .finalize_run_group_cancel(FinalizeRunGroupCancelRecord {
                run_group_id,
                dispatch_id,
                outcome,
                now,
            })
            .await
    }

    pub async fn create_manual_run_group(
        &self,
        playbook_id: Uuid,
        input: CreateRunGroupInput,
        user_id: Option<Uuid>,
    ) -> Result<PlaybookRunGroupCreated, AppError> {
        if input.schedule_id.is_some() || input.scheduled_for.is_some() {
            return Err(AppError::Validation(
                "Manual run endpoint cannot attach a schedule occurrence".to_string(),
            ));
        }
        let trigger_kind = normalize_manual_run_trigger_kind(input.trigger_kind)?;
        self.create_run_group_internal(playbook_id, input, trigger_kind, user_id, None)
            .await
    }

    pub async fn create_scheduled_run_group(
        &self,
        playbook_id: Uuid,
        mut input: CreateRunGroupInput,
        service_principal_id: Option<Uuid>,
    ) -> Result<PlaybookRunGroupCreated, AppError> {
        if input.schedule_id.is_none() || input.scheduled_for.is_none() {
            return Err(AppError::Validation(
                "schedule_id and scheduled_for are required for scheduled run groups".to_string(),
            ));
        }
        let Some(service_principal_id) = service_principal_id else {
            return Err(AppError::Validation(
                "service_principal_id is required for scheduled run groups".to_string(),
            ));
        };
        let trigger_kind = normalize_scheduled_run_trigger_kind(input.trigger_kind)?;
        let target_set_id = self
            .resolve_run_target_set_id(playbook_id, input.target_set_id)
            .await?;
        let service_principal = self
            .ensure_scheduled_run_service_principal(
                service_principal_id,
                playbook_id,
                target_set_id,
            )
            .await?;

        // SECURITY (C3): a scheduled fire must apply the SAME secret-environment
        // boundary as a manual run. The service authors the snapshot here (the
        // worker never trusts a caller-supplied one) so the run worker can
        // re-check the run group's service principal at execution time.
        //
        // The token policy is parsed ONLY when the version actually declares
        // runtime secrets. A secretless version keeps its pre-existing
        // dispatch-time policy validation (the worker parses it there), so a
        // malformed policy with no `runtime.secrets.env` entry does not move its
        // failure from dispatch to fire.
        input.secret_env_authorization = None;
        if let Some(version) = self
            .repo
            .get_catalog_item(playbook_id)
            .await?
            .and_then(|item| item.current_version)
            && declares_runtime_secret_env(&version.token_policy)
        {
            let declared = runtime_policy_from_token_policy(&version.token_policy)?.secret_env;
            let Some(authorizer) = &self.scheduled_run_secret_authorizer else {
                return Err(AppError::Forbidden(
                    "Playbook scheduled-run secret authorizer is not configured".to_string(),
                ));
            };
            input.secret_env_authorization = authorizer
                .authorize_scheduled_run_secret_env(&service_principal, &declared)
                .await?;
        }

        self.create_run_group_internal(
            playbook_id,
            input,
            trigger_kind,
            None,
            Some(service_principal_id),
        )
        .await
    }

    async fn resolve_run_target_set_id(
        &self,
        playbook_id: Uuid,
        requested_target_set_id: Option<Uuid>,
    ) -> Result<Uuid, AppError> {
        let Some(catalog_item) = self.repo.get_catalog_item(playbook_id).await? else {
            return Err(AppError::NotFound("Playbook not found".to_string()));
        };
        requested_target_set_id
            .or(catalog_item.playbook.default_target_set_id)
            .ok_or_else(|| {
                AppError::Validation(
                    "target_set_id is required when the playbook has no default target set"
                        .to_string(),
                )
            })
    }

    /// Resolve the concrete fan-out for a manual run group exactly as
    /// `create_run_group` will, so the REST layer can authorize the requesting
    /// subject against every host BEFORE any run rows are persisted.
    ///
    /// SECURITY (C1): this resolves the effective target set (including the
    /// playbook's `default_target_set_id` fallback) and the same `target_limit`
    /// the repository's `create_run_group` uses, then previews the target set at
    /// that uncapped limit through the repository (the public `preview_target_set`
    /// wrapper clamps to 500 and must NOT be used for authorization — that would
    /// authorize at most 500 hosts while up to 50_000 are executed). The returned
    /// `target_set_id` and `target_ids` are the exact authorization surface the
    /// caller must clear before creating the run group.
    pub async fn resolve_manual_run_authorization_targets(
        &self,
        playbook_id: Uuid,
        requested_target_set_id: Option<Uuid>,
        requested_target_limit: Option<i64>,
    ) -> Result<RunAuthorizationTargets, AppError> {
        let target_set_id = self
            .resolve_run_target_set_id(playbook_id, requested_target_set_id)
            .await?;
        let target_limit = requested_target_limit
            .unwrap_or(PLAYBOOK_RUN_DEFAULT_TARGET_LIMIT)
            .clamp(1, PLAYBOOK_RUN_MAX_TARGET_LIMIT);
        let Some(preview) = self
            .repo
            .preview_target_set(target_set_id, target_limit)
            .await?
        else {
            return Err(AppError::NotFound("Target set not found".to_string()));
        };
        let target_ids = preview
            .items
            .iter()
            .map(|item| item.target_id)
            .collect::<Vec<_>>();
        Ok(RunAuthorizationTargets {
            target_set_id,
            target_ids,
        })
    }

    pub async fn resolve_target_set_authorization_targets(
        &self,
        requested_target_set_id: Option<Uuid>,
        requested_target_limit: Option<i64>,
    ) -> Result<RunAuthorizationTargets, AppError> {
        let target_set_id = requested_target_set_id.ok_or_else(|| {
            AppError::Validation(
                "target_set_id is required for project-backed job templates".to_string(),
            )
        })?;
        let target_limit = requested_target_limit
            .unwrap_or(PLAYBOOK_RUN_DEFAULT_TARGET_LIMIT)
            .clamp(1, PLAYBOOK_RUN_MAX_TARGET_LIMIT);
        let Some(preview) = self
            .repo
            .preview_target_set(target_set_id, target_limit)
            .await?
        else {
            return Err(AppError::NotFound("Target set not found".to_string()));
        };
        let target_ids = preview
            .items
            .iter()
            .map(|item| item.target_id)
            .collect::<Vec<_>>();
        Ok(RunAuthorizationTargets {
            target_set_id,
            target_ids,
        })
    }

    /// Load a playbook service principal and enforce the active/non-revoked
    /// gate, returning the opaque [`Principal::ServicePrincipal`].
    ///
    /// This is the canonical load + validated-construct site: a service
    /// principal is never built from a bare UUID anywhere else. The scheduled
    /// authoring path and the dispatch worker both reach the SP identity through
    /// this method, so a revoked or inactive principal can neither author nor
    /// dispatch a run.
    pub async fn load_validated_service_principal(
        &self,
        service_principal_id: Uuid,
    ) -> Result<Principal, AppError> {
        let Some(principal) = self
            .repo
            .get_service_principal(service_principal_id)
            .await?
        else {
            return Err(AppError::NotFound(
                "Playbook service principal not found".to_string(),
            ));
        };
        if principal.status != PlaybookServicePrincipalStatus::Active
            || principal.revoked_at.is_some()
        {
            return Err(AppError::Forbidden(
                "Playbook service principal is not active".to_string(),
            ));
        }
        Ok(Principal::service_principal(principal.id))
    }

    async fn ensure_scheduled_run_service_principal(
        &self,
        service_principal_id: Uuid,
        playbook_id: Uuid,
        target_set_id: Uuid,
    ) -> Result<Principal, AppError> {
        let principal = self
            .load_validated_service_principal(service_principal_id)
            .await?;
        let Some(authorizer) = &self.service_principal_authorizer else {
            return Err(AppError::Forbidden(
                "Playbook service principal authorizer is not configured".to_string(),
            ));
        };
        authorizer
            .authorize_scheduled_run(&principal, playbook_id, target_set_id)
            .await?;
        Ok(principal)
    }

    async fn create_run_group_internal(
        &self,
        playbook_id: Uuid,
        input: CreateRunGroupInput,
        trigger_kind: PlaybookRunTriggerKind,
        requested_by_user_id: Option<Uuid>,
        service_principal_id: Option<Uuid>,
    ) -> Result<PlaybookRunGroupCreated, AppError> {
        let target_set_id = self
            .resolve_run_target_set_id(playbook_id, input.target_set_id)
            .await?;
        let playbook = self
            .repo
            .get_catalog_item(playbook_id)
            .await?
            .ok_or_else(|| AppError::NotFound("Playbook not found".to_string()))?;
        let metadata = validate_run_launch_metadata(
            playbook.playbook.vars_schema.as_ref(),
            normalize_metadata(input.metadata)?,
        )?;
        let target_limit = input
            .target_limit
            .unwrap_or(PLAYBOOK_RUN_DEFAULT_TARGET_LIMIT)
            .clamp(1, PLAYBOOK_RUN_MAX_TARGET_LIMIT);
        let record = CreateRunGroupRecord {
            playbook_id,
            target_set_id,
            schedule_id: input.schedule_id,
            scheduled_for: input.scheduled_for,
            trigger_kind,
            requested_by_user_id,
            service_principal_id,
            metadata,
            target_limit,
            secret_env_authorization: input.secret_env_authorization,
        };
        let created = self.repo.create_run_group(record).await?;
        Ok(PlaybookRunGroupCreated {
            run_group: created.run_group,
            target_set: created.target_set,
            runs: created.runs,
        })
    }

    pub async fn create_job_token(
        &self,
        input: CreateJobTokenInput,
        now: DateTime<Utc>,
    ) -> Result<CreatedPlaybookJobToken, AppError> {
        let token_id = Uuid::now_v7();
        let secret = generate_token_secret(PLAYBOOK_JOB_TOKEN_SECRET_BYTES)?;
        let raw_token = format!("{PLAYBOOK_JOB_TOKEN_PREFIX}_{token_id}_{secret}");
        let token_hash = hex_sha256(raw_token.as_bytes());
        let token_prefix = format!(
            "{PLAYBOOK_JOB_TOKEN_PREFIX}_{}",
            secret.chars().take(8).collect::<String>()
        );
        let scopes = normalize_job_token_scopes(input.scopes)?;
        let ttl_seconds = input
            .expires_in_seconds
            .unwrap_or(PLAYBOOK_JOB_TOKEN_DEFAULT_TTL_SECONDS)
            .clamp(1, PLAYBOOK_JOB_TOKEN_MAX_TTL_SECONDS);
        let expires_at = now
            .checked_add_signed(Duration::seconds(ttl_seconds))
            .ok_or_else(|| AppError::Validation("Invalid job token expiration".to_string()))?;
        let record = CreateJobTokenRecord {
            id: token_id,
            run_id: input.run_id,
            token_hash,
            token_prefix,
            scopes,
            expires_at,
            metadata: normalize_metadata(input.metadata)?,
        };
        let token = self.repo.create_job_token(record).await?;
        Ok(CreatedPlaybookJobToken { token, raw_token })
    }

    pub async fn create_artifact_with_job_token(
        &self,
        run_id: Uuid,
        raw_token: &str,
        input: CreateArtifactInput,
        now: DateTime<Utc>,
    ) -> Result<PlaybookArtifact, AppError> {
        let record = normalize_artifact_input(run_id, input)?;
        self.repo
            .create_artifact_with_callback(callback_token_auth(raw_token, now)?, record)
            .await
    }

    pub async fn append_run_event_with_job_token(
        &self,
        run_id: Uuid,
        raw_token: &str,
        input: CreateRunEventInput,
        now: DateTime<Utc>,
    ) -> Result<PlaybookRunEvent, AppError> {
        let record = normalize_run_event_record(run_id, input)?;
        let content_size_bytes = callback_event_size_bytes(
            record.host.as_deref(),
            &record.event_type,
            record.message.as_deref(),
            &record.payload,
        )?;
        self.repo
            .append_run_event_with_callback(
                callback_token_auth(raw_token, now)?,
                record,
                content_size_bytes,
            )
            .await
    }

    pub async fn append_run_group_event_with_group_callback_token(
        &self,
        run_group_id: Uuid,
        raw_token: &str,
        input: CreateRunGroupEventInput,
        now: DateTime<Utc>,
    ) -> Result<PlaybookRunEvent, AppError> {
        let record = normalize_run_group_event_record(run_group_id, input)?;
        let content_size_bytes = callback_event_size_bytes(
            record.host.as_deref(),
            &record.event_type,
            record.message.as_deref(),
            &record.payload,
        )?;
        self.repo
            .append_run_group_event_with_callback(
                callback_token_auth(raw_token, now)?,
                record,
                content_size_bytes,
            )
            .await
    }

    pub async fn append_run_group_events_with_group_callback_token(
        &self,
        run_group_id: Uuid,
        batch_id: Uuid,
        raw_token: &str,
        inputs: Vec<CreateRunGroupEventInput>,
        now: DateTime<Utc>,
    ) -> Result<AppendRunGroupEventBatchOutcome, AppError> {
        if inputs.is_empty() {
            return Err(AppError::Validation(
                "event batch must contain at least one event".to_string(),
            ));
        }
        if inputs.len() > PLAYBOOK_RUN_GROUP_EVENT_BATCH_MAX_ITEMS {
            return Err(AppError::Validation(format!(
                "event batch cannot exceed {PLAYBOOK_RUN_GROUP_EVENT_BATCH_MAX_ITEMS} events"
            )));
        }

        let mut records = Vec::with_capacity(inputs.len());
        for input in inputs {
            let record = normalize_run_group_event_record(run_group_id, input)?;
            let content_size_bytes = callback_event_size_bytes(
                record.host.as_deref(),
                &record.event_type,
                record.message.as_deref(),
                &record.payload,
            )?;
            records.push((record, content_size_bytes));
        }

        self.repo
            .append_run_group_event_batch_with_callback(
                callback_token_auth(raw_token, now)?,
                batch_id,
                records,
            )
            .await
    }

    pub async fn complete_run_with_job_token(
        &self,
        run_id: Uuid,
        raw_token: &str,
        input: CompleteRunInput,
        finished_at: DateTime<Utc>,
    ) -> Result<(PlaybookRunCompletion, bool), AppError> {
        if input.run_id != run_id {
            return Err(AppError::Validation(
                "run_id path and payload must match".to_string(),
            ));
        }
        let record = CompleteRunRecord {
            run_id,
            lease_owner: normalize_lease_owner(input.lease_owner)?,
            status: normalize_terminal_run_status(input.status)?,
            exit_code: input.exit_code,
            error_message: normalize_optional_text(
                input.error_message,
                PLAYBOOK_RUN_ERROR_MAX_CHARS,
            )?,
            finished_at,
        };
        let completed = self
            .repo
            .complete_run_with_callback(callback_token_auth(raw_token, finished_at)?, record)
            .await?;
        if completed.newly_completed {
            record_terminal_run_metrics(&completed.run);
        }
        Ok((
            PlaybookRunCompletion {
                run: completed.run,
                run_group: completed.run_group,
            },
            completed.newly_completed,
        ))
    }

    pub async fn complete_run_group_with_group_callback_token(
        &self,
        run_group_id: Uuid,
        raw_token: &str,
        input: CompleteRunGroupBatchInput,
        finished_at: DateTime<Utc>,
    ) -> Result<PlaybookRunGroupCompletionBatch, AppError> {
        if input.runs.is_empty() {
            return Err(AppError::Validation(
                "completion batch must contain at least one run".to_string(),
            ));
        }
        if input.runs.len() > PLAYBOOK_RUN_GROUP_COMPLETION_BATCH_MAX_ITEMS {
            return Err(AppError::Validation(format!(
                "completion batch cannot exceed {PLAYBOOK_RUN_GROUP_COMPLETION_BATCH_MAX_ITEMS} runs"
            )));
        }

        let mut unique_run_ids = HashSet::with_capacity(input.runs.len());
        let mut records = Vec::with_capacity(input.runs.len());
        for item in input.runs {
            if !unique_run_ids.insert(item.run_id) {
                return Err(AppError::Validation(
                    "completion batch contains duplicate run_id".to_string(),
                ));
            }
            records.push(CompleteRunBatchItemRecord {
                run_id: item.run_id,
                status: normalize_terminal_run_status(item.status)?,
                exit_code: item.exit_code,
                error_message: normalize_optional_text(
                    item.error_message,
                    PLAYBOOK_RUN_ERROR_MAX_CHARS,
                )?,
            });
        }
        let run_ids = unique_run_ids.into_iter().collect::<Vec<_>>();
        let valid_count = self
            .repo
            .count_run_group_runs(run_group_id, &run_ids)
            .await?;
        let expected_count = i64::try_from(run_ids.len()).map_err(|_| {
            AppError::Validation("completion batch is too large to validate".to_string())
        })?;
        if valid_count != expected_count {
            return Err(AppError::Validation(
                "completion batch contains a run outside of the run group".to_string(),
            ));
        }

        let completed = self
            .repo
            .complete_run_group_batch_with_callback(
                callback_token_auth(raw_token, finished_at)?,
                CompleteRunGroupBatchRecord {
                    run_group_id,
                    lease_owner: normalize_lease_owner(input.lease_owner)?,
                    runs: records,
                    new_host_keys: input.new_host_keys,
                    finished_at,
                },
            )
            .await?;

        for run in &completed.newly_completed_runs {
            record_terminal_run_metrics(run);
        }

        Ok(PlaybookRunGroupCompletionBatch {
            run_group: completed.run_group,
            completed_runs: completed.newly_completed_runs,
        })
    }

    /// Validate a run-bound playbook job token for normal inventory REST calls.
    ///
    /// Unlike lifecycle callbacks, inventory routes do not carry `run_id` in the
    /// URL. The repository still binds the token to a live run before returning
    /// it, and this service narrows it to explicit inventory REST scopes.
    pub async fn validate_inventory_rest_job_token(
        &self,
        raw_token: &str,
        now: DateTime<Utc>,
    ) -> Result<PlaybookJobToken, AppError> {
        let (token_id, _) = parse_job_token(raw_token)
            .ok_or_else(|| AppError::Auth("TOKEN_INVALID".to_string()))?;
        let token_hash = hex_sha256(raw_token.as_bytes());
        let token = self
            .repo
            .validate_active_job_token(token_id, token_hash.as_str(), now)
            .await?;
        if !job_token_has_scope(&token.scopes, PLAYBOOK_INVENTORY_READ_SCOPE)
            && !job_token_has_scope(&token.scopes, PLAYBOOK_INVENTORY_CREATE_SCOPE)
            && !job_token_has_scope(&token.scopes, PLAYBOOK_INVENTORY_UPDATE_SCOPE)
        {
            return Err(AppError::Forbidden(
                "TOKEN_SCOPE_DENIED: inventory".to_string(),
            ));
        }
        Ok(token)
    }

    pub async fn list_run_artifacts(
        &self,
        run_id: Uuid,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<PlaybookArtifact>, AppError> {
        self.repo.list_artifacts(run_id, limit, offset).await
    }

    pub async fn list_run_events(
        &self,
        run_id: Uuid,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<PlaybookRunEvent>, AppError> {
        self.repo.list_run_events(run_id, limit, offset).await
    }

    pub async fn list_run_group_events(
        &self,
        run_group_id: Uuid,
        filter: PlaybookRunGroupEventListFilter,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<PlaybookRunEvent>, AppError> {
        let filter = PlaybookRunGroupEventListFilter {
            run_id: filter.run_id,
            host: normalize_optional_text(filter.host, PLAYBOOK_RUN_EVENT_TYPE_MAX_CHARS)?,
            after_sequence: filter.after_sequence.map(|value| value.max(0)),
        };
        let mut page = self
            .repo
            .list_run_group_events(run_group_id, filter, limit, offset)
            .await?;
        page.items = dedupe_group_hostless_events(page.items);
        Ok(page)
    }

    pub async fn lease_queued_runs(
        &self,
        input: LeaseQueuedRunsInput,
        now: DateTime<Utc>,
    ) -> Result<LeasedPlaybookRuns, AppError> {
        let lease_owner = normalize_lease_owner(input.lease_owner)?;
        let limit = input
            .limit
            .unwrap_or(PLAYBOOK_LEASE_DEFAULT_LIMIT)
            .clamp(1, PLAYBOOK_RUN_LEASE_MAX_LIMIT);
        let ttl_seconds = input
            .lease_ttl_seconds
            .unwrap_or(PLAYBOOK_LEASE_DEFAULT_TTL_SECONDS)
            .clamp(1, PLAYBOOK_LEASE_MAX_TTL_SECONDS);
        let max_concurrent_groups = input
            .max_concurrent_groups
            .unwrap_or(limit)
            .clamp(1, PLAYBOOK_RUN_MAX_TARGET_LIMIT);
        let max_concurrent_runs_per_group = input
            .max_concurrent_runs_per_group
            .unwrap_or(limit)
            .clamp(1, PLAYBOOK_RUN_MAX_TARGET_LIMIT);
        let lease_expires_at = now
            .checked_add_signed(Duration::seconds(ttl_seconds))
            .ok_or_else(|| AppError::Validation("Invalid lease expiration".to_string()))?;
        let record = LeaseQueuedRunsRecord {
            lease_owner: lease_owner.clone(),
            limit,
            max_concurrent_groups,
            max_concurrent_runs_per_group,
            now,
            lease_expires_at,
        };
        let runs = self.repo.lease_queued_runs(record).await?;
        Ok(LeasedPlaybookRuns {
            lease_owner,
            lease_expires_at,
            runs,
        })
    }

    pub async fn list_due_schedules(
        &self,
        limit: Option<i64>,
    ) -> Result<Vec<DuePlaybookSchedule>, AppError> {
        let limit = limit
            .unwrap_or(PLAYBOOK_LEASE_DEFAULT_LIMIT)
            .clamp(1, PLAYBOOK_LEASE_MAX_LIMIT);
        self.repo.list_due_schedules(limit).await.map(|records| {
            records
                .into_iter()
                .map(
                    |DueScheduleRecord {
                         schedule,
                         database_now,
                     }| DuePlaybookSchedule {
                        schedule,
                        database_now,
                    },
                )
                .collect()
        })
    }

    pub async fn lease_schedule_occurrences(
        &self,
        input: LeaseDueSchedulesInput,
    ) -> Result<LeasedPlaybookScheduleOccurrences, AppError> {
        let lease_owner = normalize_lease_owner(input.lease_owner)?;
        let limit = input
            .limit
            .unwrap_or(PLAYBOOK_LEASE_DEFAULT_LIMIT)
            .clamp(1, PLAYBOOK_LEASE_MAX_LIMIT);
        let ttl_seconds = input
            .lease_ttl_seconds
            .unwrap_or(PLAYBOOK_LEASE_DEFAULT_TTL_SECONDS)
            .clamp(1, PLAYBOOK_LEASE_MAX_TTL_SECONDS);
        let mut seen = HashSet::with_capacity(input.advances.len());
        let advances = input
            .advances
            .into_iter()
            .map(|advance| {
                if !seen.insert(advance.schedule_id) {
                    return Err(AppError::Validation(
                        "A schedule can be advanced at most once per claim".to_string(),
                    ));
                }
                if advance
                    .next_run_at
                    .is_some_and(|next_run_at| next_run_at <= advance.scheduled_for)
                {
                    return Err(AppError::Validation(
                        "next_run_at must be after scheduled_for".to_string(),
                    ));
                }
                Ok(ScheduleOccurrenceAdvanceRecord {
                    schedule_id: advance.schedule_id,
                    scheduled_for: advance.scheduled_for,
                    next_run_at: advance.next_run_at,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let record = LeaseScheduleOccurrencesRecord {
            lease_owner: lease_owner.clone(),
            limit,
            lease_ttl_seconds: ttl_seconds,
            advances,
        };
        let occurrences = self
            .repo
            .lease_schedule_occurrences(record)
            .await?
            .into_iter()
            .map(
                |LeasedScheduleOccurrenceRecord {
                     schedule,
                     scheduled_for,
                     fired_at,
                 }| LeasedPlaybookScheduleOccurrence {
                    schedule,
                    scheduled_for,
                    fired_at,
                },
            )
            .collect();
        Ok(LeasedPlaybookScheduleOccurrences {
            lease_owner,
            occurrences,
        })
    }

    pub async fn release_schedule_occurrence(
        &self,
        input: ReleaseScheduleOccurrenceInput,
    ) -> Result<bool, AppError> {
        let lease_owner = normalize_lease_owner(input.lease_owner)?;
        self.repo
            .release_schedule_occurrence(ReleaseScheduleOccurrenceRecord {
                schedule_id: input.schedule_id,
                lease_owner,
                scheduled_for: input.scheduled_for,
            })
            .await
    }

    pub async fn start_leased_run(
        &self,
        input: StartLeasedRunInput,
        now: DateTime<Utc>,
    ) -> Result<PlaybookRunExecution, AppError> {
        if input.dispatch_attempt < 1 {
            return Err(AppError::Validation(
                "dispatch_attempt must be at least 1".to_string(),
            ));
        }
        let lease_owner = normalize_lease_owner(input.lease_owner)?;
        let token = self
            .build_job_token_record(
                input.run_id,
                input.job_token_expires_in_seconds,
                input.job_token_scopes,
                input.metadata,
                now,
            )
            .await?;
        let record = StartLeasedRunRecord {
            run_id: input.run_id,
            lease_owner,
            dispatch_id: input.dispatch_id,
            dispatch_attempt: input.dispatch_attempt,
            now,
            job_token: token.record,
        };
        let started = self.repo.start_leased_run(record).await?;
        Ok(PlaybookRunExecution {
            run: started.run,
            version: started.version,
            job_token: started.job_token,
            raw_job_token: token.raw_token,
        })
    }

    pub async fn start_leased_runs(
        &self,
        input: StartLeasedRunsInput,
        now: DateTime<Utc>,
    ) -> Result<PlaybookRunGroupExecution, AppError> {
        if input.run_ids.is_empty() {
            return Err(AppError::Validation(
                "start batch must contain at least one run".to_string(),
            ));
        }
        if input.dispatch_attempt < 1 {
            return Err(AppError::Validation(
                "dispatch_attempt must be at least 1".to_string(),
            ));
        }
        let mut unique_run_ids = HashSet::with_capacity(input.run_ids.len());
        for run_id in &input.run_ids {
            if !unique_run_ids.insert(*run_id) {
                return Err(AppError::Validation(
                    "start batch contains duplicate run ids".to_string(),
                ));
            }
        }

        let lease_owner = normalize_lease_owner(input.lease_owner)?;
        let mut token_records = Vec::with_capacity(input.run_ids.len());
        let mut raw_tokens_by_run_id = HashMap::with_capacity(input.run_ids.len());
        for run_id in &input.run_ids {
            let token = self
                .build_job_token_record(
                    *run_id,
                    input.job_token_expires_in_seconds,
                    input.job_token_scopes.clone(),
                    input.metadata.clone(),
                    now,
                )
                .await?;
            raw_tokens_by_run_id.insert(*run_id, token.raw_token);
            token_records.push(token.record);
        }
        let group_callback_token =
            build_group_callback_token_record(input.job_token_expires_in_seconds, now)?;

        let record = StartLeasedRunsRecord {
            run_ids: input.run_ids.clone(),
            lease_owner,
            dispatch_id: input.dispatch_id,
            dispatch_attempt: input.dispatch_attempt,
            now,
            job_tokens: token_records,
            group_callback_token: group_callback_token.record,
        };
        let started = self.repo.start_leased_runs(record).await?;
        let mut executions = Vec::with_capacity(started.len());
        for item in started {
            let raw_job_token = raw_tokens_by_run_id.remove(&item.run.id).ok_or_else(|| {
                AppError::Database("Raw job token missing from batch result".to_string())
            })?;
            executions.push(PlaybookRunExecution {
                run: item.run,
                version: item.version,
                job_token: item.job_token,
                raw_job_token,
            });
        }
        Ok(PlaybookRunGroupExecution {
            executions,
            raw_group_callback_token: group_callback_token.raw_token,
        })
    }

    /// Transition a durably accepted runner dispatch to `running`.
    pub async fn mark_run_running(
        &self,
        input: MarkRunRunningInput,
        now: DateTime<Utc>,
    ) -> Result<PlaybookRun, AppError> {
        let lease_owner = normalize_lease_owner(input.lease_owner)?;
        let record = MarkRunRunningRecord {
            run_id: input.run_id,
            lease_owner,
            now,
        };
        self.repo.mark_run_running(record).await
    }

    pub async fn mark_runs_running(
        &self,
        input: MarkRunsRunningInput,
        now: DateTime<Utc>,
    ) -> Result<Vec<PlaybookRun>, AppError> {
        if input.run_ids.is_empty() {
            return Ok(Vec::new());
        }
        let lease_owner = normalize_lease_owner(input.lease_owner)?;
        let record = MarkRunsRunningRecord {
            run_ids: input.run_ids,
            lease_owner,
            now,
        };
        self.repo.mark_runs_running(record).await
    }

    /// Return a definitively unaccepted dispatch to the lease queue. Callers
    /// must never use this for ambiguous transport failures.
    pub async fn requeue_started_runs(
        &self,
        input: RequeueStartedRunsInput,
        now: DateTime<Utc>,
    ) -> Result<Vec<PlaybookRun>, AppError> {
        if input.run_ids.is_empty() {
            return Ok(Vec::new());
        }
        let lease_owner = normalize_lease_owner(input.lease_owner)?;
        let retry_delay_seconds = input
            .retry_delay_seconds
            .clamp(1, PLAYBOOK_LEASE_MAX_TTL_SECONDS);
        let retry_at = now
            .checked_add_signed(Duration::seconds(retry_delay_seconds))
            .ok_or_else(|| AppError::Validation("Invalid dispatch retry backoff".to_string()))?;
        let reason = truncate_run_event_message(input.reason.trim(), PLAYBOOK_RUN_ERROR_MAX_CHARS);
        self.repo
            .requeue_started_runs(RequeueStartedRunsRecord {
                run_ids: input.run_ids,
                lease_owner,
                retry_at,
                reason,
                now,
            })
            .await
    }

    pub async fn set_active_run_timeouts(
        &self,
        input: SetActiveRunTimeoutsInput,
        now: DateTime<Utc>,
    ) -> Result<Vec<PlaybookRun>, AppError> {
        if input.run_ids.is_empty() {
            return Ok(Vec::new());
        }
        let lease_owner = normalize_lease_owner(input.lease_owner)?;
        let timeout_seconds = normalize_version_timeout(Some(input.timeout_seconds))?;
        let lease_expires_at = now
            .checked_add_signed(Duration::seconds(i64::from(timeout_seconds)))
            .ok_or_else(|| AppError::Validation("Invalid run timeout expiration".to_string()))?;
        self.repo
            .set_active_run_timeouts(SetActiveRunTimeoutsRecord {
                run_ids: input.run_ids,
                lease_owner,
                timeout_seconds,
                lease_expires_at,
                now,
            })
            .await
    }

    /// Complete a run, returning the completion plus a `newly_completed` flag.
    /// The flag is `false` when the run was already terminal on arrival (a late
    /// or duplicate sidecar callback, e.g. one racing an operator cancel): the
    /// repository returned the existing terminal state as an idempotent no-op,
    /// so the caller must NOT publish a completion frame or re-audit. It is kept
    /// OUT of the serialized [`PlaybookRunCompletion`] DTO (contract-locked) and
    /// surfaced as a tuple sibling instead.
    pub async fn complete_run(
        &self,
        input: CompleteRunInput,
        finished_at: DateTime<Utc>,
    ) -> Result<(PlaybookRunCompletion, bool), AppError> {
        let lease_owner = normalize_lease_owner(input.lease_owner)?;
        let status = normalize_terminal_run_status(input.status)?;
        let error_message =
            normalize_optional_text(input.error_message, PLAYBOOK_RUN_ERROR_MAX_CHARS)?;
        let record = CompleteRunRecord {
            run_id: input.run_id,
            lease_owner,
            status,
            exit_code: input.exit_code,
            error_message,
            finished_at,
        };
        let completed = self.repo.complete_run(record).await?;
        if completed.newly_completed {
            record_terminal_run_metrics(&completed.run);
        }
        Ok((
            PlaybookRunCompletion {
                run: completed.run,
                run_group: completed.run_group,
            },
            completed.newly_completed,
        ))
    }

    /// Fail runs that exceeded their per-run timeout while still in a
    /// non-terminal worker state (`dispatching`/`running`),
    /// rolling up the affected run-group counters. `start_leased_run` clears
    /// `lease_expires_at` for running runs, so a crashed worker leaves runs
    /// stuck forever; this reaper is the only path that reclaims them.
    pub async fn timeout_stuck_runs(
        &self,
        now: DateTime<Utc>,
        limit: Option<i64>,
    ) -> Result<TimedOutPlaybookRuns, AppError> {
        let limit = limit
            .unwrap_or(PLAYBOOK_LEASE_DEFAULT_LIMIT)
            .clamp(1, PLAYBOOK_LEASE_MAX_LIMIT);
        let timed_out = self
            .repo
            .timeout_stuck_runs(TimeoutStuckRunsRecord { now, limit })
            .await?;
        for run in &timed_out.runs {
            record_terminal_run_metrics(run);
        }
        Ok(TimedOutPlaybookRuns {
            runs: timed_out.runs,
            run_groups: timed_out.run_groups,
        })
    }

    async fn build_job_token_record(
        &self,
        run_id: Uuid,
        expires_in_seconds: Option<i64>,
        scopes: Option<Vec<String>>,
        metadata: Option<Value>,
        now: DateTime<Utc>,
    ) -> Result<BuiltJobTokenRecord, AppError> {
        let token_id = Uuid::now_v7();
        let secret = generate_token_secret(PLAYBOOK_JOB_TOKEN_SECRET_BYTES)?;
        let raw_token = format!("{PLAYBOOK_JOB_TOKEN_PREFIX}_{token_id}_{secret}");
        let token_hash = hex_sha256(raw_token.as_bytes());
        let token_prefix = format!(
            "{PLAYBOOK_JOB_TOKEN_PREFIX}_{}",
            secret.chars().take(8).collect::<String>()
        );
        let scopes = normalize_job_token_scopes(scopes)?;
        let ttl_seconds = expires_in_seconds
            .unwrap_or(PLAYBOOK_JOB_TOKEN_DEFAULT_TTL_SECONDS)
            .clamp(1, PLAYBOOK_JOB_TOKEN_MAX_TTL_SECONDS);
        let expires_at = now
            .checked_add_signed(Duration::seconds(ttl_seconds))
            .ok_or_else(|| AppError::Validation("Invalid job token expiration".to_string()))?;
        Ok(BuiltJobTokenRecord {
            record: CreateJobTokenRecord {
                id: token_id,
                run_id,
                token_hash,
                token_prefix,
                scopes,
                expires_at,
                metadata: normalize_metadata(metadata)?,
            },
            raw_token,
        })
    }
}

fn record_terminal_run_metrics(run: &PlaybookRun) {
    metrics::record_playbook_run_outcome(
        run.status.as_str(),
        run.queued_at,
        run.started_at,
        run.finished_at,
    );
}

#[async_trait]
pub trait PlaybookServicePrincipalAuthorizer: Send + Sync {
    /// Authorize the already-validated service-principal `principal` (obtained
    /// only from `PlaybookService::load_validated_service_principal`, which
    /// enforces the active/non-revoked gate) for a scheduled fire.
    async fn authorize_scheduled_run(
        &self,
        principal: &Principal,
        playbook_id: Uuid,
        target_set_id: Uuid,
    ) -> Result<(), AppError>;
}

/// Gate **C3** for a scheduled fire: clear the per-secret
/// `Execute × SecretVariable` grant for the run group's service principal and
/// author the `secret_env_authorization` snapshot.
///
/// The service calls this at author time for every declared runtime secret; the
/// app layer implements it with the shared `secret_gate` core so a scheduled
/// fire reaches the same verdict as a manual launch. The principal is the
/// already-validated identity from `ensure_scheduled_run_service_principal`;
/// the trait never receives, and cannot construct, a bare UUID.
#[async_trait]
pub trait PlaybookScheduledRunSecretAuthorizer: Send + Sync {
    async fn authorize_scheduled_run_secret_env(
        &self,
        principal: &Principal,
        declared: &BTreeMap<String, String>,
    ) -> Result<Option<Value>, AppError>;
}

#[async_trait]
impl<U, R, P> PlaybookServicePrincipalAuthorizer for AccessControlService<U, R, P>
where
    U: UserRepository,
    R: RoleRepository,
    P: PolicyRepository,
{
    async fn authorize_scheduled_run(
        &self,
        principal: &Principal,
        playbook_id: Uuid,
        target_set_id: Uuid,
    ) -> Result<(), AppError> {
        // `principal` is already the validated `Principal::ServicePrincipal` built
        // by `load_validated_service_principal` after the active/non-revoked load
        // gate; this authorizer never fabricates one from a bare id.
        for (action, resource_type, resource_id) in [
            (
                Action::Execute,
                ResourceType::Playbook,
                playbook_id.to_string(),
            ),
            (
                Action::Read,
                ResourceType::PlaybookTargetSet,
                target_set_id.to_string(),
            ),
            (
                Action::Create,
                ResourceType::PlaybookRunGroup,
                "scheduled".to_string(),
            ),
        ] {
            let decision = self
                .check_access_for_principal(
                    principal,
                    action,
                    resource_type,
                    Some(resource_id.as_str()),
                    None,
                )
                .await?;
            if decision.decision != PolicyDecision::Allow {
                return Err(AppError::Forbidden(format!(
                    "Playbook service principal {} is not allowed to fire scheduled runs",
                    principal_subject(principal)
                )));
            }
        }
        Ok(())
    }
}

/// Canonical `subject` string for a principal (`user:{uuid}` /
/// `service_principal:{uuid}`), matching the C3 snapshot's subject form.
pub fn principal_subject(principal: &Principal) -> String {
    match principal {
        Principal::User { id } => format!("user:{id}"),
        Principal::ServicePrincipal(sp_id) => format!("service_principal:{}", sp_id.get()),
    }
}

struct BuiltJobTokenRecord {
    record: CreateJobTokenRecord,
    raw_token: String,
}

struct BuiltGroupCallbackTokenRecord {
    record: CreateRunGroupCallbackTokenRecord,
    raw_token: String,
}

fn build_group_callback_token_record(
    expires_in_seconds: Option<i64>,
    now: DateTime<Utc>,
) -> Result<BuiltGroupCallbackTokenRecord, AppError> {
    let token_id = Uuid::now_v7();
    let secret = generate_token_secret(PLAYBOOK_JOB_TOKEN_SECRET_BYTES)?;
    let raw_token = format!("{PLAYBOOK_JOB_TOKEN_PREFIX}_{token_id}_{secret}");
    let ttl_seconds = expires_in_seconds
        .unwrap_or(PLAYBOOK_JOB_TOKEN_DEFAULT_TTL_SECONDS)
        .clamp(1, PLAYBOOK_JOB_TOKEN_MAX_TTL_SECONDS);
    let expires_at = now
        .checked_add_signed(Duration::seconds(ttl_seconds))
        .ok_or_else(|| AppError::Validation("Invalid callback token expiration".to_string()))?;
    Ok(BuiltGroupCallbackTokenRecord {
        record: CreateRunGroupCallbackTokenRecord {
            id: token_id,
            token_hash: hex_sha256(raw_token.as_bytes()),
            token_prefix: format!(
                "{PLAYBOOK_JOB_TOKEN_PREFIX}_{}",
                secret.chars().take(8).collect::<String>()
            ),
            expires_at,
            metadata: json!({}),
        },
        raw_token,
    })
}

fn normalize_name(value: String) -> Result<String, AppError> {
    let normalized = value.trim().to_string();
    if normalized.is_empty() {
        return Err(AppError::Validation(
            "Playbook name is required".to_string(),
        ));
    }
    if normalized.chars().count() > PLAYBOOK_NAME_MAX_CHARS {
        return Err(AppError::Validation(
            "Playbook name is too long".to_string(),
        ));
    }
    Ok(normalized)
}

pub fn normalize_playbook_slug(value: &str) -> Result<String, AppError> {
    let normalized = value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| match ch {
            'a'..='z' | '0'..='9' => ch,
            '-' | '_' => ch,
            _ => '-',
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");

    let normalized = normalized.trim_matches(['-', '_']).to_string();
    if normalized.len() < 3 || normalized.len() > 128 {
        return Err(AppError::Validation(
            "Playbook slug must be 3-128 URL-safe characters".to_string(),
        ));
    }
    Ok(normalized)
}

/// Normalize a service-principal slug to match the DB constraint
/// `chk_playbook_service_principal_slug` (`^[a-z0-9][a-z0-9_-]{1,78}[a-z0-9]$`):
/// lowercase, URL-safe, must start and end with an alphanumeric, 3-80 chars.
fn normalize_service_principal_slug(value: &str) -> Result<String, AppError> {
    let normalized = value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| match ch {
            'a'..='z' | '0'..='9' | '-' | '_' => ch,
            _ => '-',
        })
        .collect::<String>();
    let normalized = normalized.trim_matches(['-', '_']).to_string();
    if normalized.len() < 3 || normalized.len() > 80 {
        return Err(AppError::Validation(
            "Service principal slug must be 3-80 URL-safe characters".to_string(),
        ));
    }
    Ok(normalized)
}

fn normalize_optional_text(
    value: Option<String>,
    max_chars: usize,
) -> Result<Option<String>, AppError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let normalized = value.trim().to_string();
    if normalized.is_empty() {
        return Ok(None);
    }
    if normalized.chars().count() > max_chars {
        return Err(AppError::Validation("Text value is too long".to_string()));
    }
    Ok(Some(normalized))
}

fn normalize_choice(value: &str, allowed: &[&str], field: &str) -> Result<String, AppError> {
    let normalized = value.trim().to_ascii_lowercase();
    if allowed.iter().any(|allowed| *allowed == normalized) {
        return Ok(normalized);
    }
    Err(AppError::Validation(format!(
        "{field} must be one of: {}",
        allowed.join(", ")
    )))
}

fn normalize_project_path(value: Option<String>, field: &str) -> Result<Option<String>, AppError> {
    let Some(value) = normalize_optional_text(value, PLAYBOOK_PROJECT_PATH_MAX_CHARS)? else {
        return Ok(None);
    };
    if value.starts_with('/')
        || value.contains('\\')
        || value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(AppError::Validation(format!(
            "{field} must be a relative POSIX path without traversal"
        )));
    }
    Ok(Some(value))
}

fn join_project_playbook_path(
    base_path: Option<&str>,
    playbook_path: &str,
) -> Result<String, AppError> {
    let joined = match base_path.map(str::trim).filter(|value| !value.is_empty()) {
        Some(base_path) => format!("{base_path}/{playbook_path}"),
        None => playbook_path.to_string(),
    };
    normalize_project_path(Some(joined), "project playbook path")?
        .ok_or_else(|| AppError::Validation("project playbook path must not be empty".to_string()))
}

fn normalize_project_scm_url(
    value: Option<String>,
    scm_type: &str,
) -> Result<Option<String>, AppError> {
    let Some(value) = normalize_optional_text(value, PLAYBOOK_CONTAINER_IMAGE_MAX_CHARS)? else {
        return Ok(None);
    };
    if scm_type != "git" {
        return Ok(Some(value));
    }
    let parsed = Url::parse(&value)
        .map_err(|_| AppError::Validation("scm_url must be an absolute URL".to_string()))?;
    if !matches!(parsed.scheme(), "https" | "ssh") {
        return Err(AppError::Validation(
            "scm_url scheme must be https or ssh".to_string(),
        ));
    }
    if parsed.host_str().is_none() {
        return Err(AppError::Validation(
            "scm_url must include a host".to_string(),
        ));
    }
    if !parsed.password().unwrap_or_default().is_empty() {
        return Err(AppError::Validation(
            "scm_url must not contain credentials".to_string(),
        ));
    }
    if parsed.scheme() == "https" && !parsed.username().is_empty() {
        return Err(AppError::Validation(
            "https scm_url must not contain credentials".to_string(),
        ));
    }
    Ok(Some(value))
}

fn host_from_payload(payload: Option<&Value>) -> Option<String> {
    payload
        .and_then(Value::as_object)
        .and_then(|object| object.get("host"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

type HostlessEventKey = (String, String, Option<String>, String);
type HostlessEventSeen = (bool, HashSet<Uuid>);

fn dedupe_group_hostless_events(events: Vec<PlaybookRunEvent>) -> Vec<PlaybookRunEvent> {
    let mut seen_by_run: HashMap<HostlessEventKey, HostlessEventSeen> = HashMap::new();
    let mut deduped = Vec::with_capacity(events.len());
    for event in events {
        if event.host.is_none() {
            let payload_key = serde_json::to_string(&event.payload).unwrap_or_default();
            let key = (
                format!("{:?}", event.event_kind),
                event.event_type.clone(),
                event.message.clone(),
                payload_key,
            );
            let seen = seen_by_run.entry(key).or_default();
            if should_skip_hostless_event(seen, event.run_id) {
                continue;
            }
        }
        deduped.push(event);
    }
    deduped
}

fn should_skip_hostless_event(seen: &mut HostlessEventSeen, run_id: Option<Uuid>) -> bool {
    if let Some(run_id) = run_id {
        if !seen.1.is_empty() && !seen.1.contains(&run_id) {
            seen.1.insert(run_id);
            return true;
        }
        seen.1.insert(run_id);
        false
    } else {
        if seen.0 {
            return true;
        }
        seen.0 = true;
        false
    }
}

fn normalize_category(value: Option<String>) -> Result<String, AppError> {
    let normalized = value
        .unwrap_or_else(|| "general".to_string())
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| match ch {
            'a'..='z' | '0'..='9' => ch,
            '-' | '_' => ch,
            _ => '-',
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");

    let normalized = normalized.trim_matches(['-', '_']).to_string();
    if normalized.len() < 3 || normalized.len() > PLAYBOOK_CATEGORY_MAX_CHARS {
        return Err(AppError::Validation(
            "Playbook category must be 3-64 URL-safe characters".to_string(),
        ));
    }
    Ok(normalized)
}

fn normalize_cron_expression(value: String) -> Result<String, AppError> {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() > PLAYBOOK_SCHEDULE_CRON_MAX_CHARS {
        return Err(AppError::Validation(
            "cron_expression is too long".to_string(),
        ));
    }
    let valid_chars = normalized.chars().all(|ch| {
        ch.is_ascii_alphanumeric()
            || matches!(
                ch,
                ' ' | '*' | '?' | '/' | ',' | '-' | '_' | '#' | 'L' | 'W'
            )
    });
    if !valid_chars {
        return Err(AppError::Validation(
            "cron_expression contains unsupported characters".to_string(),
        ));
    }
    // Quartz `?` is semantically equivalent to `*` in the day-of-month and
    // day-of-week fields; saffron does not accept it, so normalize it before
    // parsing. Parse with the SAME parser the scheduler executes with
    // (`saffron::Cron`) so anything that validates here is guaranteed runnable
    // by `compute_next_run_at` — no fire-once-then-silently-disable divergence.
    let parseable = normalized.replace('?', "*");
    if Cron::from_str(&parseable).is_err() {
        return Err(AppError::Validation(
            "cron_expression must be a valid 5-field standard cron expression".to_string(),
        ));
    }
    Ok(parseable)
}

fn normalize_timezone(value: Option<String>) -> Result<String, AppError> {
    let normalized = value
        .unwrap_or_else(|| "UTC".to_string())
        .trim()
        .to_string();
    if normalized.is_empty()
        || normalized.chars().count() > PLAYBOOK_SCHEDULE_TIMEZONE_MAX_CHARS
        || normalized.starts_with('/')
        || normalized.ends_with('/')
        || normalized.contains("//")
        || normalized.contains("..")
        || !normalized
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '_' | '-' | '+'))
    {
        return Err(AppError::Validation(
            "timezone must be a valid IANA-style identifier".to_string(),
        ));
    }
    // Validate against the SAME tz database the scheduler resolves with so a
    // schedule that creates successfully is guaranteed to resolve a real UTC
    // instant in `compute_next_run_at`.
    if chrono_tz::Tz::from_str(&normalized).is_err() {
        return Err(AppError::Validation(
            "timezone must be a valid IANA timezone identifier".to_string(),
        ));
    }
    Ok(normalized)
}

fn normalize_assignment_scope(
    scope_kind: PlaybookAssignmentScopeKind,
    scope_id: Option<Uuid>,
) -> Result<(PlaybookAssignmentScopeKind, Option<Uuid>), AppError> {
    match scope_kind {
        PlaybookAssignmentScopeKind::Global => {
            if scope_id.is_some() {
                return Err(AppError::Validation(
                    "Global playbook assignments must not set scope_id".to_string(),
                ));
            }
            Ok((scope_kind, None))
        }
        _ => {
            let Some(scope_id) = scope_id else {
                return Err(AppError::Validation(
                    "Scoped playbook assignments require scope_id".to_string(),
                ));
            };
            Ok((scope_kind, Some(scope_id)))
        }
    }
}

fn normalize_optional_range(
    value: Option<i32>,
    field: &str,
    min: i32,
    max: i32,
) -> Result<Option<i32>, AppError> {
    if let Some(value) = value
        && !(min..=max).contains(&value)
    {
        return Err(AppError::Validation(format!(
            "{field} must be between {min} and {max}"
        )));
    }
    Ok(value)
}

fn normalize_run_event_type(value: String) -> Result<String, AppError> {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized.is_empty() || normalized.chars().count() > PLAYBOOK_RUN_EVENT_TYPE_MAX_CHARS {
        return Err(AppError::Validation(
            "Playbook run event type must be 1-120 characters".to_string(),
        ));
    }
    if !normalized
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':'))
    {
        return Err(AppError::Validation(
            "Playbook run event type must be machine-readable".to_string(),
        ));
    }
    Ok(normalized)
}

fn validate_run_launch_metadata(
    vars_schema: Option<&Value>,
    mut metadata: Value,
) -> Result<Value, AppError> {
    let Some(vars_schema) = vars_schema else {
        return Ok(metadata);
    };
    let extravars = metadata
        .get("launch_options")
        .and_then(Value::as_object)
        .and_then(|options| options.get("extravars"))
        .cloned()
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
    let extravars = merge_schema_defaults(vars_schema, extravars)?;
    validate_launch_extravars(Some(vars_schema), &extravars)?;
    let metadata_object = metadata
        .as_object_mut()
        .ok_or_else(|| AppError::Validation("run metadata must be a JSON object".to_string()))?;
    let launch_options = metadata_object
        .entry("launch_options".to_string())
        .or_insert_with(|| Value::Object(serde_json::Map::new()))
        .as_object_mut()
        .ok_or_else(|| AppError::Validation("launch_options must be a JSON object".to_string()))?;
    launch_options.insert("extravars".to_string(), extravars);
    Ok(metadata)
}

fn merge_schema_defaults(vars_schema: &Value, extravars: Value) -> Result<Value, AppError> {
    let mut merged = vars_schema
        .get("properties")
        .and_then(Value::as_object)
        .map(|properties| {
            properties
                .iter()
                .filter_map(|(name, spec)| {
                    spec.get("default")
                        .map(|value| (name.clone(), value.clone()))
                })
                .collect::<serde_json::Map<String, Value>>()
        })
        .unwrap_or_default();
    let extravars = extravars.as_object().ok_or_else(|| {
        AppError::Validation("launch_options.extravars must be a JSON object".to_string())
    })?;
    merged.extend(extravars.clone());
    Ok(Value::Object(merged))
}

fn validate_launch_extravars(
    vars_schema: Option<&Value>,
    extravars: &Value,
) -> Result<(), AppError> {
    let Some(vars_schema) = vars_schema else {
        return Ok(());
    };
    validate_json_schema_value(vars_schema, extravars, "extravars")
}

fn validate_json_schema_value(schema: &Value, value: &Value, path: &str) -> Result<(), AppError> {
    let schema = schema
        .as_object()
        .ok_or_else(|| AppError::Validation(format!("{path} schema must be a JSON object")))?;
    const SUPPORTED_SCHEMA_KEYWORDS: &[&str] = &[
        "type",
        "enum",
        "default",
        "title",
        "description",
        "minimum",
        "maximum",
        "minLength",
        "maxLength",
        "properties",
        "required",
        "additionalProperties",
        "items",
    ];
    if let Some(keyword) = schema
        .keys()
        .find(|keyword| !SUPPORTED_SCHEMA_KEYWORDS.contains(&keyword.as_str()))
    {
        return Err(AppError::Validation(format!(
            "{path} schema keyword `{keyword}` is not supported"
        )));
    }

    if let Some(expected_type) = schema.get("type") {
        let expected_type = expected_type
            .as_str()
            .ok_or_else(|| AppError::Validation(format!("{path} schema type must be a string")))?;
        let matches = match expected_type {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "boolean" => value.is_boolean(),
            "number" => value.is_number(),
            "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
            "null" => value.is_null(),
            _ => {
                return Err(AppError::Validation(format!(
                    "{path} schema has unsupported type `{expected_type}`"
                )));
            }
        };
        if !matches {
            return Err(AppError::Validation(format!(
                "{path} must be {expected_type}"
            )));
        }
    }

    if let Some(allowed_values) = schema.get("enum") {
        let allowed_values = allowed_values
            .as_array()
            .ok_or_else(|| AppError::Validation(format!("{path} schema enum must be an array")))?;
        if !allowed_values.contains(value) {
            return Err(AppError::Validation(format!(
                "{path} must be one of the allowed values"
            )));
        }
    }

    validate_json_schema_number(schema, value, path)?;
    validate_json_schema_string(schema, value, path)?;
    validate_json_schema_object(schema, value, path)?;
    validate_json_schema_array(schema, value, path)
}

fn validate_json_schema_number(
    schema: &serde_json::Map<String, Value>,
    value: &Value,
    path: &str,
) -> Result<(), AppError> {
    for (keyword, valid) in [("minimum", true), ("maximum", false)] {
        let Some(bound) = schema.get(keyword) else {
            continue;
        };
        let bound = bound.as_f64().ok_or_else(|| {
            AppError::Validation(format!("{path} schema {keyword} must be a number"))
        })?;
        if let Some(number) = value.as_f64()
            && ((valid && number < bound) || (!valid && number > bound))
        {
            return Err(AppError::Validation(format!("{path} violates {keyword}")));
        }
    }
    Ok(())
}

fn validate_json_schema_string(
    schema: &serde_json::Map<String, Value>,
    value: &Value,
    path: &str,
) -> Result<(), AppError> {
    for keyword in ["minLength", "maxLength"] {
        let Some(limit) = schema.get(keyword) else {
            continue;
        };
        let limit = limit.as_u64().ok_or_else(|| {
            AppError::Validation(format!(
                "{path} schema {keyword} must be a non-negative integer"
            ))
        })? as usize;
        if let Some(string) = value.as_str()
            && ((keyword == "minLength" && string.chars().count() < limit)
                || (keyword == "maxLength" && string.chars().count() > limit))
        {
            return Err(AppError::Validation(format!("{path} violates {keyword}")));
        }
    }
    Ok(())
}

fn validate_json_schema_object(
    schema: &serde_json::Map<String, Value>,
    value: &Value,
    path: &str,
) -> Result<(), AppError> {
    let Some(object) = value.as_object() else {
        return Ok(());
    };
    let properties = schema.get("properties").map_or(Ok(None), |properties| {
        properties.as_object().map(Some).ok_or_else(|| {
            AppError::Validation(format!("{path} schema properties must be an object"))
        })
    })?;
    validate_json_schema_required(schema, object, path)?;
    validate_json_schema_additional_properties(schema, properties, object, path)?;
    if let Some(properties) = properties {
        for (name, property_schema) in properties {
            if let Some(property) = object.get(name) {
                validate_json_schema_value(property_schema, property, &format!("{path}.{name}"))?;
            }
        }
    }
    Ok(())
}

fn validate_json_schema_required(
    schema: &serde_json::Map<String, Value>,
    object: &serde_json::Map<String, Value>,
    path: &str,
) -> Result<(), AppError> {
    let Some(required) = schema.get("required") else {
        return Ok(());
    };
    let required = required
        .as_array()
        .ok_or_else(|| AppError::Validation(format!("{path} schema required must be an array")))?;
    for name in required {
        let name = name.as_str().ok_or_else(|| {
            AppError::Validation(format!("{path} schema required entries must be strings"))
        })?;
        if !object.contains_key(name) {
            return Err(AppError::Validation(format!("{path}.{name} is required")));
        }
    }
    Ok(())
}

fn validate_json_schema_additional_properties(
    schema: &serde_json::Map<String, Value>,
    properties: Option<&serde_json::Map<String, Value>>,
    object: &serde_json::Map<String, Value>,
    path: &str,
) -> Result<(), AppError> {
    if !matches!(schema.get("additionalProperties"), Some(Value::Bool(false))) {
        return Ok(());
    }
    let Some(properties) = properties else {
        return Ok(());
    };
    if let Some(name) = object.keys().find(|name| !properties.contains_key(*name)) {
        return Err(AppError::Validation(format!(
            "{path}.{name} is not allowed by the schema"
        )));
    }
    Ok(())
}

fn validate_json_schema_array(
    schema: &serde_json::Map<String, Value>,
    value: &Value,
    path: &str,
) -> Result<(), AppError> {
    let Some(items) = schema.get("items") else {
        return Ok(());
    };
    let items = items
        .as_object()
        .ok_or_else(|| AppError::Validation(format!("{path} schema items must be an object")))?;
    if let Some(values) = value.as_array() {
        for (index, item) in values.iter().enumerate() {
            validate_json_schema_value(
                &Value::Object(items.clone()),
                item,
                &format!("{path}[{index}]"),
            )?;
        }
    }
    Ok(())
}

fn job_template_launch_metadata(
    template: &PlaybookJobTemplate,
    metadata: Option<Value>,
) -> Result<Value, AppError> {
    let mut metadata = normalize_metadata(metadata)?
        .as_object()
        .cloned()
        .unwrap_or_default();

    let mut launch_options = template
        .launch_options
        .as_object()
        .cloned()
        .unwrap_or_default();
    let user_launch_options = metadata
        .get("launch_options")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    let mut extravars = launch_options
        .get("extravars")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(template_extra_vars) = template.extra_vars.as_object() {
        extravars.extend(template_extra_vars.clone());
    }
    if let Some(user_extravars) = user_launch_options
        .get("extravars")
        .and_then(Value::as_object)
    {
        extravars.extend(user_extravars.clone());
    }

    launch_options.extend(user_launch_options);
    if !extravars.is_empty() {
        launch_options.insert("extravars".to_string(), Value::Object(extravars));
    }
    metadata.insert("launch_options".to_string(), Value::Object(launch_options));
    metadata.insert("source".to_string(), json!("playbooks.job_template"));
    metadata.insert(
        "job_template_id".to_string(),
        json!(template.id.to_string()),
    );
    metadata.insert(
        "job_template_name".to_string(),
        json!(template.name.clone()),
    );
    metadata.insert(
        "job_template".to_string(),
        json!({
            "id": template.id,
            "name": template.name,
            "playbook_id": template.playbook_id,
            "project_id": template.project_id,
            "playbook_path": template.playbook_path,
            "execution_environment_id": template.execution_environment_id,
            "source_kind": if template.project_id.is_some() { "project" } else { "playbook" },
        }),
    );

    Ok(Value::Object(metadata))
}

fn normalize_metadata(value: Option<Value>) -> Result<Value, AppError> {
    let value = value.unwrap_or_else(|| Value::Object(Default::default()));
    validate_metadata(value)
}

/// Strip the provenance-marking keys (`system`, `factory`) from a source
/// playbook's metadata when duplicating it, so a duplicate never carries the
/// built-in markers of its source. Returns a JSON object (empty when the source
/// metadata was not an object).
fn strip_system_metadata(metadata: &Value) -> Value {
    let Some(map) = metadata.as_object() else {
        return Value::Object(serde_json::Map::new());
    };
    let cleaned: serde_json::Map<String, Value> = map
        .iter()
        .filter(|(key, _)| key.as_str() != "system" && key.as_str() != "factory")
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    Value::Object(cleaned)
}

fn normalize_json_object(value: Option<Value>, field: &str) -> Result<Value, AppError> {
    let value = value.unwrap_or_else(|| Value::Object(Default::default()));
    if !value.is_object() {
        return Err(AppError::Validation(format!(
            "{field} must be a JSON object"
        )));
    }
    validate_no_plaintext_secret_keys(&value, field)?;
    Ok(value)
}

/// Normalize the optional typed launch-form `vars_schema`. `None`/`null` clears
/// it; a present value must be a JSON object (mirrors the `chk_playbook_vars_
/// schema_object` migration CHECK) and carry no plaintext secret keys.
fn normalize_optional_vars_schema(value: Option<Value>) -> Result<Option<Value>, AppError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(value) => {
            if !value.is_object() {
                return Err(AppError::Validation(
                    "vars_schema must be a JSON object".to_string(),
                ));
            }
            validate_no_plaintext_secret_keys(&value, "vars_schema")?;
            Ok(Some(value))
        }
    }
}

fn normalize_token_policy(value: Option<Value>) -> Result<Value, AppError> {
    let value = value.unwrap_or_else(|| {
        json!({
            "ttl_seconds": PLAYBOOK_JOB_TOKEN_DEFAULT_TTL_SECONDS,
            "scopes": [PLAYBOOK_ARTIFACT_CREATE_SCOPE],
        })
    });
    if !value.is_object() {
        return Err(AppError::Validation(
            "token_policy must be a JSON object".to_string(),
        ));
    }
    if let Some(ttl) = value.get("ttl_seconds") {
        let ttl = ttl.as_i64().ok_or_else(|| {
            AppError::Validation("token_policy.ttl_seconds must be an integer".to_string())
        })?;
        if !(1..=PLAYBOOK_JOB_TOKEN_MAX_TTL_SECONDS).contains(&ttl) {
            return Err(AppError::Validation(
                "token_policy.ttl_seconds is out of range".to_string(),
            ));
        }
    }
    if let Some(scopes) = value.get("scopes") {
        let scopes = scopes.as_array().ok_or_else(|| {
            AppError::Validation("token_policy.scopes must be an array".to_string())
        })?;
        if scopes.iter().any(|scope| {
            scope
                .as_str()
                .map(|value| value.trim().is_empty())
                .unwrap_or(true)
        }) {
            return Err(AppError::Validation(
                "token_policy.scopes must contain non-empty strings".to_string(),
            ));
        }
    }
    runtime_policy_from_token_policy(&value)?;
    validate_no_plaintext_secret_keys_except_runtime_secret_env(&value, "token_policy")?;
    Ok(value)
}

/// Lightweight declaration probe: does `token_policy` declare ANY
/// `runtime.secrets.env` entry?
///
/// This never validates and never fails. It exists so the scheduled-fire
/// authoring path can skip the full `runtime_policy_from_token_policy` parse for
/// a secretless version: the worker keeps owning that version's policy
/// validation at dispatch, so a malformed-but-secretless policy does not move
/// its failure from dispatch to fire. Any shape other than a non-empty
/// `runtime.secrets.env` object (absent, empty object, or a malformed
/// non-object) reports "no declared secrets" — the full parse then runs only if
/// this says yes, and if the full parse rejects the secret-bearing shape it
/// rejects at fire where the gate must run.
pub fn declares_runtime_secret_env(token_policy: &Value) -> bool {
    token_policy
        .get("runtime")
        .and_then(|runtime| runtime.get("secrets"))
        .and_then(|secrets| secrets.get("env"))
        .and_then(Value::as_object)
        .map(|env| !env.is_empty())
        .unwrap_or(false)
}

pub fn runtime_policy_from_token_policy(value: &Value) -> Result<PlaybookRuntimePolicy, AppError> {
    let Some(runtime) = value.get("runtime") else {
        return Ok(PlaybookRuntimePolicy::default());
    };
    let runtime = runtime.as_object().ok_or_else(|| {
        AppError::Validation("token_policy.runtime must be a JSON object".to_string())
    })?;

    let mut policy = PlaybookRuntimePolicy::default();
    if let Some(piping_api) = runtime.get("piping_api") {
        let piping_api = piping_api.as_object().ok_or_else(|| {
            AppError::Validation(
                "token_policy.runtime.piping_api must be a JSON object".to_string(),
            )
        })?;
        policy.expose_piping_api = piping_api
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if let Some(inventory) = piping_api.get("inventory") {
            if !policy.expose_piping_api {
                return Err(AppError::Validation(
                    "token_policy.runtime.piping_api.inventory requires runtime.piping_api.enabled=true"
                        .to_string(),
                ));
            }
            let inventory = inventory.as_array().ok_or_else(|| {
                AppError::Validation(
                    "token_policy.runtime.piping_api.inventory must be an array".to_string(),
                )
            })?;
            for item in inventory {
                let scope = match item.as_str().map(str::trim) {
                    Some("read") => PLAYBOOK_INVENTORY_READ_SCOPE,
                    Some("create") => PLAYBOOK_INVENTORY_CREATE_SCOPE,
                    Some("update") => PLAYBOOK_INVENTORY_UPDATE_SCOPE,
                    Some(other) => {
                        return Err(AppError::Validation(format!(
                            "Unsupported token_policy.runtime.piping_api.inventory permission: {other}"
                        )));
                    }
                    None => {
                        return Err(AppError::Validation(
                            "token_policy.runtime.piping_api.inventory must contain strings"
                                .to_string(),
                        ));
                    }
                };
                if !policy
                    .inventory_scopes
                    .iter()
                    .any(|existing| existing == scope)
                {
                    policy.inventory_scopes.push(scope.to_string());
                }
            }
        }
    }

    if let Some(secrets) = runtime.get("secrets") {
        let secrets = secrets.as_object().ok_or_else(|| {
            AppError::Validation("token_policy.runtime.secrets must be a JSON object".to_string())
        })?;
        if let Some(env) = secrets.get("env") {
            let env = env.as_object().ok_or_else(|| {
                AppError::Validation(
                    "token_policy.runtime.secrets.env must be a JSON object".to_string(),
                )
            })?;
            if env.len() > PLAYBOOK_RUNTIME_SECRET_ENV_MAX_ITEMS {
                return Err(AppError::Validation(
                    "token_policy.runtime.secrets.env has too many entries".to_string(),
                ));
            }
            for (env_name, secret_key) in env {
                validate_runtime_secret_env_name(env_name)?;
                let secret_key = secret_key.as_str().ok_or_else(|| {
                    AppError::Validation(
                        "token_policy.runtime.secrets.env values must be secret variable keys"
                            .to_string(),
                    )
                })?;
                let secret_key = secret_key.trim();
                if secret_key.is_empty()
                    || secret_key.chars().count() > PLAYBOOK_RUNTIME_SECRET_KEY_MAX_CHARS
                {
                    return Err(AppError::Validation(
                        "token_policy.runtime.secrets.env secret keys must be non-empty and bounded"
                            .to_string(),
                    ));
                }
                policy
                    .secret_env
                    .insert(env_name.clone(), secret_key.to_string());
            }
        }
    }

    Ok(policy)
}

fn validate_runtime_secret_env_name(value: &str) -> Result<(), AppError> {
    if value.is_empty()
        || value.chars().count() > PLAYBOOK_RUNTIME_SECRET_ENV_NAME_MAX_CHARS
        || !value.bytes().enumerate().all(|(index, byte)| match byte {
            b'A'..=b'Z' | b'_' => true,
            b'0'..=b'9' => index > 0,
            _ => false,
        })
    {
        return Err(AppError::Validation(
            "token_policy.runtime.secrets.env names must match [A-Z_][A-Z0-9_]*".to_string(),
        ));
    }
    if value.starts_with("PIPING_")
        || value.starts_with("ANSIBLE_")
        || value.starts_with("SSH_")
        || matches!(value, "HOME" | "PATH")
    {
        return Err(AppError::Validation(format!(
            "token_policy.runtime.secrets.env name `{value}` is reserved"
        )));
    }
    Ok(())
}

fn validate_no_plaintext_secret_keys_except_runtime_secret_env(
    value: &Value,
    field: &str,
) -> Result<(), AppError> {
    fn walk(value: &Value, field: &str, path: &mut Vec<String>) -> Result<(), AppError> {
        match value {
            Value::Object(map) => {
                for (key, child) in map {
                    let parent_is_runtime_secret_env =
                        path.as_slice() == ["runtime", "secrets", "env"];
                    path.push(key.clone());
                    if !parent_is_runtime_secret_env {
                        let normalized_key = key.to_ascii_lowercase().replace(['-', ' '], "_");
                        if is_plaintext_secret_key(&normalized_key) {
                            let full_path = path.join(".");
                            return Err(AppError::Validation(format!(
                                "{field} must not contain plaintext credential key `{full_path}`; store credential references instead"
                            )));
                        }
                    }
                    walk(child, field, path)?;
                    path.pop();
                }
            }
            Value::Array(items) => {
                for (index, child) in items.iter().enumerate() {
                    path.push(index.to_string());
                    walk(child, field, path)?;
                    path.pop();
                }
            }
            _ => {}
        }
        Ok(())
    }

    walk(value, field, &mut Vec::new())
}

fn normalize_job_token_scopes(scopes: Option<Vec<String>>) -> Result<Value, AppError> {
    let scopes = scopes.unwrap_or_else(|| vec![PLAYBOOK_ARTIFACT_CREATE_SCOPE.to_string()]);
    if scopes.len() > PLAYBOOK_JOB_TOKEN_MAX_SCOPES {
        return Err(AppError::Validation(
            "Too many playbook job token scopes".to_string(),
        ));
    }
    let mut normalized = Vec::<String>::new();
    for scope in scopes {
        let scope = scope.trim().to_ascii_lowercase();
        if scope.is_empty() {
            return Err(AppError::Validation(
                "Playbook job token scopes must be non-empty".to_string(),
            ));
        }
        if !PLAYBOOK_JOB_TOKEN_ALLOWED_SCOPES.contains(&scope.as_str()) {
            return Err(AppError::Validation(format!(
                "Unsupported playbook job token scope: {scope}"
            )));
        }
        if !normalized.iter().any(|existing| existing == &scope) {
            normalized.push(scope);
        }
    }
    if normalized.is_empty() {
        return Err(AppError::Validation(
            "Playbook job token requires at least one callback scope".to_string(),
        ));
    }
    Ok(json!(normalized))
}

fn normalize_selector(value: Option<Value>) -> Result<Value, AppError> {
    let value = value.unwrap_or_else(|| Value::Object(Default::default()));
    if !value.is_object() {
        return Err(AppError::Validation(
            "Target set selector must be a JSON object".to_string(),
        ));
    }
    validate_no_plaintext_secret_keys(&value, "selector")?;
    Ok(value)
}

#[derive(Debug, Clone)]
struct NormalizedTemplateCredentialRef {
    reference: RunCredentialReference,
    purpose: Option<String>,
}

fn parse_job_template_credential_refs(
    credential_refs: Option<&Value>,
) -> Result<Vec<NormalizedTemplateCredentialRef>, AppError> {
    let Some(value) = credential_refs else {
        return Ok(Vec::new());
    };
    if value.is_null() {
        return Ok(Vec::new());
    }
    validate_no_plaintext_secret_keys(value, "credential_refs")?;
    let items = value
        .as_array()
        .ok_or_else(|| AppError::Validation("credential_refs must be a JSON array".to_string()))?;
    let mut refs = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let object = item.as_object().ok_or_else(|| {
            AppError::Validation(format!("credential_refs[{index}] must be a JSON object"))
        })?;
        for key in object.keys() {
            if !matches!(
                key.as_str(),
                "account_kind" | "account_id" | "kind" | "id" | "purpose"
            ) {
                return Err(AppError::Validation(format!(
                    "credential_refs[{index}].{key} is not supported; store credential references only"
                )));
            }
        }
        let account_kind = object
            .get("account_kind")
            .or_else(|| object.get("kind"))
            .and_then(Value::as_str)
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                AppError::Validation(format!("credential_refs[{index}].account_kind is required"))
            })?;
        if !matches!(
            account_kind.as_str(),
            "connection_config" | "shared_credential_set"
        ) {
            return Err(AppError::Validation(format!(
                "credential_refs[{index}].account_kind must be connection_config or shared_credential_set"
            )));
        }
        let account_id = object
            .get("account_id")
            .or_else(|| object.get("id"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                AppError::Validation(format!("credential_refs[{index}].account_id is required"))
            })
            .and_then(|value| {
                Uuid::parse_str(value).map_err(|_| {
                    AppError::Validation(format!(
                        "credential_refs[{index}].account_id must be a UUID"
                    ))
                })
            })?;
        let purpose = object
            .get("purpose")
            .and_then(Value::as_str)
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty());
        if let Some(purpose) = purpose.as_deref()
            && !matches!(purpose, "machine" | "vault" | "scm")
        {
            return Err(AppError::Validation(format!(
                "credential_refs[{index}].purpose must be machine, vault, or scm"
            )));
        }
        refs.push(NormalizedTemplateCredentialRef {
            reference: RunCredentialReference {
                account_kind,
                account_id,
            },
            purpose,
        });
    }
    Ok(refs)
}

fn normalize_job_template_credential_refs(value: Option<Value>) -> Result<Value, AppError> {
    let refs = parse_job_template_credential_refs(value.as_ref())?;
    Ok(Value::Array(
        refs.into_iter()
            .map(|item| {
                let mut object = serde_json::Map::new();
                object.insert(
                    "account_kind".to_string(),
                    Value::String(item.reference.account_kind),
                );
                object.insert(
                    "account_id".to_string(),
                    Value::String(item.reference.account_id.to_string()),
                );
                if let Some(purpose) = item.purpose {
                    object.insert("purpose".to_string(), Value::String(purpose));
                }
                Value::Object(object)
            })
            .collect(),
    ))
}

fn normalize_static_member(
    value: StaticTargetMemberInput,
) -> Result<StaticTargetMemberRecord, AppError> {
    let target_kind = value.target_kind.trim().to_ascii_lowercase();
    if target_kind != "connection_target" {
        return Err(AppError::Validation(
            "Playbook execution supports connection_target members only".to_string(),
        ));
    }
    let source_kind = value.source_kind.trim().to_ascii_lowercase();
    if !matches!(source_kind.as_str(), "device" | "virtual_machine") {
        return Err(AppError::Validation(
            "Static playbook target members require source_kind device or virtual_machine"
                .to_string(),
        ));
    }
    Ok(StaticTargetMemberRecord {
        target_kind,
        target_id: value.target_id,
        source_kind,
        source_id: value.source_id.unwrap_or(value.target_id),
        sort_order: value.sort_order.unwrap_or(0).max(0),
    })
}

fn validate_metadata(value: Value) -> Result<Value, AppError> {
    if !value.is_object() {
        return Err(AppError::Validation(
            "Playbook metadata must be a JSON object".to_string(),
        ));
    }
    validate_no_plaintext_secret_keys(&value, "metadata")?;
    Ok(value)
}

/// Clip an ingested event message to `max_chars`, appending the visible
/// truncation marker when anything was cut.
fn truncate_run_event_message(message: &str, max_chars: usize) -> String {
    if message.chars().count() <= max_chars {
        return message.to_string();
    }
    let clipped: String = message.chars().take(max_chars).collect();
    format!("{clipped}{PLAYBOOK_RUN_EVENT_TRUNCATION_MARKER}")
}

/// Ingest-side message normalization for runner callback events. Unlike
/// `normalize_optional_text`, an over-long message is clipped with a visible
/// marker instead of rejecting the whole batch: a long ansible module result is
/// legitimate log evidence, not a validation error. Raw `console_line` lines
/// get a far higher cap so 1:1 output survives, and are never key-masked —
/// they are opaque ansible text, not structured payload.
fn normalize_run_event_message(message: Option<String>, event_type: &str) -> Option<String> {
    let message = message?;
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return None;
    }
    let max_chars = if event_type == PLAYBOOK_RUN_EVENT_RAW_CONSOLE_EVENT_TYPE {
        PLAYBOOK_RUN_EVENT_RAW_MESSAGE_MAX_CHARS
    } else {
        PLAYBOOK_RUN_EVENT_MESSAGE_MAX_CHARS
    };
    Some(truncate_run_event_message(trimmed, max_chars))
}

/// Mask (instead of reject) plaintext-credential-looking keys inside a runner
/// event payload. Ansible module results legitimately echo keys such as
/// `token`/`password`/`api_key`, so the callback must keep the event: the
/// secret VALUES are replaced with [`PLAYBOOK_RUN_EVENT_SECRET_MASK`] while the
/// object structure and every other key survive verbatim.
fn mask_plaintext_secret_values(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, child)| {
                    if is_plaintext_secret_key(&normalize_secret_key_name(&key)) {
                        (
                            key,
                            Value::String(PLAYBOOK_RUN_EVENT_SECRET_MASK.to_string()),
                        )
                    } else {
                        (key, mask_plaintext_secret_values(child))
                    }
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(mask_plaintext_secret_values)
                .collect(),
        ),
        other => other,
    }
}

/// Normalize a runner callback event payload. Structural validation (payload
/// must be a JSON object) still rejects with 400, but plaintext-secret keys are
/// masked in place instead of failing the batch — rejecting legitimate ansible
/// output would tear holes in the 1:1 run log.
fn normalize_run_event_payload(value: Option<Value>) -> Result<Value, AppError> {
    let value = value.unwrap_or_else(|| Value::Object(Default::default()));
    if !value.is_object() {
        return Err(AppError::Validation(
            "payload must be a JSON object".to_string(),
        ));
    }
    Ok(mask_plaintext_secret_values(value))
}

fn normalize_run_event_record(
    run_id: Uuid,
    input: CreateRunEventInput,
) -> Result<CreateRunEventRecord, AppError> {
    let host = host_from_payload(input.payload.as_ref());
    let event_type = normalize_run_event_type(input.event_type)?;
    let message = normalize_run_event_message(input.message, &event_type);
    Ok(CreateRunEventRecord {
        run_id,
        host,
        event_kind: input.event_kind,
        event_type,
        message,
        payload: normalize_run_event_payload(input.payload)?,
    })
}

fn normalize_run_group_event_record(
    run_group_id: Uuid,
    input: CreateRunGroupEventInput,
) -> Result<CreateRunGroupEventRecord, AppError> {
    let event_type = normalize_run_event_type(input.event_type)?;
    let message = normalize_run_event_message(input.message, &event_type);
    Ok(CreateRunGroupEventRecord {
        run_group_id,
        run_id: input.run_id,
        host: normalize_optional_text(input.host, PLAYBOOK_RUN_EVENT_TYPE_MAX_CHARS)?
            .or_else(|| host_from_payload(input.payload.as_ref())),
        event_kind: input.event_kind,
        event_type,
        message,
        payload: normalize_run_event_payload(input.payload)?,
    })
}

fn callback_event_size_bytes(
    host: Option<&str>,
    event_type: &str,
    message: Option<&str>,
    payload: &Value,
) -> Result<i64, AppError> {
    let encoded = serde_json::to_vec(&json!({
        "host": host,
        "event_type": event_type,
        "message": message,
        "payload": payload,
    }))?;
    i64::try_from(encoded.len())
        .map_err(|_| AppError::Validation("Callback event is too large".to_string()))
}

fn validate_no_plaintext_secret_keys(value: &Value, field: &str) -> Result<(), AppError> {
    fn walk(value: &Value, field: &str, path: &mut Vec<String>) -> Result<(), AppError> {
        match value {
            Value::Object(map) => {
                for (key, child) in map {
                    let normalized_key = normalize_secret_key_name(key);
                    if is_plaintext_secret_key(&normalized_key) {
                        let mut full_path = path.join(".");
                        if !full_path.is_empty() {
                            full_path.push('.');
                        }
                        full_path.push_str(key);
                        return Err(AppError::Validation(format!(
                            "{field} must not contain plaintext credential key `{full_path}`; store credential references instead"
                        )));
                    }
                    path.push(key.clone());
                    walk(child, field, path)?;
                    path.pop();
                }
            }
            Value::Array(items) => {
                for (index, child) in items.iter().enumerate() {
                    path.push(index.to_string());
                    walk(child, field, path)?;
                    path.pop();
                }
            }
            _ => {}
        }
        Ok(())
    }

    walk(value, field, &mut Vec::new())
}

fn is_plaintext_secret_key(key: &str) -> bool {
    matches!(
        key,
        "password"
            | "passwd"
            | "passphrase"
            | "private_key"
            | "ssh_private_key"
            | "secret"
            | "token"
            | "secret_key"
            | "client_secret"
            | "access_token"
            | "refresh_token"
            | "api_key"
            | "raw_token"
            | "token_value"
            | "token_hash"
            | "credential_password"
            | "credential_private_key"
            | "secret_access_key"
            | "resolved_credentials"
            | "rendered_env"
            | "proxy_payload"
            | "secret_material"
    ) || key.ends_with("_password")
        || key.ends_with("_private_key")
        || key.contains("_private_key_")
        || key.contains("_password_")
        || key.contains("_secret_")
        || key.contains("_token_")
        || key.ends_with("_secret")
        || key.ends_with("_token")
        || key.ends_with("_api_key")
}

fn normalize_secret_key_name(key: &str) -> String {
    let mut normalized = String::with_capacity(key.len());
    let mut previous_was_separator = true;
    let mut previous_was_lower_or_digit = false;

    for character in key.chars() {
        if character == '-' || character == ' ' || character == '.' {
            if !previous_was_separator && !normalized.ends_with('_') {
                normalized.push('_');
            }
            previous_was_separator = true;
            previous_was_lower_or_digit = false;
            continue;
        }

        if character.is_uppercase() && previous_was_lower_or_digit && !normalized.ends_with('_') {
            normalized.push('_');
        }

        for lower in character.to_lowercase() {
            normalized.push(lower);
        }
        previous_was_separator = false;
        previous_was_lower_or_digit = character.is_lowercase() || character.is_ascii_digit();
    }

    normalized.trim_matches('_').to_string()
}

fn normalize_content(value: String) -> Result<String, AppError> {
    let content = value.trim_end().to_string();
    if content.trim().is_empty() {
        return Err(AppError::Validation(
            "Playbook content is required".to_string(),
        ));
    }
    if content.len() > PLAYBOOK_CONTENT_MAX_BYTES {
        return Err(AppError::Validation(
            "Playbook content is too large".to_string(),
        ));
    }
    Ok(content)
}

fn normalize_version_timeout(value: Option<i32>) -> Result<i32, AppError> {
    let timeout = value.unwrap_or(PLAYBOOK_VERSION_DEFAULT_TIMEOUT_SECONDS);
    if !(1..=PLAYBOOK_VERSION_MAX_TIMEOUT_SECONDS).contains(&timeout) {
        return Err(AppError::Validation(
            "timeout_seconds must be between 1 and 86400".to_string(),
        ));
    }
    Ok(timeout)
}

fn normalize_manual_run_trigger_kind(
    trigger_kind: Option<PlaybookRunTriggerKind>,
) -> Result<PlaybookRunTriggerKind, AppError> {
    let trigger_kind = trigger_kind.unwrap_or(PlaybookRunTriggerKind::Manual);
    if matches!(trigger_kind, PlaybookRunTriggerKind::Schedule) {
        return Err(AppError::Validation(
            "Manual run endpoint cannot create scheduled run groups".to_string(),
        ));
    }
    Ok(trigger_kind)
}

fn normalize_scheduled_run_trigger_kind(
    trigger_kind: Option<PlaybookRunTriggerKind>,
) -> Result<PlaybookRunTriggerKind, AppError> {
    let trigger_kind = trigger_kind.unwrap_or(PlaybookRunTriggerKind::Schedule);
    if !matches!(trigger_kind, PlaybookRunTriggerKind::Schedule) {
        return Err(AppError::Validation(
            "Scheduled run groups must use schedule trigger_kind".to_string(),
        ));
    }
    Ok(trigger_kind)
}

fn normalize_required_text(
    value: String,
    field: &str,
    max_chars: usize,
) -> Result<String, AppError> {
    let normalized = value.trim().to_string();
    if normalized.is_empty() {
        return Err(AppError::Validation(format!("{field} is required")));
    }
    if normalized.chars().count() > max_chars {
        return Err(AppError::Validation(format!("{field} is too long")));
    }
    Ok(normalized)
}

fn normalize_lease_owner(value: String) -> Result<String, AppError> {
    let normalized = value.trim().to_string();
    if normalized.is_empty() {
        return Err(AppError::Validation("lease_owner is required".to_string()));
    }
    if normalized.chars().count() > PLAYBOOK_LEASE_OWNER_MAX_CHARS {
        return Err(AppError::Validation("lease_owner is too long".to_string()));
    }
    Ok(normalized)
}

fn normalize_terminal_run_status(status: PlaybookRunStatus) -> Result<PlaybookRunStatus, AppError> {
    if matches!(
        status,
        PlaybookRunStatus::Succeeded
            | PlaybookRunStatus::Failed
            | PlaybookRunStatus::Canceled
            | PlaybookRunStatus::TimedOut
    ) {
        Ok(status)
    } else {
        Err(AppError::Validation(
            "Run completion status must be terminal".to_string(),
        ))
    }
}

fn normalize_artifact_input(
    run_id: Uuid,
    input: CreateArtifactInput,
) -> Result<CreateArtifactRecord, AppError> {
    let artifact_type = normalize_required_text(
        input.artifact_type,
        "artifact_type",
        PLAYBOOK_ARTIFACT_TYPE_MAX_CHARS,
    )?;
    let name = normalize_required_text(input.name, "name", PLAYBOOK_ARTIFACT_NAME_MAX_CHARS)?;
    let content_type = input
        .content_type
        .map(|value| normalize_required_text(value, "content_type", 200))
        .transpose()?
        .unwrap_or_else(|| "application/json".to_string());
    let content_text = input
        .content_text
        .map(|value| {
            if value.is_empty() {
                return Err(AppError::Validation(
                    "content_text must not be empty".to_string(),
                ));
            }
            if value.len() > PLAYBOOK_ARTIFACT_TEXT_MAX_BYTES {
                Err(AppError::Validation(
                    "content_text is too large".to_string(),
                ))
            } else {
                Ok(value)
            }
        })
        .transpose()?;
    if input.content_json.is_some() && content_text.is_some() {
        return Err(AppError::Validation(
            "Artifact must contain either content_json or content_text, not both".to_string(),
        ));
    }
    if input.content_json.is_none() && content_text.is_none() {
        return Err(AppError::Validation(
            "Artifact content_json or content_text is required".to_string(),
        ));
    }
    let content_json = input.content_json;
    if content_json
        .as_ref()
        .map(|value| !value.is_object())
        .unwrap_or(false)
    {
        return Err(AppError::Validation(
            "content_json must be a JSON object".to_string(),
        ));
    }
    if let Some(content_json) = content_json.as_ref() {
        validate_no_plaintext_secret_keys(content_json, "content_json")?;
    }
    let hash_payload = if let Some(content_json) = content_json.as_ref() {
        serde_json::to_vec(content_json)?
    } else {
        content_text
            .as_deref()
            .unwrap_or_default()
            .as_bytes()
            .to_vec()
    };
    if hash_payload.len() > PLAYBOOK_ARTIFACT_TEXT_MAX_BYTES {
        return Err(AppError::Validation(
            "Artifact content is too large".to_string(),
        ));
    }
    let content_size_bytes = i64::try_from(hash_payload.len())
        .map_err(|_| AppError::Validation("Artifact content is too large".to_string()))?;
    Ok(CreateArtifactRecord {
        run_id,
        artifact_type,
        name,
        content_type,
        content_json,
        content_text,
        content_sha256: hex_sha256(&hash_payload),
        content_size_bytes,
    })
}

fn generate_token_secret(size: usize) -> Result<String, AppError> {
    let mut bytes = vec![0_u8; size];
    rand_bytes(&mut bytes)
        .map_err(|error| AppError::Internal(format!("Failed to generate token bytes: {error}")))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn hex_sha256(bytes: &[u8]) -> String {
    sha256(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn callback_token_auth(raw_token: &str, now: DateTime<Utc>) -> Result<CallbackTokenAuth, AppError> {
    let (token_id, _) =
        parse_job_token(raw_token).ok_or_else(|| AppError::Auth("TOKEN_INVALID".to_string()))?;
    Ok(CallbackTokenAuth {
        token_id,
        token_hash: hex_sha256(raw_token.as_bytes()),
        now,
    })
}

pub fn parse_job_token(value: &str) -> Option<(Uuid, &str)> {
    if value.trim() != value {
        return None;
    }
    let mut parts = value.splitn(3, '_');
    if parts.next()? != PLAYBOOK_JOB_TOKEN_PREFIX {
        return None;
    }
    let token_id = Uuid::parse_str(parts.next()?).ok()?;
    let secret = parts.next()?;
    if !is_url_safe_job_token_secret(secret) {
        return None;
    }
    Some((token_id, secret))
}

fn is_url_safe_job_token_secret(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn job_token_has_scope(scopes: &Value, required_scope: &str) -> bool {
    scopes
        .as_array()
        .map(|items| {
            items.iter().any(|item| {
                item.as_str()
                    .map(|scope| scope.eq_ignore_ascii_case(required_scope))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests;
