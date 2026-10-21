use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::domain::DeviceType;
use crate::contexts::inventory::infrastructure::persistence::{
    DEVICE_TYPE_COLUMNS, SortSpec, parse_csv_uuids,
};
use crate::errors::AppError;

use super::super::{
    BulkCreateRequest, BulkCreateResponse, DeviceTypeListRowResponse, DeviceTypePayload,
    InventoryListEnvelope, bump_host_config_cache_generation, ensure_access,
    ensure_create_inventory_access, parse_required_string, parse_uuid, push_bulk_error,
    require_bulk_items,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};

/// Query parameters for device-type listing — pagination, free-text search, and
/// allowlisted sorting.
///
/// `sort` is a stable logical column key (e.g. `model`, `manufacturer`);
/// `order` is `asc` (default) or `desc`. An unknown sort key or invalid order
/// is rejected with 400.
#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct DeviceTypeListQuery {
    /// Comma-separated manufacturer ids; matches any (`IN`).
    pub manufacturer_id: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    /// Case-insensitive substring search on device-type model.
    pub search: Option<String>,
    /// Logical sort column key (allowlisted; unknown keys are rejected).
    pub sort: Option<String>,
    /// Sort direction: `asc` (default) or `desc`.
    pub order: Option<String>,
}

/// List device types.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/device_types",
    tag = "inventory",
    params(DeviceTypeListQuery),
    responses(
        (status = 200, description = "Device types", body = InventoryListEnvelope<DeviceTypeListRowResponse>),
        (status = "4XX", description = "Device type list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Device type list failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_device_types(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<DeviceTypeListQuery>,
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
        DEVICE_TYPE_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let manufacturer_id = match parse_csv_uuids(query.manufacturer_id.as_deref(), "manufacturer_id")
    {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let resolved_limit = clamp_limit(query.limit);
    let resolved_offset = clamp_offset(query.offset);
    let search = query
        .search
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| format!("%{}%", s));
    let service = state.services.inventory_admin.clone();
    match service
        .list_device_types_hydrated(
            crate::contexts::inventory::application::InventoryListQuery::new(
                query.limit,
                query.offset,
                search,
            )
            .with_manufacturer_id(manufacturer_id)
            .with_sort(sort),
        )
        .await
    {
        Ok((rows, total)) => {
            let items: Vec<DeviceTypeListRowResponse> = rows
                .into_iter()
                .map(|row| DeviceTypeListRowResponse {
                    id: row.record.id,
                    external_id: row.record.external_id,
                    manufacturer_id: row.record.manufacturer_id,
                    manufacturer_name: row.manufacturer_name,
                    model: row.record.model,
                    slug: row.record.slug,
                    default_platform_id: row.record.default_platform_id,
                    part_number: row.record.part_number,
                    u_height: row.record.u_height,
                    is_full_depth: row.record.is_full_depth,
                    subdevice_role: row.record.subdevice_role,
                    airflow: row.record.airflow,
                    weight: row.record.weight,
                    weight_unit: row.record.weight_unit,
                    front_image: row.record.front_image,
                    rear_image: row.record.rear_image,
                    description: row.record.description,
                    comments: row.record.comments,
                })
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

/// Get a device type.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/device_types/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Device type id"),
    ),
    responses(
        (status = 200, description = "Device type", body = DeviceType),
        (status = 404, description = "Device type not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Device type get request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Device type get failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_device_type(
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
    let device_type_id = match parse_uuid(&id, "device_type_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.inventory_admin.clone();
    match service.get_device_type(device_type_id).await {
        Ok(Some(item)) => (StatusCode::OK, Json(item)).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Device type not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a device type.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/device_types",
    tag = "inventory",
    request_body = DeviceTypePayload,
    responses(
        (status = 201, description = "Device type created", body = DeviceType),
        (status = "4XX", description = "Device type create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Device type create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_device_type(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<DeviceTypePayload>,
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
    let manufacturer_id_raw =
        match parse_required_string(&payload.manufacturer_id, "manufacturer_id") {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };
    let manufacturer_id = match parse_uuid(&manufacturer_id_raw, "manufacturer_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let model = match parse_required_string(&payload.model, "model") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let slug = match parse_required_string(&payload.slug, "slug") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let device_type = DeviceType {
        id: Uuid::now_v7(),
        external_id: None,
        manufacturer_id,
        model,
        slug,
        description: payload.description,
        comments: payload.comments,
        piping_synced_at: None,
        ..Default::default()
    };
    let service = state.services.inventory_admin.clone();
    match service.create_device_type(device_type.clone()).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            (StatusCode::CREATED, Json(device_type)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create device types.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/device_types/bulk",
    tag = "inventory",
    request_body = BulkCreateRequest<DeviceTypePayload>,
    responses(
        (status = 200, description = "Bulk device type create result", body = BulkCreateResponse<DeviceType>),
        (status = "4XX", description = "Bulk device type create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Bulk device type create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_device_types(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(request): EnvelopeJson<BulkCreateRequest<DeviceTypePayload>>,
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
        let manufacturer_id_raw =
            match parse_required_string(&payload.manufacturer_id, "manufacturer_id") {
                Ok(value) => value,
                Err(error) => {
                    push_bulk_error(&mut errors, index, error);
                    continue;
                }
            };
        let manufacturer_id = match parse_uuid(&manufacturer_id_raw, "manufacturer_id") {
            Ok(value) => value,
            Err(error) => {
                push_bulk_error(&mut errors, index, error);
                continue;
            }
        };
        let model = match parse_required_string(&payload.model, "model") {
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
        let device_type = DeviceType {
            id: Uuid::now_v7(),
            external_id: None,
            manufacturer_id,
            model,
            slug,
            description: payload.description,
            comments: payload.comments,
            piping_synced_at: None,
            ..Default::default()
        };

        match service.create_device_type(device_type.clone()).await {
            Ok(()) => created.push(device_type),
            Err(error) => push_bulk_error(&mut errors, index, error),
        }
    }

    if !created.is_empty() {
        bump_host_config_cache_generation(&state).await;
    }

    (StatusCode::OK, Json(BulkCreateResponse { created, errors })).into_response()
}

/// Update a device type.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/device_types/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Device type id"),
    ),
    request_body = DeviceTypePayload,
    responses(
        (status = 200, description = "Device type updated", body = DeviceType),
        (status = 404, description = "Device type not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Device type update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Device type update failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_device_type(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<DeviceTypePayload>,
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
    let device_type_id = match parse_uuid(&id, "device_type_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let manufacturer_id_raw =
        match parse_required_string(&payload.manufacturer_id, "manufacturer_id") {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };
    let manufacturer_id = match parse_uuid(&manufacturer_id_raw, "manufacturer_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let model = match parse_required_string(&payload.model, "model") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let slug = match parse_required_string(&payload.slug, "slug") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let service = state.services.inventory_admin.clone();
    match service.get_device_type(device_type_id).await {
        Ok(None) => {
            return handle_app_error(AppError::NotFound("Device type not found".to_string()));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };

    let device_type = DeviceType {
        id: device_type_id,
        external_id: None,
        manufacturer_id,
        model,
        slug,
        description: payload.description,
        comments: payload.comments,
        piping_synced_at: None,
        ..Default::default()
    };

    match service.update_device_type(device_type.clone()).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            (StatusCode::OK, Json(device_type)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Delete a device type.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/device_types/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Device type id"),
    ),
    responses(
        (status = 204, description = "Device type deleted"),
        (status = 404, description = "Device type not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Device type delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Device type delete failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_device_type(
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
    let device_type_id = match parse_uuid(&id, "device_type_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.inventory_admin.clone();
    match service.get_device_type(device_type_id).await {
        Ok(None) => {
            return handle_app_error(AppError::NotFound("Device type not found".to_string()));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };
    match service.delete_device_type(device_type_id).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => handle_app_error(error),
    }
}
