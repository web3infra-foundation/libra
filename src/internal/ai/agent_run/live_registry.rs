//! CEX-S2-16 live run registry — the in-memory view of in-flight sub-agent
//! runs' real-time state (current tool / current file / source-call count) for
//! the `/agents` pane's live fields.
//!
//! Distinct from the persisted `AgentRun` snapshots (JSONL, read by
//! `AgentRunEventStore`): this registry holds only the *volatile* in-flight
//! view — populated when a run starts, mutated as the run's tool loop
//! progresses, and dropped when the run finishes. The persisted snapshots carry
//! the durable run identity / status / usage; this carries the moment-to-moment
//! "what is it doing right now" that the live pane needs and that no persisted
//! record captures (a JSONL replay can reconstruct status + usage, but not the
//! transient current-tool/current-file of a still-running child).
//!
//! Thread-safe and cheap to clone (`Arc`-backed), so the dispatcher can hand a
//! handle to each child tool loop (the writer) while the TUI render path reads
//! a deterministic [`snapshot`](LiveRunRegistry::snapshot) (ordered by run id).
//!
//! This is the data-structure foundation; wiring the writers (child tool loop)
//! and the reader (the `/agents` pane) is the follow-on, mirroring how
//! [`ParallelSchedulerState`](super::ParallelSchedulerState) landed as a tested
//! state machine before the dispatcher consumed it.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use super::AgentRunId;

/// The live, in-flight state of a single sub-agent run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LiveRunState {
    /// The tool the run is currently executing, if any.
    pub current_tool: Option<String>,
    /// The file the current tool is operating on, if known.
    pub current_file: Option<String>,
    /// Count of Source Pool / MCP / OpenAPI calls observed so far this run.
    pub source_call_count: u32,
}

/// A run's live state tagged with its id, as produced by
/// [`LiveRunRegistry::snapshot`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveRunSnapshot {
    pub agent_run_id: AgentRunId,
    pub state: LiveRunState,
}

/// Thread-safe in-memory registry of in-flight sub-agent runs' live state.
#[derive(Clone, Debug, Default)]
pub struct LiveRunRegistry {
    inner: Arc<Mutex<HashMap<AgentRunId, LiveRunState>>>,
}

impl LiveRunRegistry {
    /// A fresh, empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an in-flight run (idempotent — re-registering resets the run's
    /// live state to the default, matching a fresh start).
    pub fn register(&self, run_id: AgentRunId) {
        self.with_map(|map| {
            map.insert(run_id, LiveRunState::default());
        });
    }

    /// Set the tool the run is currently executing, creating the run's entry if
    /// it was never registered so a writer never silently drops an update.
    pub fn set_current_tool(&self, run_id: AgentRunId, tool: impl Into<String>) {
        let tool = tool.into();
        self.with_map(|map| {
            map.entry(run_id).or_default().current_tool = Some(tool);
        });
    }

    /// Set the file the current tool is operating on.
    pub fn set_current_file(&self, run_id: AgentRunId, file: impl Into<String>) {
        let file = file.into();
        self.with_map(|map| {
            map.entry(run_id).or_default().current_file = Some(file);
        });
    }

    /// Clear the current tool/file (e.g. between tool calls, when the run is
    /// thinking rather than executing a tool).
    pub fn clear_current_activity(&self, run_id: AgentRunId) {
        self.with_map(|map| {
            if let Some(state) = map.get_mut(&run_id) {
                state.current_tool = None;
                state.current_file = None;
            }
        });
    }

    /// Increment the run's observed source-call count, returning the new total.
    pub fn record_source_call(&self, run_id: AgentRunId) -> u32 {
        self.with_map(|map| {
            let state = map.entry(run_id).or_default();
            state.source_call_count = state.source_call_count.saturating_add(1);
            state.source_call_count
        })
    }

    /// Remove a finished run's live state. Idempotent.
    pub fn finish(&self, run_id: &AgentRunId) {
        self.with_map(|map| {
            map.remove(run_id);
        });
    }

    /// The live state of a single run, if it is in flight.
    pub fn get(&self, run_id: &AgentRunId) -> Option<LiveRunState> {
        self.with_map(|map| map.get(run_id).cloned())
    }

    /// `true` when no runs are currently in flight.
    pub fn is_empty(&self) -> bool {
        self.with_map(|map| map.is_empty())
    }

    /// A deterministic snapshot of every in-flight run's live state, ordered by
    /// run id so the pane renders stably across reads (CEX-S2-16 replay parity).
    pub fn snapshot(&self) -> Vec<LiveRunSnapshot> {
        let mut snapshot: Vec<LiveRunSnapshot> = self.with_map(|map| {
            map.iter()
                .map(|(id, state)| LiveRunSnapshot {
                    agent_run_id: *id,
                    state: state.clone(),
                })
                .collect()
        });
        snapshot.sort_by_key(|entry| entry.agent_run_id.0);
        snapshot
    }

    /// Run `f` against the locked map, recovering from a poisoned lock rather
    /// than propagating the panic — a writer that panics mid-update must not
    /// wedge the whole registry (the live view is best-effort telemetry).
    fn with_map<T>(&self, f: impl FnOnce(&mut HashMap<AgentRunId, LiveRunState>) -> T) -> T {
        match self.inner.lock() {
            Ok(mut guard) => f(&mut guard),
            Err(poisoned) => f(&mut poisoned.into_inner()),
        }
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;

    fn run(n: u128) -> AgentRunId {
        AgentRunId(Uuid::from_u128(n))
    }

    #[test]
    fn register_then_get_yields_default_state() {
        let reg = LiveRunRegistry::new();
        assert!(reg.is_empty());
        let id = run(1);
        reg.register(id);
        assert_eq!(reg.get(&id), Some(LiveRunState::default()));
        assert!(!reg.is_empty());
    }

    #[test]
    fn set_current_tool_and_file_are_reflected() {
        let reg = LiveRunRegistry::new();
        let id = run(2);
        reg.register(id);
        reg.set_current_tool(id, "apply_patch");
        reg.set_current_file(id, "src/lib.rs");
        let state = reg.get(&id).expect("run present");
        assert_eq!(state.current_tool.as_deref(), Some("apply_patch"));
        assert_eq!(state.current_file.as_deref(), Some("src/lib.rs"));
    }

    #[test]
    fn updates_on_unregistered_run_create_the_entry() {
        // A writer never silently drops an update: setting state on a run that
        // was never `register`ed materialises it with the update applied.
        let reg = LiveRunRegistry::new();
        let id = run(3);
        reg.set_current_tool(id, "shell");
        assert_eq!(
            reg.get(&id).and_then(|s| s.current_tool),
            Some("shell".to_string())
        );
    }

    #[test]
    fn clear_current_activity_drops_tool_and_file_but_keeps_run() {
        let reg = LiveRunRegistry::new();
        let id = run(4);
        reg.set_current_tool(id, "grep");
        reg.set_current_file(id, "a.rs");
        reg.clear_current_activity(id);
        let state = reg.get(&id).expect("run still in flight");
        assert_eq!(state.current_tool, None);
        assert_eq!(state.current_file, None);
    }

    #[test]
    fn record_source_call_increments_and_returns_total() {
        let reg = LiveRunRegistry::new();
        let id = run(5);
        assert_eq!(reg.record_source_call(id), 1);
        assert_eq!(reg.record_source_call(id), 2);
        assert_eq!(reg.get(&id).map(|s| s.source_call_count), Some(2));
    }

    #[test]
    fn finish_removes_the_run_and_is_idempotent() {
        let reg = LiveRunRegistry::new();
        let id = run(6);
        reg.register(id);
        reg.finish(&id);
        assert_eq!(reg.get(&id), None);
        // Second finish is a no-op, never a panic.
        reg.finish(&id);
        assert!(reg.is_empty());
    }

    #[test]
    fn snapshot_is_ordered_by_run_id_and_deterministic() {
        let reg = LiveRunRegistry::new();
        // Insert out of id order.
        reg.set_current_tool(run(30), "c");
        reg.set_current_tool(run(10), "a");
        reg.set_current_tool(run(20), "b");
        let snap = reg.snapshot();
        let ids: Vec<Uuid> = snap.iter().map(|e| e.agent_run_id.0).collect();
        assert_eq!(
            ids,
            vec![
                Uuid::from_u128(10),
                Uuid::from_u128(20),
                Uuid::from_u128(30)
            ],
        );
        // A second snapshot of the same state is identical (replay-stable).
        assert_eq!(reg.snapshot(), snap);
    }

    #[test]
    fn registry_clone_shares_the_same_state() {
        // The `Arc`-backed clone the dispatcher hands to a child writer must see
        // the same map the reader holds.
        let reader = LiveRunRegistry::new();
        let writer = reader.clone();
        let id = run(7);
        writer.set_current_tool(id, "read_file");
        assert_eq!(
            reader.get(&id).and_then(|s| s.current_tool),
            Some("read_file".to_string()),
        );
    }
}
