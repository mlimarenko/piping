#!/usr/bin/env bash
set -euo pipefail

for cmd in curl jq; do
  if ! command -v "${cmd}" >/dev/null 2>&1; then
    echo "[keycloak-bootstrap] Missing required command: ${cmd}" >&2
    exit 1
  fi
done

BASE_URL="${PIPING_KEYCLOAK_URL:-https://localhost}"
BASE_PATH="${PIPING_KEYCLOAK_BASE_PATH:-/keycloak}"
ADMIN_USERNAME="${PIPING_KEYCLOAK_ADMIN_USERNAME:-admin}"
ADMIN_PASSWORD="${PIPING_KEYCLOAK_ADMIN_PASSWORD:-admin}"
REALM="${PIPING_KEYCLOAK_REALM:-piping}"
CLIENT_ID="${PIPING_KEYCLOAK_CLIENT_ID:-piping-web}"
ROTATE_CLIENT_SECRET="${PIPING_KEYCLOAK_ROTATE_CLIENT_SECRET:-false}"
PRINT_SECRETS="${PIPING_KEYCLOAK_PRINT_SECRETS:-true}"
CREATE_TEST_USERS="${PIPING_KEYCLOAK_CREATE_TEST_USERS:-true}"

REDIRECT_HTTPS="${PIPING_KEYCLOAK_REDIRECT_URI_HTTPS:-https://localhost/api/v1/auth/sso/callback}"
REDIRECT_HTTP="${PIPING_KEYCLOAK_REDIRECT_URI_HTTP:-http://localhost/api/v1/auth/sso/callback}"
WEB_ORIGIN_HTTPS="${PIPING_KEYCLOAK_WEB_ORIGIN_HTTPS:-https://localhost}"
WEB_ORIGIN_HTTP="${PIPING_KEYCLOAK_WEB_ORIGIN_HTTP:-http://localhost}"

BASE_GROUP_NAME="${PIPING_KEYCLOAK_GROUP_BASE:-piping}"
ADMIN_GROUP_NAME="${PIPING_KEYCLOAK_GROUP_ADMIN:-piping-admin}"
EDITOR_GROUP_NAME="${PIPING_KEYCLOAK_GROUP_EDITOR:-piping-editor}"
VIEWER_GROUP_NAME="${PIPING_KEYCLOAK_GROUP_VIEWER:-piping-viewer}"

ADMIN_ROLE_NAME="${PIPING_KEYCLOAK_ROLE_ADMIN:-admin}"
EDITOR_ROLE_NAME="${PIPING_KEYCLOAK_ROLE_EDITOR:-editor}"
VIEWER_ROLE_NAME="${PIPING_KEYCLOAK_ROLE_VIEWER:-viewer}"

TEST_ADMIN_USERNAME="${PIPING_KEYCLOAK_TEST_ADMIN_USERNAME:-kc-admin}"
TEST_ADMIN_PASSWORD="${PIPING_KEYCLOAK_TEST_ADMIN_PASSWORD:-kc-admin-2026}"
TEST_ADMIN_EMAIL="${PIPING_KEYCLOAK_TEST_ADMIN_EMAIL:-kc-admin@example.local}"

TEST_USER_USERNAME="${PIPING_KEYCLOAK_TEST_USER_USERNAME:-kc-user}"
TEST_USER_PASSWORD="${PIPING_KEYCLOAK_TEST_USER_PASSWORD:-kc-user-2026}"
TEST_USER_EMAIL="${PIPING_KEYCLOAK_TEST_USER_EMAIL:-kc-user@example.local}"

API_ROOT="${BASE_URL%/}${BASE_PATH}"
INTERNAL_BASE="${PIPING_KEYCLOAK_INTERNAL_URL:-http://keycloak:8080}"
INTERNAL_ISSUER="${INTERNAL_BASE%/}${BASE_PATH}/realms/${REALM}"
PUBLIC_ISSUER="${BASE_URL%/}${BASE_PATH}/realms/${REALM}"

uri_encode() {
  jq -rn --arg v "$1" '$v|@uri'
}

keycloak_admin_token() {
  curl -kfsS \
    -X POST \
    -H "Content-Type: application/x-www-form-urlencoded" \
    --data-urlencode "grant_type=password" \
    --data-urlencode "client_id=admin-cli" \
    --data-urlencode "username=${ADMIN_USERNAME}" \
    --data-urlencode "password=${ADMIN_PASSWORD}" \
    "${API_ROOT}/realms/master/protocol/openid-connect/token" \
    | jq -r '.access_token // empty'
}

TOKEN="$(keycloak_admin_token)"
if [ -z "${TOKEN}" ]; then
  echo "[keycloak-bootstrap] Failed to obtain admin access token" >&2
  exit 1
fi

kc_get() {
  local url="$1"
  curl -kfsS \
    -H "Authorization: Bearer ${TOKEN}" \
    -H "Content-Type: application/json" \
    "${url}"
}

kc_post() {
  local url="$1"
  local payload="$2"
  curl -kfsS \
    -X POST \
    -H "Authorization: Bearer ${TOKEN}" \
    -H "Content-Type: application/json" \
    --data "${payload}" \
    "${url}" >/dev/null
}

kc_put() {
  local url="$1"
  local payload="$2"
  curl -kfsS \
    -X PUT \
    -H "Authorization: Bearer ${TOKEN}" \
    -H "Content-Type: application/json" \
    --data "${payload}" \
    "${url}" >/dev/null
}

realm_status="$(curl -ks -o /dev/null -w '%{http_code}' -H "Authorization: Bearer ${TOKEN}" "${API_ROOT}/admin/realms/${REALM}")"
if [ "${realm_status}" = "404" ]; then
  echo "[keycloak-bootstrap] Creating realm '${REALM}'"
  realm_payload="$(jq -n --arg realm "${REALM}" '{enabled: true, realm: $realm}')"
  kc_post "${API_ROOT}/admin/realms" "${realm_payload}"
else
  echo "[keycloak-bootstrap] Realm '${REALM}' already exists"
fi

find_client_uuid() {
  local encoded
  encoded="$(uri_encode "${CLIENT_ID}")"
  kc_get "${API_ROOT}/admin/realms/${REALM}/clients?clientId=${encoded}" \
    | jq -r '.[0].id // empty'
}

CLIENT_UUID="$(find_client_uuid)"
if [ -z "${CLIENT_UUID}" ]; then
  echo "[keycloak-bootstrap] Creating client '${CLIENT_ID}'"
  client_payload="$(
    jq -n \
      --arg cid "${CLIENT_ID}" \
      --arg redirectHttps "${REDIRECT_HTTPS}" \
      --arg redirectHttp "${REDIRECT_HTTP}" \
      --arg webOriginHttps "${WEB_ORIGIN_HTTPS}" \
      --arg webOriginHttp "${WEB_ORIGIN_HTTP}" \
      '{
        baseUrl: "/",
        clientId: $cid,
        directAccessGrantsEnabled: false,
        enabled: true,
        frontchannelLogout: true,
        name: "Piping Web",
        protocol: "openid-connect",
        publicClient: false,
        redirectUris: [$redirectHttps, $redirectHttp],
        serviceAccountsEnabled: false,
        standardFlowEnabled: true,
        webOrigins: [$webOriginHttps, $webOriginHttp],
        attributes: {
          "pkce.code.challenge.method": "S256"
        }
      }'
  )"
  kc_post "${API_ROOT}/admin/realms/${REALM}/clients" "${client_payload}"
  CLIENT_UUID="$(find_client_uuid)"
fi

if [ -z "${CLIENT_UUID}" ]; then
  echo "[keycloak-bootstrap] Failed to resolve client id for '${CLIENT_ID}'" >&2
  exit 1
fi

client_representation="$(kc_get "${API_ROOT}/admin/realms/${REALM}/clients/${CLIENT_UUID}")"
client_representation="$(
  echo "${client_representation}" | jq \
    --arg redirectHttps "${REDIRECT_HTTPS}" \
    --arg redirectHttp "${REDIRECT_HTTP}" \
    --arg webOriginHttps "${WEB_ORIGIN_HTTPS}" \
    --arg webOriginHttp "${WEB_ORIGIN_HTTP}" \
    '.enabled = true
    | .protocol = "openid-connect"
    | .publicClient = false
    | .standardFlowEnabled = true
    | .directAccessGrantsEnabled = false
    | .serviceAccountsEnabled = false
    | .frontchannelLogout = true
    | .baseUrl = "/"
    | .redirectUris = [$redirectHttps, $redirectHttp]
    | .webOrigins = [$webOriginHttps, $webOriginHttp]
    | .attributes = ((.attributes // {}) + {"pkce.code.challenge.method": "S256"})'
)"
kc_put "${API_ROOT}/admin/realms/${REALM}/clients/${CLIENT_UUID}" "${client_representation}"

mapper_payload='{
  "name":"groups",
  "protocol":"openid-connect",
  "protocolMapper":"oidc-group-membership-mapper",
  "config":{
    "access.token.claim":"true",
    "claim.name":"groups",
    "full.path":"true",
    "id.token.claim":"true",
    "jsonType.label":"String",
    "userinfo.token.claim":"true"
  }
}'
existing_mapper_id="$(
  kc_get "${API_ROOT}/admin/realms/${REALM}/clients/${CLIENT_UUID}/protocol-mappers/models" \
    | jq -r '.[] | select(.name == "groups" and .protocolMapper == "oidc-group-membership-mapper") | .id' \
    | head -n1
)"
if [ -z "${existing_mapper_id}" ]; then
  echo "[keycloak-bootstrap] Creating groups protocol mapper for client '${CLIENT_ID}'"
  kc_post "${API_ROOT}/admin/realms/${REALM}/clients/${CLIENT_UUID}/protocol-mappers/models" "${mapper_payload}"
else
  echo "[keycloak-bootstrap] Updating groups protocol mapper for client '${CLIENT_ID}'"
  mapper_payload="$(echo "${mapper_payload}" | jq --arg id "${existing_mapper_id}" '.id = $id')"
  kc_put "${API_ROOT}/admin/realms/${REALM}/clients/${CLIENT_UUID}/protocol-mappers/models/${existing_mapper_id}" "${mapper_payload}"
fi

ensure_client_role() {
  local role_name="$1"
  local description="$2"
  local encoded
  encoded="$(uri_encode "${role_name}")"

  local role_status
  role_status="$(curl -ks -o /dev/null -w '%{http_code}' -H "Authorization: Bearer ${TOKEN}" "${API_ROOT}/admin/realms/${REALM}/clients/${CLIENT_UUID}/roles/${encoded}")"
  if [ "${role_status}" = "404" ]; then
    echo "[keycloak-bootstrap] Creating client role '${role_name}'"
    kc_post "${API_ROOT}/admin/realms/${REALM}/clients/${CLIENT_UUID}/roles" \
      "$(jq -n --arg name "${role_name}" --arg description "${description}" '{name: $name, description: $description}')"
  elif [ "${role_status}" != "200" ]; then
    echo "[keycloak-bootstrap] Failed to inspect client role '${role_name}' (HTTP ${role_status})" >&2
    exit 1
  fi
}

assign_client_role_to_group() {
  local group_id="$1"
  local group_name="$2"
  local role_name="$3"
  local encoded_role
  encoded_role="$(uri_encode "${role_name}")"

  local assigned
  assigned="$(
    kc_get "${API_ROOT}/admin/realms/${REALM}/groups/${group_id}/role-mappings/clients/${CLIENT_UUID}" \
      | jq -r --arg role "${role_name}" '.[] | select(.name == $role) | .name' \
      | head -n1
  )"
  if [ -n "${assigned}" ]; then
    return
  fi

  local role_payload
  role_payload="$(kc_get "${API_ROOT}/admin/realms/${REALM}/clients/${CLIENT_UUID}/roles/${encoded_role}")"
  echo "[keycloak-bootstrap] Mapping group '${group_name}' to client role '${role_name}'"
  kc_post "${API_ROOT}/admin/realms/${REALM}/groups/${group_id}/role-mappings/clients/${CLIENT_UUID}" "[${role_payload}]"
}

ensure_group() {
  local group_name="$1"
  local encoded
  encoded="$(uri_encode "${group_name}")"
  local group_id
  group_id="$(
    kc_get "${API_ROOT}/admin/realms/${REALM}/groups?search=${encoded}" \
      | jq -r --arg name "${group_name}" '.[] | select(.name == $name) | .id' \
      | head -n1
  )"
  if [ -z "${group_id}" ]; then
    kc_post "${API_ROOT}/admin/realms/${REALM}/groups" "$(jq -n --arg name "${group_name}" '{name: $name}')"
    group_id="$(
      kc_get "${API_ROOT}/admin/realms/${REALM}/groups?search=${encoded}" \
        | jq -r --arg name "${group_name}" '.[] | select(.name == $name) | .id' \
        | head -n1
    )"
  fi
  if [ -z "${group_id}" ]; then
    echo "[keycloak-bootstrap] Failed to resolve group id for '${group_name}'" >&2
    exit 1
  fi
  echo "${group_id}"
}

ensure_user() {
  local username="$1"
  local email="$2"
  local password="$3"
  local group_id="$4"
  local encoded_username
  encoded_username="$(uri_encode "${username}")"

  local user_id
  user_id="$(
    kc_get "${API_ROOT}/admin/realms/${REALM}/users?username=${encoded_username}&exact=true" \
      | jq -r '.[0].id // empty'
  )"

  if [ -z "${user_id}" ]; then
    local create_payload
    create_payload="$(
      jq -n \
        --arg email "${email}" \
        --arg username "${username}" \
        '{email: $email, emailVerified: true, enabled: true, username: $username}'
    )"
    kc_post "${API_ROOT}/admin/realms/${REALM}/users" "${create_payload}"
    user_id="$(
      kc_get "${API_ROOT}/admin/realms/${REALM}/users?username=${encoded_username}&exact=true" \
        | jq -r '.[0].id // empty'
    )"
  fi

  if [ -z "${user_id}" ]; then
    echo "[keycloak-bootstrap] Failed to resolve user id for '${username}'" >&2
    exit 1
  fi

  local password_payload
  password_payload="$(jq -n --arg value "${password}" '{temporary: false, type: "password", value: $value}')"
  kc_put "${API_ROOT}/admin/realms/${REALM}/users/${user_id}/reset-password" "${password_payload}"
  curl -kfsS \
    -X PUT \
    -H "Authorization: Bearer ${TOKEN}" \
    "${API_ROOT}/admin/realms/${REALM}/users/${user_id}/groups/${group_id}" >/dev/null
}

BASE_GROUP_ID="$(ensure_group "${BASE_GROUP_NAME}")"
ADMIN_GROUP_ID="$(ensure_group "${ADMIN_GROUP_NAME}")"
EDITOR_GROUP_ID="$(ensure_group "${EDITOR_GROUP_NAME}")"
VIEWER_GROUP_ID="$(ensure_group "${VIEWER_GROUP_NAME}")"

ensure_client_role "${ADMIN_ROLE_NAME}" "Full administrative access"
ensure_client_role "${EDITOR_ROLE_NAME}" "Workspace editing and connection access without RBAC administration"
ensure_client_role "${VIEWER_ROLE_NAME}" "Read-only portal access"

assign_client_role_to_group "${BASE_GROUP_ID}" "${BASE_GROUP_NAME}" "${VIEWER_ROLE_NAME}"
assign_client_role_to_group "${VIEWER_GROUP_ID}" "${VIEWER_GROUP_NAME}" "${VIEWER_ROLE_NAME}"
assign_client_role_to_group "${EDITOR_GROUP_ID}" "${EDITOR_GROUP_NAME}" "${EDITOR_ROLE_NAME}"
assign_client_role_to_group "${ADMIN_GROUP_ID}" "${ADMIN_GROUP_NAME}" "${ADMIN_ROLE_NAME}"

if [ "${CREATE_TEST_USERS}" = "true" ]; then
  ensure_user "${TEST_ADMIN_USERNAME}" "${TEST_ADMIN_EMAIL}" "${TEST_ADMIN_PASSWORD}" "${ADMIN_GROUP_ID}"
  ensure_user "${TEST_USER_USERNAME}" "${TEST_USER_EMAIL}" "${TEST_USER_PASSWORD}" "${BASE_GROUP_ID}"
fi

if [ "${ROTATE_CLIENT_SECRET}" = "true" ]; then
  echo "[keycloak-bootstrap] Rotating client secret for '${CLIENT_ID}'"
  curl -kfsS \
    -X POST \
    -H "Authorization: Bearer ${TOKEN}" \
    "${API_ROOT}/admin/realms/${REALM}/clients/${CLIENT_UUID}/client-secret" >/dev/null
fi

CLIENT_SECRET="$(
  kc_get "${API_ROOT}/admin/realms/${REALM}/clients/${CLIENT_UUID}/client-secret" \
    | jq -r '.value // empty'
)"

if [ "${PRINT_SECRETS}" = "true" ]; then
  client_secret_line="Client Secret: ${CLIENT_SECRET}"
  ui_client_secret_line="  5. Client Secret: ${CLIENT_SECRET}"
else
  client_secret_line="Client Secret: <hidden; fetch from Keycloak client credentials>"
  ui_client_secret_line="  5. Client Secret: <hidden; fetch from Keycloak client credentials>"
fi

cat <<EOF
[keycloak-bootstrap] Done
Realm: ${REALM}
Issuer URL (Piping internal): ${INTERNAL_ISSUER}
Issuer URL (public): ${PUBLIC_ISSUER}
Client ID: ${CLIENT_ID}
${client_secret_line}

Groups and roles:
  /${BASE_GROUP_NAME} -> ${VIEWER_ROLE_NAME}
  /${VIEWER_GROUP_NAME} -> ${VIEWER_ROLE_NAME}
  /${EDITOR_GROUP_NAME} -> ${EDITOR_ROLE_NAME}
  /${ADMIN_GROUP_NAME} -> ${ADMIN_ROLE_NAME}

Test users:
  creation enabled: ${CREATE_TEST_USERS}
EOF

if [ "${CREATE_TEST_USERS}" = "true" ]; then
  if [ "${PRINT_SECRETS}" = "true" ]; then
    cat <<EOF
  ${TEST_ADMIN_USERNAME} / ${TEST_ADMIN_PASSWORD}  (group: /${ADMIN_GROUP_NAME})
  ${TEST_USER_USERNAME} / ${TEST_USER_PASSWORD}    (group: /${BASE_GROUP_NAME})
EOF
  else
    cat <<EOF
  ${TEST_ADMIN_USERNAME} / <hidden>  (group: /${ADMIN_GROUP_NAME})
  ${TEST_USER_USERNAME} / <hidden>   (group: /${BASE_GROUP_NAME})
EOF
  fi
fi

cat <<EOF

Piping UI setup:
  1. Administration -> Access Control -> SSO
  2. Enable OIDC login
  3. Issuer URL: ${INTERNAL_ISSUER}
  4. Client ID: ${CLIENT_ID}
${ui_client_secret_line}
  6. Redirect URL: ${REDIRECT_HTTPS}
  7. Scopes: openid, profile, email, groups
  8. Group claim: groups
  9. Provisioning mode: jit (or manual if users pre-created)

Piping RBAC baseline:
  - The backend seeds roles '${ADMIN_ROLE_NAME}', '${EDITOR_ROLE_NAME}', '${VIEWER_ROLE_NAME}'
  - The backend maps OIDC groups above through identity_group.external_id
  - Users in /${BASE_GROUP_NAME} receive '${VIEWER_ROLE_NAME}' by default and can still receive direct UI role assignments
EOF
