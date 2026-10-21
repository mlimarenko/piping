use super::*;

#[derive(Clone, Copy)]
pub(super) struct AccessControlEntityRoute {
    pub(super) entity_type: &'static str,
    pub(super) path_segment: &'static str,
    pub(super) resource_type: &'static str,
    pub(super) resource_prefix: &'static str,
}

macro_rules! inventory_entity_route {
    ($path_segment:literal) => {
        InventoryEntityRoute {
            entity_type: $path_segment,
            path_segment: $path_segment,
            supports_bulk_create: true,
        }
    };
    ($path_segment:literal, bulk = false) => {
        InventoryEntityRoute {
            entity_type: $path_segment,
            path_segment: $path_segment,
            supports_bulk_create: false,
        }
    };
}

pub(super) const INVENTORY_ENTITY_ROUTES: &[InventoryEntityRoute] = &[
    inventory_entity_route!("aggregates"),
    inventory_entity_route!("asn-ranges"),
    inventory_entity_route!("asns"),
    inventory_entity_route!("cables"),
    inventory_entity_route!("circuit-groups"),
    inventory_entity_route!("circuit-terminations"),
    inventory_entity_route!("circuit-types"),
    inventory_entity_route!("circuits"),
    inventory_entity_route!("cluster_groups"),
    inventory_entity_route!("cluster_types"),
    inventory_entity_route!("clusters"),
    inventory_entity_route!("console-ports"),
    inventory_entity_route!("console-server-ports"),
    inventory_entity_route!("contact-assignments"),
    inventory_entity_route!("contact-groups"),
    inventory_entity_route!("contact-roles"),
    inventory_entity_route!("contacts"),
    inventory_entity_route!("device-bays"),
    inventory_entity_route!("device_roles"),
    inventory_entity_route!("device_types"),
    inventory_entity_route!("devices"),
    inventory_entity_route!("dataflows", bulk = false),
    inventory_entity_route!("front-ports"),
    inventory_entity_route!("ike-policies"),
    inventory_entity_route!("ike-proposals"),
    inventory_entity_route!("interfaces"),
    inventory_entity_route!("inventory-item-roles"),
    inventory_entity_route!("inventory-items"),
    inventory_entity_route!("ip-addresses", bulk = false),
    inventory_entity_route!("ip_ranges"),
    inventory_entity_route!("ipam_roles"),
    inventory_entity_route!("ipsec-policies"),
    inventory_entity_route!("ipsec-profiles"),
    inventory_entity_route!("ipsec-proposals"),
    inventory_entity_route!("locations"),
    inventory_entity_route!("mac-addresses"),
    inventory_entity_route!("manufacturers"),
    inventory_entity_route!("module-bays"),
    inventory_entity_route!("module-types"),
    inventory_entity_route!("modules"),
    inventory_entity_route!("platforms"),
    inventory_entity_route!("power-feeds"),
    inventory_entity_route!("power-outlets"),
    inventory_entity_route!("power-panels"),
    inventory_entity_route!("power-ports"),
    inventory_entity_route!("prefixes"),
    inventory_entity_route!("provider-accounts"),
    inventory_entity_route!("provider-networks"),
    inventory_entity_route!("providers"),
    inventory_entity_route!("rack-reservations"),
    inventory_entity_route!("rack_roles"),
    inventory_entity_route!("rack_types"),
    inventory_entity_route!("racks"),
    inventory_entity_route!("rear-ports"),
    inventory_entity_route!("regions"),
    inventory_entity_route!("rirs"),
    inventory_entity_route!("route_targets"),
    inventory_entity_route!("service-templates"),
    inventory_entity_route!("services"),
    inventory_entity_route!("site_groups"),
    inventory_entity_route!("sites"),
    inventory_entity_route!("tenant_groups"),
    inventory_entity_route!("tenants"),
    inventory_entity_route!("tunnel-groups"),
    inventory_entity_route!("tunnel-terminations"),
    inventory_entity_route!("tunnels"),
    inventory_entity_route!("virtual-chassis"),
    inventory_entity_route!("virtual-circuit-types"),
    inventory_entity_route!("virtual-circuits"),
    inventory_entity_route!("virtual-device-contexts"),
    inventory_entity_route!("virtual-disks"),
    inventory_entity_route!("virtual-machine-types"),
    inventory_entity_route!("virtual_machines"),
    inventory_entity_route!("vlan_groups"),
    inventory_entity_route!("vlans"),
    inventory_entity_route!("vm-interfaces"),
    inventory_entity_route!("vrfs"),
];

pub(super) const ACCESS_CONTROL_ENTITY_ROUTES: &[AccessControlEntityRoute] = &[
    AccessControlEntityRoute {
        entity_type: "users",
        path_segment: "users",
        resource_type: "user",
        resource_prefix: "user:",
    },
    AccessControlEntityRoute {
        entity_type: "groups",
        path_segment: "groups",
        resource_type: "group",
        resource_prefix: "group:",
    },
    AccessControlEntityRoute {
        entity_type: "roles",
        path_segment: "roles",
        resource_type: "role",
        resource_prefix: "role:",
    },
    AccessControlEntityRoute {
        entity_type: "policies",
        path_segment: "policies",
        resource_type: "policy",
        resource_prefix: "policy:",
    },
];

pub(super) const STATIC_MCP_REST_ENDPOINTS: &[StaticMcpRestEndpoint] = &[
    StaticMcpRestEndpoint {
        operation_id: "automation_secrets.list",
        title: "List automation secrets",
        description: "List named automation secrets (id, key, description, whether a value is set, and usage count). Never returns secret values.",
        method: "GET",
        path_template: "/api/v1/automation-secrets",
        action: "read",
        resource_type: "secret_variable",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "automation_secrets.create",
        title: "Create automation secret",
        description: "Create a named automation secret by key (no value). Set the value separately through the write-only value route.",
        method: "POST",
        path_template: "/api/v1/automation-secrets",
        action: "create",
        resource_type: "secret_variable",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "automation_secrets.update",
        title: "Update automation secret",
        description: "Update a named automation secret's description.",
        method: "PATCH",
        path_template: "/api/v1/automation-secrets/{id}",
        action: "update",
        resource_type: "secret_variable",
        resource_id_source: ResourceIdSource::PathParam {
            key: "id",
            prefix: "secret_variable:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "automation_secrets.delete",
        title: "Delete automation secret",
        description: "Delete a named automation secret. Fails if it is still referenced by any automation.",
        method: "DELETE",
        path_template: "/api/v1/automation-secrets/{id}",
        action: "delete",
        resource_type: "secret_variable",
        resource_id_source: ResourceIdSource::PathParam {
            key: "id",
            prefix: "secret_variable:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "audit.events.list",
        title: "List audit events",
        description: "List audit-log events through the REST API.",
        method: "GET",
        path_template: "/api/v1/audit/events",
        action: "read",
        resource_type: "audit_event",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "auth.oidc_settings.get",
        title: "Get OIDC settings",
        description: "Read instance OIDC settings.",
        method: "GET",
        path_template: "/api/v1/admin/auth/oidc/settings",
        action: "read",
        resource_type: "keycloak",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "auth.oidc_settings.update",
        title: "Update OIDC settings",
        description: "Update instance OIDC settings.",
        method: "PUT",
        path_template: "/api/v1/admin/auth/oidc/settings",
        action: "update",
        resource_type: "keycloak",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "auth.oidc_settings.test_connection",
        title: "Test OIDC settings",
        description: "Test the configured OIDC connection.",
        method: "POST",
        path_template: "/api/v1/admin/auth/oidc/test-connection",
        action: "read",
        resource_type: "keycloak",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "instance.branding.get",
        title: "Get instance branding",
        description: "Read instance branding settings.",
        method: "GET",
        path_template: "/api/v1/instance/branding",
        action: "read",
        resource_type: "keycloak",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "instance.branding.update",
        title: "Update instance branding",
        description: "Update instance branding settings.",
        method: "PUT",
        path_template: "/api/v1/instance/branding",
        action: "update",
        resource_type: "keycloak",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "connection_config.records.list",
        title: "List connection-config records",
        description: "List connection configuration records.",
        method: "GET",
        path_template: "/api/v1/connection-config/records",
        action: "read",
        resource_type: "config_context",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "connection_config.records.create",
        title: "Create connection-config record",
        description: "Create a connection configuration record.",
        method: "POST",
        path_template: "/api/v1/connection-config/records",
        action: "create",
        resource_type: "config_context",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "connection_config.records.get",
        title: "Get connection-config record",
        description: "Read one connection configuration record.",
        method: "GET",
        path_template: "/api/v1/connection-config/records/{id}",
        action: "read",
        resource_type: "config_context",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "connection_config.records.update",
        title: "Update connection-config record",
        description: "Update one connection configuration record.",
        method: "PUT",
        path_template: "/api/v1/connection-config/records/{id}",
        action: "update",
        resource_type: "config_context",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "connection_config.records.delete",
        title: "Delete connection-config record",
        description: "Delete one connection configuration record.",
        method: "DELETE",
        path_template: "/api/v1/connection-config/records/{id}",
        action: "delete",
        resource_type: "config_context",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "connection_config.assignments.list",
        title: "List connection-config assignments",
        description: "List connection configuration assignments.",
        method: "GET",
        path_template: "/api/v1/connection-config/assignments",
        action: "read",
        resource_type: "config_context",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "connection_config.assignments.create",
        title: "Create connection-config assignment",
        description: "Create a connection configuration assignment.",
        method: "POST",
        path_template: "/api/v1/connection-config/assignments",
        action: "create",
        resource_type: "config_context",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "connection_config.assignments.update",
        title: "Update connection-config assignment",
        description: "Update one connection configuration assignment.",
        method: "PUT",
        path_template: "/api/v1/connection-config/assignments/{id}",
        action: "update",
        resource_type: "config_context",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "connection_config.assignments.delete",
        title: "Delete connection-config assignment",
        description: "Delete one connection configuration assignment.",
        method: "DELETE",
        path_template: "/api/v1/connection-config/assignments/{id}",
        action: "delete",
        resource_type: "config_context",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "connection_config.assignments.bulk_assign",
        title: "Bulk-assign connection-config record",
        description: "Assign a connection configuration record to many device/VM targets in one call, reporting a per-target outcome.",
        method: "POST",
        path_template: "/api/v1/connection-config/records/{id}/assignments/bulk",
        action: "create",
        resource_type: "config_context",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "monitoring.sd.preview",
        title: "Preview monitoring service discovery",
        description: "Preview the Prometheus-compatible target groups produced by monitoring exporter assignments.",
        method: "GET",
        path_template: "/api/v1/monitoring/sd/preview",
        action: "read",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "monitoring.exporters.list",
        title: "List monitoring exporter catalog",
        description: "List exporter catalog profiles used by service discovery assignments.",
        method: "GET",
        path_template: "/api/v1/monitoring/exporters",
        action: "read",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "monitoring.exporters.create",
        title: "Create monitoring exporter profile",
        description: "Create an exporter catalog profile for service discovery.",
        method: "POST",
        path_template: "/api/v1/monitoring/exporters",
        action: "create",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "monitoring.exporters.update",
        title: "Update monitoring exporter profile",
        description: "Update one exporter catalog profile for service discovery.",
        method: "PUT",
        path_template: "/api/v1/monitoring/exporters/{id}",
        action: "update",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "monitoring.exporters.delete",
        title: "Delete monitoring exporter profile",
        description: "Delete one exporter catalog profile and its dependent service-discovery assignments.",
        method: "DELETE",
        path_template: "/api/v1/monitoring/exporters/{id}",
        action: "delete",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "monitoring.exporter_assignments.list",
        title: "List monitoring exporter assignments",
        description: "List exporter assignments that decide which targets service discovery emits.",
        method: "GET",
        path_template: "/api/v1/monitoring/exporter-assignments",
        action: "read",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "monitoring.exporter_assignments.create",
        title: "Create monitoring exporter assignment",
        description: "Bind an exporter profile to a global, site, tenant, role, or connection-target scope.",
        method: "POST",
        path_template: "/api/v1/monitoring/exporter-assignments",
        action: "create",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "monitoring.exporter_assignments.update",
        title: "Update monitoring exporter assignment",
        description: "Update one exporter assignment used by service discovery.",
        method: "PUT",
        path_template: "/api/v1/monitoring/exporter-assignments/{id}",
        action: "update",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "monitoring.exporter_assignments.delete",
        title: "Delete monitoring exporter assignment",
        description: "Delete one exporter assignment used by service discovery.",
        method: "DELETE",
        path_template: "/api/v1/monitoring/exporter-assignments/{id}",
        action: "delete",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "monitoring.presets.list",
        title: "List monitoring exporter presets",
        description: "Enumerate the built-in exporter presets that the apply and quick-start endpoints accept.",
        method: "GET",
        path_template: "/api/v1/monitoring/presets",
        action: "read",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "monitoring.exporters.apply",
        title: "Apply monitoring exporter preset",
        description: "Apply a built-in exporter preset (node/docker/blackbox/postgres) at a scope. Idempotent: re-applies reuse the same metric set and assignment without overwriting operator edits.",
        method: "POST",
        path_template: "/api/v1/monitoring/exporters/{slug}/apply",
        action: "create",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "monitoring.quick_start",
        title: "Monitoring quick-start",
        description: "Create a monitoring query source, attach it to a scope, and optionally apply an exporter preset in one call.",
        method: "POST",
        path_template: "/api/v1/monitoring/quick-start",
        action: "create",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "monitoring.targets.get",
        title: "Get monitoring target state",
        description: "Read resolved service-discovery scrapes, inspector metric sources, metric sets, and latest metric values for one connection target.",
        method: "GET",
        path_template: "/api/v1/monitoring/targets/{id}",
        action: "read",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "monitoring.latest.get",
        title: "List latest monitoring metrics",
        description: "Read latest inspector metric values for monitored connection targets.",
        method: "GET",
        path_template: "/api/v1/monitoring/latest",
        action: "read",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "monitoring.metrics.get",
        title: "List host metrics",
        description: "List the metrics configured and available for a monitored connection target. Returns a descriptor list (query, label, unit, value_type, widget_type, source_type) that can be passed to monitoring.history.get.",
        method: "GET",
        path_template: "/api/v1/monitoring/metrics",
        action: "read",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "monitoring.history.get",
        title: "Get metric history",
        description: "Fetch the time-series history of a metric for a connection target from the configured Prometheus-compatible TSDB. Pass target_id and a query string from monitoring.metrics.get; the backend validates it against configured descriptors before executing the range query.",
        method: "GET",
        path_template: "/api/v1/monitoring/history",
        action: "read",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "monitoring.query_sources.list",
        title: "List monitoring query sources",
        description: "List Prometheus-compatible and Zabbix servers used by inspector metric reads.",
        method: "GET",
        path_template: "/api/v1/monitoring/query-sources",
        action: "read",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "monitoring.query_sources.create",
        title: "Create monitoring query source",
        description: "Create a metrics server source, such as Prometheus, VictoriaMetrics, or Zabbix, for backend-side inspector metric reads.",
        method: "POST",
        path_template: "/api/v1/monitoring/query-sources",
        action: "create",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "monitoring.query_sources.update",
        title: "Update monitoring query source",
        description: "Update one metrics server source used by inspector metric reads.",
        method: "PUT",
        path_template: "/api/v1/monitoring/query-sources/{id}",
        action: "update",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "monitoring.query_sources.delete",
        title: "Delete monitoring query source",
        description: "Delete one metrics server source and stop using it for inspector metric reads.",
        method: "DELETE",
        path_template: "/api/v1/monitoring/query-sources/{id}",
        action: "delete",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "monitoring.query_source_assignments.list",
        title: "List monitoring query source assignments",
        description: "List rules that choose which metrics server source applies globally or to a site, tenant, role, or connection target.",
        method: "GET",
        path_template: "/api/v1/monitoring/query-source-assignments",
        action: "read",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "monitoring.query_source_assignments.create",
        title: "Create monitoring query source assignment",
        description: "Bind a metrics server source to a global, site, tenant, role, or connection-target scope.",
        method: "POST",
        path_template: "/api/v1/monitoring/query-source-assignments",
        action: "create",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "monitoring.query_source_assignments.update",
        title: "Update monitoring query source assignment",
        description: "Update one rule that chooses a metrics server source for inspector metric reads.",
        method: "PUT",
        path_template: "/api/v1/monitoring/query-source-assignments/{id}",
        action: "update",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "monitoring.query_source_assignments.delete",
        title: "Delete monitoring query source assignment",
        description: "Delete one rule that chooses a metrics server source for inspector metric reads.",
        method: "DELETE",
        path_template: "/api/v1/monitoring/query-source-assignments/{id}",
        action: "delete",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "monitoring.metric_queries.list",
        title: "List monitoring metric queries",
        description: "List reusable inspector metric queries, including display metadata for compact metric cards.",
        method: "GET",
        path_template: "/api/v1/monitoring/metric-queries",
        action: "read",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "monitoring.metric_queries.create",
        title: "Create monitoring metric query",
        description: "Create a reusable inspector metric query with label, unit, value type, widget type, and optional status color thresholds.",
        method: "POST",
        path_template: "/api/v1/monitoring/metric-queries",
        action: "create",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "monitoring.metric_queries.update",
        title: "Update monitoring metric query",
        description: "Update one inspector metric query and its display contract for host cards.",
        method: "PUT",
        path_template: "/api/v1/monitoring/metric-queries/{id}",
        action: "update",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "monitoring.metric_queries.delete",
        title: "Delete monitoring metric query",
        description: "Delete one reusable inspector metric query.",
        method: "DELETE",
        path_template: "/api/v1/monitoring/metric-queries/{id}",
        action: "delete",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "monitoring.metric_sets.list",
        title: "List monitoring metric sets",
        description: "List reusable sets of inspector metric queries shown together on connection-target cards.",
        method: "GET",
        path_template: "/api/v1/monitoring/metric-sets",
        action: "read",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "monitoring.metric_sets.create",
        title: "Create monitoring metric set",
        description: "Create a reusable inspector metric set for grouping card metrics.",
        method: "POST",
        path_template: "/api/v1/monitoring/metric-sets",
        action: "create",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "monitoring.metric_sets.update",
        title: "Update monitoring metric set",
        description: "Update one inspector metric set.",
        method: "PUT",
        path_template: "/api/v1/monitoring/metric-sets/{id}",
        action: "update",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "monitoring.metric_sets.delete",
        title: "Delete monitoring metric set",
        description: "Delete one inspector metric set.",
        method: "DELETE",
        path_template: "/api/v1/monitoring/metric-sets/{id}",
        action: "delete",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "monitoring.metric_set_items.create",
        title: "Add monitoring metric set item",
        description: "Add one metric query to an inspector metric set and choose ordering or item-level overrides.",
        method: "POST",
        path_template: "/api/v1/monitoring/metric-sets/{id}/items",
        action: "create",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "monitoring.metric_set_items.update",
        title: "Update monitoring metric set item",
        description: "Update one metric query item inside an inspector metric set.",
        method: "PUT",
        path_template: "/api/v1/monitoring/metric-sets/{id}/items/{item_id}",
        action: "update",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "monitoring.metric_set_items.delete",
        title: "Delete monitoring metric set item",
        description: "Remove one metric query item from an inspector metric set.",
        method: "DELETE",
        path_template: "/api/v1/monitoring/metric-sets/{id}/items/{item_id}",
        action: "delete",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "monitoring.metric_set_assignments.list",
        title: "List monitoring metric set assignments",
        description: "List rules that decide which inspector metric sets appear globally or on a site, tenant, role, or connection target.",
        method: "GET",
        path_template: "/api/v1/monitoring/metric-set-assignments",
        action: "read",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "monitoring.metric_set_assignments.create",
        title: "Create monitoring metric set assignment",
        description: "Bind an inspector metric set to a global, site, tenant, role, or connection-target scope.",
        method: "POST",
        path_template: "/api/v1/monitoring/metric-set-assignments",
        action: "create",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "monitoring.metric_set_assignments.update",
        title: "Update monitoring metric set assignment",
        description: "Update one rule that decides where an inspector metric set appears.",
        method: "PUT",
        path_template: "/api/v1/monitoring/metric-set-assignments/{id}",
        action: "update",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "monitoring.metric_set_assignments.delete",
        title: "Delete monitoring metric set assignment",
        description: "Delete one rule that decides where an inspector metric set appears.",
        method: "DELETE",
        path_template: "/api/v1/monitoring/metric-set-assignments/{id}",
        action: "delete",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "monitoring.sd_label_settings.get",
        title: "Get monitoring SD label settings",
        description: "Read service-discovery label groups used to shape Prometheus-compatible target metadata.",
        method: "GET",
        path_template: "/api/v1/monitoring/sd/label-settings",
        action: "read",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "monitoring.sd_label_settings.update",
        title: "Update monitoring SD label settings",
        description: "Update service-discovery label groups used to shape Prometheus-compatible target metadata.",
        method: "PUT",
        path_template: "/api/v1/monitoring/sd/label-settings",
        action: "update",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.catalog.list",
        title: "List playbooks",
        description: "List Linux-over-SSH playbooks with current version metadata. Use this before choosing a playbook to run or assign.",
        method: "GET",
        path_template: "/api/v1/playbooks",
        action: "read",
        resource_type: "playbook",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.catalog.create",
        title: "Create playbook",
        description: "Create a Linux-over-SSH playbook catalog item and its initial version.",
        method: "POST",
        path_template: "/api/v1/playbooks",
        action: "create",
        resource_type: "playbook",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.catalog.get",
        title: "Get playbook",
        description: "Read one playbook catalog item by id.",
        method: "GET",
        path_template: "/api/v1/playbooks/{id}",
        action: "read",
        resource_type: "playbook",
        resource_id_source: ResourceIdSource::PathParam {
            key: "id",
            prefix: "playbook:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.catalog.update",
        title: "Update playbook",
        description: "Update one playbook catalog item.",
        method: "PATCH",
        path_template: "/api/v1/playbooks/{id}",
        action: "update",
        resource_type: "playbook",
        resource_id_source: ResourceIdSource::PathParam {
            key: "id",
            prefix: "playbook:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.catalog.delete",
        title: "Delete playbook",
        description: "Delete one playbook catalog item.",
        method: "DELETE",
        path_template: "/api/v1/playbooks/{id}",
        action: "delete",
        resource_type: "playbook",
        resource_id_source: ResourceIdSource::PathParam {
            key: "id",
            prefix: "playbook:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.catalog.duplicate",
        title: "Duplicate playbook",
        description: "Duplicate one playbook catalog item into an independent copy.",
        method: "POST",
        path_template: "/api/v1/playbooks/{id}/duplicate",
        action: "create",
        resource_type: "playbook",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.versions.list",
        title: "List playbook versions",
        description: "List versions for a playbook before editing or launching a specific revision.",
        method: "GET",
        path_template: "/api/v1/playbooks/{id}/versions",
        action: "read",
        resource_type: "playbook",
        resource_id_source: ResourceIdSource::PathParam {
            key: "id",
            prefix: "playbook:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.versions.create",
        title: "Create playbook version",
        description: "Create a new version for an existing Linux-over-SSH playbook.",
        method: "POST",
        path_template: "/api/v1/playbooks/{id}/versions",
        action: "update",
        resource_type: "playbook",
        resource_id_source: ResourceIdSource::PathParam {
            key: "id",
            prefix: "playbook:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.execution_environments.list",
        title: "List execution environments",
        description: "List container execution environments available to job templates and projects.",
        method: "GET",
        path_template: "/api/v1/playbooks/execution-environments",
        action: "read",
        resource_type: "playbook_execution_environment",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.execution_environments.create",
        title: "Create execution environment",
        description: "Register a container image and runner settings usable as a playbook execution environment.",
        method: "POST",
        path_template: "/api/v1/playbooks/execution-environments",
        action: "create",
        resource_type: "playbook_execution_environment",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.execution_environments.get",
        title: "Get execution environment",
        description: "Read one execution environment by id.",
        method: "GET",
        path_template: "/api/v1/playbooks/execution-environments/{id}",
        action: "read",
        resource_type: "playbook_execution_environment",
        resource_id_source: ResourceIdSource::PathParam {
            key: "id",
            prefix: "playbook_execution_environment:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.execution_environments.update",
        title: "Update execution environment",
        description: "Update one execution environment by id.",
        method: "PATCH",
        path_template: "/api/v1/playbooks/execution-environments/{id}",
        action: "update",
        resource_type: "playbook_execution_environment",
        resource_id_source: ResourceIdSource::PathParam {
            key: "id",
            prefix: "playbook_execution_environment:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.execution_environments.delete",
        title: "Delete execution environment",
        description: "Delete one execution environment; projects or job templates still using it must be retargeted first.",
        method: "DELETE",
        path_template: "/api/v1/playbooks/execution-environments/{id}",
        action: "delete",
        resource_type: "playbook_execution_environment",
        resource_id_source: ResourceIdSource::PathParam {
            key: "id",
            prefix: "playbook_execution_environment:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.projects.list",
        title: "List playbook projects",
        description: "List manual and git-backed playbook projects.",
        method: "GET",
        path_template: "/api/v1/playbooks/projects",
        action: "read",
        resource_type: "playbook_project",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.projects.create",
        title: "Create playbook project",
        description: "Create a manual or git-backed project that supplies playbook files to job templates.",
        method: "POST",
        path_template: "/api/v1/playbooks/projects",
        action: "create",
        resource_type: "playbook_project",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.projects.get",
        title: "Get playbook project",
        description: "Read one playbook project by id.",
        method: "GET",
        path_template: "/api/v1/playbooks/projects/{id}",
        action: "read",
        resource_type: "playbook_project",
        resource_id_source: ResourceIdSource::PathParam {
            key: "id",
            prefix: "playbook_project:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.projects.update",
        title: "Update playbook project",
        description: "Update one playbook project's identity and SCM settings; project files are managed separately.",
        method: "PATCH",
        path_template: "/api/v1/playbooks/projects/{id}",
        action: "update",
        resource_type: "playbook_project",
        resource_id_source: ResourceIdSource::PathParam {
            key: "id",
            prefix: "playbook_project:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.projects.delete",
        title: "Delete playbook project",
        description: "Delete one playbook project; job templates using it must be deleted or retargeted first.",
        method: "DELETE",
        path_template: "/api/v1/playbooks/projects/{id}",
        action: "delete",
        resource_type: "playbook_project",
        resource_id_source: ResourceIdSource::PathParam {
            key: "id",
            prefix: "playbook_project:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.projects.upload_files",
        title: "Upload manual project files",
        description: "Replace the stored file set of a manual project; relative paths only, no traversal or ansible.cfg.",
        method: "POST",
        path_template: "/api/v1/playbooks/projects/{id}/files",
        action: "update",
        resource_type: "playbook_project",
        resource_id_source: ResourceIdSource::PathParam {
            key: "id",
            prefix: "playbook_project:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.job_templates.list",
        title: "List job templates",
        description: "List reusable job templates that bind a playbook or project to defaults for launch.",
        method: "GET",
        path_template: "/api/v1/playbooks/job-templates",
        action: "read",
        resource_type: "playbook_job_template",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.job_templates.create",
        title: "Create job template",
        description: "Create a job template binding a playbook or project + path to launch defaults.",
        method: "POST",
        path_template: "/api/v1/playbooks/job-templates",
        action: "create",
        resource_type: "playbook_job_template",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.job_templates.get",
        title: "Get job template",
        description: "Read one job template by id.",
        method: "GET",
        path_template: "/api/v1/playbooks/job-templates/{id}",
        action: "read",
        resource_type: "playbook_job_template",
        resource_id_source: ResourceIdSource::PathParam {
            key: "id",
            prefix: "playbook_job_template:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.job_templates.update",
        title: "Update job template",
        description: "Update one job template's defaults; its playbook/project source binding is immutable.",
        method: "PATCH",
        path_template: "/api/v1/playbooks/job-templates/{id}",
        action: "update",
        resource_type: "playbook_job_template",
        resource_id_source: ResourceIdSource::PathParam {
            key: "id",
            prefix: "playbook_job_template:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.job_templates.delete",
        title: "Delete job template",
        description: "Delete one job template by id.",
        method: "DELETE",
        path_template: "/api/v1/playbooks/job-templates/{id}",
        action: "delete",
        resource_type: "playbook_job_template",
        resource_id_source: ResourceIdSource::PathParam {
            key: "id",
            prefix: "playbook_job_template:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.job_templates.launch",
        title: "Launch job template",
        description: "Queue a run group from a job template; target_set_id can override the default target set.",
        method: "POST",
        path_template: "/api/v1/playbooks/job-templates/{id}/launch",
        action: "execute",
        resource_type: "playbook",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.target_sets.list",
        title: "List playbook target sets",
        description: "List saved static or dynamic target selectors for default hosts, bulk launch, schedules, and agent automation.",
        method: "GET",
        path_template: "/api/v1/playbooks/target-sets",
        action: "read",
        resource_type: "playbook_target_set",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.target_sets.create",
        title: "Create playbook target set",
        description: "Create a static or dynamic connection-target selector for playbook execution.",
        method: "POST",
        path_template: "/api/v1/playbooks/target-sets",
        action: "create",
        resource_type: "playbook_target_set",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.target_sets.get",
        title: "Get playbook target set",
        description: "Read one saved playbook target set without resolving it.",
        method: "GET",
        path_template: "/api/v1/playbooks/target-sets/{id}",
        action: "read",
        resource_type: "playbook_target_set",
        resource_id_source: ResourceIdSource::PathParam {
            key: "id",
            prefix: "playbook_target_set:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.target_sets.update",
        title: "Update playbook target set",
        description: "Replace one saved static or dynamic target selector.",
        method: "PUT",
        path_template: "/api/v1/playbooks/target-sets/{id}",
        action: "update",
        resource_type: "playbook_target_set",
        resource_id_source: ResourceIdSource::PathParam {
            key: "id",
            prefix: "playbook_target_set:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.target_sets.delete",
        title: "Delete playbook target set",
        description: "Delete one saved target set. Schedules using it must be removed or retargeted first.",
        method: "DELETE",
        path_template: "/api/v1/playbooks/target-sets/{id}",
        action: "delete",
        resource_type: "playbook_target_set",
        resource_id_source: ResourceIdSource::PathParam {
            key: "id",
            prefix: "playbook_target_set:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.target_sets.preview",
        title: "Preview playbook target set",
        description: "Preview target resolution before launching; zero targets must not fall back to all hosts.",
        method: "GET",
        path_template: "/api/v1/playbooks/target-sets/{id}/preview",
        action: "read",
        resource_type: "playbook_target_set",
        resource_id_source: ResourceIdSource::PathParam {
            key: "id",
            prefix: "playbook_target_set:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.assignments.list",
        title: "List playbook assignments",
        description: "List where playbooks are offered and which defaults apply by global, site, tenant, role, platform, cluster, or connection-target scope.",
        method: "GET",
        path_template: "/api/v1/playbooks/assignments",
        action: "read",
        resource_type: "playbook_assignment",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.assignments.create",
        title: "Create playbook assignment",
        description: "Bind a playbook and target set to a scope with quick-action and default-run settings.",
        method: "POST",
        path_template: "/api/v1/playbooks/assignments",
        action: "create",
        resource_type: "playbook_assignment",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.assignments.get",
        title: "Get playbook assignment",
        description: "Read one playbook assignment by id.",
        method: "GET",
        path_template: "/api/v1/playbooks/assignments/{id}",
        action: "read",
        resource_type: "playbook_assignment",
        resource_id_source: ResourceIdSource::PathParam {
            key: "id",
            prefix: "playbook_assignment:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.assignments.update",
        title: "Update playbook assignment",
        description: "Update one playbook assignment by id.",
        method: "PATCH",
        path_template: "/api/v1/playbooks/assignments/{id}",
        action: "update",
        resource_type: "playbook_assignment",
        resource_id_source: ResourceIdSource::PathParam {
            key: "id",
            prefix: "playbook_assignment:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.assignments.delete",
        title: "Delete playbook assignment",
        description: "Delete one playbook assignment by id.",
        method: "DELETE",
        path_template: "/api/v1/playbooks/assignments/{id}",
        action: "delete",
        resource_type: "playbook_assignment",
        resource_id_source: ResourceIdSource::PathParam {
            key: "id",
            prefix: "playbook_assignment:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.assignments.list_for_target",
        title: "List target playbook assignments",
        description: "List playbook assignments eligible for one selected connection target.",
        method: "GET",
        path_template: "/api/v1/playbooks/targets/{target_id}/assignments",
        action: "read",
        resource_type: "playbook_assignment",
        resource_id_source: ResourceIdSource::PathParam {
            key: "target_id",
            prefix: "connection_target:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "connection_target.cctv.capabilities",
        title: "Read connection target CCTV capabilities",
        description: "Read a CCTV target's provider-neutral capability and permission manifest. Available when the caller has any cctv:* permission for this target; does not contact the device.",
        method: "GET",
        path_template: "/api/v1/cctv/{target_id}/capabilities",
        action: "view",
        resource_type: "cctv",
        resource_id_source: ResourceIdSource::PathParam {
            key: "target_id",
            prefix: "cctv:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "connection_target.cctv.channels",
        title: "List connection target CCTV channels",
        description: "Enumerate a CCTV target's camera/input channels. Requires cctv:view for this target.",
        method: "GET",
        path_template: "/api/v1/cctv/{target_id}/channels",
        action: "view",
        resource_type: "cctv",
        resource_id_source: ResourceIdSource::PathParam {
            key: "target_id",
            prefix: "cctv:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "connection_target.cctv.ptz",
        title: "Control connection target camera PTZ",
        description: "Issue a pan/tilt/zoom command for a structurally confirmed CCTV channel. Requires cctv:ptz for this target.",
        method: "POST",
        path_template: "/api/v1/cctv/{target_id}/ptz",
        action: "ptz",
        resource_type: "cctv",
        resource_id_source: ResourceIdSource::PathParam {
            key: "target_id",
            prefix: "cctv:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "connection_target.device.info",
        title: "Read connection target device info",
        description: "Read a CCTV target's device identity, firmware, and clock. Requires cctv:view for this target.",
        method: "GET",
        path_template: "/api/v1/cctv/{target_id}/info",
        action: "view",
        resource_type: "cctv",
        resource_id_source: ResourceIdSource::PathParam {
            key: "target_id",
            prefix: "cctv:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "connection_target.device.users",
        title: "List connection target device users",
        description: "List user accounts on a CCTV device without returning secret values. Requires device_users:connection_target for this target.",
        method: "GET",
        path_template: "/api/v1/device-fleet/{target_id}/users",
        action: "device_users",
        resource_type: "connection_target",
        resource_id_source: ResourceIdSource::PathParam {
            key: "target_id",
            prefix: "connection_target:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "connection_target.device.authorities",
        title: "List connection target device authority catalog",
        description: "List the verbatim authority tokens accepted by a CCTV device for user accounts. Requires device_users:connection_target for this target.",
        method: "GET",
        path_template: "/api/v1/cctv/{target_id}/authorities",
        action: "device_users",
        resource_type: "connection_target",
        resource_id_source: ResourceIdSource::PathParam {
            key: "target_id",
            prefix: "connection_target:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "connection_target.device.disk",
        title: "Read connection target device disk health",
        description: "Read disk/storage health from a CCTV device. Requires cctv:settings for this target.",
        method: "GET",
        path_template: "/api/v1/cctv/{target_id}/disk",
        action: "settings",
        resource_type: "cctv",
        resource_id_source: ResourceIdSource::PathParam {
            key: "target_id",
            prefix: "cctv:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "connection_target.device.recording",
        title: "Read connection target device recording state",
        description: "Read recording state and available footage spans from a CCTV device. Requires cctv:recordings for this target.",
        method: "GET",
        path_template: "/api/v1/cctv/{target_id}/recording",
        action: "recordings",
        resource_type: "cctv",
        resource_id_source: ResourceIdSource::PathParam {
            key: "target_id",
            prefix: "cctv:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "connection_target.device.recordings.search",
        title: "Search connection target device recordings",
        description: "Search recorded footage for one channel and RFC3339 time window on a CCTV device — per-channel, unlike the device-wide recording state. Requires cctv:recordings for this target.",
        method: "GET",
        path_template: "/api/v1/cctv/{target_id}/recordings",
        action: "recordings",
        resource_type: "cctv",
        resource_id_source: ResourceIdSource::PathParam {
            key: "target_id",
            prefix: "cctv:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "connection_target.device.users.preview",
        title: "Preview connection target device user changes",
        description: "Dry-run a provider-neutral CCTV user change without mutating the device. Requires device_users:connection_target for this target.",
        method: "POST",
        path_template: "/api/v1/device-fleet/{target_id}/users:preview",
        action: "device_users",
        resource_type: "connection_target",
        resource_id_source: ResourceIdSource::PathParam {
            key: "target_id",
            prefix: "connection_target:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "connection_target.device.users.create",
        title: "Provision connection target device users",
        description: "Provision user accounts on a CCTV device. Requires device_users:connection_target for this target. Passwords are write-only and never returned.",
        method: "POST",
        path_template: "/api/v1/device-fleet/{target_id}/users:create",
        action: "device_users",
        resource_type: "connection_target",
        resource_id_source: ResourceIdSource::PathParam {
            key: "target_id",
            prefix: "connection_target:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "connection_target.device.users.update",
        title: "Edit connection target device users",
        description: "Edit existing CCTV device users. Requires device_users:connection_target for this target. Passwords are write-only and never returned.",
        method: "PUT",
        path_template: "/api/v1/device-fleet/{target_id}/users",
        action: "device_users",
        resource_type: "connection_target",
        resource_id_source: ResourceIdSource::PathParam {
            key: "target_id",
            prefix: "connection_target:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "connection_target.device.users.delete",
        title: "Remove connection target device users",
        description: "Remove CCTV device users by login name. Requires device_users:connection_target for this target.",
        method: "POST",
        path_template: "/api/v1/device-fleet/{target_id}/users:delete",
        action: "device_users",
        resource_type: "connection_target",
        resource_id_source: ResourceIdSource::PathParam {
            key: "target_id",
            prefix: "connection_target:",
        },
    },
    // Fleet operations carry target_ids in the body. The device_users:connection_target scope makes
    // the tools visible; the handlers still evaluate the same permission and
    // resource pattern independently for every requested target.
    StaticMcpRestEndpoint {
        operation_id: "connection_target.device_fleet.users.preview",
        title: "Preview fleet device user changes",
        description: "Dry-run CCTV user changes across many targets. Requires device_users:connection_target; each target is independently authorized and denied targets are reported as not_permitted.",
        method: "POST",
        path_template: "/api/v1/device-fleet/users:preview",
        action: "device_users",
        resource_type: "connection_target",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "connection_target.device_fleet.users.create",
        title: "Provision fleet device users",
        description: "Provision CCTV users across many targets. Requires device_users:connection_target; each target is independently authorized. Passwords are write-only and never returned.",
        method: "POST",
        path_template: "/api/v1/device-fleet/users:create",
        action: "device_users",
        resource_type: "connection_target",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "connection_target.device_fleet.users.delete",
        title: "Remove fleet device users",
        description: "Remove CCTV users across many targets. Requires device_users:connection_target; each target is independently authorized.",
        method: "POST",
        path_template: "/api/v1/device-fleet/users:delete",
        action: "device_users",
        resource_type: "connection_target",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.run_groups.queue",
        title: "Queue playbook run group",
        description: "Queue a manual Linux-over-SSH playbook run group; target_set_id can override the default target set.",
        method: "POST",
        path_template: "/api/v1/playbooks/{id}/runs",
        action: "execute",
        resource_type: "playbook",
        resource_id_source: ResourceIdSource::PathParam {
            key: "id",
            prefix: "playbook:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.secrets_preflight",
        title: "Preflight playbook secret grants",
        description: "Report, per declared runtime secret, whether the caller may export it into a run before launching (dry-run of gate C3).",
        method: "GET",
        path_template: "/api/v1/playbooks/{id}/secrets-preflight",
        action: "read",
        resource_type: "playbook",
        resource_id_source: ResourceIdSource::PathParam {
            key: "id",
            prefix: "playbook:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.schedules.list",
        title: "List playbook schedules",
        description: "List durable cron-like playbook schedules and their next run state.",
        method: "GET",
        path_template: "/api/v1/playbooks/schedules",
        action: "read",
        resource_type: "playbook_schedule",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.schedules.create",
        title: "Create playbook schedule",
        description: "Create a durable cron-like schedule for a playbook target set and service principal; scheduled execution uses service-principal lease internals.",
        method: "POST",
        path_template: "/api/v1/playbooks/schedules",
        action: "create",
        resource_type: "playbook_schedule",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.schedules.get",
        title: "Get playbook schedule",
        description: "Read one durable playbook schedule by id.",
        method: "GET",
        path_template: "/api/v1/playbooks/schedules/{id}",
        action: "read",
        resource_type: "playbook_schedule",
        resource_id_source: ResourceIdSource::PathParam {
            key: "id",
            prefix: "playbook_schedule:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.run_artifacts.list",
        title: "List playbook run artifacts",
        description: "Read artifacts uploaded by the remote wrapper for one playbook run; callback writes are intentionally not normal MCP control-plane operations.",
        method: "GET",
        path_template: "/api/v1/playbooks/runs/{run_id}/artifacts",
        action: "read",
        resource_type: "playbook_run",
        resource_id_source: ResourceIdSource::PathParam {
            key: "run_id",
            prefix: "playbook_run:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.run_events.list",
        title: "List playbook run events",
        description: "Read status/stdout/stderr/system/result events uploaded by the remote wrapper; callback writes are intentionally not normal MCP control-plane operations.",
        method: "GET",
        path_template: "/api/v1/playbooks/runs/{run_id}/events",
        action: "read",
        resource_type: "playbook_run",
        resource_id_source: ResourceIdSource::PathParam {
            key: "run_id",
            prefix: "playbook_run:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.schedules.update",
        title: "Update playbook schedule",
        description: "Update a durable cron-like playbook schedule (cron expression, target set, enabled state, etc.).",
        method: "PATCH",
        path_template: "/api/v1/playbooks/schedules/{id}",
        action: "update",
        resource_type: "playbook_schedule",
        resource_id_source: ResourceIdSource::PathParam {
            key: "id",
            prefix: "playbook_schedule:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.schedules.delete",
        title: "Delete playbook schedule",
        description: "Delete a durable playbook schedule by id.",
        method: "DELETE",
        path_template: "/api/v1/playbooks/schedules/{id}",
        action: "delete",
        resource_type: "playbook_schedule",
        resource_id_source: ResourceIdSource::PathParam {
            key: "id",
            prefix: "playbook_schedule:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.service_principals.list",
        title: "List playbook service principals",
        description: "List service principals used for scheduled execution; scheduled execution uses service-principal lease internals.",
        method: "GET",
        path_template: "/api/v1/playbooks/service-principals",
        action: "read",
        resource_type: "playbook_service_principal",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.service_principals.create",
        title: "Create playbook service principal",
        description: "Create a service principal for automated scheduled playbook execution.",
        method: "POST",
        path_template: "/api/v1/playbooks/service-principals",
        action: "create",
        resource_type: "playbook_service_principal",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.service_principals.get",
        title: "Get playbook service principal",
        description: "Read one playbook service principal by id.",
        method: "GET",
        path_template: "/api/v1/playbooks/service-principals/{id}",
        action: "read",
        resource_type: "playbook_service_principal",
        resource_id_source: ResourceIdSource::PathParam {
            key: "id",
            prefix: "playbook_service_principal:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.service_principals.revoke",
        title: "Revoke playbook service principal",
        description: "Revoke a playbook service principal, preventing future scheduled execution leases.",
        method: "POST",
        path_template: "/api/v1/playbooks/service-principals/{id}/revoke",
        action: "delete",
        resource_type: "playbook_service_principal",
        resource_id_source: ResourceIdSource::PathParam {
            key: "id",
            prefix: "playbook_service_principal:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.runs.list",
        title: "List playbook runs",
        description: "List playbook run history, filterable by playbook and status.",
        method: "GET",
        path_template: "/api/v1/playbooks/runs",
        action: "read",
        resource_type: "playbook_run",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.runs.get",
        title: "Get playbook run",
        description: "Read a single playbook run with current live status.",
        method: "GET",
        path_template: "/api/v1/playbooks/runs/{run_id}",
        action: "read",
        resource_type: "playbook_run",
        resource_id_source: ResourceIdSource::PathParam {
            key: "run_id",
            prefix: "playbook_run:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.runs.cancel",
        title: "Cancel playbook run",
        description: "Request cancellation for one playbook run.",
        method: "POST",
        path_template: "/api/v1/playbooks/runs/{run_id}/cancel",
        action: "execute",
        resource_type: "playbook",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.run_groups.list",
        title: "List playbook run groups",
        description: "List playbook run groups with aggregate target counts and status rollups.",
        method: "GET",
        path_template: "/api/v1/playbooks/run-groups",
        action: "read",
        resource_type: "playbook_run_group",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.run_groups.get",
        title: "Get playbook run group",
        description: "Read a playbook run group with its per-target runs.",
        method: "GET",
        path_template: "/api/v1/playbooks/run-groups/{id}",
        action: "read",
        resource_type: "playbook_run_group",
        resource_id_source: ResourceIdSource::PathParam {
            key: "id",
            prefix: "playbook_run_group:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.run_group_events.list",
        title: "List playbook run-group events",
        description: "Read the aggregated per-run event stream of one playbook run group; callback writes are intentionally not normal MCP control-plane operations.",
        method: "GET",
        path_template: "/api/v1/playbooks/run-groups/{group_id}/events",
        action: "read",
        resource_type: "playbook_run_group",
        resource_id_source: ResourceIdSource::PathParam {
            key: "group_id",
            prefix: "playbook_run_group:",
        },
    },
    StaticMcpRestEndpoint {
        operation_id: "playbooks.run_groups.cancel",
        title: "Cancel playbook run group",
        description: "Request cancellation for active runs in one playbook run group.",
        method: "POST",
        path_template: "/api/v1/playbooks/run-groups/{group_id}/cancel",
        action: "execute",
        resource_type: "playbook",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "providers.list",
        title: "List sync providers",
        description: "List configured sync providers.",
        method: "GET",
        path_template: "/api/v1/providers",
        action: "read",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "providers.create",
        title: "Create sync provider",
        description: "Create a sync provider.",
        method: "POST",
        path_template: "/api/v1/providers",
        action: "create",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "providers.get",
        title: "Get sync provider",
        description: "Read one sync provider.",
        method: "GET",
        path_template: "/api/v1/providers/{id}",
        action: "read",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "providers.update",
        title: "Update sync provider",
        description: "Update one sync provider.",
        method: "PATCH",
        path_template: "/api/v1/providers/{id}",
        action: "update",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "providers.delete",
        title: "Delete sync provider",
        description: "Delete one sync provider.",
        method: "DELETE",
        path_template: "/api/v1/providers/{id}",
        action: "delete",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "providers.bootstrap",
        title: "Bootstrap sync provider",
        description: "Run provider bootstrap.",
        method: "POST",
        path_template: "/api/v1/providers/{id}/bootstrap",
        action: "execute",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "providers.status",
        title: "Get sync provider status",
        description: "Read one sync provider status.",
        method: "GET",
        path_template: "/api/v1/providers/{id}/status",
        action: "read",
        resource_type: "inventory",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "webhooks.list",
        title: "List webhooks",
        description: "List outbound webhook subscriptions.",
        method: "GET",
        path_template: "/api/v1/webhooks",
        action: "read",
        resource_type: "webhook",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "webhooks.create",
        title: "Create webhook",
        description: "Create an outbound webhook subscription.",
        method: "POST",
        path_template: "/api/v1/webhooks",
        action: "update",
        resource_type: "webhook",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "webhooks.get",
        title: "Get webhook",
        description: "Read one outbound webhook subscription.",
        method: "GET",
        path_template: "/api/v1/webhooks/{id}",
        action: "read",
        resource_type: "webhook",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "webhooks.update",
        title: "Update webhook",
        description: "Update one outbound webhook subscription.",
        method: "PUT",
        path_template: "/api/v1/webhooks/{id}",
        action: "update",
        resource_type: "webhook",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "webhooks.delete",
        title: "Delete webhook",
        description: "Delete one outbound webhook subscription.",
        method: "DELETE",
        path_template: "/api/v1/webhooks/{id}",
        action: "update",
        resource_type: "webhook",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "webhooks.test_fire",
        title: "Test-fire webhook",
        description: "Send a test event to one webhook subscription.",
        method: "POST",
        path_template: "/api/v1/webhooks/{id}/test-fire",
        action: "update",
        resource_type: "webhook",
        resource_id_source: ResourceIdSource::None,
    },
    StaticMcpRestEndpoint {
        operation_id: "webhooks.clear_secret",
        title: "Clear webhook secret",
        description: "Clear one webhook signing secret.",
        method: "DELETE",
        path_template: "/api/v1/webhooks/{id}/secret",
        action: "update",
        resource_type: "webhook",
        resource_id_source: ResourceIdSource::None,
    },
];

#[cfg(test)]
mod tests {
    use crate::api::mcp::test_support::*;

    #[test]
    fn full_control_plane_exposes_monitoring_agent_workflow_endpoints() {
        let request_context = full_context();
        let endpoints = visible_scoped_rest_endpoints(&request_context);
        let operations = endpoints
            .iter()
            .map(|endpoint| endpoint.operation_id.as_str())
            .collect::<Vec<_>>();

        for operation in [
            "monitoring.sd.preview",
            "monitoring.exporters.list",
            "monitoring.exporters.create",
            "monitoring.exporters.update",
            "monitoring.exporters.delete",
            "monitoring.exporter_assignments.list",
            "monitoring.exporter_assignments.create",
            "monitoring.exporter_assignments.update",
            "monitoring.exporter_assignments.delete",
            "monitoring.targets.get",
            "monitoring.latest.get",
            "monitoring.query_sources.list",
            "monitoring.query_sources.create",
            "monitoring.query_sources.update",
            "monitoring.query_sources.delete",
            "monitoring.query_source_assignments.list",
            "monitoring.query_source_assignments.create",
            "monitoring.query_source_assignments.update",
            "monitoring.query_source_assignments.delete",
            "monitoring.metric_queries.list",
            "monitoring.metric_queries.create",
            "monitoring.metric_queries.update",
            "monitoring.metric_queries.delete",
            "monitoring.metric_sets.list",
            "monitoring.metric_sets.create",
            "monitoring.metric_sets.update",
            "monitoring.metric_sets.delete",
            "monitoring.metric_set_items.create",
            "monitoring.metric_set_items.update",
            "monitoring.metric_set_items.delete",
            "monitoring.metric_set_assignments.list",
            "monitoring.metric_set_assignments.create",
            "monitoring.metric_set_assignments.update",
            "monitoring.metric_set_assignments.delete",
            "monitoring.sd_label_settings.get",
            "monitoring.sd_label_settings.update",
        ] {
            assert!(
                operations.contains(&operation),
                "full-control MCP catalog must expose {operation}"
            );
        }

        assert!(
            endpoints
                .iter()
                .all(|endpoint| endpoint.path_template != "/api/v1/monitoring/sd"),
            "collector service-discovery endpoint must stay out of the agent control-plane catalog"
        );
    }

    #[test]
    fn scoped_rest_endpoint_catalog_paths_are_present_in_openapi() {
        let yaml: serde_json::Value =
            serde_yml::from_str(include_str!("../../../contracts/piping.openapi.yaml"))
                .expect("valid OpenAPI yaml");
        let paths = yaml
            .get("paths")
            .and_then(serde_json::Value::as_object)
            .expect("the OpenAPI paths object");

        for endpoint in mcp_rest_endpoints() {
            let expected_path = if endpoint
                .path_template
                .starts_with("/api/v1/inventory/resources/")
                && endpoint.path_template.ends_with("/bulk")
            {
                "/api/v1/inventory/resources/{entity_type}/bulk"
            } else {
                endpoint.path_template.as_str()
            };
            let path = paths
                .get(expected_path)
                .unwrap_or_else(|| panic!("MCP endpoint path missing from OpenAPI: {endpoint:?}"));
            let method = endpoint.method.as_str().to_ascii_lowercase();
            assert!(
                path.get(method.as_str()).is_some(),
                "MCP endpoint method missing from OpenAPI: {} {}",
                endpoint.method,
                endpoint.path_template
            );
        }
    }

    #[test]
    fn device_surfaces_use_exact_namespaced_permissions() {
        let expected = [
            (
                Method::GET,
                "/api/v1/cctv/{target_id}/capabilities",
                "view",
                "cctv",
            ),
            (
                Method::GET,
                "/api/v1/cctv/{target_id}/channels",
                "view",
                "cctv",
            ),
            (Method::GET, "/api/v1/cctv/{target_id}/info", "view", "cctv"),
            (
                Method::GET,
                "/api/v1/device-fleet/{target_id}/users",
                "device_users",
                "connection_target",
            ),
            (
                Method::GET,
                "/api/v1/cctv/{target_id}/disk",
                "settings",
                "cctv",
            ),
            (
                Method::GET,
                "/api/v1/cctv/{target_id}/recording",
                "recordings",
                "cctv",
            ),
            (Method::POST, "/api/v1/cctv/{target_id}/ptz", "ptz", "cctv"),
            (
                Method::POST,
                "/api/v1/device-fleet/{target_id}/users:preview",
                "device_users",
                "connection_target",
            ),
            (
                Method::POST,
                "/api/v1/device-fleet/{target_id}/users:create",
                "device_users",
                "connection_target",
            ),
            (
                Method::PUT,
                "/api/v1/device-fleet/{target_id}/users",
                "device_users",
                "connection_target",
            ),
            (
                Method::POST,
                "/api/v1/device-fleet/{target_id}/users:delete",
                "device_users",
                "connection_target",
            ),
            (
                Method::POST,
                "/api/v1/device-fleet/users:preview",
                "device_users",
                "connection_target",
            ),
            (
                Method::POST,
                "/api/v1/device-fleet/users:create",
                "device_users",
                "connection_target",
            ),
            (
                Method::POST,
                "/api/v1/device-fleet/users:delete",
                "device_users",
                "connection_target",
            ),
        ];
        let endpoints = mcp_rest_endpoints();
        for (method, path, action, resource_type) in expected {
            let matches = endpoints
                .iter()
                .filter(|endpoint| endpoint.method == method && endpoint.path_template == path)
                .collect::<Vec<_>>();
            assert_eq!(
                matches.len(),
                1,
                "expected exactly one MCP mapping for {method} {path}"
            );
            assert_eq!(matches[0].action, action);
            assert_eq!(matches[0].resource_type, resource_type);
        }
    }

    #[test]
    fn monitoring_control_plane_rest_paths_are_covered_by_scoped_mcp_catalog() {
        let yaml: serde_json::Value =
            serde_yml::from_str(include_str!("../../../contracts/piping.openapi.yaml"))
                .expect("valid OpenAPI yaml");
        let paths = yaml
            .get("paths")
            .and_then(serde_json::Value::as_object)
            .expect("the OpenAPI paths object");
        let mcp_paths = mcp_rest_endpoints()
            .into_iter()
            .map(|endpoint| {
                (
                    endpoint.method.as_str().to_ascii_lowercase(),
                    endpoint.path_template.to_string(),
                )
            })
            .collect::<HashSet<_>>();
        let collector_only_paths = HashSet::from([("get", "/api/v1/monitoring/sd")]);

        for (path_template, path_spec) in paths {
            if !path_template.starts_with("/api/v1/monitoring/") {
                continue;
            }

            let methods = path_spec
                .as_object()
                .expect("an OpenAPI path spec object")
                .keys()
                .filter(|method| {
                    matches!(method.as_str(), "get" | "post" | "put" | "patch" | "delete")
                });

            for method in methods {
                let path_key = (method.as_str(), path_template.as_str());
                if collector_only_paths.contains(&path_key) {
                    continue;
                }

                assert!(
                    mcp_paths.contains(&(method.clone(), path_template.clone())),
                    "monitoring REST control-plane path is missing from scoped MCP catalog: {} {}",
                    method.to_ascii_uppercase(),
                    path_template
                );
            }
        }
    }

    #[test]
    fn playbook_control_plane_rest_paths_are_agent_friendly_scoped_mcp_operations() {
        let expected = [
            (
                "playbooks.catalog.list",
                Method::GET,
                "/api/v1/playbooks",
                "read",
                "playbook",
            ),
            (
                "playbooks.catalog.create",
                Method::POST,
                "/api/v1/playbooks",
                "create",
                "playbook",
            ),
            (
                "playbooks.catalog.get",
                Method::GET,
                "/api/v1/playbooks/{id}",
                "read",
                "playbook",
            ),
            (
                "playbooks.catalog.update",
                Method::PATCH,
                "/api/v1/playbooks/{id}",
                "update",
                "playbook",
            ),
            (
                "playbooks.catalog.delete",
                Method::DELETE,
                "/api/v1/playbooks/{id}",
                "delete",
                "playbook",
            ),
            (
                "playbooks.catalog.duplicate",
                Method::POST,
                "/api/v1/playbooks/{id}/duplicate",
                "create",
                "playbook",
            ),
            (
                "playbooks.versions.list",
                Method::GET,
                "/api/v1/playbooks/{id}/versions",
                "read",
                "playbook",
            ),
            (
                "playbooks.versions.create",
                Method::POST,
                "/api/v1/playbooks/{id}/versions",
                "update",
                "playbook",
            ),
            (
                "playbooks.target_sets.list",
                Method::GET,
                "/api/v1/playbooks/target-sets",
                "read",
                "playbook_target_set",
            ),
            (
                "playbooks.target_sets.create",
                Method::POST,
                "/api/v1/playbooks/target-sets",
                "create",
                "playbook_target_set",
            ),
            (
                "playbooks.target_sets.get",
                Method::GET,
                "/api/v1/playbooks/target-sets/{id}",
                "read",
                "playbook_target_set",
            ),
            (
                "playbooks.target_sets.update",
                Method::PUT,
                "/api/v1/playbooks/target-sets/{id}",
                "update",
                "playbook_target_set",
            ),
            (
                "playbooks.target_sets.delete",
                Method::DELETE,
                "/api/v1/playbooks/target-sets/{id}",
                "delete",
                "playbook_target_set",
            ),
            (
                "playbooks.target_sets.preview",
                Method::GET,
                "/api/v1/playbooks/target-sets/{id}/preview",
                "read",
                "playbook_target_set",
            ),
            (
                "playbooks.assignments.list",
                Method::GET,
                "/api/v1/playbooks/assignments",
                "read",
                "playbook_assignment",
            ),
            (
                "playbooks.assignments.create",
                Method::POST,
                "/api/v1/playbooks/assignments",
                "create",
                "playbook_assignment",
            ),
            (
                "playbooks.assignments.get",
                Method::GET,
                "/api/v1/playbooks/assignments/{id}",
                "read",
                "playbook_assignment",
            ),
            (
                "playbooks.assignments.update",
                Method::PATCH,
                "/api/v1/playbooks/assignments/{id}",
                "update",
                "playbook_assignment",
            ),
            (
                "playbooks.assignments.delete",
                Method::DELETE,
                "/api/v1/playbooks/assignments/{id}",
                "delete",
                "playbook_assignment",
            ),
            (
                "playbooks.assignments.list_for_target",
                Method::GET,
                "/api/v1/playbooks/targets/{target_id}/assignments",
                "read",
                "playbook_assignment",
            ),
            (
                "playbooks.run_groups.queue",
                Method::POST,
                "/api/v1/playbooks/{id}/runs",
                "execute",
                "playbook",
            ),
            (
                "playbooks.schedules.list",
                Method::GET,
                "/api/v1/playbooks/schedules",
                "read",
                "playbook_schedule",
            ),
            (
                "playbooks.schedules.create",
                Method::POST,
                "/api/v1/playbooks/schedules",
                "create",
                "playbook_schedule",
            ),
            (
                "playbooks.schedules.get",
                Method::GET,
                "/api/v1/playbooks/schedules/{id}",
                "read",
                "playbook_schedule",
            ),
            (
                "playbooks.run_artifacts.list",
                Method::GET,
                "/api/v1/playbooks/runs/{run_id}/artifacts",
                "read",
                "playbook_run",
            ),
            (
                "playbooks.run_events.list",
                Method::GET,
                "/api/v1/playbooks/runs/{run_id}/events",
                "read",
                "playbook_run",
            ),
            (
                "playbooks.schedules.update",
                Method::PATCH,
                "/api/v1/playbooks/schedules/{id}",
                "update",
                "playbook_schedule",
            ),
            (
                "playbooks.schedules.delete",
                Method::DELETE,
                "/api/v1/playbooks/schedules/{id}",
                "delete",
                "playbook_schedule",
            ),
            // Service-principal endpoints are intentionally NOT in this
            // scoped-visible list: they are machine identities excluded from the
            // human PAT scope catalog (see playbooks_service_principal_security),
            // so they are full-control-only — asserted separately below.
            (
                "playbooks.runs.list",
                Method::GET,
                "/api/v1/playbooks/runs",
                "read",
                "playbook_run",
            ),
            (
                "playbooks.runs.get",
                Method::GET,
                "/api/v1/playbooks/runs/{run_id}",
                "read",
                "playbook_run",
            ),
            (
                "playbooks.runs.cancel",
                Method::POST,
                "/api/v1/playbooks/runs/{run_id}/cancel",
                "execute",
                "playbook",
            ),
            (
                "playbooks.run_groups.list",
                Method::GET,
                "/api/v1/playbooks/run-groups",
                "read",
                "playbook_run_group",
            ),
            (
                "playbooks.run_groups.get",
                Method::GET,
                "/api/v1/playbooks/run-groups/{id}",
                "read",
                "playbook_run_group",
            ),
            (
                "playbooks.run_groups.cancel",
                Method::POST,
                "/api/v1/playbooks/run-groups/{group_id}/cancel",
                "execute",
                "playbook",
            ),
        ];

        for (operation_id, method, path_template, action, resource_type) in expected {
            let endpoint = mcp_rest_endpoint_for_operation_id(operation_id)
                .unwrap_or_else(|| panic!("missing playbook MCP operation {operation_id}"));
            assert_eq!(endpoint.method, method, "{operation_id} method");
            assert_eq!(endpoint.path_template, path_template, "{operation_id} path");
            assert_eq!(endpoint.action, action, "{operation_id} action");
            assert_eq!(
                endpoint.resource_type, resource_type,
                "{operation_id} resource_type"
            );
            assert!(
                !endpoint.full_control_only,
                "{operation_id} must be visible through scoped tokens, not only full-control fallback"
            );

            let visible = visible_scoped_rest_endpoints(&context(action, resource_type, "*"));
            assert!(
                visible
                    .iter()
                    .any(|endpoint| endpoint.operation_id == operation_id),
                "{operation_id} must be visible to a scoped {action}:{resource_type} token"
            );
        }

        // Service principals are machine identities kept out of the human PAT
        // scope catalog, so their MCP endpoints must be full-control-only and
        // invisible to a scoped `*:playbook_service_principal` token.
        for operation_id in [
            "playbooks.service_principals.list",
            "playbooks.service_principals.create",
            "playbooks.service_principals.get",
            "playbooks.service_principals.revoke",
        ] {
            let endpoint = mcp_rest_endpoint_for_operation_id(operation_id)
                .unwrap_or_else(|| panic!("missing playbook MCP operation {operation_id}"));
            assert!(
                endpoint.full_control_only,
                "{operation_id} must be full-control-only (service principals are not scoped-grantable)"
            );
            let scoped = visible_scoped_rest_endpoints(&context(
                endpoint.action,
                endpoint.resource_type,
                "*",
            ));
            assert!(
                !scoped
                    .iter()
                    .any(|endpoint| endpoint.operation_id == operation_id),
                "{operation_id} must not be visible to a scoped service-principal token"
            );
        }
    }

    #[test]
    fn playbook_job_token_callbacks_are_not_agent_friendly_scoped_mcp_operations() {
        for endpoint in mcp_rest_endpoints() {
            if endpoint.path_template == "/api/v1/playbooks/runs/{run_id}/completion" {
                assert!(
                    endpoint.full_control_only,
                    "completion callback must not be exposed as a normal scoped MCP operation"
                );
            }
            if matches!(
                endpoint.path_template.as_str(),
                "/api/v1/playbooks/runs/{run_id}/artifacts"
                    | "/api/v1/playbooks/runs/{run_id}/events"
            ) && endpoint.method == Method::POST
            {
                assert!(
                    endpoint.full_control_only,
                    "job-token callback {} {} must not be exposed as a normal scoped MCP operation",
                    endpoint.method, endpoint.path_template
                );
            }
        }
    }

    #[test]
    fn full_control_scoped_mcp_catalog_covers_openapi_rest_control_plane() {
        let yaml: serde_json::Value =
            serde_yml::from_str(include_str!("../../../contracts/piping.openapi.yaml"))
                .expect("valid OpenAPI yaml");
        let paths = yaml
            .get("paths")
            .and_then(serde_json::Value::as_object)
            .expect("the OpenAPI paths object");
        let request_context = full_context();
        let mcp_paths = visible_scoped_rest_endpoints(&request_context)
            .into_iter()
            .map(|endpoint| {
                (
                    endpoint.method.as_str().to_ascii_lowercase(),
                    endpoint.path_template,
                )
            })
            .collect::<HashSet<_>>();

        for (path_template, path_spec) in paths {
            let methods = path_spec
                .as_object()
                .expect("an OpenAPI path spec object")
                .keys()
                .filter(|method| {
                    method_from_str(method).is_ok()
                        && openapi_rest_fallback_path_allowed(method, path_template)
                });

            for method in methods {
                assert!(
                    mcp_paths.contains(&(method.clone(), path_template.clone())),
                    "full-control scoped MCP catalog is missing OpenAPI REST path: {} {}",
                    method.to_ascii_uppercase(),
                    path_template
                );
            }
        }
    }

    #[test]
    fn openapi_rest_fallback_endpoints_are_hidden_from_scoped_tokens() {
        let request_context = context("read", "inventory", "inventory_entity:devices");
        let endpoints = visible_scoped_rest_endpoints(&request_context);

        assert!(
            endpoints
                .iter()
                .all(|endpoint| !endpoint.operation_id.starts_with("rest.openapi.")),
            "OpenAPI fallback endpoints require full-control token scope"
        );
    }

    #[test]
    fn scoped_rest_endpoint_catalog_excludes_internal_protocol_routes() {
        for endpoint in mcp_rest_endpoints() {
            for blocked in super::MCP_REST_BLOCKED_PREFIXES {
                assert!(
                    endpoint.path_template != *blocked
                        && !endpoint
                            .path_template
                            .starts_with(format!("{blocked}/").as_str()),
                    "internal route leaked into MCP scoped REST catalog: {} {}",
                    endpoint.method,
                    endpoint.path_template
                );
            }
            for blocked in super::MCP_REST_BLOCKED_EXACT_PATHS {
                assert_ne!(
                    endpoint.path_template, *blocked,
                    "collector route leaked into MCP scoped REST catalog: {} {}",
                    endpoint.method, endpoint.path_template
                );
            }
        }

        let full_control_paths = visible_scoped_rest_endpoints(&full_context())
            .into_iter()
            .map(|endpoint| endpoint.path_template)
            .collect::<HashSet<_>>();
        assert!(full_control_paths.contains("/api/v1/admin/auth/oidc/settings"));
        assert!(full_control_paths.contains("/api/v1/monitoring/sd/preview"));
        assert!(full_control_paths.contains("/api/v1/connections/sessions"));
        assert!(full_control_paths.contains("/api/v1/webhooks"));
        assert!(!full_control_paths.contains("/api/v1/auth/login"));
        assert!(!full_control_paths.contains("/api/v1/webhooks/netbox/{provider_id}"));
        assert!(!full_control_paths.contains("/api/v1/monitoring/sd"));
    }

    /// Every CCTV media path must stay out of the generic REST fallback catalog,
    /// including for a full-control token.
    ///
    /// Enumerated from the catalog itself rather than from a hand-written list,
    /// so a media route added later fails this test instead of silently becoming
    /// callable. A generic caller cannot consume these bodies at all: the binary
    /// ones would be JSON-parsed into garbage, and the streaming ones pin a
    /// media-plane slot and a slice of the shared budget for the whole call while
    /// returning nothing usable — repeat calls starve the plane for real viewers.

    #[test]
    fn mcp_rest_catalog_excludes_every_cctv_media_path() {
        const MEDIA_SUFFIXES: &[&str] = &["/snapshot", "/export", "/stream"];

        let full_control_paths = visible_scoped_rest_endpoints(&full_context())
            .into_iter()
            .map(|endpoint| endpoint.path_template)
            .collect::<HashSet<_>>();

        use utoipa::OpenApi as _;
        let media_paths = crate::api::openapi::ApiDoc::openapi()
            .paths
            .paths
            .keys()
            .filter(|path| path.starts_with("/api/v1/cctv/"))
            .filter(|path| MEDIA_SUFFIXES.iter().any(|suffix| path.ends_with(suffix)))
            .cloned()
            .collect::<Vec<_>>();

        assert!(
            media_paths.len() >= MEDIA_SUFFIXES.len(),
            "expected every CCTV media path in the contract, found {media_paths:?}"
        );
        for path in media_paths {
            assert!(
                super::MCP_REST_BLOCKED_EXACT_PATHS.contains(&path.as_str()),
                "CCTV media path {path} is not in MCP_REST_BLOCKED_EXACT_PATHS"
            );
            assert!(
                !full_control_paths.contains(&path),
                "CCTV media path {path} leaked into the generic REST catalog"
            );
        }
    }
}
