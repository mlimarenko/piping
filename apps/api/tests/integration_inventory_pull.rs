//! PIP-86 real-wire integration tests for the ordinary local-inventory pull.
//!
//! These tests exercise the production path end to end: a synthetic NetBox
//! HTTP fixture (built from the committed synthetic manifest
//! `tests/fixtures/netbox/inventory_pull_manifest.json`, never from a live
//! instance) is read over the wire by the real `NetboxInventorySource`, and the
//! real `LocalInventoryCatalog` writes the typed graph into PostgreSQL through
//! the immutable mapping configuration. One test drives the whole thing through
//! the actual HTTP API (`POST /api/v1/sync/jobs`) behind the app middleware and
//! login; the remaining tests pin the service directly so the transaction and
//! binding behaviour can be asserted without the request wrapper.
//!
//! The file name deliberately contains no `netbox`: the CI suite excludes
//! `netbox`-named integration binaries because they require a live NetBox
//! profile. These tests require only PostgreSQL, Redis and a loopback mock, so
//! they belong to the ordinary backend suite.
//!
//! Every test is `#[ignore]`d for the same reason as the rest of the
//! integration suites: they need the dev Postgres/Redis. Run with
//! `cargo test --test integration_inventory_pull -- --ignored`.
//!
//! No live credential or live NetBox payload is used or printed; the fixture is
//! synthetic and the mock token is an obvious placeholder.

mod common;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use axum::Router;
use axum::extract::State as AxumState;
use axum::http::{Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use chrono::Utc;
use serde_json::{Value, json};
use tokio::sync::Mutex;
use uuid::Uuid;

use piping_backend::app::AppState;
use piping_backend::contexts::sync::application::inventory_pull::InventoryPullService;
use piping_backend::contexts::sync::domain::{
    ConflictMode, EntityMapping, FieldMapping, InventoryEntity, LocalInventoryCatalogView,
    MappingConfiguration, MappingConfigurationStatus, MappingDirection, MappingRef, MappingTarget,
    SourceEntity, SyncJob,
};
use piping_backend::contexts::sync::infrastructure::persistence::local_inventory_target::LocalInventoryCatalog;
use piping_backend::contexts::sync::infrastructure::persistence::{
    MappingConfigurationRepository, SqlxMappingConfigurationRepository,
};
use piping_backend::contexts::sync::infrastructure::providers::NetboxInventorySource;
use piping_backend::errors::AppError;
use piping_backend::integrations::netbox::{
    NetboxClient, NetboxRepository, NetboxResourceRegistry,
};

use common::{
    TEST_ADMIN_PASSWORD, TEST_ADMIN_USERNAME, build_test_app_state, login_and_get_cookie, request,
    unique_suffix,
};

/// The committed synthetic source manifest. Mirrors the closed source census:
/// 26 approved resources, only the graph entities carry records.
const MANIFEST: &str = include_str!("fixtures/netbox/inventory_pull_manifest.json");

/// One loopback token for the mock; obvious placeholder, never a real secret.
const MOCK_TOKEN: &str = "synthetic-token";

type TestError = Box<dyn std::error::Error + Send + Sync>;
type TestResult = Result<(), TestError>;

fn inventory_pull_test_lock() -> &'static Arc<Mutex<()>> {
    static LOCK: OnceLock<Arc<Mutex<()>>> = OnceLock::new();
    LOCK.get_or_init(|| Arc::new(Mutex::new(())))
}

// ---------------------------------------------------------------------------
// Synthetic NetBox wire fixture
// ---------------------------------------------------------------------------

#[derive(Default)]
struct MockCounters {
    data_get: AtomicUsize,
    schema_get: AtomicUsize,
    post: AtomicUsize,
    patch: AtomicUsize,
    delete: AtomicUsize,
}

struct MockState {
    counters: Arc<MockCounters>,
    records: Arc<Mutex<BTreeMap<String, Vec<Value>>>>,
    schema: Value,
}

/// A running synthetic NetBox endpoint plus its request ledger.
struct MockNetbox {
    base_url: String,
    suffix: String,
    state: Arc<MockState>,
}

impl MockNetbox {
    fn schema(&self) -> &Value {
        &self.state.schema
    }

    fn data_get_count(&self) -> usize {
        self.state.counters.data_get.load(Ordering::SeqCst)
    }

    fn schema_get_count(&self) -> usize {
        self.state.counters.schema_get.load(Ordering::SeqCst)
    }

    fn write_count(&self) -> usize {
        let counters = &self.state.counters;
        counters.post.load(Ordering::SeqCst)
            + counters.patch.load(Ordering::SeqCst)
            + counters.delete.load(Ordering::SeqCst)
    }

    async fn replace_records(&self, source_key: &str, records: Vec<Value>) {
        self.state
            .records
            .lock()
            .await
            .insert(source_key.to_string(), records);
    }

    async fn records(&self, source_key: &str) -> Vec<Value> {
        self.state
            .records
            .lock()
            .await
            .get(source_key)
            .cloned()
            .unwrap_or_default()
    }
}

/// Append a per-test suffix to every `name`/`slug` value in the manifest so the
/// natural keys (`dcim_site.name`/`slug`, `dcim_manufacturer.name`/`slug`,
/// `inventory_tag.slug`) cannot collide with rows another suite or run left
/// behind. The `id` fields are untouched: the provider-scoped UUIDv5 binding,
/// not the natural key, is the identity the pull is allowed to trust.
fn suffix_natural_keys(value: &mut Value, suffix: &str) {
    match value {
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                if matches!(key.as_str(), "name" | "slug") {
                    if let Some(text) = child.as_str() {
                        let suffixed = format!("{text}-{suffix}");
                        *child = Value::String(suffixed);
                    } else {
                        suffix_natural_keys(child, suffix);
                    }
                } else {
                    suffix_natural_keys(child, suffix);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                suffix_natural_keys(item, suffix);
            }
        }
        _ => {}
    }
}

/// Build the OpenAPI document the registry introspects: one list path per
/// approved resource, exactly the census `api_path`s.
fn build_schema(manifest: &Value) -> Value {
    let mut paths = serde_json::Map::new();
    if let Some(resources) = manifest.get("resources").and_then(Value::as_object) {
        for entry in resources.values() {
            if let Some(path) = entry.get("api_path").and_then(Value::as_str) {
                paths.insert(
                    path.to_string(),
                    json!({
                        "get": {
                            "parameters": [
                                {"name": "ordering", "in": "query", "schema": {"type": "string"}},
                                {"name": "limit", "in": "query", "schema": {"type": "integer"}},
                                {"name": "offset", "in": "query", "schema": {"type": "integer"}}
                            ],
                            "responses": {
                                "200": {
                                    "description": "list",
                                    "content": { "application/json": { "schema": { "type": "object" } } }
                                }
                            }
                        }
                    }),
                );
            }
        }
    }
    json!({
        "openapi": "3.0.0",
        "info": { "title": "synthetic-pip86-source", "version": "0" },
        "paths": paths,
        "components": { "schemas": {} }
    })
}

fn source_key_for_path(path: &str) -> Option<String> {
    let rest = path.strip_prefix("/api/")?;
    let mut segments = rest
        .trim_end_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty());
    let app = segments.next()?;
    let resource = segments.next()?;
    Some(format!("{app}.{resource}"))
}

async fn handle_schema(AxumState(state): AxumState<Arc<MockState>>) -> impl IntoResponse {
    state.counters.schema_get.fetch_add(1, Ordering::SeqCst);
    axum::Json(state.schema.clone())
}

/// Any non-schema request. GET serves the paginated synthetic page; every write
/// verb is recorded and refused, so any attempted POST/PATCH/DELETE shows up in
/// the ledger even though it would fail server-side too.
async fn handle_any(
    AxumState(state): AxumState<Arc<MockState>>,
    method: Method,
    uri: Uri,
) -> Response {
    match method {
        Method::GET => {
            state.counters.data_get.fetch_add(1, Ordering::SeqCst);
            let mut records = match source_key_for_path(uri.path()) {
                Some(key) => state
                    .records
                    .lock()
                    .await
                    .get(&key)
                    .cloned()
                    .unwrap_or_default(),
                None => Vec::new(),
            };
            let parameters: BTreeMap<_, _> = uri
                .query()
                .unwrap_or_default()
                .split('&')
                .filter_map(|pair| pair.split_once('='))
                .collect();
            let limit = parameters
                .get("limit")
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(100);
            let offset = parameters
                .get("offset")
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(0);
            let count = records.len();
            if parameters.get("ordering") == Some(&"id") {
                records.sort_by_key(|record| record["id"].as_u64().unwrap_or(0));
            } else if count > 100 {
                // Simulate a database choosing a different order among equal names.
                records.rotate_left(offset % count);
            }
            let page: Vec<_> = records.into_iter().skip(offset).take(limit).collect();
            axum::Json(json!({
                "count": count,
                "results": page,
                "next": null,
                "previous": null
            }))
            .into_response()
        }
        Method::POST => {
            state.counters.post.fetch_add(1, Ordering::SeqCst);
            (StatusCode::FORBIDDEN, "source is read-only").into_response()
        }
        Method::PATCH => {
            state.counters.patch.fetch_add(1, Ordering::SeqCst);
            (StatusCode::FORBIDDEN, "source is read-only").into_response()
        }
        Method::DELETE => {
            state.counters.delete.fetch_add(1, Ordering::SeqCst);
            (StatusCode::FORBIDDEN, "source is read-only").into_response()
        }
        _ => (StatusCode::METHOD_NOT_ALLOWED, "unsupported").into_response(),
    }
}

async fn spawn_mock_netbox() -> Result<MockNetbox, TestError> {
    let suffix = unique_suffix();
    let mut manifest: Value = serde_json::from_str(MANIFEST)?;
    suffix_natural_keys(&mut manifest, &suffix);
    let field_name = format!("owner_team_{}", suffix.replace('-', "_"));
    manifest["resources"]["extras.custom-fields"]["records"][0]["name"] = json!(field_name);
    for resource in manifest["resources"]
        .as_object_mut()
        .ok_or("resources missing")?
        .values_mut()
    {
        for record in resource["records"]
            .as_array_mut()
            .ok_or("records missing")?
        {
            if let Some(fields) = record
                .get_mut("custom_fields")
                .and_then(Value::as_object_mut)
                && let Some(value) = fields.remove("owner_team")
            {
                fields.insert(field_name.clone(), value);
            }
        }
    }

    let mut records = BTreeMap::new();
    if let Some(resources) = manifest.get("resources").and_then(Value::as_object) {
        for (key, entry) in resources {
            records.insert(
                key.clone(),
                entry
                    .get("records")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
            );
        }
    }
    let schema = build_schema(&manifest);

    let state = Arc::new(MockState {
        counters: Arc::new(MockCounters::default()),
        records: Arc::new(Mutex::new(records)),
        schema,
    });
    let app = Router::new()
        .route("/api/schema/", get(handle_schema))
        .fallback(handle_any)
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
        suffix,
        state,
    })
}

// ---------------------------------------------------------------------------
// Mapping configuration + field catalogue
// ---------------------------------------------------------------------------

fn source_entity_keys(manifest: &Value) -> Vec<String> {
    manifest
        .get("resources")
        .and_then(Value::as_object)
        .map(|resources| resources.keys().cloned().collect())
        .unwrap_or_default()
}

/// Insert one immutable `(logical_id, 1)` configuration covering every approved
/// source entity (the full enabled manifest) with an identity field mapping per
/// entity. Returns the immutable reference the job request must name.
async fn seed_mapping_config(
    repository: &SqlxMappingConfigurationRepository,
    manifest: &Value,
    provider_id: Uuid,
    direction: MappingDirection,
    target: MappingTarget,
    conflict_mode: ConflictMode,
) -> Result<MappingRef, TestError> {
    let logical_id = Uuid::now_v7();
    let mut entity_mappings = Vec::new();
    for source_key in source_entity_keys(manifest) {
        let entity = InventoryEntity::from_source_key(&source_key)
            .ok_or_else(|| format!("unknown source entity in fixture: {source_key}"))?;
        entity_mappings.push(EntityMapping {
            id: Uuid::now_v7(),
            mapping_config_logical_id: logical_id,
            mapping_config_version_id: 1,
            source_entity: source_key.clone(),
            target_entity: entity.target_key(),
            enabled: true,
            created_at: Utc::now(),
        });
    }

    let mut field_mappings = Vec::new();
    for mapping in &entity_mappings {
        let entity = InventoryEntity::from_source_key(&mapping.source_entity)
            .ok_or("unknown fixture entity")?;
        for field in entity.field_iter() {
            let target_field = field.name();
            let source_field = if entity == InventoryEntity::Service && target_field == "parent" {
                "parent_object_id"
            } else {
                target_field
            };
            field_mappings.push(FieldMapping {
                id: Uuid::now_v7(),
                entity_mapping_id: mapping.id,
                source_field: source_field.to_string(),
                target_field: target_field.to_string(),
                forward_transform_json: json!({
                    "op": "identity",
                    "args": [{ "field": source_field }]
                }),
                reverse_transform_json: None,
                round_trip_critical: false,
                created_at: Utc::now(),
            });
        }
    }

    let configuration = MappingConfiguration {
        logical_id,
        version_id: 1,
        name: format!("local-inventory-pull-{}", unique_suffix()),
        description: None,
        source_provider_id: provider_id,
        target,
        status: MappingConfigurationStatus::Active,
        direction,
        conflict_mode,
        created_by: None,
        created_at: Utc::now(),
        retired_at: None,
    };
    repository
        .insert_new_version(&configuration, &entity_mappings, &field_mappings)
        .await?;
    Ok(MappingRef::new(logical_id, 1))
}

/// Insert a dedicated provider row so a direct-service test owns its own
/// binding namespace and job history, independent of the shared test provider.
/// The direct source is built against the mock explicitly, so the row's config
/// is intentionally empty.
async fn seed_provider_row(pool: &sqlx::PgPool) -> Result<Uuid, TestError> {
    let provider_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO sync_provider (id, name, provider_type, config, is_enabled) \
         VALUES ($1, $2, 'netbox', '{}'::jsonb, true)",
    )
    .bind(provider_id)
    .bind(format!("pip86-direct-{}", unique_suffix()))
    .execute(pool)
    .await?;
    Ok(provider_id)
}

async fn run_pull_direct(
    state: &AppState,
    mock: &MockNetbox,
    provider_id: Uuid,
    reference: &MappingRef,
) -> Result<SyncJob, AppError> {
    let target = LocalInventoryCatalog::new(state.db_pool.clone());
    let approved: Vec<SourceEntity> = target
        .resources()
        .into_iter()
        .map(|resource| resource.source_entity)
        .collect();
    let registry = Arc::new(NetboxResourceRegistry::from_schema(mock.schema().clone())?);
    let client = NetboxClient::new(mock.base_url.clone(), MOCK_TOKEN.to_string())?;
    let repository = NetboxRepository::new(client, registry.clone());
    let source = NetboxInventorySource::new(repository, &registry, provider_id, approved, 100)?;
    let service = InventoryPullService::new(
        state.repositories.sync_mapping_configurations.clone(),
        state.repositories.sync_jobs.clone(),
        source,
        target,
        state.redis.clone(),
        60,
    );
    service.run(provider_id, &reference.to_wire()).await
}

// ---------------------------------------------------------------------------
// HTTP helpers
// ---------------------------------------------------------------------------

async fn create_provider_via_api(
    router: &Router,
    cookie: &str,
    base_url: &str,
) -> Result<Uuid, TestError> {
    let body = json!({
        "name": format!("pip86-{}", unique_suffix()),
        "provider_type": "netbox",
        "is_enabled": true,
        "bootstrap": false,
        "config": {
            "base_url": base_url,
            "api_token": MOCK_TOKEN,
            "page_size": 100
            , "sync_mode": "pull_only"
        }
    })
    .to_string();
    let (status, bytes) = request(
        router,
        "POST",
        "/api/v1/providers",
        Some(&body),
        Some(cookie),
    )
    .await?;
    if status != StatusCode::CREATED {
        return Err(format!(
            "provider create returned {status}: {}",
            String::from_utf8_lossy(&bytes)
        )
        .into());
    }
    let response: Value = serde_json::from_slice(&bytes)?;
    let id = response
        .get("id")
        .and_then(Value::as_str)
        .ok_or("provider create response has no id")?;
    Ok(Uuid::parse_str(id)?)
}

async fn login(router: &Router) -> Result<String, TestError> {
    login_and_get_cookie(router, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or_else(|| "test admin login failed".into())
}

async fn post_sync_job(
    router: &Router,
    cookie: &str,
    provider_id: Uuid,
    reference: &MappingRef,
) -> Result<(StatusCode, Value), TestError> {
    let body = json!({
        "provider_id": provider_id.to_string(),
        "mapping_version": reference.to_wire(),
    })
    .to_string();
    let (status, bytes) = request(
        router,
        "POST",
        "/api/v1/sync/jobs",
        Some(&body),
        Some(cookie),
    )
    .await?;
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    Ok((status, value))
}

// ---------------------------------------------------------------------------
// Database assertions
// ---------------------------------------------------------------------------

async fn binding_local_id(
    pool: &sqlx::PgPool,
    provider_id: Uuid,
    source_entity: &str,
    external_id: &str,
) -> Result<Uuid, TestError> {
    let id = sqlx::query_scalar::<_, Uuid>(
        "SELECT local_id FROM sync_inventory_binding \
         WHERE source_provider_id = $1 AND source_entity = $2 AND external_id = $3",
    )
    .bind(provider_id)
    .bind(source_entity)
    .bind(external_id)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

async fn binding_count(pool: &sqlx::PgPool, provider_id: Uuid) -> Result<i64, TestError> {
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM sync_inventory_binding WHERE source_provider_id = $1",
    )
    .bind(provider_id)
    .fetch_one(pool)
    .await?;
    Ok(count)
}

async fn content_type_id(
    pool: &sqlx::PgPool,
    app_label: &str,
    model: &str,
) -> Result<Uuid, TestError> {
    let id = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM inventory_content_type WHERE app_label = $1 AND model = $2",
    )
    .bind(app_label)
    .bind(model)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

async fn completed_jobs(pool: &sqlx::PgPool, provider_id: Uuid) -> Result<i64, TestError> {
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM sync_job WHERE provider_id = $1 AND status = 'completed'",
    )
    .bind(provider_id)
    .fetch_one(pool)
    .await?;
    Ok(count)
}

async fn failed_jobs(pool: &sqlx::PgPool, provider_id: Uuid) -> Result<i64, TestError> {
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM sync_job WHERE provider_id = $1 AND status = 'failed'",
    )
    .bind(provider_id)
    .fetch_one(pool)
    .await?;
    Ok(count)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// The mandated ordinary path: authenticated `POST /api/v1/sync/jobs` against a
/// real HTTP source, asserting the native rows, the external bindings, the
/// generic-FK/primary-IP cycle, the tag catalogue and custom-field values, and
/// that the pull issued reads only.
#[tokio::test]
#[ignore = "requires PostgreSQL and Redis; run with `cargo test --test integration_inventory_pull -- --ignored`"]
async fn authenticated_pull_imports_full_graph_read_only() -> TestResult {
    let _guard = inventory_pull_test_lock().clone().lock_owned().await;
    let state = build_test_app_state().await?;
    let mock = spawn_mock_netbox().await?;
    let manifest: Value = serde_json::from_str(MANIFEST)?;

    let router = piping_backend::app::build_router(state.clone());
    let cookie = login(&router).await?;

    // Provider creation with `bootstrap: false` must not write to the source.
    let provider_id = create_provider_via_api(&router, &cookie, &mock.base_url).await?;
    assert_eq!(
        mock.write_count(),
        0,
        "creating a provider with bootstrap=false must not write to the source"
    );
    assert_eq!(
        mock.schema_get_count(),
        0,
        "creating a provider must not even read the source schema"
    );

    let reference = seed_mapping_config(
        &state.repositories.sync_mapping_configurations,
        &manifest,
        provider_id,
        MappingDirection::Pull,
        MappingTarget::LocalInventory,
        ConflictMode::NetboxWins,
    )
    .await?;

    let (status, body) = tokio::time::timeout(
        Duration::from_secs(60),
        post_sync_job(&router, &cookie, provider_id, &reference),
    )
    .await
    .map_err(|_| "sync job did not finish within 60s")??;
    assert_eq!(
        status,
        StatusCode::OK,
        "sync job failed: {}",
        serde_json::to_string(&body).unwrap_or_default()
    );
    assert_eq!(body["status"].as_str(), Some("completed"));
    let expected_version = reference.to_wire();
    assert_eq!(
        body["mapping_version"].as_str(),
        Some(expected_version.as_str())
    );

    // Read-only towards the provider: GETs only, no write verb at all.
    assert!(
        mock.data_get_count() > 0,
        "the pull must read every enabled resource over HTTP"
    );
    assert_eq!(
        mock.write_count(),
        0,
        "the pull must never POST/PATCH/DELETE the source"
    );

    let stats = &body["stats"];
    assert!(
        stats["extracted"].as_u64().unwrap_or(0) > 0,
        "stats must record extracted records"
    );
    assert_eq!(stats["tag_memberships_committed"], 3);
    assert_eq!(
        stats["committed"].as_u64().ok_or("row count missing")? + 3,
        stats["extracted"]
            .as_u64()
            .ok_or("extraction count missing")?
    );

    let pool = &state.db_pool;
    let site_id = binding_local_id(pool, provider_id, "dcim.sites", "1").await?;
    let device_id = binding_local_id(pool, provider_id, "dcim.devices", "1").await?;
    let interface_id = binding_local_id(pool, provider_id, "dcim.interfaces", "1").await?;
    let ip_id = binding_local_id(pool, provider_id, "ipam.ip-addresses", "1").await?;
    let mac_id = binding_local_id(pool, provider_id, "dcim.mac-addresses", "1").await?;
    let vm_id = binding_local_id(pool, provider_id, "virtualization.virtual-machines", "1").await?;
    let service_id = binding_local_id(pool, provider_id, "ipam.services", "1").await?;
    let role_id = binding_local_id(pool, provider_id, "dcim.device-roles", "1").await?;
    let device_type_id = binding_local_id(pool, provider_id, "dcim.device-types", "1").await?;

    let vm_name: String =
        sqlx::query_scalar("SELECT name FROM virtualization_virtualmachine WHERE id = $1")
            .bind(vm_id)
            .fetch_one(pool)
            .await?;
    assert_eq!(vm_name, format!("syn-vm-1-{}", mock.suffix));

    let (site_name, site_slug, site_tags): (String, String, Vec<String>) =
        sqlx::query_as("SELECT name, slug, tags FROM dcim_site WHERE id = $1")
            .bind(site_id)
            .fetch_one(pool)
            .await?;
    assert_eq!(site_name, format!("Synthetic Site-{}", mock.suffix));
    assert_eq!(site_slug, format!("synthetic-site-{}", mock.suffix));
    assert!(
        site_tags.contains(&format!("edge-{}", mock.suffix)),
        "native tag membership must be preserved"
    );

    let (device_role, device_site, device_type, primary_ip4): (Uuid, Uuid, Uuid, Uuid) =
        sqlx::query_as(
            "SELECT role_id, site_id, device_type_id, primary_ip4_id FROM dcim_device WHERE id = $1",
        )
        .bind(device_id)
        .fetch_one(pool)
        .await?;
    assert_eq!(device_role, role_id);
    assert_eq!(device_site, site_id);
    assert_eq!(device_type, device_type_id);
    assert_eq!(
        primary_ip4, ip_id,
        "the primary-IP foreign key resolves to the local ip-address id"
    );

    let device_custom: Value =
        sqlx::query_scalar("SELECT custom_field_data FROM dcim_device WHERE id = $1")
            .bind(device_id)
            .fetch_one(pool)
            .await?;
    assert_eq!(
        device_custom[format!("owner_team_{}", mock.suffix.replace('-', "_"))].as_str(),
        Some("ops"),
        "source custom-field values are preserved on the row"
    );

    let interface_device: Uuid =
        sqlx::query_scalar("SELECT device_id FROM dcim_interface WHERE id = $1")
            .bind(interface_id)
            .fetch_one(pool)
            .await?;
    assert_eq!(interface_device, device_id);

    // Primary-IP cycle: ip -> interface (GFK) -> device; the GFK id column holds
    // the local UUID, and the type column the seeded content type, never the
    // remote integer or the raw `app.model` token.
    let interface_content_type = content_type_id(pool, "dcim", "interface").await?;
    let (assigned_object, assigned_object_type): (Uuid, Uuid) = sqlx::query_as(
        "SELECT assigned_object_id, assigned_object_type_id FROM ipam_ipaddress WHERE id = $1",
    )
    .bind(ip_id)
    .fetch_one(pool)
    .await?;
    assert_eq!(assigned_object, interface_id);
    assert_eq!(assigned_object_type, interface_content_type);

    let mac_object: Uuid =
        sqlx::query_scalar("SELECT assigned_object_id FROM dcim_macaddress WHERE id = $1")
            .bind(mac_id)
            .fetch_one(pool)
            .await?;
    assert_eq!(mac_object, interface_id);

    let (service_parent, service_parent_type, service_protocol, service_ports): (
        Uuid,
        Uuid,
        String,
        Vec<i32>,
    ) = sqlx::query_as(
        "SELECT parent_object_id, parent_object_type_id, protocol, ports FROM ipam_service WHERE id = $1",
    )
    .bind(service_id)
    .fetch_one(pool)
    .await?;
    let device_content_type = content_type_id(pool, "dcim", "device").await?;
    assert_eq!(service_parent, device_id);
    assert_eq!(service_parent_type, device_content_type);
    assert_eq!(service_protocol, "tcp");
    assert!(
        service_ports.is_empty(),
        "the native integer ports array stays empty when the source sent it empty"
    );

    // `port_mappings` is the string list `<protocol>/<port>`, never an object.
    let port_mappings: Vec<(String, i32)> = sqlx::query_as(
        "SELECT protocol, port FROM ipam_service_port_mapping WHERE service_id = $1 ORDER BY ordinal",
    )
    .bind(service_id)
    .fetch_all(pool)
    .await?;
    assert_eq!(
        port_mappings,
        vec![("tcp".to_string(), 443), ("tcp".to_string(), 80)]
    );

    let (tag_name, tag_slug, tag_color, tag_weight): (String, String, String, i32) =
        sqlx::query_as("SELECT name, slug, color, weight FROM inventory_tag WHERE slug = $1")
            .bind(format!("edge-{}", mock.suffix))
            .fetch_one(pool)
            .await?;
    assert_eq!(tag_name, format!("edge-{}", mock.suffix));
    assert_eq!(tag_slug, format!("edge-{}", mock.suffix));
    assert_eq!(tag_color, "00ff00");
    assert_eq!(tag_weight, 100);

    Ok(())
}

/// Re-running the same immutable reference keeps the scoped identity, and a
/// renamed source row updates the same local UUID in place.
#[tokio::test]
#[ignore = "requires PostgreSQL and Redis; run with `cargo test --test integration_inventory_pull -- --ignored`"]
async fn repeat_pull_is_idempotent_and_updates_the_same_row() -> TestResult {
    let _guard = inventory_pull_test_lock().clone().lock_owned().await;
    let state = build_test_app_state().await?;
    let mock = spawn_mock_netbox().await?;
    let manifest: Value = serde_json::from_str(MANIFEST)?;
    let provider_id = seed_provider_row(&state.db_pool).await?;
    let reference = seed_mapping_config(
        &state.repositories.sync_mapping_configurations,
        &manifest,
        provider_id,
        MappingDirection::Pull,
        MappingTarget::LocalInventory,
        ConflictMode::NetboxWins,
    )
    .await?;

    run_pull_direct(&state, &mock, provider_id, &reference).await?;
    let first_bindings = binding_count(&state.db_pool, provider_id).await?;
    let site_id = binding_local_id(&state.db_pool, provider_id, "dcim.sites", "1").await?;
    let device_id = binding_local_id(&state.db_pool, provider_id, "dcim.devices", "1").await?;

    run_pull_direct(&state, &mock, provider_id, &reference).await?;
    assert_eq!(
        binding_count(&state.db_pool, provider_id).await?,
        first_bindings,
        "a repeat pull must not add or duplicate bindings"
    );
    assert_eq!(
        binding_local_id(&state.db_pool, provider_id, "dcim.sites", "1").await?,
        site_id,
        "the scoped identity is stable across runs"
    );

    // Rename the source row: same external id, new natural key. The binding,
    // not the name, is the identity, so the local UUID is reused.
    let mut sites = mock.records("dcim.sites").await;
    let renamed = format!("Renamed Site-{}", mock.suffix);
    let renamed_slug = format!("renamed-site-{}", mock.suffix);
    sites[0]["name"] = Value::String(renamed.clone());
    sites[0]["slug"] = Value::String(renamed_slug.clone());
    mock.replace_records("dcim.sites", sites).await;

    run_pull_direct(&state, &mock, provider_id, &reference).await?;
    assert_eq!(
        binding_local_id(&state.db_pool, provider_id, "dcim.sites", "1").await?,
        site_id,
        "renaming a source row must not re-key the binding"
    );
    let (name, slug): (String, String) =
        sqlx::query_as("SELECT name, slug FROM dcim_site WHERE id = $1")
            .bind(site_id)
            .fetch_one(&state.db_pool)
            .await?;
    assert_eq!(name, renamed);
    assert_eq!(slug, renamed_slug);
    let device_site: Uuid = sqlx::query_scalar("SELECT site_id FROM dcim_device WHERE id = $1")
        .bind(device_id)
        .fetch_one(&state.db_pool)
        .await?;
    assert_eq!(
        device_site, site_id,
        "references to the renamed row resolve to the same local UUID"
    );

    Ok(())
}

/// Two providers carrying the same `external_id` and the same source entity get
/// distinct bindings and distinct local rows; their natural keys are made
/// unique so the row itself never collides.
#[tokio::test]
#[ignore = "requires PostgreSQL and Redis; run with `cargo test --test integration_inventory_pull -- --ignored`"]
async fn two_providers_share_external_id_without_binding_collision() -> TestResult {
    let _guard = inventory_pull_test_lock().clone().lock_owned().await;
    let state = build_test_app_state().await?;
    let mock = spawn_mock_netbox().await?;
    let manifest: Value = serde_json::from_str(MANIFEST)?;
    let router = piping_backend::app::build_router(state.clone());
    let cookie = login(&router).await?;

    let provider_a = create_provider_via_api(&router, &cookie, &mock.base_url).await?;
    let provider_b = create_provider_via_api(&router, &cookie, &mock.base_url).await?;

    let reference_a = seed_mapping_config(
        &state.repositories.sync_mapping_configurations,
        &manifest,
        provider_a,
        MappingDirection::Pull,
        MappingTarget::LocalInventory,
        ConflictMode::NetboxWins,
    )
    .await?;
    let reference_b = seed_mapping_config(
        &state.repositories.sync_mapping_configurations,
        &manifest,
        provider_b,
        MappingDirection::Pull,
        MappingTarget::LocalInventory,
        ConflictMode::NetboxWins,
    )
    .await?;

    // Isolate this test to sites only: importing the whole graph for two
    // providers would collide on `inventory_tag`'s globally unique slug.
    for key in source_entity_keys(&manifest) {
        mock.replace_records(&key, Vec::new()).await;
    }

    let site_a = json!({
        "id": 1,
        "name": format!("Shared Name A-{}", mock.suffix),
        "slug": format!("shared-name-a-{}", mock.suffix),
        "status": { "value": "active", "label": "Active" },
        "facility": "",
        "description": "",
        "physical_address": "",
        "shipping_address": "",
        "comments": "",
        "tags": []
    });
    let site_b = json!({
        "id": 1,
        "name": format!("Shared Name B-{}", mock.suffix),
        "slug": format!("shared-name-b-{}", mock.suffix),
        "status": { "value": "active", "label": "Active" },
        "facility": "",
        "description": "",
        "physical_address": "",
        "shipping_address": "",
        "comments": "",
        "tags": []
    });

    mock.replace_records("dcim.sites", vec![site_a]).await;
    run_pull_direct(&state, &mock, provider_a, &reference_a).await?;
    let local_a = binding_local_id(&state.db_pool, provider_a, "dcim.sites", "1").await?;

    mock.replace_records("dcim.sites", vec![site_b]).await;
    run_pull_direct(&state, &mock, provider_b, &reference_b).await?;
    let local_b = binding_local_id(&state.db_pool, provider_b, "dcim.sites", "1").await?;

    assert_ne!(
        local_a, local_b,
        "the same external id under two providers must not share a binding"
    );
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM dcim_site WHERE slug IN ($1, $2)")
        .bind(format!("shared-name-a-{}", mock.suffix))
        .bind(format!("shared-name-b-{}", mock.suffix))
        .fetch_one(&state.db_pool)
        .await?;
    assert_eq!(count, 2, "both provider-local rows exist independently");

    Ok(())
}

/// Unsupported job shapes are rejected before the source is read or cached.
#[tokio::test]
async fn pagination_uses_unique_identity_order_instead_of_equal_names() -> TestResult {
    use piping_backend::contexts::sync::application::inventory_pull::collect_snapshot;
    let mock = spawn_mock_netbox().await?;
    let records = (1..=205)
        .rev()
        .map(|id| json!({"id": id, "name": "same-label"}))
        .collect();
    mock.replace_records("dcim.sites", records).await;
    let registry = Arc::new(NetboxResourceRegistry::from_schema(mock.schema().clone())?);
    let client = NetboxClient::new(mock.base_url.clone(), MOCK_TOKEN.into())?;
    let entity = SourceEntity::new("dcim.sites")?;
    let source = NetboxInventorySource::new(
        NetboxRepository::new(client, registry.clone()),
        &registry,
        Uuid::now_v7(),
        vec![entity.clone()],
        100,
    )?;
    let snapshot = collect_snapshot(&source, &[entity]).await?;
    let identities: std::collections::HashSet<_> = snapshot
        .records
        .iter()
        .map(|record| record.identity.external_id.as_str())
        .collect();
    assert_eq!(snapshot.records.len(), 205);
    assert_eq!(
        identities.len(),
        205,
        "paging must preserve every identity across tied names"
    );
    assert_eq!(mock.data_get_count(), 3);
    assert_eq!(mock.write_count(), 0);
    Ok(())
}

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn unknown_populated_source_field_blocks_the_whole_import() -> TestResult {
    let _guard = inventory_pull_test_lock().clone().lock_owned().await;
    let state = build_test_app_state().await?;
    let mock = spawn_mock_netbox().await?;
    let provider_id = seed_provider_row(&state.db_pool).await?;
    let manifest: Value = serde_json::from_str(MANIFEST)?;
    let reference = seed_mapping_config(
        &state.repositories.sync_mapping_configurations,
        &manifest,
        provider_id,
        MappingDirection::Pull,
        MappingTarget::LocalInventory,
        ConflictMode::NetboxWins,
    )
    .await?;
    let mut sites = mock.records("dcim.sites").await;
    sites[0]["mystery_business_field"] = json!("must not disappear");
    mock.replace_records("dcim.sites", sites).await;
    let error = run_pull_direct(&state, &mock, provider_id, &reference)
        .await
        .expect_err("unrepresented data must block the pull");
    assert!(
        matches!(error, AppError::Validation(ref message) if message.contains("mystery_business_field"))
    );
    assert_eq!(binding_count(&state.db_pool, provider_id).await?, 0);
    assert_eq!(mock.write_count(), 0);
    Ok(())
}

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn another_provider_cannot_adopt_global_tag_definitions() -> TestResult {
    let _guard = inventory_pull_test_lock().clone().lock_owned().await;
    let state = build_test_app_state().await?;
    let mock = spawn_mock_netbox().await?;
    let manifest: Value = serde_json::from_str(MANIFEST)?;
    let first = seed_provider_row(&state.db_pool).await?;
    let second = seed_provider_row(&state.db_pool).await?;
    let first_config = seed_mapping_config(
        &state.repositories.sync_mapping_configurations,
        &manifest,
        first,
        MappingDirection::Pull,
        MappingTarget::LocalInventory,
        ConflictMode::NetboxWins,
    )
    .await?;
    let second_config = seed_mapping_config(
        &state.repositories.sync_mapping_configurations,
        &manifest,
        second,
        MappingDirection::Pull,
        MappingTarget::LocalInventory,
        ConflictMode::NetboxWins,
    )
    .await?;
    run_pull_direct(&state, &mock, first, &first_config).await?;
    let first_count = binding_count(&state.db_pool, first).await?;
    let error = run_pull_direct(&state, &mock, second, &second_config)
        .await
        .expect_err("global tag must not be adopted by name");
    assert!(
        matches!(error, AppError::Validation(ref message) if message.contains("global tag definition"))
    );
    assert_eq!(binding_count(&state.db_pool, second).await?, 0);
    assert_eq!(binding_count(&state.db_pool, first).await?, first_count);
    assert_eq!(mock.write_count(), 0);
    Ok(())
}

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn absent_custom_fields_preserve_but_empty_or_null_clears_values() -> TestResult {
    let _guard = inventory_pull_test_lock().clone().lock_owned().await;
    let state = build_test_app_state().await?;
    let mock = spawn_mock_netbox().await?;
    let manifest: Value = serde_json::from_str(MANIFEST)?;
    let provider = seed_provider_row(&state.db_pool).await?;
    let reference = seed_mapping_config(
        &state.repositories.sync_mapping_configurations,
        &manifest,
        provider,
        MappingDirection::Pull,
        MappingTarget::LocalInventory,
        ConflictMode::NetboxWins,
    )
    .await?;
    run_pull_direct(&state, &mock, provider, &reference).await?;
    let id = binding_local_id(&state.db_pool, provider, "dcim.devices", "1").await?;
    let original = mock.records("dcim.devices").await;
    let mut absent = original.clone();
    absent[0]
        .as_object_mut()
        .ok_or("device missing")?
        .remove("custom_fields");
    mock.replace_records("dcim.devices", absent).await;
    run_pull_direct(&state, &mock, provider, &reference).await?;
    let preserved: Value =
        sqlx::query_scalar("SELECT custom_field_data FROM dcim_device WHERE id=$1")
            .bind(id)
            .fetch_one(&state.db_pool)
            .await?;
    assert_eq!(preserved, original[0]["custom_fields"]);
    for cleared in [json!({}), Value::Null] {
        mock.replace_records("dcim.devices", original.clone()).await;
        run_pull_direct(&state, &mock, provider, &reference).await?;
        let mut records = original.clone();
        records[0]["custom_fields"] = cleared;
        mock.replace_records("dcim.devices", records).await;
        run_pull_direct(&state, &mock, provider, &reference).await?;
        let values: Value =
            sqlx::query_scalar("SELECT custom_field_data FROM dcim_device WHERE id=$1")
                .bind(id)
                .fetch_one(&state.db_pool)
                .await?;
        assert_eq!(
            values,
            json!({}),
            "explicit empty CF values must clear stale data"
        );
    }
    assert_eq!(mock.write_count(), 0);
    Ok(())
}

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn source_service_nulls_clear_native_optional_fields() -> TestResult {
    let _guard = inventory_pull_test_lock().clone().lock_owned().await;
    let state = build_test_app_state().await?;
    let mock = spawn_mock_netbox().await?;
    let manifest: Value = serde_json::from_str(MANIFEST)?;
    let provider_id = seed_provider_row(&state.db_pool).await?;
    let reference = seed_mapping_config(
        &state.repositories.sync_mapping_configurations,
        &manifest,
        provider_id,
        MappingDirection::Pull,
        MappingTarget::LocalInventory,
        ConflictMode::NetboxWins,
    )
    .await?;
    run_pull_direct(&state, &mock, provider_id, &reference).await?;
    let service_id = binding_local_id(&state.db_pool, provider_id, "ipam.services", "1").await?;
    let mut services = mock.records("ipam.services").await;
    services[0]["protocol"] = Value::Null;
    services[0]["ports"] = Value::Null;
    mock.replace_records("ipam.services", services).await;
    run_pull_direct(&state, &mock, provider_id, &reference).await?;
    let (protocol, ports): (Option<String>, Option<Vec<i32>>) =
        sqlx::query_as("SELECT protocol,ports FROM ipam_service WHERE id=$1")
            .bind(service_id)
            .fetch_one(&state.db_pool)
            .await?;
    assert!(protocol.is_none());
    assert!(ports.is_none());
    let endpoints: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ipam_service_port_mapping WHERE service_id=$1")
            .bind(service_id)
            .fetch_one(&state.db_pool)
            .await?;
    assert_eq!(
        endpoints, 2,
        "clearing optional legacy fields must preserve explicit port mappings"
    );
    assert_eq!(mock.write_count(), 0);
    Ok(())
}

/// Unsupported job shapes are rejected before the source is read or cached.
#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn pull_only_bootstrap_never_provisions_remote_resources() -> TestResult {
    let _guard = inventory_pull_test_lock().clone().lock_owned().await;
    let state = build_test_app_state().await?;
    let mock = spawn_mock_netbox().await?;
    let router = piping_backend::app::build_router(state);
    let cookie = login(&router).await?;
    let provider_id = create_provider_via_api(&router, &cookie, &mock.base_url).await?;
    let (status, _) = request(
        &router,
        "POST",
        &format!("/api/v1/providers/{provider_id}/bootstrap"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(mock.write_count(), 0);
    assert_eq!(mock.data_get_count(), 0);
    Ok(())
}

/// Unsupported job shapes are rejected before the source is read or cached.
#[tokio::test]
#[ignore = "requires PostgreSQL and Redis; run with `cargo test --test integration_inventory_pull -- --ignored`"]
async fn unsupported_job_shapes_are_rejected_before_any_source_read() -> TestResult {
    let _guard = inventory_pull_test_lock().clone().lock_owned().await;
    let state = build_test_app_state().await?;
    let mock = spawn_mock_netbox().await?;
    let manifest: Value = serde_json::from_str(MANIFEST)?;
    let provider_id = seed_provider_row(&state.db_pool).await?;

    let push = seed_mapping_config(
        &state.repositories.sync_mapping_configurations,
        &manifest,
        provider_id,
        MappingDirection::Push,
        MappingTarget::LocalInventory,
        ConflictMode::NetboxWins,
    )
    .await?;
    let provider_target = seed_mapping_config(
        &state.repositories.sync_mapping_configurations,
        &manifest,
        provider_id,
        MappingDirection::Pull,
        MappingTarget::Provider { provider_id },
        ConflictMode::NetboxWins,
    )
    .await?;
    let field_level = seed_mapping_config(
        &state.repositories.sync_mapping_configurations,
        &manifest,
        provider_id,
        MappingDirection::Pull,
        MappingTarget::LocalInventory,
        ConflictMode::FieldLevel,
    )
    .await?;

    for reference in [&push, &provider_target, &field_level] {
        let error = run_pull_direct(&state, &mock, provider_id, reference)
            .await
            .expect_err("an unsupported job shape must be rejected");
        assert!(
            matches!(error, AppError::Validation(_)),
            "expected a validation rejection, got {error:?}"
        );
    }

    assert_eq!(
        mock.data_get_count(),
        0,
        "an unsupported shape must be rejected before any source data read"
    );
    assert_eq!(
        mock.schema_get_count(),
        0,
        "an unsupported shape must be rejected before the schema is fetched/cached"
    );
    assert_eq!(mock.write_count(), 0);
    assert_eq!(binding_count(&state.db_pool, provider_id).await?, 0);

    Ok(())
}

/// Seed the four native resources whose custom-field scope the projection test
/// reads back. Each carries one synthetic record.
async fn seed_native_scope_records(mock: &MockNetbox) {
    for (resource, records) in [
        (
            "dcim.inventory-items",
            vec![
                json!({"id":1,"device":{"id":1},"name":"synthetic-item","label":"","part_id":"","serial":"","status":"active","description":"","discovered":false,"tags":[]}),
            ],
        ),
        (
            "ipam.prefixes",
            vec![
                json!({"id":1,"prefix":"10.0.0.0/24","status":"active","description":"","comments":"","is_pool":false,"mark_utilized":false,"tags":[]}),
            ],
        ),
        (
            "virtualization.interfaces",
            vec![
                json!({"id":1,"virtual_machine":{"id":1},"name":"synthetic-nic","description":"","enabled":true,"tags":[]}),
            ],
        ),
        (
            "virtualization.virtual-disks",
            vec![
                json!({"id":1,"virtual_machine":{"id":1},"name":"synthetic-disk","description":"","size":4096,"tags":[]}),
            ],
        ),
    ] {
        mock.replace_records(resource, records).await;
    }
}

/// Bind one custom-field definition to every imported scope and set the same
/// `owner_team_*` value on each scope's single record. Returns the scoped field
/// name. An `ipam.services` record also gets its protocol/ports cleared so the
/// value-junction null path is exercised.
async fn seed_native_custom_field_scopes(
    mock: &MockNetbox,
    scopes: &[(&str, &str, &str)],
) -> String {
    let field_name = format!("owner_team_{}", mock.suffix.replace('-', "_"));
    let mut definitions = mock.records("extras.custom-fields").await;
    definitions[0]["object_types"] = json!(
        scopes
            .iter()
            .map(|(_, token, _)| *token)
            .chain(["dcim.device"])
            .collect::<Vec<_>>()
    );
    mock.replace_records("extras.custom-fields", definitions)
        .await;
    for &(resource, _, _) in scopes {
        let mut records = mock.records(resource).await;
        records[0]["custom_fields"] = json!({field_name.clone(): "invented-ops"});
        if resource == "ipam.services" {
            records[0]["protocol"] = Value::Null;
            records[0]["ports"] = Value::Null;
        }
        mock.replace_records(resource, records).await;
    }
    field_name
}

/// Write one bound custom-field value for an entity token and apply its secret
/// intents, so the definition becomes a canonical secret binding.
async fn bind_native_cf_secret(
    state: &AppState,
    id: Uuid,
    content_type: &str,
    field_name: &str,
) -> TestResult {
    let mut incoming = serde_json::Map::new();
    incoming.insert(field_name.to_string(), json!("invented-bound-value"));
    let outcome = state
        .services
        .custom_field_values
        .write_for_entity_token(content_type, &incoming)
        .await?;
    state
        .services
        .custom_field_values
        .apply_secret_intents(id, outcome.secret_intents)
        .await?;
    Ok(())
}

/// The expected projection of the scoped custom field for a phase: the imported
/// value, an unbound-secret null, or the mask once a canonical binding exists.
fn expected_native_cf_value(phase: usize) -> Value {
    match phase {
        0 => json!("invented-ops"),
        1 => Value::Null,
        _ => json!("****"),
    }
}

/// Pick the projection row for a list read (found by id) or a detail read.
fn native_scope_row<'a>(body: &'a Value, id: Uuid, suffix: &str) -> Result<&'a Value, TestError> {
    if suffix.is_empty() {
        let row = body["items"]
            .as_array()
            .ok_or("items missing")?
            .iter()
            .find(|row| row["id"] == id.to_string())
            .ok_or("imported row missing")?;
        Ok(row)
    } else {
        Ok(body)
    }
}

/// Read one scope's list and detail projections behind a session and without
/// one, asserting the mask contract and the service port mapping.
async fn assert_native_scope_projection(
    router: &Router,
    cookie: &str,
    id: Uuid,
    path: &str,
    phase: usize,
    field_name: &str,
) -> TestResult {
    for suffix in [String::new(), format!("/{id}")] {
        let uri = if suffix.is_empty() {
            format!("/api/v1/inventory/{path}?limit=200")
        } else {
            format!("/api/v1/inventory/{path}{suffix}")
        };
        let (status, bytes) = request(router, "GET", &uri, None, Some(cookie)).await?;
        let (unauthenticated_status, _) = request(router, "GET", &uri, None, None).await?;
        assert_eq!(
            unauthenticated_status,
            StatusCode::FORBIDDEN,
            "unauthenticated native CF read: {path}"
        );
        assert_eq!(status, StatusCode::OK, "native CF read failed: {uri}");
        let body: Value = serde_json::from_slice(&bytes)?;
        let row = native_scope_row(&body, id, &suffix)?;
        assert_eq!(
            row["custom_fields"][field_name],
            expected_native_cf_value(phase),
            "projection mismatch: {path}"
        );
        if path == "services" {
            assert_eq!(row["port_mappings"], json!(["tcp/443", "tcp/80"]));
            assert!(row["protocol"].is_null());
            assert!(row["ports"].is_null());
        }
    }
    Ok(())
}

/// The native read projection covers every imported custom-field scope, not
/// merely the original six editable entity kinds. Secrets stay masked on GET.
#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn native_custom_fields_are_visible_and_masked_across_imported_scopes() -> TestResult {
    let _guard = inventory_pull_test_lock().clone().lock_owned().await;
    let state = build_test_app_state().await?;
    let mock = spawn_mock_netbox().await?;
    seed_native_scope_records(&mock).await;
    let scopes = [
        ("dcim.interfaces", "dcim.interface", "interfaces"),
        (
            "dcim.inventory-items",
            "dcim.inventoryitem",
            "inventory-items",
        ),
        ("ipam.ip-addresses", "ipam.ipaddress", "ip-addresses"),
        ("ipam.prefixes", "ipam.prefix", "prefixes"),
        ("ipam.services", "ipam.service", "services"),
        (
            "virtualization.interfaces",
            "virtualization.vminterface",
            "vm-interfaces",
        ),
        (
            "virtualization.virtual-disks",
            "virtualization.virtualdisk",
            "virtual-disks",
        ),
    ];
    let field_name = seed_native_custom_field_scopes(&mock, &scopes).await;
    let manifest: Value = serde_json::from_str(MANIFEST)?;
    let provider_id = seed_provider_row(&state.db_pool).await?;
    let reference = seed_mapping_config(
        &state.repositories.sync_mapping_configurations,
        &manifest,
        provider_id,
        MappingDirection::Pull,
        MappingTarget::LocalInventory,
        ConflictMode::NetboxWins,
    )
    .await?;
    run_pull_direct(&state, &mock, provider_id, &reference).await?;
    let router = piping_backend::app::build_router(state.clone());
    let cookie = login(&router).await?;
    // Unbound secret definitions return null; only canonical secret bindings
    // produce the mask. Old JSONB plaintext must never be used for either read.
    for phase in 0..3 {
        if phase == 1 {
            sqlx::query("UPDATE custom_field SET is_secret=true WHERE name=$1")
                .bind(&field_name)
                .execute(&state.db_pool)
                .await?;
        }
        for (resource, content_type, path) in scopes {
            let id = binding_local_id(&state.db_pool, provider_id, resource, "1").await?;
            if phase == 2 {
                bind_native_cf_secret(&state, id, content_type, &field_name).await?;
            }
            assert_native_scope_projection(&router, &cookie, id, path, phase, &field_name).await?;
        }
    }
    assert_eq!(mock.write_count(), 0);
    Ok(())
}

/// A late relation failure (a reference the binding set cannot resolve) rolls
/// the whole transaction back: zero bound rows, no completed job, and a failed
/// job is persisted for the operator.
#[tokio::test]
#[ignore = "requires PostgreSQL and Redis; run with `cargo test --test integration_inventory_pull -- --ignored`"]
async fn late_relation_failure_imports_nothing() -> TestResult {
    let _guard = inventory_pull_test_lock().clone().lock_owned().await;
    let state = build_test_app_state().await?;
    let mock = spawn_mock_netbox().await?;
    let manifest: Value = serde_json::from_str(MANIFEST)?;
    let provider_id = seed_provider_row(&state.db_pool).await?;
    let reference = seed_mapping_config(
        &state.repositories.sync_mapping_configurations,
        &manifest,
        provider_id,
        MappingDirection::Pull,
        MappingTarget::LocalInventory,
        ConflictMode::NetboxWins,
    )
    .await?;

    // Point the device at a site external id that no record resolves.
    let mut devices = mock.records("dcim.devices").await;
    devices[0]["site"] = json!({ "id": 999999 });
    mock.replace_records("dcim.devices", devices).await;

    let error = run_pull_direct(&state, &mock, provider_id, &reference)
        .await
        .expect_err("an unresolvable relation must fail the pull");
    match &error {
        AppError::Validation(message) => assert!(
            message.contains("cannot be resolved"),
            "expected an unresolved-relation validation message, got: {message}"
        ),
        other => panic!("expected a validation error, got {other:?}"),
    }

    assert_eq!(
        binding_count(&state.db_pool, provider_id).await?,
        0,
        "a late failure must leave zero bindings"
    );
    for (table, column, value) in [
        (
            "dcim_site",
            "slug",
            format!("synthetic-site-{}", mock.suffix),
        ),
        (
            "dcim_manufacturer",
            "slug",
            format!("synthetic-manufacturing-{}", mock.suffix),
        ),
        (
            "dcim_device",
            "name",
            format!("syn-device-1-{}", mock.suffix),
        ),
        ("dcim_interface", "name", format!("eth0-{}", mock.suffix)),
        ("ipam_service", "name", format!("syn-https-{}", mock.suffix)),
        (
            "virtualization_virtualmachine",
            "name",
            format!("syn-vm-1-{}", mock.suffix),
        ),
    ] {
        let query = format!("SELECT COUNT(*) FROM {table} WHERE {column} = $1");
        let count: i64 = sqlx::query_scalar(&query)
            .bind(&value)
            .fetch_one(&state.db_pool)
            .await?;
        assert_eq!(
            count, 0,
            "{table} must have no row for '{value}' after a rolled-back pull"
        );
    }
    assert_eq!(
        completed_jobs(&state.db_pool, provider_id).await?,
        0,
        "no completed job may survive"
    );
    assert_eq!(
        failed_jobs(&state.db_pool, provider_id).await?,
        1,
        "the failed pull must be persisted as a failed job"
    );

    Ok(())
}

/// The target catalogue the routes serve covers every approved row entity with
/// canonical field names (`role`, not `role_id`).
#[tokio::test]
#[ignore = "requires PostgreSQL and Redis; run with `cargo test --test integration_inventory_pull -- --ignored`"]
async fn local_inventory_catalog_exposes_all_approved_resources() -> TestResult {
    let _guard = inventory_pull_test_lock().clone().lock_owned().await;
    let state = build_test_app_state().await?;
    let router = piping_backend::app::build_router(state.clone());
    let cookie = login(&router).await?;

    let (status, bytes) = request(
        &router,
        "GET",
        "/api/v1/sync/targets/local-inventory/resources",
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let body: Value = serde_json::from_slice(&bytes)?;
    let resources = body["resources"]
        .as_array()
        .ok_or("resources must be an array")?;

    let mut advertised: Vec<String> = resources
        .iter()
        .filter_map(|resource| resource["source_entity"].as_str().map(str::to_string))
        .collect();
    advertised.sort();
    let mut expected: Vec<String> = InventoryEntity::all()
        .chain(std::iter::once(InventoryEntity::TaggedObject))
        .map(|entity| entity.source_key().to_string())
        .collect();
    expected.sort();
    assert_eq!(advertised, expected);

    let device = resources
        .iter()
        .find(|resource| resource["source_entity"].as_str() == Some("dcim.devices"))
        .ok_or("dcim.devices must be advertised")?;
    let field_names: Vec<&str> = device["fields"]
        .as_array()
        .ok_or("fields must be an array")?
        .iter()
        .filter_map(|field| field["name"].as_str())
        .collect();
    assert!(field_names.contains(&"role"));
    assert!(field_names.contains(&"primary_ip4"));
    assert!(
        !field_names.contains(&"role_id"),
        "the catalogue must advertise canonical names, not physical columns"
    );

    Ok(())
}
