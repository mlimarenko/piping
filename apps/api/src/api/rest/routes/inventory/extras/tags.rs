use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};

use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::permission_helpers::ensure_inventory_collection_access;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::application::tag_authorization::RequestTagAuthorizer;
use crate::contexts::inventory::domain::extras::tag::Tag;
use crate::contexts::inventory::infrastructure::persistence::repositories::extras::tag_repository::{TAG_COLUMNS, TagFilter, NewTag, UpdateTag};
use crate::contexts::shared::list_query::{SortSpec, search_pattern, parse_id_filter, parse_csv_strings};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;
use uuid::Uuid;

use super::super::{BulkCreateResponse, InventoryListEnvelope, parse_uuid, push_bulk_error};

async fn authorize_tag_attributes(
    state: &AppState,
    context: &RequestContext,
    action: Action,
    resource: &str,
    mut attributes: serde_json::Value,
) -> Result<(), AppError> {
    attributes["object_type"] = serde_json::json!("extras.tag");
    let attributes: std::collections::HashMap<String, serde_json::Value> =
        serde_json::from_value(attributes)?;
    state
        .services
        .policy_evaluation
        .ensure_access_with_attributes(
            context,
            action,
            ResourceType::Inventory,
            Some(resource),
            Some(attributes),
        )
        .await
}

async fn authorize_tag(
    state: &AppState,
    context: &RequestContext,
    action: Action,
    tag: &Tag,
) -> Result<(), AppError> {
    authorize_tag_attributes(
        state,
        context,
        action,
        &format!("inventory:{}", tag.id),
        serde_json::to_value(tag)?,
    )
    .await
}

async fn load_authorized_tag(
    state: &AppState,
    context: &RequestContext,
    action: Action,
    id: Uuid,
) -> Result<Tag, AppError> {
    let tag = state.repositories.tags.get_by_id(id).await?;
    let Some(tag) = tag else {
        // Preserve the resource/token gate for missing identities without
        // manufacturing any persisted Tag attributes.
        authorize_tag_attributes(
            state,
            context,
            action,
            &format!("inventory:{id}"),
            serde_json::json!({}),
        )
        .await?;
        return Err(AppError::NotFound("Tag not found".into()));
    };
    authorize_tag(state, context, action, &tag).await?;
    Ok(tag)
}

async fn authorize_new_tag(
    state: &AppState,
    context: &RequestContext,
    tag: &NewTag,
) -> Result<(), AppError> {
    let mut object_types = tag.object_types.clone();
    object_types.sort();
    let attributes = serde_json::json!({
        "name": tag.name, "slug": tag.slug, "color": tag.color,
        "description": tag.description, "weight": tag.weight, "object_types": object_types,
    });
    authorize_tag_attributes(state, context, Action::Create, "inventory:*", attributes).await
}

#[derive(Debug, serde::Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct TagListQuery {
    pub ids: Option<String>,
    pub name: Option<String>,
    pub slug: Option<String>,
    pub color: Option<String>,
    pub object_type: Option<String>,
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
    if let Err(error) = ensure_inventory_collection_access(&state, &context, Action::Read).await {
        return handle_app_error(error);
    }
    let sort =
        match SortSpec::parse_columns(query.sort.as_deref(), query.order.as_deref(), TAG_COLUMNS) {
            Ok(sort) => sort,
            Err(error) => return handle_app_error(error),
        };
    let ids = match parse_id_filter(query.ids.as_deref()) {
        Ok(ids) => ids,
        Err(error) => return handle_app_error(error),
    };
    let filter = TagFilter {
        ids,
        names: parse_csv_strings(query.name.as_deref()),
        slugs: parse_csv_strings(query.slug.as_deref()),
        colors: parse_csv_strings(query.color.as_deref()),
        object_type: query.object_type,
    };
    match state
        .repositories
        .tags
        .list_candidates(&filter, search_pattern(query.search.as_deref()), sort)
        .await
    {
        Ok(candidates) => {
            let mut visible = Vec::new();
            for tag in candidates {
                match authorize_tag(&state, &context, Action::Read, &tag).await {
                    Ok(()) => visible.push(tag),
                    Err(AppError::Forbidden(_)) => continue,
                    Err(error) => return handle_app_error(error),
                }
            }
            let total = match i64::try_from(visible.len()) {
                Ok(total) => total,
                Err(_) => return handle_app_error(AppError::Internal("Too many tags".into())),
            };
            let limit = clamp_limit(query.limit);
            let offset = clamp_offset(query.offset);
            let items = visible
                .into_iter()
                .skip(offset as usize)
                .take(limit as usize)
                .collect();
            (
                StatusCode::OK,
                Json(InventoryListEnvelope {
                    items,
                    total,
                    limit,
                    offset,
                }),
            )
                .into_response()
        }
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
    let id = match parse_uuid(&id, "tag_id") {
        Ok(id) => id,
        Err(error) => return handle_app_error(error),
    };
    match load_authorized_tag(&state, &context, Action::Read, id).await {
        Ok(tag) => (StatusCode::OK, Json(tag)).into_response(),
        Err(error) => handle_app_error(error),
    }
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TagBulkCreatePayload {
    #[serde(deserialize_with = "crate::api::rest::routes::validation::deserialize_bulk_items")]
    #[schema(min_items = 1)]
    pub items: Vec<NewTag>,
}

#[utoipa::path(post, path = "/api/v1/inventory/tags", tag = "inventory", request_body = NewTag, responses((status = 201, description = "Resource created", body = Tag), (status = "4XX", description = "Client error", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", description = "Server error", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn create_tag(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<NewTag>,
) -> impl IntoResponse {
    let context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_inventory_collection_access(&state, &context, Action::Create).await {
        return handle_app_error(error);
    }
    if let Err(error) = authorize_new_tag(&state, &context, &payload).await {
        return handle_app_error(error);
    }
    match state.repositories.tags.create(&payload).await {
        Ok(tag) => (StatusCode::CREATED, Json(tag)).into_response(),
        Err(error) => handle_app_error(error),
    }
}

#[utoipa::path(put, path = "/api/v1/inventory/tags/{id}", tag = "inventory", params(("id" = String, Path)), request_body = UpdateTag, responses((status = 200, description = "Successful response", body = Tag), (status = "4XX", description = "Client error", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", description = "Server error", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn update_tag(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<UpdateTag>,
) -> impl IntoResponse {
    let context = build_request_context(&request_id, Some(auth_context), &headers);
    let result = async {
        let id = parse_uuid(&id, "tag_id")?;
        let mut authorizer = RequestTagAuthorizer {
            policy_evaluation: &state.services.policy_evaluation,
            user_api_tokens: &state.services.user_api_tokens,
            context: &context,
            action: Action::Update,
        };
        let updated = state
            .repositories
            .tags
            .update(id, &payload, &mut authorizer)
            .await?;
        match updated {
            Some(tag) => Ok(tag),
            None => {
                authorize_tag_attributes(
                    &state,
                    &context,
                    Action::Update,
                    &format!("inventory:{id}"),
                    serde_json::json!({}),
                )
                .await?;
                Err(AppError::NotFound("Tag not found".into()))
            }
        }
    }
    .await;
    match result {
        Ok(tag) => (StatusCode::OK, Json(tag)).into_response(),
        Err(error) => handle_app_error(error),
    }
}

#[utoipa::path(delete, path = "/api/v1/inventory/tags/{id}", tag = "inventory", params(("id" = String, Path)), responses((status = 204, description = "Resource deleted"), (status = "4XX", description = "Client error", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", description = "Server error", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn delete_tag(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let context = build_request_context(&request_id, Some(auth_context), &headers);
    let result = async {
        let id = parse_uuid(&id, "tag_id")?;
        let mut authorizer = RequestTagAuthorizer {
            policy_evaluation: &state.services.policy_evaluation,
            user_api_tokens: &state.services.user_api_tokens,
            context: &context,
            action: Action::Delete,
        };
        if state.repositories.tags.delete(id, &mut authorizer).await? == 0 {
            authorize_tag_attributes(
                &state,
                &context,
                Action::Delete,
                &format!("inventory:{id}"),
                serde_json::json!({}),
            )
            .await?;
            return Err(AppError::NotFound("Tag not found".into()));
        }
        Ok(())
    }
    .await;
    match result {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(
            crate::api::rest::request_context::project_inventory_delete_error(
                &state, &context, error,
            )
            .await,
        ),
    }
}

#[utoipa::path(post, path = "/api/v1/inventory/tags/bulk", tag = "inventory", request_body = TagBulkCreatePayload, responses((status = 200, description = "Successful response", body = BulkCreateResponse<Tag>), (status = "4XX", description = "Client error", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", description = "Server error", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn bulk_create_tags(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<TagBulkCreatePayload>,
) -> impl IntoResponse {
    let context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_inventory_collection_access(&state, &context, Action::Create).await {
        return handle_app_error(error);
    }
    let mut created = Vec::new();
    let mut errors = Vec::new();
    for (index, item) in payload.items.into_iter().enumerate() {
        if let Err(error) = authorize_new_tag(&state, &context, &item).await {
            push_bulk_error(&mut errors, index, error);
            continue;
        }
        match state.repositories.tags.create(&item).await {
            Ok(tag) => created.push(tag),
            Err(error) => push_bulk_error(&mut errors, index, error),
        }
    }
    (StatusCode::OK, Json(BulkCreateResponse { created, errors })).into_response()
}
