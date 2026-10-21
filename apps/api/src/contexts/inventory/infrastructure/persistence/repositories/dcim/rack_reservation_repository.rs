use sqlx::postgres::PgRow;
use sqlx::{PgPool, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::inventory::domain::RackReservation;
use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, filter_column, push_order_by, push_search_ilike,
    push_string_in, push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

pub const RACK_RESERVATION_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("units", "units", "inventory.rackReservations.columns.units"),
    ListColumn::fk_filter(
        "rack_id",
        "rack_id",
        "rack_id",
        "rack_id",
        "racks",
        "inventory.rackReservations.columns.rack",
    ),
    ListColumn::fk_filter(
        "user_id",
        "user_id",
        "user_id",
        "user_id",
        "users",
        "inventory.rackReservations.columns.user",
    ),
    ListColumn::enum_filter(
        "status",
        "status",
        "status",
        "inventory.rackReservations.columns.status",
    ),
    ListColumn::fk_filter(
        "tenant_id",
        "tenant_id",
        "tenant_id",
        "tenant_id",
        "tenants",
        "inventory.rackReservations.columns.tenant",
    ),
    ListColumn::sortable(
        "description",
        "description",
        "inventory.rackReservations.columns.description",
    ),
    ListColumn::sortable(
        "comments",
        "comments",
        "inventory.rackReservations.columns.comments",
    ),
];

/// Rack reservation list filter. Empty vectors mean "no filter"; non-empty
/// vectors emit parameterized `IN` predicates against the repository-local
/// `RACK_RESERVATION_COLUMNS` metadata.
#[derive(Debug, Clone, Default)]
pub struct RackReservationFilter {
    pub search: Option<String>,
    pub rack_id: Vec<Uuid>,
    pub user_id: Vec<Uuid>,
    pub status: Vec<String>,
    pub tenant_id: Vec<Uuid>,
    pub sort: Option<SortSpec>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait RackReservationRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<RackReservation>, AppError>;
    async fn list_paginated(
        &self,
        filter: RackReservationFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<RackReservation>, i64), AppError>;
    async fn create(&self, item: &RackReservation) -> Result<(), AppError>;
    async fn update(&self, item: &RackReservation) -> Result<(), AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxRackReservationRepository {
    pool: PgPool,
}

impl SqlxRackReservationRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<RackReservation, AppError> {
        Ok(RackReservation {
            id: row.try_get("id").map_err(|e| {
                AppError::Database(format!("Failed to read dcim_rackreservation id: {}", e))
            })?,
            units: row.try_get("units").map_err(|e| {
                AppError::Database(format!("Failed to read dcim_rackreservation units: {}", e))
            })?,
            rack_id: row.try_get("rack_id").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read dcim_rackreservation rack_id: {}",
                    e
                ))
            })?,
            user_id: row.try_get("user_id").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read dcim_rackreservation user_id: {}",
                    e
                ))
            })?,
            user_name: row.try_get::<Option<String>, _>("user_name").ok().flatten(),
            status: row.try_get("status").map_err(|e| {
                AppError::Database(format!("Failed to read dcim_rackreservation status: {}", e))
            })?,
            tenant_id: row.try_get("tenant_id").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read dcim_rackreservation tenant_id: {}",
                    e
                ))
            })?,
            description: row.try_get("description").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read dcim_rackreservation description: {}",
                    e
                ))
            })?,
            comments: row.try_get("comments").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read dcim_rackreservation comments: {}",
                    e
                ))
            })?,
            custom_fields: row
                .try_get::<Option<serde_json::Value>, _>("custom_field_data")
                .ok()
                .flatten(),
        })
    }

    fn push_list_filters(
        builder: &mut QueryBuilder<'_, sqlx::Postgres>,
        filter: &RackReservationFilter,
    ) -> Result<(), AppError> {
        let has_filter = filter.search.is_some()
            || !filter.rack_id.is_empty()
            || !filter.user_id.is_empty()
            || !filter.status.is_empty()
            || !filter.tenant_id.is_empty();
        if !has_filter {
            return Ok(());
        }

        builder.push(" WHERE ");
        let mut where_state = WhereState::after_where();
        push_search_ilike(
            builder,
            &mut where_state,
            &["units", "status", "description", "comments"],
            filter.search.as_deref(),
        );

        push_uuid_in(
            builder,
            &mut where_state,
            filter_column(RACK_RESERVATION_COLUMNS, "rack reservation", "rack_id")?.sql,
            &filter.rack_id,
        );
        push_uuid_in(
            builder,
            &mut where_state,
            filter_column(RACK_RESERVATION_COLUMNS, "rack reservation", "user_id")?.sql,
            &filter.user_id,
        );
        push_string_in(
            builder,
            &mut where_state,
            filter_column(RACK_RESERVATION_COLUMNS, "rack reservation", "status")?.sql,
            &filter.status,
        );
        push_uuid_in(
            builder,
            &mut where_state,
            filter_column(RACK_RESERVATION_COLUMNS, "rack reservation", "tenant_id")?.sql,
            &filter.tenant_id,
        );

        Ok(())
    }

    async fn count_filtered(&self, filter: &RackReservationFilter) -> Result<i64, AppError> {
        let mut builder = QueryBuilder::new("SELECT COUNT(*) FROM dcim_rackreservation");
        Self::push_list_filters(&mut builder, filter)?;
        builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| {
                AppError::Database(format!("Failed to count dcim_rackreservations: {}", e))
            })
    }
}

impl RackReservationRepository for SqlxRackReservationRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<RackReservation>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, units, rack_id, user_id, NULL::text AS user_name, status, tenant_id, description, comments,
                   custom_field_data
            FROM dcim_rackreservation
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load dcim_rackreservation: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_paginated(
        &self,
        filter: RackReservationFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<RackReservation>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT id, units, rack_id, user_id, user_name, status, tenant_id,
                   description, comments, custom_field_data,
                   COUNT(*) OVER() AS total_count
            FROM (
                SELECT rr.id, rr.units, rr.rack_id, rr.user_id,
                       COALESCE(NULLIF(iu.username, ''), iu.email) AS user_name,
                       rr.status, rr.tenant_id, rr.description, rr.comments,
                       rr.custom_field_data
                FROM dcim_rackreservation rr
                JOIN identity_user iu ON iu.id = rr.user_id
            ) rack_reservation_rows
            "#,
        );
        Self::push_list_filters(&mut builder, &filter)?;
        push_order_by(
            &mut builder,
            RACK_RESERVATION_COLUMNS,
            filter.sort.as_ref(),
            "id",
        );
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder.build().fetch_all(&self.pool).await.map_err(|e| {
            AppError::Database(format!("Failed to list dcim_rackreservations: {}", e))
        })?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read dcim_rackreservation total_count: {}",
                    e
                ))
            })?,
            None => self.count_filtered(&filter).await?,
        };
        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<RackReservation>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, item: &RackReservation) -> Result<(), AppError> {
        sqlx::query(
            r#"
            INSERT INTO dcim_rackreservation (
                id, custom_field_data, units, rack_id, user_id, status, tenant_id,
                description, comments
            )
            VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6, $7, $8)
            "#,
        )
        .bind(item.id)
        .bind(&item.units)
        .bind(item.rack_id)
        .bind(item.user_id)
        .bind(&item.status)
        .bind(item.tenant_id)
        .bind(&item.description)
        .bind(&item.comments)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create dcim_rackreservation: {}", e)))?;
        Ok(())
    }

    async fn update(&self, item: &RackReservation) -> Result<(), AppError> {
        sqlx::query(
            r#"
            UPDATE dcim_rackreservation
            SET units = $2, rack_id = $3, user_id = $4, status = $5, tenant_id = $6,
                description = $7, comments = $8
            WHERE id = $1
            "#,
        )
        .bind(item.id)
        .bind(&item.units)
        .bind(item.rack_id)
        .bind(item.user_id)
        .bind(&item.status)
        .bind(item.tenant_id)
        .bind(&item.description)
        .bind(&item.comments)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update dcim_rackreservation: {}", e)))?;
        Ok(())
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "dcim_rackreservation", id)
            .await
            .map(|_| ())
    }
}
