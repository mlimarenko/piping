//! Tool-list visibility and MCP capabilities descriptors, split out of `mcp.rs`.
//!
//! Moved verbatim; behaviour is unchanged.

use super::*;

pub(super) fn visible_tools(request_context: &RequestContext) -> Vec<Value> {
    let mut tools = vec![mcp_capabilities_tool_descriptor()];
    tools.extend(
        mcp_tools()
            .into_iter()
            .filter(|tool| tool_visible(request_context, tool))
            .map(|tool| {
                json!({
                    "name": tool.name,
                    "title": tool.title,
                    "description": tool.description,
                    "inputSchema": (tool.input_schema)()
                })
            }),
    );
    if has_visible_scoped_rest_endpoints(request_context) {
        tools.push(scoped_rest_list_endpoints_tool_descriptor());
        tools.push(scoped_rest_call_endpoint_tool_descriptor(request_context));
    }
    tools.extend(inventory_tool_descriptors(request_context));
    tools.extend(access_control_tool_descriptors(request_context));
    if has_host_session_client_credentials_scope(request_context) {
        tools.push(host_session_client_credentials_tool_descriptor());
    }
    if has_host_ssh_scope(request_context) {
        tools.extend(host_ssh_tool_descriptors());
    }
    tools.extend(host_vnc_tool_descriptors(request_context));
    tools.extend(cctv_media_tool_descriptors(request_context));
    if has_full_control_plane_scope(request_context) {
        tools.push(generic_rest_tool_descriptor());
    }
    tools
}

pub(super) fn mcp_capabilities_tool_descriptor() -> Value {
    json!({
        "name": MCP_CAPABILITIES_TOOL_NAME,
        "title": "Describe Piping MCP capabilities",
        "description": "Return token-scoped MCP server capabilities, limits, tool groups, resource URIs, and safety notes. Call this first when bootstrapping an agent.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "summary": {
                    "type": "boolean",
                    "default": true,
                    "description": "Return compact counts and limits instead of full arrays."
                },
                "include_tools": {
                    "type": "boolean",
                    "default": false
                },
                "include_scopes": {
                    "type": "boolean",
                    "default": false
                },
                "tool_group": {
                    "type": "string",
                    "description": "When tools are included, return only this MCP tool group."
                },
                "max_items": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 1000,
                    "default": 50
                },
                "full": {
                    "type": "boolean",
                    "default": false,
                    "description": "Explicitly include full visible tools, resources, and token scopes."
                }
            },
            "additionalProperties": false
        }
    })
}

pub(super) fn mcp_capabilities_value(request_context: &RequestContext) -> Result<Value, String> {
    // The arguments are the hardcoded full-capabilities object, so this cannot
    // hit the malformed-caller-arguments error paths of `_for_args`.
    mcp_capabilities_value_for_args(request_context, &json!({ "full": true }))
}

pub(super) fn mcp_capabilities_value_for_args(
    request_context: &RequestContext,
    arguments: &Value,
) -> Result<Value, String> {
    let full = arguments
        .get("full")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let summary = arguments
        .get("summary")
        .and_then(Value::as_bool)
        .unwrap_or(!full);
    let include_tools = full
        || arguments
            .get("include_tools")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    let include_scopes = full
        || arguments
            .get("include_scopes")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    let max_items = optional_mcp_usize_param(
        arguments,
        "max_items",
        if full { 1000 } else { 50 },
        1,
        1000,
    )?;
    let tool_group = arguments
        .get("tool_group")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());

    let tools = visible_tools(request_context);
    let filtered_tools = tools
        .iter()
        .filter(|tool| {
            tool_group.is_none_or(|group| {
                tool.get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|name| mcp_tool_group(name) == group)
            })
        })
        .take(max_items)
        .cloned()
        .collect::<Vec<_>>();
    let resources = visible_mcp_resources_full(request_context);
    let tool_groups = [
        "mcp",
        "rest",
        "inventory",
        "access_control",
        "ssh",
        "target_diagnostics",
        "vnc_observe",
        "vnc_control",
        "visual_credentials",
    ]
    .into_iter()
    .map(|group| {
        let count = tools
            .iter()
            .filter(|tool| {
                tool.get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|name| mcp_tool_group(name) == group)
            })
            .count();
        json!({
            "group": group,
            "visible": count > 0,
            "tool_count": count
        })
    })
    .collect::<Vec<_>>();
    let scopes = request_context
        .api_token_scopes
        .iter()
        .take(max_items)
        .map(|scope| {
            json!({
                "action": scope.action,
                "resource_type": scope.resource_type,
                "resource_pattern": scope.resource_pattern
            })
        })
        .collect::<Vec<_>>();

    let mut value = json!({
        "protocol": {
            "version": MCP_PROTOCOL_VERSION,
            "legacy_version": MCP_PRIOR_PROTOCOL_VERSION // allow: canonical-vocab
        },
        "server": {
            "name": "piping",
            "version": crate::PRODUCT_VERSION
        },
        "limits": {
            "mcp_response_body_bytes": MCP_RESPONSE_BODY_LIMIT_BYTES,
            "ssh_read_default_bytes": DEFAULT_SSH_READ_MAX_BYTES,
            "ssh_read_max_bytes": MAX_SSH_READ_MAX_BYTES,
            "ssh_exec_default_timeout_ms": DEFAULT_SSH_EXEC_TIMEOUT_MS,
            "ssh_exec_max_timeout_ms": MAX_SSH_EXEC_TIMEOUT_MS,
            "vnc_text_max_bytes": MAX_VNC_TEXT_BYTES,
            "rate_limit_window_seconds": MCP_RATE_LIMIT_WINDOW_SECS
        },
        "safety": {
            "interactive_session_creation": "requires connect:connection_target and secret:connection_target for the concrete connection target",
            "ssh_diagnostics": "fixed command templates only; arbitrary shell stays in piping.connection_target.ssh_exec",
            "rdp_observe_act": {
                "supported": false,
                "reason": "RDP uses the browser Guacamole path today; MCP observe-act tools expose VNC only until a backend guacd bridge is added."
            }
        },
        "tool_groups": tool_groups,
        "summary": {
            "compact": summary,
            "tool_count": tools.len(),
            "resource_count": resources.len(),
            "api_token_scope_count": request_context.api_token_scopes.len(),
            "returned_tool_count": if include_tools { filtered_tools.len() } else { 0 },
            "returned_scope_count": if include_scopes { scopes.len() } else { 0 },
            "max_items": max_items,
            "tool_group": tool_group
        }
    });
    if include_tools {
        value["tools"] = Value::Array(filtered_tools);
    }
    if full {
        value["resources"] = Value::Array(resources);
    }
    if include_scopes {
        value["api_token_scopes"] = Value::Array(scopes);
    }
    Ok(value)
}

pub(super) fn optional_mcp_usize_param(
    arguments: &Value,
    key: &str,
    default: usize,
    min: usize,
    max: usize,
) -> Result<usize, String> {
    let Some(value) = arguments.get(key) else {
        return Ok(default);
    };
    if value.is_null() {
        return Ok(default);
    }
    let value = value
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| format!("'{key}' must be an unsigned integer"))?;
    if value < min || value > max {
        return Err(format!("'{key}' must be between {min} and {max}"));
    }
    Ok(value)
}
