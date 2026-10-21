import type {
  HostLabelMetric,
  LatestMetricItem,
  MonitoringMetricDescriptor,
} from "@/features/observability/api";

/**
 * Projects a cached `HostLabelMetric` onto the live `LatestMetricItem` shape so
 * the shared metricHelpers / metricDisplay formatting and coloring path applies
 * unchanged. Single source for this conversion, reused by the list label, the
 * alert rollup and the hover card instead of re-deriving it per call site.
 */
export function hostLabelMetricItem(metric: HostLabelMetric): LatestMetricItem {
  return {
    query: metric.query,
    value: metric.value,
    error: null,
    unit: metric.unit ?? null,
    value_type: metric.value_type ?? null,
    status: "success",
  };
}

/**
 * Projects a cached `HostLabelMetric` onto the `MonitoringMetricDescriptor`
 * shape so the same status/threshold logic the inspector card uses applies to
 * the list label, the alert rollup and the hover card.
 */
export function hostLabelMetricDescriptor(metric: HostLabelMetric): MonitoringMetricDescriptor {
  return {
    name: metric.label ?? metric.query,
    label: metric.label ?? null,
    query: metric.query,
    unit: metric.unit ?? null,
    value_type: metric.value_type ?? null,
    widget_type: metric.widget_type ?? null,
    options: metric.options,
    list_render: metric.list_render,
    show_in_list: true,
    // Round-trip the grouping affordance so the descriptor surface stays
    // consistent across the cache-projected path (list label / hover card)
    // and the inspector path; any future grouping consumer reads one shape.
    category: metric.category ?? null,
    semantic: metric.semantic ?? null,
  };
}
