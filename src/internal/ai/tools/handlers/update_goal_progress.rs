//! Handler for the `update_goal_progress` tool.
//!
//! Used inside Goal mode (`docs/development/commands/_general.md` lines 540-560,
//! 1808). The model invokes this between `submit_goal_complete`
//! attempts to record progress without claiming completion. The
//! supervisor (P6.3) reads the parsed payload from the just-finished
//! `ToolLoopTurn` and turns each successful call into a
//! [`GoalEvent::ProgressRecorded`] envelope (see
//! [`crate::internal::ai::goal::GoalSupervisor::step`]).
//!
//! The handler itself is intentionally minimal — it only validates
//! the payload shape and mirrors the parsed record back as
//! structured JSON. Goal-spec-aware checks (e.g. "claimed criterion
//! ids exist in the spec") live in the verifier (P6.2); the
//! continuation prompt builder (P6.3) is the layer that surfaces
//! gaps to the model on the next turn.
//!
//! # Why a thin validator
//!
//! The doc forbids stuffing raw transcripts back into Goal events
//! (opencode.md:619-625). Keeping the handler thin means the
//! supervisor — not the tool — owns the canonical envelope shape.
//! This also makes the tool replayable: a handler that did
//! side-effecting work (writing events, mutating state) would break
//! the "tools land in JSONL exactly once" invariant.
//!
//! [`GoalEvent::ProgressRecorded`]: crate::internal::ai::goal::GoalEvent::ProgressRecorded

use async_trait::async_trait;

use super::parse_arguments;
use crate::internal::ai::{
    goal::GoalProgressRecord,
    tools::{
        ToolResult,
        context::{ToolInvocation, ToolKind, ToolOutput, ToolPayload},
        error::ToolError,
        registry::ToolHandler,
        spec::ToolSpec,
    },
};

/// Validator + structured terminator for `update_goal_progress`.
///
/// The handler does not write to the Goal event stream directly —
/// the supervisor (P6.3) reads the just-finished tool call and
/// emits `GoalEvent::ProgressRecorded`. Treat this tool as a
/// structured progress note with shape validation, not as an
/// additional persistence channel.
pub struct UpdateGoalProgressHandler;

#[async_trait]
impl ToolHandler for UpdateGoalProgressHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> ToolResult<ToolOutput> {
        let arguments = match invocation.payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(ToolError::IncompatiblePayload(
                    "update_goal_progress requires Function payload".into(),
                ));
            }
        };

        let record: GoalProgressRecord = parse_arguments(&arguments)?;
        validate(&record)?;

        Ok(ToolOutput::success(format!(
            "Progress recorded: {} (newly satisfied criteria: {}; evidence refs: {}; next steps: {})",
            trimmed_excerpt(&record.summary),
            record.completed_criteria.len(),
            record.evidence_refs.len(),
            record.next_steps.len(),
        )))
    }

    fn schema(&self) -> ToolSpec {
        ToolSpec::update_goal_progress()
    }
}

fn validate(record: &GoalProgressRecord) -> ToolResult<()> {
    if record.summary.trim().is_empty() {
        return Err(ToolError::InvalidArguments(
            "update_goal_progress requires a non-empty summary".to_string(),
        ));
    }
    for (idx, id) in record.completed_criteria.iter().enumerate() {
        if id.trim().is_empty() {
            return Err(ToolError::InvalidArguments(format!(
                "update_goal_progress completed_criteria[{idx}] must not be blank"
            )));
        }
    }
    Ok(())
}

/// Bound the summary echoed back to the model so a runaway
/// progress note doesn't bloat the audit-log line. The doc forbids
/// raw transcripts in Goal events; this handler's reply is one of
/// the boundaries that protects that invariant.
fn trimmed_excerpt(text: &str) -> String {
    const MAX_EXCERPT_BYTES: usize = 240;
    let trimmed = text.trim();
    if trimmed.len() <= MAX_EXCERPT_BYTES {
        return trimmed.to_string();
    }
    let mut end = MAX_EXCERPT_BYTES;
    while end > 0 && !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &trimmed[..end])
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn make_invocation(json: &str) -> ToolInvocation {
        ToolInvocation::new(
            "call-update-progress-1",
            "update_goal_progress",
            ToolPayload::Function {
                arguments: json.to_string(),
            },
            PathBuf::from("/tmp"),
        )
    }

    #[tokio::test]
    async fn accepts_minimal_payload_with_summary_only() {
        let inv = make_invocation(r#"{"summary": "ran cargo check; clean"}"#);
        let output = UpdateGoalProgressHandler.handle(inv).await.unwrap();
        assert!(output.is_success());
        assert!(
            output
                .as_text()
                .is_some_and(|t| t.contains("Progress recorded"))
        );
    }

    #[tokio::test]
    async fn accepts_full_payload_with_evidence_and_next_steps() {
        let inv = make_invocation(
            r#"{
                "summary": "implemented the feature; running verification next",
                "completed_criteria": ["compiles"],
                "evidence_refs": [
                    {
                        "criterion_id": "compiles",
                        "target": {"kind": "tool_call", "call_id": "tc-7"},
                        "description": "cargo check returned 0"
                    }
                ],
                "next_steps": ["run tests", "update changelog"]
            }"#,
        );
        let output = UpdateGoalProgressHandler.handle(inv).await.unwrap();
        assert!(output.is_success());
        let text = output.as_text().unwrap_or_default();
        assert!(text.contains("newly satisfied criteria: 1"));
        assert!(text.contains("evidence refs: 1"));
        assert!(text.contains("next steps: 2"));
    }

    #[tokio::test]
    async fn rejects_empty_summary() {
        let inv = make_invocation(r#"{"summary": "  "}"#);
        let result = UpdateGoalProgressHandler.handle(inv).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn rejects_blank_completed_criterion_id() {
        let inv = make_invocation(r#"{"summary": "x", "completed_criteria": ["compiles", "  "]}"#);
        let result = UpdateGoalProgressHandler.handle(inv).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn rejects_non_function_payload() {
        let inv = ToolInvocation::new(
            "call-1",
            "update_goal_progress",
            ToolPayload::Custom {
                input: "not a function payload".to_string(),
            },
            PathBuf::from("/tmp"),
        );
        let err = UpdateGoalProgressHandler.handle(inv).await.unwrap_err();
        assert!(matches!(err, ToolError::IncompatiblePayload(_)));
    }

    #[test]
    fn exposes_expected_schema() {
        let spec = UpdateGoalProgressHandler.schema();
        assert_eq!(spec.function.name, "update_goal_progress");
    }
}
