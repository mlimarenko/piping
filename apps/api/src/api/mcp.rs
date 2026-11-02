//! MCP endpoint `/api/v1/mcp`: JSON-RPC method dispatch, tool and resource catalogue, internal REST bridging, scope visibility and mandatory audit recording.
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    Extension, Json,
    body::{Body, to_bytes},
    extract::State,
    http::{
        HeaderMap, HeaderName, HeaderValue, Method, Request, StatusCode,
        header::{ACCEPT, CONTENT_TYPE, HOST, ORIGIN, RETRY_AFTER, USER_AGENT},
    },
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::{Duration as ChronoDuration, Utc};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};
use tower::util::ServiceExt;
use uuid::Uuid;

use crate::api::access_control_helpers::build_device_management_service;
use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::request_context::build_request_context;
use crate::api::rest::router::build_rest_router;
use crate::api::rest::routes::cctv::{ensure_target_scoped_access, max_export_window_seconds};
use crate::api::rest::routes::connections::InternalConnectionSessionCreatePolicy;
use crate::app::AppState;
use crate::contexts::access_control::application::user_api_token_scope_catalog_pairs;
use crate::contexts::access_control::domain::{Action, PolicyDecision, ResourceType};
use crate::contexts::audit::domain::AuditEvent;
use crate::contexts::connection_config::LaunchAccount;
use crate::contexts::connections::domain::{ConnectionProtocol, UserCredentialSet};
use crate::contexts::device_management::domain::SnapshotRef;
use crate::contexts::shared::request_context::{ApiTokenScopeResourceOverride, RequestContext};
use crate::observability::{EventFields, log_error_event};
use crate::proxy;

const MCP_PROTOCOL_VERSION: &str = "2025-06-18";
const MCP_PRIOR_PROTOCOL_VERSION: &str = "2025-03-26";
const MCP_RESPONSE_BODY_LIMIT_BYTES: usize = 4 * 1024 * 1024;
const MCP_PROTOCOL_VERSION_HEADER: &str = "mcp-protocol-version";
const MCP_AUDIT_RECORD_FAILED_CODE: &str = "AUDIT_RECORD_FAILED";
const MCP_AUDIT_RECORD_FAILED_MESSAGE: &str =
    "The MCP call failed because its mandatory audit record could not be stored";
const MCP_CAPABILITIES_TOOL_NAME: &str = "piping.mcp.capabilities";
const CCTV_CAPABILITIES_OPERATION_ID: &str = "connection_target.cctv.capabilities";
const CCTV_SCOPE_ACTIONS: &[&str] = &["view", "recordings", "export", "settings", "ptz"];
const MCP_RESOURCE_CAPABILITIES_URI: &str = "piping://mcp/capabilities";
const MCP_RESOURCE_TOOLS_URI: &str = "piping://mcp/tools";
const MCP_RESOURCE_SCOPE_CATALOG_URI: &str = "piping://access-control/scope-catalog";
const MCP_RESOURCE_INVENTORY_ENTITIES_URI: &str = "piping://inventory/entities";
const MCP_RESOURCE_INVENTORY_ENTITY_PREFIX: &str = "piping://inventory/entities/";
const MCP_RESOURCE_ACCESS_CONTROL_ENTITIES_URI: &str = "piping://access-control/entities";
const MCP_RESOURCE_ACCESS_CONTROL_ENTITY_PREFIX: &str = "piping://access-control/entities/";
const MCP_RESOURCE_CONNECTION_TARGET_PREFIX: &str = "piping://connection_target/";
const GENERIC_REST_TOOL_NAME: &str = "piping.rest.call_api";
const SCOPED_REST_LIST_ENDPOINTS_TOOL_NAME: &str = "piping.rest.list_allowed_endpoints";
const SCOPED_REST_CALL_ENDPOINT_TOOL_NAME: &str = "piping.rest.call_endpoint";
const INVENTORY_LIST_TOOL_NAME: &str = "piping.inventory.list_resources";
const INVENTORY_GET_TOOL_NAME: &str = "piping.inventory.get_resource";
const INVENTORY_CREATE_TOOL_NAME: &str = "piping.inventory.create_resource";
const INVENTORY_UPDATE_TOOL_NAME: &str = "piping.inventory.update_resource";
const INVENTORY_DELETE_TOOL_NAME: &str = "piping.inventory.delete_resource";
const INVENTORY_BULK_CREATE_TOOL_NAME: &str = "piping.inventory.bulk_create_resources";
const INVENTORY_BULK_MUTATE_TOOL_NAME: &str = "piping.inventory.bulk_mutate_resources";
const INVENTORY_ENTITY_RESOURCE_PREFIX: &str = "inventory_entity:";
const ACCESS_CONTROL_LIST_TOOL_NAME: &str = "piping.access_control.list_resources";
const ACCESS_CONTROL_GET_TOOL_NAME: &str = "piping.access_control.get_resource";
const ACCESS_CONTROL_CREATE_TOOL_NAME: &str = "piping.access_control.create_resource";
const ACCESS_CONTROL_UPDATE_TOOL_NAME: &str = "piping.access_control.update_resource";
const ACCESS_CONTROL_DELETE_TOOL_NAME: &str = "piping.access_control.delete_resource";
const ACCESS_CONTROL_BULK_TOOL_NAME: &str = "piping.access_control.bulk_resources";
const CREATE_CONNECTION_SESSION_TOOL_NAME: &str = "piping.rest.create_connection_session";
const CREATE_CONNECTION_SESSION_OPERATION_ID: &str = "create_connection_session";
const CONNECTION_SESSIONS_PATH: &str = "/api/v1/connections/sessions";
const HOST_SSH_OPEN_TOOL_NAME: &str = "piping.connection_target.ssh_open";
const HOST_SSH_EXEC_TOOL_NAME: &str = "piping.connection_target.ssh_exec";
const HOST_SSH_WRITE_TOOL_NAME: &str = "piping.connection_target.ssh_write";
const HOST_SSH_READ_TOOL_NAME: &str = "piping.connection_target.ssh_read";
const HOST_SSH_READ_UNTIL_TOOL_NAME: &str = "piping.connection_target.ssh_read_until";
const HOST_SSH_WAIT_FOR_PROMPT_TOOL_NAME: &str = "piping.connection_target.ssh_wait_for_prompt";
const HOST_SSH_WAIT_FOR_QUIET_TOOL_NAME: &str = "piping.connection_target.ssh_wait_for_quiet";
const HOST_RESOLVE_CONNECTION_TOOL_NAME: &str = "piping.connection_target.resolve_connection";
const HOST_SSH_TAIL_LOG_TOOL_NAME: &str = "piping.connection_target.ssh_tail_log";
const HOST_INSPECT_TOOL_NAME: &str = "piping.connection_target.inspect";
const HOST_CHECK_PORT_TOOL_NAME: &str = "piping.connection_target.check_port";
const HOST_READ_FILE_TOOL_NAME: &str = "piping.connection_target.read_file";
const HOST_FIND_LOGS_TOOL_NAME: &str = "piping.connection_target.find_logs";
const HOST_SSH_RESIZE_TOOL_NAME: &str = "piping.connection_target.ssh_resize";
const HOST_SSH_CLOSE_TOOL_NAME: &str = "piping.connection_target.ssh_close";
const HOST_VNC_OPEN_TOOL_NAME: &str = "piping.connection_target.vnc_open";
const HOST_VNC_CAPTURE_TOOL_NAME: &str = "piping.connection_target.vnc_capture";
const CCTV_SNAPSHOT_TOOL_NAME: &str = "piping.connection_target.cctv_snapshot";
const CCTV_EXPORT_TOOL_NAME: &str = "piping.connection_target.cctv_export";
const CCTV_PLAYBACK_TOOL_NAME: &str = "piping.connection_target.cctv_playback";
const HOST_VNC_POINTER_TOOL_NAME: &str = "piping.connection_target.vnc_pointer";
const HOST_VNC_KEY_TOOL_NAME: &str = "piping.connection_target.vnc_key";
const HOST_VNC_TEXT_TOOL_NAME: &str = "piping.connection_target.vnc_text";
const HOST_VNC_CLOSE_TOOL_NAME: &str = "piping.connection_target.vnc_close";
const HOST_SESSION_CLIENT_CREDENTIALS_TOOL_NAME: &str =
    "piping.connection_target.session_client_credentials";
const MCP_REST_BLOCKED_PREFIXES: &[&str] = &[
    "/api/v1/mcp",
    "/api/v1/setup",
    "/api/v1/openapi",
    "/api/v1/health",
    "/api/v1/live",
    "/api/v1/ready",
    "/api/v1/metrics",
    "/api/v1/ws",
    "/api/v1/proxy",
    "/api/v1/auth",
    "/api/v1/webhooks/netbox",
];
/// REST paths the generic `call_api` fallback tool must never expose.
///
/// Every CCTV media path belongs here. Two reasons, and the second is the
/// dangerous one: a binary body would be JSON-parsed into garbage, and — for
/// the *streaming* paths — the caller acquires a media-plane slot plus a slice
/// of the shared budget and holds both while the response accumulates toward
/// the buffer cap, only to fail at the end. Repeated calls starve the plane for
/// real viewers while returning nothing. `mcp_rest_catalog_excludes_every_cctv_media_path`
/// pins this list against the contract so a newly added media route cannot
/// silently reopen the hole.
const MCP_REST_BLOCKED_EXACT_PATHS: &[&str] = &[
    "/api/v1/monitoring/sd",
    // Binary JPEG response: the generic `call_api` tool would try to JSON-parse
    // the image body, so keep the snapshot endpoint off the REST fallback catalog.
    "/api/v1/cctv/{target_id}/snapshot",
    // Binary video response: same reason as snapshot — the exported clip is a
    // binary download, not JSON.
    "/api/v1/cctv/{target_id}/export",
    // Unbounded live MJPEG: holds a live-transcode slot (the most expensive
    // plane) for the whole session. A generic caller cannot consume a multipart
    // stream, so it can only ever waste the slot.
    "/api/v1/cctv/{target_id}/stream",
    // Streaming MP4 of recorded footage: same bytes as export, same reasons.
    "/api/v1/cctv/{target_id}/playback",
];
const DEFAULT_SSH_READ_MAX_BYTES: usize = 32 * 1024;
const MAX_SSH_READ_MAX_BYTES: usize = 256 * 1024;
const DEFAULT_HOST_READ_FILE_BYTES: usize = 16 * 1024;
const MAX_HOST_FIND_LOGS_LIMIT: u64 = 100;
const DEFAULT_HOST_FIND_LOGS_LIMIT: u64 = 25;
const DEFAULT_SSH_EXEC_TIMEOUT_MS: u64 = 30_000;
const MAX_SSH_EXEC_TIMEOUT_MS: u64 = 300_000;
const DEFAULT_SSH_WAIT_TIMEOUT_MS: u64 = 5_000;
const MAX_SSH_WAIT_TIMEOUT_MS: u64 = 120_000;
const DEFAULT_SSH_WAIT_POLL_MS: u64 = 200;
const DEFAULT_SSH_QUIET_MS: u64 = 500;
const MAX_SSH_QUIET_MS: u64 = 30_000;
const HOST_SSH_PER_TARGET_CONCURRENCY: usize = 2;
const MAX_SSH_TAIL_LINES: u64 = 10_000;
const DEFAULT_SSH_TAIL_LINES: u64 = 200;
const MAX_VNC_TEXT_BYTES: usize = 4096;
const MCP_VNC_CAPTURE_TTL_SECONDS: u64 = 120;
const MCP_VNC_CAPTURE_TTL_CHRONO_SECONDS: i64 = 120;
const MAX_CCTV_SNAPSHOT_BYTES: usize = 8 * 1024 * 1024;
const DEFAULT_VNC_INLINE_IMAGE_MAX_BYTES: usize = 512 * 1024;
const MCP_RATE_LIMIT_WINDOW_SECS: u64 = 60;

/// Adapts an audit-failure [`Response`] to the enclosing MCP tool function's
/// return type: a bare `Response` for the dispatch layer, or `Err(..)` for the
/// authorizers that return a boxed error.
trait McpToolAuditReturn {
    fn from_audit_failure(response: Response) -> Self;
}

impl McpToolAuditReturn for Response {
    fn from_audit_failure(response: Response) -> Self {
        response
    }
}

fn audit_failure_return<T: McpToolAuditReturn>(response: Response) -> T {
    T::from_audit_failure(response)
}

macro_rules! require_mcp_tool_audit {
    ($id:expr, $audit:expr) => {
        if $audit.await.is_err() {
            return audit_failure_return(mcp_tool_audit_failure($id.clone()));
        }
    };
}

mod capabilities;
mod catalog;
mod dispatch;
mod host_ssh;
mod host_visual;
mod protocol;
mod resources;
mod routes_table;
mod schemas;
mod scoped_rest;
mod tool_model;
mod tools;
mod validation;
mod visibility;

#[cfg(test)]
mod test_support;

use capabilities::*;
use catalog::mcp_tools;
use dispatch::handle_tool_call;
use host_ssh::*;
use host_visual::*;
use protocol::*;
use resources::*;
use routes_table::{
    ACCESS_CONTROL_ENTITY_ROUTES, AccessControlEntityRoute, INVENTORY_ENTITY_ROUTES,
    STATIC_MCP_REST_ENDPOINTS,
};
use schemas::*;
use scoped_rest::*;
use tool_model::*;
use tools::{
    handle_access_control_tool_call, handle_cctv_archive_tool_call, handle_cctv_snapshot_tool_call,
    handle_generic_rest_call, handle_host_ssh_tool_call, handle_host_vnc_tool_call,
    handle_inventory_tool_call, handle_scoped_rest_endpoint_tool_call,
};
use validation::*;
use visibility::*;

/// The framework-supplied envelope of one MCP `tools/call` request, threaded
/// intact to the dedicated handlers that all need the same seven values.
pub(in crate::api::mcp) struct McpToolCallContext {
    pub(in crate::api::mcp) state: AppState,
    pub(in crate::api::mcp) request_id: RequestId,
    pub(in crate::api::mcp) auth_context: Option<AuthContext>,
    pub(in crate::api::mcp) audit_auth_context: Option<AuthContext>,
    pub(in crate::api::mcp) headers: HeaderMap,
    pub(in crate::api::mcp) request_context: RequestContext,
    pub(in crate::api::mcp) id: Option<Value>,
}

/// The transport envelope of one internal REST dispatch: the application
/// state, the request identity, the caller credential and the forwarded
/// headers every MCP handler needs to reach the REST router. Grouped so
/// [`dispatch_rest_request`] stays within the argument budget.
pub(in crate::api::mcp) struct RestDispatchContext {
    pub(in crate::api::mcp) state: AppState,
    pub(in crate::api::mcp) request_id: RequestId,
    pub(in crate::api::mcp) auth_context: Option<AuthContext>,
    pub(in crate::api::mcp) headers: HeaderMap,
}

// dynamic request + 200 response: MCP JSON-RPC 2.0 envelope — method/params/id
// vary per call, so both are inherently arbitrary JSON (permanent exception).
/// Model Context Protocol (JSON-RPC 2.0) endpoint.
#[utoipa::path(
    post,
    path = "/api/v1/mcp",
    tag = "mcp",
    request_body(
        content = serde_json::Value,
        description = "JSON-RPC 2.0 Model Context Protocol request.",
        content_type = "application/json"
    ),
    responses(
        (status = 200, description = "JSON-RPC 2.0 MCP response or notification acknowledgement.", body = serde_json::Value),
        (status = 400, description = "Unsupported MCP protocol version or malformed transport metadata.", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Origin rejected for the MCP endpoint.", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn mcp_handler(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> Response {
    if let Err(error) = validate_mcp_origin(&state, &headers) {
        return mcp_http_error(StatusCode::FORBIDDEN, error.as_str());
    }

    let request = match parse_json_rpc_request(payload) {
        Ok(value) => value,
        Err(error) => return jsonrpc_error(None, -32600, error.as_str(), None),
    };
    if let Err(error) = validate_mcp_protocol_version(&request, &headers) {
        return mcp_http_error(StatusCode::BAD_REQUEST, error.as_str());
    }

    let auth_context_for_dispatch = auth_context.as_ref().map(|value| value.0.clone());
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if request_context.auth_method != "api_token" {
        return jsonrpc_error(
            request.id,
            -32001,
            "MCP endpoint requires a personal API token bearer credential",
            None,
        );
    }
    if let Some(decision) =
        mcp_rate_limit_decision(&state, auth_context_for_dispatch.as_ref(), &request_context).await
        && !decision.allowed
    {
        return mcp_rate_limit_error(request.id, decision.retry_after_seconds);
    }

    match request.method.as_str() {
        "initialize" => jsonrpc_result(
            request.id,
            json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {
                    "tools": {
                        "listChanged": true
                    },
                    "resources": {
                        "listChanged": true
                    }
                },
                "serverInfo": {
                    "name": "piping",
                    "title": "Piping MCP",
                    "version": crate::PRODUCT_VERSION
                },
                "instructions": "Use piping.mcp.capabilities and resources/list first. Exposed tools and resources are token-scoped; interactive session creation requires connect:connection_target plus secret:connection_target for the target."
            }),
        ),
        "notifications/initialized" => notification_response(request.id),
        "ping" => jsonrpc_result(request.id, json!({})),
        "tools/list" => jsonrpc_result(
            request.id,
            json!({
                "tools": visible_tools(&request_context)
            }),
        ),
        "resources/list" => jsonrpc_result(
            request.id.clone(),
            match mcp_resources_list_value(&request_context, &request.params) {
                Ok(value) => value,
                Err(error) => {
                    return jsonrpc_error(request.id, -32602, error.as_str(), None);
                }
            },
        ),
        "resources/read" => {
            match read_mcp_resource(&state, &request_context, &request.params).await {
                Ok(value) => jsonrpc_result(request.id, value),
                Err(error) => jsonrpc_error(request.id, -32602, error.as_str(), None),
            }
        }
        "tools/call" => {
            handle_tool_call(
                McpToolCallContext {
                    state,
                    request_id,
                    auth_context: auth_context_for_dispatch.clone(),
                    audit_auth_context: auth_context_for_dispatch,
                    headers,
                    request_context,
                    id: request.id,
                },
                request.params,
            )
            .await
        }
        _ => jsonrpc_error(request.id, -32601, "Method not found", None),
    }
}

fn generic_rest_tool_descriptor() -> Value {
    json!({
        "name": GENERIC_REST_TOOL_NAME,
        "title": "Call Piping REST API",
        "description": "Call any allowed Piping REST endpoint through the existing API router. This tool is only exposed to API tokens with wildcard scopes for the full control-plane catalog; the underlying REST handler still performs RBAC and API-token scope checks.",
        "inputSchema": generic_rest_call_schema()
    })
}

fn has_visible_scoped_rest_endpoints(request_context: &RequestContext) -> bool {
    mcp_rest_endpoints()
        .iter()
        .any(|endpoint| scoped_rest_endpoint_visible(request_context, endpoint))
}

fn scoped_rest_list_endpoints_tool_descriptor() -> Value {
    json!({
        "name": SCOPED_REST_LIST_ENDPOINTS_TOOL_NAME,
        "title": "List allowed REST endpoints",
        "description": "List REST operations exposed to this MCP token by the backend-owned endpoint catalog.",
        "inputSchema": scoped_rest_list_endpoints_schema()
    })
}

fn scoped_rest_call_endpoint_tool_descriptor(request_context: &RequestContext) -> Value {
    json!({
        "name": SCOPED_REST_CALL_ENDPOINT_TOOL_NAME,
        "title": "Call allowed REST endpoint",
        "description": "Call one REST operation from the token-scoped endpoint catalog. The request is dispatched through the existing REST router after MCP scope checks and resource override setup.",
        "inputSchema": scoped_rest_call_endpoint_schema(request_context)
    })
}

fn visible_scoped_rest_endpoints(request_context: &RequestContext) -> Vec<McpRestEndpoint> {
    let mut endpoints = mcp_rest_endpoints()
        .into_iter()
        .filter(|endpoint| scoped_rest_endpoint_visible(request_context, endpoint))
        .collect::<Vec<_>>();
    endpoints.sort_by(|left, right| left.operation_id.cmp(&right.operation_id));
    endpoints.dedup_by(|left, right| left.operation_id == right.operation_id);
    endpoints
}

fn scoped_rest_endpoint_visible(
    request_context: &RequestContext,
    endpoint: &McpRestEndpoint,
) -> bool {
    if endpoint.operation_id == CCTV_CAPABILITIES_OPERATION_ID {
        return CCTV_SCOPE_ACTIONS
            .iter()
            .any(|action| request_context.has_api_token_scope(action, "cctv"));
    }
    if endpoint.full_control_only {
        return has_full_control_plane_scope(request_context);
    }
    if is_connection_session_create_endpoint(endpoint) {
        return has_host_interactive_session_scope(request_context);
    }
    if let Some(resource_id) = endpoint.static_resource_id.as_deref() {
        return request_context.allows_api_token_scope_for_resource(
            endpoint.action,
            endpoint.resource_type,
            Some(resource_id),
        );
    }
    match endpoint.resource_id_source {
        ResourceIdSource::None => {
            request_context.allows_api_token_scope(endpoint.action, endpoint.resource_type)
        }
        ResourceIdSource::PathParam { .. }
        | ResourceIdSource::QueryParam { .. }
        | ResourceIdSource::BodyPointer { .. } => {
            request_context.has_api_token_scope(endpoint.action, endpoint.resource_type)
        }
    }
}

fn mcp_rest_endpoint_for_operation_id(operation_id: &str) -> Option<McpRestEndpoint> {
    mcp_rest_endpoints()
        .into_iter()
        .find(|endpoint| endpoint.operation_id == operation_id)
}

fn mcp_rest_endpoints() -> Vec<McpRestEndpoint> {
    let mut endpoints = Vec::new();
    endpoints.extend(mcp_tools().into_iter().map(endpoint_from_tool));
    endpoints.extend(
        STATIC_MCP_REST_ENDPOINTS
            .iter()
            .filter_map(endpoint_from_static),
    );
    endpoints.extend(inventory_rest_endpoints());
    endpoints.extend(access_control_rest_endpoints());
    let existing_paths = endpoints
        .iter()
        .map(|endpoint| {
            (
                endpoint.method.as_str().to_ascii_uppercase(),
                endpoint.path_template.clone(),
            )
        })
        .collect::<HashSet<_>>();
    endpoints.extend(
        OPENAPI_REST_FALLBACK_ENDPOINTS
            .iter()
            .filter(|endpoint| {
                !existing_paths.contains(&(
                    endpoint.method.as_str().to_ascii_uppercase(),
                    endpoint.path_template.clone(),
                ))
            })
            .cloned(),
    );
    endpoints
}

fn endpoint_from_tool(tool: McpRestTool) -> McpRestEndpoint {
    McpRestEndpoint {
        operation_id: tool
            .name
            .strip_prefix("piping.rest.")
            .unwrap_or(tool.name)
            .to_string(),
        title: tool.title.to_string(),
        description: tool.description.to_string(),
        method: tool.method,
        path_template: tool.path_template.to_string(),
        action: tool.action,
        resource_type: tool.resource_type,
        resource_id_source: tool.resource_id_source,
        static_resource_id: None,
        full_control_only: false,
    }
}

fn is_connection_session_create_tool(tool: &McpRestTool) -> bool {
    tool.name == CREATE_CONNECTION_SESSION_TOOL_NAME
        && tool.method == Method::POST
        && tool.path_template == CONNECTION_SESSIONS_PATH
}

fn is_connection_session_create_endpoint(endpoint: &McpRestEndpoint) -> bool {
    endpoint.operation_id == CREATE_CONNECTION_SESSION_OPERATION_ID
        && endpoint.method == Method::POST
        && endpoint.path_template == CONNECTION_SESSIONS_PATH
}

fn endpoint_from_static(endpoint: &StaticMcpRestEndpoint) -> Option<McpRestEndpoint> {
    let method = method_from_str(endpoint.method).ok()?;
    Some(McpRestEndpoint {
        operation_id: endpoint.operation_id.to_string(),
        title: endpoint.title.to_string(),
        description: endpoint.description.to_string(),
        method,
        path_template: endpoint.path_template.to_string(),
        action: endpoint.action,
        resource_type: endpoint.resource_type,
        resource_id_source: endpoint.resource_id_source,
        static_resource_id: None,
        // Service principals are machine identities, deliberately excluded from
        // the human PAT scope catalog (see user_api_token_scope_catalog_pairs
        // + the playbooks_service_principal_security pin). Their MCP endpoints
        // therefore can't be granted by a scoped token; expose them only to
        // full-control tokens, which re-dispatch through the REST router where
        // RBAC still applies.
        full_control_only: endpoint.resource_type == "playbook_service_principal",
    })
}

#[allow(
    clippy::expect_used,
    reason = "the OpenAPI document is embedded at compile time via include_str!, so a parse failure or a missing paths object is a programmer error caught the first time this fallback is built, never a runtime condition; the OpenAPI contract tests fail before such a document could ship"
)]
fn openapi_rest_fallback_endpoints() -> Vec<McpRestEndpoint> {
    let yaml: Value = serde_yml::from_str(include_str!("../../contracts/piping.openapi.yaml"))
        .expect("valid OpenAPI yaml");
    let paths = yaml
        .get("paths")
        .and_then(Value::as_object)
        .expect("the OpenAPI paths object");
    let mut endpoints = Vec::new();
    let mut seen_operation_ids = HashSet::new();

    for (path_template, path_spec) in paths {
        let Some(operations) = path_spec.as_object() else {
            continue;
        };
        for (method_key, operation_spec) in operations {
            let Ok(method) = method_from_str(method_key) else {
                continue;
            };
            if !openapi_rest_fallback_path_allowed(method.as_str(), path_template.as_str()) {
                continue;
            }
            let operation_id =
                openapi_rest_fallback_operation_id(method.as_str(), path_template, operation_spec);
            let operation_id = unique_operation_id(operation_id, &mut seen_operation_ids);
            endpoints.push(McpRestEndpoint {
                operation_id,
                title: openapi_rest_fallback_title(method.as_str(), path_template, operation_spec),
                description: openapi_rest_fallback_description(
                    method.as_str(),
                    path_template,
                    operation_spec,
                ),
                method,
                path_template: path_template.clone(),
                action: openapi_rest_fallback_action(method_key),
                resource_type: "rest",
                resource_id_source: ResourceIdSource::None,
                static_resource_id: None,
                full_control_only: true,
            });
        }
    }

    endpoints
}

fn openapi_rest_fallback_path_allowed(method: &str, path_template: &str) -> bool {
    if !path_template.starts_with("/api/v1/") {
        return false;
    }
    if playbook_callback_path_is_mcp_blocked(method, path_template) {
        return false;
    }
    !mcp_rest_path_is_internal_or_protocol(path_template)
}

fn playbook_callback_path_is_mcp_blocked(method: &str, path_template: &str) -> bool {
    if !method.trim().eq_ignore_ascii_case("POST") {
        return false;
    }
    // Runner ingest callbacks authenticate with run-bound job tokens (see
    // `is_playbook_job_token_endpoint` in middleware/auth.rs), not user/API-token
    // sessions; they must never surface as MCP control-plane operations.
    matches!(
        path_template,
        "/api/v1/playbooks/runs/{run_id}/artifacts"
            | "/api/v1/playbooks/runs/{run_id}/events"
            | "/api/v1/playbooks/runs/{run_id}/completion"
            | "/api/v1/playbooks/run-groups/{group_id}/events"
            | "/api/v1/playbooks/run-groups/{group_id}/events/batch"
            | "/api/v1/playbooks/run-groups/{group_id}/completion-batch"
    )
}

fn mcp_rest_path_is_internal_or_protocol(path: &str) -> bool {
    MCP_REST_BLOCKED_EXACT_PATHS.contains(&path)
        || MCP_REST_BLOCKED_PREFIXES
            .iter()
            .any(|blocked| path == *blocked || path.starts_with(&format!("{blocked}/")))
}

fn openapi_rest_fallback_action(method: &str) -> &'static str {
    match method.trim().to_ascii_uppercase().as_str() {
        "GET" => "read",
        "POST" => "create",
        "PUT" | "PATCH" => "update",
        "DELETE" => "delete",
        _ => "execute",
    }
}

fn openapi_rest_fallback_operation_id(
    method: &str,
    path_template: &str,
    operation_spec: &Value,
) -> String {
    let raw = operation_spec
        .get("operationId")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("{}_{}", method.to_ascii_lowercase(), path_template));
    format!("rest.openapi.{}", operation_id_fragment(raw.as_str()))
}

fn unique_operation_id(operation_id: String, seen: &mut HashSet<String>) -> String {
    if seen.insert(operation_id.clone()) {
        return operation_id;
    }
    let mut suffix = 2usize;
    loop {
        let candidate = format!("{operation_id}_{suffix}");
        if seen.insert(candidate.clone()) {
            return candidate;
        }
        suffix += 1;
    }
}

fn operation_id_fragment(value: &str) -> String {
    let fragment = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches(['_', '-', '.'])
        .to_string();
    if fragment.is_empty() {
        "operation".to_string()
    } else {
        fragment
    }
}

fn openapi_rest_fallback_title(
    method: &str,
    path_template: &str,
    operation_spec: &Value,
) -> String {
    operation_spec
        .get("summary")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("{} {}", method.to_ascii_uppercase(), path_template))
}

fn openapi_rest_fallback_description(
    method: &str,
    path_template: &str,
    operation_spec: &Value,
) -> String {
    let description = operation_spec
        .get("description")
        .and_then(Value::as_str)
        .or_else(|| operation_spec.get("summary").and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("OpenAPI-backed REST operation.");
    format!(
        "{description} Full-control MCP fallback for {} {}.",
        method.to_ascii_uppercase(),
        path_template
    )
}

fn inventory_rest_endpoints() -> Vec<McpRestEndpoint> {
    let mut endpoints = Vec::new();
    for route in INVENTORY_ENTITY_ROUTES {
        for operation in [
            InventoryToolOperation::List,
            InventoryToolOperation::Get,
            InventoryToolOperation::Create,
            InventoryToolOperation::Update,
            InventoryToolOperation::Delete,
            InventoryToolOperation::BulkCreate,
            InventoryToolOperation::BulkMutate,
        ] {
            if !operation.supports_route(route) {
                continue;
            }
            if operation == InventoryToolOperation::BulkMutate {
                for action in inventory_operation_actions(operation, route) {
                    let operation_key = format!("bulk_mutate_{action}");
                    endpoints.push(McpRestEndpoint {
                        operation_id: format!("inventory.{}.{}", route.entity_type, operation_key),
                        title: format!("bulk mutate {action} {}", route.entity_type),
                        description: format!(
                            "bulk mutate {action} inventory {} through the REST API.",
                            route.entity_type
                        ),
                        method: operation.method(),
                        path_template: format!(
                            "/api/v1/inventory/resources/{}/bulk",
                            route.path_segment
                        ),
                        action,
                        resource_type: "inventory",
                        resource_id_source: ResourceIdSource::None,
                        static_resource_id: Some(inventory_entity_resource_id(route)),
                        full_control_only: false,
                    });
                }
                continue;
            }
            let operation_key = match operation {
                InventoryToolOperation::List => "list",
                InventoryToolOperation::Get => "get",
                InventoryToolOperation::Create => "create",
                InventoryToolOperation::Update => "update",
                InventoryToolOperation::Delete => "delete",
                InventoryToolOperation::BulkCreate => "bulk_create",
                // Unreachable: BulkMutate is handled and continued above, so this
                // arm exists only for exhaustiveness and emits nothing.
                InventoryToolOperation::BulkMutate => continue,
            };
            let mut path_template = format!("/api/v1/inventory/{}", route.path_segment);
            if operation == InventoryToolOperation::BulkCreate {
                path_template.push_str("/bulk");
            } else if operation.requires_id() {
                path_template.push_str("/{id}");
            }
            endpoints.push(McpRestEndpoint {
                operation_id: format!("inventory.{}.{}", route.entity_type, operation_key),
                title: format!("{} {}", operation_key.replace('_', " "), route.entity_type),
                description: format!(
                    "{} inventory {} through the REST API.",
                    operation_key.replace('_', " "),
                    route.entity_type
                ),
                method: operation.method(),
                path_template,
                action: operation.action(),
                resource_type: "inventory",
                resource_id_source: ResourceIdSource::None,
                static_resource_id: Some(inventory_entity_resource_id(route)),
                full_control_only: false,
            });
        }
    }
    endpoints
}

fn access_control_rest_endpoints() -> Vec<McpRestEndpoint> {
    let mut endpoints = Vec::new();
    for route in ACCESS_CONTROL_ENTITY_ROUTES {
        for operation in [
            AccessControlToolOperation::List,
            AccessControlToolOperation::Get,
            AccessControlToolOperation::Create,
            AccessControlToolOperation::Update,
            AccessControlToolOperation::Delete,
        ] {
            let Some(action) = operation.action() else {
                continue;
            };
            let operation_key = match operation {
                AccessControlToolOperation::List => "list",
                AccessControlToolOperation::Get => "get",
                AccessControlToolOperation::Create => "create",
                AccessControlToolOperation::Update => "update",
                AccessControlToolOperation::Delete => "delete",
                AccessControlToolOperation::Bulk => continue,
            };
            let mut path_template = format!("/api/v1/admin/access-control/{}", route.path_segment);
            let resource_id_source = if operation.requires_id() {
                path_template.push_str("/{id}");
                ResourceIdSource::PathParam {
                    key: "id",
                    prefix: route.resource_prefix,
                }
            } else {
                ResourceIdSource::None
            };
            endpoints.push(McpRestEndpoint {
                operation_id: format!("access_control.{}.{}", route.entity_type, operation_key),
                title: format!("{} {}", operation_key, route.entity_type),
                description: format!(
                    "{} access-control {} through the REST API.",
                    operation_key, route.entity_type
                ),
                method: operation.method(),
                path_template,
                action,
                resource_type: route.resource_type,
                resource_id_source,
                static_resource_id: None,
                full_control_only: false,
            });
        }
    }
    endpoints
}

fn inventory_tool_descriptors(request_context: &RequestContext) -> Vec<Value> {
    [
        InventoryToolOperation::List,
        InventoryToolOperation::Get,
        InventoryToolOperation::Create,
        InventoryToolOperation::Update,
        InventoryToolOperation::Delete,
        InventoryToolOperation::BulkCreate,
        InventoryToolOperation::BulkMutate,
    ]
    .into_iter()
    .filter(|operation| inventory_tool_visible(request_context, *operation))
    .map(|operation| inventory_tool_descriptor(request_context, operation))
    .collect()
}

fn inventory_tool_visible(
    request_context: &RequestContext,
    operation: InventoryToolOperation,
) -> bool {
    INVENTORY_ENTITY_ROUTES
        .iter()
        .any(|route| inventory_operation_visible(request_context, operation, route))
}

fn inventory_tool_descriptor(
    request_context: &RequestContext,
    operation: InventoryToolOperation,
) -> Value {
    json!({
        "name": operation.name(),
        "title": operation.title(),
        "description": operation.description(),
        "inputSchema": inventory_tool_schema(request_context, operation)
    })
}

fn access_control_tool_descriptors(request_context: &RequestContext) -> Vec<Value> {
    [
        AccessControlToolOperation::List,
        AccessControlToolOperation::Get,
        AccessControlToolOperation::Create,
        AccessControlToolOperation::Update,
        AccessControlToolOperation::Delete,
        AccessControlToolOperation::Bulk,
    ]
    .into_iter()
    .filter(|operation| access_control_tool_visible(request_context, *operation))
    .map(|operation| access_control_tool_descriptor(request_context, operation))
    .collect()
}

fn access_control_tool_visible(
    request_context: &RequestContext,
    operation: AccessControlToolOperation,
) -> bool {
    ACCESS_CONTROL_ENTITY_ROUTES
        .iter()
        .any(|route| access_control_route_visible(request_context, operation, route))
}

fn access_control_route_visible(
    request_context: &RequestContext,
    operation: AccessControlToolOperation,
    route: &AccessControlEntityRoute,
) -> bool {
    if operation == AccessControlToolOperation::Bulk {
        return ["create", "update", "delete"]
            .into_iter()
            .any(|action| request_context.allows_api_token_scope(action, route.resource_type));
    }
    let Some(action) = operation.action() else {
        return false;
    };
    if operation.broad_scope_required() {
        request_context.allows_api_token_scope(action, route.resource_type)
    } else {
        request_context.has_api_token_scope(action, route.resource_type)
    }
}

fn access_control_tool_descriptor(
    request_context: &RequestContext,
    operation: AccessControlToolOperation,
) -> Value {
    json!({
        "name": operation.name(),
        "title": operation.title(),
        "description": operation.description(),
        "inputSchema": access_control_tool_schema(request_context, operation)
    })
}

fn host_session_client_credentials_tool_descriptor() -> Value {
    json!({
        "name": HOST_SESSION_CLIENT_CREDENTIALS_TOOL_NAME,
        "title": "Get session client credentials",
        "description": "Return short-lived client credentials for a VNC or RDP connection session after verifying the session owner, connection target, and token-scoped connect+secret permissions.",
        "inputSchema": host_session_client_credentials_schema()
    })
}

#[derive(Debug)]
struct InventoryToolError {
    code: &'static str,
    message: String,
    resource_id: Option<String>,
}

impl InventoryToolError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            resource_id: None,
        }
    }

    fn with_resource_id(mut self, resource_id: String) -> Self {
        self.resource_id = Some(resource_id);
        self
    }
}

struct InventoryRestCall {
    uri: String,
    body: Body,
    action: &'static str,
    resource_id: String,
}

struct AccessControlRestCall {
    uri: String,
    body: Body,
    action: &'static str,
    resource_type: &'static str,
    resource_id: Option<String>,
}

fn inventory_rest_call(
    operation: InventoryToolOperation,
    arguments: &Value,
) -> Result<InventoryRestCall, InventoryToolError> {
    let route = inventory_route_from_arguments(arguments)?;
    if !operation.supports_route(route) {
        return Err(InventoryToolError::new(
            "UNSUPPORTED_ENTITY_OPERATION",
            format!(
                "Inventory entity '{}' does not support this MCP operation",
                route.entity_type
            ),
        )
        .with_resource_id(inventory_entity_resource_id(route)));
    }

    let action = inventory_operation_action(operation, route, arguments)?;
    let mut path = if operation == InventoryToolOperation::BulkMutate {
        format!("/api/v1/inventory/resources/{}/bulk", route.path_segment)
    } else {
        format!("/api/v1/inventory/{}", route.path_segment)
    };
    if operation == InventoryToolOperation::BulkCreate {
        path.push_str("/bulk");
    } else if operation.requires_id() {
        path.push('/');
        path.push_str(inventory_resource_path_id(arguments)?.as_str());
    }
    let uri = if operation == InventoryToolOperation::List {
        append_query(path, arguments.get("query")).map_err(|error| {
            InventoryToolError::new("INVALID_ARGUMENTS", error)
                .with_resource_id(inventory_entity_resource_id(route))
        })?
    } else {
        path
    };
    let body = inventory_request_body(operation, arguments)?;

    Ok(InventoryRestCall {
        uri,
        body,
        action,
        resource_id: inventory_entity_resource_id(route),
    })
}

fn inventory_operation_action(
    operation: InventoryToolOperation,
    route: &InventoryEntityRoute,
    arguments: &Value,
) -> Result<&'static str, InventoryToolError> {
    if operation != InventoryToolOperation::BulkMutate {
        return Ok(operation.action());
    }

    let bulk_operation = bulk_mutation_operation_from_arguments(arguments)
        .map_err(|error| error.with_resource_id(inventory_entity_resource_id(route)))?;
    let supported_operations = bulk_mutation_operations_for_route(route);
    if !supported_operations.contains(&bulk_operation.as_str()) {
        return Err(InventoryToolError::new(
            "UNSUPPORTED_BULK_OPERATION",
            format!(
                "Bulk operation '{}' is not supported for inventory entity '{}'. Supported operations: {}",
                bulk_operation,
                route.entity_type,
                supported_operations.join(", ")
            ),
        )
        .with_resource_id(inventory_entity_resource_id(route)));
    }

    bulk_mutation_action_for_operation(bulk_operation.as_str()).ok_or_else(|| {
        InventoryToolError::new(
            "UNSUPPORTED_BULK_OPERATION",
            format!("Bulk operation '{bulk_operation}' is not recognized"),
        )
        .with_resource_id(inventory_entity_resource_id(route))
    })
}

fn bulk_mutation_operation_from_arguments(arguments: &Value) -> Result<String, InventoryToolError> {
    let body = arguments
        .get("body")
        .and_then(Value::as_object)
        .ok_or_else(|| InventoryToolError::new("INVALID_ARGUMENTS", "Missing required body"))?;
    let operation = body
        .get("operation")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            InventoryToolError::new(
                "INVALID_ARGUMENTS",
                "Bulk mutation body requires non-empty string field 'operation'",
            )
        })?;
    body.get("items")
        .and_then(Value::as_array)
        .filter(|items| !items.is_empty())
        .ok_or_else(|| {
            InventoryToolError::new(
                "INVALID_ARGUMENTS",
                "Bulk mutation body requires a non-empty items array",
            )
        })?;
    Ok(operation.to_string())
}

fn inventory_route_from_arguments(
    arguments: &Value,
) -> Result<&'static InventoryEntityRoute, InventoryToolError> {
    let entity_type = arguments
        .get("entity_type")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            InventoryToolError::new("INVALID_ARGUMENTS", "Missing required entity_type")
        })?;
    inventory_route_for_entity(entity_type).ok_or_else(|| {
        InventoryToolError::new(
            "UNKNOWN_INVENTORY_ENTITY",
            format!("Inventory entity '{entity_type}' is not exposed through MCP CRUD tools"),
        )
    })
}

fn inventory_route_for_entity(entity_type: &str) -> Option<&'static InventoryEntityRoute> {
    INVENTORY_ENTITY_ROUTES
        .iter()
        .find(|route| route.entity_type == entity_type)
}

fn inventory_entity_resource_id(route: &InventoryEntityRoute) -> String {
    format!("{INVENTORY_ENTITY_RESOURCE_PREFIX}{}", route.entity_type)
}

fn inventory_resource_path_id(arguments: &Value) -> Result<String, InventoryToolError> {
    let value = arguments
        .get("id")
        .or_else(|| {
            arguments
                .get("path_params")
                .and_then(|params| params.get("id"))
        })
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| InventoryToolError::new("INVALID_ARGUMENTS", "Missing required id"))?;
    if value.contains('/') || value.contains('?') || value.contains('#') || value.contains('\\') {
        return Err(InventoryToolError::new(
            "INVALID_ARGUMENTS",
            "Inventory resource id contains an invalid path character",
        ));
    }
    Ok(value.to_string())
}

fn inventory_request_body(
    operation: InventoryToolOperation,
    arguments: &Value,
) -> Result<Body, InventoryToolError> {
    if !operation.requires_body() {
        return Ok(Body::empty());
    }
    let body = arguments
        .get("body")
        .filter(|value| !value.is_null())
        .ok_or_else(|| InventoryToolError::new("INVALID_ARGUMENTS", "Missing required body"))?;
    serde_json::to_vec(body).map(Body::from).map_err(|error| {
        InventoryToolError::new(
            "INVALID_ARGUMENTS",
            format!("Failed to serialize inventory request body: {error}"),
        )
    })
}

#[derive(Debug)]
struct AccessControlToolError {
    code: &'static str,
    message: String,
    resource_id: Option<String>,
}

impl AccessControlToolError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            resource_id: None,
        }
    }

    fn with_resource_id(mut self, resource_id: Option<String>) -> Self {
        self.resource_id = resource_id;
        self
    }
}

fn access_control_rest_call(
    operation: AccessControlToolOperation,
    arguments: &Value,
    request_context: &RequestContext,
) -> Result<AccessControlRestCall, AccessControlToolError> {
    let route = access_control_route_from_arguments(arguments)?;
    let mut path = format!("/api/v1/admin/access-control/{}", route.path_segment);
    let mut resource_id = None;

    if operation == AccessControlToolOperation::Bulk {
        path.push_str("/bulk");
    } else if operation.requires_id() {
        let id = access_control_resource_path_id(arguments)?;
        resource_id = Some(format!("{}{}", route.resource_prefix, id));
        path.push('/');
        path.push_str(id.as_str());
    }

    let action = access_control_authorize(
        operation,
        route,
        resource_id.as_deref(),
        arguments,
        request_context,
    )?;
    let uri = if operation == AccessControlToolOperation::List {
        append_query(path, arguments.get("query")).map_err(|error| {
            AccessControlToolError::new("INVALID_ARGUMENTS", error)
                .with_resource_id(resource_id.clone())
        })?
    } else {
        path
    };
    let body = access_control_request_body(operation, arguments)?;

    Ok(AccessControlRestCall {
        uri,
        body,
        action,
        resource_type: route.resource_type,
        resource_id,
    })
}

fn access_control_authorize(
    operation: AccessControlToolOperation,
    route: &AccessControlEntityRoute,
    resource_id: Option<&str>,
    arguments: &Value,
    request_context: &RequestContext,
) -> Result<&'static str, AccessControlToolError> {
    if operation == AccessControlToolOperation::Bulk {
        let actions = access_control_bulk_actions(arguments)?;
        for action in actions {
            if !request_context.allows_api_token_scope(action, route.resource_type) {
                return Err(AccessControlToolError::new(
                    "TOKEN_SCOPE_DENIED",
                    format!(
                        "API token scope does not permit bulk {action} for access-control entity '{}'",
                        route.entity_type
                    ),
                ));
            }
        }
        return Ok("bulk");
    }

    let action = operation
        .action()
        .ok_or_else(|| AccessControlToolError::new("INVALID_ARGUMENTS", "Invalid operation"))?;
    let allowed = if operation.broad_scope_required() {
        request_context.allows_api_token_scope(action, route.resource_type)
    } else {
        request_context.allows_api_token_scope_for_resource(
            action,
            route.resource_type,
            resource_id,
        )
    };
    if !allowed {
        return Err(AccessControlToolError::new(
            "TOKEN_SCOPE_DENIED",
            format!(
                "API token scope does not permit {action} for access-control entity '{}'",
                route.entity_type
            ),
        )
        .with_resource_id(resource_id.map(ToString::to_string)));
    }
    Ok(action)
}

fn access_control_bulk_actions(
    arguments: &Value,
) -> Result<Vec<&'static str>, AccessControlToolError> {
    let body = access_control_body_value(arguments)?;
    let Some(body) = body.as_object() else {
        return Err(AccessControlToolError::new(
            "INVALID_ARGUMENTS",
            "Bulk body must be a JSON object",
        ));
    };
    let mut actions = Vec::new();
    for (key, action) in [
        ("create", "create"),
        ("update", "update"),
        ("delete", "delete"),
    ] {
        if body
            .get(key)
            .and_then(Value::as_array)
            .is_some_and(|items| !items.is_empty())
        {
            actions.push(action);
        }
    }
    if actions.is_empty() {
        return Err(AccessControlToolError::new(
            "INVALID_ARGUMENTS",
            "Bulk body must include at least one non-empty create, update, or delete array",
        ));
    }
    Ok(actions)
}

fn access_control_route_from_arguments(
    arguments: &Value,
) -> Result<&'static AccessControlEntityRoute, AccessControlToolError> {
    let entity_type = arguments
        .get("entity_type")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            AccessControlToolError::new("INVALID_ARGUMENTS", "Missing required entity_type")
        })?;
    access_control_route_for_entity(entity_type).ok_or_else(|| {
        AccessControlToolError::new(
            "UNKNOWN_ACCESS_CONTROL_ENTITY",
            format!("Access-control entity '{entity_type}' is not exposed through MCP CRUD tools"),
        )
    })
}

fn access_control_route_for_entity(entity_type: &str) -> Option<&'static AccessControlEntityRoute> {
    ACCESS_CONTROL_ENTITY_ROUTES
        .iter()
        .find(|route| route.entity_type == entity_type)
}

fn access_control_resource_path_id(arguments: &Value) -> Result<String, AccessControlToolError> {
    let value = arguments
        .get("id")
        .or_else(|| {
            arguments
                .get("path_params")
                .and_then(|params| params.get("id"))
        })
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AccessControlToolError::new("INVALID_ARGUMENTS", "Missing required id"))?;
    if value.contains('/') || value.contains('?') || value.contains('#') || value.contains('\\') {
        return Err(AccessControlToolError::new(
            "INVALID_ARGUMENTS",
            "Access-control resource id contains an invalid path character",
        ));
    }
    Ok(value.to_string())
}

fn access_control_body_value(arguments: &Value) -> Result<&Value, AccessControlToolError> {
    arguments
        .get("body")
        .filter(|value| !value.is_null())
        .ok_or_else(|| AccessControlToolError::new("INVALID_ARGUMENTS", "Missing required body"))
}

fn access_control_request_body(
    operation: AccessControlToolOperation,
    arguments: &Value,
) -> Result<Body, AccessControlToolError> {
    if !operation.requires_body() {
        return Ok(Body::empty());
    }
    let body = access_control_body_value(arguments)?;
    serde_json::to_vec(body).map(Body::from).map_err(|error| {
        AccessControlToolError::new(
            "INVALID_ARGUMENTS",
            format!("Failed to serialize access-control request body: {error}"),
        )
    })
}

async fn dispatch_rest_tool(
    context: RestDispatchContext,
    tool: &McpRestTool,
    resource_id: Option<&str>,
    arguments: &Value,
) -> Result<Value, String> {
    let uri = build_rest_uri(tool, arguments)?;
    let body = request_body(arguments)?;
    let auth_context = context
        .auth_context
        .map(|auth_context| rest_dispatch_auth_context(auth_context, tool, resource_id));
    dispatch_rest_request(
        RestDispatchContext {
            state: context.state,
            request_id: context.request_id,
            auth_context,
            headers: context.headers,
        },
        tool.method.clone(),
        uri,
        body,
        None,
    )
    .await
}

async fn dispatch_rest_request(
    context: RestDispatchContext,
    method: Method,
    uri: String,
    body: Body,
    internal_connection_policy: Option<InternalConnectionSessionCreatePolicy>,
) -> Result<Value, String> {
    let RestDispatchContext {
        state,
        request_id,
        auth_context,
        headers,
    } = context;
    let mut builder = Request::builder().method(method).uri(uri);
    let Some(request_headers) = builder.headers_mut() else {
        return Err("Failed to prepare internal REST headers".to_string());
    };
    copy_forwarded_headers(&headers, request_headers);
    request_headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

    let mut request = builder
        .body(body)
        .map_err(|error| format!("Failed to build internal REST request: {error}"))?;
    request.extensions_mut().insert(request_id);
    if let Some(auth_context) = auth_context {
        request.extensions_mut().insert(auth_context);
    }
    if let Some(policy) = internal_connection_policy {
        request.extensions_mut().insert(policy);
    }

    let response = build_rest_router()
        .with_state(state)
        .oneshot(request)
        .await
        .map_err(|error| format!("Internal REST dispatch failed: {error}"))?;

    map_rest_response(response).await
}

fn rest_dispatch_auth_context(
    auth_context: AuthContext,
    tool: &McpRestTool,
    resource_id: Option<&str>,
) -> AuthContext {
    if let Some(resource_id) = resource_id {
        return auth_context_with_resource_override(
            auth_context,
            tool.action,
            tool.resource_type,
            resource_id.to_string(),
        );
    }
    auth_context
}

fn build_rest_uri(tool: &McpRestTool, arguments: &Value) -> Result<String, String> {
    let path = fill_path_template(tool.path_template, arguments)?;
    append_query(path, arguments.get("query"))
}

fn fill_path_template(template: &str, arguments: &Value) -> Result<String, String> {
    let mut path = template.to_string();
    for key in path_template_keys(template) {
        let value = path_param(arguments, key)
            .ok_or_else(|| format!("Missing required path parameter '{key}'"))?;
        if value.contains('/') || value.contains('?') || value.contains('#') {
            return Err(format!(
                "Path parameter '{key}' contains an invalid character"
            ));
        }
        path = path.replace(format!("{{{key}}}").as_str(), value);
    }
    Ok(path)
}

fn path_template_keys(template: &str) -> Vec<&str> {
    let mut keys = Vec::new();
    let mut rest = template;
    while let Some((_, after_open)) = rest.split_once('{') {
        if let Some((key, after_close)) = after_open.split_once('}') {
            keys.push(key);
            rest = after_close;
        } else {
            break;
        }
    }
    keys
}

fn path_param<'a>(arguments: &'a Value, key: &str) -> Option<&'a str> {
    arguments
        .get("path_params")
        .and_then(|value| value.get(key))
        .or_else(|| arguments.get(key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn query_param<'a>(arguments: &'a Value, key: &str) -> Option<&'a str> {
    arguments
        .get("query")
        .and_then(|value| value.get(key))
        .or_else(|| arguments.get(key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn append_query(mut path: String, query: Option<&Value>) -> Result<String, String> {
    let Some(query) = query else {
        return Ok(path);
    };
    let Some(query) = query.as_object() else {
        return Err("'query' must be an object".to_string());
    };
    if query.is_empty() {
        return Ok(path);
    }

    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for (key, value) in query {
        append_query_value(&mut serializer, key, value);
    }
    let encoded = serializer.finish();
    if !encoded.is_empty() {
        path.push('?');
        path.push_str(encoded.as_str());
    }
    Ok(path)
}

fn append_query_value(
    serializer: &mut url::form_urlencoded::Serializer<'_, String>,
    key: &str,
    value: &Value,
) {
    match value {
        Value::Null => {}
        Value::Array(values) => {
            for item in values {
                append_query_value(serializer, key, item);
            }
        }
        Value::Bool(value) => {
            serializer.append_pair(key, value.to_string().as_str());
        }
        Value::Number(value) => {
            serializer.append_pair(key, value.to_string().as_str());
        }
        Value::String(value) => {
            serializer.append_pair(key, value);
        }
        Value::Object(_) => {
            serializer.append_pair(key, value.to_string().as_str());
        }
    }
}

fn request_body(arguments: &Value) -> Result<Body, String> {
    let Some(body) = arguments.get("body") else {
        return Ok(Body::empty());
    };
    if body.is_null() {
        return Ok(Body::empty());
    }
    serde_json::to_vec(body)
        .map(Body::from)
        .map_err(|error| format!("Failed to serialize request body: {error}"))
}

fn copy_forwarded_headers(source: &HeaderMap, target: &mut HeaderMap) {
    for name in [
        HOST,
        USER_AGENT,
        ACCEPT,
        HeaderName::from_static("x-forwarded-for"),
        HeaderName::from_static("x-forwarded-host"),
        HeaderName::from_static("x-forwarded-port"),
        HeaderName::from_static("x-forwarded-proto"),
        HeaderName::from_static("x-real-ip"),
        HeaderName::from_static("x-request-id"),
    ] {
        if let Some(value) = source.get(&name) {
            target.insert(name, value.clone());
        }
    }
}

async fn map_rest_response(response: Response) -> Result<Value, String> {
    let (parts, body) = response.into_parts();
    let bytes = to_bytes(body, MCP_RESPONSE_BODY_LIMIT_BYTES)
        .await
        .map_err(|error| format!("Failed to read internal REST response: {error}"))?;
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).to_string()))
    };

    let mut headers = Map::new();
    if let Some(content_type) = parts
        .headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
    {
        headers.insert(
            "content-type".to_string(),
            Value::String(content_type.to_string()),
        );
    }

    Ok(json!({
        "status": parts.status.as_u16(),
        "headers": headers,
        "body": body
    }))
}

/// What one MCP tool invocation did and how it was decided; recorded as the
/// metadata of a `mcp.tool.invoked` audit event.
struct McpToolAudit<'a> {
    tool_name: &'a str,
    action: &'a str,
    resource_type: &'a str,
    resource_id: Option<&'a str>,
    decision: &'a str,
    rest_status: Option<u16>,
    error_code: Option<&'a str>,
}

async fn record_mcp_tool_audit(
    state: &AppState,
    auth_context: Option<&AuthContext>,
    request_context: &RequestContext,
    audit: McpToolAudit<'_>,
) -> Result<(), crate::errors::AppError> {
    let McpToolAudit {
        tool_name,
        action,
        resource_type,
        resource_id,
        decision,
        rest_status,
        error_code,
    } = audit;
    let actor_id = request_context.user_id;
    let token_id = auth_context.and_then(|context| context.api_token_id.as_deref());
    let target_id = token_id.and_then(parse_uuid_or_none);
    let tool_group = mcp_tool_group(tool_name);
    let protocol = mcp_tool_protocol(tool_name);
    let risk = mcp_tool_risk(tool_name);
    let connection_target_id = connection_target_id_from_resource_id(resource_id);

    let event = AuditEvent {
        id: Uuid::now_v7(),
        event_type: "mcp.tool.invoked".to_string(),
        actor_id,
        actor_kind: "mcp_agent".to_string(),
        target_type: Some("user_api_token".to_string()),
        target_id,
        metadata: json!({
            "token_id": token_id,
            "tool": tool_name,
            "tool_group": tool_group,
            "action": action,
            "resource_type": resource_type,
            "resource_id": resource_id,
            "connection_target_id": connection_target_id,
            "protocol": protocol,
            "risk": risk,
            "decision": decision,
            "rest_status": rest_status,
            "error_code": error_code,
            "correlation_id": request_context.correlation_id.as_str(),
        }),
        created_at: Utc::now(),
    };

    state
        .services
        .audit
        .record_event(event)
        .await
        .inspect_err(|error| {
            log_error_event(
                "mcp.tool.audit_record.failed",
                "mcp_audit_record_error",
                error.to_string(),
                EventFields::default()
                    .with_operation("mcp.tool.record_audit")
                    .with_resource("mcp_tool", tool_name)
                    .with_status(decision)
                    .with_message("Failed to record MCP tool audit event"),
            );
        })
}

// CCTV tools encode their resource id as `cctv:<target_id>`. Strip that prefix
// (and the generic connection-target prefix) so audit rows share one target id.
fn connection_target_id_from_resource_id(resource_id: Option<&str>) -> Option<&str> {
    resource_id.and_then(|value| {
        value
            .strip_prefix("connection_target:")
            .or_else(|| value.strip_prefix("cctv:"))
    })
}

fn mcp_tool_group(tool_name: &str) -> &'static str {
    if tool_name == MCP_CAPABILITIES_TOOL_NAME {
        "mcp"
    } else if tool_name.starts_with("piping.rest.") || tool_name == GENERIC_REST_TOOL_NAME {
        "rest"
    } else if tool_name.starts_with("piping.inventory.") {
        "inventory"
    } else if tool_name.starts_with("piping.access_control.") {
        "access_control"
    } else if matches!(
        tool_name,
        HOST_INSPECT_TOOL_NAME
            | HOST_RESOLVE_CONNECTION_TOOL_NAME
            | HOST_CHECK_PORT_TOOL_NAME
            | HOST_READ_FILE_TOOL_NAME
            | HOST_FIND_LOGS_TOOL_NAME
    ) {
        "target_diagnostics"
    } else if tool_name.starts_with("piping.connection_target.ssh_") {
        "ssh"
    } else if matches!(tool_name, CCTV_SNAPSHOT_TOOL_NAME | CCTV_EXPORT_TOOL_NAME) {
        "cctv"
    } else if tool_name == HOST_VNC_CAPTURE_TOOL_NAME {
        "vnc_observe"
    } else if matches!(
        tool_name,
        HOST_VNC_OPEN_TOOL_NAME
            | HOST_VNC_POINTER_TOOL_NAME
            | HOST_VNC_KEY_TOOL_NAME
            | HOST_VNC_TEXT_TOOL_NAME
            | HOST_VNC_CLOSE_TOOL_NAME
    ) {
        "vnc_control"
    } else if tool_name == HOST_SESSION_CLIENT_CREDENTIALS_TOOL_NAME {
        "visual_credentials"
    } else {
        "other"
    }
}

fn mcp_tool_protocol(tool_name: &str) -> Option<&'static str> {
    if tool_name.starts_with("piping.connection_target.ssh_")
        || tool_name == HOST_RESOLVE_CONNECTION_TOOL_NAME
        || matches!(
            tool_name,
            HOST_INSPECT_TOOL_NAME
                | HOST_CHECK_PORT_TOOL_NAME
                | HOST_READ_FILE_TOOL_NAME
                | HOST_FIND_LOGS_TOOL_NAME
        )
    {
        Some("ssh")
    } else if tool_name.starts_with("piping.connection_target.vnc_") {
        Some("vnc")
    } else {
        None
    }
}

fn mcp_tool_risk(tool_name: &str) -> &'static str {
    if matches!(
        tool_name,
        CREATE_CONNECTION_SESSION_TOOL_NAME
            | HOST_SSH_OPEN_TOOL_NAME
            | HOST_SSH_EXEC_TOOL_NAME
            | HOST_SSH_TAIL_LOG_TOOL_NAME
            | HOST_INSPECT_TOOL_NAME
            | HOST_CHECK_PORT_TOOL_NAME
            | HOST_READ_FILE_TOOL_NAME
            | HOST_FIND_LOGS_TOOL_NAME
            | HOST_VNC_OPEN_TOOL_NAME
            | HOST_SESSION_CLIENT_CREDENTIALS_TOOL_NAME
    ) {
        "interactive_session"
    } else if matches!(
        tool_name,
        HOST_SSH_WRITE_TOOL_NAME
            | HOST_VNC_POINTER_TOOL_NAME
            | HOST_VNC_KEY_TOOL_NAME
            | HOST_VNC_TEXT_TOOL_NAME
    ) {
        "control"
    } else if matches!(
        tool_name,
        HOST_SSH_READ_TOOL_NAME
            | HOST_SSH_READ_UNTIL_TOOL_NAME
            | HOST_SSH_WAIT_FOR_PROMPT_TOOL_NAME
            | HOST_SSH_WAIT_FOR_QUIET_TOOL_NAME
            | HOST_RESOLVE_CONNECTION_TOOL_NAME
            | HOST_VNC_CAPTURE_TOOL_NAME
            | MCP_CAPABILITIES_TOOL_NAME
            | SCOPED_REST_LIST_ENDPOINTS_TOOL_NAME
    ) {
        "read"
    } else {
        "rest"
    }
}

fn parse_uuid_or_none(value: &str) -> Option<Uuid> {
    Uuid::parse_str(value).ok()
}

fn rest_status(value: &Value) -> Option<u16> {
    value
        .get("status")
        .and_then(Value::as_u64)
        .and_then(|status| u16::try_from(status).ok())
}

fn rest_decision(status: Option<u16>) -> &'static str {
    match status {
        Some(200..=399) => "allowed",
        Some(401 | 403) => "rest_denied",
        Some(_) => "rest_error",
        None => "rest_unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;

    #[test]
    fn user_api_token_scope_catalog_includes_observe_connection_target() {
        assert!(
            user_api_token_scope_catalog_pairs()
                .contains(&(Action::Observe, ResourceType::ConnectionTarget))
        );
        assert_eq!(action_key(&Action::Observe), "observe");
    }

    #[test]
    fn mcp_rate_limit_key_prefers_api_token_id() {
        let auth_context = AuthContext {
            user_id: Some(uuid::Uuid::nil()),
            auth_method: "api_token".to_string(),
            oidc_groups: Vec::new(),
            api_token_id: Some("token-123".to_string()),
            api_token_scopes: Vec::new(),
            api_token_scope_resource_override: None,
        };
        let request_context = RequestContext::new(
            Some(uuid::Uuid::nil()),
            "api_token".to_string(),
            Some("192.0.2.10".to_string()),
            "request".to_string(),
            crate::contexts::shared::request_context::RequestAuth::default(),
        );

        assert_eq!(
            mcp_rate_limit_key(Some(&auth_context), &request_context),
            "token:token-123"
        );
    }

    #[tokio::test]
    async fn mcp_rate_limiter_enforces_per_key_window() {
        let limiter = McpRateLimiter::new();
        let now = Instant::now();

        assert!(
            limiter
                .record_attempt_at("token:a".to_string(), 2, Duration::from_secs(60), now)
                .await
                .allowed
        );
        assert!(
            limiter
                .record_attempt_at("token:a".to_string(), 2, Duration::from_secs(60), now)
                .await
                .allowed
        );
        let denied = limiter
            .record_attempt_at(
                "token:a".to_string(),
                2,
                Duration::from_secs(60),
                now + Duration::from_secs(10),
            )
            .await;
        assert!(!denied.allowed);
        assert_eq!(denied.retry_after_seconds, 50);
        let fractional_denied = limiter
            .record_attempt_at(
                "token:a".to_string(),
                2,
                Duration::from_secs(60),
                now + Duration::from_millis(10_500),
            )
            .await;
        assert!(!fractional_denied.allowed);
        assert_eq!(fractional_denied.retry_after_seconds, 50);

        assert!(
            limiter
                .record_attempt_at("token:b".to_string(), 2, Duration::from_secs(60), now)
                .await
                .allowed
        );
    }

    #[tokio::test]
    async fn mcp_rate_limiter_allows_after_window_expires() {
        let limiter = McpRateLimiter::new();
        let now = Instant::now();

        for _ in 0..2 {
            assert!(
                limiter
                    .record_attempt_at("token:a".to_string(), 2, Duration::from_secs(60), now)
                    .await
                    .allowed
            );
        }

        assert!(
            limiter
                .record_attempt_at(
                    "token:a".to_string(),
                    2,
                    Duration::from_secs(60),
                    now + Duration::from_secs(61),
                )
                .await
                .allowed
        );
    }

    #[test]
    fn mcp_origin_validation_uses_configured_origins_or_local_dev_only() {
        let allowed = vec!["https://app.example.test".to_string()];
        assert!(mcp_origin_allowed("https://app.example.test", &allowed));
        assert!(mcp_origin_allowed("https://app.example.test/", &allowed));
        assert!(!mcp_origin_allowed("https://evil.example.test", &allowed));

        let dev_allowed = Vec::new();
        assert!(mcp_origin_allowed("http://localhost:5173", &dev_allowed));
        assert!(mcp_origin_allowed("http://127.0.0.1:5173", &dev_allowed));
        assert!(!mcp_origin_allowed(
            "https://evil.example.test",
            &dev_allowed
        ));
    }

    #[test]
    fn mcp_protocol_version_rejects_unknown_non_initialize_versions() {
        let mut headers = HeaderMap::new();
        headers.insert(
            MCP_PROTOCOL_VERSION_HEADER,
            HeaderValue::from_static("1999-01-01"),
        );
        let request = super::JsonRpcRequest {
            jsonrpc: Some("2.0".to_string()),
            method: "tools/list".to_string(),
            params: json!({}),
            id: Some(json!(1)),
        };
        assert!(validate_mcp_protocol_version(&request, &headers).is_err());

        let initialize = super::JsonRpcRequest {
            method: "initialize".to_string(),
            ..request
        };
        assert!(validate_mcp_protocol_version(&initialize, &headers).is_ok());
    }

    #[test]
    fn mcp_capabilities_tool_and_resources_are_scope_filtered() {
        let request_context = context("read", "inventory", "inventory_entity:devices");
        let tool_names = visible_tool_names(&request_context);
        assert!(tool_names.contains(&MCP_CAPABILITIES_TOOL_NAME.to_string()));

        let resources = visible_mcp_resources(&request_context);
        assert!(resources.iter().any(|resource| {
            resource.get("uri").and_then(|value| value.as_str()) == Some(MCP_RESOURCE_TOOLS_URI)
        }));
        assert!(!resources.iter().any(|resource| {
            resource.get("uri").and_then(|value| value.as_str())
                == Some(MCP_RESOURCE_INVENTORY_ENTITIES_URI)
        }));

        let full_resources = visible_mcp_resources_full(&request_context);
        assert!(full_resources.iter().any(|resource| {
            resource.get("uri").and_then(|value| value.as_str())
                == Some(MCP_RESOURCE_INVENTORY_ENTITIES_URI)
        }));
        assert!(full_resources.iter().any(|resource| {
            resource
                .get("uri")
                .and_then(|value| value.as_str())
                .is_some_and(|uri| uri.ends_with("/devices"))
        }));
        assert!(!full_resources.iter().any(|resource| {
            resource
                .get("uri")
                .and_then(|value| value.as_str())
                .is_some_and(|uri| uri.ends_with("/sites"))
        }));

        let capabilities = mcp_capabilities_value(&request_context)
            .expect("capabilities resource uses valid arguments");
        assert_eq!(
            capabilities.pointer("/safety/rdp_observe_act/supported"),
            Some(&json!(false))
        );

        let read =
            read_static_mcp_resource(&request_context, "piping://inventory/entities/devices")
                .expect("devices resource is visible");
        let text = read
            .pointer("/contents/0/text")
            .and_then(|value| value.as_str())
            .expect("resource text");
        assert!(text.contains("inventory_entity:devices"));
        assert!(text.contains("one backing source for /api/v1/connections/targets"));

        assert!(
            read_static_mcp_resource(&request_context, "piping://inventory/entities/sites")
                .is_err()
        );
    }

    #[test]
    fn resources_list_defaults_to_compact_and_validates_params() {
        let request_context = context("read", "inventory", "inventory_entity:devices");

        let compact =
            mcp_resources_list_value(&request_context, &json!({})).expect("compact resources/list");
        assert_eq!(compact.get("full"), Some(&json!(false)));
        assert!(compact["resources"].as_array().is_some_and(|resources| {
            resources.iter().any(|resource| {
                resource.get("uri").and_then(|value| value.as_str()) == Some(MCP_RESOURCE_TOOLS_URI)
            }) && !resources.iter().any(|resource| {
                resource.get("uri").and_then(|value| value.as_str())
                    == Some(MCP_RESOURCE_INVENTORY_ENTITIES_URI)
            })
        }));

        let full = mcp_resources_list_value(&request_context, &json!({ "full": true }))
            .expect("full resources/list");
        assert_eq!(full.get("full"), Some(&json!(true)));
        assert!(full["resources"].as_array().is_some_and(|resources| {
            resources.iter().any(|resource| {
                resource.get("uri").and_then(|value| value.as_str())
                    == Some(MCP_RESOURCE_INVENTORY_ENTITIES_URI)
            })
        }));

        assert!(mcp_resources_list_value(&request_context, &json!({ "unknown": true })).is_err());
        assert!(mcp_resources_list_value(&request_context, &json!({ "limit": 0 })).is_err());
        assert!(mcp_resources_list_value(&request_context, &json!({ "limit": 1001 })).is_err());
    }

    #[test]
    fn mcp_tool_argument_validation_rejects_unknowns_and_limit_overflow() {
        let schema = host_ssh_exec_schema();
        let valid = json!({
            "target_id": "018f6e6a-7d00-7000-8000-000000000001",
            "command": "uptime",
            "max_bytes": 4096
        });
        assert!(validate_mcp_tool_arguments(&schema, &valid).is_ok());

        // The old nested `body.target_id` shape is rejected by the flat schema.
        let nested = json!({
            "body": { "target_id": "018f6e6a-7d00-7000-8000-000000000001" },
            "command": "uptime"
        });
        assert!(validate_mcp_tool_arguments(&schema, &nested).is_err());

        let mut unknown = valid.clone();
        unknown["surprise"] = json!(true);
        assert!(validate_mcp_tool_arguments(&schema, &unknown).is_err());

        let mut oversized = valid;
        oversized["max_bytes"] = json!(999_999_999);
        assert!(validate_mcp_tool_arguments(&schema, &oversized).is_err());
    }

    #[test]
    fn connection_target_id_from_resource_id_strips_cctv_and_connection_target_prefixes() {
        // Reads (connection_target.*) scope by "connection_target:<id>".
        assert_eq!(
            connection_target_id_from_resource_id(Some("connection_target:abc-123")),
            Some("abc-123")
        );
        // CCTV device-write tools (PTZ, user create/update/delete) scope by
        // "cctv:<id>" instead — this is the regression this test locks in.
        assert_eq!(
            connection_target_id_from_resource_id(Some("cctv:abc-123")),
            Some("abc-123")
        );
        assert_eq!(
            connection_target_id_from_resource_id(Some("other:abc-123")),
            None
        );
        assert_eq!(connection_target_id_from_resource_id(None), None);
    }
}
