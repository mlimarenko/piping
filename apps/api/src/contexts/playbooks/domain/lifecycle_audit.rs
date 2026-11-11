//! Audit projection of persisted automation transitions.

use chrono::{DateTime, Utc};
use serde_json::json;
use uuid::Uuid;

use super::{PlaybookRun, PlaybookRunGroup, PlaybookRunStatus};
use crate::contexts::audit::domain::safe_projection::project_metadata;
use crate::contexts::audit::domain::{
    AuditAutomationState, AuditContext, AuditCorrelation, AuditDelegatedPrincipal,
    AuditDelegatedPrincipalKind, AuditEmitter, AuditEmitterKind, AuditEvent, AuditLifecycleState,
    AuditOutcome,
};
use crate::errors::AppError;

pub fn automation_audit_state(status: PlaybookRunStatus) -> AuditLifecycleState {
    AuditLifecycleState::Automation(match status {
        PlaybookRunStatus::Queued => AuditAutomationState::Queued,
        PlaybookRunStatus::Leased => AuditAutomationState::Leased,
        PlaybookRunStatus::Dispatching => AuditAutomationState::Dispatching,
        PlaybookRunStatus::Running => AuditAutomationState::Running,
        PlaybookRunStatus::Succeeded => AuditAutomationState::Succeeded,
        PlaybookRunStatus::Failed => AuditAutomationState::Failed,
        PlaybookRunStatus::Canceled => AuditAutomationState::Canceled,
        PlaybookRunStatus::TimedOut => AuditAutomationState::TimedOut,
        PlaybookRunStatus::Unreported => AuditAutomationState::Unreported,
    })
}

/// Execution cancellation is neither successful execution nor a policy denial.
pub fn automation_execution_outcome(status: PlaybookRunStatus) -> AuditOutcome {
    match status {
        PlaybookRunStatus::Succeeded => AuditOutcome::Succeeded,
        PlaybookRunStatus::Failed | PlaybookRunStatus::TimedOut => AuditOutcome::Failed,
        PlaybookRunStatus::Canceled => AuditOutcome::NotApplicable,
        _ => AuditOutcome::Unknown,
    }
}

/// The caller names the operation and its outcome; status always comes from storage.
pub fn automation_lifecycle_event(
    event_type: &str,
    group: &PlaybookRunGroup,
    run: Option<&PlaybookRun>,
    outcome: AuditOutcome,
    emitter: AuditEmitterKind,
    at: DateTime<Utc>,
) -> Result<AuditEvent, AppError> {
    if run.is_some_and(|run| run.run_group_id != group.id) {
        return Err(AppError::Validation(
            "Audit run does not belong to its group".into(),
        ));
    }
    // These fields are reserved at launch admission. Do not interpret arbitrary
    // metadata.request_id, callback payloads, or mutable names as launch evidence.
    let launch_context = group.metadata.get("audit_context").and_then(|value| {
        let context = serde_json::from_value::<AuditContext>(value.clone()).ok()?;
        (context.validate().is_ok() && serde_json::to_value(&context).ok().as_ref() == Some(value))
            .then_some(context)
    });
    let (actor_id, actor_kind) = if let Some(id) = group.requested_by_user_id {
        let kind = match group
            .metadata
            .get("audit_actor_kind")
            .and_then(|v| v.as_str())
        {
            Some("api_token") if launch_context.is_some() => "api_token",
            _ => "user",
        };
        (Some(id), kind)
    } else if let Some(id) = group.service_principal_id {
        (Some(id), "service_principal")
    } else {
        (None, "system")
    };
    let context = AuditContext {
        outcome,
        state: Some(automation_audit_state(
            run.map_or(group.status, |run| run.status),
        )),
        delegated_principal: group
            .service_principal_id
            .map(|id| AuditDelegatedPrincipal {
                kind: AuditDelegatedPrincipalKind::ServicePrincipal,
                id,
            }),
        emitter: Some(AuditEmitter {
            kind: emitter,
            id: None,
        }),
        correlation: AuditCorrelation {
            request_id: launch_context.and_then(|context| context.correlation.request_id),
            run_id: run.map(|run| run.id),
            run_group_id: Some(group.id),
            ..Default::default()
        },
        ..Default::default()
    };
    AuditEvent {
        id: Uuid::now_v7(),
        event_type: event_type.to_owned(),
        actor_id,
        actor_kind: actor_kind.to_owned(),
        target_type: Some(
            if run.is_some() {
                "playbook_run"
            } else {
                "playbook_run_group"
            }
            .to_owned(),
        ),
        target_id: Some(run.map_or(group.id, |run| run.id)),
        metadata: project_metadata(event_type, &json!({})),
        created_at: at,
    }
    .with_context(context)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contexts::audit::domain::{AuditDecision, AuditDisplaySnapshot};
    use crate::contexts::playbooks::domain::{
        InventorySelection, LaunchOptions, LaunchSpec, PlaybookRunTriggerKind,
    };

    fn group(status: PlaybookRunStatus) -> PlaybookRunGroup {
        let id = Uuid::new_v4();
        let now = Utc::now();
        PlaybookRunGroup {
            id,
            template_id: id,
            launch_spec: LaunchSpec {
                template_id: id,
                project_id: id,
                revision_id: id,
                tree_sha256: String::new(),
                execution_environment_id: id,
                image: String::new(),
                image_digest: String::new(),
                workdir: ".".into(),
                playbook_paths: vec![],
                inventory: InventorySelection::Native {
                    sources: vec![],
                    ssh_record_id: None,
                },
                vars_schema: None,
                options: LaunchOptions {
                    argv: vec![],
                    extra_vars: json!({}),
                    limit: None,
                    env: Default::default(),
                    secret_refs: Default::default(),
                    timeout_seconds: 60,
                },
                targets: vec![],
            },
            target_set_id: None,
            schedule_id: None,
            scheduled_for: None,
            requested_by_user_id: Some(Uuid::new_v4()),
            service_principal_id: None,
            trigger_kind: PlaybookRunTriggerKind::Manual,
            status,
            total_runs: 0,
            succeeded_runs: 0,
            failed_runs: 0,
            canceled_runs: 0,
            timeout_runs: 0,
            unreported_runs: 0,
            created_at: now,
            updated_at: now,
            started_at: None,
            finished_at: None,
            lease_owner: None,
            lease_expires_at: None,
            leased_at: None,
            dispatch_id: None,
            dispatch_attempt: 0,
            cancel_requested_at: None,
            cancel_deadline_at: None,
            cancel_outcome: None,
            metadata: json!({}),
        }
    }

    #[test]
    fn automation_lifecycle_preserves_terminal_state_and_operation_outcome() {
        for (status, outcome) in [
            (PlaybookRunStatus::Succeeded, AuditOutcome::Succeeded),
            (PlaybookRunStatus::Failed, AuditOutcome::Failed),
            (PlaybookRunStatus::Canceled, AuditOutcome::NotApplicable),
            (PlaybookRunStatus::TimedOut, AuditOutcome::Failed),
            (PlaybookRunStatus::Unreported, AuditOutcome::Unknown),
        ] {
            let group = group(status);
            let event = automation_lifecycle_event(
                "playbook.run_group.completed",
                &group,
                None,
                automation_execution_outcome(status),
                AuditEmitterKind::Worker,
                group.updated_at,
            )
            .unwrap();
            let context = event.audit_context();
            assert_eq!(context.state, Some(automation_audit_state(status)));
            assert_eq!(context.outcome, outcome);
            assert_eq!(context.decision, None);
            assert_eq!(context.correlation.run_group_id, Some(group.id));
            assert_eq!(context.correlation.run_id, None);
            assert_eq!(event.actor_id, group.requested_by_user_id);
            assert_eq!(event.target_id, Some(group.id));
        }
        let group = group(PlaybookRunStatus::Queued);
        let event = automation_lifecycle_event(
            "playbook.run_group.retry_scheduled",
            &group,
            None,
            AuditOutcome::Succeeded,
            AuditEmitterKind::Worker,
            group.updated_at,
        )
        .unwrap();
        assert_eq!(
            event.audit_context().state,
            Some(automation_audit_state(PlaybookRunStatus::Queued))
        );
        assert_eq!(
            automation_execution_outcome(group.status),
            AuditOutcome::Unknown
        );
    }

    #[test]
    fn automation_lifecycle_keeps_initiator_delegation_emitter_and_correlation_separate() {
        let mut group = group(PlaybookRunStatus::Running);
        let request_id = Uuid::new_v4();
        let principal_id = Uuid::new_v4();
        group.service_principal_id = Some(principal_id);
        group.metadata = json!({
            "audit_context": AuditContext { decision: Some(AuditDecision::Allowed), correlation: AuditCorrelation { request_id: Some(request_id), ..Default::default() }, ..Default::default() },
            "audit_actor_kind": "api_token",
            "request_id": Uuid::new_v4(),
            "error_message": "private diagnostic",
            "console": "private output",
            "actor_name": "mutable label",
        });
        let event = automation_lifecycle_event(
            "playbook.run_group.dispatched",
            &group,
            None,
            AuditOutcome::Succeeded,
            AuditEmitterKind::Worker,
            group.updated_at,
        )
        .unwrap();
        let context = event.audit_context();
        assert_eq!(event.actor_id, group.requested_by_user_id);
        assert_eq!(event.actor_kind, "api_token");
        assert_eq!(context.delegated_principal.unwrap().id, principal_id);
        assert_eq!(context.emitter.unwrap().kind, AuditEmitterKind::Worker);
        assert_eq!(context.correlation.request_id, Some(request_id));
        assert_eq!(context.correlation.causation_id, None);
        assert_eq!(context.correlation.session_id, None);
        assert_eq!(context.display, AuditDisplaySnapshot::default());
        assert!(event.metadata.get("console").is_none());
        assert!(event.metadata.get("error_message").is_none());
        assert_eq!(context.decision, None);
        group.requested_by_user_id = None;
        let scheduled = automation_lifecycle_event(
            "playbook.run_group.dispatched",
            &group,
            None,
            AuditOutcome::Succeeded,
            AuditEmitterKind::Worker,
            group.updated_at,
        )
        .unwrap();
        assert_eq!(scheduled.actor_id, Some(principal_id));
        assert_eq!(scheduled.actor_kind, "service_principal");
    }
}
