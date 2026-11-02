//! Provider Network repository — full CRUD over `circuits_providernetwork`.
//!
//! Runtime sqlx only (`sqlx::query` + `.bind`) so the crate type-checks and the
//! contract verifies via `cargo check` without a database.

use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use super::super::super::{ListColumn, SortSpec, push_order_by, push_uuid_in};
use crate::contexts::shared::list_query::WhereState;
use crate::contexts::shared::list_query::push_search_ilike;
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::contexts::shared::row::col;
use crate::errors::AppError;

pub const PROVIDER_NETWORK_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "pn.name", "inventory.providerNetworks.columns.name"),
    ListColumn::fk_filter(
        "provider",
        "p.name",
        "provider_id",
        "pn.provider_id",
        "providers",
        "inventory.providerNetworks.columns.provider",
    ),
    ListColumn::sortable(
        "service_id",
        "pn.service_id",
        "inventory.providerNetworks.columns.serviceId",
    ),
    ListColumn::sortable(
        "description",
        "pn.description",
        "inventory.providerNetworks.columns.description",
    ),
    ListColumn::sortable(
        "comments",
        "pn.comments",
        "inventory.providerNetworks.columns.comments",
    ),
];

/// Representation of a `circuits_providernetwork` row exposed by the REST API.
#[derive(Debug, Clone)]
pub struct ProviderNetworkRecord {
    pub id: Uuid,
    pub provider_id: Uuid,
    pub name: String,
    pub service_id: String,
    pub description: String,
    pub comments: String,
    pub custom_fields: Option<serde_json::Value>,
}

/// Hydrated list row for `circuits_providernetwork` — includes provider name
/// resolved via LEFT JOIN for the collection list endpoint.
#[derive(Debug, Clone)]
pub struct ProviderNetworkListItem {
    pub record: ProviderNetworkRecord,
    pub provider_name: Option<String>,
}

/// Input for creating a new `circuits_providernetwork` row.
#[derive(Debug, Clone)]
pub struct NewProviderNetwork {
    pub id: Uuid,
    pub provider_id: Uuid,
    pub name: String,
    pub service_id: Option<String>,
    pub description: Option<String>,
    pub comments: Option<String>,
}

/// Fields that may be updated on an existing `circuits_providernetwork` row.
#[derive(Debug, Clone)]
pub struct UpdateProviderNetwork {
    pub provider_id: Option<Uuid>,
    pub name: Option<String>,
    pub service_id: Option<String>,
    pub description: Option<String>,
    pub comments: Option<String>,
}

/// Result of a bulk-create operation.
#[derive(Debug, Clone)]
pub struct BulkProviderNetworkResult {
    pub created: Vec<ProviderNetworkRecord>,
    pub errors: Vec<String>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait ProviderNetworkRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<ProviderNetworkRecord>, AppError>;
    async fn list_hydrated(
        &self,
        provider_id: Vec<Uuid>,
        search: Option<String>,
        sort: Option<SortSpec>,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<ProviderNetworkListItem>, i64), AppError>;
    async fn create(
        &self,
        new_provider_network: &NewProviderNetwork,
    ) -> Result<ProviderNetworkRecord, AppError>;
    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateProviderNetwork,
    ) -> Result<Option<ProviderNetworkRecord>, AppError>;
    async fn bulk_create(
        &self,
        items: &[NewProviderNetwork],
    ) -> Result<BulkProviderNetworkResult, AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxProviderNetworkRepository {
    pool: PgPool,
}

const PROVIDER_NETWORK_SELECT_COLUMNS: &str = r#"
    id, provider_id, name, service_id, description, comments, custom_field_data
"#;

impl SqlxProviderNetworkRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn count_hydrated(
        &self,
        provider_id: &[Uuid],
        search: Option<&str>,
    ) -> Result<i64, AppError> {
        let mut builder = QueryBuilder::<Postgres>::new(
            "SELECT COUNT(*) FROM circuits_providernetwork pn LEFT JOIN circuits_provider p ON p.id = pn.provider_id",
        );
        push_provider_network_filters(&mut builder, provider_id, search);
        builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to count provider_networks: {}", e)))
    }

    fn parse_row(row: &PgRow) -> Result<ProviderNetworkRecord, AppError> {
        Ok(ProviderNetworkRecord {
            id: row.try_get("id").map_err(|e| {
                AppError::Database(format!("Failed to read provider_network id: {}", e))
            })?,
            provider_id: row.try_get("provider_id").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read provider_network provider_id: {}",
                    e
                ))
            })?,
            name: row.try_get("name").map_err(|e| {
                AppError::Database(format!("Failed to read provider_network name: {}", e))
            })?,
            service_id: row.try_get("service_id").map_err(|e| {
                AppError::Database(format!("Failed to read provider_network service_id: {}", e))
            })?,
            description: row.try_get("description").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read provider_network description: {}",
                    e
                ))
            })?,
            comments: row.try_get("comments").map_err(|e| {
                AppError::Database(format!("Failed to read provider_network comments: {}", e))
            })?,
            custom_fields: col(row, "custom_field_data")?,
        })
    }
}

impl ProviderNetworkRepository for SqlxProviderNetworkRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<ProviderNetworkRecord>, AppError> {
        let sql = format!(
            "SELECT {PROVIDER_NETWORK_SELECT_COLUMNS} FROM circuits_providernetwork WHERE id = $1"
        );
        let row = sqlx::query(&sql)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to load provider_network: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_hydrated(
        &self,
        provider_id: Vec<Uuid>,
        search: Option<String>,
        sort: Option<SortSpec>,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<ProviderNetworkListItem>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;
        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT pn.id, pn.provider_id, pn.name, pn.service_id, pn.description,
                   pn.comments, pn.custom_field_data,
                   p.name AS provider_name,
                   COUNT(*) OVER() AS total_count
            FROM circuits_providernetwork pn
            LEFT JOIN circuits_provider p ON p.id = pn.provider_id
            "#,
        );
        push_provider_network_filters(&mut builder, &provider_id, search.as_deref());
        push_order_by(
            &mut builder,
            PROVIDER_NETWORK_COLUMNS,
            sort.as_ref(),
            "pn.name ASC, pn.id",
        );
        if sort.is_some() {
            builder.push(", pn.id");
        }
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);
        let rows = builder.build().fetch_all(&self.pool).await.map_err(|e| {
            AppError::Database(format!("Failed to list provider_networks hydrated: {}", e))
        })?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read provider_network total_count: {}",
                    e
                ))
            })?,
            None => self.count_hydrated(&provider_id, search.as_deref()).await?,
        };

        let items = rows
            .iter()
            .map(|row| {
                Ok(ProviderNetworkListItem {
                    record: Self::parse_row(row)?,
                    provider_name: col(row, "provider_name")?,
                })
            })
            .collect::<Result<Vec<ProviderNetworkListItem>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(
        &self,
        new_provider_network: &NewProviderNetwork,
    ) -> Result<ProviderNetworkRecord, AppError> {
        let service_id = new_provider_network.service_id.clone().unwrap_or_default();
        let description = new_provider_network.description.clone().unwrap_or_default();
        let comments = new_provider_network.comments.clone().unwrap_or_default();

        sqlx::query(
            r#"
            INSERT INTO circuits_providernetwork (
                id, custom_field_data, provider_id, name, service_id, description, comments
            )
            VALUES (
                $1, '{}'::jsonb, $2, $3, $4, $5, $6
            )
            "#,
        )
        .bind(new_provider_network.id)
        .bind(new_provider_network.provider_id)
        .bind(&new_provider_network.name)
        .bind(&service_id)
        .bind(&description)
        .bind(&comments)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create provider_network: {}", e)))?;

        Ok(ProviderNetworkRecord {
            id: new_provider_network.id,
            provider_id: new_provider_network.provider_id,
            name: new_provider_network.name.clone(),
            service_id,
            description,
            comments,
            custom_fields: None,
        })
    }

    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateProviderNetwork,
    ) -> Result<Option<ProviderNetworkRecord>, AppError> {
        let Some(existing) = self.get_by_id(id).await? else {
            return Ok(None);
        };

        let provider_id = changes.provider_id.unwrap_or(existing.provider_id);
        let name = changes.name.clone().unwrap_or(existing.name);
        let service_id = changes.service_id.clone().unwrap_or(existing.service_id);
        let description = changes.description.clone().unwrap_or(existing.description);
        let comments = changes.comments.clone().unwrap_or(existing.comments);

        sqlx::query(
            r#"
            UPDATE circuits_providernetwork
            SET provider_id = $2, name = $3, service_id = $4, description = $5, comments = $6
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(provider_id)
        .bind(&name)
        .bind(&service_id)
        .bind(&description)
        .bind(&comments)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update provider_network: {}", e)))?;

        Ok(Some(ProviderNetworkRecord {
            id,
            provider_id,
            name,
            service_id,
            description,
            comments,
            custom_fields: existing.custom_fields,
        }))
    }

    async fn bulk_create(
        &self,
        items: &[NewProviderNetwork],
    ) -> Result<BulkProviderNetworkResult, AppError> {
        let mut created = Vec::new();
        let mut errors = Vec::new();

        let mut tx = super::super::bulk_insert::begin(&self.pool).await?;

        for item in items {
            let service_id = item.service_id.clone().unwrap_or_default();
            let description = item.description.clone().unwrap_or_default();
            let comments = item.comments.clone().unwrap_or_default();

            let result = super::super::bulk_insert::insert_row(
                &mut tx,
                sqlx::query(
                    r#"
                INSERT INTO circuits_providernetwork (
                    id, custom_field_data, provider_id, name, service_id, description, comments
                )
                VALUES (
                    $1, '{}'::jsonb, $2, $3, $4, $5, $6
                )
                "#,
                )
                .bind(item.id)
                .bind(item.provider_id)
                .bind(&item.name)
                .bind(&service_id)
                .bind(&description)
                .bind(&comments),
            )
            .await?;

            match result {
                Ok(_) => created.push(ProviderNetworkRecord {
                    id: item.id,
                    provider_id: item.provider_id,
                    name: item.name.clone(),
                    service_id,
                    description,
                    comments,
                    custom_fields: None,
                }),
                Err(e) => errors.push(format!(
                    "Failed to insert provider_network '{}': {}",
                    item.name, e
                )),
            }
        }

        tx.commit().await.map_err(|e| {
            AppError::Database(format!(
                "Failed to commit bulk provider_network insert: {}",
                e
            ))
        })?;

        Ok(BulkProviderNetworkResult { created, errors })
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(
            &self.pool,
            "circuits_providernetwork",
            id,
        )
        .await
        .map(|_| ())
    }
}

fn push_provider_network_filters(
    builder: &mut QueryBuilder<'_, Postgres>,
    provider_id: &[Uuid],
    search: Option<&str>,
) {
    let mut where_state = WhereState::new();
    push_uuid_in(builder, &mut where_state, "pn.provider_id", provider_id);
    push_search_ilike(
        builder,
        &mut where_state,
        &[
            "pn.name",
            "pn.service_id",
            "pn.description",
            "pn.comments",
            "p.name",
        ],
        search,
    );
}
