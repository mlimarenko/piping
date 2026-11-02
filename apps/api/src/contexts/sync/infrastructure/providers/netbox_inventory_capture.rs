//! GET-only assessment capture of the uncached source schema, not a migration.
//!
//! Every schema path receives an evidence disposition. Only exact two-segment
//! API collections with a paginated, integer-ID response schema can be read,
//! and each read requires an exact authority grant. Detail paths are tied to
//! their collection; actions and nested/plugin paths remain
//! explicit unresolved evidence. This is independent of local target support.
//! No OpenAPI description or successful page count proves full read authority,
//! stable source state or observable process coverage. The result is INCOMPLETE.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use chrono::Utc;
use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::contexts::sync::domain::inventory_export::{
    CaptureEvidence, CaptureOutcome, CaptureStatus, ConsistencyStatus, EXPORT_FORMAT_VERSION,
    ExportManifest, NETBOX_VERSION, ObservationStatus, PayloadDescriptor, ProcessArea,
    ProcessCoverage, ResourceCensus, SourceAuthority, digest,
};
use crate::contexts::sync::domain::{SourceEntity, SourceRecord};
use crate::errors::AppError;
use crate::integrations::netbox::client::CaptureGetResponse;
use crate::integrations::netbox::{NetboxClient, NetboxPage};

use super::immutable_inventory_source::seal_bundle;
use super::netbox_inventory_source::record_from_value;

fn failure(code: &'static str) -> AppError {
    AppError::External(format!("netbox_capture:{code}"))
}

/// Every record-collection read requires an exact grant and private authority
/// evidence, regardless of namespace or field names. This is not target support
/// and never upgrades consistency, visibility or process coverage.
#[derive(Serialize)]
pub struct CaptureOptions {
    pub page_size: usize,
    pub max_pages_per_resource: usize,
    pub collection_grants: BTreeSet<String>,
    pub authority_evidence: Option<Vec<u8>>,
}

impl Default for CaptureOptions {
    fn default() -> Self {
        Self {
            page_size: 100,
            max_pages_per_resource: 10_000,
            collection_grants: BTreeSet::new(),
            authority_evidence: None,
        }
    }
}

/// Private captured data. Do not Debug/log this result or its payload values.
pub struct NetboxInventoryCapture {
    pub manifest: ExportManifest,
    pub payloads: BTreeMap<String, Vec<u8>>,
}

impl NetboxInventoryCapture {
    /// Reuse the single private, no-clobber artifact writer.
    pub fn seal(&self, directory: &Path) -> Result<String, AppError> {
        seal_bundle(directory, &self.manifest, &self.payloads)
    }
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum PathReason {
    PaginatedIntegerIdCollection,
    CollectionDetail,
    CollectionActionUncaptured,
    MissingReadAuthority,
    NoGetOperation,
    UnsafeOrNoncanonicalPath,
    NestedOrPluginPath,
    UnsupportedResponseSchema,
}

#[derive(Serialize)]
struct PathEvidence {
    // Paths are private artifact evidence and may be untrusted. Errors and
    // reports must not interpolate them, even when they resemble identities.
    path: String,
    reason: PathReason,
    collection_path: Option<String>,
    capture: Option<ResourceEvidence>,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum ResourceFailure {
    HttpForbidden,
    HttpStatus,
    RequestOrEncodingFailure,
    MalformedPage,
    CountChanged,
    PaginationContradiction,
    InvalidIdentity,
    DuplicateOrUnorderedIdentity,
    PageLimit,
}

#[derive(Serialize)]
struct PageEvidence {
    offset: usize,
    count: usize,
    returned_rows: usize,
}

#[derive(Default, Serialize)]
struct ResourceEvidence {
    pages: Vec<PageEvidence>,
    failure: Option<ResourceFailure>,
    http_status: Option<u16>,
}

fn add_payload(
    payloads: &mut BTreeMap<String, Vec<u8>>,
    path: &str,
    bytes: Vec<u8>,
) -> PayloadDescriptor {
    let descriptor = PayloadDescriptor {
        path: path.into(),
        byte_length: bytes.len() as u64,
        sha256: digest(&bytes),
    };
    payloads.insert(path.into(), bytes);
    descriptor
}

fn encode(value: &impl Serialize) -> Result<Vec<u8>, AppError> {
    serde_json::to_vec(value).map_err(|_| failure("evidence_encoding"))
}

async fn status(client: &NetboxClient) -> Result<Value, AppError> {
    let response = client
        .capture_get("api/status/", &[])
        .await
        .map_err(|_| failure("source_status"))?;
    let CaptureGetResponse::Json(value) = response else {
        return Err(failure("source_status"));
    };
    if value.get("netbox-version").and_then(Value::as_str) != Some(NETBOX_VERSION) {
        return Err(failure("source_version"));
    }
    Ok(value)
}

/// Observes the actual status version before and after capture. Schema is read
/// directly through NetboxClient, bypassing schema-provider Redis caches entirely.
pub async fn capture_inventory(
    client: &NetboxClient,
    provider_id: Uuid,
    exporter_source_sha: &str,
    options: &CaptureOptions,
) -> Result<NetboxInventoryCapture, AppError> {
    if provider_id.is_nil()
        || options.page_size == 0
        || options.max_pages_per_resource == 0
        || (!options.collection_grants.is_empty()
            && options
                .authority_evidence
                .as_ref()
                .is_none_or(Vec::is_empty))
        || options
            .collection_grants
            .iter()
            .any(|key| match SourceEntity::new(key) {
                Ok(entity) => entity.as_str() != key.as_str(),
                Err(_) => true,
            })
    {
        return Err(failure("capture_options"));
    }
    let started_at = Utc::now();
    let start_status = status(client).await?;
    let schema = client
        .get_schema()
        .await
        .map_err(|_| failure("source_schema"))?;
    let paths = schema
        .get("paths")
        .and_then(Value::as_object)
        .filter(|paths| !paths.is_empty())
        .ok_or_else(|| failure("source_schema"))?;
    if !schema
        .get("openapi")
        .and_then(Value::as_str)
        .is_some_and(|v| v.starts_with("3."))
    {
        return Err(failure("source_schema"));
    }
    let mut payloads = BTreeMap::new();
    // Hash the exact serialized schema bytes stored in the artifact, not a
    // second serialization or an assumed provider label.
    let schema_descriptor = add_payload(&mut payloads, "schema.json", encode(&schema)?);
    let mut resources = Vec::new();
    let mut evidence = Vec::new();
    let mut captured_collections = BTreeSet::new();
    let ordered_paths: BTreeMap<_, _> = paths.iter().collect();
    for (index, (path, item)) in ordered_paths.iter().enumerate() {
        let classification = classify(&schema, path, item);
        let (entity, reason) = match classification {
            Classification::Collection(entity) => {
                if !options.collection_grants.contains(entity.as_str()) {
                    resources.push(unresolved(path, Some(entity), CaptureStatus::NotAttempted));
                    evidence.push(PathEvidence {
                        path: (*path).clone(),
                        reason: PathReason::MissingReadAuthority,
                        collection_path: Some((*path).clone()),
                        capture: None,
                    });
                    continue;
                }
                (Some(entity), PathReason::PaginatedIntegerIdCollection)
            }
            Classification::Other(reason) => (None, reason),
        };
        if let Some(entity) = entity {
            let (mut census, records, capture_evidence) =
                read_collection(client, provider_id, path, &entity, options).await;
            if census.status != CaptureStatus::Inaccessible {
                let file = format!("resource-{index:06}.json");
                census.payload = Some(add_payload(&mut payloads, &file, encode(&records)?));
            }
            if census.status == CaptureStatus::Captured {
                captured_collections.insert((*path).clone());
            }
            resources.push(census);
            evidence.push(PathEvidence {
                path: (*path).clone(),
                reason,
                collection_path: Some((*path).clone()),
                capture: Some(capture_evidence),
            });
        } else {
            resources.push(unresolved(path, None, CaptureStatus::Unrepresentable));
            evidence.push(PathEvidence {
                path: (*path).clone(),
                reason,
                collection_path: None,
                capture: None,
            });
        }
    }
    // Reclassify detail/action paths only against actually captured collections.
    // A detail GET is not an extra collection. Action-specific behavior/data is
    // still unresolved and retains its own census blocker.
    for entry in &mut evidence {
        if matches!(entry.reason, PathReason::NestedOrPluginPath)
            && let Some((collection, detail_only)) = parent_collection(&entry.path)
            && captured_collections.contains(&collection)
        {
            entry.collection_path = Some(collection);
            entry.reason = if detail_only {
                PathReason::CollectionDetail
            } else {
                PathReason::CollectionActionUncaptured
            };
            if detail_only {
                resources.retain(|r| r.endpoint != entry.path);
            }
        }
    }
    let end_status = status(client).await?;
    let consistency_evidence = add_payload(
        &mut payloads,
        "consistency.json",
        encode(&json!({
            "status_before": start_status, "status_after": end_status,
            "reason": "stable_source_not_proven", "matching_counts_prove_consistency": false,
        }))?,
    );
    let profile_bytes = encode(options)?;
    let profile_digest = digest(&profile_bytes);
    let discovery_evidence = add_payload(
        &mut payloads,
        "discovery.json",
        encode(&json!({
            "scope": "observed_schema_paths_only", "full_read_authority_proven": false,
            "profile_bytes": profile_bytes, "profile_sha256": profile_digest,
            "paths": evidence,
        }))?,
    );
    let manifest = ExportManifest {
        format_version: EXPORT_FORMAT_VERSION,
        source_provider_id: provider_id,
        observed_netbox_version: NETBOX_VERSION.into(),
        exporter_source_sha: exporter_source_sha.into(),
        // A live read attests nothing beyond itself; the assessment treats this
        // as unattested until an owner pins an independent attestation.
        authority: SourceAuthority::live_capture(),
        capture: CaptureEvidence {
            started_at,
            ended_at: Utc::now(),
            outcome: CaptureOutcome::Incomplete,
            consistency: ConsistencyStatus::Unproven,
            consistency_evidence,
            discovery_complete: false,
            discovery_evidence,
        },
        schema: schema_descriptor,
        resources,
        processes: [
            ProcessArea::Plugins,
            ProcessArea::ScriptsAndJobs,
            ProcessArea::Webhooks,
            ProcessArea::EventRules,
            ProcessArea::Permissions,
        ]
        .into_iter()
        .map(|area| ProcessCoverage {
            area,
            status: ObservationStatus::Unobservable,
            evidence: None,
        })
        .collect(),
    };
    manifest.validate()?;
    Ok(NetboxInventoryCapture { manifest, payloads })
}

fn unresolved(path: &str, entity: Option<SourceEntity>, status: CaptureStatus) -> ResourceCensus {
    ResourceCensus {
        resource_key: entity
            .as_ref()
            .map_or_else(|| path.into(), |e| e.as_str().into()),
        endpoint: path.into(),
        entity,
        status,
        declared_rows: None,
        observed_rows: 0,
        payload: None,
    }
}

enum Classification {
    Collection(SourceEntity),
    Other(PathReason),
}

fn classify(schema: &Value, path: &str, item: &Value) -> Classification {
    if !path.starts_with("/api/")
        || !path.ends_with('/')
        || path.contains("//")
        || path.split('/').any(|s| s == "." || s == "..")
        || !path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'{' | b'}'))
    {
        return Classification::Other(PathReason::UnsafeOrNoncanonicalPath);
    }
    let parts: Vec<_> = path.trim_matches('/').split('/').collect();
    if parts.len() != 3 || parts[1] == "plugins" {
        return Classification::Other(PathReason::NestedOrPluginPath);
    }
    let Ok(entity) = SourceEntity::new(&format!("{}.{}", parts[1], parts[2])) else {
        return Classification::Other(PathReason::UnsafeOrNoncanonicalPath);
    };
    if item.get("get").is_none() {
        return Classification::Other(PathReason::NoGetOperation);
    }
    if collection_item_schema(schema, item).is_none() {
        return Classification::Other(PathReason::UnsupportedResponseSchema);
    }
    Classification::Collection(entity)
}

fn resolve<'a>(schema: &'a Value, mut value: &'a Value) -> Option<&'a Value> {
    // Resolve local schema references only; cyclic/remote/unsupported references
    // remain explicit unsupported paths rather than issuing another HTTP GET.
    for _ in 0..32 {
        let Some(reference) = value.get("$ref") else {
            return Some(value);
        };
        let reference = reference.as_str()?.strip_prefix('#')?;
        if !reference.starts_with("/components/") {
            return None;
        }
        value = schema.pointer(reference)?;
    }
    None
}

fn collection_item_schema<'a>(schema: &'a Value, item: &'a Value) -> Option<&'a Value> {
    let response = resolve(schema, item.pointer("/get/responses/200")?)?;
    let body = resolve(
        schema,
        response.pointer("/content/application~1json/schema")?,
    )?;
    let count = resolve(schema, body.pointer("/properties/count")?)?;
    let results = resolve(schema, body.pointer("/properties/results")?)?;
    let row = resolve(schema, results.get("items")?)?;
    let id = resolve(schema, row.pointer("/properties/id")?)?;
    (count.get("type")?.as_str()? == "integer"
        && results.get("type")?.as_str()? == "array"
        && id.get("type")?.as_str()? == "integer")
        .then_some(row)
}

fn parent_collection(path: &str) -> Option<(String, bool)> {
    let parts: Vec<_> = path
        .strip_prefix("/api/")?
        .strip_suffix('/')?
        .split('/')
        .collect();
    if parts.len() < 3 || !matches!(parts[2], "{id}" | "{pk}") {
        return None;
    }
    let parent = format!("/api/{}/{}/", parts[0], parts[1]);
    Some((parent, parts.len() == 3))
}

async fn read_collection(
    client: &NetboxClient,
    provider_id: Uuid,
    path: &str,
    entity: &SourceEntity,
    options: &CaptureOptions,
) -> (ResourceCensus, Vec<SourceRecord>, ResourceEvidence) {
    let mut census = unresolved(path, Some(entity.clone()), CaptureStatus::Partial);
    let mut records = Vec::new();
    let mut evidence = ResourceEvidence::default();
    let mut previous_id = 0u64;
    for _ in 0..options.max_pages_per_resource {
        let offset = records.len();
        let params = [
            ("limit", options.page_size.to_string()),
            ("offset", offset.to_string()),
            ("ordering", "id".into()),
        ];
        let response = client
            .capture_get(path.trim_start_matches('/'), &params)
            .await;
        let value = match response {
            Ok(CaptureGetResponse::Json(value)) => value,
            Ok(CaptureGetResponse::HttpStatus(status)) => {
                evidence.http_status = Some(status);
                evidence.failure = Some(if status == 403 {
                    ResourceFailure::HttpForbidden
                } else {
                    ResourceFailure::HttpStatus
                });
                if records.is_empty() {
                    census.status = CaptureStatus::Inaccessible;
                }
                break;
            }
            Err(_) => {
                evidence.failure = Some(ResourceFailure::RequestOrEncodingFailure);
                break;
            }
        };
        // A NetBox paginated object is required here. Repository's permissive
        // bare-array convenience cannot certify declared collection counts.
        let has_metadata = value.as_object().is_some_and(|object| {
            ["count", "results", "next", "previous"]
                .iter()
                .all(|key| object.contains_key(*key))
        });
        let page = if has_metadata {
            serde_json::from_value::<NetboxPage>(value).ok()
        } else {
            None
        };
        let Some(page) = page else {
            evidence.failure = Some(ResourceFailure::MalformedPage);
            break;
        };
        evidence.pages.push(PageEvidence {
            offset,
            count: page.count,
            returned_rows: page.results.len(),
        });
        if let Some(count) = census.declared_rows {
            if count != page.count as u64 {
                evidence.failure = Some(ResourceFailure::CountChanged);
                break;
            }
        } else {
            census.declared_rows = Some(page.count as u64);
        }
        let end = offset.checked_add(page.results.len());
        if end.is_none_or(|end| end > page.count)
            || page.results.len() > options.page_size
            || (page.results.is_empty() && offset < page.count)
            || (page.next.is_some() != end.is_some_and(|end| end < page.count))
            || page.next.as_ref().is_some_and(String::is_empty)
            || page.previous.as_ref().is_some_and(String::is_empty)
            || (page.previous.is_some() != (offset > 0))
        {
            evidence.failure = Some(ResourceFailure::PaginationContradiction);
            break;
        }
        // Only presence of next/previous is checked. Their values are NEVER
        // followed, parsed as request destinations or included in diagnostics.
        for row in page.results {
            let Ok(record) = record_from_value(provider_id, &entity, row) else {
                evidence.failure = Some(ResourceFailure::InvalidIdentity);
                break;
            };
            let Ok(numeric_id) = record.identity.external_id.parse::<u64>() else {
                evidence.failure = Some(ResourceFailure::InvalidIdentity);
                break;
            };
            if numeric_id <= previous_id {
                evidence.failure = Some(ResourceFailure::DuplicateOrUnorderedIdentity);
                break;
            }
            records.push(record);
            previous_id = numeric_id;
        }
        if evidence.failure.is_some() {
            break;
        }
        if records.len() == page.count {
            census.status = CaptureStatus::Captured;
            break;
        }
    }
    if census.status == CaptureStatus::Partial && evidence.failure.is_none() {
        evidence.failure = Some(ResourceFailure::PageLimit);
    }
    census.observed_rows = records.len() as u64;
    (census, records, evidence)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Arc;

    use axum::Router;
    use axum::extract::State;
    use axum::http::{Method, StatusCode, Uri};
    use axum::response::{IntoResponse, Response};
    use tokio::sync::Mutex;

    use crate::contexts::sync::domain::InventorySource;
    use crate::contexts::sync::infrastructure::providers::ImmutableInventorySource;

    const CANARY: &str = "CANARY-private-token-expression";
    const SOURCE_SHA: &str = "6b53415ba3ebed45b55d08d2c5e15d766f2dd2c1";

    struct Reply {
        status: u16,
        body: String,
    }

    impl Reply {
        fn json(status: u16, body: Value) -> Self {
            Self {
                status,
                body: body.to_string(),
            }
        }
    }

    #[derive(Default)]
    struct MockState {
        replies: BTreeMap<String, VecDeque<Reply>>,
        calls: Vec<(String, String, String)>,
    }

    struct MockServer {
        base_url: String,
        state: Arc<Mutex<MockState>>,
        task: tokio::task::JoinHandle<Result<(), std::io::Error>>,
    }

    impl Drop for MockServer {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    async fn respond(
        State(state): State<Arc<Mutex<MockState>>>,
        method: Method,
        uri: Uri,
    ) -> Response {
        let mut state = state.lock().await;
        state.calls.push((
            method.to_string(),
            uri.path().into(),
            uri.query().unwrap_or_default().into(),
        ));
        let reply = state
            .replies
            .get_mut(uri.path())
            .and_then(VecDeque::pop_front)
            .unwrap_or_else(|| Reply::json(404, json!({"private": CANARY})));
        (
            StatusCode::from_u16(reply.status).expect("test status"),
            [("content-type", "application/json")],
            reply.body,
        )
            .into_response()
    }

    async fn server(schema: Value, collections: BTreeMap<String, Vec<Reply>>) -> MockServer {
        let mut state = MockState::default();
        state.replies.insert(
            "/api/status/".into(),
            VecDeque::from([
                Reply::json(200, json!({"netbox-version": NETBOX_VERSION})),
                Reply::json(200, json!({"netbox-version": NETBOX_VERSION})),
            ]),
        );
        state.replies.insert(
            "/api/schema/".into(),
            VecDeque::from([Reply::json(200, schema)]),
        );
        for (path, replies) in collections {
            state.replies.insert(path, replies.into());
        }
        let state = Arc::new(Mutex::new(state));
        let app = Router::new().fallback(respond).with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("mock listener");
        let base_url = format!("http://{}", listener.local_addr().expect("mock address"));
        let task = tokio::spawn(async move { axum::serve(listener, app).await });
        MockServer {
            base_url,
            state,
            task,
        }
    }

    fn schema(paths: &[&str]) -> Value {
        let mut paths_object = serde_json::Map::new();
        for path in paths {
            paths_object.insert((*path).into(), json!({"get": {"responses": {"200": {
                "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Page"}}}
            }}}}));
        }
        json!({
            "openapi": "3.0.3", "info": {"version": NETBOX_VERSION}, "paths": paths_object,
            "components": {"schemas": {
                "Row": {"type": "object", "properties": {"id": {"type": "integer"}, "name": {"type": "string"}}},
                "Page": {"type": "object", "properties": {
                    "count": {"type": "integer"}, "results": {"type": "array", "items": {"$ref": "#/components/schemas/Row"}}
                }}
            }}
        })
    }

    fn page(count: usize, ids: &[u64], next: bool, previous: bool) -> Reply {
        Reply::json(
            200,
            json!({
                "count": count,
                "results": ids.iter().map(|id| json!({"id": id, "name": CANARY})).collect::<Vec<_>>(),
                // Deliberately hostile destinations. Capture derives provider-local
                // offsets and NEVER follows either returned URL.
                "next": next.then_some("https://user:CANARY@external.invalid/next/"),
                "previous": previous.then_some("https://external.invalid/previous/"),
            }),
        )
    }

    fn options() -> CaptureOptions {
        CaptureOptions {
            page_size: 2,
            max_pages_per_resource: 5,
            ..CaptureOptions::default()
        }
    }

    fn granted_options(collections: &[&str]) -> CaptureOptions {
        CaptureOptions {
            collection_grants: collections.iter().map(|key| (*key).into()).collect(),
            authority_evidence: Some(CANARY.as_bytes().to_vec()),
            ..options()
        }
    }

    async fn capture(
        server: &MockServer,
        options: &CaptureOptions,
    ) -> Result<NetboxInventoryCapture, AppError> {
        let client = NetboxClient::new(server.base_url.clone(), "private-test-token".into())?;
        capture_inventory(&client, Uuid::from_u128(1), SOURCE_SHA, options).await
    }

    fn discovery(capture: &NetboxInventoryCapture) -> Value {
        serde_json::from_slice(&capture.payloads["discovery.json"]).expect("discovery evidence")
    }

    fn resource<'a>(capture: &'a NetboxInventoryCapture, key: &str) -> &'a ResourceCensus {
        capture
            .manifest
            .resources
            .iter()
            .find(|r| r.resource_key == key)
            .expect("census resource")
    }

    #[tokio::test]
    async fn exact_grants_capture_unknown_and_empty_collections_without_filtering_discovery() {
        let paths = [
            "/api/dcim/sites/",
            "/api/dcim/racks/",
            "/api/dcim/unknown-things/",
            "/api/dcim/sites/{id}/",
            "/api/dcim/sites/{id}/render-config/",
            "/api/plugins/example/nested/",
            "/api/users/tokens/",
            "/api/extras/webhooks/",
            "/api/unclassified/",
            "/api/custom/unknown-things/",
            "https://user:CANARY@external.invalid/path/",
        ];
        let server = server(
            schema(&paths),
            BTreeMap::from([
                (
                    "/api/dcim/sites/".into(),
                    vec![page(3, &[1, 2], true, false), page(3, &[3], false, true)],
                ),
                ("/api/dcim/racks/".into(), vec![page(0, &[], false, false)]),
                (
                    "/api/dcim/unknown-things/".into(),
                    vec![page(1, &[42], false, false)],
                ),
                (
                    "/api/custom/unknown-things/".into(),
                    vec![page(1, &[7], false, false)],
                ),
            ]),
        )
        .await;
        let captured = capture(
            &server,
            &granted_options(&[
                "dcim.sites",
                "dcim.racks",
                "dcim.unknown-things",
                "custom.unknown-things",
            ]),
        )
        .await
        .expect("assessment capture");
        assert!(captured.manifest.capture.outcome == CaptureOutcome::Incomplete);
        assert!(captured.manifest.capture.consistency == ConsistencyStatus::Unproven);
        assert!(!captured.manifest.capture.discovery_complete);
        assert_eq!(resource(&captured, "dcim.sites").observed_rows, 3);
        assert_eq!(resource(&captured, "dcim.racks").observed_rows, 0);
        assert!(resource(&captured, "dcim.racks").payload.is_some());
        assert_eq!(resource(&captured, "dcim.unknown-things").observed_rows, 1);
        assert!(
            resource(&captured, "/api/plugins/example/nested/").status
                == CaptureStatus::Unrepresentable
        );
        assert!(resource(&captured, "users.tokens").status == CaptureStatus::NotAttempted);
        assert!(resource(&captured, "custom.unknown-things").status == CaptureStatus::Captured);
        assert_eq!(captured.manifest.processes.len(), 5);
        assert!(
            captured
                .manifest
                .processes
                .iter()
                .all(|p| p.status == ObservationStatus::Unobservable)
        );
        let evidence = discovery(&captured);
        let dispositions = evidence["paths"].as_array().expect("path census");
        assert_eq!(dispositions.len(), paths.len());
        assert!(
            dispositions
                .windows(2)
                .all(|pair| pair[0]["path"].as_str() < pair[1]["path"].as_str())
        );
        assert!(
            dispositions
                .iter()
                .any(|entry| entry["reason"] == "collection_detail")
        );
        assert!(
            dispositions
                .iter()
                .any(|entry| entry["reason"] == "collection_action_uncaptured")
        );
        let calls = server.state.lock().await.calls.clone();
        assert!(calls.iter().all(|(method, _, _)| method == "GET"));
        assert_eq!(
            calls
                .iter()
                .filter(|(_, path, _)| path == "/api/schema/")
                .count(),
            1
        );
        assert!(!calls.iter().any(|(_, path, _)| path.contains("tokens")
            || path.contains("plugins")
            || path.contains("webhooks")
            || path.contains("render-config")));
        for (_, path, query) in &calls {
            if path.starts_with("/api/dcim/") {
                assert!(query.contains("ordering=id"));
            }
        }
        assert!(
            calls
                .iter()
                .any(|(_, path, query)| path == "/api/dcim/sites/" && query.contains("offset=2"))
        );
        // The capture uses the same seal and verified replay implementations.
        let output = tempfile::tempdir().expect("private output parent");
        let directory = output.path().join("capture");
        let expected = captured.seal(&directory).expect("seal assessment evidence");
        let source = ImmutableInventorySource::open(&directory, &expected, Uuid::from_u128(1), 2)
            .expect("verified assessment source");
        assert_eq!(source.supported_entities().len(), 4);
        assert!(captured.seal(&directory).is_err());
    }

    #[tokio::test]
    async fn actual_status_version_not_schema_label_is_required_before_and_after_capture() {
        for at_end in [false, true] {
            let server = server(
                schema(&["/api/dcim/sites/"]),
                BTreeMap::from([("/api/dcim/sites/".into(), vec![page(0, &[], false, false)])]),
            )
            .await;
            let mut state = server.state.lock().await;
            let statuses = state.replies.get_mut("/api/status/").expect("status queue");
            statuses[usize::from(at_end)] = Reply::json(200, json!({"netbox-version": CANARY}));
            drop(state);
            let error = capture(&server, &granted_options(&["dcim.sites"]))
                .await
                .err()
                .expect("wrong version rejected")
                .to_string();
            assert!(error.contains("netbox_capture:source_version"));
            assert!(!error.contains(CANARY));
            if !at_end {
                assert_eq!(server.state.lock().await.calls.len(), 1);
            }
        }
    }

    #[tokio::test]
    async fn independent_resources_continue_after_permission_malformed_count_and_identity_failures()
    {
        let cases = vec![
            (
                vec![Reply::json(403, json!({"private": CANARY}))],
                "http_forbidden",
                0,
            ),
            (
                vec![Reply {
                    status: 200,
                    body: CANARY.into(),
                }],
                "request_or_encoding_failure",
                0,
            ),
            (
                vec![Reply::json(200, json!({"private": CANARY}))],
                "malformed_page",
                0,
            ),
            (
                vec![page(3, &[1, 2], true, false), page(4, &[3, 4], false, true)],
                "count_changed",
                2,
            ),
            (
                vec![page(2, &[1, 1], false, false)],
                "duplicate_or_unordered_identity",
                1,
            ),
            (
                vec![page(3, &[1, 3], true, false), page(3, &[2], false, true)],
                "duplicate_or_unordered_identity",
                2,
            ),
            (
                vec![Reply::json(200, json!({"count": 0, "results": []}))],
                "malformed_page",
                0,
            ),
            (
                vec![Reply::json(
                    200,
                    json!({"count": 1, "results": [{"id": CANARY}], "next": null, "previous": null}),
                )],
                "invalid_identity",
                0,
            ),
            (
                vec![page(3, &[], true, false)],
                "pagination_contradiction",
                0,
            ),
            (
                vec![page(1, &[1, 2], false, false)],
                "pagination_contradiction",
                0,
            ),
        ];
        for (replies, expected_reason, expected_rows) in cases {
            let server = server(
                schema(&["/api/dcim/sites/", "/api/dcim/zz-independent/"]),
                BTreeMap::from([
                    ("/api/dcim/sites/".into(), replies),
                    (
                        "/api/dcim/zz-independent/".into(),
                        vec![page(0, &[], false, false)],
                    ),
                ]),
            )
            .await;
            let captured = capture(
                &server,
                &granted_options(&["dcim.sites", "dcim.zz-independent"]),
            )
            .await
            .expect("truthful incomplete assessment");
            assert!(captured.manifest.capture.outcome == CaptureOutcome::Incomplete);
            let sites = resource(&captured, "dcim.sites");
            assert!(sites.status != CaptureStatus::Captured);
            assert_eq!(sites.observed_rows, expected_rows);
            assert!(resource(&captured, "dcim.zz-independent").status == CaptureStatus::Captured);
            let evidence = discovery(&captured);
            let failure_evidence = &evidence["paths"][0]["capture"];
            assert_eq!(failure_evidence["failure"], expected_reason);
            if expected_reason == "http_forbidden" {
                assert!(sites.status == CaptureStatus::Inaccessible);
                assert!(sites.payload.is_none());
                assert_eq!(failure_evidence["http_status"], 403);
            } else {
                assert!(sites.status == CaptureStatus::Partial);
            }
            assert!(!failure_evidence.to_string().contains(CANARY));
            captured
                .manifest
                .validate()
                .expect("preserved evidence is valid format");
            assert!(
                server
                    .state
                    .lock()
                    .await
                    .calls
                    .iter()
                    .all(|(method, _, _)| method == "GET")
            );
        }
    }

    #[tokio::test]
    async fn default_authority_never_infers_safety_from_auth_key_or_arbitrary_json() {
        let paths = [
            "/api/dcim/sites/",
            "/api/ipam/fhrp-groups/",
            "/api/extras/payloads/",
            "/api/extras/opaque/",
        ];
        let mut source_schema = schema(&paths);
        for (path, row) in [
            (
                "/api/ipam/fhrp-groups/",
                json!({"properties": {
                    "id": {"type": "integer"}, "auth_key": {"type": "string"}
                }}),
            ),
            (
                "/api/extras/payloads/",
                json!({"properties": {
                    "id": {"type": "integer"}, "data": {"type": "object", "additionalProperties": true}
                }}),
            ),
            (
                "/api/extras/opaque/",
                json!({"properties": {"id": {"type": "string"}}}),
            ),
        ] {
            source_schema["paths"][path]["get"]["responses"]["200"]["content"]["application/json"]
                ["schema"] = json!({
                "properties": {"count": {"type": "integer"}, "results": {"type": "array", "items": row}}
            });
        }
        let grants: &[&[&str]] = &[
            &[],
            &["ipam.fhrp-groups"],
            &["extras.payloads"],
            &[
                "dcim.sites",
                "ipam.fhrp-groups",
                "extras.payloads",
                "extras.opaque",
            ],
        ];
        for keys in grants {
            let server = server(source_schema.clone(), BTreeMap::from([
                ("/api/dcim/sites/".into(), vec![page(0, &[], false, false)]),
                ("/api/ipam/fhrp-groups/".into(), vec![Reply::json(200, json!({
                    "count": 1, "results": [{"id": 1, "auth_key": CANARY}], "next": null, "previous": null
                }))]),
                ("/api/extras/payloads/".into(), vec![Reply::json(200, json!({
                    "count": 1, "results": [{"id": 2, "data": {"private": CANARY}}], "next": null, "previous": null
                }))]),
            ])).await;
            let profile = if keys.is_empty() {
                options()
            } else {
                granted_options(keys)
            };
            let captured = capture(&server, &profile)
                .await
                .expect("authority-bounded assessment");
            let evidence = discovery(&captured);
            for (key, endpoint) in [
                ("dcim.sites", "/api/dcim/sites/"),
                ("ipam.fhrp-groups", "/api/ipam/fhrp-groups/"),
                ("extras.payloads", "/api/extras/payloads/"),
            ] {
                let census = resource(&captured, key);
                let state = server.state.lock().await;
                let fetched = state.calls.iter().any(|(_, path, _)| path == endpoint);
                assert_eq!(fetched, keys.contains(&key));
                if !keys.contains(&key) {
                    assert!(census.status == CaptureStatus::NotAttempted);
                    assert!(census.declared_rows.is_none() && census.payload.is_none());
                    assert_eq!(census.observed_rows, 0);
                    let disposition = evidence["paths"]
                        .as_array()
                        .expect("path evidence")
                        .iter()
                        .find(|entry| entry["path"] == endpoint)
                        .expect("withheld path");
                    assert_eq!(disposition["reason"], "missing_read_authority");
                    assert!(disposition["capture"].is_null());
                } else {
                    assert!(census.status == CaptureStatus::Captured);
                }
            }
            let state = server.state.lock().await;
            assert!(state.calls.iter().all(|(method, _, _)| method == "GET"));
            assert!(
                !state
                    .calls
                    .iter()
                    .any(|(_, path, _)| path == "/api/extras/opaque/")
            );
            if keys.is_empty() {
                assert_eq!(state.calls.len(), 3);
                assert!(
                    state.calls.iter().all(|(_, path, _)| matches!(
                        path.as_str(),
                        "/api/status/" | "/api/schema/"
                    ))
                );
            }
            assert!(
                resource(&captured, "/api/extras/opaque/").status == CaptureStatus::Unrepresentable
            );
            assert!(captured.manifest.capture.outcome == CaptureOutcome::Incomplete);
            assert!(captured.manifest.capture.consistency == ConsistencyStatus::Unproven);
            assert!(!captured.manifest.capture.discovery_complete);
            assert!(
                captured
                    .manifest
                    .processes
                    .iter()
                    .all(|p| p.status == ObservationStatus::Unobservable)
            );
        }
    }

    #[tokio::test]
    async fn collections_require_exact_grants_and_private_authority_evidence() {
        let paths = [
            "/api/users/tokens/",
            "/api/extras/webhooks/",
            "/api/extras/vaults/",
        ];
        let mut source_schema = schema(&paths);
        source_schema["paths"]["/api/extras/vaults/"]["get"]["responses"]["200"]["content"]["application/json"]
            ["schema"] = json!({
            "properties": {"count": {"type": "integer"}, "results": {"type": "array", "items": {
                "properties": {"id": {"type": "integer"}, "password": {"type": "string", "format": "password"}}
            }}}
        });
        let server = server(
            source_schema,
            BTreeMap::from([(
                "/api/users/tokens/".into(),
                vec![page(1, &[7], false, false)],
            )]),
        )
        .await;
        let mut profile = options();
        profile.collection_grants.insert("users.tokens".into());
        assert!(capture(&server, &profile).await.is_err());
        assert!(server.state.lock().await.calls.is_empty());
        profile.authority_evidence = Some(Vec::new());
        assert!(capture(&server, &profile).await.is_err());
        assert!(server.state.lock().await.calls.is_empty());
        let wildcard = CaptureOptions {
            collection_grants: BTreeSet::from(["users.*".into()]),
            ..granted_options(&[])
        };
        assert!(capture(&server, &wildcard).await.is_err());
        assert!(server.state.lock().await.calls.is_empty());
        profile.authority_evidence = Some(CANARY.as_bytes().to_vec());
        let captured = capture(&server, &profile)
            .await
            .expect("authorized collection");
        assert!(resource(&captured, "users.tokens").status == CaptureStatus::Captured);
        assert!(resource(&captured, "extras.webhooks").status == CaptureStatus::NotAttempted);
        assert!(resource(&captured, "extras.vaults").status == CaptureStatus::NotAttempted);
        assert!(captured.manifest.capture.outcome == CaptureOutcome::Incomplete);
        let calls = &server.state.lock().await.calls;
        assert!(
            !calls
                .iter()
                .any(|(_, path, _)| path.contains("webhooks") || path.contains("vaults"))
        );
        let evidence = discovery(&captured);
        let bytes: Vec<u8> = serde_json::from_value(evidence["profile_bytes"].clone())
            .expect("private profile bytes");
        assert_eq!(evidence["profile_sha256"], digest(&bytes));
    }

    #[tokio::test]
    async fn unsafe_paths_fatal_schema_and_config_failures_are_sanitized() {
        let server = server(json!({"private": CANARY}), BTreeMap::new()).await;
        let error = capture(&server, &options())
            .await
            .err()
            .expect("invalid schema")
            .to_string();
        assert!(error.contains("netbox_capture:source_schema") && !error.contains(CANARY));
        let client = NetboxClient::new(server.base_url.clone(), "private-test-token".into())
            .expect("client");
        for path in [
            "https://user:CANARY@external.invalid/",
            "api/../status/",
            "api/%2e%2e/status/",
            "api//status/",
            "api/status/?secret=CANARY",
        ] {
            let error = client
                .capture_get(path, &[])
                .await
                .err()
                .expect("unsafe request path")
                .to_string();
            assert!(error.contains("netbox_capture:request_path") && !error.contains(CANARY));
        }
        assert_eq!(server.state.lock().await.calls.len(), 2);
        let credential_url =
            server
                .base_url
                .replacen("http://", &format!("http://user:{CANARY}@"), 1);
        let client =
            NetboxClient::new(credential_url, "private-test-token".into()).expect("client");
        let error = capture_inventory(&client, Uuid::from_u128(1), SOURCE_SHA, &options())
            .await
            .err()
            .expect("userinfo rejected")
            .to_string();
        assert!(!error.contains(CANARY));
        assert_eq!(server.state.lock().await.calls.len(), 2);
    }

    #[tokio::test]
    async fn resource_page_limit_is_explicit_and_never_complete() {
        let server = server(
            schema(&["/api/dcim/sites/"]),
            BTreeMap::from([(
                "/api/dcim/sites/".into(),
                vec![page(3, &[1, 2], true, false)],
            )]),
        )
        .await;
        let mut profile = granted_options(&["dcim.sites"]);
        profile.max_pages_per_resource = 1;
        let captured = capture(&server, &profile)
            .await
            .expect("limited assessment");
        assert!(resource(&captured, "dcim.sites").status == CaptureStatus::Partial);
        assert_eq!(resource(&captured, "dcim.sites").observed_rows, 2);
        assert_eq!(
            discovery(&captured)["paths"][0]["capture"]["failure"],
            "page_limit"
        );
    }
}
