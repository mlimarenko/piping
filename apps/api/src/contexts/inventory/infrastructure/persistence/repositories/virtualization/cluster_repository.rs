use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use super::super::repository::ClusterFilter;
use super::super::repository::ClusterRepository;
use crate::contexts::inventory::domain::{Cluster, ClusterListView};
use crate::contexts::shared::list_query::{
    ListColumn, WhereState, push_custom_field_filters, push_id_filter, push_order_by,
    push_string_in, push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

#[derive(Clone)]
pub struct SqlxClusterRepository {
    pool: PgPool,
}

pub const CLUSTER_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "c.name", "inventory.clusters.columns.name"),
    ListColumn::enum_filter(
        "status",
        "c.status",
        "c.status",
        "inventory.clusters.columns.status",
    ),
    ListColumn::fk_filter(
        "cluster_type",
        "ct.name",
        "cluster_type_id",
        "c.type_id",
        "Cluster Types",
        "inventory.clusters.columns.clusterType",
    ),
    ListColumn::fk_filter(
        "cluster_group",
        "cg.name",
        "cluster_group_id",
        "c.group_id",
        "Cluster Groups",
        "inventory.clusters.columns.clusterGroup",
    ),
    ListColumn::fk_filter(
        "site",
        "s.name",
        "site_id",
        "c._site_id",
        "Sites",
        "inventory.clusters.columns.site",
    ),
    ListColumn::fk_filter(
        "tenant",
        "t.name",
        "tenant_id",
        "c.tenant_id",
        "Tenants",
        "inventory.clusters.columns.tenant",
    ),
    ListColumn::sortable(
        "description",
        "c.description",
        "inventory.clusters.columns.description",
    ),
    ListColumn::sortable(
        "comments",
        "c.comments",
        "inventory.clusters.columns.comments",
    ),
];

impl SqlxClusterRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_cluster_row(row: &PgRow) -> Result<Cluster, AppError> {
        Ok(Cluster {
            id: row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read cluster id: {}", e)))?,
            external_id: row.try_get::<Option<i64>, _>("external_id").map_err(|e| {
                AppError::Database(format!("Failed to read cluster external_id: {}", e))
            })?,
            name: row
                .try_get("name")
                .map_err(|e| AppError::Database(format!("Failed to read cluster name: {}", e)))?,
            site_id: row.try_get("_site_id").ok(),
            tenant_id: row.try_get("tenant_id").map_err(|e| {
                AppError::Database(format!("Failed to read cluster tenant_id: {}", e))
            })?,
            cluster_type_id: row.try_get("type_id").ok(),
            cluster_group_id: row.try_get("group_id").ok(),
            status: row.try_get("status").ok(),
            description: row
                .try_get::<Option<String>, _>("description")
                .ok()
                .flatten(),
            comments: row.try_get::<Option<String>, _>("comments").ok().flatten(),
            tags: row.try_get::<Vec<String>, _>("tags").unwrap_or_default(),
            custom_fields: row
                .try_get::<Option<serde_json::Value>, _>("custom_field_data")
                .ok()
                .flatten(),
            piping_synced_at: row
                .try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("piping_synced_at")
                .map_err(|e| {
                    AppError::Database(format!("Failed to read cluster piping_synced_at: {}", e))
                })?,
        })
    }

    async fn count_hydrated(
        &self,
        filter: &ClusterFilter,
        search: Option<&str>,
    ) -> Result<i64, AppError> {
        let mut builder =
            QueryBuilder::<Postgres>::new("SELECT COUNT(*) FROM virtualization_cluster c");
        Self::push_hydrated_filters(&mut builder, filter, search);
        builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to count clusters: {}", e)))
    }

    fn push_hydrated_filters(
        builder: &mut QueryBuilder<'_, Postgres>,
        filter: &ClusterFilter,
        search: Option<&str>,
    ) {
        let has_filter = filter.ids.is_some()
            || !filter.cluster_type_id.is_empty()
            || !filter.cluster_group_id.is_empty()
            || !filter.site_id.is_empty()
            || !filter.tenant_id.is_empty()
            || !filter.status.is_empty()
            || !filter.custom_fields.is_empty()
            || search.is_some();
        if !has_filter {
            return;
        }

        builder.push(" WHERE ");
        let mut where_state = WhereState::after_where();

        push_id_filter(builder, &mut where_state, "c.id", filter.ids.as_deref());

        push_uuid_in(
            builder,
            &mut where_state,
            "c.type_id",
            &filter.cluster_type_id,
        );
        push_uuid_in(
            builder,
            &mut where_state,
            "c.group_id",
            &filter.cluster_group_id,
        );
        push_uuid_in(builder, &mut where_state, "c._site_id", &filter.site_id);
        push_uuid_in(builder, &mut where_state, "c.tenant_id", &filter.tenant_id);
        push_string_in(builder, &mut where_state, "c.status", &filter.status);
        push_custom_field_filters(
            builder,
            &mut where_state,
            "c.custom_field_data",
            "c.id",
            &filter.custom_fields,
        );

        if let Some(search_term) = search {
            where_state.and(builder);
            builder.push("c.name ILIKE ");
            builder.push_bind(search_term.to_owned());
        }
    }
}

impl ClusterRepository for SqlxClusterRepository {
    async fn get_by_id(&self, cluster_id: Uuid) -> Result<Option<Cluster>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, external_id, name, _site_id, tenant_id, type_id, group_id, status,
                   description, comments, tags, custom_field_data, piping_synced_at
            FROM virtualization_cluster
            WHERE id = $1
            "#,
        )
        .bind(cluster_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load cluster: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_cluster_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<Cluster>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let rows = sqlx::query(
            r#"
            SELECT id, external_id, name, _site_id, tenant_id, type_id, group_id, status,
                   description, comments, tags, custom_field_data, piping_synced_at,
                   COUNT(*) OVER() AS total_count
            FROM virtualization_cluster
            ORDER BY name
            LIMIT $1 OFFSET $2
            "#,
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to list clusters: {}", e)))?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!("Failed to read cluster total_count: {}", e))
            })?,
            None => sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM virtualization_cluster")
                .fetch_one(&self.pool)
                .await
                .map_err(|e| AppError::Database(format!("Failed to count clusters: {}", e)))?,
        };
        let items = rows
            .iter()
            .map(Self::parse_cluster_row)
            .collect::<Result<Vec<Cluster>, AppError>>()?;
        Ok((items, total))
    }

    async fn list_hydrated(
        &self,
        filter: ClusterFilter,
        limit: Option<u32>,
        offset: Option<u32>,
        search: Option<&str>,
    ) -> Result<(Vec<ClusterListView>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT c.id, c.external_id, c.name, c._site_id, c.tenant_id, c.type_id, c.group_id,
                   c.status, c.description, c.comments, c.tags, c.custom_field_data,
                   c.piping_synced_at,
                   ct.name AS cluster_type_name,
                   cg.name AS cluster_group_name,
                   s.name AS site_name,
                   t.name AS tenant_name,
                   COUNT(*) OVER() AS total_count
            FROM virtualization_cluster c
            LEFT JOIN virtualization_clustertype ct ON ct.id = c.type_id
            LEFT JOIN virtualization_clustergroup cg ON cg.id = c.group_id
            LEFT JOIN dcim_site s ON s.id = c._site_id
            LEFT JOIN tenancy_tenant t ON t.id = c.tenant_id
            "#,
        );

        Self::push_hydrated_filters(&mut builder, &filter, search);

        push_order_by(
            &mut builder,
            CLUSTER_COLUMNS,
            filter.sort.as_ref(),
            "c.name",
        );
        builder.push(", c.id");
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows =
            builder.build().fetch_all(&self.pool).await.map_err(|e| {
                AppError::Database(format!("Failed to list clusters hydrated: {}", e))
            })?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!("Failed to read cluster total_count: {}", e))
            })?,
            None => self.count_hydrated(&filter, search).await?,
        };

        let items = rows
            .iter()
            .map(|row| {
                Ok(ClusterListView {
                    record: Self::parse_cluster_row(row)?,
                    cluster_type_name: row.try_get("cluster_type_name").ok(),
                    cluster_group_name: row.try_get("cluster_group_name").ok(),
                    site_name: row.try_get("site_name").ok(),
                    tenant_name: row.try_get("tenant_name").ok(),
                })
            })
            .collect::<Result<Vec<ClusterListView>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, cluster: &Cluster) -> Result<(), AppError> {
        sqlx::query(
            r#"
            INSERT INTO virtualization_cluster (
                id,
                custom_field_data,
                name,
                comments,
                type_id,
                status,
                description,
                group_id,
                tenant_id,
                _site_id,
                tags,
                external_id,
                piping_synced_at
            )
            VALUES (
                $1, COALESCE($2, '{}'::jsonb), $3, $4, $5, $6, $7, $8, $9, $10,
                $11, $12, $13
            )
            "#,
        )
        .bind(cluster.id)
        .bind(cluster.custom_fields.clone())
        .bind(&cluster.name)
        .bind(cluster.comments.as_deref().unwrap_or(""))
        .bind(
            cluster
                .cluster_type_id
                .ok_or_else(|| AppError::Validation("Cluster type is required".to_string()))?,
        )
        .bind(cluster.status.as_deref().unwrap_or("active"))
        .bind(cluster.description.as_deref().unwrap_or(""))
        .bind(cluster.cluster_group_id)
        .bind(cluster.tenant_id)
        .bind(cluster.site_id)
        .bind(&cluster.tags)
        .bind(cluster.external_id)
        .bind(cluster.piping_synced_at)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create cluster: {}", e)))?;
        Ok(())
    }

    async fn update(&self, cluster: &Cluster) -> Result<(), AppError> {
        sqlx::query(
            r#"
            UPDATE virtualization_cluster
            SET name = $2,
                _site_id = $3,
                tenant_id = $4,
                type_id = $5,
                group_id = $6,
                status = $7,
                description = $8,
                comments = $9,
                tags = $10,
                custom_field_data = COALESCE($11, custom_field_data),
                external_id = COALESCE($12, external_id),
                piping_synced_at = COALESCE($13, piping_synced_at)
            WHERE id = $1
            "#,
        )
        .bind(cluster.id)
        .bind(&cluster.name)
        .bind(cluster.site_id)
        .bind(cluster.tenant_id)
        .bind(
            cluster
                .cluster_type_id
                .ok_or_else(|| AppError::Validation("Cluster type is required".to_string()))?,
        )
        .bind(cluster.cluster_group_id)
        .bind(cluster.status.as_deref().unwrap_or("active"))
        .bind(cluster.description.as_deref().unwrap_or(""))
        .bind(cluster.comments.as_deref().unwrap_or(""))
        .bind(&cluster.tags)
        .bind(cluster.custom_fields.clone())
        .bind(cluster.external_id)
        .bind(cluster.piping_synced_at)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update cluster: {}", e)))?;
        Ok(())
    }

    async fn delete(&self, cluster_id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(
            &self.pool,
            "virtualization_cluster",
            cluster_id,
        )
        .await
        .map(|_| ())
    }
}
