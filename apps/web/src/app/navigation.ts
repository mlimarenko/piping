import type { ElementType } from "react";
import {
  Monitor, Server, Shield, Settings, User, Plug, Sliders, Activity, Radio,
  RefreshCw, Layers, Database, FileText, Archive, Users, FileCheck, Lock,
  ScrollText, CreditCard, Webhook, Palette, Network, ClipboardList,
  History, Tags, KeyRound,
} from "lucide-react";
import { PLAYBOOK_TAB_ROUTES } from "@/features/playbooks/playbookNavigation";
import type { PermissionsValue } from "@/shared/auth/permissionsContext";

type NavGate = (permissions: PermissionsValue) => boolean;
export interface NavItem {
  labelKey: string;
  labelDefault?: string;
  path: string;
  icon: ElementType;
  gate?: NavGate;
  resolvePath?: (permissions: PermissionsValue) => string;
  subheaderKey?: string;
  hidden?: boolean;
  /** Several route families can share one sidebar destination. */
  ownedPaths?: readonly string[];
}
export interface NavSection {
  labelKey: string;
  icon: ElementType;
  basePath: string;
  items: NavItem[];
}

// Setup pill order determines the first readable landing.
const SETUP_SURFACES = [
  { resource: "PLAYBOOK_PROJECT", path: PLAYBOOK_TAB_ROUTES.projects },
  { resource: "PLAYBOOK_JOB_TEMPLATE", path: PLAYBOOK_TAB_ROUTES["job-templates"] },
  { resource: "PLAYBOOK_EXECUTION_ENVIRONMENT", path: PLAYBOOK_TAB_ROUTES["execution-environments"] },
  { resource: "PLAYBOOK_ASSIGNMENT", path: PLAYBOOK_TAB_ROUTES.assignments },
  { resource: "PLAYBOOK_TARGET_SET", path: PLAYBOOK_TAB_ROUTES["target-sets"] },
  { resource: "PLAYBOOK_SCHEDULE", path: PLAYBOOK_TAB_ROUTES.schedules },
  { resource: "PLAYBOOK_SERVICE_PRINCIPAL", path: PLAYBOOK_TAB_ROUTES["service-principals"] },
] as const;

export const navigationSections: NavSection[] = [
  { labelKey: "nav.connections", icon: Monitor, basePath: "/connections", items: [
    { labelKey: "navItems.workspace", path: "/connections", icon: Plug, gate: p => p.can("READ", "CONNECTION_TARGET") },
    { labelKey: "navItems.connectionConfig", path: "/connections/config", icon: Sliders, gate: p => p.can("READ", "CONFIG_CONTEXT") },
    { labelKey: "navItems.active", path: "/connections/active", icon: Activity, gate: p => p.can("READ", "CONNECTION_SESSION") },
  ] },
  { labelKey: "nav.inventory", icon: Server, basePath: "/inventory", items: [
    { labelKey: "navItems.resources", path: "/inventory/resources", icon: Database, gate: p => p.can("READ", "INVENTORY") },
    { labelKey: "dataflows.title", path: "/inventory/dataflows", icon: Network, gate: p => p.can("READ", "INVENTORY") },
    { labelKey: "navItems.templates", path: "/inventory/templates", icon: FileText, gate: p => p.can("READ", "INVENTORY") },
    { labelKey: "navItems.customFields", path: "/inventory/custom-fields", icon: Tags, gate: p => p.can("READ", "INVENTORY") },
    { labelKey: "navItems.sync", path: "/inventory/sync", icon: RefreshCw, gate: p => p.can("READ", "INVENTORY") },
  ] },
  { labelKey: "nav.automation", icon: ClipboardList, basePath: "/automation", items: [
    { labelKey: "navItems.playbooks", path: PLAYBOOK_TAB_ROUTES.catalog, icon: ClipboardList, gate: p => p.can("READ", "PLAYBOOK") },
    { labelKey: "navItems.runs", path: PLAYBOOK_TAB_ROUTES.runs, icon: History, gate: p => p.can("READ", "PLAYBOOK_RUN_GROUP") },
    { labelKey: "navItems.automationSecrets", path: "/automation/secrets", icon: KeyRound, gate: p => p.can("READ", "SECRET_VARIABLE") },
    { labelKey: "navItems.automationSetup", path: PLAYBOOK_TAB_ROUTES.projects, icon: Layers,
      gate: p => SETUP_SURFACES.some(s => p.can("READ", s.resource)),
      // Called only after the same capability gate passes.
      resolvePath: p => SETUP_SURFACES.find(s => p.can("READ", s.resource))!.path,
      ownedPaths: SETUP_SURFACES.map(s => s.path) },
  ] },
  { labelKey: "nav.accessControl", icon: Shield, basePath: "/access-control", items: [
    { labelKey: "navItems.accessRules", path: "/access-control?tab=access", icon: FileCheck, gate: p => p.can("READ", "POLICY") },
    { labelKey: "navItems.directory", path: "/access-control?tab=directory", icon: Users, gate: p => p.can("READ", "USER") || p.can("READ", "GROUP") || p.can("READ", "ROLE") },
    { labelKey: "navItems.sso", path: "/access-control?tab=sso", icon: Lock, gate: p => p.can("READ", "KEYCLOAK") },
  ] },
  { labelKey: "nav.administration", icon: Settings, basePath: "/administration", items: [
    { labelKey: "navItems.observability", path: "/observability", icon: Activity, subheaderKey: "navGroups.monitoring", gate: p => p.can("READ", "MONITORING") },
    { labelKey: "navItems.auditLog", path: "/audit-logs", icon: ScrollText, gate: p => p.can("VIEW_AUDIT", "AUDIT_EVENT") },
    { labelKey: "navItems.webhooks", path: "/administration/webhooks", icon: Webhook, subheaderKey: "navGroups.integrations", gate: p => p.can("READ", "WEBHOOK") },
    { labelKey: "navItems.mcp", path: "/mcp", icon: Network, gate: p => p.isAdmin },
    { labelKey: "navItems.proxyNodes", path: "/connections/proxies", icon: Radio, gate: p => p.can("READ", "PROXY_NODE") },
    { labelKey: "navItems.instanceSettings", path: "/administration/settings", icon: Settings, subheaderKey: "navGroups.system", gate: p => p.can("READ", "SETTINGS") },
    { labelKey: "navItems.backup", path: "/administration/backup", icon: Archive, gate: p => p.can("READ", "BACKUP") },
    { labelKey: "navItems.license", path: "/administration/license", icon: CreditCard, gate: p => p.can("READ", "LICENSE") },
    { labelKey: "navItems.branding", path: "/administration/branding", icon: Palette, gate: p => p.canAny("BRANDING", ["READ", "UPDATE"]) },
    { labelKey: "apiDocs.title", path: "/api-docs", icon: FileText, hidden: true },
  ] },
  { labelKey: "nav.account", icon: User, basePath: "/user", items: [
    { labelKey: "navItems.settings", path: "/user/settings", icon: User },
  ] },
];

export function visibleNavigation(permissions: PermissionsValue): NavSection[] {
  return navigationSections.map(section => ({
    ...section,
    items: section.items.filter(item => !item.hidden && (!item.gate || item.gate(permissions)))
      .map(item => item.resolvePath ? { ...item, path: item.resolvePath(permissions) } : item),
  })).filter(section => section.items.length > 0);
}

function ownsPath(path: string, pathname: string): boolean {
  return pathname === path || pathname.startsWith(`${path}/`);
}

/** Resolve ownership before visibility, so a denied proxy never lights Connections. */
export function resolveNavigation(pathname: string, search = "") {
  const query = new URLSearchParams(search);
  const candidates = navigationSections.flatMap(section => section.items.flatMap(item =>
    (item.ownedPaths ?? [item.path]).flatMap(destination => {
      const [path, params] = destination.split("?");
      if (!ownsPath(path!, pathname)) return [];
      if (params && !Array.from(new URLSearchParams(params)).every(([key, value]) => query.get(key) === value)) return [];
      return [{ section, item, specificity: destination.length }];
    }),
  ));
  const owner = candidates.sort((a, b) => b.specificity - a.specificity)[0];
  if (owner) return { section: owner.section, item: owner.item };
  return { section: navigationSections.find(section => ownsPath(section.basePath, pathname)), item: undefined };
}
