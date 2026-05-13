//! Parser for the `/goal` family of TUI slash commands.
//!
//! Per `docs/improvement/opencode.md` lines 686-693, Goal mode exposes
//! three primary user-facing slash commands plus one criteria-tweak:
//!
//! - `/goal start <objective>` — create a fresh active Goal in this
//!   session. Refused (at the dispatch layer) if a Goal is already
//!   active; the user must `/goal cancel` or wait for completion
//!   first.
//! - `/goal status` — render the active Goal's objective, criteria,
//!   completed_criteria, blockers, and budget summary.
//! - `/goal cancel <reason>` — explicit user cancellation. The
//!   reason flows into the audit log (`GoalEvent::Cancelled`).
//! - `/goal criteria add <text>` — append an acceptance criterion
//!   to the active spec mid-Goal (`CriteriaRevised` event). Reserved
//!   for the next phase but parsed here so a typo is caught at
//!   parse time.
//!
//! This module is the typed parser only. It does not invoke the
//! supervisor (P6.3) or mutate session state — that lives in the
//! `app.rs` dispatch arm that calls into this parser. Keeping the
//! parser separate makes the slash-command grammar testable in
//! isolation and stable across UI/CLI/Code-Control surfaces (P6.6
//! reuses the same shape).

use crate::internal::ai::goal::{GoalSpecError, MAX_OBJECTIVE_LEN};

/// Typed view of a parsed `/goal …` slash command. Each variant
/// carries exactly the validated arguments the dispatch layer needs;
/// constructors below run the same shape rules `GoalSpec::new`
/// enforces (non-empty objective, ≤ `MAX_OBJECTIVE_LEN` bytes) so an
/// objective that would later fail spec construction is caught now,
/// at the surface, with a precise error.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GoalSubcommand {
    /// `/goal start <objective>` — seed a new active Goal in this
    /// session. The `objective` has been trimmed and validated.
    Start { objective: String },
    /// `/goal status` — render the active Goal's snapshot. No
    /// arguments.
    Status,
    /// `/goal cancel <reason>` — user-driven cancellation. The
    /// `reason` is required and trimmed; an empty reason is rejected
    /// at parse time so the audit log always has a human-meaningful
    /// string.
    Cancel { reason: String },
    /// `/goal criteria add <text>` — user-driven criteria revision.
    /// The `text` is the criterion's natural-language description;
    /// the dispatch layer mints an id (e.g. `user-<n>`) when it
    /// emits the corresponding `CriteriaRevised` envelope.
    CriteriaAdd { text: String },
}

/// Reasons a `/goal …` invocation cannot be parsed into a typed
/// subcommand. The dispatch layer renders these directly in a
/// system cell so the user sees an actionable hint instead of a
/// generic "unknown command" error.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum GoalCommandParseError {
    /// The user typed `/goal` with no subcommand or only whitespace
    /// after the verb.
    #[error(
        "Usage: /goal <start|status|cancel|criteria> …\n  \
         /goal start <objective>            Create an active Goal\n  \
         /goal status                       Show the active Goal's snapshot\n  \
         /goal cancel <reason>              Cancel the active Goal\n  \
         /goal criteria add <description>   Append an acceptance criterion"
    )]
    MissingSubcommand,
    /// The first word after `/goal` was not one of the documented
    /// subcommands.
    #[error("Unknown `/goal` subcommand `{got}`. Expected one of: start, status, cancel, criteria")]
    UnknownSubcommand { got: String },
    /// `/goal start` was invoked without an objective string.
    #[error("`/goal start` requires a non-empty objective. Usage: /goal start <objective>")]
    StartMissingObjective,
    /// `/goal cancel` was invoked without a reason.
    #[error(
        "`/goal cancel` requires a non-empty reason for the audit log. \
         Usage: /goal cancel <reason>"
    )]
    CancelMissingReason,
    /// `/goal criteria` was invoked without `add <text>` (the only
    /// supported subverb in this version).
    #[error(
        "Usage: /goal criteria add <description> — `add` is the only supported \
         criteria verb in this build"
    )]
    CriteriaUsage,
    /// `/goal status` (or another arg-less form) was invoked with
    /// trailing text. Surface the noise rather than silently
    /// dropping it.
    #[error("`/goal {subcommand}` does not accept arguments; got `{got}`")]
    UnexpectedArguments {
        subcommand: &'static str,
        got: String,
    },
    /// The objective fails the same shape rules `GoalSpec::new`
    /// enforces — empty / whitespace-only or > MAX_OBJECTIVE_LEN
    /// bytes. Surface the same error variant the schema would
    /// produce so the dispatch layer can reuse one rendering path.
    #[error("`/goal start` objective failed validation: {source}")]
    InvalidObjective {
        #[source]
        source: GoalSpecError,
    },
}

/// Parse `args`, the trimmed argument tail that follows
/// [`crate::internal::tui::slash_command::BuiltinCommand::Goal`]. The
/// caller has already split off the leading `/goal` token, so `args`
/// is e.g. `"start ship the feature"` / `"status"` / `"cancel user
/// changed mind"` / `"criteria add tests pass"`.
pub fn parse_goal_subcommand(args: &str) -> Result<GoalSubcommand, GoalCommandParseError> {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return Err(GoalCommandParseError::MissingSubcommand);
    }
    let (verb, rest) = trimmed
        .split_once(char::is_whitespace)
        .map(|(v, r)| (v, r.trim()))
        .unwrap_or((trimmed, ""));
    match verb.to_ascii_lowercase().as_str() {
        "start" => parse_start(rest),
        "status" => parse_status(rest),
        "cancel" => parse_cancel(rest),
        "criteria" => parse_criteria(rest),
        other => Err(GoalCommandParseError::UnknownSubcommand {
            got: other.to_string(),
        }),
    }
}

fn parse_start(rest: &str) -> Result<GoalSubcommand, GoalCommandParseError> {
    let objective = rest.trim();
    if objective.is_empty() {
        return Err(GoalCommandParseError::StartMissingObjective);
    }
    validate_objective(objective)?;
    Ok(GoalSubcommand::Start {
        objective: objective.to_string(),
    })
}

fn parse_status(rest: &str) -> Result<GoalSubcommand, GoalCommandParseError> {
    if !rest.is_empty() {
        return Err(GoalCommandParseError::UnexpectedArguments {
            subcommand: "status",
            got: rest.to_string(),
        });
    }
    Ok(GoalSubcommand::Status)
}

fn parse_cancel(rest: &str) -> Result<GoalSubcommand, GoalCommandParseError> {
    let reason = rest.trim();
    if reason.is_empty() {
        return Err(GoalCommandParseError::CancelMissingReason);
    }
    Ok(GoalSubcommand::Cancel {
        reason: reason.to_string(),
    })
}

fn parse_criteria(rest: &str) -> Result<GoalSubcommand, GoalCommandParseError> {
    let trimmed = rest.trim();
    let (subverb, body) = trimmed
        .split_once(char::is_whitespace)
        .map(|(v, r)| (v, r.trim()))
        .unwrap_or((trimmed, ""));
    if !subverb.eq_ignore_ascii_case("add") || body.is_empty() {
        return Err(GoalCommandParseError::CriteriaUsage);
    }
    Ok(GoalSubcommand::CriteriaAdd {
        text: body.to_string(),
    })
}

/// Apply the same shape rules `GoalSpec::new` enforces on the
/// objective. Re-uses the schema's `GoalSpecError` variants so the
/// dispatch layer renders one consistent error path.
pub(crate) fn validate_objective(objective: &str) -> Result<(), GoalCommandParseError> {
    if objective.trim().is_empty() {
        return Err(GoalCommandParseError::InvalidObjective {
            source: GoalSpecError::EmptyObjective,
        });
    }
    let actual = objective.len();
    if actual > MAX_OBJECTIVE_LEN {
        return Err(GoalCommandParseError::InvalidObjective {
            source: GoalSpecError::ObjectiveTooLong {
                actual,
                max: MAX_OBJECTIVE_LEN,
            },
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_start_with_objective() {
        let cmd = parse_goal_subcommand("start ship the feature").unwrap();
        assert_eq!(
            cmd,
            GoalSubcommand::Start {
                objective: "ship the feature".to_string(),
            },
        );
    }

    #[test]
    fn parses_start_case_insensitive_verb() {
        let cmd = parse_goal_subcommand("START write the changelog").unwrap();
        assert_eq!(
            cmd,
            GoalSubcommand::Start {
                objective: "write the changelog".to_string(),
            },
        );
    }

    #[test]
    fn rejects_start_without_objective() {
        assert_eq!(
            parse_goal_subcommand("start").unwrap_err(),
            GoalCommandParseError::StartMissingObjective,
        );
        assert_eq!(
            parse_goal_subcommand("start    ").unwrap_err(),
            GoalCommandParseError::StartMissingObjective,
        );
    }

    #[test]
    fn rejects_oversized_start_objective() {
        let big = "x".repeat(MAX_OBJECTIVE_LEN + 1);
        let err = parse_goal_subcommand(&format!("start {big}")).unwrap_err();
        assert!(matches!(
            err,
            GoalCommandParseError::InvalidObjective {
                source: GoalSpecError::ObjectiveTooLong { .. }
            }
        ));
    }

    #[test]
    fn parses_status_with_no_args() {
        assert_eq!(
            parse_goal_subcommand("status").unwrap(),
            GoalSubcommand::Status,
        );
        assert_eq!(
            parse_goal_subcommand("STATUS").unwrap(),
            GoalSubcommand::Status,
        );
    }

    #[test]
    fn rejects_status_with_trailing_args() {
        let err = parse_goal_subcommand("status now").unwrap_err();
        assert_eq!(
            err,
            GoalCommandParseError::UnexpectedArguments {
                subcommand: "status",
                got: "now".to_string(),
            },
        );
    }

    #[test]
    fn parses_cancel_with_reason() {
        let cmd = parse_goal_subcommand("cancel user changed mind").unwrap();
        assert_eq!(
            cmd,
            GoalSubcommand::Cancel {
                reason: "user changed mind".to_string(),
            },
        );
    }

    #[test]
    fn rejects_cancel_without_reason() {
        assert_eq!(
            parse_goal_subcommand("cancel").unwrap_err(),
            GoalCommandParseError::CancelMissingReason,
        );
        assert_eq!(
            parse_goal_subcommand("cancel   ").unwrap_err(),
            GoalCommandParseError::CancelMissingReason,
        );
    }

    #[test]
    fn parses_criteria_add() {
        let cmd = parse_goal_subcommand("criteria add tests pass").unwrap();
        assert_eq!(
            cmd,
            GoalSubcommand::CriteriaAdd {
                text: "tests pass".to_string(),
            },
        );
    }

    #[test]
    fn rejects_criteria_without_add_subverb() {
        assert_eq!(
            parse_goal_subcommand("criteria").unwrap_err(),
            GoalCommandParseError::CriteriaUsage,
        );
        assert_eq!(
            parse_goal_subcommand("criteria remove x").unwrap_err(),
            GoalCommandParseError::CriteriaUsage,
        );
        // `add` with no body is also a usage error.
        assert_eq!(
            parse_goal_subcommand("criteria add").unwrap_err(),
            GoalCommandParseError::CriteriaUsage,
        );
    }

    #[test]
    fn rejects_unknown_subcommand() {
        let err = parse_goal_subcommand("explode now").unwrap_err();
        assert_eq!(
            err,
            GoalCommandParseError::UnknownSubcommand {
                got: "explode".to_string(),
            },
        );
    }

    #[test]
    fn rejects_empty_args() {
        assert_eq!(
            parse_goal_subcommand("").unwrap_err(),
            GoalCommandParseError::MissingSubcommand,
        );
        assert_eq!(
            parse_goal_subcommand("   ").unwrap_err(),
            GoalCommandParseError::MissingSubcommand,
        );
    }

    /// `validate_objective` is also used by the CLI flag path
    /// (`libra code --goal "<objective>"`), so its behaviour is
    /// pinned independently — both layers must reject the same
    /// shapes the schema would refuse later.
    #[test]
    fn validate_objective_matches_spec_rules() {
        assert!(validate_objective("ship feature X").is_ok());
        assert!(matches!(
            validate_objective("   ").unwrap_err(),
            GoalCommandParseError::InvalidObjective {
                source: GoalSpecError::EmptyObjective
            }
        ));
        let big = "z".repeat(MAX_OBJECTIVE_LEN + 1);
        assert!(matches!(
            validate_objective(&big).unwrap_err(),
            GoalCommandParseError::InvalidObjective {
                source: GoalSpecError::ObjectiveTooLong { .. }
            }
        ));
    }
}
