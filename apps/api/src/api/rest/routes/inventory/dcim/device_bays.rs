//! Device bays REST routes — full CRUD + bulk.
//!
//! Handlers never run SQL directly; all persistence goes through
//! `DeviceBayRepository` on `AppState`.

use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::infrastructure::persistence::repositories::dcim::device_bay_repository::{
    DEVICE_BAY_COLUMNS, DeviceBayFilter,
};
use crate::contexts::inventory::infrastructure::persistence::{
    BulkDeviceBayResult, DeviceBayRecord, DeviceBayRepository, NewDeviceBay, UpdateDeviceBay,
};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_uuids};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

use super::super::{
    InventoryListEnvelope, ensure_access, parse_optional_uuid, parse_required_string, parse_uuid,
};

/// Query parameters for device bay listing.
#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct DeviceBayListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    pub device_id: Option<String>,
    pub installed_device_id: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct DeviceBayCreatePayload {
    pub device_id: String,
    pub name: String,
    pub label: Option<String>,
    pub description: Option<String>,
    pub installed_device_id: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct DeviceBayUpdatePayload {
    pub name: Option<String>,
    pub label: Option<String>,
    pub description: Option<String>,
    pub installed_device_id: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct DeviceBayBulkCreatePayload {
    #[serde(deserialize_with = "crate::api::rest::routes::validation::deserialize_bulk_items")]
    #[schema(min_items = 1)]
    pub items: Vec<DeviceBayCreatePayload>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DeviceBayResponse {
    pub id: Uuid,
    pub device_id: Uuid,
    pub name: String,
    pub label: String,
    pub description: String,
    pub installed_device_id: Option<Uuid>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkDeviceBayResponse {
    pub created: Vec<DeviceBayResponse>,
    pub errors: Vec<String>,
}

fn map_device_bay(record: DeviceBayRecord) -> DeviceBayResponse {
    DeviceBayResponse {
        id: record.id,
        device_id: record.device_id,
        name: record.name,
        label: record.label,
        description: record.description,
        installed_device_id: record.installed_device_id,
    }
}

fn map_bulk_device_bay_result(result: BulkDeviceBayResult) -> BulkDeviceBayResponse {
    BulkDeviceBayResponse {
        created: result.created.into_iter().map(map_device_bay).collect(),
        errors: result.errors,
    }
}

/// List device bays.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/device-bays",
    tag = "inventory",
    params(DeviceBayListQuery),
    responses(
        (status = 200, description = "Device bays", body = InventoryListEnvelope<DeviceBayResponse>),
        (status = "4XX", description = "Device bay list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Device bay list failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_device_bays(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<DeviceBayListQuery>,
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

    let device_id = match parse_csv_uuids(query.device_id.as_deref(), "device_id") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let installed_device_id =
        match parse_csv_uuids(query.installed_device_id.as_deref(), "installed_device_id") {
            Ok(v) => v,
            Err(error) => return handle_app_error(error),
        };
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        DEVICE_BAY_COLUMNS,
    ) {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let filter = DeviceBayFilter {
        device_id,
        installed_device_id,
        search: crate::contexts::shared::list_query::search_pattern(query.search.as_deref()),
        sort,
    };

    let resolved_limit = clamp_limit(query.limit);
    let resolved_offset = clamp_offset(query.offset);
    match state
        .repositories
        .device_bays
        .list(filter, query.limit, query.offset)
        .await
    {
        Ok((records, total)) => {
            let items: Vec<DeviceBayResponse> = records.into_iter().map(map_device_bay).collect();
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

/// Get a device bay.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/device-bays/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Device bay id"),
    ),
    responses(
        (status = 200, description = "Device bay", body = DeviceBayResponse),
        (status = 404, description = "Device bay not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Device bay get request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Device bay get failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_device_bay(
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
    match state.repositories.device_bays.get_by_id(id).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_device_bay(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Device bay not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a device bay.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/device-bays",
    tag = "inventory",
    request_body = DeviceBayCreatePayload,
    responses(
        (status = 201, description = "Device bay created", body = DeviceBayResponse),
        (status = "4XX", description = "Device bay create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Device bay create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_device_bay(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<DeviceBayCreatePayload>,
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
    let device_id = match parse_uuid(&payload.device_id, "device_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let installed_device_id =
        match parse_optional_uuid(payload.installed_device_id, "installed_device_id") {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };

    let new_bay = NewDeviceBay {
        id: Uuid::now_v7(),
        device_id,
        name,
        label: payload.label,
        description: payload.description,
        installed_device_id,
    };

    match state.repositories.device_bays.create(&new_bay).await {
        Ok(record) => (StatusCode::CREATED, Json(map_device_bay(record))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update a device bay.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/device-bays/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Device bay id"),
    ),
    request_body = DeviceBayUpdatePayload,
    responses(
        (status = 200, description = "Device bay updated", body = DeviceBayResponse),
        (status = 404, description = "Device bay not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Device bay update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Device bay update failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_device_bay(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<DeviceBayUpdatePayload>,
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
    let installed_device_id =
        match parse_optional_uuid(payload.installed_device_id, "installed_device_id") {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };

    let changes = UpdateDeviceBay {
        name: payload.name,
        label: payload.label,
        description: payload.description,
        installed_device_id,
    };

    match state.repositories.device_bays.update(id, &changes).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_device_bay(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Device bay not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a device bay.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/device-bays/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Device bay id"),
    ),
    responses(
        (status = 204, description = "Device bay deleted"),
        (status = "4XX", description = "Device bay delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Device bay delete failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_device_bay(
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
    match state.repositories.device_bays.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create device bays.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/device-bays/bulk",
    tag = "inventory",
    request_body = DeviceBayBulkCreatePayload,
    responses(
        (status = 200, description = "Bulk device bay create result", body = BulkDeviceBayResponse),
        (status = "4XX", description = "Bulk device bay create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Bulk device bay create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_device_bays(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<DeviceBayBulkCreatePayload>,
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
        let device_id = match parse_uuid(&p.device_id, "device_id") {
            Ok(value) => value,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        let installed_device_id =
            match parse_optional_uuid(p.installed_device_id, "installed_device_id") {
                Ok(value) => value,
                Err(error) => {
                    return handle_app_error(AppError::Validation(format!(
                        "item {index}: {error:?}"
                    )));
                }
            };
        items.push(NewDeviceBay {
            id: Uuid::now_v7(),
            device_id,
            name,
            label: p.label,
            description: p.description,
            installed_device_id,
        });
    }

    match state.repositories.device_bays.bulk_create(&items).await {
        Ok(result) => (StatusCode::OK, Json(map_bulk_device_bay_result(result))).into_response(),
        Err(error) => handle_app_error(error),
    }
}
