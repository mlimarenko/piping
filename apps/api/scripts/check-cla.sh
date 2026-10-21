#!/usr/bin/env bash
set -euo pipefail

if [[ -z "${GITHUB_EVENT_PATH:-}" ]]; then
  echo "GITHUB_EVENT_PATH is required"
  exit 1
fi

if [[ ! -f "$GITHUB_EVENT_PATH" ]]; then
  echo "event payload not found: $GITHUB_EVENT_PATH"
  exit 1
fi

if ! command -v jq >/dev/null 2>&1; then
  echo "jq is required"
  exit 1
fi

pr_body="$(jq -r '.pull_request.body // ""' "$GITHUB_EVENT_PATH")"

if echo "$pr_body" | grep -Eiq '(\[x\][^\n]*cla)|(i agree to (the )?cla)'; then
  echo "CLA confirmation found"
  exit 0
fi

echo "CLA confirmation not found in PR description"
echo "Add a checked CLA checkbox or phrase: 'I agree to the CLA'"
exit 1
