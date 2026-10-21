#!/usr/bin/env bash

license_fixture_die() { printf 'licensing fixture: %s\n' "$*" >&2; exit 1; }

license_fixture_project() {
  [[ "$1" =~ ^piping-license-e2e(-[a-z0-9]+)*$ ]] ||
    license_fixture_die 'project must be an isolated piping-license-e2e project'
}

license_fixture_port() {
  [[ "$1" =~ ^[1-9][0-9]{0,4}$ ]] && (( $1 <= 65535 )) ||
    license_fixture_die 'fixture port must be a valid decimal port'
}

license_fixture_cloud_project() {
  [[ -n "$1" && "$1" != "${PIPING_LICENSE_E2E_PROJECT:-piping-license-e2e}" ]] ||
    license_fixture_die 'an explicit isolated Cloud project is required'
  license_fixture_project "$1"
}

license_fixture_app_project() {
  license_fixture_project "$1"
  [[ -z "${PIPING_LICENSE_E2E_CLOUD_PROJECT:-}" || "$1" != "$PIPING_LICENSE_E2E_CLOUD_PROJECT" ]] ||
    license_fixture_die 'app and Cloud must use separate fixture projects'
}

license_fixture_volume() {
  local project="$1" suffix="$2" allow_missing="${3:-false}" name actual owner
  name="${project}_${suffix}"
  actual="$(docker volume ls --filter "name=^${name}$" --format '{{.Name}}')" ||
    license_fixture_die 'cannot inspect fixture volumes'
  if [[ -z "$actual" && "$allow_missing" == true ]]; then return; fi
  [[ "$actual" == "$name" ]] || license_fixture_die 'fixture volume identity is unavailable'
  owner="$(docker volume inspect --format '{{index .Labels "com.docker.compose.project"}}|{{index .Labels "io.piping.fixture"}}' "$name")" ||
    license_fixture_die 'cannot inspect fixture volume ownership'
  [[ "$owner" == "$project|licensing-e2e" ]] ||
    license_fixture_die 'existing volume is not owned by this licensing fixture'
}

license_fixture_network() {
  local project="$1" allow_missing="${2:-false}" name actual owner
  license_fixture_cloud_project "$project"
  name="${project}_licensing-e2e"
  actual="$(docker network ls --filter "name=^${name}$" --format '{{.Name}}')" ||
    license_fixture_die 'cannot inspect licensing fixture network'
  if [[ -z "$actual" && "$allow_missing" == true ]]; then return; fi
  [[ "$actual" == "$name" ]] || license_fixture_die 'start the isolated Cloud fixture first'
  owner="$(docker network inspect --format '{{index .Labels "com.docker.compose.project"}}|{{index .Labels "io.piping.fixture"}}' "$name")" ||
    license_fixture_die 'cannot inspect licensing fixture network ownership'
  [[ "$owner" == "$project|licensing-e2e" ]] ||
    license_fixture_die 'network is not owned by the selected Cloud fixture'
}

license_fixture_container() {
  local project="$1" service="$2" id metadata
  id="$("${LICENSE_COMPOSE[@]}" ps --all --quiet "$service")" ||
    license_fixture_die 'cannot find fixture container'
  [[ "$id" =~ ^[a-f0-9]{64}$ ]] || license_fixture_die 'exactly one fixture container is required'
  metadata="$(docker inspect --format '{{index .Config.Labels "com.docker.compose.project"}}|{{index .Config.Labels "com.docker.compose.service"}}|{{index .Config.Labels "io.piping.fixture"}}' "$id")" ||
    license_fixture_die 'cannot inspect fixture ownership'
  [[ "$metadata" == "$project|$service|licensing-e2e" ]] ||
    license_fixture_die 'container is not owned by the selected licensing fixture'
  LICENSE_CONTAINER="$id"
}

license_fixture_binding() {
  local port="$1" expected="$2" actual
  actual="$(docker inspect --format '{{range (index .NetworkSettings.Ports "'"$port"'/tcp")}}{{println .HostIp .HostPort}}{{end}}' "$LICENSE_CONTAINER")" ||
    license_fixture_die 'cannot inspect fixture port binding'
  [[ "$actual" == "127.0.0.1 $expected" ]] ||
    license_fixture_die 'fixture API must own the selected loopback port'
}

license_fixture_database_mount() {
  local project="$1" suffix="$2" name
  name="$(docker inspect --format '{{range .Mounts}}{{if eq .Destination "/var/lib/postgresql"}}{{.Name}}{{end}}{{end}}' "$LICENSE_CONTAINER")" ||
    license_fixture_die 'cannot inspect fixture database mount'
  [[ "$name" == "${project}_${suffix}" ]] ||
    license_fixture_die 'database mount is not isolated to the fixture project'
  license_fixture_volume "$project" "$suffix"
}

license_fixture_app() {
  local root="$1" project="${PIPING_LICENSE_E2E_PROJECT:-piping-license-e2e}"
  local port="${PIPING_LICENSE_E2E_PORT:-17000}" actual
  license_fixture_app_project "$project"
  license_fixture_port "$port"
  [[ -z "${LICENSE_POSTGRES_CONTAINER:-}" && -z "${LICENSE_DATABASE_NAME:-}" ]] ||
    license_fixture_die 'container and database overrides are not accepted'
  LICENSE_BACKEND_URL="http://127.0.0.1:$port"
  [[ -z "${BACKEND_URL:-}" || "$BACKEND_URL" == "$LICENSE_BACKEND_URL" ]] ||
    license_fixture_die 'backend URL must select the loopback licensing fixture'
  license_fixture_network "${PIPING_LICENSE_E2E_CLOUD_PROJECT:-}"
  LICENSE_COMPOSE=(docker compose --project-directory "$root" --env-file "$root/env/dev.env"
    -p "$project" -f "$root/docker-compose.yml" -f "$root/tests/compose/docker-compose.licensing.yml")
  license_fixture_container "$project" postgres
  license_fixture_database_mount "$project" postgres_data
  actual="$("${LICENSE_COMPOSE[@]}" exec -T postgres psql -U piping -d piping_license_e2e -tA -c 'SELECT current_database()')" ||
    license_fixture_die 'cannot verify the fixture database'
  [[ "$actual" == piping_license_e2e ]] || license_fixture_die 'wrong licensing fixture database'
  license_fixture_container "$project" backend
  license_fixture_binding 7000 "$port"
}

license_fixture_cloud() {
  local root="$1" project="${PIPING_LICENSE_E2E_CLOUD_PROJECT:-}" file="${PIPING_LICENSE_E2E_CLOUD_COMPOSE:-}"
  local port="${PIPING_LICENSE_E2E_PORTAL_PORT:-18090}"
  license_fixture_cloud_project "$project"
  license_fixture_port "$port"
  [[ "$file" == /* && -f "$file" && ! -L "$file" ]] ||
    license_fixture_die 'an explicit regular absolute Cloud Compose file is required'
  LICENSE_PORTAL_URL="http://127.0.0.1:$port"
  [[ -z "${PIPING_SPACE_URL:-}" || "$PIPING_SPACE_URL" == "$LICENSE_PORTAL_URL" ]] ||
    license_fixture_die 'portal URL must select the loopback licensing fixture'
  [[ -z "${COMPOSE_PORTAL_FILE:-}" && -z "${COMPOSE_SERVICE:-}" ]] ||
    license_fixture_die 'unverified portal target overrides are not accepted'
  export PIPING_SPACE_POSTGRES_DB=piping_license_e2e_cloud
  license_fixture_network "$project"
  LICENSE_COMPOSE=(docker compose -p "$project" -f "$file" -f "$root/tests/compose/docker-compose.licensing-cloud.yml")
  license_fixture_container "$project" postgres
  license_fixture_database_mount "$project" postgres-data
  license_fixture_container "$project" backend
  license_fixture_binding 8090 "$port"
}
