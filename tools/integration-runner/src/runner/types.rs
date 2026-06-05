use std::path::PathBuf;

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct CommandRecord {
    pub(crate) seq: usize,
    pub(crate) command: Vec<String>,
    pub(crate) cwd: String,
    pub(crate) exit_code: Option<i32>,
    pub(crate) success: bool,
    pub(crate) stdout_log: String,
    pub(crate) stderr_log: String,
    pub(crate) stderr_tail: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ScenarioResult {
    pub(crate) id: String,
    pub(crate) wave: u8,
    pub(crate) status: String,
    pub(crate) duration_ms: u128,
    pub(crate) run_dir: String,
    pub(crate) commands: Vec<CommandRecord>,
    pub(crate) error: Option<String>,
    /// Wave 3 only: "deleted <owner/repo>" | "cleanup_required <owner/repo>" | null
    pub(crate) cleanup: Option<String>,
}

/// Aggregate status counts (mirrors integration-test-plan.md §5.5 `totals`).
///
/// The runner currently maps every non-pass/non-fail outcome to `skip`; the
/// `env_skip` / `block` slots from §5.1 are reserved here so report consumers
/// can rely on a stable shape as those statuses are differentiated (see
/// BASELINE_GAP-INTEG-010 for env-skip).
#[derive(Debug, Serialize)]
pub(crate) struct Totals {
    pub(crate) pass: usize,
    pub(crate) fail: usize,
    pub(crate) skip: usize,
    pub(crate) env_skip: usize,
    pub(crate) block: usize,
}

#[derive(Debug, Serialize)]
pub(crate) struct Report {
    pub(crate) generated_at: String,
    /// OS-ARCH the run executed on (e.g. `macos-aarch64`), per §5.5.
    pub(crate) platform: String,
    pub(crate) run_root: String,
    pub(crate) binary: String,
    /// `clean` whenever the report is produced: every command's raw stdout/stderr
    /// passes `ensure_no_secret_leak` before any (redacted) log is written, so a
    /// completed run with written artifacts is leak-clean by construction (§3.6).
    pub(crate) redaction_self_check: String,
    pub(crate) totals: Totals,
    pub(crate) passed: usize,
    pub(crate) failed: usize,
    pub(crate) skipped: usize,
    pub(crate) results: Vec<ScenarioResult>,
}

pub(crate) struct RunContext {
    pub(crate) run_root: PathBuf,
    pub(crate) binary: PathBuf,
    pub(crate) safe_path: String,
    pub(crate) results_path: PathBuf,
}

pub(crate) struct ScenarioCtx<'a> {
    pub(crate) run: &'a RunContext,
    pub(crate) id: String,
    pub(crate) wave: u8,
    pub(crate) run_dir: PathBuf,
    pub(crate) commands: Vec<CommandRecord>,
    pub(crate) seq: usize,
    /// Populated by live Wave 3 scenarios after explicit delete or on failure path.
    pub(crate) cleanup_status: Option<String>,
}
