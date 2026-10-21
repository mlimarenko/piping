//! VLAN repository — full CRUD.

use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use super::super::super::list_query::{
    ListColumn, SortSpec, push_id_filter, push_order_by, push_search_ilike, push_string_in,
    push_uuid_in,
};
use crate::contexts::shared::list_query::WhereState;
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

pub const VLAN_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("vid", "vl.vid", "inventory.vlans.columns.vid"),
    ListColumn::sortable("name", "vl.name", "inventory.vlans.columns.name"),
    ListColumn::enum_filter(
        "status",
        "vl.status",
        "vl.status",
        "inventory.vlans.columns.status",
    ),
    ListColumn::fk_filter(
        "site",
        "s.name",
        "site_id",
        "vl.site_id",
        "sites",
        "inventory.vlans.columns.site",
    ),
    ListColumn::fk_filter(
        "tenant",
        "t.name",
        "tenant_id",
        "vl.tenant_id",
        "tenants",
        "inventory.vlans.columns.tenant",
    ),
];

/// Minimal representation of an `ipam_vlan` row exposed by the REST API.
#[derive(Debug, Clone)]
pub struct VlanRecord {
    pub id: Uuid,
    pub vid: i32,
    pub name: String,
    pub status: String,
    pub site_id: Option<Uuid>,
    pub group_id: Option<Uuid>,
    pub tenant_id: Option<Uuid>,
    pub role_id: Option<Uuid>,
    pub description: Option<String>,
}

/// Hydrated list row for `ipam_vlan` — includes site/tenant names resolved via
/// LEFT JOIN for the collection list endpoint.
#[derive(Debug, Clone)]
pub struct VlanListItem {
    pub record: VlanRecord,
    pub site_name: Option<String>,
    pub tenant_name: Option<String>,
}

/// Input for creating a new `ipam_vlan` row.
#[derive(Debug, Clone)]
pub struct NewVlan {
    pub id: Uuid,
    pub vid: i32,
    pub name: String,
    pub status: Option<String>,
    pub site_id: Option<Uuid>,
    pub group_id: Option<Uuid>,
    pub tenant_id: Option<Uuid>,
    pub role_id: Option<Uuid>,
    pub description: Option<String>,
}

/// Fields that may be updated on an existing `ipam_vlan` row.
#[derive(Debug, Clone)]
pub struct UpdateVlan {
    pub vid: Option<i32>,
    pub name: Option<String>,
    pub status: Option<String>,
    pub site_id: Option<Uuid>,
    pub group_id: Option<Uuid>,
    pub tenant_id: Option<Uuid>,
    pub role_id: Option<Uuid>,
    pub description: Option<String>,
}

/// VLAN list filter. Multi-value FK fields are `Vec<Uuid>` where an empty vector
/// means "no filter" and a non-empty vector emits `col IN (…)`. `status` is a
/// multi-value string filter and `vid` an optional exact match. `sort` carries
/// an already-allowlist-validated [`SortSpec`].
#[derive(Debug, Clone, Default)]
pub struct VlanFilter {
    pub ids: Option<Vec<Uuid>>,
    pub search: Option<String>,
    pub site_id: Vec<Uuid>,
    pub group_id: Vec<Uuid>,
    pub tenant_id: Vec<Uuid>,
    pub vid: Option<i32>,
    pub status: Vec<String>,
    pub sort: Option<SortSpec>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait VlanRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<VlanRecord>, AppError>;
    async fn list_hydrated(
        &self,
        filter: VlanFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<VlanListItem>, i64), AppError>;
    async fn create(&self, new_vlan: &NewVlan) -> Result<VlanRecord, AppError>;
    async fn update(&self, id: Uuid, changes: &UpdateVlan) -> Result<Option<VlanRecord>, AppError>;
    async fn bulk_create(
        &self,
        items: &[NewVlan],
    ) -> Result<super::super::bulk_insert::BulkCreateResult<VlanRecord>, AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxVlanRepository {
    pool: PgPool,
}

impl SqlxVlanRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<VlanRecord, AppError> {
        Ok(VlanRecord {
            id: row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read vlan id: {}", e)))?,
            vid: row
                .try_get("vid")
                .map_err(|e| AppError::Database(format!("Failed to read vlan vid: {}", e)))?,
            name: row
                .try_get("name")
                .map_err(|e| AppError::Database(format!("Failed to read vlan name: {}", e)))?,
            status: row
                .try_get("status")
                .map_err(|e| AppError::Database(format!("Failed to read vlan status: {}", e)))?,
            site_id: row
                .try_get::<Option<Uuid>, _>("site_id")
                .map_err(|e| AppError::Database(format!("Failed to read vlan site_id: {}", e)))?,
            group_id: row
                .try_get::<Option<Uuid>, _>("group_id")
                .map_err(|e| AppError::Database(format!("Failed to read vlan group_id: {}", e)))?,
            tenant_id: row
                .try_get::<Option<Uuid>, _>("tenant_id")
                .map_err(|e| AppError::Database(format!("Failed to read vlan tenant_id: {}", e)))?,
            role_id: row
                .try_get::<Option<Uuid>, _>("role_id")
                .map_err(|e| AppError::Database(format!("Failed to read vlan role_id: {}", e)))?,
            description: row
                .try_get::<Option<String>, _>("description")
                .map_err(|e| {
                    AppError::Database(format!("Failed to read vlan description: {}", e))
                })?,
        })
    }
}

impl VlanRepository for SqlxVlanRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<VlanRecord>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, vid::integer AS vid, name, status, site_id, group_id, tenant_id, role_id, description
            FROM ipam_vlan
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load vlan: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_hydrated(
        &self,
        filter: VlanFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<VlanListItem>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT vl.id, vl.vid::integer AS vid, vl.name, vl.status, vl.site_id, vl.group_id, vl.tenant_id,
                   vl.role_id, vl.description,
                   s.name AS site_name,
                   t.name AS tenant_name,
                   COUNT(*) OVER() AS total_count
            FROM ipam_vlan vl
            LEFT JOIN dcim_site s ON s.id = vl.site_id
            LEFT JOIN tenancy_tenant t ON t.id = vl.tenant_id
            "#,
        );

        let has_filter = filter.ids.is_some()
            || filter.search.is_some()
            || !filter.site_id.is_empty()
            || !filter.group_id.is_empty()
            || !filter.tenant_id.is_empty()
            || filter.vid.is_some()
            || !filter.status.is_empty();
        if has_filter {
            builder.push(" WHERE ");
            let mut where_state = WhereState::after_where();
            push_id_filter(
                &mut builder,
                &mut where_state,
                "vl.id",
                filter.ids.as_deref(),
            );
            push_search_ilike(
                &mut builder,
                &mut where_state,
                &[
                    "vl.vid",
                    "vl.name",
                    "vl.status",
                    "vl.description",
                    "s.name",
                    "t.name",
                ],
                filter.search.as_deref(),
            );
            push_uuid_in(
                &mut builder,
                &mut where_state,
                "vl.site_id",
                &filter.site_id,
            );
            push_uuid_in(
                &mut builder,
                &mut where_state,
                "vl.group_id",
                &filter.group_id,
            );
            push_uuid_in(
                &mut builder,
                &mut where_state,
                "vl.tenant_id",
                &filter.tenant_id,
            );
            push_string_in(&mut builder, &mut where_state, "vl.status", &filter.status);
            if let Some(vid) = filter.vid {
                where_state.and(&mut builder);
                builder.push("vl.vid = ");
                builder.push_bind(vid);
            }
        }

        push_order_by(
            &mut builder,
            VLAN_COLUMNS,
            filter.sort.as_ref(),
            "vl.vid ASC, vl.name ASC, vl.id",
        );
        if filter.sort.is_some() {
            builder.push(", vl.id");
        }
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list vlans hydrated: {}", e)))?;

        let total = rows
            .first()
            .map(|row| row.try_get::<i64, _>("total_count"))
            .transpose()
            .map_err(|e| AppError::Database(format!("Failed to read VLAN total_count: {}", e)))?
            .unwrap_or(0);
        let items = rows
            .iter()
            .map(|row| {
                Ok::<VlanListItem, AppError>(VlanListItem {
                    record: Self::parse_row(row)?,
                    site_name: row.try_get("site_name").ok(),
                    tenant_name: row.try_get("tenant_name").ok(),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok((items, total))
    }

    async fn create(&self, new_vlan: &NewVlan) -> Result<VlanRecord, AppError> {
        let status = new_vlan
            .status
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("active")
            .to_string();

        sqlx::query(
            r#"
            INSERT INTO ipam_vlan (
                id, vid, name, status, site_id, group_id, tenant_id, role_id, description,
                custom_field_data, comments, tags
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, '{}'::jsonb, '', '{}')
            "#,
        )
        .bind(new_vlan.id)
        .bind(new_vlan.vid)
        .bind(&new_vlan.name)
        .bind(&status)
        .bind(new_vlan.site_id)
        .bind(new_vlan.group_id)
        .bind(new_vlan.tenant_id)
        .bind(new_vlan.role_id)
        .bind(new_vlan.description.as_deref().unwrap_or(""))
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create vlan: {}", e)))?;

        Ok(VlanRecord {
            id: new_vlan.id,
            vid: new_vlan.vid,
            name: new_vlan.name.clone(),
            status,
            site_id: new_vlan.site_id,
            group_id: new_vlan.group_id,
            tenant_id: new_vlan.tenant_id,
            role_id: new_vlan.role_id,
            description: new_vlan.description.clone(),
        })
    }

    async fn update(&self, id: Uuid, changes: &UpdateVlan) -> Result<Option<VlanRecord>, AppError> {
        let existing = self.get_by_id(id).await?;
        let Some(existing) = existing else {
            return Ok(None);
        };

        let vid = changes.vid.unwrap_or(existing.vid);
        let name = changes.name.clone().unwrap_or(existing.name);
        let status = changes.status.clone().unwrap_or(existing.status);
        let site_id = if changes.site_id.is_some() {
            changes.site_id
        } else {
            existing.site_id
        };
        let group_id = if changes.group_id.is_some() {
            changes.group_id
        } else {
            existing.group_id
        };
        let tenant_id = if changes.tenant_id.is_some() {
            changes.tenant_id
        } else {
            existing.tenant_id
        };
        let role_id = if changes.role_id.is_some() {
            changes.role_id
        } else {
            existing.role_id
        };
        let description = if changes.description.is_some() {
            changes.description.clone()
        } else {
            existing.description
        };

        sqlx::query(
            r#"
            UPDATE ipam_vlan
            SET vid = $2, name = $3, status = $4, site_id = $5,
                group_id = $6, tenant_id = $7, role_id = $8, description = $9
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(vid)
        .bind(&name)
        .bind(&status)
        .bind(site_id)
        .bind(group_id)
        .bind(tenant_id)
        .bind(role_id)
        .bind(description.as_deref())
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update vlan: {}", e)))?;

        Ok(Some(VlanRecord {
            id,
            vid,
            name,
            status,
            site_id,
            group_id,
            tenant_id,
            role_id,
            description,
        }))
    }

    async fn bulk_create(
        &self,
        items: &[NewVlan],
    ) -> Result<super::super::bulk_insert::BulkCreateResult<VlanRecord>, AppError> {
        super::super::bulk_insert::execute(
            &self.pool,
            items,
            "bulk vlan insert",
            |connection, item| {
                Box::pin(async move {
                    let status = item
                        .status
                        .as_deref()
                        .filter(|s| !s.is_empty())
                        .unwrap_or("active")
                        .to_string();

                    sqlx::query(
                        r#"
                INSERT INTO ipam_vlan (
                    id, vid, name, status, site_id, group_id, tenant_id, role_id, description,
                    custom_field_data, comments, tags
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, '{}'::jsonb, '', '{}')
                "#,
                    )
                    .bind(item.id)
                    .bind(item.vid)
                    .bind(&item.name)
                    .bind(&status)
                    .bind(item.site_id)
                    .bind(item.group_id)
                    .bind(item.tenant_id)
                    .bind(item.role_id)
                    .bind(item.description.as_deref().unwrap_or(""))
                    .execute(connection)
                    .await
                    .map_err(|error| format!("Failed to insert vlan vid={}: {error}", item.vid))?;

                    Ok(VlanRecord {
                        id: item.id,
                        vid: item.vid,
                        name: item.name.clone(),
                        status,
                        site_id: item.site_id,
                        group_id: item.group_id,
                        tenant_id: item.tenant_id,
                        role_id: item.role_id,
                        description: item.description.clone(),
                    })
                })
            },
        )
        .await
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "ipam_vlan", id)
            .await
            .map(|_| ())
    }
}
