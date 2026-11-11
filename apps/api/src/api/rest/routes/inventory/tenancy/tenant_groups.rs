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
use crate::contexts::inventory::domain::TenantGroup;
use crate::contexts::inventory::infrastructure::persistence::repositories::tenancy::tenant_group_repository::TENANT_GROUP_COLUMNS;
use crate::contexts::shared::list_query::{SortSpec, parse_csv_uuids};
use crate::errors::AppError;

use super::super::{
    BulkCreateRequest, BulkCreateResponse, InventoryListEnvelope, TenantGroupPayload,
    bump_host_config_cache_generation, ensure_access, ensure_create_inventory_access,
    parse_optional_uuid, parse_required_string, parse_uuid, push_bulk_error_with_hint,
    require_bulk_items,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};

#[derive(Debug, serde::Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct TenantGroupListQuery {
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub search: Option<String>,
    pub parent_id: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

/// List tenant groups.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/tenant_groups",
    tag = "inventory",
    params(TenantGroupListQuery),
    responses(
        (status = 200, description = "Tenant groups", body = InventoryListEnvelope<TenantGroup>),
        (status = "4XX", description = "Tenant group list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Tenant group list failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_tenant_groups(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<TenantGroupListQuery>,
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
        TENANT_GROUP_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let parent_id = match parse_csv_uuids(query.parent_id.as_deref(), "parent_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.inventory_admin.clone();
    match service
        .list_tenant_groups(
            crate::contexts::inventory::application::InventoryListQuery::new(
                query.limit,
                query.offset,
                search,
            )
            .with_parent_id(parent_id)
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

/// Get a tenant group.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/tenant_groups/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Tenant group id"),
    ),
    responses(
        (status = 200, description = "Tenant group", body = TenantGroup),
        (status = 404, description = "Tenant group not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Tenant group get request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Tenant group get failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_tenant_group(
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
    let tenant_group_id = match parse_uuid(&id, "tenant_group_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.inventory_admin.clone();
    match service.get_tenant_group(tenant_group_id).await {
        Ok(Some(group)) => (StatusCode::OK, Json(group)).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Tenant group not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a tenant group.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/tenant_groups",
    tag = "inventory",
    request_body = TenantGroupPayload,
    responses(
        (status = 201, description = "Tenant group created", body = TenantGroup),
        (status = "4XX", description = "Tenant group create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Tenant group create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_tenant_group(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<TenantGroupPayload>,
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
    let parent_id = match parse_optional_uuid(payload.parent_id.clone(), "parent_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let tenant_group = TenantGroup {
        id: Uuid::now_v7(),
        external_id: None,
        name,
        slug: payload.slug,
        parent_id,
        description: payload.description,
        piping_synced_at: None,
        ..Default::default()
    };
    let service = state.services.inventory_admin.clone();
    match service.create_tenant_group(tenant_group.clone()).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            (StatusCode::CREATED, Json(tenant_group)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create tenant groups.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/tenant_groups/bulk",
    tag = "inventory",
    request_body = BulkCreateRequest<TenantGroupPayload>,
    responses(
        (status = 200, description = "Bulk tenant group create result", body = BulkCreateResponse<TenantGroup>),
        (status = "4XX", description = "Bulk tenant group create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Bulk tenant group create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_tenant_groups(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(request): EnvelopeJson<BulkCreateRequest<TenantGroupPayload>>,
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
        // Capture the original `name` (may be empty/whitespace) for `key_hint`
        // so the caller can correlate failures back to their input even when
        // validation rejects the name.
        let key_hint = Some(payload.name.clone());
        let name = match parse_required_string(&payload.name, "name") {
            Ok(value) => value,
            Err(error) => {
                push_bulk_error_with_hint(&mut errors, index, key_hint, error);
                continue;
            }
        };
        let parent_id = match parse_optional_uuid(payload.parent_id.clone(), "parent_id") {
            Ok(value) => value,
            Err(error) => {
                push_bulk_error_with_hint(&mut errors, index, key_hint, error);
                continue;
            }
        };
        let tenant_group = TenantGroup {
            id: Uuid::now_v7(),
            external_id: None,
            name,
            slug: payload.slug,
            parent_id,
            description: payload.description,
            piping_synced_at: None,
            ..Default::default()
        };

        match service.create_tenant_group(tenant_group.clone()).await {
            Ok(()) => created.push(tenant_group),
            Err(error) => push_bulk_error_with_hint(&mut errors, index, key_hint, error),
        }
    }

    if !created.is_empty() {
        bump_host_config_cache_generation(&state).await;
    }

    (StatusCode::OK, Json(BulkCreateResponse { created, errors })).into_response()
}

/// Update a tenant group.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/tenant_groups/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Tenant group id"),
    ),
    request_body = TenantGroupPayload,
    responses(
        (status = 200, description = "Tenant group updated", body = TenantGroup),
        (status = 404, description = "Tenant group not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Tenant group update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Tenant group update failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_tenant_group(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<TenantGroupPayload>,
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
    let tenant_group_id = match parse_uuid(&id, "tenant_group_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let name = match parse_required_string(&payload.name, "name") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let parent_id = match parse_optional_uuid(payload.parent_id, "parent_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.inventory_admin.clone();
    match service.get_tenant_group(tenant_group_id).await {
        Ok(None) => {
            return handle_app_error(AppError::NotFound("Tenant group not found".to_string()));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };

    let tenant_group = TenantGroup {
        id: tenant_group_id,
        external_id: None,
        name,
        slug: payload.slug,
        parent_id,
        description: payload.description,
        piping_synced_at: None,
        ..Default::default()
    };
    match service.update_tenant_group(tenant_group.clone()).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            (StatusCode::OK, Json(tenant_group)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Delete a tenant group.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/tenant_groups/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Tenant group id"),
    ),
    responses(
        (status = 204, description = "Tenant group deleted"),
        (status = 404, description = "Tenant group not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Tenant group delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Tenant group delete failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_tenant_group(
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
    let tenant_group_id = match parse_uuid(&id, "tenant_group_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.inventory_admin.clone();
    match service.get_tenant_group(tenant_group_id).await {
        Ok(None) => {
            return handle_app_error(AppError::NotFound("Tenant group not found".to_string()));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };

    match service.delete_tenant_group(tenant_group_id).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => handle_app_error(error),
    }
}
