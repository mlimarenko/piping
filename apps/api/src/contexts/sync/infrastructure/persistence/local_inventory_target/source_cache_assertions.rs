//! Assertions for NetBox's denormalized component owner references.
//!
//! These assertions do not resolve or write bindings. The caller must resolve
//! the canonical source graph and separately ensure its native projection is
//! retained before admitting a cache field.

use serde_json::Value;
use uuid::Uuid;

use super::PreparedRow;
use crate::contexts::inventory::domain::decimal::scaled_decimal;
use crate::contexts::sync::domain::inventory_preflight::{Capability, Finding, FindingCode};
use crate::contexts::sync::domain::inventory_sync::InventoryValue;
use crate::contexts::sync::domain::inventory_target_schema::{
    InventoryEntity as E, SourceCacheKind,
};
use crate::contexts::sync::domain::{SourceEntity, SourceIdentity, SourceRecord, SourceSnapshot};

fn row<'a>(rows: &'a [PreparedRow], entity: E, external_id: &str) -> Result<&'a PreparedRow, ()> {
    let mut matches = rows
        .iter()
        .filter(|row| row.entity == entity && row.external_id == external_id);
    let value = matches.next().ok_or(())?;
    if matches.next().is_some() {
        return Err(());
    }
    Ok(value)
}

fn relation(
    row: &PreparedRow,
    name: &str,
    target: E,
    provider: Uuid,
) -> Result<Option<SourceIdentity>, ()> {
    if row.cleared.iter().any(|field| field.name() == name) {
        return Ok(None);
    }
    let binding = row
        .references
        .iter()
        .find(|(field, _)| field.name() == name)
        .ok_or(())?;
    if binding.1.target != Some(target) {
        return Err(());
    }
    SourceIdentity::new(
        provider,
        SourceEntity::new(target.source_key()).map_err(|_| ())?,
        &binding.1.external_id,
    )
    .map(Some)
    .map_err(|_| ())
}

fn component<'a>(
    record: &SourceRecord,
    entity: E,
    rows: &'a [PreparedRow],
) -> Result<&'a PreparedRow, ()> {
    if entity != E::CableTermination {
        return row(rows, entity, &record.identity.external_id);
    }
    if record
        .fields
        .get("termination_type")
        .and_then(Value::as_str)
        != Some("dcim.interface")
    {
        return Err(());
    }
    let id = record
        .fields
        .get("termination")
        .and_then(super::external_id_of)
        .ok_or(())?;
    row(rows, E::Interface, &id)
}

fn device(
    record: &SourceRecord,
    entity: E,
    rows: &[PreparedRow],
    provider: Uuid,
) -> Result<SourceIdentity, ()> {
    relation(
        component(record, entity, rows)?,
        "device",
        E::Device,
        provider,
    )?
    .ok_or(())
}

fn scalar(row: &PreparedRow, name: &str) -> Option<Value> {
    let field = row.entity.field(name)?;
    Some(match row.scalars.get(&field)? {
        InventoryValue::Null => Value::Null,
        InventoryValue::Boolean { value } => Value::Bool(*value),
        InventoryValue::Integer { value } => serde_json::json!(value),
        InventoryValue::Number { value } => serde_json::json!(value),
        InventoryValue::Text { value } => Value::String(value.clone()),
        InventoryValue::Json { value } => value.clone(),
    })
}

fn path(record: &SourceRecord, rows: &[PreparedRow], snapshot: &SourceSnapshot) -> Assertion {
    let raw = &record.fields["_path"];
    // CablePath's database identity is a transient trace cache, not an imported
    // endpoint or cable identity. Native trace_endpoint follows cable/mapping
    // edges directly; retain and validate those edges instead of rebinding it.
    if raw.is_null() {
        return Assertion::Matches;
    }
    if crate::contexts::sync::domain::inventory_sync::normalize_netbox_id(raw).is_err() {
        return Assertion::Invalid;
    }
    let Ok(interface) = row(rows, E::Interface, &record.identity.external_id) else {
        return Assertion::DependencyUnavailable;
    };
    let provider = record.identity.source_provider_id;
    let Ok(Some(cable)) = relation(interface, "cable", E::Cable, provider) else {
        return Assertion::DependencyUnavailable;
    };
    if row(rows, E::Cable, &cable.external_id).is_err() {
        return Assertion::DependencyUnavailable;
    }
    let mut endpoints = std::collections::BTreeMap::new();
    for termination in snapshot
        .records
        .iter()
        .filter(|candidate| candidate.identity.entity.as_str() == E::CableTermination.source_key())
    {
        if termination.identity.source_provider_id != provider {
            return Assertion::DependencyUnavailable;
        }
        let Some(raw_cable) = termination.fields.get("cable") else {
            return Assertion::DependencyUnavailable;
        };
        if raw_cable
            .get("object_type")
            .is_some_and(|value| value != "dcim.cable")
        {
            return Assertion::Invalid;
        }
        let Some(termination_cable) = super::external_id_of(raw_cable) else {
            return Assertion::DependencyUnavailable;
        };
        if termination_cable != cable.external_id {
            continue;
        }
        let Some(side @ ("A" | "B")) = termination.fields.get("cable_end").and_then(Value::as_str)
        else {
            return Assertion::Invalid;
        };
        let Ok(endpoint) = component(termination, E::CableTermination, rows) else {
            return Assertion::DependencyUnavailable;
        };
        if relation(endpoint, "cable", E::Cable, provider)
            .ok()
            .flatten()
            .as_ref()
            != Some(&cable)
            || scalar(endpoint, "cable_end")
                .as_ref()
                .and_then(Value::as_str)
                != Some(side)
        {
            return Assertion::Invalid;
        }
        if endpoints
            .insert(endpoint.external_id.clone(), side)
            .is_some()
        {
            return Assertion::Invalid;
        }
    }
    if !endpoints.contains_key(&interface.external_id) {
        return Assertion::DependencyUnavailable;
    }
    // A complete source census may have an open opposite end. Its emptiness is
    // meaningful only if neither raw nor mapped Interface attachments claim an
    // endpoint absent from the termination census. The graph owner separately
    // checks positions, connectors and the persisted opposite-side fence.
    for source in snapshot.records.iter().filter(|candidate| {
        candidate.identity.source_provider_id == provider
            && candidate.identity.entity.as_str() == E::Interface.source_key()
    }) {
        let Some(raw_cable) = source.fields.get("cable") else {
            continue;
        };
        if super::external_id_of(raw_cable).as_deref() != Some(cable.external_id.as_str()) {
            continue;
        }
        let Some(side @ ("A" | "B")) = source.fields.get("cable_end").and_then(Value::as_str)
        else {
            return Assertion::Invalid;
        };
        if endpoints.get(&source.identity.external_id).copied() != Some(side) {
            return Assertion::DependencyUnavailable;
        }
    }
    for endpoint in rows.iter().filter(|row| row.entity == E::Interface) {
        if relation(endpoint, "cable", E::Cable, provider)
            .ok()
            .flatten()
            .as_ref()
            != Some(&cable)
        {
            continue;
        }
        let mapped_end = scalar(endpoint, "cable_end");
        let Some(side @ ("A" | "B")) = mapped_end.as_ref().and_then(Value::as_str) else {
            return Assertion::Invalid;
        };
        if endpoints.get(&endpoint.external_id).copied() != Some(side) {
            return Assertion::DependencyUnavailable;
        }
    }
    Assertion::Matches
}

/// Validate source caches against the *mapped* graph, so a transform cannot
/// preserve the assertion while changing the native quantity or owner beneath it.
pub(super) fn assess(
    snapshot: &SourceSnapshot,
    rows: &[PreparedRow],
    provider: Uuid,
) -> (Vec<Finding>, bool) {
    let mut findings = Vec::new();
    let mut complete = true;
    for record in &snapshot.records {
        let Some(entity) = E::from_source_key(record.identity.entity.as_str()) else {
            continue;
        };
        for (name, raw) in &record.fields {
            let Some(kind) = entity.source_cache_kind(name) else {
                continue;
            };
            let result = match kind {
                SourceCacheKind::DeviceScope { field, target } => {
                    let expected = device(record, entity, rows, provider).and_then(|owner| {
                        relation(
                            row(rows, E::Device, &owner.external_id)?,
                            field,
                            target,
                            provider,
                        )
                    });
                    let (app, model) = target.content_type().expect("cache target metadata");
                    let token = format!("{app}.{model}");
                    assert_reference(
                        Some(raw),
                        match &expected {
                            Ok(value) => Relation::Known(value.as_ref()),
                            Err(()) => Relation::Unavailable,
                        },
                        provider,
                        target.source_key(),
                        &token,
                    )
                }
                SourceCacheKind::TerminationDevice => {
                    let expected = device(record, entity, rows, provider);
                    assert_reference(
                        Some(raw),
                        match &expected {
                            Ok(value) => Relation::Known(Some(value)),
                            Err(()) => Relation::Unavailable,
                        },
                        provider,
                        E::Device.source_key(),
                        "dcim.device",
                    )
                }
                SourceCacheKind::CablePath => path(record, rows, snapshot),
                _ => {
                    let (quantity, value_name, unit_name) = match kind {
                        SourceCacheKind::AbsoluteLength => {
                            (Quantity::Length, "length", "length_unit")
                        }
                        SourceCacheKind::AbsoluteWeight => {
                            (Quantity::Weight, "weight", "weight_unit")
                        }
                        SourceCacheKind::AbsoluteFlow => {
                            (Quantity::MaxFlow, "max_flow", "max_flow_unit")
                        }
                        SourceCacheKind::AbsoluteDiameter => {
                            (Quantity::Diameter, "diameter", "diameter_unit")
                        }
                        _ => unreachable!(),
                    };
                    let prepared = row(rows, entity, &record.identity.external_id).ok();
                    let value = prepared.and_then(|row| scalar(row, value_name));
                    let unit = prepared.and_then(|row| scalar(row, unit_name));
                    assert_quantity(quantity, Some(raw), value.as_ref(), unit.as_ref())
                }
            };
            let code = match result {
                Assertion::Omitted | Assertion::Matches => continue,
                Assertion::Invalid => FindingCode::FieldValueInvalid,
                Assertion::DependencyUnavailable => {
                    complete = false;
                    FindingCode::AnalysisDependencyUnavailable
                }
            };
            let mut finding =
                Finding::blocker(code, Capability::MappingValidation).for_record(record);
            finding.field_path = Some(name.clone());
            findings.push(finding);
        }
    }
    (findings, complete)
}

/// Omission is distinct from an explicit null, even when the owner is unknown.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Assertion {
    Omitted,
    Matches,
    Invalid,
    DependencyUnavailable,
}

/// A resolved canonical relation, or an unavailable graph dependency.
pub(super) enum Relation<'a> {
    Known(Option<&'a SourceIdentity>),
    Unavailable,
}

/// Source measurement whose cache is asserted. Input is a NetBox decimal(8,2).
#[derive(Debug, Clone, Copy)]
pub(super) enum Quantity {
    Length,
    Weight,
    MaxFlow,
    Diameter,
}

fn decimal(value: &Value, scale: u32) -> Option<i64> {
    match value {
        Value::String(value) => scaled_decimal(value, scale),
        Value::Number(value) => scaled_decimal(&value.to_string(), scale),
        _ => None,
    }
}

// Conversion factors are exact rationals. NetBox uses Decimal(float) for the
// imperial constants below, not Decimal(string); these are the exact
// IEEE-754 binary64 values of those literals. The bounded decimal(8,2) input
// keeps every intermediate within i128 and makes Decimal's 28-digit context
// irrelevant to the final four-place rounding or integer truncation.
fn factor(quantity: Quantity, unit: &str) -> Option<(i128, i128)> {
    Some(match (quantity, unit) {
        (Quantity::Length, "km") => (1000, 1),
        (Quantity::Length, "m") => (1, 1),
        (Quantity::Length, "cm") => (1, 100),
        (Quantity::Length, "mi") => (1_769_492_441_091_539, 1_i128 << 40),
        (Quantity::Length, "ft") => (5_490_788_665_690_109, 1_i128 << 54),
        (Quantity::Length, "in") => (3_660_525_777_126_739, 1_i128 << 57),
        (Quantity::Weight, "kg") => (1000, 1),
        (Quantity::Weight, "g") => (1, 1),
        (Quantity::Weight, "lb") => (3_989_837_426_129_371, 1_i128 << 43),
        (Quantity::Weight, "oz") => (3_989_837_426_129_371, 1_i128 << 47),
        (Quantity::Diameter, "mm") => (1, 1),
        (Quantity::Diameter, "cm") => (10, 1),
        (Quantity::Diameter, "in") => (254, 10),
        (Quantity::MaxFlow, "lpm") => (1, 1),
        (Quantity::MaxFlow, "m3ph") => (50, 3),
        (Quantity::MaxFlow, "gpm") => (3_785_411_784, 1_000_000_000),
        _ => return None,
    })
}

/// Compare the original source cache with its source quantity and unit.
/// Missing cache is not a claim; missing quantity/unit is an unavailable
/// dependency, rather than an implicit null. A non-null cache must be an exact
/// nonnegative decimal representable at the source cache's scale.
pub(super) fn assert_quantity(
    quantity: Quantity,
    cache: Option<&Value>,
    value: Option<&Value>,
    unit: Option<&Value>,
) -> Assertion {
    let Some(cache) = cache else {
        return Assertion::Omitted;
    };
    let Some(value) = value else {
        return Assertion::DependencyUnavailable;
    };
    let amount = if value.is_null() {
        None
    } else {
        match decimal(value, 2) {
            Some(amount @ 0..=99_999_999) => Some(amount),
            _ => return Assertion::Invalid,
        }
    };
    // WeightMixin uses truthiness, unlike the other normalization mixins.
    if amount.is_none() || (matches!(quantity, Quantity::Weight) && amount == Some(0)) {
        return if cache.is_null() {
            Assertion::Matches
        } else {
            Assertion::Invalid
        };
    }
    let Some(unit) = unit else {
        return Assertion::DependencyUnavailable;
    };
    if unit.is_null() || unit.as_str() == Some("") {
        return if cache.is_null() {
            Assertion::Matches
        } else {
            Assertion::Invalid
        };
    }
    let Some((numerator, denominator)) = unit.as_str().and_then(|unit| factor(quantity, unit))
    else {
        return Assertion::Invalid;
    };
    let weight = matches!(quantity, Quantity::Weight);
    let scale = if weight { 0 } else { 4 };
    let Some(claim) = decimal(cache, scale).filter(|claim| *claim >= 0) else {
        return Assertion::Invalid;
    };
    let product =
        i128::from(amount.expect("non-null amount")) * numerator * if weight { 1 } else { 10_000 };
    let divisor = denominator * 100;
    let quotient = product / divisor;
    let remainder = product % divisor;
    // Python Decimal round uses ROUND_HALF_EVEN. Weight's int() truncates.
    let expected = quotient
        + i128::from(
            !weight && (remainder * 2 > divisor || (remainder * 2 == divisor && quotient % 2 != 0)),
        );
    if i128::from(claim) == expected {
        Assertion::Matches
    } else {
        Assertion::Invalid
    }
}

/// Validate a raw cache against a source-qualified canonical relation.
/// `expected_entity` is the collection key; `expected_type` is the NetBox
/// object-type token. Never compare an unqualified numeric ID across types.
pub(super) fn assert_reference(
    raw: Option<&Value>,
    relation: Relation<'_>,
    provider: Uuid,
    expected_entity: &str,
    expected_type: &str,
) -> Assertion {
    let Some(raw) = raw else {
        return Assertion::Omitted;
    };
    let Relation::Known(expected) = relation else {
        return Assertion::DependencyUnavailable;
    };
    let Some(expected) = expected else {
        return if raw.is_null() {
            Assertion::Matches
        } else {
            Assertion::Invalid
        };
    };
    if expected.source_provider_id != provider || expected.entity.as_str() != expected_entity {
        return Assertion::Invalid;
    }
    if raw.is_null() {
        return Assertion::Invalid;
    }
    if let Value::Object(object) = raw {
        for name in ["source_provider_id", "provider_id"] {
            if object.get(name).is_some_and(|value| {
                value.as_str().and_then(|text| Uuid::parse_str(text).ok()) != Some(provider)
            }) {
                return Assertion::Invalid;
            }
        }
        if object
            .get("object_type")
            .is_some_and(|value| value.as_str() != Some(expected_type))
        {
            return Assertion::Invalid;
        }
    }
    let id = raw.get("id").unwrap_or(raw);
    let id = match id {
        Value::Number(value) => value
            .as_u64()
            .filter(|value| *value > 0)
            .map(|value| value.to_string()),
        Value::String(value) if !value.is_empty() && value.trim() == value => Some(value.clone()),
        _ => None,
    };
    if id.as_deref() == Some(expected.external_id.as_str()) {
        Assertion::Matches
    } else {
        Assertion::Invalid
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contexts::sync::domain::SourceEntity;
    use serde_json::json;

    fn graph() -> (SourceSnapshot, Vec<PreparedRow>, Uuid) {
        let provider = Uuid::from_u128(81);
        let records = [
            (E::Device, "1", json!({"site": 7, "rack": 8, "location": null})),
            (E::Interface, "2", json!({"device": 1, "cable": 9, "cable_end": "A", "_site": 7, "_rack": 8, "_location": null, "_path": 100})),
            (E::Interface, "3", json!({"device": 1, "cable": 9, "cable_end": "B", "_site": 7, "_rack": 8, "_location": null, "_path": 200})),
            (E::Cable, "9", json!({"label": "cache proof", "length": "12.50", "length_unit": "m", "_abs_length": "12.5000"})),
            (E::CableTermination, "4", json!({"cable": 9, "cable_end": "A", "termination_type": "dcim.interface", "termination": 2, "_device": 1, "_site": 7, "_rack": 8, "_location": null})),
            (E::CableTermination, "5", json!({"cable": 9, "cable_end": "B", "termination_type": "dcim.interface", "termination": 3, "_device": 1, "_site": 7, "_rack": 8, "_location": null})),
        ].into_iter().map(|(entity, id, fields)| SourceRecord::new(SourceIdentity::new(provider, SourceEntity::new(entity.source_key()).unwrap(), id).unwrap(), fields.as_object().unwrap().clone())).collect::<Vec<_>>();
        let rows = records
            .iter()
            .filter_map(|record| {
                let entity = E::from_source_key(record.identity.entity.as_str()).unwrap();
                if entity.is_membership() {
                    return None;
                }
                let mappings = record
                    .fields
                    .keys()
                    .filter(|name| entity.field(name).is_some())
                    .map(|name| super::super::tests::identity(name, name))
                    .collect::<Vec<_>>();
                let mappings = mappings.iter().collect::<Vec<_>>();
                Some(super::super::prepare_row(entity, provider, record, &mappings).unwrap())
            })
            .collect();
        (SourceSnapshot { records }, rows, provider)
    }

    #[test]
    fn canonical_graph_reconstructs_scope_quantity_and_transient_paths() {
        let (snapshot, rows, provider) = graph();
        let (findings, complete) = assess(&snapshot, &rows, provider);
        assert!(complete);
        assert!(findings.is_empty());
        // Removing only optional cache observations leaves the retained topology
        // unchanged. Different transient path IDs above are never rebound.
        let mut omitted = snapshot.clone();
        for record in &mut omitted.records {
            record.fields.retain(|name, _| !name.starts_with('_'));
        }
        assert!(assess(&omitted, &rows, provider).0.is_empty());
    }

    #[test]
    fn open_cable_path_accepts_complete_single_ended_census() {
        let (mut snapshot, mut rows, provider) = graph();
        snapshot.records.retain(|record| {
            !(record.identity.entity.as_str() == E::Interface.source_key()
                && record.identity.external_id == "3")
                && !(record.identity.entity.as_str() == E::CableTermination.source_key()
                    && record.identity.external_id == "5")
        });
        rows.retain(|row| !(row.entity == E::Interface && row.external_id == "3"));
        let (findings, complete) = assess(&snapshot, &rows, provider);
        assert!(complete);
        assert!(findings.is_empty());

        let mut absent_origin = snapshot.clone();
        absent_origin
            .records
            .retain(|record| record.identity.entity.as_str() != E::CableTermination.source_key());
        let (findings, complete) = assess(&absent_origin, &rows, provider);
        assert!(!complete);
        assert!(
            findings
                .iter()
                .any(|finding| finding.field_path.as_deref() == Some("_path"))
        );

        // An observed opposite endpoint whose termination was omitted does not
        // prove an empty side, even though the origin termination is present.
        let (mut incomplete, original_rows, provider) = graph();
        incomplete.records.retain(|record| {
            !(record.identity.entity.as_str() == E::CableTermination.source_key()
                && record.identity.external_id == "5")
        });
        let (findings, complete) = assess(&incomplete, &original_rows, provider);
        assert!(!complete);
        assert!(
            findings
                .iter()
                .any(|finding| finding.field_path.as_deref() == Some("_path"))
        );

        let mut duplicate = snapshot.clone();
        let mut repeated = duplicate
            .records
            .iter()
            .find(|record| record.identity.entity.as_str() == E::CableTermination.source_key())
            .unwrap()
            .clone();
        repeated.identity.external_id = "6".into();
        duplicate.records.push(repeated);
        assert!(
            assess(&duplicate, &rows, provider)
                .0
                .iter()
                .any(|finding| finding.code == FindingCode::FieldValueInvalid
                    && finding.field_path.as_deref() == Some("_path"))
        );
    }

    #[test]
    fn cache_claims_refuse_wrong_qualifiers_nulls_transforms_and_missing_topology() {
        let (snapshot, mut rows, provider) = graph();
        for (index, name, bad) in [
            (1, "_site", json!({"id":7,"object_type":"dcim.rack"})),
            (1, "_rack", Value::Null),
            (4, "_device", json!(2)),
            (3, "_abs_length", json!("12.5001")),
        ] {
            let mut bad_snapshot = snapshot.clone();
            bad_snapshot.records[index].fields.insert(name.into(), bad);
            assert!(
                assess(&bad_snapshot, &rows, provider)
                    .0
                    .iter()
                    .any(|finding| finding.code == FindingCode::FieldValueInvalid
                        && finding.field_path.as_deref() == Some(name))
            );
        }
        let device = rows.iter_mut().find(|row| row.entity == E::Device).unwrap();
        device
            .references
            .iter_mut()
            .find(|(field, _)| field.name() == "site")
            .unwrap()
            .1
            .external_id = "99".into();
        assert!(
            assess(&snapshot, &rows, provider)
                .0
                .iter()
                .any(|finding| finding.field_path.as_deref() == Some("_site"))
        );
        let (mut snapshot, rows, provider) = graph();
        snapshot
            .records
            .retain(|record| record.identity.entity.as_str() != E::CableTermination.source_key());
        let (findings, complete) = assess(&snapshot, &rows, provider);
        assert!(!complete);
        assert!(
            findings
                .iter()
                .any(|finding| finding.field_path.as_deref() == Some("_path"))
        );
    }

    #[test]
    fn quantities_match_netbox_unit_conversions_and_nulls() {
        for (kind, value, unit, cache) in [
            (Quantity::Length, "1.00", "km", "1000.0000"),
            (Quantity::Length, "1.00", "m", "1.0000"),
            (Quantity::Length, "1.00", "cm", "0.0100"),
            (Quantity::Length, "1.00", "mi", "1609.3440"),
            (Quantity::Length, "1.00", "ft", "0.3048"),
            (Quantity::Length, "1.00", "in", "0.0254"),
            (Quantity::Weight, "1.00", "kg", "1000"),
            (Quantity::Weight, "1.00", "g", "1"),
            (Quantity::Weight, "1.00", "lb", "453"),
            (Quantity::Weight, "1.00", "oz", "28"),
            (Quantity::Diameter, "1.00", "mm", "1.0000"),
            (Quantity::Diameter, "1.00", "cm", "10.0000"),
            (Quantity::Diameter, "1.00", "in", "25.4000"),
            (Quantity::MaxFlow, "1.00", "lpm", "1.0000"),
            (Quantity::MaxFlow, "1.00", "m3ph", "16.6667"),
            (Quantity::MaxFlow, "1.00", "gpm", "3.7854"),
        ] {
            assert_eq!(
                assert_quantity(
                    kind,
                    Some(&json!(cache)),
                    Some(&json!(value)),
                    Some(&json!(unit))
                ),
                Assertion::Matches,
                "{unit}"
            );
        }
        assert_eq!(
            assert_quantity(
                Quantity::Weight,
                Some(&Value::Null),
                Some(&json!("0.00")),
                Some(&json!("kg"))
            ),
            Assertion::Matches
        );
        assert_eq!(
            assert_quantity(
                Quantity::Length,
                Some(&json!("0.0000")),
                Some(&json!("0.00")),
                Some(&json!("m"))
            ),
            Assertion::Matches
        );
        assert_eq!(
            assert_quantity(
                Quantity::Diameter,
                Some(&Value::Null),
                Some(&Value::Null),
                None
            ),
            Assertion::Matches
        );
        assert_eq!(
            assert_quantity(
                Quantity::Length,
                Some(&Value::Null),
                Some(&json!("1.00")),
                Some(&Value::Null)
            ),
            Assertion::Matches
        );
    }

    #[test]
    fn quantity_claims_fail_closed_and_preserve_omission() {
        fn check(cache: Option<&Value>, value: Option<&Value>, unit: Option<&Value>) -> Assertion {
            assert_quantity(Quantity::MaxFlow, cache, value, unit)
        }
        assert_eq!(check(None, None, None), Assertion::Omitted);
        assert_eq!(
            check(Some(&json!("1")), None, Some(&json!("lpm"))),
            Assertion::DependencyUnavailable
        );
        assert_eq!(
            check(Some(&json!("1")), Some(&json!("1")), None),
            Assertion::DependencyUnavailable
        );
        assert_eq!(
            check(Some(&json!("2")), Some(&json!("1")), Some(&json!("lpm"))),
            Assertion::Invalid
        );
        assert_eq!(
            check(Some(&Value::Null), Some(&json!("1")), Some(&json!("lpm"))),
            Assertion::Invalid
        );
        assert_eq!(
            check(
                Some(&json!("1")),
                Some(&json!("1.001")),
                Some(&json!("lpm"))
            ),
            Assertion::Invalid
        );
        assert_eq!(
            check(
                Some(&json!("1")),
                Some(&json!("1")),
                Some(&json!("unknown"))
            ),
            Assertion::Invalid
        );
        assert_eq!(
            check(Some(&json!("1")), Some(&json!("-1")), Some(&json!("lpm"))),
            Assertion::Invalid
        );
        assert_eq!(
            check(
                Some(&json!("1.00001")),
                Some(&json!("1")),
                Some(&json!("lpm"))
            ),
            Assertion::Invalid
        );
    }

    #[test]
    fn quantity_rounding_and_binary_float_weight_are_exact() {
        assert_eq!(
            assert_quantity(
                Quantity::MaxFlow,
                Some(&json!("0.1667")),
                Some(&json!("0.01")),
                Some(&json!("m3ph"))
            ),
            Assertion::Matches
        );
        assert_eq!(
            assert_quantity(
                Quantity::MaxFlow,
                Some(&json!("0.0379")),
                Some(&json!("0.01")),
                Some(&json!("gpm"))
            ),
            Assertion::Matches
        );
        // Decimal(453.592) is slightly below 453.592: truncation matters at 125 lb.
        assert_eq!(
            assert_quantity(
                Quantity::Weight,
                Some(&json!("56698")),
                Some(&json!("125.00")),
                Some(&json!("lb"))
            ),
            Assertion::Matches
        );
        assert_eq!(
            assert_quantity(
                Quantity::Weight,
                Some(&json!("56699")),
                Some(&json!("125.00")),
                Some(&json!("lb"))
            ),
            Assertion::Invalid
        );
    }

    #[test]
    fn cache_assertions_preserve_absence_and_reject_mismatched_qualifiers() {
        let provider = Uuid::nil();
        let site =
            SourceIdentity::new(provider, SourceEntity::new("dcim.sites").unwrap(), "42").unwrap();
        fn check(value: Option<&Value>, relation: Relation<'_>, provider: Uuid) -> Assertion {
            assert_reference(value, relation, provider, "dcim.sites", "dcim.site")
        }
        assert_eq!(
            check(None, Relation::Unavailable, provider),
            Assertion::Omitted
        );
        assert_eq!(
            check(Some(&Value::Null), Relation::Known(None), provider),
            Assertion::Matches
        );
        assert_eq!(
            check(Some(&Value::Null), Relation::Known(Some(&site)), provider),
            Assertion::Invalid
        );
        assert_eq!(
            check(Some(&json!(42)), Relation::Known(Some(&site)), provider),
            Assertion::Matches
        );
        assert_eq!(
            check(
                Some(&json!({"id": 42, "object_type": "dcim.rack"})),
                Relation::Known(Some(&site)),
                provider
            ),
            Assertion::Invalid
        );
        assert_eq!(
            check(
                Some(&json!({"id": 42, "provider_id": Uuid::new_v4()})),
                Relation::Known(Some(&site)),
                provider
            ),
            Assertion::Invalid
        );
        assert_eq!(
            check(Some(&json!(42)), Relation::Unavailable, provider),
            Assertion::DependencyUnavailable
        );
        assert_eq!(
            check(Some(&json!(43)), Relation::Known(Some(&site)), provider),
            Assertion::Invalid
        );
    }
}
