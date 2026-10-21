import { tabConfig } from "@/features/inventory/components/inventory/inventoryTypes";
import { inventoryResourceChildRelations, inventoryResourceChildTableId } from "@/features/inventory/components/inventory/inventoryResourceChildLists";

export const INVENTORY_RESOURCE_TABLE_IDS = tabConfig.map(
  (tab) => `inventory.${tab.key}`,
);

export const INVENTORY_RESOURCE_TABLE_OWNER_FILE =
  "features/inventory/InventoryResources.tsx" as const;

export const INVENTORY_RESOURCE_API_IMPORT =
  "@/features/inventory/api" as const;

export const INVENTORY_CHILD_TABLE_OWNER_FILE =
  "features/inventory/components/inventory/InventoryResourceChildren.tsx" as const;

// This is a server-backed projection family of existing inventory resources,
// not new backend registries or the wider standalone inventory table columns.
export const INVENTORY_CHILD_TABLE_CONTRACTS = tabConfig.flatMap(parent =>
  inventoryResourceChildRelations(parent.key).map(relation => ({
    tableId: inventoryResourceChildTableId(parent.key, relation),
    parentEntity: parent.key,
    childEntity: relation.childEntity,
    listRoute: relation.listRoute,
  })),
);
export const INVENTORY_CHILD_TABLE_IDS = INVENTORY_CHILD_TABLE_CONTRACTS.map(contract => contract.tableId);

export const BACKEND_ONLY_INVENTORY_REGISTRIES = [
  {
    registry: "console_ports",
    reason: "device-detail child relation without a standalone user-facing table",
  },
  {
    registry: "console_server_ports",
    reason: "device-detail child relation without a standalone user-facing table",
  },
  {
    registry: "content_types",
    reason: "internal metadata option source without a user-facing table",
  },
  {
    registry: "device_bays",
    reason: "device-detail child relation without a standalone user-facing table",
  },
  {
    registry: "front_ports",
    reason: "device-detail child relation without a standalone user-facing table",
  },
  {
    registry: "ipam_roles",
    reason: "option-source taxonomy rendered inside other inventory table filters",
  },
  {
    registry: "module_bays",
    reason: "module/device-detail child relation without a standalone user-facing table",
  },
  {
    registry: "power_outlets",
    reason: "device-detail child relation without a standalone user-facing table",
  },
  {
    registry: "power_ports",
    reason: "device-detail child relation without a standalone user-facing table",
  },
  {
    registry: "rear_ports",
    reason: "device-detail child relation without a standalone user-facing table",
  },
  {
    registry: "vlan_groups",
    reason: "option-source taxonomy rendered inside VLAN/IPAM filters",
  },
] as const;

export const BACKEND_ONLY_NON_RESOURCE_REGISTRIES = [
  {
    registry: "connection_templates",
    route: "/api/v1/connections/templates",
    reason:
      "API option/source route for connection creation; there is no dedicated user-facing DataTable surface",
  },
  {
    registry: "playbook_runs",
    route: "/api/v1/playbooks/runs",
    reason:
      "individual run history/detail API; the user-facing runs table is grouped by /api/v1/playbooks/run-groups",
  },
] as const;

export const NON_RESOURCE_TABLE_IDS = [
  "access.users",
  "access.groups",
  "access.roles",
  "access.grants",
  "access.policies",
  "audit.log",
  "automation-secrets.list",
  "backup.preview",
  "connection-config.assignments",
  "connection-config.records",
  "connection-config.effective-preview",
  "connections.active",
  "connections.proxies",
  "connections.proxyAssignments",
  "connections.targets.desktop",
  "connections.targets.mobile",
  "inventory.config-context-bindings",
  "inventory.config-contexts",
  "inventory.customFields.choiceSets",
  "inventory.customFields.definitions",
  "inventory.ipamWorkspace.prefixes",
  "inventory.syncProviders",
  "inventory.syncMappings",
  "inventory.templateRuns",
  "inventory.templates",
  "observability.exporters",
  "observability.inspector.alert_assignments",
  "observability.inspector.alert_sources",
  "observability.inspector.metric_cards",
  "observability.inspector.metric_groups",
  "observability.inspector.profiles",
  "observability.inspector.sources",
  "playbooks.assignments",
  "playbooks.executionEnvironments",
  "playbooks.jobTemplates",
  "playbooks.projects",
  "playbooks.runs",
  "playbooks.schedules",
  "playbooks.servicePrincipals",
  "playbooks.targetSets",
  "userSettings.credentials",
  "userSettings.tokens",
  "webhooks.list",
] as const;

export type NonResourceTableId = (typeof NON_RESOURCE_TABLE_IDS)[number];

export interface NonResourceTableContract {
  ownerFile: string;
  mode: "server" | "client-only";
  apiImports?: readonly string[];
  listRoute?: string;
  forbiddenListRoutes?: readonly string[];
  backendRegistry?: string;
}

export const NON_RESOURCE_TABLE_CONTRACTS: Record<
  NonResourceTableId,
  NonResourceTableContract
> = {
  "access.users": {
    ownerFile: "features/access-control/components/access/AccessUsers.tsx",
    mode: "server",
    apiImports: ["@/features/access-control/api"],
    listRoute: "/api/v1/admin/access-control/users",
    backendRegistry: "access_users",
  },
  "access.groups": {
    ownerFile: "features/access-control/components/access/AccessGroups.tsx",
    mode: "server",
    apiImports: ["@/features/access-control/api"],
    listRoute: "/api/v1/admin/access-control/groups",
    backendRegistry: "access_groups",
  },
  "access.roles": {
    ownerFile: "features/access-control/components/access/AccessRoles.tsx",
    mode: "server",
    apiImports: ["@/features/access-control/api"],
    listRoute: "/api/v1/admin/access-control/roles",
    backendRegistry: "access_roles",
  },
  "access.grants": {
    ownerFile: "features/access-control/components/access/AccessGrants.tsx",
    mode: "server",
    apiImports: ["@/features/access-control/api"],
    listRoute: "/api/v1/admin/access-control/role-assignments",
    backendRegistry: "access_role_assignments",
  },
  "access.policies": {
    ownerFile: "features/access-control/components/access/AccessPolicies.tsx",
    mode: "server",
    apiImports: ["@/features/access-control/api"],
    listRoute: "/api/v1/admin/access-control/policies",
    backendRegistry: "access_policies",
  },
  "audit.log": {
    ownerFile: "features/audit/AuditLog.tsx",
    mode: "server",
    apiImports: ["@/features/audit/api"],
    listRoute: "/api/v1/audit/events",
    backendRegistry: "audit_events",
  },
  // The automation-secrets list endpoint returns the full (small) set without
  // pagination; the page filters and sorts the rows client-side.
  "automation-secrets.list": {
    ownerFile: "features/automation-secrets/AutomationSecretsPage.tsx",
    mode: "client-only",
  },
  // The restore preview renders the in-memory dry-run plan built from the
  // uploaded archive and the chosen restore rules; it has no server list route.
  "backup.preview": {
    ownerFile: "features/backup/BackupPage.tsx",
    mode: "client-only",
  },
  "connection-config.assignments": {
    ownerFile: "features/connections/connection-config/AssignmentsPanel.tsx",
    mode: "server",
    apiImports: ["./api"],
    listRoute: "/api/v1/connection-config/assignments",
    backendRegistry: "connection_config_assignments",
  },
  "connection-config.records": {
    ownerFile: "features/connections/connection-config/ConnectionConfig.tsx",
    mode: "server",
    apiImports: ["./api"],
    listRoute: "/api/v1/connection-config/records",
    backendRegistry: "connection_config_records",
  },
  "connection-config.effective-preview": {
    ownerFile: "features/connections/connection-config/EffectivePreview.tsx",
    mode: "client-only",
  },
  "connections.active": {
    ownerFile: "features/connections/ActiveConnections.tsx",
    mode: "server",
    apiImports: ["@/features/connections/api"],
    listRoute: "/api/v1/connections/sessions",
    backendRegistry: "connection_sessions",
  },
  "connections.proxies": {
    ownerFile: "features/connections/ProxyNodes.tsx",
    mode: "server",
    apiImports: ["@/features/connections/proxies-api"],
    listRoute: "/api/v1/admin/proxy-nodes",
    backendRegistry: "proxy_nodes",
  },
  "connections.proxyAssignments": {
    ownerFile: "features/connections/ProxyNodes.tsx",
    mode: "server",
    apiImports: ["@/features/connections/proxies-api"],
    listRoute: "/api/v1/admin/proxy-node-assignments",
    backendRegistry: "proxy_node_assignments",
  },
  "connections.targets.desktop": {
    ownerFile: "features/connections/ConnectionsWorkspace.tsx",
    mode: "server",
    apiImports: ["@/features/connections/api"],
    listRoute: "/api/v1/connections/targets",
    backendRegistry: "connection_targets",
  },
  "connections.targets.mobile": {
    ownerFile: "features/connections/ConnectionsWorkspace.tsx",
    mode: "server",
    apiImports: ["@/features/connections/api"],
    listRoute: "/api/v1/connections/targets",
    backendRegistry: "connection_targets",
  },
  "inventory.config-context-bindings": {
    ownerFile: "features/inventory/components/config-contexts/ConfigContextScopeBindings.tsx",
    mode: "server",
    apiImports: ["@/features/inventory/config-contexts-api"],
    listRoute: "/api/v1/config-context-bindings",
    backendRegistry: "config_context_bindings",
  },
  "inventory.config-contexts": {
    ownerFile: "features/inventory/ConfigContexts.tsx",
    mode: "server",
    apiImports: ["@/features/inventory/config-contexts-api"],
    listRoute: "/api/v1/config-contexts",
    backendRegistry: "config_contexts",
  },
  "inventory.customFields.choiceSets": {
    ownerFile: "features/inventory/CustomFields.tsx",
    mode: "server",
    apiImports: ["@/features/inventory/custom-fields-api"],
    listRoute: "/api/v1/custom-field-choice-sets",
    backendRegistry: "custom_field_choice_sets",
  },
  "inventory.customFields.definitions": {
    ownerFile: "features/inventory/CustomFields.tsx",
    mode: "server",
    apiImports: ["@/features/inventory/custom-fields-api"],
    listRoute: "/api/v1/custom-fields",
    backendRegistry: "custom_field_definitions",
  },
  "inventory.ipamWorkspace.prefixes": {
    ownerFile: "features/inventory/components/ipam-workspace/IpamWorkspace.tsx",
    mode: "client-only",
  },
  "inventory.syncProviders": {
    ownerFile: "features/inventory/SyncPage.tsx",
    mode: "server",
    apiImports: ["@/features/inventory/sync-api"],
    listRoute: "/api/v1/providers",
    backendRegistry: "provider_sources",
  },
  "inventory.syncMappings": {
    ownerFile: "features/inventory/SyncMappingList.tsx",
    mode: "server",
    apiImports: ["@/features/inventory/sync-mapping-api"],
    listRoute: "/api/v1/sync/mapping-configurations",
    forbiddenListRoutes: ["/api/v1/providers/{id}/mapping-configurations"],
    backendRegistry: "mapping_configurations",
  },
  "inventory.templateRuns": {
    ownerFile: "features/inventory/InventoryTemplates.tsx",
    mode: "server",
    apiImports: ["@/features/inventory/api"],
    listRoute: "/api/v1/inventory/template-runs",
    backendRegistry: "inventory_template_runs",
  },
  "inventory.templates": {
    ownerFile: "features/inventory/InventoryTemplates.tsx",
    mode: "server",
    apiImports: ["@/features/inventory/api"],
    listRoute: "/api/v1/inventory/templates",
    backendRegistry: "inventory_templates",
  },
  "observability.exporters": {
    ownerFile: "features/observability/ServiceDiscoveryScreen.tsx",
    // Server-backed complete-list projection. The collection applies URL
    // filters/sort/page locally; these describe the API/schema, not page requests.
    mode: "server",
    apiImports: ["./api"],
    listRoute: "/api/v1/monitoring/exporter-assignments",
    backendRegistry: "observability_exporter_assignments",
  },
  // Inspector metric workbench tables render in-memory query/profile data with
  // client-side sorting (no server list route / pagination), like the
  // effective-preview table — client-only contract, no shared sort-state.
  "observability.inspector.alert_assignments": {
    ownerFile: "features/observability/InspectorMetricsScreen.tsx",
    mode: "client-only",
  },
  "observability.inspector.alert_sources": {
    ownerFile: "features/observability/InspectorMetricsScreen.tsx",
    mode: "client-only",
  },
  "observability.inspector.metric_cards": {
    ownerFile: "features/observability/InspectorMetricsScreen.tsx",
    mode: "client-only",
  },
  "observability.inspector.metric_groups": {
    ownerFile: "features/observability/InspectorMetricsScreen.tsx",
    mode: "client-only",
  },
  "observability.inspector.profiles": {
    ownerFile: "features/observability/InspectorMetricsScreen.tsx",
    mode: "client-only",
  },
  "observability.inspector.sources": {
    ownerFile: "features/observability/InspectorMetricsScreen.tsx",
    mode: "client-only",
  },
  "playbooks.assignments": {
    ownerFile: "features/playbooks/AssignmentsPanel.tsx",
    mode: "server",
    apiImports: ["@/features/playbooks/api"],
    listRoute: "/api/v1/playbooks/assignments",
    backendRegistry: "playbook_assignments",
  },
  "playbooks.executionEnvironments": {
    ownerFile: "features/playbooks/ExecutionEnvironmentsPanel.tsx",
    mode: "client-only",
  },
  "playbooks.jobTemplates": {
    ownerFile: "features/playbooks/JobTemplatesPanel.tsx",
    mode: "client-only",
  },
  "playbooks.projects": {
    ownerFile: "features/playbooks/ProjectsPanel.tsx",
    mode: "client-only",
  },
  "playbooks.runs": {
    ownerFile: "features/playbooks/RunsPanel.tsx",
    mode: "server",
    apiImports: ["@/features/playbooks/api"],
    listRoute: "/api/v1/playbooks/run-groups",
    backendRegistry: "playbook_run_groups",
  },
  "playbooks.schedules": {
    ownerFile: "features/playbooks/SchedulesPanel.tsx",
    mode: "server",
    apiImports: ["@/features/playbooks/api"],
    listRoute: "/api/v1/playbooks/schedules",
    backendRegistry: "playbook_schedules",
  },
  "playbooks.servicePrincipals": {
    ownerFile: "features/playbooks/ServicePrincipalsPanel.tsx",
    mode: "server",
    apiImports: ["@/features/playbooks/api"],
    listRoute: "/api/v1/playbooks/service-principals",
    backendRegistry: "playbook_service_principals",
  },
  "playbooks.targetSets": {
    ownerFile: "features/playbooks/TargetSetsPanel.tsx",
    mode: "server",
    apiImports: ["@/features/playbooks/api"],
    listRoute: "/api/v1/playbooks/target-sets",
    backendRegistry: "playbook_target_sets",
  },
  "userSettings.credentials": {
    ownerFile: "features/user-settings/UserSettings.tsx",
    mode: "server",
    apiImports: ["@/features/user-settings/api"],
    listRoute: "/api/v1/user/settings/credentials",
    backendRegistry: "user_credential_sets",
  },
  "userSettings.tokens": {
    ownerFile: "features/user-settings/UserSettings.tsx",
    mode: "server",
    apiImports: ["@/features/user-settings/api"],
    listRoute: "/api/v1/user/settings/api-tokens",
    backendRegistry: "user_api_tokens",
  },
  "webhooks.list": {
    ownerFile: "features/webhooks/WebhooksPage.tsx",
    mode: "server",
    apiImports: ["@/features/webhooks/api"],
    listRoute: "/api/v1/webhooks",
    backendRegistry: "webhook_subscriptions",
  },
};

export const CLIENT_ONLY_STANDARDIZED_TABLE_IDS = NON_RESOURCE_TABLE_IDS.filter(
  (tableId) => NON_RESOURCE_TABLE_CONTRACTS[tableId].mode === "client-only",
);

export const STANDARDIZED_TABLE_IDS = [
  ...new Set([...INVENTORY_RESOURCE_TABLE_IDS, ...INVENTORY_CHILD_TABLE_IDS, ...NON_RESOURCE_TABLE_IDS]),
].sort((a, b) => a.localeCompare(b));
