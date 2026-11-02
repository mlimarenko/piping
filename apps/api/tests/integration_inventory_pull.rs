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
//! No live credential is used or printed. The hardware case serves committed
//! complete serializer maps captured from a synthetic NetBox dataset unchanged.

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
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use tokio::sync::Mutex;
use uuid::Uuid;

use piping_backend::app::AppState;
use piping_backend::contexts::shared::request_context::{RequestAuth, RequestContext};
use piping_backend::contexts::sync::application::PolicyRequestCommitAuthority;
use piping_backend::contexts::sync::application::inventory_pull::InventoryPullService;
use piping_backend::contexts::sync::domain::inventory_export::digest;
use piping_backend::contexts::sync::domain::inventory_preflight::{Finding, FindingCode};
use piping_backend::contexts::sync::domain::{
    ConflictMode, EntityMapping, FieldMapping, InventoryEntity, LocalInventoryCatalogView,
    MappingConfigDocument, MappingConfiguration, MappingConfigurationStatus, MappingDirection,
    MappingRef, MappingTarget, SourceEntity, SourceIdentity, SourceRecord, SourceSnapshot, SyncJob,
    TypedInventoryBatch,
};
use piping_backend::contexts::sync::infrastructure::persistence::local_inventory_target::LocalInventoryCatalog;
use piping_backend::contexts::sync::infrastructure::persistence::local_inventory_target::assess_preparation;
use piping_backend::contexts::sync::infrastructure::persistence::{
    MappingConfigurationRepository, SqlxMappingConfigurationRepository,
};
use piping_backend::contexts::sync::infrastructure::providers::NetboxInventorySource;
use piping_backend::errors::AppError;
use piping_backend::contexts::inventory::domain::{Device, DeviceRole, DeviceType, Manufacturer, Site};
use piping_backend::contexts::inventory::domain::dcim::mac_address::MacAddress;
use piping_backend::contexts::inventory::infrastructure::persistence::repositories::repository::{DeviceRepository, DeviceRoleRepository, DeviceTypeRepository, ManufacturerRepository, SiteRepository};
use piping_backend::contexts::inventory::infrastructure::persistence::repositories::dcim::mac_address_repository::MacAddressRepository;
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

    spawn_mock_manifest(&manifest, suffix).await
}

async fn spawn_mock_manifest(manifest: &Value, suffix: String) -> Result<MockNetbox, TestError> {
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
    let schema = build_schema(manifest);

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

/// Shared pure construction for native pulls and committed-manifest preparation
/// regressions. No alternate whitelist, fixture rewriting or DB context.
fn fixture_mapping_document(
    manifest: &Value,
    provider_id: Uuid,
    direction: MappingDirection,
    target: MappingTarget,
    conflict_mode: ConflictMode,
) -> Result<MappingConfigDocument, TestError> {
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

    Ok(MappingConfigDocument {
        reference: MappingRef::new(logical_id, 1),
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
        entity_mappings,
        field_mappings,
    })
}

/// Insert the same full-catalog identity mapping exercised by the pure tests.
async fn seed_mapping_config(
    repository: &SqlxMappingConfigurationRepository,
    manifest: &Value,
    provider_id: Uuid,
    direction: MappingDirection,
    target: MappingTarget,
    conflict_mode: ConflictMode,
) -> Result<MappingRef, TestError> {
    let document =
        fixture_mapping_document(manifest, provider_id, direction, target, conflict_mode)?;
    let configuration = MappingConfiguration {
        logical_id: document.reference.logical_id,
        version_id: document.reference.version_id,
        name: document.name,
        description: document.description,
        source_provider_id: document.source_provider_id,
        target: document.target,
        status: document.status,
        direction: document.direction,
        conflict_mode: document.conflict_mode,
        created_by: document.created_by,
        created_at: document.created_at,
        retired_at: document.retired_at,
    };
    repository
        .insert_new_version(
            &configuration,
            &document.entity_mappings,
            &document.field_mappings,
        )
        .await?;
    Ok(document.reference)
}

fn fixture_snapshot(manifest: &Value, provider_id: Uuid) -> Result<SourceSnapshot, TestError> {
    let mut records = Vec::new();
    for (key, resource) in manifest["resources"]
        .as_object()
        .ok_or("fixture resources missing")?
    {
        for value in resource["records"]
            .as_array()
            .ok_or("fixture records missing")?
        {
            let id = value["id"].as_u64().ok_or("fixture id missing")?;
            records.push(SourceRecord::new(
                SourceIdentity::new(provider_id, SourceEntity::new(key)?, id.to_string())?,
                value
                    .as_object()
                    .ok_or("fixture record is not an object")?
                    .clone(),
            ));
        }
    }
    Ok(SourceSnapshot { records })
}

fn pull_validation_findings(error: &AppError, code: &str) -> Vec<Finding> {
    let AppError::Validation(message) = error else {
        panic!("expected structured validation error");
    };
    let report: Value = serde_json::from_str(message).expect("structured pull validation report");
    assert_eq!(report["code"].as_str(), Some(code));
    serde_json::from_value(report["findings"].clone()).expect("typed safe pull findings")
}

#[test]
fn committed_manifest_prepares_with_the_seed_mapping_document() -> TestResult {
    let manifest: Value = serde_json::from_str(MANIFEST)?;
    let provider = Uuid::from_u128(1);
    let config = fixture_mapping_document(
        &manifest,
        provider,
        MappingDirection::Pull,
        MappingTarget::LocalInventory,
        ConflictMode::NetboxWins,
    )?;
    let snapshot = fixture_snapshot(&manifest, provider)?;
    let assessment = assess_preparation(&config, &snapshot);
    assert!(
        assessment.findings.is_empty(),
        "committed manifest preparation findings: {:?}",
        assessment.findings
    );
    assert!(assessment.analysis_complete);
    let batch = assessment
        .batch
        .expect("full graph must remain write-eligible");
    assert_eq!(batch.extracted_count(), snapshot.records.len() as u64);
    assert_eq!(
        batch.rows().len(),
        snapshot
            .records
            .iter()
            .filter(|record| record.entity().as_str() != "extras.tagged-objects")
            .count()
    );
    Ok(())
}

#[test]
fn committed_service_parent_is_validated_and_populated_unsupported_data_still_blocks() -> TestResult
{
    let manifest: Value = serde_json::from_str(MANIFEST)?;
    let provider = Uuid::from_u128(1);
    let config = fixture_mapping_document(
        &manifest,
        provider,
        MappingDirection::Pull,
        MappingTarget::LocalInventory,
        ConflictMode::NetboxWins,
    )?;
    let original = fixture_snapshot(&manifest, provider)?;
    for (field, value, code, path) in [
        (
            "parent_object_id",
            Value::Null,
            FindingCode::FieldValueInvalid,
            "parent",
        ),
        (
            "parent_object_type",
            json!("CANARY-private-type"),
            FindingCode::FieldValueInvalid,
            "parent",
        ),
        (
            "owner",
            json!({"id": 1}),
            FindingCode::FieldUnsupported,
            "owner",
        ),
        (
            "ipaddresses",
            json!([{ "id": 1 }]),
            FindingCode::FieldUnsupported,
            "ipaddresses",
        ),
    ] {
        let mut snapshot = original.clone();
        snapshot
            .records
            .iter_mut()
            .find(|record| record.entity().as_str() == "ipam.services")
            .ok_or("fixture service missing")?
            .fields
            .insert(field.into(), value);
        let assessment = assess_preparation(&config, &snapshot);
        assert!(assessment.batch.is_none());
        assert!(
            assessment
                .findings
                .iter()
                .any(|finding| finding.code == code
                    && finding.entity.as_deref() == Some("ipam.services")
                    && finding.field_path.as_deref() == Some(path))
        );
        assert!(!serde_json::to_string(&assessment.findings)?.contains("CANARY"));
    }
    let mut missing_mapping = config;
    let service_mapping = missing_mapping
        .entity_mappings
        .iter()
        .find(|mapping| mapping.source_entity == "ipam.services")
        .ok_or("fixture service mapping missing")?
        .id;
    missing_mapping.field_mappings.retain(|field| {
        field.entity_mapping_id != service_mapping || field.target_field != "parent"
    });
    let assessment = assess_preparation(&missing_mapping, &original);
    assert!(assessment.batch.is_none());
    assert!(
        assessment
            .findings
            .iter()
            .any(|finding| finding.code == FindingCode::FieldMappingMissing
                && finding.entity.as_deref() == Some("ipam.services")
                && finding.field_path.as_deref() == Some("parent"))
    );
    Ok(())
}

#[test]
fn committed_mac_and_ip_discriminators_are_consumed_by_the_generic_reference_binder() -> TestResult
{
    let manifest: Value = serde_json::from_str(MANIFEST)?;
    let provider = Uuid::from_u128(1);
    let config = fixture_mapping_document(
        &manifest,
        provider,
        MappingDirection::Pull,
        MappingTarget::LocalInventory,
        ConflictMode::NetboxWins,
    )?;
    let original = fixture_snapshot(&manifest, provider)?;
    assert!(assess_preparation(&config, &original).batch.is_some());
    for entity in ["dcim.mac-addresses", "ipam.ip-addresses"] {
        for token in [
            None,
            Some(json!("CANARY-private-type")),
            Some(json!("dcim.site")),
        ] {
            let mut snapshot = original.clone();
            let record = snapshot
                .records
                .iter_mut()
                .find(|record| record.entity().as_str() == entity)
                .ok_or("fixture generic-reference record missing")?;
            if let Some(token) = token {
                record.fields.insert("assigned_object_type".into(), token);
            } else {
                record.fields.remove("assigned_object_type");
            }
            let assessment = assess_preparation(&config, &snapshot);
            assert!(assessment.batch.is_none());
            assert!(
                assessment
                    .findings
                    .iter()
                    .any(|finding| finding.code == FindingCode::FieldValueInvalid
                        && finding.entity.as_deref() == Some(entity)
                        && finding.field_path.as_deref() == Some("assigned_object"))
            );
            assert!(
                !assessment
                    .findings
                    .iter()
                    .any(|finding| finding.code == FindingCode::FieldUnsupported
                        && finding.entity.as_deref() == Some(entity)
                        && finding.field_path.as_deref() == Some("assigned_object_type"))
            );
            assert!(!serde_json::to_string(&assessment.findings)?.contains("CANARY"));
        }
    }
    Ok(())
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
    // The direct-service path speaks for the seeded test admin, so the writer's
    // commit boundary and reference READ run against the real policy evaluator
    // and token owner instead of a permissive double. The admin grants every
    // action on every resource type, so the assertions below stay about
    // atomicity and terminal job state, not about authorization.
    let admin_user_id: Uuid =
        sqlx::query_scalar("SELECT id FROM identity_user WHERE username = $1")
            .bind(TEST_ADMIN_USERNAME)
            .fetch_one(&state.db_pool)
            .await?;
    let request_context = RequestContext::new(
        Some(admin_user_id),
        "session".to_string(),
        None,
        Uuid::now_v7().to_string(),
        RequestAuth::default(),
    );
    let service = InventoryPullService::new(
        state.repositories.sync_mapping_configurations.clone(),
        state.repositories.sync_jobs.clone(),
        source,
        target,
        state.redis.clone(),
        60,
        Arc::new(PolicyRequestCommitAuthority::new(
            state.services.policy_evaluation.clone(),
            state.services.user_api_tokens.clone(),
            Arc::new(state.repositories.users.clone()),
        )),
    );
    service
        .run(provider_id, &reference.to_wire(), request_context)
        .await
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
        // Provider responses can carry credentials; report only the status.
        return Err(format!("provider create returned {status}").into());
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

async fn site_source_dates(
    pool: &sqlx::PgPool,
    provider_id: Uuid,
) -> Result<(Option<DateTime<Utc>>, Option<DateTime<Utc>>), TestError> {
    Ok(sqlx::query_as(
        "SELECT source_created_at, source_updated_at FROM sync_inventory_binding WHERE source_provider_id = $1 AND source_entity = 'dcim.sites' AND external_id = '1'",
    )
    .bind(provider_id)
    .fetch_one(pool)
    .await?)
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

fn captured_hardware_manifest() -> Result<Value, TestError> {
    macro_rules! page {
        ($resource:literal) => {
            (
                $resource,
                include_str!(concat!(
                    "fixtures/netbox/captured-hardware-20261027/authentic/full-",
                    $resource,
                    "-page-0.json"
                )),
            )
        };
    }
    let pages = [
        page!("rack-groups"),
        page!("cable-bundles"),
        page!("module-bay-types"),
        page!("cooling-sources"),
        page!("cooling-feeds"),
        page!("cooling-intakes"),
        page!("cooling-outflows"),
        page!("cooling-intake-templates"),
        page!("cooling-outflow-templates"),
        page!("locations"),
        page!("racks"),
        page!("cables"),
        page!("module-types"),
        page!("module-bay-templates"),
        page!("module-bays"),
        page!("modules"),
        page!("interfaces"),
    ];
    let mut resources = serde_json::Map::new();
    let mut count = 0;
    for (resource, raw) in pages {
        let page: Value = serde_json::from_str(raw)?;
        let records = page["results"]
            .as_array()
            .ok_or("capture results missing")?;
        assert!(page["next"].is_null());
        assert_eq!(page["count"].as_u64(), Some(records.len() as u64));
        count += records.len();
        resources.insert(
            format!("dcim.{resource}"),
            json!({
                "api_path": format!("/api/dcim/{resource}/"), "records": records
            }),
        );
    }
    assert_eq!(resources.len(), 17);
    assert_eq!(count, 25);
    Ok(json!({"resources": resources}))
}

async fn bind_captured_prerequisite(
    pool: &sqlx::PgPool,
    provider: Uuid,
    entity: InventoryEntity,
    external_id: &Value,
    local_id: Uuid,
) -> TestResult {
    let external_id = external_id
        .as_u64()
        .ok_or("prerequisite source id missing")?
        .to_string();
    sqlx::query("INSERT INTO sync_inventory_binding (source_provider_id, source_entity, external_id, target_entity, local_id) VALUES ($1,$2,$3,$4,$5)")
        .bind(provider).bind(entity.source_key()).bind(external_id)
        .bind(entity.target_key()).bind(local_id).execute(pool).await?;
    Ok(())
}

/// Native prerequisites, not an import of the incomplete prerequisite captures.
/// Only the site/manufacturer/device/MAC identities referenced by the unchanged
/// hardware maps are prebound. Roles and types are real native prerequisite rows;
/// no UUID stands in for a missing target. Region, tenant, platform and IP source
/// graphs are deliberately not claimed as imported or fabricated here.
async fn seed_captured_hardware_prerequisites(state: &AppState, provider: Uuid) -> TestResult {
    let sites: Value = serde_json::from_str(include_str!(
        "fixtures/netbox/captured-hardware-20261027/authentic/01-get-sites.json"
    ))?;
    let manufacturers: Value = serde_json::from_str(include_str!(
        "fixtures/netbox/captured-hardware-20261027/authentic/02-get-manufacturers.json"
    ))?;
    let devices: Value = serde_json::from_str(include_str!(
        "fixtures/netbox/captured-hardware-20261027/authentic/03-get-devices.json"
    ))?;
    for raw in sites["results"].as_array().ok_or("sites missing")? {
        let row = Site {
            id: Uuid::now_v7(),
            name: raw["name"].as_str().ok_or("site name missing")?.into(),
            slug: Some(raw["slug"].as_str().ok_or("site slug missing")?.into()),
            status: Some("active".into()),
            ..Site::default()
        };
        state.repositories.sites.create(&row).await?;
        bind_captured_prerequisite(
            &state.db_pool,
            provider,
            InventoryEntity::Site,
            &raw["id"],
            row.id,
        )
        .await?;
    }
    for raw in manufacturers["results"]
        .as_array()
        .ok_or("manufacturers missing")?
    {
        let row = Manufacturer {
            id: Uuid::now_v7(),
            name: raw["name"]
                .as_str()
                .ok_or("manufacturer name missing")?
                .into(),
            slug: raw["slug"]
                .as_str()
                .ok_or("manufacturer slug missing")?
                .into(),
            ..Manufacturer::default()
        };
        state.repositories.manufacturers.create(&row).await?;
        bind_captured_prerequisite(
            &state.db_pool,
            provider,
            InventoryEntity::Manufacturer,
            &raw["id"],
            row.id,
        )
        .await?;
    }
    for raw in devices["results"].as_array().ok_or("devices missing")? {
        let role = DeviceRole {
            id: Uuid::now_v7(),
            name: raw["role"]["name"]
                .as_str()
                .ok_or("role name missing")?
                .into(),
            slug: Some(
                raw["role"]["slug"]
                    .as_str()
                    .ok_or("role slug missing")?
                    .into(),
            ),
            ..DeviceRole::default()
        };
        state.repositories.device_roles.create(&role).await?;
        let manufacturer = binding_local_id(
            &state.db_pool,
            provider,
            "dcim.manufacturers",
            &raw["device_type"]["manufacturer"]["id"].to_string(),
        )
        .await?;
        let device_type = DeviceType {
            id: Uuid::now_v7(),
            manufacturer_id: manufacturer,
            model: raw["device_type"]["model"]
                .as_str()
                .ok_or("device model missing")?
                .into(),
            slug: raw["device_type"]["slug"]
                .as_str()
                .ok_or("device type slug missing")?
                .into(),
            u_height: 1.0,
            ..DeviceType::default()
        };
        state.repositories.device_types.create(&device_type).await?;
        let site = binding_local_id(
            &state.db_pool,
            provider,
            "dcim.sites",
            &raw["site"]["id"].to_string(),
        )
        .await?;
        let device = Device {
            id: Uuid::now_v7(),
            name: raw["name"].as_str().ok_or("device name missing")?.into(),
            site_id: Some(site),
            role_id: Some(role.id),
            device_type_id: Some(device_type.id),
            status: Some("active".into()),
            ..Device::default()
        };
        state.repositories.devices.create(&device).await?;
        bind_captured_prerequisite(
            &state.db_pool,
            provider,
            InventoryEntity::Device,
            &raw["id"],
            device.id,
        )
        .await?;
    }
    let manifest = captured_hardware_manifest()?;
    for raw in manifest["resources"]["dcim.interfaces"]["records"]
        .as_array()
        .ok_or("interfaces missing")?
    {
        let source = &raw["primary_mac_address"];
        let mac = MacAddress {
            id: Uuid::now_v7(),
            mac_address: source["mac_address"].as_str().ok_or("MAC missing")?.into(),
            ..MacAddress::default()
        };
        state.repositories.mac_addresses.create(&mac).await?;
        bind_captured_prerequisite(
            &state.db_pool,
            provider,
            InventoryEntity::MacAddress,
            &source["id"],
            mac.id,
        )
        .await?;
    }
    Ok(())
}

async fn captured_native_rows(pool: &sqlx::PgPool, manifest: &Value) -> Result<Value, TestError> {
    let mut rows = serde_json::Map::new();
    for key in source_entity_keys(manifest) {
        let entity = InventoryEntity::from_source_key(&key).ok_or("unknown captured model")?;
        let table = entity.table().ok_or("captured model has no table")?;
        // Table names come only from the production closed entity catalogue.
        let query = format!(
            "SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY id), '[]'::jsonb) FROM {table} t"
        );
        rows.insert(
            key,
            sqlx::query_scalar::<_, Value>(&query)
                .fetch_one(pool)
                .await?,
        );
    }
    Ok(Value::Object(rows))
}

async fn assert_captured_hardware_rows(
    pool: &sqlx::PgPool,
    provider: Uuid,
    manifest: &Value,
) -> TestResult {
    for key in source_entity_keys(manifest) {
        let entity = InventoryEntity::from_source_key(&key).ok_or("unknown captured entity")?;
        let table = entity.table().ok_or("captured table missing")?;
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(pool)
            .await?;
        assert_eq!(
            Some(count as u64),
            manifest["resources"][&key]["records"]
                .as_array()
                .map(|rows| rows.len() as u64),
            "{key} must not materialize extra unbound rows"
        );
        for raw in manifest["resources"][&key]["records"]
            .as_array()
            .ok_or("captured records missing")?
        {
            let external_id = raw["id"].to_string();
            let id = binding_local_id(pool, provider, &key, &external_id).await?;
            let query = format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE id=$1)");
            assert!(
                sqlx::query_scalar::<_, bool>(&query)
                    .bind(id)
                    .fetch_one(pool)
                    .await?,
                "bound {key} row missing"
            );
            let (created, updated, target): (Option<DateTime<Utc>>, Option<DateTime<Utc>>, String) = sqlx::query_as("SELECT source_created_at, source_updated_at, target_entity FROM sync_inventory_binding WHERE source_provider_id=$1 AND source_entity=$2 AND external_id=$3")
                .bind(provider).bind(&key).bind(&external_id).fetch_one(pool).await?;
            assert_eq!(target, entity.target_key());
            assert_eq!(
                created,
                Some(
                    raw["created"]
                        .as_str()
                        .ok_or("created missing")?
                        .parse::<DateTime<Utc>>()?
                )
            );
            assert_eq!(
                updated,
                Some(
                    raw["last_updated"]
                        .as_str()
                        .ok_or("updated missing")?
                        .parse::<DateTime<Utc>>()?
                )
            );
            for name in [
                "cooling_capacity",
                "diameter",
                "max_flow",
                "length",
                "weight",
                "max_weight",
                "mtu",
                "speed",
                "rf_channel_frequency",
                "rf_channel_width",
            ] {
                if let Some(value) = raw.get(name) {
                    let field = entity
                        .field(name)
                        .ok_or("captured measurement is not supported")?;
                    let column = field.column();
                    let expected = (!value.is_null()).then(|| value.to_string());
                    let query = format!(
                        "SELECT {column} IS NOT DISTINCT FROM $2::text::numeric FROM {table} WHERE id=$1"
                    );
                    let exact: bool = sqlx::query_scalar(&query)
                        .bind(id)
                        .bind(expected)
                        .fetch_one(pool)
                        .await?;
                    assert!(
                        exact,
                        "{key}.{name} must preserve its exact captured measurement/null"
                    );
                }
            }
            for name in [
                "diameter_unit",
                "max_flow_unit",
                "length_unit",
                "weight_unit",
            ] {
                if let Some(value) = raw.get(name) {
                    let column = entity
                        .field(name)
                        .ok_or("captured unit is not supported")?
                        .column();
                    let expected = value.get("value").unwrap_or(value).as_str();
                    let query = format!(
                        "SELECT {column} IS NOT DISTINCT FROM $2::text FROM {table} WHERE id=$1"
                    );
                    assert!(
                        sqlx::query_scalar::<_, bool>(&query)
                            .bind(id)
                            .bind(expected)
                            .fetch_one(pool)
                            .await?,
                        "{key}.{name} must preserve the captured unit"
                    );
                }
            }
        }
    }
    let cable = binding_local_id(pool, provider, "dcim.cables", "1").await?;
    for (external, side) in [("2", "A"), ("3", "B")] {
        let interface = binding_local_id(pool, provider, "dcim.interfaces", external).await?;
        let actual: (Option<Uuid>, Option<String>) =
            sqlx::query_as("SELECT cable_id, cable_end FROM dcim_interface WHERE id=$1")
                .bind(interface)
                .fetch_one(pool)
                .await?;
        assert_eq!(actual, (Some(cable), Some(side.into())));
    }
    let module_type = binding_local_id(pool, provider, "dcim.module-types", "1").await?;
    let actual: (Option<chrono::NaiveDate>, Option<Value>) =
        sqlx::query_as("SELECT end_of_life, attribute_data FROM dcim_moduletype WHERE id=$1")
            .bind(module_type)
            .fetch_one(pool)
            .await?;
    assert_eq!(
        actual,
        (
            Some(chrono::NaiveDate::from_ymd_opt(2030, 2, 28).ok_or("invalid expected date")?),
            None
        )
    );
    for (external, bays) in [("1", 2_i64), ("2", 0_i64)] {
        let device = binding_local_id(pool, provider, "dcim.devices", external).await?;
        let counts: (i64, i64) =
            sqlx::query_as("SELECT interface_count, module_bay_count FROM dcim_device WHERE id=$1")
                .bind(device)
                .fetch_one(pool)
                .await?;
        assert_eq!(
            counts,
            (2, bays),
            "native counts must describe the actual imported graph"
        );
    }
    Ok(())
}

/// The unmodified 17-model/25-record cohort crosses the authenticated API and
/// production source/target. A deferred native database fault fires at COMMIT,
/// after row writes, tree/count rebuilds and the tentative Completed update.
#[tokio::test]
#[ignore = "requires fresh owned PostgreSQL and Redis; Main owns fixture provisioning and execution"]
async fn authenticated_captured_hardware_pull_commits_and_rolls_back_atomically() -> TestResult {
    let _guard = inventory_pull_test_lock().clone().lock_owned().await;
    let state = build_test_app_state().await?;
    let manifest = captured_hardware_manifest()?;
    let empty = captured_native_rows(&state.db_pool, &manifest).await?;
    assert!(
        empty
            .as_object()
            .ok_or("native rows missing")?
            .values()
            .all(|rows| rows == &json!([])),
        "captured acceptance requires a fresh owned hardware target"
    );
    let mut wire_manifest = manifest.clone();
    // The production adapter requires the whole approved registry. Unselected
    // models are advertised as empty, never replaced by prerequisite captures.
    for resource in LocalInventoryCatalog::new(state.db_pool.clone()).resources() {
        let key = resource.source_entity.as_str();
        wire_manifest["resources"]
            .as_object_mut()
            .ok_or("resources missing")?
            .entry(key.to_string())
            .or_insert_with(|| {
                json!({
                    "api_path": format!("/api/{}/", key.replace('.', "/")), "records": []
                })
            });
    }
    let mock = spawn_mock_manifest(&wire_manifest, unique_suffix()).await?;
    let router = piping_backend::app::build_router(state.clone());
    let cookie = login(&router).await?;
    let provider = create_provider_via_api(&router, &cookie, &mock.base_url).await?;
    seed_captured_hardware_prerequisites(&state, provider).await?;
    let prerequisite_bindings = binding_count(&state.db_pool, provider).await?;
    assert_eq!(prerequisite_bindings, 10);
    let reference = seed_mapping_config(
        &state.repositories.sync_mapping_configurations,
        &wire_manifest,
        provider,
        MappingDirection::Pull,
        MappingTarget::LocalInventory,
        ConflictMode::NetboxWins,
    )
    .await?;
    let before_devices: Value =
        sqlx::query_scalar("SELECT jsonb_agg(to_jsonb(d) ORDER BY id) FROM dcim_device d")
            .fetch_one(&state.db_pool)
            .await?;
    let fault = format!("captured_hardware_fault_{}", Uuid::now_v7().simple());
    let sequence = format!("{fault}_seen");
    sqlx::query(&format!("CREATE SEQUENCE {sequence}"))
        .execute(&state.db_pool)
        .await?;
    sqlx::query(&format!("CREATE FUNCTION {fault}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM nextval('{sequence}'); RAISE EXCEPTION 'captured hardware deferred target failure' USING ERRCODE = '23514'; END $$"))
        .execute(&state.db_pool).await?;
    sqlx::query(&format!("CREATE CONSTRAINT TRIGGER {fault} AFTER INSERT ON dcim_interface DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION {fault}()"))
        .execute(&state.db_pool).await?;
    let failed = tokio::time::timeout(
        Duration::from_secs(60),
        post_sync_job(&router, &cookie, provider, &reference),
    )
    .await;
    // Remove only this test's fault even when the API operation returns an error.
    sqlx::query(&format!("DROP TRIGGER {fault} ON dcim_interface"))
        .execute(&state.db_pool)
        .await?;
    sqlx::query(&format!("DROP FUNCTION {fault}()"))
        .execute(&state.db_pool)
        .await?;
    let fired: bool = sqlx::query_scalar(&format!("SELECT is_called FROM {sequence}"))
        .fetch_one(&state.db_pool)
        .await?;
    sqlx::query(&format!("DROP SEQUENCE {sequence}"))
        .execute(&state.db_pool)
        .await?;
    let (status, body) = failed.map_err(|_| "captured failed job timed out")??;
    // The REST validation message may contain a structured preflight report.
    // Print only its typed finding codes and catalog-owned (or hashed) paths;
    // never print the response message or source records.
    let report = body["message"]
        .as_str()
        .and_then(|message| serde_json::from_str::<Value>(message).ok());
    let stage = match report.as_ref().and_then(|report| report["code"].as_str()) {
        Some("inventory_preflight:preparation_blocked") => "preparation_blocked",
        Some("inventory_preflight:target_graph_blocked") => "target_graph_blocked",
        _ => "other or absent",
    };
    let findings: Vec<_> = report
        .and_then(|report| serde_json::from_value::<Vec<Finding>>(report["findings"].clone()).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|finding| (finding.code, finding.field_path))
        .collect();
    let code = match body["code"].as_str() {
        Some("VALIDATION_ERROR") => "VALIDATION_ERROR",
        Some("DATABASE_ERROR") => "DATABASE_ERROR",
        Some("AUTH_ERROR") => "AUTH_ERROR",
        Some("FORBIDDEN") => "FORBIDDEN",
        Some("INTERNAL_ERROR") => "INTERNAL_ERROR",
        _ => "other or absent",
    };
    assert!(
        fired,
        "the failure must reach deferred target COMMIT, not stop in preparation; HTTP status: {status}, code: {code}, stage: {stage}, findings: {findings:?}"
    );
    assert!(status.is_server_error());
    assert_ne!(body["status"].as_str(), Some("completed"));
    assert_eq!(
        captured_native_rows(&state.db_pool, &manifest).await?,
        empty
    );
    let after_devices: Value =
        sqlx::query_scalar("SELECT jsonb_agg(to_jsonb(d) ORDER BY id) FROM dcim_device d")
            .fetch_one(&state.db_pool)
            .await?;
    assert_eq!(
        after_devices, before_devices,
        "derived prerequisite counts must roll back too"
    );
    assert_eq!(
        binding_count(&state.db_pool, provider).await?,
        prerequisite_bindings
    );
    assert_eq!(completed_jobs(&state.db_pool, provider).await?, 0);
    assert_eq!(failed_jobs(&state.db_pool, provider).await?, 1);

    let mut first_ids = None;
    for expected_completed in 1..=2 {
        let (status, body) = tokio::time::timeout(
            Duration::from_secs(60),
            post_sync_job(&router, &cookie, provider, &reference),
        )
        .await
        .map_err(|_| "captured positive job timed out")??;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "completed");
        assert_eq!(body["stats"]["extracted"], 25);
        assert_eq!(body["stats"]["committed"], 25);
        assert_eq!(
            binding_count(&state.db_pool, provider).await?,
            prerequisite_bindings + 25
        );
        assert_eq!(
            completed_jobs(&state.db_pool, provider).await?,
            expected_completed
        );
        assert_captured_hardware_rows(&state.db_pool, provider, &manifest).await?;
        let ids: Value = sqlx::query_scalar("SELECT jsonb_agg(jsonb_build_array(source_entity,external_id,target_entity,local_id) ORDER BY source_entity,external_id,target_entity) FROM sync_inventory_binding WHERE source_provider_id=$1")
            .bind(provider).fetch_one(&state.db_pool).await?;
        if let Some(original) = &first_ids {
            assert_eq!(
                &ids, original,
                "rerun must reuse every provider-qualified identity"
            );
        } else {
            first_ids = Some(ids);
        }
    }

    // Follow-up snapshots intentionally vary presence only after the unchanged
    // capture and its idempotent rerun have passed. Omission is not detachment.
    let mut cables = mock.records("dcim.cables").await;
    cables[0]
        .as_object_mut()
        .ok_or("cable missing")?
        .remove("a_terminations");
    cables[0]
        .as_object_mut()
        .ok_or("cable missing")?
        .remove("b_terminations");
    mock.replace_records("dcim.cables", cables.clone()).await;
    let mut interfaces = mock.records("dcim.interfaces").await;
    for interface in &mut interfaces {
        if [json!(2), json!(3)].contains(&interface["id"]) {
            let fields = interface.as_object_mut().ok_or("interface missing")?;
            fields.remove("cable");
            fields.remove("cable_end");
        }
    }
    mock.replace_records("dcim.interfaces", interfaces).await;
    let (status, body) = post_sync_job(&router, &cookie, provider, &reference).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "completed");
    assert_captured_hardware_rows(&state.db_pool, provider, &manifest).await?;

    // Explicit empty A-side agrees with an explicit forward clear of Interface
    // 2. B remains attached and its omitted reverse side imposes no new claim.
    cables[0]["a_terminations"] = json!([]);
    mock.replace_records("dcim.cables", cables).await;
    let mut interfaces = mock.records("dcim.interfaces").await;
    let endpoint = interfaces
        .iter_mut()
        .find(|row| row["id"] == 2)
        .ok_or("A endpoint missing")?;
    endpoint["cable"] = Value::Null;
    endpoint["cable_end"] = Value::Null;
    mock.replace_records("dcim.interfaces", interfaces).await;
    let (status, body) = post_sync_job(&router, &cookie, provider, &reference).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "completed");
    let cable = binding_local_id(&state.db_pool, provider, "dcim.cables", "1").await?;
    for (external, expected) in [
        ("2", (None, None)),
        ("3", (Some(cable), Some("B".to_string()))),
    ] {
        let id = binding_local_id(&state.db_pool, provider, "dcim.interfaces", external).await?;
        let association: (Option<Uuid>, Option<String>) =
            sqlx::query_as("SELECT cable_id, cable_end FROM dcim_interface WHERE id=$1")
                .bind(id)
                .fetch_one(&state.db_pool)
                .await?;
        assert_eq!(association, expected);
    }
    assert_eq!(
        binding_count(&state.db_pool, provider).await?,
        prerequisite_bindings + 25
    );
    assert_eq!(completed_jobs(&state.db_pool, provider).await?, 4);
    assert_eq!(failed_jobs(&state.db_pool, provider).await?, 1);
    assert!(mock.data_get_count() > 0);
    assert_eq!(mock.write_count(), 0);
    Ok(())
}

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
    let tag_id = binding_local_id(pool, provider_id, "extras.tags", "1").await?;
    assert_eq!(
        site_tags,
        vec![tag_id.to_string()],
        "membership uses the bound catalog UUID"
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

    let (service_parent, service_parent_type, port_mappings): (Uuid, Uuid, Vec<String>) =
        sqlx::query_as(
            "SELECT parent_object_id, parent_object_type_id, port_mappings FROM ipam_service WHERE id = $1",
        )
        .bind(service_id)
        .fetch_one(pool)
        .await?;
    let device_content_type = content_type_id(pool, "dcim", "device").await?;
    assert_eq!(service_parent, device_id);
    assert_eq!(service_parent_type, device_content_type);
    assert_eq!(port_mappings, vec!["tcp/443", "tcp/80"]);

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

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis; run with `cargo test --test integration_inventory_pull -- --ignored`"]
async fn source_timestamps_round_trip_without_overwriting_local_dates() -> TestResult {
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

    let mut sites = mock.records("dcim.sites").await;
    let fields = sites[0].as_object_mut().ok_or("site object missing")?;
    fields.remove("created");
    fields.remove("last_updated");
    mock.replace_records("dcim.sites", sites).await;
    run_pull_direct(&state, &mock, provider_id, &reference).await?;
    assert_eq!(
        site_source_dates(&state.db_pool, provider_id).await?,
        (None, None)
    );
    let site_id = binding_local_id(&state.db_pool, provider_id, "dcim.sites", "1").await?;
    let local_synced: Option<DateTime<Utc>> =
        Some(serde_json::from_value(json!("2025-01-01T00:00:00Z"))?);
    sqlx::query("UPDATE dcim_site SET piping_synced_at = $2 WHERE id = $1")
        .bind(site_id)
        .bind(local_synced)
        .execute(&state.db_pool)
        .await?;

    let mut sites = mock.records("dcim.sites").await;
    sites[0]["created"] = json!("2020-02-29T01:02:03.123456000000000+03:00");
    sites[0]["last_updated"] = json!("2020-03-01T00:00:00Z");
    mock.replace_records("dcim.sites", sites).await;
    run_pull_direct(&state, &mock, provider_id, &reference).await?;
    let mut expected: (Option<DateTime<Utc>>, Option<DateTime<Utc>>) = (
        Some(serde_json::from_value(json!(
            "2020-02-28T22:02:03.123456Z"
        ))?),
        Some(serde_json::from_value(json!("2020-03-01T00:00:00Z"))?),
    );
    assert_eq!(
        site_source_dates(&state.db_pool, provider_id).await?,
        expected
    );
    let retained_synced: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT piping_synced_at FROM dcim_site WHERE id = $1")
            .bind(site_id)
            .fetch_one(&state.db_pool)
            .await?;
    assert_eq!(retained_synced, local_synced);
    assert_ne!(retained_synced, expected.0);
    assert_ne!(retained_synced, expected.1);

    let mut sites = mock.records("dcim.sites").await;
    let fields = sites[0].as_object_mut().ok_or("site object missing")?;
    fields.remove("created");
    fields.remove("last_updated");
    mock.replace_records("dcim.sites", sites).await;
    run_pull_direct(&state, &mock, provider_id, &reference).await?;
    assert_eq!(
        site_source_dates(&state.db_pool, provider_id).await?,
        expected
    );

    let mut sites = mock.records("dcim.sites").await;
    sites[0]["last_updated"] = json!("2020-04-01T00:00:00Z");
    mock.replace_records("dcim.sites", sites).await;
    run_pull_direct(&state, &mock, provider_id, &reference).await?;
    expected.1 = Some(serde_json::from_value(json!("2020-04-01T00:00:00Z"))?);
    assert_eq!(
        site_source_dates(&state.db_pool, provider_id).await?,
        expected
    );

    let mut sites = mock.records("dcim.sites").await;
    sites[0]["created"] = Value::Null;
    sites[0]
        .as_object_mut()
        .ok_or("site object missing")?
        .remove("last_updated");
    mock.replace_records("dcim.sites", sites).await;
    run_pull_direct(&state, &mock, provider_id, &reference).await?;
    let cleared = (None, expected.1);
    assert_eq!(
        site_source_dates(&state.db_pool, provider_id).await?,
        cleared
    );

    let original_name: String = sqlx::query_scalar("SELECT name FROM dcim_site WHERE id = $1")
        .bind(site_id)
        .fetch_one(&state.db_pool)
        .await?;
    let original_binding_count = binding_count(&state.db_pool, provider_id).await?;
    let mut sites = mock.records("dcim.sites").await;
    sites[0]["name"] = json!(format!("must-not-commit-{}", mock.suffix));
    sites[0]["last_updated"] = json!("invalid-source-date");
    mock.replace_records("dcim.sites", sites).await;
    assert!(
        run_pull_direct(&state, &mock, provider_id, &reference)
            .await
            .is_err()
    );
    assert_eq!(
        site_source_dates(&state.db_pool, provider_id).await?,
        cleared
    );
    assert_eq!(
        binding_count(&state.db_pool, provider_id).await?,
        original_binding_count
    );
    let retained_name: String = sqlx::query_scalar("SELECT name FROM dcim_site WHERE id = $1")
        .bind(site_id)
        .fetch_one(&state.db_pool)
        .await?;
    assert_eq!(retained_name, original_name);
    let retained_synced: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT piping_synced_at FROM dcim_site WHERE id = $1")
            .bind(site_id)
            .fetch_one(&state.db_pool)
            .await?;
    assert_eq!(retained_synced, local_synced);
    assert_eq!(
        binding_local_id(&state.db_pool, provider_id, "dcim.sites", "1").await?,
        site_id
    );
    assert_eq!(mock.write_count(), 0);
    Ok(())
}

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis; run with `cargo test --test integration_inventory_pull -- --ignored`"]
async fn bound_custom_field_keeps_an_arbitrary_local_uuid_on_rerun() -> TestResult {
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
    let source = mock.records("extras.custom-fields").await;
    let external_id = source[0]["id"].to_string();
    let original_id = binding_local_id(
        &state.db_pool,
        provider_id,
        "extras.custom-fields",
        &external_id,
    )
    .await?;
    let arbitrary_id = Uuid::now_v7();
    let mut transaction = state.db_pool.begin().await?;
    let name: String = sqlx::query_scalar("SELECT name FROM custom_field WHERE id=$1")
        .bind(original_id)
        .fetch_one(&mut *transaction)
        .await?;
    // Seed an existing source-owned definition whose UUID was not generated by
    // this importer. All metadata and scope relations stay intact.
    sqlx::query("UPDATE custom_field SET name=$2 WHERE id=$1")
        .bind(original_id)
        .bind(format!("retired_{}", original_id.simple()))
        .execute(&mut *transaction)
        .await?;
    sqlx::query("INSERT INTO custom_field SELECT (jsonb_populate_record(NULL::custom_field, to_jsonb(original) || jsonb_build_object('id',$2::uuid,'name',$3::text))).* FROM custom_field original WHERE id=$1")
        .bind(original_id).bind(arbitrary_id).bind(&name).execute(&mut *transaction).await?;
    sqlx::query("UPDATE custom_field_object_type SET custom_field_id=$2 WHERE custom_field_id=$1")
        .bind(original_id)
        .bind(arbitrary_id)
        .execute(&mut *transaction)
        .await?;
    sqlx::query("DELETE FROM custom_field WHERE id=$1")
        .bind(original_id)
        .execute(&mut *transaction)
        .await?;
    let rebound = sqlx::query("UPDATE sync_inventory_binding SET local_id=$4 WHERE source_provider_id=$1 AND source_entity='extras.custom-fields' AND external_id=$2 AND local_id=$3")
        .bind(provider_id).bind(&external_id).bind(original_id).bind(arbitrary_id)
        .execute(&mut *transaction).await?;
    assert_eq!(rebound.rows_affected(), 1);
    transaction.commit().await?;

    run_pull_direct(&state, &mock, provider_id, &reference).await?;
    assert_eq!(
        binding_local_id(
            &state.db_pool,
            provider_id,
            "extras.custom-fields",
            &external_id,
        )
        .await?,
        arbitrary_id,
        "the persisted custom-field binding wins over a newly derived UUID"
    );
    let retained: Uuid = sqlx::query_scalar("SELECT id FROM custom_field WHERE name=$1")
        .bind(name)
        .fetch_one(&state.db_pool)
        .await?;
    assert_eq!(retained, arbitrary_id);
    let restored_original: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT FROM custom_field WHERE id=$1)")
            .bind(original_id)
            .fetch_one(&state.db_pool)
            .await?;
    assert!(!restored_original);
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
        "created": "2020-01-01T00:00:00Z",
        "last_updated": null,
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
        "created": "2021-01-01T00:00:00Z",
        "last_updated": "2022-01-01T00:00:00Z",
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
    assert_eq!(
        serde_json::to_value(site_source_dates(&state.db_pool, provider_a).await?)?,
        json!(["2020-01-01T00:00:00Z", null]),
    );
    assert_eq!(
        serde_json::to_value(site_source_dates(&state.db_pool, provider_b).await?)?,
        json!(["2021-01-01T00:00:00Z", "2022-01-01T00:00:00Z"]),
    );

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
    let findings = pull_validation_findings(&error, "inventory_preflight:preparation_blocked");
    assert!(
        findings
            .iter()
            .any(|finding| finding.code == FindingCode::FieldUnsupported
                && finding.entity.as_deref() == Some("dcim.sites")
                && finding.record_identity_sha256.as_deref() == Some(digest(b"1").as_str())
                && finding.field_path.as_deref()
                    == Some(format!("sha256:{}", digest(b"mystery_business_field")).as_str()))
    );
    assert!(!error.to_string().contains("must not disappear"));
    assert_eq!(binding_count(&state.db_pool, provider_id).await?, 0);
    assert_eq!(mock.write_count(), 0);
    Ok(())
}

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn tag_uuid_binding_rename_omission_clear_and_invalid_input_are_atomic() -> TestResult {
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
    let pool = &state.db_pool;
    let site_id = binding_local_id(pool, provider_id, "dcim.sites", "1").await?;
    let original_id = binding_local_id(pool, provider_id, "extras.tags", "1").await?;
    let arbitrary_id = Uuid::now_v7();
    let mut transaction = pool.begin().await?;
    let slug: String = sqlx::query_scalar("SELECT slug FROM inventory_tag WHERE id=$1")
        .bind(original_id)
        .fetch_one(&mut *transaction)
        .await?;
    sqlx::query("UPDATE inventory_tag SET slug=$2 WHERE id=$1")
        .bind(original_id)
        .bind(format!("retired-{}", original_id.simple()))
        .execute(&mut *transaction)
        .await?;
    // Preserve the old catalog row and its current references while seeding a
    // source-owned binding whose UUID was not allocated by this importer.
    sqlx::query("INSERT INTO inventory_tag SELECT (jsonb_populate_record(NULL::inventory_tag, to_jsonb(original) || jsonb_build_object('id',$2::uuid,'slug',$3::text))).* FROM inventory_tag original WHERE id=$1")
        .bind(original_id).bind(arbitrary_id).bind(slug).execute(&mut *transaction).await?;
    let rebound = sqlx::query("UPDATE sync_inventory_binding SET local_id=$3 WHERE source_provider_id=$1 AND source_entity='extras.tags' AND external_id='1' AND local_id=$2")
        .bind(provider_id).bind(original_id).bind(arbitrary_id).execute(&mut *transaction).await?;
    assert_eq!(rebound.rows_affected(), 1);
    transaction.commit().await?;

    let mut definitions = mock.records("extras.tags").await;
    definitions[0]["name"] = json!(format!("Renamed-{}", mock.suffix));
    definitions[0]["slug"] = json!(format!("renamed-{}", mock.suffix));
    mock.replace_records("extras.tags", definitions).await;
    run_pull_direct(&state, &mock, provider_id, &reference).await?;
    assert_eq!(
        binding_local_id(pool, provider_id, "extras.tags", "1").await?,
        arbitrary_id
    );
    let tags: Vec<String> = sqlx::query_scalar("SELECT tags FROM dcim_site WHERE id=$1")
        .bind(site_id)
        .fetch_one(pool)
        .await?;
    assert_eq!(tags, vec![arbitrary_id.to_string()]);

    let mut sites = mock.records("dcim.sites").await;
    let site = sites
        .iter_mut()
        .find(|row| row["id"] == 1)
        .ok_or("site missing")?;
    site.as_object_mut()
        .ok_or("site is not an object")?
        .remove("tags");
    mock.replace_records("dcim.sites", sites.clone()).await;
    let definitions = mock.records("extras.tags").await;
    let mut with_second_tag = definitions.clone();
    let mut second_tag = definitions[0].clone();
    second_tag["id"] = json!(2);
    second_tag["name"] = json!(format!("Second-{}", mock.suffix));
    second_tag["slug"] = json!(format!("second-{}", mock.suffix));
    with_second_tag.push(second_tag);
    mock.replace_records("extras.tags", with_second_tag).await;
    let mut memberships = mock.records("extras.tagged-objects").await;
    for edge in &mut memberships {
        if edge["object_type"] == "dcim.site" && edge["object_id"] == 1 {
            edge["tag"] = json!({"id": 2});
        }
    }
    mock.replace_records("extras.tagged-objects", memberships.clone())
        .await;
    let completed_before = completed_jobs(pool, provider_id).await?;
    let bindings_before = binding_count(pool, provider_id).await?;
    let error = run_pull_direct(&state, &mock, provider_id, &reference)
        .await
        .expect_err("omission cannot prove a new positive edge matches the stored Tag1 array");
    let findings = pull_validation_findings(&error, "inventory_preflight:preparation_blocked");
    assert!(findings.iter().any(|finding| finding.code
        == FindingCode::AnalysisDependencyUnavailable
        && finding.field_path.as_deref() == Some("tags.graph.actual")));
    assert_eq!(completed_jobs(pool, provider_id).await?, completed_before);
    assert_eq!(binding_count(pool, provider_id).await?, bindings_before);
    let unchanged: Vec<String> = sqlx::query_scalar("SELECT tags FROM dcim_site WHERE id=$1")
        .bind(site_id)
        .fetch_one(pool)
        .await?;
    assert_eq!(unchanged, tags);
    // Only omission without a positively observed site edge may preserve.
    mock.replace_records("extras.tags", definitions).await;
    memberships.retain(|edge| !(edge["object_type"] == "dcim.site" && edge["object_id"] == 1));
    mock.replace_records("extras.tagged-objects", memberships)
        .await;
    run_pull_direct(&state, &mock, provider_id, &reference).await?;
    let preserved: Vec<String> = sqlx::query_scalar("SELECT tags FROM dcim_site WHERE id=$1")
        .bind(site_id)
        .fetch_one(pool)
        .await?;
    assert_eq!(preserved, tags);
    let initial_name: String = sqlx::query_scalar("SELECT name FROM dcim_site WHERE id=$1")
        .bind(site_id)
        .fetch_one(pool)
        .await?;
    for invalid in [
        json!(null),
        json!(["edge"]),
        json!([{"slug": "edge"}]),
        json!([{"id": 999999}]),
        json!([{"id": 1}, {"id": 1}]),
    ] {
        let site = sites
            .iter_mut()
            .find(|row| row["id"] == 1)
            .ok_or("site missing")?;
        site["tags"] = invalid;
        site["name"] = json!("must-not-commit-tag-failure");
        mock.replace_records("dcim.sites", sites.clone()).await;
        run_pull_direct(&state, &mock, provider_id, &reference)
            .await
            .expect_err("invalid tags must block the complete pull");
        let stored: (String, Vec<String>) =
            sqlx::query_as("SELECT name,tags FROM dcim_site WHERE id=$1")
                .bind(site_id)
                .fetch_one(pool)
                .await?;
        assert_eq!(stored, (initial_name.clone(), tags.clone()));
    }
    let site = sites
        .iter_mut()
        .find(|row| row["id"] == 1)
        .ok_or("site missing")?;
    site["tags"] = json!([]);
    site["name"] = json!(initial_name);
    mock.replace_records("dcim.sites", sites).await;
    let memberships = mock
        .records("extras.tagged-objects")
        .await
        .into_iter()
        .filter(|row| !(row["object_type"] == "dcim.site" && row["object_id"] == 1))
        .collect();
    mock.replace_records("extras.tagged-objects", memberships)
        .await;
    run_pull_direct(&state, &mock, provider_id, &reference).await?;
    let cleared: Vec<String> = sqlx::query_scalar("SELECT tags FROM dcim_site WHERE id=$1")
        .bind(site_id)
        .fetch_one(pool)
        .await?;
    assert!(cleared.is_empty());
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
    let findings = pull_validation_findings(&error, "inventory_preflight:target_graph_blocked");
    assert!(findings.iter().any(|finding| {
        finding.code == FindingCode::TargetDefinitionConflict
            && finding.entity.as_deref() == Some("extras.tags")
            && finding.field_path.as_deref() == Some("slug")
            && finding
                .edge
                .as_ref()
                .is_some_and(|edge| edge.target_local_id.is_some())
    }));
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
async fn source_service_null_port_mappings_blocks_without_mutation() -> TestResult {
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
    services[0]["port_mappings"] = Value::Null;
    mock.replace_records("ipam.services", services).await;
    assert!(
        run_pull_direct(&state, &mock, provider_id, &reference)
            .await
            .is_err()
    );
    let endpoints: Vec<String> =
        sqlx::query_scalar("SELECT port_mappings FROM ipam_service WHERE id=$1")
            .bind(service_id)
            .fetch_one(&state.db_pool)
            .await?;
    assert_eq!(
        endpoints,
        vec!["tcp/443", "tcp/80"],
        "invalid input must preserve stored endpoints"
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
/// name.
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
            assert!(row.get("protocol").is_none());
            assert!(row.get("ports").is_none());
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
    let findings = pull_validation_findings(&error, "inventory_preflight:target_graph_blocked");
    assert!(findings.iter().any(|finding| {
        finding.code == FindingCode::TargetReferenceMissing
            && finding.entity.as_deref() == Some("dcim.devices")
            && finding.record_identity_sha256.as_deref() == Some(digest(b"1").as_str())
            && finding.field_path.as_deref() == Some("site")
            && finding.edge.as_ref().is_some_and(|edge| {
                edge.target_entity.as_deref() == Some("dcim.sites")
                    && edge.target_identity_sha256.as_deref() == Some(digest(b"999999").as_str())
            })
    }));

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
