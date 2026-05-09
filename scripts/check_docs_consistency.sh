#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

require_doc() {
  local pattern="$1"
  local file="$2"
  if ! rg -Fq -- "$pattern" "$file"; then
    echo "missing '$pattern' in $file" >&2
    exit 1
  fi
}

for endpoint in \
  "/api/code/session" \
  "/api/code/events" \
  "/api/code/diagnostics" \
  "/api/code/controller/attach" \
  "/api/code/controller/detach" \
  "/api/code/messages" \
  "/api/code/interactions/{id}" \
  "/api/code/control/cancel"
do
  require_doc "$endpoint" "docs/automation/local-tui-control.md"
done

for header_name in "X-Libra-Control-Token" "X-Code-Controller-Token"; do
  require_doc "$header_name" "docs/automation/local-tui-control.md"
  rg -qi -- "$header_name" src docs >/dev/null
done

for code in \
  "CONTROL_DISABLED" \
  "LOOPBACK_REQUIRED" \
  "MISSING_CONTROL_TOKEN" \
  "INVALID_CONTROL_TOKEN" \
  "MISSING_CONTROLLER_TOKEN" \
  "INVALID_CONTROLLER_TOKEN" \
  "CONTROLLER_CONFLICT" \
  "SESSION_BUSY" \
  "INTERACTION_NOT_ACTIVE"
do
  rg -q -- "$code" docs src >/dev/null
done

for flag in "--control" "--control-token-file" "--control-info-file"; do
  require_doc "$flag" "docs/commands/code.md"
done

require_doc "code-control --stdio" "docs/commands/code-control.md"
require_doc "diagnostics.get" "docs/commands/code-control.md"
require_doc "test-provider" "docs/automation/local-tui-control.md"
require_doc "code_ui_scenarios" "docs/automation/local-tui-control.md"
require_doc "target/code-ui-scenarios" "docs/automation/local-tui-control.md"
require_doc "Scenario::new" "docs/automation/local-tui-control.md"
require_doc "diagnostics_redaction_test" "docs/improvement/tui.md"
require_doc "Run TUI automation scenarios" ".github/workflows/base.yml"

for required_file in \
  "tests/harness/scenario.rs" \
  "tests/diagnostics_redaction_test.rs" \
  "tests/code_codex_default_tui_test.rs"
do
  if [[ ! -f "$required_file" ]]; then
    echo "missing required TUI automation artifact: $required_file" >&2
    exit 1
  fi
done

if rg -n -- "RUST_LOG:" .github/workflows/base.yml >/dev/null; then
  echo "Run TUI automation scenarios must not set global RUST_LOG in CI" >&2
  exit 1
fi
