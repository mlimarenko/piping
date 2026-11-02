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
use crate::contexts::inventory::domain::VpnIpsecProfile;
use crate::contexts::inventory::infrastructure::persistence::{
    VPN_IPSEC_PROFILE_COLUMNS, VpnIpsecProfileFilter, VpnIpsecProfileRepository,
};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_strings, parse_csv_uuids};
use crate::errors::AppError;

use super::super::{
    BulkCreateRequest, BulkCreateResponse, InventoryListEnvelope,
    bump_host_config_cache_generation, ensure_access, ensure_create_inventory_access,
    parse_required_string, parse_uuid, push_bulk_error, require_bulk_items,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};

#[derive(Debug, serde::Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct IpsecProfileListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub mode: Option<String>,
    pub ike_policy_id: Option<String>,
    pub ipsec_policy_id: Option<String>,
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct IpsecProfilePayload {
    pub name: String,
    pub mode: String,
    pub ike_policy_id: String,
    pub ipsec_policy_id: String,
    pub description: Option<String>,
    pub comments: Option<String>,
}

/// List IPsec profiles.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/ipsec-profiles",
    operation_id = "list_inventory_ipsec_profiles",
    tag = "inventory",
    params(IpsecProfileListQuery),
    responses(
        (status = 200, description = "IPSec profiles", body = InventoryListEnvelope<VpnIpsecProfile>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_ipsec_profiles(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<IpsecProfileListQuery>,
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
        VPN_IPSEC_PROFILE_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let ike_policy_id = match parse_csv_uuids(query.ike_policy_id.as_deref(), "ike_policy_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let ipsec_policy_id = match parse_csv_uuids(query.ipsec_policy_id.as_deref(), "ipsec_policy_id")
    {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let filter = VpnIpsecProfileFilter {
        search: crate::contexts::shared::list_query::search_pattern(query.search.as_deref()),
        mode: parse_csv_strings(query.mode.as_deref()),
        ike_policy_id,
        ipsec_policy_id,
    };
    let resolved_limit = clamp_limit(query.limit);
    let resolved_offset = clamp_offset(query.offset);
    match state
        .repositories
        .vpn_ipsec_profiles
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

/// Get an IPsec profile.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/ipsec-profiles/{id}",
    operation_id = "get_inventory_ipsec_profile",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "IPSec profile ID"),
    ),
    responses(
        (status = 200, description = "IPSec profile", body = VpnIpsecProfile),
        (status = 404, description = "IPSec profile not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_ipsec_profile(
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
    let entity_id = match parse_uuid(&id, "ipsec_profile_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .vpn_ipsec_profiles
        .get_by_id(entity_id)
        .await
    {
        Ok(Some(item)) => (StatusCode::OK, Json(item)).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("IPSec profile not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create an IPsec profile.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/ipsec-profiles",
    operation_id = "create_inventory_ipsec_profile",
    tag = "inventory",
    request_body = IpsecProfilePayload,
    responses(
        (status = 201, description = "IPSec profile created", body = VpnIpsecProfile),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_ipsec_profile(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<IpsecProfilePayload>,
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
    let mode = match parse_required_string(&payload.mode, "mode") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let ike_policy_id = match parse_uuid(&payload.ike_policy_id, "ike_policy_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let ipsec_policy_id = match parse_uuid(&payload.ipsec_policy_id, "ipsec_policy_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let item = VpnIpsecProfile {
        id: Uuid::now_v7(),
        name,
        mode,
        ike_policy_id,
        ipsec_policy_id,
        description: payload.description.unwrap_or_default(),
        comments: payload.comments.unwrap_or_default(),
        ..Default::default()
    };
    match state.repositories.vpn_ipsec_profiles.create(&item).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            (StatusCode::CREATED, Json(item)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create IPsec profiles.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/ipsec-profiles/bulk",
    operation_id = "bulk_create_inventory_ipsec_profiles",
    tag = "inventory",
    request_body = BulkCreateRequest<IpsecProfilePayload>,
    responses(
        (status = 200, description = "Bulk IPSec profile create result", body = BulkCreateResponse<VpnIpsecProfile>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_ipsec_profiles(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(request): EnvelopeJson<BulkCreateRequest<IpsecProfilePayload>>,
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
        let mode = match parse_required_string(&payload.mode, "mode") {
            Ok(value) => value,
            Err(error) => {
                push_bulk_error(&mut errors, index, error);
                continue;
            }
        };
        let ike_policy_id = match parse_uuid(&payload.ike_policy_id, "ike_policy_id") {
            Ok(value) => value,
            Err(error) => {
                push_bulk_error(&mut errors, index, error);
                continue;
            }
        };
        let ipsec_policy_id = match parse_uuid(&payload.ipsec_policy_id, "ipsec_policy_id") {
            Ok(value) => value,
            Err(error) => {
                push_bulk_error(&mut errors, index, error);
                continue;
            }
        };
        let item = VpnIpsecProfile {
            id: Uuid::now_v7(),
            name,
            mode,
            ike_policy_id,
            ipsec_policy_id,
            description: payload.description.unwrap_or_default(),
            comments: payload.comments.unwrap_or_default(),
            ..Default::default()
        };

        match state.repositories.vpn_ipsec_profiles.create(&item).await {
            Ok(()) => created.push(item),
            Err(error) => push_bulk_error(&mut errors, index, error),
        }
    }

    if !created.is_empty() {
        bump_host_config_cache_generation(&state).await;
    }

    (StatusCode::OK, Json(BulkCreateResponse { created, errors })).into_response()
}

/// Update an IPsec profile.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/ipsec-profiles/{id}",
    operation_id = "update_inventory_ipsec_profile",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "IPSec profile ID"),
    ),
    request_body = IpsecProfilePayload,
    responses(
        (status = 200, description = "IPSec profile updated", body = VpnIpsecProfile),
        (status = 404, description = "IPSec profile not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_ipsec_profile(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<IpsecProfilePayload>,
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
    let entity_id = match parse_uuid(&id, "ipsec_profile_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let name = match parse_required_string(&payload.name, "name") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let mode = match parse_required_string(&payload.mode, "mode") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let ike_policy_id = match parse_uuid(&payload.ike_policy_id, "ike_policy_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let ipsec_policy_id = match parse_uuid(&payload.ipsec_policy_id, "ipsec_policy_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match state
        .repositories
        .vpn_ipsec_profiles
        .get_by_id(entity_id)
        .await
    {
        Ok(None) => {
            return handle_app_error(AppError::NotFound("IPSec profile not found".to_string()));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };

    let item = VpnIpsecProfile {
        id: entity_id,
        name,
        mode,
        ike_policy_id,
        ipsec_policy_id,
        description: payload.description.unwrap_or_default(),
        comments: payload.comments.unwrap_or_default(),
        ..Default::default()
    };

    match state.repositories.vpn_ipsec_profiles.update(&item).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            (StatusCode::OK, Json(item)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Delete an IPsec profile.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/ipsec-profiles/{id}",
    operation_id = "delete_inventory_ipsec_profile",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "IPSec profile ID"),
    ),
    responses(
        (status = 204, description = "IPSec profile deleted"),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_ipsec_profile(
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
    let entity_id = match parse_uuid(&id, "ipsec_profile_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .vpn_ipsec_profiles
        .get_by_id(entity_id)
        .await
    {
        Ok(None) => {
            return handle_app_error(AppError::NotFound("IPSec profile not found".to_string()));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .vpn_ipsec_profiles
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
