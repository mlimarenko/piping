use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::Deserialize;
use std::collections::BTreeMap;
use uuid::Uuid;

use crate::api::connection_authz_attributes::{ConnectionTargetFacets, enrich_facets_with_region};
use crate::api::connection_target_visibility::{
    connection_target_row_facets, connection_target_row_visible,
    resolve_inventory_connection_target_visibility,
};
use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::domain::{Device, DeviceListView};
use crate::contexts::inventory::infrastructure::persistence::{
    DEVICE_COLUMNS, DeviceFilter, SortSpec, parse_csv_strings, parse_csv_uuids, parse_id_filter,
};
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;

use super::super::{
    BulkCreateRequest, BulkCreateResponse, DeviceListRowResponse, DevicePayload, DeviceResponse,
    InventoryListEnvelope, RenderedConfigContextResponse, apply_custom_field_secret_intents,
    apply_custom_fields_write, build_custom_field_list_filter, bump_host_config_cache_generation,
    ensure_access, ensure_create_inventory_access, map_device_response,
    map_rendered_config_context_response, masked_custom_fields, masked_custom_fields_page,
    normalize_optional_string, parse_optional_uuid, parse_required_string, parse_uuid,
    push_bulk_error, push_primary_ip_ids, require_bulk_items, resolve_primary_ip,
    validate_and_split_custom_fields,
};
use crate::api::rest::routes::custom_fields::CONTENT_TYPE_DEVICE;
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};

/// Query parameters for device listing — pagination, free-text search,
/// optional parent-FK filters, multi-value status, and allowlisted sorting.
///
/// FK and `status` filters accept comma-separated values (`?site_id=a,b`) and
/// combine with AND across fields, `IN` within a field. `sort` is a stable
/// logical column key (e.g. `name`, `status`, `site`, `role`, `device_type`,
/// `serial`, `tenant`); `order` is `asc` (default) or `desc`. An unknown sort
/// key or invalid order is rejected with 400.
#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct DeviceListQuery {
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    /// Comma-separated resource UUIDs (maximum 100). An empty value matches no rows.
    pub ids: Option<String>,
    /// Case-insensitive substring search on device name.
    pub search: Option<String>,
    /// Comma-separated tenant ids; matches any (`IN`).
    pub tenant_id: Option<String>,
    /// Comma-separated site ids; matches any (`IN`).
    pub site_id: Option<String>,
    /// Comma-separated device role ids; matches any (`IN`).
    pub role_id: Option<String>,
    /// Comma-separated device type ids; matches any (`IN`).
    pub device_type_id: Option<String>,
    /// Comma-separated platform ids; matches any (`IN`).
    pub platform_id: Option<String>,
    /// Comma-separated cluster ids; matches any (`IN`).
    pub cluster_id: Option<String>,
    /// Comma-separated rack ids; matches any (`IN`).
    pub rack_id: Option<String>,
    /// Comma-separated location ids; matches any (`IN`).
    pub location_id: Option<String>,
    /// Comma-separated device statuses; matches any (`IN`).
    pub status: Option<String>,
    /// Logical sort column key (allowlisted; unknown keys are rejected).
    pub sort: Option<String>,
    /// Sort direction: `asc` (default) or `desc`.
    pub order: Option<String>,
    #[serde(flatten)]
    #[param(ignore)]
    pub custom_field_filters: BTreeMap<String, String>,
}

/// Build the device list filter from the query: id/FK/status filters, custom
/// field filters, allowlisted sort, and the caller's row-level visibility.
async fn build_device_list_filter(
    state: &AppState,
    request_context: &RequestContext,
    query: &DeviceListQuery,
) -> Result<DeviceFilter, AppError> {
    let ids = parse_id_filter(query.ids.as_deref())?;
    let tenant_id = parse_csv_uuids(query.tenant_id.as_deref(), "tenant_id")?;
    let site_id = parse_csv_uuids(query.site_id.as_deref(), "site_id")?;
    let role_id = parse_csv_uuids(query.role_id.as_deref(), "role_id")?;
    let device_type_id = parse_csv_uuids(query.device_type_id.as_deref(), "device_type_id")?;
    let platform_id = parse_csv_uuids(query.platform_id.as_deref(), "platform_id")?;
    let cluster_id = parse_csv_uuids(query.cluster_id.as_deref(), "cluster_id")?;
    let rack_id = parse_csv_uuids(query.rack_id.as_deref(), "rack_id")?;
    let location_id = parse_csv_uuids(query.location_id.as_deref(), "location_id")?;
    let status = parse_csv_strings(query.status.as_deref());
    let custom_fields =
        build_custom_field_list_filter(state, CONTENT_TYPE_DEVICE, &query.custom_field_filters)
            .await?;
    let sort = SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        DEVICE_COLUMNS,
    )?;
    // Row-level visibility: browsing the inventory needs
    // `Read × Inventory` (the coarse gate); WHICH device rows are returned
    // is further scoped by the caller's `Read × CONNECTION_TARGET` policies. A
    // caller with no applicable connection-target read grant sees an empty page
    // (not a 403 — they hold the inventory-browse capability, just no hosts).
    let visibility = resolve_inventory_connection_target_visibility(state, request_context)
        .await?
        .into_filter();
    Ok(DeviceFilter {
        ids,
        tenant_id,
        site_id,
        role_id,
        device_type_id,
        platform_id,
        cluster_id,
        rack_id,
        location_id,
        status,
        custom_fields,
        sort,
        visibility,
        ..DeviceFilter::default()
    })
}

/// Hydrate a device list page: resolve primary IPs and mask custom fields in one
/// pass for the whole page, then map to response rows.
async fn map_device_list_page(
    state: &AppState,
    rows: Vec<DeviceListView>,
) -> Result<Vec<DeviceListRowResponse>, AppError> {
    let mut ip_ids = Vec::new();
    for row in &rows {
        push_primary_ip_ids(&mut ip_ids, row.primary_ip4_id, row.primary_ip6_id);
    }
    let ip_map = state.services.ipam.map_ip_addresses(&ip_ids).await?;
    // One masking pass for the whole page (3 queries), never per row.
    let cf_entities: Vec<(Uuid, serde_json::Value)> = rows
        .iter()
        .map(|row| {
            (
                row.id,
                row.custom_fields.clone().unwrap_or(serde_json::Value::Null),
            )
        })
        .collect();
    let masked_page = masked_custom_fields_page(state, CONTENT_TYPE_DEVICE, cf_entities).await?;
    let mut items = Vec::with_capacity(rows.len());
    for (row, custom_fields) in rows.into_iter().zip(masked_page) {
        let primary_ip = resolve_primary_ip(row.primary_ip4_id, row.primary_ip6_id, &ip_map);
        let custom_fields = Some(custom_fields);
        items.push(DeviceListRowResponse {
            id: row.id,
            name: row.name,
            status: row.status,
            site_id: row.site_id,
            site_name: row.site_name,
            tenant_id: row.tenant_id,
            tenant_name: row.tenant_name,
            role_id: row.role_id,
            role_name: row.role_name,
            device_type_id: row.device_type_id,
            device_type_name: row.device_type_name,
            platform_id: row.platform_id,
            platform_name: row.platform_name,
            rack_id: row.rack_id,
            rack_name: row.rack_name,
            primary_ip,
            serial: row.serial,
            asset_tag: row.asset_tag,
            position: row.position,
            face: row.face,
            airflow: row.airflow,
            custom_fields,
        });
    }
    Ok(items)
}

/// List devices.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/devices",
    tag = "inventory",
    params(DeviceListQuery),
    responses(
        (status = 200, description = "Devices listed", body = InventoryListEnvelope<DeviceListRowResponse>),
        (status = "4XX", description = "Device list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Device list request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_devices(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<DeviceListQuery>,
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
    let filter = match build_device_list_filter(&state, &request_context, &query).await {
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
        .list_devices_hydrated(
            crate::contexts::inventory::application::InventoryDeviceListQuery::new(
                filter,
                query.limit,
                query.offset,
            )
            .with_search(search),
        )
        .await
    {
        Ok((rows, total)) => match map_device_list_page(&state, rows).await {
            Ok(items) => {
                let envelope = InventoryListEnvelope {
                    items,
                    total,
                    limit: resolved_limit,
                    offset: resolved_offset,
                };
                (StatusCode::OK, Json(envelope)).into_response()
            }
            Err(error) => handle_app_error(error),
        },
        Err(error) => handle_app_error(error),
    }
}

/// Get a device.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/devices/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Device id"),
    ),
    responses(
        (status = 200, description = "Device", body = DeviceResponse),
        (status = 404, description = "Device not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Device request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Device request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_device(
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
    let device_id = match parse_uuid(&id, "device_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.inventory_admin.clone();
    match service.get_device(device_id).await {
        Ok(Some(device)) => {
            // Row-level visibility: a device the caller's
            // connection-target read policies hide from the list must not
            // resolve by direct URL either — hidden and nonexistent ids both
            // read as 404.
            // Region is site-derived, not a device column — enrich so a
            // region-scoped policy admits/denies deep-links exactly like the
            // SQL-filtered list (which resolves region via subquery).
            let facets = match enrich_facets_with_region(
                &state,
                ConnectionTargetFacets::from(&device),
            )
            .await
            {
                Ok(facets) => facets,
                Err(error) => return handle_app_error(error),
            };
            let row = connection_target_row_facets(device.id, &facets);
            match connection_target_row_visible(&state, &request_context, &row).await {
                Ok(true) => {}
                Ok(false) => {
                    return handle_app_error(AppError::NotFound("Device not found".to_string()));
                }
                Err(error) => return handle_app_error(error),
            }
            let mut ip_ids = Vec::new();
            push_primary_ip_ids(&mut ip_ids, device.primary_ip4_id, device.primary_ip6_id);
            let ip_map = match state.services.ipam.map_ip_addresses(&ip_ids).await {
                Ok(map) => map,
                Err(error) => return handle_app_error(error),
            };
            let primary_ip =
                resolve_primary_ip(device.primary_ip4_id, device.primary_ip6_id, &ip_map);
            let raw_custom_fields = device
                .custom_fields
                .clone()
                .unwrap_or(serde_json::Value::Null);
            let custom_fields = match masked_custom_fields(
                &state,
                CONTENT_TYPE_DEVICE,
                device.id,
                &raw_custom_fields,
            )
            .await
            {
                Ok(value) => Some(value),
                Err(error) => return handle_app_error(error),
            };
            let response = map_device_response(device, primary_ip, custom_fields);
            (StatusCode::OK, Json(response)).into_response()
        }
        Ok(None) => handle_app_error(AppError::NotFound("Device not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Get a device's rendered config context.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/devices/{id}/config-context",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Device id"),
    ),
    responses(
        (status = 200, description = "Rendered config context", body = RenderedConfigContextResponse),
        (status = 404, description = "Device not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Config context request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Config context request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_device_config_context(
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
    let device_id = match parse_uuid(&id, "device_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let device = match state.services.inventory_admin.get_device(device_id).await {
        Ok(Some(device)) => device,
        Ok(None) => return handle_app_error(AppError::NotFound("Device not found".to_string())),
        Err(error) => return handle_app_error(error),
    };
    // Same deep-link gate as `get_device`: the rendered config context of a
    // hidden device leaks at least as much as the device record itself.
    // Same region enrichment as `get_device` (see comment there).
    let facets =
        match enrich_facets_with_region(&state, ConnectionTargetFacets::from(&device)).await {
            Ok(facets) => facets,
            Err(error) => return handle_app_error(error),
        };
    let row = connection_target_row_facets(device.id, &facets);
    match connection_target_row_visible(&state, &request_context, &row).await {
        Ok(true) => {}
        Ok(false) => {
            return handle_app_error(AppError::NotFound("Device not found".to_string()));
        }
        Err(error) => return handle_app_error(error),
    }
    match state
        .services
        .config_queries
        .render_config_context("device", device.id, device.local_context_data)
        .await
    {
        Ok(rendered) => (
            StatusCode::OK,
            Json(map_rendered_config_context_response(rendered)),
        )
            .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Create a device.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/devices",
    tag = "inventory",
    request_body = DevicePayload,
    responses(
        (status = 201, description = "Device created", body = DeviceResponse),
        (status = "4XX", description = "Device create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Device create request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_device(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<DevicePayload>,
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
    match create_device_record(&state, payload).await {
        Ok(response) => (StatusCode::CREATED, Json(response)).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Validate, persist, primary-IP-bind, and mask one device create payload.
async fn create_device_record(
    state: &AppState,
    payload: DevicePayload,
) -> Result<DeviceResponse, AppError> {
    let name = parse_required_string(&payload.name, "name")?;
    let site_id = parse_optional_uuid(payload.site_id, "site_id")?;
    let tenant_id = parse_optional_uuid(payload.tenant_id, "tenant_id")?;
    let role_id = parse_optional_uuid(payload.role_id.clone(), "role_id")?;
    let device_type_id = parse_optional_uuid(payload.device_type_id.clone(), "device_type_id")?;
    let platform_id = parse_optional_uuid(payload.platform_id.clone(), "platform_id")?;
    let rack_id = parse_optional_uuid(payload.rack_id.clone(), "rack_id")?;
    let primary_ip_input = normalize_optional_string(payload.primary_ip);
    let device_id = Uuid::now_v7();
    // Validate + split before persisting; secret intents applied post-create.
    let custom_field_outcome =
        validate_and_split_custom_fields(state, CONTENT_TYPE_DEVICE, payload.custom_fields, true)
            .await?;
    let custom_field_data = custom_field_outcome
        .as_ref()
        .map(|outcome| outcome.custom_field_data.clone());
    let device = Device {
        id: device_id,
        external_id: None,
        name,
        site_id,
        tenant_id,
        role_id,
        device_type_id,
        platform_id,
        rack_id,
        status: payload.status,
        primary_ip4_id: None,
        primary_ip6_id: None,
        serial: normalize_optional_string(payload.serial),
        asset_tag: normalize_optional_string(payload.asset_tag),
        position: payload.position,
        face: normalize_optional_string(payload.face),
        airflow: normalize_optional_string(payload.airflow),
        description: payload.description,
        comments: payload.comments,
        custom_fields: custom_field_data,
        local_context_data: payload.local_context_data,
        piping_synced_at: None,
        ..Default::default()
    };
    state
        .services
        .inventory_admin
        .create_device(device.clone())
        .await?;
    // Row is persisted — now safe to apply secret intents.
    if let Some(outcome) = custom_field_outcome
        && let Err(error) =
            apply_custom_field_secret_intents(state, device.id, outcome.secret_intents).await
    {
        return Err(error);
    }
    let primary_ip = match state
        .services
        .ipam
        .set_primary_ip_for_device(device.id, device.tenant_id, primary_ip_input.as_deref())
        .await
    {
        Ok(Some(ip_id)) => state
            .services
            .ipam
            .map_ip_addresses(&[ip_id])
            .await?
            .get(&ip_id)
            .cloned(),
        Ok(None) => None,
        Err(error) => return Err(error),
    };
    bump_host_config_cache_generation(state).await;
    let raw_custom_fields = device
        .custom_fields
        .clone()
        .unwrap_or(serde_json::Value::Null);
    let custom_fields = Some(
        masked_custom_fields(state, CONTENT_TYPE_DEVICE, device.id, &raw_custom_fields).await?,
    );
    Ok(map_device_response(device, primary_ip, custom_fields))
}

/// Validate, persist, secret-bind, and primary-IP-bind one bulk device payload.
/// Any failure (parse, custom-field validation, create, secret intent, or
/// primary-IP bind) fails just this item.
async fn create_device_item(
    state: &AppState,
    payload: DevicePayload,
) -> Result<DeviceResponse, AppError> {
    let name = parse_required_string(&payload.name, "name")?;
    let site_id = parse_optional_uuid(payload.site_id, "site_id")?;
    let tenant_id = parse_optional_uuid(payload.tenant_id, "tenant_id")?;
    let role_id = parse_optional_uuid(payload.role_id, "role_id")?;
    let device_type_id = parse_optional_uuid(payload.device_type_id, "device_type_id")?;
    let platform_id = parse_optional_uuid(payload.platform_id, "platform_id")?;
    let rack_id = parse_optional_uuid(payload.rack_id, "rack_id")?;
    let primary_ip_input = normalize_optional_string(payload.primary_ip);
    // Validate + split per item; an invalid bag fails just this item.
    let custom_field_outcome =
        validate_and_split_custom_fields(state, CONTENT_TYPE_DEVICE, payload.custom_fields, true)
            .await?;
    let custom_field_data = custom_field_outcome
        .as_ref()
        .map(|outcome| outcome.custom_field_data.clone());
    let device = Device {
        id: Uuid::now_v7(),
        external_id: None,
        name,
        site_id,
        tenant_id,
        role_id,
        device_type_id,
        platform_id,
        rack_id,
        status: payload.status,
        primary_ip4_id: None,
        primary_ip6_id: None,
        serial: normalize_optional_string(payload.serial),
        asset_tag: normalize_optional_string(payload.asset_tag),
        position: payload.position,
        face: normalize_optional_string(payload.face),
        airflow: normalize_optional_string(payload.airflow),
        description: payload.description,
        comments: payload.comments,
        custom_fields: custom_field_data,
        local_context_data: payload.local_context_data,
        piping_synced_at: None,
        ..Default::default()
    };
    state
        .services
        .inventory_admin
        .create_device(device.clone())
        .await?;
    // Row is persisted — now safe to apply secret intents.
    if let Some(outcome) = custom_field_outcome
        && let Err(error) =
            apply_custom_field_secret_intents(state, device.id, outcome.secret_intents).await
    {
        return Err(error);
    }
    let primary_ip = match state
        .services
        .ipam
        .set_primary_ip_for_device(device.id, device.tenant_id, primary_ip_input.as_deref())
        .await
    {
        Ok(Some(ip_id)) => state
            .services
            .ipam
            .map_ip_addresses(&[ip_id])
            .await?
            .get(&ip_id)
            .cloned(),
        Ok(None) => None,
        Err(error) => return Err(error),
    };
    Ok(map_device_response(device, primary_ip, None))
}

/// Bulk-create devices.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/devices/bulk",
    tag = "inventory",
    request_body = BulkCreateRequest<DevicePayload>,
    responses(
        (status = 200, description = "Devices bulk-created", body = BulkCreateResponse<DeviceResponse>),
        (status = "4XX", description = "Devices bulk-create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Devices bulk-create request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_devices(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(request): EnvelopeJson<BulkCreateRequest<DevicePayload>>,
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
        match create_device_item(&state, payload).await {
            Ok(response) => created.push(response),
            Err(error) => push_bulk_error(&mut errors, index, error),
        }
    }

    if !created.is_empty() {
        bump_host_config_cache_generation(&state).await;
    }

    (StatusCode::OK, Json(BulkCreateResponse { created, errors })).into_response()
}

/// Update a device.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/devices/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Device id"),
    ),
    request_body = DevicePayload,
    responses(
        (status = 200, description = "Device updated", body = DeviceResponse),
        (status = 404, description = "Device not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Device update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Device update request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_device(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<DevicePayload>,
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
    match update_device_record(&state, &id, payload).await {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Validate, persist, primary-IP-bind, and mask one device update payload.
async fn update_device_record(
    state: &AppState,
    id: &str,
    payload: DevicePayload,
) -> Result<DeviceResponse, AppError> {
    let device_id = parse_uuid(id, "device_id")?;
    let name = parse_required_string(&payload.name, "name")?;
    let site_id = parse_optional_uuid(payload.site_id, "site_id")?;
    let tenant_id = parse_optional_uuid(payload.tenant_id, "tenant_id")?;
    let role_id = parse_optional_uuid(payload.role_id.clone(), "role_id")?;
    let device_type_id = parse_optional_uuid(payload.device_type_id.clone(), "device_type_id")?;
    let platform_id = parse_optional_uuid(payload.platform_id.clone(), "platform_id")?;
    let rack_id = parse_optional_uuid(payload.rack_id.clone(), "rack_id")?;
    let primary_ip_input = normalize_optional_string(payload.primary_ip);
    let (existing_custom_fields, existing_local_context_data) =
        match state.services.inventory_admin.get_device(device_id).await {
            Ok(None) => {
                return Err(AppError::NotFound("Device not found".to_string()));
            }
            Ok(Some(existing)) => (existing.custom_fields, existing.local_context_data),
            Err(error) => return Err(error),
        };

    // An omitted `custom_fields` leaves the stored data untouched; a present
    // map is validated, split, and replaces it.
    let custom_field_data = match payload.custom_fields {
        Some(incoming) => {
            apply_custom_fields_write(state, CONTENT_TYPE_DEVICE, device_id, Some(incoming)).await?
        }
        None => existing_custom_fields,
    };

    // An omitted `local_context_data` preserves the stored value; a present
    // value replaces it wholesale (arbitrary JSON, no validation).
    let local_context_data = payload.local_context_data.or(existing_local_context_data);

    let device = Device {
        id: device_id,
        external_id: None,
        name,
        site_id,
        tenant_id,
        role_id,
        device_type_id,
        platform_id,
        rack_id,
        status: payload.status,
        primary_ip4_id: None,
        primary_ip6_id: None,
        serial: normalize_optional_string(payload.serial),
        asset_tag: normalize_optional_string(payload.asset_tag),
        position: payload.position,
        face: normalize_optional_string(payload.face),
        airflow: normalize_optional_string(payload.airflow),
        description: payload.description,
        comments: payload.comments,
        custom_fields: custom_field_data,
        local_context_data,
        piping_synced_at: None,
        ..Default::default()
    };
    state
        .services
        .inventory_admin
        .update_device(device.clone())
        .await?;
    let primary_ip = match state
        .services
        .ipam
        .set_primary_ip_for_device(device.id, device.tenant_id, primary_ip_input.as_deref())
        .await
    {
        Ok(Some(ip_id)) => state
            .services
            .ipam
            .map_ip_addresses(&[ip_id])
            .await?
            .get(&ip_id)
            .cloned(),
        Ok(None) => None,
        Err(error) => return Err(error),
    };
    bump_host_config_cache_generation(state).await;
    let raw_custom_fields = device
        .custom_fields
        .clone()
        .unwrap_or(serde_json::Value::Null);
    let custom_fields = Some(
        masked_custom_fields(state, CONTENT_TYPE_DEVICE, device.id, &raw_custom_fields).await?,
    );
    Ok(map_device_response(device, primary_ip, custom_fields))
}

/// Delete a device.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/devices/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Device id"),
    ),
    responses(
        (status = 204, description = "Device deleted"),
        (status = 404, description = "Device not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Device delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Device delete request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_device(
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
    let device_id = match parse_uuid(&id, "device_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.inventory_admin.clone();
    match service.get_device(device_id).await {
        Ok(None) => return handle_app_error(AppError::NotFound("Device not found".to_string())),
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };

    match service.delete_device(device_id).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => handle_app_error(error),
    }
}
