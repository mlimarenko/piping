use sqlx::{PgPool, Postgres, QueryBuilder, Row, Transaction, postgres::PgRow};
use uuid::Uuid;

use crate::contexts::inventory::domain::{FhrpGroupAssignment, content_type};
use crate::contexts::inventory::infrastructure::persistence::repositories::content_type_lookup::lookup_content_type_id_tx;
use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, filter_column, push_order_by, push_search_ilike,
    push_string_in, push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::{AppError, classify_sqlx_error_owned};

pub const FHRP_GROUP_ASSIGNMENT_COLUMNS: &[ListColumn] = &[
    ListColumn::fk_filter(
        "group_id",
        "a.group_id",
        "group_id",
        "a.group_id",
        "fhrp_groups",
        "inventory.resources.fields.group_id",
    ),
    ListColumn::fk_filter(
        "interface_id",
        "a.interface_id",
        "interface_id",
        "a.interface_id",
        "interfaces",
        "inventory.resources.fields.interface_id",
    ),
    ListColumn::enum_filter(
        "interface_type",
        "ct.app_label || '.' || ct.model",
        "ct.app_label || '.' || ct.model",
        "inventory.resources.fields.interface_type",
    ),
    ListColumn::sortable(
        "priority",
        "a.priority",
        "inventory.resources.fields.priority",
    ),
];

macro_rules! assignment_from { () => { " FROM ipam_fhrpgroupassignment a JOIN inventory_content_type ct ON ct.id=a.interface_type_id" }; }
macro_rules! assignment_select { () => { "SELECT a.id, a.group_id, ct.app_label || '.' || ct.model AS interface_type, a.interface_id, a.priority, a.created, a.last_updated, a.external_id, a.piping_synced_at" }; }

#[derive(Debug, Clone, Default)]
pub struct FhrpGroupAssignmentFilter {
    pub group_id: Vec<Uuid>,
    pub interface_id: Vec<Uuid>,
    pub interface_type: Vec<String>,
    pub search: Option<String>,
    pub sort: Option<SortSpec>,
}

#[expect(
    async_fn_in_trait,
    reason = "Matches the inventory repository ports; consumers use the concrete SQLx repository"
)]
pub trait FhrpGroupAssignmentRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<FhrpGroupAssignment>, AppError>;
    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        filter: FhrpGroupAssignmentFilter,
    ) -> Result<(Vec<FhrpGroupAssignment>, i64), AppError>;
    async fn create(&self, item: &FhrpGroupAssignment) -> Result<(), AppError>;
    async fn update(&self, item: &FhrpGroupAssignment) -> Result<(), AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxFhrpGroupAssignmentRepository {
    pool: PgPool,
}

fn parse_row(row: &PgRow) -> Result<FhrpGroupAssignment, sqlx::Error> {
    Ok(FhrpGroupAssignment {
        id: row.try_get("id")?,
        group_id: row.try_get("group_id")?,
        interface_type: row.try_get("interface_type")?,
        interface_id: row.try_get("interface_id")?,
        priority: row.try_get("priority")?,
        created: row.try_get("created")?,
        last_updated: row.try_get("last_updated")?,
        external_id: row.try_get("external_id")?,
        piping_synced_at: row.try_get("piping_synced_at")?,
    })
}

fn push_filters(
    builder: &mut QueryBuilder<Postgres>,
    filter: &FhrpGroupAssignmentFilter,
) -> Result<(), AppError> {
    let mut state = WhereState::new();
    push_uuid_in(
        builder,
        &mut state,
        filter_column(FHRP_GROUP_ASSIGNMENT_COLUMNS, "FHRP assignment", "group_id")?.sql,
        &filter.group_id,
    );
    push_uuid_in(
        builder,
        &mut state,
        filter_column(
            FHRP_GROUP_ASSIGNMENT_COLUMNS,
            "FHRP assignment",
            "interface_id",
        )?
        .sql,
        &filter.interface_id,
    );
    push_string_in(
        builder,
        &mut state,
        filter_column(
            FHRP_GROUP_ASSIGNMENT_COLUMNS,
            "FHRP assignment",
            "interface_type",
        )?
        .sql,
        &filter.interface_type,
    );
    push_search_ilike(
        builder,
        &mut state,
        &[
            "a.group_id",
            "a.interface_id",
            "a.priority",
            "ct.app_label || '.' || ct.model",
        ],
        filter.search.as_deref(),
    );
    Ok(())
}

/// Locks the typed target while writing: the polymorphic UUID has no database FK.
async fn validate_interface(
    tx: &mut Transaction<'_, Postgres>,
    item: &FhrpGroupAssignment,
) -> Result<Uuid, AppError> {
    item.validate()?;
    let definition = content_type::by_token(&item.interface_type)
        .ok_or_else(|| AppError::Validation("Interface content type is not registered".into()))?;
    let type_id = lookup_content_type_id_tx(tx, definition.app_label, definition.model)
        .await?
        .ok_or_else(|| AppError::Validation("Interface content type is not seeded".into()))?;
    // The table comes from the canonical registry, after the domain's closed allowlist.
    let mut query = QueryBuilder::<Postgres>::new("SELECT id FROM ");
    query
        .push(definition.table)
        .push(" WHERE id=")
        .push_bind(item.interface_id)
        .push(" FOR SHARE");
    let target: Option<Uuid> = query
        .build_query_scalar()
        .fetch_optional(&mut **tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
    if target.is_none() {
        return Err(AppError::Validation(
            "Interface does not exist for the selected interface_type".into(),
        ));
    }
    Ok(type_id)
}

impl SqlxFhrpGroupAssignmentRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl FhrpGroupAssignmentRepository for SqlxFhrpGroupAssignmentRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<FhrpGroupAssignment>, AppError> {
        sqlx::query(concat!(
            assignment_select!(),
            assignment_from!(),
            " WHERE a.id=$1"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(classify_sqlx_error_owned)?
        .as_ref()
        .map(parse_row)
        .transpose()
        .map_err(classify_sqlx_error_owned)
    }

    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        filter: FhrpGroupAssignmentFilter,
    ) -> Result<(Vec<FhrpGroupAssignment>, i64), AppError> {
        let mut count =
            QueryBuilder::<Postgres>::new(concat!("SELECT COUNT(*)", assignment_from!()));
        push_filters(&mut count, &filter)?;
        let total = count
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let mut query =
            QueryBuilder::<Postgres>::new(concat!(assignment_select!(), assignment_from!()));
        push_filters(&mut query, &filter)?;
        push_order_by(
            &mut query,
            FHRP_GROUP_ASSIGNMENT_COLUMNS,
            filter.sort.as_ref(),
            "a.priority",
        );
        query.push(", a.id");
        query
            .push(" LIMIT ")
            .push_bind(i64::from(clamp_limit(limit)))
            .push(" OFFSET ")
            .push_bind(i64::from(clamp_offset(offset)));
        let rows = query
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let items = rows
            .iter()
            .map(parse_row)
            .collect::<Result<Vec<_>, _>>()
            .map_err(classify_sqlx_error_owned)?;
        Ok((items, total))
    }

    async fn create(&self, item: &FhrpGroupAssignment) -> Result<(), AppError> {
        item.validate()?;
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        let type_id = validate_interface(&mut tx, item).await?;
        sqlx::query("INSERT INTO ipam_fhrpgroupassignment (id,group_id,interface_type_id,interface_id,priority,external_id,piping_synced_at,created,last_updated) VALUES ($1,$2,$3,$4,$5,$6,$7,NOW(),NOW())")
            .bind(item.id).bind(item.group_id).bind(type_id).bind(item.interface_id).bind(item.priority)
            .bind(item.external_id).bind(item.piping_synced_at)
            .execute(&mut *tx).await.map_err(classify_sqlx_error_owned)?;
        tx.commit().await.map_err(classify_sqlx_error_owned)
    }

    async fn update(&self, item: &FhrpGroupAssignment) -> Result<(), AppError> {
        item.validate()?;
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        let type_id = validate_interface(&mut tx, item).await?;
        let result = sqlx::query("UPDATE ipam_fhrpgroupassignment SET group_id=$2, interface_type_id=$3, interface_id=$4, priority=$5, external_id=$6, piping_synced_at=$7, last_updated=NOW() WHERE id=$1")
            .bind(item.id).bind(item.group_id).bind(type_id).bind(item.interface_id).bind(item.priority)
            .bind(item.external_id).bind(item.piping_synced_at)
            .execute(&mut *tx).await.map_err(classify_sqlx_error_owned)?;
        if result.rows_affected() == 0 {
            return Err(AppError::NotFound("FHRP group assignment not found".into()));
        }
        tx.commit().await.map_err(classify_sqlx_error_owned)
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        let result = sqlx::query("DELETE FROM ipam_fhrpgroupassignment WHERE id=$1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        if result.rows_affected() == 0 {
            return Err(AppError::NotFound("FHRP group assignment not found".into()));
        }
        Ok(())
    }
}
