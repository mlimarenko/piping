//! Integration test for credential-set backfill migration semantics.
//!
//! Requires PostgreSQL and Redis (e.g. `make run-up`). Run with:
//! `cargo test --test integration_user_credential_sets_migration -- --ignored`

mod common;

use common::{build_test_app_state, build_test_db_pool};
use sqlx::Row;
use uuid::Uuid;

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_user_credential_sets_migration -- --ignored
async fn migration_backfill_creates_default_sets_and_set_scoped_bindings_deterministically() {
    let _state = build_test_app_state().await.expect("app state");
    let pool = build_test_db_pool().await.expect("db pool");

    let user_id = Uuid::now_v7();
    let username = format!("migration-user-{}", Uuid::now_v7());
    let email = format!("{username}@test.local");
    sqlx::query(
        r#"
        INSERT INTO identity_user (id, username, email, status, auth_source)
        VALUES ($1, $2, $3, 'active', 'local')
        "#,
    )
    .bind(user_id)
    .bind(&username)
    .bind(&email)
    .execute(&pool)
    .await
    .expect("insert migration test user");

    let ssh_password_secret_id = Uuid::now_v7();
    let ssh_private_key_secret_id = Uuid::now_v7();
    let ssh_password_key = format!("migration.ssh.password.{}", Uuid::now_v7());
    let ssh_private_key_key = format!("migration.ssh.private_key.{}", Uuid::now_v7());

    sqlx::query(
        r#"
        INSERT INTO secrets_variable (id, key, is_masked, is_protected)
        VALUES ($1, $2, true, false), ($3, $4, true, false)
        "#,
    )
    .bind(ssh_password_secret_id)
    .bind(&ssh_password_key)
    .bind(ssh_private_key_secret_id)
    .bind(&ssh_private_key_key)
    .execute(&pool)
    .await
    .expect("insert migration test secrets");

    sqlx::query(
        r#"
        INSERT INTO secret_usage_binding (
            id, consumer_type, consumer_id, field_key, owner_scope_type, owner_scope_id, secret_id
        )
        VALUES
            ($1, 'user_connection_credential', $2, 'ssh_password', 'user', $2, $3),
            ($4, 'user_connection_credential', $2, 'ssh_private_key', 'user', $2, $5)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(ssh_password_secret_id)
    .bind(Uuid::now_v7())
    .bind(ssh_private_key_secret_id)
    .execute(&pool)
    .await
    .expect("insert legacy usage bindings");

    let backfill_insert_sql = r#"
        WITH source_bindings AS (
            SELECT
                b.owner_scope_id AS user_id,
                CASE
                    WHEN b.field_key IN ('ssh_password', 'ssh_private_key') THEN 'ssh'
                    WHEN b.field_key IN ('rdp_username', 'rdp_password') THEN 'rdp'
                    WHEN b.field_key IN ('vnc_password') THEN 'vnc'
                    ELSE NULL
                END AS protocol
            FROM secret_usage_binding b
            WHERE b.consumer_type = 'user_connection_credential'
              AND b.owner_scope_type = 'user'
        ),
        default_sets AS (
            INSERT INTO connections_user_credential_set (id, user_id, protocol, name, is_default)
            SELECT
                uuidv7(),
                sb.user_id,
                sb.protocol,
                'Default',
                TRUE
            FROM source_bindings sb
            WHERE sb.protocol IS NOT NULL
            GROUP BY sb.user_id, sb.protocol
            ON CONFLICT DO NOTHING
            RETURNING id, user_id, protocol
        ),
        all_sets AS (
            SELECT id, user_id, protocol
            FROM default_sets
            UNION ALL
            SELECT id, user_id, protocol
            FROM connections_user_credential_set
            WHERE is_default = TRUE
        ),
        source_rows AS (
            SELECT
                b.field_key,
                b.secret_id,
                b.owner_scope_type,
                b.owner_scope_id,
                CASE
                    WHEN b.field_key IN ('ssh_password', 'ssh_private_key') THEN 'ssh'
                    WHEN b.field_key IN ('rdp_username', 'rdp_password') THEN 'rdp'
                    WHEN b.field_key IN ('vnc_password') THEN 'vnc'
                    ELSE NULL
                END AS protocol
            FROM secret_usage_binding b
            WHERE b.consumer_type = 'user_connection_credential'
              AND b.owner_scope_type = 'user'
        )
        INSERT INTO secret_usage_binding (
            id,
            consumer_type,
            consumer_id,
            field_key,
            secret_id,
            owner_scope_type,
            owner_scope_id,
            created_at,
            updated_at
        )
        SELECT
            uuidv7(),
            'user_connection_credential_set',
            s.id,
            sr.field_key,
            sr.secret_id,
            sr.owner_scope_type,
            sr.owner_scope_id,
            NOW(),
            NOW()
        FROM source_rows sr
        JOIN all_sets s
          ON s.user_id = sr.owner_scope_id
         AND s.protocol = sr.protocol
        WHERE sr.protocol IS NOT NULL
        ON CONFLICT (consumer_type, consumer_id, field_key)
        DO NOTHING;
    "#;
    let backfill_delete_legacy_sql = r#"
        DELETE FROM secret_usage_binding
        WHERE consumer_type = 'user_connection_credential';
    "#;

    sqlx::query(backfill_insert_sql)
        .execute(&pool)
        .await
        .expect("run backfill first pass");
    sqlx::query(backfill_delete_legacy_sql)
        .execute(&pool)
        .await
        .expect("delete legacy bindings first pass");

    let default_set_row = sqlx::query(
        r#"
        SELECT id, protocol, is_default
        FROM connections_user_credential_set
        WHERE user_id = $1 AND protocol = 'ssh' AND is_default = TRUE
        "#,
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .expect("default set row");
    let set_id: Uuid = default_set_row.get("id");

    let first_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)::BIGINT
        FROM secret_usage_binding
        WHERE consumer_type = 'user_connection_credential_set'
          AND consumer_id = $1
          AND field_key IN ('ssh_password', 'ssh_private_key')
        "#,
    )
    .bind(set_id)
    .fetch_one(&pool)
    .await
    .expect("count set bindings first pass");
    assert_eq!(
        first_count, 2,
        "backfill must create exactly two ssh bindings for seeded legacy rows"
    );

    sqlx::query(backfill_insert_sql)
        .execute(&pool)
        .await
        .expect("run backfill second pass");
    sqlx::query(backfill_delete_legacy_sql)
        .execute(&pool)
        .await
        .expect("delete legacy bindings second pass");

    let default_set_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)::BIGINT
        FROM connections_user_credential_set
        WHERE user_id = $1 AND protocol = 'ssh' AND is_default = TRUE
        "#,
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .expect("default set count second pass");
    assert_eq!(
        default_set_count, 1,
        "backfill rerun must not create duplicate default sets"
    );

    let second_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)::BIGINT
        FROM secret_usage_binding
        WHERE consumer_type = 'user_connection_credential_set'
          AND consumer_id = $1
          AND field_key IN ('ssh_password', 'ssh_private_key')
        "#,
    )
    .bind(set_id)
    .fetch_one(&pool)
    .await
    .expect("count set bindings second pass");
    assert_eq!(
        second_count, 2,
        "backfill rerun must be idempotent for usage bindings"
    );

    let legacy_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)::BIGINT
        FROM secret_usage_binding
        WHERE consumer_type = 'user_connection_credential'
        "#,
    )
    .fetch_one(&pool)
    .await
    .expect("legacy binding count");
    assert_eq!(
        legacy_count, 0,
        "backfill must remove legacy user_connection_credential bindings"
    );
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_user_credential_sets_migration -- --ignored
async fn user_set_binding_constraints_reject_invalid_scope_and_field_key() {
    let _state = build_test_app_state().await.expect("app state");
    let pool = build_test_db_pool().await.expect("db pool");

    let user_id = Uuid::now_v7();
    let username = format!("constraint-user-{}", Uuid::now_v7());
    let email = format!("{username}@test.local");
    sqlx::query(
        r#"
        INSERT INTO identity_user (id, username, email, status, auth_source)
        VALUES ($1, $2, $3, 'active', 'local')
        "#,
    )
    .bind(user_id)
    .bind(&username)
    .bind(&email)
    .execute(&pool)
    .await
    .expect("insert user");

    let set_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO connections_user_credential_set (id, user_id, protocol, name, is_default)
        VALUES ($1, $2, 'ssh', 'Constraint set', true)
        "#,
    )
    .bind(set_id)
    .bind(user_id)
    .execute(&pool)
    .await
    .expect("insert set");

    let secret_id = Uuid::now_v7();
    let secret_key = format!("constraint.secret.{}", Uuid::now_v7());
    sqlx::query(
        r#"
        INSERT INTO secrets_variable (id, key, is_masked, is_protected)
        VALUES ($1, $2, true, false)
        "#,
    )
    .bind(secret_id)
    .bind(&secret_key)
    .execute(&pool)
    .await
    .expect("insert secret");

    let invalid_scope_result = sqlx::query(
        r#"
        INSERT INTO secret_usage_binding (
            id, consumer_type, consumer_id, field_key, owner_scope_type, owner_scope_id, secret_id
        )
        VALUES ($1, 'user_connection_credential_set', $2, 'ssh_password', 'host', $3, $4)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(set_id)
    .bind(Uuid::now_v7())
    .bind(secret_id)
    .execute(&pool)
    .await;
    assert!(
        invalid_scope_result.is_err(),
        "owner_scope_type != user must violate user_set binding constraint"
    );

    let invalid_field_key_result = sqlx::query(
        r#"
        INSERT INTO secret_usage_binding (
            id, consumer_type, consumer_id, field_key, owner_scope_type, owner_scope_id, secret_id
        )
        VALUES ($1, 'user_connection_credential_set', $2, 'ssh_token', 'user', $3, $4)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(set_id)
    .bind(user_id)
    .bind(secret_id)
    .execute(&pool)
    .await;
    assert!(
        invalid_field_key_result.is_err(),
        "unsupported user_set field_key must violate binding constraint"
    );
}
