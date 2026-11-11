//! Source and target graph checks for the native Interface writer. Cable sides
//! are retained assertions, not another endpoint store or a second write path.
//! Commit holds the canonical hardware graph lock before planning and through
//! commit; read-only assessment makes no reservation beyond its snapshot.

use super::*;
use crate::contexts::inventory::domain::dcim::interface::{InterfaceCableEnd, InterfaceNetworking};
use crate::contexts::inventory::infrastructure::persistence::repositories::dcim::cabling_repository::{CableAssociation, CableEndpointKind, observe_cable_associations};
use crate::contexts::sync::domain::SourceIdentity;

#[cfg(test)]
#[path = "cable_side_concurrency_tests.rs"]
mod cable_side_concurrency_tests;

#[derive(Debug, Clone)]
pub(super) struct SideAssertion {
    cable: SourceIdentity,
    end: InterfaceCableEnd,
    interfaces: Vec<SourceIdentity>,
}

fn target_finding(assertion: &SideAssertion, code: FindingCode) -> Finding {
    let mut finding = Finding::blocker(code, Capability::TargetGraph);
    finding.entity = Some(assertion.cable.entity.to_string());
    finding.record_identity_sha256 = Some(digest(assertion.cable.external_id.as_bytes()));
    finding.field_path = Some(
        match assertion.end {
            InterfaceCableEnd::A => "a_terminations",
            InterfaceCableEnd::B => "b_terminations",
        }
        .into(),
    );
    finding
}

pub(super) fn attachment(row: &PlannedRow) -> Option<(Option<Uuid>, Option<InterfaceCableEnd>)> {
    let cable = row
        .columns
        .iter()
        .find(|column| column.column == "cable_id")?;
    let end = row
        .columns
        .iter()
        .find(|column| column.column == "cable_end")?;
    match (&cable.value, &end.value) {
        (ColumnValue::Uuid(None), ColumnValue::Scalar(_, InventoryValue::Null)) => {
            Some((None, None))
        }
        (ColumnValue::Uuid(Some(id)), ColumnValue::Scalar(_, InventoryValue::Text { value })) => {
            side(&Value::String(value.clone())).map(|end| (Some(*id), Some(end)))
        }
        _ => None,
    }
}

/// Resolve only through the provider-qualified binding map used by plan_row.
/// This check never constructs endpoint writes; it requires their existing
/// canonical Interface plans to account for every selected native association.
pub(super) fn assess_observed(
    batch: &LocalInventoryBatch,
    planned: &[PlannedRow],
    ids: &HashMap<(String, String), Uuid>,
    observed: &[CableAssociation],
) -> (Vec<Finding>, bool) {
    let mut findings = Vec::new();
    let mut complete = true;
    for assertion in &batch.cable_sides {
        let resolve = |identity: &SourceIdentity| {
            (identity.source_provider_id == batch.source_provider_id)
                .then(|| {
                    ids.get(&(identity.entity.to_string(), identity.external_id.clone()))
                        .copied()
                })
                .flatten()
        };
        let Some(cable_id) = resolve(&assertion.cable) else {
            findings.push(target_finding(
                assertion,
                FindingCode::AnalysisDependencyUnavailable,
            ));
            complete = false;
            continue;
        };
        let mut declared = HashSet::new();
        for identity in &assertion.interfaces {
            let endpoint = resolve(identity);
            let plan = endpoint.and_then(|id| {
                planned
                    .iter()
                    .find(|row| row.entity == InventoryEntity::Interface && row.local_id == id)
            });
            if let Some(id) = endpoint {
                declared.insert(id);
            }
            if !plan.is_some_and(|row| {
                attachment(row) == Some((Some(cable_id), Some(assertion.end.clone())))
            }) {
                findings.push(target_finding(
                    assertion,
                    FindingCode::AnalysisDependencyUnavailable,
                ));
                complete = false;
            }
        }
        for existing in observed
            .iter()
            .filter(|existing| existing.cable_id == cable_id)
        {
            // A missing/invalid side cannot safely be assumed to be the omitted
            // side. Other native endpoint kinds have no importer writer here.
            if existing.end.is_none()
                || (existing.end == Some(assertion.end.clone())
                    && existing.kind != CableEndpointKind::Interface)
            {
                findings.push(target_finding(
                    assertion,
                    FindingCode::AnalysisDependencyUnavailable,
                ));
                complete = false;
                continue;
            }
            if existing.end != Some(assertion.end.clone()) {
                continue;
            }
            let mutation = planned
                .iter()
                .find(|row| {
                    row.entity == InventoryEntity::Interface && row.local_id == existing.endpoint_id
                })
                .and_then(attachment);
            match mutation {
                Some((None, None)) => {}
                Some((Some(destination), Some(end)))
                    if destination != cable_id || end != assertion.end => {}
                Some((Some(_), Some(_))) if declared.contains(&existing.endpoint_id) => {}
                _ => {
                    findings.push(target_finding(
                        assertion,
                        FindingCode::AnalysisDependencyUnavailable,
                    ));
                    complete = false;
                }
            }
        }
        for row in planned
            .iter()
            .filter(|row| row.entity == InventoryEntity::Interface)
        {
            if attachment(row) == Some((Some(cable_id), Some(assertion.end.clone())))
                && !declared.contains(&row.local_id)
            {
                findings.push(target_finding(assertion, FindingCode::TargetRowInvalid));
            }
        }
    }
    (findings, complete)
}

pub(super) async fn assess_target(
    batch: &LocalInventoryBatch,
    planned: &[PlannedRow],
    ids: &HashMap<(String, String), Uuid>,
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<(Vec<Finding>, bool), AppError> {
    if batch.cable_sides.is_empty() {
        return Ok((Vec::new(), true));
    }
    let cables: Vec<_> = batch
        .cable_sides
        .iter()
        .filter_map(|assertion| {
            ids.get(&(
                assertion.cable.entity.to_string(),
                assertion.cable.external_id.clone(),
            ))
            .copied()
        })
        .collect();
    let observed = observe_cable_associations(transaction, &cables).await?;
    Ok(assess_observed(batch, planned, ids, &observed))
}

const NETWORKING: &[&str] = &[
    "mtu",
    "mode",
    "speed",
    "duplex",
    "poe_mode",
    "poe_type",
    "rf_role",
    "rf_channel",
    "rf_channel_frequency",
    "rf_channel_width",
    "tx_power",
    "wwn",
    "channel_id",
    "channels",
    "cable_end",
    "cable_connector",
    "cable_positions",
];

pub(super) fn valid_scalar(field: InventoryField, value: &InventoryValue) -> bool {
    if field.entity() != InventoryEntity::Interface || !NETWORKING.contains(&field.name()) {
        return true;
    }
    let value = match value {
        InventoryValue::Null => Value::Null,
        InventoryValue::Boolean { value } => Value::Bool(*value),
        InventoryValue::Integer { value } if field.name() == "speed" => {
            Value::String(value.to_string())
        }
        InventoryValue::Integer { value } => serde_json::json!(value),
        InventoryValue::Number { value } => serde_json::json!(value),
        InventoryValue::Text { value } => Value::String(value.clone()),
        InventoryValue::Json { value } => value.clone(),
    };
    let mut object = serde_json::Map::new();
    object.insert(field.name().into(), value);
    serde_json::from_value::<InterfaceNetworking>(Value::Object(object))
        .is_ok_and(|networking| networking.validate().is_ok())
}

fn finding(record: &SourceRecord, field: &str, code: FindingCode) -> Finding {
    let mut result = Finding::blocker(code, Capability::MappingValidation).for_record(record);
    result.field_path = Some(field.into());
    result
}

fn source_id(value: &Value) -> Option<String> {
    let value = value.get("id").unwrap_or(value);
    match value {
        Value::String(id) if !id.is_empty() && id.trim() == id => Some(id.clone()),
        Value::Number(id) => id.as_u64().filter(|id| *id > 0).map(|id| id.to_string()),
        _ => None,
    }
}

fn provider_matches(value: &Value, provider: Uuid) -> bool {
    ["source_provider_id", "provider_id"]
        .into_iter()
        .all(|name| {
            value.get(name).is_none_or(|value| {
                value.as_str().and_then(|id| Uuid::parse_str(id).ok()) == Some(provider)
            })
        })
}

fn side(value: &Value) -> Option<InterfaceCableEnd> {
    serde_json::from_value(value.clone()).ok()
}

fn validate_networking(record: &SourceRecord) -> bool {
    use crate::contexts::inventory::domain::dcim::interface::InterfaceRfMeasurement;

    // Use the native enum vocabulary and range validator, rather than accepting
    // arbitrary strings merely because the native storage column is text.
    let mut values = serde_json::Map::new();
    for &name in NETWORKING {
        if let Some(raw) = record.fields.get(name) {
            let mut value = raw.get("value").unwrap_or(raw).clone();
            if name == "speed" && value.is_number() {
                value = Value::String(value.to_string());
            }
            let measurement = match name {
                "rf_channel_frequency" => Some(InterfaceRfMeasurement::Frequency),
                "rf_channel_width" => Some(InterfaceRfMeasurement::Width),
                _ => None,
            };
            if let Some(measurement) = measurement.filter(|_| !value.is_null()) {
                match measurement.validate_source(&value) {
                    Ok(validated) => value = serde_json::json!(validated),
                    Err(_) => return false,
                }
            }
            values.insert(name.into(), value);
        }
    }
    serde_json::from_value::<InterfaceNetworking>(Value::Object(values))
        .is_ok_and(|networking| networking.validate().is_ok())
}

fn typed_source_id(value: &Value, token: &str, provider: Uuid) -> Option<String> {
    if !provider_matches(value, provider)
        || value.get("object_type").is_some_and(|kind| kind != token)
    {
        return None;
    }
    source_id(value)
}

fn claims_attachment(row: &PreparedRow, cable: &SourceIdentity, end: &InterfaceCableEnd) -> bool {
    row.references.iter().any(|(field, binding)| {
        field.name() == "cable"
            && binding.target == Some(InventoryEntity::Cable)
            && binding.external_id == cable.external_id
    }) && row.scalars.iter().any(|(field, value)| {
        field.name() == "cable_end"
            && matches!(value, InventoryValue::Text { value }
                if side(&Value::String(value.clone())).as_ref() == Some(end))
    })
}

/// NetBox's endpoint set_cable_termination copies these values from the relation.
/// Retain the authoritative source row in the sealed census, but write
/// only that same native Interface representation. No relation table is added.
fn project_terminations(
    snapshot: &SourceSnapshot,
    provider: Uuid,
    rows: &mut [PreparedRow],
) -> (Vec<Finding>, bool, Vec<SideAssertion>) {
    let mut findings = Vec::new();
    let mut complete = true;
    let mut groups = BTreeMap::<String, (SourceIdentity, [Vec<SourceIdentity>; 2])>::new();
    let mut endpoints = HashSet::new();
    let mut connectors = HashSet::new();
    for record in snapshot.records.iter().filter(|record| {
        record.identity.entity.as_str() == InventoryEntity::CableTermination.source_key()
    }) {
        let before = findings.len();
        for name in [
            "cable",
            "cable_end",
            "termination",
            "termination_type",
            "connector",
            "positions",
        ] {
            if !record.fields.contains_key(name) {
                findings.push(finding(record, name, FindingCode::FieldValueInvalid));
            }
        }
        if findings.len() != before {
            continue;
        }
        let cable_id = typed_source_id(&record.fields["cable"], "dcim.cable", provider);
        let endpoint_id =
            typed_source_id(&record.fields["termination"], "dcim.interface", provider);
        let end = side(&record.fields["cable_end"]);
        if record.identity.source_provider_id != provider
            || record.fields["termination_type"] != "dcim.interface"
            || cable_id.is_none()
            || endpoint_id.is_none()
            || end.is_none()
        {
            findings.push(finding(
                record,
                "termination",
                FindingCode::FieldValueInvalid,
            ));
            continue;
        }
        let cable_id = cable_id.expect("checked cable reference");
        let endpoint_id = endpoint_id.expect("checked endpoint reference");
        let end = end.expect("checked cable side");
        if !endpoints.insert(endpoint_id.clone()) {
            findings.push(finding(
                record,
                "termination",
                FindingCode::FieldValueInvalid,
            ));
        }
        let cable_records: Vec<_> = snapshot
            .records
            .iter()
            .filter(|candidate| {
                candidate.identity.source_provider_id == provider
                    && candidate.identity.entity.as_str() == InventoryEntity::Cable.source_key()
                    && candidate.identity.external_id == cable_id
            })
            .collect();
        let interfaces: Vec<_> = snapshot
            .records
            .iter()
            .filter(|candidate| {
                candidate.identity.source_provider_id == provider
                    && candidate.identity.entity.as_str() == InventoryEntity::Interface.source_key()
                    && candidate.identity.external_id == endpoint_id
            })
            .collect();
        if cable_records.len() != 1 || interfaces.len() != 1 {
            complete = false;
            findings.push(finding(
                record,
                "termination",
                FindingCode::AnalysisDependencyUnavailable,
            ));
            continue;
        }
        let cable = &cable_records[0].identity;
        let interface = interfaces[0];
        let group = groups
            .entry(cable_id.clone())
            .or_insert_with(|| (cable.clone(), [Vec::new(), Vec::new()]));
        let side_index = match end {
            InterfaceCableEnd::A => 0,
            InterfaceCableEnd::B => 1,
        };
        group.1[side_index].push(interface.identity.clone());
        if interface
            .fields
            .get("cable")
            .and_then(|value| typed_source_id(value, "dcim.cable", provider))
            .as_deref()
            != Some(cable_id.as_str())
            || interface.fields.get("cable_end").and_then(side) != Some(end.clone())
        {
            findings.push(finding(record, "cable_end", FindingCode::FieldValueInvalid));
        }
        let Some(row) = rows
            .iter_mut()
            .find(|row| row.entity == InventoryEntity::Interface && row.external_id == endpoint_id)
        else {
            complete = false;
            findings.push(finding(
                record,
                "termination",
                FindingCode::AnalysisDependencyUnavailable,
            ));
            continue;
        };
        if !claims_attachment(row, cable, &end) {
            findings.push(finding(record, "cable_end", FindingCode::FieldValueInvalid));
        }
        for (source, target) in [
            ("connector", "cable_connector"),
            ("positions", "cable_positions"),
        ] {
            let raw = &record.fields[source];
            let field = InventoryEntity::Interface
                .field(target)
                .expect("canonical attachment field");
            let value = if raw.is_null() {
                Ok(InventoryValue::Null)
            } else {
                convert_scalar(field, raw)
            };
            let Ok(value) = value else {
                findings.push(finding(record, source, FindingCode::FieldValueInvalid));
                continue;
            };
            if interface
                .fields
                .get(target)
                .is_some_and(|observed| observed != raw)
                || row
                    .scalars
                    .get(&field)
                    .is_some_and(|mapped| mapped != &value)
            {
                findings.push(finding(record, source, FindingCode::FieldValueInvalid));
                continue;
            }
            if source == "connector"
                && !raw.is_null()
                && !connectors.insert((cable_id.clone(), side_index, raw.as_i64()))
            {
                findings.push(finding(record, source, FindingCode::FieldValueInvalid));
            }
            row.scalars.insert(field, value);
        }
    }
    let mut assertions = Vec::new();
    for (_, (cable, sides)) in groups {
        for (end, interfaces) in [InterfaceCableEnd::A, InterfaceCableEnd::B]
            .into_iter()
            .zip(sides)
        {
            // The authoritative relation census is complete for each selected
            // Cable, including an empty opposite side. It cannot hide a raw or
            // transformed Interface attachment without a matching source row.
            for interface in snapshot.records.iter().filter(|record| {
                record.identity.source_provider_id == provider
                    && record.identity.entity.as_str() == InventoryEntity::Interface.source_key()
            }) {
                if interface.fields.get("cable").and_then(source_id).as_deref()
                    == Some(cable.external_id.as_str())
                    && interface.fields.get("cable_end").and_then(side) == Some(end.clone())
                    && !interfaces
                        .iter()
                        .any(|declared| declared == &interface.identity)
                {
                    findings.push(finding(interface, "cable", FindingCode::FieldValueInvalid));
                }
            }
            for row in rows
                .iter()
                .filter(|row| row.entity == InventoryEntity::Interface)
            {
                if claims_attachment(row, &cable, &end)
                    && !interfaces
                        .iter()
                        .any(|declared| declared.external_id == row.external_id)
                {
                    findings.push(prepared_finding(
                        row,
                        FindingCode::FieldValueInvalid,
                        Some("cable"),
                        Capability::MappingValidation,
                    ));
                }
            }
            assertions.push(SideAssertion {
                cable: cable.clone(),
                end,
                interfaces,
            });
        }
    }
    (findings, complete, assertions)
}

/// Full Interface rows own endpoint writes. Reverse sides require those rows
/// with explicit cable/end fields; reverse-only partial snapshots fail closed.
/// Omitted sides impose no reverse assertion. An explicit empty side asserts
/// that no supplied Interface claims it; it never clears an unseen native row.
pub(super) fn assess(
    snapshot: &SourceSnapshot,
    provider: Uuid,
    rows: &mut [PreparedRow],
) -> (Vec<Finding>, bool, Vec<SideAssertion>) {
    let (mut findings, mut complete, mut assertions) =
        project_terminations(snapshot, provider, rows);
    let interfaces: Vec<_> = snapshot
        .records
        .iter()
        .filter(|record| record.identity.entity.as_str() == InventoryEntity::Interface.source_key())
        .collect();
    let mut endpoints = HashSet::new();
    for interface in &interfaces {
        if !validate_networking(interface) {
            findings.push(finding(
                interface,
                "networking",
                FindingCode::FieldValueInvalid,
            ));
        }
        let cable = interface.fields.get("cable");
        let end = interface.fields.get("cable_end");
        // Clearing one half while preserving the other is ambiguous. Both may
        // be omitted, but an explicit attachment/clear must specify the pair.
        if cable.is_some() != end.is_some()
            || cable.zip(end).is_some_and(|(cable, end)| {
                cable.is_null() != end.is_null()
                    || (!cable.is_null()
                        && (source_id(cable).is_none()
                            || side(end).is_none()
                            || !provider_matches(cable, provider)))
            })
        {
            findings.push(finding(
                interface,
                "cable_end",
                FindingCode::FieldValueInvalid,
            ));
        }
        if let Some(row) = rows.iter().find(|row| {
            row.entity == InventoryEntity::Interface
                && row.external_id == interface.identity.external_id
        }) {
            let attached = row
                .references
                .iter()
                .any(|(field, _)| field.name() == "cable");
            let cleared = row.cleared.iter().any(|field| field.name() == "cable");
            let end = row
                .scalars
                .iter()
                .find(|(field, _)| field.name() == "cable_end");
            let valid = match (attached, cleared, end) {
                (false, false, None) => true,
                (true, false, Some((_, InventoryValue::Text { value }))) => {
                    side(&Value::String(value.clone())).is_some()
                }
                (false, true, Some((_, InventoryValue::Null))) => true,
                _ => false,
            };
            if !valid {
                findings.push(finding(
                    interface,
                    "cable_end",
                    FindingCode::FieldValueInvalid,
                ));
            }
        }
    }
    for cable in snapshot
        .records
        .iter()
        .filter(|record| record.identity.entity.as_str() == InventoryEntity::Cable.source_key())
    {
        for (name, expected_side) in [
            ("a_terminations", InterfaceCableEnd::A),
            ("b_terminations", InterfaceCableEnd::B),
        ] {
            let Some(raw) = cable.fields.get(name) else {
                continue;
            };
            if interfaces.is_empty() {
                complete = false;
                findings.push(finding(
                    cable,
                    name,
                    FindingCode::AnalysisDependencyUnavailable,
                ));
            }
            let Some(entries) = raw.as_array() else {
                findings.push(finding(cable, name, FindingCode::FieldValueInvalid));
                continue;
            };
            let mut declared = HashSet::new();
            for entry in entries {
                let id = entry
                    .get("object_id")
                    .filter(|id| !id.is_object())
                    .and_then(source_id);
                let object = entry.get("object");
                let valid = entry.get("object_type").and_then(Value::as_str)
                    == Some("dcim.interface")
                    && provider_matches(entry, provider)
                    && object.is_some_and(|object| {
                        object.is_object()
                            && provider_matches(object, provider)
                            && object
                                .get("object_type")
                                .is_none_or(|kind| kind == "dcim.interface")
                            && object.get("id").and_then(source_id) == id
                            && object.get("cable").is_none_or(|reference| {
                                source_id(reference).as_deref()
                                    == Some(cable.identity.external_id.as_str())
                                    && provider_matches(reference, provider)
                            })
                            && object
                                .get("cable_end")
                                .is_none_or(|end| side(end) == Some(expected_side.clone()))
                    });
                let Some(id) = id.filter(|_| valid) else {
                    findings.push(finding(cable, name, FindingCode::FieldValueInvalid));
                    continue;
                };
                if !declared.insert(id.clone())
                    || !endpoints.insert((cable.identity.source_provider_id, id.clone()))
                {
                    findings.push(finding(cable, name, FindingCode::FieldValueInvalid));
                }
                let matches: Vec<_> = interfaces
                    .iter()
                    .filter(|interface| {
                        interface.identity.external_id == id
                            && interface.identity.source_provider_id
                                == cable.identity.source_provider_id
                            && interface.identity.source_provider_id == provider
                    })
                    .collect();
                if matches.len() != 1 {
                    complete = false;
                    findings.push(finding(
                        cable,
                        name,
                        FindingCode::AnalysisDependencyUnavailable,
                    ));
                    continue;
                }
                let interface = matches[0];
                if !interface.fields.contains_key("cable")
                    || !interface.fields.contains_key("cable_end")
                {
                    complete = false;
                    findings.push(finding(
                        cable,
                        name,
                        FindingCode::AnalysisDependencyUnavailable,
                    ));
                } else if interface.fields.get("cable").and_then(source_id).as_deref()
                    != Some(cable.identity.external_id.as_str())
                    || interface.fields.get("cable_end").and_then(side)
                        != Some(expected_side.clone())
                {
                    findings.push(finding(cable, name, FindingCode::FieldValueInvalid));
                }
                // A mapping expression may transform the forward observation.
                // Only the canonical prepared Interface writer may satisfy the
                // reverse claim; raw agreement alone must not hide a remap.
                if let Some(row) = rows
                    .iter()
                    .find(|row| row.entity == InventoryEntity::Interface && row.external_id == id)
                {
                    let reference = row
                        .references
                        .iter()
                        .find(|(field, _)| field.name() == "cable");
                    let end = row
                        .scalars
                        .iter()
                        .find(|(field, _)| field.name() == "cable_end");
                    if !reference.is_some_and(|(_, binding)| binding.target == Some(InventoryEntity::Cable) && binding.external_id == cable.identity.external_id)
                        || !end.is_some_and(|(_, value)| matches!(value, InventoryValue::Text { value } if side(&Value::String(value.clone())) == Some(expected_side.clone())))
                    {
                        findings.push(finding(cable, name, FindingCode::FieldValueInvalid));
                    }
                }
            }
            for interface in &interfaces {
                if interface.identity.source_provider_id == cable.identity.source_provider_id
                    && interface.fields.get("cable").and_then(source_id).as_deref()
                        == Some(cable.identity.external_id.as_str())
                    && interface.fields.get("cable_end").and_then(side)
                        == Some(expected_side.clone())
                    && !declared.contains(&interface.identity.external_id)
                {
                    findings.push(finding(cable, name, FindingCode::FieldValueInvalid));
                }
            }
            for row in rows
                .iter()
                .filter(|row| row.entity == InventoryEntity::Interface)
            {
                let claims_cable = row.references.iter().any(|(field, binding)| {
                    field.name() == "cable"
                        && binding.target == Some(InventoryEntity::Cable)
                        && binding.external_id == cable.identity.external_id
                });
                let claims_side = row.scalars.iter().any(|(field, value)| {
                    field.name() == "cable_end"
                        && matches!(value, InventoryValue::Text { value }
                        if side(&Value::String(value.clone())) == Some(expected_side.clone()))
                });
                if claims_cable && claims_side && !declared.contains(&row.external_id) {
                    findings.push(finding(cable, name, FindingCode::FieldValueInvalid));
                }
            }
            assertions.push(SideAssertion {
                cable: cable.identity.clone(),
                end: expected_side,
                interfaces: interfaces
                    .iter()
                    .filter(|interface| {
                        interface.identity.source_provider_id == cable.identity.source_provider_id
                            && declared.contains(&interface.identity.external_id)
                    })
                    .map(|interface| interface.identity.clone())
                    .collect(),
            });
        }
    }
    (findings, complete, assertions)
}
