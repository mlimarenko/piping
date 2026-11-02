use super::super::*;

pub(in crate::api::mcp) async fn handle_access_control_tool_call(
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
    let Some(operation) = AccessControlToolOperation::from_name(tool_name.as_str()) else {
        return jsonrpc_error(
            id,
            -32602,
            "Unknown tool",
            Some(json!({ "name": tool_name })),
        );
    };

    if !access_control_tool_visible(&request_context, operation) {
        require_mcp_tool_audit!(
            id,
            record_mcp_tool_audit(
                &state,
                audit_auth_context.as_ref(),
                &request_context,
                McpToolAudit {
                    tool_name: tool_name.as_str(),
                    action: operation.action().unwrap_or("bulk"),
                    resource_type: "access_control",
                    resource_id: None,
                    decision: "scope_denied",
                    rest_status: None,
                    error_code: Some("TOKEN_SCOPE_DENIED")
                }
            )
        );
        return jsonrpc_result(
            id,
            tool_error(json!({
                "code": "TOKEN_SCOPE_DENIED",
                "message": "API token scope does not expose this access-control MCP tool"
            })),
        );
    }

    let rest_call = match access_control_rest_call(operation, &arguments, &request_context) {
        Ok(value) => value,
        Err(error) => {
            require_mcp_tool_audit!(
                id,
                record_mcp_tool_audit(
                    &state,
                    audit_auth_context.as_ref(),
                    &request_context,
                    McpToolAudit {
                        tool_name: tool_name.as_str(),
                        action: operation.action().unwrap_or("bulk"),
                        resource_type: "access_control",
                        resource_id: error.resource_id.as_deref(),
                        decision: "invalid_arguments",
                        rest_status: None,
                        error_code: Some(error.code)
                    }
                )
            );
            return jsonrpc_result(
                id,
                tool_error(json!({
                    "code": error.code,
                    "message": error.message
                })),
            );
        }
    };

    let dispatch_auth_context = auth_context.map(|context| {
        if let Some(resource_id) = rest_call.resource_id.as_ref() {
            auth_context_with_resource_override(
                context,
                rest_call.action,
                rest_call.resource_type,
                resource_id.clone(),
            )
        } else {
            context
        }
    });

    match dispatch_rest_request(
        RestDispatchContext {
            state: state.clone(),
            request_id,
            auth_context: dispatch_auth_context,
            headers,
        },
        operation.method(),
        rest_call.uri,
        rest_call.body,
        None,
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
                        tool_name: tool_name.as_str(),
                        action: rest_call.action,
                        resource_type: rest_call.resource_type,
                        resource_id: rest_call.resource_id.as_deref(),
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
                        tool_name: tool_name.as_str(),
                        action: rest_call.action,
                        resource_type: rest_call.resource_type,
                        resource_id: rest_call.resource_id.as_deref(),
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

    #[test]
    fn access_control_resource_scope_exposes_targeted_tools_only() {
        let request_context = context("read", "user", "user:user-1");
        let tool_names = visible_tool_names(&request_context);

        assert!(tool_names.contains(&ACCESS_CONTROL_GET_TOOL_NAME.to_string()));
        assert!(!tool_names.contains(&ACCESS_CONTROL_LIST_TOOL_NAME.to_string()));
    }

    #[test]
    fn access_control_tool_schema_lists_only_authorized_entities() {
        let request_context = RequestContext::new(
            Some(uuid::Uuid::nil()),
            "api_token".to_string(),
            None,
            "request".to_string(),
            crate::contexts::shared::request_context::RequestAuth {
                oidc_groups: Vec::new(),
                api_token_scopes: vec![
                    ApiTokenScopeContext {
                        action: "read".to_string(),
                        resource_type: "user".to_string(),
                        resource_pattern: "user:user-1".to_string(),
                    },
                    ApiTokenScopeContext {
                        action: "read".to_string(),
                        resource_type: "group".to_string(),
                        resource_pattern: "*".to_string(),
                    },
                ],
                api_token_scope_resource_override: None,
            },
        );

        let get_tool = visible_tools(&request_context)
            .into_iter()
            .find(|tool| {
                tool.get("name").and_then(serde_json::Value::as_str)
                    == Some(ACCESS_CONTROL_GET_TOOL_NAME)
            })
            .expect("get access-control tool");
        let entities = get_tool
            .pointer("/inputSchema/properties/entity_type/enum")
            .and_then(serde_json::Value::as_array)
            .expect("entity enum");
        let values: Vec<_> = entities
            .iter()
            .filter_map(serde_json::Value::as_str)
            .collect();
        assert_eq!(values, vec!["users", "groups"]);
    }

    #[test]
    fn access_control_rest_call_uses_closed_entity_manifest() {
        let request_context = context("read", "user", "*");
        let users = access_control_rest_call(
            AccessControlToolOperation::List,
            &json!({
                "entity_type": "users",
                "query": {
                    "limit": 10
                }
            }),
            &request_context,
        )
        .expect("users list call");
        assert_eq!(users.uri, "/api/v1/admin/access-control/users?limit=10");
        assert_eq!(users.resource_type, "user");
        assert_eq!(users.action, "read");

        assert!(
            access_control_rest_call(
                AccessControlToolOperation::List,
                &json!({
                    "entity_type": "../users"
                }),
                &request_context,
            )
            .is_err()
        );
    }

    #[test]
    fn access_control_bulk_requires_wildcard_scope_for_body_actions() {
        let targeted_update = context("update", "user", "user:user-1");
        let body = json!({
            "entity_type": "users",
            "body": {
                "update": [
                    {
                        "id": "user-1",
                        "data": {
                            "username": "alice",
                            "email": "alice@example.com",
                            "status": "active",
                            "auth_source": "local"
                        }
                    }
                ]
            }
        });

        let denied =
            access_control_rest_call(AccessControlToolOperation::Bulk, &body, &targeted_update);
        assert!(denied.is_err());
        let denied = denied
            .err()
            .expect("targeted scope is not enough for bulk update");
        assert_eq!(denied.code, "TOKEN_SCOPE_DENIED");

        let wildcard_update = context("update", "user", "*");
        let allowed =
            access_control_rest_call(AccessControlToolOperation::Bulk, &body, &wildcard_update)
                .expect("wildcard update can use bulk update");
        assert_eq!(allowed.uri, "/api/v1/admin/access-control/users/bulk");
        assert_eq!(allowed.action, "bulk");
        assert_eq!(allowed.resource_type, "user");
    }
}
