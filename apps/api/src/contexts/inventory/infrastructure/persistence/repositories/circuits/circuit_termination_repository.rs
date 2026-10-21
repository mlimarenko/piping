use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::inventory::domain::CircuitTermination;
use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_search_ilike, push_string_in,
    push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::contexts::shared::row::col;
use crate::errors::AppError;

pub const CIRCUIT_TERMINATION_COLUMNS: &[ListColumn] = &[
    ListColumn::enum_filter(
        "term_side",
        "term_side",
        "term_side",
        "inventory.circuitTerminations.columns.termSide",
    ),
    ListColumn::fk_filter(
        "circuit_id",
        "circuit_id",
        "circuit_id",
        "circuit_id",
        "circuits",
        "inventory.circuitTerminations.columns.circuit",
    ),
    ListColumn::sortable(
        "xconnect_id",
        "xconnect_id",
        "inventory.circuitTerminations.columns.xconnect",
    ),
    ListColumn::sortable(
        "pp_info",
        "pp_info",
        "inventory.circuitTerminations.columns.ppInfo",
    ),
    ListColumn::sortable(
        "port_speed",
        "port_speed",
        "inventory.circuitTerminations.columns.portSpeed",
    ),
    ListColumn::sortable(
        "upstream_speed",
        "upstream_speed",
        "inventory.circuitTerminations.columns.upstreamSpeed",
    ),
    ListColumn::enum_filter(
        "mark_connected",
        "mark_connected",
        "mark_connected::text",
        "inventory.circuitTerminations.columns.markConnected",
    ),
    ListColumn::sortable(
        "description",
        "description",
        "inventory.circuitTerminations.columns.description",
    ),
];

#[derive(Debug, Clone, Default)]
pub struct CircuitTerminationFilter {
    pub search: Option<String>,
    pub term_side: Vec<String>,
    pub circuit_id: Vec<Uuid>,
    pub mark_connected: Vec<String>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait CircuitTerminationRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<CircuitTermination>, AppError>;
    async fn list_paginated(
        &self,
        filter: CircuitTerminationFilter,
        limit: Option<u32>,
        offset: Option<u32>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<CircuitTermination>, i64), AppError>;
    async fn create(&self, item: &CircuitTermination) -> Result<(), AppError>;
    async fn update(&self, item: &CircuitTermination) -> Result<(), AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxCircuitTerminationRepository {
    pool: PgPool,
}

impl SqlxCircuitTerminationRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<CircuitTermination, AppError> {
        Ok(CircuitTermination {
            id: row.try_get("id").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read circuits_circuittermination id: {}",
                    e
                ))
            })?,
            term_side: row.try_get("term_side").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read circuits_circuittermination term_side: {}",
                    e
                ))
            })?,
            circuit_id: row.try_get("circuit_id").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read circuits_circuittermination circuit_id: {}",
                    e
                ))
            })?,
            xconnect_id: row.try_get("xconnect_id").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read circuits_circuittermination xconnect_id: {}",
                    e
                ))
            })?,
            pp_info: row.try_get("pp_info").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read circuits_circuittermination pp_info: {}",
                    e
                ))
            })?,
            port_speed: row.try_get("port_speed").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read circuits_circuittermination port_speed: {}",
                    e
                ))
            })?,
            upstream_speed: row.try_get("upstream_speed").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read circuits_circuittermination upstream_speed: {}",
                    e
                ))
            })?,
            mark_connected: row.try_get("mark_connected").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read circuits_circuittermination mark_connected: {}",
                    e
                ))
            })?,
            description: row.try_get("description").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read circuits_circuittermination description: {}",
                    e
                ))
            })?,
            custom_fields: col(row, "custom_field_data")?,
        })
    }

    fn push_filters(builder: &mut QueryBuilder<'_, Postgres>, filter: &CircuitTerminationFilter) {
        let has_filter = filter.search.is_some()
            || !filter.term_side.is_empty()
            || !filter.circuit_id.is_empty()
            || !filter.mark_connected.is_empty();
        if !has_filter {
            return;
        }

        builder.push(" WHERE ");
        let mut where_state = WhereState::after_where();
        push_search_ilike(
            builder,
            &mut where_state,
            &[
                "term_side",
                "xconnect_id",
                "pp_info",
                "port_speed",
                "upstream_speed",
                "description",
            ],
            filter.search.as_deref(),
        );
        push_string_in(builder, &mut where_state, "term_side", &filter.term_side);
        push_uuid_in(builder, &mut where_state, "circuit_id", &filter.circuit_id);
        push_string_in(
            builder,
            &mut where_state,
            "mark_connected::text",
            &filter.mark_connected,
        );
    }

    async fn count_paginated(&self, filter: &CircuitTerminationFilter) -> Result<i64, AppError> {
        let mut builder =
            QueryBuilder::<Postgres>::new("SELECT COUNT(*) FROM circuits_circuittermination");
        Self::push_filters(&mut builder, filter);
        builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| {
                AppError::Database(format!(
                    "Failed to count circuits_circuitterminations: {}",
                    e
                ))
            })
    }
}

impl CircuitTerminationRepository for SqlxCircuitTerminationRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<CircuitTermination>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, term_side, circuit_id, xconnect_id, pp_info, port_speed, upstream_speed,
                   mark_connected, description, custom_field_data
            FROM circuits_circuittermination
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            AppError::Database(format!("Failed to load circuits_circuittermination: {}", e))
        })?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_paginated(
        &self,
        filter: CircuitTerminationFilter,
        limit: Option<u32>,
        offset: Option<u32>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<CircuitTermination>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT id, term_side, circuit_id, xconnect_id, pp_info, port_speed, upstream_speed,
                   mark_connected, description, custom_field_data,
                   COUNT(*) OVER() AS total_count
            FROM circuits_circuittermination
            "#,
        );
        Self::push_filters(&mut builder, &filter);
        push_order_by(
            &mut builder,
            CIRCUIT_TERMINATION_COLUMNS,
            sort.as_ref(),
            "term_side",
        );
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder.build().fetch_all(&self.pool).await.map_err(|e| {
            AppError::Database(format!(
                "Failed to list circuits_circuitterminations: {}",
                e
            ))
        })?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read circuits_circuittermination total_count: {}",
                    e
                ))
            })?,
            None => self.count_paginated(&filter).await?,
        };
        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<CircuitTermination>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, item: &CircuitTermination) -> Result<(), AppError> {
        sqlx::query(
            r#"
            INSERT INTO circuits_circuittermination (
                id, custom_field_data, term_side, circuit_id, xconnect_id, pp_info,
                port_speed, upstream_speed, mark_connected, description
            )
            VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6, $7, $8, $9)
            "#,
        )
        .bind(item.id)
        .bind(&item.term_side)
        .bind(item.circuit_id)
        .bind(&item.xconnect_id)
        .bind(&item.pp_info)
        .bind(item.port_speed)
        .bind(item.upstream_speed)
        .bind(item.mark_connected)
        .bind(&item.description)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            AppError::Database(format!(
                "Failed to create circuits_circuittermination: {}",
                e
            ))
        })?;
        Ok(())
    }

    async fn update(&self, item: &CircuitTermination) -> Result<(), AppError> {
        sqlx::query(
            r#"
            UPDATE circuits_circuittermination
            SET term_side = $2, circuit_id = $3, xconnect_id = $4, pp_info = $5,
                port_speed = $6, upstream_speed = $7, mark_connected = $8, description = $9
            WHERE id = $1
            "#,
        )
        .bind(item.id)
        .bind(&item.term_side)
        .bind(item.circuit_id)
        .bind(&item.xconnect_id)
        .bind(&item.pp_info)
        .bind(item.port_speed)
        .bind(item.upstream_speed)
        .bind(item.mark_connected)
        .bind(&item.description)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            AppError::Database(format!(
                "Failed to update circuits_circuittermination: {}",
                e
            ))
        })?;
        Ok(())
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(
            &self.pool,
            "circuits_circuittermination",
            id,
        )
        .await
        .map(|_| ())
    }
}
