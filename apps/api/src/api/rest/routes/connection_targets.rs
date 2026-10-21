use axum::{Extension, Json, extract::State, http::HeaderMap, response::IntoResponse};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::rest::envelope_extractors::{Query, Path};
use crate::api::connection_target_visibility::{
    ConnectionTargetVisibility, resolve_connection_target_visibility,
};
use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::request_context::build_request_context;
use crate::api::rest::routes::connection_config::{
    EffectiveConnConfigQuery, EffectiveConnConfigResponse,
    resolve_effective_connection_config_response,
};
use crate::api::rest::routes::connection_launch_options::{
    ConnectionLaunchOptionsResponse, get_connection_launch_options,
};
use crate::app::AppState;
use crate::config::constants::MAX_CONNECTION_TARGET_LIST_LIMIT;
use crate::contexts::inventory::infrastructure::persistence::repositories::{
    ConnectionTarget, ConnectionTargetFilters, ConnectionTargetKind, ConnectionTargetKindFilter,
    ConnectionTargetRepository,
};
use crate::contexts::inventory::infrastructure::persistence::repositories::native::connection_target_repository::CONNECTION_TARGET_COLUMNS;
use crate::contexts::shared::list_query::{SortSpec, parse_csv_uuids};
use crate::contexts::shared::pagination::{clamp_limit_with_max, clamp_offset};
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionTargetKindQueryParam {
    Device,
    VirtualMachine,
}

impl From<ConnectionTargetKind> for ConnectionTargetKindQueryParam {
    fn from(kind: ConnectionTargetKind) -> Self {
        match kind {
            ConnectionTargetKind::Device => Self::Device,
            ConnectionTargetKind::VirtualMachine => Self::VirtualMachine,
        }
    }
}

/// Query parameters for the connection target list.
///
/// Mirrors the shared pagination shape but adds an optional free-text `search`.
/// An empty or whitespace-only `search` is treated as no filter; otherwise it is
/// applied as a case-insensitive `ILIKE '%term%'` across the target's text
/// columns (`name`, `address`, `device_name`, `vm_name`).
#[derive(Debug, Deserialize)]
pub struct ConnectionTargetListQuery {
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub search: Option<String>,
    pub name: Option<String>,
    /// Comma-separated statuses; matches any (`IN`, case-insensitive).
    pub status: Option<String>,
    pub address: Option<String>,
    /// Comma-separated region UUIDs; matches any (`IN`).
    pub region_id: Option<String>,
    /// Comma-separated site UUIDs; matches any (`IN`).
    pub site_id: Option<String>,
    /// Comma-separated tenant UUIDs; matches any (`IN`).
    pub tenant_id: Option<String>,
    /// Comma-separated role UUIDs; matches any (`IN`).
    pub role_id: Option<String>,
    /// Comma-separated platform UUIDs; matches any (`IN`).
    pub platform_id: Option<String>,
    /// Comma-separated device type UUIDs; matches any (`IN`).
    pub device_type_id: Option<String>,
    /// Comma-separated target kinds; matches any (`IN`).
    pub target_kind: Option<String>,
    /// Logical sort column key (allowlisted; unknown keys are rejected).
    pub sort: Option<String>,
    /// Sort direction: `asc` (default) or `desc`.
    pub order: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ConnectionTargetListRowResponse {
    pub id: String,
    pub name: String,
    pub address: Option<String>,
    /// True when the selected endpoint address is bound to one of the target's
    /// inventory interfaces in IPAM. When no explicit primary IP is set, the
    /// backend falls back to the first assigned interface IP (IPv4 before
    /// IPv6), and this flag stays true for that fallback address.
    pub address_bound: bool,
    pub target_kind: ConnectionTargetKindQueryParam,
    pub status: Option<String>,
    pub device_id: Option<String>,
    pub device_name: Option<String>,
    pub vm_id: Option<String>,
    pub vm_name: Option<String>,
    pub tenant_id: Option<String>,
    pub tenant_name: Option<String>,
    pub site_id: Option<String>,
    pub site_name: Option<String>,
    pub region_id: Option<String>,
    pub region_name: Option<String>,
    pub role_id: Option<String>,
    pub role_name: Option<String>,
    pub role_custom_fields: Option<serde_json::Value>,
    pub platform_id: Option<String>,
    pub platform_name: Option<String>,
    /// Cctv device family (stable wire slug, e.g. `dvrip_sofia`) derived from the
    /// platform driver. `None` for non-cctv targets.
    pub family: Option<String>,
    /// True when the target is a managed cctv device (recorder/camera) — its
    /// device-management plane and `cctv` launch protocol are available.
    pub is_cctv: bool,
    pub device_type_id: Option<String>,
    pub device_type_name: Option<String>,
    pub cluster_id: Option<String>,
    pub cluster_name: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ConnectionTargetListEnvelope {
    pub items: Vec<ConnectionTargetListRowResponse>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

impl From<ConnectionTarget> for ConnectionTargetListRowResponse {
    fn from(target: ConnectionTarget) -> Self {
        Self {
            id: target.id.to_string(),
            name: target.name,
            address: target.address,
            address_bound: target.address_bound,
            target_kind: target.target_kind.into(),
            status: target.status,
            device_id: target.device_id.map(|value| value.to_string()),
            device_name: target.device_name,
            vm_id: target.vm_id.map(|value| value.to_string()),
            vm_name: target.vm_name,
            tenant_id: target.tenant_id.map(|value| value.to_string()),
            tenant_name: target.tenant_name,
            site_id: target.site_id.map(|value| value.to_string()),
            site_name: target.site_name,
            region_id: target.region_id.map(|value| value.to_string()),
            region_name: target.region_name,
            role_id: target.role_id.map(|value| value.to_string()),
            role_name: target.role_name,
            role_custom_fields: target.role_custom_fields,
            platform_id: target.platform_id.map(|value| value.to_string()),
            platform_name: target.platform_name,
            family: target.family,
            is_cctv: target.is_cctv,
            device_type_id: target.device_type_id.map(|value| value.to_string()),
            device_type_name: target.device_type_name,
            cluster_id: target.cluster_id.map(|value| value.to_string()),
            cluster_name: target.cluster_name,
        }
    }
}

fn connection_target_filters_from_query(
    query: ConnectionTargetListQuery,
) -> Result<ConnectionTargetFilters, AppError> {
    Ok(ConnectionTargetFilters {
        id: None,
        search: clean_query_string(query.search),
        name: clean_query_string(query.name),
        status: crate::contexts::shared::list_query::parse_csv_strings(query.status.as_deref()),
        address: clean_query_string(query.address),
        region_id: parse_csv_uuids(query.region_id.as_deref(), "region_id")?,
        site_id: parse_csv_uuids(query.site_id.as_deref(), "site_id")?,
        tenant_id: parse_csv_uuids(query.tenant_id.as_deref(), "tenant_id")?,
        role_id: parse_csv_uuids(query.role_id.as_deref(), "role_id")?,
        platform_id: parse_csv_uuids(query.platform_id.as_deref(), "platform_id")?,
        device_type_id: parse_csv_uuids(query.device_type_id.as_deref(), "device_type_id")?,
        target_kind: parse_csv_target_kinds(query.target_kind.as_deref())?,
        sort: SortSpec::parse_columns(
            query.sort.as_deref(),
            query.order.as_deref(),
            CONNECTION_TARGET_COLUMNS,
        )?,
        // Set by the handler from the caller's resolved policies.
        visibility: None,
    })
}

fn clean_query_string(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Parse a comma-separated list of target kinds into a deduplicated vector.
/// Empty/whitespace entries are skipped; an unknown kind is a client error.
fn parse_csv_target_kinds(raw: Option<&str>) -> Result<Vec<ConnectionTargetKindFilter>, AppError> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let mut out: Vec<ConnectionTargetKindFilter> = Vec::new();
    for token in raw.split(',') {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            continue;
        }
        let kind = ConnectionTargetKindFilter::parse(trimmed).ok_or_else(|| {
            AppError::Validation(
                "Invalid target_kind: expected 'device' or 'virtual_machine'".to_string(),
            )
        })?;
        if !out.contains(&kind) {
            out.push(kind);
        }
    }
    Ok(out)
}

pub async fn resolve_connection_target_list(
    state: &AppState,
    request_context: &RequestContext,
    query: ConnectionTargetListQuery,
) -> Result<ConnectionTargetListEnvelope, AppError> {
    // Row-level authorization: the caller's `Read × CONNECTION_TARGET` policies
    // compile to a per-row visibility predicate the read-model applies in SQL.
    // The coarse `ensure_access(Read, CT)` gate is not used here: it evaluates
    // with no object attributes and so denies a caller whose ONLY read grant is
    // attribute-constrained. `None` visibility (no applicable read grant at all,
    // or a PAT scope that excludes this list) is a 403, the same deny a caller
    // with zero connection-target read gets.
    let visibility = resolve_connection_target_visibility(state, request_context).await?;
    if matches!(visibility, ConnectionTargetVisibility::None) {
        return Err(AppError::Forbidden("Access denied".to_string()));
    }

    let limit_u32 = clamp_limit_with_max(query.limit, MAX_CONNECTION_TARGET_LIST_LIMIT);
    let offset_u32 = clamp_offset(query.offset);
    let limit = i64::from(limit_u32);
    let offset = i64::from(offset_u32);
    let mut filters = connection_target_filters_from_query(query)?;
    filters.visibility = visibility.into_filter();

    let repo = state.repositories.connection_targets.clone();
    let (rows, total) = repo.list(limit, offset, filters).await?;
    let items = rows.into_iter().map(Into::into).collect();
    Ok(ConnectionTargetListEnvelope {
        items,
        total,
        limit,
        offset,
    })
}

pub async fn resolve_connection_target(
    state: &AppState,
    request_context: &RequestContext,
    id: &str,
) -> Result<ConnectionTargetListRowResponse, AppError> {
    let visibility = resolve_connection_target_visibility(state, request_context).await?;
    resolve_connection_target_with_visibility(
        &state.repositories.connection_targets,
        visibility,
        id,
    )
    .await
}

async fn resolve_connection_target_with_visibility(
    repository: &dyn ConnectionTargetRepository,
    visibility: ConnectionTargetVisibility,
    id: &str,
) -> Result<ConnectionTargetListRowResponse, AppError> {
    if matches!(visibility, ConnectionTargetVisibility::None) {
        return Err(AppError::Forbidden("Access denied".to_string()));
    }

    let target_id = Uuid::parse_str(id)
        .map_err(|_| AppError::NotFound("Connection target not found".to_string()))?;
    let filters = ConnectionTargetFilters {
        id: Some(target_id),
        visibility: visibility.into_filter(),
        ..ConnectionTargetFilters::default()
    };
    let (targets, _) = repository.list(2, 0, filters).await?;
    targets
        .into_iter()
        .next()
        .map(Into::into)
        .ok_or_else(|| AppError::NotFound("Connection target not found".to_string()))
}

/// List connection targets.
///
/// This is the user-facing connection read model over connectable inventory
/// objects. The current backing sources are devices and virtual machines.
#[utoipa::path(
    get,
    path = "/api/v1/connections/targets",
    tag = "connections",
    params(
        ("limit" = Option<u32>, Query, description = "Maximum number of connection targets to return"),
        ("offset" = Option<u32>, Query, description = "Number of connection targets to skip"),
        ("search" = Option<String>, Query, description = "Optional case-insensitive substring filter applied to target name/address/device_name/vm_name. Empty or whitespace-only is treated as no filter."),
        ("name" = Option<String>, Query, description = "Optional case-insensitive substring filter applied to target name."),
        ("status" = Option<String>, Query, description = "Filter by status (comma-separated, case-insensitive, matches any)."),
        ("address" = Option<String>, Query, description = "Optional case-insensitive substring filter applied to primary address."),
        ("region_id" = Option<String>, Query, description = "Filter by region UUIDs (comma-separated, matches any)."),
        ("site_id" = Option<String>, Query, description = "Filter by site UUID (comma-separated, matches any)."),
        ("tenant_id" = Option<String>, Query, description = "Filter by tenant UUIDs (comma-separated, matches any)."),
        ("role_id" = Option<String>, Query, description = "Filter by role UUID (comma-separated, matches any)."),
        ("platform_id" = Option<String>, Query, description = "Filter by platform UUIDs (comma-separated, matches any)."),
        ("device_type_id" = Option<String>, Query, description = "Filter by device type UUIDs (comma-separated, matches any)."),
        ("target_kind" = Option<String>, Query, description = "Filter by target kind: device or virtual_machine (comma-separated, matches any)."),
        ("sort" = Option<String>, Query, description = "Sort key: name, status, targetKind, region, site, tenant, role, platform, deviceType, primaryIp (allowlisted; unknown rejected)."),
        ("order" = Option<String>, Query, description = "Sort direction: asc (default) or desc."),
    ),
    operation_id = "list_connection_targets",
    responses(
        (status = 200, description = "Connection targets listed", body = ConnectionTargetListEnvelope),
        (status = "4XX", description = "Connection target list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Connection target list request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_connection_targets(
    state: State<AppState>,
    request_id: Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ConnectionTargetListQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    match resolve_connection_target_list(&state, &request_context, query).await {
        Ok(envelope) => (axum::http::StatusCode::OK, Json(envelope)).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Get one connection target by id.
///
/// Uses the same row projection and row-level read visibility as the collection.
#[utoipa::path(
    get,
    path = "/api/v1/connections/targets/{id}",
    tag = "connections",
    params(
        ("id" = String, Path, description = "Connection target id"),
    ),
    operation_id = "get_connection_target",
    responses(
        (status = 200, description = "Connection target", body = ConnectionTargetListRowResponse),
        (status = 404, description = "Connection target not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Connection target request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Connection target request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_connection_target(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    match resolve_connection_target(&state, &request_context, &id).await {
        Ok(target) => (axum::http::StatusCode::OK, Json(target)).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Get launch options available for a connection target.
#[utoipa::path(
    get,
    path = "/api/v1/connections/targets/{id}/launch-options",
    tag = "connections",
    params(
        ("id" = String, Path, description = "Connection target id"),
    ),
    operation_id = "get_connection_target_launch_options",
    responses(
        (status = 200, description = "Per-protocol launchable accounts", body = ConnectionLaunchOptionsResponse),
        (status = 404, description = "Connection target not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_connection_target_launch_options(
    state: State<AppState>,
    request_id: Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    path: Path<String>,
) -> impl IntoResponse {
    get_connection_launch_options(state, request_id, auth_context, headers, path).await
}

/// Resolve the effective connection config for a connection target.
#[utoipa::path(
    get,
    path = "/api/v1/connections/targets/{id}/effective-config",
    tag = "connections",
    params(
        EffectiveConnConfigQuery,
        ("id" = String, Path, description = "Connection target id"),
    ),
    operation_id = "get_connection_target_effective_config",
    responses(
        (status = 200, description = "Effective connection config", body = EffectiveConnConfigResponse),
        (status = 404, description = "Connection target not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Effective config request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Effective config request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_connection_target_effective_config(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<EffectiveConnConfigQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    resolve_effective_connection_config_response(&state, &request_context, &id, query).await
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;

    use super::*;

    #[derive(Clone, Default)]
    struct StubConnectionTargetRepository {
        targets: Vec<ConnectionTarget>,
    }

    #[async_trait::async_trait]
    impl ConnectionTargetRepository for StubConnectionTargetRepository {
        async fn list(
            &self,
            _limit: i64,
            _offset: i64,
            filters: ConnectionTargetFilters,
        ) -> Result<(Vec<ConnectionTarget>, i64), AppError> {
            let rows_are_visible = !matches!(
                filters.visibility.as_ref(),
                Some(crate::contexts::access_control::domain::VisibilityPredicate::Never)
            );
            let targets = self
                .targets
                .iter()
                .filter(|target| rows_are_visible && filters.id == Some(target.id))
                .cloned()
                .collect::<Vec<_>>();
            let total = i64::try_from(targets.len()).unwrap_or(i64::MAX);
            Ok((targets, total))
        }
    }

    fn target(id: Uuid) -> ConnectionTarget {
        ConnectionTarget {
            id,
            name: "North gate recorder".to_string(),
            address: Some("198.51.100.24".to_string()),
            address_bound: true,
            target_kind: ConnectionTargetKind::Device,
            status: Some("active".to_string()),
            device_id: Some(id),
            device_name: Some("North gate recorder".to_string()),
            vm_id: None,
            vm_name: None,
            tenant_id: None,
            tenant_name: None,
            site_id: None,
            site_name: None,
            region_id: None,
            region_name: None,
            role_id: None,
            role_name: None,
            role_custom_fields: None,
            platform_id: None,
            platform_name: Some("DVRIP".to_string()),
            platform_driver: Some("dvrip_sofia".to_string()),
            family: Some("dvrip_sofia".to_string()),
            is_cctv: true,
            device_type_id: None,
            device_type_name: Some("NVR".to_string()),
            cluster_id: None,
            cluster_name: None,
        }
    }

    async fn error_json(error: AppError) -> (axum::http::StatusCode, serde_json::Value) {
        let response = handle_app_error(error);
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("error body");
        let json = serde_json::from_slice(&bytes).expect("error json");
        (status, json)
    }

    #[tokio::test]
    async fn get_connection_target_returns_the_collection_item_dto() {
        let id = Uuid::now_v7();
        let repository = StubConnectionTargetRepository {
            targets: vec![target(id)],
        };

        let response = resolve_connection_target_with_visibility(
            &repository,
            ConnectionTargetVisibility::All,
            &id.to_string(),
        )
        .await
        .expect("visible target");

        assert_eq!(
            serde_json::to_value(response).expect("response json"),
            serde_json::json!({
                "id": id.to_string(),
                "name": "North gate recorder",
                "address": "198.51.100.24",
                "address_bound": true,
                "target_kind": "device",
                "status": "active",
                "device_id": id.to_string(),
                "device_name": "North gate recorder",
                "vm_id": null,
                "vm_name": null,
                "tenant_id": null,
                "tenant_name": null,
                "site_id": null,
                "site_name": null,
                "region_id": null,
                "region_name": null,
                "role_id": null,
                "role_name": null,
                "role_custom_fields": null,
                "platform_id": null,
                "platform_name": "DVRIP",
                "family": "dvrip_sofia",
                "is_cctv": true,
                "device_type_id": null,
                "device_type_name": "NVR",
                "cluster_id": null,
                "cluster_name": null
            })
        );
    }

    #[tokio::test]
    async fn get_connection_target_returns_not_found_error_envelope_for_unknown_id() {
        let error = resolve_connection_target_with_visibility(
            &StubConnectionTargetRepository::default(),
            ConnectionTargetVisibility::All,
            &Uuid::now_v7().to_string(),
        )
        .await
        .expect_err("missing target");

        let (status, json) = error_json(error).await;
        assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
        assert_eq!(json["code"], "NOT_FOUND");
        assert_eq!(json["reason_code"], "NOT_FOUND");
    }

    #[tokio::test]
    async fn get_connection_target_hides_a_row_outside_the_callers_visibility() {
        let id = Uuid::now_v7();
        let repository = StubConnectionTargetRepository {
            targets: vec![target(id)],
        };

        let error = resolve_connection_target_with_visibility(
            &repository,
            ConnectionTargetVisibility::Restricted(
                crate::contexts::access_control::domain::VisibilityPredicate::Never,
            ),
            &id.to_string(),
        )
        .await
        .expect_err("restricted target");

        let (status, json) = error_json(error).await;
        assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
        assert_eq!(json["code"], "NOT_FOUND");
    }

    #[tokio::test]
    async fn get_connection_target_forbids_user_without_connection_target_read() {
        let error = resolve_connection_target_with_visibility(
            &StubConnectionTargetRepository::default(),
            ConnectionTargetVisibility::None,
            &Uuid::now_v7().to_string(),
        )
        .await
        .expect_err("missing read permission");

        let (status, json) = error_json(error).await;
        assert_eq!(status, axum::http::StatusCode::FORBIDDEN);
        assert_eq!(json["code"], "403");
    }
}
