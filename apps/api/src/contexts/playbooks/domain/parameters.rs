//! Canonical launch-parameter contract.
//!
//! The declaration is the existing JSON Schema document (`vars_schema`) that the
//! playbook catalog already references through `vars_schema_file`. A job
//! template names exactly one provenance for it:
//!
//! * [`VarsSchemaSource::Inline`] stores the document with the template;
//! * [`VarsSchemaSource::ProjectFile`] names a JSON file in the SAME pinned
//!   project revision as the playbooks, so a launch cannot mix a revision with
//!   another revision's declarations;
//! * [`VarsSchemaSource::None`] declares no parameters and therefore rejects
//!   every supplied value.
//!
//! Values resolve in one order: schema `default` → job template `extra_vars` →
//! run/schedule overrides. Presence is preserved, so `false`, `0`, `""` and
//! `null` are values and never mean "missing".
//!
//! Secrets are declared with `writeOnly: true` plus `format:
//! "secret-reference"`. The declaration is a reference to a secret already
//! declared in the template's `options.secret_refs` under the same name, which
//! is also the runner environment name. A secret-reference parameter is never an
//! extra-vars value: it is excluded from the effective values and validated only
//! against that declared reference. The runtime env gate materializes the secret;
//! plaintext never enters the contract, the launch snapshot or an error message.
//!
//! The schema revision is the lowercase hex SHA-256 of the canonical JSON
//! encoding of the document: object keys sorted bytewise, no insignificant
//! whitespace, numbers serialized exactly as declared. It is a digest of the
//! declared document, not a semantic equivalence class.

use std::collections::{BTreeMap, BTreeSet};

use openssl::sha::sha256;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::contexts::playbooks::domain::workspace::{
    ProjectEntry, ProjectEntryContent, validate_project_entry,
};
use crate::errors::AppError;

/// JSON Schema `format` value that marks a parameter as a secret reference.
pub const SECRET_REFERENCE_FORMAT: &str = "secret-reference";

/// The single declared provenance of a job template's parameter schema.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VarsSchemaSource {
    /// No declared parameters; a launch that supplies values is rejected.
    #[default]
    None,
    /// The JSON Schema document is stored with the job template.
    Inline { schema: Value },
    /// The JSON Schema document is a file in the pinned project revision.
    ProjectFile { path: String },
}

/// Run/schedule overrides on the wire and in persistence.
///
/// The named `extra_vars` object keeps room for future launch options without
/// overloading arbitrary top-level keys. There is no second accepted shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ParameterOverrides {
    #[serde(default = "empty_object")]
    pub extra_vars: Value,
}

impl Default for ParameterOverrides {
    fn default() -> Self {
        Self {
            extra_vars: empty_object(),
        }
    }
}

impl ParameterOverrides {
    pub fn extra_vars(&self) -> &Value {
        &self.extra_vars
    }
}

fn empty_object() -> Value {
    Value::Object(Map::new())
}

/// A validated schema document and the revision of its declaration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ResolvedVarsSchema {
    pub schema: Value,
    /// Lowercase hex SHA-256 of the canonical JSON encoding of `schema`.
    pub revision: String,
}

/// Effective values and the schema revision captured for one launch.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedParameters {
    /// `None` when the template declares no parameters at all.
    pub schema: Option<ResolvedVarsSchema>,
    /// The effective extra vars: defaults, then job values, then overrides.
    /// Secret-reference declarations never appear here; they bind through
    /// `options.secret_refs` and the runtime env gate.
    pub effective: Value,
}

/// Resolve the declared provenance against the pinned project revision.
///
/// `ProjectFile` reads the exact entry from `entries`; a missing, symlinked or
/// non-UTF-8 file fails closed instead of falling back to another source.
pub fn resolve_vars_schema(
    source: &VarsSchemaSource,
    entries: &[ProjectEntry],
) -> Result<Option<ResolvedVarsSchema>, AppError> {
    match source {
        VarsSchemaSource::None => Ok(None),
        VarsSchemaSource::Inline { schema } => compile_vars_schema(schema).map(Some),
        VarsSchemaSource::ProjectFile { path } => {
            validate_schema_path(path)?;
            let entry = entries
                .iter()
                .find(|entry| entry.path == *path)
                .ok_or_else(|| {
                    AppError::Validation(format!(
                        "vars_schema_file `{path}` is not present in the pinned project revision"
                    ))
                })?;
            let ProjectEntryContent::File { bytes, .. } = &entry.content else {
                return Err(AppError::Validation(format!(
                    "vars_schema_file `{path}` must be a regular file in the pinned project revision"
                )));
            };
            let text = std::str::from_utf8(bytes).map_err(|_| {
                AppError::Validation(format!(
                    "vars_schema_file `{path}` must be UTF-8 JSON in the pinned project revision"
                ))
            })?;
            let schema: Value = serde_json::from_str(text).map_err(|_| {
                AppError::Validation(format!(
                    "vars_schema_file `{path}` is not valid JSON in the pinned project revision"
                ))
            })?;
            compile_vars_schema(&schema).map(Some)
        }
    }
}

/// Validate a schema document and compute its revision.
///
/// The document must be a JSON Schema object with a `properties` object. Every
/// declaration must be an object. Secret declarations must use exactly
/// `writeOnly: true` with `format: "secret-reference"`, a string type and no
/// default, and their name must be a valid, non-reserved runner environment
/// name because that name is also the environment variable carrying the value.
///
/// Remote `$ref` or an unknown `$schema` draft fails compilation, so an
/// unsupported constraint is rejected rather than silently ignored.
pub fn compile_vars_schema(schema: &Value) -> Result<ResolvedVarsSchema, AppError> {
    let invalid = || AppError::Validation("Invalid playbook vars_schema document".into());
    let schema_object = schema.as_object().ok_or_else(invalid)?;
    if schema_object.get("type").and_then(Value::as_str) != Some("object") {
        return Err(invalid());
    }
    let declarations = schema_object
        .get("properties")
        .and_then(Value::as_object)
        .ok_or_else(invalid)?;
    for (name, declaration) in declarations {
        validate_parameter_name(name)?;
        if reserved_name(name) {
            return Err(AppError::Validation(format!(
                "Playbook parameter `{name}` is reserved by the runtime"
            )));
        }
        let declaration = declaration.as_object().ok_or_else(invalid)?;
        let write_only = declaration.get("writeOnly") == Some(&Value::Bool(true));
        let secret_format =
            declaration.get("format").and_then(Value::as_str) == Some(SECRET_REFERENCE_FORMAT);
        if write_only != secret_format {
            return Err(AppError::Validation(format!(
                "Playbook parameter `{name}` must declare both writeOnly: true and format: {SECRET_REFERENCE_FORMAT}"
            )));
        }
        if write_only {
            validate_secret_declaration(name, declaration)?;
        }
    }
    jsonschema::meta::validate(schema)
        .map_err(|_| AppError::Validation("Unsupported playbook vars_schema constraint".into()))?;
    jsonschema::validator_for(schema)
        .map_err(|_| AppError::Validation("Unsupported playbook vars_schema constraint".into()))?;
    Ok(ResolvedVarsSchema {
        revision: schema_revision(schema)?,
        schema: schema.clone(),
    })
}

/// Merge schema defaults, job template values and run/schedule overrides, then
/// validate the effective object against the extra-vars schema.
pub fn resolve_parameters(
    schema: Option<&ResolvedVarsSchema>,
    job: &Value,
    overrides: &Value,
    declared_secret_refs: &BTreeMap<String, Uuid>,
) -> Result<ResolvedParameters, AppError> {
    let effective = merge_parameter_layers(schema, job, overrides, declared_secret_refs)?;
    if let Some(schema) = schema {
        validate_values(&extra_vars_schema(&schema.schema), &effective, false)?;
    }
    Ok(ResolvedParameters {
        schema: schema.cloned(),
        effective,
    })
}

/// Validate values that are already fixed on a job template (the job layer and
/// its defaults) without requiring parameters a launch will supply later.
pub fn validate_parameter_declarations(
    schema: Option<&ResolvedVarsSchema>,
    job: &Value,
    overrides: &Value,
    declared_secret_refs: &BTreeMap<String, Uuid>,
) -> Result<(), AppError> {
    let effective = merge_parameter_layers(schema, job, overrides, declared_secret_refs)?;
    if let Some(schema) = schema {
        validate_values(&extra_vars_schema(&schema.schema), &effective, true)?;
    }
    Ok(())
}

/// Resolve the extra-vars layers. Secret-reference declarations are reference
/// identity only: a supplied value must equal the template's declared reference,
/// but it is never copied into the effective values. The runtime env gate
/// materializes the secret under the same name.
fn merge_parameter_layers(
    schema: Option<&ResolvedVarsSchema>,
    job: &Value,
    overrides: &Value,
    declared_secret_refs: &BTreeMap<String, Uuid>,
) -> Result<Value, AppError> {
    let empty = Map::new();
    let declarations = schema
        .and_then(|schema| schema.schema.get("properties"))
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let mut effective = Map::new();
    for (name, declaration) in declarations {
        if is_secret_declaration(declaration) {
            continue;
        }
        if let Some(value) = declaration.get("default") {
            effective.insert(name.clone(), value.clone());
        }
    }
    for layer in [job, overrides] {
        for (name, value) in layer.as_object().ok_or_else(|| {
            AppError::Validation("Playbook parameter values must be an object".into())
        })? {
            validate_parameter_name(name)?;
            if reserved_name(name) {
                return Err(AppError::Validation(format!(
                    "Playbook parameter `{name}` is reserved by the runtime"
                )));
            }
            let declaration = declarations.get(name).ok_or_else(|| {
                AppError::Validation(format!(
                    "Playbook parameter `{name}` is not declared by vars_schema"
                ))
            })?;
            if is_secret_declaration(declaration) {
                let secret_id = value
                    .as_str()
                    .filter(|value| !value.trim().is_empty())
                    .and_then(|value| Uuid::parse_str(value.trim()).ok())
                    .ok_or_else(|| {
                        AppError::Validation(format!(
                            "Secret parameter `{name}` must be a UUID secret reference"
                        ))
                    })?;
                if declared_secret_refs.get(name) != Some(&secret_id) {
                    return Err(AppError::Validation(format!(
                        "Secret parameter `{name}` must reference the secret declared in options.secret_refs; selecting a secret per launch is not supported"
                    )));
                }
                continue;
            }
            effective.insert(name.clone(), value.clone());
        }
    }
    if let Some(required) = schema
        .and_then(|schema| schema.schema.get("required"))
        .and_then(Value::as_array)
    {
        for name in required.iter().filter_map(Value::as_str) {
            if declarations.get(name).is_some_and(is_secret_declaration)
                && !declared_secret_refs.contains_key(name)
            {
                return Err(AppError::Validation(format!(
                    "Secret parameter `{name}` is required but has no declared secret reference"
                )));
            }
        }
    }
    Ok(Value::Object(effective))
}

/// The schema applied to extra-vars: secret-reference declarations are supplied
/// by the runtime env gate, not by the values object, so neither their
/// properties nor their `required` entries participate in value validation.
fn extra_vars_schema(schema: &Value) -> Value {
    let mut relaxed = schema.clone();
    let Some(object) = relaxed.as_object_mut() else {
        return relaxed;
    };
    let secret_names: BTreeSet<String> = object
        .get("properties")
        .and_then(Value::as_object)
        .map(|properties| {
            properties
                .iter()
                .filter(|(_, declaration)| is_secret_declaration(declaration))
                .map(|(name, _)| name.clone())
                .collect()
        })
        .unwrap_or_default();
    if let Some(properties) = object.get_mut("properties").and_then(Value::as_object_mut) {
        properties.retain(|name, _| !secret_names.contains(name));
    }
    if let Some(required) = object.get_mut("required").and_then(Value::as_array_mut) {
        required.retain(|name| {
            name.as_str()
                .is_none_or(|name| !secret_names.contains(name))
        });
    }
    relaxed
}

fn validate_values(
    schema: &Value,
    values: &Value,
    allow_missing_required: bool,
) -> Result<(), AppError> {
    let validator = if allow_missing_required {
        let mut relaxed = schema.clone();
        if let Some(object) = relaxed.as_object_mut() {
            object.remove("required");
        }
        jsonschema::validator_for(&relaxed)
    } else {
        jsonschema::validator_for(schema)
    }
    .map_err(|_| AppError::Validation("Unsupported playbook vars_schema constraint".into()))?;
    let mut errors = validator.iter_errors(values);
    let Some(first) = errors.next() else {
        return Ok(());
    };
    let field = first.instance_path().to_string();
    Err(AppError::Validation(if field.is_empty() || field == "/" {
        "Playbook parameter values do not satisfy the declared vars_schema".into()
    } else {
        format!("Playbook parameter values do not satisfy the declared vars_schema at `{field}`")
    }))
}

fn validate_secret_declaration(
    name: &str,
    declaration: &Map<String, Value>,
) -> Result<(), AppError> {
    if declaration.get("type").and_then(Value::as_str) != Some("string") {
        return Err(AppError::Validation(format!(
            "Secret parameter `{name}` must declare type: string"
        )));
    }
    if declaration.contains_key("default") {
        return Err(AppError::Validation(format!(
            "Secret parameter `{name}` must not declare a default value"
        )));
    }
    if name.is_empty()
        || name.chars().count() > 128
        || !name.bytes().enumerate().all(|(index, byte)| match byte {
            b'A'..=b'Z' | b'_' => true,
            b'0'..=b'9' => index > 0,
            _ => false,
        })
    {
        return Err(AppError::Validation(format!(
            "Secret parameter `{name}` must match [A-Z_][A-Z0-9_]* because it is also the runner environment name"
        )));
    }
    if name.starts_with("PIPING_")
        || name.starts_with("ANSIBLE_")
        || name.starts_with("SSH_")
        || matches!(name, "HOME" | "PATH")
    {
        return Err(AppError::Validation(format!(
            "Secret parameter `{name}` is reserved by the runtime environment"
        )));
    }
    Ok(())
}

fn is_secret_declaration(declaration: &Value) -> bool {
    declaration.get("writeOnly") == Some(&Value::Bool(true))
        && declaration.get("format").and_then(Value::as_str) == Some(SECRET_REFERENCE_FORMAT)
}

fn validate_schema_path(path: &str) -> Result<(), AppError> {
    let probe = ProjectEntry {
        path: path.to_string(),
        content: ProjectEntryContent::File {
            bytes: Vec::new(),
            executable: false,
        },
    };
    validate_project_entry(&probe).map_err(|_| {
        AppError::Validation(
            "vars_schema_file must be a relative path without traversal".to_string(),
        )
    })
}

fn validate_parameter_name(name: &str) -> Result<(), AppError> {
    if name.is_empty()
        || !name.bytes().enumerate().all(|(index, byte)| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'_' => true,
            b'0'..=b'9' => index > 0,
            _ => false,
        })
    {
        return Err(AppError::Validation(format!(
            "Playbook parameter `{name}` must be a valid Ansible variable name"
        )));
    }
    Ok(())
}

/// Ansible magic variables and the product's runtime namespace cannot be
/// supplied as launch parameters, because extra vars would override them.
fn reserved_name(name: &str) -> bool {
    const PREFIXES: &[&str] = &["ansible_", "piping_runtime_"];
    const NAMES: &[&str] = &[
        "environment",
        "group_names",
        "groups",
        "hostvars",
        "inventory_dir",
        "inventory_file",
        "inventory_hostname",
        "inventory_hostname_short",
        "item",
        "omit",
        "playbook_dir",
        "role_name",
        "role_names",
        "role_path",
        "role_uuid",
    ];
    PREFIXES.iter().any(|prefix| name.starts_with(prefix)) || NAMES.contains(&name)
}

fn schema_revision(schema: &Value) -> Result<String, AppError> {
    let bytes = serde_json::to_vec(&canonical_json(schema))
        .map_err(|_| AppError::Validation("Invalid playbook vars_schema document".into()))?;
    Ok(hex::encode(sha256(&bytes)))
}

fn canonical_json(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut entries: Vec<(&String, &Value)> = object.iter().collect();
            entries.sort_by(|left, right| left.0.cmp(right.0));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key.clone(), canonical_json(value)))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.iter().map(canonical_json).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema() -> ResolvedVarsSchema {
        compile_vars_schema(&json!({"type":"object","properties":{
            "flag":{"type":"boolean","default":true},
            "count":{"type":"integer","default":1},
            "text":{"type":"string","default":"x"},
            "nullable":{"type":["string","null"],"default":"x"}
        },"required":["flag","count"]}))
        .unwrap()
    }

    #[test]
    fn precedence_preserves_false_zero_empty_and_null() {
        let result = resolve_parameters(
            Some(&schema()),
            &json!({"flag":false,"count":0}),
            &json!({"text":"","nullable":null}),
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(
            result.effective,
            json!({"flag":false,"count":0,"text":"","nullable":null})
        );
    }

    #[test]
    fn later_layers_win_and_missing_values_stay_missing() {
        let result = resolve_parameters(
            Some(&schema()),
            &json!({"count":2}),
            &json!({"count":3}),
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(
            result.effective,
            json!({"flag":true,"count":3,"text":"x","nullable":"x"})
        );

        let result = resolve_parameters(None, &json!({}), &json!({}), &BTreeMap::new()).unwrap();
        assert_eq!(result.effective, json!({}));
        assert_eq!(result.schema, None);
    }

    #[test]
    fn rejects_invalid_missing_unknown_and_reserved_values() {
        let schema = compile_vars_schema(&json!({"type":"object","properties":{"count":{"type":"integer","minimum":1}},"required":["count"]})).unwrap();
        assert!(
            resolve_parameters(Some(&schema), &json!({}), &json!({}), &BTreeMap::new()).is_err()
        );
        assert!(
            resolve_parameters(
                Some(&schema),
                &json!({"count":0}),
                &json!({}),
                &BTreeMap::new()
            )
            .is_err()
        );
        assert!(
            resolve_parameters(
                Some(&schema),
                &json!({"count":1,"other":1}),
                &json!({}),
                &BTreeMap::new()
            )
            .is_err()
        );
        assert!(
            resolve_parameters(
                Some(&schema),
                &json!({"count":1}),
                &json!({"ansible_user":"root"}),
                &BTreeMap::new()
            )
            .is_err()
        );
        assert!(
            resolve_parameters(None, &json!({"anything":1}), &json!({}), &BTreeMap::new()).is_err()
        );
    }

    #[test]
    fn secret_values_are_reference_identity_only() {
        let secret_schema = compile_vars_schema(&json!({"type":"object","properties":{
            "ROOT_PASSWORD":{"type":"string","writeOnly":true,"format":"secret-reference"},
            "PORT":{"type":"integer","default":22}
        },"required":["ROOT_PASSWORD"]}))
        .unwrap();
        let secret_id = Uuid::now_v7();
        let declared = BTreeMap::from([("ROOT_PASSWORD".to_string(), secret_id)]);
        let result = resolve_parameters(
            Some(&secret_schema),
            &json!({"ROOT_PASSWORD": secret_id.to_string()}),
            &json!({}),
            &declared,
        )
        .unwrap();
        assert_eq!(result.effective, json!({"PORT":22}));
        assert!(
            !result
                .effective
                .as_object()
                .unwrap()
                .contains_key("ROOT_PASSWORD")
        );

        assert!(
            resolve_parameters(
                Some(&secret_schema),
                &json!({"ROOT_PASSWORD": Uuid::now_v7().to_string()}),
                &json!({}),
                &declared,
            )
            .is_err()
        );
        assert!(
            resolve_parameters(
                Some(&secret_schema),
                &json!({"ROOT_PASSWORD": "not-a-uuid"}),
                &json!({}),
                &declared,
            )
            .is_err()
        );
        assert!(
            resolve_parameters(
                Some(&secret_schema),
                &json!({"ROOT_PASSWORD": secret_id.to_string()}),
                &json!({}),
                &BTreeMap::new(),
            )
            .is_err()
        );
        assert!(
            resolve_parameters(
                Some(&secret_schema),
                &json!({}),
                &json!({}),
                &BTreeMap::new()
            )
            .is_err()
        );
    }

    #[test]
    fn declarations_reject_unsupported_and_unsafe_shapes() {
        for invalid in [
            json!({"type":"array","properties":{}}),
            json!({"type":"object"}),
            json!({"type":"object","properties":{"x":"string"}}),
            json!({"type":"object","properties":{"ansible_user":{"type":"string"}}}),
            json!({"type":"object","properties":{"bad.name":{"type":"string"}}}),
            json!({"type":"object","properties":{"S":{"type":"string","writeOnly":true}}}),
            json!({"type":"object","properties":{"S":{"type":"string","writeOnly":true,"format":"secret-reference","default":"x"}}}),
            json!({"type":"object","properties":{"lower":{"type":"string","writeOnly":true,"format":"secret-reference"}}}),
            json!({"type":"object","properties":{"x":{"$ref":"https://example.test/schema.json"}}}),
        ] {
            assert!(compile_vars_schema(&invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn revision_ignores_object_key_order() {
        let left =
            compile_vars_schema(&json!({"type":"object","properties":{"a":{"type":"string"}}}))
                .unwrap();
        let right =
            compile_vars_schema(&json!({"properties":{"a":{"type":"string"}},"type":"object"}))
                .unwrap();
        assert_eq!(left.revision, right.revision);
        assert_eq!(left.revision.len(), 64);
    }

    #[test]
    fn project_file_uses_the_pinned_revision_entry_only() {
        let entry = ProjectEntry {
            path: "vars/site.json".into(),
            content: ProjectEntryContent::File {
                bytes: br#"{"type":"object","properties":{}}"#.to_vec(),
                executable: false,
            },
        };
        let source = VarsSchemaSource::ProjectFile {
            path: "vars/site.json".into(),
        };
        let resolved = resolve_vars_schema(&source, &[entry]).unwrap().unwrap();
        assert_eq!(resolved.schema, json!({"type":"object","properties":{}}));

        assert!(resolve_vars_schema(&source, &[]).is_err());
        assert!(
            resolve_vars_schema(
                &VarsSchemaSource::ProjectFile {
                    path: "../vars/site.json".into()
                },
                &[],
            )
            .is_err()
        );
        assert!(
            resolve_vars_schema(
                &VarsSchemaSource::ProjectFile {
                    path: "vars/link.json".into()
                },
                &[ProjectEntry {
                    path: "vars/link.json".into(),
                    content: ProjectEntryContent::Symlink {
                        target: "site.json".into()
                    },
                }],
            )
            .is_err()
        );
    }

    #[test]
    fn template_validation_allows_required_values_from_a_later_launch() {
        let schema = compile_vars_schema(&json!({"type":"object","properties":{"count":{"type":"integer","minimum":1}},"required":["count"]})).unwrap();
        validate_parameter_declarations(Some(&schema), &json!({}), &json!({}), &BTreeMap::new())
            .unwrap();
        assert!(
            validate_parameter_declarations(
                Some(&schema),
                &json!({"count":0}),
                &json!({}),
                &BTreeMap::new()
            )
            .is_err()
        );
        assert!(
            validate_parameter_declarations(
                Some(&schema),
                &json!({"unknown":1}),
                &json!({}),
                &BTreeMap::new()
            )
            .is_err()
        );
    }
}
