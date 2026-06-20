//! Goal continuation-prompt builder — extracted from
//! [`super::supervisor`] for OC-Phase 6 P6.3 file-organisation parity
//! with `docs/development/commands/_general.md:1469` (which lists
//! `goal/prompt.rs` as the canonical home for the builder).
//!
//! The builder is a `GoalSupervisor::step()` collaborator that turns a
//! [`GoalTurnOutcome`] plus the post-step [`GoalState`] into a
//! continuation prompt string the assistant sees on the next turn.
//! Per opencode.md:619-625 the supervisor's prompt MUST NOT stuff raw
//! transcripts back into Goal events / prompts; [`trimmed_excerpt`]
//! enforces the 320-byte cap shared by both the prompt builder and
//! the supervisor's own synthetic-progress synthesis (the
//! `FinalTextWithoutClaim` arm).
//!
//! # Why a separate file
//!
//! The builder's logic is opinionated prose, surfaces in user-visible
//! prompts, and is the natural P6.7 E2E golden-test target. Keeping
//! it adjacent to but separate from `supervisor.rs` makes the
//! supervisor's decision-tree easier to read (the prompt-shape detail
//! lives in one file) and gives future localisation work (CN/EN
//! bilingual prompts) a self-contained module to modify without
//! risking the supervisor's decision invariants.
//!
//! # Module boundary
//!
//! `prompt` depends on `super::supervisor::GoalTurnOutcome` (the
//! input enum) and `super::event::GoalBlockReason` (matched inside
//! the `CompletionClaim` arm to surface the verifier's rejection
//! reason). The supervisor depends on this module for the trait, the
//! default impl, and [`trimmed_excerpt`]. Rust resolves sibling
//! sub-module references in two passes so this is not a
//! cyclic-module error.

use super::{event::GoalBlockReason, state::GoalState, supervisor::GoalTurnOutcome};

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
///
/// Exposed at `pub(super)` so [`super::supervisor`] can apply the
/// same 320-byte cap when synthesising a `ProgressRecorded` from the
/// `FinalTextWithoutClaim` arm (the doc's rule 4 path).
pub(super) fn trimmed_excerpt(text: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `trimmed_excerpt` returns the input verbatim when it fits in
    /// the 320-byte cap. Mirrors the supervisor's synthetic-progress
    /// path on short final texts.
    #[test]
    fn trimmed_excerpt_returns_short_input_verbatim() {
        let short = "the model emitted a brief final note";
        assert_eq!(trimmed_excerpt(short), short);
    }

    /// `trimmed_excerpt` truncates inputs longer than 320 bytes and
    /// appends a `…` marker. Used in the prompt builder + the
    /// supervisor's `FinalTextWithoutClaim` synthetic-progress arm.
    #[test]
    fn trimmed_excerpt_truncates_long_input_with_marker() {
        let long = "x".repeat(500);
        let trimmed = trimmed_excerpt(&long);
        // 320 bytes of `x` + `…` marker (3 bytes UTF-8).
        assert_eq!(trimmed.len(), 320 + "…".len());
        assert!(trimmed.starts_with("xxxx"));
        assert!(trimmed.ends_with('…'));
    }

    /// Truncation must NOT slice a multi-byte char in half. The
    /// builder walks `end` back to a char boundary when the cap lands
    /// mid-codepoint.
    #[test]
    fn trimmed_excerpt_respects_char_boundaries() {
        // 200 ASCII chars + 100 4-byte CJK chars + filler = >320 bytes.
        let mut input = "x".repeat(200);
        input.push_str(&"漢".repeat(100));
        let trimmed = trimmed_excerpt(&input);
        // Must end with `…` and the byte before must be a complete
        // codepoint (no panic on slicing mid-utf8 sequence).
        assert!(trimmed.ends_with('…'));
        assert!(trimmed.is_char_boundary(trimmed.len() - "…".len()));
    }

    /// Leading / trailing whitespace is stripped before length check
    /// so the supervisor doesn't surface accidental indentation in
    /// its prompts.
    #[test]
    fn trimmed_excerpt_trims_whitespace() {
        assert_eq!(trimmed_excerpt("  hello  "), "hello");
    }
}
