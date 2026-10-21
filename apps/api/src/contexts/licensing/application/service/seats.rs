use chrono::Utc;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::contexts::licensing::domain::policy::{LicensingPolicyEvaluator, LoginSeatDecision};
use crate::contexts::licensing::domain::state::LicenseLifecycleState;
use crate::contexts::licensing::infrastructure::repositories::{
    LicensingRepository, SeatLedgerEvent, SeatLedgerRepository,
};
use crate::contexts::shared::domain::reason_codes;
use crate::errors::AppError;

use super::{DEFAULT_MAX_SEATS, LicensingService, effective_stored_entitlement};

/// License identity and limits resolved for a login attempt, before the seat
/// decision is evaluated.
struct SeatContext {
    instance_id: String,
    lifecycle_state: LicenseLifecycleState,
    max_seats: i32,
    entitlement_expires_at: Option<chrono::DateTime<Utc>>,
}

impl LicensingService {
    pub async fn enforce_seat_on_login(&self, user_id: Uuid) -> Result<(), AppError> {
        self.enforce_seat_on_login_inner(user_id, false).await
    }

    /// Final admission check after the session row exists. It validates that
    /// exact session while holding the seat lock, so a revoked session cannot
    /// allocate a replacement seat, and only a successful login advances the
    /// LIFO timestamp.
    pub async fn confirm_seat_on_login(
        &self,
        user_id: Uuid,
        session_id: Uuid,
    ) -> Result<(), AppError> {
        let mut transaction = self.db_pool.begin().await.map_err(|error| {
            AppError::Database(format!("Failed to begin seat transaction: {error}"))
        })?;
        self.licensing_repo
            .acquire_seat_lock(&mut transaction)
            .await?;
        let active_session: bool = sqlx::query_scalar(
            r#"
            SELECT EXISTS(
                SELECT 1 FROM auth_session
                WHERE id = $1 AND user_id = $2
                  AND revoked_at IS NULL AND expires_at > NOW()
            )
            "#,
        )
        .bind(session_id)
        .bind(user_id)
        .fetch_one(&mut *transaction)
        .await
        .map_err(|error| AppError::Database(format!("Failed to check login session: {error}")))?;
        if !active_session {
            transaction.rollback().await.map_err(|error| {
                AppError::Database(format!("Failed to roll back login confirmation: {error}"))
            })?;
            return Err(AppError::Auth(
                "Session was revoked during login".to_string(),
            ));
        }
        self.admit_seat_locked(transaction, user_id, true).await
    }

    async fn enforce_seat_on_login_inner(
        &self,
        user_id: Uuid,
        touch_existing: bool,
    ) -> Result<(), AppError> {
        let mut transaction = self.db_pool.begin().await.map_err(|error| {
            AppError::Database(format!("Failed to begin seat transaction: {error}"))
        })?;
        self.licensing_repo
            .acquire_seat_lock(&mut transaction)
            .await?;
        self.admit_seat_locked(transaction, user_id, touch_existing)
            .await
    }

    async fn admit_seat_locked(
        &self,
        mut transaction: Transaction<'static, Postgres>,
        user_id: Uuid,
        touch_existing: bool,
    ) -> Result<(), AppError> {
        if self
            .licensing_repo
            .has_active_seat_for_user(&mut transaction, user_id)
            .await?
        {
            if touch_existing {
                sqlx::query(
                    "UPDATE license_seat_assignment SET updated_at = NOW() WHERE user_id = $1 AND revoked_at IS NULL",
                )
                .bind(user_id)
                .execute(&mut *transaction)
                .await?;
            }
            transaction.commit().await.map_err(|error| {
                AppError::Database(format!(
                    "Failed to commit seat transaction for existing seat: {error}"
                ))
            })?;
            return Ok(());
        }

        // Every query stays on the connection holding the advisory transaction lock.
        let now = Utc::now();
        let (mut transaction, context) =
            self.resolve_seat_context(transaction, user_id, now).await?;

        if context.lifecycle_state != LicenseLifecycleState::Suspended
            && context
                .entitlement_expires_at
                .is_some_and(|value| value <= now)
        {
            return self
                .deny_seat(
                    transaction,
                    SeatLedgerEvent {
                        instance_id: context.instance_id.clone(),
                        user_id: Some(user_id),
                        event_type: "seat_denied".to_string(),
                        reason_code: "entitlement_expired".to_string(),
                        metadata: serde_json::json!({"code":"LICENSE_REFRESH_REQUIRED"}),
                    },
                    AppError::Forbidden(
                        "LICENSE_REFRESH_REQUIRED: License entitlement expired".to_string(),
                    ),
                    "expired-entitlement seat denial",
                )
                .await;
        }

        let active_seat_count = self
            .licensing_repo
            .count_active_seats_tx(&mut transaction)
            .await?;
        let decision = LicensingPolicyEvaluator::decide_login(
            context.lifecycle_state,
            false,
            active_seat_count,
            context.max_seats,
        );
        if let LoginSeatDecision::Deny { code, message } = decision {
            return self
                .deny_seat(
                    transaction,
                    SeatLedgerEvent {
                        instance_id: context.instance_id.clone(),
                        user_id: Some(user_id),
                        event_type: "seat_denied".to_string(),
                        reason_code: "login_gate".to_string(),
                        metadata: serde_json::json!({"code": code, "message": message}),
                    },
                    AppError::Forbidden(format!("{code}: {message}")),
                    "seat-limit denial",
                )
                .await;
        }

        self.licensing_repo
            .assign_seat_for_user(&mut transaction, user_id)
            .await?;
        self.seat_ledger_repo
            .append_event(
                &mut transaction,
                &SeatLedgerEvent {
                    instance_id: context.instance_id,
                    user_id: Some(user_id),
                    event_type: "seat_assigned".to_string(),
                    reason_code: "login".to_string(),
                    metadata: serde_json::json!({}),
                },
            )
            .await?;
        transaction.commit().await.map_err(|error| {
            AppError::Database(format!("Failed to commit seat assignment: {error}"))
        })?;
        Ok(())
    }

    /// Resolves the license identity, lifecycle state and seat limits for the
    /// login attempt, returning the transaction so the caller keeps the advisory
    /// lock. A stored entitlement that fails verification is denied here.
    async fn resolve_seat_context(
        &self,
        mut transaction: Transaction<'static, Postgres>,
        user_id: Uuid,
        now: chrono::DateTime<Utc>,
    ) -> Result<(Transaction<'static, Postgres>, SeatContext), AppError> {
        let license_state = self
            .licensing_repo
            .load_latest_state_tx(&mut transaction)
            .await?;
        if let Some(state) = license_state {
            let entitlement = match effective_stored_entitlement(&state) {
                Ok(value) => value,
                Err(error) => {
                    return self
                        .deny_seat(
                            transaction,
                            SeatLedgerEvent {
                                instance_id: state.instance_id.clone(),
                                user_id: Some(user_id),
                                event_type: "seat_denied".to_string(),
                                reason_code: "entitlement_invalid".to_string(),
                                metadata: serde_json::json!({
                                    "code": reason_codes::ENTITLEMENT_BINDING_INVALID,
                                    "error": error.to_string(),
                                }),
                            },
                            AppError::Forbidden(format!(
                                "{}: License entitlement is invalid",
                                reason_codes::ENTITLEMENT_BINDING_INVALID
                            )),
                            "invalid-entitlement seat denial",
                        )
                        .await;
                }
            };
            let lifecycle_state = super::effective_lifecycle_state(&state, &entitlement, now);
            Ok((
                transaction,
                SeatContext {
                    instance_id: state.instance_id,
                    lifecycle_state,
                    max_seats: entitlement.max_seats,
                    entitlement_expires_at: Some(entitlement.entitlement_expires_at),
                },
            ))
        } else {
            let instance_id = self
                .licensing_repo
                .get_or_create_instance_fingerprint_tx(&mut transaction)
                .await?;
            Ok((
                transaction,
                SeatContext {
                    instance_id,
                    lifecycle_state: LicenseLifecycleState::Inactive,
                    max_seats: DEFAULT_MAX_SEATS,
                    entitlement_expires_at: None,
                },
            ))
        }
    }

    /// Records a seat-denial ledger event, commits the transaction and returns
    /// the caller's error. `label` names the denial in the commit failure.
    async fn deny_seat<T>(
        &self,
        mut transaction: Transaction<'static, Postgres>,
        event: SeatLedgerEvent,
        error: AppError,
        label: &str,
    ) -> Result<T, AppError> {
        self.seat_ledger_repo
            .append_event(&mut transaction, &event)
            .await?;
        transaction.commit().await.map_err(|commit_error| {
            AppError::Database(format!("Failed to commit {label}: {commit_error}"))
        })?;
        Err(error)
    }
}
