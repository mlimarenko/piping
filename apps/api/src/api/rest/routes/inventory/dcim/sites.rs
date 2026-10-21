use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::Deserialize;
use std::collections::BTreeMap;
use uuid::Uuid;

use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::domain::Site;
use crate::errors::AppError;

use super::super::{
    BulkCreateRequest, BulkCreateResponse, InventoryListEnvelope, SiteListRowResponse, SitePayload,
    apply_custom_field_secret_intents, apply_custom_fields_write, build_custom_field_list_filter,
    bump_host_config_cache_generation, enqueue_site_outbound_event, ensure_access,
    ensure_create_inventory_access, masked_custom_fields, masked_custom_fields_page,
    parse_optional_uuid, parse_required_string, parse_uuid, push_bulk_error, require_bulk_items,
    resolve_site_slug, validate_and_split_custom_fields,
};
use crate::api::rest::routes::custom_fields::CONTENT_TYPE_SITE;
use crate::contexts::inventory::infrastructure::persistence::{
    SITE_COLUMNS, SiteFilter, SortSpec, parse_csv_strings, parse_csv_uuids, parse_id_filter,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};

/// Query parameters for site listing — pagination, free-text search, optional
/// parent-FK filters, multi-value status, and allowlisted sorting.
///
/// FK and `status` filters accept comma-separated values (`?region_id=a,b`) and
/// combine with AND across fields, `IN` within a field. `sort` is a stable
/// logical column key (e.g. `name`, `slug`, `status`, `tenant`, `region`,
/// `site_group`, `facility`); `order` is `asc` (default) or `desc`. An unknown
/// sort key or invalid order is rejected with 400.
#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct SiteListQuery {
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    /// Comma-separated resource UUIDs (maximum 100). An empty value matches no rows.
    pub ids: Option<String>,
    /// Case-insensitive substring search on site name.
    pub search: Option<String>,
    /// Comma-separated region ids; matches any (`IN`).
    pub region_id: Option<String>,
    /// Comma-separated tenant ids; matches any (`IN`).
    pub tenant_id: Option<String>,
    /// Comma-separated site-group ids; matches any (`IN`).
    pub site_group_id: Option<String>,
    /// Comma-separated site statuses; matches any (`IN`).
    pub status: Option<String>,
    /// Logical sort column key (allowlisted; unknown keys are rejected).
    pub sort: Option<String>,
    /// Sort direction: `asc` (default) or `desc`.
    pub order: Option<String>,
    #[serde(flatten)]
    #[param(ignore)]
    pub custom_field_filters: BTreeMap<String, String>,
}

/// List sites.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/sites",
    tag = "inventory",
    params(SiteListQuery),
    responses(
        (status = 200, description = "Sites listed", body = InventoryListEnvelope<SiteListRowResponse>),
        (status = "4XX", description = "Site list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Site list request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_sites(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<SiteListQuery>,
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
    let ids = match parse_id_filter(query.ids.as_deref()) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let region_id = match parse_csv_uuids(query.region_id.as_deref(), "region_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let tenant_id = match parse_csv_uuids(query.tenant_id.as_deref(), "tenant_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let site_group_id = match parse_csv_uuids(query.site_group_id.as_deref(), "site_group_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let status = parse_csv_strings(query.status.as_deref());
    let custom_fields = match build_custom_field_list_filter(
        &state,
        CONTENT_TYPE_SITE,
        &query.custom_field_filters,
    )
    .await
    {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        SITE_COLUMNS,
    ) {
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
    let filter = SiteFilter {
        ids,
        region_id,
        tenant_id,
        site_group_id,
        status,
        custom_fields,
        sort,
    };
    let service = state.services.inventory_admin.clone();
    match service
        .list_sites_hydrated(
            crate::contexts::inventory::application::InventorySiteListQuery::new(
                filter,
                query.limit,
                query.offset,
            )
            .with_search(search),
        )
        .await
    {
        Ok((rows, total)) => {
            // One masking pass for the whole page (3 queries), never per row.
            let cf_entities: Vec<(Uuid, serde_json::Value)> = rows
                .iter()
                .map(|r| {
                    (
                        r.id,
                        r.custom_fields.clone().unwrap_or(serde_json::Value::Null),
                    )
                })
                .collect();
            let masked_page =
                match masked_custom_fields_page(&state, CONTENT_TYPE_SITE, cf_entities).await {
                    Ok(value) => value,
                    Err(error) => return handle_app_error(error),
                };
            let mut items = Vec::with_capacity(rows.len());
            for (r, custom_fields) in rows.into_iter().zip(masked_page) {
                let custom_fields = Some(custom_fields);
                items.push(SiteListRowResponse {
                    id: r.id,
                    name: r.name,
                    slug: r.slug,
                    status: r.status,
                    tenant_id: r.tenant_id,
                    tenant_name: r.tenant_name,
                    region_id: r.region_id,
                    region_name: r.region_name,
                    site_group_id: r.site_group_id,
                    site_group_name: r.site_group_name,
                    facility: r.facility,
                    time_zone: r.time_zone,
                    physical_address: r.physical_address,
                    shipping_address: r.shipping_address,
                    latitude: r.latitude,
                    longitude: r.longitude,
                    custom_fields,
                });
            }
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

/// Get a site.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/sites/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Site id"),
    ),
    responses(
        (status = 200, description = "Site", body = Site),
        (status = 404, description = "Site not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Site request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Site request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_site(
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
    let site_id = match parse_uuid(&id, "site_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.inventory_admin.clone();
    match service.get_site(site_id).await {
        Ok(Some(mut site)) => {
            let raw_custom_fields = site
                .custom_fields
                .clone()
                .unwrap_or(serde_json::Value::Null);
            match masked_custom_fields(&state, CONTENT_TYPE_SITE, site.id, &raw_custom_fields).await
            {
                Ok(value) => site.custom_fields = Some(value),
                Err(error) => return handle_app_error(error),
            }
            (StatusCode::OK, Json(site)).into_response()
        }
        Ok(None) => handle_app_error(AppError::NotFound("Site not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a site.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/sites",
    tag = "inventory",
    request_body = SitePayload,
    responses(
        (status = 201, description = "Site created", body = Site),
        (status = "4XX", description = "Site create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Site create request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_site(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<SitePayload>,
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
    let tenant_id = match parse_optional_uuid(payload.tenant_id.clone(), "tenant_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let region_id = match parse_optional_uuid(payload.region_id.clone(), "region_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let site_group_id = match parse_optional_uuid(payload.site_group_id.clone(), "site_group_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let site_id = Uuid::now_v7();
    // Validate + split custom fields BEFORE persisting the row; secret intents
    // are applied only after the row exists so a failed create cannot orphan a
    // secret binding.
    let custom_field_outcome = match validate_and_split_custom_fields(
        &state,
        CONTENT_TYPE_SITE,
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
    let mut site = Site {
        id: site_id,
        external_id: None,
        name,
        slug: payload.slug,
        status: payload.status,
        tenant_id,
        region_id,
        site_group_id,
        facility: payload.facility,
        time_zone: payload.time_zone,
        physical_address: payload.physical_address,
        shipping_address: payload.shipping_address,
        latitude: payload.latitude,
        longitude: payload.longitude,
        description: payload.description,
        comments: payload.comments,
        custom_fields: custom_field_data,
        piping_synced_at: None,
        ..Default::default()
    };
    let service = state.services.inventory_admin.clone();
    match service.create_site(site.clone()).await {
        Ok(()) => {
            // Row is persisted — now safe to apply secret intents.
            if let Some(outcome) = custom_field_outcome
                && let Err(error) =
                    apply_custom_field_secret_intents(&state, site.id, outcome.secret_intents).await
            {
                return handle_app_error(error);
            }
            if let Err(error) =
                enqueue_site_outbound_event(&state, &request_id, "create", &site, None).await
            {
                return handle_app_error(error);
            }
            bump_host_config_cache_generation(&state).await;
            let raw_custom_fields = site
                .custom_fields
                .clone()
                .unwrap_or(serde_json::Value::Null);
            match masked_custom_fields(&state, CONTENT_TYPE_SITE, site.id, &raw_custom_fields).await
            {
                Ok(value) => site.custom_fields = Some(value),
                Err(error) => return handle_app_error(error),
            }
            (StatusCode::CREATED, Json(site)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Validate, persist, secret-bind, and enqueue the outbound event for one bulk
/// site payload. Any failure fails just this item.
async fn create_site_item(
    state: &AppState,
    request_id: &RequestId,
    payload: SitePayload,
) -> Result<Site, AppError> {
    let name = parse_required_string(&payload.name, "name")?;
    let tenant_id = parse_optional_uuid(payload.tenant_id.clone(), "tenant_id")?;
    let region_id = parse_optional_uuid(payload.region_id.clone(), "region_id")?;
    let site_group_id = parse_optional_uuid(payload.site_group_id.clone(), "site_group_id")?;
    let custom_field_outcome =
        validate_and_split_custom_fields(state, CONTENT_TYPE_SITE, payload.custom_fields, true)
            .await?;
    let custom_field_data = custom_field_outcome
        .as_ref()
        .map(|outcome| outcome.custom_field_data.clone());
    let site = Site {
        id: Uuid::now_v7(),
        external_id: None,
        name,
        slug: payload.slug,
        status: payload.status,
        tenant_id,
        region_id,
        site_group_id,
        facility: payload.facility,
        time_zone: payload.time_zone,
        physical_address: payload.physical_address,
        shipping_address: payload.shipping_address,
        latitude: payload.latitude,
        longitude: payload.longitude,
        description: payload.description,
        comments: payload.comments,
        custom_fields: custom_field_data,
        piping_synced_at: None,
        ..Default::default()
    };
    state
        .services
        .inventory_admin
        .create_site(site.clone())
        .await?;
    if let Some(outcome) = custom_field_outcome
        && let Err(error) =
            apply_custom_field_secret_intents(state, site.id, outcome.secret_intents).await
    {
        return Err(error);
    }
    enqueue_site_outbound_event(state, request_id, "create", &site, None).await?;
    Ok(site)
}

/// Bulk-create sites.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/sites/bulk",
    tag = "inventory",
    request_body = BulkCreateRequest<SitePayload>,
    responses(
        (status = 200, description = "Sites bulk-created", body = BulkCreateResponse<Site>),
        (status = "4XX", description = "Sites bulk-create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Sites bulk-create request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_sites(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(request): EnvelopeJson<BulkCreateRequest<SitePayload>>,
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
    let mut errors = Vec::new();

    for (index, payload) in items.into_iter().enumerate() {
        match create_site_item(&state, &request_id, payload).await {
            Ok(site) => created.push(site),
            Err(error) => push_bulk_error(&mut errors, index, error),
        }
    }

    if !created.is_empty() {
        bump_host_config_cache_generation(&state).await;
    }

    (StatusCode::OK, Json(BulkCreateResponse { created, errors })).into_response()
}

/// Update a site.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/sites/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Site id"),
    ),
    request_body = SitePayload,
    responses(
        (status = 200, description = "Site updated", body = Site),
        (status = 404, description = "Site not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Site update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Site update request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_site(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<SitePayload>,
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
    let site_id = match parse_uuid(&id, "site_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let name = match parse_required_string(&payload.name, "name") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let tenant_id = match parse_optional_uuid(payload.tenant_id.clone(), "tenant_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let region_id = match parse_optional_uuid(payload.region_id.clone(), "region_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let site_group_id = match parse_optional_uuid(payload.site_group_id.clone(), "site_group_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.inventory_admin.clone();
    let previous_site = match service.get_site(site_id).await {
        Ok(None) => return handle_app_error(AppError::NotFound("Site not found".to_string())),
        Ok(Some(site)) => site,
        Err(error) => return handle_app_error(error),
    };

    // An omitted `custom_fields` leaves the stored data untouched; a present
    // map is validated, split, and replaces it.
    let custom_field_data = match payload.custom_fields {
        Some(incoming) => {
            match apply_custom_fields_write(&state, CONTENT_TYPE_SITE, site_id, Some(incoming))
                .await
            {
                Ok(value) => value,
                Err(error) => return handle_app_error(error),
            }
        }
        None => previous_site.custom_fields.clone(),
    };

    let mut site = Site {
        id: site_id,
        external_id: None,
        name,
        slug: payload.slug,
        status: payload.status,
        tenant_id,
        region_id,
        site_group_id,
        facility: payload.facility,
        time_zone: payload.time_zone,
        physical_address: payload.physical_address,
        shipping_address: payload.shipping_address,
        latitude: payload.latitude,
        longitude: payload.longitude,
        description: payload.description,
        comments: payload.comments,
        custom_fields: custom_field_data,
        piping_synced_at: None,
        ..Default::default()
    };
    match service.update_site(site.clone()).await {
        Ok(()) => {
            if let Err(error) = enqueue_site_outbound_event(
                &state,
                &request_id,
                "update",
                &site,
                Some(resolve_site_slug(&previous_site)),
            )
            .await
            {
                return handle_app_error(error);
            }
            bump_host_config_cache_generation(&state).await;
            let raw_custom_fields = site
                .custom_fields
                .clone()
                .unwrap_or(serde_json::Value::Null);
            match masked_custom_fields(&state, CONTENT_TYPE_SITE, site.id, &raw_custom_fields).await
            {
                Ok(value) => site.custom_fields = Some(value),
                Err(error) => return handle_app_error(error),
            }
            (StatusCode::OK, Json(site)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Delete a site.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/sites/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Site id"),
    ),
    responses(
        (status = 204, description = "Site deleted"),
        (status = 404, description = "Site not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Site delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Site delete request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_site(
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
    let site_id = match parse_uuid(&id, "site_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.inventory_admin.clone();
    let site_to_delete = match service.get_site(site_id).await {
        Ok(None) => return handle_app_error(AppError::NotFound("Site not found".to_string())),
        Ok(Some(site)) => site,
        Err(error) => return handle_app_error(error),
    };

    match service.delete_site(site_id).await {
        Ok(()) => {
            if let Err(error) = enqueue_site_outbound_event(
                &state,
                &request_id,
                "delete",
                &site_to_delete,
                Some(resolve_site_slug(&site_to_delete)),
            )
            .await
            {
                return handle_app_error(error);
            }
            bump_host_config_cache_generation(&state).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => handle_app_error(error),
    }
}
