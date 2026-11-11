//! Native interfaces and IP addresses. IP assignments use a typed object
//! reference to an Interface, VMInterface or FHRP group.
//!
//! Handlers here never run SQL directly; all persistence goes through the
//! `InterfaceRepository` and `IpAddressRepository` traits on `AppState`.

use super::super::ipam::vlan_translation_policies::authorize_vlan_translation_policy;
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
use crate::api::permission_helpers::{ensure_access_with_resource, ensure_inventory_collection_access};
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::api::rest::routes::inventory::association_access::{ensure_association_target_access, resolve_association_refs};
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::domain::{ObjectRef, object_ref::ObjectRefInput};
use crate::contexts::inventory::infrastructure::persistence::repositories::dcim::interface_repository::InterfaceListFilter;
use crate::contexts::inventory::infrastructure::persistence::{
    BulkInterfaceResult, INTERFACE_COLUMNS, IP_ADDRESS_COLUMNS, InterfaceRecord,
    InterfaceRepository, IpAddressListFilter, IpAddressRecord, IpAddressRepository, NewInterface,
    NewIpAddress, UpdateInterface, UpdateIpAddress, normalize_cidr,
};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_strings, parse_csv_uuids};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::contexts::shared::request_context::RequestContext;
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
    pub wireless_link_id: Option<String>,
    pub wireless_lans: Option<String>,
    pub mode: Option<String>,
    pub duplex: Option<String>,
    pub poe_mode: Option<String>,
    pub poe_type: Option<String>,
    pub rf_role: Option<String>,
    pub cable_end: Option<String>,
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
#[serde(deny_unknown_fields)]
pub struct IpAddressCreatePayload {
    pub address: String,
    pub assigned_object: Option<ObjectRefInput>,
    pub status: Option<String>,
    pub dns_name: Option<String>,
    pub description: Option<String>,
    pub tenant_id: Option<String>,
    pub vrf_id: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct IpAddressUpdatePayload {
    pub address: Option<String>,
    pub status: Option<String>,
    /// Omitted preserves the current target; null detaches the address.
    #[serde(default, deserialize_with = "deserialize_assignment_change")]
    pub assigned_object: Option<Option<ObjectRefInput>>,
    pub dns_name: Option<String>,
    pub description: Option<String>,
    pub tenant_id: Option<Option<String>>,
    pub vrf_id: Option<Option<String>>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct InterfaceCreatePayload {
    #[serde(flatten)]
    pub networking: crate::contexts::inventory::domain::InterfaceNetworking,
    pub device_id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub type_: Option<String>,
    pub label: Option<String>,
    pub description: Option<String>,
    pub enabled: Option<bool>,
    pub mgmt_only: Option<bool>,
    pub wireless_link_id: Option<Uuid>,
    #[serde(default)]
    pub wireless_lans: Vec<Uuid>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct InterfaceUpdatePayload {
    #[serde(flatten)]
    pub networking: crate::contexts::inventory::domain::InterfaceNetworkingPatch,
    pub name: Option<String>,
    #[serde(rename = "type")]
    pub type_: Option<String>,
    pub label: Option<String>,
    pub description: Option<String>,
    pub enabled: Option<bool>,
    pub mgmt_only: Option<bool>,
    #[serde(
        default,
        deserialize_with = "crate::api::rest::routes::validation::deserialize_nullable_patch"
    )]
    pub wireless_link_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "super::owners::present")]
    #[schema(nullable = false)]
    pub wireless_lans: Option<Vec<Uuid>>,
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
    pub assigned_object: Option<ObjectRef>,
    pub tenant_id: Option<Uuid>,
    pub tenant_name: Option<String>,
    pub vrf_id: Option<Uuid>,
    pub vrf_name: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct InterfaceResponse {
    #[serde(flatten)]
    pub networking: crate::contexts::inventory::domain::InterfaceNetworking,
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
    pub wireless_link_id: Option<Uuid>,
    pub wireless_lans: Vec<Uuid>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkInterfaceResponse {
    pub created: Vec<InterfaceResponse>,
    pub errors: Vec<String>,
}

super::super::impl_inventory_custom_fields!(IpAddressResponse, "ipam.ipaddress");
super::super::impl_inventory_custom_fields!(InterfaceResponse, "dcim.interface");

fn map_ip(record: IpAddressRecord, assigned_object: Option<ObjectRef>) -> IpAddressResponse {
    IpAddressResponse {
        id: record.id,
        custom_fields: None,
        address: record.address,
        status: record.status,
        role: record.role,
        dns_name: record.dns_name,
        description: record.description,
        assigned_object,
        tenant_id: record.tenant_id,
        tenant_name: record.tenant_name,
        vrf_id: record.vrf_id,
        vrf_name: record.vrf_name,
    }
}

fn deserialize_assignment_change<'de, D>(
    deserializer: D,
) -> Result<Option<Option<ObjectRefInput>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<ObjectRefInput>::deserialize(deserializer).map(Some)
}

async fn map_ips(
    state: &AppState,
    context: &RequestContext,
    records: Vec<IpAddressRecord>,
) -> Result<Vec<IpAddressResponse>, AppError> {
    let pairs: Vec<_> = records
        .iter()
        .filter_map(|record| {
            record
                .assigned_object_type_id
                .zip(record.assigned_object_id)
        })
        .collect();
    let refs = resolve_association_refs(state, context, &pairs).await?;
    Ok(records
        .into_iter()
        .map(|record| {
            let assigned = record
                .assigned_object_type_id
                .zip(record.assigned_object_id)
                .and_then(|pair| refs.get(&pair).cloned());
            map_ip(record, assigned)
        })
        .collect())
}

async fn ip_response(
    state: &AppState,
    context: &RequestContext,
    status: StatusCode,
    record: IpAddressRecord,
) -> axum::response::Response {
    match map_ips(state, context, vec![record]).await {
        Ok(mut records) => {
            super::super::masked_inventory_response(state, status, records.remove(0)).await
        }
        Err(error) => handle_app_error(error),
    }
}

fn map_interface(record: InterfaceRecord) -> InterfaceResponse {
    InterfaceResponse {
        networking: record.networking,
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
        wireless_link_id: record.wireless_link_id,
        wireless_lans: record.wireless_lans,
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
    if let Err(error) =
        ensure_inventory_collection_access(&state, &request_context, Action::Read).await
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
            let mut items = match map_ips(&state, &request_context, records).await {
                Ok(items) => items,
                Err(error) => return handle_app_error(error),
            };
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
    let id = match parse_uuid(&id, "id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access_with_resource(
        &state,
        &request_context,
        Action::Read,
        ResourceType::Inventory,
        Some(&format!("inventory:{id}")),
    )
    .await
    {
        return handle_app_error(error);
    }
    match state.repositories.ip_addresses.get_by_id(id).await {
        Ok(Some(record)) => ip_response(&state, &request_context, StatusCode::OK, record).await,
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
    if let Err(error) =
        ensure_inventory_collection_access(&state, &request_context, Action::Create).await
    {
        return handle_app_error(error);
    }

    let normalized_address = match normalize_cidr(&payload.address) {
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

    if let Some(target) = &payload.assigned_object {
        if let Err(error) = ensure_association_target_access(
            &state,
            &request_context,
            Action::Update,
            &target.object_type,
            target.object_id,
        )
        .await
        {
            return handle_app_error(error);
        }
    }

    let new_ip = NewIpAddress {
        id: Uuid::now_v7(),
        address: normalized_address,
        status,
        assigned_object: payload.assigned_object,
        dns_name: payload.dns_name,
        description: payload.description,
        tenant_id,
        vrf_id,
    };

    match state.repositories.ip_addresses.create(&new_ip).await {
        Ok(record) => ip_response(&state, &request_context, StatusCode::CREATED, record).await,
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
    let id = match parse_uuid(&id, "id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_access_with_resource(
        &state,
        &request_context,
        Action::Update,
        ResourceType::Inventory,
        Some(&format!("inventory:{id}")),
    )
    .await
    {
        return handle_app_error(error);
    }

    let address = match payload.address.as_deref() {
        Some(value) => match normalize_cidr(value) {
            Ok(normalized) => Some(normalized),
            Err(error) => return handle_app_error(error),
        },
        None => None,
    };
    let tenant_id = match parse_nullable_optional_uuid(payload.tenant_id.clone(), "tenant_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let vrf_id = match parse_nullable_optional_uuid(payload.vrf_id.clone(), "vrf_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    // Omitted references retain the persisted assignment and must not evade
    // target authorization on an otherwise unrelated update.
    let existing = match state.repositories.ip_addresses.get_by_id(id).await {
        Ok(Some(record)) => record,
        Ok(None) => return handle_app_error(AppError::NotFound("IP address not found".into())),
        Err(error) => return handle_app_error(error),
    };
    let effective_target = match &payload.assigned_object {
        Some(target) => target.clone(),
        None => match existing
            .assigned_object_type_id
            .zip(existing.assigned_object_id)
        {
            Some((type_id, target_id)) => match state
                .repositories
                .object_ref_resolver
                .resolve_one(type_id, target_id)
                .await
            {
                Ok(target) => Some(ObjectRefInput {
                    object_type: target.object_type,
                    object_id: target.object_id,
                }),
                Err(error) => return handle_app_error(error),
            },
            None => None,
        },
    };
    if let Some(target) = effective_target {
        if let Err(error) = ensure_association_target_access(
            &state,
            &request_context,
            Action::Update,
            &target.object_type,
            target.object_id,
        )
        .await
        {
            return handle_app_error(error);
        }
    }

    let changes = UpdateIpAddress {
        expected_assigned_object: (
            existing.assigned_object_type_id,
            existing.assigned_object_id,
        ),
        address,
        status: payload.status,
        assigned_object: payload.assigned_object,
        dns_name: payload.dns_name,
        description: payload.description,
        tenant_id,
        vrf_id,
    };

    match state.repositories.ip_addresses.update(id, &changes).await {
        Ok(Some(record)) => ip_response(&state, &request_context, StatusCode::OK, record).await,
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
    let id = match parse_uuid(&id, "id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access_with_resource(
        &state,
        &request_context,
        Action::Delete,
        ResourceType::Inventory,
        Some(&format!("inventory:{id}")),
    )
    .await
    {
        return handle_app_error(error);
    }
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
    let wireless_link_id =
        match parse_csv_uuids(query.wireless_link_id.as_deref(), "wireless_link_id") {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };
    let wireless_lans = match parse_csv_uuids(query.wireless_lans.as_deref(), "wireless_lans") {
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
                wireless_link_id,
                wireless_lans,
                networking: vec![
                    ("mode", parse_csv_strings(query.mode.as_deref())),
                    ("duplex", parse_csv_strings(query.duplex.as_deref())),
                    ("poe_mode", parse_csv_strings(query.poe_mode.as_deref())),
                    ("poe_type", parse_csv_strings(query.poe_type.as_deref())),
                    ("rf_role", parse_csv_strings(query.rf_role.as_deref())),
                    ("cable_end", parse_csv_strings(query.cable_end.as_deref())),
                ],
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

    if let Err(error) = authorize_vlan_translation_policy(
        &state,
        &request_context,
        payload.networking.vlan_translation_policy_id,
    )
    .await
    {
        return handle_app_error(error);
    }
    if let Err(error) = authorize_interface_wireless_references(
        &state,
        &request_context,
        payload.wireless_link_id,
        &payload.wireless_lans,
    )
    .await
    {
        return handle_app_error(error);
    }
    let new_interface = NewInterface {
        networking: payload.networking,
        id: Uuid::now_v7(),
        device_id,
        name,
        type_,
        label: payload.label,
        description: payload.description,
        enabled: payload.enabled,
        mgmt_only: payload.mgmt_only,
        wireless_link_id: payload.wireless_link_id,
        wireless_lans: payload.wireless_lans,
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
    let id = match parse_uuid(&id, "id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = ensure_access_with_resource(
        &state,
        &request_context,
        Action::Update,
        ResourceType::Inventory,
        Some(&format!("inventory:{id}")),
    )
    .await
    {
        return handle_app_error(error);
    }
    if let Err(error) = authorize_vlan_translation_policy(
        &state,
        &request_context,
        payload.networking.vlan_translation_policy_id.flatten(),
    )
    .await
    {
        return handle_app_error(error);
    }
    if let Err(error) = authorize_interface_wireless_references(
        &state,
        &request_context,
        payload.wireless_link_id.flatten(),
        payload.wireless_lans.as_deref().unwrap_or_default(),
    )
    .await
    {
        return handle_app_error(error);
    }
    let changes = UpdateInterface {
        networking: payload.networking,
        name: payload.name,
        type_: payload.type_,
        label: payload.label,
        description: payload.description,
        enabled: payload.enabled,
        mgmt_only: payload.mgmt_only,
        wireless_link_id: payload.wireless_link_id,
        wireless_lans: payload.wireless_lans,
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
        if let Err(error) = authorize_vlan_translation_policy(
            &state,
            &request_context,
            p.networking.vlan_translation_policy_id,
        )
        .await
        {
            return handle_app_error(error);
        }
        if let Err(error) = authorize_interface_wireless_references(
            &state,
            &request_context,
            p.wireless_link_id,
            &p.wireless_lans,
        )
        .await
        {
            return handle_app_error(error);
        }
        items.push(NewInterface {
            networking: p.networking,
            id: Uuid::now_v7(),
            device_id,
            name,
            type_,
            label: p.label,
            description: p.description,
            enabled: p.enabled,
            mgmt_only: p.mgmt_only,
            wireless_link_id: p.wireless_link_id,
            wireless_lans: p.wireless_lans,
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

async fn authorize_interface_wireless_references(
    state: &AppState,
    ctx: &RequestContext,
    link: Option<Uuid>,
    lans: &[Uuid],
) -> Result<(), AppError> {
    for id in link.into_iter().chain(lans.iter().copied()) {
        ensure_access_with_resource(
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

#[cfg(test)]
mod assignment_payload_tests {
    use super::{IpAddressCreatePayload, IpAddressUpdatePayload};

    #[test]
    fn assignment_update_distinguishes_omission_null_and_target() {
        let omitted: IpAddressUpdatePayload = serde_json::from_str("{}").expect("omitted");
        assert!(omitted.assigned_object.is_none());
        let detached: IpAddressUpdatePayload =
            serde_json::from_str(r#"{"assigned_object":null}"#).expect("null");
        assert!(matches!(detached.assigned_object, Some(None)));
        let assigned: IpAddressUpdatePayload = serde_json::from_str(r#"{"assigned_object":{"object_type":"ipam.fhrpgroup","object_id":"00000000-0000-0000-0000-000000000092"}}"#).expect("typed assignment");
        let target = assigned.assigned_object.flatten().expect("target");
        assert_eq!(target.object_type, "ipam.fhrpgroup");
        assert_eq!(target.object_id.as_u128(), 0x92);
    }

    #[test]
    fn assignment_input_rejects_response_labels_and_untyped_interface_ids() {
        assert!(
            serde_json::from_str::<IpAddressCreatePayload>(
                r#"{"address":"192.0.2.92","interface_id":"00000000-0000-0000-0000-000000000092"}"#
            )
            .is_err()
        );
        assert!(serde_json::from_str::<IpAddressCreatePayload>(r#"{"address":"192.0.2.92","assigned_object":{"object_type":"ipam.fhrpgroup","object_id":"00000000-0000-0000-0000-000000000092","label":"untrusted"}}"#).is_err());
    }
}
