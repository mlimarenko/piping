#!/bin/sh
set -eu

SSL_DIR=/etc/nginx/ssl
CERT_PATH="${SSL_DIR}/fullchain.pem"
KEY_PATH="${SSL_DIR}/privkey.pem"
GENERATED_DIR=/etc/nginx/generated
HTTPS_CONF="${GENERATED_DIR}/https.conf"
CLIENT_BODY_CONF="${GENERATED_DIR}/client-body-limits.conf"
DEFAULT_MAX_UPLOAD_BYTES=536870912
MULTIPART_OVERHEAD_BYTES=8388608

mkdir -p "${SSL_DIR}"
mkdir -p "${GENERATED_DIR}"

is_uint() {
    case "${1:-}" in
        ''|*[!0-9]*) return 1 ;;
        *) return 0 ;;
    esac
}

if [ -n "${PIPING_NGINX_CLIENT_MAX_BODY_SIZE:-}" ]; then
    client_max_body_size="${PIPING_NGINX_CLIENT_MAX_BODY_SIZE}"
else
    max_upload_bytes="${PIPING_PROXY_SSH_FILE_TRANSFER_MAX_UPLOAD_BYTES:-${DEFAULT_MAX_UPLOAD_BYTES}}"
    if ! is_uint "${max_upload_bytes}"; then
        echo "PIPING_PROXY_SSH_FILE_TRANSFER_MAX_UPLOAD_BYTES must be an unsigned byte count" >&2
        exit 1
    fi
    client_max_body_size=$((max_upload_bytes + MULTIPART_OVERHEAD_BYTES))
fi

case "${client_max_body_size}" in
    ''|*[!0-9kKmMgG]*)
        echo "PIPING_NGINX_CLIENT_MAX_BODY_SIZE must be an nginx size such as 545259520 or 640m" >&2
        exit 1
        ;;
esac

cat >"${CLIENT_BODY_CONF}" <<NGINX
client_max_body_size ${client_max_body_size};
client_body_timeout 600s;
proxy_request_buffering off;
NGINX

if [ ! -s "${CERT_PATH}" ] || [ ! -s "${KEY_PATH}" ]; then
    echo "TLS certificate/key required; run install.sh or supply a matching pair" >&2
    exit 1
fi

public_host="${PIPING_PUBLIC_HOST:-localhost}"
https_port="${PIPING_HTTPS_PORT:-443}"
if ! printf '%s\n' "$public_host" | grep -Eq '^([a-zA-Z0-9]([a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)(\.[a-zA-Z0-9]([a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)*$' || [ "${#public_host}" -gt 253 ]; then
    echo "Invalid PIPING_PUBLIC_HOST" >&2
    exit 1
fi
case "$https_port" in
    ''|0*|*[!0-9]*) echo "Invalid PIPING_HTTPS_PORT" >&2; exit 1 ;;
esac
if [ "${#https_port}" -gt 5 ] || [ "$https_port" -gt 65535 ]; then
    echo "Invalid PIPING_HTTPS_PORT" >&2
    exit 1
fi
authority="$public_host"
[ "$https_port" = 443 ] || authority="${public_host}:${https_port}"
printf 'set $piping_https_authority "%s";\n' "$authority" > "${GENERATED_DIR}/redirect.conf"

cat >"${HTTPS_CONF}" <<'NGINX'
server {
    listen 443 ssl;
    listen 443 quic reuseport;
    http2 on;
    http3 on;
    quic_retry on;
    server_name _;

    ssl_certificate /etc/nginx/ssl/fullchain.pem;
    ssl_certificate_key /etc/nginx/ssl/privkey.pem;
    ssl_protocols TLSv1.2 TLSv1.3;
    ssl_prefer_server_ciphers off;

    include /etc/nginx/locations/backend.conf;
    include /etc/nginx/locations/keycloak.conf;
    include /etc/nginx/locations/assets.conf;

    location / {
        proxy_pass http://piping_frontend;
        include /etc/nginx/proxy-common.conf;
    }
}
NGINX

# Native nginx validates the certificate/key pair and generated configuration.
nginx -t
