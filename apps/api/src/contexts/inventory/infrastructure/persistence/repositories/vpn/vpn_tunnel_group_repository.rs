use sqlx::postgres::PgRow;
use sqlx::{PgPool, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::inventory::domain::VpnTunnelGroup;
use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_search_ilike,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::contexts::shared::row::col;
use crate::errors::AppError;

pub const VPN_TUNNEL_GROUP_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "name", "inventory.resources.fields.name"),
    ListColumn::sortable("slug", "slug", "inventory.resources.fields.slug"),
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

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait VpnTunnelGroupRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<VpnTunnelGroup>, AppError>;
    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        search: Option<String>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<VpnTunnelGroup>, i64), AppError>;
    async fn create(&self, item: &VpnTunnelGroup) -> Result<(), AppError>;
    async fn update(&self, item: &VpnTunnelGroup) -> Result<(), AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxVpnTunnelGroupRepository {
    pool: PgPool,
}

impl SqlxVpnTunnelGroupRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<VpnTunnelGroup, AppError> {
        Ok(VpnTunnelGroup {
            id: row.try_get("id").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_tunnelgroup id: {}", e))
            })?,
            name: row.try_get("name").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_tunnelgroup name: {}", e))
            })?,
            slug: row.try_get("slug").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_tunnelgroup slug: {}", e))
            })?,
            description: row.try_get("description").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_tunnelgroup description: {}", e))
            })?,
            comments: row.try_get("comments").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_tunnelgroup comments: {}", e))
            })?,
            custom_fields: col(row, "custom_field_data")?,
        })
    }
}

impl VpnTunnelGroupRepository for SqlxVpnTunnelGroupRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<VpnTunnelGroup>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, name, slug, description, comments, custom_field_data
            FROM vpn_tunnelgroup
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load vpn_tunnelgroup: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        search: Option<String>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<VpnTunnelGroup>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT id, name, slug, description, comments, custom_field_data,
                   COUNT(*) OVER() AS total_count
            FROM vpn_tunnelgroup
            "#,
        );
        push_vpn_tunnel_group_filters(&mut builder, search.as_deref());
        push_order_by(
            &mut builder,
            VPN_TUNNEL_GROUP_COLUMNS,
            sort.as_ref(),
            "name",
        );
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows =
            builder.build().fetch_all(&self.pool).await.map_err(|e| {
                AppError::Database(format!("Failed to list vpn_tunnelgroups: {}", e))
            })?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_tunnelgroup total_count: {}", e))
            })?,
            None => count_vpn_tunnel_groups(&self.pool, search.as_deref()).await?,
        };
        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<VpnTunnelGroup>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, item: &VpnTunnelGroup) -> Result<(), AppError> {
        sqlx::query(
            r#"
            INSERT INTO vpn_tunnelgroup (
                id, custom_field_data, name, slug, description, comments
            )
            VALUES ($1, '{}'::jsonb, $2, $3, $4, $5)
            "#,
        )
        .bind(item.id)
        .bind(&item.name)
        .bind(&item.slug)
        .bind(&item.description)
        .bind(&item.comments)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create vpn_tunnelgroup: {}", e)))?;
        Ok(())
    }

    async fn update(&self, item: &VpnTunnelGroup) -> Result<(), AppError> {
        sqlx::query(
            r#"
            UPDATE vpn_tunnelgroup
            SET name = $2, slug = $3, description = $4, comments = $5
            WHERE id = $1
            "#,
        )
        .bind(item.id)
        .bind(&item.name)
        .bind(&item.slug)
        .bind(&item.description)
        .bind(&item.comments)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update vpn_tunnelgroup: {}", e)))?;
        Ok(())
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "vpn_tunnelgroup", id)
            .await
            .map(|_| ())
    }
}

fn push_vpn_tunnel_group_filters(
    builder: &mut QueryBuilder<'_, sqlx::Postgres>,
    search: Option<&str>,
) {
    let mut where_state = WhereState::new();
    push_search_ilike(
        builder,
        &mut where_state,
        &["name", "slug", "description", "comments"],
        search,
    );
}

async fn count_vpn_tunnel_groups(pool: &PgPool, search: Option<&str>) -> Result<i64, AppError> {
    let mut builder = QueryBuilder::new("SELECT COUNT(*) FROM vpn_tunnelgroup");
    push_vpn_tunnel_group_filters(&mut builder, search);
    builder
        .build_query_scalar::<i64>()
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to count vpn_tunnelgroups: {}", e)))
}
