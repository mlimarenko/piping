//! Minimal IPAM REST endpoints: interfaces and IP addresses.
//!
//! These endpoints let the UI attach an IP to a device or virtual machine by
//! first ensuring the target interface exists (POST `/interfaces` for devices,
//! POST `/vm-interfaces` for VMs) and then creating an `ipam_ipaddress` row
//! bound to that interface (POST `/ip-addresses`).
//!
//! Handlers here never run SQL directly; all persistence goes through the
//! `InterfaceRepository` and `IpAddressRepository` traits on `AppState`.

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
use crate::contexts::inventory::infrastructure::persistence::repositories::dcim::interface_repository::InterfaceListFilter;
use crate::contexts::inventory::infrastructure::persistence::{
    BulkInterfaceResult, INTERFACE_COLUMNS, IP_ADDRESS_COLUMNS, InterfaceRecord,
    InterfaceRepository, IpAddressListFilter, IpAddressRecord, IpAddressRepository, NewInterface,
    NewIpAddress, UpdateInterface, UpdateIpAddress, normalize_cidr,
};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_strings, parse_csv_uuids};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

use super::super::{
    InventoryListEnvelope, ensure_access, parse_optional_uuid, parse_required_string, parse_uuid,
};

/// Query parameters for IP address listing.
#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct IpAddressListQuery {
    pub search: Option<String>,
    /// CIDR prefix; when set, only addresses contained in this prefix are returned.
    pub parent_prefix: Option<String>,
    pub device_id: Option<String>,
    pub virtual_machine_id: Option<String>,
    pub status: Option<String>,
    pub role: Option<String>,
    pub assigned_object_id: Option<String>,
    pub assigned_object_type_id: Option<String>,
    pub tenant_id: Option<String>,
    pub vrf_id: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

/// Query parameters for interface listing.
#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct InterfaceListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    pub device_id: Option<String>,
    #[serde(rename = "type")]
    pub type_: Option<String>,
    pub enabled: Option<String>,
    pub mgmt_only: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct IpAddressCreatePayload {
    pub address: String,
    pub interface_id: String,
    pub status: Option<String>,
    pub dns_name: Option<String>,
    pub description: Option<String>,
    pub tenant_id: Option<String>,
    pub vrf_id: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct IpAddressUpdatePayload {
    pub address: Option<String>,
    pub status: Option<String>,
    pub interface_id: Option<String>,
    pub dns_name: Option<String>,
    pub description: Option<String>,
    pub tenant_id: Option<Option<String>>,
    pub vrf_id: Option<Option<String>>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct InterfaceCreatePayload {
    pub device_id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub type_: Option<String>,
    pub label: Option<String>,
    pub description: Option<String>,
    pub enabled: Option<bool>,
    pub mgmt_only: Option<bool>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct InterfaceUpdatePayload {
    pub name: Option<String>,
    #[serde(rename = "type")]
    pub type_: Option<String>,
    pub label: Option<String>,
    pub description: Option<String>,
    pub enabled: Option<bool>,
    pub mgmt_only: Option<bool>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct InterfaceBulkCreatePayload {
    #[serde(deserialize_with = "crate::api::rest::routes::validation::deserialize_bulk_items")]
    #[schema(min_items = 1)]
    pub items: Vec<InterfaceCreatePayload>,
}

fn parse_bool_enum_values(raw: Option<&str>, label: &str) -> Result<Vec<String>, AppError> {
    let mut out = Vec::new();
    for value in parse_csv_strings(raw) {
        let normalized = value.to_ascii_lowercase();
        match normalized.as_str() {
            "true" | "false" => {
                if !out.contains(&normalized) {
                    out.push(normalized);
                }
            }
            _ => {
                return Err(AppError::Validation(format!(
                    "Invalid {label} value {value:?}: expected one of: true, false"
                )));
            }
        }
    }
    Ok(out)
}

fn parse_nullable_optional_uuid(
    raw: Option<Option<String>>,
    label: &str,
) -> Result<Option<Option<Uuid>>, AppError> {
    match raw {
        Some(Some(value)) => parse_optional_uuid(Some(value), label).map(Some),
        Some(None) => Ok(Some(None)),
        None => Ok(None),
    }
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct IpAddressResponse {
    pub id: Uuid,
    pub custom_fields: Option<serde_json::Value>,
    pub address: String,
    pub status: String,
    pub role: Option<String>,
    pub dns_name: String,
    pub description: String,
    pub interface_id: Option<Uuid>,
    pub interface_name: Option<String>,
    pub tenant_id: Option<Uuid>,
    pub tenant_name: Option<String>,
    pub vrf_id: Option<Uuid>,
    pub vrf_name: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct InterfaceResponse {
    pub id: Uuid,
    pub custom_fields: Option<serde_json::Value>,
    pub device_id: Option<Uuid>,
    pub device_name: Option<String>,
    pub name: String,
    pub label: String,
    pub description: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub enabled: bool,
    pub mgmt_only: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkInterfaceResponse {
    pub created: Vec<InterfaceResponse>,
    pub errors: Vec<String>,
}

super::super::impl_inventory_custom_fields!(IpAddressResponse, "ipam.ipaddress");
super::super::impl_inventory_custom_fields!(InterfaceResponse, "dcim.interface");

fn map_ip(record: IpAddressRecord) -> IpAddressResponse {
    IpAddressResponse {
        id: record.id,
        custom_fields: None,
        address: record.address,
        status: record.status,
        role: record.role,
        dns_name: record.dns_name,
        description: record.description,
        interface_id: record.assigned_object_id,
        interface_name: record.interface_name,
        tenant_id: record.tenant_id,
        tenant_name: record.tenant_name,
        vrf_id: record.vrf_id,
        vrf_name: record.vrf_name,
    }
}

fn map_interface(record: InterfaceRecord) -> InterfaceResponse {
    InterfaceResponse {
        id: record.id,
        custom_fields: None,
        device_id: record.device_id,
        device_name: record.device_name,
        name: record.name,
        label: record.label,
        description: record.description,
        type_: record.type_,
        enabled: record.enabled,
        mgmt_only: record.mgmt_only,
    }
}

fn map_bulk_interface_result(result: BulkInterfaceResult) -> BulkInterfaceResponse {
    BulkInterfaceResponse {
        created: result.created.into_iter().map(map_interface).collect(),
        errors: result.errors,
    }
}

// ---------------- IP addresses ----------------

/// List IP addresses.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/ip-addresses",
    tag = "inventory",
    params(IpAddressListQuery),
    responses(
        (status = 200, description = "IP addresses", body = InventoryListEnvelope<IpAddressResponse>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_ip_addresses(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<IpAddressListQuery>,
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

    let device_id = match parse_optional_uuid(query.device_id.clone(), "device_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let search = query
        .search
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| format!("%{value}%"));
    let parent_prefix = match query
        .parent_prefix
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(value) => match normalize_cidr(value) {
            Ok(normalized) => Some(normalized),
            Err(error) => return handle_app_error(error),
        },
        None => None,
    };
    let virtual_machine_id =
        match parse_optional_uuid(query.virtual_machine_id.clone(), "virtual_machine_id") {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };
    let assigned_object_id =
        match parse_csv_uuids(query.assigned_object_id.as_deref(), "assigned_object_id") {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };
    let assigned_object_type_id = match parse_csv_uuids(
        query.assigned_object_type_id.as_deref(),
        "assigned_object_type_id",
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let tenant_id = match parse_csv_uuids(query.tenant_id.as_deref(), "tenant_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let vrf_id = match parse_csv_uuids(query.vrf_id.as_deref(), "vrf_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let resolved_limit = clamp_limit(query.limit);
    let resolved_offset = clamp_offset(query.offset);
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        IP_ADDRESS_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match state
        .repositories
        .ip_addresses
        .list(
            IpAddressListFilter {
                search,
                parent_prefix,
                device_id,
                virtual_machine_id,
                status: parse_csv_strings(query.status.as_deref()),
                role: parse_csv_strings(query.role.as_deref()),
                assigned_object_id,
                assigned_object_type_id,
                tenant_id,
                vrf_id,
            },
            query.limit,
            query.offset,
            sort,
        )
        .await
    {
        Ok((records, total)) => {
            let mut items: Vec<IpAddressResponse> = records.into_iter().map(map_ip).collect();
            if let Err(error) =
                super::super::mask_inventory_projection_page(&state, &mut items).await
            {
                return handle_app_error(error);
            }
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

/// Get an IP address.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/ip-addresses/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "IP address ID"),
    ),
    responses(
        (status = 200, description = "IP address", body = IpAddressResponse),
        (status = 404, description = "IP address not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_ip_address(
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
    match state.repositories.ip_addresses.get_by_id(id).await {
        Ok(Some(record)) => {
            super::super::masked_inventory_response(&state, StatusCode::OK, map_ip(record)).await
        }
        Ok(None) => handle_app_error(AppError::NotFound("IP address not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create an IP address.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/ip-addresses",
    tag = "inventory",
    request_body = IpAddressCreatePayload,
    responses(
        (status = 201, description = "IP address created", body = IpAddressResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "IP address creation is forbidden", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "IP address creation failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_ip_address(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<IpAddressCreatePayload>,
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

    let normalized_address = match normalize_cidr(&payload.address) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let interface_id = match parse_uuid(&payload.interface_id, "interface_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let tenant_id = match parse_optional_uuid(payload.tenant_id.clone(), "tenant_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let vrf_id = match parse_optional_uuid(payload.vrf_id.clone(), "vrf_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let status = payload
        .status
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("active")
        .to_string();

    let new_ip = NewIpAddress {
        id: Uuid::now_v7(),
        address: normalized_address,
        status,
        interface_id,
        dns_name: payload.dns_name,
        description: payload.description,
        tenant_id,
        vrf_id,
    };

    match state.repositories.ip_addresses.create(&new_ip).await {
        Ok(record) => {
            super::super::masked_inventory_response(&state, StatusCode::CREATED, map_ip(record))
                .await
        }
        Err(error) => handle_app_error(error),
    }
}

/// Update an IP address.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/ip-addresses/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "IP address ID"),
    ),
    request_body = IpAddressUpdatePayload,
    responses(
        (status = 200, description = "IP address updated", body = IpAddressResponse),
        (status = 404, description = "IP address not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "IP address update is forbidden", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "IP address update failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_ip_address(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<IpAddressUpdatePayload>,
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

    let address = match payload.address.as_deref() {
        Some(value) => match normalize_cidr(value) {
            Ok(normalized) => Some(normalized),
            Err(error) => return handle_app_error(error),
        },
        None => None,
    };
    let interface_id = match parse_optional_uuid(payload.interface_id.clone(), "interface_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let tenant_id = match parse_nullable_optional_uuid(payload.tenant_id.clone(), "tenant_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let vrf_id = match parse_nullable_optional_uuid(payload.vrf_id.clone(), "vrf_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let changes = UpdateIpAddress {
        address,
        status: payload.status,
        interface_id,
        dns_name: payload.dns_name,
        description: payload.description,
        tenant_id,
        vrf_id,
    };

    match state.repositories.ip_addresses.update(id, &changes).await {
        Ok(Some(record)) => {
            super::super::masked_inventory_response(&state, StatusCode::OK, map_ip(record)).await
        }
        Ok(None) => handle_app_error(AppError::NotFound("IP address not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Delete an IP address.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/ip-addresses/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "IP address ID"),
    ),
    responses(
        (status = 204, description = "IP address deleted"),
        (status = 400, description = "Invalid IP address UUID", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "IP address deletion is forbidden", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 409, description = "IP address is still referenced by other records (DELETE_BLOCKED with blocker details)", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "IP address deletion failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_ip_address(
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
    match state.repositories.ip_addresses.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

// ---------------- Interfaces ----------------

/// List interfaces.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/interfaces",
    tag = "inventory",
    params(InterfaceListQuery),
    responses(
        (status = 200, description = "Interfaces", body = InventoryListEnvelope<InterfaceResponse>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_interfaces(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<InterfaceListQuery>,
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
    let device_id = match parse_csv_uuids(query.device_id.as_deref(), "device_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let enabled = match parse_bool_enum_values(query.enabled.as_deref(), "enabled") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let mgmt_only = match parse_bool_enum_values(query.mgmt_only.as_deref(), "mgmt_only") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let resolved_limit = clamp_limit(query.limit);
    let resolved_offset = clamp_offset(query.offset);
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        INTERFACE_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match state
        .repositories
        .interfaces
        .list(
            InterfaceListFilter {
                search: crate::contexts::shared::list_query::search_pattern(
                    query.search.as_deref(),
                ),
                device_id,
                type_: parse_csv_strings(query.type_.as_deref()),
                enabled,
                mgmt_only,
            },
            query.limit,
            query.offset,
            sort,
        )
        .await
    {
        Ok((records, total)) => {
            let mut items: Vec<InterfaceResponse> =
                records.into_iter().map(map_interface).collect();
            if let Err(error) =
                super::super::mask_inventory_projection_page(&state, &mut items).await
            {
                return handle_app_error(error);
            }
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

/// Get an interface.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/interfaces/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Interface ID"),
    ),
    responses(
        (status = 200, description = "Interface", body = InterfaceResponse),
        (status = 404, description = "Interface not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_interface(
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
    match state.repositories.interfaces.get_by_id(id).await {
        Ok(Some(record)) => {
            super::super::masked_inventory_response(&state, StatusCode::OK, map_interface(record))
                .await
        }
        Ok(None) => handle_app_error(AppError::NotFound("Interface not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create an interface.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/interfaces",
    tag = "inventory",
    request_body = InterfaceCreatePayload,
    responses(
        (status = 201, description = "Interface created", body = InterfaceResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Interface creation is forbidden", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 409, description = "An interface with the same device and name already exists", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Interface creation failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_interface(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<InterfaceCreatePayload>,
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

    let device_id = match parse_uuid(&payload.device_id, "device_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let name = match parse_required_string(&payload.name, "name") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let type_ = payload
        .type_
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("virtual")
        .to_string();

    let new_interface = NewInterface {
        id: Uuid::now_v7(),
        device_id,
        name,
        type_,
        label: payload.label,
        description: payload.description,
        enabled: payload.enabled,
        mgmt_only: payload.mgmt_only,
    };

    match state.repositories.interfaces.create(&new_interface).await {
        Ok(record) => {
            super::super::masked_inventory_response(
                &state,
                StatusCode::CREATED,
                map_interface(record),
            )
            .await
        }
        Err(error) => handle_app_error(error),
    }
}

/// Update an interface.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/interfaces/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Interface ID"),
    ),
    request_body = InterfaceUpdatePayload,
    responses(
        (status = 200, description = "Interface updated", body = InterfaceResponse),
        (status = 404, description = "Interface not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Interface update is forbidden", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 409, description = "An interface with the same device and name already exists", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Interface update failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_interface(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<InterfaceUpdatePayload>,
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
    let changes = UpdateInterface {
        name: payload.name,
        type_: payload.type_,
        label: payload.label,
        description: payload.description,
        enabled: payload.enabled,
        mgmt_only: payload.mgmt_only,
    };
    match state.repositories.interfaces.update(id, &changes).await {
        Ok(Some(record)) => {
            super::super::masked_inventory_response(&state, StatusCode::OK, map_interface(record))
                .await
        }
        Ok(None) => handle_app_error(AppError::NotFound("Interface not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Delete an interface.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/interfaces/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Interface ID"),
    ),
    responses(
        (status = 204, description = "Interface deleted"),
        (status = 400, description = "Invalid interface UUID", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Interface deletion is forbidden", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 409, description = "Interface is still referenced by other records (DELETE_BLOCKED with blocker details)", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Interface deletion failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_interface(
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
    match state.repositories.interfaces.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create interfaces.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/interfaces/bulk",
    tag = "inventory",
    request_body = InterfaceBulkCreatePayload,
    responses(
        (status = 200, description = "Bulk interface create result", body = BulkInterfaceResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_interfaces(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<InterfaceBulkCreatePayload>,
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
        let device_id = match parse_uuid(&p.device_id, "device_id") {
            Ok(value) => value,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        let name = match parse_required_string(&p.name, "name") {
            Ok(value) => value,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        let type_ = p
            .type_
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("virtual")
            .to_string();
        items.push(NewInterface {
            id: Uuid::now_v7(),
            device_id,
            name,
            type_,
            label: p.label,
            description: p.description,
            enabled: p.enabled,
            mgmt_only: p.mgmt_only,
        });
    }

    match state.repositories.interfaces.bulk_create(&items).await {
        Ok(result) => {
            let mut body = map_bulk_interface_result(result);
            match super::super::mask_inventory_projection_page(&state, &mut body.created).await {
                Ok(()) => (StatusCode::OK, Json(body)).into_response(),
                Err(error) => handle_app_error(error),
            }
        }
        Err(error) => handle_app_error(error),
    }
}
