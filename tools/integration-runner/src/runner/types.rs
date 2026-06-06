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
    /// Wave 3 only: "deleted <owner/repo>" | "cleanup_required <owner/repo>" | null.
    /// "cleanup_required" is now surfaced by setting immediately after guard arm (in live scenario)
    /// so error bails after create carry it; explicit success overwrites to "deleted".
    /// (Guard Drop does best-effort but does not itself set ctx state.)
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
    // === Design-model fields per §5.5 (added additively for alignment; serialize-only,
    // backward-compat so pre-existing report consumers continue to parse).
    pub(crate) run_id: String,
    pub(crate) commit: String,
    pub(crate) started_at: String,
    pub(crate) finished_at: String,
    pub(crate) waves_run: Vec<u8>,
    pub(crate) wave3_cleanup: String,
    pub(crate) run_root_state: String,

    // Legacy/compat fields retained exactly (with original names and positions where
    // practical) so JSON shape for old consumers is a superset.
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
    /// Alias to the per-scenario records (design model in plan §5.5 uses "scenarios";
    /// "results" kept for compat). Both point to the same data.
    pub(crate) scenarios: Vec<ScenarioResult>,
}

pub(crate) struct RunContext {
    pub(crate) run_root: PathBuf,
    pub(crate) binary: PathBuf,
    pub(crate) safe_path: String,
    pub(crate) results_path: PathBuf,
    // Metadata for §5 report alignment (populated at run start via make_run_metadata in
    // normal/live + write_live_skip_report; used by write_report + derive helpers).
    // run_id/started_at per plan §3.3.1/§5 compact/RFC3339 pattern.
    // finished_at / wave3_cleanup / run_root_state are *derived at write time* inside
    // write_report (wave3_cleanup only from live results that may carry "deleted"/"cleanup_required";
    // run_root_state always "preserved" to keep report co-located with run_root per .keep()).
    pub(crate) run_id: String,
    pub(crate) commit: String,
    pub(crate) started_at: String,
    pub(crate) waves_run: Vec<u8>,
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
