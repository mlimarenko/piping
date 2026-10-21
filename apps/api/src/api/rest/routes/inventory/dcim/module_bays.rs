//! Module bays REST routes — full CRUD + bulk.
//!
//! Handlers never run SQL directly; all persistence goes through
//! `ModuleBayRepository` on `AppState`.

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
use crate::contexts::inventory::infrastructure::persistence::repositories::dcim::module_bay_repository::{
    MODULE_BAY_COLUMNS, ModuleBayFilter,
};
use crate::contexts::inventory::infrastructure::persistence::{
    BulkModuleBayResult, ModuleBayRecord, ModuleBayRepository, NewModuleBay, UpdateModuleBay,
};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_uuids};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

use super::super::{
    InventoryListEnvelope, ensure_access, parse_optional_uuid, parse_required_string, parse_uuid,
};

/// Query parameters for module bay listing.
#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct ModuleBayListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    pub device_id: Option<String>,
    pub module_id: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ModuleBayCreatePayload {
    pub device_id: String,
    pub module_id: Option<String>,
    pub name: String,
    pub label: Option<String>,
    pub position: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ModuleBayUpdatePayload {
    pub name: Option<String>,
    pub label: Option<String>,
    pub position: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ModuleBayBulkCreatePayload {
    #[serde(deserialize_with = "crate::api::rest::routes::validation::deserialize_bulk_items")]
    #[schema(min_items = 1)]
    pub items: Vec<ModuleBayCreatePayload>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ModuleBayResponse {
    pub id: Uuid,
    pub device_id: Uuid,
    pub module_id: Option<Uuid>,
    pub name: String,
    pub label: String,
    pub position: String,
    pub description: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkModuleBayResponse {
    pub created: Vec<ModuleBayResponse>,
    pub errors: Vec<String>,
}

fn map_module_bay(record: ModuleBayRecord) -> ModuleBayResponse {
    ModuleBayResponse {
        id: record.id,
        device_id: record.device_id,
        module_id: record.module_id,
        name: record.name,
        label: record.label,
        position: record.position,
        description: record.description,
    }
}

fn map_bulk_module_bay_result(result: BulkModuleBayResult) -> BulkModuleBayResponse {
    BulkModuleBayResponse {
        created: result.created.into_iter().map(map_module_bay).collect(),
        errors: result.errors,
    }
}

/// List module bays.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/module-bays",
    tag = "inventory",
    params(ModuleBayListQuery),
    responses(
        (status = 200, description = "Module bays", body = InventoryListEnvelope<ModuleBayResponse>),
        (status = "4XX", description = "Module bay list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Module bay list failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_module_bays(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ModuleBayListQuery>,
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
    let module_id = match parse_csv_uuids(query.module_id.as_deref(), "module_id") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        MODULE_BAY_COLUMNS,
    ) {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let filter = ModuleBayFilter {
        device_id,
        module_id,
        search: crate::contexts::shared::list_query::search_pattern(query.search.as_deref()),
        sort,
    };

    let resolved_limit = clamp_limit(query.limit);
    let resolved_offset = clamp_offset(query.offset);
    match state
        .repositories
        .module_bays
        .list(filter, query.limit, query.offset)
        .await
    {
        Ok((records, total)) => {
            let items: Vec<ModuleBayResponse> = records.into_iter().map(map_module_bay).collect();
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

/// Get a module bay.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/module-bays/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Module bay id"),
    ),
    responses(
        (status = 200, description = "Module bay", body = ModuleBayResponse),
        (status = 404, description = "Module bay not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Module bay get request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Module bay get failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_module_bay(
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
    match state.repositories.module_bays.get_by_id(id).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_module_bay(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Module bay not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a module bay.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/module-bays",
    tag = "inventory",
    request_body = ModuleBayCreatePayload,
    responses(
        (status = 201, description = "Module bay created", body = ModuleBayResponse),
        (status = "4XX", description = "Module bay create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Module bay create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_module_bay(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<ModuleBayCreatePayload>,
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
    let module_id = match parse_optional_uuid(payload.module_id, "module_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let new_bay = NewModuleBay {
        id: Uuid::now_v7(),
        device_id,
        module_id,
        name,
        label: payload.label,
        position: payload.position,
        description: payload.description,
    };

    match state.repositories.module_bays.create(&new_bay).await {
        Ok(record) => (StatusCode::CREATED, Json(map_module_bay(record))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update a module bay.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/module-bays/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Module bay id"),
    ),
    request_body = ModuleBayUpdatePayload,
    responses(
        (status = 200, description = "Module bay updated", body = ModuleBayResponse),
        (status = 404, description = "Module bay not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Module bay update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Module bay update failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_module_bay(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<ModuleBayUpdatePayload>,
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

    let changes = UpdateModuleBay {
        name: payload.name,
        label: payload.label,
        position: payload.position,
        description: payload.description,
    };

    match state.repositories.module_bays.update(id, &changes).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_module_bay(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Module bay not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a module bay.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/module-bays/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Module bay id"),
    ),
    responses(
        (status = 204, description = "Module bay deleted"),
        (status = "4XX", description = "Module bay delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Module bay delete failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_module_bay(
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
    match state.repositories.module_bays.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create module bays.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/module-bays/bulk",
    tag = "inventory",
    request_body = ModuleBayBulkCreatePayload,
    responses(
        (status = 200, description = "Bulk module bay create result", body = BulkModuleBayResponse),
        (status = "4XX", description = "Bulk module bay create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Bulk module bay create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_module_bays(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<ModuleBayBulkCreatePayload>,
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
        let module_id = match parse_optional_uuid(p.module_id, "module_id") {
            Ok(value) => value,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        items.push(NewModuleBay {
            id: Uuid::now_v7(),
            device_id,
            module_id,
            name,
            label: p.label,
            position: p.position,
            description: p.description,
        });
    }

    match state.repositories.module_bays.bulk_create(&items).await {
        Ok(result) => (StatusCode::OK, Json(map_bulk_module_bay_result(result))).into_response(),
        Err(error) => handle_app_error(error),
    }
}
