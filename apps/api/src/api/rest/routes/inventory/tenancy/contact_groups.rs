//! ContactGroups REST routes — full CRUD.
//!
//! Handlers never run SQL directly; all persistence goes through `ContactGroupRepository` on `AppState`.

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
use crate::contexts::inventory::infrastructure::persistence::{
    BulkContactGroupResult, CONTACT_GROUP_COLUMNS, ContactGroupRecord, ContactGroupRepository,
    NewContactGroup, UpdateContactGroup,
};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_uuids};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

use super::super::{
    InventoryListEnvelope, ensure_access, parse_optional_uuid, parse_required_string, parse_uuid,
};

#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct ContactGroupListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    pub parent_id: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ContactGroupCreatePayload {
    pub name: String,
    pub slug: String,
    pub parent_id: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ContactGroupUpdatePayload {
    pub name: Option<String>,
    pub slug: Option<String>,
    pub parent_id: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ContactGroupBulkCreatePayload {
    #[serde(deserialize_with = "crate::api::rest::routes::validation::deserialize_bulk_items")]
    #[schema(min_items = 1)]
    pub items: Vec<ContactGroupCreatePayload>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ContactGroupResponse {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub parent_id: Option<Uuid>,
    pub description: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkContactGroupResponse {
    pub created: Vec<ContactGroupResponse>,
    pub errors: Vec<String>,
}

fn map_contact_group(record: ContactGroupRecord) -> ContactGroupResponse {
    ContactGroupResponse {
        id: record.id,
        name: record.name,
        slug: record.slug,
        parent_id: record.parent_id,
        description: record.description,
    }
}

fn map_bulk_contact_group_result(result: BulkContactGroupResult) -> BulkContactGroupResponse {
    BulkContactGroupResponse {
        created: result.created.into_iter().map(map_contact_group).collect(),
        errors: result.errors,
    }
}

/// List contact groups.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/contact-groups",
    tag = "inventory",
    params(ContactGroupListQuery),
    responses(
        (status = 200, description = "Contact groups", body = InventoryListEnvelope<ContactGroupResponse>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_contact_groups(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ContactGroupListQuery>,
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

    let parent_ids = match parse_csv_uuids(query.parent_id.as_deref(), "parent_id") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let resolved_limit = clamp_limit(query.limit);
    let resolved_offset = clamp_offset(query.offset);
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        CONTACT_GROUP_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match state
        .repositories
        .contact_groups
        .list(
            &parent_ids,
            crate::contexts::shared::list_query::search_pattern(query.search.as_deref()),
            query.limit,
            query.offset,
            sort,
        )
        .await
    {
        Ok((records, total)) => {
            let items: Vec<ContactGroupResponse> =
                records.into_iter().map(map_contact_group).collect();
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

/// Get a contact group.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/contact-groups/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Contact group ID"),
    ),
    responses(
        (status = 200, description = "Contact group", body = ContactGroupResponse),
        (status = 404, description = "Contact group not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_contact_group(
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
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    match state.repositories.contact_groups.get_by_id(id).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_contact_group(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("ContactGroup not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a contact group.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/contact-groups",
    tag = "inventory",
    request_body = ContactGroupCreatePayload,
    responses(
        (status = 201, description = "Contact group created", body = ContactGroupResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_contact_group(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<ContactGroupCreatePayload>,
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
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let slug = match parse_required_string(&payload.slug, "slug") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let parent_id = match parse_optional_uuid(payload.parent_id, "parent_id") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };

    let new_group = NewContactGroup {
        id: Uuid::now_v7(),
        name,
        slug,
        parent_id,
        description: payload.description,
    };

    match state.repositories.contact_groups.create(&new_group).await {
        Ok(record) => (StatusCode::CREATED, Json(map_contact_group(record))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update a contact group.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/contact-groups/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Contact group ID"),
    ),
    request_body = ContactGroupUpdatePayload,
    responses(
        (status = 200, description = "Contact group updated", body = ContactGroupResponse),
        (status = 404, description = "Contact group not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_contact_group(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<ContactGroupUpdatePayload>,
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
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let parent_id = match parse_optional_uuid(payload.parent_id, "parent_id") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };

    let changes = UpdateContactGroup {
        name: payload.name,
        slug: payload.slug,
        parent_id,
        description: payload.description,
    };

    match state.repositories.contact_groups.update(id, &changes).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_contact_group(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("ContactGroup not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a contact group.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/contact-groups/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Contact group ID"),
    ),
    responses(
        (status = 204, description = "Contact group deleted"),
    ),
)]
pub async fn delete_contact_group(
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
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    match state.repositories.contact_groups.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create contact groups.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/contact-groups/bulk",
    tag = "inventory",
    request_body = ContactGroupBulkCreatePayload,
    responses(
        (status = 200, description = "Bulk contact group create result", body = BulkContactGroupResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_contact_groups(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<ContactGroupBulkCreatePayload>,
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
            Ok(v) => v,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        let slug = match parse_required_string(&p.slug, "slug") {
            Ok(v) => v,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        let parent_id = match parse_optional_uuid(p.parent_id, "parent_id") {
            Ok(v) => v,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        items.push(NewContactGroup {
            id: Uuid::now_v7(),
            name,
            slug,
            parent_id,
            description: p.description,
        });
    }

    match state.repositories.contact_groups.bulk_create(&items).await {
        Ok(result) => (StatusCode::OK, Json(map_bulk_contact_group_result(result))).into_response(),
        Err(error) => handle_app_error(error),
    }
}
