//! Rack roles REST routes — full CRUD + bulk.
//!
//! Handlers never run SQL directly; all persistence goes through
//! `RackRoleRepository` on `AppState`.

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
use crate::contexts::inventory::infrastructure::persistence::repositories::derive_slug;
use crate::contexts::inventory::infrastructure::persistence::repositories::dcim::rack_role_repository::RACK_ROLE_COLUMNS;
use crate::contexts::inventory::infrastructure::persistence::{
    BulkCreateResult, NewRackRole, RackRoleRecord, RackRoleRepository, UpdateRackRole,
};
use crate::contexts::shared::list_query::SortSpec;
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

use super::super::{InventoryListEnvelope, ensure_access, parse_required_string, parse_uuid};

/// Query parameters for rack role listing.
#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct RackRoleListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct RackRoleCreatePayload {
    pub name: String,
    pub slug: Option<String>,
    pub color: Option<String>,
    pub description: Option<String>,
    pub custom_fields: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct RackRoleUpdatePayload {
    pub name: Option<String>,
    pub slug: Option<String>,
    pub color: Option<String>,
    pub description: Option<String>,
    pub custom_fields: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct RackRoleBulkCreatePayload {
    #[serde(deserialize_with = "crate::api::rest::routes::validation::deserialize_bulk_items")]
    #[schema(min_items = 1)]
    pub items: Vec<RackRoleCreatePayload>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RackRoleResponse {
    pub id: Uuid,
    pub external_id: Option<i64>,
    pub name: String,
    pub slug: String,
    pub color: Option<String>,
    pub description: Option<String>,
    pub owner_id: Option<Uuid>,
    pub tags: Vec<String>,
    pub custom_fields: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkRackRoleResponse {
    pub created: Vec<RackRoleResponse>,
    pub errors: Vec<String>,
}

fn map_rack_role(record: RackRoleRecord) -> RackRoleResponse {
    RackRoleResponse {
        id: record.id,
        external_id: record.external_id,
        name: record.name,
        slug: record.slug,
        color: record.color,
        description: record.description,
        owner_id: record.owner_id,
        tags: record.tags,
        custom_fields: record.custom_fields,
    }
}

fn map_bulk_rack_role_result(result: BulkCreateResult<RackRoleRecord>) -> BulkRackRoleResponse {
    BulkRackRoleResponse {
        created: result.created.into_iter().map(map_rack_role).collect(),
        errors: result.errors,
    }
}

fn resolve_slug(slug: Option<String>, name: &str) -> String {
    slug.map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| derive_slug(name))
}

/// List rack roles.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/rack_roles",
    tag = "inventory",
    params(RackRoleListQuery),
    responses(
        (status = 200, description = "Rack roles", body = InventoryListEnvelope<RackRoleResponse>),
        (status = "4XX", description = "Rack role list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Rack role list failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_rack_roles(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<RackRoleListQuery>,
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
        RACK_ROLE_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .rack_roles
        .list(
            query.limit,
            query.offset,
            crate::contexts::shared::list_query::search_pattern(query.search.as_deref()),
            sort,
        )
        .await
    {
        Ok((records, total)) => {
            let items: Vec<RackRoleResponse> = records.into_iter().map(map_rack_role).collect();
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

/// Get a rack role.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/rack_roles/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Rack role id"),
    ),
    responses(
        (status = 200, description = "Rack role", body = RackRoleResponse),
        (status = 404, description = "Rack role not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Rack role get request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Rack role get failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_rack_role(
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
    match state.repositories.rack_roles.get_by_id(id).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_rack_role(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Rack role not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a rack role.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/rack_roles",
    tag = "inventory",
    request_body = RackRoleCreatePayload,
    responses(
        (status = 201, description = "Rack role created", body = RackRoleResponse),
        (status = "4XX", description = "Rack role create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Rack role create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_rack_role(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<RackRoleCreatePayload>,
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
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let slug = resolve_slug(payload.slug, &name);

    let new_role = NewRackRole {
        id: Uuid::now_v7(),
        name,
        slug,
        color: payload.color,
        description: payload.description,
        owner_id: None,
        custom_fields: payload.custom_fields,
    };

    match state.repositories.rack_roles.create(&new_role).await {
        Ok(record) => (StatusCode::CREATED, Json(map_rack_role(record))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update a rack role.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/rack_roles/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Rack role id"),
    ),
    request_body = RackRoleUpdatePayload,
    responses(
        (status = 200, description = "Rack role updated", body = RackRoleResponse),
        (status = 404, description = "Rack role not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Rack role update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Rack role update failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_rack_role(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<RackRoleUpdatePayload>,
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

    let changes = UpdateRackRole {
        name: payload.name,
        slug: payload.slug,
        color: payload.color,
        description: payload.description,
        custom_fields: payload.custom_fields,
    };

    match state.repositories.rack_roles.update(id, &changes).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_rack_role(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Rack role not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a rack role.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/rack_roles/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Rack role id"),
    ),
    responses(
        (status = 204, description = "Rack role deleted"),
        (status = "4XX", description = "Rack role delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Rack role delete failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_rack_role(
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
    match state.repositories.rack_roles.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create rack roles.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/rack_roles/bulk",
    tag = "inventory",
    request_body = RackRoleBulkCreatePayload,
    responses(
        (status = 200, description = "Bulk rack role create result", body = BulkRackRoleResponse),
        (status = "4XX", description = "Bulk rack role create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Bulk rack role create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_rack_roles(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<RackRoleBulkCreatePayload>,
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
            Ok(value) => value,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        let slug = resolve_slug(p.slug, &name);
        items.push(NewRackRole {
            id: Uuid::now_v7(),
            name,
            slug,
            color: p.color,
            description: p.description,
            owner_id: None,
            custom_fields: p.custom_fields,
        });
    }

    match state.repositories.rack_roles.bulk_create(&items).await {
        Ok(result) => (StatusCode::OK, Json(map_bulk_rack_role_result(result))).into_response(),
        Err(error) => handle_app_error(error),
    }
}
