//! Route target REST routes — full CRUD + bulk.
//!
//! Handlers never run SQL directly; all persistence goes through
//! `RouteTargetRepository` on `AppState`.

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
    BulkRouteTargetResult, NewRouteTarget, ROUTE_TARGET_COLUMNS, RouteTargetListItem,
    RouteTargetRecord, RouteTargetRepository, UpdateRouteTarget,
};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_uuids};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

use super::super::{InventoryListEnvelope, ensure_access, parse_required_string, parse_uuid};

/// Query parameters for route target listing.
#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct RouteTargetListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub tenant_id: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct RouteTargetCreatePayload {
    pub name: String,
    pub description: Option<String>,
    pub tenant_id: Option<Uuid>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct RouteTargetUpdatePayload {
    pub name: Option<String>,
    pub description: Option<String>,
    pub tenant_id: Option<Uuid>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct RouteTargetBulkCreatePayload {
    #[serde(deserialize_with = "crate::api::rest::routes::validation::deserialize_bulk_items")]
    #[schema(min_items = 1)]
    pub items: Vec<RouteTargetCreatePayload>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RouteTargetResponse {
    pub id: Uuid,
    pub external_id: Option<i64>,
    pub name: String,
    pub description: Option<String>,
    pub tenant_id: Option<Uuid>,
    pub owner_id: Option<Uuid>,
    pub tags: Vec<String>,
}

/// Hydrated route target list item — extends `RouteTargetResponse` with the
/// tenant name for the collection list endpoint.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RouteTargetListItemResponse {
    pub id: Uuid,
    pub external_id: Option<i64>,
    pub name: String,
    pub description: Option<String>,
    pub tenant_id: Option<Uuid>,
    pub tenant_name: Option<String>,
    pub owner_id: Option<Uuid>,
    pub tags: Vec<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkRouteTargetResponse {
    pub created: Vec<RouteTargetResponse>,
    pub errors: Vec<String>,
}

fn map_route_target_list_item(item: RouteTargetListItem) -> RouteTargetListItemResponse {
    let RouteTargetListItem {
        record,
        tenant_name,
    } = item;
    RouteTargetListItemResponse {
        id: record.id,
        external_id: record.external_id,
        name: record.name,
        description: record.description,
        tenant_id: record.tenant_id,
        tenant_name,
        owner_id: record.owner_id,
        tags: record.tags,
    }
}

fn map_route_target(record: RouteTargetRecord) -> RouteTargetResponse {
    RouteTargetResponse {
        id: record.id,
        external_id: record.external_id,
        name: record.name,
        description: record.description,
        tenant_id: record.tenant_id,
        owner_id: record.owner_id,
        tags: record.tags,
    }
}

fn map_bulk_route_target_result(result: BulkRouteTargetResult) -> BulkRouteTargetResponse {
    BulkRouteTargetResponse {
        created: result.created.into_iter().map(map_route_target).collect(),
        errors: result.errors,
    }
}

/// List route targets.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/route_targets",
    tag = "inventory",
    params(RouteTargetListQuery),
    responses(
        (status = 200, description = "Route targets", body = InventoryListEnvelope<RouteTargetListItemResponse>),
        (status = "4XX", description = "Route target list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Route target list failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_route_targets(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<RouteTargetListQuery>,
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
        ROUTE_TARGET_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let tenant_ids = match parse_csv_uuids(query.tenant_id.as_deref(), "tenant_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .route_targets
        .list(
            query.limit,
            query.offset,
            &tenant_ids,
            crate::contexts::shared::list_query::search_pattern(query.search.as_deref()),
            sort,
        )
        .await
    {
        Ok((records, total)) => {
            let items: Vec<RouteTargetListItemResponse> = records
                .into_iter()
                .map(map_route_target_list_item)
                .collect();
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

/// Get a route target.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/route_targets/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Route target id"),
    ),
    responses(
        (status = 200, description = "Route target", body = RouteTargetResponse),
        (status = 404, description = "Route target not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Route target get request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Route target get failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_route_target(
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
    match state.repositories.route_targets.get_by_id(id).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_route_target(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Route target not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a route target.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/route_targets",
    tag = "inventory",
    request_body = RouteTargetCreatePayload,
    responses(
        (status = 201, description = "Route target created", body = RouteTargetResponse),
        (status = "4XX", description = "Route target create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Route target create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_route_target(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<RouteTargetCreatePayload>,
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

    let new_target = NewRouteTarget {
        id: Uuid::now_v7(),
        name,
        description: payload.description,
        tenant_id: payload.tenant_id,
        owner_id: None,
    };

    match state.repositories.route_targets.create(&new_target).await {
        Ok(record) => (StatusCode::CREATED, Json(map_route_target(record))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update a route target.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/route_targets/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Route target id"),
    ),
    request_body = RouteTargetUpdatePayload,
    responses(
        (status = 200, description = "Route target updated", body = RouteTargetResponse),
        (status = 404, description = "Route target not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Route target update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Route target update failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_route_target(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<RouteTargetUpdatePayload>,
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

    let changes = UpdateRouteTarget {
        name: payload.name,
        description: payload.description,
        tenant_id: payload.tenant_id,
    };

    match state.repositories.route_targets.update(id, &changes).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_route_target(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Route target not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a route target.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/route_targets/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Route target id"),
    ),
    responses(
        (status = 204, description = "Route target deleted"),
        (status = "4XX", description = "Route target delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Route target delete failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_route_target(
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
    match state.repositories.route_targets.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create route targets.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/route_targets/bulk",
    tag = "inventory",
    request_body = RouteTargetBulkCreatePayload,
    responses(
        (status = 200, description = "Bulk route target create result", body = BulkRouteTargetResponse),
        (status = "4XX", description = "Bulk route target create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Bulk route target create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_route_targets(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<RouteTargetBulkCreatePayload>,
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
        items.push(NewRouteTarget {
            id: Uuid::now_v7(),
            name,
            description: p.description,
            tenant_id: p.tenant_id,
            owner_id: None,
        });
    }

    match state.repositories.route_targets.bulk_create(&items).await {
        Ok(result) => (StatusCode::OK, Json(map_bulk_route_target_result(result))).into_response(),
        Err(error) => handle_app_error(error),
    }
}
