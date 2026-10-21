use super::*;

pub(super) fn mcp_tools() -> Vec<McpRestTool> {
    vec![
        McpRestTool {
            name: "piping.rest.list_connection_targets",
            title: "List connection targets",
            description: "List read-only connection targets derived from inventory devices and virtual machines.",
            method: Method::GET,
            path_template: "/api/v1/connections/targets",
            action: "read",
            resource_type: "connection_target",
            resource_id_source: ResourceIdSource::None,
            input_schema: connection_target_list_schema,
        },
        McpRestTool {
            name: "piping.rest.list_devices",
            title: "List devices",
            description: "List inventory devices.",
            method: Method::GET,
            path_template: "/api/v1/inventory/devices",
            action: "read",
            resource_type: "inventory",
            resource_id_source: ResourceIdSource::None,
            input_schema: list_schema,
        },
        McpRestTool {
            name: "piping.rest.get_device",
            title: "Get device",
            description: "Read one inventory device by id.",
            method: Method::GET,
            path_template: "/api/v1/inventory/devices/{id}",
            action: "read",
            resource_type: "inventory",
            resource_id_source: ResourceIdSource::PathParam {
                key: "id",
                prefix: "device:",
            },
            input_schema: path_id_schema,
        },
        McpRestTool {
            name: "piping.rest.list_connection_sessions",
            title: "List connection sessions",
            description: "List connection sessions visible through the current token.",
            method: Method::GET,
            path_template: "/api/v1/connections/sessions",
            action: "read",
            resource_type: "connection_session",
            resource_id_source: ResourceIdSource::None,
            input_schema: list_schema,
        },
        McpRestTool {
            name: CREATE_CONNECTION_SESSION_TOOL_NAME,
            title: "Create connection session",
            description: "Create a token-scoped SSH, VNC, or RDP connection session for a connection target. MCP exposure requires connect:connection_target and secret:connection_target because the REST response includes an interactive proxy websocket token.",
            method: Method::POST,
            path_template: CONNECTION_SESSIONS_PATH,
            action: "connect",
            resource_type: "connection_target",
            resource_id_source: ResourceIdSource::BodyPointer {
                pointer: "/body/target_id",
                prefix: "connection_target:",
            },
            input_schema: create_connection_session_schema,
        },
        McpRestTool {
            name: "piping.rest.disconnect_connection_session",
            title: "Disconnect connection session",
            description: "Disconnect one connection session by id.",
            method: Method::POST,
            path_template: "/api/v1/connections/sessions/{id}/disconnect",
            action: "update",
            resource_type: "connection_session",
            resource_id_source: ResourceIdSource::PathParam {
                key: "id",
                prefix: "connection_session:",
            },
            input_schema: path_id_schema,
        },
        McpRestTool {
            name: "piping.rest.terminate_connection_session",
            title: "Terminate connection session",
            description: "Terminate one connection session by id.",
            method: Method::POST,
            path_template: "/api/v1/connections/sessions/{id}/terminate",
            action: "update",
            resource_type: "connection_session",
            resource_id_source: ResourceIdSource::PathParam {
                key: "id",
                prefix: "connection_session:",
            },
            input_schema: path_id_schema,
        },
        McpRestTool {
            name: "piping.rest.list_connection_templates",
            title: "List connection templates",
            description: "List connection templates available for launching sessions.",
            method: Method::GET,
            path_template: "/api/v1/connections/templates",
            action: "read",
            resource_type: "connection_session",
            resource_id_source: ResourceIdSource::None,
            input_schema: list_schema,
        },
        McpRestTool {
            name: "piping.rest.list_proxy_nodes",
            title: "List proxy nodes",
            description: "List registered connection proxy nodes.",
            method: Method::GET,
            path_template: "/api/v1/admin/proxy-nodes",
            action: "read",
            resource_type: "connection_session",
            resource_id_source: ResourceIdSource::None,
            input_schema: list_schema,
        },
        McpRestTool {
            name: "piping.rest.get_proxy_node",
            title: "Get proxy node",
            description: "Read one registered connection proxy node.",
            method: Method::GET,
            path_template: "/api/v1/admin/proxy-nodes/{id}",
            action: "read",
            resource_type: "connection_session",
            resource_id_source: ResourceIdSource::PathParam {
                key: "id",
                prefix: "proxy_node:",
            },
            input_schema: path_id_schema,
        },
        McpRestTool {
            name: "piping.rest.create_proxy_node",
            title: "Create proxy node",
            description: "Register a connection proxy node.",
            method: Method::POST,
            path_template: "/api/v1/admin/proxy-nodes",
            action: "create",
            resource_type: "connection_session",
            resource_id_source: ResourceIdSource::None,
            input_schema: proxy_node_payload_schema,
        },
        McpRestTool {
            name: "piping.rest.update_proxy_node",
            title: "Update proxy node",
            description: "Update one registered connection proxy node.",
            method: Method::PUT,
            path_template: "/api/v1/admin/proxy-nodes/{id}",
            action: "update",
            resource_type: "connection_session",
            resource_id_source: ResourceIdSource::PathParam {
                key: "id",
                prefix: "proxy_node:",
            },
            input_schema: path_id_proxy_node_payload_schema,
        },
        McpRestTool {
            name: "piping.rest.delete_proxy_node",
            title: "Delete proxy node",
            description: "Delete one registered connection proxy node.",
            method: Method::DELETE,
            path_template: "/api/v1/admin/proxy-nodes/{id}",
            action: "delete",
            resource_type: "connection_session",
            resource_id_source: ResourceIdSource::PathParam {
                key: "id",
                prefix: "proxy_node:",
            },
            input_schema: path_id_schema,
        },
        McpRestTool {
            name: "piping.rest.get_target_proxy_assignment",
            title: "Get target proxy assignment",
            description: "Read the proxy assignment for one connection target.",
            method: Method::GET,
            path_template: "/api/v1/inventory/targets/{id}/proxy-assignment",
            action: "read",
            resource_type: "connection_target",
            resource_id_source: ResourceIdSource::PathParam {
                key: "id",
                prefix: "connection_target:",
            },
            input_schema: path_id_schema,
        },
        McpRestTool {
            name: "piping.rest.upsert_target_proxy_assignment",
            title: "Set target proxy assignment",
            description: "Assign or clear the proxy node used for one connection target.",
            method: Method::PUT,
            path_template: "/api/v1/inventory/targets/{id}/proxy-assignment",
            action: "update",
            resource_type: "connection_target",
            resource_id_source: ResourceIdSource::PathParam {
                key: "id",
                prefix: "connection_target:",
            },
            input_schema: path_id_target_proxy_assignment_payload_schema,
        },
        McpRestTool {
            name: "piping.rest.clear_target_proxy_assignment",
            title: "Clear target proxy assignment",
            description: "Clear the proxy assignment for one connection target.",
            method: Method::DELETE,
            path_template: "/api/v1/inventory/targets/{id}/proxy-assignment",
            action: "update",
            resource_type: "connection_target",
            resource_id_source: ResourceIdSource::PathParam {
                key: "id",
                prefix: "connection_target:",
            },
            input_schema: path_id_schema,
        },
        McpRestTool {
            name: "piping.rest.list_config_contexts",
            title: "List config contexts",
            description: "List configuration contexts.",
            method: Method::GET,
            path_template: "/api/v1/config-contexts",
            action: "read",
            resource_type: "config_context",
            resource_id_source: ResourceIdSource::None,
            input_schema: list_schema,
        },
        McpRestTool {
            name: "piping.rest.get_config_context",
            title: "Get config context",
            description: "Read one configuration context by id.",
            method: Method::GET,
            path_template: "/api/v1/config-contexts/{id}",
            action: "read",
            resource_type: "config_context",
            resource_id_source: ResourceIdSource::PathParam {
                key: "id",
                prefix: "config_context:",
            },
            input_schema: path_id_schema,
        },
        McpRestTool {
            name: "piping.rest.create_config_context",
            title: "Create config context",
            description: "Create a configuration context.",
            method: Method::POST,
            path_template: "/api/v1/config-contexts",
            action: "create",
            resource_type: "config_context",
            resource_id_source: ResourceIdSource::None,
            input_schema: config_context_payload_schema,
        },
        McpRestTool {
            name: "piping.rest.update_config_context",
            title: "Update config context",
            description: "Update one configuration context by id.",
            method: Method::PUT,
            path_template: "/api/v1/config-contexts/{id}",
            action: "update",
            resource_type: "config_context",
            resource_id_source: ResourceIdSource::PathParam {
                key: "id",
                prefix: "config_context:",
            },
            input_schema: path_id_config_context_payload_schema,
        },
        McpRestTool {
            name: "piping.rest.delete_config_context",
            title: "Delete config context",
            description: "Delete one configuration context by id.",
            method: Method::DELETE,
            path_template: "/api/v1/config-contexts/{id}",
            action: "delete",
            resource_type: "config_context",
            resource_id_source: ResourceIdSource::PathParam {
                key: "id",
                prefix: "config_context:",
            },
            input_schema: path_id_schema,
        },
        McpRestTool {
            name: "piping.rest.list_config_context_bindings",
            title: "List config context bindings",
            description: "List bindings for one configuration context.",
            method: Method::GET,
            path_template: "/api/v1/config-context-bindings",
            action: "read",
            resource_type: "config_context",
            resource_id_source: ResourceIdSource::QueryParam {
                key: "context_id",
                prefix: "config_context:",
            },
            input_schema: config_context_binding_list_schema,
        },
        McpRestTool {
            name: "piping.rest.create_config_context_binding",
            title: "Create config context binding",
            description: "Bind a configuration context to an inventory scope.",
            method: Method::POST,
            path_template: "/api/v1/config-context-bindings",
            action: "create",
            resource_type: "config_context",
            resource_id_source: ResourceIdSource::BodyPointer {
                pointer: "/body/context_id",
                prefix: "config_context:",
            },
            input_schema: config_context_binding_payload_schema,
        },
    ]
}

#[cfg(test)]
mod tests {
    use crate::api::mcp::test_support::*;

    #[test]
    fn mcp_connection_target_tools_do_not_use_the_retired_host_namespace() {
        let request_context = full_context();
        let tool_names = visible_tool_names(&request_context);
        let connection_target_tools = [
            HOST_SSH_OPEN_TOOL_NAME,
            HOST_SSH_EXEC_TOOL_NAME,
            HOST_SSH_WRITE_TOOL_NAME,
            HOST_SSH_READ_TOOL_NAME,
            HOST_SSH_READ_UNTIL_TOOL_NAME,
            HOST_SSH_WAIT_FOR_PROMPT_TOOL_NAME,
            HOST_SSH_TAIL_LOG_TOOL_NAME,
            HOST_VNC_OPEN_TOOL_NAME,
            HOST_VNC_CAPTURE_TOOL_NAME,
            HOST_SESSION_CLIENT_CREDENTIALS_TOOL_NAME,
        ];

        for tool_name in connection_target_tools {
            assert!(
                tool_name.starts_with("piping.connection_target."),
                "{tool_name} must use the connection_target MCP namespace"
            );
            assert!(
                tool_names.iter().any(|name| name == tool_name),
                "{tool_name} should be visible with full token scopes"
            );
        }
        let retired_host_prefix = ["piping", "host"].join(".") + ".";
        assert!(
            tool_names
                .iter()
                .all(|name| !name.starts_with(&retired_host_prefix)),
            "the retired host tool namespace must not be exposed"
        );
    }

    #[test]
    fn list_tools_hides_broad_tools_for_resource_specific_scopes() {
        let request_context = RequestContext::new(
            Some(uuid::Uuid::nil()),
            "api_token".to_string(),
            None,
            "request".to_string(),
            crate::contexts::shared::request_context::RequestAuth {
                oidc_groups: Vec::new(),
                api_token_scopes: vec![ApiTokenScopeContext {
                    action: "read".to_string(),
                    resource_type: "inventory".to_string(),
                    resource_pattern: "device:dev-1".to_string(),
                }],
                api_token_scope_resource_override: None,
            },
        );
        let list_targets = mcp_tools()
            .into_iter()
            .find(|tool| tool.name == "piping.rest.list_connection_targets")
            .into_iter()
            .next();
        assert!(list_targets.is_some());
        if let Some(tool) = list_targets {
            assert!(!tool_visible(&request_context, &tool));
        }
    }

    #[test]
    fn read_connection_target_scope_exposes_connection_target_list() {
        let request_context = context("read", "connection_target", "*");
        let list_targets = mcp_tools()
            .into_iter()
            .find(|tool| tool.name == "piping.rest.list_connection_targets")
            .expect("list connection targets tool");

        assert_eq!(list_targets.resource_type, "connection_target");
        assert!(tool_visible(&request_context, &list_targets));
        assert!(
            visible_tool_names(&request_context)
                .contains(&"piping.rest.list_connection_targets".to_string())
        );
    }

    #[test]
    fn connect_only_scope_does_not_expose_connection_session_creation() {
        let request_context = context("connect", "connection_target", "connection_target:host-1");
        let create_session = mcp_tools()
            .into_iter()
            .find(|tool| tool.name == CREATE_CONNECTION_SESSION_TOOL_NAME)
            .into_iter()
            .next();
        assert!(create_session.is_some());
        if let Some(tool) = create_session {
            assert!(!tool_visible(&request_context, &tool));
        }
        assert!(!has_host_interactive_session_scope(&request_context));
        assert!(
            !visible_tool_names(&request_context)
                .contains(&CREATE_CONNECTION_SESSION_TOOL_NAME.to_string())
        );
        assert!(
            !visible_scoped_rest_endpoints(&request_context)
                .iter()
                .any(|endpoint| endpoint.operation_id == CREATE_CONNECTION_SESSION_OPERATION_ID)
        );
    }

    #[test]
    fn connect_and_secret_scope_exposes_connection_session_creation() {
        let request_context = context_with_scopes(vec![
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
        ]);
        let create_session = mcp_tools()
            .into_iter()
            .find(|tool| tool.name == CREATE_CONNECTION_SESSION_TOOL_NAME)
            .expect("create session tool");

        assert!(has_host_interactive_session_scope(&request_context));
        assert!(tool_visible(&request_context, &create_session));
        assert!(allows_host_interactive_session_for_resource(
            &request_context,
            Some("connection_target:host-1")
        ));
        assert!(!allows_host_interactive_session_for_resource(
            &request_context,
            Some("connection_target:host-2")
        ));
        assert!(
            visible_scoped_rest_endpoints(&request_context)
                .iter()
                .any(|endpoint| endpoint.operation_id == CREATE_CONNECTION_SESSION_OPERATION_ID)
        );
    }

    #[test]
    fn disjoint_connect_and_secret_scopes_hide_interactive_session_creation() {
        let request_context = context_with_scopes(vec![
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
        ]);
        let create_session = mcp_tools()
            .into_iter()
            .find(|tool| tool.name == CREATE_CONNECTION_SESSION_TOOL_NAME)
            .expect("create session tool");

        assert!(!has_host_interactive_session_scope(&request_context));
        assert!(!tool_visible(&request_context, &create_session));
        assert!(!allows_host_interactive_session_for_resource(
            &request_context,
            Some("connection_target:host-1")
        ));
    }

    #[test]
    fn only_session_creating_ssh_tools_get_interactive_gate() {
        assert!(ssh_tool_creates_connection_session(HOST_SSH_OPEN_TOOL_NAME));
        assert!(ssh_tool_creates_connection_session(HOST_SSH_EXEC_TOOL_NAME));
        assert!(ssh_tool_creates_connection_session(
            HOST_SSH_TAIL_LOG_TOOL_NAME
        ));
        assert!(ssh_tool_creates_connection_session(HOST_INSPECT_TOOL_NAME));
        assert!(ssh_tool_creates_connection_session(
            HOST_CHECK_PORT_TOOL_NAME
        ));
        assert!(ssh_tool_creates_connection_session(
            HOST_READ_FILE_TOOL_NAME
        ));
        assert!(ssh_tool_creates_connection_session(
            HOST_FIND_LOGS_TOOL_NAME
        ));
        assert!(!ssh_tool_creates_connection_session(
            HOST_SSH_READ_TOOL_NAME
        ));
        assert!(!ssh_tool_creates_connection_session(
            HOST_SSH_WRITE_TOOL_NAME
        ));
    }

    #[test]
    fn config_context_resource_scope_exposes_targeted_tools_only() {
        let request_context = context("read", "config_context", "config_context:ctx-1");
        let tools = mcp_tools();

        let list_contexts = tools
            .iter()
            .find(|tool| tool.name == "piping.rest.list_config_contexts");
        let get_context = tools
            .iter()
            .find(|tool| tool.name == "piping.rest.get_config_context");

        assert!(list_contexts.is_some());
        assert!(get_context.is_some());
        assert!(!tool_visible(&request_context, list_contexts.unwrap()));
        assert!(tool_visible(&request_context, get_context.unwrap()));
    }

    #[test]
    fn create_config_context_requires_wildcard_create_scope() {
        let request_context = context("create", "config_context", "config_context:ctx-1");
        let create_context = mcp_tools()
            .into_iter()
            .find(|tool| tool.name == "piping.rest.create_config_context")
            .into_iter()
            .next();

        assert!(create_context.is_some());
        if let Some(tool) = create_context {
            assert!(!tool_visible(&request_context, &tool));
        }
    }

    #[test]
    fn binding_delete_is_not_exposed_without_context_id_lookup() {
        assert!(
            mcp_tools()
                .iter()
                .all(|tool| tool.name != "piping.rest.delete_config_context_binding")
        );
    }

    #[test]
    fn generic_rest_tool_requires_full_wildcard_control_plane_scope() {
        let narrow_context = context("read", "inventory", "*");
        assert!(!has_full_control_plane_scope(&narrow_context));
        assert!(
            visible_tools(&narrow_context)
                .iter()
                .all(|tool| tool.get("name").and_then(|value| value.as_str())
                    != Some(GENERIC_REST_TOOL_NAME))
        );

        let full_context = full_context();
        assert!(has_full_control_plane_scope(&full_context));
        assert!(visible_tools(&full_context).iter().any(|tool| {
            tool.get("name").and_then(|value| value.as_str()) == Some(GENERIC_REST_TOOL_NAME)
        }));
    }

    #[test]
    fn mcp_tool_metadata_classification_covers_new_groups() {
        assert_eq!(mcp_tool_group(HOST_INSPECT_TOOL_NAME), "target_diagnostics");
        assert_eq!(mcp_tool_group(HOST_VNC_CAPTURE_TOOL_NAME), "vnc_observe");
        assert_eq!(mcp_tool_group(HOST_VNC_POINTER_TOOL_NAME), "vnc_control");
        assert_eq!(mcp_tool_protocol(HOST_READ_FILE_TOOL_NAME), Some("ssh"));
        assert_eq!(
            mcp_tool_risk(HOST_SESSION_CLIENT_CREDENTIALS_TOOL_NAME),
            "interactive_session"
        );
    }

    #[test]
    fn proxy_node_and_target_assignment_tools_use_scoped_resource_ids() {
        let proxy_node_tool = mcp_tools()
            .into_iter()
            .find(|tool| tool.name == "piping.rest.get_proxy_node")
            .expect("get_proxy_node tool");
        assert_eq!(
            resource_id_for_scope(
                &proxy_node_tool,
                &json!({
                    "id": "proxy-1"
                })
            ),
            Ok(Some("proxy_node:proxy-1".to_string()))
        );

        let assignment_tool = mcp_tools()
            .into_iter()
            .find(|tool| tool.name == "piping.rest.get_target_proxy_assignment")
            .expect("get_target_proxy_assignment tool");
        assert_eq!(
            resource_id_for_scope(
                &assignment_tool,
                &json!({
                    "path_params": {
                        "id": "target-1"
                    }
                })
            ),
            Ok(Some("connection_target:target-1".to_string()))
        );
    }
}
