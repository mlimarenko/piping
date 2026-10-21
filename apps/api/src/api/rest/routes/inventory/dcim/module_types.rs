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
use crate::contexts::inventory::domain::ModuleType;
use crate::contexts::inventory::infrastructure::persistence::{
    MODULE_TYPE_COLUMNS, ModuleTypeRepository,
};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_uuids};
use crate::errors::AppError;

use super::super::{
    BulkCreateRequest, BulkCreateResponse, InventoryListEnvelope,
    bump_host_config_cache_generation, ensure_access, ensure_create_inventory_access,
    parse_required_string, parse_uuid, push_bulk_error, require_bulk_items,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};

#[derive(Debug, serde::Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct ModuleTypeListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    pub manufacturer_id: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct ModuleTypePayload {
    pub manufacturer_id: String,
    pub model: String,
}

/// List module types.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/module-types",
    tag = "inventory",
    params(ModuleTypeListQuery),
    responses(
        (status = 200, description = "Module types", body = InventoryListEnvelope<ModuleType>),
        (status = "4XX", description = "Module type list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Module type list failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_module_types(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ModuleTypeListQuery>,
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
    let manufacturer_ids =
        match parse_csv_uuids(query.manufacturer_id.as_deref(), "manufacturer_id") {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        MODULE_TYPE_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let resolved_limit = clamp_limit(query.limit);
    let resolved_offset = clamp_offset(query.offset);
    match state
        .repositories
        .module_types
        .list_paginated(
            &manufacturer_ids,
            crate::contexts::shared::list_query::search_pattern(query.search.as_deref()),
            query.limit,
            query.offset,
            sort,
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

/// Get a module type.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/module-types/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Module type id"),
    ),
    responses(
        (status = 200, description = "Module type", body = ModuleType),
        (status = 404, description = "Module type not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Module type get request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Module type get failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_module_type(
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
    let module_type_id = match parse_uuid(&id, "module_type_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .module_types
        .get_by_id(module_type_id)
        .await
    {
        Ok(Some(item)) => (StatusCode::OK, Json(item)).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Module type not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a module type.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/module-types",
    tag = "inventory",
    request_body = ModuleTypePayload,
    responses(
        (status = 201, description = "Module type created", body = ModuleType),
        (status = "4XX", description = "Module type create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Module type create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_module_type(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<ModuleTypePayload>,
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
    let module_type = ModuleType {
        id: Uuid::now_v7(),
        manufacturer_id,
        model,
        ..Default::default()
    };
    match state.repositories.module_types.create(&module_type).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            (StatusCode::CREATED, Json(module_type)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create module types.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/module-types/bulk",
    tag = "inventory",
    request_body = BulkCreateRequest<ModuleTypePayload>,
    responses(
        (status = 200, description = "Bulk module type create result", body = BulkCreateResponse<ModuleType>),
        (status = "4XX", description = "Bulk module type create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Bulk module type create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_module_types(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(request): EnvelopeJson<BulkCreateRequest<ModuleTypePayload>>,
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
        let module_type = ModuleType {
            id: Uuid::now_v7(),
            manufacturer_id,
            model,
            ..Default::default()
        };

        match state.repositories.module_types.create(&module_type).await {
            Ok(()) => created.push(module_type),
            Err(error) => push_bulk_error(&mut errors, index, error),
        }
    }

    if !created.is_empty() {
        bump_host_config_cache_generation(&state).await;
    }

    (StatusCode::OK, Json(BulkCreateResponse { created, errors })).into_response()
}

/// Update a module type.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/module-types/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Module type id"),
    ),
    request_body = ModuleTypePayload,
    responses(
        (status = 200, description = "Module type updated", body = ModuleType),
        (status = 404, description = "Module type not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Module type update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Module type update failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_module_type(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<ModuleTypePayload>,
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
    let module_type_id = match parse_uuid(&id, "module_type_id") {
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

    match state
        .repositories
        .module_types
        .get_by_id(module_type_id)
        .await
    {
        Ok(None) => {
            return handle_app_error(AppError::NotFound("Module type not found".to_string()));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };

    let module_type = ModuleType {
        id: module_type_id,
        manufacturer_id,
        model,
        ..Default::default()
    };

    match state.repositories.module_types.update(&module_type).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            (StatusCode::OK, Json(module_type)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Delete a module type.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/module-types/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Module type id"),
    ),
    responses(
        (status = 204, description = "Module type deleted"),
        (status = 404, description = "Module type not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Module type delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Module type delete failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_module_type(
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
    let module_type_id = match parse_uuid(&id, "module_type_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .module_types
        .get_by_id(module_type_id)
        .await
    {
        Ok(None) => {
            return handle_app_error(AppError::NotFound("Module type not found".to_string()));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };
    match state.repositories.module_types.delete(module_type_id).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => handle_app_error(error),
    }
}
