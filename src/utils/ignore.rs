//! Ignore handling utilities defining policies for .libraignore, index-aware filtering, and helpers to test whether paths are ignored.

use std::{
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
};

use git_internal::internal::index::Index;
use walkdir::WalkDir;

use super::util;

const LIBRAIGNORE_FILE: &str = ".libraignore";
const GITIGNORE_FILE: &str = ".gitignore";
const DEFAULT_LIBRAIGNORE_CONTENT: &[u8] = b"# Libra ignore file
# Uses gitignore-compatible patterns.
# Add generated files and local-only paths below.
";

/// File-system errors raised while creating or converting ignore files.
#[derive(thiserror::Error, Debug)]
pub enum IgnoreFileError {
    #[error("failed to read ignore file '{path}': {source}")]
    Read { path: PathBuf, source: io::Error },

    #[error("failed to create parent directory '{path}' for ignore file '{target}': {source}")]
    CreateDirectory {
        path: PathBuf,
        target: PathBuf,
        source: io::Error,
    },

    #[error("failed to write ignore file '{path}': {source}")]
    Write { path: PathBuf, source: io::Error },

    #[error("failed to scan ignore files under '{path}': {source}")]
    Walk { path: PathBuf, source: io::Error },

    #[error("failed to resolve ignore file path '{path}' relative to '{root}': {message}")]
    RelativePath {
        root: PathBuf,
        path: PathBuf,
        message: String,
    },
}

impl IgnoreFileError {
    pub fn is_write(&self) -> bool {
        matches!(self, Self::CreateDirectory { .. } | Self::Write { .. })
    }

    pub fn recovery_hint(&self) -> &'static str {
        match self {
            Self::Read { .. } | Self::Write { .. } => {
                "check .gitignore/.libraignore permissions and retry."
            }
            Self::CreateDirectory { .. } => {
                "check parent directory permissions for .libraignore and retry."
            }
            Self::Walk { .. } => "check source repository permissions and retry.",
            Self::RelativePath { .. } => {
                "ensure the source ignore file is inside the repository being converted."
            }
        }
    }
}

/// Describes how commands should treat entries matched by `.libraignore`.
/// - `Respect`: honor ignore rules for untracked files but always keep tracked ones.
/// - `IncludeIgnored`: disable ignore filtering entirely, used by `add --force` and similar flows.
/// - `OnlyIgnored`: surface only the ignored set, used by `status --ignored` flows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IgnorePolicy {
    Respect,
    IncludeIgnored,
    OnlyIgnored,
}

/// Creates the root `.libraignore` for a non-bare worktree if the user has not already provided one.
pub fn ensure_root_libraignore(worktree: &Path) -> Result<(), IgnoreFileError> {
    let target = worktree.join(LIBRAIGNORE_FILE);
    if target.exists() {
        return Ok(());
    }
    fs::write(&target, DEFAULT_LIBRAIGNORE_CONTENT).map_err(|source| IgnoreFileError::Write {
        path: target,
        source,
    })
}

/// Copies every `.gitignore` under `source_root` to a sibling `.libraignore` under `target_root`.
///
/// Existing generated default `.libraignore` files are replaced; existing user-owned
/// `.libraignore` files are preserved and reported as non-fatal warnings.
pub fn convert_gitignore_files_to_libraignore(
    source_root: &Path,
    target_root: &Path,
) -> Result<Vec<String>, IgnoreFileError> {
    let mut warnings = Vec::new();
    for entry in WalkDir::new(source_root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| should_visit_ignore_entry(entry.path(), source_root))
    {
        let entry = entry.map_err(|error| walkdir_error(source_root, error))?;
        if !entry.file_type().is_file() || entry.file_name() != OsStr::new(GITIGNORE_FILE) {
            continue;
        }

        let relative = entry.path().strip_prefix(source_root).map_err(|error| {
            IgnoreFileError::RelativePath {
                root: source_root.to_path_buf(),
                path: entry.path().to_path_buf(),
                message: error.to_string(),
            }
        })?;
        let target = target_root.join(relative).with_file_name(LIBRAIGNORE_FILE);
        copy_gitignore_to_libraignore(entry.path(), &target, &mut warnings)?;
    }
    Ok(warnings)
}

fn copy_gitignore_to_libraignore(
    source: &Path,
    target: &Path,
    warnings: &mut Vec<String>,
) -> Result<(), IgnoreFileError> {
    let source_content = fs::read(source).map_err(|read_error| IgnoreFileError::Read {
        path: source.to_path_buf(),
        source: read_error,
    })?;

    if !target.exists() {
        write_libraignore(target, &source_content)?;
        return Ok(());
    }

    let existing = fs::read(target).map_err(|read_error| IgnoreFileError::Read {
        path: target.to_path_buf(),
        source: read_error,
    })?;
    if existing == DEFAULT_LIBRAIGNORE_CONTENT || existing == source_content {
        if existing != source_content {
            write_libraignore(target, &source_content)?;
        }
        return Ok(());
    }

    warnings.push(format!(
        "kept existing .libraignore at '{}'; skipped converting '{}'",
        target.display(),
        source.display()
    ));
    Ok(())
}

fn write_libraignore(target: &Path, content: &[u8]) -> Result<(), IgnoreFileError> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|source| IgnoreFileError::CreateDirectory {
            path: parent.to_path_buf(),
            target: target.to_path_buf(),
            source,
        })?;
    }
    fs::write(target, content).map_err(|source| IgnoreFileError::Write {
        path: target.to_path_buf(),
        source,
    })
}

fn should_visit_ignore_entry(path: &Path, root: &Path) -> bool {
    if path == root {
        return true;
    }
    let Some(name) = path.file_name() else {
        return true;
    };
    name != OsStr::new(".git") && name != OsStr::new(util::ROOT_DIR)
}

fn walkdir_error(root: &Path, error: walkdir::Error) -> IgnoreFileError {
    let message = error.to_string();
    let source = error
        .into_io_error()
        .unwrap_or_else(|| io::Error::other(message));
    IgnoreFileError::Walk {
        path: root.to_path_buf(),
        source,
    }
}

/// Returns `true` if the given workdir-relative `path` should be filtered out under `policy`.
/// The check is index-aware; tracked entries remain visible for `Respect`, are always included for
/// `IncludeIgnored`, and get filtered when `OnlyIgnored` is requested.
pub fn should_ignore(path: &Path, policy: IgnorePolicy, index: &Index) -> bool {
    let workdir = util::working_dir();
    should_ignore_with_workdir(path, policy, index, &workdir)
}

/// Applies [`should_ignore`] over an iterator of workdir paths and returns the retained list.
pub fn filter_workdir_paths<I>(paths: I, policy: IgnorePolicy, index: &Index) -> Vec<PathBuf>
where
    I: IntoIterator<Item = PathBuf>,
{
    let workdir = util::working_dir();
    paths
        .into_iter()
        .filter(|path| !should_ignore_with_workdir(path, policy, index, &workdir))
        .collect()
}

/// Worker that shares the ignore logic between direct calls and batched iterators.
fn should_ignore_with_workdir(
    path: &Path,
    policy: IgnorePolicy,
    index: &Index,
    workdir: &PathBuf,
) -> bool {
    let is_tracked = path_is_tracked_or_unknown_encoding(path, index);

    match policy {
        IgnorePolicy::Respect => {
            if is_tracked {
                return false;
            }
            is_path_ignored(path, workdir)
        }
        IgnorePolicy::IncludeIgnored => false,
        IgnorePolicy::OnlyIgnored => {
            if is_tracked {
                return true;
            }
            !is_path_ignored(path, workdir)
        }
    }
}

fn path_is_tracked_or_unknown_encoding(path: &Path, index: &Index) -> bool {
    match path.to_str() {
        Some(path_str) => index.tracked(path_str, 0),
        // The current index API is UTF-8 keyed. Preserve visibility for paths we cannot
        // look up instead of silently treating a possibly tracked path as untracked.
        None => true,
    }
}

fn is_path_ignored(path: &Path, workdir: &PathBuf) -> bool {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        workdir.join(path)
    };
    util::check_gitignore(workdir, &absolute)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use git_internal::internal::index::Index;
    use serial_test::serial;
    use tempfile::tempdir;

    use super::*;
    use crate::{
        command::{
            add::{self, AddArgs},
            status::{changes_to_be_committed_safe, changes_to_be_staged},
        },
        utils::test,
    };

    fn build_index() -> Index {
        Index::load(crate::utils::path::index()).unwrap()
    }

    #[test]
    fn write_libraignore_reports_parent_directory_creation_errors() {
        let repo = tempdir().unwrap();
        let parent = repo.path().join("not-a-directory");
        fs::write(&parent, "file").unwrap();
        let target = parent.join(".libraignore");

        let error = write_libraignore(&target, b"ignored\n").unwrap_err();

        match error {
            IgnoreFileError::CreateDirectory {
                path,
                target: error_target,
                ..
            } => {
                assert_eq!(path, parent);
                assert_eq!(error_target, target);
            }
            other => panic!("expected CreateDirectory error, got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_paths_use_conservative_tracked_fallback() {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};

        let repo = tempdir().unwrap();
        let workdir = repo.path().to_path_buf();
        fs::write(workdir.join(".libraignore"), "*\n").unwrap();
        let non_utf8_path = PathBuf::from(OsString::from_vec(vec![b'i', 0xff, b'n']));
        let index = Index::new();

        assert!(
            !should_ignore_with_workdir(&non_utf8_path, IgnorePolicy::Respect, &index, &workdir),
            "unknown-encoding paths should stay visible under Respect"
        );
        assert!(
            should_ignore_with_workdir(&non_utf8_path, IgnorePolicy::OnlyIgnored, &index, &workdir),
            "unknown-encoding paths should be excluded from OnlyIgnored like tracked entries"
        );
    }

    #[tokio::test]
    #[serial]
    async fn respect_policy_ignores_untracked_files() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());

        fs::write(".libraignore", "ignored.txt\n").unwrap();
        fs::write("ignored.txt", "ignored").unwrap();
        fs::write("tracked.txt", "tracked").unwrap();

        add::execute(AddArgs {
            pathspec: vec!["tracked.txt".into()],
            all: false,
            update: false,
            refresh: false,
            force: false,
            verbose: false,
            dry_run: false,
            ignore_errors: false,
        })
        .await;

        let index = build_index();
        assert!(should_ignore(
            Path::new("ignored.txt"),
            IgnorePolicy::Respect,
            &index
        ));
        assert!(!should_ignore(
            Path::new("tracked.txt"),
            IgnorePolicy::Respect,
            &index
        ));
    }

    #[tokio::test]
    #[serial]
    async fn include_ignored_policy_keeps_untracked_files() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());

        fs::write(".libraignore", "ignored.txt\n").unwrap();
        fs::write("ignored.txt", "ignored").unwrap();
        fs::write("visible.txt", "visible").unwrap();

        let index = build_index();
        assert!(!should_ignore(
            Path::new("ignored.txt"),
            IgnorePolicy::IncludeIgnored,
            &index
        ));

        let filtered = filter_workdir_paths(
            vec![PathBuf::from("ignored.txt"), PathBuf::from("visible.txt")],
            IgnorePolicy::IncludeIgnored,
            &index,
        );
        assert_eq!(
            filtered,
            vec![PathBuf::from("ignored.txt"), PathBuf::from("visible.txt")]
        );

        let unstaged =
            crate::command::status::changes_to_be_staged_with_policy(IgnorePolicy::IncludeIgnored)
                .unwrap();
        assert!(
            unstaged.new.iter().any(|p| p == Path::new("ignored.txt")),
            "IncludeIgnored policy should surface ignored entries for staging workflows"
        );
    }

    #[tokio::test]
    #[serial]
    async fn only_ignored_policy_returns_only_ignored_paths() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());

        fs::write(".libraignore", "ignored.txt\n").unwrap();
        fs::write("ignored.txt", "ignored").unwrap();
        fs::write("tracked.txt", "tracked").unwrap();

        add::execute(AddArgs {
            pathspec: vec!["tracked.txt".into()],
            all: false,
            update: false,
            refresh: false,
            force: false,
            verbose: false,
            dry_run: false,
            ignore_errors: false,
        })
        .await;

        let index = build_index();
        let workdir_files = vec![PathBuf::from("ignored.txt"), PathBuf::from("tracked.txt")];
        let filtered =
            filter_workdir_paths(workdir_files.into_iter(), IgnorePolicy::OnlyIgnored, &index);
        assert_eq!(filtered, vec![PathBuf::from("ignored.txt")]);

        let staged = changes_to_be_committed_safe().await.unwrap();
        assert!(staged.new.iter().any(|p| p == Path::new("tracked.txt")));

        let unstaged = changes_to_be_staged().unwrap();
        assert!(!unstaged.new.iter().any(|p| p == Path::new("ignored.txt")));
    }

    #[tokio::test]
    #[serial]
    async fn only_ignored_policy_excludes_tracked_entries() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());

        fs::write(".libraignore", "ignored.txt\n").unwrap();
        fs::write("ignored.txt", "initial").unwrap();

        add::execute(AddArgs {
            pathspec: vec!["ignored.txt".into()],
            all: false,
            update: false,
            refresh: false,
            force: true,
            verbose: false,
            dry_run: false,
            ignore_errors: false,
        })
        .await;

        let index = build_index();
        assert!(
            index.tracked("ignored.txt", 0),
            "sanity check: ignored file should now be tracked"
        );

        let filtered = filter_workdir_paths(
            vec![PathBuf::from("ignored.txt")],
            IgnorePolicy::OnlyIgnored,
            &index,
        );
        assert!(
            filtered.is_empty(),
            "tracked entries must be removed when requesting only ignored files"
        );

        let only_ignored =
            crate::command::status::changes_to_be_staged_with_policy(IgnorePolicy::OnlyIgnored)
                .unwrap();
        assert!(
            !only_ignored
                .new
                .iter()
                .any(|p| p == Path::new("ignored.txt")),
            "OnlyIgnored policy should hide tracked files even if they match ignore patterns"
        );
    }
}
