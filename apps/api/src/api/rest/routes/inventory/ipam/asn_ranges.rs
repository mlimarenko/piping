//! ASN range REST routes — full CRUD + bulk.
//!
//! Handlers never run SQL directly; all persistence goes through
//! `AsnRangeRepository` on `AppState`.
//! Route base: `/api/v1/inventory/asn-ranges` (kebab-case per convention).

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
use crate::contexts::inventory::infrastructure::persistence::repositories::ipam::asn_range_repository::AsnRangeListParams;
use crate::contexts::inventory::infrastructure::persistence::repositories::derive_slug;
use crate::contexts::inventory::infrastructure::persistence::{
    ASN_RANGE_COLUMNS, AsnRangeRecord, AsnRangeRepository, BulkAsnRangeResult, NewAsnRange,
    UpdateAsnRange,
};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_uuids};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

use super::super::{InventoryListEnvelope, ensure_access, parse_required_string, parse_uuid};

/// Query parameters for ASN range listing.
#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct AsnRangeListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    pub rir_id: Option<String>,
    pub tenant_id: Option<String>,
    pub owner_id: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct AsnRangeCreatePayload {
    pub name: String,
    pub slug: Option<String>,
    pub rir_id: Uuid,
    pub start: i64,
    pub end: i64,
    pub tenant_id: Option<Uuid>,
    pub description: Option<String>,
    pub comments: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct AsnRangeUpdatePayload {
    pub name: Option<String>,
    pub slug: Option<String>,
    pub rir_id: Option<Uuid>,
    pub start: Option<i64>,
    pub end: Option<i64>,
    /// Pass `null` explicitly to clear the tenant association.
    pub tenant_id: Option<Option<Uuid>>,
    pub description: Option<String>,
    pub comments: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct AsnRangeBulkCreatePayload {
    #[serde(deserialize_with = "crate::api::rest::routes::validation::deserialize_bulk_items")]
    #[schema(min_items = 1)]
    pub items: Vec<AsnRangeCreatePayload>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AsnRangeResponse {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub rir_id: Uuid,
    pub start: i64,
    pub end: i64,
    pub tenant_id: Option<Uuid>,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub owner_id: Option<Uuid>,
    pub owner_name: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkAsnRangeResponse {
    pub created: Vec<AsnRangeResponse>,
    pub errors: Vec<String>,
}

fn map_asn_range(record: AsnRangeRecord) -> AsnRangeResponse {
    AsnRangeResponse {
        id: record.id,
        name: record.name,
        slug: record.slug,
        rir_id: record.rir_id,
        start: record.start,
        end: record.end,
        tenant_id: record.tenant_id,
        description: record.description,
        comments: record.comments,
        owner_id: record.owner_id,
        owner_name: record.owner_name,
    }
}

fn map_bulk_asn_range_result(result: BulkAsnRangeResult) -> BulkAsnRangeResponse {
    BulkAsnRangeResponse {
        created: result.created.into_iter().map(map_asn_range).collect(),
        errors: result.errors,
    }
}

fn resolve_slug(slug: Option<String>, name: &str) -> String {
    slug.map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| derive_slug(name))
}

/// List ASN ranges.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/asn-ranges",
    tag = "inventory",
    params(AsnRangeListQuery),
    responses(
        (status = 200, description = "ASN ranges", body = InventoryListEnvelope<AsnRangeResponse>),
        (status = "4XX", description = "ASN range list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "ASN range list failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_asn_ranges(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<AsnRangeListQuery>,
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

    let rir_ids = match parse_csv_uuids(query.rir_id.as_deref(), "rir_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let tenant_ids = match parse_csv_uuids(query.tenant_id.as_deref(), "tenant_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let owner_ids = match parse_csv_uuids(query.owner_id.as_deref(), "owner_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let resolved_limit = clamp_limit(query.limit);
    let resolved_offset = clamp_offset(query.offset);
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        ASN_RANGE_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .asn_ranges
        .list(AsnRangeListParams {
            rir_ids: &rir_ids,
            tenant_ids: &tenant_ids,
            owner_ids: &owner_ids,
            search: crate::contexts::shared::list_query::search_pattern(query.search.as_deref()),
            limit: query.limit,
            offset: query.offset,
            sort,
        })
        .await
    {
        Ok((records, total)) => {
            let items: Vec<AsnRangeResponse> = records.into_iter().map(map_asn_range).collect();
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

/// Get an ASN range.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/asn-ranges/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "ASN range id"),
    ),
    responses(
        (status = 200, description = "ASN range", body = AsnRangeResponse),
        (status = 404, description = "ASN range not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "ASN range get request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "ASN range get failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_asn_range(
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
    match state.repositories.asn_ranges.get_by_id(id).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_asn_range(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("ASN range not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create an ASN range.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/asn-ranges",
    tag = "inventory",
    request_body = AsnRangeCreatePayload,
    responses(
        (status = 201, description = "ASN range created", body = AsnRangeResponse),
        (status = "4XX", description = "ASN range create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "ASN range create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_asn_range(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<AsnRangeCreatePayload>,
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

    let new_asn_range = NewAsnRange {
        id: Uuid::now_v7(),
        name,
        slug,
        rir_id: payload.rir_id,
        start: payload.start,
        end: payload.end,
        tenant_id: payload.tenant_id,
        description: payload.description,
        comments: payload.comments,
        owner_id: None,
    };

    match state.repositories.asn_ranges.create(&new_asn_range).await {
        Ok(record) => (StatusCode::CREATED, Json(map_asn_range(record))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update an ASN range.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/asn-ranges/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "ASN range id"),
    ),
    request_body = AsnRangeUpdatePayload,
    responses(
        (status = 200, description = "ASN range updated", body = AsnRangeResponse),
        (status = 404, description = "ASN range not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "ASN range update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "ASN range update failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_asn_range(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<AsnRangeUpdatePayload>,
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

    let changes = UpdateAsnRange {
        name: payload.name,
        slug: payload.slug,
        rir_id: payload.rir_id,
        start: payload.start,
        end: payload.end,
        tenant_id: payload.tenant_id,
        description: payload.description,
        comments: payload.comments,
    };

    match state.repositories.asn_ranges.update(id, &changes).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_asn_range(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("ASN range not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Delete an ASN range.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/asn-ranges/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "ASN range id"),
    ),
    responses(
        (status = 204, description = "ASN range deleted"),
        (status = "4XX", description = "ASN range delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "ASN range delete failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_asn_range(
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
    match state.repositories.asn_ranges.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create ASN ranges.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/asn-ranges/bulk",
    tag = "inventory",
    request_body = AsnRangeBulkCreatePayload,
    responses(
        (status = 200, description = "Bulk ASN range create result", body = BulkAsnRangeResponse),
        (status = "4XX", description = "Bulk ASN range create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Bulk ASN range create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_asn_ranges(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<AsnRangeBulkCreatePayload>,
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
        items.push(NewAsnRange {
            id: Uuid::now_v7(),
            name,
            slug,
            rir_id: p.rir_id,
            start: p.start,
            end: p.end,
            tenant_id: p.tenant_id,
            description: p.description,
            comments: p.comments,
            owner_id: None,
        });
    }

    match state.repositories.asn_ranges.bulk_create(&items).await {
        Ok(result) => (StatusCode::OK, Json(map_bulk_asn_range_result(result))).into_response(),
        Err(error) => handle_app_error(error),
    }
}
