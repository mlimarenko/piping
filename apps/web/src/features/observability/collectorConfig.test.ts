import { describe, expect, it } from "vitest";
import { parse } from "yaml";
import {
  buildCollectorArtifact,
  buildDirectConfig,
  buildHttpSdConfig,
  type CollectorTarget,
} from "./collectorConfig";

const collectorUrl = "https://piping.example.test/api/v1/monitoring/sd";

function target(overrides: Partial<CollectorTarget> & { target: string }): CollectorTarget {
  return {
    assignmentId: "a1",
    exporterSlug: "node",
    job: "node",
    labels: { __scheme__: "http", __metrics_path__: "/metrics", piping_job: "node" },
    ...overrides,
  };
}

describe("collector config", () => {
  it("serves the discovery feed from one job and restores the job label", () => {
    const artifact = buildHttpSdConfig(collectorUrl, "device db-01");
    const config = parse(artifact.yaml) as {
      scrape_configs: {
        job_name: string;
        http_sd_configs: { url: string }[];
        relabel_configs: { source_labels: string[]; target_label: string }[];
      }[];
    };

    expect(artifact.fileName).toBe("piping-scrape-configs.yaml");
    expect(config.scrape_configs).toHaveLength(1);
    const [job] = config.scrape_configs;
    expect(job?.job_name).toBe("piping");
    expect(job?.http_sd_configs[0]?.url).toBe(collectorUrl);
    expect(job?.relabel_configs).toContainEqual({
      source_labels: ["piping_job"],
      target_label: "job",
    });
    expect(artifact.yaml).toContain("# Scope: device db-01");
  });

  it("keeps blackbox probes on their own job, path and module", () => {
    const artifact = buildDirectConfig([
      target({ target: "10.0.0.5:9100" }),
      target({
        assignmentId: "a2",
        exporterSlug: "blackbox",
        job: "blackbox-icmp",
        target: "10.0.0.5:9115",
        labels: {
          __scheme__: "http",
          __metrics_path__: "/probe",
          piping_job: "blackbox-icmp",
          module: "icmp",
          piping_target_address: "10.0.0.5",
        },
      }),
    ]);
    const config = parse(artifact.yaml) as {
      scrape_configs: {
        job_name: string;
        metrics_path?: string;
        params?: { module: string[] };
        static_configs: { targets: string[]; labels?: Record<string, string> }[];
      }[];
    };

    const blackbox = config.scrape_configs.find((job) => job.job_name === "blackbox-icmp");
    expect(blackbox?.metrics_path).toBe("/probe");
    expect(blackbox?.params).toEqual({ module: ["icmp"] });
    const blackboxStatics = blackbox?.static_configs ?? [];
    expect(blackboxStatics[0]?.targets).toEqual(["10.0.0.5:9115"]);
    expect(blackboxStatics[0]?.labels).toMatchObject({
      module: "icmp",
      piping_job: "blackbox-icmp",
    });

    const node = config.scrape_configs.find((job) => job.job_name === "node");
    expect(node?.metrics_path).toBeUndefined();
    expect(node?.params).toBeUndefined();
  });

  it("carries a non-default scheme onto the generated job", () => {
    const artifact = buildDirectConfig([
      target({
        target: "10.0.0.6:9100",
        labels: { __scheme__: "https", __metrics_path__: "/custom-metrics", piping_job: "node" },
      }),
    ]);
    const config = parse(artifact.yaml) as {
      scrape_configs: { scheme?: string; metrics_path?: string; static_configs: { targets: string[] }[] }[];
    };

    const [job] = config.scrape_configs;
    expect(job?.scheme).toBe("https");
    expect(job?.metrics_path).toBe("/custom-metrics");
  });

  it("merges targets that share labels and keeps discovery labels off static targets", () => {
    const artifact = buildDirectConfig([
      target({ target: "10.0.0.5:9100" }),
      target({ target: "10.0.0.6:9100" }),
    ]);
    const config = parse(artifact.yaml) as {
      scrape_configs: { static_configs: { targets: string[]; labels?: Record<string, string> }[] }[];
    };

    expect(config.scrape_configs).toHaveLength(1);
    const statics = config.scrape_configs[0]!.static_configs;
    expect(statics).toHaveLength(1);
    expect(statics[0]!.targets).toEqual(["10.0.0.5:9100", "10.0.0.6:9100"]);
    const labels = statics[0]!.labels ?? {};
    expect(labels).not.toHaveProperty("__scheme__");
    expect(labels).not.toHaveProperty("__metrics_path__");
    expect(labels).not.toHaveProperty("job");
  });

  it("never emits secret values for the discovery endpoint", () => {
    const artifact = buildHttpSdConfig(collectorUrl);
    const config = parse(artifact.yaml) as { scrape_configs: Record<string, unknown>[] };

    expect(JSON.stringify(config)).not.toMatch(/token|password|secret/i);
    expect(artifact.yaml).toContain("credentials_file: /etc/prometheus/piping-sd-token");
  });

  it("keeps one artifact name across modes", () => {
    expect(buildCollectorArtifact("http_sd", collectorUrl, []).fileName).toBe(
      buildCollectorArtifact("direct", collectorUrl, []).fileName,
    );
    expect(parse(buildCollectorArtifact("direct", collectorUrl, []).yaml)).toMatchObject({
      scrape_configs: [],
    });
  });
});
