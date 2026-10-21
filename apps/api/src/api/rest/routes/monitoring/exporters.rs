//! Monitoring exporters routes, split out of `monitoring.rs`.
//!
//! Moved verbatim; behaviour and the OpenAPI annotations are unchanged.

use super::*;

use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::api::middleware::error::handle_app_error;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::app::AppState;
use crate::errors::AppError;

use super::helpers::{normalize_metrics_path, normalize_string};
use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::request_context::build_request_context;
use crate::contexts::access_control::domain::Action;
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};

// ===========================================================================
// Service Discovery configuration CRUD (exporters + exporter assignments)
// ===========================================================================

/// Catalog entry describing an exporter scrape profile.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ExporterCatalogItem {
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
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ExporterCatalogListResponse {
    pub items: Vec<ExporterCatalogItem>,
    pub total: usize,
}

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ExporterCatalogListQuery {
    pub sort: Option<String>,
    pub order: Option<String>,
    pub exporter_type: Option<String>,
    pub default_target_source: Option<String>,
    pub enabled: Option<String>,
}

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct ExporterCatalogRequest {
    pub name: String,
    pub slug: String,
    #[serde(default)]
    pub exporter_type: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub default_job: Option<String>,
    #[serde(default)]
    pub default_scheme: Option<String>,
    #[serde(default)]
    pub default_port: Option<i32>,
    #[serde(default)]
    pub default_metrics_path: Option<String>,
    #[serde(default)]
    pub default_target_source: Option<String>,
    #[serde(default)]
    pub labels: Option<Value>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

/// An exporter assignment joined with catalog metadata.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ExporterAssignmentItem {
    pub id: Uuid,
    pub exporter_id: Uuid,
    pub exporter_slug: String,
    pub exporter_name: String,
    pub name: Option<String>,
    pub scope_type: String,
    pub scope_id: Option<Uuid>,
    pub weight: i32,
    pub enabled: bool,
    pub target: Option<String>,
    pub job: Option<String>,
    pub scheme: Option<String>,
    pub port: Option<i32>,
    pub metrics_path: Option<String>,
    pub labels: Value,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ExporterAssignmentListResponse {
    pub items: Vec<ExporterAssignmentItem>,
    /// Total number of assignments matching the filters, not the page length.
    pub total: i64,
    pub limit: u32,
    pub offset: u32,
}

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ExporterAssignmentListQuery {
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub scope_type: Option<String>,
    pub scope_id: Option<String>,
    pub exporter_id: Option<String>,
    pub enabled: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

fn exporter_catalog_list_filter(
    query: &ExporterCatalogListQuery,
) -> Result<repository::MonitoringListFilter, AppError> {
    repository::MonitoringListFilter::from_params(
        repository::EXPORTER_COLUMNS,
        query.sort.as_deref(),
        query.order.as_deref(),
        &[
            ("exporter_type", query.exporter_type.as_deref()),
            (
                "default_target_source",
                query.default_target_source.as_deref(),
            ),
            ("enabled", query.enabled.as_deref()),
        ],
    )
}

fn exporter_assignment_list_filter(
    query: &ExporterAssignmentListQuery,
) -> Result<repository::MonitoringListFilter, AppError> {
    repository::MonitoringListFilter::from_params(
        repository::EXPORTER_ASSIGNMENT_COLUMNS,
        query.sort.as_deref(),
        query.order.as_deref(),
        &[
            ("exporter_id", query.exporter_id.as_deref()),
            ("scope_type", query.scope_type.as_deref()),
            ("scope_id", query.scope_id.as_deref()),
            ("enabled", query.enabled.as_deref()),
        ],
    )
}

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct ExporterAssignmentRequest {
    #[serde(default)]
    pub exporter_id: Option<Uuid>,
    #[serde(default)]
    pub exporter_slug: Option<String>,
    #[serde(default)]
    pub exporter_name: Option<String>,
    #[serde(default)]
    pub exporter_type: Option<String>,
    #[serde(default)]
    pub exporter_description: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    pub scope_type: String,
    #[serde(default)]
    pub scope_id: Option<Uuid>,
    #[serde(default)]
    pub weight: Option<i32>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub job: Option<String>,
    #[serde(default)]
    pub scheme: Option<String>,
    #[serde(default)]
    pub port: Option<i32>,
    #[serde(default)]
    pub metrics_path: Option<String>,
    #[serde(default)]
    pub labels: Option<Value>,
}

const EXPORTER_ASSIGNMENT_SCOPE_TYPES: [&str; 5] =
    ["global", "site", "tenant", "role", "connection_target"];

const EXPORTER_DEFAULT_TARGET_SOURCES: [&str; 5] = [
    "host_address",
    "host_name",
    "host_id",
    "interface_addresses",
    "tunnel_addresses",
];

struct ValidatedExporterAssignment {
    exporter_id: Option<Uuid>,
    custom_exporter: Option<repository::ExporterUpsertInput>,
    name: Option<String>,
    scope_type: String,
    scope_id: Option<Uuid>,
    weight: i32,
    enabled: bool,
    target: Option<String>,
    job: Option<String>,
    scheme: Option<String>,
    port: Option<i32>,
    metrics_path: Option<String>,
    labels: Value,
}

impl ValidatedExporterAssignment {
    fn into_repository_input(self, exporter_id: Uuid) -> repository::ExporterAssignmentInput {
        repository::ExporterAssignmentInput {
            exporter_id,
            name: self.name,
            scope_type: self.scope_type,
            scope_id: self.scope_id,
            weight: self.weight,
            enabled: self.enabled,
            target: self.target,
            job: self.job,
            scheme: self.scheme,
            port: self.port,
            metrics_path: self.metrics_path,
            labels: self.labels,
        }
    }
}

fn exporter_catalog_item(record: repository::ExporterRecord) -> ExporterCatalogItem {
    ExporterCatalogItem {
        id: record.id,
        slug: record.slug,
        name: record.name,
        exporter_type: record.exporter_type,
        description: record.description,
        default_job: record.default_job,
        default_scheme: record.default_scheme,
        default_port: record.default_port,
        default_metrics_path: record.default_metrics_path,
        default_target_source: record.default_target_source,
        labels: record.labels,
        enabled: record.enabled,
        created_at: record.created_at.to_rfc3339(),
        updated_at: record.updated_at.to_rfc3339(),
    }
}

fn exporter_assignment_item(
    record: repository::ExporterAssignmentRecord,
) -> ExporterAssignmentItem {
    ExporterAssignmentItem {
        id: record.id,
        exporter_id: record.exporter_id,
        exporter_slug: record.exporter_slug,
        exporter_name: record.exporter_name,
        name: record.name,
        scope_type: record.scope_type,
        scope_id: record.scope_id,
        weight: record.weight,
        enabled: record.enabled,
        target: record.target,
        job: record.job,
        scheme: record.scheme,
        port: record.port,
        metrics_path: record.metrics_path,
        labels: record.labels,
        created_at: record.created_at.to_rfc3339(),
        updated_at: record.updated_at.to_rfc3339(),
    }
}

fn trim_non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn normalize_exporter_slug(value: &str) -> Option<String> {
    let mut slug = String::new();
    let mut previous_separator = false;

    for char in value.trim().chars() {
        if char.is_ascii_alphanumeric() {
            slug.push(char.to_ascii_lowercase());
            previous_separator = false;
        } else if (matches!(char, '-' | '_' | '.') || char.is_whitespace())
            && !slug.is_empty()
            && !previous_separator
        {
            slug.push('-');
            previous_separator = true;
        }
    }

    while slug.ends_with('-') {
        slug.pop();
    }

    if slug.is_empty() { None } else { Some(slug) }
}

fn validate_assignment_scheme(scheme: Option<&str>) -> Result<Option<String>, AppError> {
    match scheme.map(str::trim).filter(|value| !value.is_empty()) {
        Some(scheme) => {
            let scheme = scheme.to_ascii_lowercase();
            if scheme != "http" && scheme != "https" {
                return Err(AppError::Validation(
                    "scheme must be either `http` or `https`".to_string(),
                ));
            }
            Ok(Some(scheme))
        }
        None => Ok(None),
    }
}

pub(crate) fn validate_exporter_catalog(
    payload: ExporterCatalogRequest,
) -> Result<repository::ExporterUpsertInput, AppError> {
    let slug = normalize_exporter_slug(&payload.slug)
        .ok_or_else(|| AppError::Validation("slug must contain letters or digits".to_string()))?;
    let name = normalize_string(Some(&payload.name))
        .ok_or_else(|| AppError::Validation("name must not be empty".to_string()))?;
    let exporter_type = payload
        .exporter_type
        .as_deref()
        .and_then(normalize_exporter_slug)
        .unwrap_or_else(|| slug.clone());
    let default_job = normalize_string(payload.default_job.as_deref()).unwrap_or(slug.clone());
    let default_scheme = validate_assignment_scheme(payload.default_scheme.as_deref())?
        .unwrap_or_else(|| "http".to_string());

    if let Some(port) = payload.default_port
        && !(1..=65535).contains(&port)
    {
        return Err(AppError::Validation(
            "default_port must be between 1 and 65535".to_string(),
        ));
    }

    let default_metrics_path = payload
        .default_metrics_path
        .as_deref()
        .map(normalize_metrics_path)
        .unwrap_or_else(|| "/metrics".to_string());
    let default_target_source = validate_enum_value(
        "default_target_source",
        payload
            .default_target_source
            .as_deref()
            .unwrap_or("host_address"),
        &EXPORTER_DEFAULT_TARGET_SOURCES,
    )?;
    let labels = validate_json_object("labels", payload.labels)?;

    Ok(repository::ExporterUpsertInput {
        slug,
        name,
        exporter_type,
        description: trim_non_empty(payload.description.as_deref()),
        default_job,
        default_scheme,
        default_port: payload.default_port,
        default_metrics_path,
        default_target_source,
        labels,
        enabled: payload.enabled.unwrap_or(true),
    })
}

/// Validate the mutable fields of an exporter assignment. The returned value
/// still needs an exporter id; custom exporter refs are resolved after
/// validation so invalid assignment payloads never create catalog rows.
fn validate_exporter_assignment(
    payload: ExporterAssignmentRequest,
) -> Result<ValidatedExporterAssignment, AppError> {
    let scope_type = payload.scope_type.trim().to_ascii_lowercase();
    if !EXPORTER_ASSIGNMENT_SCOPE_TYPES.contains(&scope_type.as_str()) {
        return Err(AppError::Validation(format!(
            "scope_type must be one of: {}",
            EXPORTER_ASSIGNMENT_SCOPE_TYPES.join(", ")
        )));
    }

    if scope_type == "global" {
        if payload.scope_id.is_some() {
            return Err(AppError::Validation(
                "scope_id must be omitted when scope_type is `global`".to_string(),
            ));
        }
    } else if payload.scope_id.is_none() {
        return Err(AppError::Validation(format!(
            "scope_id is required when scope_type is `{scope_type}`"
        )));
    }

    if let Some(port) = payload.port
        && !(1..=65535).contains(&port)
    {
        return Err(AppError::Validation(
            "port must be between 1 and 65535".to_string(),
        ));
    }

    let scheme = validate_assignment_scheme(payload.scheme.as_deref())?;

    let labels = match payload.labels {
        Some(Value::Object(map)) => Value::Object(map),
        None | Some(Value::Null) => Value::Object(serde_json::Map::new()),
        Some(_) => {
            return Err(AppError::Validation(
                "labels must be a JSON object".to_string(),
            ));
        }
    };

    let custom_exporter = if payload.exporter_id.is_some() {
        None
    } else {
        let raw_exporter = trim_non_empty(payload.exporter_slug.as_deref())
            .or_else(|| trim_non_empty(payload.exporter_name.as_deref()))
            .ok_or_else(|| {
                AppError::Validation(
                    "exporter_id or exporter_slug/exporter_name is required".to_string(),
                )
            })?;
        let slug = normalize_exporter_slug(&raw_exporter).ok_or_else(|| {
            AppError::Validation("exporter_slug must contain letters or digits".to_string())
        })?;
        let name = trim_non_empty(payload.exporter_name.as_deref()).unwrap_or(raw_exporter);
        let exporter_type = payload
            .exporter_type
            .as_deref()
            .and_then(normalize_exporter_slug)
            .unwrap_or_else(|| "custom".to_string());
        Some(repository::ExporterUpsertInput {
            slug: slug.clone(),
            name,
            exporter_type,
            description: trim_non_empty(payload.exporter_description.as_deref()),
            default_job: normalize_string(payload.job.as_deref()).unwrap_or(slug),
            default_scheme: scheme.clone().unwrap_or_else(|| "http".to_string()),
            default_port: payload.port,
            default_metrics_path: payload
                .metrics_path
                .as_deref()
                .map(normalize_metrics_path)
                .unwrap_or_else(|| "/metrics".to_string()),
            default_target_source: "host_address".to_string(),
            labels: Value::Object(serde_json::Map::new()),
            enabled: true,
        })
    };

    Ok(ValidatedExporterAssignment {
        exporter_id: payload.exporter_id,
        custom_exporter,
        name: trim_non_empty(payload.name.as_deref()),
        scope_type,
        scope_id: payload.scope_id,
        weight: payload.weight.unwrap_or(0),
        enabled: payload.enabled.unwrap_or(true),
        target: normalize_string(payload.target.as_deref()),
        job: normalize_string(payload.job.as_deref()),
        scheme,
        port: payload.port,
        metrics_path: normalize_string(payload.metrics_path.as_deref()),
        labels,
    })
}

async fn resolve_exporter_assignment_input(
    pool: &sqlx::PgPool,
    payload: ExporterAssignmentRequest,
) -> Result<repository::ExporterAssignmentInput, AppError> {
    let validated = validate_exporter_assignment(payload)?;
    let exporter_id = match validated.exporter_id {
        Some(exporter_id) => exporter_id,
        None => {
            let custom_exporter = validated.custom_exporter.as_ref().ok_or_else(|| {
                AppError::Validation(
                    "exporter_id or exporter_slug/exporter_name is required".to_string(),
                )
            })?;
            repository::upsert_exporter(pool, custom_exporter).await?.id
        }
    };

    Ok(validated.into_repository_input(exporter_id))
}

/// Get the service-discovery label-group settings.
#[utoipa::path(
    get,
    path = "/api/v1/monitoring/sd/label-settings",
    tag = "monitoring",
    responses(
        (status = 200, description = "Service-discovery label-group settings", body = SdLabelSettingsResponse),
        (status = "4XX", description = "Request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn get_sd_label_settings(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
) -> Response {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_monitoring_config_read(&state, &request_context).await {
        return handle_app_error(error);
    }

    match state.repositories.monitoring_sd_label_settings.get().await {
        Ok(groups) => Json(SdLabelSettingsResponse {
            groups: groups.into(),
        })
        .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update the service-discovery label-group settings.
#[utoipa::path(
    put,
    path = "/api/v1/monitoring/sd/label-settings",
    tag = "monitoring",
    request_body = SdLabelSettingsPayload,
    responses(
        (status = 200, description = "Service-discovery label-group settings updated", body = SdLabelSettingsResponse),
        (status = "4XX", description = "Request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn update_sd_label_settings(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<SdLabelSettingsPayload>,
) -> Response {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) =
        ensure_monitoring_config_write(&state, &request_context, Action::Update).await
    {
        return handle_app_error(error);
    }

    match state
        .repositories
        .monitoring_sd_label_settings
        .upsert(payload.groups.into())
        .await
    {
        Ok(groups) => Json(SdLabelSettingsResponse {
            groups: groups.into(),
        })
        .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// List the exporter catalog.
#[utoipa::path(
    get,
    path = "/api/v1/monitoring/exporters",
    tag = "monitoring",
    params(ExporterCatalogListQuery),
    responses(
        (status = 200, description = "Catalog of available monitoring exporters", body = ExporterCatalogListResponse),
        (status = "4XX", description = "Request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn list_exporters(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ExporterCatalogListQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_monitoring_config_read(&state, &request_context).await {
        return handle_app_error(error);
    }

    let filter = match exporter_catalog_list_filter(&query) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match repository::list_exporters(&state.db_pool, &filter).await {
        Ok(records) => {
            let items: Vec<ExporterCatalogItem> =
                records.into_iter().map(exporter_catalog_item).collect();
            (
                StatusCode::OK,
                Json(ExporterCatalogListResponse {
                    total: items.len(),
                    items,
                }),
            )
                .into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Create an exporter catalog profile.
#[utoipa::path(
    post,
    path = "/api/v1/monitoring/exporters",
    tag = "monitoring",
    request_body = ExporterCatalogRequest,
    responses(
        (status = 201, description = "Exporter catalog profile created", body = ExporterCatalogItem),
        (status = 400, description = "Validation error", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 409, description = "Exporter slug already exists", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn create_exporter(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<ExporterCatalogRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) =
        ensure_monitoring_config_write(&state, &request_context, Action::Create).await
    {
        return handle_app_error(error);
    }

    let input = match validate_exporter_catalog(payload) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match repository::create_exporter(&state.db_pool, &input).await {
        Ok(record) => (StatusCode::CREATED, Json(exporter_catalog_item(record))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update an exporter catalog profile.
#[utoipa::path(
    put,
    path = "/api/v1/monitoring/exporters/{id}",
    tag = "monitoring",
    params(("id" = Uuid, Path, description = "Exporter catalog profile id")),
    request_body = ExporterCatalogRequest,
    responses(
        (status = 200, description = "Exporter catalog profile updated", body = ExporterCatalogItem),
        (status = 400, description = "Validation error", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Exporter catalog profile not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 409, description = "Exporter slug already exists", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn update_exporter(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    EnvelopeJson(payload): EnvelopeJson<ExporterCatalogRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) =
        ensure_monitoring_config_write(&state, &request_context, Action::Update).await
    {
        return handle_app_error(error);
    }

    let input = match validate_exporter_catalog(payload) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match repository::update_exporter(&state.db_pool, id, &input).await {
        Ok(Some(record)) => (StatusCode::OK, Json(exporter_catalog_item(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound(
            "Exporter catalog profile not found".to_string(),
        )),
        Err(error) => handle_app_error(error),
    }
}

/// Delete an exporter catalog profile.
#[utoipa::path(
    delete,
    path = "/api/v1/monitoring/exporters/{id}",
    tag = "monitoring",
    params(("id" = Uuid, Path, description = "Exporter catalog profile id")),
    responses(
        (status = 204, description = "Exporter catalog profile deleted"),
        (status = 404, description = "Exporter catalog profile not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn delete_exporter(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) =
        ensure_monitoring_config_write(&state, &request_context, Action::Delete).await
    {
        return handle_app_error(error);
    }

    match repository::delete_exporter(&state.db_pool, id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => handle_app_error(AppError::NotFound(
            "Exporter catalog profile not found".to_string(),
        )),
        Err(error) => handle_app_error(error),
    }
}

/// List exporter assignments.
#[utoipa::path(
    get,
    path = "/api/v1/monitoring/exporter-assignments",
    tag = "monitoring",
    params(ExporterAssignmentListQuery),
    responses(
        (status = 200, description = "Exporter assignments matching the scope filters", body = ExporterAssignmentListResponse),
        (status = "4XX", description = "Request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn list_exporter_assignments(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ExporterAssignmentListQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_monitoring_config_read(&state, &request_context).await {
        return handle_app_error(error);
    }

    let filter = match exporter_assignment_list_filter(&query) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let limit = clamp_limit(query.limit);
    let offset = clamp_offset(query.offset);

    match repository::list_exporter_assignments(
        &state.db_pool,
        &filter,
        i64::from(limit),
        i64::from(offset),
    )
    .await
    {
        Ok((records, total)) => {
            let items: Vec<ExporterAssignmentItem> =
                records.into_iter().map(exporter_assignment_item).collect();
            (
                StatusCode::OK,
                Json(ExporterAssignmentListResponse {
                    total,
                    limit,
                    offset,
                    items,
                }),
            )
                .into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Create an exporter assignment.
#[utoipa::path(
    post,
    path = "/api/v1/monitoring/exporter-assignments",
    tag = "monitoring",
    request_body = ExporterAssignmentRequest,
    responses(
        (status = 201, description = "Exporter assignment created", body = ExporterAssignmentItem),
        (status = 400, description = "Validation error", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn create_exporter_assignment(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<ExporterAssignmentRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) =
        ensure_monitoring_config_write(&state, &request_context, Action::Create).await
    {
        return handle_app_error(error);
    }

    let input = match resolve_exporter_assignment_input(&state.db_pool, payload).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match repository::create_exporter_assignment(&state.db_pool, &input).await {
        Ok(record) => (StatusCode::CREATED, Json(exporter_assignment_item(record))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update an exporter assignment.
#[utoipa::path(
    put,
    path = "/api/v1/monitoring/exporter-assignments/{id}",
    tag = "monitoring",
    params(("id" = Uuid, Path, description = "Exporter assignment id")),
    request_body = ExporterAssignmentRequest,
    responses(
        (status = 200, description = "Exporter assignment updated", body = ExporterAssignmentItem),
        (status = 400, description = "Validation error", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Exporter assignment not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn update_exporter_assignment(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    EnvelopeJson(payload): EnvelopeJson<ExporterAssignmentRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) =
        ensure_monitoring_config_write(&state, &request_context, Action::Update).await
    {
        return handle_app_error(error);
    }

    let input = match resolve_exporter_assignment_input(&state.db_pool, payload).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match repository::update_exporter_assignment(&state.db_pool, id, &input).await {
        Ok(Some(record)) => {
            (StatusCode::OK, Json(exporter_assignment_item(record))).into_response()
        }
        Ok(None) => handle_app_error(AppError::NotFound(
            "Exporter assignment not found".to_string(),
        )),
        Err(error) => handle_app_error(error),
    }
}

/// Delete an exporter assignment.
#[utoipa::path(
    delete,
    path = "/api/v1/monitoring/exporter-assignments/{id}",
    tag = "monitoring",
    params(("id" = Uuid, Path, description = "Exporter assignment id")),
    responses(
        (status = 204, description = "Exporter assignment deleted"),
        (status = 404, description = "Exporter assignment not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn delete_exporter_assignment(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) =
        ensure_monitoring_config_write(&state, &request_context, Action::Delete).await
    {
        return handle_app_error(error);
    }

    match repository::delete_exporter_assignment(&state.db_pool, id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => handle_app_error(AppError::NotFound(
            "Exporter assignment not found".to_string(),
        )),
        Err(error) => handle_app_error(error),
    }
}
