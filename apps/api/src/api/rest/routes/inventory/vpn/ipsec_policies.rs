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
use crate::contexts::inventory::domain::VpnIpsecPolicy;
use crate::contexts::inventory::infrastructure::persistence::{
    VPN_IPSEC_POLICY_COLUMNS, VpnIpsecPolicyFilter, VpnIpsecPolicyRepository,
};
use crate::contexts::shared::list_query::SortSpec;
use crate::errors::AppError;

use super::super::{
    BulkCreateRequest, BulkCreateResponse, InventoryListEnvelope,
    bump_host_config_cache_generation, ensure_access, ensure_create_inventory_access,
    parse_required_string, parse_uuid, push_bulk_error, require_bulk_items,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};

#[derive(Debug, serde::Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct IpsecPolicyListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct IpsecPolicyPayload {
    pub name: String,
    pub pfs_group: Option<i16>,
    pub description: Option<String>,
    pub comments: Option<String>,
}

/// List IPsec policies.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/ipsec-policies",
    operation_id = "list_inventory_ipsec_policies",
    tag = "inventory",
    params(IpsecPolicyListQuery),
    responses(
        (status = 200, description = "IPSec policies", body = InventoryListEnvelope<VpnIpsecPolicy>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_ipsec_policies(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<IpsecPolicyListQuery>,
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
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        VPN_IPSEC_POLICY_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let resolved_limit = clamp_limit(query.limit);
    let resolved_offset = clamp_offset(query.offset);
    let filter = VpnIpsecPolicyFilter {
        search: crate::contexts::shared::list_query::search_pattern(query.search.as_deref()),
    };
    match state
        .repositories
        .vpn_ipsec_policies
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

/// Get an IPsec policy.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/ipsec-policies/{id}",
    operation_id = "get_inventory_ipsec_policy",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "IPSec policy ID"),
    ),
    responses(
        (status = 200, description = "IPSec policy", body = VpnIpsecPolicy),
        (status = 404, description = "IPSec policy not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_ipsec_policy(
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
    let entity_id = match parse_uuid(&id, "ipsec_policy_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .vpn_ipsec_policies
        .get_by_id(entity_id)
        .await
    {
        Ok(Some(item)) => (StatusCode::OK, Json(item)).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("IPSec policy not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create an IPsec policy.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/ipsec-policies",
    operation_id = "create_inventory_ipsec_policy",
    tag = "inventory",
    request_body = IpsecPolicyPayload,
    responses(
        (status = 201, description = "IPSec policy created", body = VpnIpsecPolicy),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_ipsec_policy(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<IpsecPolicyPayload>,
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
    let name = match parse_required_string(&payload.name, "name") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let item = VpnIpsecPolicy {
        id: Uuid::now_v7(),
        name,
        pfs_group: payload.pfs_group,
        description: payload.description.unwrap_or_default(),
        comments: payload.comments.unwrap_or_default(),
        ..Default::default()
    };
    match state.repositories.vpn_ipsec_policies.create(&item).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            (StatusCode::CREATED, Json(item)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create IPsec policies.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/ipsec-policies/bulk",
    operation_id = "bulk_create_inventory_ipsec_policies",
    tag = "inventory",
    request_body = BulkCreateRequest<IpsecPolicyPayload>,
    responses(
        (status = 200, description = "Bulk IPSec policy create result", body = BulkCreateResponse<VpnIpsecPolicy>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_ipsec_policies(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(request): EnvelopeJson<BulkCreateRequest<IpsecPolicyPayload>>,
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
        let name = match parse_required_string(&payload.name, "name") {
            Ok(value) => value,
            Err(error) => {
                push_bulk_error(&mut errors, index, error);
                continue;
            }
        };
        let item = VpnIpsecPolicy {
            id: Uuid::now_v7(),
            name,
            pfs_group: payload.pfs_group,
            description: payload.description.unwrap_or_default(),
            comments: payload.comments.unwrap_or_default(),
            ..Default::default()
        };

        match state.repositories.vpn_ipsec_policies.create(&item).await {
            Ok(()) => created.push(item),
            Err(error) => push_bulk_error(&mut errors, index, error),
        }
    }

    if !created.is_empty() {
        bump_host_config_cache_generation(&state).await;
    }

    (StatusCode::OK, Json(BulkCreateResponse { created, errors })).into_response()
}

/// Update an IPsec policy.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/ipsec-policies/{id}",
    operation_id = "update_inventory_ipsec_policy",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "IPSec policy ID"),
    ),
    request_body = IpsecPolicyPayload,
    responses(
        (status = 200, description = "IPSec policy updated", body = VpnIpsecPolicy),
        (status = 404, description = "IPSec policy not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_ipsec_policy(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<IpsecPolicyPayload>,
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
    let entity_id = match parse_uuid(&id, "ipsec_policy_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let name = match parse_required_string(&payload.name, "name") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match state
        .repositories
        .vpn_ipsec_policies
        .get_by_id(entity_id)
        .await
    {
        Ok(None) => {
            return handle_app_error(AppError::NotFound("IPSec policy not found".to_string()));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };

    let item = VpnIpsecPolicy {
        id: entity_id,
        name,
        pfs_group: payload.pfs_group,
        description: payload.description.unwrap_or_default(),
        comments: payload.comments.unwrap_or_default(),
        ..Default::default()
    };

    match state.repositories.vpn_ipsec_policies.update(&item).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            (StatusCode::OK, Json(item)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Delete an IPsec policy.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/ipsec-policies/{id}",
    operation_id = "delete_inventory_ipsec_policy",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "IPSec policy ID"),
    ),
    responses(
        (status = 204, description = "IPSec policy deleted"),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_ipsec_policy(
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
    let entity_id = match parse_uuid(&id, "ipsec_policy_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .vpn_ipsec_policies
        .get_by_id(entity_id)
        .await
    {
        Ok(None) => {
            return handle_app_error(AppError::NotFound("IPSec policy not found".to_string()));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .vpn_ipsec_policies
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
