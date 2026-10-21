//! VRF REST routes — full CRUD + bulk.
//!
//! Handlers never run SQL directly; all persistence goes through
//! `VrfRepository` on `AppState`. Import/export route targets are surfaced as
//! arrays of route-target ids and replaced atomically by the repository.

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
use crate::contexts::inventory::infrastructure::persistence::repositories::ipam::vrf_repository::VrfListParams;
use crate::contexts::inventory::infrastructure::persistence::{
    BulkCreateResult, NewVrf, UpdateVrf, VRF_COLUMNS, VrfListItem, VrfRecord, VrfRepository,
};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_strings, parse_csv_uuids};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

use super::super::{InventoryListEnvelope, ensure_access, parse_required_string, parse_uuid};

/// Query parameters for VRF listing.
#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct VrfListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub enforce_unique: Option<String>,
    pub tenant_id: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct VrfCreatePayload {
    pub name: String,
    pub rd: Option<String>,
    pub enforce_unique: Option<bool>,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub tenant_id: Option<Uuid>,
    pub import_targets: Option<Vec<Uuid>>,
    pub export_targets: Option<Vec<Uuid>>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct VrfUpdatePayload {
    pub name: Option<String>,
    pub rd: Option<String>,
    pub enforce_unique: Option<bool>,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub tenant_id: Option<Uuid>,
    pub import_targets: Option<Vec<Uuid>>,
    pub export_targets: Option<Vec<Uuid>>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct VrfBulkCreatePayload {
    #[serde(deserialize_with = "crate::api::rest::routes::validation::deserialize_bulk_items")]
    #[schema(min_items = 1)]
    pub items: Vec<VrfCreatePayload>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct VrfResponse {
    pub id: Uuid,
    pub external_id: Option<i64>,
    pub name: String,
    pub rd: Option<String>,
    pub enforce_unique: bool,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub tenant_id: Option<Uuid>,
    pub import_targets: Vec<Uuid>,
    pub export_targets: Vec<Uuid>,
    pub owner_id: Option<Uuid>,
    pub tags: Vec<String>,
}

/// Hydrated VRF list item — extends `VrfResponse` with the tenant name for the
/// collection list endpoint.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct VrfListItemResponse {
    pub id: Uuid,
    pub external_id: Option<i64>,
    pub name: String,
    pub rd: Option<String>,
    pub enforce_unique: bool,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub tenant_id: Option<Uuid>,
    pub tenant_name: Option<String>,
    pub import_targets: Vec<Uuid>,
    pub export_targets: Vec<Uuid>,
    pub owner_id: Option<Uuid>,
    pub tags: Vec<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkVrfResponse {
    pub created: Vec<VrfResponse>,
    pub errors: Vec<String>,
}

fn map_vrf_list_item(item: VrfListItem) -> VrfListItemResponse {
    let VrfListItem {
        record,
        tenant_name,
    } = item;
    VrfListItemResponse {
        id: record.id,
        external_id: record.external_id,
        name: record.name,
        rd: record.rd,
        enforce_unique: record.enforce_unique,
        description: record.description,
        comments: record.comments,
        tenant_id: record.tenant_id,
        tenant_name,
        import_targets: record.import_targets,
        export_targets: record.export_targets,
        owner_id: record.owner_id,
        tags: record.tags,
    }
}

fn map_vrf(record: VrfRecord) -> VrfResponse {
    VrfResponse {
        id: record.id,
        external_id: record.external_id,
        name: record.name,
        rd: record.rd,
        enforce_unique: record.enforce_unique,
        description: record.description,
        comments: record.comments,
        tenant_id: record.tenant_id,
        import_targets: record.import_targets,
        export_targets: record.export_targets,
        owner_id: record.owner_id,
        tags: record.tags,
    }
}

fn map_bulk_vrf_result(result: BulkCreateResult<VrfRecord>) -> BulkVrfResponse {
    BulkVrfResponse {
        created: result.created.into_iter().map(map_vrf).collect(),
        errors: result.errors,
    }
}

/// List VRFs.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/vrfs",
    tag = "inventory",
    params(VrfListQuery),
    responses(
        (status = 200, description = "VRFs", body = InventoryListEnvelope<VrfListItemResponse>),
        (status = "4XX", description = "VRF list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "VRF list failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_vrfs(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<VrfListQuery>,
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
        match SortSpec::parse_columns(query.sort.as_deref(), query.order.as_deref(), VRF_COLUMNS) {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };
    let tenant_ids = match parse_csv_uuids(query.tenant_id.as_deref(), "tenant_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let enforce_unique = parse_csv_strings(query.enforce_unique.as_deref());
    match state
        .repositories
        .vrfs
        .list(VrfListParams {
            limit: query.limit,
            offset: query.offset,
            enforce_unique: &enforce_unique,
            tenant_ids: &tenant_ids,
            search: crate::contexts::shared::list_query::search_pattern(query.search.as_deref()),
            sort,
        })
        .await
    {
        Ok((records, total)) => {
            let items: Vec<VrfListItemResponse> =
                records.into_iter().map(map_vrf_list_item).collect();
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

/// Get a VRF.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/vrfs/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "VRF id"),
    ),
    responses(
        (status = 200, description = "VRF", body = VrfResponse),
        (status = 404, description = "VRF not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "VRF get request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "VRF get failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_vrf(
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
    match state.repositories.vrfs.get_by_id(id).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_vrf(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("VRF not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a VRF.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/vrfs",
    tag = "inventory",
    request_body = VrfCreatePayload,
    responses(
        (status = 201, description = "VRF created", body = VrfResponse),
        (status = "4XX", description = "VRF create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "VRF create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_vrf(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<VrfCreatePayload>,
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

    let new_vrf = NewVrf {
        id: Uuid::now_v7(),
        name,
        rd: payload.rd,
        enforce_unique: payload.enforce_unique.unwrap_or(false),
        description: payload.description,
        comments: payload.comments,
        tenant_id: payload.tenant_id,
        import_targets: payload.import_targets.unwrap_or_default(),
        export_targets: payload.export_targets.unwrap_or_default(),
        owner_id: None,
    };

    match state.repositories.vrfs.create(&new_vrf).await {
        Ok(record) => (StatusCode::CREATED, Json(map_vrf(record))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update a VRF.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/vrfs/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "VRF id"),
    ),
    request_body = VrfUpdatePayload,
    responses(
        (status = 200, description = "VRF updated", body = VrfResponse),
        (status = 404, description = "VRF not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "VRF update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "VRF update failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_vrf(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<VrfUpdatePayload>,
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

    let changes = UpdateVrf {
        name: payload.name,
        rd: payload.rd,
        enforce_unique: payload.enforce_unique,
        description: payload.description,
        comments: payload.comments,
        tenant_id: payload.tenant_id,
        import_targets: payload.import_targets,
        export_targets: payload.export_targets,
    };

    match state.repositories.vrfs.update(id, &changes).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_vrf(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("VRF not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a VRF.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/vrfs/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "VRF id"),
    ),
    responses(
        (status = 204, description = "VRF deleted"),
        (status = "4XX", description = "VRF delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "VRF delete failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_vrf(
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
    match state.repositories.vrfs.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create VRFs.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/vrfs/bulk",
    tag = "inventory",
    request_body = VrfBulkCreatePayload,
    responses(
        (status = 200, description = "Bulk VRF create result", body = BulkVrfResponse),
        (status = "4XX", description = "Bulk VRF create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Bulk VRF create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_vrfs(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<VrfBulkCreatePayload>,
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
        items.push(NewVrf {
            id: Uuid::now_v7(),
            name,
            rd: p.rd,
            enforce_unique: p.enforce_unique.unwrap_or(false),
            description: p.description,
            comments: p.comments,
            tenant_id: p.tenant_id,
            import_targets: p.import_targets.unwrap_or_default(),
            export_targets: p.export_targets.unwrap_or_default(),
            owner_id: None,
        });
    }

    match state.repositories.vrfs.bulk_create(&items).await {
        Ok(result) => (StatusCode::OK, Json(map_bulk_vrf_result(result))).into_response(),
        Err(error) => handle_app_error(error),
    }
}
