//! PWA-10 migration contract for legacy monitoring query-source credentials.
//!
//! This target deliberately creates a uniquely named child database from the
//! explicitly supplied `PIPING_DATABASE__URL`. It never applies migrations to
//! that parent/shared database. Run only against a disposable PostgreSQL test
//! server with CREATEDB privilege:
//!
//! `cargo test -p piping-backend --test integration_monitoring_query_source_secret_migration -- --ignored --test-threads=1`

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sqlx::migrate::{MigrateDatabase, Migrator};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Postgres, Row};
use url::Url;
use uuid::Uuid;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const QUERY_SOURCE_CREDENTIAL_CONSUMER_TYPE: &str = "monitoring_query_source";
const QUERY_SOURCE_CREDENTIAL_FIELD_KEY: &str = "bearer_token";
const QUERY_SOURCE_CREDENTIAL_OWNER_SCOPE_TYPE: &str = "monitoring_query_source";
const ALERT_SOURCE_CREDENTIAL_CONSUMER_TYPE: &str = "monitoring_alert_source";
const ALERT_SOURCE_CREDENTIAL_FIELD_KEY: &str = "bearer_token";
const ALERT_SOURCE_CREDENTIAL_OWNER_SCOPE_TYPE: &str = "monitoring_alert_source";
const MIGRATION_VERSION: i64 = 2;
const ALERT_MIGRATION_VERSION: i64 = 3;
const REBASELINE_CHECKSUM_MISMATCH_SQLSTATE: &str = "22000";
const LEGACY_VERSION_TWO_DESCRIPTION: &str = "init audit";
const RENAMED_CURRENT_VERSION_TWO_DESCRIPTION: &str = "reconciled migration label";
const UNKNOWN_FUTURE_FORWARD_MIGRATION_VERSION: i64 = 9_999;
// SHA-384 of the historical `0002_init_audit.sql` migration from the deployed
// pre-squash lineage. Use the real SQLx checksum rather than a synthetic
// value so this regression represents a database that could actually exist.
const LEGACY_VERSION_TWO_CHECKSUM_HEX: &str = "04be20aa7e1967e54cbac3c22f28d6865d784f7d173c9af2add449931b89f92e573059738d1f9a0df5ac86fbd5e22c19";

#[derive(Debug, Eq, PartialEq)]
struct MigrationHistoryRow {
    version: i64,
    description: String,
    installed_on: String,
    success: bool,
    checksum_hex: String,
    execution_time: i64,
}

/// PostgreSQL accepts UUID separators after each four-digit group, while the
/// historical `Uuid::parse_str` accepts only its 32-byte, hyphenated, braced,
/// and lowercase-URN spellings. This fixture is intentionally valid to
/// PostgreSQL but invalid to the old Rust parser.
fn postgres_only_uuid_spelling(secret_id: Uuid) -> String {
    let compact = secret_id.simple().to_string();
    format!(
        "{}-{}-{}-{}-{}-{}-{}-{}",
        &compact[0..4],
        &compact[4..8],
        &compact[8..12],
        &compact[12..16],
        &compact[16..20],
        &compact[20..24],
        &compact[24..28],
        &compact[28..32],
    )
}

fn ensure(condition: bool, message: impl Into<String>) -> TestResult {
    if condition {
        Ok(())
    } else {
        Err(message.into().into())
    }
}

fn api_migrations_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations")
}

fn workspace_root() -> TestResult<PathBuf> {
    let api_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let apps_dir = api_dir
        .parent()
        .ok_or("could not determine workspace apps directory")?;
    let root = apps_dir
        .parent()
        .ok_or("could not determine workspace root")?;
    Ok(root.to_path_buf())
}

fn rebaseline_sql() -> TestResult<String> {
    let script = workspace_root()?.join("scripts/rebaseline-sqlx-0001.sh");
    let output = Command::new("bash").arg(script).arg("print-sql").output()?;
    ensure(
        output.status.success(),
        "could not render the isolated baseline re-stamp SQL",
    )?;
    Ok(String::from_utf8(output.stdout)?)
}

/// Forward migration versions the canonical directory ships, ascending. The
/// re-baseline must preserve every already-applied checksum-matching forward
/// row, so the expected post-rebaseline history is version 1 followed by
/// exactly these. Deriving the list from the directory keeps the expectation
/// equal to the deployed set instead of naming migrations one by one.
fn canonical_forward_migration_versions() -> TestResult<Vec<i64>> {
    let mut versions = Vec::new();
    for entry in fs::read_dir(api_migrations_dir())? {
        let entry = entry?;
        let Some(file_name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some((prefix, _)) = file_name.split_once('_') else {
            continue;
        };
        if !file_name.ends_with(".sql") {
            continue;
        }
        let version = prefix.parse::<i64>().map_err(|_| {
            format!("migration file `{file_name}` does not start with a numeric version")
        })?;
        if version > 1 {
            versions.push(version);
        }
    }
    versions.sort_unstable();
    Ok(versions)
}

fn copy_migrations(
    scratch_dir: &Path,
    directory_name: &str,
    include_forward_migrations: bool,
) -> TestResult<PathBuf> {
    let destination = scratch_dir.join(directory_name);
    fs::create_dir(&destination)?;
    let source = api_migrations_dir();
    fs::copy(
        source.join("0001_init.sql"),
        destination.join("0001_init.sql"),
    )?;
    if include_forward_migrations {
        // Mirror `rebaseline-sqlx-0001.sh`: every post-baseline forward file is
        // applied in order. Copying each one keeps the exercised set equal to
        // the deployed set instead of naming migrations one by one.
        for entry in fs::read_dir(&source)? {
            let entry = entry?;
            let file_name = entry.file_name();
            let Some(file_name) = file_name.to_str() else {
                continue;
            };
            let is_forward = file_name.ends_with(".sql")
                && file_name != "0001_init.sql"
                && file_name
                    .chars()
                    .next()
                    .is_some_and(|first| first.is_ascii_digit());
            if is_forward {
                fs::copy(entry.path(), destination.join(file_name))?;
            }
        }
    }
    Ok(destination)
}

fn disposable_database_url() -> TestResult<String> {
    let parent_url = std::env::var("PIPING_DATABASE__URL").map_err(|_| {
        "PIPING_DATABASE__URL must be set to a PostgreSQL server with CREATEDB for this isolated migration test"
    })?;
    let mut url = Url::parse(&parent_url)?;
    let database_name = format!("pwa10_mqsc_{}", Uuid::now_v7().simple());
    url.set_path(&format!("/{database_name}"));
    Ok(url.to_string())
}

async fn create_disposable_database(database_url: &str) -> TestResult {
    if <Postgres as MigrateDatabase>::database_exists(database_url).await? {
        <Postgres as MigrateDatabase>::force_drop_database(database_url).await?;
    }
    <Postgres as MigrateDatabase>::create_database(database_url).await?;
    Ok(())
}

async fn force_drop_disposable_database(database_url: &str) -> TestResult {
    if <Postgres as MigrateDatabase>::database_exists(database_url).await? {
        <Postgres as MigrateDatabase>::force_drop_database(database_url).await?;
    }
    Ok(())
}

async fn child_pool(database_url: &str) -> TestResult<PgPool> {
    Ok(PgPoolOptions::new()
        .max_connections(1)
        .connect(database_url)
        .await?)
}

/// Always use a fresh one-connection pool. SQLx's PostgreSQL migration lock is
/// session-scoped and a failed `Migrator::run` exits before unlock; closing this
/// pool releases it before the next abort/backfill attempt.
async fn apply_migrations(database_url: &str, migration_dir: &Path) -> TestResult {
    let migrator = Migrator::new(migration_dir).await?;
    let pool = child_pool(database_url).await?;
    let result: TestResult = migrator.run(&pool).await.map_err(Into::into);
    pool.close().await;
    result
}

/// A migration RED assertion must prove the intended database rejection, not
/// merely treat a syntax, connection, or bookkeeping failure as a pass.
///
/// `ticket` names which forward migration the case belongs to (`PWA-10` for the
/// query-source file, `PWA-11` for the alert-source file) so the success-error
/// text proves the intended abort instead of hard-coding one tracker.
async fn assert_migration_aborts_with_sqlstate(
    database_url: &str,
    migration_dir: &Path,
    expected_sqlstate: &str,
    case: &str,
    ticket: &str,
) -> TestResult {
    let migration_error = match apply_migrations(database_url, migration_dir).await {
        Ok(()) => return Err(format!("{case} must abort {ticket}").into()),
        Err(error) => error,
    };
    let database_error = match migration_error.downcast_ref::<sqlx::migrate::MigrateError>() {
        Some(sqlx::migrate::MigrateError::ExecuteMigration(error, _)) => error.as_database_error(),
        _ => None,
    };
    let actual_sqlstate = database_error
        .and_then(|error| error.code())
        .map(|code| code.into_owned());
    ensure(
        actual_sqlstate.as_deref() == Some(expected_sqlstate),
        format!(
            "{case} must fail with PostgreSQL SQLSTATE {expected_sqlstate}, got {}",
            actual_sqlstate
                .as_deref()
                .unwrap_or("non-database migration error"),
        ),
    )
}

/// Execute the production re-baseline rendering against the child database.
/// This is intentionally part of the migration test: stamping 0002 without
/// executing it would make the new runtime skip the required raw-ref backfill.
async fn apply_rebaseline(database_url: &str) -> TestResult {
    let sql = rebaseline_sql()?;
    let pool = child_pool(database_url).await?;
    let result: TestResult = sqlx::raw_sql(&sql)
        .execute(&pool)
        .await
        .map(|_| ())
        .map_err(Into::into);
    pool.close().await;
    result
}

async fn seed_secret(
    pool: &PgPool,
    id: Uuid,
    key: &str,
    kind: &str,
    is_protected: bool,
) -> TestResult {
    sqlx::query(
        r#"
        INSERT INTO secrets_variable (id, key, kind, is_masked, is_protected)
        VALUES ($1, $2, $3, true, $4)
        "#,
    )
    .bind(id)
    .bind(key)
    .bind(kind)
    .bind(is_protected)
    .execute(pool)
    .await?;
    Ok(())
}

async fn seed_named_secret(pool: &PgPool, id: Uuid, key: &str, is_protected: bool) -> TestResult {
    seed_secret(pool, id, key, "named", is_protected).await
}

async fn seed_legacy_query_source(
    pool: &PgPool,
    id: Uuid,
    slug: &str,
    credential_ref: &str,
) -> TestResult {
    seed_legacy_query_source_with_url(
        pool,
        id,
        slug,
        "https://migration.example.test/api/v1/query",
        credential_ref,
    )
    .await
}

async fn seed_legacy_query_source_with_url(
    pool: &PgPool,
    id: Uuid,
    slug: &str,
    query_url: &str,
    credential_ref: &str,
) -> TestResult {
    sqlx::query(
        r#"
        INSERT INTO monitoring_query_source (
            id, name, slug, source_type, query_url, credential_ref,
            timeout_ms, options, enabled
        )
        VALUES (
            $1, $2, $3, 'victoriametrics',
            $4, $5,
            3000, '{}'::jsonb, true
        )
        "#,
    )
    .bind(id)
    .bind(format!("PWA-10 migration {slug}"))
    .bind(slug)
    .bind(query_url)
    .bind(credential_ref)
    .execute(pool)
    .await?;
    Ok(())
}

async fn seed_legacy_alert_source(pool: &PgPool, id: Uuid, credential_ref: &str) -> TestResult {
    seed_legacy_alert_source_with_url(
        pool,
        id,
        "https://alerts.migration.example.test/api/v1/alerts",
        credential_ref,
    )
    .await
}

async fn seed_legacy_alert_source_with_url(
    pool: &PgPool,
    id: Uuid,
    query_url: &str,
    credential_ref: &str,
) -> TestResult {
    sqlx::query(
        r#"
        INSERT INTO monitoring_alert_source (
            id, name, source_type, query_url, credential_ref,
            timeout_ms, options, enabled
        )
        VALUES (
            $1, $2, 'alertmanager',
            $3, $4,
            3000, '{}'::jsonb, true
        )
        "#,
    )
    .bind(id)
    .bind(format!("PWA-11 migration alert source {id}"))
    .bind(query_url)
    .bind(credential_ref)
    .execute(pool)
    .await?;
    Ok(())
}

async fn seed_query_source_binding(
    pool: &PgPool,
    source_id: Uuid,
    secret_id: Uuid,
    owner_scope_type: &str,
    owner_scope_id: Uuid,
) -> TestResult {
    sqlx::query(
        r#"
        INSERT INTO secret_usage_binding (
            consumer_type, consumer_id, field_key,
            owner_scope_type, owner_scope_id, secret_id
        )
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(QUERY_SOURCE_CREDENTIAL_CONSUMER_TYPE)
    .bind(source_id)
    .bind(QUERY_SOURCE_CREDENTIAL_FIELD_KEY)
    .bind(owner_scope_type)
    .bind(owner_scope_id)
    .bind(secret_id)
    .execute(pool)
    .await?;
    Ok(())
}

async fn seed_alert_source_binding(
    pool: &PgPool,
    source_id: Uuid,
    secret_id: Uuid,
    owner_scope_type: &str,
    owner_scope_id: Uuid,
) -> TestResult {
    sqlx::query(
        r#"
        INSERT INTO secret_usage_binding (
            consumer_type, consumer_id, field_key,
            owner_scope_type, owner_scope_id, secret_id
        )
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(ALERT_SOURCE_CREDENTIAL_CONSUMER_TYPE)
    .bind(source_id)
    .bind(ALERT_SOURCE_CREDENTIAL_FIELD_KEY)
    .bind(owner_scope_type)
    .bind(owner_scope_id)
    .bind(secret_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// The consolidated baseline already contains this old migration's schema, but
/// an existing deployment can still retain its SQLx bookkeeping row. Its
/// numeric version collides with PWA-10 while its SQLx description identifies
/// it as historical history that must be discarded, leaving PWA-10 pending.
async fn seed_legacy_prebaseline_version_two_history(pool: &PgPool) -> TestResult {
    sqlx::query(
        r#"
        INSERT INTO _sqlx_migrations (
            version, description, success, checksum, execution_time
        )
        VALUES ($1, $2, true, decode($3, 'hex'), 0)
        "#,
    )
    .bind(MIGRATION_VERSION)
    .bind(LEGACY_VERSION_TWO_DESCRIPTION)
    .bind(LEGACY_VERSION_TWO_CHECKSUM_HEX)
    .execute(pool)
    .await?;
    Ok(())
}

/// An applied version beyond this checkout's forward files must never be
/// silently forgotten by re-baselining. It may be a real deployment that this
/// binary is simply too old to understand.
async fn seed_unknown_future_forward_migration_history(
    database_url: &str,
) -> TestResult<Vec<MigrationHistoryRow>> {
    let pool = child_pool(database_url).await?;
    let result = async {
        let inserted = sqlx::query(
            r#"
            INSERT INTO _sqlx_migrations (
                version, description, success, checksum, execution_time
            )
            VALUES ($1, 'future deployment migration', true, decode($2, 'hex'), 0)
            "#,
        )
        .bind(UNKNOWN_FUTURE_FORWARD_MIGRATION_VERSION)
        .bind(LEGACY_VERSION_TWO_CHECKSUM_HEX)
        .execute(&pool)
        .await?
        .rows_affected();
        ensure(
            inserted == 1,
            "expected to seed exactly one unknown future migration history row",
        )?;
        migration_history(&pool).await
    }
    .await;
    pool.close().await;
    result
}

async fn delete_unknown_future_forward_migration_history(database_url: &str) -> TestResult {
    let pool = child_pool(database_url).await?;
    let result = async {
        let deleted = sqlx::query("DELETE FROM _sqlx_migrations WHERE version = $1")
            .bind(UNKNOWN_FUTURE_FORWARD_MIGRATION_VERSION)
            .execute(&pool)
            .await?
            .rows_affected();
        ensure(
            deleted == 1,
            "expected to remove exactly one unknown future migration history row",
        )
    }
    .await;
    pool.close().await;
    result
}

async fn delete_legacy_query_source(database_url: &str, id: Uuid) -> TestResult {
    let pool = child_pool(database_url).await?;
    let result = async {
        let deleted = sqlx::query("DELETE FROM monitoring_query_source WHERE id = $1")
            .bind(id)
            .execute(&pool)
            .await?
            .rows_affected();
        ensure(
            deleted == 1,
            "expected the isolated legacy query source to exist",
        )
    }
    .await;
    pool.close().await;
    result
}

async fn delete_query_source_binding(database_url: &str, source_id: Uuid) -> TestResult {
    let pool = child_pool(database_url).await?;
    let result = async {
        sqlx::query(
            r#"
            DELETE FROM secret_usage_binding
            WHERE consumer_type = $1
              AND consumer_id = $2
              AND field_key = $3
            "#,
        )
        .bind(QUERY_SOURCE_CREDENTIAL_CONSUMER_TYPE)
        .bind(source_id)
        .bind(QUERY_SOURCE_CREDENTIAL_FIELD_KEY)
        .execute(&pool)
        .await?;
        Ok(())
    }
    .await;
    pool.close().await;
    result
}

async fn delete_legacy_alert_source(database_url: &str, id: Uuid) -> TestResult {
    let pool = child_pool(database_url).await?;
    let result = async {
        let deleted = sqlx::query("DELETE FROM monitoring_alert_source WHERE id = $1")
            .bind(id)
            .execute(&pool)
            .await?
            .rows_affected();
        ensure(
            deleted == 1,
            "expected the isolated legacy alert source to exist",
        )
    }
    .await;
    pool.close().await;
    result
}

async fn delete_alert_source_binding(database_url: &str, source_id: Uuid) -> TestResult {
    let pool = child_pool(database_url).await?;
    let result = async {
        sqlx::query(
            r#"
            DELETE FROM secret_usage_binding
            WHERE consumer_type = $1
              AND consumer_id = $2
              AND field_key = $3
            "#,
        )
        .bind(ALERT_SOURCE_CREDENTIAL_CONSUMER_TYPE)
        .bind(source_id)
        .bind(ALERT_SOURCE_CREDENTIAL_FIELD_KEY)
        .execute(&pool)
        .await?;
        Ok(())
    }
    .await;
    pool.close().await;
    result
}

async fn raw_query_source_column_exists(pool: &PgPool) -> TestResult<bool> {
    Ok(sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1
            FROM information_schema.columns
            WHERE table_schema = 'public'
              AND table_name = 'monitoring_query_source'
              AND column_name = 'credential_ref'
        )
        "#,
    )
    .fetch_one(pool)
    .await?)
}

async fn raw_alert_source_column_exists(pool: &PgPool) -> TestResult<bool> {
    Ok(sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1
            FROM information_schema.columns
            WHERE table_schema = 'public'
              AND table_name = 'monitoring_alert_source'
              AND column_name = 'credential_ref'
        )
        "#,
    )
    .fetch_one(pool)
    .await?)
}

async fn migration_is_applied(pool: &PgPool) -> TestResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM _sqlx_migrations WHERE version = $1 AND success)",
    )
    .bind(MIGRATION_VERSION)
    .fetch_one(pool)
    .await?)
}

async fn alert_migration_is_applied(pool: &PgPool) -> TestResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM _sqlx_migrations WHERE version = $1 AND success)",
    )
    .bind(ALERT_MIGRATION_VERSION)
    .fetch_one(pool)
    .await?)
}

async fn migration_history(pool: &PgPool) -> TestResult<Vec<MigrationHistoryRow>> {
    let rows = sqlx::query(
        r#"
        SELECT
            version,
            description,
            installed_on::text AS installed_on,
            success,
            encode(checksum, 'hex') AS checksum_hex,
            execution_time
        FROM _sqlx_migrations
        ORDER BY version, installed_on
        "#,
    )
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            Ok(MigrationHistoryRow {
                version: row.try_get("version")?,
                description: row.try_get("description")?,
                installed_on: row.try_get("installed_on")?,
                success: row.try_get("success")?,
                checksum_hex: row.try_get("checksum_hex")?,
                execution_time: row.try_get("execution_time")?,
            })
        })
        .collect()
}

async fn corrupt_current_forward_migration_checksum(
    database_url: &str,
) -> TestResult<Vec<MigrationHistoryRow>> {
    let pool = child_pool(database_url).await?;
    let result = async {
        let changed = sqlx::query(
            r#"
            UPDATE _sqlx_migrations
            SET checksum = set_byte(checksum, 0, (get_byte(checksum, 0) + 1) % 256)
            WHERE version = $1
              AND success
            "#,
        )
        .bind(MIGRATION_VERSION)
        .execute(&pool)
        .await?
        .rows_affected();
        ensure(
            changed == 1,
            "expected exactly one applied PWA-10 migration to corrupt for the re-baseline checksum test",
        )?;
        migration_history(&pool).await
    }
    .await;
    pool.close().await;
    result
}

async fn rename_current_forward_migration_description(
    database_url: &str,
    description: &str,
) -> TestResult<Vec<MigrationHistoryRow>> {
    let pool = child_pool(database_url).await?;
    let result = async {
        let changed = sqlx::query(
            r#"
            UPDATE _sqlx_migrations
            SET description = $1
            WHERE version = $2
              AND success
            "#,
        )
        .bind(description)
        .bind(MIGRATION_VERSION)
        .execute(&pool)
        .await?
        .rows_affected();
        ensure(
            changed == 1,
            "expected exactly one applied PWA-10 migration to rename for the re-baseline identity test",
        )?;
        migration_history(&pool).await
    }
    .await;
    pool.close().await;
    result
}

async fn assert_rebaseline_rejects_unidentified_forward_history(
    database_url: &str,
    expected_history: &[MigrationHistoryRow],
) -> TestResult {
    let rebaseline_error = match apply_rebaseline(database_url).await {
        Ok(()) => {
            return Err(
                "re-baseline must abort when an applied forward migration is not current or explicitly known historical"
                    .into(),
            );
        }
        Err(error) => error,
    };
    let database_error = rebaseline_error
        .downcast_ref::<sqlx::Error>()
        .and_then(|error| error.as_database_error());
    let actual_sqlstate = database_error
        .and_then(|error| error.code())
        .map(|code| code.into_owned());
    ensure(
        actual_sqlstate.as_deref() == Some(REBASELINE_CHECKSUM_MISMATCH_SQLSTATE),
        format!(
            "unidentified forward history must fail with PostgreSQL SQLSTATE {REBASELINE_CHECKSUM_MISMATCH_SQLSTATE}, got {}",
            actual_sqlstate
                .as_deref()
                .unwrap_or("non-database re-baseline error"),
        ),
    )?;

    let pool = child_pool(database_url).await?;
    let result = async {
        let actual_history = migration_history(&pool).await?;
        ensure(
            actual_history.as_slice() == expected_history,
            "an unidentified forward history must leave migration history untouched",
        )?;
        ensure(
            !raw_query_source_column_exists(&pool).await?,
            "an unidentified forward history must not replay the applied PWA-10 migration",
        )
    }
    .await;
    pool.close().await;
    result
}

async fn assert_rebaseline_leaves_forward_migration_pending(database_url: &str) -> TestResult {
    let pool = child_pool(database_url).await?;
    let result = async {
        let versions: Vec<i64> = sqlx::query_scalar(
            "SELECT version FROM _sqlx_migrations WHERE success ORDER BY version",
        )
        .fetch_all(&pool)
        .await?;
        ensure(
            versions == vec![1],
            "re-baseline must stamp only canonical 0001 and leave 0002 pending",
        )?;
        let legacy_version_two_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*)::BIGINT FROM _sqlx_migrations WHERE version = $1 AND description = $2",
        )
        .bind(MIGRATION_VERSION)
        .bind(LEGACY_VERSION_TWO_DESCRIPTION)
        .fetch_one(&pool)
        .await?;
        ensure(
            legacy_version_two_count == 0,
            "re-baseline must discard a reused historical version-2 description so PWA-10 remains pending",
        )?;
        ensure(
            raw_query_source_column_exists(&pool).await?,
            "re-baseline must not apply the PWA-10 query-source column drop",
        )?;
        ensure(
            raw_alert_source_column_exists(&pool).await?,
            "re-baseline must not apply the PWA-11 alert-source column drop",
        )
    }
    .await;
    pool.close().await;
    result
}

async fn assert_rebaseline_preserves_current_forward_migration(
    database_url: &str,
    expected_history: Option<&[MigrationHistoryRow]>,
) -> TestResult {
    let pool = child_pool(database_url).await?;
    let result = async {
        let versions: Vec<i64> = sqlx::query_scalar(
            "SELECT version FROM _sqlx_migrations WHERE success ORDER BY version",
        )
        .fetch_all(&pool)
        .await?;
        let mut expected_versions = vec![1];
        expected_versions.extend(canonical_forward_migration_versions()?);
        ensure(
            versions == expected_versions,
            "re-baseline must retain every already-successful checksum-matching forward migration",
        )?;
        ensure(
            migration_is_applied(&pool).await?,
            "re-baseline must retain the current PWA-10 migration bookkeeping row",
        )?;
        if let Some(expected_history) = expected_history {
            let actual_history = migration_history(&pool).await?;
            let expected_current = expected_history
                .iter()
                .find(|row| row.version == MIGRATION_VERSION)
                .ok_or("expected history is missing the renamed PWA-10 migration")?;
            let actual_current = actual_history
                .iter()
                .find(|row| row.version == MIGRATION_VERSION);
            ensure(
                actual_current == Some(expected_current),
                "re-baseline must preserve the current migration row even when its SQLx description changed",
            )?;
        }
        ensure(
            !raw_query_source_column_exists(&pool).await?,
            "retaining an applied PWA-10 migration must not restore the dropped query-source column",
        )?;
        ensure(
            !raw_alert_source_column_exists(&pool).await?,
            "retaining an applied PWA-11 migration must not restore the dropped alert-source column",
        )
    }
    .await;
    pool.close().await;
    result
}

async fn query_source_binding_count(pool: &PgPool) -> TestResult<i64> {
    Ok(sqlx::query_scalar(
        r#"
        SELECT COUNT(*)::BIGINT
        FROM secret_usage_binding
        WHERE consumer_type = $1
        "#,
    )
    .bind(QUERY_SOURCE_CREDENTIAL_CONSUMER_TYPE)
    .fetch_one(pool)
    .await?)
}

async fn alert_source_binding_count(pool: &PgPool) -> TestResult<i64> {
    Ok(sqlx::query_scalar(
        r#"
        SELECT COUNT(*)::BIGINT
        FROM secret_usage_binding
        WHERE consumer_type = $1
        "#,
    )
    .bind(ALERT_SOURCE_CREDENTIAL_CONSUMER_TYPE)
    .fetch_one(pool)
    .await?)
}

async fn assert_failed_migration_is_atomic(
    database_url: &str,
    expected_preexisting_binding_count: i64,
) -> TestResult {
    let pool = child_pool(database_url).await?;
    let result = async {
        ensure(
            raw_query_source_column_exists(&pool).await?,
            "failed PWA-10 migration must leave the legacy query-source column intact",
        )?;
        ensure(
            raw_alert_source_column_exists(&pool).await?,
            "a failed migration batch must leave the legacy alert-source column intact",
        )?;
        ensure(
            !migration_is_applied(&pool).await?,
            "failed PWA-10 migration must not stamp version 2",
        )?;
        ensure(
            query_source_binding_count(&pool).await? == expected_preexisting_binding_count,
            "failed PWA-10 migration must not leave partial query-source bindings",
        )?;
        ensure(
            alert_source_binding_count(&pool).await? == 0,
            "a failed migration batch must not backfill alert-source bindings",
        )
    }
    .await;
    pool.close().await;
    result
}

/// SQLx applies `0002` and `0003` in separate transactions, so an aborting
/// `0003` commits `0002` while rolling back only the alert-source migration.
/// This helper asserts exactly that split state: the query column is gone and
/// version 2 is stamped, while the alert column is intact, version 3 is not
/// stamped, and only the alert bindings that already existed before the run
/// survive. It deliberately does not require the query binding count to be
/// zero, because the committed `0002` backfill is expected to persist.
async fn assert_failed_alert_migration_is_atomic(
    database_url: &str,
    expected_preexisting_alert_binding_count: i64,
) -> TestResult {
    let pool = child_pool(database_url).await?;
    let result = async {
        ensure(
            !raw_query_source_column_exists(&pool).await?,
            "an aborting PWA-11 migration must observe the committed PWA-10 query-source column drop",
        )?;
        ensure(
            migration_is_applied(&pool).await?,
            "an aborting PWA-11 migration must observe the committed version 2 stamp",
        )?;
        ensure(
            raw_alert_source_column_exists(&pool).await?,
            "failed PWA-11 migration must leave the legacy alert-source column intact",
        )?;
        ensure(
            !alert_migration_is_applied(&pool).await?,
            "failed PWA-11 migration must not stamp version 3",
        )?;
        ensure(
            alert_source_binding_count(&pool).await? == expected_preexisting_alert_binding_count,
            "failed PWA-11 migration must not leave partial alert-source bindings",
        )
    }
    .await;
    pool.close().await;
    result
}

async fn assert_successful_backfill(
    database_url: &str,
    expected_bindings: &[(Uuid, Uuid)],
    expected_alert_binding: (Uuid, Uuid),
) -> TestResult {
    let pool = child_pool(database_url).await?;
    let result = async {
        ensure(
            !raw_query_source_column_exists(&pool).await?,
            "successful PWA-10 migration must drop the query-source raw credential column",
        )?;
        ensure(
            !raw_alert_source_column_exists(&pool).await?,
            "successful PWA-11 migration must drop the alert-source raw credential column",
        )?;
        ensure(
            migration_is_applied(&pool).await?,
            "successful PWA-10 migration must stamp version 2",
        )?;
        ensure(
            query_source_binding_count(&pool).await? == expected_bindings.len() as i64,
            "successful PWA-10 migration must create exactly one canonical binding per legacy source",
        )?;

        for (source_id, secret_id) in expected_bindings {
            let row = sqlx::query(
                r#"
                SELECT secret_id, owner_scope_type, owner_scope_id
                FROM secret_usage_binding
                WHERE consumer_type = $1
                  AND consumer_id = $2
                  AND field_key = $3
                "#,
            )
            .bind(QUERY_SOURCE_CREDENTIAL_CONSUMER_TYPE)
            .bind(*source_id)
            .bind(QUERY_SOURCE_CREDENTIAL_FIELD_KEY)
            .fetch_optional(&pool)
            .await?;
            let Some(row) = row else {
                return Err("successful PWA-10 migration is missing a canonical binding".into());
            };
            let actual_secret_id: Uuid = row.try_get("secret_id")?;
            let owner_scope_type: String = row.try_get("owner_scope_type")?;
            let owner_scope_id: Uuid = row.try_get("owner_scope_id")?;
            ensure(
                actual_secret_id == *secret_id,
                "successful PWA-10 migration must preserve the named secret identity",
            )?;
            ensure(
                owner_scope_type == QUERY_SOURCE_CREDENTIAL_OWNER_SCOPE_TYPE
                    && owner_scope_id == *source_id,
                "successful PWA-10 migration must use the canonical query-source owner scope",
            )?;
        }

        // The alert source was seeded with a legacy spelling of its named
        // secret UUID. A successful migration must project it into exactly one
        // canonical binding and leave no free-text reference behind: the only
        // surviving copy of the credential identity is the typed `secret_id`.
        let (alert_source_id, alert_secret_id) = expected_alert_binding;
        ensure(
            alert_source_binding_count(&pool).await? == 1,
            "successful PWA-11 migration must create exactly one canonical alert-source binding",
        )?;
        let alert_row = sqlx::query(
            r#"
            SELECT secret_id, owner_scope_type, owner_scope_id
            FROM secret_usage_binding
            WHERE consumer_type = $1
              AND consumer_id = $2
              AND field_key = $3
            "#,
        )
        .bind(ALERT_SOURCE_CREDENTIAL_CONSUMER_TYPE)
        .bind(alert_source_id)
        .bind(ALERT_SOURCE_CREDENTIAL_FIELD_KEY)
        .fetch_optional(&pool)
        .await?;
        let Some(alert_row) = alert_row else {
            return Err("successful PWA-11 migration is missing a canonical alert-source binding".into());
        };
        let alert_bound_secret_id: Uuid = alert_row.try_get("secret_id")?;
        let alert_owner_scope_type: String = alert_row.try_get("owner_scope_type")?;
        let alert_owner_scope_id: Uuid = alert_row.try_get("owner_scope_id")?;
        ensure(
            alert_bound_secret_id == alert_secret_id,
            "successful PWA-11 migration must preserve the alert-source named secret identity",
        )?;
        ensure(
            alert_owner_scope_type == ALERT_SOURCE_CREDENTIAL_OWNER_SCOPE_TYPE
                && alert_owner_scope_id == alert_source_id,
            "successful PWA-11 migration must use the canonical alert-source owner scope",
        )?;

        let shape_constraint_present: bool = sqlx::query_scalar(
            r#"
            SELECT EXISTS(
                SELECT 1
                FROM pg_constraint
                WHERE conname = 'chk_secret_usage_binding_monitoring_query_source_shape'
            )
            "#,
        )
        .fetch_one(&pool)
        .await?;
        ensure(
            shape_constraint_present,
            "successful PWA-10 migration must enforce canonical binding shape",
        )?;

        let alert_shape_constraint_present: bool = sqlx::query_scalar(
            r#"
            SELECT EXISTS(
                SELECT 1
                FROM pg_constraint
                WHERE conname = 'chk_secret_usage_binding_monitoring_alert_source_shape'
            )
            "#,
        )
        .fetch_one(&pool)
        .await?;
        ensure(
            alert_shape_constraint_present,
            "successful PWA-11 migration must enforce canonical alert-source binding shape",
        )?;
        let alert_url_constraint_present: bool = sqlx::query_scalar(
            r#"
            SELECT EXISTS(
                SELECT 1
                FROM pg_constraint
                WHERE conname = 'chk_monitoring_alert_source_endpoint_url'
            )
            "#,
        )
        .fetch_one(&pool)
        .await?;
        ensure(
            alert_url_constraint_present,
            "successful PWA-11 migration must enforce endpoint-only alert-source URLs",
        )
    }
    .await;
    pool.close().await;
    result
}

#[allow(
    clippy::expect_used,
    reason = "a rejected constraint write is the asserted outcome of this migration-contract helper; the panic message names the missing CHECK"
)]
async fn assert_shape_constraint_rejects_invalid_binding(
    database_url: &str,
    source_id: Uuid,
    secret_id: Uuid,
) -> TestResult {
    let pool = child_pool(database_url).await?;
    let result = async {
        let error = sqlx::query(
            r#"
            INSERT INTO secret_usage_binding (
                consumer_type, consumer_id, field_key,
                owner_scope_type, owner_scope_id, secret_id
            )
            VALUES ($1, $2, 'not_bearer_token', 'not_monitoring_query_source', $3, $4)
            "#,
        )
        .bind(QUERY_SOURCE_CREDENTIAL_CONSUMER_TYPE)
        .bind(source_id)
        .bind(Uuid::now_v7())
        .bind(secret_id)
        .execute(&pool)
        .await
        .expect_err("PWA-10 binding-shape CHECK must reject an invalid query-source binding");
        let sqlstate = error
            .as_database_error()
            .and_then(|database_error| database_error.code())
            .map(|code| code.into_owned());
        ensure(
            sqlstate.as_deref() == Some("23514"),
            "PWA-10 binding-shape CHECK must fail with PostgreSQL check-violation SQLSTATE 23514",
        )
    }
    .await;
    pool.close().await;
    result
}

#[allow(
    clippy::expect_used,
    reason = "a rejected constraint write is the asserted outcome of this migration-contract helper; the panic message names the missing CHECK"
)]
async fn assert_endpoint_url_constraint_rejects_inline_credential_channel(
    database_url: &str,
    source_id: Uuid,
) -> TestResult {
    let pool = child_pool(database_url).await?;
    let result = async {
        let constraint_present: bool = sqlx::query_scalar(
            r#"
            SELECT EXISTS(
                SELECT 1
                FROM pg_constraint
                WHERE conname = 'chk_monitoring_query_source_endpoint_url'
            )
            "#,
        )
        .fetch_one(&pool)
        .await?;
        ensure(
            constraint_present,
            "successful PWA-10 migration must enforce endpoint-only query-source URLs",
        )?;

        for (channel, query_url) in [
            (
                "query component",
                "https://monitoring.example.test/api/v1/query?scope=platform",
            ),
            (
                "userinfo after extra slash",
                "https:///operator:placeholder@monitoring.example.test/api/v1/query",
            ),
            (
                "userinfo without slash",
                "https:operator:placeholder@monitoring.example.test/api/v1/query",
            ),
            (
                "userinfo after backslash",
                r"https:\\\\operator:placeholder@monitoring.example.test/api/v1/query",
            ),
        ] {
            let error = sqlx::query("UPDATE monitoring_query_source SET query_url = $1 WHERE id = $2")
                .bind(query_url)
                .bind(source_id)
                .execute(&pool)
                .await
                .expect_err("PWA-10 endpoint URL CHECK must reject an inline credential channel");
            let sqlstate = error
                .as_database_error()
                .and_then(|database_error| database_error.code())
                .map(|code| code.into_owned());
            ensure(
                sqlstate.as_deref() == Some("23514"),
                format!(
                    "PWA-10 endpoint URL CHECK must reject {channel} with PostgreSQL check-violation SQLSTATE 23514"
                ),
            )?;
        }

        Ok(())
    }
    .await;
    pool.close().await;
    result
}

#[allow(
    clippy::expect_used,
    reason = "a rejected constraint write is the asserted outcome of this migration-contract helper; the panic message names the missing CHECK"
)]
async fn assert_alert_shape_constraint_rejects_invalid_binding(
    database_url: &str,
    source_id: Uuid,
    secret_id: Uuid,
) -> TestResult {
    let pool = child_pool(database_url).await?;
    let result = async {
        let error = sqlx::query(
            r#"
            INSERT INTO secret_usage_binding (
                consumer_type, consumer_id, field_key,
                owner_scope_type, owner_scope_id, secret_id
            )
            VALUES ($1, $2, 'not_bearer_token', 'not_monitoring_alert_source', $3, $4)
            "#,
        )
        .bind(ALERT_SOURCE_CREDENTIAL_CONSUMER_TYPE)
        .bind(source_id)
        .bind(Uuid::now_v7())
        .bind(secret_id)
        .execute(&pool)
        .await
        .expect_err("PWA-11 binding-shape CHECK must reject an invalid alert-source binding");
        let sqlstate = error
            .as_database_error()
            .and_then(|database_error| database_error.code())
            .map(|code| code.into_owned());
        ensure(
            sqlstate.as_deref() == Some("23514"),
            "PWA-11 binding-shape CHECK must fail with PostgreSQL check-violation SQLSTATE 23514",
        )
    }
    .await;
    pool.close().await;
    result
}

#[allow(
    clippy::expect_used,
    reason = "a rejected constraint write is the asserted outcome of this migration-contract helper; the panic message names the missing CHECK"
)]
async fn assert_alert_endpoint_url_constraint_rejects_inline_credential_channel(
    database_url: &str,
    source_id: Uuid,
) -> TestResult {
    let pool = child_pool(database_url).await?;
    let result = async {
        let constraint_present: bool = sqlx::query_scalar(
            r#"
            SELECT EXISTS(
                SELECT 1
                FROM pg_constraint
                WHERE conname = 'chk_monitoring_alert_source_endpoint_url'
            )
            "#,
        )
        .fetch_one(&pool)
        .await?;
        ensure(
            constraint_present,
            "successful PWA-11 migration must enforce endpoint-only alert-source URLs",
        )?;

        for (channel, query_url) in [
            (
                "query component",
                "https://monitoring.example.test/api/v1/alerts?scope=platform",
            ),
            (
                "userinfo after extra slash",
                "https:///operator:placeholder@monitoring.example.test/api/v1/alerts",
            ),
            (
                "userinfo without slash",
                "https:operator:placeholder@monitoring.example.test/api/v1/alerts",
            ),
            (
                "userinfo after backslash",
                r"https:\\\\operator:placeholder@monitoring.example.test/api/v1/alerts",
            ),
        ] {
            let error =
                sqlx::query("UPDATE monitoring_alert_source SET query_url = $1 WHERE id = $2")
                    .bind(query_url)
                    .bind(source_id)
                    .execute(&pool)
                    .await
                    .expect_err("PWA-11 endpoint URL CHECK must reject an inline credential channel");
            let sqlstate = error
                .as_database_error()
                .and_then(|database_error| database_error.code())
                .map(|code| code.into_owned());
            ensure(
                sqlstate.as_deref() == Some("23514"),
                format!(
                    "PWA-11 endpoint URL CHECK must reject {channel} with PostgreSQL check-violation SQLSTATE 23514"
                ),
            )?;
        }

        Ok(())
    }
    .await;
    pool.close().await;
    result
}

/// Every identifier, reference and expected binding the migration contract
/// drives through its phases. Built once and handed to each phase helper so the
/// fixture values stay identical across the whole run.
#[derive(Clone)]
struct MigrationFixtures {
    bare_secret_id: Uuid,
    simple_secret_id: Uuid,
    braced_secret_id: Uuid,
    short_secret_id: Uuid,
    uri_secret_id: Uuid,
    urn_secret_id: Uuid,
    short_urn_secret_id: Uuid,
    uri_urn_secret_id: Uuid,
    bare_source_id: Uuid,
    simple_source_id: Uuid,
    braced_source_id: Uuid,
    short_source_id: Uuid,
    uri_source_id: Uuid,
    urn_source_id: Uuid,
    short_urn_source_id: Uuid,
    uri_urn_source_id: Uuid,
    alert_secret_id: Uuid,
    alert_source_id: Uuid,
    bare_reference: String,
    simple_reference: String,
    braced_reference: String,
    short_reference: String,
    uri_reference: String,
    urn_reference: String,
    short_urn_reference: String,
    uri_urn_reference: String,
    alert_reference: String,
    query_bindings: [(Uuid, Uuid); 8],
    alert_binding: (Uuid, Uuid),
}

async fn exercise_migration(database_url: &str) -> TestResult {
    let (_scratch, complete, fixtures) = prepare_migration_test(database_url).await?;
    run_query_source_rejection_phases(database_url, &complete, &fixtures).await?;
    run_alert_source_rejection_phases(database_url, &complete, &fixtures).await?;
    run_backfill_and_rebaseline_phases(database_url, &complete, &fixtures).await
}

async fn prepare_migration_test(
    database_url: &str,
) -> TestResult<(tempfile::TempDir, PathBuf, MigrationFixtures)> {
    let scratch = tempfile::tempdir()?;
    let baseline = copy_migrations(scratch.path(), "baseline", false)?;
    let complete = copy_migrations(scratch.path(), "complete", true)?;
    apply_migrations(database_url, &baseline).await?;
    let fixtures = prepare_migration_fixtures(database_url).await?;
    Ok((scratch, complete, fixtures))
}

async fn prepare_migration_fixtures(database_url: &str) -> TestResult<MigrationFixtures> {
    let fixtures = build_migration_fixtures().await?;
    seed_legacy_fixture_rows(database_url, &fixtures).await?;
    assert_historical_rebaseline_leaves_pwa10_pending(database_url).await?;
    Ok(fixtures)
}

async fn run_query_source_rejection_phases(
    database_url: &str,
    complete: &Path,
    fixtures: &MigrationFixtures,
) -> TestResult {
    reject_query_source_url_and_credential_cases(database_url, complete, fixtures).await?;
    reject_query_source_credential_cases(database_url, complete).await?;
    reject_query_source_binding_cases(database_url, complete).await?;
    seed_stale_owner_binding(database_url, fixtures).await
}

async fn run_alert_source_rejection_phases(
    database_url: &str,
    complete: &Path,
    fixtures: &MigrationFixtures,
) -> TestResult {
    reject_alert_url_and_credential_cases(database_url, complete, fixtures).await?;
    reject_alert_credential_cases(database_url, complete).await?;
    reject_alert_binding_cases(database_url, complete).await
}

async fn run_backfill_and_rebaseline_phases(
    database_url: &str,
    complete: &Path,
    fixtures: &MigrationFixtures,
) -> TestResult {
    run_successful_backfill_and_constraint_phases(database_url, complete, fixtures).await?;
    run_rebaseline_history_phases(database_url, complete, fixtures).await
}

async fn run_successful_backfill_and_constraint_phases(
    database_url: &str,
    complete: &Path,
    fixtures: &MigrationFixtures,
) -> TestResult {
    apply_migrations(database_url, complete).await?;
    assert_successful_backfill(
        database_url,
        &fixtures.query_bindings,
        fixtures.alert_binding,
    )
    .await?;
    assert_binding_constraints(database_url, fixtures).await
}

async fn assert_binding_constraints(
    database_url: &str,
    fixtures: &MigrationFixtures,
) -> TestResult {
    assert_shape_constraint_rejects_invalid_binding(
        database_url,
        fixtures.bare_source_id,
        fixtures.bare_secret_id,
    )
    .await?;
    assert_endpoint_url_constraint_rejects_inline_credential_channel(
        database_url,
        fixtures.bare_source_id,
    )
    .await?;
    assert_alert_shape_constraint_rejects_invalid_binding(
        database_url,
        fixtures.alert_source_id,
        fixtures.alert_secret_id,
    )
    .await?;
    assert_alert_endpoint_url_constraint_rejects_inline_credential_channel(
        database_url,
        fixtures.alert_source_id,
    )
    .await
}

async fn run_rebaseline_history_phases(
    database_url: &str,
    complete: &Path,
    fixtures: &MigrationFixtures,
) -> TestResult {
    run_rebaseline_preservation(database_url, complete, fixtures).await?;
    run_renamed_history_preservation(database_url, complete, fixtures).await?;
    run_unknown_future_history_rejection(database_url).await?;
    run_corrupted_checksum_rejection(database_url).await
}

async fn reject_query_source_url_and_credential_cases(
    database_url: &str,
    complete: &Path,
    fixtures: &MigrationFixtures,
) -> TestResult {
    assert_query_source_url_rejections(database_url, complete, fixtures).await?;
    assert_malformed_query_source_rejected(database_url, complete).await
}

async fn reject_query_source_credential_cases(database_url: &str, complete: &Path) -> TestResult {
    assert_urn_simple_query_credential_rejected(database_url, complete).await?;
    assert_postgres_only_query_credential_rejected(database_url, complete).await?;
    assert_missing_query_secret_rejected(database_url, complete).await?;
    assert_field_query_secret_rejected(database_url, complete).await?;
    assert_protected_query_secret_rejected(database_url, complete).await
}

async fn reject_query_source_binding_cases(database_url: &str, complete: &Path) -> TestResult {
    assert_orphan_query_binding_rejected(database_url, complete).await?;
    assert_conflicting_query_binding_rejected(database_url, complete).await
}

async fn reject_alert_url_and_credential_cases(
    database_url: &str,
    complete: &Path,
    fixtures: &MigrationFixtures,
) -> TestResult {
    assert_alert_url_rejections(database_url, complete, fixtures).await?;
    assert_malformed_alert_credential_rejected(database_url, complete).await
}

async fn reject_alert_credential_cases(database_url: &str, complete: &Path) -> TestResult {
    assert_urn_simple_alert_credential_rejected(database_url, complete).await?;
    assert_postgres_only_alert_credential_rejected(database_url, complete).await?;
    assert_missing_alert_secret_rejected(database_url, complete).await?;
    assert_field_alert_secret_rejected(database_url, complete).await?;
    assert_protected_alert_secret_rejected(database_url, complete).await
}

async fn reject_alert_binding_cases(database_url: &str, complete: &Path) -> TestResult {
    assert_orphan_alert_binding_rejected(database_url, complete).await?;
    assert_conflicting_alert_binding_rejected(database_url, complete).await
}

async fn seed_legacy_fixture_rows(database_url: &str, fixtures: &MigrationFixtures) -> TestResult {
    let pool = child_pool(database_url).await?;
    let result = seed_legacy_fixture_rows_with_pool(&pool, fixtures).await;
    pool.close().await;
    result
}

async fn seed_legacy_fixture_rows_with_pool(
    pool: &PgPool,
    fixtures: &MigrationFixtures,
) -> TestResult {
    seed_query_fixture_secrets(pool, fixtures).await?;
    seed_query_fixture_sources(pool, fixtures).await?;
    seed_alert_fixture(pool, fixtures).await?;
    seed_legacy_prebaseline_version_two_history(pool).await
}

async fn seed_query_fixture_secrets(pool: &PgPool, fixtures: &MigrationFixtures) -> TestResult {
    seed_query_fixture_secrets_a(pool, fixtures).await?;
    seed_query_fixture_secrets_b(pool, fixtures).await
}

async fn seed_query_fixture_sources(pool: &PgPool, fixtures: &MigrationFixtures) -> TestResult {
    seed_query_fixture_sources_a(pool, fixtures).await?;
    seed_query_fixture_sources_b(pool, fixtures).await
}

async fn seed_query_fixture_secrets_a(pool: &PgPool, fixtures: &MigrationFixtures) -> TestResult {
    let MigrationFixtures {
        bare_secret_id,
        simple_secret_id,
        braced_secret_id,
        short_secret_id,
        ..
    } = fixtures.clone();
    seed_named_secret(pool, bare_secret_id, "pwa10-migration-bare", false).await?;
    seed_named_secret(pool, simple_secret_id, "pwa10-migration-simple", false).await?;
    seed_named_secret(pool, braced_secret_id, "pwa10-migration-braced", false).await?;
    seed_named_secret(pool, short_secret_id, "pwa10-migration-short", false).await
}

async fn seed_query_fixture_secrets_b(pool: &PgPool, fixtures: &MigrationFixtures) -> TestResult {
    let MigrationFixtures {
        uri_secret_id,
        urn_secret_id,
        short_urn_secret_id,
        uri_urn_secret_id,
        ..
    } = fixtures.clone();
    seed_named_secret(pool, uri_secret_id, "pwa10-migration-uri", false).await?;
    seed_named_secret(pool, urn_secret_id, "pwa10-migration-urn", false).await?;
    seed_named_secret(
        pool,
        short_urn_secret_id,
        "pwa10-migration-short-urn",
        false,
    )
    .await?;
    seed_named_secret(pool, uri_urn_secret_id, "pwa10-migration-uri-urn", false).await
}

async fn seed_query_fixture_sources_a(pool: &PgPool, fixtures: &MigrationFixtures) -> TestResult {
    let MigrationFixtures {
        bare_source_id,
        bare_reference,
        simple_source_id,
        simple_reference,
        braced_source_id,
        braced_reference,
        short_source_id,
        short_reference,
        ..
    } = fixtures.clone();
    seed_legacy_query_source(
        pool,
        bare_source_id,
        "pwa10-migration-bare",
        &bare_reference,
    )
    .await?;
    seed_legacy_query_source(
        pool,
        simple_source_id,
        "pwa10-migration-simple",
        &simple_reference,
    )
    .await?;
    seed_legacy_query_source(
        pool,
        braced_source_id,
        "pwa10-migration-braced",
        &braced_reference,
    )
    .await?;
    seed_legacy_query_source(
        pool,
        short_source_id,
        "pwa10-migration-short",
        &short_reference,
    )
    .await
}

async fn seed_query_fixture_sources_b(pool: &PgPool, fixtures: &MigrationFixtures) -> TestResult {
    let MigrationFixtures {
        uri_source_id,
        uri_reference,
        urn_source_id,
        urn_reference,
        short_urn_source_id,
        short_urn_reference,
        uri_urn_source_id,
        uri_urn_reference,
        ..
    } = fixtures.clone();
    seed_legacy_query_source(pool, uri_source_id, "pwa10-migration-uri", &uri_reference).await?;
    seed_legacy_query_source(pool, urn_source_id, "pwa10-migration-urn", &urn_reference).await?;
    seed_legacy_query_source(
        pool,
        short_urn_source_id,
        "pwa10-migration-short-urn",
        &short_urn_reference,
    )
    .await?;
    seed_legacy_query_source(
        pool,
        uri_urn_source_id,
        "pwa10-migration-uri-urn",
        &uri_urn_reference,
    )
    .await
}

async fn seed_alert_fixture(pool: &PgPool, fixtures: &MigrationFixtures) -> TestResult {
    let MigrationFixtures {
        alert_secret_id,
        alert_source_id,
        alert_reference,
        ..
    } = fixtures.clone();
    seed_named_secret(pool, alert_secret_id, "pwa11-migration-alert", false).await?;
    seed_legacy_alert_source(pool, alert_source_id, &alert_reference).await
}

async fn build_migration_fixtures() -> TestResult<MigrationFixtures> {
    let bare_secret_id = Uuid::now_v7();
    let simple_secret_id = Uuid::now_v7();
    let braced_secret_id = Uuid::now_v7();
    let short_secret_id = Uuid::now_v7();
    let uri_secret_id = Uuid::now_v7();
    let urn_secret_id = Uuid::now_v7();
    let short_urn_secret_id = Uuid::now_v7();
    let uri_urn_secret_id = Uuid::now_v7();
    let bare_source_id = Uuid::now_v7();
    let simple_source_id = Uuid::now_v7();
    let braced_source_id = Uuid::now_v7();
    let short_source_id = Uuid::now_v7();
    let uri_source_id = Uuid::now_v7();
    let urn_source_id = Uuid::now_v7();
    let short_urn_source_id = Uuid::now_v7();
    let uri_urn_source_id = Uuid::now_v7();
    let alert_secret_id = Uuid::now_v7();
    let alert_source_id = Uuid::now_v7();
    let bare_reference = bare_secret_id.to_string();
    let simple_reference = simple_secret_id.simple().to_string();
    let braced_reference = format!("{{{braced_secret_id}}}");
    let short_reference = format!("secret:{short_secret_id}");
    let uri_reference = format!("secret://{uri_secret_id}");
    let urn_reference = format!("urn:uuid:{urn_secret_id}");
    let short_urn_reference = format!("secret:urn:uuid:{short_urn_secret_id}");
    let uri_urn_reference = format!("secret://urn:uuid:{uri_urn_secret_id}");
    // PWA-11 seeds its alert source with a historical `secret://` spelling so
    // the migration proves it normalizes the legacy grammar rather than merely
    // copying text into a typed column.
    let alert_reference = format!("secret://{alert_secret_id}");
    ensure(
        Uuid::parse_str(&simple_reference).ok() == Some(simple_secret_id),
        "the legacy simple UUID fixture must parse to its named secret id",
    )?;
    ensure(
        Uuid::parse_str(&braced_reference).ok() == Some(braced_secret_id),
        "the legacy braced UUID fixture must parse to its named secret id",
    )?;

    Ok(MigrationFixtures {
        bare_secret_id,
        simple_secret_id,
        braced_secret_id,
        short_secret_id,
        uri_secret_id,
        urn_secret_id,
        short_urn_secret_id,
        uri_urn_secret_id,
        bare_source_id,
        simple_source_id,
        braced_source_id,
        short_source_id,
        uri_source_id,
        urn_source_id,
        short_urn_source_id,
        uri_urn_source_id,
        alert_secret_id,
        alert_source_id,
        bare_reference,
        simple_reference,
        braced_reference,
        short_reference,
        uri_reference,
        urn_reference,
        short_urn_reference,
        uri_urn_reference,
        alert_reference,
        query_bindings: [
            (bare_source_id, bare_secret_id),
            (simple_source_id, simple_secret_id),
            (braced_source_id, braced_secret_id),
            (short_source_id, short_secret_id),
            (uri_source_id, uri_secret_id),
            (urn_source_id, urn_secret_id),
            (short_urn_source_id, short_urn_secret_id),
            (uri_urn_source_id, uri_urn_secret_id),
        ],
        alert_binding: (alert_source_id, alert_secret_id),
    })
}

async fn assert_historical_rebaseline_leaves_pwa10_pending(database_url: &str) -> TestResult {
    // Historical databases need the re-baseline bridge before SQLx sees the
    // consolidated 0001. It must discard the old version-2 `init audit` row
    // by its distinct SQLx description, leaving the current PWA-10 version 2
    // pending so the real migration can backfill these rows.
    apply_rebaseline(database_url).await?;
    assert_rebaseline_leaves_forward_migration_pending(database_url).await
}

async fn assert_query_source_url_rejections(
    database_url: &str,
    complete: &Path,
    fixtures: &MigrationFixtures,
) -> TestResult {
    // Query-source URLs are durable Monitoring read metadata. Before any
    // binding backfill, reject every legacy row that could hide a credential in
    // URL userinfo, a query component, or a fragment. The raw URL itself must
    // not appear in the failure output.
    for (label, query_url) in [
        (
            "userinfo",
            "https://operator@migration.example.test/api/v1/query",
        ),
        (
            "userinfo after extra slash",
            "https:///operator:placeholder@migration.example.test/api/v1/query",
        ),
        (
            "userinfo without slash",
            "https:operator:placeholder@migration.example.test/api/v1/query",
        ),
        (
            "userinfo after backslash",
            r"https:\\\\operator:placeholder@migration.example.test/api/v1/query",
        ),
        (
            "query component",
            "https://migration.example.test/api/v1/query?scope=platform",
        ),
        (
            "fragment",
            "https://migration.example.test/api/v1/query#diagnostic",
        ),
    ] {
        assert_query_source_url_rejected(database_url, complete, label, query_url, fixtures)
            .await?;
    }
    Ok(())
}

async fn assert_query_source_url_rejected(
    database_url: &str,
    complete: &Path,
    label: &str,
    query_url: &str,
    fixtures: &MigrationFixtures,
) -> TestResult {
    let MigrationFixtures { bare_reference, .. } = fixtures.clone();
    let source_id = Uuid::now_v7();
    let pool = child_pool(database_url).await?;
    let seed = seed_legacy_query_source_with_url(
        &pool,
        source_id,
        &format!("pwa10-migration-url-{label}"),
        query_url,
        &bare_reference,
    )
    .await;
    pool.close().await;
    seed?;
    assert_migration_aborts_with_sqlstate(
        database_url,
        complete,
        "22023",
        "a legacy query-source URL with an inline credential channel",
        "PWA-10",
    )
    .await?;
    assert_failed_migration_is_atomic(database_url, 0).await?;
    delete_legacy_query_source(database_url, source_id).await
}

async fn assert_malformed_query_source_rejected(database_url: &str, complete: &Path) -> TestResult {
    // Invalid text must abort the entire migration before valid rows are
    // backfilled. No raw input value is reported by either migration or test.
    let malformed_source_id = Uuid::now_v7();
    let pool = child_pool(database_url).await?;
    let malformed_seed = seed_legacy_query_source(
        &pool,
        malformed_source_id,
        "pwa10-migration-malformed",
        "secret://not-a-uuid",
    )
    .await;
    pool.close().await;
    malformed_seed?;
    assert_migration_aborts_with_sqlstate(
        database_url,
        complete,
        "22023",
        "a malformed legacy query credential",
        "PWA-10",
    )
    .await?;
    assert_failed_migration_is_atomic(database_url, 0).await?;
    delete_legacy_query_source(database_url, malformed_source_id).await
}

async fn assert_urn_simple_query_credential_rejected(
    database_url: &str,
    complete: &Path,
) -> TestResult {
    // The legacy parser accepted a UUID simple form by itself, but only a
    // hyphenated UUID after the exact lowercase `urn:uuid:` prefix. Keep that
    // distinction explicit so a future guard cannot accidentally broaden the
    // legacy language while normalizing before PostgreSQL's cast.
    let urn_simple_secret_id = Uuid::now_v7();
    let urn_simple_source_id = Uuid::now_v7();
    let urn_simple_reference = format!("urn:uuid:{}", urn_simple_secret_id.simple());
    ensure(
        Uuid::parse_str(&urn_simple_reference).is_err(),
        "the simple UUID form must remain invalid after the legacy URN prefix",
    )?;
    seed_urn_simple_query_source(
        database_url,
        urn_simple_secret_id,
        urn_simple_source_id,
        &urn_simple_reference,
    )
    .await?;
    assert_migration_aborts_with_sqlstate(
        database_url,
        complete,
        "22023",
        "a simple UUID after the legacy URN prefix",
        "PWA-10",
    )
    .await?;
    assert_failed_migration_is_atomic(database_url, 0).await?;
    delete_legacy_query_source(database_url, urn_simple_source_id).await
}

async fn seed_urn_simple_query_source(
    database_url: &str,
    secret_id: Uuid,
    source_id: Uuid,
    reference: &str,
) -> TestResult {
    let pool = child_pool(database_url).await?;
    let result = async {
        seed_named_secret(&pool, secret_id, "pwa10-migration-urn-simple", false).await?;
        seed_legacy_query_source(&pool, source_id, "pwa10-migration-urn-simple", reference).await
    }
    .await;
    pool.close().await;
    result
}

async fn assert_postgres_only_query_credential_rejected(
    database_url: &str,
    complete: &Path,
) -> TestResult {
    // PostgreSQL's uuid input accepts hyphens between arbitrary groups of four
    // hex digits. The old Rust parser rejected this spelling, so accepting it
    // during backfill would silently bind an old-invalid credential reference.
    // Prove both sides of that historical mismatch on the disposable database.
    let postgres_only_secret_id = Uuid::now_v7();
    let postgres_only_source_id = Uuid::now_v7();
    let postgres_only_reference = postgres_only_uuid_spelling(postgres_only_secret_id);
    ensure(
        Uuid::parse_str(&postgres_only_reference).is_err(),
        "the PostgreSQL-only UUID regression fixture must be invalid to the legacy Rust parser",
    )?;
    seed_postgres_only_query_source(
        database_url,
        postgres_only_secret_id,
        postgres_only_source_id,
        &postgres_only_reference,
    )
    .await?;
    assert_migration_aborts_with_sqlstate(
        database_url,
        complete,
        "22023",
        "a PostgreSQL-only legacy UUID spelling",
        "PWA-10",
    )
    .await?;
    assert_failed_migration_is_atomic(database_url, 0).await?;
    delete_legacy_query_source(database_url, postgres_only_source_id).await
}

async fn seed_postgres_only_query_source(
    database_url: &str,
    secret_id: Uuid,
    source_id: Uuid,
    reference: &str,
) -> TestResult {
    let pool = child_pool(database_url).await?;
    let result = async {
        seed_named_secret(
            &pool,
            secret_id,
            "pwa10-migration-postgres-only-uuid-spelling",
            false,
        )
        .await?;
        let postgres_parsed_id: Uuid = sqlx::query_scalar("SELECT $1::uuid")
            .bind(reference)
            .fetch_one(&pool)
            .await?;
        ensure(
            postgres_parsed_id == secret_id,
            "the PostgreSQL-only UUID regression fixture must parse to its named secret id",
        )?;
        seed_legacy_query_source(
            &pool,
            source_id,
            "pwa10-migration-postgres-only-uuid-spelling",
            reference,
        )
        .await
    }
    .await;
    pool.close().await;
    result
}

async fn assert_missing_query_secret_rejected(database_url: &str, complete: &Path) -> TestResult {
    // UUID syntax alone is insufficient: the target must be an existing named
    // secret, not merely a UUID that could later become a dangling reference.
    let missing_source_id = Uuid::now_v7();
    let missing_reference = Uuid::now_v7().to_string();
    let pool = child_pool(database_url).await?;
    let missing_seed = seed_legacy_query_source(
        &pool,
        missing_source_id,
        "pwa10-migration-missing",
        &missing_reference,
    )
    .await;
    pool.close().await;
    missing_seed?;
    assert_migration_aborts_with_sqlstate(
        database_url,
        complete,
        "23503",
        "a missing legacy named secret",
        "PWA-10",
    )
    .await?;
    assert_failed_migration_is_atomic(database_url, 0).await?;
    delete_legacy_query_source(database_url, missing_source_id).await
}

async fn assert_field_query_secret_rejected(database_url: &str, complete: &Path) -> TestResult {
    // An existing field secret must not be silently promoted just because its
    // UUID parses: query-source credentials are named-secret references only.
    let field_secret_id = Uuid::now_v7();
    let field_source_id = Uuid::now_v7();
    let field_reference = field_secret_id.to_string();
    let pool = child_pool(database_url).await?;
    let field_seed = async {
        seed_secret(
            &pool,
            field_secret_id,
            "pwa10-migration-field-secret",
            "field",
            false,
        )
        .await?;
        seed_legacy_query_source(
            &pool,
            field_source_id,
            "pwa10-migration-field-secret",
            &field_reference,
        )
        .await
    }
    .await;
    pool.close().await;
    field_seed?;
    assert_migration_aborts_with_sqlstate(
        database_url,
        complete,
        "23503",
        "a non-named legacy secret",
        "PWA-10",
    )
    .await?;
    assert_failed_migration_is_atomic(database_url, 0).await?;
    delete_legacy_query_source(database_url, field_source_id).await
}

async fn assert_protected_query_secret_rejected(database_url: &str, complete: &Path) -> TestResult {
    // Protected named secrets are intentionally not exportable to a monitoring
    // source, so they are also an explicit migration-abort condition.
    let protected_secret_id = Uuid::now_v7();
    let protected_source_id = Uuid::now_v7();
    let protected_reference = protected_secret_id.to_string();
    let pool = child_pool(database_url).await?;
    let protected_seed = async {
        seed_named_secret(
            &pool,
            protected_secret_id,
            "pwa10-migration-protected",
            true,
        )
        .await?;
        seed_legacy_query_source(
            &pool,
            protected_source_id,
            "pwa10-migration-protected",
            &protected_reference,
        )
        .await
    }
    .await;
    pool.close().await;
    protected_seed?;
    assert_migration_aborts_with_sqlstate(
        database_url,
        complete,
        "23503",
        "a protected legacy named secret",
        "PWA-10",
    )
    .await?;
    assert_failed_migration_is_atomic(database_url, 0).await?;
    delete_legacy_query_source(database_url, protected_source_id).await
}

async fn assert_orphan_query_binding_rejected(database_url: &str, complete: &Path) -> TestResult {
    // The migration's global validation must also cover an interrupted rollout
    // binding that has no raw legacy source row to visit. This orphan is the
    // canonical shape but references no query-source id, so it exercises the
    // post-backfill `source.id IS NULL` guard and must roll back all work.
    let orphan_secret_id = Uuid::now_v7();
    let orphan_source_id = Uuid::now_v7();
    let pool = child_pool(database_url).await?;
    let orphan_seed = async {
        seed_named_secret(
            &pool,
            orphan_secret_id,
            "pwa10-migration-orphan-binding",
            false,
        )
        .await?;
        seed_query_source_binding(
            &pool,
            orphan_source_id,
            orphan_secret_id,
            QUERY_SOURCE_CREDENTIAL_OWNER_SCOPE_TYPE,
            orphan_source_id,
        )
        .await
    }
    .await;
    pool.close().await;
    orphan_seed?;
    assert_migration_aborts_with_sqlstate(
        database_url,
        complete,
        "23514",
        "an orphaned pre-existing query-source binding",
        "PWA-10",
    )
    .await?;
    assert_failed_migration_is_atomic(database_url, 1).await?;
    delete_query_source_binding(database_url, orphan_source_id).await
}

async fn assert_conflicting_query_binding_rejected(
    database_url: &str,
    complete: &Path,
) -> TestResult {
    // A prior interrupted rollout must never choose silently between different
    // identities. A pre-existing canonical binding which disagrees with raw
    // legacy text aborts, retaining only its original row after rollback.
    let conflicting_raw_secret_id = Uuid::now_v7();
    let conflicting_bound_secret_id = Uuid::now_v7();
    let conflicting_source_id = Uuid::now_v7();
    let conflicting_raw_reference = conflicting_raw_secret_id.to_string();
    seed_conflicting_query_binding(
        database_url,
        conflicting_raw_secret_id,
        conflicting_bound_secret_id,
        conflicting_source_id,
        &conflicting_raw_reference,
    )
    .await?;
    assert_migration_aborts_with_sqlstate(
        database_url,
        complete,
        "23514",
        "a disagreeing interrupted-rollout credential binding",
        "PWA-10",
    )
    .await?;
    assert_failed_migration_is_atomic(database_url, 1).await?;
    delete_query_source_binding(database_url, conflicting_source_id).await?;
    delete_legacy_query_source(database_url, conflicting_source_id).await
}

async fn seed_conflicting_query_binding(
    database_url: &str,
    raw_secret_id: Uuid,
    bound_secret_id: Uuid,
    source_id: Uuid,
    raw_reference: &str,
) -> TestResult {
    let pool = child_pool(database_url).await?;
    let result = async {
        seed_named_secret(
            &pool,
            raw_secret_id,
            "pwa10-migration-conflicting-raw",
            false,
        )
        .await?;
        seed_named_secret(
            &pool,
            bound_secret_id,
            "pwa10-migration-conflicting-bound",
            false,
        )
        .await?;
        seed_legacy_query_source(
            &pool,
            source_id,
            "pwa10-migration-conflicting",
            raw_reference,
        )
        .await?;
        seed_query_source_binding(
            &pool,
            source_id,
            bound_secret_id,
            QUERY_SOURCE_CREDENTIAL_OWNER_SCOPE_TYPE,
            source_id,
        )
        .await
    }
    .await;
    pool.close().await;
    result
}

async fn seed_stale_owner_binding(database_url: &str, fixtures: &MigrationFixtures) -> TestResult {
    // An equal binding from an interrupted rollout is safe to retain, but its
    // stale owner shape must be normalized by the one canonical migration. Seed
    // it before any apply that can commit PWA-10: the first alert-source abort
    // below is that apply, and this binding must survive its committed backfill.
    let MigrationFixtures {
        bare_source_id,
        bare_secret_id,
        ..
    } = fixtures.clone();
    let stale_owner_scope_id = Uuid::now_v7();
    let pool = child_pool(database_url).await?;
    let equal_binding_seed = seed_query_source_binding(
        &pool,
        bare_source_id,
        bare_secret_id,
        "stale_monitoring_query_source_scope",
        stale_owner_scope_id,
    )
    .await;
    pool.close().await;
    equal_binding_seed
}

async fn assert_alert_url_rejections(
    database_url: &str,
    complete: &Path,
    fixtures: &MigrationFixtures,
) -> TestResult {
    // Alert-source abort cases. SQLx applies 0002 and 0003 in separate
    // transactions, so the first apply here commits PWA-10 (dropping the query
    // column and stamping version 2) and then rolls back only the aborted
    // PWA-11 migration. The valid query rows and the retained equal binding are
    // already seeded, so every alert case below observes that committed split
    // state instead of replaying the query-source migration.
    for (label, alert_url) in [
        (
            "userinfo",
            "https://operator@migration.example.test/api/v1/alerts",
        ),
        (
            "userinfo after extra slash",
            "https:///operator:placeholder@migration.example.test/api/v1/alerts",
        ),
        (
            "userinfo without slash",
            "https:operator:placeholder@migration.example.test/api/v1/alerts",
        ),
        (
            "userinfo after backslash",
            r"https:\\\\operator:placeholder@migration.example.test/api/v1/alerts",
        ),
        (
            "query component",
            "https://migration.example.test/api/v1/alerts?scope=platform",
        ),
        (
            "fragment",
            "https://migration.example.test/api/v1/alerts#diagnostic",
        ),
    ] {
        assert_alert_url_rejected(database_url, complete, label, alert_url, fixtures).await?;
    }
    Ok(())
}

async fn assert_alert_url_rejected(
    database_url: &str,
    complete: &Path,
    label: &str,
    alert_url: &str,
    fixtures: &MigrationFixtures,
) -> TestResult {
    let MigrationFixtures {
        alert_reference, ..
    } = fixtures.clone();
    let source_id = Uuid::now_v7();
    let pool = child_pool(database_url).await?;
    let seed =
        seed_legacy_alert_source_with_url(&pool, source_id, alert_url, &alert_reference).await;
    pool.close().await;
    seed?;
    assert_migration_aborts_with_sqlstate(
        database_url,
        complete,
        "22023",
        &format!("a legacy alert-source URL with an inline credential channel ({label})"),
        "PWA-11",
    )
    .await?;
    assert_failed_alert_migration_is_atomic(database_url, 0).await?;
    delete_legacy_alert_source(database_url, source_id).await
}

async fn assert_malformed_alert_credential_rejected(
    database_url: &str,
    complete: &Path,
) -> TestResult {
    // Invalid alert credential text must abort 0003 before it backfills valid
    // rows. No raw input value is reported by either migration or test.
    let alert_malformed_source_id = Uuid::now_v7();
    let pool = child_pool(database_url).await?;
    let alert_malformed_seed =
        seed_legacy_alert_source(&pool, alert_malformed_source_id, "secret://not-a-uuid").await;
    pool.close().await;
    alert_malformed_seed?;
    assert_migration_aborts_with_sqlstate(
        database_url,
        complete,
        "22023",
        "a malformed legacy alert credential",
        "PWA-11",
    )
    .await?;
    assert_failed_alert_migration_is_atomic(database_url, 0).await?;
    delete_legacy_alert_source(database_url, alert_malformed_source_id).await
}

async fn assert_urn_simple_alert_credential_rejected(
    database_url: &str,
    complete: &Path,
) -> TestResult {
    // The legacy alert parser shares the query parser's URN distinction: a
    // simple UUID stays invalid after the exact `urn:uuid:` prefix even though
    // PostgreSQL's cast would accept a hyphenated form.
    let alert_urn_simple_secret_id = Uuid::now_v7();
    let alert_urn_simple_source_id = Uuid::now_v7();
    let alert_urn_simple_reference = format!("urn:uuid:{}", alert_urn_simple_secret_id.simple());
    let pool = child_pool(database_url).await?;
    let alert_urn_simple_seed = async {
        seed_named_secret(
            &pool,
            alert_urn_simple_secret_id,
            "pwa11-migration-urn-simple",
            false,
        )
        .await?;
        seed_legacy_alert_source(
            &pool,
            alert_urn_simple_source_id,
            &alert_urn_simple_reference,
        )
        .await
    }
    .await;
    pool.close().await;
    alert_urn_simple_seed?;
    assert_migration_aborts_with_sqlstate(
        database_url,
        complete,
        "22023",
        "a simple UUID after the legacy alert URN prefix",
        "PWA-11",
    )
    .await?;
    assert_failed_alert_migration_is_atomic(database_url, 0).await?;
    delete_legacy_alert_source(database_url, alert_urn_simple_source_id).await
}

async fn assert_postgres_only_alert_credential_rejected(
    database_url: &str,
    complete: &Path,
) -> TestResult {
    // PostgreSQL's uuid input accepts hyphen placements the old Rust parser
    // rejected, so a PostgreSQL-only spelling must abort the alert backfill.
    // Prove both sides of that historical mismatch on the disposable database.
    let alert_postgres_only_secret_id = Uuid::now_v7();
    let alert_postgres_only_source_id = Uuid::now_v7();
    let alert_postgres_only_reference = postgres_only_uuid_spelling(alert_postgres_only_secret_id);
    ensure(
        Uuid::parse_str(&alert_postgres_only_reference).is_err(),
        "the PostgreSQL-only alert UUID fixture must be invalid to the legacy Rust parser",
    )?;
    seed_postgres_only_alert_source(
        database_url,
        alert_postgres_only_secret_id,
        alert_postgres_only_source_id,
        &alert_postgres_only_reference,
    )
    .await?;
    assert_migration_aborts_with_sqlstate(
        database_url,
        complete,
        "22023",
        "a PostgreSQL-only legacy alert UUID spelling",
        "PWA-11",
    )
    .await?;
    assert_failed_alert_migration_is_atomic(database_url, 0).await?;
    delete_legacy_alert_source(database_url, alert_postgres_only_source_id).await
}

async fn seed_postgres_only_alert_source(
    database_url: &str,
    secret_id: Uuid,
    source_id: Uuid,
    reference: &str,
) -> TestResult {
    let pool = child_pool(database_url).await?;
    let result = async {
        seed_named_secret(
            &pool,
            secret_id,
            "pwa11-migration-postgres-only-uuid-spelling",
            false,
        )
        .await?;
        let postgres_parsed_id: Uuid = sqlx::query_scalar("SELECT $1::uuid")
            .bind(reference)
            .fetch_one(&pool)
            .await?;
        ensure(
            postgres_parsed_id == secret_id,
            "the PostgreSQL-only alert UUID fixture must parse to its named secret id",
        )?;
        seed_legacy_alert_source(&pool, source_id, reference).await
    }
    .await;
    pool.close().await;
    result
}

async fn assert_missing_alert_secret_rejected(database_url: &str, complete: &Path) -> TestResult {
    // A missing alert secret must abort rather than leave a dangling binding.
    let alert_missing_source_id = Uuid::now_v7();
    let alert_missing_reference = Uuid::now_v7().to_string();
    let pool = child_pool(database_url).await?;
    let alert_missing_seed =
        seed_legacy_alert_source(&pool, alert_missing_source_id, &alert_missing_reference).await;
    pool.close().await;
    alert_missing_seed?;
    assert_migration_aborts_with_sqlstate(
        database_url,
        complete,
        "23503",
        "a missing legacy alert named secret",
        "PWA-11",
    )
    .await?;
    assert_failed_alert_migration_is_atomic(database_url, 0).await?;
    delete_legacy_alert_source(database_url, alert_missing_source_id).await
}

async fn assert_field_alert_secret_rejected(database_url: &str, complete: &Path) -> TestResult {
    // A field secret must not be promoted to an alert credential just because
    // its UUID parses.
    let alert_field_secret_id = Uuid::now_v7();
    let alert_field_source_id = Uuid::now_v7();
    let alert_field_reference = alert_field_secret_id.to_string();
    let pool = child_pool(database_url).await?;
    let alert_field_seed = async {
        seed_secret(
            &pool,
            alert_field_secret_id,
            "pwa11-migration-field-secret",
            "field",
            false,
        )
        .await?;
        seed_legacy_alert_source(&pool, alert_field_source_id, &alert_field_reference).await
    }
    .await;
    pool.close().await;
    alert_field_seed?;
    assert_migration_aborts_with_sqlstate(
        database_url,
        complete,
        "23503",
        "a non-named legacy alert secret",
        "PWA-11",
    )
    .await?;
    assert_failed_alert_migration_is_atomic(database_url, 0).await?;
    delete_legacy_alert_source(database_url, alert_field_source_id).await
}

async fn assert_protected_alert_secret_rejected(database_url: &str, complete: &Path) -> TestResult {
    // A protected named alert secret is not exportable to a monitoring source.
    let alert_protected_secret_id = Uuid::now_v7();
    let alert_protected_source_id = Uuid::now_v7();
    let alert_protected_reference = alert_protected_secret_id.to_string();
    let pool = child_pool(database_url).await?;
    let alert_protected_seed = async {
        seed_named_secret(
            &pool,
            alert_protected_secret_id,
            "pwa11-migration-protected",
            true,
        )
        .await?;
        seed_legacy_alert_source(&pool, alert_protected_source_id, &alert_protected_reference).await
    }
    .await;
    pool.close().await;
    alert_protected_seed?;
    assert_migration_aborts_with_sqlstate(
        database_url,
        complete,
        "23503",
        "a protected legacy alert named secret",
        "PWA-11",
    )
    .await?;
    assert_failed_alert_migration_is_atomic(database_url, 0).await?;
    delete_legacy_alert_source(database_url, alert_protected_source_id).await
}

async fn assert_orphan_alert_binding_rejected(database_url: &str, complete: &Path) -> TestResult {
    // An interrupted rollout can leave an alert binding whose consumer id has
    // no source row. The global validation must catch it and roll back 0003,
    // retaining the sole pre-existing alert binding.
    let orphan_alert_secret_id = Uuid::now_v7();
    let orphan_alert_source_id = Uuid::now_v7();
    let pool = child_pool(database_url).await?;
    let orphan_alert_seed = async {
        seed_named_secret(
            &pool,
            orphan_alert_secret_id,
            "pwa11-migration-orphan-binding",
            false,
        )
        .await?;
        seed_alert_source_binding(
            &pool,
            orphan_alert_source_id,
            orphan_alert_secret_id,
            ALERT_SOURCE_CREDENTIAL_OWNER_SCOPE_TYPE,
            orphan_alert_source_id,
        )
        .await
    }
    .await;
    pool.close().await;
    orphan_alert_seed?;
    assert_migration_aborts_with_sqlstate(
        database_url,
        complete,
        "23514",
        "an orphaned pre-existing alert-source binding",
        "PWA-11",
    )
    .await?;
    assert_failed_alert_migration_is_atomic(database_url, 1).await?;
    delete_alert_source_binding(database_url, orphan_alert_source_id).await
}

async fn assert_conflicting_alert_binding_rejected(
    database_url: &str,
    complete: &Path,
) -> TestResult {
    // A pre-existing alert binding that disagrees with the raw legacy text
    // aborts instead of silently choosing one identity, retaining its row.
    let alert_conflicting_raw_secret_id = Uuid::now_v7();
    let alert_conflicting_bound_secret_id = Uuid::now_v7();
    let alert_conflicting_source_id = Uuid::now_v7();
    let alert_conflicting_raw_reference = alert_conflicting_raw_secret_id.to_string();
    seed_conflicting_alert_binding(
        database_url,
        alert_conflicting_raw_secret_id,
        alert_conflicting_bound_secret_id,
        alert_conflicting_source_id,
        &alert_conflicting_raw_reference,
    )
    .await?;
    assert_migration_aborts_with_sqlstate(
        database_url,
        complete,
        "23514",
        "a disagreeing interrupted-rollout alert credential binding",
        "PWA-11",
    )
    .await?;
    assert_failed_alert_migration_is_atomic(database_url, 1).await?;
    delete_alert_source_binding(database_url, alert_conflicting_source_id).await?;
    delete_legacy_alert_source(database_url, alert_conflicting_source_id).await
}

async fn seed_conflicting_alert_binding(
    database_url: &str,
    raw_secret_id: Uuid,
    bound_secret_id: Uuid,
    source_id: Uuid,
    raw_reference: &str,
) -> TestResult {
    let pool = child_pool(database_url).await?;
    let result = async {
        seed_named_secret(
            &pool,
            raw_secret_id,
            "pwa11-migration-conflicting-raw",
            false,
        )
        .await?;
        seed_named_secret(
            &pool,
            bound_secret_id,
            "pwa11-migration-conflicting-bound",
            false,
        )
        .await?;
        seed_legacy_alert_source(&pool, source_id, raw_reference).await?;
        seed_alert_source_binding(
            &pool,
            source_id,
            bound_secret_id,
            ALERT_SOURCE_CREDENTIAL_OWNER_SCOPE_TYPE,
            source_id,
        )
        .await
    }
    .await;
    pool.close().await;
    result
}

async fn run_rebaseline_preservation(
    database_url: &str,
    complete: &Path,
    fixtures: &MigrationFixtures,
) -> TestResult {
    // A later baseline reconciliation must keep the real, checksum-matching
    // PWA-10 record. Otherwise SQLx would attempt to run the destructive
    // migration again against a schema where its raw column is already gone.
    apply_rebaseline(database_url).await?;
    assert_rebaseline_preserves_current_forward_migration(database_url, None).await?;
    apply_migrations(database_url, complete).await?;
    assert_successful_backfill(
        database_url,
        &fixtures.query_bindings,
        fixtures.alert_binding,
    )
    .await
}

async fn run_renamed_history_preservation(
    database_url: &str,
    complete: &Path,
    fixtures: &MigrationFixtures,
) -> TestResult {
    // SQLx validates current migration content by version and checksum, not the
    // descriptive text. A renamed but checksum-matching PWA-10 row must remain
    // applied; otherwise the re-baseline would make SQLx replay a destructive
    // migration against an already-upgraded schema.
    let renamed_history = rename_current_forward_migration_description(
        database_url,
        RENAMED_CURRENT_VERSION_TWO_DESCRIPTION,
    )
    .await?;
    apply_rebaseline(database_url).await?;
    assert_rebaseline_preserves_current_forward_migration(database_url, Some(&renamed_history))
        .await?;
    apply_migrations(database_url, complete).await?;
    assert_successful_backfill(
        database_url,
        &fixtures.query_bindings,
        fixtures.alert_binding,
    )
    .await
}

async fn run_unknown_future_history_rejection(database_url: &str) -> TestResult {
    // A database can be ahead of the binary doing the re-baseline. Treat its
    // applied newer version as unrecognized forward history and abort before
    // deleting anything; otherwise a subsequent newer deployment can replay a
    // destructive migration whose SQLx history was silently erased.
    let future_history = seed_unknown_future_forward_migration_history(database_url).await?;
    assert_rebaseline_rejects_unidentified_forward_history(database_url, &future_history).await?;
    delete_unknown_future_forward_migration_history(database_url).await
}

async fn run_corrupted_checksum_rejection(database_url: &str) -> TestResult {
    // A reused forward version with an unrecognized description and checksum
    // is not proven legacy history. Deleting it would hide a collision and
    // could make SQLx replay this destructive migration, so re-baselining must
    // fail before any persisted mutation.
    let mismatched_history = corrupt_current_forward_migration_checksum(database_url).await?;
    assert_rebaseline_rejects_unidentified_forward_history(database_url, &mismatched_history).await
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL with PIPING_DATABASE__URL and CREATEDB"]
async fn query_source_credential_migration_aborts_then_backfills_transactionally() -> TestResult {
    let database_url = disposable_database_url()?;
    create_disposable_database(&database_url).await?;

    let exercise_result = exercise_migration(&database_url).await;
    let cleanup_result = force_drop_disposable_database(&database_url).await;
    match (exercise_result, cleanup_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(exercise_error), Ok(())) => Err(exercise_error),
        (Ok(()), Err(cleanup_error)) => Err(cleanup_error),
        (Err(_exercise_error), Err(_cleanup_error)) => Err(
            "PWA-10 migration test failed and its disposable database cleanup also failed".into(),
        ),
    }
}
