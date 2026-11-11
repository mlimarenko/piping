use super::selector::push_target_selector_predicates;
use super::sql_strings::TARGET_PREVIEW_BASE_SQL;
use crate::contexts::playbooks::domain::{TargetSetFacet, TargetSetFacetKind, TargetSetFacetPage};
use crate::errors::{AppError, classify_sqlx_error_owned};
use serde_json::Value;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};

fn facet_query(
    kind: TargetSetFacetKind,
    selector: &Value,
    search: &str,
) -> Result<QueryBuilder<Postgres>, AppError> {
    let mut query = QueryBuilder::new("WITH selected AS (");
    query.push(TARGET_PREVIEW_BASE_SQL);
    push_target_selector_predicates(&mut query, selector)?;
    query.push("), facets AS (SELECT ");
    let (id, name) = match kind {
        TargetSetFacetKind::Region => ("selected.region_id::text", "selected.region_name"),
        TargetSetFacetKind::Site => ("selected.site_id::text", "selected.site_name"),
        TargetSetFacetKind::Tenant => ("selected.tenant_id::text", "selected.tenant_name"),
        TargetSetFacetKind::Role => ("selected.role_id::text", "selected.role_name"),
        TargetSetFacetKind::Platform => ("selected.platform_id::text", "selected.platform_name"),
        TargetSetFacetKind::Cluster => ("selected.cluster_id::text", "selected.cluster_name"),
        TargetSetFacetKind::DeviceType => {
            ("selected.device_type_id::text", "selected.device_type_name")
        }
        TargetSetFacetKind::Tag => ("tag.id::text", "tag.name"),
        TargetSetFacetKind::Status => ("selected.status::text", "selected.status::text"),
    };
    query
        .push(id)
        .push(" AS id, ")
        .push(name)
        .push(" AS name, ");
    query.push("COUNT(DISTINCT (selected.source_kind, selected.source_id)) AS count FROM selected");
    if matches!(kind, TargetSetFacetKind::Tag) {
        query.push(" CROSS JOIN LATERAL unnest(selected.tags) AS selected_tag(id)");
        query.push(" JOIN inventory_tag tag ON tag.id::text = selected_tag.id");
    }
    query
        .push(" WHERE ")
        .push(id)
        .push(" IS NOT NULL AND ")
        .push(name)
        .push(" IS NOT NULL");
    query
        .push(" GROUP BY ")
        .push(id)
        .push(", ")
        .push(name)
        .push(") ");
    query.push(", filtered AS (SELECT id, name, count FROM facets");
    if !search.is_empty() {
        query
            .push(" WHERE name ILIKE ")
            .push_bind(format!("%{search}%"));
    }
    query.push(") ");
    Ok(query)
}

pub(super) async fn list_target_set_facets(
    pool: &PgPool,
    kind: TargetSetFacetKind,
    selector: &Value,
    search: &str,
    limit: i64,
    offset: i64,
) -> Result<TargetSetFacetPage, AppError> {
    let mut query = facet_query(kind, selector, search)?;
    // Keep the total even when the requested page has no rows.
    query.push("SELECT page.id, page.name, page.count, totals.total FROM ");
    query.push("(SELECT COUNT(*) AS total FROM filtered) totals LEFT JOIN LATERAL ");
    query.push("(SELECT id, name, count FROM filtered ORDER BY LOWER(name), id LIMIT ");
    query
        .push_bind(limit)
        .push(" OFFSET ")
        .push_bind(offset)
        .push(") page ON TRUE");
    let rows = query
        .build()
        .fetch_all(pool)
        .await
        .map_err(classify_sqlx_error_owned)?;
    let total = rows
        .first()
        .map(|row| row.try_get::<i64, _>("total"))
        .transpose()
        .map_err(classify_sqlx_error_owned)?
        .unwrap_or(0);
    let mut items = Vec::with_capacity(rows.len());
    for row in &rows {
        let id: Option<String> = row.try_get("id").map_err(classify_sqlx_error_owned)?;
        if let Some(id) = id {
            items.push(TargetSetFacet {
                id,
                name: row.try_get("name").map_err(classify_sqlx_error_owned)?,
                count: row.try_get("count").map_err(classify_sqlx_error_owned)?,
            });
        }
    }
    Ok(TargetSetFacetPage {
        items,
        total,
        limit,
        offset,
    })
}
