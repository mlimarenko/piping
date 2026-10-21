use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use uuid::Uuid;

use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::domain::ClusterGroup;
use crate::contexts::inventory::infrastructure::persistence::repositories::virtualization::cluster_group_repository::CLUSTER_GROUP_COLUMNS;
use crate::contexts::shared::list_query::SortSpec;
use crate::errors::AppError;

use super::super::{
    BulkCreateRequest, BulkCreateResponse, ClusterGroupPayload, InventoryListEnvelope,
    bump_host_config_cache_generation, ensure_access, ensure_create_inventory_access,
    parse_required_string, parse_uuid, push_bulk_error, require_bulk_items,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};

#[derive(Debug, serde::Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct ClusterGroupListQuery {
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub search: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

/// List cluster groups.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/cluster_groups",
    tag = "inventory",
    params(ClusterGroupListQuery),
    responses(
        (status = 200, description = "Cluster groups", body = InventoryListEnvelope<ClusterGroup>),
        (status = "4XX", description = "Cluster group list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Cluster group list failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_cluster_groups(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ClusterGroupListQuery>,
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
    let search = query
        .search
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| format!("%{s}%"));
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        CLUSTER_GROUP_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.inventory_admin.clone();
    match service
        .list_cluster_groups(
            crate::contexts::inventory::application::InventoryListQuery::new(
                query.limit,
                query.offset,
                search,
            )
            .with_sort(sort),
        )
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

/// Get a cluster group.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/cluster_groups/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Cluster group id"),
    ),
    responses(
        (status = 200, description = "Cluster group", body = ClusterGroup),
        (status = 404, description = "Cluster group not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Cluster group get request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Cluster group get failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_cluster_group(
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
    let cluster_group_id = match parse_uuid(&id, "cluster_group_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.inventory_admin.clone();
    match service.get_cluster_group(cluster_group_id).await {
        Ok(Some(group)) => (StatusCode::OK, Json(group)).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Cluster group not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a cluster group.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/cluster_groups",
    tag = "inventory",
    request_body = ClusterGroupPayload,
    responses(
        (status = 201, description = "Cluster group created", body = ClusterGroup),
        (status = "4XX", description = "Cluster group create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Cluster group create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_cluster_group(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<ClusterGroupPayload>,
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
    let slug = match parse_required_string(&payload.slug, "slug") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let group = ClusterGroup {
        id: Uuid::now_v7(),
        external_id: None,
        name,
        slug,
        description: payload.description,
        piping_synced_at: None,
        ..Default::default()
    };
    let service = state.services.inventory_admin.clone();
    match service.create_cluster_group(group.clone()).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            (StatusCode::CREATED, Json(group)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create cluster groups.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/cluster_groups/bulk",
    tag = "inventory",
    request_body = BulkCreateRequest<ClusterGroupPayload>,
    responses(
        (status = 200, description = "Bulk cluster group create result", body = BulkCreateResponse<ClusterGroup>),
        (status = "4XX", description = "Bulk cluster group create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Bulk cluster group create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_cluster_groups(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(request): EnvelopeJson<BulkCreateRequest<ClusterGroupPayload>>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_create_inventory_access(&state, &request_context).await {
        return handle_app_error(error);
    }

    let items = match require_bulk_items(request.items) {
        Ok(items) => items,
        Err(error) => return handle_app_error(error),
    };

    let service = state.services.inventory_admin.clone();
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
        let slug = match parse_required_string(&payload.slug, "slug") {
            Ok(value) => value,
            Err(error) => {
                push_bulk_error(&mut errors, index, error);
                continue;
            }
        };
        let group = ClusterGroup {
            id: Uuid::now_v7(),
            external_id: None,
            name,
            slug,
            description: payload.description,
            piping_synced_at: None,
            ..Default::default()
        };

        match service.create_cluster_group(group.clone()).await {
            Ok(()) => created.push(group),
            Err(error) => push_bulk_error(&mut errors, index, error),
        }
    }

    if !created.is_empty() {
        bump_host_config_cache_generation(&state).await;
    }

    (StatusCode::OK, Json(BulkCreateResponse { created, errors })).into_response()
}

/// Update a cluster group.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/cluster_groups/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Cluster group id"),
    ),
    request_body = ClusterGroupPayload,
    responses(
        (status = 200, description = "Cluster group updated", body = ClusterGroup),
        (status = 404, description = "Cluster group not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Cluster group update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Cluster group update failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_cluster_group(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<ClusterGroupPayload>,
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
    let cluster_group_id = match parse_uuid(&id, "cluster_group_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let name = match parse_required_string(&payload.name, "name") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let slug = match parse_required_string(&payload.slug, "slug") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let service = state.services.inventory_admin.clone();
    match service.get_cluster_group(cluster_group_id).await {
        Ok(None) => {
            return handle_app_error(AppError::NotFound("Cluster group not found".to_string()));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };

    let group = ClusterGroup {
        id: cluster_group_id,
        external_id: None,
        name,
        slug,
        description: payload.description,
        piping_synced_at: None,
        ..Default::default()
    };

    match service.update_cluster_group(group.clone()).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            (StatusCode::OK, Json(group)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Delete a cluster group.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/cluster_groups/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Cluster group id"),
    ),
    responses(
        (status = 204, description = "Cluster group deleted"),
        (status = 404, description = "Cluster group not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Cluster group delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Cluster group delete failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_cluster_group(
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
    let cluster_group_id = match parse_uuid(&id, "cluster_group_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.inventory_admin.clone();
    match service.get_cluster_group(cluster_group_id).await {
        Ok(None) => {
            return handle_app_error(AppError::NotFound("Cluster group not found".to_string()));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };
    match service.delete_cluster_group(cluster_group_id).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => handle_app_error(error),
    }
}
