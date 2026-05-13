//! Shared installer helpers for provider-specific hook setup.
//!
//! Each provider (Claude, Gemini, …) has its own settings file format, but they
//! all share three concerns: locating the project root, deciding which Libra
//! binary path to embed in hook commands, and atomically updating a JSON
//! settings file. This module centralises those primitives so the per-provider
//! installers can stay short and audit-friendly.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Serialize, de::DeserializeOwned};

use crate::utils::util;

/// Locate the working directory of the active Libra repository.
///
/// Boundary conditions: returns an error message that asks the user to run the
/// command from inside a repository. Callers must not silently fall back to the
/// current directory.
pub(super) fn resolve_project_root() -> Result<PathBuf> {
    util::try_working_dir()
        .context("hook installation commands must be run inside a Libra repository")
}

/// Pick the path to the Libra binary to embed in the provider's hook command and
/// return it as a shell-quoted string ready for inclusion in a config file.
///
/// Functional scope:
/// - When the user passes `--binary-path`, that path is honoured (relative paths
///   are resolved against the current directory).
/// - Otherwise the path of the currently running binary is used.
/// - In both cases the path is canonicalised so the resulting hook command does
///   not depend on the user's CWD when later executed by the provider.
///
/// Boundary conditions:
/// - Empty `--binary-path` is rejected with an actionable message.
/// - A canonicalisation failure (e.g. the binary was deleted between resolution
///   and `fs::canonicalize`) bubbles up with the path in the error message.
pub(super) fn resolve_hook_binary_path(input: Option<&str>) -> Result<String> {
    let path = match input {
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                bail!("invalid --binary-path: value cannot be empty");
            }
            let path = PathBuf::from(trimmed);
            if path.is_absolute() {
                path
            } else {
                std::env::current_dir()
                    .context("failed to read current directory while resolving --binary-path")?
                    .join(path)
            }
        }
        None => std::env::current_exe()
            .context("failed to resolve current Libra binary path for hook installation")?,
    };

    let canonical = fs::canonicalize(&path)
        .with_context(|| format!("failed to resolve Libra binary path '{}'", path.display()))?;

    Ok(quote_command_path(&canonical))
}

/// Quote a path for safe inclusion in a shell command stored on disk.
///
/// Functional scope: on Windows the path is wrapped in double quotes when it
/// contains whitespace or quote characters. On Unix-likes the path is left
/// unquoted when it consists solely of the conservative alphabet
/// `[A-Za-z0-9/._\-:]`, otherwise it is single-quoted with embedded apostrophes
/// escaped using the standard `'\''` form.
fn quote_command_path(path: &Path) -> String {
    let rendered = path.to_string_lossy();

    #[cfg(windows)]
    {
        if rendered.contains([' ', '\t', '"']) {
            return format!("\"{}\"", rendered.replace('"', "\\\""));
        }
        rendered.into_owned()
    }

    #[cfg(not(windows))]
    {
        if rendered
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | ':'))
        {
            return rendered.into_owned();
        }
        format!("'{}'", rendered.replace('\'', r#"'\''"#))
    }
}

/// Read a provider's settings JSON file, returning `T::default()` when it is
/// missing or empty.
///
/// Functional scope: callers can roundtrip-update settings without having to
/// branch on whether the file already exists. Whitespace-only files are also
/// treated as empty so an editor that saved an unfinished file does not derail
/// the installer.
///
/// Boundary conditions: a syntactically broken JSON file is reported as an error
/// referring to the file path, never as a silent default.
pub(super) fn load_json_settings<T>(path: &Path, provider_name: &str) -> Result<T>
where
    T: Default + DeserializeOwned,
{
    if !path.exists() {
        return Ok(T::default());
    }

    let content = fs::read_to_string(path).with_context(|| {
        format!(
            "failed to read {provider_name} settings file '{}'",
            path.display()
        )
    })?;
    if content.trim().is_empty() {
        return Ok(T::default());
    }

    serde_json::from_str(&content).map_err(|err| {
        anyhow!(
            "invalid {provider_name} settings JSON at '{}': {err}",
            path.display()
        )
    })
}

/// Atomically write `settings` to `path` using a temp-file + rename dance.
///
/// Functional scope:
/// - Creates parent directories as needed.
/// - Serialises with `to_vec_pretty` and appends a trailing newline so the file
///   is friendly to text editors.
/// - Writes a sibling `<path>.tmp`, then renames it on top of the target so the
///   file is never observed in a partial state.
///
/// Boundary conditions:
/// - On Windows `fs::rename` cannot overwrite an existing file, so the target is
///   removed first; the temp file is cleaned up if removal fails.
/// - All error messages include both the target and temp paths so operators can
///   recover from a partial write manually.
pub(super) fn write_json_settings<T>(path: &Path, settings: &T, provider_name: &str) -> Result<()>
where
    T: Serialize,
{
    let parent = path.parent().ok_or_else(|| {
        anyhow!(
            "invalid {provider_name} settings path without parent: '{}'",
            path.display()
        )
    })?;
    fs::create_dir_all(parent).with_context(|| {
        format!(
            "failed to create {provider_name} settings directory '{}'",
            parent.display()
        )
    })?;

    let mut data = serde_json::to_vec_pretty(settings)
        .with_context(|| format!("failed to serialize {provider_name} settings to JSON"))?;
    data.push(b'\n');

    let tmp_path = path.with_extension("json.tmp");
    fs::write(&tmp_path, &data).with_context(|| {
        format!(
            "failed to write temporary {provider_name} settings file '{}'",
            tmp_path.display()
        )
    })?;

    #[cfg(windows)]
    {
        if path.exists() {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => {
                    let _ = fs::remove_file(&tmp_path);
                    return Err(anyhow!(
                        "failed to replace existing {provider_name} settings file '{}': {err}",
                        path.display()
                    ));
                }
            }
        }
    }

    fs::rename(&tmp_path, path).with_context(|| {
        format!(
            "failed to replace {provider_name} settings file '{}' with '{}'",
            path.display(),
            tmp_path.display()
        )
    })?;
    Ok(())
}
