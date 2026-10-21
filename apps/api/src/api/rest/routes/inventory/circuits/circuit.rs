//! Circuits REST routes — full CRUD + bulk.
//!
//! Handlers never run SQL directly; all persistence goes through
//! `CircuitRepository` on `AppState`.

use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use chrono::NaiveDate;
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
    BulkCircuitResult, CIRCUIT_COLUMNS, CircuitFilter, CircuitListItem, CircuitRecord,
    CircuitRepository, NewCircuit, SortSpec, UpdateCircuit, parse_csv_strings, parse_csv_uuids,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

use super::super::{
    InventoryListEnvelope, ensure_access, parse_optional_uuid, parse_required_string, parse_uuid,
};

/// Query parameters for circuit listing.
#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct CircuitListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    pub provider_id: Option<String>,
    pub tenant_id: Option<String>,
    pub type_id: Option<String>,
    pub status: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CircuitCreatePayload {
    pub cid: String,
    pub status: Option<String>,
    pub provider_id: String,
    pub type_id: String,
    pub tenant_id: Option<String>,
    pub provider_account_id: Option<String>,
    pub termination_a_id: Option<String>,
    pub termination_z_id: Option<String>,
    pub install_date: Option<NaiveDate>,
    pub termination_date: Option<NaiveDate>,
    pub commit_rate: Option<i32>,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub distance: Option<f64>,
    pub distance_unit: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CircuitUpdatePayload {
    pub cid: Option<String>,
    pub status: Option<String>,
    pub provider_id: Option<String>,
    pub type_id: Option<String>,
    pub tenant_id: Option<String>,
    pub provider_account_id: Option<String>,
    pub termination_a_id: Option<String>,
    pub termination_z_id: Option<String>,
    pub install_date: Option<NaiveDate>,
    pub termination_date: Option<NaiveDate>,
    pub commit_rate: Option<i32>,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub distance: Option<f64>,
    pub distance_unit: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CircuitBulkCreatePayload {
    #[serde(deserialize_with = "crate::api::rest::routes::validation::deserialize_bulk_items")]
    #[schema(min_items = 1)]
    pub items: Vec<CircuitCreatePayload>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CircuitResponse {
    pub id: Uuid,
    pub cid: String,
    pub status: String,
    pub provider_id: Uuid,
    pub type_id: Uuid,
    pub tenant_id: Option<Uuid>,
    pub provider_account_id: Option<Uuid>,
    pub termination_a_id: Option<Uuid>,
    pub termination_z_id: Option<Uuid>,
    pub install_date: Option<NaiveDate>,
    pub termination_date: Option<NaiveDate>,
    pub commit_rate: Option<i32>,
    pub description: String,
    pub comments: String,
    pub distance: Option<f64>,
    pub distance_unit: Option<String>,
}

/// Hydrated circuit list item — extends `CircuitResponse` with FK names.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CircuitListItemResponse {
    pub id: Uuid,
    pub cid: String,
    pub status: String,
    pub provider_id: Uuid,
    pub provider_name: Option<String>,
    pub type_id: Uuid,
    pub type_name: Option<String>,
    pub tenant_id: Option<Uuid>,
    pub tenant_name: Option<String>,
    pub provider_account_id: Option<Uuid>,
    pub termination_a_id: Option<Uuid>,
    pub termination_z_id: Option<Uuid>,
    pub install_date: Option<NaiveDate>,
    pub termination_date: Option<NaiveDate>,
    pub commit_rate: Option<i32>,
    pub description: String,
    pub comments: String,
    pub distance: Option<f64>,
    pub distance_unit: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkCircuitResponse {
    pub created: Vec<CircuitResponse>,
    pub errors: Vec<String>,
}

fn map_circuit_list_item(item: CircuitListItem) -> CircuitListItemResponse {
    let CircuitListItem {
        record,
        provider_name,
        type_name,
        tenant_name,
    } = item;
    CircuitListItemResponse {
        id: record.id,
        cid: record.cid,
        status: record.status,
        provider_id: record.provider_id,
        provider_name,
        type_id: record.type_id,
        type_name,
        tenant_id: record.tenant_id,
        tenant_name,
        provider_account_id: record.provider_account_id,
        termination_a_id: record.termination_a_id,
        termination_z_id: record.termination_z_id,
        install_date: record.install_date,
        termination_date: record.termination_date,
        commit_rate: record.commit_rate,
        description: record.description,
        comments: record.comments,
        distance: record.distance,
        distance_unit: record.distance_unit,
    }
}

fn map_circuit(record: CircuitRecord) -> CircuitResponse {
    CircuitResponse {
        id: record.id,
        cid: record.cid,
        status: record.status,
        provider_id: record.provider_id,
        type_id: record.type_id,
        tenant_id: record.tenant_id,
        provider_account_id: record.provider_account_id,
        termination_a_id: record.termination_a_id,
        termination_z_id: record.termination_z_id,
        install_date: record.install_date,
        termination_date: record.termination_date,
        commit_rate: record.commit_rate,
        description: record.description,
        comments: record.comments,
        distance: record.distance,
        distance_unit: record.distance_unit,
    }
}

fn map_bulk_circuit_result(result: BulkCircuitResult) -> BulkCircuitResponse {
    BulkCircuitResponse {
        created: result.created.into_iter().map(map_circuit).collect(),
        errors: result.errors,
    }
}

/// List circuits.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/circuits",
    tag = "inventory",
    params(CircuitListQuery),
    responses(
        (status = 200, description = "Circuits", body = InventoryListEnvelope<CircuitListItemResponse>),
        (status = "4XX", description = "Circuit list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Circuit list failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_circuits(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<CircuitListQuery>,
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

    let provider_id = match parse_csv_uuids(query.provider_id.as_deref(), "provider_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let tenant_id = match parse_csv_uuids(query.tenant_id.as_deref(), "tenant_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let type_id = match parse_csv_uuids(query.type_id.as_deref(), "type_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let status = parse_csv_strings(query.status.as_deref());
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        CIRCUIT_COLUMNS,
    ) {
        Ok(sort) => sort,
        Err(error) => return handle_app_error(error),
    };
    let resolved_limit = clamp_limit(query.limit);
    let resolved_offset = clamp_offset(query.offset);
    match state
        .repositories
        .circuits
        .list_hydrated(
            CircuitFilter {
                provider_id,
                tenant_id,
                type_id,
                status,
                search: crate::contexts::shared::list_query::search_pattern(
                    query.search.as_deref(),
                ),
                sort,
            },
            query.limit,
            query.offset,
        )
        .await
    {
        Ok((records, total)) => {
            let items: Vec<CircuitListItemResponse> =
                records.into_iter().map(map_circuit_list_item).collect();
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

/// Get a circuit.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/circuits/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Circuit id"),
    ),
    responses(
        (status = 200, description = "Circuit", body = CircuitResponse),
        (status = 404, description = "Circuit not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Circuit get request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Circuit get failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_circuit(
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
    match state.repositories.circuits.get_by_id(id).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_circuit(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Circuit not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a circuit.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/circuits",
    tag = "inventory",
    request_body = CircuitCreatePayload,
    responses(
        (status = 201, description = "Circuit created", body = CircuitResponse),
        (status = "4XX", description = "Circuit create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Circuit create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_circuit(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<CircuitCreatePayload>,
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

    let cid = match parse_required_string(&payload.cid, "cid") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let provider_id = match parse_uuid(&payload.provider_id, "provider_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let type_id = match parse_uuid(&payload.type_id, "type_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let tenant_id = match parse_optional_uuid(payload.tenant_id.clone(), "tenant_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let provider_account_id =
        match parse_optional_uuid(payload.provider_account_id.clone(), "provider_account_id") {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };
    let termination_a_id =
        match parse_optional_uuid(payload.termination_a_id.clone(), "termination_a_id") {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };
    let termination_z_id =
        match parse_optional_uuid(payload.termination_z_id.clone(), "termination_z_id") {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };

    let new_circuit = NewCircuit {
        id: Uuid::now_v7(),
        cid,
        status: payload.status,
        provider_id,
        type_id,
        tenant_id,
        provider_account_id,
        termination_a_id,
        termination_z_id,
        install_date: payload.install_date,
        termination_date: payload.termination_date,
        commit_rate: payload.commit_rate,
        description: payload.description,
        comments: payload.comments,
        distance: payload.distance,
        distance_unit: payload.distance_unit,
    };

    match state.repositories.circuits.create(&new_circuit).await {
        Ok(record) => (StatusCode::CREATED, Json(map_circuit(record))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update a circuit.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/circuits/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Circuit id"),
    ),
    request_body = CircuitUpdatePayload,
    responses(
        (status = 200, description = "Circuit updated", body = CircuitResponse),
        (status = 404, description = "Circuit not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Circuit update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Circuit update failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_circuit(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<CircuitUpdatePayload>,
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
    let provider_id = match parse_optional_uuid(payload.provider_id.clone(), "provider_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let type_id = match parse_optional_uuid(payload.type_id.clone(), "type_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let tenant_id = match parse_optional_uuid(payload.tenant_id.clone(), "tenant_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let provider_account_id =
        match parse_optional_uuid(payload.provider_account_id.clone(), "provider_account_id") {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };
    let termination_a_id =
        match parse_optional_uuid(payload.termination_a_id.clone(), "termination_a_id") {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };
    let termination_z_id =
        match parse_optional_uuid(payload.termination_z_id.clone(), "termination_z_id") {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };

    let changes = UpdateCircuit {
        cid: payload.cid,
        status: payload.status,
        provider_id,
        type_id,
        tenant_id,
        provider_account_id,
        termination_a_id,
        termination_z_id,
        install_date: payload.install_date,
        termination_date: payload.termination_date,
        commit_rate: payload.commit_rate,
        description: payload.description,
        comments: payload.comments,
        distance: payload.distance,
        distance_unit: payload.distance_unit,
    };

    match state.repositories.circuits.update(id, &changes).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_circuit(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Circuit not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a circuit.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/circuits/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Circuit id"),
    ),
    responses(
        (status = 204, description = "Circuit deleted"),
        (status = "4XX", description = "Circuit delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Circuit delete failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_circuit(
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
    match state.repositories.circuits.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create circuits.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/circuits/bulk",
    tag = "inventory",
    request_body = CircuitBulkCreatePayload,
    responses(
        (status = 200, description = "Bulk circuit create result", body = BulkCircuitResponse),
        (status = "4XX", description = "Bulk circuit create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Bulk circuit create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_circuits(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<CircuitBulkCreatePayload>,
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
        let cid = match parse_required_string(&p.cid, "cid") {
            Ok(value) => value,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        let provider_id = match parse_uuid(&p.provider_id, "provider_id") {
            Ok(value) => value,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        let type_id = match parse_uuid(&p.type_id, "type_id") {
            Ok(value) => value,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        let tenant_id = match parse_optional_uuid(p.tenant_id.clone(), "tenant_id") {
            Ok(value) => value,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        let provider_account_id =
            match parse_optional_uuid(p.provider_account_id.clone(), "provider_account_id") {
                Ok(value) => value,
                Err(error) => {
                    return handle_app_error(AppError::Validation(format!(
                        "item {index}: {error:?}"
                    )));
                }
            };
        let termination_a_id =
            match parse_optional_uuid(p.termination_a_id.clone(), "termination_a_id") {
                Ok(value) => value,
                Err(error) => {
                    return handle_app_error(AppError::Validation(format!(
                        "item {index}: {error:?}"
                    )));
                }
            };
        let termination_z_id =
            match parse_optional_uuid(p.termination_z_id.clone(), "termination_z_id") {
                Ok(value) => value,
                Err(error) => {
                    return handle_app_error(AppError::Validation(format!(
                        "item {index}: {error:?}"
                    )));
                }
            };
        items.push(NewCircuit {
            id: Uuid::now_v7(),
            cid,
            status: p.status,
            provider_id,
            type_id,
            tenant_id,
            provider_account_id,
            termination_a_id,
            termination_z_id,
            install_date: p.install_date,
            termination_date: p.termination_date,
            commit_rate: p.commit_rate,
            description: p.description,
            comments: p.comments,
            distance: p.distance,
            distance_unit: p.distance_unit,
        });
    }

    match state.repositories.circuits.bulk_create(&items).await {
        Ok(result) => (StatusCode::OK, Json(map_bulk_circuit_result(result))).into_response(),
        Err(error) => handle_app_error(error),
    }
}
