//! Modules REST routes — full CRUD + bulk.
//!
//! Handlers never run SQL directly; all persistence goes through
//! `ModuleRepository` on `AppState`.

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
use crate::contexts::inventory::infrastructure::persistence::repositories::dcim::module_repository::ModuleFilter;
use crate::contexts::inventory::infrastructure::persistence::{
    BulkModuleResult, MODULE_COLUMNS, ModuleRecord, ModuleRepository, NewModule, UpdateModule,
    parse_csv_strings, parse_csv_uuids,
};
use crate::contexts::shared::list_query::SortSpec;
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

use super::super::{InventoryListEnvelope, ensure_access, parse_uuid};

/// Query parameters for module listing.
#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct ModuleListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    /// Comma-separated device ids; matches any (`IN`).
    pub device_id: Option<String>,
    /// Comma-separated module bay ids; matches any (`IN`).
    pub module_bay_id: Option<String>,
    /// Comma-separated module type ids; matches any (`IN`).
    pub module_type_id: Option<String>,
    /// Comma-separated module statuses; matches any (`IN`).
    pub status: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ModuleCreatePayload {
    pub device_id: String,
    pub module_bay_id: String,
    pub module_type_id: String,
    pub status: Option<String>,
    pub serial: Option<String>,
    pub asset_tag: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ModuleUpdatePayload {
    pub status: Option<String>,
    pub serial: Option<String>,
    /// Pass `null` explicitly to clear the asset tag; omit to leave unchanged.
    pub asset_tag: Option<Option<String>>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ModuleBulkCreatePayload {
    #[serde(deserialize_with = "crate::api::rest::routes::validation::deserialize_bulk_items")]
    #[schema(min_items = 1)]
    pub items: Vec<ModuleCreatePayload>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ModuleResponse {
    pub id: Uuid,
    pub device_id: Uuid,
    pub module_bay_id: Uuid,
    pub module_bay_name: Option<String>,
    pub module_type_id: Uuid,
    pub module_type_name: Option<String>,
    pub status: String,
    pub serial: String,
    pub asset_tag: Option<String>,
    pub description: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkModuleResponse {
    pub created: Vec<ModuleResponse>,
    pub errors: Vec<String>,
}

fn map_module(record: ModuleRecord) -> ModuleResponse {
    ModuleResponse {
        id: record.id,
        device_id: record.device_id,
        module_bay_id: record.module_bay_id,
        module_bay_name: record.module_bay_name,
        module_type_id: record.module_type_id,
        module_type_name: record.module_type_name,
        status: record.status,
        serial: record.serial,
        asset_tag: record.asset_tag,
        description: record.description,
    }
}

fn map_bulk_module_result(result: BulkModuleResult) -> BulkModuleResponse {
    BulkModuleResponse {
        created: result.created.into_iter().map(map_module).collect(),
        errors: result.errors,
    }
}

/// List modules.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/modules",
    tag = "inventory",
    params(ModuleListQuery),
    responses(
        (status = 200, description = "Modules", body = InventoryListEnvelope<ModuleResponse>),
        (status = "4XX", description = "Module list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Module list failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_modules(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ModuleListQuery>,
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
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let module_bay_id = match parse_csv_uuids(query.module_bay_id.as_deref(), "module_bay_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let module_type_id = match parse_csv_uuids(query.module_type_id.as_deref(), "module_type_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let status = parse_csv_strings(query.status.as_deref());

    let resolved_limit = clamp_limit(query.limit);
    let resolved_offset = clamp_offset(query.offset);
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        MODULE_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let filter = ModuleFilter {
        search: crate::contexts::shared::list_query::search_pattern(query.search.as_deref()),
        device_id,
        module_bay_id,
        module_type_id,
        status,
        sort,
    };
    match state
        .repositories
        .modules
        .list(filter, query.limit, query.offset)
        .await
    {
        Ok((records, total)) => {
            let items: Vec<ModuleResponse> = records.into_iter().map(map_module).collect();
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

/// Get a module.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/modules/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Module id"),
    ),
    responses(
        (status = 200, description = "Module", body = ModuleResponse),
        (status = 404, description = "Module not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Module get request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Module get failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_module(
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
    match state.repositories.modules.get_by_id(id).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_module(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Module not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a module.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/modules",
    tag = "inventory",
    request_body = ModuleCreatePayload,
    responses(
        (status = 201, description = "Module created", body = ModuleResponse),
        (status = "4XX", description = "Module create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Module create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_module(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<ModuleCreatePayload>,
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

    let device_id = match parse_uuid(&payload.device_id, "device_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let module_bay_id = match parse_uuid(&payload.module_bay_id, "module_bay_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let module_type_id = match parse_uuid(&payload.module_type_id, "module_type_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let new_module = NewModule {
        id: Uuid::now_v7(),
        device_id,
        module_bay_id,
        module_type_id,
        status: payload.status,
        serial: payload.serial,
        asset_tag: payload.asset_tag,
        description: payload.description,
    };

    match state.repositories.modules.create(&new_module).await {
        Ok(record) => (StatusCode::CREATED, Json(map_module(record))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update a module.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/modules/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Module id"),
    ),
    request_body = ModuleUpdatePayload,
    responses(
        (status = 200, description = "Module updated", body = ModuleResponse),
        (status = 404, description = "Module not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Module update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Module update failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_module(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<ModuleUpdatePayload>,
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

    let changes = UpdateModule {
        status: payload.status,
        serial: payload.serial,
        asset_tag: payload.asset_tag,
        description: payload.description,
    };

    match state.repositories.modules.update(id, &changes).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_module(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Module not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a module.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/modules/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Module id"),
    ),
    responses(
        (status = 204, description = "Module deleted"),
        (status = "4XX", description = "Module delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Module delete failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_module(
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
    match state.repositories.modules.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create modules.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/modules/bulk",
    tag = "inventory",
    request_body = ModuleBulkCreatePayload,
    responses(
        (status = 200, description = "Bulk module create result", body = BulkModuleResponse),
        (status = "4XX", description = "Bulk module create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Bulk module create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_modules(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<ModuleBulkCreatePayload>,
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
        let device_id = match parse_uuid(&p.device_id, "device_id") {
            Ok(value) => value,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        let module_bay_id = match parse_uuid(&p.module_bay_id, "module_bay_id") {
            Ok(value) => value,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        let module_type_id = match parse_uuid(&p.module_type_id, "module_type_id") {
            Ok(value) => value,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        items.push(NewModule {
            id: Uuid::now_v7(),
            device_id,
            module_bay_id,
            module_type_id,
            status: p.status,
            serial: p.serial,
            asset_tag: p.asset_tag,
            description: p.description,
        });
    }

    match state.repositories.modules.bulk_create(&items).await {
        Ok(result) => (StatusCode::OK, Json(map_bulk_module_result(result))).into_response(),
        Err(error) => handle_app_error(error),
    }
}
