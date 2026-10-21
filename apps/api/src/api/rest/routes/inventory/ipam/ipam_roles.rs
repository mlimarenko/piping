//! IPAM roles REST routes — full CRUD + bulk.
//!
//! The shared prefix/VLAN role (distinct from DCIM device roles). Handlers never
//! run SQL directly; all persistence goes through `IpamRoleRepository` on
//! `AppState`.

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
use crate::contexts::inventory::infrastructure::persistence::{
    BulkCreateResult, IPAM_ROLE_COLUMNS, IpamRoleRecord, IpamRoleRepository, NewIpamRole,
    UpdateIpamRole,
};
use crate::contexts::shared::list_query::SortSpec;
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

use super::super::{InventoryListEnvelope, ensure_access, parse_required_string, parse_uuid};

/// Query parameters for IPAM role listing.
#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct IpamRoleListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct IpamRoleCreatePayload {
    pub name: String,
    pub slug: Option<String>,
    pub weight: Option<i16>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct IpamRoleUpdatePayload {
    pub name: Option<String>,
    pub slug: Option<String>,
    pub weight: Option<i16>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct IpamRoleBulkCreatePayload {
    #[serde(deserialize_with = "crate::api::rest::routes::validation::deserialize_bulk_items")]
    #[schema(min_items = 1)]
    pub items: Vec<IpamRoleCreatePayload>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct IpamRoleResponse {
    pub id: Uuid,
    pub external_id: Option<i64>,
    pub name: String,
    pub slug: String,
    pub weight: i16,
    pub description: Option<String>,
    pub owner_id: Option<Uuid>,
    pub tags: Vec<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkIpamRoleResponse {
    pub created: Vec<IpamRoleResponse>,
    pub errors: Vec<String>,
}

fn map_ipam_role(record: IpamRoleRecord) -> IpamRoleResponse {
    IpamRoleResponse {
        id: record.id,
        external_id: record.external_id,
        name: record.name,
        slug: record.slug,
        weight: record.weight,
        description: record.description,
        owner_id: record.owner_id,
        tags: record.tags,
    }
}

fn map_bulk_ipam_role_result(result: BulkCreateResult<IpamRoleRecord>) -> BulkIpamRoleResponse {
    BulkIpamRoleResponse {
        created: result.created.into_iter().map(map_ipam_role).collect(),
        errors: result.errors,
    }
}

fn resolve_slug(slug: Option<String>, name: &str) -> String {
    slug.map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| derive_slug(name))
}

/// List IPAM roles.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/ipam_roles",
    tag = "inventory",
    params(IpamRoleListQuery),
    responses(
        (status = 200, description = "IPAM roles", body = InventoryListEnvelope<IpamRoleResponse>),
        (status = "4XX", description = "IPAM role list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "IPAM role list failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_ipam_roles(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<IpamRoleListQuery>,
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
        IPAM_ROLE_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .ipam_roles
        .list(
            query.limit,
            query.offset,
            crate::contexts::shared::list_query::search_pattern(query.search.as_deref()),
            sort,
        )
        .await
    {
        Ok((records, total)) => {
            let items: Vec<IpamRoleResponse> = records.into_iter().map(map_ipam_role).collect();
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

/// Get an IPAM role.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/ipam_roles/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "IPAM role id"),
    ),
    responses(
        (status = 200, description = "IPAM role", body = IpamRoleResponse),
        (status = 404, description = "IPAM role not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "IPAM role get request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "IPAM role get failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_ipam_role(
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
    match state.repositories.ipam_roles.get_by_id(id).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_ipam_role(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("IPAM role not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create an IPAM role.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/ipam_roles",
    tag = "inventory",
    request_body = IpamRoleCreatePayload,
    responses(
        (status = 201, description = "IPAM role created", body = IpamRoleResponse),
        (status = "4XX", description = "IPAM role create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "IPAM role create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_ipam_role(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<IpamRoleCreatePayload>,
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

    let new_role = NewIpamRole {
        id: Uuid::now_v7(),
        name,
        slug,
        weight: payload.weight,
        description: payload.description,
        owner_id: None,
    };

    match state.repositories.ipam_roles.create(&new_role).await {
        Ok(record) => (StatusCode::CREATED, Json(map_ipam_role(record))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update an IPAM role.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/ipam_roles/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "IPAM role id"),
    ),
    request_body = IpamRoleUpdatePayload,
    responses(
        (status = 200, description = "IPAM role updated", body = IpamRoleResponse),
        (status = 404, description = "IPAM role not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "IPAM role update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "IPAM role update failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_ipam_role(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<IpamRoleUpdatePayload>,
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

    let changes = UpdateIpamRole {
        name: payload.name,
        slug: payload.slug,
        weight: payload.weight,
        description: payload.description,
    };

    match state.repositories.ipam_roles.update(id, &changes).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_ipam_role(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("IPAM role not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Delete an IPAM role.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/ipam_roles/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "IPAM role id"),
    ),
    responses(
        (status = 204, description = "IPAM role deleted"),
        (status = "4XX", description = "IPAM role delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "IPAM role delete failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_ipam_role(
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
    match state.repositories.ipam_roles.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create IPAM roles.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/ipam_roles/bulk",
    tag = "inventory",
    request_body = IpamRoleBulkCreatePayload,
    responses(
        (status = 200, description = "Bulk IPAM role create result", body = BulkIpamRoleResponse),
        (status = "4XX", description = "Bulk IPAM role create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Bulk IPAM role create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_ipam_roles(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<IpamRoleBulkCreatePayload>,
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
        items.push(NewIpamRole {
            id: Uuid::now_v7(),
            name,
            slug,
            weight: p.weight,
            description: p.description,
            owner_id: None,
        });
    }

    match state.repositories.ipam_roles.bulk_create(&items).await {
        Ok(result) => (StatusCode::OK, Json(map_bulk_ipam_role_result(result))).into_response(),
        Err(error) => handle_app_error(error),
    }
}
