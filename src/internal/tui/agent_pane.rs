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
//! calls, context-pack hash and permission profile. None of that lives on the
//! [`AgentRun`] snapshot; it is joined from the usage / budget / event /
//! context-pack records by a later slice. This projection surfaces only the
//! fields the snapshot itself carries.

use crate::internal::ai::agent_run::{AgentRun, AgentRunId, AgentRunStatus, AgentTaskId};

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
}

impl AgentRunRow {
    /// Project a single [`AgentRun`] snapshot into a display row (1:1 field map).
    fn from_run(run: &AgentRun) -> Self {
        Self {
            agent_run_id: run.id,
            task_id: run.task_id,
            status: run.status,
            provider: run.provider.clone(),
            model: run.model.clone(),
            transcript_path: run.transcript_path.clone(),
            workspace_path: run.workspace_path.clone(),
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
}
