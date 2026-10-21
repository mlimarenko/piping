//! Services REST routes — full CRUD + bulk (IPAM > Services).
//!
//! Handlers never run SQL directly; all persistence goes through
//! `ServiceRepository` on `AppState`.

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
use crate::contexts::inventory::domain::ObjectRef;
use crate::contexts::inventory::domain::content_type::SERVICE_PARENT_OBJECT_TYPES;
use crate::contexts::inventory::infrastructure::persistence::repositories::ipam::service_repository::ServiceFilter;
use crate::contexts::inventory::infrastructure::persistence::{
    BulkCreateResult, NewService, SERVICE_COLUMNS, ServiceRecord, ServiceRepository, UpdateService,
};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_strings, parse_csv_uuids};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

use super::super::{
    InventoryListEnvelope, ObjectRefInput, ensure_access, parse_required_string, parse_uuid,
    resolve_object_type,
};

/// Query parameters for service listing.
#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct ServiceListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    /// Comma-separated service protocols; matches any (`IN`).
    pub protocol: Option<String>,
    /// Comma-separated parent object ids; matches any (`IN`).
    pub parent_object_id: Option<String>,
    /// Comma-separated owner ids; matches any (`IN`).
    pub owner_id: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ServiceCreatePayload {
    pub name: String,
    pub protocol: String,
    pub ports: Vec<i32>,
    /// Canonical polymorphic parent: `{ object_type: "app_label.model", object_id }`.
    /// Allowed targets: `dcim.device`, `virtualization.virtualmachine`.
    pub parent_object: ObjectRefInput,
    pub description: Option<String>,
    /// Explicit listener/egress IP bindings; never inferred from management IPs.
    #[serde(default)]
    pub ipaddresses: Vec<Uuid>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ServiceUpdatePayload {
    pub name: Option<String>,
    pub protocol: Option<String>,
    pub ports: Option<Vec<i32>>,
    /// When present, re-points the polymorphic parent.
    pub parent_object: Option<ObjectRefInput>,
    pub description: Option<String>,
    /// Omitted keeps existing bindings; [] clears them.
    pub ipaddresses: Option<Vec<Uuid>>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ServiceBulkCreatePayload {
    #[serde(deserialize_with = "crate::api::rest::routes::validation::deserialize_bulk_items")]
    #[schema(min_items = 1)]
    pub items: Vec<ServiceCreatePayload>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ServiceResponse {
    pub id: Uuid,
    pub custom_fields: Option<serde_json::Value>,
    pub name: String,
    /// `null` when the imported source row omitted it.
    pub protocol: Option<String>,
    /// `null` when the imported source row omitted it.
    pub ports: Option<Vec<i32>>,
    /// Canonical port-mapping projection, `<protocol>/<port>`
    /// endpoint strings in source order. Empty when the service declares none.
    pub port_mappings: Vec<String>,
    pub ipaddresses: Vec<Uuid>,
    /// Materialized polymorphic parent. NOT NULL.
    pub parent_object: ObjectRef,
    pub description: Option<String>,
    pub owner_id: Option<Uuid>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkServiceResponse {
    pub created: Vec<ServiceResponse>,
    pub errors: Vec<String>,
}

super::super::impl_inventory_custom_fields!(ServiceResponse, "ipam.service");

fn map_service(record: ServiceRecord, parent_object: ObjectRef) -> ServiceResponse {
    ServiceResponse {
        id: record.id,
        custom_fields: None,
        name: record.name,
        protocol: record.protocol,
        ports: record.ports,
        port_mappings: record.port_mappings,
        ipaddresses: record.ipaddresses,
        parent_object,
        description: record.description,
        owner_id: record.owner_id,
    }
}

/// Resolve a single record into a response (get/create/update path).
async fn map_one_service(
    state: &AppState,
    record: ServiceRecord,
) -> Result<ServiceResponse, AppError> {
    let parent_object = state
        .repositories
        .object_ref_resolver
        .resolve_one(record.parent_object_type_id, record.parent_object_id)
        .await?;
    let mut response = map_service(record, parent_object);
    super::super::mask_inventory_projection_page(state, std::slice::from_mut(&mut response))
        .await?;
    Ok(response)
}

/// Batch-resolve a page of records (one resolver round-trip, no N+1).
async fn map_services(
    state: &AppState,
    records: Vec<ServiceRecord>,
) -> Result<Vec<ServiceResponse>, AppError> {
    let pairs: Vec<(Uuid, Uuid)> = records
        .iter()
        .map(|record| (record.parent_object_type_id, record.parent_object_id))
        .collect();
    let refs = state
        .repositories
        .object_ref_resolver
        .resolve(&pairs)
        .await?;
    let mut responses: Vec<_> = records
        .into_iter()
        .map(|record| {
            let parent_object = refs
                .get(&(record.parent_object_type_id, record.parent_object_id))
                .cloned()
                .unwrap_or(ObjectRef {
                    object_type: String::new(),
                    object_id: record.parent_object_id,
                    label: None,
                });
            map_service(record, parent_object)
        })
        .collect();
    super::super::mask_inventory_projection_page(state, &mut responses).await?;
    Ok(responses)
}

async fn map_bulk_service_result(
    state: &AppState,
    result: BulkCreateResult<ServiceRecord>,
) -> Result<BulkServiceResponse, AppError> {
    Ok(BulkServiceResponse {
        created: map_services(state, result.created).await?,
        errors: result.errors,
    })
}

/// List services.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/services",
    tag = "inventory",
    params(ServiceListQuery),
    responses(
        (status = 200, description = "Services", body = InventoryListEnvelope<ServiceResponse>),
        (status = "4XX", description = "Service list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Service list failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_services(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ServiceListQuery>,
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
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        SERVICE_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let parent_object_id =
        match parse_csv_uuids(query.parent_object_id.as_deref(), "parent_object_id") {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };
    let owner_id = match parse_csv_uuids(query.owner_id.as_deref(), "owner_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let filter = ServiceFilter {
        search: crate::contexts::shared::list_query::search_pattern(query.search.as_deref()),
        protocol: parse_csv_strings(query.protocol.as_deref()),
        parent_object_id,
        owner_id,
        sort,
    };
    match state
        .repositories
        .services
        .list(filter, query.limit, query.offset)
        .await
    {
        Ok((records, total)) => match map_services(&state, records).await {
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
        },
        Err(error) => handle_app_error(error),
    }
}

/// Get a service.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/services/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Service id"),
    ),
    responses(
        (status = 200, description = "Service", body = ServiceResponse),
        (status = 404, description = "Service not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Service get request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Service get failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_service(
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
    match state.repositories.services.get_by_id(id).await {
        Ok(Some(record)) => match map_one_service(&state, record).await {
            Ok(body) => (StatusCode::OK, Json(body)).into_response(),
            Err(error) => handle_app_error(error),
        },
        Ok(None) => handle_app_error(AppError::NotFound("Service not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a service.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/services",
    tag = "inventory",
    request_body = ServiceCreatePayload,
    responses(
        (status = 201, description = "Service created", body = ServiceResponse),
        (status = "4XX", description = "Service create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Service create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_service(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<ServiceCreatePayload>,
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
    let protocol = match parse_required_string(&payload.protocol, "protocol") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let parent_object_type_id = match resolve_object_type(
        &state.db_pool,
        &payload.parent_object.object_type,
        SERVICE_PARENT_OBJECT_TYPES,
    )
    .await
    {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let new_service = NewService {
        ipaddresses: payload.ipaddresses,
        id: Uuid::now_v7(),
        name,
        protocol,
        ports: payload.ports,
        parent_object_type_id,
        parent_object_id: payload.parent_object.object_id,
        description: payload.description,
        owner_id: None,
    };

    match state.repositories.services.create(&new_service).await {
        Ok(record) => match map_one_service(&state, record).await {
            Ok(body) => (StatusCode::CREATED, Json(body)).into_response(),
            Err(error) => handle_app_error(error),
        },
        Err(error) => handle_app_error(error),
    }
}

/// Update a service.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/services/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Service id"),
    ),
    request_body = ServiceUpdatePayload,
    responses(
        (status = 200, description = "Service updated", body = ServiceResponse),
        (status = 404, description = "Service not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Service update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Service update failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_service(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<ServiceUpdatePayload>,
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

    let (parent_object_type_id, parent_object_id) = match payload.parent_object {
        Some(input) => {
            let type_id = match resolve_object_type(
                &state.db_pool,
                &input.object_type,
                SERVICE_PARENT_OBJECT_TYPES,
            )
            .await
            {
                Ok(value) => value,
                Err(error) => return handle_app_error(error),
            };
            (Some(type_id), Some(input.object_id))
        }
        None => (None, None),
    };

    let changes = UpdateService {
        ipaddresses: payload.ipaddresses,
        name: payload.name,
        protocol: payload.protocol,
        ports: payload.ports,
        parent_object_type_id,
        parent_object_id,
        description: payload.description,
    };

    match state.repositories.services.update(id, &changes).await {
        Ok(Some(record)) => match map_one_service(&state, record).await {
            Ok(body) => (StatusCode::OK, Json(body)).into_response(),
            Err(error) => handle_app_error(error),
        },
        Ok(None) => handle_app_error(AppError::NotFound("Service not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a service.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/services/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Service id"),
    ),
    responses(
        (status = 204, description = "Service deleted"),
        (status = "4XX", description = "Service delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Service delete failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_service(
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
    match state.repositories.services.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create services.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/services/bulk",
    tag = "inventory",
    request_body = ServiceBulkCreatePayload,
    responses(
        (status = 200, description = "Bulk service create result", body = BulkServiceResponse),
        (status = "4XX", description = "Bulk service create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Bulk service create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_services(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<ServiceBulkCreatePayload>,
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
        let protocol = match parse_required_string(&p.protocol, "protocol") {
            Ok(value) => value,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        let parent_object_type_id = match resolve_object_type(
            &state.db_pool,
            &p.parent_object.object_type,
            SERVICE_PARENT_OBJECT_TYPES,
        )
        .await
        {
            Ok(value) => value,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        items.push(NewService {
            ipaddresses: p.ipaddresses,
            id: Uuid::now_v7(),
            name,
            protocol,
            ports: p.ports,
            parent_object_type_id,
            parent_object_id: p.parent_object.object_id,
            description: p.description,
            owner_id: None,
        });
    }

    match state.repositories.services.bulk_create(&items).await {
        Ok(result) => match map_bulk_service_result(&state, result).await {
            Ok(body) => (StatusCode::OK, Json(body)).into_response(),
            Err(error) => handle_app_error(error),
        },
        Err(error) => handle_app_error(error),
    }
}
