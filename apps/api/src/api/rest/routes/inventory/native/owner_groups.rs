use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use uuid::Uuid;

use super::super::{
    BulkCreateRequest, BulkCreateResponse, InventoryListEnvelope, ensure_access,
    ensure_create_inventory_access, parse_uuid, push_bulk_error, require_bulk_items,
};
use super::owners::{present, validate_admin_fields};
use crate::api::middleware::{auth::AuthContext, error::handle_app_error, request_id::RequestId};
use crate::api::rest::{
    envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated},
    request_context::build_request_context,
};
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::{
    domain::OwnerGroup,
    infrastructure::persistence::{OWNER_GROUP_COLUMNS, OwnerGroupRepository, SortSpec},
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;

#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct OwnerGroupPayload {
    #[serde(default, deserialize_with = "present")]
    #[schema(nullable = false, min_length = 1, max_length = 100)]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "present")]
    #[schema(nullable = false, max_length = 200)]
    pub description: Option<String>,
}
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct OwnerGroupCreatePayload {
    #[schema(min_length = 1, max_length = 100)]
    pub name: String,
    #[serde(default)]
    #[schema(max_length = 200)]
    pub description: String,
}
impl From<OwnerGroupCreatePayload> for OwnerGroupPayload {
    fn from(p: OwnerGroupCreatePayload) -> Self {
        Self {
            name: Some(p.name),
            description: Some(p.description),
        }
    }
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct OwnerGroupPutPayload {
    #[schema(min_length = 1, max_length = 100)]
    pub name: String,
    #[serde(default, deserialize_with = "present")]
    #[schema(nullable = false, max_length = 200)]
    pub description: Option<String>,
}

impl From<OwnerGroupPutPayload> for OwnerGroupPayload {
    fn from(p: OwnerGroupPutPayload) -> Self {
        Self {
            name: Some(p.name),
            description: p.description,
        }
    }
}
impl OwnerGroupPayload {
    fn create(self) -> Result<OwnerGroup, AppError> {
        validate_admin_fields(&self.name, &self.description, true)?;
        Ok(OwnerGroup {
            id: Uuid::now_v7(),
            name: self
                .name
                .ok_or_else(|| AppError::Validation("name is required".into()))?,
            description: self.description.unwrap_or_default(),
        })
    }
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct OwnerGroupListQuery {
    pub search: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

/// List owner groups.
#[utoipa::path(get, path="/api/v1/inventory/owner_groups", tag="inventory", params(OwnerGroupListQuery), responses((status=200, description="Owner groups", body=InventoryListEnvelope<OwnerGroup>), (status="4XX", description="Rejected", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", description="Failed", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn list_owner_groups(
    State(state): State<AppState>,
    Extension(rid): Extension<RequestId>,
    auth: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(q): Query<OwnerGroupListQuery>,
) -> Response {
    let ctx = build_request_context(&rid, auth, &headers);
    if let Err(e) = ensure_access(&state, &ctx, Action::Read, ResourceType::Inventory).await {
        return handle_app_error(e);
    }
    let sort = match SortSpec::parse_columns(
        q.sort.as_deref().or(q.order.as_ref().map(|_| "name")),
        q.order.as_deref(),
        OWNER_GROUP_COLUMNS,
    ) {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };
    match state
        .repositories
        .owner_groups
        .list(q.search, q.limit, q.offset, sort)
        .await
    {
        Ok((items, total)) => Json(InventoryListEnvelope {
            items,
            total,
            limit: clamp_limit(q.limit),
            offset: clamp_offset(q.offset),
        })
        .into_response(),
        Err(e) => handle_app_error(e),
    }
}

fn group_response(result: Result<Option<OwnerGroup>, AppError>) -> Response {
    match result {
        Ok(Some(v)) => Json(v).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Owner group not found".into())),
        Err(e) => handle_app_error(e),
    }
}

/// Get an owner group.
#[utoipa::path(get, path="/api/v1/inventory/owner_groups/{id}", tag="inventory", params(("id"=Uuid, Path)), responses((status=200, description="Owner group", body=OwnerGroup), (status="4XX", description="Rejected", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", description="Failed", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn get_owner_group(
    State(state): State<AppState>,
    Extension(rid): Extension<RequestId>,
    auth: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let ctx = build_request_context(&rid, auth, &headers);
    if let Err(e) = ensure_access(&state, &ctx, Action::Read, ResourceType::Inventory).await {
        return handle_app_error(e);
    }
    let id = match parse_uuid(&id, "id") {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };
    group_response(state.repositories.owner_groups.get_by_id(id).await)
}

/// Create an owner group.
#[utoipa::path(post, path="/api/v1/inventory/owner_groups", tag="inventory", request_body=OwnerGroupCreatePayload, responses((status=201, description="Created", body=OwnerGroup), (status="4XX", description="Rejected", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", description="Failed", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn create_owner_group(
    State(state): State<AppState>,
    Extension(rid): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<OwnerGroupCreatePayload>,
) -> Response {
    let ctx = build_request_context(&rid, Some(auth), &headers);
    if let Err(e) = ensure_create_inventory_access(&state, &ctx).await {
        return handle_app_error(e);
    }
    let group = match OwnerGroupPayload::from(payload).create() {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };
    match state.repositories.owner_groups.create(&group).await {
        Ok(v) => (StatusCode::CREATED, Json(v)).into_response(),
        Err(e) => handle_app_error(e),
    }
}

/// Bulk-create owner groups.
#[utoipa::path(post, path="/api/v1/inventory/owner_groups/bulk", tag="inventory", request_body=BulkCreateRequest<OwnerGroupCreatePayload>, responses((status=200, description="Bulk result", body=BulkCreateResponse<OwnerGroup>), (status="4XX", description="Rejected", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", description="Failed", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn bulk_create_owner_groups(
    State(state): State<AppState>,
    Extension(rid): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<BulkCreateRequest<OwnerGroupCreatePayload>>,
) -> Response {
    let ctx = build_request_context(&rid, Some(auth), &headers);
    if let Err(e) = ensure_create_inventory_access(&state, &ctx).await {
        return handle_app_error(e);
    }
    let items = match require_bulk_items(payload.items) {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };
    let mut created = Vec::new();
    let mut errors = Vec::new();
    for (i, p) in items.into_iter().enumerate() {
        let result = match OwnerGroupPayload::from(p).create() {
            Ok(v) => state.repositories.owner_groups.create(&v).await,
            Err(e) => Err(e),
        };
        match result {
            Ok(v) => created.push(v),
            Err(e) => push_bulk_error(&mut errors, i, e),
        }
    }
    Json(BulkCreateResponse { created, errors }).into_response()
}

/// Update an owner group.
#[utoipa::path(put, path="/api/v1/inventory/owner_groups/{id}", tag="inventory", params(("id"=Uuid, Path)), request_body=OwnerGroupPutPayload, responses((status=200, description="Updated", body=OwnerGroup), (status="4XX", description="Rejected", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", description="Failed", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn update_owner_group(
    State(state): State<AppState>,
    Extension(rid): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<OwnerGroupPutPayload>,
) -> Response {
    let ctx = build_request_context(&rid, Some(auth), &headers);
    write_group(state, ctx, id, payload.into(), true).await
}

/// Update an owner group's fields.
#[utoipa::path(patch, path="/api/v1/inventory/owner_groups/{id}", tag="inventory", params(("id"=Uuid, Path)), request_body=OwnerGroupPayload, responses((status=200, description="Patched", body=OwnerGroup), (status="4XX", description="Rejected", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", description="Failed", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn patch_owner_group(
    State(state): State<AppState>,
    Extension(rid): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<OwnerGroupPayload>,
) -> Response {
    let ctx = build_request_context(&rid, Some(auth), &headers);
    write_group(state, ctx, id, payload, false).await
}

async fn write_group(
    state: AppState,
    ctx: RequestContext,
    id: String,
    payload: OwnerGroupPayload,
    required: bool,
) -> Response {
    if let Err(e) = ensure_access(&state, &ctx, Action::Update, ResourceType::Inventory).await {
        return handle_app_error(e);
    }
    let id = match parse_uuid(&id, "id") {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };
    if let Err(e) = validate_admin_fields(&payload.name, &payload.description, required) {
        return handle_app_error(e);
    }
    group_response(
        state
            .repositories
            .owner_groups
            .update(id, payload.name.as_deref(), payload.description.as_deref())
            .await,
    )
}

/// Delete an owner group.
#[utoipa::path(delete, path="/api/v1/inventory/owner_groups/{id}", tag="inventory", params(("id"=Uuid, Path)), responses((status=204, description="Deleted"), (status="4XX", description="Rejected", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", description="Failed", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn delete_owner_group(
    State(state): State<AppState>,
    Extension(rid): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let ctx = build_request_context(&rid, Some(auth), &headers);
    if let Err(e) = ensure_access(&state, &ctx, Action::Delete, ResourceType::Inventory).await {
        return handle_app_error(e);
    }
    let id = match parse_uuid(&id, "id") {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };
    match state.repositories.owner_groups.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => handle_app_error(e),
    }
}
