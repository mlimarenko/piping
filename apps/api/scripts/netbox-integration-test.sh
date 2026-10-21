#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
BACKEND_DIR="${ROOT_DIR}/backend"

echo "[netbox-integration-test] start NetBox profile"
(
  cd "${ROOT_DIR}"
  make netbox-up
  make netbox-seed
)

echo "[netbox-integration-test] run backend integration suites"
(
  cd "${BACKEND_DIR}"
  PIPING_DATABASE__URL="${PIPING_DATABASE__URL:-postgresql://piping:piping-dev-password@localhost:5432/piping}" \
  PIPING_REDIS__URL="${PIPING_REDIS__URL:-redis://:dev-redis-password@localhost:6379/0}" \
  REDIS_URL="${REDIS_URL:-redis://:dev-redis-password@localhost:6379/0}" \
  PIPING_SECRETS__CIPHER_KEY_B64="${PIPING_SECRETS__CIPHER_KEY_B64:-MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=}" \
  PIPING_AUTH__LOCAL_ENABLED=true \
  PIPING_AUTH__PASSWORD_PEPPER="${PIPING_AUTH__PASSWORD_PEPPER:-test-pepper}" \
  cargo test --test integration_provider_sources -- --ignored --test-threads=1

  PIPING_DATABASE__URL="${PIPING_DATABASE__URL:-postgresql://piping:piping-dev-password@localhost:5432/piping}" \
  PIPING_REDIS__URL="${PIPING_REDIS__URL:-redis://:dev-redis-password@localhost:6379/0}" \
  REDIS_URL="${REDIS_URL:-redis://:dev-redis-password@localhost:6379/0}" \
  PIPING_SECRETS__CIPHER_KEY_B64="${PIPING_SECRETS__CIPHER_KEY_B64:-MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=}" \
  PIPING_AUTH__LOCAL_ENABLED=true \
  PIPING_AUTH__PASSWORD_PEPPER="${PIPING_AUTH__PASSWORD_PEPPER:-test-pepper}" \
  cargo test --test integration_netbox_source_flow -- --ignored --test-threads=1

  PIPING_DATABASE__URL="${PIPING_DATABASE__URL:-postgresql://piping:piping-dev-password@localhost:5432/piping}" \
  PIPING_REDIS__URL="${PIPING_REDIS__URL:-redis://:dev-redis-password@localhost:6379/0}" \
  REDIS_URL="${REDIS_URL:-redis://:dev-redis-password@localhost:6379/0}" \
  PIPING_SECRETS__CIPHER_KEY_B64="${PIPING_SECRETS__CIPHER_KEY_B64:-MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=}" \
  PIPING_AUTH__LOCAL_ENABLED=true \
  PIPING_AUTH__PASSWORD_PEPPER="${PIPING_AUTH__PASSWORD_PEPPER:-test-pepper}" \
  cargo test --test integration_netbox_webhook_security -- --ignored --test-threads=1
)

echo "[netbox-integration-test] all suites passed"
