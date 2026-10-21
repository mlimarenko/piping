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
use crate::contexts::inventory::domain::CircuitTermination;
use crate::contexts::inventory::infrastructure::persistence::{
    CIRCUIT_TERMINATION_COLUMNS, CircuitTerminationFilter, CircuitTerminationRepository,
};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_strings, parse_csv_uuids};
use crate::errors::AppError;

use super::super::{
    BulkCreateRequest, BulkCreateResponse, InventoryListEnvelope,
    bump_host_config_cache_generation, ensure_access, ensure_create_inventory_access,
    parse_required_string, parse_uuid, push_bulk_error, require_bulk_items,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};

#[derive(Debug, serde::Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct CircuitTerminationListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    pub term_side: Option<String>,
    pub circuit_id: Option<String>,
    pub mark_connected: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct CircuitTerminationPayload {
    pub term_side: String,
    pub circuit_id: String,
    pub xconnect_id: Option<String>,
    pub pp_info: Option<String>,
    pub port_speed: Option<i32>,
    pub upstream_speed: Option<i32>,
    pub mark_connected: Option<bool>,
    pub description: Option<String>,
}

struct ParsedCircuitTermination {
    term_side: String,
    circuit_id: Uuid,
}

fn parse_circuit_termination_payload(
    payload: &CircuitTerminationPayload,
) -> Result<ParsedCircuitTermination, AppError> {
    Ok(ParsedCircuitTermination {
        term_side: parse_required_string(&payload.term_side, "term_side")?,
        circuit_id: parse_uuid(&payload.circuit_id, "circuit_id")?,
    })
}

fn build_circuit_termination(
    id: Uuid,
    parsed: ParsedCircuitTermination,
    payload: CircuitTerminationPayload,
) -> CircuitTermination {
    CircuitTermination {
        id,
        term_side: parsed.term_side,
        circuit_id: parsed.circuit_id,
        xconnect_id: payload.xconnect_id.unwrap_or_default(),
        pp_info: payload.pp_info.unwrap_or_default(),
        port_speed: payload.port_speed,
        upstream_speed: payload.upstream_speed,
        mark_connected: payload.mark_connected.unwrap_or_default(),
        description: payload.description.unwrap_or_default(),
        ..Default::default()
    }
}

/// List circuit terminations.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/circuit-terminations",
    operation_id = "list_inventory_circuit_terminations",
    tag = "inventory",
    params(CircuitTerminationListQuery),
    responses(
        (status = 200, description = "Circuit terminations", body = InventoryListEnvelope<CircuitTermination>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_circuit_terminations(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<CircuitTerminationListQuery>,
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
        CIRCUIT_TERMINATION_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let filter = CircuitTerminationFilter {
        search: crate::contexts::shared::list_query::search_pattern(query.search.as_deref()),
        term_side: parse_csv_strings(query.term_side.as_deref()),
        circuit_id: match parse_csv_uuids(query.circuit_id.as_deref(), "circuit_id") {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        },
        mark_connected: parse_csv_strings(query.mark_connected.as_deref()),
    };
    match state
        .repositories
        .circuit_terminations
        .list_paginated(filter, query.limit, query.offset, sort)
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

/// Get a circuit termination.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/circuit-terminations/{id}",
    operation_id = "get_inventory_circuit_termination",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Circuit termination ID"),
    ),
    responses(
        (status = 200, description = "Circuit termination", body = CircuitTermination),
        (status = 404, description = "Circuit termination not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_circuit_termination(
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
    let entity_id = match parse_uuid(&id, "circuit_termination_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .circuit_terminations
        .get_by_id(entity_id)
        .await
    {
        Ok(Some(item)) => (StatusCode::OK, Json(item)).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound(
            "Circuit termination not found".to_string(),
        )),
        Err(error) => handle_app_error(error),
    }
}

/// Create a circuit termination.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/circuit-terminations",
    operation_id = "create_inventory_circuit_termination",
    tag = "inventory",
    request_body = CircuitTerminationPayload,
    responses(
        (status = 201, description = "Circuit termination created", body = CircuitTermination),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_circuit_termination(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<CircuitTerminationPayload>,
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
    let parsed = match parse_circuit_termination_payload(&payload) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let item = build_circuit_termination(Uuid::now_v7(), parsed, payload);
    match state.repositories.circuit_terminations.create(&item).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            (StatusCode::CREATED, Json(item)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create circuit terminations.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/circuit-terminations/bulk",
    operation_id = "bulk_create_inventory_circuit_terminations",
    tag = "inventory",
    request_body = BulkCreateRequest<CircuitTerminationPayload>,
    responses(
        (status = 200, description = "Bulk circuit termination create result", body = BulkCreateResponse<CircuitTermination>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_circuit_terminations(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(request): EnvelopeJson<BulkCreateRequest<CircuitTerminationPayload>>,
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
        let parsed = match parse_circuit_termination_payload(&payload) {
            Ok(value) => value,
            Err(error) => {
                push_bulk_error(&mut errors, index, error);
                continue;
            }
        };
        let item = build_circuit_termination(Uuid::now_v7(), parsed, payload);

        match state.repositories.circuit_terminations.create(&item).await {
            Ok(()) => created.push(item),
            Err(error) => push_bulk_error(&mut errors, index, error),
        }
    }

    if !created.is_empty() {
        bump_host_config_cache_generation(&state).await;
    }

    (StatusCode::OK, Json(BulkCreateResponse { created, errors })).into_response()
}

/// Update a circuit termination.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/circuit-terminations/{id}",
    operation_id = "update_inventory_circuit_termination",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Circuit termination ID"),
    ),
    request_body = CircuitTerminationPayload,
    responses(
        (status = 200, description = "Circuit termination updated", body = CircuitTermination),
        (status = 404, description = "Circuit termination not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_circuit_termination(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<CircuitTerminationPayload>,
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
    let entity_id = match parse_uuid(&id, "circuit_termination_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let parsed = match parse_circuit_termination_payload(&payload) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match state
        .repositories
        .circuit_terminations
        .get_by_id(entity_id)
        .await
    {
        Ok(None) => {
            return handle_app_error(AppError::NotFound(
                "Circuit termination not found".to_string(),
            ));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };

    let item = build_circuit_termination(entity_id, parsed, payload);
    match state.repositories.circuit_terminations.update(&item).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            (StatusCode::OK, Json(item)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Delete a circuit termination.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/circuit-terminations/{id}",
    operation_id = "delete_inventory_circuit_termination",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Circuit termination ID"),
    ),
    responses(
        (status = 204, description = "Circuit termination deleted"),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_circuit_termination(
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
    let entity_id = match parse_uuid(&id, "circuit_termination_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .circuit_terminations
        .get_by_id(entity_id)
        .await
    {
        Ok(None) => {
            return handle_app_error(AppError::NotFound(
                "Circuit termination not found".to_string(),
            ));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .circuit_terminations
        .delete(entity_id)
        .await
    {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => handle_app_error(error),
    }
}
