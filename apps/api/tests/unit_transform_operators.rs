//! Integration test target.

// Per-operator unit tests for the typed-DSL evaluator.
//
// Forward path is exercised on representative inputs for every operator.
// Operators marked `invertibility_hint=true` must round-trip stably:
//   forward(reverse(forward(x))) == forward(x)
//   reverse(forward(reverse(y))) == reverse(y)
// Operators marked `invertibility_hint=false` must surface
// `OperatorError::NonInvertible` (mapped to `AppError::Validation`) when
// reverse evaluation is requested — never a silent fallthrough.

use serde_json::{Value, json};

use piping_backend::contexts::sync::domain::{Operator, TransformDirection, evaluate_expression};
use piping_backend::errors::AppError;

fn forward(expr: &Value, sample: &Value) -> Result<Value, AppError> {
    evaluate_expression(expr, sample, TransformDirection::Forward)
}

fn reverse(expr: &Value, sample: &Value) -> Result<Value, AppError> {
    evaluate_expression(expr, sample, TransformDirection::Reverse)
}

fn assert_non_invertible(
    expr: &Value,
    sample: &Value,
    op_name: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let err = reverse(expr, sample)
        .err()
        .ok_or_else(|| format!("{op_name} reverse should fail"))?;
    let message = err.to_string();
    assert!(
        message.contains("not invertible") || message.contains("NonInvertible"),
        "expected NonInvertible error for {op_name}, got: {message}"
    );
    Ok(())
}

// === identity ==============================================================

#[test]
fn identity_forward_resolves_field_path() {
    let expr = json!({"op": "identity", "args": [{"field": "name"}]});
    let sample = json!({"name": "rack-01"});
    assert_eq!(forward(&expr, &sample).unwrap(), json!("rack-01"));
}

#[test]
fn identity_round_trip_stable() {
    let expr = json!({"op": "identity", "args": [{"field": "name"}]});
    let sample = json!({"name": "rack-01"});
    let f = forward(&expr, &sample).unwrap();
    let r = reverse(&expr, &sample).unwrap();
    assert_eq!(f, r);
}

// === literal ===============================================================

#[test]
fn literal_forward_returns_constant() {
    let expr = json!({"op": "literal", "args": ["active"]});
    let sample = json!({});
    assert_eq!(forward(&expr, &sample).unwrap(), json!("active"));
}

#[test]
fn literal_reverse_is_non_invertible() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let expr = json!({"op": "literal", "args": ["active"]});
    let sample = json!({});
    assert_non_invertible(&expr, &sample, "literal")
}

// === concat ================================================================

#[test]
fn concat_forward_joins_with_default_empty_separator() {
    let expr = json!({
        "op": "concat",
        "args": [{"field": "a"}, {"field": "b"}],
    });
    let sample = json!({"a": "foo", "b": "bar"});
    assert_eq!(forward(&expr, &sample).unwrap(), json!("foobar"));
}

#[test]
fn concat_forward_joins_with_separator_literal() {
    let expr = json!({
        "op": "concat",
        "args": [{"field": "a"}, {"field": "b"}, {"sep": "-"}],
    });
    let sample = json!({"a": "left", "b": "right"});
    assert_eq!(forward(&expr, &sample).unwrap(), json!("left-right"));
}

#[test]
fn concat_reverse_is_non_invertible() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let expr = json!({
        "op": "concat",
        "args": [{"field": "a"}, {"field": "b"}],
    });
    let sample = json!({"a": "x", "b": "y"});
    assert_non_invertible(&expr, &sample, "concat")
}

// === split_index ===========================================================

#[test]
fn split_index_forward_returns_segment() {
    let expr = json!({
        "op": "split_index",
        "args": [{"field": "fqdn"}, ".", 0],
    });
    let sample = json!({"fqdn": "host.example.com"});
    assert_eq!(forward(&expr, &sample).unwrap(), json!("host"));
}

#[test]
fn split_index_forward_out_of_range_returns_empty() {
    let expr = json!({
        "op": "split_index",
        "args": [{"field": "fqdn"}, ".", 99],
    });
    let sample = json!({"fqdn": "host.example.com"});
    assert_eq!(forward(&expr, &sample).unwrap(), json!(""));
}

#[test]
fn split_index_reverse_is_non_invertible() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let expr = json!({
        "op": "split_index",
        "args": [{"field": "fqdn"}, ".", 0],
    });
    let sample = json!({"fqdn": "host.example.com"});
    assert_non_invertible(&expr, &sample, "split_index")
}

// === trim / lower / upper ==================================================

#[test]
fn trim_forward_strips_whitespace() {
    let expr = json!({"op": "trim", "args": [{"field": "n"}]});
    let sample = json!({"n": "  spaced  "});
    assert_eq!(forward(&expr, &sample).unwrap(), json!("spaced"));
}

#[test]
fn trim_reverse_is_non_invertible() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let expr = json!({"op": "trim", "args": [{"field": "n"}]});
    let sample = json!({"n": "  spaced  "});
    assert_non_invertible(&expr, &sample, "trim")
}

#[test]
fn lower_forward_lowercases() {
    let expr = json!({"op": "lower", "args": [{"field": "n"}]});
    let sample = json!({"n": "Mixed"});
    assert_eq!(forward(&expr, &sample).unwrap(), json!("mixed"));
}

#[test]
fn lower_reverse_is_non_invertible() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let expr = json!({"op": "lower", "args": [{"field": "n"}]});
    let sample = json!({"n": "Mixed"});
    assert_non_invertible(&expr, &sample, "lower")
}

#[test]
fn upper_forward_uppercases() {
    let expr = json!({"op": "upper", "args": [{"field": "n"}]});
    let sample = json!({"n": "Mixed"});
    assert_eq!(forward(&expr, &sample).unwrap(), json!("MIXED"));
}

#[test]
fn upper_reverse_is_non_invertible() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let expr = json!({"op": "upper", "args": [{"field": "n"}]});
    let sample = json!({"n": "Mixed"});
    assert_non_invertible(&expr, &sample, "upper")
}

// === coerce ================================================================

#[test]
fn coerce_forward_to_string() {
    let expr = json!({
        "op": "coerce",
        "args": [{"field": "n"}, "string"],
    });
    let sample = json!({"n": 42});
    assert_eq!(forward(&expr, &sample).unwrap(), json!("42"));
}

#[test]
fn coerce_forward_to_integer() {
    let expr = json!({
        "op": "coerce",
        "args": [{"field": "n"}, "integer"],
    });
    let sample = json!({"n": "42"});
    assert_eq!(forward(&expr, &sample).unwrap(), json!(42));
}

#[test]
fn coerce_forward_to_boolean_from_string() {
    let expr = json!({
        "op": "coerce",
        "args": [{"field": "flag"}, "boolean"],
    });
    let sample = json!({"flag": "true"});
    assert_eq!(forward(&expr, &sample).unwrap(), json!(true));
}

#[test]
fn coerce_round_trip_string_to_string_stable() {
    // For a string input coerced to string, both forward and reverse paths
    // are stable: forward(reverse(forward(x))) == forward(x).
    let expr = json!({
        "op": "coerce",
        "args": [{"field": "n"}, "string"],
    });
    let sample = json!({"n": "hello"});
    let f1 = forward(&expr, &sample).unwrap();
    // Reverse expects the input to also be in the sample at the same path; we
    // simulate by building a new sample where `n` carries the forward result.
    let reversed_sample = json!({"n": f1.as_str().unwrap()});
    let r = reverse(&expr, &reversed_sample).unwrap();
    let f2 = forward(&expr, &reversed_sample).unwrap();
    // forward(reverse(forward(x))) == forward(x)
    assert_eq!(f2, f1);
    // reverse must be a string representation of the input
    assert_eq!(r, json!("hello"));
}

// === lookup ================================================================

fn bijective_status_table() -> Value {
    json!({
        "active": "ACTIVE",
        "planned": "PLANNED",
        "retired": "RETIRED",
    })
}

#[test]
fn lookup_forward_returns_mapped_value() {
    let expr = json!({
        "op": "lookup",
        "args": [bijective_status_table(), {"field": "status"}],
    });
    let sample = json!({"status": "active"});
    assert_eq!(forward(&expr, &sample).unwrap(), json!("ACTIVE"));
}

#[test]
fn lookup_forward_missing_key_errors() {
    let expr = json!({
        "op": "lookup",
        "args": [bijective_status_table(), {"field": "status"}],
    });
    let sample = json!({"status": "unknown"});
    let err = forward(&expr, &sample).expect_err("lookup miss");
    assert!(err.to_string().contains("lookup miss"), "got: {err}");
}

#[test]
fn lookup_reverse_returns_inverse_key_for_bijective_table() {
    let expr = json!({
        "op": "lookup",
        "args": [bijective_status_table(), {"field": "status_external"}],
    });
    // Reverse: given the external value, recover the source key.
    let sample = json!({"status_external": "ACTIVE"});
    assert_eq!(reverse(&expr, &sample).unwrap(), json!("active"));
}

#[test]
fn lookup_round_trip_stable_for_bijective_table() {
    let table = bijective_status_table();
    let expr = json!({
        "op": "lookup",
        "args": [table, {"field": "status"}],
    });
    // Start with source value: "active".
    let sample = json!({"status": "active"});
    let forward_value = forward(&expr, &sample).unwrap(); // "ACTIVE"

    // Reverse-eval should give back "active" (stored under same field name).
    let reverse_expr = json!({
        "op": "lookup",
        "args": [bijective_status_table(), {"field": "status"}],
    });
    let reverse_sample = json!({"status": forward_value.as_str().unwrap()});
    let r = reverse(&reverse_expr, &reverse_sample).unwrap(); // "active"
    assert_eq!(r, json!("active"));

    // forward(reverse(forward(x))) == forward(x)
    let again = forward(&expr, &json!({"status": r.as_str().unwrap()})).unwrap();
    assert_eq!(again, forward_value);
}

#[test]
fn lookup_non_bijective_table_reverse_errors() {
    let table = json!({
        "active": "ON",
        "running": "ON",
        "retired": "OFF",
    });
    let expr = json!({
        "op": "lookup",
        "args": [table, {"field": "v"}],
    });
    let sample = json!({"v": "ON"});
    let err = reverse(&expr, &sample).expect_err("non-bijective");
    assert!(err.to_string().contains("not bijective"), "got: {err}");
}

// === template ==============================================================

#[test]
fn template_forward_substitutes_named_vars() {
    let expr = json!({
        "op": "template",
        "args": [
            "host-{name}-{site}",
            {"name": "name", "value": {"field": "n"}},
            {"name": "site", "value": {"field": "s"}},
        ],
    });
    let sample = json!({"n": "leaf01", "s": "ams"});
    assert_eq!(forward(&expr, &sample).unwrap(), json!("host-leaf01-ams"));
}

#[test]
fn template_reverse_is_non_invertible() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let expr = json!({
        "op": "template",
        "args": ["{x}", {"name": "x", "value": {"field": "x"}}],
    });
    let sample = json!({"x": "v"});
    assert_non_invertible(&expr, &sample, "template")
}

// === dereference ===========================================================

#[test]
fn dereference_forward_traverses_fk_chain() {
    let expr = json!({
        "op": "dereference",
        "args": [{"field": "device.site"}, "name"],
    });
    let sample = json!({
        "device": {"site": {"name": "ams01"}},
    });
    assert_eq!(forward(&expr, &sample).unwrap(), json!("ams01"));
}

#[test]
fn dereference_reverse_is_non_invertible() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let expr = json!({
        "op": "dereference",
        "args": [{"field": "device.site"}, "name"],
    });
    let sample = json!({"device": {"site": {"name": "ams01"}}});
    assert_non_invertible(&expr, &sample, "dereference")
}

// === date_format ===========================================================

#[test]
fn date_format_forward_reformats_date() {
    let expr = json!({
        "op": "date_format",
        "args": [{"field": "d"}, "%Y-%m-%d", "%d/%m/%Y"],
    });
    let sample = json!({"d": "2026-04-24"});
    assert_eq!(forward(&expr, &sample).unwrap(), json!("24/04/2026"));
}

#[test]
fn date_format_round_trip_stable() {
    let expr = json!({
        "op": "date_format",
        "args": [{"field": "d"}, "%Y-%m-%d", "%d/%m/%Y"],
    });
    let sample = json!({"d": "2026-04-24"});
    let forward_value = forward(&expr, &sample).unwrap(); // "24/04/2026"

    // Reverse: parse with fmt_out, render with fmt_in.
    let reverse_sample = json!({"d": forward_value.as_str().unwrap()});
    let reversed = reverse(&expr, &reverse_sample).unwrap();
    assert_eq!(reversed, json!("2026-04-24"));

    // forward(reverse(forward(x))) == forward(x)
    let f2 = forward(&expr, &json!({"d": reversed.as_str().unwrap()})).unwrap();
    assert_eq!(f2, forward_value);
}

// === tags_merge ============================================================

#[test]
fn tags_merge_forward_unions_arrays() {
    let expr = json!({
        "op": "tags_merge",
        "args": [{"field": "a"}, {"field": "b"}],
    });
    let sample = json!({"a": ["x", "y"], "b": ["y", "z"]});
    let result = forward(&expr, &sample).unwrap();
    let mut got: Vec<String> = result
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    got.sort();
    assert_eq!(got, vec!["x".to_string(), "y".to_string(), "z".to_string()]);
}

#[test]
fn tags_merge_reverse_is_non_invertible() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let expr = json!({
        "op": "tags_merge",
        "args": [{"field": "a"}, {"field": "b"}],
    });
    let sample = json!({"a": ["x"], "b": ["y"]});
    assert_non_invertible(&expr, &sample, "tags_merge")
}

// === mac_normalize =========================================================

#[test]
fn mac_normalize_forward_canonicalizes() {
    let expr = json!({"op": "mac_normalize", "args": [{"field": "m"}]});
    let sample = json!({"m": "AA-BB-CC-DD-EE-FF"});
    assert_eq!(forward(&expr, &sample).unwrap(), json!("aa:bb:cc:dd:ee:ff"));
}

#[test]
fn mac_normalize_handles_continuous_hex() {
    let expr = json!({"op": "mac_normalize", "args": [{"field": "m"}]});
    let sample = json!({"m": "aabbccddeeff"});
    assert_eq!(forward(&expr, &sample).unwrap(), json!("aa:bb:cc:dd:ee:ff"));
}

#[test]
fn mac_normalize_reverse_is_non_invertible() -> Result<(), Box<dyn std::error::Error + Send + Sync>>
{
    let expr = json!({"op": "mac_normalize", "args": [{"field": "m"}]});
    let sample = json!({"m": "aa:bb:cc:dd:ee:ff"});
    assert_non_invertible(&expr, &sample, "mac_normalize")
}

// === ipv4_prefix_canonical =================================================

#[test]
fn ipv4_prefix_canonical_forward_zeros_host_bits() {
    let expr = json!({"op": "ipv4_prefix_canonical", "args": [{"field": "p"}]});
    let sample = json!({"p": "10.0.0.5/24"});
    assert_eq!(forward(&expr, &sample).unwrap(), json!("10.0.0.0/24"));
}

#[test]
fn ipv4_prefix_canonical_forward_zero_prefix() {
    let expr = json!({"op": "ipv4_prefix_canonical", "args": [{"field": "p"}]});
    let sample = json!({"p": "192.168.1.42/0"});
    assert_eq!(forward(&expr, &sample).unwrap(), json!("0.0.0.0/0"));
}

#[test]
fn ipv4_prefix_canonical_reverse_is_non_invertible()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let expr = json!({"op": "ipv4_prefix_canonical", "args": [{"field": "p"}]});
    let sample = json!({"p": "10.0.0.5/24"});
    assert_non_invertible(&expr, &sample, "ipv4_prefix_canonical")
}

// === invertibility hints ===================================================
//
// Lock the operator registry's hints down so a future edit cannot flip
// invertibility silently. This pins the contract the validator relies on.
#[test]
fn invertibility_hints_match_spec() {
    let invertible = [
        Operator::Identity,
        Operator::Coerce,
        Operator::Lookup,
        Operator::DateFormat,
    ];
    let non_invertible = [
        Operator::Literal,
        Operator::Concat,
        Operator::SplitIndex,
        Operator::Trim,
        Operator::Lower,
        Operator::Upper,
        Operator::Template,
        Operator::Dereference,
        Operator::TagsMerge,
        Operator::MacNormalize,
        Operator::Ipv4PrefixCanonical,
    ];
    for op in invertible {
        assert!(
            op.invertibility_hint(),
            "{} should be invertible",
            op.signature().name
        );
    }
    for op in non_invertible {
        assert!(
            !op.invertibility_hint(),
            "{} should NOT be invertible",
            op.signature().name
        );
    }
}
