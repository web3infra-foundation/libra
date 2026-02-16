//! Utilities for converting existing Git repositories into Libra repositories by reusing fetch and clone logic.

use std::{io, path::Path};

use crate::{
    command::{clone, fetch},
    internal::config::RemoteConfig,
};

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
) -> Result<(), crate::command::init::InitError> {
    if !git_repo.exists() {
        return Err(crate::command::init::InitError::Io(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "source git repository '{}' does not exist",
                git_repo.display()
            ),
        )));
    }

    let git_dir = if git_repo.join(".git").exists() {
        git_repo.join(".git")
    } else if git_repo.join("HEAD").exists() && git_repo.join("objects").exists() {
        git_repo.to_path_buf()
    } else {
        return Err(crate::command::init::InitError::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("'{}' is not a valid git repository", git_repo.display()),
        )));
    };

    let url = git_dir.to_str().ok_or_else(|| {
        crate::command::init::InitError::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "source git repository path '{}' contains invalid UTF-8",
                git_dir.display()
            ),
        ))
    })?;

    let remote = RemoteConfig {
        name: "origin".to_string(),
        url: url.to_string(),
    };

    fetch::fetch_repository(remote.clone(), None, false, None).await;

    clone::setup_repository(remote, None, !is_bare)
        .await
        .map_err(|e| crate::command::init::InitError::ConversionFailed(e.to_string()))?;

    Ok(())
}
