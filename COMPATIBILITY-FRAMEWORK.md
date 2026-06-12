# Libra Enhanced Compatibility Framework

**Design Document** — Integrates grit-style test-driven validation into Libra's 4-tier compatibility system.

---

## Executive Summary

This framework elevates Libra's compatibility governance from a human-editable matrix (`COMPATIBILITY.md`) into an **automated, evidence-based system** that:

1. **Ties each tier to measurable test coverage**: `supported` requires 100% integration test pass rate; `partial` explicitly documents coverage %; `unsupported`/`intentionally-different` have justified documentation
2. **Discovers and validates commands/flags dynamically**: Extract command surfaces from `src/cli.rs`, correlate with `tests/command/` and `docs/development/integration-scenarios.yaml` coverage, detect drift
3. **Enforces consistency via CI gates**: Pre-commit tooling catches COMPATIBILITY.md edits, test file additions, and integration scenario entries that don't align
4. **Structures the validation audit trail**: Per-command status TOMLs in `data/compatibility/` allow incremental coverage tracking and third-party tools to reason about compatibility state
5. **Provides rich reporting**: Compatibility dashboards showing command/flag coverage metrics, test → command mapping, and historical tier changes

---

## Core Architecture

### 1. Four-Tier System (Enhanced)

Each tier now carries **validation requirements** that must be satisfied before promotion or modification:

| Tier | Definition | Validation Requirement | Documentation |
|------|-----------|------------------------|-----------------|
| **supported** | Command/flag behavior matches stock Git or is functionally equivalent | Must pass 100% of mapped integration tests (0 blocked/skipped) | Link to passing test file(s), security justification if divergent |
| **partial** | Command is exposed but incomplete (missing flags/subcommands) | Must pass N% of mapped tests (explicit threshold); all gaps documented with justification for deferral or intentional omission | Per-flag breakdown, blocked test identifiers, timeline for completion |
| **unsupported** | Not implemented, no public plumbing | No test requirement; may have stub tests for negative cases | RFC/design document link, rationale for non-implementation |
| **intentionally-different** | Behavior deliberately diverges from Git | Must have security/design justification + test cases demonstrating the difference and why it's better/safer | Link to security analysis, threat model, alternative-considered memo, and test validation |

---

## Directory Structure & Artifacts

```
libra/
├── COMPATIBILITY.md                                # Human-readable matrix (remains as-is, auto-validated)
├── COMPATIBILITY-FRAMEWORK.md                      # This document
│
├── data/compatibility/                             # Per-command validation state (NEW)
│   ├── status.toml                                # Aggregate stats: tier distribution, coverage %, last-sync date
│   ├── commands/
│   │   ├── add.toml                               # Command-level: tier, flags, test IDs, coverage %
│   │   ├── commit.toml
│   │   ├── push.toml
│   │   └── ...
│   │
│   ├── discovery/
│   │   ├── cli_commands.json                      # Auto-generated: all Commands variants from src/cli.rs
│   │   ├── integration_scenarios.json             # Auto-generated: all cli.* scenario IDs from yaml
│   │   ├── test_files_map.json                    # Auto-generated: test file → command correlation
│   │   └── flag_inventory.json                    # Auto-generated: per-command flags discovered from Args structs
│   │
│   ├── reports/
│   │   ├── coverage_by_tier.json                  # Metrics: supported/partial/unsupported/intentionally-different breakdown
│   │   ├── test_status_matrix.json                # Which tests pass/fail/skip/blocked per command
│   │   ├── flag_coverage.json                     # Per-command, per-flag test coverage %
│   │   └── drift_report.json                      # Code ↔ test ↔ COMPATIBILITY.md reconciliation gaps
│   │
│   └── justifications/
│       ├── intentional-differences.md             # Consolidated intentionally-different rationales
│       ├── declined-features.md                   # Reasons for `unsupported` (RFC links)
│       └── partial-deferral.md                    # Blocked/deferred flag timelines

├── tools/compat-validation/                       # New validation toolkit (Rust + CLI drivers)
│   ├── Cargo.toml
│   ├── src/
│   │   ├── lib.rs
│   │   ├── main.rs                                # CLI: check, sync, report subcommands
│   │   ├── discovery.rs                           # Parse src/cli.rs, tests/, yaml
│   │   ├── validation.rs                          # Test coverage queries, drift detection
│   │   ├── reporting.rs                           # JSON + markdown output formatters
│   │   ├── sync.rs                                # Reconcile COMPATIBILITY.md ↔ code
│   │   └── model.rs                               # Shared types (Command, Tier, Coverage)
│   │
│   └── tests/
│       └── ...
│
├── docs/improvement/compatibility/
│   ├── framework.md                               # Operator guide: using the framework, adding commands
│   ├── tier-promotion-guide.md                    # Workflow: moving supported → partial → intentionally-different
│   ├── writing-validation-tests.md                # Test patterns for gated tier changes
│   └── security-justification-template.md         # Template for intentionally-different items
│
├── .github/workflows/
│   ├── compat-validation.yml                      # NEW: runs check-plan + drift detection, gates on main
│   └── base.yml                                   # UPDATED: includes new compat checks
│
└── tests/compat/
    └── compat_compatibility_framework.rs          # Register test matrix alignment (extended)
```

---

## 2. Enhanced Tier Validation System

### 2.1 Command Promotion Lifecycle

A command transitions through tiers as tests are added and features mature:

```
[idea]
  ↓ (write design doc)
[unsupported]  ← Must exist in src/cli.rs with CLI docs; intentionally-different items need justification
  ↓ (implement + write integration tests for common paths)
[partial]      ← N% of tests pass; blocking gaps documented; COMPATIBILITY.md lists deferred flags
  ↓ (complete missing flags, add edge-case tests)
[supported]    ← 100% of integration tests pass; no blocked/skipped tests
  ↓ (diverge intentionally with security analysis)
[intentionally-different] ← All tests pass; security justification in place; migration guide for users
```

Each state change requires:
1. **Code evidence**: implementation in `src/command/<name>.rs` + test file(s)
2. **Test evidence**: passing or blocked test IDs in `docs/development/integration-scenarios/` + `tests/command/`
3. **Documentation evidence**: updated COMPATIBILITY.md entry + per-tier justification doc
4. **Drift check**: `cargo compat validation check` confirms alignment

### 2.2 Per-Command Validation Rules

Each command has a TOML status file (e.g., `data/compatibility/commands/push.toml`):

```toml
# data/compatibility/commands/push.toml
tier = "partial"
last_reviewed = "2026-06-12"
reviewed_by = ["Claude Code", "libra-core"]

[coverage]
integration_tests_total = 18
integration_tests_passing = 16
integration_tests_blocked = 2
coverage_percentage = 89
test_ids = ["cli.push-force-with-lease", "cli.push-atomic", ...]
blocked_test_ids = ["live.push-gpg-sign-cert"]  # Needs vault setup
deferred_test_ids = []

[flags]
# Per-flag coverage (auto-discovered from src/cli.rs)
"--force-with-lease" = { tier = "supported", tests = ["cli.push-force-with-lease"], coverage_pct = 100 }
"--atomic" = { tier = "supported", tests = ["cli.push-atomic"], coverage_pct = 95 }
"--force" = { tier = "partial", tests = ["cli.push-force"], coverage_pct = 50, blocked = "needs security audit" }
"--follow-tags" = { tier = "supported", tests = ["cli.push-follow-tags"], coverage_pct = 100 }

[justifications]
# Why any flags are partial/deferred/intentionally-different
partial_deferred = [
  { flag = "--force", reason = "implementation complete but test coverage incomplete", timeline = "2026-Q3" }
]

[git_compatibility_notes]
# Linked from COMPATIBILITY.md § push
# Example: https://github.com/libra-org/libra/docs/development/compatibility.md#push
summary = "Core push with tracking ref lease, atomic updates, cert signing"
intentional_differences = [
  { feature = "--force-if-includes no-op", why = "lease uses tracking-ref OID only; self-contained pack", link = "docs/improvement/compatibility/push.md#force-if-includes" }
]
```

### 2.3 Auto-Generated Inventories

The validation framework discovers and reconciles these inventories:

#### `data/compatibility/discovery/cli_commands.json`
Auto-extracted from `src/cli.rs::Commands` enum:

```json
{
  "commands": [
    {
      "name": "init",
      "tier_in_code": "supported",  // From COMPATIBILITY.md
      "doc_section": "Repository Setup",
      "hidden": false,
      "tier_listed_in_compat_md": "supported",
      "drift": false
    },
    {
      "name": "code",
      "tier_in_code": "intentionally-different",
      "doc_section": "AI And Automation",
      "hidden": false,
      "tier_listed_in_compat_md": "intentionally-different",
      "drift": false
    }
  ],
  "last_sync": "2026-06-12T14:30:00Z"
}
```

#### `data/compatibility/discovery/integration_scenarios.json`
Extracted from `docs/development/integration-scenarios.yaml`:

```json
{
  "scenarios": [
    {
      "id": "cli.push-force-with-lease",
      "group": "push",
      "wave": 2,
      "purpose": "Verify --force-with-lease validates tracking-ref OID before sending",
      "key_assertion_categories": ["intentional_difference", "negative_exit"],
      "requires_git": false,
      "gh_required": false
    },
    {
      "id": "cli.push-atomic",
      "group": "push",
      "wave": 2,
      "purpose": "Verify --atomic requires remote `atomic` capability and updates refs transactionally",
      "key_assertion_categories": ["json_envelope", "fsck"],
      "requires_git": false,
      "gh_required": false
    }
  ]
}
```

#### `data/compatibility/discovery/test_files_map.json`
Correlates test files to commands:

```json
{
  "command_test_mapping": [
    {
      "command": "push",
      "test_files": [
        "tests/command/push_test.rs",
        "tests/command/push_error_test.rs",
        "tests/command/push_lease_test.rs"
      ],
      "integration_scenarios": [
        "cli.push-force-with-lease",
        "cli.push-atomic",
        "cli.push-follow-tags",
        "cli.push-gpg-sign-cert"
      ],
      "unit_test_count": 34,
      "integration_test_count": 4
    },
    {
      "command": "add",
      "test_files": [
        "tests/command/add_test.rs"
      ],
      "integration_scenarios": [
        "cli.add-basic",
        "cli.add-chmod",
        "cli.add-pathspec"
      ],
      "unit_test_count": 18,
      "integration_test_count": 3
    }
  ]
}
```

#### `data/compatibility/discovery/flag_inventory.json`
Extracted from `src/command/<name>::Args` structs via `syn` parser:

```json
{
  "push": {
    "flags": [
      { "name": "--force", "short": "-f", "takes_value": false, "repeatable": false },
      { "name": "--force-with-lease", "short": null, "takes_value": true, "repeatable": false, "value_type": "ref_or_expect" },
      { "name": "--atomic", "short": null, "takes_value": false, "repeatable": false },
      { "name": "--all", "short": "-a", "takes_value": false, "repeatable": false },
      { "name": "--tags", "short": "-t", "takes_value": false, "repeatable": false },
      { "name": "--follow-tags", "short": null, "takes_value": false, "repeatable": false }
    ]
  },
  "commit": {
    "flags": [
      { "name": "--message", "short": "-m", "takes_value": true, "repeatable": true, "value_type": "string" },
      { "name": "--edit", "short": "-e", "takes_value": false, "repeatable": false }
    ]
  }
}
```

---

## 3. Automated Validation (Rust Toolkit)

### 3.1 CLI Entry Point: `cargo compat validation`

```bash
# Check alignment: COMPATIBILITY.md ↔ code ↔ tests
cargo compat validation check [--verbose] [--command <name>]
# Output: pass/fail + drift report (JSON/table)
# Exit code: 0 if all aligned, 128 if drift detected

# Sync inventories from source
cargo compat validation sync
# Auto-generates data/compatibility/discovery/*.json

# Generate reports
cargo compat validation report [--format json|markdown|html]
# Outputs: coverage_by_tier.json, test_status_matrix.json, flag_coverage.json

# Interactive mode for manual tier updates
cargo compat validation promote <command> [from_tier] [to_tier]
# Prompts for test evidence, updates COMPATIBILITY.md + data/compatibility/

# Audit trail
cargo compat validation history --command <name> --since <date>
# Shows tier transitions, test additions, documentation changes
```

### 3.2 Validation Logic (pseudocode)

```rust
// tools/compat-validation/src/lib.rs

pub struct CompatValidator {
    cli_commands: HashMap<String, Command>,           // Extracted from src/cli.rs
    integration_scenarios: Vec<Scenario>,             // From yaml
    test_files_map: HashMap<String, Vec<String>>,     // Command → test files
    compatibility_matrix: HashMap<String, Tier>,      // From COMPATIBILITY.md
    command_status_files: HashMap<String, CommandStatus>, // From data/compatibility/
}

impl CompatValidator {
    // 1. Discover phase: extract surfaces
    pub fn discover_commands() -> Result<Vec<Command>> {
        // Parse src/cli.rs with syn, extract Commands enum variants
        // For each variant, read src/command/<name>.rs and extract Args struct
        // Return Vec<Command> with name, tier_in_code, flags, etc.
    }

    pub fn discover_integration_scenarios() -> Result<Vec<Scenario>> {
        // Load docs/development/integration-scenarios.yaml
        // Extract id, group, wave, assertions
    }

    pub fn discover_test_files() -> Result<HashMap<String, Vec<String>>> {
        // Find all tests/command/*_test.rs
        // Grep for #[tokio::test], test_name patterns
        // Correlate with command names (test name contains command name)
    }

    pub fn discover_flags() -> Result<HashMap<String, Vec<Flag>>> {
        // For each command in CLI, locate src/command/<name>.rs
        // Parse the Args struct with syn::ItemStruct
        // Extract field names + clap attributes (#[arg(long = ...)])
    }

    // 2. Validation phase: check consistency
    pub fn validate_tier_alignment(&self) -> Vec<Drift> {
        // For each command:
        //   1. Assert: tier in COMPATIBILITY.md == tier in data/compatibility/commands/<name>.toml
        //   2. Assert: tier in COMPATIBILITY.md matches src/cli.rs docs comment
        //   3. If tier == "supported": assert integration_test_count > 0 && all pass
        //   4. If tier == "partial": assert coverage_pct defined in .toml
        //   5. If tier == "intentionally-different": assert justification.md linked
        // Collect and return all mismatches
    }

    pub fn validate_test_coverage(&self) -> Vec<TestGap> {
        // For each command with tier "supported" or "partial":
        //   - Find integration_scenarios matching group == command_name
        //   - Count how many scenarios have test files in tests/command/
        //   - Warn if integration_scenarios.yaml lists a scenario but no test exists
        //   - Return coverage metrics + gaps
    }

    pub fn validate_flags_coverage(&self) -> Vec<FlagGap> {
        // For each command, each flag:
        //   - Check if flag is mentioned in integration scenario .md files
        //   - Verify test_ids exist in tests/command/
        //   - Return per-flag coverage %, missing test IDs
    }

    pub fn validate_documentation(&self) -> Vec<DocGap> {
        // For each "intentionally-different" command:
        //   - Verify docs/improvement/compatibility/<cmd>.md exists
        //   - Check that COMPATIBILITY.md entry links to it
        //   - Assert justification.md file present
        // For each "partial" command:
        //   - Verify blocked_test_ids are documented in .toml
        //   - Check timeline entries are present
    }

    pub fn generate_drift_report(&self) -> DriftReport {
        // Collect all validation errors into structured report
        // Include: command mismatches, missing tests, undocumented flags, etc.
    }
}

pub struct DriftReport {
    pub tier_misalignments: Vec<(String, Drift)>,  // Command → what drifted
    pub test_gaps: Vec<TestGap>,                   // Commands missing integration tests
    pub flag_gaps: Vec<FlagGap>,                   // Flags without test coverage
    pub doc_gaps: Vec<DocGap>,                     // Missing justifications
    pub error_count: usize,
    pub warning_count: usize,
}
```

### 3.3 Data Model

```rust
// tools/compat-validation/src/model.rs

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Tier {
    Supported,
    Partial,
    Unsupported,
    IntentionallyDifferent,
}

#[derive(Debug, Clone, Serialize)]
pub struct Command {
    pub name: String,
    pub tier: Tier,
    pub doc_section: String,
    pub hidden: bool,
    pub flags: Vec<Flag>,
    pub test_coverage_pct: f64,
    pub integration_test_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Flag {
    pub name: String,
    pub short: Option<char>,
    pub takes_value: bool,
    pub repeatable: bool,
    pub tier: Tier,
    pub test_ids: Vec<String>,
    pub coverage_pct: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandStatus {
    pub tier: Tier,
    pub last_reviewed: String,
    pub reviewed_by: Vec<String>,
    pub coverage: CoverageMetrics,
    pub flags: HashMap<String, FlagStatus>,
    pub justifications: JustificationNotes,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverageMetrics {
    pub integration_tests_total: usize,
    pub integration_tests_passing: usize,
    pub integration_tests_blocked: usize,
    pub coverage_percentage: f64,
    pub test_ids: Vec<String>,
    pub blocked_test_ids: Vec<String>,
    pub deferred_test_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlagStatus {
    pub tier: Tier,
    pub tests: Vec<String>,
    pub coverage_pct: f64,
    pub blocked: Option<String>,
}

#[derive(Debug)]
pub enum Drift {
    TierMismatch { expected: Tier, found: Tier },
    MissingIntegrationTest { scenario_id: String },
    MissingJustification { tier: Tier },
    UndocumentedFlag { flag: String },
    BlockedTestNotDocumented { test_id: String },
}
```

---

## 4. CI Integration

### 4.1 New GitHub Workflow: `compat-validation.yml`

```yaml
name: Compatibility Framework Validation
on: [pull_request, push_to_main]

jobs:
  compat-check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0  # Full history for drift detection

      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable

      - name: Cache
        uses: Swatinem/rust-cache@v2

      - name: Sync inventories
        run: cargo compat validation sync --verbose

      - name: Check tier alignment
        run: cargo compat validation check --format json --output /tmp/drift.json
        continue-on-error: true

      - name: Fail on drift
        if: failure()
        run: |
          echo "Compatibility drift detected:"
          cat /tmp/drift.json | jq '.drifts[]'
          exit 128

      - name: Report coverage
        run: cargo compat validation report --format markdown > /tmp/coverage.md
        if: always()

      - name: Comment PR
        if: github.event_name == 'pull_request'
        uses: actions/github-script@v6
        with:
          script: |
            const fs = require('fs');
            const report = fs.readFileSync('/tmp/coverage.md', 'utf8');
            github.rest.issues.createComment({
              issue_number: context.issue.number,
              owner: context.repo.owner,
              repo: context.repo.repo,
              body: report
            });

      - name: Upload artifacts
        uses: actions/upload-artifact@v3
        if: always()
        with:
          name: compat-reports
          path: |
            /tmp/drift.json
            /tmp/coverage.md
            data/compatibility/reports/
```

### 4.2 Updated `base.yml` Gate

```yaml
# .github/workflows/base.yml
compat-validation:
  runs-on: [self-hosted]
  steps:
    - uses: actions/checkout@v4
    - run: cargo compat validation check --verbose
    - run: cargo compat validation sync && git diff --exit-code data/compatibility/discovery/
      # Fail if inventories drifted (developer forgot to sync)
```

---

## 5. Documentation Structure

### 5.1 Operator Guide: `docs/improvement/compatibility/framework.md`

Covers:
- How to **read** COMPATIBILITY.md + data/compatibility/ artifacts
- How to **add a new command**: template workflow
- How to **promote a command**: unsupported → partial → supported
- How to **declare intentionally-different**: security justification template
- Troubleshooting: drift detection, test mapping, flag discovery

### 5.2 Per-Tier Justification Docs

#### `docs/improvement/compatibility/intentional-differences.md`
For each intentionally-different command/flag:
- What differs from Git
- Why the difference is necessary (security/design)
- Trade-offs
- Migration guide for users

Example excerpt:
```markdown
## push --force-if-includes

**Difference**: `--force-if-includes` is a no-op in Libra; `--force-with-lease` is the
primary safety mechanism.

**Why**: Git's `--force-if-includes` checks that **commits** being pushed are ancestors
of the remote-tracking branch — a heuristic prone to false positives in rebased branches.
Libra's `--force-with-lease` is stricter: it validates that the **remote's HEAD** still
matches the OID recorded in the local tracking ref, which is atomic and deterministic.

**Trade-offs**:
- Pro: Eliminates accidental clobber of concurrent pushes to shared branches.
- Con: Adds an implicit round-trip to fetch the tracking ref before push (users must
  run `git fetch` / `libra fetch` first to validate the tracking ref). Mitigated by
  `push.checkRemote` (write-side automation hook).

**User migration**:
```sh
# Git users accustomed to:
git push --force-if-includes

# Should use in Libra:
libra push --force-with-lease
# Or rely on pre-push automation: https://docs.libra.sh/agent/push-safety
```
```

#### `docs/improvement/compatibility/declined-features.md`
For each `unsupported` feature with a design doc or RFC:
- Feature name + Git description
- Link to RFC or design document
- Rationale (product boundary, complexity, deferral timeline)

Example:
```markdown
## clone --sparse + sparse-checkout command

**RFC**: docs/improvement/compatibility/declined.md#d10 (linked from COMPATIBILITY.md)

**Rationale**: Sparse checkouts require a separate state machine (`sparse-checkout` command)
and `.git/info/sparse-checkout` config; Libra defers this to focus on core VCS stability.
Timeline: Q4 2026 (if product direction confirms priority).
```

---

## 6. Test Patterns for Tier Validation

### 6.1 Integration Test Anatomy

Each integration test in `docs/development/integration-scenarios/<id>.md` carries **assertion categories** (from the yaml) that tie directly to tier validation:

```markdown
## cli.push-force-with-lease

**Purpose**: Verify `--force-with-lease` validates tracking-ref OID before sending.

**Wave**: 2 (local only, no network)
**Requires**: Not required

### Assertions
- `intentional_difference`: Confirm that `--force-with-lease=<expect>` format matches Libra's semantics (not Git's)
- `negative_exit`: Verify rejection when tracking-ref OID doesn't match remote
- `json_envelope`: Confirm `--json --dry-run` outputs structured push result with `reason: "force-with-lease-mismatch"`

### Steps
1. Init a repo, commit a file, push to a remote
2. Fetch (populate tracking-ref)
3. Rebase the local branch
4. Run `libra push --force-with-lease=<original-oid>` → expect exit 128 (mismatch)
5. Verify JSON output includes `reason: "force-with-lease-mismatch"`
6. Run `libra push --force-with-lease` (no OID suffix) → should proceed
7. Verify tracking-ref updated post-push
```

The assertions directly validate the **intentionally-different tier status** by confirming the behavior diverges from Git intentionally.

### 6.2 Tier Promotion Testing

When promoting `push` from `partial` to `supported`:

```rust
// tests/compat/compat_push_promotion.rs (Rust test, not integration scenario)

#[test]
fn push_tier_promotion_requires_100_percent_pass_rate() {
    // Verify all integration scenarios with group="push" pass
    let scenarios = load_scenarios_for_group("push");
    assert!(scenarios.iter().all(|s| s.status == "passing"));
    
    // Verify no "blocked" scenarios remain
    let blocked = scenarios.iter().filter(|s| s.blocked).count();
    assert_eq!(blocked, 0);
    
    // Verify no "deferred" scenarios
    let deferred = scenarios.iter().filter(|s| s.deferred).count();
    assert_eq!(deferred, 0);
}

#[test]
fn push_flags_have_test_coverage() {
    let flags = discover_flags_for_command("push");
    let scenarios = load_scenarios_for_group("push");
    
    for flag in &flags {
        let coverage = scenarios
            .iter()
            .filter(|s| s.description.contains(&flag.name))
            .count();
        assert!(coverage > 0, "Flag {} has no test coverage", flag.name);
    }
}

#[test]
fn push_no_intentional_differences_without_justification() {
    let tier = load_tier_for_command("push");
    if tier == "intentionally-different" {
        // Fail if no justification docs found
        let doc = std::fs::read_to_string("docs/improvement/compatibility/push.md");
        assert!(doc.is_ok(), "Missing justification for intentionally-different push");
    }
}
```

---

## 7. Maintenance Workflow

### 7.1 Adding a New Command

1. **Implement**: Write `src/command/<name>.rs` + `Args` struct
2. **Register**: Add `Name` variant to `src/cli.rs::Commands` enum
3. **Initialize**: Run `cargo compat validation init-command <name>`
   - Creates `data/compatibility/commands/<name>.toml` with tier = "unsupported"
   - Discovers flags automatically
4. **Sync**: Run `cargo compat validation sync`
   - Updates `data/compatibility/discovery/*.json`
5. **Test**: Decide on tier; add test files + integration scenarios
   - If `supported`: write tests for all paths in `tests/command/<name>_test.rs`
   - If `partial`: list deferred flags + tests in `.toml`; create integration scenarios for implemented paths
   - If `unsupported`: write docs explaining why; optionally add negative/stub test cases
6. **Document**: Add COMPATIBILITY.md entry
7. **Validate**: Run `cargo compat validation check`
   - Must pass before merge (CI gate)

### 7.2 Promoting a Command Tier

From `partial` → `supported`:

1. **Pre-flight check**:
   ```bash
   cargo compat validation check --command push
   # Reports: X blocking issues, Y deferred tests, coverage Z%
   ```

2. **Address gaps**:
   - Complete implementation of deferred flags
   - Write missing integration tests (add to yaml + scenarios/)
   - Move tests from `blocked_test_ids` → `test_ids` in TOML

3. **Update COMPATIBILITY.md**: Change tier, remove deferral notes

4. **Sync & validate**:
   ```bash
   cargo compat validation sync
   cargo compat validation promote push partial supported
   # Prompts for: test evidence, security review, sign-off
   # Generates commit message + updates COMPATIBILITY.md + .toml
   ```

5. **CI gates**: All tests must pass; drift check must show no mismatches

---

## 8. Reporting & Dashboards

### 8.1 Coverage Reports

```bash
cargo compat validation report --format json --output report.json
```

Output structure:
```json
{
  "generated_at": "2026-06-12T14:30:00Z",
  "summary": {
    "total_commands": 89,
    "supported_count": 42,
    "partial_count": 31,
    "unsupported_count": 12,
    "intentionally_different_count": 4
  },
  "tier_distribution": {
    "supported": 47.2,
    "partial": 34.8,
    "unsupported": 13.5,
    "intentionally-different": 4.5
  },
  "coverage_metrics": {
    "avg_integration_test_coverage_pct": 78.3,
    "commands_with_100_pct_coverage": 42,
    "commands_with_0_pct_coverage": 4,
    "test_count_total": 312,
    "test_count_passing": 289,
    "test_count_blocked": 12,
    "test_count_deferred": 11
  },
  "commands": [
    {
      "name": "push",
      "tier": "partial",
      "integration_test_coverage_pct": 89,
      "integration_test_count": 18,
      "integration_test_passing": 16,
      "integration_test_blocked": 2,
      "flags": {
        "total": 14,
        "supported": 10,
        "partial": 3,
        "deferred": 1
      },
      "last_reviewed": "2026-06-10",
      "gaps": [
        {
          "flag": "--force",
          "reason": "security audit pending",
          "timeline": "2026-Q3"
        }
      ]
    }
  ],
  "drift": []
}
```

### 8.2 HTML Dashboard (Generated from above)

Visual breakdown:
- Tier distribution pie chart
- Coverage trend graph (over time, if git history available)
- Per-command flag coverage heatmap
- Test status matrix (pass/fail/blocked per scenario)
- Drift alerts (code ↔ tests ↔ docs misalignment)

---

## 9. Security & Justification Templates

### 9.1 Intentionally-Different Justification Template

For tier == "intentionally-different", require:

```markdown
# <Command> --<flag>: Intentionally-Different Justification

## Summary
Brief statement of divergence from Git.

## Threat Model
What vulnerability or design flaw in Git's approach does Libra fix?
- Example: Git's `--force-if-includes` is a heuristic that checks commit ancestry;
  Libra's atomic tracking-ref validation eliminates false-positive overwrites.

## Design Alternative
What other approaches were considered and rejected?
- Example: TOML config file lock (rejected: too stateful, incompatible with concurrent pushes)
- Example: Operator approval flow (rejected: breaks CI pipelines; users demand atomic decisions)

## Trade-offs
Costs of the intentional difference:
- Requires users to fetch tracking-ref first (one extra round-trip)
- Incompatible with Git workflows that assume `--force-if-includes` behavior

## Test Coverage
Which integration tests validate the intentional difference?
- cli.push-force-with-lease (negative case: mismatch rejected)
- cli.push-force-with-lease-explicit (positive case: matches)

## User Migration
Guidance for Git users switching to Libra.
```

### 9.2 Security Review Gate

Before promoting to intentionally-different, require:

1. **CISO review** (in org context) or **explicit security@ approval**
2. **Threat model document** (linked in justification)
3. **Test case** demonstrating the security benefit
4. **Deprecation notice** if replacing a Git behavior (in migration guide)

---

## 10. Implementation Roadmap

### Phase 1: Foundation (2–3 weeks)
- [ ] Create `tools/compat-validation/` scaffolding (Rust + CLI)
- [ ] Implement command/flag discovery from `src/cli.rs` + `src/command/` (syn parser)
- [ ] Implement integration scenario loading from yaml
- [ ] Create base model types + validation logic (tier alignment, test mapping)
- [ ] Add `cargo compat validation check` subcommand
- [ ] Create `data/compatibility/commands/` template + 5 test commands

### Phase 2: Automation (2 weeks)
- [ ] Implement `cargo compat validation sync` (auto-generate discovery/*.json)
- [ ] Add drift detection + reporting
- [ ] Implement `cargo compat validation promote` (interactive tier updates)
- [ ] Create GitHub Actions workflow (compat-validation.yml)
- [ ] Gate CI: `compat-validation` must pass on all PRs

### Phase 3: Migration (1 week)
- [ ] Backfill `data/compatibility/commands/` for all 89 commands
- [ ] Review existing COMPATIBILITY.md entries + cross-check
- [ ] Create `docs/improvement/compatibility/framework.md` (operator guide)
- [ ] Create `docs/improvement/compatibility/intentional-differences.md` (consolidated justifications)

### Phase 4: Reporting & Docs (1 week)
- [ ] Implement `cargo compat validation report` (JSON + markdown)
- [ ] Create HTML dashboard template
- [ ] Write per-command justification docs for intentionally-different items
- [ ] Add compat-framework tests to `tests/compat/`

---

## 11. Example: Validating `push` Tier

### Current State (COMPATIBILITY.md)
```markdown
| push | partial | ... many flags ...
```

### Data Artifacts
```toml
# data/compatibility/commands/push.toml
tier = "partial"
coverage_percentage = 89
integration_tests_passing = 16
integration_tests_blocked = 2
blocked_test_ids = ["live.push-gpg-sign-cert"]
```

### Validation Check
```bash
$ cargo compat validation check --command push
✓ Tier "partial" → coverage 89% OK (threshold: 50%)
✓ Integration scenario count: 18 found in yaml
✓ Test files found: tests/command/push*.rs (4 files)
✓ Flag inventory: 14 flags discovered in Args struct
✓ All flags have test entries in integration-scenarios/
✓ COMPATIBILITY.md entry matches code
✓ 2 blocked tests documented in .toml
✗ Drift: cli.push-gpg-sign-cert scenario exists but live.push-gpg-sign-cert not in integration_scenarios.yaml
```

### Fixing Drift
```bash
$ cargo compat validation sync
# Regenerates data/compatibility/discovery/*.json with corrected scenario list

$ cargo compat validation check --command push
# Now passes
```

---

## 12. Edge Cases & Future Extensions

### 12.1 Flags with Complex Interactions

Some flags are mutually exclusive (e.g., `--ff` / `--no-ff` / `--ff-only` in merge).
The framework should:
- Discover these from clap's `group` attributes
- Track coverage per **combination** (e.g., test that `--ff --no-ff` errors)
- Report coverage % per combination, not individual flags

### 12.2 Hidden/Internal Commands

Commands with `#[command(hidden = true)]` should:
- Still appear in validation (internal commands like `hooks`, `db` are Libra-only but public within agents)
- Be marked in discovery/*.json with `hidden: true`
- Include intentional-difference justifications (explaining why hidden from `--help`)

### 12.3 Subcommand Hierarchies

Commands with subcommands (e.g., `push <subcommand>`):
- Discover as separate "virtual" commands
- Example: `push::force-with-lease` instead of `push --force-with-lease`
- Use hierarchical TOML keys for clarity

### 12.4 Future: Multi-Repo Compatibility Matrix

Extend framework to compare Libra's tiers with other VCS (Jujutsu, Fossil, Pijul):
```json
{
  "command": "push",
  "tier": { "libra": "partial", "git": "supported", "jujutsu": "partial", "fossil": "unsupported" },
  "comparative_notes": "Libra's force-with-lease is stricter than Git's force-if-includes"
}
```

---

## 13. Success Metrics

- [ ] **100% of commands in src/cli.rs reflected in data/compatibility/discovery/**
- [ ] **Zero drift on every CI run**: COMPATIBILITY.md ↔ code ↔ tests align
- [ ] **Tier promotion workflow documented**: ≤5 PR comments per tier change (guidance clear)
- [ ] **Test coverage % visible in reporting**: operators can see at a glance which commands need more tests
- [ ] **Intentionally-different items have signed-off justifications**: each one references security review + test validation

---

## References

- **Current**: `COMPATIBILITY.md`, `docs/development/integration-scenarios.yaml`, `tests/compat/`
- **New**: `data/compatibility/`, `tools/compat-validation/`, `docs/improvement/compatibility/framework.md`
- **Inspiration**: grit's test-driven validation model, Cargo feature matrix gates, rustc's tier system
