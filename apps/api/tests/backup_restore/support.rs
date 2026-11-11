use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, OnceLock};

use piping_backend::contexts::inventory::application::backup_service::FullBackupArchive;
use sqlx::PgPool;
use tokio::sync::{Mutex, OwnedMutexGuard};
use uuid::Uuid;

use crate::common;

pub(crate) type TestError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Clone, Copy)]
pub(crate) struct TenantFixture {
    pub(crate) group_id: Uuid,
    pub(crate) tenant_id: Uuid,
}

pub(crate) async fn suite_guard() -> OwnedMutexGuard<()> {
    static LOCK: OnceLock<Arc<Mutex<()>>> = OnceLock::new();
    LOCK.get_or_init(|| Arc::new(Mutex::new(())))
        .clone()
        .lock_owned()
        .await
}

pub(crate) async fn migrated_pool() -> Result<PgPool, TestError> {
    let pool = common::build_test_db_pool().await?;
    piping_backend::storage::database::migrations::run_migrations(&pool)
        .await
        .map_err(|error| format!("run migrations: {error}"))?;
    Ok(pool)
}

pub(crate) async fn seed_tenant_fixture(
    pool: &PgPool,
    label: &str,
) -> Result<TenantFixture, TestError> {
    let group_id = Uuid::now_v7();
    let tenant_id = Uuid::now_v7();
    let suffix = tenant_id.simple().to_string();
    let group_name = format!("backup-{label}-group-{suffix}");
    let tenant_name = format!("backup-{label}-tenant-{suffix}");

    sqlx::query(
        r#"
        INSERT INTO tenancy_tenantgroup (
            id, custom_field_data, name, slug, description, lft, rght,
            tree_id, level, comments
        )
        VALUES ($1, '{}'::jsonb, $2, $2, '', 0, 1, 0, 0, '')
        "#,
    )
    .bind(group_id)
    .bind(&group_name)
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO tenancy_tenant (
            id, custom_field_data, name, slug, description, comments, group_id
        )
        VALUES ($1, '{}'::jsonb, $2, $2, '', '', $3)
        "#,
    )
    .bind(tenant_id)
    .bind(&tenant_name)
    .bind(group_id)
    .execute(pool)
    .await?;

    Ok(TenantFixture {
        group_id,
        tenant_id,
    })
}

pub(crate) async fn seed_standalone_tenant(pool: &PgPool, label: &str) -> Result<Uuid, TestError> {
    let tenant_id = Uuid::now_v7();
    let name = format!("backup-{label}-{}", tenant_id.simple());
    sqlx::query(
        r#"
        INSERT INTO tenancy_tenant (
            id, custom_field_data, name, slug, description, comments
        )
        VALUES ($1, '{}'::jsonb, $2, $2, '', '')
        "#,
    )
    .bind(tenant_id)
    .bind(name)
    .execute(pool)
    .await?;
    Ok(tenant_id)
}

pub(crate) async fn cleanup_tenants(pool: &PgPool, tenant_ids: &[Uuid]) -> Result<(), TestError> {
    for tenant_id in tenant_ids {
        sqlx::query("DELETE FROM tenancy_tenant WHERE id = $1")
            .bind(tenant_id)
            .execute(pool)
            .await?;
    }
    Ok(())
}

pub(crate) async fn cleanup_fixture(
    pool: &PgPool,
    fixture: TenantFixture,
) -> Result<(), TestError> {
    cleanup_tenants(pool, &[fixture.tenant_id]).await?;
    sqlx::query("DELETE FROM tenancy_tenantgroup WHERE id = $1")
        .bind(fixture.group_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub(crate) fn archive_table_counts(archive: &FullBackupArchive) -> BTreeMap<String, usize> {
    archive
        .tables
        .iter()
        .map(|table| (table.name.clone(), table.rows.len()))
        .collect()
}

pub(crate) fn canonical_database_bytes(archive: &FullBackupArchive) -> Result<Vec<u8>, TestError> {
    let tables = archive
        .tables
        .iter()
        .map(|table| {
            let mut rows = table
                .rows
                .iter()
                .map(serde_json::to_string)
                .collect::<Result<Vec<_>, _>>()?;
            rows.sort();
            Ok((table.name.as_str(), rows))
        })
        .collect::<Result<Vec<_>, serde_json::Error>>()?;
    Ok(serde_json::to_vec(&tables)?)
}

pub(crate) async fn expected_backup_table_names(
    pool: &PgPool,
) -> Result<BTreeSet<String>, TestError> {
    let names = sqlx::query_scalar::<_, String>(
        r#"
        SELECT table_name
        FROM information_schema.tables
        WHERE table_schema = 'public'
          AND table_type = 'BASE TABLE'
          AND LEFT(table_name, 1) <> '_'
        ORDER BY table_name
        "#,
    )
    .fetch_all(pool)
    .await?;
    Ok(names.into_iter().collect())
}

pub(crate) fn archive_table_names(archive: &FullBackupArchive) -> BTreeSet<String> {
    archive
        .tables
        .iter()
        .map(|table| table.name.clone())
        .collect()
}
