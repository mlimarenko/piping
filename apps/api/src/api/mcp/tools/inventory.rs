use super::super::*;

pub(in crate::api::mcp) async fn handle_inventory_tool_call(
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
    let Some(operation) = InventoryToolOperation::from_name(tool_name.as_str()) else {
        return jsonrpc_error(
            id,
            -32602,
            "Unknown tool",
            Some(json!({ "name": tool_name })),
        );
    };

    if !inventory_tool_visible(&request_context, operation) {
        require_mcp_tool_audit!(
            id,
            record_mcp_tool_audit(
                &state,
                audit_auth_context.as_ref(),
                &request_context,
                McpToolAudit {
                    tool_name: tool_name.as_str(),
                    action: operation.action(),
                    resource_type: "inventory",
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
                "message": "API token scope does not expose this inventory MCP tool"
            })),
        );
    }

    let rest_call = match inventory_rest_call(operation, &arguments) {
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
                        action: operation.action(),
                        resource_type: "inventory",
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

    if !request_context.allows_api_token_scope_for_resource(
        rest_call.action,
        "inventory",
        Some(rest_call.resource_id.as_str()),
    ) {
        require_mcp_tool_audit!(
            id,
            record_mcp_tool_audit(
                &state,
                audit_auth_context.as_ref(),
                &request_context,
                McpToolAudit {
                    tool_name: tool_name.as_str(),
                    action: rest_call.action,
                    resource_type: "inventory",
                    resource_id: Some(rest_call.resource_id.as_str()),
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
                "message": "API token scope does not permit this inventory entity"
            })),
        );
    }

    let dispatch_auth_context = auth_context.map(|context| {
        auth_context_with_resource_override(
            context,
            rest_call.action,
            "inventory",
            rest_call.resource_id.clone(),
        )
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
                        resource_type: "inventory",
                        resource_id: Some(rest_call.resource_id.as_str()),
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
                        resource_type: "inventory",
                        resource_id: Some(rest_call.resource_id.as_str()),
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
    fn inventory_entity_scope_exposes_parameterized_read_tools() {
        let request_context = context("read", "inventory", "inventory_entity:devices");
        let tools = visible_tools(&request_context);

        assert!(tools.iter().any(|tool| {
            tool.get("name").and_then(|value| value.as_str()) == Some(INVENTORY_LIST_TOOL_NAME)
        }));
        assert!(tools.iter().any(|tool| {
            tool.get("name").and_then(|value| value.as_str()) == Some(INVENTORY_GET_TOOL_NAME)
        }));
        assert!(tools.iter().all(|tool| {
            tool.get("name").and_then(|value| value.as_str()) != Some(INVENTORY_CREATE_TOOL_NAME)
        }));
    }

    #[test]
    fn inventory_tool_schema_lists_only_authorized_entities() {
        let request_context = context("read", "inventory", "inventory_entity:devices");
        let tools = visible_tools(&request_context);
        let list_tool = tools
            .iter()
            .find(|tool| {
                tool.get("name").and_then(|value| value.as_str()) == Some(INVENTORY_LIST_TOOL_NAME)
            })
            .expect("list tool is visible");
        let entities = list_tool
            .pointer("/inputSchema/properties/entity_type/enum")
            .and_then(|value| value.as_array())
            .expect("entity enum is present")
            .iter()
            .filter_map(|value| value.as_str())
            .collect::<Vec<_>>();

        assert_eq!(entities, vec!["devices"]);
    }

    #[test]
    fn inventory_entity_scope_is_checked_against_call_entity() {
        let request_context = context("read", "inventory", "inventory_entity:devices");
        let devices = inventory_route_for_entity("devices").expect("devices route");
        let sites = inventory_route_for_entity("sites").expect("sites route");
        let devices_resource_id = inventory_entity_resource_id(devices);
        let sites_resource_id = inventory_entity_resource_id(sites);

        assert!(request_context.allows_api_token_scope_for_resource(
            "read",
            "inventory",
            Some(devices_resource_id.as_str())
        ));
        assert!(!request_context.allows_api_token_scope_for_resource(
            "read",
            "inventory",
            Some(sites_resource_id.as_str())
        ));
    }

    #[test]
    fn inventory_rest_call_uses_closed_entity_manifest() {
        let devices = inventory_rest_call(
            InventoryToolOperation::List,
            &json!({
                "entity_type": "devices",
                "query": {
                    "limit": 10
                }
            }),
        )
        .expect("devices list is allowed");
        assert_eq!(devices.uri, "/api/v1/inventory/devices?limit=10");
        assert_eq!(devices.resource_id, "inventory_entity:devices");

        assert!(
            inventory_rest_call(
                InventoryToolOperation::List,
                &json!({
                    "entity_type": "../devices"
                }),
            )
            .is_err()
        );
        assert!(
            inventory_rest_call(
                InventoryToolOperation::List,
                &json!({
                    "entity_type": "hosts"
                }),
            )
            .is_err()
        );
    }

    #[test]
    fn inventory_rest_call_rejects_invalid_ids_and_unsupported_bulk() {
        assert!(
            inventory_rest_call(
                InventoryToolOperation::Get,
                &json!({
                    "entity_type": "devices",
                    "id": "bad/id"
                }),
            )
            .is_err()
        );
        assert!(
            inventory_rest_call(
                InventoryToolOperation::BulkCreate,
                &json!({
                    "entity_type": "ip-addresses",
                    "body": {
                        "items": []
                    }
                }),
            )
            .is_err()
        );
        assert!(
            inventory_rest_call(
                InventoryToolOperation::Create,
                &json!({
                    "entity_type": "devices"
                }),
            )
            .is_err()
        );
    }

    #[test]
    fn inventory_bulk_mutate_uses_entity_specific_rest_bulk_endpoint_and_action() {
        let device_update = inventory_rest_call(
            InventoryToolOperation::BulkMutate,
            &json!({
                "entity_type": "devices",
                "body": {
                    "operation": "set_status",
                    "items": [{ "id": "019e8f40-0000-7000-8000-000000000001" }],
                    "params": { "status": "offline" }
                }
            }),
        )
        .expect("device status bulk update is allowed");
        assert_eq!(device_update.action, "update");
        assert_eq!(
            device_update.uri,
            "/api/v1/inventory/resources/devices/bulk"
        );
        assert_eq!(device_update.resource_id, "inventory_entity:devices");

        let cluster_delete = inventory_rest_call(
            InventoryToolOperation::BulkMutate,
            &json!({
                "entity_type": "clusters",
                "body": {
                    "operation": "delete",
                    "items": [{ "id": "019e8f40-0000-7000-8000-000000000002" }]
                }
            }),
        )
        .expect("cluster bulk delete is allowed");
        assert_eq!(cluster_delete.action, "delete");
        assert_eq!(
            cluster_delete.uri,
            "/api/v1/inventory/resources/clusters/bulk"
        );
        assert_eq!(cluster_delete.resource_id, "inventory_entity:clusters");
    }

    #[test]
    fn inventory_bulk_mutate_rejects_unsupported_operation_before_dispatch() {
        assert!(matches!(
            inventory_rest_call(
                InventoryToolOperation::BulkMutate,
                &json!({
                    "entity_type": "clusters",
                    "body": {
                        "operation": "set_status",
                        "items": [{ "id": "019e8f40-0000-7000-8000-000000000003" }]
                    }
                }),
            ),
            Err(error) if error.code == "UNSUPPORTED_BULK_OPERATION"
        ));
        assert!(matches!(
            inventory_rest_call(
                InventoryToolOperation::BulkMutate,
                &json!({
                    "entity_type": "devices",
                    "body": {
                        "operation": "delete",
                        "items": []
                    }
                }),
            ),
            Err(error) if error.code == "INVALID_ARGUMENTS"
        ));
        assert!(matches!(
            inventory_rest_call(
                InventoryToolOperation::BulkMutate,
                &json!({
                    "entity_type": "hosts",
                    "body": {
                        "operation": "delete",
                        "items": [{ "id": "019e8f40-0000-7000-8000-000000000004" }]
                    }
                }),
            ),
            Err(error) if error.code == "UNKNOWN_INVENTORY_ENTITY"
        ));
    }

    #[test]
    fn inventory_bulk_mutate_visibility_follows_resolved_action_scope() {
        let update_context = context("update", "inventory", "inventory_entity:devices");
        let update_tool = visible_tools(&update_context)
            .into_iter()
            .find(|tool| {
                tool.get("name").and_then(|value| value.as_str())
                    == Some(INVENTORY_BULK_MUTATE_TOOL_NAME)
            })
            .expect("bulk mutate is visible for device update scope");
        let update_entities = update_tool
            .pointer("/inputSchema/properties/entity_type/enum")
            .and_then(|value| value.as_array())
            .expect("entity enum is present")
            .iter()
            .filter_map(|value| value.as_str())
            .collect::<Vec<_>>();
        assert_eq!(update_entities, vec!["devices"]);

        let delete_context = context("delete", "inventory", "inventory_entity:clusters");
        let delete_tool = visible_tools(&delete_context)
            .into_iter()
            .find(|tool| {
                tool.get("name").and_then(|value| value.as_str())
                    == Some(INVENTORY_BULK_MUTATE_TOOL_NAME)
            })
            .expect("bulk mutate is visible for cluster delete scope");
        let delete_entities = delete_tool
            .pointer("/inputSchema/properties/entity_type/enum")
            .and_then(|value| value.as_array())
            .expect("entity enum is present")
            .iter()
            .filter_map(|value| value.as_str())
            .collect::<Vec<_>>();
        assert_eq!(delete_entities, vec!["clusters"]);
    }

    #[test]
    fn create_inventory_scope_exposes_bulk_create_when_available() {
        let request_context = context("create", "inventory", "inventory_entity:devices");
        let tools = visible_tools(&request_context);

        assert!(tools.iter().any(|tool| {
            tool.get("name").and_then(|value| value.as_str()) == Some(INVENTORY_CREATE_TOOL_NAME)
        }));
        assert!(tools.iter().any(|tool| {
            tool.get("name").and_then(|value| value.as_str())
                == Some(INVENTORY_BULK_CREATE_TOOL_NAME)
        }));
        assert!(tools.iter().all(|tool| {
            tool.get("name").and_then(|value| value.as_str()) != Some(INVENTORY_LIST_TOOL_NAME)
        }));
    }

    #[test]
    fn create_inventory_scope_hides_bulk_create_without_supported_entity() {
        let request_context = context("create", "inventory", "inventory_entity:ip-addresses");
        let tools = visible_tools(&request_context);

        assert!(tools.iter().any(|tool| {
            tool.get("name").and_then(|value| value.as_str()) == Some(INVENTORY_CREATE_TOOL_NAME)
        }));
        assert!(tools.iter().all(|tool| {
            tool.get("name").and_then(|value| value.as_str())
                != Some(INVENTORY_BULK_CREATE_TOOL_NAME)
        }));
    }
}
