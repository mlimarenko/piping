//! Atomic batch and caller-owned transaction regressions on the isolated database.

mod common;

use std::sync::Arc;

use piping_backend::contexts::custom_fields::SecretWriteIntent;
use piping_backend::contexts::secrets::application::{
    ApplySecretFieldIntentInput, SecretFieldIntent, SecretFieldIntentService,
};
use piping_backend::contexts::secrets::infrastructure::{SecretRepository, SqlxSecretRepository};
use piping_backend::errors::AppError;
use sqlx::PgPool;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

fn input(owner: Uuid, field: &str) -> ApplySecretFieldIntentInput {
    ApplySecretFieldIntentInput {
        consumer_type: "proxy_node:credential".into(),
        consumer_id: owner,
        owner_scope_type: "proxy_node".into(),
        owner_scope_id: owner,
        field_key: field.into(),
        intent: SecretFieldIntent::Set,
        value: Some("synthetic-batch-value".into()),
        secret_id: None,
    }
}

fn native_input(owner: Uuid) -> ApplySecretFieldIntentInput {
    ApplySecretFieldIntentInput {
        consumer_type: "inventory_fhrp_group".into(),
        owner_scope_type: "inventory_fhrp_group".into(),
        ..input(owner, "auth_key")
    }
}

fn invalid_native_inputs(owner: Uuid) -> Vec<ApplySecretFieldIntentInput> {
    let valid = native_input(owner);
    let mut wrong_field = valid.clone();
    wrong_field.field_key = "other_field".into();
    let mut wrong_scope = valid.clone();
    wrong_scope.owner_scope_type = "proxy_node".into();
    let mut wrong_owner = valid.clone();
    wrong_owner.owner_scope_id = Uuid::now_v7();
    let mut attached = valid.clone();
    attached.secret_id = Some(Uuid::now_v7());
    let mut oversized = valid;
    oversized.value = Some("я".repeat(256));
    vec![wrong_field, wrong_scope, wrong_owner, attached, oversized]
}

#[tokio::test]
async fn whole_batch_native_preflight_needs_no_database_at_service_and_repository_boundaries() {
    // A closed lazy pool cannot perform any database work. Reaching pool.begin()
    // produces PoolClosed rather than the expected pure Validation failure.
    let pool = sqlx::postgres::PgPoolOptions::new().connect_lazy_with(
        sqlx::postgres::PgConnectOptions::new()
            .host("127.0.0.1")
            .port(1)
            .username("unused_preflight_fixture")
            .database("unused_preflight_fixture"),
    );
    pool.close().await;
    assert!(matches!(pool.acquire().await, Err(sqlx::Error::PoolClosed)));
    let repository = Arc::new(SqlxSecretRepository::new(pool));
    let service = SecretFieldIntentService::new(repository.clone());
    let owner = Uuid::now_v7();
    for invalid in invalid_native_inputs(owner) {
        let batch = vec![input(owner, "valid-first"), invalid];
        assert!(matches!(
            service.apply_intents(batch.clone()).await,
            Err(AppError::Validation(_))
        ));
        assert!(matches!(
            repository.apply_field_intents(batch).await,
            Err(AppError::Validation(_))
        ));
    }
    // Exactly 255 Unicode characters is valid and therefore reaches the closed
    // persistence boundary rather than being rejected by a byte-length limit.
    let mut valid = native_input(owner);
    valid.value = Some("я".repeat(255));
    let result = service.apply_intents(vec![valid]).await;
    assert!(result.is_err());
    assert!(!matches!(result, Err(AppError::Validation(_))));
}

async fn proxy(pool: &PgPool, id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO connections_proxy_node (id, name, slug, base_url) VALUES ($1, $2, $2, 'https://synthetic-batch.invalid')")
        .bind(id)
        .bind(format!("batch-{id}"))
        .execute(pool)
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore] // common resets the database; requires the explicitly isolated native stack.
async fn late_batch_failure_rolls_back_earlier_secret_rotation() -> TestResult {
    let state = common::build_test_app_state().await?;
    let pool = &state.db_pool;
    let repository = Arc::new(SqlxSecretRepository::new(pool.clone()));
    let service = SecretFieldIntentService::new(repository.clone());
    let owner = Uuid::now_v7();
    proxy(pool, owner).await?;
    let original = service.apply_intent(input(owner, "credential")).await?;
    let original_secret = original.secret_id.expect("managed secret");
    let mut rotation = input(owner, "credential");
    rotation.intent = SecretFieldIntent::Rotate;
    assert!(
        service
            .apply_intents(vec![rotation, input(Uuid::now_v7(), "missing-owner")])
            .await
            .is_err()
    );
    let binding = repository
        .get_usage_binding("proxy_node:credential", owner, "credential")
        .await?
        .expect("original binding");
    assert_eq!(binding.secret_id, original_secret);
    assert!(
        repository
            .get_variable_by_id(original_secret)
            .await?
            .is_some()
    );
    Ok(())
}

#[tokio::test]
#[ignore] // common resets the database; requires the explicitly isolated native stack.
async fn whitespace_in_later_input_is_rejected_before_any_mutation() -> TestResult {
    let state = common::build_test_app_state().await?;
    let entity_id = Uuid::now_v7();
    let values = &state.services.custom_field_values;
    assert!(matches!(
        values.prepare_secret_intents(
            entity_id,
            vec![
                SecretWriteIntent {
                    field_key: "first".into(),
                    intent: SecretFieldIntent::Set,
                    value: Some("synthetic-value".into())
                },
                SecretWriteIntent {
                    field_key: "second".into(),
                    intent: SecretFieldIntent::Set,
                    value: Some(" \t ".into())
                },
            ]
        ),
        Err(AppError::Validation(_))
    ));
    let prepared = values.prepare_secret_intents(
        entity_id,
        vec![SecretWriteIntent {
            field_key: " credential ".into(),
            intent: SecretFieldIntent::Clear,
            value: None,
        }],
    )?;
    assert_eq!(prepared[0].field_key, "credential");
    assert_eq!(prepared[0].intent, SecretFieldIntent::Clear);
    let pool = &state.db_pool;
    let repository = Arc::new(SqlxSecretRepository::new(pool.clone()));
    let service = SecretFieldIntentService::new(repository.clone());
    let owner = Uuid::now_v7();
    proxy(pool, owner).await?;
    let mut invalid = input(owner, "second");
    invalid.value = Some(" \t\n ".into());
    assert!(matches!(
        service
            .apply_intents(vec![input(owner, "first"), invalid.clone()])
            .await,
        Err(AppError::Validation(_))
    ));
    assert!(
        repository
            .list_usage_bindings_for_consumer("proxy_node:credential", owner)
            .await?
            .is_empty()
    );
    let mut transaction = pool.begin().await?;
    assert!(matches!(
        service
            .apply_intents_in_transaction(&mut transaction, vec![input(owner, "first"), invalid])
            .await,
        Err(AppError::Validation(_))
    ));
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM secret_usage_binding WHERE consumer_id = $1")
            .bind(owner)
            .fetch_one(&mut *transaction)
            .await?;
    assert_eq!(
        count, 0,
        "validation must finish before the first transaction write"
    );
    for invalid in invalid_native_inputs(owner) {
        assert!(matches!(
            repository
                .apply_field_intents_in_transaction(
                    &mut transaction,
                    vec![input(owner, "first"), invalid],
                )
                .await,
            Err(AppError::Validation(_))
        ));
    }
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM secret_usage_binding WHERE consumer_id = $1")
            .bind(owner)
            .fetch_one(&mut *transaction)
            .await?;
    assert_eq!(
        count, 0,
        "repository preflight must precede every lifecycle write"
    );
    transaction.rollback().await?;
    Ok(())
}

#[tokio::test]
#[ignore] // common resets the database; requires the explicitly isolated native stack.
async fn supplied_transaction_keeps_entity_and_secret_uncommitted() -> TestResult {
    let state = common::build_test_app_state().await?;
    let pool = &state.db_pool;
    let repository = Arc::new(SqlxSecretRepository::new(pool.clone()));
    let service = SecretFieldIntentService::new(repository.clone());
    let owner = Uuid::now_v7();
    let mut transaction = pool.begin().await?;
    sqlx::query("INSERT INTO connections_proxy_node (id, name, slug, base_url) VALUES ($1, $2, $2, 'https://synthetic-batch.invalid')")
        .bind(owner)
        .bind(format!("batch-{owner}"))
        .execute(&mut *transaction)
        .await?;
    let states = service
        .apply_intents_in_transaction(
            &mut transaction,
            vec![input(owner, "first"), input(owner, "second")],
        )
        .await?;
    assert_eq!(states.len(), 2);
    transaction.rollback().await?;
    for state in states {
        assert!(
            repository
                .get_variable_by_id(state.secret_id.expect("managed secret"))
                .await?
                .is_none()
        );
    }
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM connections_proxy_node WHERE id = $1)")
            .bind(owner)
            .fetch_one(pool)
            .await?;
    assert!(!exists);
    Ok(())
}
