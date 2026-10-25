//! Typed run selections and contextual projection share the API field registry.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use serde::Deserialize;
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::errors::AppError;

#[derive(Debug, Deserialize)]
struct Registry {
    targets: HashMap<String, TargetDefinition>,
    fields: HashMap<String, HashMap<String, ReferenceField>>,
}

#[derive(Debug, Deserialize)]
struct TargetDefinition {
    table: String,
}

#[derive(Debug)]
pub(crate) struct ReferenceTarget {
    pub(crate) resource: String,
    pub(crate) table: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReferenceField {
    resource: String,
    id_field: String,
}

static REGISTRY: LazyLock<Result<Registry, serde_json::Error>> = LazyLock::new(|| {
    serde_json::from_str(include_str!(
        "../../../../../contracts/inventory-template-references.json"
    ))
});

fn registry() -> Result<&'static Registry, AppError> {
    REGISTRY.as_ref().map_err(|error| {
        AppError::Internal(format!("Invalid template reference registry: {error}"))
    })
}

pub(crate) fn targets() -> Result<Vec<ReferenceTarget>, AppError> {
    Ok(registry()?
        .targets
        .iter()
        .map(|(resource, target)| ReferenceTarget {
            resource: resource.clone(),
            table: target.table.clone(),
        })
        .collect())
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Hash)]
#[serde(deny_unknown_fields)]
pub struct TemplateSelection {
    pub resource: String,
    pub id: Uuid,
}

pub(crate) fn selection(value: &Value) -> Result<TemplateSelection, AppError> {
    let selected: TemplateSelection = serde_json::from_value(value.clone()).map_err(|_| {
        AppError::Validation(
            "A template selection requires exactly resource and UUID id".to_string(),
        )
    })?;
    if !registry()?.targets.contains_key(&selected.resource) {
        return Err(AppError::Validation(format!(
            "Unknown template reference resource: {}",
            selected.resource
        )));
    }
    Ok(selected)
}

fn whole_token(value: &Value, key: &str) -> bool {
    value
        .as_str()
        .and_then(|text| text.trim().strip_prefix("{{")?.strip_suffix("}}"))
        .is_some_and(|token| token.trim() == key)
}

fn contains_token(value: &Value, key: &str) -> bool {
    match value {
        Value::String(text) => text.split("{{").skip(1).any(|part| {
            part.split_once("}}")
                .is_some_and(|(token, _)| token.split_whitespace().any(|part| part == key))
        }),
        Value::Object(map) => map.values().any(|value| contains_token(value, key)),
        Value::Array(values) => values.iter().any(|value| contains_token(value, key)),
        _ => false,
    }
}

/// JSON variables remain JSON even when they happen to contain resource/id keys.
/// Only non-JSON variable objects at this boundary can be typed selections.
pub(crate) fn validate_run_selections(
    definitions: &[Value],
    steps: &[Value],
    variables: &Value,
) -> Result<Vec<TemplateSelection>, AppError> {
    let registry = registry()?;
    let Some(variables) = variables.as_object() else {
        return Ok(Vec::new());
    };
    let mut selections = HashSet::new();
    for (key, value) in variables {
        let definition = definitions
            .iter()
            .find(|definition| definition.get("key").and_then(Value::as_str) == Some(key));
        if definition
            .and_then(|definition| definition.get("value_type"))
            .and_then(Value::as_str)
            == Some("json")
        {
            for step in steps {
                let fields = step
                    .get("resource_type")
                    .and_then(Value::as_str)
                    .and_then(|resource| registry.fields.get(resource));
                if value.is_object()
                    && step
                        .get("payload_template")
                        .and_then(Value::as_object)
                        .is_some_and(|payload| {
                            payload.iter().any(|(field, value)| {
                                fields.is_some_and(|fields| fields.contains_key(field))
                                    && contains_token(value, key)
                            })
                        })
                {
                    return Err(AppError::Validation(format!(
                        "JSON variable {key} is not a reference selection"
                    )));
                }
            }
            continue;
        }
        if !value.is_object() {
            if !value.is_string()
                && !value.is_null()
                && steps.iter().any(|step| {
                    let fields = step
                        .get("resource_type")
                        .and_then(Value::as_str)
                        .and_then(|resource| registry.fields.get(resource));
                    step.get("payload_template")
                        .and_then(Value::as_object)
                        .is_some_and(|payload| {
                            payload.iter().any(|(field, field_value)| {
                                fields.is_some_and(|fields| fields.contains_key(field))
                                    && contains_token(field_value, key)
                            })
                        })
                })
            {
                return Err(AppError::Validation(format!(
                    "Reference input {key} must be a string or a structured selection"
                )));
            }
            continue;
        }
        let selected = selection(value)?;
        let mut consumed = false;
        for step in steps {
            let resource_type = step
                .get("resource_type")
                .and_then(Value::as_str)
                .unwrap_or("");
            let fields = registry.fields.get(resource_type);
            if let Some(payload) = step.get("payload_template").and_then(Value::as_object) {
                for (field_key, field_value) in payload {
                    if !contains_token(field_value, key) {
                        continue;
                    }
                    consumed = true;
                    let field = fields.and_then(|fields| fields.get(field_key));
                    if !whole_token(field_value, key)
                        || field.is_none_or(|field| field.resource != selected.resource)
                    {
                        return Err(AppError::Validation(format!(
                            "Selection {key} has an incompatible consumer: {resource_type}.{field_key}"
                        )));
                    }
                }
            }
            // Selections cannot be interpolated into titles, repeat controls or dependencies.
            if let Some(map) = step.as_object()
                && map
                    .iter()
                    .any(|(field, value)| field != "payload_template" && contains_token(value, key))
            {
                return Err(AppError::Validation(format!(
                    "Selection {key} is used outside a reference field"
                )));
            }
        }
        if !consumed {
            let declared = definition
                .filter(|definition| {
                    definition.get("value_type").and_then(Value::as_str) == Some("resource_ref")
                })
                .and_then(|definition| definition.pointer("/validation_rules/resource"))
                .and_then(Value::as_str);
            if declared != Some(selected.resource.as_str()) {
                return Err(AppError::Validation(format!(
                    "Selection {key} has no compatible reference declaration or consumer"
                )));
            }
        }
        selections.insert(selected);
    }
    for step in steps {
        let fields = step
            .get("resource_type")
            .and_then(Value::as_str)
            .and_then(|resource| registry.fields.get(resource));
        if step
            .get("payload_template")
            .and_then(Value::as_object)
            .is_some_and(|payload| {
                payload.iter().any(|(key, value)| {
                    fields.is_some_and(|fields| fields.contains_key(key)) && value.is_object()
                })
            })
        {
            return Err(AppError::Validation(
                "Reference payload fields must use a string or a whole run-variable token"
                    .to_string(),
            ));
        }
    }
    Ok(selections.into_iter().collect())
}

/// Promote selected name references to UUID fields without changing literal names.
pub(crate) fn project_payload(step: &Value, payload: Value) -> Result<Value, AppError> {
    let resource_type = step
        .get("resource_type")
        .and_then(Value::as_str)
        .unwrap_or("");
    let Some(fields) = registry()?.fields.get(resource_type) else {
        return Ok(payload);
    };
    let Some(original) = payload.as_object() else {
        return Ok(payload);
    };
    let mut projected: Map<String, Value> = original.clone();
    for (key, field) in fields {
        let Some(value) = original.get(key).filter(|value| value.is_object()) else {
            continue;
        };
        let selected = selection(value)?;
        if selected.resource != field.resource {
            return Err(AppError::Validation(format!(
                "Selection target does not match {resource_type}.{key}"
            )));
        }
        let uuid_value = Value::String(selected.id.to_string());
        if key != &field.id_field {
            if let Some(existing) = original
                .get(&field.id_field)
                .filter(|value| !value.is_null())
            {
                let agrees = if existing.is_object() {
                    selection(existing)? == selected
                } else {
                    existing == &uuid_value
                };
                if !agrees {
                    return Err(AppError::Validation(format!(
                        "Conflicting template reference fields: {key} and {}",
                        field.id_field
                    )));
                }
            }
            projected.remove(key);
        } else {
            for (name_key, name_field) in fields {
                if name_key != key
                    && name_field.id_field == *key
                    && let Some(existing) = original.get(name_key).filter(|value| !value.is_null())
                    && (!existing.is_object() || selection(existing)? != selected)
                {
                    return Err(AppError::Validation(format!(
                        "Conflicting template reference fields: {name_key} and {key}"
                    )));
                }
            }
        }
        projected.insert(field.id_field.clone(), uuid_value);
    }
    Ok(Value::Object(projected))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn template_reference_boundary_rejects_invalid_and_incompatible_selections() {
        let definitions = vec![json!({"key":"chosen","value_type":"string"})];
        let step = json!({"resource_type":"device","payload_template":{"role":"{{chosen}}","role_id":"{{chosen}}"}});
        let id = Uuid::new_v4();
        let selected = json!({"resource":"device_role","id":id});
        assert_eq!(
            validate_run_selections(
                &definitions,
                std::slice::from_ref(&step),
                &json!({"chosen":selected})
            )
            .expect("valid selection")
            .len(),
            1
        );
        for invalid in [
            json!({"resource":"site","id":id}),
            json!({"resource":"unknown","id":id}),
            json!({"resource":"device_role","id":"bad"}),
            json!({"resource":"device_role","id":id,"name":"extra"}),
            json!({"id":id}),
            json!([{"resource":"device_role","id":id}]),
            json!(123),
        ] {
            assert!(
                validate_run_selections(
                    &definitions,
                    std::slice::from_ref(&step),
                    &json!({"chosen":invalid})
                )
                .is_err()
            );
        }
        for payload in [
            json!({"name":"{{chosen}}"}),
            json!({"role":"prefix {{chosen}}"}),
            json!({"role":["{{chosen}}"]}),
            json!({"role":"{{cidr_host chosen 1}}"}),
        ] {
            let bad = json!({"resource_type":"device","payload_template":payload});
            assert!(
                validate_run_selections(&definitions, &[bad], &json!({"chosen":selected})).is_err()
            );
        }
    }

    #[test]
    fn template_reference_json_remains_json_and_manual_strings_remain_strings() {
        let value = json!({"resource":"device_type","id":"not-an-identity","extra":true});
        let definitions = vec![json!({"key":"data","value_type":"json"})];
        let step = json!({"resource_type":"site","payload_template":{"comments":"{{data}}"}});
        assert!(
            validate_run_selections(
                &definitions,
                std::slice::from_ref(&step),
                &json!({"data":value})
            )
            .expect("JSON remains JSON")
            .is_empty()
        );
        let payload = json!({"role":"Core Switch","device_type":"same-model"});
        assert_eq!(
            project_payload(&json!({"resource_type":"device"}), payload.clone())
                .expect("manual strings"),
            payload
        );
        let reference_consumer =
            json!({"resource_type":"device","payload_template":{"device_type":"{{data}}"}});
        assert!(
            validate_run_selections(
                &definitions,
                &[reference_consumer],
                &json!({"data":{"resource":"device_type","id":Uuid::new_v4()}})
            )
            .is_err()
        );
    }

    #[test]
    fn template_reference_tenant_is_shared_by_inventory_and_vpn_steps() {
        let id = Uuid::new_v4();
        let selected = json!({"resource":"tenant","id":id});
        let definitions = vec![json!({"key":"tenant_id","value_type":"resource_ref",
            "validation_rules":{"resource":"tenant"}})];
        let steps: Vec<Value> = ["site", "prefix", "device", "tunnel"]
            .into_iter()
            .map(|resource| {
                json!({"resource_type":resource,
                "payload_template":{"tenant_id":"{{tenant_id}}"}})
            })
            .collect();
        assert_eq!(
            validate_run_selections(&definitions, &steps, &json!({"tenant_id":selected}))
                .expect("one tenant selection may be reused by inventory and VPN")
                .len(),
            1
        );
        for step in &steps {
            assert_eq!(
                project_payload(step, json!({"tenant_id":selected})).expect("tenant UUID"),
                json!({"tenant_id":id})
            );
        }
        assert!(
            validate_run_selections(
                &definitions,
                &steps,
                &json!({"tenant_id":{"resource":"site","id":id}})
            )
            .is_err()
        );
    }

    #[test]
    fn template_reference_projects_supported_tenant_and_vpn_foreign_keys() {
        for (resource_type, field, target) in [
            ("tenant", "group_id", "tenant_group"),
            ("tunnel", "group_id", "tunnel_group"),
            ("tunnel", "ipsec_profile_id", "ipsec_profile"),
            ("tunnel_termination", "tunnel_id", "tunnel"),
            ("tunnel_termination", "interface_id", "device_interface"),
            ("tunnel_termination", "vm_interface_id", "vm_interface"),
            ("tunnel_termination", "outside_ip_id", "ip_address"),
        ] {
            let id = Uuid::new_v4();
            let selected = json!({"resource":target,"id":id});
            let step =
                json!({"resource_type":resource_type,"payload_template":{field:"{{chosen}}"}});
            assert_eq!(
                validate_run_selections(
                    &[json!({"key":"chosen","value_type":"resource_ref"})],
                    std::slice::from_ref(&step),
                    &json!({"chosen":selected})
                )
                .expect("supported reference")
                .len(),
                1
            );
            assert_eq!(
                project_payload(&step, json!({field:selected})).expect("project UUID"),
                json!({field:id})
            );
        }
    }

    #[test]
    fn template_reference_projection_rejects_conflicting_fields() {
        let selected = json!({"resource":"device_role","id":Uuid::new_v4()});
        let step = json!({"resource_type":"device"});
        assert!(project_payload(&step, json!({"role":selected,"role_id":Uuid::new_v4()})).is_err());
        assert!(project_payload(&step, json!({"role":"manual name","role_id":selected})).is_err());
        let id = selected.get("id").expect("selection ID");
        assert_eq!(
            project_payload(&step, json!({"role":selected,"role_id":selected}))
                .expect("identical fields"),
            json!({"role_id":id})
        );
    }
}
