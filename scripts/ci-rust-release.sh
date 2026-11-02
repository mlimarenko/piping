#!/usr/bin/env bash
set -euo pipefail

operation="${1:-}"
case "$operation" in
    build|check) shift ;;
    *) echo "Usage: ci-rust-release.sh {build|check} [cargo arguments]" >&2; exit 2 ;;
esac

encoded_public_key="${PIPING_LICENSE_BAKED_PUBLIC_KEY_PEM_B64:-}"
if [[ -n "$encoded_public_key" ]]; then
  if ! decoded_public_key="$(printf '%s' "$encoded_public_key" | base64 --decode 2>/dev/null)"; then
    echo "PIPING_LICENSE_BAKED_PUBLIC_KEY_PEM_B64 is not valid base64" >&2
    exit 1
  fi
  export PIPING_LICENSE_BAKED_PUBLIC_KEY_PEM="$decoded_public_key"
elif [[ -z "${PIPING_LICENSE_BAKED_PUBLIC_KEY_PEM:-}" ]]; then
  echo "PIPING_LICENSE_BAKED_PUBLIC_KEY_PEM_B64 is required for a release build" >&2
  exit 1
fi

unset encoded_public_key decoded_public_key PIPING_LICENSE_BAKED_PUBLIC_KEY_PEM_B64
bash scripts/validate-license-public-key.sh

if [[ "$#" -eq 0 ]]; then
  set -- --workspace --release
fi
exec cargo "$operation" --locked "$@"
