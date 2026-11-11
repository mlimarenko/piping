//! Pure SELECT-only content-type lookup for the WRITE path.
//!
//! Zero-legacy: there is no write-time `INSERT ... ON CONFLICT` that lazily
//! mints `inventory_content_type` rows. Every legitimate content type is seeded
//! by the `seed_content_types` migration from the
//! [`crate::contexts::inventory::domain::content_type::CONTENT_TYPES`] registry,
//! so a known type always resolves. An unknown/unseeded `(app_label, model)` on
//! write is a client error (`AppError::Validation` → 400), never an INSERT.
//!
//! One SQL string, two thin executor wrappers (pool for repository writes,
//! transaction for repository-scoped write paths).

use sqlx::{PgPool, Postgres, QueryBuilder, Row, Transaction};
use uuid::Uuid;

use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_string_in,
};
use crate::errors::AppError;

const LOOKUP_SQL: &str =
    "SELECT id FROM inventory_content_type WHERE app_label = $1 AND model = $2";

pub const CONTENT_TYPE_COLUMNS: &[ListColumn] = &[
    ListColumn::enum_filter(
        "app_label",
        "app_label",
        "app_label",
        "customFields.contentTypes.appLabel",
    ),
    ListColumn::enum_filter("model", "model", "model", "customFields.contentTypes.model"),
];

/// One seeded `inventory_content_type` row: the install-portable id plus its
/// `(app_label, model)` segments. Read-only projection for the listing path.
#[derive(Debug, Clone)]
pub struct ContentTypeRow {
    pub id: Uuid,
    pub app_label: String,
    pub model: String,
}

/// List every seeded `inventory_content_type` row, ordered by `(app_label,
/// model)`. The id is minted per-install via uuidv7, so callers that need a
/// stable cross-install handle use the `app_label.model` token derived from the
/// returned segments.
pub async fn list_content_types(
    pool: &PgPool,
    sort: Option<SortSpec>,
    app_labels: &[String],
    models: &[String],
) -> Result<Vec<ContentTypeRow>, AppError> {
    let mut builder = QueryBuilder::new("SELECT id, app_label, model FROM inventory_content_type");
    let mut where_state = WhereState::new();
    let app_label_column = CONTENT_TYPE_COLUMNS
        .iter()
        .find_map(|column| column.filter.filter(|filter| filter.param == "app_label"))
        .ok_or_else(|| {
            AppError::Database("content_type app_label filter is not registered".to_string())
        })?;
    push_string_in(
        &mut builder,
        &mut where_state,
        app_label_column.sql,
        app_labels,
    );
    let model_column = CONTENT_TYPE_COLUMNS
        .iter()
        .find_map(|column| column.filter.filter(|filter| filter.param == "model"))
        .ok_or_else(|| {
            AppError::Database("content_type model filter is not registered".to_string())
        })?;
    push_string_in(&mut builder, &mut where_state, model_column.sql, models);
    push_order_by(
        &mut builder,
        CONTENT_TYPE_COLUMNS,
        sort.as_ref(),
        "app_label, model",
    );
    let rows = builder
        .build()
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to list content types: {e}")))?;
    rows.iter()
        .map(|row| {
            Ok(ContentTypeRow {
                id: row
                    .try_get("id")
                    .map_err(|e| AppError::Database(format!("content_type.id: {e}")))?,
                app_label: row
                    .try_get("app_label")
                    .map_err(|e| AppError::Database(format!("content_type.app_label: {e}")))?,
                model: row
                    .try_get("model")
                    .map_err(|e| AppError::Database(format!("content_type.model: {e}")))?,
            })
        })
        .collect()
}

/// Look up a content-type id by `(app_label, model)` against a pool. Returns
/// `None` when no seeded row exists.
pub async fn lookup_content_type_id(
    pool: &PgPool,
    app_label: &str,
    model: &str,
) -> Result<Option<Uuid>, AppError> {
    let row = sqlx::query(LOOKUP_SQL)
        .bind(app_label)
        .bind(model)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load content type: {e}")))?;
    row.map(|row| {
        row.try_get("id")
            .map_err(|e| AppError::Database(format!("Failed to read content type id: {e}")))
    })
    .transpose()
}

/// Transaction variant of [`lookup_content_type_id`] for repository flows that
/// already own a wider inventory write transaction.
pub async fn lookup_content_type_id_tx(
    tx: &mut Transaction<'_, Postgres>,
    app_label: &str,
    model: &str,
) -> Result<Option<Uuid>, AppError> {
    let row = sqlx::query(LOOKUP_SQL)
        .bind(app_label)
        .bind(model)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load content type: {e}")))?;
    row.map(|row| {
        row.try_get("id")
            .map_err(|e| AppError::Database(format!("Failed to read content type id: {e}")))
    })
    .transpose()
}
