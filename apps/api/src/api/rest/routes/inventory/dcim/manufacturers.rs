use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use uuid::Uuid;

use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::domain::Manufacturer;
use crate::contexts::inventory::infrastructure::persistence::MANUFACTURER_COLUMNS;
use crate::contexts::shared::list_query::SortSpec;
use crate::errors::AppError;

use super::super::{
    BulkCreateRequest, BulkCreateResponse, InventoryListEnvelope, ManufacturerPayload,
    bump_host_config_cache_generation, ensure_access, ensure_create_inventory_access,
    parse_required_string, parse_uuid, push_bulk_error, require_bulk_items,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};

#[derive(Debug, serde::Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct ManufacturerListQuery {
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub search: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

/// List manufacturers.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/manufacturers",
    tag = "inventory",
    params(ManufacturerListQuery),
    responses(
        (status = 200, description = "Manufacturers", body = InventoryListEnvelope<Manufacturer>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_manufacturers(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ManufacturerListQuery>,
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
    let search = query
        .search
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| format!("%{s}%"));
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        MANUFACTURER_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.inventory_admin.clone();
    match service
        .list_manufacturers(
            crate::contexts::inventory::application::InventoryListQuery::new(
                query.limit,
                query.offset,
                search,
            )
            .with_sort(sort),
        )
        .await
    {
        Ok((items, total)) => (
            StatusCode::OK,
            Json(InventoryListEnvelope {
                items,
                total,
                limit: resolved_limit,
                offset: resolved_offset,
            }),
        )
            .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Get a manufacturer.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/manufacturers/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Manufacturer ID"),
    ),
    responses(
        (status = 200, description = "Manufacturer", body = Manufacturer),
        (status = 404, description = "Manufacturer not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_manufacturer(
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
    let manufacturer_id = match parse_uuid(&id, "manufacturer_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.inventory_admin.clone();
    match service.get_manufacturer(manufacturer_id).await {
        Ok(Some(item)) => (StatusCode::OK, Json(item)).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Manufacturer not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a manufacturer.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/manufacturers",
    tag = "inventory",
    request_body = ManufacturerPayload,
    responses(
        (status = 201, description = "Manufacturer created", body = Manufacturer),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_manufacturer(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<ManufacturerPayload>,
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
    let manufacturer = Manufacturer {
        id: Uuid::now_v7(),
        external_id: None,
        name,
        slug,
        description: payload.description,
        piping_synced_at: None,
        ..Default::default()
    };
    let service = state.services.inventory_admin.clone();
    match service.create_manufacturer(manufacturer.clone()).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            (StatusCode::CREATED, Json(manufacturer)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create manufacturers.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/manufacturers/bulk",
    tag = "inventory",
    request_body = BulkCreateRequest<ManufacturerPayload>,
    responses(
        (status = 200, description = "Bulk manufacturer create result", body = BulkCreateResponse<Manufacturer>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_manufacturers(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(request): EnvelopeJson<BulkCreateRequest<ManufacturerPayload>>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_create_inventory_access(&state, &request_context).await {
        return handle_app_error(error);
    }

    let items = match require_bulk_items(request.items) {
        Ok(items) => items,
        Err(error) => return handle_app_error(error),
    };

    let service = state.services.inventory_admin.clone();
    let mut created = Vec::new();
    let mut errors = Vec::new();

    for (index, payload) in items.into_iter().enumerate() {
        let name = match parse_required_string(&payload.name, "name") {
            Ok(value) => value,
            Err(error) => {
                push_bulk_error(&mut errors, index, error);
                continue;
            }
        };
        let slug = match parse_required_string(&payload.slug, "slug") {
            Ok(value) => value,
            Err(error) => {
                push_bulk_error(&mut errors, index, error);
                continue;
            }
        };
        let manufacturer = Manufacturer {
            id: Uuid::now_v7(),
            external_id: None,
            name,
            slug,
            description: payload.description,
            piping_synced_at: None,
            ..Default::default()
        };

        match service.create_manufacturer(manufacturer.clone()).await {
            Ok(()) => created.push(manufacturer),
            Err(error) => push_bulk_error(&mut errors, index, error),
        }
    }

    if !created.is_empty() {
        bump_host_config_cache_generation(&state).await;
    }

    (StatusCode::OK, Json(BulkCreateResponse { created, errors })).into_response()
}

/// Update a manufacturer.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/manufacturers/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Manufacturer ID"),
    ),
    request_body = ManufacturerPayload,
    responses(
        (status = 200, description = "Manufacturer updated", body = Manufacturer),
        (status = 404, description = "Manufacturer not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_manufacturer(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<ManufacturerPayload>,
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
    let manufacturer_id = match parse_uuid(&id, "manufacturer_id") {
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

    let service = state.services.inventory_admin.clone();
    match service.get_manufacturer(manufacturer_id).await {
        Ok(None) => {
            return handle_app_error(AppError::NotFound("Manufacturer not found".to_string()));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };

    let manufacturer = Manufacturer {
        id: manufacturer_id,
        external_id: None,
        name,
        slug,
        description: payload.description,
        piping_synced_at: None,
        ..Default::default()
    };

    match service.update_manufacturer(manufacturer.clone()).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            (StatusCode::OK, Json(manufacturer)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Delete a manufacturer.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/manufacturers/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Manufacturer ID"),
    ),
    responses(
        (status = 204, description = "Manufacturer deleted"),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_manufacturer(
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
    let manufacturer_id = match parse_uuid(&id, "manufacturer_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.inventory_admin.clone();
    match service.get_manufacturer(manufacturer_id).await {
        Ok(None) => {
            return handle_app_error(AppError::NotFound("Manufacturer not found".to_string()));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };
    match service.delete_manufacturer(manufacturer_id).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => handle_app_error(error),
    }
}
