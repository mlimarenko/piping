//! MCP resource listing and reading, split out of `mcp.rs`.
//!
//! Moved verbatim; behaviour is unchanged.

use super::*;

pub(super) fn visible_mcp_resources(request_context: &RequestContext) -> Vec<Value> {
    visible_mcp_resources_compact(request_context)
}

pub(super) fn visible_mcp_resources_compact(_request_context: &RequestContext) -> Vec<Value> {
    mcp_top_level_resources()
}

pub(super) fn mcp_top_level_resources() -> Vec<Value> {
    vec![
        mcp_resource_descriptor(
            MCP_RESOURCE_CAPABILITIES_URI,
            "mcp-capabilities",
            "MCP capabilities",
            "Token-scoped Piping MCP capabilities, limits, tools, resources, and safety notes.",
        ),
        mcp_resource_descriptor(
            MCP_RESOURCE_TOOLS_URI,
            "mcp-tools",
            "Visible MCP tools",
            "Tool descriptors visible to the current API token.",
        ),
        mcp_resource_descriptor(
            MCP_RESOURCE_SCOPE_CATALOG_URI,
            "api-token-scope-catalog",
            "API token scope catalog",
            "Supported API-token action/resource pairs.",
        ),
    ]
}

pub(super) fn visible_mcp_resources_full(request_context: &RequestContext) -> Vec<Value> {
    let mut resources = mcp_top_level_resources();

    let inventory_routes = allowed_inventory_entity_routes(request_context);
    if !inventory_routes.is_empty() {
        resources.push(mcp_resource_descriptor(
            MCP_RESOURCE_INVENTORY_ENTITIES_URI,
            "inventory-entities",
            "Inventory entity catalog",
            "Inventory entity types and operations visible to the current API token.",
        ));
        resources.extend(inventory_routes.into_iter().map(|route| {
            mcp_resource_descriptor(
                format!(
                    "{MCP_RESOURCE_INVENTORY_ENTITY_PREFIX}{}",
                    route.entity_type
                ),
                format!("inventory-entity-{}", route.entity_type),
                format!("Inventory entity {}", route.entity_type),
                format!(
                    "Operation manifest for the '{}' inventory entity.",
                    route.entity_type
                ),
            )
        }));
    }

    let access_routes = allowed_access_control_entity_routes(request_context);
    if !access_routes.is_empty() {
        resources.push(mcp_resource_descriptor(
            MCP_RESOURCE_ACCESS_CONTROL_ENTITIES_URI,
            "access-control-entities",
            "Access-control entity catalog",
            "Access-control entity types and operations visible to the current API token.",
        ));
        resources.extend(access_routes.into_iter().map(|route| {
            mcp_resource_descriptor(
                format!(
                    "{MCP_RESOURCE_ACCESS_CONTROL_ENTITY_PREFIX}{}",
                    route.entity_type
                ),
                format!("access-control-entity-{}", route.entity_type),
                format!("Access-control entity {}", route.entity_type),
                format!(
                    "Operation manifest for the '{}' access-control entity.",
                    route.entity_type
                ),
            )
        }));
    }

    resources
}

pub(super) fn mcp_resources_list_value(
    request_context: &RequestContext,
    params: &Value,
) -> Result<Value, String> {
    let empty_params = Value::Object(Map::new());
    let params = if params.is_null() {
        &empty_params
    } else {
        params
    };
    let object = params
        .as_object()
        .ok_or_else(|| "resources/list params must be an object".to_string())?;
    for key in object.keys() {
        if !matches!(
            key.as_str(),
            "full" | "prefix" | "category" | "limit" | "offset"
        ) {
            return Err(format!("resources/list param '{key}' is not supported"));
        }
    }
    let full = optional_resource_bool_param(object, "full", false)?;
    let prefix = optional_resource_string_param(object, "prefix")?;
    let category = optional_resource_string_param(object, "category")?;
    let limit = optional_resource_usize_param(object, "limit", 20, 1, 1000)?;
    let offset = optional_resource_usize_param(object, "offset", 0, 0, usize::MAX)?;

    let include_dynamic = full || prefix.is_some() || category.is_some();
    let mut resources = if include_dynamic {
        visible_mcp_resources_full(request_context)
    } else {
        visible_mcp_resources(request_context)
    };
    if let Some(prefix) = prefix {
        resources.retain(|resource| {
            resource
                .get("uri")
                .and_then(Value::as_str)
                .is_some_and(|uri| uri.starts_with(prefix.as_str()))
        });
    }
    if let Some(category) = category {
        resources.retain(|resource| {
            resource
                .get("uri")
                .and_then(Value::as_str)
                .is_some_and(|uri| uri.contains(category.as_str()))
                || resource
                    .get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|name| name.contains(category.as_str()))
        });
    }
    let total = resources.len();
    let page = resources
        .into_iter()
        .skip(offset)
        .take(limit)
        .collect::<Vec<_>>();
    let count = page.len();
    Ok(json!({
        "resources": page,
        "count": count,
        "total": total,
        "limit": limit,
        "offset": offset,
        "full": include_dynamic
    }))
}

pub(super) fn optional_resource_bool_param(
    object: &Map<String, Value>,
    key: &str,
    default: bool,
) -> Result<bool, String> {
    match object.get(key) {
        Some(Value::Bool(value)) => Ok(*value),
        Some(Value::Null) | None => Ok(default),
        Some(_) => Err(format!("resources/list param '{key}' must be a boolean")),
    }
}

pub(super) fn optional_resource_string_param(
    object: &Map<String, Value>,
    key: &str,
) -> Result<Option<String>, String> {
    match object.get(key) {
        Some(Value::String(value)) => {
            Ok(Some(value.trim().to_string()).filter(|value| !value.is_empty()))
        }
        Some(Value::Null) | None => Ok(None),
        Some(_) => Err(format!("resources/list param '{key}' must be a string")),
    }
}

pub(super) fn optional_resource_usize_param(
    object: &Map<String, Value>,
    key: &str,
    default: usize,
    min: usize,
    max: usize,
) -> Result<usize, String> {
    let value = match object.get(key) {
        Some(Value::Number(number)) => number
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| format!("resources/list param '{key}' must be an unsigned integer"))?,
        Some(Value::Null) | None => return Ok(default),
        Some(_) => return Err(format!("resources/list param '{key}' must be an integer")),
    };
    if value < min || value > max {
        return Err(format!(
            "resources/list param '{key}' must be between {min} and {max}"
        ));
    }
    Ok(value)
}

pub(super) fn mcp_resource_descriptor(
    uri: impl Into<String>,
    name: impl Into<String>,
    title: impl Into<String>,
    description: impl Into<String>,
) -> Value {
    json!({
        "uri": uri.into(),
        "name": name.into(),
        "title": title.into(),
        "description": description.into(),
        "mimeType": "application/json"
    })
}

pub(super) async fn read_mcp_resource(
    state: &AppState,
    request_context: &RequestContext,
    params: &Value,
) -> Result<Value, String> {
    let uri = mcp_resource_uri_from_params(params)?;
    if parse_cctv_snapshot_resource_uri(uri).is_some() {
        return read_cctv_snapshot_resource(state, request_context, uri).await;
    }
    if parse_vnc_capture_resource_uri(uri).is_some() {
        return read_vnc_capture_resource(state, request_context, uri).await;
    }
    read_static_mcp_resource(request_context, uri)
}

pub(super) fn mcp_resource_uri_from_params(params: &Value) -> Result<&str, String> {
    params
        .get("uri")
        .or_else(|| params.pointer("/resource/uri"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Missing required resource uri".to_string())
}

pub(super) fn read_static_mcp_resource(
    request_context: &RequestContext,
    uri: &str,
) -> Result<Value, String> {
    let value = match uri {
        MCP_RESOURCE_CAPABILITIES_URI => mcp_capabilities_value(request_context)?,
        MCP_RESOURCE_TOOLS_URI => json!({ "tools": visible_tools(request_context) }),
        MCP_RESOURCE_SCOPE_CATALOG_URI => json!({
            "scopes": user_api_token_scope_catalog_pairs()
                .iter()
                .map(|(action, resource_type)| json!({
                    "action": action_key(action),
                    "resource_type": resource_type.as_str()
                }))
                .collect::<Vec<_>>()
        }),
        MCP_RESOURCE_INVENTORY_ENTITIES_URI => json!({
            "entities": allowed_inventory_entity_routes(request_context)
                .into_iter()
                .map(|route| inventory_entity_manifest(request_context, route))
                .collect::<Vec<_>>()
        }),
        MCP_RESOURCE_ACCESS_CONTROL_ENTITIES_URI => json!({
            "entities": allowed_access_control_entity_routes(request_context)
                .into_iter()
                .map(|route| access_control_entity_manifest(request_context, route))
                .collect::<Vec<_>>()
        }),
        value if value.starts_with(MCP_RESOURCE_INVENTORY_ENTITY_PREFIX) => {
            let entity_type = value
                .strip_prefix(MCP_RESOURCE_INVENTORY_ENTITY_PREFIX)
                .unwrap_or_default();
            let route = inventory_route_for_entity(entity_type)
                .ok_or_else(|| format!("Unknown inventory entity resource '{entity_type}'"))?;
            if !allowed_inventory_entity_routes(request_context)
                .iter()
                .any(|allowed| allowed.entity_type == route.entity_type)
            {
                return Err("API token scope does not expose this inventory entity".to_string());
            }
            inventory_entity_manifest(request_context, route)
        }
        value if value.starts_with(MCP_RESOURCE_ACCESS_CONTROL_ENTITY_PREFIX) => {
            let entity_type = value
                .strip_prefix(MCP_RESOURCE_ACCESS_CONTROL_ENTITY_PREFIX)
                .unwrap_or_default();
            let route = ACCESS_CONTROL_ENTITY_ROUTES
                .iter()
                .find(|route| route.entity_type == entity_type)
                .ok_or_else(|| format!("Unknown access-control entity resource '{entity_type}'"))?;
            if !allowed_access_control_entity_routes(request_context)
                .iter()
                .any(|allowed| allowed.entity_type == route.entity_type)
            {
                return Err(
                    "API token scope does not expose this access-control entity".to_string()
                );
            }
            access_control_entity_manifest(request_context, route)
        }
        _ => return Err(format!("Unknown MCP resource uri '{uri}'")),
    };

    let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
    Ok(json!({
        "contents": [
            {
                "uri": uri,
                "mimeType": "application/json",
                "text": text
            }
        ]
    }))
}

pub(super) async fn read_cctv_snapshot_resource(
    state: &AppState,
    request_context: &RequestContext,
    uri: &str,
) -> Result<Value, String> {
    let (target_id, capture_id) = parse_cctv_snapshot_resource_uri(uri)
        .ok_or_else(|| format!("Unknown MCP resource uri '{uri}'"))?;
    let resource_id = format!("cctv:{target_id}");
    if !request_context.allows_api_token_scope_for_resource(
        "view",
        "cctv",
        Some(resource_id.as_str()),
    ) {
        return Err("API token scope does not permit reading this CCTV snapshot".to_string());
    }
    let target_uuid =
        Uuid::parse_str(target_id).map_err(|_| "Invalid CCTV target id".to_string())?;
    ensure_target_scoped_access(state, request_context, target_uuid, Action::CctvView)
        .await
        .map_err(|_| "Access denied: missing cctv:view for this target".to_string())?;
    let owner_api_token_id = request_context
        .api_token_id
        .as_deref()
        .ok_or_else(|| "CCTV snapshot resources require an API token credential".to_string())?;
    let mut redis = state.redis.clone();
    let blob = crate::storage::cache::mcp_capture::get(&mut redis, capture_id)
        .await
        .map_err(|error| format!("Failed to read CCTV snapshot resource: {error}"))?
        .ok_or_else(|| "CCTV snapshot resource expired or does not exist".to_string())?;
    if blob.target_id != target_id
        || blob.owner_api_token_id != owner_api_token_id
        || blob.session_id != "cctv-snapshot"
    {
        return Err("CCTV snapshot resource ownership mismatch".to_string());
    }
    Ok(json!({
        "contents": [{
            "uri": uri,
            "mimeType": blob.mime_type,
            "blob": blob.image_base64
        }]
    }))
}

pub(super) fn parse_cctv_snapshot_resource_uri(uri: &str) -> Option<(&str, &str)> {
    let suffix = uri.strip_prefix(MCP_RESOURCE_CONNECTION_TARGET_PREFIX)?;
    let mut parts = suffix.split('/');
    let target_id = parts.next()?.trim();
    let marker = parts.next()?;
    let capture_id = parts.next()?.trim();
    if target_id.is_empty()
        || marker != "cctv-snapshot"
        || capture_id.is_empty()
        || parts.next().is_some()
    {
        return None;
    }
    Some((target_id, capture_id))
}

pub(super) async fn read_vnc_capture_resource(
    state: &AppState,
    request_context: &RequestContext,
    uri: &str,
) -> Result<Value, String> {
    let (target_id, capture_id) = parse_vnc_capture_resource_uri(uri)
        .ok_or_else(|| format!("Unknown MCP resource uri '{uri}'"))?;
    let resource_id = format!("connection_target:{target_id}");
    if !request_context.allows_api_token_scope_for_resource(
        "observe",
        "connection_target",
        Some(resource_id.as_str()),
    ) {
        record_mcp_resource_audit(
            state,
            request_context,
            uri,
            Some(resource_id.as_str()),
            "resource_denied",
            Some("TOKEN_SCOPE_DENIED"),
        )
        .await
        .map_err(|_| MCP_AUDIT_RECORD_FAILED_MESSAGE.to_string())?;
        return Err("API token scope does not permit reading this VNC capture".to_string());
    }
    // RBAC precheck: token scope only narrows the caller's policy, it does not
    // grant access. Other MCP families re-check the policy engine via
    // mcp_rest_rbac_denied; this direct resource path must too, or a PAT with an
    // observe scope could read a capture the user's policy denies (OBSERVE on
    // the connection target).
    if mcp_rest_rbac_denied(
        state,
        request_context,
        "observe",
        "connection_target",
        Some(resource_id.as_str()),
    )
    .await
    {
        record_mcp_resource_audit(
            state,
            request_context,
            uri,
            Some(resource_id.as_str()),
            "resource_denied",
            Some("RBAC_DENIED"),
        )
        .await
        .map_err(|_| MCP_AUDIT_RECORD_FAILED_MESSAGE.to_string())?;
        return Err(
            "Access denied: missing observe permission for this connection target".to_string(),
        );
    }
    let owner_api_token_id = request_context
        .api_token_id
        .as_deref()
        .ok_or_else(|| "MCP capture resources require an API token credential".to_string())?;

    let mut redis = state.redis.clone();
    let blob = crate::storage::cache::mcp_capture::get(&mut redis, capture_id)
        .await
        .map_err(|error| format!("Failed to read VNC capture resource: {error}"))?
        .ok_or_else(|| "VNC capture resource expired or does not exist".to_string())?;

    if blob.target_id != target_id || blob.owner_api_token_id != owner_api_token_id {
        record_mcp_resource_audit(
            state,
            request_context,
            uri,
            Some(resource_id.as_str()),
            "owner_denied",
            Some("CAPTURE_OWNER_MISMATCH"),
        )
        .await
        .map_err(|_| MCP_AUDIT_RECORD_FAILED_MESSAGE.to_string())?;
        return Err("VNC capture resource is not owned by this API token".to_string());
    }

    record_mcp_resource_audit(
        state,
        request_context,
        uri,
        Some(resource_id.as_str()),
        "allowed",
        None,
    )
    .await
    .map_err(|_| MCP_AUDIT_RECORD_FAILED_MESSAGE.to_string())?;

    Ok(json!({
        "contents": [
            {
                "uri": uri,
                "mimeType": blob.mime_type,
                "blob": blob.image_base64
            }
        ]
    }))
}

pub(super) fn parse_vnc_capture_resource_uri(uri: &str) -> Option<(&str, &str)> {
    let tail = uri.strip_prefix(MCP_RESOURCE_CONNECTION_TARGET_PREFIX)?;
    let mut parts = tail.split('/');
    let target_id = parts.next()?.trim();
    let marker = parts.next()?.trim();
    let capture_id = parts.next()?.trim();
    if target_id.is_empty()
        || marker != "capture"
        || capture_id.is_empty()
        || parts.next().is_some()
    {
        return None;
    }
    Some((target_id, capture_id))
}

pub(super) async fn record_mcp_resource_audit(
    state: &AppState,
    request_context: &RequestContext,
    uri: &str,
    resource_id: Option<&str>,
    decision: &str,
    error_code: Option<&str>,
) -> Result<(), crate::errors::AppError> {
    let actor_id = request_context.user_id;
    let target_id = request_context
        .api_token_id
        .as_deref()
        .and_then(parse_uuid_or_none);
    let connection_target_id =
        resource_id.and_then(|value| value.strip_prefix("connection_target:"));

    let event = AuditEvent {
        id: Uuid::now_v7(),
        event_type: "mcp.resource.read".to_string(),
        actor_id,
        actor_kind: "mcp_agent".to_string(),
        target_type: Some("user_api_token".to_string()),
        target_id,
        metadata: json!({
            "token_id": request_context.api_token_id,
            "resource_uri": uri,
            "action": "observe",
            "resource_type": "connection_target",
            "resource_id": resource_id,
            "connection_target_id": connection_target_id,
            "protocol": "vnc",
            "risk": "read",
            "decision": decision,
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
                "mcp.resource.audit_record.failed",
                "mcp_audit_record_error",
                error.to_string(),
                EventFields::default()
                    .with_operation("mcp.resource.record_audit")
                    .with_resource("mcp_resource", uri)
                    .with_status(decision)
                    .with_message("Failed to record MCP resource audit event"),
            );
        })
}
