use sqlx::postgres::PgRow;
use sqlx::{PgPool, QueryBuilder, Row};
use uuid::Uuid;

use super::super::repository::ClusterTypeRepository;
use crate::contexts::inventory::domain::ClusterType;
use crate::contexts::shared::list_query::{ListColumn, SortSpec, push_order_by};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

pub const CLUSTER_TYPE_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable(
        "external_id",
        "external_id",
        "inventory.clusterTypes.columns.externalId",
    ),
    ListColumn::sortable("name", "name", "inventory.clusterTypes.columns.name"),
    ListColumn::sortable("slug", "slug", "inventory.clusterTypes.columns.slug"),
    ListColumn::sortable(
        "description",
        "description",
        "inventory.clusterTypes.columns.description",
    ),
    ListColumn::sortable("tags", "tags", "inventory.clusterTypes.columns.tags"),
    ListColumn::sortable(
        "piping_synced_at",
        "piping_synced_at",
        "inventory.clusterTypes.columns.syncedAt",
    ),
];

#[derive(Clone)]
pub struct SqlxClusterTypeRepository {
    pool: PgPool,
}

impl SqlxClusterTypeRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_cluster_type_row(row: &PgRow) -> Result<ClusterType, AppError> {
        Ok(ClusterType {
            id: row.try_get("id").map_err(|e| {
                AppError::Database(format!("Failed to read cluster type id: {}", e))
            })?,
            external_id: row.try_get::<Option<i64>, _>("external_id").map_err(|e| {
                AppError::Database(format!("Failed to read cluster type external_id: {}", e))
            })?,
            name: row.try_get("name").map_err(|e| {
                AppError::Database(format!("Failed to read cluster type name: {}", e))
            })?,
            slug: row.try_get("slug").map_err(|e| {
                AppError::Database(format!("Failed to read cluster type slug: {}", e))
            })?,
            description: row
                .try_get::<Option<String>, _>("description")
                .ok()
                .flatten(),
            tags: row.try_get::<Vec<String>, _>("tags").unwrap_or_default(),
            custom_fields: row
                .try_get::<Option<serde_json::Value>, _>("custom_field_data")
                .ok()
                .flatten(),
            piping_synced_at: row
                .try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("piping_synced_at")
                .map_err(|e| {
                    AppError::Database(format!(
                        "Failed to read cluster type piping_synced_at: {}",
                        e
                    ))
                })?,
        })
    }
}

impl ClusterTypeRepository for SqlxClusterTypeRepository {
    async fn get_by_id(&self, cluster_type_id: Uuid) -> Result<Option<ClusterType>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, external_id, name, slug, description, tags,
                   custom_field_data, piping_synced_at
            FROM virtualization_clustertype
            WHERE id = $1
            "#,
        )
        .bind(cluster_type_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load cluster type: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_cluster_type_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        search: Option<&str>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<ClusterType>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT id, external_id, name, slug, description, tags,
                   custom_field_data, piping_synced_at,
                   COUNT(*) OVER() AS total_count
            FROM virtualization_clustertype
            "#,
        );
        builder.push(" WHERE (");
        builder.push_bind(search);
        builder.push("::text IS NULL OR name ILIKE ");
        builder.push_bind(search);
        builder.push(")");
        push_order_by(&mut builder, CLUSTER_TYPE_COLUMNS, sort.as_ref(), "name");
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list cluster types: {}", e)))?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!("Failed to read cluster type total_count: {}", e))
            })?,
            None => sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM virtualization_clustertype")
                .fetch_one(&self.pool)
                .await
                .map_err(|e| AppError::Database(format!("Failed to count cluster types: {}", e)))?,
        };
        let items = rows
            .iter()
            .map(Self::parse_cluster_type_row)
            .collect::<Result<Vec<ClusterType>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, cluster_type: &ClusterType) -> Result<(), AppError> {
        sqlx::query(
            r#"
            INSERT INTO virtualization_clustertype (
                id,
                custom_field_data,
                name,
                slug,
                description,
                comments,
                tags,
                external_id,
                piping_synced_at
            )
            VALUES ($1, COALESCE($2, '{}'::jsonb), $3, $4, $5, '', $6, $7, $8)
            "#,
        )
        .bind(cluster_type.id)
        .bind(cluster_type.custom_fields.clone())
        .bind(&cluster_type.name)
        .bind(&cluster_type.slug)
        .bind(cluster_type.description.as_deref().unwrap_or(""))
        .bind(&cluster_type.tags)
        .bind(cluster_type.external_id)
        .bind(cluster_type.piping_synced_at)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create cluster type: {}", e)))?;
        Ok(())
    }

    async fn update(&self, cluster_type: &ClusterType) -> Result<(), AppError> {
        sqlx::query(
            r#"
            UPDATE virtualization_clustertype
            SET name = $2,
                slug = $3,
                description = $4,
                tags = $5,
                custom_field_data = COALESCE($6, custom_field_data),
                external_id = COALESCE($7, external_id),
                piping_synced_at = COALESCE($8, piping_synced_at)
            WHERE id = $1
            "#,
        )
        .bind(cluster_type.id)
        .bind(&cluster_type.name)
        .bind(&cluster_type.slug)
        .bind(cluster_type.description.as_deref().unwrap_or(""))
        .bind(&cluster_type.tags)
        .bind(cluster_type.custom_fields.clone())
        .bind(cluster_type.external_id)
        .bind(cluster_type.piping_synced_at)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update cluster type: {}", e)))?;
        Ok(())
    }

    async fn delete(&self, cluster_type_id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(
            &self.pool,
            "virtualization_clustertype",
            cluster_type_id,
        )
        .await
        .map(|_| ())
    }
}
