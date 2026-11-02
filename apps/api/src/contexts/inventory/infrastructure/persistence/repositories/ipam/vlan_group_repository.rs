//! VLAN group repository — full CRUD over `ipam_vlangroup`.
//!
//! Runtime sqlx only (`sqlx::query` + `.bind`) so the crate type-checks and the
//! contract verifies via `cargo check` without a database.
//!
//! `vid_ranges` is a Postgres `int4range[]`, which has no native sqlx decoder;
//! it is read via a `::text[]` cast (`Vec<String>`) and written by casting the
//! bound `text[]` back to `int4range[]`. The NetBox-maintained `_total_vlan_ids`
//! cache column is initialized to 0 on create (recomputed by the sync engine).

use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, filter_column, push_order_by, push_search_ilike, push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

pub const VLAN_GROUP_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "name", "inventory.vlanGroups.columns.name"),
    ListColumn::sortable("slug", "slug", "inventory.vlanGroups.columns.slug"),
    ListColumn::fk_filter(
        "scope_id",
        "scope_id",
        "scope_id",
        "scope_id",
        "scopes",
        "inventory.vlanGroups.columns.scope",
    ),
    ListColumn::fk_filter(
        "scope_type_id",
        "scope_type_id",
        "scope_type_id",
        "scope_type_id",
        "contentTypes",
        "inventory.vlanGroups.columns.scopeType",
    ),
    ListColumn::fk_filter(
        "tenant_id",
        "tenant_id",
        "tenant_id",
        "tenant_id",
        "tenants",
        "inventory.vlanGroups.columns.tenant",
    ),
    ListColumn::sortable(
        "description",
        "description",
        "inventory.vlanGroups.columns.description",
    ),
    ListColumn::fk_filter(
        "owner_id",
        "owner_id",
        "owner_id",
        "owner_id",
        "users",
        "inventory.vlanGroups.columns.owner",
    ),
];

/// Representation of an `ipam_vlangroup` row exposed by the REST API.
#[derive(Debug, Clone)]
pub struct VlanGroupRecord {
    pub id: Uuid,
    pub external_id: Option<i64>,
    pub name: String,
    pub slug: String,
    pub scope_id: Option<Uuid>,
    pub scope_type_id: Option<Uuid>,
    pub vid_ranges: Vec<String>,
    pub tenant_id: Option<Uuid>,
    pub description: Option<String>,
    pub owner_id: Option<Uuid>,
    pub tags: Vec<String>,
    pub custom_fields: Option<serde_json::Value>,
    pub piping_synced_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Input for creating a new `ipam_vlangroup` row.
#[derive(Debug, Clone)]
pub struct NewVlanGroup {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub scope_id: Option<Uuid>,
    pub scope_type_id: Option<Uuid>,
    pub vid_ranges: Vec<String>,
    pub tenant_id: Option<Uuid>,
    pub description: Option<String>,
    pub owner_id: Option<Uuid>,
}

/// Fields that may be updated on an existing `ipam_vlangroup` row.
#[derive(Debug, Clone)]
pub struct UpdateVlanGroup {
    pub name: Option<String>,
    pub slug: Option<String>,
    pub scope_id: Option<Uuid>,
    pub scope_type_id: Option<Uuid>,
    pub vid_ranges: Option<Vec<String>>,
    pub tenant_id: Option<Uuid>,
    pub description: Option<String>,
}

/// Result of a bulk-create operation.
#[derive(Debug, Clone)]
pub struct BulkVlanGroupResult {
    pub created: Vec<VlanGroupRecord>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct VlanGroupFilter {
    pub scope_id: Vec<Uuid>,
    pub scope_type_id: Vec<Uuid>,
    pub tenant_id: Vec<Uuid>,
    pub owner_id: Vec<Uuid>,
    pub search: Option<String>,
    pub sort: Option<SortSpec>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait VlanGroupRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<VlanGroupRecord>, AppError>;
    async fn list(
        &self,
        filter: VlanGroupFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<VlanGroupRecord>, i64), AppError>;
    async fn create(&self, new_group: &NewVlanGroup) -> Result<VlanGroupRecord, AppError>;
    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateVlanGroup,
    ) -> Result<Option<VlanGroupRecord>, AppError>;
    async fn bulk_create(&self, items: &[NewVlanGroup]) -> Result<BulkVlanGroupResult, AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxVlanGroupRepository {
    pool: PgPool,
}

/// Selected columns; `vid_ranges` is cast to `text[]` because `int4range[]` has
/// no native sqlx decoder.
const VLAN_GROUP_SELECT_COLUMNS: &str = r#"
    id, external_id, name, slug, scope_id, scope_type_id,
    vid_ranges::text[] AS vid_ranges, tenant_id, description, owner_id, tags,
    custom_field_data, piping_synced_at
"#;

impl SqlxVlanGroupRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<VlanGroupRecord, AppError> {
        Ok(VlanGroupRecord {
            id: row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read vlan_group id: {}", e)))?,
            external_id: row.try_get::<Option<i64>, _>("external_id").map_err(|e| {
                AppError::Database(format!("Failed to read vlan_group external_id: {}", e))
            })?,
            name: row.try_get("name").map_err(|e| {
                AppError::Database(format!("Failed to read vlan_group name: {}", e))
            })?,
            slug: row.try_get("slug").map_err(|e| {
                AppError::Database(format!("Failed to read vlan_group slug: {}", e))
            })?,
            scope_id: row.try_get::<Option<Uuid>, _>("scope_id").ok().flatten(),
            scope_type_id: row
                .try_get::<Option<Uuid>, _>("scope_type_id")
                .ok()
                .flatten(),
            vid_ranges: row
                .try_get::<Vec<String>, _>("vid_ranges")
                .unwrap_or_default(),
            tenant_id: row.try_get::<Option<Uuid>, _>("tenant_id").ok().flatten(),
            description: row
                .try_get::<Option<String>, _>("description")
                .ok()
                .flatten(),
            owner_id: row.try_get::<Option<Uuid>, _>("owner_id").map_err(|e| {
                AppError::Database(format!("Failed to read vlan_group owner_id: {}", e))
            })?,
            tags: row.try_get::<Vec<String>, _>("tags").unwrap_or_default(),
            custom_fields: row
                .try_get::<Option<serde_json::Value>, _>("custom_field_data")
                .ok()
                .flatten(),
            piping_synced_at: row
                .try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("piping_synced_at")
                .map_err(|e| {
                    AppError::Database(format!("Failed to read vlan_group piping_synced_at: {}", e))
                })?,
        })
    }

    fn push_list_filters(
        builder: &mut QueryBuilder<'_, Postgres>,
        filter: &VlanGroupFilter,
    ) -> Result<(), AppError> {
        let mut where_state = WhereState::new();
        push_uuid_in(
            builder,
            &mut where_state,
            filter_column(VLAN_GROUP_COLUMNS, "vlan group", "scope_id")?.sql,
            &filter.scope_id,
        );
        push_uuid_in(
            builder,
            &mut where_state,
            filter_column(VLAN_GROUP_COLUMNS, "vlan group", "scope_type_id")?.sql,
            &filter.scope_type_id,
        );
        push_uuid_in(
            builder,
            &mut where_state,
            filter_column(VLAN_GROUP_COLUMNS, "vlan group", "tenant_id")?.sql,
            &filter.tenant_id,
        );
        push_uuid_in(
            builder,
            &mut where_state,
            filter_column(VLAN_GROUP_COLUMNS, "vlan group", "owner_id")?.sql,
            &filter.owner_id,
        );
        push_search_ilike(
            builder,
            &mut where_state,
            &[
                "external_id",
                "name",
                "slug",
                "vid_ranges::text",
                "description",
                "tags",
            ],
            filter.search.as_deref(),
        );
        Ok(())
    }

    async fn count_filtered(&self, filter: &VlanGroupFilter) -> Result<i64, AppError> {
        let mut builder = QueryBuilder::new("SELECT COUNT(*) FROM ipam_vlangroup");
        Self::push_list_filters(&mut builder, filter)?;
        builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to count vlan_groups: {}", e)))
    }
}

impl VlanGroupRepository for SqlxVlanGroupRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<VlanGroupRecord>, AppError> {
        let sql = format!("SELECT {VLAN_GROUP_SELECT_COLUMNS} FROM ipam_vlangroup WHERE id = $1");
        let row = sqlx::query(&sql)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to load vlan_group: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list(
        &self,
        filter: VlanGroupFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<VlanGroupRecord>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(format!(
            r#"
            SELECT {VLAN_GROUP_SELECT_COLUMNS}, COUNT(*) OVER() AS total_count
            FROM ipam_vlangroup
            "#,
        ));
        Self::push_list_filters(&mut builder, &filter)?;
        push_order_by(
            &mut builder,
            VLAN_GROUP_COLUMNS,
            filter.sort.as_ref(),
            "name",
        );
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list vlan_groups: {}", e)))?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!("Failed to read vlan_group total_count: {}", e))
            })?,
            None => self.count_filtered(&filter).await?,
        };
        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<VlanGroupRecord>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, new_group: &NewVlanGroup) -> Result<VlanGroupRecord, AppError> {
        let description = new_group.description.clone().unwrap_or_default();

        sqlx::query(
            r#"
            INSERT INTO ipam_vlangroup (
                id, custom_field_data, name, slug, scope_id, scope_type_id,
                vid_ranges, _total_vlan_ids, tenant_id, description, comments,
                owner_id, tags
            )
            VALUES (
                $1, '{}'::jsonb, $2, $3, $4, $5, $6::text[]::int4range[], 0, $7, $8,
                '', $9, '{}'
            )
            "#,
        )
        .bind(new_group.id)
        .bind(&new_group.name)
        .bind(&new_group.slug)
        .bind(new_group.scope_id)
        .bind(new_group.scope_type_id)
        .bind(&new_group.vid_ranges)
        .bind(new_group.tenant_id)
        .bind(&description)
        .bind(new_group.owner_id)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create vlan_group: {}", e)))?;

        Ok(VlanGroupRecord {
            id: new_group.id,
            external_id: None,
            name: new_group.name.clone(),
            slug: new_group.slug.clone(),
            scope_id: new_group.scope_id,
            scope_type_id: new_group.scope_type_id,
            vid_ranges: new_group.vid_ranges.clone(),
            tenant_id: new_group.tenant_id,
            description: Some(description),
            owner_id: new_group.owner_id,
            tags: Vec::new(),
            custom_fields: None,
            piping_synced_at: None,
        })
    }

    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateVlanGroup,
    ) -> Result<Option<VlanGroupRecord>, AppError> {
        let Some(existing) = self.get_by_id(id).await? else {
            return Ok(None);
        };

        let name = changes.name.clone().unwrap_or(existing.name);
        let slug = changes.slug.clone().unwrap_or(existing.slug);
        let scope_id = if changes.scope_id.is_some() {
            changes.scope_id
        } else {
            existing.scope_id
        };
        let scope_type_id = if changes.scope_type_id.is_some() {
            changes.scope_type_id
        } else {
            existing.scope_type_id
        };
        let vid_ranges = changes.vid_ranges.clone().unwrap_or(existing.vid_ranges);
        let tenant_id = if changes.tenant_id.is_some() {
            changes.tenant_id
        } else {
            existing.tenant_id
        };
        let description = if changes.description.is_some() {
            changes.description.clone()
        } else {
            existing.description
        };

        sqlx::query(
            r#"
            UPDATE ipam_vlangroup
            SET name = $2, slug = $3, scope_id = $4, scope_type_id = $5,
                vid_ranges = $6::text[]::int4range[], tenant_id = $7, description = $8
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(&name)
        .bind(&slug)
        .bind(scope_id)
        .bind(scope_type_id)
        .bind(&vid_ranges)
        .bind(tenant_id)
        .bind(description.clone().unwrap_or_default())
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update vlan_group: {}", e)))?;

        Ok(Some(VlanGroupRecord {
            id,
            external_id: existing.external_id,
            name,
            slug,
            scope_id,
            scope_type_id,
            vid_ranges,
            tenant_id,
            description,
            owner_id: existing.owner_id,
            tags: existing.tags,
            custom_fields: existing.custom_fields,
            piping_synced_at: existing.piping_synced_at,
        }))
    }

    async fn bulk_create(&self, items: &[NewVlanGroup]) -> Result<BulkVlanGroupResult, AppError> {
        let mut created = Vec::new();
        let mut errors = Vec::new();

        let mut tx = super::super::bulk_insert::begin(&self.pool).await?;

        for item in items {
            let description = item.description.clone().unwrap_or_default();

            let result = super::super::bulk_insert::insert_row(
                &mut tx,
                sqlx::query(
                    r#"
                INSERT INTO ipam_vlangroup (
                    id, custom_field_data, name, slug, scope_id, scope_type_id,
                    vid_ranges, _total_vlan_ids, tenant_id, description, comments,
                    owner_id, tags
                )
                VALUES (
                    $1, '{}'::jsonb, $2, $3, $4, $5, $6::text[]::int4range[], 0, $7, $8,
                    '', $9, '{}'
                )
                "#,
                )
                .bind(item.id)
                .bind(&item.name)
                .bind(&item.slug)
                .bind(item.scope_id)
                .bind(item.scope_type_id)
                .bind(&item.vid_ranges)
                .bind(item.tenant_id)
                .bind(&description)
                .bind(item.owner_id),
            )
            .await?;

            match result {
                Ok(_) => created.push(VlanGroupRecord {
                    id: item.id,
                    external_id: None,
                    name: item.name.clone(),
                    slug: item.slug.clone(),
                    scope_id: item.scope_id,
                    scope_type_id: item.scope_type_id,
                    vid_ranges: item.vid_ranges.clone(),
                    tenant_id: item.tenant_id,
                    description: Some(description),
                    owner_id: item.owner_id,
                    tags: Vec::new(),
                    custom_fields: None,
                    piping_synced_at: None,
                }),
                Err(e) => errors.push(format!(
                    "Failed to insert vlan_group '{}': {}",
                    item.name, e
                )),
            }
        }

        tx.commit().await.map_err(|e| {
            AppError::Database(format!("Failed to commit bulk vlan_group insert: {}", e))
        })?;

        Ok(BulkVlanGroupResult { created, errors })
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "ipam_vlangroup", id)
            .await
            .map(|_| ())
    }
}
