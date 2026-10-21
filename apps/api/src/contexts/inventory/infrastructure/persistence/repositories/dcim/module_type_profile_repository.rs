use sqlx::{PgPool, QueryBuilder};
use uuid::Uuid;

use crate::contexts::inventory::domain::dcim::module_type_profile::ModuleTypeProfile;
use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_search_ilike,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

pub const MODULE_TYPE_PROFILE_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "name", "inventory.resources.fields.name"),
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

#[derive(Clone)]
pub struct SqlxModuleTypeProfileRepository {
    pool: PgPool,
}

impl SqlxModuleTypeProfileRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn get_by_id(&self, id: Uuid) -> Result<Option<ModuleTypeProfile>, AppError> {
        sqlx::query_as!(
            ModuleTypeProfile,
            "SELECT id, name, description, comments, schema, owner_id FROM dcim_moduletypeprofile WHERE id = $1",
            id,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| AppError::Database("Failed to load module type profile".into()))
    }

    pub async fn list_paginated(
        &self,
        search: Option<String>,
        limit: Option<u32>,
        offset: Option<u32>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<ModuleTypeProfile>, i64), AppError> {
        let mut count = QueryBuilder::new("SELECT COUNT(*) FROM dcim_moduletypeprofile");
        let mut where_state = WhereState::new();
        push_search_ilike(
            &mut count,
            &mut where_state,
            &["name", "description", "comments"],
            search.as_deref(),
        );
        let total = count
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_err(|_| AppError::Database("Failed to count module type profiles".into()))?;

        let mut query = QueryBuilder::new(
            "SELECT id, name, description, comments, schema, owner_id FROM dcim_moduletypeprofile",
        );
        let mut where_state = WhereState::new();
        push_search_ilike(
            &mut query,
            &mut where_state,
            &["name", "description", "comments"],
            search.as_deref(),
        );
        push_order_by(
            &mut query,
            MODULE_TYPE_PROFILE_COLUMNS,
            sort.as_ref(),
            "name",
        );
        query
            .push(" LIMIT ")
            .push_bind(i64::from(clamp_limit(limit)));
        query
            .push(" OFFSET ")
            .push_bind(i64::from(clamp_offset(offset)));
        let items = query
            .build_query_as::<ModuleTypeProfile>()
            .fetch_all(&self.pool)
            .await
            .map_err(|_| AppError::Database("Failed to list module type profiles".into()))?;
        Ok((items, total))
    }
}
