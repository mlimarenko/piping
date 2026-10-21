use sqlx::postgres::PgRow;
use sqlx::{PgPool, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::inventory::domain::VpnTunnelTermination;
use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_search_ilike, push_string_in,
    push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::contexts::shared::row::col;
use crate::errors::AppError;

pub const VPN_TUNNEL_TERMINATION_COLUMNS: &[ListColumn] = &[
    ListColumn::enum_filter(
        "role",
        "role",
        "role",
        "inventory.tunnelTerminations.columns.role",
    ),
    ListColumn::fk_filter(
        "tunnel_id",
        "tunnel_id",
        "tunnel_id",
        "tunnel_id",
        "tunnels",
        "inventory.tunnelTerminations.columns.tunnel",
    ),
    ListColumn::fk_filter(
        "termination_type_id",
        "termination_type_id",
        "termination_type_id",
        "termination_type_id",
        "contentTypes",
        "inventory.tunnelTerminations.columns.terminationType",
    ),
    ListColumn::fk_filter(
        "termination_id",
        "termination_id",
        "termination_id",
        "termination_id",
        "objects",
        "inventory.tunnelTerminations.columns.termination",
    ),
    ListColumn::fk_filter(
        "outside_ip_id",
        "outside_ip_id",
        "outside_ip_id",
        "outside_ip_id",
        "ipAddresses",
        "inventory.tunnelTerminations.columns.outsideIp",
    ),
];

#[derive(Debug, Clone, Default)]
pub struct VpnTunnelTerminationFilter {
    pub search: Option<String>,
    pub role: Vec<String>,
    pub tunnel_id: Vec<Uuid>,
    pub termination_type_id: Vec<Uuid>,
    pub termination_id: Vec<Uuid>,
    pub outside_ip_id: Vec<Uuid>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait VpnTunnelTerminationRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<VpnTunnelTermination>, AppError>;
    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        filter: &VpnTunnelTerminationFilter,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<VpnTunnelTermination>, i64), AppError>;
    async fn create(&self, item: &VpnTunnelTermination) -> Result<(), AppError>;
    async fn update(&self, item: &VpnTunnelTermination) -> Result<(), AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxVpnTunnelTerminationRepository {
    pool: PgPool,
}

impl SqlxVpnTunnelTerminationRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<VpnTunnelTermination, AppError> {
        Ok(VpnTunnelTermination {
            id: row.try_get("id").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_tunneltermination id: {}", e))
            })?,
            role: row.try_get("role").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_tunneltermination role: {}", e))
            })?,
            tunnel_id: row.try_get("tunnel_id").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read vpn_tunneltermination tunnel_id: {}",
                    e
                ))
            })?,
            tunnel_name: col(row, "tunnel_name")?,
            termination_type_id: row.try_get("termination_type_id").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read vpn_tunneltermination termination_type_id: {}",
                    e
                ))
            })?,
            termination_id: row.try_get("termination_id").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read vpn_tunneltermination termination_id: {}",
                    e
                ))
            })?,
            outside_ip_id: row.try_get("outside_ip_id").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read vpn_tunneltermination outside_ip_id: {}",
                    e
                ))
            })?,
            outside_ip: col(row, "outside_ip")?,
            custom_fields: col(row, "custom_field_data")?,
        })
    }
}

impl VpnTunnelTerminationRepository for SqlxVpnTunnelTerminationRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<VpnTunnelTermination>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, role, tunnel_id, NULL::text AS tunnel_name,
                   termination_type_id, termination_id, outside_ip_id, NULL::text AS outside_ip,
                   custom_field_data
            FROM vpn_tunneltermination
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load vpn_tunneltermination: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        filter: &VpnTunnelTerminationFilter,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<VpnTunnelTermination>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT id, role, tunnel_id, tunnel_name, termination_type_id, termination_id,
                   outside_ip_id, outside_ip, custom_field_data,
                   COUNT(*) OVER() AS total_count
            FROM (
                SELECT vtt.id, vtt.role, vtt.tunnel_id, t.name AS tunnel_name,
                       vtt.termination_type_id, vtt.termination_id, vtt.outside_ip_id,
                       host(ip.address) AS outside_ip, vtt.custom_field_data
                FROM vpn_tunneltermination vtt
                JOIN vpn_tunnel t ON t.id = vtt.tunnel_id
                LEFT JOIN ipam_ipaddress ip ON ip.id = vtt.outside_ip_id
            ) tunnel_termination_rows
            "#,
        );
        push_vpn_tunnel_termination_filters(&mut builder, filter);
        push_order_by(
            &mut builder,
            VPN_TUNNEL_TERMINATION_COLUMNS,
            sort.as_ref(),
            "id",
        );
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder.build().fetch_all(&self.pool).await.map_err(|e| {
            AppError::Database(format!("Failed to list vpn_tunnelterminations: {}", e))
        })?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read vpn_tunneltermination total_count: {}",
                    e
                ))
            })?,
            None => count_vpn_tunnel_terminations(&self.pool, filter).await?,
        };
        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<VpnTunnelTermination>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, item: &VpnTunnelTermination) -> Result<(), AppError> {
        sqlx::query(
            r#"
            INSERT INTO vpn_tunneltermination (
                id, custom_field_data, role, tunnel_id, termination_type_id, termination_id,
                outside_ip_id
            )
            VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(item.id)
        .bind(&item.role)
        .bind(item.tunnel_id)
        .bind(item.termination_type_id)
        .bind(item.termination_id)
        .bind(item.outside_ip_id)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            AppError::Database(format!("Failed to create vpn_tunneltermination: {}", e))
        })?;
        Ok(())
    }

    async fn update(&self, item: &VpnTunnelTermination) -> Result<(), AppError> {
        sqlx::query(
            r#"
            UPDATE vpn_tunneltermination
            SET role = $2, tunnel_id = $3, termination_type_id = $4, termination_id = $5,
                outside_ip_id = $6
            WHERE id = $1
            "#,
        )
        .bind(item.id)
        .bind(&item.role)
        .bind(item.tunnel_id)
        .bind(item.termination_type_id)
        .bind(item.termination_id)
        .bind(item.outside_ip_id)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            AppError::Database(format!("Failed to update vpn_tunneltermination: {}", e))
        })?;
        Ok(())
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "vpn_tunneltermination", id)
            .await
            .map(|_| ())
    }
}

fn push_vpn_tunnel_termination_filters(
    builder: &mut QueryBuilder<'_, sqlx::Postgres>,
    filter: &VpnTunnelTerminationFilter,
) {
    let mut where_state = WhereState::new();
    push_search_ilike(
        builder,
        &mut where_state,
        &[
            "role",
            "tunnel_id",
            "termination_type_id",
            "termination_id",
            "outside_ip_id",
        ],
        filter.search.as_deref(),
    );
    push_string_in(builder, &mut where_state, "role", &filter.role);
    push_uuid_in(builder, &mut where_state, "tunnel_id", &filter.tunnel_id);
    push_uuid_in(
        builder,
        &mut where_state,
        "termination_type_id",
        &filter.termination_type_id,
    );
    push_uuid_in(
        builder,
        &mut where_state,
        "termination_id",
        &filter.termination_id,
    );
    push_uuid_in(
        builder,
        &mut where_state,
        "outside_ip_id",
        &filter.outside_ip_id,
    );
}

async fn count_vpn_tunnel_terminations(
    pool: &PgPool,
    filter: &VpnTunnelTerminationFilter,
) -> Result<i64, AppError> {
    let mut builder = QueryBuilder::new("SELECT COUNT(*) FROM vpn_tunneltermination");
    push_vpn_tunnel_termination_filters(&mut builder, filter);
    builder
        .build_query_scalar::<i64>()
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to count vpn_tunnelterminations: {}", e)))
}
