use std::collections::BTreeMap;

use crate::contexts::playbooks::domain::{
    InventorySelection, JobTemplate, Project, ProjectAuth, ProjectEntry, ProjectEntryContent,
    ProjectRevision, ProjectSource, RevisionChange,
};
use crate::contexts::secrets::application::JOB_TEMPLATE_RUNTIME_ENV_CONSUMER_TYPE;
use crate::errors::{AppError, classify_sqlx_error_owned};
use piping_contracts::playbook_runner::{
    SnapshotContent, SnapshotEntry, snapshot_tree_sha256, validate_snapshot_entries,
};
use sqlx::{PgPool, Row};
use uuid::Uuid;

fn snapshot(entries: &[ProjectEntry]) -> Vec<SnapshotEntry> {
    entries
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
        .collect()
}

fn tree_hash(entries: &[ProjectEntry]) -> Result<String, AppError> {
    let entries = snapshot(entries);
    validate_snapshot_entries(&entries).map_err(AppError::Validation)?;
    snapshot_tree_sha256(&entries).map_err(AppError::Validation)
}

pub(super) fn project_from_row(row: &sqlx::postgres::PgRow) -> Result<Project, AppError> {
    Ok(Project {
        id: row.try_get("id").map_err(classify_sqlx_error_owned)?,
        name: row.try_get("name").map_err(classify_sqlx_error_owned)?,
        description: row
            .try_get("description")
            .map_err(classify_sqlx_error_owned)?,
        source: serde_json::from_value(row.try_get("source").map_err(classify_sqlx_error_owned)?)?,
        current_revision_id: row
            .try_get("current_revision_id")
            .map_err(classify_sqlx_error_owned)?,
        created_by: row
            .try_get("created_by_user_id")
            .map_err(classify_sqlx_error_owned)?,
        created_at: row
            .try_get("created_at")
            .map_err(classify_sqlx_error_owned)?,
        updated_at: row
            .try_get("updated_at")
            .map_err(classify_sqlx_error_owned)?,
    })
}

pub(super) fn template_from_row(row: &sqlx::postgres::PgRow) -> Result<JobTemplate, AppError> {
    Ok(JobTemplate {
        id: row.try_get("id").map_err(classify_sqlx_error_owned)?,
        name: row.try_get("name").map_err(classify_sqlx_error_owned)?,
        project_id: row
            .try_get("project_id")
            .map_err(classify_sqlx_error_owned)?,
        workdir: row.try_get("workdir").map_err(classify_sqlx_error_owned)?,
        playbook_paths: row
            .try_get("playbook_paths")
            .map_err(classify_sqlx_error_owned)?,
        revision: serde_json::from_value(
            row.try_get("revision_selection")
                .map_err(classify_sqlx_error_owned)?,
        )?,
        inventory: serde_json::from_value(
            row.try_get("inventory")
                .map_err(classify_sqlx_error_owned)?,
        )?,
        vars_schema: serde_json::from_value(
            row.try_get("vars_schema")
                .map_err(classify_sqlx_error_owned)?,
        )?,
        options: serde_json::from_value(
            row.try_get("options").map_err(classify_sqlx_error_owned)?,
        )?,
        execution_environment_id: row
            .try_get("execution_environment_id")
            .map_err(classify_sqlx_error_owned)?,
    })
}

/// Hold the target-set reference lock for a managed inventory selection. The
/// lock lives in the same transaction as the template row write, so the
/// existence check cannot pass before a concurrent delete commits.
async fn lock_managed_reference_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    inventory: &InventorySelection,
) -> Result<(), AppError> {
    if let InventorySelection::Managed { target_set_id } = inventory {
        super::target_set_lock::lock_target_sets_for_reference_tx(tx, &[*target_set_id]).await?;
    }
    Ok(())
}

pub(super) async fn create_template(
    pool: &PgPool,
    record: super::CreateJobTemplateRecord,
) -> Result<JobTemplate, AppError> {
    let item = record.template;
    let mut tx = pool.begin().await.map_err(classify_sqlx_error_owned)?;
    lock_managed_reference_tx(&mut tx, &item.inventory).await?;
    let row = sqlx::query("INSERT INTO playbook_job_template (id,name,project_id,workdir,playbook_paths,revision_selection,inventory,vars_schema,options,execution_environment_id,created_by_user_id,updated_by_user_id) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$11) RETURNING *")
        .bind(item.id).bind(item.name).bind(item.project_id).bind(item.workdir).bind(item.playbook_paths)
        .bind(serde_json::to_value(item.revision)?).bind(serde_json::to_value(item.inventory)?).bind(serde_json::to_value(&item.vars_schema)?)
        .bind(serde_json::to_value(&item.options)?)
        .bind(item.execution_environment_id).bind(record.user_id).fetch_one(&mut *tx).await.map_err(classify_sqlx_error_owned)?;
    reconcile_template_secret_usage_tx(&mut tx, item.id, &item.options.secret_refs).await?;
    let template = template_from_row(&row)?;
    tx.commit().await.map_err(classify_sqlx_error_owned)?;
    Ok(template)
}

pub(super) async fn update_template(
    pool: &PgPool,
    id: Uuid,
    record: super::CreateJobTemplateRecord,
) -> Result<Option<JobTemplate>, AppError> {
    let item = record.template;
    let mut tx = pool.begin().await.map_err(classify_sqlx_error_owned)?;
    lock_managed_reference_tx(&mut tx, &item.inventory).await?;
    let row = sqlx::query("UPDATE playbook_job_template SET name=$2,project_id=$3,workdir=$4,playbook_paths=$5,revision_selection=$6,inventory=$7,vars_schema=$8,options=$9,execution_environment_id=$10,updated_by_user_id=$11,updated_at=now() WHERE id=$1 RETURNING *")
        .bind(id).bind(item.name).bind(item.project_id).bind(item.workdir).bind(item.playbook_paths)
        .bind(serde_json::to_value(item.revision)?).bind(serde_json::to_value(item.inventory)?).bind(serde_json::to_value(&item.vars_schema)?)
        .bind(serde_json::to_value(&item.options)?)
        .bind(item.execution_environment_id).bind(record.user_id).fetch_optional(&mut *tx).await.map_err(classify_sqlx_error_owned)?;
    let template = row.as_ref().map(template_from_row).transpose()?;
    if template.is_some() {
        reconcile_template_secret_usage_tx(&mut tx, id, &item.options.secret_refs).await?;
    }
    tx.commit().await.map_err(classify_sqlx_error_owned)?;
    Ok(template)
}

/// Project `options.secret_refs` into the shared usage-binding table. The
/// binding names the environment variable and the secret id; it never stores a
/// value. Dispatch checks this live projection before resolving secret material.
pub(super) async fn reconcile_template_secret_usage_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    template_id: Uuid,
    secret_refs: &BTreeMap<String, Uuid>,
) -> Result<(), AppError> {
    for (env_name, secret_id) in secret_refs {
        super::sqlx_impl::ensure_named_secret_tx(tx, *secret_id).await?;
        sqlx::query(
            r#"
            INSERT INTO secret_usage_binding
                (consumer_type, consumer_id, field_key,
                 owner_scope_type, owner_scope_id, secret_id)
            VALUES ($1, $2, $3, 'playbook_job_template', $2, $4)
            ON CONFLICT (consumer_type, consumer_id, field_key)
            DO UPDATE
            SET owner_scope_type = EXCLUDED.owner_scope_type,
                owner_scope_id = EXCLUDED.owner_scope_id,
                secret_id = EXCLUDED.secret_id,
                updated_at = now()
            "#,
        )
        .bind(JOB_TEMPLATE_RUNTIME_ENV_CONSUMER_TYPE)
        .bind(template_id)
        .bind(env_name)
        .bind(secret_id)
        .execute(&mut **tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
    }
    let names = secret_refs.keys().cloned().collect::<Vec<_>>();
    sqlx::query(
        r#"
        DELETE FROM secret_usage_binding
        WHERE consumer_type = $1
          AND consumer_id = $2
          AND NOT (field_key = ANY($3))
        "#,
    )
    .bind(JOB_TEMPLATE_RUNTIME_ENV_CONSUMER_TYPE)
    .bind(template_id)
    .bind(&names)
    .execute(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?;
    Ok(())
}

pub(super) async fn delete_template_secret_usage_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    template_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        DELETE FROM secret_usage_binding
        WHERE consumer_type = $1
          AND consumer_id = $2
        "#,
    )
    .bind(JOB_TEMPLATE_RUNTIME_ENV_CONSUMER_TYPE)
    .bind(template_id)
    .execute(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?;
    Ok(())
}

pub(super) async fn create_project(
    pool: &PgPool,
    record: super::CreateProjectRecord,
) -> Result<Project, AppError> {
    if matches!(record.source, ProjectSource::System { .. }) {
        return Err(AppError::Validation(
            "system project is managed by the bundled catalog loader".to_string(),
        ));
    }
    let mut tx = pool.begin().await.map_err(classify_sqlx_error_owned)?;
    let id = Uuid::now_v7();
    let editable = matches!(record.source, ProjectSource::Editable);
    sqlx::query("INSERT INTO playbook_project (id, name, description, source, created_by_user_id, updated_by_user_id) VALUES ($1,$2,$3,$4,$5,$5)")
        .bind(id).bind(&record.name).bind(&record.description).bind(serde_json::to_value(&record.source)?).bind(record.user_id).execute(&mut *tx).await.map_err(classify_sqlx_error_owned)?;
    super::sqlx_impl::reconcile_project_credential_binding_tx(
        &mut tx,
        id,
        project_secret(&record.source),
    )
    .await?;
    if editable {
        let revision_id = Uuid::now_v7();
        let entries: Vec<ProjectEntry> = Vec::new();
        let hash = tree_hash(&entries)?;
        sqlx::query("INSERT INTO playbook_project_revision (revision_id, project_id, tree_sha256, created_by, entries) VALUES ($1,$2,$3,$4,$5)")
            .bind(revision_id).bind(id).bind(hash).bind(record.user_id).bind(serde_json::to_value(entries)?).execute(&mut *tx).await.map_err(classify_sqlx_error_owned)?;
        sqlx::query("UPDATE playbook_project SET current_revision_id = $2 WHERE id = $1")
            .bind(id)
            .bind(revision_id)
            .execute(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
    }
    let row = sqlx::query("SELECT * FROM playbook_project WHERE id=$1")
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
    let project = project_from_row(&row)?;
    tx.commit().await.map_err(classify_sqlx_error_owned)?;
    Ok(project)
}

pub(super) fn project_secret(source: &ProjectSource) -> Option<Uuid> {
    match source {
        ProjectSource::Git {
            auth: ProjectAuth::HttpsBasic { secret_id, .. } | ProjectAuth::HttpsBearer { secret_id },
            ..
        } => Some(*secret_id),
        _ => None,
    }
}

fn apply(
    mut entries: BTreeMap<String, ProjectEntry>,
    changes: Vec<RevisionChange>,
) -> Result<Vec<ProjectEntry>, AppError> {
    for change in changes {
        match change {
            RevisionChange::Put { entry } => {
                entries.insert(entry.path.clone(), entry);
            }
            RevisionChange::Delete { path } => {
                let prefix = format!("{path}/");
                let before = entries.len();
                entries.retain(|key, _| key != &path && !key.starts_with(&prefix));
                if before == entries.len() {
                    return Err(AppError::Validation("path does not exist".into()));
                }
            }
            RevisionChange::Rename { from, to } => {
                let prefix = format!("{from}/");
                let moved: Vec<_> = entries
                    .iter()
                    .filter(|(key, _)| *key == &from || key.starts_with(&prefix))
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect();
                if moved.is_empty() || to == from || to.starts_with(&prefix) {
                    return Err(AppError::Validation("invalid rename".into()));
                }
                for (key, _) in &moved {
                    entries.remove(key);
                }
                let destination_prefix = format!("{to}/");
                if entries
                    .keys()
                    .any(|key| key == &to || key.starts_with(&destination_prefix))
                {
                    return Err(AppError::Conflict("rename destination exists".into()));
                }
                for (key, mut entry) in moved {
                    let dest = format!("{to}{}", &key[from.len()..]);
                    if entries.contains_key(&dest) {
                        return Err(AppError::Conflict("rename destination exists".into()));
                    }
                    entry.path = dest.clone();
                    entries.insert(dest, entry);
                }
            }
        }
    }
    let entries: Vec<_> = entries.into_values().collect();
    tree_hash(&entries)?;
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, bytes: &[u8]) -> ProjectEntry {
        ProjectEntry {
            path: path.into(),
            content: ProjectEntryContent::File {
                bytes: bytes.into(),
                executable: true,
            },
        }
    }

    #[test]
    fn renames_and_deletes_subtrees_without_losing_bytes_or_mode() {
        let entry = file("roles/a/run", &[0, 255]);
        let input = [(entry.path.clone(), entry)].into_iter().collect();
        let renamed = apply(
            input,
            vec![RevisionChange::Rename {
                from: "roles/a".into(),
                to: "roles/b".into(),
            }],
        )
        .unwrap();
        assert_eq!(renamed, vec![file("roles/b/run", &[0, 255])]);
        let input = renamed
            .into_iter()
            .map(|entry| (entry.path.clone(), entry))
            .collect();
        assert!(
            apply(
                input,
                vec![RevisionChange::Delete {
                    path: "roles/b".into()
                }]
            )
            .unwrap()
            .is_empty()
        );
    }

    #[test]
    fn rename_rejects_an_existing_destination_subtree() {
        let entries = [file("a/file", b"a"), file("b/other", b"b")];
        let input = entries
            .into_iter()
            .map(|entry| (entry.path.clone(), entry))
            .collect();
        assert!(matches!(
            apply(
                input,
                vec![RevisionChange::Rename {
                    from: "a".into(),
                    to: "b".into()
                }]
            ),
            Err(AppError::Conflict(_))
        ));
    }
}

pub(super) async fn get(
    pool: &PgPool,
    project_id: Uuid,
    revision_id: Uuid,
) -> Result<Option<ProjectRevision>, AppError> {
    let row = sqlx::query("SELECT revision_id, project_id, tree_sha256, source_commit_oid, created_by, created_at, entries FROM playbook_project_revision WHERE project_id = $1 AND revision_id = $2")
        .bind(project_id).bind(revision_id).fetch_optional(pool).await.map_err(classify_sqlx_error_owned)?;
    row.map(|row| {
        Ok(ProjectRevision {
            revision_id: row
                .try_get("revision_id")
                .map_err(classify_sqlx_error_owned)?,
            project_id: row
                .try_get("project_id")
                .map_err(classify_sqlx_error_owned)?,
            tree_sha256: row
                .try_get("tree_sha256")
                .map_err(classify_sqlx_error_owned)?,
            source_commit_oid: row
                .try_get("source_commit_oid")
                .map_err(classify_sqlx_error_owned)?,
            created_by: row
                .try_get("created_by")
                .map_err(classify_sqlx_error_owned)?,
            created_at: row
                .try_get("created_at")
                .map_err(classify_sqlx_error_owned)?,
            entries: serde_json::from_value(
                row.try_get("entries").map_err(classify_sqlx_error_owned)?,
            )?,
        })
    })
    .transpose()
}

pub(super) async fn change(
    pool: &PgPool,
    project_id: Uuid,
    base: Uuid,
    changes: Vec<RevisionChange>,
    created_by: Option<Uuid>,
) -> Result<ProjectRevision, AppError> {
    let mut tx = pool.begin().await.map_err(classify_sqlx_error_owned)?;
    let row = sqlx::query(
        "SELECT current_revision_id, source FROM playbook_project WHERE id = $1 FOR UPDATE",
    )
    .bind(project_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(classify_sqlx_error_owned)?
    .ok_or_else(|| AppError::Validation("project not found".into()))?;
    let current: Option<Uuid> = row
        .try_get("current_revision_id")
        .map_err(classify_sqlx_error_owned)?;
    if current != Some(base) {
        return Err(AppError::Conflict("stale base revision".into()));
    }
    let source: ProjectSource =
        serde_json::from_value(row.try_get("source").map_err(classify_sqlx_error_owned)?)?;
    if !matches!(source, ProjectSource::Editable) {
        return Err(AppError::Conflict("Git projects are read-only".into()));
    }
    let value: serde_json::Value = sqlx::query_scalar(
        "SELECT entries FROM playbook_project_revision WHERE project_id = $1 AND revision_id = $2",
    )
    .bind(project_id)
    .bind(base)
    .fetch_one(&mut *tx)
    .await
    .map_err(classify_sqlx_error_owned)?;
    let existing: Vec<ProjectEntry> = serde_json::from_value(value)?;
    let entries = apply(
        existing
            .into_iter()
            .map(|entry| (entry.path.clone(), entry))
            .collect(),
        changes,
    )?;
    let hash = tree_hash(&entries)?;
    let id = Uuid::now_v7();
    let created_at = sqlx::query_scalar("INSERT INTO playbook_project_revision (revision_id, project_id, tree_sha256, created_by, entries) VALUES ($1,$2,$3,$4,$5) RETURNING created_at")
        .bind(id).bind(project_id).bind(&hash).bind(created_by).bind(serde_json::to_value(&entries)?).fetch_one(&mut *tx).await.map_err(classify_sqlx_error_owned)?;
    sqlx::query(
        "UPDATE playbook_project SET current_revision_id = $2, updated_at = now() WHERE id = $1",
    )
    .bind(project_id)
    .bind(id)
    .execute(&mut *tx)
    .await
    .map_err(classify_sqlx_error_owned)?;
    tx.commit().await.map_err(classify_sqlx_error_owned)?;
    Ok(ProjectRevision {
        revision_id: id,
        project_id,
        tree_sha256: hash,
        source_commit_oid: None,
        created_by,
        created_at,
        entries,
    })
}

pub(super) async fn import(
    pool: &PgPool,
    project_id: Uuid,
    base: Option<Uuid>,
    expected_updated_at: chrono::DateTime<chrono::Utc>,
    source_commit_oid: String,
    mut entries: Vec<ProjectEntry>,
    created_by: Option<Uuid>,
) -> Result<ProjectRevision, AppError> {
    if !matches!(source_commit_oid.len(), 40 | 64)
        || !source_commit_oid
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(AppError::Validation("invalid Git commit OID".into()));
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    let hash = tree_hash(&entries)?;
    let mut tx = pool.begin().await.map_err(classify_sqlx_error_owned)?;
    let row = sqlx::query(
        "SELECT current_revision_id, updated_at, source FROM playbook_project WHERE id = $1 FOR UPDATE",
    )
    .bind(project_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(classify_sqlx_error_owned)?
    .ok_or_else(|| AppError::Validation("project not found".into()))?;
    let current: Option<Uuid> = row
        .try_get("current_revision_id")
        .map_err(classify_sqlx_error_owned)?;
    let stored_updated_at: chrono::DateTime<chrono::Utc> = row
        .try_get("updated_at")
        .map_err(classify_sqlx_error_owned)?;
    if current != base || stored_updated_at != expected_updated_at {
        return Err(AppError::Conflict(
            "stale project revision or metadata".into(),
        ));
    }
    let source: ProjectSource =
        serde_json::from_value(row.try_get("source").map_err(classify_sqlx_error_owned)?)?;
    if !matches!(source, ProjectSource::Git { .. }) {
        return Err(AppError::Conflict(
            "only Git projects can import revisions".into(),
        ));
    }
    let id = Uuid::now_v7();
    let created_at = sqlx::query_scalar("INSERT INTO playbook_project_revision (revision_id, project_id, tree_sha256, source_commit_oid, created_by, entries) VALUES ($1,$2,$3,$4,$5,$6) RETURNING created_at")
        .bind(id).bind(project_id).bind(&hash).bind(&source_commit_oid).bind(created_by).bind(serde_json::to_value(&entries)?).fetch_one(&mut *tx).await.map_err(classify_sqlx_error_owned)?;
    sqlx::query(
        "UPDATE playbook_project SET current_revision_id = $2, updated_at = now() WHERE id = $1",
    )
    .bind(project_id)
    .bind(id)
    .execute(&mut *tx)
    .await
    .map_err(classify_sqlx_error_owned)?;
    tx.commit().await.map_err(classify_sqlx_error_owned)?;
    Ok(ProjectRevision {
        revision_id: id,
        project_id,
        tree_sha256: hash,
        source_commit_oid: Some(source_commit_oid),
        created_by,
        created_at,
        entries,
    })
}

pub(super) async fn fork(
    pool: &PgPool,
    project_id: Uuid,
    revision_id: Uuid,
    name: String,
    created_by: Option<Uuid>,
) -> Result<Project, AppError> {
    let mut tx = pool.begin().await.map_err(classify_sqlx_error_owned)?;
    let row = sqlx::query("SELECT tree_sha256, entries FROM playbook_project_revision WHERE project_id = $1 AND revision_id = $2").bind(project_id).bind(revision_id).fetch_optional(&mut *tx).await.map_err(classify_sqlx_error_owned)?.ok_or_else(|| AppError::Validation("revision not found".into()))?;
    let entries: serde_json::Value = row.try_get("entries").map_err(classify_sqlx_error_owned)?;
    let hash: String = row
        .try_get("tree_sha256")
        .map_err(classify_sqlx_error_owned)?;
    let id = Uuid::now_v7();
    let new_revision = Uuid::now_v7();
    let source = ProjectSource::Editable;
    let row = sqlx::query("INSERT INTO playbook_project (id, name, source, created_by_user_id) VALUES ($1,$2,$3,$4) RETURNING created_at, updated_at")
        .bind(id).bind(&name).bind(serde_json::to_value(&source)?).bind(created_by).fetch_one(&mut *tx).await.map_err(classify_sqlx_error_owned)?;
    sqlx::query("INSERT INTO playbook_project_revision (revision_id, project_id, tree_sha256, created_by, entries) VALUES ($1,$2,$3,$4,$5)")
        .bind(new_revision).bind(id).bind(hash).bind(created_by).bind(entries).execute(&mut *tx).await.map_err(classify_sqlx_error_owned)?;
    sqlx::query("UPDATE playbook_project SET current_revision_id = $2 WHERE id = $1")
        .bind(id)
        .bind(new_revision)
        .execute(&mut *tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
    tx.commit().await.map_err(classify_sqlx_error_owned)?;
    Ok(Project {
        id,
        name,
        description: None,
        source,
        current_revision_id: Some(new_revision),
        created_by,
        created_at: row
            .try_get("created_at")
            .map_err(classify_sqlx_error_owned)?,
        updated_at: row
            .try_get("updated_at")
            .map_err(classify_sqlx_error_owned)?,
    })
}
