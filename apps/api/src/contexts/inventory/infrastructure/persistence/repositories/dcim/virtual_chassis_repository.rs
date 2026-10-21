//! Virtual chassis repository — full CRUD over `dcim_virtualchassis`.
//!
//! Runtime sqlx only (`sqlx::query` + `.bind`) so the crate type-checks and the
//! contract verifies via `cargo check` without a database.

use sqlx::postgres::PgRow;
use sqlx::{PgPool, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_search_ilike, push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

pub const VIRTUAL_CHASSIS_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "name", "inventory.virtualChassis.columns.name"),
    ListColumn::sortable(
        "domain",
        "domain",
        "inventory.virtualChassis.columns.domain",
    ),
    ListColumn::fk_filter(
        "master_id",
        "master_id",
        "master_id",
        "master_id",
        "devices",
        "inventory.virtualChassis.columns.master",
    ),
    ListColumn::sortable(
        "description",
        "description",
        "inventory.virtualChassis.columns.description",
    ),
    ListColumn::sortable(
        "comments",
        "comments",
        "inventory.virtualChassis.columns.comments",
    ),
    ListColumn::sortable(
        "member_count",
        "member_count",
        "inventory.virtualChassis.columns.memberCount",
    ),
    ListColumn::fk_filter(
        "owner_id",
        "owner_id",
        "owner_id",
        "owner_id",
        "users",
        "inventory.virtualChassis.columns.owner",
    ),
];

/// Representation of a `dcim_virtualchassis` row exposed by the REST API.
#[derive(Debug, Clone)]
pub struct VirtualChassisRecord {
    pub id: Uuid,
    pub name: String,
    pub domain: String,
    pub master_id: Option<Uuid>,
    pub description: String,
    pub comments: String,
    pub member_count: i64,
    pub custom_fields: Option<serde_json::Value>,
    pub owner_id: Option<Uuid>,
    pub owner_name: Option<String>,
}

/// Input for creating a new `dcim_virtualchassis` row.
#[derive(Debug, Clone)]
pub struct NewVirtualChassis {
    pub id: Uuid,
    pub name: String,
    pub domain: String,
    pub master_id: Option<Uuid>,
    pub description: String,
    pub comments: String,
    pub owner_id: Option<Uuid>,
}

/// Fields that may be updated on an existing `dcim_virtualchassis` row.
#[derive(Debug, Clone)]
pub struct UpdateVirtualChassis {
    pub name: Option<String>,
    pub domain: Option<String>,
    pub master_id: Option<Option<Uuid>>,
    pub description: Option<String>,
    pub comments: Option<String>,
}

/// Result of a bulk-create operation.
#[derive(Debug, Clone)]
pub struct BulkVirtualChassisResult {
    pub created: Vec<VirtualChassisRecord>,
    pub errors: Vec<String>,
}

/// Selection, paging and ordering for the virtual chassis collection.
pub struct VirtualChassisListParams<'a> {
    pub master_ids: &'a [Uuid],
    pub owner_ids: &'a [Uuid],
    pub search: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<SortSpec>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait VirtualChassisRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<VirtualChassisRecord>, AppError>;
    async fn list(
        &self,
        params: VirtualChassisListParams<'_>,
    ) -> Result<(Vec<VirtualChassisRecord>, i64), AppError>;
    async fn create(&self, new_vc: &NewVirtualChassis) -> Result<VirtualChassisRecord, AppError>;
    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateVirtualChassis,
    ) -> Result<Option<VirtualChassisRecord>, AppError>;
    async fn bulk_create(
        &self,
        items: &[NewVirtualChassis],
    ) -> Result<BulkVirtualChassisResult, AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxVirtualChassisRepository {
    pool: PgPool,
}

impl SqlxVirtualChassisRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<VirtualChassisRecord, AppError> {
        Ok(VirtualChassisRecord {
            id: row.try_get("id").map_err(|e| {
                AppError::Database(format!("Failed to read virtual_chassis id: {}", e))
            })?,
            name: row.try_get("name").map_err(|e| {
                AppError::Database(format!("Failed to read virtual_chassis name: {}", e))
            })?,
            domain: row.try_get("domain").map_err(|e| {
                AppError::Database(format!("Failed to read virtual_chassis domain: {}", e))
            })?,
            master_id: row.try_get::<Option<Uuid>, _>("master_id").map_err(|e| {
                AppError::Database(format!("Failed to read virtual_chassis master_id: {}", e))
            })?,
            description: row.try_get("description").map_err(|e| {
                AppError::Database(format!("Failed to read virtual_chassis description: {}", e))
            })?,
            comments: row.try_get("comments").map_err(|e| {
                AppError::Database(format!("Failed to read virtual_chassis comments: {}", e))
            })?,
            member_count: row.try_get("member_count").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read virtual_chassis member_count: {}",
                    e
                ))
            })?,
            custom_fields: row
                .try_get::<Option<serde_json::Value>, _>("custom_field_data")
                .ok()
                .flatten(),
            owner_id: row.try_get::<Option<Uuid>, _>("owner_id").map_err(|e| {
                AppError::Database(format!("Failed to read virtual_chassis owner_id: {}", e))
            })?,
            owner_name: row
                .try_get::<Option<String>, _>("owner_name")
                .ok()
                .flatten(),
        })
    }
}

impl VirtualChassisRepository for SqlxVirtualChassisRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<VirtualChassisRecord>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, name, domain, master_id, description, comments,
                   member_count, custom_field_data, owner_id, NULL::text AS owner_name
            FROM dcim_virtualchassis
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load virtual_chassis: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list(
        &self,
        params: VirtualChassisListParams<'_>,
    ) -> Result<(Vec<VirtualChassisRecord>, i64), AppError> {
        let limit = clamp_limit(params.limit) as i64;
        let offset = clamp_offset(params.offset) as i64;

        let mut builder = QueryBuilder::new(
            "
            SELECT id, name, domain, master_id, description, comments,
                   member_count, custom_field_data, owner_id, owner_name,
                   COUNT(*) OVER() AS total_count
            FROM (
                SELECT vc.id, vc.name, vc.domain, vc.master_id, vc.description, vc.comments,
                       vc.member_count, vc.custom_field_data, vc.owner_id,
                       COALESCE(NULLIF(u.username, ''), u.email) AS owner_name
                FROM dcim_virtualchassis vc
                LEFT JOIN identity_user u ON u.id = vc.owner_id
            ) virtual_chassis_rows
            ",
        );
        push_virtual_chassis_filters(
            &mut builder,
            params.master_ids,
            params.owner_ids,
            params.search.as_deref(),
        );
        push_order_by(
            &mut builder,
            VIRTUAL_CHASSIS_COLUMNS,
            params.sort.as_ref(),
            "name",
        );
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows =
            builder.build().fetch_all(&self.pool).await.map_err(|e| {
                AppError::Database(format!("Failed to list virtual_chassis: {}", e))
            })?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!("Failed to read virtual_chassis total_count: {}", e))
            })?,
            None => {
                let mut count_builder =
                    QueryBuilder::new("SELECT COUNT(*) FROM dcim_virtualchassis");
                push_virtual_chassis_filters(
                    &mut count_builder,
                    params.master_ids,
                    params.owner_ids,
                    params.search.as_deref(),
                );
                count_builder
                    .build_query_scalar::<i64>()
                    .fetch_one(&self.pool)
                    .await
                    .map_err(|e| {
                        AppError::Database(format!("Failed to count virtual_chassis: {}", e))
                    })?
            }
        };

        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<VirtualChassisRecord>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, new_vc: &NewVirtualChassis) -> Result<VirtualChassisRecord, AppError> {
        sqlx::query(
            r#"
            INSERT INTO dcim_virtualchassis (
                id, custom_field_data, name, domain, master_id,
                description, comments, member_count, owner_id
            )
            VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6, 0, $7)
            "#,
        )
        .bind(new_vc.id)
        .bind(&new_vc.name)
        .bind(&new_vc.domain)
        .bind(new_vc.master_id)
        .bind(&new_vc.description)
        .bind(&new_vc.comments)
        .bind(new_vc.owner_id)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create virtual_chassis: {}", e)))?;

        Ok(VirtualChassisRecord {
            id: new_vc.id,
            name: new_vc.name.clone(),
            domain: new_vc.domain.clone(),
            master_id: new_vc.master_id,
            description: new_vc.description.clone(),
            comments: new_vc.comments.clone(),
            member_count: 0,
            custom_fields: None,
            owner_id: new_vc.owner_id,
            owner_name: None,
        })
    }

    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateVirtualChassis,
    ) -> Result<Option<VirtualChassisRecord>, AppError> {
        let Some(existing) = self.get_by_id(id).await? else {
            return Ok(None);
        };

        let name = changes.name.clone().unwrap_or(existing.name);
        let domain = changes.domain.clone().unwrap_or(existing.domain);
        let master_id = match &changes.master_id {
            Some(v) => *v,
            None => existing.master_id,
        };
        let description = changes.description.clone().unwrap_or(existing.description);
        let comments = changes.comments.clone().unwrap_or(existing.comments);

        sqlx::query(
            r#"
            UPDATE dcim_virtualchassis
            SET name = $2, domain = $3, master_id = $4,
                description = $5, comments = $6
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(&name)
        .bind(&domain)
        .bind(master_id)
        .bind(&description)
        .bind(&comments)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update virtual_chassis: {}", e)))?;

        Ok(Some(VirtualChassisRecord {
            id,
            name,
            domain,
            master_id,
            description,
            comments,
            member_count: existing.member_count,
            custom_fields: existing.custom_fields,
            owner_id: existing.owner_id,
            owner_name: existing.owner_name,
        }))
    }

    async fn bulk_create(
        &self,
        items: &[NewVirtualChassis],
    ) -> Result<BulkVirtualChassisResult, AppError> {
        let mut created = Vec::new();
        let mut errors = Vec::new();

        let mut tx = super::super::bulk_insert::begin(&self.pool).await?;

        for item in items {
            let result = super::super::bulk_insert::insert_row(
                &mut tx,
                sqlx::query(
                    r#"
                INSERT INTO dcim_virtualchassis (
                    id, custom_field_data, name, domain, master_id,
                    description, comments, member_count, owner_id
                )
                VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6, 0, $7)
                "#,
                )
                .bind(item.id)
                .bind(&item.name)
                .bind(&item.domain)
                .bind(item.master_id)
                .bind(&item.description)
                .bind(&item.comments)
                .bind(item.owner_id),
            )
            .await?;

            match result {
                Ok(_) => created.push(VirtualChassisRecord {
                    id: item.id,
                    name: item.name.clone(),
                    domain: item.domain.clone(),
                    master_id: item.master_id,
                    description: item.description.clone(),
                    comments: item.comments.clone(),
                    member_count: 0,
                    custom_fields: None,
                    owner_id: item.owner_id,
                    owner_name: None,
                }),
                Err(e) => errors.push(format!(
                    "Failed to insert virtual_chassis '{}': {}",
                    item.name, e
                )),
            }
        }

        tx.commit().await.map_err(|e| {
            AppError::Database(format!(
                "Failed to commit bulk virtual_chassis insert: {}",
                e
            ))
        })?;

        Ok(BulkVirtualChassisResult { created, errors })
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "dcim_virtualchassis", id)
            .await
            .map(|_| ())
    }
}

fn push_virtual_chassis_filters(
    builder: &mut QueryBuilder<'_, sqlx::Postgres>,
    master_ids: &[Uuid],
    owner_ids: &[Uuid],
    search: Option<&str>,
) {
    let mut where_state = WhereState::new();
    push_uuid_in(builder, &mut where_state, "master_id", master_ids);
    push_uuid_in(builder, &mut where_state, "owner_id", owner_ids);
    push_search_ilike(
        builder,
        &mut where_state,
        &["name", "domain", "description", "comments", "member_count"],
        search,
    );
}
