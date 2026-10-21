//! Integration tests for standardized sync mapping configuration listing.
//!
//! Requires PostgreSQL and Redis (e.g. `make run-up`). Run with:
//! `cargo test --test integration_sync_mapping_configurations -- --ignored`

mod common;

use std::sync::Arc;

use axum::{
    Extension,
    body::to_bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use piping_backend::api::middleware::auth::AuthContext;
use piping_backend::api::middleware::request_id::RequestId;
use piping_backend::api::rest::envelope_extractors::Query;
use piping_backend::api::rest::routes::mapping_specs::{
    ListSyncMappingConfigurationsQuery, list_sync_mapping_configurations,
};
use piping_backend::contexts::access_control::domain::{Action, ResourceType};
use piping_backend::contexts::shared::list_query::SortSpec;
use piping_backend::contexts::shared::request_context::{RequestAuth, RequestContext};
use piping_backend::contexts::sync::infrastructure::persistence::MappingConfigurationRepository;
use piping_backend::contexts::sync::infrastructure::persistence::repositories::MappingConfigurationListFilters;
use piping_backend::contexts::sync::infrastructure::persistence::repositories::mapping_configuration_repository::MappingConfigurationScope;

use common::{
    TEST_ADMIN_USERNAME, TEST_PROVIDER_ID, build_test_db_pool, build_test_redis, ensure_test_admin,
    ensure_test_provider, ensure_test_secret,
};

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_sync_mapping_configurations -- --ignored
async fn sync_mapping_list_honors_logical_id_sort_filter_and_rejects_bad_sort()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_state_without_migrations()
        .await
        .expect("build app state");
    seed_mapping_configuration_rows(&state.db_pool)
        .await
        .expect("seed mapping configurations");
    let provider_id = Uuid::parse_str(TEST_PROVIDER_ID).expect("provider uuid");

    let rows = state
        .repositories
        .sync_mapping_configurations
        .list_for_provider_logical_id(
            MappingConfigurationScope {
                provider_id: Some(provider_id),
                logical_id: Some(logical_a()?),
            },
            &MappingConfigurationListFilters::default(),
            Some(&SortSpec {
                key: "versions".to_string(),
                descending: false,
            }),
            20,
            0,
        )
        .await
        .expect("repository list_for_provider_logical_id");
    assert_eq!(rows.len(), 2);

    let total = state
        .repositories
        .sync_mapping_configurations
        .count_for_provider_logical_id(
            provider_id,
            logical_a()?,
            &MappingConfigurationListFilters::default(),
        )
        .await
        .expect("repository count_for_provider_logical_id");
    assert_eq!(total, 2);

    state
        .repositories
        .sync_mapping_configurations
        .get_document(logical_a()?, 1)
        .await
        .expect("repository get_document");
    state
        .repositories
        .sync_mapping_configurations
        .list_versions_for_logical_id(logical_a()?)
        .await
        .expect("repository list_versions_for_logical_id");

    let auth = build_test_auth_context(&state.db_pool).await?;
    let request_context = RequestContext::new(
        auth.user_id,
        auth.auth_method.clone(),
        None,
        "test-sync-mapping-configurations".to_string(),
        RequestAuth {
            oidc_groups: auth.oidc_groups.clone(),
            api_token_scopes: auth.api_token_scopes.clone(),
            api_token_scope_resource_override: auth.api_token_scope_resource_override.clone(),
        },
    );
    state
        .services
        .policy_evaluation
        .ensure_access(
            &request_context,
            Action::Read,
            ResourceType::Inventory,
            None,
        )
        .await
        .expect("policy evaluation should allow sync mapping list");
    for row in &rows {
        let document = state
            .repositories
            .sync_mapping_configurations
            .get_document(row.logical_id, row.version_id)
            .await
            .expect("repository get_document during handler loop")
            .expect("handler loop document exists");
        let versions = state
            .repositories
            .sync_mapping_configurations
            .list_versions_for_logical_id(row.logical_id)
            .await
            .expect("repository list_versions during handler loop");
        let is_latest = versions
            .iter()
            .map(|configuration| configuration.version_id)
            .max()
            .map(|latest| latest == row.version_id)
            .unwrap_or(false);
        assert_eq!(document.reference.version_id, row.version_id);
        assert_eq!(document.reference.logical_id, row.logical_id);
        assert_eq!(
            is_latest,
            row.version_id
                == rows
                    .iter()
                    .filter(|candidate| candidate.logical_id == row.logical_id)
                    .map(|candidate| candidate.version_id)
                    .max()
                    .unwrap_or(row.version_id)
        );
    }

    let auth_context = Some(Extension(auth));

    let asc = get_json(
        &state,
        auth_context.clone(),
        ListSyncMappingConfigurationsQuery {
            provider_id: Some(TEST_PROVIDER_ID.to_string()),
            logical_id: Some(logical_a()?.to_string()),
            search: None,
            direction: None,
            sort: Some("versions".to_string()),
            order: Some("asc".to_string()),
            limit: 20,
            offset: 0,
        },
    )
    .await?;
    let desc = get_json(
        &state,
        auth_context.clone(),
        ListSyncMappingConfigurationsQuery {
            provider_id: Some(TEST_PROVIDER_ID.to_string()),
            logical_id: Some(logical_a()?.to_string()),
            search: None,
            direction: None,
            sort: Some("versions".to_string()),
            order: Some("desc".to_string()),
            limit: 20,
            offset: 0,
        },
    )
    .await?;
    let pull = get_json(
        &state,
        auth_context.clone(),
        ListSyncMappingConfigurationsQuery {
            provider_id: Some(TEST_PROVIDER_ID.to_string()),
            logical_id: Some(logical_a()?.to_string()),
            search: None,
            direction: Some("pull".to_string()),
            sort: None,
            order: None,
            limit: 20,
            offset: 0,
        },
    )
    .await?;
    let push = get_json(
        &state,
        auth_context.clone(),
        ListSyncMappingConfigurationsQuery {
            provider_id: Some(TEST_PROVIDER_ID.to_string()),
            logical_id: Some(logical_a()?.to_string()),
            search: None,
            direction: Some("push".to_string()),
            sort: None,
            order: None,
            limit: 20,
            offset: 0,
        },
    )
    .await?;

    assert_eq!(extract_versions(&asc), vec![1, 2]);
    assert_eq!(extract_versions(&desc), vec![2, 1]);
    assert_eq!(asc.get("total").and_then(Value::as_u64), Some(2));
    assert_eq!(desc.get("total").and_then(Value::as_u64), Some(2));
    assert_eq!(pull.get("total").and_then(Value::as_u64), Some(1));
    assert_eq!(push.get("total").and_then(Value::as_u64), Some(1));
    assert_eq!(extract_versions(&pull), vec![1]);
    assert_eq!(extract_versions(&push), vec![2]);

    let (status, body) = invoke_list(
        &state,
        auth_context,
        ListSyncMappingConfigurationsQuery {
            provider_id: Some(TEST_PROVIDER_ID.to_string()),
            logical_id: Some(logical_a()?.to_string()),
            search: None,
            direction: None,
            sort: Some("__bad__".to_string()),
            order: Some("asc".to_string()),
            limit: 20,
            offset: 0,
        },
    )
    .await
    .expect("bad sort request");
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let payload: Value = serde_json::from_slice(&body).expect("bad sort json");
    assert_eq!(
        payload.get("message").and_then(Value::as_str),
        Some("Invalid sort key \"__bad__\"")
    );
    Ok(())
}

async fn get_json(
    state: &piping_backend::app::AppState,
    auth_context: Option<Extension<AuthContext>>,
    query: ListSyncMappingConfigurationsQuery,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    let (status, body) = invoke_list(state, auth_context, query)
        .await
        .map_err(|e| format!("request: {e}"))?;
    let body_text = String::from_utf8_lossy(&body);
    assert_eq!(
        status,
        StatusCode::OK,
        "unexpected status for sync mapping list: {body_text}"
    );
    Ok(serde_json::from_slice(&body).map_err(|e| format!("json body: {e}"))?)
}

async fn invoke_list(
    state: &piping_backend::app::AppState,
    auth_context: Option<Extension<AuthContext>>,
    query: ListSyncMappingConfigurationsQuery,
) -> Result<(StatusCode, Vec<u8>), Box<dyn std::error::Error + Send + Sync>> {
    let response = list_sync_mapping_configurations(
        State(state.clone()),
        Extension(RequestId("test-sync-mapping-configurations".to_string())),
        auth_context,
        HeaderMap::new(),
        Query(query),
    )
    .await
    .into_response();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await?;
    Ok((status, body.to_vec()))
}

fn extract_versions(payload: &Value) -> Vec<i64> {
    payload
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("version_id").and_then(Value::as_i64))
        .collect()
}

async fn seed_mapping_configuration_rows(
    pool: &PgPool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let provider_id = Uuid::parse_str(TEST_PROVIDER_ID)?;
    let logical_a = logical_a()?;
    let logical_b = logical_b()?;

    sqlx::query(
        r#"
        INSERT INTO mapping_configurations (
            logical_id,
            version_id,
            name,
            description,
            source_provider_id,
            target_provider_id,
            status,
            direction,
            conflict_mode,
            created_by
        ) VALUES
            ($1, 1, 'Alpha mapping', 'smoke row v1', $3, $3, 'draft', 'pull', 'netbox_wins', NULL),
            ($1, 2, 'Beta mapping', 'smoke row v2', $3, $3, 'active', 'push', 'field_level', NULL),
            ($2, 1, 'Gamma mapping', 'smoke row v3', $3, $3, 'active', 'bidirectional', 'piping_wins', NULL)
        "#,
    )
    .bind(logical_a)
    .bind(logical_b)
    .bind(provider_id)
    .execute(pool)
    .await?;

    Ok(())
}

fn logical_a() -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
    Ok(Uuid::parse_str("11111111-1111-4111-8111-111111111111")
        .map_err(|e| format!("logical_a uuid: {e}"))?)
}

fn logical_b() -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
    Ok(Uuid::parse_str("22222222-2222-4222-8222-222222222222")
        .map_err(|e| format!("logical_b uuid: {e}"))?)
}

async fn build_test_auth_context(
    pool: &PgPool,
) -> Result<AuthContext, Box<dyn std::error::Error + Send + Sync>> {
    let user_id = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id
        FROM identity_user
        WHERE username = $1
        "#,
    )
    .bind(TEST_ADMIN_USERNAME)
    .fetch_one(pool)
    .await
    .map_err(|e| format!("load test admin id: {e}"))?;

    Ok(AuthContext {
        user_id: Some(user_id),
        auth_method: "session".to_string(),
        oidc_groups: Vec::new(),
        api_token_id: None,
        api_token_scopes: Vec::new(),
        api_token_scope_resource_override: None,
    })
}

async fn build_test_state_without_migrations()
-> Result<piping_backend::app::AppState, Box<dyn std::error::Error + Send + Sync>> {
    let mut settings = piping_backend::config::load_settings()?;
    if settings.database.url.trim().is_empty() {
        settings.database.url = common::database_url();
    }
    if settings.redis.url.trim().is_empty() {
        settings.redis.url = common::redis_url();
    }
    settings.secrets.cipher.key_material_ref =
        "inline:MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=".to_string();
    settings.security.allow_weak_dev_secrets = true;
    let password_pepper = settings.auth.password_pepper.clone();
    let run_mode = settings.run_mode;
    let settings = Arc::new(settings);
    settings.validate_for_mode(run_mode)?;

    let db_pool = build_test_db_pool().await?;
    ensure_identity_group_membership_schema(&db_pool).await?;
    ensure_mapping_configuration_schema(&db_pool).await?;
    sqlx::query(
        r#"
        DELETE FROM mapping_configurations
        WHERE logical_id IN ($1, $2)
        "#,
    )
    .bind(logical_a()?)
    .bind(logical_b()?)
    .execute(&db_pool)
    .await?;
    ensure_test_admin(&db_pool, &password_pepper).await?;
    ensure_test_provider(&db_pool).await?;
    ensure_test_secret(&db_pool).await?;
    let redis = build_test_redis().await?;
    let state = piping_backend::app::AppState::build(settings, db_pool, redis).await?;
    Ok(state)
}

async fn ensure_mapping_configuration_schema(
    pool: &PgPool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let exists: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM information_schema.tables
            WHERE table_schema = 'public' AND table_name = 'mapping_configurations'
        )
        "#,
    )
    .fetch_one(pool)
    .await?;

    if exists {
        return Ok(());
    }

    // Schema is provisioned by the consolidated baseline; apply it if a bare
    // pool slipped through without migrations.
    piping_backend::storage::database::migrations::run_migrations(pool).await?;

    Ok(())
}

async fn ensure_identity_group_membership_schema(
    pool: &PgPool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let exists: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM information_schema.tables
            WHERE table_schema = 'public' AND table_name = 'identity_group_membership'
        )
        "#,
    )
    .fetch_one(pool)
    .await?;

    if exists {
        return Ok(());
    }

    // Schema is provisioned by the consolidated baseline; apply it if a bare
    // pool slipped through without migrations.
    piping_backend::storage::database::migrations::run_migrations(pool).await?;

    Ok(())
}
