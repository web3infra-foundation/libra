//! Built-in TUI commands that are intercepted before reaching the AI model.
//!
//! These are distinct from the YAML-defined slash commands in `ai::commands`,
//! which expand into prompts sent to the model. Built-in commands perform
//! direct TUI actions (clear history, quit, show info, etc.).

/// A built-in TUI command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinCommand {
    Help,
    Clear,
    Model,
    Status,
    Plan,
    Intent,
    Mux,
    Quit,
}

impl BuiltinCommand {
    /// The command name (without leading `/`).
    pub fn name(self) -> &'static str {
        match self {
            Self::Help => "help",
            Self::Clear => "clear",
            Self::Model => "model",
            Self::Status => "status",
            Self::Plan => "plan",
            Self::Intent => "intent",
            Self::Mux => "mux",
            Self::Quit => "quit",
        }
    }

    /// Short description shown in the autocomplete popup.
    pub fn description(self) -> &'static str {
        match self {
            Self::Help => "Show available commands",
            Self::Clear => "Clear conversation history",
            Self::Model => "Show current model info",
            Self::Status => "Show current status",
            Self::Plan => "Generate validated IntentSpec from a request",
            Self::Intent => "IntentSpec utilities (show latest or execute it)",
            Self::Mux => "Control task mux view during parallel execution",
            Self::Quit => "Quit the application",
        }
    }

    /// All built-in commands in display order.
    pub fn all() -> &'static [BuiltinCommand] {
        &[
            Self::Help,
            Self::Clear,
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
    pub fn all_hints() -> Vec<(String, String)> {
        Self::all()
            .iter()
            .map(|cmd| (cmd.name().to_string(), cmd.description().to_string()))
            .collect()
    }
}

/// Try to parse input as a built-in command.
///
/// Returns `Some((command, remaining_args))` if the input matches a built-in,
/// or `None` if it should be handled by `CommandDispatcher` or sent to the model.
pub fn parse_builtin(input: &str) -> Option<(BuiltinCommand, &str)> {
    let input = input.trim();
    let rest = input.strip_prefix('/')?;
    let (name, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));

    BuiltinCommand::all()
        .iter()
        .find(|cmd| cmd.name().eq_ignore_ascii_case(name))
        .map(|&cmd| (cmd, args.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_known_commands() {
        assert_eq!(parse_builtin("/help"), Some((BuiltinCommand::Help, "")));
        assert_eq!(parse_builtin("/clear"), Some((BuiltinCommand::Clear, "")));
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
            parse_builtin("/mux next"),
            Some((BuiltinCommand::Mux, "next"))
        );
        assert_eq!(
            parse_builtin("/model gemini"),
            Some((BuiltinCommand::Model, "gemini"))
        );
    }

    #[test]
    fn parse_case_insensitive() {
        assert_eq!(parse_builtin("/HELP"), Some((BuiltinCommand::Help, "")));
        assert_eq!(parse_builtin("/Quit"), Some((BuiltinCommand::Quit, "")));
        assert_eq!(parse_builtin("/PLAN x"), Some((BuiltinCommand::Plan, "x")));
    }

    #[test]
    fn parse_unknown_returns_none() {
        assert!(parse_builtin("/unknown").is_none());
        assert!(parse_builtin("not a command").is_none());
        assert!(parse_builtin("").is_none());
    }

    #[test]
    fn all_hints_returns_all() {
        let hints = BuiltinCommand::all_hints();
        assert_eq!(hints.len(), 8);
        assert!(hints.iter().any(|(n, _)| n == "help"));
        assert!(hints.iter().any(|(n, _)| n == "quit"));
        assert!(hints.iter().any(|(n, _)| n == "plan"));
        assert!(hints.iter().any(|(n, _)| n == "intent"));
        assert!(hints.iter().any(|(n, _)| n == "mux"));
    }
}
