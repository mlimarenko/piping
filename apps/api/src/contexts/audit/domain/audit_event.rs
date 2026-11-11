use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::event_context::{AuditContext, invalid_context};
use crate::errors::AppError;

/// Canonical `actor_kind` for every playbook callback credential.
///
/// Run-bound job tokens and run-group callback tokens are one actor class: both
/// are `pjt_` tokens minted for the automation runner. Their difference is the
/// binding, which the event carries as `delegated_principal` identity plus the
/// `run_id`/`run_group_id` target and correlation — never a second actor kind.
pub const AUDIT_ACTOR_KIND_PLAYBOOK_JOB_TOKEN: &str = "playbook_job_token";

/// Audit event domain model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    /// Unique identifier (UUID)
    pub id: Uuid,
    /// Event type (e.g., "connection.created")
    pub event_type: String,
    /// Actor identifier (user/service)
    pub actor_id: Option<Uuid>,
    /// Actor category (`user`, `service_principal`, `playbook_job_token`, etc.)
    pub actor_kind: String,
    /// Target type (e.g., "connection_session")
    pub target_type: Option<String>,
    /// Target identifier
    pub target_id: Option<Uuid>,
    /// Event metadata
    pub metadata: Value,
    /// Creation timestamp
    pub created_at: DateTime<Utc>,
}

impl AuditEvent {
    /// Attach validated context while preserving all other metadata.
    pub fn with_context(mut self, context: AuditContext) -> Result<Self, AppError> {
        context.validate()?;
        let metadata = self.metadata.as_object_mut().ok_or_else(invalid_context)?;
        let value = serde_json::to_value(context).map_err(|_| invalid_context())?;
        metadata.insert("audit_context".to_owned(), value);
        Ok(self)
    }

    /// Missing, malformed or noncanonical context is unknown, never inferred.
    /// Stored context must equal the full serialized typed representation;
    /// partial/defaulted JSON is not a canonical persisted context.
    pub fn audit_context(&self) -> AuditContext {
        self.parse_audit_context()
            .ok()
            .flatten()
            .unwrap_or_default()
    }

    pub(crate) fn parse_audit_context(&self) -> Result<Option<AuditContext>, AppError> {
        let Some(value) = self.metadata.get("audit_context") else {
            return Ok(None);
        };
        let context: AuditContext =
            serde_json::from_value(value.clone()).map_err(|_| invalid_context())?;
        context.validate()?;
        if serde_json::to_value(&context).map_err(|_| invalid_context())? != *value {
            return Err(invalid_context());
        }
        Ok(Some(context))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contexts::audit::domain::{AuditLifecycleState, AuditOutcome, AuditSessionState};
    use serde_json::json;

    fn event(metadata: Value) -> AuditEvent {
        AuditEvent {
            id: Uuid::nil(),
            event_type: "test".into(),
            actor_id: None,
            actor_kind: "system".into(),
            target_type: None,
            target_id: None,
            metadata,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn context_round_trip_preserves_metadata_and_typed_wire_shape() {
        let context = AuditContext {
            outcome: AuditOutcome::Succeeded,
            state: Some(AuditLifecycleState::Session(AuditSessionState::Active)),
            ..Default::default()
        };
        let event = event(json!({"other": 42}))
            .with_context(context.clone())
            .unwrap();
        assert_eq!(event.metadata["other"], 42);
        assert_eq!(event.metadata["audit_context"]["outcome"], "succeeded");
        assert_eq!(
            event.metadata["audit_context"]["state"],
            json!({"domain":"session", "value":"active"})
        );
        assert_eq!(event.audit_context(), context);
    }

    #[test]
    fn malformed_and_old_context_are_unknown() {
        for metadata in [
            json!({}),
            json!({"audit_context":{"outcome":"succeeded"}}),
            json!({"audit_context":{"outcome":"future"}}),
            json!({"audit_context":{"outcome":"succeeded", "correlation":{"extra":"secret"}}}),
            json!({"audit_context":{"outcome":"succeeded", "state":{"domain":"session", "value":"active", "extra":true}}}),
        ] {
            assert_eq!(event(metadata).audit_context(), AuditContext::default());
        }
    }

    #[test]
    fn valid_but_noncanonical_context_is_rejected_at_write_and_unknown_on_read() {
        let partial = event(json!({"audit_context":{"outcome":"succeeded"}}));
        assert!(partial.parse_audit_context().is_err());
        assert_eq!(partial.audit_context(), AuditContext::default());
        assert!(event(json!({})).parse_audit_context().unwrap().is_none());
    }

    #[test]
    fn labels_are_bounded_and_reject_controls_without_echoing_input() {
        for label in ["x".repeat(257), "bad\nlabel".into(), String::new()] {
            let mut context = AuditContext::default();
            context.display.actor = Some(label.clone());
            let error = event(json!({}))
                .with_context(context)
                .unwrap_err()
                .to_string();
            assert_eq!(error, "Validation error: Invalid audit context");
        }
        let mut context = AuditContext::default();
        context.display.target = Some("x".repeat(256));
        assert!(event(json!({})).with_context(context).is_ok());
    }
}
