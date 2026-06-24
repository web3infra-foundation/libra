//! Shared "open `$EDITOR` on a scratch file" helper.
//!
//! Used by `commit -e` / bare `commit`. Editor resolution follows Git
//! precedence: `$GIT_EDITOR` → `core.editor` → `$VISUAL` → `$EDITOR`. An
//! *explicitly configured* editor runs even without a TTY (so scripted editors
//! work in tests and automation); the implicit `vi` fallback is the caller's
//! responsibility and should only be used on an interactive terminal.

use std::path::Path;

use crate::internal::config::ConfigKv;

/// Failure to launch the configured editor or read back its result.
#[derive(Debug, thiserror::Error)]
pub(crate) enum EditorError {
    #[error("failed to write the editor buffer {path}: {detail}")]
    WriteBuffer { path: String, detail: String },
    #[error("failed to read the edited buffer: {detail}")]
    ReadBuffer { detail: String },
    #[error("editor '{editor}' exited abnormally; edit aborted")]
    Aborted { editor: String },
}

/// Wrap `value` in single quotes for safe inclusion in a `sh -c` command line,
/// escaping any embedded single quotes (`'` → `'\''`). Used to pass the scratch
/// file path so a repository/storage path containing shell metacharacters
/// (`$()`, backticks, quotes, spaces) cannot be expanded or break out.
fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Resolve an *explicitly configured* editor command, mirroring Git precedence:
/// `$GIT_EDITOR` → `core.editor` → `$VISUAL` → `$EDITOR`. Returns `None` when
/// none is configured (the caller decides whether to fall back to `vi`, which
/// only makes sense on an interactive terminal).
pub(crate) async fn resolve_editor() -> Option<String> {
    if let Ok(value) = std::env::var("GIT_EDITOR")
        && !value.trim().is_empty()
    {
        return Some(value);
    }
    if let Ok(Some(entry)) = ConfigKv::get("core.editor").await
        && !entry.value.trim().is_empty()
    {
        return Some(entry.value);
    }
    for var in ["VISUAL", "EDITOR"] {
        if let Ok(value) = std::env::var(var)
            && !value.trim().is_empty()
        {
            return Some(value);
        }
    }
    None
}

/// Write `initial` to `path`, open `editor` on it, and return the edited
/// contents.
///
/// `abort_on_failure` selects the failure semantics:
/// - `true` (commit): a non-zero / unspawnable editor is an [`EditorError`].
/// - `false` (degrade): the original `initial` is returned unchanged on failure.
pub(crate) async fn edit_message(
    path: &Path,
    initial: &str,
    editor: &str,
    abort_on_failure: bool,
) -> Result<String, EditorError> {
    std::fs::write(path, initial).map_err(|error| EditorError::WriteBuffer {
        path: path.display().to_string(),
        detail: error.to_string(),
    })?;
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!(
            "{editor} {}",
            shell_single_quote(&path.display().to_string())
        ))
        .status();
    match status {
        Ok(code) if code.success() => {
            std::fs::read_to_string(path).map_err(|error| EditorError::ReadBuffer {
                detail: error.to_string(),
            })
        }
        _ if abort_on_failure => Err(EditorError::Aborted {
            editor: editor.to_string(),
        }),
        _ => Ok(initial.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::shell_single_quote;

    #[test]
    fn shell_single_quote_neutralizes_metacharacters() {
        // Ordinary paths are simply wrapped.
        assert_eq!(
            shell_single_quote("/tmp/x/COMMIT_EDITMSG"),
            "'/tmp/x/COMMIT_EDITMSG'"
        );
        // Command-substitution / backticks / spaces are inert inside single quotes.
        assert_eq!(
            shell_single_quote("/tmp/$(touch pwned)/f"),
            "'/tmp/$(touch pwned)/f'"
        );
        // An embedded single quote is escaped via the '\'' idiom, so the quoting
        // cannot be broken out of.
        assert_eq!(shell_single_quote("a'b"), "'a'\\''b'");
    }
}
