//! REST routes for the connection-config context (`connconf_*`).
//!
//! Exposes the connection-config domain over REST: CRUD for `connconf_record`
//! and `connconf_assignment`, plus an effective-config resolver endpoint for a
//! device/VM. Handlers are SQL-free — they call the context repositories and the
//! pure `resolve_effective_config` resolver. Authz reuses
//! `ResourceType::ConnectionSession` Read/Write.

use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use uuid::Uuid;

use crate::api::access_control_helpers::build_policy_evaluation_service;
use crate::api::connection_authz_attributes::connection_target_authz_attributes_for;
use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::api::rest::routes::validation::{parse_uuid_or_blank, require_bulk_items};
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::audit::domain::AuditEvent;
use crate::contexts::connection_config::{
    CONNCONF_ASSIGNMENT_COLUMNS, CONNCONF_RECORD_COLUMNS, ConnConfAssignment,
    ConnConfAssignmentFilter, ConnConfAssignmentRepository, ConnConfProtocol, ConnConfRecord,
    ConnConfRecordFilter, ConnConfRecordRepository, ConnConfScope, DeviceScopeIds,
    EffectiveConnConfig, NewConnConfAssignment, NewConnConfRecord, ResolveInput,
    UpdateConnConfAssignment, UpdateConnConfRecord, resolve_effective_config,
};
use crate::contexts::secrets::application::resolve_secret_visibility;
use crate::contexts::shared::list_query::{SortSpec, parse_csv_strings, parse_csv_uuids};
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;
use chrono::Utc;

// ----------------------------------------------------------------------------
// Record DTOs
// ----------------------------------------------------------------------------

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ConnConfRecordResponse {
    pub id: String,
    pub name: String,
    pub description: String,
    pub protocol: String,
    pub base_record_id: Option<String>,
    pub enabled: bool,
    pub owner_user_id: Option<String>,
    pub params: Value,
    pub custom_field_data: Value,
    pub created_at: String,
    pub updated_at: String,
}

impl From<ConnConfRecord> for ConnConfRecordResponse {
    fn from(r: ConnConfRecord) -> Self {
        Self {
            id: r.id.to_string(),
            name: r.name,
            description: r.description,
            protocol: r.protocol.as_str().to_string(),
            base_record_id: r.base_record_id.map(|u| u.to_string()),
            enabled: r.enabled,
            owner_user_id: r.owner_user_id.map(|u| u.to_string()),
            params: r.params,
            custom_field_data: r.custom_field_data,
            created_at: r.created_at.to_rfc3339(),
            updated_at: r.updated_at.to_rfc3339(),
        }
    }
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ConnConfRecordListResponse {
    pub items: Vec<ConnConfRecordResponse>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct ConnConfRecordListQuery {
    #[serde(default)]
    pub protocol: Option<String>,
    #[serde(default)]
    pub base_record_id: Option<String>,
    #[serde(default)]
    pub owner_user_id: Option<String>,
    #[serde(default)]
    pub enabled: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub offset: Option<i64>,
    #[serde(default)]
    pub sort: Option<String>,
    #[serde(default)]
    pub order: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateConnConfRecordRequest {
    pub name: String,
    pub protocol: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub base_record_id: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub owner_user_id: Option<String>,
    #[serde(default)]
    pub params: Option<Value>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateConnConfRecordRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub protocol: Option<String>,
    /// Outer `Some` = caller set the field; inner `null` clears the base.
    #[serde(default)]
    pub base_record_id: Option<Option<String>>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub params: Option<Value>,
}

// ----------------------------------------------------------------------------
// Assignment DTOs
// ----------------------------------------------------------------------------

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ConnConfAssignmentResponse {
    pub id: String,
    pub record_id: String,
    pub scope_type: String,
    pub scope_id: Option<String>,
    pub weight: i32,
    pub enabled: bool,
    pub created_at: String,
}

impl From<ConnConfAssignment> for ConnConfAssignmentResponse {
    fn from(a: ConnConfAssignment) -> Self {
        Self {
            id: a.id.to_string(),
            record_id: a.record_id.to_string(),
            scope_type: a.scope_type.as_str().to_string(),
            scope_id: a.scope_id.map(|u| u.to_string()),
            weight: a.weight,
            enabled: a.enabled,
            created_at: a.created_at.to_rfc3339(),
        }
    }
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ConnConfAssignmentListResponse {
    pub items: Vec<ConnConfAssignmentResponse>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct ConnConfAssignmentListQuery {
    #[serde(default)]
    pub record_id: Option<String>,
    #[serde(default)]
    pub scope_type: Option<String>,
    #[serde(default)]
    pub scope_id: Option<String>,
    #[serde(default)]
    pub enabled: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub offset: Option<i64>,
    #[serde(default)]
    pub sort: Option<String>,
    #[serde(default)]
    pub order: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateConnConfAssignmentRequest {
    pub record_id: String,
    pub scope_type: String,
    #[serde(default)]
    pub scope_id: Option<String>,
    #[serde(default)]
    pub weight: Option<i32>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateConnConfAssignmentRequest {
    #[serde(default)]
    pub weight: Option<i32>,
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Optional scope retarget. When omitted, the existing scope is preserved.
    /// When present, `scope_type`/`scope_id` follow the create-time rule:
    /// `global` carries no `scope_id`, every other scope requires one.
    #[serde(default)]
    pub scope_type: Option<String>,
    #[serde(default)]
    pub scope_id: Option<String>,
}

// ----------------------------------------------------------------------------
// Effective-config DTOs
// ----------------------------------------------------------------------------
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct EffectiveConnConfigSourceResponse {
    pub record_id: String,
    pub record_name: String,
    pub scope_type: Option<String>,
    pub scope_id: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct EffectiveConnConfigResponse {
    pub protocol: String,
    pub params: Value,
    /// Dotted leaf path -> winning source attribution.
    pub attribution: std::collections::BTreeMap<String, EffectiveConnConfigSourceResponse>,
    /// Ordered record ids that participated, least -> most specific.
    pub resolution_chain: Vec<String>,
}

impl From<EffectiveConnConfig> for EffectiveConnConfigResponse {
    fn from(e: EffectiveConnConfig) -> Self {
        Self {
            protocol: e.protocol.as_str().to_string(),
            params: e.params,
            attribution: e
                .attribution
                .into_iter()
                .map(|(path, source)| {
                    (
                        path,
                        EffectiveConnConfigSourceResponse {
                            record_id: source.record_id.to_string(),
                            record_name: source.record_name,
                            scope_type: source.scope_type.map(|s| s.as_str().to_string()),
                            scope_id: source.scope_id.map(|u| u.to_string()),
                        },
                    )
                })
                .collect(),
            resolution_chain: e.resolution_chain.iter().map(Uuid::to_string).collect(),
        }
    }
}

#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct EffectiveConnConfigQuery {
    #[serde(default)]
    pub protocol: Option<String>,
}

// ----------------------------------------------------------------------------
// Bulk-assign DTOs
// ----------------------------------------------------------------------------

/// Hard cap on how many targets one bulk-assign request may carry. Bounds the
/// endpoint to a fixed number of sequential DB round-trips per call instead of
/// an open-ended request.
const MAX_BULK_ASSIGN_TARGETS: usize = 1000;

/// One target of a bulk-assign request.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct BulkAssignConnConfTarget {
    /// Device or VM id to assign the record to.
    pub target_id: String,
    /// Per-target weight override. Falls back to the request-level `weight`,
    /// then to the default weight, when omitted.
    #[serde(default)]
    pub weight: Option<i32>,
}

/// Bulk-assign a connection-config record to many device/VM targets in one
/// call. Assigning one record to several targets makes it their shared
/// credential source; a target already assigned to the record is reported
/// `already_assigned` and never re-inserted.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct BulkAssignConnConfRequest {
    #[schema(min_items = 1)]
    pub targets: Vec<BulkAssignConnConfTarget>,
    /// Fallback weight applied to any target that omits its own.
    #[serde(default)]
    pub weight: Option<i32>,
}

impl BulkAssignConnConfRequest {
    /// A bulk-assign must name at least one target; an empty batch would answer
    /// `200` while assigning nothing, so it is a 400 checked before any I/O.
    pub(crate) fn validate(&self) -> Result<(), AppError> {
        require_bulk_items(&self.targets)?;
        Ok(())
    }
}

/// Per-target outcome of a bulk-assign call.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkAssignConnConfOutcomeItem {
    pub target_id: String,
    /// One of `assigned`, `already_assigned`, `failed`.
    pub status: String,
    pub assignment: Option<ConnConfAssignmentResponse>,
    pub error: Option<String>,
}

/// The result of a bulk-assign call across every requested target. A single
/// target's failure is captured in its own item and never aborts the rest
/// of the batch.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkAssignConnConfResponse {
    pub items: Vec<BulkAssignConnConfOutcomeItem>,
    pub assigned_count: i64,
    pub already_assigned_count: i64,
    pub failed_count: i64,
}

// ----------------------------------------------------------------------------
// Helpers
// ----------------------------------------------------------------------------

fn parse_required_uuid(value: &str, field: &str) -> Result<Uuid, AppError> {
    Uuid::parse_str(value).map_err(|e| AppError::Validation(format!("{field}: {e}")))
}

fn parse_protocol(raw: &str) -> Result<ConnConfProtocol, AppError> {
    ConnConfProtocol::parse(raw)
        .ok_or_else(|| AppError::Validation(format!("unknown protocol '{raw}'")))
}

fn parse_protocol_list(raw: Option<&str>) -> Result<Vec<ConnConfProtocol>, AppError> {
    parse_csv_strings(raw)
        .into_iter()
        .map(|value| parse_protocol(&value))
        .collect()
}

async fn ensure_base_record_matches_protocol(
    state: &AppState,
    record_id: Option<Uuid>,
    base_record_id: Option<Uuid>,
    protocol: ConnConfProtocol,
) -> Result<(), AppError> {
    let Some(base_id) = base_record_id else {
        return Ok(());
    };
    if Some(base_id) == record_id {
        return Err(AppError::Validation(
            "base_record_id cannot point to the same record".to_string(),
        ));
    }

    let Some(base_record) = state.repositories.connconf_records.find(base_id).await? else {
        return Err(AppError::NotFound(
            "base connconf_record not found".to_string(),
        ));
    };
    if base_record.protocol != protocol {
        return Err(AppError::Validation(format!(
            "base_record_id protocol '{}' does not match record protocol '{}'",
            base_record.protocol.as_str(),
            protocol.as_str()
        )));
    }
    Ok(())
}

fn validate_protocol_change_does_not_break_children(
    current_protocol: ConnConfProtocol,
    requested_protocol: Option<ConnConfProtocol>,
    has_direct_children: bool,
) -> Result<(), AppError> {
    let Some(next_protocol) = requested_protocol else {
        return Ok(());
    };
    if next_protocol != current_protocol && has_direct_children {
        return Err(AppError::Validation(
            "Cannot change protocol while this connection-config record is used as a template parent"
                .to_string(),
        ));
    }
    Ok(())
}

fn parse_optional_protocol(value: Option<&str>) -> Result<Option<ConnConfProtocol>, AppError> {
    value.map(parse_protocol).transpose()
}

fn parse_optional_base_record_id(
    value: Option<Option<String>>,
) -> Result<Option<Option<Uuid>>, AppError> {
    value
        .map(|inner| parse_uuid_or_blank(inner, "base_record_id"))
        .transpose()
}

/// Load the record's current protocol when the update touches the protocol or
/// its base, and reject a protocol change that would orphan direct children.
async fn validate_protocol_update(
    state: &AppState,
    id: Uuid,
    protocol: Option<ConnConfProtocol>,
    base_record_id: Option<Option<Uuid>>,
) -> Result<Option<ConnConfProtocol>, AppError> {
    if protocol.is_none() && !matches!(base_record_id, Some(Some(_))) {
        return Ok(None);
    }
    let Some(record) = state.repositories.connconf_records.find(id).await? else {
        return Err(AppError::NotFound("connconf_record not found".to_string()));
    };
    let current_protocol = record.protocol;
    let has_direct_children = if protocol.is_some() {
        state
            .repositories
            .connconf_records
            .has_direct_children(id)
            .await?
    } else {
        false
    };
    validate_protocol_change_does_not_break_children(
        current_protocol,
        protocol,
        has_direct_children,
    )?;
    Ok(Some(current_protocol))
}

/// Reject a base record whose protocol does not match the effective protocol.
async fn ensure_effective_base_record(
    state: &AppState,
    id: Uuid,
    effective_protocol: Option<ConnConfProtocol>,
    base_record_id: Option<Option<Uuid>>,
) -> Result<(), AppError> {
    if let (Some(protocol), Some(Some(base_id))) = (effective_protocol, base_record_id) {
        ensure_base_record_matches_protocol(state, Some(id), Some(base_id), protocol).await?;
    }
    Ok(())
}

fn parse_scope(raw: &str) -> Result<ConnConfScope, AppError> {
    ConnConfScope::parse(raw)
        .ok_or_else(|| AppError::Validation(format!("unknown scope_type '{raw}'")))
}

fn parse_scope_list(raw: Option<&str>) -> Result<Vec<ConnConfScope>, AppError> {
    parse_csv_strings(raw)
        .into_iter()
        .map(|value| parse_scope(&value))
        .collect()
}

/// Reads of connection-config records/assignments are an administrative config
/// surface, gated like other config admin routes (`ResourceType::ConfigContext`)
/// rather than the generic `ConnectionSession` capability.
async fn ensure_config_read(
    state: &AppState,
    request_context: &RequestContext,
) -> Result<(), AppError> {
    build_policy_evaluation_service(state)
        .ensure_access(
            request_context,
            Action::Read,
            ResourceType::ConfigContext,
            None,
        )
        .await
}

/// Record/assignment mutations require an admin-level config capability — they
/// edit globally-scoped connection config and secret bindings, so they must NOT
/// be reachable with a generic `ConnectionSession:Write`. Mirrors the
/// create/update/delete gating of `config_contexts` admin routes.
async fn ensure_config_mutation(
    state: &AppState,
    request_context: &RequestContext,
    action: Action,
) -> Result<(), AppError> {
    build_policy_evaluation_service(state)
        .ensure_access(request_context, action, ResourceType::ConfigContext, None)
        .await
}

/// The effective-config endpoint resolves connect-time config (incl. secret
/// references) for a SPECIFIC device/VM. Authorize against that target host the
/// same way the connect path does (`Action::Connect`/`Action::Secret` +
/// `ResourceType::ConnectionTarget` against the target id), not a global capability.
async fn ensure_effective_config_access(
    state: &AppState,
    request_context: &RequestContext,
    device_id: Uuid,
    protocol: &str,
) -> Result<(), AppError> {
    let policy_service = build_policy_evaluation_service(state);
    // Canonical `connection_target:{id}` form so resource-scoped DENY policies
    // match (a bare id silently skips prefixed patterns). Attributes (facets +
    // the REQUESTED protocol, same default as the config resolution below) let
    // attribute-constrained Connect/Secret policies apply here exactly as they
    // do at the connect-create site.
    let resource_id = format!("connection_target:{device_id}");
    let attributes =
        connection_target_authz_attributes_for(state, device_id, Some(protocol)).await?;
    policy_service
        .ensure_access_with_attributes(
            request_context,
            Action::Connect,
            ResourceType::ConnectionTarget,
            Some(resource_id.as_str()),
            Some(attributes.clone()),
        )
        .await?;
    policy_service
        .ensure_access_with_attributes(
            request_context,
            Action::Secret,
            ResourceType::ConnectionTarget,
            Some(resource_id.as_str()),
            Some(attributes),
        )
        .await
}

// ----------------------------------------------------------------------------
// Record handlers
// ----------------------------------------------------------------------------

/// List connection-config records.
#[utoipa::path(
    get,
    path = "/api/v1/connection-config/records",
    tag = "connection-config",
    params(ConnConfRecordListQuery),
    operation_id = "list_connconf_records",
    responses(
        (status = 200, description = "Connection-config records", body = ConnConfRecordListResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_connconf_records(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ConnConfRecordListQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_config_read(&state, &request_context).await {
        return handle_app_error(error);
    }

    let protocol = match parse_protocol_list(query.protocol.as_deref()) {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };
    let base_record_id = match parse_csv_uuids(query.base_record_id.as_deref(), "base_record_id") {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };
    let owner_user_id = match parse_csv_uuids(query.owner_user_id.as_deref(), "owner_user_id") {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        CONNCONF_RECORD_COLUMNS,
    ) {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };

    match state
        .repositories
        .connconf_records
        .list(ConnConfRecordFilter {
            protocol,
            base_record_id,
            owner_user_id,
            enabled: parse_csv_strings(query.enabled.as_deref()),
            sort,
        })
        .await
    {
        Ok(records) => {
            let total = records.len() as i64;
            let items: Vec<ConnConfRecordResponse> = records.into_iter().map(Into::into).collect();
            (
                StatusCode::OK,
                Json(ConnConfRecordListResponse {
                    items,
                    total,
                    limit: query.limit.unwrap_or(total),
                    offset: query.offset.unwrap_or(0),
                }),
            )
                .into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Get a connection-config record.
#[utoipa::path(
    get,
    path = "/api/v1/connection-config/records/{id}",
    tag = "connection-config",
    params(("id" = String, Path, description = "Record id")),
    operation_id = "get_connconf_record",
    responses(
        (status = 200, description = "Connection-config record", body = ConnConfRecordResponse),
        (status = 404, description = "Record not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_connconf_record(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_config_read(&state, &request_context).await {
        return handle_app_error(error);
    }

    let id = match parse_required_uuid(&id, "id") {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };

    match state.repositories.connconf_records.find(id).await {
        Ok(Some(record)) => {
            (StatusCode::OK, Json(ConnConfRecordResponse::from(record))).into_response()
        }
        Ok(None) => handle_app_error(AppError::NotFound("connconf_record not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a connection-config record.
#[utoipa::path(
    post,
    path = "/api/v1/connection-config/records",
    tag = "connection-config",
    request_body = CreateConnConfRecordRequest,
    operation_id = "create_connconf_record",
    responses(
        (status = 201, description = "Record created", body = ConnConfRecordResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_connconf_record(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<CreateConnConfRecordRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_config_mutation(&state, &request_context, Action::Create).await {
        return handle_app_error(error);
    }

    let protocol = match parse_protocol(&payload.protocol) {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };
    let base_record_id = match parse_uuid_or_blank(payload.base_record_id, "base_record_id") {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };
    let owner_user_id = match parse_uuid_or_blank(payload.owner_user_id, "owner_user_id") {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };
    if let Err(error) =
        ensure_base_record_matches_protocol(&state, None, base_record_id, protocol).await
    {
        return handle_app_error(error);
    }

    let input = NewConnConfRecord {
        name: payload.name,
        description: payload.description,
        protocol,
        base_record_id,
        enabled: payload.enabled,
        owner_user_id,
        params: payload.params,
    };

    match state.repositories.connconf_records.create(&input).await {
        Ok(record) => (
            StatusCode::CREATED,
            Json(ConnConfRecordResponse::from(record)),
        )
            .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update a connection-config record.
#[utoipa::path(
    put,
    path = "/api/v1/connection-config/records/{id}",
    tag = "connection-config",
    params(("id" = String, Path, description = "Record id")),
    request_body = UpdateConnConfRecordRequest,
    operation_id = "update_connconf_record",
    responses(
        (status = 200, description = "Record updated", body = ConnConfRecordResponse),
        (status = 404, description = "Record not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_connconf_record(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<UpdateConnConfRecordRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_config_mutation(&state, &request_context, Action::Update).await {
        return handle_app_error(error);
    }

    let id = match parse_required_uuid(&id, "id") {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };

    let base_record_id = match parse_optional_base_record_id(payload.base_record_id) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let protocol = match parse_optional_protocol(payload.protocol.as_deref()) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let current_record = match validate_protocol_update(&state, id, protocol, base_record_id).await
    {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let protocol_changed =
        matches!((current_record, protocol), (Some(current), Some(next)) if next != current);
    let base_record_id = if protocol_changed && base_record_id.is_none() {
        Some(None)
    } else {
        base_record_id
    };
    let effective_protocol = protocol.or(current_record);
    if let Err(error) =
        ensure_effective_base_record(&state, id, effective_protocol, base_record_id).await
    {
        return handle_app_error(error);
    }

    let input = UpdateConnConfRecord {
        name: payload.name,
        description: payload.description,
        protocol,
        base_record_id,
        enabled: payload.enabled,
        params: payload.params,
    };

    match state.repositories.connconf_records.update(id, &input).await {
        Ok(Some(record)) => {
            (StatusCode::OK, Json(ConnConfRecordResponse::from(record))).into_response()
        }
        Ok(None) => handle_app_error(AppError::NotFound("connconf_record not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a connection-config record.
#[utoipa::path(
    delete,
    path = "/api/v1/connection-config/records/{id}",
    tag = "connection-config",
    params(("id" = String, Path, description = "Record id")),
    operation_id = "delete_connconf_record",
    responses(
        (status = 204, description = "Record deleted"),
        (status = 404, description = "Record not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 409, description = "Record is used as a base by other records (DELETE_BLOCKED with blocker tables, counts, and examples)", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_connconf_record(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_config_mutation(&state, &request_context, Action::Delete).await {
        return handle_app_error(error);
    }

    let id = match parse_required_uuid(&id, "id") {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };

    match state.repositories.connconf_records.delete(id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => handle_app_error(AppError::NotFound("connconf_record not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

// ----------------------------------------------------------------------------
// Credential reveal DTOs + handler
// ----------------------------------------------------------------------------

/// One credential field of a record in the reveal response. `is_secret_ref`
/// reflects whether the field is backed by a named secret; `value`
/// carries the resolved plaintext ONLY when `is_secret_ref && can_view_secrets`.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ConnConfCredentialField {
    /// Dotted credential path, e.g. `credentials.private_key`.
    pub path: String,
    /// Whether the field references a secret (binding present).
    pub is_secret_ref: bool,
    /// Resolved secret value; present only for secret-ref fields when the caller
    /// may view secrets.
    pub value: Option<String>,
}

/// Reveal of a record's resolved credential references.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ConnConfRecordCredentialsResponse {
    /// Whether the current principal is permitted to view secret values.
    pub can_view_secrets: bool,
    /// The record's secret-reference credential fields. Literal credential
    /// values are not echoed back (the client already holds them).
    pub fields: Vec<ConnConfCredentialField>,
}

/// Reveal a record's resolved credential references.
///
/// Returns each secret-reference credential field of the record, resolving the
/// real secret value only when the caller may view secrets. Separate from
/// `get_connconf_record`, which never carries secrets.
#[utoipa::path(
    get,
    path = "/api/v1/connection-config/records/{id}/credentials",
    tag = "connection-config",
    params(("id" = String, Path, description = "Record id")),
    operation_id = "reveal_connconf_record_credentials",
    responses(
        (status = 200, description = "Record credential references", body = ConnConfRecordCredentialsResponse),
        (status = 404, description = "Record not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn reveal_connconf_record_credentials(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    // Same record-read guard as `get_connconf_record`.
    if let Err(error) = ensure_config_read(&state, &request_context).await {
        return handle_app_error(error);
    }

    let id = match parse_required_uuid(&id, "id") {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };

    // 404 if the record does not exist (mirrors `get_connconf_record`).
    match state.repositories.connconf_records.find(id).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return handle_app_error(AppError::NotFound("connconf_record not found".to_string()));
        }
        Err(error) => return handle_app_error(error),
    }

    // Secret-visibility gate: identical to the effective-config/connect path —
    // `PolicyDecision::Allow` on `check_secret_visibility`.
    let can_view_secrets = match resolve_secret_visibility(
        Some(state.services.access_control.as_ref()),
        Some(&request_context),
    )
    .await
    {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let audit_actor_id = if can_view_secrets {
        match request_context.require_authenticated_user_id() {
            Ok(user_id) => Some(user_id),
            Err(error) => return handle_app_error(error),
        }
    } else {
        None
    };

    // SQL-free handler: binding lookup + value resolution live in the service.
    let resolved = match state
        .services
        .secret_field_states
        .reveal_record_credentials(id, can_view_secrets)
        .await
    {
        Ok(fields) => fields,
        Err(error) => return handle_app_error(error),
    };

    // Mirror `resolve_secrets`: record an audit event when secret values are
    // actually revealed.
    if can_view_secrets && !resolved.is_empty() {
        let keys: Vec<String> = resolved.iter().map(|f| f.field_key.clone()).collect();
        let event = AuditEvent {
            id: Uuid::now_v7(),
            event_type: "secrets.resolve".to_string(),
            actor_id: audit_actor_id,
            actor_kind: request_context.audit_actor_kind().to_string(),
            target_type: Some("connection_config:record".to_string()),
            target_id: Some(id),
            metadata: serde_json::json!({ "keys": keys }),
            created_at: Utc::now(),
        };
        if let Err(error) = state.services.audit.record_event(event).await {
            return handle_app_error(error);
        }
    }

    let fields = resolved
        .into_iter()
        .map(|f| ConnConfCredentialField {
            path: f.field_key,
            is_secret_ref: f.is_secret_ref,
            value: f.value,
        })
        .collect();

    (
        StatusCode::OK,
        Json(ConnConfRecordCredentialsResponse {
            can_view_secrets,
            fields,
        }),
    )
        .into_response()
}

// ----------------------------------------------------------------------------
// Assignment handlers
// ----------------------------------------------------------------------------

/// List connection-config assignments.
#[utoipa::path(
    get,
    path = "/api/v1/connection-config/assignments",
    tag = "connection-config",
    params(ConnConfAssignmentListQuery),
    operation_id = "list_connconf_assignments",
    responses(
        (status = 200, description = "Connection-config assignments", body = ConnConfAssignmentListResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_connconf_assignments(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ConnConfAssignmentListQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_config_read(&state, &request_context).await {
        return handle_app_error(error);
    }

    let record_id = match parse_csv_uuids(query.record_id.as_deref(), "record_id") {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };
    let scope_type = match parse_scope_list(query.scope_type.as_deref()) {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };
    let scope_id = match parse_csv_uuids(query.scope_id.as_deref(), "scope_id") {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        CONNCONF_ASSIGNMENT_COLUMNS,
    ) {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };

    match state
        .repositories
        .connconf_assignments
        .list(ConnConfAssignmentFilter {
            record_id,
            scope_type,
            scope_id,
            enabled: parse_csv_strings(query.enabled.as_deref()),
            sort,
        })
        .await
    {
        Ok(assignments) => {
            let total = assignments.len() as i64;
            let items: Vec<ConnConfAssignmentResponse> =
                assignments.into_iter().map(Into::into).collect();
            (
                StatusCode::OK,
                Json(ConnConfAssignmentListResponse {
                    items,
                    total,
                    limit: query.limit.unwrap_or(total),
                    offset: query.offset.unwrap_or(0),
                }),
            )
                .into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Assign a connection-config record to a scope.
#[utoipa::path(
    post,
    path = "/api/v1/connection-config/assignments",
    tag = "connection-config",
    request_body = CreateConnConfAssignmentRequest,
    operation_id = "create_connconf_assignment",
    responses(
        (status = 201, description = "Assignment created", body = ConnConfAssignmentResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_connconf_assignment(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<CreateConnConfAssignmentRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_config_mutation(&state, &request_context, Action::Create).await {
        return handle_app_error(error);
    }

    let record_id = match parse_required_uuid(&payload.record_id, "record_id") {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };
    let scope_type = match parse_scope(&payload.scope_type) {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };
    let scope_id = match parse_uuid_or_blank(payload.scope_id, "scope_id") {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };

    // Mirror the DB CHECK: global has no target; every other scope requires one.
    if scope_type == ConnConfScope::Global && scope_id.is_some() {
        return handle_app_error(AppError::Validation(
            "global scope must not carry a scope_id".to_string(),
        ));
    }
    if scope_type != ConnConfScope::Global && scope_id.is_none() {
        return handle_app_error(AppError::Validation(format!(
            "scope_type '{}' requires a scope_id",
            scope_type.as_str()
        )));
    }

    let input = NewConnConfAssignment {
        record_id,
        scope_type,
        scope_id,
        weight: payload.weight,
        enabled: payload.enabled,
    };

    match state.repositories.connconf_assignments.create(&input).await {
        Ok(assignment) => (
            StatusCode::CREATED,
            Json(ConnConfAssignmentResponse::from(assignment)),
        )
            .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update a connection-config assignment.
#[utoipa::path(
    put,
    path = "/api/v1/connection-config/assignments/{id}",
    tag = "connection-config",
    params(("id" = String, Path, description = "Assignment id")),
    request_body = UpdateConnConfAssignmentRequest,
    operation_id = "update_connconf_assignment",
    responses(
        (status = 200, description = "Assignment updated", body = ConnConfAssignmentResponse),
        (status = 404, description = "Assignment not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_connconf_assignment(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<UpdateConnConfAssignmentRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_config_mutation(&state, &request_context, Action::Update).await {
        return handle_app_error(error);
    }

    let id = match parse_required_uuid(&id, "id") {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };

    // Optional scope retarget. Only validate/apply when scope_type is present;
    // an absent scope_type leaves the existing scope untouched. The rule mirrors
    // `create_connconf_assignment`: global ⟺ scope_id IS NULL.
    let scope_type = match payload.scope_type.as_deref().map(parse_scope).transpose() {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };
    let scope_id = match parse_uuid_or_blank(payload.scope_id, "scope_id") {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };
    if let Some(scope_type) = scope_type {
        if scope_type == ConnConfScope::Global && scope_id.is_some() {
            return handle_app_error(AppError::Validation(
                "global scope must not carry a scope_id".to_string(),
            ));
        }
        if scope_type != ConnConfScope::Global && scope_id.is_none() {
            return handle_app_error(AppError::Validation(format!(
                "scope_type '{}' requires a scope_id",
                scope_type.as_str()
            )));
        }
    }

    let input = UpdateConnConfAssignment {
        weight: payload.weight,
        enabled: payload.enabled,
        scope_type,
        scope_id,
    };

    match state
        .repositories
        .connconf_assignments
        .update(id, &input)
        .await
    {
        Ok(Some(assignment)) => (
            StatusCode::OK,
            Json(ConnConfAssignmentResponse::from(assignment)),
        )
            .into_response(),
        Ok(None) => handle_app_error(AppError::NotFound(
            "connconf_assignment not found".to_string(),
        )),
        Err(error) => handle_app_error(error),
    }
}

/// Remove a connection-config assignment.
#[utoipa::path(
    delete,
    path = "/api/v1/connection-config/assignments/{id}",
    tag = "connection-config",
    params(("id" = String, Path, description = "Assignment id")),
    operation_id = "delete_connconf_assignment",
    responses(
        (status = 204, description = "Assignment deleted"),
        (status = 404, description = "Assignment not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_connconf_assignment(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_config_mutation(&state, &request_context, Action::Delete).await {
        return handle_app_error(error);
    }

    let id = match parse_required_uuid(&id, "id") {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };

    match state.repositories.connconf_assignments.delete(id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => handle_app_error(AppError::NotFound(
            "connconf_assignment not found".to_string(),
        )),
        Err(error) => handle_app_error(error),
    }
}

// ----------------------------------------------------------------------------
// Bulk-assign handler
// ----------------------------------------------------------------------------

/// Pure classification of one bulk-assign target against the record's
/// existing assignments and the target's inventory scope-ancestry.
///
/// Structural: `scope_type` is derived from which id `DeviceScopeIds` carries
/// (`device_id` for a device target, `vm_id` for a VM target) — never from
/// client input. Mirrors this file's existing pure-fn-plus-unit-test style
/// (see `validate_protocol_change_does_not_break_children`).
fn classify_bulk_target(
    scopes: Option<&DeviceScopeIds>,
    existing: &HashSet<(ConnConfScope, Uuid)>,
) -> Result<(ConnConfScope, Uuid, bool), AppError> {
    let scopes = scopes.ok_or_else(|| AppError::NotFound("device or vm not found".to_string()))?;
    let (scope_type, scope_id) = match (scopes.device_id, scopes.vm_id) {
        (Some(device_id), _) => (ConnConfScope::Device, device_id),
        (None, Some(vm_id)) => (ConnConfScope::Vm, vm_id),
        (None, None) => {
            return Err(AppError::Validation(
                "target is neither a device nor a vm".to_string(),
            ));
        }
    };
    let already_assigned = existing.contains(&(scope_type, scope_id));
    Ok((scope_type, scope_id, already_assigned))
}

/// Resolve, classify, and (if new) create the `device`/`vm`-scoped assignment
/// for one bulk-assign target. Returns `Ok(None)` when the target is already
/// assigned to this record (no-op, idempotent re-run), `Ok(Some(_))` for a
/// newly created assignment. Isolated per-target so one target's error is
/// caught by the caller instead of unwinding the whole batch.
async fn bulk_assign_one_target(
    assignment_repo: &crate::contexts::connection_config::SqlxConnConfAssignmentRepository,
    record_id: Uuid,
    target_id_raw: &str,
    weight: Option<i32>,
    existing: &mut HashSet<(ConnConfScope, Uuid)>,
) -> Result<Option<ConnConfAssignment>, AppError> {
    let target_id = parse_required_uuid(target_id_raw, "target_id")?;
    let scopes = assignment_repo.get_device_scopes(target_id).await?;
    let (scope_type, scope_id, already_assigned) = classify_bulk_target(scopes.as_ref(), existing)?;
    if already_assigned {
        return Ok(None);
    }

    let input = NewConnConfAssignment {
        record_id,
        scope_type,
        scope_id: Some(scope_id),
        weight,
        enabled: None,
    };
    let assignment = assignment_repo.create_if_absent(&input).await?;
    if assignment.is_some() {
        // Track it locally so a duplicate target_id LATER IN THE SAME REQUEST is
        // also classified `already_assigned` without another database round-trip.
        existing.insert((scope_type, scope_id));
    }
    Ok(assignment)
}

/// Bulk-assign a connection-config record to many device/VM targets.
///
/// Resolves each target the same way the effective-config resolver resolves a
/// device's scope ancestry, then creates one `device`/`vm`-scoped assignment
/// per NEW target. Targets already assigned to this record are reported
/// `already_assigned` and never re-inserted. A single target's failure is
/// caught and reported per-item — it does not abort the rest of the batch.
#[utoipa::path(
    post,
    path = "/api/v1/connection-config/records/{id}/assignments/bulk",
    tag = "connection-config",
    params(("id" = String, Path, description = "Record id")),
    request_body = BulkAssignConnConfRequest,
    operation_id = "bulk_assign_connconf_record",
    responses(
        (status = 200, description = "Per-target bulk-assign outcome", body = BulkAssignConnConfResponse),
        (status = 400, description = "Too many targets in one request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Record not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_assign_connconf_record(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<BulkAssignConnConfRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = payload.validate() {
        return handle_app_error(error);
    }
    // Same admin-level config-mutation gate as `create_connconf_assignment` —
    // bulk-assign is still an assignment create, just fanned out server-side.
    if let Err(error) = ensure_config_mutation(&state, &request_context, Action::Create).await {
        return handle_app_error(error);
    }

    let record_id = match parse_required_uuid(&id, "id") {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };

    // Reject the whole request before any I/O when the target list is too big.
    if payload.targets.len() > MAX_BULK_ASSIGN_TARGETS {
        return handle_app_error(AppError::Validation(format!(
            "targets exceeds the maximum of {MAX_BULK_ASSIGN_TARGETS} per bulk-assign request"
        )));
    }

    match state.repositories.connconf_records.find(record_id).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return handle_app_error(AppError::NotFound("connconf_record not found".to_string()));
        }
        Err(error) => return handle_app_error(error),
    }

    let assignment_repo = &state.repositories.connconf_assignments;
    let mut existing: HashSet<(ConnConfScope, Uuid)> =
        match assignment_repo.list_for_record(record_id).await {
            Ok(assignments) => assignments
                .into_iter()
                .filter_map(|a| a.scope_id.map(|scope_id| (a.scope_type, scope_id)))
                .collect(),
            Err(error) => return handle_app_error(error),
        };

    let mut items = Vec::with_capacity(payload.targets.len());
    let mut assigned_count = 0i64;
    let mut already_assigned_count = 0i64;
    let mut failed_count = 0i64;

    for target in payload.targets {
        let target_id_raw = target.target_id;
        let weight = target.weight.or(payload.weight);
        let outcome = bulk_assign_one_target(
            assignment_repo,
            record_id,
            &target_id_raw,
            weight,
            &mut existing,
        )
        .await;

        match outcome {
            Ok(Some(assignment)) => {
                assigned_count += 1;
                items.push(BulkAssignConnConfOutcomeItem {
                    target_id: target_id_raw,
                    status: "assigned".to_string(),
                    assignment: Some(ConnConfAssignmentResponse::from(assignment)),
                    error: None,
                });
            }
            Ok(None) => {
                already_assigned_count += 1;
                items.push(BulkAssignConnConfOutcomeItem {
                    target_id: target_id_raw,
                    status: "already_assigned".to_string(),
                    assignment: None,
                    error: None,
                });
            }
            Err(error) => {
                failed_count += 1;
                items.push(BulkAssignConnConfOutcomeItem {
                    target_id: target_id_raw,
                    status: "failed".to_string(),
                    assignment: None,
                    error: Some(error.to_string()),
                });
            }
        }
    }

    (
        StatusCode::OK,
        Json(BulkAssignConnConfResponse {
            items,
            assigned_count,
            already_assigned_count,
            failed_count,
        }),
    )
        .into_response()
}

// ----------------------------------------------------------------------------
// Effective-config resolver
// ----------------------------------------------------------------------------

/// Translate the device's `DeviceScopeIds` ancestry into the
/// `(scope_type, scope_id)` pairs the resolver matches on, plus the implicit
/// `global` scope. Pure mapping, no SQL.
fn build_device_scopes(scopes: &DeviceScopeIds) -> Vec<(ConnConfScope, Option<Uuid>)> {
    scopes.to_scope_pairs()
}

/// Resolve the effective connection config for a target.
#[utoipa::path(
    get,
    path = "/api/v1/connection-config/effective/{device_id}",
    tag = "connection-config",
    params(
        EffectiveConnConfigQuery,
        ("device_id" = String, Path, description = "Device or VM id"),
    ),
    operation_id = "get_effective_connection_config",
    responses(
        (status = 200, description = "Effective connection config", body = EffectiveConnConfigResponse),
        (status = 404, description = "Device not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_effective_connection_config(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(device_id): Path<String>,
    Query(query): Query<EffectiveConnConfigQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    resolve_effective_connection_config_response(&state, &request_context, &device_id, query).await
}

pub(crate) async fn resolve_effective_connection_config_response(
    state: &AppState,
    request_context: &RequestContext,
    device_id: &str,
    query: EffectiveConnConfigQuery,
) -> axum::response::Response {
    let device_id = match parse_required_uuid(device_id, "device_id") {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };
    // Default to ssh when the caller does not specify a protocol. Parsed
    // BEFORE authorization so the authorized protocol is the same validated
    // value the config resolution below uses.
    let protocol = match query.protocol.as_deref() {
        Some(raw) => match parse_protocol(raw) {
            Ok(v) => v,
            Err(e) => return handle_app_error(e),
        },
        None => ConnConfProtocol::Ssh,
    };
    // Authorize against the SPECIFIC target host, like the connect path — a
    // global read capability is insufficient to expose a device's effective
    // connect-time config and its secret references.
    if let Err(error) =
        ensure_effective_config_access(state, request_context, device_id, protocol.as_str()).await
    {
        return handle_app_error(error);
    }

    let assignment_repo = &state.repositories.connconf_assignments;

    // Load the device/VM scope ancestry (SQL lives in the repo).
    let device_scope_ids = match assignment_repo.get_device_scopes(device_id).await {
        Ok(Some(ids)) => ids,
        Ok(None) => {
            return handle_app_error(AppError::NotFound("device or vm not found".to_string()));
        }
        Err(error) => return handle_app_error(error),
    };
    let device_scopes = build_device_scopes(&device_scope_ids);

    // Candidate assignments matching the device scopes (SQL in the repo).
    let assignments = match assignment_repo.list_for_scopes(&device_scopes).await {
        Ok(a) => a,
        Err(error) => return handle_app_error(error),
    };

    // Records for the requested protocol plus any base-record template chains.
    // Keeping the protocol predicate in SQL avoids loading unrelated protocols
    // while still preserving resolver validation for mismatched bases.
    let records = match state
        .repositories
        .connconf_records
        .list_by_protocol_with_bases(protocol)
        .await
    {
        Ok(r) => r,
        Err(error) => return handle_app_error(error),
    };

    let input = ResolveInput {
        protocol,
        device_scopes: &device_scopes,
        assignments: &assignments,
        records: &records,
    };
    match resolve_effective_config(&input) {
        Ok(effective) => (
            StatusCode::OK,
            Json(EffectiveConnConfigResponse::from(effective)),
        )
            .into_response(),
        Err(error) => handle_app_error(AppError::Validation(error.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ConnConfProtocol, ConnConfScope, DeviceScopeIds, classify_bulk_target,
        validate_protocol_change_does_not_break_children,
    };
    use crate::errors::AppError;
    use std::collections::HashSet;
    use uuid::Uuid;

    #[test]
    fn protocol_change_is_rejected_for_template_parent() {
        let error = validate_protocol_change_does_not_break_children(
            ConnConfProtocol::Ssh,
            Some(ConnConfProtocol::Rdp),
            true,
        )
        .expect_err("parent protocol change must be rejected");

        match error {
            AppError::Validation(message) => assert!(
                message.contains("template parent"),
                "unexpected message: {message}"
            ),
            other => panic!("unexpected error variant: {other:?}"),
        }
    }

    #[test]
    fn same_protocol_update_is_allowed_for_template_parent() {
        validate_protocol_change_does_not_break_children(
            ConnConfProtocol::Ssh,
            Some(ConnConfProtocol::Ssh),
            true,
        )
        .expect("same protocol does not break children");
    }

    #[test]
    fn classify_bulk_target_maps_device_target_when_unassigned() {
        let device_id = Uuid::now_v7();
        let scopes = DeviceScopeIds {
            device_id: Some(device_id),
            ..DeviceScopeIds::default()
        };
        let existing = HashSet::new();

        let (scope_type, scope_id, already_assigned) =
            classify_bulk_target(Some(&scopes), &existing).expect("device target classifies");

        assert_eq!(scope_type, ConnConfScope::Device);
        assert_eq!(scope_id, device_id);
        assert!(!already_assigned);
    }

    #[test]
    fn classify_bulk_target_maps_vm_target_when_unassigned() {
        let vm_id = Uuid::now_v7();
        let scopes = DeviceScopeIds {
            vm_id: Some(vm_id),
            ..DeviceScopeIds::default()
        };
        let existing = HashSet::new();

        let (scope_type, scope_id, already_assigned) =
            classify_bulk_target(Some(&scopes), &existing).expect("vm target classifies");

        assert_eq!(scope_type, ConnConfScope::Vm);
        assert_eq!(scope_id, vm_id);
        assert!(!already_assigned);
    }

    #[test]
    fn classify_bulk_target_reports_already_assigned_when_in_existing_set() {
        let device_id = Uuid::now_v7();
        let scopes = DeviceScopeIds {
            device_id: Some(device_id),
            ..DeviceScopeIds::default()
        };
        let mut existing = HashSet::new();
        existing.insert((ConnConfScope::Device, device_id));

        let (_, _, already_assigned) =
            classify_bulk_target(Some(&scopes), &existing).expect("device target classifies");

        assert!(
            already_assigned,
            "a target already in the existing set must be reported as such"
        );
    }

    #[test]
    fn classify_bulk_target_rejects_unresolved_target() {
        let existing = HashSet::new();

        let error = classify_bulk_target(None, &existing)
            .expect_err("an unresolved target must be rejected");

        match error {
            AppError::NotFound(message) => assert!(
                message.contains("device or vm"),
                "unexpected message: {message}"
            ),
            other => panic!("unexpected error variant: {other:?}"),
        }
    }
}
