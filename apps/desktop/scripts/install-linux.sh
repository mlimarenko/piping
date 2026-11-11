#!/usr/bin/env bash
# Non-interactive Linux installation of Piping Desktop.
#
# Installs the AppImage into a fixed per-user prefix, optionally pre-fills the
# instance address and creates a desktop entry. Safe to run again: the binary is
# replaced atomically and an existing app.json is kept unless --force-config is
# given.
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: install-linux.sh --appimage PATH [options]

  --appimage PATH        Piping AppImage to install (required)
  --service-url URL      Pre-fill the instance address in app.json
  --prefix DIR           Installation directory (default: ~/.local/opt/piping)
  --sha256 HEX           Expected SHA-256 of the AppImage
  --direct               Put --service-url into the desktop entry so the
                         chooser is skipped for this instance
  --no-desktop-entry     Do not create ~/.local/share/applications/piping.desktop
  --force-config         Rewrite app.json even if it already exists
  -h, --help             Show this help

The configuration directory is ${PIPING_DESKTOP_CONFIG_DIR} when set, otherwise
${XDG_CONFIG_HOME}/piping or ~/.config/piping. The client also accepts
PIPING_DESKTOP_SERVICE_URL and --service-url at runtime.
USAGE
}

die() {
  printf 'install-linux.sh: %s\n' "$*" >&2
  exit 1
}

appimage=""
service_url=""
prefix=""
expected_sha=""
direct=false
desktop_entry=true
force_config=false

while [ "$#" -gt 0 ]; do
  case "$1" in
    --appimage) appimage="${2:-}"; shift 2 ;;
    --appimage=*) appimage="${1#*=}"; shift ;;
    --service-url) service_url="${2:-}"; shift 2 ;;
    --service-url=*) service_url="${1#*=}"; shift ;;
    --prefix) prefix="${2:-}"; shift 2 ;;
    --prefix=*) prefix="${1#*=}"; shift ;;
    --sha256) expected_sha="${2:-}"; shift 2 ;;
    --sha256=*) expected_sha="${1#*=}"; shift ;;
    --direct) direct=true; shift ;;
    --no-desktop-entry) desktop_entry=false; shift ;;
    --force-config) force_config=true; shift ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown argument: $1" ;;
  esac
done

[ -n "${HOME:-}" ] || die "HOME must be set"
[ "${HOME#/}" != "$HOME" ] || die "HOME must be an absolute path"
[ -n "$prefix" ] || prefix="${HOME}/.local/opt/piping"
[ -n "$appimage" ] || die "--appimage is required"
[ -f "$appimage" ] || die "AppImage not found: $appimage"
[ -n "$prefix" ] || die "--prefix must not be empty"
[ "${prefix#/}" != "$prefix" ] || die "--prefix must be an absolute path"

if [ -n "$expected_sha" ]; then
  printf '%s  %s\n' "$expected_sha" "$appimage" | sha256sum -c - >/dev/null \
    || die "AppImage checksum does not match"
fi

if [ -n "$service_url" ]; then
  [[ "$service_url" =~ ^https?://[A-Za-z0-9._~:/?#@!\$\&\'\(\)\*+,\;=%-]+$ ]] \
    || die "--service-url must be an absolute http(s) URL without spaces or quotes"
  if [[ "$service_url" =~ ^[a-zA-Z][a-zA-Z0-9+.-]*://[^/]*@ ]]; then
    die "--service-url must not contain credentials"
  fi
fi

if [ -n "${PIPING_DESKTOP_CONFIG_DIR:-}" ]; then
  config_dir="$PIPING_DESKTOP_CONFIG_DIR"
  [ "${config_dir#/}" != "$config_dir" ] || die "PIPING_DESKTOP_CONFIG_DIR must be absolute"
elif [ -n "${XDG_CONFIG_HOME:-}" ] && [ "${XDG_CONFIG_HOME#/}" != "$XDG_CONFIG_HOME" ]; then
  config_dir="${XDG_CONFIG_HOME}/piping"
else
  config_dir="${HOME}/.config/piping"
fi

mkdir -p "$prefix"
install -m 0755 "$appimage" "$prefix/.Piping.AppImage.new"
mv -f "$prefix/.Piping.AppImage.new" "$prefix/Piping.AppImage"

mkdir -p "$config_dir"
if [ -n "$service_url" ]; then
  if [ -f "$config_dir/app.json" ] && [ "$force_config" != true ]; then
    printf 'install-linux.sh: keeping existing %s; pass --force-config to replace it\n' \
      "$config_dir/app.json" >&2
  else
    pending="$config_dir/.app.json.pending.$$"
    trap 'rm -f "$pending"' EXIT
    printf '{\n  "serviceUrl": "%s",\n  "services": [\n    "%s"\n  ]\n}\n' \
      "$service_url" "$service_url" >"$pending"
    mv -f "$pending" "$config_dir/app.json"
    trap - EXIT
  fi
fi

if [ "$desktop_entry" = true ]; then
  if [ -n "${XDG_DATA_HOME:-}" ] && [ "${XDG_DATA_HOME#/}" != "$XDG_DATA_HOME" ]; then
    data_home="$XDG_DATA_HOME"
  else
    data_home="$HOME/.local/share"
  fi
  applications_dir="${data_home}/applications"
  mkdir -p "$applications_dir"
  exec_line="\"${prefix}/Piping.AppImage\""
  if [ "$direct" = true ] && [ -n "$service_url" ]; then
    # Desktop Entry reserves '%' for field codes; a literal percent must be doubled.
    exec_url="${service_url//%/%%}"
    exec_line="\"${prefix}/Piping.AppImage\" --service-url \"${exec_url}\""
  fi
  desktop_file="${applications_dir}/piping.desktop"
  {
    printf '[Desktop Entry]\n'
    printf 'Type=Application\n'
    printf 'Name=Piping\n'
    printf 'Comment=Piping desktop client\n'
    printf 'Exec=%s\n' "$exec_line"
    printf 'Terminal=false\n'
    printf 'Categories=Network;\n'
  } >"$desktop_file"
  chmod 0644 "$desktop_file"
fi

printf 'Piping Desktop installed: %s/Piping.AppImage\n' "$prefix"
if [ -n "$service_url" ]; then
  printf 'Instance pre-filled in %s/app.json\n' "$config_dir"
fi
