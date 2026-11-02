use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use uuid::Uuid;

use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::domain::VpnTunnel;
use crate::contexts::inventory::infrastructure::persistence::{
    VPN_TUNNEL_COLUMNS, VpnTunnelFilter, VpnTunnelRepository,
};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_strings, parse_csv_uuids};
use crate::errors::AppError;

use super::super::{
    BulkCreateRequest, BulkCreateResponse, InventoryListEnvelope,
    bump_host_config_cache_generation, ensure_access, ensure_create_inventory_access,
    parse_optional_uuid, parse_required_string, parse_uuid, push_bulk_error, require_bulk_items,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};

#[derive(Debug, serde::Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct TunnelListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub status: Option<String>,
    pub encapsulation: Option<String>,
    pub group_id: Option<String>,
    pub ipsec_profile_id: Option<String>,
    pub tenant_id: Option<String>,
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct TunnelPayload {
    pub name: String,
    pub status: String,
    pub encapsulation: String,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub group_id: Option<String>,
    pub ipsec_profile_id: Option<String>,
    pub tunnel_id: Option<i64>,
    pub tenant_id: Option<String>,
}

struct ParsedTunnel {
    name: String,
    status: String,
    encapsulation: String,
    group_id: Option<Uuid>,
    ipsec_profile_id: Option<Uuid>,
    tenant_id: Option<Uuid>,
}

fn parse_tunnel_payload(payload: &TunnelPayload) -> Result<ParsedTunnel, AppError> {
    Ok(ParsedTunnel {
        name: parse_required_string(&payload.name, "name")?,
        status: parse_required_string(&payload.status, "status")?,
        encapsulation: parse_required_string(&payload.encapsulation, "encapsulation")?,
        group_id: parse_optional_uuid(payload.group_id.clone(), "group_id")?,
        ipsec_profile_id: parse_optional_uuid(
            payload.ipsec_profile_id.clone(),
            "ipsec_profile_id",
        )?,
        tenant_id: parse_optional_uuid(payload.tenant_id.clone(), "tenant_id")?,
    })
}

/// List tunnels.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/tunnels",
    operation_id = "list_inventory_tunnels",
    tag = "inventory",
    params(TunnelListQuery),
    responses(
        (status = 200, description = "Tunnels", body = InventoryListEnvelope<VpnTunnel>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_tunnels(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<TunnelListQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Read,
        ResourceType::Inventory,
    )
    .await
    {
        return handle_app_error(error);
    }
    let resolved_limit = clamp_limit(query.limit);
    let resolved_offset = clamp_offset(query.offset);
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        VPN_TUNNEL_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let group_id = match parse_csv_uuids(query.group_id.as_deref(), "group_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let ipsec_profile_id =
        match parse_csv_uuids(query.ipsec_profile_id.as_deref(), "ipsec_profile_id") {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };
    let tenant_id = match parse_csv_uuids(query.tenant_id.as_deref(), "tenant_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let filter = VpnTunnelFilter {
        search: crate::contexts::shared::list_query::search_pattern(query.search.as_deref()),
        status: parse_csv_strings(query.status.as_deref()),
        encapsulation: parse_csv_strings(query.encapsulation.as_deref()),
        group_id,
        ipsec_profile_id,
        tenant_id,
    };
    match state
        .repositories
        .vpn_tunnels
        .list_paginated(query.limit, query.offset, &filter, sort)
        .await
    {
        Ok((items, total)) => (
            StatusCode::OK,
            Json(InventoryListEnvelope {
                items,
                total,
                limit: resolved_limit,
                offset: resolved_offset,
            }),
        )
            .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Get a tunnel.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/tunnels/{id}",
    operation_id = "get_inventory_tunnel",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Tunnel ID"),
    ),
    responses(
        (status = 200, description = "Tunnel", body = VpnTunnel),
        (status = 404, description = "Tunnel not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_tunnel(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Read,
        ResourceType::Inventory,
    )
    .await
    {
        return handle_app_error(error);
    }
    let entity_id = match parse_uuid(&id, "tunnel_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state.repositories.vpn_tunnels.get_by_id(entity_id).await {
        Ok(Some(item)) => (StatusCode::OK, Json(item)).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Tunnel not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a tunnel.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/tunnels",
    operation_id = "create_inventory_tunnel",
    tag = "inventory",
    request_body = TunnelPayload,
    responses(
        (status = 201, description = "Tunnel created", body = VpnTunnel),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_tunnel(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<TunnelPayload>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Create,
        ResourceType::Inventory,
    )
    .await
    {
        return handle_app_error(error);
    }
    let parsed = match parse_tunnel_payload(&payload) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let item = VpnTunnel {
        id: Uuid::now_v7(),
        name: parsed.name,
        status: parsed.status,
        encapsulation: parsed.encapsulation,
        description: payload.description.unwrap_or_default(),
        comments: payload.comments.unwrap_or_default(),
        group_id: parsed.group_id,
        ipsec_profile_id: parsed.ipsec_profile_id,
        tunnel_id: payload.tunnel_id,
        tenant_id: parsed.tenant_id,
        ..Default::default()
    };
    match state.repositories.vpn_tunnels.create(&item).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            (StatusCode::CREATED, Json(item)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create tunnels.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/tunnels/bulk",
    operation_id = "bulk_create_inventory_tunnels",
    tag = "inventory",
    request_body = BulkCreateRequest<TunnelPayload>,
    responses(
        (status = 200, description = "Bulk tunnel create result", body = BulkCreateResponse<VpnTunnel>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_tunnels(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(request): EnvelopeJson<BulkCreateRequest<TunnelPayload>>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_create_inventory_access(&state, &request_context).await {
        return handle_app_error(error);
    }

    let items = match require_bulk_items(request.items) {
        Ok(items) => items,
        Err(error) => return handle_app_error(error),
    };

    let mut created = Vec::new();
    let mut errors = Vec::new();

    for (index, payload) in items.into_iter().enumerate() {
        let parsed = match parse_tunnel_payload(&payload) {
            Ok(value) => value,
            Err(error) => {
                push_bulk_error(&mut errors, index, error);
                continue;
            }
        };
        let item = VpnTunnel {
            id: Uuid::now_v7(),
            name: parsed.name,
            status: parsed.status,
            encapsulation: parsed.encapsulation,
            description: payload.description.unwrap_or_default(),
            comments: payload.comments.unwrap_or_default(),
            group_id: parsed.group_id,
            ipsec_profile_id: parsed.ipsec_profile_id,
            tunnel_id: payload.tunnel_id,
            tenant_id: parsed.tenant_id,
            ..Default::default()
        };

        match state.repositories.vpn_tunnels.create(&item).await {
            Ok(()) => created.push(item),
            Err(error) => push_bulk_error(&mut errors, index, error),
        }
    }

    if !created.is_empty() {
        bump_host_config_cache_generation(&state).await;
    }

    (StatusCode::OK, Json(BulkCreateResponse { created, errors })).into_response()
}

/// Update a tunnel.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/tunnels/{id}",
    operation_id = "update_inventory_tunnel",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Tunnel ID"),
    ),
    request_body = TunnelPayload,
    responses(
        (status = 200, description = "Tunnel updated", body = VpnTunnel),
        (status = 404, description = "Tunnel not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_tunnel(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<TunnelPayload>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Update,
        ResourceType::Inventory,
    )
    .await
    {
        return handle_app_error(error);
    }
    let entity_id = match parse_uuid(&id, "tunnel_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let parsed = match parse_tunnel_payload(&payload) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match state.repositories.vpn_tunnels.get_by_id(entity_id).await {
        Ok(None) => {
            return handle_app_error(AppError::NotFound("Tunnel not found".to_string()));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };

    let item = VpnTunnel {
        id: entity_id,
        name: parsed.name,
        status: parsed.status,
        encapsulation: parsed.encapsulation,
        description: payload.description.unwrap_or_default(),
        comments: payload.comments.unwrap_or_default(),
        group_id: parsed.group_id,
        ipsec_profile_id: parsed.ipsec_profile_id,
        tunnel_id: payload.tunnel_id,
        tenant_id: parsed.tenant_id,
        ..Default::default()
    };

    match state.repositories.vpn_tunnels.update(&item).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            (StatusCode::OK, Json(item)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Delete a tunnel.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/tunnels/{id}",
    operation_id = "delete_inventory_tunnel",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Tunnel ID"),
    ),
    responses(
        (status = 204, description = "Tunnel deleted"),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_tunnel(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Delete,
        ResourceType::Inventory,
    )
    .await
    {
        return handle_app_error(error);
    }
    let entity_id = match parse_uuid(&id, "tunnel_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state.repositories.vpn_tunnels.get_by_id(entity_id).await {
        Ok(None) => {
            return handle_app_error(AppError::NotFound("Tunnel not found".to_string()));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };
    match state.repositories.vpn_tunnels.delete(entity_id).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => handle_app_error(error),
    }
}
