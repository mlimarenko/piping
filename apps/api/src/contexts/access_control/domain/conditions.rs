use std::collections::HashMap;
use std::net::IpAddr;

use chrono::{DateTime, Utc};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub enum ConditionExpression {
    And(Vec<ConditionExpression>),
    Or(Vec<ConditionExpression>),
    Condition(Condition),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Condition {
    IpRange {
        cidr: String,
    },
    TimeWindow {
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    },
    Attributes {
        attributes: HashMap<String, Value>,
    },
    Unknown(Value),
}

pub fn parse_constraints(value: &Value) -> ConditionExpression {
    match value {
        Value::Object(map) => {
            if let Some(and_value) = map.get("and") {
                return parse_logical_list(ConditionExpression::And, and_value, value);
            }
            if let Some(or_value) = map.get("or") {
                return parse_logical_list(ConditionExpression::Or, or_value, value);
            }
            ConditionExpression::Condition(parse_condition(value))
        }
        Value::Array(items) => {
            let expressions = items.iter().map(parse_constraints).collect();
            ConditionExpression::And(expressions)
        }
        _ => ConditionExpression::Condition(parse_condition(value)),
    }
}

/// Validate a raw constraints JSON value at write time: every leaf must parse to
/// a known condition shape (`ip_range` / `time_window` / `attributes`). Returns
/// `Err` with a human message if any leaf is unrecognized or malformed, so the
/// API can reject it with 400 instead of silently persisting a policy whose
/// constraint always evaluates to `Unknown` → deny.
pub fn validate_constraints(value: &Value) -> Result<(), String> {
    fn walk(expression: &ConditionExpression) -> Result<(), String> {
        match expression {
            ConditionExpression::And(items) | ConditionExpression::Or(items) => {
                items.iter().try_for_each(walk)
            }
            ConditionExpression::Condition(Condition::Unknown(raw)) => Err(format!(
                "unrecognized constraint shape (expected ip_range/time_window/attributes): {raw}"
            )),
            ConditionExpression::Condition(_) => Ok(()),
        }
    }
    walk(&parse_constraints(value))
}

/// Collect every attribute key referenced by an `attributes` condition anywhere
/// in a constraint expression (deduplicated). Used by the policy write path to
/// reject a constraint that scopes on an attribute key no enforcement site
/// supplies — the `role` vs `role_id` footgun — instead of persisting a policy
/// whose constraint can never match.
pub fn constraint_attribute_keys_used(value: &Value) -> Vec<String> {
    fn walk(expression: &ConditionExpression, out: &mut Vec<String>) {
        match expression {
            ConditionExpression::And(items) | ConditionExpression::Or(items) => {
                items.iter().for_each(|item| walk(item, out));
            }
            ConditionExpression::Condition(Condition::Attributes { attributes }) => {
                for key in attributes.keys() {
                    if !out.contains(key) {
                        out.push(key.clone());
                    }
                }
            }
            ConditionExpression::Condition(_) => {}
        }
    }
    let mut keys = Vec::new();
    walk(&parse_constraints(value), &mut keys);
    keys
}

/// Attribute-bearing leaves in a constraint expression that are not guarded by a
/// supported `object_type` discriminator, as their sorted key lists.
///
/// A leaf is guarded when it carries a supported `object_type` itself, or when
/// an enclosing `and` contains a sibling sub-expression that guarantees one.
/// `or` never propagates a guard: every branch must be guarded on its own,
/// because any single branch can satisfy the whole expression. `and` guarantees
/// when any child does; `or` guarantees only when every child does.
///
/// `is_supported_object_type` is supplied by the policy vocabulary, so this
/// parser stays free of resource-specific knowledge. The policy write path
/// rejects a non-empty result: an unguarded branch would evaluate `attributes`
/// against a context that no legacy route populates, making a deny silently
/// inert.
pub fn unguarded_attribute_branches(
    value: &Value,
    is_supported_object_type: &dyn Fn(&str) -> bool,
) -> Vec<Vec<String>> {
    fn guarantees(expression: &ConditionExpression, supported: &dyn Fn(&str) -> bool) -> bool {
        match expression {
            ConditionExpression::And(items) => items.iter().any(|item| guarantees(item, supported)),
            ConditionExpression::Or(items) => {
                !items.is_empty() && items.iter().all(|item| guarantees(item, supported))
            }
            ConditionExpression::Condition(Condition::Attributes { attributes }) => attributes
                .get("object_type")
                .and_then(Value::as_str)
                .is_some_and(|token| supported(token)),
            ConditionExpression::Condition(_) => false,
        }
    }

    fn walk(
        expression: &ConditionExpression,
        guarded: bool,
        supported: &dyn Fn(&str) -> bool,
        out: &mut Vec<Vec<String>>,
    ) {
        match expression {
            ConditionExpression::And(items) => {
                let sibling_guard = items.iter().any(|item| guarantees(item, supported));
                for item in items {
                    walk(item, guarded || sibling_guard, supported, out);
                }
            }
            ConditionExpression::Or(items) => {
                for item in items {
                    walk(item, guarded, supported, out);
                }
            }
            ConditionExpression::Condition(Condition::Attributes { attributes }) => {
                let self_guard = attributes
                    .get("object_type")
                    .and_then(Value::as_str)
                    .is_some_and(|token| supported(token));
                if !(guarded || self_guard) {
                    let mut keys: Vec<String> = attributes.keys().cloned().collect();
                    keys.sort_unstable();
                    out.push(keys);
                }
            }
            ConditionExpression::Condition(_) => {}
        }
    }

    let mut unguarded = Vec::new();
    walk(
        &parse_constraints(value),
        false,
        is_supported_object_type,
        &mut unguarded,
    );
    unguarded
}

/// Collect every `attributes` predicate as `(key, value)` pairs from the parsed
/// constraint expression, in every supported shape (`attributes`, `and`, `or`,
/// and a bare array of expressions).
///
/// Used by the policy write path where the VALUE, not only the key, determines
/// whether an enforcement site can honour the constraint.
pub fn constraint_attributes_used(value: &Value) -> Vec<(String, Value)> {
    fn walk(expression: &ConditionExpression, out: &mut Vec<(String, Value)>) {
        match expression {
            ConditionExpression::And(items) | ConditionExpression::Or(items) => {
                items.iter().for_each(|item| walk(item, out));
            }
            ConditionExpression::Condition(Condition::Attributes { attributes }) => {
                for (key, value) in attributes {
                    out.push((key.clone(), value.clone()));
                }
            }
            ConditionExpression::Condition(_) => {}
        }
    }
    let mut pairs = Vec::new();
    walk(&parse_constraints(value), &mut pairs);
    pairs
}

pub fn evaluate_constraints(expression: &ConditionExpression, context: &Value) -> bool {
    match expression {
        ConditionExpression::And(items) => {
            items.iter().all(|item| evaluate_constraints(item, context))
        }
        ConditionExpression::Or(items) => {
            items.iter().any(|item| evaluate_constraints(item, context))
        }
        ConditionExpression::Condition(condition) => evaluate_condition(condition, context),
    }
}

fn parse_logical_list(
    constructor: fn(Vec<ConditionExpression>) -> ConditionExpression,
    value: &Value,
    fallback: &Value,
) -> ConditionExpression {
    match value {
        Value::Array(items) => {
            let expressions = items.iter().map(parse_constraints).collect();
            constructor(expressions)
        }
        _ => ConditionExpression::Condition(parse_condition(fallback)),
    }
}

fn parse_condition(value: &Value) -> Condition {
    if let Value::Object(map) = value {
        if let Some(cidr) = map.get("ip_range").and_then(|value| value.as_str()) {
            if cidr.trim().is_empty() {
                return Condition::Unknown(value.clone());
            }
            return Condition::IpRange {
                cidr: cidr.to_string(),
            };
        }

        if let Some(window) = map.get("time_window")
            && let Value::Object(window_map) = window
        {
            let start = window_map
                .get("start")
                .and_then(|value| value.as_str())
                .and_then(parse_rfc3339);
            let end = window_map
                .get("end")
                .and_then(|value| value.as_str())
                .and_then(parse_rfc3339);

            if let (Some(start), Some(end)) = (start, end) {
                return Condition::TimeWindow { start, end };
            }
        }

        if let Some(attributes) = map.get("attributes")
            && let Value::Object(attribute_map) = attributes
        {
            let attributes = attribute_map
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            return Condition::Attributes { attributes };
        }
    }

    Condition::Unknown(value.clone())
}

fn evaluate_condition(condition: &Condition, context: &Value) -> bool {
    match condition {
        Condition::IpRange { cidr } => {
            let ip = context
                .get("ip")
                .and_then(|value| value.as_str())
                .and_then(|value| value.parse::<IpAddr>().ok());
            let ip = match ip {
                Some(ip) => ip,
                None => return false,
            };
            ip_in_cidr(ip, cidr)
        }
        Condition::TimeWindow { start, end } => {
            let timestamp = context
                .get("timestamp")
                .and_then(|value| value.as_str())
                .and_then(parse_rfc3339);
            let timestamp = match timestamp {
                Some(timestamp) => timestamp,
                None => return false,
            };
            timestamp >= *start && timestamp <= *end
        }
        Condition::Attributes { attributes } => {
            let context_attributes = context
                .get("attributes")
                .and_then(|value| value.as_object());
            let context_attributes = match context_attributes {
                Some(attrs) => attrs,
                None => return false,
            };

            attributes.iter().all(|(key, value)| {
                context_attributes
                    .get(key)
                    .map(|context_value| context_value == value)
                    .unwrap_or(false)
            })
        }
        Condition::Unknown(_) => false,
    }
}

fn parse_rfc3339(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|parsed| parsed.with_timezone(&Utc))
}

fn ip_in_cidr(ip: IpAddr, cidr: &str) -> bool {
    let mut parts = cidr.splitn(2, '/');
    let network = parts.next().unwrap_or("").trim();
    let prefix = parts.next().and_then(|value| value.parse::<u8>().ok());

    if network.is_empty() {
        return false;
    }

    let network = network.parse::<IpAddr>().ok();
    let prefix = match prefix {
        Some(prefix) => prefix,
        None => return false,
    };

    match (ip, network) {
        (IpAddr::V4(ip), Some(IpAddr::V4(network))) => {
            if prefix > 32 {
                return false;
            }
            let ip = u32::from(ip);
            let network = u32::from(network);
            let mask = if prefix == 0 {
                0
            } else {
                u32::MAX.checked_shl(u32::from(32 - prefix)).unwrap_or(0)
            };
            (ip & mask) == (network & mask)
        }
        (IpAddr::V6(ip), Some(IpAddr::V6(network))) => {
            if prefix > 128 {
                return false;
            }
            let ip = u128::from(ip);
            let network = u128::from(network);
            let mask = if prefix == 0 {
                0
            } else {
                u128::MAX.checked_shl(u32::from(128 - prefix)).unwrap_or(0)
            };
            (ip & mask) == (network & mask)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn collects_attribute_keys_from_nested_and_or() {
        let constraint = json!({
            "and": [
                {"attributes": {"role_id": "ru3"}},
                {"or": [
                    {"attributes": {"site_id": "s1"}},
                    {"attributes": {"site_id": "s2", "protocol": "vnc"}}
                ]},
                {"ip_range": "10.0.0.0/8"}
            ]
        });
        let mut keys = constraint_attribute_keys_used(&constraint);
        keys.sort();
        assert_eq!(keys, vec!["protocol", "role_id", "site_id"]);
    }

    #[test]
    fn no_attribute_conditions_yields_no_keys() {
        let constraint = json!({"time_window": {"start": "2026-01-01T00:00:00Z", "end": "2030-01-01T00:00:00Z"}});
        assert!(constraint_attribute_keys_used(&constraint).is_empty());
    }

    #[test]
    fn collects_attribute_values_from_every_supported_expression_shape() {
        let nested = json!({
            "and": [
                {"attributes": {"object_type": "dcim.powerporttemplate"}},
                {"or": [
                    {"attributes": {"object_type": "dcim.devicetype"}},
                    {"ip_range": "10.0.0.0/8"}
                ]}
            ]
        });
        let values: Vec<String> = constraint_attributes_used(&nested)
            .into_iter()
            .filter(|(key, _)| key == "object_type")
            .filter_map(|(_, value)| value.as_str().map(str::to_string))
            .collect();
        assert_eq!(
            values,
            vec!["dcim.powerporttemplate", "dcim.devicetype"],
            "values are read from the parsed and/or expression, not from raw text"
        );

        let bare = json!([{"attributes": {"object_type": "dcim.interfacetemplate"}}]);
        assert_eq!(
            constraint_attributes_used(&bare),
            vec![("object_type".to_string(), json!("dcim.interfacetemplate"))]
        );
    }

    #[test]
    fn guard_coverage_is_checked_per_attribute_branch() {
        let supported = |token: &str| token == "dcim.consoleporttemplate";

        // An `or` sibling does not guard its sibling: each branch must stand alone.
        let unguarded_or = json!({"or": [
            {"attributes": {"object_type": "dcim.consoleporttemplate"}},
            {"attributes": {"manufacturer_id": "m1"}}
        ]});
        assert_eq!(
            unguarded_attribute_branches(&unguarded_or, &supported),
            vec![vec!["manufacturer_id".to_string()]]
        );

        // Every branch guarded on its own is accepted.
        let guarded_or = json!({"or": [
            {"attributes": {"object_type": "dcim.consoleporttemplate", "device_type_id": "d1"}},
            {"attributes": {"object_type": "dcim.consoleporttemplate", "parent_id": "p1"}}
        ]});
        assert!(unguarded_attribute_branches(&guarded_or, &supported).is_empty());

        // A split `and` guard covers its attribute siblings.
        let split_and = json!({"and": [
            {"attributes": {"object_type": "dcim.consoleporttemplate"}},
            {"attributes": {"manufacturer_id": "m1"}}
        ]});
        assert!(unguarded_attribute_branches(&split_and, &supported).is_empty());

        // An `and` guard also covers a nested `or` of plain attribute branches.
        let guarded_nested_or = json!({"and": [
            {"attributes": {"object_type": "dcim.consoleporttemplate"}},
            {"or": [
                {"attributes": {"manufacturer_id": "m1"}},
                {"attributes": {"role_id": "r1"}}
            ]}
        ]});
        assert!(unguarded_attribute_branches(&guarded_nested_or, &supported).is_empty());

        // An unsupported literal is not a guard, so both branches stay unguarded.
        let unsupported = json!({"or": [
            {"attributes": {"object_type": "dcim.devicetype"}},
            {"attributes": {"manufacturer_id": "m1"}}
        ]});
        assert_eq!(
            unguarded_attribute_branches(&unsupported, &supported).len(),
            2,
            "an unsupported discriminator guards nothing, including its own branch"
        );

        // A bare attribute leaf with no discriminator is unguarded.
        assert_eq!(
            unguarded_attribute_branches(&json!({"attributes": {"role_id": "r1"}}), &supported),
            vec![vec!["role_id".to_string()]]
        );
    }
}
