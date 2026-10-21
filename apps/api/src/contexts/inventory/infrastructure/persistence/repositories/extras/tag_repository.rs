use sqlx::{PgPool, QueryBuilder};
use uuid::Uuid;

use crate::contexts::inventory::domain::extras::tag::Tag;
use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_search_ilike,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

pub const TAG_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "name", "inventory.resources.fields.name"),
    ListColumn::sortable("slug", "slug", "inventory.resources.fields.slug"),
    ListColumn::sortable("color", "color", "inventory.resources.fields.color"),
    ListColumn::sortable("weight", "weight", "inventory.resources.fields.tagWeight"),
    ListColumn::sortable(
        "description",
        "description",
        "inventory.resources.fields.description",
    ),
];

/// Read-only access to the native `inventory_tag` definition catalogue.
///
/// Tag definitions are synced from the source; the API exposes no manual
/// create/update/delete affordance, so this repository intentionally omits the
/// write methods its sibling CRUD repositories carry.
#[derive(Clone)]
pub struct SqlxTagRepository {
    pool: PgPool,
}

impl SqlxTagRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn get_by_id(&self, id: Uuid) -> Result<Option<Tag>, AppError> {
        // Runtime `query_as` (not the compile-time macro) keeps this read off
        // the SQLx query cache: no new offline metadata entry is required.
        sqlx::query_as::<_, Tag>(
            "SELECT id, name, slug, color, description, weight FROM inventory_tag WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| AppError::Database("Failed to load tag".into()))
    }

    pub async fn list_paginated(
        &self,
        search: Option<String>,
        limit: Option<u32>,
        offset: Option<u32>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<Tag>, i64), AppError> {
        let mut count = QueryBuilder::new("SELECT COUNT(*) FROM inventory_tag");
        let mut where_state = WhereState::new();
        push_search_ilike(
            &mut count,
            &mut where_state,
            &["name", "slug", "description"],
            search.as_deref(),
        );
        let total = count
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_err(|_| AppError::Database("Failed to count tags".into()))?;

        let mut query = QueryBuilder::new(
            "SELECT id, name, slug, color, description, weight FROM inventory_tag",
        );
        let mut where_state = WhereState::new();
        push_search_ilike(
            &mut query,
            &mut where_state,
            &["name", "slug", "description"],
            search.as_deref(),
        );
        push_order_by(&mut query, TAG_COLUMNS, sort.as_ref(), "name");
        query
            .push(" LIMIT ")
            .push_bind(i64::from(clamp_limit(limit)));
        query
            .push(" OFFSET ")
            .push_bind(i64::from(clamp_offset(offset)));
        let items = query
            .build_query_as::<Tag>()
            .fetch_all(&self.pool)
            .await
            .map_err(|_| AppError::Database("Failed to list tags".into()))?;
        Ok((items, total))
    }
}
