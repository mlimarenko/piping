//! Cable repository — full CRUD.

use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_id_filter, push_order_by, push_search_ilike,
    push_string_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

pub const CABLE_COLUMNS: &[ListColumn] = &[
    ListColumn::enum_filter("type", "type", "type", "inventory.resources.fields.type"),
    ListColumn::enum_filter(
        "status",
        "status",
        "status",
        "inventory.resources.fields.status",
    ),
    ListColumn::sortable("label", "label", "inventory.resources.fields.label"),
    ListColumn::sortable("color", "color", "inventory.resources.fields.color"),
    ListColumn::sortable("length", "length", "inventory.resources.fields.length"),
    ListColumn::enum_filter(
        "length_unit",
        "length_unit",
        "length_unit",
        "inventory.resources.fields.lengthUnit",
    ),
];

/// Minimal representation of a `dcim_cable` row exposed by the REST API.
#[derive(Debug, Clone)]
pub struct CableRecord {
    pub id: Uuid,
    pub type_: Option<String>,
    pub status: String,
    pub label: Option<String>,
    pub color: Option<String>,
    pub length: Option<f64>,
    pub length_unit: Option<String>,
}

/// Input for creating a new `dcim_cable` row.
#[derive(Debug, Clone)]
pub struct NewCable {
    pub id: Uuid,
    pub type_: Option<String>,
    pub status: Option<String>,
    pub label: Option<String>,
    pub color: Option<String>,
    pub length: Option<f64>,
    pub length_unit: Option<String>,
}

/// Fields that may be updated on an existing `dcim_cable` row.
#[derive(Debug, Clone)]
pub struct UpdateCable {
    pub type_: Option<String>,
    pub status: Option<String>,
    pub label: Option<String>,
    pub color: Option<String>,
    pub length: Option<f64>,
    pub length_unit: Option<String>,
}

/// Result of a bulk-create operation.
#[derive(Debug, Clone)]
pub struct BulkCableResult {
    pub created: Vec<CableRecord>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct CableFilter {
    pub ids: Option<Vec<Uuid>>,
    pub search: Option<String>,
    pub type_: Vec<String>,
    pub status: Vec<String>,
    pub length_unit: Vec<String>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait CableRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<CableRecord>, AppError>;
    async fn list(
        &self,
        filter: CableFilter,
        limit: Option<u32>,
        offset: Option<u32>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<CableRecord>, i64), AppError>;
    async fn create(&self, new_cable: &NewCable) -> Result<CableRecord, AppError>;
    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateCable,
    ) -> Result<Option<CableRecord>, AppError>;
    async fn bulk_create(&self, items: &[NewCable]) -> Result<BulkCableResult, AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxCableRepository {
    pool: PgPool,
}

impl SqlxCableRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn normalize_color(value: Option<&str>) -> Result<String, AppError> {
        let trimmed = value.unwrap_or("").trim();
        if trimmed.is_empty() {
            return Ok(String::new());
        }

        let body = trimmed
            .strip_prefix('#')
            .unwrap_or(trimmed)
            .to_ascii_lowercase();
        let expanded = match body.len() {
            3 if body.chars().all(|c| c.is_ascii_hexdigit()) => {
                body.chars().flat_map(|c| [c, c]).collect::<String>()
            }
            6 if body.chars().all(|c| c.is_ascii_hexdigit()) => body,
            _ => {
                return Err(AppError::Validation(
                    "Cable color must be a 3- or 6-digit hex color".to_string(),
                ));
            }
        };
        Ok(expanded)
    }

    fn parse_row(row: &PgRow) -> Result<CableRecord, AppError> {
        Ok(CableRecord {
            id: row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read cable id: {}", e)))?,
            type_: row
                .try_get::<Option<String>, _>("type")
                .map_err(|e| AppError::Database(format!("Failed to read cable type: {}", e)))?,
            status: row
                .try_get("status")
                .map_err(|e| AppError::Database(format!("Failed to read cable status: {}", e)))?,
            label: row
                .try_get::<Option<String>, _>("label")
                .map_err(|e| AppError::Database(format!("Failed to read cable label: {}", e)))?,
            color: row
                .try_get::<Option<String>, _>("color")
                .map_err(|e| AppError::Database(format!("Failed to read cable color: {}", e)))?,
            length: row
                .try_get::<Option<f64>, _>("length")
                .map_err(|e| AppError::Database(format!("Failed to read cable length: {}", e)))?,
            length_unit: row
                .try_get::<Option<String>, _>("length_unit")
                .map_err(|e| {
                    AppError::Database(format!("Failed to read cable length_unit: {}", e))
                })?,
        })
    }

    fn push_filters(builder: &mut QueryBuilder<'_, Postgres>, filter: &CableFilter) {
        let has_filter = filter.ids.is_some()
            || filter.search.is_some()
            || !filter.type_.is_empty()
            || !filter.status.is_empty()
            || !filter.length_unit.is_empty();
        if !has_filter {
            return;
        }

        builder.push(" WHERE ");
        let mut where_state = WhereState::after_where();
        push_id_filter(builder, &mut where_state, "id", filter.ids.as_deref());
        push_search_ilike(
            builder,
            &mut where_state,
            &["type", "status", "label", "color", "length", "length_unit"],
            filter.search.as_deref(),
        );
        push_string_in(builder, &mut where_state, "type", &filter.type_);
        push_string_in(builder, &mut where_state, "status", &filter.status);
        push_string_in(
            builder,
            &mut where_state,
            "length_unit",
            &filter.length_unit,
        );
    }
}

impl CableRepository for SqlxCableRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<CableRecord>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, type, status, label, color, length::float8 AS length, length_unit
            FROM dcim_cable
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load cable: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list(
        &self,
        filter: CableFilter,
        limit: Option<u32>,
        offset: Option<u32>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<CableRecord>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT id, type, status, label, color, length::float8 AS length, length_unit,
                   COUNT(*) OVER() AS total_count
            FROM dcim_cable
            "#,
        );
        Self::push_filters(&mut builder, &filter);
        push_order_by(&mut builder, CABLE_COLUMNS, sort.as_ref(), "id");
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list cables: {}", e)))?;

        let total = rows
            .first()
            .map(|row| row.try_get::<i64, _>("total_count"))
            .transpose()
            .map_err(|e| AppError::Database(format!("Failed to read cable total_count: {}", e)))?
            .unwrap_or(0);
        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok((items, total))
    }

    async fn create(&self, new_cable: &NewCable) -> Result<CableRecord, AppError> {
        let status = new_cable
            .status
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("connected")
            .to_string();
        let label = new_cable.label.clone().unwrap_or_default();
        let color = Self::normalize_color(new_cable.color.as_deref())?;

        sqlx::query(
            r#"
            INSERT INTO dcim_cable (
                id, custom_field_data, type, status, label, color, length,
                length_unit, comments, description, profile, tags
            )
            VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6, $7, '', '', '', '{}')
            "#,
        )
        .bind(new_cable.id)
        .bind(new_cable.type_.as_deref())
        .bind(&status)
        .bind(&label)
        .bind(&color)
        .bind(new_cable.length)
        .bind(new_cable.length_unit.as_deref())
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create cable: {}", e)))?;

        Ok(CableRecord {
            id: new_cable.id,
            type_: new_cable.type_.clone(),
            status,
            label: Some(label),
            color: Some(color),
            length: new_cable.length,
            length_unit: new_cable.length_unit.clone(),
        })
    }

    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateCable,
    ) -> Result<Option<CableRecord>, AppError> {
        let existing = self.get_by_id(id).await?;
        let Some(existing) = existing else {
            return Ok(None);
        };

        let type_ = if changes.type_.is_some() {
            changes.type_.clone()
        } else {
            existing.type_
        };
        let status = changes.status.clone().unwrap_or(existing.status);
        let label = if changes.label.is_some() {
            changes.label.clone()
        } else {
            existing.label
        };
        let color = if let Some(color) = changes.color.as_deref() {
            Some(Self::normalize_color(Some(color))?)
        } else {
            existing.color
        };
        let length = if changes.length.is_some() {
            changes.length
        } else {
            existing.length
        };
        let length_unit = if changes.length_unit.is_some() {
            changes.length_unit.clone()
        } else {
            existing.length_unit
        };

        sqlx::query(
            r#"
            UPDATE dcim_cable
            SET type = $2, status = $3, label = $4, color = $5, length = $6,
                length_unit = $7
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(type_.as_deref())
        .bind(&status)
        .bind(label.as_deref())
        .bind(color.as_deref())
        .bind(length)
        .bind(length_unit.as_deref())
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update cable: {}", e)))?;

        Ok(Some(CableRecord {
            id,
            type_,
            status,
            label,
            color,
            length,
            length_unit,
        }))
    }

    async fn bulk_create(&self, items: &[NewCable]) -> Result<BulkCableResult, AppError> {
        let mut created = Vec::new();
        let mut errors = Vec::new();

        let mut tx = super::super::bulk_insert::begin(&self.pool).await?;

        for item in items {
            let status = item
                .status
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or("connected")
                .to_string();
            let label = item.label.clone().unwrap_or_default();
            let color = match Self::normalize_color(item.color.as_deref()) {
                Ok(value) => value,
                Err(error) => {
                    errors.push(error.to_string());
                    continue;
                }
            };

            let result = super::super::bulk_insert::insert_row(
                &mut tx,
                sqlx::query(
                    r#"
                INSERT INTO dcim_cable (
                    id, custom_field_data, type, status, label, color, length,
                    length_unit, comments, description, profile, tags
                )
                VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6, $7, '', '', '', '{}')
                "#,
                )
                .bind(item.id)
                .bind(item.type_.as_deref())
                .bind(&status)
                .bind(&label)
                .bind(&color)
                .bind(item.length)
                .bind(item.length_unit.as_deref()),
            )
            .await?;

            match result {
                Ok(_) => created.push(CableRecord {
                    id: item.id,
                    type_: item.type_.clone(),
                    status,
                    label: Some(label),
                    color: Some(color),
                    length: item.length,
                    length_unit: item.length_unit.clone(),
                }),
                Err(e) => errors.push(format!("Failed to insert cable: {}", e)),
            }
        }

        tx.commit().await.map_err(|e| {
            AppError::Database(format!("Failed to commit bulk cable insert: {}", e))
        })?;

        Ok(BulkCableResult { created, errors })
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "dcim_cable", id)
            .await
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_ui_hex_color_to_netbox_storage_format() {
        assert_eq!(
            SqlxCableRepository::normalize_color(Some("#155DFC")).unwrap(),
            "155dfc",
        );
        assert_eq!(
            SqlxCableRepository::normalize_color(Some("abc")).unwrap(),
            "aabbcc",
        );
        assert_eq!(SqlxCableRepository::normalize_color(None).unwrap(), "");
    }

    #[test]
    fn rejects_invalid_cable_color() {
        let error = SqlxCableRepository::normalize_color(Some("#12g")).unwrap_err();
        assert!(matches!(error, AppError::Validation(_)));
    }
}
