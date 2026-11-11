//! Canonical shipped playbook catalog: manifest, identity and validation.
//!
//! The versioned contract is `apps/api/docs/playbook-catalog.md`. This module is
//! the single owner of builtin project, revision and job-template identity; the
//! manifest owns playbook content only. There is no second catalog.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::config::constants::PLAYBOOK_RUN_MAX_TIMEOUT_SECONDS;
use crate::errors::AppError;

use super::parameters::VarsSchemaSource;
use super::workspace::{JobTemplate, LaunchOptions};

pub const CATALOG_API_VERSION: &str = "piping.space/playbook-catalog/v1";
/// The document name that the deterministic project identity is derived from.
pub const CATALOG_PROJECT_NAME: &str = "https://piping.space/playbook-catalog/v1/project";
/// Prefix for the deterministic job-template identity of one catalog slug.
pub const CATALOG_JOB_NAME_PREFIX: &str = "https://piping.space/playbook-catalog/v1/job/";
/// Prefix for the deterministic immutable revision identity of a catalog tree.
pub const CATALOG_REVISION_NAME_PREFIX: &str = "https://piping.space/playbook-catalog/v1/revision/";
/// Display name of the single builtin catalog project row.
pub const CATALOG_PROJECT_DISPLAY_NAME: &str = "Built-in playbooks";
pub const CATALOG_PROJECT_DESCRIPTION: &str =
    "Read-only playbook project shipped with the product; edited only by the bundled catalog loader.";
/// Packaged catalog root inside the backend image. One documented default, no
/// per-directory fallback.
pub const CATALOG_DEFAULT_ROOT: &str = "/app/ansible/playbooks";
/// Manifest file name inside the catalog root.
pub const CATALOG_MANIFEST_FILE: &str = "piping-playbooks.yaml";
/// RFC 4122 URL namespace, the canonical namespace for product-owned names.
const CATALOG_NAMESPACE_BYTES: [u8; 16] = [
    0x6b, 0xa7, 0xb8, 0x11, 0x9d, 0xad, 0x11, 0xd1, 0x80, 0xb4, 0x00, 0xc0, 0x4f, 0xd4, 0x30, 0xc8,
];

const ALLOWED_TOKEN_SCOPES: &[&str] = &[
    "artifact:create",
    "event:append",
    "status:update",
    "inventory:read",
    "inventory:create",
    "inventory:update",
];

/// Stable catalog project id. It is derived only from the versioned name, never
/// from an instance hostname, URL, display name or random value.
pub fn project_id() -> Uuid {
    Uuid::new_v5(
        &Uuid::from_bytes(CATALOG_NAMESPACE_BYTES),
        CATALOG_PROJECT_NAME.as_bytes(),
    )
}

/// Stable job-template id for one catalog slug.
pub fn job_id(slug: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::from_bytes(CATALOG_NAMESPACE_BYTES),
        format!("{CATALOG_JOB_NAME_PREFIX}{slug}").as_bytes(),
    )
}

/// Stable identity of one immutable catalog revision, derived from the catalog
/// tree hash that the revision owner computes.
pub fn revision_id(tree_sha256: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::from_bytes(CATALOG_NAMESPACE_BYTES),
        format!("{CATALOG_REVISION_NAME_PREFIX}{tree_sha256}").as_bytes(),
    )
}

/// Stable non-system project id for a catalog entry that omits `id`.
pub fn catalog_entry_id(slug: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::from_bytes(CATALOG_NAMESPACE_BYTES),
        format!("{CATALOG_JOB_NAME_PREFIX}entry/{slug}").as_bytes(),
    )
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogManifest {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub catalog: CatalogProvenance,
    pub playbooks: Vec<CatalogEntry>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogProvenance {
    pub source_repo: Option<String>,
    pub base_path: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogEntry {
    pub slug: String,
    pub name: String,
    pub description: Option<String>,
    pub category: String,
    pub risk: String,
    pub runtime: String,
    pub connection_default: String,
    pub status: String,
    /// Non-system entries may pin an id; system entries must derive it.
    pub id: Option<Uuid>,
    pub entrypoint_file: String,
    pub language: String,
    pub timeout_seconds: i64,
    #[serde(default)]
    pub input_schema: Value,
    #[serde(default)]
    pub output_schema: Value,
    pub token_policy: Value,
    #[serde(default)]
    pub launch_options: CatalogLaunchOptions,
    pub vars_schema_file: Option<String>,
    #[serde(default)]
    pub system: bool,
    #[serde(default)]
    pub inventory_rest_enabled: bool,
}

/// Approved launch options of one catalog entry. The only supported vocabulary
/// is the canonical `piping_contracts::ansible_options` matrix, which the same
/// validator owns for API and runner launches; a declaration outside it fails
/// the whole catalog instead of being silently dropped.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogLaunchOptions {
    /// Ansible CLI flags, validated and normalized by the canonical owner.
    #[serde(default)]
    pub argv: Vec<String>,
}

/// One catalog job definition projected into the canonical job-template owner
/// types. It carries definition fields only; instance settings (targets,
/// execution environment, credentials) stay with the job template row.
#[derive(Debug, Clone, PartialEq)]
pub struct CatalogTemplateDefinition {
    pub name: String,
    pub playbook_paths: Vec<String>,
    pub vars_schema: VarsSchemaSource,
    pub options: LaunchOptions,
}

impl CatalogTemplateDefinition {
    /// Whether a persisted row still carries exactly this catalog definition.
    /// Only catalog-owned fields are compared; instance settings are ignored by
    /// design so a re-seed never reverts them.
    pub fn matches_template(&self, template: &JobTemplate, project_id: Uuid) -> bool {
        template.project_id == project_id
            && template.name == self.name
            && template.workdir == "."
            && template.playbook_paths == self.playbook_paths
            && template.vars_schema == self.vars_schema
            && template.options.argv == self.options.argv
    }
}

/// The catalog-owned field that a proposed change would alter.
///
/// This is the single definition/instance split shared by the startup seed and
/// the API guard. Catalog-owned: the project, name, workdir, playbook paths,
/// declared parameter schema and the approved launch arguments. Instance-owned
/// and therefore absent here: revision selection, the execution
/// environment, targets, credential bindings, parameter overrides, runtime
/// environment values and the run timeout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinDefinitionField {
    Project,
    Name,
    Workdir,
    PlaybookPaths,
    VarsSchema,
    LaunchArguments,
}

impl BuiltinDefinitionField {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Name => "name",
            Self::Workdir => "workdir",
            Self::PlaybookPaths => "playbook_paths",
            Self::VarsSchema => "vars_schema",
            Self::LaunchArguments => "launch arguments",
        }
    }
}

/// The first catalog-owned field changed between two versions of one builtin
/// job, or `None` when only instance settings differ.
pub fn builtin_definition_change(
    existing: &JobTemplate,
    proposed: &JobTemplate,
) -> Option<BuiltinDefinitionField> {
    if proposed.project_id != existing.project_id {
        return Some(BuiltinDefinitionField::Project);
    }
    if proposed.name != existing.name {
        return Some(BuiltinDefinitionField::Name);
    }
    if proposed.workdir != existing.workdir {
        return Some(BuiltinDefinitionField::Workdir);
    }
    if proposed.playbook_paths != existing.playbook_paths {
        return Some(BuiltinDefinitionField::PlaybookPaths);
    }
    if proposed.vars_schema != existing.vars_schema {
        return Some(BuiltinDefinitionField::VarsSchema);
    }
    if proposed.options.argv != existing.options.argv {
        return Some(BuiltinDefinitionField::LaunchArguments);
    }
    None
}

impl CatalogEntry {
    /// Identity of the entry: a system entry always derives the catalog id; a
    /// non-system entry may pin its own id, otherwise the slug derives one.
    pub fn stable_id(&self) -> Uuid {
        self.id.unwrap_or_else(|| catalog_entry_id(&self.slug))
    }

    /// Deterministic job-template identity of this catalog entry.
    pub fn template_id(&self) -> Uuid {
        job_id(&self.slug)
    }

    /// Canonical Ansible argv, normalized through the approved option matrix.
    pub fn launch_argv(&self) -> Result<Vec<String>, AppError> {
        let parsed = piping_contracts::ansible_options::parse_ansible_options(
            &self.launch_options.argv,
        )
        .map_err(|error| {
            AppError::Validation(format!(
                "catalog entry `{}` launch_options: {error}",
                self.slug
            ))
        })?;
        Ok(parsed.argv())
    }

    /// Run timeout in the job-template representation.
    pub fn launch_timeout_seconds(&self) -> Result<u32, AppError> {
        if self.timeout_seconds < 1
            || self.timeout_seconds > i64::from(PLAYBOOK_RUN_MAX_TIMEOUT_SECONDS)
        {
            return Err(AppError::Validation(format!(
                "catalog entry `{}` timeout_seconds is out of range",
                self.slug
            )));
        }
        u32::try_from(self.timeout_seconds).map_err(|_| {
            AppError::Validation(format!(
                "catalog entry `{}` timeout_seconds is out of range",
                self.slug
            ))
        })
    }

    /// Definition of the job template this entry owns.
    pub fn template_definition(&self) -> Result<CatalogTemplateDefinition, AppError> {
        Ok(CatalogTemplateDefinition {
            name: self.name.clone(),
            playbook_paths: vec![self.entrypoint_file.clone()],
            vars_schema: match self.vars_schema_file.as_deref() {
                Some(path) => VarsSchemaSource::ProjectFile {
                    path: path.to_string(),
                },
                None => VarsSchemaSource::None,
            },
            options: LaunchOptions {
                argv: self.launch_argv()?,
                extra_vars: Value::Object(serde_json::Map::new()),
                limit: None,
                env: BTreeMap::new(),
                secret_refs: BTreeMap::new(),
                timeout_seconds: self.launch_timeout_seconds()?,
            },
        })
    }
}

impl CatalogManifest {
    pub fn system_entries(&self) -> impl Iterator<Item = &CatalogEntry> {
        self.playbooks.iter().filter(|entry| entry.system)
    }
}

/// Validate the whole manifest before any mutation. Rejects an unsupported
/// version, duplicate slug, unsafe path, a system entry with a pinned id, an
/// unknown token scope and a malformed schema document.
pub fn validate_manifest(manifest: &CatalogManifest) -> Result<(), AppError> {
    if manifest.api_version != CATALOG_API_VERSION {
        return Err(AppError::Validation(format!(
            "unsupported playbook catalog apiVersion `{}`",
            manifest.api_version
        )));
    }
    validate_relative_path(&manifest.catalog.base_path, "catalog.base_path")?;
    if manifest.playbooks.is_empty() {
        return Err(AppError::Validation(
            "playbook catalog must contain at least one entry".to_string(),
        ));
    }

    let mut slugs = BTreeSet::new();
    let mut ids: BTreeMap<Uuid, String> = BTreeMap::new();
    for entry in &manifest.playbooks {
        validate_slug(&entry.slug)?;
        if !slugs.insert(entry.slug.as_str()) {
            return Err(AppError::Validation(format!(
                "duplicate playbook catalog slug `{}`",
                entry.slug
            )));
        }
        validate_relative_path(&entry.entrypoint_file, "entrypoint_file")?;
        if let Some(vars_schema_file) = entry.vars_schema_file.as_deref() {
            validate_relative_path(vars_schema_file, "vars_schema_file")?;
        }
        if entry.system && entry.id.is_some() {
            return Err(AppError::Validation(format!(
                "system catalog entry `{}` must not pin an id",
                entry.slug
            )));
        }
        if entry.language != "yaml" {
            return Err(AppError::Validation(format!(
                "catalog entry `{}` has unsupported language `{}`",
                entry.slug, entry.language
            )));
        }
        validate_schema_object(&entry.input_schema, "input_schema", &entry.slug)?;
        validate_schema_object(&entry.output_schema, "output_schema", &entry.slug)?;
        validate_launch_options(entry, &mut ids)?;
        validate_token_policy(&entry.token_policy, &entry.slug)?;
    }
    Ok(())
}

fn validate_slug(slug: &str) -> Result<(), AppError> {
    let valid = !slug.is_empty()
        && slug.len() <= 100
        && slug
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if valid {
        Ok(())
    } else {
        Err(AppError::Validation(format!(
            "invalid playbook catalog slug `{slug}`"
        )))
    }
}

fn validate_relative_path(value: &str, field: &str) -> Result<(), AppError> {
    let invalid = value.is_empty()
        || value.starts_with('/')
        || value.contains('\\')
        || value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..");
    if invalid {
        return Err(AppError::Validation(format!(
            "catalog {field} must be a safe relative path"
        )));
    }
    Ok(())
}

fn validate_schema_object(value: &Value, field: &str, slug: &str) -> Result<(), AppError> {
    if !value.is_object() {
        return Err(AppError::Validation(format!(
            "catalog entry `{slug}` {field} must be a JSON object"
        )));
    }
    Ok(())
}

fn validate_launch_options(
    entry: &CatalogEntry,
    seen_ids: &mut BTreeMap<Uuid, String>,
) -> Result<(), AppError> {
    entry.launch_argv()?;
    entry.launch_timeout_seconds()?;
    if let Some(previous) = seen_ids.insert(entry.stable_id(), entry.slug.clone()) {
        return Err(AppError::Validation(format!(
            "catalog entries `{previous}` and `{}` share one id",
            entry.slug
        )));
    }
    Ok(())
}

fn validate_token_policy(value: &Value, slug: &str) -> Result<(), AppError> {
    let Some(policy) = value.as_object() else {
        return Err(AppError::Validation(format!(
            "catalog entry `{slug}` token_policy must be a JSON object"
        )));
    };
    let Some(scopes) = policy.get("scopes").and_then(Value::as_array) else {
        return Err(AppError::Validation(format!(
            "catalog entry `{slug}` token_policy.scopes must be an array"
        )));
    };
    if scopes.len() > 16 {
        return Err(AppError::Validation(format!(
            "catalog entry `{slug}` declares too many token scopes"
        )));
    }
    let mut seen = BTreeSet::new();
    for scope in scopes {
        let Some(scope) = scope.as_str() else {
            return Err(AppError::Validation(format!(
                "catalog entry `{slug}` token scope must be a string"
            )));
        };
        if !ALLOWED_TOKEN_SCOPES.contains(&scope) {
            return Err(AppError::Validation(format!(
                "catalog entry `{slug}` declares unknown token scope `{scope}`"
            )));
        }
        if !seen.insert(scope) {
            return Err(AppError::Validation(format!(
                "catalog entry `{slug}` declares duplicate token scope `{scope}`"
            )));
        }
    }
    Ok(())
}

/// One builtin entry as resolved by the startup loader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedSystemTemplate {
    pub slug: String,
    /// Persisted job-template id; absent while no execution environment is
    /// configured, because the canonical job model requires one.
    pub template_id: Option<Uuid>,
}

/// Builtin catalog state produced by the startup loader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedSystemCatalog {
    pub project_id: Uuid,
    pub revision_id: Uuid,
    pub tree_sha256: String,
    pub templates: Vec<LoadedSystemTemplate>,
}

impl LoadedSystemCatalog {
    pub fn template(&self, slug: &str) -> Option<&LoadedSystemTemplate> {
        self.templates
            .iter()
            .find(|template| template.slug == slug)
    }
}

/// Application-state view of the builtin catalog. A failed load reports its
/// reason instead of leaving a partially seeded catalog.
#[derive(Debug, Clone)]
pub enum BuiltinCatalog {
    Loaded(LoadedSystemCatalog),
    Unavailable { reason: String },
}

impl BuiltinCatalog {
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self::Unavailable {
            reason: reason.into(),
        }
    }

    pub fn loaded(&self) -> Option<&LoadedSystemCatalog> {
        match self {
            Self::Loaded(catalog) => Some(catalog),
            Self::Unavailable { .. } => None,
        }
    }
}

/// Why a builtin job cannot be launched right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BuiltinUnavailableReason {
    CatalogNotLoaded,
    ExecutionEnvironmentNotConfigured,
    ExecutionEnvironmentUnavailable,
    TemplateNotSeeded,
    TargetsNotConfigured,
    RevisionUnavailable,
}

/// Canonical builtin-job lookup result: definition identity plus truthful
/// availability. An unavailable job keeps its reason instead of hiding behind a
/// generic error or a fabricated execution environment.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct BuiltinJobLookup {
    pub slug: String,
    pub template_id: Option<Uuid>,
    pub available: bool,
    pub unavailable_reason: Option<BuiltinUnavailableReason>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(slug: &str, system: bool) -> CatalogEntry {
        CatalogEntry {
            slug: slug.to_string(),
            name: slug.to_string(),
            description: None,
            category: "inventory".to_string(),
            risk: "low".to_string(),
            runtime: "ansible".to_string(),
            connection_default: "ssh".to_string(),
            status: "active".to_string(),
            id: None,
            entrypoint_file: format!("system/{slug}.yml"),
            language: "yaml".to_string(),
            timeout_seconds: 90,
            input_schema: json!({}),
            output_schema: json!({"type": "object"}),
            token_policy: json!({"ttl_seconds": 3600, "scopes": ["artifact:create"]}),
            launch_options: CatalogLaunchOptions::default(),
            vars_schema_file: Some(format!("system/{slug}.vars-schema.json")),
            system,
            inventory_rest_enabled: system,
        }
    }

    fn manifest(playbooks: Vec<CatalogEntry>) -> CatalogManifest {
        CatalogManifest {
            api_version: CATALOG_API_VERSION.to_string(),
            catalog: CatalogProvenance {
                source_repo: Some("https://github.com/mlimarenko/piping".to_string()),
                base_path: "ansible/playbooks".to_string(),
            },
            playbooks,
        }
    }

    #[test]
    fn identity_is_stable_and_product_owned() {
        assert_eq!(
            project_id(),
            Uuid::new_v5(
                &Uuid::from_bytes(CATALOG_NAMESPACE_BYTES),
                CATALOG_PROJECT_NAME.as_bytes()
            )
        );
        assert_ne!(project_id(), job_id("system-auto-update-inventory"));
        assert_eq!(
            job_id("system-auto-update-inventory"),
            job_id("system-auto-update-inventory")
        );
        assert_ne!(
            job_id("system-auto-update-inventory"),
            job_id("system-exporter-discovery")
        );
        assert_eq!(job_id("system-auto-update-inventory").get_version_num(), 5);
    }

    #[test]
    fn both_system_entries_validate_and_are_discovered() {
        let manifest = manifest(vec![
            entry("system-auto-update-inventory", true),
            entry("system-exporter-discovery", true),
        ]);
        validate_manifest(&manifest).expect("valid system catalog");
        assert_eq!(manifest.system_entries().count(), 2);
    }

    #[test]
    fn duplicate_slug_is_rejected() {
        let manifest = manifest(vec![entry("same", true), entry("same", true)]);
        assert!(validate_manifest(&manifest).is_err());
    }

    #[test]
    fn path_traversal_is_rejected() {
        let mut entry = entry("safe", true);
        entry.entrypoint_file = "../outside.yml".to_string();
        assert!(validate_manifest(&manifest(vec![entry])).is_err());
    }

    #[test]
    fn unsupported_version_is_rejected() {
        let mut manifest = manifest(vec![entry("safe", true)]);
        manifest.api_version = "piping.space/playbook-catalog/v2".to_string();
        assert!(validate_manifest(&manifest).is_err());
    }

    #[test]
    fn system_entry_with_pinned_id_is_rejected() {
        let mut entry = entry("safe", true);
        entry.id = Some(Uuid::now_v7());
        assert!(validate_manifest(&manifest(vec![entry])).is_err());
    }

    #[test]
    fn unknown_token_scope_is_rejected() {
        let mut entry = entry("safe", true);
        entry.token_policy = json!({"ttl_seconds": 60, "scopes": ["admin:all"]});
        assert!(validate_manifest(&manifest(vec![entry])).is_err());
    }

    #[test]
    fn approved_launch_argv_is_normalized_by_the_canonical_owner() {
        let mut entry = entry("safe", true);
        entry.launch_options.argv = vec![
            "-t".to_string(),
            "verify".to_string(),
            "--diff".to_string(),
        ];
        validate_manifest(&manifest(vec![entry.clone()])).expect("approved options");
        assert_eq!(entry.launch_argv().unwrap(), vec!["--tags=verify", "--diff"]);
        let definition = entry.template_definition().unwrap();
        assert_eq!(definition.options.argv, vec!["--tags=verify", "--diff"]);
        assert_eq!(definition.options.timeout_seconds, 90);
        assert_eq!(
            definition.vars_schema,
            VarsSchemaSource::ProjectFile {
                path: "system/safe.vars-schema.json".to_string()
            }
        );
    }

    #[test]
    fn declaration_outside_the_canonical_option_matrix_is_rejected() {
        for arguments in [
            vec!["--forks=64".to_string()],
            vec!["--ssh-common-args=-o ProxyCommand=x".to_string()],
            vec!["-e".to_string(), "x=1".to_string()],
            vec!["--extra-vars=x=1".to_string()],
        ] {
            let mut entry = entry("safe", true);
            entry.launch_options.argv = arguments;
            assert!(
                validate_manifest(&manifest(vec![entry])).is_err(),
                "unapproved launch declaration must fail the catalog"
            );
        }
    }

    #[test]
    fn unimplemented_launch_vocabulary_does_not_parse() {
        let document = r#"
apiVersion: piping.space/playbook-catalog/v1
catalog:
  source_repo: null
  base_path: ansible/playbooks
playbooks:
  - slug: safe
    name: Safe
    description: null
    category: inventory
    risk: low
    runtime: ansible
    connection_default: ssh
    status: active
    id: null
    entrypoint_file: system/safe.yml
    language: yaml
    timeout_seconds: 90
    input_schema: {}
    output_schema: {type: object}
    token_policy: {ttl_seconds: 60, scopes: [artifact:create]}
    launch_options:
      forks: 64
      strategy: free
      shell_executable: /bin/sh
      ssh_common_args: -o ProxyCommand=x
    vars_schema_file: null
    system: true
    inventory_rest_enabled: false
"#;
        assert!(serde_yml::from_str::<CatalogManifest>(document).is_err());
    }

    #[test]
    fn out_of_range_timeout_is_rejected() {
        for timeout_seconds in [0_i64, i64::from(PLAYBOOK_RUN_MAX_TIMEOUT_SECONDS) + 1] {
            let mut entry = entry("safe", true);
            entry.timeout_seconds = timeout_seconds;
            assert!(validate_manifest(&manifest(vec![entry])).is_err());
        }
    }

    #[test]
    fn shipped_catalog_matches_the_loader_contract() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../ansible/playbooks")
            .join(CATALOG_MANIFEST_FILE);
        let document = std::fs::read_to_string(&root).expect("shipped catalog manifest");
        let manifest: CatalogManifest =
            serde_yml::from_str(&document).expect("shipped catalog parses");
        validate_manifest(&manifest).expect("shipped catalog satisfies the contract");
        assert_eq!(manifest.system_entries().count(), 2);
        for entry in manifest.system_entries() {
            entry.template_definition().expect("system definition");
        }
    }
}
