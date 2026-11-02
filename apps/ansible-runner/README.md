# piping-ansible-runner

Rust REST sidecar that supervises native `ansible-playbook` inside the Piping
Ansible execution image. It is the execution primitive for Piping playbooks:
Ansible remains the runtime, while this service owns process lifecycle,
credential materialization, event callbacks and cancellation.

## Why a sidecar

The platform needs to launch one grouped Ansible job for large target sets,
stream progress back to Piping, and terminate the whole process tree reliably.
The previous wrapper layer added overhead and hid too much behavior. The Rust
sidecar keeps the boundary small:
backend resolves the job, Rust supervises the native `ansible-playbook` process,
and Ansible itself handles playbooks, inventories, collections and transport
semantics.

## Flow

1. Backend lease worker resolves a target set, connection configs, credentials,
   runtime variables and callback URL.
2. Backend sends one `POST /jobs` with `Authorization: Bearer <shared-secret>`.
   The payload contains one inventory for the whole run group and `targets[]`
   with per-target run IDs and callback tokens.
3. The sidecar creates a group-scoped private data dir, materializes either the
   legacy inline playbook or a multi-file project bundle, starts an in-memory
   `ssh-agent` when an SSH key is present, then runs one native
   `ansible-playbook -i inventory/hosts -e @env/extravars project/<entrypoint>`.
4. Stdout/stderr are parsed into bounded, batched run-group callbacks:
   `{callback_base_url}/api/v1/playbooks/run-groups/events/batch`.
5. Terminal status for every target is sent through
   `{callback_base_url}/api/v1/playbooks/run-groups/{group_id}/completion-batch`.
6. Credentials are cleaned up on every exit path. SSH private keys are loaded
   into a per-run in-memory agent and are never written to the data directory.

Auth is bearer-only: the runner accepts the shared secret as an
`Authorization: Bearer <secret>` header and no other scheme, and the backend
client sends it that way.

## API

- `POST /jobs` -> `202 {dispatch_id, run_group_id, dispatch_attempt}`
- `GET /dispatches/{dispatch_id}` -> the durable receipt and current runner state
- `POST /dispatches/{dispatch_id}/cancel` -> the same current receipt, idempotently
- `GET /health`

The request/response contract is defined by
`piping_contracts::playbook_runner`. The runner rejects unknown
`protocol_version` values before launching Ansible.

`RunnerJobRequest.playbook` remains the compatibility path for single-file
launches. AWX-like launches should use `project_files[]` plus `entrypoint` so
roles, templates, files, `group_vars` and `host_vars` are materialized as a real
Ansible project tree. The runner validates relative paths, rejects traversal and
does not allow project bundles to override generated controller files such as
`ansible.cfg`.

## Event policy

The runner preserves normal `ansible-playbook` console output as the primary
diagnostic stream. It enriches host-specific result lines with the matching
Piping `run_id`, suppresses low-signal continuation data in compact mode, and
sends events in batches so callback I/O does not dominate large fan-out runs.

Do not make JSON stdout the default. Ansible supports only one stdout callback,
and forcing JSON would replace the familiar console view for user-authored
playbooks. Structured UI should be derived from the runner callback stream and
from explicit opt-in Ansible settings where needed.

## Environment

- `PIPING_ANSIBLE_RUNNER__SHARED_SECRET` - required backend-to-runner secret.
- `PIPING_ANSIBLE_RUNNER__BIND` - listen address, default `0.0.0.0:8080`.
- `PIPING_ANSIBLE_RUNNER__DATA_DIR` - private data root, default
  `/var/lib/piping-runner`.
- `PIPING_ANSIBLE_RUNNER__CALLBACK_TIMEOUT_S` - callback request timeout,
  default `15`.
- `PIPING_ANSIBLE_RUNNER__EVENT_QUEUE_SIZE` - queued event bound, default
  `50000`.
- `PIPING_ANSIBLE_RUNNER__EVENT_BATCH_SIZE` - max callback batch size, default
  `1000`.
- `PIPING_ANSIBLE_RUNNER__EVENT_BATCH_FLUSH_MS` - max batch flush delay, default
  `100`.
- `PIPING_ANSIBLE_RUNNER__EVENT_DRAIN_TIMEOUT_S` - best-effort event drain
  window on process exit, default `120`.
- `PIPING_ANSIBLE_RUNNER__RESULT_FORCE_COMPACT_PAYLOAD_BYTES` - result payload
  compaction threshold, default `4000`.
- `PIPING_ANSIBLE_RUNNER__KEEP_DATA_DIR` - keep run data dirs for debugging,
  default `false`.
- `PIPING_ANSIBLE_RUNNER__DEFAULT_JOB_TIMEOUT_S` - job timeout applied when the
  backend does not send `settings.job_timeout`, default `3600`.
- `PIPING_ANSIBLE_RUNNER__OUTPUT_DRAIN_GRACE_S` - how long to keep reading
  stdout/stderr after `ansible-playbook` exits before force-closing pipes held
  open by leftover child processes, default `10`.
- `PIPING_ANSIBLE_RUNNER__DISPATCH_RECEIPT_RETENTION_S` - how long durable
  dispatch receipts are kept before pruning, default `2592000` (30 days).
- `PIPING_ANSIBLE_RUNNER__GALAXY_CACHE_DIR` - galaxy collection/role cache that
  survives the per-job data-dir wipe, default `<data_dir>/_galaxy_cache`
  (`/var/lib/piping-runner/_galaxy_cache`).
- `PIPING_ANSIBLE_RUNNER__GALAXY_INSTALL_TIMEOUT_S` - wall-clock budget for the
  whole `ansible-galaxy` dependency install step, default `600`.

The `ansible-playbook` process does not inherit the runner environment. Only an
explicit allowlist passes through (`PATH`, `HOME`, `LANG`, `TZ`, `TMPDIR` plus
the `ANSIBLE_*`, `PYTHON*` and `LC_*` prefixes); everything else - including
`PIPING_ANSIBLE_RUNNER__*` control-plane variables - is stripped, and per-job
values come from typed settings, the job ssh-agent and the request
`playbook_env`.

The runner sets transport-safe process defaults such as `ANSIBLE_FORCE_COLOR`
and per-job settings supplied by backend. Output policy, forks, timeout,
strategy, tags, limits and related behavior are modeled as typed runner
settings; raw `cmdline` injection is intentionally unsupported.

By default the runner emits `console_line` events for raw stdout/stderr before
the structured projection events used by host/result views. Set
`raw_stdout_passthrough=false` only for deliberately compact, high-volume runs
where an exact console stream is less important than callback volume.

## Runtime policy for controller-side playbooks

A playbook version can opt into run-bound API access and runtime secrets via
`token_policy.runtime`:

```json
{
  "ttl_seconds": 3600,
  "scopes": ["artifact:create"],
  "runtime": {
    "piping_api": { "enabled": true, "inventory": ["read", "create", "update"] },
    "secrets": { "env": { "EXTERNAL_API_TOKEN": "external_api_token" } }
  }
}
```

`runtime.piping_api.inventory` is accepted only when
`runtime.piping_api.enabled` is `true`. `runtime.secrets.env` maps environment
variable names to existing Piping secret-variable keys. Values are resolved by
the backend for the current run target and are never stored in playbook
metadata.

## Tests

```bash
cargo test -p piping-ansible-runner
docker compose build ansible-runner
```

## Local smoke

```bash
docker compose build ansible-runner
docker compose up -d ansible-runner

curl -s -X POST localhost:8080/jobs \
  -H "Authorization: Bearer $SECRET" \
  -H 'Content-Type: application/json' \
  -d '{
    "protocol_version":4,
    "dispatch_id":"00000000-0000-0000-0000-000000000998",
    "dispatch_attempt":1,
    "run_group_id":"00000000-0000-0000-0000-000000000999",
    "group_callback_token":"group-token",
    "lease_owner":"w",
    "callback_base_url":"http://host.docker.internal:9999",
    "playbook":"- hosts: localhost\n  connection: local\n  gather_facts: false\n  tasks:\n    - debug: msg=hello\n",
    "inventory":"localhost ansible_connection=local\n",
    "targets":[{
      "alias":"localhost",
      "run_id":"00000000-0000-0000-0000-000000000001",
      "job_token":"t",
      "target_kind":"connection_target",
      "target_id":"00000000-0000-0000-0000-000000000002",
      "source_kind":"device",
      "source_id":"00000000-0000-0000-0000-000000000003"
    }],
    "extravars":{}
  }'
```
