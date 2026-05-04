//! Core utility toolbox for repo detection, path conversion, ignore checking, storage access, hashing helpers, and miscellaneous formatting/time utilities.

use std::{
    collections::{HashMap, HashSet},
    env,
    ffi::OsStr,
    fs, io,
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};

use git_internal::{
    hash::ObjectHash,
    internal::object::{commit::Commit, types::ObjectType},
};
use ignore::{Match, gitignore::Gitignore};
use indicatif::{ProgressBar, ProgressStyle};
use once_cell::sync::Lazy;
use path_absolutize::*;

use crate::{
    command::load_object,
    internal::{
        branch::{Branch, BranchStoreError},
        config::ConfigKv,
        head::Head,
        tag,
    },
    utils::{client_storage::ClientStorage, error::emit_legacy_stderr, path, path_ext::PathExt},
};

// SAFETY: The unwrap() and expect() calls in this module are documented with safety
// justifications where used. These are intentional panics for unrecoverable errors
// or cases where invariants are guaranteed by the code structure.

pub const ROOT_DIR: &str = ".libra";
pub const DATABASE: &str = "libra.db";
pub const ATTRIBUTES: &str = ".libra_attributes";

static OBJECTS_STORAGE_CACHE: Lazy<Mutex<HashMap<PathBuf, ClientStorage>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitRepositoryLocation {
    pub root: PathBuf,
    pub is_bare: bool,
}

/// Returns the current working directory as a `PathBuf`.
///
/// This function wraps the `std::env::current_dir()` function and provides
/// robust fallback behavior when the current directory is not available.
///
/// # Returns
///
/// A `PathBuf` representing the current working directory. If the current
/// directory cannot be determined, this function uses the following fallbacks:
/// 1. The `PWD` environment variable (if set and points to a valid directory)
/// 2. The parent directory of the current executable (if available)
/// 3. The root directory `/` as a last resort
pub fn cur_dir() -> PathBuf {
    match env::current_dir() {
        Ok(dir) => dir,
        Err(_) => {
            // Fallback 1: use PWD if present and valid
            if let Ok(pwd) = env::var("PWD") {
                let p = PathBuf::from(&pwd);
                if p.exists() && p.is_dir() {
                    return p;
                }
            }

            // Fallback 2: directory of the current executable if available
            if let Ok(exec) = env::current_exe()
                && let Some(parent) = exec.parent()
                && parent.exists()
                && parent.is_dir()
            {
                return parent.to_path_buf();
            }

            // Fallback 3: root directory to ensure a stable, existing path
            PathBuf::from("/")
        }
    }
}

fn is_valid_storage_dir(path: &Path) -> bool {
    if path.join(DATABASE).exists() {
        return true;
    }

    ["objects", "info/exclude", "hooks"]
        .iter()
        .filter(|marker| path.join(marker).exists())
        .count()
        >= 2
}

fn read_gitdir_file(path: &Path, worktree: &Path) -> Option<PathBuf> {
    let contents = fs::read_to_string(path).ok()?;
    let line = contents.lines().next()?.trim();
    let raw = line.strip_prefix("gitdir:")?.trim();
    if raw.is_empty() {
        return None;
    }

    let git_dir = Path::new(raw);
    Some(if git_dir.is_absolute() {
        git_dir.to_path_buf()
    } else {
        worktree.join(git_dir)
    })
}

fn resolve_dot_git_dir(worktree: &Path) -> Option<PathBuf> {
    let dot_git = worktree.join(".git");
    let metadata = fs::metadata(&dot_git).ok()?;
    if metadata.is_dir() {
        return Some(dot_git);
    }
    if metadata.is_file() {
        return read_gitdir_file(&dot_git, worktree);
    }
    None
}

fn git_common_dir(git_dir: &Path) -> Option<PathBuf> {
    let contents = fs::read_to_string(git_dir.join("commondir")).ok()?;
    let raw = contents.lines().next()?.trim();
    if raw.is_empty() {
        return None;
    }

    let common_dir = Path::new(raw);
    Some(if common_dir.is_absolute() {
        common_dir.to_path_buf()
    } else {
        git_dir.join(common_dir)
    })
}

fn git_dir_marker_exists(git_dir: &Path, common_dir: Option<&Path>, marker: &str) -> bool {
    git_dir.join(marker).exists() || common_dir.is_some_and(|dir| dir.join(marker).exists())
}

fn is_valid_git_storage_dir(git_dir: &Path) -> bool {
    let common_dir = git_common_dir(git_dir);
    git_dir.join("HEAD").exists()
        && git_dir_marker_exists(git_dir, common_dir.as_deref(), "config")
        && git_dir_marker_exists(git_dir, common_dir.as_deref(), "objects")
}

fn git_config_declares_bare(git_dir: &Path) -> bool {
    fs::read_to_string(git_dir.join("config")).is_ok_and(|config| {
        config.lines().any(|line| {
            line.trim().split_once('=').is_some_and(|(key, value)| {
                key.trim() == "bare" && value.trim().eq_ignore_ascii_case("true")
            })
        })
    })
}

fn worktree_root_for_dot_git_storage(candidate: &Path) -> Option<PathBuf> {
    if candidate.file_name() != Some(OsStr::new(".git"))
        || !is_valid_git_storage_dir(candidate)
        || git_config_declares_bare(candidate)
    {
        return None;
    }
    let parent = candidate.parent()?;
    Some(fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf()))
}

pub fn find_git_repository(path: Option<&Path>) -> Option<GitRepositoryLocation> {
    let mut candidate = path.map(Path::to_path_buf).unwrap_or_else(cur_dir);
    if candidate.is_file() {
        candidate.pop();
    }

    loop {
        if let Some(git_dir) = resolve_dot_git_dir(&candidate)
            && is_valid_git_storage_dir(&git_dir)
        {
            return Some(GitRepositoryLocation {
                root: fs::canonicalize(&candidate).unwrap_or_else(|_| candidate.clone()),
                is_bare: false,
            });
        }

        if let Some(root) = worktree_root_for_dot_git_storage(&candidate) {
            return Some(GitRepositoryLocation {
                root,
                is_bare: false,
            });
        }

        if is_valid_git_storage_dir(&candidate) {
            return Some(GitRepositoryLocation {
                root: fs::canonicalize(&candidate).unwrap_or_else(|_| candidate.clone()),
                is_bare: true,
            });
        }

        if !candidate.pop() {
            return None;
        }
    }
}

fn try_get_paths(path: Option<PathBuf>) -> Result<(PathBuf, PathBuf), io::Error> {
    let mut path = path.clone().unwrap_or_else(cur_dir);
    let orig = path.clone();

    loop {
        let standard_repo = path.join(ROOT_DIR);
        if standard_repo.is_dir() && is_valid_storage_dir(&standard_repo) {
            // unwrap_or is safe here: if canonicalize fails, we use the original path
            let storage = fs::canonicalize(&standard_repo).unwrap_or(standard_repo);
            return Ok((storage, path.clone()));
        }

        if path.join(DATABASE).exists() && path.join("objects").exists() {
            return Ok((path.clone(), path.clone()));
        }

        if !path.pop() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{orig:?} is not a libra repository"),
            ));
        }
    }
}

/// Try to get the storage path of the repository, which is the path of the `.libra` directory
/// - if the current directory or given path is not a repository, return an error
pub fn try_get_storage_path(path: Option<PathBuf>) -> Result<PathBuf, io::Error> {
    let (storage, _) = try_get_paths(path)?;
    Ok(storage)
}

/// Load the storage path with optional given repository
pub fn storage_path() -> PathBuf {
    // unwrap is safe here: this function is only called after verifying we're in a repo
    try_get_storage_path(None).unwrap()
}

/// Return an error instead of printing when the current directory is not a repository.
pub fn require_repo() -> io::Result<()> {
    try_get_storage_path(None).map(|_| ())
}

/// Legacy repository check that still prints for commands not yet migrated.
pub fn check_repo_exist() -> bool {
    if require_repo().is_err() {
        emit_legacy_stderr(
            "fatal: not a libra repository (or any of the parent directories): .libra",
        );
        return false;
    }
    true
}

/// Get `ClientStorage` for the `objects` directory
pub fn objects_storage() -> ClientStorage {
    cached_objects_storage(path::objects())
}

/// Get `ClientStorage` for the `objects` directory, returning a Result
pub fn try_objects_storage() -> io::Result<ClientStorage> {
    // Check if we are in a valid repo first to avoid panic in path::objects() if possible,
    // though path::objects() currently panics if storage_path() fails.
    // Ideally path::objects() should also be fallible.
    // For now, let's wrap the panic-prone call if we can, or just rely on try_get_storage_path check.
    if try_get_storage_path(None).is_err() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "not a libra repository",
        ));
    }
    Ok(cached_objects_storage(path::objects()))
}

fn cached_objects_storage(base_path: PathBuf) -> ClientStorage {
    let mut cache = OBJECTS_STORAGE_CACHE
        .lock()
        .expect("objects storage cache mutex poisoned"); // panic is intentional: if poisoned, we cannot recover
    if let Some(storage) = cache.get(&base_path) {
        return storage.clone();
    }

    let storage = ClientStorage::init(base_path.clone());
    cache.insert(base_path, storage.clone());
    storage
}

pub fn reset_objects_storage_cache_for_path(base_path: &Path) {
    if let Ok(mut cache) = OBJECTS_STORAGE_CACHE.lock() {
        cache.remove(base_path);
    }
}

/// Get the working directory of the repository
/// - panics if the current directory is not a repository
pub fn working_dir() -> PathBuf {
    // unwrap is safe here: documented to panic if not in a repository
    let (_, workdir) = try_get_paths(None).unwrap();
    workdir
}

/// Get the working directory of the repository.
pub fn try_working_dir() -> io::Result<PathBuf> {
    let (_, workdir) = try_get_paths(None)?;
    Ok(workdir)
}

/// Get the working directory of the repository as a string, panics if the path is not valid utf-8
pub fn working_dir_string() -> String {
    // unwrap is safe here: documented to panic on non-UTF-8 paths
    working_dir().to_str().unwrap().to_string()
}

/// Get the working directory of the repository as UTF-8.
pub fn try_working_dir_string() -> io::Result<String> {
    let workdir = try_working_dir()?;
    workdir.to_str().map(str::to_string).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("path '{}' is not valid UTF-8", workdir.display()),
        )
    })
}

/// Turn a path to a relative path to the working directory
/// - not check existence
pub fn to_workdir_path(path: impl AsRef<Path>) -> PathBuf {
    to_relative(path, working_dir())
}

/// Turn a workdir path to absolute path
pub fn workdir_to_absolute(path: impl AsRef<Path>) -> PathBuf {
    working_dir().join(path.as_ref())
}

/// Turn a workdir path to absolute path.
pub fn try_workdir_to_absolute(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    Ok(try_working_dir()?.join(path.as_ref()))
}

/// Judge if the path is a sub path of the parent path
/// - Not check existence
/// - `true` if path == parent
pub fn is_sub_path<P, B>(path: P, parent: B) -> bool
where
    P: AsRef<Path>,
    B: AsRef<Path>,
{
    fn normalize_abs_path(path: &Path) -> PathBuf {
        use std::path::Component;

        let mut out = PathBuf::new();
        for comp in path.components() {
            match comp {
                Component::Prefix(prefix) => out.push(prefix.as_os_str()),
                Component::RootDir => out.push(Path::new(comp.as_os_str())),
                Component::CurDir => {}
                Component::ParentDir => {
                    // Never allow `..` to escape above filesystem root/prefix.
                    if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                        out.pop();
                    }
                }
                Component::Normal(part) => out.push(part),
            }
        }
        out
    }

    // Avoid panics and avoid depending on a valid current directory when inputs are absolute.
    let path_abs = if path.as_ref().is_absolute() {
        normalize_abs_path(path.as_ref())
    } else {
        match path.as_ref().absolutize() {
            Ok(p) => p.to_path_buf(),
            Err(_) => return false,
        }
    };

    let parent_abs = if parent.as_ref().is_absolute() {
        normalize_abs_path(parent.as_ref())
    } else {
        match parent.as_ref().absolutize() {
            Ok(p) => p.to_path_buf(),
            Err(_) => return false,
        }
    };

    path_abs.starts_with(parent_abs)
}

/// Judge if the `path` is sub-path of `paths`(include sub-dirs)
/// - absolute path or relative path to the current dir
/// - Not check existence
pub fn is_sub_of_paths<P, U>(path: impl AsRef<Path>, paths: U) -> bool
where
    P: AsRef<Path>,
    U: IntoIterator<Item = P>,
{
    for p in paths {
        if is_sub_path(path.as_ref(), p.as_ref()) {
            return true;
        }
    }
    false
}

/// Filter paths to fit the given paths, include sub-dirs
/// - return the paths that are sub-path of the fit paths
/// - `paths`: to workdir
/// - `fit_paths`: abs or rel
/// - Not check existence
pub fn filter_to_fit_paths<P>(paths: &[P], fit_paths: &Vec<P>) -> Vec<P>
where
    P: AsRef<Path> + Clone,
{
    paths
        .iter()
        .filter(|p| {
            let p = workdir_to_absolute(p.as_ref());
            is_sub_of_paths(p, fit_paths)
        })
        .cloned()
        .collect()
}

/// `path` & `base` must be absolute or relative (to current dir)
/// <br> return "." if `path` == `base`
pub fn to_relative<P, B>(path: P, base: B) -> PathBuf
where
    P: AsRef<Path>,
    B: AsRef<Path>,
{
    // Snapshot the current directory once so both inputs resolve against the same base
    // even if another test or thread changes the process cwd concurrently.
    let cwd = cur_dir();
    let path_abs = match path.as_ref().absolutize_from(&cwd) {
        Ok(p) => p.into_owned(),
        Err(_) => cwd.join(path.as_ref()),
    };
    let base_abs = match base.as_ref().absolutize_from(&cwd) {
        Ok(b) => b.into_owned(),
        Err(_) => cwd.join(base.as_ref()),
    };

    if let Some(rel_path) = pathdiff::diff_paths(path_abs, base_abs) {
        if rel_path.to_string_lossy() == "" {
            PathBuf::from(".")
        } else {
            rel_path
        }
    } else {
        // panic is intentional: this indicates a bug in path resolution logic
        panic!(
            "fatal: path {:?} cannot convert to relative based on {:?}",
            path.as_ref(),
            base.as_ref()
        );
    }
}

#[allow(dead_code)]
/// Convert a path to relative path to the current directory
/// - `path` must be absolute or relative (to current dir)
pub fn to_current_dir<P>(path: P) -> PathBuf
where
    P: AsRef<Path>,
{
    to_relative(path, cur_dir())
}

/// Convert a workdir path to relative path
/// - `base` must be absolute or relative (to current dir)
pub fn workdir_to_relative<P, B>(path: P, base: B) -> PathBuf
where
    P: AsRef<Path>,
    B: AsRef<Path>,
{
    let path_abs = workdir_to_absolute(path);
    to_relative(path_abs, base)
}

/// Convert a workdir path to relative path to the current directory
pub fn workdir_to_current<P>(path: P) -> PathBuf
where
    P: AsRef<Path>,
{
    workdir_to_relative(path, cur_dir())
}

/// List all files in the given dir and its sub_dir, except `.libra`
/// - input `path`: absolute path or relative path to the current dir
/// - output: to workdir path
pub fn list_files(path: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    if path.is_dir() {
        // unwrap_or_default is safe: returns empty string which won't match ROOT_DIR
        if path.file_name().unwrap_or_default() == ROOT_DIR {
            // ignore `.libra`
            return Ok(files);
        }
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                files.extend(list_files(&path)?);
            } else {
                files.push(to_workdir_path(&path));
            }
        }
    }
    Ok(files)
}

/// list all files in the working dir(include sub_dir)
/// - output: to workdir path
pub fn list_workdir_files() -> io::Result<Vec<PathBuf>> {
    list_files(&working_dir())
}

/// Integrate the input paths (relative, absolute, file, dir) to workdir paths.
/// Only existing files are expanded; directory traversal failures are surfaced
/// to the caller so command code can report them instead of panicking.
pub fn integrate_pathspec(paths: &[PathBuf]) -> io::Result<HashSet<PathBuf>> {
    let mut workdir_paths = HashSet::new();
    for path in paths {
        if path.is_dir() {
            let files = list_files(path)?;
            workdir_paths.extend(files);
        } else {
            workdir_paths.insert(path.to_workdir());
        }
    }
    Ok(workdir_paths)
}

/// write content to file
/// - create parent directory if not exist
pub fn write_file(content: &[u8], file: &PathBuf) -> io::Result<()> {
    let mut parent = file.clone();
    parent.pop();
    fs::create_dir_all(parent)?;
    let mut file = fs::File::create(file)?;
    file.write_all(content)
}

/// Removing the empty directories in cascade until meet the root of workdir or the current dir
pub fn clear_empty_dir(dir: &Path) {
    let mut dir = if dir.is_dir() {
        dir.to_path_buf()
    } else {
        let Some(parent) = dir.parent() else {
            return;
        };
        parent.to_path_buf()
    };

    let repo = storage_path();
    // CAN NOT remove .libra & current dir
    while !is_sub_path(&repo, &dir) && !is_cur_dir(&dir) {
        if is_empty_dir(&dir) {
            if fs::remove_dir(&dir).is_err() {
                break;
            }
        } else {
            break; // once meet a non-empty dir, stop
        }
        dir.pop();
    }
}

pub fn is_empty_dir(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    match fs::read_dir(dir) {
        Ok(mut entries) => entries.next().is_none(),
        Err(_) => false,
    }
}

pub fn is_cur_dir(dir: &Path) -> bool {
    let Ok(dir) = dir.absolutize() else {
        return false;
    };
    let current_dir = cur_dir();
    let Ok(current) = current_dir.absolutize() else {
        return false;
    };
    dir == current
}

/// Transform a path to a string.
///
/// This function uses `to_string_lossy()` which converts the path to a string,
/// replacing any invalid UTF-8 sequences with the Unicode replacement character (U+FFFD).
/// This is preferred over `into_os_string().into_string().unwrap()` which would panic
/// on non-UTF-8 paths. The path separators are preserved as-is by the underlying OS
/// path representation; this function does not perform separator normalization.
pub fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

#[derive(Debug, thiserror::Error)]
pub enum CommitBaseError {
    #[error("HEAD does not point to a commit")]
    HeadUnborn,
    #[error("{0}")]
    InvalidReference(String),
    #[error("{0}")]
    ReadFailure(String),
    #[error("{0}")]
    CorruptReference(String),
}

impl CommitBaseError {
    fn from_branch_store_error(context: String, error: BranchStoreError) -> Self {
        let message = format!("{context}: {error}");
        match error {
            BranchStoreError::Query(_) | BranchStoreError::Delete { .. } => {
                Self::ReadFailure(message)
            }
            BranchStoreError::Corrupt { .. } => Self::CorruptReference(message),
            BranchStoreError::NotFound(_) => Self::InvalidReference(message),
        }
    }

    fn classify_storage_failure(message: String) -> Self {
        let lower = message.to_ascii_lowercase();
        if lower.contains("database is locked")
            || lower.contains("database schema is locked")
            || lower.contains("permission denied")
            || lower.contains("input/output error")
            || lower.contains("failed to read")
            || lower.contains("could not read")
            || lower.contains("failed to query")
        {
            Self::ReadFailure(message)
        } else {
            Self::CorruptReference(message)
        }
    }
}

async fn resolve_branch_commit_typed(
    branch_name: &str,
    remote: Option<&str>,
    display_name: &str,
) -> Result<Option<ObjectHash>, CommitBaseError> {
    let context = match remote {
        Some(remote_name) => {
            format!("failed to resolve branch '{display_name}' on remote '{remote_name}'")
        }
        None => format!("failed to resolve branch '{display_name}'"),
    };

    match Branch::find_branch_result(branch_name, remote).await {
        Ok(Some(branch)) => Ok(Some(branch.commit)),
        Ok(None) => match Branch::exists_result(branch_name, remote).await {
            Ok(true) => Err(CommitBaseError::InvalidReference(format!(
                "branch '{display_name}' does not point to a commit"
            ))),
            Ok(false) => Ok(None),
            Err(error) => Err(CommitBaseError::from_branch_store_error(context, error)),
        },
        Err(error) => Err(CommitBaseError::from_branch_store_error(context, error)),
    }
}

fn split_revision_navigation(name: &str) -> Option<(&str, &str)> {
    name.char_indices()
        .find(|(_, ch)| *ch == '~' || *ch == '^')
        .map(|(index, _)| name.split_at(index))
}

pub(crate) fn remote_tracking_candidates(name: &str) -> impl Iterator<Item = (&str, &str)> + '_ {
    name.char_indices().filter_map(|(index, ch)| {
        if ch != '/' {
            return None;
        }

        let remote = &name[..index];
        let branch_name = &name[index + 1..];
        (!remote.is_empty() && !branch_name.is_empty()).then_some((remote, branch_name))
    })
}

fn nth_parent_commit_typed(
    commit_id: &ObjectHash,
    n: usize,
    display_name: &str,
) -> Result<ObjectHash, CommitBaseError> {
    let commit: Commit = load_object(commit_id).map_err(|error| {
        CommitBaseError::classify_storage_failure(format!(
            "failed to load commit object while resolving '{display_name}': {error}"
        ))
    })?;

    if n == 0 || n > commit.parent_commit_ids.len() {
        return Err(CommitBaseError::InvalidReference(format!(
            "invalid reference: {display_name}"
        )));
    }

    Ok(commit.parent_commit_ids[n - 1])
}

fn navigate_commit_path_typed(
    mut current: ObjectHash,
    path: &str,
    display_name: &str,
) -> Result<ObjectHash, CommitBaseError> {
    let mut chars = path.chars().peekable();

    while let Some(symbol) = chars.next() {
        if symbol != '^' && symbol != '~' {
            return Err(CommitBaseError::InvalidReference(format!(
                "invalid reference: {display_name}"
            )));
        }

        let mut digits = String::new();
        while let Some(ch) = chars.peek() {
            if ch.is_ascii_digit() {
                digits.push(*ch);
                chars.next();
            } else {
                break;
            }
        }

        let step = if digits.is_empty() {
            1
        } else {
            digits.parse::<usize>().map_err(|_| {
                CommitBaseError::InvalidReference(format!("invalid reference: {display_name}"))
            })?
        };

        if step == 0 {
            // `~0` is identity. `^0` is also identity here because
            // `resolve_commit_base_atom_typed()` already peels named tags and
            // direct tag-object hashes to their referenced object before
            // navigation runs.
            continue;
        }

        match symbol {
            '^' => {
                current = nth_parent_commit_typed(&current, step, display_name)?;
            }
            '~' => {
                for _ in 0..step {
                    current = nth_parent_commit_typed(&current, 1, display_name)?;
                }
            }
            _ => unreachable!(),
        }
    }

    Ok(current)
}

async fn resolve_commit_base_atom_typed(name: &str) -> Result<ObjectHash, CommitBaseError> {
    // 1. Check for HEAD
    if name == "HEAD" {
        return match Head::current_commit_result().await {
            Ok(Some(commit_id)) => Ok(commit_id),
            Ok(None) => Err(CommitBaseError::HeadUnborn),
            Err(error) => Err(CommitBaseError::from_branch_store_error(
                "failed to resolve HEAD".to_string(),
                error,
            )),
        };
    }

    // 2. Check for a local branch
    if let Some(commit) = resolve_branch_commit_typed(name, None, name).await? {
        return Ok(commit);
    }

    // Support both short remote branches (`origin/main`) and fetched
    // remote-tracking refs (`refs/remotes/origin/main`), including multi-segment
    // remotes like `upstream/origin/main`.
    if let Some(short_name) = name.strip_prefix("refs/remotes/") {
        if let Some(commit) = resolve_branch_commit_typed(name, None, name).await? {
            return Ok(commit);
        }

        for (remote, branch_name) in remote_tracking_candidates(short_name) {
            if let Some(commit) = resolve_branch_commit_typed(name, Some(remote), name).await? {
                return Ok(commit);
            }

            if let Some(commit) =
                resolve_branch_commit_typed(branch_name, Some(remote), name).await?
            {
                return Ok(commit);
            }
        }
    } else {
        for (remote, branch_name) in remote_tracking_candidates(name) {
            if let Some(commit) = resolve_branch_commit_typed(
                &format!("refs/remotes/{remote}/{branch_name}"),
                Some(remote),
                name,
            )
            .await?
            {
                return Ok(commit);
            }

            if let Some(commit) =
                resolve_branch_commit_typed(branch_name, Some(remote), name).await?
            {
                return Ok(commit);
            }
        }
    }

    // 3. Check for a tag
    match tag::find_tag_and_commit(name).await {
        Ok(Some((_tag_object, commit))) => return Ok(commit.id),
        Ok(None) => {}
        Err(error) => {
            return Err(CommitBaseError::classify_storage_failure(format!(
                "failed to resolve tag '{name}': {error}"
            )));
        }
    }

    // 4. Check for a hash prefix
    let storage = objects_storage();
    let commits = storage.search_result(name).await.map_err(|error| {
        CommitBaseError::classify_storage_failure(format!(
            "failed to search objects while resolving '{name}': {error}"
        ))
    })?;
    if commits.is_empty() {
        return Err(CommitBaseError::InvalidReference(format!(
            "invalid reference: {name}"
        )));
    } else if commits.len() > 1 {
        return Err(CommitBaseError::InvalidReference(format!(
            "ambiguous argument: {name}"
        )));
    }

    let object_id = commits[0];
    let object_type = storage.get_object_type(&object_id).map_err(|e| {
        CommitBaseError::classify_storage_failure(format!(
            "could not read object type for {name}: {e}"
        ))
    })?;

    match object_type {
        ObjectType::Commit => Ok(object_id),
        ObjectType::Tag => peel_tag_hash_to_commit(&storage, object_id, name),
        _ => Err(CommitBaseError::InvalidReference(format!(
            "reference is not a commit: {name}, is {object_type}"
        ))),
    }
}

fn peel_tag_hash_to_commit(
    storage: &ClientStorage,
    object_id: ObjectHash,
    display_name: &str,
) -> Result<ObjectHash, CommitBaseError> {
    let mut current = object_id;
    let mut seen = HashSet::new();

    loop {
        if !seen.insert(current) {
            return Err(CommitBaseError::CorruptReference(format!(
                "tag cycle detected while resolving '{display_name}'"
            )));
        }

        let tag_obj: git_internal::internal::object::tag::Tag =
            load_object(&current).map_err(|error| {
                CommitBaseError::classify_storage_failure(format!(
                    "failed to load tag object while resolving '{display_name}': {error}"
                ))
            })?;
        let target_type = storage
            .get_object_type(&tag_obj.object_hash)
            .map_err(|error| {
                CommitBaseError::classify_storage_failure(format!(
                    "could not read tag target type while resolving '{display_name}': {error}"
                ))
            })?;

        match target_type {
            ObjectType::Commit => return Ok(tag_obj.object_hash),
            ObjectType::Tag => current = tag_obj.object_hash,
            _ => {
                return Err(CommitBaseError::InvalidReference(format!(
                    "reference is not a commit: {display_name}, tag points to {target_type}"
                )));
            }
        }
    }
}

pub async fn get_commit_base_typed(name: &str) -> Result<ObjectHash, CommitBaseError> {
    if let Some((base_ref, path)) = split_revision_navigation(name) {
        if base_ref.is_empty() {
            return Err(CommitBaseError::InvalidReference(format!(
                "invalid reference: {name}"
            )));
        }

        let base_commit = resolve_commit_base_atom_typed(base_ref).await?;
        return navigate_commit_path_typed(base_commit, path, name);
    }

    resolve_commit_base_atom_typed(name).await
}

/// Resolve a string to a commit [`ObjectHash`].
/// The string can be a local branch name, a remote-tracking branch name
/// (such as `origin/main`), a tag name, or a commit hash prefix.
/// Order of resolution:
/// 1. HEAD
/// 2. Local branch
/// 3. Remote-tracking branch (e.g. `origin/main`)
/// 4. Tag
/// 5. Commit hash prefix
pub async fn get_commit_base(name: &str) -> Result<ObjectHash, String> {
    get_commit_base_typed(name)
        .await
        .map_err(|error| format!("fatal: {error}"))
}

/// Get the repository name from the url
/// - e.g. `https://github.com/web3infra-foundation/mega.git/` -> mega
/// - e.g. `https://github.com/web3infra-foundation/mega.git` -> mega
pub fn get_repo_name_from_url(mut url: &str) -> Option<&str> {
    if url.ends_with('/') {
        url = &url[..url.len() - 1];
    }

    let repo_start = url.rfind('/')? + 1;
    let repo = &url[repo_start..];
    if repo.is_empty() {
        return None;
    }

    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    if repo.is_empty() { None } else { Some(repo) }
}

/// Find the appropriate unit and value for Bytes.
/// ### Examples
/// - 1024 bytes -> 1 KiB
/// - 1024 * 1024 bytes -> 1 MiB
pub fn auto_unit_bytes(bytes: u64) -> byte_unit::AdjustedByte {
    let bytes = byte_unit::Byte::from(bytes);
    bytes.get_appropriate_unit(byte_unit::UnitType::Binary)
}
/// Create a default style progress bar
pub fn default_progress_bar(len: u64) -> ProgressBar {
    let progress_bar = ProgressBar::new(len);
    progress_bar.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.magenta} [{elapsed_precise}] [{bar:40.green/white}] {bytes}/{total_bytes} ({eta}) {bytes_per_sec}")
            // unwrap is safe: template string is valid and well-tested
            .unwrap()
            .progress_chars("=> "),
    );
    progress_bar
}

/// Check each directory level from `work_dir` to `target_file` to see if there is a `.libraignore`
/// file that matches `target_file`.
///
/// Low-level helper historically used by status/add flows. Prefer the higher-level wrappers in
/// `crate::utils::ignore::{should_ignore, filter_workdir_paths}` so that ignore policies and index
/// awareness stay consistent. Call this directly only when you explicitly need raw `.libraignore`
/// parsing.
///
/// Assume `target_file` is in `work_dir`.
pub fn check_gitignore(work_dir: &PathBuf, target_file: &PathBuf) -> bool {
    assert!(target_file.starts_with(work_dir));
    let mut dir = target_file.clone();
    dir.pop();

    while dir.starts_with(work_dir) {
        let gitignore_path = dir.join(".libraignore");
        if !gitignore_path.exists() {
            dir.pop();
            continue;
        }

        let (ignore, err) = Gitignore::new(&gitignore_path);
        if let Some(e) = err {
            eprintln!(
                "warning: There are some invalid globs in libraignore file {gitignore_path:#?}:\n{e}\n"
            );
        }

        match ignore.matched(target_file, target_file.is_dir()) {
            Match::Ignore(_) => return true,
            Match::Whitelist(_) => return false,
            Match::None => (),
        }

        let mut parent_dir = if target_file.is_dir() {
            target_file.clone()
        } else {
            // unwrap is safe: if target_file is not a dir and has no parent, it's a bug
            target_file.parent().unwrap().to_path_buf()
        };

        while parent_dir.starts_with(work_dir) {
            match ignore.matched(&parent_dir, true) {
                Match::Ignore(_) => return true,
                Match::Whitelist(_) => return false,
                Match::None => (),
            };
            parent_dir.pop();
        }

        dir.pop();
    }

    false
}

use git_internal::internal::object::signature::{Signature, SignatureType};

pub async fn create_signatures() -> (Signature, Signature) {
    let user_name = ConfigKv::get("user.name")
        .await
        .ok()
        .flatten()
        .map(|e| e.value)
        .unwrap_or_else(|| "Stasher".to_string());
    let user_email = ConfigKv::get("user.email")
        .await
        .ok()
        .flatten()
        .map(|e| e.value)
        .unwrap_or_else(|| "stasher@example.com".to_string());

    let author = Signature::new(SignatureType::Author, user_name.clone(), user_email.clone());
    let committer = Signature::new(SignatureType::Committer, user_name, user_email);
    (author, committer)
}

/// Compute the minimum prefix length at which all commit IDs are uniquely identifiable.
///
/// This function inspects the textual object IDs of all `commits` and searches for the
/// smallest prefix length `len` such that the first `len` characters of every commit ID
/// are pairwise distinct. The search range is from `7` (inclusive) up to the maximum
/// hash string length present in `commits` (inclusive).
///
/// Return value semantics:
/// - If `commits` is empty or contains only a single commit, this returns `7`. In these
///   cases, there is no ambiguity, and the conventional minimal prefix length is used.
/// - Otherwise, it returns the smallest `len >= 7` for which all commit ID prefixes of
///   length `len` are unique.
/// - If no such `len` exists before the end of the hash strings, the full hash length
///   (i.e., the maximum ID length observed) is returned.
///
/// This is useful for producing short, Git-style abbreviated IDs that remain unambiguous
/// across the given set of reachable commits.
pub fn get_min_unique_hash_length(commits: &[Commit]) -> usize {
    // Get all commit IDs.
    let hashes: Vec<String> = commits.iter().map(|commit| commit.id.to_string()).collect();
    // If there is no commit or only one commit, return 7.
    if hashes.is_empty() || hashes.len() == 1 {
        7
    } else {
        // Get the maximum length of all commit IDs.
        let max_length = hashes.iter().map(|h| h.len()).max().unwrap_or(0);
        (7..=max_length)
            .find(|&len| {
                let mut prefixes = HashSet::new();
                hashes
                    .iter()
                    // unwrap_or is safe: returns the full hash if slice fails
                    .all(|hash| prefixes.insert(hash.get(0..len).unwrap_or(hash)))
            })
            .unwrap_or(max_length) // Worst case: use full hash length
    }
}

#[cfg(test)]
mod test {
    use std::{env, path::PathBuf};

    use git_internal::internal::object::{
        signature::{Signature, SignatureType},
        tag::Tag as GitTag,
    };
    use sea_orm::{ActiveModelTrait, Set};
    use serial_test::serial;
    use tempfile::tempdir;

    use super::*;
    use crate::{
        command::{
            add::{self, AddArgs},
            commit::{self, CommitArgs},
            save_object,
        },
        internal::{db::get_db_conn_instance, head::Head, model::reference, tag as internal_tag},
        utils::test,
    };

    fn test_tag_object(object_hash: ObjectHash, object_type: ObjectType, name: &str) -> GitTag {
        GitTag::new(
            object_hash,
            object_type,
            name.to_string(),
            Signature {
                signature_type: SignatureType::Tagger,
                name: "tester".to_string(),
                email: "tester@example.com".to_string(),
                timestamp: 1,
                timezone: "+0000".to_string(),
            },
            format!("{name} message"),
        )
    }

    #[test]
    ///Test get current directory success.
    fn cur_dir_returns_current_directory() {
        match env::current_dir() {
            Ok(expected) => {
                let actual = cur_dir();
                assert_eq!(actual, expected);
            }
            Err(_) => {
                // On some Linux/CI environments, current_dir can fail if the working
                // directory was removed. In that case, ensure cur_dir still returns
                // a stable, existing directory via its fallback logic.
                let actual = cur_dir();
                assert!(actual.exists(), "cur_dir should return an existing path");
                assert!(actual.is_dir(), "cur_dir should point to a directory");
            }
        }
    }

    #[test]
    #[serial]
    ///Test the function of is_sub_path.
    fn test_is_sub_path() {
        let _guard = test::ChangeDirGuard::new(Path::new(env!("CARGO_MANIFEST_DIR")));

        assert!(is_sub_path("src/main.rs", "src"));
        assert!(is_sub_path("src/main.rs", "src/"));
        assert!(is_sub_path("src/main.rs", "src/main.rs"));
        assert!(is_sub_path("src/main.rs", "."));
    }

    #[test]
    fn test_is_sub_path_parent_dir_cannot_escape_root() {
        assert!(!is_sub_path("/../../etc/passwd", "/tmp"));
    }

    #[cfg(windows)]
    #[test]
    fn test_is_sub_path_preserves_windows_prefix() {
        assert!(is_sub_path(r"C:\repo\sub\..\file.txt", r"C:\repo"));
        assert!(!is_sub_path(r"C:\repo\..\Windows\System32", r"C:\repo"));
    }

    #[test]
    ///Test the function of to_relative.
    fn test_to_relative() {
        assert_eq!(to_relative("src/main.rs", "src"), PathBuf::from("main.rs"));
        assert_eq!(to_relative(".", "src"), PathBuf::from(".."));
    }

    #[test]
    fn is_empty_dir_returns_false_for_missing_directory() {
        let temp = tempdir().unwrap();
        let missing = temp.path().join("missing");
        assert!(!is_empty_dir(&missing));
    }

    #[test]
    fn clear_empty_dir_ignores_parentless_paths() {
        clear_empty_dir(Path::new(""));
    }

    #[tokio::test]
    #[serial]
    async fn get_commit_base_typed_rejects_unborn_branch_before_hash_fallback() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());

        test::ensure_file("tracked.txt", Some("tracked\n"));
        add::execute(AddArgs {
            pathspec: vec!["tracked.txt".into()],
            all: false,
            update: false,
            refresh: false,
            verbose: false,
            force: false,
            dry_run: false,
            ignore_errors: false,
        })
        .await;
        commit::execute(CommitArgs {
            message: Some("base".into()),
            disable_pre: true,
            no_verify: true,
            ..Default::default()
        })
        .await;

        let head_commit = Head::current_commit()
            .await
            .expect("expected committed HEAD");
        let branch_name = head_commit.to_string()[..7].to_string();

        let db = get_db_conn_instance().await;
        reference::ActiveModel {
            name: Set(Some(branch_name.clone())),
            kind: Set(reference::ConfigKind::Branch),
            commit: Set(None),
            remote: Set(None),
            ..Default::default()
        }
        .insert(&db)
        .await
        .expect("failed to insert unborn branch");

        let error = get_commit_base_typed(&branch_name)
            .await
            .expect_err("unborn branch must not fall back to hash prefix resolution");
        assert!(matches!(error, CommitBaseError::InvalidReference(_)));
        assert!(
            error.to_string().contains(&format!(
                "branch '{branch_name}' does not point to a commit"
            )),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    #[serial]
    async fn get_commit_base_typed_head_navigation_reports_unborn_head() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());

        let error = get_commit_base_typed("HEAD~1")
            .await
            .expect_err("unborn HEAD navigation must not panic");
        assert!(matches!(error, CommitBaseError::HeadUnborn));
    }

    #[tokio::test]
    #[serial]
    async fn get_commit_base_typed_tag_object_hash_with_caret_zero_resolves_commit() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());

        test::ensure_file("tracked.txt", Some("tracked\n"));
        add::execute(AddArgs {
            pathspec: vec!["tracked.txt".into()],
            all: false,
            update: false,
            refresh: false,
            verbose: false,
            force: false,
            dry_run: false,
            ignore_errors: false,
        })
        .await;
        commit::execute(CommitArgs {
            message: Some("base".into()),
            disable_pre: true,
            no_verify: true,
            ..Default::default()
        })
        .await;

        let head_commit = Head::current_commit()
            .await
            .expect("expected committed HEAD");
        let created = internal_tag::create("v1.0.0", Some("release".into()), false)
            .await
            .expect("failed to create annotated tag");

        let resolved = get_commit_base_typed(&format!("{}^0", created.target))
            .await
            .expect("tag object hash ^0 should resolve to the tagged commit");
        assert_eq!(resolved, head_commit);
    }

    #[tokio::test]
    #[serial]
    async fn get_commit_base_typed_peels_nested_tag_object_hash_to_commit() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());

        test::ensure_file("tracked.txt", Some("tracked\n"));
        add::execute(AddArgs {
            pathspec: vec!["tracked.txt".into()],
            all: false,
            update: false,
            refresh: false,
            verbose: false,
            force: false,
            dry_run: false,
            ignore_errors: false,
        })
        .await;
        commit::execute(CommitArgs {
            message: Some("base".into()),
            disable_pre: true,
            no_verify: true,
            ..Default::default()
        })
        .await;

        let head_commit = Head::current_commit()
            .await
            .expect("expected committed HEAD");
        let inner = internal_tag::create("inner", Some("inner tag".into()), false)
            .await
            .expect("failed to create inner tag");
        let outer = test_tag_object(inner.target, ObjectType::Tag, "outer");
        save_object(&outer, &outer.id).expect("failed to save outer tag object");

        let resolved = get_commit_base_typed(&outer.id.to_string())
            .await
            .expect("nested tag object hash should resolve to commit");

        assert_eq!(resolved, head_commit);
    }

    #[tokio::test]
    #[serial]
    async fn get_commit_base_typed_reports_tag_cycle_as_corruption() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());

        test::ensure_file("tracked.txt", Some("tracked\n"));
        add::execute(AddArgs {
            pathspec: vec!["tracked.txt".into()],
            all: false,
            update: false,
            refresh: false,
            verbose: false,
            force: false,
            dry_run: false,
            ignore_errors: false,
        })
        .await;
        commit::execute(CommitArgs {
            message: Some("base".into()),
            disable_pre: true,
            no_verify: true,
            ..Default::default()
        })
        .await;

        let head_commit = Head::current_commit()
            .await
            .expect("expected committed HEAD");
        let tag_a = test_tag_object(head_commit, ObjectType::Commit, "tag-a");
        let tag_b = test_tag_object(tag_a.id, ObjectType::Tag, "tag-b");
        let tag_a_cycle = test_tag_object(tag_b.id, ObjectType::Tag, "tag-a");
        save_object(&tag_b, &tag_b.id).expect("failed to save tag-b");
        save_object(&tag_a_cycle, &tag_a.id).expect("failed to save cyclic tag-a");

        let error = get_commit_base_typed(&tag_a.id.to_string())
            .await
            .expect_err("tag cycle should fail");

        assert!(matches!(error, CommitBaseError::CorruptReference(_)));
        assert!(error.to_string().contains("tag cycle detected"));
    }

    #[tokio::test]
    #[serial]
    ///Test the function of to_workdir_path.
    async fn test_to_workdir_path() {
        let temp_path = tempdir().unwrap();
        test::setup_with_new_libra_in(temp_path.path()).await;
        let _guard = test::ChangeDirGuard::new(temp_path.path());

        assert_eq!(
            to_workdir_path("./src/abc/../main.rs"),
            PathBuf::from("src/main.rs")
        );
        assert_eq!(to_workdir_path("."), PathBuf::from("."));
        assert_eq!(to_workdir_path("./"), PathBuf::from("."));
        assert_eq!(to_workdir_path(""), PathBuf::from("."));
    }

    #[test]
    #[serial]
    /// Tests that files matching patterns in .libraignore are correctly identified as ignored.
    fn test_check_gitignore_ignore_files() {
        let temp_path = tempdir().unwrap();
        let _guard = test::ChangeDirGuard::new(temp_path.path());

        let mut gitignore_file = fs::File::create(".libraignore").unwrap();
        gitignore_file.write_all(b"*.bar").unwrap();

        let target = temp_path.path().join("tmp/foo.bar");
        assert!(check_gitignore(&temp_path.keep(), &target));
    }

    #[test]
    #[serial]
    /// Tests that directories matching patterns in .libraignore are correctly identified as ignored.
    fn test_check_gitignore_ignore_directory() {
        let temp_path = tempdir().unwrap();
        let _guard = test::ChangeDirGuard::new(temp_path.path());

        let mut gitignore_file = fs::File::create(".libraignore").unwrap();
        gitignore_file.write_all(b"foo/").unwrap();

        let target = temp_path.path().join("foo/bar");
        assert!(check_gitignore(&temp_path.keep(), &target));
    }

    #[test]
    #[serial]
    /// Tests ignore pattern matching in subdirectories with .libraignore files at different directory levels.
    fn test_check_gitignore_ignore_subdirectory_files() {
        let temp_path = tempdir().unwrap();
        let _guard = test::ChangeDirGuard::new(temp_path.path());

        fs::create_dir_all("tmp").unwrap();
        fs::create_dir_all("tmp/tmp1").unwrap();
        fs::create_dir_all("tmp/tmp1/tmp2").unwrap();
        let mut gitignore_file1 = fs::File::create("tmp/.libraignore").unwrap();
        gitignore_file1.write_all(b"*.bar").unwrap();
        let workdir = env::current_dir().unwrap();
        let target = workdir.join("tmp/tmp1/tmp2/foo.bar");
        assert!(check_gitignore(&workdir, &target));
        fs::remove_dir_all(workdir.join("tmp")).unwrap();
    }

    #[test]
    #[serial]
    /// Tests that files not matching patterns in .libraignore are correctly identified as not ignored.
    fn test_check_gitignore_not_ignore() {
        let temp_path = tempdir().unwrap();
        let _guard = test::ChangeDirGuard::new(temp_path.path());

        let mut gitignore_file = fs::File::create(".libraignore").unwrap();
        gitignore_file.write_all(b"*.bar").unwrap();
        let workdir = env::current_dir().unwrap();
        let target = workdir.join("tmp/bar.foo");
        assert!(!check_gitignore(&workdir, &target));
        fs::remove_file(workdir.join(".libraignore")).unwrap();
    }

    #[test]
    #[serial]
    /// Tests that files not matching subdirectory-specific patterns in .libraignore are correctly identified as not ignored.
    fn test_check_gitignore_not_ignore_subdirectory_files() {
        let temp_path = tempdir().unwrap();
        let _guard = test::ChangeDirGuard::new(temp_path.path());

        fs::create_dir_all("tmp").unwrap();
        fs::create_dir_all("tmp/tmp1").unwrap();
        fs::create_dir_all("tmp/tmp1/tmp2").unwrap();
        let mut gitignore_file1 = fs::File::create("tmp/.libraignore").unwrap();
        gitignore_file1.write_all(b"tmp/tmp1/tmp2/*.bar").unwrap();
        let workdir = env::current_dir().unwrap();
        let target = workdir.join("tmp/tmp1/tmp2/foo.bar");
        assert!(!check_gitignore(&workdir, &target));
        fs::remove_dir_all(workdir.join("tmp")).unwrap();
    }

    #[test]
    fn test_get_repo_name_from_url_with_git_suffix() {
        assert_eq!(
            get_repo_name_from_url("https://example.com/owner/repo.git"),
            Some("repo")
        );
    }

    #[test]
    fn test_get_repo_name_from_url_without_suffix() {
        assert_eq!(
            get_repo_name_from_url("https://example.com/owner/repo"),
            Some("repo")
        );
    }

    #[test]
    fn test_get_repo_name_from_file_url_without_suffix() {
        assert_eq!(
            get_repo_name_from_url("file:///home/user/projects/repo"),
            Some("repo")
        );
    }

    #[test]
    #[serial]
    fn test_try_get_storage_path_ignores_global_libra_dir_without_repo_markers() {
        let temp = tempdir().unwrap();
        let home_like = temp.path();
        let global_libra = home_like.join(".libra");
        fs::create_dir_all(global_libra.join("vault-keys")).unwrap();
        fs::write(global_libra.join("config.db"), b"not a repo db").unwrap();

        let workdir = home_like.join("workspace").join("project");
        fs::create_dir_all(&workdir).unwrap();

        let _guard = test::ChangeDirGuard::new(&workdir);
        let result = try_get_storage_path(None);

        assert!(
            result.is_err(),
            "global ~/.libra directory without repo markers must not be treated as a repository"
        );
    }
    #[test]
    #[serial]
    fn test_try_get_storage_path_accepts_valid_repo_under_ancestor_with_global_libra_dir() {
        let temp = tempdir().unwrap();
        let home_like = temp.path();
        let global_libra = home_like.join(".libra");
        fs::create_dir_all(global_libra.join("vault-keys")).unwrap();
        fs::write(global_libra.join("config.db"), b"not a repo db").unwrap();

        let repo = home_like.join("workspace").join("repo");
        let storage = repo.join(ROOT_DIR);
        fs::create_dir_all(storage.join("objects")).unwrap();
        fs::create_dir_all(storage.join("hooks")).unwrap();
        fs::create_dir_all(storage.join("info")).unwrap();
        fs::write(storage.join(DATABASE), b"repo db").unwrap();
        fs::write(storage.join("info").join("exclude"), b"").unwrap();

        let nested = repo.join("src");
        fs::create_dir_all(&nested).unwrap();

        let _guard = test::ChangeDirGuard::new(&nested);
        let resolved = try_get_storage_path(None).unwrap();

        assert_eq!(
            resolved.canonicalize().unwrap(),
            storage.canonicalize().unwrap()
        );
    }
    #[test]
    #[serial]
    fn test_try_get_storage_path_rejects_libra_dir_with_only_hooks() {
        let temp = tempdir().unwrap();
        let repo = temp.path().join("project");
        fs::create_dir_all(&repo).unwrap();

        let libra = repo.join(".libra");
        fs::create_dir_all(libra.join("hooks")).unwrap();

        let _guard = test::ChangeDirGuard::new(&repo);
        let result = try_get_storage_path(None);

        assert!(
            result.is_err(),
            ".libra with only hooks/ should not be treated as a valid repository"
        );
    }
    #[test]
    #[serial]
    fn test_try_get_storage_path_rejects_libra_dir_with_only_objects() {
        let temp = tempdir().unwrap();
        let repo = temp.path().join("project");
        fs::create_dir_all(&repo).unwrap();

        let libra = repo.join(".libra");
        fs::create_dir_all(libra.join("objects")).unwrap();

        let _guard = test::ChangeDirGuard::new(&repo);
        let result = try_get_storage_path(None);

        assert!(
            result.is_err(),
            ".libra with only objects/ should not be treated as a valid repository"
        );
    }

    #[test]
    fn test_find_git_repository_detects_worktree_ancestor() {
        let temp = tempdir().unwrap();
        let repo = temp.path().join("project");
        let git = repo.join(".git");
        fs::create_dir_all(git.join("objects")).unwrap();
        fs::write(git.join("HEAD"), b"ref: refs/heads/main\n").unwrap();
        fs::write(
            git.join("config"),
            b"[core]\n\trepositoryformatversion = 0\n",
        )
        .unwrap();
        let nested = repo.join("src").join("lib");
        fs::create_dir_all(&nested).unwrap();

        let location = find_git_repository(Some(&nested)).expect("should detect Git repository");

        assert_eq!(location.root, repo.canonicalize().unwrap());
        assert!(!location.is_bare);
    }

    #[test]
    fn test_find_git_repository_detects_gitdir_file_worktree() {
        let temp = tempdir().unwrap();
        let repo = temp.path().join("linked-worktree");
        let common = temp.path().join("main.git");
        let worktree_git = common.join("worktrees").join("linked-worktree");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(common.join("objects")).unwrap();
        fs::create_dir_all(&worktree_git).unwrap();
        fs::write(
            repo.join(".git"),
            format!("gitdir: {}\n", worktree_git.display()),
        )
        .unwrap();
        fs::write(
            common.join("config"),
            b"[core]\n\trepositoryformatversion = 0\n",
        )
        .unwrap();
        fs::write(worktree_git.join("HEAD"), b"ref: refs/heads/main\n").unwrap();
        fs::write(worktree_git.join("commondir"), b"../..\n").unwrap();

        let location = find_git_repository(Some(&repo)).expect("should detect Git worktree");

        assert_eq!(location.root, repo.canonicalize().unwrap());
        assert!(!location.is_bare);
    }

    #[test]
    fn test_find_git_repository_treats_dot_git_directory_as_worktree() {
        let temp = tempdir().unwrap();
        let repo = temp.path().join("project");
        let git = repo.join(".git");
        fs::create_dir_all(git.join("objects").join("aa")).unwrap();
        fs::write(git.join("HEAD"), b"ref: refs/heads/main\n").unwrap();
        fs::write(
            git.join("config"),
            b"[core]\n\trepositoryformatversion = 0\n\tbare = false\n",
        )
        .unwrap();

        let location = find_git_repository(Some(&git.join("objects")))
            .expect("should detect parent Git worktree from inside .git");

        assert_eq!(location.root, repo.canonicalize().unwrap());
        assert!(!location.is_bare);
    }

    #[test]
    fn test_find_git_repository_keeps_bare_dot_git_directory_bare() {
        let temp = tempdir().unwrap();
        let bare = temp.path().join(".git");
        fs::create_dir_all(bare.join("objects")).unwrap();
        fs::write(bare.join("HEAD"), b"ref: refs/heads/main\n").unwrap();
        fs::write(
            bare.join("config"),
            b"[core]\n\trepositoryformatversion = 0\n\tbare = true\n",
        )
        .unwrap();

        let location = find_git_repository(Some(&bare))
            .expect("should detect a bare repository named .git as bare");

        assert_eq!(location.root, bare.canonicalize().unwrap());
        assert!(location.is_bare);
    }
}
