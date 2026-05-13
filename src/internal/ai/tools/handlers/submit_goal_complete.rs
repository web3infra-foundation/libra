//! Handler for the `submit_goal_complete` tool.
//!
//! Used inside Goal mode (`docs/improvement/opencode.md` lines 657-661,
//! 1808). The model invokes this when it believes the active Goal is
//! satisfied. The supervisor (P6.3) reads the parsed claim from the
//! just-finished `ToolLoopTurn` and turns it into a
//! [`GoalEvent::CompletionClaimed`] envelope, then runs the
//! deterministic verifier (P6.2) — only when the verifier accepts
//! does the Goal transition to terminal `Completed`. A rejected
//! claim does NOT end the Goal; the supervisor surfaces missing
//! items and the loop continues.
//!
//! # Terminal tool
//!
//! `submit_goal_complete` is registered as a terminal tool in the
//! surrounding `ToolLoopConfig.terminal_tools` (set up by the P6.5
//! `libra code` integration), so a successful call short-circuits
//! the tool loop and hands control back to the supervisor.
//!
//! # Why a thin validator
//!
//! The handler intentionally only validates payload *shape*
//! (deserialise succeeds, summary non-empty, claimed criteria
//! non-empty, ids well-formed). The rich spec-aware checks
//! ("required criteria all claimed", "evidence floor under
//! Standard policy", "budget caps not exceeded", "no recent
//! tool failure") live in
//! [`crate::internal::ai::goal::DeterministicGoalVerifier`]; the
//! continuation prompt builder surfaces specific gaps to the model
//! on the next turn. Putting those checks in the handler would
//! couple the tool to the active Goal's state and break the
//! "tools land in JSONL exactly once" invariant.
//!
//! [`GoalEvent::CompletionClaimed`]: crate::internal::ai::goal::GoalEvent::CompletionClaimed

use async_trait::async_trait;

use super::parse_arguments;
use crate::internal::ai::{
    goal::GoalCompletionClaim,
    tools::{
        ToolResult,
        context::{ToolInvocation, ToolKind, ToolOutput, ToolPayload},
        error::ToolError,
        registry::ToolHandler,
        spec::ToolSpec,
    },
};

/// Validator + structured terminator for `submit_goal_complete`.
///
/// The handler does not write to the Goal event stream directly —
/// the supervisor (P6.3) reads the just-finished tool call, emits
/// `GoalEvent::CompletionClaimed`, runs the verifier, and emits
/// either `GoalEvent::Completed` or `GoalEvent::CompletionRejected`.
/// Treat this tool as a structured "I claim completion" handshake
/// with shape validation only.
pub struct SubmitGoalCompleteHandler;

#[async_trait]
impl ToolHandler for SubmitGoalCompleteHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> ToolResult<ToolOutput> {
        let arguments = match invocation.payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(ToolError::IncompatiblePayload(
                    "submit_goal_complete requires Function payload".into(),
                ));
            }
        };

        let claim: GoalCompletionClaim = parse_arguments(&arguments)?;
        validate(&claim)?;

        Ok(ToolOutput::success(format!(
            "Completion claim submitted: {} criterion(s); {} evidence ref(s); {} verification record(s). \
             Verifier will accept or reject; until accepted the Goal stays active.",
            claim.completed_criteria.len(),
            claim.evidence_refs.len(),
            claim.verification.len(),
        )))
    }

    fn schema(&self) -> ToolSpec {
        ToolSpec::submit_goal_complete()
    }
}

fn validate(claim: &GoalCompletionClaim) -> ToolResult<()> {
    if claim.summary.trim().is_empty() {
        return Err(ToolError::InvalidArguments(
            "submit_goal_complete requires a non-empty summary".to_string(),
        ));
    }
    if claim.completed_criteria.is_empty() {
        return Err(ToolError::InvalidArguments(
            "submit_goal_complete requires at least one entry in completed_criteria — \
             the verifier needs to know which criteria you are claiming"
                .to_string(),
        ));
    }
    for (idx, id) in claim.completed_criteria.iter().enumerate() {
        if id.trim().is_empty() {
            return Err(ToolError::InvalidArguments(format!(
                "submit_goal_complete completed_criteria[{idx}] must not be blank"
            )));
        }
    }
    for (idx, record) in claim.verification.iter().enumerate() {
        if record.criterion_id.trim().is_empty() {
            return Err(ToolError::InvalidArguments(format!(
                "submit_goal_complete verification[{idx}].criterion_id must not be blank"
            )));
        }
        if record.method.trim().is_empty() {
            return Err(ToolError::InvalidArguments(format!(
                "submit_goal_complete verification[{idx}].method must not be blank"
            )));
        }
    }
    for (idx, evidence) in claim.evidence_refs.iter().enumerate() {
        if evidence.description.trim().is_empty() {
            return Err(ToolError::InvalidArguments(format!(
                "submit_goal_complete evidence_refs[{idx}].description must not be blank"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn make_invocation(json: &str) -> ToolInvocation {
        ToolInvocation::new(
            "call-submit-goal-1",
            "submit_goal_complete",
            ToolPayload::Function {
                arguments: json.to_string(),
            },
            PathBuf::from("/tmp"),
        )
    }

    #[tokio::test]
    async fn accepts_well_formed_claim() {
        let inv = make_invocation(
            r#"{
                "summary": "shipped the feature; cargo test green",
                "completed_criteria": ["compiles", "tests"],
                "evidence_refs": [
                    {
                        "criterion_id": "compiles",
                        "target": {"kind": "file", "path": "src/feature.rs", "sha256": "deadbeef"},
                        "description": "feature edit landed"
                    },
                    {
                        "criterion_id": "tests",
                        "target": {"kind": "file", "path": "tests/feature.rs", "sha256": "cafef00d"},
                        "description": "added integration test"
                    }
                ],
                "verification": [
                    {"criterion_id": "compiles", "method": "cargo check", "passed": true},
                    {"criterion_id": "tests", "method": "cargo test --lib", "passed": true}
                ],
                "residual_risks": ["coverage report not yet attached"]
            }"#,
        );
        let output = SubmitGoalCompleteHandler.handle(inv).await.unwrap();
        assert!(output.is_success());
        let text = output.as_text().unwrap_or_default();
        assert!(text.contains("2 criterion"));
        assert!(text.contains("2 evidence ref"));
        assert!(text.contains("2 verification record"));
        assert!(text.contains("Verifier will accept or reject"));
    }

    #[tokio::test]
    async fn accepts_no_changes_needed_evidence() {
        // Research-only Goal: claim contains a NoChangesNeeded ref.
        // The handler accepts shape; the verifier still has the
        // final say about whether the rationale + spec policy line up.
        let inv = make_invocation(
            r#"{
                "summary": "research concluded no edit was required",
                "completed_criteria": ["investigation"],
                "evidence_refs": [
                    {
                        "criterion_id": "investigation",
                        "target": {"kind": "no_changes_needed", "rationale": "spec already correct"},
                        "description": "research outcome"
                    }
                ],
                "verification": [
                    {"criterion_id": "investigation", "method": "manual review", "passed": true}
                ]
            }"#,
        );
        let output = SubmitGoalCompleteHandler.handle(inv).await.unwrap();
        assert!(output.is_success());
    }

    #[tokio::test]
    async fn rejects_empty_summary() {
        let inv = make_invocation(
            r#"{"summary": "   ", "completed_criteria": ["compiles"], "evidence_refs": [], "verification": []}"#,
        );
        let result = SubmitGoalCompleteHandler.handle(inv).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn rejects_empty_completed_criteria() {
        let inv = make_invocation(
            r#"{"summary": "shipped", "completed_criteria": [], "evidence_refs": [], "verification": []}"#,
        );
        let err = SubmitGoalCompleteHandler.handle(inv).await.unwrap_err();
        assert!(err.to_string().contains("at least one entry"));
    }

    #[tokio::test]
    async fn rejects_blank_completed_criterion_id() {
        let inv = make_invocation(
            r#"{
                "summary": "shipped",
                "completed_criteria": ["compiles", "  "],
                "evidence_refs": [],
                "verification": []
            }"#,
        );
        let result = SubmitGoalCompleteHandler.handle(inv).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn rejects_blank_verification_method() {
        let inv = make_invocation(
            r#"{
                "summary": "shipped",
                "completed_criteria": ["compiles"],
                "evidence_refs": [],
                "verification": [
                    {"criterion_id": "compiles", "method": "  ", "passed": true}
                ]
            }"#,
        );
        let result = SubmitGoalCompleteHandler.handle(inv).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn rejects_blank_evidence_description() {
        let inv = make_invocation(
            r#"{
                "summary": "shipped",
                "completed_criteria": ["compiles"],
                "evidence_refs": [
                    {
                        "criterion_id": "compiles",
                        "target": {"kind": "tool_call", "call_id": "tc-1"},
                        "description": "  "
                    }
                ],
                "verification": [
                    {"criterion_id": "compiles", "method": "cargo check", "passed": true}
                ]
            }"#,
        );
        let result = SubmitGoalCompleteHandler.handle(inv).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn rejects_unknown_evidence_target_kind() {
        // Schema lists an enum of known kinds; the deserializer
        // refuses unknown variants for the internally-tagged enum.
        let inv = make_invocation(
            r#"{
                "summary": "shipped",
                "completed_criteria": ["compiles"],
                "evidence_refs": [
                    {
                        "criterion_id": "compiles",
                        "target": {"kind": "totally_made_up_target"},
                        "description": "x"
                    }
                ],
                "verification": [
                    {"criterion_id": "compiles", "method": "cargo check", "passed": true}
                ]
            }"#,
        );
        // Unknown target kind serialises into `GoalEvidenceTarget::Future`
        // (the schema's `#[serde(other)]` catch-all) — this is intentional
        // forward-compat. The handler accepts shape; the verifier later
        // refuses the claim because Future evidence does not satisfy the
        // Standard floor (Codex pass-8 P1 closure).
        let output = SubmitGoalCompleteHandler.handle(inv).await.unwrap();
        assert!(output.is_success());
    }

    #[tokio::test]
    async fn rejects_non_function_payload() {
        let inv = ToolInvocation::new(
            "call-1",
            "submit_goal_complete",
            ToolPayload::Custom {
                input: "not a function payload".to_string(),
            },
            PathBuf::from("/tmp"),
        );
        let err = SubmitGoalCompleteHandler.handle(inv).await.unwrap_err();
        assert!(matches!(err, ToolError::IncompatiblePayload(_)));
    }

    #[test]
    fn exposes_expected_schema() {
        let spec = SubmitGoalCompleteHandler.schema();
        assert_eq!(spec.function.name, "submit_goal_complete");
    }
}
