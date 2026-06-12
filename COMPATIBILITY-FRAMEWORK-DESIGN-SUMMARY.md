# Libra Enhanced Compatibility Framework: Design Summary

**Document**: Architect-level overview of the compatibility validation framework
**Version**: 1.0
**Status**: Design Phase (Ready for Implementation)
**Date**: 2026-06-12

---

## Problem Statement

Libra maintains a **4-tier Git compatibility matrix** (`COMPATIBILITY.md`), but currently:

1. **Drift is manual to detect**: No automated sync between COMPATIBILITY.md, src/cli.rs, tests/, and integration-scenarios.yaml
2. **Test coverage is opaque**: No visibility into which tests validate which commands/flags
3. **Tier promotions lack governance**: Moving from `partial` → `supported` has no structured validation
4. **Intentionally-different items lack justification**: Security/design rationale not systematically captured
5. **New commands need boilerplate**: Adding a command requires parallel updates in 4+ places (cli.rs, compat doc, test files, integration scenarios)

**Goal**: Apply grit's **test-driven validation** model to Libra's compatibility framework, automating discovery, validation, and governance.

---

## Solution Overview

### Core Idea

Transform compatibility from **human-editable prose** → **automated, evidence-based system** that:

1. **Discovers surfaces automatically**
   - Extract commands from `src/cli.rs` (syn parser)
   - Extract flags from `src/command/<name>::Args` (syn parser)
   - Load integration scenarios from yaml
   - Scan test files (tests/command/*, integration-runner/)

2. **Validates consistency via gates**
   - Tier alignment: `COMPATIBILITY.md` ↔ `data/compatibility/commands/<name>.toml` ↔ code
   - Test coverage: `supported` → 100% passing tests; `partial` → N% passing (explicit threshold)
   - Documentation: `intentionally-different` → security justification + tests
   - CI gate: All three must align before merge

3. **Structures validation evidence**
   - Per-command status TOMLs in `data/compatibility/commands/` (tier, coverage %, blocked tests, flags)
   - Auto-generated inventories in `data/compatibility/discovery/` (commands, scenarios, test map, flags)
   - Justification docs in `docs/improvement/compatibility/` (intentional-differences, declined-features)

4. **Provides operational tooling**
   - `cargo compat validation check` → detect drift
   - `cargo compat validation sync` → regenerate inventories
   - `cargo compat validation promote <cmd>` → interactive tier migration (with test evidence)
   - `cargo compat validation report` → coverage metrics, test status matrix, flag breakdown

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                    COMPATIBILITY.MD (Human)                     │
│  (4-tier matrix, tier descriptions, links to per-tier docs)     │
└─────────────────────────────────────────────────────────────────┘
                              ↕ (sync via CI gate)
┌─────────────────────────────────────────────────────────────────┐
│        data/compatibility/ (Structured Validation State)         │
├───────────────────────────────────────────────────────────────────┤
│ commands/                  discovery/              reports/       │
│ ├─ push.toml              ├─ cli_commands.json    ├─ (on-demand) │
│ ├─ commit.toml            ├─ scenarios.json       └─ coverage*.json
│ └─ ... (auto-populated)   ├─ test_files_map.json                │
│                            └─ flag_inventory.json                │
└─────────────────────────────────────────────────────────────────┘
                 ↓ (validation inputs)
┌──────────────────────────────────────────────────────────────────┐
│        tools/compat-validation/ (Validation Toolkit)              │
├──────────────────────────────────────────────────────────────────┤
│ discovery.rs         validation.rs       sync.rs      reporting.rs
│ (extract surfaces)   (consistency checks) (update    (output
│ from code            (tier alignment,    inventories) formats)
│                      test coverage,
│                      documentation)
└──────────────────────────────────────────────────────────────────┘
                 ↕
┌──────────────────────────────────────────────────────────────────┐
│      src/cli.rs, src/command/, tests/command/, yaml               │
│              (Source of truth: code + tests)                      │
└──────────────────────────────────────────────────────────────────┘
```

### The Four Tiers (Enhanced)

| Tier | Meaning | Test Requirement | Documentation |
|------|---------|------------------|-----------------|
| **supported** | Matches Git semantics | 100% integration tests passing | Link to test IDs, design notes |
| **partial** | Incomplete; common paths work | N% integration tests passing (e.g., 50%); gaps documented | Blocked test IDs + timeline |
| **unsupported** | Not implemented | No test requirement (optional stub tests) | RFC link, rationale for deferral |
| **intentionally-different** | Deliberate divergence from Git | All tests passing; behavior difference proven | Security justification + threat model + test validation |

---

## Key Artifacts

### 1. Per-Command Status Files (`data/compatibility/commands/<cmd>.toml`)

```toml
tier = "partial"
last_reviewed = "2026-06-10"
reviewed_by = ["Claude Code"]

[coverage]
integration_tests_total = 18
integration_tests_passing = 16
integration_tests_blocked = 2
test_ids = ["cli.push-...", ...]
blocked_test_ids = ["live.push-gpg-sign-cert"]

[flags]
"--force-with-lease" = { tier = "supported", tests = [...], coverage_pct = 100 }
"--atomic" = { tier = "supported", tests = [...], coverage_pct = 95 }

[justifications]
[[intentional_differences]]
feature = "--force-if-includes"
why = "Atomic OID validation vs Git's heuristic"
link = "docs/improvement/compatibility/push.md"
```

**Owner**: Maintainer (for justifications); auto-populated (for metrics)
**Validation**: `cargo compat validation check --command push`

### 2. Auto-Generated Inventories (`data/compatibility/discovery/`)

**cli_commands.json**: All Commands variants + tiers from src/cli.rs
**integration_scenarios.json**: All scenarios from yaml
**test_files_map.json**: Commands → test files
**flag_inventory.json**: All flags per command

**Owner**: Automated (read-only)
**Generation**: `cargo compat validation sync`

### 3. Justification Documents (`docs/improvement/compatibility/`)

For each intentionally-different command/flag:
```markdown
# push --force-if-includes: Intentionally-Different Justification

## Summary
Libra rejects --force-if-includes; uses --force-with-lease instead.

## Threat Model
Git's heuristic is prone to false positives in rebased branches.

## Design Alternative
[considered and rejected options]

## Test Coverage
- cli.push-force-with-lease-mismatch: verify rejection
- cli.push-force-with-lease-success: verify acceptance

## User Migration
[guide for Git users]
```

**Owner**: Maintainer
**Validation**: CI confirms file exists for all intentionally-different tiers

### 4. Validation Toolkit (`tools/compat-validation/`)

Rust CLI with subcommands:
- `check` — Detect tier/test/doc drift
- `sync` — Regenerate discovery/*.json
- `promote` — Interactive tier migration
- `init-command` — Scaffold a new command
- `report` — Coverage metrics + dashboards

**Owner**: Rust maintainers
**Integration**: Used in CI gates + local development

---

## Validation Gates (CI Integration)

### Pre-Commit Hook (Local)
```bash
cargo compat validation check --command $(git diff --cached src/cli.rs | grep Commands | head -1)
# Fail if drift detected; guide developer to sync
```

### CI: `compat-validation.yml` (New)
```bash
cargo compat validation sync
git diff --exit-code data/compatibility/discovery/
# Fail if inventories drifted (developer forgot to sync)

cargo compat validation check --verbose
# Fail if tier/test/doc misalignment detected
```

### CI: `base.yml` (Updated)
```bash
cargo compat validation check
# Runs as part of main CI pipeline
```

---

## Workflow Examples

### Adding a Command (new)
1. Implement `src/command/push_server.rs` + register in cli.rs
2. Run `cargo compat validation init-command push_server` → creates `data/compatibility/commands/push_server.toml` (tier=unsupported)
3. Write tests, create integration scenarios
4. Run `cargo compat validation sync` → updates inventories
5. Run `cargo compat validation check` → verify alignment
6. Commit

### Promoting a Command (e.g., `push` partial → supported)
1. Complete missing flag implementations
2. Write missing integration tests (add to yaml + scenarios/)
3. Run `cargo compat validation promote push --from partial --to supported`
   - Interactive: prompts for test evidence, review notes, sign-off
   - Updates COMPATIBILITY.md + data/compatibility/commands/push.toml
   - Generates commit message
4. CI verifies: all tests passing, tier alignment correct, docs present
5. Merge

### Declaring Intentionally-Different (e.g., `push --force`)
1. Implement intentionally-different behavior + write tests that prove it differs from Git
2. Create `docs/improvement/compatibility/push-force.md` with:
   - What differs + why
   - Threat model / design alternative
   - Test coverage
   - User migration guide
3. Update `data/compatibility/commands/push.toml`:
   ```toml
   [[justifications.intentional_differences]]
   feature = "--force"
   why = "..."
   link = "docs/improvement/compatibility/push-force.md"
   ```
4. Update COMPATIBILITY.md entry (tier = "intentionally-different")
5. Run `cargo compat validation check` → CI verifies docs exist + tests pass
6. Merge

---

## Success Metrics

✅ **Zero drift on main**: COMPATIBILITY.md ↔ code ↔ tests always in sync (CI gate enforces)

✅ **Test coverage visible**: Every command/flag has coverage % in reports; operators see gaps at a glance

✅ **Tier promotions tracked**: Each tier change has evidence (test IDs, sign-off) recorded in TOML + git log

✅ **Intentionally-different justified**: All 4 items have signed-off security/design docs + test validation

✅ **New commands easy**: Boilerplate reduced; `init-command` scaffold + `promote` workflow guide most of the work

✅ **Extensible**: Framework applies same validation model to new commands as they're added (no special cases)

---

## Phased Implementation

### Phase 1: Foundation (2–3 weeks)
- Scaffold `tools/compat-validation/` Rust crate
- Implement `discovery.rs` (parse src/cli.rs, src/command/, yaml, test files)
- Implement `validation.rs` (tier alignment, test coverage, documentation checks)
- Implement `cargo compat validation check`
- Create `data/compatibility/` directory structure + sample TOMLs

### Phase 2: Automation (2 weeks)
- Implement `cargo compat validation sync`
- Add drift detection + reporting
- Implement `cargo compat validation promote` (interactive)
- Create GitHub Actions workflow (compat-validation.yml)
- Gate CI: compat-validation must pass

### Phase 3: Migration (1 week)
- Backfill `data/compatibility/commands/` for all 89 commands
- Review + cross-check COMPATIBILITY.md entries
- Create `docs/improvement/compatibility/framework.md` (operator guide)

### Phase 4: Reporting & Docs (1 week)
- Implement `cargo compat validation report` (JSON + markdown)
- Create per-command justification docs for intentionally-different items
- Add framework tests to `tests/compat/`

---

## Comparison to Grit's Model

**Grit's approach** (test-driven):
- Specify rule behavior declaratively (YAML)
- Pair with executable tests that prove correctness
- Auto-detect coverage gaps; gate promotion on test evidence
- Provide tooling to explore coverage space

**Libra's adaptation**:
- Specify command compatibility tier + flags (TOML)
- Pair with integration tests (integration-scenarios.yaml + tests/command/)
- Auto-detect coverage gaps; gate tier promotion on test evidence
- Provide `cargo compat validation` tooling to explore + validate

**Parallel structure**:
```
Grit                    Libra
──────────────────────  ──────────────────────
rule definition (YAML)  → command tier + flags (TOML)
rule tests              → integration scenarios + tests/command/
rule coverage report    → `cargo compat validation report`
test-driven promotion   → `cargo compat validation promote`
CI gate on drift        → `compat-validation.yml` CI job
```

---

## Risk Mitigation

**Risk**: Discovery parser (syn) breaks on new Rust features
- **Mitigation**: Fallback to grep-based flag discovery; document minimum syn version; test parser against Cargo's src/cli.rs

**Risk**: Integration scenario → command mapping ambiguous (e.g., `cli.add-chmod` could be add OR commit)
- **Mitigation**: Explicit `group` field in yaml (already present); validation asserts group matches command name

**Risk**: Backfilling 89 commands is tedious
- **Mitigation**: Template generation; batch script to auto-create TOMLs with placeholder coverage metrics; human review in one PR

**Risk**: CI gate is too strict (blocks legitimate work)
- **Mitigation**: `validation_options` in status.toml allow configuring thresholds; emergency `-X` flag to skip (with comment requirement)

**Risk**: Justification docs become outdated
- **Mitigation**: Link check in CI; `last_reviewed` date in TOML; remind via CI comment if > 3 months old

---

## Future Extensions

1. **Comparative VCS matrix**: Track tiers across Libra, Git, Jujutsu, Fossil, Pijul
2. **Tier stability scoring**: Measure churn; warn if a tier changes frequently
3. **Coverage trend analysis**: Chart coverage % over time (requires git history mining)
4. **Automated migration guides**: Generate user-facing `.md` from intentional-difference docs
5. **Flag interaction matrix**: Validate mutually exclusive flags (clap group) have test coverage
6. **Performance tier**: Track complexity (time/space) of commands relative to Git

---

## Reference Implementation Notes

### Key Modules in `tools/compat-validation/src/`

1. **discovery.rs** (250 lines)
   - ParseCommandsFromCliRs (syn)
   - ParseFlagsFromArgs (syn)
   - LoadIntegrationScenarios (serde_yaml)
   - ScanTestFiles (walkdir + grep)

2. **validation.rs** (200 lines)
   - ValidateTierAlignment
   - ValidateTestCoverage
   - ValidateDocumentation
   - GenerateDriftReport

3. **reporting.rs** (150 lines)
   - CoverageByTier (JSON, Markdown)
   - TestStatusMatrix
   - FlagCoverage
   - HistoricalTrends

4. **sync.rs** (100 lines)
   - RegenerateCLICommands
   - RegenerateScenarios
   - RegenerateTestMap
   - RegenerateFlagInventory

### CLI Entry Point (main.rs + cli.rs)
- Clap-based subcommand dispatch
- --verbose, --repo-root flags
- Subcommands: check, sync, promote, init-command, report, history

---

## Conclusion

The enhanced compatibility framework transforms Libra's tier system from **manual governance** into an **automated, evidence-based validation pipeline**. By adopting grit's test-driven validation model, Libra gains:

- **Drift detection**: CI automatically catches code ↔ test ↔ docs misalignment
- **Transparency**: Operators see exact coverage % and test blockers per command
- **Governance**: Tier promotions require documented evidence + review sign-off
- **Scalability**: Adding new commands follows a single, repeatable workflow
- **Assurance**: Intentionally-different behaviors are justified + tested, not ad-hoc

The four-tier system remains unchanged in spirit; the framework simply enforces rigor and provides tooling to make tier transitions smooth and auditable.
