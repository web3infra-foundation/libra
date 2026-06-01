//! TUI agent-run pane projection (CEX-S2-16, Step 2.6).
//!
//! A **pure** projection of [`AgentRun`] snapshots into stable display rows for
//! the `/agents` pane. Ordering is deterministic so a pane rebuilt from a JSONL
//! replay renders identically (CEX-S2-16 验收 (5)): in-flight runs (`Queued` /
//! `Running` / `Blocked`) sort before terminal ones (`Completed` / `Failed`),
//! ties broken by run id. No I/O occurs here.
//!
//! # Scope
//!
//! The full pane the card describes also shows live per-run telemetry — current
//! tool / file, elapsed, token usage, budget remaining, cost estimate, source
//! calls, context-pack hash and permission profile. Most of that is still joined
//! by later slices; this slice joins the **persisted per-run [`RunUsage`]** (the
//! dispatcher writes one on each terminal run) so the pane can show total tokens
//! and cost estimate alongside the snapshot fields. The join itself is the
//! caller's job (via [`agent_pane_rows_with_usage`] /
//! [`format_agent_run_pane_with_usage`]) — this module stays pure (no I/O).

use crate::internal::ai::agent_run::{AgentRun, AgentRunId, AgentRunStatus, AgentTaskId, RunUsage};

/// One row of the agent-run pane, projected from an [`AgentRun`] snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentRunRow {
    /// The run's identifier.
    pub agent_run_id: AgentRunId,
    /// The task this run executes.
    pub task_id: AgentTaskId,
    /// Current lifecycle status.
    pub status: AgentRunStatus,
    /// Provider slug (e.g. `"deepseek"`).
    pub provider: String,
    /// Model id within the provider.
    pub model: String,
    /// On-disk JSONL transcript path for the run.
    pub transcript_path: String,
    /// Isolated workspace path, if one has been materialized.
    pub workspace_path: Option<String>,
    /// Persisted per-run usage (total tokens, cost estimate, tool calls), joined
    /// from the run's `RunUsage` record when available. `None` when the run has
    /// no terminal usage record yet (e.g. still in flight) or the join is skipped.
    pub usage: Option<RunUsage>,
}

impl AgentRunRow {
    /// Project a single [`AgentRun`] snapshot into a display row (1:1 field map).
    /// `usage` is left unset; use [`agent_pane_rows_with_usage`] to join it.
    fn from_run(run: &AgentRun) -> Self {
        Self {
            agent_run_id: run.id,
            task_id: run.task_id,
            status: run.status,
            provider: run.provider.clone(),
            model: run.model.clone(),
            transcript_path: run.transcript_path.clone(),
            workspace_path: run.workspace_path.clone(),
            usage: None,
        }
    }
}

/// Project a slice of [`AgentRun`] snapshots into ordered pane rows.
///
/// In-flight runs sort before terminal ones; ties break by run id so the order
/// is deterministic and stable across replays. Pure — no I/O.
pub fn agent_pane_rows(runs: &[AgentRun]) -> Vec<AgentRunRow> {
    let mut rows: Vec<AgentRunRow> = runs.iter().map(AgentRunRow::from_run).collect();
    // `is_terminal()` is `false` for in-flight runs and `false < true`, so
    // in-flight rows sort first; the run id breaks ties deterministically.
    rows.sort_by_key(|row| (row.status.is_terminal(), row.agent_run_id.0));
    rows
}

/// Like [`agent_pane_rows`], but joins each run's persisted [`RunUsage`] via the
/// caller-supplied `usage_lookup` (e.g. `AgentRunEventStore::read_run_usage`).
/// Ordering is identical and deterministic. Pure — the I/O lives in the closure,
/// so a JSONL-replay caller can pass the same lookup and rebuild an identical
/// pane (CEX-S2-16 验收 (5)).
pub fn agent_pane_rows_with_usage<F>(runs: &[AgentRun], usage_lookup: F) -> Vec<AgentRunRow>
where
    F: Fn(&AgentRun) -> Option<RunUsage>,
{
    let mut rows: Vec<AgentRunRow> = runs
        .iter()
        .map(|run| {
            let mut row = AgentRunRow::from_run(run);
            row.usage = usage_lookup(run);
            row
        })
        .collect();
    rows.sort_by_key(|row| (row.status.is_terminal(), row.agent_run_id.0));
    rows
}

/// Render the agent-run pane as a stable monospace table for the `/agents` TUI
/// surface (and any future `libra agent status` CLI). Rows are ordered by
/// [`agent_pane_rows_with_usage`] — in-flight runs first, then by id — so a pane
/// rebuilt from persisted snapshots renders deterministically. An empty input
/// yields a documented placeholder so the surface never shows a blank pane.
///
/// Each run's persisted [`RunUsage`] is joined via `usage_lookup` (e.g.
/// `AgentRunEventStore::read_run_usage`) so the `tokens` / `cost` columns show
/// real totals; a run with no usage record renders `-` in those cells. Pass
/// `|_| None` to render the table without any usage join. Pure — the I/O lives
/// in the closure.
pub fn format_agent_run_pane_with_usage<F>(runs: &[AgentRun], usage_lookup: F) -> String
where
    F: Fn(&AgentRun) -> Option<RunUsage>,
{
    render_pane(&agent_pane_rows_with_usage(runs, usage_lookup))
}

/// Shared table renderer over already-ordered rows, factored out of the public
/// entry point so the layout has a single source of truth.
fn render_pane(rows: &[AgentRunRow]) -> String {
    if rows.is_empty() {
        return "No sub-agent runs recorded yet.".to_string();
    }

    let mut out = String::from("Agent runs:\n");
    let header = format!(
        "  {:<36} {:<10} {:<12} {:<24} {:>9} {:>10} {:>10}",
        "run", "status", "provider", "model", "elapsed", "tokens", "cost"
    );
    out.push_str(&header);
    out.push('\n');
    out.push_str("  ");
    out.push_str(&"-".repeat(header.len() - 2));
    out.push('\n');
    for row in rows {
        // `elapsed` / `tokens` / `cost` all derive from the joined per-run
        // `RunUsage`; a run with no usage record renders `-` in all three.
        let (elapsed, tokens, cost) = match row.usage {
            Some(usage) => (
                format_elapsed_ms(usage.wall_clock_ms),
                usage.total_tokens().to_string(),
                format_micro_dollars(usage.cost_estimate_micro_dollars),
            ),
            None => ("-".to_string(), "-".to_string(), "-".to_string()),
        };
        out.push_str(&format!(
            "  {:<36} {:<10} {:<12} {:<24} {:>9} {:>10} {:>10}\n",
            row.agent_run_id.0,
            status_label(row.status),
            truncate(&row.provider, 12),
            truncate(&row.model, 24),
            elapsed,
            tokens,
            cost,
        ));
    }
    out
}

/// Format a micro-dollar (millionth-of-a-dollar) cost estimate as `$X.XXXX`.
/// Four decimals keep sub-cent per-run estimates legible without scientific
/// notation; the value never wraps because it is a plain integer divide.
fn format_micro_dollars(micro_dollars: u64) -> String {
    let dollars = micro_dollars / 1_000_000;
    let frac = (micro_dollars % 1_000_000) / 100; // 4 decimal places (1e-4 dollars)
    format!("${dollars}.{frac:04}")
}

/// Format an elapsed wall-clock duration (milliseconds) compactly: `<1s` as
/// `NNNms`, under a minute as `N.Ns`, otherwise `NmN.Ns`. Pure integer math so
/// a pathological value never panics.
fn format_elapsed_ms(ms: u64) -> String {
    if ms < 1_000 {
        return format!("{ms}ms");
    }
    let tenths = (ms % 1_000) / 100;
    let total_secs = ms / 1_000;
    if total_secs < 60 {
        return format!("{total_secs}.{tenths}s");
    }
    let minutes = total_secs / 60;
    let secs = total_secs % 60;
    format!("{minutes}m{secs}.{tenths}s")
}

/// Stable lower-case label for a run status (the `{:?}` Debug form is not a
/// display contract). Exhaustive so a new `AgentRunStatus` variant is a compile
/// error here rather than a silent `"unknown"`.
pub fn status_label(status: AgentRunStatus) -> &'static str {
    match status {
        AgentRunStatus::Queued => "queued",
        AgentRunStatus::Running => "running",
        AgentRunStatus::Blocked => "blocked",
        AgentRunStatus::Completed => "completed",
        AgentRunStatus::Failed => "failed",
    }
}

/// Classification of a `/agent cancel <id>` target against the persisted run
/// snapshots, so the command only fires an abort for a genuinely in-flight run
/// and reports accurately otherwise (CEX-S2-16 验收 (2)).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentRunCancelTarget {
    /// The id matches a run still in flight (`Queued` / `Running` / `Blocked`):
    /// cancellation should fire.
    InFlight,
    /// The id matches a run that already reached a terminal state — there is
    /// nothing to cancel.
    AlreadyTerminal(AgentRunStatus),
    /// No persisted run matches the id (unparseable id or unknown run).
    NotFound,
}

/// Classify a `/agent cancel <id>` request against the persisted run snapshots.
/// `run_id` is the raw user-supplied string; a malformed UUID resolves to
/// [`AgentRunCancelTarget::NotFound`] rather than firing a blind abort. Pure —
/// a lookup over `runs`, no I/O.
///
/// With the CEX-S2-12 concurrency cap of 1 there is at most one in-flight run,
/// so an `InFlight` classification uniquely identifies the run the parent abort
/// token will stop; true multi-run targeting arrives with the S2-14 registry.
pub fn classify_cancel_target(run_id: &str, runs: &[AgentRun]) -> AgentRunCancelTarget {
    let Ok(uuid) = uuid::Uuid::parse_str(run_id.trim()) else {
        return AgentRunCancelTarget::NotFound;
    };
    let target = AgentRunId(uuid);
    match runs.iter().find(|run| run.id == target) {
        Some(run) if run.status.is_terminal() => AgentRunCancelTarget::AlreadyTerminal(run.status),
        Some(_) => AgentRunCancelTarget::InFlight,
        None => AgentRunCancelTarget::NotFound,
    }
}

/// Char-count (not byte) truncation so a multi-byte model slug cannot panic by
/// slicing mid-codepoint; an over-long cell ends with `…`.
fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    if max <= 1 {
        return value.chars().take(max).collect();
    }
    let head: String = value.chars().take(max - 1).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;

    fn run(id: u128, status: AgentRunStatus) -> AgentRun {
        AgentRun {
            id: AgentRunId(Uuid::from_u128(id)),
            task_id: AgentTaskId(Uuid::from_u128(id)),
            thread_id: Uuid::from_u128(0xffff),
            provider: "deepseek".to_string(),
            model: "deepseek-chat".to_string(),
            transcript_path: format!(".libra/sessions/t/agents/{id}.jsonl"),
            workspace_path: None,
            status,
        }
    }

    #[test]
    fn empty_input_yields_no_rows() {
        assert!(agent_pane_rows(&[]).is_empty());
    }

    #[test]
    fn fields_map_one_to_one() {
        let mut r = run(1, AgentRunStatus::Running);
        r.workspace_path = Some("/ws".to_string());
        let rows = agent_pane_rows(std::slice::from_ref(&r));
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.agent_run_id, r.id);
        assert_eq!(row.task_id, r.task_id);
        assert_eq!(row.status, AgentRunStatus::Running);
        assert_eq!(row.provider, "deepseek");
        assert_eq!(row.model, "deepseek-chat");
        assert_eq!(row.transcript_path, r.transcript_path);
        assert_eq!(row.workspace_path.as_deref(), Some("/ws"));
    }

    #[test]
    fn in_flight_rows_sort_before_terminal() {
        // Interleave terminal and in-flight runs; every in-flight row must
        // precede every terminal row regardless of input order.
        let runs = vec![
            run(10, AgentRunStatus::Completed),
            run(11, AgentRunStatus::Running),
            run(12, AgentRunStatus::Failed),
            run(13, AgentRunStatus::Queued),
            run(14, AgentRunStatus::Blocked),
        ];
        let rows = agent_pane_rows(&runs);
        let first_terminal = rows
            .iter()
            .position(|r| r.status.is_terminal())
            .expect("there is a terminal row");
        assert!(
            rows[..first_terminal]
                .iter()
                .all(|r| r.status.is_in_flight()),
            "all rows before the first terminal row must be in-flight",
        );
        assert!(
            rows[first_terminal..]
                .iter()
                .all(|r| r.status.is_terminal()),
            "all rows from the first terminal row on must be terminal",
        );
    }

    #[test]
    fn format_pane_empty_yields_placeholder() {
        let out = format_agent_run_pane_with_usage(&[], |_| None);
        assert!(out.contains("No sub-agent runs recorded yet."));
    }

    #[test]
    fn format_pane_renders_rows_in_flight_first_with_status_labels() {
        let runs = vec![
            run(20, AgentRunStatus::Completed),
            run(21, AgentRunStatus::Running),
        ];
        let out = format_agent_run_pane_with_usage(&runs, |_| None);
        assert!(out.contains("Agent runs:"));
        assert!(out.contains("run") && out.contains("status") && out.contains("provider"));
        assert!(out.contains("running") && out.contains("completed"));
        assert!(out.contains("deepseek"));
        // In-flight (run 21, Running) sorts before terminal (run 20, Completed).
        let running_pos = out.find("running").expect("running row");
        let completed_pos = out.find("completed").expect("completed row");
        assert!(
            running_pos < completed_pos,
            "in-flight runs must render before terminal ones",
        );
    }

    #[test]
    fn format_pane_truncates_long_model() {
        let mut r = run(30, AgentRunStatus::Running);
        r.model = "acmecorp/some-extremely-long-model-identifier-slug".to_string();
        let out = format_agent_run_pane_with_usage(std::slice::from_ref(&r), |_| None);
        assert!(
            out.contains('…'),
            "an over-long model cell must be truncated"
        );
    }

    #[test]
    fn ordering_is_deterministic_and_breaks_ties_by_id() {
        // Same status, ids out of order on input -> sorted ascending by id.
        let runs = vec![
            run(3, AgentRunStatus::Running),
            run(1, AgentRunStatus::Running),
            run(2, AgentRunStatus::Running),
        ];
        let rows = agent_pane_rows(&runs);
        let ids: Vec<Uuid> = rows.iter().map(|r| r.agent_run_id.0).collect();
        assert_eq!(
            ids,
            vec![Uuid::from_u128(1), Uuid::from_u128(2), Uuid::from_u128(3)],
        );
        // Re-projecting the same input yields the identical order (replay-stable).
        assert_eq!(agent_pane_rows(&runs), rows);
    }

    fn usage(prompt: u64, completion: u64, cost_micro: u64) -> RunUsage {
        RunUsage {
            prompt_tokens: prompt,
            completion_tokens: completion,
            cost_estimate_micro_dollars: cost_micro,
            ..RunUsage::default()
        }
    }

    #[test]
    fn rows_with_usage_join_and_preserve_ordering() {
        let runs = vec![
            run(40, AgentRunStatus::Completed),
            run(41, AgentRunStatus::Running),
        ];
        // Look up usage only for the completed run (41 is still in flight).
        let rows = agent_pane_rows_with_usage(&runs, |r| {
            (r.id == AgentRunId(Uuid::from_u128(40))).then(|| usage(120, 60, 1_500))
        });
        // In-flight (41) still sorts before terminal (40).
        assert_eq!(rows[0].agent_run_id, AgentRunId(Uuid::from_u128(41)));
        assert_eq!(rows[0].usage, None, "the in-flight run has no usage joined");
        assert_eq!(rows[1].agent_run_id, AgentRunId(Uuid::from_u128(40)));
        assert_eq!(rows[1].usage.expect("usage joined").total_tokens(), 180);
    }

    #[test]
    fn format_pane_with_usage_shows_tokens_and_cost() {
        let runs = vec![run(50, AgentRunStatus::Completed)];
        let out = format_agent_run_pane_with_usage(&runs, |_| Some(usage(120, 60, 1_500)));
        assert!(
            out.contains("tokens") && out.contains("cost"),
            "headers present"
        );
        assert!(
            out.contains("180"),
            "total tokens (120+60) must render: {out}"
        );
        // 1_500 micro-dollars = $0.0015.
        assert!(out.contains("$0.0015"), "cost estimate must render: {out}");
    }

    #[test]
    fn format_pane_without_usage_shows_dashes() {
        let runs = vec![run(60, AgentRunStatus::Running)];
        let out = format_agent_run_pane_with_usage(&runs, |_| None);
        // The token/cost cells render `-` when no usage is joined.
        assert!(
            out.contains("tokens") && out.contains('-'),
            "no-usage rows show dashes in the token/cost columns: {out}",
        );
    }

    #[test]
    fn classify_cancel_target_distinguishes_in_flight_terminal_and_unknown() {
        let in_flight = run(80, AgentRunStatus::Running);
        let done = run(81, AgentRunStatus::Completed);
        let runs = vec![in_flight.clone(), done.clone()];

        // An in-flight run is cancellable.
        assert_eq!(
            classify_cancel_target(&in_flight.id.0.to_string(), &runs),
            AgentRunCancelTarget::InFlight,
        );
        // A terminal run reports its state and is not cancelled.
        assert_eq!(
            classify_cancel_target(&done.id.0.to_string(), &runs),
            AgentRunCancelTarget::AlreadyTerminal(AgentRunStatus::Completed),
        );
        // An unknown but valid uuid, and a malformed id, are both NotFound — no
        // blind abort fires for a garbage id.
        assert_eq!(
            classify_cancel_target(&Uuid::from_u128(0xdead).to_string(), &runs),
            AgentRunCancelTarget::NotFound,
        );
        assert_eq!(
            classify_cancel_target("not-a-uuid", &runs),
            AgentRunCancelTarget::NotFound,
        );
        // Surrounding whitespace is tolerated.
        assert_eq!(
            classify_cancel_target(&format!("  {}  ", in_flight.id.0), &runs),
            AgentRunCancelTarget::InFlight,
        );
    }

    #[test]
    fn micro_dollars_format_is_four_decimal_dollars() {
        assert_eq!(format_micro_dollars(0), "$0.0000");
        assert_eq!(format_micro_dollars(1_500), "$0.0015");
        assert_eq!(format_micro_dollars(1_000_000), "$1.0000");
        assert_eq!(format_micro_dollars(2_345_600), "$2.3456");
    }

    #[test]
    fn elapsed_format_scales_ms_seconds_minutes() {
        assert_eq!(format_elapsed_ms(0), "0ms");
        assert_eq!(format_elapsed_ms(420), "420ms");
        assert_eq!(format_elapsed_ms(4_200), "4.2s");
        assert_eq!(format_elapsed_ms(59_900), "59.9s");
        assert_eq!(format_elapsed_ms(63_400), "1m3.4s");
    }

    #[test]
    fn format_pane_shows_elapsed_from_wall_clock() {
        let runs = vec![run(70, AgentRunStatus::Completed)];
        let out = format_agent_run_pane_with_usage(&runs, |_| {
            Some(RunUsage {
                prompt_tokens: 10,
                completion_tokens: 5,
                wall_clock_ms: 4_200,
                ..RunUsage::default()
            })
        });
        assert!(out.contains("elapsed"), "elapsed header present: {out}");
        assert!(
            out.contains("4.2s"),
            "wall-clock elapsed must render: {out}"
        );
    }
}
