//! Rack repository — full CRUD.

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

pub const RACK_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "r.name", "inventory.racks.columns.name"),
    ListColumn::enum_filter(
        "status",
        "r.status",
        "r.status",
        "inventory.racks.columns.status",
    ),
    ListColumn::fk_filter(
        "site",
        "s.name",
        "site_id",
        "r.site_id",
        "sites",
        "inventory.racks.columns.site",
    ),
    ListColumn::fk_filter(
        "tenant",
        "t.name",
        "tenant_id",
        "r.tenant_id",
        "tenants",
        "inventory.racks.columns.tenant",
    ),
    ListColumn::fk_filter(
        "rack_role",
        "rr.name",
        "role_id",
        "r.role_id",
        "rackRoles",
        "inventory.racks.columns.rackRole",
    ),
    ListColumn::sortable(
        "rack_type",
        "r.form_factor",
        "inventory.racks.columns.rackType",
    ),
    ListColumn::sortable(
        "facility_id",
        "r.facility_id",
        "inventory.racks.columns.facilityId",
    ),
    ListColumn::sortable("serial", "r.serial", "inventory.racks.columns.serial"),
    ListColumn::sortable(
        "asset_tag",
        "r.asset_tag",
        "inventory.racks.columns.assetTag",
    ),
    ListColumn::sortable("width", "r.width", "inventory.racks.columns.width"),
    ListColumn::sortable("u_height", "r.u_height", "inventory.racks.columns.uHeight"),
    ListColumn::sortable(
        "outer_width",
        "r.outer_width",
        "inventory.racks.columns.outerWidth",
    ),
    ListColumn::sortable(
        "outer_depth",
        "r.outer_depth",
        "inventory.racks.columns.outerDepth",
    ),
    ListColumn::enum_filter(
        "desc_units",
        "r.desc_units",
        "r.desc_units::text",
        "inventory.racks.columns.descUnits",
    ),
];

/// Minimal representation of a `dcim_rack` row exposed by the REST API.
#[derive(Debug, Clone)]
pub struct RackRecord {
    pub id: Uuid,
    pub name: String,
    pub site_id: Uuid,
    pub status: String,
    pub facility_id: Option<String>,
    pub tenant_id: Option<Uuid>,
    pub rack_role_id: Option<Uuid>,
    pub serial: Option<String>,
    pub asset_tag: Option<String>,
    pub type_: Option<String>,
    pub width: Option<i32>,
    pub u_height: Option<i32>,
    pub desc_units: bool,
    pub outer_width: Option<i32>,
    pub outer_depth: Option<i32>,
}

/// Hydrated list row for `dcim_rack` — includes site/tenant/role names resolved
/// via LEFT JOIN for the collection list endpoint.
#[derive(Debug, Clone)]
pub struct RackListItem {
    pub record: RackRecord,
    pub site_name: Option<String>,
    pub tenant_name: Option<String>,
    pub rack_role_name: Option<String>,
}

/// Input for creating a new `dcim_rack` row.
#[derive(Debug, Clone)]
pub struct NewRack {
    pub id: Uuid,
    pub name: String,
    pub site_id: Uuid,
    pub status: Option<String>,
    pub facility_id: Option<String>,
    pub tenant_id: Option<Uuid>,
    pub rack_role_id: Option<Uuid>,
    pub serial: Option<String>,
    pub asset_tag: Option<String>,
    pub type_: Option<String>,
    pub width: Option<i32>,
    pub u_height: Option<i32>,
    pub desc_units: Option<bool>,
    pub outer_width: Option<i32>,
    pub outer_depth: Option<i32>,
}

/// Fields that may be updated on an existing `dcim_rack` row.
#[derive(Debug, Clone)]
pub struct UpdateRack {
    pub name: Option<String>,
    pub site_id: Option<Uuid>,
    pub status: Option<String>,
    pub facility_id: Option<String>,
    pub tenant_id: Option<Uuid>,
    pub rack_role_id: Option<Uuid>,
    pub serial: Option<String>,
    pub asset_tag: Option<String>,
    pub type_: Option<String>,
    pub width: Option<i32>,
    pub u_height: Option<i32>,
    pub desc_units: Option<bool>,
    pub outer_width: Option<i32>,
    pub outer_depth: Option<i32>,
}

/// Result of a bulk-create operation.
#[derive(Debug, Clone)]
pub struct BulkRackResult {
    pub created: Vec<RackRecord>,
    pub errors: Vec<String>,
}

/// Rack list filter. Multi-value FK fields are `Vec<Uuid>` where an empty vector
/// means "no filter" and a non-empty vector emits `col IN (…)`. `status` is a
/// multi-value string filter. `sort` carries an already-allowlist-validated
/// [`SortSpec`].
#[derive(Debug, Clone, Default)]
pub struct RackFilter {
    pub ids: Option<Vec<Uuid>>,
    pub search: Option<String>,
    pub site_id: Vec<Uuid>,
    pub location_id: Vec<Uuid>,
    pub role_id: Vec<Uuid>,
    pub tenant_id: Vec<Uuid>,
    pub status: Vec<String>,
    pub desc_units: Vec<String>,
    pub sort: Option<SortSpec>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait RackRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<RackRecord>, AppError>;
    async fn list_hydrated(
        &self,
        filter: RackFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<RackListItem>, i64), AppError>;
    async fn create(&self, new_rack: &NewRack) -> Result<RackRecord, AppError>;
    async fn update(&self, id: Uuid, changes: &UpdateRack) -> Result<Option<RackRecord>, AppError>;
    async fn bulk_create(&self, items: &[NewRack]) -> Result<BulkRackResult, AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxRackRepository {
    pool: PgPool,
}

impl SqlxRackRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<RackRecord, AppError> {
        Ok(RackRecord {
            id: row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read rack id: {}", e)))?,
            name: row
                .try_get("name")
                .map_err(|e| AppError::Database(format!("Failed to read rack name: {}", e)))?,
            site_id: row
                .try_get("site_id")
                .map_err(|e| AppError::Database(format!("Failed to read rack site_id: {}", e)))?,
            status: row
                .try_get("status")
                .map_err(|e| AppError::Database(format!("Failed to read rack status: {}", e)))?,
            facility_id: row
                .try_get::<Option<String>, _>("facility_id")
                .map_err(|e| {
                    AppError::Database(format!("Failed to read rack facility_id: {}", e))
                })?,
            tenant_id: row
                .try_get::<Option<Uuid>, _>("tenant_id")
                .map_err(|e| AppError::Database(format!("Failed to read rack tenant_id: {}", e)))?,
            rack_role_id: row
                .try_get::<Option<Uuid>, _>("role_id")
                .map_err(|e| AppError::Database(format!("Failed to read rack role_id: {}", e)))?,
            serial: row
                .try_get::<Option<String>, _>("serial")
                .map_err(|e| AppError::Database(format!("Failed to read rack serial: {}", e)))?,
            asset_tag: row
                .try_get::<Option<String>, _>("asset_tag")
                .map_err(|e| AppError::Database(format!("Failed to read rack asset_tag: {}", e)))?,
            type_: row
                .try_get::<Option<String>, _>("form_factor")
                .map_err(|e| AppError::Database(format!("Failed to read rack type: {}", e)))?,
            width: row
                .try_get::<Option<i32>, _>("width")
                .map_err(|e| AppError::Database(format!("Failed to read rack width: {}", e)))?,
            u_height: row
                .try_get::<Option<i32>, _>("u_height")
                .map_err(|e| AppError::Database(format!("Failed to read rack u_height: {}", e)))?,
            desc_units: row.try_get("desc_units").map_err(|e| {
                AppError::Database(format!("Failed to read rack desc_units: {}", e))
            })?,
            outer_width: row.try_get::<Option<i32>, _>("outer_width").map_err(|e| {
                AppError::Database(format!("Failed to read rack outer_width: {}", e))
            })?,
            outer_depth: row.try_get::<Option<i32>, _>("outer_depth").map_err(|e| {
                AppError::Database(format!("Failed to read rack outer_depth: {}", e))
            })?,
        })
    }
}

impl RackRepository for SqlxRackRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<RackRecord>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, name, site_id, status, facility_id, tenant_id, role_id,
                   serial, asset_tag, form_factor,
                   width::integer AS width,
                   u_height::integer AS u_height,
                   desc_units,
                   outer_width::integer AS outer_width,
                   outer_depth::integer AS outer_depth
            FROM dcim_rack
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load rack: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_hydrated(
        &self,
        filter: RackFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<RackListItem>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT r.id, r.name, r.site_id, r.status, r.facility_id, r.tenant_id, r.role_id,
                   r.serial, r.asset_tag, r.form_factor,
                   r.width::integer AS width,
                   r.u_height::integer AS u_height,
                   r.desc_units,
                   r.outer_width::integer AS outer_width,
                   r.outer_depth::integer AS outer_depth,
                   s.name AS site_name,
                   t.name AS tenant_name,
                   rr.name AS rack_role_name,
                   COUNT(*) OVER() AS total_count
            FROM dcim_rack r
            LEFT JOIN dcim_site s ON s.id = r.site_id
            LEFT JOIN tenancy_tenant t ON t.id = r.tenant_id
            LEFT JOIN dcim_rackrole rr ON rr.id = r.role_id
            "#,
        );

        let has_filter = filter.ids.is_some()
            || filter.search.is_some()
            || !filter.site_id.is_empty()
            || !filter.location_id.is_empty()
            || !filter.role_id.is_empty()
            || !filter.tenant_id.is_empty()
            || !filter.desc_units.is_empty()
            || !filter.status.is_empty();
        if has_filter {
            builder.push(" WHERE ");
            let mut where_state = WhereState::after_where();
            push_id_filter(
                &mut builder,
                &mut where_state,
                "r.id",
                filter.ids.as_deref(),
            );
            push_search_ilike(
                &mut builder,
                &mut where_state,
                &[
                    "r.name",
                    "r.status",
                    "r.facility_id",
                    "r.serial",
                    "r.asset_tag",
                    "r.form_factor",
                    "s.name",
                    "t.name",
                    "rr.name",
                ],
                filter.search.as_deref(),
            );
            push_uuid_in(&mut builder, &mut where_state, "r.site_id", &filter.site_id);
            push_uuid_in(
                &mut builder,
                &mut where_state,
                "r.location_id",
                &filter.location_id,
            );
            push_uuid_in(&mut builder, &mut where_state, "r.role_id", &filter.role_id);
            push_uuid_in(
                &mut builder,
                &mut where_state,
                "r.tenant_id",
                &filter.tenant_id,
            );
            push_string_in(&mut builder, &mut where_state, "r.status", &filter.status);
            push_string_in(
                &mut builder,
                &mut where_state,
                "r.desc_units::text",
                &filter.desc_units,
            );
        }

        push_order_by(
            &mut builder,
            RACK_COLUMNS,
            filter.sort.as_ref(),
            "r.name ASC, r.id",
        );
        if filter.sort.is_some() {
            builder.push(", r.id");
        }
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list racks hydrated: {}", e)))?;

        let total = rows
            .first()
            .map(|row| row.try_get::<i64, _>("total_count"))
            .transpose()
            .map_err(|e| AppError::Database(format!("Failed to read rack total_count: {}", e)))?
            .unwrap_or(0);
        let items = rows
            .iter()
            .map(|row| {
                Ok::<RackListItem, AppError>(RackListItem {
                    record: Self::parse_row(row)?,
                    site_name: row.try_get("site_name").ok(),
                    tenant_name: row.try_get("tenant_name").ok(),
                    rack_role_name: row.try_get("rack_role_name").ok(),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok((items, total))
    }

    async fn create(&self, new_rack: &NewRack) -> Result<RackRecord, AppError> {
        let status = new_rack
            .status
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("active")
            .to_string();
        let desc_units = new_rack.desc_units.unwrap_or(false);

        sqlx::query(
            r#"
            INSERT INTO dcim_rack (
                id, name, site_id, status, facility_id, tenant_id, role_id,
                serial, asset_tag, form_factor, width, u_height, desc_units, outer_width, outer_depth,
                custom_field_data, comments, description, starting_unit, tags
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15,
                    '{}'::jsonb, '', '', 1, '{}')
            "#,
        )
        .bind(new_rack.id)
        .bind(&new_rack.name)
        .bind(new_rack.site_id)
        .bind(&status)
        .bind(new_rack.facility_id.as_deref())
        .bind(new_rack.tenant_id)
        .bind(new_rack.rack_role_id)
        .bind(new_rack.serial.as_deref())
        .bind(new_rack.asset_tag.as_deref())
        .bind(new_rack.type_.as_deref())
        .bind(new_rack.width)
        .bind(new_rack.u_height)
        .bind(desc_units)
        .bind(new_rack.outer_width)
        .bind(new_rack.outer_depth)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create rack: {}", e)))?;

        Ok(RackRecord {
            id: new_rack.id,
            name: new_rack.name.clone(),
            site_id: new_rack.site_id,
            status,
            facility_id: new_rack.facility_id.clone(),
            tenant_id: new_rack.tenant_id,
            rack_role_id: new_rack.rack_role_id,
            serial: new_rack.serial.clone(),
            asset_tag: new_rack.asset_tag.clone(),
            type_: new_rack.type_.clone(),
            width: new_rack.width,
            u_height: new_rack.u_height,
            desc_units,
            outer_width: new_rack.outer_width,
            outer_depth: new_rack.outer_depth,
        })
    }

    async fn update(&self, id: Uuid, changes: &UpdateRack) -> Result<Option<RackRecord>, AppError> {
        let existing = self.get_by_id(id).await?;
        let Some(existing) = existing else {
            return Ok(None);
        };

        let name = changes.name.clone().unwrap_or(existing.name);
        let site_id = changes.site_id.unwrap_or(existing.site_id);
        let status = changes.status.clone().unwrap_or(existing.status);
        let desc_units = changes.desc_units.unwrap_or(existing.desc_units);
        let facility_id = if changes.facility_id.is_some() {
            changes.facility_id.clone()
        } else {
            existing.facility_id
        };
        let tenant_id = if changes.tenant_id.is_some() {
            changes.tenant_id
        } else {
            existing.tenant_id
        };
        let rack_role_id = if changes.rack_role_id.is_some() {
            changes.rack_role_id
        } else {
            existing.rack_role_id
        };
        let serial = if changes.serial.is_some() {
            changes.serial.clone()
        } else {
            existing.serial
        };
        let asset_tag = if changes.asset_tag.is_some() {
            changes.asset_tag.clone()
        } else {
            existing.asset_tag
        };
        let type_ = if changes.type_.is_some() {
            changes.type_.clone()
        } else {
            existing.type_
        };
        let width = if changes.width.is_some() {
            changes.width
        } else {
            existing.width
        };
        let u_height = if changes.u_height.is_some() {
            changes.u_height
        } else {
            existing.u_height
        };
        let outer_width = if changes.outer_width.is_some() {
            changes.outer_width
        } else {
            existing.outer_width
        };
        let outer_depth = if changes.outer_depth.is_some() {
            changes.outer_depth
        } else {
            existing.outer_depth
        };

        sqlx::query(
            r#"
            UPDATE dcim_rack
            SET name = $2, site_id = $3, status = $4, facility_id = $5,
                tenant_id = $6, role_id = $7, serial = $8, asset_tag = $9,
                form_factor = $10, width = $11, u_height = $12, desc_units = $13,
                outer_width = $14, outer_depth = $15
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(&name)
        .bind(site_id)
        .bind(&status)
        .bind(facility_id.as_deref())
        .bind(tenant_id)
        .bind(rack_role_id)
        .bind(serial.as_deref())
        .bind(asset_tag.as_deref())
        .bind(type_.as_deref())
        .bind(width)
        .bind(u_height)
        .bind(desc_units)
        .bind(outer_width)
        .bind(outer_depth)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update rack: {}", e)))?;

        Ok(Some(RackRecord {
            id,
            name,
            site_id,
            status,
            facility_id,
            tenant_id,
            rack_role_id,
            serial,
            asset_tag,
            type_,
            width,
            u_height,
            desc_units,
            outer_width,
            outer_depth,
        }))
    }

    async fn bulk_create(&self, items: &[NewRack]) -> Result<BulkRackResult, AppError> {
        let mut created = Vec::new();
        let mut errors = Vec::new();

        let mut tx = super::super::bulk_insert::begin(&self.pool).await?;

        for item in items {
            let status = item
                .status
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or("active")
                .to_string();
            let desc_units = item.desc_units.unwrap_or(false);

            let result = super::super::bulk_insert::insert_row(
                &mut tx,
                sqlx::query(
                r#"
                INSERT INTO dcim_rack (
                    id, name, site_id, status, facility_id, tenant_id, role_id,
                    serial, asset_tag, form_factor, width, u_height, desc_units, outer_width, outer_depth,
                    custom_field_data, comments, description, starting_unit, tags
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15,
                        '{}'::jsonb, '', '', 1, '{}')
                "#,
            )
            .bind(item.id)
            .bind(&item.name)
            .bind(item.site_id)
            .bind(&status)
            .bind(item.facility_id.as_deref())
            .bind(item.tenant_id)
            .bind(item.rack_role_id)
            .bind(item.serial.as_deref())
            .bind(item.asset_tag.as_deref())
            .bind(item.type_.as_deref())
            .bind(item.width)
            .bind(item.u_height)
            .bind(desc_units)
            .bind(item.outer_width)
            .bind(item.outer_depth),
            )
            .await?;

            match result {
                Ok(_) => created.push(RackRecord {
                    id: item.id,
                    name: item.name.clone(),
                    site_id: item.site_id,
                    status,
                    facility_id: item.facility_id.clone(),
                    tenant_id: item.tenant_id,
                    rack_role_id: item.rack_role_id,
                    serial: item.serial.clone(),
                    asset_tag: item.asset_tag.clone(),
                    type_: item.type_.clone(),
                    width: item.width,
                    u_height: item.u_height,
                    desc_units,
                    outer_width: item.outer_width,
                    outer_depth: item.outer_depth,
                }),
                Err(e) => errors.push(format!("Failed to insert rack '{}': {}", item.name, e)),
            }
        }

        tx.commit()
            .await
            .map_err(|e| AppError::Database(format!("Failed to commit bulk rack insert: {}", e)))?;

        Ok(BulkRackResult { created, errors })
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        // The DELETE contract for this entity advertises 404; deleting an
        // absent row must not silently answer 204.
        let deleted =
            super::super::delete_guard::delete_inventory_record(&self.pool, "dcim_rack", id)
                .await?;
        if deleted == 0 {
            return Err(AppError::NotFound(format!("Rack '{id}' not found")));
        }
        Ok(())
    }
}
