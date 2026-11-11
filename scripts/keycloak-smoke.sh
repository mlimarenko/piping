#!/usr/bin/env bash
set -euo pipefail

BASE_URL="${PIPING_KEYCLOAK_URL:-https://localhost}"
BASE_PATH="${PIPING_KEYCLOAK_BASE_PATH:-/keycloak}"
REALM="${PIPING_KEYCLOAK_REALM:-master}"
ADMIN_USERNAME="${PIPING_KEYCLOAK_ADMIN_USERNAME:-admin}"
ADMIN_PASSWORD="${PIPING_KEYCLOAK_ADMIN_PASSWORD:-admin}"

DISCOVERY_URL="${BASE_URL}${BASE_PATH}/realms/${REALM}/.well-known/openid-configuration"
echo "[keycloak-smoke] Checking discovery: ${DISCOVERY_URL}"
DISCOVERY_JSON=""
if ! DISCOVERY_JSON="$(curl -kfsS "${DISCOVERY_URL}")"; then
  FALLBACK_DISCOVERY_URL="${BASE_URL}${BASE_PATH}/realms/master/.well-known/openid-configuration"
  echo "[keycloak-smoke] Realm '${REALM}' not available yet, fallback to: ${FALLBACK_DISCOVERY_URL}"
  DISCOVERY_JSON="$(curl -kfsS "${FALLBACK_DISCOVERY_URL}")"
fi

TOKEN_ENDPOINT="$(echo "${DISCOVERY_JSON}" | jq -r '.token_endpoint // empty')"
if [ -z "${TOKEN_ENDPOINT}" ]; then
  echo "[keycloak-smoke] token_endpoint missing in discovery payload" >&2
  exit 1
fi

echo "[keycloak-smoke] Checking password grant token endpoint"
TOKEN_RESPONSE="$(
  curl -kfsS \
    -X POST \
    -H "Content-Type: application/x-www-form-urlencoded" \
    --data-urlencode "grant_type=password" \
    --data-urlencode "client_id=admin-cli" \
    --data-urlencode "username=${ADMIN_USERNAME}" \
    --data-urlencode "password=${ADMIN_PASSWORD}" \
    "${TOKEN_ENDPOINT}"
)"
echo "${TOKEN_RESPONSE}" | jq -e '.access_token | strings | length > 20' >/dev/null

echo "[keycloak-smoke] OK"
