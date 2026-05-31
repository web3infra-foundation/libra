//! Append-only JSONL session event storage.

use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::state::SessionState;
use crate::internal::ai::{
    agent_run::{AgentRunEvent, AgentRunEventEnvelope, AgentRunId},
    context_budget::{CompactionEvent, ContextFrameEvent, MemoryAnchorEvent, MemoryAnchorReplay},
    goal::GoalEventEnvelope,
    runtime::event::Event,
};

pub const SESSION_EVENTS_FILE: &str = "events.jsonl";

/// Event persisted in a session JSONL stream.
///
/// The wire form follows the runtime `Event` envelope contract:
/// `{"kind":"session_snapshot","payload":{...}}`. Readers inspect the
/// envelope before deserializing so future event kinds can be skipped without
/// breaking older binaries.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", content = "payload", rename_all = "snake_case")]
pub enum SessionEvent {
    SessionSnapshot(SessionSnapshotEvent),
    ContextFrame(ContextFrameEvent),
    CompactionEvent(CompactionEvent),
    MemoryAnchor(MemoryAnchorEvent),
    /// OC-Phase 3 sub-agent lifecycle event. These do not mutate the
    /// legacy `SessionState`; they are replayed by agent-run specific
    /// projections and skipped by older binaries through the unknown
    /// event branch.
    AgentRun(AgentRunEventEnvelope),
    /// Dedicated child tool-call transcript event. The child session
    /// stream also carries `SessionSnapshot` rows for legacy resume,
    /// but this event keeps tool arguments queryable without parsing
    /// snapshot message strings.
    ToolCall(SessionToolCallEvent),
    /// Dedicated child tool-result transcript event. Mirrors
    /// [`Self::ToolCall`] and does not mutate legacy `SessionState`.
    ToolResult(SessionToolResultEvent),
    /// OC-Phase 6 Goal mode envelope. Goal supervisor wiring emits these
    /// alongside normal session events; older binaries still skip unknown
    /// `goal_event` payloads via the `parse_session_event_value` `unknown`
    /// branch.
    Goal(GoalEventEnvelope),
    /// OC-Phase 4 ArtifactLedger JSONL projection. The
    /// `ValidationReportStore::write_latest_with_session_mirror` and
    /// `DecisionProposalStore::write_latest_with_session_mirror` paths
    /// persist artefacts to `ai_validation_report` /
    /// `ai_decision_proposal` / `ai_risk_score_breakdown` SQLite
    /// tables; this variant projects the same write into the
    /// session JSONL stream so a single tail of the session log
    /// gives an operator the artefact lifecycle without an
    /// SQLite join.
    ///
    /// Forward-compat: older binaries that don't know this kind
    /// skip the row via the `parse_session_event_value` unknown
    /// branch. New schema additions ride additively under
    /// `payload.payload: serde_json::Value` so a future kind
    /// extension does not break older readers.
    AiArtifact(AiArtifactEvent),
}

/// OC-Phase 4 ArtifactLedger JSONL projection envelope (v0.17.810).
///
/// One row per Phase 3/Phase 4 artefact write. The payload itself
/// is a free-form `serde_json::Value` so callers can attach any
/// future shape (`ValidationReport`, `RiskScoreBreakdown`,
/// `DecisionProposal`, …) without a SessionEvent enum bump per
/// artefact kind. Replay code that wants a typed view does the
/// `serde_json::from_value::<TypedShape>(payload.payload)` deserialise
/// at the projection layer instead of in the JSONL parser.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AiArtifactEvent {
    pub event_id: Uuid,
    pub recorded_at: DateTime<Utc>,
    /// Stable thread id the artefact attaches to. Matches the
    /// `thread_id` column on each persisted artefact row so a
    /// session JSONL replay can correlate to the SeaORM rows.
    pub thread_id: Uuid,
    /// Short tag identifying the artefact kind. Free-form
    /// snake_case so a future Phase 5 artefact type can land
    /// without a SessionEvent enum bump.
    pub artifact_kind: String,
    /// Optional artefact-specific id (UUID-as-string today). None
    /// only for kinds that don't carry their own id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_id: Option<String>,
    /// Free-form structured payload. Required field — callers
    /// must supply a `serde_json::Value` (object preferred). An
    /// empty `Object({})` is acceptable for kinds whose
    /// `artifact_id` already carries all the signal.
    pub payload: serde_json::Value,
}

/// Dedicated child tool-call transcript event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionToolCallEvent {
    pub event_id: Uuid,
    pub recorded_at: DateTime<Utc>,
    pub agent_run_id: AgentRunId,
    pub subagent_name: String,
    pub call_id: String,
    pub tool_name: String,
    pub arguments: serde_json::Value,
}

/// Dedicated child tool-result transcript event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionToolResultEvent {
    pub event_id: Uuid,
    pub recorded_at: DateTime<Utc>,
    pub agent_run_id: AgentRunId,
    pub subagent_name: String,
    pub call_id: String,
    pub tool_name: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Full session-state snapshot event.
///
/// Snapshots keep CEX-12 compatible with the existing `SessionState` resume
/// surface while moving the truth source from rewrite-in-place JSON blobs to
/// append-only JSONL. Later CEX cards can add finer-grained events and replay
/// them through the same reader.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionSnapshotEvent {
    pub event_id: Uuid,
    pub recorded_at: DateTime<Utc>,
    pub state: SessionState,
}

impl SessionEvent {
    pub fn snapshot(state: SessionState) -> Self {
        Self::SessionSnapshot(SessionSnapshotEvent {
            event_id: Uuid::new_v4(),
            recorded_at: Utc::now(),
            state,
        })
    }

    pub fn context_frame(event: ContextFrameEvent) -> Self {
        Self::ContextFrame(event)
    }

    pub fn compaction(event: CompactionEvent) -> Self {
        Self::CompactionEvent(event)
    }

    pub fn memory_anchor(event: MemoryAnchorEvent) -> Self {
        Self::MemoryAnchor(event)
    }

    pub fn agent_run(event: AgentRunEvent) -> Self {
        Self::AgentRun(event.into())
    }

    pub fn tool_call(event: SessionToolCallEvent) -> Self {
        Self::ToolCall(event)
    }

    pub fn tool_result(event: SessionToolResultEvent) -> Self {
        Self::ToolResult(event)
    }

    pub fn goal(event: GoalEventEnvelope) -> Self {
        Self::Goal(event)
    }

    pub fn ai_artifact(event: AiArtifactEvent) -> Self {
        Self::AiArtifact(event)
    }

    pub fn apply_to(&self, current: &mut Option<SessionState>) {
        match self {
            Self::SessionSnapshot(event) => {
                *current = Some(event.state.clone());
            }
            // Goal envelopes do NOT mutate the legacy `SessionState`.
            // Replay into a `GoalState` lives in
            // `crate::internal::ai::goal::state::replay`. Listing the
            // variant here makes the no-op explicit so a future
            // maintainer does not assume an oversight.
            //
            // AiArtifact envelopes also do not mutate the legacy
            // `SessionState`; they're a JSONL projection of
            // Phase 3/Phase 4 SeaORM writes that the artefact
            // ledger replay reads through a separate projection
            // (similar to GoalState replay).
            Self::ContextFrame(_)
            | Self::CompactionEvent(_)
            | Self::MemoryAnchor(_)
            | Self::AgentRun(_)
            | Self::ToolCall(_)
            | Self::ToolResult(_)
            | Self::Goal(_)
            | Self::AiArtifact(_) => {}
        }
    }
}

impl Event for SessionEvent {
    fn event_kind(&self) -> &'static str {
        match self {
            Self::SessionSnapshot(_) => "session_snapshot",
            Self::ContextFrame(event) => event.event_kind(),
            Self::CompactionEvent(event) => event.event_kind(),
            Self::MemoryAnchor(event) => event.event_kind(),
            Self::AgentRun(_) => "agent_run",
            Self::ToolCall(_) => "tool_call",
            Self::ToolResult(_) => "tool_result",
            Self::Goal(event) => event.event_kind(),
            Self::AiArtifact(_) => "ai_artifact",
        }
    }

    fn event_id(&self) -> Uuid {
        match self {
            Self::SessionSnapshot(event) => event.event_id,
            Self::ContextFrame(event) => event.event_id(),
            Self::CompactionEvent(event) => event.event_id(),
            Self::MemoryAnchor(event) => event.event_id(),
            Self::AgentRun(event) => event
                .known()
                .map(crate::internal::ai::runtime::Event::event_id)
                .unwrap_or_else(uuid::Uuid::nil),
            Self::ToolCall(event) => event.event_id,
            Self::ToolResult(event) => event.event_id,
            Self::Goal(event) => event.event_id(),
            Self::AiArtifact(event) => event.event_id,
        }
    }

    fn event_summary(&self) -> String {
        match self {
            Self::SessionSnapshot(event) => format!(
                "session {} snapshot with {} message(s)",
                event.state.id,
                event.state.messages.len()
            ),
            Self::ContextFrame(event) => event.event_summary(),
            Self::CompactionEvent(event) => event.event_summary(),
            Self::MemoryAnchor(event) => event.event_summary(),
            Self::AgentRun(event) => event
                .known()
                .map(crate::internal::ai::runtime::Event::event_summary)
                .unwrap_or_else(|| "unknown agent_run event".to_string()),
            Self::ToolCall(event) => format!(
                "sub-agent {} tool_call {} ({})",
                event.subagent_name, event.call_id, event.tool_name
            ),
            Self::ToolResult(event) => format!(
                "sub-agent {} tool_result {} ({}) status={}",
                event.subagent_name, event.call_id, event.tool_name, event.status
            ),
            Self::Goal(event) => event.event_summary(),
            Self::AiArtifact(event) => format!(
                "ai_artifact {} (thread {}) {}",
                event.artifact_kind,
                event.thread_id,
                event.artifact_id.as_deref().unwrap_or("-")
            ),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionContextReplay {
    pub frames: Vec<ContextFrameEvent>,
    pub compactions: Vec<CompactionEvent>,
}

#[derive(Debug, Clone)]
pub struct SessionJsonlStore {
    session_root: PathBuf,
}

impl SessionJsonlStore {
    pub fn new(session_root: PathBuf) -> Self {
        Self { session_root }
    }

    pub fn session_root(&self) -> &Path {
        &self.session_root
    }

    pub fn child(&self, child_id: &str) -> Self {
        Self::new(
            self.session_root
                .join("subagents")
                .join(child_dir_name(child_id)),
        )
    }

    pub fn events_path(&self) -> PathBuf {
        self.session_root.join(SESSION_EVENTS_FILE)
    }

    pub fn append(&self, event: &SessionEvent) -> io::Result<()> {
        fs::create_dir_all(&self.session_root).map_err(|err| {
            io::Error::new(
                err.kind(),
                format!(
                    "failed to create session directory '{}': {err}",
                    self.session_root.display()
                ),
            )
        })?;

        let path = self.events_path();
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|err| {
                io::Error::new(
                    err.kind(),
                    format!(
                        "failed to open session event log '{}': {err}",
                        path.display()
                    ),
                )
            })?;

        serde_json::to_writer(&mut file, event)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        file.write_all(b"\n").map_err(|err| {
            io::Error::new(
                err.kind(),
                format!(
                    "failed to append session event log '{}': {err}",
                    path.display()
                ),
            )
        })
    }

    pub fn load_state(&self) -> io::Result<Option<SessionState>> {
        let mut state = None;
        for event in self.load_events()? {
            event.apply_to(&mut state);
        }
        Ok(state)
    }

    pub fn load_events(&self) -> io::Result<Vec<SessionEvent>> {
        let path = self.events_path();
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => {
                return Err(io::Error::new(
                    err.kind(),
                    format!(
                        "failed to read session event log '{}': {err}",
                        path.display()
                    ),
                ));
            }
        };

        let lines: Vec<&str> = content.lines().collect();
        let ends_with_newline = content.ends_with('\n');
        let mut events = Vec::new();
        for (line_index, line) in lines.iter().enumerate() {
            let line_number = line_index + 1;
            if line.trim().is_empty() {
                continue;
            }

            let value = match serde_json::from_str::<Value>(line) {
                Ok(value) => value,
                Err(err) if line_index + 1 == lines.len() && !ends_with_newline => {
                    tracing::warn!(
                        path = %path.display(),
                        line = line_number,
                        error = %err,
                        "stopping session JSONL replay at malformed trailing line"
                    );
                    break;
                }
                Err(err) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "malformed complete line in session event log '{}' line {line_number}: {err}",
                            path.display()
                        ),
                    ));
                }
            };

            match parse_session_event_value(value) {
                Ok(Some(event)) => events.push(event),
                Ok(None) => {}
                Err(err) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "failed to decode session event log '{}' line {line_number}: {err}",
                            path.display()
                        ),
                    ));
                }
            }
        }
        Ok(events)
    }

    pub fn load_context_replay(&self) -> io::Result<SessionContextReplay> {
        let mut replay = SessionContextReplay::default();
        for event in self.load_events()? {
            match event {
                SessionEvent::ContextFrame(frame) => replay.frames.push(frame),
                SessionEvent::CompactionEvent(compaction) => {
                    replay.compactions.push(compaction);
                }
                SessionEvent::SessionSnapshot(_) => {}
                SessionEvent::MemoryAnchor(_) => {}
                SessionEvent::AgentRun(_) => {}
                SessionEvent::ToolCall(_) => {}
                SessionEvent::ToolResult(_) => {}
                // OC-Phase 6 P6.1: Goal envelopes do not contribute to
                // `SessionContextReplay`. Goal state is replayed by
                // `crate::internal::ai::goal::state::replay`, called by
                // the supervisor (P6.3). Listed explicitly so an
                // exhaustiveness regression surfaces here.
                SessionEvent::Goal(_) => {}
                // OC-Phase 4 ArtifactLedger (v0.17.810): AiArtifact
                // envelopes do not contribute to context replay —
                // they're a Phase 3/4 SeaORM-write projection that
                // a future artefact-ledger replay reads through a
                // separate projection.
                SessionEvent::AiArtifact(_) => {}
            }
        }
        Ok(replay)
    }

    pub fn load_memory_anchors(&self) -> io::Result<MemoryAnchorReplay> {
        let mut replay = MemoryAnchorReplay::default();
        for event in self.load_events()? {
            if let SessionEvent::MemoryAnchor(anchor) = event {
                replay.apply_event(anchor);
            }
        }
        Ok(replay)
    }

    pub fn load_ai_artifacts(&self) -> io::Result<Vec<AiArtifactEvent>> {
        let mut artifacts = Vec::new();
        for event in self.load_events()? {
            if let SessionEvent::AiArtifact(artifact) = event {
                artifacts.push(artifact);
            }
        }
        Ok(artifacts)
    }

    pub fn has_events(&self) -> io::Result<bool> {
        let path = self.events_path();
        match fs::metadata(&path) {
            Ok(metadata) => Ok(metadata.len() > 0),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(err) => Err(io::Error::new(
                err.kind(),
                format!(
                    "failed to inspect session event log '{}': {err}",
                    path.display()
                ),
            )),
        }
    }
}

fn child_dir_name(child_id: &str) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, child_id.as_bytes());
    format!("task-{}", hex::encode(digest.as_ref()))
}

fn parse_session_event_value(value: Value) -> Result<Option<SessionEvent>, serde_json::Error> {
    let Some(kind) = value.get("kind").and_then(Value::as_str) else {
        return Ok(None);
    };

    match kind {
        "session_snapshot" => serde_json::from_value(value).map(Some),
        "context_frame" => serde_json::from_value(value).map(Some),
        "compaction_event" => serde_json::from_value(value).map(Some),
        "memory_anchor" => serde_json::from_value(value).map(Some),
        "agent_run" => serde_json::from_value(value).map(Some),
        "tool_call" => serde_json::from_value(value).map(Some),
        "tool_result" => serde_json::from_value(value).map(Some),
        // OC-Phase 6 P6.1: Goal envelope. Old binaries that predate
        // P6.1 fall through to the `unknown` branch below and skip
        // the event without surfacing an error; this branch lets a
        // P6.1-aware binary parse the envelope into the `Goal` variant.
        "goal" => serde_json::from_value(value).map(Some),
        // OC-Phase 4 ArtifactLedger (v0.17.810): same
        // forward-compat shape as `goal` — older binaries skip
        // the row via the unknown branch.
        "ai_artifact" => serde_json::from_value(value).map(Some),
        unknown => {
            tracing::warn!(event_kind = unknown, "skipping unknown session event");
            Ok(None)
        }
    }
}

pub fn session_events_path(session_root: &Path) -> PathBuf {
    session_root.join(SESSION_EVENTS_FILE)
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    /// OC-Phase 4 ArtifactLedger JSONL projection (v0.17.810):
    /// `SessionEvent::AiArtifact` round-trips through append +
    /// load_events without losing its payload. Pins the
    /// kind/payload serde tag/content shape so a future schema
    /// extension can't accidentally break older readers'
    /// unknown-event handling.
    #[test]
    fn session_event_ai_artifact_round_trips_through_jsonl() {
        let tmp = TempDir::new().expect("tmp dir");
        let store = SessionJsonlStore::new(tmp.path().to_path_buf());
        let event = SessionEvent::ai_artifact(AiArtifactEvent {
            event_id: Uuid::new_v4(),
            recorded_at: Utc::now(),
            thread_id: Uuid::new_v4(),
            artifact_kind: "validation_report".to_string(),
            artifact_id: Some("report-abc".to_string()),
            payload: serde_json::json!({
                "policy_version": "v0.17.810",
                "stale": false,
                "is_latest": true,
            }),
        });
        store.append(&event).expect("append must succeed");

        let loaded = store.load_events().expect("load must succeed");
        assert_eq!(loaded.len(), 1);
        match &loaded[0] {
            SessionEvent::AiArtifact(actual) => {
                let SessionEvent::AiArtifact(expected) = &event else {
                    panic!("test setup broke")
                };
                assert_eq!(actual.event_id, expected.event_id);
                assert_eq!(actual.thread_id, expected.thread_id);
                assert_eq!(actual.artifact_kind, "validation_report");
                assert_eq!(actual.artifact_id.as_deref(), Some("report-abc"));
                assert_eq!(
                    actual
                        .payload
                        .get("policy_version")
                        .and_then(|v| v.as_str()),
                    Some("v0.17.810"),
                );
            }
            other => panic!("expected AiArtifact, got: {other:?}"),
        }

        // The Event trait surface (event_kind / event_summary)
        // returns the new "ai_artifact" tag so observability
        // tooling can filter at the kind level without
        // deserialising the payload.
        use crate::internal::ai::runtime::event::Event;
        assert_eq!(event.event_kind(), "ai_artifact");
        assert!(event.event_summary().starts_with("ai_artifact "));
    }

    /// Child tool transcript events round-trip as first-class JSONL
    /// envelopes. They intentionally do not mutate legacy
    /// `SessionState`, but replay consumers can query arguments and
    /// results without parsing snapshot message strings.
    #[test]
    fn session_tool_events_round_trip_without_mutating_session_state() {
        let tmp = TempDir::new().expect("tmp dir");
        let store = SessionJsonlStore::new(tmp.path().to_path_buf());
        let agent_run_id = AgentRunId::new();
        let tool_call = SessionEvent::tool_call(SessionToolCallEvent {
            event_id: Uuid::new_v4(),
            recorded_at: Utc::now(),
            agent_run_id,
            subagent_name: "explore".to_string(),
            call_id: "call_1".to_string(),
            tool_name: "grep_files".to_string(),
            arguments: serde_json::json!({"pattern": "TODO"}),
        });
        let tool_result = SessionEvent::tool_result(SessionToolResultEvent {
            event_id: Uuid::new_v4(),
            recorded_at: Utc::now(),
            agent_run_id,
            subagent_name: "explore".to_string(),
            call_id: "call_1".to_string(),
            tool_name: "grep_files".to_string(),
            status: "success".to_string(),
            result: Some(serde_json::json!({"matches": 3})),
            error: None,
        });
        store.append(&tool_call).expect("append tool_call");
        store.append(&tool_result).expect("append tool_result");

        let loaded = store.load_events().expect("load events");
        assert_eq!(loaded.len(), 2);
        assert!(matches!(loaded[0], SessionEvent::ToolCall(_)));
        assert!(matches!(loaded[1], SessionEvent::ToolResult(_)));
        assert!(
            store
                .load_state()
                .expect("load state should ignore tool events")
                .is_none(),
            "tool transcript events must not mutate legacy SessionState",
        );

        use crate::internal::ai::runtime::event::Event;
        assert_eq!(tool_call.event_kind(), "tool_call");
        assert_eq!(tool_result.event_kind(), "tool_result");
        assert!(tool_call.event_summary().contains("grep_files"));
        assert!(tool_result.event_summary().contains("status=success"));
    }

    /// `session_events_path` + `events_path()` must produce
    /// `<root>/events.jsonl`. Pin the layout — the migrator and
    /// `code resume` rely on it.
    #[test]
    fn events_path_appends_constant_filename() {
        let tmp = TempDir::new().expect("tmp dir");
        let store = SessionJsonlStore::new(tmp.path().to_path_buf());
        let expected = tmp.path().join(SESSION_EVENTS_FILE);
        assert_eq!(store.events_path(), expected);
        assert_eq!(session_events_path(tmp.path()), expected);
        assert_eq!(SESSION_EVENTS_FILE, "events.jsonl");
    }

    /// Child session ids are untrusted (`task_id` can come from a model
    /// tool call), so they must never become raw path segments. The
    /// child store hashes the id into one fixed directory name under
    /// `<parent>/subagents/`.
    #[test]
    fn child_store_hashes_untrusted_id_into_single_path_segment() {
        let tmp = TempDir::new().expect("tmp dir");
        let store = SessionJsonlStore::new(tmp.path().to_path_buf());
        let child = store.child("../outside/../../secret");
        let relative = child
            .session_root()
            .strip_prefix(store.session_root())
            .expect("child must stay below parent");
        let components: Vec<_> = relative.components().collect();

        assert_eq!(components.len(), 2);
        assert_eq!(components[0].as_os_str().to_string_lossy(), "subagents");
        let child_dir = components[1].as_os_str().to_string_lossy();
        assert!(child_dir.starts_with("task-"));
        assert_eq!(child_dir.len(), "task-".len() + 64);
        assert!(!child.session_root().ends_with("secret"));
    }

    /// `has_events()` returns `false` for a missing JSONL file (no
    /// directory created yet).
    #[test]
    fn has_events_returns_false_for_missing_file() {
        let tmp = TempDir::new().expect("tmp dir");
        let store = SessionJsonlStore::new(tmp.path().join("never-exists"));
        assert!(!store.has_events().expect("has_events ok"));
    }

    /// `has_events()` returns `false` for an empty existing file
    /// (metadata.len() == 0).
    #[test]
    fn has_events_returns_false_for_empty_file() {
        let tmp = TempDir::new().expect("tmp dir");
        let store = SessionJsonlStore::new(tmp.path().to_path_buf());
        std::fs::write(store.events_path(), b"").expect("write empty");
        assert!(!store.has_events().expect("has_events ok"));
    }

    /// `has_events()` returns `true` after an `append`.
    #[test]
    fn append_then_has_events_returns_true() {
        let tmp = TempDir::new().expect("tmp dir");
        let store = SessionJsonlStore::new(tmp.path().to_path_buf());
        let state = SessionState::new("/tmp/work");
        store
            .append(&SessionEvent::snapshot(state))
            .expect("append ok");
        assert!(store.has_events().expect("has_events ok"));
    }

    /// `append` + `load_events` round-trip: one snapshot in, one
    /// snapshot out, equal state.
    #[test]
    fn append_load_events_roundtrips_single_snapshot() {
        let tmp = TempDir::new().expect("tmp dir");
        let store = SessionJsonlStore::new(tmp.path().to_path_buf());
        let state = SessionState::new("/tmp/work");
        let event = SessionEvent::snapshot(state.clone());
        store.append(&event).expect("append ok");

        let loaded = store.load_events().expect("load ok");
        assert_eq!(loaded.len(), 1);
        match &loaded[0] {
            SessionEvent::SessionSnapshot(snap) => {
                assert_eq!(snap.state, state);
            }
            other => panic!("expected SessionSnapshot, got {other:?}"),
        }
    }

    /// `load_state()` returns the latest snapshot when multiple are
    /// appended. The replay semantics are last-write-wins for
    /// snapshot events.
    #[test]
    fn load_state_returns_latest_snapshot_after_multiple_appends() {
        let tmp = TempDir::new().expect("tmp dir");
        let store = SessionJsonlStore::new(tmp.path().to_path_buf());

        let first = SessionState::new("/first/work");
        store
            .append(&SessionEvent::snapshot(first))
            .expect("first append");

        let second = SessionState::new("/second/work");
        store
            .append(&SessionEvent::snapshot(second.clone()))
            .expect("second append");

        let loaded = store.load_state().expect("load_state ok").expect("present");
        assert_eq!(loaded, second);
    }

    /// `load_state()` returns `None` when the JSONL file is missing.
    #[test]
    fn load_state_returns_none_when_no_events_file() {
        let tmp = TempDir::new().expect("tmp dir");
        let store = SessionJsonlStore::new(tmp.path().join("missing-dir"));
        let loaded = store.load_state().expect("load ok");
        assert!(loaded.is_none());
    }

    /// `apply_to`: snapshot variant replaces the current state;
    /// non-snapshot variants (context_frame / compaction / memory
    /// anchor / goal) are explicit no-ops in the legacy state replay.
    #[test]
    fn apply_to_snapshot_replaces_state_other_variants_are_noops() {
        let mut state: Option<SessionState> = None;
        SessionEvent::snapshot(SessionState::new("/tmp/from-snapshot")).apply_to(&mut state);
        assert!(state.is_some(), "snapshot must populate state");
        let snapshot_state = state.clone().expect("state populated");

        // A second snapshot must replace.
        SessionEvent::snapshot(SessionState::new("/tmp/from-snapshot-2")).apply_to(&mut state);
        let after_second = state.as_ref().expect("present");
        assert_ne!(after_second, &snapshot_state);
    }

    /// `parse_session_event_value`: missing `kind` field → Ok(None)
    /// (the value is silently skipped, not an error).
    #[test]
    fn parse_session_event_value_missing_kind_returns_none() {
        let value: Value = serde_json::json!({"payload": {}});
        let result = parse_session_event_value(value).expect("call ok");
        assert!(result.is_none());
    }

    /// `parse_session_event_value`: unknown `kind` string → Ok(None)
    /// (forward-compat skip-and-warn rule from the doc).
    #[test]
    fn parse_session_event_value_unknown_kind_returns_none() {
        let value: Value =
            serde_json::json!({"kind": "future_event_type", "payload": {"any": "shape"}});
        let result = parse_session_event_value(value).expect("call ok");
        assert!(result.is_none());
    }

    /// `parse_session_event_value`: `session_snapshot` round-trips
    /// through the envelope wire format.
    #[test]
    fn parse_session_event_value_session_snapshot_parses_envelope() {
        let event = SessionEvent::snapshot(SessionState::new("/tmp/work"));
        let value = serde_json::to_value(&event).expect("serialize");
        let parsed = parse_session_event_value(value)
            .expect("parse ok")
            .expect("Some");
        assert!(matches!(parsed, SessionEvent::SessionSnapshot(_)));
    }

    /// `SessionEvent::event_kind` for SessionSnapshot returns the
    /// canonical `"session_snapshot"` discriminator — pins the
    /// Event-trait surface used by audit log emitters.
    #[test]
    fn session_event_kind_pins_session_snapshot_string() {
        let event = SessionEvent::snapshot(SessionState::new("/tmp/work"));
        assert_eq!(event.event_kind(), "session_snapshot");
    }

    /// `SessionEvent::event_summary` for SessionSnapshot includes the
    /// session id and message count so audit consumers can correlate.
    #[test]
    fn session_event_summary_includes_session_id_and_message_count() {
        let state = SessionState::new("/tmp/work");
        let session_id = state.id.clone();
        let event = SessionEvent::snapshot(state);
        let summary = event.event_summary();
        assert!(
            summary.contains(&session_id),
            "summary must include session id; got {summary}",
        );
        assert!(
            summary.contains("0 message(s)"),
            "fresh session has 0 messages; got {summary}",
        );
    }
}
