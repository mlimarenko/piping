//! SQL local inventory target (PIP-86).
//!
//! [`LocalInventoryCatalog`] implements the
//! [`LocalInventoryTarget`](crate::contexts::sync::domain::inventory_sync::LocalInventoryTarget)
//! port: [`prepare`](LocalInventoryTarget::prepare) turns a full source snapshot
//! into a typed batch validated against the closed
//! [`inventory_target_schema`](crate::contexts::sync::domain::inventory_target_schema)
//! catalogue, and [`commit_pull`](LocalInventoryTarget::commit_pull) writes the
//! whole graph plus its binding rows and the job's terminal state in one
//! transaction.
//!
//! Design rules enforced here:
//!
//! * Every physical table/column comes from the closed catalogue; a caller can
//!   never name one.
//! * A source record is bound by its provider-scoped identity (UUIDv5 over the
//!   provider namespace and the `(source_entity, external_id, target_entity)`
//!   tuple); an existing binding always wins and a row is never adopted by a
//!   mutable natural key.
//! * A populated source field that is neither approved nor explicitly excluded
//!   blocks the pull instead of being dropped.
//! * The graph is resolved and validated before any write; a late failure
//!   rolls the whole transaction back, so no partial binding or half-written
//!   row can survive.
//! * No raw provider body, credential or unmapped value is ever logged or
//!   stored in `sync_job.stats`.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::Value;
use sqlx::{PgPool, Postgres, QueryBuilder, Row, Transaction};
use uuid::Uuid;

use crate::contexts::custom_fields::domain::{
    CustomFieldDefinition, CustomFieldType, FilterLogic, UiEditable, UiVisible,
    validate_definition, validate_value,
};
use crate::contexts::sync::domain::inventory_sync::{
    CommitPull, InventoryCommitStats, InventoryValue, LocalInventoryCatalogView,
    LocalInventoryResource, LocalInventoryResourceField, LocalInventoryTarget,
    MappingConfigDocument, SourceRecord, SourceSnapshot, TargetSyncRequest, TypedInventoryBatch,
    TypedInventoryRow,
};
use crate::contexts::sync::domain::inventory_target_schema::{
    Absent, FieldKind, InventoryEntity, InventoryField, PgType,
};
use crate::contexts::sync::domain::sync_job::{SyncJob, SyncStatus};

mod counters;
mod tag_memberships;
use crate::contexts::sync::domain::{
    EntityMapping, FieldMapping, SourceEntity, TransformDirection, evaluate_expression,
};
use crate::errors::AppError;

/// A provider-scoped reference to another record, captured during `prepare` and
/// resolved to a local id only inside the commit transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceBinding {
    /// Local target entity, or `None` for an out-of-scope relation.
    pub target: Option<InventoryEntity>,
    /// Canonical `app_label.model` token for a generic reference.
    pub type_token: Option<String>,
    /// The provider-side id used to look the binding up.
    pub external_id: String,
}

/// One canonical service port mapping parsed from a source
/// `<protocol>/<port>` entry. No range expansion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortMapping {
    pub protocol: String,
    pub port: i32,
}

fn parse_port_mapping(raw: &str) -> Result<PortMapping, AppError> {
    let (protocol, port) = raw.split_once('/').ok_or_else(|| {
        AppError::Validation("a port mapping is not '<protocol>/<port>'".to_string())
    })?;
    if protocol.is_empty() || protocol.len() > 50 {
        return Err(AppError::Validation(
            "a port mapping has an invalid protocol".to_string(),
        ));
    }
    let port: i32 = port
        .parse()
        .map_err(|_| AppError::Validation("a port mapping has a non-numeric port".to_string()))?;
    if !(0..=65535).contains(&port) {
        return Err(AppError::Validation(
            "a port mapping port is out of range".to_string(),
        ));
    }
    Ok(PortMapping {
        protocol: protocol.to_string(),
        port,
    })
}

/// One prepared row: identity, typed scalars, and the relations to resolve at
/// commit time.
#[derive(Debug, Clone)]
pub struct PreparedRow {
    entity: InventoryEntity,
    source_entity: String,
    external_id: String,
    local_id: Uuid,
    scalars: BTreeMap<InventoryField, InventoryValue>,
    references: Vec<(InventoryField, ReferenceBinding)>,
    /// Relations whose source value was explicitly null: their columns must be
    /// set to SQL NULL (GFK type + id together), never left stale.
    cleared: Vec<InventoryField>,
    junctions: Vec<(InventoryField, Vec<ReferenceBinding>)>,
    port_mappings: Option<(InventoryField, Vec<PortMapping>)>,
    custom_fields: Option<serde_json::Map<String, Value>>,
    /// Canonical, already-validated definition for a `CustomField` row, so the
    /// commit preflight can seed the incoming-definition cache without the raw
    /// source record.
    prepared_definition: Option<CustomFieldDefinition>,
    tags: Option<Vec<String>>,
}

impl PreparedRow {
    /// Canonical local id derived for this source record.
    #[must_use]
    pub fn local_id(&self) -> Uuid {
        self.local_id
    }

    /// The binding target key for this row.
    #[must_use]
    pub fn target_key(&self) -> String {
        self.entity.target_key()
    }

    /// Provider-side identity string (never a field value).
    #[must_use]
    pub fn external_id(&self) -> &str {
        &self.external_id
    }
}

impl TypedInventoryRow for PreparedRow {
    fn entity(&self) -> InventoryEntity {
        self.entity
    }

    fn value(&self, field: InventoryField) -> Option<&InventoryValue> {
        self.scalars.get(&field)
    }
}

/// A prepared batch of typed rows.
#[derive(Debug, Clone, Default)]
pub struct LocalInventoryBatch {
    source_provider_id: Uuid,
    rows: Vec<PreparedRow>,
    extracted_count: u64,
    tag_membership_count: u64,
    entities_extracted: BTreeMap<String, u64>,
}

impl LocalInventoryBatch {
    /// The provider this batch belongs to.
    #[must_use]
    pub fn source_provider_id(&self) -> Uuid {
        self.source_provider_id
    }
}

impl TypedInventoryBatch for LocalInventoryBatch {
    type Row = PreparedRow;

    fn rows(&self) -> &[Self::Row] {
        &self.rows
    }

    fn extracted_count(&self) -> u64 {
        self.extracted_count
    }
}

/// The concrete SQL local inventory target.
#[derive(Clone)]
pub struct LocalInventoryCatalog {
    pool: PgPool,
}

impl LocalInventoryCatalog {
    /// Build the target over a connection pool.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

/// Map one closed entity to the read-only resource view the routes serve.
fn resource_of(entity: InventoryEntity) -> Option<LocalInventoryResource> {
    let source_entity = SourceEntity::new(entity.source_key()).ok()?;
    Some(LocalInventoryResource {
        source_entity,
        target_entity: entity.target_key(),
        fields: entity
            .field_iter()
            .map(|field| LocalInventoryResourceField {
                name: field.name().to_string(),
                field_type: format!("{:?}", field.sql_type()).to_lowercase(),
                required: !field.nullable(),
            })
            .collect(),
    })
}

impl LocalInventoryCatalogView for LocalInventoryCatalog {
    fn resources(&self) -> Vec<LocalInventoryResource> {
        InventoryEntity::all()
            .chain(std::iter::once(InventoryEntity::TaggedObject))
            .filter_map(resource_of)
            .collect()
    }

    fn resource(&self, source_entity: &SourceEntity) -> Option<LocalInventoryResource> {
        InventoryEntity::from_source_key(source_entity.as_str()).and_then(resource_of)
    }
}

/// Derive the per-provider UUIDv5 namespace.
#[must_use]
fn provider_namespace(source_provider_id: Uuid) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("piping.inventory.sync/provider/{source_provider_id}").as_bytes(),
    )
}

/// Derive the deterministic local id for one source record. The tuple is joined
/// with U+001F (unit separator), which cannot appear in a canonical source key
/// or a provider id, so the encoding is unambiguous.
#[must_use]
fn derive_local_id(
    source_provider_id: Uuid,
    source_entity: &str,
    external_id: &str,
    target_key: &str,
) -> Uuid {
    let namespace = provider_namespace(source_provider_id);
    let mut seed =
        String::with_capacity(source_entity.len() + external_id.len() + target_key.len() + 2);
    seed.push_str(source_entity);
    seed.push('\u{1f}');
    seed.push_str(external_id);
    seed.push('\u{1f}');
    seed.push_str(target_key);
    Uuid::new_v5(&namespace, seed.as_bytes())
}

fn external_id_of(value: &Value) -> Option<String> {
    match value {
        Value::Object(map) => map.get("id").and_then(scalar_to_string),
        Value::Number(_) | Value::String(_) | Value::Bool(_) => scalar_to_string(value),
        _ => None,
    }
}

fn scalar_to_string(value: &Value) -> Option<String> {
    match value {
        Value::Number(number) => Some(number.to_string()),
        Value::String(text) => Some(text.clone()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

fn tag_slugs(value: &Value) -> Result<Vec<String>, AppError> {
    match value {
        Value::Null => Ok(Vec::new()),
        Value::Array(items) => items
            .iter()
            .map(|item| match item {
                Value::Object(map) => map
                    .get("slug")
                    .and_then(Value::as_str)
                    .or_else(|| map.get("name").and_then(Value::as_str))
                    .map(str::to_string)
                    .ok_or_else(|| {
                        AppError::Validation("a tag has neither slug nor name".to_string())
                    }),
                Value::String(text) => Ok(text.clone()),
                _ => Err(AppError::Validation(
                    "a tag entry is not an object".to_string(),
                )),
            })
            .collect(),
        _ => Err(AppError::Validation("tags is not a list".to_string())),
    }
}

/// Normalise a NetBox choice object to its `.value`; a non-object is returned
/// unchanged.
fn normalize_choice(value: &Value) -> Result<Value, AppError> {
    if let Value::Object(map) = value {
        match map.get("value") {
            Some(inner) => Ok(inner.clone()),
            None => Err(AppError::Validation(
                "choice object has no value".to_string(),
            )),
        }
    } else {
        Ok(value.clone())
    }
}

fn wrong_shape(spec: InventoryField) -> AppError {
    AppError::Validation(format!(
        "field {}::{} has a value of the wrong shape",
        spec.entity().source_key(),
        spec.name()
    ))
}

fn convert_scalar(spec: InventoryField, value: &Value) -> Result<InventoryValue, AppError> {
    let metadata = spec.spec();
    let value = if metadata.choice_normalized {
        normalize_choice(value)?
    } else {
        value.clone()
    };
    let converted = match metadata.sql_type {
        PgType::Text | PgType::Inet | PgType::Cidr | PgType::MacAddr => InventoryValue::Text {
            value: value.as_str().ok_or_else(|| wrong_shape(spec))?.to_string(),
        },
        PgType::Bool => InventoryValue::Boolean {
            value: value.as_bool().ok_or_else(|| wrong_shape(spec))?,
        },
        PgType::Integer => {
            let number = value.as_i64().ok_or_else(|| wrong_shape(spec))?;
            let narrowed = i32::try_from(number).map_err(|_| {
                AppError::Validation(format!(
                    "field {}::{} exceeds the integer range",
                    spec.entity().source_key(),
                    spec.name()
                ))
            })?;
            InventoryValue::Integer {
                value: i64::from(narrowed),
            }
        }
        PgType::BigInt | PgType::SmallInt => InventoryValue::Integer {
            value: value.as_i64().ok_or_else(|| wrong_shape(spec))?,
        },
        PgType::Numeric | PgType::Double | PgType::Real => InventoryValue::Number {
            value: value.as_f64().ok_or_else(|| wrong_shape(spec))?,
        },
        PgType::Uuid => {
            let raw = value.as_str().ok_or_else(|| wrong_shape(spec))?;
            let parsed = Uuid::parse_str(raw).map_err(|_| wrong_shape(spec))?;
            InventoryValue::Text {
                value: parsed.to_string(),
            }
        }
        PgType::Jsonb
        | PgType::Json
        | PgType::TextArray
        | PgType::IntArray
        | PgType::UuidArray
        | PgType::BigIntArray => InventoryValue::Json { value },
        PgType::Date | PgType::Time | PgType::Timestamp | PgType::TimestampTz => {
            return Err(AppError::Validation(format!(
                "field {}::{} has no supported temporal conversion",
                spec.entity().source_key(),
                spec.name()
            )));
        }
    };
    Ok(converted)
}

fn record_value<'a>(record: &'a SourceRecord, name: &str) -> Option<&'a Value> {
    record.fields.get(name)
}

impl LocalInventoryTarget for LocalInventoryCatalog {
    type Batch = LocalInventoryBatch;

    fn prepare(
        &self,
        config: &MappingConfigDocument,
        snapshot: &SourceSnapshot,
    ) -> Result<Self::Batch, AppError> {
        TargetSyncRequest {
            reference: config.reference,
            source_provider_id: config.source_provider_id,
            target: config.target.clone(),
            direction: config.direction,
            conflict_mode: config.conflict_mode,
        }
        .ensure_supported()?;

        // The immutable document is the whitelist: an entity is pulled only
        // through its enabled entity mapping, and only target fields named by
        // one of that mapping's field mappings are ever written.
        let mappings = enabled_entity_mappings(config);
        let fields_by_mapping = field_mappings_by_entity(config);

        let mut rows = Vec::new();
        let mut seen = HashSet::new();
        for record in &snapshot.records {
            let entity = check_record_identity(record, config.source_provider_id, &mut seen)?;
            check_supported_fields(entity, record)?;
            if entity.is_membership() {
                // Tag membership is native on the owning rows; the resource has
                // no row of its own.
                continue;
            }
            let source_entity = record.identity.entity.as_str();
            let mapping = mappings.get(source_entity).ok_or_else(|| {
                AppError::Validation(format!(
                    "no enabled mapping for source entity {source_entity}"
                ))
            })?;
            let fields = fields_by_mapping.get(&mapping.id).ok_or_else(|| {
                AppError::Validation(format!(
                    "no field mappings for source entity {source_entity}"
                ))
            })?;
            check_full_mapping(entity, record, fields)?;
            rows.push(prepare_row(
                entity,
                config.source_provider_id,
                record,
                fields,
            )?);
        }
        tag_memberships::validate(snapshot, &rows)?;
        Ok(LocalInventoryBatch {
            source_provider_id: config.source_provider_id,
            rows,
            extracted_count: snapshot.records.len() as u64,
            tag_membership_count: count_tag_memberships(snapshot),
            entities_extracted: count_entities_extracted(snapshot),
        })
    }

    async fn commit_pull(&self, commit: CommitPull<Self::Batch>) -> Result<SyncJob, AppError> {
        let CommitPull {
            mut job,
            batch,
            authorisation,
        } = commit;
        let mut transaction = self.pool.begin().await.map_err(|error| {
            AppError::Database(format!("Failed to open sync transaction: {error}"))
        })?;

        // Serialise concurrent pulls of the same provider for the whole
        // transaction.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(batch.source_provider_id.to_string())
            .execute(&mut *transaction)
            .await
            .map_err(|error| {
                AppError::Database(format!("Failed to take the provider lock: {error}"))
            })?;

        authorisation.ensure_owned().await?;
        let existing = load_bindings(&mut transaction, batch.source_provider_id).await?;
        let local_ids = merge_local_ids(&batch, existing);
        reject_tag_slug_conflicts(&mut transaction, &batch, &local_ids).await?;
        let mut content_types: HashMap<String, Uuid> = HashMap::new();
        let mut stats = InventoryCommitStats {
            extracted: batch.extracted_count(),
            ..InventoryCommitStats::default()
        };

        // Preflight the custom-field definitions carried by this batch: a value
        // referencing a definition imported in the same transaction validates
        // against the cached canonical definition, because the definition row
        // is not written until after planning.
        let incoming_definitions =
            prepare_custom_field_definitions(&batch, &mut content_types, &mut transaction).await?;

        // Resolve the whole graph and validate it before the first write.
        let planned = plan_rows(
            &batch,
            &local_ids,
            &incoming_definitions,
            &mut content_types,
            &mut transaction,
        )
        .await?;

        commit_planned_rows(
            &mut transaction,
            batch.source_provider_id,
            job.id,
            &planned,
            &mut stats,
        )
        .await?;

        // Rebuild the nested-set bookkeeping of every touched MPTT table from
        // the whole parent graph, so hierarchy filters stay valid.
        rebuild_touched_trees(&mut transaction, &planned).await?;

        recompute_counts(&mut transaction, &planned).await?;

        job.complete();
        stats.tag_memberships_committed = batch.tag_membership_count;
        stats.entities_extracted = batch.entities_extracted;
        stats.entities_committed.insert(
            InventoryEntity::TaggedObject.source_key().into(),
            batch.tag_membership_count,
        );
        job.stats = serde_json::to_value(&stats)
            .map_err(|error| AppError::Internal(format!("Failed to encode sync stats: {error}")))?;
        let updated = update_job(&mut transaction, &job).await?;

        authorisation.ensure_owned().await?;
        transaction.commit().await.map_err(|error| {
            AppError::Database(format!("Failed to commit the sync transaction: {error}"))
        })?;
        Ok(updated)
    }
}

/// Collect the enabled entity mappings, keyed by their source entity key. A
/// disabled mapping is a whitelist miss, so an entity is pulled only through
/// its enabled mapping.
fn enabled_entity_mappings(config: &MappingConfigDocument) -> HashMap<&str, &EntityMapping> {
    let mut mappings: HashMap<&str, &EntityMapping> = HashMap::new();
    for mapping in &config.entity_mappings {
        if mapping.enabled {
            mappings.insert(mapping.source_entity.as_str(), mapping);
        }
    }
    mappings
}

/// Group the immutable document's field mappings by the entity mapping they
/// belong to.
fn field_mappings_by_entity(config: &MappingConfigDocument) -> HashMap<Uuid, Vec<&FieldMapping>> {
    let mut fields_by_mapping: HashMap<Uuid, Vec<&FieldMapping>> = HashMap::new();
    for field in &config.field_mappings {
        fields_by_mapping
            .entry(field.entity_mapping_id)
            .or_default()
            .push(field);
    }
    fields_by_mapping
}

/// Validate one snapshot record's identity and return its approved entity.
///
/// A duplicate `(provider, entity, external_id)` identity, a record belonging
/// to a different provider, or an unsupported source entity blocks the pull.
fn check_record_identity(
    record: &SourceRecord,
    source_provider_id: Uuid,
    seen: &mut HashSet<(Uuid, SourceEntity, String)>,
) -> Result<InventoryEntity, AppError> {
    if !seen.insert((
        record.identity.source_provider_id,
        record.identity.entity.clone(),
        record.identity.external_id.clone(),
    )) {
        return Err(AppError::Validation(format!(
            "Source snapshot contains duplicate object identities for {}",
            record.identity.entity.as_str(),
        )));
    }
    if record.identity.source_provider_id != source_provider_id {
        return Err(AppError::Validation(
            "snapshot record belongs to a different source provider".to_string(),
        ));
    }
    InventoryEntity::from_source_key(record.identity.entity.as_str()).ok_or_else(|| {
        AppError::Validation(format!(
            "unsupported source entity {}",
            record.identity.entity.as_str()
        ))
    })
}

/// Whether a source value counts as populated for the closed-catalogue check.
fn source_value_populated(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(properties) => !properties.is_empty(),
        Value::Bool(_) | Value::Number(_) => true,
    }
}

/// Reject a populated source field the closed catalogue neither approves nor
/// explicitly excludes, so an unmapped value can never be dropped silently.
fn check_supported_fields(entity: InventoryEntity, record: &SourceRecord) -> Result<(), AppError> {
    for (name, value) in &record.fields {
        if source_value_populated(value)
            && entity.field(name).is_none()
            && entity.excluded_source_field(name).is_none()
        {
            return Err(AppError::Validation(format!(
                "Unsupported populated source field {}::{name}",
                entity.source_key()
            )));
        }
    }
    Ok(())
}

/// Require every populated catalogue field to be named by an enabled field
/// mapping, so an import can never omit part of an entity's approved surface.
fn check_full_mapping(
    entity: InventoryEntity,
    record: &SourceRecord,
    fields: &[&FieldMapping],
) -> Result<(), AppError> {
    for field in entity.field_iter() {
        if record.fields.contains_key(field.name())
            && !fields
                .iter()
                .any(|mapped| mapped.target_field == field.name())
        {
            return Err(AppError::Validation(format!(
                "Full inventory mapping omits {}::{}",
                entity.source_key(),
                field.name()
            )));
        }
    }
    Ok(())
}

/// Count the extracted rows per source entity for `sync_job.stats`.
fn count_entities_extracted(snapshot: &SourceSnapshot) -> BTreeMap<String, u64> {
    let mut entities_extracted = BTreeMap::new();
    for record in &snapshot.records {
        *entities_extracted
            .entry(record.identity.entity.as_str().to_string())
            .or_insert(0) += 1;
    }
    entities_extracted
}

/// Count the `extras.tagged-objects` memberships carried by this snapshot.
fn count_tag_memberships(snapshot: &SourceSnapshot) -> u64 {
    snapshot
        .records
        .iter()
        .filter(|record| {
            record.identity.entity.as_str() == InventoryEntity::TaggedObject.source_key()
        })
        .count() as u64
}

/// Merge the batch's deterministic ids with the already-recorded bindings; a
/// recorded binding always wins so a re-run never re-keys an existing row.
fn merge_local_ids(
    batch: &LocalInventoryBatch,
    existing: HashMap<(String, String), Uuid>,
) -> HashMap<(String, String), Uuid> {
    let mut local_ids: HashMap<(String, String), Uuid> = HashMap::new();
    for row in &batch.rows {
        local_ids.insert(
            (row.source_entity.clone(), row.external_id.clone()),
            row.local_id,
        );
    }
    for (key, value) in existing {
        local_ids.insert(key, value);
    }
    local_ids
}

/// Reject a source tag whose slug already identifies a different local tag.
///
/// Tag slugs are the native membership keys; a local or another provider's
/// global definition is never adopted merely because its slug matches.
async fn reject_tag_slug_conflicts(
    transaction: &mut Transaction<'_, Postgres>,
    batch: &LocalInventoryBatch,
    local_ids: &HashMap<(String, String), Uuid>,
) -> Result<(), AppError> {
    for row in batch
        .rows
        .iter()
        .filter(|row| row.entity == InventoryEntity::Tag)
    {
        let slug_field = row
            .entity
            .field("slug")
            .ok_or_else(|| AppError::Internal("Tag slug catalog missing".into()))?;
        let Some(InventoryValue::Text { value: slug }) = row.scalars.get(&slug_field) else {
            return Err(AppError::Validation(
                "A source tag must declare its slug".into(),
            ));
        };
        let local_id = local_ids
            .get(&(row.source_entity.clone(), row.external_id.clone()))
            .copied()
            .ok_or_else(|| AppError::Internal("Tag identity missing".into()))?;
        let colliding: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM inventory_tag WHERE slug=$1 AND id<>$2)",
        )
        .bind(slug)
        .bind(local_id)
        .fetch_one(&mut **transaction)
        .await?;
        if colliding {
            return Err(AppError::Validation("Source tag conflicts with an existing global tag definition; name-based adoption is not supported".into()));
        }
    }
    Ok(())
}

/// Resolve every prepared row against the local graph before the first write.
async fn plan_rows(
    batch: &LocalInventoryBatch,
    local_ids: &HashMap<(String, String), Uuid>,
    incoming_definitions: &HashMap<(String, Uuid), CustomFieldDefinition>,
    content_types: &mut HashMap<String, Uuid>,
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<Vec<PlannedRow>, AppError> {
    let mut planned: Vec<PlannedRow> = Vec::with_capacity(batch.rows.len());
    for row in &batch.rows {
        planned.push(
            plan_row(
                row,
                local_ids,
                incoming_definitions,
                content_types,
                transaction,
            )
            .await?,
        );
    }
    Ok(planned)
}

/// Write every planned row, its junctions and its binding, accumulating commit
/// statistics.
async fn commit_planned_rows(
    transaction: &mut Transaction<'_, Postgres>,
    source_provider_id: Uuid,
    job_id: Uuid,
    planned: &[PlannedRow],
    stats: &mut InventoryCommitStats,
) -> Result<(), AppError> {
    for plan in planned {
        upsert_binding(transaction, source_provider_id, plan, job_id).await?;
    }
    for plan in planned {
        upsert_row(transaction, plan).await?;
        replace_junctions(transaction, plan).await?;
        stats.committed += 1;
        *stats
            .entities_committed
            .entry(plan.source_entity.clone())
            .or_insert(0) += 1;
        for (name, _) in &plan.relations {
            *stats.foreign_keys.entry(name.clone()).or_insert(0) += 1;
        }
    }
    Ok(())
}

/// Rebuild the nested-set bookkeeping of every touched MPTT table from the
/// whole parent graph, so hierarchy filters stay valid.
async fn rebuild_touched_trees(
    transaction: &mut Transaction<'_, Postgres>,
    planned: &[PlannedRow],
) -> Result<(), AppError> {
    let mut tree_entities: Vec<InventoryEntity> = planned
        .iter()
        .map(|plan| plan.entity)
        .filter(|entity| is_tree_entity(*entity))
        .collect();
    tree_entities.sort_unstable();
    tree_entities.dedup();
    for entity in tree_entities {
        recompute_tree(transaction, entity).await?;
    }
    Ok(())
}

/// Recompute the maintained counters for the entities that own one.
async fn recompute_counts(
    transaction: &mut Transaction<'_, Postgres>,
    planned: &[PlannedRow],
) -> Result<(), AppError> {
    let ids_for = |entity| {
        planned
            .iter()
            .filter(|plan| plan.entity == entity)
            .map(|plan| plan.local_id)
            .collect::<Vec<_>>()
    };
    counters::recompute(
        transaction,
        &ids_for(InventoryEntity::Device),
        &ids_for(InventoryEntity::VirtualMachine),
        &ids_for(InventoryEntity::Prefix),
    )
    .await?;
    Ok(())
}

/// Whether the source property feeding a field mapping is present in the
/// record. A declared `literal null` is always an explicit null; otherwise the
/// referenced source field (or the mapping's `source_field`) decides.
fn property_present(
    record: &SourceRecord,
    field_mapping: &FieldMapping,
    expression: &Value,
) -> bool {
    if is_literal_null(expression) {
        return true;
    }
    if let Some(field) = referenced_field(expression) {
        return record.fields.contains_key(field);
    }
    record.fields.contains_key(&field_mapping.source_field)
}

fn is_literal_null(expression: &Value) -> bool {
    expression.get("op").and_then(Value::as_str) == Some("literal")
        && expression
            .get("args")
            .and_then(Value::as_array)
            .and_then(|args| args.first())
            .is_none_or(Value::is_null)
}

fn referenced_field(expression: &Value) -> Option<&str> {
    expression
        .get("args")
        .and_then(Value::as_array)
        .and_then(|args| args.first())
        .and_then(|arg| arg.get("field"))
        .and_then(Value::as_str)
}

/// Explicit null uses the native empty-object encoding; absent fields are
/// filtered by the caller before evaluating this mapped value.
fn mapped_custom_fields(
    source_entity: &str,
    value: &Value,
) -> Result<serde_json::Map<String, Value>, AppError> {
    if value.is_null() {
        return Ok(serde_json::Map::new());
    }
    let Some(map) = value.as_object() else {
        return Err(AppError::Validation(format!(
            "relation {source_entity}::custom_fields is not an object"
        )));
    };
    Ok(map.clone())
}

/// Fallback capture of `custom_fields` from the raw record when no mapping
/// produced an object. Missing preserves values; explicit null clears them.
fn custom_fields_from_record(record: &SourceRecord) -> Option<serde_json::Map<String, Value>> {
    match record.fields.get("custom_fields") {
        Some(Value::Object(map)) => Some(map.clone()),
        Some(Value::Null) => Some(serde_json::Map::new()),
        _ => None,
    }
}

/// Mutable accumulator for the per-field decisions `prepare_row` makes before
/// the row is handed to the commit transaction.
#[derive(Default)]
struct PreparedColumns {
    scalars: BTreeMap<InventoryField, InventoryValue>,
    references: Vec<(InventoryField, ReferenceBinding)>,
    cleared: Vec<InventoryField>,
    junctions: Vec<(InventoryField, Vec<ReferenceBinding>)>,
    port_mappings: Option<(InventoryField, Vec<PortMapping>)>,
    custom_fields: Option<serde_json::Map<String, Value>>,
    tags: Option<Vec<String>>,
}

/// Immutable per-field inputs shared by the binding helpers: the source record,
/// its field mapping, the owning source key, the field name and the mapped value.
struct FieldContext<'a> {
    record: &'a SourceRecord,
    field_mapping: &'a FieldMapping,
    source_entity: &'a str,
    name: &'a str,
    value: &'a Value,
}

/// Capture a mapped `custom_fields` object when the mapping produced a value or
/// the source property is present; otherwise leave the accumulator untouched so
/// the raw-record fallback applies.
fn mapped_custom_fields_value(
    record: &SourceRecord,
    field_mapping: &FieldMapping,
    value: &Value,
    source_entity: &str,
) -> Result<Option<serde_json::Map<String, Value>>, AppError> {
    if value.is_null()
        && !property_present(record, field_mapping, &field_mapping.forward_transform_json)
    {
        return Ok(None);
    }
    Ok(Some(mapped_custom_fields(source_entity, value)?))
}

/// Apply the explicit-null encoding for one field: clear the mapped column or
/// its catalogue-approved empty encoding, never leaving a stale relation. A
/// truly absent property preserves the column unless it is required on insert.
fn apply_explicit_null(
    columns: &mut PreparedColumns,
    field: InventoryField,
    context: &FieldContext<'_>,
) -> Result<(), AppError> {
    let source_entity = context.source_entity;
    let name = context.name;
    if !property_present(
        context.record,
        context.field_mapping,
        &context.field_mapping.forward_transform_json,
    ) {
        // A truly absent property preserves the local column, unless the
        // column is required on insert with no default.
        if field.absent() == Absent::Reject {
            return Err(AppError::Validation(format!(
                "required field {source_entity}::{name} is absent"
            )));
        }
        return Ok(());
    }
    // Explicit null: clear the mapped column (or its catalogue-approved empty
    // encoding); never leave a stale relation or a non-null value.
    match field.kind() {
        FieldKind::Scalar => apply_null_scalar(columns, field, context),
        FieldKind::Tags => {
            columns.tags = Some(Vec::new());
            Ok(())
        }
        FieldKind::Junction { .. } | FieldKind::ContentTypeJunction { .. } => {
            // An explicit null clears the relation to empty; the definition row
            // itself is still validated and written by the generic path.
            columns.junctions.push((field, Vec::new()));
            Ok(())
        }
        FieldKind::ValueJunction { .. } => {
            columns.port_mappings = Some((field, Vec::new()));
            Ok(())
        }
        FieldKind::ForeignKey { .. } | FieldKind::GenericReference { .. } => {
            apply_null_relation(columns, field, context)
        }
    }
}

/// Encode an explicit null for a scalar column using its catalogue `Absent`
/// policy.
fn apply_null_scalar(
    columns: &mut PreparedColumns,
    field: InventoryField,
    context: &FieldContext<'_>,
) -> Result<(), AppError> {
    let source_entity = context.source_entity;
    let name = context.name;
    match field.absent() {
        Absent::EmptyText => {
            columns.scalars.insert(
                field,
                InventoryValue::Text {
                    value: String::new(),
                },
            );
        }
        Absent::EmptyArray => {
            columns.scalars.insert(
                field,
                InventoryValue::Json {
                    value: Value::Array(Vec::new()),
                },
            );
        }
        Absent::Zero => {
            columns
                .scalars
                .insert(field, InventoryValue::Integer { value: 0 });
        }
        Absent::Reject => {
            return Err(AppError::Validation(format!(
                "required field {source_entity}::{name} is null"
            )));
        }
        Absent::Preserve => {
            if field.nullable() {
                columns.scalars.insert(field, InventoryValue::Null);
            } else {
                return Err(AppError::Validation(format!(
                    "required field {source_entity}::{name} cannot be null"
                )));
            }
        }
    }
    Ok(())
}

/// Clear a nullable explicit-null relation; a non-nullable one is rejected.
fn apply_null_relation(
    columns: &mut PreparedColumns,
    field: InventoryField,
    context: &FieldContext<'_>,
) -> Result<(), AppError> {
    if !field.nullable() {
        return Err(AppError::Validation(format!(
            "required relation {}::{} cannot be null",
            context.source_entity, context.name
        )));
    }
    columns.cleared.push(field);
    Ok(())
}

/// Bind one present (non-null) mapped value to its column, reference or junction
/// according to the field's kind.
fn bind_present_value(
    columns: &mut PreparedColumns,
    field: InventoryField,
    context: &FieldContext<'_>,
) -> Result<(), AppError> {
    match field.kind() {
        FieldKind::Scalar | FieldKind::Tags => bind_scalar_or_tags(columns, field, context),
        FieldKind::ForeignKey { target } => bind_foreign_key(columns, field, target, context),
        FieldKind::GenericReference {
            source_type_field,
            allowed,
            ..
        } => bind_generic_reference(columns, field, source_type_field, allowed, context),
        FieldKind::ValueJunction { .. } => {
            columns.port_mappings = Some((field, parse_port_mappings(context)?));
            Ok(())
        }
        FieldKind::ContentTypeJunction { .. } => {
            columns
                .junctions
                .push((field, content_type_references(context)?));
            Ok(())
        }
        FieldKind::Junction { target, .. } => {
            columns
                .junctions
                .push((field, junction_references(target, context)?));
            Ok(())
        }
    }
}

fn bind_scalar_or_tags(
    columns: &mut PreparedColumns,
    field: InventoryField,
    context: &FieldContext<'_>,
) -> Result<(), AppError> {
    if matches!(field.kind(), FieldKind::Tags) || field.sql_type() == PgType::TextArray {
        columns.tags = Some(tag_slugs(context.value)?);
    } else {
        columns
            .scalars
            .insert(field, convert_scalar(field, context.value)?);
    }
    Ok(())
}

fn bind_foreign_key(
    columns: &mut PreparedColumns,
    field: InventoryField,
    target: Option<InventoryEntity>,
    context: &FieldContext<'_>,
) -> Result<(), AppError> {
    let external_id = external_id_of(context.value).ok_or_else(|| {
        AppError::Validation(format!(
            "relation {}::{} has no provider id",
            context.source_entity, context.name
        ))
    })?;
    columns.references.push((
        field,
        ReferenceBinding {
            target,
            type_token: None,
            external_id,
        },
    ));
    Ok(())
}

fn bind_generic_reference(
    columns: &mut PreparedColumns,
    field: InventoryField,
    source_type_field: &str,
    allowed: &[InventoryEntity],
    context: &FieldContext<'_>,
) -> Result<(), AppError> {
    let external_id = external_id_of(context.value).ok_or_else(|| {
        AppError::Validation(format!(
            "generic relation {}::{} has no provider id",
            context.source_entity, context.name
        ))
    })?;
    let token = record_value(context.record, source_type_field)
        .and_then(Value::as_str)
        .ok_or_else(|| {
            AppError::Validation(format!(
                "generic relation {}::{} has no type token",
                context.source_entity, context.name
            ))
        })?;
    let resolved = InventoryEntity::from_content_type_token(token).ok_or_else(|| {
        AppError::Validation(format!(
            "generic relation {}::{} has an unknown type",
            context.source_entity, context.name
        ))
    })?;
    if !allowed.contains(&resolved) {
        return Err(AppError::Validation(format!(
            "generic relation {}::{} points at a disallowed type",
            context.source_entity, context.name
        )));
    }
    columns.references.push((
        field,
        ReferenceBinding {
            target: Some(resolved),
            type_token: Some(token.to_string()),
            external_id,
        },
    ));
    Ok(())
}

/// Parse a `ipam_service_port_mapping` value list into canonical port mappings.
fn parse_port_mappings(context: &FieldContext<'_>) -> Result<Vec<PortMapping>, AppError> {
    let items = context.value.as_array().ok_or_else(|| {
        AppError::Validation(format!(
            "relation {}::{} is not a list",
            context.source_entity, context.name
        ))
    })?;
    let mut parsed = Vec::with_capacity(items.len());
    for item in items {
        let raw = item.as_str().ok_or_else(|| {
            AppError::Validation(format!(
                "relation {}::{} has a non-string entry",
                context.source_entity, context.name
            ))
        })?;
        parsed.push(parse_port_mapping(raw)?);
    }
    Ok(parsed)
}

/// Resolve a content-type junction value list into canonical type-token
/// bindings.
fn content_type_references(context: &FieldContext<'_>) -> Result<Vec<ReferenceBinding>, AppError> {
    let items = context.value.as_array().ok_or_else(|| {
        AppError::Validation(format!(
            "relation {}::{} is not a list",
            context.source_entity, context.name
        ))
    })?;
    let mut refs = Vec::with_capacity(items.len());
    for item in items {
        let token = item.as_str().ok_or_else(|| {
            AppError::Validation(format!(
                "relation {}::{} has a non-token entry",
                context.source_entity, context.name
            ))
        })?;
        refs.push(ReferenceBinding {
            target: None,
            type_token: Some(token.to_string()),
            external_id: token.to_string(),
        });
    }
    Ok(refs)
}

/// Resolve a junction value list into provider-scoped reference bindings.
fn junction_references(
    target: Option<InventoryEntity>,
    context: &FieldContext<'_>,
) -> Result<Vec<ReferenceBinding>, AppError> {
    let items = context.value.as_array().ok_or_else(|| {
        AppError::Validation(format!(
            "relation {}::{} is not a list",
            context.source_entity, context.name
        ))
    })?;
    let mut refs = Vec::with_capacity(items.len());
    for item in items {
        let external_id = external_id_of(item).ok_or_else(|| {
            AppError::Validation(format!(
                "relation {}::{} has an entry without a provider id",
                context.source_entity, context.name
            ))
        })?;
        refs.push(ReferenceBinding {
            target,
            type_token: None,
            external_id,
        });
    }
    Ok(refs)
}

fn prepare_row(
    entity: InventoryEntity,
    source_provider_id: Uuid,
    record: &SourceRecord,
    fields: &[&FieldMapping],
) -> Result<PreparedRow, AppError> {
    let source_entity = entity.source_key().to_string();
    let local_id = derive_local_id(
        source_provider_id,
        &source_entity,
        &record.identity.external_id,
        &entity.target_key(),
    );
    let sample = Value::Object(record.fields.clone());
    let prepared_definition = if entity == InventoryEntity::CustomField {
        Some(validate_source_custom_field(record)?)
    } else {
        None
    };
    let mut columns = PreparedColumns::default();

    for field_mapping in fields {
        let name = field_mapping.target_field.as_str();
        let value = evaluate_expression(
            &field_mapping.forward_transform_json,
            &sample,
            TransformDirection::Forward,
        )
        .map_err(|error| {
            AppError::Validation(format!(
                "forward transform for {source_entity}::{name} failed: {error}"
            ))
        })?;
        if name == "custom_fields" {
            if let Some(mapped) =
                mapped_custom_fields_value(record, field_mapping, &value, &source_entity)?
            {
                columns.custom_fields = Some(mapped);
            }
            continue;
        }
        let field = entity.field(name).ok_or_else(|| {
            AppError::Validation(format!(
                "mapping targets field {source_entity}::{name} which the closed catalogue does not approve"
            ))
        })?;
        let context = FieldContext {
            record,
            field_mapping,
            source_entity: &source_entity,
            name,
            value: &value,
        };
        if value.is_null() {
            apply_explicit_null(&mut columns, field, &context)?;
            continue;
        }
        bind_present_value(&mut columns, field, &context)?;
    }

    if columns.custom_fields.is_none() {
        columns.custom_fields = custom_fields_from_record(record);
    }

    Ok(PreparedRow {
        entity,
        source_entity,
        external_id: record.identity.external_id.clone(),
        local_id,
        scalars: columns.scalars,
        references: columns.references,
        cleared: columns.cleared,
        junctions: columns.junctions,
        port_mappings: columns.port_mappings,
        custom_fields: columns.custom_fields,
        prepared_definition,
        tags: columns.tags,
    })
}

/// A fully resolved row ready to write.
struct PlannedRow {
    entity: InventoryEntity,
    source_entity: String,
    external_id: String,
    local_id: Uuid,
    columns: Vec<PlannedColumn>,
    /// Relation name to resolved count, for `sync_job.stats`.
    relations: Vec<(String, usize)>,
    junctions: Vec<PlannedJunction>,
    value_junctions: Vec<PlannedValueJunction>,
}

struct PlannedValueJunction {
    table: &'static str,
    local_column: &'static str,
    protocol_column: &'static str,
    port_column: &'static str,
    ordinal_column: &'static str,
    rows: Vec<PortMapping>,
}

struct PlannedColumn {
    column: &'static str,
    value: ColumnValue,
    updatable: bool,
}

struct PlannedJunction {
    table: &'static str,
    local_column: &'static str,
    remote_column: &'static str,
    refs: Vec<Uuid>,
}

/// A column value ready to bind.
enum ColumnValue {
    Scalar(PgType, InventoryValue),
    Uuid(Option<Uuid>),
    Tags(Vec<String>),
    Literal(&'static str),
    Copy(String),
}

async fn load_bindings(
    transaction: &mut Transaction<'_, Postgres>,
    source_provider_id: Uuid,
) -> Result<HashMap<(String, String), Uuid>, AppError> {
    let rows = sqlx::query(
        "SELECT source_entity, external_id, local_id FROM sync_inventory_binding \
         WHERE source_provider_id = $1",
    )
    .bind(source_provider_id)
    .fetch_all(&mut **transaction)
    .await
    .map_err(|error| AppError::Database(format!("Failed to load existing bindings: {error}")))?;
    let mut map = HashMap::with_capacity(rows.len());
    for row in rows {
        let source_entity: String = row.try_get("source_entity")?;
        let external_id: String = row.try_get("external_id")?;
        let local_id: Uuid = row.try_get("local_id")?;
        map.insert((source_entity, external_id), local_id);
    }
    Ok(map)
}

async fn content_type_id(
    transaction: &mut Transaction<'_, Postgres>,
    cache: &mut HashMap<String, Uuid>,
    token: &str,
) -> Result<Option<Uuid>, AppError> {
    if let Some(cached) = cache.get(token) {
        return Ok(Some(*cached));
    }
    let (app_label, model) = token
        .split_once('.')
        .ok_or_else(|| AppError::Validation("content type token is malformed".to_string()))?;
    let row =
        sqlx::query("SELECT id FROM inventory_content_type WHERE app_label = $1 AND model = $2")
            .bind(app_label)
            .bind(model)
            .fetch_optional(&mut **transaction)
            .await
            .map_err(|error| {
                AppError::Database(format!("Failed to resolve a content type: {error}"))
            })?;
    match row {
        Some(row) => {
            let id: Uuid = row.try_get("id")?;
            cache.insert(token.to_string(), id);
            Ok(Some(id))
        }
        None => Ok(None),
    }
}

fn resolve_reference(
    binding: &ReferenceBinding,
    local_ids: &HashMap<(String, String), Uuid>,
) -> Option<Uuid> {
    let target = binding.target?;
    local_ids
        .get(&(target.source_key().to_string(), binding.external_id.clone()))
        .copied()
}

/// Validate and merge a record's source custom-field values into the entity's
/// stored `custom_field_data`, scoped to the entity's content type. Returns
/// `None` for an absent field (preserve); an explicit empty map clears values.
async fn resolve_custom_field_data(
    transaction: &mut Transaction<'_, Postgres>,
    incoming_definitions: &HashMap<(String, Uuid), CustomFieldDefinition>,
    content_types: &mut HashMap<String, Uuid>,
    row: &PreparedRow,
    local_id: Uuid,
) -> Result<Option<Value>, AppError> {
    let Some(source_fields) = &row.custom_fields else {
        return Ok(None);
    };
    if source_fields.is_empty() {
        return Ok(Some(Value::Object(serde_json::Map::new())));
    }
    let (app_label, model) = row
        .entity
        .content_type()
        .ok_or_else(|| AppError::Internal("entity has no content type".to_string()))?;
    let token = format!("{app_label}.{model}");
    let content_type_id = content_type_id(transaction, content_types, &token)
        .await?
        .ok_or_else(|| {
            AppError::Validation(format!(
                "entity {} has no seeded content type",
                row.entity.source_key()
            ))
        })?;
    let table = row
        .entity
        .table()
        .ok_or_else(|| AppError::Internal("entity has no table".to_string()))?;

    let select = format!("SELECT custom_field_data FROM {table} WHERE id = $1");
    let existing: Option<Value> = sqlx::query(&select)
        .bind(local_id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(|error| AppError::Database(format!("Failed to read custom fields: {error}")))?
        .map(|result| result.try_get("custom_field_data"))
        .transpose()?;
    let mut merged = existing
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();

    for (key, value) in source_fields {
        // Prefer a definition imported by this same batch (still uncommitted);
        // otherwise fall back to the stored definition.
        let definition =
            if let Some(cached) = incoming_definitions.get(&(key.clone(), content_type_id)) {
                cached.clone()
            } else {
                load_custom_field_definition(transaction, key, content_type_id)
                    .await?
                    .ok_or_else(|| {
                        AppError::Validation(format!(
                            "custom field '{key}' is not defined for {}",
                            row.entity.source_key()
                        ))
                    })?
            };
        if definition.is_secret {
            return Err(AppError::Validation(format!(
                "custom field '{key}' is secret and cannot be imported by this target"
            )));
        }
        validate_value(&definition, value, &[]).map_err(|error| {
            AppError::Validation(format!("custom field '{key}' value is invalid: {error}"))
        })?;
        merged.insert(key.clone(), value.clone());
    }
    Ok(Some(Value::Object(merged)))
}

/// Load the scoped custom-field definition for `(name, content_type_id)` and
/// project it into the canonical [`CustomFieldDefinition`] so the shared
/// [`validate_value`] can be reused instead of a second validator.
/// Build the validated, scoped definition cache for the custom fields imported
/// by this batch, resolving each definition's content-type scope tokens to the
/// native `inventory_content_type` ids (never a remote integer id) and refusing
/// a collision with an existing unbound definition or a secret downgrade.
async fn prepare_custom_field_definitions(
    batch: &LocalInventoryBatch,
    content_types: &mut HashMap<String, Uuid>,
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<HashMap<(String, Uuid), CustomFieldDefinition>, AppError> {
    let mut definitions: HashMap<(String, Uuid), CustomFieldDefinition> = HashMap::new();
    for row in &batch.rows {
        if row.entity != InventoryEntity::CustomField {
            continue;
        }
        let Some(definition) = row.prepared_definition.clone() else {
            continue;
        };
        let mut scopes: Vec<Uuid> = Vec::new();
        for (field, refs) in &row.junctions {
            if field.name() != "object_types" {
                continue;
            }
            for binding in refs {
                let Some(token) = &binding.type_token else {
                    continue;
                };
                let id = content_type_id(transaction, content_types, token)
                    .await?
                    .ok_or_else(|| {
                        AppError::Validation(format!(
                            "custom field '{}' scope '{token}' is not a native content type",
                            definition.name
                        ))
                    })?;
                scopes.push(id);
            }
        }
        check_custom_field_definition_conflicts(transaction, row.local_id, &definition).await?;
        for scope in scopes {
            definitions.insert((definition.name.clone(), scope), definition.clone());
        }
    }
    Ok(definitions)
}

/// A source-imported definition may only touch a row it already owns: a
/// same-name row with a different id is an unbound operator definition and must
/// not be auto-adopted, and a bound secret definition must never be downgraded.
async fn check_custom_field_definition_conflicts(
    transaction: &mut Transaction<'_, Postgres>,
    local_id: Uuid,
    definition: &CustomFieldDefinition,
) -> Result<(), AppError> {
    let row = sqlx::query("SELECT id, is_secret FROM custom_field WHERE name = $1")
        .bind(&definition.name)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(|error| AppError::Database(format!("Failed to load a custom field: {error}")))?;
    let Some(row) = row else {
        return Ok(());
    };
    let existing_id: Uuid = row.try_get("id")?;
    let existing_secret: bool = row.try_get("is_secret")?;
    if existing_id != local_id {
        return Err(AppError::Validation(format!(
            "custom field '{}' conflicts with an existing local definition",
            definition.name
        )));
    }
    if existing_secret && !definition.is_secret {
        return Err(AppError::Validation(format!(
            "custom field '{}' cannot be downgraded from secret",
            definition.name
        )));
    }
    Ok(())
}

async fn load_custom_field_definition(
    transaction: &mut Transaction<'_, Postgres>,
    name: &str,
    content_type_id: Uuid,
) -> Result<Option<CustomFieldDefinition>, AppError> {
    let row = sqlx::query(
        "SELECT cf.id, cf.name, cf.label, cf.field_type, cf.description, cf.group_name, \
         cf.required, cf.enforce_unique, cf.is_secret, cf.default_value, cf.weight, \
         cf.validation_minimum::float8 AS validation_minimum, \
         cf.validation_maximum::float8 AS validation_maximum, cf.validation_regex, \
         cf.validation_schema, cf.choice_set_id, cf.related_object_type_id, \
         cf.related_object_filter, cf.ui_visible, cf.ui_editable, cf.is_cloneable, \
         cf.search_weight, cf.filter_logic, cf.created_at, cf.updated_at \
         FROM custom_field cf \
         JOIN custom_field_object_type cfot ON cfot.custom_field_id = cf.id \
         WHERE cf.name = $1 AND cfot.content_type_id = $2",
    )
    .bind(name)
    .bind(content_type_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|error| AppError::Database(format!("Failed to load a custom field: {error}")))?;
    let Some(row) = row else {
        return Ok(None);
    };
    let field_type_raw: String = row.try_get("field_type")?;
    let field_type = CustomFieldType::parse(&field_type_raw).ok_or_else(|| {
        AppError::Validation(format!("custom field '{name}' has an unknown type"))
    })?;
    let ui_visible_raw: String = row.try_get("ui_visible")?;
    let ui_editable_raw: String = row.try_get("ui_editable")?;
    let filter_logic_raw: String = row.try_get("filter_logic")?;
    let definition = CustomFieldDefinition {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        label: row.try_get("label")?,
        field_type,
        description: row.try_get("description")?,
        group_name: row.try_get("group_name")?,
        required: row.try_get("required")?,
        enforce_unique: row.try_get("enforce_unique")?,
        is_secret: row.try_get("is_secret")?,
        default_value: row.try_get("default_value")?,
        weight: row.try_get("weight")?,
        validation_minimum: row.try_get("validation_minimum")?,
        validation_maximum: row.try_get("validation_maximum")?,
        validation_regex: row.try_get("validation_regex")?,
        validation_schema: row.try_get("validation_schema")?,
        choice_set_id: row.try_get("choice_set_id")?,
        related_object_type_id: row.try_get("related_object_type_id")?,
        related_object_filter: row.try_get("related_object_filter")?,
        ui_visible: UiVisible::parse(&ui_visible_raw).ok_or_else(|| {
            AppError::Database(format!("custom field '{name}' has an invalid ui_visible"))
        })?,
        ui_editable: UiEditable::parse(&ui_editable_raw).ok_or_else(|| {
            AppError::Database(format!("custom field '{name}' has an invalid ui_editable"))
        })?,
        is_cloneable: row.try_get("is_cloneable")?,
        search_weight: row.try_get("search_weight")?,
        filter_logic: FilterLogic::parse(&filter_logic_raw).ok_or_else(|| {
            AppError::Database(format!("custom field '{name}' has an invalid filter_logic"))
        })?,
        object_types: Vec::new(),
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    };
    validate_definition(&definition)?;
    Ok(Some(definition))
}

/// Validate an imported custom-field definition with the canonical checker
/// before it is written, instead of trusting the provider shape.
fn validate_source_custom_field(record: &SourceRecord) -> Result<CustomFieldDefinition, AppError> {
    fn text(record: &SourceRecord, name: &str) -> Option<String> {
        record
            .fields
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
    }
    fn choice(record: &SourceRecord, name: &str) -> Option<String> {
        match record.fields.get(name) {
            Some(Value::Object(map)) => {
                map.get("value").and_then(Value::as_str).map(str::to_string)
            }
            Some(Value::String(value)) => Some(value.clone()),
            _ => None,
        }
    }
    fn flag(record: &SourceRecord, name: &str) -> bool {
        record
            .fields
            .get(name)
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }
    fn integer(record: &SourceRecord, name: &str) -> i32 {
        record
            .fields
            .get(name)
            .and_then(Value::as_i64)
            .and_then(|value| i32::try_from(value).ok())
            .unwrap_or(0)
    }

    let name = text(record, "name").unwrap_or_default();
    let field_type_raw = choice(record, "type")
        .or_else(|| text(record, "data_type"))
        .unwrap_or_default();
    let field_type = CustomFieldType::parse(&field_type_raw).ok_or_else(|| {
        AppError::Validation(format!("custom field '{name}' has an unknown type"))
    })?;
    let definition = CustomFieldDefinition {
        id: Uuid::nil(),
        name: name.clone(),
        label: text(record, "label").unwrap_or_default(),
        field_type,
        description: text(record, "description").unwrap_or_default(),
        group_name: text(record, "group_name").unwrap_or_default(),
        required: flag(record, "required"),
        enforce_unique: flag(record, "unique"),
        is_secret: flag(record, "is_secret"),
        default_value: record
            .fields
            .get("default")
            .filter(|value| !value.is_null())
            .cloned(),
        weight: integer(record, "weight"),
        validation_minimum: record
            .fields
            .get("validation_minimum")
            .and_then(Value::as_f64),
        validation_maximum: record
            .fields
            .get("validation_maximum")
            .and_then(Value::as_f64),
        validation_regex: text(record, "validation_regex").unwrap_or_default(),
        validation_schema: record
            .fields
            .get("validation_schema")
            .filter(|value| !value.is_null())
            .cloned(),
        choice_set_id: None,
        related_object_type_id: None,
        related_object_filter: None,
        ui_visible: choice(record, "ui_visible")
            .and_then(|value| UiVisible::parse(&value))
            .unwrap_or(UiVisible::Always),
        ui_editable: choice(record, "ui_editable")
            .and_then(|value| UiEditable::parse(&value))
            .unwrap_or(UiEditable::Yes),
        is_cloneable: flag(record, "is_cloneable"),
        search_weight: integer(record, "search_weight"),
        filter_logic: choice(record, "filter_logic")
            .and_then(|value| FilterLogic::parse(&value))
            .unwrap_or(FilterLogic::Loose),
        object_types: Vec::new(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    validate_definition(&definition).map_err(|error| {
        AppError::Validation(format!(
            "custom field '{name}' definition is invalid: {error}"
        ))
    })?;
    Ok(definition)
}

async fn plan_row(
    row: &PreparedRow,
    local_ids: &HashMap<(String, String), Uuid>,
    incoming_definitions: &HashMap<(String, Uuid), CustomFieldDefinition>,
    content_types: &mut HashMap<String, Uuid>,
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<PlannedRow, AppError> {
    let local_id = local_ids
        .get(&(row.source_entity.clone(), row.external_id.clone()))
        .copied()
        .ok_or_else(|| AppError::Internal("Prepared row has no allocated identity".into()))?;
    let mut columns: Vec<PlannedColumn> = Vec::new();
    for (field, value) in &row.scalars {
        columns.push(PlannedColumn {
            column: field.column(),
            value: ColumnValue::Scalar(field.sql_type(), value.clone()),
            updatable: true,
        });
    }
    let mut relations = Vec::new();
    append_reference_columns(
        row,
        local_ids,
        content_types,
        transaction,
        &mut columns,
        &mut relations,
    )
    .await?;
    for field in &row.cleared {
        // Explicit null clears the whole reference: a generic reference clears
        // its type column and its id column together.
        if let FieldKind::GenericReference { type_column, .. } = field.kind() {
            columns.push(PlannedColumn {
                column: type_column,
                value: ColumnValue::Uuid(None),
                updatable: true,
            });
        }
        columns.push(PlannedColumn {
            column: field.column(),
            value: ColumnValue::Uuid(None),
            updatable: true,
        });
    }
    if let Some(tags) = &row.tags {
        columns.push(PlannedColumn {
            column: "tags",
            value: ColumnValue::Tags(tags.clone()),
            updatable: true,
        });
    }
    for &(column, kind, argument) in row.entity.internal_columns() {
        columns.push(internal_column(row, column, kind, argument)?);
    }
    let merged = resolve_custom_field_data(
        transaction,
        incoming_definitions,
        content_types,
        row,
        local_id,
    )
    .await?;
    apply_custom_field_data(&mut columns, merged);
    let mut junctions = Vec::new();
    for (field, refs) in &row.junctions {
        match field.kind() {
            FieldKind::Junction {
                table,
                local_column,
                remote_column,
                ..
            } => {
                let resolved: Vec<Uuid> = refs
                    .iter()
                    .map(|binding| {
                        resolve_reference(binding, local_ids).ok_or_else(|| {
                            AppError::Validation(format!(
                                "Unresolved relation {}::{}",
                                row.source_entity,
                                field.name()
                            ))
                        })
                    })
                    .collect::<Result<_, _>>()?;
                junctions.push(PlannedJunction {
                    table,
                    local_column,
                    remote_column,
                    refs: resolved,
                });
            }
            FieldKind::ContentTypeJunction {
                table,
                local_column,
                remote_column,
            } => {
                let mut resolved = Vec::with_capacity(refs.len());
                for binding in refs {
                    let token = binding.type_token.as_deref().ok_or_else(|| {
                        AppError::Validation("Content-type scope has no type".into())
                    })?;
                    let id = content_type_id(transaction, content_types, token)
                        .await?
                        .ok_or_else(|| {
                            AppError::Validation(format!(
                                "Unresolved scope {}::{}",
                                row.source_entity,
                                field.name()
                            ))
                        })?;
                    resolved.push(id);
                }
                junctions.push(PlannedJunction {
                    table,
                    local_column,
                    remote_column,
                    refs: resolved,
                });
            }
            _ => {}
        }
    }
    let value_junctions = plan_value_junctions(row);
    Ok(PlannedRow {
        entity: row.entity,
        source_entity: row.source_entity.clone(),
        external_id: row.external_id.clone(),
        local_id,
        columns,
        relations,
        junctions,
        value_junctions,
    })
}

/// Append the planned columns and relation statistics for a row's non-null
/// references, resolving each against the local bindings.
async fn append_reference_columns(
    row: &PreparedRow,
    local_ids: &HashMap<(String, String), Uuid>,
    content_types: &mut HashMap<String, Uuid>,
    transaction: &mut Transaction<'_, Postgres>,
    columns: &mut Vec<PlannedColumn>,
    relations: &mut Vec<(String, usize)>,
) -> Result<(), AppError> {
    for (field, binding) in &row.references {
        let resolved = resolve_reference(binding, local_ids).ok_or_else(|| {
            AppError::Validation(format!(
                "relation {}::{} cannot be resolved to a local row",
                row.entity.source_key(),
                field.name()
            ))
        })?;
        if let Some(token) = &binding.type_token {
            let content_type = content_type_id(transaction, content_types, token)
                .await?
                .ok_or_else(|| {
                    AppError::Validation(format!(
                        "required content type for {}::{} is not seeded",
                        row.entity.source_key(),
                        field.name()
                    ))
                })?;
            if let FieldKind::GenericReference { type_column, .. } = field.kind() {
                columns.push(PlannedColumn {
                    column: type_column,
                    value: ColumnValue::Uuid(Some(content_type)),
                    updatable: true,
                });
            }
        }
        columns.push(PlannedColumn {
            column: field.column(),
            value: ColumnValue::Uuid(Some(resolved)),
            updatable: true,
        });
        relations.push((field.name().to_string(), 1));
    }
    Ok(())
}

/// A synthetic column the target must add for a `NOT NULL`-without-default
/// column that no mapped field writes.
fn internal_column(
    row: &PreparedRow,
    column: &'static str,
    kind: &str,
    argument: &'static str,
) -> Result<PlannedColumn, AppError> {
    Ok(match kind {
        "literal" => PlannedColumn {
            column,
            value: ColumnValue::Literal(argument),
            updatable: false,
        },
        "copy" => {
            let copied = row
                .entity
                .field(argument)
                .and_then(|field| row.scalars.get(&field))
                .map(text_of)
                .unwrap_or_default();
            PlannedColumn {
                column,
                value: ColumnValue::Copy(copied),
                updatable: true,
            }
        }
        other => {
            return Err(AppError::Internal(format!(
                "unknown internal column kind {other}"
            )));
        }
    })
}

/// Write the merged `custom_field_data` jsonb, replacing an existing column in
/// place or appending it when none was planned.
fn apply_custom_field_data(columns: &mut Vec<PlannedColumn>, merged: Option<Value>) {
    let Some(merged) = merged else {
        return;
    };
    let mut replaced = false;
    for column in columns.iter_mut() {
        if column.column == "custom_field_data" {
            column.value = ColumnValue::Scalar(
                PgType::Jsonb,
                InventoryValue::Json {
                    value: merged.clone(),
                },
            );
            column.updatable = true;
            replaced = true;
        }
    }
    if !replaced {
        columns.push(PlannedColumn {
            column: "custom_field_data",
            value: ColumnValue::Scalar(PgType::Jsonb, InventoryValue::Json { value: merged }),
            updatable: true,
        });
    }
}

/// Value-junction columns planned for a row (service port mappings).
fn plan_value_junctions(row: &PreparedRow) -> Vec<PlannedValueJunction> {
    let mut value_junctions = Vec::new();
    if let Some((field, mappings)) = &row.port_mappings
        && let FieldKind::ValueJunction {
            table,
            local_column,
            protocol_column,
            port_column,
            ordinal_column,
        } = field.kind()
    {
        value_junctions.push(PlannedValueJunction {
            table,
            local_column,
            protocol_column,
            port_column,
            ordinal_column,
            rows: mappings.clone(),
        });
    }
    value_junctions
}

async fn upsert_binding(
    transaction: &mut Transaction<'_, Postgres>,
    source_provider_id: Uuid,
    plan: &PlannedRow,
    job_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO sync_inventory_binding \
         (source_provider_id, source_entity, external_id, target_entity, local_id, last_seen_job_id) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         ON CONFLICT (source_provider_id, source_entity, external_id, target_entity) \
         DO UPDATE SET local_id = EXCLUDED.local_id, last_seen_job_id = EXCLUDED.last_seen_job_id, \
                       updated_at = now()",
    )
    .bind(source_provider_id)
    .bind(&plan.source_entity)
    .bind(&plan.external_id)
    .bind(plan.entity.target_key())
    .bind(plan.local_id)
    .bind(job_id)
    .execute(&mut **transaction)
    .await
    .map_err(|error| AppError::Database(format!("Failed to upsert an inventory binding: {error}")))?;
    Ok(())
}

async fn upsert_row(
    transaction: &mut Transaction<'_, Postgres>,
    plan: &PlannedRow,
) -> Result<(), AppError> {
    let Some(table) = plan.entity.table() else {
        return Err(AppError::Internal(
            "a membership resource cannot be written as a row".to_string(),
        ));
    };
    let mut builder: QueryBuilder<'_, Postgres> = QueryBuilder::new("INSERT INTO ");
    builder.push(table);
    builder.push(" (id");
    let mut updatable: Vec<&'static str> = Vec::new();
    for column in &plan.columns {
        builder.push(", ").push(column.column);
        if column.updatable {
            updatable.push(column.column);
        }
    }
    builder.push(") VALUES (");
    builder.push_bind(plan.local_id);
    for column in &plan.columns {
        builder.push(", ");
        push_column_value(&mut builder, &column.value)?;
    }
    builder.push(") ON CONFLICT (id) DO UPDATE SET ");
    if updatable.is_empty() {
        builder.push("id = EXCLUDED.id");
    } else {
        for (index, column) in updatable.iter().enumerate() {
            if index > 0 {
                builder.push(", ");
            }
            builder.push(*column).push(" = EXCLUDED.").push(*column);
        }
    }
    builder
        .build()
        .execute(&mut **transaction)
        .await
        .map_err(|error| {
            AppError::Database(format!(
                "Failed to write a {} row: {error}",
                plan.source_entity
            ))
        })?;
    Ok(())
}

async fn replace_junctions(
    transaction: &mut Transaction<'_, Postgres>,
    plan: &PlannedRow,
) -> Result<(), AppError> {
    for junction in &plan.junctions {
        let delete = format!(
            "DELETE FROM {} WHERE {} = $1",
            junction.table, junction.local_column
        );
        sqlx::query(&delete)
            .bind(plan.local_id)
            .execute(&mut **transaction)
            .await
            .map_err(|error| AppError::Database(format!("Failed to clear a relation: {error}")))?;
        for remote in &junction.refs {
            let mut insert = QueryBuilder::<Postgres>::new("INSERT INTO ");
            insert.push(junction.table);
            insert.push(" (");
            insert.push(junction.local_column);
            insert.push(", ");
            insert.push(junction.remote_column);
            insert.push(") VALUES (");
            insert.push_bind(plan.local_id);
            insert.push(", ");
            insert.push_bind(*remote);
            insert.push(")");
            insert
                .build()
                .execute(&mut **transaction)
                .await
                .map_err(|error| {
                    AppError::Database(format!("Failed to write a relation: {error}"))
                })?;
        }
    }
    for value_junction in &plan.value_junctions {
        let delete = format!(
            "DELETE FROM {} WHERE {} = $1",
            value_junction.table, value_junction.local_column
        );
        sqlx::query(&delete)
            .bind(plan.local_id)
            .execute(&mut **transaction)
            .await
            .map_err(|error| AppError::Database(format!("Failed to clear a relation: {error}")))?;
        for (ordinal, mapping) in value_junction.rows.iter().enumerate() {
            let mut insert = QueryBuilder::<Postgres>::new("INSERT INTO ");
            insert.push(value_junction.table);
            insert.push(" (");
            insert.push(value_junction.local_column);
            insert.push(", ");
            insert.push(value_junction.protocol_column);
            insert.push(", ");
            insert.push(value_junction.port_column);
            insert.push(", ");
            insert.push(value_junction.ordinal_column);
            insert.push(") VALUES (");
            insert.push_bind(plan.local_id);
            insert.push(", ");
            insert.push_bind(mapping.protocol.as_str());
            insert.push(", ");
            insert.push_bind(mapping.port);
            insert.push(", ");
            insert.push_bind(i32::try_from(ordinal).unwrap_or(i32::MAX));
            insert.push(")");
            insert
                .build()
                .execute(&mut **transaction)
                .await
                .map_err(|error| {
                    AppError::Database(format!("Failed to write a relation: {error}"))
                })?;
        }
    }
    Ok(())
}

fn is_tree_entity(entity: InventoryEntity) -> bool {
    entity
        .internal_columns()
        .iter()
        .any(|(column, _, _)| *column == "tree_id")
}

/// Recompute `tree_id`/`level`/`lft`/`rght` for one nested-set table from the
/// whole parent graph. A row whose parent is absent (dangling or root) becomes
/// a root; a cycle is rejected. Existing rows are preserved — only the
/// bookkeeping columns change.
async fn recompute_tree(
    transaction: &mut Transaction<'_, Postgres>,
    entity: InventoryEntity,
) -> Result<(), AppError> {
    let table = entity
        .table()
        .ok_or_else(|| AppError::Internal("entity has no table".to_string()))?;
    let parent_of = load_parent_graph(transaction, table).await?;
    let (children, roots) = build_forest_hierarchy(&parent_of);
    let bookkeeping = walk_forest(&roots, &children, parent_of.len())?;
    write_tree_bookkeeping(transaction, table, &bookkeeping).await
}

/// Read a nested-set table's `(id, parent_id)` graph.
async fn load_parent_graph(
    transaction: &mut Transaction<'_, Postgres>,
    table: &str,
) -> Result<HashMap<Uuid, Option<Uuid>>, AppError> {
    let select = format!("SELECT id, parent_id FROM {table}");
    let rows = sqlx::query(&select)
        .fetch_all(&mut **transaction)
        .await
        .map_err(|error| AppError::Database(format!("Failed to read a hierarchy: {error}")))?;
    let mut parent_of: HashMap<Uuid, Option<Uuid>> = HashMap::with_capacity(rows.len());
    for row in &rows {
        let id: Uuid = row.try_get("id")?;
        let parent: Option<Uuid> = row.try_get("parent_id")?;
        parent_of.insert(id, parent);
    }
    Ok(parent_of)
}

/// Build the child adjacency and the sorted root list from a parent graph. A
/// parent absent from the graph (dangling or null) makes its row a root.
fn build_forest_hierarchy(
    parent_of: &HashMap<Uuid, Option<Uuid>>,
) -> (HashMap<Uuid, Vec<Uuid>>, Vec<Uuid>) {
    let mut children: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    let mut roots: Vec<Uuid> = Vec::new();
    for (&id, parent) in parent_of {
        match (*parent).filter(|value| parent_of.contains_key(value)) {
            Some(parent) => children.entry(parent).or_default().push(id),
            None => roots.push(id),
        }
    }
    roots.sort_unstable();
    for list in children.values_mut() {
        list.sort_unstable();
    }
    (children, roots)
}

/// The nested-set bookkeeping assigned to every row of one table.
struct TreeBookkeeping {
    tree_of: HashMap<Uuid, i32>,
    level_of: HashMap<Uuid, i32>,
    lft_of: HashMap<Uuid, i32>,
    rght_of: HashMap<Uuid, i32>,
}

/// Assign `tree_id`/`level`/`lft`/`rght` by walking each root's subtree. A
/// revisit proves a cycle, as does a walk that does not reach every row.
fn walk_forest(
    roots: &[Uuid],
    children: &HashMap<Uuid, Vec<Uuid>>,
    row_count: usize,
) -> Result<TreeBookkeeping, AppError> {
    let mut bookkeeping = TreeBookkeeping {
        tree_of: HashMap::with_capacity(row_count),
        level_of: HashMap::new(),
        lft_of: HashMap::new(),
        rght_of: HashMap::new(),
    };
    let mut counter: i32 = 1;
    for (index, root) in roots.iter().enumerate() {
        let tree_id = i32::try_from(index + 1)
            .map_err(|_| AppError::Validation("hierarchy is too large".to_string()))?;
        walk_subtree(*root, tree_id, children, &mut bookkeeping, &mut counter)?;
    }
    if bookkeeping.tree_of.len() != row_count {
        return Err(AppError::Validation(
            "hierarchy contains a cycle".to_string(),
        ));
    }
    Ok(bookkeeping)
}

/// Walk one root's subtree depth-first, numbering the nested-set columns and
/// rejecting a row reached twice (a cycle).
fn walk_subtree(
    root: Uuid,
    tree_id: i32,
    children: &HashMap<Uuid, Vec<Uuid>>,
    bookkeeping: &mut TreeBookkeeping,
    counter: &mut i32,
) -> Result<(), AppError> {
    let mut stack: Vec<(Uuid, i32, bool)> = vec![(root, 0, false)];
    while let Some((id, depth, exiting)) = stack.pop() {
        if exiting {
            bookkeeping.rght_of.insert(id, *counter);
            *counter += 1;
            continue;
        }
        if bookkeeping.tree_of.contains_key(&id) {
            return Err(AppError::Validation(
                "hierarchy contains a cycle".to_string(),
            ));
        }
        bookkeeping.tree_of.insert(id, tree_id);
        bookkeeping.level_of.insert(id, depth);
        bookkeeping.lft_of.insert(id, *counter);
        *counter += 1;
        stack.push((id, depth, true));
        if let Some(kids) = children.get(&id) {
            for child in kids.iter().rev() {
                stack.push((*child, depth + 1, false));
            }
        }
    }
    Ok(())
}

/// Write the computed bookkeeping columns back for every visited row.
async fn write_tree_bookkeeping(
    transaction: &mut Transaction<'_, Postgres>,
    table: &str,
    bookkeeping: &TreeBookkeeping,
) -> Result<(), AppError> {
    for (id, tree_id) in &bookkeeping.tree_of {
        let update = format!(
            "UPDATE {table} SET tree_id = $2, level = $3, lft = $4, rght = $5 WHERE id = $1"
        );
        sqlx::query(&update)
            .bind(id)
            .bind(*tree_id)
            .bind(bookkeeping.level_of.get(id).copied().unwrap_or(0))
            .bind(bookkeeping.lft_of.get(id).copied().unwrap_or(0))
            .bind(bookkeeping.rght_of.get(id).copied().unwrap_or(0))
            .execute(&mut **transaction)
            .await
            .map_err(|error| AppError::Database(format!("Failed to write a hierarchy: {error}")))?;
    }
    Ok(())
}

async fn update_job(
    transaction: &mut Transaction<'_, Postgres>,
    job: &SyncJob,
) -> Result<SyncJob, AppError> {
    let row = sqlx::query(
        "UPDATE sync_job SET status = $2, started_at = COALESCE(started_at, $3), \
         finished_at = $4, stats = $5 WHERE id = $1 \
         RETURNING id, provider_id, mapping_version, status, started_at, finished_at, stats",
    )
    .bind(job.id)
    .bind(status_to_string(&job.status))
    .bind(job.started_at)
    .bind(job.finished_at)
    .bind(&job.stats)
    .fetch_one(&mut **transaction)
    .await
    .map_err(|error| AppError::Database(format!("Failed to complete the sync job: {error}")))?;
    let status: String = row.try_get("status")?;
    Ok(SyncJob {
        id: row.try_get("id")?,
        provider_id: row.try_get("provider_id")?,
        mapping_version: row.try_get("mapping_version")?,
        status: parse_status(&status)?,
        started_at: row.try_get("started_at")?,
        finished_at: row.try_get("finished_at")?,
        stats: row.try_get("stats")?,
    })
}

fn status_to_string(status: &SyncStatus) -> String {
    match status {
        SyncStatus::Queued => "queued",
        SyncStatus::Running => "running",
        SyncStatus::Completed => "completed",
        SyncStatus::Failed => "failed",
    }
    .to_string()
}

fn parse_status(status: &str) -> Result<SyncStatus, AppError> {
    match status {
        "queued" => Ok(SyncStatus::Queued),
        "running" => Ok(SyncStatus::Running),
        "completed" => Ok(SyncStatus::Completed),
        "failed" => Ok(SyncStatus::Failed),
        other => Err(AppError::Validation(format!(
            "Unknown sync status '{other}'"
        ))),
    }
}

fn text_of(value: &InventoryValue) -> String {
    match value {
        InventoryValue::Text { value } => value.clone(),
        InventoryValue::Integer { value } => value.to_string(),
        InventoryValue::Number { value } => value.to_string(),
        InventoryValue::Boolean { value } => value.to_string(),
        InventoryValue::Null | InventoryValue::Json { .. } => String::new(),
    }
}

fn push_column_value<'a>(
    builder: &mut QueryBuilder<'a, Postgres>,
    value: &'a ColumnValue,
) -> Result<(), AppError> {
    match value {
        ColumnValue::Literal(sql) => {
            builder.push(*sql);
        }
        ColumnValue::Copy(text) => {
            builder.push_bind(text.as_str());
        }
        ColumnValue::Uuid(Some(uuid)) => {
            builder.push_bind(*uuid);
        }
        ColumnValue::Uuid(None) => {
            builder.push("NULL");
        }
        ColumnValue::Tags(tags) => {
            builder.push_bind(tags.clone());
        }
        ColumnValue::Scalar(sql_type, scalar) => push_scalar(builder, *sql_type, scalar)?,
    }
    Ok(())
}

fn json_string_array(value: &Value) -> Result<Vec<String>, AppError> {
    value
        .as_array()
        .ok_or_else(|| AppError::Validation("expected a text array value".to_string()))?
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_string)
                .ok_or_else(|| AppError::Validation("array entry is not text".to_string()))
        })
        .collect()
}

fn json_int_array(value: &Value) -> Result<Vec<i32>, AppError> {
    value
        .as_array()
        .ok_or_else(|| AppError::Validation("expected an integer array value".to_string()))?
        .iter()
        .map(|item| {
            let number = item
                .as_i64()
                .ok_or_else(|| AppError::Validation("array entry is not an integer".to_string()))?;
            i32::try_from(number).map_err(|_| {
                AppError::Validation("array entry exceeds the integer range".to_string())
            })
        })
        .collect()
}

fn push_scalar<'a>(
    builder: &mut QueryBuilder<'a, Postgres>,
    sql_type: PgType,
    value: &'a InventoryValue,
) -> Result<(), AppError> {
    match (sql_type, value) {
        (_, InventoryValue::Null) => {
            builder.push("NULL");
        }
        (PgType::Bool, InventoryValue::Boolean { value }) => {
            builder.push_bind(*value);
        }
        (PgType::Integer | PgType::SmallInt, InventoryValue::Integer { value }) => {
            let narrowed = i32::try_from(*value)
                .map_err(|_| AppError::Validation("integer value is out of range".to_string()))?;
            builder.push_bind(narrowed);
        }
        (PgType::BigInt, InventoryValue::Integer { value }) => {
            builder.push_bind(*value);
        }
        (PgType::Numeric | PgType::Double | PgType::Real, InventoryValue::Number { value }) => {
            builder.push_bind(*value);
        }
        (PgType::Text, InventoryValue::Text { value }) => {
            builder.push_bind(value.as_str());
        }
        (PgType::Uuid, InventoryValue::Text { value }) => {
            builder.push_bind(value.as_str());
            builder.push("::uuid");
        }
        (PgType::Inet, InventoryValue::Text { value }) => {
            builder.push_bind(value.as_str());
            builder.push("::inet");
        }
        (PgType::Cidr, InventoryValue::Text { value }) => {
            builder.push_bind(value.as_str());
            builder.push("::cidr");
        }
        (PgType::MacAddr, InventoryValue::Text { value }) => {
            builder.push_bind(value.as_str());
            builder.push("::macaddr");
        }
        (PgType::Jsonb, InventoryValue::Json { value }) => {
            builder.push_bind(value);
        }
        (PgType::Json, InventoryValue::Json { value }) => {
            builder.push_bind(value);
            builder.push("::json");
        }
        (PgType::TextArray, InventoryValue::Json { value }) => {
            builder.push_bind(json_string_array(value)?);
        }
        (PgType::IntArray, InventoryValue::Json { value }) => {
            builder.push_bind(json_int_array(value)?);
        }
        (other, _) => {
            return Err(AppError::Validation(format!(
                "unsupported column type {other:?}"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    use crate::contexts::sync::domain::inventory_sync::{SourceEntity, SourceIdentity};

    fn record(entity: &str, external_id: &str, fields: Value) -> SourceRecord {
        let identity = SourceIdentity::new(
            Uuid::now_v7(),
            SourceEntity::new(entity).expect("valid source entity"),
            external_id,
        )
        .expect("valid identity");
        let Value::Object(fields) = fields else {
            panic!("fields must be an object");
        };
        SourceRecord::new(identity, fields)
    }

    fn literal(target: &str, value: Value) -> FieldMapping {
        FieldMapping {
            id: Uuid::now_v7(),
            entity_mapping_id: Uuid::now_v7(),
            source_field: target.to_string(),
            target_field: target.to_string(),
            forward_transform_json: json!({ "op": "literal", "args": [value] }),
            reverse_transform_json: None,
            round_trip_critical: false,
            created_at: chrono::Utc::now(),
        }
    }

    fn identity(target: &str, source: &str) -> FieldMapping {
        FieldMapping {
            id: Uuid::now_v7(),
            entity_mapping_id: Uuid::now_v7(),
            source_field: source.to_string(),
            target_field: target.to_string(),
            forward_transform_json: json!({ "op": "identity", "args": [{ "field": source }] }),
            reverse_transform_json: None,
            round_trip_critical: false,
            created_at: chrono::Utc::now(),
        }
    }

    fn borrow(mappings: &[FieldMapping]) -> Vec<&FieldMapping> {
        mappings.iter().collect()
    }

    #[test]
    fn local_id_is_deterministic_and_tuple_sensitive() {
        let provider = Uuid::now_v7();
        let a = derive_local_id(provider, "dcim.devices", "5", "piping.inventory.device");
        assert_eq!(
            a,
            derive_local_id(provider, "dcim.devices", "5", "piping.inventory.device")
        );
        assert_ne!(
            a,
            derive_local_id(provider, "dcim.devices", "6", "piping.inventory.device")
        );
        assert_ne!(
            a,
            derive_local_id(provider, "dcim.sites", "5", "piping.inventory.device")
        );
        assert_ne!(
            a,
            derive_local_id(provider, "dcim.devices", "5", "piping.inventory.site")
        );
        assert_ne!(
            a,
            derive_local_id(
                Uuid::now_v7(),
                "dcim.devices",
                "5",
                "piping.inventory.device"
            )
        );
    }

    #[test]
    fn nullable_definition_metadata_is_absent_in_the_domain() {
        let source = record(
            "extras.custom-fields",
            "1",
            json!({
                "name": "owner_team", "type": "text", "default": null, "validation_schema": null,
            }),
        );
        let definition =
            validate_source_custom_field(&source).expect("null optional metadata is valid");
        assert!(definition.default_value.is_none());
        assert!(definition.validation_schema.is_none());
    }

    #[test]
    fn device_row_converts_scalars_choices_and_references() {
        let provider = Uuid::now_v7();
        let rec = record("dcim.devices", "5", json!({ "name": "r1" }));
        let mappings = vec![
            identity("name", "name"),
            literal("status", json!({ "value": "active", "label": "Active" })),
            literal("role", json!({ "id": 7 })),
            literal("site", json!({ "id": 3 })),
            literal("tags", json!([{ "slug": "core" }])),
        ];
        let row = prepare_row(InventoryEntity::Device, provider, &rec, &borrow(&mappings))
            .expect("prepared");
        assert_eq!(row.entity, InventoryEntity::Device);
        assert_eq!(row.external_id, "5");
        assert_eq!(row.tags.as_deref(), Some(&["core".to_string()][..]));
        let name = InventoryEntity::Device.field("name").expect("name");
        assert_eq!(
            row.scalars.get(&name),
            Some(&InventoryValue::Text {
                value: "r1".to_string()
            })
        );
        let status = InventoryEntity::Device.field("status").expect("status");
        assert_eq!(
            row.scalars.get(&status),
            Some(&InventoryValue::Text {
                value: "active".to_string()
            })
        );
        let role = row
            .references
            .iter()
            .find(|(field, _)| field.name() == "role")
            .expect("role reference");
        assert_eq!(role.1.target, Some(InventoryEntity::DeviceRole));
        assert_eq!(role.1.external_id, "7");
    }

    #[test]
    fn generic_reference_captures_the_type_token() {
        let provider = Uuid::now_v7();
        let rec = record(
            "dcim.mac-addresses",
            "9",
            json!({ "assigned_object_type": "dcim.interface" }),
        );
        let mappings = vec![
            literal("mac_address", json!("aa:bb:cc:dd:ee:ff")),
            literal("assigned_object", json!({ "id": 42 })),
        ];
        let row = prepare_row(
            InventoryEntity::MacAddress,
            provider,
            &rec,
            &borrow(&mappings),
        )
        .expect("prepared");
        let (field, binding) = row
            .references
            .iter()
            .find(|(field, _)| field.name() == "assigned_object")
            .expect("gfk reference");
        assert_eq!(binding.target, Some(InventoryEntity::Interface));
        assert_eq!(binding.type_token.as_deref(), Some("dcim.interface"));
        assert_eq!(binding.external_id, "42");
        assert!(matches!(field.kind(), FieldKind::GenericReference { .. }));
    }

    #[test]
    fn generic_reference_rejects_unknown_or_disallowed_type() {
        let provider = Uuid::now_v7();
        for token in ["dcim.cable", "dcim.device"] {
            let rec = record(
                "dcim.mac-addresses",
                "9",
                json!({ "assigned_object_type": token }),
            );
            let mappings = vec![literal("assigned_object", json!({ "id": 42 }))];
            assert!(
                prepare_row(
                    InventoryEntity::MacAddress,
                    provider,
                    &rec,
                    &borrow(&mappings)
                )
                .is_err(),
                "token {token} must be rejected"
            );
        }
    }

    #[test]
    fn integer_overflow_is_blocked() {
        let provider = Uuid::now_v7();
        let rec = record("dcim.interfaces", "1", json!({}));
        let mappings = vec![literal("mtu", json!(5_000_000_000_i64))];
        assert!(
            prepare_row(
                InventoryEntity::Interface,
                provider,
                &rec,
                &borrow(&mappings)
            )
            .is_err()
        );
    }

    #[test]
    fn unknown_mapping_target_blocks_and_derived_target_blocks() {
        let provider = Uuid::now_v7();
        let rec = record("dcim.devices", "5", json!({}));
        for target in ["mystery_business_field", "display"] {
            let mappings = vec![literal(target, json!("x"))];
            assert!(
                prepare_row(InventoryEntity::Device, provider, &rec, &borrow(&mappings)).is_err(),
                "target {target} must be rejected"
            );
        }
    }

    #[test]
    fn service_ports_keep_the_native_integer_array() {
        let provider = Uuid::now_v7();
        let rec = record("ipam.services", "1", json!({}));
        let mappings = vec![
            literal("name", json!("https")),
            literal("ports", json!([80, 443])),
            literal("tags", json!(["core"])),
        ];
        let row = prepare_row(InventoryEntity::Service, provider, &rec, &borrow(&mappings))
            .expect("prepared");
        let ports = InventoryEntity::Service.field("ports").expect("ports");
        assert!(matches!(
            row.scalars.get(&ports),
            Some(InventoryValue::Json { .. })
        ));
        assert_eq!(row.tags.as_deref(), Some(&["core".to_string()][..]));
    }

    #[test]
    fn missing_reference_id_blocks() {
        let provider = Uuid::now_v7();
        let rec = record("dcim.devices", "5", json!({}));
        let mappings = vec![literal("role", json!({ "display": "no id" }))];
        assert!(prepare_row(InventoryEntity::Device, provider, &rec, &borrow(&mappings)).is_err());
    }

    #[test]
    fn service_port_mappings_parse_protocol_and_port() {
        // Genuine live shape: a service with an empty `ports` array but a
        // non-empty `port_mappings` list of `<protocol>/<port>` strings.
        let provider = Uuid::now_v7();
        let rec = record("ipam.services", "1", json!({ "ports": [] }));
        let mappings = vec![
            literal("ports", json!([])),
            literal("port_mappings", json!(["tcp/80", "udp/53"])),
        ];
        let row = prepare_row(InventoryEntity::Service, provider, &rec, &borrow(&mappings))
            .expect("prepared");
        let (field, parsed) = row.port_mappings.clone().expect("port mappings captured");
        assert_eq!(field.name(), "port_mappings");
        assert_eq!(
            parsed,
            vec![
                PortMapping {
                    protocol: "tcp".to_string(),
                    port: 80
                },
                PortMapping {
                    protocol: "udp".to_string(),
                    port: 53
                }
            ]
        );
    }

    #[test]
    fn malformed_port_mapping_blocks() {
        let provider = Uuid::now_v7();
        for bad in [json!(["not-a-mapping"]), json!(["tcp/70000"]), json!([7])] {
            let rec = record("ipam.services", "1", json!({}));
            let mappings = vec![literal("port_mappings", bad.clone())];
            assert!(
                prepare_row(InventoryEntity::Service, provider, &rec, &borrow(&mappings)).is_err(),
                "value {bad} must be rejected"
            );
        }
    }

    #[test]
    fn custom_fields_are_captured_from_raw_record_or_mapping() {
        let provider = Uuid::now_v7();
        // Read straight from the raw source record when the mapping omits it.
        let rec = record(
            "dcim.devices",
            "5",
            json!({ "custom_fields": { "owner_team": "ops" }, "name": "r1" }),
        );
        let mappings = vec![identity("name", "name")];
        let row = prepare_row(InventoryEntity::Device, provider, &rec, &borrow(&mappings))
            .expect("prepared");
        assert_eq!(row.custom_fields.as_ref().map(|map| map.len()), Some(1));

        // An explicit `custom_fields` mapping target is honoured too.
        let rec = record("dcim.devices", "5", json!({ "name": "r1" }));
        let mappings = vec![
            identity("name", "name"),
            literal("custom_fields", json!({ "owner_team": "ops" })),
        ];
        let row = prepare_row(InventoryEntity::Device, provider, &rec, &borrow(&mappings))
            .expect("prepared");
        assert_eq!(row.custom_fields.as_ref().map(|map| map.len()), Some(1));

        // An explicitly empty object plans a clear, not a preserve.
        let rec = record("dcim.devices", "5", json!({ "custom_fields": {} }));
        let row = prepare_row(
            InventoryEntity::Device,
            provider,
            &rec,
            &borrow(&Vec::<FieldMapping>::new()),
        )
        .expect("prepared");
        assert_eq!(row.custom_fields, Some(serde_json::Map::new()));
    }

    #[test]
    fn tree_entities_are_the_nested_set_tables() {
        for entity in [
            InventoryEntity::Region,
            InventoryEntity::SiteGroup,
            InventoryEntity::Platform,
            InventoryEntity::DeviceRole,
            InventoryEntity::InventoryItem,
        ] {
            assert!(is_tree_entity(entity), "{entity:?} must be a tree entity");
        }
        for entity in [
            InventoryEntity::Device,
            InventoryEntity::Site,
            InventoryEntity::IpAddress,
        ] {
            assert!(
                !is_tree_entity(entity),
                "{entity:?} must not be a tree entity"
            );
        }
    }

    #[test]
    fn required_tag_identity_fields_reject_absence() {
        let provider = Uuid::now_v7();
        let rec = record("extras.tags", "1", json!({}));
        for field in ["name", "slug"] {
            let mappings = vec![identity(field, field)];
            assert!(
                prepare_row(InventoryEntity::Tag, provider, &rec, &borrow(&mappings)).is_err(),
                "absent tag {field} must be rejected"
            );
        }
    }

    #[test]
    fn populated_out_of_slice_inventory_component_references_are_rejected() {
        let provider = Uuid::now_v7();
        let rec = record(
            "dcim.inventory-items",
            "1",
            json!({"component_type":"dcim.interface","component":{"id":1}}),
        );
        let mappings = vec![identity("component", "component")];
        assert!(
            prepare_row(
                InventoryEntity::InventoryItem,
                provider,
                &rec,
                &borrow(&mappings)
            )
            .is_err()
        );
    }

    #[test]
    fn null_optional_columns_use_native_empty_encoding() {
        let provider = Uuid::now_v7();
        // DeviceType.front_image/rear_image are NOT NULL with no DB default and
        // are null in the source: native empty string, never a missing column.
        let rec = record("dcim.device-types", "1", json!({}));
        let mappings = vec![
            literal("front_image", Value::Null),
            literal("rear_image", Value::Null),
        ];
        let row = prepare_row(
            InventoryEntity::DeviceType,
            provider,
            &rec,
            &borrow(&mappings),
        )
        .expect("prepared");
        let front = InventoryEntity::DeviceType
            .field("front_image")
            .expect("front_image");
        assert_eq!(
            row.scalars.get(&front),
            Some(&InventoryValue::Text {
                value: String::new()
            })
        );

        // Nullable service fields preserve explicit null, not an empty encoding.
        let rec = record("ipam.services", "1", json!({}));
        let mappings = vec![
            literal("protocol", Value::Null),
            literal("ports", Value::Null),
        ];
        let row = prepare_row(InventoryEntity::Service, provider, &rec, &borrow(&mappings))
            .expect("prepared");
        let protocol = InventoryEntity::Service
            .field("protocol")
            .expect("protocol");
        assert_eq!(row.scalars.get(&protocol), Some(&InventoryValue::Null));
        let ports = InventoryEntity::Service.field("ports").expect("ports");
        assert_eq!(row.scalars.get(&ports), Some(&InventoryValue::Null));
    }

    #[test]
    fn null_required_field_is_rejected_and_nullable_is_preserved() {
        let provider = Uuid::now_v7();
        let rec = record("dcim.sites", "5", json!({}));

        // Site.name is NOT NULL; Device.name is nullable in the native schema.
        let reject = vec![literal("name", Value::Null)];
        assert!(prepare_row(InventoryEntity::Site, provider, &rec, &borrow(&reject)).is_err());
        let rec = record("dcim.devices", "5", json!({}));

        // Explicit null on a nullable scalar clears it to SQL NULL.
        let clear = vec![literal("latitude", Value::Null)];
        let row = prepare_row(InventoryEntity::Device, provider, &rec, &borrow(&clear))
            .expect("prepared");
        let latitude = InventoryEntity::Device.field("latitude").expect("latitude");
        assert_eq!(row.scalars.get(&latitude), Some(&InventoryValue::Null));

        // A missing property preserves the local column (nothing planned).
        let missing = vec![identity("latitude", "latitude")];
        let row = prepare_row(InventoryEntity::Device, provider, &rec, &borrow(&missing))
            .expect("prepared");
        assert!(!row.scalars.contains_key(&latitude));
    }

    #[test]
    fn explicit_null_clears_relations_and_missing_reference_preserves() {
        let provider = Uuid::now_v7();
        let rec = record("dcim.devices", "5", json!({}));

        // Explicit null on a nullable FK records a clear of the column.
        let clear = vec![literal("rack", Value::Null)];
        let row = prepare_row(InventoryEntity::Device, provider, &rec, &borrow(&clear))
            .expect("prepared");
        let rack = InventoryEntity::Device.field("rack").expect("rack");
        assert_eq!(row.cleared, vec![rack]);
        assert!(row.references.is_empty());

        // Explicit null on a GFK clears both columns (type + id).
        let rec = record("dcim.mac-addresses", "9", json!({}));
        let clear = vec![literal("assigned_object", Value::Null)];
        let row = prepare_row(InventoryEntity::MacAddress, provider, &rec, &borrow(&clear))
            .expect("prepared");
        let gfk = InventoryEntity::MacAddress
            .field("assigned_object")
            .expect("assigned_object");
        assert!(row.cleared.contains(&gfk));

        // Missing FK property preserves the local column.
        let rec = record("dcim.devices", "5", json!({}));
        let missing = vec![identity("rack", "rack")];
        let row = prepare_row(InventoryEntity::Device, provider, &rec, &borrow(&missing))
            .expect("prepared");
        assert!(row.cleared.is_empty());
        assert!(row.references.is_empty());

        // A NOT NULL relation cannot be cleared.
        let rec = record("virtualization.clusters", "1", json!({}));
        let clear_type = vec![literal("type", Value::Null)];
        assert!(
            prepare_row(
                InventoryEntity::Cluster,
                provider,
                &rec,
                &borrow(&clear_type)
            )
            .is_err()
        );
    }
    #[test]
    fn imported_custom_field_definitions_are_validated_canonically() {
        let good = record(
            "extras.custom-fields",
            "1",
            json!({ "name": "owner_team", "type": { "value": "text" }, "unique": false, "required": false }),
        );
        assert!(validate_source_custom_field(&good).is_ok());

        let bad_name = record(
            "extras.custom-fields",
            "1",
            json!({ "name": "Bad Name", "type": { "value": "text" } }),
        );
        assert!(validate_source_custom_field(&bad_name).is_err());

        let unknown_type = record(
            "extras.custom-fields",
            "1",
            json!({ "name": "ok", "type": { "value": "nope" } }),
        );
        assert!(validate_source_custom_field(&unknown_type).is_err());
    }
    #[test]
    fn populated_out_of_scope_reference_is_recorded_and_unresolvable() {
        let provider = Uuid::now_v7();
        // `config_template` targets an out-of-scope table: the reference is
        // recorded (never silently dropped) and cannot be resolved, so the
        // commit planner fails rather than leaving the column unset.
        let rec = record("dcim.devices", "5", json!({}));
        let mappings = vec![literal("config_template", json!({ "id": 7 }))];
        let row = prepare_row(InventoryEntity::Device, provider, &rec, &borrow(&mappings))
            .expect("prepared");
        let (_, binding) = row.references.first().expect("reference");
        assert_eq!(binding.target, None);
        assert_eq!(binding.external_id, "7");
        assert!(resolve_reference(binding, &HashMap::new()).is_none());
    }
    #[test]
    fn source_custom_field_definition_is_returned_for_the_incoming_cache() {
        let rec = record(
            "extras.custom-fields",
            "1",
            json!({ "name": "meta", "type": { "value": "json" }, "required": false }),
        );
        let definition = validate_source_custom_field(&rec).expect("valid definition");
        assert_eq!(definition.name, "meta");
        assert_eq!(definition.field_type, CustomFieldType::Json);
        // The definition is the cache payload keyed by (name, scope), so a
        // value validated against it needs no DB row.
        assert_eq!(
            validate_source_custom_field(&rec).expect("again").name,
            "meta"
        );
    }
}
