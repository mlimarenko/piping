use sqlx::postgres::PgRow;
use sqlx::{PgPool, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::inventory::domain::VpnIpsecPolicy;
use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_search_ilike,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::contexts::shared::row::col;
use crate::errors::AppError;

pub const VPN_IPSEC_POLICY_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "name", "inventory.resources.fields.name"),
    ListColumn::sortable(
        "pfs_group",
        "pfs_group",
        "inventory.resources.fields.pfsGroup",
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
pub struct VpnIpsecPolicyFilter {
    pub search: Option<String>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait VpnIpsecPolicyRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<VpnIpsecPolicy>, AppError>;
    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        filter: &VpnIpsecPolicyFilter,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<VpnIpsecPolicy>, i64), AppError>;
    async fn create(&self, item: &VpnIpsecPolicy) -> Result<(), AppError>;
    async fn update(&self, item: &VpnIpsecPolicy) -> Result<(), AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxVpnIpsecPolicyRepository {
    pool: PgPool,
}

impl SqlxVpnIpsecPolicyRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<VpnIpsecPolicy, AppError> {
        Ok(VpnIpsecPolicy {
            id: row.try_get("id").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_ipsecpolicy id: {}", e))
            })?,
            name: row.try_get("name").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_ipsecpolicy name: {}", e))
            })?,
            pfs_group: row.try_get("pfs_group").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_ipsecpolicy pfs_group: {}", e))
            })?,
            description: row.try_get("description").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_ipsecpolicy description: {}", e))
            })?,
            comments: row.try_get("comments").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_ipsecpolicy comments: {}", e))
            })?,
            custom_fields: col(row, "custom_field_data")?,
        })
    }
}

impl VpnIpsecPolicyRepository for SqlxVpnIpsecPolicyRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<VpnIpsecPolicy>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, name, pfs_group, description, comments, custom_field_data
            FROM vpn_ipsecpolicy
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load vpn_ipsecpolicy: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        filter: &VpnIpsecPolicyFilter,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<VpnIpsecPolicy>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT id, name, pfs_group, description, comments, custom_field_data,
                   COUNT(*) OVER() AS total_count
            FROM vpn_ipsecpolicy
            "#,
        );
        push_vpn_ipsec_policy_filters(&mut builder, filter);
        push_order_by(
            &mut builder,
            VPN_IPSEC_POLICY_COLUMNS,
            sort.as_ref(),
            "name",
        );
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows =
            builder.build().fetch_all(&self.pool).await.map_err(|e| {
                AppError::Database(format!("Failed to list vpn_ipsecpolicies: {}", e))
            })?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_ipsecpolicy total_count: {}", e))
            })?,
            None => count_vpn_ipsec_policies(&self.pool, filter).await?,
        };
        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<VpnIpsecPolicy>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, item: &VpnIpsecPolicy) -> Result<(), AppError> {
        sqlx::query(
            r#"
            INSERT INTO vpn_ipsecpolicy (
                id, custom_field_data, name, pfs_group, description, comments
            )
            VALUES ($1, '{}'::jsonb, $2, $3, $4, $5)
            "#,
        )
        .bind(item.id)
        .bind(&item.name)
        .bind(item.pfs_group)
        .bind(&item.description)
        .bind(&item.comments)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create vpn_ipsecpolicy: {}", e)))?;
        Ok(())
    }

    async fn update(&self, item: &VpnIpsecPolicy) -> Result<(), AppError> {
        sqlx::query(
            r#"
            UPDATE vpn_ipsecpolicy
            SET name = $2, pfs_group = $3, description = $4, comments = $5
            WHERE id = $1
            "#,
        )
        .bind(item.id)
        .bind(&item.name)
        .bind(item.pfs_group)
        .bind(&item.description)
        .bind(&item.comments)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update vpn_ipsecpolicy: {}", e)))?;
        Ok(())
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "vpn_ipsecpolicy", id)
            .await
            .map(|_| ())
    }
}

fn push_vpn_ipsec_policy_filters(
    builder: &mut QueryBuilder<'_, sqlx::Postgres>,
    filter: &VpnIpsecPolicyFilter,
) {
    let mut where_state = WhereState::new();
    push_search_ilike(
        builder,
        &mut where_state,
        &["name", "pfs_group", "description", "comments"],
        filter.search.as_deref(),
    );
}

async fn count_vpn_ipsec_policies(
    pool: &PgPool,
    filter: &VpnIpsecPolicyFilter,
) -> Result<i64, AppError> {
    let mut builder = QueryBuilder::new("SELECT COUNT(*) FROM vpn_ipsecpolicy");
    push_vpn_ipsec_policy_filters(&mut builder, filter);
    builder
        .build_query_scalar::<i64>()
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to count vpn_ipsecpolicies: {}", e)))
}
