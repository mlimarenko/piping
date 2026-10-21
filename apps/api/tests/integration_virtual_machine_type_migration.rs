//! PWA-95 fresh/upgrade checks use uniquely named child databases, never the
//! caller's parent/shared database. Requires a disposable PostgreSQL 18 server.

use std::fs;
use std::path::PathBuf;

use piping_backend::contexts::inventory::domain::VirtualMachineType;
use piping_backend::contexts::inventory::infrastructure::persistence::{
    SqlxVirtualMachineTypeRepository, VirtualMachineTypeChanges, VirtualMachineTypeFilter,
    VirtualMachineTypeRepository,
};
use sqlx::migrate::{MigrateDatabase, Migrator};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Postgres};
use url::Url;
use uuid::Uuid;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

async fn history(pool: &PgPool) -> TestResult<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT row_to_json(m)::text FROM _sqlx_migrations m WHERE version <= 14 ORDER BY version",
    )
    .fetch_all(pool)
    .await?)
}

async fn schema_columns(pool: &PgPool) -> TestResult<Vec<String>> {
    Ok(sqlx::query_scalar("SELECT table_name || '/' || column_name || '/' || udt_name || '/' || is_nullable || '/' || COALESCE(column_default, '') FROM information_schema.columns WHERE table_schema = 'public' AND table_name <> '_sqlx_migrations' ORDER BY table_name, ordinal_position")
        .fetch_all(pool).await?)
}

async fn typed_repository_round_trip(pool: &PgPool) -> TestResult {
    let repository = SqlxVirtualMachineTypeRepository::new(pool.clone());
    let item = VirtualMachineType {
        id: Uuid::now_v7(),
        name: "Repository roundtrip".into(),
        slug: "repository-roundtrip".into(),
        default_vcpus: Some(1.23),
        default_memory: Some(512),
        description: "description".into(),
        comments: "comment".into(),
        custom_fields: Some(serde_json::json!({"roundtrip":true})),
        ..Default::default()
    };
    repository.create(&item).await?;
    let loaded = repository
        .get_by_id(item.id)
        .await?
        .ok_or("created type missing")?;
    assert_eq!(loaded.default_vcpus, Some(1.23));
    assert_eq!(loaded.default_memory, Some(512));
    assert_eq!(loaded.custom_fields, item.custom_fields);
    assert_eq!(loaded.virtual_machine_count, 0);
    let (items, total) = repository
        .list_paginated(
            Some(1),
            Some(50),
            VirtualMachineTypeFilter {
                search: Some("%Repository%".into()),
                ..Default::default()
            },
        )
        .await?;
    assert!(items.is_empty());
    assert_eq!(total, 1, "empty-page total must retain the search filter");
    repository_updates_and_reference_guard(pool, &repository, &item).await
}

async fn repository_updates_and_reference_guard(
    pool: &PgPool,
    repository: &SqlxVirtualMachineTypeRepository,
    item: &VirtualMachineType,
) -> TestResult {
    assert!(
        repository
            .update(
                item.id,
                &VirtualMachineTypeChanges {
                    default_vcpus: Some(None),
                    default_memory: Some(Some(0)),
                    ..Default::default()
                }
            )
            .await?
    );
    let loaded = repository
        .get_by_id(item.id)
        .await?
        .ok_or("updated type missing")?;
    assert_eq!(loaded.default_vcpus, None);
    assert_eq!(loaded.default_memory, Some(0));
    sqlx::query("UPDATE virtualization_virtualmachine SET virtual_machine_type_id=$1 WHERE id='01900000-0000-7000-8000-000000000095'").bind(item.id).execute(pool).await?;
    assert_eq!(
        repository
            .get_by_id(item.id)
            .await?
            .ok_or("assigned type missing")?
            .virtual_machine_count,
        1
    );
    assert!(
        repository.delete(item.id).await.is_err(),
        "assigned type must have blocked deletion"
    );
    sqlx::query("UPDATE virtualization_virtualmachine SET virtual_machine_type_id=NULL WHERE virtual_machine_type_id=$1").bind(item.id).execute(pool).await?;
    repository.delete(item.id).await?;
    assert!(repository.get_by_id(item.id).await?.is_none());
    Ok(())
}

async fn exercise(upgrade_url: &str, fresh_url: &str) -> TestResult {
    let migrations = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let baseline = baseline_migrations(&migrations)?;
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(upgrade_url)
        .await?;
    Migrator::new(baseline.path()).await?.run(&pool).await?;
    sqlx::raw_sql(include_str!(
        "fixtures/netbox/virtual_machine_type_before.sql"
    ))
    .execute(&pool)
    .await?;
    let old_history = history(&pool).await?;
    let current = Migrator::new(migrations.as_path()).await?;
    current.run(&pool).await?;
    assert_eq!(
        history(&pool).await?,
        old_history,
        "deployed migration history changed"
    );
    typed_repository_round_trip(&pool).await?;
    sqlx::raw_sql(include_str!(
        "fixtures/netbox/virtual_machine_type_after.sql"
    ))
    .execute(&pool)
    .await?;
    let upgraded_columns = schema_columns(&pool).await?;
    pool.close().await;
    fresh_matches_upgrade(fresh_url, &current, upgraded_columns).await
}

async fn fresh_matches_upgrade(
    fresh_url: &str,
    current: &Migrator,
    upgraded_columns: Vec<String>,
) -> TestResult {
    let fresh = PgPoolOptions::new()
        .max_connections(2)
        .connect(fresh_url)
        .await?;
    current.run(&fresh).await?;
    assert_eq!(
        schema_columns(&fresh).await?,
        upgraded_columns,
        "fresh and upgrade columns diverged"
    );
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM inventory_content_type WHERE app_label = 'virtualization' AND model = 'virtualmachinetype'").fetch_one(&fresh).await?, 1);
    fresh.close().await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 with CREATEDB via PIPING_DATABASE__URL"]
async fn virtual_machine_types_upgrade_preserves_state_and_matches_fresh_schema() -> TestResult {
    let parent = std::env::var("PIPING_DATABASE__URL")?;
    let mut upgrade = Url::parse(&parent)?;
    upgrade.set_path(&format!("/pwa95_upgrade_{}", Uuid::now_v7().simple()));
    let mut fresh = Url::parse(&parent)?;
    fresh.set_path(&format!("/pwa95_fresh_{}", Uuid::now_v7().simple()));
    let upgrade = upgrade.to_string();
    let fresh = fresh.to_string();
    <Postgres as MigrateDatabase>::create_database(&upgrade).await?;
    if let Err(error) = <Postgres as MigrateDatabase>::create_database(&fresh).await {
        <Postgres as MigrateDatabase>::force_drop_database(&upgrade).await?;
        return Err(error.into());
    }
    let result = exercise(&upgrade, &fresh).await;
    let upgrade_cleanup = <Postgres as MigrateDatabase>::force_drop_database(&upgrade).await;
    let fresh_cleanup = <Postgres as MigrateDatabase>::force_drop_database(&fresh).await;
    result?;
    upgrade_cleanup?;
    fresh_cleanup?;
    Ok(())
}

fn baseline_migrations(migrations: &std::path::Path) -> TestResult<tempfile::TempDir> {
    let baseline = tempfile::tempdir()?;
    for entry in fs::read_dir(migrations)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some((version, _)) = name.split_once('_') else {
            continue;
        };
        if path.extension().is_some_and(|extension| extension == "sql")
            && version.parse::<u32>()? <= 14
        {
            fs::copy(&path, baseline.path().join(name))?;
        }
    }
    Ok(baseline)
}
