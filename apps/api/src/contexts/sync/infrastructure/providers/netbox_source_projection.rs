//! Typed binding from approved inventory catalogue keys to NetBox's canonical
//! REST surface.
//!
//! Most catalogue keys name their own NetBox REST collection. One does not:
//! `dcim.port-template-mappings` is the ORM-only model behind the `rear_ports`
//! items embedded in every `dcim.front-port-templates` record, and NetBox 4.7.x
//! exposes no collection for it. The binding names the collection that really
//! carries the relationship, and the projection flattens each embedded item into
//! one mapping record. The catalogue therefore stays complete without inventing
//! an endpoint, serving an empty collection or dropping the relationship.
//!
//! Only the canonical upstream shape is accepted. A parent record without
//! `rear_ports`, a legacy scalar `rear_port` pair, a null or non-integer
//! position/id, or two items with the same front-port position fails here,
//! before any local write.

use std::collections::HashSet;

use serde_json::{Map, Value};
use uuid::Uuid;

use crate::contexts::inventory::domain::dcim::component_template::{
    PORT_POSITION_MAX, PORT_POSITION_MIN,
};
use crate::contexts::sync::domain::inventory_sync::normalize_netbox_id;
use crate::contexts::sync::domain::{InventoryEntity, SourceEntity, SourceIdentity, SourceRecord};
use crate::errors::AppError;

/// How one approved catalogue key is read from NetBox.
#[derive(Debug, Clone, Copy)]
pub(super) enum SourceBinding {
    /// The catalogue key names its own NetBox REST collection.
    Collection,
    /// `dcim.port-template-mappings`: the `rear_ports` items of
    /// `dcim.front-port-templates` records.
    FrontPortTemplateRearPorts,
}

impl SourceBinding {
    /// Bind one approved catalogue key to its canonical REST collection.
    pub(super) fn of(entity: &SourceEntity) -> Self {
        if entity.as_str() == InventoryEntity::PortTemplateMapping.source_key() {
            Self::FrontPortTemplateRearPorts
        } else {
            Self::Collection
        }
    }

    /// The REST collection this binding reads pages from.
    pub(super) fn collection(self, entity: &SourceEntity) -> &str {
        match self {
            Self::Collection => entity.as_str(),
            // The embedded relationship is only serialized on front port
            // templates; the rear port's `front_ports` is its reverse view.
            Self::FrontPortTemplateRearPorts => InventoryEntity::FrontPortTemplate.source_key(),
        }
    }
}

fn invalid(code: &str) -> AppError {
    AppError::Validation(format!("netbox_source:{code}"))
}

/// A positive NetBox relationship id in its canonical integer JSON shape.
///
/// NetBox serializes a relation either as its integer id or, at a nested
/// depth, as a brief object carrying that id. Both spellings are the same
/// provider identity the local resolver already understands.
pub(super) fn relation_id(value: Option<&Value>, code: &str) -> Result<Value, AppError> {
    let value = value.ok_or_else(|| invalid(code))?;
    let id = match value {
        Value::Object(map) => map.get("id").ok_or_else(|| invalid(code))?,
        scalar => scalar,
    };
    let canonical = normalize_netbox_id(id).map_err(|_| invalid(code))?;
    canonical
        .parse()
        .map(Value::Number)
        .map_err(|_| invalid(code))
}

/// `device_type` / `module_type` are always serialized, nullable, and carry the
/// parent's own scope. Absence is a shape we do not understand, not a null.
fn parent_relation(template: &Map<String, Value>, field: &str) -> Result<Value, AppError> {
    let code = format!("port_template_parent.{field}");
    match template.get(field) {
        Some(Value::Null) => Ok(Value::Null),
        Some(value) => relation_id(Some(value), &code),
        None => Err(invalid(&code)),
    }
}

/// One front-port position, as NetBox spells it in the embedded serializer.
///
/// The range is the native template owner's `PORT_POSITION_MIN..=MAX` contract,
/// so a value the writer would reject never reaches the local target.
fn position(item: &Map<String, Value>, field: &str) -> Result<i64, AppError> {
    let code = format!("port_template_mapping.{field}");
    item.get(field)
        .and_then(Value::as_i64)
        .filter(|value| {
            (i64::from(PORT_POSITION_MIN)..=i64::from(PORT_POSITION_MAX)).contains(value)
        })
        .ok_or_else(|| invalid(&code))
}

/// Flatten `rear_ports` items of one parent page into mapping records.
///
/// The identity is `dcim.front-port-templates/<front id>/position/<position>`:
/// deterministic, stable across pulls, and derived from the relationship itself
/// so a mapping survives unrelated renames of either endpoint.
pub(super) fn project_port_template_mappings(
    provider_id: Uuid,
    templates: &[Value],
    entity: &SourceEntity,
) -> Result<Vec<SourceRecord>, AppError> {
    let front_key = InventoryEntity::FrontPortTemplate.source_key();
    let mut records = Vec::new();
    for template in templates {
        let template = template
            .as_object()
            .ok_or_else(|| invalid("port_template_shape"))?;
        let front_port = relation_id(template.get("id"), "port_template_identity")?;
        let front_port_id = front_port
            .as_u64()
            .ok_or_else(|| invalid("port_template_identity"))?;
        let device_type = parent_relation(template, "device_type")?;
        let module_type = parent_relation(template, "module_type")?;
        let rear_ports = template
            .get("rear_ports")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid("port_template_rear_ports"))?;
        let mut seen = HashSet::with_capacity(rear_ports.len());
        for item in rear_ports {
            let item = item
                .as_object()
                .ok_or_else(|| invalid("port_template_mapping_shape"))?;
            let front_port_position = position(item, "position")?;
            if !seen.insert(front_port_position) {
                return Err(invalid("port_template_mapping_duplicate_position"));
            }
            let external_id = format!("{front_key}/{front_port_id}/position/{front_port_position}");
            let mut fields = Map::new();
            fields.insert("front_port".into(), front_port.clone());
            fields.insert(
                "front_port_position".into(),
                Value::from(front_port_position),
            );
            fields.insert(
                "rear_port".into(),
                relation_id(item.get("rear_port"), "port_template_mapping.rear_port")?,
            );
            fields.insert(
                "rear_port_position".into(),
                Value::from(position(item, "rear_port_position")?),
            );
            fields.insert("device_type".into(), device_type.clone());
            fields.insert("module_type".into(), module_type.clone());
            records.push(SourceRecord::new(
                SourceIdentity::new(provider_id, entity.clone(), external_id)?,
                fields,
            ));
        }
    }
    Ok(records)
}

/// Advance one offset cursor by the **consumed raw parent records**.
///
/// A derived resource emits more than one record per parent, so the page guard
/// must compare `page.count` with the parent records read, never with the
/// projected rows it produced.
pub(super) fn page_cursor(
    offset: usize,
    consumed: usize,
    count: usize,
) -> Result<Option<String>, AppError> {
    let fetched = offset
        .checked_add(consumed)
        .ok_or_else(|| AppError::Validation("Source page offset overflow".into()))?;
    if fetched > count || (consumed == 0 && fetched < count) {
        return Err(AppError::Validation(
            "Source pagination contradicts its declared record count".into(),
        ));
    }
    Ok(if fetched == count {
        None
    } else {
        Some(fetched.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn mapping_entity() -> SourceEntity {
        SourceEntity::new("dcim.port-template-mappings").expect("canonical mapping key")
    }

    fn template(id: u64, rear_ports: Value) -> Value {
        json!({
            "id": id,
            "name": format!("front-{id}"),
            "device_type": {"id": 7, "url": "http://127.0.0.1:8000/api/dcim/device-types/7/", "display": "dt-7"},
            "module_type": null,
            "rear_ports": rear_ports,
        })
    }

    #[test]
    fn every_position_becomes_one_stable_mapping_record() {
        let entity = mapping_entity();
        let provider = Uuid::from_u128(1);
        let parent = template(
            4,
            json!([
                {"position": 2, "rear_port": 11, "rear_port_position": 5},
                {"position": 1, "rear_port": 10, "rear_port_position": 3},
            ]),
        );
        let records = project_port_template_mappings(provider, &[parent.clone()], &entity)
            .expect("canonical embedded relationship");
        assert_eq!(records.len(), 2);
        let identities: Vec<&str> = records
            .iter()
            .map(|record| record.identity.external_id.as_str())
            .collect();
        assert_eq!(
            identities,
            [
                "dcim.front-port-templates/4/position/2",
                "dcim.front-port-templates/4/position/1"
            ]
        );
        assert_eq!(
            Value::Object(records[0].fields.clone()),
            json!({
                "front_port": 4,
                "front_port_position": 2,
                "rear_port": 11,
                "rear_port_position": 5,
                "device_type": 7,
                "module_type": null
            })
        );
        // The same positions in the other array order keep the same identities.
        let reordered = template(
            4,
            json!([
                {"position": 1, "rear_port": 10, "rear_port_position": 3},
                {"position": 2, "rear_port": 11, "rear_port_position": 5},
            ]),
        );
        let again = project_port_template_mappings(provider, &[reordered], &entity)
            .expect("canonical embedded relationship");
        let mut first: Vec<&str> = identities;
        let mut second: Vec<&str> = again
            .iter()
            .map(|record| record.identity.external_id.as_str())
            .collect();
        first.sort_unstable();
        second.sort_unstable();
        assert_eq!(first, second);
        assert_eq!(
            again[0].fields.get("front_port_position"),
            Some(&json!(1)),
            "field values stay bound to their own position"
        );
    }

    #[test]
    fn module_type_scope_is_preserved_and_required() {
        let entity = mapping_entity();
        let provider = Uuid::from_u128(1);
        let parent = json!({
            "id": 9,
            "device_type": null,
            "module_type": {"id": 3, "display": "module-3"},
            "rear_ports": [{"position": 1, "rear_port": 2, "rear_port_position": 1}],
        });
        let records = project_port_template_mappings(provider, &[parent], &entity)
            .expect("module-type parent");
        assert_eq!(records[0].fields.get("device_type"), Some(&Value::Null));
        assert_eq!(records[0].fields.get("module_type"), Some(&json!(3)));
    }

    #[test]
    fn noncanonical_or_incomplete_relationships_fail_closed() {
        let entity = mapping_entity();
        let provider = Uuid::from_u128(1);
        let cases = [
            json!({"id": 1, "device_type": 7, "module_type": null}),
            json!({"id": 1, "device_type": 7, "module_type": null, "rear_ports": null}),
            json!({"id": 1, "device_type": 7, "module_type": null, "rear_ports": [null]}),
            json!({"id": 1, "device_type": 7, "module_type": null, "rear_ports": [
                {"position": 0, "rear_port": 2, "rear_port_position": 1}
            ]}),
            json!({"id": 1, "device_type": 7, "module_type": null, "rear_ports": [
                {"position": "1", "rear_port": 2, "rear_port_position": 1}
            ]}),
            json!({"id": 1, "device_type": 7, "module_type": null, "rear_ports": [
                {"position": 1, "rear_port": 0, "rear_port_position": 1}
            ]}),
            json!({"id": 1, "device_type": 7, "module_type": null, "rear_ports": [
                {"position": 1, "rear_port": 2}
            ]}),
            json!({"id": 1, "device_type": 7, "module_type": null, "rear_ports": [
                {"position": 1, "rear_port": 2, "rear_port_position": 1},
                {"position": 1, "rear_port": 3, "rear_port_position": 2}
            ]}),
            json!({"id": 1, "module_type": null, "rear_ports": []}),
            json!({"id": 1, "device_type": 7, "rear_ports": []}),
            json!({"id": 1, "device_type": {"display": "no id"}, "module_type": null, "rear_ports": []}),
            json!({"id": 1, "device_type": 7, "module_type": null, "rear_ports": [
                {"position": 1, "rear_port": -1, "rear_port_position": 1}
            ]}),
        ];
        for case in cases {
            assert!(
                project_port_template_mappings(provider, &[case.clone()], &entity).is_err(),
                "{case} must fail closed"
            );
        }
        assert!(
            project_port_template_mappings(provider, &[json!({"id": 1})], &entity).is_err(),
            "a parent without the embedded relationship must fail closed"
        );
        assert!(
            project_port_template_mappings(provider, &[json!("front-1")], &entity).is_err(),
            "a non-object parent must fail closed"
        );
        let legacy = json!({
            "id": 1,
            "device_type": 7,
            "module_type": null,
            "rear_port": 2,
            "rear_port_position": 1,
        });
        assert!(
            project_port_template_mappings(provider, &[legacy], &entity).is_err(),
            "the legacy scalar shape has no embedded relationship and must fail closed"
        );
    }

    #[test]
    fn pagination_counts_consumed_parent_records_not_projected_rows() {
        let entity = mapping_entity();
        let provider = Uuid::from_u128(1);
        let parent = template(
            1,
            json!([
                {"position": 1, "rear_port": 2, "rear_port_position": 1},
                {"position": 2, "rear_port": 3, "rear_port_position": 1},
                {"position": 3, "rear_port": 4, "rear_port_position": 1},
            ]),
        );
        let records =
            project_port_template_mappings(provider, &[parent], &entity).expect("three mappings");
        assert_eq!(records.len(), 3);
        assert_eq!(
            page_cursor(0, 1, 1).expect("one parent page"),
            None,
            "three projected rows from one parent must not contradict a count of one"
        );
        assert_eq!(
            page_cursor(0, 1, 4).expect("more parents"),
            Some("1".into())
        );
        assert_eq!(page_cursor(3, 1, 4).expect("last parent"), None);
        assert!(
            page_cursor(0, 0, 4).is_err(),
            "an empty page is not the end"
        );
        assert!(
            page_cursor(0, 2, 1).is_err(),
            "consumed parents exceed the count"
        );
        assert!(
            page_cursor(usize::MAX, 1, 4).is_err(),
            "offset overflow fails"
        );
    }
}
