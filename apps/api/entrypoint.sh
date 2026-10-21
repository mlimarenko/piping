#!/usr/bin/env sh
set -eu

mode="${PIPING_START_MODE:-start}"
run_mode="${PIPING_RUN_MODE:-backend}"

start_embedded_guacd() {
  # Start guacd alongside the server so browser RDP works in a single container.
  # We ship our own guacd (built from upstream) to avoid distro/version drift.
  # No fallback to system guacd - we want deterministic behavior.
  guacd_bin="/usr/local/sbin/guacd"
  if [ ! -x "${guacd_bin}" ]; then
    echo "guacd binary not found at ${guacd_bin}; browser RDP will not work" >&2
    exit 1
  fi

  guacd_bind="127.0.0.1"
  guacd_port="4822"
  guacd_drive_base="${PIPING_PROXY__RDP_DRIVE_BASE_PATH:-/tmp/guac-drives}"
  guacd_log_level="${PIPING_GUACD_LOG_LEVEL:-info}"

  mkdir -p "${guacd_drive_base}"
  if ! chmod 1777 "${guacd_drive_base}" 2>/dev/null && [ ! -w "${guacd_drive_base}" ]; then
    echo "RDP drive base is not writable: ${guacd_drive_base}" >&2
    exit 1
  fi
  export PIPING_PROXY__RDP_DRIVE_BASE_PATH="${guacd_drive_base}"
  export PIPING_PROXY__EMBEDDED_GUACD_DRIVE_BASE="${guacd_drive_base}"
  echo "Starting embedded guacd on ${guacd_bind}:${guacd_port}"
  "${guacd_bin}" -L "${guacd_log_level}" -b "${guacd_bind}" -l "${guacd_port}" -f &
  guacd_pid="$!"

  # Best-effort sanity check: fail fast if guacd crashed immediately.
  sleep 0.1
  if ! kill -0 "${guacd_pid}" >/dev/null 2>&1; then
    echo "guacd failed to start" >&2
    exit 1
  fi
}

should_start_embedded_guacd() {
  mode="${PIPING_START_EMBEDDED_GUACD:-auto}"
  guacd_host="${PIPING_PROXY__GUACD_HOST:-127.0.0.1}"

  case "${mode}" in
    true)
      return 0
      ;;
    false)
      return 1
      ;;
    auto)
      case "${guacd_host}" in
        ""|127.0.0.1|localhost|::1)
          return 0
          ;;
        *)
          return 1
          ;;
      esac
      ;;
    *)
      echo "Unknown PIPING_START_EMBEDDED_GUACD: ${mode}" >&2
      exit 1
      ;;
  esac
}

case "$mode" in
  migrate)
    echo "Running migrations only"
    export PIPING_START_MODE="migrate"
    export PIPING_RUN_MODE="$run_mode"
    exec /usr/local/bin/piping-backend
    ;;
  start|serve)
    export PIPING_RUN_MODE="$run_mode"
    if should_start_embedded_guacd; then
      start_embedded_guacd
    else
      echo "Using external guacd at ${PIPING_PROXY__GUACD_HOST:-127.0.0.1}:${PIPING_PROXY__GUACD_PORT:-4822}"
    fi
    exec /usr/local/bin/piping-backend
    ;;
  *)
    echo "Unknown PIPING_START_MODE: $mode" >&2
    echo "Expected: start|serve|migrate" >&2
    exit 1
    ;;
esac
