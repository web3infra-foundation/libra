use anyhow::{Result, bail};

use super::super::super::lifecycle::{
    LifecycleEvent, LifecycleEventKind, SessionHookEnvelope, build_lifecycle_event,
};

pub(super) const CLAUDE_LIFECYCLE_FALLBACK_EVENTS: &[&str] = &[
    "SessionStart",
    "Stop",
    "SessionStop",
    "SessionEnd",
    "Compaction",
];

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

    #[test]
    fn parser_rejects_unknown_hook() {
        let mut envelope = canonical_envelope();
        envelope.hook_event_name = "UnknownHook".to_string();
        assert!(parse_claude_hook_event("UnknownHook", &envelope).is_err());
    }
}
