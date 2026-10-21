//! RIR REST routes — full CRUD + bulk.
//!
//! Handlers never run SQL directly; all persistence goes through
//! `RirRepository` on `AppState`.

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
use crate::contexts::inventory::infrastructure::persistence::repositories::ipam::rir_repository::RirListParams;
use crate::contexts::inventory::infrastructure::persistence::{
    BulkCreateResult, NewRir, RIR_COLUMNS, RirRecord, RirRepository, UpdateRir,
};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_strings, parse_csv_uuids};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

use super::super::{InventoryListEnvelope, ensure_access, parse_required_string, parse_uuid};

/// Query parameters for RIR listing.
#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct RirListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub is_private: Option<String>,
    pub owner_id: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct RirCreatePayload {
    pub name: String,
    pub slug: Option<String>,
    pub is_private: Option<bool>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct RirUpdatePayload {
    pub name: Option<String>,
    pub slug: Option<String>,
    pub is_private: Option<bool>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct RirBulkCreatePayload {
    #[serde(deserialize_with = "crate::api::rest::routes::validation::deserialize_bulk_items")]
    #[schema(min_items = 1)]
    pub items: Vec<RirCreatePayload>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RirResponse {
    pub id: Uuid,
    pub external_id: Option<i64>,
    pub name: String,
    pub slug: String,
    pub is_private: bool,
    pub description: Option<String>,
    pub owner_id: Option<Uuid>,
    pub tags: Vec<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkRirResponse {
    pub created: Vec<RirResponse>,
    pub errors: Vec<String>,
}

fn map_rir(record: RirRecord) -> RirResponse {
    RirResponse {
        id: record.id,
        external_id: record.external_id,
        name: record.name,
        slug: record.slug,
        is_private: record.is_private,
        description: record.description,
        owner_id: record.owner_id,
        tags: record.tags,
    }
}

fn map_bulk_rir_result(result: BulkCreateResult<RirRecord>) -> BulkRirResponse {
    BulkRirResponse {
        created: result.created.into_iter().map(map_rir).collect(),
        errors: result.errors,
    }
}

fn resolve_slug(slug: Option<String>, name: &str) -> String {
    slug.map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| derive_slug(name))
}

/// List RIRs.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/rirs",
    tag = "inventory",
    params(RirListQuery),
    responses(
        (status = 200, description = "RIRs", body = InventoryListEnvelope<RirResponse>),
        (status = "4XX", description = "RIR list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "RIR list failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_rirs(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<RirListQuery>,
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
    let sort =
        match SortSpec::parse_columns(query.sort.as_deref(), query.order.as_deref(), RIR_COLUMNS) {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };
    let owner_ids = match parse_csv_uuids(query.owner_id.as_deref(), "owner_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let is_private = parse_csv_strings(query.is_private.as_deref());
    match state
        .repositories
        .rirs
        .list(RirListParams {
            limit: query.limit,
            offset: query.offset,
            is_private: &is_private,
            owner_ids: &owner_ids,
            search: crate::contexts::shared::list_query::search_pattern(query.search.as_deref()),
            sort,
        })
        .await
    {
        Ok((records, total)) => {
            let items: Vec<RirResponse> = records.into_iter().map(map_rir).collect();
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

/// Get a RIR.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/rirs/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "RIR id"),
    ),
    responses(
        (status = 200, description = "RIR", body = RirResponse),
        (status = 404, description = "RIR not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "RIR get request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "RIR get failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_rir(
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
    match state.repositories.rirs.get_by_id(id).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_rir(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("RIR not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a RIR.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/rirs",
    tag = "inventory",
    request_body = RirCreatePayload,
    responses(
        (status = 201, description = "RIR created", body = RirResponse),
        (status = "4XX", description = "RIR create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "RIR create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_rir(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<RirCreatePayload>,
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

    let new_rir = NewRir {
        id: Uuid::now_v7(),
        name,
        slug,
        is_private: payload.is_private.unwrap_or(false),
        description: payload.description,
        owner_id: None,
    };

    match state.repositories.rirs.create(&new_rir).await {
        Ok(record) => (StatusCode::CREATED, Json(map_rir(record))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update a RIR.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/rirs/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "RIR id"),
    ),
    request_body = RirUpdatePayload,
    responses(
        (status = 200, description = "RIR updated", body = RirResponse),
        (status = 404, description = "RIR not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "RIR update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "RIR update failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_rir(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<RirUpdatePayload>,
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

    let changes = UpdateRir {
        name: payload.name,
        slug: payload.slug,
        is_private: payload.is_private,
        description: payload.description,
    };

    match state.repositories.rirs.update(id, &changes).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_rir(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("RIR not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a RIR.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/rirs/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "RIR id"),
    ),
    responses(
        (status = 204, description = "RIR deleted"),
        (status = "4XX", description = "RIR delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "RIR delete failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_rir(
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
    match state.repositories.rirs.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create RIRs.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/rirs/bulk",
    tag = "inventory",
    request_body = RirBulkCreatePayload,
    responses(
        (status = 200, description = "Bulk RIR create result", body = BulkRirResponse),
        (status = "4XX", description = "Bulk RIR create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Bulk RIR create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_rirs(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<RirBulkCreatePayload>,
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
        items.push(NewRir {
            id: Uuid::now_v7(),
            name,
            slug,
            is_private: p.is_private.unwrap_or(false),
            description: p.description,
            owner_id: None,
        });
    }

    match state.repositories.rirs.bulk_create(&items).await {
        Ok(result) => (StatusCode::OK, Json(map_bulk_rir_result(result))).into_response(),
        Err(error) => handle_app_error(error),
    }
}
