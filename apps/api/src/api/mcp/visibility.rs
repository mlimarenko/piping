//! Entity manifests, API-token scope predicates and tool visibility, split out of `mcp.rs`.
//!
//! Moved verbatim; behaviour is unchanged.

use super::*;

pub(super) fn allowed_inventory_entity_routes(
    request_context: &RequestContext,
) -> Vec<&'static InventoryEntityRoute> {
    INVENTORY_ENTITY_ROUTES
        .iter()
        .filter(|route| {
            [
                InventoryToolOperation::List,
                InventoryToolOperation::Get,
                InventoryToolOperation::Create,
                InventoryToolOperation::Update,
                InventoryToolOperation::Delete,
                InventoryToolOperation::BulkCreate,
                InventoryToolOperation::BulkMutate,
            ]
            .into_iter()
            .any(|operation| inventory_operation_visible(request_context, operation, route))
        })
        .collect()
}

pub(super) fn allowed_access_control_entity_routes(
    request_context: &RequestContext,
) -> Vec<&'static AccessControlEntityRoute> {
    ACCESS_CONTROL_ENTITY_ROUTES
        .iter()
        .filter(|route| {
            [
                AccessControlToolOperation::List,
                AccessControlToolOperation::Get,
                AccessControlToolOperation::Create,
                AccessControlToolOperation::Update,
                AccessControlToolOperation::Delete,
                AccessControlToolOperation::Bulk,
            ]
            .into_iter()
            .any(|operation| access_control_route_visible(request_context, operation, route))
        })
        .collect()
}

pub(super) fn inventory_entity_manifest(
    request_context: &RequestContext,
    route: &InventoryEntityRoute,
) -> Value {
    let resource_id = inventory_entity_resource_id(route);
    let operations = [
        InventoryToolOperation::List,
        InventoryToolOperation::Get,
        InventoryToolOperation::Create,
        InventoryToolOperation::Update,
        InventoryToolOperation::Delete,
        InventoryToolOperation::BulkCreate,
        InventoryToolOperation::BulkMutate,
    ]
    .into_iter()
    .filter(|operation| inventory_operation_visible(request_context, *operation, route))
    .map(|operation| {
        let actions = inventory_operation_actions(operation, route);
        let action = actions
            .first()
            .copied()
            .unwrap_or_else(|| operation.action());
        json!({
            "operation": operation.name(),
            "action": action,
            "actions": actions,
            "method": operation.method().as_str(),
            "requires_id": operation.requires_id(),
            "requires_body": operation.requires_body()
        })
    })
    .collect::<Vec<_>>();

    json!({
        "entity_type": route.entity_type,
        "path_segment": route.path_segment,
        "resource_id": resource_id,
        "supports_bulk_create": route.supports_bulk_create,
        "supports_bulk_mutate": !bulk_mutation_operations_for_route(route).is_empty(),
        "bulk_mutation_operations": bulk_mutation_operations_for_route(route),
        "notes": inventory_entity_notes(route),
        "operations": operations
    })
}

pub(super) fn inventory_operation_visible(
    request_context: &RequestContext,
    operation: InventoryToolOperation,
    route: &InventoryEntityRoute,
) -> bool {
    if !operation.supports_route(route) {
        return false;
    }
    let resource_id = inventory_entity_resource_id(route);
    inventory_operation_actions(operation, route)
        .into_iter()
        .any(|action| {
            request_context.allows_api_token_scope_for_resource(
                action,
                "inventory",
                Some(resource_id.as_str()),
            )
        })
}

pub(super) fn inventory_operation_actions(
    operation: InventoryToolOperation,
    route: &InventoryEntityRoute,
) -> Vec<&'static str> {
    if operation == InventoryToolOperation::BulkMutate {
        let supported_actions = bulk_mutation_operations_for_route(route)
            .iter()
            .filter_map(|operation| bulk_mutation_action_for_operation(operation))
            .collect::<HashSet<_>>();
        return ["delete", "update"]
            .into_iter()
            .filter(|action| supported_actions.contains(action))
            .collect();
    }
    vec![operation.action()]
}

pub(super) fn inventory_entity_notes(route: &InventoryEntityRoute) -> Option<&'static str> {
    match route.entity_type {
        "devices" => Some(
            "Devices are one backing source for /api/v1/connections/targets. Use connection targets for read-only session-launch and discovery reads; mutate devices here.",
        ),
        "virtual_machines" => Some(
            "Virtual machines are one backing source for /api/v1/connections/targets. Use connection targets for read-only session-launch and discovery reads; mutate virtual_machines here.",
        ),
        _ => None,
    }
}

pub(super) fn access_control_entity_manifest(
    request_context: &RequestContext,
    route: &AccessControlEntityRoute,
) -> Value {
    let operations = [
        AccessControlToolOperation::List,
        AccessControlToolOperation::Get,
        AccessControlToolOperation::Create,
        AccessControlToolOperation::Update,
        AccessControlToolOperation::Delete,
        AccessControlToolOperation::Bulk,
    ]
    .into_iter()
    .filter(|operation| access_control_route_visible(request_context, *operation, route))
    .map(|operation| {
        json!({
            "operation": operation.name(),
            "action": operation.action().unwrap_or("bulk"),
            "method": operation.method().as_str(),
            "requires_id": operation.requires_id(),
            "requires_body": operation.requires_body()
        })
    })
    .collect::<Vec<_>>();

    json!({
        "entity_type": route.entity_type,
        "path_segment": route.path_segment,
        "resource_type": route.resource_type,
        "resource_prefix": route.resource_prefix,
        "operations": operations
    })
}

pub(super) fn has_host_ssh_scope(request_context: &RequestContext) -> bool {
    api_token_scope_patterns_intersect(
        request_context,
        "execute",
        "connection_target",
        "connect",
        "connection_target",
    )
}

pub(super) fn has_host_session_client_credentials_scope(request_context: &RequestContext) -> bool {
    api_token_scope_patterns_intersect(
        request_context,
        "connect",
        "connection_target",
        "secret",
        "connection_target",
    )
}

pub(super) fn has_host_interactive_session_scope(request_context: &RequestContext) -> bool {
    has_host_session_client_credentials_scope(request_context)
}

pub(super) fn allows_host_interactive_session_for_resource(
    request_context: &RequestContext,
    resource_id: Option<&str>,
) -> bool {
    request_context.allows_api_token_scope_for_resource("connect", "connection_target", resource_id)
        && request_context.allows_api_token_scope_for_resource(
            "secret",
            "connection_target",
            resource_id,
        )
}

pub(super) fn has_host_visual_scope(request_context: &RequestContext) -> bool {
    has_host_session_client_credentials_scope(request_context)
}

pub(super) fn has_host_visual_observe_scope(request_context: &RequestContext) -> bool {
    has_host_session_client_credentials_scope(request_context)
        && request_context.has_api_token_scope("observe", "connection_target")
}

pub(super) fn has_host_visual_control_scope(request_context: &RequestContext) -> bool {
    has_host_session_client_credentials_scope(request_context)
        && api_token_scope_patterns_intersect(
            request_context,
            "execute",
            "connection_target",
            "connect",
            "connection_target",
        )
        && api_token_scope_patterns_intersect(
            request_context,
            "execute",
            "connection_target",
            "secret",
            "connection_target",
        )
}

pub(super) fn is_host_vnc_control_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        HOST_VNC_POINTER_TOOL_NAME | HOST_VNC_KEY_TOOL_NAME | HOST_VNC_TEXT_TOOL_NAME
    )
}

pub(super) fn allows_host_visual_control_for_resource(
    request_context: &RequestContext,
    resource_id: Option<&str>,
) -> bool {
    allows_host_interactive_session_for_resource(request_context, resource_id)
        && request_context.allows_api_token_scope_for_resource(
            "execute",
            "connection_target",
            resource_id,
        )
}

pub(super) fn api_token_scope_patterns_intersect(
    request_context: &RequestContext,
    left_action: &str,
    left_resource_type: &str,
    right_action: &str,
    right_resource_type: &str,
) -> bool {
    request_context
        .api_token_scopes
        .iter()
        .filter(|scope| scope.action == left_action && scope.resource_type == left_resource_type)
        .any(|left| {
            request_context
                .api_token_scopes
                .iter()
                .filter(|scope| {
                    scope.action == right_action && scope.resource_type == right_resource_type
                })
                .any(|right| {
                    resource_patterns_have_known_intersection(
                        left.resource_pattern.as_str(),
                        right.resource_pattern.as_str(),
                    )
                })
        })
}

pub(super) fn resource_patterns_have_known_intersection(left: &str, right: &str) -> bool {
    let left = left.trim();
    let right = right.trim();
    if left.is_empty() || right.is_empty() {
        return false;
    }
    if left == "*" || right == "*" || left == right {
        return true;
    }
    crate::contexts::access_control::domain::matches_resource_pattern(left, right)
        || crate::contexts::access_control::domain::matches_resource_pattern(right, left)
}

pub(super) fn tool_visible(request_context: &RequestContext, tool: &McpRestTool) -> bool {
    if is_connection_session_create_tool(tool) {
        return has_host_interactive_session_scope(request_context);
    }
    match tool.resource_id_source {
        ResourceIdSource::None => {
            request_context.allows_api_token_scope(tool.action, tool.resource_type)
        }
        ResourceIdSource::PathParam { .. }
        | ResourceIdSource::QueryParam { .. }
        | ResourceIdSource::BodyPointer { .. } => {
            request_context.has_api_token_scope(tool.action, tool.resource_type)
        }
    }
}

pub(super) fn has_full_control_plane_scope(request_context: &RequestContext) -> bool {
    user_api_token_scope_catalog_pairs()
        .iter()
        .all(|(action, resource_type)| {
            request_context.allows_api_token_scope(action_key(action), resource_type.as_str())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::mcp::routes_table::{ACCESS_CONTROL_ENTITY_ROUTES, INVENTORY_ENTITY_ROUTES};
    use crate::api::mcp::test_support::*;

    fn inventory_route(entity_type: &str) -> &'static InventoryEntityRoute {
        INVENTORY_ENTITY_ROUTES
            .iter()
            .find(|route| route.entity_type == entity_type)
            .expect("route present")
    }

    #[test]
    fn inventory_entity_notes_document_devices_and_virtual_machines() {
        assert!(
            inventory_entity_notes(inventory_route("devices"))
                .expect("devices note")
                .contains("Devices are one backing source")
        );
        assert!(
            inventory_entity_notes(inventory_route("virtual_machines"))
                .expect("virtual machine note")
                .contains("Virtual machines are one backing source")
        );
        assert_eq!(
            inventory_entity_notes(inventory_route("sites")),
            None,
            "sites carry no extra note"
        );
    }

    #[test]
    fn access_control_entity_manifest_lists_visible_operations() {
        let context = full_context();
        let route = ACCESS_CONTROL_ENTITY_ROUTES.first().expect("routes");
        let manifest = access_control_entity_manifest(&context, route);
        assert_eq!(manifest["entity_type"], json!(route.entity_type));
        assert_eq!(manifest["resource_type"], json!(route.resource_type));
        assert_eq!(manifest["resource_prefix"], json!(route.resource_prefix));
        let operations = manifest["operations"].as_array().expect("operations");
        assert!(!operations.is_empty());
        assert!(
            operations[0]["operation"]
                .as_str()
                .is_some_and(|name| !name.is_empty())
        );
        assert!(manifest.get("supports_bulk_create").is_none());
    }

    #[test]
    fn resource_pattern_intersection_handles_empty_exact_and_wildcard() {
        assert!(!resource_patterns_have_known_intersection("", "*"));
        assert!(!resource_patterns_have_known_intersection("devices", ""));
        assert!(resource_patterns_have_known_intersection("*", "anything"));
        assert!(resource_patterns_have_known_intersection(
            "devices", "devices"
        ));
        assert!(resource_patterns_have_known_intersection(
            "devices/*",
            "devices/42"
        ));
    }

    #[test]
    fn allowed_entity_routes_follow_token_scope() {
        let context = full_context();
        assert!(!allowed_inventory_entity_routes(&context).is_empty());
        assert!(!allowed_access_control_entity_routes(&context).is_empty());
        let unscoped = context_with_scopes(Vec::new());
        assert!(allowed_inventory_entity_routes(&unscoped).is_empty());
        assert!(allowed_access_control_entity_routes(&unscoped).is_empty());
    }
}
