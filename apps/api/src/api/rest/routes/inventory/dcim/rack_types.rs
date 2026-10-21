//! Rack types REST routes — full CRUD + bulk.
//!
//! Handlers never run SQL directly; all persistence goes through
//! `RackTypeRepository` on `AppState`.

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
use crate::contexts::inventory::infrastructure::persistence::repositories::dcim::rack_type_repository::RACK_TYPE_LIST_COLUMNS;
use crate::contexts::inventory::infrastructure::persistence::{
    BulkRackTypeResult, NewRackType, RackTypeFilter, RackTypeListItem, RackTypeRecord,
    RackTypeRepository, UpdateRackType,
};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_strings, parse_csv_uuids};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

use super::super::{
    InventoryListEnvelope, ensure_access, parse_optional_uuid, parse_required_string, parse_uuid,
};

/// Query parameters for rack type listing.
#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct RackTypeListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    pub manufacturer_id: Option<String>,
    pub form_factor: Option<String>,
    pub desc_units: Option<String>,
    pub outer_unit: Option<String>,
    pub weight_unit: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct RackTypeCreatePayload {
    pub manufacturer_id: String,
    pub model: String,
    pub slug: Option<String>,
    pub form_factor: Option<String>,
    pub width: Option<i16>,
    pub u_height: Option<i16>,
    pub starting_unit: Option<i16>,
    pub desc_units: Option<bool>,
    pub outer_width: Option<i16>,
    pub outer_depth: Option<i16>,
    pub outer_height: Option<i16>,
    pub outer_unit: Option<String>,
    pub mounting_depth: Option<i16>,
    pub weight: Option<f64>,
    pub max_weight: Option<i32>,
    pub weight_unit: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct RackTypeUpdatePayload {
    pub manufacturer_id: Option<String>,
    pub model: Option<String>,
    pub slug: Option<String>,
    pub form_factor: Option<String>,
    pub width: Option<i16>,
    pub u_height: Option<i16>,
    pub starting_unit: Option<i16>,
    pub desc_units: Option<bool>,
    pub outer_width: Option<i16>,
    pub outer_depth: Option<i16>,
    pub outer_height: Option<i16>,
    pub outer_unit: Option<String>,
    pub mounting_depth: Option<i16>,
    pub weight: Option<f64>,
    pub max_weight: Option<i32>,
    pub weight_unit: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct RackTypeBulkCreatePayload {
    #[serde(deserialize_with = "crate::api::rest::routes::validation::deserialize_bulk_items")]
    #[schema(min_items = 1)]
    pub items: Vec<RackTypeCreatePayload>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RackTypeResponse {
    pub id: Uuid,
    pub external_id: Option<i64>,
    pub manufacturer_id: Uuid,
    pub model: String,
    pub slug: String,
    pub form_factor: String,
    pub width: Option<i16>,
    pub u_height: Option<i16>,
    pub starting_unit: Option<i16>,
    pub desc_units: bool,
    pub outer_width: Option<i16>,
    pub outer_depth: Option<i16>,
    pub outer_height: Option<i16>,
    pub outer_unit: Option<String>,
    pub mounting_depth: Option<i16>,
    pub weight: Option<f64>,
    pub max_weight: Option<i32>,
    pub weight_unit: Option<String>,
    pub description: Option<String>,
    pub owner_id: Option<Uuid>,
    pub tags: Vec<String>,
}

/// Hydrated rack type list item — extends `RackTypeResponse` with the
/// manufacturer name for the collection list endpoint.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RackTypeListItemResponse {
    pub id: Uuid,
    pub external_id: Option<i64>,
    pub manufacturer_id: Uuid,
    pub manufacturer_name: Option<String>,
    pub model: String,
    pub slug: String,
    pub form_factor: String,
    pub width: Option<i16>,
    pub u_height: Option<i16>,
    pub starting_unit: Option<i16>,
    pub desc_units: bool,
    pub outer_width: Option<i16>,
    pub outer_depth: Option<i16>,
    pub outer_height: Option<i16>,
    pub outer_unit: Option<String>,
    pub mounting_depth: Option<i16>,
    pub weight: Option<f64>,
    pub max_weight: Option<i32>,
    pub weight_unit: Option<String>,
    pub description: Option<String>,
    pub owner_id: Option<Uuid>,
    pub tags: Vec<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkRackTypeResponse {
    pub created: Vec<RackTypeResponse>,
    pub errors: Vec<String>,
}

fn map_rack_type_list_item(item: RackTypeListItem) -> RackTypeListItemResponse {
    let RackTypeListItem {
        record,
        manufacturer_name,
    } = item;
    RackTypeListItemResponse {
        id: record.id,
        external_id: record.external_id,
        manufacturer_id: record.manufacturer_id,
        manufacturer_name,
        model: record.model,
        slug: record.slug,
        form_factor: record.form_factor,
        width: record.width,
        u_height: record.u_height,
        starting_unit: record.starting_unit,
        desc_units: record.desc_units,
        outer_width: record.outer_width,
        outer_depth: record.outer_depth,
        outer_height: record.outer_height,
        outer_unit: record.outer_unit,
        mounting_depth: record.mounting_depth,
        weight: record.weight,
        max_weight: record.max_weight,
        weight_unit: record.weight_unit,
        description: record.description,
        owner_id: record.owner_id,
        tags: record.tags,
    }
}

fn map_rack_type(record: RackTypeRecord) -> RackTypeResponse {
    RackTypeResponse {
        id: record.id,
        external_id: record.external_id,
        manufacturer_id: record.manufacturer_id,
        model: record.model,
        slug: record.slug,
        form_factor: record.form_factor,
        width: record.width,
        u_height: record.u_height,
        starting_unit: record.starting_unit,
        desc_units: record.desc_units,
        outer_width: record.outer_width,
        outer_depth: record.outer_depth,
        outer_height: record.outer_height,
        outer_unit: record.outer_unit,
        mounting_depth: record.mounting_depth,
        weight: record.weight,
        max_weight: record.max_weight,
        weight_unit: record.weight_unit,
        description: record.description,
        owner_id: record.owner_id,
        tags: record.tags,
    }
}

fn map_bulk_rack_type_result(result: BulkRackTypeResult) -> BulkRackTypeResponse {
    BulkRackTypeResponse {
        created: result.created.into_iter().map(map_rack_type).collect(),
        errors: result.errors,
    }
}

fn resolve_slug(slug: Option<String>, model: &str) -> String {
    slug.map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| derive_slug(model))
}

/// List rack types.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/rack_types",
    tag = "inventory",
    params(RackTypeListQuery),
    responses(
        (status = 200, description = "Rack types", body = InventoryListEnvelope<RackTypeListItemResponse>),
        (status = "4XX", description = "Rack type list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Rack type list failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_rack_types(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<RackTypeListQuery>,
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
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let resolved_limit = clamp_limit(query.limit);
    let resolved_offset = clamp_offset(query.offset);
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        RACK_TYPE_LIST_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .rack_types
        .list(
            RackTypeFilter {
                search: crate::contexts::shared::list_query::search_pattern(
                    query.search.as_deref(),
                ),
                manufacturer_id,
                form_factor: parse_csv_strings(query.form_factor.as_deref()),
                desc_units: parse_csv_strings(query.desc_units.as_deref()),
                outer_unit: parse_csv_strings(query.outer_unit.as_deref()),
                weight_unit: parse_csv_strings(query.weight_unit.as_deref()),
                sort,
            },
            query.limit,
            query.offset,
        )
        .await
    {
        Ok((records, total)) => {
            let items: Vec<RackTypeListItemResponse> =
                records.into_iter().map(map_rack_type_list_item).collect();
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

/// Get a rack type.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/rack_types/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Rack type id"),
    ),
    responses(
        (status = 200, description = "Rack type", body = RackTypeResponse),
        (status = 404, description = "Rack type not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Rack type get request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Rack type get failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_rack_type(
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
    match state.repositories.rack_types.get_by_id(id).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_rack_type(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Rack type not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a rack type.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/rack_types",
    tag = "inventory",
    request_body = RackTypeCreatePayload,
    responses(
        (status = 201, description = "Rack type created", body = RackTypeResponse),
        (status = "4XX", description = "Rack type create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Rack type create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_rack_type(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<RackTypeCreatePayload>,
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

    let manufacturer_id = match parse_uuid(&payload.manufacturer_id, "manufacturer_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let model = match parse_required_string(&payload.model, "model") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let slug = resolve_slug(payload.slug, &model);

    let new_type = NewRackType {
        id: Uuid::now_v7(),
        manufacturer_id,
        model,
        slug,
        form_factor: payload.form_factor.unwrap_or_default(),
        width: payload.width.unwrap_or(0),
        u_height: payload.u_height.unwrap_or(0),
        starting_unit: payload.starting_unit.unwrap_or(1),
        desc_units: payload.desc_units.unwrap_or(false),
        outer_width: payload.outer_width,
        outer_depth: payload.outer_depth,
        outer_height: payload.outer_height,
        outer_unit: payload.outer_unit,
        mounting_depth: payload.mounting_depth,
        weight: payload.weight,
        max_weight: payload.max_weight,
        weight_unit: payload.weight_unit,
        description: payload.description,
        owner_id: None,
    };

    match state.repositories.rack_types.create(&new_type).await {
        Ok(record) => (StatusCode::CREATED, Json(map_rack_type(record))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update a rack type.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/rack_types/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Rack type id"),
    ),
    request_body = RackTypeUpdatePayload,
    responses(
        (status = 200, description = "Rack type updated", body = RackTypeResponse),
        (status = 404, description = "Rack type not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Rack type update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Rack type update failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_rack_type(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<RackTypeUpdatePayload>,
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
    let manufacturer_id =
        match parse_optional_uuid(payload.manufacturer_id.clone(), "manufacturer_id") {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };

    let changes = UpdateRackType {
        manufacturer_id,
        model: payload.model,
        slug: payload.slug,
        form_factor: payload.form_factor,
        width: payload.width,
        u_height: payload.u_height,
        starting_unit: payload.starting_unit,
        desc_units: payload.desc_units,
        outer_width: payload.outer_width,
        outer_depth: payload.outer_depth,
        outer_height: payload.outer_height,
        outer_unit: payload.outer_unit,
        mounting_depth: payload.mounting_depth,
        weight: payload.weight,
        max_weight: payload.max_weight,
        weight_unit: payload.weight_unit,
        description: payload.description,
    };

    match state.repositories.rack_types.update(id, &changes).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_rack_type(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Rack type not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a rack type.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/rack_types/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Rack type id"),
    ),
    responses(
        (status = 204, description = "Rack type deleted"),
        (status = "4XX", description = "Rack type delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Rack type delete failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_rack_type(
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
    match state.repositories.rack_types.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create rack types.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/rack_types/bulk",
    tag = "inventory",
    request_body = RackTypeBulkCreatePayload,
    responses(
        (status = 200, description = "Bulk rack type create result", body = BulkRackTypeResponse),
        (status = "4XX", description = "Bulk rack type create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Bulk rack type create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_rack_types(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<RackTypeBulkCreatePayload>,
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
        let manufacturer_id = match parse_uuid(&p.manufacturer_id, "manufacturer_id") {
            Ok(value) => value,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        let model = match parse_required_string(&p.model, "model") {
            Ok(value) => value,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        let slug = resolve_slug(p.slug, &model);
        items.push(NewRackType {
            id: Uuid::now_v7(),
            manufacturer_id,
            model,
            slug,
            form_factor: p.form_factor.unwrap_or_default(),
            width: p.width.unwrap_or(0),
            u_height: p.u_height.unwrap_or(0),
            starting_unit: p.starting_unit.unwrap_or(1),
            desc_units: p.desc_units.unwrap_or(false),
            outer_width: p.outer_width,
            outer_depth: p.outer_depth,
            outer_height: p.outer_height,
            outer_unit: p.outer_unit,
            mounting_depth: p.mounting_depth,
            weight: p.weight,
            max_weight: p.max_weight,
            weight_unit: p.weight_unit,
            description: p.description,
            owner_id: None,
        });
    }

    match state.repositories.rack_types.bulk_create(&items).await {
        Ok(result) => (StatusCode::OK, Json(map_bulk_rack_type_result(result))).into_response(),
        Err(error) => handle_app_error(error),
    }
}
