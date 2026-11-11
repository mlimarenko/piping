//! Console server ports REST routes — full CRUD + bulk.
//!
//! Handlers never run SQL directly; all persistence goes through
//! `ConsoleServerPortRepository` on `AppState`.

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
use crate::contexts::inventory::infrastructure::persistence::repositories::dcim::console_server_port_repository::{
    CONSOLE_SERVER_PORT_COLUMNS, ConsoleServerPortFilter,
};
use crate::contexts::inventory::infrastructure::persistence::{
    BulkConsoleServerPortResult, ConsoleServerPortRecord, ConsoleServerPortRepository,
    NewConsoleServerPort, UpdateConsoleServerPort,
};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_strings, parse_csv_uuids};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

use super::super::{
    InventoryListEnvelope, ensure_access, parse_optional_uuid, parse_required_string, parse_uuid,
};

/// Query parameters for console server port listing.
#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct ConsoleServerPortListQuery {
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
pub struct ConsoleServerPortCreatePayload {
    pub device_id: String,
    pub module_id: Option<String>,
    pub name: String,
    pub label: Option<String>,
    pub description: Option<String>,
    pub type_: Option<String>,
    pub speed: Option<i32>,
    pub mark_connected: Option<bool>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ConsoleServerPortUpdatePayload {
    pub name: Option<String>,
    pub label: Option<String>,
    pub description: Option<String>,
    pub type_: Option<String>,
    pub speed: Option<i32>,
    pub mark_connected: Option<bool>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ConsoleServerPortBulkCreatePayload {
    #[serde(deserialize_with = "crate::api::rest::routes::validation::deserialize_bulk_items")]
    #[schema(min_items = 1)]
    pub items: Vec<ConsoleServerPortCreatePayload>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ConsoleServerPortResponse {
    pub id: Uuid,
    pub device_id: Uuid,
    pub module_id: Option<Uuid>,
    pub name: String,
    pub label: String,
    pub description: String,
    pub type_: Option<String>,
    pub speed: Option<i32>,
    pub mark_connected: bool,
    pub cable_id: Option<Uuid>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkConsoleServerPortResponse {
    pub created: Vec<ConsoleServerPortResponse>,
    pub errors: Vec<String>,
}

fn map_console_server_port(record: ConsoleServerPortRecord) -> ConsoleServerPortResponse {
    ConsoleServerPortResponse {
        id: record.id,
        device_id: record.device_id,
        module_id: record.module_id,
        name: record.name,
        label: record.label,
        description: record.description,
        type_: record.type_,
        speed: record.speed,
        mark_connected: record.mark_connected,
        cable_id: record.cable_id,
    }
}

fn map_bulk_console_server_port_result(
    result: BulkConsoleServerPortResult,
) -> BulkConsoleServerPortResponse {
    BulkConsoleServerPortResponse {
        created: result
            .created
            .into_iter()
            .map(map_console_server_port)
            .collect(),
        errors: result.errors,
    }
}

/// List console server ports.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/console-server-ports",
    tag = "inventory",
    params(ConsoleServerPortListQuery),
    responses(
        (status = 200, description = "Console server ports", body = InventoryListEnvelope<ConsoleServerPortResponse>),
        (status = "4XX", description = "Console server port list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Console server port list failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_console_server_ports(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ConsoleServerPortListQuery>,
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
        CONSOLE_SERVER_PORT_COLUMNS,
    ) {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let filter = ConsoleServerPortFilter {
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
        .console_server_ports
        .list(filter, query.limit, query.offset)
        .await
    {
        Ok((records, total)) => {
            let items: Vec<ConsoleServerPortResponse> =
                records.into_iter().map(map_console_server_port).collect();
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

/// Get a console server port.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/console-server-ports/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Console server port id"),
    ),
    responses(
        (status = 200, description = "Console server port", body = ConsoleServerPortResponse),
        (status = 404, description = "Console server port not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Console server port get request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Console server port get failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_console_server_port(
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
    match state.repositories.console_server_ports.get_by_id(id).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_console_server_port(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound(
            "Console server port not found".to_string(),
        )),
        Err(error) => handle_app_error(error),
    }
}

/// Create a console server port.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/console-server-ports",
    tag = "inventory",
    request_body = ConsoleServerPortCreatePayload,
    responses(
        (status = 201, description = "Console server port created", body = ConsoleServerPortResponse),
        (status = "4XX", description = "Console server port create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Console server port create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_console_server_port(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<ConsoleServerPortCreatePayload>,
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

    let new_port = NewConsoleServerPort {
        id: Uuid::now_v7(),
        device_id,
        module_id,
        name,
        label: payload.label,
        description: payload.description,
        type_: payload.type_,
        speed: payload.speed,
        mark_connected: payload.mark_connected,
    };

    match state
        .repositories
        .console_server_ports
        .create(&new_port)
        .await
    {
        Ok(record) => (StatusCode::CREATED, Json(map_console_server_port(record))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update a console server port.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/console-server-ports/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Console server port id"),
    ),
    request_body = ConsoleServerPortUpdatePayload,
    responses(
        (status = 200, description = "Console server port updated", body = ConsoleServerPortResponse),
        (status = 404, description = "Console server port not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Console server port update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Console server port update failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_console_server_port(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<ConsoleServerPortUpdatePayload>,
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

    let changes = UpdateConsoleServerPort {
        name: payload.name,
        label: payload.label,
        description: payload.description,
        type_: payload.type_,
        speed: payload.speed,
        mark_connected: payload.mark_connected,
    };

    match state
        .repositories
        .console_server_ports
        .update(id, &changes)
        .await
    {
        Ok(Some(record)) => (StatusCode::OK, Json(map_console_server_port(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound(
            "Console server port not found".to_string(),
        )),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a console server port.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/console-server-ports/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Console server port id"),
    ),
    responses(
        (status = 204, description = "Console server port deleted"),
        (status = "4XX", description = "Console server port delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Console server port delete failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_console_server_port(
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
    match state.repositories.console_server_ports.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create console server ports.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/console-server-ports/bulk",
    tag = "inventory",
    request_body = ConsoleServerPortBulkCreatePayload,
    responses(
        (status = 200, description = "Bulk console server port create result", body = BulkConsoleServerPortResponse),
        (status = "4XX", description = "Bulk console server port create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Bulk console server port create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_console_server_ports(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<ConsoleServerPortBulkCreatePayload>,
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
        items.push(NewConsoleServerPort {
            id: Uuid::now_v7(),
            device_id,
            module_id,
            name,
            label: p.label,
            description: p.description,
            type_: p.type_,
            speed: p.speed,
            mark_connected: p.mark_connected,
        });
    }

    match state
        .repositories
        .console_server_ports
        .bulk_create(&items)
        .await
    {
        Ok(result) => (
            StatusCode::OK,
            Json(map_bulk_console_server_port_result(result)),
        )
            .into_response(),
        Err(error) => handle_app_error(error),
    }
}
