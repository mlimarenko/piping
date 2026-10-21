//! Platforms REST routes — full CRUD.
//!
//! Handlers never run SQL directly; all persistence goes through `PlatformRepository` on `AppState`.

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
    BulkCreateResult, NewPlatform, PLATFORM_COLUMNS, PlatformListItem, PlatformRecord,
    PlatformRepository, SortSpec, UpdatePlatform, parse_csv_uuids,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

use super::super::{
    InventoryListEnvelope, ensure_access, parse_optional_uuid, parse_required_string,
};

#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct PlatformListQuery {
    pub manufacturer_id: Option<String>,
    pub search: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct PlatformCreatePayload {
    pub name: String,
    pub slug: String,
    pub manufacturer_id: Option<String>,
    pub napalm_driver: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct PlatformUpdatePayload {
    pub name: Option<String>,
    pub slug: Option<String>,
    pub manufacturer_id: Option<String>,
    pub napalm_driver: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct PlatformBulkCreatePayload {
    #[serde(deserialize_with = "crate::api::rest::routes::validation::deserialize_bulk_items")]
    #[schema(min_items = 1)]
    pub items: Vec<PlatformCreatePayload>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PlatformResponse {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub manufacturer_id: Option<Uuid>,
    pub napalm_driver: Option<String>,
    pub description: Option<String>,
}

/// Hydrated platform list item — extends `PlatformResponse` with the
/// manufacturer name for the collection list endpoint.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PlatformListItemResponse {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub manufacturer_id: Option<Uuid>,
    pub manufacturer_name: Option<String>,
    pub napalm_driver: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkPlatformResponse {
    pub created: Vec<PlatformResponse>,
    pub errors: Vec<String>,
}

fn map_platform_list_item(item: PlatformListItem) -> PlatformListItemResponse {
    let PlatformListItem {
        record,
        manufacturer_name,
    } = item;
    PlatformListItemResponse {
        id: record.id,
        name: record.name,
        slug: record.slug,
        manufacturer_id: record.manufacturer_id,
        manufacturer_name,
        napalm_driver: record.napalm_driver,
        description: record.description,
    }
}

fn map_platform(record: PlatformRecord) -> PlatformResponse {
    PlatformResponse {
        id: record.id,
        name: record.name,
        slug: record.slug,
        manufacturer_id: record.manufacturer_id,
        napalm_driver: record.napalm_driver,
        description: record.description,
    }
}

fn map_bulk_platform_result(result: BulkCreateResult<PlatformRecord>) -> BulkPlatformResponse {
    BulkPlatformResponse {
        created: result.created.into_iter().map(map_platform).collect(),
        errors: result.errors,
    }
}

/// List platforms.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/platforms",
    tag = "inventory",
    params(PlatformListQuery),
    responses(
        (status = 200, description = "Platforms", body = InventoryListEnvelope<PlatformListItemResponse>),
        (status = "4XX", description = "Platform list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Platform list failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_platforms(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<PlatformListQuery>,
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

    let manufacturer_id = match parse_csv_uuids(query.manufacturer_id.as_deref(), "manufacturer_id")
    {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        PLATFORM_COLUMNS,
    ) {
        Ok(sort) => sort,
        Err(error) => return handle_app_error(error),
    };
    let resolved_limit = clamp_limit(query.limit);
    let resolved_offset = clamp_offset(query.offset);

    match state
        .repositories
        .platforms
        .list_hydrated(
            manufacturer_id,
            query.search,
            sort,
            query.limit,
            query.offset,
        )
        .await
    {
        Ok((records, total)) => {
            let items: Vec<PlatformListItemResponse> =
                records.into_iter().map(map_platform_list_item).collect();
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

/// Get a platform.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/platforms/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Platform id"),
    ),
    responses(
        (status = 200, description = "Platform", body = PlatformResponse),
        (status = 404, description = "Platform not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Platform get request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Platform get failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_platform(
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
    let id = match id.parse::<Uuid>() {
        Ok(v) => v,
        Err(_) => return handle_app_error(AppError::Validation("invalid id".to_string())),
    };
    match state.repositories.platforms.get_by_id(id).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_platform(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Platform not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a platform.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/platforms",
    tag = "inventory",
    request_body = PlatformCreatePayload,
    responses(
        (status = 201, description = "Platform created", body = PlatformResponse),
        (status = "4XX", description = "Platform create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Platform create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_platform(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<PlatformCreatePayload>,
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
    let manufacturer_id = match parse_optional_uuid(payload.manufacturer_id, "manufacturer_id") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };

    let new_platform = NewPlatform {
        id: Uuid::now_v7(),
        name,
        slug,
        manufacturer_id,
        napalm_driver: payload.napalm_driver,
        description: payload.description,
    };

    match state.repositories.platforms.create(&new_platform).await {
        Ok(record) => (StatusCode::CREATED, Json(map_platform(record))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update a platform.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/platforms/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Platform id"),
    ),
    request_body = PlatformUpdatePayload,
    responses(
        (status = 200, description = "Platform updated", body = PlatformResponse),
        (status = 404, description = "Platform not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Platform update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Platform update failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_platform(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<PlatformUpdatePayload>,
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
    let id = match id.parse::<Uuid>() {
        Ok(v) => v,
        Err(_) => return handle_app_error(AppError::Validation("invalid id".to_string())),
    };
    let manufacturer_id = match parse_optional_uuid(payload.manufacturer_id, "manufacturer_id") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };

    let changes = UpdatePlatform {
        name: payload.name,
        slug: payload.slug,
        manufacturer_id,
        napalm_driver: payload.napalm_driver,
        description: payload.description,
    };

    match state.repositories.platforms.update(id, &changes).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_platform(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Platform not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a platform.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/platforms/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Platform id"),
    ),
    responses(
        (status = 204, description = "Platform deleted"),
        (status = 404, description = "Platform not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Platform delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Platform delete failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_platform(
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
    let id = match id.parse::<Uuid>() {
        Ok(v) => v,
        Err(_) => return handle_app_error(AppError::Validation("invalid id".to_string())),
    };
    match state.repositories.platforms.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create platforms.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/platforms/bulk",
    tag = "inventory",
    request_body = PlatformBulkCreatePayload,
    responses(
        (status = 200, description = "Bulk platform create result", body = BulkPlatformResponse),
        (status = "4XX", description = "Bulk platform create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Bulk platform create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_platforms(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<PlatformBulkCreatePayload>,
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
        let manufacturer_id = match parse_optional_uuid(p.manufacturer_id, "manufacturer_id") {
            Ok(v) => v,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        items.push(NewPlatform {
            id: Uuid::now_v7(),
            name,
            slug,
            manufacturer_id,
            napalm_driver: p.napalm_driver,
            description: p.description,
        });
    }

    match state.repositories.platforms.bulk_create(&items).await {
        Ok(result) => (StatusCode::OK, Json(map_bulk_platform_result(result))).into_response(),
        Err(error) => handle_app_error(error),
    }
}
