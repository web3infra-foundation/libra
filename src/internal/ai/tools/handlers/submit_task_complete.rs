//! Handler for the `submit_task_complete` tool.
//!
//! AI user story: implementation/analysis tasks need an explicit "I'm done"
//! signal so the orchestrator's tool loop converges deterministically. Without
//! it, agents may keep re-running verification commands until the loop hits
//! its max-turn cap and exits without a verdict — defeating Phase 4 review.
//!
//! `submit_task_complete` is registered as a terminal tool: a successful call
//! short-circuits the tool loop immediately and the structured arguments
//! become the task's final output. The handler itself only validates the
//! payload; persistence of the verdict is handled by the executor observer
//! pipeline that records every tool call.

use async_trait::async_trait;

use super::parse_arguments;
use crate::internal::ai::tools::{
    ToolResult,
    context::{
        SubmitTaskCompleteArgs, TaskCompleteResult, ToolInvocation, ToolKind, ToolOutput,
        ToolPayload,
    },
    error::ToolError,
    registry::ToolHandler,
    spec::ToolSpec,
};

/// Final-handshake handler for implementation/analysis task convergence.
///
/// The handler does not write to MCP storage directly — the tool-call
/// observer in the executor already captures arguments verbatim into the
/// task record. Treat this tool as a structured terminator with validation,
/// not as an additional persistence channel.
pub struct SubmitTaskCompleteHandler;

#[async_trait]
impl ToolHandler for SubmitTaskCompleteHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> ToolResult<ToolOutput> {
        let arguments = match invocation.payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(ToolError::IncompatiblePayload(
                    "submit_task_complete requires Function payload".into(),
                ));
            }
        };

        let args: SubmitTaskCompleteArgs = parse_arguments(&arguments)?;
        validate(&args)?;

        let label = match args.result {
            TaskCompleteResult::Pass => "pass",
            TaskCompleteResult::Fail => "fail",
            TaskCompleteResult::NoChangesNeeded => "no_changes_needed",
        };
        let evidence_count = args.evidence.len();
        Ok(ToolOutput::success(format!(
            "Task complete: {label} ({evidence_count} evidence entries)"
        )))
    }

    fn schema(&self) -> ToolSpec {
        ToolSpec::submit_task_complete()
    }
}

fn validate(args: &SubmitTaskCompleteArgs) -> ToolResult<()> {
    if args.summary.trim().is_empty() {
        return Err(ToolError::InvalidArguments(
            "submit_task_complete requires a non-empty summary".to_string(),
        ));
    }
    if args.result == TaskCompleteResult::Pass && contains_blocked_verification_claim(&args.summary)
    {
        return Err(ToolError::InvalidArguments(
            "submit_task_complete result 'pass' cannot describe blocked, failed, or unexecuted verification; use result 'fail' when acceptance evidence is incomplete"
                .to_string(),
        ));
    }
    // For pass/fail outcomes, evidence is strongly recommended but not
    // required: analysis tasks with read-only verification may legitimately
    // have empty evidence.  Reject obviously malformed entries instead.
    for (idx, entry) in args.evidence.iter().enumerate() {
        if entry.command.trim().is_empty() {
            return Err(ToolError::InvalidArguments(format!(
                "submit_task_complete evidence[{idx}].command must not be empty"
            )));
        }
        if args.result == TaskCompleteResult::Pass && entry.exit_code != 0 {
            return Err(ToolError::InvalidArguments(format!(
                "submit_task_complete result 'pass' cannot include failing evidence: evidence[{idx}] '{}' exited with {}",
                entry.command, entry.exit_code
            )));
        }
        if args.result == TaskCompleteResult::Pass
            && entry
                .output_excerpt
                .as_deref()
                .is_some_and(contains_blocked_verification_claim)
        {
            return Err(ToolError::InvalidArguments(format!(
                "submit_task_complete result 'pass' cannot include blocked, failed, or unexecuted verification evidence: evidence[{idx}] '{}'",
                entry.command
            )));
        }
    }
    Ok(())
}

fn contains_blocked_verification_claim(text: &str) -> bool {
    let normalized = text.to_ascii_lowercase();
    [
        "could not be executed",
        "could not execute",
        "could not be run",
        "could not run",
        "did not run",
        "not executed",
        "unable to execute",
        "unable to run",
        "blocked by",
        "verification failure",
        "tool execution failed",
        "failed to snapshot workspace",
        "failed to inspect workspace",
        "device not configured",
        "os error 6",
        "enotconn",
    ]
    .iter()
    .any(|phrase| normalized.contains(phrase))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn make_invocation(json: &str) -> ToolInvocation {
        ToolInvocation::new(
            "call-task-complete-1",
            "submit_task_complete",
            ToolPayload::Function {
                arguments: json.to_string(),
            },
            PathBuf::from("/tmp"),
        )
    }

    #[tokio::test]
    async fn accepts_pass_with_evidence() {
        let handler = SubmitTaskCompleteHandler;
        let inv = make_invocation(
            r#"{
                "result": "pass",
                "summary": "All three subcommands print their names",
                "evidence": [
                    {"command": "cargo run -- code", "exit_code": 0, "output_excerpt": "code\n"},
                    {"command": "cargo run -- cloud", "exit_code": 0, "output_excerpt": "cloud\n"}
                ]
            }"#,
        );

        let output = handler.handle(inv).await.unwrap();
        assert!(output.is_success());
        assert!(output.as_text().is_some_and(|t| t.contains("pass")));
    }

    #[tokio::test]
    async fn rejects_pass_with_failing_evidence() {
        let handler = SubmitTaskCompleteHandler;
        let inv = make_invocation(
            r#"{
                "result": "pass",
                "summary": "All checks passed",
                "evidence": [
                    {"command": "cargo clippy", "exit_code": -1, "output_excerpt": "failed to snapshot workspace: Device not configured (os error 6)"}
                ]
            }"#,
        );

        let result = handler.handle(inv).await;

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("cannot include failing evidence")
        );
    }

    #[tokio::test]
    async fn rejects_pass_when_summary_admits_verification_was_blocked() {
        let handler = SubmitTaskCompleteHandler;
        let inv = make_invocation(
            r#"{
                "result": "pass",
                "summary": "cargo build succeeded, but cargo clippy and cargo fmt --check could not be executed due to a persistent FUSE filesystem disconnect (ENOTCONN) on the workspace mount.",
                "evidence": [
                    {"command": "cargo build", "exit_code": 0, "output_excerpt": "Finished dev profile"},
                    {"command": "cargo run -- code", "exit_code": 0, "output_excerpt": "code"}
                ]
            }"#,
        );

        let result = handler.handle(inv).await;

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("acceptance evidence is incomplete")
        );
    }

    #[tokio::test]
    async fn rejects_pass_when_successful_evidence_excerpt_contains_tool_failure() {
        let handler = SubmitTaskCompleteHandler;
        let inv = make_invocation(
            r#"{
                "result": "pass",
                "summary": "All checks passed",
                "evidence": [
                    {"command": "cargo clippy || true", "exit_code": 0, "output_excerpt": "Tool execution failed: failed to snapshot workspace: Device not configured (os error 6)"}
                ]
            }"#,
        );

        let result = handler.handle(inv).await;

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("cannot include blocked")
        );
    }

    #[tokio::test]
    async fn accepts_fail_with_failing_evidence() {
        let handler = SubmitTaskCompleteHandler;
        let inv = make_invocation(
            r#"{
                "result": "fail",
                "summary": "clippy could not run",
                "evidence": [
                    {"command": "cargo clippy", "exit_code": -1, "output_excerpt": "failed to snapshot workspace: Device not configured (os error 6)"}
                ]
            }"#,
        );

        let output = handler.handle(inv).await.unwrap();

        assert!(output.is_success());
        assert!(output.as_text().is_some_and(|t| t.contains("fail")));
    }

    #[tokio::test]
    async fn accepts_no_changes_needed_without_evidence() {
        let handler = SubmitTaskCompleteHandler;
        let inv = make_invocation(
            r#"{"result": "no_changes_needed", "summary": "src/main.rs already implements the requested CLI"}"#,
        );

        let output = handler.handle(inv).await.unwrap();
        assert!(output.is_success());
    }

    #[tokio::test]
    async fn rejects_empty_summary() {
        let handler = SubmitTaskCompleteHandler;
        let inv = make_invocation(r#"{"result": "pass", "summary": "   "}"#);
        let result = handler.handle(inv).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn rejects_evidence_with_empty_command() {
        let handler = SubmitTaskCompleteHandler;
        let inv = make_invocation(
            r#"{
                "result": "pass",
                "summary": "ok",
                "evidence": [{"command": "  ", "exit_code": 0}]
            }"#,
        );
        let result = handler.handle(inv).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn rejects_unknown_result_variant() {
        let handler = SubmitTaskCompleteHandler;
        let inv = make_invocation(r#"{"result": "complete", "summary": "x"}"#);
        let result = handler.handle(inv).await;
        assert!(result.is_err());
    }

    #[test]
    fn exposes_expected_schema() {
        let handler = SubmitTaskCompleteHandler;
        let spec = handler.schema();
        assert_eq!(spec.function.name, "submit_task_complete");
    }
}
