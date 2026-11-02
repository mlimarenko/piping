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
use crate::api::rest::request_context::build_request_context;
use crate::api::rest::routes::custom_fields::CONTENT_TYPE_FHRP_GROUP;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::domain::FhrpGroup;
use crate::contexts::inventory::domain::ipam::fhrp_group::FhrpGroupRequest;
use crate::contexts::inventory::infrastructure::persistence::{
    FHRP_GROUP_COLUMNS, FhrpGroupFilter, FhrpGroupRepository,
};
use crate::contexts::shared::list_query::{
    SortSpec, parse_csv_strings, parse_csv_uuids, search_pattern,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

use super::super::{
    BulkCreateRequest, BulkCreateResponse, BulkCreatedBatch, InventoryListEnvelope,
    bump_host_config_cache_generation, finalize_bulk_created, parse_uuid, push_bulk_error,
    require_bulk_items, validate_and_split_custom_fields,
};

super::super::impl_inventory_custom_fields!(FhrpGroup, "ipam.fhrpgroup");

#[derive(Debug, serde::Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct FhrpGroupListQuery {
    pub owner_id: Option<String>,
    pub protocol: Option<String>,
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

#[utoipa::path(
    get, path = "/api/v1/inventory/fhrp-groups",
    operation_id = "list_inventory_fhrp_groups", tag = "inventory",
    params(FhrpGroupListQuery),
    responses(
        (status = 200, description = "FHRP groups", body = InventoryListEnvelope<FhrpGroup>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Inventory read denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_fhrp_groups(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<FhrpGroupListQuery>,
) -> impl IntoResponse {
    let context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_inventory_collection_access(&state, &context, Action::Read).await {
        return handle_app_error(error);
    }
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        FHRP_GROUP_COLUMNS,
    ) {
        Ok(sort) => sort,
        Err(error) => return handle_app_error(error),
    };
    let owner_id = match parse_csv_uuids(query.owner_id.as_deref(), "owner_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let protocol = parse_csv_strings(query.protocol.as_deref());
    if protocol.iter().any(|value| {
        !matches!(
            value.as_str(),
            "vrrp2" | "vrrp3" | "carp" | "clusterxl" | "glbp" | "hsrp" | "other"
        )
    }) {
        return handle_app_error(AppError::Validation("Unsupported FHRP protocol".into()));
    }
    let filter = FhrpGroupFilter {
        owner_id,
        protocol,
        search: search_pattern(query.search.as_deref()),
        sort,
    };
    match state
        .repositories
        .fhrp_groups
        .list_paginated(query.limit, query.offset, filter)
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
                    limit: clamp_limit(query.limit),
                    offset: clamp_offset(query.offset),
                }),
            )
                .into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

#[utoipa::path(
    get, path = "/api/v1/inventory/fhrp-groups/{id}",
    operation_id = "get_inventory_fhrp_group", tag = "inventory",
    params(("id" = String, Path, description = "FHRP group ID")),
    responses(
        (status = 200, description = "FHRP group", body = FhrpGroup),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Inventory read denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_fhrp_group(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let context = build_request_context(&request_id, auth_context, &headers);
    let id = match parse_uuid(&id, "fhrp_group_id") {
        Ok(id) => id,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = ensure_access_with_resource(
        &state,
        &context,
        Action::Read,
        ResourceType::Inventory,
        Some(&format!("inventory:{id}")),
    )
    .await
    {
        return handle_app_error(error);
    }
    match state.repositories.fhrp_groups.get_by_id(id).await {
        Ok(Some(item)) => {
            super::super::masked_inventory_response(&state, StatusCode::OK, item).await
        }
        Ok(None) => handle_app_error(AppError::NotFound("FHRP group not found".into())),
        Err(error) => handle_app_error(error),
    }
}

#[utoipa::path(
    post, path = "/api/v1/inventory/fhrp-groups",
    operation_id = "create_inventory_fhrp_group", tag = "inventory",
    request_body = FhrpGroupRequest,
    responses(
        (status = 201, description = "FHRP group created", body = FhrpGroup),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Inventory create denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_fhrp_group(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(FhrpGroupRequest(mut item)): EnvelopeJson<FhrpGroupRequest>,
) -> impl IntoResponse {
    let context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_inventory_collection_access(&state, &context, Action::Create).await {
        return handle_app_error(error);
    }
    item.id = Uuid::now_v7();
    if let Err(error) = item.validate() {
        return handle_app_error(error);
    }
    let outcome = match validate_and_split_custom_fields(
        &state,
        CONTENT_TYPE_FHRP_GROUP,
        item.custom_fields.take(),
        true,
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(error) => return handle_app_error(error),
    };
    item.custom_fields = outcome
        .as_ref()
        .map(|outcome| outcome.custom_field_data.clone());
    let inputs = match state.services.custom_field_values.prepare_secret_intents(
        item.id,
        outcome
            .map(|outcome| outcome.secret_intents)
            .unwrap_or_default(),
    ) {
        Ok(inputs) => inputs,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = state
        .repositories
        .fhrp_groups
        .create_with_secret_intents(&item, inputs)
        .await
    {
        return handle_app_error(error);
    }
    bump_host_config_cache_generation(&state).await;
    match state.repositories.fhrp_groups.get_by_id(item.id).await {
        Ok(Some(item)) => {
            super::super::masked_inventory_response(&state, StatusCode::CREATED, item).await
        }
        Ok(None) => handle_app_error(AppError::NotFound("Created FHRP group not found".into())),
        Err(error) => handle_app_error(error),
    }
}

#[utoipa::path(
    post, path = "/api/v1/inventory/fhrp-groups/bulk",
    operation_id = "bulk_create_inventory_fhrp_groups", tag = "inventory",
    request_body = BulkCreateRequest<FhrpGroupRequest>,
    responses(
        (status = 200, description = "Bulk FHRP group create result", body = BulkCreateResponse<FhrpGroup>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Inventory create denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_fhrp_groups(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(request): EnvelopeJson<BulkCreateRequest<FhrpGroupRequest>>,
) -> impl IntoResponse {
    let context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_inventory_collection_access(&state, &context, Action::Create).await {
        return handle_app_error(error);
    }
    let items = match require_bulk_items(request.items) {
        Ok(items) => items,
        Err(error) => return handle_app_error(error),
    };
    let mut created = Vec::new();
    let mut created_indices = Vec::new();
    let mut errors = Vec::new();
    let mut wrote = false;
    for (index, FhrpGroupRequest(mut item)) in items.into_iter().enumerate() {
        item.id = Uuid::now_v7();
        if let Err(error) = item.validate() {
            push_bulk_error(&mut errors, index, error);
            continue;
        }
        let outcome = match validate_and_split_custom_fields(
            &state,
            CONTENT_TYPE_FHRP_GROUP,
            item.custom_fields.take(),
            true,
        )
        .await
        {
            Ok(outcome) => outcome,
            Err(error) => {
                push_bulk_error(&mut errors, index, error);
                continue;
            }
        };
        item.custom_fields = outcome
            .as_ref()
            .map(|outcome| outcome.custom_field_data.clone());
        let inputs = match state.services.custom_field_values.prepare_secret_intents(
            item.id,
            outcome
                .map(|outcome| outcome.secret_intents)
                .unwrap_or_default(),
        ) {
            Ok(inputs) => inputs,
            Err(error) => {
                push_bulk_error(&mut errors, index, error);
                continue;
            }
        };
        if let Err(error) = state
            .repositories
            .fhrp_groups
            .create_with_secret_intents(&item, inputs)
            .await
        {
            push_bulk_error(&mut errors, index, error);
            continue;
        }
        wrote = true;
        match state.repositories.fhrp_groups.get_by_id(item.id).await {
            Ok(Some(item)) => {
                created.push(item);
                created_indices.push(index);
            }
            Ok(None) => push_bulk_error(
                &mut errors,
                index,
                AppError::NotFound("Created FHRP group not found".into()),
            ),
            Err(error) => push_bulk_error(&mut errors, index, error),
        }
    }
    finalize_bulk_created(
        &state,
        CONTENT_TYPE_FHRP_GROUP,
        BulkCreatedBatch {
            created: &mut created,
            created_indices: &created_indices,
            errors: &mut errors,
        },
        |item: &FhrpGroup| item.id,
        |item: &FhrpGroup| item.custom_fields.as_ref(),
        |item: &mut FhrpGroup, value| item.custom_fields = Some(value),
    )
    .await;
    // A post-commit read/masking failure still changed inventory.
    if wrote && created.is_empty() {
        bump_host_config_cache_generation(&state).await;
    }
    (StatusCode::OK, Json(BulkCreateResponse { created, errors })).into_response()
}

#[utoipa::path(
    put, path = "/api/v1/inventory/fhrp-groups/{id}",
    operation_id = "update_inventory_fhrp_group", tag = "inventory",
    params(("id" = String, Path, description = "FHRP group ID")),
    request_body = FhrpGroupRequest,
    responses(
        (status = 200, description = "FHRP group updated", body = FhrpGroup),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Inventory update denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_fhrp_group(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(FhrpGroupRequest(mut item)): EnvelopeJson<FhrpGroupRequest>,
) -> impl IntoResponse {
    let context = build_request_context(&request_id, Some(auth_context), &headers);
    item.id = match parse_uuid(&id, "fhrp_group_id") {
        Ok(id) => id,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) =
        super::fhrp_auth_key::ensure_fhrp_access(&state, &context, item.id, Action::Update).await
    {
        return handle_app_error(error);
    }
    if let Err(error) = item.validate() {
        return handle_app_error(error);
    }
    let outcome = match validate_and_split_custom_fields(
        &state,
        CONTENT_TYPE_FHRP_GROUP,
        item.custom_fields.take(),
        false,
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(error) => return handle_app_error(error),
    };
    item.custom_fields = outcome
        .as_ref()
        .map(|outcome| outcome.custom_field_data.clone());
    let inputs = match state.services.custom_field_values.prepare_secret_intents(
        item.id,
        outcome
            .map(|outcome| outcome.secret_intents)
            .unwrap_or_default(),
    ) {
        Ok(inputs) => inputs,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = state
        .repositories
        .fhrp_groups
        .update_with_secret_intents(&item, inputs)
        .await
    {
        return handle_app_error(error);
    }
    bump_host_config_cache_generation(&state).await;
    match state.repositories.fhrp_groups.get_by_id(item.id).await {
        Ok(Some(item)) => {
            super::super::masked_inventory_response(&state, StatusCode::OK, item).await
        }
        Ok(None) => handle_app_error(AppError::NotFound("Updated FHRP group not found".into())),
        Err(error) => handle_app_error(error),
    }
}

#[utoipa::path(
    delete, path = "/api/v1/inventory/fhrp-groups/{id}",
    operation_id = "delete_inventory_fhrp_group", tag = "inventory",
    params(("id" = String, Path, description = "FHRP group ID")),
    responses(
        (status = 204, description = "FHRP group deleted"),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Inventory delete denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_fhrp_group(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let context = build_request_context(&request_id, Some(auth_context), &headers);
    let id = match parse_uuid(&id, "fhrp_group_id") {
        Ok(id) => id,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = ensure_access_with_resource(
        &state,
        &context,
        Action::Delete,
        ResourceType::Inventory,
        Some(&format!("inventory:{id}")),
    )
    .await
    {
        return handle_app_error(error);
    }
    match state.repositories.fhrp_groups.get_by_id(id).await {
        Ok(Some(_)) => (),
        Ok(None) => return handle_app_error(AppError::NotFound("FHRP group not found".into())),
        Err(error) => return handle_app_error(error),
    }
    match state.repositories.fhrp_groups.delete(id).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => handle_app_error(error),
    }
}
