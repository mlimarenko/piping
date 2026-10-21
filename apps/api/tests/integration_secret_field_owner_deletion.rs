//! Native owner-adapter retirement, definition scope and deferred-blocker proof.

mod common;
#[path = "support/secret_field_inventory_fixtures.rs"]
mod inventory_fixtures;

use std::sync::Arc;
use piping_backend::contexts::connections::infrastructure::ConnectionRepository;
use piping_backend::contexts::connections::infrastructure::persistence::repositories::sqlx_connection_repository::SqlxConnectionRepository;
use piping_backend::contexts::connection_config::infrastructure::persistence::record_repository::{ConnConfRecordRepository, SqlxConnConfRecordRepository};
use piping_backend::contexts::custom_fields::domain::UpdateCustomFieldDefinition;
use piping_backend::contexts::custom_fields::infrastructure::persistence::repositories::custom_field_definition_repository::{CustomFieldDefinitionRepository, SqlxCustomFieldDefinitionRepository};
use piping_backend::contexts::inventory::infrastructure::persistence::repositories::delete_guard::delete_inventory_record;
use piping_backend::contexts::secrets::application::{ApplySecretFieldIntentInput, SecretFieldIntent, SecretFieldIntentService};
use piping_backend::contexts::secrets::infrastructure::{SecretRepository, SqlxSecretRepository};
use piping_backend::contexts::sync::infrastructure::persistence::repositories::{ProviderRepository, SqlxProviderRepository};
use sqlx::PgPool;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[allow(
    clippy::expect_used,
    reason = "adapter helper outside the #[test] fns clippy.toml exempts; a set intent that yields no secret id is a broken fixture, so the calling test must abort"
)]
async fn set(
    repository: &Arc<SqlxSecretRepository>,
    consumer_type: &str,
    consumer_id: Uuid,
    scope: &str,
    owner: Uuid,
    field: &str,
) -> Result<Uuid, piping_backend::errors::AppError> {
    Ok(SecretFieldIntentService::new(repository.clone())
        .apply_intent(ApplySecretFieldIntentInput {
            consumer_type: consumer_type.into(),
            consumer_id,
            owner_scope_type: scope.into(),
            owner_scope_id: owner,
            field_key: field.into(),
            intent: SecretFieldIntent::Set,
            value: Some("synthetic-native-owner-value".into()),
            secret_id: None,
        })
        .await?
        .secret_id
        .expect("native field secret"))
}

async fn retained(pool: &PgPool, secret_id: Uuid, expected: i64) -> TestResult {
    for table in [
        "secrets_variable",
        "secrets_store",
        "secrets_binding",
        "secret_usage_binding",
    ] {
        let column = if table == "secrets_variable" {
            "id"
        } else {
            "secret_id"
        };
        let count: i64 =
            sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE {column} = $1"))
                .bind(secret_id)
                .fetch_one(pool)
                .await?;
        assert_eq!(
            count, expected,
            "native owner lifecycle count differs in {table}"
        );
    }
    Ok(())
}

#[tokio::test]
#[ignore] // Runs actual repository adapters against only the isolated database.
async fn credential_sets_personal_shared_records_and_providers_retire_only_owned_fields()
-> TestResult {
    let state = common::build_test_app_state().await?;
    let pool = &state.db_pool;
    let secrets = Arc::new(SqlxSecretRepository::new(pool.clone()));
    let user: Uuid = sqlx::query_scalar("SELECT id FROM identity_user WHERE username = $1")
        .bind(common::TEST_ADMIN_USERNAME)
        .fetch_one(pool)
        .await?;
    let connections = SqlxConnectionRepository::new(pool.clone());
    let records = SqlxConnConfRecordRepository::new(pool.clone());
    let providers = SqlxProviderRepository::new(pool.clone());

    let credential_set = Uuid::now_v7();
    sqlx::query("INSERT INTO connections_user_credential_set (id, user_id, protocol, name, is_default) VALUES ($1, $2, 'ssh', 'synthetic-nondefault', false)")
        .bind(credential_set).bind(user).execute(pool).await?;
    let field = set(
        &secrets,
        "user_connection_credential_set",
        credential_set,
        "user",
        user,
        "ssh_password",
    )
    .await?;
    connections
        .delete_user_credential_set(credential_set)
        .await?;
    retained(pool, field, 0).await?;
    connections
        .delete_user_credential_set(credential_set)
        .await?;

    for owner in [Some(user), None] {
        let record = Uuid::now_v7();
        sqlx::query("INSERT INTO connconf_record (id, name, protocol, owner_user_id) VALUES ($1, $2, 'ssh', $3)")
            .bind(record).bind(format!("synthetic-record-{record}")).bind(owner).execute(pool).await?;
        let (scope, owner_id) = owner.map_or(("connection_config", record), |id| ("user", id));
        let field = set(
            &secrets,
            "connection_config:record",
            record,
            scope,
            owner_id,
            "credentials.password",
        )
        .await?;
        assert!(records.delete(record).await?);
        retained(pool, field, 0).await?;
        assert!(!records.delete(record).await?);
    }

    let provider = Uuid::now_v7();
    sqlx::query("INSERT INTO sync_provider (id, name, provider_type, config) VALUES ($1, $2, 'netbox', '{}')")
        .bind(provider).bind(format!("synthetic-provider-{provider}")).execute(pool).await?;
    let field = set(
        &secrets,
        "sync_provider",
        provider,
        "provider",
        provider,
        "api_token",
    )
    .await?;
    providers.delete(provider).await?;
    retained(pool, field, 0).await?;
    Ok(())
}

#[tokio::test]
#[ignore] // Real definition/inventory adapters; native deferred FK must roll back all field rows.
async fn definition_scope_secrecy_toggle_and_inventory_blocker_are_atomic() -> TestResult {
    let state = common::build_test_app_state().await?;
    let pool = &state.db_pool;
    let secrets = Arc::new(SqlxSecretRepository::new(pool.clone()));
    let definitions = SqlxCustomFieldDefinitionRepository::new(pool.clone());
    let site_type: Uuid = sqlx::query_scalar(
        "SELECT id FROM inventory_content_type WHERE app_label = 'dcim' AND model = 'site'",
    )
    .fetch_one(pool)
    .await?;
    let vm_type: Uuid = sqlx::query_scalar("SELECT id FROM inventory_content_type WHERE app_label = 'virtualization' AND model = 'virtualmachine'").fetch_one(pool).await?;
    let site = Uuid::now_v7();
    let vm = Uuid::now_v7();
    inventory_fixtures::seed_inventory_entity(pool, "dcim.site", site).await?;
    inventory_fixtures::seed_inventory_entity(pool, "virtualization.virtualmachine", vm).await?;
    let field_name = format!("lifecycle_{}", Uuid::now_v7().simple());
    let definition = Uuid::now_v7();
    sqlx::query("INSERT INTO custom_field (id, name, is_secret) VALUES ($1, $2, true)")
        .bind(definition)
        .bind(&field_name)
        .execute(pool)
        .await?;
    for content_type in [site_type, vm_type] {
        sqlx::query("INSERT INTO custom_field_object_type (custom_field_id, content_type_id) VALUES ($1, $2)")
            .bind(definition).bind(content_type).execute(pool).await?;
    }
    let site_secret = set(
        &secrets,
        "inventory_custom_field",
        site,
        "inventory_custom_field",
        site,
        &field_name,
    )
    .await?;
    let vm_secret = set(
        &secrets,
        "inventory_custom_field",
        vm,
        "inventory_custom_field",
        vm,
        &field_name,
    )
    .await?;
    definitions
        .update(
            definition,
            &UpdateCustomFieldDefinition {
                object_types: Some(vec![vm_type]),
                ..Default::default()
            },
        )
        .await?;
    retained(pool, site_secret, 0).await?;
    retained(pool, vm_secret, 1).await?;
    definitions
        .update(
            definition,
            &UpdateCustomFieldDefinition {
                is_secret: Some(false),
                ..Default::default()
            },
        )
        .await?;
    retained(pool, vm_secret, 0).await?;
    definitions
        .update(
            definition,
            &UpdateCustomFieldDefinition {
                is_secret: Some(true),
                ..Default::default()
            },
        )
        .await?;
    let vm_secret = set(
        &secrets,
        "inventory_custom_field",
        vm,
        "inventory_custom_field",
        vm,
        &field_name,
    )
    .await?;
    assert!(definitions.delete(definition).await?);
    retained(pool, vm_secret, 0).await?;
    assert!(!definitions.delete(definition).await?);

    let target_secret = set(
        &secrets,
        "connection_scope:connection_target",
        vm,
        "connection_target",
        vm,
        "piping.connections.ssh.0.password",
    )
    .await?;
    assert_eq!(
        delete_inventory_record(pool, "virtualization_virtualmachine", vm).await?,
        1
    );
    retained(pool, target_secret, 0).await?;

    let definition = Uuid::now_v7();
    let field_name = format!("blocked_{}", Uuid::now_v7().simple());
    sqlx::query("INSERT INTO custom_field (id, name, is_secret) VALUES ($1, $2, true)")
        .bind(definition)
        .bind(&field_name)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO custom_field_object_type (custom_field_id, content_type_id) VALUES ($1, $2)",
    )
    .bind(definition)
    .bind(site_type)
    .execute(pool)
    .await?;
    let secret = set(
        &secrets,
        "inventory_custom_field",
        site,
        "inventory_custom_field",
        site,
        &field_name,
    )
    .await?;
    let device = Uuid::now_v7();
    inventory_fixtures::seed_inventory_entity(pool, "dcim.device", device).await?;
    sqlx::query("UPDATE dcim_device SET site_id = $2 WHERE id = $1")
        .bind(device)
        .bind(site)
        .execute(pool)
        .await?;
    assert!(matches!(
        delete_inventory_record(pool, "dcim_site", site).await,
        Err(piping_backend::errors::AppError::DeleteBlocked(_))
    ));
    retained(pool, secret, 1).await?;
    assert_eq!(
        delete_inventory_record(pool, "dcim_device", device).await?,
        1
    );
    assert_eq!(delete_inventory_record(pool, "dcim_site", site).await?, 1);
    retained(pool, secret, 0).await?;
    assert!(
        secrets
            .get_usage_binding("inventory_custom_field", site, &field_name)
            .await?
            .is_none()
    );
    Ok(())
}

#[tokio::test]
#[ignore] // Actual native cooling CASCADE, not CONTENT_TYPES parent-label metadata.
async fn device_delete_retires_cascaded_cooling_child_custom_fields() -> TestResult {
    let state = common::build_test_app_state().await?;
    let pool = &state.db_pool;
    let secrets = Arc::new(SqlxSecretRepository::new(pool.clone()));
    let device = Uuid::now_v7();
    inventory_fixtures::seed_inventory_entity(pool, "dcim.device", device).await?;
    let intake = Uuid::now_v7();
    sqlx::query("INSERT INTO dcim_coolingintake (id, custom_field_data, name, label, device_id, description) VALUES ($1, '{}', 'synthetic-intake', '', $2, '')")
        .bind(intake).bind(device).execute(pool).await?;
    let content_type: Uuid = sqlx::query_scalar("SELECT id FROM inventory_content_type WHERE app_label = 'dcim' AND model = 'coolingintake'")
        .fetch_one(pool).await?;
    let definition = Uuid::now_v7();
    let field_name = format!("cooling_{}", Uuid::now_v7().simple());
    sqlx::query("INSERT INTO custom_field (id, name, is_secret) VALUES ($1, $2, true)")
        .bind(definition)
        .bind(&field_name)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO custom_field_object_type (custom_field_id, content_type_id) VALUES ($1, $2)",
    )
    .bind(definition)
    .bind(content_type)
    .execute(pool)
    .await?;
    let secret = set(
        &secrets,
        "inventory_custom_field",
        intake,
        "inventory_custom_field",
        intake,
        &field_name,
    )
    .await?;
    assert_eq!(
        delete_inventory_record(pool, "dcim_device", device).await?,
        1
    );
    retained(pool, secret, 0).await?;
    let remains: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM dcim_coolingintake WHERE id = $1)")
            .bind(intake)
            .fetch_one(pool)
            .await?;
    assert!(!remains);
    Ok(())
}
