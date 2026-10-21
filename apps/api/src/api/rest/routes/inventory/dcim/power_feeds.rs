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
use crate::contexts::inventory::domain::PowerFeed;
use crate::contexts::inventory::infrastructure::persistence::repositories::dcim::power_feed_repository::PowerFeedFilter;
use crate::contexts::inventory::infrastructure::persistence::{
    POWER_FEED_COLUMNS, PowerFeedRepository,
};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_strings, parse_csv_uuids};
use crate::errors::AppError;

use super::super::{
    BulkCreateRequest, BulkCreateResponse, InventoryListEnvelope,
    bump_host_config_cache_generation, ensure_access, ensure_create_inventory_access,
    parse_optional_uuid, parse_required_string, parse_uuid, push_bulk_error, require_bulk_items,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};

#[derive(Debug, serde::Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct PowerFeedListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub status: Option<String>,
    pub r#type: Option<String>,
    pub supply: Option<String>,
    pub phase: Option<String>,
    pub power_panel_id: Option<String>,
    pub rack_id: Option<String>,
    pub mark_connected: Option<String>,
    pub tenant_id: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct PowerFeedPayload {
    pub name: String,
    pub status: String,
    pub r#type: String,
    pub supply: String,
    pub phase: String,
    pub voltage: Option<i16>,
    pub amperage: Option<i16>,
    pub max_utilization: Option<i16>,
    pub available_power: Option<i32>,
    pub power_panel_id: String,
    pub rack_id: Option<String>,
    pub mark_connected: Option<bool>,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub tenant_id: Option<String>,
}

struct ParsedPowerFeed {
    name: String,
    status: String,
    r#type: String,
    supply: String,
    phase: String,
    power_panel_id: Uuid,
    rack_id: Option<Uuid>,
    tenant_id: Option<Uuid>,
}

fn parse_power_feed_payload(payload: &PowerFeedPayload) -> Result<ParsedPowerFeed, AppError> {
    Ok(ParsedPowerFeed {
        name: parse_required_string(&payload.name, "name")?,
        status: parse_required_string(&payload.status, "status")?,
        r#type: parse_required_string(&payload.r#type, "type")?,
        supply: parse_required_string(&payload.supply, "supply")?,
        phase: parse_required_string(&payload.phase, "phase")?,
        power_panel_id: parse_uuid(&payload.power_panel_id, "power_panel_id")?,
        rack_id: parse_optional_uuid(payload.rack_id.clone(), "rack_id")?,
        tenant_id: parse_optional_uuid(payload.tenant_id.clone(), "tenant_id")?,
    })
}

fn build_power_feed(id: Uuid, parsed: ParsedPowerFeed, payload: PowerFeedPayload) -> PowerFeed {
    PowerFeed {
        id,
        name: parsed.name,
        status: parsed.status,
        r#type: parsed.r#type,
        supply: parsed.supply,
        phase: parsed.phase,
        voltage: payload.voltage.unwrap_or_default(),
        amperage: payload.amperage.unwrap_or_default(),
        max_utilization: payload.max_utilization.unwrap_or_default(),
        available_power: payload.available_power.unwrap_or_default(),
        power_panel_id: parsed.power_panel_id,
        rack_id: parsed.rack_id,
        mark_connected: payload.mark_connected.unwrap_or_default(),
        description: payload.description.unwrap_or_default(),
        comments: payload.comments.unwrap_or_default(),
        tenant_id: parsed.tenant_id,
        ..Default::default()
    }
}

/// List power feeds.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/power-feeds",
    operation_id = "list_inventory_power_feeds",
    tag = "inventory",
    params(PowerFeedListQuery),
    responses(
        (status = 200, description = "Power feeds", body = InventoryListEnvelope<PowerFeed>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_power_feeds(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<PowerFeedListQuery>,
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
        POWER_FEED_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let power_panel_id = match parse_csv_uuids(query.power_panel_id.as_deref(), "power_panel_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let rack_id = match parse_csv_uuids(query.rack_id.as_deref(), "rack_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let tenant_id = match parse_csv_uuids(query.tenant_id.as_deref(), "tenant_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .power_feeds
        .list_paginated(
            query.limit,
            query.offset,
            PowerFeedFilter {
                search: crate::contexts::shared::list_query::search_pattern(
                    query.search.as_deref(),
                ),
                status: parse_csv_strings(query.status.as_deref()),
                r#type: parse_csv_strings(query.r#type.as_deref()),
                supply: parse_csv_strings(query.supply.as_deref()),
                phase: parse_csv_strings(query.phase.as_deref()),
                power_panel_id,
                rack_id,
                mark_connected: parse_csv_strings(query.mark_connected.as_deref()),
                tenant_id,
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

/// Get a power feed.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/power-feeds/{id}",
    operation_id = "get_inventory_power_feed",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Power feed ID"),
    ),
    responses(
        (status = 200, description = "Power feed", body = PowerFeed),
        (status = 404, description = "Power feed not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_power_feed(
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
    let entity_id = match parse_uuid(&id, "power_feed_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state.repositories.power_feeds.get_by_id(entity_id).await {
        Ok(Some(item)) => (StatusCode::OK, Json(item)).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Power feed not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a power feed.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/power-feeds",
    operation_id = "create_inventory_power_feed",
    tag = "inventory",
    request_body = PowerFeedPayload,
    responses(
        (status = 201, description = "Power feed created", body = PowerFeed),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_power_feed(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<PowerFeedPayload>,
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
    let parsed = match parse_power_feed_payload(&payload) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let item = build_power_feed(Uuid::now_v7(), parsed, payload);
    match state.repositories.power_feeds.create(&item).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            (StatusCode::CREATED, Json(item)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create power feeds.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/power-feeds/bulk",
    operation_id = "bulk_create_inventory_power_feeds",
    tag = "inventory",
    request_body = BulkCreateRequest<PowerFeedPayload>,
    responses(
        (status = 200, description = "Bulk power feed create result", body = BulkCreateResponse<PowerFeed>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_power_feeds(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(request): EnvelopeJson<BulkCreateRequest<PowerFeedPayload>>,
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
        let parsed = match parse_power_feed_payload(&payload) {
            Ok(value) => value,
            Err(error) => {
                push_bulk_error(&mut errors, index, error);
                continue;
            }
        };
        let item = build_power_feed(Uuid::now_v7(), parsed, payload);

        match state.repositories.power_feeds.create(&item).await {
            Ok(()) => created.push(item),
            Err(error) => push_bulk_error(&mut errors, index, error),
        }
    }

    if !created.is_empty() {
        bump_host_config_cache_generation(&state).await;
    }

    (StatusCode::OK, Json(BulkCreateResponse { created, errors })).into_response()
}

/// Update a power feed.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/power-feeds/{id}",
    operation_id = "update_inventory_power_feed",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Power feed ID"),
    ),
    request_body = PowerFeedPayload,
    responses(
        (status = 200, description = "Power feed updated", body = PowerFeed),
        (status = 404, description = "Power feed not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_power_feed(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<PowerFeedPayload>,
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
    let entity_id = match parse_uuid(&id, "power_feed_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let parsed = match parse_power_feed_payload(&payload) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match state.repositories.power_feeds.get_by_id(entity_id).await {
        Ok(None) => {
            return handle_app_error(AppError::NotFound("Power feed not found".to_string()));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };

    let item = build_power_feed(entity_id, parsed, payload);
    match state.repositories.power_feeds.update(&item).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            (StatusCode::OK, Json(item)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Delete a power feed.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/power-feeds/{id}",
    operation_id = "delete_inventory_power_feed",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Power feed ID"),
    ),
    responses(
        (status = 204, description = "Power feed deleted"),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_power_feed(
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
    let entity_id = match parse_uuid(&id, "power_feed_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state.repositories.power_feeds.get_by_id(entity_id).await {
        Ok(None) => {
            return handle_app_error(AppError::NotFound("Power feed not found".to_string()));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };
    match state.repositories.power_feeds.delete(entity_id).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => handle_app_error(error),
    }
}
