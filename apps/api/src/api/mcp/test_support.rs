pub(in crate::api::mcp) use super::{
    ACCESS_CONTROL_GET_TOOL_NAME, ACCESS_CONTROL_LIST_TOOL_NAME, AccessControlToolOperation,
    CCTV_CAPABILITIES_OPERATION_ID, CCTV_EXPORT_TOOL_NAME, CCTV_SCOPE_ACTIONS,
    CCTV_SNAPSHOT_TOOL_NAME, CREATE_CONNECTION_SESSION_OPERATION_ID,
    CREATE_CONNECTION_SESSION_TOOL_NAME, GENERIC_REST_TOOL_NAME, HOST_CHECK_PORT_TOOL_NAME,
    HOST_FIND_LOGS_TOOL_NAME, HOST_INSPECT_TOOL_NAME, HOST_READ_FILE_TOOL_NAME,
    HOST_SESSION_CLIENT_CREDENTIALS_TOOL_NAME, HOST_SSH_CLOSE_TOOL_NAME, HOST_SSH_EXEC_TOOL_NAME,
    HOST_SSH_OPEN_TOOL_NAME, HOST_SSH_READ_TOOL_NAME, HOST_SSH_READ_UNTIL_TOOL_NAME,
    HOST_SSH_RESIZE_TOOL_NAME, HOST_SSH_TAIL_LOG_TOOL_NAME, HOST_SSH_WAIT_FOR_PROMPT_TOOL_NAME,
    HOST_SSH_WRITE_TOOL_NAME, HOST_VNC_CAPTURE_TOOL_NAME, HOST_VNC_KEY_TOOL_NAME,
    HOST_VNC_OPEN_TOOL_NAME, HOST_VNC_POINTER_TOOL_NAME, HOST_VNC_TEXT_TOOL_NAME,
    INVENTORY_BULK_CREATE_TOOL_NAME, INVENTORY_BULK_MUTATE_TOOL_NAME, INVENTORY_CREATE_TOOL_NAME,
    INVENTORY_GET_TOOL_NAME, INVENTORY_LIST_TOOL_NAME, InventoryToolOperation,
    MCP_CAPABILITIES_TOOL_NAME, MCP_PROTOCOL_VERSION_HEADER, MCP_RESOURCE_INVENTORY_ENTITIES_URI,
    MCP_RESOURCE_TOOLS_URI, McpRateLimiter, ResourceIdSource, SCOPED_REST_CALL_ENDPOINT_TOOL_NAME,
    SCOPED_REST_LIST_ENDPOINTS_TOOL_NAME, SshResponseMode, VncCaptureToolResult,
    access_control_rest_call, action_key, allows_host_interactive_session_for_resource,
    allows_host_visual_control_for_resource, connection_target_id_from_resource_id,
    generic_rest_is_connection_session_create, generic_rest_uri, has_full_control_plane_scope,
    has_host_interactive_session_scope, has_host_session_client_credentials_scope,
    has_host_ssh_scope, has_host_visual_control_scope, has_host_visual_scope, host_ssh_exec_schema,
    host_ssh_open_schema, host_ssh_read_schema, host_ssh_target_id, host_vnc_capture_schema,
    host_vnc_open_schema, host_vnc_target_id, inventory_entity_resource_id, inventory_rest_call,
    inventory_route_for_entity, is_host_vnc_control_tool, mcp_capabilities_value,
    mcp_origin_allowed, mcp_rate_limit_key, mcp_resources_list_value,
    mcp_rest_endpoint_for_operation_id, mcp_rest_endpoints, mcp_tool_group, mcp_tool_protocol,
    mcp_tool_risk, mcp_tools, method_from_str, openapi_rest_fallback_path_allowed,
    parse_cctv_snapshot_resource_uri, parse_ssh_response_mode, parse_u8_argument,
    parse_u16_argument, parse_u32_argument, parse_vnc_capture_format,
    parse_vnc_capture_resource_uri, read_static_mcp_resource, resource_id_for_endpoint,
    resource_id_for_scope, rest_dispatch_auth_context, rest_endpoint_dispatch_auth_context,
    shape_ssh_terminal_response, shell_quote, ssh_tool_creates_connection_session,
    strip_ansi_codes, tool_visible, validate_find_pattern, validate_mcp_protocol_version,
    validate_mcp_tool_arguments, validate_probe_host, validate_safe_remote_path,
    validate_ssh_open_body, validate_vnc_open_body, visible_mcp_resources,
    visible_mcp_resources_full, visible_scoped_rest_endpoints, visible_tools,
    vnc_capture_tool_success,
};
pub(in crate::api::mcp) use crate::api::middleware::auth::AuthContext;
pub(in crate::api::mcp) use crate::contexts::access_control::application::user_api_token_scope_catalog_pairs;
pub(in crate::api::mcp) use crate::contexts::access_control::domain::{Action, ResourceType};
pub(in crate::api::mcp) use crate::contexts::shared::request_context::{
    ApiTokenScopeContext, RequestContext,
};
pub(in crate::api::mcp) use axum::http::{HeaderMap, HeaderValue, Method};
pub(in crate::api::mcp) use serde_json::json;
pub(in crate::api::mcp) use std::collections::HashSet;
pub(in crate::api::mcp) use std::time::{Duration, Instant};

pub(in crate::api::mcp) fn context(
    action: &str,
    resource_type: &str,
    resource_pattern: &str,
) -> RequestContext {
    context_with_scopes(vec![ApiTokenScopeContext {
        action: action.to_string(),
        resource_type: resource_type.to_string(),
        resource_pattern: resource_pattern.to_string(),
    }])
}

pub(in crate::api::mcp) fn context_with_scopes(
    scopes: Vec<ApiTokenScopeContext>,
) -> RequestContext {
    RequestContext::new(
        Some(uuid::Uuid::nil()),
        "api_token".to_string(),
        None,
        "request".to_string(),
        crate::contexts::shared::request_context::RequestAuth {
            oidc_groups: Vec::new(),
            api_token_scopes: scopes,
            api_token_scope_resource_override: None,
        },
    )
}

pub(in crate::api::mcp) fn full_context() -> RequestContext {
    RequestContext::new(
        Some(uuid::Uuid::nil()),
        "api_token".to_string(),
        None,
        "request".to_string(),
        crate::contexts::shared::request_context::RequestAuth {
            oidc_groups: Vec::new(),
            api_token_scopes: user_api_token_scope_catalog_pairs()
                .iter()
                .map(|(action, resource_type)| ApiTokenScopeContext {
                    action: action_key(action).to_string(),
                    resource_type: resource_type.as_str().to_string(),
                    resource_pattern: "*".to_string(),
                })
                .collect(),
            api_token_scope_resource_override: None,
        },
    )
}

pub(in crate::api::mcp) fn visible_tool_names(request_context: &RequestContext) -> Vec<String> {
    visible_tools(request_context)
        .into_iter()
        .filter_map(|tool| {
            tool.get("name")
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string)
        })
        .collect()
}
