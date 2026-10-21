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
if ! command -v openssl >/dev/null 2>&1; then
  echo "openssl is required" >&2
  exit 1
fi

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
SOURCE_DIR="$(cd -- "$SCRIPT_DIR/.." && pwd)"
source "$SCRIPT_DIR/license-e2e-fixture.sh"
license_fixture_cloud "$SOURCE_DIR"
PIPING_SPACE_URL="$LICENSE_PORTAL_URL"
license_fixture_app "$SOURCE_DIR"
BACKEND_URL="$LICENSE_BACKEND_URL"
ORG_EMAIL="${ORG_EMAIL:-license-e2e+$(date +%s)@example.local}"
ORG_PASSWORD="${ORG_PASSWORD:-PipingComPass123!}"
ORG_NAME="${ORG_NAME:-License E2E Org}"
EXTRA_SEATS="${EXTRA_SEATS:-1}"
INSTANCE_ID="${INSTANCE_ID:-piping-e2e-instance}"
ADMIN_IDENTIFIER="${ADMIN_IDENTIFIER:-admin}"
ADMIN_PASSWORD="${ADMIN_PASSWORD:-piping123}"
ATTEMPTS="${ATTEMPTS:-11}"
RUN_OUTAGE_SIM="${RUN_OUTAGE_SIM:-1}"
RESET_STATE="${RESET_STATE:-1}"
WEBHOOK_SECRET="${PIPING_SPACE_BILLING_WEBHOOK_SECRET:-dev_webhook_secret}"

tmp_dir="$(mktemp -d)"
admin_cookie="${tmp_dir}/admin.cookie"
cleanup() {
  rm -rf "${tmp_dir}"
}
trap cleanup EXIT

db_scalar() {
  if ! command -v docker >/dev/null 2>&1; then
    echo "docker is required for DB-backed lifecycle assertions" >&2
    exit 1
  fi
  "${LICENSE_COMPOSE[@]}" exec -T postgres \
    psql -U piping -d piping_license_e2e -tA -c "$1" | tr -d '[:space:]'
}

wait_for_seat_limit() {
  local expected="$1"
  local limit=""
  for _ in $(seq 1 30); do
    status_json="$(curl -fsS -b "${admin_cookie}" "${BACKEND_URL}/api/v1/license/state")"
    limit="$(echo "${status_json}" | jq -r '.seatLimit')"
    if [[ "${limit}" == "${expected}" ]]; then
      return 0
    fi
    sleep 1
  done
  echo "Expected seatLimit=${expected}, got ${limit:-unknown}" >&2
  echo "${status_json:-}" >&2
  exit 1
}

refresh_license_state() {
  curl -fsS -X POST -b "${admin_cookie}" "${BACKEND_URL}/api/v1/license/refresh" >/dev/null
}

active_license_assignments() {
  db_scalar "SELECT COUNT(*) FROM license_seat_assignment WHERE revoked_at IS NULL;"
}

send_billing_webhook() {
  local event_id="$1"
  local status="$2"
  local extra_seats="$3"
  local period_end="$4"
  local body
  local signature

  body="$(jq -n \
    --arg id "${event_id}" \
    --arg org "${ORG_ID}" \
    --arg status "${status}" \
    --arg period_end "${period_end}" \
    --argjson extra_seats "${extra_seats}" \
    '{
      id: $id,
      type: "invoice.updated",
      organization_id: $org,
      status: $status,
      extra_seats: $extra_seats,
      occurred_at: (now | strftime("%Y-%m-%dT%H:%M:%SZ")),
      period_end: $period_end,
      payload: {
        provider: "license-e2e-script"
      }
    }')"
  signature="$(printf '%s' "${body}" | openssl dgst -sha256 -hmac "${WEBHOOK_SECRET}" | awk '{print $2}')"

  curl -fsS -X POST \
    -H "x-piping-signature: sha256=${signature}" \
    -H 'Content-Type: application/json' \
    -d "${body}" \
    "${PIPING_SPACE_URL}/api/v1/billing/webhooks" >/dev/null
}

echo "[1/10] Signup in com.backend"
signup_payload="$(jq -n \
  --arg email "${ORG_EMAIL}" \
  --arg password "${ORG_PASSWORD}" \
  --arg org "${ORG_NAME}" \
  '{email:$email,password:$password,organization_name:$org}')"
signup_json="$(curl -fsS \
  -H 'Content-Type: application/json' \
  -d "${signup_payload}" \
  "${PIPING_SPACE_URL}/api/v1/auth/signup")"
session_token="$(echo "${signup_json}" | jq -r '.session_token')"
if [[ -z "${session_token}" || "${session_token}" == "null" ]]; then
  echo "Failed to get com.backend session token" >&2
  exit 1
fi
ORG_ID="$(echo "${signup_json}" | jq -r '.organization_id')"
if [[ -z "${ORG_ID}" || "${ORG_ID}" == "null" ]]; then
  echo "Failed to get organization id" >&2
  exit 1
fi

echo "[2/10] Checkout +${EXTRA_SEATS} seats in com.backend"
checkout_payload="$(jq -n \
  --argjson extra_seats "${EXTRA_SEATS}" \
  '{extra_seats:$extra_seats,success_url:"http://localhost/success",cancel_url:"http://localhost/cancel"}')"
curl -fsS \
  -H "Authorization: Bearer ${session_token}" \
  -H 'Content-Type: application/json' \
  -d "${checkout_payload}" \
  "${PIPING_SPACE_URL}/api/v1/me/subscription/checkout" >/dev/null

echo "[3/10] Wait until subscription.extra_seats >= ${EXTRA_SEATS}"
extra_seats="0"
for _ in $(seq 1 30); do
  sub_json="$(curl -fsS -H "Authorization: Bearer ${session_token}" "${PIPING_SPACE_URL}/api/v1/me/subscription")"
  extra_seats="$(echo "${sub_json}" | jq -r '.extra_seats')"
  if [[ "${extra_seats}" =~ ^[0-9]+$ ]] && [[ "${extra_seats}" -ge "${EXTRA_SEATS}" ]]; then
    break
  fi
  sleep 1
done
if [[ ! "${extra_seats}" =~ ^[0-9]+$ ]] || [[ "${extra_seats}" -lt "${EXTRA_SEATS}" ]]; then
  echo "Subscription was not updated by billing flow" >&2
  exit 1
fi

echo "[4/10] Create activation token"
token_json="$(curl -fsS -X POST -H "Authorization: Bearer ${session_token}" "${PIPING_SPACE_URL}/api/v1/cabinet/tokens")"
activation_token="$(echo "${token_json}" | jq -r '.plainToken')"
if [[ -z "${activation_token}" || "${activation_token}" == "null" ]]; then
  echo "Failed to create activation token" >&2
  exit 1
fi

echo "[5/10] Login product admin and activate instance license"
login_payload="$(jq -n --arg i "${ADMIN_IDENTIFIER}" --arg p "${ADMIN_PASSWORD}" '{identifier:$i,password:$p}')"
curl -fsS -c "${admin_cookie}" \
  -H 'Content-Type: application/json' \
  -d "${login_payload}" \
  "${BACKEND_URL}/api/v1/auth/login" >/dev/null

if [[ "${RESET_STATE}" == "1" ]]; then
  if ! command -v docker >/dev/null 2>&1; then
    echo "docker is required when RESET_STATE=1" >&2
    exit 1
  fi
  echo " - resetting the isolated licensing fixture"
  "${LICENSE_COMPOSE[@]}" exec -T postgres psql -U piping -d piping_license_e2e >/dev/null <<'SQL'
DELETE FROM license_seat_assignment;
DELETE FROM license_seat_ledger;
DELETE FROM license_state;
DELETE FROM identity_user WHERE username LIKE 'license-e2e-user-%';
SQL
fi

activate_payload="$(jq -n \
  --arg t "${activation_token}" \
  --arg id "${INSTANCE_ID}" \
  '{activationToken:$t,instanceFingerprint:$id}')"
curl -fsS -b "${admin_cookie}" \
  -H 'Content-Type: application/json' \
  -d "${activate_payload}" \
  "${BACKEND_URL}/api/v1/license/activate" >/dev/null

expected_paid_limit=$((10 + EXTRA_SEATS))
wait_for_seat_limit "${expected_paid_limit}"
active_after_activation="$(active_license_assignments)"
default_available_after_activation=$((10 - active_after_activation))
paid_available_after_activation=$((expected_paid_limit - active_after_activation))
if [[ "${paid_available_after_activation}" -le "${default_available_after_activation}" ]]; then
  echo "Expected paid license to increase available seats (${paid_available_after_activation} <= ${default_available_after_activation})" >&2
  exit 1
fi

echo "[6/10] Simulate unpaid renewal and verify fallback to default 10 seats"
send_billing_webhook \
  "license-e2e-past-due-$(date +%s)" \
  "past_due" \
  "${EXTRA_SEATS}" \
  "$(date -u -d '1 day ago' +%Y-%m-%dT%H:%M:%SZ)"
refresh_license_state
wait_for_seat_limit "10"
active_after_expiry="$(active_license_assignments)"
available_after_expiry=$((10 - active_after_expiry))
if [[ "${available_after_expiry}" -ne "${default_available_after_activation}" ]]; then
  echo "Expected expired paid license to fall back to base available seats (${default_available_after_activation}), got ${available_after_expiry}" >&2
  exit 1
fi

echo "[7/10] Simulate subscription recovery and verify the same token restores paid seats"
send_billing_webhook \
  "license-e2e-active-$(date +%s)" \
  "active" \
  "${EXTRA_SEATS}" \
  "$(date -u -d '30 days' +%Y-%m-%dT%H:%M:%SZ)"
refresh_license_state
wait_for_seat_limit "${expected_paid_limit}"
active_after_restore="$(active_license_assignments)"
available_after_restore=$((expected_paid_limit - active_after_restore))
if [[ "${available_after_restore}" -ne "${paid_available_after_activation}" ]]; then
  echo "Expected restored paid license to restore available seats (${paid_available_after_activation}), got ${available_after_restore}" >&2
  exit 1
fi

status_before_users="$(curl -fsS -b "${admin_cookie}" "${BACKEND_URL}/api/v1/license/state")"
seat_limit="$(echo "${status_before_users}" | jq -r '.seatLimit')"
baseline_active_seats="$(active_license_assignments)"
if [[ ! "${baseline_active_seats}" =~ ^[0-9]+$ ]] || [[ ! "${seat_limit}" =~ ^[0-9]+$ ]]; then
  echo "Invalid seat counters before user logins" >&2
  exit 1
fi
allowed_new_users=$((seat_limit - baseline_active_seats))
if [[ "${allowed_new_users}" -lt 1 ]]; then
  echo "No available seats before user login test (baseline=${baseline_active_seats}, seat_limit=${seat_limit})" >&2
  exit 1
fi

echo "[8/10] Create ${ATTEMPTS} users and verify dynamic seat boundary (allowed new users: ${allowed_new_users})"
for i in $(seq 1 "${ATTEMPTS}"); do
  username="license-e2e-user-${i}-$(date +%s)"
  password="Abcd1234!"
  create_payload="$(jq -n \
    --arg username "${username}" \
    --arg email "${username}@test.local" \
    --arg password "${password}" \
    '{
      username:$username,
      email:$email,
      status:"active",
      auth_source:"local",
      credential:{mode:"manual",password:$password}
    }')"
  curl -fsS -b "${admin_cookie}" \
    -H 'Content-Type: application/json' \
    -d "${create_payload}" \
    "${BACKEND_URL}/api/v1/admin/access-control/users" >/dev/null

  user_login_payload="$(jq -n --arg i "${username}" --arg p "${password}" '{identifier:$i,password:$p}')"
  status_file="${tmp_dir}/status.${i}"
  body_file="${tmp_dir}/body.${i}.json"
  http_code="$(curl -sS -o "${body_file}" -w '%{http_code}' \
    -H 'Content-Type: application/json' \
    -d "${user_login_payload}" \
    "${BACKEND_URL}/api/v1/auth/login")"

  if [[ "${i}" -le "${allowed_new_users}" ]]; then
    if [[ "${http_code}" != "200" ]]; then
      echo "Expected login success for user ${i}, got HTTP ${http_code}" >&2
      cat "${body_file}" >&2
      exit 1
    fi
  else
    if [[ "${http_code}" != "403" ]]; then
      echo "Expected seat limit rejection for user ${i}, got HTTP ${http_code}" >&2
      cat "${body_file}" >&2
      exit 1
    fi
    code="$(jq -r '.error.code // .code // empty' "${body_file}")"
    if [[ "${code}" != "LICENSE_SEAT_LIMIT_EXCEEDED" ]]; then
      echo "Expected LICENSE_SEAT_LIMIT_EXCEEDED, got '${code}'" >&2
      cat "${body_file}" >&2
      exit 1
    fi
  fi
done

echo "[9/10] Verify license status after boundary"
status_json="$(curl -fsS -b "${admin_cookie}" "${BACKEND_URL}/api/v1/license/state")"
seat_limit="$(echo "${status_json}" | jq -r '.seatLimit')"
active_seats="$(db_scalar "SELECT COUNT(*) FROM license_seat_assignment WHERE revoked_at IS NULL;")"
available_seats=$((seat_limit - active_seats))
if [[ "${active_seats}" != "${seat_limit}" ]]; then
  echo "Expected active_seats==seat_limit (${seat_limit}), got ${active_seats}" >&2
  exit 1
fi
if [[ "${available_seats}" != "0" ]]; then
  echo "Expected available_seats=0, got ${available_seats}" >&2
  exit 1
fi

echo "[10/10] Optional outage/grace simulation"
if [[ "${RUN_OUTAGE_SIM}" == "1" ]]; then
  BACKEND_URL="${BACKEND_URL}" \
  PIPING_SPACE_URL="${PIPING_SPACE_URL}" \
  ADMIN_IDENTIFIER="${ADMIN_IDENTIFIER}" \
  ADMIN_PASSWORD="${ADMIN_PASSWORD}" \
  "$(dirname "$0")/license-outage-sim.sh"
fi

echo "E2E lifecycle completed successfully."
