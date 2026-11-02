use sqlx::postgres::PgRow;
use sqlx::{PgPool, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::inventory::domain::VpnTunnel;
use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_search_ilike, push_string_in,
    push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::contexts::shared::row::col;
use crate::errors::AppError;

pub const VPN_TUNNEL_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "name", "inventory.tunnels.columns.name"),
    ListColumn::enum_filter(
        "status",
        "status",
        "status",
        "inventory.tunnels.columns.status",
    ),
    ListColumn::enum_filter(
        "encapsulation",
        "encapsulation",
        "encapsulation",
        "inventory.tunnels.columns.encapsulation",
    ),
    ListColumn::sortable(
        "description",
        "description",
        "inventory.tunnels.columns.description",
    ),
    ListColumn::sortable("comments", "comments", "inventory.tunnels.columns.comments"),
    ListColumn::fk_filter(
        "group_id",
        "group_id",
        "group_id",
        "group_id",
        "tunnelGroups",
        "inventory.tunnels.columns.group",
    ),
    ListColumn::fk_filter(
        "ipsec_profile_id",
        "ipsec_profile_id",
        "ipsec_profile_id",
        "ipsec_profile_id",
        "ipsecProfiles",
        "inventory.tunnels.columns.ipsecProfile",
    ),
    ListColumn::sortable(
        "tunnel_id",
        "tunnel_id",
        "inventory.tunnels.columns.tunnelId",
    ),
    ListColumn::fk_filter(
        "tenant_id",
        "tenant_id",
        "tenant_id",
        "tenant_id",
        "tenants",
        "inventory.tunnels.columns.tenant",
    ),
];

#[derive(Debug, Clone, Default)]
pub struct VpnTunnelFilter {
    pub search: Option<String>,
    pub status: Vec<String>,
    pub encapsulation: Vec<String>,
    pub group_id: Vec<Uuid>,
    pub ipsec_profile_id: Vec<Uuid>,
    pub tenant_id: Vec<Uuid>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait VpnTunnelRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<VpnTunnel>, AppError>;
    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        filter: &VpnTunnelFilter,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<VpnTunnel>, i64), AppError>;
    async fn create(&self, item: &VpnTunnel) -> Result<(), AppError>;
    async fn update(&self, item: &VpnTunnel) -> Result<(), AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxVpnTunnelRepository {
    pool: PgPool,
}

impl SqlxVpnTunnelRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<VpnTunnel, AppError> {
        Ok(VpnTunnel {
            id: row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read vpn_tunnel id: {}", e)))?,
            name: row.try_get("name").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_tunnel name: {}", e))
            })?,
            status: row.try_get("status").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_tunnel status: {}", e))
            })?,
            encapsulation: row.try_get("encapsulation").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_tunnel encapsulation: {}", e))
            })?,
            description: row.try_get("description").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_tunnel description: {}", e))
            })?,
            comments: row.try_get("comments").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_tunnel comments: {}", e))
            })?,
            group_id: row.try_get("group_id").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_tunnel group_id: {}", e))
            })?,
            ipsec_profile_id: row.try_get("ipsec_profile_id").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_tunnel ipsec_profile_id: {}", e))
            })?,
            tunnel_id: row.try_get("tunnel_id").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_tunnel tunnel_id: {}", e))
            })?,
            tenant_id: row.try_get("tenant_id").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_tunnel tenant_id: {}", e))
            })?,
            custom_fields: col(row, "custom_field_data")?,
        })
    }
}

impl VpnTunnelRepository for SqlxVpnTunnelRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<VpnTunnel>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, name, status, encapsulation, description, comments, group_id,
                   ipsec_profile_id, tunnel_id, tenant_id, custom_field_data
            FROM vpn_tunnel
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load vpn_tunnel: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        filter: &VpnTunnelFilter,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<VpnTunnel>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT id, name, status, encapsulation, description, comments, group_id,
                   ipsec_profile_id, tunnel_id, tenant_id, custom_field_data,
                   COUNT(*) OVER() AS total_count
            FROM vpn_tunnel
            "#,
        );
        push_vpn_tunnel_filters(&mut builder, filter);
        push_order_by(&mut builder, VPN_TUNNEL_COLUMNS, sort.as_ref(), "name");
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list vpn_tunnels: {}", e)))?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_tunnel total_count: {}", e))
            })?,
            None => count_vpn_tunnels(&self.pool, filter).await?,
        };
        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<VpnTunnel>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, item: &VpnTunnel) -> Result<(), AppError> {
        sqlx::query(
            r#"
            INSERT INTO vpn_tunnel (
                id, custom_field_data, name, status, encapsulation, description, comments,
                group_id, ipsec_profile_id, tunnel_id, tenant_id
            )
            VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            "#,
        )
        .bind(item.id)
        .bind(&item.name)
        .bind(&item.status)
        .bind(&item.encapsulation)
        .bind(&item.description)
        .bind(&item.comments)
        .bind(item.group_id)
        .bind(item.ipsec_profile_id)
        .bind(item.tunnel_id)
        .bind(item.tenant_id)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create vpn_tunnel: {}", e)))?;
        Ok(())
    }

    async fn update(&self, item: &VpnTunnel) -> Result<(), AppError> {
        sqlx::query(
            r#"
            UPDATE vpn_tunnel
            SET name = $2, status = $3, encapsulation = $4, description = $5, comments = $6,
                group_id = $7, ipsec_profile_id = $8, tunnel_id = $9, tenant_id = $10
            WHERE id = $1
            "#,
        )
        .bind(item.id)
        .bind(&item.name)
        .bind(&item.status)
        .bind(&item.encapsulation)
        .bind(&item.description)
        .bind(&item.comments)
        .bind(item.group_id)
        .bind(item.ipsec_profile_id)
        .bind(item.tunnel_id)
        .bind(item.tenant_id)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update vpn_tunnel: {}", e)))?;
        Ok(())
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "vpn_tunnel", id)
            .await
            .map(|_| ())
    }
}

fn push_vpn_tunnel_filters(
    builder: &mut QueryBuilder<'_, sqlx::Postgres>,
    filter: &VpnTunnelFilter,
) {
    let mut where_state = WhereState::new();
    push_search_ilike(
        builder,
        &mut where_state,
        &[
            "name",
            "status",
            "encapsulation",
            "description",
            "comments",
            "tunnel_id",
        ],
        filter.search.as_deref(),
    );
    push_string_in(builder, &mut where_state, "status", &filter.status);
    push_string_in(
        builder,
        &mut where_state,
        "encapsulation",
        &filter.encapsulation,
    );
    push_uuid_in(builder, &mut where_state, "group_id", &filter.group_id);
    push_uuid_in(
        builder,
        &mut where_state,
        "ipsec_profile_id",
        &filter.ipsec_profile_id,
    );
    push_uuid_in(builder, &mut where_state, "tenant_id", &filter.tenant_id);
}

async fn count_vpn_tunnels(pool: &PgPool, filter: &VpnTunnelFilter) -> Result<i64, AppError> {
    let mut builder = QueryBuilder::new("SELECT COUNT(*) FROM vpn_tunnel");
    push_vpn_tunnel_filters(&mut builder, filter);
    builder
        .build_query_scalar::<i64>()
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to count vpn_tunnels: {}", e)))
}
