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
    #[error("failed to write the commit edit buffer {path}: {detail}")]
    WriteBuffer { path: String, detail: String },
    #[error("failed to read the edited commit message: {detail}")]
    ReadBuffer { detail: String },
    #[error("editor '{editor}' exited abnormally; commit aborted")]
    Aborted { editor: String },
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
        .arg(format!("{editor} \"{}\"", path.display()))
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
