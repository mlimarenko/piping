//! Circuit types REST routes — full CRUD + bulk.
//!
//! Handlers never run SQL directly; all persistence goes through
//! `CircuitTypeRepository` on `AppState`.

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
use crate::contexts::inventory::infrastructure::persistence::repositories::derive_slug;
use crate::contexts::inventory::infrastructure::persistence::{
    BulkCircuitTypeResult, CIRCUIT_TYPE_COLUMNS, CircuitTypeRecord, CircuitTypeRepository,
    NewCircuitType, UpdateCircuitType,
};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_uuids};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

use super::super::{InventoryListEnvelope, ensure_access, parse_required_string, parse_uuid};

/// Query parameters for circuit type listing.
#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct CircuitTypeListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub owner_id: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CircuitTypeCreatePayload {
    pub name: String,
    pub slug: Option<String>,
    pub color: Option<String>,
    pub description: Option<String>,
    pub custom_fields: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CircuitTypeUpdatePayload {
    pub name: Option<String>,
    pub slug: Option<String>,
    pub color: Option<String>,
    pub description: Option<String>,
    pub custom_fields: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CircuitTypeBulkCreatePayload {
    #[serde(deserialize_with = "crate::api::rest::routes::validation::deserialize_bulk_items")]
    #[schema(min_items = 1)]
    pub items: Vec<CircuitTypeCreatePayload>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CircuitTypeResponse {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub color: Option<String>,
    pub description: Option<String>,
    pub owner_id: Option<Uuid>,
    pub custom_fields: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkCircuitTypeResponse {
    pub created: Vec<CircuitTypeResponse>,
    pub errors: Vec<String>,
}

fn map_circuit_type(record: CircuitTypeRecord) -> CircuitTypeResponse {
    CircuitTypeResponse {
        id: record.id,
        name: record.name,
        slug: record.slug,
        color: record.color,
        description: record.description,
        owner_id: record.owner_id,
        custom_fields: record.custom_fields,
    }
}

fn map_bulk_circuit_type_result(result: BulkCircuitTypeResult) -> BulkCircuitTypeResponse {
    BulkCircuitTypeResponse {
        created: result.created.into_iter().map(map_circuit_type).collect(),
        errors: result.errors,
    }
}

fn resolve_slug(slug: Option<String>, name: &str) -> String {
    slug.map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| derive_slug(name))
}

/// List circuit types.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/circuit-types",
    tag = "inventory",
    params(CircuitTypeListQuery),
    responses(
        (status = 200, description = "Circuit types", body = InventoryListEnvelope<CircuitTypeResponse>),
        (status = "4XX", description = "Circuit type list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Circuit type list failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_circuit_types(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<CircuitTypeListQuery>,
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
        CIRCUIT_TYPE_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let owner_ids = match parse_csv_uuids(query.owner_id.as_deref(), "owner_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .circuit_types
        .list(
            query.limit,
            query.offset,
            &owner_ids,
            crate::contexts::shared::list_query::search_pattern(query.search.as_deref()),
            sort,
        )
        .await
    {
        Ok((records, total)) => {
            let items: Vec<CircuitTypeResponse> =
                records.into_iter().map(map_circuit_type).collect();
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

/// Get a circuit type.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/circuit-types/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Circuit type id"),
    ),
    responses(
        (status = 200, description = "Circuit type", body = CircuitTypeResponse),
        (status = 404, description = "Circuit type not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Circuit type get request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Circuit type get failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_circuit_type(
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
    match state.repositories.circuit_types.get_by_id(id).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_circuit_type(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Circuit type not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a circuit type.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/circuit-types",
    tag = "inventory",
    request_body = CircuitTypeCreatePayload,
    responses(
        (status = 201, description = "Circuit type created", body = CircuitTypeResponse),
        (status = "4XX", description = "Circuit type create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Circuit type create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_circuit_type(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<CircuitTypeCreatePayload>,
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
    let slug = resolve_slug(payload.slug, &name);

    let new_ct = NewCircuitType {
        id: Uuid::now_v7(),
        name,
        slug,
        color: payload.color,
        description: payload.description,
        owner_id: None,
        custom_fields: payload.custom_fields,
    };

    match state.repositories.circuit_types.create(&new_ct).await {
        Ok(record) => (StatusCode::CREATED, Json(map_circuit_type(record))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update a circuit type.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/circuit-types/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Circuit type id"),
    ),
    request_body = CircuitTypeUpdatePayload,
    responses(
        (status = 200, description = "Circuit type updated", body = CircuitTypeResponse),
        (status = 404, description = "Circuit type not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Circuit type update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Circuit type update failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_circuit_type(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<CircuitTypeUpdatePayload>,
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

    let changes = UpdateCircuitType {
        name: payload.name,
        slug: payload.slug,
        color: payload.color,
        description: payload.description,
        custom_fields: payload.custom_fields,
    };

    match state.repositories.circuit_types.update(id, &changes).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_circuit_type(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Circuit type not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a circuit type.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/circuit-types/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Circuit type id"),
    ),
    responses(
        (status = 204, description = "Circuit type deleted"),
        (status = "4XX", description = "Circuit type delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Circuit type delete failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_circuit_type(
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
    match state.repositories.circuit_types.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create circuit types.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/circuit-types/bulk",
    tag = "inventory",
    request_body = CircuitTypeBulkCreatePayload,
    responses(
        (status = 200, description = "Bulk circuit type create result", body = BulkCircuitTypeResponse),
        (status = "4XX", description = "Bulk circuit type create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Bulk circuit type create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_circuit_types(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<CircuitTypeBulkCreatePayload>,
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
        let slug = resolve_slug(p.slug, &name);
        items.push(NewCircuitType {
            id: Uuid::now_v7(),
            name,
            slug,
            color: p.color,
            description: p.description,
            owner_id: None,
            custom_fields: p.custom_fields,
        });
    }

    match state.repositories.circuit_types.bulk_create(&items).await {
        Ok(result) => (StatusCode::OK, Json(map_bulk_circuit_type_result(result))).into_response(),
        Err(error) => handle_app_error(error),
    }
}
