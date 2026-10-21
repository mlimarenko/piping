//! VLAN groups REST routes — full CRUD + bulk.
//!
//! Handlers never run SQL directly; all persistence goes through
//! `VlanGroupRepository` on `AppState`.

use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::domain::ObjectRef;
use crate::contexts::inventory::domain::content_type::VLAN_GROUP_SCOPE_OBJECT_TYPES;
use crate::contexts::inventory::infrastructure::persistence::repositories::derive_slug;
use crate::contexts::inventory::infrastructure::persistence::repositories::ipam::vlan_group_repository::{
    VLAN_GROUP_COLUMNS, VlanGroupFilter,
};
use crate::contexts::inventory::infrastructure::persistence::{
    BulkVlanGroupResult, NewVlanGroup, SqlxObjectRefResolver, UpdateVlanGroup, VlanGroupRecord,
    VlanGroupRepository,
};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_uuids};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

use super::super::{
    InventoryListEnvelope, ObjectRefInput, ensure_access, parse_required_string, parse_uuid,
    resolve_object_type,
};

/// Query parameters for VLAN group listing.
#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct VlanGroupListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    pub scope_id: Option<String>,
    pub scope_type_id: Option<String>,
    pub tenant_id: Option<String>,
    pub owner_id: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct VlanGroupCreatePayload {
    pub name: String,
    pub slug: Option<String>,
    /// Canonical polymorphic scope: `{ object_type: "app_label.model",
    /// object_id }`. `null`/absent leaves the group global. Allowed targets:
    /// region, sitegroup, site, location, rack, cluster, clustergroup.
    pub scope: Option<ObjectRefInput>,
    pub vid_ranges: Option<Vec<String>>,
    pub tenant_id: Option<Uuid>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct VlanGroupUpdatePayload {
    pub name: Option<String>,
    pub slug: Option<String>,
    /// When present, re-points the polymorphic scope.
    pub scope: Option<ObjectRefInput>,
    pub vid_ranges: Option<Vec<String>>,
    pub tenant_id: Option<Uuid>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct VlanGroupBulkCreatePayload {
    #[serde(deserialize_with = "crate::api::rest::routes::validation::deserialize_bulk_items")]
    #[schema(min_items = 1)]
    pub items: Vec<VlanGroupCreatePayload>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct VlanGroupResponse {
    pub id: Uuid,
    pub external_id: Option<i64>,
    pub name: String,
    pub slug: String,
    /// Materialized polymorphic scope, or `null` when the group is global.
    pub scope: Option<ObjectRef>,
    pub vid_ranges: Vec<String>,
    pub tenant_id: Option<Uuid>,
    pub description: Option<String>,
    pub owner_id: Option<Uuid>,
    pub tags: Vec<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkVlanGroupResponse {
    pub created: Vec<VlanGroupResponse>,
    pub errors: Vec<String>,
}

fn map_vlan_group(record: VlanGroupRecord, scope: Option<ObjectRef>) -> VlanGroupResponse {
    VlanGroupResponse {
        id: record.id,
        external_id: record.external_id,
        name: record.name,
        slug: record.slug,
        scope,
        vid_ranges: record.vid_ranges,
        tenant_id: record.tenant_id,
        description: record.description,
        owner_id: record.owner_id,
        tags: record.tags,
    }
}

/// Resolve a single record's scope pair into a response.
async fn map_one_vlan_group(
    resolver: &SqlxObjectRefResolver,
    record: VlanGroupRecord,
) -> Result<VlanGroupResponse, AppError> {
    let scope = match (record.scope_type_id, record.scope_id) {
        (Some(type_id), Some(object_id)) => Some(resolver.resolve_one(type_id, object_id).await?),
        _ => None,
    };
    Ok(map_vlan_group(record, scope))
}

/// Batch-resolve a page of records (one resolver round-trip, no N+1).
async fn map_vlan_groups(
    resolver: &SqlxObjectRefResolver,
    records: Vec<VlanGroupRecord>,
) -> Result<Vec<VlanGroupResponse>, AppError> {
    let pairs: Vec<(Uuid, Uuid)> = records
        .iter()
        .filter_map(|record| match (record.scope_type_id, record.scope_id) {
            (Some(type_id), Some(object_id)) => Some((type_id, object_id)),
            _ => None,
        })
        .collect();
    let refs = resolver.resolve(&pairs).await?;
    Ok(records
        .into_iter()
        .map(|record| {
            let scope = match (record.scope_type_id, record.scope_id) {
                (Some(type_id), Some(object_id)) => Some(
                    refs.get(&(type_id, object_id))
                        .cloned()
                        .unwrap_or(ObjectRef {
                            object_type: String::new(),
                            object_id,
                            label: None,
                        }),
                ),
                _ => None,
            };
            map_vlan_group(record, scope)
        })
        .collect())
}

async fn map_bulk_vlan_group_result(
    resolver: &SqlxObjectRefResolver,
    result: BulkVlanGroupResult,
) -> Result<BulkVlanGroupResponse, AppError> {
    Ok(BulkVlanGroupResponse {
        created: map_vlan_groups(resolver, result.created).await?,
        errors: result.errors,
    })
}

fn resolve_slug(slug: Option<String>, name: &str) -> String {
    slug.map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| derive_slug(name))
}

/// Resolve an optional polymorphic scope input into the `(scope_type_id,
/// scope_id)` pair stored on the record. `None` input leaves both `None`.
async fn resolve_scope(
    state: &AppState,
    scope: Option<&ObjectRefInput>,
) -> Result<(Option<Uuid>, Option<Uuid>), AppError> {
    match scope {
        Some(input) => {
            let type_id = resolve_object_type(
                &state.db_pool,
                &input.object_type,
                VLAN_GROUP_SCOPE_OBJECT_TYPES,
            )
            .await?;
            Ok((Some(type_id), Some(input.object_id)))
        }
        None => Ok((None, None)),
    }
}

/// List VLAN groups.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/vlan_groups",
    tag = "inventory",
    params(VlanGroupListQuery),
    responses(
        (status = 200, description = "VLAN groups", body = InventoryListEnvelope<VlanGroupResponse>),
        (status = "4XX", description = "VLAN group list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "VLAN group list failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_vlan_groups(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<VlanGroupListQuery>,
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

    let scope_id = match parse_csv_uuids(query.scope_id.as_deref(), "scope_id") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let scope_type_id = match parse_csv_uuids(query.scope_type_id.as_deref(), "scope_type_id") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let tenant_id = match parse_csv_uuids(query.tenant_id.as_deref(), "tenant_id") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let owner_id = match parse_csv_uuids(query.owner_id.as_deref(), "owner_id") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        VLAN_GROUP_COLUMNS,
    ) {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let filter = VlanGroupFilter {
        scope_id,
        scope_type_id,
        tenant_id,
        owner_id,
        search: crate::contexts::shared::list_query::search_pattern(query.search.as_deref()),
        sort,
    };

    let resolved_limit = clamp_limit(query.limit);
    let resolved_offset = clamp_offset(query.offset);
    match state
        .repositories
        .vlan_groups
        .list(filter, query.limit, query.offset)
        .await
    {
        Ok((records, total)) => {
            match map_vlan_groups(&state.repositories.object_ref_resolver, records).await {
                Ok(items) => (
                    StatusCode::OK,
                    Json(InventoryListEnvelope {
                        items,
                        total,
                        limit: resolved_limit,
                        offset: resolved_offset,
                    }),
                )
                    .into_response(),
                Err(error) => handle_app_error(error),
            }
        }
        Err(error) => handle_app_error(error),
    }
}

/// Get a VLAN group.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/vlan_groups/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "VLAN group id"),
    ),
    responses(
        (status = 200, description = "VLAN group", body = VlanGroupResponse),
        (status = 404, description = "VLAN group not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "VLAN group get request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "VLAN group get failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_vlan_group(
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
    match state.repositories.vlan_groups.get_by_id(id).await {
        Ok(Some(record)) => {
            match map_one_vlan_group(&state.repositories.object_ref_resolver, record).await {
                Ok(body) => (StatusCode::OK, Json(body)).into_response(),
                Err(error) => handle_app_error(error),
            }
        }
        Ok(None) => handle_app_error(AppError::NotFound("VLAN group not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a VLAN group.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/vlan_groups",
    tag = "inventory",
    request_body = VlanGroupCreatePayload,
    responses(
        (status = 201, description = "VLAN group created", body = VlanGroupResponse),
        (status = "4XX", description = "VLAN group create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "VLAN group create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_vlan_group(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<VlanGroupCreatePayload>,
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
    let (scope_type_id, scope_id) = match resolve_scope(&state, payload.scope.as_ref()).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let new_group = NewVlanGroup {
        id: Uuid::now_v7(),
        name,
        slug,
        scope_id,
        scope_type_id,
        vid_ranges: payload.vid_ranges.unwrap_or_default(),
        tenant_id: payload.tenant_id,
        description: payload.description,
        owner_id: None,
    };

    match state.repositories.vlan_groups.create(&new_group).await {
        Ok(record) => {
            match map_one_vlan_group(&state.repositories.object_ref_resolver, record).await {
                Ok(body) => (StatusCode::CREATED, Json(body)).into_response(),
                Err(error) => handle_app_error(error),
            }
        }
        Err(error) => handle_app_error(error),
    }
}

/// Update a VLAN group.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/vlan_groups/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "VLAN group id"),
    ),
    request_body = VlanGroupUpdatePayload,
    responses(
        (status = 200, description = "VLAN group updated", body = VlanGroupResponse),
        (status = 404, description = "VLAN group not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "VLAN group update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "VLAN group update failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_vlan_group(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<VlanGroupUpdatePayload>,
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

    let (scope_type_id, scope_id) = match resolve_scope(&state, payload.scope.as_ref()).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let changes = UpdateVlanGroup {
        name: payload.name,
        slug: payload.slug,
        scope_id,
        scope_type_id,
        vid_ranges: payload.vid_ranges,
        tenant_id: payload.tenant_id,
        description: payload.description,
    };

    match state.repositories.vlan_groups.update(id, &changes).await {
        Ok(Some(record)) => {
            match map_one_vlan_group(&state.repositories.object_ref_resolver, record).await {
                Ok(body) => (StatusCode::OK, Json(body)).into_response(),
                Err(error) => handle_app_error(error),
            }
        }
        Ok(None) => handle_app_error(AppError::NotFound("VLAN group not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a VLAN group.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/vlan_groups/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "VLAN group id"),
    ),
    responses(
        (status = 204, description = "VLAN group deleted"),
        (status = "4XX", description = "VLAN group delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "VLAN group delete failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_vlan_group(
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
    match state.repositories.vlan_groups.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create VLAN groups.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/vlan_groups/bulk",
    tag = "inventory",
    request_body = VlanGroupBulkCreatePayload,
    responses(
        (status = 200, description = "Bulk VLAN group create result", body = BulkVlanGroupResponse),
        (status = "4XX", description = "Bulk VLAN group create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Bulk VLAN group create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_vlan_groups(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<VlanGroupBulkCreatePayload>,
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
        let (scope_type_id, scope_id) = match resolve_scope(&state, p.scope.as_ref()).await {
            Ok(value) => value,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        items.push(NewVlanGroup {
            id: Uuid::now_v7(),
            name,
            slug,
            scope_id,
            scope_type_id,
            vid_ranges: p.vid_ranges.unwrap_or_default(),
            tenant_id: p.tenant_id,
            description: p.description,
            owner_id: None,
        });
    }

    match state.repositories.vlan_groups.bulk_create(&items).await {
        Ok(result) => {
            match map_bulk_vlan_group_result(&state.repositories.object_ref_resolver, result).await
            {
                Ok(body) => (StatusCode::OK, Json(body)).into_response(),
                Err(error) => handle_app_error(error),
            }
        }
        Err(error) => handle_app_error(error),
    }
}
