use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::contexts::licensing::domain::state::LicenseHealthSnapshot;
use crate::errors::AppError;

#[derive(Debug, Clone)]
pub struct EntitlementMetadata {
    pub entitlement_version: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct LicenseStateRecord {
    pub instance_id: String,
    pub status: String,
    pub max_seats: i32,
    pub entitlement_expires_at: DateTime<Utc>,
    pub entitlement_jwt: String,
    pub entitlement_version: String,
    pub activation_token_encrypted: String,
    pub consecutive_failures: i32,
    pub grace_started_at: Option<DateTime<Utc>>,
    pub grace_ends_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct ActivationContext {
    pub revision: i64,
    pub instance_id: String,
    pub activation_token_encrypted: String,
    pub entitlement_version: String,
    pub status: String,
    pub consecutive_failures: i32,
    pub grace_started_at: Option<DateTime<Utc>>,
    pub grace_ends_at: Option<DateTime<Utc>>,
}

/// Columns written when a license refresh attempt fails.
pub struct RefreshFailure<'a> {
    pub status: &'a str,
    pub error_message: &'a str,
    pub failure_kind: &'a str,
    pub consecutive_failures: i32,
    pub grace_started_at: Option<DateTime<Utc>>,
    pub grace_ends_at: Option<DateTime<Utc>>,
}

/// Columns written when a license refresh attempt succeeds.
pub struct RefreshSuccess<'a> {
    pub revision: i64,
    pub instance_id: &'a str,
    pub entitlement_expires_at: DateTime<Utc>,
    pub max_seats: i32,
    pub entitlement_jwt: &'a str,
    pub entitlement_version: &'a str,
    pub now: DateTime<Utc>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait LicensingRepository: Send + Sync {
    async fn load_instance_fingerprint(&self) -> Result<Option<String>, AppError>;
    async fn get_or_create_instance_fingerprint(&self) -> Result<String, AppError>;
    /// Same upsert as `get_or_create_instance_fingerprint`, on an open
    /// transaction — see `load_latest_state_tx` for why the seat path may not
    /// reach for a second pool connection.
    async fn get_or_create_instance_fingerprint_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
    ) -> Result<String, AppError>;
    async fn store_instance_fingerprint(&self, instance_fingerprint: &str) -> Result<(), AppError>;
    async fn load_latest_state(&self) -> Result<Option<LicenseStateRecord>, AppError>;
    /// Same read as `load_latest_state`, but on an open transaction.
    ///
    /// The seat decision runs inside a transaction that already holds the seat
    /// advisory lock. Reading through the pool there would check out a SECOND
    /// connection while the first is still held, which deadlocks against a
    /// saturated pool until the acquire timeout — the login then fails with a
    /// database error instead of a seat verdict.
    async fn load_latest_state_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
    ) -> Result<Option<LicenseStateRecord>, AppError>;
    async fn load_latest_health_snapshot(&self) -> Result<Option<LicenseHealthSnapshot>, AppError>;
    async fn load_entitlement_metadata(&self) -> Result<Option<EntitlementMetadata>, AppError>;
    async fn begin_refresh(&self) -> Result<Option<ActivationContext>, AppError>;
    async fn count_active_seats(&self) -> Result<i64, AppError>;
    async fn has_active_seat_for_user(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        user_id: Uuid,
    ) -> Result<bool, AppError>;
    async fn assign_seat_for_user(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        user_id: Uuid,
    ) -> Result<(), AppError>;
    async fn count_active_seats_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
    ) -> Result<i64, AppError>;
    async fn acquire_seat_lock(&self, tx: &mut Transaction<'_, Postgres>) -> Result<(), AppError>;
    async fn mark_refresh_failure(
        &self,
        instance_id: &str,
        revision: i64,
        now: DateTime<Utc>,
        failure: RefreshFailure<'_>,
    ) -> Result<(), AppError>;
    async fn mark_refresh_success(&self, success: RefreshSuccess<'_>) -> Result<(), AppError>;
}

#[derive(Debug, Clone)]
pub struct SeatLedgerEvent {
    pub instance_id: String,
    pub user_id: Option<Uuid>,
    pub event_type: String,
    pub reason_code: String,
    pub metadata: Value,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait SeatLedgerRepository: Send + Sync {
    async fn append_event(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        event: &SeatLedgerEvent,
    ) -> Result<(), AppError>;
}
