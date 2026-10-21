//! Rear ports REST routes — full CRUD + bulk.
//!
//! Handlers never run SQL directly; all persistence goes through
//! `RearPortRepository` on `AppState`.

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
use crate::contexts::inventory::infrastructure::persistence::repositories::dcim::rear_port_repository::{
    REAR_PORT_COLUMNS, RearPortFilter,
};
use crate::contexts::inventory::infrastructure::persistence::{
    BulkRearPortResult, NewRearPort, RearPortRecord, RearPortRepository, UpdateRearPort,
};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_strings, parse_csv_uuids};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

use super::super::{
    InventoryListEnvelope, ensure_access, parse_optional_uuid, parse_required_string, parse_uuid,
};

/// Query parameters for rear port listing.
#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct RearPortListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    pub device_id: Option<String>,
    pub module_id: Option<String>,
    #[serde(rename = "type")]
    pub type_: Option<String>,
    pub mark_connected: Option<String>,
    pub cable_id: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct RearPortCreatePayload {
    pub device_id: String,
    pub module_id: Option<String>,
    pub name: String,
    pub label: Option<String>,
    pub description: Option<String>,
    pub type_: String,
    pub color: Option<String>,
    pub positions: i16,
    pub mark_connected: Option<bool>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct RearPortUpdatePayload {
    pub name: Option<String>,
    pub label: Option<String>,
    pub description: Option<String>,
    pub type_: Option<String>,
    pub color: Option<String>,
    pub positions: Option<i16>,
    pub mark_connected: Option<bool>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct RearPortBulkCreatePayload {
    #[serde(deserialize_with = "crate::api::rest::routes::validation::deserialize_bulk_items")]
    #[schema(min_items = 1)]
    pub items: Vec<RearPortCreatePayload>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RearPortResponse {
    pub id: Uuid,
    pub device_id: Uuid,
    pub module_id: Option<Uuid>,
    pub name: String,
    pub label: String,
    pub description: String,
    pub type_: String,
    pub color: String,
    pub positions: i16,
    pub mark_connected: bool,
    pub cable_id: Option<Uuid>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkRearPortResponse {
    pub created: Vec<RearPortResponse>,
    pub errors: Vec<String>,
}

fn map_rear_port(record: RearPortRecord) -> RearPortResponse {
    RearPortResponse {
        id: record.id,
        device_id: record.device_id,
        module_id: record.module_id,
        name: record.name,
        label: record.label,
        description: record.description,
        type_: record.type_,
        color: record.color,
        positions: record.positions,
        mark_connected: record.mark_connected,
        cable_id: record.cable_id,
    }
}

fn map_bulk_rear_port_result(result: BulkRearPortResult) -> BulkRearPortResponse {
    BulkRearPortResponse {
        created: result.created.into_iter().map(map_rear_port).collect(),
        errors: result.errors,
    }
}

/// List rear ports.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/rear-ports",
    tag = "inventory",
    params(RearPortListQuery),
    responses(
        (status = 200, description = "Rear ports", body = InventoryListEnvelope<RearPortResponse>),
        (status = "4XX", description = "Rear port list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Rear port list failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_rear_ports(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<RearPortListQuery>,
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
    let cable_id = match parse_csv_uuids(query.cable_id.as_deref(), "cable_id") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        REAR_PORT_COLUMNS,
    ) {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let filter = RearPortFilter {
        device_id,
        module_id,
        type_: parse_csv_strings(query.type_.as_deref()),
        mark_connected: parse_csv_strings(query.mark_connected.as_deref()),
        cable_id,
        search: crate::contexts::shared::list_query::search_pattern(query.search.as_deref()),
        sort,
    };

    let resolved_limit = clamp_limit(query.limit);
    let resolved_offset = clamp_offset(query.offset);
    match state
        .repositories
        .rear_ports
        .list(filter, query.limit, query.offset)
        .await
    {
        Ok((records, total)) => {
            let items: Vec<RearPortResponse> = records.into_iter().map(map_rear_port).collect();
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

/// Get a rear port.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/rear-ports/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Rear port id"),
    ),
    responses(
        (status = 200, description = "Rear port", body = RearPortResponse),
        (status = 404, description = "Rear port not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Rear port get request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Rear port get failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_rear_port(
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
    match state.repositories.rear_ports.get_by_id(id).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_rear_port(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Rear port not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a rear port.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/rear-ports",
    tag = "inventory",
    request_body = RearPortCreatePayload,
    responses(
        (status = 201, description = "Rear port created", body = RearPortResponse),
        (status = "4XX", description = "Rear port create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Rear port create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_rear_port(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<RearPortCreatePayload>,
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

    let new_port = NewRearPort {
        id: Uuid::now_v7(),
        device_id,
        module_id,
        name,
        label: payload.label,
        description: payload.description,
        type_: payload.type_,
        color: payload.color,
        positions: payload.positions,
        mark_connected: payload.mark_connected,
    };

    match state.repositories.rear_ports.create(&new_port).await {
        Ok(record) => (StatusCode::CREATED, Json(map_rear_port(record))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update a rear port.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/rear-ports/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Rear port id"),
    ),
    request_body = RearPortUpdatePayload,
    responses(
        (status = 200, description = "Rear port updated", body = RearPortResponse),
        (status = 404, description = "Rear port not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Rear port update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Rear port update failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_rear_port(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<RearPortUpdatePayload>,
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

    let changes = UpdateRearPort {
        name: payload.name,
        label: payload.label,
        description: payload.description,
        type_: payload.type_,
        color: payload.color,
        positions: payload.positions,
        mark_connected: payload.mark_connected,
    };

    match state.repositories.rear_ports.update(id, &changes).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_rear_port(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Rear port not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a rear port.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/rear-ports/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Rear port id"),
    ),
    responses(
        (status = 204, description = "Rear port deleted"),
        (status = "4XX", description = "Rear port delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Rear port delete failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_rear_port(
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
    match state.repositories.rear_ports.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create rear ports.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/rear-ports/bulk",
    tag = "inventory",
    request_body = RearPortBulkCreatePayload,
    responses(
        (status = 200, description = "Bulk rear port create result", body = BulkRearPortResponse),
        (status = "4XX", description = "Bulk rear port create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Bulk rear port create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_rear_ports(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<RearPortBulkCreatePayload>,
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
        items.push(NewRearPort {
            id: Uuid::now_v7(),
            device_id,
            module_id,
            name,
            label: p.label,
            description: p.description,
            type_: p.type_,
            color: p.color,
            positions: p.positions,
            mark_connected: p.mark_connected,
        });
    }

    match state.repositories.rear_ports.bulk_create(&items).await {
        Ok(result) => (StatusCode::OK, Json(map_bulk_rear_port_result(result))).into_response(),
        Err(error) => handle_app_error(error),
    }
}
