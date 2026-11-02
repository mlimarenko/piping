use serde_json::{Value, json};

use crate::contexts::sync::domain::Operator;
use crate::errors::AppError;

use super::SyncOperatorResponse;

pub(super) const SYNC_OPERATOR_NAMES: &[&str] = &[
    "identity",
    "literal",
    "concat",
    "split_index",
    "trim",
    "lower",
    "upper",
    "coerce",
    "lookup",
    "template",
    "dereference",
    "date_format",
    "tags_merge",
    "mac_normalize",
    "ipv4_prefix_canonical",
];

fn operator_category(name: &str) -> &'static str {
    match name {
        "identity" | "literal" => "identity_literal",
        "concat" | "split_index" | "trim" | "lower" | "upper" | "template" => "string",
        "coerce" => "type",
        "lookup" => "lookup",
        "dereference" => "reference",
        "date_format" | "tags_merge" | "mac_normalize" | "ipv4_prefix_canonical" => "normalization",
        _ => "string",
    }
}

fn operator_description(name: &str) -> &'static str {
    match name {
        "identity" => "Pass a field value unchanged",
        "literal" => "Use a constant value",
        "concat" => "Join fields or literals",
        "split_index" => "Extract one segment from a split string",
        "trim" => "Remove leading and trailing whitespace",
        "lower" => "Lowercase a string",
        "upper" => "Uppercase a string",
        "coerce" => "Cast a value to another scalar type",
        "lookup" => "Translate values through a lookup table",
        "template" => "Render a string template from fields",
        "dereference" => "Read an attribute from a referenced object",
        "date_format" => "Convert between date formats",
        "tags_merge" => "Merge two tag arrays",
        "mac_normalize" => "Normalize a MAC address",
        "ipv4_prefix_canonical" => "Canonicalize an IPv4 prefix",
        _ => "Transform field values",
    }
}

fn operator_args_schema(name: &str) -> Value {
    match name {
        "identity" | "trim" | "lower" | "upper" | "mac_normalize" | "ipv4_prefix_canonical" => {
            json!({ "required": ["field"] })
        }
        "literal" => json!({ "required": ["value"] }),
        "concat" => json!({ "required": ["operands"] }),
        "split_index" => json!({ "required": ["field", "separator", "index"] }),
        "coerce" => json!({ "required": ["field", "to"] }),
        "lookup" => json!({ "required": ["key_field", "table"] }),
        "template" => json!({ "required": ["pattern", "vars"] }),
        "dereference" => json!({ "required": ["fk_field", "target_attr"] }),
        "date_format" => json!({ "required": ["field", "format"] }),
        "tags_merge" => json!({ "required": ["field_a", "field_b"] }),
        _ => json!({}),
    }
}

pub(super) fn sync_operator_response(name: &str) -> Result<SyncOperatorResponse, AppError> {
    let Some(operator) = Operator::from_name(name) else {
        return Err(AppError::Internal(format!(
            "Unknown sync operator '{name}' in REST operator list"
        )));
    };
    let invertibility_hint = if operator.invertibility_hint() {
        "invertible"
    } else if operator_category(name) == "normalization" {
        "normalization"
    } else {
        "forward_only"
    };
    Ok(SyncOperatorResponse {
        name: name.to_string(),
        label: name.to_string(),
        description: operator_description(name).to_string(),
        category: operator_category(name).to_string(),
        invertibility_hint: invertibility_hint.to_string(),
        args_schema: operator_args_schema(name),
        suggested_reverse: operator.invertibility_hint().then(|| {
            json!({
                "operator": name,
                "args_transform": "same_args",
            })
        }),
    })
}
