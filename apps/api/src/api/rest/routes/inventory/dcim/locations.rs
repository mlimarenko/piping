//! Locations REST routes — full CRUD + bulk.
//!
//! Handlers never run SQL directly; all persistence goes through
//! `LocationRepository` on `AppState`. The hierarchy is exposed via `parent_id`
//! (repository reads use a recursive CTE, never MPTT columns).

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
use crate::contexts::inventory::infrastructure::persistence::repositories::dcim::location_repository::LOCATION_LIST_COLUMNS;
use crate::contexts::inventory::infrastructure::persistence::{
    BulkLocationResult, LocationFilter, LocationListItem, LocationRecord, LocationRepository,
    NewLocation, UpdateLocation,
};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_strings, parse_csv_uuids};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

use super::super::{
    InventoryListEnvelope, ensure_access, parse_optional_uuid, parse_required_string, parse_uuid,
};

/// Query parameters for location listing.
#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct LocationListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    pub site_id: Option<String>,
    pub parent_id: Option<String>,
    pub status: Option<String>,
    pub tenant_id: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct LocationCreatePayload {
    pub name: String,
    pub slug: Option<String>,
    pub site_id: String,
    pub parent_id: Option<String>,
    pub status: Option<String>,
    pub tenant_id: Option<String>,
    pub facility: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct LocationUpdatePayload {
    pub name: Option<String>,
    pub slug: Option<String>,
    pub site_id: Option<String>,
    pub parent_id: Option<String>,
    pub status: Option<String>,
    pub tenant_id: Option<String>,
    pub facility: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct LocationBulkCreatePayload {
    #[serde(deserialize_with = "crate::api::rest::routes::validation::deserialize_bulk_items")]
    #[schema(min_items = 1)]
    pub items: Vec<LocationCreatePayload>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct LocationResponse {
    pub id: Uuid,
    pub external_id: Option<i64>,
    pub name: String,
    pub slug: String,
    pub site_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub status: String,
    pub tenant_id: Option<Uuid>,
    pub facility: Option<String>,
    pub description: Option<String>,
    pub owner_id: Option<Uuid>,
    pub tags: Vec<String>,
}

/// Hydrated location list item — extends `LocationResponse` with site/parent/
/// tenant names for the collection list endpoint.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct LocationListItemResponse {
    pub id: Uuid,
    pub external_id: Option<i64>,
    pub name: String,
    pub slug: String,
    pub site_id: Uuid,
    pub site_name: Option<String>,
    pub parent_id: Option<Uuid>,
    pub parent_name: Option<String>,
    pub status: String,
    pub tenant_id: Option<Uuid>,
    pub tenant_name: Option<String>,
    pub facility: Option<String>,
    pub description: Option<String>,
    pub owner_id: Option<Uuid>,
    pub tags: Vec<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkLocationResponse {
    pub created: Vec<LocationResponse>,
    pub errors: Vec<String>,
}

fn map_location_list_item(item: LocationListItem) -> LocationListItemResponse {
    let LocationListItem {
        record,
        site_name,
        parent_name,
        tenant_name,
    } = item;
    LocationListItemResponse {
        id: record.id,
        external_id: record.external_id,
        name: record.name,
        slug: record.slug,
        site_id: record.site_id,
        site_name,
        parent_id: record.parent_id,
        parent_name,
        status: record.status,
        tenant_id: record.tenant_id,
        tenant_name,
        facility: record.facility,
        description: record.description,
        owner_id: record.owner_id,
        tags: record.tags,
    }
}

fn map_location(record: LocationRecord) -> LocationResponse {
    LocationResponse {
        id: record.id,
        external_id: record.external_id,
        name: record.name,
        slug: record.slug,
        site_id: record.site_id,
        parent_id: record.parent_id,
        status: record.status,
        tenant_id: record.tenant_id,
        facility: record.facility,
        description: record.description,
        owner_id: record.owner_id,
        tags: record.tags,
    }
}

fn map_bulk_location_result(result: BulkLocationResult) -> BulkLocationResponse {
    BulkLocationResponse {
        created: result.created.into_iter().map(map_location).collect(),
        errors: result.errors,
    }
}

fn resolve_slug(slug: Option<String>, name: &str) -> String {
    slug.map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| derive_slug(name))
}

/// List locations.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/locations",
    tag = "inventory",
    params(LocationListQuery),
    responses(
        (status = 200, description = "Locations", body = InventoryListEnvelope<LocationListItemResponse>),
        (status = "4XX", description = "Location list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Location list failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_locations(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<LocationListQuery>,
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

    let site_ids = match parse_csv_uuids(query.site_id.as_deref(), "site_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let parent_ids = match parse_csv_uuids(query.parent_id.as_deref(), "parent_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let tenant_ids = match parse_csv_uuids(query.tenant_id.as_deref(), "tenant_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let status = parse_csv_strings(query.status.as_deref());
    let resolved_limit = clamp_limit(query.limit);
    let resolved_offset = clamp_offset(query.offset);
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        LOCATION_LIST_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .locations
        .list(
            LocationFilter {
                site_ids: &site_ids,
                parent_ids: &parent_ids,
                tenant_ids: &tenant_ids,
                status: &status,
                search: crate::contexts::shared::list_query::search_pattern(
                    query.search.as_deref(),
                ),
                sort,
            },
            query.limit,
            query.offset,
        )
        .await
    {
        Ok((records, total)) => {
            let items: Vec<LocationListItemResponse> =
                records.into_iter().map(map_location_list_item).collect();
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

/// Get a location.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/locations/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Location id"),
    ),
    responses(
        (status = 200, description = "Location", body = LocationResponse),
        (status = 404, description = "Location not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Location get request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Location get failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_location(
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
    match state.repositories.locations.get_by_id(id).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_location(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Location not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a location.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/locations",
    tag = "inventory",
    request_body = LocationCreatePayload,
    responses(
        (status = 201, description = "Location created", body = LocationResponse),
        (status = "4XX", description = "Location create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Location create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_location(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<LocationCreatePayload>,
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
    let site_id = match parse_uuid(&payload.site_id, "site_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let parent_id = match parse_optional_uuid(payload.parent_id.clone(), "parent_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let tenant_id = match parse_optional_uuid(payload.tenant_id.clone(), "tenant_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let slug = resolve_slug(payload.slug, &name);

    let new_location = NewLocation {
        id: Uuid::now_v7(),
        name,
        slug,
        site_id,
        parent_id,
        status: payload.status,
        tenant_id,
        facility: payload.facility,
        description: payload.description,
        owner_id: None,
    };

    match state.repositories.locations.create(&new_location).await {
        Ok(record) => (StatusCode::CREATED, Json(map_location(record))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update a location.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/locations/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Location id"),
    ),
    request_body = LocationUpdatePayload,
    responses(
        (status = 200, description = "Location updated", body = LocationResponse),
        (status = 404, description = "Location not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Location update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Location update failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_location(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<LocationUpdatePayload>,
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
    let site_id = match parse_optional_uuid(payload.site_id.clone(), "site_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let parent_id = match parse_optional_uuid(payload.parent_id.clone(), "parent_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let tenant_id = match parse_optional_uuid(payload.tenant_id.clone(), "tenant_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let changes = UpdateLocation {
        name: payload.name,
        slug: payload.slug,
        site_id,
        parent_id,
        status: payload.status,
        tenant_id,
        facility: payload.facility,
        description: payload.description,
    };

    match state.repositories.locations.update(id, &changes).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_location(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Location not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a location.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/locations/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Location id"),
    ),
    responses(
        (status = 204, description = "Location deleted"),
        (status = "4XX", description = "Location delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Location delete failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_location(
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
    match state.repositories.locations.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create locations.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/locations/bulk",
    tag = "inventory",
    request_body = LocationBulkCreatePayload,
    responses(
        (status = 200, description = "Bulk location create result", body = BulkLocationResponse),
        (status = "4XX", description = "Bulk location create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Bulk location create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_locations(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<LocationBulkCreatePayload>,
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
        let site_id = match parse_uuid(&p.site_id, "site_id") {
            Ok(value) => value,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        let parent_id = match parse_optional_uuid(p.parent_id.clone(), "parent_id") {
            Ok(value) => value,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        let tenant_id = match parse_optional_uuid(p.tenant_id.clone(), "tenant_id") {
            Ok(value) => value,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        let slug = resolve_slug(p.slug, &name);
        items.push(NewLocation {
            id: Uuid::now_v7(),
            name,
            slug,
            site_id,
            parent_id,
            status: p.status,
            tenant_id,
            facility: p.facility,
            description: p.description,
            owner_id: None,
        });
    }

    match state.repositories.locations.bulk_create(&items).await {
        Ok(result) => (StatusCode::OK, Json(map_bulk_location_result(result))).into_response(),
        Err(error) => handle_app_error(error),
    }
}
