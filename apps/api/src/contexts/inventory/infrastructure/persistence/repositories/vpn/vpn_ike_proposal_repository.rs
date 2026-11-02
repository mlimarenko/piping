use sqlx::postgres::PgRow;
use sqlx::{PgPool, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::inventory::domain::VpnIkeProposal;
use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_search_ilike, push_string_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::contexts::shared::row::col;
use crate::errors::AppError;

pub const VPN_IKE_PROPOSAL_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "name", "inventory.resources.fields.name"),
    ListColumn::enum_filter(
        "authentication_method",
        "authentication_method",
        "authentication_method",
        "inventory.resources.fields.authenticationMethod",
    ),
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
    ListColumn::sortable("group", "\"group\"", "inventory.resources.fields.ikeGroup"),
    ListColumn::sortable(
        "sa_lifetime",
        "sa_lifetime",
        "inventory.resources.fields.saLifetime",
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
pub struct VpnIkeProposalFilter {
    pub search: Option<String>,
    pub authentication_method: Vec<String>,
    pub encryption_algorithm: Vec<String>,
    pub authentication_algorithm: Vec<String>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait VpnIkeProposalRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<VpnIkeProposal>, AppError>;
    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        filter: &VpnIkeProposalFilter,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<VpnIkeProposal>, i64), AppError>;
    async fn create(&self, item: &VpnIkeProposal) -> Result<(), AppError>;
    async fn update(&self, item: &VpnIkeProposal) -> Result<(), AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxVpnIkeProposalRepository {
    pool: PgPool,
}

impl SqlxVpnIkeProposalRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<VpnIkeProposal, AppError> {
        Ok(VpnIkeProposal {
            id: row.try_get("id").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_ikeproposal id: {}", e))
            })?,
            name: row.try_get("name").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_ikeproposal name: {}", e))
            })?,
            authentication_method: row.try_get("authentication_method").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read vpn_ikeproposal authentication_method: {}",
                    e
                ))
            })?,
            encryption_algorithm: row.try_get("encryption_algorithm").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read vpn_ikeproposal encryption_algorithm: {}",
                    e
                ))
            })?,
            authentication_algorithm: row.try_get("authentication_algorithm").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read vpn_ikeproposal authentication_algorithm: {}",
                    e
                ))
            })?,
            group: row.try_get("group").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_ikeproposal group: {}", e))
            })?,
            sa_lifetime: row.try_get("sa_lifetime").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_ikeproposal sa_lifetime: {}", e))
            })?,
            description: row.try_get("description").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_ikeproposal description: {}", e))
            })?,
            comments: row.try_get("comments").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_ikeproposal comments: {}", e))
            })?,
            custom_fields: col(row, "custom_field_data")?,
        })
    }
}

impl VpnIkeProposalRepository for SqlxVpnIkeProposalRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<VpnIkeProposal>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, name, authentication_method, encryption_algorithm,
                   authentication_algorithm, "group", sa_lifetime, description, comments,
                   custom_field_data
            FROM vpn_ikeproposal
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load vpn_ikeproposal: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        filter: &VpnIkeProposalFilter,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<VpnIkeProposal>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT id, name, authentication_method, encryption_algorithm,
                   authentication_algorithm, "group", sa_lifetime, description, comments,
                   custom_field_data,
                   COUNT(*) OVER() AS total_count
            FROM vpn_ikeproposal
            "#,
        );
        push_vpn_ike_proposal_filters(&mut builder, filter);
        push_order_by(
            &mut builder,
            VPN_IKE_PROPOSAL_COLUMNS,
            sort.as_ref(),
            "name",
        );
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows =
            builder.build().fetch_all(&self.pool).await.map_err(|e| {
                AppError::Database(format!("Failed to list vpn_ikeproposals: {}", e))
            })?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!("Failed to read vpn_ikeproposal total_count: {}", e))
            })?,
            None => count_vpn_ike_proposals(&self.pool, filter).await?,
        };
        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<VpnIkeProposal>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, item: &VpnIkeProposal) -> Result<(), AppError> {
        sqlx::query(
            r#"
            INSERT INTO vpn_ikeproposal (
                id, custom_field_data, name, authentication_method, encryption_algorithm,
                authentication_algorithm, "group", sa_lifetime, description, comments
            )
            VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6, $7, $8, $9)
            "#,
        )
        .bind(item.id)
        .bind(&item.name)
        .bind(&item.authentication_method)
        .bind(&item.encryption_algorithm)
        .bind(&item.authentication_algorithm)
        .bind(item.group)
        .bind(item.sa_lifetime)
        .bind(&item.description)
        .bind(&item.comments)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create vpn_ikeproposal: {}", e)))?;
        Ok(())
    }

    async fn update(&self, item: &VpnIkeProposal) -> Result<(), AppError> {
        sqlx::query(
            r#"
            UPDATE vpn_ikeproposal
            SET name = $2, authentication_method = $3, encryption_algorithm = $4,
                authentication_algorithm = $5, "group" = $6, sa_lifetime = $7,
                description = $8, comments = $9
            WHERE id = $1
            "#,
        )
        .bind(item.id)
        .bind(&item.name)
        .bind(&item.authentication_method)
        .bind(&item.encryption_algorithm)
        .bind(&item.authentication_algorithm)
        .bind(item.group)
        .bind(item.sa_lifetime)
        .bind(&item.description)
        .bind(&item.comments)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update vpn_ikeproposal: {}", e)))?;
        Ok(())
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "vpn_ikeproposal", id)
            .await
            .map(|_| ())
    }
}

fn push_vpn_ike_proposal_filters(
    builder: &mut QueryBuilder<'_, sqlx::Postgres>,
    filter: &VpnIkeProposalFilter,
) {
    let mut where_state = WhereState::new();
    push_search_ilike(
        builder,
        &mut where_state,
        &[
            "name",
            "authentication_method",
            "encryption_algorithm",
            "authentication_algorithm",
            "\"group\"",
            "sa_lifetime",
            "description",
            "comments",
        ],
        filter.search.as_deref(),
    );
    push_string_in(
        builder,
        &mut where_state,
        "authentication_method",
        &filter.authentication_method,
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

async fn count_vpn_ike_proposals(
    pool: &PgPool,
    filter: &VpnIkeProposalFilter,
) -> Result<i64, AppError> {
    let mut builder = QueryBuilder::new("SELECT COUNT(*) FROM vpn_ikeproposal");
    push_vpn_ike_proposal_filters(&mut builder, filter);
    builder
        .build_query_scalar::<i64>()
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to count vpn_ikeproposals: {}", e)))
}
