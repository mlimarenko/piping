use super::row_mappers::target_preview_item_from_row;
use super::selector::{push_dynamic_target_order, push_target_selector_predicates};
use super::sql_strings::TARGET_PREVIEW_BASE_SQL;
use crate::contexts::playbooks::domain::{
    PlaybookTargetPreviewItem, PlaybookTargetSet, PlaybookTargetSetMode, PlaybookTargetSetPreview,
};
use crate::errors::{AppError, classify_sqlx_error_owned};
use sqlx::{PgPool, Postgres, QueryBuilder, Row};

pub(super) async fn preview_dynamic_target_set(
    pool: &PgPool,
    target_set: PlaybookTargetSet,
    limit: i64,
) -> Result<PlaybookTargetSetPreview, AppError> {
    let mut rows_query = QueryBuilder::<Postgres>::new(TARGET_PREVIEW_BASE_SQL);
    push_target_selector_predicates(&mut rows_query, &target_set.selector)?;
    push_dynamic_target_order(&mut rows_query, &target_set.selector)?;
    rows_query.push(" LIMIT ");
    rows_query.push_bind(limit);
    let rows = rows_query
        .build()
        .fetch_all(pool)
        .await
        .map_err(classify_sqlx_error_owned)?;

    let mut count_query = QueryBuilder::<Postgres>::new("SELECT COUNT(*) AS total FROM (");
    count_query.push(TARGET_PREVIEW_BASE_SQL);
    push_target_selector_predicates(&mut count_query, &target_set.selector)?;
    count_query.push(") AS selected_targets");
    let total: i64 = count_query
        .build()
        .fetch_one(pool)
        .await
        .map_err(classify_sqlx_error_owned)?
        .try_get("total")
        .map_err(classify_sqlx_error_owned)?;

    let items = rows
        .iter()
        .map(target_preview_item_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(PlaybookTargetSetPreview {
        target_set,
        truncated: total > limit,
        total,
        limit,
        items,
    })
}

pub(super) fn push_exact_connection_target_predicate(
    builder: &mut QueryBuilder<'_, Postgres>,
    target: &PlaybookTargetPreviewItem,
) {
    builder.push(" AND connection_targets.target_kind = ");
    builder.push_bind(target.target_kind.clone());
    builder.push(" AND connection_targets.target_id = ");
    builder.push_bind(target.target_id);
    builder.push(" AND connection_targets.source_kind = ");
    builder.push_bind(target.source_kind.clone());
    builder.push(" AND connection_targets.source_id = ");
    builder.push_bind(target.source_id);
}

pub(super) async fn target_belongs_to_target_set(
    pool: &PgPool,
    target_set: &PlaybookTargetSet,
    target: &PlaybookTargetPreviewItem,
) -> Result<bool, AppError> {
    if target_set.mode == PlaybookTargetSetMode::Static {
        let count: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*)
            FROM playbook_target_set_member
            WHERE target_set_id = $1
              AND target_kind = $2
              AND target_id = $3
              AND source_kind = $4
              AND source_id = $5
            "#,
        )
        .bind(target_set.id)
        .bind(&target.target_kind)
        .bind(target.target_id)
        .bind(&target.source_kind)
        .bind(target.source_id)
        .fetch_one(pool)
        .await
        .map_err(classify_sqlx_error_owned)?;
        return Ok(count > 0);
    }

    let mut builder = QueryBuilder::<Postgres>::new("SELECT 1 FROM (");
    builder.push(TARGET_PREVIEW_BASE_SQL);
    push_target_selector_predicates(&mut builder, &target_set.selector)?;
    push_exact_connection_target_predicate(&mut builder, target);
    builder.push(") AS selected_targets LIMIT 1");
    Ok(builder
        .build()
        .fetch_optional(pool)
        .await
        .map_err(classify_sqlx_error_owned)?
        .is_some())
}

pub(super) async fn preview_static_target_set(
    pool: &PgPool,
    target_set: PlaybookTargetSet,
    limit: i64,
) -> Result<PlaybookTargetSetPreview, AppError> {
    let rows_sql = format!(
        r#"
        WITH selected_members AS (
            SELECT target_kind, target_id, source_kind, source_id, sort_order
            FROM playbook_target_set_member
            WHERE target_set_id = $1
            ORDER BY sort_order ASC, added_at ASC
        ), targets AS (
            {TARGET_PREVIEW_BASE_SQL}
        )
        SELECT targets.*
        FROM selected_members
        JOIN targets
          ON targets.target_kind = selected_members.target_kind
         AND targets.target_id = selected_members.target_id
         AND targets.source_kind = selected_members.source_kind
         AND targets.source_id = selected_members.source_id
        ORDER BY selected_members.sort_order ASC, targets.name ASC, targets.target_id ASC
        LIMIT $2
        "#
    );
    let rows = sqlx::query(&rows_sql)
        .bind(target_set.id)
        .bind(limit)
        .fetch_all(pool)
        .await
        .map_err(classify_sqlx_error_owned)?;
    let count_sql = format!(
        r#"
        WITH selected_members AS (
            SELECT target_kind, target_id, source_kind, source_id
            FROM playbook_target_set_member
            WHERE target_set_id = $1
        ), targets AS (
            {TARGET_PREVIEW_BASE_SQL}
        )
        SELECT COUNT(*) AS total
        FROM selected_members
        JOIN targets
          ON targets.target_kind = selected_members.target_kind
         AND targets.target_id = selected_members.target_id
         AND targets.source_kind = selected_members.source_kind
         AND targets.source_id = selected_members.source_id
        "#
    );
    let total: i64 = sqlx::query(&count_sql)
        .bind(target_set.id)
        .fetch_one(pool)
        .await
        .map_err(classify_sqlx_error_owned)?
        .try_get("total")
        .map_err(classify_sqlx_error_owned)?;
    let items = rows
        .iter()
        .map(target_preview_item_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(PlaybookTargetSetPreview {
        target_set,
        truncated: total > limit,
        total,
        limit,
        items,
    })
}
