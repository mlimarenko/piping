//! Intake of the core-only NetBox producer's private transport. HTTP evidence
//! cannot enter this path: a distinct, exact-byte operator endorsement and the
//! shipped version contract are mandatory before sealing a v2 bundle.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::contexts::sync::domain::inventory_export::{
    CaptureEvidence, CaptureOutcome, CaptureStatus, ConsistencyStatus, EXPORT_FORMAT_VERSION,
    EdgeCoverage, ExportManifest, NETBOX_VERSION, ObservationStatus, PROCESS_AREAS,
    PayloadDescriptor, ProcessArea, ProcessCoverage, ResourceCensus, SnapshotAttestation,
    SourceAuthority, digest,
};
use crate::contexts::sync::domain::inventory_sync::normalize_netbox_id;
use crate::contexts::sync::domain::{SourceEntity, SourceRecord};
use crate::errors::AppError;

use super::immutable_inventory_source::{ImmutableInventorySource, snapshot_expectations};
use super::netbox_inventory_capture::NetboxInventoryCapture;
use super::netbox_inventory_source::record_from_value;

const SOURCE_CONTRACT: &[u8] = include_bytes!("../../../../../contracts/netbox-4.7.1-source.json");
pub const MAX_SNAPSHOT_TRANSPORT_BYTES: u64 = 512 * 1024 * 1024;

fn invalid(code: &'static str) -> AppError {
    AppError::Validation(format!("netbox_snapshot:{code}"))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotTransport {
    format_version: u32,
    netbox_version: String,
    source_provider_id: Uuid,
    exporter_source_sha: String,
    started_at: DateTime<Utc>,
    ended_at: DateTime<Utc>,
    contract_sha256: String,
    schema: Value,
    discovery: Value,
    consistency: Value,
    processes: Vec<SnapshotProcess>,
    resources: Vec<SnapshotResource>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotResource {
    resource_key: String,
    endpoint: String,
    rows: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotProcess {
    area: ProcessArea,
    status: ObservationStatus,
    evidence: Value,
}

fn payload(
    payloads: &mut BTreeMap<String, Vec<u8>>,
    path: String,
    bytes: Vec<u8>,
) -> PayloadDescriptor {
    let descriptor = PayloadDescriptor {
        path: path.clone(),
        byte_length: bytes.len() as u64,
        sha256: digest(&bytes),
    };
    payloads.insert(path, bytes);
    descriptor
}

fn encoded(value: &impl Serialize) -> Result<Vec<u8>, AppError> {
    serde_json::to_vec(value).map_err(|_| invalid("encoding"))
}

fn validate_generic_projection(
    record: &SourceRecord,
    model: &Value,
    exported_types: &BTreeSet<&str>,
) -> Result<(), AppError> {
    let pairs = model.get("generic_pairs").and_then(Value::as_array);
    for pair in pairs.into_iter().flatten() {
        let name = pair
            .get("field")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("compiled_contract"))?;
        let type_name = pair
            .get("type_field")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("compiled_contract"))?;
        let value = record
            .fields
            .get(name)
            .ok_or_else(|| invalid("generic_reference_projection"))?;
        let type_value = record
            .fields
            .get(type_name)
            .ok_or_else(|| invalid("generic_reference_projection"))?;
        for field in model
            .get("fields")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if field.get("output_name").and_then(Value::as_str) == Some(name)
                && let Some(raw) = field.get("name").and_then(Value::as_str)
                && raw != name
                && record.fields.contains_key(raw)
            {
                return Err(invalid("generic_reference_projection"));
            }
        }
        if value.is_null() && type_value.is_null() {
            continue;
        }
        let token = type_value
            .as_str()
            .filter(|token| exported_types.contains(token))
            .ok_or_else(|| invalid("generic_reference_projection"))?;
        if token.is_empty() || value.is_null() {
            return Err(invalid("generic_reference_projection"));
        }
        let id = value
            .as_object()
            .and_then(|object| object.get("id"))
            .unwrap_or(value);
        normalize_netbox_id(id).map_err(|_| invalid("generic_reference_projection"))?;
    }
    Ok(())
}

/// Convert only contract-declared ORM tag relations to the shared source shape.
fn normalize_tag_relations(record: &mut SourceRecord, model: &Value) -> Result<(), AppError> {
    let fields = model
        .get("fields")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("compiled_contract"))?;
    for field in fields {
        if field.get("disposition").and_then(Value::as_str) != Some("export")
            || field.get("target").and_then(Value::as_str) != Some("extras.tag")
        {
            continue;
        }
        let kind = field
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("compiled_contract"))?;
        if kind != "NetBoxTaggableManagerField" {
            return Err(invalid("tag_relation_contract"));
        }
        let name = field
            .get("output_name")
            .and_then(Value::as_str)
            .or_else(|| field.get("name").and_then(Value::as_str))
            .ok_or_else(|| invalid("compiled_contract"))?;
        let Some(value) = record.fields.get_mut(name) else {
            continue;
        };
        let items = value
            .as_array()
            .ok_or_else(|| invalid("tag_relation_shape"))?;
        let mut ids = BTreeSet::new();
        let mut normalized = Vec::with_capacity(items.len());
        for item in items {
            let id = normalize_netbox_id(item).map_err(|_| invalid("tag_relation_identity"))?;
            if !ids.insert(id.clone()) {
                return Err(invalid("duplicate_tag_identity"));
            }
            normalized.push(serde_json::json!({"id": id}));
        }
        *value = Value::Array(normalized);
    }
    Ok(())
}

/// Convert the bounded transport through the shared NetBox JSON converter. The
/// caller must independently supply the provider, operator and exact transport
/// digest; neither an HTTP capture nor a self-described digest is an endorsement.
/// The same operator is authenticated again by descriptor preparation and each
/// mutation boundary, not by this offline conversion.
pub fn capture_snapshot(
    bytes: &[u8],
    provider_id: Uuid,
    operator_id: Uuid,
    endorsed_provenance_sha256: &str,
) -> Result<NetboxInventoryCapture, AppError> {
    if bytes.len() as u64 > MAX_SNAPSHOT_TRANSPORT_BYTES
        || provider_id.is_nil()
        || operator_id.is_nil()
        || digest(bytes) != endorsed_provenance_sha256
    {
        return Err(invalid("endorsement"));
    }
    let transport: SnapshotTransport =
        serde_json::from_slice(bytes).map_err(|_| invalid("transport"))?;
    let contract: Value =
        serde_json::from_slice(SOURCE_CONTRACT).map_err(|_| invalid("compiled_contract"))?;
    if transport.format_version != 1
        || transport.netbox_version != NETBOX_VERSION
        || transport.source_provider_id != provider_id
        || transport.contract_sha256 != digest(SOURCE_CONTRACT)
        || contract.get("schema") != Some(&transport.schema)
        || transport.ended_at < transport.started_at
        || transport.discovery.get("complete") != Some(&Value::Bool(true))
        || transport
            .consistency
            .get("isolation_level")
            .and_then(Value::as_str)
            != Some("repeatable read")
        || transport.consistency.get("read_only") != Some(&Value::Bool(true))
        || !transport
            .consistency
            .get("snapshot")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty())
    {
        return Err(invalid("source_contract"));
    }
    let mut payloads = BTreeMap::new();
    let provenance = payload(
        &mut payloads,
        "snapshot-transport.json".into(),
        bytes.to_vec(),
    );
    let schema = payload(
        &mut payloads,
        "schema.json".into(),
        encoded(&transport.schema)?,
    );
    let discovery_evidence = payload(
        &mut payloads,
        "discovery.json".into(),
        encoded(&transport.discovery)?,
    );
    let consistency_evidence = payload(
        &mut payloads,
        "consistency.json".into(),
        encoded(&transport.consistency)?,
    );
    let mut resources = Vec::new();
    let mut by_entity: BTreeMap<String, Vec<SourceRecord>> = BTreeMap::new();
    let mut keys = BTreeSet::new();
    let models = transport
        .schema
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("model_census"))?;
    let exported_types: BTreeSet<&str> = models
        .iter()
        .filter(|model| model.get("disposition").and_then(Value::as_str) == Some("export"))
        .filter_map(|model| model.get("model").and_then(Value::as_str))
        .collect();
    for (index, resource) in transport.resources.into_iter().enumerate() {
        let entity =
            SourceEntity::new(&resource.resource_key).map_err(|_| invalid("resource_identity"))?;
        let (app, name) = entity
            .as_str()
            .split_once('.')
            .ok_or_else(|| invalid("resource_identity"))?;
        if resource.resource_key != entity.as_str()
            || resource.endpoint != format!("/api/{app}/{name}/")
            || !keys.insert(resource.resource_key.clone())
        {
            return Err(invalid("resource_identity"));
        }
        let mut records = Vec::with_capacity(resource.rows.len());
        let mut ids = BTreeSet::new();
        let model = models
            .iter()
            .find(|model| {
                model.get("resource_key").and_then(Value::as_str)
                    == Some(resource.resource_key.as_str())
            })
            .ok_or_else(|| invalid("resource_census"))?;
        for row in resource.rows {
            let mut record = record_from_value(provider_id, &entity, row)?;
            // Source schema, including templates not present in an older local
            // catalog, owns the one canonical ORM-to-transport pair shape.
            validate_generic_projection(&record, model, &exported_types)?;
            normalize_tag_relations(&mut record, model)?;
            if !ids.insert(record.identity.external_id.clone()) {
                return Err(invalid("duplicate_identity"));
            }
            records.push(record);
        }
        records.sort_by(|a, b| a.identity.external_id.cmp(&b.identity.external_id));
        let count = records.len() as u64;
        let descriptor = payload(
            &mut payloads,
            format!("records-{index:04}.json"),
            encoded(&records)?,
        );
        resources.push(ResourceCensus {
            resource_key: resource.resource_key.clone(),
            endpoint: resource.endpoint,
            entity: Some(entity),
            status: CaptureStatus::Captured,
            declared_rows: Some(count),
            observed_rows: count,
            payload: Some(descriptor),
        });
        by_entity.insert(resource.resource_key, records);
    }
    // The producer's contract owns the model-to-resource disposition. Checking
    // this set independently prevents deletion of an empty model from narrowing
    // the snapshot while discovery still claims completeness.
    let expected_keys: BTreeSet<String> = transport
        .schema
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("model_census"))?
        .iter()
        .filter_map(|model| model.get("resource_key").and_then(Value::as_str))
        .map(str::to_owned)
        .collect();
    if keys != expected_keys {
        return Err(invalid("resource_census"));
    }
    let mut processes = Vec::new();
    let mut areas = BTreeSet::new();
    for (index, process) in transport.processes.into_iter().enumerate() {
        if process.status != ObservationStatus::Observed
            || process.evidence.is_null()
            || !areas.insert(process.area)
        {
            return Err(invalid("process_coverage"));
        }
        processes.push(ProcessCoverage {
            area: process.area,
            status: process.status,
            evidence: Some(payload(
                &mut payloads,
                format!("process-{index:02}.json"),
                encoded(&process.evidence)?,
            )),
        });
    }
    if areas != BTreeSet::from(PROCESS_AREAS) {
        return Err(invalid("process_coverage"));
    }
    let expectations = snapshot_expectations(&by_entity)?;
    let expectations = payload(
        &mut payloads,
        "snapshot-expectations.json".into(),
        encoded(&expectations)?,
    );
    let mut manifest = ExportManifest {
        format_version: EXPORT_FORMAT_VERSION,
        source_provider_id: provider_id,
        observed_netbox_version: transport.netbox_version,
        exporter_source_sha: transport.exporter_source_sha,
        authority: SourceAuthority::live_capture(),
        capture: CaptureEvidence {
            started_at: transport.started_at,
            ended_at: transport.ended_at,
            outcome: CaptureOutcome::Complete,
            consistency: ConsistencyStatus::Consistent,
            consistency_evidence,
            discovery_complete: true,
            discovery_evidence,
        },
        schema,
        resources,
        processes,
    };
    manifest.authority = SourceAuthority::operator_endorsed_snapshot(SnapshotAttestation {
        issuer: "netbox-4.7.1/core-snapshot".into(),
        issued_at: Utc::now(),
        scope: manifest.declared_scope()?,
        edge_coverage: EdgeCoverage::TypedInventoryReferences,
        expectations,
        operator_id,
        provenance,
    });
    manifest.validate()?;
    Ok(NetboxInventoryCapture { manifest, payloads })
}

/// Bind sealed values and evidence back to the exact transport the operator
/// endorsed. A bundle author cannot retain a genuine provenance payload while
/// replacing records and regenerating their expectations under the same digest.
pub(crate) fn verify_snapshot_provenance(
    source: &ImmutableInventorySource,
) -> Result<(), AppError> {
    if source.orm_provenance_verified() {
        return Ok(());
    }
    let manifest = source.manifest();
    let attestation = manifest
        .authority
        .snapshot
        .as_ref()
        .ok_or_else(|| invalid("authority"))?;
    let bytes = source
        .verified_metadata(&attestation.provenance.path)
        .ok_or_else(|| invalid("provenance_missing"))?;
    let rebuilt = capture_snapshot(
        bytes,
        manifest.source_provider_id,
        attestation.operator_id,
        &attestation.provenance.sha256,
    )?;
    let expected_attestation = rebuilt
        .manifest
        .authority
        .snapshot
        .as_ref()
        .ok_or_else(|| invalid("authority"))?;
    if rebuilt.manifest.exporter_source_sha != manifest.exporter_source_sha
        || encoded(&rebuilt.manifest.capture)? != encoded(&manifest.capture)?
        || encoded(&rebuilt.manifest.schema)? != encoded(&manifest.schema)?
        || encoded(&rebuilt.manifest.resources)? != encoded(&manifest.resources)?
        || encoded(&rebuilt.manifest.processes)? != encoded(&manifest.processes)?
        || encoded(&expected_attestation.expectations)? != encoded(&attestation.expectations)?
        || expected_attestation.issuer != attestation.issuer
    {
        return Err(invalid("provenance_mismatch"));
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::contexts::sync::infrastructure::providers::ImmutableInventorySource;
    use serde_json::json;

    pub(crate) fn transport(provider: Uuid) -> Value {
        let contract: Value = serde_json::from_slice(SOURCE_CONTRACT).expect("shipped contract");
        let resources: Vec<Value> = contract["schema"]["models"]
            .as_array()
            .expect("models")
            .iter()
            .filter_map(|model| model["resource_key"].as_str())
            .map(|key| {
                let (app, name) = key.split_once('.').expect("canonical key");
                json!({"resource_key": key, "endpoint": format!("/api/{app}/{name}/"), "rows": []})
            })
            .collect();
        json!({
            "format_version": 1,
            "netbox_version": NETBOX_VERSION,
            "source_provider_id": provider,
            "exporter_source_sha": "a".repeat(40),
            "started_at": "2026-10-28T00:00:00Z",
            "ended_at": "2026-10-28T00:00:01Z",
            "contract_sha256": digest(SOURCE_CONTRACT),
            "schema": contract["schema"],
            "discovery": {"complete": true},
            "consistency": {"isolation_level": "repeatable read", "read_only": true, "snapshot": "1:1:"},
            "processes": PROCESS_AREAS.iter().map(|area| json!({
                "area": area, "status": "observed", "evidence": {"recorded": true}
            })).collect::<Vec<_>>(),
            "resources": resources
        })
    }

    #[test]
    fn exact_operator_endorsed_transport_seals_and_replays_unknown_empty_collections() {
        let provider = Uuid::now_v7();
        let operator = Uuid::now_v7();
        let bytes = encoded(&transport(provider)).expect("transport");
        let capture =
            capture_snapshot(&bytes, provider, operator, &digest(&bytes)).expect("capture");
        let private = tempfile::tempdir().expect("private root");
        let bundle = private.path().join("snapshot");
        let anchor = capture.seal(&bundle).expect("seal v2");
        let replay = ImmutableInventorySource::open(&bundle, &anchor, provider, 100)
            .expect("verified replay");
        let identity = replay
            .manifest()
            .authority
            .attestation_identity()
            .expect("identity")
            .expect("endorsed");
        verify_snapshot_provenance(&replay).expect("exact provenance binding");
        assert_eq!(identity.operator_id, operator);
        assert_eq!(identity.provenance_sha256, digest(&bytes));
        assert_eq!(
            replay.verified_metadata("snapshot-transport.json"),
            Some(bytes.as_slice())
        );
        assert!(
            replay
                .census()
                .iter()
                .all(|resource| resource.observed_rows == 0)
        );
    }

    #[test]
    fn endorsement_and_source_identity_are_independent_inputs() {
        let provider = Uuid::now_v7();
        let operator = Uuid::now_v7();
        let bytes = encoded(&transport(provider)).expect("transport");
        assert!(capture_snapshot(&bytes, provider, operator, &"0".repeat(64)).is_err());
        assert!(capture_snapshot(&bytes, Uuid::now_v7(), operator, &digest(&bytes)).is_err());
        assert!(capture_snapshot(&bytes, provider, Uuid::nil(), &digest(&bytes)).is_err());
    }

    #[test]
    fn orm_tag_ids_are_normalized_without_changing_endorsed_transport() {
        let provider = Uuid::now_v7();
        let mut value = transport(provider);
        for resource in value["resources"].as_array_mut().expect("resources") {
            match resource["resource_key"].as_str().expect("key") {
                "dcim.sites" => resource["rows"] = json!([{"id": 1, "tags": [7, 8]}]),
                "extras.tags" => resource["rows"] = json!([{"id": 7}, {"id": 8}]),
                _ => {}
            }
        }
        let bytes = encoded(&value).expect("transport");
        let capture = capture_snapshot(&bytes, provider, Uuid::now_v7(), &digest(&bytes))
            .expect("capture tag references");
        let private = tempfile::tempdir().expect("synthetic root");
        let bundle = private.path().join("tags");
        let anchor = capture.seal(&bundle).expect("seal");
        let replay = ImmutableInventorySource::open(&bundle, &anchor, provider, 100)
            .expect("replay expectations");
        verify_snapshot_provenance(&replay).expect("rebuild exact provenance");
        assert_eq!(
            replay.verified_metadata("snapshot-transport.json"),
            Some(bytes.as_slice())
        );
        let snapshot = replay.snapshot();
        let site = snapshot
            .records
            .iter()
            .find(|r| r.identity.entity.as_str() == "dcim.sites")
            .expect("site");
        assert_eq!(site.fields["tags"], json!([{"id": "7"}, {"id": "8"}]));

        for tags in [json!([0]), json!([{}]), json!([7, "007"]), json!("7")] {
            let mut bad = value.clone();
            let site = bad["resources"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|r| r["resource_key"] == "dcim.sites")
                .unwrap();
            site["rows"][0]["tags"] = tags;
            let bytes = encoded(&bad).unwrap();
            assert!(capture_snapshot(&bytes, provider, Uuid::now_v7(), &digest(&bytes)).is_err());
        }
    }

    #[test]
    fn verified_orm_tag_census_guards_prepared_graph() {
        use crate::contexts::sync::domain::inventory_preflight::FindingCode;
        use crate::contexts::sync::domain::inventory_target_schema::Absent;
        use crate::contexts::sync::domain::{
            ConflictMode, EntityMapping, FieldMapping, InventoryEntity, MappingConfigDocument,
            MappingConfigurationStatus, MappingDirection, MappingRef, MappingTarget,
            TypedInventoryBatch,
        };
        use crate::contexts::sync::infrastructure::persistence::local_inventory_target::{
            assess_preparation, assess_preparation_from_source,
        };

        let provider = Uuid::now_v7();
        let mut value = transport(provider);
        for resource in value["resources"].as_array_mut().unwrap() {
            match resource["resource_key"].as_str().unwrap() {
                "dcim.sites" => {
                    resource["rows"] = json!([
                        {"id": 1, "name": "Site A", "slug": "site-a", "tags": [7]},
                        {"id": 2, "name": "Site B", "slug": "site-b", "tags": []}
                    ])
                }
                "extras.tags" => {
                    resource["rows"] = json!([
                        {"id": 7, "name": "Tag A", "slug": "tag-a"},
                        {"id": 8, "name": "Tag B", "slug": "tag-b"}
                    ])
                }
                _ => {}
            }
        }
        let bytes = encoded(&value).unwrap();
        let capture = capture_snapshot(&bytes, provider, Uuid::now_v7(), &digest(&bytes)).unwrap();
        let root = tempfile::tempdir().unwrap();
        let bundle = root.path().join("graph");
        let anchor = capture.seal(&bundle).unwrap();
        let source = ImmutableInventorySource::open(&bundle, &anchor, provider, 100).unwrap();
        let evidence = source
            .verified_tag_relationships()
            .expect("canonical provenance");
        let snapshot = source.snapshot();
        assert_eq!(snapshot.records.len(), 4);
        assert!(
            !snapshot
                .records
                .iter()
                .any(|r| r.identity.entity.as_str() == InventoryEntity::TaggedObject.source_key())
        );
        assert!(evidence.matches(&snapshot, provider));
        let mut altered = snapshot.clone();
        altered.records.pop();
        assert!(!evidence.matches(&altered, provider));
        assert!(!evidence.matches(&snapshot, Uuid::now_v7()));

        let logical = Uuid::from_u128(22);
        let mut config = MappingConfigDocument {
            absence_policy: Default::default(),
            reference: MappingRef::new(logical, 1),
            name: "synthetic".into(),
            description: None,
            source_provider_id: provider,
            target: MappingTarget::LocalInventory,
            status: MappingConfigurationStatus::Active,
            direction: MappingDirection::Pull,
            conflict_mode: ConflictMode::NetboxWins,
            created_by: None,
            created_at: Utc::now(),
            retired_at: None,
            entity_mappings: Vec::new(),
            field_mappings: Vec::new(),
        };
        for (entity, names) in [
            (InventoryEntity::Site, &["name", "slug", "tags"][..]),
            (InventoryEntity::Tag, &["name", "slug"][..]),
        ] {
            let mapping_id = Uuid::new_v5(&logical, entity.source_key().as_bytes());
            config.entity_mappings.push(EntityMapping {
                id: mapping_id,
                mapping_config_logical_id: logical,
                mapping_config_version_id: 1,
                source_entity: entity.source_key().into(),
                target_entity: entity.target_key(),
                enabled: true,
                created_at: Utc::now(),
            });
            for field in entity
                .field_iter()
                .filter(|field| names.contains(&field.name()) || field.absent() == Absent::Reject)
            {
                let name = field.name();
                let expression = if names.contains(&name) {
                    json!({"op": "identity", "args": [{"field": name}]})
                } else {
                    json!({"op": "literal", "args": [if name == "status" { "active" } else { "" }]})
                };
                config.field_mappings.push(FieldMapping {
                    id: Uuid::now_v7(),
                    entity_mapping_id: mapping_id,
                    source_field: name.into(),
                    target_field: name.into(),
                    forward_transform_json: expression,
                    reverse_transform_json: None,
                    round_trip_critical: false,
                    created_at: Utc::now(),
                });
            }
        }
        let good = assess_preparation_from_source(&config, &source);
        let batch = good
            .batch
            .expect(&format!("verified graph: {:?}", good.findings));
        assert_eq!(batch.rows().len(), snapshot.records.len());
        assert!(
            assess_preparation(&config, &snapshot).batch.is_none(),
            "ordinary snapshots require independent memberships"
        );
        let site_mapping = config
            .entity_mappings
            .iter()
            .find(|m| m.source_entity == "dcim.sites")
            .unwrap()
            .id;
        for expression in [
            json!({"op": "literal", "args": [[]]}),
            json!({"op": "literal", "args": [[{"id": "7"}, {"id": "8"}]]}),
            json!({"op": "literal", "args": [[{"id": "8"}]]}),
        ] {
            let mut changed = config.clone();
            changed
                .field_mappings
                .iter_mut()
                .find(|m| m.entity_mapping_id == site_mapping && m.target_field == "tags")
                .unwrap()
                .forward_transform_json = expression;
            let assessment = assess_preparation_from_source(&changed, &source);
            assert!(assessment.batch.is_none());
            assert!(
                assessment
                    .findings
                    .iter()
                    .any(|f| f.code == FindingCode::TagGraphMismatch),
                "{:?}",
                assessment.findings
            );
        }
        let mut omitted = config.clone();
        omitted
            .field_mappings
            .retain(|m| !(m.entity_mapping_id == site_mapping && m.target_field == "tags"));
        let assessment = assess_preparation_from_source(&omitted, &source);
        assert!(assessment.batch.is_none());
        assert!(
            assessment
                .findings
                .iter()
                .any(|f| f.field_path.as_deref() == Some("tags.graph.actual"))
        );
        let mut wrong_provider = config.clone();
        wrong_provider.source_provider_id = Uuid::now_v7();
        assert!(
            assess_preparation_from_source(&wrong_provider, &source)
                .batch
                .is_none()
        );
    }

    #[test]
    fn populated_internal_cable_relations_seal_and_replay_without_reverse_api_arrays() {
        use crate::contexts::sync::domain::inventory_export::SnapshotExpectations;

        let provider = Uuid::now_v7();
        let mut value = transport(provider);
        for resource in value["resources"].as_array_mut().expect("resources") {
            match resource["resource_key"].as_str().expect("key") {
                "dcim.cables" => resource["rows"] = json!([{"id": 1, "label": "cable"}]),
                "dcim.interfaces" => {
                    resource["rows"] = json!([
                        {"id": 2, "cable": 1, "cable_end": "A"},
                        {"id": 3, "cable": 1, "cable_end": "B"}
                    ])
                }
                "dcim.cable-terminations" => {
                    resource["rows"] = json!([
                        {"id": 7, "cable": 1, "cable_end": "A", "connector": 2,
                            "positions": [1024, 1, 1], "termination_type": "dcim.interface", "termination": 2},
                        {"id": 8, "cable": 1, "cable_end": "B", "connector": null,
                            "positions": null, "termination_type": "dcim.interface", "termination": 3}
                    ])
                }
                _ => {}
            }
        }
        let bytes = encoded(&value).expect("transport");
        let capture = capture_snapshot(&bytes, provider, Uuid::now_v7(), &digest(&bytes))
            .expect("populated source-only cable relations are supported");
        let expectations: SnapshotExpectations =
            serde_json::from_slice(&capture.payloads["snapshot-expectations.json"])
                .expect("exact typed census");
        let relation_edges: Vec<_> = expectations
            .edges
            .iter()
            .filter(|edge| edge.from_entity.as_str() == "dcim.cable-terminations")
            .collect();
        assert_eq!(relation_edges.len(), 4);
        for id in ["7", "8"] {
            assert!(relation_edges.iter().any(|edge| edge.from_external_id == id
                && edge.field == "cable"
                && edge.to_entity.as_str() == "dcim.cables"
                && edge.to_external_id == "1"));
            assert!(relation_edges.iter().any(|edge| edge.from_external_id == id
                && edge.field == "termination"
                && edge.to_entity.as_str() == "dcim.interfaces"
                && edge.to_external_id == if id == "7" { "2" } else { "3" }));
        }
        let private = tempfile::tempdir().expect("synthetic bundle root");
        let bundle = private.path().join("cable-relations");
        let anchor = capture.seal(&bundle).expect("seal exact cable graph");
        let replay = ImmutableInventorySource::open(&bundle, &anchor, provider, 100)
            .expect("verified cable graph replay");
        verify_snapshot_provenance(&replay).expect("exact producer and contract binding");
        let snapshot = replay.snapshot();
        let relation = snapshot
            .records
            .iter()
            .find(|record| {
                record.identity.entity.as_str() == "dcim.cable-terminations"
                    && record.identity.external_id == "7"
            })
            .expect("retained source relation");
        assert_eq!(relation.fields["positions"], json!([1024, 1, 1]));
        assert_eq!(relation.fields["connector"], json!(2));
        assert!(
            !snapshot
                .records
                .iter()
                .find(|record| record.identity.entity.as_str() == "dcim.cables")
                .unwrap()
                .fields
                .contains_key("a_terminations")
        );
    }

    #[test]
    fn selected_custom_field_values_and_definitions_survive_the_same_snapshot() {
        let provider = Uuid::now_v7();
        let mut value = transport(provider);
        let resources = value["resources"].as_array_mut().expect("resources");
        for resource in resources {
            match resource["resource_key"].as_str().expect("key") {
                "dcim.sites" => {
                    resource["rows"] = json!([{
                        "id": 1, "name": "site", "slug": "site",
                        "custom_fields": {"asset_label": "rack-a"}
                    }])
                }
                "extras.custom-fields" => {
                    resource["rows"] = json!([{
                        "id": 7, "name": "asset_label", "type": "text",
                    "object_types": ["dcim.site"], "related_object_type": null, "choice_set": null
                    }])
                }
                _ => {}
            }
        }
        let bytes = encoded(&value).expect("transport");
        let capture = capture_snapshot(&bytes, provider, Uuid::now_v7(), &digest(&bytes))
            .expect("CF values have captured definitions");
        let private = tempfile::tempdir().expect("private root");
        let bundle = private.path().join("custom-fields");
        let anchor = capture.seal(&bundle).expect("seal typed CF snapshot");
        let source = ImmutableInventorySource::open(&bundle, &anchor, provider, 100)
            .expect("replay typed CF snapshot");
        verify_snapshot_provenance(&source).expect("provenance matches CF definitions and values");
        let snapshot = source.snapshot();
        let site = snapshot
            .records
            .iter()
            .find(|record| record.identity.entity.as_str() == "dcim.sites")
            .expect("site");
        assert_eq!(site.fields["custom_fields"]["asset_label"], json!("rack-a"));
    }

    #[test]
    fn every_canonical_generic_reference_is_an_edge_and_requires_its_snapshot_target() {
        use crate::contexts::sync::domain::inventory_export::SnapshotExpectations;

        for (key, field, type_field, token, target) in [
            (
                "ipam.ip-addresses",
                "assigned_object",
                "assigned_object_type",
                "dcim.interface",
                "dcim.interfaces",
            ),
            (
                "ipam.prefixes",
                "scope",
                "scope_type",
                "dcim.site",
                "dcim.sites",
            ),
            (
                "ipam.services",
                "parent",
                "parent_object_type",
                "dcim.device",
                "dcim.devices",
            ),
        ] {
            let provider = Uuid::now_v7();
            let mut value = transport(provider);
            let resource = value["resources"]
                .as_array_mut()
                .expect("resources")
                .iter_mut()
                .find(|resource| resource["resource_key"] == key)
                .expect("generic-reference collection");
            let mut row = json!({"id": 1});
            row[field] = json!(99);
            row[type_field] = json!(token);
            resource["rows"] = json!([row]);
            let bytes = encoded(&value).expect("transport");
            let capture = capture_snapshot(&bytes, provider, Uuid::now_v7(), &digest(&bytes))
                .expect("derive generic edge");
            let expectations: SnapshotExpectations =
                serde_json::from_slice(&capture.payloads["snapshot-expectations.json"])
                    .expect("expectations");
            assert!(expectations.edges.iter().any(|edge| edge.field == field
                && edge.to_entity.as_str() == target
                && edge.to_external_id == "99"));
            let private = tempfile::tempdir().expect("private root");
            let error = capture
                .seal(&private.path().join("missing-reference"))
                .err()
                .expect("missing target cannot be sealed as complete");
            assert!(
                matches!(error, AppError::Validation(code) if code == "inventory_export:authority_edge_mismatch")
            );
        }
    }

    #[test]
    fn unsupported_custom_field_choice_set_cannot_be_silently_lost() {
        let provider = Uuid::now_v7();
        let mut value = transport(provider);
        value["resources"]
            .as_array_mut()
            .expect("resources")
            .iter_mut()
            .find(|resource| resource["resource_key"] == "extras.custom-fields")
            .expect("CF definitions")["rows"] = json!([{
            "id": 7, "name": "asset_label", "type": "text", "choice_set": 19,
            "object_types": ["dcim.site"], "related_object_type": null
        }]);
        let bytes = encoded(&value).expect("transport");
        let error = capture_snapshot(&bytes, provider, Uuid::now_v7(), &digest(&bytes))
            .err()
            .expect("unsupported choice reference must block");
        assert!(
            matches!(error, AppError::Validation(code) if code == "inventory_export:authority_edge_type")
        );
    }

    #[test]
    fn orm_generic_ids_and_type_only_records_cannot_hide_an_edge_from_snapshot_intake() {
        let provider = Uuid::now_v7();
        for row in [
            json!({"id": 1, "assigned_object_type": "dcim.interface", "assigned_object_id": 99}),
            json!({"id": 1, "assigned_object_type": "dcim.interface"}),
        ] {
            let mut value = transport(provider);
            value["resources"]
                .as_array_mut()
                .expect("resources")
                .iter_mut()
                .find(|resource| resource["resource_key"] == "ipam.ip-addresses")
                .expect("IP collection")["rows"] = json!([row]);
            let bytes = encoded(&value).expect("transport");
            assert!(capture_snapshot(&bytes, provider, Uuid::now_v7(), &digest(&bytes)).is_err());
        }
    }

    #[test]
    fn template_component_pair_distinguishes_empty_from_populated_without_type_evidence() {
        let contract: Value = serde_json::from_slice(SOURCE_CONTRACT).expect("contract");
        let models = contract["schema"]["models"].as_array().expect("models");
        let model = models
            .iter()
            .find(|model| model["model"] == "dcim.inventoryitemtemplate")
            .expect("inventory item template schema");
        let exported_types = models
            .iter()
            .filter(|model| model["disposition"] == "export")
            .filter_map(|model| model["model"].as_str())
            .collect();
        let entity = SourceEntity::new("dcim.inventory-item-templates").expect("entity");
        for (row, valid) in [
            (
                json!({"id": 1, "component": null, "component_type": null}),
                true,
            ),
            (
                json!({"id": 1, "component": 99, "component_type": "dcim.frontporttemplate"}),
                true,
            ),
            (
                json!({"id": 1, "component": {"id": 99}, "component_type": "dcim.frontporttemplate"}),
                true,
            ),
            (json!({"id": 1, "component": 99}), false),
            (
                json!({"id": 1, "component_id": 99, "component_type": "dcim.frontporttemplate"}),
                false,
            ),
            (
                json!({"id": 1, "component": 99, "component_type": null}),
                false,
            ),
            (
                json!({"id": 1, "component": null, "component_type": "dcim.frontporttemplate"}),
                false,
            ),
            (
                json!({"id": 1, "component": 99, "component_type": "auth.user"}),
                false,
            ),
        ] {
            let provider = Uuid::now_v7();
            let record = record_from_value(provider, &entity, row).expect("source record");
            assert_eq!(record.identity.source_provider_id, provider);
            assert_eq!(
                validate_generic_projection(&record, model, &exported_types).is_ok(),
                valid
            );
        }
    }

    #[test]
    fn template_component_integer_and_object_ids_reconcile_to_the_same_typed_source_edge() {
        use crate::contexts::sync::domain::inventory_export::SnapshotExpectations;

        // This control intentionally requires Main's integrated template catalog;
        // the exporter checkpoint does not introduce a parallel entity mapping.
        for component in [
            json!(99),
            json!({"id": 99, "object_type": "dcim.frontporttemplate"}),
        ] {
            let provider = Uuid::now_v7();
            let mut value = transport(provider);
            for resource in value["resources"].as_array_mut().expect("resources") {
                if resource["resource_key"] == "dcim.inventory-item-templates" {
                    resource["rows"] = json!([{
                        "id": 1, "component_type": "dcim.frontporttemplate", "component": component
                    }]);
                } else if resource["resource_key"] == "dcim.front-port-templates" {
                    resource["rows"] = json!([{"id": 99}]);
                }
            }
            let bytes = encoded(&value).expect("transport");
            let capture = capture_snapshot(&bytes, provider, Uuid::now_v7(), &digest(&bytes))
                .expect("integrated template catalog types the component edge");
            let expectations: SnapshotExpectations =
                serde_json::from_slice(&capture.payloads["snapshot-expectations.json"])
                    .expect("expectations");
            assert!(
                expectations
                    .edges
                    .iter()
                    .any(
                        |edge| edge.from_entity.as_str() == "dcim.inventory-item-templates"
                            && edge.from_external_id == "1"
                            && edge.field == "component"
                            && edge.to_entity.as_str() == "dcim.front-port-templates"
                            && edge.to_external_id == "99"
                    )
            );
            let private = tempfile::tempdir().expect("private root");
            let bundle = private.path().join("typed-template-component");
            let anchor = capture.seal(&bundle).expect("reconciled component target");
            let source = ImmutableInventorySource::open(&bundle, &anchor, provider, 100)
                .expect("source-qualified template records");
            assert!(
                source
                    .snapshot()
                    .records
                    .iter()
                    .all(|record| record.identity.source_provider_id == provider)
            );
        }
    }

    #[test]
    fn regenerated_record_expectations_cannot_reuse_endorsement_of_different_values() {
        let provider = Uuid::now_v7();
        let mut value = transport(provider);
        let sites = value["resources"]
            .as_array_mut()
            .expect("resources")
            .iter_mut()
            .find(|resource| resource["resource_key"] == "dcim.sites")
            .expect("sites");
        sites["rows"] = json!([{"id": 1, "name": "endorsed-name", "slug": "endorsed-name"}]);
        let bytes = encoded(&value).expect("transport");
        let mut capture =
            capture_snapshot(&bytes, provider, Uuid::now_v7(), &digest(&bytes)).expect("capture");
        let mut records_by_entity = BTreeMap::new();
        for resource in &mut capture.manifest.resources {
            let descriptor = resource.payload.as_mut().expect("record payload");
            let mut records: Vec<SourceRecord> =
                serde_json::from_slice(&capture.payloads[&descriptor.path]).expect("records");
            if resource.resource_key == "dcim.sites" {
                records[0]
                    .fields
                    .insert("name".into(), json!("not-endorsed"));
                *descriptor = payload(
                    &mut capture.payloads,
                    descriptor.path.clone(),
                    encoded(&records).expect("records"),
                );
            }
            records_by_entity.insert(resource.resource_key.clone(), records);
        }
        let regenerated = snapshot_expectations(&records_by_entity).expect("coherent expectations");
        let attestation = capture
            .manifest
            .authority
            .snapshot
            .as_mut()
            .expect("attestation");
        attestation.expectations = payload(
            &mut capture.payloads,
            attestation.expectations.path.clone(),
            encoded(&regenerated).expect("expectations"),
        );
        let private = tempfile::tempdir().expect("private root");
        let bundle = private.path().join("substitution");
        let anchor = capture
            .seal(&bundle)
            .expect("structurally consistent bundle");
        assert!(matches!(
            ImmutableInventorySource::open(&bundle, &anchor, provider, 100),
            Err(AppError::Validation(code)) if code == "netbox_snapshot:provenance_mismatch"
        ));
    }

    #[test]
    fn unsafe_or_incomplete_transport_cannot_be_relabelled_as_a_snapshot() {
        let provider = Uuid::now_v7();
        let operator = Uuid::now_v7();
        for mutation in 0..7 {
            let mut value = transport(provider);
            match mutation {
                0 => value["schema"]["models"] = json!([]),
                1 => value["contract_sha256"] = json!("0".repeat(64)),
                2 => value["consistency"]["read_only"] = json!(false),
                3 => value["consistency"]["isolation_level"] = json!("read committed"),
                4 => {
                    value["resources"].as_array_mut().expect("resources").pop();
                }
                5 => value["processes"][0]["status"] = json!("unobservable"),
                _ => value["discovery"]["complete"] = json!(false),
            }
            let bytes = encoded(&value).expect("transport");
            assert!(
                capture_snapshot(&bytes, provider, operator, &digest(&bytes)).is_err(),
                "mutation {mutation}"
            );
        }
    }
}
