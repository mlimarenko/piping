#!/usr/bin/env bash
set -euo pipefail

commit_msg_file="${1:?commit message file is required}"
subject="$(sed -n '1p' "$commit_msg_file")"
type_re='(web|backend|docs|chore|build|fix|feat|refactor|test|ci|perf|style|deploy)'
scope_re='[a-z0-9]+([_+-][a-z0-9]+)*'
pattern="^${type_re}(\\(${scope_re}\\))?: [^[:space:]].*$"

if [[ ! "$subject" =~ $pattern ]]; then
  cat >&2 <<'EOF'
Invalid commit message.

Expected: <type>: <subject> or <type>(<scope>): <subject>
Allowed types: web, backend, docs, chore, build, fix, feat, refactor, test, ci, perf, style, deploy
Scope: optional short lowercase identifier, e.g. ssh, connection-profiles, api_cache, phase-3+5
EOF
  exit 1
fi

message_subject="${subject#*: }"

if [[ "$message_subject" == *. ]]; then
  echo "Invalid commit message: subject must not end with a period." >&2
  exit 1
fi

if ((${#subject} > 100)); then
  echo "Invalid commit message: first line must be 100 characters or fewer." >&2
  exit 1
fi

if ((${#subject} > 72)); then
  echo "Warning: commit message first line is over the recommended 72 characters." >&2
fi
