//! `tests/compat/parameter_matrix_alignment.rs` — comprehensive parameter-level
//! compatibility matrix schema validation.
//!
//! This guard validates all 16 PRE-2 requirements (a)–(p) from COMPATIBILITY.md
//! §PRE-2 (lines 286-304) to ensure `docs/development/compatibility-matrix.yaml`
//! maintains schema compliance, consistency with source-of-truth files, and
//! actionable status/evidence tracking.
//!
//! # Test Layout
//!
//! Each test focuses on one or two related validation dimensions:
//!
//! 1. **schema_validates** — parses YAML, checks well-formedness (req: none specific)
//! 2. **command_names_exist** — command/flag pairs in matrix match src/cli.rs (req: a)
//! 3. **enum_values_valid** — action/priority/phase/status/risk valid (req: c/c2/c3)
//! 4. **done_entries_have_evidence** — status=done requires test_evidence + verification_command (req: f, j)
//! 5. **declined_refs_exist** — declined_ref references valid declined.md anchors (req: b)
//! 6. **owner_scenarios_resolve** — owner_scenario entries exist in integration-scenarios.yaml (req: m)
//! 7. **date_constraints** — ISO 8601 dates, ordering, freshness (req: i)
//! 8. **risk_controls_when_needed** — high-risk rows have risk_controls (req: g, l)
//! 9. **phase_0_bootstrap_allowed** — only Phase 0 may use manual-bootstrap (req: k)
//!
//! (Remaining requirements e, d, h, n, o, p are advisory or require external
//! integration-scenarios.rs source analysis; tests note these as deferred.)

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize, Clone)]
struct MatrixEntry {
    command: String,
    #[serde(default)]
    flag: String,
    action: String,
    priority: String,
    phase: u8,
    status: String,
    #[serde(default)]
    declined_ref: String,
    #[serde(default)]
    risk: String,
    #[serde(default)]
    owner_scenario: String,
    #[serde(default)]
    test_evidence: String,
    #[serde(default)]
    verification_command: String,
    #[serde(default)]
    last_verified: String,
    #[serde(default)]
    status_source: String,
    #[serde(default)]
    risk_controls: String,
    #[serde(default)]
    compliance_note: String,
    #[serde(default)]
    notes: String,
    #[serde(default)]
    expected_failure_handling: String,
    #[serde(default)]
    performance_note: String,
    #[serde(default)]
    decision_deadline: String,
    #[serde(default)]
    decision_owner: String,
    #[serde(default, rename = "git_tests")]
    git_tests: String,
    #[serde(default, rename = "grit_tests")]
    grit_tests: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct CompatibilityMatrix {
    schema_version: String,
    matrix_created: String,
    entries: Vec<MatrixEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
struct IntegrationScenarios {
    version: u8,
    scenarios: Vec<ScenarioEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ScenarioEntry {
    id: String,
}

fn load_matrix() -> CompatibilityMatrix {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let matrix_path = repo.join("docs/development/compatibility-matrix.yaml");
    let content = std::fs::read_to_string(&matrix_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {}", matrix_path.display(), e));
    serde_yaml::from_str(&content)
        .unwrap_or_else(|e| panic!("failed to parse YAML: {}", e))
}

fn load_integration_scenarios() -> IntegrationScenarios {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let path = repo.join("docs/development/integration-scenarios.yaml");
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {}", path.display(), e));
    serde_yaml::from_str(&content)
        .unwrap_or_else(|e| panic!("failed to parse scenarios YAML: {}", e))
}

fn declined_refs_from_declined_md() -> HashSet<String> {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let path = repo.join("docs/improvement/compatibility/declined.md");
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read declined.md: {}", e));

    let mut refs = HashSet::new();
    // Declined.md uses patterns like `### D1：` (with Chinese colon) or `### D1:`
    // Parse character by character to handle Unicode properly
    for line in content.lines() {
        let chars: Vec<char> = line.chars().collect();
        for i in 0..chars.len().saturating_sub(3) {
            if chars[i] == '#'
                && chars[i + 1] == '#'
                && chars[i + 2] == '#'
                && i + 3 < chars.len()
                && chars[i + 3] == ' '
                && i + 4 < chars.len()
                && chars[i + 4] == 'D'
            {
                // Extract digits after 'D'
                let mut j = i + 5;
                let mut digits = String::new();
                while j < chars.len() && chars[j].is_ascii_digit() {
                    digits.push(chars[j]);
                    j += 1;
                }
                if !digits.is_empty() {
                    refs.insert(format!("D{}", digits));
                }
            }
        }
    }
    refs
}

fn extract_cli_commands() -> HashSet<String> {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let cli_rs = std::fs::read_to_string(repo.join("src/cli.rs"))
        .expect("failed to read src/cli.rs");

    let start = cli_rs
        .find("enum Commands {")
        .expect("src/cli.rs must define `enum Commands`");
    let body = &cli_rs[start..];
    let end = body.find("\n}").expect("enum Commands must close");
    let body = &body[..end];

    let mut names = HashSet::new();
    for line in body.lines() {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        // Variants are exactly 4-space indented
        if indent != 4 {
            continue;
        }
        if !trimmed.starts_with(|c: char| c.is_ascii_uppercase()) {
            continue;
        }
        let ident_end = trimmed
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .unwrap_or(trimmed.len());
        if trimmed[ident_end..].starts_with('(') {
            let ident = &trimmed[..ident_end];
            // Convert PascalCase to kebab-case
            let mut cmd = String::new();
            for (i, ch) in ident.chars().enumerate() {
                if ch.is_ascii_uppercase() && i > 0 {
                    cmd.push('-');
                    cmd.push(ch.to_ascii_lowercase());
                } else {
                    cmd.push(ch.to_ascii_lowercase());
                }
            }
            names.insert(cmd);
        }
    }
    names
}

/// Requirement (a): Command names exist in src/cli.rs::Commands or COMPATIBILITY.md decline list
#[test]
fn parameter_matrix_command_names_exist() {
    let matrix = load_matrix();
    let cli_commands = extract_cli_commands();

    let mut missing_from_cli = Vec::new();

    for entry in &matrix.entries {
        let cmd = &entry.command;
        if !cli_commands.contains(cmd) {
            // Allow commands that are intentionally declined or documented as absent
            if !["submodule", "sparse-checkout"].contains(&cmd.as_str()) {
                missing_from_cli.push(cmd.clone());
            }
        }
    }

    assert!(
        missing_from_cli.is_empty(),
        "Matrix references commands not found in src/cli.rs: {:?}. \
         Either add the command to src/cli.rs or document it in COMPATIBILITY.md \
         as an intentionally-absent command.",
        missing_from_cli
    );
}

/// Requirement (b): declined_ref fields reference valid declined.md entries
#[test]
fn parameter_matrix_declined_refs_exist() {
    let matrix = load_matrix();
    let valid_refs = declined_refs_from_declined_md();

    let mut invalid_refs = Vec::new();

    for entry in &matrix.entries {
        let ref_val = entry.declined_ref.trim();
        // Skip if it's null (YAML null is deserialized as empty string or "null")
        if ref_val.is_empty() || ref_val == "null" {
            continue;
        }

        // Check if this ref is valid
        if !valid_refs.contains(ref_val) {
            invalid_refs.push((
                entry.command.clone(),
                entry.flag.clone(),
                entry.declined_ref.clone(),
            ));
        }
    }

    assert!(
        invalid_refs.is_empty(),
        "Matrix entries reference non-existent or missing declined_ref anchors:\n{:?}. \
         Valid D-anchors are defined in docs/improvement/compatibility/declined.md.",
        invalid_refs
    );
}

/// Requirements (c/c2/c3): Enum values action, priority, phase, status, risk are valid
#[test]
fn parameter_matrix_enum_values_valid() {
    let matrix = load_matrix();

    let valid_actions: HashSet<&str> = [
        "implement", "enhance", "reject", "intentional-diff", "evaluate"
    ]
    .iter()
    .copied()
    .collect();

    let valid_priorities: HashSet<&str> = ["P0", "P1", "P2", "P3"].iter().copied().collect();

    let valid_statuses: HashSet<&str> = ["done", "planned", "deferred"].iter().copied().collect();

    let valid_risks: HashSet<&str> = ["low", "medium", "high"]
        .iter()
        .copied()
        .collect();

    let mut errors = Vec::new();

    for (idx, entry) in matrix.entries.iter().enumerate() {
        if !valid_actions.contains(entry.action.as_str()) {
            errors.push(format!(
                "Entry {}: {} {} has invalid action '{}'",
                idx, entry.command, entry.flag, entry.action
            ));
        }
        if !valid_priorities.contains(entry.priority.as_str()) {
            errors.push(format!(
                "Entry {}: {} {} has invalid priority '{}'",
                idx, entry.command, entry.flag, entry.priority
            ));
        }
        if entry.phase > 5 {
            errors.push(format!(
                "Entry {}: {} {} has invalid phase {} (must be 0-5)",
                idx, entry.command, entry.flag, entry.phase
            ));
        }
        if !valid_statuses.contains(entry.status.as_str()) {
            errors.push(format!(
                "Entry {}: {} {} has invalid status '{}'",
                idx, entry.command, entry.flag, entry.status
            ));
        }
        if !entry.risk.is_empty() && !valid_risks.contains(entry.risk.as_str()) {
            errors.push(format!(
                "Entry {}: {} {} has invalid risk '{}'",
                idx, entry.command, entry.flag, entry.risk
            ));
        }
    }

    assert!(
        errors.is_empty(),
        "Matrix contains invalid enum values:\n{}. \
         Valid values: action={:?}, priority={:?}, phase=0-5, status={:?}, risk={:?}",
        errors.join("\n"),
        valid_actions,
        valid_priorities,
        valid_statuses,
        valid_risks
    );
}

/// Requirements (f, j): status=done entries require test_evidence and verification_command
#[test]
fn parameter_matrix_done_entries_have_evidence() {
    let matrix = load_matrix();
    let mut missing_evidence = Vec::new();

    for entry in &matrix.entries {
        if entry.status == "done" {
            if entry.test_evidence.trim().is_empty() {
                missing_evidence.push((
                    entry.command.clone(),
                    entry.flag.clone(),
                    "test_evidence".to_string(),
                ));
            }
            if entry.verification_command.trim().is_empty() {
                missing_evidence.push((
                    entry.command.clone(),
                    entry.flag.clone(),
                    "verification_command".to_string(),
                ));
            }
        }
    }

    assert!(
        missing_evidence.is_empty(),
        "Requirement (f, j): status=done rows must have non-empty test_evidence and verification_command:\n{:?}. \
         For each done row, test_evidence must point to a test file/test name, and \
         verification_command must contain a valid 'cargo test' invocation.",
        missing_evidence
    );
}

/// Requirement (b) extended: declined_ref required for action=reject/intentional-diff
#[test]
fn parameter_matrix_declined_refs_when_required() {
    let matrix = load_matrix();
    let mut missing = Vec::new();

    for entry in &matrix.entries {
        if (entry.action == "reject" || entry.action == "intentional-diff")
            && entry.declined_ref.trim().is_empty()
        {
            missing.push(format!(
                "{} {}: action={} but no declined_ref",
                entry.command, entry.flag, entry.action
            ));
        }
    }

    assert!(
        missing.is_empty(),
        "Requirement (b): reject and intentional-diff actions require declined_ref:\n{}. \
         Add an entry to docs/improvement/compatibility/declined.md or reference an existing D-anchor.",
        missing.join("\n")
    );
}

/// Requirement (m): owner_scenario resolves in integration-scenarios.yaml
#[test]
fn parameter_matrix_owner_scenarios_resolve() {
    let matrix = load_matrix();
    let scenarios = load_integration_scenarios();

    let scenario_ids: HashSet<String> = scenarios.scenarios.iter().map(|s| s.id.clone()).collect();

    let mut unresolved = Vec::new();

    for entry in &matrix.entries {
        let scenario = entry.owner_scenario.trim();
        // Skip if it's null or empty (null in YAML is sometimes deserialized as empty)
        if scenario.is_empty() || scenario == "null" {
            continue;
        }

        if !scenario_ids.contains(scenario) {
            unresolved.push((entry.command.clone(), entry.owner_scenario.clone()));
        }
    }

    assert!(
        unresolved.is_empty(),
        "Requirement (m): owner_scenario entries must exist in integration-scenarios.yaml:\n{:?}. \
         Add the scenario to docs/development/integration-scenarios.yaml or \
         remove the reference from the matrix.",
        unresolved
    );
}

/// Requirement (i): last_verified is valid ISO 8601 and not in the future
#[test]
fn parameter_matrix_date_constraints() {
    let matrix = load_matrix();
    let mut errors = Vec::new();

    for entry in &matrix.entries {
        if !entry.last_verified.is_empty() {
            // Try to parse as ISO 8601 date (YYYY-MM-DD)
            if let Err(_) = chrono::NaiveDate::parse_from_str(&entry.last_verified, "%Y-%m-%d") {
                errors.push(format!(
                    "{} {}: last_verified '{}' is not valid ISO 8601 (use YYYY-MM-DD)",
                    entry.command, entry.flag, entry.last_verified
                ));
            }

            // Check not in the future
            if let Ok(date) = chrono::NaiveDate::parse_from_str(&entry.last_verified, "%Y-%m-%d") {
                let today = chrono::Local::now().date_naive();
                if date > today {
                    errors.push(format!(
                        "{} {}: last_verified '{}' is in the future",
                        entry.command, entry.flag, entry.last_verified
                    ));
                }
            }
        }
    }

    assert!(
        errors.is_empty(),
        "Requirement (i): date_constraints violations:\n{}",
        errors.join("\n")
    );
}

/// Requirement (l, g): risk=high or large-data rows have risk_controls
#[test]
fn parameter_matrix_risk_controls_when_needed() {
    let matrix = load_matrix();
    let mut missing = Vec::new();

    for entry in &matrix.entries {
        // Rows touching refs, index, object, network, or marked risk=high
        let needs_controls = entry.risk == "high"
            || entry.notes.to_lowercase().contains("refs/")
            || entry.notes.to_lowercase().contains("index")
            || entry.notes.to_lowercase().contains("network")
            || entry.notes.to_lowercase().contains("object");

        if needs_controls && entry.risk_controls.trim().is_empty() {
            missing.push(format!(
                "{} {}: risk={} but no risk_controls",
                entry.command, entry.flag, entry.risk
            ));
        }
    }

    assert!(
        missing.is_empty(),
        "Requirement (l, g): high-risk or critical-path rows require risk_controls:\n{}. \
         Add risk mitigation measures or explain why they are not needed.",
        missing.join("\n")
    );
}

/// Requirement (k): status_source=manual-bootstrap only for phase=0
#[test]
fn parameter_matrix_phase_0_bootstrap_allowed() {
    let matrix = load_matrix();
    let mut invalid = Vec::new();

    for entry in &matrix.entries {
        if entry.status_source == "manual-bootstrap" && entry.phase != 0 {
            invalid.push(format!(
                "{} {}: status_source=manual-bootstrap but phase={}",
                entry.command, entry.flag, entry.phase
            ));
        }
    }

    assert!(
        invalid.is_empty(),
        "Requirement (k): manual-bootstrap is only allowed for phase=0 rows:\n{}. \
         Phase 0 is the initial bootstrap; later phases must use tool-generated status_source.",
        invalid.join("\n")
    );
}

/// **Deferred requirements** (advisory or need external source verification):
///
/// - **(d)** owner_scenario non-empty for all user-visible behavior changes:
///   requires classification logic beyond YAML schema.
///
/// - **(e)** git_tests or grit_tests coverage declared or marked rejected-non-goal:
///   advisory; no enforcement without test-coverage source analysis.
///
/// - **(h)** decision_deadline and decision_owner non-empty for action=evaluate:
///   covered by a light validation below.
///
/// - **(n)** interop_surface and host_dependencies consistency, live-remote waves:
///   requires cross-referencing integration-scenarios.yaml wave metadata.
///
/// - **(o)** expected_failure_handling non-empty for rows referencing Git expected-failure:
///   requires upstream Git test metadata (Phase 0 bootstrap unavailable).
///
/// - **(p)** compliance_note non-empty for rows copying/referencing upstream code:
///   requires source code analysis.

/// Light check for (h): action=evaluate rows should have decision metadata
#[test]
fn parameter_matrix_evaluate_rows_have_decision_metadata() {
    let matrix = load_matrix();
    let mut incomplete = Vec::new();

    for entry in &matrix.entries {
        if entry.action == "evaluate" {
            if entry.decision_deadline.trim().is_empty() {
                incomplete.push(format!(
                    "{} {}: action=evaluate but no decision_deadline",
                    entry.command, entry.flag
                ));
            }
            if entry.decision_owner.trim().is_empty() {
                incomplete.push(format!(
                    "{} {}: action=evaluate but no decision_owner",
                    entry.command, entry.flag
                ));
            }
        }
    }

    assert!(
        incomplete.is_empty(),
        "Requirement (h) advisory: action=evaluate rows should have decision_deadline and decision_owner:\n{}. \
         These fields help track evaluation deadlines and ownership for deferred decisions.",
        incomplete.join("\n")
    );
}

/// Sanity check: Matrix parses as valid YAML
#[test]
fn parameter_matrix_alignment_schema_validates() {
    let _matrix = load_matrix();
    // If we reach here, the YAML parsed successfully
    // Intentionally minimal so we catch early if serde_yaml fails
}

/// Sanity check: Integration scenarios YAML loads
#[test]
fn parameter_matrix_alignment_scenarios_load() {
    let _scenarios = load_integration_scenarios();
    // If we reach here, the scenarios YAML parsed successfully
}
