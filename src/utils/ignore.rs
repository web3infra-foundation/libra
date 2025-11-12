use git_internal::internal::index::Index;
use std::path::{Path, PathBuf};

use super::util;

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
    let path_str = path
        .to_str()
        .unwrap_or_else(|| panic!("path {:?} is not valid UTF-8", path));
    let is_tracked = index.tracked(path_str, 0);

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
    use super::*;
    use crate::command::add::{self, AddArgs};
    use crate::command::status::{changes_to_be_committed, changes_to_be_staged};
    use crate::utils::test;
    use git_internal::internal::index::Index;
    use serial_test::serial;
    use std::fs;
    use tempfile::tempdir;

    fn build_index() -> Index {
        Index::load(crate::utils::path::index()).unwrap()
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
            crate::command::status::changes_to_be_staged_with_policy(IgnorePolicy::IncludeIgnored);
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

        let staged = changes_to_be_committed().await;
        assert!(staged.new.iter().any(|p| p == Path::new("tracked.txt")));

        let unstaged = changes_to_be_staged();
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
            crate::command::status::changes_to_be_staged_with_policy(IgnorePolicy::OnlyIgnored);
        assert!(
            !only_ignored
                .new
                .iter()
                .any(|p| p == Path::new("ignored.txt")),
            "OnlyIgnored policy should hide tracked files even if they match ignore patterns"
        );
    }
}
