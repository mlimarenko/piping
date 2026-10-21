use chrono::Utc;
use tokio::sync::watch;

use crate::contexts::playbooks::application::RunStreamFrame;
use crate::contexts::playbooks::domain::{
    PlaybookCancelOutcome, PlaybookRunGroup, PlaybookRunStatus,
};
use crate::contexts::playbooks::infrastructure::{
    AnsibleDispatchStatus, AnsibleJobState, AnsibleRunnerClient,
};
use crate::observability::{EventFields, LogLevel, log_error_event, log_event, metrics};

use super::AppState;

const CANCEL_BATCH_LIMIT: i64 = 50;

pub(super) async fn run_playbook_cancel_worker(
    state: AppState,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    metrics::record_worker_heartbeat("playbook_cancel");
    let poll_interval_ms = state.settings.playbook_runtime.run_poll_interval_ms;
    let runner_client = AnsibleRunnerClient::from_settings(&state.settings);
    if let Err(error) = &runner_client {
        log_error_event(
            "playbooks.cancel_worker.runner_client_init_failed",
            "playbook_cancel_worker_error",
            error.to_string(),
            EventFields::default()
                .with_operation("playbook_cancel_worker_init")
                .with_status("failed"),
        );
    }
    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(poll_interval_ms));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    break;
                }
            }
            _ = ticker.tick() => {
                metrics::record_worker_heartbeat("playbook_cancel");
                run_cancel_worker_tick(&state, runner_client.as_ref().ok()).await;
            }
        }
    }
}

async fn run_cancel_worker_tick(state: &AppState, runner_client: Option<&AnsibleRunnerClient>) {
    let pending = match state
        .services
        .playbooks
        .list_pending_run_group_cancels(CANCEL_BATCH_LIMIT)
        .await
    {
        Ok(pending) => pending,
        Err(error) => {
            log_error_event(
                "playbooks.cancel_worker.load_failed",
                "playbook_cancel_worker_error",
                error.to_string(),
                EventFields::default()
                    .with_operation("playbook_cancel_worker_load")
                    .with_status("failed"),
            );
            return;
        }
    };

    // Three separate questions per group, kept separate: did the runner stop,
    // what does that make the outcome, and what has to be written down. Inlined
    // they read as one long branch and the middle question — the only one with
    // a policy in it — disappears into the noise.
    for group in pending {
        let Some(dispatch_id) = group.dispatch_id else {
            continue;
        };
        let now = Utc::now();
        let runner_confirmed = runner_confirmed_stop(runner_client, &group, dispatch_id).await;
        let Some(outcome) = resolve_cancel_outcome(runner_confirmed, &group, now) else {
            continue;
        };
        finalize_cancel(state, &group, dispatch_id, outcome, now).await;
    }
}

/// Whether the runner has evidenced that this dispatch is over.
///
/// Every uncertain answer is `false`: an unreachable runner, a receipt for a
/// different dispatch, no receipt at all. Reporting `canceled` over a process
/// that may still be changing hosts is the failure this worker exists to
/// prevent, so silence never counts as confirmation.
async fn runner_confirmed_stop(
    runner_client: Option<&AnsibleRunnerClient>,
    group: &PlaybookRunGroup,
    dispatch_id: uuid::Uuid,
) -> bool {
    let Some(client) = runner_client else {
        return false;
    };
    match client.cancel_dispatch(dispatch_id).await {
        Ok(Some(receipt)) if runner_receipt_matches_group(&receipt, group) => {
            runner_state_is_terminal(receipt.status)
        }
        Ok(Some(_)) => {
            log_error_event(
                "playbooks.cancel_worker.runner_receipt_mismatch",
                "playbook_cancel_worker_error",
                "Runner cancellation receipt did not match the database dispatch".to_string(),
                cancel_worker_fields("playbook_cancel_worker_request", group, dispatch_id)
                    .with_status("retrying"),
            );
            false
        }
        Ok(None) => false,
        Err(error) => {
            log_error_event(
                "playbooks.cancel_worker.runner_request_failed",
                "playbook_cancel_worker_error",
                error.to_string(),
                cancel_worker_fields("playbook_cancel_worker_request", group, dispatch_id)
                    .with_status("retrying"),
            );
            false
        }
    }
}

/// Whether the runner receipt belongs to this database dispatch attempt.
fn runner_receipt_matches_group(receipt: &AnsibleDispatchStatus, group: &PlaybookRunGroup) -> bool {
    receipt.receipt.run_group_id == group.id
        && receipt.receipt.dispatch_attempt == group.dispatch_attempt
}

/// The outcome to record, or `None` to keep waiting.
///
/// Waiting is the default: a cancel resolves either because the runner
/// confirmed it stopped, or because the deadline the operator was promised has
/// passed and we say so plainly rather than claiming success.
fn resolve_cancel_outcome(
    runner_confirmed: bool,
    group: &PlaybookRunGroup,
    now: chrono::DateTime<Utc>,
) -> Option<PlaybookCancelOutcome> {
    if runner_confirmed {
        return Some(PlaybookCancelOutcome::RunnerConfirmed);
    }
    group
        .cancel_deadline_at
        .is_some_and(|deadline| now >= deadline)
        .then_some(PlaybookCancelOutcome::DeadlineExpired)
}

async fn finalize_cancel(
    state: &AppState,
    group: &PlaybookRunGroup,
    dispatch_id: uuid::Uuid,
    outcome: PlaybookCancelOutcome,
    now: chrono::DateTime<Utc>,
) {
    match state
        .services
        .playbooks
        .finalize_run_group_cancel(group.id, dispatch_id, outcome, now)
        .await
    {
        Ok(Some(finalized)) => {
            for run in &finalized.canceled_runs {
                state.run_event_broadcaster.publish(
                    run.id,
                    RunStreamFrame::Completion {
                        run_id: run.id,
                        status: PlaybookRunStatus::Canceled.as_str().to_string(),
                        exit_code: None,
                        error_message: run.error_message.clone(),
                    },
                );
            }
            // A deadline expiry is a warning, not news: it means the runner
            // never confirmed and we stopped waiting.
            let level = if outcome == PlaybookCancelOutcome::DeadlineExpired {
                LogLevel::Warning
            } else {
                LogLevel::Info
            };
            log_event(
                level,
                "playbooks.cancel_worker.finalized",
                cancel_worker_fields("playbook_cancel_worker_finalize", group, dispatch_id)
                    .with_status(outcome.as_str()),
            );
        }
        Ok(None) => {}
        Err(error) => log_error_event(
            "playbooks.cancel_worker.finalize_failed",
            "playbook_cancel_worker_error",
            error.to_string(),
            cancel_worker_fields("playbook_cancel_worker_finalize", group, dispatch_id)
                .with_status("failed"),
        ),
    }
}

fn cancel_worker_fields(
    operation: &str,
    group: &PlaybookRunGroup,
    dispatch_id: uuid::Uuid,
) -> EventFields {
    EventFields::default()
        .with_operation(operation)
        .with_resource("playbook_run_group", group.id.to_string())
        .with_context("dispatch_id", dispatch_id.to_string())
}

fn runner_state_is_terminal(state: AnsibleJobState) -> bool {
    matches!(
        state,
        AnsibleJobState::Succeeded
            | AnsibleJobState::Failed
            | AnsibleJobState::TimedOut
            | AnsibleJobState::Canceled
    )
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, TimeZone};
    use serde_json::json;

    use super::*;
    use crate::contexts::playbooks::domain::PlaybookRunTriggerKind;
    use crate::contexts::playbooks::infrastructure::AnsibleDispatchReceipt;

    fn run_group(
        id: uuid::Uuid,
        dispatch_attempt: i32,
        cancel_deadline_at: Option<chrono::DateTime<Utc>>,
    ) -> PlaybookRunGroup {
        let now = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        PlaybookRunGroup {
            id,
            playbook_id: uuid::Uuid::from_u128(0x100),
            playbook_version_id: uuid::Uuid::from_u128(0x101),
            target_set_id: None,
            schedule_id: None,
            scheduled_for: None,
            requested_by_user_id: None,
            service_principal_id: None,
            trigger_kind: PlaybookRunTriggerKind::Manual,
            status: PlaybookRunStatus::Running,
            total_runs: 1,
            succeeded_runs: 0,
            failed_runs: 0,
            canceled_runs: 0,
            timeout_runs: 0,
            created_at: now,
            updated_at: now,
            started_at: Some(now),
            finished_at: None,
            lease_owner: None,
            lease_expires_at: None,
            leased_at: None,
            dispatch_id: Some(uuid::Uuid::from_u128(0x200)),
            dispatch_attempt,
            cancel_requested_at: Some(now),
            cancel_deadline_at,
            cancel_outcome: None,
            metadata: json!({}),
        }
    }

    #[test]
    fn cancel_outcome_honors_confirmation_and_deadline_boundaries() {
        let now = Utc.timestamp_opt(1_700_000_100, 0).unwrap();
        let group_without_deadline = run_group(uuid::Uuid::from_u128(1), 3, None);
        assert_eq!(
            resolve_cancel_outcome(true, &group_without_deadline, now),
            Some(PlaybookCancelOutcome::RunnerConfirmed)
        );
        assert_eq!(
            resolve_cancel_outcome(false, &group_without_deadline, now),
            None
        );

        let before_deadline = run_group(
            uuid::Uuid::from_u128(2),
            3,
            Some(now + Duration::seconds(1)),
        );
        assert_eq!(resolve_cancel_outcome(false, &before_deadline, now), None);

        let at_deadline = run_group(uuid::Uuid::from_u128(3), 3, Some(now));
        assert_eq!(
            resolve_cancel_outcome(false, &at_deadline, now),
            Some(PlaybookCancelOutcome::DeadlineExpired)
        );

        let after_deadline = run_group(
            uuid::Uuid::from_u128(4),
            3,
            Some(now - Duration::seconds(1)),
        );
        assert_eq!(
            resolve_cancel_outcome(false, &after_deadline, now),
            Some(PlaybookCancelOutcome::DeadlineExpired)
        );
    }

    #[test]
    fn runner_receipt_requires_matching_identity_and_classifies_terminal_state() {
        let group_id = uuid::Uuid::from_u128(0x300);
        let group = run_group(group_id, 7, None);
        let dispatch_id = uuid::Uuid::from_u128(0x301);
        let status = |run_group_id, dispatch_attempt, state| AnsibleDispatchStatus {
            receipt: AnsibleDispatchReceipt {
                dispatch_id,
                run_group_id,
                dispatch_attempt,
            },
            status: state,
            rc: None,
        };

        for state in [
            AnsibleJobState::Succeeded,
            AnsibleJobState::Failed,
            AnsibleJobState::TimedOut,
            AnsibleJobState::Canceled,
        ] {
            let receipt = status(group_id, 7, state);
            assert!(runner_receipt_matches_group(&receipt, &group));
            assert!(runner_state_is_terminal(receipt.status));
        }
        for state in [
            AnsibleJobState::Accepted,
            AnsibleJobState::Running,
            AnsibleJobState::Canceling,
        ] {
            let receipt = status(group_id, 7, state);
            assert!(runner_receipt_matches_group(&receipt, &group));
            assert!(!runner_state_is_terminal(receipt.status));
        }
        assert!(!runner_receipt_matches_group(
            &status(uuid::Uuid::from_u128(0x302), 7, AnsibleJobState::Canceled),
            &group
        ));
        assert!(!runner_receipt_matches_group(
            &status(group_id, 8, AnsibleJobState::Canceled),
            &group
        ));
    }

    #[tokio::test]
    async fn missing_runner_never_confirms_that_dispatch_stopped() {
        let group = run_group(uuid::Uuid::from_u128(0x400), 1, None);
        assert!(
            !runner_confirmed_stop(None, &group, uuid::Uuid::from_u128(0x401)).await,
            "an unavailable runner leaves the dispatch outcome uncertain"
        );
    }
}
