//! Aggregate REST routes — full CRUD + bulk.
//!
//! Handlers never run SQL directly; all persistence goes through
//! `AggregateRepository` on `AppState`.

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
use crate::contexts::inventory::infrastructure::persistence::repositories::ipam::aggregate_repository::AggregateListParams;
use crate::contexts::inventory::infrastructure::persistence::{
    AGGREGATE_COLUMNS, AggregateListItem, AggregateRecord, AggregateRepository,
    BulkAggregateResult, NewAggregate, UpdateAggregate,
};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_uuids};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

use super::super::{InventoryListEnvelope, ensure_access, parse_uuid};

/// Query parameters for aggregate listing.
#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct AggregateListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub rir_id: Option<String>,
    pub tenant_id: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct AggregateCreatePayload {
    pub prefix: String,
    pub rir_id: Uuid,
    pub tenant_id: Option<Uuid>,
    pub date_added: Option<chrono::NaiveDate>,
    pub description: Option<String>,
    pub comments: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct AggregateUpdatePayload {
    pub prefix: Option<String>,
    pub rir_id: Option<Uuid>,
    /// Pass `null` explicitly to clear the tenant association.
    pub tenant_id: Option<Option<Uuid>>,
    /// Pass `null` explicitly to clear the added date.
    pub date_added: Option<Option<chrono::NaiveDate>>,
    pub description: Option<String>,
    pub comments: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct AggregateBulkCreatePayload {
    #[serde(deserialize_with = "crate::api::rest::routes::validation::deserialize_bulk_items")]
    #[schema(min_items = 1)]
    pub items: Vec<AggregateCreatePayload>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AggregateResponse {
    pub id: Uuid,
    pub external_id: Option<i64>,
    pub prefix: String,
    pub rir_id: Uuid,
    pub tenant_id: Option<Uuid>,
    pub date_added: Option<chrono::NaiveDate>,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub owner_id: Option<Uuid>,
    pub tags: Vec<String>,
}

/// Hydrated aggregate list item — extends `AggregateResponse` with rir/tenant
/// names for the collection list endpoint.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AggregateListItemResponse {
    pub id: Uuid,
    pub external_id: Option<i64>,
    pub prefix: String,
    pub rir_id: Uuid,
    pub rir_name: Option<String>,
    pub tenant_id: Option<Uuid>,
    pub tenant_name: Option<String>,
    pub date_added: Option<chrono::NaiveDate>,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub owner_id: Option<Uuid>,
    pub tags: Vec<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkAggregateResponse {
    pub created: Vec<AggregateResponse>,
    pub errors: Vec<String>,
}

fn map_aggregate_list_item(item: AggregateListItem) -> AggregateListItemResponse {
    let AggregateListItem {
        record,
        rir_name,
        tenant_name,
    } = item;
    AggregateListItemResponse {
        id: record.id,
        external_id: record.external_id,
        prefix: record.prefix,
        rir_id: record.rir_id,
        rir_name,
        tenant_id: record.tenant_id,
        tenant_name,
        date_added: record.date_added,
        description: record.description,
        comments: record.comments,
        owner_id: record.owner_id,
        tags: record.tags,
    }
}

fn map_aggregate(record: AggregateRecord) -> AggregateResponse {
    AggregateResponse {
        id: record.id,
        external_id: record.external_id,
        prefix: record.prefix,
        rir_id: record.rir_id,
        tenant_id: record.tenant_id,
        date_added: record.date_added,
        description: record.description,
        comments: record.comments,
        owner_id: record.owner_id,
        tags: record.tags,
    }
}

fn map_bulk_aggregate_result(result: BulkAggregateResult) -> BulkAggregateResponse {
    BulkAggregateResponse {
        created: result.created.into_iter().map(map_aggregate).collect(),
        errors: result.errors,
    }
}

/// List aggregates.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/aggregates",
    tag = "inventory",
    params(AggregateListQuery),
    responses(
        (status = 200, description = "Aggregates", body = InventoryListEnvelope<AggregateListItemResponse>),
        (status = "4XX", description = "Aggregate list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Aggregate list failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_aggregates(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<AggregateListQuery>,
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
        AGGREGATE_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let rir_ids = match parse_csv_uuids(query.rir_id.as_deref(), "rir_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let tenant_ids = match parse_csv_uuids(query.tenant_id.as_deref(), "tenant_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .aggregates
        .list(AggregateListParams {
            limit: query.limit,
            offset: query.offset,
            rir_ids: &rir_ids,
            tenant_ids: &tenant_ids,
            search: crate::contexts::shared::list_query::search_pattern(query.search.as_deref()),
            sort,
        })
        .await
    {
        Ok((records, total)) => {
            let items: Vec<AggregateListItemResponse> =
                records.into_iter().map(map_aggregate_list_item).collect();
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

/// Get an aggregate.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/aggregates/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Aggregate id"),
    ),
    responses(
        (status = 200, description = "Aggregate", body = AggregateResponse),
        (status = 404, description = "Aggregate not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Aggregate get request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Aggregate get failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_aggregate(
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
    match state.repositories.aggregates.get_by_id(id).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_aggregate(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Aggregate not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create an aggregate.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/aggregates",
    tag = "inventory",
    request_body = AggregateCreatePayload,
    responses(
        (status = 201, description = "Aggregate created", body = AggregateResponse),
        (status = "4XX", description = "Aggregate create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Aggregate create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_aggregate(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<AggregateCreatePayload>,
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

    let new_aggregate = NewAggregate {
        id: Uuid::now_v7(),
        prefix: payload.prefix,
        rir_id: payload.rir_id,
        tenant_id: payload.tenant_id,
        date_added: payload.date_added,
        description: payload.description,
        comments: payload.comments,
        owner_id: None,
    };

    match state.repositories.aggregates.create(&new_aggregate).await {
        Ok(record) => (StatusCode::CREATED, Json(map_aggregate(record))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update an aggregate.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/aggregates/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Aggregate id"),
    ),
    request_body = AggregateUpdatePayload,
    responses(
        (status = 200, description = "Aggregate updated", body = AggregateResponse),
        (status = 404, description = "Aggregate not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Aggregate update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Aggregate update failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_aggregate(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<AggregateUpdatePayload>,
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

    let changes = UpdateAggregate {
        prefix: payload.prefix,
        rir_id: payload.rir_id,
        tenant_id: payload.tenant_id,
        date_added: payload.date_added,
        description: payload.description,
        comments: payload.comments,
    };

    match state.repositories.aggregates.update(id, &changes).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_aggregate(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Aggregate not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Delete an aggregate.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/aggregates/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Aggregate id"),
    ),
    responses(
        (status = 204, description = "Aggregate deleted"),
        (status = "4XX", description = "Aggregate delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Aggregate delete failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_aggregate(
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
    match state.repositories.aggregates.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create aggregates.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/aggregates/bulk",
    tag = "inventory",
    request_body = AggregateBulkCreatePayload,
    responses(
        (status = 200, description = "Bulk aggregate create result", body = BulkAggregateResponse),
        (status = "4XX", description = "Bulk aggregate create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Bulk aggregate create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_aggregates(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<AggregateBulkCreatePayload>,
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

    let items: Vec<NewAggregate> = payload
        .items
        .into_iter()
        .map(|p| NewAggregate {
            id: Uuid::now_v7(),
            prefix: p.prefix,
            rir_id: p.rir_id,
            tenant_id: p.tenant_id,
            date_added: p.date_added,
            description: p.description,
            comments: p.comments,
            owner_id: None,
        })
        .collect();

    match state.repositories.aggregates.bulk_create(&items).await {
        Ok(result) => (StatusCode::OK, Json(map_bulk_aggregate_result(result))).into_response(),
        Err(error) => handle_app_error(error),
    }
}
