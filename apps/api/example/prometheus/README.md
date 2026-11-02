# Prometheus / VictoriaMetrics examples

This folder contains ready-to-use examples for the monitoring endpoints:

- `GET /metrics`
- `GET /api/v1/monitoring/sd`

Default backend URL in examples: `http://backend:7000`.
Change it to your actual backend address if needed.

## Files

- `prometheus.metrics-only.yml`:
  scrape only backend app metrics.
- `prometheus.sd-default.yml`:
  simple HTTP SD for all hosts, one target-group per host.
- `prometheus.sd-grouped.yml`:
  grouped and filtered SD with richer relabeling.
- `prometheus.multi-jobs.yml`:
  multiple SD jobs for devices and virtual machines using exporter assignments
  from the dedicated monitoring domain.
- `prometheus.sd-secure-token.yml`:
  token-protected metrics and SD endpoints.
- `vmagent.sd.yml`:
  vmagent-compatible config using the same SD endpoint.

## Run examples

Prometheus:

```bash
prometheus --config.file=example/prometheus/prometheus.sd-default.yml
```

VictoriaMetrics vmagent:

```bash
vmagent -promscrape.config=example/prometheus/vmagent.sd.yml
```

## Notes

- SD endpoint returns Prometheus/VictoriaMetrics `http_sd_config` JSON:
  `[{ "targets": [...], "labels": {...} }]`.
- Scrape targets come from the dedicated monitoring model:
  `monitoring_exporter` plus `monitoring_exporter_assignment`.
  Assign exporters globally or to a site, tenant, role, host, device, or
  virtual machine. The most specific enabled assignment wins per exporter.
- The SD query only filters and formats the already configured monitoring
  model. Common params are `kind`, `target`, `group_by`, `site_id`,
  `tenant_id`, `role_id`, `status`, `search`, `limit`, `offset`, `label`, and
  `dedup`.
- UI metric previews come from `monitoring_metric_query`,
  `monitoring_query_source`, and `monitoring_metric_set_assignment`.
  Query sources default to polling every 60 seconds and retaining 5 samples.
- Add `authorization` under `http_sd_configs`; SD is token-protected unless
  `[monitoring].sd_public_enabled` / `PIPING_MONITORING__SD_PUBLIC_ENABLED` is
  explicitly set to `true`.
- Add scrape `authorization` when `PIPING_MONITORING__METRICS_TOKEN` is set.
