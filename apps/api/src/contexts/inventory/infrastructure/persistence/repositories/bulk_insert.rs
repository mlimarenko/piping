//! Transaction orchestration for bulk inserts that report partial success.
//!
//! Every inventory `bulk_create` promises partial success: its result carries
//! both the rows it created and a per-row `errors` list, and the REST layer
//! surfaces both. Delivering that promise inside a single flat transaction is
//! impossible on PostgreSQL, which is what this module exists to fix.
//!
//! PostgreSQL aborts the entire transaction block on the first failed
//! statement. Every command after it fails with `current transaction is
//! aborted, commands ignored until end of transaction block`, and the closing
//! `COMMIT` silently degrades into a `ROLLBACK`. A loop that catches each
//! insert error and keeps going therefore produced a result that was actively
//! wrong: rows inserted BEFORE the first failure were reported as created even
//! though the rollback removed them, and every row after it was reported as
//! failed with a misleading "transaction is aborted" message instead of its own
//! cause.
//!
//! Wrapping each item operation in a SAVEPOINT scopes a failure to its own row,
//! so the surrounding transaction stays usable and the reported outcome
//! matches what is actually in the database. An item operation may contain
//! more than one statement when those statements must succeed atomically.

use std::future::Future;
use std::pin::Pin;

// `Acquire::begin` on an open transaction is what emits the SAVEPOINT; on a
// pool it would start a top-level transaction instead.
use sqlx::postgres::PgArguments;
use sqlx::{Acquire, PgConnection, PgPool, Postgres, Transaction};

use crate::errors::AppError;

/// The canonical outcome of an inventory bulk-create operation.
#[derive(Debug, Clone)]
pub struct BulkCreateResult<Record> {
    pub created: Vec<Record>,
    pub errors: Vec<String>,
}

/// A repository-owned item operation run inside one helper-owned savepoint.
pub type ItemFuture<'a, Record> = Pin<Box<dyn Future<Output = Result<Record, String>> + Send + 'a>>;

/// Begin a partial-success bulk transaction with immediate constraints.
///
/// Both the complete [`execute`] orchestration and lower-level one-statement
/// callers use this function, so transaction setup has one implementation.
///
/// # Errors
///
/// Returns [`AppError::Database`] if setup fails.
pub async fn begin(pool: &PgPool) -> Result<Transaction<'static, Postgres>, AppError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|error| AppError::Database(format!("Failed to begin transaction: {error}")))?;
    sqlx::query("SET CONSTRAINTS ALL IMMEDIATE")
        .execute(&mut *tx)
        .await
        .map_err(|error| {
            AppError::Database(format!("Failed to make constraints immediate: {error}"))
        })?;
    Ok(tx)
}

/// Insert a batch with one transaction and one savepoint per item.
///
/// Constraints are checked IMMEDIATE for the life of the transaction because
/// per-item isolation depends on violations surfacing before their savepoint is
/// released. Most foreign keys in this schema are `DEFERRABLE INITIALLY
/// DEFERRED`, inherited from the NetBox/Django schema; leaving them deferred
/// would move a bad reference to the final commit and roll back valid items too.
///
/// `create_item` owns the repository-specific statements and returned record.
/// Returning `Err` rejects only that item. Failures to manage the transaction or
/// a savepoint remain hard [`AppError::Database`] errors because the batch's
/// final state can no longer be trusted.
///
/// # Errors
///
/// Returns [`AppError::Database`] if transaction setup, savepoint management,
/// or the final commit fails.
pub async fn execute<Item, Record, CreateItem>(
    pool: &PgPool,
    items: &[Item],
    commit_operation: &str,
    mut create_item: CreateItem,
) -> Result<BulkCreateResult<Record>, AppError>
where
    Item: Sync,
    Record: Send,
    CreateItem: for<'a> FnMut(&'a mut PgConnection, &'a Item) -> ItemFuture<'a, Record> + Send,
{
    let mut created = Vec::with_capacity(items.len());
    let mut errors = Vec::new();

    let mut tx = begin(pool).await?;

    for item in items {
        let mut savepoint = tx
            .begin()
            .await
            .map_err(|error| AppError::Database(format!("Failed to open savepoint: {error}")))?;

        match create_item(&mut savepoint, item).await {
            Ok(record) => {
                savepoint.commit().await.map_err(|error| {
                    AppError::Database(format!("Failed to release savepoint: {error}"))
                })?;
                created.push(record);
            }
            Err(error) => {
                savepoint.rollback().await.map_err(|rollback_error| {
                    AppError::Database(format!("Failed to roll back savepoint: {rollback_error}"))
                })?;
                errors.push(error);
            }
        }
    }

    tx.commit().await.map_err(|error| {
        AppError::Database(format!("Failed to commit {commit_operation}: {error}"))
    })?;

    Ok(BulkCreateResult { created, errors })
}

/// Insert one query in its own savepoint.
///
/// # Errors
///
/// Returns [`AppError::Database`] if savepoint management fails.
pub async fn insert_row(
    tx: &mut Transaction<'_, Postgres>,
    query: sqlx::query::Query<'_, Postgres, PgArguments>,
) -> Result<Result<(), sqlx::Error>, AppError> {
    let mut savepoint = tx
        .begin()
        .await
        .map_err(|error| AppError::Database(format!("Failed to open savepoint: {error}")))?;

    match query.execute(&mut *savepoint).await {
        Ok(_) => {
            savepoint.commit().await.map_err(|error| {
                AppError::Database(format!("Failed to release savepoint: {error}"))
            })?;
            Ok(Ok(()))
        }
        Err(insert_error) => {
            savepoint.rollback().await.map_err(|error| {
                AppError::Database(format!("Failed to roll back savepoint: {error}"))
            })?;
            Ok(Err(insert_error))
        }
    }
}
