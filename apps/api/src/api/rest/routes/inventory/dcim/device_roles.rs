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
use crate::contexts::inventory::domain::DeviceRole;
use crate::contexts::inventory::infrastructure::persistence::repositories::dcim::device_role_repository::DEVICE_ROLE_COLUMNS;
use crate::contexts::shared::list_query::{SortSpec, parse_csv_strings};
use crate::errors::AppError;
use crate::observability::{EventFields, log_error_event};

use super::super::{
    BulkCreateRequest, BulkCreateResponse, DeviceRolePayload, InventoryListEnvelope,
    bump_host_config_cache_generation, ensure_access, ensure_create_inventory_access,
    parse_required_string, parse_uuid, push_bulk_error, require_bulk_items,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};

#[derive(Debug, serde::Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct DeviceRoleListQuery {
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub search: Option<String>,
    pub vm_role: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

/// List device roles.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/device_roles",
    tag = "inventory",
    params(DeviceRoleListQuery),
    responses(
        (status = 200, description = "Device roles", body = InventoryListEnvelope<DeviceRole>),
        (status = "4XX", description = "Device role list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Device role list failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_device_roles(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<DeviceRoleListQuery>,
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
        DEVICE_ROLE_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.inventory_admin.clone();
    match service
        .list_device_roles(
            crate::contexts::inventory::application::InventoryListQuery::new(
                query.limit,
                query.offset,
                search,
            )
            .with_vm_role(parse_csv_strings(query.vm_role.as_deref()))
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

/// Get a device role.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/device_roles/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Device role id"),
    ),
    responses(
        (status = 200, description = "Device role", body = DeviceRole),
        (status = 404, description = "Device role not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Device role get request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Device role get failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_device_role(
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
    let role_id = match parse_uuid(&id, "device_role_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.inventory_admin.clone();
    match service.get_device_role(role_id).await {
        Ok(Some(role)) => (StatusCode::OK, Json(role)).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Device role not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a device role.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/device_roles",
    tag = "inventory",
    request_body = DeviceRolePayload,
    responses(
        (status = 201, description = "Device role created", body = DeviceRole),
        (status = "4XX", description = "Device role create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Device role create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_device_role(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<DeviceRolePayload>,
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
    let role = DeviceRole {
        id: Uuid::now_v7(),
        external_id: None,
        name,
        slug: payload.slug,
        color: payload.color,
        vm_role: payload.vm_role,
        description: payload.description,
        custom_fields: payload.custom_fields,
        piping_synced_at: None,
        ..Default::default()
    };
    let service = state.services.inventory_admin.clone();
    match service.create_device_role(role.clone()).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            (StatusCode::CREATED, Json(role)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create device roles.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/device_roles/bulk",
    tag = "inventory",
    request_body = BulkCreateRequest<DeviceRolePayload>,
    responses(
        (status = 200, description = "Bulk device role create result", body = BulkCreateResponse<DeviceRole>),
        (status = "4XX", description = "Bulk device role create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Bulk device role create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_device_roles(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(request): EnvelopeJson<BulkCreateRequest<DeviceRolePayload>>,
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
        let role = DeviceRole {
            id: Uuid::now_v7(),
            external_id: None,
            name,
            slug: payload.slug,
            color: payload.color,
            vm_role: payload.vm_role,
            description: payload.description,
            custom_fields: payload.custom_fields,
            piping_synced_at: None,
            ..Default::default()
        };

        match service.create_device_role(role.clone()).await {
            Ok(()) => created.push(role),
            Err(error) => push_bulk_error(&mut errors, index, error),
        }
    }

    if !created.is_empty() {
        bump_host_config_cache_generation(&state).await;
    }

    (StatusCode::OK, Json(BulkCreateResponse { created, errors })).into_response()
}

/// Update a device role.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/device_roles/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Device role id"),
    ),
    request_body = DeviceRolePayload,
    responses(
        (status = 200, description = "Device role updated", body = DeviceRole),
        (status = 404, description = "Device role not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Device role update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Device role update failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_device_role(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<DeviceRolePayload>,
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
    let role_id = match parse_uuid(&id, "device_role_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let name = match parse_required_string(&payload.name, "name") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.inventory_admin.clone();
    match service.get_device_role(role_id).await {
        Ok(None) => {
            return handle_app_error(AppError::NotFound("Device role not found".to_string()));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };

    let role = DeviceRole {
        id: role_id,
        external_id: None,
        name,
        slug: payload.slug,
        color: payload.color,
        vm_role: payload.vm_role,
        description: payload.description,
        custom_fields: payload.custom_fields,
        piping_synced_at: None,
        ..Default::default()
    };
    match service.update_device_role(role.clone()).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            (StatusCode::OK, Json(role)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Delete a device role.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/device_roles/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Device role id"),
    ),
    responses(
        (status = 204, description = "Device role deleted"),
        (status = 404, description = "Device role not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Device role delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Device role delete failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_device_role(
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
    let role_id = match parse_uuid(&id, "device_role_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.inventory_admin.clone();
    let role = match service.get_device_role(role_id).await {
        Ok(None) => {
            return handle_app_error(AppError::NotFound("Device role not found".to_string()));
        }
        Ok(Some(role)) => role,
        Err(error) => return handle_app_error(error),
    };

    match service.delete_device_role(role_id).await {
        Ok(()) => {
            if let Err(error) = state
                .services
                .playbooks
                .prune_deleted_role_from_target_sets(role_id, &role.name)
                .await
            {
                log_error_event(
                    "playbooks.target_set.role_selector_prune_failed",
                    "target_set_role_selector_prune_failed",
                    error.to_string(),
                    EventFields::default()
                        .with_request_id(request_id.0.clone())
                        .with_operation("delete_device_role")
                        .with_resource("device_role", role_id.to_string()),
                );
            }
            bump_host_config_cache_generation(&state).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => handle_app_error(error),
    }
}
