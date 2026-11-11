// Repository for `provider_request_journal`.
//
// Storage contract:
// - INSERT-only on new attempts (`insert_prepared`).
// - Forward-only state transitions enforced at the repository layer:
//     prepared      -> sent_unknown | failed_no_send
//     sent_unknown  -> succeeded | failed_after_send
//   Backward transitions and side-jumps return AppError::Validation.
// - `payload_hash` is SHA-256 hex of canonicalized payload — never raw
//   payload.
//
// Scope: INSERT/probe + forward-only state-transition methods, plus TTL
// pruning driven by the journal-prune worker.

use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::contexts::sync::domain::{JournalState, ProviderRequestJournalEntry};
use crate::errors::AppError;

/// Result of an inbound webhook echo-dedup probe.
#[derive(Debug, Clone)]
pub struct JournalEchoMatch {
    pub journal_id: Uuid,
    pub state: JournalState,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait ProviderRequestJournalRepository: Send + Sync {
    /// INSERT a new row with `state='prepared'`. Caller is responsible for
    /// ensuring `(provider_id, outbound_event_uuid, attempt_no)` is unique
    /// (the table-level UNIQUE constraint is the canonical guarantee).
    async fn insert_prepared(&self, entry: &ProviderRequestJournalEntry) -> Result<(), AppError>;

    /// Transition a row to `sent_unknown` (sets `sent_at`).
    /// Rejected unless current state is `prepared`.
    async fn transition_to_sent_unknown(
        &self,
        journal_id: Uuid,
        sent_at: DateTime<Utc>,
    ) -> Result<(), AppError>;

    /// Transition a row to `succeeded` (sets `completed_at`, optional
    /// `netbox_revision_observed`, optional `http_status`).
    /// Rejected unless current state is `sent_unknown` or `prepared`.
    async fn transition_to_succeeded(
        &self,
        journal_id: Uuid,
        netbox_revision_observed: Option<i32>,
        http_status: Option<i32>,
    ) -> Result<(), AppError>;

    /// Transition a row to `failed_no_send`. Rejected unless current state
    /// is `prepared`.
    async fn transition_to_failed_no_send(
        &self,
        journal_id: Uuid,
        last_error: &str,
    ) -> Result<(), AppError>;

    /// Transition a row to `failed_after_send`. Rejected unless current
    /// state is `sent_unknown`.
    async fn transition_to_failed_after_send(
        &self,
        journal_id: Uuid,
        http_status: Option<i32>,
        last_error: &str,
    ) -> Result<(), AppError>;

    /// Echo-dedup probe.
    /// Returns the FIRST eligible row with `created_at < received_at` and
    /// `state IN ('prepared','sent_unknown','succeeded')`.
    async fn probe_for_echo_dedup(
        &self,
        provider_id: Uuid,
        model: &str,
        entity_identity: &str,
        payload_hash: &str,
        received_at: DateTime<Utc>,
    ) -> Result<Option<JournalEchoMatch>, AppError>;

    /// Tertiary fallback: probe by `payload_hash` within a ±window_seconds
    /// time band. Used only after the primary journal probe AND the
    /// stamp-based fallback both miss.
    async fn probe_by_hash_window(
        &self,
        provider_id: Uuid,
        model: &str,
        payload_hash: &str,
        received_at: DateTime<Utc>,
        window_seconds: i64,
    ) -> Result<Option<JournalEchoMatch>, AppError>;

    /// Fetch the maximum `attempt_no` for a given `outbound_event_uuid` so
    /// the publisher can issue the next attempt as `attempt_no + 1`.
    async fn max_attempt_no(
        &self,
        provider_id: Uuid,
        outbound_event_uuid: Uuid,
    ) -> Result<i32, AppError>;

    async fn get(&self, journal_id: Uuid) -> Result<Option<ProviderRequestJournalEntry>, AppError>;

    /// Prune terminal-state journal entries older than `days` days.
    /// Only `succeeded`, `failed_after_send`, and `failed_no_send` rows are
    /// pruned — `prepared` and `sent_unknown` are NEVER pruned (they are
    /// in-flight or unresolved). Returns count of rows deleted.
    async fn prune_older_than(&self, days: i64) -> Result<u64, AppError>;
}

#[derive(Clone)]
pub struct SqlxProviderRequestJournalRepository {
    pool: PgPool,
}

impl SqlxProviderRequestJournalRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn parse_row(row: &sqlx::postgres::PgRow) -> Result<ProviderRequestJournalEntry, AppError> {
    let state_text: String = row.try_get("state")?;
    let state = JournalState::from_wire(&state_text).ok_or_else(|| {
        AppError::Internal(format!(
            "provider_request_journal row has invalid state: {state_text}"
        ))
    })?;
    Ok(ProviderRequestJournalEntry {
        id: row.try_get("id")?,
        provider_id: row.try_get("provider_id")?,
        outbound_event_uuid: row.try_get("outbound_event_uuid")?,
        attempt_no: row.try_get("attempt_no")?,
        state,
        model: row.try_get("model")?,
        entity_identity: row.try_get("entity_identity")?,
        payload_hash: row.try_get("payload_hash")?,
        netbox_revision_observed: row.try_get("netbox_revision_observed")?,
        stamped_external_request_id: row.try_get("stamped_external_request_id")?,
        http_status: row.try_get("http_status")?,
        created_at: row.try_get("created_at")?,
        sent_at: row.try_get("sent_at")?,
        completed_at: row.try_get("completed_at")?,
        last_error: row.try_get("last_error")?,
    })
}

impl ProviderRequestJournalRepository for SqlxProviderRequestJournalRepository {
    async fn insert_prepared(&self, entry: &ProviderRequestJournalEntry) -> Result<(), AppError> {
        if entry.state != JournalState::Prepared {
            return Err(AppError::Validation(format!(
                "insert_prepared requires state='prepared', got '{}'",
                entry.state.as_str()
            )));
        }
        sqlx::query(
            r#"
            INSERT INTO provider_request_journal (
                id, provider_id, outbound_event_uuid, attempt_no, state,
                model, entity_identity, payload_hash,
                netbox_revision_observed, stamped_external_request_id, http_status,
                created_at, sent_at, completed_at, last_error
            )
            VALUES ($1, $2, $3, $4, 'prepared', $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
            "#,
        )
        .bind(entry.id)
        .bind(entry.provider_id)
        .bind(entry.outbound_event_uuid)
        .bind(entry.attempt_no)
        .bind(&entry.model)
        .bind(&entry.entity_identity)
        .bind(&entry.payload_hash)
        .bind(entry.netbox_revision_observed)
        .bind(entry.stamped_external_request_id.as_deref())
        .bind(entry.http_status)
        .bind(entry.created_at)
        .bind(entry.sent_at)
        .bind(entry.completed_at)
        .bind(entry.last_error.as_deref())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn transition_to_sent_unknown(
        &self,
        journal_id: Uuid,
        sent_at: DateTime<Utc>,
    ) -> Result<(), AppError> {
        let result = sqlx::query(
            r#"
            UPDATE provider_request_journal
            SET state = 'sent_unknown',
                sent_at = $2
            WHERE id = $1 AND state = 'prepared'
            "#,
        )
        .bind(journal_id)
        .bind(sent_at)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(AppError::Validation(
                "Journal row not found or not in 'prepared' state".to_string(),
            ));
        }
        Ok(())
    }

    async fn transition_to_succeeded(
        &self,
        journal_id: Uuid,
        netbox_revision_observed: Option<i32>,
        http_status: Option<i32>,
    ) -> Result<(), AppError> {
        let result = sqlx::query(
            r#"
            UPDATE provider_request_journal
            SET state = 'succeeded',
                completed_at = $2,
                netbox_revision_observed = COALESCE($3, netbox_revision_observed),
                http_status = COALESCE($4, http_status)
            WHERE id = $1 AND state IN ('sent_unknown', 'prepared')
            "#,
        )
        .bind(journal_id)
        .bind(Utc::now())
        .bind(netbox_revision_observed)
        .bind(http_status)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(AppError::Validation(
                "Journal row not found or not in 'sent_unknown'/'prepared' state".to_string(),
            ));
        }
        Ok(())
    }

    async fn transition_to_failed_no_send(
        &self,
        journal_id: Uuid,
        last_error: &str,
    ) -> Result<(), AppError> {
        let result = sqlx::query(
            r#"
            UPDATE provider_request_journal
            SET state = 'failed_no_send',
                completed_at = $2,
                last_error = $3
            WHERE id = $1 AND state = 'prepared'
            "#,
        )
        .bind(journal_id)
        .bind(Utc::now())
        .bind(last_error)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(AppError::Validation(
                "Journal row not found or not in 'prepared' state".to_string(),
            ));
        }
        Ok(())
    }

    async fn transition_to_failed_after_send(
        &self,
        journal_id: Uuid,
        http_status: Option<i32>,
        last_error: &str,
    ) -> Result<(), AppError> {
        let result = sqlx::query(
            r#"
            UPDATE provider_request_journal
            SET state = 'failed_after_send',
                completed_at = $2,
                http_status = COALESCE($3, http_status),
                last_error = $4
            WHERE id = $1 AND state = 'sent_unknown'
            "#,
        )
        .bind(journal_id)
        .bind(Utc::now())
        .bind(http_status)
        .bind(last_error)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(AppError::Validation(
                "Journal row not found or not in 'sent_unknown' state".to_string(),
            ));
        }
        Ok(())
    }

    async fn probe_for_echo_dedup(
        &self,
        provider_id: Uuid,
        model: &str,
        entity_identity: &str,
        payload_hash: &str,
        received_at: DateTime<Utc>,
    ) -> Result<Option<JournalEchoMatch>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, state
            FROM provider_request_journal
            WHERE provider_id = $1
              AND model = $2
              AND entity_identity = $3
              AND payload_hash = $4
              AND state IN ('prepared', 'sent_unknown', 'succeeded')
              AND created_at < $5
            ORDER BY created_at DESC
            LIMIT 1
            "#,
        )
        .bind(provider_id)
        .bind(model)
        .bind(entity_identity)
        .bind(payload_hash)
        .bind(received_at)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(row) => {
                let state_text: String = row.try_get("state")?;
                let state = JournalState::from_wire(&state_text).ok_or_else(|| {
                    AppError::Internal(format!("Invalid journal state: {state_text}"))
                })?;
                Ok(Some(JournalEchoMatch {
                    journal_id: row.try_get("id")?,
                    state,
                }))
            }
            None => Ok(None),
        }
    }

    async fn probe_by_hash_window(
        &self,
        provider_id: Uuid,
        model: &str,
        payload_hash: &str,
        received_at: DateTime<Utc>,
        window_seconds: i64,
    ) -> Result<Option<JournalEchoMatch>, AppError> {
        let lower = received_at - chrono::Duration::seconds(window_seconds);
        let row = sqlx::query(
            r#"
            SELECT id, state
            FROM provider_request_journal
            WHERE provider_id = $1
              AND model = $2
              AND payload_hash = $3
              AND state IN ('prepared', 'sent_unknown', 'succeeded')
              AND created_at >= $4
              AND created_at < $5
            ORDER BY created_at DESC
            LIMIT 1
            "#,
        )
        .bind(provider_id)
        .bind(model)
        .bind(payload_hash)
        .bind(lower)
        .bind(received_at)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(row) => {
                let state_text: String = row.try_get("state")?;
                let state = JournalState::from_wire(&state_text).ok_or_else(|| {
                    AppError::Internal(format!("Invalid journal state: {state_text}"))
                })?;
                Ok(Some(JournalEchoMatch {
                    journal_id: row.try_get("id")?,
                    state,
                }))
            }
            None => Ok(None),
        }
    }

    async fn max_attempt_no(
        &self,
        provider_id: Uuid,
        outbound_event_uuid: Uuid,
    ) -> Result<i32, AppError> {
        let row = sqlx::query(
            r#"
            SELECT COALESCE(MAX(attempt_no), 0) AS max_attempt
            FROM provider_request_journal
            WHERE provider_id = $1 AND outbound_event_uuid = $2
            "#,
        )
        .bind(provider_id)
        .bind(outbound_event_uuid)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.try_get::<i32, _>("max_attempt")?)
    }

    async fn get(&self, journal_id: Uuid) -> Result<Option<ProviderRequestJournalEntry>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, provider_id, outbound_event_uuid, attempt_no, state,
                   model, entity_identity, payload_hash,
                   netbox_revision_observed, stamped_external_request_id,
                   http_status, created_at, sent_at, completed_at, last_error
            FROM provider_request_journal
            WHERE id = $1
            "#,
        )
        .bind(journal_id)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(row) => Ok(Some(parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn prune_older_than(&self, days: i64) -> Result<u64, AppError> {
        let result = sqlx::query(
            r#"
            DELETE FROM provider_request_journal
            WHERE created_at < NOW() - make_interval(days => $1::int)
              AND state IN ('succeeded', 'failed_after_send', 'failed_no_send')
            "#,
        )
        .bind(days as i32)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }
}
