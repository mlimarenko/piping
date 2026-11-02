//! NetBox-style condition sets for filtering outbound webhook deliveries.
//!
//! A [`ConditionSet`] is a recursive boolean tree of `and`/`or` groups whose
//! leaves compare a dot-path attribute of the event JSON against a literal
//! value with an operator. A subscription with no conditions (`NULL`) always
//! matches; a present-but-empty set is rejected at validation time.
//!
//! Shape (mirrors NetBox):
//! ```json
//! { "and": [
//!     { "attr": "event_type", "op": "eq", "value": "inventory.tenant.created" },
//!     { "or": [
//!         { "attr": "metadata.name", "op": "contains", "value": "prod" },
//!         { "attr": "metadata.priority", "op": "gte", "value": 5 }
//!     ] }
//! ] }
//! ```

use regex::Regex;
use serde_json::Value;

use crate::errors::AppError;

const MAX_DEPTH: usize = 16;

/// A boolean group: every group is either an `and` or an `or` over members.
#[derive(Debug, Clone)]
enum Group {
    And(Vec<Node>),
    Or(Vec<Node>),
}

/// A node is either a nested group or a single comparison leaf.
#[derive(Debug, Clone)]
enum Node {
    Group(Group),
    Leaf(Condition),
}

/// A single attribute comparison.
#[derive(Debug, Clone)]
struct Condition {
    attr: String,
    op: Operator,
    value: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Operator {
    Eq,
    Ne,
    Gt,
    Gte,
    Lt,
    Lte,
    In,
    Contains,
    Regex,
}

impl Operator {
    fn parse(raw: &str) -> Result<Self, AppError> {
        Ok(match raw {
            "eq" => Operator::Eq,
            "ne" => Operator::Ne,
            "gt" => Operator::Gt,
            "gte" => Operator::Gte,
            "lt" => Operator::Lt,
            "lte" => Operator::Lte,
            "in" => Operator::In,
            "contains" => Operator::Contains,
            "regex" => Operator::Regex,
            other => {
                return Err(AppError::Validation(format!(
                    "unknown condition operator '{other}'"
                )));
            }
        })
    }
}

/// A parsed, validated condition tree ready for evaluation.
#[derive(Debug, Clone)]
pub struct ConditionSet {
    root: Group,
}

impl ConditionSet {
    /// Parse and validate a raw JSON condition set, rejecting malformed input.
    pub fn parse(raw: &Value) -> Result<Self, AppError> {
        let root = parse_group(raw, 0)?;
        Ok(Self { root })
    }

    /// Validate a raw JSON condition set without retaining the parsed tree.
    pub fn validate(raw: &Value) -> Result<(), AppError> {
        Self::parse(raw).map(|_| ())
    }

    /// Evaluate the condition tree against the event JSON.
    #[must_use]
    pub fn evaluate(&self, event: &Value) -> bool {
        eval_group(&self.root, event)
    }
}

fn parse_group(raw: &Value, depth: usize) -> Result<Group, AppError> {
    if depth > MAX_DEPTH {
        return Err(AppError::Validation(
            "condition set nested too deeply".to_string(),
        ));
    }
    let obj = raw
        .as_object()
        .ok_or_else(|| AppError::Validation("condition group must be a JSON object".to_string()))?;

    let has_and = obj.contains_key("and");
    let has_or = obj.contains_key("or");
    if has_and == has_or {
        return Err(AppError::Validation(
            "condition group must contain exactly one of 'and' or 'or'".to_string(),
        ));
    }

    let (key, is_and) = if has_and {
        ("and", true)
    } else {
        ("or", false)
    };
    let members = obj
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::Validation(format!("condition group '{key}' must be an array")))?;
    if members.is_empty() {
        return Err(AppError::Validation(format!(
            "condition group '{key}' must not be empty"
        )));
    }

    let nodes = members
        .iter()
        .map(|member| parse_node(member, depth + 1))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(if is_and {
        Group::And(nodes)
    } else {
        Group::Or(nodes)
    })
}

fn parse_node(raw: &Value, depth: usize) -> Result<Node, AppError> {
    let obj = raw.as_object().ok_or_else(|| {
        AppError::Validation("condition member must be a JSON object".to_string())
    })?;

    if obj.contains_key("and") || obj.contains_key("or") {
        return Ok(Node::Group(parse_group(raw, depth)?));
    }

    let attr = obj
        .get("attr")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::Validation("condition leaf requires string 'attr'".to_string()))?;
    if attr.trim().is_empty() {
        return Err(AppError::Validation(
            "condition leaf 'attr' must not be empty".to_string(),
        ));
    }
    let op_raw = obj
        .get("op")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::Validation("condition leaf requires string 'op'".to_string()))?;
    let op = Operator::parse(op_raw)?;
    let value = obj
        .get("value")
        .cloned()
        .ok_or_else(|| AppError::Validation("condition leaf requires a 'value'".to_string()))?;

    if op == Operator::In && !value.is_array() {
        return Err(AppError::Validation(
            "condition operator 'in' requires an array 'value'".to_string(),
        ));
    }
    if op == Operator::Regex {
        let pattern = value.as_str().ok_or_else(|| {
            AppError::Validation("condition operator 'regex' requires a string 'value'".to_string())
        })?;
        Regex::new(pattern).map_err(|e| {
            AppError::Validation(format!("condition 'regex' value is not a valid regex: {e}"))
        })?;
    }

    Ok(Node::Leaf(Condition {
        attr: attr.to_string(),
        op,
        value,
    }))
}

fn eval_group(group: &Group, event: &Value) -> bool {
    match group {
        Group::And(nodes) => nodes.iter().all(|n| eval_node(n, event)),
        Group::Or(nodes) => nodes.iter().any(|n| eval_node(n, event)),
    }
}

fn eval_node(node: &Node, event: &Value) -> bool {
    match node {
        Node::Group(group) => eval_group(group, event),
        Node::Leaf(cond) => eval_leaf(cond, event),
    }
}

fn eval_leaf(cond: &Condition, event: &Value) -> bool {
    let resolved = resolve_path(event, &cond.attr);
    match cond.op {
        Operator::Eq => resolved.is_some_and(|v| v == &cond.value),
        Operator::Ne => resolved.is_none_or(|v| v != &cond.value),
        Operator::Gt => compare_numbers(resolved, &cond.value, |o| o.is_gt()),
        Operator::Gte => compare_numbers(resolved, &cond.value, |o| o.is_ge()),
        Operator::Lt => compare_numbers(resolved, &cond.value, |o| o.is_lt()),
        Operator::Lte => compare_numbers(resolved, &cond.value, |o| o.is_le()),
        Operator::In => match (resolved, cond.value.as_array()) {
            (Some(v), Some(arr)) => arr.iter().any(|item| item == v),
            _ => false,
        },
        Operator::Contains => eval_contains(resolved, &cond.value),
        Operator::Regex => match (resolved.and_then(Value::as_str), cond.value.as_str()) {
            (Some(haystack), Some(pattern)) => Regex::new(pattern)
                .map(|re| re.is_match(haystack))
                .unwrap_or(false),
            _ => false,
        },
    }
}

fn compare_numbers(
    resolved: Option<&Value>,
    expected: &Value,
    pred: impl Fn(std::cmp::Ordering) -> bool,
) -> bool {
    match (resolved.and_then(Value::as_f64), expected.as_f64()) {
        (Some(a), Some(b)) => a.partial_cmp(&b).map(pred).unwrap_or(false),
        _ => false,
    }
}

fn eval_contains(resolved: Option<&Value>, needle: &Value) -> bool {
    match resolved {
        Some(Value::String(haystack)) => needle
            .as_str()
            .map(|n| haystack.contains(n))
            .unwrap_or(false),
        Some(Value::Array(items)) => items.iter().any(|item| item == needle),
        _ => false,
    }
}

/// Resolve a dot-separated path against a JSON value. Numeric segments index
/// into arrays; all other segments index into objects.
fn resolve_path<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = root;
    for segment in path.split('.') {
        current = match current {
            Value::Object(map) => map.get(segment)?,
            Value::Array(items) => {
                let idx: usize = segment.parse().ok()?;
                items.get(idx)?
            }
            _ => return None,
        };
    }
    Some(current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event() -> Value {
        json!({
            "event_type": "inventory.tenant.created",
            "target_type": "tenant",
            "target_id": "11111111-1111-1111-1111-111111111111",
            "metadata": {
                "name": "prod-tenant",
                "priority": 7,
                "tags": ["prod", "critical"],
                "nested": { "region": "eu" }
            }
        })
    }

    #[test]
    fn eq_matches_top_level() {
        let set = ConditionSet::parse(&json!({
            "and": [{ "attr": "event_type", "op": "eq", "value": "inventory.tenant.created" }]
        }))
        .unwrap();
        assert!(set.evaluate(&event()));
    }

    #[test]
    fn eq_mismatch_fails() {
        let set = ConditionSet::parse(&json!({
            "and": [{ "attr": "event_type", "op": "eq", "value": "other" }]
        }))
        .unwrap();
        assert!(!set.evaluate(&event()));
    }

    #[test]
    fn ne_on_missing_path_is_true() {
        let set = ConditionSet::parse(&json!({
            "and": [{ "attr": "metadata.absent", "op": "ne", "value": "x" }]
        }))
        .unwrap();
        assert!(set.evaluate(&event()));
    }

    #[test]
    fn numeric_comparisons() {
        let gte = ConditionSet::parse(&json!({
            "and": [{ "attr": "metadata.priority", "op": "gte", "value": 7 }]
        }))
        .unwrap();
        assert!(gte.evaluate(&event()));

        let gt = ConditionSet::parse(&json!({
            "and": [{ "attr": "metadata.priority", "op": "gt", "value": 7 }]
        }))
        .unwrap();
        assert!(!gt.evaluate(&event()));

        let lt = ConditionSet::parse(&json!({
            "and": [{ "attr": "metadata.priority", "op": "lt", "value": 10 }]
        }))
        .unwrap();
        assert!(lt.evaluate(&event()));
    }

    #[test]
    fn in_operator() {
        let set = ConditionSet::parse(&json!({
            "and": [{ "attr": "metadata.name", "op": "in", "value": ["a", "prod-tenant"] }]
        }))
        .unwrap();
        assert!(set.evaluate(&event()));
    }

    #[test]
    fn contains_string_and_array() {
        let string_set = ConditionSet::parse(&json!({
            "and": [{ "attr": "metadata.name", "op": "contains", "value": "prod" }]
        }))
        .unwrap();
        assert!(string_set.evaluate(&event()));

        let array_set = ConditionSet::parse(&json!({
            "and": [{ "attr": "metadata.tags", "op": "contains", "value": "critical" }]
        }))
        .unwrap();
        assert!(array_set.evaluate(&event()));

        let array_miss = ConditionSet::parse(&json!({
            "and": [{ "attr": "metadata.tags", "op": "contains", "value": "absent" }]
        }))
        .unwrap();
        assert!(!array_miss.evaluate(&event()));
    }

    #[test]
    fn regex_operator() {
        let set = ConditionSet::parse(&json!({
            "and": [{ "attr": "event_type", "op": "regex", "value": "^inventory\\..*\\.created$" }]
        }))
        .unwrap();
        assert!(set.evaluate(&event()));
    }

    #[test]
    fn nested_and_or() {
        let set = ConditionSet::parse(&json!({
            "and": [
                { "attr": "target_type", "op": "eq", "value": "tenant" },
                { "or": [
                    { "attr": "metadata.priority", "op": "gte", "value": 100 },
                    { "attr": "metadata.nested.region", "op": "eq", "value": "eu" }
                ] }
            ]
        }))
        .unwrap();
        assert!(set.evaluate(&event()));
    }

    #[test]
    fn nested_or_all_false() {
        let set = ConditionSet::parse(&json!({
            "and": [
                { "or": [
                    { "attr": "metadata.priority", "op": "gte", "value": 100 },
                    { "attr": "metadata.nested.region", "op": "eq", "value": "us" }
                ] }
            ]
        }))
        .unwrap();
        assert!(!set.evaluate(&event()));
    }

    #[test]
    fn array_index_path() {
        let set = ConditionSet::parse(&json!({
            "and": [{ "attr": "metadata.tags.0", "op": "eq", "value": "prod" }]
        }))
        .unwrap();
        assert!(set.evaluate(&event()));
    }

    #[test]
    fn rejects_non_object_root() {
        assert!(ConditionSet::validate(&json!([])).is_err());
        assert!(ConditionSet::validate(&json!("x")).is_err());
    }

    #[test]
    fn rejects_both_and_or() {
        assert!(ConditionSet::validate(&json!({ "and": [], "or": [] })).is_err());
    }

    #[test]
    fn rejects_empty_group() {
        assert!(ConditionSet::validate(&json!({ "and": [] })).is_err());
    }

    #[test]
    fn rejects_neither_and_or() {
        assert!(ConditionSet::validate(&json!({ "attr": "x", "op": "eq", "value": 1 })).is_err());
    }

    #[test]
    fn rejects_unknown_operator() {
        assert!(
            ConditionSet::validate(&json!({
                "and": [{ "attr": "x", "op": "startswith", "value": "y" }]
            }))
            .is_err()
        );
    }

    #[test]
    fn rejects_missing_value() {
        assert!(
            ConditionSet::validate(&json!({
                "and": [{ "attr": "x", "op": "eq" }]
            }))
            .is_err()
        );
    }

    #[test]
    fn rejects_in_with_non_array() {
        assert!(
            ConditionSet::validate(&json!({
                "and": [{ "attr": "x", "op": "in", "value": "notarray" }]
            }))
            .is_err()
        );
    }

    #[test]
    fn rejects_invalid_regex() {
        assert!(
            ConditionSet::validate(&json!({
                "and": [{ "attr": "x", "op": "regex", "value": "(" }]
            }))
            .is_err()
        );
    }
}
