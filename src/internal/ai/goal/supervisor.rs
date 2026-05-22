//! Goal supervisor — Goal-bound tool-loop orchestrator.
//!
//! Per `docs/improvement/opencode.md` lines 632-668, the supervisor is
//! the entity that turns a freshly returned `ToolLoopTurn` into the
//! next loop decision. It does **not** drive `run_tool_loop` itself
//! (that integration lives in P6.5 / P6.6 — `libra code` CLI/TUI and
//! Code Control NDJSON); it consumes the *outcome* of a turn and
//! produces:
//!
//! 1. A list of `GoalEventEnvelope`s the caller must append to the
//!    session JSONL stream (and apply via [`super::state::apply`]),
//!    e.g. a fresh `CompletionClaimed` followed by
//!    `Completed`/`CompletionRejected` after the verifier ran.
//! 2. A [`GoalLoopDecision`] that tells the caller what to do for
//!    the next iteration: continue with a freshly-built continuation
//!    prompt, await user input, mark the Goal completed, or honor a
//!    cancellation.
//!
//! This decoupling keeps the supervisor's invariants
//! ("non-completion never marks the Goal complete", "verifier is the
//! only path to terminal Completed", "budget overrun → Blocked, never
//! Completed") testable in isolation.
//!
//! # Decision rules (mapped to opencode.md:653-668)
//!
//! | Turn outcome                              | Result                                                               |
//! |-------------------------------------------|----------------------------------------------------------------------|
//! | Model called `submit_goal_complete`       | Append `CompletionClaimed`. Run verifier. Append `Completed` or `CompletionRejected`. Decide `Completed { report }` or `Continue { prompt }`. |
//! | Model called `update_goal_progress`       | Append `ProgressRecorded`. `Continue { prompt }`.                    |
//! | Model gave final text without a claim     | Append `ProgressRecorded` (synthesised from the text). `Continue { prompt }`. (Rule 4: final text MUST NOT idle the session.) |
//! | Approval denied                           | Append `Blocked { ApprovalDenied }`. `AwaitUser { question }`.       |
//! | Provider unrecoverable                    | Append `Blocked { ProviderUnrecoverable }`. `AwaitUser`.             |
//! | Hard budget cap reached                   | Append `Blocked { BudgetApprovalRequired }`. `AwaitUser`.            |
//! | Wall-clock expired                        | Append `Blocked { WallClockExpired }`. `AwaitUser`.                  |
//! | Continuation-loop cap                     | Append `Blocked { LoopLimitNeedsUser }`. `AwaitUser`.                |
//! | `max_turns` reached                       | Append `Blocked { MaxTurnsReached }`. `AwaitUser`.                   |
//! | Repeat abort                              | Append `Blocked { RepeatAborted }`. `AwaitUser`.                     |
//! | Context overflow exhausted                | Append `Blocked { ContextOverflowExhausted }`. `AwaitUser`.          |
//! | User-driven scope question                | Append `Blocked { AwaitingScopeChange }`. `AwaitUser`.               |
//! | Explicit user / automation cancel         | Append `Cancelled`. `Cancelled`.                                     |
//! | Plain progress (model used tools cleanly) | No event appended (the tools' own events landed earlier). `Continue { prompt }`. |
//!
//! # Determinism
//!
//! `step()` is a pure function of `(state, outcome, verifier_ctx,
//! clock)`. Envelope ids and timestamps come exclusively from the
//! [`GoalEventClock`] trait so tests can assert byte-stable event
//! sequences. Production callers wire a clock that delegates to
//! `Uuid::new_v4()` and `Utc::now()`.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::{
    event::{
        GoalBlockReason, GoalCompletionClaim, GoalCompletionReport, GoalEvent, GoalEventEnvelope,
        GoalProgressRecord,
    },
    prompt::{GoalContinuationPromptBuilder, trimmed_excerpt},
    spec::GoalActor,
    state::GoalState,
    verifier::{GoalVerifier, GoalVerifierContext, GoalVerifyOutcome},
};
use crate::internal::ai::{
    agent::runtime::tool_loop::{
        ToolLoopConfig, ToolLoopObserver, ToolLoopTurn, run_tool_loop_with_history_and_observer,
    },
    completion::{AssistantContent, CompletionModel, CompletionUsage, Message},
    tools::ToolRegistry,
};

/// Fresh envelope id + wall-clock instant. Production wires
/// `Uuid::new_v4()` + `Utc::now()`; tests use a deterministic clock
/// so the supervisor's emitted event stream is byte-stable.
pub trait GoalEventClock {
    fn mint_envelope_id(&self) -> Uuid;
    fn now(&self) -> DateTime<Utc>;
}

/// Stop policy the caller (P6.5 CLI/TUI / P6.6 Code Control) checks
/// to decide whether to keep the assistant turn open or release the
/// session to idle.
///
/// `Normal` keeps the legacy non-Goal behaviour: the tool loop ends
/// after a single turn unless the model continues. `GoalBound { goal_id }`
/// pins the loop to a specific Goal — final text alone never lets the
/// session idle (opencode.md:657, 663-668).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GoalStopPolicy {
    Normal,
    GoalBound { goal_id: Uuid },
}

/// What the just-finished `run_tool_loop` turn produced from the
/// supervisor's perspective. The caller maps the richer
/// `ToolLoopTurn` + budget metering down to this enum before
/// invoking [`GoalSupervisor::step`].
///
/// `Progressing` is the normal in-flight case (the model used tools
/// and the supervisor will simply nudge it with a continuation
/// prompt). The remaining variants surface terminal / blocking
/// signals the supervisor must record before deciding the next
/// step.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GoalTurnOutcome {
    /// The model used tools and produced no terminal signal — the
    /// supervisor will just nudge it with a continuation prompt.
    /// Carries the final text fragment (if any) so the prompt
    /// builder can echo it back into context.
    Progressing { last_assistant_text: Option<String> },
    /// The model emitted a final text without calling
    /// `submit_goal_complete`. Per opencode.md:657 the supervisor
    /// must NOT let the session idle: it records a synthetic
    /// `ProgressRecorded` and continues with a continuation prompt
    /// nudging the model toward the completion protocol.
    FinalTextWithoutClaim { text: String },
    /// The model called `update_goal_progress` with a record. The
    /// supervisor appends `ProgressRecorded(record)` and
    /// continues.
    ProgressUpdate { record: GoalProgressRecord },
    /// The model called `submit_goal_complete` with a claim. The
    /// supervisor appends `CompletionClaimed`, runs the verifier,
    /// then appends either `Completed { report }` (verifier
    /// accepted → terminal) or `CompletionRejected { ... }`
    /// (continue with a continuation prompt that lists what was
    /// missing).
    CompletionClaim { claim: GoalCompletionClaim },
    /// The permission layer denied a tool call.
    ApprovalDenied {
        denied_tool: String,
        denied_args_summary: Option<String>,
        reason: String,
    },
    /// The provider returned a non-recoverable error.
    ProviderUnrecoverable {
        provider_id: String,
        message: String,
    },
    /// The hard budget cap was hit.
    BudgetHardCap {
        cap_micro_usd: u64,
        spent_micro_usd: u64,
    },
    /// The wall-clock cap was hit.
    WallClockExpired { wall_clock_seconds: u64 },
    /// The supervisor's continuation-loop count was exhausted.
    LoopLimitReached { loops_run: u32 },
    /// The single-turn `max_turns` cap was hit without forward
    /// progress.
    MaxTurnsReached { turns: u32 },
    /// The repeat-abort heuristic fired.
    RepeatAborted { signature: String, repetitions: u32 },
    /// Context-overflow compaction failed.
    ContextOverflowExhausted { attempts: u32, last_error: String },
    /// The supervisor (or the model via prompt) asked the user a
    /// concrete scope question that requires explicit input.
    AwaitingScopeChange { question: String },
    /// The user / automation owner / lease holder explicitly
    /// cancelled the Goal.
    UserCancelled {
        reason: String,
        cancelled_by: GoalActor,
    },
}

/// Final outcome of [`GoalSupervisor::step`] — what the caller
/// should do for the next iteration.
///
/// Only `Completed` and `Cancelled` release the session to idle.
/// `Continue` re-enters the tool loop with a freshly built prompt;
/// `AwaitUser` keeps the Goal active but stops the current turn so
/// the user can answer the surfaced question. Per opencode.md:665
/// "non-terminal pause boundary": `AwaitUser` MUST NOT be rendered
/// as `completed` by any UI.
///
/// `Completed`'s report is boxed so the enum's large terminal
/// payload doesn't bloat the stack size of the common
/// `Continue`/`AwaitUser` paths the supervisor returns on every
/// loop tick.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GoalLoopDecision {
    Continue { prompt: String },
    AwaitUser { question: String },
    Completed { report: Box<GoalCompletionReport> },
    Cancelled,
}

/// One supervisor step result. `events` are the envelopes the
/// caller must append to the session JSONL (and fold into the
/// state via [`super::state::apply`]) **before** acting on
/// `decision`. The order matters: `decision == Completed { report }`
/// arrives only after the corresponding `GoalEvent::Completed`
/// envelope is in `events`, so a caller that defers the apply step
/// will see a consistent state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GoalSupervisorStep {
    pub events: Vec<GoalEventEnvelope>,
    pub decision: GoalLoopDecision,
}

/// Result from running one Goal-bound supervisor loop around the
/// normal tool loop.
#[derive(Clone, Debug, PartialEq)]
pub struct GoalSupervisedRun {
    pub state: GoalState,
    pub events: Vec<GoalEventEnvelope>,
    pub decision: GoalLoopDecision,
    pub final_text: String,
    pub history: Vec<Message>,
    pub loops_run: u32,
}

/// Input bundle for [`run_goal_supervised_tool_loop`]. The runner has
/// to coordinate the normal tool-loop dependencies plus Goal state,
/// verifier context, and event clock; bundling those fields keeps the
/// public entrypoint readable and avoids positional argument drift.
pub struct GoalSupervisedToolLoopRequest<'a, M, O, V, P>
where
    M: CompletionModel,
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
    pub verifier_ctx: &'a (dyn GoalVerifierContext + Sync),
    pub clock: &'a (dyn GoalEventClock + Sync),
}

// The `GoalContinuationPromptBuilder` trait, the default impl
// (`DefaultGoalContinuationPromptBuilder`), and the shared
// `trimmed_excerpt` helper now live in `super::prompt` so the
// supervisor's decision tree reads as a sequence of state-machine
// rules rather than a 110-line prose template. The supervisor still
// uses `trimmed_excerpt` for its `FinalTextWithoutClaim` synthetic-
// progress arm (the doc's rule 4 path), so it's imported back via
// the `use super::prompt::{...}` line at the top of this file.

/// Goal-mode supervisor. Holds the stop policy + verifier + prompt
/// builder; [`step`](Self::step) is the one entry point that turns a
/// turn outcome into events + decision.
#[derive(Clone, Debug)]
pub struct GoalSupervisor<V, P>
where
    V: GoalVerifier,
    P: GoalContinuationPromptBuilder,
{
    pub stop_policy: GoalStopPolicy,
    pub verifier: V,
    pub prompt_builder: P,
}

impl<V, P> GoalSupervisor<V, P>
where
    V: GoalVerifier,
    P: GoalContinuationPromptBuilder,
{
    /// Apply the supervisor's decision rules to one turn outcome.
    ///
    /// Returns the events to append (in order) plus the decision the
    /// caller should act on. The caller is responsible for
    /// persisting envelopes to the session JSONL and folding them
    /// into [`GoalState`] via [`super::state::apply`] — the
    /// supervisor is pure.
    ///
    /// # Panics
    ///
    /// Never. Every variant of [`GoalTurnOutcome`] has a defined
    /// branch; verifier rejections and accepts are both legal
    /// outcomes that flow into a final decision.
    pub fn step(
        &self,
        state: &GoalState,
        outcome: GoalTurnOutcome,
        verifier_ctx: &dyn GoalVerifierContext,
        clock: &dyn GoalEventClock,
    ) -> GoalSupervisorStep {
        let goal_id = state.spec.goal_id;
        let mut events: Vec<GoalEventEnvelope> = Vec::new();

        match outcome {
            GoalTurnOutcome::CompletionClaim { claim } => {
                // Supervisor protocol (opencode.md:1465-1467):
                // append CompletionClaimed FIRST so the schema's
                // claim binding has an envelope_id to bind against,
                // then run the verifier and append the verifier's
                // verdict.
                let claim_envelope_id = clock.mint_envelope_id();
                let claim_recorded_at = clock.now();
                events.push(GoalEventEnvelope {
                    envelope_id: claim_envelope_id,
                    goal_id,
                    recorded_at: claim_recorded_at,
                    event: GoalEvent::CompletionClaimed(claim.clone()),
                });
                let verifier_outcome =
                    self.verifier
                        .verify(verifier_ctx, &state.spec, &claim, claim_envelope_id);
                match verifier_outcome {
                    GoalVerifyOutcome::Accept(report) => {
                        events.push(GoalEventEnvelope {
                            envelope_id: clock.mint_envelope_id(),
                            goal_id,
                            recorded_at: clock.now(),
                            event: GoalEvent::Completed(report.clone()),
                        });
                        GoalSupervisorStep {
                            events,
                            decision: GoalLoopDecision::Completed {
                                report: Box::new(report),
                            },
                        }
                    }
                    GoalVerifyOutcome::Reject { missing, reason } => {
                        events.push(GoalEventEnvelope {
                            envelope_id: clock.mint_envelope_id(),
                            goal_id,
                            recorded_at: clock.now(),
                            event: GoalEvent::CompletionRejected {
                                claim_envelope_id,
                                missing: missing.clone(),
                                reason: reason.clone(),
                            },
                        });
                        // Build the continuation prompt against the
                        // *post-event* state so it sees the new
                        // blocker. The caller cannot give us the
                        // post-event state without applying the
                        // events first, so rebuild it here.
                        let next_state = state_with_events_applied(state, &events);
                        let prompt = self
                            .prompt_builder
                            .build(&next_state, &GoalTurnOutcome::CompletionClaim { claim });
                        GoalSupervisorStep {
                            events,
                            decision: GoalLoopDecision::Continue { prompt },
                        }
                    }
                }
            }
            GoalTurnOutcome::ProgressUpdate { record } => {
                events.push(GoalEventEnvelope {
                    envelope_id: clock.mint_envelope_id(),
                    goal_id,
                    recorded_at: clock.now(),
                    event: GoalEvent::ProgressRecorded(record.clone()),
                });
                let next_state = state_with_events_applied(state, &events);
                let prompt = self
                    .prompt_builder
                    .build(&next_state, &GoalTurnOutcome::ProgressUpdate { record });
                GoalSupervisorStep {
                    events,
                    decision: GoalLoopDecision::Continue { prompt },
                }
            }
            GoalTurnOutcome::FinalTextWithoutClaim { text } => {
                // Synthesise a `ProgressRecorded` so the audit log
                // captures the model's narrative — opencode.md:657
                // mandates the supervisor never lets a Goal idle on
                // final text alone.
                let synthetic = GoalProgressRecord {
                    summary: trimmed_excerpt(&text),
                    completed_criteria: Vec::new(),
                    evidence_refs: Vec::new(),
                    next_steps: Vec::new(),
                };
                events.push(GoalEventEnvelope {
                    envelope_id: clock.mint_envelope_id(),
                    goal_id,
                    recorded_at: clock.now(),
                    event: GoalEvent::ProgressRecorded(synthetic),
                });
                let next_state = state_with_events_applied(state, &events);
                let prompt = self.prompt_builder.build(
                    &next_state,
                    &GoalTurnOutcome::FinalTextWithoutClaim { text },
                );
                GoalSupervisorStep {
                    events,
                    decision: GoalLoopDecision::Continue { prompt },
                }
            }
            GoalTurnOutcome::Progressing {
                last_assistant_text,
            } => {
                // Plain progress — the tools' own events landed
                // earlier in the JSONL. The supervisor only needs to
                // build the next continuation prompt.
                let prompt = self.prompt_builder.build(
                    state,
                    &GoalTurnOutcome::Progressing {
                        last_assistant_text,
                    },
                );
                GoalSupervisorStep {
                    events,
                    decision: GoalLoopDecision::Continue { prompt },
                }
            }
            GoalTurnOutcome::UserCancelled {
                reason,
                cancelled_by,
            } => {
                events.push(GoalEventEnvelope {
                    envelope_id: clock.mint_envelope_id(),
                    goal_id,
                    recorded_at: clock.now(),
                    event: GoalEvent::Cancelled {
                        reason,
                        cancelled_by,
                    },
                });
                GoalSupervisorStep {
                    events,
                    decision: GoalLoopDecision::Cancelled,
                }
            }
            GoalTurnOutcome::AwaitingScopeChange { question } => {
                events.push(GoalEventEnvelope {
                    envelope_id: clock.mint_envelope_id(),
                    goal_id,
                    recorded_at: clock.now(),
                    event: GoalEvent::Blocked {
                        reason: GoalBlockReason::AwaitingScopeChange {
                            question: question.clone(),
                        },
                        requested_input: Some(question.clone()),
                    },
                });
                GoalSupervisorStep {
                    events,
                    decision: GoalLoopDecision::AwaitUser { question },
                }
            }
            // The remaining variants all map to a `Blocked { ... }`
            // event + AwaitUser. Bundle them through one helper to
            // keep the per-variant code small and the doc rules
            // explicit.
            GoalTurnOutcome::ApprovalDenied {
                denied_tool,
                denied_args_summary,
                reason,
            } => block_and_await(
                clock,
                goal_id,
                GoalBlockReason::ApprovalDenied {
                    denied_tool: denied_tool.clone(),
                    denied_args_summary: denied_args_summary.clone(),
                    reason: reason.clone(),
                },
                format!(
                    "Approval denied for `{denied_tool}`: {reason}. \
                     Adjust scope, re-grant the tool, or cancel the Goal."
                ),
            ),
            GoalTurnOutcome::ProviderUnrecoverable {
                provider_id,
                message,
            } => block_and_await(
                clock,
                goal_id,
                GoalBlockReason::ProviderUnrecoverable {
                    provider_id: provider_id.clone(),
                    message: message.clone(),
                },
                format!(
                    "Provider `{provider_id}` returned a non-recoverable error: {message}. \
                     Switch model / refresh keys, then resume."
                ),
            ),
            GoalTurnOutcome::BudgetHardCap {
                cap_micro_usd,
                spent_micro_usd,
            } => block_and_await(
                clock,
                goal_id,
                GoalBlockReason::BudgetApprovalRequired {
                    cap_micro_usd,
                    spent_micro_usd,
                },
                format!(
                    "Hard budget cap reached ({} / {} micro-USD). \
                     Run `/budget goal approve <amount>` or cancel.",
                    spent_micro_usd, cap_micro_usd,
                ),
            ),
            GoalTurnOutcome::WallClockExpired { wall_clock_seconds } => block_and_await(
                clock,
                goal_id,
                GoalBlockReason::WallClockExpired { wall_clock_seconds },
                format!(
                    "Wall-clock budget exhausted ({wall_clock_seconds}s). \
                     Approve more time or cancel."
                ),
            ),
            GoalTurnOutcome::LoopLimitReached { loops_run } => block_and_await(
                clock,
                goal_id,
                GoalBlockReason::LoopLimitNeedsUser { loops_run },
                format!(
                    "Continuation loop cap reached ({loops_run} loops). \
                     Confirm to continue or cancel."
                ),
            ),
            GoalTurnOutcome::MaxTurnsReached { turns } => block_and_await(
                clock,
                goal_id,
                GoalBlockReason::MaxTurnsReached { turns },
                format!(
                    "Single-turn `max_turns` cap reached ({turns}). \
                     Extend the cap, change scope, or cancel."
                ),
            ),
            GoalTurnOutcome::RepeatAborted {
                signature,
                repetitions,
            } => block_and_await(
                clock,
                goal_id,
                GoalBlockReason::RepeatAborted {
                    signature: signature.clone(),
                    repetitions,
                },
                format!(
                    "Aborted: tool call `{signature}` repeated {repetitions} times. \
                     Adjust the approach or cancel."
                ),
            ),
            GoalTurnOutcome::ContextOverflowExhausted {
                attempts,
                last_error,
            } => block_and_await(
                clock,
                goal_id,
                GoalBlockReason::ContextOverflowExhausted {
                    attempts,
                    last_error: last_error.clone(),
                },
                format!(
                    "Context overflow after {attempts} compaction attempts: {last_error}. \
                     Shrink scope or pick a model with a larger window."
                ),
            ),
        }
    }
}

/// Run `run_tool_loop` under a Goal supervisor until the Goal reaches
/// a terminal or pause boundary. A plain assistant final answer never
/// releases the session to idle while the Goal is active: it is folded
/// into `ProgressRecorded`, then the continuation prompt re-enters
/// the normal tool loop.
pub async fn run_goal_supervised_tool_loop<M, O, V, P>(
    request: GoalSupervisedToolLoopRequest<'_, M, O, V, P>,
) -> Result<GoalSupervisedRun, crate::internal::ai::completion::CompletionError>
where
    M: CompletionModel,
    M::Response: CompletionUsage,
    O: ToolLoopObserver,
    V: GoalVerifier,
    P: GoalContinuationPromptBuilder,
{
    let GoalSupervisedToolLoopRequest {
        model,
        mut history,
        initial_prompt,
        registry,
        mut config,
        observer,
        mut state,
        supervisor,
        verifier_ctx,
        clock,
    } = request;
    ensure_goal_terminal_tool(&mut config, "submit_goal_complete");
    let max_loops = state.spec.budget.max_continuation_loops.max(1);
    let mut prompt = initial_prompt;
    let mut events = Vec::new();
    let mut loops_run = 0u32;

    loop {
        loops_run = loops_run.saturating_add(1);
        let turn = run_tool_loop_with_history_and_observer(
            model,
            history,
            prompt,
            registry,
            config.clone(),
            &mut *observer,
        )
        .await?;
        let turn_final_text = turn.final_text.clone();
        history = turn.history.clone();
        let outcome = goal_turn_outcome_from_tool_loop_turn(&turn);
        let step = supervisor.step(&state, outcome, verifier_ctx, clock);
        apply_supervisor_events(&mut state, &step.events);
        events.extend(step.events.clone());

        match step.decision {
            GoalLoopDecision::Continue {
                prompt: next_prompt,
            } => {
                if loops_run >= max_loops {
                    let cap_step = supervisor.step(
                        &state,
                        GoalTurnOutcome::LoopLimitReached { loops_run },
                        verifier_ctx,
                        clock,
                    );
                    apply_supervisor_events(&mut state, &cap_step.events);
                    events.extend(cap_step.events.clone());
                    return Ok(GoalSupervisedRun {
                        state,
                        events,
                        decision: cap_step.decision,
                        final_text: turn_final_text,
                        history,
                        loops_run,
                    });
                }
                prompt = next_prompt;
            }
            decision => {
                return Ok(GoalSupervisedRun {
                    state,
                    events,
                    decision,
                    final_text: turn_final_text,
                    history,
                    loops_run,
                });
            }
        }
    }
}

fn ensure_goal_terminal_tool(config: &mut ToolLoopConfig, tool_name: &str) {
    let terminal_tools = config.terminal_tools.get_or_insert_with(Vec::new);
    if !terminal_tools.iter().any(|name| name == tool_name) {
        terminal_tools.push(tool_name.to_string());
    }
}

fn apply_supervisor_events(state: &mut GoalState, events: &[GoalEventEnvelope]) {
    for envelope in events {
        let _ = super::state::apply(state, envelope);
    }
}

pub fn goal_turn_outcome_from_tool_loop_turn(turn: &ToolLoopTurn) -> GoalTurnOutcome {
    if let Some(claim) = latest_submit_goal_complete_claim(turn) {
        return GoalTurnOutcome::CompletionClaim { claim };
    }
    if let Some(record) = latest_update_goal_progress_record(turn) {
        return GoalTurnOutcome::ProgressUpdate { record };
    }
    if !turn.final_text.trim().is_empty() {
        return GoalTurnOutcome::FinalTextWithoutClaim {
            text: turn.final_text.clone(),
        };
    }
    GoalTurnOutcome::Progressing {
        last_assistant_text: None,
    }
}

fn latest_submit_goal_complete_claim(turn: &ToolLoopTurn) -> Option<GoalCompletionClaim> {
    latest_assistant_tool_arguments(turn, "submit_goal_complete")
        .and_then(|value| serde_json::from_value(value).ok())
}

fn latest_update_goal_progress_record(turn: &ToolLoopTurn) -> Option<GoalProgressRecord> {
    latest_assistant_tool_arguments(turn, "update_goal_progress")
        .and_then(|value| serde_json::from_value(value).ok())
}

fn latest_assistant_tool_arguments(
    turn: &ToolLoopTurn,
    tool_name: &str,
) -> Option<serde_json::Value> {
    turn.history.iter().rev().find_map(|message| {
        let Message::Assistant { content, .. } = message else {
            return None;
        };
        let items = content.iter().collect::<Vec<_>>();
        items.into_iter().rev().find_map(|item| {
            let AssistantContent::ToolCall(call) = item else {
                return None;
            };
            if call.function.name != tool_name {
                return None;
            }
            normalize_tool_arguments_value(&call.function.arguments)
        })
    })
}

fn normalize_tool_arguments_value(value: &serde_json::Value) -> Option<serde_json::Value> {
    match value {
        serde_json::Value::String(raw) => serde_json::from_str(raw).ok(),
        other => Some(other.clone()),
    }
}

/// Helper for the family of outcomes that all map to a single
/// `Blocked { reason }` event + `AwaitUser { question }`. Keeps the
/// match arms readable.
fn block_and_await(
    clock: &dyn GoalEventClock,
    goal_id: Uuid,
    reason: GoalBlockReason,
    question: String,
) -> GoalSupervisorStep {
    let event = GoalEventEnvelope {
        envelope_id: clock.mint_envelope_id(),
        goal_id,
        recorded_at: clock.now(),
        event: GoalEvent::Blocked {
            reason,
            requested_input: Some(question.clone()),
        },
    };
    GoalSupervisorStep {
        events: vec![event],
        decision: GoalLoopDecision::AwaitUser { question },
    }
}

/// Apply the in-flight events to a clone of `state`, returning the
/// post-event state. Used for continuation-prompt building so the
/// prompt sees the up-to-date blockers / pending claim. Errors from
/// `apply` are best-effort logged via `Result` discarding — the
/// supervisor ships only events it has just constructed and should
/// always pass apply (the schema floors are aligned with what the
/// supervisor emits).
fn state_with_events_applied(state: &GoalState, events: &[GoalEventEnvelope]) -> GoalState {
    let mut next = state.clone();
    for envelope in events {
        let _ = super::state::apply(&mut next, envelope);
    }
    next
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use chrono::TimeZone;
    use serde_json::json;

    use super::*;
    use crate::internal::ai::{
        agent::runtime::tool_loop::{ToolLoopConfig, ToolLoopObserver},
        completion::{
            AssistantContent, CompletionError, CompletionModel, CompletionRequest,
            CompletionResponse, Function, Message, Text, ToolCall, UserContent,
        },
        goal::{
            DefaultGoalContinuationPromptBuilder, DeterministicGoalVerifier, GoalActor, GoalBudget,
            GoalCompletionClaim, GoalCriterion, GoalEvidencePolicy, GoalEvidenceRef,
            GoalEvidenceTarget, GoalSpec, GoalStatus, GoalVerificationRecord, RecentToolCall,
        },
        tools::{ToolRegistry, handlers::SubmitGoalCompleteHandler},
    };

    fn fixture_now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 5, 8, 13, 0, 0).unwrap()
    }

    fn fixture_spec() -> GoalSpec {
        GoalSpec::new(
            Uuid::parse_str("00000000-0000-0000-0000-00000000a1a1").unwrap(),
            "thread-1",
            "session-1",
            "deliver feature X",
            vec![GoalCriterion {
                id: "patch".to_string(),
                description: "edit landed".to_string(),
                required: true,
                verifier_hint: None,
                requires_workspace_change: true,
            }],
            Vec::new(),
            GoalEvidencePolicy::Standard,
            GoalBudget::default(),
            fixture_now(),
            GoalActor::User { id: None },
        )
        .expect("fixture spec must construct")
    }

    /// Deterministic clock — emits a stable sequence of envelope ids
    /// and an advancing wall-clock so tests can assert exact
    /// envelope contents.
    struct FixedClock {
        next: Mutex<u128>,
    }

    impl FixedClock {
        fn new() -> Self {
            Self {
                next: Mutex::new(1),
            }
        }
    }

    impl GoalEventClock for FixedClock {
        fn mint_envelope_id(&self) -> Uuid {
            let mut next = self.next.lock().expect("fixed clock mutex poisoned");
            let n = *next;
            *next += 1;
            Uuid::from_u128(n)
        }

        fn now(&self) -> DateTime<Utc> {
            // Each call advances by 1 second so successive envelopes
            // pass the apply() monotonic guard.
            let n = *self.next.lock().expect("fixed clock mutex poisoned");
            fixture_now() + chrono::Duration::seconds(n as i64)
        }
    }

    /// Verifier ctx fixture — minimal accept-path data so the
    /// happy-path supervisor test can drive a CompletionClaim
    /// through to an Accept.
    struct AcceptingCtx;

    impl GoalVerifierContext for AcceptingCtx {
        fn file_sha256(&self, _path: &str) -> Option<String> {
            Some("deadbeef".to_string())
        }

        fn recent_tool_results(&self) -> Vec<RecentToolCall> {
            Vec::new()
        }

        fn changed_files(&self) -> Vec<String> {
            vec!["src/feature.rs".to_string()]
        }

        fn now(&self) -> DateTime<Utc> {
            fixture_now()
        }

        fn finalised_by(&self) -> GoalActor {
            GoalActor::System {
                reason: "verifier accepted".to_string(),
            }
        }

        fn total_spent_micro_usd(&self) -> u64 {
            500_000
        }

        fn elapsed_wall_clock_seconds(&self) -> u64 {
            300
        }

        fn continuation_loops_used(&self) -> u32 {
            2
        }
    }

    fn fixture_supervisor()
    -> GoalSupervisor<DeterministicGoalVerifier, DefaultGoalContinuationPromptBuilder> {
        GoalSupervisor {
            stop_policy: GoalStopPolicy::GoalBound {
                goal_id: fixture_spec().goal_id,
            },
            verifier: DeterministicGoalVerifier,
            prompt_builder: DefaultGoalContinuationPromptBuilder,
        }
    }

    #[derive(Default)]
    struct TestObserver;

    impl ToolLoopObserver for TestObserver {}

    /// Happy path: model emits a well-formed CompletionClaim →
    /// supervisor appends CompletionClaimed + Completed and returns
    /// `Completed { report }`.
    #[test]
    fn completion_claim_accepted_yields_completed_decision() {
        let spec = fixture_spec();
        let state = GoalState::from_spec(spec);
        let claim = GoalCompletionClaim {
            summary: "shipped".to_string(),
            completed_criteria: vec!["patch".to_string()],
            evidence_refs: vec![GoalEvidenceRef {
                criterion_id: Some("patch".to_string()),
                target: GoalEvidenceTarget::File {
                    path: "src/feature.rs".to_string(),
                    sha256: "deadbeef".to_string(),
                },
                description: "edit".to_string(),
            }],
            verification: vec![GoalVerificationRecord {
                criterion_id: "patch".to_string(),
                method: "cargo check".to_string(),
                passed: true,
                output_summary: None,
            }],
            residual_risks: Vec::new(),
        };
        let supervisor = fixture_supervisor();
        let step = supervisor.step(
            &state,
            GoalTurnOutcome::CompletionClaim { claim },
            &AcceptingCtx,
            &FixedClock::new(),
        );
        assert_eq!(step.events.len(), 2);
        assert!(matches!(
            step.events[0].event,
            GoalEvent::CompletionClaimed(_)
        ));
        assert!(matches!(step.events[1].event, GoalEvent::Completed(_)));
        let GoalLoopDecision::Completed { report } = step.decision else {
            panic!("expected Completed decision");
        };
        assert_eq!(report.completed_criteria, vec!["patch".to_string()]);
        assert_eq!(report.claim_envelope_id, step.events[0].envelope_id);
    }

    /// Verifier rejection: supervisor appends CompletionClaimed +
    /// CompletionRejected, returns `Continue { prompt }` with the
    /// missing items surfaced in the prompt.
    #[test]
    fn completion_claim_rejected_yields_continue_decision() {
        let spec = fixture_spec();
        let state = GoalState::from_spec(spec);
        // Claim omits evidence so the verifier rejects on Rule 2.
        let claim = GoalCompletionClaim {
            summary: "no evidence".to_string(),
            completed_criteria: vec!["patch".to_string()],
            evidence_refs: Vec::new(),
            verification: vec![GoalVerificationRecord {
                criterion_id: "patch".to_string(),
                method: "x".to_string(),
                passed: true,
                output_summary: None,
            }],
            residual_risks: Vec::new(),
        };
        let supervisor = fixture_supervisor();
        let step = supervisor.step(
            &state,
            GoalTurnOutcome::CompletionClaim { claim },
            &AcceptingCtx,
            &FixedClock::new(),
        );
        assert_eq!(step.events.len(), 2);
        assert!(matches!(
            step.events[0].event,
            GoalEvent::CompletionClaimed(_)
        ));
        assert!(matches!(
            step.events[1].event,
            GoalEvent::CompletionRejected { .. }
        ));
        let GoalLoopDecision::Continue { prompt } = step.decision else {
            panic!("expected Continue decision, got {:?}", step.decision);
        };
        assert!(prompt.contains("Verifier rejected"));
    }

    /// Integration path: the Goal supervisor wraps the normal
    /// `run_tool_loop`. Turn 1 returns plain final text and therefore
    /// must not idle the session; the wrapper records progress and
    /// re-enters with a continuation prompt. Turn 2 calls
    /// `submit_goal_complete`; the wrapper parses the tool call,
    /// invokes the deterministic verifier, and returns Completed.
    #[tokio::test]
    async fn supervised_tool_loop_continues_after_final_text_until_completion_claim_accepts() {
        #[derive(Clone)]
        struct TwoTurnGoalModel {
            calls: Arc<AtomicUsize>,
        }

        impl CompletionModel for TwoTurnGoalModel {
            type Response = ();

            async fn completion(
                &self,
                request: CompletionRequest,
            ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
                let call = self.calls.fetch_add(1, Ordering::SeqCst);
                if call == 0 {
                    return Ok(CompletionResponse {
                        content: vec![AssistantContent::Text(Text {
                            text: "I think we're done.".to_string(),
                        })],
                        reasoning_content: None,
                        raw_response: (),
                    });
                }

                let last_user_text = request.chat_history.iter().rev().find_map(|message| {
                    let Message::User { content } = message else {
                        return None;
                    };
                    content.iter().find_map(|item| match item {
                        UserContent::Text(text) => Some(text.text.as_str()),
                        _ => None,
                    })
                });
                assert!(
                    last_user_text.is_some_and(|text| text.contains("submit_goal_complete")),
                    "second turn must be driven by the continuation prompt"
                );
                Ok(CompletionResponse {
                    content: vec![AssistantContent::ToolCall(ToolCall {
                        id: "call-goal-complete".to_string(),
                        name: "submit_goal_complete".to_string(),
                        function: Function {
                            name: "submit_goal_complete".to_string(),
                            arguments: json!({
                                "summary": "edit landed and tests passed",
                                "completed_criteria": ["patch"],
                                "evidence_refs": [{
                                    "criterion_id": "patch",
                                    "target": {
                                        "kind": "file",
                                        "path": "src/feature.rs",
                                        "sha256": "deadbeef"
                                    },
                                    "description": "feature edit landed"
                                }],
                                "verification": [{
                                    "criterion_id": "patch",
                                    "method": "cargo check",
                                    "passed": true
                                }],
                                "residual_risks": []
                            }),
                        },
                    })],
                    reasoning_content: None,
                    raw_response: (),
                })
            }
        }

        let temp = tempfile::tempdir().unwrap();
        let mut registry = ToolRegistry::with_working_dir(temp.path().to_path_buf());
        registry.register("submit_goal_complete", Arc::new(SubmitGoalCompleteHandler));
        let calls = Arc::new(AtomicUsize::new(0));
        let model = TwoTurnGoalModel {
            calls: Arc::clone(&calls),
        };
        let mut observer = TestObserver;
        let supervisor = fixture_supervisor();
        let state = GoalState::from_spec(fixture_spec());
        let clock = FixedClock::new();

        let run = run_goal_supervised_tool_loop(GoalSupervisedToolLoopRequest {
            model: &model,
            history: Vec::new(),
            initial_prompt: "Ship feature X".to_string(),
            registry: &registry,
            config: ToolLoopConfig::default(),
            observer: &mut observer,
            state,
            supervisor: &supervisor,
            verifier_ctx: &AcceptingCtx,
            clock: &clock,
        })
        .await
        .expect("supervised goal loop should complete");

        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(matches!(run.decision, GoalLoopDecision::Completed { .. }));
        assert_eq!(run.state.status, GoalStatus::Completed);
        assert_eq!(run.loops_run, 2);
        assert_eq!(run.events.len(), 3);
        assert!(matches!(
            run.events[0].event,
            GoalEvent::ProgressRecorded(_)
        ));
        assert!(matches!(
            run.events[1].event,
            GoalEvent::CompletionClaimed(_)
        ));
        assert!(matches!(run.events[2].event, GoalEvent::Completed(_)));
    }

    /// Final text without a claim: supervisor synthesises a
    /// ProgressRecorded so the audit log captures the narrative,
    /// then continues with a continuation prompt nudging the model
    /// back into the completion protocol.
    #[test]
    fn final_text_without_claim_synthesises_progress_and_continues() {
        let spec = fixture_spec();
        let state = GoalState::from_spec(spec);
        let supervisor = fixture_supervisor();
        let step = supervisor.step(
            &state,
            GoalTurnOutcome::FinalTextWithoutClaim {
                text: "I think we're done".to_string(),
            },
            &AcceptingCtx,
            &FixedClock::new(),
        );
        assert_eq!(step.events.len(), 1);
        assert!(matches!(
            step.events[0].event,
            GoalEvent::ProgressRecorded(_)
        ));
        let GoalLoopDecision::Continue { prompt } = step.decision else {
            panic!("expected Continue decision, got {:?}", step.decision);
        };
        assert!(prompt.contains("Final text alone does not complete a Goal"));
    }

    /// Approval denied: supervisor appends Blocked + returns
    /// AwaitUser. The Goal stays Active (non-terminal pause) so
    /// `--resume` can pick it up.
    #[test]
    fn approval_denied_yields_blocked_event_and_await_user() {
        let spec = fixture_spec();
        let state = GoalState::from_spec(spec);
        let supervisor = fixture_supervisor();
        let step = supervisor.step(
            &state,
            GoalTurnOutcome::ApprovalDenied {
                denied_tool: "shell".to_string(),
                denied_args_summary: Some("rm -rf /".to_string()),
                reason: "destructive".to_string(),
            },
            &AcceptingCtx,
            &FixedClock::new(),
        );
        assert_eq!(step.events.len(), 1);
        let GoalEvent::Blocked { reason, .. } = &step.events[0].event else {
            panic!("expected Blocked event");
        };
        assert!(matches!(reason, GoalBlockReason::ApprovalDenied { .. }));
        assert!(matches!(step.decision, GoalLoopDecision::AwaitUser { .. }));
    }

    /// Hard budget cap → Blocked(BudgetApprovalRequired) +
    /// AwaitUser. The doc forbids transitioning to Completed once a
    /// cap is exhausted.
    #[test]
    fn budget_hard_cap_yields_blocked_and_await_user() {
        let spec = fixture_spec();
        let state = GoalState::from_spec(spec);
        let supervisor = fixture_supervisor();
        let step = supervisor.step(
            &state,
            GoalTurnOutcome::BudgetHardCap {
                cap_micro_usd: 1_000_000,
                spent_micro_usd: 1_500_000,
            },
            &AcceptingCtx,
            &FixedClock::new(),
        );
        let GoalEvent::Blocked { reason, .. } = &step.events[0].event else {
            panic!("expected Blocked event");
        };
        assert!(matches!(
            reason,
            GoalBlockReason::BudgetApprovalRequired { .. }
        ));
        assert!(matches!(step.decision, GoalLoopDecision::AwaitUser { .. }));
    }

    /// User cancel → Cancelled event + Cancelled decision.
    #[test]
    fn user_cancel_yields_cancelled_event_and_decision() {
        let spec = fixture_spec();
        let state = GoalState::from_spec(spec);
        let supervisor = fixture_supervisor();
        let step = supervisor.step(
            &state,
            GoalTurnOutcome::UserCancelled {
                reason: "user pressed Ctrl-C".to_string(),
                cancelled_by: GoalActor::User { id: None },
            },
            &AcceptingCtx,
            &FixedClock::new(),
        );
        assert_eq!(step.events.len(), 1);
        assert!(matches!(step.events[0].event, GoalEvent::Cancelled { .. }));
        assert!(matches!(step.decision, GoalLoopDecision::Cancelled));
    }

    /// Plain progressing turn: no events, just a continuation
    /// prompt. The legacy tool-loop events landed earlier in the
    /// JSONL, so the supervisor only nudges.
    #[test]
    fn progressing_yields_no_events_just_a_continue_prompt() {
        let spec = fixture_spec();
        let state = GoalState::from_spec(spec);
        let supervisor = fixture_supervisor();
        let step = supervisor.step(
            &state,
            GoalTurnOutcome::Progressing {
                last_assistant_text: Some("ran cargo check".to_string()),
            },
            &AcceptingCtx,
            &FixedClock::new(),
        );
        assert!(step.events.is_empty());
        let GoalLoopDecision::Continue { prompt } = step.decision else {
            panic!("expected Continue decision, got {:?}", step.decision);
        };
        assert!(prompt.contains("Continue the work"));
    }

    /// Default prompt builder lists the still-pending required
    /// criteria so the model knows what's left.
    #[test]
    fn continuation_prompt_lists_pending_required_criteria() {
        let spec = fixture_spec();
        let state = GoalState::from_spec(spec);
        let prompt = DefaultGoalContinuationPromptBuilder.build(
            &state,
            &GoalTurnOutcome::Progressing {
                last_assistant_text: None,
            },
        );
        assert!(prompt.contains("Required criteria still pending"));
        assert!(prompt.contains("patch"));
    }

    /// The CompletionClaim arm builds the next-turn prompt against
    /// the *post-rejection* state, so the rejection blocker is
    /// visible.
    #[test]
    fn rejection_prompt_surfaces_blocker_from_post_event_state() {
        let spec = fixture_spec();
        let state = GoalState::from_spec(spec);
        let claim = GoalCompletionClaim {
            summary: "no evidence".to_string(),
            completed_criteria: vec!["patch".to_string()],
            evidence_refs: Vec::new(),
            verification: vec![GoalVerificationRecord {
                criterion_id: "patch".to_string(),
                method: "x".to_string(),
                passed: true,
                output_summary: None,
            }],
            residual_risks: Vec::new(),
        };
        let supervisor = fixture_supervisor();
        let step = supervisor.step(
            &state,
            GoalTurnOutcome::CompletionClaim { claim },
            &AcceptingCtx,
            &FixedClock::new(),
        );
        let GoalLoopDecision::Continue { prompt } = step.decision else {
            panic!("expected Continue decision");
        };
        assert!(prompt.contains("Missing"));
    }

    /// A supervisor's own emitted events fold cleanly into
    /// `apply()` — defense-in-depth that the supervisor never emits
    /// envelopes the schema apply path would refuse.
    #[test]
    fn supervisor_events_apply_cleanly_via_state_apply() {
        let spec = fixture_spec();
        let mut state = GoalState::from_spec(spec.clone());
        let claim = GoalCompletionClaim {
            summary: "shipped".to_string(),
            completed_criteria: vec!["patch".to_string()],
            evidence_refs: vec![GoalEvidenceRef {
                criterion_id: Some("patch".to_string()),
                target: GoalEvidenceTarget::File {
                    path: "src/feature.rs".to_string(),
                    sha256: "deadbeef".to_string(),
                },
                description: "edit".to_string(),
            }],
            verification: vec![GoalVerificationRecord {
                criterion_id: "patch".to_string(),
                method: "cargo check".to_string(),
                passed: true,
                output_summary: None,
            }],
            residual_risks: Vec::new(),
        };
        let supervisor = fixture_supervisor();
        let step = supervisor.step(
            &state,
            GoalTurnOutcome::CompletionClaim { claim },
            &AcceptingCtx,
            &FixedClock::new(),
        );
        for envelope in &step.events {
            super::super::state::apply(&mut state, envelope)
                .expect("supervisor-emitted envelope must apply cleanly");
        }
        assert_eq!(state.status, GoalStatus::Completed);
        assert!(state.status.is_terminal());
    }
}
