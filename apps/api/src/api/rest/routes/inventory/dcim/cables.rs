//! Cables REST routes — full CRUD.
//!
//! Handlers never run SQL directly; all persistence goes through `CableRepository` on `AppState`.

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
use crate::contexts::inventory::infrastructure::persistence::{
    BulkCableResult, CABLE_COLUMNS, CableFilter, CableRecord, CableRepository, NewCable,
    UpdateCable, parse_id_filter,
};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_strings};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

use super::super::{InventoryListEnvelope, ensure_access};

#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct CableListQuery {
    /// Comma-separated resource UUIDs (maximum 100). An empty value matches no rows.
    pub ids: Option<String>,
    #[serde(rename = "type")]
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    pub type_: Option<String>,
    pub status: Option<String>,
    pub length_unit: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CableCreatePayload {
    #[serde(rename = "type")]
    pub type_: Option<String>,
    pub status: Option<String>,
    pub label: Option<String>,
    pub color: Option<String>,
    pub length: Option<f64>,
    pub length_unit: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CableUpdatePayload {
    #[serde(rename = "type")]
    pub type_: Option<String>,
    pub status: Option<String>,
    pub label: Option<String>,
    pub color: Option<String>,
    pub length: Option<f64>,
    pub length_unit: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CableBulkCreatePayload {
    #[serde(deserialize_with = "crate::api::rest::routes::validation::deserialize_bulk_items")]
    #[schema(min_items = 1)]
    pub items: Vec<CableCreatePayload>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CableResponse {
    pub id: Uuid,
    #[serde(rename = "type")]
    pub type_: Option<String>,
    pub status: String,
    pub label: Option<String>,
    pub color: Option<String>,
    pub length: Option<f64>,
    pub length_unit: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkCableResponse {
    pub created: Vec<CableResponse>,
    pub errors: Vec<String>,
}

fn map_cable(record: CableRecord) -> CableResponse {
    CableResponse {
        id: record.id,
        type_: record.type_,
        status: record.status,
        label: record.label,
        color: record.color,
        length: record.length,
        length_unit: record.length_unit,
    }
}

fn map_bulk_cable_result(result: BulkCableResult) -> BulkCableResponse {
    BulkCableResponse {
        created: result.created.into_iter().map(map_cable).collect(),
        errors: result.errors,
    }
}

/// List cables.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/cables",
    tag = "inventory",
    params(CableListQuery),
    responses(
        (status = 200, description = "Cables", body = InventoryListEnvelope<CableResponse>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_cables(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<CableListQuery>,
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

    let ids = match parse_id_filter(query.ids.as_deref()) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let resolved_limit = clamp_limit(query.limit);
    let resolved_offset = clamp_offset(query.offset);
    let sort =
        match SortSpec::parse_columns(query.sort.as_deref(), query.order.as_deref(), CABLE_COLUMNS)
        {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };
    let filter = CableFilter {
        ids,
        search: crate::contexts::shared::list_query::search_pattern(query.search.as_deref()),
        type_: parse_csv_strings(query.type_.as_deref()),
        status: parse_csv_strings(query.status.as_deref()),
        length_unit: parse_csv_strings(query.length_unit.as_deref()),
    };
    match state
        .repositories
        .cables
        .list(filter, query.limit, query.offset, sort)
        .await
    {
        Ok((records, total)) => {
            let items: Vec<CableResponse> = records.into_iter().map(map_cable).collect();
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

/// Get a cable.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/cables/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Cable ID"),
    ),
    responses(
        (status = 200, description = "Cable", body = CableResponse),
        (status = 404, description = "Cable not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_cable(
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
    let id = match id.parse::<Uuid>() {
        Ok(v) => v,
        Err(_) => return handle_app_error(AppError::Validation("invalid id".to_string())),
    };
    match state.repositories.cables.get_by_id(id).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_cable(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Cable not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a cable.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/cables",
    tag = "inventory",
    request_body = CableCreatePayload,
    responses(
        (status = 201, description = "Cable created", body = CableResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_cable(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<CableCreatePayload>,
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

    let new_cable = NewCable {
        id: Uuid::now_v7(),
        type_: payload.type_,
        status: payload.status,
        label: payload.label,
        color: payload.color,
        length: payload.length,
        length_unit: payload.length_unit,
    };

    match state.repositories.cables.create(&new_cable).await {
        Ok(record) => (StatusCode::CREATED, Json(map_cable(record))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update a cable.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/cables/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Cable ID"),
    ),
    request_body = CableUpdatePayload,
    responses(
        (status = 200, description = "Cable updated", body = CableResponse),
        (status = 404, description = "Cable not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_cable(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<CableUpdatePayload>,
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
    let id = match id.parse::<Uuid>() {
        Ok(v) => v,
        Err(_) => return handle_app_error(AppError::Validation("invalid id".to_string())),
    };

    let changes = UpdateCable {
        type_: payload.type_,
        status: payload.status,
        label: payload.label,
        color: payload.color,
        length: payload.length,
        length_unit: payload.length_unit,
    };

    match state.repositories.cables.update(id, &changes).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_cable(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Cable not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a cable.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/cables/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Cable ID"),
    ),
    responses(
        (status = 204, description = "Cable deleted"),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_cable(
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
    let id = match id.parse::<Uuid>() {
        Ok(v) => v,
        Err(_) => return handle_app_error(AppError::Validation("invalid id".to_string())),
    };
    match state.repositories.cables.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create cables.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/cables/bulk",
    tag = "inventory",
    request_body = CableBulkCreatePayload,
    responses(
        (status = 200, description = "Bulk cable create result", body = BulkCableResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_cables(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<CableBulkCreatePayload>,
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

    let items: Vec<NewCable> = payload
        .items
        .into_iter()
        .map(|p| NewCable {
            id: Uuid::now_v7(),
            type_: p.type_,
            status: p.status,
            label: p.label,
            color: p.color,
            length: p.length,
            length_unit: p.length_unit,
        })
        .collect();

    match state.repositories.cables.bulk_create(&items).await {
        Ok(result) => (StatusCode::OK, Json(map_bulk_cable_result(result))).into_response(),
        Err(error) => handle_app_error(error),
    }
}
