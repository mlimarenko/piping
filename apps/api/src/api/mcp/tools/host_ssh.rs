use super::super::*;

impl McpToolAuditReturn for Result<String, Box<Response>> {
    fn from_audit_failure(response: Response) -> Self {
        Err(Box::new(response))
    }
}

/// Enforce the token-scope gates that every agent-native SSH tool shares
/// before it creates a session.
///
/// Returns the connection target id on success, or the JSON-RPC response that
/// already audited the denial.
async fn authorize_host_ssh_tool_call(
    state: &AppState,
    audit_auth_context: Option<&AuthContext>,
    request_context: &RequestContext,
    id: &Option<Value>,
    tool_name: &str,
    arguments: &Value,
) -> Result<String, Box<Response>> {
    if !has_host_ssh_scope(request_context) {
        require_mcp_tool_audit!(
            id,
            record_mcp_tool_audit(
                state,
                audit_auth_context,
                request_context,
                McpToolAudit {
                    tool_name,
                    action: "execute",
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
                "message": "API token scope does not expose agent-native SSH tools"
            })),
        )));
    }

    let target_id = match host_ssh_target_id(tool_name, arguments) {
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
                        action: "execute",
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
    if !request_context.allows_api_token_scope_for_resource(
        "execute",
        "connection_target",
        Some(resource_id.as_str()),
    ) {
        require_mcp_tool_audit!(
            id,
            record_mcp_tool_audit(
                state,
                audit_auth_context,
                request_context,
                McpToolAudit {
                    tool_name,
                    action: "execute",
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
                "message": "API token scope does not permit SSH execution on this connection target"
            })),
        )));
    }
    if !request_context.allows_api_token_scope_for_resource(
        "connect",
        "connection_target",
        Some(resource_id.as_str()),
    ) {
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
                "message": "API token scope does not permit using SSH tools on this connection target"
            })),
        )));
    }
    Ok(target_id)
}

pub(in crate::api::mcp) async fn handle_host_ssh_tool_call(
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
    let target_id = match authorize_host_ssh_tool_call(
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
    if ssh_tool_creates_connection_session(tool_name.as_str())
        && !allows_host_interactive_session_for_resource(
            &request_context,
            Some(resource_id.as_str()),
        )
    {
        require_mcp_tool_audit!(
            id,
            record_mcp_tool_audit(
                &state,
                audit_auth_context.as_ref(),
                &request_context,
                McpToolAudit {
                    tool_name: tool_name.as_str(),
                    action: "secret",
                    resource_type: "connection_target",
                    resource_id: Some(resource_id.as_str()),
                    decision: "resource_denied",
                    rest_status: None,
                    error_code: Some("TOKEN_SCOPE_DENIED")
                }
            )
        );
        return jsonrpc_result(
            id,
            tool_error(json!({
                "code": "TOKEN_SCOPE_DENIED",
                "message": "MCP SSH session creation requires connect:connection_target and secret:connection_target for the target"
            })),
        );
    }

    let result = dispatch_host_ssh_tool(
        RestDispatchContext {
            state: state.clone(),
            request_id,
            auth_context,
            headers,
        },
        &request_context,
        &target_id,
        tool_name.as_str(),
        &arguments,
    )
    .await;

    match result {
        Ok(value) => {
            require_mcp_tool_audit!(
                id,
                record_mcp_tool_audit(
                    &state,
                    audit_auth_context.as_ref(),
                    &request_context,
                    McpToolAudit {
                        tool_name: tool_name.as_str(),
                        action: "execute",
                        resource_type: "connection_target",
                        resource_id: Some(resource_id.as_str()),
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
                    &state,
                    audit_auth_context.as_ref(),
                    &request_context,
                    McpToolAudit {
                        tool_name: tool_name.as_str(),
                        action: "execute",
                        resource_type: "connection_target",
                        resource_id: Some(resource_id.as_str()),
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

/// Execute a scope-checked SSH tool; the caller owns the audit and JSON-RPC envelope.
async fn dispatch_host_ssh_tool(
    rest: RestDispatchContext,
    request_context: &RequestContext,
    target_id: &str,
    tool_name: &str,
    arguments: &Value,
) -> Result<Value, HostSshToolError> {
    let RestDispatchContext {
        state,
        request_id,
        auth_context,
        headers,
    } = rest;
    match tool_name {
        HOST_SSH_OPEN_TOOL_NAME => {
            host_ssh_open(
                state.clone(),
                request_id,
                auth_context,
                headers,
                target_id,
                arguments,
            )
            .await
        }
        HOST_SSH_EXEC_TOOL_NAME => {
            host_ssh_exec(
                state.clone(),
                request_id,
                auth_context,
                headers,
                target_id,
                arguments,
            )
            .await
        }
        HOST_SSH_TAIL_LOG_TOOL_NAME => {
            host_ssh_tail_log(
                state.clone(),
                request_id,
                auth_context,
                headers,
                target_id,
                arguments,
            )
            .await
        }
        HOST_INSPECT_TOOL_NAME => {
            host_inspect(
                state.clone(),
                request_id,
                auth_context,
                headers,
                target_id,
                arguments,
            )
            .await
        }
        HOST_CHECK_PORT_TOOL_NAME => {
            host_check_port(
                state.clone(),
                request_id,
                auth_context,
                headers,
                target_id,
                arguments,
            )
            .await
        }
        HOST_READ_FILE_TOOL_NAME => {
            host_read_file(
                state.clone(),
                request_id,
                auth_context,
                headers,
                target_id,
                arguments,
            )
            .await
        }
        HOST_FIND_LOGS_TOOL_NAME => {
            host_find_logs(
                state.clone(),
                request_id,
                auth_context,
                headers,
                target_id,
                arguments,
            )
            .await
        }
        _ => {
            dispatch_host_ssh_session_tool(state, request_context, target_id, tool_name, arguments)
                .await
        }
    }
}

/// Dispatch SSH tools that use the already-authorized request context directly.
async fn dispatch_host_ssh_session_tool(
    state: AppState,
    request_context: &RequestContext,
    target_id: &str,
    tool_name: &str,
    arguments: &Value,
) -> Result<Value, HostSshToolError> {
    match tool_name {
        HOST_RESOLVE_CONNECTION_TOOL_NAME => {
            host_resolve_connection(state.clone(), request_context, target_id, arguments).await
        }
        HOST_SSH_WRITE_TOOL_NAME => {
            host_ssh_write(state.clone(), request_context, target_id, arguments).await
        }
        HOST_SSH_READ_TOOL_NAME => {
            host_ssh_read(state.clone(), request_context, target_id, arguments).await
        }
        HOST_SSH_READ_UNTIL_TOOL_NAME => {
            host_ssh_read_until(state.clone(), request_context, target_id, arguments).await
        }
        HOST_SSH_WAIT_FOR_PROMPT_TOOL_NAME => {
            host_ssh_wait_for_prompt(state.clone(), request_context, target_id, arguments).await
        }
        HOST_SSH_WAIT_FOR_QUIET_TOOL_NAME => {
            host_ssh_wait_for_quiet(state.clone(), request_context, target_id, arguments).await
        }
        HOST_SSH_RESIZE_TOOL_NAME => {
            host_ssh_resize(state.clone(), request_context, target_id, arguments).await
        }
        HOST_SSH_CLOSE_TOOL_NAME => {
            host_ssh_close(state.clone(), request_context, target_id, arguments).await
        }
        _ => Err(HostSshToolError::new(
            "UNKNOWN_TOOL",
            "Unknown agent-native SSH tool",
        )),
    }
}

#[cfg(test)]
mod tests {
    use crate::api::mcp::test_support::*;

    #[test]
    fn ssh_terminal_response_modes_preserve_cursors_and_allow_compaction() {
        assert!(matches!(
            parse_ssh_response_mode(&json!({})),
            Ok(SshResponseMode::Full)
        ));
        let mut metadata = serde_json::Map::new();
        metadata.insert("next_after_seq".to_string(), json!(7));
        metadata.insert("truncated".to_string(), json!(false));

        let compact = shape_ssh_terminal_response(
            metadata,
            vec![json!({ "seq": 7, "text": "\u{1b}[31mred\u{1b}[0m" })],
            "\u{1b}[31mred\u{1b}[0m".to_string(),
            &json!({ "response_mode": "combined", "strip_ansi": true }),
        )
        .expect("compact response");

        assert_eq!(compact.get("next_after_seq"), Some(&json!(7)));
        assert_eq!(compact.get("combined_text"), Some(&json!("red")));
        assert!(compact.get("chunks").is_none());
        assert_eq!(compact.get("response_mode"), Some(&json!("combined")));
        assert_eq!(strip_ansi_codes("\u{1b}[32mok\u{1b}[0m"), "ok");
        assert_eq!(
            strip_ansi_codes("\u{1b}]0;remote-title\u{7}\u{1b}[32mok\u{1b}[0m"),
            "ok"
        );
    }

    #[test]
    fn host_diagnostic_validators_reject_shell_and_secret_paths() {
        assert!(validate_probe_host("127.0.0.1").is_ok());
        assert!(validate_probe_host("bad;rm").is_err());
        assert!(validate_safe_remote_path("/var/log/syslog").is_ok());
        assert!(validate_safe_remote_path("../var/log/syslog").is_err());
        assert!(validate_safe_remote_path("/etc/shadow").is_err());
        assert!(validate_safe_remote_path("/home/alice/.ssh/id_rsa").is_err());
        assert!(validate_find_pattern("*.log").is_ok());
        assert!(validate_find_pattern("../*.log").is_err());
    }

    #[test]
    fn execute_and_connect_host_scopes_expose_agent_native_ssh_tools() {
        let request_context = RequestContext::new(
            Some(uuid::Uuid::nil()),
            "api_token".to_string(),
            None,
            "request".to_string(),
            crate::contexts::shared::request_context::RequestAuth {
                oidc_groups: Vec::new(),
                api_token_scopes: vec![
                    ApiTokenScopeContext {
                        action: "execute".to_string(),
                        resource_type: "connection_target".to_string(),
                        resource_pattern: "connection_target:host-1".to_string(),
                    },
                    ApiTokenScopeContext {
                        action: "connect".to_string(),
                        resource_type: "connection_target".to_string(),
                        resource_pattern: "connection_target:host-1".to_string(),
                    },
                ],
                api_token_scope_resource_override: None,
            },
        );
        let tools = visible_tools(&request_context);

        for tool_name in [
            HOST_SSH_OPEN_TOOL_NAME,
            HOST_SSH_EXEC_TOOL_NAME,
            HOST_SSH_WRITE_TOOL_NAME,
            HOST_SSH_READ_TOOL_NAME,
            HOST_SSH_READ_UNTIL_TOOL_NAME,
            HOST_SSH_WAIT_FOR_PROMPT_TOOL_NAME,
            HOST_SSH_TAIL_LOG_TOOL_NAME,
            HOST_SSH_RESIZE_TOOL_NAME,
            HOST_SSH_CLOSE_TOOL_NAME,
        ] {
            assert!(
                tools.iter().any(|tool| {
                    tool.get("name").and_then(|value| value.as_str()) == Some(tool_name)
                }),
                "{tool_name} should be visible"
            );
        }
    }

    #[test]
    fn execute_without_connect_host_scope_hides_agent_native_ssh_tools() {
        let request_context = context("execute", "connection_target", "connection_target:host-1");
        let tools = visible_tools(&request_context);

        for tool_name in [
            HOST_SSH_OPEN_TOOL_NAME,
            HOST_SSH_EXEC_TOOL_NAME,
            HOST_SSH_WRITE_TOOL_NAME,
            HOST_SSH_READ_TOOL_NAME,
            HOST_SSH_READ_UNTIL_TOOL_NAME,
            HOST_SSH_WAIT_FOR_PROMPT_TOOL_NAME,
            HOST_SSH_TAIL_LOG_TOOL_NAME,
            HOST_SSH_RESIZE_TOOL_NAME,
            HOST_SSH_CLOSE_TOOL_NAME,
        ] {
            assert!(
                !tools.iter().any(|tool| {
                    tool.get("name").and_then(|value| value.as_str()) == Some(tool_name)
                }),
                "{tool_name} should be hidden"
            );
        }
    }

    #[test]
    fn host_ssh_tools_require_intersecting_execute_and_connect_patterns() {
        let request_context = RequestContext::new(
            Some(uuid::Uuid::nil()),
            "api_token".to_string(),
            None,
            "request".to_string(),
            crate::contexts::shared::request_context::RequestAuth {
                oidc_groups: Vec::new(),
                api_token_scopes: vec![
                    ApiTokenScopeContext {
                        action: "execute".to_string(),
                        resource_type: "connection_target".to_string(),
                        resource_pattern: "connection_target:host-1".to_string(),
                    },
                    ApiTokenScopeContext {
                        action: "connect".to_string(),
                        resource_type: "connection_target".to_string(),
                        resource_pattern: "connection_target:host-2".to_string(),
                    },
                ],
                api_token_scope_resource_override: None,
            },
        );

        assert!(!has_host_ssh_scope(&request_context));
        let tools = visible_tools(&request_context);
        for tool_name in [
            HOST_SSH_OPEN_TOOL_NAME,
            HOST_SSH_EXEC_TOOL_NAME,
            HOST_SSH_WRITE_TOOL_NAME,
            HOST_SSH_READ_TOOL_NAME,
            HOST_SSH_READ_UNTIL_TOOL_NAME,
            HOST_SSH_WAIT_FOR_PROMPT_TOOL_NAME,
            HOST_SSH_TAIL_LOG_TOOL_NAME,
            HOST_SSH_RESIZE_TOOL_NAME,
            HOST_SSH_CLOSE_TOOL_NAME,
        ] {
            assert!(
                !tools.iter().any(|tool| {
                    tool.get("name").and_then(|value| value.as_str()) == Some(tool_name)
                }),
                "{tool_name} should be hidden"
            );
        }
    }

    #[test]
    fn connect_and_secret_host_scopes_expose_session_client_credentials_tool() {
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
                ],
                api_token_scope_resource_override: None,
            },
        );

        assert!(has_host_session_client_credentials_scope(&request_context));
        assert!(has_host_visual_scope(&request_context));
        assert!(!has_host_visual_control_scope(&request_context));
        let tools = visible_tools(&request_context);
        assert!(tools.iter().any(|tool| {
            tool.get("name").and_then(|value| value.as_str())
                == Some(HOST_SESSION_CLIENT_CREDENTIALS_TOOL_NAME)
        }));
        assert!(tools.iter().any(|tool| {
            tool.get("name").and_then(|value| value.as_str()) == Some(HOST_VNC_OPEN_TOOL_NAME)
        }));
        assert!(!tools.iter().any(|tool| {
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
            "secret",
            "connection_target",
            Some("connection_target:host-1")
        ));
        assert!(!request_context.allows_api_token_scope_for_resource(
            "secret",
            "connection_target",
            Some("connection_target:host-2")
        ));
    }

    #[test]
    fn connect_without_secret_hides_session_client_credentials_tool() {
        let request_context = context("connect", "connection_target", "connection_target:host-1");
        let tools = visible_tools(&request_context);

        assert!(!has_host_session_client_credentials_scope(&request_context));
        assert!(!tools.iter().any(|tool| {
            tool.get("name").and_then(|value| value.as_str())
                == Some(HOST_SESSION_CLIENT_CREDENTIALS_TOOL_NAME)
        }));
        assert!(!tools.iter().any(|tool| {
            tool.get("name").and_then(|value| value.as_str()) == Some(HOST_VNC_OPEN_TOOL_NAME)
        }));
    }

    #[test]
    fn session_client_credentials_tool_requires_intersecting_connect_and_secret_patterns() {
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
                        resource_pattern: "connection_target:host-2".to_string(),
                    },
                ],
                api_token_scope_resource_override: None,
            },
        );

        assert!(!has_host_session_client_credentials_scope(&request_context));
        let tools = visible_tools(&request_context);
        assert!(!tools.iter().any(|tool| {
            tool.get("name").and_then(|value| value.as_str())
                == Some(HOST_SESSION_CLIENT_CREDENTIALS_TOOL_NAME)
        }));
        assert!(!has_host_visual_scope(&request_context));
        assert!(!tools.iter().any(|tool| {
            tool.get("name").and_then(|value| value.as_str()) == Some(HOST_VNC_CAPTURE_TOOL_NAME)
        }));
    }

    #[test]
    fn host_ssh_resource_id_comes_from_flat_target_argument() {
        // Every curated SSH tool now takes a flat top-level `target_id`, with no
        // `body` wrapper on any of them.
        assert_eq!(
            host_ssh_target_id(HOST_SSH_OPEN_TOOL_NAME, &json!({ "target_id": "target-1" })),
            Ok("target-1".to_string())
        );
        assert_eq!(
            host_ssh_target_id(
                HOST_SSH_READ_TOOL_NAME,
                &json!({
                    "target_id": "target-2",
                    "session_id": "session-1"
                })
            ),
            Ok("target-2".to_string())
        );
        assert_eq!(
            host_ssh_target_id(
                HOST_SSH_EXEC_TOOL_NAME,
                &json!({
                    "target_id": "target-3",
                    "command": "hostname"
                })
            ),
            Ok("target-3".to_string())
        );
        assert_eq!(
            host_ssh_target_id(
                HOST_SSH_TAIL_LOG_TOOL_NAME,
                &json!({
                    "target_id": "target-4",
                    "path": "/var/log/syslog"
                })
            ),
            Ok("target-4".to_string())
        );
        // A nested `body.target_id` is no longer accepted — the flat contract
        // is the only shape.
        assert!(
            host_ssh_target_id(
                HOST_SSH_EXEC_TOOL_NAME,
                &json!({ "body": { "target_id": "target-x" }, "command": "hostname" })
            )
            .is_err()
        );
    }

    #[test]
    fn shell_quote_escapes_tail_paths() {
        assert_eq!(shell_quote("/var/log/syslog"), "'/var/log/syslog'");
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn ssh_open_body_rejects_proxy_node_override() {
        let body = json!({
            "target_id": "target-1",
            "proxy_node_id": "proxy-node-1"
        });
        let error = validate_ssh_open_body(body.as_object().expect("object")).unwrap_err();
        assert_eq!(error.code, "EXTERNAL_PROXY_UNSUPPORTED");
    }

    #[test]
    fn agent_native_open_schemas_do_not_advertise_external_proxy_node_id() {
        for (tool_name, schema) in [
            (HOST_SSH_OPEN_TOOL_NAME, host_ssh_open_schema()),
            (HOST_SSH_EXEC_TOOL_NAME, host_ssh_exec_schema()),
            (HOST_VNC_OPEN_TOOL_NAME, host_vnc_open_schema()),
        ] {
            assert!(
                schema
                    .pointer("/properties/body/properties/proxy_node_id")
                    .is_none(),
                "{tool_name} must not advertise proxy_node_id"
            );
        }
    }

    #[test]
    fn ssh_compact_response_schema_matches_supported_tools() {
        let read_schema = host_ssh_read_schema();
        assert!(
            read_schema.pointer("/properties/response_mode").is_some(),
            "ssh_read supports response_mode"
        );
        assert!(
            read_schema.pointer("/properties/strip_ansi").is_some(),
            "ssh_read supports strip_ansi"
        );

        let exec_schema = host_ssh_exec_schema();
        assert!(
            exec_schema.pointer("/properties/response_mode").is_none(),
            "ssh_exec must not advertise response_mode until it shapes exec responses"
        );
        assert!(
            exec_schema.pointer("/properties/strip_ansi").is_none(),
            "ssh_exec must not advertise strip_ansi until it shapes exec responses"
        );
    }

    /// A database failure must never hand a driver-level string to an MCP
    /// client. The raw text names tables, columns, and constraints, and MCP
    /// callers are frequently less trusted than the operators using the UI.

    #[test]
    fn host_tool_errors_redact_database_failure_text() {
        let raw = "relation \"license_seat_assignment\" violates constraint \"fk_user\"";

        for error in [
            super::host_ssh_app_error(crate::errors::AppError::Database(raw.to_string())),
            super::host_visual_app_error(crate::errors::AppError::Database(raw.to_string())),
        ] {
            assert!(
                !error.message.contains("license_seat_assignment")
                    && !error.message.contains("fk_user"),
                "database internals leaked to an MCP client: {}",
                error.message
            );
            assert_eq!(
                error.message, "Database error",
                "expected the canonical redacted REST message"
            );
        }
    }

    /// The redaction must not swallow errors whose text is the actual answer:
    /// an external-service failure is the operator-facing diagnostic and REST
    /// passes it through unchanged, so MCP must too.

    #[test]
    fn host_tool_errors_keep_external_failure_text() {
        let error = super::host_ssh_app_error(crate::errors::AppError::External(
            "ssh: connect to host 10.0.0.9 port 22: Connection refused".to_string(),
        ));
        assert!(
            error.message.contains("Connection refused"),
            "external diagnostics must survive redaction: {}",
            error.message
        );
    }
}
