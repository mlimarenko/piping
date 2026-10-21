use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};

use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::{Path, Query};
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::domain::dcim::module_type_profile::ModuleTypeProfile;
use crate::contexts::inventory::infrastructure::persistence::repositories::dcim::module_type_profile_repository::MODULE_TYPE_PROFILE_COLUMNS;
use crate::contexts::shared::list_query::{SortSpec, search_pattern};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

use super::super::{InventoryListEnvelope, ensure_access, parse_uuid};

#[derive(Debug, serde::Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct ModuleTypeProfileListQuery {
    pub search: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

/// List module type profiles.
#[utoipa::path(
    get, path = "/api/v1/inventory/module-type-profiles", tag = "inventory",
    params(ModuleTypeProfileListQuery),
    responses(
        (status = 200, description = "Module type profiles", body = InventoryListEnvelope<ModuleTypeProfile>),
        (status = "4XX", description = "Request rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_module_type_profiles(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ModuleTypeProfileListQuery>,
) -> impl IntoResponse {
    let context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(&state, &context, Action::Read, ResourceType::Inventory).await
    {
        return handle_app_error(error);
    }
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        MODULE_TYPE_PROFILE_COLUMNS,
    ) {
        Ok(sort) => sort,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .module_type_profiles
        .list_paginated(
            search_pattern(query.search.as_deref()),
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
                limit: clamp_limit(query.limit),
                offset: clamp_offset(query.offset),
            }),
        )
            .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Get a module type profile.
#[utoipa::path(
    get, path = "/api/v1/inventory/module-type-profiles/{id}", tag = "inventory",
    params(("id" = String, Path, description = "Module type profile id")),
    responses(
        (status = 200, description = "Module type profile", body = ModuleTypeProfile),
        (status = 404, description = "Profile not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Request rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_module_type_profile(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(&state, &context, Action::Read, ResourceType::Inventory).await
    {
        return handle_app_error(error);
    }
    let id = match parse_uuid(&id, "module_type_profile_id") {
        Ok(id) => id,
        Err(error) => return handle_app_error(error),
    };
    match state.repositories.module_type_profiles.get_by_id(id).await {
        Ok(Some(profile)) => (StatusCode::OK, Json(profile)).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Module type profile not found".into())),
        Err(error) => handle_app_error(error),
    }
}
