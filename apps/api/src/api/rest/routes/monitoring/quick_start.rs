//! Monitoring quick start routes, split out of `monitoring.rs`.
//!
//! Moved verbatim; behaviour and the OpenAPI annotations are unchanged.

use super::*;

use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::middleware::error::handle_app_error;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, RequireAuthenticated};
use crate::app::AppState;
use crate::errors::AppError;

use super::helpers::normalize_string;
use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::request_context::build_request_context;
use crate::contexts::access_control::domain::Action;

use crate::contexts::monitoring::application::validate_monitoring_query_source_url;

// ===========================================================================
// Preset apply + quick-start composites
//
// These collapse the "configure-monitoring-for-a-host" path from three or
// four round-trips into a single POST. Both routes are scope-aware,
// idempotent, and run inside a single pool-level transaction so a partial
// failure rolls back every row created by the call.
// ===========================================================================

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ExporterPresetCatalogItem {
    /// Exporter slug accepted by `/api/v1/monitoring/exporters/{slug}/apply`.
    pub exporter_slug: String,
    /// Slug stored on the metric set the preset creates.
    pub set_slug: String,
    /// Human-readable preset name.
    pub name: String,
    /// Short description shown in the catalog.
    pub description: String,
    /// Number of metric queries bundled in the preset.
    pub query_count: u32,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ExporterPresetCatalogResponse {
    pub items: Vec<ExporterPresetCatalogItem>,
}

/// List the built-in exporter presets accepted by the apply and quick-start
/// endpoints.
#[utoipa::path(
    get,
    path = "/api/v1/monitoring/presets",
    tag = "monitoring",
    responses(
        (status = 200, description = "Preset catalog", body = ExporterPresetCatalogResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn list_monitoring_presets(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_monitoring_config_read(&state, &request_context).await {
        return handle_app_error(error);
    }

    let items = presets::EXPORTER_PRESETS
        .iter()
        .map(|preset| ExporterPresetCatalogItem {
            exporter_slug: preset.exporter_slug.to_string(),
            set_slug: preset.set_slug.to_string(),
            name: preset.name.to_string(),
            description: preset.description.to_string(),
            query_count: u32::try_from(preset.queries.len()).unwrap_or(u32::MAX),
        })
        .collect();
    (
        StatusCode::OK,
        Json(ExporterPresetCatalogResponse { items }),
    )
        .into_response()
}

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct ExporterPresetApplyRequest {
    /// Query source that backs the preset's metric queries. The apply call
    /// enables an assignment for it at the same scope so the preset reads from
    /// this source on resolution.
    pub query_source_id: Uuid,
    /// Scope at which the resulting metric set assignment is attached.
    pub scope_type: String,
    /// Required for any `scope_type` other than `"global"`.
    #[serde(default)]
    pub scope_id: Option<Uuid>,
    /// Sort weight applied when the metric-set assignment is created.
    /// Re-applies do NOT overwrite an existing weight.
    #[serde(default)]
    pub weight: Option<i32>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ExporterPresetApplyResponse {
    pub exporter_id: Uuid,
    pub query_source_id: Uuid,
    pub query_source_assignment_id: Uuid,
    pub metric_set_id: Uuid,
    pub metric_set_assignment_id: Uuid,
    pub metric_query_ids: Vec<Uuid>,
}

/// Apply a built-in exporter preset to the given scope.
///
/// The path `{slug}` is the **exporter slug** (e.g. `node`, `docker`,
/// `blackbox`, `postgres`). The endpoint runs in a single transaction:
///   * resolves the exporter catalog profile (fails 404 if absent);
///   * validates `query_source_id` references an enabled query source;
///   * ensures every preset metric query exists for that exporter
///     (a slug is unique within an exporter — re-apply reuses existing rows);
///   * ensures the preset's metric set exists, KEEPING the operator's
///     name/description if the set already exists;
///   * ensures one metric-set item per preset query, KEEPING any
///     operator-tuned `show_in_list`/`sort_order` on existing items;
///   * ensures the metric set is assigned at the requested scope (`weight`
///     applies only to a newly created assignment);
///   * ensures the query source is enabled at the same scope so preset
///     queries actually read from the chosen source.
#[utoipa::path(
    post,
    path = "/api/v1/monitoring/exporters/{slug}/apply",
    tag = "monitoring",
    params(("slug" = String, Path, description = "Exporter slug to apply (resolves a built-in preset)")),
    request_body = ExporterPresetApplyRequest,
    responses(
        (status = 200, description = "Preset applied", body = ExporterPresetApplyResponse),
        (status = 400, description = "Validation error", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "No built-in preset for the given slug", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn apply_exporter_preset(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(slug): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<ExporterPresetApplyRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) =
        ensure_monitoring_config_write(&state, &request_context, Action::Create).await
    {
        return handle_app_error(error);
    }

    let preset = match presets::find_preset_by_exporter_slug(&slug) {
        Some(value) => value,
        None => {
            return handle_app_error(AppError::NotFound(format!(
                "no monitoring preset for exporter slug `{slug}`"
            )));
        }
    };

    let scope_type = match validate_monitoring_scope(&payload.scope_type, payload.scope_id) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let weight = payload.weight.unwrap_or(0);

    let applied = match repository::apply_exporter_preset(
        &state.db_pool,
        preset,
        payload.query_source_id,
        &scope_type,
        payload.scope_id,
        weight,
    )
    .await
    {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    (
        StatusCode::OK,
        Json(ExporterPresetApplyResponse {
            exporter_id: applied.exporter_id,
            query_source_id: payload.query_source_id,
            query_source_assignment_id: applied.query_source_assignment_id,
            metric_set_id: applied.metric_set_id,
            metric_set_assignment_id: applied.metric_set_assignment_id,
            metric_query_ids: applied.metric_query_ids,
        }),
    )
        .into_response()
}

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct MonitoringQuickStartRequest {
    /// Endpoint-only Prometheus-compatible URL (e.g. VictoriaMetrics or
    /// vmselect). URL userinfo, query parameters, and fragments are rejected;
    /// bind credentials through `credential_ref` instead.
    pub query_url: String,
    /// Source type discriminator: `prometheus`, `victoriametrics`, or `zabbix`.
    pub source_type: String,
    /// Optional display name. Defaults to a derivation of `query_url`.
    #[serde(default)]
    pub name: Option<String>,
    /// Optional canonical UUID of an existing named secret used as the source
    /// bearer token.
    #[serde(default, deserialize_with = "deserialize_optional_canonical_uuid")]
    #[schema(pattern = "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")]
    pub credential_ref: Option<Uuid>,
    /// Optional exporter preset to apply once the source is in place. Must be
    /// an **exporter slug** (e.g. `node`); use `GET /api/v1/monitoring/presets`
    /// to enumerate the available slugs.
    #[serde(default)]
    pub preset_slug: Option<String>,
    /// Scope for both the query source assignment and the optional preset.
    /// Defaults to `"global"` (scope_id NULL).
    #[serde(default)]
    pub scope_type: Option<String>,
    #[serde(default)]
    pub scope_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct MonitoringQuickStartResponse {
    pub query_source_id: Uuid,
    pub query_source_assignment_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub applied_preset: Option<ExporterPresetApplyResponse>,
}

/// Create or update a query source, attach it to a scope, and optionally apply
/// a built-in exporter preset; a preset-apply failure rolls back the whole
/// call.
#[utoipa::path(
    post,
    path = "/api/v1/monitoring/quick-start",
    tag = "monitoring",
    request_body = MonitoringQuickStartRequest,
    responses(
        (status = 200, description = "Quick-start applied", body = MonitoringQuickStartResponse),
        (status = 400, description = "Validation error", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Monitoring configuration write or query-source credential disclosure is forbidden", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Referenced preset slug not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn monitoring_quick_start(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<MonitoringQuickStartRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) =
        ensure_monitoring_config_write(&state, &request_context, Action::Create).await
    {
        return handle_app_error(error);
    }
    let can_update =
        match monitoring_config_write_allowed(&state, &request_context, Action::Update).await {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };
    let quick_start_authorization = repository::QuickStartAuthorization {
        can_create: true,
        can_update,
    };

    let query_url = match normalize_string(Some(&payload.query_url)) {
        Some(value) => value,
        None => {
            return handle_app_error(AppError::Validation(
                "query_url must not be empty".to_string(),
            ));
        }
    };
    if let Err(error) = validate_monitoring_query_source_url(&query_url) {
        return handle_app_error(error);
    }
    let source_type = match validate_enum_value(
        "source_type",
        &payload.source_type,
        &MONITORING_SOURCE_TYPES,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let credential_secret_id = payload.credential_ref;
    if let Err(error) =
        ensure_query_source_credential_use(&state, &request_context, credential_secret_id).await
    {
        return handle_app_error(error);
    }
    let raw_scope = payload.scope_type.as_deref().unwrap_or("global");
    let scope_type = match validate_monitoring_scope(raw_scope, payload.scope_id) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let name = payload
        .name
        .as_deref()
        .and_then(|v| normalize_string(Some(v)))
        .unwrap_or_else(|| derive_quick_start_name(&query_url, &source_type));
    // Slug folds in source_type + the URL host AND path so two distinct URLs
    // on the same host don't silently collide into the same query source.
    let slug = derive_quick_start_slug(&format!("{source_type} {query_url}"));

    let preset = match payload.preset_slug.as_deref() {
        None => None,
        Some(raw) => match presets::find_preset_by_exporter_slug(raw) {
            Some(value) => Some(value),
            None => {
                return handle_app_error(AppError::NotFound(format!(
                    "no monitoring preset for exporter slug `{raw}`"
                )));
            }
        },
    };

    let (result, applied) = match repository::run_quick_start(
        &state.db_pool,
        quick_start_authorization,
        repository::QuickStartSpec {
            name: &name,
            slug: &slug,
            source_type: &source_type,
            query_url: &query_url,
            credential_secret_id,
            scope_type: &scope_type,
            scope_id: payload.scope_id,
        },
        preset,
    )
    .await
    {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let applied_preset_dto = applied.map(|applied| ExporterPresetApplyResponse {
        exporter_id: applied.exporter_id,
        query_source_id: result.query_source_id,
        query_source_assignment_id: applied.query_source_assignment_id,
        metric_set_id: applied.metric_set_id,
        metric_set_assignment_id: applied.metric_set_assignment_id,
        metric_query_ids: applied.metric_query_ids,
    });

    (
        StatusCode::OK,
        Json(MonitoringQuickStartResponse {
            query_source_id: result.query_source_id,
            query_source_assignment_id: result.query_source_assignment_id,
            applied_preset: applied_preset_dto,
        }),
    )
        .into_response()
}

/// Derive a default display name for the quick-start query source when the
/// caller didn't pass one. Keeps the value short and recognisable; the slug
/// folds in the URL path so two URLs on the same host don't collide.
pub(crate) fn derive_quick_start_name(query_url: &str, source_type: &str) -> String {
    let stripped = query_url
        .strip_prefix("https://")
        .or_else(|| query_url.strip_prefix("http://"))
        .unwrap_or(query_url)
        .trim_start_matches('/');
    let host = stripped.split('/').next().unwrap_or("");
    if host.is_empty() {
        format!("{source_type} quick-start")
    } else {
        format!("{source_type} {host}")
    }
}

/// Slugify the source name for the quick-start `monitoring_query_source` row.
/// Lowercases, replaces non-alphanumerics with `-`, and clamps to a sensible
/// length so callers don't blow the 128-char column.
pub(crate) fn derive_quick_start_slug(name: &str) -> String {
    let mut slug = String::with_capacity(name.len());
    let mut last_was_dash = true;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            for lower in ch.to_lowercase() {
                slug.push(lower);
            }
            last_was_dash = false;
        } else if !last_was_dash {
            slug.push('-');
            last_was_dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        slug.push_str("quick-start");
    }
    if slug.len() > 96 {
        slug.truncate(96);
        while slug.ends_with('-') {
            slug.pop();
        }
    }
    slug
}
