//! Goal-supervised tool-loop driver — OC-Phase 6 P6.3 wiring between
//! the pure-decision [`super::supervisor::GoalSupervisor::step`] and
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
//! Tool calls (`update_goal_progress` / `submit_goal_complete`) are
//! recorded by their handlers; by the time the loop returns, the
//! supervisor's input is purely "did the model produce final text?".
//! [`goal_turn_outcome_from_tool_loop_turn`] therefore emits
//! [`GoalTurnOutcome::FinalTextWithoutClaim`] for non-empty final
//! text (the rule-4 path that must not let the session idle) and
//! [`GoalTurnOutcome::Progressing`] for an empty final text (the
//! model called tools and produced no closing message). Richer
//! outcomes — `ProgressUpdate`, `CompletionClaim`, the various
//! `Blocked*` arms — are produced inside the tool handlers and
//! routed through the supervisor on subsequent driver invocations.

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
    completion::{CompletionError, CompletionModel, CompletionUsage, Message},
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
