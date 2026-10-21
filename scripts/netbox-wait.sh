#!/usr/bin/env bash
set -euo pipefail

NETBOX_BASE_URL="${NETBOX_BASE_URL:-http://localhost:${NETBOX_PORT:-28080}}"
NETBOX_STATUS_URL="${NETBOX_BASE_URL%/}/api/status/"
NETBOX_API_TOKEN="${NETBOX_API_TOKEN:-${NETBOX_SUPERUSER_API_TOKEN:-netbox-admin-token}}"
NETBOX_WAIT_TIMEOUT_SECONDS="${NETBOX_WAIT_TIMEOUT_SECONDS:-300}"

echo "[netbox-wait] waiting for NetBox at ${NETBOX_STATUS_URL}"

deadline=$((SECONDS + NETBOX_WAIT_TIMEOUT_SECONDS))
while [ "${SECONDS}" -lt "${deadline}" ]; do
  if curl -fsS -H "Authorization: Token ${NETBOX_API_TOKEN}" "${NETBOX_STATUS_URL}" >/dev/null 2>&1; then
    echo "[netbox-wait] NetBox API is ready"
    exit 0
  fi
  sleep 2
done

echo "[netbox-wait] timeout waiting for NetBox readiness (${NETBOX_WAIT_TIMEOUT_SECONDS}s)"
exit 1
