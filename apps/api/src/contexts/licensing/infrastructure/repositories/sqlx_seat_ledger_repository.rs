use chrono::Utc;
use sqlx::{Postgres, Transaction};

use crate::contexts::licensing::infrastructure::repositories::repository::{
    SeatLedgerEvent, SeatLedgerRepository,
};
use crate::errors::AppError;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxSeatLedgerRepository;

impl SqlxSeatLedgerRepository {
    pub fn new() -> Self {
        Self
    }
}

impl SeatLedgerRepository for SqlxSeatLedgerRepository {
    async fn append_event(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        event: &SeatLedgerEvent,
    ) -> Result<(), AppError> {
        sqlx::query(
            r#"
            INSERT INTO license_seat_ledger (
                instance_id,
                user_id,
                event_type,
                reason_code,
                metadata_json,
                created_at
            )
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(&event.instance_id)
        .bind(event.user_id)
        .bind(&event.event_type)
        .bind(&event.reason_code)
        .bind(&event.metadata)
        .bind(Utc::now())
        .execute(&mut **transaction)
        .await
        .map_err(|error| {
            AppError::Database(format!("Failed to append seat ledger event: {error}"))
        })?;
        Ok(())
    }
}
