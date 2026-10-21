//! Schema-drift + seed gate for the generic-FK content-type registry
//! (`contexts/inventory/domain/content_type::CONTENT_TYPES`).
//!
//! Codex-mandated condition 1: the registry is the ONLY source of truth, and
//! the DB must back every claim it makes. This test asserts, against a migrated
//! canonical database, that for EVERY registry def:
//!   - the target `table` exists and has `id` + its declared label column,
//!   - the parent join (`parent.table`, `parent.fk`) exists when declared,
//!   - the `seed_content_types` migration created an `inventory_content_type`
//!     row for the def's `(app_label, model)` token.
//!
//! `#[ignore]`d like the other DB integration tests; run with the dev Postgres
//! (`make test`) or:
//!
//! ```text
//! cargo test --test integration_content_type_registry -- --ignored --test-threads=1
//! ```

mod common;

use piping_backend::contexts::inventory::domain::content_type::CONTENT_TYPES;
use sqlx::Row;

/// Acquire a migrated pool, or `None` if no test database is reachable (skip).
async fn migrated_pool() -> Result<sqlx::PgPool, Box<dyn std::error::Error + Send + Sync>> {
    let pool = common::build_test_db_pool().await?;
    piping_backend::storage::database::migrations::run_migrations(&pool)
        .await
        .map_err(|e| format!("run migrations: {e}"))?;
    Ok(pool)
}

/// True iff `table.column` exists in the `public` schema.
async fn column_exists(
    pool: &sqlx::PgPool,
    table: &str,
    column: &str,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let row = sqlx::query(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM information_schema.columns
            WHERE table_schema = 'public' AND table_name = $1 AND column_name = $2
        ) AS present
        "#,
    )
    .bind(table)
    .bind(column)
    .fetch_one(pool)
    .await
    .map_err(|e| format!("information_schema.columns query: {e}"))?;
    Ok(row.try_get::<bool, _>("present").unwrap_or(false))
}

#[tokio::test]
#[ignore = "requires database (run via make test)"]
async fn every_registry_def_matches_schema() -> Result<(), Box<dyn std::error::Error + Send + Sync>>
{
    let pool = migrated_pool().await?;

    for def in CONTENT_TYPES {
        let token = def.token();
        assert!(
            column_exists(&pool, def.table, "id").await?,
            "{token}: table {} has no `id` column",
            def.table
        );
        assert!(
            column_exists(&pool, def.table, def.label_column()).await?,
            "{token}: table {} has no declared label column `{}`",
            def.table,
            def.label_column()
        );
        if let Some(parent) = def.parent {
            assert!(
                column_exists(&pool, def.table, parent.fk).await?,
                "{token}: table {} has no parent fk column `{}`",
                def.table,
                parent.fk
            );
            assert!(
                column_exists(&pool, parent.table, "id").await?,
                "{token}: parent table {} has no `id` column",
                parent.table
            );
            assert!(
                column_exists(&pool, parent.table, "name").await?,
                "{token}: parent table {} has no `name` column",
                parent.table
            );
        }
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires database (run via make test)"]
async fn every_registry_token_is_seeded() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let pool = migrated_pool().await?;

    for def in CONTENT_TYPES {
        let token = def.token();
        let row = sqlx::query(
            "SELECT id FROM inventory_content_type WHERE app_label = $1 AND model = $2",
        )
        .bind(def.app_label)
        .bind(def.model)
        .fetch_optional(&pool)
        .await
        .expect("inventory_content_type lookup");
        assert!(
            row.is_some(),
            "{token}: no seeded inventory_content_type row (seed migration missing it)"
        );
    }
    Ok(())
}
