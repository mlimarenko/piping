//! Conversion of a pinned revision into the runner's immutable snapshot.

use piping_contracts::playbook_runner::{
    ProjectSnapshot, SnapshotContent, SnapshotEntry, snapshot_tree_sha256,
};

use crate::contexts::playbooks::domain::{LaunchSpec, ProjectEntryContent, ProjectRevision};
use crate::errors::AppError;

/// Refuse revisions other than the one captured in the stored launch.
///
/// The digest is recomputed from the loaded entries with the same shared
/// function the runner uses, so a revision row whose stored digest drifted from
/// its content cannot be dispatched under the pinned identity.
pub(crate) fn snapshot_for_launch(
    launch: &LaunchSpec,
    revision: ProjectRevision,
) -> Result<ProjectSnapshot, AppError> {
    if revision.project_id != launch.project_id
        || revision.revision_id != launch.revision_id
        || revision.tree_sha256 != launch.tree_sha256
    {
        return Err(AppError::Validation(
            "Stored launch revision identity does not match the project snapshot".to_string(),
        ));
    }
    let entries = revision
        .entries
        .into_iter()
        .map(|entry| SnapshotEntry {
            path: entry.path,
            content: match entry.content {
                ProjectEntryContent::File { bytes, executable } => {
                    SnapshotContent::File { bytes, executable }
                }
                ProjectEntryContent::Symlink { target } => SnapshotContent::Symlink { target },
            },
        })
        .collect::<Vec<_>>();
    let tree_sha256 = snapshot_tree_sha256(&entries)
        .map_err(|error| AppError::Validation(format!("Invalid project snapshot: {error}")))?;
    if tree_sha256 != launch.tree_sha256 {
        return Err(AppError::Validation(
            "Stored launch digest does not match the recomputed project snapshot".to_string(),
        ));
    }
    Ok(ProjectSnapshot {
        project_id: revision.project_id,
        revision_id: revision.revision_id,
        tree_sha256,
        source_commit_oid: revision.source_commit_oid,
        entries,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::Utc;
    use uuid::Uuid;

    use super::*;
    use crate::contexts::playbooks::domain::{InventorySelection, LaunchOptions, ProjectEntry};

    #[test]
    fn snapshot_uses_only_the_pinned_revision_and_preserves_bytes() {
        let project_id = Uuid::from_u128(1);
        let revision_id = Uuid::from_u128(2);
        let entries = vec![ProjectEntry {
            path: "site.yml".into(),
            content: ProjectEntryContent::File {
                bytes: vec![0, 255],
                executable: false,
            },
        }];
        let tree_sha256 = snapshot_tree_sha256(&[SnapshotEntry {
            path: "site.yml".into(),
            content: SnapshotContent::File {
                bytes: vec![0, 255],
                executable: false,
            },
        }])
        .unwrap();
        let launch = LaunchSpec {
            template_id: Uuid::from_u128(3),
            project_id,
            revision_id,
            tree_sha256: tree_sha256.clone(),
            execution_environment_id: Uuid::from_u128(4),
            image: "registry.example/ansible".into(),
            image_digest: "sha256:digest".into(),
            workdir: ".".into(),
            playbook_paths: vec!["site.yml".into()],
            inventory: InventorySelection::Native {
                sources: vec!["inventory.yml".into()],
                ssh_record_id: None,
            },
            vars_schema: None,
            options: LaunchOptions {
                argv: vec![],
                extra_vars: serde_json::json!({}),
                limit: None,
                env: BTreeMap::new(),
                secret_refs: BTreeMap::new(),
                timeout_seconds: 60,
            },
            targets: vec![],
        };
        let revision = ProjectRevision {
            project_id,
            revision_id,
            tree_sha256: tree_sha256.clone(),
            source_commit_oid: None,
            created_by: None,
            created_at: Utc::now(),
            entries,
        };
        let snapshot = snapshot_for_launch(&launch, revision.clone()).unwrap();
        assert_eq!(snapshot.tree_sha256, tree_sha256);
        assert_eq!(
            snapshot.entries[0].content,
            SnapshotContent::File {
                bytes: vec![0, 255],
                executable: false,
            }
        );
        let mut changed = revision.clone();
        changed.tree_sha256 = "other-tree".into();
        assert!(snapshot_for_launch(&launch, changed).is_err());
        let mut drifted = revision;
        drifted.entries[0].content = ProjectEntryContent::File {
            bytes: vec![1, 2, 3],
            executable: false,
        };
        assert!(
            snapshot_for_launch(&launch, drifted).is_err(),
            "a revision whose content no longer hashes to the pinned digest must be refused"
        );
    }
}
