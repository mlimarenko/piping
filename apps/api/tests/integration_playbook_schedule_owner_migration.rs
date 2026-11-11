//! PIP-530 proof of the `0020 -> 0021` schedule-owner upgrade.
//!
//! Runs against a disposable database created from `PIPING_DATABASE__URL` and
//! force-dropped afterwards. The fixture seeds an active and an inactive owner,
//! applies 0021 once, and asserts the archived service-principal identity, the
//! paused ambiguous row, the rename and that no row is deleted.
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, migrate::MigrateDatabase};
use uuid::Uuid;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const MIGRATIONS: [&str; 20] = [
    include_str!("../migrations/0001_init.sql"),
    include_str!("../migrations/0002_interface_networking.sql"),
    include_str!("../migrations/0003_service_port_mappings.sql"),
    include_str!("../migrations/0004_fhrp_auth_key_secret.sql"),
    include_str!("../migrations/0005_module_bay_types.sql"),
    include_str!("../migrations/0006_canonical_tag_ids.sql"),
    include_str!("../migrations/0007_cooling_hardware.sql"),
    include_str!("../migrations/0008_module_type_profile_scope.sql"),
    include_str!("../migrations/0009_circuit_group_assignments.sql"),
    include_str!("../migrations/0010_vlan_translation.sql"),
    include_str!("../migrations/0011_termination_graph.sql"),
    include_str!("../migrations/0012_inventory_transfer_run.sql"),
    include_str!("../migrations/0013_component_template_instantiation.sql"),
    include_str!("../migrations/0014_tag_object_type_scope.sql"),
    include_str!("../migrations/0015_device_bay_template_enabled.sql"),
    include_str!("../migrations/0016_project_workspace.sql"),
    include_str!("../migrations/0017_dataflow_catalog.sql"),
    include_str!("../migrations/0018_target_set_delete_restrict.sql"),
    include_str!("../migrations/0019_config_context_semantics.sql"),
    include_str!("../migrations/0020_monitoring_profiles.sql"),
];
const OWNER: &str = include_str!("../migrations/0021_playbook_schedule_owner.sql");

const ACTIVE_OWNER: &str = "02530000-0000-7000-8000-000000000001";
const INACTIVE_OWNER: &str = "02530000-0000-7000-8000-000000000002";
const SERVICE_PRINCIPAL: &str = "02530000-0000-7000-8000-000000000003";
const PROJECT: &str = "02530000-0000-7000-8000-000000000004";
const ENVIRONMENT: &str = "02530000-0000-7000-8000-000000000005";
const TEMPLATE: &str = "02530000-0000-7000-8000-000000000006";
const ACTIVE_SCHEDULE: &str = "02530000-0000-7000-8000-000000000007";
const INACTIVE_SCHEDULE: &str = "02530000-0000-7000-8000-000000000008";

const FIXTURE: &str = r#"
INSERT INTO identity_user (id, username, email, status, auth_source) VALUES
    ('02530000-0000-7000-8000-000000000001', 'owner-active', 'active@test.local', 'active', 'local'),
    ('02530000-0000-7000-8000-000000000002', 'owner-inactive', 'inactive@test.local', 'inactive', 'local');
INSERT INTO playbook_service_principal (id, name, slug) VALUES
    ('02530000-0000-7000-8000-000000000003', 'retired executor', 'retired-executor');
INSERT INTO playbook_project (id, name, source) VALUES
    ('02530000-0000-7000-8000-000000000004', 'schedule-owner', '{"kind":"editable"}');
INSERT INTO playbook_execution_environment (id, name, image) VALUES
    ('02530000-0000-7000-8000-000000000005', 'schedule-owner-ee', 'image');
INSERT INTO playbook_job_template
    (id, name, project_id, execution_environment_id, workdir, playbook_paths,
     revision_selection, inventory, options)
VALUES
    ('02530000-0000-7000-8000-000000000006', 'schedule-owner-template',
     '02530000-0000-7000-8000-000000000004', '02530000-0000-7000-8000-000000000005',
     '.', ARRAY['site.yml'],
     '{"kind":"fixed","revision_id":"02530000-0000-7000-8000-00000000000a"}',
     '{"kind":"native","sources":["inventory.yml"],"ssh_record_id":null}',
     '{"argv":[],"extra_vars":{},"limit":null,"env":{},"secret_refs":{},"timeout_seconds":60}');
INSERT INTO playbook_schedule
    (id, template_id, targets, service_principal_id, delegated_by_user_id,
     name, cron_expression, enabled, next_run_at, metadata)
VALUES
    ('02530000-0000-7000-8000-000000000007', '02530000-0000-7000-8000-000000000006',
     '{"kind":"fixed","targets":[]}', '02530000-0000-7000-8000-000000000003',
     '02530000-0000-7000-8000-000000000001', 'active-owner', '0 3 * * *', true,
     now() + interval '1 day', '{"kept":true}'),
    ('02530000-0000-7000-8000-000000000008', '02530000-0000-7000-8000-000000000006',
     '{"kind":"fixed","targets":[]}', '02530000-0000-7000-8000-000000000003',
     '02530000-0000-7000-8000-000000000002', 'inactive-owner', '0 3 * * *', true,
     now() + interval '1 day', '{}');
"#;

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL 18 database with CREATEDB"]
async fn schedule_owner_upgrade_archives_identity_and_pauses_ambiguous_rows() -> TestResult {
    let url = create_database("pip530mig").await?;
    let outcome = run_upgrade(&url).await;
    let cleanup = <Postgres as MigrateDatabase>::force_drop_database(&url).await;
    outcome?;
    cleanup?;
    Ok(())
}

async fn create_database(prefix: &str) -> TestResult<String> {
    let mut url = url::Url::parse(&std::env::var("PIPING_DATABASE__URL")?)?;
    url.set_path(&format!("/{prefix}_{}", Uuid::now_v7().simple()));
    let url = url.to_string();
    <Postgres as MigrateDatabase>::create_database(&url).await?;
    Ok(url)
}

async fn run_upgrade(url: &str) -> TestResult {
    let pool = PgPool::connect(url).await?;
    for migration in MIGRATIONS {
        sqlx::raw_sql(migration).execute(&pool).await?;
    }
    sqlx::raw_sql(FIXTURE).execute(&pool).await?;

    let mut transaction = pool.begin().await?;
    sqlx::raw_sql(OWNER).execute(&mut *transaction).await?;
    transaction.commit().await?;

    assert_identity_columns(&pool).await?;
    assert_rows_preserved(&pool).await?;
    assert_history_columns_remain(&pool).await?;

    pool.close().await;
    Ok(())
}

async fn assert_identity_columns(pool: &PgPool) -> TestResult {
    assert!(column_exists(pool, "owner_user_id").await?);
    assert!(!column_exists(pool, "delegated_by_user_id").await?);
    assert!(!column_exists(pool, "service_principal_id").await?);
    assert!(constraint_exists(pool, "playbook_schedule_owner_user_id_fkey").await?);
    assert!(constraint_exists(pool, "playbook_schedule_owner_user_id_not_null").await?);
    assert!(!constraint_exists(pool, "playbook_schedule_delegated_by_user_id_fkey").await?);
    assert!(!constraint_exists(pool, "playbook_schedule_service_principal_id_fkey").await?);
    assert!(!index_exists(pool, "idx_playbook_schedule_service_principal").await?);
    Ok(())
}

async fn assert_rows_preserved(pool: &PgPool) -> TestResult {
    let rows: Vec<(String, bool, Uuid, Value)> = sqlx::query_as(
        "SELECT name, enabled, owner_user_id, metadata FROM playbook_schedule ORDER BY name",
    )
    .fetch_all(pool)
    .await?;
    assert_eq!(rows.len(), 2, "0021 must not delete schedule rows");

    let (active_name, active_enabled, active_owner, active_metadata) = &rows[0];
    assert_eq!(active_name, "active-owner");
    assert!(active_enabled, "an active owner keeps the schedule enabled");
    assert_eq!(active_owner.to_string(), ACTIVE_OWNER);
    assert_eq!(active_metadata["kept"], json!(true));
    assert_eq!(
        active_metadata["migrated_from_service_principal_id"],
        json!(SERVICE_PRINCIPAL)
    );
    assert_eq!(active_metadata["owner_migration_paused"], Value::Null);

    let (inactive_name, inactive_enabled, inactive_owner, inactive_metadata) = &rows[1];
    assert_eq!(inactive_name, "inactive-owner");
    assert!(
        !inactive_enabled,
        "an ambiguous owner pauses the schedule instead of firing"
    );
    assert_eq!(inactive_owner.to_string(), INACTIVE_OWNER);
    assert_eq!(
        inactive_metadata["migrated_from_service_principal_id"],
        json!(SERVICE_PRINCIPAL)
    );
    assert_eq!(inactive_metadata["owner_migration_paused"], json!(true));
    assert_eq!(
        inactive_metadata["owner_migration_pause_reason"],
        json!("owner_missing_or_inactive")
    );
    Ok(())
}

async fn assert_history_columns_remain(pool: &PgPool) -> TestResult {
    // The run group's service-principal column is history, not authority.
    let present: bool = sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM information_schema.columns
            WHERE table_schema = 'public' AND table_name = 'playbook_run_group'
              AND column_name = 'service_principal_id'
        )",
    )
    .fetch_one(pool)
    .await?;
    assert!(
        present,
        "playbook_run_group.service_principal_id must keep historical identity"
    );
    Ok(())
}

async fn column_exists(pool: &PgPool, column: &str) -> TestResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM information_schema.columns
            WHERE table_schema = 'public' AND table_name = 'playbook_schedule'
              AND column_name = $1
        )",
    )
    .bind(column)
    .fetch_one(pool)
    .await?)
}

async fn constraint_exists(pool: &PgPool, name: &str) -> TestResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM pg_constraint
            WHERE conrelid = 'public.playbook_schedule'::regclass AND conname = $1
        )",
    )
    .bind(name)
    .fetch_one(pool)
    .await?)
}

async fn index_exists(pool: &PgPool, name: &str) -> TestResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM pg_indexes
            WHERE schemaname = 'public' AND tablename = 'playbook_schedule'
              AND indexname = $1
        )",
    )
    .bind(name)
    .fetch_one(pool)
    .await?)
}
