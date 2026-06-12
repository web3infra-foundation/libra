#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  status="$(git status --porcelain -- web/out)"
else
  status="$(
    libra status --short \
      | awk 'substr($0, 4) ~ /^web\/out(\/|$)/ { print }'
  )"
fi

if [[ -n "$status" ]]; then
  echo "web/out has untracked, staged, or unstaged files after the static export build." >&2
  echo "Run 'pnpm --dir web build' locally and commit the updated web/out files." >&2
  printf '%s\n' "$status" >&2
  exit 1
fi
