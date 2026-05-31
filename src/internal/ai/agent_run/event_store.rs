//! Append-only per-run event store for sub-agent runs (CEX-S2-11 (3)).
//!
//! 子代理运行的每次运行仅追加事件存储 (CEX-S2-11 (3))。
//!
//! Every sub-agent lifecycle event — including the
//! `workspace_materialized` event a workspace materialization emits —
//! is written to a **per-run** JSONL transcript at
//! `.libra/sessions/{thread_id}/agents/{run_id}.jsonl`, *not* the main
//! session JSONL. Keeping run events in their own file is what lets the
//! main session stay byte-equivalent to the CEX-00 / CP-S2-2 baseline
//! while sub-agent runs accumulate their own append-only history
//! (`docs/improvement/agent.md` CEX-S2-11 (3), and the `AgentRun`
//! `transcript_path` contract at [`super::run::AgentRun`]).
//!
//! This module owns only the path resolution and the append / read I/O.
//! Producing the events (selecting a strategy, materializing the
//! workspace, measuring timings) lives in
//! [`super::workspace_strategy`] and the dispatcher wiring that calls it.

use std::{
    fs::{self, OpenOptions},
    io::{self, BufRead, BufReader, Write},
    path::{Path, PathBuf},
};

use uuid::Uuid;

use super::{
    AgentRunId,
    event::{AgentRunEvent, AgentRunEventEnvelope},
};

/// Append-only store for per-run agent event transcripts, rooted at a
/// `.libra/sessions` directory.
///
/// The store is stateless beyond the root path: each call resolves the
/// run's transcript path and performs a single append or a full read.
///
/// # Single-writer-per-run invariant
///
/// A given run's transcript is written by exactly one producer — the
/// runtime driving that `AgentRun`, which emits the run's lifecycle
/// events sequentially (one append per event as the run progresses).
/// The store deliberately takes no lock: distinct runs write distinct
/// paths, and a single run is never appended to concurrently. Under
/// that invariant each [`append`](Self::append) writes a complete line
/// and the transcript is never torn. The store is **not** a concurrent
/// multi-writer queue for one run; callers that would fan multiple
/// threads into the same run's transcript must serialize themselves.
#[derive(Clone, Debug)]
pub struct AgentRunEventStore {
    /// The `.libra/sessions` directory that holds `{thread_id}/...` trees.
    sessions_root: PathBuf,
}

impl AgentRunEventStore {
    /// Construct a store rooted at a `.libra/sessions` directory.
    pub fn new(sessions_root: impl Into<PathBuf>) -> Self {
        Self {
            sessions_root: sessions_root.into(),
        }
    }

    /// Resolve the per-run transcript path
    /// `.libra/sessions/{thread_id}/agents/{run_id}.jsonl`.
    ///
    /// The `agents/` segment is what separates run transcripts from the
    /// main session's `events.jsonl`, satisfying the CEX-S2-11 (3)
    /// requirement that run events never land in the main session file.
    pub fn transcript_path(&self, thread_id: Uuid, run_id: AgentRunId) -> PathBuf {
        self.sessions_root
            .join(thread_id.to_string())
            .join("agents")
            .join(format!("{}.jsonl", run_id.0))
    }

    /// Append one event as a single JSON line, creating the
    /// `{thread_id}/agents/` parent directories on first write.
    ///
    /// The event is serialized through [`AgentRunEvent`]'s
    /// `tag = "kind"` / `content = "payload"` shape; readers parse it
    /// back through [`AgentRunEventEnvelope`].
    pub fn append(
        &self,
        thread_id: Uuid,
        run_id: AgentRunId,
        event: &AgentRunEvent,
    ) -> io::Result<()> {
        let path = self.transcript_path(thread_id, run_id);
        ensure_parent_dir(&path)?;

        let mut line = serde_json::to_string(event).map_err(io::Error::other)?;
        line.push('\n');

        let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
        file.write_all(line.as_bytes())
    }

    /// Read every event in a run's transcript in append order, parsing
    /// each line through the forward-compatible [`AgentRunEventEnvelope`].
    ///
    /// Per that envelope's contract, a line lands in
    /// [`AgentRunEventEnvelope::Unknown`] when it is not parseable as a
    /// recognized [`AgentRunEvent`] — this covers **both** a genuinely
    /// future event kind (the intended forward-compat case, S2-INV-10)
    /// **and** a line whose `kind` is recognized but whose payload is
    /// malformed: the untagged envelope cannot distinguish the two, so
    /// data corruption surfaces as `Unknown` rather than a read error.
    /// Only a line that is not valid JSON at all fails the read. Callers
    /// that need to detect corruption of a known kind must re-validate
    /// the `Unknown` rows against the kinds they expect.
    ///
    /// A missing transcript is not an error — a run that never emitted an
    /// event yields an empty vec. Blank lines are skipped.
    pub fn read(
        &self,
        thread_id: Uuid,
        run_id: AgentRunId,
    ) -> io::Result<Vec<AgentRunEventEnvelope>> {
        let path = self.transcript_path(thread_id, run_id);
        let file = match fs::File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };

        let mut events = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let envelope: AgentRunEventEnvelope =
                serde_json::from_str(&line).map_err(io::Error::other)?;
            events.push(envelope);
        }
        Ok(events)
    }
}

fn ensure_parent_dir(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::agent_run::{
        event::WorkspaceStrategy,
        workspace_strategy::{WorkspaceSizing, record_materialization},
    };

    fn store() -> (tempfile::TempDir, AgentRunEventStore) {
        let temp = tempfile::tempdir().expect("tempdir for event store");
        let sessions_root = temp.path().join(".libra").join("sessions");
        let store = AgentRunEventStore::new(&sessions_root);
        (temp, store)
    }

    /// The transcript path is exactly
    /// `.libra/sessions/{thread_id}/agents/{run_id}.jsonl` — pins the
    /// CEX-S2-11 (3) / `AgentRun::transcript_path` contract, and in
    /// particular the `agents/` segment that keeps run events out of the
    /// main session `events.jsonl`.
    #[test]
    fn transcript_path_is_under_per_thread_agents_dir() {
        let (temp, store) = store();
        let thread_id = Uuid::new_v4();
        let run_id = AgentRunId::new();

        let path = store.transcript_path(thread_id, run_id);
        let expected = temp
            .path()
            .join(".libra")
            .join("sessions")
            .join(thread_id.to_string())
            .join("agents")
            .join(format!("{}.jsonl", run_id.0));
        assert_eq!(path, expected);

        // It must NOT be the main session events.jsonl.
        let main_session = temp
            .path()
            .join(".libra")
            .join("sessions")
            .join(thread_id.to_string())
            .join("events.jsonl");
        assert_ne!(path, main_session);
    }

    /// Appending creates the parent dirs on first write and accumulates
    /// events in order; `read` returns them as recognized `Known`
    /// envelopes.
    #[test]
    fn append_creates_dirs_and_accumulates_in_order() {
        let (_temp, store) = store();
        let thread_id = Uuid::new_v4();
        let run_id = AgentRunId::new();

        let started = AgentRunEvent::Started {
            agent_run_id: run_id,
        };
        let completed = AgentRunEvent::Completed {
            agent_run_id: run_id,
        };

        store
            .append(thread_id, run_id, &started)
            .expect("append started");
        store
            .append(thread_id, run_id, &completed)
            .expect("append completed");

        let events = store.read(thread_id, run_id).expect("read back");
        assert_eq!(events.len(), 2, "both appends must be present, in order");
        assert_eq!(events[0].known(), Some(&started));
        assert_eq!(events[1].known(), Some(&completed));
    }

    /// Reading a run that never emitted an event is not an error — it
    /// yields an empty vec (missing file == no events).
    #[test]
    fn read_missing_transcript_yields_empty() {
        let (_temp, store) = store();
        let events = store
            .read(Uuid::new_v4(), AgentRunId::new())
            .expect("missing transcript must read as empty, not error");
        assert!(events.is_empty());
    }

    /// The `workspace_materialized` event (CEX-S2-11 (3)) round-trips
    /// through the store with its snake_case `kind` tag intact — this is
    /// the exact wire shape the dispatcher will append once materialization
    /// is wired in. Pins both the on-disk tag and the payload fields.
    #[test]
    fn workspace_materialized_event_round_trips_with_snake_case_kind() {
        let (_temp, store) = store();
        let thread_id = Uuid::new_v4();
        let run_id = AgentRunId::new();

        let materialization = record_materialization(
            WorkspaceStrategy::Sparse,
            WorkspaceSizing {
                repo_size_bytes: 2 * 1024 * 1024 * 1024,
                worktree_file_count: 250_000,
            },
            250_000,
            1_500,
            None,
        );
        let event = AgentRunEvent::WorkspaceMaterialized {
            agent_run_id: run_id,
            materialization: materialization.clone(),
        };
        store
            .append(thread_id, run_id, &event)
            .expect("append event");

        // Raw on-disk line carries the snake_case `kind` tag.
        let raw = std::fs::read_to_string(store.transcript_path(thread_id, run_id))
            .expect("read raw transcript");
        assert!(
            raw.contains("\"kind\":\"workspace_materialized\""),
            "on-disk line must use the snake_case workspace_materialized tag; got {raw}",
        );

        let events = store.read(thread_id, run_id).expect("read back");
        assert_eq!(events.len(), 1);
        match events[0].known() {
            Some(AgentRunEvent::WorkspaceMaterialized {
                materialization: back,
                ..
            }) => {
                assert_eq!(back, &materialization);
            }
            other => panic!("expected WorkspaceMaterialized, got {other:?}"),
        }
    }

    /// A line emitted by a future, unrecognized event type must parse as
    /// `Unknown` (forward compatibility, S2-INV-10) rather than failing
    /// the whole read — an old reader can still consume a newer
    /// transcript.
    #[test]
    fn read_preserves_unknown_future_events() {
        let (_temp, store) = store();
        let thread_id = Uuid::new_v4();
        let run_id = AgentRunId::new();

        // A recognized event, then a hand-written future-kind line.
        store
            .append(
                thread_id,
                run_id,
                &AgentRunEvent::Started {
                    agent_run_id: run_id,
                },
            )
            .expect("append started");
        let path = store.transcript_path(thread_id, run_id);
        let mut file = OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("open for append");
        file.write_all(b"{\"kind\":\"future_event_from_step_3\",\"payload\":{\"x\":1}}\n")
            .expect("append future line");

        let events = store.read(thread_id, run_id).expect("read back");
        assert_eq!(events.len(), 2);
        assert!(events[0].known().is_some(), "known event stays known");
        assert!(
            events[1].is_unknown(),
            "unrecognized future event must parse as Unknown, not fail the read",
        );
    }

    /// Pins the documented `read()` contract: a line whose `kind` IS
    /// recognized but whose payload is malformed (here, a non-UUID
    /// `agent_run_id`) lands in `Unknown` — the untagged envelope can't
    /// tell corruption from a future kind — while a line that is not
    /// valid JSON at all fails the whole read.
    #[test]
    fn read_routes_malformed_known_to_unknown_and_fails_on_non_json() {
        let (_temp, store) = store();
        let thread_id = Uuid::new_v4();
        let run_id = AgentRunId::new();

        // Recognized kind, malformed payload (agent_run_id is not a UUID).
        let path = store.transcript_path(thread_id, run_id);
        ensure_parent_dir(&path).expect("mk parent");
        {
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .expect("open for append");
            file.write_all(
                b"{\"kind\":\"started\",\"payload\":{\"agent_run_id\":\"not-a-uuid\"}}\n",
            )
            .expect("append malformed-known line");
        }
        let events = store
            .read(thread_id, run_id)
            .expect("malformed-known must not fail read");
        assert_eq!(events.len(), 1);
        assert!(
            events[0].is_unknown(),
            "a recognized kind with a malformed payload must surface as Unknown",
        );

        // A line that is not valid JSON fails the read outright.
        {
            let mut file = OpenOptions::new()
                .append(true)
                .open(&path)
                .expect("open for append");
            file.write_all(b"this is not json at all\n")
                .expect("append non-json line");
        }
        assert!(
            store.read(thread_id, run_id).is_err(),
            "a non-JSON line must fail the read, not be swallowed",
        );
    }
}
