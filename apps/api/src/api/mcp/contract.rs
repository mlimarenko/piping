//! Per-operation MCP argument schemas derived from the embedded OpenAPI document.
//!
//! `piping.rest.call_endpoint` advertises and enforces one envelope per REST
//! operation: the declared `path_params`, the declared `query` parameters and
//! the resolved `application/json` request body. The document is parsed once
//! here and is the only source of those schemas.

use super::*;

/// Prefix of the component references resolved by [`resolved_schema`].
const OPENAPI_COMPONENT_PREFIX: &str = "#/components/schemas/";
/// Guards a component graph that repeats the same reference along one path.
const OPENAPI_SCHEMA_DEPTH_LIMIT: usize = 24;

/// Description of the envelope used when the contract declares no JSON body.
const GENERIC_ENVELOPE_DESCRIPTION: &str = "Generic MCP argument envelope: the OpenAPI document declares no application/json input schema for this operation, so path_params, query and body are forwarded to the REST layer, which validates them.";

#[allow(
    clippy::expect_used,
    reason = "the OpenAPI document is embedded at compile time via include_str!, so a parse failure is a programmer error caught the first time any MCP REST endpoint is built, never a runtime condition; the OpenAPI contract tests fail before such a document could ship"
)]
pub(super) static OPENAPI_DOCUMENT: Lazy<Value> = Lazy::new(|| {
    serde_yml::from_str(include_str!("../../../contracts/piping.openapi.yaml"))
        .expect("valid OpenAPI yaml")
});

/// Memoized envelopes, keyed by lowercase method and path template.
type OperationInputSchemas = HashMap<(String, String), Arc<Value>>;

static OPERATION_INPUT_SCHEMAS: Lazy<OperationInputSchemas> =
    Lazy::new(build_operation_input_schemas);

static GENERIC_INPUT_SCHEMA: Lazy<Arc<Value>> = Lazy::new(|| Arc::new(generic_input_schema()));

/// The request body one operation declares, if any.
enum ContractBody {
    /// The operation declares no request body.
    None,
    Json {
        schema: Value,
        required: bool,
    },
    /// A request body that is not `application/json`, such as a multipart upload.
    Unsupported,
}

/// The schema of one operation's MCP argument envelope, keyed by method and
/// path template. Operations absent from the document yield the generic
/// envelope.
pub(super) fn endpoint_input_schema(method: &str, path_template: &str) -> Arc<Value> {
    let key = (
        method.trim().to_ascii_lowercase(),
        path_template.to_string(),
    );
    OPERATION_INPUT_SCHEMAS
        .get(&key)
        .map_or_else(|| Arc::clone(&GENERIC_INPUT_SCHEMA), Arc::clone)
}

/// The owned envelope of one operation. The MCP tests inspect it directly;
/// production code passes the memoized `Arc` around instead.
#[cfg(test)]
pub(super) fn operation_input_schema(method: &str, path_template: &str) -> Value {
    endpoint_input_schema(method, path_template)
        .as_ref()
        .clone()
}

fn build_operation_input_schemas() -> OperationInputSchemas {
    let mut schemas = HashMap::new();
    let Some(paths) = OPENAPI_DOCUMENT.get("paths").and_then(Value::as_object) else {
        return schemas;
    };
    for (path_template, path_spec) in paths {
        let Some(operations) = path_spec.as_object() else {
            continue;
        };
        let path_parameters = path_spec
            .get("parameters")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        for (method_key, operation) in operations {
            if method_from_str(method_key).is_err() {
                continue;
            }
            let key = (method_key.to_ascii_lowercase(), path_template.clone());
            let schema = match contract_body(operation) {
                ContractBody::Unsupported => Arc::clone(&GENERIC_INPUT_SCHEMA),
                body => Arc::new(operation_envelope_schema(
                    method_key,
                    path_template,
                    path_parameters,
                    operation,
                    body,
                )),
            };
            schemas.insert(key, schema);
        }
    }
    schemas
}

fn operation_envelope_schema(
    method: &str,
    path_template: &str,
    path_parameters: &[Value],
    operation: &Value,
    body: ContractBody,
) -> Value {
    let parameters = operation_parameters(path_parameters, operation);
    let mut properties = Map::new();
    properties.insert(
        "path_params".to_string(),
        path_parameters_schema(path_template, &parameters),
    );
    properties.insert("query".to_string(), query_parameters_schema(&parameters));
    let mut required = Vec::new();
    if let ContractBody::Json {
        schema,
        required: body_required,
    } = body
    {
        properties.insert("body".to_string(), schema);
        if body_required {
            required.push(Value::String("body".to_string()));
        }
    }
    json!({
        "type": "object",
        "description": format!(
            "MCP argument envelope for {} {}.",
            method.to_ascii_uppercase(),
            path_template
        ),
        "properties": properties,
        "required": required,
        "additionalProperties": true
    })
}

fn generic_input_schema() -> Value {
    json!({
        "type": "object",
        "description": GENERIC_ENVELOPE_DESCRIPTION,
        "properties": {
            "path_params": {
                "type": "object",
                "description": "Path parameter values substituted into the operation path template.",
                "additionalProperties": { "type": "string" }
            },
            "query": {
                "type": "object",
                "description": "Optional REST query parameters.",
                "additionalProperties": true
            },
            "body": {
                "description": "Optional JSON request body, validated by the REST layer."
            }
        },
        "additionalProperties": true
    })
}

fn contract_body(operation: &Value) -> ContractBody {
    let Some(request_body) = operation.get("requestBody") else {
        return ContractBody::None;
    };
    let Some(schema) = request_body
        .get("content")
        .and_then(|content| content.get("application/json"))
        .and_then(|media| media.get("schema"))
    else {
        return ContractBody::Unsupported;
    };
    ContractBody::Json {
        schema: resolved_schema(schema, &mut Vec::new(), 0),
        required: request_body
            .get("required")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

fn operation_parameters<'a>(path_parameters: &'a [Value], operation: &'a Value) -> Vec<&'a Value> {
    let mut parameters = path_parameters.iter().collect::<Vec<_>>();
    if let Some(operation_level) = operation.get("parameters").and_then(Value::as_array) {
        parameters.extend(operation_level.iter());
    }
    parameters
}

fn declared_parameter<'a>(
    parameters: &[&'a Value],
    name: &str,
    location: &str,
) -> Option<&'a Value> {
    parameters.iter().rev().copied().find(|parameter| {
        parameter.get("name").and_then(Value::as_str) == Some(name)
            && parameter.get("in").and_then(Value::as_str) == Some(location)
    })
}

fn parameter_schema(parameters: &[&Value], name: &str, location: &str) -> Option<Value> {
    let schema = declared_parameter(parameters, name, location)?.get("schema")?;
    Some(resolved_schema(schema, &mut Vec::new(), 0))
}

fn path_parameters_schema(path_template: &str, parameters: &[&Value]) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for key in path_template_keys(path_template) {
        properties.insert(
            key.to_string(),
            parameter_schema(parameters, key, "path")
                .unwrap_or_else(|| json!({ "type": "string" })),
        );
        required.push(Value::String(key.to_string()));
    }
    json!({
        "type": "object",
        "description": "Path parameters substituted into the operation path template.",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

fn query_parameters_schema(parameters: &[&Value]) -> Value {
    let mut properties = Map::new();
    for parameter in parameters {
        if parameter.get("in").and_then(Value::as_str) != Some("query") {
            continue;
        }
        let Some(name) = parameter.get("name").and_then(Value::as_str) else {
            continue;
        };
        let schema = parameter
            .get("schema")
            .map(|schema| resolved_schema(schema, &mut Vec::new(), 0))
            .unwrap_or_else(|| json!({ "type": "string" }));
        properties.insert(name.to_string(), schema);
    }
    json!({
        "type": "object",
        "description": "Optional REST query parameters declared by the contract.",
        "properties": properties,
        "additionalProperties": true
    })
}

/// Resolve the component references one schema reaches and merge `allOf` parts.
///
/// `oneOf` and `anyOf` branches are copied verbatim: the MCP argument validator
/// does not evaluate them, and choosing one branch would reject valid calls.
fn resolved_schema(schema: &Value, seen: &mut Vec<String>, depth: usize) -> Value {
    if depth > OPENAPI_SCHEMA_DEPTH_LIMIT {
        return Value::Object(Map::new());
    }
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        return resolved_component(reference, seen, depth);
    }
    match schema {
        Value::Object(object) if object.contains_key("allOf") => {
            resolved_all_of(object, seen, depth)
        }
        Value::Object(object) => resolved_object_schema(object, seen, depth),
        other => other.clone(),
    }
}

fn resolved_component(reference: &str, seen: &mut Vec<String>, depth: usize) -> Value {
    let Some(name) = reference.strip_prefix(OPENAPI_COMPONENT_PREFIX) else {
        return Value::Object(Map::new());
    };
    if seen.iter().any(|seen| seen == name) {
        return Value::Object(Map::new());
    }
    let Some(component) = OPENAPI_DOCUMENT
        .get("components")
        .and_then(|components| components.get("schemas"))
        .and_then(|schemas| schemas.get(name))
    else {
        return Value::Object(Map::new());
    };
    seen.push(name.to_string());
    let resolved = resolved_schema(component, seen, depth + 1);
    seen.pop();
    resolved
}

fn resolved_all_of(object: &Map<String, Value>, seen: &mut Vec<String>, depth: usize) -> Value {
    let mut merged = Map::new();
    if let Some(parts) = object.get("allOf").and_then(Value::as_array) {
        for part in parts {
            merge_schema(&mut merged, &resolved_schema(part, seen, depth + 1));
        }
    }
    for (key, value) in object {
        if key == "allOf" {
            continue;
        }
        merged.entry(key.clone()).or_insert_with(|| value.clone());
    }
    Value::Object(merged)
}

fn merge_schema(target: &mut Map<String, Value>, part: &Value) {
    let Some(part) = part.as_object() else {
        return;
    };
    for (key, value) in part {
        match key.as_str() {
            "properties" => {
                let Some(value) = value.as_object() else {
                    continue;
                };
                match target.get_mut("properties").and_then(Value::as_object_mut) {
                    Some(properties) => {
                        for (name, schema) in value {
                            properties.insert(name.clone(), schema.clone());
                        }
                    }
                    None => {
                        target.insert("properties".to_string(), Value::Object(value.clone()));
                    }
                }
            }
            "required" => {
                let Some(value) = value.as_array() else {
                    continue;
                };
                let required = target
                    .entry("required".to_string())
                    .or_insert_with(|| Value::Array(Vec::new()));
                let Some(required) = required.as_array_mut() else {
                    continue;
                };
                for name in value {
                    if !required.contains(name) {
                        required.push(name.clone());
                    }
                }
            }
            "additionalProperties" if value == &Value::Bool(false) => {
                target.insert(key.clone(), value.clone());
            }
            _ => {
                target.entry(key.clone()).or_insert_with(|| value.clone());
            }
        }
    }
}

fn resolved_object_schema(
    object: &Map<String, Value>,
    seen: &mut Vec<String>,
    depth: usize,
) -> Value {
    let mut resolved = Map::new();
    for (key, value) in object {
        let resolved_value = match key.as_str() {
            "properties" => value.as_object().map_or_else(
                || value.clone(),
                |properties| {
                    Value::Object(
                        properties
                            .iter()
                            .map(|(name, schema)| {
                                (name.clone(), resolved_schema(schema, seen, depth + 1))
                            })
                            .collect(),
                    )
                },
            ),
            "items" | "additionalProperties" if value.is_object() => {
                resolved_schema(value, seen, depth + 1)
            }
            _ => value.clone(),
        };
        resolved.insert(key.clone(), resolved_value);
    }
    Value::Object(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::mcp::test_support::*;

    #[test]
    fn every_mcp_rest_endpoint_advertises_an_input_schema() {
        let endpoints = mcp_rest_endpoints();
        assert!(!endpoints.is_empty());
        for endpoint in endpoints {
            assert!(
                endpoint.input_schema.is_some(),
                "{} has no input schema",
                endpoint.operation_id
            );
        }
    }

    #[test]
    fn playbook_project_path_parameters_are_all_required() {
        let path_template = "/api/v1/playbooks/projects/{id}";
        let schema = operation_input_schema("GET", path_template);
        let required = schema
            .pointer("/properties/path_params/required")
            .and_then(json_array)
            .expect("path_params.required");
        assert_eq!(
            required,
            path_template_keys(path_template)
                .into_iter()
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            schema.pointer("/properties/path_params/properties/id/type"),
            Some(&json!("string"))
        );
        assert_eq!(
            schema.pointer("/properties/path_params/additionalProperties"),
            Some(&json!(false))
        );
    }

    #[test]
    fn playbook_project_list_and_create_follow_the_contract() {
        let list = operation_input_schema("GET", "/api/v1/playbooks/projects");
        assert_eq!(
            list.pointer("/properties/query/properties/limit/type"),
            Some(&json!(["integer", "null"]))
        );
        assert_eq!(
            list.pointer("/properties/path_params/required"),
            Some(&json!([]))
        );
        assert!(
            list.get("required")
                .is_some_and(|value| value == &json!([]))
        );

        let create = operation_input_schema("POST", "/api/v1/playbooks/projects");
        assert_eq!(create.pointer("/required"), Some(&json!(["body"])));
        let body = create
            .pointer("/properties/body")
            .expect("resolved CreateProjectRequest body");
        assert_eq!(body.get("required"), Some(&json!(["name", "source"])));
        assert!(body.pointer("/properties/name").is_some());
        assert!(body.pointer("/properties/description").is_some());
        assert!(body.pointer("/properties/source").is_some());
    }

    #[test]
    fn all_of_parts_are_merged_into_the_body_schema() {
        let schema = operation_input_schema("POST", "/api/v1/inventory/interfaces");
        let body = schema
            .pointer("/properties/body")
            .expect("resolved InterfaceCreatePayload body");

        // Declared by the inline `allOf` part.
        assert!(body.pointer("/properties/device_id").is_some());
        // Declared by the referenced `InterfaceNetworking` part.
        assert!(body.pointer("/properties/cable_end").is_some());
        let required = body
            .get("required")
            .and_then(json_array)
            .expect("merged required list");
        assert!(required.contains(&"device_id".to_string()));
        assert!(required.contains(&"name".to_string()));
    }

    #[test]
    fn non_json_bodies_fall_back_to_the_generic_envelope() {
        let upload =
            operation_input_schema("POST", "/api/v1/connections/sessions/{id}/files/upload");
        let absent = operation_input_schema("POST", "/api/v1/mcp/absent-operation");

        assert_eq!(upload, absent);
        assert!(upload.pointer("/properties/body/type").is_none());
        assert_eq!(
            upload.pointer("/properties/path_params/additionalProperties"),
            Some(&json!({ "type": "string" }))
        );
        assert_eq!(upload.get("additionalProperties"), Some(&json!(true)));
    }

    #[test]
    fn contract_refs_are_resolved_into_the_body_schema() {
        let schema = operation_input_schema("POST", "/api/v1/playbooks/projects");
        assert!(schema.pointer("/properties/body/$ref").is_none());
        assert!(
            schema
                .pointer("/properties/body/properties/name")
                .is_some_and(|name| name.get("type") == Some(&json!("string")))
        );
    }

    fn json_array(value: &Value) -> Option<Vec<String>> {
        value
            .as_array()?
            .iter()
            .map(Value::as_str)
            .map(|item| item.map(ToOwned::to_owned))
            .collect()
    }
}
