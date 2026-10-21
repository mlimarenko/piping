use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::contexts::licensing::domain::state::{LicenseHealthSnapshot, LicenseLifecycleState};
use crate::contexts::licensing::infrastructure::repositories::repository::{
    ActivationContext, EntitlementMetadata, LicenseStateRecord, LicensingRepository,
    RefreshFailure, RefreshSuccess,
};
use crate::contexts::shared::row::col;
use crate::errors::AppError;

#[derive(Debug, Clone)]
pub struct SqlxLicensingRepository {
    pool: PgPool,
}

impl SqlxLicensingRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Enforce a verified signed ceiling while the caller holds the seat lock.
    /// LIFO removes the most recent entrants, not the least recently used seats.
    pub(crate) async fn evict_over_capacity(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        instance_id: &str,
        max_seats: i32,
    ) -> Result<(), AppError> {
        let max_seats = max_seats.max(0);
        let candidates: Vec<(Uuid, Uuid, bool)> = sqlx::query_as(
            r#"
            SELECT lsa.id, lsa.user_id,
                   EXISTS (
                       SELECT 1
                       FROM rbac_role_assignment ra
                       JOIN rbac_role r ON r.id = ra.role_id
                       WHERE r.name = 'admin'
                         AND (
                             ra.user_id = lsa.user_id
                             OR ra.group_id IN (
                                 SELECT gm.group_id
                                 FROM identity_group_membership gm
                                 WHERE gm.user_id = lsa.user_id
                             )
                         )
                   ) AS is_admin
            FROM license_seat_assignment lsa
            JOIN identity_user iu ON iu.id = lsa.user_id
            WHERE lsa.revoked_at IS NULL AND iu.status = 'active'
            ORDER BY lsa.updated_at DESC, lsa.assigned_at DESC, lsa.id DESC
            "#,
        )
        .fetch_all(&mut **tx)
        .await?;

        let max_seats = usize::try_from(max_seats).unwrap_or(0);
        if candidates.len() <= max_seats {
            return Ok(());
        }
        let excess = candidates.len() - max_seats;

        // LIFO among ordinary users, then administrators only if necessary.
        // Keep one administrator while any seat remains, so a strict LIFO
        // reduction can never lock every administrator out of the instance.
        let mut selected: Vec<(Uuid, Uuid)> = Vec::with_capacity(excess);
        if max_seats == 0 {
            selected.extend(candidates.iter().map(|(seat, user, _)| (*seat, *user)));
        } else {
            selected.extend(
                candidates
                    .iter()
                    .filter(|(_, _, is_admin)| !is_admin)
                    .take(excess)
                    .map(|(seat, user, _)| (*seat, *user)),
            );
            let admin_budget = candidates
                .iter()
                .filter(|(_, _, is_admin)| *is_admin)
                .count()
                .saturating_sub(1);
            selected.extend(
                candidates
                    .iter()
                    .filter(|(_, _, is_admin)| *is_admin)
                    .take(admin_budget.min(excess.saturating_sub(selected.len())))
                    .map(|(seat, user, _)| (*seat, *user)),
            );
        }

        for (seat_id, user_id) in selected {
            sqlx::query(
                r#"
                UPDATE license_seat_assignment
                SET revoked_at = NOW(), revoked_reason = 'seat_evicted_over_capacity', updated_at = NOW()
                WHERE id = $1
                "#,
            )
            .bind(seat_id)
            .execute(&mut **tx)
            .await?;
            sqlx::query(
                r#"
                INSERT INTO license_seat_ledger (instance_id, user_id, event_type, reason_code, metadata_json)
                VALUES ($1, $2, 'seat_revoked', 'seat_evicted_over_capacity', $3)
                "#,
            )
            .bind(instance_id)
            .bind(user_id)
            .bind(serde_json::json!({"max_seats": max_seats}))
            .execute(&mut **tx)
            .await?;
            sqlx::query("UPDATE auth_session SET revoked_at = NOW() WHERE user_id = $1 AND revoked_at IS NULL")
                .bind(user_id)
                .execute(&mut **tx)
                .await?;
            sqlx::query(
                r#"
                UPDATE auth_user_api_token SET status = 'revoked', revoked_at = NOW(), updated_at = NOW()
                WHERE user_id = $1 AND status = 'active'
                "#,
            )
            .bind(user_id)
            .execute(&mut **tx)
            .await?;
        }
        Ok(())
    }
}

const INSTANCE_FINGERPRINT_UPSERT_SQL: &str = r#"
    INSERT INTO license_instance_identity (id, instance_fingerprint)
    VALUES (TRUE, 'piping-' || uuidv7()::text)
    ON CONFLICT (id) DO UPDATE
    SET instance_fingerprint = license_instance_identity.instance_fingerprint
    RETURNING instance_fingerprint
"#;

const LATEST_STATE_SQL: &str = r#"
    SELECT
        instance_id,
        status,
        max_seats,
        entitlement_expires_at,
        entitlement_jwt,
        entitlement_version,
        activation_token_encrypted,
        consecutive_failures,
        grace_started_at,
        grace_ends_at
    FROM license_state
    ORDER BY updated_at DESC
    LIMIT 1
"#;

impl LicensingRepository for SqlxLicensingRepository {
    async fn load_instance_fingerprint(&self) -> Result<Option<String>, AppError> {
        sqlx::query_scalar(
            r#"
            SELECT instance_fingerprint
            FROM license_instance_identity
            WHERE id = TRUE
            "#,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| {
            AppError::Database(format!("Failed to load license instance identity: {error}"))
        })
    }

    async fn get_or_create_instance_fingerprint(&self) -> Result<String, AppError> {
        sqlx::query_scalar(INSTANCE_FINGERPRINT_UPSERT_SQL)
            .fetch_one(&self.pool)
            .await
            .map_err(instance_identity_error)
    }

    async fn get_or_create_instance_fingerprint_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<String, AppError> {
        sqlx::query_scalar(INSTANCE_FINGERPRINT_UPSERT_SQL)
            .fetch_one(&mut **tx)
            .await
            .map_err(instance_identity_error)
    }

    async fn store_instance_fingerprint(&self, instance_fingerprint: &str) -> Result<(), AppError> {
        sqlx::query(
            r#"
            INSERT INTO license_instance_identity (id, instance_fingerprint, updated_at)
            VALUES (TRUE, $1, NOW())
            ON CONFLICT (id) DO UPDATE
            SET
                instance_fingerprint = EXCLUDED.instance_fingerprint,
                updated_at = NOW()
            "#,
        )
        .bind(instance_fingerprint)
        .execute(&self.pool)
        .await
        .map_err(|error| {
            AppError::Database(format!(
                "Failed to store license instance identity: {error}"
            ))
        })?;
        Ok(())
    }

    async fn load_latest_state(&self) -> Result<Option<LicenseStateRecord>, AppError> {
        let row = sqlx::query(LATEST_STATE_SQL)
            .fetch_optional(&self.pool)
            .await
            .map_err(latest_state_error)?;
        parse_latest_state_row(row)
    }

    async fn load_latest_state_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<Option<LicenseStateRecord>, AppError> {
        let row = sqlx::query(LATEST_STATE_SQL)
            .fetch_optional(&mut **tx)
            .await
            .map_err(latest_state_error)?;
        parse_latest_state_row(row)
    }

    async fn load_latest_health_snapshot(&self) -> Result<Option<LicenseHealthSnapshot>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT
                status,
                entitlement_expires_at,
                grace_started_at,
                grace_ends_at,
                last_sync_at,
                last_successful_sync_at,
                last_sync_error,
                last_failure_kind,
                consecutive_failures
            FROM license_state
            ORDER BY updated_at DESC
            LIMIT 1
            "#,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| {
            AppError::Database(format!("Failed to load license health snapshot: {error}"))
        })?;

        let Some(row) = row else {
            return Ok(None);
        };

        let status: String = col(&row, "status")?;
        Ok(Some(LicenseHealthSnapshot {
            state: LicenseLifecycleState::parse(&status),
            entitlement_expires_at: col(&row, "entitlement_expires_at")?,
            grace_started_at: col(&row, "grace_started_at")?,
            grace_ends_at: col(&row, "grace_ends_at")?,
            last_sync_at: col(&row, "last_sync_at")?,
            last_successful_sync_at: col(&row, "last_successful_sync_at")?,
            last_sync_error: col(&row, "last_sync_error")?,
            last_failure_kind: col(&row, "last_failure_kind")?,
            consecutive_failures: col(&row, "consecutive_failures")?,
        }))
    }

    async fn load_entitlement_metadata(&self) -> Result<Option<EntitlementMetadata>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT entitlement_version, updated_at
            FROM license_state
            ORDER BY updated_at DESC
            LIMIT 1
            "#,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| {
            AppError::Database(format!(
                "Failed to load entitlement state metadata: {error}"
            ))
        })?;

        let Some(row) = row else {
            return Ok(None);
        };

        Ok(Some(EntitlementMetadata {
            entitlement_version: col(&row, "entitlement_version")?,
            updated_at: col(&row, "updated_at")?,
        }))
    }

    async fn begin_refresh(&self) -> Result<Option<ActivationContext>, AppError> {
        let row = sqlx::query(
            r#"
            UPDATE license_state SET revision = revision + 1
            WHERE instance_id = (SELECT instance_id FROM license_state ORDER BY updated_at DESC LIMIT 1)
            RETURNING
                revision,
                instance_id,
                activation_token_encrypted,
                entitlement_version,
                status,
                consecutive_failures,
                grace_started_at,
                grace_ends_at
            "#,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| {
            AppError::Database(format!(
                "Failed to load license activation context: {error}"
            ))
        })?;

        row.map(|row| {
            Ok(ActivationContext {
                revision: col(&row, "revision")?,
                instance_id: col(&row, "instance_id")?,
                activation_token_encrypted: col(&row, "activation_token_encrypted")?,
                entitlement_version: col(&row, "entitlement_version")?,
                status: col(&row, "status")?,
                consecutive_failures: col(&row, "consecutive_failures")?,
                grace_started_at: col(&row, "grace_started_at")?,
                grace_ends_at: col(&row, "grace_ends_at")?,
            })
        })
        .transpose()
    }

    async fn count_active_seats(&self) -> Result<i64, AppError> {
        sqlx::query_scalar(
            r#"
            SELECT COUNT(*)
            FROM license_seat_assignment lsa
            JOIN identity_user iu ON iu.id = lsa.user_id
            WHERE lsa.revoked_at IS NULL
              AND iu.status = 'active'
            "#,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|error| AppError::Database(format!("Failed to count active seats: {error}")))
    }

    async fn has_active_seat_for_user(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        user_id: Uuid,
    ) -> Result<bool, AppError> {
        sqlx::query_scalar(
            r#"
            SELECT EXISTS(
                SELECT 1
                FROM license_seat_assignment
                WHERE user_id = $1
                  AND revoked_at IS NULL
            )
            "#,
        )
        .bind(user_id)
        .fetch_one(&mut **tx)
        .await
        .map_err(|error| AppError::Database(format!("Failed to check seat assignment: {error}")))
    }

    async fn assign_seat_for_user(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        user_id: Uuid,
    ) -> Result<(), AppError> {
        sqlx::query(
            r#"
            INSERT INTO license_seat_assignment (user_id)
            VALUES ($1)
            ON CONFLICT (user_id) WHERE revoked_at IS NULL DO NOTHING
            "#,
        )
        .bind(user_id)
        .execute(&mut **tx)
        .await
        .map_err(|error| AppError::Database(format!("Failed to assign seat: {error}")))?;
        Ok(())
    }

    async fn count_active_seats_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<i64, AppError> {
        sqlx::query_scalar(
            r#"
            SELECT COUNT(*)
            FROM license_seat_assignment lsa
            JOIN identity_user iu ON iu.id = lsa.user_id
            WHERE lsa.revoked_at IS NULL
              AND iu.status = 'active'
            "#,
        )
        .fetch_one(&mut **tx)
        .await
        .map_err(|error| AppError::Database(format!("Failed to count active seats: {error}")))
    }

    async fn acquire_seat_lock(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<(), AppError> {
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(421_001_i64)
            .execute(&mut **tx)
            .await
            .map_err(|error| AppError::Database(format!("Failed to acquire seat lock: {error}")))?;
        Ok(())
    }

    async fn mark_refresh_failure(
        &self,
        instance_id: &str,
        revision: i64,
        now: DateTime<Utc>,
        failure: RefreshFailure<'_>,
    ) -> Result<(), AppError> {
        let RefreshFailure {
            status,
            error_message,
            failure_kind,
            consecutive_failures,
            grace_started_at,
            grace_ends_at,
        } = failure;
        let mut transaction = self.pool.begin().await?;
        self.acquire_seat_lock(&mut transaction).await?;
        let result = sqlx::query(
            r#"
            UPDATE license_state
            SET status = CASE WHEN status = 'suspended' THEN status ELSE $1 END,
                last_sync_at = $2,
                last_sync_error = CASE WHEN status = 'suspended' THEN last_sync_error ELSE $3 END,
                last_failure_kind = CASE WHEN status = 'suspended' THEN last_failure_kind ELSE $4 END,
                consecutive_failures = $5,
                grace_started_at = $6,
                grace_ends_at = $7,
                updated_at = NOW()
            WHERE instance_id = $8 AND revision = $9
            "#,
        )
        .bind(status)
        .bind(now)
        .bind(error_message)
        .bind(failure_kind)
        .bind(consecutive_failures)
        .bind(grace_started_at)
        .bind(grace_ends_at)
        .bind(instance_id)
        .bind(revision)
        .execute(&mut *transaction)
        .await
        .map_err(|error| {
            AppError::Database(format!("Failed to persist refresh failure: {error}"))
        })?;
        if result.rows_affected() != 1 {
            return Err(AppError::LicenseRefreshSuperseded);
        }
        transaction.commit().await?;
        Ok(())
    }

    async fn mark_refresh_success(&self, success: RefreshSuccess<'_>) -> Result<(), AppError> {
        let RefreshSuccess {
            revision,
            instance_id,
            entitlement_expires_at,
            max_seats,
            entitlement_jwt,
            entitlement_version,
            now,
        } = success;
        let mut transaction = self.pool.begin().await?;
        self.acquire_seat_lock(&mut transaction).await?;
        let result = sqlx::query(
            r#"
            UPDATE license_state
            SET
                status = CASE
                    WHEN $1 < NOW() THEN 'expired'
                    ELSE 'active'
                END,
                max_seats = $2,
                entitlement_jwt = $3,
                entitlement_version = $4,
                entitlement_expires_at = $1,
                last_sync_at = $5,
                last_successful_sync_at = $5,
                last_sync_error = NULL,
                last_failure_kind = NULL,
                consecutive_failures = 0,
                grace_started_at = NULL,
                grace_ends_at = NULL,
                updated_at = NOW()
            WHERE instance_id = $6 AND revision = $7
            "#,
        )
        .bind(entitlement_expires_at)
        .bind(max_seats)
        .bind(entitlement_jwt)
        .bind(entitlement_version)
        .bind(now)
        .bind(instance_id)
        .bind(revision)
        .execute(&mut *transaction)
        .await
        .map_err(|error| {
            AppError::Database(format!("Failed to persist refresh success: {error}"))
        })?;
        if result.rows_affected() != 1 {
            return Err(AppError::LicenseRefreshSuperseded);
        }
        self.evict_over_capacity(&mut transaction, instance_id, max_seats)
            .await?;
        transaction.commit().await?;
        Ok(())
    }
}

fn instance_identity_error(error: sqlx::Error) -> AppError {
    AppError::Database(format!(
        "Failed to get or create license instance identity: {error}"
    ))
}

fn latest_state_error(error: sqlx::Error) -> AppError {
    AppError::Database(format!("Failed to load license state: {error}"))
}

fn parse_latest_state_row(
    row: Option<sqlx::postgres::PgRow>,
) -> Result<Option<LicenseStateRecord>, AppError> {
    row.map(|row| {
        Ok(LicenseStateRecord {
            instance_id: col(&row, "instance_id")?,
            status: col(&row, "status")?,
            max_seats: col(&row, "max_seats")?,
            entitlement_expires_at: col(&row, "entitlement_expires_at")?,
            entitlement_jwt: col(&row, "entitlement_jwt")?,
            entitlement_version: col(&row, "entitlement_version")?,
            activation_token_encrypted: col(&row, "activation_token_encrypted")?,
            consecutive_failures: col(&row, "consecutive_failures")?,
            grace_started_at: col(&row, "grace_started_at")?,
            grace_ends_at: col(&row, "grace_ends_at")?,
        })
    })
    .transpose()
}
