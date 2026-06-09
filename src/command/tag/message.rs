use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use super::{TagArgs, TagError};
use crate::internal::config::ConfigKv;

const MAX_TAG_MESSAGE_BYTES: u64 = 10 * 1024 * 1024;

pub(super) async fn resolve_annotation_message(args: &TagArgs) -> Result<Option<String>, TagError> {
    if let Some(message) = args.message.clone() {
        return non_empty_message(message);
    }
    if let Some(path) = args.file.as_deref() {
        return non_empty_message(read_message_file(path)?);
    }
    if args.annotate || args.edit {
        return non_empty_message(read_message_from_editor().await?);
    }
    Ok(None)
}

fn non_empty_message(message: String) -> Result<Option<String>, TagError> {
    if message.trim().is_empty() {
        return Err(TagError::EmptyAnnotationMessage);
    }
    Ok(Some(message))
}

fn read_message_file(path: &Path) -> Result<String, TagError> {
    let display = path.display().to_string();
    let canonical = path
        .canonicalize()
        .map_err(|source| TagError::ReadMessageFile {
            path: display.clone(),
            source,
        })?;
    let workdir = std::env::current_dir()
        .and_then(|dir| dir.canonicalize())
        .map_err(|source| TagError::ReadMessageFile {
            path: display.clone(),
            source,
        })?;
    if !canonical.starts_with(&workdir) {
        return Err(TagError::MessageFileOutsideWorkdir(display));
    }
    let metadata = fs::metadata(&canonical).map_err(|source| TagError::ReadMessageFile {
        path: display.clone(),
        source,
    })?;
    if metadata.len() > MAX_TAG_MESSAGE_BYTES {
        return Err(TagError::MessageFileTooLarge {
            path: display,
            limit: MAX_TAG_MESSAGE_BYTES,
        });
    }
    fs::read_to_string(&canonical).map_err(|source| TagError::ReadMessageFile {
        path: canonical.display().to_string(),
        source,
    })
}

async fn read_message_from_editor() -> Result<String, TagError> {
    let Some(editor) = resolve_editor().await else {
        return Err(TagError::EditorNotConfigured);
    };
    let tmp = tempfile::NamedTempFile::new().map_err(|source| TagError::ReadMessageFile {
        path: "tag message temp file".to_string(),
        source,
    })?;
    launch_editor(tmp.path(), &editor)?;
    fs::read_to_string(tmp.path()).map_err(|source| TagError::ReadMessageFile {
        path: tmp.path().display().to_string(),
        source,
    })
}

async fn resolve_editor() -> Option<String> {
    if let Some(value) = nonempty_env("GIT_EDITOR") {
        return Some(value);
    }
    if let Ok(Some(entry)) = ConfigKv::get("core.editor").await
        && let Some(editor) = runnable(entry.value)
    {
        return Some(editor);
    }
    nonempty_env("VISUAL").or_else(|| nonempty_env("EDITOR"))
}

fn nonempty_env(name: &str) -> Option<String> {
    std::env::var(name).ok().and_then(runnable)
}

fn runnable(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed == ":" || trimmed == "true" {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn launch_editor(path: &Path, editor: &str) -> Result<(), TagError> {
    let Some(mut argv) = shlex::split(editor) else {
        return Err(TagError::EditorCommandInvalid(editor.to_string()));
    };
    if argv.is_empty() {
        return Err(TagError::EditorCommandInvalid(editor.to_string()));
    }
    let program = argv.remove(0);
    let status = Command::new(program)
        .args(argv)
        .arg(PathBuf::from(path))
        .status()
        .map_err(|source| TagError::EditorFailed(format!("failed to start editor: {source}")))?;
    if !status.success() {
        return Err(TagError::EditorFailed(format!(
            "editor exited with status {}",
            status.code().unwrap_or(-1)
        )));
    }
    Ok(())
}
