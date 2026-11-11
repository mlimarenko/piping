# Piping

Self-hosted infrastructure inventory, browser-based remote sessions and Ansible automation.

[piping.space](https://piping.space) · [Install](#install) · [Contribute](CONTRIBUTING.md) · [License](LICENSE)

[Explore Piping](https://piping.space) — a scroll-driven product tour, followed by the [interactive demo](https://piping.space/demo).

Russian-language film. The interface uses synthetic demo data; sessions are simulated.

## Features

- Inventory for devices, virtual machines, sites and IP addresses
- [Configuration contexts](apps/api/docs/config-contexts.md) selected by inventory scopes and tags
- Connections Workspace with tree, card, table and map views and a shared inspector
- Browser-based SSH, RDP and VNC sessions with role and policy controls
- Ansible playbook runs against inventory targets
- NetBox imports and synchronization
- Prometheus-compatible HTTP service discovery
- MCP and API access with scoped tokens, including explicitly permitted SSH
  actions using the configured infrastructure identity and audit log

## Try the interface

The demo uses the real frontend with synthetic data. [Run it locally](CONTRIBUTING.md#run-the-ui-demo)
without a backend or credentials. Changes reset on reload.

## Telemetry

The Compose stack sends backend traces, metrics and logs, plus anonymous UI
usage and performance metrics, to `https://otel.piping.space` by default.
Set `PIPING_TELEMETRY_ENABLED=false` in the installation's `.env` and recreate
the backend (and proxy, if used) to disable both channels. Reload open browser
tabs to apply the UI setting. A saved browser opt-out remains effective.

## Install

Requires Linux/amd64, Bash 4+, Docker with Compose v2, OpenSSL, Python 3.12+,
GNU coreutils (`realpath` and `nproc`) and util-linux (`flock` and `findmnt`). Your user must
be able to run Docker.

Fresh installation also requires the local `/var/run/docker.sock` and root or
passwordless `sudo` for assigning two new private directories to UID/GID 10001.
The installation path may contain only letters, digits, underscore, dot, slash
and hyphen. The installer records the socket GID and creates `ansible-state` and
`ansible-jobs` under the installation directory with mode 700. It does not alter
ownership of existing data. If preparation fails, inspect the partial installation
and its directories before recovery; do not reuse or recursively chown old data.
Jobs have no network by default, so SSH and other network access will not work
until an administrator explicitly authorizes a separate Docker job network.

```sh
curl -fsSL https://raw.githubusercontent.com/mlimarenko/piping/1.0.0/install.sh | bash
```

The installer downloads the release and its public container images, installs
into `~/piping` and serves Piping at **https://localhost**. The initial self-signed
certificate needs browser trust; use a trusted certificate for a shared service.
Find the administrator password in the installation's private `.env` file.
Keep that file and the installation's data safe; rerunning the installer cannot
recover lost secrets.

Run `~/piping/install.sh --help` for hostname, port, bind, resource and image options.
Published ports listen on all interfaces by default. To place the stack behind an
existing local reverse proxy and keep PostgreSQL and Redis off other networks,
select explicit IPv4 bind addresses:

```sh
./install.sh --hostname piping.example.com --http-port 8080 --https-port 8443 \
  --edge-bind 192.0.2.10 --frontend-bind 192.0.2.10 \
  --postgres-port 15432 --postgres-bind 127.0.0.1 \
  --redis-port 16379 --redis-bind 127.0.0.1
```

The selected addresses are written to the installation's `.env`; later starts and
guarded updates use them. A bind address must be a dotted IPv4 address.

On a host shared with other workloads, select resource limits that fit the
remaining capacity with `--small-cpus`, `--small-memory`, `--medium-cpus`,
`--medium-memory`, `--large-cpus`, `--large-memory`, `--runner-cpus` and
`--runner-memory`. CPU defaults never exceed the host's processor count; choose
memory limits that fit the memory left by co-resident services. The selected
limits are also written to `.env`.
Memory limits must cover the existing reservations: 128M for small services,
512M for medium services, and 1G for large services and the runner. Smaller limits
are rejected before the installation is prepared.

Rerunning the installed script starts the configured stack; it does **not**
automatically upgrade images. Explicit image updates use the guarded backup and
migration flow. If an update fails, inspect its private backup and journal before
attempting recovery.

Existing installations are not converted by rerunning the installer. The guarded
updater, `scripts/deploy_release_bundle.py`, supports explicit runner adoption
through `stage --runner-candidate DIRECTORY`. Supply a private directory (700)
containing only `.env` and the installed Compose input, normally
`compose.yaml` (600), copied from the installed
configuration. Change only the sandbox runner settings and these deployment keys:
`PIPING_DOCKER_GID`, `PIPING_ANSIBLE_STATE_ROOT`, `PIPING_ANSIBLE_JOB_ROOT` and
`PIPING_ANSIBLE_JOB_NETWORK`. The resolved candidate must preserve other services,
secrets, existing mounts and runner settings. Do not replace customer Compose
configuration with the repository template.

Adoption requires the local Docker socket, its numeric group, and dedicated
`ansible-state` and `ansible-jobs` directories directly under the installation
root. Missing directories are created with mode 700 and ownership 10001:10001
using root or passwordless `sudo`; existing directories are checked, never
recursively changed. The candidate must leave the job network empty. Enabling an
authorized job network is a separate administrative change, not part of adoption.
The updater detects the existing saved `startup.compose.json` through installed
writer labels and seals its bytes. Only its label-only ownership shape is
supported. Both resolved Compose models are sealed; later interpolation or
`env_file` drift is refused. Installed configuration remains unchanged until
authenticated `apply`.

Before stopping writers, the updater checks the retained runner against the
original resolved Compose service and image defaults, including its environment,
command, user, security settings, mounts, network identities and startup labels.
Unapplied Compose edits are refused; the updater does not derive a replacement
recovery configuration from a mismatched container. Runner recovery requires
explicit bind or plain named volumes, not unrepresented anonymous image volumes.

### Guarded job network

After adoption, an administrator can authorize a separate Docker job network.
The native deny foundation, replaceable allow policy, network identity and
systemd units stay installation-owned under `<root>/job-network`. Activate them
through the updater; do not edit the installed Compose file, the runner restart
value or `.env`:

```sh
python3 scripts/deploy_release_bundle.py activate-job-network \
  --root /opt/docker/piping --project-name piping \
  --network piping-job-printum --bridge pj-printum
```

The action starts the installed guard and policy units, verifies both native nft
tables and the recorded Docker network identity, and records
`job-network/activation.json` plus the product-owned
`job-network/guarded.compose.yml`. It recreates the runner stopped, records the
new immutable container ID in `job-network/runner-unit`, and starts only that
unit. Every later Compose invocation includes the overlay, so the effective
restart stays `"no"` and `PIPING_SANDBOX_DOCKER_NETWORK` stays the recorded
network even when a recovery restores older `.env` or Compose bytes.

Rerunning `install.sh`, guarded updates, failed-update recovery, explicit starts
and runner recreation refuse before starting the runner when the activation
record, nft foundation, policy, units or network identity is absent, inactive or
mismatched. The shared Docker daemon and neighboring containers are never
restarted or stopped. An activated runner starts through its immutable-ID
systemd unit; `restart=no` blocks Docker's own restore path but does not block an
explicit Compose start, so the updater performs the preflight itself.

New installations use `compose.yaml`. The updater binds the explicitly selected
Compose input to the installed writer labels. For an existing installation with
a different filename, pass that exact name through `--compose-file`; it is not
guessed or renamed. The updater never rewrites the installed Compose file or a
retained startup overlay.

Activation does not create the network or write per-customer
`site.nft`/`endpoints.nft`; those native inputs follow the authorized
installation record. Guarded activation, deactivation, updates and recovery
that start or stop the immutable-ID unit run with root privileges; ordinary
network-disabled installations keep their existing non-root Docker-group path.

To return to a network-disabled runner, run
`deactivate-job-network` with the same root and project name. That action stops
and disables the recorded unit, removes the activation record and overlay, and
recreates and starts the runner without the overlay only after checking the
effective job network is empty. If an earlier procedure put the network name in
`.env`, clear that key first; deactivation refuses a non-empty effective job
network. Remove the nft foundation only after the network
has no endpoints.

### Stopped-writer automation cutover

For the exact 0015-to-0016 workspace transition, keep the original recovery source
**R** separate from the prepared database **C**:

1. Stage the release and runner candidate while the old writers are running.
   `capture-cutover-r` observes the backend's actual PID1 environment, records the
   exact writer identities and start epochs, stops them, fences PostgreSQL, and
   exports R with the old images and configuration.
2. Copy the private dossier to the independent signer and run `verify-backup`.
   For R, this proves native restore and conservation without applying 0016.
   Return the signed receipt to `accept-cutover-r --receipt FILE
   --receipt-signature FILE`. Acceptance requires the original database to remain
   unchanged. Retain `cutover-r.dump`, its manifest and signed proof.
3. Only after that proof, an authorized operator may remove the retired automation
   rows and their `playbook_version:runtime_env` / `playbook_project` secret-usage
   bindings with narrow native PostgreSQL operations. The updater performs no
   cleanup and grants no retirement authority. Do not restart the old writers.
4. Run `backup` on the same stage. It authenticates the retained R proof, rejects
   restarted or replaced writers, proves the access fence, and checks that cleanup
   preserved all other rows, bindings, schema and migration history. It captures C
   in `offline-pre-upgrade.dump`. Independently verify C with `verify-backup`, then
   pass that C receipt to `apply`. Package, image, schema and conservation guards
   remain in force.

Ordinary backups include a full `backup-manifest.json` recovery set. After an
interrupted apply, `restore-backup` restores **C** with its matching old images and
configuration and leaves writers stopped. Before apply has consumed a receipt,
provide its `--receipt` and `--receipt-signature` explicitly. `restore-cutover-r`
instead restores retained pre-cleanup **R**; it is not FHRP recovery. Both retain
the failed database under a quarantine name. If C capture fails in `dumping`,
`restore-cutover-r` remains available through the same authenticated R, configuration
and access-fence checks; this does not authorize retrying an incomplete C backup.
Startup is a separate explicit
`start-backup` or `start-cutover-r` action. Starting the old application on C can
recreate retired automation; recover R when restoring original availability.

### Offline map archive

The frontend normally serves its shipped archive at `/maps/world.pmtiles`.
Use a vector PMTiles archive with Protomaps Basemap v4 layers compatible with the
bundled style, and preserve its attribution.
To use an instance-owned archive, back up the installation's `compose.yaml`
and add this bind under `services.frontend.volumes`, using an absolute path to
the archive on the Docker host:

```yaml
- type: bind
  source: /absolute/path/to/world.pmtiles
  target: /usr/share/nginx/html/maps/world.pmtiles
  read_only: true
  bind:
    create_host_path: false
```

The source must exist as a regular file readable by frontend UID 101. Verify its
expected SHA-256 before use. `create_host_path: false` makes a missing source
fail container creation instead of falling back to the image asset. Keep the
archive and base Compose configuration in place across frontend image updates.
For replacement, stage and verify a file on the same filesystem, atomically
rename it into place, then explicitly recreate the frontend: an existing bind
retains the previous inode. Verify the served archive after recreation. To roll
back the first adoption, restore the original Compose configuration and retained
frontend image, then recreate the frontend. Do not change the configuration or
archive during a guarded update.

Existing plaintext FHRP keys require the explicit [guarded preparation and recovery
procedure](apps/api/migrations/README.md#explicit-guarded-preparation-and-recovery),
not an ordinary backup or startup.

[Development and contributions](CONTRIBUTING.md) · [Desktop client](apps/desktop/README.md) · [License](LICENSE)
