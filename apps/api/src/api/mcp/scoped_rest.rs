//! Scoped REST tool calls, RBAC prechecks and endpoint projection, split out of `mcp.rs`.
//!
//! Moved verbatim; behaviour is unchanged.

use super::*;

pub(super) fn generic_rest_method(arguments: &Value) -> Result<Method, String> {
    let method = arguments
        .get("method")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Missing required method".to_string())?
        .to_ascii_uppercase();
    method_from_str(method.as_str())
}

pub(super) fn method_from_str(method: &str) -> Result<Method, String> {
    match method.trim().to_ascii_uppercase().as_str() {
        "GET" => Ok(Method::GET),
        "POST" => Ok(Method::POST),
        "PUT" => Ok(Method::PUT),
        "PATCH" => Ok(Method::PATCH),
        "DELETE" => Ok(Method::DELETE),
        _ => Err(format!("Unsupported REST method '{method}'")),
    }
}

pub(super) fn generic_rest_uri(arguments: &Value) -> Result<String, String> {
    let path = arguments
        .get("path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Missing required path".to_string())?;
    validate_generic_rest_path(path)?;
    append_query(path.to_string(), arguments.get("query"))
}

pub(super) fn generic_rest_is_connection_session_create(method: &Method, uri: &str) -> bool {
    method == Method::POST
        && uri.split_once('?').map_or(uri, |(path, _)| path) == CONNECTION_SESSIONS_PATH
}

pub(super) fn connection_session_create_resource_id_from_arguments(
    arguments: &Value,
) -> Result<String, String> {
    let target_id = arguments
        .pointer("/body/target_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            "Missing required body.target_id for MCP connection session creation".to_string()
        })?;
    if target_id.contains('/') || target_id.contains('?') || target_id.contains('#') {
        return Err("body.target_id contains an invalid character".to_string());
    }
    Ok(format!("connection_target:{target_id}"))
}

pub(super) fn validate_generic_rest_path(path: &str) -> Result<(), String> {
    if !path.starts_with("/api/v1/") {
        return Err("Generic REST calls must target an /api/v1 path".to_string());
    }
    if path.contains('?') || path.contains('#') || path.contains("://") || path.contains('\\') {
        return Err(
            "Path must not contain query strings, fragments, schemes, or backslashes".to_string(),
        );
    }
    if path
        .split('/')
        .any(|segment| segment == "." || segment == "..")
    {
        return Err("Path must not contain dot segments".to_string());
    }
    if mcp_rest_path_is_internal_or_protocol(path) {
        return Err(format!(
            "Path '{path}' is not exposed through MCP generic REST calls"
        ));
    }
    Ok(())
}

pub(super) fn action_key(action: &Action) -> &'static str {
    match action {
        Action::Create => "create",
        Action::Read => "read",
        Action::Update => "update",
        Action::Delete => "delete",
        Action::Connect => "connect",
        Action::Observe => "observe",
        Action::Execute => "execute",
        Action::CctvView => "view",
        Action::CctvRecordings => "recordings",
        Action::CctvExport => "export",
        Action::DeviceUsers => "device_users",
        Action::CctvSettings => "settings",
        Action::CctvPtz => "ptz",
        Action::ManageRoles => "manage_roles",
        Action::ManagePolicies => "manage_policies",
        Action::ViewAudit => "view_audit",
        Action::Secret => "secret",
    }
}

pub(super) fn resource_id_for_scope(
    tool: &McpRestTool,
    arguments: &Value,
) -> Result<Option<String>, String> {
    match tool.resource_id_source {
        ResourceIdSource::None => Ok(None),
        ResourceIdSource::PathParam { key, prefix } => {
            let raw = path_param(arguments, key)
                .ok_or_else(|| format!("Missing required path parameter '{key}'"))?;
            Ok(Some(format!("{prefix}{raw}")))
        }
        ResourceIdSource::QueryParam { key, prefix } => {
            let raw = query_param(arguments, key)
                .ok_or_else(|| format!("Missing required query parameter '{key}'"))?;
            Ok(Some(format!("{prefix}{raw}")))
        }
        ResourceIdSource::BodyPointer { pointer, prefix } => {
            let raw = arguments
                .pointer(pointer)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| format!("Missing required body field '{pointer}'"))?;
            Ok(Some(format!("{prefix}{raw}")))
        }
    }
}

pub(super) async fn handle_scoped_rest_list_endpoints_tool_call(
    state: AppState,
    audit_auth_context: Option<AuthContext>,
    request_context: RequestContext,
    id: Option<Value>,
    arguments: Value,
) -> Response {
    let full = arguments
        .get("full")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let summary = arguments
        .get("summary")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let limit = match optional_mcp_usize_param(&arguments, "limit", 50, 1, 1000) {
        Ok(value) => value,
        Err(error) => {
            return jsonrpc_result(
                id,
                tool_error(json!({
                    "code": "INVALID_ARGUMENTS",
                    "message": error
                })),
            );
        }
    };
    let offset = match optional_mcp_usize_param(&arguments, "offset", 0, 0, usize::MAX) {
        Ok(value) => value,
        Err(error) => {
            return jsonrpc_result(
                id,
                tool_error(json!({
                    "code": "INVALID_ARGUMENTS",
                    "message": error
                })),
            );
        }
    };
    let fields = endpoint_projection_fields(&arguments);
    let scope_visible = visible_scoped_rest_endpoints(&request_context)
        .into_iter()
        .filter(|endpoint| scoped_rest_endpoint_matches_filter(endpoint, &arguments))
        .collect::<Vec<_>>();
    let scope_visible_total = scope_visible.len();
    let mut rbac_filtered = Vec::new();
    let mut rbac_denied_count = 0usize;
    let mut rbac_deferred_count = 0usize;
    for endpoint in scope_visible {
        let rbac = scoped_rest_endpoint_rbac_status(&state, &request_context, &endpoint).await;
        match rbac {
            ScopedRestEndpointRbacStatus::Denied => {
                rbac_denied_count += 1;
            }
            ScopedRestEndpointRbacStatus::Deferred => {
                rbac_deferred_count += 1;
                rbac_filtered.push((endpoint, rbac));
            }
            ScopedRestEndpointRbacStatus::Allowed => {
                rbac_filtered.push((endpoint, rbac));
            }
        }
    }
    let total = rbac_filtered.len();
    let page_limit = if full { usize::MAX } else { limit };
    let endpoints = rbac_filtered
        .into_iter()
        .skip(if full { 0 } else { offset })
        .take(page_limit)
        .map(|(endpoint, rbac)| {
            let value = if summary {
                scoped_rest_endpoint_summary_json(endpoint, rbac)
            } else {
                scoped_rest_endpoint_json(endpoint, rbac)
            };
            project_endpoint_fields(value, fields.as_ref())
        })
        .collect::<Vec<_>>();
    require_mcp_tool_audit!(
        id,
        record_mcp_tool_audit(
            &state,
            audit_auth_context.as_ref(),
            &request_context,
            McpToolAudit {
                tool_name: SCOPED_REST_LIST_ENDPOINTS_TOOL_NAME,
                action: "read",
                resource_type: "rest",
                resource_id: None,
                decision: "success",
                rest_status: None,
                error_code: None
            }
        )
    );
    jsonrpc_result(
        id,
        tool_success(json!({
            "count": endpoints.len(),
            "total": total,
            "limit": if full { Value::Null } else { Value::from(limit) },
            "offset": if full { Value::from(0) } else { Value::from(offset) },
            "full": full,
            "summary": summary,
            "authorization": {
                "token_scope": "checked",
                "rbac": "prechecked_for_broad_or_static_resource_operations",
                "deferred": "operations that require a path/query/body resource id are RBAC-checked at call time",
                "scope_visible_total": scope_visible_total,
                "rbac_denied_count": rbac_denied_count,
                "rbac_deferred_count": rbac_deferred_count
            },
            "endpoints": endpoints
        })),
    )
}

/// One rejected scoped-REST tool call: what to audit, and what to answer with.
pub(super) struct ScopedRestRejection<'a> {
    /// Audited action; `call` before the endpoint is known.
    pub(super) action: &'a str,
    /// Audited resource type; `rest` before the endpoint is known.
    pub(super) resource_type: &'a str,
    pub(super) resource_id: Option<&'a str>,
    /// Audit outcome label, e.g. `scope_denied`.
    pub(super) outcome: &'a str,
    /// Wire error code. Recorded on the audit row from this same field, so the
    /// two can never disagree.
    pub(super) code: &'a str,
    pub(super) message: String,
}

/// Audit a rejected scoped-REST tool call and build its JSON-RPC error.
///
/// Every rejection in [`handle_scoped_rest_endpoint_tool_call`] funnels through
/// here, so the audit call and the error literal are written once and the error
/// code cannot drift between the audit row and the wire.
pub(super) async fn reject_scoped_rest_call(
    state: &AppState,
    audit_auth_context: Option<&AuthContext>,
    request_context: &RequestContext,
    id: Option<Value>,
    rejection: ScopedRestRejection<'_>,
) -> Response {
    require_mcp_tool_audit!(
        id,
        record_mcp_tool_audit(
            state,
            audit_auth_context,
            request_context,
            McpToolAudit {
                tool_name: SCOPED_REST_CALL_ENDPOINT_TOOL_NAME,
                action: rejection.action,
                resource_type: rejection.resource_type,
                resource_id: rejection.resource_id,
                decision: rejection.outcome,
                rest_status: None,
                error_code: Some(rejection.code)
            }
        )
    );
    jsonrpc_result(
        id,
        tool_error(json!({
            "code": rejection.code,
            "message": rejection.message,
        })),
    )
}

pub(super) fn scoped_rest_endpoint_matches_filter(
    endpoint: &McpRestEndpoint,
    arguments: &Value,
) -> bool {
    optional_string_filter(arguments, "operation_id")
        .is_none_or(|filter| endpoint.operation_id == filter)
        && optional_string_filter(arguments, "action")
            .is_none_or(|filter| endpoint.action == filter)
        && optional_string_filter(arguments, "resource_type")
            .is_none_or(|filter| endpoint.resource_type == filter)
        && optional_string_filter(arguments, "method")
            .is_none_or(|filter| endpoint.method.as_str().eq_ignore_ascii_case(filter))
        && optional_string_filter(arguments, "path_contains")
            .is_none_or(|filter| endpoint.path_template.contains(filter))
}

pub(super) fn optional_string_filter<'a>(arguments: &'a Value, key: &str) -> Option<&'a str> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ScopedRestEndpointRbacStatus {
    Allowed,
    Deferred,
    Denied,
}

impl ScopedRestEndpointRbacStatus {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Allowed => "allowed",
            Self::Deferred => "deferred",
            Self::Denied => "denied",
        }
    }

    pub(super) fn note(self) -> &'static str {
        match self {
            Self::Allowed => "Token scope and RBAC both allow this operation.",
            Self::Deferred => {
                "Token scope exposes this operation; RBAC is checked at call time with the concrete resource id."
            }
            Self::Denied => {
                "Token scope exposes this operation, but RBAC denies the token principal."
            }
        }
    }
}

pub(super) async fn scoped_rest_endpoint_rbac_status(
    state: &AppState,
    request_context: &RequestContext,
    endpoint: &McpRestEndpoint,
) -> ScopedRestEndpointRbacStatus {
    if !scoped_rest_endpoint_has_precheckable_resource(endpoint) {
        return ScopedRestEndpointRbacStatus::Deferred;
    }
    if mcp_endpoint_rbac_denied(
        state,
        request_context,
        endpoint,
        endpoint.static_resource_id.as_deref(),
    )
    .await
    {
        ScopedRestEndpointRbacStatus::Denied
    } else {
        ScopedRestEndpointRbacStatus::Allowed
    }
}

pub(super) fn scoped_rest_endpoint_has_precheckable_resource(endpoint: &McpRestEndpoint) -> bool {
    matches!(endpoint.resource_id_source, ResourceIdSource::None)
        || endpoint.static_resource_id.is_some()
}

pub(super) async fn mcp_endpoint_rbac_denied(
    state: &AppState,
    request_context: &RequestContext,
    endpoint: &McpRestEndpoint,
    resource_id: Option<&str>,
) -> bool {
    if endpoint.operation_id == CCTV_CAPABILITIES_OPERATION_ID {
        for action in CCTV_SCOPE_ACTIONS {
            if !mcp_rest_rbac_denied(state, request_context, action, "cctv", resource_id).await {
                return false;
            }
        }
        return true;
    }
    mcp_rest_rbac_denied(
        state,
        request_context,
        endpoint.action,
        endpoint.resource_type,
        resource_id,
    )
    .await
}

pub(super) async fn mcp_rest_rbac_denied(
    state: &AppState,
    request_context: &RequestContext,
    action: &str,
    resource_type: &str,
    resource_id: Option<&str>,
) -> bool {
    let Some(action) = mcp_action_from_key(action) else {
        return false;
    };
    let Some(resource_type) = ResourceType::parse(resource_type) else {
        return false;
    };
    let resource_type_name = resource_type.as_str();
    match state
        .services
        .access_control
        .check_access(request_context, action, resource_type, resource_id)
        .await
    {
        Ok(PolicyDecision::Allow) => false,
        Ok(PolicyDecision::Deny) => true,
        Err(error) => {
            log_error_event(
                "mcp.rest.rbac_precheck.failed",
                "mcp_rbac_precheck_error",
                error.to_string(),
                EventFields::default()
                    .with_operation("mcp.rest.rbac_precheck")
                    .with_resource("rest", resource_type_name)
                    .with_message("MCP REST RBAC precheck failed"),
            );
            true
        }
    }
}

pub(super) fn mcp_action_from_key(action: &str) -> Option<Action> {
    match action {
        "create" => Some(Action::Create),
        "read" => Some(Action::Read),
        "update" => Some(Action::Update),
        "delete" => Some(Action::Delete),
        "connect" => Some(Action::Connect),
        "observe" => Some(Action::Observe),
        "execute" => Some(Action::Execute),
        "manage_roles" => Some(Action::ManageRoles),
        "manage_policies" => Some(Action::ManagePolicies),
        "view_audit" => Some(Action::ViewAudit),
        "secret" => Some(Action::Secret),
        "view" => Some(Action::CctvView),
        "recordings" => Some(Action::CctvRecordings),
        "export" => Some(Action::CctvExport),
        "device_users" => Some(Action::DeviceUsers),
        "settings" => Some(Action::CctvSettings),
        "ptz" => Some(Action::CctvPtz),
        _ => None,
    }
}

pub(super) fn scoped_rest_endpoint_json(
    endpoint: McpRestEndpoint,
    rbac: ScopedRestEndpointRbacStatus,
) -> Value {
    json!({
        "operation_id": endpoint.operation_id,
        "title": endpoint.title,
        "description": endpoint.description,
        "method": endpoint.method.as_str(),
        "path_template": endpoint.path_template,
        "action": endpoint.action,
        "resource_type": endpoint.resource_type,
        "static_resource_id": endpoint.static_resource_id,
        "full_control_only": endpoint.full_control_only,
        "resource_id_source": resource_id_source_json(endpoint.resource_id_source),
        "authorization": {
            "token_scope": "allowed",
            "rbac": rbac.as_str(),
            "note": rbac.note()
        }
    })
}

pub(super) fn scoped_rest_endpoint_summary_json(
    endpoint: McpRestEndpoint,
    rbac: ScopedRestEndpointRbacStatus,
) -> Value {
    json!({
        "operation_id": endpoint.operation_id,
        "method": endpoint.method.as_str(),
        "path_template": endpoint.path_template,
        "action": endpoint.action,
        "resource_type": endpoint.resource_type,
        "full_control_only": endpoint.full_control_only,
        "authorization": {
            "token_scope": "allowed",
            "rbac": rbac.as_str()
        }
    })
}

pub(super) fn endpoint_projection_fields(arguments: &Value) -> Option<HashSet<String>> {
    let fields = arguments.get("fields")?.as_array()?;
    let set = fields
        .iter()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect::<HashSet<_>>();
    if set.is_empty() { None } else { Some(set) }
}

pub(super) fn project_endpoint_fields(
    mut endpoint: Value,
    fields: Option<&HashSet<String>>,
) -> Value {
    let Some(fields) = fields else {
        return endpoint;
    };
    let Some(object) = endpoint.as_object_mut() else {
        return endpoint;
    };
    object.retain(|key, _| fields.contains(key));
    endpoint
}

pub(super) fn resource_id_source_json(source: ResourceIdSource) -> Value {
    match source {
        ResourceIdSource::None => json!({ "kind": "none" }),
        ResourceIdSource::PathParam { key, prefix } => {
            json!({ "kind": "path_param", "key": key, "prefix": prefix })
        }
        ResourceIdSource::QueryParam { key, prefix } => {
            json!({ "kind": "query_param", "key": key, "prefix": prefix })
        }
        ResourceIdSource::BodyPointer { pointer, prefix } => {
            json!({ "kind": "body_pointer", "pointer": pointer, "prefix": prefix })
        }
    }
}

pub(super) fn required_endpoint_operation_id(arguments: &Value) -> Result<&str, String> {
    arguments
        .get("operation_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Missing required operation_id".to_string())
}

pub(super) fn resource_id_for_endpoint(
    endpoint: &McpRestEndpoint,
    arguments: &Value,
) -> Result<Option<String>, String> {
    if let Some(resource_id) = endpoint.static_resource_id.as_ref() {
        return Ok(Some(resource_id.clone()));
    }
    match endpoint.resource_id_source {
        ResourceIdSource::None => Ok(None),
        ResourceIdSource::PathParam { key, prefix } => {
            let raw = path_param(arguments, key)
                .ok_or_else(|| format!("Missing required path parameter '{key}'"))?;
            Ok(Some(format!("{prefix}{raw}")))
        }
        ResourceIdSource::QueryParam { key, prefix } => {
            let raw = query_param(arguments, key)
                .ok_or_else(|| format!("Missing required query parameter '{key}'"))?;
            Ok(Some(format!("{prefix}{raw}")))
        }
        ResourceIdSource::BodyPointer { pointer, prefix } => {
            let raw = arguments
                .pointer(pointer)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| format!("Missing required body field '{pointer}'"))?;
            Ok(Some(format!("{prefix}{raw}")))
        }
    }
}

pub(super) fn build_rest_endpoint_uri(
    endpoint: &McpRestEndpoint,
    arguments: &Value,
) -> Result<String, String> {
    let path = fill_path_template(endpoint.path_template.as_str(), arguments)?;
    append_query(path, arguments.get("query"))
}

pub(super) fn rest_endpoint_dispatch_auth_context(
    auth_context: AuthContext,
    endpoint: &McpRestEndpoint,
    resource_id: Option<&str>,
) -> AuthContext {
    if let Some(resource_id) = resource_id {
        return auth_context_with_resource_override(
            auth_context,
            endpoint.action,
            endpoint.resource_type,
            resource_id.to_string(),
        );
    }
    auth_context
}
