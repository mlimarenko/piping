use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::inventory::domain::MacAddress;
use crate::contexts::shared::list_query::{
    FilterKind, ListColumn, SortSpec, WhereState, push_order_by, push_search_ilike, push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

pub const MAC_ADDRESS_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable(
        "mac_address",
        "mac_address",
        "inventory.macAddresses.columns.macAddress",
    ),
    ListColumn::fk_filter(
        "assigned_object_type_id",
        "assigned_object_type_id",
        "assigned_object_type_id",
        "assigned_object_type_id",
        "contentTypes",
        "inventory.macAddresses.columns.assignedObjectType",
    ),
    ListColumn::fk_filter(
        "assigned_object_id",
        "assigned_object_id",
        "assigned_object_id",
        "assigned_object_id",
        "objects",
        "inventory.macAddresses.columns.assignedObject",
    ),
    ListColumn::sortable(
        "description",
        "description",
        "inventory.macAddresses.columns.description",
    ),
    ListColumn::sortable(
        "comments",
        "comments",
        "inventory.macAddresses.columns.comments",
    ),
];

#[derive(Debug, Clone, Default)]
pub struct MacAddressFilter {
    pub search: Option<String>,
    pub assigned_object_type_id: Vec<Uuid>,
    pub assigned_object_id: Vec<Uuid>,
    pub sort: Option<SortSpec>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait MacAddressRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<MacAddress>, AppError>;
    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        filter: MacAddressFilter,
    ) -> Result<(Vec<MacAddress>, i64), AppError>;
    async fn create(&self, item: &MacAddress) -> Result<(), AppError>;
    async fn update(&self, item: &MacAddress) -> Result<(), AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxMacAddressRepository {
    pool: PgPool,
}

impl SqlxMacAddressRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<MacAddress, AppError> {
        Ok(MacAddress {
            id: row.try_get("id").map_err(|e| {
                AppError::Database(format!("Failed to read dcim_macaddress id: {}", e))
            })?,
            mac_address: row.try_get("mac_address").map_err(|e| {
                AppError::Database(format!("Failed to read dcim_macaddress mac_address: {}", e))
            })?,
            assigned_object_type_id: row.try_get("assigned_object_type_id").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read dcim_macaddress assigned_object_type_id: {}",
                    e
                ))
            })?,
            assigned_object_id: row.try_get("assigned_object_id").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read dcim_macaddress assigned_object_id: {}",
                    e
                ))
            })?,
            description: row.try_get("description").map_err(|e| {
                AppError::Database(format!("Failed to read dcim_macaddress description: {}", e))
            })?,
            comments: row.try_get("comments").map_err(|e| {
                AppError::Database(format!("Failed to read dcim_macaddress comments: {}", e))
            })?,
            custom_fields: row
                .try_get::<Option<serde_json::Value>, _>("custom_field_data")
                .ok()
                .flatten(),
        })
    }

    fn filter_values<'a>(filter: &'a MacAddressFilter, param: &str) -> &'a [Uuid] {
        match param {
            "assigned_object_type_id" => &filter.assigned_object_type_id,
            "assigned_object_id" => &filter.assigned_object_id,
            _ => &[],
        }
    }

    fn push_list_filters(builder: &mut QueryBuilder<'_, Postgres>, filter: &MacAddressFilter) {
        if filter.search.is_none()
            && filter.assigned_object_type_id.is_empty()
            && filter.assigned_object_id.is_empty()
        {
            return;
        }

        builder.push(" WHERE ");
        let mut where_state = WhereState::after_where();
        push_search_ilike(
            builder,
            &mut where_state,
            &["mac_address", "description", "comments"],
            filter.search.as_deref(),
        );

        for column in MAC_ADDRESS_COLUMNS {
            let Some(column_filter) = column.filter else {
                continue;
            };
            if column_filter.kind == FilterKind::Uuid {
                push_uuid_in(
                    builder,
                    &mut where_state,
                    column_filter.sql,
                    Self::filter_values(filter, column_filter.param),
                );
            }
        }
    }

    async fn count_filtered(&self, filter: &MacAddressFilter) -> Result<i64, AppError> {
        let mut builder = QueryBuilder::<Postgres>::new("SELECT COUNT(*) FROM dcim_macaddress");
        Self::push_list_filters(&mut builder, filter);
        builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to count dcim_macaddresses: {}", e)))
    }
}

impl MacAddressRepository for SqlxMacAddressRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<MacAddress>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, mac_address::text AS mac_address, assigned_object_type_id,
                   assigned_object_id, description, comments, custom_field_data
            FROM dcim_macaddress
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load dcim_macaddress: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        filter: MacAddressFilter,
    ) -> Result<(Vec<MacAddress>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            "
            SELECT id, mac_address::text AS mac_address, assigned_object_type_id,
                   assigned_object_id, description, comments, custom_field_data,
                   COUNT(*) OVER() AS total_count
            FROM dcim_macaddress
            ",
        );
        Self::push_list_filters(&mut builder, &filter);
        push_order_by(
            &mut builder,
            MAC_ADDRESS_COLUMNS,
            filter.sort.as_ref(),
            "mac_address",
        );
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows =
            builder.build().fetch_all(&self.pool).await.map_err(|e| {
                AppError::Database(format!("Failed to list dcim_macaddresses: {}", e))
            })?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!("Failed to read dcim_macaddress total_count: {}", e))
            })?,
            None => self.count_filtered(&filter).await?,
        };
        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<MacAddress>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, item: &MacAddress) -> Result<(), AppError> {
        sqlx::query(
            r#"
            INSERT INTO dcim_macaddress (
                id, custom_field_data, mac_address, assigned_object_type_id,
                assigned_object_id, description, comments
            )
            VALUES ($1, '{}'::jsonb, $2::macaddr, $3, $4, $5, $6)
            "#,
        )
        .bind(item.id)
        .bind(&item.mac_address)
        .bind(item.assigned_object_type_id)
        .bind(item.assigned_object_id)
        .bind(&item.description)
        .bind(&item.comments)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create dcim_macaddress: {}", e)))?;
        Ok(())
    }

    async fn update(&self, item: &MacAddress) -> Result<(), AppError> {
        sqlx::query(
            r#"
            UPDATE dcim_macaddress
            SET mac_address = $2::macaddr, assigned_object_type_id = $3,
                assigned_object_id = $4, description = $5, comments = $6
            WHERE id = $1
            "#,
        )
        .bind(item.id)
        .bind(&item.mac_address)
        .bind(item.assigned_object_type_id)
        .bind(item.assigned_object_id)
        .bind(&item.description)
        .bind(&item.comments)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update dcim_macaddress: {}", e)))?;
        Ok(())
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "dcim_macaddress", id)
            .await
            .map(|_| ())
    }
}
