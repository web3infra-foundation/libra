//! Implements restore flows to reset files or entire trees from commits or the index, respecting pathspecs and staged vs worktree targets.

use std::{
    collections::{HashMap, HashSet},
    fs, io,
    path::{Path, PathBuf},
};

use clap::Parser;
use git_internal::{
    hash::ObjectHash,
    internal::{
        index::{Index, IndexEntry},
        object::{blob::Blob, commit::Commit, tree::Tree, types::ObjectType},
    },
};
use serde::Serialize;

use crate::{
    command::{calc_file_blob_hash, load_object},
    internal::{
        branch::{Branch, BranchStoreError},
        head::Head,
        protocol::lfs_client::LFSClient,
    },
    utils::{
        client_storage::ClientStorage,
        error::{CliError, CliResult, StableErrorCode},
        lfs,
        object_ext::{BlobExt, CommitExt, TreeExt},
        output::{OutputConfig, emit_json_data},
        path,
        path_ext::PathExt,
        util,
    },
};

const RESTORE_EXAMPLES: &str = "\
EXAMPLES:
    libra restore file.txt                Restore file from index to worktree
    libra restore --staged file.txt       Unstage a file (restore index from HEAD)
    libra restore --source HEAD~1 .       Restore all files from a previous commit
    libra restore -S -W file.txt          Restore both worktree and index
    libra restore --json --source HEAD .  Structured JSON output for agents";

// ── Typed error ──────────────────────────────────────────────────────

/// Typed error for checkout / restore operations, providing enough detail for
/// callers (e.g. `clone`) to map each failure into a stable error code without
/// resorting to string matching on `io::Error` messages.
#[derive(thiserror::Error, Debug)]
pub enum RestoreError {
    #[error("failed to resolve checkout source")]
    ResolveSource,
    #[error("reference is not a commit")]
    ReferenceNotCommit,
    #[error("pathspec '{0}' did not match any files")]
    PathspecNotMatched(String),
    #[error("failed to read index")]
    ReadIndex,
    #[error("failed to read object")]
    ReadObject,
    #[error("failed to read worktree")]
    ReadWorktree,
    #[error("invalid path encoding")]
    InvalidPathEncoding,
    #[error("failed to write worktree file")]
    WriteWorktree,
    #[error("failed to download LFS content")]
    LfsDownload,
}

impl RestoreError {
    fn stable_code(&self) -> StableErrorCode {
        match self {
            Self::ResolveSource => StableErrorCode::CliInvalidTarget,
            Self::ReferenceNotCommit => StableErrorCode::CliInvalidTarget,
            Self::PathspecNotMatched(_) => StableErrorCode::CliInvalidTarget,
            Self::ReadIndex => StableErrorCode::IoReadFailed,
            Self::ReadObject => StableErrorCode::IoReadFailed,
            Self::ReadWorktree => StableErrorCode::IoReadFailed,
            Self::InvalidPathEncoding => StableErrorCode::CliInvalidArguments,
            Self::WriteWorktree => StableErrorCode::IoWriteFailed,
            Self::LfsDownload => StableErrorCode::NetworkUnavailable,
        }
    }
}

impl From<RestoreError> for CliError {
    fn from(error: RestoreError) -> Self {
        let stable_code = error.stable_code();
        let message = error.to_string();
        match error {
            // Ref resolution keeps Git-compatible exit 128 semantics even though
            // the stable code stays target-oriented for machine classification.
            RestoreError::ResolveSource => CliError::fatal(message)
                .with_stable_code(stable_code)
                .with_exit_code(128)
                .with_hint("check that the source ref exists with 'libra log'"),
            RestoreError::ReferenceNotCommit => CliError::fatal(message)
                .with_stable_code(stable_code)
                .with_exit_code(128)
                .with_hint("only commit references can be used as restore source"),
            RestoreError::PathspecNotMatched(_) => CliError::fatal(message)
                .with_stable_code(stable_code)
                .with_hint("check the path and try again"),
            RestoreError::LfsDownload => CliError::fatal(message)
                .with_stable_code(stable_code)
                .with_hint("check LFS server availability"),
            _ => CliError::fatal(message).with_stable_code(stable_code),
        }
    }
}

// ── Structured output ────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct RestoreOutput {
    pub source: Option<String>,
    pub worktree: bool,
    pub staged: bool,
    pub restored_files: Vec<String>,
    pub deleted_files: Vec<String>,
}

// ── Entry points ─────────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(about = "Restore working tree files")]
#[command(after_help = RESTORE_EXAMPLES)]
pub struct RestoreArgs {
    /// files or dir to restore
    #[clap(required = true)]
    pub pathspec: Vec<String>,
    /// source
    #[clap(long, short)]
    pub source: Option<String>,
    /// worktree
    #[clap(long, short = 'W')]
    pub worktree: bool,
    /// staged
    #[clap(long, short = 'S')]
    pub staged: bool,
}

pub async fn execute(args: RestoreArgs) {
    if let Err(e) = execute_safe(args, &OutputConfig::default()).await {
        e.print_stderr();
    }
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting.
///
/// # Side Effects
/// - Restores selected paths from the index or a commit tree.
/// - May rewrite index entries when `--staged` is set.
/// - May overwrite working-tree files when the worktree target is active.
/// - Renders human or JSON output for restored paths.
///
/// # Errors
/// Returns [`CliError`] when the repository is missing, the source revision or
/// pathspecs cannot be resolved, object reads fail, or index/worktree writes
/// fail.
pub async fn execute_safe(args: RestoreArgs, output: &OutputConfig) -> CliResult<()> {
    util::require_repo().map_err(|_| CliError::repo_not_found())?;
    let result = run_restore(args).await.map_err(CliError::from)?;
    render_restore_output(&result, output)
}

// ── Core execution ───────────────────────────────────────────────────

async fn run_restore(args: RestoreArgs) -> Result<RestoreOutput, RestoreError> {
    let staged = args.staged;
    let mut worktree = args.worktree;
    if !staged {
        worktree = true;
    }

    const HEAD: &str = "HEAD";
    let mut source = args.source;
    if source.is_none() && staged {
        source = Some(HEAD.to_string());
    }

    let storage = util::objects_storage();
    let target_blobs = resolve_target_blobs(source.as_deref(), staged, &storage).await?;

    let paths = args
        .pathspec
        .iter()
        .map(PathBuf::from)
        .collect::<Vec<PathBuf>>();

    let mut restored_files = Vec::new();
    let mut deleted_files = Vec::new();

    if worktree {
        let (restored, deleted) = restore_worktree_tracked(&paths, &target_blobs).await?;
        restored_files.extend(restored);
        deleted_files.extend(deleted);
    }
    if staged {
        let (restored, deleted) = restore_index_tracked(&paths, &target_blobs)?;
        let mut restored_seen: HashSet<String> = restored_files.iter().cloned().collect();
        let mut deleted_seen: HashSet<String> = deleted_files.iter().cloned().collect();

        for f in restored {
            if restored_seen.insert(f.clone()) {
                restored_files.push(f);
            }
        }
        for f in deleted {
            if deleted_seen.insert(f.clone()) {
                deleted_files.push(f);
            }
        }
    }

    Ok(RestoreOutput {
        source: source.clone(),
        worktree,
        staged,
        restored_files,
        deleted_files,
    })
}

// ── Rendering ────────────────────────────────────────────────────────

fn render_restore_output(result: &RestoreOutput, output: &OutputConfig) -> CliResult<()> {
    if output.is_json() {
        return emit_json_data("restore", result, output);
    }

    if output.quiet {
        return Ok(());
    }

    let total = result.restored_files.len() + result.deleted_files.len();
    if total > 0 {
        let source_desc = result.source.as_deref().unwrap_or("the index");
        println!("Updated {total} path(s) from {source_desc}");
    }
    Ok(())
}

// ── Resolve target blobs ─────────────────────────────────────────────

async fn resolve_target_blobs(
    source: Option<&str>,
    staged: bool,
    storage: &ClientStorage,
) -> Result<Vec<(PathBuf, ObjectHash)>, RestoreError> {
    const HEAD: &str = "HEAD";

    match source {
        None => {
            if staged {
                return Err(RestoreError::ResolveSource);
            }
            let index = Index::load(path::index()).map_err(|_| RestoreError::ReadIndex)?;
            Ok(index
                .tracked_entries(0)
                .into_iter()
                .map(|entry| (PathBuf::from(&entry.name), entry.hash))
                .collect())
        }
        Some(src) => {
            let commit = if src == HEAD {
                Head::current_commit_result()
                    .await
                    .map_err(map_restore_branch_store_error)?
                    .ok_or(RestoreError::ResolveSource)?
            } else if src.contains('~') || src.contains('^') {
                util::get_commit_base_typed(src)
                    .await
                    .map_err(|_| RestoreError::ResolveSource)?
            } else {
                resolve_source_commit(src, storage).await?
            };

            let tree_id = load_object::<Commit>(&commit)
                .map_err(|_| RestoreError::ReadObject)?
                .tree_id;
            Ok(load_object::<Tree>(&tree_id)
                .map_err(|_| RestoreError::ReadObject)?
                .get_plain_items())
        }
    }
}

// ── Worktree restore (unified typed path) ────────────────────────────

async fn restore_worktree_tracked(
    filter: &[PathBuf],
    target_blobs: &[(PathBuf, ObjectHash)],
) -> Result<(Vec<String>, Vec<String>), RestoreError> {
    let target_map = preprocess_blobs(target_blobs);
    let deleted_files = get_worktree_deleted_files_in_filters(filter, &target_map);

    for path in filter {
        if !path.exists()
            && !target_map
                .iter()
                .any(|(p, _)| util::is_sub_path(p.workdir_to_absolute(), path))
        {
            return Err(pathspec_not_matched(path));
        }
    }

    let mut file_paths =
        util::integrate_pathspec(filter).map_err(|_| RestoreError::ReadWorktree)?;
    file_paths.extend(deleted_files);

    let index = Index::load(path::index()).map_err(|_| RestoreError::ReadIndex)?;
    let mut restored = Vec::new();
    let mut deleted = Vec::new();

    for path_wd in &file_paths {
        let path_abs = util::workdir_to_absolute(path_wd);
        if !path_abs.exists() {
            if target_map.contains_key(path_wd) {
                restore_to_file_typed(&target_map[path_wd], path_wd).await?;
                restored.push(path_wd.display().to_string());
            } else {
                return Err(pathspec_not_matched(path_wd));
            }
        } else {
            let path_wd_str = path_to_utf8_typed(path_wd)?;
            let hash = calc_file_blob_hash(&path_abs).map_err(|_| RestoreError::ReadObject)?;
            if target_map.contains_key(path_wd) {
                if hash != target_map[path_wd] {
                    restore_to_file_typed(&target_map[path_wd], path_wd).await?;
                    restored.push(path_wd.display().to_string());
                }
            } else if index.tracked(path_wd_str, 0) {
                fs::remove_file(&path_abs).map_err(|_| RestoreError::WriteWorktree)?;
                util::clear_empty_dir(&path_abs);
                deleted.push(path_wd.display().to_string());
            }
        }
    }

    Ok((restored, deleted))
}

// ── Index restore (unified typed path) ───────────────────────────────

fn restore_index_tracked(
    filter: &[PathBuf],
    target_blobs: &[(PathBuf, ObjectHash)],
) -> Result<(Vec<String>, Vec<String>), RestoreError> {
    let target_map = preprocess_blobs(target_blobs);

    let idx_file = path::index();
    let mut index = Index::load(&idx_file).map_err(|_| RestoreError::ReadIndex)?;
    let deleted_files_index =
        get_index_deleted_files_in_filters_typed(&index, filter, &target_map)?;

    let filter_vec = filter.to_vec();
    let mut file_paths = util::filter_to_fit_paths(&index.tracked_files(), &filter_vec);
    file_paths.extend(deleted_files_index);

    let mut restored = Vec::new();
    let mut deleted = Vec::new();

    for path in &file_paths {
        let path_str = path_to_utf8_typed(path)?;
        if !index.tracked(path_str, 0) {
            if target_map.contains_key(path) {
                let hash = target_map[path];
                let blob = load_object::<Blob>(&hash).map_err(|_| RestoreError::ReadObject)?;
                index.add(IndexEntry::new_from_blob(
                    path_str.to_string(),
                    hash,
                    blob.data.len() as u32,
                ));
                restored.push(path.display().to_string());
            } else {
                return Err(pathspec_not_matched(path));
            }
        } else if target_map.contains_key(path) {
            let hash = target_map[path];
            if !index.verify_hash(path_str, 0, &hash) {
                let blob = load_object::<Blob>(&hash).map_err(|_| RestoreError::ReadObject)?;
                index.update(IndexEntry::new_from_blob(
                    path_str.to_string(),
                    hash,
                    blob.data.len() as u32,
                ));
                restored.push(path.display().to_string());
            }
        } else {
            index.remove(path_str, 0);
            deleted.push(path.display().to_string());
        }
    }

    index
        .save(&idx_file)
        .map_err(|_| RestoreError::WriteWorktree)?;

    Ok((restored, deleted))
}

// ── Legacy public API (used by worktree.rs and checkout) ─────────────

/// Low-level restore that skips the repository-existence check.
///
/// # Preconditions
///
/// The caller **must** ensure a valid libra repository is reachable from the
/// current working directory (e.g. by calling `util::require_repo()` or
/// `execute_safe()` first).  This function is `pub` because it is used by
/// `worktree.rs`, which performs its own repository validation.
pub async fn execute_checked(args: RestoreArgs) -> io::Result<()> {
    let staged = args.staged;
    let mut worktree = args.worktree;
    if !staged {
        worktree = true;
    }

    const HEAD: &str = "HEAD";
    let mut source = args.source;
    if source.is_none() && staged {
        source = Some(HEAD.to_string());
    }

    let storage = util::objects_storage();
    let target_commit: Option<ObjectHash> = match source {
        None => {
            assert!(!staged);
            None
        }
        Some(ref src) => {
            if src == HEAD {
                Some(
                    Head::current_commit_result()
                        .await
                        .map_err(|error| io::Error::other(error.to_string()))?
                        .ok_or_else(|| io::Error::other("could not resolve HEAD"))?,
                )
            } else if src.contains('~') || src.contains('^') {
                Some(
                    util::get_commit_base_typed(src)
                        .await
                        .map_err(|error| io::Error::other(error.to_string()))?,
                )
            } else {
                resolve_source_commit_io(src, &storage)
                    .await
                    .map(Some)
                    .map_err(|error| io::Error::other(error.to_string()))?
            }
        }
    };

    let target_blobs: Vec<(PathBuf, ObjectHash)> = {
        match (source.as_ref(), target_commit) {
            (None, _) => {
                assert!(!staged);
                let index =
                    Index::load(path::index()).map_err(|e| io::Error::other(e.to_string()))?;
                index
                    .tracked_entries(0)
                    .into_iter()
                    .map(|entry| (PathBuf::from(&entry.name), entry.hash))
                    .collect()
            }
            (Some(_), Some(commit)) => {
                let tree_id = Commit::load(&commit).tree_id;
                let tree = Tree::load(&tree_id);
                tree.get_plain_items()
            }
            (Some(src), None) => {
                if storage
                    .search_result(src)
                    .await
                    .map_err(|error| io::Error::other(error.to_string()))?
                    .len()
                    != 1
                {
                    return Err(io::Error::other(format!("could not resolve {src}")));
                } else {
                    return Err(io::Error::other(format!(
                        "reference is not a commit: {src}"
                    )));
                }
            }
        }
    };

    let paths = args
        .pathspec
        .iter()
        .map(PathBuf::from)
        .collect::<Vec<PathBuf>>();

    if worktree {
        restore_worktree(&paths, &target_blobs).await?;
    }
    if staged {
        restore_index(&paths, &target_blobs)?;
    }
    Ok(())
}

/// Typed checkout entry point that returns [`RestoreError`] instead of
/// `io::Error`, allowing callers like `clone` to map each failure category
/// into a distinct stable error code.
pub async fn execute_checked_typed(args: RestoreArgs) -> Result<(), RestoreError> {
    let staged = args.staged;
    let mut worktree = args.worktree;
    if !staged {
        worktree = true;
    }

    const HEAD: &str = "HEAD";
    let mut source = args.source;
    if source.is_none() && staged {
        source = Some(HEAD.to_string());
    }

    let storage = util::objects_storage();
    let target_blobs: Vec<(PathBuf, ObjectHash)> = match source.as_ref() {
        None => {
            if staged {
                return Err(RestoreError::ResolveSource);
            }
            let index = Index::load(path::index()).map_err(|_| RestoreError::ReadIndex)?;
            index
                .tracked_entries(0)
                .into_iter()
                .map(|entry| (PathBuf::from(&entry.name), entry.hash))
                .collect()
        }
        Some(src) => {
            let commit = if src == HEAD {
                Head::current_commit_result()
                    .await
                    .map_err(map_restore_branch_store_error)?
                    .ok_or(RestoreError::ResolveSource)?
            } else {
                resolve_source_commit(src, &storage).await?
            };

            let tree_id = load_object::<Commit>(&commit)
                .map_err(|_| RestoreError::ReadObject)?
                .tree_id;
            load_object::<Tree>(&tree_id)
                .map_err(|_| RestoreError::ReadObject)?
                .get_plain_items()
        }
    };

    let paths = args.pathspec.iter().map(PathBuf::from).collect::<Vec<_>>();
    if worktree {
        restore_worktree_legacy_typed(&paths, &target_blobs).await?;
    }
    if staged {
        restore_index_legacy_typed(&paths, &target_blobs)?;
    }
    Ok(())
}

// ── Shared helpers ───────────────────────────────────────────────────

async fn resolve_source_commit(
    src: &str,
    storage: &ClientStorage,
) -> Result<ObjectHash, RestoreError> {
    if let Some(branch) = Branch::find_branch_result(src, None)
        .await
        .map_err(map_restore_branch_store_error)?
    {
        return Ok(branch.commit);
    }

    if Branch::exists_result(src, None)
        .await
        .map_err(map_restore_branch_store_error)?
    {
        return Err(RestoreError::ResolveSource);
    }

    let objs = storage
        .search_result(src)
        .await
        .map_err(|_| RestoreError::ReadObject)?;
    if objs.len() != 1 {
        return Err(RestoreError::ResolveSource);
    }
    if !storage.is_object_type(&objs[0], ObjectType::Commit) {
        return Err(RestoreError::ReferenceNotCommit);
    }
    Ok(objs[0])
}

async fn resolve_source_commit_io(
    src: &str,
    storage: &ClientStorage,
) -> Result<ObjectHash, String> {
    if let Some(branch) = Branch::find_branch_result(src, None)
        .await
        .map_err(|e| e.to_string())?
    {
        return Ok(branch.commit);
    }

    if Branch::exists_result(src, None)
        .await
        .map_err(|e| e.to_string())?
    {
        return Err(format!("could not resolve {src}"));
    }

    let objs = storage
        .search_result(src)
        .await
        .map_err(|e| e.to_string())?;
    if objs.len() != 1 {
        return Err(format!("could not resolve {src}"));
    }
    if !storage.is_object_type(&objs[0], ObjectType::Commit) {
        return Err(format!("reference is not a commit: {src}"));
    }
    Ok(objs[0])
}

fn map_restore_branch_store_error(error: BranchStoreError) -> RestoreError {
    match error {
        BranchStoreError::Query(_) => RestoreError::ReadObject,
        BranchStoreError::Corrupt { .. } => RestoreError::ReadObject,
        BranchStoreError::NotFound(_) => RestoreError::ResolveSource,
        BranchStoreError::Delete { .. } => RestoreError::ReadObject,
    }
}

fn preprocess_blobs(blobs: &[(PathBuf, ObjectHash)]) -> HashMap<PathBuf, ObjectHash> {
    blobs
        .iter()
        .map(|(path, hash)| (path.clone(), *hash))
        .collect()
}

fn path_to_utf8(path: &Path) -> io::Result<&str> {
    path.to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("non-UTF8 path: {}", path.display()),
        )
    })
}

fn path_to_utf8_typed(path: &Path) -> Result<&str, RestoreError> {
    path.to_str().ok_or(RestoreError::InvalidPathEncoding)
}

fn pathspec_not_matched(path: &Path) -> RestoreError {
    RestoreError::PathspecNotMatched(path.display().to_string())
}

async fn restore_to_file_typed(hash: &ObjectHash, path: &PathBuf) -> Result<(), RestoreError> {
    let blob = load_object::<Blob>(hash).map_err(|_| RestoreError::ReadObject)?;
    let path_abs = util::workdir_to_absolute(path);
    if let Some(parent) = path_abs.parent() {
        fs::create_dir_all(parent).map_err(|_| RestoreError::WriteWorktree)?;
    }

    match lfs::parse_pointer_data(&blob.data) {
        Some((oid, size)) => {
            let lfs_obj_path = lfs::lfs_object_path(&oid);
            if lfs_obj_path.exists() {
                fs::copy(&lfs_obj_path, &path_abs).map_err(|_| RestoreError::WriteWorktree)?;
            } else {
                LFSClient::get()
                    .await
                    .download_object(&oid, size, &path_abs, None)
                    .await
                    .map_err(|_| RestoreError::LfsDownload)?;
            }
        }
        None => {
            util::write_file(&blob.data, &path_abs).map_err(|_| RestoreError::WriteWorktree)?;
        }
    }

    Ok(())
}

/// Restore a blob to file.
/// If blob is an LFS pointer, download the actual file from LFS server.
/// - `path` : to workdir
pub async fn restore_to_file(hash: &ObjectHash, path: &PathBuf) -> io::Result<()> {
    let blob = Blob::load(hash);
    let path_abs = util::workdir_to_absolute(path);
    if let Some(parent) = path_abs.parent() {
        fs::create_dir_all(parent)?;
    }
    match lfs::parse_pointer_data(&blob.data) {
        Some((oid, size)) => {
            let lfs_obj_path = lfs::lfs_object_path(&oid);
            if lfs_obj_path.exists() {
                fs::copy(&lfs_obj_path, &path_abs)?;
            } else if let Err(e) = LFSClient::get()
                .await
                .download_object(&oid, size, &path_abs, None)
                .await
            {
                return Err(io::Error::other(e.to_string()));
            }
        }
        None => {
            util::write_file(&blob.data, &path_abs)?;
        }
    }
    Ok(())
}

fn get_worktree_deleted_files_in_filters(
    filters: &[PathBuf],
    target_blobs: &HashMap<PathBuf, ObjectHash>,
) -> HashSet<PathBuf> {
    target_blobs
        .iter()
        .filter(|(path, _)| {
            let path = util::workdir_to_absolute(path);
            !path.exists() && path.sub_of_paths(filters)
        })
        .map(|(path, _)| path.clone())
        .collect()
}

// ── Legacy worktree/index restore (kept for execute_checked) ─────────

pub async fn restore_worktree(
    filter: &[PathBuf],
    target_blobs: &[(PathBuf, ObjectHash)],
) -> io::Result<()> {
    let target_blobs = preprocess_blobs(target_blobs);
    let deleted_files = get_worktree_deleted_files_in_filters(filter, &target_blobs);

    {
        for path in filter {
            if !path.exists()
                && !target_blobs
                    .iter()
                    .any(|(p, _)| util::is_sub_path(p.workdir_to_absolute(), path))
            {
                return Err(io::Error::other(format!(
                    "pathspec '{}' did not match any files",
                    path.display()
                )));
            }
        }
    }

    let mut file_paths = util::integrate_pathspec(filter)?;
    file_paths.extend(deleted_files);

    let index = Index::load(path::index()).map_err(|e| io::Error::other(e.to_string()))?;
    for path_wd in &file_paths {
        let path_abs = util::workdir_to_absolute(path_wd);
        if !path_abs.exists() {
            if target_blobs.contains_key(path_wd) {
                restore_to_file(&target_blobs[path_wd], path_wd).await?;
            } else {
                return Err(io::Error::other(format!(
                    "pathspec '{}' did not match any files",
                    path_wd.display()
                )));
            }
        } else {
            let path_wd_str = path_to_utf8(path_wd)?;
            let hash =
                calc_file_blob_hash(&path_abs).map_err(|e| io::Error::other(e.to_string()))?;
            if target_blobs.contains_key(path_wd) {
                if hash != target_blobs[path_wd] {
                    restore_to_file(&target_blobs[path_wd], path_wd).await?;
                }
            } else if index.tracked(path_wd_str, 0) {
                fs::remove_file(&path_abs)?;
                util::clear_empty_dir(&path_abs);
            }
        }
    }
    Ok(())
}

async fn restore_worktree_legacy_typed(
    filter: &[PathBuf],
    target_blobs: &[(PathBuf, ObjectHash)],
) -> Result<(), RestoreError> {
    let target_blobs = preprocess_blobs(target_blobs);
    let deleted_files = get_worktree_deleted_files_in_filters(filter, &target_blobs);

    for path in filter {
        if !path.exists()
            && !target_blobs
                .iter()
                .any(|(p, _)| util::is_sub_path(p.workdir_to_absolute(), path))
        {
            return Err(pathspec_not_matched(path));
        }
    }

    let mut file_paths =
        util::integrate_pathspec(filter).map_err(|_| RestoreError::ReadWorktree)?;
    file_paths.extend(deleted_files);

    let index = Index::load(path::index()).map_err(|_| RestoreError::ReadIndex)?;
    for path_wd in &file_paths {
        let path_abs = util::workdir_to_absolute(path_wd);
        if !path_abs.exists() {
            if target_blobs.contains_key(path_wd) {
                restore_to_file_typed(&target_blobs[path_wd], path_wd).await?;
            } else {
                return Err(pathspec_not_matched(path_wd));
            }
        } else {
            let path_wd_str = path_to_utf8_typed(path_wd)?;
            let hash = calc_file_blob_hash(&path_abs).map_err(|_| RestoreError::ReadObject)?;
            if target_blobs.contains_key(path_wd) {
                if hash != target_blobs[path_wd] {
                    restore_to_file_typed(&target_blobs[path_wd], path_wd).await?;
                }
            } else if index.tracked(path_wd_str, 0) {
                fs::remove_file(&path_abs).map_err(|_| RestoreError::WriteWorktree)?;
                util::clear_empty_dir(&path_abs);
            }
        }
    }

    Ok(())
}

pub fn restore_index(filter: &[PathBuf], target_blobs: &[(PathBuf, ObjectHash)]) -> io::Result<()> {
    let target_blobs = preprocess_blobs(target_blobs);

    let idx_file = path::index();
    let mut index = Index::load(&idx_file).map_err(|e| io::Error::other(e.to_string()))?;
    let deleted_files_index = get_index_deleted_files_in_filters(&index, filter, &target_blobs)?;

    let filter_vec = filter.to_vec();
    let mut file_paths = util::filter_to_fit_paths(&index.tracked_files(), &filter_vec);
    file_paths.extend(deleted_files_index);

    for path in &file_paths {
        let path_str = path_to_utf8(path)?;
        if !index.tracked(path_str, 0) {
            if target_blobs.contains_key(path) {
                let hash = target_blobs[path];
                let blob = Blob::load(&hash);
                index.add(IndexEntry::new_from_blob(
                    path_str.to_string(),
                    hash,
                    blob.data.len() as u32,
                ));
            } else {
                return Err(io::Error::other(format!(
                    "pathspec '{}' did not match any files",
                    path.display()
                )));
            }
        } else if target_blobs.contains_key(path) {
            let hash = target_blobs[path];
            if !index.verify_hash(path_str, 0, &hash) {
                let blob = Blob::load(&hash);
                index.update(IndexEntry::new_from_blob(
                    path_str.to_string(),
                    hash,
                    blob.data.len() as u32,
                ));
            }
        } else {
            index.remove(path_str, 0);
        }
    }
    index
        .save(&idx_file)
        .map_err(|e| io::Error::other(e.to_string()))?;
    Ok(())
}

fn restore_index_legacy_typed(
    filter: &[PathBuf],
    target_blobs: &[(PathBuf, ObjectHash)],
) -> Result<(), RestoreError> {
    let target_blobs = preprocess_blobs(target_blobs);

    let idx_file = path::index();
    let mut index = Index::load(&idx_file).map_err(|_| RestoreError::ReadIndex)?;
    let deleted_files_index =
        get_index_deleted_files_in_filters_typed(&index, filter, &target_blobs)?;

    let filter_vec = filter.to_vec();
    let mut file_paths = util::filter_to_fit_paths(&index.tracked_files(), &filter_vec);
    file_paths.extend(deleted_files_index);

    for path in &file_paths {
        let path_str = path_to_utf8_typed(path)?;
        if !index.tracked(path_str, 0) {
            if target_blobs.contains_key(path) {
                let hash = target_blobs[path];
                let blob = load_object::<Blob>(&hash).map_err(|_| RestoreError::ReadObject)?;
                index.add(IndexEntry::new_from_blob(
                    path_str.to_string(),
                    hash,
                    blob.data.len() as u32,
                ));
            } else {
                return Err(pathspec_not_matched(path));
            }
        } else if target_blobs.contains_key(path) {
            let hash = target_blobs[path];
            if !index.verify_hash(path_str, 0, &hash) {
                let blob = load_object::<Blob>(&hash).map_err(|_| RestoreError::ReadObject)?;
                index.update(IndexEntry::new_from_blob(
                    path_str.to_string(),
                    hash,
                    blob.data.len() as u32,
                ));
            }
        } else {
            index.remove(path_str, 0);
        }
    }

    index
        .save(&idx_file)
        .map_err(|_| RestoreError::WriteWorktree)?;
    Ok(())
}

fn get_index_deleted_files_in_filters(
    index: &Index,
    filters: &[PathBuf],
    target_blobs: &HashMap<PathBuf, ObjectHash>,
) -> io::Result<HashSet<PathBuf>> {
    let mut deleted = HashSet::new();
    for path_wd in target_blobs.keys() {
        let path_wd_str = path_to_utf8(path_wd)?;
        let path_abs = util::workdir_to_absolute(path_wd);
        if !index.tracked(path_wd_str, 0) && util::is_sub_of_paths(path_abs, filters) {
            deleted.insert(path_wd.clone());
        }
    }
    Ok(deleted)
}

fn get_index_deleted_files_in_filters_typed(
    index: &Index,
    filters: &[PathBuf],
    target_blobs: &HashMap<PathBuf, ObjectHash>,
) -> Result<HashSet<PathBuf>, RestoreError> {
    let mut deleted = HashSet::new();
    for path_wd in target_blobs.keys() {
        let path_wd_str = path_to_utf8_typed(path_wd)?;
        let path_abs = util::workdir_to_absolute(path_wd);
        if !index.tracked(path_wd_str, 0) && util::is_sub_of_paths(path_abs, filters) {
            deleted.insert(path_wd.clone());
        }
    }
    Ok(deleted)
}
