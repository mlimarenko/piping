# syntax=docker/dockerfile:1.7

############################
# Alpine index fingerprint
############################
# The registry build cache keys a RUN by its command text and parent layer, not
# by the Alpine package index that layer read. Without an index-derived key the
# `apk upgrade` in the runtime stage below is reused indefinitely and the image
# keeps shipping package versions Alpine has already superseded — proven on this
# image on 2026-09-25: libexpat 2.8.4-r0 shipped although 2.8.5-r0 had been
# published on 2026-09-23, because the cached upgrade layer never re-resolved
# (CVE-2026-93990). `ADD` re-checks the index on every build, so this stage
# changes exactly when Alpine publishes and the upgrade layer is invalidated
# with it. The URLs mirror the Alpine release of the base image below; move them
# together when the base image's Alpine major/minor moves. Nothing from this
# stage reaches any image.
FROM scratch AS alpine-index
ADD https://dl-cdn.alpinelinux.org/alpine/v3.24/main/x86_64/APKINDEX.tar.gz /index/main-APKINDEX.tar.gz
ADD https://dl-cdn.alpinelinux.org/alpine/v3.24/community/x86_64/APKINDEX.tar.gz /index/community-APKINDEX.tar.gz

FROM node:26-alpine AS builder
WORKDIR /app/apps/web
ARG VERSION
ENV VITE_APP_VERSION=${VERSION}
RUN <<'SH'
node --input-type=module <<'JS'
const numeric = "(?:0|[1-9][0-9]*)";
const prerelease = `(?:${numeric}|[0-9]*[A-Za-z-][0-9A-Za-z-]*)`;
const metadata = "[0-9A-Za-z-]+(?:\\.[0-9A-Za-z-]+)*";
// The final lookahead also rejects a trailing newline, which $ alone accepts.
const semver = new RegExp(`^v?${numeric}\\.${numeric}\\.${numeric}(?:-${prerelease}(?:\\.${prerelease})*)?(?:\\+${metadata})?$(?![\\s\\S])`);
if (!semver.test(process.env.VITE_APP_VERSION ?? "")) {
  console.error("VERSION must be a valid release version");
  process.exit(1);
}
JS
SH
RUN npm install -g corepack \
 && corepack enable \
 && corepack prepare pnpm@11.22.0 --activate
COPY apps/web/package.json apps/web/pnpm-lock.yaml apps/web/pnpm-workspace.yaml ./
RUN --mount=type=cache,target=/root/.local/share/pnpm/store \
    pnpm install --frozen-lockfile
COPY apps/web/ ./
COPY apps/api/contracts/inventory-template-references.json /app/apps/api/contracts/inventory-template-references.json
RUN pnpm run build

# Unprivileged nginx: runs as uid 101 (nginx), listens on 8080, and needs no
# root — the image drops the master process' root and pre-creates the temp/pid
# paths owned by that uid. Bind ports stay >1024 so no CAP_NET_BIND_SERVICE.
FROM nginxinc/nginx-unprivileged:1.30-alpine AS runtime
# The base image already defaults to uid 101 (see the USER note below), so
# apk needs an explicit root switch here or it cannot open its own lock/log
# files. Pull patched OS packages within this Alpine release: the base image
# tag is floating and does not always carry the newest security fixes
# (e.g. CVE-2026-93990 in libexpat, fixed upstream in 2.8.5-r0) at build time.
USER root
RUN --mount=type=bind,from=alpine-index,source=/index,target=/tmp/alpine-index \
    echo "alpine index fingerprint:" \
    && cksum /tmp/alpine-index/*.tar.gz \
    && apk upgrade --no-cache
ARG VERSION
LABEL org.opencontainers.image.title="piping-frontend" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.source="https://github.com/mlimarenko/piping"
COPY apps/web/nginx.conf /etc/nginx/piping/default.conf.template
COPY apps/web/docker-entrypoint.d/90-render-nginx-config.sh /docker-entrypoint.d/90-render-nginx-config.sh
COPY --from=builder /app/apps/web/dist /usr/share/nginx/html
# The base image already defaults to this uid; stating it explicitly keeps the
# guarantee from silently regressing if the base image ever changes its default,
# and makes the non-root runtime visible to image scanners.
USER 101
EXPOSE 8080
HEALTHCHECK --interval=10s --timeout=3s --retries=6 \
    CMD wget -q --spider http://127.0.0.1:8080/ || exit 1
