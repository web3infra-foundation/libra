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
    spec::GoalActor,
    state::GoalState,
    verifier::{GoalVerifier, GoalVerifierContext, GoalVerifyOutcome},
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

/// Continuation-prompt formatter. Default implementation
/// [`DefaultGoalContinuationPromptBuilder`] is sufficient for
/// OC-Phase 6; the trait shape lets P6.7 E2E tests plug a fake
/// builder for byte-stable golden assertions, and lets future
/// localisation work (CN/EN bilingual prompts) layer in without
/// touching the supervisor.
pub trait GoalContinuationPromptBuilder {
    fn build(&self, state: &GoalState, outcome: &GoalTurnOutcome) -> String;
}

/// Default continuation-prompt builder. The wording is opinionated
/// but stable: it lists the objective, the still-missing required
/// criteria, recent context (rejection reason / failed tool / final
/// text the model just gave), and the minimum next step. Phrased so
/// the model is nudged back into the completion protocol rather
/// than told what to do verbatim.
#[derive(Clone, Copy, Debug, Default)]
pub struct DefaultGoalContinuationPromptBuilder;

impl GoalContinuationPromptBuilder for DefaultGoalContinuationPromptBuilder {
    fn build(&self, state: &GoalState, outcome: &GoalTurnOutcome) -> String {
        let mut out = String::new();
        out.push_str("Goal still active.\n");
        out.push_str(&format!("Objective: {}\n", state.spec.objective));

        // Missing required criteria — the verifier will reject any
        // claim that doesn't cover these, so surface them every turn.
        let missing_required: Vec<&str> = state
            .spec
            .acceptance_criteria
            .iter()
            .filter(|c| c.required && !state.completed_criteria.contains(c.id.as_str()))
            .map(|c| c.id.as_str())
            .collect();
        if missing_required.is_empty() {
            out.push_str("All required criteria already satisfied; ");
            out.push_str("call `submit_goal_complete` when the workspace evidence is in place.\n");
        } else {
            out.push_str("Required criteria still pending: ");
            out.push_str(&missing_required.join(", "));
            out.push('\n');
        }

        // Outcome-specific nudge.
        match outcome {
            GoalTurnOutcome::Progressing {
                last_assistant_text,
            } => {
                if let Some(text) = last_assistant_text
                    && !text.trim().is_empty()
                {
                    out.push_str(&format!("Last assistant note: {}\n", trimmed_excerpt(text)));
                }
                out.push_str(
                    "Continue the work — call tools or `update_goal_progress` to record \
                     progress, then `submit_goal_complete` when you have evidence for every \
                     required criterion.\n",
                );
            }
            GoalTurnOutcome::FinalTextWithoutClaim { text } => {
                out.push_str(&format!("Last assistant text: {}\n", trimmed_excerpt(text)));
                out.push_str(
                    "Final text alone does not complete a Goal. Call `submit_goal_complete` \
                     with the full evidence list, or `update_goal_progress` if work remains.\n",
                );
            }
            GoalTurnOutcome::ProgressUpdate { record } => {
                out.push_str(&format!(
                    "Recorded progress: {}\n",
                    trimmed_excerpt(&record.summary)
                ));
                out.push_str("Drive the next step toward the remaining criteria.\n");
            }
            GoalTurnOutcome::CompletionClaim { .. } => {
                // Verifier rejected — `step()` already appended the
                // CompletionRejected event, so the missing list and
                // reason live in `state.blockers`.
                if let Some(blocker) = state.blockers.last()
                    && let GoalBlockReason::CompletionRejected { missing, reason } = &blocker.reason
                {
                    out.push_str(&format!(
                        "Verifier rejected the last claim: {}\nMissing: {}\n",
                        reason,
                        missing.join(", "),
                    ));
                }
                out.push_str(
                    "Address the missing items, then call `submit_goal_complete` again \
                     with the full evidence list.\n",
                );
            }
            // The remaining variants either lead to AwaitUser /
            // Cancelled / Completed (where no continuation prompt
            // is built) or are caller-internal signals that hit a
            // Blocked event the supervisor records. The
            // continuation prompt for the *next* turn (after the
            // user resolves the blocker) is built from the new
            // `state.blockers` and lands on the `Progressing` arm
            // above. Default to a generic nudge here so a caller
            // that asks for a prompt anyway gets something useful.
            _ => {
                if let Some(blocker) = state.blockers.last() {
                    out.push_str(&format!("Outstanding blocker: {:?}\n", blocker.reason,));
                }
                out.push_str("Resume by addressing the outstanding blocker.\n");
            }
        }

        out
    }
}

/// Trim and excerpt a long piece of text so the continuation prompt
/// stays bounded. The doc forbids stuffing raw transcripts back into
/// Goal events / prompts (opencode.md:619-625), and the supervisor's
/// generated prompt counts as one of those entry points.
fn trimmed_excerpt(text: &str) -> String {
    const MAX_EXCERPT_BYTES: usize = 320;
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
    use std::cell::Cell;

    use chrono::TimeZone;

    use super::*;
    use crate::internal::ai::goal::{
        DeterministicGoalVerifier, GoalActor, GoalBudget, GoalCompletionClaim, GoalCriterion,
        GoalEvidencePolicy, GoalEvidenceRef, GoalEvidenceTarget, GoalSpec, GoalStatus,
        GoalVerificationRecord, RecentToolCall,
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
        next: Cell<u128>,
    }

    impl FixedClock {
        fn new() -> Self {
            Self { next: Cell::new(1) }
        }
    }

    impl GoalEventClock for FixedClock {
        fn mint_envelope_id(&self) -> Uuid {
            let n = self.next.get();
            self.next.set(n + 1);
            Uuid::from_u128(n)
        }

        fn now(&self) -> DateTime<Utc> {
            // Each call advances by 1 second so successive envelopes
            // pass the apply() monotonic guard.
            let n = self.next.get();
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
