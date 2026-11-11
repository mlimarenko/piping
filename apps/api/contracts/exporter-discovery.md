# Exporter discovery result contract

Discovery answers "what exporter exists here"; availability answers "can it be
scraped now". They are separate observations of the same endpoint and never
overwrite each other. The collector submits bounded evidence to the monitoring
owner, which stores the report and reconciles source-owned assignments.

## Owners

| Concern | Owner |
| --- | --- |
| Host identity and addresses | `DiscoveryHost` in `apps/api/src/contexts/monitoring/domain/host.rs:82-118` |
| Exporter type vocabulary | `monitoring_exporter.exporter_type`; validation in `apps/api/src/api/rest/routes/monitoring/exporters.rs:182-234` |
| Assignment and scrape defaults | `ProfileAssignment` in `apps/api/src/contexts/monitoring/infrastructure/persistence.rs:20-49`; `effective_profile_assignments_for_host` in `application/read_model.rs:488-516` |
| SD target rendering, dedup and order | `build_discovered_targets` and `build_target_groups` in `apps/api/src/api/rest/routes/monitoring/discovery.rs:1202-1425`; `deduplicate_targets` in `api/rest/routes/monitoring/helpers.rs:46` |
| SSRF policy and URL provenance | `apps/api/src/contexts/monitoring/application/source_resolution.rs:165-333` |
| Job token scopes and run binding | `apps/api/src/contexts/playbooks/application/playbook_service.rs:70-118`; `JobTokenAudience` in `contexts/playbooks/domain/playbook.rs:594-644` |
| Inventory writeback | `apps/api/docs/collected-facts.md` (unchanged by this contract) |

Discovery reuses the monitoring catalog, assignments and SD endpoint. It adds
no parallel service discovery, inventory model or physical-hardware entities.

## Endpoint identity

An endpoint is identified by the exact tuple

```
(host_id, address, port, scheme, path)
```

- `host_id` is the inventory object identity (`DiscoveryHost.host_id`), not a
  display name. `host_name` is a label and never matches an object.
- `address` is the target string the endpoint is scraped at: `host_address`,
  `primary_ip4`, `primary_ip6`, an `interface_addresses` address or a
  `tunnel_addresses` address. `address_source` records which one, using the
  existing `EXPORTER_DEFAULT_TARGET_SOURCES` vocabulary
  (`host_address`, `host_name`, `host_id`, `interface_addresses`,
  `tunnel_addresses`; `exporters.rs:109-115`).
- `port` is 1..=65535; the existing catalog validation rejects everything else
  (`exporters.rs:198-204`).
- `scheme` is `http` or `https` (`validate_assignment_scheme`,
  `exporters.rs:167-180`).
- `path` is the normalized metrics path with a leading slash
  (`normalize_metrics_path`, `api/rest/routes/monitoring/helpers.rs:10-19`).

Two ports or paths on one host are different endpoints. The same tuple observed
by both sources is one endpoint with two observations; the source is evidence,
not identity.

### Exporter type

`exporter_type` is a value from the existing exporter catalog vocabulary, or
null. Type is established only from declared or machine identifiers:

1. A matching catalog assignment or profile item declares the type.
2. Host inspection observes a Linux systemd unit, process executable, container
   image, or Windows service and executable, and resolves it through an explicit
   typed association owned by the exporter catalog.
3. An endpoint probe alone never establishes type. A response without a declared
   type leaves `exporter_type` null and the status `unverified`.

The association in (2) contains protocol and model identifiers only. Product
names, localized descriptions, response-body keywords and other free text are
not a type classifier; no natural-language routing is added. Where the
association is stored and how it is versioned is a PIP-562 design choice, not
fixed by this document.

## Observation

Each endpoint observation carries:

| field | meaning |
| --- | --- |
| `source` | `host_inspection` or `endpoint_probe`; the two are enabled independently |
| `platform` | `linux` or `windows` for host inspection; null for probes |
| `observed_at` | RFC3339 UTC time the evidence was produced, not report assembly time |
| `provenance` | `run_id`, `run_group_id`, `connection_target_id`, `source_kind`, `source_id`, `observed_from` (`runner` for host inspection, `backend` for HTTP probes) |
| `proof` | bounded structured evidence, below; `proof.origin` is `configured_assignment`, `host_listener`, `host_process`, `host_service`, `host_container` or `endpoint_http` |
| `status` | one of the six statuses below |
| `reason` | non-empty code from the status vocabulary |

`proof` never contains secret values. For host inspection it records the
listener (`address`, `port`, `tcp`/`udp`), the owning identity (unit, service,
image or executable basename), and inspection coverage (`complete` or
`partial`). For an endpoint probe it records connect result, HTTP status,
content type, body size, redirect decision, TLS verification and duration. It
never records process environment values, process argument values, response
bodies, URL userinfo or query parameters. Redaction follows `redacted_url`
(`source_resolution.rs:148-163`) and the runner secret mask.

## Statuses

| status | means | may come from |
| --- | --- | --- |
| `confirmed_discovered` | presence proven: a listener owned by a typed exporter, or a metrics response on the declared endpoint | host inspection, probe |
| `unverified` | something was observed but presence or type is not provable: unrecognized listener owner, or an HTTP response that is not a metrics response | host inspection, probe |
| `unreachable` | the probe could not connect: refused, timeout, TLS failure, or a rejected redirect | probe only |
| `confirmed_absent` | positive absence: a complete host inspection observed the host's listening sockets and found no listener for the declared endpoint, and the endpoint address is observable from that host | host inspection only |
| `skipped` | the source was disabled, the platform is unsupported, the target is out of scope, or permission is insufficient | either |
| `error` | the observation itself failed: inspection command error, parse error, truncated or partial result | either |

`unreachable` is not absence. A failed or partial inspection is `error` or
`skipped`, never `confirmed_absent`. One HTTP failure never proves absence.
The ingestion endpoint runs HTTP probes from the Piping backend; neither that
result nor the runner's host inspection proves Prometheus scraper reachability.

`reason` is a stable snake_case token, for example `no_listener`,
`complete_inspection`, `connection_refused`, `timeout`, `tls_verification_failed`,
`redirect_not_followed`, `unrecognized_owner`, `not_metrics_response`,
`source_disabled`, `unsupported_platform`, `out_of_scope`, `permission_denied`,
`inspection_failed` or `parse_failed`. Empty or free-form reason text is
invalid.

## Report

The report is one run artifact with `schema_version` (starting at `1`),
`run_id`, `run_group_id`, `started_at`, `finished_at`, the enabled sources,
`endpoints[]` and `groups[]`.

`groups[]` is the per-group outcome: one entry per host identity with
`host_id`, `host_name`, `kind`, counts by status, the reason codes present, and
`partial` when any observation is `error` or `skipped`. Counts must add up to
`endpoints[]`; a group with `partial` cannot yield `confirmed_absent` for any of
its endpoints.

Ordering is deterministic and does not change SD output: groups by `host_name`
then `host_id`; endpoints by identity tuple with numeric port order and path
after port. Existing SD ordering stays as it is: `exporter_slug`, assignment
`id`, `item_sort_order`, `item_id` (`read_model.rs:506-515`), with existing
dedup by target (`helpers.rs:46`) unchanged. A discovery run does not reorder
targets, mark a cached host stale, or rewrite the existing `piping_*` SD labels
and exporter/item labels; the report reuses that label vocabulary and adds no
new label keys.

## Capabilities and run binding

Discovery runs as the existing system playbook job, under the run's connection
and target set. The sandbox job token is the published E2 contract:

- audience `sandbox`, minted per run and bound to `run_id`, `target_kind`,
  `target_id`, `source_kind`, `source_id` (`playbook.rs:633-644`);
- scopes are restricted to the closed allowlist in `playbook_service.rs:110-118`
  (`artifact:create`, `event:append`, `status:update`, `inventory:read`,
  `inventory:create`, `inventory:update`);
- the token is active, not revoked and not expired; the same run, run group,
  target and source binding is re-checked on write, as the collected-facts path
  already does.

The discovery report uses `artifact:create`. Host inspection uses the run's
connection credentials and gains no inventory or monitoring authority from them.
No new token scope is introduced here.

The collector reads `PIPING_API_URL` and the private mode-0600 JSON token map
named by `PIPING_RUN_TOKENS_FILE`. It selects the sandbox token by the host's
`piping_run_id`, not by host order or a shared token. The group comes from
`PIPING_RUN_GROUP_ID`; target and source identities come from the same host's
runtime inventory variables. Missing context, a missing run token or a
non-private token file stops the collector before sending a report. Tokens are
not launch parameters and are not included in the report or run summary.

Reconciliation is a monitoring write and requires the current run owner's
monitoring permission. `artifact:create` alone grants neither reconciliation
nor general monitoring CRUD. Run, group, target and source binding still apply;
revoked access is not replaced by the job's earlier launch authority.

## Bounds

Every numeric bound is either an existing native value or an explicit design
choice for the implementing task; none is approved by this document.

| dimension | bound | source |
| --- | --- | --- |
| port | 1..=65535 | `exporters.rs:198-204` |
| scheme | `http`, `https` | `exporters.rs:167-180` |
| path | leading `/`, default `/metrics` | `helpers.rs:10-19` |
| targets per run | <= 50 000 | `PLAYBOOK_RUN_MAX_TARGET_LIMIT` |
| run wall time | <= 86 400 s; inventory collector catalog entry 90 s | `PLAYBOOK_RUN_MAX_TIMEOUT_SECONDS`; `piping-playbooks.yaml` |
| probe timeout | 3 000 ms default, minimum 100 ms | `DEFAULT_MONITORING_QUERY_TIMEOUT_MS`; `source_resolution.rs:81-84` |
| job token TTL | 3 600 s default, 86 400 s maximum | `playbook_service.rs:72-73` |
| SD page size | <= `monitoring.sd_max_limit` (default 50 000) | `DEFAULT_SD_MAX_LIMIT` |
| report artifact | <= 2 097 152 bytes, name <= 240 chars, type <= 120 chars | `PLAYBOOK_ARTIFACT_TEXT_MAX_BYTES` and neighbours |
| artifacts per run | <= 64 | `RUN_CALLBACK_ARTIFACT_LIMIT` |
| probes per endpoint and concurrency | design choice for PIP-563; concurrency may use the catalog `forks` value | not approved here |
| per-host endpoint cap and report truncation policy | design choice for PIP-564 | not approved here |

Truncation is explicit in the report; a truncated report cannot be treated as a
complete inspection for absence.

Disabling host inspection marks only that source as skipped. Enabled HTTP probes
still run against eligible endpoints. A whole-host skip or exclusion suppresses
both sources; neither case proves absence.

## Scope and network safety

Host inspection touches only the run's inventory targets, over the run's
connection (Linux SSH with explicit opt-in become; Windows WinRM/PSRP and
OpenSSH). It does not enumerate ports and does not inspect unrelated hosts.

Probes touch only endpoints derived from the target's own inventory addresses or
from configured assignment targets. There is no CIDR sweep, no port range scan
and no caller-supplied host. Before connecting, the address is resolved and
pinned with the existing monitoring policy: `localhost`/`metadata` names and
cloud-metadata addresses are always blocked, and private or internal addresses
are allowed only with the same admin-created-source provenance used for
monitoring query sources (`source_resolution.rs:165-333`, `shared/ssrf.rs`).

Redirects are not followed to another origin; a redirect is recorded and leaves
the endpoint `unverified` with reason `redirect_not_followed`. TLS certificates
are verified; verification failure is `unreachable`. Host inspection still
requires the runner's authorized network and connection transport. Endpoint
probes use the backend network; they do not bypass the runner's network policy.

## Service-discovery lifecycle policy

The existing job variables schema owns the policy settings. The collector
includes them in the discovery report request, and the monitoring owner applies
one policy implementation. There is no separate monitoring settings store.

| job variable | default | meaning |
| --- | --- | --- |
| `piping_discovery_absence_policy` | `manual` | `automatic` explicitly enables exclusion and restoration |
| `piping_discovery_absence_confirm_runs` | `3` | consecutive complete confirmed absences, from 1 to 50 |
| `piping_discovery_restore_on_reappearance` | `true` | permits restoration only while the mode is `automatic` |

These typed parameters use the normal job, launch and schedule variable flow.
The run summary reports the selected mode, confirmation window and restore gate.

`manual` is the default for new and existing settings. A `confirmed_absent`
observation records the absence in history and leaves the SD output unchanged.
Reappearance clears the mark and records history; SD output was never changed.

`automatic` is off by default and requires explicit opt-in with validated
conditions. It may change only assignments whose provenance is this discovery
source; manual assignments, manually edited labels and foreign hosts are never
changed. Exclusion requires `confirmed_absent` on the configured number of
consecutive complete runs; `unreachable`, `unverified`, `error`, `skipped` and
any `partial` group never count. Restoration requires automatic mode,
`piping_discovery_restore_on_reappearance=true` and `confirmed_discovered` for
the same endpoint identity; an `unreachable` endpoint does not restore by itself.
Objects and history are preserved, never deleted. Switching back to `manual`
stops automatic changes, including restoration of an excluded assignment, and
keeps history for recovery.

Because the absence of a runner-observed endpoint is not the Prometheus
scraper's view, automatic exclusion applies only to source-owned assignments and
only on the confirmed-absence evidence above.
