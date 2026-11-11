import { stringify } from "yaml";

/**
 * One resolved scrape target together with the assignment that produced it.
 *
 * `labels` are the labels service discovery resolves for the target, including
 * the `__scheme__`, `__metrics_path__` and `piping_job` entries the SD endpoint
 * generates. Keeping them here means the generated collector file is derived
 * from the same data `/api/v1/monitoring/sd` emits, not from a second resolver.
 */
export interface CollectorTarget {
  assignmentId: string;
  exporterSlug: string;
  job: string | null;
  target: string;
  labels: Record<string, string>;
}

export interface CollectorArtifact {
  yaml: string;
  fileName: string;
}

export type CollectorMode = "http_sd" | "direct";

const CONFIG_FILE_NAME = "piping-scrape-configs.yaml";
const DEFAULT_SCRAPE_INTERVAL = "30s";
const DEFAULT_METRICS_PATH = "/metrics";
const DEFAULT_SCHEME = "http";

/** Labels service discovery owns; they must not become static target labels. */
const RESERVED_LABELS = new Set(["__scheme__", "__metrics_path__", "__address__", "job"]);

interface StaticGroup {
  job: string;
  scheme: string;
  metricsPath: string;
  modules: string[];
  /** Target list keyed by the static labels shared by those targets. */
  staticConfigs: Map<string, { labels: Record<string, string>; targets: string[] }>;
}

function staticLabels(labels: Record<string, string>): Record<string, string> {
  const result: Record<string, string> = {};
  for (const [key, value] of Object.entries(labels)) {
    if (RESERVED_LABELS.has(key) || key.startsWith("__")) continue;
    result[key] = value;
  }
  return result;
}

function commentHeader(subject: string | null): string[] {
  const lines = [
    "# Piping collector configuration.",
    "# Generated from the effective service-discovery targets; re-generate after",
    "# changing exporter assignments. Secret values are never emitted: point the",
    "# credential files at locally readable copies instead.",
  ];
  if (subject) lines.push(`# Scope: ${subject}`);
  return lines;
}

function render(body: Record<string, unknown>, footer: string[], subject: string | null): CollectorArtifact {
  const yaml = [...commentHeader(subject), stringify(body).trimEnd(), ...footer, ""].join("\n");
  return { yaml, fileName: CONFIG_FILE_NAME };
}

/**
 * Prometheus/vmagent file for the HTTP service-discovery feed.
 *
 * One job keeps the collector configuration small and lets Piping add or remove
 * targets without touching the collector. `piping_job` restores the per-target
 * job label that discovery resolved, and the SD labels supply `__scheme__` and
 * `__metrics_path__` per target.
 */
export function buildHttpSdConfig(
  collectorUrl: string,
  subject: string | null = null,
): CollectorArtifact {
  const body = {
    global: { scrape_interval: DEFAULT_SCRAPE_INTERVAL },
    scrape_configs: [
      {
        job_name: "piping",
        http_sd_configs: [{ url: collectorUrl }],
        relabel_configs: [{ source_labels: ["piping_job"], target_label: "job" }],
      },
    ],
  };
  const footer = [
    "# Add a bearer token when the discovery endpoint requires one:",
    "#   authorization:",
    "#     type: Bearer",
    "#     credentials_file: /etc/prometheus/piping-sd-token",
    "# TLS for the discovery endpoint, when Piping serves it over a private CA:",
    "#   tls_config:",
    "#     ca_file: /etc/prometheus/piping-ca.pem",
  ];
  return render(body, footer, subject);
}

/**
 * Static file for collectors that cannot reach the discovery endpoint.
 *
 * Targets are grouped by the scrape settings they share, so a blackbox probe
 * keeps its `/probe` path and `module` parameter instead of collapsing into a
 * node-exporter style scrape.
 */
export function buildDirectConfig(
  targets: CollectorTarget[],
  subject: string | null = null,
): CollectorArtifact {
  const groups = new Map<string, StaticGroup>();
  for (const item of targets) {
    const scheme = (item.labels.__scheme__ ?? DEFAULT_SCHEME).toLowerCase();
    const metricsPath = item.labels.__metrics_path__ ?? DEFAULT_METRICS_PATH;
    const job = item.job ?? item.labels.piping_job ?? item.exporterSlug;
    const module = item.labels.module;
    const groupKey = [job, scheme, metricsPath, module ?? ""].join("\u0000");
    let group = groups.get(groupKey);
    if (!group) {
      group = {
        job,
        scheme,
        metricsPath,
        modules: module ? [module] : [],
        staticConfigs: new Map(),
      };
      groups.set(groupKey, group);
    }
    const labels = staticLabels(item.labels);
    const labelKey = JSON.stringify(labels);
    const entry = group.staticConfigs.get(labelKey);
    if (entry) {
      if (!entry.targets.includes(item.target)) entry.targets.push(item.target);
    } else {
      group.staticConfigs.set(labelKey, { labels, targets: [item.target] });
    }
  }

  const scrapeConfigs = [...groups.values()].map((group) => ({
    job_name: group.job,
    ...(group.scheme !== DEFAULT_SCHEME ? { scheme: group.scheme } : {}),
    ...(group.metricsPath !== DEFAULT_METRICS_PATH ? { metrics_path: group.metricsPath } : {}),
    ...(group.modules.length > 0 ? { params: { module: group.modules } } : {}),
    static_configs: [...group.staticConfigs.values()].map((entry) => ({
      targets: entry.targets,
      ...(Object.keys(entry.labels).length > 0 ? { labels: entry.labels } : {}),
    })),
  }));

  const body = {
    global: { scrape_interval: DEFAULT_SCRAPE_INTERVAL },
    scrape_configs: scrapeConfigs,
  };
  const footer = [
    "# TLS for a scrape target that serves HTTPS, when Piping does not publish",
    "# the certificate authority:",
    "#   tls_config:",
    "#     ca_file: /etc/prometheus/piping-ca.pem",
  ];
  return render(body, footer, subject);
}

export function buildCollectorArtifact(
  mode: CollectorMode,
  collectorUrl: string,
  targets: CollectorTarget[],
  subject: string | null = null,
): CollectorArtifact {
  return mode === "http_sd"
    ? buildHttpSdConfig(collectorUrl, subject)
    : buildDirectConfig(targets, subject);
}
