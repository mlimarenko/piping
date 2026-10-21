//! Power outlets REST routes — full CRUD + bulk.
//!
//! Handlers never run SQL directly; all persistence goes through
//! `PowerOutletRepository` on `AppState`.

use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::infrastructure::persistence::repositories::dcim::power_outlet_repository::{
    POWER_OUTLET_COLUMNS, PowerOutletFilter,
};
use crate::contexts::inventory::infrastructure::persistence::{
    BulkPowerOutletResult, NewPowerOutlet, PowerOutletRecord, PowerOutletRepository,
    UpdatePowerOutlet,
};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_strings, parse_csv_uuids};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

use super::super::{
    InventoryListEnvelope, ensure_access, parse_optional_uuid, parse_required_string, parse_uuid,
};

/// Query parameters for power outlet listing.
#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct PowerOutletListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    pub device_id: Option<String>,
    pub module_id: Option<String>,
    #[serde(rename = "type")]
    pub type_: Option<String>,
    pub feed_leg: Option<String>,
    pub power_port_id: Option<String>,
    pub mark_connected: Option<String>,
    pub cable_id: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct PowerOutletCreatePayload {
    pub device_id: String,
    pub module_id: Option<String>,
    pub name: String,
    pub label: Option<String>,
    pub description: Option<String>,
    pub type_: Option<String>,
    pub feed_leg: Option<String>,
    pub power_port_id: Option<String>,
    pub mark_connected: Option<bool>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct PowerOutletUpdatePayload {
    pub name: Option<String>,
    pub label: Option<String>,
    pub description: Option<String>,
    pub type_: Option<String>,
    pub feed_leg: Option<String>,
    pub power_port_id: Option<String>,
    pub mark_connected: Option<bool>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct PowerOutletBulkCreatePayload {
    #[serde(deserialize_with = "crate::api::rest::routes::validation::deserialize_bulk_items")]
    #[schema(min_items = 1)]
    pub items: Vec<PowerOutletCreatePayload>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PowerOutletResponse {
    pub id: Uuid,
    pub device_id: Uuid,
    pub module_id: Option<Uuid>,
    pub name: String,
    pub label: String,
    pub description: String,
    pub type_: Option<String>,
    pub feed_leg: Option<String>,
    pub power_port_id: Option<Uuid>,
    pub mark_connected: bool,
    pub cable_id: Option<Uuid>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkPowerOutletResponse {
    pub created: Vec<PowerOutletResponse>,
    pub errors: Vec<String>,
}

fn map_power_outlet(record: PowerOutletRecord) -> PowerOutletResponse {
    PowerOutletResponse {
        id: record.id,
        device_id: record.device_id,
        module_id: record.module_id,
        name: record.name,
        label: record.label,
        description: record.description,
        type_: record.type_,
        feed_leg: record.feed_leg,
        power_port_id: record.power_port_id,
        mark_connected: record.mark_connected,
        cable_id: record.cable_id,
    }
}

fn map_bulk_power_outlet_result(result: BulkPowerOutletResult) -> BulkPowerOutletResponse {
    BulkPowerOutletResponse {
        created: result.created.into_iter().map(map_power_outlet).collect(),
        errors: result.errors,
    }
}

/// List power outlets.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/power-outlets",
    tag = "inventory",
    params(PowerOutletListQuery),
    responses(
        (status = 200, description = "Power outlets", body = InventoryListEnvelope<PowerOutletResponse>),
        (status = "4XX", description = "Power outlet list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Power outlet list failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_power_outlets(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<PowerOutletListQuery>,
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

    let device_id = match parse_csv_uuids(query.device_id.as_deref(), "device_id") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let module_id = match parse_csv_uuids(query.module_id.as_deref(), "module_id") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let power_port_id = match parse_csv_uuids(query.power_port_id.as_deref(), "power_port_id") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let cable_id = match parse_csv_uuids(query.cable_id.as_deref(), "cable_id") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        POWER_OUTLET_COLUMNS,
    ) {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let filter = PowerOutletFilter {
        device_id,
        module_id,
        type_: parse_csv_strings(query.type_.as_deref()),
        feed_leg: parse_csv_strings(query.feed_leg.as_deref()),
        power_port_id,
        mark_connected: parse_csv_strings(query.mark_connected.as_deref()),
        cable_id,
        search: crate::contexts::shared::list_query::search_pattern(query.search.as_deref()),
        sort,
    };

    let resolved_limit = clamp_limit(query.limit);
    let resolved_offset = clamp_offset(query.offset);
    match state
        .repositories
        .power_outlets
        .list(filter, query.limit, query.offset)
        .await
    {
        Ok((records, total)) => {
            let items: Vec<PowerOutletResponse> =
                records.into_iter().map(map_power_outlet).collect();
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

/// Get a power outlet.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/power-outlets/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Power outlet id"),
    ),
    responses(
        (status = 200, description = "Power outlet", body = PowerOutletResponse),
        (status = 404, description = "Power outlet not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Power outlet get request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Power outlet get failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_power_outlet(
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
    match state.repositories.power_outlets.get_by_id(id).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_power_outlet(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Power outlet not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a power outlet.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/power-outlets",
    tag = "inventory",
    request_body = PowerOutletCreatePayload,
    responses(
        (status = 201, description = "Power outlet created", body = PowerOutletResponse),
        (status = "4XX", description = "Power outlet create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Power outlet create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_power_outlet(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<PowerOutletCreatePayload>,
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
    let device_id = match parse_uuid(&payload.device_id, "device_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let module_id = match parse_optional_uuid(payload.module_id, "module_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let power_port_id = match parse_optional_uuid(payload.power_port_id, "power_port_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let new_outlet = NewPowerOutlet {
        id: Uuid::now_v7(),
        device_id,
        module_id,
        name,
        label: payload.label,
        description: payload.description,
        type_: payload.type_,
        feed_leg: payload.feed_leg,
        power_port_id,
        mark_connected: payload.mark_connected,
    };

    match state.repositories.power_outlets.create(&new_outlet).await {
        Ok(record) => (StatusCode::CREATED, Json(map_power_outlet(record))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update a power outlet.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/power-outlets/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Power outlet id"),
    ),
    request_body = PowerOutletUpdatePayload,
    responses(
        (status = 200, description = "Power outlet updated", body = PowerOutletResponse),
        (status = 404, description = "Power outlet not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Power outlet update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Power outlet update failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_power_outlet(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<PowerOutletUpdatePayload>,
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
    let power_port_id = match parse_optional_uuid(payload.power_port_id, "power_port_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let changes = UpdatePowerOutlet {
        name: payload.name,
        label: payload.label,
        description: payload.description,
        type_: payload.type_,
        feed_leg: payload.feed_leg,
        power_port_id,
        mark_connected: payload.mark_connected,
    };

    match state.repositories.power_outlets.update(id, &changes).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_power_outlet(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Power outlet not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a power outlet.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/power-outlets/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Power outlet id"),
    ),
    responses(
        (status = 204, description = "Power outlet deleted"),
        (status = "4XX", description = "Power outlet delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Power outlet delete failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_power_outlet(
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
    match state.repositories.power_outlets.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create power outlets.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/power-outlets/bulk",
    tag = "inventory",
    request_body = PowerOutletBulkCreatePayload,
    responses(
        (status = 200, description = "Bulk power outlet create result", body = BulkPowerOutletResponse),
        (status = "4XX", description = "Bulk power outlet create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Bulk power outlet create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_power_outlets(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<PowerOutletBulkCreatePayload>,
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
        let device_id = match parse_uuid(&p.device_id, "device_id") {
            Ok(value) => value,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        let module_id = match parse_optional_uuid(p.module_id, "module_id") {
            Ok(value) => value,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        let power_port_id = match parse_optional_uuid(p.power_port_id, "power_port_id") {
            Ok(value) => value,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        items.push(NewPowerOutlet {
            id: Uuid::now_v7(),
            device_id,
            module_id,
            name,
            label: p.label,
            description: p.description,
            type_: p.type_,
            feed_leg: p.feed_leg,
            power_port_id,
            mark_connected: p.mark_connected,
        });
    }

    match state.repositories.power_outlets.bulk_create(&items).await {
        Ok(result) => (StatusCode::OK, Json(map_bulk_power_outlet_result(result))).into_response(),
        Err(error) => handle_app_error(error),
    }
}
