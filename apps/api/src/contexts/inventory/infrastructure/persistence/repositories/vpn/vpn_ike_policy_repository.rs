use sqlx::postgres::PgRow;
use sqlx::{PgPool, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::inventory::domain::VpnIkePolicy;
use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_search_ilike, push_string_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::contexts::shared::row::col;
use crate::errors::AppError;

pub const VPN_IKE_POLICY_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "name", "inventory.resources.fields.name"),
    ListColumn::sortable(
        "version",
        "version",
        "inventory.resources.fields.ikeVersion",
    ),
    ListColumn::enum_filter("mode", "mode", "mode", "inventory.resources.fields.vpnMode"),
    ListColumn::sortable(
        "preshared_key",
        "preshared_key",
        "inventory.resources.fields.presharedKey",
    ),
    ListColumn::sortable(
        "description",
        "description",
        "inventory.resources.fields.description",
    ),
    ListColumn::sortable(
        "comments",
        "comments",
        "inventory.resources.fields.comments",
    ),
];

#[derive(Debug, Clone, Default)]
pub struct VpnIkePolicyFilter {
    pub search: Option<String>,
    pub mode: Vec<String>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait VpnIkePolicyRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<VpnIkePolicy>, AppError>;
    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        filter: &VpnIkePolicyFilter,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<VpnIkePolicy>, i64), AppError>;
    async fn create(&self, item: &VpnIkePolicy) -> Result<(), AppError>;
    async fn update(&self, item: &VpnIkePolicy) -> Result<(), AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxVpnIkePolicyRepository {
    pool: PgPool,
}

impl SqlxVpnIkePolicyRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<VpnIkePolicy, AppError> {
        Ok(VpnIkePolicy {
            id: row.try_get("id").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_ikepolicy id: {}", e))
            })?,
            name: row.try_get("name").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_ikepolicy name: {}", e))
            })?,
            version: row.try_get("version").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_ikepolicy version: {}", e))
            })?,
            mode: row.try_get("mode").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_ikepolicy mode: {}", e))
            })?,
            preshared_key: row.try_get("preshared_key").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_ikepolicy preshared_key: {}", e))
            })?,
            description: row.try_get("description").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_ikepolicy description: {}", e))
            })?,
            comments: row.try_get("comments").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_ikepolicy comments: {}", e))
            })?,
            custom_fields: col(row, "custom_field_data")?,
        })
    }
}

impl VpnIkePolicyRepository for SqlxVpnIkePolicyRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<VpnIkePolicy>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, name, version, mode, preshared_key, description, comments, custom_field_data
            FROM vpn_ikepolicy
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load vpn_ikepolicy: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        filter: &VpnIkePolicyFilter,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<VpnIkePolicy>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT id, name, version, mode, preshared_key, description, comments, custom_field_data,
                   COUNT(*) OVER() AS total_count
            FROM vpn_ikepolicy
            "#,
        );
        push_vpn_ike_policy_filters(&mut builder, filter);
        push_order_by(&mut builder, VPN_IKE_POLICY_COLUMNS, sort.as_ref(), "name");
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows =
            builder.build().fetch_all(&self.pool).await.map_err(|e| {
                AppError::Database(format!("Failed to list vpn_ikepolicies: {}", e))
            })?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_ikepolicy total_count: {}", e))
            })?,
            None => count_vpn_ike_policies(&self.pool, filter).await?,
        };
        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<VpnIkePolicy>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, item: &VpnIkePolicy) -> Result<(), AppError> {
        sqlx::query(
            r#"
            INSERT INTO vpn_ikepolicy (
                id, custom_field_data, name, version, mode, preshared_key, description, comments
            )
            VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6, $7)
            "#,
        )
        .bind(item.id)
        .bind(&item.name)
        .bind(item.version)
        .bind(&item.mode)
        .bind(&item.preshared_key)
        .bind(&item.description)
        .bind(&item.comments)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create vpn_ikepolicy: {}", e)))?;
        Ok(())
    }

    async fn update(&self, item: &VpnIkePolicy) -> Result<(), AppError> {
        sqlx::query(
            r#"
            UPDATE vpn_ikepolicy
            SET name = $2, version = $3, mode = $4, preshared_key = $5,
                description = $6, comments = $7
            WHERE id = $1
            "#,
        )
        .bind(item.id)
        .bind(&item.name)
        .bind(item.version)
        .bind(&item.mode)
        .bind(&item.preshared_key)
        .bind(&item.description)
        .bind(&item.comments)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update vpn_ikepolicy: {}", e)))?;
        Ok(())
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "vpn_ikepolicy", id)
            .await
            .map(|_| ())
    }
}

fn push_vpn_ike_policy_filters(
    builder: &mut QueryBuilder<'_, sqlx::Postgres>,
    filter: &VpnIkePolicyFilter,
) {
    let mut where_state = WhereState::new();
    push_search_ilike(
        builder,
        &mut where_state,
        &["name", "version", "mode", "description", "comments"],
        filter.search.as_deref(),
    );
    push_string_in(builder, &mut where_state, "mode", &filter.mode);
}

async fn count_vpn_ike_policies(
    pool: &PgPool,
    filter: &VpnIkePolicyFilter,
) -> Result<i64, AppError> {
    let mut builder = QueryBuilder::new("SELECT COUNT(*) FROM vpn_ikepolicy");
    push_vpn_ike_policy_filters(&mut builder, filter);
    builder
        .build_query_scalar::<i64>()
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to count vpn_ikepolicies: {}", e)))
}
