use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::inventory::domain::VirtualCircuit;
use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_search_ilike, push_string_in,
    push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::contexts::shared::row::col;
use crate::errors::AppError;

/// Hydrated list row for `circuits_virtualcircuit` — includes provider-network,
/// type and tenant names resolved via LEFT JOIN for the collection list endpoint.
#[derive(Debug, Clone)]
pub struct VirtualCircuitListItem {
    pub record: VirtualCircuit,
    pub provider_network_name: Option<String>,
    pub type_name: Option<String>,
    pub provider_account_name: Option<String>,
    pub tenant_name: Option<String>,
}

pub const VIRTUAL_CIRCUIT_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("cid", "vc.cid", "inventory.resources.fields.cid"),
    ListColumn::enum_filter(
        "status",
        "vc.status",
        "vc.status",
        "inventory.resources.fields.status",
    ),
    ListColumn::fk_filter(
        "provider_network",
        "pn.name",
        "provider_network_id",
        "vc.provider_network_id",
        "Provider Networks",
        "inventory.resources.fields.providerNetwork",
    ),
    ListColumn::fk_filter(
        "virtual_circuit_type",
        "vct.name",
        "type_id",
        "vc.type_id",
        "Virtual Circuit Types",
        "inventory.resources.fields.virtualCircuitType",
    ),
    ListColumn::fk_filter(
        "provider_account",
        "pa.name",
        "provider_account_id",
        "vc.provider_account_id",
        "Provider Accounts",
        "inventory.resources.fields.providerAccount",
    ),
    ListColumn::fk_filter(
        "tenant",
        "t.name",
        "tenant_id",
        "vc.tenant_id",
        "Tenants",
        "inventory.resources.fields.tenant",
    ),
    ListColumn::sortable(
        "description",
        "vc.description",
        "inventory.resources.fields.description",
    ),
    ListColumn::sortable(
        "comments",
        "vc.comments",
        "inventory.resources.fields.comments",
    ),
];

#[derive(Debug, Clone, Default)]
pub struct VirtualCircuitFilter {
    pub search: Option<String>,
    pub status: Vec<String>,
    pub provider_network_id: Vec<Uuid>,
    pub type_id: Vec<Uuid>,
    pub provider_account_id: Vec<Uuid>,
    pub tenant_id: Vec<Uuid>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait VirtualCircuitRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<VirtualCircuit>, AppError>;
    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<VirtualCircuit>, i64), AppError>;
    async fn list_paginated_hydrated(
        &self,
        filter: VirtualCircuitFilter,
        sort: Option<SortSpec>,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<VirtualCircuitListItem>, i64), AppError>;
    async fn create(&self, item: &VirtualCircuit) -> Result<(), AppError>;
    async fn update(&self, item: &VirtualCircuit) -> Result<(), AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxVirtualCircuitRepository {
    pool: PgPool,
}

impl SqlxVirtualCircuitRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn push_hydrated_filters(
        builder: &mut QueryBuilder<'_, Postgres>,
        filter: &VirtualCircuitFilter,
    ) {
        let has_filter = filter.search.is_some()
            || !filter.status.is_empty()
            || !filter.provider_network_id.is_empty()
            || !filter.type_id.is_empty()
            || !filter.provider_account_id.is_empty()
            || !filter.tenant_id.is_empty();
        if !has_filter {
            return;
        }

        builder.push(" WHERE ");
        let mut where_state = WhereState::after_where();
        push_search_ilike(
            builder,
            &mut where_state,
            &["vc.cid", "vc.status", "vc.description", "vc.comments"],
            filter.search.as_deref(),
        );
        push_string_in(builder, &mut where_state, "vc.status", &filter.status);
        push_uuid_in(
            builder,
            &mut where_state,
            "vc.provider_network_id",
            &filter.provider_network_id,
        );
        push_uuid_in(builder, &mut where_state, "vc.type_id", &filter.type_id);
        push_uuid_in(
            builder,
            &mut where_state,
            "vc.provider_account_id",
            &filter.provider_account_id,
        );
        push_uuid_in(builder, &mut where_state, "vc.tenant_id", &filter.tenant_id);
    }

    async fn count_hydrated(&self, filter: &VirtualCircuitFilter) -> Result<i64, AppError> {
        let mut builder =
            QueryBuilder::<Postgres>::new("SELECT COUNT(*) FROM circuits_virtualcircuit vc");
        Self::push_hydrated_filters(&mut builder, filter);
        builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| {
                AppError::Database(format!("Failed to count circuits_virtualcircuits: {}", e))
            })
    }

    fn parse_row(row: &PgRow) -> Result<VirtualCircuit, AppError> {
        Ok(VirtualCircuit {
            id: row.try_get("id").map_err(|e| {
                AppError::Database(format!("Failed to read circuits_virtualcircuit id: {}", e))
            })?,
            cid: row.try_get("cid").map_err(|e| {
                AppError::Database(format!("Failed to read circuits_virtualcircuit cid: {}", e))
            })?,
            status: row.try_get("status").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read circuits_virtualcircuit status: {}",
                    e
                ))
            })?,
            provider_network_id: row.try_get("provider_network_id").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read circuits_virtualcircuit provider_network_id: {}",
                    e
                ))
            })?,
            type_id: row.try_get("type_id").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read circuits_virtualcircuit type_id: {}",
                    e
                ))
            })?,
            provider_account_id: row.try_get("provider_account_id").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read circuits_virtualcircuit provider_account_id: {}",
                    e
                ))
            })?,
            tenant_id: row.try_get("tenant_id").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read circuits_virtualcircuit tenant_id: {}",
                    e
                ))
            })?,
            description: row.try_get("description").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read circuits_virtualcircuit description: {}",
                    e
                ))
            })?,
            comments: row.try_get("comments").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read circuits_virtualcircuit comments: {}",
                    e
                ))
            })?,
            custom_fields: col(row, "custom_field_data")?,
        })
    }
}

impl VirtualCircuitRepository for SqlxVirtualCircuitRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<VirtualCircuit>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, cid, status, provider_network_id, type_id, provider_account_id,
                   tenant_id, description, comments, custom_field_data
            FROM circuits_virtualcircuit
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            AppError::Database(format!("Failed to load circuits_virtualcircuit: {}", e))
        })?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<VirtualCircuit>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let rows = sqlx::query(
            r#"
            SELECT id, cid, status, provider_network_id, type_id, provider_account_id,
                   tenant_id, description, comments, custom_field_data,
                   COUNT(*) OVER() AS total_count
            FROM circuits_virtualcircuit
            ORDER BY cid
            LIMIT $1 OFFSET $2
            "#,
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            AppError::Database(format!("Failed to list circuits_virtualcircuits: {}", e))
        })?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read circuits_virtualcircuit total_count: {}",
                    e
                ))
            })?,
            None => sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM circuits_virtualcircuit")
                .fetch_one(&self.pool)
                .await
                .map_err(|e| {
                    AppError::Database(format!("Failed to count circuits_virtualcircuits: {}", e))
                })?,
        };
        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<VirtualCircuit>, AppError>>()?;
        Ok((items, total))
    }

    async fn list_paginated_hydrated(
        &self,
        filter: VirtualCircuitFilter,
        sort: Option<SortSpec>,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<VirtualCircuitListItem>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;
        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT vc.id, vc.cid, vc.status, vc.provider_network_id, vc.type_id,
                   vc.provider_account_id, vc.tenant_id, vc.description, vc.comments,
                   vc.custom_field_data,
                   pn.name AS provider_network_name,
                   vct.name AS type_name,
                   pa.name AS provider_account_name,
                   t.name AS tenant_name,
                   COUNT(*) OVER() AS total_count
            FROM circuits_virtualcircuit vc
            LEFT JOIN circuits_providernetwork pn ON pn.id = vc.provider_network_id
            LEFT JOIN circuits_virtualcircuittype vct ON vct.id = vc.type_id
            LEFT JOIN circuits_provideraccount pa ON pa.id = vc.provider_account_id
            LEFT JOIN tenancy_tenant t ON t.id = vc.tenant_id
            "#,
        );
        Self::push_hydrated_filters(&mut builder, &filter);
        push_order_by(
            &mut builder,
            VIRTUAL_CIRCUIT_COLUMNS,
            sort.as_ref(),
            "vc.cid",
        );
        builder.push(", vc.id LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);
        let rows = builder.build().fetch_all(&self.pool).await.map_err(|e| {
            AppError::Database(format!(
                "Failed to list circuits_virtualcircuits hydrated: {}",
                e
            ))
        })?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read circuits_virtualcircuit total_count: {}",
                    e
                ))
            })?,
            None => self.count_hydrated(&filter).await?,
        };
        let items = rows
            .iter()
            .map(|row| {
                Ok(VirtualCircuitListItem {
                    record: Self::parse_row(row)?,
                    provider_network_name: col(row, "provider_network_name")?,
                    type_name: col(row, "type_name")?,
                    provider_account_name: col(row, "provider_account_name")?,
                    tenant_name: col(row, "tenant_name")?,
                })
            })
            .collect::<Result<Vec<VirtualCircuitListItem>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, item: &VirtualCircuit) -> Result<(), AppError> {
        sqlx::query(
            r#"
            INSERT INTO circuits_virtualcircuit (
                id, custom_field_data, cid, status, provider_network_id, type_id,
                provider_account_id, tenant_id, description, comments
            )
            VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6, $7, $8, $9)
            "#,
        )
        .bind(item.id)
        .bind(&item.cid)
        .bind(&item.status)
        .bind(item.provider_network_id)
        .bind(item.type_id)
        .bind(item.provider_account_id)
        .bind(item.tenant_id)
        .bind(&item.description)
        .bind(&item.comments)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            AppError::Database(format!("Failed to create circuits_virtualcircuit: {}", e))
        })?;
        Ok(())
    }

    async fn update(&self, item: &VirtualCircuit) -> Result<(), AppError> {
        sqlx::query(
            r#"
            UPDATE circuits_virtualcircuit
            SET cid = $2, status = $3, provider_network_id = $4, type_id = $5,
                provider_account_id = $6, tenant_id = $7, description = $8, comments = $9
            WHERE id = $1
            "#,
        )
        .bind(item.id)
        .bind(&item.cid)
        .bind(&item.status)
        .bind(item.provider_network_id)
        .bind(item.type_id)
        .bind(item.provider_account_id)
        .bind(item.tenant_id)
        .bind(&item.description)
        .bind(&item.comments)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            AppError::Database(format!("Failed to update circuits_virtualcircuit: {}", e))
        })?;
        Ok(())
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(
            &self.pool,
            "circuits_virtualcircuit",
            id,
        )
        .await
        .map(|_| ())
    }
}
