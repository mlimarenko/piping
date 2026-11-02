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
use crate::api::permission_helpers::{
    ensure_access_with_resource, ensure_inventory_collection_access,
};
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::rest::request_context::{build_request_context, project_inventory_delete_error};
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::domain::ModuleType;
use crate::contexts::inventory::domain::dcim::module_type::UpdateModuleType;
use crate::contexts::inventory::infrastructure::persistence::{
    MODULE_TYPE_COLUMNS, ModuleTypeRepository,
};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_uuids};
use crate::errors::AppError;

use super::super::{
    BulkCreateRequest, BulkCreateResponse, InventoryListEnvelope,
    bump_host_config_cache_generation, parse_required_string, parse_uuid, push_bulk_error,
    require_bulk_items,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};

super::super::impl_inventory_custom_fields!(ModuleType, "dcim.moduletype");

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
#[serde(deny_unknown_fields)]
pub struct ModuleTypePayload {
    pub profile_id: Option<Uuid>,
    pub attribute_data: Option<serde_json::Value>,
    pub cooling_method: Option<String>,
    pub end_of_life: Option<chrono::NaiveDate>,
    pub manufacturer_id: String,
    pub model: String,
    #[serde(
        default,
        deserialize_with = "crate::api::rest::routes::validation::deserialize_nullable_patch"
    )]
    pub module_bay_types: Option<Option<Vec<Uuid>>>,
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
    if let Err(error) =
        ensure_inventory_collection_access(&state, &request_context, Action::Read).await
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
        Ok((mut items, total)) => {
            if let Err(error) =
                super::super::mask_inventory_projection_page(&state, &mut items).await
            {
                return handle_app_error(error);
            }
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
    let module_type_id = match parse_uuid(&id, "module_type_id") {
        Ok(id) => id,
        Err(e) => return handle_app_error(e),
    };
    if let Err(error) = ensure_access_with_resource(
        &state,
        &request_context,
        Action::Read,
        ResourceType::Inventory,
        Some(&format!("inventory:{module_type_id}")),
    )
    .await
    {
        return handle_app_error(error);
    }
    match state
        .repositories
        .module_types
        .get_by_id(module_type_id)
        .await
    {
        Ok(Some(item)) => {
            super::super::masked_inventory_response(&state, StatusCode::OK, item).await
        }
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
    if let Err(error) =
        ensure_inventory_collection_access(&state, &request_context, Action::Create).await
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
        profile_id: payload.profile_id,
        attribute_data: payload.attribute_data,
        cooling_method: payload.cooling_method,
        end_of_life: payload.end_of_life,
        manufacturer_id,
        model,
        module_bay_types: match payload.module_bay_types {
            Some(None) => {
                return handle_app_error(AppError::Validation(
                    "module_bay_types must be an array, not null".into(),
                ));
            }
            Some(Some(ids)) => ids,
            None => Vec::new(),
        },
        ..Default::default()
    };
    if let Err(error) = authorize_profile(&state, &request_context, module_type.profile_id).await {
        return handle_app_error(error);
    }
    match state.repositories.module_types.create(&module_type).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            match state
                .repositories
                .module_types
                .get_by_id(module_type.id)
                .await
            {
                Ok(Some(item)) => {
                    super::super::masked_inventory_response(&state, StatusCode::CREATED, item).await
                }
                Ok(None) => handle_app_error(AppError::NotFound(
                    "Module type not found after create".into(),
                )),
                Err(error) => handle_app_error(error),
            }
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
    if let Err(error) =
        ensure_inventory_collection_access(&state, &request_context, Action::Create).await
    {
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
            profile_id: payload.profile_id,
            attribute_data: payload.attribute_data,
            cooling_method: payload.cooling_method,
            end_of_life: payload.end_of_life,
            manufacturer_id,
            model,
            module_bay_types: match payload.module_bay_types {
                Some(None) => {
                    push_bulk_error(
                        &mut errors,
                        index,
                        AppError::Validation("module_bay_types must be an array, not null".into()),
                    );
                    continue;
                }
                Some(Some(ids)) => ids,
                None => Vec::new(),
            },
            ..Default::default()
        };

        if let Err(error) =
            authorize_profile(&state, &request_context, module_type.profile_id).await
        {
            push_bulk_error(&mut errors, index, error);
            continue;
        }
        match state.repositories.module_types.create(&module_type).await {
            Ok(()) => match state
                .repositories
                .module_types
                .get_by_id(module_type.id)
                .await
            {
                Ok(Some(item)) => created.push(item),
                Ok(None) => push_bulk_error(
                    &mut errors,
                    index,
                    AppError::NotFound("Module type not found after create".into()),
                ),
                Err(error) => push_bulk_error(&mut errors, index, error),
            },
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
    request_body = UpdateModuleType,
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
    EnvelopeJson(mut payload): EnvelopeJson<UpdateModuleType>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    let module_type_id = match parse_uuid(&id, "module_type_id") {
        Ok(id) => id,
        Err(e) => return handle_app_error(e),
    };
    if let Err(error) = ensure_access_with_resource(
        &state,
        &request_context,
        Action::Update,
        ResourceType::Inventory,
        Some(&format!("inventory:{module_type_id}")),
    )
    .await
    {
        return handle_app_error(error);
    }
    if let Err(error) =
        authorize_profile(&state, &request_context, payload.profile_id.flatten()).await
    {
        return handle_app_error(error);
    }
    let outcome = match super::super::validate_and_split_custom_fields(
        &state,
        "dcim.moduletype",
        payload.custom_fields.take(),
        false,
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(error) => return handle_app_error(error),
    };
    payload.custom_fields = outcome.as_ref().map(|v| v.custom_field_data.clone());
    let inputs = match state.services.custom_field_values.prepare_secret_intents(
        module_type_id,
        outcome.map(|v| v.secret_intents).unwrap_or_default(),
    ) {
        Ok(inputs) => inputs,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .module_types
        .update(module_type_id, &payload, inputs)
        .await
    {
        Ok(Some(module_type)) => {
            bump_host_config_cache_generation(&state).await;
            super::super::masked_inventory_response(&state, StatusCode::OK, module_type).await
        }
        Ok(None) => handle_app_error(AppError::NotFound("Module type not found".into())),
        Err(error) => handle_app_error(error),
    }
}

async fn authorize_profile(
    state: &AppState,
    context: &crate::contexts::shared::request_context::RequestContext,
    profile_id: Option<Uuid>,
) -> Result<(), AppError> {
    if let Some(id) = profile_id {
        ensure_access_with_resource(
            state,
            context,
            Action::Read,
            ResourceType::Inventory,
            Some(&format!("inventory:{id}")),
        )
        .await?;
    }
    Ok(())
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
    let module_type_id = match parse_uuid(&id, "module_type_id") {
        Ok(id) => id,
        Err(e) => return handle_app_error(e),
    };
    if let Err(error) = ensure_access_with_resource(
        &state,
        &request_context,
        Action::Delete,
        ResourceType::Inventory,
        Some(&format!("inventory:{module_type_id}")),
    )
    .await
    {
        return handle_app_error(error);
    }
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
        Err(error) => {
            handle_app_error(project_inventory_delete_error(&state, &request_context, error).await)
        }
    }
}
