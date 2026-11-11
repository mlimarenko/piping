//! Flow-owned selectors, directed intent CRUD, bulk actions and read-only host policy.
use super::super::{
    InventoryListEnvelope, apply_custom_field_secret_intents, ensure_access,
    masked_custom_fields_page, validate_and_split_custom_fields,
};
pub use crate::contexts::inventory::{
    application::dataflow_policy::DataflowPolicyResponse,
    domain::native::dataflow::{
        DataflowEndpointInput, DataflowEndpointResolutionResponse, DataflowEndpointResponse,
    },
};
use crate::{
    api::{
        connection_authz_attributes::batch_connection_target_is_allowed,
        connection_target_visibility::{
            ConnectionTargetVisibility, resolve_inventory_connection_target_visibility,
            resolve_job_bound_host_visibility,
        },
        middleware::{auth::AuthContext, error::handle_app_error, request_id::RequestId},
        rest::{
            envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated},
            request_context::build_request_context,
        },
    },
    app::AppState,
    contexts::{
        access_control::domain::{Action, ResourceType},
        inventory::{
            application::dataflow_policy::resolve_host_policy,
            domain::native::dataflow::{
                Dataflow, DataflowAddressMode, DataflowEndpoint, DataflowStatus, DataflowTransport,
                EndpointType,
            },
            infrastructure::persistence::repositories::native::dataflow_repository::{
                BulkDataflowAction, DataflowFilter, DataflowPatch,
            },
        },
        shared::{
            pagination::{clamp_limit, clamp_offset},
            request_context::RequestContext,
        },
    },
    errors::AppError,
};
use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DataflowCreatePayload {
    pub name: Option<String>,
    /// null/omission means an empty description.
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub comments: String,
    #[serde(default)]
    #[schema(default = DataflowStatus::default)]
    pub status: DataflowStatus,
    pub sources: Vec<DataflowEndpointInput>,
    pub destinations: Vec<DataflowEndpointInput>,
    #[serde(default)]
    #[schema(default = DataflowTransport::default)]
    pub transport: DataflowTransport,
    /// null/omission means any ports; [] is invalid.
    pub destination_ports: Option<Vec<i32>>,
    pub source_ports: Option<Vec<i32>>,
    pub icmp_types: Option<Vec<i32>>,
    #[serde(default)]
    pub allow_cross_vrf: bool,
    pub group_id: Option<Uuid>,
    pub application_id: Option<Uuid>,
    pub owner_id: Option<Uuid>,
    pub tenant_id: Option<Uuid>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default = "empty_object")]
    pub custom_field_data: Value,
}

fn empty_object() -> Value {
    Value::Object(serde_json::Map::new())
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DataflowUpdatePayload {
    pub name: Option<String>,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub status: Option<DataflowStatus>,
    pub sources: Option<Vec<DataflowEndpointInput>>,
    pub destinations: Option<Vec<DataflowEndpointInput>>,
    pub transport: Option<DataflowTransport>,
    #[serde(default, deserialize_with = "nullable_ports")]
    pub destination_ports: Option<Option<Vec<i32>>>,
    #[serde(default, deserialize_with = "nullable_ports")]
    pub source_ports: Option<Option<Vec<i32>>>,
    #[serde(default, deserialize_with = "nullable_ports")]
    pub icmp_types: Option<Option<Vec<i32>>>,
    pub allow_cross_vrf: Option<bool>,
    #[serde(default, deserialize_with = "nullable_uuid")]
    pub group_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "nullable_uuid")]
    pub application_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "nullable_uuid")]
    pub owner_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "nullable_uuid")]
    pub tenant_id: Option<Option<Uuid>>,
    pub tags: Option<Vec<String>>,
    pub custom_field_data: Option<Value>,
}

fn nullable_ports<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Option<Vec<i32>>>, D::Error> {
    Option::<Vec<i32>>::deserialize(deserializer).map(Some)
}

fn nullable_uuid<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Option<Uuid>>, D::Error> {
    Option::<Uuid>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DataflowResponse {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub comments: String,
    pub status: DataflowStatus,
    pub effective_status: DataflowStatus,
    pub blocked_by_group_id: Option<Uuid>,
    pub sources: Vec<DataflowEndpointResponse>,
    pub destinations: Vec<DataflowEndpointResponse>,
    pub transport: DataflowTransport,
    pub destination_ports: Option<Vec<i32>>,
    pub source_ports: Option<Vec<i32>>,
    pub icmp_types: Option<Vec<i32>>,
    pub allow_cross_vrf: bool,
    pub group_id: Option<Uuid>,
    pub application_id: Option<Uuid>,
    pub owner_id: Option<Uuid>,
    pub tenant_id: Option<Uuid>,
    pub tags: Vec<String>,
    pub inherited_tags: Vec<Uuid>,
    pub effective_tags: Vec<Uuid>,
    pub custom_field_data: Value,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct DataflowListQuery {
    pub status: Option<DataflowStatus>,
    pub effective_status: Option<DataflowStatus>,
    pub search: Option<String>,
    pub source_type: Option<EndpointType>,
    pub source_id: Option<Uuid>,
    pub source_tag_object_type: Option<EndpointType>,
    pub destination_type: Option<EndpointType>,
    pub destination_id: Option<Uuid>,
    pub destination_tag_object_type: Option<EndpointType>,
    pub application_id: Option<Uuid>,
    pub group_id: Option<Uuid>,
    pub transport: Option<DataflowTransport>,
    pub tag_id: Option<Uuid>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct DataflowPolicyQuery {
    pub host_type: EndpointType,
    pub host_id: Uuid,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DataflowBulkKind {
    SetStatus,
    Move,
    Delete,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DataflowBulkActionPayload {
    pub ids: Vec<Uuid>,
    pub action: DataflowBulkKind,
    pub status: Option<DataflowStatus>,
    #[serde(default, deserialize_with = "nullable_uuid")]
    pub group_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "nullable_uuid")]
    pub application_id: Option<Option<Uuid>>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DataflowBulkAffected {
    pub id: Uuid,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DataflowBulkActionResponse {
    pub action: DataflowBulkKind,
    pub affected: Vec<DataflowBulkAffected>,
}

fn filter_endpoint(
    kind: Option<EndpointType>,
    id: Option<Uuid>,
    tag_object_type: Option<EndpointType>,
    side: &str,
) -> Result<Option<DataflowEndpointInput>, AppError> {
    if tag_object_type.is_some() && (kind != Some(EndpointType::Tag) || id.is_none()) {
        return Err(AppError::Validation(format!(
            "{side}_tag_object_type requires {side}_type extras.tag and {side}_id"
        )));
    }
    if let Some(scope) = tag_object_type
        && !scope.is_tag_scope()
    {
        return Err(AppError::Validation(format!(
            "{side}_tag_object_type must be dcim.device or virtualization.virtualmachine"
        )));
    }
    let Some(object_type) = kind else {
        if id.is_some() {
            return Err(AppError::Validation(
                "Reference filter requires object type".into(),
            ));
        }
        return Ok(None);
    };
    let mut selector = DataflowEndpointInput {
        object_type,
        object_id: id,
        address_mode: DataflowAddressMode::Default,
        ipaddresses: Vec::new(),
        tag_object_type,
    };
    if object_type.is_tag() {
        // Omitted tag scope matches both device and VM selectors.
        if selector.object_id.is_none() {
            return Err(AppError::Validation(
                "A tag filter requires source_id or destination_id".into(),
            ));
        }
    } else {
        selector.normalize()?;
    }
    Ok(Some(selector))
}

const DATAFLOW_CONTENT_TYPE: &str = "inventory.dataflow";

async fn project_page(
    state: &AppState,
    flows: Vec<Dataflow>,
) -> Result<Vec<DataflowResponse>, AppError> {
    let entities: Vec<_> = flows
        .iter()
        .map(|flow| (flow.id, flow.custom_field_data.clone()))
        .collect();
    let refs = state.repositories.dataflows.projections(&flows).await?;
    let masked = masked_custom_fields_page(state, DATAFLOW_CONTENT_TYPE, entities).await?;
    Ok(flows
        .into_iter()
        .zip(refs)
        .zip(masked)
        .map(
            |((flow, (sources, destinations)), custom_field_data)| DataflowResponse {
                id: flow.id,
                name: flow.name,
                description: flow.description,
                comments: flow.comments,
                status: flow.status,
                effective_status: flow.effective_status,
                blocked_by_group_id: flow.blocked_by_group_id,
                sources,
                destinations,
                transport: flow.transport,
                destination_ports: flow.destination_ports,
                source_ports: flow.source_ports,
                icmp_types: flow.icmp_types,
                allow_cross_vrf: flow.allow_cross_vrf,
                group_id: flow.group_id,
                application_id: flow.application_id,
                owner_id: flow.owner_id,
                tenant_id: flow.tenant_id,
                tags: flow.tags,
                inherited_tags: flow.inherited_tags,
                effective_tags: flow.effective_tags,
                custom_field_data: if custom_field_data.is_null() {
                    empty_object()
                } else {
                    custom_field_data
                },
            },
        )
        .collect())
}

async fn project(state: &AppState, flow: Dataflow) -> Result<DataflowResponse, AppError> {
    project_page(state, vec![flow])
        .await?
        .pop()
        .ok_or_else(|| AppError::Internal("Missing flow projection".into()))
}

async fn authorize(
    state: &AppState,
    context: &RequestContext,
    action: Action,
) -> Result<(), AppError> {
    ensure_access(state, context, action, ResourceType::Inventory).await
}

/// List dataflows.
#[utoipa::path(get, path = "/api/v1/inventory/dataflows", tag = "inventory", params(DataflowListQuery), responses((status = 200, description = "Successful response", body = InventoryListEnvelope<DataflowResponse>), (status = "4XX", description = "Client error", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", description = "Server error", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn list_dataflows(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<DataflowListQuery>,
) -> Response {
    let context = build_request_context(&request_id, auth, &headers);
    let result = async {
        authorize(&state, &context, Action::Read).await?;
        let filter = DataflowFilter {
            status: query.status,
            effective_status: query.effective_status,
            search: crate::contexts::shared::list_query::search_pattern(query.search.as_deref()),
            source: filter_endpoint(
                query.source_type,
                query.source_id,
                query.source_tag_object_type,
                "source",
            )?,
            destination: filter_endpoint(
                query.destination_type,
                query.destination_id,
                query.destination_tag_object_type,
                "destination",
            )?,
            application_id: query.application_id,
            group_id: query.group_id,
            transport: query.transport,
            tag_id: query.tag_id,
        };
        let (flows, total) = state
            .repositories
            .dataflows
            .list(&filter, query.limit, query.offset)
            .await?;
        Ok::<_, AppError>(InventoryListEnvelope {
            items: project_page(&state, flows).await?,
            total,
            limit: clamp_limit(query.limit),
            offset: clamp_offset(query.offset),
        })
    }
    .await;
    match result {
        Ok(body) => Json(body).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Get a dataflow.
#[utoipa::path(get, path = "/api/v1/inventory/dataflows/{id}", tag = "inventory", params(("id" = Uuid, Path)), responses((status = 200, description = "Successful response", body = DataflowResponse), (status = "4XX", description = "Client error", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", description = "Server error", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn get_dataflow(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let context = build_request_context(&request_id, auth, &headers);
    let result = async {
        authorize(&state, &context, Action::Read).await?;
        let flow = state
            .repositories
            .dataflows
            .get(id)
            .await?
            .ok_or_else(|| AppError::NotFound("Dataflow not found".into()))?;
        project(&state, flow).await
    }
    .await;
    match result {
        Ok(body) => Json(body).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Create a dataflow.
#[utoipa::path(post, path = "/api/v1/inventory/dataflows", tag = "inventory", request_body = DataflowCreatePayload, responses((status = 201, description = "Resource created", body = DataflowResponse), (status = "4XX", description = "Client error", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", description = "Server error", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn create_dataflow(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<DataflowCreatePayload>,
) -> Response {
    let context = build_request_context(&request_id, Some(auth), &headers);
    let result = async {
        authorize(&state, &context, Action::Create).await?;
        let outcome = validate_and_split_custom_fields(
            &state,
            DATAFLOW_CONTENT_TYPE,
            Some(payload.custom_field_data),
            true,
        )
        .await?
        .ok_or_else(|| AppError::Internal("Custom-field split returned no outcome".into()))?;
        let intents = outcome.secret_intents;
        let flow = Dataflow {
            id: Uuid::now_v7(),
            name: payload.name.unwrap_or_default(),
            description: payload.description.unwrap_or_default(),
            comments: payload.comments,
            status: payload.status,
            sources: payload.sources,
            destinations: payload.destinations,
            transport: payload.transport,
            destination_ports: payload.destination_ports,
            source_ports: payload.source_ports,
            icmp_types: payload.icmp_types,
            allow_cross_vrf: payload.allow_cross_vrf,
            group_id: payload.group_id,
            application_id: payload.application_id,
            owner_id: payload.owner_id,
            tenant_id: payload.tenant_id,
            tags: payload.tags,
            custom_field_data: outcome.custom_field_data,
            effective_status: payload.status,
            blocked_by_group_id: None,
            inherited_tags: Vec::new(),
            effective_tags: Vec::new(),
        };
        let created = state.repositories.dataflows.create(&flow).await?;
        apply_custom_field_secret_intents(&state, created.id, intents).await?;
        project(&state, created).await
    }
    .await;
    match result {
        Ok(body) => (StatusCode::CREATED, Json(body)).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update a dataflow.
#[utoipa::path(put, path = "/api/v1/inventory/dataflows/{id}", tag = "inventory", params(("id" = Uuid, Path)), request_body = DataflowUpdatePayload, responses((status = 200, description = "Successful response", body = DataflowResponse), (status = "4XX", description = "Client error", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", description = "Server error", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn update_dataflow(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    EnvelopeJson(payload): EnvelopeJson<DataflowUpdatePayload>,
) -> Response {
    let context = build_request_context(&request_id, Some(auth), &headers);
    let result = async {
        authorize(&state, &context, Action::Update).await?;
        let (custom_field_data, intents) = match validate_and_split_custom_fields(
            &state,
            DATAFLOW_CONTENT_TYPE,
            payload.custom_field_data,
            false,
        )
        .await?
        {
            Some(outcome) => (Some(outcome.custom_field_data), outcome.secret_intents),
            None => (None, Vec::new()),
        };
        let patch = DataflowPatch {
            name: payload.name,
            description: payload.description,
            comments: payload.comments,
            status: payload.status,
            sources: payload.sources,
            destinations: payload.destinations,
            transport: payload.transport,
            destination_ports: payload.destination_ports,
            source_ports: payload.source_ports,
            icmp_types: payload.icmp_types,
            allow_cross_vrf: payload.allow_cross_vrf,
            group_id: payload.group_id,
            application_id: payload.application_id,
            owner_id: payload.owner_id,
            tenant_id: payload.tenant_id,
            tags: payload.tags,
            custom_field_data,
        };
        let updated = state.repositories.dataflows.update(id, patch).await?;
        apply_custom_field_secret_intents(&state, updated.id, intents).await?;
        project(&state, updated).await
    }
    .await;
    match result {
        Ok(body) => Json(body).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a dataflow.
#[utoipa::path(delete, path = "/api/v1/inventory/dataflows/{id}", tag = "inventory", params(("id" = Uuid, Path)), responses((status = 204, description = "Resource deleted"), (status = "4XX", description = "Client error", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", description = "Server error", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn delete_dataflow(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let context = build_request_context(&request_id, Some(auth), &headers);
    let result = async {
        authorize(&state, &context, Action::Delete).await?;
        state.repositories.dataflows.delete(id).await
    }
    .await;
    match result {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Atomic bulk status, move or delete over a selected flow set.
#[utoipa::path(post, path = "/api/v1/inventory/dataflows/bulk-action", tag = "inventory", request_body = DataflowBulkActionPayload, responses((status = 200, description = "Successful response", body = DataflowBulkActionResponse), (status = "4XX", description = "Client error", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", description = "Server error", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn bulk_action_dataflows(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<DataflowBulkActionPayload>,
) -> Response {
    let context = build_request_context(&request_id, Some(auth), &headers);
    let result = async {
        if payload.ids.is_empty() || payload.ids.len() > 1000 {
            return Err(AppError::Validation(
                "ids must contain 1..1000 dataflows".into(),
            ));
        }
        let action = match payload.action {
            DataflowBulkKind::SetStatus => {
                if payload.group_id.is_some() || payload.application_id.is_some() {
                    return Err(AppError::Validation(
                        "set_status accepts only the status field".into(),
                    ));
                }
                BulkDataflowAction::SetStatus(
                    payload
                        .status
                        .ok_or_else(|| AppError::Validation("set_status requires status".into()))?,
                )
            }
            DataflowBulkKind::Move => {
                if payload.status.is_some() {
                    return Err(AppError::Validation("move does not accept status".into()));
                }
                if payload.group_id.is_none() && payload.application_id.is_none() {
                    return Err(AppError::Validation(
                        "move requires group_id or application_id".into(),
                    ));
                }
                BulkDataflowAction::Move {
                    group_id: payload.group_id,
                    application_id: payload.application_id,
                }
            }
            DataflowBulkKind::Delete => {
                if payload.status.is_some()
                    || payload.group_id.is_some()
                    || payload.application_id.is_some()
                {
                    return Err(AppError::Validation(
                        "delete accepts no other action fields".into(),
                    ));
                }
                BulkDataflowAction::Delete
            }
        };
        let permission = if action.requires_delete_permission() {
            Action::Delete
        } else {
            Action::Update
        };
        authorize(&state, &context, permission).await?;
        let affected = state
            .repositories
            .dataflows
            .bulk_action(&payload.ids, &action)
            .await?;
        Ok::<_, AppError>(DataflowBulkActionResponse {
            action: payload.action,
            affected: affected
                .into_iter()
                .map(|id| DataflowBulkAffected { id })
                .collect(),
        })
    }
    .await;
    match result {
        Ok(body) => Json(body).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Get the dataflow firewall policy for a host.
#[utoipa::path(get, path = "/api/v1/inventory/dataflows/firewall-policy", tag = "inventory", params(DataflowPolicyQuery), responses((status = 200, description = "Successful response", body = DataflowPolicyResponse), (status = "4XX", description = "Client error", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", description = "Server error", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn dataflow_firewall_policy(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<DataflowPolicyQuery>,
) -> Response {
    let context = build_request_context(&request_id, auth, &headers);
    let result = async {
        authorize(&state, &context, Action::Read).await?;
        // A playbook job token has no user identity: it may read the policy only
        // for the concrete host its run is bound to, and only while that run's
        // live initiator still holds the host and peer visibility.
        let visibility = if context.playbook_job_binding.is_some() {
            resolve_job_bound_host_visibility(&state, &context, query.host_type, query.host_id)
                .await?
        } else {
            resolve_inventory_connection_target_visibility(&state, &context).await?
        };
        if matches!(visibility, ConnectionTargetVisibility::None) {
            return Err(AppError::Forbidden(
                "Host policy requires connection-target row visibility".into(),
            ));
        }
        let snapshot = state
            .repositories
            .dataflows
            .policy_snapshot(DataflowEndpoint {
                object_type: query.host_type,
                object_id: query.host_id,
            })
            .await?;
        if let ConnectionTargetVisibility::Restricted(predicate) = visibility
            && snapshot.required_hosts().iter().any(|host| {
                !snapshot.authorization_facets.contains_key(host)
                    || !batch_connection_target_is_allowed(
                        &predicate,
                        host.object_id,
                        snapshot.authorization_facets.get(host),
                    )
            })
        {
            return Err(AppError::Forbidden(
                "Host policy includes a target outside connection-target visibility".into(),
            ));
        }
        Ok::<_, AppError>(resolve_host_policy(&snapshot))
    }
    .await;
    match result {
        Ok(body) => Json(body).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Resolve a dataflow endpoint reference.
#[utoipa::path(post, path = "/api/v1/inventory/dataflows/endpoint-resolution", tag = "inventory", request_body = DataflowEndpointInput, responses((status = 200, description = "Successful response", body = DataflowEndpointResolutionResponse), (status = "4XX", description = "Client error", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", description = "Server error", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn dataflow_endpoint_resolution(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    EnvelopeJson(selector): EnvelopeJson<DataflowEndpointInput>,
) -> Response {
    let context = build_request_context(&request_id, auth, &headers);
    let result = async {
        authorize(&state, &context, Action::Read).await?;
        state
            .repositories
            .dataflows
            .endpoint_resolution(selector)
            .await
    }
    .await;
    match result {
        Ok(body) => Json(body).into_response(),
        Err(error) => handle_app_error(error),
    }
}
