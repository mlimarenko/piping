//! Atomic, idempotent write of the builtin catalog.
//!
//! One transaction owns the single system project, one immutable revision per
//! catalog tree hash and the job definitions of the system entries. A repeated
//! seed with unchanged inputs writes nothing; a changed catalog appends a new
//! revision and never mutates a previous one. Instance settings on an existing
//! job template row (inventory selection, assignments, secret bindings, audit
//! authors) are never reset here.

use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::contexts::playbooks::domain::{
    InventorySelection, JobTemplate, LaunchOptions, ProjectSource, RevisionSelection,
    VarsSchemaSource,
};
use crate::errors::{AppError, classify_sqlx_error_owned};

use super::SeedSystemCatalogRecord;

pub(super) async fn seed_system_catalog(
    pool: &PgPool,
    record: SeedSystemCatalogRecord,
) -> Result<super::SeededSystemCatalog, AppError> {
    let mut tx = pool.begin().await.map_err(classify_sqlx_error_owned)?;
    // Serialize concurrent first starts instead of racing read-then-insert: the
    // loser waits, then observes the winner's committed rows and adopts them.
    // This is database serialization, not a retry loop masking conflicts.
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(advisory_lock_key(record.project_id))
        .execute(&mut *tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
    let mut outcome = super::SeededSystemCatalog::default();

    ensure_project_tx(&mut tx, &record, &mut outcome).await?;
    ensure_revision_tx(&mut tx, &record, &mut outcome).await?;
    ensure_templates_tx(&mut tx, &record, &mut outcome).await?;

    tx.commit().await.map_err(classify_sqlx_error_owned)?;
    Ok(outcome)
}

/// Stable 64-bit advisory-lock key of the catalog project identity.
fn advisory_lock_key(project_id: Uuid) -> i64 {
    let bytes = project_id.as_bytes();
    let mut head = [0_u8; 8];
    head.copy_from_slice(&bytes[..8]);
    i64::from_be_bytes(head)
}

/// The catalog project is identified by its deterministic id and nothing else.
/// An existing row with another source, or another project already using that
/// display name, fails closed: the loader never adopts a user project by name.
async fn ensure_project_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    record: &SeedSystemCatalogRecord,
    outcome: &mut super::SeededSystemCatalog,
) -> Result<(), AppError> {
    let existing = sqlx::query("SELECT name, source FROM playbook_project WHERE id = $1")
        .bind(record.project_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
    if let Some(row) = existing {
        let name: String = row.try_get("name").map_err(classify_sqlx_error_owned)?;
        let source: ProjectSource = serde_json::from_value(
            row.try_get("source").map_err(classify_sqlx_error_owned)?,
        )?;
        let matches = matches!(
            &source,
            ProjectSource::System { key } if key == &record.source_key
        ) && name == record.project_name;
        if !matches {
            return Err(AppError::Conflict(
                "builtin catalog identity collides with an existing playbook project".to_string(),
            ));
        }
        return Ok(());
    }
    let collision: Option<Uuid> = sqlx::query_scalar("SELECT id FROM playbook_project WHERE name = $1")
        .bind(&record.project_name)
        .fetch_optional(&mut **tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
    if collision.is_some() {
        return Err(AppError::Conflict(
            "builtin catalog name is already used by another playbook project".to_string(),
        ));
    }
    sqlx::query(
        "INSERT INTO playbook_project (id, name, description, source) VALUES ($1,$2,$3,$4)",
    )
    .bind(record.project_id)
    .bind(&record.project_name)
    .bind(&record.project_description)
    .bind(serde_json::to_value(ProjectSource::System {
        key: record.source_key.clone(),
    })?)
    .execute(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?;
    outcome.project_created = true;
    Ok(())
}

/// One immutable revision per catalog tree hash. The revision id is derived from
/// that hash, so a re-seed finds it instead of appending a duplicate, and a
/// changed catalog necessarily receives a new id.
async fn ensure_revision_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    record: &SeedSystemCatalogRecord,
    outcome: &mut super::SeededSystemCatalog,
) -> Result<(), AppError> {
    let by_tree: Option<Uuid> = sqlx::query_scalar(
        "SELECT revision_id FROM playbook_project_revision WHERE project_id = $1 AND tree_sha256 = $2",
    )
    .bind(record.project_id)
    .bind(&record.tree_sha256)
    .fetch_optional(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?;
    let by_revision: Option<String> = sqlx::query_scalar(
        "SELECT tree_sha256 FROM playbook_project_revision WHERE project_id = $1 AND revision_id = $2",
    )
    .bind(record.project_id)
    .bind(record.revision_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?;
    if by_tree.is_some_and(|revision_id| revision_id != record.revision_id)
        || by_revision
            .as_deref()
            .is_some_and(|tree_sha256| tree_sha256 != record.tree_sha256)
    {
        return Err(AppError::Conflict(
            "builtin catalog revision identity does not match its tree hash".to_string(),
        ));
    }
    if by_tree.is_none() {
        sqlx::query(
            "INSERT INTO playbook_project_revision (revision_id, project_id, tree_sha256, entries) VALUES ($1,$2,$3,$4)",
        )
        .bind(record.revision_id)
        .bind(record.project_id)
        .bind(&record.tree_sha256)
        .bind(serde_json::to_value(&record.entries)?)
        .execute(&mut **tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
        outcome.revision_created = true;
    }
    sqlx::query(
        "UPDATE playbook_project SET current_revision_id = $2, updated_at = now() WHERE id = $1 AND current_revision_id IS DISTINCT FROM $2",
    )
    .bind(record.project_id)
    .bind(record.revision_id)
    .execute(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?;
    Ok(())
}

struct ExistingTemplate {
    project_id: Uuid,
    name: String,
    workdir: String,
    playbook_paths: Vec<String>,
    revision_selection: RevisionSelection,
    inventory: InventorySelection,
    vars_schema: VarsSchemaSource,
    options: LaunchOptions,
    execution_environment_id: Uuid,
}

/// Definition fields are replaced from the manifest; instance settings stay as
/// the owner configured them. A template of another project can never be
/// adopted, even with a matching id.
async fn ensure_templates_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    record: &SeedSystemCatalogRecord,
    outcome: &mut super::SeededSystemCatalog,
) -> Result<(), AppError> {
    for template in &record.templates {
        let existing = sqlx::query(
            "SELECT project_id, name, workdir, playbook_paths, revision_selection, inventory, vars_schema, options, execution_environment_id FROM playbook_job_template WHERE id = $1",
        )
        .bind(template.id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(classify_sqlx_error_owned)?
        .map(existing_template_from_row)
        .transpose()?;
        match existing {
            None => {
                sqlx::query(
                    "INSERT INTO playbook_job_template (id,name,project_id,workdir,playbook_paths,revision_selection,inventory,vars_schema,options,execution_environment_id) VALUES ($1,$2,$3,'.',$4,$5,$6,$7,$8,$9)",
                )
                .bind(template.id)
                .bind(&template.definition.name)
                .bind(record.project_id)
                .bind(&template.definition.playbook_paths)
                .bind(serde_json::to_value(RevisionSelection::Current)?)
                // No instance targets are configured yet; the launch path and
                // the availability lookup say so instead of inventing a set.
                .bind(serde_json::to_value(InventorySelection::Native {
                    sources: Vec::new(),
                    ssh_record_id: None,
                })?)
                .bind(serde_json::to_value(&template.definition.vars_schema)?)
                .bind(serde_json::to_value(&template.definition.options)?)
                // The configured default is only the seed value of a new row.
                .bind(template.execution_environment_id.ok_or_else(|| {
                    AppError::Validation(
                        "builtin job needs a configured execution environment".to_string(),
                    )
                })?)
                .execute(&mut **tx)
                .await
                .map_err(classify_sqlx_error_owned)?;
                outcome.templates_created += 1;
            }
            Some(existing) => {
                if existing.project_id != record.project_id {
                    return Err(AppError::Conflict(
                        "builtin job template identity is owned by another project".to_string(),
                    ));
                }
                let persisted = JobTemplate {
                    id: template.id,
                    name: existing.name,
                    project_id: existing.project_id,
                    workdir: existing.workdir,
                    playbook_paths: existing.playbook_paths,
                    revision: existing.revision_selection,
                    inventory: existing.inventory,
                    vars_schema: existing.vars_schema,
                    options: existing.options,
                    execution_environment_id: existing.execution_environment_id,
                };
                if template.definition.matches_template(&persisted, record.project_id) {
                    continue;
                }
                // Instance-owned option fields (parameter overrides, runtime env,
                // credentials, timeout) and the execution environment are not in
                // the UPDATE: only the catalog definition moves.
                let mut options = persisted.options.clone();
                options.argv = template.definition.options.argv.clone();
                sqlx::query(
                    "UPDATE playbook_job_template SET name = $2, playbook_paths = $3, vars_schema = $4, options = $5, updated_at = now() WHERE id = $1",
                )
                .bind(template.id)
                .bind(&template.definition.name)
                .bind(&template.definition.playbook_paths)
                .bind(serde_json::to_value(&template.definition.vars_schema)?)
                .bind(serde_json::to_value(&options)?)
                .execute(&mut **tx)
                .await
                .map_err(classify_sqlx_error_owned)?;
                outcome.templates_updated += 1;
            }
        }
    }
    Ok(())
}

fn existing_template_from_row(row: sqlx::postgres::PgRow) -> Result<ExistingTemplate, AppError> {
    Ok(ExistingTemplate {
        project_id: row.try_get("project_id").map_err(classify_sqlx_error_owned)?,
        name: row.try_get("name").map_err(classify_sqlx_error_owned)?,
        workdir: row.try_get("workdir").map_err(classify_sqlx_error_owned)?,
        playbook_paths: row
            .try_get("playbook_paths")
            .map_err(classify_sqlx_error_owned)?,
        revision_selection: serde_json::from_value(
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
