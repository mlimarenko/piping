#!/bin/sh
set -eu

template=/etc/nginx/piping/default.conf.template
output=/etc/nginx/conf.d/default.conf
default_max_upload_bytes=536870912
multipart_overhead_bytes=8388608

is_uint() {
    case "${1:-}" in
        ''|*[!0-9]*) return 1 ;;
        *) return 0 ;;
    esac
}

if [ -n "${PIPING_NGINX_CLIENT_MAX_BODY_SIZE:-}" ]; then
    client_max_body_size="${PIPING_NGINX_CLIENT_MAX_BODY_SIZE}"
else
    max_upload_bytes="${PIPING_PROXY_SSH_FILE_TRANSFER_MAX_UPLOAD_BYTES:-${default_max_upload_bytes}}"
    if ! is_uint "${max_upload_bytes}"; then
        echo "PIPING_PROXY_SSH_FILE_TRANSFER_MAX_UPLOAD_BYTES must be an unsigned byte count" >&2
        exit 1
    fi
    client_max_body_size=$((max_upload_bytes + multipart_overhead_bytes))
fi

case "${client_max_body_size}" in
    ''|*[!0-9kKmMgG]*)
        echo "PIPING_NGINX_CLIENT_MAX_BODY_SIZE must be an nginx size such as 545259520 or 640m" >&2
        exit 1
        ;;
esac

sed "s/__PIPING_NGINX_CLIENT_MAX_BODY_SIZE__/${client_max_body_size}/g" "${template}" >"${output}"
