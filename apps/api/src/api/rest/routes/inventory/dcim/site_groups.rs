use std::collections::HashSet;

use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use uuid::Uuid;

use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::domain::SiteGroup;
use crate::contexts::inventory::infrastructure::persistence::repositories::dcim::site_group_repository::SITE_GROUP_COLUMNS;
use crate::contexts::shared::list_query::{SortSpec, parse_csv_uuids};
use crate::errors::AppError;

use super::super::{
    BulkCreateRequest, BulkCreateResponse, InventoryListEnvelope, SiteGroupPayload,
    bump_host_config_cache_generation, ensure_access, ensure_create_inventory_access,
    normalize_optional_string, parse_optional_uuid, parse_required_string, parse_uuid,
    push_bulk_error, require_bulk_items,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};

async fn validate_site_group_parent(
    state: &AppState,
    group_id: Uuid,
    parent_id: Option<Uuid>,
) -> Result<(), AppError> {
    let Some(parent_id) = parent_id else {
        return Ok(());
    };
    if parent_id == group_id {
        return Err(AppError::Validation(
            "parent_id cannot reference the same site group".to_string(),
        ));
    }

    let service = state.services.inventory_admin.clone();
    let mut seen = HashSet::new();
    let mut cursor = Some(parent_id);
    while let Some(id) = cursor {
        if id == group_id {
            return Err(AppError::Validation(
                "parent_id cannot reference a descendant site group".to_string(),
            ));
        }
        if !seen.insert(id) {
            return Err(AppError::Validation(
                "site group parent hierarchy already contains a cycle".to_string(),
            ));
        }
        if seen.len() > 200 {
            return Err(AppError::Validation(
                "site group parent hierarchy is too deep".to_string(),
            ));
        }

        let Some(group) = service.get_site_group(id).await? else {
            return Err(AppError::Validation(
                "parent_id does not reference an existing site group".to_string(),
            ));
        };
        cursor = group.parent_id;
    }

    Ok(())
}

#[derive(Debug, serde::Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct SiteGroupListQuery {
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub search: Option<String>,
    pub parent_id: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

/// List site groups.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/site_groups",
    tag = "inventory",
    params(SiteGroupListQuery),
    responses(
        (status = 200, description = "Site groups", body = InventoryListEnvelope<SiteGroup>),
        (status = "4XX", description = "Site group list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Site group list failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_site_groups(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<SiteGroupListQuery>,
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
    let search = query
        .search
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| format!("%{s}%"));
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        SITE_GROUP_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let parent_id = match parse_csv_uuids(query.parent_id.as_deref(), "parent_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.inventory_admin.clone();
    match service
        .list_site_groups(
            crate::contexts::inventory::application::InventoryListQuery::new(
                query.limit,
                query.offset,
                search,
            )
            .with_parent_id(parent_id)
            .with_sort(sort),
        )
        .await
    {
        Ok((items, total)) => (
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

/// Get a site group.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/site_groups/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Site group id"),
    ),
    responses(
        (status = 200, description = "Site group", body = SiteGroup),
        (status = 404, description = "Site group not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Site group get request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Site group get failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_site_group(
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
    let group_id = match parse_uuid(&id, "site_group_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.inventory_admin.clone();
    match service.get_site_group(group_id).await {
        Ok(Some(group)) => (StatusCode::OK, Json(group)).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Site group not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a site group.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/site_groups",
    tag = "inventory",
    request_body = SiteGroupPayload,
    responses(
        (status = 201, description = "Site group created", body = SiteGroup),
        (status = "4XX", description = "Site group create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Site group create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_site_group(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<SiteGroupPayload>,
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
    let slug = match parse_required_string(&payload.slug, "slug") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let parent_id = match parse_optional_uuid(payload.parent_id.clone(), "parent_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let group_id = Uuid::now_v7();
    if let Err(error) = validate_site_group_parent(&state, group_id, parent_id).await {
        return handle_app_error(error);
    }
    let description = normalize_optional_string(payload.description);
    let group = SiteGroup {
        id: group_id,
        external_id: None,
        name,
        slug,
        parent_id,
        description,
        piping_synced_at: None,
        ..Default::default()
    };
    let service = state.services.inventory_admin.clone();
    match service.create_site_group(group.clone()).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            (StatusCode::CREATED, Json(group)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create site groups.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/site_groups/bulk",
    tag = "inventory",
    request_body = BulkCreateRequest<SiteGroupPayload>,
    responses(
        (status = 200, description = "Bulk site group create result", body = BulkCreateResponse<SiteGroup>),
        (status = "4XX", description = "Bulk site group create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Bulk site group create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_site_groups(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(request): EnvelopeJson<BulkCreateRequest<SiteGroupPayload>>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_create_inventory_access(&state, &request_context).await {
        return handle_app_error(error);
    }

    let items = match require_bulk_items(request.items) {
        Ok(items) => items,
        Err(error) => return handle_app_error(error),
    };

    let service = state.services.inventory_admin.clone();
    let mut created = Vec::new();
    let mut errors = Vec::new();

    for (index, payload) in items.into_iter().enumerate() {
        let name = match parse_required_string(&payload.name, "name") {
            Ok(value) => value,
            Err(error) => {
                push_bulk_error(&mut errors, index, error);
                continue;
            }
        };
        let slug = match parse_required_string(&payload.slug, "slug") {
            Ok(value) => value,
            Err(error) => {
                push_bulk_error(&mut errors, index, error);
                continue;
            }
        };
        let parent_id = match parse_optional_uuid(payload.parent_id.clone(), "parent_id") {
            Ok(value) => value,
            Err(error) => {
                push_bulk_error(&mut errors, index, error);
                continue;
            }
        };
        let group_id = Uuid::now_v7();
        if let Err(error) = validate_site_group_parent(&state, group_id, parent_id).await {
            push_bulk_error(&mut errors, index, error);
            continue;
        }
        let description = normalize_optional_string(payload.description);
        let group = SiteGroup {
            id: group_id,
            external_id: None,
            name,
            slug,
            parent_id,
            description,
            piping_synced_at: None,
            ..Default::default()
        };

        match service.create_site_group(group.clone()).await {
            Ok(()) => created.push(group),
            Err(error) => push_bulk_error(&mut errors, index, error),
        }
    }

    if !created.is_empty() {
        bump_host_config_cache_generation(&state).await;
    }

    (StatusCode::OK, Json(BulkCreateResponse { created, errors })).into_response()
}

/// Update a site group.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/site_groups/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Site group id"),
    ),
    request_body = SiteGroupPayload,
    responses(
        (status = 200, description = "Site group updated", body = SiteGroup),
        (status = 404, description = "Site group not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Site group update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Site group update failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_site_group(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<SiteGroupPayload>,
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
    let group_id = match parse_uuid(&id, "site_group_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let name = match parse_required_string(&payload.name, "name") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let slug = match parse_required_string(&payload.slug, "slug") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let parent_id = match parse_optional_uuid(payload.parent_id.clone(), "parent_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = validate_site_group_parent(&state, group_id, parent_id).await {
        return handle_app_error(error);
    }
    let description = normalize_optional_string(payload.description);

    let service = state.services.inventory_admin.clone();
    match service.get_site_group(group_id).await {
        Ok(None) => {
            return handle_app_error(AppError::NotFound("Site group not found".to_string()));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };

    let group = SiteGroup {
        id: group_id,
        external_id: None,
        name,
        slug,
        parent_id,
        description,
        piping_synced_at: None,
        ..Default::default()
    };

    match service.update_site_group(group.clone()).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            (StatusCode::OK, Json(group)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Delete a site group.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/site_groups/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Site group id"),
    ),
    responses(
        (status = 204, description = "Site group deleted"),
        (status = 404, description = "Site group not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Site group delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Site group delete failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_site_group(
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
    let group_id = match parse_uuid(&id, "site_group_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.inventory_admin.clone();
    match service.get_site_group(group_id).await {
        Ok(None) => {
            return handle_app_error(AppError::NotFound("Site group not found".to_string()));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };
    match service.delete_site_group(group_id).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => handle_app_error(error),
    }
}
