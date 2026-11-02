#!/usr/bin/env bash
set -euo pipefail

if ! command -v curl >/dev/null 2>&1; then
  echo "curl is required" >&2
  exit 1
fi
if ! command -v jq >/dev/null 2>&1; then
  echo "jq is required" >&2
  exit 1
fi
if ! command -v docker >/dev/null 2>&1; then
  echo "docker is required" >&2
  exit 1
fi

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
BUILDER_DIR="$(cd -- "${SCRIPT_DIR}/.." && pwd)"
cd "${BUILDER_DIR}"

source "$SCRIPT_DIR/license-e2e-fixture.sh"
license_fixture_app "$BUILDER_DIR"
BACKEND_URL="$LICENSE_BACKEND_URL"
license_fixture_cloud "$BUILDER_DIR"
PIPING_SPACE_URL="$LICENSE_PORTAL_URL"
ADMIN_IDENTIFIER="${ADMIN_IDENTIFIER:-admin}"
ADMIN_PASSWORD="${ADMIN_PASSWORD:-piping123}"
compose_cmd=("${LICENSE_COMPOSE[@]}")

tmp_dir="$(mktemp -d)"
admin_cookie="${tmp_dir}/admin.cookie"
portal_stopped=0
cleanup() {
  if [[ "$portal_stopped" == 1 ]]; then
    "${compose_cmd[@]}" start backend >/dev/null || true
  fi
  rm -rf "${tmp_dir}"
}
trap cleanup EXIT

echo "[outage] Login as product admin"
login_payload="$(jq -n --arg i "${ADMIN_IDENTIFIER}" --arg p "${ADMIN_PASSWORD}" '{identifier:$i,password:$p}')"
curl -fsS -c "${admin_cookie}" \
  -H 'Content-Type: application/json' \
  -d "${login_payload}" \
  "${BACKEND_URL}/api/v1/auth/login" >/dev/null

echo "[outage] Stop fixture portal"
"${compose_cmd[@]}" stop backend >/dev/null
portal_stopped=1

echo "[outage] Force refresh attempts while control-plane is down"
for _ in $(seq 1 6); do
  curl -sS -o /dev/null -w '' \
    -b "${admin_cookie}" \
    -X POST \
    "${BACKEND_URL}/api/v1/license/refresh" || true
  sleep 2
done

health_json="$(curl -fsS -b "${admin_cookie}" "${BACKEND_URL}/api/v1/license/health/details")"
status="$(echo "${health_json}" | jq -r '.status')"
if [[ "${status}" != "degraded" && "${status}" != "grace" && "${status}" != "expired" ]]; then
  echo "Expected degraded/grace/expired after outage, got '${status}'" >&2
  echo "${health_json}" >&2
  exit 1
fi
echo "[outage] State after outage: ${status}"

echo "[outage] Start fixture portal"
"${compose_cmd[@]}" start backend >/dev/null
portal_stopped=0

echo "[outage] Wait for com.backend readiness"
for _ in $(seq 1 30); do
  if curl -fsS "${PIPING_SPACE_URL}/readyz" >/dev/null 2>&1; then
    break
  fi
  sleep 1
done

echo "[outage] Trigger refresh and assert recovery to active"
active_ok="0"
for _ in $(seq 1 20); do
  curl -sS -o /dev/null -w '' \
    -b "${admin_cookie}" \
    -X POST \
    "${BACKEND_URL}/api/v1/license/refresh" || true
  latest_health="$(curl -fsS -b "${admin_cookie}" "${BACKEND_URL}/api/v1/license/health/details")"
  latest_status="$(echo "${latest_health}" | jq -r '.status')"
  if [[ "${latest_status}" == "active" ]]; then
    active_ok="1"
    break
  fi
  sleep 1
done

if [[ "${active_ok}" != "1" ]]; then
  echo "License state did not recover to active after control-plane restore" >&2
  exit 1
fi

echo "Outage simulation finished successfully."
