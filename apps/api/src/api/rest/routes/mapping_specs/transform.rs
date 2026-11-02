use std::collections::HashMap;

use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::contexts::sync::application::{
    EntityMappingDraft, FieldMappingDraft, MappingConfigurationDraft,
};
use crate::contexts::sync::domain::{FieldMapping, MappingRef, MappingTarget};
use crate::contexts::sync::infrastructure::persistence::MappingConfigDocument;
use crate::errors::AppError;

// Shared with the rest of the `mapping_specs` subtree via `super::transform`.
pub(super) use crate::api::rest::routes::validation::parse_uuid;

pub(super) fn format_mapping_configuration_id(logical_id: Uuid, version_id: i32) -> String {
    MappingRef {
        logical_id,
        version_id,
    }
    .to_wire()
}

pub(super) fn parse_mapping_configuration_id(id: &str) -> Result<(Uuid, i32), AppError> {
    let reference = MappingRef::parse(id)?;
    Ok((reference.logical_id, reference.version_id))
}

fn object_field<'a>(map: &'a Map<String, Value>, key: &str) -> Result<&'a Value, AppError> {
    map.get(key)
        .ok_or_else(|| AppError::Validation(format!("{key} is required")))
}

fn string_field(map: &Map<String, Value>, key: &str) -> Result<String, AppError> {
    object_field(map, key)?
        .as_str()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::Validation(format!("{key} must be a non-empty string")))
}

fn optional_string_field(map: &Map<String, Value>, key: &str) -> Option<String> {
    map.get(key)
        .and_then(Value::as_str)
        .map(|value| value.trim().to_string())
}

fn nullable_string_patch(
    map: &Map<String, Value>,
    key: &str,
    current: Option<String>,
) -> Option<String> {
    match map.get(key) {
        Some(Value::Null) => None,
        Some(Value::String(value)) => Some(value.trim().to_string()),
        _ => current,
    }
}

fn uuid_field(map: &Map<String, Value>, key: &str) -> Result<Uuid, AppError> {
    let value = string_field(map, key)?;
    parse_uuid(&value, key)
}

fn optional_uuid_field(map: &Map<String, Value>, key: &str) -> Result<Option<Uuid>, AppError> {
    match optional_string_field(map, key) {
        Some(value) if !value.is_empty() => parse_uuid(&value, key).map(Some),
        _ => Ok(None),
    }
}

fn field_arg(path: Option<&Value>) -> Value {
    json!({ "field": path.and_then(Value::as_str).unwrap_or_default() })
}

pub(super) fn transform_operator_name(expression: &Value) -> Option<&str> {
    expression.as_object().and_then(|map| {
        map.get("op")
            .or_else(|| map.get("operator"))
            .and_then(Value::as_str)
    })
}

fn normalize_sync_transform_to_domain(expression: Value) -> Value {
    let Some(map) = expression.as_object() else {
        return expression;
    };
    if map.contains_key("op") {
        return expression;
    }

    let Some(op) = map.get("operator").and_then(Value::as_str) else {
        return expression;
    };
    let args = map
        .get("args")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    let domain_args = match op {
        "identity" | "trim" | "lower" | "upper" | "mac_normalize" | "ipv4_prefix_canonical" => {
            vec![field_arg(args.get("field"))]
        }
        "literal" => vec![args.get("value").cloned().unwrap_or(Value::Null)],
        "coerce" => vec![
            field_arg(args.get("field")),
            Value::String(
                args.get("to")
                    .and_then(Value::as_str)
                    .map(|value| if value == "float" { "number" } else { value })
                    .unwrap_or("string")
                    .to_string(),
            ),
        ],
        "split_index" => vec![
            field_arg(args.get("field")),
            args.get("separator")
                .cloned()
                .unwrap_or_else(|| Value::String("-".to_string())),
            args.get("index").cloned().unwrap_or_else(|| json!(0)),
        ],
        "lookup" => {
            let mut table = Map::new();
            if let Some(rows) = args.get("table").and_then(Value::as_array) {
                for row in rows {
                    if let (Some(from), Some(to)) = (
                        row.get("from").and_then(Value::as_str),
                        row.get("to").and_then(Value::as_str),
                    ) {
                        table.insert(from.to_string(), Value::String(to.to_string()));
                    }
                }
            }
            vec![
                Value::Object(table),
                field_arg(args.get("key_field").or_else(|| args.get("field"))),
            ]
        }
        "template" => {
            let mut out = vec![
                args.get("pattern")
                    .cloned()
                    .unwrap_or_else(|| Value::String(String::new())),
            ];
            if let Some(vars) = args.get("vars").and_then(Value::as_object) {
                for (name, field) in vars {
                    out.push(json!({
                        "name": name,
                        "value": field_arg(Some(field)),
                    }));
                }
            }
            out
        }
        "dereference" => vec![
            field_arg(args.get("fk_field").or_else(|| args.get("field"))),
            args.get("target_attr")
                .cloned()
                .unwrap_or_else(|| Value::String(String::new())),
        ],
        "date_format" => {
            let format = args
                .get("format")
                .cloned()
                .unwrap_or_else(|| Value::String("%Y-%m-%d".to_string()));
            vec![field_arg(args.get("field")), format.clone(), format]
        }
        "tags_merge" => vec![
            field_arg(args.get("field_a")),
            field_arg(args.get("field_b")),
        ],
        _ => map
            .get("args")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
    };

    json!({ "op": op, "args": domain_args })
}

fn normalize_entity_mappings_transforms(mut value: Value) -> Value {
    let Value::Array(entities) = &mut value else {
        return value;
    };

    for entity in entities {
        let Some(entity_map) = entity.as_object_mut() else {
            continue;
        };
        let Some(fields) = entity_map
            .get_mut("field_mappings")
            .and_then(Value::as_array_mut)
        else {
            continue;
        };
        for field in fields {
            let Some(field_map) = field.as_object_mut() else {
                continue;
            };
            if let Some(forward) = field_map.remove("forward_transform") {
                field_map.insert(
                    "forward_transform".to_string(),
                    normalize_sync_transform_to_domain(forward),
                );
            }
            if let Some(reverse) = field_map.remove("reverse_transform") {
                let reverse = if reverse.is_null() {
                    reverse
                } else {
                    normalize_sync_transform_to_domain(reverse)
                };
                field_map.insert("reverse_transform".to_string(), reverse);
            }
        }
    }

    value
}

pub(super) fn entity_drafts_from_value(value: Value) -> Result<Vec<EntityMappingDraft>, AppError> {
    serde_json::from_value(normalize_entity_mappings_transforms(value))
        .map_err(|error| AppError::Validation(format!("invalid entity_mappings: {error}")))
}

pub(super) fn create_draft_from_sync_payload(
    payload: Value,
) -> Result<(Uuid, MappingConfigurationDraft), AppError> {
    let Value::Object(map) = payload else {
        return Err(AppError::Validation(
            "mapping configuration create body must be a JSON object".to_string(),
        ));
    };

    let provider_id = uuid_field(&map, "provider_id")?;
    let logical_id = optional_uuid_field(&map, "logical_id")?.unwrap_or_else(Uuid::now_v7);
    let source_provider_id =
        optional_uuid_field(&map, "source_provider_id")?.unwrap_or(provider_id);
    let target = mapping_target_from_payload(&map)?;
    let entity_mappings = entity_drafts_from_value(
        map.get("entity_mappings")
            .cloned()
            .unwrap_or_else(|| json!([])),
    )?;

    Ok((
        logical_id,
        MappingConfigurationDraft {
            name: string_field(&map, "name")?,
            description: optional_string_field(&map, "description"),
            source_provider_id,
            target,
            direction: string_field(&map, "direction")?,
            conflict_mode: string_field(&map, "conflict_mode")?,
            entity_mappings,
        },
    ))
}

/// Parse the required `target` object from a sync mapping-configuration
/// payload. A missing target is rejected: there is no implicit default.
fn mapping_target_from_payload(map: &Map<String, Value>) -> Result<MappingTarget, AppError> {
    let value = map.get("target").ok_or_else(|| {
        AppError::Validation("mapping configuration requires an explicit target".to_string())
    })?;
    serde_json::from_value(value.clone())
        .map_err(|error| AppError::Validation(format!("invalid target: {error}")))
}

fn document_to_draft(document: &MappingConfigDocument) -> MappingConfigurationDraft {
    let mut fields_by_entity: HashMap<Uuid, Vec<&FieldMapping>> = HashMap::new();
    for field in &document.field_mappings {
        fields_by_entity
            .entry(field.entity_mapping_id)
            .or_default()
            .push(field);
    }

    let entity_mappings = document
        .entity_mappings
        .iter()
        .map(|entity| EntityMappingDraft {
            source_entity: entity.source_entity.clone(),
            target_entity: entity.target_entity.clone(),
            enabled: entity.enabled,
            field_mappings: fields_by_entity
                .get(&entity.id)
                .into_iter()
                .flat_map(|fields| fields.iter().copied())
                .map(|field| FieldMappingDraft {
                    source_field: field.source_field.clone(),
                    target_field: field.target_field.clone(),
                    forward_transform: field.forward_transform_json.clone(),
                    reverse_transform: field.reverse_transform_json.clone(),
                    round_trip_critical: field.round_trip_critical,
                })
                .collect(),
        })
        .collect();

    MappingConfigurationDraft {
        name: document.name.clone(),
        description: document.description.clone(),
        source_provider_id: document.source_provider_id,
        target: document.target.clone(),
        direction: document.direction.as_str().to_string(),
        conflict_mode: document.conflict_mode.as_str().to_string(),
        entity_mappings,
    }
}

pub(super) fn patch_draft_from_sync_payload(
    payload: Value,
    document: &MappingConfigDocument,
) -> Result<MappingConfigurationDraft, AppError> {
    let Value::Object(map) = payload else {
        return Err(AppError::Validation(
            "mapping configuration patch body must be a JSON object".to_string(),
        ));
    };

    let current = document_to_draft(document);
    let entity_mappings = match map.get("entity_mappings") {
        Some(value) => entity_drafts_from_value(value.clone())?,
        None => current.entity_mappings,
    };

    Ok(MappingConfigurationDraft {
        name: optional_string_field(&map, "name").unwrap_or(current.name),
        description: nullable_string_patch(&map, "description", current.description),
        source_provider_id: document.source_provider_id,
        target: document.target.clone(),
        direction: optional_string_field(&map, "direction").unwrap_or(current.direction),
        conflict_mode: map
            .get("conflict_mode")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or(current.conflict_mode),
        entity_mappings,
    })
}

fn field_path_from_arg(value: Option<&Value>) -> String {
    value
        .and_then(|arg| arg.get("field"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn sync_transform_from_domain(expression: &Value) -> Value {
    let Some(map) = expression.as_object() else {
        return expression.clone();
    };
    if map.contains_key("operator") {
        return expression.clone();
    }

    let Some(op) = map.get("op").and_then(Value::as_str) else {
        return expression.clone();
    };
    let args = map
        .get("args")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let args_value = match op {
        "identity" | "trim" | "lower" | "upper" | "mac_normalize" | "ipv4_prefix_canonical" => {
            json!({ "field": field_path_from_arg(args.first()) })
        }
        "literal" => json!({
            "value": args.first().cloned().unwrap_or(Value::Null),
            "value_type": "string",
        }),
        "coerce" => json!({
            "field": field_path_from_arg(args.first()),
            "to": args.get(1).and_then(Value::as_str).unwrap_or("string"),
        }),
        "split_index" => json!({
            "field": field_path_from_arg(args.first()),
            "separator": args.get(1).and_then(Value::as_str).unwrap_or("-"),
            "index": args.get(2).and_then(Value::as_i64).unwrap_or(0),
        }),
        "lookup" => {
            let table = args
                .first()
                .and_then(Value::as_object)
                .map(|table| {
                    table
                        .iter()
                        .map(|(from, to)| json!({ "from": from, "to": to }))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            json!({
                "key_field": field_path_from_arg(args.get(1)),
                "table": table,
            })
        }
        "template" => {
            let mut vars = Map::new();
            for arg in args.iter().skip(1) {
                if let (Some(name), Some(value)) = (
                    arg.get("name").and_then(Value::as_str),
                    arg.get("value")
                        .and_then(|value| value.get("field"))
                        .and_then(Value::as_str),
                ) {
                    vars.insert(name.to_string(), Value::String(value.to_string()));
                }
            }
            json!({
                "pattern": args.first().and_then(Value::as_str).unwrap_or_default(),
                "vars": Value::Object(vars),
            })
        }
        "dereference" => json!({
            "fk_field": field_path_from_arg(args.first()),
            "target_attr": args.get(1).and_then(Value::as_str).unwrap_or_default(),
        }),
        "date_format" => json!({
            "field": field_path_from_arg(args.first()),
            "format": args.get(2).and_then(Value::as_str).unwrap_or("%Y-%m-%d"),
        }),
        "tags_merge" => json!({
            "field_a": field_path_from_arg(args.first()),
            "field_b": field_path_from_arg(args.get(1)),
        }),
        _ => json!({}),
    };

    json!({ "operator": op, "args": args_value })
}

pub(super) fn sync_entity_mappings(document: &MappingConfigDocument) -> Vec<Value> {
    let mut fields_by_entity: HashMap<Uuid, Vec<&FieldMapping>> = HashMap::new();
    for field in &document.field_mappings {
        fields_by_entity
            .entry(field.entity_mapping_id)
            .or_default()
            .push(field);
    }

    document
        .entity_mappings
        .iter()
        .map(|entity| {
            let field_mappings = fields_by_entity
                .get(&entity.id)
                .into_iter()
                .flat_map(|fields| fields.iter().copied())
                .map(|field| {
                    json!({
                        "id": field.id.to_string(),
                        "source_field": field.source_field,
                        "target_field": field.target_field,
                        "enabled": true,
                        "forward_transform": sync_transform_from_domain(&field.forward_transform_json),
                        "reverse_transform": field.reverse_transform_json.as_ref().map(sync_transform_from_domain),
                    })
                })
                .collect::<Vec<_>>();
            json!({
                "id": entity.id.to_string(),
                "source_entity": entity.source_entity,
                "target_entity": entity.target_entity,
                "enabled": entity.enabled,
                "field_mappings": field_mappings,
            })
        })
        .collect()
}

pub(super) fn sync_mapping_configuration_response(
    document: &MappingConfigDocument,
    is_latest: bool,
) -> super::SyncMappingConfigurationResponse {
    let reference = document.reference;
    super::SyncMappingConfigurationResponse {
        id: format_mapping_configuration_id(reference.logical_id, reference.version_id),
        logical_id: reference.logical_id.to_string(),
        name: document.name.clone(),
        version: reference.version_id,
        version_id: reference.version_id,
        is_latest,
        provider_id: document.source_provider_id.to_string(),
        source_provider_id: document.source_provider_id.to_string(),
        target: document.target.clone(),
        status: document.status.as_str().to_string(),
        direction: document.direction.as_str().to_string(),
        conflict_mode: document.conflict_mode.as_str().to_string(),
        entity_mappings: sync_entity_mappings(document),
        created_at: document.created_at.to_rfc3339(),
        created_by: document.created_by.map(|id| id.to_string()),
        retired_at: document.retired_at.map(|ts| ts.to_rfc3339()),
        in_flight_job_count: 0,
    }
}
