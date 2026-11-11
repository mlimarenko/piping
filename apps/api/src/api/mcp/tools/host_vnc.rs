use super::super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VncScopeRequirement {
    InteractiveSession,
    VisualControl,
    Action {
        action: &'static str,
        denial_message: &'static str,
    },
}

impl VncScopeRequirement {
    fn allows(self, request_context: &RequestContext, resource_id: &str) -> bool {
        match self {
            Self::InteractiveSession => {
                allows_host_interactive_session_for_resource(request_context, Some(resource_id))
            }
            Self::VisualControl => {
                allows_host_visual_control_for_resource(request_context, Some(resource_id))
            }
            Self::Action { action, .. } => request_context.allows_api_token_scope_for_resource(
                action,
                "connection_target",
                Some(resource_id),
            ),
        }
    }

    fn denial(self) -> VncScopeDenial {
        match self {
            Self::InteractiveSession => VncScopeDenial {
                audit_action: "secret",
                message: "MCP VNC session creation requires connect:connection_target and secret:connection_target for the target",
            },
            Self::VisualControl => VncScopeDenial {
                audit_action: "execute",
                message: "MCP VNC control requires execute:connection_target, connect:connection_target, and secret:connection_target for the target",
            },
            Self::Action {
                action,
                denial_message,
            } => VncScopeDenial {
                audit_action: action,
                message: denial_message,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VncScopeDenial {
    audit_action: &'static str,
    message: &'static str,
}

const VNC_CONNECT_REQUIREMENT: VncScopeRequirement = VncScopeRequirement::Action {
    action: "connect",
    denial_message: "API token scope does not permit connecting to this connection target",
};
const VNC_SECRET_REQUIREMENT: VncScopeRequirement = VncScopeRequirement::Action {
    action: "secret",
    denial_message: "API token scope does not permit VNC visual access to this connection target",
};
const VNC_OBSERVE_REQUIREMENT: VncScopeRequirement = VncScopeRequirement::Action {
    action: "observe",
    denial_message: "MCP VNC capture requires observe:connection_target for the target",
};
const VNC_BASE_SCOPE_REQUIREMENTS: &[VncScopeRequirement] =
    &[VNC_CONNECT_REQUIREMENT, VNC_SECRET_REQUIREMENT];
const VNC_OPEN_SCOPE_REQUIREMENTS: &[VncScopeRequirement] =
    &[VncScopeRequirement::InteractiveSession];
const VNC_CAPTURE_SCOPE_REQUIREMENTS: &[VncScopeRequirement] = &[
    VNC_OBSERVE_REQUIREMENT,
    VNC_CONNECT_REQUIREMENT,
    VNC_SECRET_REQUIREMENT,
];
const VNC_CONTROL_SCOPE_REQUIREMENTS: &[VncScopeRequirement] =
    &[VncScopeRequirement::VisualControl];

#[derive(Clone, Copy)]
enum VncToolNameMatcher {
    Exact(&'static str),
    Control,
}

impl VncToolNameMatcher {
    fn matches(self, tool_name: &str) -> bool {
        match self {
            Self::Exact(expected) => tool_name == expected,
            Self::Control => is_host_vnc_control_tool(tool_name),
        }
    }
}

const VNC_SCOPE_REQUIREMENTS_BY_TOOL: &[(VncToolNameMatcher, &[VncScopeRequirement])] = &[
    (
        VncToolNameMatcher::Exact(HOST_VNC_OPEN_TOOL_NAME),
        VNC_OPEN_SCOPE_REQUIREMENTS,
    ),
    (
        VncToolNameMatcher::Exact(HOST_VNC_CAPTURE_TOOL_NAME),
        VNC_CAPTURE_SCOPE_REQUIREMENTS,
    ),
    (VncToolNameMatcher::Control, VNC_CONTROL_SCOPE_REQUIREMENTS),
    (
        VncToolNameMatcher::Exact(HOST_VNC_CLOSE_TOOL_NAME),
        VNC_BASE_SCOPE_REQUIREMENTS,
    ),
];

fn host_vnc_scope_requirements(tool_name: &str) -> &'static [VncScopeRequirement] {
    VNC_SCOPE_REQUIREMENTS_BY_TOOL
        .iter()
        .find(|(matcher, _)| matcher.matches(tool_name))
        .map_or(VNC_BASE_SCOPE_REQUIREMENTS, |(_, requirements)| {
            *requirements
        })
}

/// Return the first denied requirement. The arrays above preserve the former
/// guard order because that order determines both the audit action and the
/// error message when several target-specific scopes are absent.
fn first_host_vnc_scope_denial(
    request_context: &RequestContext,
    tool_name: &str,
    resource_id: &str,
) -> Option<VncScopeDenial> {
    host_vnc_scope_requirements(tool_name)
        .iter()
        .copied()
        .find(|requirement| !requirement.allows(request_context, resource_id))
        .map(VncScopeRequirement::denial)
}

/// Enforce the token-scope gates shared by every agent-native VNC tool.
///
/// Returns the connection target id on success, or the JSON-RPC response that
/// already audited the denial.
async fn authorize_host_vnc_tool_call(
    state: &AppState,
    audit_auth_context: Option<&AuthContext>,
    request_context: &RequestContext,
    id: &Option<Value>,
    tool_name: &str,
    arguments: &Value,
) -> Result<String, Box<Response>> {
    if !has_host_visual_scope(request_context) {
        require_mcp_tool_audit!(
            id,
            record_mcp_tool_audit(
                state,
                audit_auth_context,
                request_context,
                McpToolAudit {
                    tool_name,
                    action: "connect",
                    resource_type: "connection_target",
                    resource_id: None,
                    decision: "scope_denied",
                    rest_status: None,
                    error_code: Some("TOKEN_SCOPE_DENIED")
                }
            )
        );
        return Err(Box::new(jsonrpc_result(
            id.clone(),
            tool_error(json!({
                "code": "TOKEN_SCOPE_DENIED",
                "message": "API token scope does not expose agent-native VNC visual tools"
            })),
        )));
    }

    let target_id = match host_vnc_target_id(tool_name, arguments) {
        Ok(value) => value,
        Err(error) => {
            require_mcp_tool_audit!(
                id,
                record_mcp_tool_audit(
                    state,
                    audit_auth_context,
                    request_context,
                    McpToolAudit {
                        tool_name,
                        action: "connect",
                        resource_type: "connection_target",
                        resource_id: None,
                        decision: "invalid_arguments",
                        rest_status: None,
                        error_code: Some("INVALID_ARGUMENTS")
                    }
                )
            );
            return Err(Box::new(jsonrpc_result(
                id.clone(),
                tool_error(json!({
                    "code": "INVALID_ARGUMENTS",
                    "message": error
                })),
            )));
        }
    };
    let resource_id = format!("connection_target:{target_id}");
    if let Some(denial) =
        first_host_vnc_scope_denial(request_context, tool_name, resource_id.as_str())
    {
        require_mcp_tool_audit!(
            id,
            record_mcp_tool_audit(
                state,
                audit_auth_context,
                request_context,
                McpToolAudit {
                    tool_name,
                    action: denial.audit_action,
                    resource_type: "connection_target",
                    resource_id: Some(resource_id.as_str()),
                    decision: "resource_denied",
                    rest_status: None,
                    error_code: Some("TOKEN_SCOPE_DENIED")
                }
            )
        );
        return Err(Box::new(jsonrpc_result(
            id.clone(),
            tool_error(json!({
                "code": "TOKEN_SCOPE_DENIED",
                "message": denial.message
            })),
        )));
    }
    Ok(target_id)
}

/// The audit identity of one agent-native VNC tool call.
struct HostVncToolAudit<'a> {
    tool_name: &'a str,
    resource_id: &'a str,
}

async fn finish_host_vnc_capture(
    state: &AppState,
    audit_auth_context: Option<&AuthContext>,
    request_context: &RequestContext,
    id: Option<Value>,
    audit: HostVncToolAudit<'_>,
    result: Result<VncCaptureToolResult, HostSshToolError>,
) -> Response {
    match result {
        Ok(value) => {
            require_mcp_tool_audit!(
                id,
                record_mcp_tool_audit(
                    state,
                    audit_auth_context,
                    request_context,
                    McpToolAudit {
                        tool_name: audit.tool_name,
                        action: "observe",
                        resource_type: "connection_target",
                        resource_id: Some(audit.resource_id),
                        decision: "allowed",
                        rest_status: None,
                        error_code: None
                    }
                )
            );
            jsonrpc_result(id, vnc_capture_tool_success(value))
        }
        Err(error) => {
            require_mcp_tool_audit!(
                id,
                record_mcp_tool_audit(
                    state,
                    audit_auth_context,
                    request_context,
                    McpToolAudit {
                        tool_name: audit.tool_name,
                        action: "observe",
                        resource_type: "connection_target",
                        resource_id: Some(audit.resource_id),
                        decision: "dispatch_failed",
                        rest_status: None,
                        error_code: Some(error.code)
                    }
                )
            );
            jsonrpc_result(id, tool_error(error.to_value()))
        }
    }
}

async fn finish_host_vnc_call(
    state: &AppState,
    audit_auth_context: Option<&AuthContext>,
    request_context: &RequestContext,
    id: Option<Value>,
    audit: HostVncToolAudit<'_>,
    result: Result<Value, HostSshToolError>,
) -> Response {
    match result {
        Ok(value) => {
            require_mcp_tool_audit!(
                id,
                record_mcp_tool_audit(
                    state,
                    audit_auth_context,
                    request_context,
                    McpToolAudit {
                        tool_name: audit.tool_name,
                        action: "connect",
                        resource_type: "connection_target",
                        resource_id: Some(audit.resource_id),
                        decision: "allowed",
                        rest_status: None,
                        error_code: None
                    }
                )
            );
            jsonrpc_result(id, tool_success(value))
        }
        Err(error) => {
            require_mcp_tool_audit!(
                id,
                record_mcp_tool_audit(
                    state,
                    audit_auth_context,
                    request_context,
                    McpToolAudit {
                        tool_name: audit.tool_name,
                        action: "connect",
                        resource_type: "connection_target",
                        resource_id: Some(audit.resource_id),
                        decision: "dispatch_failed",
                        rest_status: None,
                        error_code: Some(error.code)
                    }
                )
            );
            jsonrpc_result(id, tool_error(error.to_value()))
        }
    }
}

pub(in crate::api::mcp) async fn handle_host_vnc_tool_call(
    context: McpToolCallContext,
    tool_name: String,
    arguments: Value,
) -> Response {
    let McpToolCallContext {
        state,
        request_id,
        auth_context,
        audit_auth_context,
        headers,
        request_context,
        id,
    } = context;
    let target_id = match authorize_host_vnc_tool_call(
        &state,
        audit_auth_context.as_ref(),
        &request_context,
        &id,
        tool_name.as_str(),
        &arguments,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let resource_id = format!("connection_target:{target_id}");
    let audit = HostVncToolAudit {
        tool_name: tool_name.as_str(),
        resource_id: resource_id.as_str(),
    };

    if tool_name == HOST_VNC_CAPTURE_TOOL_NAME {
        let result =
            host_vnc_capture(state.clone(), &request_context, &target_id, &arguments).await;
        return finish_host_vnc_capture(
            &state,
            audit_auth_context.as_ref(),
            &request_context,
            id,
            audit,
            result,
        )
        .await;
    }

    let result = match tool_name.as_str() {
        HOST_VNC_OPEN_TOOL_NAME => {
            host_vnc_open(
                state.clone(),
                request_id,
                auth_context,
                headers,
                &target_id,
                &arguments,
            )
            .await
        }
        HOST_VNC_POINTER_TOOL_NAME => {
            host_vnc_pointer(state.clone(), &request_context, &target_id, &arguments).await
        }
        HOST_VNC_KEY_TOOL_NAME => {
            host_vnc_key(state.clone(), &request_context, &target_id, &arguments).await
        }
        HOST_VNC_TEXT_TOOL_NAME => {
            host_vnc_text(state.clone(), &request_context, &target_id, &arguments).await
        }
        HOST_VNC_CLOSE_TOOL_NAME => {
            host_vnc_close(state.clone(), &request_context, &target_id, &arguments).await
        }
        _ => Err(HostSshToolError::new(
            "UNKNOWN_TOOL",
            "Unknown agent-native VNC tool",
        )),
    };

    finish_host_vnc_call(
        &state,
        audit_auth_context.as_ref(),
        &request_context,
        id,
        audit,
        result,
    )
    .await
}

#[cfg(test)]
mod tests {
    use crate::api::mcp::test_support::*;

    use super::{
        HOST_VNC_CLOSE_TOOL_NAME, VNC_BASE_SCOPE_REQUIREMENTS, VNC_CAPTURE_SCOPE_REQUIREMENTS,
        VNC_CONNECT_REQUIREMENT, VNC_CONTROL_SCOPE_REQUIREMENTS, VNC_OBSERVE_REQUIREMENT,
        VNC_OPEN_SCOPE_REQUIREMENTS, VNC_SECRET_REQUIREMENT, VncScopeDenial, VncScopeRequirement,
        first_host_vnc_scope_denial, host_vnc_scope_requirements,
    };

    #[test]
    fn vnc_scope_requirement_table_preserves_operation_specific_order() {
        assert_eq!(
            host_vnc_scope_requirements(HOST_VNC_OPEN_TOOL_NAME),
            VNC_OPEN_SCOPE_REQUIREMENTS
        );
        assert_eq!(
            host_vnc_scope_requirements(HOST_VNC_CAPTURE_TOOL_NAME),
            VNC_CAPTURE_SCOPE_REQUIREMENTS
        );
        for tool_name in [
            HOST_VNC_POINTER_TOOL_NAME,
            HOST_VNC_KEY_TOOL_NAME,
            HOST_VNC_TEXT_TOOL_NAME,
        ] {
            assert_eq!(
                host_vnc_scope_requirements(tool_name),
                VNC_CONTROL_SCOPE_REQUIREMENTS,
                "{tool_name}"
            );
        }
        assert_eq!(
            host_vnc_scope_requirements(HOST_VNC_CLOSE_TOOL_NAME),
            VNC_BASE_SCOPE_REQUIREMENTS
        );
        assert_eq!(
            VNC_CAPTURE_SCOPE_REQUIREMENTS,
            &[
                VNC_OBSERVE_REQUIREMENT,
                VNC_CONNECT_REQUIREMENT,
                VNC_SECRET_REQUIREMENT
            ]
        );
        assert_eq!(
            VNC_OPEN_SCOPE_REQUIREMENTS,
            &[VncScopeRequirement::InteractiveSession]
        );

        let connect_and_secret = context_with_scopes(
            ["connect", "secret"]
                .into_iter()
                .map(|action| ApiTokenScopeContext {
                    action: action.to_string(),
                    resource_type: "connection_target".to_string(),
                    resource_pattern: "connection_target:host-1".to_string(),
                })
                .collect(),
        );
        assert_eq!(
            first_host_vnc_scope_denial(
                &connect_and_secret,
                HOST_VNC_CAPTURE_TOOL_NAME,
                "connection_target:host-1"
            ),
            Some(VncScopeDenial {
                audit_action: "observe",
                message: "MCP VNC capture requires observe:connection_target for the target",
            }),
            "capture must report its first operation-specific denial before common scopes"
        );
    }

    #[test]
    fn vnc_capture_requires_observe_scope_in_addition_to_connect_and_secret() {
        let request_context = RequestContext::new(
            Some(uuid::Uuid::nil()),
            "api_token".to_string(),
            None,
            "request".to_string(),
            crate::contexts::shared::request_context::RequestAuth {
                oidc_groups: Vec::new(),
                api_token_scopes: vec![
                    ApiTokenScopeContext {
                        action: "connect".to_string(),
                        resource_type: "connection_target".to_string(),
                        resource_pattern: "connection_target:host-1".to_string(),
                    },
                    ApiTokenScopeContext {
                        action: "secret".to_string(),
                        resource_type: "connection_target".to_string(),
                        resource_pattern: "connection_target:host-1".to_string(),
                    },
                    ApiTokenScopeContext {
                        action: "observe".to_string(),
                        resource_type: "connection_target".to_string(),
                        resource_pattern: "connection_target:host-1".to_string(),
                    },
                ],
                api_token_scope_resource_override: None,
            },
        );
        let tools = visible_tools(&request_context);

        assert!(tools.iter().any(|tool| {
            tool.get("name").and_then(|value| value.as_str()) == Some(HOST_VNC_OPEN_TOOL_NAME)
        }));
        assert!(tools.iter().any(|tool| {
            tool.get("name").and_then(|value| value.as_str()) == Some(HOST_VNC_CAPTURE_TOOL_NAME)
        }));
        for tool_name in [
            HOST_VNC_POINTER_TOOL_NAME,
            HOST_VNC_KEY_TOOL_NAME,
            HOST_VNC_TEXT_TOOL_NAME,
        ] {
            assert!(
                !tools.iter().any(|tool| {
                    tool.get("name").and_then(|value| value.as_str()) == Some(tool_name)
                }),
                "{tool_name} should be hidden without execute:connection_target"
            );
        }
        assert!(request_context.allows_api_token_scope_for_resource(
            "observe",
            "connection_target",
            Some("connection_target:host-1")
        ));
    }

    #[test]
    fn vnc_control_requires_execute_scope_in_addition_to_connect_and_secret() {
        let request_context = RequestContext::new(
            Some(uuid::Uuid::nil()),
            "api_token".to_string(),
            None,
            "request".to_string(),
            crate::contexts::shared::request_context::RequestAuth {
                oidc_groups: Vec::new(),
                api_token_scopes: vec![
                    ApiTokenScopeContext {
                        action: "connect".to_string(),
                        resource_type: "connection_target".to_string(),
                        resource_pattern: "connection_target:host-1".to_string(),
                    },
                    ApiTokenScopeContext {
                        action: "secret".to_string(),
                        resource_type: "connection_target".to_string(),
                        resource_pattern: "connection_target:host-1".to_string(),
                    },
                    ApiTokenScopeContext {
                        action: "execute".to_string(),
                        resource_type: "connection_target".to_string(),
                        resource_pattern: "connection_target:host-1".to_string(),
                    },
                ],
                api_token_scope_resource_override: None,
            },
        );
        let tools = visible_tools(&request_context);

        assert!(has_host_visual_control_scope(&request_context));
        for tool_name in [
            HOST_VNC_POINTER_TOOL_NAME,
            HOST_VNC_KEY_TOOL_NAME,
            HOST_VNC_TEXT_TOOL_NAME,
        ] {
            assert!(is_host_vnc_control_tool(tool_name));
            assert!(
                tools.iter().any(|tool| {
                    tool.get("name").and_then(|value| value.as_str()) == Some(tool_name)
                }),
                "{tool_name} should be visible"
            );
        }
        assert!(!tools.iter().any(|tool| {
            tool.get("name").and_then(|value| value.as_str()) == Some(HOST_VNC_CAPTURE_TOOL_NAME)
        }));
    }

    #[test]
    fn vnc_control_scope_requires_intersecting_execute_pattern() {
        let request_context = RequestContext::new(
            Some(uuid::Uuid::nil()),
            "api_token".to_string(),
            None,
            "request".to_string(),
            crate::contexts::shared::request_context::RequestAuth {
                oidc_groups: Vec::new(),
                api_token_scopes: vec![
                    ApiTokenScopeContext {
                        action: "connect".to_string(),
                        resource_type: "connection_target".to_string(),
                        resource_pattern: "connection_target:host-1".to_string(),
                    },
                    ApiTokenScopeContext {
                        action: "secret".to_string(),
                        resource_type: "connection_target".to_string(),
                        resource_pattern: "connection_target:host-1".to_string(),
                    },
                    ApiTokenScopeContext {
                        action: "execute".to_string(),
                        resource_type: "connection_target".to_string(),
                        resource_pattern: "connection_target:host-2".to_string(),
                    },
                ],
                api_token_scope_resource_override: None,
            },
        );
        let tools = visible_tools(&request_context);

        assert!(!has_host_visual_control_scope(&request_context));
        for tool_name in [
            HOST_VNC_POINTER_TOOL_NAME,
            HOST_VNC_KEY_TOOL_NAME,
            HOST_VNC_TEXT_TOOL_NAME,
        ] {
            assert!(
                !tools.iter().any(|tool| {
                    tool.get("name").and_then(|value| value.as_str()) == Some(tool_name)
                }),
                "{tool_name} should be hidden without an intersecting execute:connection_target pattern"
            );
        }
    }

    #[test]
    fn vnc_control_call_time_scope_is_target_specific() {
        let request_context = RequestContext::new(
            Some(uuid::Uuid::nil()),
            "api_token".to_string(),
            None,
            "request".to_string(),
            crate::contexts::shared::request_context::RequestAuth {
                oidc_groups: Vec::new(),
                api_token_scopes: vec![
                    ApiTokenScopeContext {
                        action: "connect".to_string(),
                        resource_type: "connection_target".to_string(),
                        resource_pattern: "*".to_string(),
                    },
                    ApiTokenScopeContext {
                        action: "secret".to_string(),
                        resource_type: "connection_target".to_string(),
                        resource_pattern: "*".to_string(),
                    },
                    ApiTokenScopeContext {
                        action: "execute".to_string(),
                        resource_type: "connection_target".to_string(),
                        resource_pattern: "connection_target:host-1".to_string(),
                    },
                ],
                api_token_scope_resource_override: None,
            },
        );

        assert!(has_host_visual_control_scope(&request_context));
        assert!(allows_host_visual_control_for_resource(
            &request_context,
            Some("connection_target:host-1")
        ));
        assert!(!allows_host_visual_control_for_resource(
            &request_context,
            Some("connection_target:host-2")
        ));
    }

    #[test]
    fn host_vnc_resource_id_comes_from_flat_target_argument() {
        assert_eq!(
            host_vnc_target_id(HOST_VNC_OPEN_TOOL_NAME, &json!({ "target_id": "target-1" })),
            Ok("target-1".to_string())
        );
        assert_eq!(
            host_vnc_target_id(
                HOST_VNC_CAPTURE_TOOL_NAME,
                &json!({
                    "target_id": "target-2",
                    "session_id": "session-1"
                })
            ),
            Ok("target-2".to_string())
        );
    }

    #[test]
    fn vnc_numeric_arguments_are_bounded() {
        assert_eq!(
            parse_u8_argument(&json!({ "button_mask": 255 }), "button_mask").unwrap(),
            255
        );
        assert!(parse_u8_argument(&json!({ "button_mask": 256 }), "button_mask").is_err());
        assert_eq!(
            parse_u16_argument(&json!({ "x": 65535 }), "x").unwrap(),
            65535
        );
        assert!(parse_u16_argument(&json!({ "x": 65536 }), "x").is_err());
        assert_eq!(
            parse_u32_argument(&json!({ "keysym": 0xff0d_u64 }), "keysym").unwrap(),
            0xff0d
        );
    }

    #[test]
    fn vnc_capture_format_defaults_to_png_and_accepts_bmp() {
        assert_eq!(
            parse_vnc_capture_format(&json!({})).unwrap(),
            crate::proxy::AgentVncCaptureFormat::Png
        );
        assert_eq!(
            parse_vnc_capture_format(&json!({ "format": "bmp" })).unwrap(),
            crate::proxy::AgentVncCaptureFormat::Bmp
        );
        assert!(parse_vnc_capture_format(&json!({ "format": "jpeg" })).is_err());
    }

    #[test]
    fn vnc_capture_schema_advertises_png_default_and_bmp_option() {
        let schema = host_vnc_capture_schema();
        assert_eq!(
            schema
                .pointer("/properties/format/default")
                .and_then(|value| value.as_str()),
            Some("png")
        );
        assert_eq!(
            schema
                .pointer("/properties/format/enum/1")
                .and_then(|value| value.as_str()),
            Some("bmp")
        );
        assert_eq!(
            schema
                .pointer("/properties/include_image/default")
                .and_then(|value| value.as_bool()),
            Some(false)
        );
        assert!(schema.pointer("/properties/max_inline_bytes").is_some());
    }

    #[test]
    fn vnc_capture_default_result_uses_resource_link_without_base64_text() {
        let metadata = json!({
            "session_id": "01900000-0000-7000-8000-000000000001",
            "connection_id": "01900000-0000-7000-8000-000000000002",
            "target_id": "01900000-0000-7000-8000-000000000003",
            "protocol": "vnc",
            "capture_id": "11111111-2222-4333-8444-555555555555",
            "resource_uri": "piping://connection_target/01900000-0000-7000-8000-000000000003/capture/11111111-2222-4333-8444-555555555555",
            "expires_at": "2026-06-15T20:00:00Z",
            "width": 1024,
            "height": 768,
            "source_width": 1024,
            "source_height": 768,
            "scaled": false,
            "mime_type": "image/png",
            "format": "png",
            "bytes": 31395,
            "inline_image_included": false
        });
        let result = vnc_capture_tool_success(VncCaptureToolResult {
            metadata,
            resource_uri: "piping://connection_target/01900000-0000-7000-8000-000000000003/capture/11111111-2222-4333-8444-555555555555".to_string(),
            resource_name: "vnc-capture-11111111-2222-4333-8444-555555555555.png".to_string(),
            resource_title: "VNC framebuffer 1024x768".to_string(),
            mime_type: "image/png".to_string(),
            inline_image_base64: None,
        });

        assert!(result.pointer("/structuredContent/image_base64").is_none());
        let text = result
            .pointer("/content/0/text")
            .and_then(|value| value.as_str())
            .expect("metadata text");
        assert!(!text.contains("image_base64"));
        assert!(result["content"].as_array().is_some_and(|content| {
            content.iter().any(|item| {
                item.get("type").and_then(|value| value.as_str()) == Some("resource_link")
                    && item
                        .get("uri")
                        .and_then(|value| value.as_str())
                        .is_some_and(|uri| {
                            uri.starts_with("piping://connection_target/")
                                && uri.contains("/capture/")
                        })
            })
        }));
    }

    #[test]
    fn vnc_capture_resource_uri_parser_is_strict() {
        assert_eq!(
            parse_vnc_capture_resource_uri("piping://connection_target/target-1/capture/capture-1"),
            Some(("target-1", "capture-1"))
        );
        assert!(parse_vnc_capture_resource_uri("piping://connection_target/target-1").is_none());
        assert!(
            parse_vnc_capture_resource_uri(
                "piping://connection_target/target-1/capture/capture-1/extra"
            )
            .is_none()
        );
    }

    #[test]
    fn vnc_open_body_rejects_proxy_node_override() {
        let body = json!({
            "target_id": "target-1",
            "proxy_node_id": "proxy-node-1"
        });
        let error = validate_vnc_open_body(body.as_object().expect("object")).unwrap_err();
        assert_eq!(error.code, "EXTERNAL_PROXY_UNSUPPORTED");
    }
}
