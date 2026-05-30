//! Defensive guard for the agent.md 2026-05-04 follow-up item (g):
//! "agent_run 模块多个 enum 缺 `#[non_exhaustive]`."
//!
//! Public enums in `src/internal/ai/agent_run/` must stay marked
//! `#[non_exhaustive]` so downstream embedders cannot accidentally write
//! exhaustive matches that break when later Step 2 / Step 3 cards introduce
//! new variants. This guard scans each file for the expected enum name and
//! fails the build if a `pub enum` line is no longer immediately preceded by
//! `#[non_exhaustive]`.
//!
//! `AgentRunEvent` and `AgentRunEventEnvelope` are intentionally excluded:
//! the envelope already carries an explicit `Unknown(serde_json::Value)`
//! variant for forward compatibility (S2-INV-10), and `AgentRunEvent` is
//! consumed through that envelope at the wire boundary.

use std::{fs, path::PathBuf};

/// Tuple of (relative path under repo root, public enum name) the guard
/// inspects. Update alongside any rename of these enums.
const TARGETS: &[(&str, &str)] = &[
    ("src/internal/ai/agent_run/budget.rs", "BudgetDimension"),
    ("src/internal/ai/agent_run/decision.rs", "ReviewState"),
    ("src/internal/ai/agent_run/decision.rs", "RiskLevel"),
    ("src/internal/ai/agent_run/evidence.rs", "AgentType"),
    ("src/internal/ai/agent_run/mod.rs", "AnchorScope"),
    ("src/internal/ai/agent_run/run.rs", "AgentRunStatus"),
    ("src/internal/ai/agent_run/permission.rs", "ApprovalRouting"),
    ("src/internal/ai/agent_run/event.rs", "HookPhase"),
    ("src/internal/ai/agent_run/event.rs", "HookKind"),
    ("src/internal/ai/agent_run/event.rs", "HookFailureReason"),
    ("src/internal/ai/agent_run/event.rs", "PostToolReason"),
    ("src/internal/ai/agent_run/event.rs", "WorkspaceStrategy"),
    ("src/internal/ai/agent_run/event.rs", "CancellationReason"),
];

#[test]
fn agent_run_public_enums_are_non_exhaustive() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut offenders: Vec<String> = Vec::new();

    for (rel_path, enum_name) in TARGETS {
        let abs_path = manifest_dir.join(rel_path);
        let text = fs::read_to_string(&abs_path)
            .unwrap_or_else(|err| panic!("failed to read '{}': {err}", abs_path.display()));

        let needle = format!("pub enum {enum_name} ");
        let lines: Vec<&str> = text.lines().collect();
        let Some(enum_line_idx) = lines.iter().position(|line| line.starts_with(&needle)) else {
            offenders.push(format!(
                "{rel_path}: could not find `{needle}` — has the enum been renamed or moved?"
            ));
            continue;
        };

        // Walk backwards through derive / serde / cfg attributes and skip blank
        // lines; the very next attribute we expect to see is
        // `#[non_exhaustive]`. If we hit a non-attribute line first, the guard
        // fails.
        let mut found = false;
        for prior in lines[..enum_line_idx].iter().rev() {
            let trimmed = prior.trim();
            if trimmed.is_empty() {
                continue;
            }
            if trimmed == "#[non_exhaustive]" {
                found = true;
                break;
            }
            if trimmed.starts_with("#[") {
                continue;
            }
            // Anything else (doc comment, struct, fn) means the attribute is
            // missing from this enum's block.
            break;
        }

        if !found {
            offenders.push(format!(
                "{rel_path}: `pub enum {enum_name}` is missing `#[non_exhaustive]` — \
                 add the attribute to keep external matches forward-compatible",
            ));
        }
    }

    assert!(
        offenders.is_empty(),
        "agent_run public enums must carry `#[non_exhaustive]`:\n  - {}",
        offenders.join("\n  - "),
    );
}
