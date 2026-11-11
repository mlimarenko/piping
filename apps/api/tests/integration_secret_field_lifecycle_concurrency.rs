//! Native lock-order/cancellation-bounded races, without timers as synchronization.

mod common;
#[path = "support/secret_field_inventory_fixtures.rs"]
mod inventory_fixtures;

use std::sync::Arc;
use std::time::Duration;
use piping_backend::contexts::custom_fields::infrastructure::persistence::repositories::custom_field_definition_repository::{CustomFieldDefinitionRepository, SqlxCustomFieldDefinitionRepository};
use piping_backend::contexts::inventory::infrastructure::persistence::repositories::delete_guard::delete_inventory_record;
use piping_backend::contexts::secrets::application::{ApplySecretFieldIntentInput, SecretFieldIntent, SecretFieldIntentService};
use piping_backend::contexts::secrets::domain::SecretBinding;
use piping_backend::contexts::secrets::infrastructure::{SecretRepository, SqlxSecretRepository};
use sqlx::PgPool;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

fn intent(
    consumer: &str,
    scope: &str,
    owner: Uuid,
    field: &str,
    command: SecretFieldIntent,
    secret_id: Option<Uuid>,
) -> ApplySecretFieldIntentInput {
    ApplySecretFieldIntentInput {
        consumer_type: consumer.into(),
        consumer_id: owner,
        owner_scope_type: scope.into(),
        owner_scope_id: owner,
        field_key: field.into(),
        intent: command,
        secret_id,
        value: (command != SecretFieldIntent::Clear && secret_id.is_none())
            .then(|| "synthetic-race-value".into()),
    }
}

async fn absent(pool: &PgPool, secret_id: Uuid) -> TestResult {
    let rows: i64 = sqlx::query_scalar("SELECT (SELECT count(*) FROM secrets_variable WHERE id = $1) + (SELECT count(*) FROM secrets_store WHERE secret_id = $1) + (SELECT count(*) FROM secrets_binding WHERE secret_id = $1) + (SELECT count(*) FROM secret_usage_binding WHERE secret_id = $1)")
        .bind(secret_id).fetch_one(pool).await?;
    assert_eq!(rows, 0, "raced retirement left field rows");
    Ok(())
}

async fn definition(pool: &PgPool, model: &str, name: &str) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO custom_field (id, name, is_secret) VALUES ($1, $2, true)")
        .bind(id)
        .bind(name)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO custom_field_object_type (custom_field_id, content_type_id) SELECT $1, id FROM inventory_content_type WHERE app_label = 'dcim' AND model = $2")
        .bind(id).bind(model).execute(pool).await?;
    Ok(id)
}

#[tokio::test]
#[ignore] // Native catalogue latch and actual cascade row locks, isolated server only.
async fn definition_delete_competing_with_set_and_parent_cascade_is_bounded() -> TestResult {
    let state = common::build_test_app_state().await?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .connect(&common::database_url())
        .await?;
    let secrets = Arc::new(SqlxSecretRepository::new(pool.clone()));
    let service = Arc::new(SecretFieldIntentService::new(secrets));
    let definitions = Arc::new(SqlxCustomFieldDefinitionRepository::new(pool.clone()));
    let site = Uuid::now_v7();
    inventory_fixtures::seed_inventory_entity(&pool, "dcim.site", site).await?;
    let field = format!("race_{}", Uuid::now_v7().simple());
    let definition_id = definition(&pool, "site", &field).await?;
    let old = service
        .apply_intent(intent(
            "inventory_custom_field",
            "inventory_custom_field",
            site,
            &field,
            SecretFieldIntent::Set,
            None,
        ))
        .await?
        .secret_id
        .expect("initial field");
    let barrier = Arc::new(tokio::sync::Barrier::new(3));
    let write = {
        let service = service.clone();
        let barrier = barrier.clone();
        let field = field.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            service
                .apply_intent(intent(
                    "inventory_custom_field",
                    "inventory_custom_field",
                    site,
                    &field,
                    SecretFieldIntent::Set,
                    None,
                ))
                .await
        })
    };
    let deletion = {
        let definitions = definitions.clone();
        let barrier = barrier.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            definitions.delete(definition_id).await
        })
    };
    barrier.wait().await;
    let (write, deletion) = tokio::time::timeout(Duration::from_secs(15), async {
        tokio::join!(write, deletion)
    })
    .await?;
    assert!(deletion??);
    if let Ok(written) = write? {
        absent(&pool, written.secret_id.expect("raced field")).await?;
    }
    absent(&pool, old).await?;

    let device = Uuid::now_v7();
    inventory_fixtures::seed_inventory_entity(&pool, "dcim.device", device).await?;
    let child = Uuid::now_v7();
    sqlx::query("INSERT INTO dcim_coolingintake (id, custom_field_data, name, label, device_id, description) VALUES ($1, '{}', 'synthetic-race-intake', '', $2, '')")
        .bind(child).bind(device).execute(&pool).await?;
    let field = format!("race_{}", Uuid::now_v7().simple());
    let definition_id = definition(&pool, "coolingintake", &field).await?;
    let secret = service
        .apply_intent(intent(
            "inventory_custom_field",
            "inventory_custom_field",
            child,
            &field,
            SecretFieldIntent::Set,
            None,
        ))
        .await?
        .secret_id
        .expect("child field");
    let barrier = Arc::new(tokio::sync::Barrier::new(3));
    let definition_delete = {
        let definitions = definitions.clone();
        let barrier = barrier.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            definitions.delete(definition_id).await
        })
    };
    let cascade = {
        let pool = pool.clone();
        let barrier = barrier.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            delete_inventory_record(&pool, "dcim_device", device).await
        })
    };
    barrier.wait().await;
    let (definition_delete, cascade) = tokio::time::timeout(Duration::from_secs(15), async {
        tokio::join!(definition_delete, cascade)
    })
    .await?;
    assert!(definition_delete??);
    assert_eq!(cascade??, 1);
    absent(&pool, secret).await?;
    pool.close().await;
    drop(state);
    Ok(())
}

#[tokio::test]
#[ignore] // Actual PostgreSQL variable locks for promotion/rebind/retirement.
async fn explicit_protection_and_rebind_competing_with_retirement_are_bounded() -> TestResult {
    let state = common::build_test_app_state().await?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .connect(&common::database_url())
        .await?;
    let secrets = Arc::new(SqlxSecretRepository::new(pool.clone()));
    let service = Arc::new(SecretFieldIntentService::new(secrets.clone()));
    let owner = Uuid::now_v7();
    sqlx::query("INSERT INTO connections_proxy_node (id, name, slug, base_url) VALUES ($1, $2, $2, 'https://synthetic-race.invalid')")
        .bind(owner).bind(format!("lifecycle-{owner}")).execute(&pool).await?;
    let secret = service
        .apply_intent(intent(
            "proxy_node:credential",
            "proxy_node",
            owner,
            "protect",
            SecretFieldIntent::Set,
            None,
        ))
        .await?
        .secret_id
        .expect("initial field");
    let barrier = Arc::new(tokio::sync::Barrier::new(3));
    let promotion = {
        let secrets = secrets.clone();
        let barrier = barrier.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            secrets
                .create_binding(SecretBinding {
                    id: Uuid::now_v7(),
                    secret_id: secret,
                    scope_type: "proxy_node".into(),
                    scope_id: owner,
                    weight: 0,
                    managed_by_field_usage: true,
                })
                .await
        })
    };
    let retirement = {
        let service = service.clone();
        let barrier = barrier.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            service
                .apply_intent(intent(
                    "proxy_node:credential",
                    "proxy_node",
                    owner,
                    "protect",
                    SecretFieldIntent::Clear,
                    None,
                ))
                .await
        })
    };
    barrier.wait().await;
    let (promotion, retirement) = tokio::time::timeout(Duration::from_secs(15), async {
        tokio::join!(promotion, retirement)
    })
    .await?;
    retirement??;
    match promotion? {
        Ok(binding) => {
            assert!(!binding.managed_by_field_usage);
            assert!(secrets.get_variable_by_id(secret).await?.is_some());
            assert_eq!(secrets.list_bindings_for_secret(secret).await?.len(), 1);
        }
        Err(piping_backend::errors::AppError::NotFound(_)) => absent(&pool, secret).await?,
        Err(error) => return Err(error.into()),
    }

    let old = service
        .apply_intent(intent(
            "proxy_node:credential",
            "proxy_node",
            owner,
            "source",
            SecretFieldIntent::Set,
            None,
        ))
        .await?
        .secret_id
        .expect("source");
    let replacement = service
        .apply_intent(intent(
            "proxy_node:credential",
            "proxy_node",
            owner,
            "target",
            SecretFieldIntent::Set,
            None,
        ))
        .await?
        .secret_id
        .expect("target");
    let barrier = Arc::new(tokio::sync::Barrier::new(3));
    let rebind = {
        let service = service.clone();
        let barrier = barrier.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            service
                .apply_intent(intent(
                    "proxy_node:credential",
                    "proxy_node",
                    owner,
                    "source",
                    SecretFieldIntent::Set,
                    Some(replacement),
                ))
                .await
        })
    };
    let retirement = {
        let service = service.clone();
        let barrier = barrier.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            service
                .apply_intent(intent(
                    "proxy_node:credential",
                    "proxy_node",
                    owner,
                    "target",
                    SecretFieldIntent::Clear,
                    None,
                ))
                .await
        })
    };
    barrier.wait().await;
    let (rebind, retirement) = tokio::time::timeout(Duration::from_secs(15), async {
        tokio::join!(rebind, retirement)
    })
    .await?;
    retirement??;
    match rebind? {
        Ok(state) => {
            assert_eq!(state.secret_id, Some(replacement));
            absent(&pool, old).await?;
            assert_eq!(
                secrets
                    .get_usage_binding("proxy_node:credential", owner, "source")
                    .await?
                    .expect("replacement usage")
                    .secret_id,
                replacement
            );
            assert_eq!(
                secrets.list_bindings_for_secret(replacement).await?.len(),
                1
            );
        }
        Err(piping_backend::errors::AppError::NotFound(_)) => {
            absent(&pool, replacement).await?;
            assert_eq!(
                secrets
                    .get_usage_binding("proxy_node:credential", owner, "source")
                    .await?
                    .expect("unchanged usage")
                    .secret_id,
                old
            );
        }
        Err(error) => return Err(error.into()),
    }
    pool.close().await;
    drop(state);
    Ok(())
}
