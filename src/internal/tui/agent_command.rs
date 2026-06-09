//! Typed parser for the `/agent` subcommand family (CEX-S2-16, Step 2.6).
//!
//! `/agent` controls individual sub-agent runs from the TUI. Step 2.6 requires
//! `/agent cancel <id>` to cancel a single run without ending the main session
//! (agent.md Step 2.6 应该完成的功能 (2)). The companion `/agents` (plural)
//! command lists runs; `/agent list` is accepted here as an alias so the
//! singular verb is self-contained.
//!
//! The parser is **pure** — it validates the verb and its argument and returns a
//! typed [`AgentSubcommand`] or an actionable [`AgentCommandParseError`]. The
//! dispatch layer in `app.rs` turns a parsed `Cancel` into the actual
//! cancellation request; routing is out of scope here. Mirrors the
//! `/goal` parser ([`super::goal_command::parse_goal_subcommand`]) so the two
//! command families share one shape.

/// A parsed `/agent …` subcommand.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AgentSubcommand {
    /// `/agent list` — list the current sub-agent runs (alias of `/agents`).
    List,
    /// `/agent cancel <id>` — cancel a single run by id. The `run_id` is
    /// trimmed and required; an empty id is rejected at parse time.
    Cancel { run_id: String },
}

/// Reasons a `/agent …` invocation cannot be parsed. The dispatch layer renders
/// these in a system cell so the user sees an actionable hint.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum AgentCommandParseError {
    /// `/agent` was typed with no subcommand or only whitespace after the verb.
    #[error(
        "Usage: /agent <list|cancel> …\n  \
         /agent list             List current sub-agent runs\n  \
         /agent cancel <id>      Cancel a single sub-agent run by id"
    )]
    MissingSubcommand,
    /// The first word after `/agent` was not a documented subcommand.
    #[error("Unknown `/agent` subcommand `{got}`. Expected one of: list, cancel")]
    UnknownSubcommand { got: String },
    /// `/agent cancel` was invoked without a run id.
    #[error("`/agent cancel` requires a run id. Usage: /agent cancel <id>")]
    CancelMissingId,
    /// An arg-less subcommand (`list`) was given trailing text.
    #[error("`/agent {subcommand}` does not accept arguments; got `{got}`")]
    UnexpectedArguments {
        subcommand: &'static str,
        got: String,
    },
}

/// Parse `args`, the trimmed argument tail that follows the `/agent` token
/// (e.g. `"cancel 1f2e…"` / `"list"`).
pub fn parse_agent_subcommand(args: &str) -> Result<AgentSubcommand, AgentCommandParseError> {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return Err(AgentCommandParseError::MissingSubcommand);
    }
    let (verb, rest) = trimmed
        .split_once(char::is_whitespace)
        .map(|(v, r)| (v, r.trim()))
        .unwrap_or((trimmed, ""));
    match verb.to_ascii_lowercase().as_str() {
        "list" => {
            if rest.is_empty() {
                Ok(AgentSubcommand::List)
            } else {
                Err(AgentCommandParseError::UnexpectedArguments {
                    subcommand: "list",
                    got: rest.to_string(),
                })
            }
        }
        "cancel" => {
            if rest.is_empty() {
                Err(AgentCommandParseError::CancelMissingId)
            } else {
                Ok(AgentSubcommand::Cancel {
                    run_id: rest.to_string(),
                })
            }
        }
        other => Err(AgentCommandParseError::UnknownSubcommand {
            got: other.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_list() {
        assert_eq!(parse_agent_subcommand("list"), Ok(AgentSubcommand::List));
        // Case-insensitive verb.
        assert_eq!(parse_agent_subcommand("LIST"), Ok(AgentSubcommand::List));
    }

    #[test]
    fn parses_cancel_with_id() {
        assert_eq!(
            parse_agent_subcommand("cancel 1f2e3d"),
            Ok(AgentSubcommand::Cancel {
                run_id: "1f2e3d".to_string()
            }),
        );
        // Surrounding whitespace on the id is trimmed.
        assert_eq!(
            parse_agent_subcommand("cancel    run-7   "),
            Ok(AgentSubcommand::Cancel {
                run_id: "run-7".to_string()
            }),
        );
    }

    #[test]
    fn empty_args_is_missing_subcommand() {
        assert_eq!(
            parse_agent_subcommand(""),
            Err(AgentCommandParseError::MissingSubcommand),
        );
        assert_eq!(
            parse_agent_subcommand("   "),
            Err(AgentCommandParseError::MissingSubcommand),
        );
    }

    #[test]
    fn cancel_without_id_is_rejected() {
        assert_eq!(
            parse_agent_subcommand("cancel"),
            Err(AgentCommandParseError::CancelMissingId),
        );
        assert_eq!(
            parse_agent_subcommand("cancel   "),
            Err(AgentCommandParseError::CancelMissingId),
        );
    }

    #[test]
    fn list_rejects_trailing_arguments() {
        assert_eq!(
            parse_agent_subcommand("list extra"),
            Err(AgentCommandParseError::UnexpectedArguments {
                subcommand: "list",
                got: "extra".to_string(),
            }),
        );
    }

    #[test]
    fn unknown_subcommand_is_rejected() {
        assert_eq!(
            parse_agent_subcommand("pause r1"),
            Err(AgentCommandParseError::UnknownSubcommand {
                got: "pause".to_string()
            }),
        );
    }

    #[test]
    fn error_messages_are_actionable() {
        // The missing-subcommand usage string lists both verbs.
        let usage = AgentCommandParseError::MissingSubcommand.to_string();
        assert!(usage.contains("/agent list"));
        assert!(usage.contains("/agent cancel <id>"));
    }
}
