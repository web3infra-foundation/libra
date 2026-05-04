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
    context_budget::{CompactionEvent, ContextFrameEvent, MemoryAnchorEvent, MemoryAnchorReplay},
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

    pub fn apply_to(&self, current: &mut Option<SessionState>) {
        match self {
            Self::SessionSnapshot(event) => {
                *current = Some(event.state.clone());
            }
            Self::ContextFrame(_) | Self::CompactionEvent(_) | Self::MemoryAnchor(_) => {}
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
        }
    }

    fn event_id(&self) -> Uuid {
        match self {
            Self::SessionSnapshot(event) => event.event_id,
            Self::ContextFrame(event) => event.event_id(),
            Self::CompactionEvent(event) => event.event_id(),
            Self::MemoryAnchor(event) => event.event_id(),
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

fn parse_session_event_value(value: Value) -> Result<Option<SessionEvent>, serde_json::Error> {
    let Some(kind) = value.get("kind").and_then(Value::as_str) else {
        return Ok(None);
    };

    match kind {
        "session_snapshot" => serde_json::from_value(value).map(Some),
        "context_frame" => serde_json::from_value(value).map(Some),
        "compaction_event" => serde_json::from_value(value).map(Some),
        "memory_anchor" => serde_json::from_value(value).map(Some),
        unknown => {
            tracing::warn!(event_kind = unknown, "skipping unknown session event");
            Ok(None)
        }
    }
}

pub fn session_events_path(session_root: &Path) -> PathBuf {
    session_root.join(SESSION_EVENTS_FILE)
}
