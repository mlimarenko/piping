//! Integration checks for canonical secret storage and schema contracts.
//!
//! Requires PostgreSQL/Redis stack. Run with:
//! `cargo test --test integration_secrets_canonicalization -- --ignored`

mod common;

use common::build_test_db_pool;
use sqlx::Row;

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_secrets_canonicalization -- --ignored
async fn secrets_schema_has_no_legacy_secret_ref_column() {
    let db = build_test_db_pool().await.expect("database pool");
    let row = sqlx::query(
        r#"
        SELECT COUNT(*)::BIGINT AS count
        FROM information_schema.columns
        WHERE table_name = 'secrets_variable'
          AND column_name = 'secret_ref'
        "#,
    )
    .fetch_one(&db)
    .await
    .expect("query information_schema");

    let count: i64 = row.try_get("count").expect("count");
    assert_eq!(
        count, 0,
        "secrets_variable.secret_ref must not exist in canonical schema"
    );
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_secrets_canonicalization -- --ignored
async fn secrets_store_rows_use_canonical_envelope_prefix() {
    let db = build_test_db_pool().await.expect("database pool");
    let row = sqlx::query(
        r#"
        SELECT COUNT(*)::BIGINT AS count
        FROM secrets_store
        WHERE encrypted_value NOT LIKE 'enc:v1:%'
        "#,
    )
    .fetch_one(&db)
    .await
    .expect("query secrets_store");

    let count: i64 = row.try_get("count").expect("count");
    assert_eq!(count, 0, "all secrets must be stored as enc:v1 envelopes");
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_secrets_canonicalization -- --ignored
async fn secrets_variable_has_named_automation_columns() {
    let db = build_test_db_pool().await.expect("database pool");
    let row = sqlx::query(
        r#"
        SELECT COUNT(*)::BIGINT AS count
        FROM information_schema.columns
        WHERE table_name = 'secrets_variable'
          AND column_name IN ('kind', 'description', 'created_at', 'updated_at', 'created_by')
        "#,
    )
    .fetch_one(&db)
    .await
    .expect("query information_schema");

    let count: i64 = row.try_get("count").expect("count");
    assert_eq!(
        count, 5,
        "the baseline must define kind/description/created_at/updated_at/created_by on secrets_variable"
    );
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_secrets_canonicalization -- --ignored
async fn secrets_variable_kind_is_constrained_and_backfilled() {
    let db = build_test_db_pool().await.expect("database pool");

    let constraint = sqlx::query(
        r#"
        SELECT COUNT(*)::BIGINT AS count
        FROM information_schema.check_constraints
        WHERE constraint_name = 'chk_secrets_variable_kind'
        "#,
    )
    .fetch_one(&db)
    .await
    .expect("query check_constraints");
    let constraint_count: i64 = constraint.try_get("count").expect("count");
    assert_eq!(
        constraint_count, 1,
        "chk_secrets_variable_kind CHECK constraint must exist"
    );

    // Net-state invariants that hold regardless of post-migration inserts:
    // every row carries a valid kind, and no machine `secret-field/%` row is
    // ever classified as a named automation secret (create_named rejects the
    // reserved prefix; the backfill only promoted non-secret-field keys).
    let drift = sqlx::query(
        r#"
        SELECT COUNT(*)::BIGINT AS count
        FROM secrets_variable
        WHERE kind NOT IN ('field', 'named')
           OR (kind = 'named' AND key LIKE 'secret-field/%')
        "#,
    )
    .fetch_one(&db)
    .await
    .expect("query secrets_variable");
    let drift_count: i64 = drift.try_get("count").expect("count");
    assert_eq!(
        drift_count, 0,
        "kind must be valid and no secret-field/* row may be classified as named"
    );
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_secrets_canonicalization -- --ignored
async fn config_context_payload_has_no_secret_transport_schemes() {
    let db = build_test_db_pool().await.expect("database pool");
    let row = sqlx::query(
        r#"
        SELECT COUNT(*)::BIGINT AS count
        FROM config_context
        WHERE data::text ILIKE '%secret:%'
           OR data::text ILIKE '%vault:%'
        "#,
    )
    .fetch_one(&db)
    .await
    .expect("query config_context");

    let count: i64 = row.try_get("count").expect("count");
    assert_eq!(
        count, 0,
        "config_context data must not contain secret transport schemes"
    );
}
