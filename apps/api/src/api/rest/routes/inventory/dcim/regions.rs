use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::api::rest::routes::custom_fields::CONTENT_TYPE_REGION;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::domain::Region;
use crate::contexts::inventory::infrastructure::persistence::{
    REGION_COLUMNS, SortSpec, parse_csv_uuids,
};
use crate::errors::AppError;

use super::super::{
    BulkCreateRequest, BulkCreateResponse, BulkCreatedBatch, InventoryListEnvelope,
    RegionListRowResponse, RegionPayload, apply_custom_field_secret_intents,
    apply_custom_fields_write, bump_host_config_cache_generation, ensure_access,
    ensure_create_inventory_access, finalize_bulk_created, masked_custom_fields,
    parse_optional_uuid, parse_required_string, parse_uuid, push_bulk_error, require_bulk_items,
    validate_and_split_custom_fields,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};

/// Query parameters for region listing — pagination, free-text search, and
/// allowlisted sorting.
///
/// `sort` is a stable logical column key (`name`, `parent`); `order` is `asc`
/// (default) or `desc`. An unknown sort key or invalid order is rejected with
/// 400.
#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct RegionListQuery {
    /// Comma-separated parent region ids; matches any (`IN`).
    pub parent_id: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    /// Case-insensitive substring search on region name.
    pub search: Option<String>,
    /// Logical sort column key (allowlisted; unknown keys are rejected).
    pub sort: Option<String>,
    /// Sort direction: `asc` (default) or `desc`.
    pub order: Option<String>,
}

/// List regions.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/regions",
    tag = "inventory",
    params(RegionListQuery),
    responses(
        (status = 200, description = "Regions listed", body = InventoryListEnvelope<RegionListRowResponse>),
        (status = "4XX", description = "Region list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Region list request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_regions(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<RegionListQuery>,
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
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        REGION_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let parent_id = match parse_csv_uuids(query.parent_id.as_deref(), "parent_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let resolved_limit = clamp_limit(query.limit);
    let resolved_offset = clamp_offset(query.offset);
    let search = query
        .search
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| format!("%{}%", s));
    let service = state.services.inventory_admin.clone();
    match service
        .list_regions_hydrated(
            crate::contexts::inventory::application::InventoryListQuery::new(
                query.limit,
                query.offset,
                search,
            )
            .with_parent_id(parent_id)
            .with_sort(sort),
        )
        .await
    {
        Ok((rows, total)) => {
            let items = rows
                .into_iter()
                .map(|r| RegionListRowResponse {
                    id: r.id,
                    name: r.name,
                    slug: r.slug,
                    parent_id: r.parent_id,
                    parent_name: r.parent_name,
                })
                .collect();
            let envelope = InventoryListEnvelope {
                items,
                total,
                limit: resolved_limit,
                offset: resolved_offset,
            };
            (StatusCode::OK, Json(envelope)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Get a region.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/regions/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Region id"),
    ),
    responses(
        (status = 200, description = "Region", body = Region),
        (status = 404, description = "Region not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Region request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Region request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_region(
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
    let region_id = match parse_uuid(&id, "region_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.inventory_admin.clone();
    match service.get_region(region_id).await {
        Ok(Some(mut region)) => {
            let raw_custom_fields = region
                .custom_fields
                .clone()
                .unwrap_or(serde_json::Value::Null);
            match masked_custom_fields(&state, CONTENT_TYPE_REGION, region.id, &raw_custom_fields)
                .await
            {
                Ok(value) => region.custom_fields = Some(value),
                Err(error) => return handle_app_error(error),
            }
            (StatusCode::OK, Json(region)).into_response()
        }
        Ok(None) => handle_app_error(AppError::NotFound("Region not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a region.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/regions",
    tag = "inventory",
    request_body = RegionPayload,
    responses(
        (status = 201, description = "Region created", body = Region),
        (status = "4XX", description = "Region create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Region create request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_region(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<RegionPayload>,
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
    let slug = match parse_required_string(&payload.slug, "slug") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let parent_id = match parse_optional_uuid(payload.parent_id.clone(), "parent_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let region_id = Uuid::now_v7();
    let custom_field_outcome = match validate_and_split_custom_fields(
        &state,
        CONTENT_TYPE_REGION,
        payload.custom_fields,
        true,
    )
    .await
    {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let custom_field_data = custom_field_outcome
        .as_ref()
        .map(|outcome| outcome.custom_field_data.clone());
    let mut region = Region {
        id: region_id,
        external_id: None,
        name,
        slug,
        parent_id,
        description: payload.description,
        custom_fields: custom_field_data,
        piping_synced_at: None,
        ..Default::default()
    };
    let service = state.services.inventory_admin.clone();
    match service.create_region(region.clone()).await {
        Ok(()) => {
            if let Some(outcome) = custom_field_outcome
                && let Err(error) =
                    apply_custom_field_secret_intents(&state, region.id, outcome.secret_intents)
                        .await
            {
                return handle_app_error(error);
            }
            bump_host_config_cache_generation(&state).await;
            let raw_custom_fields = region
                .custom_fields
                .clone()
                .unwrap_or(serde_json::Value::Null);
            match masked_custom_fields(&state, CONTENT_TYPE_REGION, region.id, &raw_custom_fields)
                .await
            {
                Ok(value) => region.custom_fields = Some(value),
                Err(error) => return handle_app_error(error),
            }
            (StatusCode::CREATED, Json(region)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Validate, persist, and secret-bind one bulk region payload. Any failure
/// (parse, custom-field validation, create, or secret intent) fails just this
/// item.
async fn create_region_item(state: &AppState, payload: RegionPayload) -> Result<Region, AppError> {
    let name = parse_required_string(&payload.name, "name")?;
    let slug = parse_required_string(&payload.slug, "slug")?;
    let parent_id = parse_optional_uuid(payload.parent_id.clone(), "parent_id")?;
    let custom_field_outcome =
        validate_and_split_custom_fields(state, CONTENT_TYPE_REGION, payload.custom_fields, true)
            .await?;
    let custom_field_data = custom_field_outcome
        .as_ref()
        .map(|outcome| outcome.custom_field_data.clone());
    let region = Region {
        id: Uuid::now_v7(),
        external_id: None,
        name,
        slug,
        parent_id,
        description: payload.description,
        custom_fields: custom_field_data,
        piping_synced_at: None,
        ..Default::default()
    };
    state
        .services
        .inventory_admin
        .create_region(region.clone())
        .await?;
    if let Some(outcome) = custom_field_outcome
        && let Err(error) =
            apply_custom_field_secret_intents(state, region.id, outcome.secret_intents).await
    {
        return Err(error);
    }
    Ok(region)
}

/// Bulk-create regions.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/regions/bulk",
    tag = "inventory",
    request_body = BulkCreateRequest<RegionPayload>,
    responses(
        (status = 200, description = "Regions bulk-created", body = BulkCreateResponse<Region>),
        (status = "4XX", description = "Regions bulk-create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Regions bulk-create request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_regions(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(request): EnvelopeJson<BulkCreateRequest<RegionPayload>>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_create_inventory_access(&state, &request_context).await {
        return handle_app_error(error);
    }

    let items = match require_bulk_items(request.items) {
        Ok(items) => items,
        Err(error) => return handle_app_error(error),
    };

    let mut created = Vec::new();
    let mut created_indices: Vec<usize> = Vec::new();
    let mut errors = Vec::new();

    for (index, payload) in items.into_iter().enumerate() {
        match create_region_item(&state, payload).await {
            Ok(region) => {
                created.push(region);
                created_indices.push(index);
            }
            Err(error) => push_bulk_error(&mut errors, index, error),
        }
    }

    finalize_bulk_created(
        &state,
        CONTENT_TYPE_REGION,
        BulkCreatedBatch {
            created: &mut created,
            created_indices: &created_indices,
            errors: &mut errors,
        },
        |region: &Region| region.id,
        |region: &Region| region.custom_fields.as_ref(),
        |region: &mut Region, value| region.custom_fields = Some(value),
    )
    .await;

    (StatusCode::OK, Json(BulkCreateResponse { created, errors })).into_response()
}

/// Update a region.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/regions/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Region id"),
    ),
    request_body = RegionPayload,
    responses(
        (status = 200, description = "Region updated", body = Region),
        (status = 404, description = "Region not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Region update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Region update request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_region(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<RegionPayload>,
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
    let region_id = match parse_uuid(&id, "region_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let name = match parse_required_string(&payload.name, "name") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let slug = match parse_required_string(&payload.slug, "slug") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let parent_id = match parse_optional_uuid(payload.parent_id.clone(), "parent_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let service = state.services.inventory_admin.clone();
    let existing = match service.get_region(region_id).await {
        Ok(None) => return handle_app_error(AppError::NotFound("Region not found".to_string())),
        Ok(Some(existing)) => existing,
        Err(error) => return handle_app_error(error),
    };
    let custom_fields = match payload.custom_fields {
        Some(incoming) => {
            match apply_custom_fields_write(&state, CONTENT_TYPE_REGION, region_id, Some(incoming))
                .await
            {
                Ok(value) => value,
                Err(error) => return handle_app_error(error),
            }
        }
        None => existing.custom_fields,
    };

    let mut region = Region {
        id: region_id,
        external_id: existing.external_id,
        name,
        slug,
        parent_id,
        description: payload.description,
        custom_fields,
        piping_synced_at: existing.piping_synced_at,
        ..Default::default()
    };

    match service.update_region(region.clone()).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            let raw_custom_fields = region
                .custom_fields
                .clone()
                .unwrap_or(serde_json::Value::Null);
            match masked_custom_fields(&state, CONTENT_TYPE_REGION, region.id, &raw_custom_fields)
                .await
            {
                Ok(value) => region.custom_fields = Some(value),
                Err(error) => return handle_app_error(error),
            }
            (StatusCode::OK, Json(region)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Delete a region.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/regions/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Region id"),
    ),
    responses(
        (status = 204, description = "Region deleted"),
        (status = 404, description = "Region not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Region delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Region delete request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_region(
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
    let region_id = match parse_uuid(&id, "region_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.inventory_admin.clone();
    match service.get_region(region_id).await {
        Ok(None) => return handle_app_error(AppError::NotFound("Region not found".to_string())),
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };
    match service.delete_region(region_id).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => handle_app_error(error),
    }
}
