//! Startup loader for the bundled playbook catalog.
//!
//! The loader is the only writer of builtin project, revision and job-template
//! identity. It validates the whole document before any mutation, then asks the
//! repository for one atomic, idempotent upsert. A tree that changes appends a
//! new immutable revision; nothing here mutates an existing revision or resets
//! instance settings of an existing job template.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use piping_contracts::playbook_runner::{
    SnapshotContent, SnapshotEntry, snapshot_tree_sha256, validate_snapshot_entries,
};
use uuid::Uuid;

use crate::config::constants::{
    PLAYBOOK_CATALOG_MAX_FILE_BYTES, PLAYBOOK_CATALOG_MAX_FILES, PLAYBOOK_CATALOG_MAX_MANIFEST_BYTES,
    PLAYBOOK_CATALOG_MAX_TOTAL_BYTES,
};
use crate::contexts::playbooks::domain::{
    CATALOG_DEFAULT_ROOT, CATALOG_MANIFEST_FILE, CATALOG_PROJECT_DESCRIPTION,
    CATALOG_PROJECT_DISPLAY_NAME, CatalogManifest, LoadedSystemCatalog, LoadedSystemTemplate,
    ProjectEntry, ProjectEntryContent, VarsSchemaSource, project_id, revision_id,
    validate_manifest,
};
use crate::contexts::playbooks::infrastructure::persistence::{
    PlaybookRepository, SeedSystemCatalogRecord, SeedSystemTemplateRecord,
};
use crate::errors::AppError;

/// Root of the packaged catalog: the configured directory when set, otherwise
/// the single documented image path. There is no per-directory fallback.
pub fn catalog_root(configured: Option<&str>) -> PathBuf {
    PathBuf::from(configured.unwrap_or(CATALOG_DEFAULT_ROOT))
}

/// Validated catalog document plus the exact shipped file tree it refers to.
#[derive(Debug)]
pub struct CatalogDocument {
    pub manifest: CatalogManifest,
    pub entries: Vec<ProjectEntry>,
    pub tree_sha256: String,
}

/// Read the manifest, validate the whole document and every referenced file, and
/// compute the revision tree hash with the existing revision owner.
pub fn read_catalog(root: &Path) -> Result<CatalogDocument, AppError> {
    let manifest_path = root.join(CATALOG_MANIFEST_FILE);
    let manifest_size = std::fs::metadata(&manifest_path)
        .map_err(|error| {
            AppError::Validation(format!(
                "playbook catalog manifest {} is unreadable: {error}",
                manifest_path.display()
            ))
        })?
        .len();
    if manifest_size > PLAYBOOK_CATALOG_MAX_MANIFEST_BYTES {
        return Err(AppError::Validation(format!(
            "playbook catalog manifest {} exceeds {} bytes",
            manifest_path.display(),
            PLAYBOOK_CATALOG_MAX_MANIFEST_BYTES
        )));
    }
    let document = std::fs::read_to_string(&manifest_path).map_err(|error| {
        AppError::Validation(format!(
            "playbook catalog manifest {} is unreadable: {error}",
            manifest_path.display()
        ))
    })?;
    let manifest: CatalogManifest = serde_yml::from_str(&document).map_err(|error| {
        AppError::Validation(format!(
            "playbook catalog manifest {} is invalid: {error}",
            manifest_path.display()
        ))
    })?;
    validate_manifest(&manifest)?;
    let entries = read_catalog_tree(root)?;
    let files: BTreeMap<&str, &[u8]> = entries
        .iter()
        .filter_map(|entry| match &entry.content {
            ProjectEntryContent::File { bytes, .. } => Some((entry.path.as_str(), bytes.as_slice())),
            ProjectEntryContent::Symlink { .. } => None,
        })
        .collect();
    for entry in &manifest.playbooks {
        require_file(&files, &entry.entrypoint_file, &entry.slug)?;
        if let Some(vars_schema_file) = entry.vars_schema_file.as_deref() {
            require_file(&files, vars_schema_file, &entry.slug)?;
        }
    }
    // Compile every declared parameter schema with the E1 validator, so the
    // availability answer never claims a job whose schema cannot resolve.
    for entry in &manifest.playbooks {
        let Some(vars_schema_file) = entry.vars_schema_file.as_deref() else {
            continue;
        };
        crate::contexts::playbooks::domain::parameters::resolve_vars_schema(
            &VarsSchemaSource::ProjectFile {
                path: vars_schema_file.to_string(),
            },
            &entries,
        )
        .map_err(|error| {
            AppError::Validation(format!(
                "catalog entry `{}` vars_schema_file is invalid: {error}",
                entry.slug
            ))
        })?;
    }
    let tree_sha256 = tree_hash(&entries)?;
    Ok(CatalogDocument {
        manifest,
        entries,
        tree_sha256,
    })
}

fn require_file<'a>(
    files: &'a BTreeMap<&str, &'a [u8]>,
    path: &str,
    slug: &str,
) -> Result<&'a [u8], AppError> {
    files.get(path).copied().ok_or_else(|| {
        AppError::Validation(format!(
            "catalog entry `{slug}` references a missing file `{path}`"
        ))
    })
}

fn read_catalog_tree(root: &Path) -> Result<Vec<ProjectEntry>, AppError> {
    let mut files = BTreeSet::new();
    collect_files(root, root, &mut files)?;
    if files.len() > PLAYBOOK_CATALOG_MAX_FILES {
        return Err(AppError::Validation(format!(
            "playbook catalog contains more than {PLAYBOOK_CATALOG_MAX_FILES} files"
        )));
    }
    let mut entries = Vec::with_capacity(files.len());
    let mut total_bytes = 0_u64;
    for path in files {
        let file_path = root.join(&path);
        let size = std::fs::metadata(&file_path)
            .map_err(|error| {
                AppError::Validation(format!("catalog file `{path}` is unreadable: {error}"))
            })?
            .len();
        if size > PLAYBOOK_CATALOG_MAX_FILE_BYTES {
            return Err(AppError::Validation(format!(
                "catalog file `{path}` exceeds {PLAYBOOK_CATALOG_MAX_FILE_BYTES} bytes"
            )));
        }
        total_bytes = total_bytes.saturating_add(size);
        if total_bytes > PLAYBOOK_CATALOG_MAX_TOTAL_BYTES {
            return Err(AppError::Validation(format!(
                "playbook catalog exceeds {PLAYBOOK_CATALOG_MAX_TOTAL_BYTES} bytes"
            )));
        }
        let bytes = std::fs::read(&file_path).map_err(|error| {
            AppError::Validation(format!("catalog file `{path}` is unreadable: {error}"))
        })?;
        entries.push(ProjectEntry {
            path,
            content: ProjectEntryContent::File {
                bytes,
                // Shipped playbooks are data; the product never marks them
                // executable in the revision snapshot.
                executable: false,
            },
        });
    }
    Ok(entries)
}

fn collect_files(root: &Path, directory: &Path, files: &mut BTreeSet<String>) -> Result<(), AppError> {
    let listing = std::fs::read_dir(directory).map_err(|error| {
        AppError::Validation(format!(
            "playbook catalog directory {} is unreadable: {error}",
            directory.display()
        ))
    })?;
    for item in listing {
        let item = item.map_err(|error| {
            AppError::Validation(format!(
                "playbook catalog directory {} is unreadable: {error}",
                directory.display()
            ))
        })?;
        let path = item.path();
        let file_type = item.file_type().map_err(|error| {
            AppError::Validation(format!(
                "playbook catalog entry {} is unreadable: {error}",
                path.display()
            ))
        })?;
        if file_type.is_symlink() {
            return Err(AppError::Validation(format!(
                "playbook catalog must not contain symlinks: {}",
                path.display()
            )));
        }
        if file_type.is_dir() {
            collect_files(root, &path, files)?;
            continue;
        }
        let relative = path.strip_prefix(root).map_err(|_| {
            AppError::Validation(format!(
                "playbook catalog path escapes its root: {}",
                path.display()
            ))
        })?;
        let relative = relative.to_str().ok_or_else(|| {
            AppError::Validation(format!(
                "playbook catalog path is not valid UTF-8: {}",
                path.display()
            ))
        })?;
        files.insert(relative.replace('\\', "/"));
    }
    Ok(())
}

fn tree_hash(entries: &[ProjectEntry]) -> Result<String, AppError> {
    let snapshot: Vec<SnapshotEntry> = entries
        .iter()
        .map(|entry| SnapshotEntry {
            path: entry.path.clone(),
            content: match &entry.content {
                ProjectEntryContent::File { bytes, executable } => SnapshotContent::File {
                    bytes: bytes.clone(),
                    executable: *executable,
                },
                ProjectEntryContent::Symlink { target } => SnapshotContent::Symlink {
                    target: target.clone(),
                },
            },
        })
        .collect();
    validate_snapshot_entries(&snapshot).map_err(AppError::Validation)?;
    snapshot_tree_sha256(&snapshot).map_err(AppError::Validation)
}

/// Seed the builtin catalog and return its application-state view.
///
/// The project and its immutable revision are always written; job templates are
/// written only for an explicitly configured, present and enabled execution
/// environment, because the canonical job model requires one. Without it the
/// JSON/YAML definitions are still loadable and the lookup reports the exact
/// reason instead of fabricating an environment or a digest.
pub async fn load_system_catalog(
    root: &Path,
    execution_environment_id: Option<Uuid>,
    repository: &dyn PlaybookRepository,
) -> Result<LoadedSystemCatalog, AppError> {
    let document = read_catalog(root)?;
    let environment = match execution_environment_id {
        Some(id) => Some(id),
        None => None,
    };
    let seedable_environment = match environment {
        Some(id) => repository
            .get_execution_environment(id)
            .await?
            .filter(|environment| environment.enabled)
            .map(|environment| environment.id),
        None => None,
    };
    let system_entries: Vec<_> = document
        .manifest
        .system_entries()
        .cloned()
        .collect();
    let mut templates = Vec::with_capacity(system_entries.len());
    let mut seed_templates = Vec::new();
    for entry in &system_entries {
        // A new row needs the configured environment (the canonical job model
        // requires one); an existing row keeps its own instance-owned
        // environment and its definition is still refreshed without it.
        let existing_row = repository.get_job_template(entry.template_id()).await?.is_some();
        let seeded = if seedable_environment.is_some() || existing_row {
            seed_templates.push(SeedSystemTemplateRecord {
                id: entry.template_id(),
                definition: entry.template_definition()?,
                execution_environment_id: seedable_environment,
            });
            Some(entry.template_id())
        } else {
            None
        };
        templates.push(LoadedSystemTemplate {
            slug: entry.slug.clone(),
            template_id: seeded,
        });
    }
    let revision_id = revision_id(&document.tree_sha256);
    let project_id = project_id();
    repository
        .seed_system_catalog(SeedSystemCatalogRecord {
            source_key: crate::contexts::playbooks::domain::CATALOG_API_VERSION.to_string(),
            project_id,
            project_name: CATALOG_PROJECT_DISPLAY_NAME.to_string(),
            project_description: Some(CATALOG_PROJECT_DESCRIPTION.to_string()),
            revision_id,
            tree_sha256: document.tree_sha256.clone(),
            entries: document.entries,
            templates: seed_templates,
        })
        .await?;
    Ok(LoadedSystemCatalog {
        project_id,
        revision_id,
        tree_sha256: document.tree_sha256,
        templates,
    })
}

