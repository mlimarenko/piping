#!/usr/bin/env bash
set -euo pipefail
IFS=$'\n\t'
umask 077

# Rendered by the source publisher, never selected from the caller's environment.
PUBLIC_VERSION='1.0.0'
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
EDGE_BIND=""
POSTGRES_BIND=""
REDIS_BIND=""
FRONTEND_BIND=""
HOST_CPUS=""
RES_SMALL_CPUS=""
RES_SMALL_MEM=""
RES_MEDIUM_CPUS=""
RES_MEDIUM_MEM=""
RES_LARGE_CPUS=""
RES_LARGE_MEM=""
RUNNER_CPUS=""
RUNNER_MEM=""
PROJECT_NAME=""
BACKEND_IMAGE=""
FRONTEND_IMAGE=""
ANSIBLE_RUNNER_IMAGE=""
SOURCE_BASE_URL=""
SKIP_PULL=0
UPDATE_IMAGES=()
LAYOUT_OPTIONS=()
STACK_SOURCE_PATHS=(
  install.sh env/images.env compose.yaml
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
  --edge-bind ADDR          Fresh-install HTTP/HTTPS bind address (default: all)
  --postgres-bind ADDR      Fresh-install PostgreSQL bind address (default: all)
  --redis-bind ADDR         Fresh-install Redis bind address (default: all)
  --frontend-bind ADDR      Fresh-install frontend bind address (default: all)
  --small-cpus CPUS         Small-service CPU limit (default: 1.00, at most host)
  --small-memory SIZE       Small-service memory limit (default: 512M)
  --medium-cpus CPUS        Medium-service CPU limit (default: 2.00, at most host)
  --medium-memory SIZE      Medium-service memory limit (default: 2G)
  --large-cpus CPUS         Large-service CPU limit (default: 4.00, at most host)
  --large-memory SIZE       Large-service memory limit (default: 4G)
  --runner-cpus CPUS        Runner CPU limit (default: 8.00, at most host)
  --runner-memory SIZE      Runner memory limit (default: 4G)
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
Fresh installs require the local /var/run/docker.sock and permission to create
UID/GID 10001 directories (root or passwordless sudo). Jobs have no network by default.
An explicitly activated job network starts the runner through its guarded
systemd unit; activation and deactivation use scripts/deploy_release_bundle.py
as documented in README.md.
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
      for relative in .env .git env docker scripts compose.yaml docker-compose.yml .deploy-release.lock .fresh-install-in-progress .images.env; do
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

validate_bind_address() {
  local address="$1" octet
  local -a octets
  [[ "$address" =~ ^([0-9]{1,3}\.){3}[0-9]{1,3}$ ]] || die "Bind address must be a dotted IPv4 address"
  IFS=. read -r -a octets <<< "$address"
  for octet in "${octets[@]}"; do
    [[ "$octet" == 0 || "$octet" =~ ^[1-9][0-9]{0,2}$ ]] || die "Bind address must not contain leading zeros"
    (( octet <= 255 )) || die "Bind address octet must be between 0 and 255"
  done
}

cpu_default() {
  local preferred="$1" host="$2"
  if (( preferred < host )); then printf '%s.00' "$preferred"; else printf '%s.00' "$host"; fi
}

validate_cpu_limit() {
  local value="$1" whole fraction scaled
  [[ "$value" =~ ^(0|[1-9][0-9]{0,3})(\.([0-9]{1,2}))?$ ]] || die "CPU limit must be a positive decimal with at most two places"
  whole="${BASH_REMATCH[1]}"
  fraction="${BASH_REMATCH[3]:-}"
  fraction="${fraction}00"
  scaled=$((10#$whole * 100 + 10#${fraction:0:2}))
  (( scaled > 0 )) || die "CPU limit must be greater than zero"
  (( scaled <= HOST_CPUS * 100 )) || die "CPU limit $value exceeds host CPUs ($HOST_CPUS)"
}

validate_memory_limit() {
  local value="$1" minimum_mib="$2" family="$3" amount unit
  [[ "$value" =~ ^[1-9][0-9]{0,5}[MmGg]$ ]] || die "Memory limit must be a positive integer with M or G suffix"
  amount="${value%?}"
  unit="${value: -1}"
  [[ "$unit" == [Mm] ]] || amount=$((amount * 1024))
  (( amount >= minimum_mib )) || die "$family memory limit must be at least ${minimum_mib}M to cover its reservation"
}

apply_resource_defaults() {
  if [[ -z "$HOST_CPUS" ]]; then
    require_cmd nproc
    HOST_CPUS="$(nproc)"
    [[ "$HOST_CPUS" =~ ^[1-9][0-9]{0,3}$ ]] || die "Cannot determine host CPU count"
  fi
  RES_SMALL_CPUS="${RES_SMALL_CPUS:-$(cpu_default 1 "$HOST_CPUS")}"
  RES_SMALL_MEM="${RES_SMALL_MEM:-512M}"
  RES_MEDIUM_CPUS="${RES_MEDIUM_CPUS:-$(cpu_default 2 "$HOST_CPUS")}"
  RES_MEDIUM_MEM="${RES_MEDIUM_MEM:-2G}"
  RES_LARGE_CPUS="${RES_LARGE_CPUS:-$(cpu_default 4 "$HOST_CPUS")}"
  RES_LARGE_MEM="${RES_LARGE_MEM:-4G}"
  RUNNER_CPUS="${RUNNER_CPUS:-$(cpu_default 8 "$HOST_CPUS")}"
  RUNNER_MEM="${RUNNER_MEM:-4G}"
}

validate_resource_limits() {
  validate_cpu_limit "$RES_SMALL_CPUS"
  validate_cpu_limit "$RES_MEDIUM_CPUS"
  validate_cpu_limit "$RES_LARGE_CPUS"
  validate_cpu_limit "$RUNNER_CPUS"
  validate_memory_limit "$RES_SMALL_MEM" 128 Small
  validate_memory_limit "$RES_MEDIUM_MEM" 512 Medium
  validate_memory_limit "$RES_LARGE_MEM" 1024 Large
  validate_memory_limit "$RUNNER_MEM" 1024 Runner
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
      --edge-bind) EDGE_BIND="$2"; LAYOUT_OPTIONS+=("PIPING_EDGE_BIND=$2") ;;
      --postgres-bind) POSTGRES_BIND="$2"; LAYOUT_OPTIONS+=("PIPING_POSTGRES_BIND=$2") ;;
      --redis-bind) REDIS_BIND="$2"; LAYOUT_OPTIONS+=("PIPING_REDIS_BIND=$2") ;;
      --frontend-bind) FRONTEND_BIND="$2"; LAYOUT_OPTIONS+=("PIPING_FRONTEND_BIND=$2") ;;
      --small-cpus) RES_SMALL_CPUS="$2"; LAYOUT_OPTIONS+=("PIPING_RES_SMALL_CPUS=$2") ;;
      --small-memory) RES_SMALL_MEM="$2"; LAYOUT_OPTIONS+=("PIPING_RES_SMALL_MEM=$2") ;;
      --medium-cpus) RES_MEDIUM_CPUS="$2"; LAYOUT_OPTIONS+=("PIPING_RES_MEDIUM_CPUS=$2") ;;
      --medium-memory) RES_MEDIUM_MEM="$2"; LAYOUT_OPTIONS+=("PIPING_RES_MEDIUM_MEM=$2") ;;
      --large-cpus) RES_LARGE_CPUS="$2"; LAYOUT_OPTIONS+=("PIPING_RES_LARGE_CPUS=$2") ;;
      --large-memory) RES_LARGE_MEM="$2"; LAYOUT_OPTIONS+=("PIPING_RES_LARGE_MEM=$2") ;;
      --runner-cpus) RUNNER_CPUS="$2"; LAYOUT_OPTIONS+=("PIPING_ANSIBLE_RUNNER_CPUS=$2") ;;
      --runner-memory) RUNNER_MEM="$2"; LAYOUT_OPTIONS+=("PIPING_ANSIBLE_RUNNER_MEM=$2") ;;
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
  prepare_sandbox_roots
  BACKEND_IMAGE="${BACKEND_IMAGE:-$(read_env_value PIPING_BACKEND_IMAGE "$INSTALL_DIR/env/images.env")}"
  FRONTEND_IMAGE="${FRONTEND_IMAGE:-$(read_env_value PIPING_FRONTEND_IMAGE "$INSTALL_DIR/env/images.env")}"
  ANSIBLE_RUNNER_IMAGE="${ANSIBLE_RUNNER_IMAGE:-$(read_env_value PIPING_ANSIBLE_RUNNER_IMAGE "$INSTALL_DIR/env/images.env")}"
  for relative in "$BACKEND_IMAGE" "$FRONTEND_IMAGE" "$ANSIBLE_RUNNER_IMAGE"; do validate_image_reference "$relative"; done
  apply_resource_defaults
  validate_resource_limits
  local private_env database_password redis_password cipher_key pepper administrator_password
  database_password="$(openssl rand -hex 24)"
  redis_password="$(openssl rand -hex 24)"
  cipher_key="$(openssl rand -base64 32)"
  pepper="$(openssl rand -hex 24)"
  administrator_password="$(openssl rand -hex 16)"
  private_env="$(mktemp "$INSTALL_DIR/.env.XXXXXX")"
  {
    printf 'COMPOSE_PROJECT_NAME=%s\n' "$PROJECT_NAME"
    printf 'PIPING_HTTP_PORT=%s\nPIPING_HTTPS_PORT=%s\nPIPING_EDGE_BIND=%s\nPIPING_PUBLIC_HOST=%s\n' "$HTTP_PORT" "$HTTPS_PORT" "$EDGE_BIND" "$PUBLIC_HOST"
    printf 'PIPING_POSTGRES_PORT=%s\nPIPING_POSTGRES_BIND=%s\nPIPING_REDIS_PORT=%s\nPIPING_REDIS_BIND=%s\nPIPING_FRONTEND_BIND=%s\n' "$POSTGRES_PORT" "$POSTGRES_BIND" "$REDIS_PORT" "$REDIS_BIND" "$FRONTEND_BIND"
    printf 'PIPING_RES_SMALL_CPUS=%s\nPIPING_RES_SMALL_MEM=%s\nPIPING_RES_MEDIUM_CPUS=%s\nPIPING_RES_MEDIUM_MEM=%s\nPIPING_RES_LARGE_CPUS=%s\nPIPING_RES_LARGE_MEM=%s\n' "$RES_SMALL_CPUS" "$RES_SMALL_MEM" "$RES_MEDIUM_CPUS" "$RES_MEDIUM_MEM" "$RES_LARGE_CPUS" "$RES_LARGE_MEM"
    printf 'PIPING_ANSIBLE_RUNNER_CPUS=%s\nPIPING_ANSIBLE_RUNNER_MEM=%s\n' "$RUNNER_CPUS" "$RUNNER_MEM"
    printf 'PIPING_DOCKER_GID=%s\nPIPING_ANSIBLE_STATE_ROOT=%s\nPIPING_ANSIBLE_JOB_ROOT=%s\nPIPING_ANSIBLE_JOB_NETWORK=\n' "$DOCKER_GID" "$INSTALL_DIR/ansible-state" "$INSTALL_DIR/ansible-jobs"
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
  write_progress_marker
}

prepare_sandbox_roots() {
  local root
  for root in "$INSTALL_DIR/ansible-state" "$INSTALL_DIR/ansible-jobs"; do
    [[ ! -e "$root" && ! -L "$root" ]] || die "Sandbox directory already exists; inspect it before recovery: $root"
  done
  for root in "$INSTALL_DIR/ansible-state" "$INSTALL_DIR/ansible-jobs"; do
    mkdir -m 700 -- "$root"
    if (( EUID == 0 )); then
      chown 10001:10001 -- "$root"
    else
      sudo -n chown 10001:10001 -- "$root"
    fi
    [[ "$(stat -c '%u:%g:%a' -- "$root")" == 10001:10001:700 ]] || die "Sandbox directory ownership or mode is incorrect: $root"
  done
}

validate_local_docker_endpoint() {
  # Do not infer precedence from `docker context show`: CLI versions differ in
  # how it reports simultaneous DOCKER_HOST and DOCKER_CONTEXT overrides.
  [[ -z "${DOCKER_HOST:-}" || -z "${DOCKER_CONTEXT:-}" ]] ||
    die "Set only one of DOCKER_HOST and DOCKER_CONTEXT for fresh installation"
  if [[ -n "${DOCKER_HOST:-}" ]]; then
    [[ "$DOCKER_HOST" == unix:///var/run/docker.sock ]] || die "Fresh installation requires the local Docker socket, not a remote daemon"
  else
    local context="${DOCKER_CONTEXT:-}"
    [[ -n "$context" ]] || context="$(docker context show)"
    [[ "$(docker context inspect "$context" --format '{{.Endpoints.docker.Host}}')" == unix:///var/run/docker.sock ]] ||
      die "Fresh installation requires a Docker context using /var/run/docker.sock"
  fi
}

run_compose() {
  # Fresh installation and guarded updates use the same environment boundary.
  python3 -c 'import os,sys; sys.path.insert(0,sys.argv[1]); from scripts.deploy_release_bundle import environment; os.execvpe(sys.argv[2],sys.argv[2:],environment())' "$INSTALL_DIR" "$@"
}

fresh_install_state() {
  python3 - "$INSTALL_DIR" "$1" "${STACK_SOURCE_PATHS[@]}" -- "${@:2}" <<'PY'
import hashlib
import sys
from pathlib import Path
root, action = Path(sys.argv[1]), sys.argv[2]
sys.path.insert(0, str(root))
from scripts.deploy_release_bundle import render_images, write_atomic
settings = root / ".env"
marker = root / ".fresh-install-in-progress"
divider = sys.argv.index("--", 3)
paths = [".env", *sys.argv[3:divider]]
def checksum(data):
    return hashlib.sha256(data).hexdigest()
def current_marker():
    return "".join(checksum((root / name).read_bytes()) + "  " + name + "\n" for name in paths).encode()
if action == "check":
    entries = {}
    for line in marker.read_text().splitlines():
        digest, separator, name = line.partition("  ")
        if not separator or len(digest) != 64 or any(c not in "0123456789abcdef" for c in digest):
            raise SystemExit(1)
        entries.setdefault(name, []).append(digest)
    if set(entries) != set(paths) or any(
        len(entries[name]) not in ((1, 2) if name == ".env" else (1,))
        or checksum((root / name).read_bytes()) not in entries[name] for name in paths
    ):
        raise SystemExit(1)
elif action == "write":
    write_atomic(marker, current_marker())
elif action == "pin":
    import subprocess
    identities = {}
    references = sys.argv[divider + 1:]
    if len(references) != 3:
        raise SystemExit(1)
    for role, reference in zip(("backend", "frontend", "ansible-runner"), references):
        identity = subprocess.check_output(["docker", "image", "inspect", "--format", "{{.Id}}", reference], text=True).strip()
        if not identity.startswith("sha256:") or len(identity) != 71 or any(c not in "0123456789abcdef" for c in identity[7:]):
            raise SystemExit("Pulled images must have an immutable identity")
        identities[role] = identity
    normalized = render_images(settings.read_bytes(), identities)
    # Record both exact settings states before replacing either file. A retry
    # can authenticate an interrupted normalization, but not arbitrary edits.
    write_atomic(marker, current_marker() + (checksum(normalized) + "  .env\n").encode())
    write_atomic(settings, normalized)
    write_atomic(marker, current_marker())
else:
    raise SystemExit(1)
PY
}

write_progress_marker() { fresh_install_state write; }

installed_images_pinned() {
  local key
  for key in PIPING_BACKEND_IMAGE PIPING_FRONTEND_IMAGE PIPING_ANSIBLE_RUNNER_IMAGE; do
    [[ "$(read_env_value "$key" "$INSTALL_DIR/.env")" == sha256:* ]] || return 1
  done
}

pin_installed_images() {
  # Offline recovery reconnects a saved image only by its immutable local ID;
  # a registry digest name cannot be resolved after loading the archive. The
  # pulled digest still verified the content, and the updater uses this form.
  fresh_install_state pin "$(read_env_value PIPING_BACKEND_IMAGE "$INSTALL_DIR/.env")" \
    "$(read_env_value PIPING_FRONTEND_IMAGE "$INSTALL_DIR/.env")" \
    "$(read_env_value PIPING_ANSIBLE_RUNNER_IMAGE "$INSTALL_DIR/.env")"
}

deploy_stack() {
  local -a compose=(docker compose --project-name "$PROJECT_NAME" --project-directory "$INSTALL_DIR" --env-file "$INSTALL_DIR/.env")
  [[ ! -f "$INSTALL_DIR/.images.env" ]] || compose+=(--env-file "$INSTALL_DIR/.images.env")
  compose+=(--profile edge -f "$INSTALL_DIR/compose.yaml")
  if (( ! SKIP_PULL )); then
    local registry_services
    registry_services="$(run_compose "${compose[@]}" config --format json | python3 -c 'import json,sys; print("\n".join(name for name, service in json.load(sys.stdin)["services"].items() if not service["image"].startswith("sha256:") and (not service.get("profiles") or "edge" in service["profiles"])))')"
    local -a pull_services=()
    if [[ -n "$registry_services" ]]; then
      mapfile -t pull_services <<< "$registry_services"
      run_compose "${compose[@]}" pull --policy missing "${pull_services[@]}"
    fi
  fi
  if ! installed_images_pinned; then
    pin_installed_images
  fi
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
  if [[ ! -f "$INSTALL_DIR/.env" || -e "$INSTALL_DIR/.fresh-install-in-progress" ]]; then
    validate_local_docker_endpoint
  fi
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
        die "Existing installation settings are retained; edit .env and supply matching TLS material explicitly"
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
      fresh_install_state check ||
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
    [[ "$INSTALL_DIR" =~ ^/[a-zA-Z0-9_./-]+$ ]] || die "Fresh installation path may contain only letters, digits, underscore, dot, slash and hyphen"
    [[ -S /var/run/docker.sock ]] || die "Fresh installation requires the local /var/run/docker.sock"
    DOCKER_GID="$(stat -c '%g' /var/run/docker.sock)"
    [[ "$DOCKER_GID" =~ ^[0-9]+$ ]] || die "Cannot determine Docker socket group ID"
    if (( EUID != 0 )); then
      require_cmd sudo
      sudo -n true || die "Fresh installation needs passwordless sudo to assign sandbox directories to UID/GID 10001"
    fi
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
    [[ -z "$EDGE_BIND" ]] || validate_bind_address "$EDGE_BIND"
    [[ -z "$POSTGRES_BIND" ]] || validate_bind_address "$POSTGRES_BIND"
    [[ -z "$REDIS_BIND" ]] || validate_bind_address "$REDIS_BIND"
    [[ -z "$FRONTEND_BIND" ]] || validate_bind_address "$FRONTEND_BIND"
    apply_resource_defaults
    validate_resource_limits
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
