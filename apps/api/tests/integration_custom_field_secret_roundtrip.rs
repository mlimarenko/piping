//! Integration regression tests for inventory custom-field secret handling.
//!
//! Requires PostgreSQL and Redis (e.g. `make run-up`). Run with:
//! `cargo test --test integration_custom_field_secret_roundtrip -- --ignored --test-threads=1`

mod common;
#[path = "support/secret_field_inventory_fixtures.rs"]
mod secret_field_inventory_fixtures;

use common::{build_test_app_state, build_test_db_pool};
use piping_backend::contexts::custom_fields::{
    CustomFieldType, FilterLogic, NewCustomFieldDefinition, UiEditable, UiVisible,
};
use piping_backend::contexts::secrets::infrastructure::SecretRepository;
use serde_json::{Map, Value, json};
use uuid::Uuid;

const SECRET_MASK: &str = "****";

/// A per-run unique, schema-valid (`^[a-z][a-z0-9_]{0,49}$`) field name that
/// fits the `custom_field.name varchar(50)` column. Custom-field
/// definitions are global and persist across test runs, so names must not
/// collide with a prior run's leftovers.
fn unique_field_name(prefix: &str) -> String {
    let suffix: String = Uuid::now_v7()
        .simple()
        .to_string()
        .chars()
        .take(12)
        .collect();
    format!("{prefix}_{suffix}")
}

/// Build a minimal secret text-field definition scoped to one content type.
fn secret_text_def(name: &str, content_type_id: Uuid) -> NewCustomFieldDefinition {
    NewCustomFieldDefinition {
        name: name.to_string(),
        label: name.to_string(),
        field_type: CustomFieldType::Text,
        description: String::new(),
        group_name: String::new(),
        required: false,
        enforce_unique: false,
        is_secret: true,
        default_value: None,
        weight: 100,
        validation_minimum: None,
        validation_maximum: None,
        validation_regex: String::new(),
        validation_schema: None,
        choice_set_id: None,
        related_object_type_id: None,
        related_object_filter: None,
        ui_visible: UiVisible::Always,
        ui_editable: UiEditable::Yes,
        is_cloneable: false,
        search_weight: 1000,
        filter_logic: FilterLogic::Loose,
        object_types: vec![content_type_id],
    }
}

/// HIGH-1: a read masks a set secret to `****`. A read-modify-write client that
/// PUTs the whole object back unchanged (including the `****` mask) must NOT
/// overwrite the stored cleartext with the literal mask — the mask is treated as
/// "keep existing". Exercised against every content type that shares the write
/// path (site / device / virtual machine), keyed off the static tokens.
#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: -- --ignored --test-threads=1
async fn masked_secret_round_trip_does_not_overwrite_cleartext()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await.expect("app state");
    let pool = build_test_db_pool().await.expect("db pool");
    let values = state.services.custom_field_values.clone();

    // One case per content type that shares `write_for_entity` / the masked
    // read path. Field names are globally unique (definitions are global).
    let cases = [
        ("dcim", "site", "site_secret_high1"),
        ("dcim", "device", "device_secret_high1"),
        ("virtualization", "virtualmachine", "vm_secret_high1"),
    ];

    for (app_label, model, prefix) in cases {
        let token = format!("{app_label}.{model}");
        let field_name = unique_field_name(prefix);
        let field_name = field_name.as_str();
        let content_type_id = lookup_content_type_id(&pool, app_label, model).await?;

        // A fresh secret definition for this content type.
        state
            .services
            .custom_field_definitions
            .create(secret_text_def(field_name, content_type_id))
            .await
            .unwrap_or_else(|e| panic!("create def for {token}: {e}"));

        let entity_id = Uuid::now_v7();
        secret_field_inventory_fixtures::seed_inventory_entity(&pool, &token, entity_id).await?;
        let cleartext = format!("super-secret-for-{model}");

        // 1) Set the secret.
        let mut incoming = Map::new();
        incoming.insert(field_name.to_string(), json!(cleartext));
        let outcome = values
            .write_for_entity_token(&token, &incoming)
            .await
            .unwrap_or_else(|e| panic!("initial write for {token}: {e}"));
        values
            .apply_secret_intents(entity_id, outcome.secret_intents)
            .await
            .expect("apply initial intents");

        // 2) Masked read shows `****` (secret never surfaced on the read path).
        let masked = values
            .read_for_entity_token(&token, entity_id, &Value::Null, false)
            .await
            .expect("masked read");
        let masked_map = masked.to_value_map();
        assert_eq!(
            masked_map.get(field_name),
            Some(&json!(SECRET_MASK)),
            "{token}: a set secret must read back as the mask"
        );

        // 3) Read-modify-write: a client PUTs the FULL masked object back
        //    unchanged — including the `****` mask for the secret field.
        let put_back = masked_map
            .as_object()
            .expect("masked map is an object")
            .clone();
        let outcome = values
            .write_for_entity_token(&token, &put_back)
            .await
            .unwrap_or_else(|e| panic!("round-trip write for {token}: {e}"));
        // The masked `****` for THIS field must emit no intent (keep existing).
        // (Other unset secret fields scoped to the same content type legitimately
        // emit Clear intents for their own keys; only this field is asserted.)
        assert!(
            outcome
                .secret_intents
                .iter()
                .all(|intent| intent.field_key != field_name),
            "{token}: a `****` mask must emit NO secret intent for {field_name} (keep existing)"
        );
        values
            .apply_secret_intents(entity_id, outcome.secret_intents)
            .await
            .expect("apply round-trip intents");

        // 4) Reveal must still return the ORIGINAL cleartext, not `****`/null.
        let revealed = values
            .reveal_secret(entity_id, field_name)
            .await
            .expect("reveal");
        assert_eq!(
            revealed,
            Some(cleartext),
            "{token}: round-tripping the mask must preserve the original cleartext"
        );
        let secret_id = state
            .repositories
            .secrets
            .get_usage_binding("inventory_custom_field", entity_id, field_name)
            .await?
            .expect("persisted custom-field usage")
            .secret_id;
        let table = match model {
            "site" => "dcim_site",
            "device" => "dcim_device",
            "virtualmachine" => "virtualization_virtualmachine",
            _ => panic!("unsupported native fixture model"),
        };
        let deleted = piping_backend::contexts::inventory::infrastructure::persistence::repositories::delete_guard::delete_inventory_record(&pool, table, entity_id).await?;
        assert_eq!(deleted, 1);
        assert_retired(&pool, secret_id).await?;
    }
    Ok(())
}

/// A blank value on a secret field is the legitimate delete path: it clears the
/// stored secret (distinct from the `****` keep-existing case in HIGH-1).
#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: -- --ignored --test-threads=1
async fn blank_secret_value_clears_existing() -> Result<(), Box<dyn std::error::Error + Send + Sync>>
{
    let state = build_test_app_state().await.expect("app state");
    let pool = build_test_db_pool().await.expect("db pool");
    let values = state.services.custom_field_values.clone();

    let content_type_id = lookup_content_type_id(&pool, "dcim", "device").await?;
    let field_name = unique_field_name("device_secret_high1_clear");
    let field_name = field_name.as_str();
    state
        .services
        .custom_field_definitions
        .create(secret_text_def(field_name, content_type_id))
        .await
        .expect("create def");

    let entity_id = Uuid::now_v7();
    secret_field_inventory_fixtures::seed_inventory_entity(&pool, "dcim.device", entity_id).await?;

    let mut set = Map::new();
    set.insert(field_name.to_string(), json!("to-be-cleared"));
    let outcome = values
        .write_for_entity_token("dcim.device", &set)
        .await
        .expect("set");
    values
        .apply_secret_intents(entity_id, outcome.secret_intents)
        .await
        .expect("apply set");
    let secret_id = state
        .repositories
        .secrets
        .get_usage_binding("inventory_custom_field", entity_id, field_name)
        .await?
        .expect("persisted custom-field usage")
        .secret_id;
    assert_eq!(
        values
            .reveal_secret(entity_id, field_name)
            .await
            .expect("reveal set"),
        Some("to-be-cleared".to_string())
    );

    // Blank clears.
    let mut blank = Map::new();
    blank.insert(field_name.to_string(), json!(""));
    let outcome = values
        .write_for_entity_token("dcim.device", &blank)
        .await
        .expect("clear");
    values
        .apply_secret_intents(entity_id, outcome.secret_intents)
        .await
        .expect("apply clear");
    assert_eq!(
        values
            .reveal_secret(entity_id, field_name)
            .await
            .expect("reveal cleared"),
        None,
        "a blank value must clear the secret (the delete path)"
    );
    assert_retired(&pool, secret_id).await?;
    Ok(())
}

async fn assert_retired(pool: &sqlx::PgPool, secret_id: Uuid) -> Result<(), sqlx::Error> {
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
        let remaining: i64 =
            sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE {column} = $1"))
                .bind(secret_id)
                .fetch_one(pool)
                .await?;
        assert_eq!(
            remaining, 0,
            "custom-field retirement leaves rows in {table}"
        );
    }
    Ok(())
}

async fn lookup_content_type_id(
    pool: &sqlx::PgPool,
    app_label: &str,
    model: &str,
) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM inventory_content_type WHERE app_label = $1 AND model = $2",
    )
    .bind(app_label)
    .bind(model)
    .fetch_one(pool)
    .await
    .map_err(|e| format!("content type {app_label}.{model}: {e}"))?)
}
