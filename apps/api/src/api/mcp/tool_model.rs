//! MCP REST tool and endpoint model, split out of `mcp.rs`.
//!
//! Moved verbatim; behaviour is unchanged.

use super::*;

#[derive(Debug, Clone, Copy)]
pub(super) enum ResourceIdSource {
    None,
    PathParam {
        key: &'static str,
        prefix: &'static str,
    },
    QueryParam {
        key: &'static str,
        prefix: &'static str,
    },
    BodyPointer {
        pointer: &'static str,
        prefix: &'static str,
    },
}

pub(super) struct McpRestTool {
    pub(super) name: &'static str,
    pub(super) title: &'static str,
    pub(super) description: &'static str,
    pub(super) method: Method,
    pub(super) path_template: &'static str,
    pub(super) action: &'static str,
    pub(super) resource_type: &'static str,
    pub(super) resource_id_source: ResourceIdSource,
    pub(super) input_schema: fn() -> Value,
}
#[derive(Debug, Clone, Copy)]
pub(super) struct StaticMcpRestEndpoint {
    pub(super) operation_id: &'static str,
    pub(super) title: &'static str,
    pub(super) description: &'static str,
    pub(super) method: &'static str,
    pub(super) path_template: &'static str,
    pub(super) action: &'static str,
    pub(super) resource_type: &'static str,
    pub(super) resource_id_source: ResourceIdSource,
}

#[derive(Debug, Clone)]
pub(super) struct McpRestEndpoint {
    pub(super) operation_id: String,
    pub(super) title: String,
    pub(super) description: String,
    pub(super) method: Method,
    pub(super) path_template: String,
    pub(super) action: &'static str,
    pub(super) resource_type: &'static str,
    pub(super) resource_id_source: ResourceIdSource,
    pub(super) static_resource_id: Option<String>,
    pub(super) full_control_only: bool,
}

pub(super) static OPENAPI_REST_FALLBACK_ENDPOINTS: Lazy<Vec<McpRestEndpoint>> =
    Lazy::new(openapi_rest_fallback_endpoints);

#[derive(Clone, Copy)]
pub(super) struct InventoryEntityRoute {
    pub(super) entity_type: &'static str,
    pub(super) path_segment: &'static str,
    pub(super) supports_bulk_create: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum InventoryToolOperation {
    List,
    Get,
    Create,
    Update,
    Delete,
    BulkCreate,
    BulkMutate,
}

impl InventoryToolOperation {
    pub(super) fn from_name(name: &str) -> Option<Self> {
        match name {
            INVENTORY_LIST_TOOL_NAME => Some(Self::List),
            INVENTORY_GET_TOOL_NAME => Some(Self::Get),
            INVENTORY_CREATE_TOOL_NAME => Some(Self::Create),
            INVENTORY_UPDATE_TOOL_NAME => Some(Self::Update),
            INVENTORY_DELETE_TOOL_NAME => Some(Self::Delete),
            INVENTORY_BULK_CREATE_TOOL_NAME => Some(Self::BulkCreate),
            INVENTORY_BULK_MUTATE_TOOL_NAME => Some(Self::BulkMutate),
            _ => None,
        }
    }

    pub(super) fn name(self) -> &'static str {
        match self {
            Self::List => INVENTORY_LIST_TOOL_NAME,
            Self::Get => INVENTORY_GET_TOOL_NAME,
            Self::Create => INVENTORY_CREATE_TOOL_NAME,
            Self::Update => INVENTORY_UPDATE_TOOL_NAME,
            Self::Delete => INVENTORY_DELETE_TOOL_NAME,
            Self::BulkCreate => INVENTORY_BULK_CREATE_TOOL_NAME,
            Self::BulkMutate => INVENTORY_BULK_MUTATE_TOOL_NAME,
        }
    }

    pub(super) fn title(self) -> &'static str {
        match self {
            Self::List => "List inventory resources",
            Self::Get => "Get inventory resource",
            Self::Create => "Create inventory resource",
            Self::Update => "Update inventory resource",
            Self::Delete => "Delete inventory resource",
            Self::BulkCreate => "Bulk create inventory resources",
            Self::BulkMutate => "Bulk mutate inventory resources",
        }
    }

    pub(super) fn description(self) -> &'static str {
        match self {
            Self::List => {
                "List resources for one allowlisted inventory entity type through the existing REST API."
            }
            Self::Get => {
                "Read one resource for one allowlisted inventory entity type through the existing REST API."
            }
            Self::Create => {
                "Create one resource for one allowlisted inventory entity type through the existing REST API."
            }
            Self::Update => {
                "Update one resource for one allowlisted inventory entity type through the existing REST API."
            }
            Self::Delete => {
                "Delete one resource for one allowlisted inventory entity type through the existing REST API."
            }
            Self::BulkCreate => {
                "Bulk create resources for one allowlisted inventory entity type through the existing REST API."
            }
            Self::BulkMutate => {
                "Apply a supported non-atomic bulk cleanup operation to one allowlisted inventory entity type. Supported operations are entity-specific: delete, set_status, assign_tenant, set_role, set_site, set_cluster, and set_device_type."
            }
        }
    }

    pub(super) fn action(self) -> &'static str {
        match self {
            Self::List | Self::Get => "read",
            Self::Create | Self::BulkCreate => "create",
            Self::Update => "update",
            Self::Delete => "delete",
            Self::BulkMutate => "update",
        }
    }

    pub(super) fn method(self) -> Method {
        match self {
            Self::List | Self::Get => Method::GET,
            Self::Create | Self::BulkCreate | Self::BulkMutate => Method::POST,
            Self::Update => Method::PUT,
            Self::Delete => Method::DELETE,
        }
    }

    pub(super) fn requires_id(self) -> bool {
        matches!(self, Self::Get | Self::Update | Self::Delete)
    }

    pub(super) fn requires_body(self) -> bool {
        matches!(
            self,
            Self::Create | Self::Update | Self::BulkCreate | Self::BulkMutate
        )
    }

    pub(super) fn supports_route(self, route: &InventoryEntityRoute) -> bool {
        match self {
            Self::BulkCreate => route.supports_bulk_create,
            Self::BulkMutate => !bulk_mutation_operations_for_route(route).is_empty(),
            _ => true,
        }
    }
}

pub(super) fn bulk_mutation_operations_for_route(
    route: &InventoryEntityRoute,
) -> &'static [&'static str] {
    match route.entity_type {
        "devices" => &[
            "delete",
            "set_status",
            "assign_tenant",
            "set_role",
            "set_site",
            "set_device_type",
        ],
        "virtual_machines" => &[
            "delete",
            "set_status",
            "assign_tenant",
            "set_role",
            "set_site",
            "set_cluster",
        ],
        "sites" => &["delete", "set_status", "assign_tenant"],
        "tenant_groups" | "tenants" | "regions" | "site_groups" | "cluster_types"
        | "cluster_groups" | "clusters" | "manufacturers" | "device_types" | "device_roles" => {
            &["delete"]
        }
        _ => &[],
    }
}

pub(super) fn bulk_mutation_action_for_operation(operation: &str) -> Option<&'static str> {
    match operation {
        "delete" => Some("delete"),
        "set_status" | "assign_tenant" | "set_role" | "set_site" | "set_cluster"
        | "set_device_type" => Some("update"),
        _ => None,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum AccessControlToolOperation {
    List,
    Get,
    Create,
    Update,
    Delete,
    Bulk,
}

impl AccessControlToolOperation {
    pub(super) fn from_name(name: &str) -> Option<Self> {
        match name {
            ACCESS_CONTROL_LIST_TOOL_NAME => Some(Self::List),
            ACCESS_CONTROL_GET_TOOL_NAME => Some(Self::Get),
            ACCESS_CONTROL_CREATE_TOOL_NAME => Some(Self::Create),
            ACCESS_CONTROL_UPDATE_TOOL_NAME => Some(Self::Update),
            ACCESS_CONTROL_DELETE_TOOL_NAME => Some(Self::Delete),
            ACCESS_CONTROL_BULK_TOOL_NAME => Some(Self::Bulk),
            _ => None,
        }
    }

    pub(super) fn name(self) -> &'static str {
        match self {
            Self::List => ACCESS_CONTROL_LIST_TOOL_NAME,
            Self::Get => ACCESS_CONTROL_GET_TOOL_NAME,
            Self::Create => ACCESS_CONTROL_CREATE_TOOL_NAME,
            Self::Update => ACCESS_CONTROL_UPDATE_TOOL_NAME,
            Self::Delete => ACCESS_CONTROL_DELETE_TOOL_NAME,
            Self::Bulk => ACCESS_CONTROL_BULK_TOOL_NAME,
        }
    }

    pub(super) fn title(self) -> &'static str {
        match self {
            Self::List => "List access-control resources",
            Self::Get => "Get access-control resource",
            Self::Create => "Create access-control resource",
            Self::Update => "Update access-control resource",
            Self::Delete => "Delete access-control resource",
            Self::Bulk => "Bulk mutate access-control resources",
        }
    }

    pub(super) fn description(self) -> &'static str {
        match self {
            Self::List => {
                "List users, groups, roles, or policies through the access-control REST API."
            }
            Self::Get => {
                "Read one user, group, role, or policy by id through the access-control REST API."
            }
            Self::Create => {
                "Create a user, group, role, or policy through the access-control REST API."
            }
            Self::Update => {
                "Update one user, group, role, or policy by id through the access-control REST API."
            }
            Self::Delete => {
                "Delete one user, group, role, or policy by id through the access-control REST API."
            }
            Self::Bulk => {
                "Apply the existing bulk create/update/delete access-control REST endpoint for users, groups, roles, or policies. The token needs wildcard scope for every non-empty mutation kind in the body."
            }
        }
    }

    pub(super) fn action(self) -> Option<&'static str> {
        match self {
            Self::List | Self::Get => Some("read"),
            Self::Create => Some("create"),
            Self::Update => Some("update"),
            Self::Delete => Some("delete"),
            Self::Bulk => None,
        }
    }

    pub(super) fn method(self) -> Method {
        match self {
            Self::List | Self::Get => Method::GET,
            Self::Create | Self::Bulk => Method::POST,
            Self::Update => Method::PUT,
            Self::Delete => Method::DELETE,
        }
    }

    pub(super) fn requires_id(self) -> bool {
        matches!(self, Self::Get | Self::Update | Self::Delete)
    }

    pub(super) fn requires_body(self) -> bool {
        matches!(self, Self::Create | Self::Update | Self::Bulk)
    }

    pub(super) fn broad_scope_required(self) -> bool {
        matches!(self, Self::List | Self::Create | Self::Bulk)
    }
}
