//! Provider Networks REST routes — full CRUD + bulk.
//!
//! Handlers never run SQL directly; all persistence goes through
//! `ProviderNetworkRepository` on `AppState`.

use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::infrastructure::persistence::{
    BulkProviderNetworkResult, NewProviderNetwork, PROVIDER_NETWORK_COLUMNS,
    ProviderNetworkListItem, ProviderNetworkRecord, ProviderNetworkRepository, SortSpec,
    UpdateProviderNetwork, parse_csv_uuids,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

use super::super::{
    InventoryListEnvelope, ensure_access, parse_optional_uuid, parse_required_string, parse_uuid,
};

/// Query parameters for provider network listing.
#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct ProviderNetworkListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    pub provider_id: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ProviderNetworkCreatePayload {
    pub provider_id: String,
    pub name: String,
    pub service_id: Option<String>,
    pub description: Option<String>,
    pub comments: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ProviderNetworkUpdatePayload {
    pub provider_id: Option<String>,
    pub name: Option<String>,
    pub service_id: Option<String>,
    pub description: Option<String>,
    pub comments: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ProviderNetworkBulkCreatePayload {
    #[serde(deserialize_with = "crate::api::rest::routes::validation::deserialize_bulk_items")]
    #[schema(min_items = 1)]
    pub items: Vec<ProviderNetworkCreatePayload>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ProviderNetworkResponse {
    pub id: Uuid,
    pub provider_id: Uuid,
    pub name: String,
    pub service_id: String,
    pub description: String,
    pub comments: String,
}

/// Hydrated provider network list item — extends the base response with the
/// provider name.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ProviderNetworkListItemResponse {
    pub id: Uuid,
    pub provider_id: Uuid,
    pub provider_name: Option<String>,
    pub name: String,
    pub service_id: String,
    pub description: String,
    pub comments: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkProviderNetworkResponse {
    pub created: Vec<ProviderNetworkResponse>,
    pub errors: Vec<String>,
}

fn map_provider_network_list_item(
    item: ProviderNetworkListItem,
) -> ProviderNetworkListItemResponse {
    let ProviderNetworkListItem {
        record,
        provider_name,
    } = item;
    ProviderNetworkListItemResponse {
        id: record.id,
        provider_id: record.provider_id,
        provider_name,
        name: record.name,
        service_id: record.service_id,
        description: record.description,
        comments: record.comments,
    }
}

fn map_provider_network(record: ProviderNetworkRecord) -> ProviderNetworkResponse {
    ProviderNetworkResponse {
        id: record.id,
        provider_id: record.provider_id,
        name: record.name,
        service_id: record.service_id,
        description: record.description,
        comments: record.comments,
    }
}

fn map_bulk_provider_network_result(
    result: BulkProviderNetworkResult,
) -> BulkProviderNetworkResponse {
    BulkProviderNetworkResponse {
        created: result
            .created
            .into_iter()
            .map(map_provider_network)
            .collect(),
        errors: result.errors,
    }
}

/// List provider networks.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/provider-networks",
    tag = "inventory",
    params(ProviderNetworkListQuery),
    responses(
        (status = 200, description = "Provider Networks", body = InventoryListEnvelope<ProviderNetworkListItemResponse>),
        (status = "4XX", description = "Provider network list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Provider network list failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_provider_networks(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ProviderNetworkListQuery>,
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

    let provider_id = match parse_csv_uuids(query.provider_id.as_deref(), "provider_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        PROVIDER_NETWORK_COLUMNS,
    ) {
        Ok(sort) => sort,
        Err(error) => return handle_app_error(error),
    };
    let resolved_limit = clamp_limit(query.limit);
    let resolved_offset = clamp_offset(query.offset);
    match state
        .repositories
        .provider_networks
        .list_hydrated(
            provider_id,
            crate::contexts::shared::list_query::search_pattern(query.search.as_deref()),
            sort,
            query.limit,
            query.offset,
        )
        .await
    {
        Ok((records, total)) => {
            let items: Vec<ProviderNetworkListItemResponse> = records
                .into_iter()
                .map(map_provider_network_list_item)
                .collect();
            (
                StatusCode::OK,
                Json(InventoryListEnvelope {
                    items,
                    total,
                    limit: resolved_limit,
                    offset: resolved_offset,
                }),
            )
                .into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Get a provider network.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/provider-networks/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Provider Network id"),
    ),
    responses(
        (status = 200, description = "Provider Network", body = ProviderNetworkResponse),
        (status = 404, description = "Provider Network not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Provider network get request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Provider network get failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_provider_network(
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
    let id = match parse_uuid(&id, "id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state.repositories.provider_networks.get_by_id(id).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_provider_network(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Provider Network not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a provider network.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/provider-networks",
    tag = "inventory",
    request_body = ProviderNetworkCreatePayload,
    responses(
        (status = 201, description = "Provider Network created", body = ProviderNetworkResponse),
        (status = "4XX", description = "Provider network create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Provider network create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_provider_network(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<ProviderNetworkCreatePayload>,
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
    let provider_id = match parse_uuid(&payload.provider_id, "provider_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let new_provider_network = NewProviderNetwork {
        id: Uuid::now_v7(),
        provider_id,
        name,
        service_id: payload.service_id,
        description: payload.description,
        comments: payload.comments,
    };

    match state
        .repositories
        .provider_networks
        .create(&new_provider_network)
        .await
    {
        Ok(record) => (StatusCode::CREATED, Json(map_provider_network(record))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update a provider network.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/provider-networks/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Provider Network id"),
    ),
    request_body = ProviderNetworkUpdatePayload,
    responses(
        (status = 200, description = "Provider Network updated", body = ProviderNetworkResponse),
        (status = 404, description = "Provider Network not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Provider network update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Provider network update failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_provider_network(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<ProviderNetworkUpdatePayload>,
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
    let id = match parse_uuid(&id, "id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let provider_id = match parse_optional_uuid(payload.provider_id.clone(), "provider_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let changes = UpdateProviderNetwork {
        provider_id,
        name: payload.name,
        service_id: payload.service_id,
        description: payload.description,
        comments: payload.comments,
    };

    match state
        .repositories
        .provider_networks
        .update(id, &changes)
        .await
    {
        Ok(Some(record)) => (StatusCode::OK, Json(map_provider_network(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Provider Network not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a provider network.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/provider-networks/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Provider Network id"),
    ),
    responses(
        (status = 204, description = "Provider Network deleted"),
        (status = "4XX", description = "Provider network delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Provider network delete failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_provider_network(
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
    let id = match parse_uuid(&id, "id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state.repositories.provider_networks.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create provider networks.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/provider-networks/bulk",
    tag = "inventory",
    request_body = ProviderNetworkBulkCreatePayload,
    responses(
        (status = 200, description = "Bulk provider network create result", body = BulkProviderNetworkResponse),
        (status = "4XX", description = "Bulk provider network create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Bulk provider network create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_provider_networks(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<ProviderNetworkBulkCreatePayload>,
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

    let mut items = Vec::new();
    for (index, p) in payload.items.into_iter().enumerate() {
        let name = match parse_required_string(&p.name, "name") {
            Ok(value) => value,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        let provider_id = match parse_uuid(&p.provider_id, "provider_id") {
            Ok(value) => value,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        items.push(NewProviderNetwork {
            id: Uuid::now_v7(),
            provider_id,
            name,
            service_id: p.service_id,
            description: p.description,
            comments: p.comments,
        });
    }

    match state
        .repositories
        .provider_networks
        .bulk_create(&items)
        .await
    {
        Ok(result) => (
            StatusCode::OK,
            Json(map_bulk_provider_network_result(result)),
        )
            .into_response(),
        Err(error) => handle_app_error(error),
    }
}
