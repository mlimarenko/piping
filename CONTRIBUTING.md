# Contributing to Piping

Piping accepts code contributions. Read the [license](LICENSE) and [CLA](CLA.md)
before submitting your work.

Report bugs, discuss changes and open pull requests in
[GitHub](https://github.com/mlimarenko/piping). Use `main` as the base for your work.

## Run the UI demo

Requires Node.js 26 and pnpm compatible with `apps/web/pnpm-lock.yaml`.

```sh
cd apps/web
pnpm install --frozen-lockfile
pnpm dev:demo
```

Open http://localhost:5174/demo-app/?lang=en. This is the real frontend with
synthetic inventory and session fixtures; it needs no backend or credentials.
Demo changes reset on reload.

## Develop against the API

The backend needs Docker, Python 3.12+ and the Rust toolchain specified in
the root `rust-toolchain.toml`. Start the API and its local PostgreSQL/Redis services from the
repository root:

```sh
make -C apps/api run
```

In another terminal, run `pnpm dev` from `apps/web` after installing its
dependencies. The UI serves on http://localhost:5173 and proxies `/api/v1/*`
to port 7000. Local sign-in is at `/auth/login` with the `admin` account.
Configure its password with `PIPING_AUTH__LOCAL_ADMIN_PASSWORD`; never commit
or share credential values. Weak development secrets are for local use only.

Backend defaults live in `apps/api/config/default.toml`, local overrides in
`apps/api/config/local.toml`, and environment overrides use `PIPING_*` names
with `__` between nested keys.

### Compose fixtures

`compose.yaml` defines the application stack. `compose-dev.yaml` adds optional
test services; it is not loaded by a normal `docker compose up`.

```sh
docker compose --env-file env/dev.env -f compose.yaml -f compose-dev.yaml --profile ssh-target up -d
```

Use the `netbox`, `keycloak` or `demo` profile for the corresponding integration
fixture. Configure private fixture credentials before running `make netbox-up`
or `make keycloak-up`. Do not use `env/dev.env` for a deployed installation.

## Checks and review

Run the relevant checks for your change; `make check` is the combined local gate.
Frontend checks run from `apps/web`:

```sh
pnpm lint
pnpm typecheck
pnpm test
pnpm build
```

`make install-hooks` installs the repository hooks. Include behavior tests for
code changes and keep one canonical implementation, without compatibility shims
or parallel paths. API changes must update the backend, OpenAPI contract,
frontend clients, tests and documentation together. Generate the contract with
`cargo run -p piping-backend --bin dump-openapi`.

## Inventory transfer

Build the CLI with
`cargo build -p piping-backend --bin inventory-transfer`. The backend image also
ships the CLI; its snapshot producer and pinned source contract are in
`/usr/local/share/piping/inventory-transfer/`. Override the image entrypoint when
running the CLI. Use `inventory-transfer --help` for flags and the compiled
catalog/tool digests.

- `export` seals a private NetBox 4.7.1 snapshot transport supplied through
  `--snapshot-transport`. The core-only producer is
  `apps/api/scripts/netbox_snapshot_export.py`; it reads one read-only database
  snapshot using explicit `NETBOX_DB_NAME`, `NETBOX_DB_USER`,
  `NETBOX_DB_PASSWORD`, `NETBOX_DB_HOST` and `NETBOX_DB_PORT`. It does not load
  source settings or plugins. Export requires the provider/operator identities
  and an independently endorsed provenance digest. A hash of untrusted input is
  not an endorsement.
- `capture-http` reads a NetBox 4.7.1 HTTP assessment using GET only. Explicit
  `--collection-grants` and `--authority-evidence` permit collection reads; they
  do not establish complete visibility or authorize writes. HTTP capture is not
  a replacement for an operator-endorsed snapshot.
- `preflight` checks the pinned bundle, mapping and profile offline. `dry-run`
  adds target assessment in a repeatable-read, read-only transaction. Use a
  dedicated SELECT-only database role for dry-run; the CLI does not create it.
  Offline-only `preflight` retains `target_graph_not_assessed` (exit 2) even when
  all source checks pass.
  A successful assessment does not authorize an import.
- `descriptor` observes the target and authenticates the operator, refreshing
  token usage metadata but not inventory. It requires a separate approval
  window and explicit scope, and pins the bundle, endorsed provenance, mapping,
  profile, catalogue, tool, schema and exact executable bytes.
- `apply` and `resume` are the only inventory-writing commands. They use the
  same pinned descriptor, live operator permissions, provider lease and durable
  database fence/receipt. They never contact NetBox, bootstrap the server, run
  migrations, seed data or start a schedule. Resume does not re-plan changed
  inputs; different pins require a new run. A descriptor prepared by a debug
  executable cannot be applied by a different release executable.

Connections and credentials come from the environment, never command-line
values: `INVENTORY_TRANSFER_NETBOX_TOKEN` for HTTP capture,
`INVENTORY_TRANSFER_DATABASE_URL` for target operations,
`INVENTORY_TRANSFER_REDIS_URL` for apply/resume, and
`INVENTORY_TRANSFER_OPERATOR_TOKEN` for descriptor/apply/resume. Keep input files,
JSON stdout and reports private; they may contain inventory or authority data.

The profile is the existing `PreflightProfile` with `format_version: 1`, bundle,
provider, mapping, catalogue and tool pins. Its `snapshot_authority` independently
pins the operator, provenance, issuer, scope and expectations. Pin the exact
mapping/profile bytes separately from the bundle being assessed. Mapping JSON
uses `MappingConfigDocument`, not another import format. Exit 0 means the command
completed without blockers, exit 2 means a completed refusal, blocked, stale,
busy or uncertain result, and exit 1 means an operational/input failure. No exit
code substitutes for the required authority and target verification.

For a synthetic NetBox rehearsal, use a fresh isolated Compose project, never
a customer instance or its volumes. Before `make netbox-up`, load a private
environment with `API_TOKEN_PEPPER_1` (at least 50 characters),
`NETBOX_SUPERUSER_API_KEY` (12 alphanumeric characters),
`NETBOX_SUPERUSER_API_TOKEN` (40 alphanumeric characters) and `NETBOX_API_TOKEN`
in the form `nbt_<key>.<token>`. Both NetBox processes share the pepper. Fixture
health, wait, seed and integration checks require the complete v2 credential,
with no raw-token or built-in fallback. The optional fixture binds its port only
to loopback; creating it does not rotate credentials or upgrade existing volumes.

Submit only work you own or have permission to contribute. Do not remove or
bypass licensing enforcement. The pull request description must confirm:
"I agree to the CLA".
