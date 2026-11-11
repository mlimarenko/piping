//! Product persistence checks against a database migrated through the project schema.

mod common;

use piping_backend::contexts::playbooks::domain::{
    ProjectAuth, ProjectEntry, ProjectEntryContent, ProjectSource, RevisionChange,
};
use piping_backend::contexts::playbooks::infrastructure::persistence::{
    CreateProjectRecord, PlaybookRepository, SqlxPlaybookRepository,
};
use piping_backend::errors::AppError;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

fn file(path: &str, bytes: &[u8], executable: bool) -> ProjectEntry {
    ProjectEntry {
        path: path.to_string(),
        content: ProjectEntryContent::File {
            bytes: bytes.to_vec(),
            executable,
        },
    }
}

fn create_record(source: ProjectSource) -> CreateProjectRecord {
    CreateProjectRecord {
        name: format!("workspace-proof-{}", Uuid::now_v7()),
        description: None,
        source,
        user_id: None,
    }
}

#[tokio::test]
#[ignore = "requires an isolated migrated PostgreSQL fixture"]
async fn changing_git_reference_invalidates_current_but_preserves_fixed_revision() -> TestResult {
    let pool = common::build_test_db_pool().await?;
    let repo = SqlxPlaybookRepository::new(pool);
    let source = |reference: &str| ProjectSource::Git {
        url: "https://example.invalid/project.git".to_string(),
        reference: reference.to_string(),
        auth: ProjectAuth::Anonymous,
    };
    let project = repo.create_project(create_record(source("main"))).await?;
    let revision = repo
        .import_project_revision(
            project.id,
            None,
            project.updated_at,
            "a".repeat(40),
            vec![file("site.yml", b"---\n", false)],
            None,
        )
        .await?;
    let unchanged = repo
        .update_project(project.id, create_record(source("main")))
        .await?
        .ok_or("project missing")?;
    assert_eq!(unchanged.current_revision_id, Some(revision.revision_id));
    let changed = repo
        .update_project(project.id, create_record(source("other")))
        .await?
        .ok_or("project missing")?;
    assert_eq!(changed.current_revision_id, None);
    assert!(
        repo.get_project_revision(project.id, revision.revision_id)
            .await?
            .is_some()
    );
    assert!(repo.delete_project(project.id).await?);
    Ok(())
}

#[tokio::test]
#[ignore = "requires an isolated migrated PostgreSQL fixture"]
async fn revisions_preserve_binary_modes_and_reject_stale_or_partial_edits() -> TestResult {
    let pool = common::build_test_db_pool().await?;
    let repo = SqlxPlaybookRepository::new(pool.clone());
    let project = repo
        .create_project(create_record(ProjectSource::Editable))
        .await?;
    let base = project
        .current_revision_id
        .ok_or("editable project has no initial revision")?;
    let binary = file("files/blob", &[0, 255, 128, 10], true);
    let config = file(
        "ansible.cfg",
        b"[defaults]\nstdout_callback=default\n",
        false,
    );
    let link = ProjectEntry {
        path: "roles/blob".to_string(),
        content: ProjectEntryContent::Symlink {
            target: "../files/blob".to_string(),
        },
    };
    let first = repo
        .change_project_revision(
            project.id,
            base,
            vec![binary.clone(), config.clone(), link.clone()]
                .into_iter()
                .map(|entry| RevisionChange::Put { entry })
                .collect(),
            None,
        )
        .await?;
    assert!(first.entries.contains(&binary));
    assert!(first.entries.contains(&config));
    assert!(first.entries.contains(&link));
    assert!(
        repo.get_project_revision(project.id, base)
            .await?
            .ok_or("base missing")?
            .entries
            .is_empty()
    );

    let stale = repo
        .change_project_revision(
            project.id,
            base,
            vec![RevisionChange::Delete {
                path: "files".to_string(),
            }],
            None,
        )
        .await;
    assert!(matches!(stale, Err(AppError::Conflict(_))));
    let rejected = repo
        .change_project_revision(
            project.id,
            first.revision_id,
            vec![
                RevisionChange::Put {
                    entry: file("would-be-partial", b"must not persist", false),
                },
                RevisionChange::Put {
                    entry: file("../escape", b"bad", false),
                },
            ],
            None,
        )
        .await;
    assert!(matches!(rejected, Err(AppError::Validation(_))));
    assert_eq!(
        repo.get_project(project.id)
            .await?
            .ok_or("project missing")?
            .current_revision_id,
        Some(first.revision_id)
    );
    let reread = repo
        .get_project_revision(project.id, first.revision_id)
        .await?
        .ok_or("revision missing")?;
    assert_eq!(reread.tree_sha256, first.tree_sha256);
    assert_eq!(reread.entries, first.entries);
    let direct_mutation =
        sqlx::query("UPDATE playbook_project_revision SET entries='[]' WHERE revision_id=$1")
            .bind(first.revision_id)
            .execute(&pool)
            .await;
    assert!(direct_mutation.is_err());
    assert!(repo.delete_project(project.id).await?);
    Ok(())
}

#[tokio::test]
#[ignore = "requires an isolated migrated PostgreSQL fixture"]
async fn concurrent_revision_writers_have_one_winner() -> TestResult {
    let pool = common::build_test_db_pool().await?;
    let repo = SqlxPlaybookRepository::new(pool);
    let project = repo
        .create_project(create_record(ProjectSource::Editable))
        .await?;
    let base = project
        .current_revision_id
        .ok_or("initial revision missing")?;
    let (left, right) = tokio::join!(
        repo.change_project_revision(
            project.id,
            base,
            vec![RevisionChange::Put {
                entry: file("left", b"left", false)
            }],
            None
        ),
        repo.change_project_revision(
            project.id,
            base,
            vec![RevisionChange::Put {
                entry: file("right", b"right", false)
            }],
            None
        ),
    );
    let winner = match (left, right) {
        (Ok(revision), Err(AppError::Conflict(_))) | (Err(AppError::Conflict(_)), Ok(revision)) => {
            revision
        }
        outcomes => {
            return Err(
                format!("expected one successful edit and one conflict: {outcomes:?}").into(),
            );
        }
    };
    assert_eq!(winner.entries.len(), 1);
    assert_eq!(
        repo.get_project(project.id)
            .await?
            .ok_or("project missing")?
            .current_revision_id,
        Some(winner.revision_id)
    );
    assert!(repo.delete_project(project.id).await?);
    Ok(())
}

#[tokio::test]
#[ignore = "requires an isolated migrated PostgreSQL fixture"]
async fn acquired_git_tree_is_read_only_and_fork_is_independently_editable() -> TestResult {
    let pool = common::build_test_db_pool().await?;
    let repo = SqlxPlaybookRepository::new(pool);
    let project = repo
        .create_project(create_record(ProjectSource::Git {
            url: "https://example.invalid/project.git".to_string(),
            reference: "main".to_string(),
            auth: ProjectAuth::Anonymous,
        }))
        .await?;
    assert!(project.current_revision_id.is_none());
    // Controlled acquisition result: this test performs no Git operation.
    let imported = repo
        .import_project_revision(
            project.id,
            None,
            project.updated_at,
            "a".repeat(40),
            vec![file("site.yml", b"---\n- hosts: all\n  tasks: []\n", false)],
            None,
        )
        .await?;
    let edit = repo
        .change_project_revision(
            project.id,
            imported.revision_id,
            vec![RevisionChange::Put {
                entry: file("forbidden", b"write", false),
            }],
            None,
        )
        .await;
    assert!(matches!(edit, Err(AppError::Conflict(_))));
    let fork = repo
        .fork_project(
            project.id,
            imported.revision_id,
            format!("fork-{}", Uuid::now_v7()),
            None,
        )
        .await?;
    assert!(matches!(fork.source, ProjectSource::Editable));
    let fork_base = fork.current_revision_id.ok_or("fork revision missing")?;
    let fork_revision = repo
        .get_project_revision(fork.id, fork_base)
        .await?
        .ok_or("fork snapshot missing")?;
    assert_eq!(fork_revision.tree_sha256, imported.tree_sha256);
    let changed = repo
        .change_project_revision(
            fork.id,
            fork_base,
            vec![RevisionChange::Put {
                entry: file("new", &[0, 255], true),
            }],
            None,
        )
        .await?;
    assert_ne!(changed.tree_sha256, imported.tree_sha256);
    assert_eq!(
        repo.get_project_revision(project.id, imported.revision_id)
            .await?
            .ok_or("source revision missing")?
            .entries,
        imported.entries
    );
    assert!(repo.delete_project(fork.id).await?);
    assert!(repo.delete_project(project.id).await?);
    Ok(())
}

#[tokio::test]
#[ignore = "requires an isolated migrated PostgreSQL fixture"]
async fn git_import_rejects_source_change_even_without_an_existing_revision() -> TestResult {
    let pool = common::build_test_db_pool().await?;
    let repo = SqlxPlaybookRepository::new(pool.clone());
    let project = repo
        .create_project(create_record(ProjectSource::Git {
            url: "https://example.invalid/first.git".into(),
            reference: "main".into(),
            auth: ProjectAuth::Anonymous,
        }))
        .await?;
    // Advance deterministically while both old/new sources have no revision.
    sqlx::query("UPDATE playbook_project SET source=jsonb_set(source,'{url}','\"https://example.invalid/second.git\"'), updated_at=updated_at+interval '1 second' WHERE id=$1")
        .bind(project.id).execute(&pool).await?;
    let stale = repo
        .import_project_revision(
            project.id,
            None,
            project.updated_at,
            "a".repeat(40),
            vec![file("site.yml", b"---\n", false)],
            None,
        )
        .await;
    assert!(matches!(stale, Err(AppError::Conflict(_))));
    assert!(
        repo.get_project(project.id)
            .await?
            .ok_or("project missing")?
            .current_revision_id
            .is_none()
    );
    assert!(repo.delete_project(project.id).await?);
    Ok(())
}
