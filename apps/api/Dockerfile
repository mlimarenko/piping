# syntax=docker/dockerfile:1.7
# Build context: workspace root (apps/api/Dockerfile referenced by docker-compose).

############################
# Debian archive fingerprint
############################
# The registry build cache keys a RUN by its command text and parent layer, not
# by the Debian archive that layer read. Without an archive-derived key the apt
# layers below are reused indefinitely and the image keeps shipping package
# versions the archive has already superseded — the defect proven on
# ansible-runner in PWA-44 (12 fixed HIGH/CRITICAL findings shipped). ADD
# re-checks a suite's Release file on every build, so this stage changes exactly
# when Debian publishes and the apt layers are invalidated with it. Nothing from
# this stage reaches any image.
FROM scratch AS debian-archive
ADD https://deb.debian.org/debian/dists/trixie/Release /archive/trixie.Release
ADD https://deb.debian.org/debian/dists/trixie-updates/Release /archive/trixie-updates.Release
ADD https://deb.debian.org/debian-security/dists/trixie-security/Release /archive/trixie-security.Release
ADD https://deb.debian.org/debian/dists/trixie-backports/Release /archive/trixie-backports.Release

############################
# Builder
############################
FROM rust:1.98.1-trixie AS builder

COPY docker/apt/debian-backports.sources /etc/apt/sources.list.d/debian-backports.sources
COPY docker/apt/90-backports /etc/apt/preferences.d/90-backports
# The bind mount is the cache key, not an input: the Release files change
# exactly when a suite publishes, which is when this layer has to run again.
RUN --mount=type=bind,from=debian-archive,source=/archive,target=/tmp/debian-archive \
    echo "debian archive fingerprint:" \
    && cksum /tmp/debian-archive/*.Release \
    && apt-get update \
    && apt-get upgrade -y \
    && apt-get install -y --no-install-recommends pkg-config libssl-dev ca-certificates clang openssl \
    && rm -rf /var/lib/apt/lists/*

ENV CARGO_HOME=/usr/local/cargo \
    CARGO_TARGET_DIR=/app/target-docker-trixie \
    CARGO_INCREMENTAL=0 \
    CARGO_NET_GIT_FETCH_WITH_CLI=true

WORKDIR /app

# Copy the whole source tree once. BuildKit cache mounts keep `target/` and
# the cargo registry warm between builds — better reuse than manual manifest
# layer priming.
COPY . .

ARG CACHEBUST=0
ARG VERSION
ENV VERSION="${VERSION}"
# License JWT verification public key, baked into the binary at compile time.
# Release builds fail to compile unless this is provided (fail-closed): the only
# way to ship an image is to bake a key. An empty arg must NOT set the env (the
# release const treats Some("") as "provided"), so it is exported only when
# non-empty.
ARG PIPING_LICENSE_BAKED_PUBLIC_KEY_PEM
ARG PIPING_LICENSE_BAKED_PUBLIC_KEY_PEM_B64
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,target=/app/target-docker-trixie,sharing=locked \
    echo "Cache bust: ${CACHEBUST}" \
  && bash scripts/ci-rust-release.sh build --release -p piping-backend --bin piping-backend \
 && mkdir -p /out \
 && cp "${CARGO_TARGET_DIR}/release/piping-backend" /out/piping-backend

############################
# guacd builder
############################
FROM debian:trixie-slim AS guacd-builder

ARG GUACAMOLE_SERVER_VERSION=1.6.0

WORKDIR /tmp/guacd-src

# FreeRDP 3.x on Debian 13 treats deprecated APIs as errors; Guacamole 1.6.0
# still references them — see GUACAMOLE-2146.
ENV CPPFLAGS="-Wno-error=deprecated-declarations"

COPY docker/apt/debian-backports.sources /etc/apt/sources.list.d/debian-backports.sources
COPY docker/apt/90-backports /etc/apt/preferences.d/90-backports
RUN --mount=type=bind,from=debian-archive,source=/archive,target=/tmp/debian-archive \
    echo "debian archive fingerprint:" \
    && cksum /tmp/debian-archive/*.Release \
    && apt-get update \
    && apt-get upgrade -y \
    && apt-get install -y --no-install-recommends \
      build-essential autoconf automake libtool pkg-config \
      ca-certificates curl \
      libcairo2-dev libjpeg62-turbo-dev libpng-dev libossp-uuid-dev uuid-dev \
      libavcodec-dev libavformat-dev libavutil-dev libswscale-dev libswresample-dev \
      libpango1.0-dev \
      libssl-dev libpulse-dev libvorbis-dev libwebp-dev \
      freerdp3-dev \
    && rm -rf /var/lib/apt/lists/*

RUN curl -fsSL -o guacamole-server.tar.gz \
      "https://downloads.apache.org/guacamole/${GUACAMOLE_SERVER_VERSION}/source/guacamole-server-${GUACAMOLE_SERVER_VERSION}.tar.gz" \
    || curl -fsSL -o guacamole-server.tar.gz \
      "https://archive.apache.org/dist/guacamole/${GUACAMOLE_SERVER_VERSION}/source/guacamole-server-${GUACAMOLE_SERVER_VERSION}.tar.gz"

RUN tar -xzf guacamole-server.tar.gz --strip-components=1

RUN ./configure --with-rdp \
    && make -j"$(nproc)" \
    && make install \
    && ldconfig

############################
# Runtime
############################
FROM debian:trixie-slim AS runtime-base

ARG VERSION

LABEL org.opencontainers.image.title="piping-backend" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.source="https://github.com/mlimarenko/piping"

WORKDIR /app

COPY docker/apt/debian-backports.sources /etc/apt/sources.list.d/debian-backports.sources
COPY docker/apt/90-backports /etc/apt/preferences.d/90-backports
RUN --mount=type=bind,from=debian-archive,source=/archive,target=/tmp/debian-archive \
    echo "debian archive fingerprint:" \
    && cksum /tmp/debian-archive/*.Release \
    && apt-get update \
    && apt-get upgrade -y \
    && apt-get install -y --no-install-recommends \
      ca-certificates curl git iputils-ping openssh-client libssl3t64 \
      ffmpeg \
      libfreerdp-client3-3 libfreerdp3-3 libwinpr3-3 \
      libcairo2 libjpeg62-turbo libpng16-16t64 libossp-uuid16 \
      libpango-1.0-0 libpulse0 libvorbis0a libwebp7 libwebpdemux2 libwebpmux3 \
      libavcodec61 libavformat61 libavutil59 libswscale8 libswresample5 \
    && rm -rf /var/lib/apt/lists/*

# Named boundary: `docker build --target runtime-base` builds the operating-system
# layer alone, where the Trivy debian findings live, without the Rust and guacd
# builder stages.
FROM runtime-base AS runtime

COPY --from=guacd-builder /usr/local/sbin/guacd /usr/local/sbin/guacd
COPY --from=guacd-builder /usr/local/lib/ /usr/local/lib/
COPY --from=guacd-builder /usr/lib/x86_64-linux-gnu/freerdp3/ /usr/lib/x86_64-linux-gnu/freerdp3/
COPY --from=builder /out/piping-backend /usr/local/bin/piping-backend
COPY --from=builder /app/apps/api/config ./config
COPY --from=builder /app/apps/api/migrations ./migrations
COPY --from=builder /app/ansible/playbooks ./ansible/playbooks
COPY --from=builder /app/apps/api/entrypoint.sh /usr/local/bin/entrypoint.sh

RUN ldconfig \
    && chmod +x /usr/local/bin/entrypoint.sh \
    && groupadd --system piping \
    && useradd --system --gid piping --create-home --home-dir /home/piping piping \
    && chown -R piping:piping /app

EXPOSE 8080

HEALTHCHECK --interval=10s --timeout=3s --retries=6 \
  CMD port="${PIPING_SERVER__BIND##*:}"; [ "$port" = "$PIPING_SERVER__BIND" ] && port=8080; curl -fsS "http://localhost:${port:-8080}/health" || exit 1

USER piping

ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
