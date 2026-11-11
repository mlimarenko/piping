//! Database fence shared by transfer apply and uncertain-commit resolution.

use serde_json::Value;
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use super::{begin_provider_transaction_with_mode, fingerprint_tables};
use crate::contexts::sync::domain::inventory_export::digest;
use crate::errors::AppError;

fn busy() -> AppError {
    AppError::Conflict("inventory_transfer:busy".into())
}

fn mismatch() -> AppError {
    AppError::Conflict("inventory_transfer:descriptor_mismatch".into())
}

fn fence_error(error: sqlx::Error) -> AppError {
    if error
        .as_database_error()
        .and_then(|error| error.code())
        .is_some_and(|code| code == "55P03" || code == "57014")
    {
        busy()
    } else {
        AppError::Database("inventory_transfer:fence_database_error".into())
    }
}

/// Use the same provider advisory key as ordinary pulls without waiting on a
/// live owner. All subsequent heavyweight locks have the same bounded wait.
pub(super) async fn begin(
    pool: &PgPool,
    provider_id: Uuid,
) -> Result<Transaction<'_, Postgres>, AppError> {
    begin_provider_transaction_with_mode(pool, provider_id, true).await
}

/// Resolve an uncertain outcome without re-entering the inventory writer.
pub(super) async fn receipt(
    pool: &PgPool,
    run_id: Uuid,
    descriptor_digest: &str,
) -> Result<Option<Value>, AppError> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query("SELECT descriptor_digest, receipt FROM sync_inventory_transfer_run WHERE run_id = $1 FOR UPDATE NOWAIT")
        .bind(run_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(fence_error)?;
    let receipt = if let Some(row) = row {
        let stored_digest: String = row.try_get("descriptor_digest")?;
        if stored_digest != descriptor_digest {
            return Err(mismatch());
        }
        row.try_get("receipt")?
    } else {
        None
    };
    tx.commit().await?;
    Ok(receipt)
}

/// Register immutable, non-secret descriptor metadata outside the data commit.
pub(super) async fn register(
    pool: &PgPool,
    run_id: Uuid,
    descriptor_digest: &str,
    descriptor: &[u8],
) -> Result<(), AppError> {
    let mut tx = pool.begin().await?;
    sqlx::query("SET LOCAL lock_timeout = '250ms'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO sync_inventory_transfer_run (run_id, descriptor_digest, descriptor) VALUES ($1, $2, $3) ON CONFLICT (run_id) DO NOTHING")
        .bind(run_id)
        .bind(descriptor_digest)
        .bind(descriptor)
        .execute(&mut *tx)
        .await
        .map_err(fence_error)?;
    let row = sqlx::query(
        "SELECT descriptor_digest, descriptor FROM sync_inventory_transfer_run WHERE run_id = $1",
    )
    .bind(run_id)
    .fetch_one(&mut *tx)
    .await?;
    let stored_digest: String = row.try_get("descriptor_digest")?;
    let stored_descriptor: Vec<u8> = row.try_get("descriptor")?;
    if stored_digest != descriptor_digest || stored_descriptor.as_slice() != descriptor {
        return Err(mismatch());
    }
    tx.commit().await?;
    Ok(())
}

/// Lock the run before observing inventory, returning an existing receipt.
pub(super) async fn lock_run(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    descriptor_digest: &str,
) -> Result<Option<Value>, AppError> {
    let row = sqlx::query("SELECT descriptor_digest, receipt FROM sync_inventory_transfer_run WHERE run_id = $1 FOR UPDATE NOWAIT")
        .bind(run_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(fence_error)?
        .ok_or_else(|| AppError::Conflict("inventory_transfer:run_missing".into()))?;
    let stored_digest: String = row.try_get("descriptor_digest")?;
    if stored_digest != descriptor_digest {
        return Err(mismatch());
    }
    Ok(row.try_get("receipt")?)
}

/// Exclude concurrent writes to every table observed by the fingerprint.
/// Call only after the canonical Tag and hardware graph locks.
pub(super) async fn lock_observation_tables(
    tx: &mut Transaction<'_, Postgres>,
) -> Result<(), AppError> {
    let mut tables = fingerprint_tables();
    tables.insert("sync_inventory_transfer_target");
    tables.insert("_sqlx_migrations");
    for table in tables {
        let mut query = sqlx::QueryBuilder::<Postgres>::new("LOCK TABLE ");
        query
            .push(table)
            .push(" IN SHARE ROW EXCLUSIVE MODE NOWAIT");
        query
            .build()
            .execute(&mut **tx)
            .await
            .map_err(fence_error)?;
    }
    Ok(())
}

/// Observe target and migration identity while their tables are fenced.
pub(super) async fn target_and_schema(
    tx: &mut Transaction<'_, Postgres>,
) -> Result<(Uuid, String), AppError> {
    let target: Uuid = sqlx::query_scalar(
        "SELECT target_id FROM sync_inventory_transfer_target WHERE singleton = true",
    )
    .fetch_one(&mut **tx)
    .await?;
    let rows =
        sqlx::query("SELECT version, checksum, success FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&mut **tx)
            .await?;
    let mut migrations = Vec::with_capacity(rows.len());
    for row in rows {
        let version: i64 = row.try_get("version")?;
        let checksum: Vec<u8> = row.try_get("checksum")?;
        let success: bool = row.try_get("success")?;
        if !success {
            return Err(AppError::Conflict(
                "inventory_transfer:schema_incomplete".into(),
            ));
        }
        migrations.push((version, hex::encode(checksum)));
    }
    let encoded = serde_json::to_vec(&migrations)
        .map_err(|_| AppError::Internal("inventory_transfer:schema_encoding".into()))?;
    Ok((target, digest(&encoded)))
}

/// Persist the result only inside the inventory transaction.
pub(super) async fn complete(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    descriptor_digest: &str,
    receipt: &Value,
) -> Result<(), AppError> {
    let result = sqlx::query("UPDATE sync_inventory_transfer_run SET receipt = $3, committed_at = now() WHERE run_id = $1 AND descriptor_digest = $2 AND receipt IS NULL")
        .bind(run_id)
        .bind(descriptor_digest)
        .bind(receipt)
        .execute(&mut **tx)
        .await?;
    if result.rows_affected() != 1 {
        return Err(mismatch());
    }
    Ok(())
}
