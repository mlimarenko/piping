//! Official playbook catalog manifest (`piping-playbooks.yaml`).
//!
//! The manifest is the source of truth for official playbook CONTENT and
//! capability flags only. Instance configuration (target sets, assignments,
//! schedules, service principals) stays in the backend seed code — see
//! `ensure_system_inventory_instance_config` in the loader.

use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use super::super::project_bundle::validate_project_path;
use crate::errors::AppError;

/// Well-known manifest file name at the catalog directory root.
pub const OFFICIAL_CATALOG_MANIFEST_FILE: &str = "piping-playbooks.yaml";

/// The only manifest schema version this loader understands.
pub const OFFICIAL_CATALOG_API_VERSION: &str = "piping.space/playbook-catalog/v1";

/// Maximum playbook content size in bytes (matches the in-app editor limit).
pub const OFFICIAL_CATALOG_MAX_CONTENT_BYTES: usize = 1_048_576;

const ALLOWED_RISKS: [&str; 4] = ["low", "medium", "high", "critical"];
const ALLOWED_CONNECTION_DEFAULTS: [&str; 3] = ["ssh", "winrm", "local"];
const ALLOWED_STATUSES: [&str; 3] = ["draft", "active", "archived"];
const MAX_TIMEOUT_SECONDS: i32 = 86_400;

/// Parsed `piping-playbooks.yaml`.
#[derive(Debug, Clone, Deserialize)]
pub struct CatalogManifest {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    /// Provenance for humans and audit tooling; never used as identity.
    #[allow(
        dead_code,
        reason = "external manifest field: parsed to validate the file's required `catalog:` block; provenance only, never read by the loader"
    )]
    pub catalog: CatalogMeta,
    pub playbooks: Vec<ManifestPlaybook>,
}

/// Catalog-level provenance metadata.
#[derive(Debug, Clone, Deserialize)]
pub struct CatalogMeta {
    #[allow(
        dead_code,
        reason = "external manifest field: parsed to validate the file's required `catalog:` block; provenance only, never read by the loader"
    )]
    pub source_repo: String,
    #[allow(
        dead_code,
        reason = "external manifest field: parsed to validate the file's required `catalog:` block; provenance only, never read by the loader"
    )]
    pub base_path: String,
}

/// One official playbook entry.
#[derive(Debug, Clone, Deserialize)]
pub struct ManifestPlaybook {
    pub slug: String,
    /// Optional pinned id for non-system entries. The privileged system
    /// inventory playbook never sets this — its historical fixed UUID is
    /// pinned by the loader via the well-known slug.
    #[serde(default)]
    pub id: Option<Uuid>,
    pub name: String,
    pub description: String,
    pub category: String,
    pub risk: String,
    pub runtime: String,
    pub connection_default: String,
    #[serde(default = "default_status")]
    pub status: String,

    /// Path of the playbook body relative to the catalog directory; its raw
    /// bytes become `playbook_version.content`.
    pub entrypoint_file: String,
    #[serde(default = "default_language")]
    pub language: String,
    pub timeout_seconds: i32,
    #[serde(default = "default_object")]
    pub input_schema: Value,
    pub output_schema: Value,
    pub token_policy: Value,
    pub launch_options: Value,

    /// JSON Schema file for launch variables; exactly one of this or the
    /// inline `vars_schema` must be present.
    #[serde(default)]
    pub vars_schema_file: Option<String>,
    #[serde(default)]
    pub vars_schema: Option<Value>,

    /// `true` maps to `source_kind='system'` and attaches the built-in
    /// instance configuration; `false` maps to `source_kind='official'`.
    #[serde(default)]
    pub system: bool,
    /// Maps to `playbook.inventory_rest_enabled`; gates the scoped inventory
    /// REST privilege enforced by the run workers.
    #[serde(default)]
    pub inventory_rest_enabled: bool,
}

fn default_status() -> String {
    "active".to_string()
}

fn default_language() -> String {
    "yaml".to_string()
}

fn default_object() -> Value {
    Value::Object(serde_json::Map::new())
}

/// Parses and validates the catalog manifest. Fail-loud: a malformed manifest
/// must never be applied to the database.
pub fn parse_manifest(raw: &str) -> Result<CatalogManifest, AppError> {
    let manifest: CatalogManifest = serde_yml::from_str(raw).map_err(|error| {
        AppError::Validation(format!(
            "Official catalog manifest is not valid YAML: {error}"
        ))
    })?;
    if manifest.api_version != OFFICIAL_CATALOG_API_VERSION {
        return Err(AppError::Validation(format!(
            "Official catalog manifest apiVersion `{}` is not supported (expected `{}`)",
            manifest.api_version, OFFICIAL_CATALOG_API_VERSION
        )));
    }
    if manifest.playbooks.is_empty() {
        return Err(AppError::Validation(
            "Official catalog manifest declares no playbooks".to_string(),
        ));
    }
    let mut seen_slugs = std::collections::HashSet::new();
    for entry in &manifest.playbooks {
        validate_entry(entry)?;
        if !seen_slugs.insert(entry.slug.clone()) {
            return Err(AppError::Validation(format!(
                "Official catalog manifest declares duplicate slug `{}`",
                entry.slug
            )));
        }
    }
    Ok(manifest)
}

fn validate_entry(entry: &ManifestPlaybook) -> Result<(), AppError> {
    let slug = entry.slug.trim();
    if slug.is_empty() {
        return Err(AppError::Validation(
            "Official catalog playbook slug must not be empty".to_string(),
        ));
    }
    if !is_valid_slug(slug) {
        return Err(AppError::Validation(format!(
            "Official catalog playbook slug `{slug}` must match ^[a-z0-9][a-z0-9_-]{{1,126}}[a-z0-9]$"
        )));
    }
    if entry.runtime != "ansible" {
        return Err(AppError::Validation(format!(
            "Official catalog playbook `{slug}` runtime `{}` is not supported (only `ansible`)",
            entry.runtime
        )));
    }
    if !ALLOWED_RISKS.contains(&entry.risk.as_str()) {
        return Err(AppError::Validation(format!(
            "Official catalog playbook `{slug}` risk `{}` must be one of {ALLOWED_RISKS:?}",
            entry.risk
        )));
    }
    if !ALLOWED_CONNECTION_DEFAULTS.contains(&entry.connection_default.as_str()) {
        return Err(AppError::Validation(format!(
            "Official catalog playbook `{slug}` connection_default `{}` must be one of {ALLOWED_CONNECTION_DEFAULTS:?}",
            entry.connection_default
        )));
    }
    if !ALLOWED_STATUSES.contains(&entry.status.as_str()) {
        return Err(AppError::Validation(format!(
            "Official catalog playbook `{slug}` status `{}` must be one of {ALLOWED_STATUSES:?}",
            entry.status
        )));
    }
    if entry.timeout_seconds < 1 || entry.timeout_seconds > MAX_TIMEOUT_SECONDS {
        return Err(AppError::Validation(format!(
            "Official catalog playbook `{slug}` timeout_seconds {} must be within 1..={MAX_TIMEOUT_SECONDS}",
            entry.timeout_seconds
        )));
    }
    validate_project_path(&entry.entrypoint_file, "official catalog entrypoint")?;
    match (&entry.vars_schema_file, &entry.vars_schema) {
        (Some(path), None) => {
            validate_project_path(path, "official catalog vars_schema")?;
        }
        (None, Some(inline)) => {
            if !inline.is_object() {
                return Err(AppError::Validation(format!(
                    "Official catalog playbook `{slug}` inline vars_schema must be a JSON object"
                )));
            }
        }
        _ => {
            return Err(AppError::Validation(format!(
                "Official catalog playbook `{slug}` must declare exactly one of vars_schema_file or vars_schema"
            )));
        }
    }
    if !entry.launch_options.is_object() {
        return Err(AppError::Validation(format!(
            "Official catalog playbook `{slug}` launch_options must be a JSON object"
        )));
    }
    if !entry.token_policy.is_object() {
        return Err(AppError::Validation(format!(
            "Official catalog playbook `{slug}` token_policy must be a JSON object"
        )));
    }
    Ok(())
}

fn is_valid_slug(slug: &str) -> bool {
    let bytes = slug.as_bytes();
    if bytes.len() < 3 || bytes.len() > 128 {
        return false;
    }
    let edge_ok = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    let inner_ok = |byte: u8| edge_ok(byte) || byte == b'-' || byte == b'_';
    edge_ok(bytes[0])
        && edge_ok(bytes[bytes.len() - 1])
        && bytes[1..bytes.len() - 1].iter().copied().all(inner_ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_manifest(playbook_overrides: &str) -> String {
        format!(
            r#"apiVersion: piping.space/playbook-catalog/v1
catalog:
  source_repo: https://github.com/mlimarenko/piping
  base_path: ansible/playbooks
playbooks:
  - slug: system-auto-update-inventory
    name: Auto-update inventory
    description: Test entry
    category: inventory
    risk: low
    runtime: ansible
    connection_default: ssh
    entrypoint_file: system/auto-update-inventory.yml
    timeout_seconds: 90
    output_schema:
      type: object
    token_policy:
      ttl_seconds: 3600
      scopes: []
    launch_options:
      forks: 64
    vars_schema_file: system/auto-update-inventory.vars-schema.json
    system: true
    inventory_rest_enabled: true
{playbook_overrides}"#
        )
    }

    #[test]
    fn parses_minimal_manifest_with_defaults() {
        let manifest = parse_manifest(&minimal_manifest("")).expect("manifest must parse");
        assert_eq!(manifest.api_version, OFFICIAL_CATALOG_API_VERSION);
        let entry = &manifest.playbooks[0];
        assert_eq!(entry.slug, "system-auto-update-inventory");
        assert_eq!(entry.status, "active");
        assert_eq!(entry.language, "yaml");
        assert_eq!(entry.input_schema, serde_json::json!({}));
        assert!(entry.system);
        assert!(entry.inventory_rest_enabled);
        assert!(entry.id.is_none());
    }

    #[test]
    fn rejects_unsupported_api_version() {
        let raw = minimal_manifest("").replace(
            "piping.space/playbook-catalog/v1",
            "piping.space/playbook-catalog/v999",
        );
        let error = parse_manifest(&raw).expect_err("must reject unknown apiVersion");
        assert!(error.to_string().contains("apiVersion"));
    }

    #[test]
    fn rejects_non_ansible_runtime() {
        let raw = minimal_manifest("").replace("runtime: ansible", "runtime: shell");
        let error = parse_manifest(&raw).expect_err("must reject non-ansible runtime");
        assert!(error.to_string().contains("runtime"));
    }

    #[test]
    fn rejects_entrypoint_traversal() {
        let raw = minimal_manifest("").replace(
            "entrypoint_file: system/auto-update-inventory.yml",
            "entrypoint_file: ../../../etc/passwd",
        );
        let error = parse_manifest(&raw).expect_err("must reject parent traversal");
        assert!(error.to_string().contains("traversal"));
    }

    #[test]
    fn rejects_absolute_entrypoint() {
        let raw = minimal_manifest("").replace(
            "entrypoint_file: system/auto-update-inventory.yml",
            "entrypoint_file: /etc/passwd",
        );
        assert!(parse_manifest(&raw).is_err());
    }

    #[test]
    fn rejects_both_vars_schema_forms() {
        let raw = minimal_manifest("    vars_schema:\n      type: object\n");
        let error = parse_manifest(&raw).expect_err("must reject double vars_schema");
        assert!(error.to_string().contains("exactly one"));
    }

    #[test]
    fn rejects_missing_vars_schema() {
        let raw = minimal_manifest("").replace(
            "    vars_schema_file: system/auto-update-inventory.vars-schema.json\n",
            "",
        );
        let error = parse_manifest(&raw).expect_err("must reject missing vars_schema");
        assert!(error.to_string().contains("exactly one"));
    }

    #[test]
    fn rejects_out_of_range_timeout() {
        let raw = minimal_manifest("").replace("timeout_seconds: 90", "timeout_seconds: 0");
        assert!(parse_manifest(&raw).is_err());
        let raw = minimal_manifest("").replace("timeout_seconds: 90", "timeout_seconds: 100000");
        assert!(parse_manifest(&raw).is_err());
    }

    #[test]
    fn rejects_duplicate_slugs() {
        let manifest_body = minimal_manifest("");
        let (head, entry) = manifest_body.split_once("  - slug:").expect("entry start");
        let raw = format!("{head}  - slug:{entry}  - slug:{entry}");
        let error = parse_manifest(&raw).expect_err("must reject duplicate slug");
        assert!(error.to_string().contains("duplicate"));
    }

    #[test]
    fn slug_charset_is_enforced() {
        assert!(is_valid_slug("system-auto-update-inventory"));
        assert!(is_valid_slug("abc"));
        assert!(!is_valid_slug("ab"));
        assert!(!is_valid_slug("-abc"));
        assert!(!is_valid_slug("abc-"));
        assert!(!is_valid_slug("ABC"));
        assert!(!is_valid_slug("a b"));
    }
}
