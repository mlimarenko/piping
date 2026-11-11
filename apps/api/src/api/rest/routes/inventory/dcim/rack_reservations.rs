use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use uuid::Uuid;

use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::domain::RackReservation;
use crate::contexts::inventory::infrastructure::persistence::{
    RACK_RESERVATION_COLUMNS, RackReservationRepository, parse_csv_strings, parse_csv_uuids,
};
use crate::contexts::inventory::infrastructure::persistence::repositories::dcim::rack_reservation_repository::RackReservationFilter;
use crate::contexts::shared::list_query::SortSpec;
use crate::errors::AppError;

use super::super::{
    BulkCreateRequest, BulkCreateResponse, InventoryListEnvelope,
    bump_host_config_cache_generation, ensure_access, ensure_create_inventory_access,
    parse_optional_uuid, parse_required_string, parse_uuid, push_bulk_error, require_bulk_items,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};

#[derive(Debug, serde::Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct RackReservationListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    /// Comma-separated rack ids; matches any (`IN`).
    pub rack_id: Option<String>,
    /// Comma-separated user ids; matches any (`IN`).
    pub user_id: Option<String>,
    /// Comma-separated reservation statuses; matches any (`IN`).
    pub status: Option<String>,
    /// Comma-separated tenant ids; matches any (`IN`).
    pub tenant_id: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct RackReservationPayload {
    pub units: Vec<i16>,
    pub rack_id: String,
    pub user_id: String,
    pub status: String,
    pub tenant_id: Option<String>,
    pub description: Option<String>,
    pub comments: Option<String>,
}

struct ParsedRackReservation {
    rack_id: Uuid,
    user_id: Uuid,
    status: String,
    tenant_id: Option<Uuid>,
}

fn parse_rack_reservation_payload(
    payload: &RackReservationPayload,
) -> Result<ParsedRackReservation, AppError> {
    Ok(ParsedRackReservation {
        rack_id: parse_uuid(&payload.rack_id, "rack_id")?,
        user_id: parse_uuid(&payload.user_id, "user_id")?,
        status: parse_required_string(&payload.status, "status")?,
        tenant_id: parse_optional_uuid(payload.tenant_id.clone(), "tenant_id")?,
    })
}

fn build_rack_reservation(
    id: Uuid,
    parsed: ParsedRackReservation,
    payload: RackReservationPayload,
) -> RackReservation {
    RackReservation {
        id,
        units: payload.units,
        rack_id: parsed.rack_id,
        user_id: parsed.user_id,
        status: parsed.status,
        tenant_id: parsed.tenant_id,
        description: payload.description.unwrap_or_default(),
        comments: payload.comments.unwrap_or_default(),
        ..Default::default()
    }
}

/// List rack reservations.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/rack-reservations",
    operation_id = "list_inventory_rack_reservations",
    tag = "inventory",
    params(RackReservationListQuery),
    responses(
        (status = 200, description = "Rack reservations", body = InventoryListEnvelope<RackReservation>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_rack_reservations(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<RackReservationListQuery>,
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
    let rack_id = match parse_csv_uuids(query.rack_id.as_deref(), "rack_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let user_id = match parse_csv_uuids(query.user_id.as_deref(), "user_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let tenant_id = match parse_csv_uuids(query.tenant_id.as_deref(), "tenant_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let status = parse_csv_strings(query.status.as_deref());
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        RACK_RESERVATION_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let filter = RackReservationFilter {
        search: crate::contexts::shared::list_query::search_pattern(query.search.as_deref()),
        rack_id,
        user_id,
        status,
        tenant_id,
        sort,
    };
    match state
        .repositories
        .rack_reservations
        .list_paginated(filter, query.limit, query.offset)
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

/// Get a rack reservation.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/rack-reservations/{id}",
    operation_id = "get_inventory_rack_reservation",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Rack reservation ID"),
    ),
    responses(
        (status = 200, description = "Rack reservation", body = RackReservation),
        (status = 404, description = "Rack reservation not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_rack_reservation(
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
    let entity_id = match parse_uuid(&id, "rack_reservation_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .rack_reservations
        .get_by_id(entity_id)
        .await
    {
        Ok(Some(item)) => (StatusCode::OK, Json(item)).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Rack reservation not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a rack reservation.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/rack-reservations",
    operation_id = "create_inventory_rack_reservation",
    tag = "inventory",
    request_body = RackReservationPayload,
    responses(
        (status = 201, description = "Rack reservation created", body = RackReservation),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_rack_reservation(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<RackReservationPayload>,
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
    let parsed = match parse_rack_reservation_payload(&payload) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let item = build_rack_reservation(Uuid::now_v7(), parsed, payload);
    match state.repositories.rack_reservations.create(&item).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            (StatusCode::CREATED, Json(item)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create rack reservations.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/rack-reservations/bulk",
    operation_id = "bulk_create_inventory_rack_reservations",
    tag = "inventory",
    request_body = BulkCreateRequest<RackReservationPayload>,
    responses(
        (status = 200, description = "Bulk rack reservation create result", body = BulkCreateResponse<RackReservation>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_rack_reservations(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(request): EnvelopeJson<BulkCreateRequest<RackReservationPayload>>,
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
    let mut errors = Vec::new();

    for (index, payload) in items.into_iter().enumerate() {
        let parsed = match parse_rack_reservation_payload(&payload) {
            Ok(value) => value,
            Err(error) => {
                push_bulk_error(&mut errors, index, error);
                continue;
            }
        };
        let item = build_rack_reservation(Uuid::now_v7(), parsed, payload);

        match state.repositories.rack_reservations.create(&item).await {
            Ok(()) => created.push(item),
            Err(error) => push_bulk_error(&mut errors, index, error),
        }
    }

    if !created.is_empty() {
        bump_host_config_cache_generation(&state).await;
    }

    (StatusCode::OK, Json(BulkCreateResponse { created, errors })).into_response()
}

/// Update a rack reservation.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/rack-reservations/{id}",
    operation_id = "update_inventory_rack_reservation",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Rack reservation ID"),
    ),
    request_body = RackReservationPayload,
    responses(
        (status = 200, description = "Rack reservation updated", body = RackReservation),
        (status = 404, description = "Rack reservation not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_rack_reservation(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<RackReservationPayload>,
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
    let entity_id = match parse_uuid(&id, "rack_reservation_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let parsed = match parse_rack_reservation_payload(&payload) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match state
        .repositories
        .rack_reservations
        .get_by_id(entity_id)
        .await
    {
        Ok(None) => {
            return handle_app_error(AppError::NotFound("Rack reservation not found".to_string()));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };

    let item = build_rack_reservation(entity_id, parsed, payload);
    match state.repositories.rack_reservations.update(&item).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            (StatusCode::OK, Json(item)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Delete a rack reservation.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/rack-reservations/{id}",
    operation_id = "delete_inventory_rack_reservation",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Rack reservation ID"),
    ),
    responses(
        (status = 204, description = "Rack reservation deleted"),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_rack_reservation(
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
    let entity_id = match parse_uuid(&id, "rack_reservation_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .rack_reservations
        .get_by_id(entity_id)
        .await
    {
        Ok(None) => {
            return handle_app_error(AppError::NotFound("Rack reservation not found".to_string()));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };
    match state.repositories.rack_reservations.delete(entity_id).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => handle_app_error(error),
    }
}
