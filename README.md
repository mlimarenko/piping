# Piping

Self-hosted infrastructure inventory, browser-based remote sessions and Ansible automation.

[piping.space](https://piping.space) · [Install](#install) · [Contribute](CONTRIBUTING.md) · [License](LICENSE)

[![Watch the Piping product film](https://piping.space/media/piping-poster.jpg)](https://piping.space/media/piping-overview.mp4)

[Watch the MP4 product film](https://piping.space/media/piping-overview.mp4) · [Try the interactive demo](https://piping.space/demo)

Russian-language film. The interface uses synthetic demo data; sessions are simulated.

## Features

- Inventory for devices, virtual machines, sites and IP addresses
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

## Install

Requires Linux/amd64, Bash 4+, Docker with Compose v2, OpenSSL, Python 3.12+,
GNU coreutils (`realpath`) and util-linux (`flock` and `findmnt`). Your user must
be able to run Docker.

```sh
curl -fsSL https://raw.githubusercontent.com/mlimarenko/piping/1.0.2/install.sh | bash
```

The installer downloads the release and its public container images, installs
into `~/piping` and serves Piping at **https://localhost**. The initial self-signed
certificate needs browser trust; use a trusted certificate for a shared service.
Find the administrator password in the installation's private `.env` file.
Keep that file and the installation's data safe; rerunning the installer cannot
recover lost secrets.

Run `~/piping/install.sh --help` for hostname, port and image options.
Rerunning the installed script starts the configured stack; it does **not**
automatically upgrade images. Explicit image updates use the guarded backup and
migration flow. If an update fails, inspect its private backup and journal before
attempting recovery.

[Development and contributions](CONTRIBUTING.md) · [Desktop client](apps/desktop/README.md) · [License](LICENSE)
