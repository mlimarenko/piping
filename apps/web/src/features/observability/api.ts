import { API_BASE_URL, apiClient, unwrap } from "@/shared/api/client";
import { collectAllPages } from "@/shared/api/pagination";
import type { components, paths } from "@/shared/api/schema.gen";

export type MonitoringScrapeConfig = components["schemas"]["MonitoringScrapeConfig"];
export type MonitoringMetricDescriptor = components["schemas"]["MonitoringMetricDescriptor"];
export type ServiceDiscoveryTargetGroup = components["schemas"]["ServiceDiscoveryTargetGroup"];
export type ServiceDiscoveryHostPreview = components["schemas"]["ServiceDiscoveryHostPreview"];
export type ServiceDiscoveryPreviewResponse =
  components["schemas"]["ServiceDiscoveryPreviewResponse"];
export type ConnectionTargetObservabilityResponse =
  components["schemas"]["ConnectionTargetObservabilityResponse"];
export type LatestMetricItem = components["schemas"]["LatestMetricItem"];
export type LatestMetricsResponse = components["schemas"]["LatestMetricsResponse"];
export type HostMetricDescriptor = components["schemas"]["HostMetricDescriptor"];
export type MetricHistoryResponse = components["schemas"]["MetricHistoryResponse"];
export type MetricHistorySeries = components["schemas"]["MetricHistorySeries"];
export type HostLabelsResponse = components["schemas"]["HostLabelsResponse"];
export type HostLabelEntry = components["schemas"]["HostLabelEntry"];
export type HostLabelMetric = components["schemas"]["HostLabelMetric"];
export type HostAlert = components["schemas"]["HostAlert"];

const LATEST_METRIC_QUERY_BATCH_SIZE = 12;

type ServiceDiscoveryQueryParams = NonNullable<
  paths["/api/v1/monitoring/sd/preview"]["get"]["parameters"]["query"]
>;

export type ServiceDiscoveryParams = Pick<
  ServiceDiscoveryQueryParams,
  "kind" | "group_by" | "search" | "port" | "job" | "with_port" | "limit"
>;

function appendParam(params: URLSearchParams, key: string, value: unknown): void {
  if (value === undefined || value === null || value === "") return;
  if (typeof value === "string") {
    params.set(key, value);
    return;
  }
  if (typeof value === "number" || typeof value === "boolean" || typeof value === "bigint") {
    params.set(key, value.toString());
    return;
  }
  params.set(key, JSON.stringify(value));
}

function discoveryQuery(params?: ServiceDiscoveryParams): string {
  const search = new URLSearchParams();
  appendParam(search, "kind", params?.kind);
  appendParam(search, "group_by", params?.group_by);
  appendParam(search, "search", params?.search);
  appendParam(search, "port", params?.port);
  appendParam(search, "job", params?.job);
  appendParam(search, "with_port", params?.with_port);
  appendParam(search, "limit", params?.limit);
  const query = search.toString();
  return query ? `?${query}` : "";
}

function absoluteApiUrl(path: string): string {
  const apiPath = `${API_BASE_URL}${path}`;
  if (typeof window === "undefined") return apiPath;
  return new URL(apiPath, window.location.origin).toString();
}

export async function getServiceDiscoveryPreview(
  params?: ServiceDiscoveryParams,
): Promise<ServiceDiscoveryPreviewResponse> {
  return unwrap(
    await apiClient.GET("/api/v1/monitoring/sd/preview", {
      params: { query: params },
    }),
  );
}

export async function getConnectionTargetObservability(
  targetId: string,
  params?: ServiceDiscoveryParams,
): Promise<ConnectionTargetObservabilityResponse> {
  return unwrap(
    await apiClient.GET("/api/v1/monitoring/targets/{id}", {
      params: { path: { id: targetId }, query: params },
    }),
  );
}

export async function getLatestMetrics(
  queries: string[],
  targetId?: string,
): Promise<LatestMetricsResponse> {
  const cleanedQueries = queries.filter(Boolean);
  if (cleanedQueries.length <= LATEST_METRIC_QUERY_BATCH_SIZE) {
    return getLatestMetricsBatch(cleanedQueries, targetId);
  }

  const batches: string[][] = [];
  for (let index = 0; index < cleanedQueries.length; index += LATEST_METRIC_QUERY_BATCH_SIZE) {
    batches.push(cleanedQueries.slice(index, index + LATEST_METRIC_QUERY_BATCH_SIZE));
  }

  const responses = await Promise.all(
    batches.map((batch) => getLatestMetricsBatch(batch, targetId)),
  );
  const firstResponse = responses[0];
  const firstSource = responses.find((response) => response.source)?.source;
  const firstMessage = responses.find((response) => response.message)?.message;

  return {
    configured: responses.some((response) => response.configured),
    items: responses.flatMap((response) => response.items ?? []),
    message: firstMessage ?? firstResponse?.message,
    source: firstSource ?? firstResponse?.source,
  };
}

async function getLatestMetricsBatch(
  queries: string[],
  targetId?: string,
): Promise<LatestMetricsResponse> {
  return unwrap(
    await apiClient.GET("/api/v1/monitoring/latest", {
      params: { query: { query: queries, target_id: targetId || undefined } },
    }),
  );
}

export async function getMetricHistory(
  targetId: string,
  query: string,
  start: number,
  end: number,
  step?: number,
): Promise<MetricHistoryResponse> {
  return unwrap(
    await apiClient.GET("/api/v1/monitoring/history", {
      params: { query: { target_id: targetId, query, start, end, step } },
    }),
  );
}

export async function getHostLabels(hostIds: string[], signal?: AbortSignal): Promise<HostLabelsResponse> {
  return unwrap(
    await apiClient.GET("/api/v1/monitoring/host-labels", {
      params: { query: { host_id: hostIds.filter(Boolean) } },
      signal,
    }),
  );
}

export function serviceDiscoveryCollectorUrl(params?: ServiceDiscoveryParams): string {
  return absoluteApiUrl(`/api/v1/monitoring/sd${discoveryQuery(params)}`);
}

// ── Shared scope vocabulary ──────────────────────────────
//
// The monitoring CRUD endpoints (exporter / query-source / metric-set
// assignments) all share one scope vocabulary. `global` carries no `scope_id`;
// every other layer resolves `scope_id` against an inventory entity.

export const MONITORING_SCOPE_TYPES = [
  "global",
  "site",
  "tenant",
  "role",
  "connection_target",
] as const;
export type MonitoringScopeType = (typeof MONITORING_SCOPE_TYPES)[number];

// ── Service Discovery: label settings ───────────────────

export type SdLabelGroups = components["schemas"]["SdLabelGroupsDto"] & {
  device_statuses: string[];
  site_statuses: string[];
};
export type SdLabelSettingsResponse = Omit<
  components["schemas"]["SdLabelSettingsResponse"],
  "groups"
> & { groups: SdLabelGroups };

function normalizeSdLabelSettings(
  response: components["schemas"]["SdLabelSettingsResponse"],
): SdLabelSettingsResponse {
  return {
    ...response,
    groups: {
      ...response.groups,
      device_statuses: response.groups.device_statuses ?? ["active"],
      site_statuses: response.groups.site_statuses ?? ["active"],
    },
  };
}

export async function getSdLabelSettings(): Promise<SdLabelSettingsResponse> {
  return normalizeSdLabelSettings(unwrap(await apiClient.GET("/api/v1/monitoring/sd/label-settings")));
}

export async function updateSdLabelSettings(groups: SdLabelGroups): Promise<SdLabelSettingsResponse> {
  return normalizeSdLabelSettings(
    unwrap(await apiClient.PUT("/api/v1/monitoring/sd/label-settings", { body: { groups } })),
  );
}

// ── Service Discovery: exporter catalog / suggestions ────

export type ExporterCatalogItem = components["schemas"]["ExporterCatalogItem"];
export type ExporterCatalogListResponse = components["schemas"]["ExporterCatalogListResponse"];
export type ExporterPresetApplyRequest = components["schemas"]["ExporterPresetApplyRequest"];
export type ExporterPresetApplyResponse = components["schemas"]["ExporterPresetApplyResponse"];
export type ExporterPresetCatalogItem = components["schemas"]["ExporterPresetCatalogItem"];
export type ExporterPresetCatalogResponse =
  components["schemas"]["ExporterPresetCatalogResponse"];
export type MonitoringQuickStartRequest = components["schemas"]["MonitoringQuickStartRequest"];
export type MonitoringQuickStartResponse = components["schemas"]["MonitoringQuickStartResponse"];

export interface MonitoringListControls {
  sort?: string;
  order?: string;
  enabled?: string;
  limit?: number;
  offset?: number;
}

export interface ExporterCatalogFilter extends MonitoringListControls {
  exporter_type?: string;
  default_target_source?: string;
}

export async function listExporters(filter?: ExporterCatalogFilter): Promise<ExporterCatalogListResponse> {
  return unwrap(
    await apiClient.GET("/api/v1/monitoring/exporters", {
      params: { query: filter as never },
    }),
  );
}

/**
 * Enumerate the built-in exporter presets the apply + quick-start endpoints
 * accept. Used by the wizard and the apply dialog so they only offer
 * exporters that actually have a preset (no broken-combo 404s).
 */
export async function listMonitoringPresets(): Promise<ExporterPresetCatalogResponse> {
  return unwrap(await apiClient.GET("/api/v1/monitoring/presets"));
}

/**
 * Apply a built-in exporter preset to a scope. The backend resolves the preset
 * from the exporter slug, ensures queries + set + assignment exist atomically,
 * and is idempotent on re-apply.
 */
export async function applyExporterPreset(
  slug: string,
  body: ExporterPresetApplyRequest,
): Promise<ExporterPresetApplyResponse> {
  return unwrap(
    await apiClient.POST("/api/v1/monitoring/exporters/{slug}/apply", {
      params: { path: { slug } },
      body,
    }),
  );
}

/**
 * Quick-start: create a query source, attach it to a scope, optionally apply a
 * built-in preset. Each step is idempotent on re-call.
 */
export async function monitoringQuickStart(
  body: MonitoringQuickStartRequest,
): Promise<MonitoringQuickStartResponse> {
  return unwrap(
    await apiClient.POST("/api/v1/monitoring/quick-start", { body }),
  );
}

// ── Service Discovery: exporter assignments (CRUD) ───────

export type ExporterAssignmentItem = components["schemas"]["ExporterAssignmentItem"];
export type ExporterAssignmentListResponse =
  components["schemas"]["ExporterAssignmentListResponse"];
export type ExporterAssignmentRequest = components["schemas"]["ExporterAssignmentRequest"];

export interface ScopeFilter extends MonitoringListControls {
  scope_type?: string;
  scope_id?: string;
}

export interface ExporterAssignmentFilter extends ScopeFilter {
  exporter_id?: string;
}

export async function listExporterAssignments(
  filter?: ExporterAssignmentFilter,
): Promise<ExporterAssignmentListResponse> {
  return unwrap(
    await apiClient.GET("/api/v1/monitoring/exporter-assignments", {
      params: { query: filter as never },
    }),
  );
}

/**
 * Page size for assignment reads that need the whole set.
 *
 * The endpoint honours `limit`/`offset` and defaults to a page, so a screen
 * that renders "every assignment" must page to completion instead of trusting
 * a single response to be complete.
 */
const EXPORTER_ASSIGNMENT_PAGE_SIZE = 200;

/** Every exporter assignment matching the filter, paged to completion. */
export async function listAllExporterAssignments(
  filter?: ExporterAssignmentFilter,
): Promise<ExporterAssignmentItem[]> {
  return collectAllPages(
    (limit, offset) => listExporterAssignments({ ...filter, limit, offset }),
    EXPORTER_ASSIGNMENT_PAGE_SIZE,
  );
}

export async function createExporterAssignment(
  body: ExporterAssignmentRequest,
): Promise<ExporterAssignmentItem> {
  return unwrap(
    await apiClient.POST("/api/v1/monitoring/exporter-assignments", { body }),
  );
}

export async function updateExporterAssignment(
  id: string,
  body: ExporterAssignmentRequest,
): Promise<ExporterAssignmentItem> {
  return unwrap(
    await apiClient.PUT("/api/v1/monitoring/exporter-assignments/{id}", {
      params: { path: { id } },
      body,
    }),
  );
}

export async function deleteExporterAssignment(id: string): Promise<void> {
  await apiClient.DELETE("/api/v1/monitoring/exporter-assignments/{id}", {
    params: { path: { id } },
  });
}

// ── Inspector: query sources (CRUD) ──────────────────────

export type QuerySourceItem = components["schemas"]["QuerySourceItem"];
export type QuerySourceListResponse = components["schemas"]["QuerySourceListResponse"];
export type QuerySourceRequest = components["schemas"]["QuerySourceRequest"];

export interface QuerySourceFilter extends MonitoringListControls {
  source_type?: string;
}

export async function listQuerySources(filter?: QuerySourceFilter): Promise<QuerySourceListResponse> {
  return unwrap(
    await apiClient.GET("/api/v1/monitoring/query-sources", {
      params: { query: filter as never },
    }),
  );
}

export async function createQuerySource(body: QuerySourceRequest): Promise<QuerySourceItem> {
  return unwrap(await apiClient.POST("/api/v1/monitoring/query-sources", { body }));
}

export async function updateQuerySource(
  id: string,
  body: QuerySourceRequest,
): Promise<QuerySourceItem> {
  return unwrap(
    await apiClient.PUT("/api/v1/monitoring/query-sources/{id}", {
      params: { path: { id } },
      body,
    }),
  );
}

export async function deleteQuerySource(id: string): Promise<void> {
  await apiClient.DELETE("/api/v1/monitoring/query-sources/{id}", {
    params: { path: { id } },
  });
}

// ── Inspector: query source assignments (CRUD) ───────────

export type QuerySourceAssignmentItem = components["schemas"]["QuerySourceAssignmentItem"];
export type QuerySourceAssignmentListResponse =
  components["schemas"]["QuerySourceAssignmentListResponse"];
export type QuerySourceAssignmentRequest =
  components["schemas"]["QuerySourceAssignmentRequest"];

export async function listQuerySourceAssignments(
  filter?: ScopeFilter,
): Promise<QuerySourceAssignmentListResponse> {
  return unwrap(
    await apiClient.GET("/api/v1/monitoring/query-source-assignments", {
      params: { query: filter as never },
    }),
  );
}

export async function createQuerySourceAssignment(
  body: QuerySourceAssignmentRequest,
): Promise<QuerySourceAssignmentItem> {
  return unwrap(
    await apiClient.POST("/api/v1/monitoring/query-source-assignments", { body }),
  );
}

export async function updateQuerySourceAssignment(
  id: string,
  body: QuerySourceAssignmentRequest,
): Promise<QuerySourceAssignmentItem> {
  return unwrap(
    await apiClient.PUT("/api/v1/monitoring/query-source-assignments/{id}", {
      params: { path: { id } },
      body,
    }),
  );
}

export async function deleteQuerySourceAssignment(id: string): Promise<void> {
  await apiClient.DELETE("/api/v1/monitoring/query-source-assignments/{id}", {
    params: { path: { id } },
  });
}

// ── Inspector: metric queries (CRUD) ─────────────────────

export type MetricQueryItem = components["schemas"]["MetricQueryItem"];
export type MetricQueryListResponse = components["schemas"]["MetricQueryListResponse"];
export type MetricQueryRequest = components["schemas"]["MetricQueryRequest"];

export interface MetricQueryFilter extends MonitoringListControls {
  exporter_id?: string;
  source_type?: string;
  query_type?: string;
  value_type?: string;
  widget_type?: string;
}

export async function listMetricQueries(
  filter?: MetricQueryFilter,
): Promise<MetricQueryListResponse> {
  return unwrap(
    await apiClient.GET("/api/v1/monitoring/metric-queries", {
      params: { query: filter as never },
    }),
  );
}

export async function createMetricQuery(body: MetricQueryRequest): Promise<MetricQueryItem> {
  return unwrap(await apiClient.POST("/api/v1/monitoring/metric-queries", { body }));
}

export async function updateMetricQuery(
  id: string,
  body: MetricQueryRequest,
): Promise<MetricQueryItem> {
  return unwrap(
    await apiClient.PUT("/api/v1/monitoring/metric-queries/{id}", {
      params: { path: { id } },
      body,
    }),
  );
}

export async function deleteMetricQuery(id: string): Promise<void> {
  await apiClient.DELETE("/api/v1/monitoring/metric-queries/{id}", {
    params: { path: { id } },
  });
}

// ── Inspector: metric sets + items (CRUD) ────────────────

export type MetricSetDetail = components["schemas"]["MetricSetDetail"];
export type MetricSetListResponse = components["schemas"]["MetricSetListResponse"];
export type MetricSetRequest = components["schemas"]["MetricSetRequest"];
export type MetricSetItemEntry = components["schemas"]["MetricSetItemEntry"];
export type MetricSetItemRequest = components["schemas"]["MetricSetItemRequest"];

export async function listMetricSets(
  filter?: MonitoringListControls,
): Promise<MetricSetListResponse> {
  return unwrap(
    await apiClient.GET("/api/v1/monitoring/metric-sets", {
      params: { query: filter as never },
    }),
  );
}

export async function createMetricSet(body: MetricSetRequest): Promise<MetricSetDetail> {
  return unwrap(await apiClient.POST("/api/v1/monitoring/metric-sets", { body }));
}

export async function updateMetricSet(
  id: string,
  body: MetricSetRequest,
): Promise<MetricSetDetail> {
  return unwrap(
    await apiClient.PUT("/api/v1/monitoring/metric-sets/{id}", {
      params: { path: { id } },
      body,
    }),
  );
}

export async function deleteMetricSet(id: string): Promise<void> {
  await apiClient.DELETE("/api/v1/monitoring/metric-sets/{id}", {
    params: { path: { id } },
  });
}

export async function createMetricSetItem(
  setId: string,
  body: MetricSetItemRequest,
): Promise<MetricSetItemEntry> {
  return unwrap(
    await apiClient.POST("/api/v1/monitoring/metric-sets/{id}/items", {
      params: { path: { id: setId } },
      body,
    }),
  );
}

export async function updateMetricSetItem(
  setId: string,
  itemId: string,
  body: MetricSetItemRequest,
): Promise<MetricSetItemEntry> {
  return unwrap(
    await apiClient.PUT("/api/v1/monitoring/metric-sets/{id}/items/{item_id}", {
      params: { path: { id: setId, item_id: itemId } },
      body,
    }),
  );
}

export async function deleteMetricSetItem(setId: string, itemId: string): Promise<void> {
  await apiClient.DELETE("/api/v1/monitoring/metric-sets/{id}/items/{item_id}", {
    params: { path: { id: setId, item_id: itemId } },
  });
}

// ── Inspector: metric set assignments (CRUD) ─────────────

export type MetricSetAssignmentItem = components["schemas"]["MetricSetAssignmentItem"];
export type MetricSetAssignmentListResponse =
  components["schemas"]["MetricSetAssignmentListResponse"];
export type MetricSetAssignmentRequest =
  components["schemas"]["MetricSetAssignmentRequest"];

export async function listMetricSetAssignments(
  filter?: ScopeFilter,
): Promise<MetricSetAssignmentListResponse> {
  return unwrap(
    await apiClient.GET("/api/v1/monitoring/metric-set-assignments", {
      params: { query: filter as never },
    }),
  );
}

export async function createMetricSetAssignment(
  body: MetricSetAssignmentRequest,
): Promise<MetricSetAssignmentItem> {
  return unwrap(
    await apiClient.POST("/api/v1/monitoring/metric-set-assignments", { body }),
  );
}

export async function updateMetricSetAssignment(
  id: string,
  body: MetricSetAssignmentRequest,
): Promise<MetricSetAssignmentItem> {
  return unwrap(
    await apiClient.PUT("/api/v1/monitoring/metric-set-assignments/{id}", {
      params: { path: { id } },
      body,
    }),
  );
}

export async function deleteMetricSetAssignment(id: string): Promise<void> {
  await apiClient.DELETE("/api/v1/monitoring/metric-set-assignments/{id}", {
    params: { path: { id } },
  });
}

// ── Alerts: alert sources (CRUD) ─────────────────────────
//
// Alert sources are the alert-side mirror of query sources: an external
// alerting backend (Alertmanager / vmalert / Zabbix) the poller pulls active
// alerts from, correlated to hosts and surfaced on the list-label entries.

export type AlertSourceItem = components["schemas"]["AlertSourceItem"];
export type AlertSourceListResponse = components["schemas"]["AlertSourceListResponse"];
export type AlertSourceRequest = components["schemas"]["AlertSourceRequest"];

export async function listAlertSources(filter?: QuerySourceFilter): Promise<AlertSourceListResponse> {
  return unwrap(
    await apiClient.GET("/api/v1/monitoring/alert-sources", {
      params: { query: filter as never },
    }),
  );
}

export async function createAlertSource(body: AlertSourceRequest): Promise<AlertSourceItem> {
  return unwrap(await apiClient.POST("/api/v1/monitoring/alert-sources", { body }));
}

export async function updateAlertSource(
  id: string,
  body: AlertSourceRequest,
): Promise<AlertSourceItem> {
  return unwrap(
    await apiClient.PUT("/api/v1/monitoring/alert-sources/{id}", {
      params: { path: { id } },
      body,
    }),
  );
}

export async function deleteAlertSource(id: string): Promise<void> {
  await apiClient.DELETE("/api/v1/monitoring/alert-sources/{id}", {
    params: { path: { id } },
  });
}

// ── Alerts: alert source assignments (CRUD) ──────────────

export type AlertSourceAssignmentItem = components["schemas"]["AlertSourceAssignmentItem"];
export type AlertSourceAssignmentListResponse =
  components["schemas"]["AlertSourceAssignmentListResponse"];
export type AlertSourceAssignmentRequest =
  components["schemas"]["AlertSourceAssignmentRequest"];

export async function listAlertSourceAssignments(
  filter?: ScopeFilter,
): Promise<AlertSourceAssignmentListResponse> {
  return unwrap(
    await apiClient.GET("/api/v1/monitoring/alert-source-assignments", {
      params: { query: filter as never },
    }),
  );
}

export async function createAlertSourceAssignment(
  body: AlertSourceAssignmentRequest,
): Promise<AlertSourceAssignmentItem> {
  return unwrap(
    await apiClient.POST("/api/v1/monitoring/alert-source-assignments", { body }),
  );
}

export async function updateAlertSourceAssignment(
  id: string,
  body: AlertSourceAssignmentRequest,
): Promise<AlertSourceAssignmentItem> {
  return unwrap(
    await apiClient.PUT("/api/v1/monitoring/alert-source-assignments/{id}", {
      params: { path: { id } },
      body,
    }),
  );
}

export async function deleteAlertSourceAssignment(id: string): Promise<void> {
  await apiClient.DELETE("/api/v1/monitoring/alert-source-assignments/{id}", {
    params: { path: { id } },
  });
}

// ── Metric query: per-host overrides (CRUD) ──────────────
//
// Persisted per-host override of a metric query's label/unit/display rules.
// Backend stores `options_override` as the `display` subtree (replaces
// wholesale at apply_host_overrides time — see monitoring.rs).

export type MetricQueryHostOverrideItem =
  components["schemas"]["MetricQueryHostOverrideItem"];
export type MetricQueryHostOverrideListResponse =
  components["schemas"]["MetricQueryHostOverrideListResponse"];
export type MetricQueryHostOverrideRequest =
  components["schemas"]["MetricQueryHostOverrideRequest"];

export async function listMetricQueryHostOverrides(
  hostId: string,
): Promise<MetricQueryHostOverrideListResponse> {
  return unwrap(
    await apiClient.GET("/api/v1/monitoring/host-overrides", {
      params: { query: { host_id: hostId } },
    }),
  );
}

export async function createMetricQueryHostOverride(
  body: MetricQueryHostOverrideRequest,
): Promise<MetricQueryHostOverrideItem> {
  return unwrap(await apiClient.POST("/api/v1/monitoring/host-overrides", { body }));
}

export async function updateMetricQueryHostOverride(
  id: string,
  body: MetricQueryHostOverrideRequest,
): Promise<MetricQueryHostOverrideItem> {
  return unwrap(
    await apiClient.PUT("/api/v1/monitoring/host-overrides/{id}", {
      params: { path: { id } },
      body,
    }),
  );
}

export async function deleteMetricQueryHostOverride(id: string): Promise<void> {
  await apiClient.DELETE("/api/v1/monitoring/host-overrides/{id}", {
    params: { path: { id } },
  });
}

// ── Metric query: live test (ad-hoc PromQL/Zabbix) ───────

export type MetricQueryTestRequest = components["schemas"]["MetricQueryTestRequest"];
export type MetricQueryTestResponse = components["schemas"]["MetricQueryTestResponse"];

export async function testMetricQuery(
  body: MetricQueryTestRequest,
): Promise<MetricQueryTestResponse> {
  return unwrap(await apiClient.POST("/api/v1/monitoring/metric-queries/test", { body }));
}
