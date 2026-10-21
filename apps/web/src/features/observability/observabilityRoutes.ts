/**
 * Canonical route contract for the observability feature.
 *
 * The observability tab shell stays mounted at {@link OBSERVABILITY_ROUTE}; the
 * four inspector-metric entities (query sources, metric queries, metric sets,
 * alert sources) each get a routed create/edit/detail page underneath it, the
 * same shape the connection-config feature uses. `view` names the inspector
 * workbench tab that lists the entity, so returning from an editor lands the
 * operator back on the list they launched it from.
 */

export const OBSERVABILITY_ROUTE = "/observability";
export const EXPORTER_ASSIGNMENTS_BASE = `${OBSERVABILITY_ROUTE}/exporter-assignments`;

const ASSIGNMENT_LIST_KEYS = ["search", "sort", "order", "page", "exporter", "scope_type", "scope_id", "enabled"] as const;

export function exporterListContext(search: string): string {
  const input = new URLSearchParams(search);
  const context = new URLSearchParams();
  for (const key of ASSIGNMENT_LIST_KEYS) {
    const value = input.get(key);
    if (value) context.set(key, value);
  }
  const query = context.toString();
  return query ? `?${query}` : "";
}

export function exporterAssignmentsHref(search = ""): string {
  return `${OBSERVABILITY_ROUTE}${exporterListContext(search)}`;
}

/** Workbench tabs inside the inspector-metrics screen. */
export const INSPECTOR_VIEWS = ["profiles", "cards", "groups", "sources", "alerts"] as const;
export type InspectorView = (typeof INSPECTOR_VIEWS)[number];

interface ObservabilityEntityRoute {
  /** Path prefix for the entity's `/new`, `/:id/edit` and `/:id` routes. */
  base: string;
  /** Inspector workbench tab the entity is listed under. */
  view: InspectorView;
}

export const OBSERVABILITY_ENTITY_ROUTES = {
  querySources: { base: `${OBSERVABILITY_ROUTE}/sources`, view: "sources" },
  metricQueries: { base: `${OBSERVABILITY_ROUTE}/metrics`, view: "cards" },
  metricSets: { base: `${OBSERVABILITY_ROUTE}/sets`, view: "groups" },
  alertSources: { base: `${OBSERVABILITY_ROUTE}/alert-sources`, view: "alerts" },
} as const satisfies Record<string, ObservabilityEntityRoute>;

/** `/<base>/<id>` — the read-only detail route for an entity. */
export function observabilityRecordHref(base: string, id: string): string {
  return `${base}/${encodeURIComponent(id)}`;
}

/** `/<base>/new` — the create route for an entity. */
export function observabilityNewHref(base: string): string {
  return `${base}/new`;
}

/** `/<base>/<id>/edit` — the edit route for an entity. */
export function observabilityEditHref(base: string, id: string): string {
  return `${base}/${encodeURIComponent(id)}/edit`;
}

/** `/observability?tab=inspector-metrics&view=<view>` — the entity's list. */
export function observabilityListHref(view: InspectorView): string {
  return `${OBSERVABILITY_ROUTE}?tab=inspector-metrics&view=${view}`;
}
