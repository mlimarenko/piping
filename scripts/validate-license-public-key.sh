#!/usr/bin/env bash
set -euo pipefail

public_key="${PIPING_LICENSE_BAKED_PUBLIC_KEY_PEM:-}"
if [[ -z "$public_key" ]]; then
  echo "PIPING_LICENSE_BAKED_PUBLIC_KEY_PEM is required for a release build" >&2
  exit 1
fi
if ! command -v openssl >/dev/null 2>&1; then
  echo "openssl is required to validate the release public key" >&2
  exit 1
fi

umask 077
validation_directory="$(mktemp -d "${TMPDIR:-/tmp}/piping-license-key.XXXXXX")"
pem_path="$validation_directory/public.pem"
der_path="$validation_directory/public.der"
canonical_pem_path="$validation_directory/canonical-public.pem"
cleanup_validation_files() {
  rm -f -- "$pem_path" "$der_path" "$canonical_pem_path"
  rmdir -- "$validation_directory" 2>/dev/null || true
}
trap cleanup_validation_files EXIT

printf '%s' "$public_key" > "$pem_path"
valid_ed25519_spki=false
if openssl pkey -pubin -in "$pem_path" -pubcheck -noout >/dev/null 2>&1 \
  && openssl pkey -pubin -in "$pem_path" -pubout -out "$canonical_pem_path" >/dev/null 2>&1 \
  && openssl pkey -pubin -in "$pem_path" -outform DER -out "$der_path" >/dev/null 2>&1; then
  canonical_public_key="$(<"$canonical_pem_path")"
  der_size="$(wc -c < "$der_path" | tr -d '[:space:]')"
  der_prefix="$(od -An -tx1 -N 12 "$der_path" | tr -d '[:space:]')"
  if [[ ("$public_key" == "$canonical_public_key" || "$public_key" == "$canonical_public_key"$'\n') \
    && "$der_size" == "44" \
    && "$der_prefix" == "302a300506032b6570032100" ]]; then
    valid_ed25519_spki=true
  fi
fi

if [[ "$valid_ed25519_spki" != "true" ]]; then
  echo "PIPING_LICENSE_BAKED_PUBLIC_KEY_PEM must contain exactly one canonical Ed25519 SubjectPublicKeyInfo public key" >&2
  exit 1
fi
