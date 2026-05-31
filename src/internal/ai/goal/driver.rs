//! Goal-supervised tool-loop driver — OC-Phase 6 P6.3 wiring between
//! the pure-decision [`super::supervisor::GoalSupervisor::step`] and
//!
//! 目标监督工具循环驱动程序 — OC-Phase 6 P6.3 在纯决策 [`super::supervisor::GoalSupervisor::step`] 和实际工具执行之间的连接。
//! the standard
//! [`crate::internal::ai::agent::runtime::run_tool_loop_with_history_and_observer`]
//! entry point.
//!
//! The driver runs assistant turns through the tool loop until the
//! supervisor returns a terminal decision. Each iteration:
//!
//! 1. Run a single tool-loop turn against the current history.
//! 2. Project the [`ToolLoopTurn`] into a [`GoalTurnOutcome`] via
//!    [`goal_turn_outcome_from_tool_loop_turn`].
//! 3. Ask [`GoalSupervisor::step`] for the next decision, folding
//!    the emitted envelopes back into [`GoalState`].
//! 4. Branch on the [`GoalLoopDecision`]:
//!    * `Completed { .. }` / `Cancelled` / `AwaitUser { .. }` are
//!      terminal — break out of the loop and return the accumulated
//!      run.
//!    * `Continue { prompt }` feeds the continuation prompt back
//!      into step 1 with the updated history, bounded by
//!      [`DEFAULT_MAX_CONTINUATION_LOOPS`] to prevent a runaway
//!      driver if the supervisor keeps returning `Continue` (e.g.
//!      after a buggy verifier).
//!
//! When the iteration cap is reached the driver synthesises a
//! `LoopLimitReached` outcome and asks the supervisor for one final
//! decision (which the supervisor folds into a `Blocked {
//! LoopLimitNeedsUser }` per opencode.md rule 6); that decision is
//! what the caller sees in [`GoalSupervisedRun::decision`].
//!
//! # Mapping `ToolLoopTurn` to `GoalTurnOutcome`
//!
//! Goal protocol tool calls (`update_goal_progress` /
//! `submit_goal_complete`) are thin validators: they do not write
//! Goal events directly. When the loop returns,
//! [`goal_turn_outcome_from_tool_loop_turn`] recovers the latest
//! successful Goal tool call from the tool-loop history and maps it
//! into [`GoalTurnOutcome::ProgressUpdate`] or
//! [`GoalTurnOutcome::CompletionClaim`]. If no successful Goal tool
//! call exists, non-empty final text maps to
//! [`GoalTurnOutcome::FinalTextWithoutClaim`] (rule 4: final text
//! alone must not idle the session) and empty final text maps to
//! [`GoalTurnOutcome::Progressing`].

/// Default iteration cap for the supervisor-driven continuation
/// loop. The supervisor itself per opencode.md rule 6 emits
/// `Blocked { LoopLimitNeedsUser }` when its continuation budget is
/// exhausted; the driver enforces the same cap as a defensive
/// belt-and-braces guard so a buggy verifier cannot create an
/// infinite loop here.
const DEFAULT_MAX_CONTINUATION_LOOPS: u32 = 8;

use super::{
    event::GoalEventEnvelope,
    prompt::GoalContinuationPromptBuilder,
    state::{GoalState, apply},
    supervisor::{GoalEventClock, GoalLoopDecision, GoalSupervisor, GoalTurnOutcome},
    verifier::{GoalVerifier, GoalVerifierContext},
};
use crate::internal::ai::{
    agent::runtime::{
        ToolLoopConfig, ToolLoopObserver, ToolLoopTurn, run_tool_loop_with_history_and_observer,
    },
    completion::{
        AssistantContent, CompletionError, CompletionModel, CompletionUsage, Message, UserContent,
    },
    tools::ToolRegistry,
};

/// Output of one driver invocation. Mirrors [`ToolLoopTurn`]'s
/// `final_text` + `history` plus the supervisor-folded `state`,
/// emitted `events`, and the [`GoalLoopDecision`] the caller acts on
/// next.
#[derive(Clone, Debug)]
pub struct GoalSupervisedRun {
    pub final_text: String,
    pub history: Vec<Message>,
    pub state: GoalState,
    pub events: Vec<GoalEventEnvelope>,
    pub decision: GoalLoopDecision,
}

/// Request shape for one driver invocation. The lifetime `'a` ties
/// the borrowed runtime collaborators (model, registry, observer,
/// supervisor, verifier ctx, clock) to the call site so the driver
/// itself stays allocation-free on the hot path.
pub struct GoalSupervisedToolLoopRequest<'a, M, O, V, P>
where
    M: CompletionModel,
    M::Response: CompletionUsage,
    O: ToolLoopObserver,
    V: GoalVerifier,
    P: GoalContinuationPromptBuilder,
{
    pub model: &'a M,
    pub history: Vec<Message>,
    pub initial_prompt: String,
    pub registry: &'a ToolRegistry,
    pub config: ToolLoopConfig,
    pub observer: &'a mut O,
    pub state: GoalState,
    pub supervisor: &'a GoalSupervisor<V, P>,
    pub verifier_ctx: &'a (dyn GoalVerifierContext + Send + Sync),
    pub clock: &'a (dyn GoalEventClock + Send + Sync),
}

/// Run a Goal-supervised assistant interaction end-to-end. The
/// driver loops the tool-loop + supervisor pair until a terminal
/// decision (Completed / Cancelled / AwaitUser) or the iteration
/// cap is reached. See module docs for the per-iteration flow.
pub async fn run_goal_supervised_tool_loop<'a, M, O, V, P>(
    request: GoalSupervisedToolLoopRequest<'a, M, O, V, P>,
) -> Result<GoalSupervisedRun, CompletionError>
where
    M: CompletionModel,
    M::Response: CompletionUsage,
    O: ToolLoopObserver,
    V: GoalVerifier,
    P: GoalContinuationPromptBuilder,
{
    let GoalSupervisedToolLoopRequest {
        model,
        history,
        initial_prompt,
        registry,
        config,
        observer,
        state,
        supervisor,
        verifier_ctx,
        clock,
    } = request;

    let mut current_state = state;
    let mut current_history = history;
    let mut next_prompt = initial_prompt;
    let mut accumulated_events: Vec<GoalEventEnvelope> = Vec::new();
    let mut loops_run: u32 = 0;

    loop {
        let turn = run_tool_loop_with_history_and_observer(
            model,
            current_history.clone(),
            next_prompt.clone(),
            registry,
            config.clone(),
            observer,
        )
        .await?;

        let outcome = goal_turn_outcome_from_tool_loop_turn(&turn);
        let step = supervisor.step(&current_state, outcome, verifier_ctx, clock);
        for envelope in &step.events {
            // INVARIANT: supervisor.step emits envelopes that are
            // already shape-validated; replay rejections here would
            // indicate an internal bug (mis-ordered clock, duplicate
            // envelope id). We surface them via trace rather than
            // aborting so the caller can still see the partial state.
            if let Err(reject) = apply(&mut current_state, envelope) {
                tracing::warn!(
                    envelope_id = %envelope.envelope_id,
                    ?reject,
                    "goal supervisor envelope rejected on apply",
                );
            }
        }
        accumulated_events.extend(step.events);

        current_history = turn.history;
        let last_final_text = turn.final_text;
        loops_run = loops_run.saturating_add(1);

        match step.decision {
            GoalLoopDecision::Continue { prompt } => {
                if loops_run >= DEFAULT_MAX_CONTINUATION_LOOPS {
                    // Synthesise the supervisor's
                    // `LoopLimitReached` outcome so it can convert
                    // to a `Blocked { LoopLimitNeedsUser }`
                    // decision (opencode.md rule 6) — the caller
                    // gets the Blocked terminal we agreed on
                    // instead of a silent runaway exit.
                    let final_step = supervisor.step(
                        &current_state,
                        GoalTurnOutcome::LoopLimitReached { loops_run },
                        verifier_ctx,
                        clock,
                    );
                    for envelope in &final_step.events {
                        if let Err(reject) = apply(&mut current_state, envelope) {
                            tracing::warn!(
                                envelope_id = %envelope.envelope_id,
                                ?reject,
                                "goal supervisor envelope rejected on apply (loop-limit branch)",
                            );
                        }
                    }
                    accumulated_events.extend(final_step.events);
                    return Ok(GoalSupervisedRun {
                        final_text: last_final_text,
                        history: current_history,
                        state: current_state,
                        events: accumulated_events,
                        decision: final_step.decision,
                    });
                }
                next_prompt = prompt;
            }
            terminal @ (GoalLoopDecision::Completed { .. }
            | GoalLoopDecision::Cancelled
            | GoalLoopDecision::AwaitUser { .. }) => {
                return Ok(GoalSupervisedRun {
                    final_text: last_final_text,
                    history: current_history,
                    state: current_state,
                    events: accumulated_events,
                    decision: terminal,
                });
            }
        }
    }
}

/// Project a [`ToolLoopTurn`] down to a [`GoalTurnOutcome`].
///
/// Non-empty `final_text` → `FinalTextWithoutClaim` (rule 4: final
/// text alone must NOT idle the session). Empty `final_text` →
/// `Progressing { last_assistant_text: None }` (the model only made
/// tool calls; the loop ended via terminal-tool or max-turns).
pub fn goal_turn_outcome_from_tool_loop_turn(turn: &ToolLoopTurn) -> GoalTurnOutcome {
    if let Some(outcome) = latest_goal_tool_outcome(&turn.history) {
        return outcome;
    }

    let trimmed = turn.final_text.trim();
    if trimmed.is_empty() {
        GoalTurnOutcome::Progressing {
            last_assistant_text: None,
        }
    } else {
        GoalTurnOutcome::FinalTextWithoutClaim {
            text: turn.final_text.clone(),
        }
    }
}

fn latest_goal_tool_outcome(history: &[Message]) -> Option<GoalTurnOutcome> {
    let successful_results = successful_tool_results(history);
    for message in history.iter().rev() {
        let Message::Assistant { content, .. } = message else {
            continue;
        };
        let parts = content.iter().collect::<Vec<_>>();
        for part in parts.into_iter().rev() {
            let AssistantContent::ToolCall(call) = part else {
                continue;
            };
            let tool_name = call.function.name.as_str();
            if !matches!(tool_name, "submit_goal_complete" | "update_goal_progress") {
                continue;
            }
            if !successful_results
                .iter()
                .any(|(id, name)| id == &call.id && name == tool_name)
            {
                continue;
            }
            return match tool_name {
                "submit_goal_complete" => serde_json::from_value(call.function.arguments.clone())
                    .ok()
                    .map(|claim| GoalTurnOutcome::CompletionClaim { claim }),
                "update_goal_progress" => serde_json::from_value(call.function.arguments.clone())
                    .ok()
                    .map(|record| GoalTurnOutcome::ProgressUpdate { record }),
                _ => None,
            };
        }
    }
    None
}

fn successful_tool_results(history: &[Message]) -> Vec<(String, String)> {
    history
        .iter()
        .filter_map(|message| {
            let Message::User { content } = message else {
                return None;
            };
            Some(content.iter().filter_map(|part| {
                let UserContent::ToolResult(result) = part else {
                    return None;
                };
                if tool_result_succeeded(&result.result) {
                    Some((result.id.clone(), result.name.clone()))
                } else {
                    None
                }
            }))
        })
        .flatten()
        .collect()
}

fn tool_result_succeeded(result: &serde_json::Value) -> bool {
    result
        .get("success")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::completion::{Function, OneOrMany, ToolCall, ToolResult};

    /// Non-empty final text routes through `FinalTextWithoutClaim`
    /// (rule 4) so the supervisor records a synthetic
    /// `ProgressRecorded` and keeps the session active.
    #[test]
    fn non_empty_final_text_maps_to_final_text_without_claim() {
        let turn = ToolLoopTurn {
            final_text: "  here is the plan  ".to_string(),
            history: Vec::new(),
        };
        match goal_turn_outcome_from_tool_loop_turn(&turn) {
            GoalTurnOutcome::FinalTextWithoutClaim { text } => {
                assert_eq!(text, "  here is the plan  ");
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    /// Empty / whitespace-only final text means the model exited via
    /// tool calls. The supervisor sees it as `Progressing`, not a
    /// terminal signal.
    #[test]
    fn empty_final_text_maps_to_progressing_without_last_text() {
        let turn = ToolLoopTurn {
            final_text: "   ".to_string(),
            history: Vec::new(),
        };
        match goal_turn_outcome_from_tool_loop_turn(&turn) {
            GoalTurnOutcome::Progressing {
                last_assistant_text,
            } => {
                assert!(last_assistant_text.is_none());
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[test]
    fn successful_submit_goal_complete_tool_call_maps_to_completion_claim() {
        let arguments = serde_json::json!({
            "summary": "done",
            "completed_criteria": ["criterion-a"],
            "evidence_refs": [
                {
                    "criterion_id": "criterion-a",
                    "target": {"kind": "tool_call", "call_id": "tc-cargo-test"},
                    "description": "cargo test passed"
                }
            ],
            "verification": [
                {"criterion_id": "criterion-a", "method": "cargo test", "passed": true}
            ],
            "residual_risks": []
        });
        let turn = ToolLoopTurn {
            final_text: "Completion claim submitted".to_string(),
            history: vec![
                tool_call_message("call-submit", "submit_goal_complete", arguments),
                tool_result_message("call-submit", "submit_goal_complete", true),
            ],
        };

        match goal_turn_outcome_from_tool_loop_turn(&turn) {
            GoalTurnOutcome::CompletionClaim { claim } => {
                assert_eq!(claim.summary, "done");
                assert_eq!(claim.completed_criteria, vec!["criterion-a"]);
            }
            other => panic!("expected completion claim, got {other:?}"),
        }
    }

    #[test]
    fn failed_submit_goal_complete_tool_result_falls_back_to_final_text() {
        let turn = ToolLoopTurn {
            final_text: "Completion claim failed shape validation".to_string(),
            history: vec![
                tool_call_message(
                    "call-submit",
                    "submit_goal_complete",
                    serde_json::json!({"summary": ""}),
                ),
                tool_result_message("call-submit", "submit_goal_complete", false),
            ],
        };

        match goal_turn_outcome_from_tool_loop_turn(&turn) {
            GoalTurnOutcome::FinalTextWithoutClaim { text } => {
                assert_eq!(text, "Completion claim failed shape validation");
            }
            other => panic!("expected final text fallback, got {other:?}"),
        }
    }

    #[test]
    fn successful_update_goal_progress_tool_call_maps_to_progress_update() {
        let turn = ToolLoopTurn {
            final_text: "Progress recorded".to_string(),
            history: vec![
                tool_call_message(
                    "call-progress",
                    "update_goal_progress",
                    serde_json::json!({
                        "summary": "added the test",
                        "completed_criteria": ["test-added"],
                        "evidence_refs": [],
                        "next_steps": ["run cargo test"]
                    }),
                ),
                tool_result_message("call-progress", "update_goal_progress", true),
            ],
        };

        match goal_turn_outcome_from_tool_loop_turn(&turn) {
            GoalTurnOutcome::ProgressUpdate { record } => {
                assert_eq!(record.summary, "added the test");
                assert_eq!(record.completed_criteria, vec!["test-added"]);
            }
            other => panic!("expected progress update, got {other:?}"),
        }
    }

    fn tool_call_message(id: &str, name: &str, arguments: serde_json::Value) -> Message {
        Message::Assistant {
            id: None,
            reasoning_content: None,
            content: OneOrMany::One(AssistantContent::ToolCall(ToolCall {
                id: id.to_string(),
                name: name.to_string(),
                function: Function {
                    name: name.to_string(),
                    arguments,
                },
            })),
        }
    }

    fn tool_result_message(id: &str, name: &str, success: bool) -> Message {
        Message::User {
            content: OneOrMany::One(UserContent::ToolResult(ToolResult {
                id: id.to_string(),
                name: name.to_string(),
                result: serde_json::json!({
                    "content": if success { "ok" } else { "failed" },
                    "success": success,
                }),
            })),
        }
    }
}
