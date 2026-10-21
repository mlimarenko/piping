use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Deserializer};
use uuid::Uuid;

use super::super::{
    BulkCreateRequest, BulkCreateResponse, InventoryListEnvelope, ensure_access,
    ensure_create_inventory_access, parse_uuid, push_bulk_error, require_bulk_items,
};
use crate::api::middleware::{auth::AuthContext, error::handle_app_error, request_id::RequestId};
use crate::api::rest::{
    envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated},
    request_context::build_request_context,
};
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::{
    domain::Owner,
    infrastructure::persistence::{
        OWNER_COLUMNS, OwnerChanges, OwnerFilter, OwnerRepository, SortSpec, parse_csv_uuids,
    },
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;

pub(in crate::api::rest::routes::inventory) fn present<
    'de,
    D: Deserializer<'de>,
    T: Deserialize<'de>,
>(
    d: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}

fn nullable<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct OwnerPayload {
    #[serde(default, deserialize_with = "present")]
    #[schema(nullable = false, min_length = 1, max_length = 100)]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "present")]
    #[schema(nullable = false, max_length = 200)]
    pub description: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub group_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "present")]
    #[schema(nullable = false)]
    pub users: Option<Vec<Uuid>>,
    #[serde(default, deserialize_with = "present")]
    #[schema(nullable = false)]
    pub user_groups: Option<Vec<Uuid>>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct OwnerCreatePayload {
    #[schema(min_length = 1, max_length = 100)]
    pub name: String,
    #[serde(default)]
    #[schema(max_length = 200)]
    pub description: String,
    pub group_id: Option<Uuid>,
    #[serde(default)]
    pub users: Vec<Uuid>,
    #[serde(default)]
    pub user_groups: Vec<Uuid>,
}

impl From<OwnerCreatePayload> for OwnerPayload {
    fn from(p: OwnerCreatePayload) -> Self {
        Self {
            name: Some(p.name),
            description: Some(p.description),
            group_id: Some(p.group_id),
            users: Some(p.users),
            user_groups: Some(p.user_groups),
        }
    }
}

/// PUT requires a name while retaining omitted associations.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct OwnerPutPayload {
    #[schema(min_length = 1, max_length = 100)]
    pub name: String,
    #[serde(default, deserialize_with = "present")]
    #[schema(nullable = false, max_length = 200)]
    pub description: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub group_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "present")]
    #[schema(nullable = false)]
    pub users: Option<Vec<Uuid>>,
    #[serde(default, deserialize_with = "present")]
    #[schema(nullable = false)]
    pub user_groups: Option<Vec<Uuid>>,
}

impl From<OwnerPutPayload> for OwnerPayload {
    fn from(p: OwnerPutPayload) -> Self {
        Self {
            name: Some(p.name),
            description: p.description,
            group_id: p.group_id,
            users: p.users,
            user_groups: p.user_groups,
        }
    }
}

pub(super) fn validate_admin_fields(
    name: &Option<String>,
    description: &Option<String>,
    required: bool,
) -> Result<(), AppError> {
    if required && name.is_none() {
        return Err(AppError::Validation("name is required".into()));
    }
    if let Some(name) = name
        && (name.trim().is_empty() || name.chars().count() > 100)
    {
        return Err(AppError::Validation(
            "name must contain 1–100 characters".into(),
        ));
    }
    if description
        .as_ref()
        .is_some_and(|v| v.chars().count() > 200)
    {
        return Err(AppError::Validation(
            "description must contain at most 200 characters".into(),
        ));
    }
    Ok(())
}

impl OwnerPayload {
    fn create(self) -> Result<Owner, AppError> {
        validate_admin_fields(&self.name, &self.description, true)?;
        Ok(Owner {
            id: Uuid::now_v7(),
            name: self
                .name
                .ok_or_else(|| AppError::Validation("name is required".into()))?,
            description: self.description.unwrap_or_default(),
            group_id: self.group_id.flatten(),
            users: self.users.unwrap_or_default(),
            user_groups: self.user_groups.unwrap_or_default(),
        })
    }
    fn changes(self, required: bool) -> Result<OwnerChanges, AppError> {
        validate_admin_fields(&self.name, &self.description, required)?;
        Ok(OwnerChanges {
            name: self.name,
            description: self.description,
            group_id: self.group_id,
            users: self.users,
            user_groups: self.user_groups,
        })
    }
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct OwnerListQuery {
    pub search: Option<String>,
    pub group_id: Option<String>,
    pub user_id: Option<String>,
    pub user_group_id: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

/// List owners.
#[utoipa::path(get, path="/api/v1/inventory/owners", tag="inventory", params(OwnerListQuery), responses((status=200, description="Owners", body=InventoryListEnvelope<Owner>), (status="4XX", description="Rejected", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", description="Failed", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn list_owners(
    State(state): State<AppState>,
    Extension(rid): Extension<RequestId>,
    auth: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(q): Query<OwnerListQuery>,
) -> Response {
    let ctx = build_request_context(&rid, auth, &headers);
    if let Err(e) = ensure_access(&state, &ctx, Action::Read, ResourceType::Inventory).await {
        return handle_app_error(e);
    }
    let filter = (|| {
        Ok::<_, AppError>(OwnerFilter {
            search: q.search,
            group_ids: parse_csv_uuids(q.group_id.as_deref(), "group_id")?,
            user_ids: parse_csv_uuids(q.user_id.as_deref(), "user_id")?,
            user_group_ids: parse_csv_uuids(q.user_group_id.as_deref(), "user_group_id")?,
            limit: q.limit,
            offset: q.offset,
            sort: SortSpec::parse_columns(
                q.sort.as_deref().or(q.order.as_ref().map(|_| "name")),
                q.order.as_deref(),
                OWNER_COLUMNS,
            )?,
        })
    })();
    let filter = match filter {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };
    match state.repositories.owners.list(filter).await {
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

/// Get an owner.
#[utoipa::path(get, path="/api/v1/inventory/owners/{id}", tag="inventory", params(("id"=Uuid, Path)), responses((status=200, description="Owner", body=Owner), (status="4XX", description="Rejected", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", description="Failed", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn get_owner(
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
    owner_response(state.repositories.owners.get_by_id(id).await)
}

fn owner_response(result: Result<Option<Owner>, AppError>) -> Response {
    match result {
        Ok(Some(v)) => Json(v).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Owner not found".into())),
        Err(e) => handle_app_error(e),
    }
}

/// Create an owner.
#[utoipa::path(post, path="/api/v1/inventory/owners", tag="inventory", request_body=OwnerCreatePayload, responses((status=201, description="Created", body=Owner), (status="4XX", description="Rejected", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", description="Failed", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn create_owner(
    State(state): State<AppState>,
    Extension(rid): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<OwnerCreatePayload>,
) -> Response {
    let ctx = build_request_context(&rid, Some(auth), &headers);
    if let Err(e) = ensure_create_inventory_access(&state, &ctx).await {
        return handle_app_error(e);
    }
    let owner = match OwnerPayload::from(payload).create() {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };
    match state.repositories.owners.create(&owner).await {
        Ok(v) => (StatusCode::CREATED, Json(v)).into_response(),
        Err(e) => handle_app_error(e),
    }
}

/// Bulk-create owners.
#[utoipa::path(post, path="/api/v1/inventory/owners/bulk", tag="inventory", request_body=BulkCreateRequest<OwnerCreatePayload>, responses((status=200, description="Bulk result", body=BulkCreateResponse<Owner>), (status="4XX", description="Rejected", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", description="Failed", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn bulk_create_owners(
    State(state): State<AppState>,
    Extension(rid): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<BulkCreateRequest<OwnerCreatePayload>>,
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
        let result = match OwnerPayload::from(p).create() {
            Ok(v) => state.repositories.owners.create(&v).await,
            Err(e) => Err(e),
        };
        match result {
            Ok(v) => created.push(v),
            Err(e) => push_bulk_error(&mut errors, i, e),
        }
    }
    Json(BulkCreateResponse { created, errors }).into_response()
}

/// Update an owner.
#[utoipa::path(put, path="/api/v1/inventory/owners/{id}", tag="inventory", params(("id"=Uuid, Path)), request_body=OwnerPutPayload, responses((status=200, description="Updated", body=Owner), (status="4XX", description="Rejected", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", description="Failed", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn update_owner(
    State(state): State<AppState>,
    Extension(rid): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<OwnerPutPayload>,
) -> Response {
    let ctx = build_request_context(&rid, Some(auth), &headers);
    write_owner(state, ctx, id, payload.into(), true).await
}

/// Update an owner's fields.
#[utoipa::path(patch, path="/api/v1/inventory/owners/{id}", tag="inventory", params(("id"=Uuid, Path)), request_body=OwnerPayload, responses((status=200, description="Patched", body=Owner), (status="4XX", description="Rejected", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", description="Failed", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn patch_owner(
    State(state): State<AppState>,
    Extension(rid): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<OwnerPayload>,
) -> Response {
    let ctx = build_request_context(&rid, Some(auth), &headers);
    write_owner(state, ctx, id, payload, false).await
}

async fn write_owner(
    state: AppState,
    ctx: RequestContext,
    id: String,
    payload: OwnerPayload,
    required: bool,
) -> Response {
    if let Err(e) = ensure_access(&state, &ctx, Action::Update, ResourceType::Inventory).await {
        return handle_app_error(e);
    }
    let id = match parse_uuid(&id, "id") {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };
    let changes = match payload.changes(required) {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };
    owner_response(state.repositories.owners.update(id, &changes).await)
}

/// Delete an owner.
#[utoipa::path(delete, path="/api/v1/inventory/owners/{id}", tag="inventory", params(("id"=Uuid, Path)), responses((status=204, description="Deleted"), (status="4XX", description="Rejected", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", description="Failed", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn delete_owner(
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
    match state.repositories.owners.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => handle_app_error(e),
    }
}
