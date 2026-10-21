// Typed DSL operator registry.
//
// MVP set: `identity, literal, concat, split_index, trim, lower, upper,
// coerce, lookup, template, dereference`.
// Expansion set: `date_format, tags_merge, mac_normalize,
// ipv4_prefix_canonical`.
//
// Only `invertibility_hint=true` operators
// are allowed to author a `reverse_transform_json`; normalization operators
// are tested for idempotence only (see `unit_transform_idempotence`).

use std::collections::BTreeSet;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::errors::AppError;

/// Output type produced by a DSL operator.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutputType {
    String,
    Integer,
    Number,
    Boolean,
    Array,
    Object,
    Any,
}

impl OutputType {
    pub fn as_str(&self) -> &'static str {
        match self {
            OutputType::String => "string",
            OutputType::Integer => "integer",
            OutputType::Number => "number",
            OutputType::Boolean => "boolean",
            OutputType::Array => "array",
            OutputType::Object => "object",
            OutputType::Any => "any",
        }
    }
}

/// Argument shape for an operator parameter.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ArgKind {
    /// Field reference (e.g. `{"field": "name"}`).
    Field,
    /// Literal scalar.
    Literal,
    /// Variadic list of expressions of any kind.
    Variadic,
    /// Nested operator expression.
    Expr,
}

#[derive(Debug, Clone)]
pub struct OperatorSig {
    pub name: &'static str,
    pub args: &'static [ArgKind],
    pub output_type: OutputType,
    /// True when (a) `reverse(forward(x))==x` AND (b) `forward(reverse(y))==y`
    /// can be authored as a paired DSL expression.
    pub invertibility_hint: bool,
    /// Plain-English signature for UI/error messages.
    pub signature: &'static str,
}

/// Typed DSL operator enum (MVP + expansion set).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Operator {
    Identity,
    Literal,
    Concat,
    SplitIndex,
    Trim,
    Lower,
    Upper,
    Coerce,
    Lookup,
    Template,
    Dereference,
    DateFormat,
    TagsMerge,
    MacNormalize,
    Ipv4PrefixCanonical,
}

impl Operator {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "identity" => Some(Operator::Identity),
            "literal" => Some(Operator::Literal),
            "concat" => Some(Operator::Concat),
            "split_index" => Some(Operator::SplitIndex),
            "trim" => Some(Operator::Trim),
            "lower" => Some(Operator::Lower),
            "upper" => Some(Operator::Upper),
            "coerce" => Some(Operator::Coerce),
            "lookup" => Some(Operator::Lookup),
            "template" => Some(Operator::Template),
            "dereference" => Some(Operator::Dereference),
            "date_format" => Some(Operator::DateFormat),
            "tags_merge" => Some(Operator::TagsMerge),
            "mac_normalize" => Some(Operator::MacNormalize),
            "ipv4_prefix_canonical" => Some(Operator::Ipv4PrefixCanonical),
            _ => None,
        }
    }

    pub fn signature(&self) -> OperatorSig {
        match self {
            Operator::Identity => OperatorSig {
                name: "identity",
                args: &[ArgKind::Field],
                output_type: OutputType::Any,
                invertibility_hint: true,
                signature: "identity(field)",
            },
            Operator::Literal => OperatorSig {
                name: "literal",
                args: &[ArgKind::Literal],
                output_type: OutputType::Any,
                // `literal` is forward-only: reverse cannot recover the source
                // because the forward path discards its input entirely.
                invertibility_hint: false,
                signature: "literal(value)",
            },
            Operator::Concat => OperatorSig {
                name: "concat",
                args: &[ArgKind::Variadic],
                output_type: OutputType::String,
                invertibility_hint: false,
                signature: "concat(a, b, ..., sep)",
            },
            Operator::SplitIndex => OperatorSig {
                name: "split_index",
                args: &[ArgKind::Expr, ArgKind::Literal, ArgKind::Literal],
                output_type: OutputType::String,
                // Lossy — the unselected segments cannot be reconstructed.
                invertibility_hint: false,
                signature: "split_index(s, sep, i)",
            },
            Operator::Trim => OperatorSig {
                name: "trim",
                args: &[ArgKind::Expr],
                output_type: OutputType::String,
                invertibility_hint: false,
                signature: "trim(s)",
            },
            Operator::Lower => OperatorSig {
                name: "lower",
                args: &[ArgKind::Expr],
                output_type: OutputType::String,
                invertibility_hint: false,
                signature: "lower(s)",
            },
            Operator::Upper => OperatorSig {
                name: "upper",
                args: &[ArgKind::Expr],
                output_type: OutputType::String,
                invertibility_hint: false,
                signature: "upper(s)",
            },
            Operator::Coerce => OperatorSig {
                name: "coerce",
                args: &[ArgKind::Expr, ArgKind::Literal],
                output_type: OutputType::Any,
                invertibility_hint: true,
                signature: "coerce(s, type)",
            },
            Operator::Lookup => OperatorSig {
                name: "lookup",
                args: &[ArgKind::Literal, ArgKind::Expr],
                output_type: OutputType::Any,
                invertibility_hint: true,
                signature: "lookup(table, key)",
            },
            Operator::Template => OperatorSig {
                name: "template",
                args: &[ArgKind::Literal, ArgKind::Variadic],
                output_type: OutputType::String,
                invertibility_hint: false,
                signature: "template(pattern, vars...)",
            },
            Operator::Dereference => OperatorSig {
                name: "dereference",
                args: &[ArgKind::Field, ArgKind::Literal],
                output_type: OutputType::Any,
                invertibility_hint: false,
                signature: "dereference(fk_field, target_attr)",
            },
            Operator::DateFormat => OperatorSig {
                name: "date_format",
                args: &[ArgKind::Expr, ArgKind::Literal, ArgKind::Literal],
                output_type: OutputType::String,
                invertibility_hint: true,
                signature: "date_format(s, fmt_in, fmt_out)",
            },
            Operator::TagsMerge => OperatorSig {
                name: "tags_merge",
                args: &[ArgKind::Expr, ArgKind::Expr],
                output_type: OutputType::Array,
                invertibility_hint: false,
                signature: "tags_merge(a, b)",
            },
            Operator::MacNormalize => OperatorSig {
                name: "mac_normalize",
                args: &[ArgKind::Expr],
                output_type: OutputType::String,
                invertibility_hint: false,
                signature: "mac_normalize(s)",
            },
            Operator::Ipv4PrefixCanonical => OperatorSig {
                name: "ipv4_prefix_canonical",
                args: &[ArgKind::Expr],
                output_type: OutputType::String,
                invertibility_hint: false,
                signature: "ipv4_prefix_canonical(s)",
            },
        }
    }

    pub fn output_type(&self) -> OutputType {
        self.signature().output_type
    }

    pub fn invertibility_hint(&self) -> bool {
        self.signature().invertibility_hint
    }
}

/// Direction of evaluation for a transform expression.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransformDirection {
    Forward,
    Reverse,
}

/// Domain-level error type for DSL operator evaluation.
///
/// Distinct from `AppError` so callers (tests, validator) can pattern-match on
/// the precise failure mode. Converted into `AppError` at the API boundary via
/// `From<OperatorError>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperatorError {
    /// Reverse evaluation requested for an operator marked
    /// `invertibility_hint=false`.
    NonInvertible { operator: &'static str },
    /// Argument or field had a type that the operator cannot consume.
    TypeMismatch {
        operator: &'static str,
        expected: &'static str,
        got: String,
    },
    /// Parsing (date, MAC, CIDR, …) of a string-shaped value failed.
    ParseError {
        operator: &'static str,
        detail: String,
    },
    /// `lookup(table, key)` could not find a matching entry.
    LookupMissing { operator: &'static str, key: String },
    /// Argument shape was invalid for the operator (wrong arity / wrong kind).
    InvalidArgs {
        operator: &'static str,
        detail: String,
    },
}

impl std::fmt::Display for OperatorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OperatorError::NonInvertible { operator } => {
                write!(
                    f,
                    "operator '{operator}' is not invertible; reverse_transform_json is not allowed",
                )
            }
            OperatorError::TypeMismatch {
                operator,
                expected,
                got,
            } => write!(f, "operator '{operator}' expected {expected}, got {got}",),
            OperatorError::ParseError { operator, detail } => {
                write!(f, "operator '{operator}' parse error: {detail}")
            }
            OperatorError::LookupMissing { operator, key } => {
                write!(f, "operator '{operator}' lookup miss for key '{key}'")
            }
            OperatorError::InvalidArgs { operator, detail } => {
                write!(f, "operator '{operator}' invalid args: {detail}")
            }
        }
    }
}

impl std::error::Error for OperatorError {}

impl From<OperatorError> for AppError {
    fn from(error: OperatorError) -> Self {
        match error {
            OperatorError::NonInvertible { .. } | OperatorError::LookupMissing { .. } => {
                AppError::Validation(error.to_string())
            }
            OperatorError::TypeMismatch { .. }
            | OperatorError::ParseError { .. }
            | OperatorError::InvalidArgs { .. } => AppError::Validation(error.to_string()),
        }
    }
}

/// Evaluate a typed-DSL expression on a sample record in the given direction.
///
/// `direction == Forward` runs the operator's forward path. `direction ==
/// Reverse` runs the reverse path; non-invertible operators return
/// `OperatorError::NonInvertible` (mapped to `AppError::Validation`) instead of
/// silently passing.
pub fn evaluate_expression(
    expr: &Value,
    sample: &Value,
    direction: TransformDirection,
) -> Result<Value, AppError> {
    let (op, args) = parse_op_call(expr)?;
    match direction {
        TransformDirection::Forward => forward(op, &args, sample).map_err(AppError::from),
        TransformDirection::Reverse => reverse(op, &args, sample).map_err(AppError::from),
    }
}

fn parse_op_call(expr: &Value) -> Result<(Operator, Vec<Value>), AppError> {
    let Value::Object(map) = expr else {
        return Err(AppError::Validation(
            "transform expression must be a JSON object".to_string(),
        ));
    };
    let op_name = map
        .get("op")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::Validation("transform expression missing 'op'".to_string()))?;
    let op = Operator::from_name(op_name)
        .ok_or_else(|| AppError::Validation(format!("unknown operator '{op_name}'")))?;
    let args = map
        .get("args")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok((op, args))
}

// === Forward evaluation =====================================================

fn forward(op: Operator, args: &[Value], sample: &Value) -> Result<Value, OperatorError> {
    match op {
        Operator::Identity => op_identity_forward(args, sample),
        Operator::Literal => op_literal_forward(args),
        Operator::Concat => op_concat_forward(args, sample),
        Operator::SplitIndex => op_split_index_forward(args, sample),
        Operator::Trim => op_trim_forward(args, sample),
        Operator::Lower => op_lower_forward(args, sample),
        Operator::Upper => op_upper_forward(args, sample),
        Operator::Coerce => op_coerce_forward(args, sample),
        Operator::Lookup => op_lookup_forward(args, sample),
        Operator::Template => op_template_forward(args, sample),
        Operator::Dereference => op_dereference_forward(args, sample),
        Operator::DateFormat => op_date_format_forward(args, sample),
        Operator::TagsMerge => op_tags_merge_forward(args, sample),
        Operator::MacNormalize => op_mac_normalize_forward(args, sample),
        Operator::Ipv4PrefixCanonical => op_ipv4_prefix_canonical_forward(args, sample),
    }
}

// === Reverse evaluation =====================================================

fn reverse(op: Operator, args: &[Value], sample: &Value) -> Result<Value, OperatorError> {
    if !op.invertibility_hint() {
        return Err(OperatorError::NonInvertible {
            operator: op.signature().name,
        });
    }
    match op {
        // Identity round-trips by definition.
        Operator::Identity => op_identity_forward(args, sample),
        // `coerce(s, T)` reverses by coercing back to the source's declared
        // type. We treat reverse symmetrically: coerce the input into a string
        // representation. The validator + UI guarantees the target type is
        // lossless for a reversed configuration.
        Operator::Coerce => op_coerce_reverse(args, sample),
        // `lookup` reverses by looking up the value in the inverted table.
        // Bijectivity is enforced at config-save time (validator); here we
        // simply try the reverse map and fall back to LookupMissing.
        Operator::Lookup => op_lookup_reverse(args, sample),
        // `date_format` reverses by parsing with `fmt_out` and re-rendering
        // with `fmt_in`.
        Operator::DateFormat => op_date_format_reverse(args, sample),
        // The rest declared `invertibility_hint=false` and are caught above.
        _ => Err(OperatorError::NonInvertible {
            operator: op.signature().name,
        }),
    }
}

// === Operators ==============================================================

fn op_identity_forward(args: &[Value], sample: &Value) -> Result<Value, OperatorError> {
    let arg = args.first().ok_or_else(|| OperatorError::InvalidArgs {
        operator: "identity",
        detail: "expected 1 argument".to_string(),
    })?;
    let path =
        arg.get("field")
            .and_then(Value::as_str)
            .ok_or_else(|| OperatorError::InvalidArgs {
                operator: "identity",
                detail: "argument must be {\"field\": \"<path>\"}".to_string(),
            })?;
    Ok(resolve_field_path(sample, path))
}

fn op_literal_forward(args: &[Value]) -> Result<Value, OperatorError> {
    let value = args.first().ok_or_else(|| OperatorError::InvalidArgs {
        operator: "literal",
        detail: "expected 1 argument".to_string(),
    })?;
    Ok(value.clone())
}

fn op_concat_forward(args: &[Value], sample: &Value) -> Result<Value, OperatorError> {
    // Convention: variadic list of expressions; the last argument is a
    // separator literal ({"sep": "..."}). If no `sep` is provided, parts are
    // joined with the empty string.
    if args.is_empty() {
        return Err(OperatorError::InvalidArgs {
            operator: "concat",
            detail: "expected at least 1 argument".to_string(),
        });
    }
    let (sep, parts_args) = extract_sep(args);
    let mut parts: Vec<String> = Vec::with_capacity(parts_args.len());
    for arg in parts_args {
        let value = resolve_arg(arg, sample)?;
        parts.push(value_to_string(&value));
    }
    Ok(Value::String(parts.join(&sep)))
}

fn extract_sep(args: &[Value]) -> (String, &[Value]) {
    if let Some(last) = args.last()
        && let Some(sep) = last
            .as_object()
            .and_then(|m| m.get("sep"))
            .and_then(Value::as_str)
    {
        return (sep.to_string(), &args[..args.len() - 1]);
    }
    (String::new(), args)
}

fn op_split_index_forward(args: &[Value], sample: &Value) -> Result<Value, OperatorError> {
    if args.len() != 3 {
        return Err(OperatorError::InvalidArgs {
            operator: "split_index",
            detail: format!("expected 3 arguments, got {}", args.len()),
        });
    }
    let s = require_string(&resolve_arg(&args[0], sample)?, "split_index")?;
    let sep = args[1].as_str().ok_or_else(|| OperatorError::InvalidArgs {
        operator: "split_index",
        detail: "second arg must be a string literal (separator)".to_string(),
    })?;
    let idx = args[2].as_u64().ok_or_else(|| OperatorError::InvalidArgs {
        operator: "split_index",
        detail: "third arg must be a non-negative integer literal (index)".to_string(),
    })? as usize;
    let segments: Vec<&str> = s.split(sep).collect();
    let part = segments.get(idx).copied().unwrap_or("");
    Ok(Value::String(part.to_string()))
}

fn op_trim_forward(args: &[Value], sample: &Value) -> Result<Value, OperatorError> {
    let s = require_string(&resolve_first_arg(args, sample, "trim")?, "trim")?;
    Ok(Value::String(s.trim().to_string()))
}

fn op_lower_forward(args: &[Value], sample: &Value) -> Result<Value, OperatorError> {
    let s = require_string(&resolve_first_arg(args, sample, "lower")?, "lower")?;
    Ok(Value::String(s.to_lowercase()))
}

fn op_upper_forward(args: &[Value], sample: &Value) -> Result<Value, OperatorError> {
    let s = require_string(&resolve_first_arg(args, sample, "upper")?, "upper")?;
    Ok(Value::String(s.to_uppercase()))
}

/// Target types accepted by `coerce(s, type)`.
fn parse_coerce_target(arg: &Value) -> Result<&'static str, OperatorError> {
    let raw = arg.as_str().ok_or_else(|| OperatorError::InvalidArgs {
        operator: "coerce",
        detail: "second arg must be a string literal naming the target type".to_string(),
    })?;
    match raw {
        "string" => Ok("string"),
        "integer" => Ok("integer"),
        "number" => Ok("number"),
        "boolean" => Ok("boolean"),
        other => Err(OperatorError::InvalidArgs {
            operator: "coerce",
            detail: format!("unsupported target type '{other}'"),
        }),
    }
}

fn coerce_value(value: &Value, target: &'static str) -> Result<Value, OperatorError> {
    match target {
        "string" => Ok(Value::String(value_to_string(value))),
        "integer" => match value {
            Value::Number(n) => n.as_i64().map(|i| Value::Number(i.into())).ok_or_else(|| {
                OperatorError::TypeMismatch {
                    operator: "coerce",
                    expected: "integer-castable number",
                    got: format!("{n}"),
                }
            }),
            Value::String(s) => s
                .trim()
                .parse::<i64>()
                .map(|i| Value::Number(i.into()))
                .map_err(|e| OperatorError::ParseError {
                    operator: "coerce",
                    detail: format!("integer parse failure: {e}"),
                }),
            Value::Bool(b) => Ok(Value::Number(i64::from(*b).into())),
            other => Err(OperatorError::TypeMismatch {
                operator: "coerce",
                expected: "scalar",
                got: type_label(other).to_string(),
            }),
        },
        "number" => match value {
            Value::Number(n) => Ok(Value::Number(n.clone())),
            Value::String(s) => s
                .trim()
                .parse::<f64>()
                .ok()
                .and_then(serde_json::Number::from_f64)
                .map(Value::Number)
                .ok_or_else(|| OperatorError::ParseError {
                    operator: "coerce",
                    detail: format!("number parse failure: '{s}'"),
                }),
            other => Err(OperatorError::TypeMismatch {
                operator: "coerce",
                expected: "scalar",
                got: type_label(other).to_string(),
            }),
        },
        "boolean" => match value {
            Value::Bool(b) => Ok(Value::Bool(*b)),
            Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" => Ok(Value::Bool(true)),
                "false" | "0" | "no" => Ok(Value::Bool(false)),
                other => Err(OperatorError::ParseError {
                    operator: "coerce",
                    detail: format!("boolean parse failure: '{other}'"),
                }),
            },
            Value::Number(n) => Ok(Value::Bool(n.as_f64().unwrap_or(0.0) != 0.0)),
            other => Err(OperatorError::TypeMismatch {
                operator: "coerce",
                expected: "scalar",
                got: type_label(other).to_string(),
            }),
        },
        other => Err(OperatorError::InvalidArgs {
            operator: "coerce",
            detail: format!("unsupported target '{other}'"),
        }),
    }
}

fn op_coerce_forward(args: &[Value], sample: &Value) -> Result<Value, OperatorError> {
    if args.len() != 2 {
        return Err(OperatorError::InvalidArgs {
            operator: "coerce",
            detail: format!("expected 2 arguments, got {}", args.len()),
        });
    }
    let value = resolve_arg(&args[0], sample)?;
    let target = parse_coerce_target(&args[1])?;
    coerce_value(&value, target)
}

fn op_coerce_reverse(args: &[Value], sample: &Value) -> Result<Value, OperatorError> {
    // For the reverse direction, pull the raw input from the sample and coerce
    // it back into a string representation. The validator guarantees the
    // forward target type is lossless.
    if args.len() != 2 {
        return Err(OperatorError::InvalidArgs {
            operator: "coerce",
            detail: format!("expected 2 arguments, got {}", args.len()),
        });
    }
    let value = resolve_arg(&args[0], sample)?;
    Ok(Value::String(value_to_string(&value)))
}

fn op_lookup_forward(args: &[Value], sample: &Value) -> Result<Value, OperatorError> {
    if args.len() != 2 {
        return Err(OperatorError::InvalidArgs {
            operator: "lookup",
            detail: format!("expected 2 arguments, got {}", args.len()),
        });
    }
    let table = args[0]
        .as_object()
        .ok_or_else(|| OperatorError::InvalidArgs {
            operator: "lookup",
            detail: "first arg must be a JSON object literal (the lookup table)".to_string(),
        })?;
    let key_value = resolve_arg(&args[1], sample)?;
    let key = require_string(&key_value, "lookup")?;
    table
        .get(&key)
        .cloned()
        .ok_or(OperatorError::LookupMissing {
            operator: "lookup",
            key,
        })
}

fn op_lookup_reverse(args: &[Value], sample: &Value) -> Result<Value, OperatorError> {
    if args.len() != 2 {
        return Err(OperatorError::InvalidArgs {
            operator: "lookup",
            detail: format!("expected 2 arguments, got {}", args.len()),
        });
    }
    let table = args[0]
        .as_object()
        .ok_or_else(|| OperatorError::InvalidArgs {
            operator: "lookup",
            detail: "first arg must be a JSON object literal (the lookup table)".to_string(),
        })?;
    let value = resolve_arg(&args[1], sample)?;
    // Find the key whose forward-mapped value equals the requested `value`.
    let mut hit: Option<&str> = None;
    for (k, v) in table {
        if v == &value {
            if hit.is_some() {
                // Non-bijective table; reverse is undefined.
                return Err(OperatorError::ParseError {
                    operator: "lookup",
                    detail: "lookup table is not bijective; reverse is undefined".to_string(),
                });
            }
            hit = Some(k.as_str());
        }
    }
    hit.map(|k| Value::String(k.to_string()))
        .ok_or(OperatorError::LookupMissing {
            operator: "lookup",
            key: value_to_string(&value),
        })
}

fn op_template_forward(args: &[Value], sample: &Value) -> Result<Value, OperatorError> {
    if args.is_empty() {
        return Err(OperatorError::InvalidArgs {
            operator: "template",
            detail: "expected pattern + variadic vars".to_string(),
        });
    }
    let pattern = args[0].as_str().ok_or_else(|| OperatorError::InvalidArgs {
        operator: "template",
        detail: "first arg must be a string literal pattern".to_string(),
    })?;
    // Each subsequent arg is `{"name": "<var>", "value": <expr>}`.
    let mut bindings: Vec<(String, String)> = Vec::with_capacity(args.len().saturating_sub(1));
    for arg in &args[1..] {
        let map = arg.as_object().ok_or_else(|| OperatorError::InvalidArgs {
            operator: "template",
            detail: "each var arg must be an object {\"name\": ..., \"value\": <expr>}".to_string(),
        })?;
        let name =
            map.get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| OperatorError::InvalidArgs {
                    operator: "template",
                    detail: "var arg missing string 'name'".to_string(),
                })?;
        let value_expr = map.get("value").ok_or_else(|| OperatorError::InvalidArgs {
            operator: "template",
            detail: "var arg missing 'value'".to_string(),
        })?;
        let resolved = resolve_arg(value_expr, sample)?;
        bindings.push((name.to_string(), value_to_string(&resolved)));
    }
    let mut result = pattern.to_string();
    for (name, value) in bindings {
        result = result.replace(&format!("{{{name}}}"), &value);
    }
    Ok(Value::String(result))
}

fn op_dereference_forward(args: &[Value], sample: &Value) -> Result<Value, OperatorError> {
    if args.len() != 2 {
        return Err(OperatorError::InvalidArgs {
            operator: "dereference",
            detail: format!("expected 2 arguments, got {}", args.len()),
        });
    }
    let fk_path = args[0]
        .get("field")
        .and_then(Value::as_str)
        .ok_or_else(|| OperatorError::InvalidArgs {
            operator: "dereference",
            detail: "first arg must be {\"field\": \"<path>\"}".to_string(),
        })?;
    let target_attr = args[1].as_str().ok_or_else(|| OperatorError::InvalidArgs {
        operator: "dereference",
        detail: "second arg must be a string literal (target attribute)".to_string(),
    })?;
    // Compose `<fk_path>.<target_attr>` and resolve in the sample.
    let combined = format!("{fk_path}.{target_attr}");
    Ok(resolve_field_path(sample, &combined))
}

fn op_date_format_forward(args: &[Value], sample: &Value) -> Result<Value, OperatorError> {
    if args.len() != 3 {
        return Err(OperatorError::InvalidArgs {
            operator: "date_format",
            detail: format!("expected 3 arguments, got {}", args.len()),
        });
    }
    let s = require_string(&resolve_arg(&args[0], sample)?, "date_format")?;
    let fmt_in = args[1].as_str().ok_or_else(|| OperatorError::InvalidArgs {
        operator: "date_format",
        detail: "second arg must be a strftime format string (fmt_in)".to_string(),
    })?;
    let fmt_out = args[2].as_str().ok_or_else(|| OperatorError::InvalidArgs {
        operator: "date_format",
        detail: "third arg must be a strftime format string (fmt_out)".to_string(),
    })?;
    let parsed = NaiveDate::parse_from_str(&s, fmt_in).map_err(|e| OperatorError::ParseError {
        operator: "date_format",
        detail: format!("could not parse '{s}' with fmt '{fmt_in}': {e}"),
    })?;
    Ok(Value::String(parsed.format(fmt_out).to_string()))
}

fn op_date_format_reverse(args: &[Value], sample: &Value) -> Result<Value, OperatorError> {
    if args.len() != 3 {
        return Err(OperatorError::InvalidArgs {
            operator: "date_format",
            detail: format!("expected 3 arguments, got {}", args.len()),
        });
    }
    let s = require_string(&resolve_arg(&args[0], sample)?, "date_format")?;
    let fmt_in = args[1].as_str().ok_or_else(|| OperatorError::InvalidArgs {
        operator: "date_format",
        detail: "second arg must be a strftime format string (fmt_in)".to_string(),
    })?;
    let fmt_out = args[2].as_str().ok_or_else(|| OperatorError::InvalidArgs {
        operator: "date_format",
        detail: "third arg must be a strftime format string (fmt_out)".to_string(),
    })?;
    // Reverse: parse the formatted output (`fmt_out`) and re-render with the
    // source format (`fmt_in`).
    let parsed = NaiveDate::parse_from_str(&s, fmt_out).map_err(|e| OperatorError::ParseError {
        operator: "date_format",
        detail: format!("could not parse '{s}' with fmt '{fmt_out}': {e}"),
    })?;
    Ok(Value::String(parsed.format(fmt_in).to_string()))
}

fn op_tags_merge_forward(args: &[Value], sample: &Value) -> Result<Value, OperatorError> {
    if args.len() != 2 {
        return Err(OperatorError::InvalidArgs {
            operator: "tags_merge",
            detail: format!("expected 2 arguments, got {}", args.len()),
        });
    }
    let a = resolve_arg(&args[0], sample)?;
    let b = resolve_arg(&args[1], sample)?;
    let mut set: BTreeSet<String> = BTreeSet::new();
    for v in [a, b] {
        match v {
            Value::Array(items) => {
                for item in items {
                    set.insert(value_to_string(&item));
                }
            }
            Value::Null => {}
            other => {
                return Err(OperatorError::TypeMismatch {
                    operator: "tags_merge",
                    expected: "array",
                    got: type_label(&other).to_string(),
                });
            }
        }
    }
    Ok(Value::Array(set.into_iter().map(Value::String).collect()))
}

fn op_mac_normalize_forward(args: &[Value], sample: &Value) -> Result<Value, OperatorError> {
    let s = require_string(
        &resolve_first_arg(args, sample, "mac_normalize")?,
        "mac_normalize",
    )?;
    let cleaned: String = s
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    if cleaned.len() != 12 {
        return Err(OperatorError::ParseError {
            operator: "mac_normalize",
            detail: format!(
                "expected 12 hex digits, got {} (input: '{s}')",
                cleaned.len()
            ),
        });
    }
    let mut out = String::with_capacity(17);
    for (i, ch) in cleaned.chars().enumerate() {
        if i > 0 && i % 2 == 0 {
            out.push(':');
        }
        out.push(ch);
    }
    Ok(Value::String(out))
}

fn op_ipv4_prefix_canonical_forward(
    args: &[Value],
    sample: &Value,
) -> Result<Value, OperatorError> {
    let s = require_string(
        &resolve_first_arg(args, sample, "ipv4_prefix_canonical")?,
        "ipv4_prefix_canonical",
    )?;
    let (addr_str, prefix_str) = s.split_once('/').ok_or_else(|| OperatorError::ParseError {
        operator: "ipv4_prefix_canonical",
        detail: format!("expected CIDR (a.b.c.d/N), got '{s}'"),
    })?;
    let octets: Vec<u8> = addr_str
        .split('.')
        .map(|seg| seg.parse::<u8>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| OperatorError::ParseError {
            operator: "ipv4_prefix_canonical",
            detail: format!("invalid IPv4 address '{addr_str}': {e}"),
        })?;
    if octets.len() != 4 {
        return Err(OperatorError::ParseError {
            operator: "ipv4_prefix_canonical",
            detail: format!("expected 4 octets, got {} ('{addr_str}')", octets.len()),
        });
    }
    let prefix: u32 = prefix_str.parse().map_err(|e| OperatorError::ParseError {
        operator: "ipv4_prefix_canonical",
        detail: format!("invalid prefix length '{prefix_str}': {e}"),
    })?;
    if prefix > 32 {
        return Err(OperatorError::ParseError {
            operator: "ipv4_prefix_canonical",
            detail: format!("prefix length must be <= 32, got {prefix}"),
        });
    }
    let mut addr: u32 = ((octets[0] as u32) << 24)
        | ((octets[1] as u32) << 16)
        | ((octets[2] as u32) << 8)
        | (octets[3] as u32);
    let mask: u32 = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    addr &= mask;
    let canon = format!(
        "{}.{}.{}.{}/{}",
        (addr >> 24) & 0xff,
        (addr >> 16) & 0xff,
        (addr >> 8) & 0xff,
        addr & 0xff,
        prefix,
    );
    Ok(Value::String(canon))
}

// === Helpers ================================================================

fn resolve_first_arg(
    args: &[Value],
    sample: &Value,
    op: &'static str,
) -> Result<Value, OperatorError> {
    let arg = args.first().ok_or_else(|| OperatorError::InvalidArgs {
        operator: op,
        detail: "expected at least 1 argument".to_string(),
    })?;
    resolve_arg(arg, sample)
}

/// Resolve an argument node to a value: `{"field": ...}` walks the sample,
/// `{"op": ...}` recursively evaluates a nested expression in the forward
/// direction, otherwise the literal value is returned as-is.
fn resolve_arg(arg: &Value, sample: &Value) -> Result<Value, OperatorError> {
    if let Some(map) = arg.as_object() {
        if let Some(path) = map.get("field").and_then(Value::as_str) {
            return Ok(resolve_field_path(sample, path));
        }
        if map.contains_key("op") {
            // Nested expression: forward-eval and bubble up.
            let (op, nested_args) = parse_op_call(arg).map_err(map_app_error_to_operator)?;
            return forward(op, &nested_args, sample);
        }
        if let Some(literal) = map.get("literal") {
            return Ok(literal.clone());
        }
    }
    Ok(arg.clone())
}

fn map_app_error_to_operator(err: AppError) -> OperatorError {
    OperatorError::InvalidArgs {
        operator: "expr",
        detail: err.to_string(),
    }
}

fn require_string(value: &Value, op: &'static str) -> Result<String, OperatorError> {
    match value {
        Value::String(s) => Ok(s.clone()),
        Value::Null => Ok(String::new()),
        Value::Number(n) => Ok(n.to_string()),
        Value::Bool(b) => Ok(b.to_string()),
        other => Err(OperatorError::TypeMismatch {
            operator: op,
            expected: "string",
            got: type_label(other).to_string(),
        }),
    }
}

fn type_label(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn value_to_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn resolve_field_path(sample: &Value, path: &str) -> Value {
    let mut cursor = sample;
    for segment in path.split('.') {
        if segment.is_empty() {
            continue;
        }
        match cursor.get(segment) {
            Some(next) => cursor = next,
            None => return Value::Null,
        }
    }
    cursor.clone()
}
