use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::inventory::domain::ServiceTemplate;
use crate::contexts::shared::list_query::{
    FilterKind, ListColumn, SortSpec, WhereState, push_order_by, push_search_ilike, push_string_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

pub const SERVICE_TEMPLATE_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "name", "inventory.resources.fields.name"),
    ListColumn::enum_filter(
        "protocol",
        "protocol",
        "protocol",
        "inventory.resources.fields.protocol",
    ),
    ListColumn::sortable("ports", "ports", "inventory.resources.fields.ports"),
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

#[derive(Debug, Clone, Default)]
pub struct ServiceTemplateFilter {
    pub search: Option<String>,
    pub protocol: Vec<String>,
    pub sort: Option<SortSpec>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait ServiceTemplateRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<ServiceTemplate>, AppError>;
    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        filter: ServiceTemplateFilter,
    ) -> Result<(Vec<ServiceTemplate>, i64), AppError>;
    async fn create(&self, item: &ServiceTemplate) -> Result<(), AppError>;
    async fn update(&self, item: &ServiceTemplate) -> Result<(), AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxServiceTemplateRepository {
    pool: PgPool,
}

impl SqlxServiceTemplateRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<ServiceTemplate, AppError> {
        Ok(ServiceTemplate {
            id: row.try_get("id").map_err(|e| {
                AppError::Database(format!("Failed to read ipam_servicetemplate id: {}", e))
            })?,
            name: row.try_get("name").map_err(|e| {
                AppError::Database(format!("Failed to read ipam_servicetemplate name: {}", e))
            })?,
            protocol: row.try_get("protocol").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read ipam_servicetemplate protocol: {}",
                    e
                ))
            })?,
            ports: row.try_get("ports").map_err(|e| {
                AppError::Database(format!("Failed to read ipam_servicetemplate ports: {}", e))
            })?,
            description: row.try_get("description").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read ipam_servicetemplate description: {}",
                    e
                ))
            })?,
            comments: row.try_get("comments").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read ipam_servicetemplate comments: {}",
                    e
                ))
            })?,
            custom_fields: row
                .try_get::<Option<serde_json::Value>, _>("custom_field_data")
                .ok()
                .flatten(),
        })
    }

    fn filter_values<'a>(filter: &'a ServiceTemplateFilter, param: &str) -> &'a [String] {
        match param {
            "protocol" => &filter.protocol,
            _ => &[],
        }
    }

    fn push_list_filters(builder: &mut QueryBuilder<'_, Postgres>, filter: &ServiceTemplateFilter) {
        if filter.search.is_none() && filter.protocol.is_empty() {
            return;
        }

        builder.push(" WHERE ");
        let mut where_state = WhereState::after_where();
        push_search_ilike(
            builder,
            &mut where_state,
            &["name", "protocol", "ports", "description", "comments"],
            filter.search.as_deref(),
        );

        for column in SERVICE_TEMPLATE_COLUMNS {
            let Some(column_filter) = column.filter else {
                continue;
            };
            if column_filter.kind == FilterKind::String {
                push_string_in(
                    builder,
                    &mut where_state,
                    column_filter.sql,
                    Self::filter_values(filter, column_filter.param),
                );
            }
        }
    }

    async fn count_filtered(&self, filter: &ServiceTemplateFilter) -> Result<i64, AppError> {
        let mut builder =
            QueryBuilder::<Postgres>::new("SELECT COUNT(*) FROM ipam_servicetemplate");
        Self::push_list_filters(&mut builder, filter);
        builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| {
                AppError::Database(format!("Failed to count ipam_servicetemplates: {}", e))
            })
    }
}

impl ServiceTemplateRepository for SqlxServiceTemplateRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<ServiceTemplate>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, name, protocol, ports, description, comments, custom_field_data
            FROM ipam_servicetemplate
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load ipam_servicetemplate: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        filter: ServiceTemplateFilter,
    ) -> Result<(Vec<ServiceTemplate>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT id, name, protocol, ports, description, comments, custom_field_data,
                   COUNT(*) OVER() AS total_count
            FROM ipam_servicetemplate
            "#,
        );
        Self::push_list_filters(&mut builder, &filter);
        push_order_by(
            &mut builder,
            SERVICE_TEMPLATE_COLUMNS,
            filter.sort.as_ref(),
            "name",
        );
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder.build().fetch_all(&self.pool).await.map_err(|e| {
            AppError::Database(format!("Failed to list ipam_servicetemplates: {}", e))
        })?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read ipam_servicetemplate total_count: {}",
                    e
                ))
            })?,
            None => self.count_filtered(&filter).await?,
        };
        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<ServiceTemplate>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, item: &ServiceTemplate) -> Result<(), AppError> {
        sqlx::query(
            r#"
            INSERT INTO ipam_servicetemplate (
                id, custom_field_data, name, protocol, ports, description, comments
            )
            VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(item.id)
        .bind(&item.name)
        .bind(&item.protocol)
        .bind(&item.ports)
        .bind(&item.description)
        .bind(&item.comments)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create ipam_servicetemplate: {}", e)))?;
        Ok(())
    }

    async fn update(&self, item: &ServiceTemplate) -> Result<(), AppError> {
        sqlx::query(
            r#"
            UPDATE ipam_servicetemplate
            SET name = $2, protocol = $3, ports = $4, description = $5, comments = $6
            WHERE id = $1
            "#,
        )
        .bind(item.id)
        .bind(&item.name)
        .bind(&item.protocol)
        .bind(&item.ports)
        .bind(&item.description)
        .bind(&item.comments)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update ipam_servicetemplate: {}", e)))?;
        Ok(())
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "ipam_servicetemplate", id)
            .await
            .map(|_| ())
    }
}
