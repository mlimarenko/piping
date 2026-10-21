//! ASN REST routes — full CRUD + bulk.
//!
//! Handlers never run SQL directly; all persistence goes through
//! `AsnRepository` on `AppState`.

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
use crate::contexts::inventory::infrastructure::persistence::repositories::ipam::asn_repository::AsnListParams;
use crate::contexts::inventory::infrastructure::persistence::{
    ASN_COLUMNS, AsnListItem, AsnRecord, AsnRepository, BulkCreateResult, NewAsn, UpdateAsn,
};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_uuids};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

use super::super::{InventoryListEnvelope, ensure_access, parse_uuid};

/// Query parameters for ASN listing.
#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct AsnListQuery {
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
pub struct AsnCreatePayload {
    pub asn: i64,
    pub rir_id: Uuid,
    pub tenant_id: Option<Uuid>,
    pub description: Option<String>,
    pub comments: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct AsnUpdatePayload {
    pub asn: Option<i64>,
    pub rir_id: Option<Uuid>,
    /// Pass `null` explicitly to clear the tenant association.
    pub tenant_id: Option<Option<Uuid>>,
    pub description: Option<String>,
    pub comments: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct AsnBulkCreatePayload {
    #[serde(deserialize_with = "crate::api::rest::routes::validation::deserialize_bulk_items")]
    #[schema(min_items = 1)]
    pub items: Vec<AsnCreatePayload>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AsnResponse {
    pub id: Uuid,
    pub external_id: Option<i64>,
    pub asn: i64,
    pub rir_id: Uuid,
    pub tenant_id: Option<Uuid>,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub owner_id: Option<Uuid>,
    pub tags: Vec<String>,
}

/// Hydrated ASN list item — extends `AsnResponse` with rir/tenant names for the
/// collection list endpoint.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AsnListItemResponse {
    pub id: Uuid,
    pub external_id: Option<i64>,
    pub asn: i64,
    pub rir_id: Uuid,
    pub rir_name: Option<String>,
    pub tenant_id: Option<Uuid>,
    pub tenant_name: Option<String>,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub owner_id: Option<Uuid>,
    pub tags: Vec<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkAsnResponse {
    pub created: Vec<AsnResponse>,
    pub errors: Vec<String>,
}

fn map_asn_list_item(item: AsnListItem) -> AsnListItemResponse {
    let AsnListItem {
        record,
        rir_name,
        tenant_name,
    } = item;
    AsnListItemResponse {
        id: record.id,
        external_id: record.external_id,
        asn: record.asn,
        rir_id: record.rir_id,
        rir_name,
        tenant_id: record.tenant_id,
        tenant_name,
        description: record.description,
        comments: record.comments,
        owner_id: record.owner_id,
        tags: record.tags,
    }
}

fn map_asn(record: AsnRecord) -> AsnResponse {
    AsnResponse {
        id: record.id,
        external_id: record.external_id,
        asn: record.asn,
        rir_id: record.rir_id,
        tenant_id: record.tenant_id,
        description: record.description,
        comments: record.comments,
        owner_id: record.owner_id,
        tags: record.tags,
    }
}

fn map_bulk_asn_result(result: BulkCreateResult<AsnRecord>) -> BulkAsnResponse {
    BulkAsnResponse {
        created: result.created.into_iter().map(map_asn).collect(),
        errors: result.errors,
    }
}

/// List ASNs.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/asns",
    tag = "inventory",
    params(AsnListQuery),
    responses(
        (status = 200, description = "ASNs", body = InventoryListEnvelope<AsnListItemResponse>),
        (status = "4XX", description = "ASN list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "ASN list failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_asns(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<AsnListQuery>,
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
        match SortSpec::parse_columns(query.sort.as_deref(), query.order.as_deref(), ASN_COLUMNS) {
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
        .asns
        .list(AsnListParams {
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
            let items: Vec<AsnListItemResponse> =
                records.into_iter().map(map_asn_list_item).collect();
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

/// Get an ASN.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/asns/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "ASN id"),
    ),
    responses(
        (status = 200, description = "ASN", body = AsnResponse),
        (status = 404, description = "ASN not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "ASN get request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "ASN get failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_asn(
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
    match state.repositories.asns.get_by_id(id).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_asn(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("ASN not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create an ASN.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/asns",
    tag = "inventory",
    request_body = AsnCreatePayload,
    responses(
        (status = 201, description = "ASN created", body = AsnResponse),
        (status = "4XX", description = "ASN create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "ASN create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_asn(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<AsnCreatePayload>,
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

    let new_asn = NewAsn {
        id: Uuid::now_v7(),
        asn: payload.asn,
        rir_id: payload.rir_id,
        tenant_id: payload.tenant_id,
        description: payload.description,
        comments: payload.comments,
        owner_id: None,
    };

    match state.repositories.asns.create(&new_asn).await {
        Ok(record) => (StatusCode::CREATED, Json(map_asn(record))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update an ASN.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/asns/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "ASN id"),
    ),
    request_body = AsnUpdatePayload,
    responses(
        (status = 200, description = "ASN updated", body = AsnResponse),
        (status = 404, description = "ASN not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "ASN update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "ASN update failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_asn(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<AsnUpdatePayload>,
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

    let changes = UpdateAsn {
        asn: payload.asn,
        rir_id: payload.rir_id,
        tenant_id: payload.tenant_id,
        description: payload.description,
        comments: payload.comments,
    };

    match state.repositories.asns.update(id, &changes).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_asn(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("ASN not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Delete an ASN.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/asns/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "ASN id"),
    ),
    responses(
        (status = 204, description = "ASN deleted"),
        (status = "4XX", description = "ASN delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "ASN delete failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_asn(
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
    match state.repositories.asns.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create ASNs.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/asns/bulk",
    tag = "inventory",
    request_body = AsnBulkCreatePayload,
    responses(
        (status = 200, description = "Bulk ASN create result", body = BulkAsnResponse),
        (status = "4XX", description = "Bulk ASN create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Bulk ASN create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_asns(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<AsnBulkCreatePayload>,
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

    let items: Vec<NewAsn> = payload
        .items
        .into_iter()
        .map(|p| NewAsn {
            id: Uuid::now_v7(),
            asn: p.asn,
            rir_id: p.rir_id,
            tenant_id: p.tenant_id,
            description: p.description,
            comments: p.comments,
            owner_id: None,
        })
        .collect();

    match state.repositories.asns.bulk_create(&items).await {
        Ok(result) => (StatusCode::OK, Json(map_bulk_asn_result(result))).into_response(),
        Err(error) => handle_app_error(error),
    }
}
