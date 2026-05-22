//! Goal-supervised tool-loop driver — OC-Phase 6 P6.3 wiring between
//! the pure-decision [`super::supervisor::GoalSupervisor::step`] and
//! the standard
//! [`crate::internal::ai::agent::runtime::run_tool_loop_with_history_and_observer`]
//! entry point.
//!
//! The driver runs **one** assistant turn through the tool loop,
//! extracts a [`GoalTurnOutcome`] from the resulting
//! [`ToolLoopTurn`], asks the supervisor for the next decision, and
//! folds the resulting envelopes back into a fresh [`GoalState`].
//! Callers (the TUI / Code Control / future automation loop) own the
//! re-entry decision: if `decision == Continue { prompt }` they
//! invoke the driver again with the supervisor's continuation
//! prompt; if `Completed` / `AwaitUser` / `Cancelled` they release
//! the turn back to idle. This single-turn shape keeps the driver
//! independent of any specific event-bus or UI cadence.
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

/// Run one Goal-supervised assistant turn end-to-end. See module
/// docs for the loop-vs-caller-driven shape.
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

    let turn = run_tool_loop_with_history_and_observer(
        model,
        history,
        initial_prompt,
        registry,
        config,
        observer,
    )
    .await?;

    let outcome = goal_turn_outcome_from_tool_loop_turn(&turn);
    let step = supervisor.step(&state, outcome, verifier_ctx, clock);
    let mut new_state = state;
    for envelope in &step.events {
        // INVARIANT: supervisor.step emits envelopes that are
        // already shape-validated; replay rejections here would
        // indicate an internal bug (mis-ordered clock, duplicate
        // envelope id). We surface them via trace rather than
        // aborting so the caller can still see the partial state.
        if let Err(reject) = apply(&mut new_state, envelope) {
            tracing::warn!(
                envelope_id = %envelope.envelope_id,
                ?reject,
                "goal supervisor envelope rejected on apply",
            );
        }
    }

    Ok(GoalSupervisedRun {
        final_text: turn.final_text,
        history: turn.history,
        state: new_state,
        events: step.events,
        decision: step.decision,
    })
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
