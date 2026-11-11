use std::collections::{HashMap, HashSet};

use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::contexts::inventory::infrastructure::persistence::repositories::extras::tag_repository::validate_scope_token;
use crate::contexts::sync::domain::InventoryEntity;
use crate::contexts::sync::domain::inventory_preflight::{Capability, Finding, FindingCode};
use crate::errors::AppError;

use super::{ColumnValue, LocalInventoryBatch, PlannedRow, prepared_finding};

fn finding(
    batch: &LocalInventoryBatch,
    row: &PlannedRow,
    code: FindingCode,
    field: &str,
) -> Result<Finding, AppError> {
    let prepared = batch
        .rows
        .iter()
        .find(|candidate| {
            candidate.entity == row.entity
                && candidate.source_entity == row.source_entity
                && candidate.external_id == row.external_id
        })
        .ok_or_else(|| AppError::Internal("inventory_transfer:tag_scope_plan_mismatch".into()))?;
    Ok(prepared_finding(
        prepared,
        code,
        Some(field),
        Capability::TargetGraph,
    ))
}

// Only identifiers returned by the native subject registry are used in SQL.
fn quoted_subject(table: &str) -> Result<String, AppError> {
    if table.is_empty()
        || !table
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
    {
        return Err(AppError::Internal(
            "inventory_transfer:invalid_tag_subject".into(),
        ));
    }
    Ok(format!("\"{table}\""))
}

/// Assess the proposed final scope and memberships without changing the snapshot.
pub(super) async fn assess(
    batch: &LocalInventoryBatch,
    planned: &[PlannedRow],
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<Vec<Finding>, AppError> {
    let subjects: Vec<String> =
        sqlx::query_scalar("SELECT table_name FROM inventory_tag_subjects()")
            .fetch_all(&mut **transaction)
            .await?;
    for subject in &subjects {
        quoted_subject(subject)?;
    }
    let subject_set: HashSet<&str> = subjects.iter().map(String::as_str).collect();
    let mut findings = Vec::new();
    let mut scopes: HashMap<Uuid, HashSet<String>> = HashMap::new();
    let mut changed_scopes = Vec::new();

    for row in planned
        .iter()
        .filter(|row| row.entity == InventoryEntity::Tag)
    {
        let junction = row.junctions.iter().find(|j| {
            j.table == "inventory_tag_object_type"
                && j.local_column == "tag_id"
                && j.remote_column == "content_type_id"
        });
        let types: Vec<(Uuid, String, String)> = if let Some(junction) = junction {
            sqlx::query_as("SELECT id, app_label || '.' || model, inventory_content_type_table(app_label, model) FROM inventory_content_type WHERE id = ANY($1)")
            .bind(&junction.refs).fetch_all(&mut **transaction).await?
        } else {
            sqlx::query_as("SELECT c.id, c.app_label || '.' || c.model, inventory_content_type_table(c.app_label, c.model) FROM inventory_tag_object_type s JOIN inventory_content_type c ON c.id=s.content_type_id WHERE s.tag_id=$1")
                .bind(row.local_id).fetch_all(&mut **transaction).await?
        };
        let valid = junction.is_none_or(|junction| types.len() == junction.refs.len())
            && types.iter().all(|(_, token, table)| {
                validate_scope_token(token).is_ok() && subject_set.contains(table.as_str())
            });
        if !valid {
            findings.push(finding(
                batch,
                row,
                FindingCode::TagDefinitionInvalid,
                "object_types",
            )?);
            continue;
        }
        scopes.insert(
            row.local_id,
            types.into_iter().map(|(_, _, table)| table).collect(),
        );
        changed_scopes.push(row);
    }

    // Resolve retained scopes and planned attachments using the same registry gates.
    let mut memberships = Vec::new();
    let mut explicitly_replaced: HashMap<&str, Vec<Uuid>> = HashMap::new();
    for row in planned {
        let Some(table) = row.entity.table() else {
            continue;
        };
        if !subject_set.contains(table) {
            if row.columns.iter().any(|column| column.column == "tags") {
                findings.push(finding(batch, row, FindingCode::TagsInvalid, "tags")?);
            }
            continue;
        }
        let tags = if let Some(column) = row.columns.iter().find(|column| column.column == "tags") {
            let ColumnValue::Tags(tags) = &column.value else {
                return Err(AppError::Internal(
                    "inventory_transfer:invalid_planned_tags".into(),
                ));
            };
            explicitly_replaced
                .entry(table)
                .or_default()
                .push(row.local_id);
            tags.clone()
        } else {
            let mut query = sqlx::QueryBuilder::<Postgres>::new("SELECT tags FROM public.");
            query
                .push(quoted_subject(table)?)
                .push(" WHERE id=")
                .push_bind(row.local_id);
            query
                .build_query_scalar::<Vec<String>>()
                .fetch_optional(&mut **transaction)
                .await?
                .unwrap_or_default()
        };
        for tag in tags {
            let id = Uuid::parse_str(&tag).map_err(|_| {
                AppError::Internal("inventory_transfer:invalid_tag_membership".into())
            })?;
            memberships.push((row, table, id));
        }
    }
    let mut invalid_scopes = HashSet::new();
    for (row, _, id) in &memberships {
        if scopes.contains_key(id) {
            continue;
        }
        if invalid_scopes.contains(id) {
            findings.push(finding(batch, row, FindingCode::TagsInvalid, "tags")?);
            continue;
        }
        let types: Vec<(String, String)> = sqlx::query_as("SELECT c.app_label || '.' || c.model, inventory_content_type_table(c.app_label, c.model) FROM inventory_tag_object_type s JOIN inventory_content_type c ON c.id=s.content_type_id WHERE s.tag_id=$1")
            .bind(id).fetch_all(&mut **transaction).await?;
        if types.iter().any(|(token, table)| {
            validate_scope_token(token).is_err() || !subject_set.contains(table.as_str())
        }) {
            invalid_scopes.insert(*id);
            findings.push(finding(batch, row, FindingCode::TagsInvalid, "tags")?);
            continue;
        }
        scopes.insert(*id, types.into_iter().map(|(_, table)| table).collect());
    }
    for (row, table, id) in memberships {
        if scopes
            .get(&id)
            .is_some_and(|scope| !scope.is_empty() && !scope.contains(table))
        {
            findings.push(finding(batch, row, FindingCode::TagsInvalid, "tags")?);
        }
    }

    // Old uses cleared by a selected row must not block a restriction; all
    // other native uses, including rows outside the batch, remain in force.
    for row in changed_scopes {
        let scope = &scopes[&row.local_id];
        if scope.is_empty() {
            continue;
        }
        for table in &subjects {
            if scope.contains(table) {
                continue;
            }
            let excluded = explicitly_replaced
                .get(table.as_str())
                .cloned()
                .unwrap_or_default();
            let mut query =
                sqlx::QueryBuilder::<Postgres>::new("SELECT EXISTS (SELECT 1 FROM public.");
            query
                .push(quoted_subject(table)?)
                .push(" WHERE tags @> ARRAY[")
                .push_bind(row.local_id.to_string())
                .push("::text] AND NOT (id = ANY(")
                .push_bind(&excluded)
                .push("::uuid[])))");
            let used: bool = query
                .build_query_scalar()
                .fetch_one(&mut **transaction)
                .await?;
            if used {
                findings.push(finding(
                    batch,
                    row,
                    FindingCode::TagDefinitionInvalid,
                    "object_types",
                )?);
                break;
            }
        }
    }
    Ok(findings)
}
