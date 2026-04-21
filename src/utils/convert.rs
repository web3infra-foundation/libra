//! Utilities for converting existing Git repositories into Libra repositories by reusing fetch and clone logic.

use std::path::{Path, PathBuf};

use crate::{
    command::{clone, fetch, init::InitError},
    internal::{branch::Branch, config::RemoteConfig},
    utils::output::{OutputConfig, ProgressMode},
};

#[derive(Debug, Clone)]
pub struct ConversionReport {
    pub source_git_dir: String,
    pub remote_url: String,
}

/// Convert an existing local Git repository into the current Libra repository.
///
/// This function assumes that `libra init` has already created the Libra
/// storage layout and database in the target directory. It will:
/// - Normalize the provided Git repository path.
/// - Fetch all objects and references from the Git repository.
/// - Configure the `origin` remote, local branches, and HEAD using the same
///   logic as the `clone` command.
pub async fn convert_from_git_repository(
    git_repo: &Path,
    is_bare: bool,
) -> Result<ConversionReport, InitError> {
    let git_dir = resolve_git_source_dir(git_repo)?;

    let url = git_dir.to_str().ok_or_else(|| InitError::InvalidUtf8Path {
        path: git_dir.clone(),
    })?;

    let remote = RemoteConfig {
        name: "origin".to_string(),
        url: url.to_string(),
    };

    let child_output = OutputConfig {
        quiet: true,
        progress: ProgressMode::None,
        json_format: None,
        pager: false,
        ..Default::default()
    };

    fetch::fetch_repository_safe(remote.clone(), None, false, None, &child_output)
        .await
        .map_err(|error| InitError::ConversionFailed {
            repo: git_dir.clone(),
            stage: "fetch",
            message: error.to_string(),
        })?;

    let remote_branches = Branch::list_branches_result(Some(&remote.name))
        .await
        .map_err(|error| InitError::ConversionFailed {
            repo: git_dir.clone(),
            stage: "setup",
            message: format!("failed to inspect fetched branches: {error}"),
        })?;
    if remote_branches.is_empty() {
        return Err(InitError::ConversionFailed {
            repo: git_dir.clone(),
            stage: "setup",
            message: "no refs fetched from source git repository".to_string(),
        });
    }

    clone::setup_repository(remote, None, !is_bare)
        .await
        .map(|_| ()) // discard SetupResult; convert only needs success/failure
        .map_err(|error| InitError::ConversionFailed {
            repo: git_dir.clone(),
            stage: "setup",
            message: error.to_string(),
        })?;

    Ok(ConversionReport {
        source_git_dir: git_dir.to_string_lossy().to_string(),
        remote_url: url.to_string(),
    })
}

pub(crate) fn resolve_git_source_dir(git_repo: &Path) -> Result<PathBuf, InitError> {
    let git_dir = if git_repo.join(".git").exists() {
        git_repo.join(".git")
    } else {
        git_repo.to_path_buf()
    };

    let valid = git_dir.join("HEAD").exists()
        && git_dir.join("config").exists()
        && git_dir.join("objects").exists();
    if !valid {
        return Err(InitError::InvalidGitRepository {
            path: git_repo.to_path_buf(),
        });
    }

    git_dir.canonicalize().map_err(InitError::Io)
}
