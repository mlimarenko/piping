//! Persistence layer for monitoring Service Discovery configuration CRUD.
//!
//! REST handlers in `monitoring.rs` must not run inline SQL (enforced by the
//! `compliance_no_handler_sql` scans). All SQL for the exporter catalog and
//! exporter-assignment CRUD lives here, behind thin repository functions that
//! return plain records; the handlers map those records to DTOs.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::secrets::domain::SECRET_KIND_NAMED;
use crate::contexts::shared::list_query::{
    FilterKind, ListColumn, SortSpec, WhereState, parse_csv_strings, parse_csv_uuids,
    push_order_by, push_string_in, push_uuid_in,
};
use crate::errors::{AppError, classify_sqlx_error_owned};

pub(crate) const EXPORTER_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("slug", "slug", "observability.exporter.columns.slug"),
    ListColumn::sortable("name", "name", "observability.exporter.columns.name"),
    ListColumn::enum_filter(
        "exporter_type",
        "exporter_type",
        "exporter_type",
        "observability.exporter.columns.type",
    ),
    ListColumn::enum_filter(
        "default_target_source",
        "default_target_source",
        "default_target_source",
        "observability.exporter.columns.targetSource",
    ),
    ListColumn::enum_filter(
        "enabled",
        "enabled",
        "enabled::text",
        "observability.columns.enabled",
    ),
    ListColumn::sortable(
        "created_at",
        "created_at",
        "observability.columns.createdAt",
    ),
    ListColumn::sortable(
        "updated_at",
        "updated_at",
        "observability.columns.updatedAt",
    ),
];

pub(crate) const QUERY_SOURCE_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "s.name", "observability.source.columns.name"),
    ListColumn::sortable("slug", "s.slug", "observability.source.columns.slug"),
    ListColumn::enum_filter(
        "source_type",
        "s.source_type",
        "s.source_type",
        "observability.source.columns.type",
    ),
    ListColumn::enum_filter(
        "enabled",
        "s.enabled",
        "s.enabled::text",
        "observability.columns.enabled",
    ),
    ListColumn::sortable(
        "created_at",
        "s.created_at",
        "observability.columns.createdAt",
    ),
    ListColumn::sortable(
        "updated_at",
        "s.updated_at",
        "observability.columns.updatedAt",
    ),
];

pub(crate) const QUERY_SOURCE_ASSIGNMENT_COLUMNS: &[ListColumn] = &[
    ListColumn::fk_filter(
        "query_source",
        "s.name",
        "query_source_id",
        "a.query_source_id",
        "monitoringQuerySources",
        "observability.assign.columns.source",
    ),
    ListColumn::enum_filter(
        "scope_type",
        "a.scope_type",
        "a.scope_type",
        "observability.sd.columns.scope",
    ),
    ListColumn::fk_filter(
        "scope_id",
        "a.scope_id",
        "scope_id",
        "a.scope_id",
        "monitoringScopes",
        "observability.sd.columns.scopeId",
    ),
    ListColumn::sortable("weight", "a.weight", "observability.sd.columns.weight"),
    ListColumn::enum_filter(
        "enabled",
        "a.enabled",
        "a.enabled::text",
        "observability.sd.columns.enabled",
    ),
    ListColumn::sortable(
        "created_at",
        "a.created_at",
        "observability.columns.createdAt",
    ),
    ListColumn::sortable(
        "updated_at",
        "a.updated_at",
        "observability.columns.updatedAt",
    ),
];

pub(crate) const ALERT_SOURCE_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "s.name", "observability.alertSource.columns.name"),
    ListColumn::enum_filter(
        "source_type",
        "s.source_type",
        "s.source_type",
        "observability.alertSource.columns.type",
    ),
    ListColumn::enum_filter(
        "enabled",
        "s.enabled",
        "s.enabled::text",
        "observability.columns.enabled",
    ),
    ListColumn::sortable(
        "created_at",
        "s.created_at",
        "observability.columns.createdAt",
    ),
    ListColumn::sortable(
        "updated_at",
        "s.updated_at",
        "observability.columns.updatedAt",
    ),
];

pub(crate) const ALERT_SOURCE_ASSIGNMENT_COLUMNS: &[ListColumn] = &[
    ListColumn::fk_filter(
        "alert_source",
        "s.name",
        "alert_source_id",
        "a.alert_source_id",
        "monitoringAlertSources",
        "observability.assign.columns.alertSource",
    ),
    ListColumn::enum_filter(
        "scope_type",
        "a.scope_type",
        "a.scope_type",
        "observability.sd.columns.scope",
    ),
    ListColumn::fk_filter(
        "scope_id",
        "a.scope_id",
        "scope_id",
        "a.scope_id",
        "monitoringScopes",
        "observability.sd.columns.scopeId",
    ),
    ListColumn::sortable("weight", "a.weight", "observability.sd.columns.weight"),
    ListColumn::enum_filter(
        "enabled",
        "a.enabled",
        "a.enabled::text",
        "observability.sd.columns.enabled",
    ),
    ListColumn::sortable(
        "created_at",
        "a.created_at",
        "observability.columns.createdAt",
    ),
    ListColumn::sortable(
        "updated_at",
        "a.updated_at",
        "observability.columns.updatedAt",
    ),
];

pub(crate) const METRIC_QUERY_COLUMNS: &[ListColumn] = &[
    ListColumn::fk_filter(
        "exporter",
        "exporter_id",
        "exporter_id",
        "exporter_id",
        "monitoringExporters",
        "observability.metric.columns.exporter",
    ),
    ListColumn::enum_filter(
        "source_type",
        "source_type",
        "source_type",
        "observability.metric.columns.sourceType",
    ),
    ListColumn::enum_filter(
        "query_type",
        "query_type",
        "query_type",
        "observability.metric.columns.queryType",
    ),
    ListColumn::enum_filter(
        "value_type",
        "value_type",
        "value_type",
        "observability.metric.columns.valueType",
    ),
    ListColumn::enum_filter(
        "widget_type",
        "widget_type",
        "widget_type",
        "observability.metric.columns.widgetType",
    ),
    ListColumn::sortable("name", "name", "observability.metric.columns.name"),
    ListColumn::sortable("slug", "slug", "observability.metric.columns.slug"),
    ListColumn::sortable(
        "sort_order",
        "sort_order",
        "observability.metric.columns.sortOrder",
    ),
    ListColumn::enum_filter(
        "enabled",
        "enabled",
        "enabled::text",
        "observability.columns.enabled",
    ),
    ListColumn::sortable(
        "created_at",
        "created_at",
        "observability.columns.createdAt",
    ),
    ListColumn::sortable(
        "updated_at",
        "updated_at",
        "observability.columns.updatedAt",
    ),
];

pub(super) const METRIC_SET_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "name", "observability.set.columns.name"),
    ListColumn::sortable("slug", "slug", "observability.set.columns.slug"),
    ListColumn::enum_filter(
        "enabled",
        "enabled",
        "enabled::text",
        "observability.columns.enabled",
    ),
    ListColumn::sortable(
        "created_at",
        "created_at",
        "observability.columns.createdAt",
    ),
    ListColumn::sortable(
        "updated_at",
        "updated_at",
        "observability.columns.updatedAt",
    ),
];

pub(super) const METRIC_SET_ASSIGNMENT_COLUMNS: &[ListColumn] = &[
    ListColumn::fk_filter(
        "metric_set",
        "s.name",
        "metric_set_id",
        "a.metric_set_id",
        "monitoringMetricSets",
        "observability.assign.columns.metricSet",
    ),
    ListColumn::enum_filter(
        "scope_type",
        "a.scope_type",
        "a.scope_type",
        "observability.sd.columns.scope",
    ),
    ListColumn::fk_filter(
        "scope_id",
        "a.scope_id",
        "scope_id",
        "a.scope_id",
        "monitoringScopes",
        "observability.sd.columns.scopeId",
    ),
    ListColumn::sortable("weight", "a.weight", "observability.sd.columns.weight"),
    ListColumn::enum_filter(
        "enabled",
        "a.enabled",
        "a.enabled::text",
        "observability.sd.columns.enabled",
    ),
    ListColumn::sortable(
        "created_at",
        "a.created_at",
        "observability.columns.createdAt",
    ),
    ListColumn::sortable(
        "updated_at",
        "a.updated_at",
        "observability.columns.updatedAt",
    ),
];

#[derive(Debug, Clone, Default)]
pub(super) struct MonitoringListFilter {
    pub sort: Option<SortSpec>,
    pub values: HashMap<&'static str, MonitoringFilterValues>,
}

#[derive(Debug, Clone)]
pub(super) enum MonitoringFilterValues {
    Strings(Vec<String>),
    Uuids(Vec<Uuid>),
}

impl MonitoringListFilter {
    pub(super) fn from_params(
        columns: &[ListColumn],
        sort: Option<&str>,
        order: Option<&str>,
        params: &[(&'static str, Option<&str>)],
    ) -> Result<Self, AppError> {
        let mut values = HashMap::new();
        for column in columns {
            let Some(filter) = column.filter else {
                continue;
            };
            let raw = params
                .iter()
                .find_map(|(param, value)| (*param == filter.param).then_some(*value))
                .flatten();
            match filter.kind {
                FilterKind::Uuid => {
                    let parsed = parse_csv_uuids(raw, filter.param)?;
                    if !parsed.is_empty() {
                        values.insert(filter.param, MonitoringFilterValues::Uuids(parsed));
                    }
                }
                FilterKind::String => {
                    let parsed = parse_csv_strings(raw);
                    if !parsed.is_empty() {
                        values.insert(filter.param, MonitoringFilterValues::Strings(parsed));
                    }
                }
            }
        }
        Ok(Self {
            sort: SortSpec::parse_columns(sort, order, columns)?,
            values,
        })
    }
}

fn push_list_filters(
    builder: &mut QueryBuilder<Postgres>,
    columns: &[ListColumn],
    filter: &MonitoringListFilter,
) {
    let mut where_state = WhereState::new();
    push_list_filters_with_state(builder, columns, filter, &mut where_state);
}

fn push_list_filters_with_state(
    builder: &mut QueryBuilder<Postgres>,
    columns: &[ListColumn],
    filter: &MonitoringListFilter,
    where_state: &mut WhereState,
) {
    for column in columns {
        let Some(column_filter) = column.filter else {
            continue;
        };
        let Some(values) = filter.values.get(column_filter.param) else {
            continue;
        };
        match values {
            MonitoringFilterValues::Strings(values) => {
                push_string_in(builder, where_state, column_filter.sql, values);
            }
            MonitoringFilterValues::Uuids(values) => {
                push_uuid_in(builder, where_state, column_filter.sql, values);
            }
        }
    }
}

/// A row from the `monitoring_exporter` catalog.
#[derive(Debug, Clone)]
pub(super) struct ExporterRecord {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub exporter_type: String,
    pub description: Option<String>,
    pub default_job: String,
    pub default_scheme: String,
    pub default_port: Option<i32>,
    pub default_metrics_path: String,
    pub default_target_source: String,
    pub labels: Value,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Mutable fields for an exporter profile.
#[derive(Debug, Clone)]
pub(crate) struct ExporterUpsertInput {
    pub slug: String,
    pub name: String,
    pub exporter_type: String,
    pub description: Option<String>,
    pub default_job: String,
    pub default_scheme: String,
    pub default_port: Option<i32>,
    pub default_metrics_path: String,
    pub default_target_source: String,
    pub labels: Value,
    pub enabled: bool,
}

fn map_exporter_row(row: &sqlx::postgres::PgRow) -> Result<ExporterRecord, AppError> {
    Ok(ExporterRecord {
        id: row.try_get("id").map_err(classify_sqlx_error_owned)?,
        slug: row.try_get("slug").map_err(classify_sqlx_error_owned)?,
        name: row.try_get("name").map_err(classify_sqlx_error_owned)?,
        exporter_type: row
            .try_get("exporter_type")
            .map_err(classify_sqlx_error_owned)?,
        description: row.try_get("description").ok(),
        default_job: row
            .try_get("default_job")
            .map_err(classify_sqlx_error_owned)?,
        default_scheme: row
            .try_get("default_scheme")
            .map_err(classify_sqlx_error_owned)?,
        default_port: row.try_get("default_port").ok(),
        default_metrics_path: row
            .try_get("default_metrics_path")
            .map_err(classify_sqlx_error_owned)?,
        default_target_source: row
            .try_get("default_target_source")
            .map_err(classify_sqlx_error_owned)?,
        labels: row
            .try_get("labels")
            .unwrap_or_else(|_| Value::Object(serde_json::Map::new())),
        enabled: row.try_get("enabled").map_err(classify_sqlx_error_owned)?,
        created_at: row
            .try_get("created_at")
            .map_err(classify_sqlx_error_owned)?,
        updated_at: row
            .try_get("updated_at")
            .map_err(classify_sqlx_error_owned)?,
    })
}

/// List the exporter catalog.
pub(super) async fn list_exporters(
    pool: &PgPool,
    filter: &MonitoringListFilter,
) -> Result<Vec<ExporterRecord>, AppError> {
    let mut builder = QueryBuilder::new(
        r#"
        SELECT id, slug, name, exporter_type, description, default_job,
               default_scheme, default_port, default_metrics_path,
               default_target_source, labels, enabled, created_at, updated_at
        FROM monitoring_exporter
        "#,
    );
    push_list_filters(&mut builder, EXPORTER_COLUMNS, filter);
    push_order_by(&mut builder, EXPORTER_COLUMNS, filter.sort.as_ref(), "slug");
    let rows = builder
        .build()
        .fetch_all(pool)
        .await
        .map_err(classify_sqlx_error_owned)?;

    rows.iter().map(map_exporter_row).collect()
}

async fn fetch_exporter_by_id(pool: &PgPool, id: Uuid) -> Result<Option<ExporterRecord>, AppError> {
    let row = sqlx::query(
        r#"
        SELECT id, slug, name, exporter_type, description, default_job,
               default_scheme, default_port, default_metrics_path,
               default_target_source, labels, enabled, created_at, updated_at
        FROM monitoring_exporter
        WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(classify_sqlx_error_owned)?;

    row.as_ref().map(map_exporter_row).transpose()
}

/// Insert a new exporter catalog profile and return the stored record.
pub(super) async fn create_exporter(
    pool: &PgPool,
    input: &ExporterUpsertInput,
) -> Result<ExporterRecord, AppError> {
    let row = sqlx::query(
        r#"
        INSERT INTO monitoring_exporter
            (slug, name, exporter_type, description, default_job,
             default_scheme, default_port, default_metrics_path,
             default_target_source, labels, enabled)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
        RETURNING id
        "#,
    )
    .bind(&input.slug)
    .bind(&input.name)
    .bind(&input.exporter_type)
    .bind(input.description.as_deref())
    .bind(&input.default_job)
    .bind(&input.default_scheme)
    .bind(input.default_port)
    .bind(&input.default_metrics_path)
    .bind(&input.default_target_source)
    .bind(&input.labels)
    .bind(input.enabled)
    .fetch_one(pool)
    .await
    .map_err(classify_sqlx_error_owned)?;

    let id: Uuid = row.try_get("id").map_err(classify_sqlx_error_owned)?;
    fetch_exporter_by_id(pool, id)
        .await?
        .ok_or_else(|| AppError::Internal("Created exporter could not be reloaded".to_string()))
}

/// Update all mutable fields of an exporter profile (full replacement).
/// Returns `None` when no exporter with `id` exists.
pub(super) async fn update_exporter(
    pool: &PgPool,
    id: Uuid,
    input: &ExporterUpsertInput,
) -> Result<Option<ExporterRecord>, AppError> {
    let mut tx = pool.begin().await.map_err(classify_sqlx_error_owned)?;
    lock_profile_scrape_mutations_tx(&mut tx).await?;
    let result = sqlx::query(
        r#"
        UPDATE monitoring_exporter
        SET slug = $2,
            name = $3,
            exporter_type = $4,
            description = $5,
            default_job = $6,
            default_scheme = $7,
            default_port = $8,
            default_metrics_path = $9,
            default_target_source = $10,
            labels = $11,
            enabled = $12,
            updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(id)
    .bind(&input.slug)
    .bind(&input.name)
    .bind(&input.exporter_type)
    .bind(input.description.as_deref())
    .bind(&input.default_job)
    .bind(&input.default_scheme)
    .bind(input.default_port)
    .bind(&input.default_metrics_path)
    .bind(&input.default_target_source)
    .bind(&input.labels)
    .bind(input.enabled)
    .execute(&mut *tx)
    .await
    .map_err(classify_sqlx_error_owned)?;

    if result.rows_affected() == 0 {
        return Ok(None);
    }

    let profile_ids = sqlx::query_scalar::<_, Uuid>(
        "SELECT DISTINCT profile_id FROM monitoring_profile_item WHERE exporter_id = $1",
    )
    .bind(id)
    .fetch_all(&mut *tx)
    .await
    .map_err(classify_sqlx_error_owned)?;
    for profile_id in profile_ids {
        validate_profile_scrape_conflicts_tx(&mut tx, profile_id).await?;
    }
    tx.commit().await.map_err(classify_sqlx_error_owned)?;
    fetch_exporter_by_id(pool, id).await
}

/// Delete an exporter profile by id. A profile item references the exporter with
/// `ON DELETE RESTRICT`, so a referenced exporter cannot disappear and leave
/// profiles with dangling items; the guard returns 409 naming the profiles.
pub(super) async fn delete_exporter(pool: &PgPool, id: Uuid) -> Result<bool, AppError> {
    let referencing_profiles = sqlx::query(
        r#"
        SELECT DISTINCT p.slug
        FROM monitoring_profile_item i
        JOIN monitoring_profile p ON p.id = i.profile_id
        WHERE i.exporter_id = $1
        ORDER BY p.slug
        "#,
    )
    .bind(id)
    .fetch_all(pool)
    .await
    .map_err(classify_sqlx_error_owned)?;
    if !referencing_profiles.is_empty() {
        let slugs = referencing_profiles
            .iter()
            .map(|row| row.try_get::<String, _>("slug"))
            .collect::<Result<Vec<_>, _>>()
            .map_err(classify_sqlx_error_owned)?;
        return Err(AppError::Conflict(format!(
            "exporter is still used by monitoring profiles: {}",
            slugs.join(", ")
        )));
    }

    let result = sqlx::query("DELETE FROM monitoring_exporter WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .map_err(classify_sqlx_error_owned)?;

    Ok(result.rows_affected() > 0)
}

// ===========================================================================
// monitoring_query_source
// ===========================================================================

/// Canonical usage-binding vocabulary for the bearer token sent by a query
/// source. The source row deliberately has no credential column: the binding
/// is its only secret reference and therefore participates in where-used and
/// named-secret deletion protection.
const QUERY_SOURCE_CREDENTIAL_CONSUMER_TYPE: &str = "monitoring_query_source";
const QUERY_SOURCE_CREDENTIAL_FIELD_KEY: &str = "bearer_token";
const QUERY_SOURCE_CREDENTIAL_OWNER_SCOPE_TYPE: &str = "monitoring_query_source";

/// A row from `monitoring_query_source`.
#[derive(Debug, Clone)]
pub(super) struct QuerySourceRecord {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub source_type: String,
    pub query_url: String,
    pub credential_secret_id: Option<Uuid>,
    pub timeout_ms: i32,
    pub options: Value,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Mutable fields accepted on create/update of a query source.
#[derive(Debug, Clone)]
pub(crate) struct QuerySourceInput {
    pub name: String,
    pub slug: String,
    pub source_type: String,
    pub query_url: String,
    pub credential_secret_id: Option<Uuid>,
    pub timeout_ms: i32,
    pub options: Value,
    pub enabled: bool,
}

fn map_query_source_row(row: &sqlx::postgres::PgRow) -> Result<QuerySourceRecord, AppError> {
    Ok(QuerySourceRecord {
        id: row.try_get("id").map_err(classify_sqlx_error_owned)?,
        name: row.try_get("name").map_err(classify_sqlx_error_owned)?,
        slug: row.try_get("slug").map_err(classify_sqlx_error_owned)?,
        source_type: row
            .try_get("source_type")
            .map_err(classify_sqlx_error_owned)?,
        query_url: row
            .try_get("query_url")
            .map_err(classify_sqlx_error_owned)?,
        // A NULL binding means no credential. Any other row-shape or decoding
        // failure must surface instead of silently making an authenticated
        // source fall back to the unauthenticated path.
        credential_secret_id: row
            .try_get::<Option<Uuid>, _>("credential_secret_id")
            .map_err(classify_sqlx_error_owned)?,
        timeout_ms: row
            .try_get("timeout_ms")
            .map_err(classify_sqlx_error_owned)?,
        options: row
            .try_get("options")
            .unwrap_or_else(|_| Value::Object(serde_json::Map::new())),
        enabled: row.try_get("enabled").map_err(classify_sqlx_error_owned)?,
        created_at: row
            .try_get("created_at")
            .map_err(classify_sqlx_error_owned)?,
        updated_at: row
            .try_get("updated_at")
            .map_err(classify_sqlx_error_owned)?,
    })
}

/// List query sources.
pub(super) async fn list_query_sources(
    pool: &PgPool,
    filter: &MonitoringListFilter,
) -> Result<Vec<QuerySourceRecord>, AppError> {
    let mut builder = QueryBuilder::new(
        r#"
        SELECT s.id, s.name, s.slug, s.source_type, s.query_url,
               credential_binding.secret_id AS credential_secret_id,
               s.timeout_ms, s.options, s.enabled, s.created_at, s.updated_at
        FROM monitoring_query_source s
        LEFT JOIN secret_usage_binding credential_binding
          ON credential_binding.consumer_type = 'monitoring_query_source'
         AND credential_binding.consumer_id = s.id
         AND credential_binding.field_key = 'bearer_token'
        "#,
    );
    push_list_filters(&mut builder, QUERY_SOURCE_COLUMNS, filter);
    push_order_by(
        &mut builder,
        QUERY_SOURCE_COLUMNS,
        filter.sort.as_ref(),
        "s.slug",
    );
    let rows = builder
        .build()
        .fetch_all(pool)
        .await
        .map_err(classify_sqlx_error_owned)?;

    rows.iter().map(map_query_source_row).collect()
}

async fn fetch_query_source_by_id(
    pool: &PgPool,
    id: Uuid,
) -> Result<Option<QuerySourceRecord>, AppError> {
    let row = sqlx::query(
        r#"
        SELECT s.id, s.name, s.slug, s.source_type, s.query_url,
               credential_binding.secret_id AS credential_secret_id,
               s.timeout_ms, s.options, s.enabled, s.created_at, s.updated_at
        FROM monitoring_query_source s
        LEFT JOIN secret_usage_binding credential_binding
          ON credential_binding.consumer_type = 'monitoring_query_source'
         AND credential_binding.consumer_id = s.id
         AND credential_binding.field_key = 'bearer_token'
        WHERE s.id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(classify_sqlx_error_owned)?;

    row.as_ref().map(map_query_source_row).transpose()
}

/// Verify and key-share-lock the named secret while a query-source transaction
/// creates or rebinds its canonical usage row. The lock closes the race with
/// named-secret deletion: deletion takes `FOR UPDATE` before counting usages.
async fn ensure_named_query_source_credential_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    secret_id: Uuid,
) -> Result<(), AppError> {
    let named_secret = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id
        FROM secrets_variable
        WHERE id = $1
          AND kind = $2
          AND is_protected = false
        FOR KEY SHARE
        "#,
    )
    .bind(secret_id)
    .bind(SECRET_KIND_NAMED)
    .fetch_optional(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?;

    if named_secret.is_none() {
        return Err(AppError::Validation(
            "credential_ref must identify an existing named secret".to_string(),
        ));
    }

    Ok(())
}

/// Reconcile the one canonical bearer-token binding for a query source. Both
/// the source mutation and this projection run in the same transaction, so a
/// rejected reference cannot strand a partially-written source or binding.
async fn reconcile_query_source_credential_binding_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    query_source_id: Uuid,
    credential_secret_id: Option<Uuid>,
) -> Result<(), AppError> {
    if let Some(secret_id) = credential_secret_id {
        ensure_named_query_source_credential_tx(tx, secret_id).await?;
        sqlx::query(
            r#"
            INSERT INTO secret_usage_binding
                (consumer_type, consumer_id, field_key,
                 owner_scope_type, owner_scope_id, secret_id)
            VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT (consumer_type, consumer_id, field_key)
            DO UPDATE
            SET owner_scope_type = EXCLUDED.owner_scope_type,
                owner_scope_id = EXCLUDED.owner_scope_id,
                secret_id = EXCLUDED.secret_id,
                updated_at = now()
            "#,
        )
        .bind(QUERY_SOURCE_CREDENTIAL_CONSUMER_TYPE)
        .bind(query_source_id)
        .bind(QUERY_SOURCE_CREDENTIAL_FIELD_KEY)
        .bind(QUERY_SOURCE_CREDENTIAL_OWNER_SCOPE_TYPE)
        .bind(query_source_id)
        .bind(secret_id)
        .execute(&mut **tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
    } else {
        sqlx::query(
            r#"
            DELETE FROM secret_usage_binding
            WHERE consumer_type = $1
              AND consumer_id = $2
              AND field_key = $3
            "#,
        )
        .bind(QUERY_SOURCE_CREDENTIAL_CONSUMER_TYPE)
        .bind(query_source_id)
        .bind(QUERY_SOURCE_CREDENTIAL_FIELD_KEY)
        .execute(&mut **tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
    }

    Ok(())
}

/// Remove every secret usage projection owned by a deleted query source. The
/// current source contract owns only `bearer_token`, but deleting by consumer
/// rather than a single field prevents a future source-owned field from
/// permanently blocking a named secret after its source is gone.
async fn delete_query_source_usage_bindings_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    query_source_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        DELETE FROM secret_usage_binding
        WHERE consumer_type = $1
          AND consumer_id = $2
        "#,
    )
    .bind(QUERY_SOURCE_CREDENTIAL_CONSUMER_TYPE)
    .bind(query_source_id)
    .execute(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?;

    Ok(())
}

/// Insert a new query source and return the stored record.
pub(super) async fn create_query_source(
    pool: &PgPool,
    input: &QuerySourceInput,
) -> Result<QuerySourceRecord, AppError> {
    let mut tx = pool.begin().await.map_err(classify_sqlx_error_owned)?;
    let row = sqlx::query(
        r#"
        INSERT INTO monitoring_query_source
            (name, slug, source_type, query_url, timeout_ms, options, enabled)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        RETURNING id
        "#,
    )
    .bind(&input.name)
    .bind(&input.slug)
    .bind(&input.source_type)
    .bind(&input.query_url)
    .bind(input.timeout_ms)
    .bind(&input.options)
    .bind(input.enabled)
    .fetch_one(&mut *tx)
    .await
    .map_err(classify_sqlx_error_owned)?;

    let id: Uuid = row.try_get("id").map_err(classify_sqlx_error_owned)?;
    reconcile_query_source_credential_binding_tx(&mut tx, id, input.credential_secret_id).await?;
    tx.commit().await.map_err(classify_sqlx_error_owned)?;

    fetch_query_source_by_id(pool, id)
        .await?
        .ok_or_else(|| AppError::Internal("Created query source could not be reloaded".to_string()))
}

/// Update all mutable fields of an existing query source (full replacement).
/// Returns `None` when no source with `id` exists.
pub(super) async fn update_query_source(
    pool: &PgPool,
    id: Uuid,
    input: &QuerySourceInput,
) -> Result<Option<QuerySourceRecord>, AppError> {
    let mut tx = pool.begin().await.map_err(classify_sqlx_error_owned)?;
    let result = sqlx::query(
        r#"
        UPDATE monitoring_query_source
        SET name = $2,
            slug = $3,
            source_type = $4,
            query_url = $5,
            timeout_ms = $6,
            options = $7,
            enabled = $8,
            updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(id)
    .bind(&input.name)
    .bind(&input.slug)
    .bind(&input.source_type)
    .bind(&input.query_url)
    .bind(input.timeout_ms)
    .bind(&input.options)
    .bind(input.enabled)
    .execute(&mut *tx)
    .await
    .map_err(classify_sqlx_error_owned)?;

    if result.rows_affected() == 0 {
        tx.rollback().await.map_err(classify_sqlx_error_owned)?;
        return Ok(None);
    }

    reconcile_query_source_credential_binding_tx(&mut tx, id, input.credential_secret_id).await?;
    tx.commit().await.map_err(classify_sqlx_error_owned)?;

    fetch_query_source_by_id(pool, id).await
}

/// Delete a query source by id. Returns `true` when a row was removed.
pub(super) async fn delete_query_source(pool: &PgPool, id: Uuid) -> Result<bool, AppError> {
    let mut tx = pool.begin().await.map_err(classify_sqlx_error_owned)?;
    let source_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM monitoring_query_source WHERE id = $1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(classify_sqlx_error_owned)?;
    if source_id.is_none() {
        tx.rollback().await.map_err(classify_sqlx_error_owned)?;
        return Ok(false);
    }

    delete_query_source_usage_bindings_tx(&mut tx, id).await?;
    let result = sqlx::query("DELETE FROM monitoring_query_source WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
    tx.commit().await.map_err(classify_sqlx_error_owned)?;

    Ok(result.rows_affected() > 0)
}

// ===========================================================================
// monitoring_query_source_assignment
// ===========================================================================

/// A row from `monitoring_query_source_assignment`, joined with its source.
#[derive(Debug, Clone)]
pub(super) struct QuerySourceAssignmentRecord {
    pub id: Uuid,
    pub query_source_id: Uuid,
    pub query_source_slug: String,
    pub query_source_name: String,
    pub scope_type: String,
    pub scope_id: Option<Uuid>,
    pub weight: i32,
    pub enabled: bool,
    pub options: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Mutable fields accepted on create/update of a query source assignment.
#[derive(Debug, Clone)]
pub(super) struct QuerySourceAssignmentInput {
    pub query_source_id: Uuid,
    pub scope_type: String,
    pub scope_id: Option<Uuid>,
    pub weight: i32,
    pub enabled: bool,
    pub options: Value,
}

fn map_query_source_assignment_row(
    row: &sqlx::postgres::PgRow,
) -> Result<QuerySourceAssignmentRecord, AppError> {
    Ok(QuerySourceAssignmentRecord {
        id: row.try_get("id").map_err(classify_sqlx_error_owned)?,
        query_source_id: row
            .try_get("query_source_id")
            .map_err(classify_sqlx_error_owned)?,
        query_source_slug: row
            .try_get("query_source_slug")
            .map_err(classify_sqlx_error_owned)?,
        query_source_name: row
            .try_get("query_source_name")
            .map_err(classify_sqlx_error_owned)?,
        scope_type: row
            .try_get("scope_type")
            .map_err(classify_sqlx_error_owned)?,
        scope_id: row.try_get("scope_id").ok(),
        weight: row.try_get("weight").map_err(classify_sqlx_error_owned)?,
        enabled: row.try_get("enabled").map_err(classify_sqlx_error_owned)?,
        options: row
            .try_get("options")
            .unwrap_or_else(|_| Value::Object(serde_json::Map::new())),
        created_at: row
            .try_get("created_at")
            .map_err(classify_sqlx_error_owned)?,
        updated_at: row
            .try_get("updated_at")
            .map_err(classify_sqlx_error_owned)?,
    })
}

/// List query source assignments (joined with their source), optionally
/// filtered by `scope_type` and/or `scope_id`.
pub(super) async fn list_query_source_assignments(
    pool: &PgPool,
    filter: &MonitoringListFilter,
) -> Result<Vec<QuerySourceAssignmentRecord>, AppError> {
    let mut builder = QueryBuilder::new(
        r#"
        SELECT
            a.id,
            a.query_source_id,
            s.slug AS query_source_slug,
            s.name AS query_source_name,
            a.scope_type,
            a.scope_id,
            a.weight,
            a.enabled,
            a.options,
            a.created_at,
            a.updated_at
        FROM monitoring_query_source_assignment a
        JOIN monitoring_query_source s ON s.id = a.query_source_id
        "#,
    );
    push_list_filters(&mut builder, QUERY_SOURCE_ASSIGNMENT_COLUMNS, filter);
    push_order_by(
        &mut builder,
        QUERY_SOURCE_ASSIGNMENT_COLUMNS,
        filter.sort.as_ref(),
        "a.scope_type, a.weight, a.created_at",
    );
    let rows = builder
        .build()
        .fetch_all(pool)
        .await
        .map_err(classify_sqlx_error_owned)?;

    rows.iter().map(map_query_source_assignment_row).collect()
}

async fn fetch_query_source_assignment_by_id(
    pool: &PgPool,
    id: Uuid,
) -> Result<Option<QuerySourceAssignmentRecord>, AppError> {
    let row = sqlx::query(
        r#"
        SELECT
            a.id,
            a.query_source_id,
            s.slug AS query_source_slug,
            s.name AS query_source_name,
            a.scope_type,
            a.scope_id,
            a.weight,
            a.enabled,
            a.options,
            a.created_at,
            a.updated_at
        FROM monitoring_query_source_assignment a
        JOIN monitoring_query_source s ON s.id = a.query_source_id
        WHERE a.id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(classify_sqlx_error_owned)?;

    row.as_ref()
        .map(map_query_source_assignment_row)
        .transpose()
}

/// Insert a new query source assignment and return the joined record.
pub(super) async fn create_query_source_assignment(
    pool: &PgPool,
    input: &QuerySourceAssignmentInput,
) -> Result<QuerySourceAssignmentRecord, AppError> {
    let row = sqlx::query(
        r#"
        INSERT INTO monitoring_query_source_assignment
            (query_source_id, scope_type, scope_id, weight, enabled, options)
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING id
        "#,
    )
    .bind(input.query_source_id)
    .bind(&input.scope_type)
    .bind(input.scope_id)
    .bind(input.weight)
    .bind(input.enabled)
    .bind(&input.options)
    .fetch_one(pool)
    .await
    .map_err(classify_sqlx_error_owned)?;

    let id: Uuid = row.try_get("id").map_err(classify_sqlx_error_owned)?;
    fetch_query_source_assignment_by_id(pool, id)
        .await?
        .ok_or_else(|| {
            AppError::Internal("Created query source assignment could not be reloaded".to_string())
        })
}

/// Update all mutable fields of an existing query source assignment.
/// Returns `None` when no assignment with `id` exists.
pub(super) async fn update_query_source_assignment(
    pool: &PgPool,
    id: Uuid,
    input: &QuerySourceAssignmentInput,
) -> Result<Option<QuerySourceAssignmentRecord>, AppError> {
    let result = sqlx::query(
        r#"
        UPDATE monitoring_query_source_assignment
        SET query_source_id = $2,
            scope_type = $3,
            scope_id = $4,
            weight = $5,
            enabled = $6,
            options = $7,
            updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(id)
    .bind(input.query_source_id)
    .bind(&input.scope_type)
    .bind(input.scope_id)
    .bind(input.weight)
    .bind(input.enabled)
    .bind(&input.options)
    .execute(pool)
    .await
    .map_err(classify_sqlx_error_owned)?;

    if result.rows_affected() == 0 {
        return Ok(None);
    }

    fetch_query_source_assignment_by_id(pool, id).await
}

/// Delete a query source assignment by id. Returns `true` when a row was removed.
pub(super) async fn delete_query_source_assignment(
    pool: &PgPool,
    id: Uuid,
) -> Result<bool, AppError> {
    let result = sqlx::query("DELETE FROM monitoring_query_source_assignment WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .map_err(classify_sqlx_error_owned)?;

    Ok(result.rows_affected() > 0)
}

// ===========================================================================
// monitoring_alert_source
// ===========================================================================

/// Canonical usage-binding vocabulary for the bearer token sent by an alert
/// source. The source row deliberately has no credential column: the binding
/// is its only secret reference and therefore participates in where-used and
/// named-secret deletion protection. Alert sources of every `source_type`
/// share this shape; a `zabbix` source with `options.auth_mode = 'auth_param'`
/// differs only in the provider request builder (JSON-RPC `auth` field), not
/// in credential storage.
const ALERT_SOURCE_CREDENTIAL_CONSUMER_TYPE: &str = "monitoring_alert_source";
const ALERT_SOURCE_CREDENTIAL_FIELD_KEY: &str = "bearer_token";
const ALERT_SOURCE_CREDENTIAL_OWNER_SCOPE_TYPE: &str = "monitoring_alert_source";

/// A row from `monitoring_alert_source`.
#[derive(Debug, Clone)]
pub(super) struct AlertSourceRecord {
    pub id: Uuid,
    pub name: String,
    pub source_type: String,
    pub query_url: String,
    pub credential_secret_id: Option<Uuid>,
    pub timeout_ms: i32,
    pub options: Value,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Mutable fields accepted on create/update of an alert source.
#[derive(Debug, Clone)]
pub(super) struct AlertSourceInput {
    pub name: String,
    pub source_type: String,
    pub query_url: String,
    pub credential_secret_id: Option<Uuid>,
    pub timeout_ms: i32,
    pub options: Value,
    pub enabled: bool,
}

fn map_alert_source_row(row: &sqlx::postgres::PgRow) -> Result<AlertSourceRecord, AppError> {
    Ok(AlertSourceRecord {
        id: row.try_get("id").map_err(classify_sqlx_error_owned)?,
        name: row.try_get("name").map_err(classify_sqlx_error_owned)?,
        source_type: row
            .try_get("source_type")
            .map_err(classify_sqlx_error_owned)?,
        query_url: row
            .try_get("query_url")
            .map_err(classify_sqlx_error_owned)?,
        // A NULL binding means no credential. Any other row-shape or decoding
        // failure must surface instead of silently making an authenticated
        // source fall back to the unauthenticated path.
        credential_secret_id: row
            .try_get::<Option<Uuid>, _>("credential_secret_id")
            .map_err(classify_sqlx_error_owned)?,
        timeout_ms: row
            .try_get("timeout_ms")
            .map_err(classify_sqlx_error_owned)?,
        options: row
            .try_get("options")
            .unwrap_or_else(|_| Value::Object(serde_json::Map::new())),
        enabled: row.try_get("enabled").map_err(classify_sqlx_error_owned)?,
        created_at: row
            .try_get("created_at")
            .map_err(classify_sqlx_error_owned)?,
        updated_at: row
            .try_get("updated_at")
            .map_err(classify_sqlx_error_owned)?,
    })
}

/// List alert sources.
pub(super) async fn list_alert_sources(
    pool: &PgPool,
    filter: &MonitoringListFilter,
) -> Result<Vec<AlertSourceRecord>, AppError> {
    let mut builder = QueryBuilder::new(
        r#"
        SELECT s.id, s.name, s.source_type, s.query_url,
               credential_binding.secret_id AS credential_secret_id,
               s.timeout_ms, s.options, s.enabled, s.created_at, s.updated_at
        FROM monitoring_alert_source s
        LEFT JOIN secret_usage_binding credential_binding
          ON credential_binding.consumer_type = 'monitoring_alert_source'
         AND credential_binding.consumer_id = s.id
         AND credential_binding.field_key = 'bearer_token'
        "#,
    );
    push_list_filters(&mut builder, ALERT_SOURCE_COLUMNS, filter);
    push_order_by(
        &mut builder,
        ALERT_SOURCE_COLUMNS,
        filter.sort.as_ref(),
        "name",
    );
    let rows = builder
        .build()
        .fetch_all(pool)
        .await
        .map_err(classify_sqlx_error_owned)?;

    rows.iter().map(map_alert_source_row).collect()
}

async fn fetch_alert_source_by_id(
    pool: &PgPool,
    id: Uuid,
) -> Result<Option<AlertSourceRecord>, AppError> {
    let row = sqlx::query(
        r#"
        SELECT s.id, s.name, s.source_type, s.query_url,
               credential_binding.secret_id AS credential_secret_id,
               s.timeout_ms, s.options, s.enabled, s.created_at, s.updated_at
        FROM monitoring_alert_source s
        LEFT JOIN secret_usage_binding credential_binding
          ON credential_binding.consumer_type = 'monitoring_alert_source'
         AND credential_binding.consumer_id = s.id
         AND credential_binding.field_key = 'bearer_token'
        WHERE s.id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(classify_sqlx_error_owned)?;

    row.as_ref().map(map_alert_source_row).transpose()
}

/// Verify and key-share-lock the named secret while an alert-source transaction
/// creates or rebinds its canonical usage row. The lock closes the race with
/// named-secret deletion: deletion takes `FOR UPDATE` before counting usages.
async fn ensure_named_alert_source_credential_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    secret_id: Uuid,
) -> Result<(), AppError> {
    let named_secret = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id
        FROM secrets_variable
        WHERE id = $1
          AND kind = $2
          AND is_protected = false
        FOR KEY SHARE
        "#,
    )
    .bind(secret_id)
    .bind(SECRET_KIND_NAMED)
    .fetch_optional(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?;

    if named_secret.is_none() {
        return Err(AppError::Validation(
            "credential_ref must identify an existing named secret".to_string(),
        ));
    }

    Ok(())
}

/// Reconcile the one canonical bearer-token binding for an alert source. Both
/// the source mutation and this projection run in the same transaction, so a
/// rejected reference cannot strand a partially-written source or binding.
async fn reconcile_alert_source_credential_binding_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    alert_source_id: Uuid,
    credential_secret_id: Option<Uuid>,
) -> Result<(), AppError> {
    if let Some(secret_id) = credential_secret_id {
        ensure_named_alert_source_credential_tx(tx, secret_id).await?;
        sqlx::query(
            r#"
            INSERT INTO secret_usage_binding
                (consumer_type, consumer_id, field_key,
                 owner_scope_type, owner_scope_id, secret_id)
            VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT (consumer_type, consumer_id, field_key)
            DO UPDATE
            SET owner_scope_type = EXCLUDED.owner_scope_type,
                owner_scope_id = EXCLUDED.owner_scope_id,
                secret_id = EXCLUDED.secret_id,
                updated_at = now()
            "#,
        )
        .bind(ALERT_SOURCE_CREDENTIAL_CONSUMER_TYPE)
        .bind(alert_source_id)
        .bind(ALERT_SOURCE_CREDENTIAL_FIELD_KEY)
        .bind(ALERT_SOURCE_CREDENTIAL_OWNER_SCOPE_TYPE)
        .bind(alert_source_id)
        .bind(secret_id)
        .execute(&mut **tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
    } else {
        sqlx::query(
            r#"
            DELETE FROM secret_usage_binding
            WHERE consumer_type = $1
              AND consumer_id = $2
              AND field_key = $3
            "#,
        )
        .bind(ALERT_SOURCE_CREDENTIAL_CONSUMER_TYPE)
        .bind(alert_source_id)
        .bind(ALERT_SOURCE_CREDENTIAL_FIELD_KEY)
        .execute(&mut **tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
    }

    Ok(())
}

/// Remove every secret usage projection owned by a deleted alert source. The
/// current source contract owns only `bearer_token`, but deleting by consumer
/// rather than a single field prevents a future source-owned field from
/// permanently blocking a named secret after its source is gone.
async fn delete_alert_source_usage_bindings_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    alert_source_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        DELETE FROM secret_usage_binding
        WHERE consumer_type = $1
          AND consumer_id = $2
        "#,
    )
    .bind(ALERT_SOURCE_CREDENTIAL_CONSUMER_TYPE)
    .bind(alert_source_id)
    .execute(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?;

    Ok(())
}

/// Insert a new alert source and return the stored record.
pub(super) async fn create_alert_source(
    pool: &PgPool,
    input: &AlertSourceInput,
) -> Result<AlertSourceRecord, AppError> {
    let mut tx = pool.begin().await.map_err(classify_sqlx_error_owned)?;
    let row = sqlx::query(
        r#"
        INSERT INTO monitoring_alert_source
            (name, source_type, query_url, timeout_ms, options, enabled)
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING id
        "#,
    )
    .bind(&input.name)
    .bind(&input.source_type)
    .bind(&input.query_url)
    .bind(input.timeout_ms)
    .bind(&input.options)
    .bind(input.enabled)
    .fetch_one(&mut *tx)
    .await
    .map_err(classify_sqlx_error_owned)?;

    let id: Uuid = row.try_get("id").map_err(classify_sqlx_error_owned)?;
    reconcile_alert_source_credential_binding_tx(&mut tx, id, input.credential_secret_id).await?;
    tx.commit().await.map_err(classify_sqlx_error_owned)?;

    fetch_alert_source_by_id(pool, id)
        .await?
        .ok_or_else(|| AppError::Internal("Created alert source could not be reloaded".to_string()))
}

/// Update all mutable fields of an existing alert source (full replacement).
/// Returns `None` when no source with `id` exists.
pub(super) async fn update_alert_source(
    pool: &PgPool,
    id: Uuid,
    input: &AlertSourceInput,
) -> Result<Option<AlertSourceRecord>, AppError> {
    let mut tx = pool.begin().await.map_err(classify_sqlx_error_owned)?;
    let result = sqlx::query(
        r#"
        UPDATE monitoring_alert_source
        SET name = $2,
            source_type = $3,
            query_url = $4,
            timeout_ms = $5,
            options = $6,
            enabled = $7,
            updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(id)
    .bind(&input.name)
    .bind(&input.source_type)
    .bind(&input.query_url)
    .bind(input.timeout_ms)
    .bind(&input.options)
    .bind(input.enabled)
    .execute(&mut *tx)
    .await
    .map_err(classify_sqlx_error_owned)?;

    if result.rows_affected() == 0 {
        tx.rollback().await.map_err(classify_sqlx_error_owned)?;
        return Ok(None);
    }

    reconcile_alert_source_credential_binding_tx(&mut tx, id, input.credential_secret_id).await?;
    tx.commit().await.map_err(classify_sqlx_error_owned)?;

    fetch_alert_source_by_id(pool, id).await
}

/// Delete an alert source by id. Returns `true` when a row was removed.
pub(super) async fn delete_alert_source(pool: &PgPool, id: Uuid) -> Result<bool, AppError> {
    let mut tx = pool.begin().await.map_err(classify_sqlx_error_owned)?;
    let source_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM monitoring_alert_source WHERE id = $1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(classify_sqlx_error_owned)?;
    if source_id.is_none() {
        tx.rollback().await.map_err(classify_sqlx_error_owned)?;
        return Ok(false);
    }

    delete_alert_source_usage_bindings_tx(&mut tx, id).await?;
    let result = sqlx::query("DELETE FROM monitoring_alert_source WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
    tx.commit().await.map_err(classify_sqlx_error_owned)?;

    Ok(result.rows_affected() > 0)
}

// ===========================================================================
// monitoring_alert_source_assignment
// ===========================================================================

/// A row from `monitoring_alert_source_assignment`, joined with its source.
#[derive(Debug, Clone)]
pub(super) struct AlertSourceAssignmentRecord {
    pub id: Uuid,
    pub alert_source_id: Uuid,
    pub alert_source_name: String,
    pub scope_type: String,
    pub scope_id: Option<Uuid>,
    pub weight: i32,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Mutable fields accepted on create/update of an alert source assignment.
#[derive(Debug, Clone)]
pub(super) struct AlertSourceAssignmentInput {
    pub alert_source_id: Uuid,
    pub scope_type: String,
    pub scope_id: Option<Uuid>,
    pub weight: i32,
    pub enabled: bool,
}

fn map_alert_source_assignment_row(
    row: &sqlx::postgres::PgRow,
) -> Result<AlertSourceAssignmentRecord, AppError> {
    Ok(AlertSourceAssignmentRecord {
        id: row.try_get("id").map_err(classify_sqlx_error_owned)?,
        alert_source_id: row
            .try_get("alert_source_id")
            .map_err(classify_sqlx_error_owned)?,
        alert_source_name: row
            .try_get("alert_source_name")
            .map_err(classify_sqlx_error_owned)?,
        scope_type: row
            .try_get("scope_type")
            .map_err(classify_sqlx_error_owned)?,
        scope_id: row.try_get("scope_id").ok(),
        weight: row.try_get("weight").map_err(classify_sqlx_error_owned)?,
        enabled: row.try_get("enabled").map_err(classify_sqlx_error_owned)?,
        created_at: row
            .try_get("created_at")
            .map_err(classify_sqlx_error_owned)?,
        updated_at: row
            .try_get("updated_at")
            .map_err(classify_sqlx_error_owned)?,
    })
}

/// List alert source assignments (joined with their source), optionally
/// filtered by `scope_type` and/or `scope_id`.
pub(super) async fn list_alert_source_assignments(
    pool: &PgPool,
    filter: &MonitoringListFilter,
) -> Result<Vec<AlertSourceAssignmentRecord>, AppError> {
    let mut builder = QueryBuilder::new(
        r#"
        SELECT
            a.id,
            a.alert_source_id,
            s.name AS alert_source_name,
            a.scope_type,
            a.scope_id,
            a.weight,
            a.enabled,
            a.created_at,
            a.updated_at
        FROM monitoring_alert_source_assignment a
        JOIN monitoring_alert_source s ON s.id = a.alert_source_id
        "#,
    );
    push_list_filters(&mut builder, ALERT_SOURCE_ASSIGNMENT_COLUMNS, filter);
    push_order_by(
        &mut builder,
        ALERT_SOURCE_ASSIGNMENT_COLUMNS,
        filter.sort.as_ref(),
        "a.scope_type, a.weight, a.created_at",
    );
    let rows = builder
        .build()
        .fetch_all(pool)
        .await
        .map_err(classify_sqlx_error_owned)?;

    rows.iter().map(map_alert_source_assignment_row).collect()
}

async fn fetch_alert_source_assignment_by_id(
    pool: &PgPool,
    id: Uuid,
) -> Result<Option<AlertSourceAssignmentRecord>, AppError> {
    let row = sqlx::query(
        r#"
        SELECT
            a.id,
            a.alert_source_id,
            s.name AS alert_source_name,
            a.scope_type,
            a.scope_id,
            a.weight,
            a.enabled,
            a.created_at,
            a.updated_at
        FROM monitoring_alert_source_assignment a
        JOIN monitoring_alert_source s ON s.id = a.alert_source_id
        WHERE a.id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(classify_sqlx_error_owned)?;

    row.as_ref()
        .map(map_alert_source_assignment_row)
        .transpose()
}

/// Insert a new alert source assignment and return the joined record.
pub(super) async fn create_alert_source_assignment(
    pool: &PgPool,
    input: &AlertSourceAssignmentInput,
) -> Result<AlertSourceAssignmentRecord, AppError> {
    let row = sqlx::query(
        r#"
        INSERT INTO monitoring_alert_source_assignment
            (alert_source_id, scope_type, scope_id, weight, enabled)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id
        "#,
    )
    .bind(input.alert_source_id)
    .bind(&input.scope_type)
    .bind(input.scope_id)
    .bind(input.weight)
    .bind(input.enabled)
    .fetch_one(pool)
    .await
    .map_err(classify_sqlx_error_owned)?;

    let id: Uuid = row.try_get("id").map_err(classify_sqlx_error_owned)?;
    fetch_alert_source_assignment_by_id(pool, id)
        .await?
        .ok_or_else(|| {
            AppError::Internal("Created alert source assignment could not be reloaded".to_string())
        })
}

/// Update all mutable fields of an existing alert source assignment.
/// Returns `None` when no assignment with `id` exists.
pub(super) async fn update_alert_source_assignment(
    pool: &PgPool,
    id: Uuid,
    input: &AlertSourceAssignmentInput,
) -> Result<Option<AlertSourceAssignmentRecord>, AppError> {
    let result = sqlx::query(
        r#"
        UPDATE monitoring_alert_source_assignment
        SET alert_source_id = $2,
            scope_type = $3,
            scope_id = $4,
            weight = $5,
            enabled = $6,
            updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(id)
    .bind(input.alert_source_id)
    .bind(&input.scope_type)
    .bind(input.scope_id)
    .bind(input.weight)
    .bind(input.enabled)
    .execute(pool)
    .await
    .map_err(classify_sqlx_error_owned)?;

    if result.rows_affected() == 0 {
        return Ok(None);
    }

    fetch_alert_source_assignment_by_id(pool, id).await
}

/// Delete an alert source assignment by id. Returns `true` when a row was removed.
pub(super) async fn delete_alert_source_assignment(
    pool: &PgPool,
    id: Uuid,
) -> Result<bool, AppError> {
    let result = sqlx::query("DELETE FROM monitoring_alert_source_assignment WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .map_err(classify_sqlx_error_owned)?;

    Ok(result.rows_affected() > 0)
}

// ===========================================================================
// monitoring_metric_query
// ===========================================================================

/// A row from `monitoring_metric_query`.
#[derive(Debug, Clone)]
pub(super) struct MetricQueryRecord {
    pub id: Uuid,
    pub exporter_id: Option<Uuid>,
    pub source_type: String,
    pub query_type: String,
    pub value_type: String,
    pub widget_type: String,
    pub name: String,
    pub slug: String,
    pub label: Option<String>,
    pub query: String,
    pub unit: Option<String>,
    pub description: Option<String>,
    pub default_show_in_inspector: bool,
    pub default_show_in_list: bool,
    pub sort_order: i32,
    pub options: Value,
    pub enabled: bool,
    pub category: Option<String>,
    pub semantic: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Mutable fields accepted on create/update of a metric query.
#[derive(Debug, Clone)]
pub(crate) struct MetricQueryInput {
    pub exporter_id: Option<Uuid>,
    pub source_type: String,
    pub query_type: String,
    pub value_type: String,
    pub widget_type: String,
    pub name: String,
    pub slug: String,
    pub label: Option<String>,
    pub query: String,
    pub unit: Option<String>,
    pub description: Option<String>,
    pub default_show_in_inspector: bool,
    pub default_show_in_list: bool,
    pub sort_order: i32,
    pub options: Value,
    pub enabled: bool,
    pub category: Option<String>,
    pub semantic: Option<String>,
}

fn map_metric_query_row(row: &sqlx::postgres::PgRow) -> Result<MetricQueryRecord, AppError> {
    Ok(MetricQueryRecord {
        id: row.try_get("id").map_err(classify_sqlx_error_owned)?,
        exporter_id: row.try_get("exporter_id").ok(),
        source_type: row
            .try_get("source_type")
            .map_err(classify_sqlx_error_owned)?,
        query_type: row
            .try_get("query_type")
            .map_err(classify_sqlx_error_owned)?,
        value_type: row
            .try_get("value_type")
            .map_err(classify_sqlx_error_owned)?,
        widget_type: row
            .try_get("widget_type")
            .map_err(classify_sqlx_error_owned)?,
        name: row.try_get("name").map_err(classify_sqlx_error_owned)?,
        slug: row.try_get("slug").map_err(classify_sqlx_error_owned)?,
        label: row.try_get("label").ok(),
        query: row.try_get("query").map_err(classify_sqlx_error_owned)?,
        unit: row.try_get("unit").ok(),
        description: row.try_get("description").ok(),
        default_show_in_inspector: row
            .try_get("default_show_in_inspector")
            .map_err(classify_sqlx_error_owned)?,
        default_show_in_list: row
            .try_get("default_show_in_list")
            .map_err(classify_sqlx_error_owned)?,
        sort_order: row
            .try_get("sort_order")
            .map_err(classify_sqlx_error_owned)?,
        options: row
            .try_get("options")
            .unwrap_or_else(|_| Value::Object(serde_json::Map::new())),
        enabled: row.try_get("enabled").map_err(classify_sqlx_error_owned)?,
        category: row.try_get("category").ok(),
        semantic: row.try_get("semantic").ok(),
        created_at: row
            .try_get("created_at")
            .map_err(classify_sqlx_error_owned)?,
        updated_at: row
            .try_get("updated_at")
            .map_err(classify_sqlx_error_owned)?,
    })
}

/// List metric queries, optionally filtered by `exporter_id` and/or `source_type`.
pub(super) async fn list_metric_queries(
    pool: &PgPool,
    filter: &MonitoringListFilter,
) -> Result<Vec<MetricQueryRecord>, AppError> {
    let mut builder = QueryBuilder::new(
        r#"
        SELECT id, exporter_id, source_type, query_type, value_type, widget_type,
               name, slug, label, query, unit, description,
               default_show_in_inspector, default_show_in_list, sort_order,
               options, enabled, category, semantic, created_at, updated_at
        FROM monitoring_metric_query
        "#,
    );
    push_list_filters(&mut builder, METRIC_QUERY_COLUMNS, filter);
    push_order_by(
        &mut builder,
        METRIC_QUERY_COLUMNS,
        filter.sort.as_ref(),
        "source_type, sort_order, name",
    );
    let rows = builder
        .build()
        .fetch_all(pool)
        .await
        .map_err(classify_sqlx_error_owned)?;

    rows.iter().map(map_metric_query_row).collect()
}

async fn fetch_metric_query_by_id(
    pool: &PgPool,
    id: Uuid,
) -> Result<Option<MetricQueryRecord>, AppError> {
    let row = sqlx::query(
        r#"
        SELECT id, exporter_id, source_type, query_type, value_type, widget_type,
               name, slug, label, query, unit, description,
               default_show_in_inspector, default_show_in_list, sort_order,
               options, enabled, category, semantic, created_at, updated_at
        FROM monitoring_metric_query
        WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(classify_sqlx_error_owned)?;

    row.as_ref().map(map_metric_query_row).transpose()
}

/// Insert a new metric query and return the stored record.
pub(super) async fn create_metric_query(
    pool: &PgPool,
    input: &MetricQueryInput,
) -> Result<MetricQueryRecord, AppError> {
    let row = sqlx::query(
        r#"
        INSERT INTO monitoring_metric_query
            (exporter_id, source_type, query_type, value_type, widget_type,
             name, slug, label, query, unit, description,
             default_show_in_inspector, default_show_in_list, sort_order,
             options, enabled, category, semantic)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18)
        RETURNING id
        "#,
    )
    .bind(input.exporter_id)
    .bind(&input.source_type)
    .bind(&input.query_type)
    .bind(&input.value_type)
    .bind(&input.widget_type)
    .bind(&input.name)
    .bind(&input.slug)
    .bind(input.label.as_deref())
    .bind(&input.query)
    .bind(input.unit.as_deref())
    .bind(input.description.as_deref())
    .bind(input.default_show_in_inspector)
    .bind(input.default_show_in_list)
    .bind(input.sort_order)
    .bind(&input.options)
    .bind(input.enabled)
    .bind(input.category.as_deref())
    .bind(input.semantic.as_deref())
    .fetch_one(pool)
    .await
    .map_err(classify_sqlx_error_owned)?;

    let id: Uuid = row.try_get("id").map_err(classify_sqlx_error_owned)?;
    fetch_metric_query_by_id(pool, id)
        .await?
        .ok_or_else(|| AppError::Internal("Created metric query could not be reloaded".to_string()))
}

/// Update all mutable fields of an existing metric query (full replacement).
/// Returns `None` when no metric query with `id` exists.
pub(super) async fn update_metric_query(
    pool: &PgPool,
    id: Uuid,
    input: &MetricQueryInput,
) -> Result<Option<MetricQueryRecord>, AppError> {
    let result = sqlx::query(
        r#"
        UPDATE monitoring_metric_query
        SET exporter_id = $2,
            source_type = $3,
            query_type = $4,
            value_type = $5,
            widget_type = $6,
            name = $7,
            slug = $8,
            label = $9,
            query = $10,
            unit = $11,
            description = $12,
            default_show_in_inspector = $13,
            default_show_in_list = $14,
            sort_order = $15,
            options = $16,
            enabled = $17,
            category = $18,
            semantic = $19,
            updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(id)
    .bind(input.exporter_id)
    .bind(&input.source_type)
    .bind(&input.query_type)
    .bind(&input.value_type)
    .bind(&input.widget_type)
    .bind(&input.name)
    .bind(&input.slug)
    .bind(input.label.as_deref())
    .bind(&input.query)
    .bind(input.unit.as_deref())
    .bind(input.description.as_deref())
    .bind(input.default_show_in_inspector)
    .bind(input.default_show_in_list)
    .bind(input.sort_order)
    .bind(&input.options)
    .bind(input.enabled)
    .bind(input.category.as_deref())
    .bind(input.semantic.as_deref())
    .execute(pool)
    .await
    .map_err(classify_sqlx_error_owned)?;

    if result.rows_affected() == 0 {
        return Ok(None);
    }

    fetch_metric_query_by_id(pool, id).await
}

/// Delete a metric query by id. Returns `true` when a row was removed.
pub(super) async fn delete_metric_query(pool: &PgPool, id: Uuid) -> Result<bool, AppError> {
    let result = sqlx::query("DELETE FROM monitoring_metric_query WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .map_err(classify_sqlx_error_owned)?;

    Ok(result.rows_affected() > 0)
}

// ===========================================================================
// monitoring_metric_query_host_override
// ===========================================================================

/// A row from `monitoring_metric_query_host_override`.
///
/// The `enabled` column is intentionally NOT projected onto the public DTO —
/// the public surface treats "exists" as "active", and the column only exists
/// to support future operator tooling that flips rows without deleting them.
#[derive(Debug, Clone)]
pub(super) struct HostOverrideRecord {
    pub id: Uuid,
    pub metric_query_id: Uuid,
    pub host_id: Uuid,
    pub label_override: Option<String>,
    pub unit_override: Option<String>,
    pub options_override: Option<Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Mutable fields accepted on create/update of a host override.
#[derive(Debug, Clone)]
pub(crate) struct HostOverrideInput {
    pub metric_query_id: Uuid,
    pub host_id: Uuid,
    pub label_override: Option<String>,
    pub unit_override: Option<String>,
    pub options_override: Option<Value>,
}

fn map_host_override_row(row: &sqlx::postgres::PgRow) -> Result<HostOverrideRecord, AppError> {
    Ok(HostOverrideRecord {
        id: row.try_get("id").map_err(classify_sqlx_error_owned)?,
        metric_query_id: row
            .try_get("metric_query_id")
            .map_err(classify_sqlx_error_owned)?,
        host_id: row.try_get("host_id").map_err(classify_sqlx_error_owned)?,
        label_override: row.try_get("label_override").ok(),
        unit_override: row.try_get("unit_override").ok(),
        options_override: row
            .try_get::<Option<Value>, _>("options_override")
            .ok()
            .flatten(),
        created_at: row
            .try_get("created_at")
            .map_err(classify_sqlx_error_owned)?,
        updated_at: row
            .try_get("updated_at")
            .map_err(classify_sqlx_error_owned)?,
    })
}

/// List host overrides for a single host. The public API requires `host_id`
/// so this repository function does not expose an "all hosts" variant — that
/// would let any caller with `Monitoring:Read` enumerate overrides across the
/// platform.
pub(super) async fn list_host_overrides(
    pool: &PgPool,
    host_id: Uuid,
) -> Result<Vec<HostOverrideRecord>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT id, metric_query_id, host_id, label_override, unit_override,
               options_override, created_at, updated_at
        FROM monitoring_metric_query_host_override
        WHERE host_id = $1
        ORDER BY metric_query_id
        "#,
    )
    .bind(host_id)
    .fetch_all(pool)
    .await
    .map_err(classify_sqlx_error_owned)?;
    rows.iter().map(map_host_override_row).collect()
}

pub(super) async fn fetch_host_override_by_id(
    pool: &PgPool,
    id: Uuid,
) -> Result<Option<HostOverrideRecord>, AppError> {
    let row = sqlx::query(
        r#"
        SELECT id, metric_query_id, host_id, label_override, unit_override,
               options_override, created_at, updated_at
        FROM monitoring_metric_query_host_override
        WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(classify_sqlx_error_owned)?;
    row.as_ref().map(map_host_override_row).transpose()
}

pub(super) async fn create_host_override(
    pool: &PgPool,
    input: &HostOverrideInput,
) -> Result<HostOverrideRecord, AppError> {
    // `enabled` is not part of the public DTO; the column keeps its DB default
    // of `true`. The merge step (`fetch_host_overrides_for_hosts`) still filters
    // on the column so a future "soft-disable" tool can flip rows directly.
    let row = sqlx::query(
        r#"
        INSERT INTO monitoring_metric_query_host_override
            (metric_query_id, host_id, label_override, unit_override,
             options_override)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id, metric_query_id, host_id, label_override, unit_override,
                  options_override, created_at, updated_at
        "#,
    )
    .bind(input.metric_query_id)
    .bind(input.host_id)
    .bind(input.label_override.as_deref())
    .bind(input.unit_override.as_deref())
    .bind(input.options_override.clone())
    .fetch_one(pool)
    .await
    .map_err(classify_sqlx_error_owned)?;
    map_host_override_row(&row)
}

pub(super) async fn update_host_override(
    pool: &PgPool,
    id: Uuid,
    input: &HostOverrideInput,
) -> Result<Option<HostOverrideRecord>, AppError> {
    // `enabled` is intentionally left untouched — the DTO does not expose it.
    let result = sqlx::query(
        r#"
        UPDATE monitoring_metric_query_host_override
        SET metric_query_id = $2,
            host_id = $3,
            label_override = $4,
            unit_override = $5,
            options_override = $6,
            updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(id)
    .bind(input.metric_query_id)
    .bind(input.host_id)
    .bind(input.label_override.as_deref())
    .bind(input.unit_override.as_deref())
    .bind(input.options_override.clone())
    .execute(pool)
    .await
    .map_err(classify_sqlx_error_owned)?;
    if result.rows_affected() == 0 {
        return Ok(None);
    }
    fetch_host_override_by_id(pool, id).await
}

/// Delete one host override. Returns the deleted row's `host_id` when a row
/// was removed so the caller can invalidate the per-host `obs:host:{id}`
/// Redis cache. `None` means the id did not match any row.
pub(super) async fn delete_host_override(
    pool: &PgPool,
    id: Uuid,
) -> Result<Option<Uuid>, AppError> {
    let row = sqlx::query(
        r#"
        DELETE FROM monitoring_metric_query_host_override
        WHERE id = $1
        RETURNING host_id
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(classify_sqlx_error_owned)?;
    row.map(|r| {
        r.try_get::<Uuid, _>("host_id")
            .map_err(classify_sqlx_error_owned)
    })
    .transpose()
}

// ===========================================================================
// monitoring_metric_set (+ items)
// ===========================================================================

/// A row from `monitoring_metric_set`.
#[derive(Debug, Clone)]
pub(super) struct MetricSetRecord {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Mutable fields accepted on create/update of a metric set.
#[derive(Debug, Clone)]
pub(super) struct MetricSetInput {
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
    pub enabled: bool,
}

/// A row from `monitoring_metric_set_item`, joined with its metric query.
#[derive(Debug, Clone)]
pub(super) struct MetricSetItemRecord {
    pub id: Uuid,
    pub metric_set_id: Uuid,
    pub metric_query_id: Uuid,
    pub metric_query_name: String,
    pub metric_query_slug: String,
    pub show_in_inspector: bool,
    pub show_in_list: bool,
    pub show_in_hover: bool,
    pub list_render: String,
    pub sort_order: i32,
    pub label_override: Option<String>,
    pub unit_override: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Mutable fields accepted on create/update of a metric set item.
#[derive(Debug, Clone)]
pub(super) struct MetricSetItemInput {
    pub metric_query_id: Uuid,
    pub show_in_inspector: bool,
    pub show_in_list: bool,
    pub show_in_hover: bool,
    pub list_render: String,
    pub sort_order: i32,
    pub label_override: Option<String>,
    pub unit_override: Option<String>,
}

fn map_metric_set_row(row: &sqlx::postgres::PgRow) -> Result<MetricSetRecord, AppError> {
    Ok(MetricSetRecord {
        id: row.try_get("id").map_err(classify_sqlx_error_owned)?,
        name: row.try_get("name").map_err(classify_sqlx_error_owned)?,
        slug: row.try_get("slug").map_err(classify_sqlx_error_owned)?,
        description: row.try_get("description").ok(),
        enabled: row.try_get("enabled").map_err(classify_sqlx_error_owned)?,
        created_at: row
            .try_get("created_at")
            .map_err(classify_sqlx_error_owned)?,
        updated_at: row
            .try_get("updated_at")
            .map_err(classify_sqlx_error_owned)?,
    })
}

fn map_metric_set_item_row(row: &sqlx::postgres::PgRow) -> Result<MetricSetItemRecord, AppError> {
    Ok(MetricSetItemRecord {
        id: row.try_get("id").map_err(classify_sqlx_error_owned)?,
        metric_set_id: row
            .try_get("metric_set_id")
            .map_err(classify_sqlx_error_owned)?,
        metric_query_id: row
            .try_get("metric_query_id")
            .map_err(classify_sqlx_error_owned)?,
        metric_query_name: row
            .try_get("metric_query_name")
            .map_err(classify_sqlx_error_owned)?,
        metric_query_slug: row
            .try_get("metric_query_slug")
            .map_err(classify_sqlx_error_owned)?,
        show_in_inspector: row
            .try_get("show_in_inspector")
            .map_err(classify_sqlx_error_owned)?,
        show_in_list: row
            .try_get("show_in_list")
            .map_err(classify_sqlx_error_owned)?,
        show_in_hover: row
            .try_get("show_in_hover")
            .map_err(classify_sqlx_error_owned)?,
        list_render: row
            .try_get("list_render")
            .map_err(classify_sqlx_error_owned)?,
        sort_order: row
            .try_get("sort_order")
            .map_err(classify_sqlx_error_owned)?,
        label_override: row.try_get("label_override").ok(),
        unit_override: row.try_get("unit_override").ok(),
        created_at: row
            .try_get("created_at")
            .map_err(classify_sqlx_error_owned)?,
        updated_at: row
            .try_get("updated_at")
            .map_err(classify_sqlx_error_owned)?,
    })
}

/// List metric sets.
pub(super) async fn list_metric_sets(
    pool: &PgPool,
    filter: &MonitoringListFilter,
) -> Result<Vec<MetricSetRecord>, AppError> {
    let mut builder = QueryBuilder::new(
        r#"
        SELECT id, name, slug, description, enabled, created_at, updated_at
        FROM monitoring_metric_set
        "#,
    );
    push_list_filters(&mut builder, METRIC_SET_COLUMNS, filter);
    push_order_by(
        &mut builder,
        METRIC_SET_COLUMNS,
        filter.sort.as_ref(),
        "slug",
    );
    let rows = builder
        .build()
        .fetch_all(pool)
        .await
        .map_err(classify_sqlx_error_owned)?;

    rows.iter().map(map_metric_set_row).collect()
}

/// List the items of the given metric sets (joined with their metric queries).
pub(super) async fn list_metric_set_items(
    pool: &PgPool,
    metric_set_ids: &[Uuid],
) -> Result<Vec<MetricSetItemRecord>, AppError> {
    if metric_set_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT
            i.id,
            i.metric_set_id,
            i.metric_query_id,
            q.name AS metric_query_name,
            q.slug AS metric_query_slug,
            i.show_in_inspector,
            i.show_in_list,
            i.show_in_hover,
            i.list_render,
            i.sort_order,
            i.label_override,
            i.unit_override,
            i.created_at,
            i.updated_at
        FROM monitoring_metric_set_item i
        JOIN monitoring_metric_query q ON q.id = i.metric_query_id
        WHERE i.metric_set_id = ANY($1::uuid[])
        ORDER BY i.metric_set_id, i.sort_order, q.name
        "#,
    )
    .bind(metric_set_ids)
    .fetch_all(pool)
    .await
    .map_err(classify_sqlx_error_owned)?;

    rows.iter().map(map_metric_set_item_row).collect()
}

async fn fetch_metric_set_by_id(
    pool: &PgPool,
    id: Uuid,
) -> Result<Option<MetricSetRecord>, AppError> {
    let row = sqlx::query(
        r#"
        SELECT id, name, slug, description, enabled, created_at, updated_at
        FROM monitoring_metric_set
        WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(classify_sqlx_error_owned)?;

    row.as_ref().map(map_metric_set_row).transpose()
}

async fn fetch_metric_set_item_by_id(
    pool: &PgPool,
    metric_set_id: Uuid,
    item_id: Uuid,
) -> Result<Option<MetricSetItemRecord>, AppError> {
    let row = sqlx::query(
        r#"
        SELECT
            i.id,
            i.metric_set_id,
            i.metric_query_id,
            q.name AS metric_query_name,
            q.slug AS metric_query_slug,
            i.show_in_inspector,
            i.show_in_list,
            i.show_in_hover,
            i.list_render,
            i.sort_order,
            i.label_override,
            i.unit_override,
            i.created_at,
            i.updated_at
        FROM monitoring_metric_set_item i
        JOIN monitoring_metric_query q ON q.id = i.metric_query_id
        WHERE i.id = $1 AND i.metric_set_id = $2
        "#,
    )
    .bind(item_id)
    .bind(metric_set_id)
    .fetch_optional(pool)
    .await
    .map_err(classify_sqlx_error_owned)?;

    row.as_ref().map(map_metric_set_item_row).transpose()
}

/// Returns `true` when a metric set with `id` exists.
pub(super) async fn metric_set_exists(pool: &PgPool, id: Uuid) -> Result<bool, AppError> {
    Ok(fetch_metric_set_by_id(pool, id).await?.is_some())
}

/// Insert a new metric set and return the stored record.
pub(super) async fn create_metric_set(
    pool: &PgPool,
    input: &MetricSetInput,
) -> Result<MetricSetRecord, AppError> {
    let row = sqlx::query(
        r#"
        INSERT INTO monitoring_metric_set (name, slug, description, enabled)
        VALUES ($1, $2, $3, $4)
        RETURNING id
        "#,
    )
    .bind(&input.name)
    .bind(&input.slug)
    .bind(input.description.as_deref())
    .bind(input.enabled)
    .fetch_one(pool)
    .await
    .map_err(classify_sqlx_error_owned)?;

    let id: Uuid = row.try_get("id").map_err(classify_sqlx_error_owned)?;
    fetch_metric_set_by_id(pool, id)
        .await?
        .ok_or_else(|| AppError::Internal("Created metric set could not be reloaded".to_string()))
}

/// Update all mutable fields of an existing metric set (full replacement).
/// Returns `None` when no metric set with `id` exists.
pub(super) async fn update_metric_set(
    pool: &PgPool,
    id: Uuid,
    input: &MetricSetInput,
) -> Result<Option<MetricSetRecord>, AppError> {
    let result = sqlx::query(
        r#"
        UPDATE monitoring_metric_set
        SET name = $2,
            slug = $3,
            description = $4,
            enabled = $5,
            updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(id)
    .bind(&input.name)
    .bind(&input.slug)
    .bind(input.description.as_deref())
    .bind(input.enabled)
    .execute(pool)
    .await
    .map_err(classify_sqlx_error_owned)?;

    if result.rows_affected() == 0 {
        return Ok(None);
    }

    fetch_metric_set_by_id(pool, id).await
}

/// Delete a metric set by id. Returns `true` when a row was removed.
pub(super) async fn delete_metric_set(pool: &PgPool, id: Uuid) -> Result<bool, AppError> {
    let result = sqlx::query("DELETE FROM monitoring_metric_set WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .map_err(classify_sqlx_error_owned)?;

    Ok(result.rows_affected() > 0)
}

/// Insert a new metric set item and return the joined record.
pub(super) async fn create_metric_set_item(
    pool: &PgPool,
    metric_set_id: Uuid,
    input: &MetricSetItemInput,
) -> Result<MetricSetItemRecord, AppError> {
    let row = sqlx::query(
        r#"
        INSERT INTO monitoring_metric_set_item
            (metric_set_id, metric_query_id, show_in_inspector, show_in_list,
             show_in_hover, list_render, sort_order, label_override,
             unit_override)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        RETURNING id
        "#,
    )
    .bind(metric_set_id)
    .bind(input.metric_query_id)
    .bind(input.show_in_inspector)
    .bind(input.show_in_list)
    .bind(input.show_in_hover)
    .bind(&input.list_render)
    .bind(input.sort_order)
    .bind(input.label_override.as_deref())
    .bind(input.unit_override.as_deref())
    .fetch_one(pool)
    .await
    .map_err(classify_sqlx_error_owned)?;

    let id: Uuid = row.try_get("id").map_err(classify_sqlx_error_owned)?;
    fetch_metric_set_item_by_id(pool, metric_set_id, id)
        .await?
        .ok_or_else(|| {
            AppError::Internal("Created metric set item could not be reloaded".to_string())
        })
}

/// Update all mutable fields of an existing metric set item.
/// Returns `None` when no item with `item_id` exists in the given set.
pub(super) async fn update_metric_set_item(
    pool: &PgPool,
    metric_set_id: Uuid,
    item_id: Uuid,
    input: &MetricSetItemInput,
) -> Result<Option<MetricSetItemRecord>, AppError> {
    let result = sqlx::query(
        r#"
        UPDATE monitoring_metric_set_item
        SET metric_query_id = $3,
            show_in_inspector = $4,
            show_in_list = $5,
            show_in_hover = $6,
            list_render = $7,
            sort_order = $8,
            label_override = $9,
            unit_override = $10,
            updated_at = now()
        WHERE id = $1 AND metric_set_id = $2
        "#,
    )
    .bind(item_id)
    .bind(metric_set_id)
    .bind(input.metric_query_id)
    .bind(input.show_in_inspector)
    .bind(input.show_in_list)
    .bind(input.show_in_hover)
    .bind(&input.list_render)
    .bind(input.sort_order)
    .bind(input.label_override.as_deref())
    .bind(input.unit_override.as_deref())
    .execute(pool)
    .await
    .map_err(classify_sqlx_error_owned)?;

    if result.rows_affected() == 0 {
        return Ok(None);
    }

    fetch_metric_set_item_by_id(pool, metric_set_id, item_id).await
}

/// Delete a metric set item by id within the given set. Returns `true` when a
/// row was removed.
pub(super) async fn delete_metric_set_item(
    pool: &PgPool,
    metric_set_id: Uuid,
    item_id: Uuid,
) -> Result<bool, AppError> {
    let result =
        sqlx::query("DELETE FROM monitoring_metric_set_item WHERE id = $1 AND metric_set_id = $2")
            .bind(item_id)
            .bind(metric_set_id)
            .execute(pool)
            .await
            .map_err(classify_sqlx_error_owned)?;

    Ok(result.rows_affected() > 0)
}

// ===========================================================================
// monitoring_metric_set_assignment
// ===========================================================================

/// A row from `monitoring_metric_set_assignment`, joined with its set.
#[derive(Debug, Clone)]
pub(super) struct MetricSetAssignmentRecord {
    pub id: Uuid,
    pub metric_set_id: Uuid,
    pub metric_set_slug: String,
    pub metric_set_name: String,
    pub scope_type: String,
    pub scope_id: Option<Uuid>,
    pub weight: i32,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Mutable fields accepted on create/update of a metric set assignment.
#[derive(Debug, Clone)]
pub(super) struct MetricSetAssignmentInput {
    pub metric_set_id: Uuid,
    pub scope_type: String,
    pub scope_id: Option<Uuid>,
    pub weight: i32,
    pub enabled: bool,
}

fn map_metric_set_assignment_row(
    row: &sqlx::postgres::PgRow,
) -> Result<MetricSetAssignmentRecord, AppError> {
    Ok(MetricSetAssignmentRecord {
        id: row.try_get("id").map_err(classify_sqlx_error_owned)?,
        metric_set_id: row
            .try_get("metric_set_id")
            .map_err(classify_sqlx_error_owned)?,
        metric_set_slug: row
            .try_get("metric_set_slug")
            .map_err(classify_sqlx_error_owned)?,
        metric_set_name: row
            .try_get("metric_set_name")
            .map_err(classify_sqlx_error_owned)?,
        scope_type: row
            .try_get("scope_type")
            .map_err(classify_sqlx_error_owned)?,
        scope_id: row.try_get("scope_id").ok(),
        weight: row.try_get("weight").map_err(classify_sqlx_error_owned)?,
        enabled: row.try_get("enabled").map_err(classify_sqlx_error_owned)?,
        created_at: row
            .try_get("created_at")
            .map_err(classify_sqlx_error_owned)?,
        updated_at: row
            .try_get("updated_at")
            .map_err(classify_sqlx_error_owned)?,
    })
}

/// List metric set assignments (joined with their set), optionally filtered by
/// `scope_type` and/or `scope_id`.
pub(super) async fn list_metric_set_assignments(
    pool: &PgPool,
    filter: &MonitoringListFilter,
) -> Result<Vec<MetricSetAssignmentRecord>, AppError> {
    let mut builder = QueryBuilder::new(
        r#"
        SELECT
            a.id,
            a.metric_set_id,
            s.slug AS metric_set_slug,
            s.name AS metric_set_name,
            a.scope_type,
            a.scope_id,
            a.weight,
            a.enabled,
            a.created_at,
            a.updated_at
        FROM monitoring_metric_set_assignment a
        JOIN monitoring_metric_set s ON s.id = a.metric_set_id
        "#,
    );
    push_list_filters(&mut builder, METRIC_SET_ASSIGNMENT_COLUMNS, filter);
    push_order_by(
        &mut builder,
        METRIC_SET_ASSIGNMENT_COLUMNS,
        filter.sort.as_ref(),
        "a.scope_type, a.weight, a.created_at",
    );
    let rows = builder
        .build()
        .fetch_all(pool)
        .await
        .map_err(classify_sqlx_error_owned)?;

    rows.iter().map(map_metric_set_assignment_row).collect()
}

async fn fetch_metric_set_assignment_by_id(
    pool: &PgPool,
    id: Uuid,
) -> Result<Option<MetricSetAssignmentRecord>, AppError> {
    let row = sqlx::query(
        r#"
        SELECT
            a.id,
            a.metric_set_id,
            s.slug AS metric_set_slug,
            s.name AS metric_set_name,
            a.scope_type,
            a.scope_id,
            a.weight,
            a.enabled,
            a.created_at,
            a.updated_at
        FROM monitoring_metric_set_assignment a
        JOIN monitoring_metric_set s ON s.id = a.metric_set_id
        WHERE a.id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(classify_sqlx_error_owned)?;

    row.as_ref().map(map_metric_set_assignment_row).transpose()
}

/// Insert a new metric set assignment and return the joined record.
pub(super) async fn create_metric_set_assignment(
    pool: &PgPool,
    input: &MetricSetAssignmentInput,
) -> Result<MetricSetAssignmentRecord, AppError> {
    let row = sqlx::query(
        r#"
        INSERT INTO monitoring_metric_set_assignment
            (metric_set_id, scope_type, scope_id, weight, enabled)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id
        "#,
    )
    .bind(input.metric_set_id)
    .bind(&input.scope_type)
    .bind(input.scope_id)
    .bind(input.weight)
    .bind(input.enabled)
    .fetch_one(pool)
    .await
    .map_err(classify_sqlx_error_owned)?;

    let id: Uuid = row.try_get("id").map_err(classify_sqlx_error_owned)?;
    fetch_metric_set_assignment_by_id(pool, id)
        .await?
        .ok_or_else(|| {
            AppError::Internal("Created metric set assignment could not be reloaded".to_string())
        })
}

/// Update all mutable fields of an existing metric set assignment.
/// Returns `None` when no assignment with `id` exists.
pub(super) async fn update_metric_set_assignment(
    pool: &PgPool,
    id: Uuid,
    input: &MetricSetAssignmentInput,
) -> Result<Option<MetricSetAssignmentRecord>, AppError> {
    let result = sqlx::query(
        r#"
        UPDATE monitoring_metric_set_assignment
        SET metric_set_id = $2,
            scope_type = $3,
            scope_id = $4,
            weight = $5,
            enabled = $6,
            updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(id)
    .bind(input.metric_set_id)
    .bind(&input.scope_type)
    .bind(input.scope_id)
    .bind(input.weight)
    .bind(input.enabled)
    .execute(pool)
    .await
    .map_err(classify_sqlx_error_owned)?;

    if result.rows_affected() == 0 {
        return Ok(None);
    }

    fetch_metric_set_assignment_by_id(pool, id).await
}

/// Delete a metric set assignment by id. Returns `true` when a row was removed.
pub(super) async fn delete_metric_set_assignment(
    pool: &PgPool,
    id: Uuid,
) -> Result<bool, AppError> {
    let result = sqlx::query("DELETE FROM monitoring_metric_set_assignment WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .map_err(classify_sqlx_error_owned)?;

    Ok(result.rows_affected() > 0)
}

// ===========================================================================
// Exporter preset apply + quick-start composites
//
// These power `POST /api/v1/monitoring/exporters/{slug}/apply` and
// `POST /api/v1/monitoring/quick-start`. Both routines are atomic
// (one `pool.begin()` per call) and idempotent: re-running with the same
// (preset_slug, scope) updates the existing metric set + assignment instead
// of duplicating rows.
// ===========================================================================

use super::presets::{ExporterPreset, PresetMetricQuery};

/// Resolved ids returned by [`apply_exporter_preset_tx`] and surfaced in the
/// REST response so the UI can deep-link straight to the new resources.
#[derive(Debug, Clone)]
pub(super) struct AppliedPreset {
    pub exporter_id: Uuid,
    pub metric_set_id: Uuid,
    pub metric_set_assignment_id: Uuid,
    pub query_source_assignment_id: Uuid,
    pub metric_query_ids: Vec<Uuid>,
    pub scrape_assignment_id: Option<Uuid>,
}

async fn find_metric_query_id(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    exporter_id: Uuid,
    slug: &str,
) -> Result<Option<Uuid>, AppError> {
    let row = sqlx::query(
        r#"
        SELECT id FROM monitoring_metric_query
        WHERE exporter_id = $1 AND slug = $2
        "#,
    )
    .bind(exporter_id)
    .bind(slug)
    .fetch_optional(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?;

    row.map(|row| {
        row.try_get::<Uuid, _>("id")
            .map_err(classify_sqlx_error_owned)
    })
    .transpose()
}

async fn update_existing_preset_metric_query_semantic(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    metric_query_id: Uuid,
    spec: &PresetMetricQuery,
) -> Result<(), AppError> {
    if spec.semantic.is_none() {
        return Ok(());
    }

    sqlx::query(
        r#"
        UPDATE monitoring_metric_query
        SET semantic = $2, updated_at = now()
        WHERE id = $1
          AND semantic IS NULL
        "#,
    )
    .bind(metric_query_id)
    .bind(spec.semantic)
    .execute(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?;

    Ok(())
}

async fn insert_preset_metric_query(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    exporter_id: Uuid,
    spec: &PresetMetricQuery,
) -> Result<Uuid, AppError> {
    let options = (spec.options)();
    let row = sqlx::query(
        r#"
        INSERT INTO monitoring_metric_query
            (exporter_id, source_type, query_type, value_type, widget_type,
             name, slug, label, query, unit, description,
             default_show_in_inspector, default_show_in_list, sort_order,
             options, enabled, semantic)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, NULL,
                $11, $12, $13, $14, true, $15)
        RETURNING id
        "#,
    )
    .bind(exporter_id)
    .bind(spec.source_type)
    .bind(spec.query_type)
    .bind(spec.value_type)
    .bind(spec.widget_type)
    .bind(spec.name)
    .bind(spec.slug)
    .bind(spec.label)
    .bind(spec.query)
    .bind(spec.unit)
    .bind(spec.default_show_in_inspector)
    .bind(spec.default_show_in_list)
    .bind(spec.sort_order)
    .bind(&options)
    .bind(spec.semantic)
    .fetch_one(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?;

    row.try_get::<Uuid, _>("id")
        .map_err(classify_sqlx_error_owned)
}

async fn find_metric_set_id_by_slug(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    slug: &str,
) -> Result<Option<Uuid>, AppError> {
    let row = sqlx::query(
        r#"
        SELECT id FROM monitoring_metric_set
        WHERE slug = $1
        "#,
    )
    .bind(slug)
    .fetch_optional(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?;

    row.map(|row| {
        row.try_get::<Uuid, _>("id")
            .map_err(classify_sqlx_error_owned)
    })
    .transpose()
}

async fn insert_metric_set_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    name: &str,
    slug: &str,
    description: Option<&str>,
) -> Result<Uuid, AppError> {
    let row = sqlx::query(
        r#"
        INSERT INTO monitoring_metric_set (name, slug, description, enabled)
        VALUES ($1, $2, $3, true)
        RETURNING id
        "#,
    )
    .bind(name)
    .bind(slug)
    .bind(description)
    .fetch_one(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?;

    row.try_get::<Uuid, _>("id")
        .map_err(classify_sqlx_error_owned)
}

async fn ensure_metric_set_item_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    metric_set_id: Uuid,
    metric_query_id: Uuid,
    spec: &PresetMetricQuery,
) -> Result<(), AppError> {
    // Re-apply must NOT overwrite operator edits to existing set items —
    // mirrors the `presets.rs` "never overwrite live queries" contract.
    sqlx::query(
        r#"
        INSERT INTO monitoring_metric_set_item
            (metric_set_id, metric_query_id, show_in_inspector, show_in_list,
             show_in_hover, list_render, sort_order, label_override,
             unit_override)
        VALUES ($1, $2, $3, $4, false, 'auto', $5, NULL, NULL)
        ON CONFLICT (metric_set_id, metric_query_id) DO NOTHING
        "#,
    )
    .bind(metric_set_id)
    .bind(metric_query_id)
    .bind(spec.default_show_in_inspector)
    .bind(spec.default_show_in_list)
    .bind(spec.sort_order)
    .execute(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?;
    Ok(())
}

async fn find_metric_set_assignment_id(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    metric_set_id: Uuid,
    scope_type: &str,
    scope_id: Option<Uuid>,
) -> Result<Option<Uuid>, AppError> {
    let row = sqlx::query(
        r#"
        SELECT id FROM monitoring_metric_set_assignment
        WHERE metric_set_id = $1
          AND scope_type = $2
          AND scope_id IS NOT DISTINCT FROM $3
        LIMIT 1
        "#,
    )
    .bind(metric_set_id)
    .bind(scope_type)
    .bind(scope_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?;

    row.map(|row| {
        row.try_get::<Uuid, _>("id")
            .map_err(classify_sqlx_error_owned)
    })
    .transpose()
}

async fn ensure_metric_set_assignment_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    metric_set_id: Uuid,
    scope_type: &str,
    scope_id: Option<Uuid>,
    weight: i32,
) -> Result<Uuid, AppError> {
    // Re-apply re-enables an existing assignment but does NOT overwrite the
    // operator's hand-tuned weight. The `weight` argument is only used when
    // we have to create a fresh row.
    if let Some(existing) =
        find_metric_set_assignment_id(tx, metric_set_id, scope_type, scope_id).await?
    {
        sqlx::query(
            r#"
            UPDATE monitoring_metric_set_assignment
            SET enabled = true, updated_at = now()
            WHERE id = $1
            "#,
        )
        .bind(existing)
        .execute(&mut **tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
        return Ok(existing);
    }

    let row = sqlx::query(
        r#"
        INSERT INTO monitoring_metric_set_assignment
            (metric_set_id, scope_type, scope_id, weight, enabled)
        VALUES ($1, $2, $3, $4, true)
        RETURNING id
        "#,
    )
    .bind(metric_set_id)
    .bind(scope_type)
    .bind(scope_id)
    .bind(weight)
    .fetch_one(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?;

    row.try_get::<Uuid, _>("id")
        .map_err(classify_sqlx_error_owned)
}

/// Shared with `run_quick_start_tx`: ensure an enabled
/// `monitoring_query_source_assignment` exists for `(query_source_id, scope)`.
/// Used by the apply path so picking a "data source" actually binds the
/// preset's metric queries to that source at the same scope.
async fn ensure_query_source_assignment_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    query_source_id: Uuid,
    scope_type: &str,
    scope_id: Option<Uuid>,
) -> Result<Uuid, AppError> {
    let existing = sqlx::query(
        r#"
        SELECT id FROM monitoring_query_source_assignment
        WHERE query_source_id = $1
          AND scope_type = $2
          AND scope_id IS NOT DISTINCT FROM $3
        LIMIT 1
        "#,
    )
    .bind(query_source_id)
    .bind(scope_type)
    .bind(scope_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?
    .map(|row| {
        row.try_get::<Uuid, _>("id")
            .map_err(classify_sqlx_error_owned)
    })
    .transpose()?;

    if let Some(existing) = existing {
        sqlx::query(
            r#"
            UPDATE monitoring_query_source_assignment
            SET enabled = true, updated_at = now()
            WHERE id = $1
            "#,
        )
        .bind(existing)
        .execute(&mut **tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
        return Ok(existing);
    }

    let row = sqlx::query(
        r#"
        INSERT INTO monitoring_query_source_assignment
            (query_source_id, scope_type, scope_id, weight, enabled, options)
        VALUES ($1, $2, $3, 0, true, '{}'::jsonb)
        RETURNING id
        "#,
    )
    .bind(query_source_id)
    .bind(scope_type)
    .bind(scope_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?;

    row.try_get::<Uuid, _>("id")
        .map_err(classify_sqlx_error_owned)
}

async fn ensure_query_source_exists_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    query_source_id: Uuid,
) -> Result<(), AppError> {
    let exists = sqlx::query(
        r#"
        SELECT 1 FROM monitoring_query_source WHERE id = $1 AND enabled = true
        "#,
    )
    .bind(query_source_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?
    .is_some();

    if !exists {
        return Err(AppError::NotFound(format!(
            "monitoring query source `{query_source_id}` not found or disabled"
        )));
    }
    Ok(())
}

/// Ensure the preset's single-item profile and its assignment exist at the
/// given scope. Re-apply must not create a second profile/item/assignment and
/// must not change an existing row: a disabled or operator-tuned profile item
/// or assignment keeps its state.
async fn ensure_single_item_profile_assignment_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    exporter_id: Uuid,
    exporter_name: &str,
    profile_slug: &str,
    scope_type: &str,
    scope_id: Option<Uuid>,
) -> Result<Uuid, AppError> {
    // Serialize preset-created profile/assignment checks for this slug and
    // scope. Manual profile assignments may legitimately coexist at the same
    // scope with a different target/job, so the scope key alone is not enough.
    sqlx::query(
        r#"
        SELECT pg_advisory_xact_lock(
            hashtextextended(
                'monitoring_profile.preset:'
                    || $1 || ':' || $2 || ':' || COALESCE($3::text, ''),
                0
            )
        )
        "#,
    )
    .bind(profile_slug)
    .bind(scope_type)
    .bind(scope_id)
    .execute(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?;

    // Migrated assignments have operator-derived profile slugs, not the preset
    // slug. Reuse their single-exporter profile without re-enabling or retuning it.
    let existing_assignment = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT a.id
        FROM monitoring_profile_assignment a
        JOIN monitoring_profile p ON p.id = a.profile_id
        JOIN monitoring_profile_item i ON i.profile_id = p.id
        WHERE a.scope_type = $1
          AND a.scope_id IS NOT DISTINCT FROM $2
          AND i.exporter_id = $3
          AND (SELECT count(*) FROM monitoring_profile_item all_items
               WHERE all_items.profile_id = p.id) = 1
        ORDER BY a.created_at, a.id
        LIMIT 1
        "#,
    )
    .bind(scope_type)
    .bind(scope_id)
    .bind(exporter_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?;
    if let Some(id) = existing_assignment {
        return Ok(id);
    }

    let profile_id = match sqlx::query(
        r#"
        SELECT id FROM monitoring_profile WHERE slug = $1
        "#,
    )
    .bind(profile_slug)
    .fetch_optional(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?
    {
        Some(row) => row
            .try_get::<Uuid, _>("id")
            .map_err(classify_sqlx_error_owned)?,
        None => {
            let row = sqlx::query(
                r#"
                INSERT INTO monitoring_profile (name, slug, enabled)
                VALUES ($1, $2, true)
                RETURNING id
                "#,
            )
            .bind(exporter_name)
            .bind(profile_slug)
            .fetch_one(&mut **tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
            row.try_get::<Uuid, _>("id")
                .map_err(classify_sqlx_error_owned)?
        }
    };

    sqlx::query(
        r#"
        INSERT INTO monitoring_profile_item
            (profile_id, exporter_id, job, scheme, port, metrics_path, labels,
             enabled, sort_order)
        SELECT $1, $2, NULL, NULL, NULL, NULL, '{}'::jsonb, true, 0
        WHERE NOT EXISTS (
            SELECT 1
            FROM monitoring_profile_item
            WHERE profile_id = $1 AND exporter_id = $2
        )
        "#,
    )
    .bind(profile_id)
    .bind(exporter_id)
    .execute(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?;

    let existing = sqlx::query(
        r#"
        SELECT id FROM monitoring_profile_assignment
        WHERE profile_id = $1
          AND scope_type = $2
          AND scope_id IS NOT DISTINCT FROM $3
        ORDER BY created_at, id
        LIMIT 1
        "#,
    )
    .bind(profile_id)
    .bind(scope_type)
    .bind(scope_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?;

    if let Some(row) = existing {
        validate_profile_scrape_conflicts_tx(tx, profile_id).await?;
        return row
            .try_get::<Uuid, _>("id")
            .map_err(classify_sqlx_error_owned);
    }

    let row = sqlx::query(
        r#"
        INSERT INTO monitoring_profile_assignment
            (profile_id, scope_type, scope_id, weight, enabled, target)
        VALUES ($1, $2, $3, 0, true, NULL)
        RETURNING id
        "#,
    )
    .bind(profile_id)
    .bind(scope_type)
    .bind(scope_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?;

    validate_profile_scrape_conflicts_tx(tx, profile_id).await?;
    row.try_get::<Uuid, _>("id")
        .map_err(classify_sqlx_error_owned)
}

/// Idempotent preset apply, scoped to an open transaction so quick-start can
/// run it after `run_quick_start_tx` without a second commit boundary.
///
/// Re-apply contract (ensure-exists, never-overwrite):
/// - existing `monitoring_metric_query` rows for `(exporter_id, slug)` keep
///   their stored body — we only insert the row when it's missing;
/// - existing `monitoring_metric_set` keeps its name/description/enabled flag;
/// - existing `monitoring_metric_set_item` rows keep their per-set overrides
///   (`show_in_list`, `sort_order`, etc.);
/// - existing `monitoring_metric_set_assignment` keeps its `weight`; the
///   `weight` argument only applies to a freshly inserted assignment;
/// - `query_source_id` is required: the caller's chosen data source is also
///   bound to the same scope via `monitoring_query_source_assignment` so the
///   preset's PromQL actually reads from it;
/// - when `create_scrape_assignment` is true, a single-item profile for the
///   preset exporter and its assignment at the same scope are reused as-is;
///   otherwise they are inserted with the exporter defaults.
async fn apply_exporter_preset_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    preset: &ExporterPreset,
    query_source_id: Uuid,
    scope_type: &str,
    scope_id: Option<Uuid>,
    weight: i32,
    create_scrape_assignment: bool,
) -> Result<AppliedPreset, AppError> {
    let exporter_row = sqlx::query(
        r#"
        SELECT id, name FROM monitoring_exporter WHERE slug = $1
        "#,
    )
    .bind(preset.exporter_slug)
    .fetch_optional(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?;
    let (exporter_id, exporter_name) = match exporter_row {
        Some(row) => (
            row.try_get::<Uuid, _>("id")
                .map_err(classify_sqlx_error_owned)?,
            row.try_get::<String, _>("name")
                .map_err(classify_sqlx_error_owned)?,
        ),
        None => {
            return Err(AppError::NotFound(format!(
                "monitoring exporter `{}` not found; seed `monitoring_exporter` before applying presets",
                preset.exporter_slug
            )));
        }
    };

    ensure_query_source_exists_tx(tx, query_source_id).await?;

    let mut metric_query_ids = Vec::with_capacity(preset.queries.len());
    for spec in preset.queries {
        let id = match find_metric_query_id(tx, exporter_id, spec.slug).await? {
            Some(existing) => {
                update_existing_preset_metric_query_semantic(tx, existing, spec).await?;
                existing
            }
            None => insert_preset_metric_query(tx, exporter_id, spec).await?,
        };
        metric_query_ids.push(id);
    }

    let metric_set_id = match find_metric_set_id_by_slug(tx, preset.set_slug).await? {
        Some(existing) => existing,
        None => {
            insert_metric_set_tx(tx, preset.name, preset.set_slug, Some(preset.description)).await?
        }
    };

    for (spec, &query_id) in preset.queries.iter().zip(metric_query_ids.iter()) {
        ensure_metric_set_item_tx(tx, metric_set_id, query_id, spec).await?;
    }

    let assignment_id =
        ensure_metric_set_assignment_tx(tx, metric_set_id, scope_type, scope_id, weight).await?;

    let query_source_assignment_id =
        ensure_query_source_assignment_tx(tx, query_source_id, scope_type, scope_id).await?;

    let scrape_assignment_id = if create_scrape_assignment {
        Some(
            ensure_single_item_profile_assignment_tx(
                tx,
                exporter_id,
                &exporter_name,
                preset.set_slug,
                scope_type,
                scope_id,
            )
            .await?,
        )
    } else {
        None
    };

    Ok(AppliedPreset {
        exporter_id,
        metric_set_id,
        metric_set_assignment_id: assignment_id,
        query_source_assignment_id,
        metric_query_ids,
        scrape_assignment_id,
    })
}

/// Top-level entry point for `POST /api/v1/monitoring/exporters/{slug}/apply`.
/// One pool-level transaction; idempotent on re-apply per the contract above.
pub(super) async fn apply_exporter_preset(
    pool: &PgPool,
    preset: &ExporterPreset,
    query_source_id: Uuid,
    scope_type: &str,
    scope_id: Option<Uuid>,
    weight: i32,
    create_scrape_assignment: bool,
) -> Result<AppliedPreset, AppError> {
    let mut tx = pool.begin().await.map_err(classify_sqlx_error_owned)?;
    lock_profile_scrape_mutations_tx(&mut tx).await?;
    let applied = apply_exporter_preset_tx(
        &mut tx,
        preset,
        query_source_id,
        scope_type,
        scope_id,
        weight,
        create_scrape_assignment,
    )
    .await?;
    tx.commit().await.map_err(classify_sqlx_error_owned)?;
    Ok(applied)
}

/// Resolved ids returned by [`run_quick_start`].
#[derive(Debug, Clone)]
pub(super) struct QuickStartResult {
    pub query_source_id: Uuid,
    pub query_source_assignment_id: Uuid,
}

/// Request-start authorization verdicts for the two branches of quick-start's
/// deterministic query-source upsert. The transaction enforces the applicable
/// verdict only after it serializes and inspects the slug.
#[derive(Debug, Clone, Copy)]
pub(super) struct QuickStartAuthorization {
    pub can_create: bool,
    pub can_update: bool,
}

/// Cohesive query-source identity and scope for a quick-start upsert, shared
/// by `run_quick_start` and its transaction body.
#[derive(Clone, Copy)]
pub(super) struct QuickStartSpec<'a> {
    pub(super) name: &'a str,
    pub(super) slug: &'a str,
    pub(super) source_type: &'a str,
    pub(super) query_url: &'a str,
    pub(super) credential_secret_id: Option<Uuid>,
    pub(super) scope_type: &'a str,
    pub(super) scope_id: Option<Uuid>,
}

/// [`run_quick_start`]'s transaction body. It takes the same cohesive
/// [`QuickStartSpec`] as its public sibling.
async fn run_quick_start_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    authorization: QuickStartAuthorization,
    spec: QuickStartSpec<'_>,
) -> Result<QuickStartResult, AppError> {
    let QuickStartSpec {
        name,
        slug,
        source_type,
        query_url,
        credential_secret_id,
        scope_type,
        scope_id,
    } = spec;
    // PostgreSQL does not lock a missing row with `FOR UPDATE`. Serialize every
    // request for this deterministic slug before deciding whether quick-start
    // will insert or update, so a concurrent creator cannot turn a Create-only
    // request into an unauthorized update.
    sqlx::query(
        "SELECT pg_advisory_xact_lock(hashtextextended('monitoring_query_source.quick_start:' || $1, 0))",
    )
    .bind(slug)
    .execute(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?;

    let existing_source = sqlx::query(
        r#"
        SELECT id, source_type FROM monitoring_query_source
        WHERE slug = $1
        FOR UPDATE
        "#,
    )
    .bind(slug)
    .fetch_optional(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?;

    let query_source_id = if let Some(row) = existing_source {
        if !authorization.can_update {
            return Err(AppError::Forbidden("Access denied".to_string()));
        }
        let existing_id = row
            .try_get::<Uuid, _>("id")
            .map_err(classify_sqlx_error_owned)?;
        let existing_source_type = row
            .try_get::<String, _>("source_type")
            .map_err(classify_sqlx_error_owned)?;
        // Reject silent `source_type` rewrites: a same-slug quick-start that
        // flipped `prometheus -> zabbix` would orphan any preset queries
        // already attached at the previous type.
        if existing_source_type != source_type {
            return Err(AppError::Validation(format!(
                "monitoring query source `{slug}` already exists with source_type `{existing_source_type}`; refusing to rewrite to `{source_type}`"
            )));
        }
        sqlx::query(
            r#"
            UPDATE monitoring_query_source
            SET name = $2, query_url = $3, enabled = true, updated_at = now()
            WHERE id = $1
            "#,
        )
        .bind(existing_id)
        .bind(name)
        .bind(query_url)
        .execute(&mut **tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
        existing_id
    } else {
        if !authorization.can_create {
            return Err(AppError::Forbidden("Access denied".to_string()));
        }
        let row = sqlx::query(
            r#"
            INSERT INTO monitoring_query_source
                (name, slug, source_type, query_url, timeout_ms, options, enabled)
            VALUES ($1, $2, $3, $4, 3000, '{}'::jsonb, true)
            RETURNING id
            "#,
        )
        .bind(name)
        .bind(slug)
        .bind(source_type)
        .bind(query_url)
        .fetch_one(&mut **tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
        row.try_get::<Uuid, _>("id")
            .map_err(classify_sqlx_error_owned)?
    };

    reconcile_query_source_credential_binding_tx(tx, query_source_id, credential_secret_id).await?;

    let assignment_id =
        ensure_query_source_assignment_tx(tx, query_source_id, scope_type, scope_id).await?;

    Ok(QuickStartResult {
        query_source_id,
        query_source_assignment_id: assignment_id,
    })
}

/// Atomically create or update a deterministic query source, assign it to the
/// scope, and optionally apply a preset. The transaction serializes by slug,
/// then applies the branch-specific authorization before it mutates state.
/// Every source, assignment, and optional-preset change rolls back together.
pub(super) async fn run_quick_start(
    pool: &PgPool,
    authorization: QuickStartAuthorization,
    spec: QuickStartSpec<'_>,
    preset: Option<&ExporterPreset>,
    create_scrape_assignment: bool,
) -> Result<(QuickStartResult, Option<AppliedPreset>), AppError> {
    let QuickStartSpec {
        scope_type,
        scope_id,
        ..
    } = spec;
    let mut tx = pool.begin().await.map_err(classify_sqlx_error_owned)?;
    lock_profile_scrape_mutations_tx(&mut tx).await?;
    let result = run_quick_start_tx(&mut tx, authorization, spec).await?;
    let applied = if let Some(preset) = preset {
        Some(
            apply_exporter_preset_tx(
                &mut tx,
                preset,
                result.query_source_id,
                scope_type,
                scope_id,
                0,
                create_scrape_assignment,
            )
            .await?,
        )
    } else {
        None
    };
    tx.commit().await.map_err(classify_sqlx_error_owned)?;
    Ok((result, applied))
}

// ===========================================================================
// monitoring_profile, monitoring_profile_item, monitoring_profile_assignment
// ===========================================================================

// Profile item edits and assignments of different profiles can change the same
// job/target pair. Lock the scrape domain before taking any per-profile/source
// locks, and check the resulting rows inside the writing transaction.
async fn lock_profile_scrape_mutations_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
) -> Result<(), AppError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('monitoring_profile.scrapes', 0))")
        .execute(&mut **tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
    Ok(())
}

async fn validate_profile_scrape_conflicts_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    profile_id: Uuid,
) -> Result<(), AppError> {
    let rows = sqlx::query(
        r#"
        SELECT a.profile_id, a.scope_type, a.scope_id, a.target, p.slug,
               COALESCE(i.job, e.default_job) AS job,
               COALESCE(i.port, e.default_port) AS port,
               e.default_target_source, e.labels AS exporter_labels, i.labels AS item_labels
        FROM monitoring_profile_assignment a
        JOIN monitoring_profile p ON p.id = a.profile_id
        JOIN monitoring_profile_item i ON i.profile_id = p.id
        JOIN monitoring_exporter e ON e.id = i.exporter_id
        WHERE a.enabled AND p.enabled AND i.enabled AND e.enabled
          AND EXISTS (
              SELECT 1 FROM monitoring_profile_assignment changed
              WHERE changed.profile_id = $1
                AND changed.scope_type = a.scope_type
                AND changed.scope_id IS NOT DISTINCT FROM a.scope_id
          )
        "#,
    )
    .bind(profile_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?;

    let mut owners = HashMap::new();
    for row in rows {
        let owner: Uuid = row
            .try_get("profile_id")
            .map_err(classify_sqlx_error_owned)?;
        let slug: String = row.try_get("slug").map_err(classify_sqlx_error_owned)?;
        let scope: String = row
            .try_get("scope_type")
            .map_err(classify_sqlx_error_owned)?;
        let scope_id: Option<Uuid> = row.try_get("scope_id").map_err(classify_sqlx_error_owned)?;
        let mut labels =
            crate::contexts::monitoring::infrastructure::persistence::labels_from_value(
                row.try_get("exporter_labels")
                    .map_err(classify_sqlx_error_owned)?,
            );
        labels.extend(
            crate::contexts::monitoring::infrastructure::persistence::labels_from_value(
                row.try_get("item_labels")
                    .map_err(classify_sqlx_error_owned)?,
            ),
        );
        let job = labels.get("job").cloned().unwrap_or(
            row.try_get::<String, _>("job")
                .map_err(classify_sqlx_error_owned)?,
        );
        let target: Option<String> = row.try_get("target").map_err(classify_sqlx_error_owned)?;
        let port: Option<i32> = row.try_get("port").map_err(classify_sqlx_error_owned)?;
        let port = port.and_then(|value| u16::try_from(value).ok());
        let source: String = row
            .try_get("default_target_source")
            .map_err(classify_sqlx_error_owned)?;
        let target = target
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let (endpoint, implicit_port, implicit_source) = if let Some(target) = target {
            (
                Some(
                    port.map(|port| super::helpers::append_port(target, port))
                        .unwrap_or_else(|| target.to_string()),
                ),
                None,
                String::new(),
            )
        } else {
            // Without a host, only identical implicit sources can be compared.
            // The host endpoint reports overlaps after canonical SD resolution.
            (None, port, source)
        };
        let key = (
            scope,
            scope_id,
            job.clone(),
            endpoint.clone(),
            implicit_port,
            implicit_source,
        );
        if let Some((previous_owner, previous_slug)) = owners.get(&key) {
            if owner == profile_id || *previous_owner == profile_id {
                let target = endpoint.as_deref().unwrap_or("*");
                return Err(AppError::Conflict(format!(
                    "job `{job}` and target `{target}` are already assigned by profile `{previous_slug}`"
                )));
            }
        } else {
            owners.insert(key, (owner, slug));
        }
    }
    Ok(())
}

pub(crate) const PROFILE_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "p.name", "observability.profile.columns.name"),
    ListColumn::sortable("slug", "p.slug", "observability.profile.columns.slug"),
    ListColumn::enum_filter(
        "enabled",
        "p.enabled",
        "p.enabled::text",
        "observability.columns.enabled",
    ),
    ListColumn::sortable(
        "created_at",
        "p.created_at",
        "observability.columns.createdAt",
    ),
    ListColumn::sortable(
        "updated_at",
        "p.updated_at",
        "observability.columns.updatedAt",
    ),
];

pub(super) const PROFILE_ASSIGNMENT_COLUMNS: &[ListColumn] = &[
    ListColumn::fk_filter(
        "profile_id",
        "a.profile_id",
        "profile_id",
        "a.profile_id",
        "monitoring_profile",
        "observability.profile.columns.name",
    ),
    ListColumn::enum_filter(
        "scope_type",
        "a.scope_type",
        "a.scope_type",
        "observability.columns.scopeType",
    ),
    ListColumn::fk_filter(
        "scope_id",
        "a.scope_id",
        "scope_id",
        "a.scope_id",
        "connection_target",
        "observability.columns.scope",
    ),
    ListColumn::enum_filter(
        "enabled",
        "a.enabled",
        "a.enabled::text",
        "observability.columns.enabled",
    ),
    ListColumn::sortable("weight", "a.weight", "observability.columns.weight"),
    ListColumn::sortable(
        "created_at",
        "a.created_at",
        "observability.columns.createdAt",
    ),
    ListColumn::sortable(
        "updated_at",
        "a.updated_at",
        "observability.columns.updatedAt",
    ),
];

#[derive(Debug, Clone)]
pub(super) struct ProfileRecord {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
    pub metric_set_id: Option<Uuid>,
    pub query_source_id: Option<Uuid>,
    pub alert_source_id: Option<Uuid>,
    pub enabled: bool,
    pub item_count: i64,
    pub assignment_count: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub(super) struct ProfileItemRecord {
    pub id: Uuid,
    pub profile_id: Uuid,
    pub exporter_id: Uuid,
    pub exporter_slug: String,
    pub exporter_name: String,
    pub exporter_enabled: bool,
    pub exporter_labels: Value,
    pub default_job: String,
    pub default_scheme: String,
    pub default_port: Option<i32>,
    pub default_metrics_path: String,
    pub default_target_source: String,
    pub job: Option<String>,
    pub scheme: Option<String>,
    pub port: Option<i32>,
    pub metrics_path: Option<String>,
    pub labels: Value,
    pub enabled: bool,
    pub sort_order: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub(super) struct ProfileInput {
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
    pub metric_set_id: Option<Uuid>,
    pub query_source_id: Option<Uuid>,
    pub alert_source_id: Option<Uuid>,
    pub enabled: bool,
    pub items: Vec<ProfileItemInput>,
}

#[derive(Debug, Clone)]
pub(super) struct ProfileItemInput {
    pub exporter_id: Uuid,
    pub job: Option<String>,
    pub scheme: Option<String>,
    pub port: Option<i32>,
    pub metrics_path: Option<String>,
    pub labels: Value,
    pub enabled: bool,
    pub sort_order: i32,
}

#[derive(Debug, Clone)]
pub(super) struct ProfileAssignmentRecord {
    pub id: Uuid,
    pub profile_id: Uuid,
    pub profile_slug: String,
    pub profile_name: String,
    pub scope_type: String,
    pub scope_id: Option<Uuid>,
    pub weight: i32,
    pub enabled: bool,
    pub target: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub(super) struct ProfileAssignmentInput {
    pub profile_id: Uuid,
    pub scope_type: String,
    pub scope_id: Option<Uuid>,
    pub weight: i32,
    pub enabled: bool,
    pub target: Option<String>,
}

/// One exporter/job pair of an enabled profile item at a scope. Used for the
/// effective-job conflict check, which needs every assignment at the scope.
#[derive(Debug, Clone)]
pub(super) struct ScopeProfileJob {
    pub assignment_id: Uuid,
    pub profile_id: Uuid,
    pub profile_slug: String,
    pub profile_name: String,
    pub target: Option<String>,
    pub job: String,
}

#[derive(Debug, Clone)]
pub(super) struct ExporterReference {
    pub id: Uuid,
    pub slug: String,
    pub enabled: bool,
    pub default_job: String,
}

const PROFILE_SELECT: &str = r#"
    SELECT p.id, p.name, p.slug, p.description, p.metric_set_id,
           p.query_source_id, p.alert_source_id, p.enabled,
           (SELECT count(*) FROM monitoring_profile_item i
             WHERE i.profile_id = p.id) AS item_count,
           (SELECT count(*) FROM monitoring_profile_assignment a
             WHERE a.profile_id = p.id) AS assignment_count,
           p.created_at, p.updated_at
    FROM monitoring_profile p
"#;

const PROFILE_ITEM_SELECT: &str = r#"
    SELECT i.id, i.profile_id, i.exporter_id,
           e.slug AS exporter_slug, e.name AS exporter_name,
           e.enabled AS exporter_enabled, e.labels AS exporter_labels,
           e.default_job, e.default_scheme, e.default_port,
           e.default_metrics_path, e.default_target_source,
           i.job, i.scheme, i.port, i.metrics_path, i.labels, i.enabled,
           i.sort_order, i.created_at, i.updated_at
    FROM monitoring_profile_item i
    JOIN monitoring_exporter e ON e.id = i.exporter_id
"#;

const PROFILE_ASSIGNMENT_SELECT: &str = r#"
    SELECT a.id, a.profile_id, p.slug AS profile_slug, p.name AS profile_name,
           a.scope_type, a.scope_id, a.weight, a.enabled, a.target,
           a.created_at, a.updated_at
    FROM monitoring_profile_assignment a
    JOIN monitoring_profile p ON p.id = a.profile_id
"#;

fn map_profile_row(row: &sqlx::postgres::PgRow) -> Result<ProfileRecord, AppError> {
    Ok(ProfileRecord {
        id: row.try_get("id").map_err(classify_sqlx_error_owned)?,
        name: row.try_get("name").map_err(classify_sqlx_error_owned)?,
        slug: row.try_get("slug").map_err(classify_sqlx_error_owned)?,
        description: row.try_get("description").ok(),
        metric_set_id: row.try_get("metric_set_id").ok(),
        query_source_id: row.try_get("query_source_id").ok(),
        alert_source_id: row.try_get("alert_source_id").ok(),
        enabled: row.try_get("enabled").map_err(classify_sqlx_error_owned)?,
        item_count: row
            .try_get("item_count")
            .map_err(classify_sqlx_error_owned)?,
        assignment_count: row
            .try_get("assignment_count")
            .map_err(classify_sqlx_error_owned)?,
        created_at: row
            .try_get("created_at")
            .map_err(classify_sqlx_error_owned)?,
        updated_at: row
            .try_get("updated_at")
            .map_err(classify_sqlx_error_owned)?,
    })
}

fn map_profile_item_row(row: &sqlx::postgres::PgRow) -> Result<ProfileItemRecord, AppError> {
    Ok(ProfileItemRecord {
        id: row.try_get("id").map_err(classify_sqlx_error_owned)?,
        profile_id: row
            .try_get("profile_id")
            .map_err(classify_sqlx_error_owned)?,
        exporter_id: row
            .try_get("exporter_id")
            .map_err(classify_sqlx_error_owned)?,
        exporter_slug: row
            .try_get("exporter_slug")
            .map_err(classify_sqlx_error_owned)?,
        exporter_name: row
            .try_get("exporter_name")
            .map_err(classify_sqlx_error_owned)?,
        exporter_enabled: row
            .try_get("exporter_enabled")
            .map_err(classify_sqlx_error_owned)?,
        exporter_labels: row
            .try_get("exporter_labels")
            .unwrap_or_else(|_| Value::Object(serde_json::Map::new())),
        default_job: row
            .try_get("default_job")
            .map_err(classify_sqlx_error_owned)?,
        default_scheme: row
            .try_get("default_scheme")
            .map_err(classify_sqlx_error_owned)?,
        default_port: row.try_get("default_port").ok(),
        default_metrics_path: row
            .try_get("default_metrics_path")
            .map_err(classify_sqlx_error_owned)?,
        default_target_source: row
            .try_get("default_target_source")
            .map_err(classify_sqlx_error_owned)?,
        job: row.try_get("job").ok(),
        scheme: row.try_get("scheme").ok(),
        port: row.try_get("port").ok(),
        metrics_path: row.try_get("metrics_path").ok(),
        labels: row
            .try_get("labels")
            .unwrap_or_else(|_| Value::Object(serde_json::Map::new())),
        enabled: row.try_get("enabled").map_err(classify_sqlx_error_owned)?,
        sort_order: row
            .try_get("sort_order")
            .map_err(classify_sqlx_error_owned)?,
        created_at: row
            .try_get("created_at")
            .map_err(classify_sqlx_error_owned)?,
        updated_at: row
            .try_get("updated_at")
            .map_err(classify_sqlx_error_owned)?,
    })
}

fn map_profile_assignment_row(
    row: &sqlx::postgres::PgRow,
) -> Result<ProfileAssignmentRecord, AppError> {
    Ok(ProfileAssignmentRecord {
        id: row.try_get("id").map_err(classify_sqlx_error_owned)?,
        profile_id: row
            .try_get("profile_id")
            .map_err(classify_sqlx_error_owned)?,
        profile_slug: row
            .try_get("profile_slug")
            .map_err(classify_sqlx_error_owned)?,
        profile_name: row
            .try_get("profile_name")
            .map_err(classify_sqlx_error_owned)?,
        scope_type: row
            .try_get("scope_type")
            .map_err(classify_sqlx_error_owned)?,
        scope_id: row.try_get("scope_id").ok(),
        weight: row.try_get("weight").map_err(classify_sqlx_error_owned)?,
        enabled: row.try_get("enabled").map_err(classify_sqlx_error_owned)?,
        target: row.try_get("target").ok(),
        created_at: row
            .try_get("created_at")
            .map_err(classify_sqlx_error_owned)?,
        updated_at: row
            .try_get("updated_at")
            .map_err(classify_sqlx_error_owned)?,
    })
}

/// List profiles, optionally narrowed by a case-insensitive name/slug search.
pub(super) async fn list_profiles(
    pool: &PgPool,
    filter: &MonitoringListFilter,
    search: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<(Vec<ProfileRecord>, i64), AppError> {
    let mut state = WhereState::new();
    let mut count_builder = QueryBuilder::new("SELECT count(*) FROM monitoring_profile p");
    push_list_filters_with_state(&mut count_builder, PROFILE_COLUMNS, filter, &mut state);
    if let Some(search) = search {
        let pattern = format!("%{search}%");
        state.and(&mut count_builder);
        count_builder
            .push("(p.name ILIKE ")
            .push_bind(pattern.clone())
            .push(" OR p.slug ILIKE ")
            .push_bind(pattern)
            .push(")");
    }
    let total: i64 = count_builder
        .build_query_scalar()
        .fetch_one(pool)
        .await
        .map_err(classify_sqlx_error_owned)?;

    let mut builder = QueryBuilder::new(PROFILE_SELECT);
    let mut state = WhereState::new();
    push_list_filters_with_state(&mut builder, PROFILE_COLUMNS, filter, &mut state);
    if let Some(search) = search {
        let pattern = format!("%{search}%");
        state.and(&mut builder);
        builder
            .push("(p.name ILIKE ")
            .push_bind(pattern.clone())
            .push(" OR p.slug ILIKE ")
            .push_bind(pattern)
            .push(")");
    }
    push_order_by(
        &mut builder,
        PROFILE_COLUMNS,
        filter.sort.as_ref(),
        "p.name",
    );
    builder.push(" LIMIT ").push_bind(limit);
    builder.push(" OFFSET ").push_bind(offset);

    let rows = builder
        .build()
        .fetch_all(pool)
        .await
        .map_err(classify_sqlx_error_owned)?;
    let records = rows
        .iter()
        .map(map_profile_row)
        .collect::<Result<Vec<_>, _>>()?;
    Ok((records, total))
}

pub(super) async fn fetch_profile_by_id(
    pool: &PgPool,
    id: Uuid,
) -> Result<Option<ProfileRecord>, AppError> {
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "{PROFILE_SELECT} WHERE p.id = $1"
    )))
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(classify_sqlx_error_owned)?;
    row.as_ref().map(map_profile_row).transpose()
}

/// List items for a set of profiles, ordered by the profile's item order.
pub(super) async fn list_profile_items(
    pool: &PgPool,
    profile_ids: &[Uuid],
) -> Result<Vec<ProfileItemRecord>, AppError> {
    if profile_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "{PROFILE_ITEM_SELECT} WHERE i.profile_id = ANY($1) ORDER BY i.profile_id, i.sort_order, i.id"
    )))
    .bind(profile_ids)
    .fetch_all(pool)
    .await
    .map_err(classify_sqlx_error_owned)?;
    rows.iter().map(map_profile_item_row).collect()
}

async fn insert_profile_items_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    profile_id: Uuid,
    items: &[ProfileItemInput],
) -> Result<(), AppError> {
    for item in items {
        sqlx::query(
            r#"
            INSERT INTO monitoring_profile_item
                (profile_id, exporter_id, job, scheme, port, metrics_path, labels,
                 enabled, sort_order)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            "#,
        )
        .bind(profile_id)
        .bind(item.exporter_id)
        .bind(item.job.as_deref())
        .bind(item.scheme.as_deref())
        .bind(item.port)
        .bind(item.metrics_path.as_deref())
        .bind(&item.labels)
        .bind(item.enabled)
        .bind(item.sort_order)
        .execute(&mut **tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
    }
    Ok(())
}

pub(super) async fn create_profile(
    pool: &PgPool,
    input: &ProfileInput,
) -> Result<ProfileRecord, AppError> {
    let mut tx = pool.begin().await.map_err(classify_sqlx_error_owned)?;
    let row = sqlx::query(
        r#"
        INSERT INTO monitoring_profile
            (name, slug, description, metric_set_id, query_source_id,
             alert_source_id, enabled)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        RETURNING id
        "#,
    )
    .bind(&input.name)
    .bind(&input.slug)
    .bind(input.description.as_deref())
    .bind(input.metric_set_id)
    .bind(input.query_source_id)
    .bind(input.alert_source_id)
    .bind(input.enabled)
    .fetch_one(&mut *tx)
    .await
    .map_err(classify_sqlx_error_owned)?;
    let profile_id: Uuid = row.try_get("id").map_err(classify_sqlx_error_owned)?;
    insert_profile_items_tx(&mut tx, profile_id, &input.items).await?;
    tx.commit().await.map_err(classify_sqlx_error_owned)?;

    fetch_profile_by_id(pool, profile_id)
        .await?
        .ok_or_else(|| AppError::Internal("Created profile could not be reloaded".to_string()))
}

/// Full replacement: every item is replaced by `input.items`.
pub(super) async fn update_profile(
    pool: &PgPool,
    id: Uuid,
    input: &ProfileInput,
) -> Result<Option<ProfileRecord>, AppError> {
    let mut tx = pool.begin().await.map_err(classify_sqlx_error_owned)?;
    lock_profile_scrape_mutations_tx(&mut tx).await?;
    let row = sqlx::query(
        r#"
        UPDATE monitoring_profile
        SET name = $2,
            slug = $3,
            description = $4,
            metric_set_id = $5,
            query_source_id = $6,
            alert_source_id = $7,
            enabled = $8,
            updated_at = now()
        WHERE id = $1
        RETURNING id
        "#,
    )
    .bind(id)
    .bind(&input.name)
    .bind(&input.slug)
    .bind(input.description.as_deref())
    .bind(input.metric_set_id)
    .bind(input.query_source_id)
    .bind(input.alert_source_id)
    .bind(input.enabled)
    .fetch_optional(&mut *tx)
    .await
    .map_err(classify_sqlx_error_owned)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let profile_id: Uuid = row.try_get("id").map_err(classify_sqlx_error_owned)?;

    sqlx::query("DELETE FROM monitoring_profile_item WHERE profile_id = $1")
        .bind(profile_id)
        .execute(&mut *tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
    insert_profile_items_tx(&mut tx, profile_id, &input.items).await?;
    validate_profile_scrape_conflicts_tx(&mut tx, profile_id).await?;
    tx.commit().await.map_err(classify_sqlx_error_owned)?;

    fetch_profile_by_id(pool, profile_id).await
}

/// Assignment scopes that block deleting a profile, formatted for the 409
/// message (`global`, `site:<uuid>`, ...).
pub(super) async fn profile_assignment_scopes(
    pool: &PgPool,
    profile_id: Uuid,
) -> Result<Vec<String>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT scope_type, scope_id
        FROM monitoring_profile_assignment
        WHERE profile_id = $1
        ORDER BY created_at, id
        "#,
    )
    .bind(profile_id)
    .fetch_all(pool)
    .await
    .map_err(classify_sqlx_error_owned)?;

    rows.iter()
        .map(|row| {
            let scope_type: String = row
                .try_get("scope_type")
                .map_err(classify_sqlx_error_owned)?;
            match row.try_get::<Option<Uuid>, _>("scope_id") {
                Ok(Some(scope_id)) => Ok(format!("{scope_type}:{scope_id}")),
                Ok(None) => Ok(scope_type),
                Err(error) => Err(classify_sqlx_error_owned(error)),
            }
        })
        .collect()
}

/// Delete a profile and its items. Assignments are refused by the caller before
/// this runs; the database keeps `ON DELETE RESTRICT` as the final guard.
pub(super) async fn delete_profile(pool: &PgPool, id: Uuid) -> Result<bool, AppError> {
    let result = sqlx::query("DELETE FROM monitoring_profile WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .map_err(classify_sqlx_error_owned)?;
    Ok(result.rows_affected() > 0)
}

/// List profile assignments with the profile identity attached.
pub(super) async fn list_profile_assignments(
    pool: &PgPool,
    filter: &MonitoringListFilter,
    limit: i64,
    offset: i64,
) -> Result<(Vec<ProfileAssignmentRecord>, i64), AppError> {
    let mut state = WhereState::new();
    let mut count_builder = QueryBuilder::new(
        "SELECT count(*) FROM monitoring_profile_assignment a JOIN monitoring_profile p ON p.id = a.profile_id",
    );
    push_list_filters_with_state(
        &mut count_builder,
        PROFILE_ASSIGNMENT_COLUMNS,
        filter,
        &mut state,
    );
    let total: i64 = count_builder
        .build_query_scalar()
        .fetch_one(pool)
        .await
        .map_err(classify_sqlx_error_owned)?;

    let mut builder = QueryBuilder::new(PROFILE_ASSIGNMENT_SELECT);
    let mut state = WhereState::new();
    push_list_filters_with_state(&mut builder, PROFILE_ASSIGNMENT_COLUMNS, filter, &mut state);
    push_order_by(
        &mut builder,
        PROFILE_ASSIGNMENT_COLUMNS,
        filter.sort.as_ref(),
        "a.created_at",
    );
    builder.push(" LIMIT ").push_bind(limit);
    builder.push(" OFFSET ").push_bind(offset);

    let rows = builder
        .build()
        .fetch_all(pool)
        .await
        .map_err(classify_sqlx_error_owned)?;
    let records = rows
        .iter()
        .map(map_profile_assignment_row)
        .collect::<Result<Vec<_>, _>>()?;
    Ok((records, total))
}

pub(super) async fn fetch_profile_assignment_by_id(
    pool: &PgPool,
    id: Uuid,
) -> Result<Option<ProfileAssignmentRecord>, AppError> {
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "{PROFILE_ASSIGNMENT_SELECT} WHERE a.id = $1"
    )))
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(classify_sqlx_error_owned)?;
    row.as_ref().map(map_profile_assignment_row).transpose()
}

pub(super) async fn find_profile_assignment(
    pool: &PgPool,
    profile_id: Uuid,
    scope_type: &str,
    scope_id: Option<Uuid>,
) -> Result<Option<ProfileAssignmentRecord>, AppError> {
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "{PROFILE_ASSIGNMENT_SELECT} WHERE a.profile_id = $1 AND a.scope_type = $2 AND a.scope_id IS NOT DISTINCT FROM $3"
    )))
    .bind(profile_id)
    .bind(scope_type)
    .bind(scope_id)
    .fetch_optional(pool)
    .await
    .map_err(classify_sqlx_error_owned)?;
    row.as_ref().map(map_profile_assignment_row).transpose()
}

pub(super) async fn create_profile_assignment(
    pool: &PgPool,
    input: &ProfileAssignmentInput,
) -> Result<ProfileAssignmentRecord, AppError> {
    let mut tx = pool.begin().await.map_err(classify_sqlx_error_owned)?;
    lock_profile_scrape_mutations_tx(&mut tx).await?;
    let row = sqlx::query(
        r#"
        INSERT INTO monitoring_profile_assignment
            (profile_id, scope_type, scope_id, weight, enabled, target)
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING id
        "#,
    )
    .bind(input.profile_id)
    .bind(&input.scope_type)
    .bind(input.scope_id)
    .bind(input.weight)
    .bind(input.enabled)
    .bind(input.target.as_deref())
    .fetch_one(&mut *tx)
    .await
    .map_err(classify_sqlx_error_owned)?;
    let id: Uuid = row.try_get("id").map_err(classify_sqlx_error_owned)?;
    validate_profile_scrape_conflicts_tx(&mut tx, input.profile_id).await?;
    tx.commit().await.map_err(classify_sqlx_error_owned)?;
    fetch_profile_assignment_by_id(pool, id)
        .await?
        .ok_or_else(|| AppError::Internal("Created assignment could not be reloaded".to_string()))
}

pub(super) async fn update_profile_assignment(
    pool: &PgPool,
    id: Uuid,
    input: &ProfileAssignmentInput,
) -> Result<Option<ProfileAssignmentRecord>, AppError> {
    let mut tx = pool.begin().await.map_err(classify_sqlx_error_owned)?;
    lock_profile_scrape_mutations_tx(&mut tx).await?;
    let result = sqlx::query(
        r#"
        UPDATE monitoring_profile_assignment
        SET profile_id = $2,
            scope_type = $3,
            scope_id = $4,
            weight = $5,
            enabled = $6,
            target = $7,
            updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(id)
    .bind(input.profile_id)
    .bind(&input.scope_type)
    .bind(input.scope_id)
    .bind(input.weight)
    .bind(input.enabled)
    .bind(input.target.as_deref())
    .execute(&mut *tx)
    .await
    .map_err(classify_sqlx_error_owned)?;
    if result.rows_affected() == 0 {
        return Ok(None);
    }
    validate_profile_scrape_conflicts_tx(&mut tx, input.profile_id).await?;
    tx.commit().await.map_err(classify_sqlx_error_owned)?;
    fetch_profile_assignment_by_id(pool, id).await
}

pub(super) async fn delete_profile_assignment(pool: &PgPool, id: Uuid) -> Result<bool, AppError> {
    let result = sqlx::query("DELETE FROM monitoring_profile_assignment WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .map_err(classify_sqlx_error_owned)?;
    Ok(result.rows_affected() > 0)
}

/// Enabled profile items that already occupy a scope, flattened to
/// exporter/job pairs for effective-job conflict detection.
pub(super) async fn list_scope_profile_jobs(
    pool: &PgPool,
    scope_type: &str,
    scope_id: Option<Uuid>,
) -> Result<Vec<ScopeProfileJob>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT a.id AS assignment_id,
               a.profile_id,
               p.slug AS profile_slug,
               p.name AS profile_name,
               a.target,
               COALESCE(i.job, e.default_job) AS job
        FROM monitoring_profile_assignment a
        JOIN monitoring_profile p ON p.id = a.profile_id
        JOIN monitoring_profile_item i ON i.profile_id = p.id
        JOIN monitoring_exporter e ON e.id = i.exporter_id
        WHERE a.enabled = true
          AND p.enabled = true
          AND i.enabled = true
          AND e.enabled = true
          AND a.scope_type = $1
          AND a.scope_id IS NOT DISTINCT FROM $2
        ORDER BY a.id, i.sort_order, i.id
        "#,
    )
    .bind(scope_type)
    .bind(scope_id)
    .fetch_all(pool)
    .await
    .map_err(classify_sqlx_error_owned)?;

    rows.iter()
        .map(|row| {
            Ok(ScopeProfileJob {
                assignment_id: row
                    .try_get("assignment_id")
                    .map_err(classify_sqlx_error_owned)?,
                profile_id: row
                    .try_get("profile_id")
                    .map_err(classify_sqlx_error_owned)?,
                profile_slug: row
                    .try_get("profile_slug")
                    .map_err(classify_sqlx_error_owned)?,
                profile_name: row
                    .try_get("profile_name")
                    .map_err(classify_sqlx_error_owned)?,
                target: row.try_get("target").ok(),
                job: row.try_get("job").map_err(classify_sqlx_error_owned)?,
            })
        })
        .collect()
}

/// Fetch exporter catalog fields needed to validate profile items.
pub(super) async fn fetch_exporter_references(
    pool: &PgPool,
    exporter_ids: &[Uuid],
) -> Result<Vec<ExporterReference>, AppError> {
    if exporter_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT id, slug, enabled, default_job
        FROM monitoring_exporter
        WHERE id = ANY($1)
        "#,
    )
    .bind(exporter_ids)
    .fetch_all(pool)
    .await
    .map_err(classify_sqlx_error_owned)?;

    rows.iter()
        .map(|row| {
            Ok(ExporterReference {
                id: row.try_get("id").map_err(classify_sqlx_error_owned)?,
                slug: row.try_get("slug").map_err(classify_sqlx_error_owned)?,
                enabled: row.try_get("enabled").map_err(classify_sqlx_error_owned)?,
                default_job: row
                    .try_get("default_job")
                    .map_err(classify_sqlx_error_owned)?,
            })
        })
        .collect()
}

/// Idempotently ensure the source assignments linked by a profile exist at the
/// same scope. Never overwrites an existing assignment (weight, enabled and
/// options keep the operator's values).
async fn ensure_profile_source_assignments_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    profile: &ProfileRecord,
    scope_type: &str,
    scope_id: Option<Uuid>,
) -> Result<(), AppError> {
    if let Some(metric_set_id) = profile.metric_set_id {
        sqlx::query(
            r#"
            INSERT INTO monitoring_metric_set_assignment
                (metric_set_id, scope_type, scope_id, weight, enabled)
            SELECT $1, $2, $3, 0, true
            WHERE NOT EXISTS (
                SELECT 1 FROM monitoring_metric_set_assignment
                WHERE metric_set_id = $1
                  AND scope_type = $2
                  AND scope_id IS NOT DISTINCT FROM $3
            )
            "#,
        )
        .bind(metric_set_id)
        .bind(scope_type)
        .bind(scope_id)
        .execute(&mut **tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
    }

    if let Some(query_source_id) = profile.query_source_id {
        sqlx::query(
            r#"
            INSERT INTO monitoring_query_source_assignment
                (query_source_id, scope_type, scope_id, weight, enabled, options)
            SELECT $1, $2, $3, 0, true, '{}'::jsonb
            WHERE NOT EXISTS (
                SELECT 1 FROM monitoring_query_source_assignment
                WHERE query_source_id = $1
                  AND scope_type = $2
                  AND scope_id IS NOT DISTINCT FROM $3
            )
            "#,
        )
        .bind(query_source_id)
        .bind(scope_type)
        .bind(scope_id)
        .execute(&mut **tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
    }

    if let Some(alert_source_id) = profile.alert_source_id {
        sqlx::query(
            r#"
            INSERT INTO monitoring_alert_source_assignment
                (alert_source_id, scope_type, scope_id, weight, enabled)
            SELECT $1, $2, $3, 0, true
            WHERE NOT EXISTS (
                SELECT 1 FROM monitoring_alert_source_assignment
                WHERE alert_source_id = $1
                  AND scope_type = $2
                  AND scope_id IS NOT DISTINCT FROM $3
            )
            "#,
        )
        .bind(alert_source_id)
        .bind(scope_type)
        .bind(scope_id)
        .execute(&mut **tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
    }

    Ok(())
}

/// Apply a profile to a scope. Idempotent per `(profile_id, scope)`: an existing
/// assignment is returned unchanged with `created = false`.
pub(super) async fn apply_profile_to_scope(
    pool: &PgPool,
    profile: &ProfileRecord,
    input: &ProfileAssignmentInput,
) -> Result<(ProfileAssignmentRecord, bool), AppError> {
    let mut tx = pool.begin().await.map_err(classify_sqlx_error_owned)?;
    lock_profile_scrape_mutations_tx(&mut tx).await?;
    sqlx::query(
        r#"
        SELECT pg_advisory_xact_lock(
            hashtextextended(
                'monitoring_profile.apply:' || $1::text || ':' || $2 || ':' || COALESCE($3::text, ''),
                0
            )
        )
        "#,
    )
    .bind(input.profile_id)
    .bind(&input.scope_type)
    .bind(input.scope_id)
    .execute(&mut *tx)
    .await
    .map_err(classify_sqlx_error_owned)?;

    let existing = sqlx::query(
        r#"
        SELECT id FROM monitoring_profile_assignment
        WHERE profile_id = $1
          AND scope_type = $2
          AND scope_id IS NOT DISTINCT FROM $3
        LIMIT 1
        "#,
    )
    .bind(input.profile_id)
    .bind(&input.scope_type)
    .bind(input.scope_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(classify_sqlx_error_owned)?;

    if let Some(row) = existing {
        let id: Uuid = row.try_get("id").map_err(classify_sqlx_error_owned)?;
        ensure_profile_source_assignments_tx(&mut tx, profile, &input.scope_type, input.scope_id)
            .await?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        let record = fetch_profile_assignment_by_id(pool, id)
            .await?
            .ok_or_else(|| AppError::Internal("Applied assignment disappeared".to_string()))?;
        return Ok((record, false));
    }

    let row = sqlx::query(
        r#"
        INSERT INTO monitoring_profile_assignment
            (profile_id, scope_type, scope_id, weight, enabled, target)
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING id
        "#,
    )
    .bind(input.profile_id)
    .bind(&input.scope_type)
    .bind(input.scope_id)
    .bind(input.weight)
    .bind(input.enabled)
    .bind(input.target.as_deref())
    .fetch_one(&mut *tx)
    .await
    .map_err(classify_sqlx_error_owned)?;
    let id: Uuid = row.try_get("id").map_err(classify_sqlx_error_owned)?;

    validate_profile_scrape_conflicts_tx(&mut tx, input.profile_id).await?;
    ensure_profile_source_assignments_tx(&mut tx, profile, &input.scope_type, input.scope_id)
        .await?;
    tx.commit().await.map_err(classify_sqlx_error_owned)?;

    let record = fetch_profile_assignment_by_id(pool, id)
        .await?
        .ok_or_else(|| AppError::Internal("Applied assignment disappeared".to_string()))?;
    Ok((record, true))
}
