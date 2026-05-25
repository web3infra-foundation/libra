//! `AgentRun[S]` snapshot: one sub-agent execution attempt for an `AgentTask`.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{AgentRunId, AgentTaskId};

/// Lifecycle status of an `AgentRun`. Five reachable states matching
/// CEX-S2-16 TUI agent pane requirements.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AgentRunStatus {
    Queued,
    Running,
    Blocked,
    Completed,
    Failed,
}

impl AgentRunStatus {
    /// `true` for the **terminal** states — `Completed` / `Failed` —
    /// from which no further status transition occurs.
    ///
    /// Written as an exhaustive `match` (not `matches!`) so that adding
    /// a future variant to this `#[non_exhaustive]` enum is a
    /// compile error here until it is explicitly classified as terminal
    /// or non-terminal — terminal-ness is never inferred by default.
    ///
    /// `is_terminal()` and [`is_in_flight()`](Self::is_in_flight)
    /// partition the enum: exactly one is `true` for every variant.
    pub fn is_terminal(self) -> bool {
        match self {
            Self::Completed | Self::Failed => true,
            Self::Queued | Self::Running | Self::Blocked => false,
        }
    }

    /// `true` for the **non-terminal** states — `Queued` / `Running` /
    /// `Blocked` — a run that has not reached a terminal state.
    /// `Blocked` is non-terminal, not terminal: a blocked run is
    /// awaiting approval / human input / a budget top-up and may still
    /// resume.
    ///
    /// Complement of [`is_terminal()`](Self::is_terminal).
    pub fn is_in_flight(self) -> bool {
        !self.is_terminal()
    }
}

/// One sub-agent execution attempt. Bound to a provider/model and an isolated
/// workspace at spawn time.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentRun {
    pub id: AgentRunId,

    pub task_id: AgentTaskId,

    /// Thread id from the parent Layer 1 session. Used for trace id chain
    /// `thread_id → agent_run_id → tool_call_id → source_call_id`.
    pub thread_id: Uuid,

    /// Provider slug (e.g. `"deepseek"`, `"ollama"`, `"anthropic"`). The
    /// runtime maps this to a real provider client at dispatch time.
    pub provider: String,

    /// Model id within the provider (e.g. `"deepseek-chat"`).
    pub model: String,

    /// Path on disk to the JSONL transcript for this run. Lives under
    /// `.libra/sessions/{thread_id}/agents/{run_id}.jsonl` per CEX-S2-10 (3).
    pub transcript_path: String,

    /// Path on disk to the isolated workspace (worktree / sparse / blocked /
    /// full-copy fallback). `None` until CEX-S2-11 materializes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_path: Option<String>,

    /// Current status. Mutated only by Runtime via append-only events.
    pub status: AgentRunStatus,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `is_terminal` / `is_in_flight` must partition every
    /// `AgentRunStatus` variant: exactly one is `true`. (The
    /// exhaustive `match` in `is_terminal` is what forces a future
    /// variant to be classified — a compile error — rather than this
    /// test; this test pins the partition property itself.)
    #[test]
    fn terminal_and_in_flight_partition_every_status() {
        let all = [
            AgentRunStatus::Queued,
            AgentRunStatus::Running,
            AgentRunStatus::Blocked,
            AgentRunStatus::Completed,
            AgentRunStatus::Failed,
        ];
        for status in all {
            assert_ne!(
                status.is_terminal(),
                status.is_in_flight(),
                "{status:?} must be exactly one of terminal / in-flight",
            );
        }
    }

    /// Pin the exact terminal set. `Completed` and `Failed` are
    /// terminal; `Queued` / `Running` / `Blocked` are in-flight.
    /// `Blocked` is the subtle one — it awaits external input but can
    /// still resume, so it must NOT be classified terminal.
    #[test]
    fn terminal_set_is_completed_and_failed_only() {
        assert!(AgentRunStatus::Completed.is_terminal());
        assert!(AgentRunStatus::Failed.is_terminal());

        for in_flight in [
            AgentRunStatus::Queued,
            AgentRunStatus::Running,
            AgentRunStatus::Blocked,
        ] {
            assert!(
                in_flight.is_in_flight(),
                "{in_flight:?} must be in-flight, not terminal",
            );
            assert!(!in_flight.is_terminal());
        }
    }

    /// `AgentRunStatus` serializes to stable snake_case wire tags that
    /// JSONL transcript / projection readers depend on. Pin them so a
    /// rename trips here rather than silently desyncing persisted runs.
    #[test]
    fn status_serializes_to_stable_snake_case_tags() {
        for (status, tag) in [
            (AgentRunStatus::Queued, "\"queued\""),
            (AgentRunStatus::Running, "\"running\""),
            (AgentRunStatus::Blocked, "\"blocked\""),
            (AgentRunStatus::Completed, "\"completed\""),
            (AgentRunStatus::Failed, "\"failed\""),
        ] {
            let wire = serde_json::to_string(&status).expect("serialize AgentRunStatus");
            assert_eq!(wire, tag, "unexpected wire tag for {status:?}");
            let back: AgentRunStatus =
                serde_json::from_str(&wire).expect("deserialize AgentRunStatus");
            assert_eq!(back, status, "AgentRunStatus wire tag must round-trip");
        }
    }

    /// CEX-S2-10 freezes the `AgentRun` wire contract
    /// (`#[serde(deny_unknown_fields)]`) — the central run record
    /// persisted to `agents/{run_id}.jsonl` and replayed by projection.
    /// The tests above pin only the `AgentRunStatus` field; pin the
    /// struct's required field set, the `workspace_path` skip-when-None,
    /// the `deny_unknown_fields` rejection, and the round-trip too, so a
    /// rename / added field / dropped-skip silently breaking persisted
    /// runs trips here.
    #[test]
    fn agent_run_wire_contract_is_frozen() {
        // Pre-materialization run: `workspace_path` None is omitted.
        let unmaterialized = AgentRun {
            id: AgentRunId::new(),
            task_id: AgentTaskId::new(),
            thread_id: Uuid::new_v4(),
            provider: "deepseek".to_string(),
            model: "deepseek-chat".to_string(),
            transcript_path: ".libra/sessions/t/agents/r.jsonl".to_string(),
            workspace_path: None,
            status: AgentRunStatus::Queued,
        };
        let json = serde_json::to_value(&unmaterialized).expect("serialize AgentRun");
        let obj = json.as_object().expect("AgentRun serializes to an object");
        // Assert the EXACT frozen key set, not just presence — so a
        // future added field (which old deny_unknown_fields readers would
        // reject) trips here rather than slipping through.
        let keys: std::collections::BTreeSet<&str> = obj.keys().map(String::as_str).collect();
        let base_keys: std::collections::BTreeSet<&str> = [
            "id",
            "task_id",
            "thread_id",
            "provider",
            "model",
            "transcript_path",
            "status",
        ]
        .into_iter()
        .collect();
        assert_eq!(
            keys, base_keys,
            "AgentRun (None workspace_path) must serialize EXACTLY the frozen key set \
             (workspace_path omitted by skip_serializing_if), got: {json}",
        );

        // Materialized run: `workspace_path` present when `Some` — and
        // exactly one extra key beyond the base set.
        let materialized = AgentRun {
            workspace_path: Some(".libra/sessions/t/worktrees/tasks/r/workspace".to_string()),
            ..unmaterialized
        };
        let materialized_json = serde_json::to_value(&materialized).expect("serialize AgentRun");
        let materialized_keys: std::collections::BTreeSet<&str> = materialized_json
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        let mut materialized_expected = base_keys.clone();
        materialized_expected.insert("workspace_path");
        assert_eq!(
            materialized_keys, materialized_expected,
            "materialized AgentRun must serialize EXACTLY the base set plus workspace_path, \
             got: {materialized_json}",
        );

        // deny_unknown_fields: an unknown field is rejected on read.
        let mut with_extra = materialized_json.as_object().expect("object").clone();
        with_extra.insert("bogus".to_string(), serde_json::Value::Bool(true));
        assert!(
            serde_json::from_value::<AgentRun>(serde_json::Value::Object(with_extra)).is_err(),
            "deny_unknown_fields must reject an unknown field",
        );

        // Round-trip: the wire shape deserializes and re-serializes intact.
        let back: AgentRun =
            serde_json::from_value(materialized_json.clone()).expect("deserialize AgentRun");
        assert_eq!(
            serde_json::to_value(&back).expect("re-serialize"),
            materialized_json,
            "AgentRun must round-trip its wire shape",
        );
    }
}
