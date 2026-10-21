//! VMInterfaces REST routes — full CRUD.
//!
//! Handlers never run SQL directly; all persistence goes through `VmInterfaceRepository` on `AppState`.

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
use crate::contexts::inventory::infrastructure::persistence::{
    BulkVmInterfaceResult, NewVmInterface, UpdateVmInterface, VM_INTERFACE_COLUMNS,
    VmInterfaceRecord, VmInterfaceRepository,
};
use crate::contexts::inventory::infrastructure::persistence::repositories::virtualization::vm_interface_repository::VmInterfaceListFilter;
use crate::contexts::shared::list_query::{SortSpec, parse_csv_strings, parse_csv_uuids};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

use super::super::{InventoryListEnvelope, ensure_access, parse_required_string, parse_uuid};

#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct VmInterfaceListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    pub virtual_machine_id: Option<String>,
    pub enabled: Option<String>,
    pub mode: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct VmInterfaceCreatePayload {
    pub virtual_machine_id: String,
    pub name: String,
    pub enabled: Option<bool>,
    pub mtu: Option<i32>,
    pub mode: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct VmInterfaceUpdatePayload {
    pub name: Option<String>,
    pub enabled: Option<bool>,
    pub mtu: Option<i32>,
    pub mode: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct VmInterfaceBulkCreatePayload {
    #[serde(deserialize_with = "crate::api::rest::routes::validation::deserialize_bulk_items")]
    #[schema(min_items = 1)]
    pub items: Vec<VmInterfaceCreatePayload>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct VmInterfaceResponse {
    pub id: Uuid,
    pub custom_fields: Option<serde_json::Value>,
    pub virtual_machine_id: Uuid,
    pub virtual_machine_name: Option<String>,
    pub name: String,
    pub enabled: bool,
    pub mtu: Option<i32>,
    pub mode: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkVmInterfaceResponse {
    pub created: Vec<VmInterfaceResponse>,
    pub errors: Vec<String>,
}

super::super::impl_inventory_custom_fields!(VmInterfaceResponse, "virtualization.vminterface");

fn map_vm_interface(record: VmInterfaceRecord) -> VmInterfaceResponse {
    VmInterfaceResponse {
        id: record.id,
        custom_fields: None,
        virtual_machine_id: record.virtual_machine_id,
        virtual_machine_name: record.virtual_machine_name,
        name: record.name,
        enabled: record.enabled,
        mtu: record.mtu,
        mode: record.mode,
        description: record.description,
    }
}

fn map_bulk_vm_interface_result(result: BulkVmInterfaceResult) -> BulkVmInterfaceResponse {
    BulkVmInterfaceResponse {
        created: result.created.into_iter().map(map_vm_interface).collect(),
        errors: result.errors,
    }
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

/// List VM interfaces.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/vm-interfaces",
    tag = "inventory",
    params(VmInterfaceListQuery),
    responses(
        (status = 200, description = "VM interfaces", body = InventoryListEnvelope<VmInterfaceResponse>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_vm_interfaces(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<VmInterfaceListQuery>,
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

    let virtual_machine_id =
        match parse_csv_uuids(query.virtual_machine_id.as_deref(), "virtual_machine_id") {
            Ok(v) => v,
            Err(error) => return handle_app_error(error),
        };
    let enabled = match parse_bool_enum_values(query.enabled.as_deref(), "enabled") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let resolved_limit = clamp_limit(query.limit);
    let resolved_offset = clamp_offset(query.offset);
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        VM_INTERFACE_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match state
        .repositories
        .vm_interfaces
        .list(
            VmInterfaceListFilter {
                search: crate::contexts::shared::list_query::search_pattern(
                    query.search.as_deref(),
                ),
                virtual_machine_id,
                enabled,
                mode: parse_csv_strings(query.mode.as_deref()),
            },
            query.limit,
            query.offset,
            sort,
        )
        .await
    {
        Ok((records, total)) => {
            let mut items: Vec<VmInterfaceResponse> =
                records.into_iter().map(map_vm_interface).collect();
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

/// Get a VM interface.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/vm-interfaces/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "VM interface ID"),
    ),
    responses(
        (status = 200, description = "VM interface", body = VmInterfaceResponse),
        (status = 404, description = "VM interface not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_vm_interface(
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
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    match state.repositories.vm_interfaces.get_by_id(id).await {
        Ok(Some(record)) => {
            super::super::masked_inventory_response(
                &state,
                StatusCode::OK,
                map_vm_interface(record),
            )
            .await
        }
        Ok(None) => handle_app_error(AppError::NotFound("VMInterface not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a VM interface.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/vm-interfaces",
    tag = "inventory",
    request_body = VmInterfaceCreatePayload,
    responses(
        (status = 201, description = "VM interface created", body = VmInterfaceResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_vm_interface(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<VmInterfaceCreatePayload>,
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
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let virtual_machine_id = match parse_uuid(&payload.virtual_machine_id, "virtual_machine_id") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };

    let new_iface = NewVmInterface {
        id: Uuid::now_v7(),
        virtual_machine_id,
        name,
        enabled: payload.enabled,
        mtu: payload.mtu,
        mode: payload.mode,
        description: payload.description,
    };

    match state.repositories.vm_interfaces.create(&new_iface).await {
        Ok(record) => {
            super::super::masked_inventory_response(
                &state,
                StatusCode::CREATED,
                map_vm_interface(record),
            )
            .await
        }
        Err(error) => handle_app_error(error),
    }
}

/// Update a VM interface.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/vm-interfaces/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "VM interface ID"),
    ),
    request_body = VmInterfaceUpdatePayload,
    responses(
        (status = 200, description = "VM interface updated", body = VmInterfaceResponse),
        (status = 404, description = "VM interface not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_vm_interface(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<VmInterfaceUpdatePayload>,
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
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };

    let changes = UpdateVmInterface {
        name: payload.name,
        enabled: payload.enabled,
        mtu: payload.mtu,
        mode: payload.mode,
        description: payload.description,
    };

    match state.repositories.vm_interfaces.update(id, &changes).await {
        Ok(Some(record)) => {
            super::super::masked_inventory_response(
                &state,
                StatusCode::OK,
                map_vm_interface(record),
            )
            .await
        }
        Ok(None) => handle_app_error(AppError::NotFound("VMInterface not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a VM interface.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/vm-interfaces/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "VM interface ID"),
    ),
    responses(
        (status = 204, description = "VM interface deleted"),
    ),
)]
pub async fn delete_vm_interface(
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
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    match state.repositories.vm_interfaces.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create VM interfaces.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/vm-interfaces/bulk",
    tag = "inventory",
    request_body = VmInterfaceBulkCreatePayload,
    responses(
        (status = 200, description = "Bulk VM interface create result", body = BulkVmInterfaceResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_vm_interfaces(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<VmInterfaceBulkCreatePayload>,
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
            Ok(v) => v,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        let virtual_machine_id = match parse_uuid(&p.virtual_machine_id, "virtual_machine_id") {
            Ok(v) => v,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        items.push(NewVmInterface {
            id: Uuid::now_v7(),
            virtual_machine_id,
            name,
            enabled: p.enabled,
            mtu: p.mtu,
            mode: p.mode,
            description: p.description,
        });
    }

    match state.repositories.vm_interfaces.bulk_create(&items).await {
        Ok(result) => {
            let mut body = map_bulk_vm_interface_result(result);
            match super::super::mask_inventory_projection_page(&state, &mut body.created).await {
                Ok(()) => (StatusCode::OK, Json(body)).into_response(),
                Err(error) => handle_app_error(error),
            }
        }
        Err(error) => handle_app_error(error),
    }
}
