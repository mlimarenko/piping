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
use crate::contexts::inventory::domain::extras::tag::Tag;
use crate::contexts::inventory::infrastructure::persistence::repositories::extras::tag_repository::TAG_COLUMNS;
use crate::contexts::shared::list_query::{SortSpec, search_pattern};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

use super::super::{InventoryListEnvelope, ensure_access, parse_uuid};

#[derive(Debug, serde::Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct TagListQuery {
    pub search: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

/// List tags.
#[utoipa::path(
    get, path = "/api/v1/inventory/tags", tag = "inventory",
    params(TagListQuery),
    responses(
        (status = 200, description = "Tags", body = InventoryListEnvelope<Tag>),
        (status = "4XX", description = "Request rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_tags(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<TagListQuery>,
) -> impl IntoResponse {
    let context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(&state, &context, Action::Read, ResourceType::Inventory).await
    {
        return handle_app_error(error);
    }
    let sort =
        match SortSpec::parse_columns(query.sort.as_deref(), query.order.as_deref(), TAG_COLUMNS) {
            Ok(sort) => sort,
            Err(error) => return handle_app_error(error),
        };
    match state
        .repositories
        .tags
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

/// Get a tag.
#[utoipa::path(
    get, path = "/api/v1/inventory/tags/{id}", tag = "inventory",
    params(("id" = String, Path, description = "Tag id")),
    responses(
        (status = 200, description = "Tag", body = Tag),
        (status = 404, description = "Tag not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Request rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_tag(
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
    let id = match parse_uuid(&id, "tag_id") {
        Ok(id) => id,
        Err(error) => return handle_app_error(error),
    };
    match state.repositories.tags.get_by_id(id).await {
        Ok(Some(tag)) => (StatusCode::OK, Json(tag)).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Tag not found".into())),
        Err(error) => handle_app_error(error),
    }
}
