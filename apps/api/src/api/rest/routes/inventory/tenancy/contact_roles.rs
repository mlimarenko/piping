//! ContactRoles REST routes — full CRUD.
//!
//! Handlers never run SQL directly; all persistence goes through `ContactRoleRepository` on `AppState`.

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
    BulkCreateResult, CONTACT_ROLE_COLUMNS, ContactRoleRecord, ContactRoleRepository,
    NewContactRole, UpdateContactRole,
};
use crate::contexts::shared::list_query::SortSpec;
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

use super::super::{InventoryListEnvelope, ensure_access, parse_required_string, parse_uuid};

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ContactRoleListQuery {
    pub search: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ContactRoleCreatePayload {
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ContactRoleUpdatePayload {
    pub name: Option<String>,
    pub slug: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ContactRoleBulkCreatePayload {
    #[serde(deserialize_with = "crate::api::rest::routes::validation::deserialize_bulk_items")]
    #[schema(min_items = 1)]
    pub items: Vec<ContactRoleCreatePayload>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ContactRoleResponse {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkContactRoleResponse {
    pub created: Vec<ContactRoleResponse>,
    pub errors: Vec<String>,
}

fn map_contact_role(record: ContactRoleRecord) -> ContactRoleResponse {
    ContactRoleResponse {
        id: record.id,
        name: record.name,
        slug: record.slug,
        description: record.description,
    }
}

fn map_bulk_contact_role_result(
    result: BulkCreateResult<ContactRoleRecord>,
) -> BulkContactRoleResponse {
    BulkContactRoleResponse {
        created: result.created.into_iter().map(map_contact_role).collect(),
        errors: result.errors,
    }
}

/// List contact roles.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/contact-roles",
    tag = "inventory",
    params(
        ("search" = Option<String>, Query, description = "Search text"),
        ("limit" = Option<u32>, Query, description = "Maximum number of contact roles to return"),
        ("offset" = Option<u32>, Query, description = "Number of contact roles to skip"),
    ),
    responses(
        (status = 200, description = "Contact roles listed", body = InventoryListEnvelope<ContactRoleResponse>),
        (status = "4XX", description = "Contact role list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Contact role list request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_contact_roles(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ContactRoleListQuery>,
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

    let resolved_limit = clamp_limit(query.limit);
    let resolved_offset = clamp_offset(query.offset);
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        CONTACT_ROLE_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .contact_roles
        .list(query.search, query.limit, query.offset, sort)
        .await
    {
        Ok((records, total)) => {
            let items: Vec<ContactRoleResponse> =
                records.into_iter().map(map_contact_role).collect();
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

/// Get a contact role.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/contact-roles/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Contact role id"),
    ),
    responses(
        (status = 200, description = "Contact role", body = ContactRoleResponse),
        (status = 404, description = "Contact role not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Contact role request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Contact role request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_contact_role(
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
    match state.repositories.contact_roles.get_by_id(id).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_contact_role(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("ContactRole not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a contact role.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/contact-roles",
    tag = "inventory",
    request_body = ContactRoleCreatePayload,
    responses(
        (status = 201, description = "Contact role created", body = ContactRoleResponse),
        (status = "4XX", description = "Contact role create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Contact role create request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_contact_role(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<ContactRoleCreatePayload>,
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

    let new_role = NewContactRole {
        id: Uuid::now_v7(),
        name,
        slug,
        description: payload.description,
    };

    match state.repositories.contact_roles.create(&new_role).await {
        Ok(record) => (StatusCode::CREATED, Json(map_contact_role(record))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update a contact role.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/contact-roles/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Contact role id"),
    ),
    request_body = ContactRoleUpdatePayload,
    responses(
        (status = 200, description = "Contact role updated", body = ContactRoleResponse),
        (status = 404, description = "Contact role not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Contact role update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Contact role update request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_contact_role(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<ContactRoleUpdatePayload>,
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

    let changes = UpdateContactRole {
        name: payload.name,
        slug: payload.slug,
        description: payload.description,
    };

    match state.repositories.contact_roles.update(id, &changes).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_contact_role(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("ContactRole not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a contact role.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/contact-roles/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Contact role id"),
    ),
    responses(
        (status = 204, description = "Contact role deleted"),
        (status = 404, description = "Contact role not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Contact role delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Contact role delete request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_contact_role(
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
    match state.repositories.contact_roles.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create contact roles.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/contact-roles/bulk",
    tag = "inventory",
    request_body = ContactRoleBulkCreatePayload,
    responses(
        (status = 200, description = "Contact roles bulk-created", body = BulkContactRoleResponse),
        (status = "4XX", description = "Contact roles bulk-create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Contact roles bulk-create request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_contact_roles(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<ContactRoleBulkCreatePayload>,
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
        items.push(NewContactRole {
            id: Uuid::now_v7(),
            name,
            slug,
            description: p.description,
        });
    }

    match state.repositories.contact_roles.bulk_create(&items).await {
        Ok(result) => (StatusCode::OK, Json(map_bulk_contact_role_result(result))).into_response(),
        Err(error) => handle_app_error(error),
    }
}
