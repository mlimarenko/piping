//! Integration test target.

// Normalization idempotence tests.
//
// These operators are NOT invertible (information is discarded by the forward
// pass), but they must be idempotent: applying them twice produces the same
// result as applying them once. This is the contract the round-trip test
// suite relies on when fields are excluded from byte-equal assertions.
//
// Idempotence is a separate concern from invertibility and
// has its own test surface to avoid conflating the two.

use serde_json::{Value, json};

use piping_backend::contexts::sync::domain::{TransformDirection, evaluate_expression};
use piping_backend::errors::AppError;

fn forward(expr: &Value, sample: &Value) -> Result<Value, AppError> {
    evaluate_expression(expr, sample, TransformDirection::Forward)
}

/// Apply `expr` twice in the forward direction, threading the result back
/// through the field referenced in `expr.args[0].field`. Asserts that the
/// second application equals the first.
fn assert_forward_idempotent(
    expr: &Value,
    sample: &Value,
    field_path: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let f1 = forward(expr, sample).map_err(|e| format!("first forward: {e}"))?;
    let updated_sample = embed(sample.clone(), field_path, f1.clone())?;
    let f2 = forward(expr, &updated_sample).map_err(|e| format!("second forward: {e}"))?;
    assert_eq!(
        f1, f2,
        "operator must be idempotent for sample = {sample}, but f1={f1} f2={f2}"
    );
    Ok(())
}

/// Build a new sample by overwriting the value at the dotted `path`.
fn embed(
    mut root: Value,
    path: &str,
    value: Value,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    let segments: Vec<&str> = path.split('.').collect();
    let mut cursor = &mut root;
    for (i, seg) in segments.iter().enumerate() {
        if i == segments.len() - 1 {
            if let Value::Object(map) = cursor {
                map.insert((*seg).to_string(), value);
                return Ok(root);
            }
            return Err(format!("embed: cursor is not an object at segment '{seg}'").into());
        }
        cursor = match cursor {
            Value::Object(map) => map.entry((*seg).to_string()).or_insert(Value::Null),
            _ => {
                return Err(format!("embed: cursor is not an object at segment '{seg}'").into());
            }
        };
    }
    Ok(root)
}

// === trim ==================================================================

#[test]
fn trim_is_idempotent() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let expr = json!({"op": "trim", "args": [{"field": "n"}]});
    let sample = json!({"n": "  spaced  "});
    assert_forward_idempotent(&expr, &sample, "n")?;
    Ok(())
}

#[test]
fn trim_is_idempotent_when_already_clean() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let expr = json!({"op": "trim", "args": [{"field": "n"}]});
    let sample = json!({"n": "clean"});
    assert_forward_idempotent(&expr, &sample, "n")?;
    Ok(())
}

// === lower / upper =========================================================

#[test]
fn lower_is_idempotent() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let expr = json!({"op": "lower", "args": [{"field": "n"}]});
    let sample = json!({"n": "MiXeD"});
    assert_forward_idempotent(&expr, &sample, "n")?;
    Ok(())
}

#[test]
fn upper_is_idempotent() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let expr = json!({"op": "upper", "args": [{"field": "n"}]});
    let sample = json!({"n": "MiXeD"});
    assert_forward_idempotent(&expr, &sample, "n")?;
    Ok(())
}

// === mac_normalize =========================================================

#[test]
fn mac_normalize_is_idempotent_canonical_input()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let expr = json!({"op": "mac_normalize", "args": [{"field": "m"}]});
    let sample = json!({"m": "aa:bb:cc:dd:ee:ff"});
    assert_forward_idempotent(&expr, &sample, "m")?;
    Ok(())
}

#[test]
fn mac_normalize_is_idempotent_dashed_input() -> Result<(), Box<dyn std::error::Error + Send + Sync>>
{
    let expr = json!({"op": "mac_normalize", "args": [{"field": "m"}]});
    let sample = json!({"m": "AA-BB-CC-DD-EE-FF"});
    assert_forward_idempotent(&expr, &sample, "m")?;
    Ok(())
}

#[test]
fn mac_normalize_is_idempotent_hex_run_input()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let expr = json!({"op": "mac_normalize", "args": [{"field": "m"}]});
    let sample = json!({"m": "aabbccddeeff"});
    assert_forward_idempotent(&expr, &sample, "m")?;
    Ok(())
}

// === ipv4_prefix_canonical =================================================

#[test]
fn ipv4_prefix_canonical_is_idempotent_already_canonical()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let expr = json!({"op": "ipv4_prefix_canonical", "args": [{"field": "p"}]});
    let sample = json!({"p": "10.0.0.0/24"});
    assert_forward_idempotent(&expr, &sample, "p")?;
    Ok(())
}

#[test]
fn ipv4_prefix_canonical_is_idempotent_host_bits_set()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let expr = json!({"op": "ipv4_prefix_canonical", "args": [{"field": "p"}]});
    let sample = json!({"p": "10.0.0.5/24"});
    // First application zeros host bits; second application returns the same
    // canonical form.
    assert_forward_idempotent(&expr, &sample, "p")?;
    Ok(())
}

#[test]
fn ipv4_prefix_canonical_is_idempotent_full_mask()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let expr = json!({"op": "ipv4_prefix_canonical", "args": [{"field": "p"}]});
    let sample = json!({"p": "192.168.42.7/32"});
    assert_forward_idempotent(&expr, &sample, "p")?;
    Ok(())
}
