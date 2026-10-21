use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::Deserialize;
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
use crate::contexts::inventory::domain::VirtualMachine;
use crate::contexts::inventory::infrastructure::persistence::{
    SortSpec, VIRTUAL_MACHINE_COLUMNS, VirtualMachineFilter, VirtualMachineTypeRepository,
    parse_csv_strings, parse_csv_uuids, parse_id_filter,
};
use crate::errors::AppError;

use super::super::{
    BulkCreateRequest, BulkCreateResponse, InventoryListEnvelope, RenderedConfigContextResponse,
    VirtualMachineListRowResponse, VirtualMachinePayload, VirtualMachineResponse,
    apply_custom_field_secret_intents, apply_custom_fields_write,
    bump_host_config_cache_generation, ensure_access, ensure_create_inventory_access,
    map_rendered_config_context_response, map_virtual_machine_response, masked_custom_fields,
    normalize_optional_string, parse_optional_uuid, parse_required_string, parse_uuid,
    push_bulk_error, push_primary_ip_ids, require_bulk_items, resolve_primary_ip,
    validate_and_split_custom_fields,
};
use crate::api::rest::routes::custom_fields::CONTENT_TYPE_VIRTUAL_MACHINE;
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};

/// Query parameters for virtual machine listing — pagination, free-text search,
/// optional parent-FK filters, multi-value status, and allowlisted sorting.
///
/// FK and `status` filters accept comma-separated values (`?cluster_id=a,b`) and
/// combine with AND across fields, `IN` within a field. `sort` is a stable
/// logical column key (`name`, `status`, `cluster`, `tenant`, `role`); `order`
/// is `asc` (default) or `desc`. An unknown sort key or invalid order is
/// rejected with 400.
#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct VirtualMachineListQuery {
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    /// Comma-separated resource UUIDs (maximum 100). An empty value matches no rows.
    pub ids: Option<String>,
    /// Case-insensitive substring search on VM name.
    pub search: Option<String>,
    /// Comma-separated cluster ids; matches any (`IN`).
    pub cluster_id: Option<String>,
    /// Comma-separated site ids; matches any (`IN`).
    pub site_id: Option<String>,
    /// Comma-separated tenant ids; matches any (`IN`).
    pub tenant_id: Option<String>,
    /// Comma-separated role ids; matches any (`IN`).
    pub role_id: Option<String>,
    /// Comma-separated platform ids; matches any (`IN`).
    pub platform_id: Option<String>,
    /// Comma-separated virtual machine type ids; matches any (`IN`).
    pub virtual_machine_type_id: Option<String>,
    /// Comma-separated VM statuses; matches any (`IN`).
    pub status: Option<String>,
    /// Logical sort column key (allowlisted; unknown keys are rejected).
    pub sort: Option<String>,
    /// Sort direction: `asc` (default) or `desc`.
    pub order: Option<String>,
}

/// List virtual machines.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/virtual_machines",
    tag = "inventory",
    params(VirtualMachineListQuery),
    responses(
        (status = 200, description = "Virtual machines listed", body = InventoryListEnvelope<VirtualMachineListRowResponse>),
        (status = "4XX", description = "Virtual machine list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Virtual machine list request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_virtual_machines(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<VirtualMachineListQuery>,
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
    let cluster_id = match parse_csv_uuids(query.cluster_id.as_deref(), "cluster_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let site_id = match parse_csv_uuids(query.site_id.as_deref(), "site_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let tenant_id = match parse_csv_uuids(query.tenant_id.as_deref(), "tenant_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let role_id = match parse_csv_uuids(query.role_id.as_deref(), "role_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let platform_id = match parse_csv_uuids(query.platform_id.as_deref(), "platform_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let status = parse_csv_strings(query.status.as_deref());
    let virtual_machine_type_id = match parse_csv_uuids(
        query.virtual_machine_type_id.as_deref(),
        "virtual_machine_type_id",
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        VIRTUAL_MACHINE_COLUMNS,
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
    // Row-level visibility: same as devices — inventory-browse
    // capability gates the endpoint, connection-target read policies scope which
    // VM rows appear. No applicable grant → empty page.
    let visibility =
        match resolve_inventory_connection_target_visibility(&state, &request_context).await {
            Ok(value) => value.into_filter(),
            Err(error) => return handle_app_error(error),
        };
    let filter = VirtualMachineFilter {
        ids,
        cluster_id,
        site_id,
        tenant_id,
        role_id,
        platform_id,
        virtual_machine_type_id,
        status,
        sort,
        visibility,
    };
    let service = state.services.inventory_admin.clone();
    match service
        .list_virtual_machines_hydrated(
            crate::contexts::inventory::application::InventoryVirtualMachineListQuery::new(
                filter,
                query.limit,
                query.offset,
            )
            .with_search(search),
        )
        .await
    {
        Ok((rows, total)) => {
            let mut ip_ids = Vec::new();
            for row in &rows {
                push_primary_ip_ids(&mut ip_ids, row.primary_ip4_id, row.primary_ip6_id);
            }
            let ip_map = match state.services.ipam.map_ip_addresses(&ip_ids).await {
                Ok(map) => map,
                Err(error) => return handle_app_error(error),
            };
            let items = rows
                .into_iter()
                .map(|row| {
                    let primary_ip =
                        resolve_primary_ip(row.primary_ip4_id, row.primary_ip6_id, &ip_map);
                    VirtualMachineListRowResponse {
                        id: row.id,
                        name: row.name,
                        status: row.status,
                        site_id: row.site_id,
                        site_name: row.site_name,
                        cluster_id: row.cluster_id,
                        cluster_name: row.cluster_name,
                        tenant_id: row.tenant_id,
                        tenant_name: row.tenant_name,
                        role_id: row.role_id,
                        role_name: row.role_name,
                        platform_id: row.platform_id,
                        platform_name: row.platform_name,
                        virtual_machine_type_id: row.virtual_machine_type_id,
                        virtual_machine_type_name: row.virtual_machine_type_name,
                        primary_ip,
                        vcpus: row.vcpus,
                        memory: row.memory,
                        disk: row.disk,
                    }
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

/// Get a virtual machine.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/virtual_machines/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Virtual machine id"),
    ),
    responses(
        (status = 200, description = "Virtual machine", body = VirtualMachineResponse),
        (status = 404, description = "Virtual machine not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Virtual machine request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Virtual machine request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_virtual_machine(
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
    let vm_id = match parse_uuid(&id, "virtual_machine_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.inventory_admin.clone();
    match service.get_virtual_machine(vm_id).await {
        Ok(Some(vm)) => {
            // Row-level visibility: a VM the caller's
            // connection-target read policies hide from the list must not
            // resolve by direct URL either — hidden and nonexistent ids both
            // read as 404.
            // Region is site-derived, not a VM column — enrich so a
            // region-scoped policy admits/denies deep-links exactly like the
            // SQL-filtered list (which resolves region via subquery).
            let facets =
                match enrich_facets_with_region(&state, ConnectionTargetFacets::from(&vm)).await {
                    Ok(facets) => facets,
                    Err(error) => return handle_app_error(error),
                };
            let row = connection_target_row_facets(vm.id, &facets);
            match connection_target_row_visible(&state, &request_context, &row).await {
                Ok(true) => {}
                Ok(false) => {
                    return handle_app_error(AppError::NotFound(
                        "Virtual machine not found".to_string(),
                    ));
                }
                Err(error) => return handle_app_error(error),
            }
            let mut ip_ids = Vec::new();
            push_primary_ip_ids(&mut ip_ids, vm.primary_ip4_id, vm.primary_ip6_id);
            let ip_map = match state.services.ipam.map_ip_addresses(&ip_ids).await {
                Ok(map) => map,
                Err(error) => return handle_app_error(error),
            };
            let primary_ip = resolve_primary_ip(vm.primary_ip4_id, vm.primary_ip6_id, &ip_map);
            let raw_custom_fields = vm.custom_fields.clone().unwrap_or(serde_json::Value::Null);
            let custom_fields = match masked_custom_fields(
                &state,
                CONTENT_TYPE_VIRTUAL_MACHINE,
                vm.id,
                &raw_custom_fields,
            )
            .await
            {
                Ok(value) => Some(value),
                Err(error) => return handle_app_error(error),
            };
            let response = map_virtual_machine_response(vm, primary_ip, custom_fields);
            (StatusCode::OK, Json(response)).into_response()
        }
        Ok(None) => handle_app_error(AppError::NotFound("Virtual machine not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Get a virtual machine's rendered config context.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/virtual_machines/{id}/config-context",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Virtual machine id"),
    ),
    responses(
        (status = 200, description = "Rendered config context", body = RenderedConfigContextResponse),
        (status = 404, description = "Virtual machine not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Config context request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Config context request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_virtual_machine_config_context(
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
    let vm_id = match parse_uuid(&id, "virtual_machine_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let vm = match state
        .services
        .inventory_admin
        .get_virtual_machine(vm_id)
        .await
    {
        Ok(Some(vm)) => vm,
        Ok(None) => {
            return handle_app_error(AppError::NotFound("Virtual machine not found".to_string()));
        }
        Err(error) => return handle_app_error(error),
    };
    // Same deep-link gate as `get_virtual_machine`: the rendered config context
    // of a hidden VM leaks at least as much as the VM record itself.
    // Same region enrichment as `get_virtual_machine` (see comment there).
    let facets = match enrich_facets_with_region(&state, ConnectionTargetFacets::from(&vm)).await {
        Ok(facets) => facets,
        Err(error) => return handle_app_error(error),
    };
    let row = connection_target_row_facets(vm.id, &facets);
    match connection_target_row_visible(&state, &request_context, &row).await {
        Ok(true) => {}
        Ok(false) => {
            return handle_app_error(AppError::NotFound("Virtual machine not found".to_string()));
        }
        Err(error) => return handle_app_error(error),
    }
    match state
        .services
        .config_queries
        .render_config_context("vm", vm.id, vm.local_context_data)
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

/// Create a virtual machine.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/virtual_machines",
    tag = "inventory",
    request_body = VirtualMachinePayload,
    responses(
        (status = 201, description = "Virtual machine created", body = VirtualMachineResponse),
        (status = "4XX", description = "Virtual machine create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Virtual machine create request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_virtual_machine(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<VirtualMachinePayload>,
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
    if let Err(error) = authorize_vm_type_reference(
        &state,
        &request_context,
        payload.virtual_machine_type_id.flatten(),
    )
    .await
    {
        return handle_app_error(error);
    }
    match create_virtual_machine_record(&state, payload).await {
        Ok(response) => (StatusCode::CREATED, Json(response)).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Validate, persist, primary-IP-bind, and mask one VM create payload.
async fn create_virtual_machine_record(
    state: &AppState,
    payload: VirtualMachinePayload,
) -> Result<VirtualMachineResponse, AppError> {
    let name = parse_required_string(&payload.name, "name")?;
    let cluster_id = parse_optional_uuid(payload.cluster_id, "cluster_id")?;
    let site_id = parse_optional_uuid(payload.site_id, "site_id")?;
    let tenant_id = parse_optional_uuid(payload.tenant_id, "tenant_id")?;
    let role_id = parse_optional_uuid(payload.role_id, "role_id")?;
    let mut platform_id = parse_optional_uuid(payload.platform_id.flatten(), "platform_id")?;
    let virtual_machine_type_id = payload.virtual_machine_type_id.flatten();
    let mut vcpus = payload.vcpus.flatten();
    let mut memory = payload.memory.flatten();
    let primary_ip_input = normalize_optional_string(payload.primary_ip);
    let vm_id = Uuid::now_v7();
    apply_virtual_machine_type_defaults(
        state,
        virtual_machine_type_id,
        &mut platform_id,
        &mut vcpus,
        &mut memory,
    )
    .await?;
    // Validate + split before persisting; secret intents applied post-create.
    let custom_field_outcome = validate_and_split_custom_fields(
        state,
        CONTENT_TYPE_VIRTUAL_MACHINE,
        payload.custom_fields,
        true,
    )
    .await?;
    let custom_field_data = custom_field_outcome
        .as_ref()
        .map(|outcome| outcome.custom_field_data.clone());
    let vm = VirtualMachine {
        id: vm_id,
        external_id: None,
        name,
        cluster_id,
        site_id,
        tenant_id,
        role_id,
        platform_id,
        virtual_machine_type_id,
        status: payload.status,
        primary_ip4_id: None,
        primary_ip6_id: None,
        vcpus,
        memory,
        disk: payload.disk,
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
        .create_virtual_machine(vm.clone())
        .await?;
    // Row is persisted — now safe to apply secret intents.
    if let Some(outcome) = custom_field_outcome
        && let Err(error) =
            apply_custom_field_secret_intents(state, vm.id, outcome.secret_intents).await
    {
        return Err(error);
    }
    let primary_ip = match state
        .services
        .ipam
        .set_primary_ip_for_vm(vm.id, vm.tenant_id, primary_ip_input.as_deref())
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
    let raw_custom_fields = vm.custom_fields.clone().unwrap_or(serde_json::Value::Null);
    let custom_fields = Some(
        masked_custom_fields(
            state,
            CONTENT_TYPE_VIRTUAL_MACHINE,
            vm.id,
            &raw_custom_fields,
        )
        .await?,
    );
    Ok(map_virtual_machine_response(vm, primary_ip, custom_fields))
}

/// Validate, persist, secret-bind, and primary-IP-bind one bulk VM payload. Any
/// failure (parse, custom-field validation, create, secret intent, or
/// primary-IP bind) fails just this item.
async fn create_virtual_machine_item(
    state: &AppState,
    payload: VirtualMachinePayload,
) -> Result<VirtualMachineResponse, AppError> {
    let name = parse_required_string(&payload.name, "name")?;
    let cluster_id = parse_optional_uuid(payload.cluster_id, "cluster_id")?;
    let site_id = parse_optional_uuid(payload.site_id, "site_id")?;
    let tenant_id = parse_optional_uuid(payload.tenant_id, "tenant_id")?;
    let role_id = parse_optional_uuid(payload.role_id, "role_id")?;
    let mut platform_id = parse_optional_uuid(payload.platform_id.flatten(), "platform_id")?;
    let virtual_machine_type_id = payload.virtual_machine_type_id.flatten();
    let mut vcpus = payload.vcpus.flatten();
    let mut memory = payload.memory.flatten();
    let primary_ip_input = normalize_optional_string(payload.primary_ip);
    apply_virtual_machine_type_defaults(
        state,
        virtual_machine_type_id,
        &mut platform_id,
        &mut vcpus,
        &mut memory,
    )
    .await?;
    // Validate + split per item; an invalid bag fails just this item.
    let custom_field_outcome = validate_and_split_custom_fields(
        state,
        CONTENT_TYPE_VIRTUAL_MACHINE,
        payload.custom_fields,
        true,
    )
    .await?;
    let custom_field_data = custom_field_outcome
        .as_ref()
        .map(|outcome| outcome.custom_field_data.clone());
    let vm = VirtualMachine {
        id: Uuid::now_v7(),
        external_id: None,
        name,
        cluster_id,
        site_id,
        tenant_id,
        role_id,
        platform_id,
        virtual_machine_type_id,
        status: payload.status,
        primary_ip4_id: None,
        primary_ip6_id: None,
        vcpus,
        memory,
        disk: payload.disk,
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
        .create_virtual_machine(vm.clone())
        .await?;
    // Row is persisted — now safe to apply secret intents.
    if let Some(outcome) = custom_field_outcome
        && let Err(error) =
            apply_custom_field_secret_intents(state, vm.id, outcome.secret_intents).await
    {
        return Err(error);
    }
    let primary_ip = match state
        .services
        .ipam
        .set_primary_ip_for_vm(vm.id, vm.tenant_id, primary_ip_input.as_deref())
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
    Ok(map_virtual_machine_response(vm, primary_ip, None))
}

/// Bulk-create virtual machines.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/virtual_machines/bulk",
    tag = "inventory",
    request_body = BulkCreateRequest<VirtualMachinePayload>,
    responses(
        (status = 200, description = "Virtual machines bulk-created", body = BulkCreateResponse<VirtualMachineResponse>),
        (status = "4XX", description = "Virtual machines bulk-create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Virtual machines bulk-create request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_virtual_machines(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(request): EnvelopeJson<BulkCreateRequest<VirtualMachinePayload>>,
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
        if let Err(error) = authorize_vm_type_reference(
            &state,
            &request_context,
            payload.virtual_machine_type_id.flatten(),
        )
        .await
        {
            push_bulk_error(&mut errors, index, error);
            continue;
        }
        match create_virtual_machine_item(&state, payload).await {
            Ok(response) => created.push(response),
            Err(error) => push_bulk_error(&mut errors, index, error),
        }
    }

    if !created.is_empty() {
        bump_host_config_cache_generation(&state).await;
    }

    (StatusCode::OK, Json(BulkCreateResponse { created, errors })).into_response()
}

/// Update a virtual machine.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/virtual_machines/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Virtual machine id"),
    ),
    request_body = VirtualMachinePayload,
    responses(
        (status = 200, description = "Virtual machine updated", body = VirtualMachineResponse),
        (status = 404, description = "Virtual machine not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Virtual machine update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Virtual machine update request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_virtual_machine(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<VirtualMachinePayload>,
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
    if let Err(error) = authorize_vm_type_reference(
        &state,
        &request_context,
        payload.virtual_machine_type_id.flatten(),
    )
    .await
    {
        return handle_app_error(error);
    }
    match update_virtual_machine_record(&state, &id, payload).await {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Validate, persist, primary-IP-bind, and mask one VM update payload.
async fn update_virtual_machine_record(
    state: &AppState,
    id: &str,
    payload: VirtualMachinePayload,
) -> Result<VirtualMachineResponse, AppError> {
    let vm_id = parse_uuid(id, "virtual_machine_id")?;
    let name = parse_required_string(&payload.name, "name")?;
    let cluster_id = parse_optional_uuid(payload.cluster_id, "cluster_id")?;
    let site_id = parse_optional_uuid(payload.site_id, "site_id")?;
    let tenant_id = parse_optional_uuid(payload.tenant_id, "tenant_id")?;
    let role_id = parse_optional_uuid(payload.role_id, "role_id")?;
    let platform_input = payload.platform_id;
    let primary_ip_input = normalize_optional_string(payload.primary_ip);
    let (
        existing_custom_fields,
        existing_local_context_data,
        existing_type_id,
        existing_platform,
        existing_vcpus,
        existing_memory,
    ) = match state
        .services
        .inventory_admin
        .get_virtual_machine(vm_id)
        .await
    {
        Ok(Some(existing)) => (
            existing.custom_fields,
            existing.local_context_data,
            existing.virtual_machine_type_id,
            existing.platform_id,
            existing.vcpus,
            existing.memory,
        ),
        Ok(None) => {
            return Err(AppError::NotFound("Virtual machine not found".to_string()));
        }
        Err(error) => return Err(error),
    };

    let virtual_machine_type_id = payload.virtual_machine_type_id.unwrap_or(existing_type_id);
    // Range validation applies to the submitted change, not an omitted legacy
    // value. Existing finite compute must not block an unrelated metadata edit.
    super::virtual_machine_types::validate_compute_defaults(
        payload.vcpus.flatten(),
        payload.memory.flatten(),
    )?;
    let vcpus = payload.vcpus.unwrap_or(existing_vcpus);
    let memory = payload.memory.unwrap_or(existing_memory);
    let platform_id = match platform_input {
        None => existing_platform,
        Some(value) => parse_optional_uuid(value, "platform_id")?,
    };
    if let Some(type_id) = virtual_machine_type_id {
        require_virtual_machine_type(state, type_id).await?;
    }

    // An omitted `custom_fields` leaves the stored data untouched; a present
    // map is validated, split, and replaces it.
    let custom_field_data = match payload.custom_fields {
        Some(incoming) => {
            apply_custom_fields_write(state, CONTENT_TYPE_VIRTUAL_MACHINE, vm_id, Some(incoming))
                .await?
        }
        None => existing_custom_fields,
    };

    // An omitted `local_context_data` preserves the stored value; a present
    // value replaces it wholesale (arbitrary JSON, no validation).
    let local_context_data = payload.local_context_data.or(existing_local_context_data);

    let vm = VirtualMachine {
        id: vm_id,
        external_id: None,
        name,
        cluster_id,
        site_id,
        tenant_id,
        role_id,
        platform_id,
        virtual_machine_type_id,
        status: payload.status,
        primary_ip4_id: None,
        primary_ip6_id: None,
        vcpus,
        memory,
        disk: payload.disk,
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
        .update_virtual_machine(vm.clone())
        .await?;
    let primary_ip = match state
        .services
        .ipam
        .set_primary_ip_for_vm(vm.id, vm.tenant_id, primary_ip_input.as_deref())
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
    let raw_custom_fields = vm.custom_fields.clone().unwrap_or(serde_json::Value::Null);
    let custom_fields = Some(
        masked_custom_fields(
            state,
            CONTENT_TYPE_VIRTUAL_MACHINE,
            vm.id,
            &raw_custom_fields,
        )
        .await?,
    );
    Ok(map_virtual_machine_response(vm, primary_ip, custom_fields))
}

async fn authorize_vm_type_reference(
    state: &AppState,
    ctx: &crate::contexts::shared::request_context::RequestContext,
    type_id: Option<Uuid>,
) -> Result<(), AppError> {
    if let Some(id) = type_id {
        crate::api::permission_helpers::ensure_access_with_resource(
            state,
            ctx,
            Action::Read,
            ResourceType::Inventory,
            Some(&format!("inventory:{id}")),
        )
        .await?;
    }
    Ok(())
}

async fn require_virtual_machine_type(
    state: &AppState,
    id: Uuid,
) -> Result<crate::contexts::inventory::domain::VirtualMachineType, AppError> {
    state
        .repositories
        .virtual_machine_types
        .get_by_id(id)
        .await?
        .ok_or_else(|| AppError::Validation("virtual_machine_type_id does not exist".to_owned()))
}

async fn apply_virtual_machine_type_defaults(
    state: &AppState,
    type_id: Option<Uuid>,
    platform: &mut Option<Uuid>,
    vcpus: &mut Option<f64>,
    memory: &mut Option<i32>,
) -> Result<(), AppError> {
    if let Some(id) = type_id {
        require_virtual_machine_type(state, id)
            .await?
            .apply_defaults(platform, vcpus, memory);
    }
    super::virtual_machine_types::validate_compute_defaults(*vcpus, *memory)
}

/// Delete a virtual machine.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/virtual_machines/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Virtual machine id"),
    ),
    responses(
        (status = 204, description = "Virtual machine deleted"),
        (status = 404, description = "Virtual machine not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Virtual machine delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Virtual machine delete request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_virtual_machine(
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
    let vm_id = match parse_uuid(&id, "virtual_machine_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.inventory_admin.clone();
    match service.get_virtual_machine(vm_id).await {
        Ok(None) => {
            return handle_app_error(AppError::NotFound("Virtual machine not found".to_string()));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };

    match service.delete_virtual_machine(vm_id).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => handle_app_error(error),
    }
}
