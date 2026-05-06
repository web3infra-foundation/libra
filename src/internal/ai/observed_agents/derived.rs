//! Back-derive [`ToolCallRecord`] entries from a captured agent
//! session's `normalized_events` stream.
//!
//! Phase 4.3 (entire.md §14.4 item 3). The hook runtime appends a
//! projection-friendly summary of each lifecycle event onto
//! `SessionState.metadata["normalized_events"]`. Each entry carries
//! `kind`, `timestamp`, `tool_name`, `tool_use_id`, and a few `has_*`
//! flags. By pairing `pre_tool_use` with the matching `post_tool_use`
//! (joined by `tool_use_id`) we can reconstruct enough information to
//! emit Libra's own [`ToolCallRecord`] shape — the same struct the
//! orchestrator's `TaskResult` carries.
//!
//! This module is intentionally a pure function over `SessionState`:
//! tests can build fixtures in-memory without touching the SessionStore
//! JSONL, and the CLI dispatch path lives in
//! `command::agent::session::derive_tool_calls`.
//!
//! Mapping rules (deliberately conservative for v1):
//! - `pre_tool_use` and `post_tool_use` events with the same
//!   `tool_use_id` pair into one [`ToolCallRecord`]. The tool name
//!   wins from `pre_tool_use` (when both present); the action is
//!   pre/post-derived (`"call"` for both seen, `"invoke_only"` when
//!   only the pre side is present, `"observe_only"` when only post).
//! - `success` is `true` if a `post_tool_use` was observed (the
//!   captured stream does not surface the agent's own success/error
//!   payload at this level — adapters land that in v2).
//! - `paths_read` / `paths_written` / `arguments_json` / `summary` /
//!   `diffs` are left empty: the normalized stream stores `tool_name`
//!   only, and the raw envelope (`tool_input` / `tool_response`) does
//!   not survive into the projection. v2 adapters can re-parse the
//!   per-agent transcript to fill these in.

use serde_json::Value;

use crate::internal::ai::{orchestrator::types::ToolCallRecord, session::SessionState};

/// Walk `session.metadata["normalized_events"]` and return one
/// [`ToolCallRecord`] per logical tool call. Returns an empty vec when
/// the session has no normalized events or none of them are tool-use
/// events.
pub fn derive_tool_call_records(session: &SessionState) -> Vec<ToolCallRecord> {
    let Some(Value::Array(events)) = session.metadata.get("normalized_events") else {
        return Vec::new();
    };

    use std::collections::HashMap;

    /// Mutable accumulator keyed by tool_use_id (or, when missing, by
    /// the event's own index — that fallback ensures unkeyed events
    /// still produce a record rather than getting silently merged).
    #[derive(Default)]
    struct Accum {
        tool_name: Option<String>,
        saw_pre: bool,
        saw_post: bool,
    }

    let mut by_key: HashMap<String, Accum> = HashMap::new();
    // Preserve first-seen order for deterministic output. HashMap
    // doesn't, so we track keys separately.
    let mut order: Vec<String> = Vec::new();

    for (idx, event) in events.iter().enumerate() {
        let kind = event
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let is_pre = kind == "pre_tool_use";
        let is_post = kind == "post_tool_use";
        if !is_pre && !is_post {
            continue;
        }
        let key = event
            .get("tool_use_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("__unkeyed__:{idx}"));
        let entry = by_key.entry(key.clone()).or_insert_with(|| {
            order.push(key.clone());
            Accum::default()
        });
        if is_pre {
            entry.saw_pre = true;
        }
        if is_post {
            entry.saw_post = true;
        }
        if entry.tool_name.is_none()
            && let Some(name) = event.get("tool_name").and_then(Value::as_str)
        {
            entry.tool_name = Some(name.to_string());
        }
    }

    let mut out = Vec::with_capacity(order.len());
    for key in order {
        let accum = by_key.remove(&key).expect("key was inserted");
        let action = match (accum.saw_pre, accum.saw_post) {
            (true, true) => "call",
            (true, false) => "invoke_only",
            (false, true) => "observe_only",
            (false, false) => continue, // unreachable given the filter above
        };
        out.push(ToolCallRecord {
            tool_name: accum.tool_name.unwrap_or_default(),
            action: action.to_string(),
            arguments_json: None,
            paths_read: Vec::new(),
            paths_written: Vec::new(),
            success: accum.saw_post,
            summary: None,
            diffs: Vec::new(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn fixture_session(events: Vec<Value>) -> SessionState {
        let mut session = SessionState::new("/tmp/fixture");
        session
            .metadata
            .insert("normalized_events".to_string(), Value::Array(events));
        session
    }

    #[test]
    fn returns_empty_when_no_normalized_events_present() {
        let session = SessionState::new("/tmp/fixture");
        assert!(derive_tool_call_records(&session).is_empty());
    }

    #[test]
    fn returns_empty_when_normalized_events_has_no_tool_use_kinds() {
        let session = fixture_session(vec![
            json!({"kind": "session_start", "timestamp": "2026-05-05T10:00:00Z"}),
            json!({"kind": "user_prompt_submit", "timestamp": "2026-05-05T10:01:00Z"}),
        ]);
        assert!(derive_tool_call_records(&session).is_empty());
    }

    #[test]
    fn pairs_pre_and_post_by_tool_use_id() {
        let session = fixture_session(vec![
            json!({
                "kind": "pre_tool_use",
                "tool_use_id": "tu-001",
                "tool_name": "Read",
                "timestamp": "2026-05-05T10:00:00Z",
            }),
            json!({
                "kind": "post_tool_use",
                "tool_use_id": "tu-001",
                "timestamp": "2026-05-05T10:00:01Z",
            }),
        ]);
        let records = derive_tool_call_records(&session);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].tool_name, "Read");
        assert_eq!(records[0].action, "call");
        assert!(records[0].success);
    }

    #[test]
    fn pre_only_pair_marks_invoke_only_and_unsuccessful() {
        // A `pre_tool_use` with no matching post — the agent crashed
        // mid-call or the hook stream truncated. Surface as
        // `invoke_only` so the operator can see the call started.
        let session = fixture_session(vec![json!({
            "kind": "pre_tool_use",
            "tool_use_id": "tu-002",
            "tool_name": "Bash",
            "timestamp": "2026-05-05T10:00:00Z",
        })]);
        let records = derive_tool_call_records(&session);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].tool_name, "Bash");
        assert_eq!(records[0].action, "invoke_only");
        assert!(!records[0].success);
    }

    #[test]
    fn post_only_pair_marks_observe_only_and_succeeds() {
        // A `post_tool_use` with no matching pre — typically a hook
        // dropout. The post-side is enough to know the call ran.
        let session = fixture_session(vec![json!({
            "kind": "post_tool_use",
            "tool_use_id": "tu-003",
            "tool_name": "Edit",
            "timestamp": "2026-05-05T10:00:00Z",
        })]);
        let records = derive_tool_call_records(&session);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].tool_name, "Edit");
        assert_eq!(records[0].action, "observe_only");
        assert!(records[0].success);
    }

    #[test]
    fn unkeyed_events_each_become_their_own_record() {
        // Without `tool_use_id`, two events from the same logical
        // tool call would otherwise collapse together. The fallback
        // key uses the event index so unkeyed events stay distinct.
        let session = fixture_session(vec![
            json!({
                "kind": "pre_tool_use",
                "tool_name": "Bash",
                "timestamp": "2026-05-05T10:00:00Z",
            }),
            json!({
                "kind": "pre_tool_use",
                "tool_name": "Read",
                "timestamp": "2026-05-05T10:00:01Z",
            }),
        ]);
        let records = derive_tool_call_records(&session);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].tool_name, "Bash");
        assert_eq!(records[1].tool_name, "Read");
    }

    #[test]
    fn order_is_first_seen_event_order() {
        let session = fixture_session(vec![
            json!({
                "kind": "pre_tool_use",
                "tool_use_id": "tu-A",
                "tool_name": "First",
                "timestamp": "2026-05-05T10:00:00Z",
            }),
            json!({
                "kind": "pre_tool_use",
                "tool_use_id": "tu-B",
                "tool_name": "Second",
                "timestamp": "2026-05-05T10:00:01Z",
            }),
            json!({
                "kind": "post_tool_use",
                "tool_use_id": "tu-A",
                "timestamp": "2026-05-05T10:00:02Z",
            }),
        ]);
        let records = derive_tool_call_records(&session);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].tool_name, "First");
        assert_eq!(records[1].tool_name, "Second");
    }
}
