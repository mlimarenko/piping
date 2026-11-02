//! Persistence of immutable mapping configuration versions and their mappings.

use chrono::Utc;
use sqlx::{PgPool, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_string_in,
};
pub use crate::contexts::sync::domain::MappingConfigDocument;
use crate::contexts::sync::domain::{
    ConflictMode, EntityMapping, FieldMapping, MappingConfiguration, MappingConfigurationStatus,
    MappingDirection, MappingTarget,
};
use crate::errors::AppError;

pub const MAPPING_CONFIGURATION_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "LOWER(mc.name)", "sync.mapping.nameColumn"),
    ListColumn::sortable("versions", "mc.version_id", "sync.mapping.versionsLabel"),
    ListColumn::enum_filter(
        "direction",
        "mc.direction",
        "mc.direction",
        "sync.mapping.directionLabel",
    ),
    ListColumn::sortable(
        "provider_id",
        "mc.source_provider_id::text",
        "sync.mapping.providerLabel",
    ),
];

#[derive(Debug, Clone, Default)]
pub struct MappingConfigurationListFilters {
    pub search: Option<String>,
    pub direction: Vec<String>,
}

impl MappingConfigurationListFilters {
    #[must_use]
    pub fn has_advanced_filters(&self) -> bool {
        self.search
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
            || !self.direction.is_empty()
    }
}

/// Provider/logical-id scope for a mapping-configuration page query.
#[derive(Debug, Clone, Copy, Default)]
pub struct MappingConfigurationScope {
    pub provider_id: Option<Uuid>,
    pub logical_id: Option<Uuid>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait MappingConfigurationRepository: Send + Sync {
    /// Insert a new (logical_id, version_id) row. UPDATE is never allowed.
    async fn insert_new_version(
        &self,
        configuration: &MappingConfiguration,
        entity_mappings: &[EntityMapping],
        field_mappings: &[FieldMapping],
    ) -> Result<(), AppError>;

    /// Fetch a single (logical_id, version_id) tuple.
    async fn get(
        &self,
        logical_id: Uuid,
        version_id: i32,
    ) -> Result<Option<MappingConfiguration>, AppError>;

    /// Fetch a single configuration version with its entity and field mappings.
    fn get_document(
        &self,
        logical_id: Uuid,
        version_id: i32,
    ) -> impl Future<Output = Result<Option<MappingConfigDocument>, AppError>> + Send;

    /// List all versions for a given `logical_id`, ordered ascending.
    async fn list_versions_for_logical_id(
        &self,
        logical_id: Uuid,
    ) -> Result<Vec<MappingConfiguration>, AppError>;

    /// Resolve the active configuration for a (source_provider_id, target)
    /// pair. The local-inventory target matches rows whose `target_provider_id`
    /// column is NULL. Returns None when no active version exists.
    async fn get_active_for_provider_pair(
        &self,
        source_provider_id: Uuid,
        target: MappingTarget,
    ) -> Result<Option<MappingConfiguration>, AppError>;

    /// Page over mapping configurations, optionally scoped to a provider and/or logical id.
    async fn list(
        &self,
        scope: MappingConfigurationScope,
        filters: &MappingConfigurationListFilters,
        sort: Option<&SortSpec>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<MappingConfiguration>, AppError>;
    async fn count(
        &self,
        provider_id: Option<Uuid>,
        logical_id: Option<Uuid>,
        filters: &MappingConfigurationListFilters,
    ) -> Result<u64, AppError>;

    /// Page over all configurations for a provider (as source OR target).
    async fn list_for_provider(
        &self,
        provider_id: Uuid,
        filters: &MappingConfigurationListFilters,
        sort: Option<&SortSpec>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<MappingConfiguration>, AppError>;
    async fn list_for_provider_logical_id(
        &self,
        scope: MappingConfigurationScope,
        filters: &MappingConfigurationListFilters,
        sort: Option<&SortSpec>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<MappingConfiguration>, AppError>;
    async fn count_for_provider(
        &self,
        provider_id: Uuid,
        filters: &MappingConfigurationListFilters,
    ) -> Result<u64, AppError>;
    async fn count_for_provider_logical_id(
        &self,
        provider_id: Uuid,
        logical_id: Uuid,
        filters: &MappingConfigurationListFilters,
    ) -> Result<u64, AppError>;

    /// Soft-retire a version. Sets status='retired' + `retired_at=now()`.
    /// Does NOT physically delete rows.
    async fn retire(&self, logical_id: Uuid, version_id: i32) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxMappingConfigurationRepository {
    pool: PgPool,
}

impl SqlxMappingConfigurationRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn list_scoped(
        &self,
        scope: MappingConfigurationScope,
        filters: &MappingConfigurationListFilters,
        sort: Option<&SortSpec>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<MappingConfiguration>, AppError> {
        let MappingConfigurationScope {
            provider_id,
            logical_id,
        } = scope;
        let mut builder = QueryBuilder::new(
            r#"
            SELECT logical_id, version_id, name, description,
                   source_provider_id, target_provider_id,
                   status, direction, conflict_mode,
                   created_by, created_at, retired_at
            FROM mapping_configurations mc
            WHERE TRUE"#,
        );
        push_mapping_configuration_scope_predicates(&mut builder, provider_id, logical_id);
        push_mapping_configuration_filter_predicates(&mut builder, filters);
        push_order_by(
            &mut builder,
            MAPPING_CONFIGURATION_COLUMNS,
            sort,
            "mc.logical_id, mc.version_id DESC",
        );
        if sort.is_some() {
            builder.push(", mc.version_id DESC, mc.logical_id");
        }
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder.build().fetch_all(&self.pool).await?;

        let mut out = Vec::with_capacity(rows.len());
        for row in &rows {
            out.push(parse_row(row)?);
        }
        Ok(out)
    }

    async fn count_scoped(
        &self,
        provider_id: Option<Uuid>,
        logical_id: Option<Uuid>,
        filters: &MappingConfigurationListFilters,
    ) -> Result<u64, AppError> {
        let mut builder = QueryBuilder::new(
            r#"
            SELECT COUNT(*)::bigint AS total
            FROM mapping_configurations mc
            WHERE TRUE"#,
        );
        push_mapping_configuration_scope_predicates(&mut builder, provider_id, logical_id);
        push_mapping_configuration_filter_predicates(&mut builder, filters);

        let row = builder.build().fetch_one(&self.pool).await?;
        let total: i64 = row.try_get("total")?;
        u64::try_from(total).map_err(|_| {
            AppError::Internal(format!(
                "mapping configuration count overflowed u64: {total}"
            ))
        })
    }
}

fn push_mapping_configuration_scope_predicates(
    builder: &mut QueryBuilder<'_, sqlx::Postgres>,
    provider_id: Option<Uuid>,
    logical_id: Option<Uuid>,
) {
    if let Some(provider_id) = provider_id {
        builder.push(" AND (mc.source_provider_id = ");
        builder.push_bind(provider_id);
        builder.push(" OR mc.target_provider_id = ");
        builder.push_bind(provider_id);
        builder.push(")");
    }
    if let Some(logical_id) = logical_id {
        builder.push(" AND mc.logical_id = ");
        builder.push_bind(logical_id);
    }
}

fn push_mapping_configuration_filter_predicates(
    builder: &mut QueryBuilder<'_, sqlx::Postgres>,
    filters: &MappingConfigurationListFilters,
) {
    if !filters.direction.is_empty() {
        let mut where_state = WhereState::after_predicate();
        push_string_in(
            builder,
            &mut where_state,
            "mc.direction",
            &filters.direction,
        );
    }
    if let Some(search) = filters
        .search
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let pattern = format!("%{search}%");
        builder.push(" AND (");
        builder.push("mc.name ILIKE ");
        builder.push_bind(pattern.clone());
        builder.push(" OR COALESCE(mc.description, '') ILIKE ");
        builder.push_bind(pattern.clone());
        builder.push(" OR mc.logical_id::text ILIKE ");
        builder.push_bind(pattern);
        builder.push(")");
    }
}

fn parse_row(row: &sqlx::postgres::PgRow) -> Result<MappingConfiguration, AppError> {
    let status_text: String = row.try_get("status")?;
    let direction_text: String = row.try_get("direction")?;
    let conflict_mode_text: String = row.try_get("conflict_mode")?;
    Ok(MappingConfiguration {
        logical_id: row.try_get("logical_id")?,
        version_id: row.try_get("version_id")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        source_provider_id: row.try_get("source_provider_id")?,
        target: MappingTarget::from_provider_column(row.try_get("target_provider_id")?),
        status: MappingConfigurationStatus::from_wire(&status_text).ok_or_else(|| {
            AppError::Internal(format!(
                "mapping_configurations row has invalid status: {status_text}"
            ))
        })?,
        direction: MappingDirection::from_wire(&direction_text).ok_or_else(|| {
            AppError::Internal(format!(
                "mapping_configurations row has invalid direction: {direction_text}"
            ))
        })?,
        conflict_mode: ConflictMode::from_wire(&conflict_mode_text).ok_or_else(|| {
            AppError::Internal(format!(
                "mapping_configurations row has invalid conflict_mode: {conflict_mode_text}"
            ))
        })?,
        created_by: row.try_get("created_by")?,
        created_at: row.try_get("created_at")?,
        retired_at: row.try_get("retired_at")?,
    })
}

fn parse_entity_row(row: &sqlx::postgres::PgRow) -> Result<EntityMapping, AppError> {
    Ok(EntityMapping {
        id: row.try_get("id")?,
        mapping_config_logical_id: row.try_get("mapping_config_logical_id")?,
        mapping_config_version_id: row.try_get("mapping_config_version_id")?,
        source_entity: row.try_get("source_entity")?,
        target_entity: row.try_get("target_entity")?,
        enabled: row.try_get("enabled")?,
        created_at: row.try_get("created_at")?,
    })
}

fn parse_field_row(row: &sqlx::postgres::PgRow) -> Result<FieldMapping, AppError> {
    Ok(FieldMapping {
        id: row.try_get("id")?,
        entity_mapping_id: row.try_get("entity_mapping_id")?,
        source_field: row.try_get("source_field")?,
        target_field: row.try_get("target_field")?,
        forward_transform_json: row.try_get("forward_transform")?,
        reverse_transform_json: row.try_get("reverse_transform")?,
        round_trip_critical: row.try_get("round_trip_critical")?,
        created_at: row.try_get("created_at")?,
    })
}

impl MappingConfigurationRepository for SqlxMappingConfigurationRepository {
    async fn insert_new_version(
        &self,
        configuration: &MappingConfiguration,
        entity_mappings: &[EntityMapping],
        field_mappings: &[FieldMapping],
    ) -> Result<(), AppError> {
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            r#"
            INSERT INTO mapping_configurations (
                logical_id, version_id, name, description,
                source_provider_id, target_provider_id,
                status, direction, conflict_mode,
                created_by, created_at, retired_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            "#,
        )
        .bind(configuration.logical_id)
        .bind(configuration.version_id)
        .bind(&configuration.name)
        .bind(&configuration.description)
        .bind(configuration.source_provider_id)
        .bind(configuration.target.to_provider_column())
        .bind(configuration.status.as_str())
        .bind(configuration.direction.as_str())
        .bind(configuration.conflict_mode.as_str())
        .bind(configuration.created_by)
        .bind(configuration.created_at)
        .bind(configuration.retired_at)
        .execute(&mut *tx)
        .await?;

        for entity in entity_mappings {
            sqlx::query(
                r#"
                INSERT INTO entity_mappings (
                    id, mapping_config_logical_id, mapping_config_version_id,
                    source_entity, target_entity, enabled, created_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7)
                "#,
            )
            .bind(entity.id)
            .bind(entity.mapping_config_logical_id)
            .bind(entity.mapping_config_version_id)
            .bind(&entity.source_entity)
            .bind(&entity.target_entity)
            .bind(entity.enabled)
            .bind(entity.created_at)
            .execute(&mut *tx)
            .await?;
        }

        for field in field_mappings {
            sqlx::query(
                r#"
                INSERT INTO field_mappings (
                    id, entity_mapping_id, source_field, target_field,
                    forward_transform, reverse_transform,
                    round_trip_critical, created_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                "#,
            )
            .bind(field.id)
            .bind(field.entity_mapping_id)
            .bind(&field.source_field)
            .bind(&field.target_field)
            .bind(&field.forward_transform_json)
            .bind(field.reverse_transform_json.as_ref())
            .bind(field.round_trip_critical)
            .bind(field.created_at)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    async fn get(
        &self,
        logical_id: Uuid,
        version_id: i32,
    ) -> Result<Option<MappingConfiguration>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT logical_id, version_id, name, description,
                   source_provider_id, target_provider_id,
                   status, direction, conflict_mode,
                   created_by, created_at, retired_at
            FROM mapping_configurations
            WHERE logical_id = $1 AND version_id = $2
            "#,
        )
        .bind(logical_id)
        .bind(version_id)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(row) => Ok(Some(parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn get_document(
        &self,
        logical_id: Uuid,
        version_id: i32,
    ) -> Result<Option<MappingConfigDocument>, AppError> {
        let Some(configuration) = self.get(logical_id, version_id).await? else {
            return Ok(None);
        };

        let entity_rows = sqlx::query(
            r#"
            SELECT id, mapping_config_logical_id, mapping_config_version_id,
                   source_entity, target_entity, enabled, created_at
            FROM entity_mappings
            WHERE mapping_config_logical_id = $1
              AND mapping_config_version_id = $2
            ORDER BY created_at ASC, id ASC
            "#,
        )
        .bind(logical_id)
        .bind(version_id)
        .fetch_all(&self.pool)
        .await?;

        let field_rows = sqlx::query(
            r#"
            SELECT fm.id, fm.entity_mapping_id, fm.source_field, fm.target_field,
                   fm.forward_transform, fm.reverse_transform,
                   fm.round_trip_critical, fm.created_at
            FROM field_mappings fm
            INNER JOIN entity_mappings em ON em.id = fm.entity_mapping_id
            WHERE em.mapping_config_logical_id = $1
              AND em.mapping_config_version_id = $2
            ORDER BY em.created_at ASC, em.id ASC, fm.created_at ASC, fm.id ASC
            "#,
        )
        .bind(logical_id)
        .bind(version_id)
        .fetch_all(&self.pool)
        .await?;

        let mut entity_mappings = Vec::with_capacity(entity_rows.len());
        for row in &entity_rows {
            entity_mappings.push(parse_entity_row(row)?);
        }

        let mut field_mappings = Vec::with_capacity(field_rows.len());
        for row in &field_rows {
            field_mappings.push(parse_field_row(row)?);
        }

        Ok(Some(MappingConfigDocument {
            reference: configuration.mapping_ref(),
            name: configuration.name,
            description: configuration.description,
            source_provider_id: configuration.source_provider_id,
            target: configuration.target,
            status: configuration.status,
            direction: configuration.direction,
            conflict_mode: configuration.conflict_mode,
            created_by: configuration.created_by,
            created_at: configuration.created_at,
            retired_at: configuration.retired_at,
            entity_mappings,
            field_mappings,
        }))
    }

    async fn list_versions_for_logical_id(
        &self,
        logical_id: Uuid,
    ) -> Result<Vec<MappingConfiguration>, AppError> {
        let rows = sqlx::query(
            r#"
            SELECT logical_id, version_id, name, description,
                   source_provider_id, target_provider_id,
                   status, direction, conflict_mode,
                   created_by, created_at, retired_at
            FROM mapping_configurations
            WHERE logical_id = $1
            ORDER BY version_id ASC
            "#,
        )
        .bind(logical_id)
        .fetch_all(&self.pool)
        .await?;

        let mut out = Vec::with_capacity(rows.len());
        for row in &rows {
            out.push(parse_row(row)?);
        }
        Ok(out)
    }

    async fn get_active_for_provider_pair(
        &self,
        source_provider_id: Uuid,
        target: MappingTarget,
    ) -> Result<Option<MappingConfiguration>, AppError> {
        let row = match target.to_provider_column() {
            Some(target_provider_id) => {
                sqlx::query(
                    r#"
                    SELECT logical_id, version_id, name, description,
                           source_provider_id, target_provider_id,
                           status, direction, conflict_mode,
                           created_by, created_at, retired_at
                    FROM mapping_configurations
                    WHERE source_provider_id = $1
                      AND target_provider_id = $2
                      AND status = 'active'
                    ORDER BY version_id DESC
                    LIMIT 1
                    "#,
                )
                .bind(source_provider_id)
                .bind(target_provider_id)
                .fetch_optional(&self.pool)
                .await?
            }
            None => {
                sqlx::query(
                    r#"
                    SELECT logical_id, version_id, name, description,
                           source_provider_id, target_provider_id,
                           status, direction, conflict_mode,
                           created_by, created_at, retired_at
                    FROM mapping_configurations
                    WHERE source_provider_id = $1
                      AND target_provider_id IS NULL
                      AND status = 'active'
                    ORDER BY version_id DESC
                    LIMIT 1
                    "#,
                )
                .bind(source_provider_id)
                .fetch_optional(&self.pool)
                .await?
            }
        };

        match row {
            Some(row) => Ok(Some(parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list(
        &self,
        scope: MappingConfigurationScope,
        filters: &MappingConfigurationListFilters,
        sort: Option<&SortSpec>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<MappingConfiguration>, AppError> {
        self.list_scoped(scope, filters, sort, limit, offset).await
    }

    async fn count(
        &self,
        provider_id: Option<Uuid>,
        logical_id: Option<Uuid>,
        filters: &MappingConfigurationListFilters,
    ) -> Result<u64, AppError> {
        self.count_scoped(provider_id, logical_id, filters).await
    }

    async fn list_for_provider(
        &self,
        provider_id: Uuid,
        filters: &MappingConfigurationListFilters,
        sort: Option<&SortSpec>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<MappingConfiguration>, AppError> {
        self.list_scoped(
            MappingConfigurationScope {
                provider_id: Some(provider_id),
                logical_id: None,
            },
            filters,
            sort,
            limit,
            offset,
        )
        .await
    }

    async fn count_for_provider(
        &self,
        provider_id: Uuid,
        filters: &MappingConfigurationListFilters,
    ) -> Result<u64, AppError> {
        self.count_scoped(Some(provider_id), None, filters).await
    }

    async fn list_for_provider_logical_id(
        &self,
        scope: MappingConfigurationScope,
        filters: &MappingConfigurationListFilters,
        sort: Option<&SortSpec>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<MappingConfiguration>, AppError> {
        self.list_scoped(scope, filters, sort, limit, offset).await
    }

    async fn count_for_provider_logical_id(
        &self,
        provider_id: Uuid,
        logical_id: Uuid,
        filters: &MappingConfigurationListFilters,
    ) -> Result<u64, AppError> {
        self.count_scoped(Some(provider_id), Some(logical_id), filters)
            .await
    }

    async fn retire(&self, logical_id: Uuid, version_id: i32) -> Result<(), AppError> {
        let result = sqlx::query(
            r#"
            UPDATE mapping_configurations
            SET status = 'retired', retired_at = $3
            WHERE logical_id = $1 AND version_id = $2 AND status <> 'retired'
            "#,
        )
        .bind(logical_id)
        .bind(version_id)
        .bind(Utc::now())
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(AppError::NotFound(
                "mapping configuration not found or already retired".to_string(),
            ));
        }
        Ok(())
    }
}
