# Compatibility Framework Implementation Guide

**For**: Libra developers, AI agents, maintainers
**Purpose**: Operationalize the enhanced compatibility framework (see `COMPATIBILITY-FRAMEWORK.md`)
**Status**: Phase 1 (Discovery & Validation Tooling)

---

## Quick Start

### For Reviewing Compatibility
```bash
# Check if COMPATIBILITY.md aligns with code + tests
cargo compat validation check --verbose

# See per-command coverage %
cargo compat validation report --format markdown
```

### For Promoting a Command Tier
```bash
# e.g., promoting `push` from partial → supported
cargo compat validation promote push --from partial --to supported
# Interactive: prompts for test IDs, review notes, sign-off
```

### For Adding a New Command
```bash
# Create initial status file + discover flags
cargo compat validation init-command <name>

# Later: sync inventories after implementing tests
cargo compat validation sync
```

---

## Directory Structure

The framework uses these directories:

```
libra/
├── data/compatibility/                 # Central validation state
│   ├── commands/                       # Per-command TOMLs
│   │   ├── init.toml                  # Tier, coverage %, flag breakdown
│   │   ├── push.toml
│   │   └── ...
│   │
│   ├── discovery/                      # Auto-generated inventories
│   │   ├── cli_commands.json           # Commands from src/cli.rs
│   │   ├── integration_scenarios.json  # Scenarios from yaml
│   │   ├── test_files_map.json         # Tests → commands
│   │   └── flag_inventory.json         # Flags per command
│   │
│   └── reports/                        # Generated on-demand
│       ├── coverage_by_tier.json
│       ├── test_status_matrix.json
│       └── flag_coverage.json
│
└── tools/compat-validation/            # Validation toolkit
    ├── Cargo.toml
    ├── src/
    │   ├── lib.rs                      # Public API
    │   ├── main.rs                     # CLI entry point
    │   ├── cli.rs                      # clap subcommands
    │   ├── model.rs                    # Data structures
    │   ├── discovery.rs                # Command/flag/test parsing
    │   ├── validation.rs               # Consistency checking
    │   ├── reporting.rs                # Report generation
    │   └── sync.rs                     # Inventory updates
    └── tests/
        └── validation_test.rs
```

---

## Core Components

### 1. `tools/compat-validation/src/model.rs`

Defines the data types used throughout the framework:

```rust
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Compatibility tier for a command or flag
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    Supported,
    Partial,
    Unsupported,
    #[serde(rename = "intentionally-different")]
    IntentionallyDifferent,
}

/// A Git command (e.g., "push", "commit")
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Command {
    pub name: String,
    pub tier: Tier,
    pub doc_section: String,     // From ROOT_AFTER_HELP in cli.rs
    pub hidden: bool,
    pub flags: Vec<Flag>,
    pub integration_test_count: usize,
    pub integration_test_passing: usize,
    pub integration_test_blocked: usize,
}

impl Command {
    pub fn coverage_pct(&self) -> f64 {
        if self.integration_test_count == 0 {
            0.0
        } else {
            (self.integration_test_passing as f64 / self.integration_test_count as f64) * 100.0
        }
    }
}

/// A command-line flag (e.g., "--force", "--message")
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Flag {
    pub name: String,
    pub short: Option<char>,
    pub long: Option<String>,
    pub takes_value: bool,
    pub repeatable: bool,
    pub tier: Tier,
    pub test_ids: Vec<String>,  // Integration scenario IDs covering this flag
}

/// Metadata about a single integration test scenario
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scenario {
    pub id: String,            // e.g., "cli.push-force-with-lease"
    pub group: String,         // e.g., "push"
    pub wave: u8,
    pub purpose: String,
    pub gh_required: bool,
    pub requires_git: bool,
    pub key_assertion_categories: Vec<String>,
}

/// Per-command validation state (loaded from data/compatibility/commands/<name>.toml)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandStatus {
    pub tier: Tier,
    pub last_reviewed: String,  // ISO 8601 date
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
    pub test_ids: Vec<String>,
    pub blocked_test_ids: Vec<String>,
    pub deferred_test_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlagStatus {
    pub tier: Tier,
    pub tests: Vec<String>,
    pub coverage_pct: f64,
    pub blocked: Option<String>,  // Reason if blocked
    pub timeline: Option<String>,  // e.g., "2026-Q3"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JustificationNotes {
    pub summary: Option<String>,
    pub intentional_differences: Vec<DifferenceNote>,
    pub partial_deferral: Vec<DeferralNote>,
    pub security_justification: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DifferenceNote {
    pub feature: String,           // e.g., "--force-if-includes"
    pub why: String,               // Why Libra diverges
    pub link: Option<String>,      // Docs link
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeferralNote {
    pub flag: String,
    pub reason: String,
    pub timeline: String,
}

/// Validation error / drift
#[derive(Debug, Clone, Serialize)]
pub struct Drift {
    pub command: String,
    pub drift_type: DriftType,
    pub severity: Severity,
    pub details: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DriftType {
    TierMismatch,
    MissingIntegrationTest,
    MissingJustification,
    UndocumentedFlag,
    BlockedTestNotDocumented,
    ScenarioMissing,
    TestFileNotFound,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Severity {
    Error,
    Warning,
}

/// Output of a validation check
#[derive(Debug, Serialize)]
pub struct ValidationReport {
    pub generated_at: String,      // ISO 8601
    pub drifts: Vec<Drift>,
    pub summary: ValidationSummary,
}

#[derive(Debug, Serialize)]
pub struct ValidationSummary {
    pub total_commands: usize,
    pub commands_with_drift: usize,
    pub error_count: usize,
    pub warning_count: usize,
    pub commands_checked: Vec<String>,
}
```

### 2. `tools/compat-validation/src/discovery.rs`

Extracts command, flag, scenario, and test information from the codebase:

```rust
use crate::model::*;
use anyhow::Result;
use std::path::Path;

pub struct DiscoveryEngine {
    repo_root: std::path::PathBuf,
}

impl DiscoveryEngine {
    pub fn new(repo_root: &Path) -> Self {
        Self {
            repo_root: repo_root.to_path_buf(),
        }
    }

    /// Discover all commands from src/cli.rs::Commands enum
    pub fn discover_commands(&self) -> Result<Vec<Command>> {
        // 1. Read src/cli.rs
        // 2. Parse with syn to find Commands enum
        // 3. For each variant, look up src/command/<name>.rs
        // 4. Extract clap Args struct and parse flags
        // 5. Build Command structs with placeholders for coverage (filled in later)
        todo!()
    }

    /// Discover all integration scenarios from docs/development/integration-scenarios.yaml
    pub fn discover_scenarios(&self) -> Result<Vec<Scenario>> {
        // 1. Load integration-scenarios.yaml
        // 2. Deserialize scenario list
        // 3. Return Vec<Scenario>
        todo!()
    }

    /// Map test files to commands by filename convention
    pub fn discover_test_files(&self) -> Result<HashMap<String, Vec<String>>> {
        // 1. Scan tests/command/ for *_test.rs files
        // 2. Extract command name from filename (convention: <cmd>_test.rs)
        // 3. Build map: command_name → [test file paths]
        // 4. Also record test function names (grep #[tokio::test])
        todo!()
    }

    /// Extract flag inventory from Args structs
    pub fn discover_flags(&self) -> Result<HashMap<String, Vec<Flag>>> {
        // 1. For each command, find src/command/<name>.rs
        // 2. Parse Args struct with syn
        // 3. Extract field names and clap attributes (#[arg(long = ...)])
        // 4. Build Flag structs; default tier = Unsupported (filled by TOML later)
        // 5. Return map: command_name → [flags]
        todo!()
    }

    /// Correlate scenarios to commands by group
    pub fn scenario_to_command_map(&self) -> Result<HashMap<String, Vec<String>>> {
        // scenarios grouped by "group" field → command names
        let scenarios = self.discover_scenarios()?;
        let mut map = HashMap::new();
        for scenario in scenarios {
            map.entry(scenario.group)
                .or_insert_with(Vec::new)
                .push(scenario.id);
        }
        Ok(map)
    }
}
```

### 3. `tools/compat-validation/src/validation.rs`

Consistency checking and drift detection:

```rust
use crate::model::*;
use anyhow::Result;
use std::collections::HashMap;

pub struct CompatValidator {
    commands: Vec<Command>,
    scenarios: Vec<Scenario>,
    test_files_map: HashMap<String, Vec<String>>,
    status_files: HashMap<String, CommandStatus>,  // Loaded from data/compatibility/commands/
}

impl CompatValidator {
    pub fn validate_tier_alignment(&self) -> Vec<Drift> {
        let mut drifts = Vec::new();

        for cmd in &self.commands {
            // Check: status file tier matches COMPATIBILITY.md tier
            if let Some(status) = self.status_files.get(&cmd.name) {
                if status.tier != cmd.tier {
                    drifts.push(Drift {
                        command: cmd.name.clone(),
                        drift_type: DriftType::TierMismatch,
                        severity: Severity::Error,
                        details: format!(
                            "COMPATIBILITY.md says {:?}, but data/compatibility/{}.toml says {:?}",
                            cmd.tier, cmd.name, status.tier
                        ),
                    });
                }
            }
        }

        drifts
    }

    pub fn validate_test_coverage(&self) -> Vec<Drift> {
        let mut drifts = Vec::new();

        for cmd in &self.commands {
            // For each command, check that integration scenarios exist
            let relevant_scenarios: Vec<_> = self
                .scenarios
                .iter()
                .filter(|s| s.group == cmd.name)
                .collect();

            // If tier is "supported" or "partial", must have scenarios
            match cmd.tier {
                Tier::Supported => {
                    if relevant_scenarios.is_empty() {
                        drifts.push(Drift {
                            command: cmd.name.clone(),
                            drift_type: DriftType::MissingIntegrationTest,
                            severity: Severity::Error,
                            details: "Tier is 'supported' but no integration scenarios found".into(),
                        });
                    }
                }
                Tier::Partial => {
                    if relevant_scenarios.is_empty() {
                        drifts.push(Drift {
                            command: cmd.name.clone(),
                            drift_type: DriftType::MissingIntegrationTest,
                            severity: Severity::Warning,
                            details: "Tier is 'partial' but no integration scenarios found".into(),
                        });
                    }
                }
                _ => {} // unsupported, intentionally-different: no requirement
            }
        }

        drifts
    }

    pub fn validate_documentation(&self) -> Vec<Drift> {
        let mut drifts = Vec::new();

        for cmd in &self.commands {
            if cmd.tier == Tier::IntentionallyDifferent {
                // Must have docs/improvement/compatibility/<cmd>.md
                let doc_path = format!("docs/improvement/compatibility/{}.md", cmd.name);
                if !std::path::Path::new(&doc_path).exists() {
                    drifts.push(Drift {
                        command: cmd.name.clone(),
                        drift_type: DriftType::MissingJustification,
                        severity: Severity::Error,
                        details: format!("Missing justification doc: {}", doc_path),
                    });
                }
            }
        }

        drifts
    }

    pub fn validate_all(&self) -> ValidationReport {
        let mut drifts = Vec::new();

        drifts.extend(self.validate_tier_alignment());
        drifts.extend(self.validate_test_coverage());
        drifts.extend(self.validate_documentation());

        let error_count = drifts.iter().filter(|d| d.severity == Severity::Error).count();
        let warning_count = drifts.iter().filter(|d| d.severity == Severity::Warning).count();

        ValidationReport {
            generated_at: chrono::Utc::now().to_rfc3339(),
            summary: ValidationSummary {
                total_commands: self.commands.len(),
                commands_with_drift: drifts.iter().map(|d| &d.command).collect::<std::collections::HashSet<_>>().len(),
                error_count,
                warning_count,
                commands_checked: self.commands.iter().map(|c| c.name.clone()).collect(),
            },
            drifts,
        }
    }
}
```

### 4. `tools/compat-validation/src/cli.rs` & `main.rs`

Command-line interface:

```rust
// src/cli.rs
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "compat-validation")]
#[command(about = "Libra compatibility framework toolkit")]
pub struct CompatValidationCli {
    #[command(subcommand)]
    pub command: CompatValidationCommand,

    /// Verbose output
    #[arg(global = true, short, long)]
    pub verbose: bool,

    /// Path to Libra repo root (auto-detected if in a Libra repo)
    #[arg(global = true, long)]
    pub repo_root: Option<std::path::PathBuf>,
}

#[derive(Subcommand)]
pub enum CompatValidationCommand {
    /// Check tier alignment: COMPATIBILITY.md ↔ code ↔ tests
    Check {
        /// Specific command to check (all if unset)
        #[arg(long)]
        command: Option<String>,

        /// Output format
        #[arg(long, value_parser = ["json", "markdown", "text"])]
        format: Option<String>,

        /// Write report to file (stdout if unset)
        #[arg(long, short)]
        output: Option<std::path::PathBuf>,
    },

    /// Sync: regenerate auto-discovered inventories
    Sync {
        /// Only regenerate this section (commands, scenarios, tests, flags)
        #[arg(long)]
        section: Option<String>,
    },

    /// Promote: move command to a new tier (interactive)
    Promote {
        /// Command name
        command: String,

        /// From tier
        #[arg(long)]
        from: String,

        /// To tier
        #[arg(long)]
        to: String,
    },

    /// Initialize: create initial status file for a new command
    InitCommand {
        /// Command name
        command: String,
    },

    /// Report: generate coverage/statistics report
    Report {
        /// Output format
        #[arg(long, value_parser = ["json", "markdown", "html"])]
        format: Option<String>,

        /// Write to file
        #[arg(long, short)]
        output: Option<std::path::PathBuf>,
    },

    /// History: audit tier changes
    History {
        /// Command name (all if unset)
        #[arg(long)]
        command: Option<String>,

        /// Since date (ISO 8601)
        #[arg(long)]
        since: Option<String>,
    },
}

// src/main.rs
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = CompatValidationCli::parse();

    let repo_root = cli
        .repo_root
        .or_else(|| find_libra_repo_root())
        .ok_or_else(|| anyhow::anyhow!("Not in a Libra repository"))?;

    match cli.command {
        CompatValidationCommand::Check { command, format, output } => {
            // 1. Load commands from src/cli.rs
            // 2. Load status files from data/compatibility/
            // 3. Run validation
            // 4. Format output
            // 5. Write to stdout or file
            todo!()
        }
        CompatValidationCommand::Sync { section } => {
            // Regenerate discovery/*.json files
            todo!()
        }
        CompatValidationCommand::Promote { command, from, to } => {
            // Interactive workflow: gather test IDs, sign-off, update files
            todo!()
        }
        // ... other commands
    }

    Ok(())
}
```

---

## Sample TOML Files

### `data/compatibility/commands/push.toml`

```toml
tier = "partial"
last_reviewed = "2026-06-10"
reviewed_by = ["Claude Code", "libra-maintainers"]

[coverage]
integration_tests_total = 18
integration_tests_passing = 16
integration_tests_blocked = 2
test_ids = [
  "cli.push-basic",
  "cli.push-multi-refspec",
  "cli.push-force-with-lease",
  "cli.push-atomic",
  "cli.push-follow-tags",
  # ... others
]
blocked_test_ids = [
  "live.push-gpg-sign-cert",  # Requires vault + live key
  "live.push-verify-signatures",  # Requires remote with signed commits
]
deferred_test_ids = []

[flags]
"--force" = { tier = "partial", tests = ["cli.push-basic"], coverage_pct = 40, blocked = "security audit pending" }
"--force-with-lease" = { tier = "supported", tests = ["cli.push-force-with-lease"], coverage_pct = 100 }
"--atomic" = { tier = "supported", tests = ["cli.push-atomic"], coverage_pct = 95 }
"--all" = { tier = "supported", tests = ["cli.push-all"], coverage_pct = 100 }
"--tags" = { tier = "supported", tests = ["cli.push-tags"], coverage_pct = 100 }
"--follow-tags" = { tier = "supported", tests = ["cli.push-follow-tags"], coverage_pct = 100 }

[justifications]
[[justifications.intentional_differences]]
feature = "--force-if-includes"
why = "Libra's force-with-lease is stricter: atomic tracking-ref validation vs Git's heuristic commit-ancestry check"
link = "docs/improvement/compatibility/push.md#force-if-includes"

[[justifications.partial_deferral]]
flag = "--force"
reason = "Implementation complete, test coverage incomplete pending security audit"
timeline = "2026-Q3"

[git_compatibility_notes]
summary = "Core push with tracking ref lease, atomic updates, cert signing"
```

### `data/compatibility/commands/init.toml`

```toml
tier = "supported"
last_reviewed = "2026-05-15"
reviewed_by = ["Claude Code"]

[coverage]
integration_tests_total = 12
integration_tests_passing = 12
integration_tests_blocked = 0
test_ids = [
  "cli.init-basic",
  "cli.init-directory-and-quiet",
  "cli.init-object-format",
  "cli.init-shared-mode",
  "cli.init-safe-reinit",
  # ... others
]
blocked_test_ids = []
deferred_test_ids = []

[flags]
"--quiet" = { tier = "supported", tests = ["cli.init-quiet"], coverage_pct = 100 }
"--object-format" = { tier = "supported", tests = ["cli.init-object-format"], coverage_pct = 100 }
"--shared" = { tier = "supported", tests = ["cli.init-shared-mode"], coverage_pct = 100 }

[git_compatibility_notes]
summary = "Full init with vault-backed signing, shared mode, object-format selection"
```

---

## Workflow Examples

### Example 1: Checking Drift on Main

```bash
$ cd libra/
$ cargo compat validation check --verbose

✓ 89 commands discovered
✓ 142 integration scenarios loaded
✓ 67 test files scanned
✓ 412 flags inventoried

Validating alignment...
✓ All 89 commands have tier entries in COMPATIBILITY.md
✓ All 42 "supported" commands have passing integration tests
✓ All 4 "intentionally-different" commands have justifications
✓ No drift detected

Report: cargo compat validation report --format markdown
```

### Example 2: Adding a New Command (`push-server`)

```bash
# 1. Implement the command
$ cat > src/command/push_server.rs << 'EOF'
pub struct Args { /* ... */ }
pub async fn execute(args: Args) -> CliResult<()> { /* ... */ }
EOF

# 2. Register in src/cli.rs
# (add PushServer variant to Commands enum)

# 3. Initialize status file
$ cargo compat validation init-command push-server
# Creates: data/compatibility/commands/push_server.toml with tier=unsupported

# 4. Sync inventories
$ cargo compat validation sync
# Updates discovery/*.json to include push_server

# 5. Check drift (expect: push_server tier is unsupported)
$ cargo compat validation check --command push-server
✓ push_server: tier unsupported (no test requirement)
```

### Example 3: Promoting `push` from `partial` → `supported`

```bash
$ cargo compat validation promote push --from partial --to supported

Interactive prompt:
  1. Enter test IDs that validate all flags (will validate they exist in yaml):
     > cli.push-force-with-lease cli.push-atomic cli.push-follow-tags ...
  2. Are all integration tests passing? (y/n):
     > y
  3. Any remaining blocked tests? (y/n):
     > n
  4. Sign-off (your name/email):
     > Claude Code <claude@anthropic.com>

✓ Tier updated: push partial → supported
✓ Updated: COMPATIBILITY.md, data/compatibility/commands/push.toml
✓ Git diff:
  - COMPATIBILITY.md: | push | supported | ...
  - data/compatibility/commands/push.toml: tier = "supported" (from "partial")

Ready to commit? (y/n)
> y

Commit message:
  feat(compat): promote push command to supported tier
  
  - All 18 integration test scenarios passing
  - No blocked tests remain
  - All flags have documented coverage
  
  Co-Authored-By: Claude Code <claude@anthropic.com>
```

### Example 4: Declaring an Intentionally-Different Tier

```bash
# After implementing push with intentionally-different --force behavior

# 1. Create justification doc
$ cat > docs/improvement/compatibility/push-force.md << 'EOF'
# push --force: Intentionally-Different Justification

## Summary
Libra requires `--force-with-lease` instead of bare `--force`.

## Threat Model
Bare `--force` allows clobbering concurrent pushes from other developers
without validation. Libra rejects bare `--force` to prevent accidental data loss.

## Design Alternative
Could add a whitelist of "trusted" remotes, but that's too stateful.
Could add a confirmation prompt, but that breaks CI pipelines.

## Test Coverage
- cli.push-force-rejected: Verify `libra push --force` exits 128
- cli.push-force-with-lease-works: Verify `--force-with-lease` succeeds

## User Migration
```sh
# Old Git workflow:
git push --force

# New Libra workflow:
libra push --force-with-lease
# or rely on CI automation: libra agent enable push-safety
```
EOF

# 2. Update status file
$ cat >> data/compatibility/commands/push.toml << 'EOF'

[[justifications.intentional_differences]]
feature = "--force"
why = "Libra rejects bare --force to prevent accidental overwrite of concurrent pushes"
link = "docs/improvement/compatibility/push-force.md"
EOF

# 3. Update COMPATIBILITY.md
$ vim COMPATIBILITY.md
# Change: | push | partial | ... (add intentional-different note)

# 4. Validate
$ cargo compat validation check --command push
✓ push: tier partial → intentionally-different migration (valid)
✓ Justification doc found: docs/improvement/compatibility/push-force.md
```

---

## Testing the Framework Itself

Unit tests for the framework live in `tools/compat-validation/tests/`:

```rust
#[test]
fn test_command_discovery() {
    let engine = DiscoveryEngine::new(Path::new("."));
    let commands = engine.discover_commands().unwrap();
    assert!(commands.iter().any(|c| c.name == "push"));
    assert!(commands.iter().any(|c| c.name == "commit"));
}

#[test]
fn test_scenario_discovery() {
    let engine = DiscoveryEngine::new(Path::new("."));
    let scenarios = engine.discover_scenarios().unwrap();
    assert!(scenarios.iter().any(|s| s.id == "cli.push-basic"));
}

#[test]
fn test_drift_detection_tier_mismatch() {
    // Create a mock Command with tier=supported
    // Load a CommandStatus with tier=partial
    // Verify validator detects the drift
}

#[test]
fn test_validation_report_generation() {
    // Mock a full validation run
    // Verify report JSON structure
}
```

---

## Integration with CI

### GitHub Actions: `compat-validation.yml`

```yaml
name: Compatibility Validation
on: [pull_request, push]

jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2

      - name: Sync inventories
        run: cargo compat validation sync --verbose

      - name: Check drift
        id: check
        run: cargo compat validation check --format json --output /tmp/drift.json
        continue-on-error: true

      - name: Comment on PR
        if: github.event_name == 'pull_request' && steps.check.outcome == 'failure'
        uses: actions/github-script@v6
        with:
          script: |
            const fs = require('fs');
            const drift = JSON.parse(fs.readFileSync('/tmp/drift.json'));
            const comment = `## Compatibility Drift Detected\n\n${
              drift.drifts.map(d => `- ${d.command}: ${d.details}`).join('\n')
            }`;
            github.rest.issues.createComment({
              issue_number: context.issue.number,
              owner: context.repo.owner,
              repo: context.repo.repo,
              body: comment
            });

      - name: Fail on error drifts
        if: steps.check.outcome == 'failure'
        run: |
          cat /tmp/drift.json | jq '.summary'
          exit 128
```

---

## Handoff Notes

- **Phase 1 deliverables**: `tools/compat-validation/`, basic `discovery.rs` + `validation.rs`, `cargo compat validation check` CLI
- **Phase 2**: Full sync, report generation, promote workflow
- **Phase 3**: Backfill data/compatibility/ for all commands, migrate existing COMPATIBILITY.md entries
- **Phase 4**: Rich reporting, HTML dashboard, per-command docs

Each phase has detailed Cargo test coverage to ensure correctness as components are integrated.
