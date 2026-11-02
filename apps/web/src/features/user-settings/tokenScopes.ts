import type { TokenScopeCatalogItem } from "@/features/user-settings/api";

/**
 * Pure scope-pack helpers for the Create API token form.
 *
 * Packs and per-resource groups are derived dynamically from the runtime scope
 * catalog (the backend list can drift), never hardcoded. Every helper here is
 * side-effect free so it can be unit-tested in isolation and reused by the
 * dialog without dragging React state along.
 */

/** Actions that are considered "read-only" for the read-everything pack and
 *  the per-resource "Read" quick toggle. */
const READ_ACTIONS = new Set(["read", "observe", "view_audit"]);

/** Domain section each resource type belongs to, keyed by the lowercase wire
 *  resource_type. The i18n key is `accessControl.policies.actionGroups.<domain>`
 *  so section titles localize with the same dictionary the policy editor uses.
 *  Anything unmapped falls into "admin". */
const RESOURCE_DOMAIN: Record<string, string> = {
  connection_target: "connections",
  connection_session: "connections",
  connection_filter_preset: "connections",
  cctv: "connections",
  inventory: "inventory",
  config_context: "inventory",
  monitoring: "monitoring",
  playbook: "playbooks",
  playbook_version: "playbooks",
  playbook_assignment: "playbooks",
  playbook_target_set: "playbooks",
  playbook_run_group: "playbooks",
  playbook_run: "playbooks",
  playbook_schedule: "playbooks",
  playbook_artifact: "playbooks",
  playbook_service_principal: "playbooks",
  playbook_execution_environment: "playbooks",
  playbook_project: "playbooks",
  playbook_job_template: "playbooks",
  secret_variable: "secrets",
  assistant: "assistant",
  assistant_conversation: "assistant",
  audit_event: "audit",
  user: "admin",
  group: "admin",
  role: "admin",
  policy: "admin",
  keycloak: "admin",
  webhook: "admin",
  settings: "admin",
  branding: "admin",
  license: "admin",
  proxy_node: "admin",
  backup: "admin",
};

/** Ordered domain sections for the token scope picker. */
export const RESOURCE_DOMAIN_ORDER = [
  "connections",
  "inventory",
  "monitoring",
  "playbooks",
  "secrets",
  "assistant",
  "admin",
  "audit",
] as const;

/** The domain-section key for a resource type (defaults to "admin"). */
export function resourceDomain(resourceType: string): string {
  return RESOURCE_DOMAIN[resourceType] ?? "admin";
}

/** Stable display ordering for resource groups in the checklist. Anything not
 *  listed falls back to alphabetical after the known ones. */
const RESOURCE_ORDER = [
  "connection_target",
  "connection_session",
  "cctv",
  "inventory",
  "config_context",
  "playbook",
  "playbook_version",
  "playbook_assignment",
  "playbook_target_set",
  "playbook_run_group",
  "playbook_run",
  "playbook_schedule",
  "playbook_artifact",
  "secret_variable",
  "webhook",
  "user",
  "group",
  "role",
  "policy",
  "audit_event",
  "keycloak",
];

export interface ScopeGroup {
  resourceType: string;
  scopes: TokenScopeCatalogItem[];
}

export interface ScopePack {
  /** Stable identifier, used as React key and i18n label suffix. */
  id: string;
  /** Concrete scope values (action:resource) this pack toggles, intersected
   *  with the available catalog. Never empty for a returned pack. */
  scopes: string[];
}

/** Group the catalog by resourceType in a stable, human-friendly order. */
export function groupScopesByResource(catalog: TokenScopeCatalogItem[]): ScopeGroup[] {
  const groups = new Map<string, TokenScopeCatalogItem[]>();
  for (const item of catalog) {
    const existing = groups.get(item.resourceType);
    if (existing) existing.push(item);
    else groups.set(item.resourceType, [item]);
  }
  const orderIndex = (resourceType: string): number => {
    const idx = RESOURCE_ORDER.indexOf(resourceType);
    return idx === -1 ? RESOURCE_ORDER.length : idx;
  };
  // Sort by domain section first (so each section's resource cards are
  // contiguous and sections appear in a sensible order), then by the stable
  // per-resource order within a section.
  const domainIndex = (resourceType: string): number => {
    const idx = RESOURCE_DOMAIN_ORDER.indexOf(
      resourceDomain(resourceType) as (typeof RESOURCE_DOMAIN_ORDER)[number],
    );
    return idx === -1 ? RESOURCE_DOMAIN_ORDER.length : idx;
  };
  return [...groups.entries()]
    .sort(([a], [b]) => {
      const byDomain = domainIndex(a) - domainIndex(b);
      if (byDomain !== 0) return byDomain;
      const byOrder = orderIndex(a) - orderIndex(b);
      return byOrder !== 0 ? byOrder : a.localeCompare(b);
    })
    .map(([resourceType, scopes]) => ({ resourceType, scopes }));
}

/** Whether a catalog item is safe for the read-everything pack. CCTV uses a
 * namespaced operation vocabulary, where only view/recordings are read packs;
 * export, users, settings, and PTZ require deliberate selection. */
function isReadScope(scope: TokenScopeCatalogItem): boolean {
  if (scope.action !== "cctv") return READ_ACTIONS.has(scope.action);
  return scope.value === "cctv:view" || scope.value === "cctv:recordings";
}

/** Scope values for the "Read {resource}" quick toggle of one resource type. */
export function readScopeValues(scopes: TokenScopeCatalogItem[]): string[] {
  return scopes.filter(isReadScope).map((scope) => scope.value);
}

/** Scope values for the "Manage {resource}" quick toggle: every action the
 *  catalog exposes for that resource type. */
export function manageScopeValues(scopes: TokenScopeCatalogItem[]): string[] {
  return scopes.map((s) => s.value);
}

/**
 * Compute role-oriented packs from the catalog. A pack is only returned when
 * it intersects at least one available scope, so packs never reference scopes
 * the backend didn't actually offer. Pack `scopes` are de-duplicated and
 * intersected with the catalog.
 */
export function computeScopePacks(catalog: TokenScopeCatalogItem[]): ScopePack[] {
  const available = new Set(catalog.map((s) => s.value));
  const byResource = (resourceType: string): TokenScopeCatalogItem[] =>
    catalog.filter((s) => s.resourceType === resourceType);

  const intersect = (values: string[]): string[] => {
    const seen = new Set<string>();
    const out: string[] = [];
    for (const v of values) {
      if (available.has(v) && !seen.has(v)) {
        seen.add(v);
        out.push(v);
      }
    }
    return out;
  };

  const readOnly = catalog.filter(isReadScope).map((scope) => scope.value);

  const connectionsOperator = [
    ...byResource("connection_target")
      .filter((s) => ["read", "connect", "observe", "execute"].includes(s.action))
      .map((s) => s.value),
    ...manageScopeValues(byResource("connection_session")),
  ];

  const inventoryManager = [
    ...manageScopeValues(byResource("inventory")),
    ...manageScopeValues(byResource("config_context")),
  ];

  const playbookOperator = [
    ...byResource("playbook")
      .filter((s) => ["read", "execute"].includes(s.action))
      .map((s) => s.value),
    ...byResource("playbook_run_group")
      .filter((s) => ["read", "create"].includes(s.action))
      .map((s) => s.value),
    ...readScopeValues(byResource("playbook_run")),
    ...readScopeValues(byResource("playbook_artifact")),
  ];

  const playbookManager = [
    ...manageScopeValues(byResource("playbook")),
    ...manageScopeValues(byResource("playbook_version")),
    ...manageScopeValues(byResource("playbook_assignment")),
    ...manageScopeValues(byResource("playbook_target_set")),
    ...manageScopeValues(byResource("playbook_run_group")),
    ...manageScopeValues(byResource("playbook_run")),
    ...manageScopeValues(byResource("playbook_schedule")),
    ...readScopeValues(byResource("playbook_artifact")),
  ];

  const accessAdmin = [
    ...manageScopeValues(byResource("user")),
    ...manageScopeValues(byResource("group")),
    ...manageScopeValues(byResource("role")),
    ...manageScopeValues(byResource("policy")),
  ];

  const fullAccess = catalog.map((s) => s.value);

  const candidates: ScopePack[] = [
    { id: "readOnly", scopes: intersect(readOnly) },
    { id: "connectionsOperator", scopes: intersect(connectionsOperator) },
    { id: "inventoryManager", scopes: intersect(inventoryManager) },
    { id: "playbookOperator", scopes: intersect(playbookOperator) },
    { id: "playbookManager", scopes: intersect(playbookManager) },
    { id: "accessAdmin", scopes: intersect(accessAdmin) },
    { id: "fullAccess", scopes: intersect(fullAccess) },
  ];

  return candidates.filter((pack) => pack.scopes.length > 0);
}

/** True when every scope value in `values` is present in `selected`. An empty
 *  `values` is never considered active (avoids a no-op pack reading active). */
export function isSubsetSelected(values: string[], selected: Set<string>): boolean {
  return values.length > 0 && values.every((v) => selected.has(v));
}

/**
 * Toggle a bundle of scope values against the current selection.
 *
 * If every value is already selected, the result removes all of them
 * (toggle off). Otherwise the result adds the missing ones (toggle on),
 * preserving everything else already selected. Returns a new ordered array.
 */
export function toggleBundle(values: string[], selected: string[]): string[] {
  const selectedSet = new Set(selected);
  const allActive = isSubsetSelected(values, selectedSet);
  if (allActive) {
    const removing = new Set(values);
    return selected.filter((v) => !removing.has(v));
  }
  const next = [...selected];
  for (const v of values) {
    if (!selectedSet.has(v)) {
      selectedSet.add(v);
      next.push(v);
    }
  }
  return next;
}

/**
 * Minimal scope-row shape the targeting reconcilers operate on. The concrete
 * `TokenScopeRow` carries an `id` too; row creation (and id generation) is
 * injected via a `makeRow` factory so these helpers stay pure and testable.
 */
export interface ScopeRowLike {
  scope: string;
  resourcePattern: string;
}

/** Distinct scope values in first-seen order. A scope may own several rows
 *  (one per targeted entity); this collapses them to the unique scope set. */
export function distinctScopeValues(rows: ScopeRowLike[]): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const r of rows) {
    if (!seen.has(r.scope)) {
      seen.add(r.scope);
      out.push(r.scope);
    }
  }
  return out;
}

/** Resource patterns currently attached to a scope value (one per row). */
export function patternsForScope(rows: ScopeRowLike[], scopeValue: string): string[] {
  return rows.filter((r) => r.scope === scopeValue).map((r) => r.resourcePattern);
}

/**
 * Reconcile the selected scope-value set into rows, preserving ALL existing
 * rows for retained scopes (so multi-entity targeting survives toggling other
 * scopes) and seeding newly added scopes with a single `*` row.
 */
export function reconcileSelectedScopes<R extends ScopeRowLike>(
  rows: R[],
  nextValues: string[],
  makeRow: (scope: string, pattern: string) => R,
): R[] {
  const byScope = new Map<string, R[]>();
  for (const r of rows) {
    const arr = byScope.get(r.scope);
    if (arr) arr.push(r);
    else byScope.set(r.scope, [r]);
  }
  const out: R[] = [];
  for (const value of nextValues) {
    const existing = byScope.get(value);
    if (existing && existing.length) out.push(...existing);
    else out.push(makeRow(value, "*"));
  }
  return out;
}

/**
 * Replace every row for `scopeValue` with one row per pattern (in place,
 * preserving the scope's position in the list). An empty/`*`-only pattern set
 * collapses to a single `*` row ("all resources").
 */
export function setScopeTargetRows<R extends ScopeRowLike>(
  rows: R[],
  scopeValue: string,
  patterns: string[],
  makeRow: (scope: string, pattern: string) => R,
): R[] {
  const specific = patterns.filter((p) => p && p !== "*");
  const finalPatterns = specific.length ? specific : ["*"];
  const newRows = finalPatterns.map((p) => makeRow(scopeValue, p));

  if (!rows.some((r) => r.scope === scopeValue)) {
    return [...rows, ...newRows];
  }
  const result: R[] = [];
  let inserted = false;
  for (const r of rows) {
    if (r.scope === scopeValue) {
      if (!inserted) {
        result.push(...newRows);
        inserted = true;
      }
    } else {
      result.push(r);
    }
  }
  return result;
}

/** Parse a scope value (`action:resource_type`, optionally `@pattern`) into a
 *  catalog entry, or null when it is not a well-formed scope. */
export function tokenScopeCatalogEntry(scopeValue: string): TokenScopeCatalogItem | null {
  const normalized = scopeValue.trim();
  if (!normalized) return null;

  const baseScope = normalized.includes("@")
    ? normalized.slice(0, normalized.indexOf("@"))
    : normalized;
  const separator = baseScope.indexOf(":");
  if (separator <= 0 || separator === baseScope.length - 1) return null;

  return {
    action: baseScope.slice(0, separator),
    resourceType: baseScope.startsWith("cctv:") ? "cctv" : baseScope.slice(separator + 1),
    label: baseScope,
    value: baseScope,
  };
}

/**
 * Merge the runtime scope catalog with scope values observed elsewhere (listed
 * tokens, active table filters, a draft token), de-duplicating by value and
 * falling back to a single `read:inventory` entry when the catalog is empty.
 * Sort is by value so the picker and table filter stay stable.
 */
export function mergeTokenScopeOptions(
  catalog: TokenScopeCatalogItem[],
  observed: TokenScopeCatalogItem[],
): TokenScopeCatalogItem[] {
  const mergedByValue = new Map<string, TokenScopeCatalogItem>();
  for (const entry of [...catalog, ...observed]) {
    mergedByValue.set(entry.value, entry);
  }
  if (mergedByValue.size > 0) {
    return Array.from(mergedByValue.values()).sort((left, right) =>
      left.value.localeCompare(right.value),
    );
  }
  return [
    {
      action: "read",
      resourceType: "inventory",
      label: "read:inventory",
      value: "read:inventory",
    },
  ];
}
