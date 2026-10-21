use sqlx::postgres::PgRow;
use sqlx::{PgPool, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::inventory::domain::VpnIpsecProposal;
use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_search_ilike, push_string_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::contexts::shared::row::col;
use crate::errors::AppError;

pub const VPN_IPSEC_PROPOSAL_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "name", "inventory.resources.fields.name"),
    ListColumn::enum_filter(
        "encryption_algorithm",
        "encryption_algorithm",
        "encryption_algorithm",
        "inventory.resources.fields.encryptionAlgorithm",
    ),
    ListColumn::enum_filter(
        "authentication_algorithm",
        "authentication_algorithm",
        "authentication_algorithm",
        "inventory.resources.fields.authenticationAlgorithm",
    ),
    ListColumn::sortable(
        "sa_lifetime_seconds",
        "sa_lifetime_seconds",
        "inventory.resources.fields.saLifetimeSeconds",
    ),
    ListColumn::sortable(
        "sa_lifetime_data",
        "sa_lifetime_data",
        "inventory.resources.fields.saLifetimeData",
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
pub struct VpnIpsecProposalFilter {
    pub search: Option<String>,
    pub encryption_algorithm: Vec<String>,
    pub authentication_algorithm: Vec<String>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait VpnIpsecProposalRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<VpnIpsecProposal>, AppError>;
    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        filter: &VpnIpsecProposalFilter,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<VpnIpsecProposal>, i64), AppError>;
    async fn create(&self, item: &VpnIpsecProposal) -> Result<(), AppError>;
    async fn update(&self, item: &VpnIpsecProposal) -> Result<(), AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxVpnIpsecProposalRepository {
    pool: PgPool,
}

impl SqlxVpnIpsecProposalRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<VpnIpsecProposal, AppError> {
        Ok(VpnIpsecProposal {
            id: row.try_get("id").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_ipsecproposal id: {}", e))
            })?,
            name: row.try_get("name").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_ipsecproposal name: {}", e))
            })?,
            encryption_algorithm: row.try_get("encryption_algorithm").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read vpn_ipsecproposal encryption_algorithm: {}",
                    e
                ))
            })?,
            authentication_algorithm: row.try_get("authentication_algorithm").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read vpn_ipsecproposal authentication_algorithm: {}",
                    e
                ))
            })?,
            sa_lifetime_seconds: row.try_get("sa_lifetime_seconds").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read vpn_ipsecproposal sa_lifetime_seconds: {}",
                    e
                ))
            })?,
            sa_lifetime_data: row.try_get("sa_lifetime_data").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read vpn_ipsecproposal sa_lifetime_data: {}",
                    e
                ))
            })?,
            description: row.try_get("description").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read vpn_ipsecproposal description: {}",
                    e
                ))
            })?,
            comments: row.try_get("comments").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_ipsecproposal comments: {}", e))
            })?,
            custom_fields: col(row, "custom_field_data")?,
        })
    }
}

impl VpnIpsecProposalRepository for SqlxVpnIpsecProposalRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<VpnIpsecProposal>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, name, encryption_algorithm, authentication_algorithm,
                   sa_lifetime_seconds, sa_lifetime_data, description, comments, custom_field_data
            FROM vpn_ipsecproposal
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load vpn_ipsecproposal: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        filter: &VpnIpsecProposalFilter,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<VpnIpsecProposal>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT id, name, encryption_algorithm, authentication_algorithm,
                   sa_lifetime_seconds, sa_lifetime_data, description, comments, custom_field_data,
                   COUNT(*) OVER() AS total_count
            FROM vpn_ipsecproposal
            "#,
        );
        push_vpn_ipsec_proposal_filters(&mut builder, filter);
        push_order_by(
            &mut builder,
            VPN_IPSEC_PROPOSAL_COLUMNS,
            sort.as_ref(),
            "name",
        );
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows =
            builder.build().fetch_all(&self.pool).await.map_err(|e| {
                AppError::Database(format!("Failed to list vpn_ipsecproposals: {}", e))
            })?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read vpn_ipsecproposal total_count: {}",
                    e
                ))
            })?,
            None => count_vpn_ipsec_proposals(&self.pool, filter).await?,
        };
        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<VpnIpsecProposal>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, item: &VpnIpsecProposal) -> Result<(), AppError> {
        sqlx::query(
            r#"
            INSERT INTO vpn_ipsecproposal (
                id, custom_field_data, name, encryption_algorithm, authentication_algorithm,
                sa_lifetime_seconds, sa_lifetime_data, description, comments
            )
            VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6, $7, $8)
            "#,
        )
        .bind(item.id)
        .bind(&item.name)
        .bind(&item.encryption_algorithm)
        .bind(&item.authentication_algorithm)
        .bind(item.sa_lifetime_seconds)
        .bind(item.sa_lifetime_data)
        .bind(&item.description)
        .bind(&item.comments)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create vpn_ipsecproposal: {}", e)))?;
        Ok(())
    }

    async fn update(&self, item: &VpnIpsecProposal) -> Result<(), AppError> {
        sqlx::query(
            r#"
            UPDATE vpn_ipsecproposal
            SET name = $2, encryption_algorithm = $3, authentication_algorithm = $4,
                sa_lifetime_seconds = $5, sa_lifetime_data = $6, description = $7, comments = $8
            WHERE id = $1
            "#,
        )
        .bind(item.id)
        .bind(&item.name)
        .bind(&item.encryption_algorithm)
        .bind(&item.authentication_algorithm)
        .bind(item.sa_lifetime_seconds)
        .bind(item.sa_lifetime_data)
        .bind(&item.description)
        .bind(&item.comments)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update vpn_ipsecproposal: {}", e)))?;
        Ok(())
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "vpn_ipsecproposal", id)
            .await
            .map(|_| ())
    }
}

fn push_vpn_ipsec_proposal_filters(
    builder: &mut QueryBuilder<'_, sqlx::Postgres>,
    filter: &VpnIpsecProposalFilter,
) {
    let mut where_state = WhereState::new();
    push_search_ilike(
        builder,
        &mut where_state,
        &[
            "name",
            "encryption_algorithm",
            "authentication_algorithm",
            "sa_lifetime_seconds",
            "sa_lifetime_data",
            "description",
            "comments",
        ],
        filter.search.as_deref(),
    );
    push_string_in(
        builder,
        &mut where_state,
        "encryption_algorithm",
        &filter.encryption_algorithm,
    );
    push_string_in(
        builder,
        &mut where_state,
        "authentication_algorithm",
        &filter.authentication_algorithm,
    );
}

async fn count_vpn_ipsec_proposals(
    pool: &PgPool,
    filter: &VpnIpsecProposalFilter,
) -> Result<i64, AppError> {
    let mut builder = QueryBuilder::new("SELECT COUNT(*) FROM vpn_ipsecproposal");
    push_vpn_ipsec_proposal_filters(&mut builder, filter);
    builder
        .build_query_scalar::<i64>()
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to count vpn_ipsecproposals: {}", e)))
}
