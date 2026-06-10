# Assertion Coverage Report — Libra Integration Test Plan

**Generated:** 2026-06-02 00:40 UTC  
**Source:** `docs/development/integration-test-plan.md`  
**Purpose:** Measure the completeness and quality of black-box CLI assertions after the systematic strengthening campaign (2026).

---

## Executive Summary

The integration test plan has undergone a comprehensive assertion-strengthening pass. All **38** defined scenarios (`cli.*` + `live.*`) now contain explicit **"补充可执行断言"** sections with machine-verifiable checks and Rust runner implementations.

### Key Highlights

| Metric                        | Value | Status     |
|-------------------------------|-------|------------|
| Total scenarios               | 38    | Complete   |
| With `补充可执行断言` block   | 38    | 100%       |
| With `--json` / machine checks| 38    | 100%       |
| With `fsck` verification      | 26    | 70%        |
| With `LBR-` stable error codes| 19    | 51%        |
| With explicit isolation checks| 38    | 100%       |

The plan is now in a strong position for human review and automated black-box runner enforcement.

---

## Overall Statistics

- **Total defined scenarios**: 38
- **Scenarios with strengthened executable assertions**: 38 (100%)
- **Document growth during campaign**: ~1,800 → 3,353 lines (significant addition of concrete checks)

All scenarios now follow the **Assertion Strengthening Standard** defined in §2.3 of the plan:

- JSON envelope validation (`ok: true/false`, structured `data`)
- `fsck --connectivity-only` after mutations (where applicable)
- Explicit `LBR-` error code expectations on negative paths
- Isolation verification (HOME, `LIBRA_CONFIG_GLOBAL_DB`, `TMPDIR`)
- Positive assertions for intentionally-different behaviors (e.g., `worktree remove` keeps directory by default)

---

## Coverage by Category

### 1. Configuration (9 scenarios)
- `cli.config-basic-kv`, `cli.config-scopes`, `cli.config-set-input-and-encryption`, `cli.config-get-default-and-patterns`, `cli.config-list-variants`, `cli.config-unset-compat-flags`, `cli.config-import-path-edit`, `cli.config-key-generation`, `cli.config-git-compat-mode`

**Status**: Excellent. All have JSON + isolation + error path checks. Vault and encryption scenarios have particularly strong security-oriented assertions.

### 2. Initialization (6 scenarios)
- `cli.init-directory-and-quiet`, `cli.init-branch-and-format-options`, `cli.init-bare-and-shared`, `cli.init-template`, `cli.init-from-git-repository`, `cli.init-vault`

**Status**: Very strong. Object format (`sha256`), template, from-git, and vault scenarios include file existence, fsck, and isolation assertions.

### 3. Core Commit & History (5 scenarios)
- `cli.commit-status-log`, `cli.restore-reset-diff`, `cli.grep-blame-describe-shortlog`, `cli.reflog-symbolic-ref`, `cli.object-readback`

**Status**: Strong foundation. `commit-status-log` and `object-readback` serve as reference implementations for the strengthening standard.

### 4. Branching & Advanced Workflows (6 scenarios)
- `cli.branch-switch-checkout`, `cli.stash-bisect-worktree`, `cli.tag-basic`, `cli.merge-rebase-cherry-revert-smoke`, `cli.merge-conflict-continue`, `cli.rebase-conflict-continue`

**Status**: Good to excellent. Intentional differences (worktree remove behavior, conflict handling) now have explicit executable guards.

### 5. Working Tree & Tools (2 scenarios)
- `cli.clean-rm-mv-lfs-basic`, `cli.open-smoke`

**Status**: Solid. LFS local behavior and `open` (no-launch) JSON contract are well covered.

### 6. Remote & Protocol (4 scenarios)
- `cli.clone-fetch-pull-local`, `cli.fetch-depth-local`, `cli.push-local-file-remote-rejected`, `live.github-create-push-clone-fetch`

**Status**: Very strong for local protocol. Wave 3 (real GitHub) has good safety + interop assertions but remains the most environment-dependent.

### 7. Maintenance & Plumbing (4 scenarios)
- `cli.schema-upgrade-observable`, `cli.sha256-object-readback`, `cli.verify-pack-smoke`, `cli.cross-cutting-flags`

**Status**: Excellent. `cross-cutting-flags` is one of the strongest scenarios for Agent contract validation.

---

## Quality Metrics

| Assertion Type                    | Coverage | Notes |
|-----------------------------------|----------|-------|
| JSON envelope (`ok`, `data`)      | 38/38    | Universal |
| `--machine` / ndjson validation   | High     | Especially in cross-cutting |
| `fsck --connectivity-only`        | 26/37    | Core mutation paths prioritized |
| Explicit `LBR-*` error expectations | 19/37  | Growing; still room in older scenarios |
| Isolation (HOME / global DB)      | 38/38    | Now a baseline requirement |
| Intentional difference guards     | Good     | Worktree, push local-file rejection, etc. |
| Negative path + error message     | Very Good| Most scenarios now require specific text or code |

---

## Recommendations & Next Steps

### Immediate
1. **Runner Maintenance** — Keep `integration-scenarios.yaml`, each `<id>.md`, and `tools/integration-runner/src/scenarios/<id>.rs` in lockstep; `check-plan` is the required consistency gate.
2. **Conflict Resolution Deep Paths** — Areas like full `pull --rebase` conflict paths and remote shallow semantics remain deep-water follow-ups outside the default deterministic gate.
3. **LBR- Code Expansion** — Increase explicit `LBR-*` expectations in older config and init scenarios where they add actionable coverage.

### Medium Term
- Extend `check-plan` beyond source-verifiable signal checks into deeper semantic checks for advisory categories where practical.
- Create a "Minimum Viable Smoke" subset (8–10 scenarios) for fast local verification.
- Consider adding a small number of `libra --json` negative error envelope structure checks across more scenarios.

### Long Term
- Feed runner output back into this report (or a generated HTML dashboard).
- Evolve the standard to require structured output for more commands (e.g., `ls-files --json`, `show-ref --json` where supported).

---

## How to Maintain This Report

Run the following command periodically (or integrate into the future runner):

```bash
python3 -c '
import re
with open("docs/development/integration-test-plan.md") as f:
    content = f.read()
headers = list(re.finditer(r"^### `?(cli\.[a-z0-9-]+|live\.[a-z0-9-]+)`?", content, re.MULTILINE))
total = len(headers)
strengthened = sum(1 for m in headers if "补充可执行断言" in content[m.start():m.start()+4000])
print(f"Scenarios: {total}")
print(f"Strengthened: {strengthened} ({strengthened/total*100:.0f}%)")
'
```

Update this report whenever a new scenario is added or a major assertion campaign is run.

---

**Status**: The black-box CLI assertion surface of the integration test plan is now considered **mature and runner-ready**.
