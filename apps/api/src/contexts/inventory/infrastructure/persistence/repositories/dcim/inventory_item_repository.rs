//! Inventory item repository — full CRUD over `dcim_inventoryitem`.
//!
//! Runtime sqlx only (`sqlx::query` + `.bind`) so the crate type-checks and the
//! contract verifies via `cargo check` without a database.
//!
//! NOTE: `dcim_inventoryitem` HAS the piping envelope columns (`external_id`,
//! `piping_synced_at`, `tags`), but this repository neither reads nor
//! writes them — as with the other half of the inventory repositories.
//! Nothing populates `external_id` anywhere today, so this is an
//! unfinished envelope rather than a data-loss path; wire it up here when
//! the sync engine actually starts writing it.
//!
//! MPTT columns (`lft`, `rght`, `tree_id`, `level`) are seeded to 0 on INSERT
//! and are not updated by this repository — tree maintenance is left to the
//! application layer or a future MPTT migration.

use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, filter_column, push_order_by, push_search_ilike,
    push_string_in, push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

pub const INVENTORY_ITEM_COLUMNS: &[ListColumn] = &[
    ListColumn::fk_filter(
        "device_id",
        "device_id",
        "device_id",
        "device_id",
        "devices",
        "inventory.inventoryItems.columns.device",
    ),
    ListColumn::fk_filter(
        "parent_id",
        "parent_id",
        "parent_id",
        "parent_id",
        "inventoryItems",
        "inventory.inventoryItems.columns.parent",
    ),
    ListColumn::fk_filter(
        "role_id",
        "role_id",
        "role_id",
        "role_id",
        "inventoryItemRoles",
        "inventory.inventoryItems.columns.role",
    ),
    ListColumn::fk_filter(
        "manufacturer_id",
        "manufacturer_id",
        "manufacturer_id",
        "manufacturer_id",
        "manufacturers",
        "inventory.inventoryItems.columns.manufacturer",
    ),
    ListColumn::sortable("name", "name", "inventory.inventoryItems.columns.name"),
    ListColumn::sortable("label", "label", "inventory.inventoryItems.columns.label"),
    ListColumn::sortable(
        "part_id",
        "part_id",
        "inventory.inventoryItems.columns.partId",
    ),
    ListColumn::sortable(
        "serial",
        "serial",
        "inventory.inventoryItems.columns.serial",
    ),
    ListColumn::sortable(
        "asset_tag",
        "asset_tag",
        "inventory.inventoryItems.columns.assetTag",
    ),
    ListColumn::enum_filter(
        "discovered",
        "discovered",
        "discovered::text",
        "inventory.inventoryItems.columns.discovered",
    ),
    ListColumn::sortable(
        "description",
        "description",
        "inventory.inventoryItems.columns.description",
    ),
    ListColumn::enum_filter(
        "status",
        "status",
        "status",
        "inventory.inventoryItems.columns.status",
    ),
    ListColumn::fk_filter(
        "component_type_id",
        "component_type_id",
        "component_type_id",
        "component_type_id",
        "contentTypes",
        "inventory.inventoryItems.columns.componentType",
    ),
    ListColumn::fk_filter(
        "component_id",
        "component_id",
        "component_id",
        "component_id",
        "components",
        "inventory.inventoryItems.columns.component",
    ),
];

/// Representation of a `dcim_inventoryitem` row exposed by the REST API.
#[derive(Debug, Clone)]
pub struct InventoryItemRecord {
    pub id: Uuid,
    pub device_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub role_id: Option<Uuid>,
    pub manufacturer_id: Option<Uuid>,
    pub name: String,
    pub label: String,
    pub part_id: String,
    pub serial: String,
    pub asset_tag: Option<String>,
    pub discovered: bool,
    pub description: String,
    pub status: String,
    pub component_type_id: Option<Uuid>,
    pub component_id: Option<Uuid>,
    pub custom_fields: Option<serde_json::Value>,
}

/// Input for creating a new `dcim_inventoryitem` row.
#[derive(Debug, Clone)]
pub struct NewInventoryItem {
    pub id: Uuid,
    pub device_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub role_id: Option<Uuid>,
    pub manufacturer_id: Option<Uuid>,
    pub name: String,
    pub label: Option<String>,
    pub part_id: Option<String>,
    pub serial: Option<String>,
    pub asset_tag: Option<String>,
    pub discovered: Option<bool>,
    pub description: Option<String>,
    pub status: Option<String>,
    pub component_type_id: Option<Uuid>,
    pub component_id: Option<Uuid>,
}

/// Fields that may be updated on an existing `dcim_inventoryitem` row.
#[derive(Debug, Clone)]
pub struct UpdateInventoryItem {
    pub parent_id: Option<Uuid>,
    pub role_id: Option<Uuid>,
    pub manufacturer_id: Option<Uuid>,
    pub name: Option<String>,
    pub label: Option<String>,
    pub part_id: Option<String>,
    pub serial: Option<String>,
    pub asset_tag: Option<String>,
    pub discovered: Option<bool>,
    pub description: Option<String>,
    pub status: Option<String>,
    pub component_type_id: Option<Uuid>,
    pub component_id: Option<Uuid>,
}

/// Result of a bulk-create operation.
#[derive(Debug, Clone)]
pub struct BulkInventoryItemResult {
    pub created: Vec<InventoryItemRecord>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct InventoryItemFilter {
    pub device_id: Vec<Uuid>,
    pub parent_id: Vec<Uuid>,
    pub role_id: Vec<Uuid>,
    pub manufacturer_id: Vec<Uuid>,
    pub discovered: Vec<String>,
    pub status: Vec<String>,
    pub component_type_id: Vec<Uuid>,
    pub component_id: Vec<Uuid>,
    pub search: Option<String>,
    pub sort: Option<SortSpec>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait InventoryItemRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<InventoryItemRecord>, AppError>;
    async fn list(
        &self,
        filter: InventoryItemFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<InventoryItemRecord>, i64), AppError>;
    async fn create(&self, new_item: &NewInventoryItem) -> Result<InventoryItemRecord, AppError>;
    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateInventoryItem,
    ) -> Result<Option<InventoryItemRecord>, AppError>;
    async fn bulk_create(
        &self,
        items: &[NewInventoryItem],
    ) -> Result<BulkInventoryItemResult, AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxInventoryItemRepository {
    pool: PgPool,
}

impl SqlxInventoryItemRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<InventoryItemRecord, AppError> {
        Ok(InventoryItemRecord {
            id: row.try_get("id").map_err(|e| {
                AppError::Database(format!("Failed to read inventory_item id: {}", e))
            })?,
            device_id: row.try_get("device_id").map_err(|e| {
                AppError::Database(format!("Failed to read inventory_item device_id: {}", e))
            })?,
            parent_id: row.try_get::<Option<Uuid>, _>("parent_id").ok().flatten(),
            role_id: row.try_get::<Option<Uuid>, _>("role_id").ok().flatten(),
            manufacturer_id: row
                .try_get::<Option<Uuid>, _>("manufacturer_id")
                .ok()
                .flatten(),
            name: row.try_get("name").map_err(|e| {
                AppError::Database(format!("Failed to read inventory_item name: {}", e))
            })?,
            label: row.try_get("label").map_err(|e| {
                AppError::Database(format!("Failed to read inventory_item label: {}", e))
            })?,
            part_id: row.try_get("part_id").map_err(|e| {
                AppError::Database(format!("Failed to read inventory_item part_id: {}", e))
            })?,
            serial: row.try_get("serial").map_err(|e| {
                AppError::Database(format!("Failed to read inventory_item serial: {}", e))
            })?,
            asset_tag: row.try_get::<Option<String>, _>("asset_tag").ok().flatten(),
            discovered: row.try_get("discovered").map_err(|e| {
                AppError::Database(format!("Failed to read inventory_item discovered: {}", e))
            })?,
            description: row.try_get("description").map_err(|e| {
                AppError::Database(format!("Failed to read inventory_item description: {}", e))
            })?,
            status: row.try_get("status").map_err(|e| {
                AppError::Database(format!("Failed to read inventory_item status: {}", e))
            })?,
            component_type_id: row
                .try_get::<Option<Uuid>, _>("component_type_id")
                .ok()
                .flatten(),
            component_id: row
                .try_get::<Option<Uuid>, _>("component_id")
                .ok()
                .flatten(),
            custom_fields: row
                .try_get::<Option<serde_json::Value>, _>("custom_field_data")
                .ok()
                .flatten(),
        })
    }

    fn push_list_filters(
        builder: &mut QueryBuilder<'_, Postgres>,
        filter: &InventoryItemFilter,
    ) -> Result<(), AppError> {
        let mut where_state = WhereState::new();
        push_uuid_in(
            builder,
            &mut where_state,
            filter_column(INVENTORY_ITEM_COLUMNS, "inventory item", "device_id")?.sql,
            &filter.device_id,
        );
        push_uuid_in(
            builder,
            &mut where_state,
            filter_column(INVENTORY_ITEM_COLUMNS, "inventory item", "parent_id")?.sql,
            &filter.parent_id,
        );
        push_uuid_in(
            builder,
            &mut where_state,
            filter_column(INVENTORY_ITEM_COLUMNS, "inventory item", "role_id")?.sql,
            &filter.role_id,
        );
        push_uuid_in(
            builder,
            &mut where_state,
            filter_column(INVENTORY_ITEM_COLUMNS, "inventory item", "manufacturer_id")?.sql,
            &filter.manufacturer_id,
        );
        push_string_in(
            builder,
            &mut where_state,
            filter_column(INVENTORY_ITEM_COLUMNS, "inventory item", "discovered")?.sql,
            &filter.discovered,
        );
        push_string_in(
            builder,
            &mut where_state,
            filter_column(INVENTORY_ITEM_COLUMNS, "inventory item", "status")?.sql,
            &filter.status,
        );
        push_uuid_in(
            builder,
            &mut where_state,
            filter_column(
                INVENTORY_ITEM_COLUMNS,
                "inventory item",
                "component_type_id",
            )?
            .sql,
            &filter.component_type_id,
        );
        push_uuid_in(
            builder,
            &mut where_state,
            filter_column(INVENTORY_ITEM_COLUMNS, "inventory item", "component_id")?.sql,
            &filter.component_id,
        );
        push_search_ilike(
            builder,
            &mut where_state,
            &[
                "name",
                "label",
                "part_id",
                "serial",
                "asset_tag",
                "discovered",
                "description",
                "status",
            ],
            filter.search.as_deref(),
        );
        Ok(())
    }

    async fn count_filtered(&self, filter: &InventoryItemFilter) -> Result<i64, AppError> {
        let mut builder = QueryBuilder::new("SELECT COUNT(*) FROM dcim_inventoryitem");
        Self::push_list_filters(&mut builder, filter)?;
        builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to count inventory_items: {}", e)))
    }
}

impl InventoryItemRepository for SqlxInventoryItemRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<InventoryItemRecord>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, device_id, parent_id, role_id, manufacturer_id,
                   name, label, part_id, serial, asset_tag, discovered,
                   description, status, component_type_id, component_id,
                   custom_field_data
            FROM dcim_inventoryitem
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load inventory_item: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list(
        &self,
        filter: InventoryItemFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<InventoryItemRecord>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT id, device_id, parent_id, role_id, manufacturer_id,
                   name, label, part_id, serial, asset_tag, discovered,
                   description, status, component_type_id, component_id,
                   custom_field_data,
                   COUNT(*) OVER() AS total_count
            FROM dcim_inventoryitem
            "#,
        );
        Self::push_list_filters(&mut builder, &filter)?;
        push_order_by(
            &mut builder,
            INVENTORY_ITEM_COLUMNS,
            filter.sort.as_ref(),
            "name",
        );
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows =
            builder.build().fetch_all(&self.pool).await.map_err(|e| {
                AppError::Database(format!("Failed to list inventory_items: {}", e))
            })?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!("Failed to read inventory_item total_count: {}", e))
            })?,
            None => self.count_filtered(&filter).await?,
        };

        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<InventoryItemRecord>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, new_item: &NewInventoryItem) -> Result<InventoryItemRecord, AppError> {
        let label = new_item.label.clone().unwrap_or_default();
        let part_id = new_item.part_id.clone().unwrap_or_default();
        let serial = new_item.serial.clone().unwrap_or_default();
        let discovered = new_item.discovered.unwrap_or(false);
        let description = new_item.description.clone().unwrap_or_default();
        let status = new_item
            .status
            .clone()
            .unwrap_or_else(|| "active".to_string());

        sqlx::query(
            r#"
            INSERT INTO dcim_inventoryitem (
                id, custom_field_data, device_id, parent_id, role_id, manufacturer_id,
                name, label, part_id, serial, asset_tag, discovered, description,
                status, component_type_id, component_id,
                lft, rght, tree_id, level
            )
            VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15,
                    0, 0, 0, 0)
            "#,
        )
        .bind(new_item.id)
        .bind(new_item.device_id)
        .bind(new_item.parent_id)
        .bind(new_item.role_id)
        .bind(new_item.manufacturer_id)
        .bind(&new_item.name)
        .bind(&label)
        .bind(&part_id)
        .bind(&serial)
        .bind(&new_item.asset_tag)
        .bind(discovered)
        .bind(&description)
        .bind(&status)
        .bind(new_item.component_type_id)
        .bind(new_item.component_id)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create inventory_item: {}", e)))?;

        Ok(InventoryItemRecord {
            id: new_item.id,
            device_id: new_item.device_id,
            parent_id: new_item.parent_id,
            role_id: new_item.role_id,
            manufacturer_id: new_item.manufacturer_id,
            name: new_item.name.clone(),
            label,
            part_id,
            serial,
            asset_tag: new_item.asset_tag.clone(),
            discovered,
            description,
            status,
            component_type_id: new_item.component_type_id,
            component_id: new_item.component_id,
            custom_fields: None,
        })
    }

    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateInventoryItem,
    ) -> Result<Option<InventoryItemRecord>, AppError> {
        let Some(existing) = self.get_by_id(id).await? else {
            return Ok(None);
        };

        let name = changes.name.clone().unwrap_or(existing.name);
        let label = changes.label.clone().unwrap_or(existing.label);
        let part_id = changes.part_id.clone().unwrap_or(existing.part_id);
        let serial = changes.serial.clone().unwrap_or(existing.serial);
        let asset_tag = if changes.asset_tag.is_some() {
            changes.asset_tag.clone()
        } else {
            existing.asset_tag
        };
        let discovered = changes.discovered.unwrap_or(existing.discovered);
        let description = changes.description.clone().unwrap_or(existing.description);
        let status = changes.status.clone().unwrap_or(existing.status);
        let parent_id = if changes.parent_id.is_some() {
            changes.parent_id
        } else {
            existing.parent_id
        };
        let role_id = if changes.role_id.is_some() {
            changes.role_id
        } else {
            existing.role_id
        };
        let manufacturer_id = if changes.manufacturer_id.is_some() {
            changes.manufacturer_id
        } else {
            existing.manufacturer_id
        };
        let component_type_id = if changes.component_type_id.is_some() {
            changes.component_type_id
        } else {
            existing.component_type_id
        };
        let component_id = if changes.component_id.is_some() {
            changes.component_id
        } else {
            existing.component_id
        };

        sqlx::query(
            r#"
            UPDATE dcim_inventoryitem
            SET name = $2, label = $3, part_id = $4, serial = $5, asset_tag = $6,
                discovered = $7, description = $8, status = $9,
                parent_id = $10, role_id = $11, manufacturer_id = $12,
                component_type_id = $13, component_id = $14
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(&name)
        .bind(&label)
        .bind(&part_id)
        .bind(&serial)
        .bind(&asset_tag)
        .bind(discovered)
        .bind(&description)
        .bind(&status)
        .bind(parent_id)
        .bind(role_id)
        .bind(manufacturer_id)
        .bind(component_type_id)
        .bind(component_id)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update inventory_item: {}", e)))?;

        Ok(Some(InventoryItemRecord {
            id,
            device_id: existing.device_id,
            parent_id,
            role_id,
            manufacturer_id,
            name,
            label,
            part_id,
            serial,
            asset_tag,
            discovered,
            description,
            status,
            component_type_id,
            component_id,
            custom_fields: existing.custom_fields,
        }))
    }

    async fn bulk_create(
        &self,
        items: &[NewInventoryItem],
    ) -> Result<BulkInventoryItemResult, AppError> {
        let mut created = Vec::new();
        let mut errors = Vec::new();

        let mut tx = super::super::bulk_insert::begin(&self.pool).await?;

        for item in items {
            let label = item.label.clone().unwrap_or_default();
            let part_id = item.part_id.clone().unwrap_or_default();
            let serial = item.serial.clone().unwrap_or_default();
            let discovered = item.discovered.unwrap_or(false);
            let description = item.description.clone().unwrap_or_default();
            let status = item.status.clone().unwrap_or_else(|| "active".to_string());

            let result = super::super::bulk_insert::insert_row(
                &mut tx,
                sqlx::query(
                r#"
                INSERT INTO dcim_inventoryitem (
                    id, custom_field_data, device_id, parent_id, role_id, manufacturer_id,
                    name, label, part_id, serial, asset_tag, discovered, description,
                    status, component_type_id, component_id,
                    lft, rght, tree_id, level
                )
                VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15,
                        0, 0, 0, 0)
                "#,
            )
            .bind(item.id)
            .bind(item.device_id)
            .bind(item.parent_id)
            .bind(item.role_id)
            .bind(item.manufacturer_id)
            .bind(&item.name)
            .bind(&label)
            .bind(&part_id)
            .bind(&serial)
            .bind(&item.asset_tag)
            .bind(discovered)
            .bind(&description)
            .bind(&status)
            .bind(item.component_type_id)
            .bind(item.component_id),
            )
            .await?;

            match result {
                Ok(_) => created.push(InventoryItemRecord {
                    id: item.id,
                    device_id: item.device_id,
                    parent_id: item.parent_id,
                    role_id: item.role_id,
                    manufacturer_id: item.manufacturer_id,
                    name: item.name.clone(),
                    label,
                    part_id,
                    serial,
                    asset_tag: item.asset_tag.clone(),
                    discovered,
                    description,
                    status,
                    component_type_id: item.component_type_id,
                    component_id: item.component_id,
                    custom_fields: None,
                }),
                Err(e) => {
                    errors.push(format!(
                        "Failed to insert inventory_item '{}': {}",
                        item.name, e
                    ));
                }
            }
        }

        tx.commit().await.map_err(|e| {
            AppError::Database(format!(
                "Failed to commit bulk inventory_item insert: {}",
                e
            ))
        })?;

        Ok(BulkInventoryItemResult { created, errors })
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "dcim_inventoryitem", id)
            .await
            .map(|_| ())
    }
}
