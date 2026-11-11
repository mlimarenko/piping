use serde::{Deserialize, Serialize};
use strum::IntoEnumIterator;
use uuid::Uuid;

use crate::errors::AppError;

/// Maximum UTF-8 byte length of a server-derived display label.
pub const MAX_AUDIT_LABEL_BYTES: usize = 256;

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    Default,
    utoipa::ToSchema,
    strum::EnumIter,
)]
#[serde(rename_all = "snake_case")]
pub enum AuditOutcome {
    Succeeded,
    Failed,
    Denied,
    #[default]
    Unknown,
    NotApplicable,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema, strum::EnumIter,
)]
#[serde(rename_all = "snake_case")]
pub enum AuditDecision {
    Allowed,
    Denied,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema, strum::EnumIter,
)]
#[serde(rename_all = "snake_case")]
pub enum AuditSessionState {
    Pending,
    Active,
    Disconnected,
    Terminated,
    Failed,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema, strum::EnumIter,
)]
#[serde(rename_all = "snake_case")]
pub enum AuditTransportState {
    Connected,
    Disconnected,
    Detached,
    Failed,
}

/// Values correspond to the playbook run status vocabulary.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema, strum::EnumIter,
)]
#[serde(rename_all = "snake_case")]
pub enum AuditAutomationState {
    Queued,
    Leased,
    Dispatching,
    Running,
    Succeeded,
    Failed,
    Canceled,
    TimedOut,
    /// The run finished without a valid terminal structured host result.
    Unreported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(
    tag = "domain",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum AuditLifecycleState {
    Session(AuditSessionState),
    Transport(AuditTransportState),
    Automation(AuditAutomationState),
}

/// Canonical wire values of a unit enum, derived from its serde representation.
pub(crate) fn serialized_values<T: IntoEnumIterator + Serialize>() -> Vec<String> {
    T::iter()
        .map(|value| {
            serde_json::to_value(value)
                .expect("audit enum serialization is infallible")
                .as_str()
                .expect("audit unit enum must serialize as a string")
                .to_owned()
        })
        .collect()
}

/// All typed lifecycle variants and their actual serialized domain/value pairs.
pub(crate) fn serialized_lifecycle_states() -> Vec<(String, String)> {
    AuditSessionState::iter()
        .map(AuditLifecycleState::Session)
        .chain(AuditTransportState::iter().map(AuditLifecycleState::Transport))
        .chain(AuditAutomationState::iter().map(AuditLifecycleState::Automation))
        .map(|state| {
            let value =
                serde_json::to_value(state).expect("audit lifecycle serialization is infallible");
            (
                value["domain"]
                    .as_str()
                    .expect("audit lifecycle domain is a string")
                    .to_owned(),
                value["value"]
                    .as_str()
                    .expect("audit lifecycle value is a string")
                    .to_owned(),
            )
        })
        .collect()
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema, strum::EnumIter,
)]
#[serde(rename_all = "snake_case")]
pub enum AuditDelegatedPrincipalKind {
    ServicePrincipal,
    PlaybookJobToken,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AuditDelegatedPrincipal {
    pub kind: AuditDelegatedPrincipalKind,
    pub id: Uuid,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema, strum::EnumIter,
)]
#[serde(rename_all = "snake_case")]
pub enum AuditEmitterKind {
    Api,
    Worker,
    Proxy,
    System,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AuditEmitter {
    pub kind: AuditEmitterKind,
    pub id: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default, utoipa::ToSchema)]
#[serde(default, deny_unknown_fields)]
pub struct AuditCorrelation {
    pub request_id: Option<Uuid>,
    pub session_id: Option<Uuid>,
    pub run_id: Option<Uuid>,
    pub run_group_id: Option<Uuid>,
    pub causation_id: Option<Uuid>,
}

/// Server-derived event-time labels; never identities or free-form summaries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default, utoipa::ToSchema)]
#[serde(default, deny_unknown_fields)]
pub struct AuditDisplaySnapshot {
    pub actor: Option<String>,
    pub target: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AuditContext {
    pub outcome: AuditOutcome,
    pub decision: Option<AuditDecision>,
    pub state: Option<AuditLifecycleState>,
    pub delegated_principal: Option<AuditDelegatedPrincipal>,
    pub emitter: Option<AuditEmitter>,
    #[serde(default)]
    pub correlation: AuditCorrelation,
    #[serde(default)]
    pub display: AuditDisplaySnapshot,
}

impl AuditContext {
    /// Context for a known API operation result, not a read-time inference.
    pub fn api(outcome: AuditOutcome, request_id: Option<&str>) -> Self {
        Self {
            outcome,
            emitter: Some(AuditEmitter {
                kind: AuditEmitterKind::Api,
                id: None,
            }),
            correlation: AuditCorrelation {
                request_id: request_id.and_then(|value| Uuid::parse_str(value).ok()),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    pub(crate) fn validate(&self) -> Result<(), AppError> {
        for label in [&self.display.actor, &self.display.target]
            .into_iter()
            .flatten()
        {
            if label.is_empty()
                || label.len() > MAX_AUDIT_LABEL_BYTES
                || label.chars().any(char::is_control)
            {
                return Err(invalid_context());
            }
        }
        Ok(())
    }
}

pub(crate) fn invalid_context() -> AppError {
    AppError::Validation("Invalid audit context".to_owned())
}
