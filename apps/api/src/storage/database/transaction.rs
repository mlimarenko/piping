use sqlx::PgPool;
use sqlx::postgres::PgTransaction;

use crate::errors::AppError;

/// Executes a function within a database transaction with automatic commit/rollback.
///
/// Process:
/// 1. Begins a new database transaction
/// 2. Executes the provided function with the transaction
/// 3. If function succeeds: commits the transaction
/// 4. If function fails: rolls back the transaction
///
/// This ensures atomicity: either all operations succeed or none do.
/// Used for multi-step operations that must be atomic (e.g., creating a user with credentials).
///
/// # Example
/// ```rust,ignore
/// use piping_backend::storage::database::transaction::in_transaction;
/// # let pool = sqlx::PgPool::connect_lazy("postgres://postgres:postgres@localhost/piping")
/// #     .expect("pool");
///
/// in_transaction(&pool, |tx| async move {
///     // Multiple database operations here
///     // All succeed or all fail together
///     Ok(())
/// }).await?;
/// ```
pub async fn in_transaction<F, Fut, T>(pool: &PgPool, func: F) -> Result<T, AppError>
where
    F: FnOnce(&mut PgTransaction<'_>) -> Fut,
    Fut: Future<Output = Result<T, AppError>>,
{
    // Begin transaction
    let mut tx = pool.begin().await?;

    // Execute the function with the transaction
    let result = func(&mut tx).await;

    match result {
        Ok(value) => {
            // Success: commit all changes
            tx.commit().await?;
            Ok(value)
        }
        Err(e) => {
            // Error: rollback all changes
            tx.rollback().await?;
            Err(e)
        }
    }
}
