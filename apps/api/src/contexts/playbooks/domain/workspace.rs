//! Canonical project, revision and launch definitions.
use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::errors::AppError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProjectSource {
    Editable,
    Git {
        url: String,
        reference: String,
        auth: ProjectAuth,
    },
    /// Built-in catalog project. Read-only through the public project, file and
    /// job-source mutation endpoints; only the startup catalog loader writes it.
    System {
        /// Stable catalog identity, for example `piping.space/playbook-catalog/v1`.
        key: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProjectAuth {
    Anonymous,
    HttpsBasic { username: String, secret_id: Uuid },
    HttpsBearer { secret_id: Uuid },
    SshRecord { ssh_record_id: Uuid },
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Project {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub source: ProjectSource,
    pub current_revision_id: Option<Uuid>,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProjectEntryContent {
    File { bytes: Vec<u8>, executable: bool },
    Symlink { target: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ProjectEntry {
    pub path: String,
    pub content: ProjectEntryContent,
}

/// Reject paths that cannot be safely materialized under a project root.
pub fn validate_project_entry(entry: &ProjectEntry) -> Result<(), AppError> {
    let invalid = || AppError::Validation("invalid project entry path or symlink".into());
    let path = &entry.path;
    if path.is_empty()
        || path.contains(['\\', '\0'])
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(invalid());
    }
    if let ProjectEntryContent::Symlink { target } = &entry.content {
        if target.is_empty() || target.starts_with('/') || target.contains(['\\', '\0']) {
            return Err(invalid());
        }
        let mut depth = path.split('/').count() - 1;
        for part in target.split('/') {
            match part {
                "" | "." => return Err(invalid()),
                ".." => depth = depth.checked_sub(1).ok_or_else(invalid)?,
                _ => depth += 1,
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ProjectRevision {
    pub revision_id: Uuid,
    pub project_id: Uuid,
    pub tree_sha256: String,
    pub source_commit_oid: Option<String>,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub entries: Vec<ProjectEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum RevisionChange {
    Put { entry: ProjectEntry },
    Delete { path: String },
    Rename { from: String, to: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RevisionSelection {
    Current,
    Fixed { revision_id: Uuid },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InventorySelection {
    Managed {
        target_set_id: Uuid,
    },
    Native {
        sources: Vec<String>,
        ssh_record_id: Option<Uuid>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct LaunchOptions {
    pub argv: Vec<String>,
    pub extra_vars: Value,
    pub limit: Option<String>,
    pub env: BTreeMap<String, String>,
    pub secret_refs: BTreeMap<String, Uuid>,
    pub timeout_seconds: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct JobTemplate {
    pub id: Uuid,
    pub name: String,
    pub project_id: Uuid,
    pub workdir: String,
    pub playbook_paths: Vec<String>,
    pub revision: RevisionSelection,
    pub inventory: InventorySelection,
    /// The single declared provenance of this template's launch parameters.
    #[serde(default)]
    pub vars_schema: super::parameters::VarsSchemaSource,
    pub options: LaunchOptions,
    pub execution_environment_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ResolvedTarget {
    pub hostname: String,
    pub target_id: Option<Uuid>,
    pub source_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct LaunchSpec {
    pub template_id: Uuid,
    pub project_id: Uuid,
    pub revision_id: Uuid,
    pub tree_sha256: String,
    pub execution_environment_id: Uuid,
    pub image: String,
    pub image_digest: String,
    pub workdir: String,
    pub playbook_paths: Vec<String>,
    pub inventory: InventorySelection,
    /// Effective parameter schema (document and canonical revision) captured
    /// for this launch; absent when the template declares no parameters. Values
    /// live in `options.extra_vars`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vars_schema: Option<super::parameters::ResolvedVarsSchema>,
    pub options: LaunchOptions,
    pub targets: Vec<ResolvedTarget>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScheduleTargets {
    Fixed {
        targets: Vec<ResolvedTarget>,
        /// Target set this fixed list was copied from, when it came from one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_set_id: Option<Uuid>,
    },
    Dynamic {
        selector: Value,
        /// Target set this selector was copied from, when it came from one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_set_id: Option<Uuid>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_executable_and_contained_links_survive_json() {
        let entries = vec![
            ProjectEntry {
                path: "bin/run".into(),
                content: ProjectEntryContent::File {
                    bytes: vec![0, 255, 42],
                    executable: true,
                },
            },
            ProjectEntry {
                path: "bin/link".into(),
                content: ProjectEntryContent::Symlink {
                    target: "../bin/run".into(),
                },
            },
        ];
        for entry in &entries {
            validate_project_entry(entry).unwrap();
        }
        let encoded = serde_json::to_vec(&entries).unwrap();
        let decoded: Vec<ProjectEntry> = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, entries);
    }

    #[test]
    fn project_paths_and_escaping_links_are_rejected() {
        for (path, target) in [
            ("../x", "x"),
            ("/x", "x"),
            ("a/link", "../../x"),
            ("link", "/etc/passwd"),
            ("a//b", "x"),
        ] {
            let entry = ProjectEntry {
                path: path.into(),
                content: ProjectEntryContent::Symlink {
                    target: target.into(),
                },
            };
            assert!(validate_project_entry(&entry).is_err());
        }
    }
}
