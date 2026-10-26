#!/usr/bin/env bash
set -euo pipefail
IFS=$'\n\t'
umask 077

# Rendered by the source publisher, never selected from the caller's environment.
PUBLIC_VERSION='1.0.3'
SOURCE_FILE="${BASH_SOURCE[0]:-}"
SOURCE_DIR=""
if [[ -n "$SOURCE_FILE" && -f "$SOURCE_FILE" ]]; then
  SOURCE_DIR="$(cd -- "$(dirname -- "$SOURCE_FILE")" && pwd)"
fi
INSTALL_DIR="$HOME/piping"
if [[ -n "$SOURCE_DIR" && ( -e "$SOURCE_DIR/.env" || -L "$SOURCE_DIR/.env" || -e "$SOURCE_DIR/.deploy-release.lock" || -e "$SOURCE_DIR/docker/nginx/ssl/fullchain.pem" || -e "$SOURCE_DIR/docker/nginx/ssl/privkey.pem" ) ]]; then
  INSTALL_DIR="$SOURCE_DIR"
fi
HTTP_PORT=80
HTTPS_PORT=443
PUBLIC_HOST=localhost
POSTGRES_PORT=5432
REDIS_PORT=6379
PROJECT_NAME=""
BACKEND_IMAGE=""
FRONTEND_IMAGE=""
ANSIBLE_RUNNER_IMAGE=""
SOURCE_BASE_URL=""
SKIP_PULL=0
UPDATE_IMAGES=()
LAYOUT_OPTIONS=()
STACK_SOURCE_PATHS=(
  install.sh env/images.env docker-compose.yml
  docker/nginx/nginx.conf docker/nginx/proxy-common.conf
  docker/nginx/locations/assets.conf docker/nginx/locations/backend.conf
  docker/nginx/locations/keycloak.conf docker/nginx/entrypoint.d/10-init-ssl.sh
  scripts/public_release.py scripts/deploy_release_bundle.py scripts/ci/apply_release_bundle.py
  scripts/ci/compose_delivery_contract.py scripts/ci/postgres_migration_contract.py
)

die() { printf 'piping: %s\n' "$*" >&2; exit 1; }
log_info() { printf 'piping: %s\n' "$*"; }
require_cmd() { command -v "$1" >/dev/null 2>&1 || die "Required command is missing: $1"; }

usage() {
  cat <<'USAGE'
Install or update Piping with Docker Compose.

  ./install.sh
  ./install.sh --backend-image registry.example/piping/backend:version

Options:
  --install-dir PATH        Stack directory (default: ~/piping, or this installed copy)
  --backend-image REF       Update backend to this complete image reference
  --frontend-image REF      Update frontend to this complete image reference
  --ansible-runner-image REF Update runner to this complete image reference
  --hostname HOST           Fresh-install DNS name or IPv4 address (default: localhost)
  --http-port PORT          Fresh-install HTTP port (default: 80)
  --https-port PORT         Fresh-install HTTPS port (default: 443)
  --postgres-port PORT      Fresh-install PostgreSQL port (default: 5432)
  --redis-port PORT         Fresh-install Redis port (default: 6379)
  --project-name NAME       Compose project (existing installations retain theirs)
  --source-base-url URL     Explicit source transport instead of the local checkout
  --skip-pull               Use images already available in Docker
  --help                    Show help

Fresh source installation uses the image cohort in env/images.env. Piped public
installation verifies the selected release and its digest-pinned images first. Existing settings, secrets
and volumes are retained. Image updates require a running stack and stop writers while the
existing guarded updater verifies the backup and migrations. Do not interrupt
an update; on failure inspect its private backup/journal before recovery.
Backend images must carry their source revision and guarded migration contract.
USAGE
}

acquire_public_package() (
  [[ "$PUBLIC_VERSION" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ && ${#PUBLIC_VERSION} -le 64 ]] ||
    die "Public installation must come from a published version tag"
  [[ -z "$SOURCE_BASE_URL" ]] || die "Public installation uses only the selected release transport"
  require_cmd python3; require_cmd curl
  local temporary
  temporary="$(mktemp -d "${TMPDIR:-/tmp}/piping-package.XXXXXXXX")"
  trap 'rm -f -- "$temporary/public_release.py"; rmdir -- "$temporary"' EXIT
  curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' \
    --connect-timeout 15 --max-time 90 --max-filesize 1048576 \
    "https://raw.githubusercontent.com/mlimarenko/piping/${PUBLIC_VERSION}/scripts/public_release.py" \
    --output "$temporary/public_release.py" || die "Cannot load the selected release verifier"
  python3 "$temporary/public_release.py" install --version "$PUBLIC_VERSION" -- "$@"
)

select_source() {
  local relative path
  [[ -n "$SOURCE_DIR" ]] || return 1
  [[ ! -L "$SOURCE_FILE" ]] || die "Installer must not be a symlink"
  if [[ -e "$SOURCE_DIR/.env" || -L "$SOURCE_DIR/.env" || -e "$SOURCE_DIR/.deploy-release.lock" || -e "$SOURCE_DIR/.fresh-install-in-progress" || -e "$SOURCE_DIR/.images.env" || -e "$SOURCE_DIR/docker/nginx/ssl/fullchain.pem" || -e "$SOURCE_DIR/docker/nginx/ssl/privkey.pem" ]]; then
    [[ -f "$SOURCE_DIR/.env" && ! -L "$SOURCE_DIR/.env" ]] || die "Existing installation settings are missing; restore the original .env"
  fi
  for relative in "${STACK_SOURCE_PATHS[@]}"; do
    if [[ ! -f "$SOURCE_DIR/$relative" || -L "$SOURCE_DIR/$relative" ]]; then
      # A standalone published script can acquire its package. A partial source
      # or installed layout must never be reinterpreted as that fresh script.
      for relative in .env .git env docker scripts docker-compose.yml .deploy-release.lock .fresh-install-in-progress .images.env; do
        [[ ! -e "$SOURCE_DIR/$relative" && ! -L "$SOURCE_DIR/$relative" ]] ||
          die "Incomplete installation/source layout; restore its files explicitly"
      done
      return 1
    fi
    path="$SOURCE_DIR/$relative"
    while [[ -n "$path" && "$path" != / ]]; do
      [[ ! -L "$path" ]] || die "Installation/source path must not contain symlinks"
      path="${path%/*}"
    done
  done
  if [[ -e "$SOURCE_DIR/env/public-source.json" || -L "$SOURCE_DIR/env/public-source.json" ]]; then
    require_cmd python3
    python3 -c 'import pathlib,sys; sys.path.insert(0,sys.argv[1]); from scripts.public_release import decode,regular_file,source_descriptor; source_descriptor(decode(regular_file(pathlib.Path(sys.argv[1]),"env/public-source.json").read_bytes()),sys.argv[2])' "$SOURCE_DIR" "$PUBLIC_VERSION" ||
      die "Public installation version differs from its source descriptor"
  fi
}

validate_image_reference() {
  local reference="$1"
  [[ "$reference" =~ ^[a-zA-Z0-9][a-zA-Z0-9._:/@-]*$ && "$reference" != *"://"* ]] ||
    die "Image references must be single-line Docker references without expressions"
  if [[ "$reference" == *@* ]]; then
    [[ "$reference" =~ ^[^@]+@sha256:[a-f0-9]{64}$ ]] || die "Invalid image digest"
  fi
}

validate_port() {
  [[ "$1" =~ ^[1-9][0-9]{0,4}$ ]] || die "Port must be a decimal integer without leading zeros"
  (( $1 >= 1 && $1 <= 65535 )) || die "Port must be between 1 and 65535"
}

validate_public_host() {
  [[ ${#PUBLIC_HOST} -le 253 && "$PUBLIC_HOST" =~ ^[a-zA-Z0-9]([a-zA-Z0-9.-]*[a-zA-Z0-9])?$ ]] ||
    die "Hostname must be a DNS name or IPv4 address without scheme, port or path"
  local label
  local -a labels
  IFS=. read -r -a labels <<< "$PUBLIC_HOST"
  for label in "${labels[@]}"; do
    [[ ${#label} -le 63 && "$label" =~ ^[a-zA-Z0-9]([a-zA-Z0-9-]*[a-zA-Z0-9])?$ ]] || die "Invalid hostname label"
  done
}

parse_args() {
  while (( $# )); do
    [[ "$1" == --help || "$1" == -h ]] && { usage; exit 0; }
    [[ "$1" == --skip-pull ]] && { SKIP_PULL=1; shift; continue; }
    [[ $# -ge 2 && -n "$2" ]] || die "Missing option value"
    case "$1" in
      --install-dir) INSTALL_DIR="$2" ;;
      --http-port) HTTP_PORT="$2"; LAYOUT_OPTIONS+=("PIPING_HTTP_PORT=$2") ;;
      --https-port) HTTPS_PORT="$2"; LAYOUT_OPTIONS+=("PIPING_HTTPS_PORT=$2") ;;
      --hostname) PUBLIC_HOST="$2"; LAYOUT_OPTIONS+=("PIPING_PUBLIC_HOST=$2") ;;
      --postgres-port) POSTGRES_PORT="$2"; LAYOUT_OPTIONS+=("PIPING_POSTGRES_PORT=$2") ;;
      --redis-port) REDIS_PORT="$2"; LAYOUT_OPTIONS+=("PIPING_REDIS_PORT=$2") ;;
      --project-name) PROJECT_NAME="$2" ;;
      --source-base-url) SOURCE_BASE_URL="$2" ;;
      --backend-image) validate_image_reference "$2"; BACKEND_IMAGE="$2"; UPDATE_IMAGES+=("backend=$2") ;;
      --frontend-image) validate_image_reference "$2"; FRONTEND_IMAGE="$2"; UPDATE_IMAGES+=("frontend=$2") ;;
      --ansible-runner-image) validate_image_reference "$2"; ANSIBLE_RUNNER_IMAGE="$2"; UPDATE_IMAGES+=("ansible-runner=$2") ;;
      *) die "Unknown option: $1" ;;
    esac
    shift 2
  done
}

read_env_value() {
  local key="$1" file="$2" value=""
  [[ -f "$file" ]] || return 0
  value="$(grep -E "^${key}=" "$file" | tail -n 1 || true)"
  value="${value#*=}"
  if [[ "$value" == \"*\" || "$value" == \'*\' ]]; then value="${value:1:${#value}-2}"; fi
  printf '%s' "$value"
}

copy_source() {
  local relative="$1" target="$2"
  mkdir -p -- "$(dirname -- "$target")"
  if [[ -n "$SOURCE_BASE_URL" ]]; then
    curl --fail --silent --show-error --location "$SOURCE_BASE_URL/$relative" --output "$target" ||
      die "Cannot load installation file: $relative"
  elif [[ "$SOURCE_DIR/$relative" != "$target" ]]; then
    [[ -f "$SOURCE_DIR/$relative" && ! -L "$SOURCE_DIR/$relative" ]] || die "Missing source file: $relative"
    cp -- "$SOURCE_DIR/$relative" "$target"
  fi
}

prepare_tls() {
  local ssl_dir="$INSTALL_DIR/docker/nginx/ssl"
  local cert="$ssl_dir/fullchain.pem" key="$ssl_dir/privkey.pem"
  local san="DNS:$PUBLIC_HOST" identity_check=-checkhost
  if [[ "$PUBLIC_HOST" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]]; then san="IP:$PUBLIC_HOST"; identity_check=-checkip; fi
  if [[ ! -e "$cert" && ! -e "$key" ]]; then
    openssl req -x509 -nodes -newkey rsa:2048 -days 365 \
      -subj "/CN=$PUBLIC_HOST" -addext "subjectAltName=$san" \
      -keyout "$key" -out "$cert" >/dev/null 2>&1 || die "Cannot create TLS certificate"
    chmod 644 "$cert"
  fi
  [[ -s "$cert" && -s "$key" ]] || die "TLS certificate/key pair is incomplete; restore it explicitly"
  openssl x509 -in "$cert" -noout -checkend 0 >/dev/null 2>&1 || die "TLS certificate is invalid or expired"
  openssl pkey -in "$key" -passin pass: -check -noout >/dev/null 2>&1 || die "TLS private key is invalid or encrypted"
  local cert_public key_public
  cert_public="$(openssl x509 -in "$cert" -pubkey -noout | openssl pkey -pubin -outform DER | openssl dgst -sha256)"
  key_public="$(openssl pkey -in "$key" -passin pass: -pubout -outform DER | openssl dgst -sha256)"
  [[ "$cert_public" == "$key_public" ]] || die "TLS certificate and key do not match"
  openssl x509 -in "$cert" -noout "$identity_check" "$PUBLIC_HOST" >/dev/null 2>&1 ||
    die "TLS certificate does not match the configured hostname; supply a matching pair explicitly"
  chmod 600 "$key"
}

prepare_layout() {
  local relative
  mkdir -p -- "$INSTALL_DIR/docker/nginx/ssl" "$INSTALL_DIR/docker/nginx/acme-challenge"
  for relative in "${STACK_SOURCE_PATHS[@]}"; do copy_source "$relative" "$INSTALL_DIR/$relative"; done
  if [[ -f "$SOURCE_DIR/env/public-source.json" ]]; then
    copy_source env/public-source.json "$INSTALL_DIR/env/public-source.json"
  fi
  chmod +x "$INSTALL_DIR/install.sh" "$INSTALL_DIR/docker/nginx/entrypoint.d/10-init-ssl.sh"
  prepare_tls
  [[ ! -e "$INSTALL_DIR/.env" ]] || return 0
  BACKEND_IMAGE="${BACKEND_IMAGE:-$(read_env_value PIPING_BACKEND_IMAGE "$INSTALL_DIR/env/images.env")}"
  FRONTEND_IMAGE="${FRONTEND_IMAGE:-$(read_env_value PIPING_FRONTEND_IMAGE "$INSTALL_DIR/env/images.env")}"
  ANSIBLE_RUNNER_IMAGE="${ANSIBLE_RUNNER_IMAGE:-$(read_env_value PIPING_ANSIBLE_RUNNER_IMAGE "$INSTALL_DIR/env/images.env")}"
  for relative in "$BACKEND_IMAGE" "$FRONTEND_IMAGE" "$ANSIBLE_RUNNER_IMAGE"; do validate_image_reference "$relative"; done
  local private_env database_password redis_password cipher_key pepper administrator_password
  database_password="$(openssl rand -hex 24)"
  redis_password="$(openssl rand -hex 24)"
  cipher_key="$(openssl rand -base64 32)"
  pepper="$(openssl rand -hex 24)"
  administrator_password="$(openssl rand -hex 16)"
  private_env="$(mktemp "$INSTALL_DIR/.env.XXXXXX")"
  {
    printf 'COMPOSE_PROJECT_NAME=%s\n' "$PROJECT_NAME"
    printf 'PIPING_HTTP_PORT=%s\nPIPING_HTTPS_PORT=%s\nPIPING_PUBLIC_HOST=%s\n' "$HTTP_PORT" "$HTTPS_PORT" "$PUBLIC_HOST"
    printf 'PIPING_POSTGRES_PORT=%s\nPIPING_REDIS_PORT=%s\n' "$POSTGRES_PORT" "$REDIS_PORT"
    printf 'PIPING_BACKEND_IMAGE=%s\nPIPING_FRONTEND_IMAGE=%s\nPIPING_ANSIBLE_RUNNER_IMAGE=%s\n' "$BACKEND_IMAGE" "$FRONTEND_IMAGE" "$ANSIBLE_RUNNER_IMAGE"
    printf 'POSTGRES_USER=piping\nPOSTGRES_DB=piping\nPOSTGRES_PASSWORD=%s\n' "$database_password"
    printf 'REDIS_PASSWORD=%s\n' "$redis_password"
    printf 'PIPING_SECRETS__CIPHER_KEY_B64=%s\n' "$cipher_key"
    printf 'PIPING_AUTH_PASSWORD_PEPPER=%s\n' "$pepper"
    printf 'PIPING_AUTH_LOCAL_ADMIN_PASSWORD=%s\n' "$administrator_password"
  } > "$private_env"
  if ! ln -- "$private_env" "$INSTALL_DIR/.env"; then
    rm -f -- "$private_env"
    die "Settings already exist; they were not replaced"
  fi
  rm -f -- "$private_env"
  # A failed initial pull/start can resume only with the same settings and source.
  (cd -- "$INSTALL_DIR" && sha256sum -- .env "${STACK_SOURCE_PATHS[@]}" > .fresh-install-in-progress)
}

run_compose() {
  # Fresh installation and guarded updates use the same environment boundary.
  python3 -c 'import os,sys; sys.path.insert(0,sys.argv[1]); from scripts.deploy_release_bundle import environment; os.execvpe(sys.argv[2],sys.argv[2:],environment())' "$INSTALL_DIR" "$@"
}

deploy_stack() {
  local -a compose=(docker compose --project-name "$PROJECT_NAME" --project-directory "$INSTALL_DIR" --env-file "$INSTALL_DIR/.env")
  [[ ! -f "$INSTALL_DIR/.images.env" ]] || compose+=(--env-file "$INSTALL_DIR/.images.env")
  compose+=(--profile edge -f "$INSTALL_DIR/docker-compose.yml")
  if (( ! SKIP_PULL )); then run_compose "${compose[@]}" pull --policy missing; fi
  run_compose "${compose[@]}" up -d --no-build --pull never --wait --wait-timeout 180 "$@"
}

print_success() {
  local authority="$PUBLIC_HOST"
  [[ "$HTTPS_PORT" == 443 ]] || authority+=":$HTTPS_PORT"
  printf 'Piping: https://%s/\nStack: %s\n' "$authority" "$INSTALL_DIR"
  printf 'Administrator password: PIPING_AUTH_LOCAL_ADMIN_PASSWORD in %s/.env\n' "$INSTALL_DIR"
}

main() {
  parse_args "$@"
  if ! select_source; then
    acquire_public_package "$@"
    return
  fi
  require_cmd python3; require_cmd realpath
  INSTALL_DIR="${INSTALL_DIR/#\~/$HOME}"
  INSTALL_DIR="$(realpath -ms -- "$INSTALL_DIR")"
  [[ "$INSTALL_DIR" != / && "$INSTALL_DIR" != "$HOME" && ! -L "$INSTALL_DIR" ]] || die "Choose a stack-specific directory"
  python3 -c 'import pathlib,sys; p=pathlib.Path(sys.argv[1]); sys.exit(any(q.is_symlink() for q in (p,*p.parents)))' "$INSTALL_DIR" ||
    die "Installation path must not contain symlinks"
  [[ ! -L "$INSTALL_DIR/.env" ]] || die "Installation settings must not be a symlink"
  if [[ ! -f "$INSTALL_DIR/.env" ]]; then
    python3 -c 'import pathlib,sys; p=pathlib.Path(sys.argv[1]); sys.exit(p.exists() and (not p.is_dir() or any(p.iterdir())))' "$INSTALL_DIR" ||
      die "Existing installation directory is not empty; restore its original settings explicitly"
  fi
  require_cmd docker; require_cmd openssl
  [[ -z "$SOURCE_BASE_URL" ]] || require_cmd curl
  docker compose version >/dev/null 2>&1 || die "Docker Compose v2 is required"
  docker info >/dev/null 2>&1 || die "Docker daemon is unavailable"
  # Existing start/update actions take this same lock in the owning engine.
  if [[ ! -f "$INSTALL_DIR/.env" || -e "$INSTALL_DIR/.fresh-install-in-progress" ]]; then
    require_cmd flock
    mkdir -p -- "$INSTALL_DIR"
    [[ ! -L "$INSTALL_DIR/.deploy-release.lock" ]] || die "Deployment lock must not be a symlink"
    exec {DEPLOY_LOCK_FD}<>"$INSTALL_DIR/.deploy-release.lock"
    [[ -f "$INSTALL_DIR/.deploy-release.lock" &&
       "$(stat -c '%a:%h' "$INSTALL_DIR/.deploy-release.lock")" == '600:1' ]] ||
      die "Deployment lock must be a private regular file"
    flock --nonblock "$DEPLOY_LOCK_FD" || die "Another installation/update is active"
  fi
  if [[ -f "$INSTALL_DIR/.env" ]]; then
    local installed_project option
    for option in "${LAYOUT_OPTIONS[@]}"; do
      [[ "${option#*=}" == "$(read_env_value "${option%%=*}" "$INSTALL_DIR/.env")" ]] ||
        die "Existing network settings are retained; edit .env and supply matching TLS material explicitly"
    done
    installed_project="$(read_env_value COMPOSE_PROJECT_NAME "$INSTALL_DIR/.env")"
    [[ -z "$PROJECT_NAME" || "$PROJECT_NAME" == "$installed_project" ]] || die "Existing Compose project cannot be renamed"
    PROJECT_NAME="$installed_project"
    HTTP_PORT="$(read_env_value PIPING_HTTP_PORT "$INSTALL_DIR/.env")"
    HTTPS_PORT="$(read_env_value PIPING_HTTPS_PORT "$INSTALL_DIR/.env")"
    PUBLIC_HOST="$(read_env_value PIPING_PUBLIC_HOST "$INSTALL_DIR/.env")"
    PUBLIC_HOST="${PUBLIC_HOST:-localhost}"
    validate_public_host
    [[ -s "$INSTALL_DIR/docker/nginx/ssl/fullchain.pem" && -s "$INSTALL_DIR/docker/nginx/ssl/privkey.pem" ]] ||
      die "Existing TLS material is missing; restore the original pair explicitly"
    prepare_tls
    if [[ -e "$INSTALL_DIR/.fresh-install-in-progress" ]]; then
      (( ${#UPDATE_IMAGES[@]} == 0 )) || die "Complete the initial installation before updating images"
      (cd -- "$INSTALL_DIR" && sha256sum --check --status .fresh-install-in-progress) ||
        die "Initial installation settings or source changed; inspect the incomplete installation before retrying"
      # Recreate partial containers after an interrupted first boot. Docker can
      # retain an unbound port map after a failed network setup; volumes stay.
      deploy_stack --force-recreate
      rm -- "$INSTALL_DIR/.fresh-install-in-progress"
      print_success
      return
    fi
    if (( ${#UPDATE_IMAGES[@]} )); then
      require_cmd findmnt
      [[ -f "$SOURCE_DIR/scripts/deploy_release_bundle.py" ]] || die "Run install.sh from its complete source directory"
      local -a update=(python3 "$SOURCE_DIR/scripts/deploy_release_bundle.py" local-update --root "$INSTALL_DIR" --project-name "$PROJECT_NAME")
      local image
      for image in "${UPDATE_IMAGES[@]}"; do update+=(--image "$image"); done
      (( ! SKIP_PULL )) || update+=(--skip-pull)
      "${update[@]}"
    else
      python3 "$SOURCE_DIR/scripts/deploy_release_bundle.py" start --root "$INSTALL_DIR" --project-name "$PROJECT_NAME"
    fi
  else
    PROJECT_NAME="${PROJECT_NAME:-piping-${USER:-user}-$(basename -- "$INSTALL_DIR")}"
    PROJECT_NAME="$(printf '%s' "$PROJECT_NAME" | tr '[:upper:]' '[:lower:]' | tr -cs 'a-z0-9_-' '-')"
    [[ "$PROJECT_NAME" =~ ^[a-z0-9][a-z0-9_-]{0,62}$ ]] || die "Invalid Compose project name"
    local port
    local -A seen=()
    for port in "$HTTP_PORT" "$HTTPS_PORT" "$POSTGRES_PORT" "$REDIS_PORT"; do
      validate_port "$port"
      [[ -z "${seen[$port]:-}" ]] || die "Ports must be unique"
      seen[$port]=1
    done
    validate_public_host
    local existing_containers existing_volumes
    existing_containers="$(docker ps --all --quiet --filter "label=com.docker.compose.project=$PROJECT_NAME")"
    existing_volumes="$(docker volume ls --quiet --filter "label=com.docker.compose.project=$PROJECT_NAME")"
    [[ -z "$existing_containers" && -z "$existing_volumes" ]] ||
      die "Compose project already has containers or data volumes; restore its original .env or choose a different --project-name"
    prepare_layout
    deploy_stack
    rm -- "$INSTALL_DIR/.fresh-install-in-progress"
  fi
  print_success
}

if [[ "${BASH_SOURCE[0]:-$0}" == "$0" ]]; then main "$@"; fi
