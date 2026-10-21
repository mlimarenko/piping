use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::permission_helpers::ensure_access;
use crate::api::rest::envelope_extractors::Path;
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::application::admin_service::{
    NamedListParams, PolicyListParams, RoleAssignmentListParams, UserListParams,
};
use crate::contexts::access_control::application::validate_local_password;
use crate::contexts::access_control::domain::vocabulary::{
    EnforcementStatus, allowed_constraint_attribute_keys, resource_governance,
};
use crate::contexts::access_control::domain::{
    Action, CredentialProvisioningCommand, CredentialProvisioningMode, Group as DomainGroup,
    Policy as DomainPolicy, PolicyDecision, PolicyEffect, Principal, ResourceType,
    Role as DomainRole, RoleAssignment as DomainRoleAssignment, User as DomainUser,
    constraint_attribute_keys_used, resource_pattern_is_valid, validate_constraints,
};
use crate::contexts::access_control::infrastructure::persistence::{
    GROUP_COLUMNS, POLICY_COLUMNS, ROLE_ASSIGNMENT_COLUMNS, ROLE_COLUMNS, USER_COLUMNS,
};
use crate::contexts::audit::{application::AuditRecorder, domain::AuditEvent};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_strings, parse_csv_uuids};
use crate::contexts::shared::pagination::normalize_page;
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;
use crate::observability::{EventFields, log_error_event};
// Shared UUID parsers, re-exported so the `admin_access_control` submodules
// keep importing them via `super::`.
pub(super) use crate::api::rest::routes::validation::{parse_optional_uuid, parse_uuid};
use strum::IntoEnumIterator;

pub mod bulk;

pub use bulk::*;

pub mod bindings_policies;
pub mod evaluation;
pub mod groups_roles;
pub mod users;

pub use bindings_policies::*;
pub use evaluation::*;
pub use groups_roles::*;
pub use users::*;

/// Record an access-control mutation that has *already* been applied.
///
/// Called after the change is committed, so it cannot report failure to the
/// caller: answering "error" for a user that now exists would invite a retry
/// and a duplicate. A failed write is therefore logged as an error event —
/// loudly, with the event type and target, so the gap can be reconciled — and
/// the handler still returns what actually happened. This matches how the rest
/// of the codebase treats post-commit audit (monitoring metric-query test,
/// inventory import/export).
///
/// Making the record atomic with the mutation is the stronger guarantee, but it
/// requires the access-control services to accept a caller-owned transaction;
/// that is a change to those services, not to these handlers.
async fn record_access_control_mutation<R>(
    recorder: &R,
    request_context: &RequestContext,
    event_type: &'static str,
    target_type: &'static str,
    target_id: Uuid,
) where
    R: AuditRecorder + ?Sized,
{
    let outcome = match request_context.require_authenticated_user_id() {
        Ok(actor_id) => {
            recorder
                .record_event(AuditEvent {
                    id: Uuid::now_v7(),
                    event_type: event_type.to_string(),
                    actor_id: Some(actor_id),
                    actor_kind: request_context.audit_actor_kind().to_string(),
                    target_type: Some(target_type.to_string()),
                    target_id: Some(target_id),
                    metadata: json!({
                        "result": "success",
                        "auth_method": request_context.auth_method,
                        "correlation_id": request_context.correlation_id,
                    }),
                    created_at: Utc::now(),
                })
                .await
        }
        Err(error) => Err(error),
    };

    if let Err(error) = outcome {
        log_error_event(
            "access_control.audit.record_failed",
            "access_control_audit_record_failed",
            error.to_string(),
            EventFields::default()
                .with_operation("access_control_audit_record")
                .with_status("failed")
                .with_context("event_type", event_type.to_string())
                .with_context("target_type", target_type.to_string())
                .with_context("target_id", target_id.to_string()),
        );
    }
}

#[cfg(test)]
mod audit_tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;

    use super::*;

    #[derive(Clone, Default)]
    struct TestRecorder {
        events: Arc<Mutex<Vec<AuditEvent>>>,
    }

    #[async_trait]
    impl AuditRecorder for TestRecorder {
        async fn record_event(&self, event: AuditEvent) -> Result<(), AppError> {
            self.events
                .lock()
                .expect("test recorder mutex should not be poisoned")
                .push(event);
            Ok(())
        }
    }

    #[tokio::test]
    async fn sensitive_mutations_record_actor_and_success_result() {
        let recorder = TestRecorder::default();
        let actor_id = Uuid::now_v7();
        let request_context = RequestContext::new(
            Some(actor_id),
            "session".to_string(),
            None,
            "audit-test-request".to_string(),
            crate::contexts::shared::request_context::RequestAuth::default(),
        );
        let expected = [
            ("access_control.user.created", "user"),
            ("access_control.user.updated", "user"),
            ("access_control.user.password_rotated", "user"),
            ("access_control.user.deleted", "user"),
            ("access_control.role.created", "role"),
            ("access_control.role.deleted", "role"),
            ("access_control.role_assignment.created", "role_assignment"),
            ("access_control.role_assignment.deleted", "role_assignment"),
        ];

        for (event_type, target_type) in expected {
            record_access_control_mutation(
                &recorder,
                &request_context,
                event_type,
                target_type,
                Uuid::now_v7(),
            )
            .await;
        }

        let events = recorder
            .events
            .lock()
            .expect("test recorder mutex should not be poisoned");
        assert_eq!(events.len(), expected.len());
        for (event, (event_type, target_type)) in events.iter().zip(expected) {
            assert_eq!(event.event_type, event_type);
            assert_eq!(event.actor_id, Some(actor_id));
            assert_eq!(event.actor_kind, "user");
            assert_eq!(event.target_type.as_deref(), Some(target_type));
            assert!(event.target_id.is_some());
            assert_eq!(event.metadata["result"], "success");
        }

        let handler_sources = [
            include_str!("admin_access_control/users.rs"),
            include_str!("admin_access_control/groups_roles.rs"),
            include_str!("admin_access_control/bindings_policies.rs"),
        ]
        .concat();
        for (event_type, _) in expected {
            assert_eq!(
                handler_sources.matches(event_type).count(),
                1,
                "{event_type} must be wired to exactly one handler"
            );
        }
    }

    /// A recorder that refuses every write, standing in for a sick audit store.
    #[derive(Clone, Default)]
    struct FailingRecorder;

    #[async_trait]
    impl AuditRecorder for FailingRecorder {
        async fn record_event(&self, _event: AuditEvent) -> Result<(), AppError> {
            Err(AppError::Internal("audit store is down".to_string()))
        }
    }

    /// The mutation is already committed when the record is attempted, so a
    /// failed record must not become the caller's problem: reporting an error
    /// for a change that happened would invite a retry and a duplicate.
    ///
    /// What this test actually covers: the emitter completes against a store
    /// that refuses every write, so a sick audit store cannot take a handler
    /// down with it.
    ///
    /// What it does *not* need to cover: an earlier revision wrapped each call
    /// site in `if let Err(...) { return handle_app_error(error) }`, answering
    /// "500" for a user that had just been created. Returning unit rules that
    /// out at compile time — restoring the old form is a type error, verified
    /// by planting it. So the call-site invariant is held by the signature, not
    /// by an assertion here. The count below is still worth keeping: the
    /// compiler is happy if a call site is deleted outright.
    #[tokio::test]
    async fn a_failed_audit_write_does_not_turn_into_a_failed_request() {
        let request_context = RequestContext::new(
            Some(Uuid::now_v7()),
            "session".to_string(),
            None,
            "audit-failure-request".to_string(),
            crate::contexts::shared::request_context::RequestAuth::default(),
        );

        record_access_control_mutation(
            &FailingRecorder,
            &request_context,
            "access_control.user.created",
            "user",
            Uuid::now_v7(),
        )
        .await;

        let handler_sources = [
            include_str!("admin_access_control/users.rs"),
            include_str!("admin_access_control/groups_roles.rs"),
            include_str!("admin_access_control/bindings_policies.rs"),
        ]
        .concat();
        let calls = handler_sources
            .matches("record_access_control_mutation(")
            .count();
        assert_eq!(calls, 8, "every audited mutation must still emit a record");
    }
}
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CredentialInput {
    pub mode: String,
    pub password: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UserInput {
    pub username: String,
    pub email: String,
    pub status: String,
    pub auth_source: String,
    pub external_id: Option<String>,
    pub credential: Option<CredentialInput>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct UserResponse {
    pub id: String,
    pub username: String,
    pub email: String,
    pub status: String,
    pub auth_source: String,
    pub external_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generated_password: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct PasswordRotationInput {
    pub mode: String,
    pub password: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PasswordRotationResponse {
    pub user_id: String,
    pub password_changed_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generated_password: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct GroupInput {
    pub name: String,
    pub external_id: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct GroupResponse {
    pub id: String,
    pub name: String,
    pub external_id: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct RoleInput {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RoleResponse {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct RoleAssignmentInput {
    pub user_id: Option<String>,
    pub group_id: Option<String>,
    pub role_id: String,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct RoleAssignmentListQuery {
    /// Comma-separated role ids; matches any (`IN`).
    pub role_id: Option<String>,
    /// Comma-separated user ids; matches any (`IN`).
    pub user_id: Option<String>,
    /// Comma-separated group ids; matches any (`IN`).
    pub group_id: Option<String>,
    /// Comma-separated subject type filters: user, group.
    pub subject_type: Option<String>,
    /// Optional free-text search over role name and subject (user
    /// username/email, group name), applied as case-insensitive `ILIKE`.
    #[serde(default)]
    pub search: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub offset: Option<u32>,
    /// Logical sort column key (allowlisted; unknown keys are rejected).
    pub sort: Option<String>,
    /// Sort direction: `asc` (default) or `desc`.
    pub order: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RoleAssignmentResponse {
    pub id: String,
    pub user_id: Option<String>,
    pub group_id: Option<String>,
    pub role_id: String,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct PolicyInput {
    pub name: String,
    pub description: Option<String>,
    pub effect: String,
    pub actions: Vec<String>,
    pub resource_types: Vec<String>,
    pub constraints: Option<Value>,
    pub subjects: Vec<String>,
    pub resources: Vec<String>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct PolicyListQuery {
    pub name: Option<String>,
    /// Comma-separated effects (allow/deny); matches any (`IN`).
    pub effect: Option<String>,
    /// Comma-separated enabled flags (true/false); matches any (`IN`).
    pub enabled: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub offset: Option<u32>,
    /// Logical sort column key (allowlisted; unknown keys are rejected).
    pub sort: Option<String>,
    /// Sort direction: `asc` (default) or `desc`.
    pub order: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RoleGrantsResponse {
    pub role_id: String,
    pub assignments: Vec<RoleAssignmentResponse>,
    /// Policies granted to this role, i.e. policies whose `subjects` contains
    /// `role:{role_id}`.
    pub policies: Vec<PolicyResponse>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PolicyResponse {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub effect: String,
    pub actions: Vec<String>,
    pub resource_types: Vec<String>,
    pub constraints: Option<Value>,
    pub subjects: Vec<String>,
    pub resources: Vec<String>,
    pub enabled: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DeleteResponse {
    pub deleted: bool,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ListPageQuery {
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub offset: Option<u32>,
    /// Optional free-text search. Applied as a case-insensitive `ILIKE '%term%'`
    /// on the entity's primary text column(s). An empty or whitespace-only value
    /// is treated as no filter.
    pub search: Option<String>,
    /// Logical sort column key (allowlisted; unknown keys are rejected).
    pub sort: Option<String>,
    /// Sort direction: `asc` (default) or `desc`.
    pub order: Option<String>,
}

/// Query parameters for user listing — pagination, free-text search, multi-value
/// `status` / `auth_source` IN filters, and allowlisted sorting.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UserListQuery {
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub offset: Option<u32>,
    /// Case-insensitive `ILIKE '%term%'` over username/email. Empty means no filter.
    pub search: Option<String>,
    /// Comma-separated statuses; matches any (`IN`).
    pub status: Option<String>,
    /// Comma-separated auth sources; matches any (`IN`).
    pub auth_source: Option<String>,
    /// Logical sort column key (allowlisted; unknown keys are rejected).
    pub sort: Option<String>,
    /// Sort direction: `asc` (default) or `desc`.
    pub order: Option<String>,
}

/// Pagination envelope for access-control list endpoints.
///
/// Every `/api/v1/admin/access-control/*` list route returns this shape:
/// `{ items, total, limit, offset }`, where `limit`/`offset` are the applied
/// values the server actually used.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AccessControlListEnvelope<T> {
    pub items: Vec<T>,
    pub total: u64,
    pub limit: u32,
    pub offset: u32,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CacheInvalidationResponse {
    pub invalidated: bool,
    pub scope: String,
    pub generation: u64,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct AccessEvaluationRequest {
    pub user_id: String,
    pub action: String,
    pub resource_type: String,
    pub resource_id: Option<String>,
    pub attributes: Option<Value>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PolicyTraceRefResponse {
    pub policy_id: String,
    pub policy_name: String,
    pub effect: String,
    pub via_roles: Vec<String>,
    pub via_subjects: Vec<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AccessEvaluationResponse {
    pub decision: String,
    pub reasons: Vec<String>,
    pub matched_allow: Vec<PolicyTraceRefResponse>,
    pub matched_deny: Vec<PolicyTraceRefResponse>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct BulkRequest<T> {
    pub create: Option<Vec<T>>,
    pub update: Option<Vec<BulkUpdate<T>>>,
    pub delete: Option<Vec<String>>,
}

impl<T> BulkRequest<T> {
    /// A bulk request must carry at least one non-empty operation. An all-empty
    /// (or all-absent) batch is a client mistake — it would answer `200` while
    /// doing nothing — so it is a 400, checked before any side effect.
    pub(crate) fn validate(&self) -> Result<(), AppError> {
        let has_create = self.create.as_ref().is_some_and(|items| !items.is_empty());
        let has_update = self.update.as_ref().is_some_and(|items| !items.is_empty());
        let has_delete = self.delete.as_ref().is_some_and(|items| !items.is_empty());
        if has_create || has_update || has_delete {
            return Ok(());
        }
        Err(AppError::Validation(
            "bulk request must contain at least one non-empty create, update or delete".to_string(),
        ))
    }
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct BulkUpdate<T> {
    pub id: String,
    pub data: T,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkResponse<T> {
    pub created: Vec<T>,
    pub updated: Vec<T>,
    pub deleted: Vec<String>,
    pub errors: Vec<BulkError>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkError {
    pub id: Option<String>,
    pub error: String,
}

fn parse_attributes(value: Option<Value>) -> Result<Option<HashMap<String, Value>>, AppError> {
    match value {
        None => Ok(None),
        Some(Value::Object(map)) => Ok(Some(map.into_iter().collect())),
        Some(_) => Err(AppError::Validation(
            "Attributes must be a JSON object".to_string(),
        )),
    }
}

fn policy_decision_to_string(decision: PolicyDecision) -> &'static str {
    match decision {
        PolicyDecision::Allow => "allow",
        PolicyDecision::Deny => "deny",
    }
}

/// Reconstruct a matched policy's trace provenance as pure set logic over the
/// principal's resolved canonical subjects.
///
/// `matched = subject_set ∩ policy.subjects`, then partitioned by kind prefix:
/// `role:{uuid}` resolves (via `role_names`) to the role name for `via_roles`
/// (an unresolvable role uuid falls through to `via_subjects`, never crashes);
/// every other kind (`user:`/`group:`/`service_principal:`) goes to
/// `via_subjects`. Replaces the deleted binding/assignment re-query.
fn map_policy_trace(
    policy_id: &Uuid,
    policies: &HashMap<Uuid, DomainPolicy>,
    subjects: &HashSet<String>,
    role_names: &HashMap<Uuid, String>,
    fallback_effect: PolicyEffect,
) -> PolicyTraceRefResponse {
    let Some(policy) = policies.get(policy_id) else {
        return PolicyTraceRefResponse {
            policy_id: policy_id.to_string(),
            policy_name: policy_id.to_string(),
            effect: serialize_enum(&fallback_effect),
            via_roles: Vec::new(),
            via_subjects: Vec::new(),
        };
    };

    let matched = policy
        .subjects
        .iter()
        .filter(|subject| subjects.contains(*subject));

    let mut via_roles = Vec::new();
    let mut via_subjects = Vec::new();
    for subject in matched {
        if let Some(raw_uuid) = subject.strip_prefix("role:") {
            match Uuid::parse_str(raw_uuid)
                .ok()
                .and_then(|role_id| role_names.get(&role_id))
            {
                Some(name) => via_roles.push(name.clone()),
                // Orphaned/unresolvable role uuid: surface the raw subject rather
                // than crash or silently drop it.
                None => via_subjects.push(subject.clone()),
            }
        } else {
            via_subjects.push(subject.clone());
        }
    }
    via_roles.sort();
    via_roles.dedup();
    via_subjects.sort();
    via_subjects.dedup();

    PolicyTraceRefResponse {
        policy_id: policy.id.to_string(),
        policy_name: policy.name.clone(),
        effect: serialize_enum(&policy.effect),
        via_roles,
        via_subjects,
    }
}

fn parse_optional_credential_command(
    credential: Option<&CredentialInput>,
    auth_source: &str,
) -> Result<Option<CredentialProvisioningCommand>, AppError> {
    match credential {
        Some(credential) => {
            if !auth_source.eq_ignore_ascii_case("local") {
                return Err(AppError::Validation(
                    "Credential payload is supported only for local users".to_string(),
                ));
            }
            parse_credential_command(credential.mode.as_str(), credential.password.clone())
                .map(Some)
        }
        None => Ok(None),
    }
}

fn parse_credential_command(
    mode: &str,
    password: Option<String>,
) -> Result<CredentialProvisioningCommand, AppError> {
    let mode = match mode.trim().to_ascii_lowercase().as_str() {
        "manual" => CredentialProvisioningMode::Manual,
        "generate" => CredentialProvisioningMode::Generate,
        _ => {
            return Err(AppError::Validation(format!(
                "Invalid credential mode: {}",
                mode
            )));
        }
    };
    CredentialProvisioningCommand::new(mode, password)
}

fn validate_credential_command_for_policy(
    command: &CredentialProvisioningCommand,
    password_min_length: usize,
) -> Result<(), AppError> {
    if command.mode == CredentialProvisioningMode::Manual {
        let password = command.password.as_ref().ok_or_else(|| {
            AppError::Validation("Password is required when credential mode is manual".to_string())
        })?;
        validate_local_password(password, password_min_length)?;
    }
    Ok(())
}

fn build_user(payload: &UserInput, id: Uuid) -> Result<DomainUser, AppError> {
    let user = DomainUser {
        id,
        username: payload.username.clone(),
        email: payload.email.clone(),
        status: payload.status.clone(),
        auth_source: payload.auth_source.clone(),
        external_id: payload.external_id.clone(),
    };
    user.validate().map_err(AppError::Validation)?;
    Ok(user)
}

fn build_policy_from_input(payload: PolicyInput, id: Uuid) -> Result<DomainPolicy, AppError> {
    let effect = parse_policy_effect(Some(payload.effect.as_str()))?
        .ok_or_else(|| AppError::Validation("Policy effect is required".to_string()))?;
    let actions = parse_actions(&payload.actions)?;
    let resource_types = parse_resource_types(&payload.resource_types)?;
    // Reject malformed/unrecognized constraint JSON at write time. Without this a
    // bad constraint parses to `Unknown` and silently makes the policy always
    // deny — a confusing footgun. Fail loudly with 400 instead.
    if let Some(constraints) = payload.constraints.as_ref() {
        validate_constraints(constraints).map_err(AppError::Validation)?;
        // Reject an `attributes` constraint that scopes on a key no enforcement
        // site supplies for this policy's resource types (e.g. `role` instead of
        // `role_id`, or any object attribute on a resource whose enforcement is
        // not attribute-wired). Such a constraint would silently never match.
        let used_keys = constraint_attribute_keys_used(constraints);
        if !used_keys.is_empty() {
            let allowed = allowed_constraint_attribute_keys(&resource_types);
            if let Some(bad) = used_keys.iter().find(|key| !allowed.contains(key.as_str())) {
                let mut allowed_list: Vec<&str> = allowed.into_iter().collect();
                allowed_list.sort_unstable();
                let allowed_display = if allowed_list.is_empty() {
                    "none for these resource types".to_string()
                } else {
                    allowed_list.join(", ")
                };
                return Err(AppError::Validation(format!(
                    "constraint attribute `{bad}` cannot be scoped for the policy's resource \
                     types (allowed: {allowed_display})"
                )));
            }
        }
    }

    // Reject a resource pattern the engine's matcher cannot compile — a `re:`
    // regex the Rust `regex` crate rejects (lookaround, backreferences, POSIX
    // collating classes). Left unvalidated it matches nothing in the engine but
    // could be accepted by a database regex dialect (the row-visibility SQL
    // path), disclosing rows the engine denies; and a syntactically invalid
    // regex would make a list query error at runtime. Fail loudly at write time.
    if let Some(bad) = payload
        .resources
        .iter()
        .find(|pattern| !resource_pattern_is_valid(pattern))
    {
        return Err(AppError::Validation(format!(
            "resource pattern `{bad}` is not a valid matcher (a `re:` regex must compile with the \
             Rust regex syntax: no lookaround, backreferences, or POSIX collating classes)"
        )));
    }

    Ok(DomainPolicy {
        id,
        name: payload.name,
        description: payload.description,
        effect,
        actions,
        resource_types,
        constraints: payload.constraints,
        subjects: payload.subjects,
        resources: payload.resources,
        enabled: payload.enabled.unwrap_or(true),
    })
}

fn parse_policy_effect(value: Option<&str>) -> Result<Option<PolicyEffect>, AppError> {
    let value = match value {
        Some(value) => value.trim(),
        None => return Ok(None),
    };
    if value.is_empty() {
        return Ok(None);
    }
    match value.to_lowercase().as_str() {
        "allow" => Ok(Some(PolicyEffect::Allow)),
        "deny" => Ok(Some(PolicyEffect::Deny)),
        _ => Err(AppError::Validation(format!(
            "Invalid policy effect: {}",
            value
        ))),
    }
}

/// Parse a comma-separated list of policy effects into a deduplicated vector.
/// Empty/whitespace entries are skipped; an invalid effect is a client error.
fn parse_csv_policy_effects(raw: Option<&str>) -> Result<Vec<PolicyEffect>, AppError> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let mut out: Vec<PolicyEffect> = Vec::new();
    for token in raw.split(',') {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            continue;
        }
        let effect = parse_policy_effect(Some(trimmed))?
            .ok_or_else(|| AppError::Validation("Invalid policy effect".to_string()))?;
        if !out.contains(&effect) {
            out.push(effect);
        }
    }
    Ok(out)
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

fn parse_actions(values: &[String]) -> Result<Vec<Action>, AppError> {
    values.iter().map(|value| parse_action(value)).collect()
}

fn parse_action(value: &str) -> Result<Action, AppError> {
    let normalized = value.trim().to_uppercase();
    serde_json::from_str::<Action>(&format!("\"{}\"", normalized))
        .map_err(|_| AppError::Validation(format!("Invalid policy action: {}", value)))
}

fn parse_resource_types(values: &[String]) -> Result<Vec<ResourceType>, AppError> {
    values
        .iter()
        .map(|value| parse_resource_type(value))
        .collect()
}

fn parse_resource_type(value: &str) -> Result<ResourceType, AppError> {
    let normalized = value.trim();
    let canonical = normalized.to_lowercase().replace('_', "-");
    if let Some(resource_type) = ResourceType::parse(&canonical) {
        return Ok(resource_type);
    }

    Err(AppError::Validation(format!(
        "Invalid resource type: {}",
        value
    )))
}

fn serialize_enum<T>(value: &T) -> String
where
    T: Serialize,
{
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(ToString::to_string))
        .unwrap_or_else(|| "UNKNOWN".to_string())
}

impl UserResponse {
    fn from_user(user: DomainUser, generated_password: Option<String>) -> Self {
        Self {
            id: user.id.to_string(),
            username: user.username,
            email: user.email,
            status: user.status,
            auth_source: user.auth_source,
            external_id: user.external_id,
            generated_password,
        }
    }
}

impl From<DomainUser> for UserResponse {
    fn from(user: DomainUser) -> Self {
        Self::from_user(user, None)
    }
}

impl From<DomainGroup> for GroupResponse {
    fn from(group: DomainGroup) -> Self {
        Self {
            id: group.id.to_string(),
            name: group.name,
            external_id: group.external_id,
        }
    }
}

impl From<DomainRole> for RoleResponse {
    fn from(role: DomainRole) -> Self {
        Self {
            id: role.id.to_string(),
            name: role.name,
            description: role.description,
        }
    }
}

impl From<DomainRoleAssignment> for RoleAssignmentResponse {
    fn from(assignment: DomainRoleAssignment) -> Self {
        Self {
            id: assignment.id.to_string(),
            user_id: assignment.user_id.map(|value| value.to_string()),
            group_id: assignment.group_id.map(|value| value.to_string()),
            role_id: assignment.role_id.to_string(),
        }
    }
}

impl From<DomainPolicy> for PolicyResponse {
    fn from(policy: DomainPolicy) -> Self {
        Self {
            id: policy.id.to_string(),
            name: policy.name,
            description: policy.description,
            effect: match policy.effect {
                PolicyEffect::Allow => "allow".to_string(),
                PolicyEffect::Deny => "deny".to_string(),
            },
            actions: policy.actions.iter().map(serialize_enum).collect(),
            resource_types: policy.resource_types.iter().map(serialize_enum).collect(),
            constraints: policy.constraints,
            subjects: policy.subjects,
            resources: policy.resources,
            enabled: policy.enabled,
        }
    }
}

// ============================================================================
// Access vocabulary — the permission dictionary, served from the registry
// ============================================================================

/// Enforcement status of a resource type.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AccessVocabularyStatus {
    /// A first-party endpoint enforces this resource type.
    Enforced,
    /// Declared but not reachable by any enforcement check.
    Reserved,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AccessVocabularyResource {
    pub resource_type: ResourceType,
    pub status: AccessVocabularyStatus,
    /// Actions a human API token may be granted on this resource. Empty means
    /// the resource is session-only (never grantable through a token).
    pub pat_grantable_actions: Vec<Action>,
    /// Object-attribute keys a policy constraint may scope on for this resource
    /// (what its enforcement site feeds into the decision). Empty means the
    /// resource is not attribute-scopable — an `attributes` constraint on it
    /// is rejected at write time. The UI offers exactly these keys.
    pub constraint_attribute_keys: Vec<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AccessVocabularyResponse {
    /// Every action in the model, in canonical order.
    pub actions: Vec<Action>,
    /// Every resource type with its governance flags.
    pub resources: Vec<AccessVocabularyResource>,
}

/// Get the permission vocabulary used by access-control policies and token scopes.
#[utoipa::path(
    get,
    path = "/api/v1/access-control/vocabulary",
    tag = "access-control",
    responses(
        (status = 200, description = "Permission vocabulary", body = AccessVocabularyResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_access_vocabulary(
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
) -> Response {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if request_context.user_id.is_none() {
        return handle_app_error(AppError::Auth("Authentication required".to_string()));
    }
    let resources = ResourceType::iter()
        .map(|resource_type| {
            let governance = resource_governance(resource_type.clone());
            let constraint_attribute_keys =
                crate::contexts::access_control::domain::vocabulary::constraint_attribute_keys(
                    resource_type.clone(),
                )
                .iter()
                .map(|key| (*key).to_string())
                .collect();
            AccessVocabularyResource {
                resource_type,
                status: match governance.status {
                    EnforcementStatus::Enforced => AccessVocabularyStatus::Enforced,
                    EnforcementStatus::Reserved => AccessVocabularyStatus::Reserved,
                },
                pat_grantable_actions: governance.pat_grantable_actions.to_vec(),
                constraint_attribute_keys,
            }
        })
        .collect();
    (
        StatusCode::OK,
        Json(AccessVocabularyResponse {
            actions: Action::iter().collect(),
            resources,
        }),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::{BulkRequest, parse_resource_type};
    use crate::contexts::access_control::domain::ResourceType;

    #[test]
    fn parse_resource_type_accepts_canonical_format() {
        let parsed = parse_resource_type("connection_target");
        assert_eq!(parsed.ok(), Some(ResourceType::ConnectionTarget));
    }

    #[test]
    fn bulk_request_rejects_an_all_empty_batch_with_a_400() {
        use axum::http::StatusCode;

        use crate::api::rest::error_mapping::map_app_error_to_http;

        let absent: BulkRequest<u8> =
            serde_json::from_value(serde_json::json!({})).expect("deserialize absent");
        let empty_arrays: BulkRequest<u8> = serde_json::from_value(serde_json::json!({
            "create": [],
            "update": [],
            "delete": [],
        }))
        .expect("deserialize empty arrays");
        for validated in [absent.validate(), empty_arrays.validate()] {
            assert_eq!(
                map_app_error_to_http(validated.unwrap_err()).status(),
                StatusCode::BAD_REQUEST,
                "an all-empty bulk request must answer 400",
            );
        }

        let non_empty: BulkRequest<u8> =
            serde_json::from_value(serde_json::json!({"delete": ["one"]}))
                .expect("deserialize delete");
        assert!(non_empty.validate().is_ok());
    }
}
