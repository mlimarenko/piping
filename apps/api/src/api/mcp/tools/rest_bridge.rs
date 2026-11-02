use super::super::*;

pub(in crate::api::mcp) async fn handle_generic_rest_call(
    context: McpToolCallContext,
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
    if !has_full_control_plane_scope(&request_context) {
        require_mcp_tool_audit!(
            id,
            record_mcp_tool_audit(
                &state,
                audit_auth_context.as_ref(),
                &request_context,
                McpToolAudit {
                    tool_name: GENERIC_REST_TOOL_NAME,
                    action: "call",
                    resource_type: "rest",
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
                "message": "The generic REST MCP tool requires wildcard scopes for the full API-token control-plane catalog"
            })),
        );
    }

    let plan = match generic_rest_call_plan(&arguments) {
        Ok(value) => value,
        Err(error) => {
            require_mcp_tool_audit!(
                id,
                record_mcp_tool_audit(
                    &state,
                    audit_auth_context.as_ref(),
                    &request_context,
                    McpToolAudit {
                        tool_name: GENERIC_REST_TOOL_NAME,
                        action: "call",
                        resource_type: "rest",
                        resource_id: None,
                        decision: "invalid_arguments",
                        rest_status: None,
                        error_code: Some("INVALID_ARGUMENTS")
                    }
                )
            );
            return jsonrpc_result(
                id,
                tool_error(json!({
                    "code": "INVALID_ARGUMENTS",
                    "message": error
                })),
            );
        }
    };
    if generic_rest_is_connection_session_create(&plan.method, plan.uri.as_str()) {
        let resource_id = connection_session_create_resource_id_from_arguments(&arguments).ok();
        if !allows_host_interactive_session_for_resource(&request_context, resource_id.as_deref()) {
            require_mcp_tool_audit!(
                id,
                record_mcp_tool_audit(
                    &state,
                    audit_auth_context.as_ref(),
                    &request_context,
                    McpToolAudit {
                        tool_name: GENERIC_REST_TOOL_NAME,
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
                    "message": "MCP generic REST cannot create connection sessions unless target_id is present and the token has connect:connection_target plus secret:connection_target for that target"
                })),
            );
        }
    }
    dispatch_generic_rest_call(
        RestDispatchContext {
            state: state.clone(),
            request_id,
            auth_context,
            headers,
        },
        audit_auth_context,
        request_context,
        id,
        plan,
    )
    .await
}

/// The parsed method, URI and body of one generic REST tool call.
struct GenericRestCallPlan {
    method: Method,
    uri: String,
    body: Body,
}

fn generic_rest_call_plan(arguments: &Value) -> Result<GenericRestCallPlan, String> {
    Ok(GenericRestCallPlan {
        method: generic_rest_method(arguments)?,
        uri: generic_rest_uri(arguments)?,
        body: request_body(arguments)?,
    })
}

async fn dispatch_generic_rest_call(
    rest: RestDispatchContext,
    audit_auth_context: Option<AuthContext>,
    request_context: RequestContext,
    id: Option<Value>,
    plan: GenericRestCallPlan,
) -> Response {
    let RestDispatchContext {
        state,
        request_id,
        auth_context,
        headers,
    } = rest;
    match dispatch_rest_request(
        RestDispatchContext {
            state: state.clone(),
            request_id,
            auth_context,
            headers,
        },
        plan.method,
        plan.uri,
        plan.body,
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
                        tool_name: GENERIC_REST_TOOL_NAME,
                        action: "call",
                        resource_type: "rest",
                        resource_id: None,
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
                        tool_name: GENERIC_REST_TOOL_NAME,
                        action: "call",
                        resource_type: "rest",
                        resource_id: None,
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

/// The endpoint and resource id an authorized scoped-REST tool call resolved to.
struct ScopedRestAuthorization {
    endpoint: McpRestEndpoint,
    resource_id: Option<String>,
}

/// Enforce the endpoint-scope gates that every scoped-REST tool call shares.
///
/// Returns the resolved endpoint and resource id on success, or the JSON-RPC
/// response that already audited the denial.
async fn authorize_scoped_rest_call(
    state: &AppState,
    audit_auth_context: Option<&AuthContext>,
    request_context: &RequestContext,
    id: Option<Value>,
    arguments: &Value,
) -> Result<ScopedRestAuthorization, Box<Response>> {
    let operation_id = match required_endpoint_operation_id(arguments) {
        Ok(value) => value,
        Err(error) => {
            return Err(Box::new(
                reject_scoped_rest_call(
                    state,
                    audit_auth_context,
                    request_context,
                    id,
                    ScopedRestRejection {
                        action: "call",
                        resource_type: "rest",
                        resource_id: None,
                        outcome: "invalid_arguments",
                        code: "INVALID_ARGUMENTS",
                        message: error,
                    },
                )
                .await,
            ));
        }
    };
    let Some(endpoint) = mcp_rest_endpoint_for_operation_id(operation_id) else {
        return Err(Box::new(
            reject_scoped_rest_call(
                state,
                audit_auth_context,
                request_context,
                id,
                ScopedRestRejection {
                    action: "call",
                    resource_type: "rest",
                    resource_id: None,
                    outcome: "unknown_operation",
                    code: "UNKNOWN_OPERATION",
                    message: format!("REST operation '{operation_id}' is not exposed through MCP"),
                },
            )
            .await,
        ));
    };

    if !scoped_rest_endpoint_visible(request_context, &endpoint) {
        return Err(Box::new(
            reject_scoped_rest_call(
                state,
                audit_auth_context,
                request_context,
                id,
                ScopedRestRejection {
                    action: endpoint.action,
                    resource_type: endpoint.resource_type,
                    resource_id: None,
                    outcome: "scope_denied",
                    code: "TOKEN_SCOPE_DENIED",
                    message: "API token scope does not expose this REST operation".to_owned(),
                },
            )
            .await,
        ));
    }

    let resource_id = match resource_id_for_endpoint(&endpoint, arguments) {
        Ok(value) => value,
        Err(error) => {
            return Err(Box::new(
                reject_scoped_rest_call(
                    state,
                    audit_auth_context,
                    request_context,
                    id,
                    ScopedRestRejection {
                        action: endpoint.action,
                        resource_type: endpoint.resource_type,
                        resource_id: None,
                        outcome: "invalid_arguments",
                        code: "INVALID_ARGUMENTS",
                        message: error,
                    },
                )
                .await,
            ));
        }
    };

    if endpoint.full_control_only && !has_full_control_plane_scope(request_context) {
        return Err(Box::new(
            reject_scoped_rest_call(
                state,
                audit_auth_context,
                request_context,
                id,
                ScopedRestRejection {
                    action: endpoint.action,
                    resource_type: endpoint.resource_type,
                    resource_id: resource_id.as_deref(),
                    outcome: "scope_denied",
                    code: "TOKEN_SCOPE_DENIED",
                    message: "This REST operation is exposed only to full-control API tokens"
                        .to_owned(),
                },
            )
            .await,
        ));
    }

    let endpoint_scope_allows = if endpoint.operation_id == CCTV_CAPABILITIES_OPERATION_ID {
        CCTV_SCOPE_ACTIONS.iter().any(|action| {
            request_context.allows_api_token_scope_for_resource(
                action,
                "cctv",
                resource_id.as_deref(),
            )
        })
    } else {
        request_context.allows_api_token_scope_for_resource(
            endpoint.action,
            endpoint.resource_type,
            resource_id.as_deref(),
        )
    };
    if !endpoint.full_control_only && !endpoint_scope_allows {
        return Err(Box::new(
            reject_scoped_rest_call(
                state,
                audit_auth_context,
                request_context,
                id,
                ScopedRestRejection {
                    action: endpoint.action,
                    resource_type: endpoint.resource_type,
                    resource_id: resource_id.as_deref(),
                    outcome: "resource_denied",
                    code: "TOKEN_SCOPE_DENIED",
                    message: "API token scope does not permit this REST operation resource"
                        .to_owned(),
                },
            )
            .await,
        ));
    }

    Ok(ScopedRestAuthorization {
        endpoint,
        resource_id,
    })
}

pub(in crate::api::mcp) async fn handle_scoped_rest_endpoint_tool_call(
    context: McpToolCallContext,
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
    let authorization = match authorize_scoped_rest_call(
        &state,
        audit_auth_context.as_ref(),
        &request_context,
        id.clone(),
        &arguments,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let endpoint = authorization.endpoint;
    let resource_id = authorization.resource_id;

    if is_connection_session_create_endpoint(&endpoint)
        && !allows_host_interactive_session_for_resource(&request_context, resource_id.as_deref())
    {
        return reject_scoped_rest_call(
            &state,
            audit_auth_context.as_ref(),
            &request_context,
            id,
            ScopedRestRejection {
                action: "secret",
                resource_type: "connection_target",
                resource_id: resource_id.as_deref(),
                outcome: "resource_denied",
                code: "TOKEN_SCOPE_DENIED",
                message: "MCP connection session creation requires connect:connection_target and secret:connection_target for the target because it mints an interactive proxy token".to_owned(),
            },
        )
        .await;
    }

    if mcp_endpoint_rbac_denied(&state, &request_context, &endpoint, resource_id.as_deref()).await {
        return reject_scoped_rest_call(
            &state,
            audit_auth_context.as_ref(),
            &request_context,
            id,
            ScopedRestRejection {
                action: endpoint.action,
                resource_type: endpoint.resource_type,
                resource_id: resource_id.as_deref(),
                outcome: "rbac_denied",
                code: "RBAC_DENIED",
                message: "API token scope permits this REST operation, but RBAC denies the token principal. Assign a direct role or policy to the API token user/service principal.".to_owned(),
            },
        )
        .await;
    }

    let invalid_arguments = |error: String| ScopedRestRejection {
        action: endpoint.action,
        resource_type: endpoint.resource_type,
        resource_id: resource_id.as_deref(),
        outcome: "invalid_arguments",
        code: "INVALID_ARGUMENTS",
        message: error,
    };
    let uri = match build_rest_endpoint_uri(&endpoint, &arguments) {
        Ok(value) => value,
        Err(error) => {
            return reject_scoped_rest_call(
                &state,
                audit_auth_context.as_ref(),
                &request_context,
                id,
                invalid_arguments(error),
            )
            .await;
        }
    };
    let body = match request_body(&arguments) {
        Ok(value) => value,
        Err(error) => {
            return reject_scoped_rest_call(
                &state,
                audit_auth_context.as_ref(),
                &request_context,
                id,
                invalid_arguments(error),
            )
            .await;
        }
    };

    let dispatch_auth_context = auth_context.map(|context| {
        rest_endpoint_dispatch_auth_context(context, &endpoint, resource_id.as_deref())
    });
    match dispatch_rest_request(
        RestDispatchContext {
            state: state.clone(),
            request_id,
            auth_context: dispatch_auth_context,
            headers,
        },
        endpoint.method.clone(),
        uri,
        body,
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
                        tool_name: SCOPED_REST_CALL_ENDPOINT_TOOL_NAME,
                        action: endpoint.action,
                        resource_type: endpoint.resource_type,
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
            reject_scoped_rest_call(
                &state,
                audit_auth_context.as_ref(),
                &request_context,
                id,
                ScopedRestRejection {
                    action: endpoint.action,
                    resource_type: endpoint.resource_type,
                    resource_id: resource_id.as_deref(),
                    outcome: "dispatch_failed",
                    code: "REST_DISPATCH_FAILED",
                    message: error,
                },
            )
            .await
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::api::mcp::test_support::*;

    #[test]
    fn generic_rest_connection_session_create_is_classified_as_interactive() {
        assert!(generic_rest_is_connection_session_create(
            &Method::POST,
            "/api/v1/connections/sessions"
        ));
        assert!(generic_rest_is_connection_session_create(
            &Method::POST,
            "/api/v1/connections/sessions?x=1"
        ));
        assert!(!generic_rest_is_connection_session_create(
            &Method::GET,
            "/api/v1/connections/sessions"
        ));
    }

    #[test]
    fn scoped_rest_endpoint_tools_are_visible_for_entity_scoped_tokens() {
        let request_context = context("read", "inventory", "inventory_entity:devices");
        let tool_names = visible_tool_names(&request_context);

        assert!(tool_names.contains(&SCOPED_REST_LIST_ENDPOINTS_TOOL_NAME.to_string()));
        assert!(tool_names.contains(&SCOPED_REST_CALL_ENDPOINT_TOOL_NAME.to_string()));
        assert!(!tool_names.contains(&GENERIC_REST_TOOL_NAME.to_string()));

        let call_tool = visible_tools(&request_context)
            .into_iter()
            .find(|tool| {
                tool.get("name").and_then(serde_json::Value::as_str)
                    == Some(SCOPED_REST_CALL_ENDPOINT_TOOL_NAME)
            })
            .expect("scoped REST call tool");
        let operations = call_tool
            .pointer("/inputSchema/properties/operation_id/enum")
            .and_then(serde_json::Value::as_array)
            .expect("operation enum");
        let operation_ids = operations
            .iter()
            .filter_map(serde_json::Value::as_str)
            .collect::<Vec<_>>();

        assert!(operation_ids.contains(&"inventory.devices.list"));
        assert!(operation_ids.contains(&"inventory.devices.get"));
        assert!(!operation_ids.contains(&"inventory.sites.list"));
    }

    #[test]
    fn scoped_rest_endpoint_catalog_filters_by_token_scope() {
        let request_context = context("read", "inventory", "inventory_entity:devices");
        let operations = visible_scoped_rest_endpoints(&request_context)
            .into_iter()
            .map(|endpoint| endpoint.operation_id)
            .collect::<Vec<_>>();

        assert!(operations.contains(&"inventory.devices.list".to_string()));
        assert!(operations.contains(&"inventory.devices.get".to_string()));
        assert!(!operations.contains(&"inventory.devices.create".to_string()));
        assert!(!operations.contains(&"inventory.sites.list".to_string()));
    }

    #[test]
    fn scoped_rest_bulk_mutate_catalog_respects_resolved_action_scope() {
        let update_context = context("update", "inventory", "inventory_entity:devices");
        let update_operations = visible_scoped_rest_endpoints(&update_context)
            .into_iter()
            .map(|endpoint| endpoint.operation_id)
            .collect::<Vec<_>>();

        assert!(update_operations.contains(&"inventory.devices.bulk_mutate_update".to_string()));
        assert!(!update_operations.contains(&"inventory.devices.bulk_mutate_delete".to_string()));
        assert!(visible_tools(&update_context).iter().any(|tool| {
            tool.get("name").and_then(|value| value.as_str())
                == Some(INVENTORY_BULK_MUTATE_TOOL_NAME)
        }));

        let delete_context = context("delete", "inventory", "inventory_entity:clusters");
        let delete_operations = visible_scoped_rest_endpoints(&delete_context)
            .into_iter()
            .map(|endpoint| endpoint.operation_id)
            .collect::<Vec<_>>();

        assert!(delete_operations.contains(&"inventory.clusters.bulk_mutate_delete".to_string()));
        assert!(!delete_operations.contains(&"inventory.clusters.bulk_mutate_update".to_string()));
        assert!(visible_tools(&delete_context).iter().any(|tool| {
            tool.get("name").and_then(|value| value.as_str())
                == Some(INVENTORY_BULK_MUTATE_TOOL_NAME)
        }));

        let endpoint = mcp_rest_endpoint_for_operation_id("inventory.devices.bulk_mutate_update")
            .expect("devices bulk mutate update endpoint");
        assert_eq!(endpoint.method, Method::POST);
        assert_eq!(
            endpoint.path_template,
            "/api/v1/inventory/resources/devices/bulk"
        );
        assert_eq!(endpoint.action, "update");
        assert_eq!(
            endpoint.static_resource_id.as_deref(),
            Some("inventory_entity:devices")
        );
    }

    #[test]
    fn scoped_rest_endpoint_dispatch_uses_static_resource_override() {
        let endpoint = mcp_rest_endpoint_for_operation_id("inventory.devices.list")
            .expect("inventory devices list endpoint");
        assert_eq!(
            resource_id_for_endpoint(&endpoint, &json!({})),
            Ok(Some("inventory_entity:devices".to_string()))
        );

        let auth_context = AuthContext {
            user_id: Some(uuid::Uuid::nil()),
            auth_method: "api_token".to_string(),
            oidc_groups: Vec::new(),
            api_token_id: Some("token".to_string()),
            api_token_scopes: vec![ApiTokenScopeContext {
                action: "read".to_string(),
                resource_type: "inventory".to_string(),
                resource_pattern: "inventory_entity:devices".to_string(),
            }],
            api_token_scope_resource_override: None,
        };
        let dispatched = rest_endpoint_dispatch_auth_context(
            auth_context,
            &endpoint,
            Some("inventory_entity:devices"),
        );

        assert!(dispatched.api_token_scopes.iter().all(|scope| {
            !(scope.action == "read"
                && scope.resource_type == "inventory"
                && scope.resource_pattern == "*")
        }));
        assert_eq!(
            dispatched
                .api_token_scope_resource_override
                .as_ref()
                .map(|value| value.resource_id.as_str()),
            Some("inventory_entity:devices")
        );
    }

    #[test]
    fn scoped_rest_endpoint_requires_path_param_for_targeted_resources() {
        let endpoint = mcp_rest_endpoint_for_operation_id("access_control.users.get")
            .expect("access-control user get endpoint");

        assert_eq!(
            resource_id_for_endpoint(
                &endpoint,
                &json!({
                    "path_params": {
                        "id": "user-1"
                    }
                })
            ),
            Ok(Some("user:user-1".to_string()))
        );
        assert!(resource_id_for_endpoint(&endpoint, &json!({})).is_err());
    }

    #[test]
    fn generic_rest_uri_rejects_internal_or_ambiguous_paths() {
        assert!(
            generic_rest_uri(&json!({
                "method": "GET",
                "path": "/api/v1/inventory/devices",
                "query": {
                    "limit": 10
                }
            }))
            .is_ok()
        );
        assert!(
            generic_rest_uri(&json!({
                "method": "GET",
                "path": "/api/v1/mcp"
            }))
            .is_err()
        );
        assert!(
            generic_rest_uri(&json!({
                "method": "GET",
                "path": "/api/v1/auth/login"
            }))
            .is_err()
        );
        assert!(
            generic_rest_uri(&json!({
                "method": "POST",
                "path": "/api/v1/webhooks/netbox/provider-1"
            }))
            .is_err()
        );
        assert!(
            generic_rest_uri(&json!({
                "method": "GET",
                "path": "/api/v1/monitoring/sd"
            }))
            .is_err()
        );
        assert!(
            generic_rest_uri(&json!({
                "method": "GET",
                "path": "/api/v1/admin/auth/oidc/settings"
            }))
            .is_ok()
        );
        assert!(
            generic_rest_uri(&json!({
                "method": "GET",
                "path": "/api/v1/monitoring/sd/preview"
            }))
            .is_ok()
        );
        assert!(
            generic_rest_uri(&json!({
                "method": "GET",
                "path": "/api/v1/inventory/devices?limit=10"
            }))
            .is_err()
        );
        assert!(
            generic_rest_uri(&json!({
                "method": "GET",
                "path": "https://example.test/api/v1/inventory/devices"
            }))
            .is_err()
        );
    }
}
