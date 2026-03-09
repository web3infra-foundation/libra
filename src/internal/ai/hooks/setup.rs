//! Shared installer helpers for provider-specific hook setup.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Serialize, de::DeserializeOwned};

use crate::utils::util;

pub(super) fn resolve_project_root() -> Result<PathBuf> {
    util::try_working_dir()
        .context("hook installation commands must be run inside a Libra repository")
}

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
