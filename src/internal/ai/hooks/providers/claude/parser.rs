//! Claude Code hook envelope parser.
//!
//! Maps each Claude `hook_event_name` to a canonical [`LifecycleEventKind`] and
//! delegates the rest of the field extraction to [`build_lifecycle_event`]. The
//! provider's hook taxonomy is intentionally hard-coded here: silently
//! forwarding unknown events would let breaking upstream changes go undetected.

use anyhow::{Result, bail};

use super::super::super::lifecycle::{
    LifecycleEvent, LifecycleEventKind, SessionHookEnvelope, build_lifecycle_event,
};

/// Claude event names that should fall back to `session_id` when no canonical
/// identity field (event_id, request_id, …) is present in the payload.
pub(super) const CLAUDE_LIFECYCLE_FALLBACK_EVENTS: &[&str] = &[
    "SessionStart",
    "Stop",
    "SessionStop",
    "SessionEnd",
    "Compaction",
];

/// Translate a Claude hook event name into a canonical lifecycle event.
///
/// Functional scope: routes each known Claude hook (SessionStart,
/// UserPromptSubmit, PreToolUse/PostToolUse, Stop, ModelUpdate, Compaction,
/// SessionEnd) through [`build_lifecycle_event`], which extracts the standard
/// fields from `envelope.extra`.
///
/// Boundary conditions: unknown event names produce a hard error so that
/// upstream changes are surfaced immediately rather than silently dropped.
///
/// See: `tests::parser_maps_canonical_hooks`, `tests::parser_rejects_unknown_hook`.
pub(super) fn parse_claude_hook_event(
    hook_event_name: &str,
    envelope: &SessionHookEnvelope,
) -> Result<LifecycleEvent> {
    let kind = match hook_event_name {
        "SessionStart" => LifecycleEventKind::SessionStart,
        "UserPromptSubmit" => LifecycleEventKind::TurnStart,
        "PostToolUse" | "PreToolUse" => LifecycleEventKind::ToolUse,
        "Stop" | "SessionStop" => LifecycleEventKind::TurnEnd,
        "ModelUpdate" => LifecycleEventKind::ModelUpdate,
        "Compaction" => LifecycleEventKind::Compaction,
        "SessionEnd" => LifecycleEventKind::SessionEnd,
        other => bail!("unknown Claude Code hook event: '{other}'"),
    };
    Ok(build_lifecycle_event(kind, envelope))
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, Value};

    use super::*;

    fn canonical_envelope() -> SessionHookEnvelope {
        SessionHookEnvelope {
            hook_event_name: "SessionStart".to_string(),
            session_id: "s1".to_string(),
            cwd: "/tmp".to_string(),
            transcript_path: Some("/tmp/transcript.jsonl".to_string()),
            extra: {
                let mut map = Map::new();
                map.insert("prompt".to_string(), Value::String("hello".to_string()));
                map
            },
        }
    }

    // Scenario: the well-known Claude hook names map to their canonical kinds.
    #[test]
    fn parser_maps_canonical_hooks() {
        let envelope = canonical_envelope();
        let cases = [
            ("SessionStart", LifecycleEventKind::SessionStart),
            ("UserPromptSubmit", LifecycleEventKind::TurnStart),
            ("PostToolUse", LifecycleEventKind::ToolUse),
            ("Stop", LifecycleEventKind::TurnEnd),
            ("SessionEnd", LifecycleEventKind::SessionEnd),
        ];

        for (name, kind) in cases {
            let event = parse_claude_hook_event(name, &envelope).expect("parse should succeed");
            assert_eq!(event.kind, kind);
        }
    }

    // Scenario: a hook name not in the known set returns an error.
    #[test]
    fn parser_rejects_unknown_hook() {
        let mut envelope = canonical_envelope();
        envelope.hook_event_name = "UnknownHook".to_string();
        assert!(parse_claude_hook_event("UnknownHook", &envelope).is_err());
    }
}
