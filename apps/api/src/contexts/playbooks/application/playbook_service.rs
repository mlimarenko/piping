use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Duration, Utc};
use openssl::rand::rand_bytes;
use openssl::sha::sha256;
use piping_contracts::playbook_runner::{
    ConsoleAck, ConsolePage, MAX_CONSOLE_SEQUENCE, RunnerConsoleBatch, SnapshotContent,
    SnapshotEntry, snapshot_tree_sha256, validate_console_batch, validate_snapshot_entries,
};
use saffron::Cron;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use url::Url;
use uuid::Uuid;

use crate::config::constants::{
    PLAYBOOK_RUN_DEFAULT_TIMEOUT_SECONDS, PLAYBOOK_RUN_MAX_TIMEOUT_SECONDS,
};
use crate::contexts::access_control::domain::Principal;
use crate::contexts::audit::domain::{AuditContext, AuditCorrelation, AuditDecision};
use crate::contexts::playbooks::domain::{
    BuiltinCatalog, BuiltinJobLookup, BuiltinUnavailableReason, InventorySelection, JobTemplate,
    JobTokenAudience, LaunchOptions, LaunchSpec, builtin_definition_change,
    ParameterOverrides, PlaybookArtifact, PlaybookAssignment, PlaybookAssignmentScopeKind,
    PlaybookCancelOutcome, PlaybookExecutionEnvironment, PlaybookJobToken, PlaybookRun,
    PlaybookRunEvent, PlaybookRunEventKind, PlaybookRunGroup, PlaybookRunStatus,
    PlaybookRunTriggerKind, PlaybookSchedule, PlaybookScheduleConcurrencyPolicy,
    PlaybookScheduleMisfirePolicy, PlaybookServicePrincipalStatus, PlaybookTargetPreviewItem,
    PlaybookTargetSelectorPreview, PlaybookTargetSet, PlaybookTargetSetListItem,
    PlaybookTargetSetMode, PlaybookTargetSetPreview, Project, ProjectAuth, ProjectEntry,
    ProjectEntryContent, ProjectRevision, ProjectSource, ResolvedTarget, ResolvedVarsSchema, job_id,
    RevisionSelection, SCHEDULE_OWNER_ACCESS_REVOKED, ScheduleTargets, VarsSchemaSource,
    compile_vars_schema, resolve_parameters, resolve_vars_schema, validate_parameter_declarations,
};
use crate::contexts::playbooks::infrastructure::persistence::{
    AppendRunGroupEventBatchOutcome, CallbackTokenAuth, CompleteRunBatchItemRecord,
    CompleteRunGroupBatchRecord, CompleteRunRecord, CreateArtifactRecord, CreateAssignmentRecord,
    CreateExecutionEnvironmentRecord, CreateJobTemplateRecord, CreateJobTokenRecord,
    CreateProjectRecord, CreateRunEventRecord, CreateRunGroupCallbackTokenRecord,
    CreateRunGroupEventRecord, CreateRunGroupRecord, CreateScheduleRecord, CreateTargetSetRecord,
    DueScheduleRecord, FinalizeRunGroupCancelRecord, FinalizedRunGroupCancelRecord,
    LeaseQueuedRunsRecord, LeaseScheduleOccurrencesRecord, LeasedScheduleOccurrenceRecord,
    PlaybookAssignmentListFilter, PlaybookRepository, PlaybookRunListFilter,
    PlaybookScheduleListFilter, PlaybookTargetSetListFilter, ReleaseScheduleOccurrenceRecord,
    RunGroupWithRuns, ScheduleOccurrenceAdvanceRecord, SetActiveRunTimeoutsRecord,
    StartLeasedRunsRecord, StaticTargetMemberRecord, TimeoutStuckRunsRecord,
    UpdateAssignmentRecord, UpdateScheduleRecord, UpdateTargetSetRecord,
};
use crate::contexts::shared::pagination::PageResult;
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;
use crate::observability::metrics;

const PLAYBOOK_NAME_MAX_CHARS: usize = 200;
const PLAYBOOK_TEMPLATE_NAME_MAX_CHARS: usize = 200;
const PLAYBOOK_DESCRIPTION_MAX_CHARS: usize = 4_000;
const PLAYBOOK_ENTRYPOINT_MAX_CHARS: usize = 240;
const PLAYBOOK_CONTAINER_IMAGE_MAX_CHARS: usize = 512;
const PLAYBOOK_RUNNER_GROUP_MAX_CHARS: usize = 120;
const PLAYBOOK_RUN_MAX_TARGET_LIMIT: i64 = 50_000;
/// Upper bound of one target preview page, shared by the saved-set preview and
/// the unsaved-selector preview so both report the same truncation point.
const PLAYBOOK_TARGET_PREVIEW_MAX_LIMIT: i64 = 500;
const PLAYBOOK_ARTIFACT_NAME_MAX_CHARS: usize = 240;
const PLAYBOOK_ARTIFACT_TYPE_MAX_CHARS: usize = 120;
const PLAYBOOK_ARTIFACT_TEXT_MAX_BYTES: usize = 2_097_152;
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
const PLAYBOOK_JOB_TOKEN_MAX_SCOPES: usize = 16;
const PLAYBOOK_RUNTIME_SECRET_ENV_NAME_MAX_CHARS: usize = 128;
const PLAYBOOK_SCHEDULE_CRON_MAX_CHARS: usize = 200;
const PLAYBOOK_SCHEDULE_TIMEZONE_MAX_CHARS: usize = 120;
const PLAYBOOK_JOB_TOKEN_ALLOWED_SCOPES: &[&str] = &[
    "artifact:create",
    "event:append",
    "status:update",
    "inventory:read",
    "inventory:create",
    "inventory:update",
];

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
    pub source: ProjectSource,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateJobTemplateInput {
    pub template: JobTemplate,
}

/// Validate the execution inputs of a workspace template before storing or
/// preparing it. The runner still validates the materialized tree independently.
pub fn validate_workspace_job_template(template: &JobTemplate) -> Result<(), AppError> {
    let validate_path = |path: &str, field: &str| {
        validate_snapshot_entries(&[SnapshotEntry {
            path: path.to_string(),
            content: SnapshotContent::File {
                bytes: Vec::new(),
                executable: false,
            },
        }])
        .map_err(|_| {
            AppError::Validation(format!("{field} must be a relative path without traversal"))
        })
    };

    if template.name.trim().is_empty() {
        return Err(AppError::Validation(
            "Job template name is required".to_string(),
        ));
    }
    if template.workdir != "." {
        validate_path(&template.workdir, "workdir")?;
    }
    if template.playbook_paths.is_empty() {
        return Err(AppError::Validation(
            "At least one playbook path is required".to_string(),
        ));
    }
    for path in &template.playbook_paths {
        validate_path(path, "playbook")?;
    }
    if let InventorySelection::Native { sources, .. } = &template.inventory
        && sources
            .iter()
            .any(|source| source.trim().is_empty() || source.contains('\0'))
    {
        return Err(AppError::Validation(
            "Native inventory sources must not be empty".to_string(),
        ));
    }
    if template.options.timeout_seconds == 0
        || template.options.timeout_seconds > PLAYBOOK_RUN_MAX_TIMEOUT_SECONDS as u32
    {
        return Err(AppError::Validation(
            "Job timeout is out of range".to_string(),
        ));
    }
    if !template.options.extra_vars.is_object() {
        return Err(AppError::Validation(
            "extra_vars must be an object".to_string(),
        ));
    }
    piping_contracts::ansible_options::parse_ansible_options(&template.options.argv)
        .map_err(|error| AppError::Validation(error.to_string()))?;
    if template
        .options
        .limit
        .as_ref()
        .is_some_and(|limit| limit.contains('\0'))
    {
        return Err(AppError::Validation(
            "Launch arguments must not contain NUL".to_string(),
        ));
    }
    for (name, value) in &template.options.env {
        validate_runtime_secret_env_name(name)?;
        if value.contains('\0') || template.options.secret_refs.contains_key(name) {
            return Err(AppError::Validation(
                "Environment values must not contain NUL or shadow a secret reference".to_string(),
            ));
        }
    }
    for name in template.options.secret_refs.keys() {
        validate_runtime_secret_env_name(name)?;
    }
    match &template.vars_schema {
        VarsSchemaSource::Inline { schema } => {
            compile_vars_schema(schema)?;
        }
        VarsSchemaSource::None => {
            if template
                .options
                .extra_vars
                .as_object()
                .is_some_and(|values| !values.is_empty())
            {
                return Err(AppError::Validation(
                    "extra_vars require a declared vars_schema".to_string(),
                ));
            }
        }
        VarsSchemaSource::ProjectFile { .. } => {}
    }
    Ok(())
}

pub(crate) fn split_pinned_image(value: &str) -> Result<(String, String), AppError> {
    let invalid = || {
        AppError::Validation(
            "Execution image must be a repository pinned by a lowercase sha256 digest".into(),
        )
    };
    let (repository, digest) = value.split_once("@sha256:").ok_or_else(invalid)?;
    let parts: Vec<&str> = repository.split('/').collect();
    if repository.is_empty()
        || repository.len() > PLAYBOOK_CONTAINER_IMAGE_MAX_CHARS
        || parts.iter().enumerate().any(|(index, part)| {
            let name = if let Some((host, port)) = part.split_once(':') {
                if index != 0
                    || parts.len() < 2
                    || port.is_empty()
                    || !port.bytes().all(|b| b.is_ascii_digit())
                {
                    return true;
                }
                host
            } else {
                part
            };
            name.is_empty()
                || !name.as_bytes()[0].is_ascii_lowercase() && !name.as_bytes()[0].is_ascii_digit()
                || !name.as_bytes()[name.len() - 1].is_ascii_lowercase()
                    && !name.as_bytes()[name.len() - 1].is_ascii_digit()
                || !name.bytes().all(|b| {
                    b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
                })
                || name.contains("..")
        })
        || digest.len() != 64
        || !digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid());
    }
    Ok((repository.to_string(), format!("sha256:{digest}")))
}

fn validate_schedule_targets(targets: &ScheduleTargets) -> Result<(), AppError> {
    match targets {
        ScheduleTargets::Fixed {
            targets,
            target_set_id: _,
        } => {
            if targets.len() > PLAYBOOK_RUN_MAX_TARGET_LIMIT as usize
                || targets.iter().any(|target| {
                    target.target_id.is_none()
                        || target.source_id.is_none()
                        || target.hostname.trim().is_empty()
                        || target.hostname.contains('\0')
                })
            {
                return Err(AppError::Validation(
                    "Fixed schedule targets must have concrete identities and hostnames".into(),
                ));
            }
        }
        ScheduleTargets::Dynamic {
            selector,
            target_set_id: _,
        } => {
            normalize_selector(Some(selector.clone()))?;
        }
    }
    Ok(())
}

/// Server-owned `metadata` key carrying the W3C `traceparent` of the launching request.
pub(crate) const LAUNCH_TRACEPARENT_KEY: &str = "traceparent";

fn validate_launch_metadata(value: Value) -> Result<Value, AppError> {
    let metadata = validate_metadata(value)?;
    if ["audit_context", "audit_actor_kind", LAUNCH_TRACEPARENT_KEY]
        .iter()
        .any(|key| metadata.get(key).is_some())
    {
        return Err(AppError::Validation(
            "Launch audit identity is server-owned".into(),
        ));
    }
    if metadata.get("launch_options").is_some() {
        return Err(AppError::Validation(
            "Launch metadata cannot override template options".into(),
        ));
    }
    Ok(metadata)
}

fn stamp_launch_audit_metadata(mut metadata: Value, context: Option<&RequestContext>) -> Value {
    metadata["audit_context"] = json!(AuditContext {
        decision: Some(AuditDecision::Allowed),
        correlation: AuditCorrelation {
            request_id: context.and_then(|context| Uuid::parse_str(&context.correlation_id).ok()),
            ..Default::default()
        },
        ..Default::default()
    });
    if let Some(context) = context {
        metadata["audit_actor_kind"] = json!(context.audit_actor_kind());
    }
    // Lets the background worker continue the launching request's trace.
    if let Some(traceparent) = crate::observability::current_traceparent() {
        metadata[LAUNCH_TRACEPARENT_KEY] = json!(traceparent);
    }
    metadata
}

fn narrow_manual_targets(
    spec: &mut LaunchSpec,
    target_ids: Option<&[Uuid]>,
) -> Result<(), AppError> {
    let Some(target_ids) = target_ids else {
        return Ok(());
    };
    if !matches!(spec.inventory, InventorySelection::Managed { .. }) {
        return Err(AppError::Validation(
            "Manual target selection requires managed inventory".into(),
        ));
    }
    if target_ids.is_empty() {
        return Err(AppError::Validation(
            "Manual target selection must not be empty".into(),
        ));
    }
    let selected: HashSet<_> = target_ids.iter().copied().collect();
    if selected.len() != target_ids.len() || selected.contains(&Uuid::nil()) {
        return Err(AppError::Validation(
            "Manual target selection must contain unique, valid target IDs".into(),
        ));
    }
    let resolved: HashSet<_> = spec
        .targets
        .iter()
        .filter_map(|target| target.target_id)
        .collect();
    if !selected.is_subset(&resolved) {
        return Err(AppError::Validation(
            "Selected target is outside the resolved template target set".into(),
        ));
    }
    // Keep the already resolved identities, source attribution and ordering. No
    // second lookup may expand the authorized selection before persistence.
    spec.targets
        .retain(|target| target.target_id.is_some_and(|id| selected.contains(&id)));
    Ok(())
}

fn validate_revision_changes(
    changes: &[crate::contexts::playbooks::domain::RevisionChange],
) -> Result<(), AppError> {
    use crate::contexts::playbooks::domain::{
        ProjectEntry, ProjectEntryContent, RevisionChange, validate_project_entry,
    };
    let validate_path = |path: &str| {
        validate_project_entry(&ProjectEntry {
            path: path.to_string(),
            content: ProjectEntryContent::File {
                bytes: Vec::new(),
                executable: false,
            },
        })
    };
    if changes.is_empty() {
        return Err(AppError::Validation(
            "Revision changes must not be empty".into(),
        ));
    }
    for change in changes {
        match change {
            RevisionChange::Put { entry } => validate_project_entry(entry)?,
            RevisionChange::Delete { path } => validate_path(path)?,
            RevisionChange::Rename { from, to } => {
                validate_path(from)?;
                validate_path(to)?;
            }
        }
    }
    Ok(())
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

/// Partial update for a playbook project. The source kind cannot change in
/// place; fork the project to move between editable and Git-backed storage.
#[derive(Debug, Clone, Deserialize)]
pub struct UpdateProjectInput {
    pub name: Option<String>,
    pub description: Option<Option<String>>,
    pub source: Option<ProjectSource>,
}

/// Partial update for a job template. Absent fields keep the stored value; the
/// path identifier always wins over any identifier carried by the body.
#[derive(Debug, Clone, Deserialize)]
pub struct UpdateJobTemplateInput {
    pub name: Option<String>,
    pub project_id: Option<Uuid>,
    pub workdir: Option<String>,
    pub playbook_paths: Option<Vec<String>>,
    pub revision: Option<RevisionSelection>,
    pub inventory: Option<InventorySelection>,
    pub vars_schema: Option<VarsSchemaSource>,
    pub options: Option<LaunchOptions>,
    pub execution_environment_id: Option<Uuid>,
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

/// Persisted result of an update plus the audit-relevant deltas. Only selector
/// key names are exposed, never selector values, so an audit event cannot leak
/// a secret-looking value.
#[derive(Debug, Clone)]
pub struct TargetSetUpdateOutcome {
    pub target_set: PlaybookTargetSet,
    pub changed_selector_keys: Vec<String>,
    pub mode_changed: bool,
    pub name_changed: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateAssignmentInput {
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

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateAssignmentInput {
    pub job_template_id: Option<Uuid>,
    pub target_set_id: Option<Uuid>,
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
#[serde(deny_unknown_fields)]
pub struct LaunchJobTemplateInput {
    pub metadata: Option<Value>,
    pub target_ids: Option<Vec<Uuid>>,
    /// Run-level parameter values; they override the template's job layer.
    pub overrides: Option<ParameterOverrides>,
}

/// Effective parameter declaration exposed to clients: the single provenance
/// plus the resolved JSON Schema and its revision. `schema` is absent only when
/// the template declares no parameters.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct JobTemplateVarsSchema {
    pub source: VarsSchemaSource,
    pub schema: Option<Value>,
    pub revision: Option<String>,
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
#[serde(deny_unknown_fields)]
pub struct CreateScheduleInput {
    pub template_id: Uuid,
    pub targets: ScheduleTargets,
    /// Run-level parameter values applied at every fire.
    pub overrides: Option<ParameterOverrides>,
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
#[serde(deny_unknown_fields)]
pub struct UpdateScheduleInput {
    pub targets: Option<ScheduleTargets>,
    /// Replace the stored run overrides when present.
    pub overrides: Option<ParameterOverrides>,
    pub name: Option<String>,
    pub cron_expression: Option<String>,
    pub timezone: Option<String>,
    pub misfire_policy: Option<PlaybookScheduleMisfirePolicy>,
    pub concurrency_policy: Option<PlaybookScheduleConcurrencyPolicy>,
    pub enabled: Option<bool>,
    pub next_run_at: Option<DateTime<Utc>>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct CreateJobTokenInput {
    pub run_id: Uuid,
    pub expires_in_seconds: Option<i64>,
    pub scopes: Option<Vec<String>>,
    /// The surface this token is minted for. Required: a token must never be
    /// minted with an implicit audience that could be replayed on another one.
    pub audience: JobTokenAudience,
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
pub struct SetActiveRunTimeoutsInput {
    pub run_ids: Vec<Uuid>,
    pub lease_owner: String,
    pub timeout_seconds: i32,
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
    pub dispatch_id: Uuid,
    pub dispatch_attempt: i32,
    pub console_next_sequence: u64,
    pub lease_owner: String,
    pub group_status: PlaybookRunStatus,
    pub exit_code: Option<i32>,
    pub error_message: Option<String>,
    pub runs: Vec<CompleteRunGroupBatchItemInput>,
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
pub struct PlaybookTargetAssignment {
    pub assignment: PlaybookAssignment,
    pub job_template: JobTemplate,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct PlaybookTargetAssignments {
    pub target: PlaybookTargetPreviewItem,
    pub items: Vec<PlaybookTargetAssignment>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct PlaybookRunGroupCreated {
    pub run_group: PlaybookRunGroup,
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
    pub job_token: String,
}

#[derive(Debug, Clone)]
pub struct PlaybookRunGroupExecution {
    pub run_group: PlaybookRunGroup,
    pub executions: Vec<PlaybookRunExecution>,
    pub group_callback_token: String,
    /// Raw sandbox writeback tokens keyed by run ID, minted with the supervisor
    /// tokens in the same start transaction and delivered to the job only.
    pub sandbox_run_tokens: BTreeMap<Uuid, String>,
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
    launch_authorizer: Option<Arc<dyn PlaybookLaunchAuthorizer>>,
    builtin_catalog: BuiltinCatalog,
    /// Explicitly configured execution environment for builtin jobs; unset means
    /// builtin jobs stay unavailable.
    builtin_environment: Option<Uuid>,
}

/// Server-authored authorization for the immutable launch spec.
#[derive(Debug, Clone, Default)]
pub struct LaunchAuthorizationSnapshot {
    pub secret_env: Option<Value>,
    pub ssh_record_contexts: Vec<Uuid>,
}

/// Authorizes the final, immutable launch snapshot before it is persisted.
#[async_trait]
pub trait PlaybookLaunchAuthorizer: Send + Sync {
    async fn validate_schedule_owner(&self, user_id: Uuid) -> Result<(), AppError>;

    async fn authorize_launch(
        &self,
        principal: &Principal,
        request_context: Option<&RequestContext>,
        spec: &LaunchSpec,
    ) -> Result<LaunchAuthorizationSnapshot, AppError>;
}

impl<R> PlaybookService<R>
where
    R: PlaybookRepository,
{
    pub fn new(repo: Arc<R>) -> Self {
        Self {
            repo,
            launch_authorizer: None,
            builtin_catalog: BuiltinCatalog::unavailable(
                "the bundled playbook catalog has not been loaded",
            ),
            builtin_environment: None,
        }
    }

    pub fn with_launch_authorizer(mut self, authorizer: Arc<dyn PlaybookLaunchAuthorizer>) -> Self {
        self.launch_authorizer = Some(authorizer);
        self
    }

    /// Install the catalog state produced by the startup loader and the single
    /// explicitly enabled execution environment for builtin jobs.
    pub fn with_builtin_catalog(
        mut self,
        builtin_catalog: BuiltinCatalog,
        execution_environment_id: Option<Uuid>,
    ) -> Self {
        self.builtin_catalog = builtin_catalog;
        self.builtin_environment = execution_environment_id;
        self
    }

    /// Canonical builtin lookup by catalog slug.
    ///
    /// Authorization stays with the existing job-template read gate at the API
    /// boundary. Availability is reported from the persisted job template and
    /// the explicitly configured execution environment; nothing here falls back
    /// to another environment or invents a launchable definition.
    pub async fn resolve_builtin_job(&self, slug: &str) -> Result<BuiltinJobLookup, AppError> {
        let catalog = match self.builtin_catalog.loaded() {
            Some(catalog) => catalog,
            None => {
                return Ok(BuiltinJobLookup {
                    slug: slug.to_string(),
                    template_id: None,
                    available: false,
                    unavailable_reason: Some(BuiltinUnavailableReason::CatalogNotLoaded),
                });
            }
        };
        let template = catalog.template(slug).ok_or_else(|| {
            AppError::NotFound(format!("builtin playbook catalog has no entry `{slug}`"))
        })?;
        // The execution environment is instance-owned, so an existing job row is
        // looked up by its deterministic identity before any config-only branch:
        // removing the configured default must not hide a seeded, enabled job.
        let template_id = template.template_id.unwrap_or_else(|| job_id(slug));
        let unavailable = |reason| BuiltinJobLookup {
            slug: slug.to_string(),
            template_id: Some(template_id),
            available: false,
            unavailable_reason: Some(reason),
        };
        let Some(row) = self.repo.get_job_template(template_id).await? else {
            return Ok(BuiltinJobLookup {
                slug: slug.to_string(),
                template_id: None,
                available: false,
                unavailable_reason: Some(
                    if self.configured_execution_environment().is_none() {
                        BuiltinUnavailableReason::ExecutionEnvironmentNotConfigured
                    } else {
                        BuiltinUnavailableReason::TemplateNotSeeded
                    },
                ),
            });
        };
        // Availability follows the persisted row, because the execution
        // environment is an instance setting an operator may retarget. The
        // configured default only seeds a new row.
        let environment = self
            .repo
            .get_execution_environment(row.execution_environment_id)
            .await?;
        if !environment.is_some_and(|environment| environment.enabled) {
            return Ok(unavailable(
                BuiltinUnavailableReason::ExecutionEnvironmentUnavailable,
            ));
        }
        let targets_configured = match &row.inventory {
            InventorySelection::Native { sources, .. } => !sources.is_empty(),
            InventorySelection::Managed { .. } => true,
        };
        if !targets_configured {
            return Ok(unavailable(BuiltinUnavailableReason::TargetsNotConfigured));
        }
        match self.validate_builtin_revision(&row).await {
            Ok(()) => {}
            Err(AppError::NotFound(_) | AppError::Validation(_)) => {
                return Ok(unavailable(BuiltinUnavailableReason::RevisionUnavailable));
            }
            Err(error) => return Err(error),
        }
        Ok(BuiltinJobLookup {
            slug: slug.to_string(),
            template_id: Some(row.id),
            available: true,
            unavailable_reason: None,
        })
    }

    /// The single explicitly enabled execution environment for builtin jobs.
    fn configured_execution_environment(&self) -> Option<Uuid> {
        self.builtin_environment
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
    ) -> Result<PageResult<Project>, AppError> {
        self.repo.list_projects(limit, offset).await
    }

    pub async fn get_project(&self, id: Uuid) -> Result<Option<Project>, AppError> {
        self.repo.get_project(id).await
    }

    pub async fn get_project_revision(
        &self,
        project_id: Uuid,
        revision_id: Uuid,
    ) -> Result<Option<ProjectRevision>, AppError> {
        self.repo
            .get_project_revision(project_id, revision_id)
            .await
    }

    pub async fn import_project_revision(
        &self,
        project_id: Uuid,
        base_revision_id: Option<Uuid>,
        expected_updated_at: DateTime<Utc>,
        source_commit_oid: String,
        entries: Vec<ProjectEntry>,
        user_id: Option<Uuid>,
    ) -> Result<ProjectRevision, AppError> {
        self.ensure_project_mutable(project_id).await?;
        if !matches!(source_commit_oid.len(), 40 | 64)
            || !source_commit_oid
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(AppError::Validation("Invalid Git commit object ID".into()));
        }
        let snapshot: Vec<SnapshotEntry> = entries
            .iter()
            .map(|entry| SnapshotEntry {
                path: entry.path.clone(),
                content: match &entry.content {
                    ProjectEntryContent::File { bytes, executable } => SnapshotContent::File {
                        bytes: bytes.clone(),
                        executable: *executable,
                    },
                    ProjectEntryContent::Symlink { target } => SnapshotContent::Symlink {
                        target: target.clone(),
                    },
                },
            })
            .collect();
        validate_snapshot_entries(&snapshot).map_err(AppError::Validation)?;
        self.validate_project_revision_parameters(project_id, &entries)
            .await?;
        self.repo
            .import_project_revision(
                project_id,
                base_revision_id,
                expected_updated_at,
                source_commit_oid,
                entries,
                user_id,
            )
            .await
    }

    pub async fn change_project_revision(
        &self,
        project_id: Uuid,
        base_revision_id: Uuid,
        changes: Vec<crate::contexts::playbooks::domain::RevisionChange>,
        user_id: Option<Uuid>,
    ) -> Result<ProjectRevision, AppError> {
        self.ensure_project_mutable(project_id).await?;
        validate_revision_changes(&changes)?;
        self.repo
            .change_project_revision(project_id, base_revision_id, changes, user_id)
            .await
    }

    pub async fn fork_project_revision(
        &self,
        project_id: Uuid,
        revision_id: Uuid,
        name: String,
        user_id: Option<Uuid>,
    ) -> Result<Project, AppError> {
        self.ensure_project_mutable(project_id).await?;
        let name = normalize_required_text(name, "name", PLAYBOOK_TEMPLATE_NAME_MAX_CHARS)?;
        self.repo
            .fork_project(project_id, revision_id, name, user_id)
            .await
    }

    pub async fn create_project(
        &self,
        input: CreateProjectInput,
        user_id: Option<Uuid>,
    ) -> Result<Project, AppError> {
        let record = CreateProjectRecord {
            name: normalize_required_text(input.name, "name", PLAYBOOK_TEMPLATE_NAME_MAX_CHARS)?,
            description: normalize_optional_text(
                input.description,
                PLAYBOOK_DESCRIPTION_MAX_CHARS,
            )?,
            source: normalize_project_source(input.source)?,
            user_id,
        };
        self.repo.create_project(record).await
    }

    pub async fn update_project(
        &self,
        id: Uuid,
        input: UpdateProjectInput,
        user_id: Option<Uuid>,
    ) -> Result<Option<Project>, AppError> {
        let Some(existing) = self.repo.get_project(id).await? else {
            return Ok(None);
        };
        if matches!(existing.source, ProjectSource::System { .. }) {
            return Err(AppError::Validation(
                "system project is read-only".to_string(),
            ));
        }
        let source = match input.source {
            Some(source) => {
                let source = normalize_project_source(source)?;
                if !same_project_source_kind(&existing.source, &source) {
                    return Err(AppError::Validation(
                        "project source kind cannot change; fork the project instead".to_string(),
                    ));
                }
                source
            }
            None => existing.source,
        };
        let description = match input.description {
            Some(value) => value,
            None => existing.description,
        };
        let record = CreateProjectRecord {
            name: normalize_required_text(
                input.name.unwrap_or(existing.name),
                "name",
                PLAYBOOK_TEMPLATE_NAME_MAX_CHARS,
            )?,
            description: normalize_optional_text(description, PLAYBOOK_DESCRIPTION_MAX_CHARS)?,
            source,
            user_id,
        };
        self.repo.update_project(id, record).await
    }

    pub async fn list_job_templates(
        &self,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<JobTemplate>, AppError> {
        self.repo.list_job_templates(limit, offset).await
    }

    pub async fn get_job_template(&self, id: Uuid) -> Result<Option<JobTemplate>, AppError> {
        self.repo.get_job_template(id).await
    }

    /// Resolve the revision the template pins and verify its tree digest. Both
    /// parameter-file resolution and launch preparation use this one path, so a
    /// schema can never come from a different revision than the playbooks.
    async fn resolve_template_revision(
        &self,
        template: &JobTemplate,
    ) -> Result<ProjectRevision, AppError> {
        let project = self
            .repo
            .get_project(template.project_id)
            .await?
            .ok_or_else(|| AppError::NotFound("Project not found".into()))?;
        if project.id != template.project_id {
            return Err(AppError::Validation("Project identity mismatch".into()));
        }
        let revision_id = match template.revision {
            RevisionSelection::Current => project
                .current_revision_id
                .ok_or_else(|| AppError::Validation("Project has no current revision".into()))?,
            RevisionSelection::Fixed { revision_id } => revision_id,
        };
        let revision = self
            .repo
            .get_project_revision(template.project_id, revision_id)
            .await?
            .ok_or_else(|| AppError::NotFound("Project revision not found".into()))?;
        if revision.project_id != template.project_id || revision.revision_id != revision_id {
            return Err(AppError::Validation(
                "Project revision identity mismatch".into(),
            ));
        }
        let entries: Vec<SnapshotEntry> = revision
            .entries
            .iter()
            .map(|entry| SnapshotEntry {
                path: entry.path.clone(),
                content: match &entry.content {
                    ProjectEntryContent::File { bytes, executable } => SnapshotContent::File {
                        bytes: bytes.clone(),
                        executable: *executable,
                    },
                    ProjectEntryContent::Symlink { target } => SnapshotContent::Symlink {
                        target: target.clone(),
                    },
                },
            })
            .collect();
        validate_snapshot_entries(&entries).map_err(AppError::Validation)?;
        let tree_sha256 = snapshot_tree_sha256(&entries).map_err(AppError::Validation)?;
        if tree_sha256 != revision.tree_sha256 {
            return Err(AppError::Validation(
                "Project revision tree digest mismatch".into(),
            ));
        }
        Ok(revision)
    }

    /// Check the selected snapshot, not the current catalog tree. A fixed
    /// revision cannot silently fall back when its entry or schema is missing.
    async fn validate_builtin_revision(&self, template: &JobTemplate) -> Result<(), AppError> {
        let revision =
            self.resolve_template_revision(template)
                .await
                .map_err(|error| match error {
                    AppError::NotFound(_) => {
                        AppError::Validation("builtin revision not found in its project".into())
                    }
                    other => other,
                })?;
        Self::validate_builtin_snapshot(template, &revision)
    }

    fn validate_builtin_snapshot(
        template: &JobTemplate,
        revision: &ProjectRevision,
    ) -> Result<(), AppError> {
        for path in &template.playbook_paths {
            if !revision.entries.iter().any(|entry| {
                entry.path == *path && matches!(entry.content, ProjectEntryContent::File { .. })
            }) {
                return Err(AppError::Validation(
                    "builtin entrypoint is absent from selected revision".into(),
                ));
            }
        }
        let schema = resolve_vars_schema(&template.vars_schema, &revision.entries)?;
        validate_parameter_declarations(
            schema.as_ref(),
            &template.options.extra_vars,
            &json!({}),
            &template.options.secret_refs,
        )?;
        Ok(())
    }

    /// Resolve the template's single declared schema provenance.
    async fn resolve_template_vars_schema(
        &self,
        template: &JobTemplate,
    ) -> Result<Option<ResolvedVarsSchema>, AppError> {
        match &template.vars_schema {
            VarsSchemaSource::Inline { schema } => compile_vars_schema(schema).map(Some),
            VarsSchemaSource::None => Ok(None),
            VarsSchemaSource::ProjectFile { .. } => {
                let revision = self.resolve_template_revision(template).await?;
                resolve_vars_schema(&template.vars_schema, &revision.entries)
            }
        }
    }

    /// Validate the values fixed on a template. Values a launch will supply
    /// later stay optional here, but unknown, reserved, mistyped and
    /// plaintext-secret values are rejected before the template is stored.
    async fn validate_job_template_parameters(
        &self,
        template: &JobTemplate,
    ) -> Result<(), AppError> {
        let schema = self.resolve_template_vars_schema(template).await?;
        validate_parameter_declarations(
            schema.as_ref(),
            &template.options.extra_vars,
            &json!({}),
            &template.options.secret_refs,
        )?;
        Ok(())
    }

    /// Reject a candidate Git revision that breaks a template pinned to
    /// `Current`: its schema file must still exist, parse and compile, and the
    /// template's job layer must still satisfy it. Nothing is rewritten.
    async fn validate_project_revision_parameters(
        &self,
        project_id: Uuid,
        entries: &[ProjectEntry],
    ) -> Result<(), AppError> {
        for template in self.repo.list_job_templates_for_project(project_id).await? {
            if !matches!(template.revision, RevisionSelection::Current) {
                continue;
            }
            let schema = resolve_vars_schema(&template.vars_schema, entries)?;
            validate_parameter_declarations(
                schema.as_ref(),
                &template.options.extra_vars,
                &json!({}),
                &template.options.secret_refs,
            )?;
        }
        Ok(())
    }

    /// Effective schema and revision for the launch form and parameter editor.
    pub async fn get_job_template_vars_schema(
        &self,
        id: Uuid,
    ) -> Result<Option<JobTemplateVarsSchema>, AppError> {
        let Some(template) = self.repo.get_job_template(id).await? else {
            return Ok(None);
        };
        let schema = self.resolve_template_vars_schema(&template).await?;
        Ok(Some(JobTemplateVarsSchema {
            source: template.vars_schema,
            schema: schema.as_ref().map(|schema| schema.schema.clone()),
            revision: schema.map(|schema| schema.revision),
        }))
    }

    pub async fn create_job_template(
        &self,
        input: CreateJobTemplateInput,
        user_id: Option<Uuid>,
    ) -> Result<JobTemplate, AppError> {
        let mut template = input.template;
        template.id = Uuid::now_v7();
        template.name =
            normalize_required_text(template.name, "name", PLAYBOOK_TEMPLATE_NAME_MAX_CHARS)?;
        self.ensure_project_mutable(template.project_id).await?;
        validate_workspace_job_template(&template)?;
        self.validate_job_template_parameters(&template).await?;
        self.repo
            .create_job_template(CreateJobTemplateRecord { template, user_id })
            .await
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

    pub async fn update_job_template(
        &self,
        id: Uuid,
        input: UpdateJobTemplateInput,
        user_id: Option<Uuid>,
    ) -> Result<Option<JobTemplate>, AppError> {
        let Some(existing) = self.repo.get_job_template(id).await? else {
            return Ok(None);
        };
        let mut template = existing.clone();
        if let Some(name) = input.name {
            template.name = name;
        }
        if let Some(project_id) = input.project_id {
            template.project_id = project_id;
        }
        if let Some(workdir) = input.workdir {
            template.workdir = workdir;
        }
        if let Some(playbook_paths) = input.playbook_paths {
            template.playbook_paths = playbook_paths;
        }
        if let Some(revision) = input.revision {
            template.revision = revision;
        }
        if let Some(inventory) = input.inventory {
            template.inventory = inventory;
        }
        if let Some(vars_schema) = input.vars_schema {
            template.vars_schema = vars_schema;
        }
        if let Some(options) = input.options {
            template.options = options;
        }
        if let Some(execution_environment_id) = input.execution_environment_id {
            template.execution_environment_id = execution_environment_id;
        }
        template.name =
            normalize_required_text(template.name, "name", PLAYBOOK_TEMPLATE_NAME_MAX_CHARS)?;
        self.ensure_builtin_definition_unchanged(&existing, &template)
            .await?;
        validate_workspace_job_template(&template)?;
        if self
            .repo
            .get_project(template.project_id)
            .await?
            .is_some_and(|project| matches!(project.source, ProjectSource::System { .. }))
        {
            self.validate_builtin_revision(&template).await?;
        }
        self.validate_job_template_parameters(&template).await?;
        self.repo
            .update_job_template(id, CreateJobTemplateRecord { template, user_id })
            .await
    }

    /// A builtin job's definition comes from the shipped catalog; an instance
    /// may still configure its own targets, execution environment, credential
    /// bindings, parameter overrides, runtime environment and run timeout.
    ///
    /// The guard covers the boundary in both directions: a builtin job must not
    /// leave its project, and a user job must not enter the catalog project.
    /// Taking the persisted row before any early return is what makes an
    /// ordinary user job unable to PATCH itself into the catalog project and
    /// become an undeletable System row.
    async fn ensure_builtin_definition_unchanged(
        &self,
        existing: &JobTemplate,
        template: &JobTemplate,
    ) -> Result<(), AppError> {
        let persisted_system = self
            .repo
            .get_project(existing.project_id)
            .await?
            .is_some_and(|project| matches!(project.source, ProjectSource::System { .. }));
        let proposed_system = if template.project_id == existing.project_id {
            persisted_system
        } else {
            self.repo
                .get_project(template.project_id)
                .await?
                .is_some_and(|project| matches!(project.source, ProjectSource::System { .. }))
        };
        if !persisted_system && !proposed_system {
            return Ok(());
        }
        if let Some(field) = builtin_definition_change(existing, template) {
            return Err(AppError::Validation(format!(
                "builtin job definition is read-only ({}); only its revision selection, targets, execution environment, parameters, environment values and timeout can be configured",
                field.as_str()
            )));
        }
        Ok(())
    }

    /// A builtin job definition is read-only, so its row cannot be deleted
    /// through the public API; a catalog update replaces the definition instead.
    pub async fn delete_job_template(&self, id: Uuid) -> Result<bool, AppError> {
        let Some(template) = self.repo.get_job_template(id).await? else {
            return Ok(false);
        };
        if let Some(project) = self.repo.get_project(template.project_id).await?
            && matches!(project.source, ProjectSource::System { .. })
        {
            return Err(AppError::Validation(
                "builtin job definition is read-only".to_string(),
            ));
        }
        self.repo.delete_job_template(id).await
    }

    /// Refuses to delete a project that still owns job templates; the FK is
    /// `ON DELETE CASCADE`, so without this guard the delete would silently take
    /// every dependent job template with it.
    pub async fn delete_project(&self, id: Uuid) -> Result<bool, AppError> {
        self.ensure_project_mutable(id).await?;
        if self.repo.count_job_templates_for_project(id).await? > 0 {
            return Err(AppError::Conflict(
                "project still has job templates; delete or retarget them first".to_string(),
            ));
        }
        self.repo.delete_project(id).await
    }

    async fn ensure_project_mutable(&self, project_id: Uuid) -> Result<(), AppError> {
        let Some(project) = self.repo.get_project(project_id).await? else {
            return Ok(());
        };
        if matches!(project.source, ProjectSource::System { .. }) {
            return Err(AppError::Validation(
                "system project is read-only".to_string(),
            ));
        }
        Ok(())
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
    ) -> Result<TargetSetUpdateOutcome, AppError> {
        let previous = self
            .repo
            .get_target_set(id)
            .await?
            .ok_or_else(|| AppError::NotFound("Target set not found".to_string()))?;
        let name = normalize_name(input.name)?;
        let selector = normalize_selector(input.selector)?;
        let members = input
            .members
            .map(|members| {
                members
                    .into_iter()
                    .map(normalize_static_member)
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?;
        if input.mode == PlaybookTargetSetMode::Static
            && members.as_ref().is_some_and(|members| members.is_empty())
        {
            return Err(AppError::Validation(
                "Static target set requires at least one member".to_string(),
            ));
        }
        let record = UpdateTargetSetRecord {
            id,
            name,
            mode: input.mode,
            entity_kind: "connection_target".to_string(),
            selector,
            members,
            user_id,
        };
        let target_set = self
            .repo
            .update_target_set(record)
            .await?
            .ok_or_else(|| AppError::NotFound("Target set not found".to_string()))?;
        Ok(TargetSetUpdateOutcome {
            changed_selector_keys: changed_selector_keys(&previous.selector, &target_set.selector),
            mode_changed: previous.mode != target_set.mode,
            name_changed: previous.name != target_set.name,
            target_set,
        })
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
    ) -> Result<(Vec<PlaybookTargetSetListItem>, i64), AppError> {
        self.repo
            .list_target_sets(filter, limit.clamp(1, 500), offset.max(0))
            .await
    }

    pub async fn get_target_set(&self, id: Uuid) -> Result<Option<PlaybookTargetSet>, AppError> {
        let Some(mut target_set) = self.repo.get_target_set(id).await? else {
            return Ok(None);
        };
        target_set.members = Some(self.repo.list_target_set_members(id).await?);
        Ok(Some(target_set))
    }

    pub async fn preview_target_set(
        &self,
        id: Uuid,
        limit: i64,
    ) -> Result<Option<PlaybookTargetSetPreview>, AppError> {
        self.repo
            .preview_target_set(id, clamp_target_preview_limit(limit))
            .await
    }

    pub async fn list_target_set_facets(
        &self,
        kind: crate::contexts::playbooks::domain::TargetSetFacetKind,
        selector: Option<Value>,
        search: Option<String>,
        limit: i64,
        offset: i64,
    ) -> Result<crate::contexts::playbooks::domain::TargetSetFacetPage, AppError> {
        let selector = normalize_selector(selector)?;
        let search = search.unwrap_or_default();
        if search.chars().count() > 200 {
            return Err(AppError::Validation(
                "Facet search must not exceed 200 characters".into(),
            ));
        }
        self.repo
            .list_target_set_facets(
                kind,
                &selector,
                search.trim(),
                limit.clamp(1, 100),
                offset.max(0),
            )
            .await
    }

    /// Preview a selector the caller has not saved yet. Selector validation is
    /// the one target-set save uses, so the editor sees exactly the set it is
    /// about to create or update; nothing is written.
    pub async fn preview_target_selector(
        &self,
        selector: Option<Value>,
        limit: i64,
    ) -> Result<PlaybookTargetSelectorPreview, AppError> {
        let selector = normalize_selector(selector)?;
        self.repo
            .preview_target_selector(&selector, clamp_target_preview_limit(limit))
            .await
    }

    pub async fn create_assignment(
        &self,
        input: CreateAssignmentInput,
        user_id: Option<Uuid>,
    ) -> Result<PlaybookAssignment, AppError> {
        let quick_action_enabled = input.quick_action_enabled.unwrap_or(false);
        let (scope_kind, scope_id) = normalize_assignment_scope(input.scope_kind, input.scope_id)?;
        let record = CreateAssignmentRecord {
            job_template_id: input.job_template_id,
            target_set_id: input.target_set_id,
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
                PLAYBOOK_RUN_MAX_TIMEOUT_SECONDS,
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

        let record = UpdateAssignmentRecord {
            id,
            job_template_id: input.job_template_id,
            target_set_id: input.target_set_id,
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
                                PLAYBOOK_RUN_MAX_TIMEOUT_SECONDS,
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
                    "Job template or target set not found".to_string(),
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
        Ok(self
            .repo
            .list_target_assignments(target_id, limit.clamp(1, 100))
            .await?
            .map(|result| PlaybookTargetAssignments {
                target: result.target,
                items: result
                    .items
                    .into_iter()
                    .map(|item| PlaybookTargetAssignment {
                        assignment: item.assignment,
                        job_template: item.template,
                    })
                    .collect(),
            }))
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
        let owner_user_id = user_id.ok_or_else(|| {
            AppError::Forbidden("A schedule requires an authenticated owner".into())
        })?;
        validate_schedule_targets(&input.targets)?;
        let overrides = input.overrides.unwrap_or_default();
        let authorizer = self.launch_authorizer.as_ref().ok_or_else(|| {
            AppError::Config("Playbook launch authorizer is not configured".into())
        })?;
        authorizer.validate_schedule_owner(owner_user_id).await?;
        let spec = self
            .prepare_launch_with_targets(
                input.template_id,
                Some(&input.targets),
                overrides.extra_vars(),
            )
            .await?;
        authorizer
            .authorize_launch(&Principal::user(owner_user_id), None, &spec)
            .await?;
        let record = CreateScheduleRecord {
            template_id: input.template_id,
            targets: input.targets,
            overrides,
            owner_user_id,
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
            metadata: validate_launch_metadata(normalize_metadata(input.metadata)?)?,
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
        let metadata = input.metadata.map(validate_launch_metadata).transpose()?;
        if let Some(targets) = &input.targets {
            validate_schedule_targets(targets)?;
        }
        let authorizer = self.launch_authorizer.as_ref().ok_or_else(|| {
            AppError::Config("Playbook launch authorizer is not configured".into())
        })?;
        authorizer
            .validate_schedule_owner(existing.owner_user_id)
            .await?;
        let targets = input.targets.as_ref().unwrap_or(&existing.targets);
        let overrides = input.overrides.as_ref().unwrap_or(&existing.overrides);
        let spec = self
            .prepare_launch_with_targets(
                existing.template_id,
                Some(targets),
                overrides.extra_vars(),
            )
            .await?;
        authorizer
            .authorize_launch(&Principal::user(existing.owner_user_id), None, &spec)
            .await?;
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
            targets: input.targets,
            overrides: input.overrides,
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

    async fn prepare_launch_with_targets(
        &self,
        template_id: Uuid,
        schedule_targets: Option<&ScheduleTargets>,
        overrides: &Value,
    ) -> Result<LaunchSpec, AppError> {
        let template = self
            .repo
            .get_job_template(template_id)
            .await?
            .ok_or_else(|| AppError::NotFound("Job template not found".into()))?;
        if template.id != template_id {
            return Err(AppError::Validation(
                "Job template identity mismatch".into(),
            ));
        }
        validate_workspace_job_template(&template)?;
        let revision = self.resolve_template_revision(&template).await?;
        if self
            .repo
            .get_project(template.project_id)
            .await?
            .is_some_and(|project| matches!(project.source, ProjectSource::System { .. }))
        {
            Self::validate_builtin_snapshot(&template, &revision)?;
        }
        let revision_id = revision.revision_id;
        let tree_sha256 = revision.tree_sha256.clone();
        let vars_schema = resolve_vars_schema(&template.vars_schema, &revision.entries)?;
        let parameters = resolve_parameters(
            vars_schema.as_ref(),
            &template.options.extra_vars,
            overrides,
            &template.options.secret_refs,
        )?;
        let mut options = template.options;
        options.extra_vars = parameters.effective;
        let vars_schema = parameters.schema;
        let environment = self
            .repo
            .get_execution_environment(template.execution_environment_id)
            .await?
            .ok_or_else(|| AppError::NotFound("Execution environment not found".into()))?;
        if environment.id != template.execution_environment_id {
            return Err(AppError::Validation(
                "Execution environment identity mismatch".into(),
            ));
        }
        if !environment.enabled {
            return Err(AppError::Validation(
                "Execution environment is disabled".into(),
            ));
        }
        let (image, image_digest) = split_pinned_image(&environment.image)?;
        let targets = match (&template.inventory, schedule_targets) {
            (InventorySelection::Native { sources, .. }, _) => {
                // Storing a template may leave its targets unconfigured; a launch
                // may not. The availability lookup reports the same state.
                if sources.is_empty() {
                    return Err(AppError::Validation(
                        "Native inventory sources must not be empty".to_string(),
                    ));
                }
                Vec::new()
            }
            (InventorySelection::Managed { .. }, Some(policy)) => {
                validate_schedule_targets(policy)?;
                match policy {
                    ScheduleTargets::Fixed { targets, .. } if !targets.is_empty() => {
                        targets.clone()
                    }
                    ScheduleTargets::Fixed { .. } => {
                        return Err(AppError::Validation(
                            "Managed inventory has no targets".into(),
                        ));
                    }
                    ScheduleTargets::Dynamic { selector, .. } => {
                        let items = self
                            .repo
                            .resolve_target_selector(selector, PLAYBOOK_RUN_MAX_TARGET_LIMIT)
                            .await?;
                        if items.is_empty()
                            || items
                                .iter()
                                .any(|item| item.name.trim().is_empty() || item.name.contains('\0'))
                        {
                            return Err(AppError::Validation(
                                "Managed inventory has no valid targets".into(),
                            ));
                        }
                        items
                            .into_iter()
                            .map(|item| ResolvedTarget {
                                hostname: item.name,
                                target_id: Some(item.target_id),
                                source_id: Some(item.source_id),
                            })
                            .collect()
                    }
                }
            }
            (InventorySelection::Managed { target_set_id }, None) => {
                let preview = self
                    .repo
                    .preview_target_set(*target_set_id, PLAYBOOK_RUN_MAX_TARGET_LIMIT)
                    .await?
                    .ok_or_else(|| AppError::NotFound("Target set not found".into()))?;
                if preview.truncated || preview.total != preview.items.len() as i64 {
                    return Err(AppError::Validation(
                        "Target set exceeds the launch limit".into(),
                    ));
                }
                if preview.items.is_empty() {
                    return Err(AppError::Validation(
                        "Managed inventory has no targets".into(),
                    ));
                }
                if preview
                    .items
                    .iter()
                    .any(|item| item.name.trim().is_empty() || item.name.contains('\0'))
                {
                    return Err(AppError::Validation(
                        "Resolved target hostname is invalid".into(),
                    ));
                }
                preview
                    .items
                    .into_iter()
                    .map(|item| ResolvedTarget {
                        hostname: item.name,
                        target_id: Some(item.target_id),
                        source_id: Some(item.source_id),
                    })
                    .collect()
            }
        };
        Ok(LaunchSpec {
            template_id: template.id,
            project_id: template.project_id,
            revision_id,
            tree_sha256,
            execution_environment_id: template.execution_environment_id,
            image,
            image_digest,
            workdir: template.workdir,
            playbook_paths: template.playbook_paths,
            inventory: template.inventory,
            vars_schema,
            options,
            targets,
        })
    }

    pub async fn launch_job_template(
        &self,
        template_id: Uuid,
        input: LaunchJobTemplateInput,
        user_id: Uuid,
        request_context: &RequestContext,
    ) -> Result<PlaybookRunGroupCreated, AppError> {
        if request_context.user_id != Some(user_id) {
            return Err(AppError::Forbidden(
                "Request identity does not match launch principal".into(),
            ));
        }
        let authorizer = self.launch_authorizer.as_ref().ok_or_else(|| {
            AppError::Config("Playbook launch authorizer is not configured".into())
        })?;
        let metadata = validate_launch_metadata(normalize_metadata(input.metadata)?)?;
        let overrides = input.overrides.unwrap_or_default();
        let mut spec = self
            .prepare_launch_with_targets(template_id, None, overrides.extra_vars())
            .await?;
        narrow_manual_targets(&mut spec, input.target_ids.as_deref())?;
        let authorization = authorizer
            .authorize_launch(&Principal::user(user_id), Some(request_context), &spec)
            .await?;
        let metadata = stamp_launch_audit_metadata(metadata, Some(request_context));
        let created = self
            .repo
            .create_run_group(CreateRunGroupRecord {
                launch_spec: spec,
                schedule_id: None,
                scheduled_for: None,
                trigger_kind: PlaybookRunTriggerKind::Manual,
                requested_by_user_id: Some(user_id),
                service_principal_id: None,
                metadata,
                secret_env_authorization: authorization.secret_env,
                ssh_record_contexts: authorization.ssh_record_contexts,
            })
            .await?;
        Ok(PlaybookRunGroupCreated {
            run_group: created.run_group,
            runs: created.runs,
        })
    }

    pub async fn create_scheduled_run_group(
        &self,
        schedule_id: Uuid,
        scheduled_for: DateTime<Utc>,
    ) -> Result<PlaybookRunGroupCreated, AppError> {
        let schedule = self
            .repo
            .get_schedule(schedule_id)
            .await?
            .ok_or_else(|| AppError::NotFound("Schedule not found".into()))?;
        if schedule.id != schedule_id || !schedule.enabled {
            return Err(AppError::Forbidden("Schedule is not enabled".into()));
        }
        let authorizer = self.launch_authorizer.as_ref().ok_or_else(|| {
            AppError::Config("Playbook launch authorizer is not configured".into())
        })?;
        authorizer
            .validate_schedule_owner(schedule.owner_user_id)
            .await?;
        let principal = Principal::user(schedule.owner_user_id);
        let metadata = validate_launch_metadata(schedule.metadata)?;
        let spec = self
            .prepare_launch_with_targets(
                schedule.template_id,
                Some(&schedule.targets),
                schedule.overrides.extra_vars(),
            )
            .await?;
        let authorization = authorizer.authorize_launch(&principal, None, &spec).await?;
        let metadata = stamp_launch_audit_metadata(metadata, None);
        let created = self
            .repo
            .create_run_group(CreateRunGroupRecord {
                launch_spec: spec,
                schedule_id: Some(schedule_id),
                scheduled_for: Some(scheduled_for),
                trigger_kind: PlaybookRunTriggerKind::Schedule,
                requested_by_user_id: Some(schedule.owner_user_id),
                service_principal_id: None,
                metadata,
                secret_env_authorization: authorization.secret_env,
                ssh_record_contexts: authorization.ssh_record_contexts,
            })
            .await?;
        Ok(PlaybookRunGroupCreated {
            run_group: created.run_group,
            runs: created.runs,
        })
    }

    /// Stop a schedule whose owner lost launch authority.
    ///
    /// A permanent authority refusal (owner missing or inactive, or a policy
    /// grant revoked) must not retry the same occurrence on every cron tick.
    /// The scheduler calls this after a `Forbidden` fire and records the
    /// lifecycle event for the transition it observes here.
    pub async fn stop_schedule_for_owner_loss(&self, schedule_id: Uuid) -> Result<bool, AppError> {
        self.repo
            .pause_schedule_owner_authority(schedule_id, SCHEDULE_OWNER_ACCESS_REVOKED)
            .await
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
            audience: input.audience,
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
        audience: JobTokenAudience,
        now: DateTime<Utc>,
    ) -> Result<PlaybookArtifact, AppError> {
        let record = normalize_artifact_input(run_id, input)?;
        self.repo
            .create_artifact_with_callback(callback_token_auth(raw_token, now)?, record, audience)
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

    pub async fn append_console_batch_with_group_callback_token(
        &self,
        group_id: Uuid,
        raw_token: &str,
        batch: RunnerConsoleBatch,
    ) -> Result<ConsoleAck, AppError> {
        let auth = callback_token_auth(raw_token, Utc::now())?;
        validate_console_batch(&batch).map_err(AppError::Validation)?;
        self.repo
            .append_console_batch_with_callback(group_id, batch, auth)
            .await
    }

    pub async fn list_console(
        &self,
        group_id: Uuid,
        from_sequence: u64,
        limit: i64,
        dispatch_id: Option<Uuid>,
    ) -> Result<ConsolePage, AppError> {
        self.repo
            .list_console(group_id, from_sequence, limit, dispatch_id)
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
        let group_status = normalize_terminal_group_status(input.group_status)?;
        if input.dispatch_attempt < 1 || input.console_next_sequence > MAX_CONSOLE_SEQUENCE {
            return Err(AppError::Validation(
                "Invalid completion dispatch attempt or final console sequence".to_string(),
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
                status: normalize_terminal_batch_run_status(item.status)?,
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
                    group_status,
                    exit_code: input.exit_code,
                    error_message: normalize_optional_text(
                        input.error_message,
                        PLAYBOOK_RUN_ERROR_MAX_CHARS,
                    )?,
                    dispatch_id: Some(input.dispatch_id),
                    dispatch_attempt: Some(input.dispatch_attempt),
                    console_next_sequence: Some(input.console_next_sequence),
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
            .validate_active_job_token(
                token_id,
                token_hash.as_str(),
                JobTokenAudience::Sandbox,
                now,
            )
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

    /// Validate a run-bound sandbox job token for the discovery report
    /// callback. The report is an artifact write, so the only capability it
    /// needs is `artifact:create`; it never grants monitoring CRUD.
    pub async fn validate_artifact_job_token(
        &self,
        raw_token: &str,
        now: DateTime<Utc>,
    ) -> Result<PlaybookJobToken, AppError> {
        let (token_id, _) = parse_job_token(raw_token)
            .ok_or_else(|| AppError::Auth("TOKEN_INVALID".to_string()))?;
        let token_hash = hex_sha256(raw_token.as_bytes());
        let token = self
            .repo
            .validate_active_job_token(
                token_id,
                token_hash.as_str(),
                JobTokenAudience::Sandbox,
                now,
            )
            .await?;
        if !job_token_has_scope(&token.scopes, PLAYBOOK_ARTIFACT_CREATE_SCOPE) {
            return Err(AppError::Forbidden(
                "TOKEN_SCOPE_DENIED: artifact:create".to_string(),
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

    pub async fn lease_queued_groups(
        &self,
        input: LeaseQueuedRunsInput,
    ) -> Result<Vec<PlaybookRunGroup>, AppError> {
        let now = Utc::now();
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
        self.repo.lease_queued_groups(record).await
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

    pub async fn start_leased_run_group(
        &self,
        group_id: Uuid,
        lease_owner: &str,
    ) -> Result<PlaybookRunGroupExecution, AppError> {
        let now = Utc::now();
        let lease_owner = normalize_lease_owner(lease_owner.to_owned())?;
        let group = self
            .repo
            .get_run_group_with_runs(group_id, PLAYBOOK_RUN_MAX_TARGET_LIMIT)
            .await?
            .ok_or_else(|| AppError::NotFound("Playbook run group not found".into()))?;
        if group.run_group.total_runs < 0
            || group.runs.len() != group.run_group.total_runs as usize
            || group.run_group.status != PlaybookRunStatus::Leased
            || group.run_group.lease_owner.as_deref() != Some(&lease_owner)
            || group
                .run_group
                .lease_expires_at
                .is_none_or(|expires| expires <= now)
            || group.run_group.cancel_requested_at.is_some()
            || group.runs.iter().any(|run| {
                run.status != PlaybookRunStatus::Leased
                    || run.lease_owner.as_deref() != Some(&lease_owner)
                    || run.cancel_requested_at.is_some()
                    || run.lease_expires_at.is_none_or(|expires| expires <= now)
            })
        {
            return Err(AppError::Conflict(
                "Run group lease is expired, canceled, or not owned by this worker".into(),
            ));
        }
        let dispatch_attempt = group
            .run_group
            .dispatch_attempt
            .checked_add(1)
            .ok_or_else(|| AppError::Validation("Dispatch attempt overflow".into()))?;
        let dispatch_id = Uuid::now_v7();
        let run_ids: Vec<Uuid> = group.runs.iter().map(|run| run.id).collect();
        let ttl = i64::from(group.run_group.launch_spec.options.timeout_seconds)
            .saturating_add(300)
            .min(PLAYBOOK_JOB_TOKEN_MAX_TTL_SECONDS);
        let mut token_records = Vec::with_capacity(run_ids.len());
        let mut sandbox_token_records = Vec::with_capacity(run_ids.len());
        let mut raw_tokens = HashMap::with_capacity(run_ids.len());
        let mut sandbox_run_tokens = BTreeMap::new();
        for run_id in &run_ids {
            let token = self
                .build_job_token_record(
                    *run_id,
                    JobTokenAudience::Supervisor,
                    Some(ttl),
                    None,
                    None,
                    now,
                )
                .await?;
            raw_tokens.insert(*run_id, token.raw_token);
            token_records.push(token.record);
            let sandbox = self
                .build_job_token_record(
                    *run_id,
                    JobTokenAudience::Sandbox,
                    Some(ttl),
                    Some(sandbox_job_token_scopes()),
                    None,
                    now,
                )
                .await?;
            sandbox_run_tokens.insert(*run_id, sandbox.raw_token);
            sandbox_token_records.push(sandbox.record);
        }
        let callback = build_group_callback_token_record(Some(ttl), now)?;
        let started = self
            .repo
            .start_leased_runs(StartLeasedRunsRecord {
                run_group_id: group_id,
                run_ids,
                lease_owner,
                dispatch_id,
                dispatch_attempt,
                now,
                job_tokens: token_records,
                sandbox_job_tokens: sandbox_token_records,
                group_callback_token: callback.record,
            })
            .await?;
        let mut executions = Vec::with_capacity(started.len());
        for item in started {
            let job_token = raw_tokens.remove(&item.run.id).ok_or_else(|| {
                AppError::Database("Raw job token missing from batch result".into())
            })?;
            executions.push(PlaybookRunExecution {
                run: item.run,
                job_token,
            });
        }
        let run_group = self
            .repo
            .get_run_group_with_runs(group_id, PLAYBOOK_RUN_MAX_TARGET_LIMIT)
            .await?
            .ok_or_else(|| AppError::NotFound("Started run group not found".into()))?
            .run_group;
        if run_group.dispatch_id != Some(dispatch_id)
            || run_group.dispatch_attempt != dispatch_attempt
        {
            return Err(AppError::Conflict(
                "Started dispatch identity changed".into(),
            ));
        }
        Ok(PlaybookRunGroupExecution {
            run_group,
            executions,
            sandbox_run_tokens,
            group_callback_token: callback.raw_token,
        })
    }

    pub async fn mark_run_group_running(
        &self,
        group_id: Uuid,
        lease_owner: &str,
    ) -> Result<Option<PlaybookRunGroup>, AppError> {
        let lease_owner = normalize_lease_owner(lease_owner.to_owned())?;
        self.repo
            .mark_run_group_running(group_id, &lease_owner, Utc::now())
            .await
    }

    /// Requeue only after the caller has established that the dispatch was not accepted.
    pub async fn requeue_run_group(
        &self,
        group_id: Uuid,
        lease_owner: &str,
        retry_at: DateTime<Utc>,
        reason: &str,
    ) -> Result<bool, AppError> {
        let lease_owner = normalize_lease_owner(lease_owner.to_owned())?;
        let reason = truncate_run_event_message(reason.trim(), PLAYBOOK_RUN_ERROR_MAX_CHARS);
        self.repo
            .requeue_run_group(group_id, &lease_owner, retry_at, &reason, Utc::now())
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
        let timeout_seconds = normalize_run_timeout(Some(input.timeout_seconds))?;
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
        audience: JobTokenAudience,
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
                audience,
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

fn normalize_project_source(source: ProjectSource) -> Result<ProjectSource, AppError> {
    match source {
        ProjectSource::Editable => Ok(ProjectSource::Editable),
        ProjectSource::Git {
            url,
            reference,
            auth,
        } => {
            let url = normalize_project_scm_url(Some(url), "git")?
                .ok_or_else(|| AppError::Validation("git project url is required".to_string()))?;
            let reference =
                normalize_required_text(reference, "reference", PLAYBOOK_ENTRYPOINT_MAX_CHARS)?;
            if let ProjectAuth::HttpsBasic { secret_id, .. }
            | ProjectAuth::HttpsBearer { secret_id } = &auth
                && secret_id.is_nil()
            {
                return Err(AppError::Validation(
                    "git project auth secret is required".to_string(),
                ));
            }
            if let ProjectAuth::SshRecord { ssh_record_id } = &auth
                && ssh_record_id.is_nil()
            {
                return Err(AppError::Validation(
                    "git project SSH record is required".to_string(),
                ));
            }
            Ok(ProjectSource::Git {
                url,
                reference,
                auth,
            })
        }
        ProjectSource::System { .. } => Err(AppError::Validation(
            "system project source is managed by the bundled catalog loader".to_string(),
        )),
    }
}

fn same_project_source_kind(left: &ProjectSource, right: &ProjectSource) -> bool {
    matches!(
        (left, right),
        (ProjectSource::Editable, ProjectSource::Editable)
            | (ProjectSource::Git { .. }, ProjectSource::Git { .. })
            | (ProjectSource::System { .. }, ProjectSource::System { .. })
    )
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

fn normalize_metadata(value: Option<Value>) -> Result<Value, AppError> {
    let value = value.unwrap_or_else(|| Value::Object(Default::default()));
    validate_metadata(value)
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

/// Writeback scopes only: a sandbox token may report artifacts, events, status
/// and inventory for its own run and can never authenticate a supervisor
/// callback surface.
fn sandbox_job_token_scopes() -> Vec<String> {
    PLAYBOOK_JOB_TOKEN_ALLOWED_SCOPES
        .iter()
        .map(|scope| (*scope).to_string())
        .collect()
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

/// One preview page size rule for both preview endpoints: the caller's limit,
/// bounded to a positive value and to [`PLAYBOOK_TARGET_PREVIEW_MAX_LIMIT`].
fn clamp_target_preview_limit(limit: i64) -> i64 {
    limit.clamp(1, PLAYBOOK_TARGET_PREVIEW_MAX_LIMIT)
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

/// Sorted top-level selector key names. Values never leave this module for an
/// audit event.
pub fn selector_keys(selector: &Value) -> Vec<String> {
    let mut keys = selector
        .as_object()
        .map(|object| object.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    keys.sort();
    keys
}

/// Top-level keys whose value differs between two selectors, including keys
/// added or removed. Comparison is on the stored JSON, so a reordered array or
/// an equal scalar is not a change.
fn changed_selector_keys(previous: &Value, next: &Value) -> Vec<String> {
    let (Some(previous), Some(next)) = (previous.as_object(), next.as_object()) else {
        return Vec::new();
    };
    let mut changed = BTreeSet::new();
    for (key, value) in previous {
        if next.get(key) != Some(value) {
            changed.insert(key.clone());
        }
    }
    for key in next.keys() {
        if !previous.contains_key(key) {
            changed.insert(key.clone());
        }
    }
    changed.into_iter().collect()
}

#[derive(Debug, Clone)]
struct NormalizedTemplateCredentialRef {
    reference: RunCredentialReference,
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
        });
    }
    Ok(refs)
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

fn normalize_run_timeout(value: Option<i32>) -> Result<i32, AppError> {
    let timeout = value.unwrap_or(PLAYBOOK_RUN_DEFAULT_TIMEOUT_SECONDS);
    if !(1..=PLAYBOOK_RUN_MAX_TIMEOUT_SECONDS).contains(&timeout) {
        return Err(AppError::Validation(
            "timeout_seconds must be between 1 and 86400".to_string(),
        ));
    }
    Ok(timeout)
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
    // The single-run endpoint has no group process verdict, so it cannot carry
    // `unreported`: that value is only meaningful for a member of an
    // authoritative group completion batch.
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

fn normalize_terminal_batch_run_status(
    status: PlaybookRunStatus,
) -> Result<PlaybookRunStatus, AppError> {
    if matches!(
        status,
        PlaybookRunStatus::Succeeded
            | PlaybookRunStatus::Failed
            | PlaybookRunStatus::Canceled
            | PlaybookRunStatus::TimedOut
            | PlaybookRunStatus::Unreported
    ) {
        Ok(status)
    } else {
        Err(AppError::Validation(
            "Run completion status must be terminal".to_string(),
        ))
    }
}

/// The group field carries the process/lifecycle verdict, which has no
/// `Unreported` member: a group that never produced per-host verdicts is still
/// a succeeded/failed/timed-out/canceled process.
fn normalize_terminal_group_status(
    status: PlaybookRunStatus,
) -> Result<PlaybookRunStatus, AppError> {
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
            "Run group completion status must be a terminal process outcome".to_string(),
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
