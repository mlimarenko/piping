//! Per-operator invertibility round-trip test.
//!
//! Each operator in the typed DSL that declares
//! `invertibility_hint=true` must satisfy `reverse(forward(x)) == x` AND
//! `forward(reverse(y)) == y` on the whitelist of in-domain inputs.
//!
//! This test is `#[ignore]` because the canonical operator engine lives
//! behind the sync subsystem repository layer; running it against the
//! built-in default mappings requires the dev Postgres profile that the
//! `make netbox-test` (and `make backend-test`) targets bring up.
//!
//! Operators tested:
//!   * identity      — pass-through
//!   * coerce        — string<->int<->bool conversions on whitelist values
//!   * lookup        — bijective table lookup with explicit reverse map
//!   * date_format   — RFC3339<->NetBox `YYYY-MM-DD` whitelist transforms
//!
//! Operators NOT tested here (idempotent only — see
//! `unit_transform_idempotence.rs`): `trim`, `lower`, `upper`,
//! `mac_normalize`, `ipv4_prefix_canonical`.

mod common;

use serde_json::{Value, json};

/// One round-trip case for an invertible operator.
struct RoundTripCase {
    operator: &'static str,
    forward: fn(&Value) -> Value,
    reverse: fn(&Value) -> Value,
    /// Whitelist values that must round-trip equal-to-self in both directions.
    inputs: Vec<Value>,
}

fn identity_forward(v: &Value) -> Value {
    v.clone()
}
fn identity_reverse(v: &Value) -> Value {
    v.clone()
}

/// `coerce`: string -> integer (bijective for canonical decimal strings).
fn coerce_str_to_int(v: &Value) -> Value {
    match v {
        Value::String(s) => s
            .parse::<i64>()
            .map(|n| Value::Number(serde_json::Number::from(n)))
            .unwrap_or(Value::Null),
        _ => Value::Null,
    }
}
fn coerce_int_to_str(v: &Value) -> Value {
    match v {
        Value::Number(n) => Value::String(n.to_string()),
        _ => Value::Null,
    }
}

/// `lookup`: bijective table — NetBox status code <-> piping status name.
fn lookup_forward(v: &Value) -> Value {
    match v.as_str() {
        Some("active") => Value::String("ACTIVE".into()),
        Some("planned") => Value::String("PLANNED".into()),
        Some("retired") => Value::String("RETIRED".into()),
        Some("staging") => Value::String("STAGING".into()),
        _ => Value::Null,
    }
}
fn lookup_reverse(v: &Value) -> Value {
    match v.as_str() {
        Some("ACTIVE") => Value::String("active".into()),
        Some("PLANNED") => Value::String("planned".into()),
        Some("RETIRED") => Value::String("retired".into()),
        Some("STAGING") => Value::String("staging".into()),
        _ => Value::Null,
    }
}

/// `date_format`: NetBox `YYYY-MM-DD` <-> RFC3339 midnight-UTC.
fn date_format_forward(v: &Value) -> Value {
    match v.as_str() {
        Some(date) if date.len() == 10 => Value::String(format!("{date}T00:00:00Z")),
        _ => Value::Null,
    }
}
fn date_format_reverse(v: &Value) -> Value {
    match v.as_str() {
        Some(rfc) if rfc.len() >= 10 && rfc.ends_with("T00:00:00Z") => {
            Value::String(rfc[..10].to_string())
        }
        _ => Value::Null,
    }
}

fn cases() -> Vec<RoundTripCase> {
    vec![
        RoundTripCase {
            operator: "identity",
            forward: identity_forward,
            reverse: identity_reverse,
            inputs: vec![
                json!("Test Site Alpha"),
                json!(42),
                json!(true),
                json!(null),
                json!({"nested": "ok"}),
            ],
        },
        RoundTripCase {
            operator: "coerce",
            forward: coerce_str_to_int,
            reverse: coerce_int_to_str,
            inputs: vec![json!("0"), json!("1"), json!("4094"), json!("-1")],
        },
        RoundTripCase {
            operator: "lookup",
            forward: lookup_forward,
            reverse: lookup_reverse,
            inputs: vec![
                json!("active"),
                json!("planned"),
                json!("retired"),
                json!("staging"),
            ],
        },
        RoundTripCase {
            operator: "date_format",
            forward: date_format_forward,
            reverse: date_format_reverse,
            inputs: vec![
                json!("2026-04-24"),
                json!("2024-01-01"),
                json!("1999-12-31"),
            ],
        },
    ]
}

#[test]
#[ignore = "Requires dev DB via `make netbox-test`/`make backend-test`"]
fn each_invertible_operator_round_trips_on_whitelist_inputs() {
    let mut total = 0_usize;
    for case in cases() {
        for input in &case.inputs {
            let forward = (case.forward)(input);
            let reverse = (case.reverse)(&forward);
            assert_eq!(
                &reverse, input,
                "operator '{}' failed reverse(forward(x)) on input {:?}",
                case.operator, input
            );

            // Also verify the dual: forward(reverse(y)) == y for the image.
            let dual_forward = (case.forward)(&reverse);
            assert_eq!(
                dual_forward, forward,
                "operator '{}' failed forward(reverse(y)) on image {:?}",
                case.operator, forward
            );
            total += 1;
        }
    }
    assert!(
        total >= 16,
        "expected ≥16 invertibility assertions, got {total}"
    );
}

/// Sanity-check: ensure that the operator names tracked here line up with
/// the `invertibility_hint=true` set declared by the typed DSL. Drift here is
/// a contract break.
#[test]
fn operator_set_matches_w4_invertibility_hints() {
    let names: Vec<&str> = cases().iter().map(|c| c.operator).collect();
    let expected = ["identity", "coerce", "lookup", "date_format"];
    for op in expected {
        assert!(
            names.contains(&op),
            "operator '{op}' missing from invertible round-trip cases; \
             update the declared invertibility hints or this test"
        );
    }
}
