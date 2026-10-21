//! Built-in exporter preset catalog.
//!
//! A preset bundles a small set of opinionated `metric_query` specs to a known
//! exporter slug. `POST /api/v1/monitoring/exporters/{slug}/apply` resolves the
//! exporter from `monitoring_exporter` and ensures these queries + a metric set
//! + a scope assignment exist atomically (idempotent on re-apply).
//!
//! The catalog is also the source of truth for the matching seed rows inserted
//! by `migrations/0001_init.sql`; the migration must stay in sync with the
//! const tables below so a fresh database boots with the presets already
//! applied at global scope. Seed changes that must also reach an already
//! deployed database ship as a forward migration (for the availability
//! default, `migrations/0006_monitoring_availability_default.sql`).

use serde_json::{Value, json};

/// A single metric query inside a preset, addressed by `slug` (unique within
/// the exporter via `(exporter_id, slug)`). When the slug is missing on apply,
/// the row is created from the spec; when it exists, the spec is treated as
/// metadata only (we never overwrite live queries).
#[derive(Debug, Clone)]
pub(super) struct PresetMetricQuery {
    pub slug: &'static str,
    pub name: &'static str,
    pub label: &'static str,
    pub query: &'static str,
    pub unit: Option<&'static str>,
    pub source_type: &'static str,
    pub query_type: &'static str,
    pub value_type: &'static str,
    pub widget_type: &'static str,
    pub default_show_in_list: bool,
    pub default_show_in_inspector: bool,
    pub sort_order: i32,
    /// Inline threshold/options object stored on the metric query row.
    pub options: fn() -> Value,
    /// Optional cross-surface meaning for special metrics such as host availability.
    pub semantic: Option<&'static str>,
}

/// A preset = an exporter slug + a curated metric query list.
#[derive(Debug, Clone)]
pub(super) struct ExporterPreset {
    /// Exporter slug to resolve in `monitoring_exporter`.
    pub exporter_slug: &'static str,
    /// Slug used for the `monitoring_metric_set` created on apply.
    pub set_slug: &'static str,
    /// Human-readable name for both UI and the metric set.
    pub name: &'static str,
    /// Short description shown in the catalog.
    pub description: &'static str,
    pub queries: &'static [PresetMetricQuery],
}

/// Empty `{}` options object (the default for queries without thresholds).
fn empty_options() -> Value {
    Value::Object(serde_json::Map::new())
}

fn percent_warn_70_crit_90() -> Value {
    json!({
        "display": {
            "status": {
                "mode": "numeric",
                "warning": 70,
                "critical": 90,
                "direction": "higher_is_worse"
            }
        }
    })
}

fn percent_warn_80_crit_90() -> Value {
    json!({
        "display": {
            "status": {
                "mode": "numeric",
                "warning": 80,
                "critical": 90,
                "direction": "higher_is_worse"
            }
        }
    })
}

fn percent_warn_80_crit_92() -> Value {
    json!({
        "display": {
            "status": {
                "mode": "numeric",
                "warning": 80,
                "critical": 92,
                "direction": "higher_is_worse"
            }
        }
    })
}

fn state_normal_up() -> Value {
    json!({
        "display": {
            "status": {
                "mode": "state",
                "normal": ["up", "1", "true", "ok", "active"],
                "warning": ["unknown", "pending"],
                "critical": ["down", "0", "false", "failed", "error"]
            }
        }
    })
}

fn probe_seconds_warn_500_crit_1500() -> Value {
    json!({
        "display": {
            "status": {
                "mode": "numeric",
                "warning": 0.5,
                "critical": 1.5,
                "direction": "higher_is_worse"
            }
        }
    })
}

/// Node exporter preset — the four signals an operator wants at a glance.
const NODE_EXPORTER_QUERIES: &[PresetMetricQuery] = &[
    PresetMetricQuery {
        slug: "up",
        name: "up",
        label: "Up",
        // Fleet-capable on purpose: no `$host*` token, so the poller demuxes
        // one `up{job="node"}` query by the SD correlation label instead of
        // issuing one templated query per host. This is the canonical default
        // availability descriptor; the blackbox preset's `probe_success` is
        // only eligible when the blackbox exporter is actually applied.
        query: r#"up{job="node"}"#,
        unit: None,
        source_type: "prometheus",
        query_type: "promql",
        value_type: "state",
        widget_type: "badge",
        default_show_in_list: true,
        default_show_in_inspector: true,
        sort_order: 10,
        options: state_normal_up,
        semantic: Some("availability"),
    },
    PresetMetricQuery {
        slug: "cpu_busy",
        name: "cpu_busy",
        label: "CPU busy",
        query: r#"100 - (avg by (piping_target_id) (rate(node_cpu_seconds_total{piping_target_id="$host_id",job="node",mode="idle"}[5m])) * 100)"#,
        unit: Some("%"),
        source_type: "prometheus",
        query_type: "promql",
        value_type: "percent",
        widget_type: "gauge",
        default_show_in_list: true,
        default_show_in_inspector: true,
        sort_order: 30,
        semantic: None,
        options: percent_warn_70_crit_90,
    },
    PresetMetricQuery {
        slug: "memory_used",
        name: "memory_used",
        label: "Memory used",
        query: r#"(1 - (node_memory_MemAvailable_bytes{piping_target_id="$host_id",job="node"} / node_memory_MemTotal_bytes{piping_target_id="$host_id",job="node"})) * 100"#,
        unit: Some("%"),
        source_type: "prometheus",
        query_type: "promql",
        value_type: "percent",
        widget_type: "gauge",
        default_show_in_list: true,
        default_show_in_inspector: true,
        sort_order: 40,
        semantic: None,
        options: percent_warn_80_crit_92,
    },
    PresetMetricQuery {
        slug: "disk_used_root",
        name: "disk_used_root",
        label: "Disk used /",
        query: r#"(1 - (node_filesystem_avail_bytes{piping_target_id="$host_id",job="node",mountpoint="/",fstype!~"tmpfs|overlay|squashfs"} / node_filesystem_size_bytes{piping_target_id="$host_id",job="node",mountpoint="/",fstype!~"tmpfs|overlay|squashfs"})) * 100"#,
        unit: Some("%"),
        source_type: "prometheus",
        query_type: "promql",
        value_type: "percent",
        widget_type: "gauge",
        default_show_in_list: true,
        default_show_in_inspector: true,
        sort_order: 50,
        semantic: None,
        options: percent_warn_80_crit_90,
    },
];

/// cAdvisor / Docker preset.
const CADVISOR_QUERIES: &[PresetMetricQuery] = &[
    PresetMetricQuery {
        slug: "containers",
        name: "containers",
        label: "Containers",
        query: r#"count(container_last_seen{piping_host_name="$host",job="docker"})"#,
        unit: Some("count"),
        source_type: "prometheus",
        query_type: "promql",
        value_type: "number",
        widget_type: "stat",
        default_show_in_list: true,
        default_show_in_inspector: true,
        sort_order: 10,
        semantic: None,
        options: empty_options,
    },
    PresetMetricQuery {
        slug: "container_cpu",
        name: "container_cpu",
        label: "Container CPU",
        query: r#"sum(rate(container_cpu_usage_seconds_total{piping_host_name="$host",job="docker",container!=""}[5m])) * 100"#,
        unit: Some("%"),
        source_type: "prometheus",
        query_type: "promql",
        value_type: "percent",
        widget_type: "gauge",
        default_show_in_list: true,
        default_show_in_inspector: true,
        sort_order: 20,
        semantic: None,
        options: percent_warn_70_crit_90,
    },
];

/// Blackbox ICMP preset — reachability plus probe-time signals for offline-style hosts.
///
/// `probe_success` is this preset's single availability descriptor. It is only
/// selected as the canonical availability metric when the blackbox exporter is
/// applied to the host; a fresh install that never scrapes blackbox falls back
/// to the node exporter's `up`. The preset stays a single source of truth — the
/// migration `0006_monitoring_availability_default.sql` converges an already
/// deployed database, and the read model filters by the host's applied exporters.
const BLACKBOX_ICMP_QUERIES: &[PresetMetricQuery] = &[
    PresetMetricQuery {
        slug: "probe_success",
        name: "probe_success",
        label: "Reachable",
        query: r#"probe_success{piping_host_name="$host",job="blackbox"}"#,
        unit: Some("state"),
        source_type: "prometheus",
        query_type: "promql",
        value_type: "state",
        widget_type: "badge",
        default_show_in_list: true,
        default_show_in_inspector: true,
        sort_order: 5,
        options: state_normal_up,
        semantic: Some("availability"),
    },
    PresetMetricQuery {
        slug: "icmp_rtt",
        name: "icmp_rtt",
        label: "ICMP RTT",
        query: r#"min by (piping_target_id) (last_over_time(ping_rtt_mean_seconds{piping_target_id="$host_id"}[5m]))"#,
        unit: Some("s"),
        source_type: "prometheus",
        query_type: "promql",
        value_type: "duration",
        widget_type: "stat",
        default_show_in_list: true,
        default_show_in_inspector: true,
        sort_order: 10,
        semantic: None,
        options: probe_seconds_warn_500_crit_1500,
    },
    PresetMetricQuery {
        slug: "probe_duration",
        name: "probe_duration",
        label: "Probe time",
        query: r#"min by (piping_target_id) (last_over_time(ping_rtt_worst_seconds{piping_target_id="$host_id"}[5m]))"#,
        unit: Some("s"),
        source_type: "prometheus",
        query_type: "promql",
        value_type: "duration",
        widget_type: "stat",
        default_show_in_list: false,
        default_show_in_inspector: true,
        sort_order: 20,
        semantic: None,
        options: empty_options,
    },
];

/// Postgres exporter preset.
const POSTGRES_EXPORTER_QUERIES: &[PresetMetricQuery] = &[PresetMetricQuery {
    slug: "pg_connections",
    name: "pg_connections",
    label: "DB connections",
    query: r#"sum(pg_stat_activity_count{piping_host_name="$host",job="postgres"})"#,
    unit: Some("count"),
    source_type: "prometheus",
    query_type: "promql",
    value_type: "number",
    widget_type: "stat",
    default_show_in_list: true,
    default_show_in_inspector: true,
    sort_order: 20,
    semantic: None,
    options: empty_options,
}];

/// The built-in preset catalog.
pub(super) const EXPORTER_PRESETS: &[ExporterPreset] = &[
    ExporterPreset {
        exporter_slug: "node",
        set_slug: "preset-node-exporter",
        name: "Node exporter quick view",
        description: "Up + CPU + memory + root disk for Linux node exporter.",
        queries: NODE_EXPORTER_QUERIES,
    },
    ExporterPreset {
        exporter_slug: "docker",
        set_slug: "preset-cadvisor",
        name: "cAdvisor quick view",
        description: "Container count and CPU pressure for Docker hosts.",
        queries: CADVISOR_QUERIES,
    },
    ExporterPreset {
        exporter_slug: "blackbox",
        set_slug: "preset-blackbox-icmp",
        name: "Blackbox ICMP probe",
        description: "Reachability + ICMP RTT + total probe duration via the blackbox exporter.",
        queries: BLACKBOX_ICMP_QUERIES,
    },
    ExporterPreset {
        exporter_slug: "postgres",
        set_slug: "preset-postgres-exporter",
        name: "Postgres exporter quick view",
        description: "Active database connection count for the postgres exporter.",
        queries: POSTGRES_EXPORTER_QUERIES,
    },
];

/// Look up a preset by its `exporter_slug`. Single addressing scheme — the
/// exporter slug is the same identifier used by `POST .../exporters/{slug}/apply`
/// and surfaced in `GET /api/v1/monitoring/presets`.
pub(super) fn find_preset_by_exporter_slug(slug: &str) -> Option<&'static ExporterPreset> {
    EXPORTER_PRESETS
        .iter()
        .find(|preset| preset.exporter_slug.eq_ignore_ascii_case(slug))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exporter_presets_have_unique_set_slugs() {
        let mut seen = std::collections::HashSet::new();
        for preset in EXPORTER_PRESETS {
            assert!(
                seen.insert(preset.set_slug),
                "preset set_slug `{}` is duplicated; metric_set.slug is UNIQUE",
                preset.set_slug
            );
        }
    }

    #[test]
    fn exporter_presets_have_unique_query_slugs_within_preset() {
        for preset in EXPORTER_PRESETS {
            let mut seen = std::collections::HashSet::new();
            for query in preset.queries {
                assert!(
                    seen.insert(query.slug),
                    "preset `{}` has duplicate query slug `{}`; (exporter_id, slug) is UNIQUE",
                    preset.set_slug,
                    query.slug
                );
            }
        }
    }

    #[test]
    fn lookups_match_existing_exporter_slugs() {
        for slug in ["node", "docker", "blackbox", "postgres"] {
            assert!(
                find_preset_by_exporter_slug(slug).is_some(),
                "expected preset for exporter slug `{slug}` so apply can reuse seeded exporters"
            );
        }
        assert!(find_preset_by_exporter_slug("nonexistent").is_none());
    }

    #[test]
    fn at_least_one_default_show_in_list_per_preset() {
        for preset in EXPORTER_PRESETS {
            assert!(
                preset
                    .queries
                    .iter()
                    .any(|query| query.default_show_in_list),
                "preset `{}` exposes nothing in the list view; default-set fallback would skip it",
                preset.set_slug
            );
        }
    }

    #[test]
    fn preset_options_render_as_objects() {
        for preset in EXPORTER_PRESETS {
            for query in preset.queries {
                let rendered = (query.options)();
                assert!(
                    rendered.is_object(),
                    "preset `{}` query `{}` options must be a JSON object (CHECK in init.sql)",
                    preset.set_slug,
                    query.slug
                );
            }
        }
    }
}
