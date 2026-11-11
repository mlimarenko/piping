use std::{env, path::PathBuf};

use sqlx::{PgPool, migrate::Migrator};

use crate::errors::AppError;

/// Runs database migrations.
///
/// Resolution order:
/// 1) `PIPING_MIGRATIONS_DIR` (if set)
/// 2) `/app/migrations` (container runtime)
/// 3) `./migrations` (local dev)
///
/// `0001_init.sql` is the consolidated baseline (see `migrations/README.md`).
/// Future schema changes append forward deltas to the 1.0 baseline.
/// `Migrator::run` applies the full ordered set.
///
/// This runs the resolved migrations and nothing else. Missing applied versions
/// or checksum conflicts fail closed. Pre-1.0 installations require the explicit
/// offline maintenance transition before startup; runtime never re-stamps history.
pub async fn run_migrations(pool: &PgPool) -> Result<(), AppError> {
    let migrations_path = env::var("PIPING_MIGRATIONS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let container_default = PathBuf::from("/app/migrations");
            if container_default.exists() {
                container_default
            } else {
                PathBuf::from("./migrations")
            }
        });

    let migrator = Migrator::new(migrations_path.as_path())
        .await
        .map_err(|e| AppError::Database(format!("Failed to load migrations: {}", e)))?;

    migrator
        .run(pool)
        .await
        .map_err(|e| AppError::Database(format!("Migration failed: {}", e)))?;

    Ok(())
}
