use sqlx::postgres::PgRow;
use sqlx::{PgPool, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::inventory::domain::VpnIpsecProfile;
use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_search_ilike, push_string_in,
    push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::contexts::shared::row::col;
use crate::errors::AppError;

pub const VPN_IPSEC_PROFILE_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "name", "inventory.resources.fields.name"),
    ListColumn::enum_filter("mode", "mode", "mode", "inventory.resources.fields.vpnMode"),
    ListColumn::fk_filter(
        "ike_policy",
        "ike_policy_id",
        "ike_policy_id",
        "ike_policy_id",
        "IKE Policies",
        "inventory.resources.fields.ikePolicy",
    ),
    ListColumn::fk_filter(
        "ipsec_policy",
        "ipsec_policy_id",
        "ipsec_policy_id",
        "ipsec_policy_id",
        "IPsec Policies",
        "inventory.resources.fields.ipsecPolicy",
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
pub struct VpnIpsecProfileFilter {
    pub search: Option<String>,
    pub mode: Vec<String>,
    pub ike_policy_id: Vec<Uuid>,
    pub ipsec_policy_id: Vec<Uuid>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait VpnIpsecProfileRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<VpnIpsecProfile>, AppError>;
    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        filter: &VpnIpsecProfileFilter,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<VpnIpsecProfile>, i64), AppError>;
    async fn create(&self, item: &VpnIpsecProfile) -> Result<(), AppError>;
    async fn update(&self, item: &VpnIpsecProfile) -> Result<(), AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxVpnIpsecProfileRepository {
    pool: PgPool,
}

impl SqlxVpnIpsecProfileRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<VpnIpsecProfile, AppError> {
        Ok(VpnIpsecProfile {
            id: row.try_get("id").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_ipsecprofile id: {}", e))
            })?,
            name: row.try_get("name").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_ipsecprofile name: {}", e))
            })?,
            mode: row.try_get("mode").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_ipsecprofile mode: {}", e))
            })?,
            ike_policy_id: row.try_get("ike_policy_id").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read vpn_ipsecprofile ike_policy_id: {}",
                    e
                ))
            })?,
            ipsec_policy_id: row.try_get("ipsec_policy_id").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read vpn_ipsecprofile ipsec_policy_id: {}",
                    e
                ))
            })?,
            description: row.try_get("description").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read vpn_ipsecprofile description: {}",
                    e
                ))
            })?,
            comments: row.try_get("comments").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_ipsecprofile comments: {}", e))
            })?,
            custom_fields: col(row, "custom_field_data")?,
        })
    }
}

impl VpnIpsecProfileRepository for SqlxVpnIpsecProfileRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<VpnIpsecProfile>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, name, mode, ike_policy_id, ipsec_policy_id, description, comments,
                   custom_field_data
            FROM vpn_ipsecprofile
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load vpn_ipsecprofile: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        filter: &VpnIpsecProfileFilter,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<VpnIpsecProfile>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT id, name, mode, ike_policy_id, ipsec_policy_id, description, comments,
                   custom_field_data,
                   COUNT(*) OVER() AS total_count
            FROM vpn_ipsecprofile
            "#,
        );
        push_vpn_ipsec_profile_filters(&mut builder, filter);
        push_order_by(
            &mut builder,
            VPN_IPSEC_PROFILE_COLUMNS,
            sort.as_ref(),
            "name",
        );
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows =
            builder.build().fetch_all(&self.pool).await.map_err(|e| {
                AppError::Database(format!("Failed to list vpn_ipsecprofiles: {}", e))
            })?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read vpn_ipsecprofile total_count: {}",
                    e
                ))
            })?,
            None => count_vpn_ipsec_profiles(&self.pool, filter).await?,
        };
        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<VpnIpsecProfile>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, item: &VpnIpsecProfile) -> Result<(), AppError> {
        sqlx::query(
            r#"
            INSERT INTO vpn_ipsecprofile (
                id, custom_field_data, name, mode, ike_policy_id, ipsec_policy_id,
                description, comments
            )
            VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6, $7)
            "#,
        )
        .bind(item.id)
        .bind(&item.name)
        .bind(&item.mode)
        .bind(item.ike_policy_id)
        .bind(item.ipsec_policy_id)
        .bind(&item.description)
        .bind(&item.comments)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create vpn_ipsecprofile: {}", e)))?;
        Ok(())
    }

    async fn update(&self, item: &VpnIpsecProfile) -> Result<(), AppError> {
        sqlx::query(
            r#"
            UPDATE vpn_ipsecprofile
            SET name = $2, mode = $3, ike_policy_id = $4, ipsec_policy_id = $5,
                description = $6, comments = $7
            WHERE id = $1
            "#,
        )
        .bind(item.id)
        .bind(&item.name)
        .bind(&item.mode)
        .bind(item.ike_policy_id)
        .bind(item.ipsec_policy_id)
        .bind(&item.description)
        .bind(&item.comments)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update vpn_ipsecprofile: {}", e)))?;
        Ok(())
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "vpn_ipsecprofile", id)
            .await
            .map(|_| ())
    }
}

fn push_vpn_ipsec_profile_filters(
    builder: &mut QueryBuilder<'_, sqlx::Postgres>,
    filter: &VpnIpsecProfileFilter,
) {
    let mut where_state = WhereState::new();
    push_search_ilike(
        builder,
        &mut where_state,
        &["name", "mode", "description", "comments"],
        filter.search.as_deref(),
    );
    push_string_in(builder, &mut where_state, "mode", &filter.mode);
    push_uuid_in(
        builder,
        &mut where_state,
        "ike_policy_id",
        &filter.ike_policy_id,
    );
    push_uuid_in(
        builder,
        &mut where_state,
        "ipsec_policy_id",
        &filter.ipsec_policy_id,
    );
}

async fn count_vpn_ipsec_profiles(
    pool: &PgPool,
    filter: &VpnIpsecProfileFilter,
) -> Result<i64, AppError> {
    let mut builder = QueryBuilder::new("SELECT COUNT(*) FROM vpn_ipsecprofile");
    push_vpn_ipsec_profile_filters(&mut builder, filter);
    builder
        .build_query_scalar::<i64>()
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to count vpn_ipsecprofiles: {}", e)))
}
