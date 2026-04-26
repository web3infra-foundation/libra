//! Utilities for converting existing Git repositories into Libra repositories by reusing fetch and clone logic.
//!
//! `libra init --from-git-repository <path>` calls into this module after the empty
//! Libra database has been bootstrapped. The conversion path treats the source Git
//! repository as a remote named `origin`, runs a normal `fetch` and `setup_repository`
//! against it, and then translates `.gitignore` files into `.libraignore` siblings so
//! ignore rules survive the migration.

use std::{
    env,
    path::{Path, PathBuf},
};

use crate::{
    command::{clone, fetch, init::InitError},
    internal::{branch::Branch, config::RemoteConfig},
    utils::{
        ignore,
        output::{OutputConfig, ProgressMode},
    },
};

/// Outcome of a successful Git -> Libra conversion.
///
/// Captures the canonical source location, the URL recorded as `origin`, and any
/// non-fatal warnings (typically from gitignore translation) that the caller may
/// surface to the user.
#[derive(Debug, Clone)]
pub struct ConversionReport {
    /// Absolute, canonical path to the source `.git` directory (or bare repo).
    pub source_git_dir: String,
    /// URL value written to `remote.origin.url`. Equal to `source_git_dir` for
    /// local-path conversions.
    pub remote_url: String,
    /// Non-fatal messages collected during conversion (e.g. unreadable
    /// `.gitignore` files that were skipped).
    pub warnings: Vec<String>,
}

/// Convert an existing local Git repository into the current Libra repository.
///
/// This function assumes that `libra init` has already created the Libra
/// storage layout and database in the target directory. It will:
/// - Normalize the provided Git repository path.
/// - Fetch all objects and references from the Git repository.
/// - Configure the `origin` remote, local branches, and HEAD using the same
///   logic as the `clone` command.
///
/// Boundary conditions:
/// - Returns `InitError::InvalidUtf8Path` when the canonicalised source path is not
///   valid UTF-8 (Git remote URLs are stored as strings, so non-UTF-8 paths cannot
///   be recorded as the origin URL).
/// - Returns `InitError::ConversionFailed { stage: "fetch" | "setup" }` when the
///   underlying fetch or remote setup fails. The `stage` field lets the user see
///   which phase broke.
/// - Returns `InitError::ConversionFailed { stage: "setup" }` when the source
///   repository has no refs at all — converting an empty Git repo would otherwise
///   produce an unusable Libra repo with no branches.
/// - Bare conversions skip the `.gitignore` -> `.libraignore` translation because
///   bare repositories have no working tree.
/// - Output is forced into quiet/no-progress mode so the host `libra init` command
///   stays in control of stdout formatting.
pub async fn convert_from_git_repository(
    git_repo: &Path,
    is_bare: bool,
) -> Result<ConversionReport, InitError> {
    let git_dir = resolve_git_source_dir(git_repo)?;
    let source_worktree = source_worktree_root(git_repo, &git_dir);

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

    let mut warnings = Vec::new();
    if !is_bare {
        let target_root = env::current_dir()?;
        let source_root = source_worktree.as_deref().unwrap_or(target_root.as_path());
        warnings.extend(ignore::convert_gitignore_files_to_libraignore(
            source_root,
            &target_root,
        )?);
    }

    Ok(ConversionReport {
        source_git_dir: git_dir.to_string_lossy().to_string(),
        remote_url: url.to_string(),
        warnings,
    })
}

/// Locate the Git directory inside `git_repo`, supporting both bare and
/// working-tree layouts.
///
/// Functional scope:
/// - When `<git_repo>/.git` exists, returns its canonicalised path. Otherwise
///   treats `git_repo` itself as the Git directory (bare-repo layout).
///
/// Boundary conditions:
/// - Returns `InitError::InvalidGitRepository` if any of the marker files
///   (`HEAD`, `config`, `objects`) are missing — these are the minimal set
///   required for `fetch` against a local file:// URL to succeed.
/// - Returns `InitError::Io` when `canonicalize` fails (path no longer exists,
///   permission denied, etc.).
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

/// Resolve the working-tree root of the source Git repository if one exists.
///
/// Functional scope:
/// - When the source has a `.git` subdirectory and that subdirectory canonicalises
///   to `git_dir`, returns the working-tree path so `.gitignore` translation can
///   walk the tree.
///
/// Boundary conditions:
/// - Returns `None` for bare repositories (no `.git` subdirectory) — there is no
///   working tree to walk.
/// - Returns `None` when `.git` is a regular file (worktree linkfile) or when
///   canonicalisation fails for any reason; the caller falls back to using the
///   target directory as the source root.
fn source_worktree_root(git_repo: &Path, git_dir: &Path) -> Option<PathBuf> {
    let dot_git = git_repo.join(".git");
    if !dot_git.exists() {
        return None;
    }
    let canonical_dot_git = dot_git.canonicalize().ok()?;
    (canonical_dot_git == git_dir).then(|| git_repo.to_path_buf())
}
