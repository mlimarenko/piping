# piping-ansible-runner: a minimal, HARDENED Ansible Execution Environment + Rust REST sidecar.
#
# Threat model: users can author free-form playbooks, and ansible-core evaluates
# ALL templating + lookups on the controller (= this container), so in-container
# code execution is treated as a given. The defense is CONTAINMENT, not prevention:
# non-root, no Linux capabilities, read-only rootfs + tmpfs, pids-limit, init reaper
# (the cap-drop / no-new-privileges / read_only / tmpfs / pids_limit knobs live in
# docker-compose.yml; this image makes them workable: non-root user, tini, and the
# ANSIBLE_* paths pointed at runtime-writable tmpfs).
#
# We build our own EE on python:slim because the upstream quay.io/ansible EE images
# require registry auth. ansible-core 2.20+ supports running the controller on
# Python 3.13/3.14; 2.18 was EOL. Galaxy collections are version-bounded in
# requirements.yml (an unpinned install is a supply-chain RCE on every rebuild).
FROM rust:1.98.1-bookworm AS builder
# Incremental artifacts are written into a layer that is then discarded — pure
# disk cost for a cold `--release` build that never reuses them. The shared
# runner has hit ENOSPC on concurrent builds, and apps/api/Dockerfile already
# sets this; this builder was simply missed.
ENV CARGO_INCREMENTAL=0
WORKDIR /workspace

COPY Cargo.toml Cargo.lock LICENSE ./
COPY crates/contracts/Cargo.toml crates/contracts/Cargo.toml
COPY apps/api/Cargo.toml apps/api/Cargo.toml
COPY apps/ansible-runner/Cargo.toml apps/ansible-runner/Cargo.toml
COPY crates/contracts crates/contracts
COPY apps/ansible-runner apps/ansible-runner
RUN cargo build --release -p piping-ansible-runner

############################
# Debian archive fingerprint
############################
# The registry build cache (the catalog sets cache-mode: registry) keys a RUN by
# its command text and parent layer, not by the Debian archive that layer read.
# Without an archive-derived key the apt layer below is reused indefinitely and
# the image keeps shipping package versions Debian has already superseded: on
# 2026-09-15 the published image carried 12 fixed HIGH/CRITICAL trixie findings.
# ADD re-checks a suite's Release file on every build, so this stage changes
# exactly when Debian publishes and the apt layer is invalidated with it.
# Nothing from this stage reaches the runtime image.
FROM scratch AS debian-archive
ADD https://deb.debian.org/debian/dists/trixie/Release /archive/trixie.Release
ADD https://deb.debian.org/debian/dists/trixie-updates/Release /archive/trixie-updates.Release
ADD https://deb.debian.org/debian-security/dists/trixie-security/Release /archive/trixie-security.Release
ADD https://deb.debian.org/debian/dists/trixie-backports/Release /archive/trixie-backports.Release

FROM python:3.14-slim AS runtime-base

# openssh-client + sshpass (password SSH); tini = a real init/PID-1 reaper because
# ansible spawns many ssh/sshpass children. WinRM/Kerberos libs come via pip.
COPY apps/ansible-runner/docker/apt/debian-backports.sources /etc/apt/sources.list.d/debian-backports.sources
COPY apps/ansible-runner/docker/apt/90-backports /etc/apt/preferences.d/90-backports
# The mount is the cache key, not an input: the Release files change exactly when
# a suite publishes, which is when this layer has to run again.
RUN --mount=type=bind,from=debian-archive,source=/archive,target=/tmp/debian-archive \
    echo "debian archive fingerprint:" \
    && cksum /tmp/debian-archive/*.Release \
    && apt-get update \
    && apt-get upgrade -y \
    && apt-get install -y --no-install-recommends openssh-client sshpass ca-certificates tini \
    && rm -rf /var/lib/apt/lists/*

# Named boundary: `docker build --target runtime-base` builds and scans just the
# operating-system layer, where the Trivy debian findings live.
FROM runtime-base

# Fixed-uid non-root runtime user (uid/gid 10001) so the compose `user:` + tmpfs
# ownership line up and container-root is never the process identity.
RUN groupadd -g 10001 runner \
    && useradd -u 10001 -g 10001 -m -d /home/runner -s /usr/sbin/nologin runner

WORKDIR /opt/piping-runner

COPY apps/ansible-runner/requirements.txt apps/ansible-runner/requirements.yml ./
RUN python -m pip install --no-cache-dir -r requirements.txt \
    && python -m pip uninstall --yes pip
# System collection path is on ansible's default search list and world-readable,
# so the non-root runtime user finds the collections without extra config.
RUN ansible-galaxy collection install -r requirements.yml -p /usr/share/ansible/collections
COPY apps/ansible-runner/collections/ansible_collections/piping/space /usr/share/ansible/collections/ansible_collections/piping/space
COPY --from=builder /workspace/target/release/piping-ansible-runner /usr/local/bin/piping-ansible-runner

# /var/lib/piping-runner (per-run private_data_dir) and the runner's $HOME/.ansible
# tree are tmpfs at runtime (see compose) — create the mountpoints owned by the
# runtime user so a read-only rootfs still has writable, executable scratch space.
RUN mkdir -p /var/lib/piping-runner /home/runner/.ansible/tmp /home/runner/.ansible/cp \
    && chown -R 10001:10001 /var/lib/piping-runner /home/runner /opt/piping-runner

ENV PIPING_ANSIBLE_RUNNER__DATA_DIR=/var/lib/piping-runner \
    HOME=/home/runner \
    ANSIBLE_HOME=/home/runner/.ansible \
    ANSIBLE_LOCAL_TEMP=/home/runner/.ansible/tmp \
    ANSIBLE_SSH_CONTROL_PATH_DIR=/home/runner/.ansible/cp \
    ANSIBLE_GALAXY_CACHE_DIR=/home/runner/.ansible/galaxy_cache

USER 10001:10001
EXPOSE 8080

# tini as PID 1: reaps the ssh/sshpass grandchildren ansible leaves behind.
ENTRYPOINT ["tini", "--"]
CMD ["/usr/local/bin/piping-ansible-runner"]
