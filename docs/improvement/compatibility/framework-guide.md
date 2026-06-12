# Libra Compatibility Framework Guide

**Welcome to Libra's test-driven compatibility validation system.**

This directory contains documentation for the enhanced compatibility framework, which governs how Libra's Git surface is classified and validated across 4 tiers (supported, partial, unsupported, intentionally-different).

---

## Quick Navigation

### For Operators & Maintainers

1. **[framework-implementation.md](framework-implementation.md)** — Step-by-step guide
   - How to use `cargo compat validation` commands
   - Adding a new command (workflow)
   - Promoting a command tier (interactive guide)
   - Understanding TOML status files
   - Integration with CI/CD

2. **[framework.md](framework.md)** — Detailed reference (todo: create)
   - All `cargo compat validation` subcommands
   - Directory structure + file roles
   - Validation rules per tier
   - Troubleshooting + FAQ

### For Architects & Reviewers

1. **[../../COMPATIBILITY-FRAMEWORK-DESIGN-SUMMARY.md](../../COMPATIBILITY-FRAMEWORK-DESIGN-SUMMARY.md)** — High-level design
   - Problem statement
   - Solution overview + architecture
   - Comparison to grit's model
   - Phased implementation roadmap
   - Risk mitigation

2. **[../../COMPATIBILITY-FRAMEWORK.md](../../COMPATIBILITY-FRAMEWORK.md)** — Full specification
   - 4-tier system + validation requirements
   - Directory structure + artifacts
   - Validation logic (pseudocode)
   - CI gates + automation
   - Data models + examples
   - Tier promotion lifecycle

### For Security Review

1. **[intentional-differences.md](intentional-differences.md)** — Consolidated justifications (todo: create)
   - All commands/flags with intentionally-different tier
   - Per-item: threat model, design alternative, trade-offs, user migration
   - Updated when intentional-differences are added

2. **[security-justification-template.md](security-justification-template.md)** — Template (todo: create)
   - Checklist: what must be included in a justification
   - Threat model framing
   - Test evidence requirements
   - Sign-off process

### For New Command Development

1. **[framework-implementation.md](framework-implementation.md#example-1-adding-a-new-command-push_server)** — "Adding a New Command" workflow
   - Step-by-step: implement → register → initialize → sync → check → merge
   - Boilerplate reduction via `cargo compat validation init-command`

2. **[writing-validation-tests.md](writing-validation-tests.md)** — Test patterns (todo: create)
   - Integration scenario structure
   - Assertion categories
   - Test naming conventions
   - Coverage for tier promotion

### For Declined Features

1. **[declined-features.md](declined-features.md)** — Consolidated rationale (todo: create)
   - All commands/flags with unsupported tier
   - Per-item: RFC link, rationale, timeline (if any)

---

## Conceptual Overview

### The 4 Tiers

```
Tier                    Test Requirement              Documentation
────────────────────────────────────────────────────────────────
supported               100% integration tests pass   Link to test IDs
partial                 N% integration tests pass     Blocked test IDs + timeline
unsupported             Optional (stub tests OK)      RFC link, rationale
intentionally-different All tests passing, proves     Security justification +
                        behavior difference           threat model + test validation
```

### The Validation Pipeline

```
┌─ Source Code ──────────────────────────────────────────────────┐
│  src/cli.rs (Commands enum)                                    │
│  src/command/<name>.rs (Args struct)                           │
│  tests/command/*_test.rs (integration tests)                  │
│  docs/development/integration-scenarios.yaml (scenarios)      │
└────────────────────────────────────────────────────────────────┘
                          ↓
┌─ Auto-Discovery (cargo compat validation sync) ────────────────┐
│  Extract commands, flags, scenarios, test mappings            │
│  Generate: data/compatibility/discovery/*.json                │
└────────────────────────────────────────────────────────────────┘
                          ↓
┌─ Per-Command Status ───────────────────────────────────────────┐
│  data/compatibility/commands/<name>.toml                       │
│  • Tier (supported/partial/unsupported/intentionally-different)│
│  • Coverage % (integration tests passing)                      │
│  • Blocked test IDs                                            │
│  • Per-flag status                                             │
│  • Justifications (intentional-differences)                    │
└────────────────────────────────────────────────────────────────┘
                          ↓
┌─ Validation Checks (cargo compat validation check) ─────────────┐
│  Tier alignment:        COMPATIBILITY.md ↔ code ↔ TOMLs         │
│  Test coverage:         mapped scenarios pass rate              │
│  Documentation:         justifications exist + current          │
│  Result:               Pass / Fail + drift report               │
└────────────────────────────────────────────────────────────────┘
                          ↓
┌─ CI Gate (compat-validation.yml) ──────────────────────────────┐
│  Enforce: zero drift on every PR + main branch                │
│  Block merges if drift detected                                │
└────────────────────────────────────────────────────────────────┘
```

### Workflows

#### 1. Adding a New Command
```bash
# 1. Implement
vim src/command/push_server.rs       # New command
vim src/cli.rs                       # Register in Commands enum

# 2. Initialize status file
cargo compat validation init-command push_server
# Creates: data/compatibility/commands/push_server.toml (tier=unsupported)

# 3. Write tests + scenarios
vim docs/development/integration-scenarios.yaml
vim tests/command/push_server_test.rs

# 4. Sync & validate
cargo compat validation sync
cargo compat validation check
# Verify: push_server appears in discovery/*.json, no drift

# 5. Commit
git add ...
git commit -m "feat: add push_server command (unsupported tier)"
```

#### 2. Promoting a Tier (partial → supported)
```bash
# 1. Assess readiness
cargo compat validation check --command push
# Output: 16/18 tests passing (88%), 2 blocked

# 2. Address gaps
vim src/command/push.rs               # Complete missing flags
vim docs/development/integration-scenarios.yaml  # Add scenarios
vim tests/command/push_*.rs           # Implement tests

# 3. Interactive promotion
cargo compat validation promote push --from partial --to supported
# Prompts:
#   - Enter test IDs (validates they exist in yaml)
#   - Confirm all tests passing
#   - Confirm no blocked tests remain
#   - Sign-off (name + email)

# 4. Validate
cargo compat validation check --command push
# Should now show: 18/18 passing (100%)

# 5. Merge
# CI verifies: tier alignment, tests passing, docs present
```

#### 3. Declaring Intentionally-Different
```bash
# 1. Implement + test the divergence
vim src/command/push.rs              # Implement divergence
vim tests/command/push_test.rs       # Write test proving difference from Git

# 2. Create justification doc
vim docs/improvement/compatibility/push-force-mitigation.md
# Sections:
#   - Summary
#   - Threat Model
#   - Design Alternative (considered/rejected)
#   - Trade-offs
#   - Test Coverage (which tests prove the difference)
#   - User Migration

# 3. Update status file
vim data/compatibility/commands/push.toml
# Add:
#   [[justifications.intentional_differences]]
#   feature = "--force"
#   why = "..."
#   link = "docs/improvement/compatibility/push-force-mitigation.md"

# 4. Update COMPATIBILITY.md
vim COMPATIBILITY.md
# Change tier from "partial" to "intentionally-different"
# Reference the justification doc

# 5. Validate
cargo compat validation check --command push
# Verifies: docs exist, tests pass, tier alignment correct

# 6. Merge
# CI gates: security review (via GH PR), signature/sign-off from maintainer
```

---

## Files Generated by the Framework

### Committed to Source Control

| Location | Role | Owner | How Often |
|----------|------|-------|-----------|
| COMPATIBILITY.md | 4-tier matrix (human-readable) | Maintainer | Per PR |
| data/compatibility/commands/*.toml | Per-command status + justifications | Maintainer | Per PR |
| docs/improvement/compatibility/*.md | Detailed justifications (intentional-diff, declined) | Maintainer | Per PR |
| tools/compat-validation/ | Validation toolkit (Rust) | Rust maintainers | Per feature/fix |

### Auto-Generated (Not Committed)

| Location | Role | Generated By | When |
|----------|------|--------------|------|
| data/compatibility/discovery/*.json | Inventories (commands, flags, scenarios, tests) | `cargo compat validation sync` | Before CI |
| data/compatibility/reports/*.json | Coverage reports, test status | `cargo compat validation report` | On-demand |

---

## Implementation Status

### Phase 1: Foundation ✅ (Completed in this Design)
- [x] Architecture + data models
- [x] Directory structure
- [x] Per-command status file template
- [x] Discovery inventory templates
- [ ] `tools/compat-validation/` Rust crate (Next)

### Phase 2: Automation ⏳ (Pending)
- [ ] `cargo compat validation check` subcommand
- [ ] `cargo compat validation sync` subcommand
- [ ] `cargo compat validation promote` subcommand
- [ ] `cargo compat validation report` subcommand
- [ ] GitHub Actions workflow (compat-validation.yml)

### Phase 3: Migration ⏳ (Pending)
- [ ] Backfill TOMLs for all 89 commands
- [ ] Review + validate existing COMPATIBILITY.md entries
- [ ] Create operator guide docs

### Phase 4: Reporting ⏳ (Pending)
- [ ] Coverage dashboards (HTML)
- [ ] Historical trend analysis
- [ ] Per-command justification docs (consolidated from COMPATIBILITY.md)

---

## Example: Validating the `push` Command

### Current State (COMPATIBILITY.md)
```markdown
| push | partial | ... [14 flags, some deferred, some intentional-different] ...
```

### Validation Check
```bash
$ cargo compat validation check --command push

✓ Discovered: 18 integration scenarios (cli.push-*)
✓ Discovered: 14 flags (--force, --force-with-lease, ...)
✓ Test files found: tests/command/push*.rs (3 files, 34 tests)
✓ Tier alignment: COMPATIBILITY.md="partial" ↔ push.toml="partial" ✓
✓ Coverage: 16/18 tests passing (88%) → meets partial threshold (50%) ✓
✓ Blocked tests documented: 2 entries in push.toml ✓
✓ Justifications present: 1 intentional-difference doc found ✓

Result: PASS
```

### Status File (`data/compatibility/commands/push.toml`)
```toml
tier = "partial"
last_reviewed = "2026-06-10"

[coverage]
integration_tests_total = 18
integration_tests_passing = 16
integration_tests_blocked = 2
test_ids = [
  "cli.push-basic", "cli.push-force-with-lease",
  "cli.push-atomic", "cli.push-follow-tags",
  # ... 14 more
]
blocked_test_ids = [
  "live.push-gpg-sign-cert",      # Needs vault + live keys
  "live.push-verify-signatures",   # Needs signed commits
]

[flags]
"--force" = { tier = "partial", coverage_pct = 40 }
"--force-with-lease" = { tier = "supported", coverage_pct = 100 }
"--atomic" = { tier = "supported", coverage_pct = 100 }
# ... others

[[justifications.intentional_differences]]
feature = "--force-if-includes"
why = "Libra's force-with-lease is stricter: atomic OID validation"
link = "docs/improvement/compatibility/push.md#force-if-includes"
```

---

## FAQ

**Q: What if my new test finds a bug in an existing command?**
A: Blocked tests are expected! Record the test ID in `blocked_test_ids` in the TOML, add a bug to the issue tracker, and unblock it when fixed. The framework makes these visible.

**Q: Can I manually edit the status TOMLs?**
A: Yes, but only the `[justifications]` section. Coverage metrics are auto-populated by discovery. Tier changes require the promotion workflow + sign-off.

**Q: How do I declare a command as "intentionally-different"?**
A: Create a justification doc in `docs/improvement/compatibility/<cmd>.md`, add entries to the TOML, update COMPATIBILITY.md tier, and run `cargo compat validation check`. CI will verify all pieces are in place.

**Q: What if integration scenarios don't map cleanly to commands?**
A: The `group` field in integration-scenarios.yaml is the source of truth. Use a 1:1 mapping; if a scenario covers multiple commands, split it into separate scenarios per the integration-test-plan.md.

**Q: Can I mark a test as "intentionally failing" to document a known gap?**
A: Yes—that's what `blocked_test_ids` are for. Document the reason (e.g., "security audit pending") and timeline in the TOML.

**Q: How often should I run `cargo compat validation sync`?**
A: Before committing any changes to src/cli.rs, src/command/, docs/development/integration-scenarios.yaml, or tests/command/. The CI gate will catch it if you forget.

---

## Getting Help

- **Questions about the framework?** → Read [COMPATIBILITY-FRAMEWORK.md](../../COMPATIBILITY-FRAMEWORK.md)
- **How do I run the toolkit?** → See [framework-implementation.md](framework-implementation.md#quick-start)
- **My command failed validation. Now what?** → Check the troubleshooting section (todo: add)
- **I want to propose a change to the framework.** → Open an issue with the `compatibility-framework` label
