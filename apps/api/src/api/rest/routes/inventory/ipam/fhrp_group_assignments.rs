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
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::domain::FhrpGroupAssignment;
use crate::contexts::inventory::infrastructure::persistence::{
    FHRP_GROUP_ASSIGNMENT_COLUMNS, FhrpGroupAssignmentFilter, FhrpGroupAssignmentRepository,
};
use crate::contexts::shared::list_query::{
    SortSpec, parse_csv_strings, parse_csv_uuids, search_pattern,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;

use super::super::association_access::ensure_association_target_access;
use super::super::{
    BulkCreateRequest, BulkCreateResponse, InventoryListEnvelope,
    bump_host_config_cache_generation, parse_uuid, push_bulk_error, require_bulk_items,
};

async fn ensure_assignment_targets_access(
    state: &AppState,
    context: &RequestContext,
    item: &FhrpGroupAssignment,
) -> Result<(), AppError> {
    ensure_association_target_access(
        state,
        context,
        Action::Update,
        "ipam.fhrpgroup",
        item.group_id,
    )
    .await?;
    ensure_association_target_access(
        state,
        context,
        Action::Update,
        &item.interface_type,
        item.interface_id,
    )
    .await
}

#[derive(Debug, serde::Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct FhrpGroupAssignmentListQuery {
    pub group_id: Option<String>,
    pub interface_id: Option<String>,
    pub interface_type: Option<String>,
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

#[utoipa::path(
    get, path = "/api/v1/inventory/fhrp-group-assignments",
    operation_id = "list_inventory_fhrp_group_assignments", tag = "inventory",
    params(FhrpGroupAssignmentListQuery),
    responses(
        (status = 200, description = "FHRP group assignments", body = InventoryListEnvelope<FhrpGroupAssignment>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Inventory read denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_fhrp_group_assignments(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<FhrpGroupAssignmentListQuery>,
) -> impl IntoResponse {
    let context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_inventory_collection_access(&state, &context, Action::Read).await {
        return handle_app_error(error);
    }
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        FHRP_GROUP_ASSIGNMENT_COLUMNS,
    ) {
        Ok(sort) => sort,
        Err(error) => return handle_app_error(error),
    };
    let group_id = match parse_csv_uuids(query.group_id.as_deref(), "group_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let interface_id = match parse_csv_uuids(query.interface_id.as_deref(), "interface_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let interface_type = parse_csv_strings(query.interface_type.as_deref());
    if interface_type.iter().any(|token| {
        !matches!(
            token.as_str(),
            "dcim.interface" | "virtualization.vminterface"
        )
    }) {
        return handle_app_error(AppError::Validation("Invalid interface_type".into()));
    }
    let filter = FhrpGroupAssignmentFilter {
        group_id,
        interface_id,
        interface_type,
        search: search_pattern(query.search.as_deref()),
        sort,
    };
    match state
        .repositories
        .fhrp_group_assignments
        .list_paginated(query.limit, query.offset, filter)
        .await
    {
        Ok((items, total)) => (
            StatusCode::OK,
            Json(InventoryListEnvelope {
                items,
                total,
                limit: clamp_limit(query.limit),
                offset: clamp_offset(query.offset),
            }),
        )
            .into_response(),
        Err(error) => handle_app_error(error),
    }
}

#[utoipa::path(
    get, path = "/api/v1/inventory/fhrp-group-assignments/{id}",
    operation_id = "get_inventory_fhrp_group_assignment", tag = "inventory",
    params(("id" = String, Path, description = "FHRP group assignment ID")),
    responses(
        (status = 200, description = "FHRP group assignment", body = FhrpGroupAssignment),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Inventory read denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_fhrp_group_assignment(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let context = build_request_context(&request_id, auth_context, &headers);
    let id = match parse_uuid(&id, "fhrp_group_assignment_id") {
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
    match state
        .repositories
        .fhrp_group_assignments
        .get_by_id(id)
        .await
    {
        Ok(Some(item)) => (StatusCode::OK, Json(item)).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("FHRP group assignment not found".into())),
        Err(error) => handle_app_error(error),
    }
}

#[utoipa::path(
    post, path = "/api/v1/inventory/fhrp-group-assignments",
    operation_id = "create_inventory_fhrp_group_assignment", tag = "inventory",
    request_body = FhrpGroupAssignment,
    responses(
        (status = 201, description = "FHRP group assignment created", body = FhrpGroupAssignment),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Inventory create denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_fhrp_group_assignment(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(mut item): EnvelopeJson<FhrpGroupAssignment>,
) -> impl IntoResponse {
    let context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_inventory_collection_access(&state, &context, Action::Create).await {
        return handle_app_error(error);
    }
    item.id = Uuid::now_v7();
    if let Err(error) = item.validate() {
        return handle_app_error(error);
    }
    if let Err(error) = ensure_assignment_targets_access(&state, &context, &item).await {
        return handle_app_error(error);
    }
    if let Err(error) = state
        .repositories
        .fhrp_group_assignments
        .create(&item)
        .await
    {
        return handle_app_error(error);
    }
    bump_host_config_cache_generation(&state).await;
    match state
        .repositories
        .fhrp_group_assignments
        .get_by_id(item.id)
        .await
    {
        Ok(Some(item)) => (StatusCode::CREATED, Json(item)).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound(
            "Created FHRP group assignment not found".into(),
        )),
        Err(error) => handle_app_error(error),
    }
}

#[utoipa::path(
    post, path = "/api/v1/inventory/fhrp-group-assignments/bulk",
    operation_id = "bulk_create_inventory_fhrp_group_assignments", tag = "inventory",
    request_body = BulkCreateRequest<FhrpGroupAssignment>,
    responses(
        (status = 200, description = "Bulk FHRP group assignment create result", body = BulkCreateResponse<FhrpGroupAssignment>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Inventory create denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_fhrp_group_assignments(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(request): EnvelopeJson<BulkCreateRequest<FhrpGroupAssignment>>,
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
    let mut errors = Vec::new();
    let mut wrote = false;
    for (index, mut item) in items.into_iter().enumerate() {
        item.id = Uuid::now_v7();
        if let Err(error) = item.validate() {
            push_bulk_error(&mut errors, index, error);
            continue;
        }
        if let Err(error) = ensure_assignment_targets_access(&state, &context, &item).await {
            push_bulk_error(&mut errors, index, error);
            continue;
        }
        if let Err(error) = state
            .repositories
            .fhrp_group_assignments
            .create(&item)
            .await
        {
            push_bulk_error(&mut errors, index, error);
            continue;
        }
        wrote = true;
        match state
            .repositories
            .fhrp_group_assignments
            .get_by_id(item.id)
            .await
        {
            Ok(Some(item)) => created.push(item),
            Ok(None) => push_bulk_error(
                &mut errors,
                index,
                AppError::NotFound("Created FHRP group assignment not found".into()),
            ),
            Err(error) => push_bulk_error(&mut errors, index, error),
        }
    }
    if wrote {
        bump_host_config_cache_generation(&state).await;
    }
    (StatusCode::OK, Json(BulkCreateResponse { created, errors })).into_response()
}

#[utoipa::path(
    put, path = "/api/v1/inventory/fhrp-group-assignments/{id}",
    operation_id = "update_inventory_fhrp_group_assignment", tag = "inventory",
    params(("id" = String, Path, description = "FHRP group assignment ID")),
    request_body = FhrpGroupAssignment,
    responses(
        (status = 200, description = "FHRP group assignment updated", body = FhrpGroupAssignment),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Inventory update denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_fhrp_group_assignment(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(mut item): EnvelopeJson<FhrpGroupAssignment>,
) -> impl IntoResponse {
    let context = build_request_context(&request_id, Some(auth_context), &headers);
    item.id = match parse_uuid(&id, "fhrp_group_assignment_id") {
        Ok(id) => id,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = ensure_access_with_resource(
        &state,
        &context,
        Action::Update,
        ResourceType::Inventory,
        Some(&format!("inventory:{}", item.id)),
    )
    .await
    {
        return handle_app_error(error);
    }
    if let Err(error) = item.validate() {
        return handle_app_error(error);
    }
    match state
        .repositories
        .fhrp_group_assignments
        .get_by_id(item.id)
        .await
    {
        Ok(Some(_)) => (),
        Ok(None) => {
            return handle_app_error(AppError::NotFound("FHRP group assignment not found".into()));
        }
        Err(error) => return handle_app_error(error),
    }
    if let Err(error) = ensure_assignment_targets_access(&state, &context, &item).await {
        return handle_app_error(error);
    }
    if let Err(error) = state
        .repositories
        .fhrp_group_assignments
        .update(&item)
        .await
    {
        return handle_app_error(error);
    }
    bump_host_config_cache_generation(&state).await;
    match state
        .repositories
        .fhrp_group_assignments
        .get_by_id(item.id)
        .await
    {
        Ok(Some(item)) => (StatusCode::OK, Json(item)).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound(
            "Updated FHRP group assignment not found".into(),
        )),
        Err(error) => handle_app_error(error),
    }
}

#[utoipa::path(
    delete, path = "/api/v1/inventory/fhrp-group-assignments/{id}",
    operation_id = "delete_inventory_fhrp_group_assignment", tag = "inventory",
    params(("id" = String, Path, description = "FHRP group assignment ID")),
    responses(
        (status = 204, description = "FHRP group assignment deleted"),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Inventory delete denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_fhrp_group_assignment(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let context = build_request_context(&request_id, Some(auth_context), &headers);
    let id = match parse_uuid(&id, "fhrp_group_assignment_id") {
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
    match state
        .repositories
        .fhrp_group_assignments
        .get_by_id(id)
        .await
    {
        Ok(Some(_)) => (),
        Ok(None) => {
            return handle_app_error(AppError::NotFound("FHRP group assignment not found".into()));
        }
        Err(error) => return handle_app_error(error),
    }
    match state.repositories.fhrp_group_assignments.delete(id).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => handle_app_error(error),
    }
}
