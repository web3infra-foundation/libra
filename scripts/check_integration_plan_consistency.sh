#!/usr/bin/env bash
#
# Verifies that docs/development/integration-test-plan.md references only real
# test targets, features, and env vars. Also validates tests/flaky_quarantine.toml
# (when present) points at real tests. CI calls this in Wave 0.
#
# Exit codes:
#   0  consistent
#   1  inconsistent (PR must fix before merge)
#

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PLAN="$ROOT/docs/development/integration-test-plan.md"
CARGO="$ROOT/Cargo.toml"
ENV_EXAMPLE="$ROOT/.env.test.example"
TESTS_DIR="$ROOT/tests"
QUARANTINE="$ROOT/tests/flaky_quarantine.toml"

errors=0
warnings=0

err() { echo "ERR: $*" >&2; errors=$((errors + 1)); }
warn() { echo "WARN: $*" >&2; warnings=$((warnings + 1)); }

if [ ! -f "$PLAN" ]; then
  err "integration-test-plan.md not found at $PLAN"
  exit 1
fi

# ---------------------------------------------------------------------------
# 1. Every `--test <name>` in the plan must map to tests/<name>.rs OR a
#    [[test]] entry in Cargo.toml.
# ---------------------------------------------------------------------------
plan_targets=$(grep -oE -- '--test [a-zA-Z][a-zA-Z0-9_]+' "$PLAN" \
  | awk '{print $2}' | sort -u)

declared_cargo_targets=$(awk '
  /^\[\[test\]\]/ { in_test=1; next }
  in_test && /^name = "/ { gsub(/"/, "", $3); print $3; in_test=0 }
' "$CARGO" | sort -u)

for t in $plan_targets; do
  if [ -f "$TESTS_DIR/$t.rs" ]; then continue; fi
  if echo "$declared_cargo_targets" | grep -qx "$t"; then continue; fi
  err "plan references unknown test target: $t"
done

# ---------------------------------------------------------------------------
# 2. Every `--features <flag,flag>` in the plan must be declared in Cargo.toml.
# ---------------------------------------------------------------------------
plan_features=$(grep -oE -- '--features [a-z][a-z0-9,_-]+' "$PLAN" \
  | awk '{print $2}' | tr ',' '\n' | sort -u)

declared_features=$(awk '
  /^\[features\]/ { f=1; next }
  /^\[/           { f=0 }
  f && /^[a-zA-Z]/ { print $1 }
' "$CARGO" | sort -u)

for flag in $plan_features; do
  if echo "$declared_features" | grep -qx "$flag"; then continue; fi
  err "plan references unknown cargo feature: $flag"
done

# ---------------------------------------------------------------------------
# 3. Env var names mentioned in the plan should appear in .env.test.example.
#    Whitelist: LIBRA_TEST_* (introduced by the plan itself) and known
#    runtime/shell vars.
# ---------------------------------------------------------------------------
WHITELIST_RE='^(LIBRA_TEST_|LIBRA_RUN_LIVE$|LIBRA_RUN_PERF$|LIBRA_ENABLE_TEST_LIVE_CLOUD$|LIBRA_ENABLE_TEST_PROVIDER$|LIBRA_CODE_TEST_PROVIDER$|LIBRA_CODE_TEST_MODEL$|CI_AGENT_KEY$)'

plan_envs=$(grep -oE '\b(LIBRA_[A-Z_]+|[A-Z_]+_API_KEY|[A-Z_]+_BASE_URL)\b' "$PLAN" \
  | grep -v '_$'      `# drop wildcard refs like LIBRA_STORAGE_ (from LIBRA_STORAGE_*)` \
  | sort -u)

for e in $plan_envs; do
  if echo "$e" | grep -qE "$WHITELIST_RE"; then continue; fi
  if [ -f "$ENV_EXAMPLE" ] && grep -q "\b$e\b" "$ENV_EXAMPLE"; then continue; fi
  warn "env var referenced in plan but not in .env.test.example: $e"
done

# ---------------------------------------------------------------------------
# 4. flaky_quarantine.toml entries must point at real tests.
# ---------------------------------------------------------------------------
if [ -f "$QUARANTINE" ]; then
  q_tests=$(grep -E '^test\s*=' "$QUARANTINE" \
    | sed -E 's/^test[[:space:]]*=[[:space:]]*"([^"]+)".*/\1/' \
    | sort -u)
  for q in $q_tests; do
    target="${q%%::*}"
    fn="${q##*::}"
    file="$TESTS_DIR/$target.rs"
    if [ ! -f "$file" ]; then
      err "quarantine references nonexistent target: $q (no $file)"
      continue
    fi
    if ! grep -qE "fn[[:space:]]+$fn\b" "$file"; then
      err "quarantine references nonexistent test fn: $q (not found in $file)"
    fi
  done
fi

# ---------------------------------------------------------------------------
echo "Plan consistency: $errors error(s), $warnings warning(s)."
[ "$errors" -eq 0 ]
