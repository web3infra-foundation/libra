//! Core utility toolbox for repo detection, path conversion, ignore checking, storage access, hashing helpers, and miscellaneous formatting/time utilities.

use std::{
    collections::{HashMap, HashSet},
    env, fs, io,
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
        head::Head,
        tag,
    },
    utils::{client_storage::ClientStorage, path, path_ext::PathExt},
};

pub const ROOT_DIR: &str = ".libra";
pub const DATABASE: &str = "libra.db";
pub const ATTRIBUTES: &str = ".libra_attributes";

static OBJECTS_STORAGE_CACHE: Lazy<Mutex<HashMap<PathBuf, ClientStorage>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Returns the current working directory as a `PathBuf`.
///
/// This function wraps the `std::env::current_dir()` function and unwraps the result.
/// If the current directory value is not available for any reason, this function will panic.
///
/// TODO - Add additional check result from `std::env::current_dir()` to handle the panic
///
/// # Returns
///
/// A `PathBuf` representing the current working directory.
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

fn try_get_paths(path: Option<PathBuf>) -> Result<(PathBuf, PathBuf), io::Error> {
    let mut path = path.clone().unwrap_or_else(cur_dir);
    let orig = path.clone();

    loop {
        let standard_repo = path.join(ROOT_DIR);
        if standard_repo.is_dir() && is_valid_storage_dir(&standard_repo) {
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
    try_get_storage_path(None).unwrap()
}

/// Return an error instead of printing when the current directory is not a repository.
pub fn require_repo() -> io::Result<()> {
    try_get_storage_path(None).map(|_| ())
}

/// Legacy repository check that still prints for commands not yet migrated.
pub fn check_repo_exist() -> bool {
    if require_repo().is_err() {
        crate::utils::error::emit_legacy_stderr(
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
        .expect("objects storage cache mutex poisoned");
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

/// Integrate the input paths (relative, absolute, file, dir) to workdir paths
/// - only include existing files
pub fn integrate_pathspec(paths: &Vec<PathBuf>) -> HashSet<PathBuf> {
    let mut workdir_paths = HashSet::new();
    for path in paths {
        if path.is_dir() {
            let files = list_files(path).unwrap(); // to workdir
            workdir_paths.extend(files);
        } else {
            workdir_paths.insert(path.to_workdir());
        }
    }
    workdir_paths
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
        dir.parent().unwrap().to_path_buf()
    };

    let repo = storage_path();
    // CAN NOT remove .libra & current dir
    while !is_sub_path(&repo, &dir) && !is_cur_dir(&dir) {
        if is_empty_dir(&dir) {
            fs::remove_dir(&dir).unwrap();
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
    fs::read_dir(dir).unwrap().next().is_none()
}

pub fn is_cur_dir(dir: &Path) -> bool {
    dir.absolutize().unwrap() == cur_dir().absolutize().unwrap()
}

/// transform path to string, use '/' as separator even on windows
/// TODO test on windows
/// TODO maybe 'into_os_string().into_string().unwrap()' is good
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
    if name.eq_ignore_ascii_case("HEAD") {
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

    // Support both short remote branches (`main` with `remote = origin`) and
    // fetched remote-tracking refs (`refs/remotes/origin/main`) for inputs such
    // as `origin/main`.
    if let Some((remote, branch_name)) = name.split_once('/')
        && !remote.is_empty()
        && !branch_name.is_empty()
    {
        if let Some(commit) = resolve_branch_commit_typed(
            &format!("refs/remotes/{remote}/{branch_name}"),
            Some(remote),
            name,
        )
        .await?
        {
            return Ok(commit);
        }

        if let Some(commit) = resolve_branch_commit_typed(branch_name, Some(remote), name).await? {
            return Ok(commit);
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
        ObjectType::Tag => {
            // Manually dereference tag if search returned a tag object directly
            let tag_obj: git_internal::internal::object::tag::Tag =
                crate::command::load_object(&object_id).map_err(|e| {
                    CommitBaseError::classify_storage_failure(format!(
                        "failed to load tag object: {e}"
                    ))
                })?;
            Ok(tag_obj.object_hash)
        }
        _ => Err(CommitBaseError::InvalidReference(format!(
            "reference is not a commit: {name}, is {object_type}"
        ))),
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

use crate::internal::config::ConfigKv;

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
                    .all(|hash| prefixes.insert(hash.get(0..len).unwrap_or(hash)))
            })
            .unwrap_or(max_length) // Worst case: use full hash length
    }
}

#[cfg(test)]
mod test {
    use std::{env, path::PathBuf};

    use sea_orm::{ActiveModelTrait, Set};
    use serial_test::serial;
    use tempfile::tempdir;

    use super::*;
    use crate::{
        command::{
            add::{self, AddArgs},
            commit::{self, CommitArgs},
        },
        internal::{db::get_db_conn_instance, head::Head, model::reference},
        utils::test,
    };

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
}
