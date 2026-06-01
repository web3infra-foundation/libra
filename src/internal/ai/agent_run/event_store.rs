//! Append-only per-run event store for sub-agent runs (CEX-S2-11 (3)).
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
    permission::AgentPermissionProfile,
    run::AgentRun,
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

    /// Resolve the per-run **snapshot** path
    /// `.libra/sessions/{thread_id}/agents/{run_id}.run.json`.
    ///
    /// The `.run.json` suffix keeps the snapshot distinct from the run's
    /// append-only `{run_id}.jsonl` event transcript in the same `agents/`
    /// directory. Where the transcript is the run's event *history*, the
    /// snapshot is its current materialized [`AgentRun`] state — the record
    /// the MCP `libra://agents/runs` resources and the TUI agent pane read.
    pub fn snapshot_path(&self, thread_id: Uuid, run_id: AgentRunId) -> PathBuf {
        self.sessions_root
            .join(thread_id.to_string())
            .join("agents")
            .join(format!("{}.run.json", run_id.0))
    }

    /// Persist the latest [`AgentRun`] snapshot, **overwriting** any prior
    /// snapshot for the run. Unlike [`append`](Self::append), this is not
    /// append-only: the snapshot is the run's *current* state, so the
    /// producer rewrites it as the run transitions (spawned → running →
    /// terminal). Creates the `{thread_id}/agents/` parent dirs on first
    /// write. The snapshot is keyed by `run.id`, so the same store can hold
    /// many runs under one thread.
    pub fn write_snapshot(&self, thread_id: Uuid, run: &AgentRun) -> io::Result<()> {
        let path = self.snapshot_path(thread_id, run.id);
        ensure_parent_dir(&path)?;
        let json = serde_json::to_string_pretty(run).map_err(io::Error::other)?;
        fs::write(&path, json)
    }

    /// Read one run's snapshot. A missing snapshot is `Ok(None)` (a run that
    /// never persisted one) rather than an error; a present-but-corrupt
    /// snapshot surfaces as an error so corruption is never silently treated
    /// as "no run".
    pub fn read_snapshot(
        &self,
        thread_id: Uuid,
        run_id: AgentRunId,
    ) -> io::Result<Option<AgentRun>> {
        let path = self.snapshot_path(thread_id, run_id);
        match fs::read_to_string(&path) {
            Ok(json) => serde_json::from_str(&json)
                .map(Some)
                .map_err(io::Error::other),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// List every persisted run snapshot under a thread's `agents/` directory,
    /// sorted by run id for deterministic output. A missing `agents/` directory
    /// yields an empty vec. Only `*.run.json` snapshot files are read — the
    /// sibling `.jsonl` event transcripts are skipped — and a snapshot file that
    /// fails to parse fails the listing (corruption is not silently dropped).
    pub fn list_snapshots(&self, thread_id: Uuid) -> io::Result<Vec<AgentRun>> {
        let dir = self
            .sessions_root
            .join(thread_id.to_string())
            .join("agents");
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };

        let mut runs = Vec::new();
        for entry in entries {
            let path = entry?.path();
            let is_snapshot = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".run.json"));
            if !is_snapshot {
                continue;
            }
            let json = fs::read_to_string(&path)?;
            let run: AgentRun = serde_json::from_str(&json).map_err(io::Error::other)?;
            runs.push(run);
        }
        runs.sort_by_key(|run| run.id.0);
        Ok(runs)
    }

    /// List every persisted run snapshot across **all** threads under the
    /// sessions root, sorted by run id. The MCP `libra://agents/runs` list view
    /// is not scoped to one thread, so it aggregates every `{thread_id}/agents/
    /// *.run.json` snapshot. A missing sessions root yields an empty vec;
    /// top-level entries whose name is not a UUID (non-thread directories) are
    /// skipped, and a corrupt snapshot fails the listing.
    pub fn list_all_snapshots(&self) -> io::Result<Vec<AgentRun>> {
        let entries = match fs::read_dir(&self.sessions_root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };

        let mut runs = Vec::new();
        for entry in entries {
            let thread_dir = entry?.path();
            // Each top-level entry is a `{thread_id}` directory; parse its name
            // as a UUID so `list_snapshots` can resolve its `agents/` subdir.
            // Non-UUID entries (other session state) are skipped.
            let Some(thread_id) = thread_dir
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| Uuid::parse_str(name).ok())
            else {
                continue;
            };
            runs.extend(self.list_snapshots(thread_id)?);
        }
        runs.sort_by_key(|run| run.id.0);
        Ok(runs)
    }

    /// Resolve the per-run **permission profile** path
    /// `.libra/sessions/{thread_id}/agents/{run_id}.permissions.json`. A sibling
    /// of the run snapshot, holding the run's static [`AgentPermissionProfile`]
    /// (it does not change over the run's life) for the MCP
    /// `libra://agents/runs/{id}/permissions` resource.
    pub fn permissions_path(&self, thread_id: Uuid, run_id: AgentRunId) -> PathBuf {
        self.sessions_root
            .join(thread_id.to_string())
            .join("agents")
            .join(format!("{}.permissions.json", run_id.0))
    }

    /// Persist a run's permission profile (write-once; the profile is fixed at
    /// dispatch). Creates the `{thread_id}/agents/` parent dirs on first write.
    pub fn write_run_permissions(
        &self,
        thread_id: Uuid,
        run_id: AgentRunId,
        profile: &AgentPermissionProfile,
    ) -> io::Result<()> {
        let path = self.permissions_path(thread_id, run_id);
        ensure_parent_dir(&path)?;
        let json = serde_json::to_string_pretty(profile).map_err(io::Error::other)?;
        fs::write(&path, json)
    }

    /// Read a run's persisted permission profile. A missing profile is
    /// `Ok(None)`; a present-but-corrupt profile surfaces as an error.
    pub fn read_run_permissions(
        &self,
        thread_id: Uuid,
        run_id: AgentRunId,
    ) -> io::Result<Option<AgentPermissionProfile>> {
        let path = self.permissions_path(thread_id, run_id);
        match fs::read_to_string(&path) {
            Ok(json) => serde_json::from_str(&json)
                .map(Some)
                .map_err(io::Error::other),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
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
        AgentRunStatus, AgentTaskId,
        event::WorkspaceStrategy,
        workspace_strategy::{WorkspaceSizing, record_materialization},
    };

    fn store() -> (tempfile::TempDir, AgentRunEventStore) {
        let temp = tempfile::tempdir().expect("tempdir for event store");
        let sessions_root = temp.path().join(".libra").join("sessions");
        let store = AgentRunEventStore::new(&sessions_root);
        (temp, store)
    }

    fn sample_run(id: AgentRunId, status: AgentRunStatus) -> AgentRun {
        AgentRun {
            id,
            task_id: AgentTaskId::new(),
            thread_id: Uuid::new_v4(),
            provider: "deepseek".to_string(),
            model: "deepseek-chat".to_string(),
            transcript_path: format!("agents/{}.jsonl", id.0),
            workspace_path: None,
            status,
        }
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

    /// The snapshot path sits in the same `agents/` dir as the transcript but
    /// carries the distinct `.run.json` suffix, so the run's current-state
    /// snapshot never collides with its append-only `.jsonl` event log.
    #[test]
    fn snapshot_path_is_distinct_from_transcript() {
        let (_temp, store) = store();
        let thread_id = Uuid::new_v4();
        let run_id = AgentRunId::new();

        let snapshot = store.snapshot_path(thread_id, run_id);
        let transcript = store.transcript_path(thread_id, run_id);
        assert_ne!(snapshot, transcript);
        assert!(
            snapshot
                .to_string_lossy()
                .ends_with(&format!("{}.run.json", run_id.0)),
        );
        assert_eq!(snapshot.parent(), transcript.parent());
    }

    /// A written snapshot round-trips: reading it back yields the same run
    /// fields. (`AgentRun` is not `PartialEq`, so the key fields are checked
    /// individually.)
    #[test]
    fn write_then_read_snapshot_round_trips() {
        let (_temp, store) = store();
        let thread_id = Uuid::new_v4();
        let run = sample_run(AgentRunId::new(), AgentRunStatus::Running);

        store
            .write_snapshot(thread_id, &run)
            .expect("write snapshot");
        let back = store
            .read_snapshot(thread_id, run.id)
            .expect("read snapshot")
            .expect("snapshot must be present");

        assert_eq!(back.id, run.id);
        assert_eq!(back.task_id, run.task_id);
        assert_eq!(back.thread_id, run.thread_id);
        assert_eq!(back.provider, run.provider);
        assert_eq!(back.model, run.model);
        assert_eq!(back.transcript_path, run.transcript_path);
        assert_eq!(back.status, run.status);
    }

    /// A run that never persisted a snapshot reads as `None`, not an error.
    #[test]
    fn read_missing_snapshot_yields_none() {
        let (_temp, store) = store();
        let snapshot = store
            .read_snapshot(Uuid::new_v4(), AgentRunId::new())
            .expect("missing snapshot must read as Ok(None)");
        assert!(snapshot.is_none());
    }

    /// The snapshot is current-state, not append-only: a second write for the
    /// same run id overwrites the first, so a status transition is reflected.
    #[test]
    fn write_snapshot_overwrites_prior_state() {
        let (_temp, store) = store();
        let thread_id = Uuid::new_v4();
        let run_id = AgentRunId::new();

        store
            .write_snapshot(thread_id, &sample_run(run_id, AgentRunStatus::Running))
            .expect("write running snapshot");
        store
            .write_snapshot(thread_id, &sample_run(run_id, AgentRunStatus::Completed))
            .expect("overwrite with completed snapshot");

        let back = store
            .read_snapshot(thread_id, run_id)
            .expect("read snapshot")
            .expect("present");
        assert_eq!(
            back.status,
            AgentRunStatus::Completed,
            "the latest write must win — snapshot is current state, not history",
        );
    }

    /// `list_snapshots` returns every run snapshot under a thread, sorted by
    /// run id, and skips the sibling `.jsonl` event transcript (a run can have
    /// both a transcript and a snapshot in the same `agents/` dir).
    #[test]
    fn list_snapshots_returns_all_sorted_and_skips_transcripts() {
        let (_temp, store) = store();
        let thread_id = Uuid::new_v4();
        let run_a = sample_run(
            AgentRunId::from(Uuid::from_u128(1)),
            AgentRunStatus::Running,
        );
        let run_b = sample_run(
            AgentRunId::from(Uuid::from_u128(2)),
            AgentRunStatus::Completed,
        );

        store.write_snapshot(thread_id, &run_b).expect("write b");
        store.write_snapshot(thread_id, &run_a).expect("write a");
        // Also append an event transcript for run_a — it must NOT be parsed as
        // a snapshot by the listing.
        store
            .append(
                thread_id,
                run_a.id,
                &AgentRunEvent::Started {
                    agent_run_id: run_a.id,
                },
            )
            .expect("append transcript event");

        let runs = store.list_snapshots(thread_id).expect("list snapshots");
        assert_eq!(
            runs.len(),
            2,
            "exactly the two snapshots, not the transcript"
        );
        assert_eq!(runs[0].id, run_a.id, "sorted by run id: u128(1) first");
        assert_eq!(runs[1].id, run_b.id);
    }

    /// `list_all_snapshots` aggregates snapshots across every thread under the
    /// sessions root (the MCP run-list view is not thread-scoped), sorted by run
    /// id, and skips non-UUID top-level entries.
    #[test]
    fn list_all_snapshots_aggregates_across_threads() {
        let (temp, store) = store();
        let thread_a = Uuid::new_v4();
        let thread_b = Uuid::new_v4();
        let run_a = sample_run(
            AgentRunId::from(Uuid::from_u128(1)),
            AgentRunStatus::Running,
        );
        let run_b = sample_run(
            AgentRunId::from(Uuid::from_u128(2)),
            AgentRunStatus::Completed,
        );
        store.write_snapshot(thread_a, &run_a).expect("write a");
        store.write_snapshot(thread_b, &run_b).expect("write b");
        // A non-UUID sibling directory under the sessions root must be ignored.
        std::fs::create_dir_all(
            temp.path()
                .join(".libra")
                .join("sessions")
                .join("not-a-thread"),
        )
        .expect("mk non-thread dir");

        let runs = store.list_all_snapshots().expect("list all snapshots");
        assert_eq!(
            runs.len(),
            2,
            "both threads' snapshots, skipping non-thread dirs"
        );
        assert_eq!(runs[0].id, run_a.id, "sorted by run id across threads");
        assert_eq!(runs[1].id, run_b.id);
    }

    /// A run's permission profile round-trips through its sibling
    /// `*.permissions.json` record; a missing profile reads as `None`.
    #[test]
    fn write_then_read_run_permissions_round_trips() {
        use std::collections::BTreeSet;

        use crate::internal::ai::agent_run::permission::{AgentPermissionProfile, ApprovalRouting};

        let (_temp, store) = store();
        let thread_id = Uuid::new_v4();
        let run_id = AgentRunId::new();
        let mut allowed = BTreeSet::new();
        allowed.insert("read_file".to_string());
        let profile = AgentPermissionProfile {
            allowed_tools: allowed,
            denied_tools: BTreeSet::new(),
            allowed_source_slugs: BTreeSet::new(),
            approval_routing: ApprovalRouting::Layer1Human,
            may_spawn_sub_agents: false,
        };

        store
            .write_run_permissions(thread_id, run_id, &profile)
            .expect("write permissions");
        let back = store
            .read_run_permissions(thread_id, run_id)
            .expect("read permissions")
            .expect("profile must be present");
        assert!(back.allowed_tools.contains("read_file"));
        assert!(!back.may_spawn_sub_agents);

        assert!(
            store
                .read_run_permissions(Uuid::new_v4(), AgentRunId::new())
                .expect("missing profile reads Ok(None)")
                .is_none(),
        );
    }

    /// A missing sessions root lists as empty (no runs ever persisted).
    #[test]
    fn list_all_snapshots_missing_root_yields_empty() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = AgentRunEventStore::new(temp.path().join("nonexistent-sessions"));
        assert!(
            store
                .list_all_snapshots()
                .expect("missing root → empty")
                .is_empty()
        );
    }

    /// A missing `agents/` directory (no runs ever persisted) lists as empty.
    #[test]
    fn list_snapshots_missing_dir_yields_empty() {
        let (_temp, store) = store();
        let runs = store
            .list_snapshots(Uuid::new_v4())
            .expect("missing dir must list as empty, not error");
        assert!(runs.is_empty());
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
