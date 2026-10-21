use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DedicatedToolHandler {
    Capabilities,
    GenericRest,
    ScopedRestList,
    ScopedRestCall,
    Inventory,
    AccessControl,
    HostSessionClientCredentials,
    CctvExport,
    CctvPlayback,
    CctvSnapshot,
    HostSsh,
    HostVnc,
}

#[derive(Clone, Copy)]
enum ToolNameMatcher {
    Exact(&'static str),
    Inventory,
    AccessControl,
    HostSsh,
    HostVnc,
}

impl ToolNameMatcher {
    fn matches(self, name: &str) -> bool {
        match self {
            Self::Exact(expected) => name == expected,
            Self::Inventory => InventoryToolOperation::from_name(name).is_some(),
            Self::AccessControl => AccessControlToolOperation::from_name(name).is_some(),
            Self::HostSsh => is_host_ssh_tool(name),
            Self::HostVnc => is_host_vnc_tool(name),
        }
    }
}

const DEDICATED_TOOL_ROUTES: &[(ToolNameMatcher, DedicatedToolHandler)] = &[
    (
        ToolNameMatcher::Exact(MCP_CAPABILITIES_TOOL_NAME),
        DedicatedToolHandler::Capabilities,
    ),
    (
        ToolNameMatcher::Exact(GENERIC_REST_TOOL_NAME),
        DedicatedToolHandler::GenericRest,
    ),
    (
        ToolNameMatcher::Exact(SCOPED_REST_LIST_ENDPOINTS_TOOL_NAME),
        DedicatedToolHandler::ScopedRestList,
    ),
    (
        ToolNameMatcher::Exact(SCOPED_REST_CALL_ENDPOINT_TOOL_NAME),
        DedicatedToolHandler::ScopedRestCall,
    ),
    (ToolNameMatcher::Inventory, DedicatedToolHandler::Inventory),
    (
        ToolNameMatcher::AccessControl,
        DedicatedToolHandler::AccessControl,
    ),
    (
        ToolNameMatcher::Exact(HOST_SESSION_CLIENT_CREDENTIALS_TOOL_NAME),
        DedicatedToolHandler::HostSessionClientCredentials,
    ),
    (
        ToolNameMatcher::Exact(CCTV_EXPORT_TOOL_NAME),
        DedicatedToolHandler::CctvExport,
    ),
    (
        ToolNameMatcher::Exact(CCTV_PLAYBACK_TOOL_NAME),
        DedicatedToolHandler::CctvPlayback,
    ),
    (
        ToolNameMatcher::Exact(CCTV_SNAPSHOT_TOOL_NAME),
        DedicatedToolHandler::CctvSnapshot,
    ),
    (ToolNameMatcher::HostSsh, DedicatedToolHandler::HostSsh),
    (ToolNameMatcher::HostVnc, DedicatedToolHandler::HostVnc),
];

/// Select the first dedicated route, preserving the former `if` chain's
/// precedence. First-match-wins matters if a future category matcher expands
/// to include a tool that already has a more specialized exact-name handler.
fn dedicated_tool_handler(name: &str) -> Option<DedicatedToolHandler> {
    DEDICATED_TOOL_ROUTES
        .iter()
        .find(|(matcher, _)| matcher.matches(name))
        .map(|(_, handler)| *handler)
}

pub(super) async fn handle_tool_call(context: McpToolCallContext, params: Value) -> Response {
    let mut call = match serde_json::from_value::<ToolCallParams>(params) {
        Ok(value) => value,
        Err(error) => {
            return jsonrpc_error(
                context.id,
                -32602,
                "Invalid tools/call params",
                Some(json!({ "error": error.to_string() })),
            );
        }
    };
    if call.arguments.is_null() {
        call.arguments = json!({});
    }

    if let Some(schema) = visible_tool_input_schema(&context.request_context, call.name.as_str())
        && let Err(error) = validate_mcp_tool_arguments(&schema, &call.arguments)
    {
        return jsonrpc_result(
            context.id,
            tool_error(json!({
                "code": "INVALID_ARGUMENTS",
                "message": error
            })),
        );
    }

    if let Some(handler) = dedicated_tool_handler(call.name.as_str()) {
        return handle_dedicated_tool_call(handler, context, call).await;
    }

    handle_generic_tool_call(context, call).await
}

/// Route one dedicated MCP tool call to the handler that owns its category.
async fn handle_dedicated_tool_call(
    handler: DedicatedToolHandler,
    context: McpToolCallContext,
    call: ToolCallParams,
) -> Response {
    let tool_name = call.name;
    let arguments = call.arguments;
    match handler {
        DedicatedToolHandler::Capabilities => {
            let capabilities =
                match mcp_capabilities_value_for_args(&context.request_context, &arguments) {
                    Ok(value) => value,
                    Err(error) => {
                        return jsonrpc_result(
                            context.id,
                            tool_error(json!({
                                "code": "INVALID_ARGUMENTS",
                                "message": error
                            })),
                        );
                    }
                };
            require_mcp_tool_audit!(
                context.id,
                record_mcp_tool_audit(
                    &context.state,
                    context.audit_auth_context.as_ref(),
                    &context.request_context,
                    McpToolAudit {
                        tool_name: MCP_CAPABILITIES_TOOL_NAME,
                        action: "read",
                        resource_type: "mcp",
                        resource_id: None,
                        decision: "success",
                        rest_status: None,
                        error_code: None
                    }
                )
            );
            jsonrpc_result(context.id, tool_success(capabilities))
        }
        DedicatedToolHandler::GenericRest => handle_generic_rest_call(context, arguments).await,
        DedicatedToolHandler::ScopedRestList => {
            handle_scoped_rest_list_endpoints_tool_call(
                context.state,
                context.audit_auth_context,
                context.request_context,
                context.id,
                arguments,
            )
            .await
        }
        DedicatedToolHandler::ScopedRestCall => {
            handle_scoped_rest_endpoint_tool_call(context, arguments).await
        }
        DedicatedToolHandler::Inventory => {
            handle_inventory_tool_call(context, tool_name, arguments).await
        }
        DedicatedToolHandler::AccessControl => {
            handle_access_control_tool_call(context, tool_name, arguments).await
        }
        DedicatedToolHandler::HostSessionClientCredentials => {
            handle_host_session_client_credentials_tool_call(context, arguments).await
        }
        DedicatedToolHandler::CctvExport => {
            handle_cctv_archive_tool_call(
                context.state,
                context.audit_auth_context,
                context.request_context,
                context.id,
                arguments,
                ArchiveDelivery::Export,
            )
            .await
        }
        DedicatedToolHandler::CctvPlayback => {
            handle_cctv_archive_tool_call(
                context.state,
                context.audit_auth_context,
                context.request_context,
                context.id,
                arguments,
                ArchiveDelivery::Watch,
            )
            .await
        }
        DedicatedToolHandler::CctvSnapshot => {
            handle_cctv_snapshot_tool_call(
                context.state,
                context.audit_auth_context,
                context.request_context,
                context.id,
                arguments,
            )
            .await
        }
        DedicatedToolHandler::HostSsh => {
            handle_host_ssh_tool_call(context, tool_name, arguments).await
        }
        DedicatedToolHandler::HostVnc => {
            handle_host_vnc_tool_call(context, tool_name, arguments).await
        }
    }
}

/// One rejected generic MCP tool call: what to audit, and what to answer with.
struct GenericToolRejection<'a> {
    tool_name: &'a str,
    action: &'a str,
    resource_type: &'a str,
    resource_id: Option<&'a str>,
    /// Audit outcome label, e.g. `scope_denied`.
    outcome: &'a str,
    /// Wire error code, also recorded on the audit row so the two cannot drift.
    code: &'a str,
    message: String,
}

/// Audit a rejected generic MCP tool call and build its JSON-RPC error.
async fn reject_generic_tool(
    state: &AppState,
    audit_auth_context: Option<&AuthContext>,
    request_context: &RequestContext,
    id: Option<Value>,
    rejection: GenericToolRejection<'_>,
) -> Response {
    require_mcp_tool_audit!(
        id,
        record_mcp_tool_audit(
            state,
            audit_auth_context,
            request_context,
            McpToolAudit {
                tool_name: rejection.tool_name,
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

/// Authorize and dispatch one catalog MCP tool through the REST router.
async fn handle_generic_tool_call(context: McpToolCallContext, call: ToolCallParams) -> Response {
    let McpToolCallContext {
        state,
        request_id,
        auth_context,
        audit_auth_context,
        headers,
        request_context,
        id,
    } = context;

    let Some(tool) = mcp_tools().into_iter().find(|tool| tool.name == call.name) else {
        return jsonrpc_error(
            id,
            -32602,
            "Unknown tool",
            Some(json!({ "name": call.name })),
        );
    };

    if !tool_visible(&request_context, &tool) {
        return reject_generic_tool(
            &state,
            audit_auth_context.as_ref(),
            &request_context,
            id,
            GenericToolRejection {
                tool_name: tool.name,
                action: tool.action,
                resource_type: tool.resource_type,
                resource_id: None,
                outcome: "scope_denied",
                code: "TOKEN_SCOPE_DENIED",
                message: "API token scope does not expose this MCP tool".to_string(),
            },
        )
        .await;
    }

    let resource_id = match resource_id_for_scope(&tool, &call.arguments) {
        Ok(value) => value,
        Err(error) => {
            return reject_generic_tool(
                &state,
                audit_auth_context.as_ref(),
                &request_context,
                id,
                GenericToolRejection {
                    tool_name: tool.name,
                    action: tool.action,
                    resource_type: tool.resource_type,
                    resource_id: None,
                    outcome: "invalid_arguments",
                    code: "INVALID_ARGUMENTS",
                    message: error,
                },
            )
            .await;
        }
    };

    if is_connection_session_create_tool(&tool)
        && !allows_host_interactive_session_for_resource(&request_context, resource_id.as_deref())
    {
        require_mcp_tool_audit!(
            id,
            record_mcp_tool_audit(
                &state,
                audit_auth_context.as_ref(),
                &request_context,
                McpToolAudit {
                    tool_name: tool.name,
                    action: "secret",
                    resource_type: "connection_target",
                    resource_id: resource_id.as_deref(),
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
                "message": "MCP connection session creation requires connect:connection_target and secret:connection_target for the target because it mints an interactive proxy token"
            })),
        );
    }

    if !request_context.allows_api_token_scope_for_resource(
        tool.action,
        tool.resource_type,
        resource_id.as_deref(),
    ) {
        return reject_generic_tool(
            &state,
            audit_auth_context.as_ref(),
            &request_context,
            id,
            GenericToolRejection {
                tool_name: tool.name,
                action: tool.action,
                resource_type: tool.resource_type,
                resource_id: resource_id.as_deref(),
                outcome: "resource_denied",
                code: "TOKEN_SCOPE_DENIED",
                message: "API token scope does not permit this resource".to_string(),
            },
        )
        .await;
    }

    if mcp_rest_rbac_denied(
        &state,
        &request_context,
        tool.action,
        tool.resource_type,
        resource_id.as_deref(),
    )
    .await
    {
        return reject_generic_tool(
            &state,
            audit_auth_context.as_ref(),
            &request_context,
            id,
            GenericToolRejection {
                tool_name: tool.name,
                action: tool.action,
                resource_type: tool.resource_type,
                resource_id: resource_id.as_deref(),
                outcome: "rbac_denied",
                code: "RBAC_DENIED",
                message: "API token scope permits this MCP tool, but RBAC denies the token principal. Assign a direct role or policy to the API token user/service principal.".to_string(),
            },
        )
        .await;
    }

    match dispatch_rest_tool(
        RestDispatchContext {
            state: state.clone(),
            request_id,
            auth_context,
            headers,
        },
        &tool,
        resource_id.as_deref(),
        &call.arguments,
    )
    .await
    {
        Ok(value) => {
            let status = rest_status(&value);
            require_mcp_tool_audit!(
                id,
                record_mcp_tool_audit(
                    &state,
                    audit_auth_context.as_ref(),
                    &request_context,
                    McpToolAudit {
                        tool_name: tool.name,
                        action: tool.action,
                        resource_type: tool.resource_type,
                        resource_id: resource_id.as_deref(),
                        decision: rest_decision(status),
                        rest_status: status,
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
                        tool_name: tool.name,
                        action: tool.action,
                        resource_type: tool.resource_type,
                        resource_id: resource_id.as_deref(),
                        decision: "dispatch_failed",
                        rest_status: None,
                        error_code: Some("REST_DISPATCH_FAILED")
                    }
                )
            );
            jsonrpc_result(
                id,
                tool_error(json!({
                    "code": "REST_DISPATCH_FAILED",
                    "message": error
                })),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::api::mcp::test_support::*;

    use super::{CCTV_PLAYBACK_TOOL_NAME, DedicatedToolHandler, dedicated_tool_handler};

    #[test]
    fn dedicated_tool_route_table_covers_every_specialized_handler() {
        let cases = [
            (
                MCP_CAPABILITIES_TOOL_NAME,
                DedicatedToolHandler::Capabilities,
            ),
            (GENERIC_REST_TOOL_NAME, DedicatedToolHandler::GenericRest),
            (
                SCOPED_REST_LIST_ENDPOINTS_TOOL_NAME,
                DedicatedToolHandler::ScopedRestList,
            ),
            (
                SCOPED_REST_CALL_ENDPOINT_TOOL_NAME,
                DedicatedToolHandler::ScopedRestCall,
            ),
            (
                InventoryToolOperation::List.name(),
                DedicatedToolHandler::Inventory,
            ),
            (
                AccessControlToolOperation::List.name(),
                DedicatedToolHandler::AccessControl,
            ),
            (
                HOST_SESSION_CLIENT_CREDENTIALS_TOOL_NAME,
                DedicatedToolHandler::HostSessionClientCredentials,
            ),
            (CCTV_EXPORT_TOOL_NAME, DedicatedToolHandler::CctvExport),
            (CCTV_PLAYBACK_TOOL_NAME, DedicatedToolHandler::CctvPlayback),
            (CCTV_SNAPSHOT_TOOL_NAME, DedicatedToolHandler::CctvSnapshot),
            (HOST_SSH_OPEN_TOOL_NAME, DedicatedToolHandler::HostSsh),
            (HOST_VNC_OPEN_TOOL_NAME, DedicatedToolHandler::HostVnc),
        ];

        for (name, expected) in cases {
            assert_eq!(dedicated_tool_handler(name), Some(expected), "{name}");
        }
        assert_eq!(dedicated_tool_handler("piping.unknown"), None);
    }

    #[test]
    fn body_pointer_resource_id_is_prefixed_for_scope_matching() {
        let tool = mcp_tools()
            .into_iter()
            .find(|tool| {
                matches!(
                    tool.resource_id_source,
                    ResourceIdSource::BodyPointer { .. }
                )
            })
            .into_iter()
            .next();
        assert!(tool.is_some());
        if let Some(tool) = tool {
            let resource_id = resource_id_for_scope(
                &tool,
                &json!({
                    "body": {
                        "target_id": "host-1",
                        "protocol": "ssh"
                    }
                }),
            );
            assert_eq!(
                resource_id,
                Ok(Some("connection_target:host-1".to_string()))
            );
        }
    }

    #[test]
    fn inventory_rest_dispatch_uses_resource_override_without_wildcard_scope() {
        let tool = mcp_tools()
            .into_iter()
            .find(|tool| tool.name == "piping.rest.get_device")
            .expect("get_device tool");
        let auth_context = AuthContext {
            user_id: Some(uuid::Uuid::nil()),
            auth_method: "api_token".to_string(),
            oidc_groups: Vec::new(),
            api_token_id: Some("token".to_string()),
            api_token_scopes: vec![ApiTokenScopeContext {
                action: "read".to_string(),
                resource_type: "inventory".to_string(),
                resource_pattern: "device:dev-1".to_string(),
            }],
            api_token_scope_resource_override: None,
        };

        let bridged = rest_dispatch_auth_context(auth_context, &tool, Some("device:dev-1"));

        assert!(bridged.api_token_scopes.iter().all(|scope| {
            !(scope.action == "read"
                && scope.resource_type == "inventory"
                && scope.resource_pattern == "*")
        }));
        assert_eq!(
            bridged
                .api_token_scope_resource_override
                .as_ref()
                .map(|value| value.resource_id.as_str()),
            Some("device:dev-1")
        );

        let request_context = RequestContext::new(
            bridged.user_id,
            bridged.auth_method,
            None,
            "request".to_string(),
            crate::contexts::shared::request_context::RequestAuth {
                oidc_groups: bridged.oidc_groups,
                api_token_scopes: bridged.api_token_scopes,
                api_token_scope_resource_override: bridged.api_token_scope_resource_override,
            },
        );
        assert!(request_context.allows_api_token_scope("read", "inventory"));
    }

    #[test]
    fn targeted_rest_dispatch_uses_resource_override_without_wildcard_scope() {
        let tool = mcp_tools()
            .into_iter()
            .find(|tool| tool.name == "piping.rest.get_config_context")
            .expect("get_config_context tool");
        let auth_context = AuthContext {
            user_id: Some(uuid::Uuid::nil()),
            auth_method: "api_token".to_string(),
            oidc_groups: Vec::new(),
            api_token_id: Some("token".to_string()),
            api_token_scopes: vec![ApiTokenScopeContext {
                action: "read".to_string(),
                resource_type: "config_context".to_string(),
                resource_pattern: "config_context:ctx-1".to_string(),
            }],
            api_token_scope_resource_override: None,
        };

        let dispatched =
            rest_dispatch_auth_context(auth_context, &tool, Some("config_context:ctx-1"));

        assert!(dispatched.api_token_scopes.iter().all(|scope| {
            !(scope.action == "read"
                && scope.resource_type == "config_context"
                && scope.resource_pattern == "*")
        }));
        assert_eq!(
            dispatched
                .api_token_scope_resource_override
                .as_ref()
                .map(|value| value.resource_id.as_str()),
            Some("config_context:ctx-1")
        );
    }

    #[test]
    fn query_param_resource_id_is_prefixed_for_scope_matching() {
        let tool = mcp_tools()
            .into_iter()
            .find(|tool| matches!(tool.resource_id_source, ResourceIdSource::QueryParam { .. }))
            .into_iter()
            .next();
        assert!(tool.is_some());
        if let Some(tool) = tool {
            let resource_id = resource_id_for_scope(
                &tool,
                &json!({
                    "query": {
                        "context_id": "ctx-1"
                    }
                }),
            );
            assert_eq!(resource_id, Ok(Some("config_context:ctx-1".to_string())));
        }
    }

    /// The internal REST re-dispatch must forward provenance headers only.
    ///
    /// `copy_forwarded_headers` is an allowlist, and that is the entire
    /// protection: the MCP request arrives with the caller's own credentials,
    /// and the re-dispatched request carries a server-side `AuthContext`
    /// injected as an extension instead. Letting `authorization`, `cookie`, or
    /// the proxy shared secret through would hand the inner handler a second,
    /// caller-controlled identity — and adding one line to the list is all it
    /// would take. Nothing else in the suite reads this list.
    #[test]
    fn forwarded_header_allowlist_carries_no_credentials() {
        use axum::http::{HeaderMap, HeaderValue};

        const CREDENTIAL_HEADERS: &[&str] = &[
            "authorization",
            "cookie",
            "x-piping-proxy-secret",
            "x-piping-peer-bearer",
            "proxy-authorization",
        ];

        let mut source = HeaderMap::new();
        for name in CREDENTIAL_HEADERS {
            source.insert(
                super::HeaderName::from_static(name),
                HeaderValue::from_static("leaked"),
            );
        }
        // One allowlisted header proves the copy actually ran.
        source.insert(
            super::HeaderName::from_static("x-request-id"),
            HeaderValue::from_static("req-1"),
        );

        let mut target = HeaderMap::new();
        super::copy_forwarded_headers(&source, &mut target);

        assert_eq!(
            target
                .get("x-request-id")
                .and_then(|value| value.to_str().ok()),
            Some("req-1"),
            "provenance headers must still be forwarded"
        );
        for name in CREDENTIAL_HEADERS {
            assert!(
                target.get(*name).is_none(),
                "credential header `{name}` was forwarded into the internal re-dispatch"
            );
        }
    }
}
