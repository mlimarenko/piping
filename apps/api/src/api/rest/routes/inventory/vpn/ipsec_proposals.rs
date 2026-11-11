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
use crate::contexts::inventory::domain::VpnIpsecProposal;
use crate::contexts::inventory::infrastructure::persistence::{
    VPN_IPSEC_PROPOSAL_COLUMNS, VpnIpsecProposalFilter, VpnIpsecProposalRepository,
};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_strings};
use crate::errors::AppError;

use super::super::{
    BulkCreateRequest, BulkCreateResponse, InventoryListEnvelope,
    bump_host_config_cache_generation, ensure_access, ensure_create_inventory_access,
    parse_required_string, parse_uuid, push_bulk_error, require_bulk_items,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};

#[derive(Debug, serde::Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct IpsecProposalListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub encryption_algorithm: Option<String>,
    pub authentication_algorithm: Option<String>,
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct IpsecProposalPayload {
    pub name: String,
    pub encryption_algorithm: Option<String>,
    pub authentication_algorithm: Option<String>,
    pub sa_lifetime_seconds: Option<i32>,
    pub sa_lifetime_data: Option<i32>,
    pub description: Option<String>,
    pub comments: Option<String>,
}

/// List IPsec proposals.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/ipsec-proposals",
    operation_id = "list_inventory_ipsec_proposals",
    tag = "inventory",
    params(IpsecProposalListQuery),
    responses(
        (status = 200, description = "IPSec proposals", body = InventoryListEnvelope<VpnIpsecProposal>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_ipsec_proposals(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<IpsecProposalListQuery>,
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
        VPN_IPSEC_PROPOSAL_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let filter = VpnIpsecProposalFilter {
        search: crate::contexts::shared::list_query::search_pattern(query.search.as_deref()),
        encryption_algorithm: parse_csv_strings(query.encryption_algorithm.as_deref()),
        authentication_algorithm: parse_csv_strings(query.authentication_algorithm.as_deref()),
    };
    let resolved_limit = clamp_limit(query.limit);
    let resolved_offset = clamp_offset(query.offset);
    match state
        .repositories
        .vpn_ipsec_proposals
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

/// Get an IPsec proposal.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/ipsec-proposals/{id}",
    operation_id = "get_inventory_ipsec_proposal",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "IPSec proposal ID"),
    ),
    responses(
        (status = 200, description = "IPSec proposal", body = VpnIpsecProposal),
        (status = 404, description = "IPSec proposal not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_ipsec_proposal(
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
    let entity_id = match parse_uuid(&id, "ipsec_proposal_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .vpn_ipsec_proposals
        .get_by_id(entity_id)
        .await
    {
        Ok(Some(item)) => (StatusCode::OK, Json(item)).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("IPSec proposal not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create an IPsec proposal.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/ipsec-proposals",
    operation_id = "create_inventory_ipsec_proposal",
    tag = "inventory",
    request_body = IpsecProposalPayload,
    responses(
        (status = 201, description = "IPSec proposal created", body = VpnIpsecProposal),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_ipsec_proposal(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<IpsecProposalPayload>,
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
    let item = VpnIpsecProposal {
        id: Uuid::now_v7(),
        name,
        encryption_algorithm: payload.encryption_algorithm,
        authentication_algorithm: payload.authentication_algorithm,
        sa_lifetime_seconds: payload.sa_lifetime_seconds,
        sa_lifetime_data: payload.sa_lifetime_data,
        description: payload.description.unwrap_or_default(),
        comments: payload.comments.unwrap_or_default(),
        ..Default::default()
    };
    match state.repositories.vpn_ipsec_proposals.create(&item).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            (StatusCode::CREATED, Json(item)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create IPsec proposals.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/ipsec-proposals/bulk",
    operation_id = "bulk_create_inventory_ipsec_proposals",
    tag = "inventory",
    request_body = BulkCreateRequest<IpsecProposalPayload>,
    responses(
        (status = 200, description = "Bulk IPSec proposal create result", body = BulkCreateResponse<VpnIpsecProposal>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_ipsec_proposals(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(request): EnvelopeJson<BulkCreateRequest<IpsecProposalPayload>>,
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
        let item = VpnIpsecProposal {
            id: Uuid::now_v7(),
            name,
            encryption_algorithm: payload.encryption_algorithm,
            authentication_algorithm: payload.authentication_algorithm,
            sa_lifetime_seconds: payload.sa_lifetime_seconds,
            sa_lifetime_data: payload.sa_lifetime_data,
            description: payload.description.unwrap_or_default(),
            comments: payload.comments.unwrap_or_default(),
            ..Default::default()
        };

        match state.repositories.vpn_ipsec_proposals.create(&item).await {
            Ok(()) => created.push(item),
            Err(error) => push_bulk_error(&mut errors, index, error),
        }
    }

    if !created.is_empty() {
        bump_host_config_cache_generation(&state).await;
    }

    (StatusCode::OK, Json(BulkCreateResponse { created, errors })).into_response()
}

/// Update an IPsec proposal.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/ipsec-proposals/{id}",
    operation_id = "update_inventory_ipsec_proposal",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "IPSec proposal ID"),
    ),
    request_body = IpsecProposalPayload,
    responses(
        (status = 200, description = "IPSec proposal updated", body = VpnIpsecProposal),
        (status = 404, description = "IPSec proposal not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_ipsec_proposal(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<IpsecProposalPayload>,
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
    let entity_id = match parse_uuid(&id, "ipsec_proposal_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let name = match parse_required_string(&payload.name, "name") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match state
        .repositories
        .vpn_ipsec_proposals
        .get_by_id(entity_id)
        .await
    {
        Ok(None) => {
            return handle_app_error(AppError::NotFound("IPSec proposal not found".to_string()));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };

    let item = VpnIpsecProposal {
        id: entity_id,
        name,
        encryption_algorithm: payload.encryption_algorithm,
        authentication_algorithm: payload.authentication_algorithm,
        sa_lifetime_seconds: payload.sa_lifetime_seconds,
        sa_lifetime_data: payload.sa_lifetime_data,
        description: payload.description.unwrap_or_default(),
        comments: payload.comments.unwrap_or_default(),
        ..Default::default()
    };

    match state.repositories.vpn_ipsec_proposals.update(&item).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            (StatusCode::OK, Json(item)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Delete an IPsec proposal.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/ipsec-proposals/{id}",
    operation_id = "delete_inventory_ipsec_proposal",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "IPSec proposal ID"),
    ),
    responses(
        (status = 204, description = "IPSec proposal deleted"),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_ipsec_proposal(
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
    let entity_id = match parse_uuid(&id, "ipsec_proposal_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .vpn_ipsec_proposals
        .get_by_id(entity_id)
        .await
    {
        Ok(None) => {
            return handle_app_error(AppError::NotFound("IPSec proposal not found".to_string()));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .vpn_ipsec_proposals
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
