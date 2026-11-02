//! Offline, digest-pinned replay. Construction verifies the whole bundle before
//! publishing any records. Fetches use only owned memory, never a live fallback.
//!
//! Linux descriptor-anchored opens use `/proc/self/fd` and `O_NOFOLLOW` on every
//! component. Unlike check-then-canonicalize, replacing a directory or symlink
//! during verification cannot redirect an already opened directory capability.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{DirBuilder, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};

use uuid::Uuid;

use crate::contexts::custom_fields::domain::CustomFieldType;
use crate::contexts::sync::domain::inventory_export::{
    AuthorityKind, EXPECTATIONS_FORMAT_VERSION, ExpectedEdge, ExpectedRecord, ExportManifest,
    MANIFEST_FILE, ProcessCoverage, ResourceCensus, SnapshotExpectations, digest, invalid,
    record_value_digest, safe_payload_path, valid_digest,
};
use crate::contexts::sync::domain::inventory_sync::normalize_netbox_id;
use crate::contexts::sync::domain::inventory_target_schema::{FieldKind, InventoryEntity};
use crate::contexts::sync::domain::{
    InventorySource, SourceEntity, SourceIdentity, SourcePage, SourceRecord,
};
use crate::errors::AppError;

/// Explicit allocation limits for this in-memory adapter, not source exclusions.
/// A larger capture needs a bounded storage implementation; exceeding a limit
/// always fails, never skips data or partitions away coverage evidence.
const MAX_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;
const MAX_PAYLOAD_BYTES: u64 = 512 * 1024 * 1024;
const MAX_BUNDLE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

pub struct ImmutableInventorySource {
    bundle_digest: String,
    artifact_digest: String,
    manifest_bytes: Vec<u8>,
    metadata: BTreeMap<String, Vec<u8>>,
    manifest: ExportManifest,
    records: BTreeMap<String, Vec<SourceRecord>>,
    tag_evidence: Option<VerifiedTagRelationships>,
    orm_provenance_verified: bool,
    page_size: usize,
}

/// Relationship census available only after complete operator authority verification.
pub(crate) struct VerifiedTagRelationships {
    provider_id: Uuid,
    values: BTreeMap<(String, String), String>,
    edges: BTreeMap<(String, String), BTreeSet<String>>,
}

impl VerifiedTagRelationships {
    pub(crate) fn matches(
        &self,
        snapshot: &crate::contexts::sync::domain::SourceSnapshot,
        provider_id: Uuid,
    ) -> bool {
        if provider_id != self.provider_id || snapshot.records.len() != self.values.len() {
            return false;
        }
        let mut seen = BTreeSet::new();
        snapshot.records.iter().all(|record| {
            let key = (
                record.identity.entity.as_str().to_owned(),
                record.identity.external_id.clone(),
            );
            record.identity.source_provider_id == provider_id
                && seen.insert(key.clone())
                && record_value_digest(record).ok().as_ref() == self.values.get(&key)
        })
    }

    pub(crate) fn edges(&self) -> &BTreeMap<(String, String), BTreeSet<String>> {
        &self.edges
    }
}

impl ImmutableInventorySource {
    /// The caller must obtain `expected_digest` independently of this bundle.
    /// The expected provider identity likewise comes from the calling context.
    /// Construction performs blocking local reads; async callers should use
    /// their blocking pool. No handle or record escapes failed verification.
    pub fn open(
        directory: &Path,
        expected_digest: &str,
        expected_provider_id: Uuid,
        page_size: usize,
    ) -> Result<Self, AppError> {
        if !valid_digest(expected_digest) {
            return Err(invalid("expected_digest"));
        }
        if page_size == 0 {
            return Err(invalid("page_size"));
        }
        let directory = open_directory(directory)?;
        let bytes = read_file(&directory, MANIFEST_FILE, MAX_MANIFEST_BYTES)?;
        if digest(&bytes) != expected_digest {
            return Err(invalid("bundle_digest"));
        }
        let manifest: ExportManifest =
            serde_json::from_slice(&bytes).map_err(|_| invalid("manifest_encoding"))?;
        manifest.validate()?;
        if manifest.source_provider_id != expected_provider_id {
            return Err(invalid("provider_identity"));
        }
        let mut payloads = BTreeMap::new();
        let mut total = 0u64;
        for descriptor in manifest.payloads() {
            total = total
                .checked_add(descriptor.byte_length)
                .ok_or_else(|| invalid("bundle_limit"))?;
            if total > MAX_BUNDLE_BYTES || descriptor.byte_length > MAX_PAYLOAD_BYTES {
                return Err(invalid("bundle_limit"));
            }
            let payload = read_file(&directory, &descriptor.path, descriptor.byte_length)?;
            if payload.len() as u64 != descriptor.byte_length
                || digest(&payload) != descriptor.sha256
            {
                return Err(invalid("payload_integrity"));
            }
            payloads.insert(descriptor.path.clone(), payload);
        }
        let records = verify_records(&manifest, &payloads)?;
        let tag_evidence = verify_authority(&manifest, &payloads, &records)?;
        // The artifact identity covers every verified original payload byte,
        // including record payloads that leave `metadata` below. It is computed
        // from those exact bytes, never by re-serialising parsed records.
        let artifact_digest = {
            let verified: Vec<(&str, &[u8])> = payloads
                .iter()
                .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
                .collect();
            crate::contexts::sync::domain::inventory_transfer::artifact_digest(&bytes, &verified)
                .ok_or_else(|| invalid("artifact_duplicate_path"))?
        };
        let record_paths: BTreeSet<_> = manifest
            .resources
            .iter()
            .filter_map(|resource| resource.payload.as_ref().map(|p| p.path.as_str()))
            .collect();
        payloads.retain(|path, _| !record_paths.contains(path.as_str()));
        drop(record_paths);
        let mut source = Self {
            bundle_digest: expected_digest.into(),
            artifact_digest,
            manifest_bytes: bytes,
            metadata: payloads,
            manifest,
            records,
            tag_evidence,
            orm_provenance_verified: false,
            page_size,
        };
        if source.metadata.contains_key("snapshot-transport.json") {
            super::netbox_snapshot_capture::verify_snapshot_provenance(&source)?;
            source.orm_provenance_verified = true;
        }
        Ok(source)
    }

    /// Merkle identity of the verified artifact: the exact manifest bytes plus
    /// every verified payload digest in path order.
    ///
    /// It is available even though record payloads are not kept in
    /// [`Self::verified_metadata`], so a descriptor can pin the whole bundle
    /// without the reader inventing a second serialization.
    pub fn artifact_digest(&self) -> &str {
        &self.artifact_digest
    }

    /// Full source evidence, independent of the local target's approved catalog.
    pub fn manifest(&self) -> &ExportManifest {
        &self.manifest
    }

    pub fn bundle_digest(&self) -> &str {
        &self.bundle_digest
    }

    /// Exact verified bytes, never a reread of mutable on-disk evidence.
    pub fn manifest_bytes(&self) -> &[u8] {
        &self.manifest_bytes
    }

    /// Private verified schema/discovery/consistency/process evidence. Never log.
    pub fn verified_metadata(&self, path: &str) -> Option<&[u8]> {
        self.metadata.get(path).map(Vec::as_slice)
    }

    pub fn snapshot(&self) -> crate::contexts::sync::domain::SourceSnapshot {
        crate::contexts::sync::domain::SourceSnapshot {
            records: self.records.values().flatten().cloned().collect(),
        }
    }

    pub(crate) fn verified_tag_relationships(&self) -> Option<&VerifiedTagRelationships> {
        self.tag_evidence
            .as_ref()
            .filter(|_| self.orm_provenance_verified)
    }

    pub(crate) fn orm_provenance_verified(&self) -> bool {
        self.orm_provenance_verified
    }

    pub fn census(&self) -> &[ResourceCensus] {
        &self.manifest.resources
    }

    pub fn process_coverage(&self) -> &[ProcessCoverage] {
        &self.manifest.processes
    }
}

impl InventorySource for ImmutableInventorySource {
    fn supported_entities(&self) -> Vec<SourceEntity> {
        // Includes captured empty resources. Inaccessible/unrepresentable
        // resources remain in census(), not falsely exposed as zero-row data.
        self.manifest
            .resources
            .iter()
            .filter(|r| r.payload.is_some())
            .filter_map(|r| r.entity.clone())
            .collect()
    }

    async fn fetch_page(
        &self,
        entity: &SourceEntity,
        cursor: Option<&str>,
    ) -> Result<SourcePage, AppError> {
        let records = self
            .records
            .get(entity.as_str())
            .ok_or_else(|| invalid("entity_not_captured"))?;
        let offset = match cursor {
            None => 0,
            Some(raw) => {
                let value = raw.parse::<usize>().map_err(|_| invalid("cursor"))?;
                if raw != value.to_string() || value == 0 || value % self.page_size != 0 {
                    return Err(invalid("cursor"));
                }
                value
            }
        };
        if offset > records.len() || (cursor.is_some() && offset == records.len()) {
            return Err(invalid("cursor"));
        }
        let end = offset.saturating_add(self.page_size).min(records.len());
        Ok(SourcePage {
            records: records[offset..end].to_vec(),
            next_cursor: (end < records.len()).then(|| end.to_string()),
        })
    }
}

fn verify_records(
    manifest: &ExportManifest,
    payloads: &BTreeMap<String, Vec<u8>>,
) -> Result<BTreeMap<String, Vec<SourceRecord>>, AppError> {
    let mut by_entity = BTreeMap::new();
    let mut identities = BTreeSet::new();
    for resource in &manifest.resources {
        let Some(descriptor) = &resource.payload else {
            continue;
        };
        let bytes = payloads
            .get(&descriptor.path)
            .ok_or_else(|| invalid("payload_missing"))?;
        let mut records: Vec<SourceRecord> =
            serde_json::from_slice(bytes).map_err(|_| invalid("record_encoding"))?;
        if records.len() as u64 != resource.observed_rows {
            return Err(invalid("observed_count"));
        }
        let entity = resource
            .entity
            .as_ref()
            .ok_or_else(|| invalid("resource_entity"))?;
        for record in &records {
            let identity = &record.identity;
            let checked_entity = SourceEntity::new(identity.entity.as_str())
                .map_err(|_| invalid("record_identity"))?;
            SourceIdentity::new(
                identity.source_provider_id,
                checked_entity.clone(),
                identity.external_id.clone(),
            )
            .map_err(|_| invalid("record_identity"))?;
            let canonical_id =
                normalize_netbox_id(&serde_json::Value::String(identity.external_id.clone()))
                    .map_err(|_| invalid("record_identity"))?;
            if identity.source_provider_id != manifest.source_provider_id
                || identity.entity != *entity
                || checked_entity != identity.entity
                || canonical_id != identity.external_id
                || !identities.insert((entity.as_str().to_owned(), identity.external_id.clone()))
            {
                return Err(invalid("record_identity"));
            }
        }
        // Stable lexical identity order, independent of exporter row ordering.
        records.sort_by(|a, b| a.identity.external_id.cmp(&b.identity.external_id));
        by_entity.insert(entity.as_str().to_owned(), records);
    }
    Ok(by_entity)
}

/// One source custom-field definition captured in the same bundle.
#[derive(Clone, Copy)]
struct SourceCustomField {
    field_type: CustomFieldType,
    related: RelatedSource,
}

/// Whether the definition names a resolvable related object type.
#[derive(Clone, Copy)]
enum RelatedSource {
    /// Not an object/multiobject field.
    NotApplicable,
    /// The exact source entity its values must reference.
    Known(InventoryEntity),
    /// Object type declared but absent or not a supported entity.
    Unresolved,
}

/// One reference a captured record really carries, typed by the canonical
/// catalog or by a source custom-field definition.
struct TypedEdge {
    implied: Option<InventoryEntity>,
    target: String,
}

/// Derive the exact record and typed-reference census for a captured snapshot.
pub(crate) fn snapshot_expectations(
    records: &BTreeMap<String, Vec<SourceRecord>>,
) -> Result<SnapshotExpectations, AppError> {
    let mut expected_records = Vec::new();
    for (entity, list) in records {
        for record in list {
            expected_records.push(ExpectedRecord {
                entity: SourceEntity::new(entity)?,
                external_id: record.identity.external_id.clone(),
                value_sha256: record_value_digest(record)?,
            });
        }
    }
    let definitions = custom_field_definitions(records)?;
    let mut edges = Vec::new();
    for ((entity, external_id, field), edge) in typed_edges(records, &definitions)? {
        let target = edge.implied.ok_or_else(|| invalid("authority_edge_type"))?;
        edges.push(ExpectedEdge {
            from_entity: SourceEntity::new(&entity)?,
            from_external_id: external_id,
            field,
            to_entity: SourceEntity::new(target.source_key())?,
            to_external_id: edge.target,
        });
    }
    Ok(SnapshotExpectations {
        format_version: EXPECTATIONS_FORMAT_VERSION,
        records: expected_records,
        edges,
    })
}

/// A snapshot attestation is authority only when the exact
/// expectations describe exactly these records, values and typed edges.
/// Anything less is rejected before a single record is exposed.
fn verify_authority(
    manifest: &ExportManifest,
    payloads: &BTreeMap<String, Vec<u8>>,
    records: &BTreeMap<String, Vec<SourceRecord>>,
) -> Result<Option<VerifiedTagRelationships>, AppError> {
    if manifest.authority.kind != AuthorityKind::OperatorEndorsedSnapshot {
        return Ok(None);
    }
    let attestation = manifest
        .authority
        .snapshot
        .as_ref()
        .ok_or_else(|| invalid("authority"))?;
    let bytes = payloads
        .get(&attestation.expectations.path)
        .ok_or_else(|| invalid("authority_expectations"))?;
    let expectations: SnapshotExpectations =
        serde_json::from_slice(bytes).map_err(|_| invalid("authority_expectations"))?;
    if expectations.format_version != EXPECTATIONS_FORMAT_VERSION {
        return Err(invalid("authority_expectations"));
    }
    let total: usize = records.values().map(Vec::len).sum();
    if expectations.records.len() != total {
        return Err(invalid("authority_expectations"));
    }
    let mut expected_values = BTreeMap::new();
    for expected in &expectations.records {
        if !canonical_entity(&expected.entity)
            || expected.external_id.trim().is_empty()
            || !valid_digest(&expected.value_sha256)
        {
            return Err(invalid("authority_expectations"));
        }
        let record = records
            .get(expected.entity.as_str())
            .and_then(|list| {
                list.iter()
                    .find(|record| record.identity.external_id == expected.external_id)
            })
            .ok_or_else(|| invalid("authority_expectations"))?;
        if record_value_digest(record)? != expected.value_sha256
            || expected_values
                .insert(
                    (
                        expected.entity.as_str().to_owned(),
                        expected.external_id.clone(),
                    ),
                    record,
                )
                .is_some()
        {
            return Err(invalid("authority_expectations"));
        }
    }

    // The typed census is derived from the canonical catalog and from the
    // captured custom-field definitions, never from the expectation document.
    let definitions = custom_field_definitions(records)?;
    let typed = typed_edges(records, &definitions)?;

    let mut declared = BTreeSet::new();
    for expected in &expectations.edges {
        if !canonical_entity(&expected.from_entity)
            || !canonical_entity(&expected.to_entity)
            || !canonical_external_id(&expected.from_external_id)
            || !canonical_external_id(&expected.to_external_id)
        {
            return Err(invalid("authority_expectations"));
        }
        let key = (
            expected.from_entity.as_str().to_owned(),
            expected.from_external_id.clone(),
            expected.field.clone(),
        );
        if !declared.insert(key.clone()) {
            return Err(invalid("authority_expectations"));
        }
        // The declared edge must be one this record really carries, with the
        // exact target id the field holds.
        let derived = typed
            .get(&key)
            .ok_or_else(|| invalid("authority_edge_mismatch"))?;
        if derived.target != expected.to_external_id {
            return Err(invalid("authority_edge_mismatch"));
        }
        // A field implies its target entity; an expectation cannot choose it.
        let implied = derived
            .implied
            .ok_or_else(|| invalid("authority_edge_type"))?;
        if implied.source_key() != expected.to_entity.as_str() {
            return Err(invalid("authority_edge_type"));
        }
        if expected_values
            .get(&(
                expected.to_entity.as_str().to_owned(),
                expected.to_external_id.clone(),
            ))
            .is_none()
        {
            return Err(invalid("authority_edge_mismatch"));
        }
    }
    // No uncovered reference may survive: the declaration must be exact.
    if declared.len() != typed.len() || typed.keys().any(|key| !declared.contains(key)) {
        return Err(invalid("authority_edge_mismatch"));
    }
    let values = expectations
        .records
        .iter()
        .map(|record| {
            (
                (
                    record.entity.as_str().to_owned(),
                    record.external_id.clone(),
                ),
                record.value_sha256.clone(),
            )
        })
        .collect();
    let mut edges = BTreeMap::<(String, String), BTreeSet<String>>::new();
    for edge in &expectations.edges {
        if edge.field.starts_with("tags[")
            && edge.to_entity.as_str() == InventoryEntity::Tag.source_key()
        {
            edges
                .entry((
                    edge.from_entity.as_str().to_owned(),
                    edge.from_external_id.clone(),
                ))
                .or_default()
                .insert(edge.to_external_id.clone());
        }
    }
    Ok(Some(VerifiedTagRelationships {
        provider_id: manifest.source_provider_id,
        values,
        edges,
    }))
}

fn canonical_entity(entity: &SourceEntity) -> bool {
    SourceEntity::new(entity.as_str())
        .ok()
        .is_some_and(|checked| checked == *entity)
}

fn canonical_external_id(value: &str) -> bool {
    normalize_netbox_id(&serde_json::Value::String(value.to_owned()))
        .is_ok_and(|canonical| canonical == value)
}

/// Exact source custom-field definitions: name, type and related object type.
/// A populated value with no definition is never treated as data.
fn custom_field_definitions(
    records: &BTreeMap<String, Vec<SourceRecord>>,
) -> Result<BTreeMap<String, SourceCustomField>, AppError> {
    let mut definitions = BTreeMap::new();
    let Some(list) = records.get(InventoryEntity::CustomField.source_key()) else {
        return Ok(definitions);
    };
    for record in list {
        let name = record
            .fields
            .get("name")
            .and_then(serde_json::Value::as_str)
            .filter(|name| !name.is_empty() && name.len() <= 128)
            .ok_or_else(|| invalid("authority_custom_field"))?;
        let raw_type = record
            .fields
            .get("type")
            .and_then(choice_text)
            .ok_or_else(|| invalid("authority_custom_field"))?;
        let field_type =
            CustomFieldType::parse(raw_type).ok_or_else(|| invalid("authority_custom_field"))?;
        let related = match record.fields.get("related_object_type") {
            None | Some(serde_json::Value::Null) => RelatedSource::NotApplicable,
            Some(serde_json::Value::String(token)) => {
                match InventoryEntity::from_content_type_token(token) {
                    Some(entity) => RelatedSource::Known(entity),
                    None => RelatedSource::Unresolved,
                }
            }
            Some(_) => return Err(invalid("authority_custom_field")),
        };
        if definitions
            .insert(
                name.to_owned(),
                SourceCustomField {
                    field_type,
                    related,
                },
            )
            .is_some()
        {
            return Err(invalid("authority_custom_field"));
        }
    }
    Ok(definitions)
}

/// A NetBox choice value is either the bare value or `{value, label}`.
fn choice_text(value: &serde_json::Value) -> Option<&str> {
    match value {
        serde_json::Value::String(text) => Some(text),
        serde_json::Value::Object(map) => map.get("value")?.as_str(),
        _ => None,
    }
}

/// The complete typed reference census of every captured record.
fn typed_edges(
    records: &BTreeMap<String, Vec<SourceRecord>>,
    definitions: &BTreeMap<String, SourceCustomField>,
) -> Result<BTreeMap<(String, String, String), TypedEdge>, AppError> {
    let mut edges = BTreeMap::new();
    for (entity_key, list) in records {
        if list.is_empty() {
            continue;
        }
        let entity = InventoryEntity::from_source_key(entity_key)
            .ok_or_else(|| invalid("authority_edge_type"))?;
        for record in list {
            for (path, edge) in record_edges(entity, record, definitions)? {
                let key = (
                    entity_key.clone(),
                    record.identity.external_id.clone(),
                    path,
                );
                if edges.insert(key, edge).is_some() {
                    return Err(invalid("authority_edge_type"));
                }
            }
        }
    }
    Ok(edges)
}

fn record_edges(
    entity: InventoryEntity,
    record: &SourceRecord,
    definitions: &BTreeMap<String, SourceCustomField>,
) -> Result<Vec<(String, TypedEdge)>, AppError> {
    if entity == InventoryEntity::CableTermination {
        return cable_termination_edges(record);
    }
    let mut edges = Vec::new();
    for field in entity.field_iter() {
        let Some(value) = record.fields.get(field.name()) else {
            continue;
        };
        let name = field.name();
        match field.kind() {
            FieldKind::Scalar => {}
            FieldKind::Tags => {
                for (index, item) in array_items(value)?.into_iter().enumerate() {
                    let target = reference_id(item, Some(InventoryEntity::Tag))?
                        .ok_or_else(|| invalid("authority_edge_shape"))?;
                    edges.push((
                        format!("{name}[{index}]"),
                        TypedEdge {
                            implied: Some(InventoryEntity::Tag),
                            target,
                        },
                    ));
                }
            }
            FieldKind::ForeignKey { target } => {
                // Custom-field content-type scope is validated separately and
                // never becomes an inventory-row edge.
                if entity == InventoryEntity::CustomField && name == "related_object_type" {
                    continue;
                }
                if target.is_none() && !value.is_null() {
                    return Err(invalid("authority_edge_type"));
                }
                if let Some(target_id) = reference_id(value, target)? {
                    edges.push((
                        name.to_owned(),
                        TypedEdge {
                            implied: target,
                            target: target_id,
                        },
                    ));
                }
            }
            FieldKind::GenericReference {
                source_type_field,
                allowed,
                ..
            } => {
                if value.is_null() {
                    continue;
                }
                let token = record
                    .fields
                    .get(source_type_field)
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| invalid("authority_edge_type"))?;
                let typed = InventoryEntity::from_content_type_token(token)
                    .filter(|typed| allowed.contains(typed))
                    .ok_or_else(|| invalid("authority_edge_type"))?;
                let target = reference_id(value, Some(typed))?
                    .ok_or_else(|| invalid("authority_edge_shape"))?;
                edges.push((
                    name.to_owned(),
                    TypedEdge {
                        implied: Some(typed),
                        target,
                    },
                ));
            }
            FieldKind::Junction { target, .. } => {
                for (index, item) in array_items(value)?.into_iter().enumerate() {
                    if target.is_none() {
                        return Err(invalid("authority_edge_type"));
                    }
                    let target_id = reference_id(item, target)?
                        .ok_or_else(|| invalid("authority_edge_shape"))?;
                    edges.push((
                        format!("{name}[{index}]"),
                        TypedEdge {
                            implied: target,
                            target: target_id,
                        },
                    ));
                }
            }
            FieldKind::ContentTypeJunction { .. } => {
                // Content-type scope is metadata, never an inventory row edge;
                // only the token itself is validated.
                for item in array_items(value)? {
                    let token = choice_text(item)
                        .or_else(|| item.get("object_type").and_then(serde_json::Value::as_str))
                        .ok_or_else(|| invalid("authority_edge_type"))?;
                    InventoryEntity::from_content_type_token(token)
                        .ok_or_else(|| invalid("authority_edge_type"))?;
                }
            }
        }
    }
    // `related_object_type` names content-type scope, not a target row; its
    // token is validated and no collection row is invented.
    if entity == InventoryEntity::CustomField {
        if let Some(token) = record
            .fields
            .get("related_object_type")
            .filter(|value| !value.is_null())
        {
            let token = token
                .as_str()
                .ok_or_else(|| invalid("authority_custom_field"))?;
            InventoryEntity::from_content_type_token(token)
                .ok_or_else(|| invalid("authority_custom_field"))?;
        }
    }
    for (name, value) in custom_field_values(record)? {
        if value.is_null() {
            continue;
        }
        let definition = definitions
            .get(name)
            .ok_or_else(|| invalid("authority_custom_field"))?;
        let (related, multiobject) = match definition.field_type {
            CustomFieldType::Object => (
                match definition.related {
                    RelatedSource::Known(entity) => entity,
                    _ => return Err(invalid("authority_custom_field")),
                },
                false,
            ),
            CustomFieldType::Multiobject => (
                match definition.related {
                    RelatedSource::Known(entity) => entity,
                    _ => return Err(invalid("authority_custom_field")),
                },
                true,
            ),
            _ => {
                // A declared scalar type makes the value data, never an edge.
                continue;
            }
        };
        let items: Vec<&serde_json::Value> = if multiobject {
            value
                .as_array()
                .ok_or_else(|| invalid("authority_edge_shape"))?
                .iter()
                .collect()
        } else {
            vec![value]
        };
        for (index, item) in items.into_iter().enumerate() {
            let target = reference_id(item, Some(related))?
                .ok_or_else(|| invalid("authority_edge_shape"))?;
            let path = if multiobject {
                format!("custom_fields.{name}[{index}]")
            } else {
                format!("custom_fields.{name}")
            };
            edges.push((
                path,
                TypedEdge {
                    implied: Some(related),
                    target,
                },
            ));
        }
    }
    Ok(edges)
}

/// Cable terminations are membership evidence, not independent target rows.
fn cable_termination_edges(record: &SourceRecord) -> Result<Vec<(String, TypedEdge)>, AppError> {
    if !canonical_external_id(&record.identity.external_id) {
        return Err(invalid("authority_edge_id"));
    }
    let cable = record
        .fields
        .get("cable")
        .ok_or_else(|| invalid("authority_edge_shape"))?;
    let cable = termination_reference_id(cable, InventoryEntity::Cable)?
        .ok_or_else(|| invalid("authority_edge_shape"))?;
    let termination = record
        .fields
        .get("termination")
        .ok_or_else(|| invalid("authority_edge_shape"))?;
    let token = record
        .fields
        .get("termination_type")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| invalid("authority_edge_type"))?;
    let target = InventoryEntity::from_content_type_token(token)
        .filter(|entity| *entity == InventoryEntity::Interface)
        .ok_or_else(|| invalid("authority_edge_type"))?;
    let termination = termination_reference_id(termination, target)?
        .ok_or_else(|| invalid("authority_edge_shape"))?;
    if !matches!(
        record
            .fields
            .get("cable_end")
            .and_then(serde_json::Value::as_str),
        Some("A" | "B")
    ) {
        return Err(invalid("authority_edge_shape"));
    }
    match record.fields.get("connector") {
        Some(serde_json::Value::Null) => {}
        Some(value) if value.as_u64().is_some_and(|n| n <= i16::MAX as u64) => {}
        _ => return Err(invalid("authority_edge_shape")),
    }
    match record.fields.get("positions") {
        Some(serde_json::Value::Null) => {}
        Some(serde_json::Value::Array(values))
            if values
                .iter()
                .all(|value| value.as_u64().is_some_and(|n| (1..=1024).contains(&n))) => {}
        _ => return Err(invalid("authority_edge_shape")),
    }
    Ok(vec![
        (
            "cable".into(),
            TypedEdge {
                implied: Some(InventoryEntity::Cable),
                target: cable,
            },
        ),
        (
            "termination".into(),
            TypedEdge {
                implied: Some(target),
                target: termination,
            },
        ),
    ])
}

fn termination_reference_id(
    value: &serde_json::Value,
    entity: InventoryEntity,
) -> Result<Option<String>, AppError> {
    let id = reference_id(value, Some(entity))?;
    let raw = value.get("id").unwrap_or(value);
    if let (Some(text), Some(normalized)) = (raw.as_str(), id.as_deref()) {
        if text != normalized {
            return Err(invalid("authority_edge_id"));
        }
    }
    Ok(id)
}

fn custom_field_values(record: &SourceRecord) -> Result<Vec<(&str, &serde_json::Value)>, AppError> {
    match record.fields.get("custom_fields") {
        None | Some(serde_json::Value::Null) => Ok(Vec::new()),
        Some(serde_json::Value::Object(map)) => Ok(map
            .iter()
            .map(|(name, value)| (name.as_str(), value))
            .collect()),
        Some(_) => Err(invalid("authority_edge_shape")),
    }
}

fn array_items(value: &serde_json::Value) -> Result<Vec<&serde_json::Value>, AppError> {
    match value {
        serde_json::Value::Null => Ok(Vec::new()),
        serde_json::Value::Array(items) => Ok(items.iter().collect()),
        _ => Err(invalid("authority_edge_shape")),
    }
}

/// Read one provider id from a value, refusing a token that contradicts the
/// entity the canonical catalog implies for this field.
fn reference_id(
    value: &serde_json::Value,
    implied: Option<InventoryEntity>,
) -> Result<Option<String>, AppError> {
    let id = match value {
        serde_json::Value::Null => return Ok(None),
        serde_json::Value::Object(map) => {
            if let Some(token) = map.get("object_type") {
                let token = token
                    .as_str()
                    .and_then(InventoryEntity::from_content_type_token)
                    .ok_or_else(|| invalid("authority_edge_type"))?;
                if implied.is_some_and(|expected| expected != token) {
                    return Err(invalid("authority_edge_type"));
                }
            }
            map.get("id").ok_or_else(|| invalid("authority_edge_id"))?
        }
        other => other,
    };
    normalize_netbox_id(id)
        .map(Some)
        .map_err(|_| invalid("authority_edge_id"))
}

fn anchored(directory: &File, name: &str) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd())).join(name)
}

fn directory_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
    options
}

fn open_directory(path: &Path) -> Result<File, AppError> {
    let mut directory = directory_options()
        .open(if path.is_absolute() { "/" } else { "." })
        .map_err(|_| invalid("directory_open"))?;
    for component in path.components() {
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(name) => {
                // `name` is one OS component, never a path from the manifest.
                let next =
                    PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd())).join(name);
                directory = directory_options()
                    .open(next)
                    .map_err(|_| invalid("directory_open"))?;
            }
            _ => return Err(invalid("directory_path")),
        }
    }
    Ok(directory)
}

fn read_file(directory: &File, name: &str, limit: u64) -> Result<Vec<u8>, AppError> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(anchored(directory, name))
        .map_err(|_| invalid("file_open"))?;
    let metadata = file.metadata().map_err(|_| invalid("file_metadata"))?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(invalid("file_length"));
    }
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| invalid("file_read"))?;
    if bytes.len() as u64 > limit {
        return Err(invalid("file_length"));
    }
    Ok(bytes)
}

/// Seal already captured bytes into a NEW private directory. This is not a live
/// exporter. All data is verified before creation. Failures after creation leave
/// an incomplete directory for investigation; nothing is overwritten or resumed.
/// Returns the independent trust anchor to transfer out of band with the bundle.
pub fn seal_bundle(
    directory: &Path,
    manifest: &ExportManifest,
    payloads: &BTreeMap<String, Vec<u8>>,
) -> Result<String, AppError> {
    manifest.validate()?;
    let descriptors = manifest.payloads();
    if descriptors.len() != payloads.len() {
        return Err(invalid("payload_set"));
    }
    let mut total = 0u64;
    for descriptor in descriptors {
        let bytes = payloads
            .get(&descriptor.path)
            .ok_or_else(|| invalid("payload_missing"))?;
        total = total
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| invalid("bundle_limit"))?;
        if total > MAX_BUNDLE_BYTES || bytes.len() as u64 > MAX_PAYLOAD_BYTES {
            return Err(invalid("bundle_limit"));
        }
        if bytes.len() as u64 != descriptor.byte_length || digest(bytes) != descriptor.sha256 {
            return Err(invalid("payload_integrity"));
        }
    }
    let records = verify_records(manifest, payloads)?;
    verify_authority(manifest, payloads, &records)?;
    let manifest_bytes = serde_json::to_vec(manifest).map_err(|_| invalid("manifest_encoding"))?;
    if manifest_bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(invalid("bundle_limit"));
    }
    let parent = open_directory(
        directory
            .parent()
            .ok_or_else(|| invalid("directory_path"))?,
    )?;
    let name = directory
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| invalid("directory_path"))?;
    if !safe_payload_path(name) {
        return Err(invalid("directory_path"));
    }
    let destination = anchored(&parent, name);
    DirBuilder::new()
        .mode(0o700)
        .create(&destination)
        .map_err(|_| invalid("directory_create"))?;
    let directory = directory_options()
        .open(destination)
        .map_err(|_| invalid("directory_open"))?;
    for (name, bytes) in payloads {
        write_private_file(&directory, name, bytes)?;
    }
    // Publish a complete, synced manifest atomically. Hard-link creation is
    // no-clobber; the temporary seal is not a valid manifest during a crash.
    let pending = ".manifest.pending";
    write_private_file(&directory, pending, &manifest_bytes)?;
    std::fs::hard_link(
        anchored(&directory, pending),
        anchored(&directory, MANIFEST_FILE),
    )
    .map_err(|_| invalid("seal_publish"))?;
    std::fs::remove_file(anchored(&directory, pending)).map_err(|_| invalid("seal_cleanup"))?;
    directory
        .sync_all()
        .map_err(|_| invalid("directory_sync"))?;
    parent.sync_all().map_err(|_| invalid("directory_sync"))?;
    Ok(digest(&manifest_bytes))
}

fn write_private_file(directory: &File, name: &str, bytes: &[u8]) -> Result<(), AppError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(anchored(directory, name))
        .map_err(|_| invalid("file_create"))?;
    file.write_all(bytes).map_err(|_| invalid("file_write"))?;
    file.sync_all().map_err(|_| invalid("file_sync"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    use chrono::Utc;
    use serde_json::{Map, Value, json};
    use tempfile::TempDir;

    use crate::contexts::sync::domain::inventory_export::{
        AuthorityKind, CaptureEvidence, CaptureOutcome, CaptureStatus, ConsistencyStatus,
        EXPECTATIONS_FORMAT_VERSION, EXPORT_FORMAT_VERSION, EdgeCoverage, ExpectedEdge,
        ExpectedRecord, NETBOX_VERSION, ObservationStatus, PayloadDescriptor, ProcessArea,
        SnapshotAttestation, SnapshotExpectations, SourceAuthority, record_value_digest,
    };

    use crate::contexts::sync::application::inventory_preflight::preflight;
    use crate::contexts::sync::domain::MappingConfigDocument;
    use crate::contexts::sync::domain::inventory_preflight::{
        ExpectedSnapshotAuthority, FindingCode, PreflightProfile, PreflightReport, catalog_digest,
        tool_digest,
    };

    const CANARY: &str = "CANARY-private-token-expression";

    struct Capture {
        root: TempDir,
        path: PathBuf,
        manifest: ExportManifest,
        payloads: BTreeMap<String, Vec<u8>>,
    }

    fn payload(
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

    fn record(provider: Uuid, entity: &str, id: &str, fields: Value) -> SourceRecord {
        let Value::Object(fields) = fields else {
            panic!("record fields must be an object");
        };
        SourceRecord::new(
            SourceIdentity::new(provider, SourceEntity::new(entity).expect("entity"), id)
                .expect("identity"),
            fields,
        )
    }

    #[test]
    fn snapshot_census_allows_empty_unknown_but_rejects_populated_unknown() {
        let mut records = BTreeMap::new();
        records.insert("unknown.items".to_owned(), Vec::new());
        let empty = snapshot_expectations(&records).expect("empty unsupported census");
        assert!(empty.records.is_empty());
        assert!(empty.edges.is_empty());
        records.insert(
            "unknown.items".to_owned(),
            vec![record(
                Uuid::from_u128(1),
                "unknown.items",
                "1",
                serde_json::json!({}),
            )],
        );
        assert!(snapshot_expectations(&records).is_err());
    }

    #[test]
    fn runtime_inventory_components_seal_only_canonical_interface_edges() {
        let provider = Uuid::from_u128(1);
        let row = record(
            provider,
            "dcim.inventory-items",
            "1",
            json!({"component": 1, "component_type": "dcim.interface"}),
        );
        let records = BTreeMap::from([("dcim.inventory-items".into(), vec![row.clone()])]);
        let census = snapshot_expectations(&records).expect("canonical component census");
        assert_eq!(census.records.len(), 1);
        assert_eq!(census.edges.len(), 1);
        let edge = &census.edges[0];
        assert_eq!(edge.field, "component");
        assert_eq!(edge.from_entity.as_str(), "dcim.inventory-items");
        assert_eq!(edge.to_entity.as_str(), "dcim.interfaces");
        assert_eq!(edge.to_external_id, "1");
        for token in ["dcim.consoleport", "dcim.device", "dcim.interfacetemplate"] {
            let mut unsupported = row.clone();
            unsupported
                .fields
                .insert("component_type".into(), json!(token));
            let records = BTreeMap::from([("dcim.inventory-items".into(), vec![unsupported])]);
            assert!(snapshot_expectations(&records).is_err());
        }
        let mut spoofed = row;
        spoofed.fields.insert(
            "component".into(),
            json!({"id": 1, "object_type": "dcim.device"}),
        );
        assert!(
            snapshot_expectations(&BTreeMap::from([(
                "dcim.inventory-items".into(),
                vec![spoofed],
            )]))
            .is_err()
        );
    }

    #[test]
    fn cable_termination_census_retains_edges_and_ordered_values() {
        let row = record(
            Uuid::from_u128(1),
            "dcim.cable-terminations",
            "1",
            json!({
                "cable": {"id": 2, "object_type": "dcim.cable"}, "cable_end": "A",
                "connector": null, "positions": [1, 2],
                "termination": {"id": 3, "object_type": "dcim.interface"},
                "termination_type": "dcim.interface",
                "created": "2026-10-28T10:00:00Z", "last_updated": "2026-10-28T11:00:00Z"
            }),
        );
        let mut records = BTreeMap::from([("dcim.cable-terminations".into(), vec![row])]);
        let census = snapshot_expectations(&records).expect("termination census");
        assert_eq!(census.records.len(), 1);
        assert_eq!(census.edges.len(), 2);
        for (field, entity, id) in [
            ("cable", "dcim.cables", "2"),
            ("termination", "dcim.interfaces", "3"),
        ] {
            let edge = census
                .edges
                .iter()
                .find(|edge| edge.field == field)
                .expect("edge");
            assert_eq!(edge.to_entity.as_str(), entity);
            assert_eq!(edge.to_external_id, id);
        }
        records.get_mut("dcim.cable-terminations").unwrap()[0]
            .fields
            .insert("positions".into(), json!([2, 1]));
        let reordered = snapshot_expectations(&records).expect("reordered census");
        assert_ne!(
            census.records[0].value_sha256,
            reordered.records[0].value_sha256
        );
        for field in ["created", "last_updated"] {
            let original = records["dcim.cable-terminations"][0].fields[field].clone();
            records.get_mut("dcim.cable-terminations").unwrap()[0]
                .fields
                .insert(field.into(), json!("2026-10-28T12:00:00Z"));
            let changed = snapshot_expectations(&records).expect("timestamp census");
            assert_ne!(
                reordered.records[0].value_sha256,
                changed.records[0].value_sha256
            );
            records.get_mut("dcim.cable-terminations").unwrap()[0]
                .fields
                .insert(field.into(), original);
        }
    }

    #[test]
    fn cable_termination_refuses_invalid_evidence() {
        let base = json!({"cable": 2, "cable_end": "A", "connector": null,
            "positions": [1, 2], "termination": 3, "termination_type": "dcim.interface"});
        for (field, value, expected) in [
            ("cable", json!(null), "authority_edge_shape"),
            (
                "cable",
                json!({"id": 2, "object_type": "dcim.interface"}),
                "authority_edge_type",
            ),
            ("termination", json!(null), "authority_edge_shape"),
            (
                "termination",
                json!({"id": 3, "object_type": "dcim.cable"}),
                "authority_edge_type",
            ),
            ("termination", json!({"id": 0}), "authority_edge_id"),
            ("termination_type", json!(null), "authority_edge_type"),
            (
                "termination_type",
                json!("dcim.frontport"),
                "authority_edge_type",
            ),
            ("positions", json!([0]), "authority_edge_shape"),
            ("connector", json!(32768), "authority_edge_shape"),
        ] {
            let mut fields = base.clone();
            fields[field] = value;
            let row = record(Uuid::from_u128(1), "dcim.cable-terminations", "1", fields);
            let AppError::Validation(code) =
                record_edges(InventoryEntity::CableTermination, &row, &BTreeMap::new())
                    .err()
                    .expect("invalid evidence")
            else {
                panic!("validation error")
            };
            assert_eq!(code, format!("inventory_export:{expected}"));
        }
        for field in ["cable", "termination", "termination_type"] {
            let mut fields = base.clone();
            fields.as_object_mut().unwrap().remove(field);
            let row = record(Uuid::from_u128(1), "dcim.cable-terminations", "1", fields);
            assert!(
                record_edges(InventoryEntity::CableTermination, &row, &BTreeMap::new()).is_err()
            );
        }
    }

    impl Capture {
        fn new() -> Self {
            // Tests create only private ephemeral synthetic captures, no service
            // fixtures and no HTTP, DB, Redis or other application state.
            let root = tempfile::tempdir().expect("test directory");
            let path = root.path().join("capture");
            let provider = Uuid::from_u128(1);
            let entity = SourceEntity::new("dcim.sites").expect("entity");
            let records: Vec<_> = ["3", "1", "2"]
                .into_iter()
                .map(|id| {
                    SourceRecord::new(
                        SourceIdentity::new(provider, entity.clone(), id).expect("identity"),
                        Map::from_iter([("private".into(), Value::String(CANARY.into()))]),
                    )
                })
                .collect();
            let mut payloads = BTreeMap::new();
            let schema = payload(&mut payloads, "schema.json", b"{}".to_vec());
            let consistency_evidence = payload(&mut payloads, "consistency.json", b"{}".to_vec());
            let discovery_evidence = payload(&mut payloads, "discovery.json", b"{}".to_vec());
            let rows = payload(
                &mut payloads,
                "sites.json",
                serde_json::to_vec(&records).expect("records"),
            );
            let empty = payload(&mut payloads, "racks.json", b"[]".to_vec());
            let now = Utc::now();
            let manifest = ExportManifest {
                format_version: EXPORT_FORMAT_VERSION,
                source_provider_id: provider,
                observed_netbox_version: NETBOX_VERSION.into(),
                exporter_source_sha: "6b53415ba3ebed45b55d08d2c5e15d766f2dd2c1".into(),
                authority: SourceAuthority::live_capture(),
                capture: CaptureEvidence {
                    started_at: now,
                    ended_at: now,
                    outcome: CaptureOutcome::Incomplete,
                    consistency: ConsistencyStatus::Unproven,
                    consistency_evidence,
                    discovery_complete: true,
                    discovery_evidence,
                },
                schema,
                resources: vec![
                    ResourceCensus {
                        resource_key: "dcim.sites".into(),
                        endpoint: "/api/dcim/sites/".into(),
                        entity: Some(entity),
                        status: CaptureStatus::Captured,
                        declared_rows: Some(3),
                        observed_rows: 3,
                        payload: Some(rows),
                    },
                    ResourceCensus {
                        resource_key: "dcim.racks".into(),
                        endpoint: "/api/dcim/racks/".into(),
                        entity: Some(SourceEntity::new("dcim.racks").expect("entity")),
                        status: CaptureStatus::Captured,
                        declared_rows: Some(0),
                        observed_rows: 0,
                        payload: Some(empty),
                    },
                    ResourceCensus {
                        resource_key: "plugin/nested/custom".into(),
                        endpoint: "/api/plugins/nested/custom/".into(),
                        entity: None,
                        status: CaptureStatus::Unrepresentable,
                        declared_rows: None,
                        observed_rows: 0,
                        payload: None,
                    },
                ],
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
            Self {
                root,
                path,
                manifest,
                payloads,
            }
        }

        fn seal(&self) -> String {
            seal_bundle(&self.path, &self.manifest, &self.payloads).expect("seal synthetic capture")
        }

        fn open(
            &self,
            expected: &str,
            page_size: usize,
        ) -> Result<ImmutableInventorySource, AppError> {
            ImmutableInventorySource::open(
                &self.path,
                expected,
                self.manifest.source_provider_id,
                page_size,
            )
        }

        /// Intentionally bypass the writer to exercise hostile boundary bytes.
        fn hostile_bundle(&self) -> String {
            std::fs::create_dir(&self.path).expect("directory");
            for (name, bytes) in &self.payloads {
                std::fs::write(self.path.join(name), bytes).expect("payload");
            }
            let bytes = serde_json::to_vec(&self.manifest).expect("manifest");
            std::fs::write(self.path.join(MANIFEST_FILE), &bytes).expect("manifest file");
            digest(&bytes)
        }

        fn replace_records(&mut self, mutate: impl FnOnce(&mut Vec<SourceRecord>)) {
            self.replace_resource("dcim.sites", mutate);
        }

        /// Replace the captured rows of one resource, keeping its descriptor.
        fn replace_resource(
            &mut self,
            resource_key: &str,
            mutate: impl FnOnce(&mut Vec<SourceRecord>),
        ) {
            let index = self
                .manifest
                .resources
                .iter()
                .position(|resource| resource.resource_key == resource_key)
                .expect("captured resource");
            let path = self.manifest.resources[index]
                .payload
                .as_ref()
                .expect("payload")
                .path
                .clone();
            let mut records: Vec<SourceRecord> =
                serde_json::from_slice(&self.payloads[&path]).expect("rows");
            mutate(&mut records);
            let observed = records.len() as u64;
            let descriptor = payload(
                &mut self.payloads,
                &path,
                serde_json::to_vec(&records).expect("rows"),
            );
            let resource = &mut self.manifest.resources[index];
            resource.payload = Some(descriptor);
            resource.observed_rows = observed;
            resource.declared_rows = Some(observed);
        }

        /// Append one captured collection of the same sealed bundle.
        fn add_resource(
            &mut self,
            entity: &str,
            file: &str,
            endpoint: &str,
            records: Vec<SourceRecord>,
        ) {
            let observed = records.len() as u64;
            let descriptor = payload(
                &mut self.payloads,
                file,
                serde_json::to_vec(&records).expect("rows"),
            );
            self.manifest.resources.push(ResourceCensus {
                resource_key: entity.into(),
                endpoint: endpoint.into(),
                entity: Some(SourceEntity::new(entity).expect("entity")),
                status: CaptureStatus::Captured,
                declared_rows: Some(observed),
                observed_rows: observed,
                payload: Some(descriptor),
            });
        }

        /// Turn the fixture into a complete capture with one reference edge. No
        /// service, database or network state is involved.
        fn complete(&mut self) {
            // A complete capture has no unrepresentable discovery entry.
            self.manifest
                .resources
                .retain(|resource| resource.entity.is_some());
            self.manifest.capture.outcome = CaptureOutcome::Complete;
            self.manifest.capture.consistency = ConsistencyStatus::Consistent;
            self.manifest.capture.discovery_complete = true;
            for index in 0..self.manifest.processes.len() {
                let evidence = payload(
                    &mut self.payloads,
                    &format!("process-{index}.json"),
                    b"{}".to_vec(),
                );
                self.manifest.processes[index].status = ObservationStatus::Observed;
                self.manifest.processes[index].evidence = Some(evidence);
            }
            // Site "3" references region 2 as an object, site "1" as a bare
            // scalar, and the custom fields cover object, scalar and list forms.
            self.replace_records(|records| {
                records[0]
                    .fields
                    .insert("region".into(), serde_json::json!({"id": 2}));
                records[1]
                    .fields
                    .insert("region".into(), serde_json::json!(2));
                records[1]
                    .fields
                    .insert("custom_fields".into(), serde_json::json!({"scalar_ref": 2}));
                records[2].fields.insert(
                    "custom_fields".into(),
                    serde_json::json!({
                        "owner_ref": {"id": 1},
                        "peer_refs": [3, "1"],
                    }),
                );
            });
            let provider = self.manifest.source_provider_id;
            self.add_resource(
                "dcim.regions",
                "regions.json",
                "/api/dcim/regions/",
                vec![record(
                    provider,
                    "dcim.regions",
                    "2",
                    serde_json::json!({"name": "north"}),
                )],
            );
            self.add_resource(
                "extras.custom-fields",
                "custom-fields.json",
                "/api/extras/custom-fields/",
                vec![
                    record(
                        provider,
                        "extras.custom-fields",
                        "1",
                        serde_json::json!({
                            "name": "owner_ref",
                            "type": "object",
                            "related_object_type": "dcim.site",
                        }),
                    ),
                    record(
                        provider,
                        "extras.custom-fields",
                        "2",
                        serde_json::json!({
                            "name": "peer_refs",
                            "type": "multiobject",
                            "related_object_type": "dcim.site",
                        }),
                    ),
                    record(
                        provider,
                        "extras.custom-fields",
                        "3",
                        serde_json::json!({
                            "name": "scalar_ref",
                            "type": "object",
                            "related_object_type": "dcim.site",
                        }),
                    ),
                ],
            );
        }

        /// The same complete snapshot, additionally attested as owned synthetic.
        fn synthetic(&mut self) {
            self.complete();
            self.attest();
        }

        /// Exact expectations: record value digests plus the
        /// exact typed edges this fixture is known to contain. They are written
        /// here by hand, not re-derived by the verifier.
        fn expectations(&self) -> SnapshotExpectations {
            let mut records = Vec::new();
            for resource in &self.manifest.resources {
                let (Some(entity), Some(descriptor)) = (&resource.entity, &resource.payload) else {
                    continue;
                };
                let list: Vec<SourceRecord> =
                    serde_json::from_slice(&self.payloads[&descriptor.path]).expect("rows");
                for record in list {
                    records.push(ExpectedRecord {
                        entity: entity.clone(),
                        external_id: record.identity.external_id.clone(),
                        value_sha256: record_value_digest(&record).expect("value digest"),
                    });
                }
            }
            let edges = [
                ("dcim.sites", "3", "region", "dcim.regions", "2"),
                ("dcim.sites", "1", "region", "dcim.regions", "2"),
                (
                    "dcim.sites",
                    "2",
                    "custom_fields.owner_ref",
                    "dcim.sites",
                    "1",
                ),
                (
                    "dcim.sites",
                    "2",
                    "custom_fields.peer_refs[0]",
                    "dcim.sites",
                    "3",
                ),
                (
                    "dcim.sites",
                    "2",
                    "custom_fields.peer_refs[1]",
                    "dcim.sites",
                    "1",
                ),
                (
                    "dcim.sites",
                    "1",
                    "custom_fields.scalar_ref",
                    "dcim.sites",
                    "2",
                ),
            ]
            .into_iter()
            .map(
                |(from_entity, from_id, field, to_entity, to_id)| ExpectedEdge {
                    from_entity: SourceEntity::new(from_entity).expect("entity"),
                    from_external_id: from_id.into(),
                    field: field.into(),
                    to_entity: SourceEntity::new(to_entity).expect("entity"),
                    to_external_id: to_id.into(),
                },
            )
            .collect();
            SnapshotExpectations {
                format_version: EXPECTATIONS_FORMAT_VERSION,
                records,
                edges,
            }
        }

        fn attest(&mut self) {
            let expectations = self.expectations();
            self.replace_expectations(expectations);
        }

        fn replace_expectations(&mut self, expectations: SnapshotExpectations) {
            let descriptor = payload(
                &mut self.payloads,
                "expectations.json",
                serde_json::to_vec(&expectations).expect("expectations"),
            );
            let scope = self.manifest.declared_scope().expect("scope");
            let provenance = payload(&mut self.payloads, "provenance.json", b"{}".to_vec());
            self.manifest.authority =
                SourceAuthority::operator_endorsed_snapshot(SnapshotAttestation {
                    operator_id: Uuid::from_u128(4),
                    provenance,
                    issuer: "pwa-81/synthetic-fixture".into(),
                    issued_at: Utc::now(),
                    scope,
                    edge_coverage: EdgeCoverage::TypedInventoryReferences,
                    expectations: descriptor,
                });
        }
    }

    fn rejected(result: Result<ImmutableInventorySource, AppError>, code: &str) {
        let error = result.err().expect("must reject").to_string();
        assert!(error.contains(code), "safe error code mismatch: {error}");
        assert!(!error.contains(CANARY));
    }

    #[test]
    fn offline_preflight_binds_original_bytes_and_repeats_without_disk_or_service_reads() {
        use crate::contexts::sync::domain::{
            ConflictMode, MappingConfigurationStatus, MappingDirection, MappingRef, MappingTarget,
        };
        let capture = Capture::new();
        let expected = capture.seal();
        let source = capture.open(&expected, 2).expect("verified source");
        let manifest_bytes = source.manifest_bytes().to_vec();
        let schema_bytes = source
            .verified_metadata("schema.json")
            .expect("verified schema")
            .to_vec();
        let config = MappingConfigDocument {
            reference: MappingRef::new(Uuid::from_u128(7), 1),
            name: CANARY.into(),
            description: None,
            source_provider_id: capture.manifest.source_provider_id,
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
        let mapping_bytes = serde_json::to_vec(&config).expect("mapping");
        let profile = PreflightProfile {
            format_version: 1,
            bundle_digest: expected.clone(),
            source_provider_id: config.source_provider_id,
            mapping_ref: config.reference,
            mapping_digest: digest(&mapping_bytes),
            catalog_digest: catalog_digest(),
            tool_digest: tool_digest(),
            // The fixture is a live incomplete capture: no authority is claimed.
            snapshot_authority: None,
        };
        let profile_bytes = serde_json::to_vec(&profile).expect("profile");
        let profile_digest = digest(&profile_bytes);
        // Replace every artifact byte after verification. No DB/Redis config,
        // pool, network client or service state is supplied to preflight.
        for descriptor in capture.manifest.payloads() {
            std::fs::write(capture.path.join(&descriptor.path), CANARY).expect("mutate disk");
        }
        std::fs::write(capture.path.join(MANIFEST_FILE), CANARY).expect("mutate seal");
        let first = preflight(&source, &mapping_bytes, &profile_bytes, &profile_digest)
            .expect("offline assessment");
        assert!(!first.can_plan_target());
        assert!(!first.report.source_coverage_complete);
        assert!(
            first
                .report
                .findings
                .iter()
                .filter(|f| f.code == FindingCode::MappingMissing)
                .count()
                >= 3
        );
        assert!(
            first
                .report
                .findings
                .iter()
                .any(|f| f.code == FindingCode::UnsupportedEmptyResourceUnused)
        );
        let encoded = serde_json::to_vec(&first.report).expect("safe report");
        assert!(!String::from_utf8_lossy(&encoded).contains(CANARY));
        let repeated = preflight(&source, &mapping_bytes, &profile_bytes, &profile_digest)
            .expect("offline repeat");
        assert_eq!(
            encoded,
            serde_json::to_vec(&repeated.report).expect("repeat report")
        );
        assert_eq!(source.bundle_digest(), expected);
        assert_eq!(source.manifest_bytes(), manifest_bytes);
        assert_eq!(
            source.verified_metadata("schema.json"),
            Some(schema_bytes.as_slice())
        );
        let mut alternate_mapping_bytes = mapping_bytes.clone();
        alternate_mapping_bytes.push(b' ');
        let changed = preflight(
            &source,
            &alternate_mapping_bytes,
            &profile_bytes,
            &profile_digest,
        )
        .expect("digest assessment");
        assert!(
            changed
                .report
                .findings
                .iter()
                .any(|f| f.code == FindingCode::MappingDigestMismatch)
        );
        let changed = preflight(&source, &mapping_bytes, &profile_bytes, &"0".repeat(64))
            .expect("profile assessment");
        assert!(
            changed
                .report
                .findings
                .iter()
                .any(|f| f.code == FindingCode::ProfileDigestMismatch)
        );
        let wrong_source = PreflightProfile {
            source_provider_id: Uuid::from_u128(99),
            ..profile
        };
        let wrong_bytes = serde_json::to_vec(&wrong_source).expect("wrong profile");
        let changed = preflight(&source, &mapping_bytes, &wrong_bytes, &digest(&wrong_bytes))
            .expect("source identity assessment");
        assert!(
            changed
                .report
                .findings
                .iter()
                .any(|f| f.code == FindingCode::BundleIdentityMismatch)
        );
    }

    #[tokio::test]
    async fn deterministic_replay_empty_resources_and_independent_coverage() {
        let capture = Capture::new();
        let expected = capture.seal();
        let source = capture.open(&expected, 2).expect("verified source");
        let sites = SourceEntity::new("dcim.sites").expect("sites");
        let racks = SourceEntity::new("dcim.racks").expect("racks");
        assert_eq!(
            source.supported_entities(),
            vec![sites.clone(), racks.clone()]
        );
        assert_eq!(source.census().len(), 3);
        assert!(source.census()[2].status == CaptureStatus::Unrepresentable);
        assert!(
            source
                .process_coverage()
                .iter()
                .all(|p| p.status == ObservationStatus::Unobservable)
        );
        assert!(source.manifest().capture.outcome == CaptureOutcome::Incomplete);
        let first = source.fetch_page(&sites, None).await.expect("first");
        assert_eq!(
            first
                .records
                .iter()
                .map(|r| r.identity.external_id.as_str())
                .collect::<Vec<_>>(),
            vec!["1", "2"]
        );
        assert_eq!(first.next_cursor.as_deref(), Some("2"));
        assert_eq!(
            source.fetch_page(&sites, None).await.expect("repeat"),
            first
        );
        // Prove replay is independent of all subsequent on-disk changes.
        std::fs::write(capture.path.join("sites.json"), b"tampered").expect("tamper");
        let last = source
            .fetch_page(&sites, first.next_cursor.as_deref())
            .await
            .expect("last");
        assert_eq!(last.records[0].identity.external_id, "3");
        assert!(last.next_cursor.is_none());
        let empty = source.fetch_page(&racks, None).await.expect("empty");
        assert!(empty.records.is_empty() && empty.next_cursor.is_none());
    }

    #[test]
    fn private_no_clobber_writer_and_pinned_identity() {
        let capture = Capture::new();
        let expected = capture.seal();
        assert_eq!(
            std::fs::metadata(&capture.path)
                .expect("mode")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        for name in capture
            .payloads
            .keys()
            .map(String::as_str)
            .chain([MANIFEST_FILE])
        {
            assert_eq!(
                std::fs::metadata(capture.path.join(name))
                    .expect("mode")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        assert!(seal_bundle(&capture.path, &capture.manifest, &capture.payloads).is_err());
        assert_eq!(
            digest(&std::fs::read(capture.path.join(MANIFEST_FILE)).expect("seal")),
            expected
        );
        rejected(capture.open(&"0".repeat(64), 1), "bundle_digest");
        rejected(capture.open(CANARY, 1), "expected_digest");
        rejected(
            ImmutableInventorySource::open(&capture.path, &expected, Uuid::from_u128(2), 1),
            "provider_identity",
        );
        rejected(capture.open(&expected, 0), "page_size");
    }

    #[test]
    fn tampering_missing_and_wrong_lengths_fail_before_exposure() {
        for operation in 0..4 {
            let mut capture = Capture::new();
            if operation == 3 {
                capture.manifest.resources[0]
                    .payload
                    .as_mut()
                    .expect("payload")
                    .byte_length += 1;
            }
            let expected = capture.hostile_bundle();
            match operation {
                0 => {
                    std::fs::write(
                        capture.path.join("sites.json"),
                        vec![b' '; capture.payloads["sites.json"].len()],
                    )
                    .expect("tamper");
                }
                1 => {
                    std::fs::remove_file(capture.path.join("schema.json")).expect("remove");
                }
                2 => {
                    std::fs::write(capture.path.join("discovery.json"), b"longer")
                        .expect("tamper evidence");
                }
                _ => {}
            }
            rejected(
                capture.open(&expected, 1),
                match operation {
                    1 => "file_open",
                    2 => "file_length",
                    _ => "payload_integrity",
                },
            );
        }
    }

    #[test]
    fn duplicate_mixed_and_invalid_source_identities_rejected_with_valid_hashes() {
        for operation in 0..5 {
            let mut capture = Capture::new();
            capture.replace_records(|records| match operation {
                0 => records[1] = records[0].clone(),
                1 => records[0].identity.source_provider_id = Uuid::from_u128(2),
                2 => records[0].identity.entity = SourceEntity::new("dcim.racks").expect("entity"),
                3 => records[0].identity.external_id = String::new(),
                _ => {
                    // Deserialize bypasses SourceEntity::new; the boundary must
                    // revalidate existing aggregate serde implementations.
                    records[0].identity.entity =
                        serde_json::from_value(Value::String(CANARY.into()))
                            .expect("unchecked entity");
                }
            });
            let expected = capture.hostile_bundle();
            rejected(capture.open(&expected, 1), "record_identity");
            assert!(
                seal_bundle(
                    &capture.root.path().join("refused"),
                    &capture.manifest,
                    &capture.payloads
                )
                .is_err()
            );
        }
    }

    #[test]
    fn valid_hashes_do_not_authorize_noncanonical_netbox_ids() {
        for external_id in [
            "00000000-0000-0000-0000-000000000001",
            "/api/dcim/sites/1/",
            CANARY,
            "0",
            "-1",
            "18446744073709551616",
            "01",
        ] {
            let mut capture = Capture::new();
            // The fixture also retains canonical "1", making "01" a distinct
            // stored identity for the same NetBox object if not rejected.
            capture.replace_records(|records| {
                records[0].identity.external_id = external_id.into();
            });
            let expected = capture.hostile_bundle();
            rejected(capture.open(&expected, 1), "record_identity");
        }
    }

    #[tokio::test]
    async fn canonical_positive_netbox_ids_replay_without_rewriting() {
        let mut capture = Capture::new();
        capture.replace_records(|records| {
            records[0].identity.external_id = u64::MAX.to_string();
            records[2].identity.external_id = "42".into();
        });
        let expected = capture.seal();
        let source = capture.open(&expected, 3).expect("canonical source");
        let sites = SourceEntity::new("dcim.sites").expect("entity");
        let page = source
            .fetch_page(&sites, None)
            .await
            .expect("canonical page");
        assert_eq!(
            page.records
                .iter()
                .map(|r| r.identity.external_id.as_str())
                .collect::<Vec<_>>(),
            vec!["1", "18446744073709551615", "42"],
        );
        assert!(page.next_cursor.is_none());
    }

    #[test]
    fn invalid_manifest_versions_counts_and_descriptors_rejected() {
        for operation in 0..13 {
            let mut capture = Capture::new();
            match operation {
                0 => capture.manifest.observed_netbox_version = "4.7.0".into(),
                1 => capture.manifest.format_version += 1,
                2 => capture.manifest.resources[0].observed_rows = 2,
                3 => {
                    capture.manifest.resources[0].observed_rows = 2;
                    capture.manifest.resources[0].declared_rows = Some(2);
                }
                4 => capture.manifest.resources[0].payload = None,
                5 => {
                    capture.manifest.resources[1].payload =
                        capture.manifest.resources[0].payload.clone()
                }
                6 => {
                    capture.manifest.processes.pop();
                }
                7 => capture.manifest.processes[1].area = ProcessArea::Plugins,
                8 => {
                    capture.manifest.capture.ended_at =
                        capture.manifest.capture.started_at - chrono::Duration::seconds(1)
                }
                9 => capture.manifest.capture.outcome = CaptureOutcome::Complete,
                10 => capture
                    .manifest
                    .resources
                    .push(capture.manifest.resources[0].clone()),
                11 => capture.manifest.exporter_source_sha = CANARY.into(),
                _ => capture.manifest.schema.sha256 = CANARY.into(),
            }
            let expected = capture.hostile_bundle();
            rejected(
                capture.open(&expected, 1),
                match operation {
                    0 | 1 | 11 => "identity",
                    2 | 4 => "resource_count",
                    3 => "observed_count",
                    5 | 10 | 12 => "payload_descriptor",
                    6 | 7 => "process_coverage",
                    8 => "capture_interval",
                    _ => "false_complete",
                },
            );
        }
    }

    #[test]
    fn malformed_or_absent_seal_never_exposes_records_or_secret_errors() {
        for bytes in [
            format!("{{\"private\":\"{CANARY}\"}}").into_bytes(),
            b"{".to_vec(),
        ] {
            let capture = Capture::new();
            std::fs::create_dir(&capture.path).expect("directory");
            std::fs::write(capture.path.join(MANIFEST_FILE), &bytes).expect("manifest");
            rejected(capture.open(&digest(&bytes), 1), "manifest_encoding");
        }
        let capture = Capture::new();
        let expected = capture.seal();
        std::fs::rename(
            capture.path.join(MANIFEST_FILE),
            capture.path.join(".manifest.pending"),
        )
        .expect("unseal");
        rejected(capture.open(&expected, 1), "file_open");

        let mut capture = Capture::new();
        let descriptor = payload(
            &mut capture.payloads,
            "sites.json",
            format!("[{{\"private\":\"{CANARY}\"}}]").into_bytes(),
        );
        capture.manifest.resources[0].payload = Some(descriptor);
        let expected = capture.hostile_bundle();
        rejected(capture.open(&expected, 1), "record_encoding");
    }

    #[test]
    fn traversal_absolute_paths_symlinks_and_collisions_refused() {
        for path in [
            "../outside.json",
            "/outside.json",
            "a/b.json",
            "a\\b.json",
            ".",
            "..",
            MANIFEST_FILE,
        ] {
            let mut capture = Capture::new();
            capture.manifest.schema.path = path.into();
            let expected = capture.hostile_bundle();
            rejected(capture.open(&expected, 1), "payload_descriptor");
        }
        let capture = Capture::new();
        let expected = capture.seal();
        let outside = capture.root.path().join("outside");
        std::fs::write(&outside, &capture.payloads["schema.json"]).expect("outside");
        std::fs::remove_file(capture.path.join("schema.json")).expect("remove");
        symlink(&outside, capture.path.join("schema.json")).expect("symlink");
        rejected(capture.open(&expected, 1), "file_open");
        let alias = capture.root.path().join("alias");
        symlink(&capture.path, &alias).expect("directory symlink");
        rejected(
            ImmutableInventorySource::open(
                &alias,
                &expected,
                capture.manifest.source_provider_id,
                1,
            ),
            "directory_open",
        );
        assert!(seal_bundle(&alias, &capture.manifest, &capture.payloads).is_err());
        let manifest_link = Capture::new();
        let expected = manifest_link.seal();
        std::fs::rename(
            manifest_link.path.join(MANIFEST_FILE),
            manifest_link.root.path().join("seal"),
        )
        .expect("move");
        symlink(
            manifest_link.root.path().join("seal"),
            manifest_link.path.join(MANIFEST_FILE),
        )
        .expect("seal link");
        rejected(manifest_link.open(&expected, 1), "file_open");
    }

    #[tokio::test]
    async fn invalid_cursors_are_safe_and_never_skip_or_loop() {
        let capture = Capture::new();
        let expected = capture.seal();
        let source = capture.open(&expected, 2).expect("source");
        let sites = SourceEntity::new("dcim.sites").expect("entity");
        for cursor in [
            CANARY,
            "0",
            "00",
            "02",
            "1",
            "3",
            "4",
            "-1",
            "99999999999999999999999999999",
        ] {
            let error = source
                .fetch_page(&sites, Some(cursor))
                .await
                .err()
                .expect("invalid cursor")
                .to_string();
            assert!(error.contains("inventory_export:cursor"));
            assert!(!error.contains(CANARY));
        }
        let absent = SourceEntity::new("dcim.devices").expect("entity");
        assert!(source.fetch_page(&absent, None).await.is_err());
    }

    /// The snapshot attestation is the only positive authority path, so every
    /// independent expectation is re-derived from the captured records.
    #[test]
    fn unendorsed_snapshot_has_no_tag_relationship_evidence() {
        let capture = Capture::new();
        let expected = capture.seal();
        let source = capture.open(&expected, 1).expect("unendorsed source");
        assert!(source.verified_tag_relationships().is_none());
    }

    #[test]
    fn operator_endorsed_snapshot_snapshot_verifies_independent_values_and_edges() {
        let mut capture = Capture::new();
        capture.synthetic();
        let expectations = capture.expectations();
        assert_eq!(expectations.records.len(), 7);
        assert_eq!(expectations.edges.len(), 6);
        assert_eq!(expectations.edges[0].from_external_id, "3");
        assert_eq!(expectations.edges[0].field, "region");
        assert_eq!(expectations.edges[0].to_entity.as_str(), "dcim.regions");
        assert_eq!(expectations.edges[0].to_external_id, "2");
        assert!(
            expectations.edges.iter().any(
                |edge| edge.field == "custom_fields.peer_refs[1]" && edge.to_external_id == "1"
            )
        );
        assert!(expectations.edges.iter().any(|edge| {
            edge.field == "custom_fields.scalar_ref"
                && edge.to_entity.as_str() == "dcim.sites"
                && edge.to_external_id == "2"
        }));
        assert!(
            expectations
                .edges
                .iter()
                .any(|edge| edge.field == "custom_fields.owner_ref" && edge.to_external_id == "1")
        );
        let expected = capture.seal();
        let source = capture.open(&expected, 2).expect("attested source");
        // This independently endorsed fixture is not a canonical ORM transport.
        assert!(source.verified_tag_relationships().is_none());
        let evidence = source
            .tag_evidence
            .as_ref()
            .expect("verified authority census");
        let mut snapshot = source.snapshot();
        assert!(evidence.matches(&snapshot, capture.manifest.source_provider_id));
        snapshot.records.pop();
        assert!(!evidence.matches(&snapshot, capture.manifest.source_provider_id));
        assert!(!evidence.matches(&source.snapshot(), Uuid::nil()));
        assert_eq!(
            source.manifest().authority.kind,
            AuthorityKind::OperatorEndorsedSnapshot
        );
        assert_eq!(
            source
                .manifest()
                .authority
                .snapshot
                .as_ref()
                .map(|a| a.issuer.as_str()),
            Some("pwa-81/synthetic-fixture")
        );
        assert_eq!(
            source.manifest().declared_scope().expect("scope"),
            capture.manifest.declared_scope().expect("scope")
        );
        // The verified expectation document survives as private evidence.
        assert!(
            source
                .verified_metadata("expectations.json")
                .is_some_and(|bytes| bytes == capture.payloads["expectations.json"].as_slice())
        );
        // The writer refuses the same mismatch before publishing any seal.
        let mut stale = Capture::new();
        stale.synthetic();
        stale.replace_records(|records| {
            records[1]
                .fields
                .insert("name".into(), Value::String(CANARY.into()));
        });
        let error = seal_bundle(
            &stale.root.path().join("stale"),
            &stale.manifest,
            &stale.payloads,
        )
        .err()
        .expect("stale expectations must not seal")
        .to_string();
        assert!(error.contains("authority_expectations") && !error.contains(CANARY));
    }

    #[test]
    fn synthetic_attestation_rejects_changed_undeclared_or_misattributed_contents() {
        for operation in 0..9 {
            let mut capture = Capture::new();
            capture.synthetic();
            let mut expectations = capture.expectations();
            let expected = match operation {
                0 => {
                    // A captured value changes after the expectations were pinned.
                    capture.replace_records(|records| {
                        records[1]
                            .fields
                            .insert("name".into(), Value::String(CANARY.into()));
                    });
                    "authority_expectations"
                }
                1 => {
                    expectations.records.pop();
                    capture.replace_expectations(expectations);
                    "authority_expectations"
                }
                2 => {
                    expectations.records.push(ExpectedRecord {
                        entity: SourceEntity::new("dcim.sites").expect("entity"),
                        external_id: "1".into(),
                        value_sha256: "0".repeat(64),
                    });
                    capture.replace_expectations(expectations);
                    "authority_expectations"
                }
                3 => {
                    expectations.edges[0].to_external_id = "1".into();
                    capture.replace_expectations(expectations);
                    "authority_edge_mismatch"
                }
                4 => {
                    expectations.edges[0].field = "site".into();
                    capture.replace_expectations(expectations);
                    "authority_edge_mismatch"
                }
                5 => {
                    // An undeclared reference edge cannot be smuggled in.
                    expectations.edges.clear();
                    capture.replace_expectations(expectations);
                    "authority_edge_mismatch"
                }
                6 => {
                    expectations.format_version = EXPECTATIONS_FORMAT_VERSION + 1;
                    capture.replace_expectations(expectations);
                    "authority_expectations"
                }
                7 => {
                    let scope = capture
                        .manifest
                        .authority
                        .snapshot
                        .as_mut()
                        .expect("attestation");
                    scope.scope.resources.pop();
                    "authority_scope"
                }
                _ => {
                    let path = capture
                        .manifest
                        .authority
                        .snapshot
                        .as_ref()
                        .expect("attestation")
                        .expectations
                        .path
                        .clone();
                    capture.payloads.remove(&path);
                    "file_open"
                }
            };
            let bundle = capture.hostile_bundle();
            rejected(capture.open(&bundle, 1), expected);
        }
    }

    /// The artifact identity must cover the resource payloads that the metadata
    /// map deliberately drops after verification.
    #[test]
    fn artifact_identity_covers_every_verified_payload_byte() {
        use crate::contexts::sync::domain::inventory_transfer::artifact_digest;

        let mut capture = Capture::new();
        capture.synthetic();
        let expected = capture.seal();
        let manifest_bytes = std::fs::read(capture.path.join(MANIFEST_FILE)).expect("seal");
        let verified: Vec<(&str, &[u8])> = capture
            .payloads
            .iter()
            .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
            .collect();
        let independent = artifact_digest(&manifest_bytes, &verified).expect("artifact identity");

        let source = capture.open(&expected, 2).expect("verified source");
        assert_eq!(source.artifact_digest(), independent.as_str());
        // Record payloads are not metadata, yet their bytes are covered.
        assert!(source.verified_metadata("sites.json").is_none());
        assert!(source.verified_metadata("expectations.json").is_some());

        // Flipping one resource byte changes the identity and cannot verify.
        let mut tampered = capture.payloads.clone();
        let mut bytes = tampered["sites.json"].clone();
        bytes[0] = b' ';
        tampered.insert("sites.json".into(), bytes);
        let changed: Vec<(&str, &[u8])> = tampered
            .iter()
            .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
            .collect();
        assert_ne!(
            artifact_digest(&manifest_bytes, &changed).expect("artifact identity"),
            independent
        );
        std::fs::write(capture.path.join("sites.json"), &tampered["sites.json"]).expect("tamper");
        rejected(capture.open(&expected, 2), "payload_integrity");
    }

    /// Existence of an equal id in another entity never satisfies a typed edge.
    #[test]
    fn equal_external_id_does_not_authorize_a_wrong_entity_expectation() {
        let mut capture = Capture::new();
        capture.synthetic();
        let mut expectations = capture.expectations();
        assert_eq!(expectations.edges[0].field, "region");
        assert_eq!(expectations.edges[0].to_external_id, "2");
        // `dcim.sites`/2 exists, so an existence-only check would accept this.
        expectations.edges[0].to_entity = SourceEntity::new("dcim.sites").expect("entity");
        capture.replace_expectations(expectations);
        let bundle = capture.hostile_bundle();
        rejected(capture.open(&bundle, 1), "authority_edge_type");
    }

    #[test]
    fn tenant_group_parent_and_tenant_group_are_typed_edges() {
        let provider = Uuid::from_u128(1);
        for (entity, source, field) in [
            (
                InventoryEntity::TenantGroup,
                "tenancy.tenant-groups",
                "parent",
            ),
            (InventoryEntity::Tenant, "tenancy.tenants", "group"),
        ] {
            let row = record(provider, source, "1", json!({(field): {"id": 7}}));
            let edges = record_edges(entity, &row, &BTreeMap::new()).expect("typed edge");
            assert_eq!(edges.len(), 1);
            assert_eq!(edges[0].0, field);
            assert_eq!(edges[0].1.implied, Some(InventoryEntity::TenantGroup));
            assert_eq!(edges[0].1.target, "7");
        }
    }

    #[test]
    fn component_template_parent_edges_are_source_typed() {
        let provider = Uuid::from_u128(1);
        for (entity, source, field, target) in [
            (
                InventoryEntity::PowerOutletTemplate,
                "dcim.power-outlet-templates",
                "power_port",
                InventoryEntity::PowerPortTemplate,
            ),
            (
                InventoryEntity::InterfaceTemplate,
                "dcim.interface-templates",
                "bridge",
                InventoryEntity::InterfaceTemplate,
            ),
            (
                InventoryEntity::InventoryItemTemplate,
                "dcim.inventory-item-templates",
                "parent",
                InventoryEntity::InventoryItemTemplate,
            ),
        ] {
            let row = record(provider, source, "1", json!({(field): {"id": 7}}));
            let edges = record_edges(entity, &row, &BTreeMap::new()).expect("typed edge");
            assert_eq!(edges.len(), 1);
            assert_eq!(edges[0].0, field);
            assert_eq!(edges[0].1.implied, Some(target));
            assert_eq!(edges[0].1.target, "7");
        }
    }

    #[test]
    fn positioned_mapping_census_preserves_all_four_typed_edges() {
        let row = record(
            Uuid::from_u128(1),
            "dcim.port-template-mappings",
            "1",
            json!({"device_type": 2, "module_type": 3, "front_port": 4,
                "rear_port": 5, "front_port_position": 6, "rear_port_position": 7}),
        );
        let edges = record_edges(InventoryEntity::PortTemplateMapping, &row, &BTreeMap::new())
            .expect("mapping edges");
        for (field, target, id) in [
            ("device_type", InventoryEntity::DeviceType, "2"),
            ("module_type", InventoryEntity::ModuleType, "3"),
            ("front_port", InventoryEntity::FrontPortTemplate, "4"),
            ("rear_port", InventoryEntity::RearPortTemplate, "5"),
        ] {
            let (_, edge) = edges.iter().find(|(name, _)| name == field).expect("edge");
            assert_eq!(edge.implied, Some(target));
            assert_eq!(edge.target, id);
        }
        assert_eq!(edges.len(), 4);
    }

    #[test]
    fn vlan_contact_census_keeps_native_reference_types() {
        let provider = Uuid::from_u128(1);
        for (entity, fields, expected) in [
            (
                InventoryEntity::Prefix,
                json!({"vlan": 7, "role": null}),
                vec![("vlan", InventoryEntity::Vlan)],
            ),
            (
                InventoryEntity::Contact,
                json!({"groups": [7]}),
                vec![("groups[0]", InventoryEntity::ContactGroup)],
            ),
            (
                InventoryEntity::ContactGroup,
                json!({"parent": 7}),
                vec![("parent", InventoryEntity::ContactGroup)],
            ),
            (
                InventoryEntity::ContactAssignment,
                json!({"contact": 7, "role": 7, "object": 7, "object_type": "dcim.site"}),
                vec![
                    ("contact", InventoryEntity::Contact),
                    ("role", InventoryEntity::ContactRole),
                    ("object", InventoryEntity::Site),
                ],
            ),
        ] {
            let row = record(provider, entity.source_key(), "1", fields);
            let edges = record_edges(entity, &row, &BTreeMap::new()).expect("typed graph");
            assert_eq!(edges.len(), expected.len());
            for (field, target) in expected {
                let (_, edge) = edges.iter().find(|(name, _)| name == field).expect("edge");
                assert_eq!(edge.implied, Some(target));
                assert_eq!(edge.target, "7");
            }
        }
        for (entity, fields) in [
            (
                InventoryEntity::Prefix,
                json!({"vlan": {"id": 7, "object_type": "tenancy.contact"}}),
            ),
            (
                InventoryEntity::Contact,
                json!({"groups": [{"id": 7, "object_type": "tenancy.contactrole"}]}),
            ),
            (
                InventoryEntity::ContactAssignment,
                json!({"object": 7, "object_type": "unknown.model"}),
            ),
            (
                InventoryEntity::ContactAssignment,
                json!({"object": {"id": 7, "object_type": "ipam.vlan"}, "object_type": "dcim.site"}),
            ),
            // Admitting VLANs does not admit the unrelated IPAM Role model.
            (InventoryEntity::Prefix, json!({"role": 7})),
        ] {
            let row = record(provider, entity.source_key(), "1", fields);
            assert!(record_edges(entity, &row, &BTreeMap::new()).is_err());
        }
    }

    /// A bare scalar at a known reference field is still a typed edge.
    #[test]
    fn bare_scalar_field_reference_must_be_declared() {
        let mut capture = Capture::new();
        capture.synthetic();
        let mut expectations = capture.expectations();
        expectations
            .edges
            .retain(|edge| !(edge.from_external_id == "1" && edge.field == "region"));
        capture.replace_expectations(expectations);
        let bundle = capture.hostile_bundle();
        rejected(capture.open(&bundle, 1), "authority_edge_mismatch");
    }

    #[test]
    fn populated_untyped_references_cannot_borrow_a_declared_target_entity() {
        let provider = Uuid::from_u128(1);
        let definitions = BTreeMap::new();
        for (entity, source, fields) in [
            (
                InventoryEntity::DeviceRole,
                "dcim.device-roles",
                json!({"config_template": {"id": 7}}),
            ),
            (
                InventoryEntity::Interface,
                "dcim.interfaces",
                json!({"tagged_vlans": [{"id": 7}]}),
            ),
        ] {
            let row = record(provider, source, "1", fields);
            let error = record_edges(entity, &row, &definitions)
                .err()
                .expect("unsupported reference type must block");
            let AppError::Validation(code) = error else {
                panic!("expected sanitized authority rejection");
            };
            assert_eq!(code, "inventory_export:authority_edge_type");
        }
        let omitted = record(
            provider,
            "dcim.device-roles",
            "1",
            json!({"config_template": null}),
        );
        assert!(
            record_edges(InventoryEntity::DeviceRole, &omitted, &definitions)
                .expect("null carries no unsupported reference")
                .is_empty()
        );
    }

    #[test]
    fn populated_custom_field_without_type_evidence_is_blocked() {
        let mut capture = Capture::new();
        capture.synthetic();
        capture.replace_resource("extras.custom-fields", |records| {
            records[0]
                .fields
                .insert("related_object_type".into(), Value::Null);
        });
        capture.attest();
        let bundle = capture.hostile_bundle();
        rejected(capture.open(&bundle, 1), "authority_custom_field");
    }

    #[test]
    fn populated_custom_field_without_definition_is_blocked() {
        let mut capture = Capture::new();
        capture.synthetic();
        capture.replace_resource("extras.custom-fields", |records| {
            records.retain(|record| {
                record.fields.get("name").and_then(Value::as_str) != Some("owner_ref")
            });
        });
        capture.attest();
        let bundle = capture.hostile_bundle();
        rejected(capture.open(&bundle, 1), "authority_custom_field");
    }

    fn mapping_document(provider: Uuid) -> (MappingConfigDocument, Vec<u8>) {
        use crate::contexts::sync::domain::{
            ConflictMode, MappingConfigurationStatus, MappingDirection, MappingRef, MappingTarget,
        };
        let config = MappingConfigDocument {
            reference: MappingRef::new(Uuid::from_u128(7), 1),
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
        let bytes = serde_json::to_vec(&config).expect("mapping");
        (config, bytes)
    }

    fn profile_document(
        bundle_digest: &str,
        config: &MappingConfigDocument,
        mapping: &[u8],
        snapshot_authority: Option<ExpectedSnapshotAuthority>,
    ) -> Vec<u8> {
        serde_json::to_vec(&PreflightProfile {
            format_version: 1,
            bundle_digest: bundle_digest.into(),
            source_provider_id: config.source_provider_id,
            mapping_ref: config.reference,
            mapping_digest: digest(mapping),
            catalog_digest: catalog_digest(),
            tool_digest: tool_digest(),
            snapshot_authority,
        })
        .expect("profile")
    }

    fn assess(
        capture: &Capture,
        bundle_digest: &str,
        pinned: Option<ExpectedSnapshotAuthority>,
    ) -> PreflightReport {
        let source = capture.open(bundle_digest, 2).expect("verified source");
        let (config, mapping) = mapping_document(capture.manifest.source_provider_id);
        let profile = profile_document(bundle_digest, &config, &mapping, pinned);
        preflight(&source, &mapping, &profile, &digest(&profile))
            .expect("offline assessment")
            .report
    }

    #[test]
    fn pinned_snapshot_authority_is_required_and_capture_status_is_not_authority() {
        let mut capture = Capture::new();
        capture.synthetic();
        let bundle_digest = capture.seal();
        let attestation = capture
            .manifest
            .authority
            .snapshot
            .as_ref()
            .expect("attestation")
            .clone();
        let scope_digest = capture.manifest.declared_scope().expect("scope").digest();
        let matched = ExpectedSnapshotAuthority {
            operator_id: attestation.operator_id,
            provenance_sha256: attestation.provenance.sha256.clone(),
            issuer: attestation.issuer.clone(),
            scope_digest: scope_digest.clone(),
            expectations_digest: attestation.expectations.sha256.clone(),
        };

        let report = assess(&capture, &bundle_digest, Some(matched.clone()));
        assert!(report.authority.attested);
        assert_eq!(
            report.authority.kind,
            AuthorityKind::OperatorEndorsedSnapshot
        );
        let identity = report.authority.attestation.clone().expect("identity");
        assert_eq!(identity.scope_digest, scope_digest);
        assert_eq!(identity.issuer, "pwa-81/synthetic-fixture");
        assert_eq!(
            identity.edge_coverage,
            EdgeCoverage::TypedInventoryReferences
        );
        assert_eq!(
            identity.expectations_sha256,
            attestation.expectations.sha256
        );
        // The identity is derived from the verified attestation, not supplied.
        assert_eq!(
            capture
                .manifest
                .authority
                .attestation_identity()
                .expect("derived identity"),
            Some(identity.clone())
        );
        assert!(report.source_coverage_complete, "{:?}", report.findings);
        assert!(!report.findings.iter().any(|finding| matches!(
            finding.code,
            FindingCode::SourceAuthorityUnattested | FindingCode::SourceAuthorityMismatch
        )));

        // No independently supplied attestation: the same bundle is unattested
        // and exposes no pinnable identity at all.
        let report = assess(&capture, &bundle_digest, None);
        assert!(!report.authority.attested);
        assert!(report.authority.attestation.is_none());
        assert!(!report.source_coverage_complete);
        assert!(
            report
                .findings
                .iter()
                .any(|finding| finding.code == FindingCode::SourceAuthorityUnattested)
        );

        // A pinned expectation that the bundle does not satisfy is a mismatch.
        for operation in 0..5 {
            let mut expected = matched.clone();
            match operation {
                0 => expected.issuer = "someone-else/fixture".into(),
                1 => expected.scope_digest = "0".repeat(64),
                2 => expected.expectations_digest = "0".repeat(64),
                3 => expected.operator_id = Uuid::from_u128(5),
                _ => expected.provenance_sha256 = "0".repeat(64),
            }
            let report = assess(&capture, &bundle_digest, Some(expected));
            assert!(!report.authority.attested);
            assert!(report.authority.attestation.is_none());
            assert!(
                report
                    .findings
                    .iter()
                    .any(|finding| finding.code == FindingCode::SourceAuthorityMismatch),
                "operation {operation}: {:?}",
                report.findings
            );
        }

        // A complete live capture keeps every capture fact and stays blocked:
        // status describes a read, it does not attest authority.
        let mut live = Capture::new();
        live.complete();
        assert_eq!(live.manifest.authority.kind, AuthorityKind::LiveCapture);
        assert_eq!(live.manifest.capture.outcome, CaptureOutcome::Complete);
        let live_digest = live.seal();
        let report = assess(&live, &live_digest, None);
        assert!(!report.source_coverage_complete);
        assert!(
            report
                .findings
                .iter()
                .any(|finding| finding.code == FindingCode::SourceAuthorityUnattested)
        );
        assert!(
            !report
                .findings
                .iter()
                .any(|finding| finding.code == FindingCode::SourceCoverageIncomplete)
        );
    }
}
