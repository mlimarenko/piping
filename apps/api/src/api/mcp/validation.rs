//! JSON-schema validation of MCP tool arguments, split out of `mcp.rs`.
//!
//! Moved verbatim; behaviour is unchanged.

use super::*;

pub(super) fn visible_tool_input_schema(
    request_context: &RequestContext,
    tool_name: &str,
) -> Option<Value> {
    visible_tools(request_context)
        .into_iter()
        .find(|tool| tool.get("name").and_then(Value::as_str) == Some(tool_name))
        .and_then(|tool| tool.get("inputSchema").cloned())
}

pub(super) fn validate_mcp_tool_arguments(schema: &Value, arguments: &Value) -> Result<(), String> {
    validate_mcp_value_against_schema(schema, arguments, "$")
}

pub(super) fn validate_mcp_value_against_schema(
    schema: &Value,
    value: &Value,
    path: &str,
) -> Result<(), String> {
    if !mcp_value_matches_type(schema, value) {
        let expected = schema
            .get("type")
            .map(Value::to_string)
            .unwrap_or_else(|| "\"object\"".to_string());
        return Err(format!("{path} must match schema type {expected}"));
    }

    if let Some(enum_values) = schema.get("enum").and_then(Value::as_array)
        && !enum_values.iter().any(|item| item == value)
    {
        return Err(format!("{path} must be one of the schema enum values"));
    }

    if let Some(max_len) = schema.get("maxLength").and_then(Value::as_u64)
        && value
            .as_str()
            .is_some_and(|text| text.chars().count() as u64 > max_len)
    {
        return Err(format!("{path} exceeds maxLength {max_len}"));
    }

    if value.is_number() {
        validate_mcp_numeric_bounds(schema, value, path)?;
    }

    if let Some(object) = value.as_object() {
        validate_mcp_object_against_schema(schema, object, path)?;
    }

    if let Some(array) = value.as_array()
        && let Some(item_schema) = schema.get("items")
    {
        for (index, item) in array.iter().enumerate() {
            validate_mcp_value_against_schema(item_schema, item, &format!("{path}[{index}]"))?;
        }
    }

    Ok(())
}

pub(super) fn mcp_value_matches_type(schema: &Value, value: &Value) -> bool {
    let Some(type_value) = schema.get("type") else {
        return true;
    };
    match type_value {
        Value::String(type_name) => mcp_value_matches_type_name(type_name, value),
        Value::Array(type_names) => type_names.iter().any(|item| {
            item.as_str()
                .is_some_and(|type_name| mcp_value_matches_type_name(type_name, value))
        }),
        _ => true,
    }
}

pub(super) fn mcp_value_matches_type_name(type_name: &str, value: &Value) -> bool {
    match type_name {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "number" => value.is_number(),
        "null" => value.is_null(),
        _ => true,
    }
}

pub(super) fn validate_mcp_numeric_bounds(
    schema: &Value,
    value: &Value,
    path: &str,
) -> Result<(), String> {
    let number = value
        .as_f64()
        .ok_or_else(|| format!("{path} must be a finite number"))?;
    if let Some(minimum) = schema.get("minimum").and_then(Value::as_f64)
        && number < minimum
    {
        return Err(format!("{path} must be >= {minimum}"));
    }
    if let Some(maximum) = schema.get("maximum").and_then(Value::as_f64)
        && number > maximum
    {
        return Err(format!("{path} must be <= {maximum}"));
    }
    Ok(())
}

pub(super) fn validate_mcp_object_against_schema(
    schema: &Value,
    object: &Map<String, Value>,
    path: &str,
) -> Result<(), String> {
    let properties = schema.get("properties").and_then(Value::as_object);
    validate_mcp_required_properties(schema, object, path)?;
    validate_mcp_additional_properties(schema, properties, object, path)?;
    if let Some(properties) = properties {
        validate_mcp_property_values(properties, object, path)?;
    }
    Ok(())
}

fn validate_mcp_required_properties(
    schema: &Value,
    object: &Map<String, Value>,
    path: &str,
) -> Result<(), String> {
    let Some(required) = schema.get("required").and_then(Value::as_array) else {
        return Ok(());
    };
    for key in required.iter().filter_map(Value::as_str) {
        if !object.contains_key(key) {
            return Err(format!("{path}.{key} is required"));
        }
    }
    Ok(())
}

fn validate_mcp_additional_properties(
    schema: &Value,
    properties: Option<&Map<String, Value>>,
    object: &Map<String, Value>,
    path: &str,
) -> Result<(), String> {
    if schema.get("additionalProperties") != Some(&Value::Bool(false)) {
        return Ok(());
    }
    let allowed = properties
        .map(|properties| properties.keys().cloned().collect::<HashSet<_>>())
        .unwrap_or_default();
    for key in object.keys() {
        if !allowed.contains(key) {
            return Err(format!("{path}.{key} is not a supported argument"));
        }
    }
    Ok(())
}

fn validate_mcp_property_values(
    properties: &Map<String, Value>,
    object: &Map<String, Value>,
    path: &str,
) -> Result<(), String> {
    for (key, property_schema) in properties {
        if let Some(child) = object.get(key) {
            validate_mcp_value_against_schema(property_schema, child, &format!("{path}.{key}"))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_type_mismatch_reports_path_and_expected_type() {
        let error = validate_mcp_tool_arguments(&json!({"type": "string"}), &json!(7)).unwrap_err();
        assert!(error.contains("$ must match schema type"), "{error}");
    }

    #[test]
    fn enum_max_length_and_numeric_bounds_are_enforced() {
        let enum_schema = json!({"type": "string", "enum": ["a", "b"]});
        assert!(
            validate_mcp_tool_arguments(&enum_schema, &json!("c"))
                .unwrap_err()
                .contains("enum")
        );

        let length_schema = json!({"type": "string", "maxLength": 2});
        assert!(
            validate_mcp_tool_arguments(&length_schema, &json!("abc"))
                .unwrap_err()
                .contains("maxLength 2")
        );

        let bounded = json!({"type": "integer", "minimum": 5, "maximum": 10});
        assert!(
            validate_mcp_tool_arguments(&bounded, &json!(1))
                .unwrap_err()
                .contains("must be >= 5")
        );
        assert!(
            validate_mcp_tool_arguments(&bounded, &json!(11))
                .unwrap_err()
                .contains("must be <= 10")
        );
        assert!(validate_mcp_tool_arguments(&bounded, &json!(7)).is_ok());
    }

    #[test]
    fn array_items_are_validated_recursively() {
        let schema = json!({"type": "array", "items": {"type": "string"}});
        assert!(
            validate_mcp_tool_arguments(&schema, &json!(["ok", 2]))
                .unwrap_err()
                .contains("$[1]")
        );
        assert!(validate_mcp_tool_arguments(&schema, &json!(["ok"])).is_ok());
    }

    #[test]
    fn union_type_names_and_missing_type_accept_the_value() {
        assert!(validate_mcp_tool_arguments(&json!({}), &json!(1)).is_ok());
        let union = json!({"type": ["string", "null"]});
        assert!(validate_mcp_tool_arguments(&union, &json!(null)).is_ok());
        assert!(validate_mcp_tool_arguments(&union, &json!(1)).is_err());
    }

    #[test]
    fn type_name_predicates_match_json_kinds() {
        assert!(mcp_value_matches_type_name("object", &json!({})));
        assert!(mcp_value_matches_type_name("array", &json!([])));
        assert!(mcp_value_matches_type_name("string", &json!("s")));
        assert!(mcp_value_matches_type_name("boolean", &json!(true)));
        assert!(mcp_value_matches_type_name("integer", &json!(1)));
        assert!(!mcp_value_matches_type_name("integer", &json!(1.5)));
        assert!(mcp_value_matches_type_name("number", &json!(1.5)));
        assert!(mcp_value_matches_type_name("null", &json!(null)));
        assert!(mcp_value_matches_type_name("unknown", &json!(1)));
    }

    #[test]
    fn object_rules_cover_required_additional_and_nested() {
        let schema = json!({
            "type": "object",
            "properties": {"name": {"type": "string"}},
            "required": ["name"],
            "additionalProperties": false
        });
        assert!(
            validate_mcp_tool_arguments(&schema, &json!({}))
                .unwrap_err()
                .contains("$.name is required")
        );
        assert!(
            validate_mcp_tool_arguments(&schema, &json!({"name": "x", "extra": 1}))
                .unwrap_err()
                .contains("$.extra is not a supported argument")
        );
        assert!(
            validate_mcp_tool_arguments(&schema, &json!({"name": 7}))
                .unwrap_err()
                .contains("$.name must match schema type")
        );
        assert!(validate_mcp_tool_arguments(&schema, &json!({"name": "x"})).is_ok());
    }
}
