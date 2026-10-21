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
}
