//! Field-lifecycle provenance migration proof on uniquely named disposable databases.
//! Requires an explicit isolated PostgreSQL URL with CREATEDB permission.

use std::borrow::Cow;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;

use futures_util::FutureExt;
use piping_backend::contexts::secrets::domain::{SecretBinding, SecretVariable};
use piping_backend::contexts::secrets::infrastructure::{SecretRepository, SqlxSecretRepository};
use serde_json::Value;
use sqlx::migrate::{MigrateDatabase, Migrator};
use sqlx::{PgPool, Postgres};
use url::Url;
use uuid::Uuid;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

async fn child_database() -> TestResult<(String, PgPool)> {
    let mut url = Url::parse(&std::env::var("PIPING_DATABASE__URL")?)?;
    url.set_path(&format!("/pwa118_lifecycle_{}", Uuid::now_v7().simple()));
    let url = url.to_string();
    Postgres::create_database(&url).await?;
    match PgPool::connect(&url).await {
        Ok(pool) => Ok((url, pool)),
        Err(error) => {
            Postgres::force_drop_database(&url).await?;
            Err(error.into())
        }
    }
}

async fn migrator() -> TestResult<Migrator> {
    Ok(Migrator::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations")).await?)
}

// Runtime SQL is deliberate here: the staged database does not yet have the
// current schema, and its old-column writers are part of the migration proof.
async fn secret_snapshot(pool: &PgPool) -> TestResult<Vec<Value>> {
    let mut snapshot = Vec::new();
    for (table, order) in [
        ("secrets_variable", "id"),
        ("secrets_binding", "id"),
        ("secrets_store", "secret_id"),
        ("secret_usage_binding", "id"),
    ] {
        let rows: Vec<Value> = sqlx::query_scalar(&format!(
            "SELECT to_jsonb(t) - 'retire_with_field_usage' - 'managed_by_field_usage' FROM {table} t ORDER BY {order}"
        )).fetch_all(pool).await?;
        snapshot.push(serde_json::to_value(rows)?);
    }
    Ok(snapshot)
}

async fn history(pool: &PgPool) -> TestResult<Vec<Value>> {
    Ok(
        sqlx::query_scalar("SELECT to_jsonb(m) FROM _sqlx_migrations m ORDER BY version")
            .fetch_all(pool)
            .await?,
    )
}

async fn schema(pool: &PgPool) -> TestResult<Vec<Value>> {
    let mut columns: Vec<Value> = sqlx::query_scalar(
        // Database identity differs intentionally between the two child databases.
        "SELECT to_jsonb(c) - 'table_catalog' - 'udt_catalog' FROM information_schema.columns c WHERE table_schema = 'public' AND table_name IN ('secrets_variable', 'secrets_binding') ORDER BY table_name, ordinal_position"
    ).fetch_all(pool).await?;
    let constraints: Vec<Value> = sqlx::query_scalar(
        "SELECT jsonb_build_object('table', r.relname, 'constraint', c.conname, 'definition', pg_get_constraintdef(c.oid)) FROM pg_constraint c JOIN pg_class r ON r.oid = c.conrelid JOIN pg_namespace n ON n.oid = r.relnamespace WHERE n.nspname = 'public' AND r.relname IN ('secrets_variable', 'secrets_binding') ORDER BY r.relname, c.conname"
    ).fetch_all(pool).await?;
    assert!(
        constraints
            .iter()
            .any(|row| row["constraint"] == "chk_secrets_variable_field_retirement")
    );
    columns.extend(constraints);
    Ok(columns)
}

async fn seed_old_writer(pool: &PgPool) -> TestResult<Uuid> {
    let secret_id = Uuid::now_v7();
    sqlx::query("INSERT INTO secrets_variable (id, key) VALUES ($1, $2)")
        .bind(secret_id)
        .bind(format!("secret-field/historical/{secret_id}"))
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO secrets_store (secret_id, encrypted_value) VALUES ($1, $2)")
        .bind(secret_id)
        .bind("synthetic-opaque-ciphertext-preserved-verbatim")
        .execute(pool)
        .await?;
    let owner_id = Uuid::now_v7();
    for weight in [0, 0, 19] {
        sqlx::query("INSERT INTO secrets_binding (secret_id, scope_type, scope_id, weight) VALUES ($1, 'user', $2, $3)")
            .bind(secret_id).bind(owner_id).bind(weight).execute(pool).await?;
    }
    sqlx::query("INSERT INTO secret_usage_binding (consumer_type, consumer_id, field_key, owner_scope_type, owner_scope_id, secret_id) VALUES ('user_auth_password', $1, 'local_password', 'user', $1, $2)")
        .bind(owner_id).bind(secret_id).execute(pool).await?;
    Ok(secret_id)
}

#[allow(
    clippy::expect_used,
    reason = "a rejected constraint write is the asserted outcome of this migration-contract helper, so `expect_err` must panic naming the missing CHECK"
)]
async fn prove_defaults_and_constraint_authority(pool: &PgPool) -> TestResult<Uuid> {
    let secret_id = seed_old_writer(pool).await?;
    let variable_managed: bool =
        sqlx::query_scalar("SELECT retire_with_field_usage FROM secrets_variable WHERE id = $1")
            .bind(secret_id)
            .fetch_one(pool)
            .await?;
    assert!(!variable_managed);
    let binding_managed: Vec<bool> = sqlx::query_scalar(
        "SELECT managed_by_field_usage FROM secrets_binding WHERE secret_id = $1",
    )
    .bind(secret_id)
    .fetch_all(pool)
    .await?;
    assert_eq!(binding_managed, vec![false; 3]);

    for (key, kind) in [
        ("not-machine", "field"),
        ("named-secret", "named"),
        ("secret-field/not-named", "named"),
    ] {
        let result = sqlx::query("INSERT INTO secrets_variable (key, kind, retire_with_field_usage) VALUES ($1, $2, true)")
            .bind(key).bind(kind).execute(pool).await;
        assert_eq!(
            result
                .expect_err("invalid retirement authority")
                .as_database_error()
                .and_then(|error| error.code())
                .as_deref(),
            Some("23514")
        );
    }
    for (table, column) in [
        ("secrets_variable", "retire_with_field_usage"),
        ("secrets_binding", "managed_by_field_usage"),
    ] {
        let result = sqlx::query(&format!("UPDATE {table} SET {column} = NULL"))
            .execute(pool)
            .await;
        assert_eq!(
            result
                .expect_err("null authority")
                .as_database_error()
                .and_then(|error| error.code())
                .as_deref(),
            Some("23502")
        );
    }
    Ok(secret_id)
}

async fn prove_ordinary_writers_cannot_grant_field_authority(
    repository: &SqlxSecretRepository,
) -> TestResult<(SecretVariable, SecretBinding)> {
    let variable = SecretVariable {
        id: Uuid::now_v7(),
        key: "ordinary-secret-variable".into(),
        is_masked: true,
        is_protected: false,
        kind: "named".into(),
        description: Some("synthetic explicit description".into()),
        retire_with_field_usage: true,
    };
    let saved = repository.create_variable(variable).await?;
    assert_eq!(saved.kind, "named");
    assert_eq!(
        saved.description.as_deref(),
        Some("synthetic explicit description")
    );
    assert!(
        !saved.retire_with_field_usage,
        "ordinary writer cannot grant retirement authority"
    );
    let owner_id = Uuid::now_v7();
    let binding = repository
        .create_binding(SecretBinding {
            id: Uuid::now_v7(),
            secret_id: saved.id,
            scope_type: "user".into(),
            scope_id: owner_id,
            weight: 0,
            managed_by_field_usage: true,
        })
        .await?;
    assert!(!binding.managed_by_field_usage);
    Ok((saved, binding))
}

#[allow(
    clippy::expect_used,
    reason = "repository reads by ids created by the fixture helper that calls this one; a missing row is a broken fixture, so the calling test must abort"
)]
async fn prove_variable_read_paths_hide_retirement_authority(
    repository: &SqlxSecretRepository,
    saved: &SecretVariable,
    scope_id: Uuid,
) -> TestResult {
    assert!(
        !repository
            .get_variable_by_id(saved.id)
            .await?
            .expect("variable")
            .retire_with_field_usage
    );
    assert!(
        !repository
            .get_variable_by_key(&saved.key)
            .await?
            .expect("variable")
            .retire_with_field_usage
    );
    assert!(!repository.list_variables(Some(&saved.key)).await?[0].retire_with_field_usage);
    let scoped = repository.list_scope_secrets("user", scope_id).await?;
    assert!(
        scoped
            .variables
            .iter()
            .any(|row| row.id == saved.id && !row.retire_with_field_usage)
    );
    let value = serde_json::to_value(saved)?;
    assert!(value.get("retire_with_field_usage").is_none());
    assert!(value.get("managed_by_field_usage").is_none());
    let mut variable_payload = serde_json::to_value(saved)?;
    variable_payload["retire_with_field_usage"] = Value::Bool(true);
    let decoded: SecretVariable = serde_json::from_value(variable_payload)?;
    assert!(!decoded.retire_with_field_usage);
    Ok(())
}

async fn prove_binding_read_paths_hide_managed_by_authority(
    repository: &SqlxSecretRepository,
    binding: &SecretBinding,
) -> TestResult {
    assert!(
        !repository
            .list_bindings_for_secret(binding.secret_id)
            .await?[0]
            .managed_by_field_usage
    );
    assert!(
        !repository
            .list_bindings_for_scope("user", binding.scope_id)
            .await?[0]
            .managed_by_field_usage
    );
    let scoped = repository
        .list_scope_secrets("user", binding.scope_id)
        .await?;
    assert!(
        scoped
            .bindings
            .iter()
            .any(|row| row.id == binding.id && !row.managed_by_field_usage)
    );
    assert!(
        repository
            .list_bindings_for_scope_hierarchy("user", binding.scope_id)
            .await?
            .iter()
            .any(|row| row.id == binding.id)
    );
    let value = serde_json::to_value(binding)?;
    assert!(value.get("retire_with_field_usage").is_none());
    assert!(value.get("managed_by_field_usage").is_none());
    let mut binding_payload = serde_json::to_value(binding)?;
    binding_payload["managed_by_field_usage"] = Value::Bool(true);
    let decoded: SecretBinding = serde_json::from_value(binding_payload)?;
    assert!(!decoded.managed_by_field_usage);
    Ok(())
}

#[allow(
    clippy::expect_used,
    reason = "the authority rows are seeded by this helper itself; a missing read is a broken fixture, so the calling test must abort"
)]
async fn prove_seeded_variable_authority_is_read_back(
    pool: &PgPool,
    repository: &SqlxSecretRepository,
    secret_id: Uuid,
) -> TestResult {
    // Seed positive authority only in this isolated schema-contract fixture;
    // ordinary repository writers above are explicitly denied that authority.
    sqlx::query("UPDATE secrets_variable SET retire_with_field_usage = true WHERE id = $1")
        .bind(secret_id)
        .execute(pool)
        .await?;
    assert!(
        repository
            .get_variable_by_id(secret_id)
            .await?
            .expect("managed variable")
            .retire_with_field_usage
    );
    assert!(
        repository
            .list_variables(None)
            .await?
            .iter()
            .any(|row| row.id == secret_id && row.retire_with_field_usage)
    );
    Ok(())
}

async fn prove_seeded_binding_authority_is_read_back(
    pool: &PgPool,
    repository: &SqlxSecretRepository,
    secret_id: Uuid,
) -> TestResult {
    let managed_scope_id = Uuid::now_v7();
    let managed_binding_id = Uuid::now_v7();
    sqlx::query("INSERT INTO secrets_binding (id, secret_id, scope_type, scope_id, managed_by_field_usage) VALUES ($1, $2, 'user', $3, true)")
        .bind(managed_binding_id).bind(secret_id).bind(managed_scope_id).execute(pool).await?;
    // The managed scope read further down is reachable only through the binding
    // seeded here, so it must report both the binding and the variable.
    assert!(
        repository
            .list_bindings_for_secret(secret_id)
            .await?
            .iter()
            .any(|row| row.id == managed_binding_id && row.managed_by_field_usage)
    );
    assert!(
        repository
            .list_bindings_for_scope("user", managed_scope_id)
            .await?[0]
            .managed_by_field_usage
    );
    assert!(
        repository
            .list_bindings_for_scope_hierarchy("user", managed_scope_id)
            .await?
            .iter()
            .any(|row| row.id == managed_binding_id && row.managed_by_field_usage)
    );
    let scoped = repository
        .list_scope_secrets("user", managed_scope_id)
        .await?;
    assert!(
        scoped
            .variables
            .iter()
            .any(|row| row.id == secret_id && row.retire_with_field_usage)
    );
    assert!(
        scoped
            .bindings
            .iter()
            .any(|row| row.id == managed_binding_id && row.managed_by_field_usage)
    );
    Ok(())
}

async fn prove_schema_and_defaults(pool: &PgPool) -> TestResult {
    let secret_id = prove_defaults_and_constraint_authority(pool).await?;
    let repository = SqlxSecretRepository::new(pool.clone());
    let (saved, binding) = prove_ordinary_writers_cannot_grant_field_authority(&repository).await?;
    prove_variable_read_paths_hide_retirement_authority(&repository, &saved, binding.scope_id)
        .await?;
    prove_binding_read_paths_hide_managed_by_authority(&repository, &binding).await?;
    prove_seeded_variable_authority_is_read_back(pool, &repository, secret_id).await?;
    prove_seeded_binding_authority_is_read_back(pool, &repository, secret_id).await
}

#[tokio::test]
#[ignore] // Creates and drops only uniquely named child databases on the explicit isolated server.
async fn fresh_and_staged_provenance_migration_preserve_historical_authority() -> TestResult {
    let all = migrator().await?;
    let (fresh_url, fresh) = child_database().await?;
    let (staged_url, staged) = match child_database().await {
        Ok(child) => child,
        Err(error) => {
            fresh.close().await;
            Postgres::force_drop_database(&fresh_url).await?;
            return Err(error);
        }
    };
    let proof = AssertUnwindSafe(async {
        all.run(&fresh).await?;
        let prefix = Migrator {
            migrations: Cow::Owned(
                all.iter()
                    .filter(|migration| migration.version <= 22)
                    .cloned()
                    .collect(),
            ),
            ..Migrator::DEFAULT
        };
        prefix.run(&staged).await?;
        seed_old_writer(&staged).await?;
        let before = secret_snapshot(&staged).await?;
        let history_before = history(&staged).await?;
        assert_eq!(history_before.len(), 22);
        all.run(&staged).await?;
        assert_eq!(secret_snapshot(&staged).await?, before);
        let history_after = history(&staged).await?;
        assert_eq!(history_after.len(), 23);
        assert_eq!(&history_after[..22], history_before.as_slice());
        assert!(
            schema(&fresh).await? == schema(&staged).await?,
            "fresh and staged column contracts differ"
        );
        all.run(&staged).await?;
        assert_eq!(
            history(&staged).await?,
            history_after,
            "repeat migration is a no-op"
        );
        assert_eq!(secret_snapshot(&staged).await?, before);
        prove_schema_and_defaults(&fresh).await?;
        prove_schema_and_defaults(&staged).await?;
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    })
    .catch_unwind()
    .await;
    fresh.close().await;
    staged.close().await;
    let fresh_cleanup = Postgres::force_drop_database(&fresh_url).await;
    let staged_cleanup = Postgres::force_drop_database(&staged_url).await;
    fresh_cleanup?;
    staged_cleanup?;
    match proof {
        Ok(result) => result,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}
