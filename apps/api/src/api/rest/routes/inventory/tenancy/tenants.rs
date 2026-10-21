use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::api::rest::routes::custom_fields::CONTENT_TYPE_TENANT;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::domain::Tenant;
use crate::contexts::inventory::infrastructure::persistence::{
    SortSpec, TENANT_COLUMNS, parse_csv_uuids,
};
use crate::errors::AppError;

use super::super::{
    BulkCreateRequest, BulkCreateResponse, BulkCreatedBatch, InventoryListEnvelope,
    TenantListRowResponse, TenantPayload, apply_custom_field_secret_intents,
    apply_custom_fields_write, bump_host_config_cache_generation, ensure_access,
    ensure_create_inventory_access, finalize_bulk_created, masked_custom_fields,
    parse_optional_uuid, parse_required_string, parse_uuid, push_bulk_error, require_bulk_items,
    validate_and_split_custom_fields,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};

/// Query parameters for tenant listing — pagination, free-text search, and
/// allowlisted sorting.
///
/// `sort` is a stable logical column key (`name`, `tenant_group`); `order` is
/// `asc` (default) or `desc`. An unknown sort key or invalid order is rejected
/// with 400.
#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct TenantListQuery {
    /// Comma-separated tenant group ids; matches any (`IN`).
    pub group_id: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    /// Case-insensitive substring search on tenant name.
    pub search: Option<String>,
    /// Logical sort column key (allowlisted; unknown keys are rejected).
    pub sort: Option<String>,
    /// Sort direction: `asc` (default) or `desc`.
    pub order: Option<String>,
}

/// List tenants.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/tenants",
    tag = "inventory",
    params(TenantListQuery),
    responses(
        (status = 200, description = "Tenants listed", body = InventoryListEnvelope<TenantListRowResponse>),
        (status = "4XX", description = "Tenant list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Tenant list request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_tenants(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<TenantListQuery>,
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
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        TENANT_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let group_id = match parse_csv_uuids(query.group_id.as_deref(), "group_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let resolved_limit = clamp_limit(query.limit);
    let resolved_offset = clamp_offset(query.offset);
    let search = query
        .search
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| format!("%{}%", s));
    let service = state.services.inventory_admin.clone();
    match service
        .list_tenants_hydrated(
            crate::contexts::inventory::application::InventoryListQuery::new(
                query.limit,
                query.offset,
                search,
            )
            .with_group_id(group_id)
            .with_sort(sort),
        )
        .await
    {
        Ok((rows, total)) => {
            let items = rows
                .into_iter()
                .map(|r| TenantListRowResponse {
                    id: r.id,
                    name: r.name,
                    slug: r.slug,
                    group_id: r.group_id,
                    group_name: r.group_name,
                })
                .collect();
            let envelope = InventoryListEnvelope {
                items,
                total,
                limit: resolved_limit,
                offset: resolved_offset,
            };
            (StatusCode::OK, Json(envelope)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Get a tenant.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/tenants/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Tenant id"),
    ),
    responses(
        (status = 200, description = "Tenant", body = Tenant),
        (status = 404, description = "Tenant not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Tenant request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Tenant request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_tenant(
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
    let tenant_id = match parse_uuid(&id, "tenant_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.inventory_admin.clone();
    match service.get_tenant(tenant_id).await {
        Ok(Some(mut tenant)) => {
            let raw_custom_fields = tenant
                .custom_fields
                .clone()
                .unwrap_or(serde_json::Value::Null);
            match masked_custom_fields(&state, CONTENT_TYPE_TENANT, tenant.id, &raw_custom_fields)
                .await
            {
                Ok(value) => tenant.custom_fields = Some(value),
                Err(error) => return handle_app_error(error),
            }
            (StatusCode::OK, Json(tenant)).into_response()
        }
        Ok(None) => handle_app_error(AppError::NotFound("Tenant not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a tenant.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/tenants",
    tag = "inventory",
    request_body = TenantPayload,
    responses(
        (status = 201, description = "Tenant created", body = Tenant),
        (status = "4XX", description = "Tenant create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Tenant create request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_tenant(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<TenantPayload>,
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
    let group_id = match parse_optional_uuid(payload.group_id.clone(), "group_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let tenant_id = Uuid::now_v7();
    let custom_field_outcome = match validate_and_split_custom_fields(
        &state,
        CONTENT_TYPE_TENANT,
        payload.custom_fields,
        true,
    )
    .await
    {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let custom_field_data = custom_field_outcome
        .as_ref()
        .map(|outcome| outcome.custom_field_data.clone());
    let mut tenant = Tenant {
        id: tenant_id,
        external_id: None,
        name,
        slug: payload.slug,
        group_id,
        description: payload.description,
        comments: payload.comments,
        custom_fields: custom_field_data,
        piping_synced_at: None,
        ..Default::default()
    };
    let service = state.services.inventory_admin.clone();
    match service.create_tenant(tenant.clone()).await {
        Ok(()) => {
            if let Some(outcome) = custom_field_outcome
                && let Err(error) =
                    apply_custom_field_secret_intents(&state, tenant.id, outcome.secret_intents)
                        .await
            {
                return handle_app_error(error);
            }
            bump_host_config_cache_generation(&state).await;
            let raw_custom_fields = tenant
                .custom_fields
                .clone()
                .unwrap_or(serde_json::Value::Null);
            match masked_custom_fields(&state, CONTENT_TYPE_TENANT, tenant.id, &raw_custom_fields)
                .await
            {
                Ok(value) => tenant.custom_fields = Some(value),
                Err(error) => return handle_app_error(error),
            }
            (StatusCode::CREATED, Json(tenant)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Validate, persist, and secret-bind one bulk tenant payload. Any failure
/// (parse, custom-field validation, create, or secret intent) fails just this
/// item.
async fn create_tenant_item(state: &AppState, payload: TenantPayload) -> Result<Tenant, AppError> {
    let name = parse_required_string(&payload.name, "name")?;
    let group_id = parse_optional_uuid(payload.group_id.clone(), "group_id")?;
    let custom_field_outcome =
        validate_and_split_custom_fields(state, CONTENT_TYPE_TENANT, payload.custom_fields, true)
            .await?;
    let custom_field_data = custom_field_outcome
        .as_ref()
        .map(|outcome| outcome.custom_field_data.clone());
    let tenant = Tenant {
        id: Uuid::now_v7(),
        external_id: None,
        name,
        slug: payload.slug,
        group_id,
        description: payload.description,
        comments: payload.comments,
        custom_fields: custom_field_data,
        piping_synced_at: None,
        ..Default::default()
    };
    state
        .services
        .inventory_admin
        .create_tenant(tenant.clone())
        .await?;
    if let Some(outcome) = custom_field_outcome
        && let Err(error) =
            apply_custom_field_secret_intents(state, tenant.id, outcome.secret_intents).await
    {
        return Err(error);
    }
    Ok(tenant)
}

/// Bulk-create tenants.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/tenants/bulk",
    tag = "inventory",
    request_body = BulkCreateRequest<TenantPayload>,
    responses(
        (status = 200, description = "Tenants bulk-created", body = BulkCreateResponse<Tenant>),
        (status = "4XX", description = "Tenants bulk-create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Tenants bulk-create request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_tenants(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(request): EnvelopeJson<BulkCreateRequest<TenantPayload>>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_create_inventory_access(&state, &request_context).await {
        return handle_app_error(error);
    }

    let items = match require_bulk_items(request.items) {
        Ok(items) => items,
        Err(error) => return handle_app_error(error),
    };

    let mut created = Vec::new();
    let mut created_indices: Vec<usize> = Vec::new();
    let mut errors = Vec::new();

    for (index, payload) in items.into_iter().enumerate() {
        match create_tenant_item(&state, payload).await {
            Ok(tenant) => {
                created.push(tenant);
                created_indices.push(index);
            }
            Err(error) => push_bulk_error(&mut errors, index, error),
        }
    }

    finalize_bulk_created(
        &state,
        CONTENT_TYPE_TENANT,
        BulkCreatedBatch {
            created: &mut created,
            created_indices: &created_indices,
            errors: &mut errors,
        },
        |tenant: &Tenant| tenant.id,
        |tenant: &Tenant| tenant.custom_fields.as_ref(),
        |tenant: &mut Tenant, value| tenant.custom_fields = Some(value),
    )
    .await;

    (StatusCode::OK, Json(BulkCreateResponse { created, errors })).into_response()
}

/// Update a tenant.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/tenants/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Tenant id"),
    ),
    request_body = TenantPayload,
    responses(
        (status = 200, description = "Tenant updated", body = Tenant),
        (status = 404, description = "Tenant not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Tenant update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Tenant update request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_tenant(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<TenantPayload>,
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
    let tenant_id = match parse_uuid(&id, "tenant_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let name = match parse_required_string(&payload.name, "name") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let group_id = match parse_optional_uuid(payload.group_id, "group_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.inventory_admin.clone();
    let existing = match service.get_tenant(tenant_id).await {
        Ok(None) => return handle_app_error(AppError::NotFound("Tenant not found".to_string())),
        Ok(Some(existing)) => existing,
        Err(error) => return handle_app_error(error),
    };
    let custom_fields = match payload.custom_fields {
        Some(incoming) => {
            match apply_custom_fields_write(&state, CONTENT_TYPE_TENANT, tenant_id, Some(incoming))
                .await
            {
                Ok(value) => value,
                Err(error) => return handle_app_error(error),
            }
        }
        None => existing.custom_fields,
    };

    let mut tenant = Tenant {
        id: tenant_id,
        external_id: existing.external_id,
        name,
        slug: payload.slug,
        group_id,
        description: payload.description,
        comments: payload.comments,
        custom_fields,
        piping_synced_at: existing.piping_synced_at,
        ..Default::default()
    };
    match service.update_tenant(tenant.clone()).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            let raw_custom_fields = tenant
                .custom_fields
                .clone()
                .unwrap_or(serde_json::Value::Null);
            match masked_custom_fields(&state, CONTENT_TYPE_TENANT, tenant.id, &raw_custom_fields)
                .await
            {
                Ok(value) => tenant.custom_fields = Some(value),
                Err(error) => return handle_app_error(error),
            }
            (StatusCode::OK, Json(tenant)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Delete a tenant.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/tenants/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Tenant id"),
    ),
    responses(
        (status = 204, description = "Tenant deleted"),
        (status = 404, description = "Tenant not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Tenant delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Tenant delete request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_tenant(
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
    let tenant_id = match parse_uuid(&id, "tenant_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.inventory_admin.clone();
    match service.get_tenant(tenant_id).await {
        Ok(None) => return handle_app_error(AppError::NotFound("Tenant not found".to_string())),
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };

    match service.delete_tenant(tenant_id).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => handle_app_error(error),
    }
}
