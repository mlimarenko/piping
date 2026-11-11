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
use crate::contexts::inventory::domain::PowerPanel;
use crate::contexts::inventory::infrastructure::persistence::repositories::dcim::power_panel_repository::PowerPanelFilter;
use crate::contexts::inventory::infrastructure::persistence::{
    POWER_PANEL_COLUMNS, PowerPanelRepository,
};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_uuids};
use crate::errors::AppError;

use super::super::{
    BulkCreateRequest, BulkCreateResponse, InventoryListEnvelope,
    bump_host_config_cache_generation, ensure_access, ensure_create_inventory_access,
    parse_optional_uuid, parse_required_string, parse_uuid, push_bulk_error, require_bulk_items,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};

#[derive(Debug, serde::Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct PowerPanelListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub site_id: Option<String>,
    pub location_id: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct PowerPanelPayload {
    pub name: String,
    pub site_id: String,
    pub location_id: Option<String>,
    pub description: Option<String>,
    pub comments: Option<String>,
}

struct ParsedPowerPanel {
    name: String,
    site_id: Uuid,
    location_id: Option<Uuid>,
}

fn parse_power_panel_payload(payload: &PowerPanelPayload) -> Result<ParsedPowerPanel, AppError> {
    Ok(ParsedPowerPanel {
        name: parse_required_string(&payload.name, "name")?,
        site_id: parse_uuid(&payload.site_id, "site_id")?,
        location_id: parse_optional_uuid(payload.location_id.clone(), "location_id")?,
    })
}

fn build_power_panel(id: Uuid, parsed: ParsedPowerPanel, payload: PowerPanelPayload) -> PowerPanel {
    PowerPanel {
        id,
        name: parsed.name,
        site_id: parsed.site_id,
        location_id: parsed.location_id,
        description: payload.description.unwrap_or_default(),
        comments: payload.comments.unwrap_or_default(),
        ..Default::default()
    }
}

/// List power panels.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/power-panels",
    operation_id = "list_inventory_power_panels",
    tag = "inventory",
    params(PowerPanelListQuery),
    responses(
        (status = 200, description = "Power panels", body = InventoryListEnvelope<PowerPanel>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_power_panels(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<PowerPanelListQuery>,
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
        POWER_PANEL_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let site_id = match parse_csv_uuids(query.site_id.as_deref(), "site_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let location_id = match parse_csv_uuids(query.location_id.as_deref(), "location_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .power_panels
        .list_paginated(
            query.limit,
            query.offset,
            PowerPanelFilter {
                search: crate::contexts::shared::list_query::search_pattern(
                    query.search.as_deref(),
                ),
                site_id,
                location_id,
                sort,
            },
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

/// Get a power panel.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/power-panels/{id}",
    operation_id = "get_inventory_power_panel",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Power panel ID"),
    ),
    responses(
        (status = 200, description = "Power panel", body = PowerPanel),
        (status = 404, description = "Power panel not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_power_panel(
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
    let entity_id = match parse_uuid(&id, "power_panel_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state.repositories.power_panels.get_by_id(entity_id).await {
        Ok(Some(item)) => (StatusCode::OK, Json(item)).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Power panel not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a power panel.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/power-panels",
    operation_id = "create_inventory_power_panel",
    tag = "inventory",
    request_body = PowerPanelPayload,
    responses(
        (status = 201, description = "Power panel created", body = PowerPanel),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_power_panel(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<PowerPanelPayload>,
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
    let parsed = match parse_power_panel_payload(&payload) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let item = build_power_panel(Uuid::now_v7(), parsed, payload);
    match state.repositories.power_panels.create(&item).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            (StatusCode::CREATED, Json(item)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create power panels.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/power-panels/bulk",
    operation_id = "bulk_create_inventory_power_panels",
    tag = "inventory",
    request_body = BulkCreateRequest<PowerPanelPayload>,
    responses(
        (status = 200, description = "Bulk power panel create result", body = BulkCreateResponse<PowerPanel>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_power_panels(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(request): EnvelopeJson<BulkCreateRequest<PowerPanelPayload>>,
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
        let parsed = match parse_power_panel_payload(&payload) {
            Ok(value) => value,
            Err(error) => {
                push_bulk_error(&mut errors, index, error);
                continue;
            }
        };
        let item = build_power_panel(Uuid::now_v7(), parsed, payload);

        match state.repositories.power_panels.create(&item).await {
            Ok(()) => created.push(item),
            Err(error) => push_bulk_error(&mut errors, index, error),
        }
    }

    if !created.is_empty() {
        bump_host_config_cache_generation(&state).await;
    }

    (StatusCode::OK, Json(BulkCreateResponse { created, errors })).into_response()
}

/// Update a power panel.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/power-panels/{id}",
    operation_id = "update_inventory_power_panel",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Power panel ID"),
    ),
    request_body = PowerPanelPayload,
    responses(
        (status = 200, description = "Power panel updated", body = PowerPanel),
        (status = 404, description = "Power panel not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_power_panel(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<PowerPanelPayload>,
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
    let entity_id = match parse_uuid(&id, "power_panel_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let parsed = match parse_power_panel_payload(&payload) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match state.repositories.power_panels.get_by_id(entity_id).await {
        Ok(None) => {
            return handle_app_error(AppError::NotFound("Power panel not found".to_string()));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };

    let item = build_power_panel(entity_id, parsed, payload);
    match state.repositories.power_panels.update(&item).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            (StatusCode::OK, Json(item)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Delete a power panel.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/power-panels/{id}",
    operation_id = "delete_inventory_power_panel",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Power panel ID"),
    ),
    responses(
        (status = 204, description = "Power panel deleted"),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_power_panel(
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
    let entity_id = match parse_uuid(&id, "power_panel_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state.repositories.power_panels.get_by_id(entity_id).await {
        Ok(None) => {
            return handle_app_error(AppError::NotFound("Power panel not found".to_string()));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };
    match state.repositories.power_panels.delete(entity_id).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => handle_app_error(error),
    }
}
