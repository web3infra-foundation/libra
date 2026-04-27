//! Built-in TUI commands that are intercepted before reaching the AI model.
//!
//! These are distinct from the YAML-defined slash commands in `ai::commands`,
//! which expand into prompts sent to the model. Built-in commands perform
//! direct TUI actions (clear history, quit, show info, etc.) and never touch
//! the model. The composer calls [`parse_builtin`] on every Enter; if a match
//! is found the app handles it locally and never sends the text upstream.

/// A built-in TUI command.
///
/// Each variant maps 1:1 to a `/<word>` shortcut typed in the composer. The
/// enum is `Copy` because instances are cheap and frequently passed around the
/// dispatch table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinCommand {
    /// `/help` — render the in-app help screen.
    Help,
    /// `/clear` — wipe the visible transcript and conversation history.
    Clear,
    /// `/chat` — bypass the plan workflow and send a direct chat message.
    Chat,
    /// `/model` — print the current model and provider in a system cell.
    Model,
    /// `/status` — show the current agent status.
    Status,
    /// `/plan` — kick off the IntentSpec generation workflow.
    Plan,
    /// `/intent` — IntentSpec subcommands (show, execute, modify, cancel).
    Intent,
    /// `/mux` — focus / unfocus task panes during parallel DAG execution.
    Mux,
    /// `/quit` — exit the application cleanly.
    Quit,
}

impl BuiltinCommand {
    /// The command name (without leading `/`).
    ///
    /// Functional scope: provides the canonical identifier used both for
    /// matching user input (case-insensitive) and for rendering the
    /// autocomplete popup.
    pub fn name(self) -> &'static str {
        match self {
            Self::Help => "help",
            Self::Clear => "clear",
            Self::Chat => "chat",
            Self::Model => "model",
            Self::Status => "status",
            Self::Plan => "plan",
            Self::Intent => "intent",
            Self::Mux => "mux",
            Self::Quit => "quit",
        }
    }

    /// Short description shown in the autocomplete popup.
    ///
    /// Functional scope: human-readable single-line hint rendered next to the
    /// command name. Kept short to fit a one-line popup row at typical TUI
    /// widths.
    pub fn description(self) -> &'static str {
        match self {
            Self::Help => "Show available commands",
            Self::Clear => "Clear conversation history",
            Self::Chat => "Send a direct chat message without plan workflow",
            Self::Model => "Show current model info",
            Self::Status => "Show current status",
            Self::Plan => "Generate validated IntentSpec from a request",
            Self::Intent => "IntentSpec utilities (show latest or execute it)",
            Self::Mux => "Control task mux view during parallel execution",
            Self::Quit => "Quit the application",
        }
    }

    /// All built-in commands in display order.
    ///
    /// Functional scope: the canonical iteration order used by both the parser
    /// (first-match wins) and the autocomplete popup (top-to-bottom listing).
    /// Order is intentional — `help` comes first because it is the most
    /// useful entry-point for new users.
    pub fn all() -> &'static [BuiltinCommand] {
        &[
            Self::Help,
            Self::Clear,
            Self::Chat,
            Self::Model,
            Self::Status,
            Self::Plan,
            Self::Intent,
            Self::Mux,
            Self::Quit,
        ]
    }

    /// Return `(name, description)` pairs for all built-in commands,
    /// suitable for merging into the command autocomplete popup.
    ///
    /// Functional scope: convenience helper that allocates `String` copies so
    /// the popup, which mixes built-in entries with dynamically loaded YAML
    /// commands, can hold a single homogeneous `Vec<(String, String)>`.
    pub fn all_hints() -> Vec<(String, String)> {
        Self::all()
            .iter()
            .map(|cmd| (cmd.name().to_string(), cmd.description().to_string()))
            .collect()
    }
}

/// Try to parse input as a built-in command.
///
/// Functional scope: trims surrounding whitespace, requires a leading `/`, and
/// splits the remaining text into a command name and its argument tail.
/// Returns `Some((command, remaining_args))` if the leading word matches a
/// built-in, or `None` if it should be handled by `CommandDispatcher` (YAML
/// slash commands) or sent verbatim to the model.
///
/// Boundary conditions:
/// - Matching is case-insensitive — `/HELP` and `/help` both match `Help`.
/// - The first whitespace separates the command from its arguments; if no
///   whitespace exists the entire tail becomes the command name and `args` is
///   empty.
/// - Empty input or input that does not start with `/` returns `None` and is
///   handled by the regular composer path.
///
/// See: [`tests::parse_known_commands`], [`tests::parse_case_insensitive`],
/// [`tests::parse_unknown_returns_none`].
pub fn parse_builtin(input: &str) -> Option<(BuiltinCommand, &str)> {
    let input = input.trim();
    let rest = input.strip_prefix('/')?;
    // Split at the first whitespace into (name, args) pair; if no whitespace
    // exists the whole rest becomes the name and args is empty.
    let (name, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));

    BuiltinCommand::all()
        .iter()
        .find(|cmd| cmd.name().eq_ignore_ascii_case(name))
        .map(|&cmd| (cmd, args.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scenario: exhaustive happy-path matrix covering every built-in shortcut
    /// with and without a trailing argument string. Guards regressions that
    /// would silently route a built-in command into the AI model instead.
    #[test]
    fn parse_known_commands() {
        assert_eq!(parse_builtin("/help"), Some((BuiltinCommand::Help, "")));
        assert_eq!(parse_builtin("/clear"), Some((BuiltinCommand::Clear, "")));
        assert_eq!(
            parse_builtin("/chat what is this"),
            Some((BuiltinCommand::Chat, "what is this"))
        );
        assert_eq!(parse_builtin("/quit"), Some((BuiltinCommand::Quit, "")));
        assert_eq!(
            parse_builtin("/plan add auth"),
            Some((BuiltinCommand::Plan, "add auth"))
        );
        assert_eq!(
            parse_builtin("/intent show"),
            Some((BuiltinCommand::Intent, "show"))
        );
        assert_eq!(
            parse_builtin("/intent execute"),
            Some((BuiltinCommand::Intent, "execute"))
        );
        assert_eq!(
            parse_builtin("/intent modify allow network"),
            Some((BuiltinCommand::Intent, "modify allow network"))
        );
        assert_eq!(
            parse_builtin("/intent cancel"),
            Some((BuiltinCommand::Intent, "cancel"))
        );
        assert_eq!(
            parse_builtin("/mux next"),
            Some((BuiltinCommand::Mux, "next"))
        );
        assert_eq!(
            parse_builtin("/model gemini"),
            Some((BuiltinCommand::Model, "gemini"))
        );
    }

    /// Scenario: command names are matched case-insensitively because users
    /// might type `/HELP` from caps-lock without realising. Pin that behaviour.
    #[test]
    fn parse_case_insensitive() {
        assert_eq!(parse_builtin("/HELP"), Some((BuiltinCommand::Help, "")));
        assert_eq!(parse_builtin("/Quit"), Some((BuiltinCommand::Quit, "")));
        assert_eq!(parse_builtin("/PLAN x"), Some((BuiltinCommand::Plan, "x")));
    }

    /// Scenario: input that doesn't match any built-in returns `None` so the
    /// caller can either dispatch to the YAML command registry or forward to
    /// the model. Covers unknown commands, plain text, and empty input.
    #[test]
    fn parse_unknown_returns_none() {
        assert!(parse_builtin("/unknown").is_none());
        assert!(parse_builtin("not a command").is_none());
        assert!(parse_builtin("").is_none());
    }

    /// Scenario: the autocomplete popup must list every built-in command. This
    /// test pins both the count (catches accidental dropouts when the enum is
    /// extended) and the inclusion of the most-used entries.
    #[test]
    fn all_hints_returns_all() {
        let hints = BuiltinCommand::all_hints();
        assert_eq!(hints.len(), 9);
        assert!(hints.iter().any(|(n, _)| n == "help"));
        assert!(hints.iter().any(|(n, _)| n == "chat"));
        assert!(hints.iter().any(|(n, _)| n == "quit"));
        assert!(hints.iter().any(|(n, _)| n == "plan"));
        assert!(hints.iter().any(|(n, _)| n == "intent"));
        assert!(hints.iter().any(|(n, _)| n == "mux"));
    }
}
