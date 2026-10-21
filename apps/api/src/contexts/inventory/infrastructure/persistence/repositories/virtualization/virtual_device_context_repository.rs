use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::inventory::domain::VirtualDeviceContext;
use crate::contexts::shared::list_query::{
    FilterKind, ListColumn, SortSpec, WhereState, push_order_by, push_search_ilike, push_string_in,
    push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

pub const VIRTUAL_DEVICE_CONTEXT_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable(
        "name",
        "name",
        "inventory.virtualDeviceContexts.columns.name",
    ),
    ListColumn::enum_filter(
        "status",
        "status",
        "status",
        "inventory.virtualDeviceContexts.columns.status",
    ),
    ListColumn::sortable(
        "identifier",
        "identifier",
        "inventory.virtualDeviceContexts.columns.identifier",
    ),
    ListColumn::fk_filter(
        "device_id",
        "device_id",
        "device_id",
        "device_id",
        "devices",
        "inventory.virtualDeviceContexts.columns.device",
    ),
    ListColumn::fk_filter(
        "primary_ip4_id",
        "primary_ip4_id",
        "primary_ip4_id",
        "primary_ip4_id",
        "ipAddresses",
        "inventory.virtualDeviceContexts.columns.primaryIp4",
    ),
    ListColumn::fk_filter(
        "primary_ip6_id",
        "primary_ip6_id",
        "primary_ip6_id",
        "primary_ip6_id",
        "ipAddresses",
        "inventory.virtualDeviceContexts.columns.primaryIp6",
    ),
    ListColumn::fk_filter(
        "tenant_id",
        "tenant_id",
        "tenant_id",
        "tenant_id",
        "tenants",
        "inventory.virtualDeviceContexts.columns.tenant",
    ),
    ListColumn::sortable(
        "description",
        "description",
        "inventory.virtualDeviceContexts.columns.description",
    ),
    ListColumn::sortable(
        "comments",
        "comments",
        "inventory.virtualDeviceContexts.columns.comments",
    ),
];

#[derive(Debug, Clone, Default)]
pub struct VirtualDeviceContextFilter {
    pub search: Option<String>,
    pub status: Vec<String>,
    pub device_id: Vec<Uuid>,
    pub primary_ip4_id: Vec<Uuid>,
    pub primary_ip6_id: Vec<Uuid>,
    pub tenant_id: Vec<Uuid>,
    pub sort: Option<SortSpec>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait VirtualDeviceContextRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<VirtualDeviceContext>, AppError>;
    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        filter: VirtualDeviceContextFilter,
    ) -> Result<(Vec<VirtualDeviceContext>, i64), AppError>;
    async fn create(&self, item: &VirtualDeviceContext) -> Result<(), AppError>;
    async fn update(&self, item: &VirtualDeviceContext) -> Result<(), AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxVirtualDeviceContextRepository {
    pool: PgPool,
}

impl SqlxVirtualDeviceContextRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<VirtualDeviceContext, AppError> {
        Ok(VirtualDeviceContext {
            id: row.try_get("id").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read dcim_virtualdevicecontext id: {}",
                    e
                ))
            })?,
            name: row.try_get("name").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read dcim_virtualdevicecontext name: {}",
                    e
                ))
            })?,
            status: row.try_get("status").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read dcim_virtualdevicecontext status: {}",
                    e
                ))
            })?,
            identifier: row.try_get("identifier").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read dcim_virtualdevicecontext identifier: {}",
                    e
                ))
            })?,
            device_id: row.try_get("device_id").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read dcim_virtualdevicecontext device_id: {}",
                    e
                ))
            })?,
            primary_ip4_id: row.try_get("primary_ip4_id").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read dcim_virtualdevicecontext primary_ip4_id: {}",
                    e
                ))
            })?,
            primary_ip4: row
                .try_get::<Option<String>, _>("primary_ip4")
                .ok()
                .flatten(),
            primary_ip6_id: row.try_get("primary_ip6_id").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read dcim_virtualdevicecontext primary_ip6_id: {}",
                    e
                ))
            })?,
            primary_ip6: row
                .try_get::<Option<String>, _>("primary_ip6")
                .ok()
                .flatten(),
            tenant_id: row.try_get("tenant_id").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read dcim_virtualdevicecontext tenant_id: {}",
                    e
                ))
            })?,
            description: row.try_get("description").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read dcim_virtualdevicecontext description: {}",
                    e
                ))
            })?,
            comments: row.try_get("comments").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read dcim_virtualdevicecontext comments: {}",
                    e
                ))
            })?,
            custom_fields: row
                .try_get::<Option<serde_json::Value>, _>("custom_field_data")
                .ok()
                .flatten(),
        })
    }

    fn filter_values<'a>(
        filter: &'a VirtualDeviceContextFilter,
        param: &str,
    ) -> VirtualDeviceContextFilterValues<'a> {
        match param {
            "status" => VirtualDeviceContextFilterValues::Strings(&filter.status),
            "device_id" => VirtualDeviceContextFilterValues::Uuids(&filter.device_id),
            "primary_ip4_id" => VirtualDeviceContextFilterValues::Uuids(&filter.primary_ip4_id),
            "primary_ip6_id" => VirtualDeviceContextFilterValues::Uuids(&filter.primary_ip6_id),
            "tenant_id" => VirtualDeviceContextFilterValues::Uuids(&filter.tenant_id),
            _ => VirtualDeviceContextFilterValues::None,
        }
    }

    fn push_list_filters(
        builder: &mut QueryBuilder<'_, Postgres>,
        filter: &VirtualDeviceContextFilter,
    ) {
        let has_filter = filter.search.is_some()
            || !filter.status.is_empty()
            || !filter.device_id.is_empty()
            || !filter.primary_ip4_id.is_empty()
            || !filter.primary_ip6_id.is_empty()
            || !filter.tenant_id.is_empty();
        if !has_filter {
            return;
        }

        builder.push(" WHERE ");
        let mut where_state = WhereState::after_where();
        push_search_ilike(
            builder,
            &mut where_state,
            &["name", "status", "identifier", "description", "comments"],
            filter.search.as_deref(),
        );

        for column in VIRTUAL_DEVICE_CONTEXT_COLUMNS {
            let Some(column_filter) = column.filter else {
                continue;
            };
            match (
                column_filter.kind,
                Self::filter_values(filter, column_filter.param),
            ) {
                (FilterKind::String, VirtualDeviceContextFilterValues::Strings(values)) => {
                    push_string_in(builder, &mut where_state, column_filter.sql, values);
                }
                (FilterKind::Uuid, VirtualDeviceContextFilterValues::Uuids(values)) => {
                    push_uuid_in(builder, &mut where_state, column_filter.sql, values);
                }
                _ => {}
            }
        }
    }

    async fn count_filtered(&self, filter: &VirtualDeviceContextFilter) -> Result<i64, AppError> {
        let mut builder =
            QueryBuilder::<Postgres>::new("SELECT COUNT(*) FROM dcim_virtualdevicecontext");
        Self::push_list_filters(&mut builder, filter);
        builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| {
                AppError::Database(format!("Failed to count dcim_virtualdevicecontexts: {}", e))
            })
    }
}

enum VirtualDeviceContextFilterValues<'a> {
    Strings(&'a [String]),
    Uuids(&'a [Uuid]),
    None,
}

impl VirtualDeviceContextRepository for SqlxVirtualDeviceContextRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<VirtualDeviceContext>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, name, status, identifier, device_id, primary_ip4_id, NULL::text AS primary_ip4,
                   primary_ip6_id, NULL::text AS primary_ip6,
                   tenant_id, description, comments, custom_field_data
            FROM dcim_virtualdevicecontext
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            AppError::Database(format!("Failed to load dcim_virtualdevicecontext: {}", e))
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
        filter: VirtualDeviceContextFilter,
    ) -> Result<(Vec<VirtualDeviceContext>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT id, name, status, identifier, device_id, primary_ip4_id, primary_ip4,
                   primary_ip6_id, primary_ip6, tenant_id, description, comments,
                   custom_field_data,
                   COUNT(*) OVER() AS total_count
            FROM (
                SELECT vdc.id, vdc.name, vdc.status, vdc.identifier, vdc.device_id,
                       vdc.primary_ip4_id, host(ip4.address) AS primary_ip4,
                       vdc.primary_ip6_id, host(ip6.address) AS primary_ip6,
                       vdc.tenant_id, vdc.description, vdc.comments, vdc.custom_field_data
                FROM dcim_virtualdevicecontext vdc
                LEFT JOIN ipam_ipaddress ip4 ON ip4.id = vdc.primary_ip4_id
                LEFT JOIN ipam_ipaddress ip6 ON ip6.id = vdc.primary_ip6_id
            ) virtual_device_context_rows
            "#,
        );
        Self::push_list_filters(&mut builder, &filter);
        push_order_by(
            &mut builder,
            VIRTUAL_DEVICE_CONTEXT_COLUMNS,
            filter.sort.as_ref(),
            "name",
        );
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder.build().fetch_all(&self.pool).await.map_err(|e| {
            AppError::Database(format!("Failed to list dcim_virtualdevicecontexts: {}", e))
        })?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read dcim_virtualdevicecontext total_count: {}",
                    e
                ))
            })?,
            None => self.count_filtered(&filter).await?,
        };
        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<VirtualDeviceContext>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, item: &VirtualDeviceContext) -> Result<(), AppError> {
        sqlx::query(
            r#"
            INSERT INTO dcim_virtualdevicecontext (
                id, custom_field_data, name, status, identifier, device_id, primary_ip4_id,
                primary_ip6_id, tenant_id, description, comments
            )
            VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            "#,
        )
        .bind(item.id)
        .bind(&item.name)
        .bind(&item.status)
        .bind(item.identifier)
        .bind(item.device_id)
        .bind(item.primary_ip4_id)
        .bind(item.primary_ip6_id)
        .bind(item.tenant_id)
        .bind(&item.description)
        .bind(&item.comments)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            AppError::Database(format!("Failed to create dcim_virtualdevicecontext: {}", e))
        })?;
        Ok(())
    }

    async fn update(&self, item: &VirtualDeviceContext) -> Result<(), AppError> {
        sqlx::query(
            r#"
            UPDATE dcim_virtualdevicecontext
            SET name = $2, status = $3, identifier = $4, device_id = $5, primary_ip4_id = $6,
                primary_ip6_id = $7, tenant_id = $8, description = $9, comments = $10
            WHERE id = $1
            "#,
        )
        .bind(item.id)
        .bind(&item.name)
        .bind(&item.status)
        .bind(item.identifier)
        .bind(item.device_id)
        .bind(item.primary_ip4_id)
        .bind(item.primary_ip6_id)
        .bind(item.tenant_id)
        .bind(&item.description)
        .bind(&item.comments)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            AppError::Database(format!("Failed to update dcim_virtualdevicecontext: {}", e))
        })?;
        Ok(())
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(
            &self.pool,
            "dcim_virtualdevicecontext",
            id,
        )
        .await
        .map(|_| ())
    }
}
