//! Flow-owned selectors, directed intent CRUD and read-only host policy.
use super::super::{InventoryListEnvelope, ensure_access};
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
                DataflowFilter, DataflowPatch,
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
use uuid::Uuid;

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DataflowCreatePayload {
    pub name: Option<String>,
    pub description: Option<String>,
    #[serde(default)]
    #[schema(default = DataflowStatus::default)]
    pub status: DataflowStatus,
    pub source: DataflowEndpointInput,
    pub destination: DataflowEndpointInput,
    #[serde(default)]
    #[schema(default = DataflowTransport::default)]
    pub transport: DataflowTransport,
    /// null/omission means any ports; [] is invalid.
    pub destination_ports: Option<Vec<i32>>,
    #[serde(default)]
    pub allow_cross_vrf: bool,
}
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DataflowUpdatePayload {
    pub name: Option<String>,
    pub description: Option<String>,
    pub status: Option<DataflowStatus>,
    pub source: Option<DataflowEndpointInput>,
    pub destination: Option<DataflowEndpointInput>,
    pub transport: Option<DataflowTransport>,
    #[serde(default, deserialize_with = "nullable_ports")]
    pub destination_ports: Option<Option<Vec<i32>>>,
    pub allow_cross_vrf: Option<bool>,
}
fn nullable_ports<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Option<Vec<i32>>>, D::Error> {
    Option::<Vec<i32>>::deserialize(deserializer).map(Some)
}
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DataflowResponse {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub status: DataflowStatus,
    pub source: DataflowEndpointResponse,
    pub destination: DataflowEndpointResponse,
    pub transport: DataflowTransport,
    pub destination_ports: Option<Vec<i32>>,
    pub allow_cross_vrf: bool,
}
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct DataflowListQuery {
    pub status: Option<DataflowStatus>,
    pub search: Option<String>,
    pub source_type: Option<EndpointType>,
    pub source_id: Option<Uuid>,
    pub destination_type: Option<EndpointType>,
    pub destination_id: Option<Uuid>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct DataflowPolicyQuery {
    pub host_type: EndpointType,
    pub host_id: Uuid,
}
fn filter_endpoint(
    kind: Option<EndpointType>,
    id: Option<Uuid>,
) -> Result<Option<DataflowEndpointInput>, AppError> {
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
    };
    selector.normalize()?;
    Ok(Some(selector))
}
async fn project_page(
    state: &AppState,
    flows: Vec<Dataflow>,
) -> Result<Vec<DataflowResponse>, AppError> {
    let refs = state.repositories.dataflows.projections(&flows).await?;
    Ok(flows
        .into_iter()
        .zip(refs)
        .map(|(flow, (source, destination))| DataflowResponse {
            id: flow.id,
            name: flow.name,
            description: flow.description,
            status: flow.status,
            source,
            destination,
            transport: flow.transport,
            destination_ports: flow.destination_ports,
            allow_cross_vrf: flow.allow_cross_vrf,
        })
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
#[utoipa::path(get, path = "/api/v1/inventory/dataflows", tag = "inventory", params(DataflowListQuery), responses((status = 200, body = InventoryListEnvelope<DataflowResponse>), (status = "4XX", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
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
            search: crate::contexts::shared::list_query::search_pattern(query.search.as_deref()),
            source: filter_endpoint(query.source_type, query.source_id)?,
            destination: filter_endpoint(query.destination_type, query.destination_id)?,
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
#[utoipa::path(get, path = "/api/v1/inventory/dataflows/{id}", tag = "inventory", params(("id" = Uuid, Path)), responses((status = 200, body = DataflowResponse), (status = "4XX", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
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
#[utoipa::path(post, path = "/api/v1/inventory/dataflows", tag = "inventory", request_body = DataflowCreatePayload, responses((status = 201, body = DataflowResponse), (status = "4XX", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
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
        let flow = Dataflow {
            id: Uuid::now_v7(),
            name: payload.name.unwrap_or_default(),
            description: payload.description.unwrap_or_default(),
            status: payload.status,
            source: payload.source,
            destination: payload.destination,
            transport: payload.transport,
            destination_ports: payload.destination_ports,
            allow_cross_vrf: payload.allow_cross_vrf,
        };
        project(&state, state.repositories.dataflows.create(&flow).await?).await
    }
    .await;
    match result {
        Ok(body) => (StatusCode::CREATED, Json(body)).into_response(),
        Err(error) => handle_app_error(error),
    }
}
/// Update a dataflow.
#[utoipa::path(put, path = "/api/v1/inventory/dataflows/{id}", tag = "inventory", params(("id" = Uuid, Path)), request_body = DataflowUpdatePayload, responses((status = 200, body = DataflowResponse), (status = "4XX", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
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
        let patch = DataflowPatch {
            name: payload.name,
            description: payload.description,
            status: payload.status,
            source: payload.source,
            destination: payload.destination,
            transport: payload.transport,
            destination_ports: payload.destination_ports,
            allow_cross_vrf: payload.allow_cross_vrf,
        };
        project(
            &state,
            state.repositories.dataflows.update(id, patch).await?,
        )
        .await
    }
    .await;
    match result {
        Ok(body) => Json(body).into_response(),
        Err(error) => handle_app_error(error),
    }
}
/// Delete a dataflow.
#[utoipa::path(delete, path = "/api/v1/inventory/dataflows/{id}", tag = "inventory", params(("id" = Uuid, Path)), responses((status = 204), (status = "4XX", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
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
/// Get the dataflow firewall policy for a host.
#[utoipa::path(get, path = "/api/v1/inventory/dataflows/firewall-policy", tag = "inventory", params(DataflowPolicyQuery), responses((status = 200, body = DataflowPolicyResponse), (status = "4XX", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
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
        let visibility = resolve_inventory_connection_target_visibility(&state, &context).await?;
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
#[utoipa::path(post, path = "/api/v1/inventory/dataflows/endpoint-resolution", tag = "inventory", request_body = DataflowEndpointInput, responses((status = 200, body = DataflowEndpointResolutionResponse), (status = "4XX", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
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
