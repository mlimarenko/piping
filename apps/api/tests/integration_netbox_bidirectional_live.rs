//! Live per-record application of inbound NetBox webhooks for mapped inventory
//! entities (PIP-321 / PIP-323).
//!
//! A signed inbound webhook is not applied from its payload: the event is
//! matched to the active bidirectional local-inventory mapping, the record is
//! re-read from NetBox by id, and only that single record is written through the
//! same planning/commit path as a pull. Deletions go through the delete guard
//! and binding removal.
//!
//! Requires PostgreSQL and Redis. Run with:
//! `cargo test --test integration_netbox_bidirectional_live -- --ignored --test-threads=1`

mod common;
#[path = "common/isolated_app.rs"]
mod isolated_app;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::body::Body;
use axum::extract::{Path, State as AxumState};
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::Utc;
use openssl::{hash::MessageDigest, pkey::PKey, sign::Signer};
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::sync::Mutex;
use tower::ServiceExt;
use uuid::Uuid;

use piping_backend::app::AppState;
use piping_backend::contexts::sync::application::{
    EntityMappingDraft, FieldMappingDraft, MappingConfigurationDraft,
    validate_mapping_configuration,
};
use piping_backend::contexts::sync::domain::{
    ConflictMode, EntityMapping, FieldMapping, InventoryEntity, MappingConfigDocument,
    MappingConfiguration, MappingConfigurationStatus, MappingDirection, MappingRef, MappingTarget,
    SourceEntity, SourceIdentity, SourceRecord, SourceSnapshot, live_entity_mapping,
};
use piping_backend::contexts::sync::infrastructure::persistence::MappingConfigurationRepository;
use piping_backend::contexts::sync::infrastructure::persistence::local_inventory_target::assess_live_preparation;

use common::{request, unique_suffix};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// One loopback token for the mock; an obvious placeholder, never a real secret.
const MOCK_TOKEN: &str = "synthetic-token";

// ---------------------------------------------------------------------------
// Synthetic NetBox endpoint
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct MockNetboxState {
    sites: Arc<Mutex<BTreeMap<i64, Value>>>,
    tenants: Arc<Mutex<BTreeMap<i64, Value>>>,
    patch_count: Arc<AtomicUsize>,
    delete_count: Arc<AtomicUsize>,
}

struct MockNetbox {
    base_url: String,
    state: MockNetboxState,
}

impl MockNetbox {
    async fn replace_site(&self, record: Value) {
        let id = record_id(&record);
        self.state.sites.lock().await.insert(id, record);
    }

    async fn replace_tenant(&self, record: Value) {
        let id = record_id(&record);
        self.state.tenants.lock().await.insert(id, record);
    }

    fn write_count(&self) -> usize {
        self.state.patch_count.load(Ordering::SeqCst)
            + self.state.delete_count.load(Ordering::SeqCst)
    }
}

fn record_id(record: &Value) -> i64 {
    record
        .get("id")
        .and_then(Value::as_i64)
        .expect("mock record carries a numeric id")
}

async fn spawn_mock_netbox(site: Value, tenant: Value) -> TestResult<MockNetbox> {
    let mut sites = BTreeMap::new();
    sites.insert(record_id(&site), site);
    let mut tenants = BTreeMap::new();
    tenants.insert(record_id(&tenant), tenant);
    let state = MockNetboxState {
        sites: Arc::new(Mutex::new(sites)),
        tenants: Arc::new(Mutex::new(tenants)),
        patch_count: Arc::new(AtomicUsize::new(0)),
        delete_count: Arc::new(AtomicUsize::new(0)),
    };
    let app = Router::new()
        .route("/api/schema/", get(mock_schema))
        .route("/dcim/sites/", get(mock_list_sites))
        .route(
            "/dcim/sites/{id}/",
            get(mock_get_site)
                .patch(mock_patch_site)
                .delete(mock_delete_site),
        )
        .route("/api/dcim/sites/", get(mock_list_sites))
        .route(
            "/api/dcim/sites/{id}/",
            get(mock_get_site)
                .patch(mock_patch_site)
                .delete(mock_delete_site),
        )
        .route("/tenancy/tenants/", get(mock_list_tenants))
        .route(
            "/tenancy/tenants/{id}/",
            get(mock_get_tenant)
                .patch(mock_patch_tenant)
                .delete(mock_delete_tenant),
        )
        .route("/api/tenancy/tenants/", get(mock_list_tenants))
        .route(
            "/api/tenancy/tenants/{id}/",
            get(mock_get_tenant)
                .patch(mock_patch_tenant)
                .delete(mock_delete_tenant),
        )
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        #[allow(
            clippy::expect_used,
            reason = "detached background task backing a test-only mock server; a serve failure has no caller left to propagate to, so panicking the task is the correct failure mode"
        )]
        axum::serve(listener, app).await.expect("serve mock netbox");
    });

    Ok(MockNetbox {
        base_url: format!("http://{addr}"),
        state,
    })
}

async fn mock_schema() -> impl IntoResponse {
    Json(json!({
        "openapi": "3.0.0",
        "paths": {
            "/api/dcim/sites/": {
                "get": {
                    "parameters": [
                        { "name": "limit", "in": "query", "schema": { "type": "integer" } },
                        { "name": "offset", "in": "query", "schema": { "type": "integer" } }
                    ],
                    "responses": {
                        "200": { "content": { "application/json": { "schema": { "$ref": "#/components/schemas/PaginatedSiteList" } } } }
                    }
                }
            },
            "/api/dcim/sites/{id}/": {
                "get": {
                    "responses": {
                        "200": { "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Site" } } } }
                    }
                },
                "patch": {
                    "requestBody": {
                        "content": { "application/json": { "schema": { "$ref": "#/components/schemas/SiteRequest" } } }
                    },
                    "responses": {
                        "200": { "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Site" } } } }
                    }
                },
                "delete": { "responses": { "204": { "description": "No content" } } }
            },
            "/api/tenancy/tenants/": {
                "get": {
                    "parameters": [
                        { "name": "limit", "in": "query", "schema": { "type": "integer" } },
                        { "name": "offset", "in": "query", "schema": { "type": "integer" } }
                    ],
                    "responses": {
                        "200": { "content": { "application/json": { "schema": { "$ref": "#/components/schemas/PaginatedTenantList" } } } }
                    }
                }
            },
            "/api/tenancy/tenants/{id}/": {
                "get": {
                    "responses": {
                        "200": { "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Tenant" } } } }
                    }
                },
                "patch": {
                    "requestBody": {
                        "content": { "application/json": { "schema": { "$ref": "#/components/schemas/TenantRequest" } } }
                    },
                    "responses": {
                        "200": { "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Tenant" } } } }
                    }
                },
                "delete": { "responses": { "204": { "description": "No content" } } }
            }
        },
        "components": {
            "schemas": {
                "Site": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "integer" },
                        "name": { "type": "string" },
                        "slug": { "type": "string" }
                    }
                },
                "SiteRequest": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "slug": { "type": "string" }
                    }
                },
                "PaginatedSiteList": {
                    "type": "object",
                    "properties": {
                        "count": { "type": "integer" },
                        "results": { "type": "array", "items": { "$ref": "#/components/schemas/Site" } }
                    }
                },
                "Tenant": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "integer" },
                        "name": { "type": "string" },
                        "slug": { "type": "string" }
                    }
                },
                "TenantRequest": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "slug": { "type": "string" }
                    }
                },
                "PaginatedTenantList": {
                    "type": "object",
                    "properties": {
                        "count": { "type": "integer" },
                        "results": { "type": "array", "items": { "$ref": "#/components/schemas/Tenant" } }
                    }
                }
            }
        }
    }))
}

fn paginated(values: &BTreeMap<i64, Value>) -> Json<Value> {
    let results: Vec<Value> = values.values().cloned().collect();
    Json(json!({
        "count": results.len(),
        "results": results,
        "next": null,
        "previous": null
    }))
}

async fn mock_list_sites(AxumState(state): AxumState<MockNetboxState>) -> impl IntoResponse {
    paginated(&*state.sites.lock().await)
}

async fn mock_list_tenants(AxumState(state): AxumState<MockNetboxState>) -> impl IntoResponse {
    paginated(&*state.tenants.lock().await)
}

async fn mock_get_site(
    AxumState(state): AxumState<MockNetboxState>,
    Path(id): Path<String>,
) -> Response {
    let id: i64 = id.parse().unwrap_or_default();
    match state.sites.lock().await.get(&id) {
        Some(record) => Json(record.clone()).into_response(),
        None => not_found("site"),
    }
}

async fn mock_get_tenant(
    AxumState(state): AxumState<MockNetboxState>,
    Path(id): Path<String>,
) -> Response {
    let id: i64 = id.parse().unwrap_or_default();
    match state.tenants.lock().await.get(&id) {
        Some(record) => Json(record.clone()).into_response(),
        None => not_found("tenant"),
    }
}

async fn mock_patch_site(
    AxumState(state): AxumState<MockNetboxState>,
    Path(id): Path<String>,
    Json(payload): Json<Value>,
) -> Response {
    state.patch_count.fetch_add(1, Ordering::SeqCst);
    let target: i64 = id.parse().unwrap_or_default();
    let mut records = state.sites.lock().await;
    patched(records.get_mut(&target), &payload, "site")
}

async fn mock_patch_tenant(
    AxumState(state): AxumState<MockNetboxState>,
    Path(id): Path<String>,
    Json(payload): Json<Value>,
) -> Response {
    state.patch_count.fetch_add(1, Ordering::SeqCst);
    let target: i64 = id.parse().unwrap_or_default();
    let mut records = state.tenants.lock().await;
    patched(records.get_mut(&target), &payload, "tenant")
}

fn patched(record: Option<&mut Value>, payload: &Value, label: &str) -> Response {
    let Some(record) = record else {
        return not_found(label);
    };
    if let (Some(base), Some(incoming)) = (record.as_object_mut(), payload.as_object()) {
        for (key, value) in incoming {
            base.insert(key.clone(), value.clone());
        }
    }
    Json(record.clone()).into_response()
}

async fn mock_delete_site(
    AxumState(state): AxumState<MockNetboxState>,
    Path(id): Path<String>,
) -> Response {
    state.delete_count.fetch_add(1, Ordering::SeqCst);
    let id: i64 = id.parse().unwrap_or_default();
    match state.sites.lock().await.remove(&id) {
        Some(_) => StatusCode::NO_CONTENT.into_response(),
        None => not_found("site"),
    }
}

async fn mock_delete_tenant(
    AxumState(state): AxumState<MockNetboxState>,
    Path(id): Path<String>,
) -> Response {
    state.delete_count.fetch_add(1, Ordering::SeqCst);
    let id: i64 = id.parse().unwrap_or_default();
    match state.tenants.lock().await.remove(&id) {
        Some(_) => StatusCode::NO_CONTENT.into_response(),
        None => not_found("tenant"),
    }
}

fn not_found(label: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({ "detail": format!("{label} not found") })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// NetBox records, provider and mapping configuration
// ---------------------------------------------------------------------------

/// A complete-enough NetBox site serializer payload: required catalogue fields
/// are present with their empty encodings, nullable relations are explicit null.
fn site_record(id: i64, name: &str, slug: &str) -> Value {
    json!({
        "id": id,
        "name": name,
        "slug": slug,
        "status": { "value": "active", "label": "Active" },
        "facility": "",
        "description": "",
        "physical_address": "",
        "shipping_address": "",
        "comments": "",
        "tenant": null,
        "time_zone": null,
        "tags": []
    })
}

fn tenant_record(id: i64, name: &str, slug: &str) -> Value {
    json!({
        "id": id,
        "name": name,
        "slug": slug,
        "description": "",
        "comments": "",
        "group": null,
        "tags": []
    })
}

async fn create_provider(
    app: &Router,
    cookie: &str,
    base_url: &str,
    secret: &str,
) -> TestResult<Uuid> {
    let (status, body) = request(
        app,
        "POST",
        "/api/v1/providers",
        Some(
            &json!({
                "name": format!("netbox-live-{}", unique_suffix()),
                "provider_type": "netbox",
                "config": {
                    "base_url": base_url,
                    "api_token": MOCK_TOKEN,
                    "page_size": 100,
                    "sync_mode": "bidirectional",
                    "webhook_secret": secret
                },
                "bootstrap": false
            })
            .to_string(),
        ),
        Some(cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::CREATED, "provider creation");
    let body: Value = serde_json::from_slice(&body)?;
    Ok(Uuid::parse_str(
        body.get("id")
            .and_then(Value::as_str)
            .ok_or("provider id missing")?,
    )?)
}

/// One bidirectional local-inventory mapping covering Site and Tenant with a
/// full identity field mapping for every catalogue field of both entities.
fn mapping_document(provider_id: Uuid, conflict_mode: ConflictMode) -> MappingConfigDocument {
    let logical_id = Uuid::now_v7();
    let mut entity_mappings = Vec::new();
    let mut field_mappings = Vec::new();
    for entity in [InventoryEntity::Site, InventoryEntity::Tenant] {
        let mapping_id = Uuid::now_v7();
        entity_mappings.push(EntityMapping {
            id: mapping_id,
            mapping_config_logical_id: logical_id,
            mapping_config_version_id: 1,
            source_entity: entity.source_key().to_string(),
            target_entity: entity.target_key(),
            enabled: true,
            created_at: Utc::now(),
        });
        for field in entity.field_iter() {
            let name = field.name();
            field_mappings.push(FieldMapping {
                id: Uuid::now_v7(),
                entity_mapping_id: mapping_id,
                source_field: name.to_string(),
                target_field: name.to_string(),
                forward_transform_json: json!({ "op": "identity", "args": [{ "field": name }] }),
                reverse_transform_json: None,
                round_trip_critical: false,
                created_at: Utc::now(),
            });
        }
    }

    MappingConfigDocument {
        absence_policy: Default::default(),
        reference: MappingRef::new(logical_id, 1),
        name: format!("netbox-live-{}", unique_suffix()),
        description: None,
        source_provider_id: provider_id,
        target: MappingTarget::LocalInventory,
        status: MappingConfigurationStatus::Active,
        direction: MappingDirection::Bidirectional,
        conflict_mode,
        created_by: None,
        created_at: Utc::now(),
        retired_at: None,
        entity_mappings,
        field_mappings,
    }
}

fn assert_no_validation_errors(document: &MappingConfigDocument) {
    let draft = MappingConfigurationDraft {
        absence_policy: Default::default(),
        name: document.name.clone(),
        description: document.description.clone(),
        source_provider_id: document.source_provider_id,
        target: document.target.clone(),
        direction: document.direction.as_str().to_string(),
        conflict_mode: document.conflict_mode.as_str().to_string(),
        entity_mappings: document
            .entity_mappings
            .iter()
            .map(|mapping| EntityMappingDraft {
                source_entity: mapping.source_entity.clone(),
                target_entity: mapping.target_entity.clone(),
                enabled: mapping.enabled,
                field_mappings: document
                    .field_mappings
                    .iter()
                    .filter(|field| field.entity_mapping_id == mapping.id)
                    .map(|field| FieldMappingDraft {
                        source_field: field.source_field.clone(),
                        target_field: field.target_field.clone(),
                        forward_transform: field.forward_transform_json.clone(),
                        reverse_transform: field.reverse_transform_json.clone(),
                        round_trip_critical: field.round_trip_critical,
                    })
                    .collect(),
            })
            .collect(),
    };
    let errors: Vec<_> = validate_mapping_configuration(&draft)
        .into_iter()
        .filter(|error| error.is_error())
        .collect();
    assert!(errors.is_empty(), "mapping validation errors: {errors:?}");
}

async fn seed_mapping(state: &AppState, document: &MappingConfigDocument) -> TestResult {
    let configuration = MappingConfiguration {
        absence_policy: Default::default(),
        logical_id: document.reference.logical_id,
        version_id: document.reference.version_id,
        name: document.name.clone(),
        description: document.description.clone(),
        source_provider_id: document.source_provider_id,
        target: document.target.clone(),
        status: document.status,
        direction: document.direction,
        conflict_mode: document.conflict_mode,
        created_by: document.created_by,
        created_at: document.created_at,
        retired_at: document.retired_at,
    };
    state
        .repositories
        .sync_mapping_configurations
        .insert_new_version(
            &configuration,
            &document.entity_mappings,
            &document.field_mappings,
        )
        .await?;
    Ok(())
}

fn source_record(
    provider_id: Uuid,
    entity: InventoryEntity,
    value: &Value,
) -> TestResult<SourceRecord> {
    let external_id = record_id(value).to_string();
    Ok(SourceRecord::new(
        SourceIdentity::new(
            provider_id,
            SourceEntity::new(entity.source_key())?,
            external_id,
        )?,
        value
            .as_object()
            .cloned()
            .ok_or("mock record is not a JSON object")?,
    ))
}

fn snapshot(provider_id: Uuid, site: &Value, tenant: &Value) -> TestResult<SourceSnapshot> {
    Ok(SourceSnapshot {
        coverage: None,
        records: vec![
            source_record(provider_id, InventoryEntity::Site, site)?,
            source_record(provider_id, InventoryEntity::Tenant, tenant)?,
        ],
    })
}

// ---------------------------------------------------------------------------
// Signed webhooks
// ---------------------------------------------------------------------------

fn sign_payload(payload: &[u8], secret: &str) -> TestResult<String> {
    let key = PKey::hmac(secret.as_bytes())?;
    let mut signer = Signer::new(MessageDigest::sha512(), &key)?;
    signer.update(payload)?;
    let signature = signer.sign_to_vec()?;
    Ok(signature
        .iter()
        .map(|byte| format!("{:02x}", byte))
        .collect())
}

async fn send_signed_webhook(
    app: &Router,
    provider_id: Uuid,
    payload: &Value,
    secret: &str,
) -> TestResult<(StatusCode, Value)> {
    let body = payload.to_string();
    let signature = sign_payload(body.as_bytes(), secret)?;
    let request = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/webhooks/netbox/{provider_id}"))
        .header("content-type", "application/json")
        .header("x-hook-signature", signature)
        .body(Body::from(body))?;
    let response = app.clone().oneshot(request).await?;
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
    Ok((status, serde_json::from_slice(&bytes)?))
}

fn webhook_payload(event: &str, model: &str, id: i64, request_id: &str) -> Value {
    json!({
        "event": event,
        "timestamp": "2026-11-08T12:00:00Z",
        "model": model,
        "request_id": request_id,
        "data": { "id": id },
        "snapshots": {}
    })
}

// ---------------------------------------------------------------------------
// Database observations
// ---------------------------------------------------------------------------

async fn site_row(pool: &PgPool, name: &str) -> TestResult<Option<(Uuid, String)>> {
    Ok(
        sqlx::query_as("SELECT id, name FROM dcim_site WHERE name = $1")
            .bind(name)
            .fetch_optional(pool)
            .await?,
    )
}

async fn tenant_row(pool: &PgPool, name: &str) -> TestResult<Option<(Uuid, String)>> {
    Ok(
        sqlx::query_as("SELECT id, name FROM tenancy_tenant WHERE name = $1")
            .bind(name)
            .fetch_optional(pool)
            .await?,
    )
}

async fn binding_local_id(
    pool: &PgPool,
    provider_id: Uuid,
    source_entity: &str,
    external_id: &str,
    target_entity: &str,
) -> TestResult<Option<Uuid>> {
    Ok(sqlx::query_scalar(
        "SELECT local_id FROM sync_inventory_binding \
         WHERE source_provider_id = $1 AND source_entity = $2 \
           AND external_id = $3 AND target_entity = $4",
    )
    .bind(provider_id)
    .bind(source_entity)
    .bind(external_id)
    .bind(target_entity)
    .fetch_optional(pool)
    .await?)
}

async fn binding_count(pool: &PgPool, provider_id: Uuid) -> TestResult<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM sync_inventory_binding WHERE source_provider_id = $1",
    )
    .bind(provider_id)
    .fetch_one(pool)
    .await?)
}

async fn latest_journal(pool: &PgPool, provider_id: Uuid) -> TestResult<(String, Option<String>)> {
    Ok(sqlx::query_as(
        "SELECT status, last_error FROM sync_webhook_event \
         WHERE source_id = $1 ORDER BY received_at DESC, id DESC LIMIT 1",
    )
    .bind(provider_id)
    .fetch_one(pool)
    .await?)
}

async fn runtime_state(pool: &PgPool, provider_id: Uuid) -> TestResult<(String, Option<String>)> {
    Ok(sqlx::query_as(
        "SELECT operational_status, last_error_code FROM sync_source_runtime WHERE source_id = $1",
    )
    .bind(provider_id)
    .fetch_one(pool)
    .await?)
}

async fn create_local_site(app: &Router, cookie: &str, name: &str) -> TestResult<Uuid> {
    let (status, body) = request(
        app,
        "POST",
        "/api/v1/inventory/sites",
        Some(&json!({ "name": name, "status": "active" }).to_string()),
        Some(cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::CREATED, "bystander site creation");
    let body: Value = serde_json::from_slice(&body)?;
    Ok(Uuid::parse_str(
        body.get("id")
            .and_then(Value::as_str)
            .ok_or("site id missing")?,
    )?)
}

fn assert_applied(body: &Value, what: &str) {
    assert_eq!(
        body.get("status").and_then(Value::as_str),
        Some("applied"),
        "expected applied for {what}: {body}"
    );
}

async fn completed_jobs(pool: &PgPool, provider: Uuid, mapping: &MappingRef) -> TestResult<i64> {
    Ok(sqlx::query_scalar("SELECT count(*) FROM sync_job WHERE provider_id=$1 AND mapping_version=$2 AND status='completed' AND finished_at IS NOT NULL")
        .bind(provider).bind(mapping.to_wire()).fetch_one(pool).await?)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn bidirectional_webhooks_apply_update_and_delete_site_and_tenant() -> TestResult {
    let harness = isolated_app::harness("pip323_netbox_live", 7).await?;
    let state = harness.state.clone();
    let pool = state.db_pool.clone();
    let app = harness.app.clone();
    let cookie = harness.cookie.clone();

    let suffix = unique_suffix();
    let site_name = format!("live-site-{suffix}");
    let tenant_name = format!("live-tenant-{suffix}");
    let mock = spawn_mock_netbox(
        site_record(101, &site_name, &site_name),
        tenant_record(202, &tenant_name, &tenant_name),
    )
    .await?;
    let secret = format!("live-webhook-secret-{suffix}");
    let provider_id = create_provider(&app, &cookie, &mock.base_url, &secret).await?;

    // The mapping document must pass the mapping validator, resolve both models
    // to live entities, and prepare a live batch for the served records.
    let document = mapping_document(provider_id, ConflictMode::NetboxWins);
    assert_no_validation_errors(&document);
    assert_eq!(
        live_entity_mapping(&document, "dcim.site").map(|(entity, _)| entity),
        Some(InventoryEntity::Site)
    );
    assert_eq!(
        live_entity_mapping(&document, "tenancy.tenant").map(|(entity, _)| entity),
        Some(InventoryEntity::Tenant)
    );
    let prepared = assess_live_preparation(
        &document,
        &snapshot(
            provider_id,
            &site_record(101, &site_name, &site_name),
            &tenant_record(202, &tenant_name, &tenant_name),
        )?,
    );
    assert!(
        prepared.findings.is_empty(),
        "live preparation findings: {:?}",
        prepared.findings
    );
    assert!(
        prepared.batch.is_some(),
        "live record must be write-eligible"
    );
    seed_mapping(&state, &document).await?;

    let stored = state
        .repositories
        .sync_mapping_configurations
        .get_document(document.reference.logical_id, document.reference.version_id)
        .await?
        .expect("stored mapping document");
    assert_eq!(
        live_entity_mapping(&stored, "dcim.site").map(|(entity, _)| entity),
        Some(InventoryEntity::Site)
    );
    assert_eq!(
        live_entity_mapping(&stored, "tenancy.tenant").map(|(entity, _)| entity),
        Some(InventoryEntity::Tenant)
    );

    // created: both mapped entities are applied from the NetBox record, not the
    // webhook payload.
    let (status, body) = send_signed_webhook(
        &app,
        provider_id,
        &webhook_payload("created", "dcim.site", 101, "live-site-created"),
        &secret,
    )
    .await?;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_applied(&body, "site created");
    assert_eq!(
        completed_jobs(&pool, provider_id, &document.reference).await?,
        1
    );
    let (site_local, site_stored_name) = site_row(&pool, &site_name).await?.expect("site row");
    assert_eq!(site_stored_name, site_name);
    assert_eq!(
        binding_local_id(
            &pool,
            provider_id,
            "dcim.sites",
            "101",
            "piping.inventory.site"
        )
        .await?,
        Some(site_local)
    );

    let (status, body) = send_signed_webhook(
        &app,
        provider_id,
        &webhook_payload("created", "tenancy.tenant", 202, "live-tenant-created"),
        &secret,
    )
    .await?;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_applied(&body, "tenant created");
    assert_eq!(
        completed_jobs(&pool, provider_id, &document.reference).await?,
        2
    );
    let (tenant_local, tenant_stored_name) =
        tenant_row(&pool, &tenant_name).await?.expect("tenant row");
    assert_eq!(tenant_stored_name, tenant_name);
    assert_eq!(
        binding_local_id(
            &pool,
            provider_id,
            "tenancy.tenants",
            "202",
            "piping.inventory.tenant"
        )
        .await?,
        Some(tenant_local)
    );

    let repeated = webhook_payload("created", "tenancy.tenant", 202, "live-tenant-created");
    let (status, body) = send_signed_webhook(&app, provider_id, &repeated, &secret).await?;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(body["status"], "deduplicated");
    assert_eq!(binding_count(&pool, provider_id).await?, 2);

    let related_name = format!("related-create-{suffix}");
    let mut related = site_record(102, &related_name, &related_name);
    related["tenant"] = json!({"id": 202});
    mock.replace_site(related).await;
    let (status, body) = send_signed_webhook(
        &app,
        provider_id,
        &webhook_payload("created", "dcim.site", 102, "related-create"),
        &secret,
    )
    .await?;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_applied(&body, "related site created");
    let (related_local, _) = site_row(&pool, &related_name).await?.expect("related site");
    let related_tenant: Option<Uuid> =
        sqlx::query_scalar("SELECT tenant_id FROM dcim_site WHERE id=$1")
            .bind(related_local)
            .fetch_one(&pool)
            .await?;
    assert_eq!(related_tenant, Some(tenant_local));
    assert_eq!(
        binding_local_id(
            &pool,
            provider_id,
            "dcim.sites",
            "102",
            "piping.inventory.site"
        )
        .await?,
        Some(related_local)
    );
    assert_eq!(
        completed_jobs(&pool, provider_id, &document.reference).await?,
        3
    );

    let missing_name = format!("missing-tenant-{suffix}");
    let mut missing = site_record(103, &missing_name, &missing_name);
    missing["tenant"] = json!({"id": 999});
    mock.replace_site(missing).await;
    let (status, _) = send_signed_webhook(
        &app,
        provider_id,
        &webhook_payload("created", "dcim.site", 103, "unresolved-tenant"),
        &secret,
    )
    .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(site_row(&pool, &missing_name).await?.is_none());
    assert_eq!(
        binding_local_id(
            &pool,
            provider_id,
            "dcim.sites",
            "103",
            "piping.inventory.site"
        )
        .await?,
        None
    );
    assert_eq!(
        completed_jobs(&pool, provider_id, &document.reference).await?,
        3
    );
    let (status, body) = send_signed_webhook(
        &app,
        provider_id,
        &webhook_payload("deleted", "dcim.site", 102, "related-delete"),
        &secret,
    )
    .await?;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_applied(&body, "related site deleted");
    assert!(site_row(&pool, &related_name).await?.is_none());
    assert_eq!(
        completed_jobs(&pool, provider_id, &document.reference).await?,
        4
    );

    // updated: the record is re-read from NetBox by id and the same local row is
    // rewritten, without a duplicate and without touching other rows.
    let site_updated_name = format!("live-site-updated-{suffix}");
    let mut related_site = site_record(101, &site_updated_name, &site_name);
    related_site["tenant"] = json!({"id": 202, "name": tenant_name, "slug": tenant_name});
    mock.replace_site(related_site).await;
    let (status, body) = send_signed_webhook(
        &app,
        provider_id,
        &webhook_payload("updated", "dcim.site", 101, "live-site-updated"),
        &secret,
    )
    .await?;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_applied(&body, "site updated");
    assert_eq!(
        completed_jobs(&pool, provider_id, &document.reference).await?,
        5
    );
    let (site_local_after, site_stored_name) = site_row(&pool, &site_updated_name)
        .await?
        .expect("updated site row");
    assert_eq!(site_stored_name, site_updated_name);
    assert_eq!(site_local_after, site_local);
    let related_tenant: Option<Uuid> =
        sqlx::query_scalar("SELECT tenant_id FROM dcim_site WHERE id=$1")
            .bind(site_local)
            .fetch_one(&pool)
            .await?;
    assert_eq!(related_tenant, Some(tenant_local));
    assert!(site_row(&pool, &site_name).await?.is_none());
    assert_eq!(
        binding_local_id(
            &pool,
            provider_id,
            "dcim.sites",
            "101",
            "piping.inventory.site"
        )
        .await?,
        Some(site_local)
    );

    let tenant_updated_name = format!("live-tenant-updated-{suffix}");
    mock.replace_tenant(tenant_record(202, &tenant_updated_name, &tenant_name))
        .await;
    let (status, body) = send_signed_webhook(
        &app,
        provider_id,
        &webhook_payload("updated", "tenancy.tenant", 202, "live-tenant-updated"),
        &secret,
    )
    .await?;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_applied(&body, "tenant updated");
    assert_eq!(
        completed_jobs(&pool, provider_id, &document.reference).await?,
        6
    );
    let (tenant_local_after, tenant_stored_name) = tenant_row(&pool, &tenant_updated_name)
        .await?
        .expect("updated tenant row");
    assert_eq!(tenant_stored_name, tenant_updated_name);
    assert_eq!(tenant_local_after, tenant_local);
    assert!(tenant_row(&pool, &tenant_name).await?.is_none());

    // The bound site references this tenant. Refusing the tenant delete must
    // leave the dependent row and both identities intact.
    let (status, _) = send_signed_webhook(
        &app,
        provider_id,
        &webhook_payload("deleted", "tenancy.tenant", 202, "live-tenant-blocked"),
        &secret,
    )
    .await?;
    assert_eq!(status, StatusCode::CONFLICT);
    let failed: String = sqlx::query_scalar("SELECT status FROM sync_webhook_event WHERE source_id=$1 AND request_id='live-tenant-blocked'")
        .bind(provider_id).fetch_one(&pool).await?;
    assert_eq!(failed, "failed");
    assert_eq!(
        completed_jobs(&pool, provider_id, &document.reference).await?,
        6
    );
    let retained: Option<Uuid> = sqlx::query_scalar("SELECT tenant_id FROM dcim_site WHERE id=$1")
        .bind(site_local)
        .fetch_one(&pool)
        .await?;
    assert_eq!(retained, Some(tenant_local));
    assert!(tenant_row(&pool, &tenant_updated_name).await?.is_some());
    assert_eq!(binding_count(&pool, provider_id).await?, 2);

    // A later source update clears the relation without re-keying either row.
    mock.replace_site(site_record(101, &site_updated_name, &site_name))
        .await;
    let (status, body) = send_signed_webhook(
        &app,
        provider_id,
        &webhook_payload("updated", "dcim.site", 101, "live-site-clear-tenant"),
        &secret,
    )
    .await?;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_applied(&body, "clear site tenant");
    assert_eq!(
        completed_jobs(&pool, provider_id, &document.reference).await?,
        7
    );
    let cleared: Option<Uuid> = sqlx::query_scalar("SELECT tenant_id FROM dcim_site WHERE id=$1")
        .bind(site_local)
        .fetch_one(&pool)
        .await?;
    assert_eq!(cleared, None);
    assert_eq!(binding_count(&pool, provider_id).await?, 2);

    let (status, body) = send_signed_webhook(
        &app,
        provider_id,
        &webhook_payload("updated", "dcim.unknown_model", 999, "unknown-model"),
        &secret,
    )
    .await?;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(body["status"], "ignored");
    assert_eq!(binding_count(&pool, provider_id).await?, 2);

    // A locally owned bystander must survive webhook deletions: no cascade.
    let bystander_name = format!("bystander-{suffix}");
    let bystander = create_local_site(&app, &cookie, &bystander_name).await?;

    // deleted: the local row and its binding are removed, then a second delete
    // for the same NetBox id is ignored.
    let (status, body) = send_signed_webhook(
        &app,
        provider_id,
        &webhook_payload("deleted", "dcim.site", 101, "live-site-deleted"),
        &secret,
    )
    .await?;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_applied(&body, "site deleted");
    assert_eq!(
        completed_jobs(&pool, provider_id, &document.reference).await?,
        8
    );
    assert!(site_row(&pool, &site_updated_name).await?.is_none());
    assert_eq!(
        binding_local_id(
            &pool,
            provider_id,
            "dcim.sites",
            "101",
            "piping.inventory.site"
        )
        .await?,
        None
    );
    assert!(
        sqlx::query_scalar::<_, Uuid>("SELECT id FROM dcim_site WHERE id = $1")
            .bind(bystander)
            .fetch_optional(&pool)
            .await?
            .is_some(),
        "bystander site must survive the webhook delete"
    );

    let (status, body) = send_signed_webhook(
        &app,
        provider_id,
        &webhook_payload("deleted", "dcim.site", 101, "live-site-deleted-again"),
        &secret,
    )
    .await?;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(
        body.get("status").and_then(Value::as_str),
        Some("ignored"),
        "second delete has no binding and is ignored: {body}"
    );

    let (status, body) = send_signed_webhook(
        &app,
        provider_id,
        &webhook_payload("deleted", "tenancy.tenant", 202, "live-tenant-deleted"),
        &secret,
    )
    .await?;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_applied(&body, "tenant deleted");
    assert_eq!(
        completed_jobs(&pool, provider_id, &document.reference).await?,
        9
    );
    assert!(tenant_row(&pool, &tenant_updated_name).await?.is_none());
    assert_eq!(
        binding_local_id(
            &pool,
            provider_id,
            "tenancy.tenants",
            "202",
            "piping.inventory.tenant"
        )
        .await?,
        None
    );

    let (status, body) = send_signed_webhook(
        &app,
        provider_id,
        &webhook_payload(
            "deleted",
            "tenancy.tenant",
            202,
            "live-tenant-deleted-again",
        ),
        &secret,
    )
    .await?;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(body.get("status").and_then(Value::as_str), Some("ignored"));

    assert_eq!(binding_count(&pool, provider_id).await?, 0);
    assert!(
        site_row(&pool, &bystander_name).await?.is_some(),
        "bystander site must remain after all webhook deletions"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM dcim_site")
            .fetch_one(&pool)
            .await?,
        1,
        "only the locally created bystander site remains"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM tenancy_tenant")
            .fetch_one(&pool)
            .await?,
        0
    );
    assert_eq!(
        mock.write_count(),
        0,
        "live inbound application must not write to NetBox"
    );

    isolated_app::shutdown(harness).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn live_webhook_rejects_conflict_mode_other_than_netbox_wins() -> TestResult {
    let harness = isolated_app::harness("pip323_netbox_conflict", 8).await?;
    let state = harness.state.clone();
    let pool = state.db_pool.clone();
    let app = harness.app.clone();
    let cookie = harness.cookie.clone();

    let suffix = unique_suffix();
    let site_name = format!("conflict-site-{suffix}");
    let tenant_name = format!("conflict-tenant-{suffix}");
    let mock = spawn_mock_netbox(
        site_record(303, &site_name, &site_name),
        tenant_record(404, &tenant_name, &tenant_name),
    )
    .await?;
    let secret = format!("conflict-webhook-secret-{suffix}");
    for mode in [
        ConflictMode::PipingWins,
        ConflictMode::LastWriteWins,
        ConflictMode::FieldLevel,
    ] {
        let provider_id = create_provider(&app, &cookie, &mock.base_url, &secret).await?;

        let document = mapping_document(provider_id, mode);
        assert_no_validation_errors(&document);
        assert_eq!(
            live_entity_mapping(&document, "dcim.site").map(|(entity, _)| entity),
            Some(InventoryEntity::Site)
        );
        seed_mapping(&state, &document).await?;

        let (status, body) = send_signed_webhook(
            &app,
            provider_id,
            &webhook_payload("created", "dcim.site", 303, "conflict-site-created"),
            &secret,
        )
        .await?;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "non netbox_wins conflict mode is a validation error: {body}"
        );
        assert!(
            body.to_string().contains("netbox_wins"),
            "error must name the required conflict mode: {body}"
        );

        let (journal_status, journal_error) = latest_journal(&pool, provider_id).await?;
        assert_eq!(journal_status, "failed");
        assert!(
            journal_error
                .as_deref()
                .unwrap_or_default()
                .contains("netbox_wins"),
            "journal error must record the rejected conflict mode: {journal_error:?}"
        );

        let (operational_status, error_code) = runtime_state(&pool, provider_id).await?;
        assert_eq!(operational_status, "degraded");
        assert_eq!(error_code.as_deref(), Some("webhook_apply_failed"));

        assert!(
            site_row(&pool, &site_name).await?.is_none(),
            "a rejected live event must not create a local row"
        );
        assert_eq!(binding_count(&pool, provider_id).await?, 0);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM dcim_site")
                .fetch_one(&pool)
                .await?,
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM tenancy_tenant")
                .fetch_one(&pool)
                .await?,
            0
        );
    }

    isolated_app::shutdown(harness).await?;
    Ok(())
}
