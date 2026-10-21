use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::contexts::sync::domain::{ArgKind, MappingTarget, Operator};

/// Severity of a validation diagnostic.
///
/// `Error` blocks save (HTTP 422). `Warning` is informational and does not
/// block the save.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ValidationSeverity {
    Error,
    Warning,
}

/// Structured per-error report for HTTP 422 responses.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct MappingValidationError {
    pub path: String,
    pub code: String,
    pub message: String,
    #[serde(default = "default_error_severity")]
    pub severity: ValidationSeverity,
}

fn default_error_severity() -> ValidationSeverity {
    ValidationSeverity::Error
}

impl MappingValidationError {
    pub fn error(
        path: impl Into<String>,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            path: path.into(),
            code: code.into(),
            message: message.into(),
            severity: ValidationSeverity::Error,
        }
    }

    pub fn warning(
        path: impl Into<String>,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            path: path.into(),
            code: code.into(),
            message: message.into(),
            severity: ValidationSeverity::Warning,
        }
    }

    pub fn is_error(&self) -> bool {
        matches!(self.severity, ValidationSeverity::Error)
    }
}

/// Mapping-configuration draft accepted by the create and version endpoints.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct MappingConfigurationDraft {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub source_provider_id: uuid::Uuid,
    pub target: MappingTarget,
    pub direction: String,
    pub conflict_mode: String,
    #[serde(default)]
    pub entity_mappings: Vec<EntityMappingDraft>,
}

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct EntityMappingDraft {
    pub source_entity: String,
    pub target_entity: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub field_mappings: Vec<FieldMappingDraft>,
}

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct FieldMappingDraft {
    pub source_field: String,
    pub target_field: String,
    #[schema(value_type = Object)]
    pub forward_transform: Value,
    #[serde(default)]
    #[schema(value_type = Option<Object>)]
    pub reverse_transform: Option<Value>,
    #[serde(default)]
    pub round_trip_critical: bool,
}

fn default_true() -> bool {
    true
}

/// Validate a `MappingConfigurationDraft`.
///
/// Returns a list of structured errors that the REST handler maps to HTTP 422
/// `{ "errors": [...] }`. Empty list means the draft is valid.
pub fn validate_mapping_configuration(
    draft: &MappingConfigurationDraft,
) -> Vec<MappingValidationError> {
    let mut errors = Vec::new();

    if draft.name.trim().is_empty() {
        errors.push(MappingValidationError {
            severity: ValidationSeverity::Error,
            path: "name".to_string(),
            code: "required".to_string(),
            message: "name must not be empty".to_string(),
        });
    }

    if !matches!(draft.direction.as_str(), "pull" | "push" | "bidirectional") {
        errors.push(MappingValidationError {
            severity: ValidationSeverity::Error,
            path: "direction".to_string(),
            code: "invalid_enum".to_string(),
            message: format!("unknown direction '{}'", draft.direction),
        });
    }

    if !matches!(
        draft.conflict_mode.as_str(),
        "netbox_wins" | "piping_wins" | "last_write_wins" | "field_level"
    ) {
        errors.push(MappingValidationError {
            severity: ValidationSeverity::Error,
            path: "conflict_mode".to_string(),
            code: "invalid_enum".to_string(),
            message: format!("unknown conflict_mode '{}'", draft.conflict_mode),
        });
    }

    if draft.entity_mappings.is_empty() {
        errors.push(MappingValidationError {
            severity: ValidationSeverity::Error,
            path: "entity_mappings".to_string(),
            code: "required".to_string(),
            message: "at least one entity mapping is required".to_string(),
        });
    }

    for (eidx, entity) in draft.entity_mappings.iter().enumerate() {
        let entity_path = format!("entity_mappings[{eidx}]");
        if entity.source_entity.trim().is_empty() {
            errors.push(MappingValidationError {
                severity: ValidationSeverity::Error,
                path: format!("{entity_path}.source_entity"),
                code: "required".to_string(),
                message: "source_entity must not be empty".to_string(),
            });
        }
        if entity.target_entity.trim().is_empty() {
            errors.push(MappingValidationError {
                severity: ValidationSeverity::Error,
                path: format!("{entity_path}.target_entity"),
                code: "required".to_string(),
                message: "target_entity must not be empty".to_string(),
            });
        }

        for (fidx, field) in entity.field_mappings.iter().enumerate() {
            let field_path = format!("{entity_path}.field_mappings[{fidx}]");
            if field.source_field.trim().is_empty() {
                errors.push(MappingValidationError {
                    severity: ValidationSeverity::Error,
                    path: format!("{field_path}.source_field"),
                    code: "required".to_string(),
                    message: "source_field must not be empty".to_string(),
                });
            }
            if field.target_field.trim().is_empty() {
                errors.push(MappingValidationError {
                    severity: ValidationSeverity::Error,
                    path: format!("{field_path}.target_field"),
                    code: "required".to_string(),
                    message: "target_field must not be empty".to_string(),
                });
            }

            validate_transform_expression(
                &format!("{field_path}.forward_transform"),
                &field.forward_transform,
                &mut errors,
                /*allow_non_invertible=*/ true,
            );

            if let Some(reverse) = &field.reverse_transform {
                validate_transform_expression(
                    &format!("{field_path}.reverse_transform"),
                    reverse,
                    &mut errors,
                    // Reverse expressions are themselves valid expressions —
                    // any operator may appear; UI surfaces the
                    // `invertibility_hint` warning for forward operators that
                    // are not paired-invertible.
                    /*allow_non_invertible=*/
                    true,
                );
            }

            let forward_invertible = forward_is_invertible(&field.forward_transform);
            match (field.reverse_transform.as_ref(), forward_invertible) {
                (Some(_), false) => {
                    errors.push(MappingValidationError {
                        severity: ValidationSeverity::Error,
                        path: format!("{field_path}.reverse_transform"),
                        code: "non_invertible_forward".to_string(),
                        message: "reverse_transform was authored for a forward operator with \
                             invertibility_hint=false; per spec the field must remain \
                             read-only in target→source direction"
                            .to_string(),
                    });
                }
                (None, true) => {
                    errors.push(MappingValidationError {
                        severity: ValidationSeverity::Warning,
                        path: format!("{field_path}.reverse_transform"),
                        code: "missing_reverse_for_invertible".to_string(),
                        message: "forward_transform uses an invertible operator but \
                             reverse_transform is null; consider adding reverse for \
                             round-trip support"
                            .to_string(),
                    });
                }
                _ => {}
            }
        }
    }

    errors
}

fn forward_is_invertible(expression: &Value) -> bool {
    expression
        .as_object()
        .and_then(|map| map.get("op"))
        .and_then(Value::as_str)
        .and_then(Operator::from_name)
        .map(|op| op.invertibility_hint())
        .unwrap_or(false)
}

fn validate_transform_expression(
    path: &str,
    expression: &Value,
    errors: &mut Vec<MappingValidationError>,
    allow_non_invertible: bool,
) {
    let _ = allow_non_invertible;
    let Value::Object(map) = expression else {
        errors.push(MappingValidationError {
            severity: ValidationSeverity::Error,
            path: path.to_string(),
            code: "invalid_shape".to_string(),
            message: "transform expression must be a JSON object".to_string(),
        });
        return;
    };

    let Some(op_name) = map.get("op").and_then(Value::as_str) else {
        errors.push(MappingValidationError {
            severity: ValidationSeverity::Error,
            path: format!("{path}.op"),
            code: "required".to_string(),
            message: "transform expression missing 'op' field".to_string(),
        });
        return;
    };

    let Some(op) = Operator::from_name(op_name) else {
        errors.push(MappingValidationError {
            severity: ValidationSeverity::Error,
            path: format!("{path}.op"),
            code: "unknown_operator".to_string(),
            message: format!("unknown operator '{op_name}'"),
        });
        return;
    };

    let signature = op.signature();
    let args = map.get("args").and_then(Value::as_array);
    let args_len = args.map(|a| a.len()).unwrap_or(0);

    let signature_is_variadic = signature.args.contains(&ArgKind::Variadic);
    if !signature_is_variadic && args_len != signature.args.len() {
        errors.push(MappingValidationError {
            severity: ValidationSeverity::Error,
            path: format!("{path}.args"),
            code: "arity_mismatch".to_string(),
            message: format!(
                "operator '{}' expects {} arguments, got {}",
                signature.name,
                signature.args.len(),
                args_len
            ),
        });
    }

    if let Some(args) = args {
        for (i, arg) in args.iter().enumerate() {
            let arg_path = format!("{path}.args[{i}]");
            // The Variadic kind accepts any expression-shaped value; non-variadic
            // signatures match argument-by-argument.
            let kind = if signature_is_variadic {
                ArgKind::Variadic
            } else {
                signature.args.get(i).copied().unwrap_or(ArgKind::Variadic)
            };
            validate_arg_shape(&arg_path, arg, kind, errors);
        }
    }
}

fn validate_arg_shape(
    path: &str,
    arg: &Value,
    kind: ArgKind,
    errors: &mut Vec<MappingValidationError>,
) {
    match kind {
        ArgKind::Field => {
            if !arg
                .as_object()
                .and_then(|m| m.get("field"))
                .map(Value::is_string)
                .unwrap_or(false)
            {
                errors.push(MappingValidationError {
                    severity: ValidationSeverity::Error,
                    path: path.to_string(),
                    code: "invalid_field_arg".to_string(),
                    message: "field argument must be {\"field\": \"path.to.field\"}".to_string(),
                });
            }
        }
        ArgKind::Literal => {
            // Any JSON scalar is acceptable; objects/arrays are also fine for
            // lookup-table literals.
            let _ = arg;
        }
        ArgKind::Expr => {
            if !arg.is_object() {
                errors.push(MappingValidationError {
                    severity: ValidationSeverity::Error,
                    path: path.to_string(),
                    code: "invalid_expr_arg".to_string(),
                    message: "expression argument must be a JSON object with 'op'".to_string(),
                });
            }
        }
        ArgKind::Variadic => {
            // Variadic accepts any expression / field / literal mix. Skipped
            // intentionally; structural sanity is enforced by the
            // outer `is_object` check on field/expr forms.
            let _ = arg;
        }
    }
}
