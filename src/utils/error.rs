//! User-facing CLI error rendering utilities.
//!
//! The CLI uses [`CliError`] as the single user-visible error type at the
//! process boundary. Domain errors inside commands should be mapped into
//! [`CliError`] with an explicit exit code and hint set instead of printing raw
//! internal causes to stderr.

use std::fmt;

/// Shared CLI result type.
pub type CliResult<T = ()> = Result<T, CliError>;

/// High-level CLI error classes used to decide prefixes and exit codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliErrorKind {
    UnknownCommand,
    ParseUsage,
    CommandUsage,
    Fatal,
    Failure,
}

/// Prefix level used for rendered messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorLevel {
    Fatal,
    Error,
}

/// Structured hint text rendered after the main error line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hint(String);

impl Hint {
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for Hint {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for Hint {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

/// User-facing CLI error with explicit rendering and exit semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError {
    kind: CliErrorKind,
    message: String,
    hints: Vec<Hint>,
    usage: Option<String>,
}

impl CliError {
    pub fn repo_not_found() -> Self {
        Self::fatal("not a libra repository (or any of the parent directories): .libra")
            .with_hint("run 'libra init' to create a repository in the current directory.")
    }

    pub fn unknown_command(message: impl Into<String>) -> Self {
        Self {
            kind: CliErrorKind::UnknownCommand,
            message: message.into(),
            hints: Vec::new(),
            usage: None,
        }
    }

    pub fn parse_usage(message: impl Into<String>) -> Self {
        Self {
            kind: CliErrorKind::ParseUsage,
            message: message.into(),
            hints: Vec::new(),
            usage: None,
        }
    }

    pub fn command_usage(message: impl Into<String>) -> Self {
        Self {
            kind: CliErrorKind::CommandUsage,
            message: message.into(),
            hints: Vec::new(),
            usage: None,
        }
    }

    pub fn fatal(message: impl Into<String>) -> Self {
        Self {
            kind: CliErrorKind::Fatal,
            message: message.into(),
            hints: Vec::new(),
            usage: None,
        }
    }

    pub fn failure(message: impl Into<String>) -> Self {
        Self {
            kind: CliErrorKind::Failure,
            message: message.into(),
            hints: Vec::new(),
            usage: None,
        }
    }

    /// Convert a legacy prefixed error string (e.g. `"fatal: ..."` or
    /// `"error: ..."`) into a structured [`CliError`].
    ///
    /// This is the shared bridge for commands whose inner implementation still
    /// returns `Result<(), String>` with a human-readable prefix.
    pub fn from_legacy_string(msg: impl Into<String>) -> Self {
        let raw = msg.into();
        let trimmed = raw.trim().to_string();
        if let Some(rest) = trimmed.strip_prefix("fatal: ") {
            Self::fatal(rest.to_string())
        } else if let Some(rest) = trimmed.strip_prefix("error: ") {
            Self::failure(rest.to_string())
        } else if let Some(rest) = trimmed.strip_prefix("warning: ") {
            // Strip the prefix so rendering doesn't produce "error: warning: …"
            Self::failure(rest.to_string())
        } else if let Some(rest) = trimmed.strip_prefix("usage: ") {
            Self::command_usage("invalid arguments").with_usage(format!("usage: {rest}"))
        } else {
            Self::failure(trimmed)
        }
    }

    pub fn kind(&self) -> CliErrorKind {
        self.kind
    }

    pub fn level(&self) -> Option<ErrorLevel> {
        match self.kind {
            CliErrorKind::Fatal => Some(ErrorLevel::Fatal),
            CliErrorKind::ParseUsage | CliErrorKind::CommandUsage | CliErrorKind::Failure => {
                Some(ErrorLevel::Error)
            }
            CliErrorKind::UnknownCommand => None,
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn usage(&self) -> Option<&str> {
        self.usage.as_deref()
    }

    pub fn hints(&self) -> &[Hint] {
        &self.hints
    }

    pub fn with_hint(mut self, hint: impl Into<Hint>) -> Self {
        if self.hints.len() >= 2 {
            return self;
        }

        let hint = normalize_hint_text(hint.into().0);
        if hint.trim().is_empty() {
            return self;
        }

        self.hints.push(Hint::new(hint));
        self
    }

    pub fn with_usage(mut self, usage: impl Into<String>) -> Self {
        let usage = usage.into();
        if !usage.trim().is_empty() {
            self.usage = Some(usage);
        }
        self
    }

    pub fn exit_code(&self) -> i32 {
        match self.kind {
            CliErrorKind::UnknownCommand => 1,
            CliErrorKind::ParseUsage => 2,
            CliErrorKind::CommandUsage => 129,
            CliErrorKind::Fatal => 128,
            CliErrorKind::Failure => 1,
        }
    }

    pub fn render(&self) -> String {
        let mut lines = Vec::new();
        match self.kind {
            CliErrorKind::UnknownCommand => lines.push(self.message.clone()),
            CliErrorKind::ParseUsage | CliErrorKind::CommandUsage | CliErrorKind::Failure => {
                lines.push(format!("error: {}", self.message));
            }
            CliErrorKind::Fatal => lines.push(format!("fatal: {}", self.message)),
        }

        if let Some(usage) = &self.usage
            && !usage.trim().is_empty()
        {
            lines.push(usage.trim_end().to_string());
        }

        for hint in &self.hints {
            lines.extend(render_hint(hint.as_str()));
        }

        lines.join("\n")
    }
}

fn normalize_hint_text(text: String) -> String {
    text.lines()
        .map(strip_hint_prefix)
        .collect::<Vec<_>>()
        .join("\n")
}

fn strip_hint_prefix(line: &str) -> String {
    let trimmed = line.trim_start();
    if let Some(stripped) = trimmed.strip_prefix("Hint:") {
        return stripped.trim_start().to_string();
    }
    if let Some(stripped) = trimmed.strip_prefix("hint:") {
        return stripped.trim_start().to_string();
    }
    line.to_string()
}

// NOTE: We use "Hint:" (capital H) rather than Git's lowercase "hint:". This is
// a deliberate stylistic choice for Libra — not a bug.
fn render_hint(text: &str) -> Vec<String> {
    text.lines().map(|line| format!("Hint: {}", line)).collect()
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render())
    }
}

impl std::error::Error for CliError {}

/// Print a user-facing error to stderr.
///
/// New command code should prefer returning [`CliError`] instead of printing
/// directly. This macro remains for legacy command paths during migration.
#[macro_export]
macro_rules! cli_error {
    ($prefix:expr => $err:expr) => {{
        eprintln!("{}: {}", $prefix, $err);
    }};
    ($err:expr, $($arg:tt)+) => {{
        eprint!($($arg)+);
        eprintln!(": {}", $err);
    }};
}

#[cfg(test)]
mod tests {
    use super::{CliError, CliErrorKind};

    #[test]
    fn fatal_render_uses_git_style_prefix() {
        let rendered = CliError::fatal("failed to open index").render();
        assert_eq!(rendered, "fatal: failed to open index");
    }

    #[test]
    fn repo_not_found_includes_standard_hint() {
        let rendered = CliError::repo_not_found().render();
        assert_eq!(
            rendered,
            "fatal: not a libra repository (or any of the parent directories): .libra\nHint: run 'libra init' to create a repository in the current directory."
        );
    }

    #[test]
    fn parse_usage_render_includes_usage_and_hints() {
        let rendered = CliError::parse_usage("unexpected argument '--bad'")
            .with_usage("Usage: libra add [OPTIONS] [PATHSPEC]...")
            .with_hint("use '--help' to see available options.")
            .render();
        assert_eq!(
            rendered,
            "error: unexpected argument '--bad'\nUsage: libra add [OPTIONS] [PATHSPEC]...\nHint: use '--help' to see available options."
        );
    }

    #[test]
    fn multiline_hint_prefixes_every_line() {
        let rendered = CliError::failure("name and email are not configured")
            .with_hint(
                "to configure, run:\n  libra config --global user.name \"Some One\"\n  libra config --global user.email \"someone@example.com\"",
            )
            .render();
        assert_eq!(
            rendered,
            "error: name and email are not configured\nHint: to configure, run:\nHint:   libra config --global user.name \"Some One\"\nHint:   libra config --global user.email \"someone@example.com\""
        );
    }

    #[test]
    fn unknown_command_has_no_error_prefix() {
        let err =
            CliError::unknown_command("libra: 'wat' is not a libra command. See 'libra --help'.");
        assert_eq!(err.kind(), CliErrorKind::UnknownCommand);
        assert_eq!(
            err.render(),
            "libra: 'wat' is not a libra command. See 'libra --help'."
        );
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn with_hint_strips_prefix_and_limits_count() {
        let rendered = CliError::failure("bad")
            .with_hint("hint: first")
            .with_hint("Hint: second")
            .with_hint("third")
            .render();
        assert_eq!(rendered, "error: bad\nHint: first\nHint: second");
    }

    #[test]
    fn from_legacy_string_strips_warning_prefix() {
        let err = CliError::from_legacy_string("warning: something off");
        assert_eq!(err.kind(), CliErrorKind::Failure);
        assert_eq!(err.render(), "error: something off");
    }

    #[test]
    fn from_legacy_string_handles_usage_prefix() {
        let err = CliError::from_legacy_string("usage: libra mv <source> <dest>");
        assert_eq!(err.kind(), CliErrorKind::CommandUsage);
        assert!(err.render().contains("usage: libra mv <source> <dest>"));
    }
}
