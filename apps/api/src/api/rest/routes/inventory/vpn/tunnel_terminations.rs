use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use uuid::Uuid;

use serde::{Deserialize, Serialize};

use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::domain::content_type::TUNNEL_TERMINATION_OBJECT_TYPES;
use crate::contexts::inventory::domain::{ObjectRef, VpnTunnelTermination};
use crate::contexts::inventory::infrastructure::persistence::{
    SqlxObjectRefResolver, VPN_TUNNEL_TERMINATION_COLUMNS, VpnTunnelTerminationFilter,
    VpnTunnelTerminationRepository,
};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_strings, parse_csv_uuids};
use crate::errors::AppError;

use super::super::{
    BulkCreateRequest, BulkCreateResponse, InventoryListEnvelope, ObjectRefInput,
    bump_host_config_cache_generation, ensure_access, ensure_create_inventory_access,
    parse_optional_uuid, parse_required_string, parse_uuid, push_bulk_error, require_bulk_items,
    resolve_object_type,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};

#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct TunnelTerminationListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub role: Option<String>,
    pub tunnel_id: Option<String>,
    pub termination_type_id: Option<String>,
    pub termination_id: Option<String>,
    pub outside_ip_id: Option<String>,
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TunnelTerminationPayload {
    pub role: String,
    pub tunnel_id: String,
    /// Canonical polymorphic termination: `{ object_type: "app_label.model",
    /// object_id }`. `null`/absent leaves the termination unset. Allowed
    /// targets: `dcim.interface`, `virtualization.vminterface`.
    pub termination: Option<ObjectRefInput>,
    pub outside_ip_id: Option<String>,
}

/// Response projection for a tunnel termination.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TunnelTerminationResponse {
    pub id: Uuid,
    pub role: String,
    pub tunnel_id: Uuid,
    pub tunnel_name: Option<String>,
    /// Materialized polymorphic termination, or `null` when unset.
    pub termination: Option<ObjectRef>,
    pub outside_ip_id: Option<Uuid>,
    pub outside_ip: Option<String>,
    pub custom_fields: Option<serde_json::Value>,
}

struct ParsedTermination {
    role: String,
    tunnel_id: Uuid,
    termination_type_id: Uuid,
    termination_id: Option<Uuid>,
    outside_ip_id: Option<Uuid>,
}

/// Validate + resolve a tunnel-termination payload. The polymorphic
/// `termination_type_id` is NOT NULL on the table, so a termination object is
/// required; only the `termination_id` half (the target row) may be null.
async fn parse_termination_payload(
    state: &AppState,
    payload: &TunnelTerminationPayload,
) -> Result<ParsedTermination, AppError> {
    let termination = payload.termination.as_ref().ok_or_else(|| {
        AppError::Validation("termination is required (object_type + object_id)".to_string())
    })?;
    let termination_type_id = resolve_object_type(
        &state.db_pool,
        &termination.object_type,
        TUNNEL_TERMINATION_OBJECT_TYPES,
    )
    .await?;
    Ok(ParsedTermination {
        role: parse_required_string(&payload.role, "role")?,
        tunnel_id: parse_uuid(&payload.tunnel_id, "tunnel_id")?,
        termination_type_id,
        termination_id: Some(termination.object_id),
        outside_ip_id: parse_optional_uuid(payload.outside_ip_id.clone(), "outside_ip_id")?,
    })
}

/// Resolve a single termination's pair into a response.
async fn map_one_termination(
    resolver: &SqlxObjectRefResolver,
    item: VpnTunnelTermination,
) -> Result<TunnelTerminationResponse, AppError> {
    let termination = match item.termination_id {
        Some(object_id) => Some(
            resolver
                .resolve_one(item.termination_type_id, object_id)
                .await?,
        ),
        None => None,
    };
    Ok(TunnelTerminationResponse {
        id: item.id,
        role: item.role,
        tunnel_id: item.tunnel_id,
        tunnel_name: item.tunnel_name,
        termination,
        outside_ip_id: item.outside_ip_id,
        outside_ip: item.outside_ip,
        custom_fields: item.custom_fields,
    })
}

/// Batch-resolve a page of terminations (one resolver round-trip, no N+1).
async fn map_terminations(
    resolver: &SqlxObjectRefResolver,
    items: Vec<VpnTunnelTermination>,
) -> Result<Vec<TunnelTerminationResponse>, AppError> {
    let pairs: Vec<(Uuid, Uuid)> = items
        .iter()
        .filter_map(|item| {
            item.termination_id
                .map(|oid| (item.termination_type_id, oid))
        })
        .collect();
    let refs = resolver.resolve(&pairs).await?;
    Ok(items
        .into_iter()
        .map(|item| {
            let termination = item.termination_id.map(|object_id| {
                refs.get(&(item.termination_type_id, object_id))
                    .cloned()
                    .unwrap_or(ObjectRef {
                        object_type: String::new(),
                        object_id,
                        label: None,
                    })
            });
            TunnelTerminationResponse {
                id: item.id,
                role: item.role,
                tunnel_id: item.tunnel_id,
                tunnel_name: item.tunnel_name,
                termination,
                outside_ip_id: item.outside_ip_id,
                outside_ip: item.outside_ip,
                custom_fields: item.custom_fields,
            }
        })
        .collect())
}

/// List tunnel terminations.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/tunnel-terminations",
    operation_id = "list_inventory_tunnel_terminations",
    tag = "inventory",
    params(TunnelTerminationListQuery),
    responses(
        (status = 200, description = "Tunnel terminations", body = InventoryListEnvelope<TunnelTerminationResponse>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_tunnel_terminations(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<TunnelTerminationListQuery>,
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
        VPN_TUNNEL_TERMINATION_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let tunnel_id = match parse_csv_uuids(query.tunnel_id.as_deref(), "tunnel_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let termination_type_id =
        match parse_csv_uuids(query.termination_type_id.as_deref(), "termination_type_id") {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };
    let termination_id = match parse_csv_uuids(query.termination_id.as_deref(), "termination_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let outside_ip_id = match parse_csv_uuids(query.outside_ip_id.as_deref(), "outside_ip_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let filter = VpnTunnelTerminationFilter {
        search: crate::contexts::shared::list_query::search_pattern(query.search.as_deref()),
        role: parse_csv_strings(query.role.as_deref()),
        tunnel_id,
        termination_type_id,
        termination_id,
        outside_ip_id,
    };
    match state
        .repositories
        .vpn_tunnel_terminations
        .list_paginated(query.limit, query.offset, &filter, sort)
        .await
    {
        Ok((items, total)) => {
            match map_terminations(&state.repositories.object_ref_resolver, items).await {
                Ok(items) => (
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
        Err(error) => handle_app_error(error),
    }
}

/// Get a tunnel termination.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/tunnel-terminations/{id}",
    operation_id = "get_inventory_tunnel_termination",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Tunnel termination ID"),
    ),
    responses(
        (status = 200, description = "Tunnel termination", body = TunnelTerminationResponse),
        (status = 404, description = "Tunnel termination not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_tunnel_termination(
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
    let entity_id = match parse_uuid(&id, "tunnel_termination_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .vpn_tunnel_terminations
        .get_by_id(entity_id)
        .await
    {
        Ok(Some(item)) => {
            match map_one_termination(&state.repositories.object_ref_resolver, item).await {
                Ok(body) => (StatusCode::OK, Json(body)).into_response(),
                Err(error) => handle_app_error(error),
            }
        }
        Ok(None) => handle_app_error(AppError::NotFound(
            "Tunnel termination not found".to_string(),
        )),
        Err(error) => handle_app_error(error),
    }
}

/// Create a tunnel termination.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/tunnel-terminations",
    operation_id = "create_inventory_tunnel_termination",
    tag = "inventory",
    request_body = TunnelTerminationPayload,
    responses(
        (status = 201, description = "Tunnel termination created", body = TunnelTerminationResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_tunnel_termination(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<TunnelTerminationPayload>,
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
    let parsed = match parse_termination_payload(&state, &payload).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let item = VpnTunnelTermination {
        id: Uuid::now_v7(),
        role: parsed.role,
        tunnel_id: parsed.tunnel_id,
        termination_type_id: parsed.termination_type_id,
        termination_id: parsed.termination_id,
        outside_ip_id: parsed.outside_ip_id,
        ..Default::default()
    };
    match state
        .repositories
        .vpn_tunnel_terminations
        .create(&item)
        .await
    {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            match map_one_termination(&state.repositories.object_ref_resolver, item).await {
                Ok(body) => (StatusCode::CREATED, Json(body)).into_response(),
                Err(error) => handle_app_error(error),
            }
        }
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create tunnel terminations.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/tunnel-terminations/bulk",
    operation_id = "bulk_create_inventory_tunnel_terminations",
    tag = "inventory",
    request_body = BulkCreateRequest<TunnelTerminationPayload>,
    responses(
        (status = 200, description = "Bulk tunnel termination create result", body = BulkCreateResponse<TunnelTerminationResponse>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_tunnel_terminations(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(request): EnvelopeJson<BulkCreateRequest<TunnelTerminationPayload>>,
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
        let parsed = match parse_termination_payload(&state, &payload).await {
            Ok(value) => value,
            Err(error) => {
                push_bulk_error(&mut errors, index, error);
                continue;
            }
        };
        let item = VpnTunnelTermination {
            id: Uuid::now_v7(),
            role: parsed.role,
            tunnel_id: parsed.tunnel_id,
            termination_type_id: parsed.termination_type_id,
            termination_id: parsed.termination_id,
            outside_ip_id: parsed.outside_ip_id,
            ..Default::default()
        };

        match state
            .repositories
            .vpn_tunnel_terminations
            .create(&item)
            .await
        {
            Ok(()) => created.push(item),
            Err(error) => push_bulk_error(&mut errors, index, error),
        }
    }

    if !created.is_empty() {
        bump_host_config_cache_generation(&state).await;
    }

    let created = match map_terminations(&state.repositories.object_ref_resolver, created).await {
        Ok(created) => created,
        Err(error) => return handle_app_error(error),
    };

    (StatusCode::OK, Json(BulkCreateResponse { created, errors })).into_response()
}

/// Update a tunnel termination.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/tunnel-terminations/{id}",
    operation_id = "update_inventory_tunnel_termination",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Tunnel termination ID"),
    ),
    request_body = TunnelTerminationPayload,
    responses(
        (status = 200, description = "Tunnel termination updated", body = TunnelTerminationResponse),
        (status = 404, description = "Tunnel termination not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_tunnel_termination(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<TunnelTerminationPayload>,
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
    let entity_id = match parse_uuid(&id, "tunnel_termination_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let parsed = match parse_termination_payload(&state, &payload).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match state
        .repositories
        .vpn_tunnel_terminations
        .get_by_id(entity_id)
        .await
    {
        Ok(None) => {
            return handle_app_error(AppError::NotFound(
                "Tunnel termination not found".to_string(),
            ));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };

    let item = VpnTunnelTermination {
        id: entity_id,
        role: parsed.role,
        tunnel_id: parsed.tunnel_id,
        termination_type_id: parsed.termination_type_id,
        termination_id: parsed.termination_id,
        outside_ip_id: parsed.outside_ip_id,
        ..Default::default()
    };

    match state
        .repositories
        .vpn_tunnel_terminations
        .update(&item)
        .await
    {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            match map_one_termination(&state.repositories.object_ref_resolver, item).await {
                Ok(body) => (StatusCode::OK, Json(body)).into_response(),
                Err(error) => handle_app_error(error),
            }
        }
        Err(error) => handle_app_error(error),
    }
}

/// Delete a tunnel termination.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/tunnel-terminations/{id}",
    operation_id = "delete_inventory_tunnel_termination",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Tunnel termination ID"),
    ),
    responses(
        (status = 204, description = "Tunnel termination deleted"),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_tunnel_termination(
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
    let entity_id = match parse_uuid(&id, "tunnel_termination_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .vpn_tunnel_terminations
        .get_by_id(entity_id)
        .await
    {
        Ok(None) => {
            return handle_app_error(AppError::NotFound(
                "Tunnel termination not found".to_string(),
            ));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .vpn_tunnel_terminations
        .delete(entity_id)
        .await
    {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => handle_app_error(error),
    }
}
