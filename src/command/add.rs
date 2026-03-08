//! Stages changes for commit by parsing pathspecs and modes, respecting ignore
//! policy, refreshing index entries, and writing blob objects.

use std::{
    env, io,
    path::{Path, PathBuf},
};

use clap::Parser;
use git_internal::{
    errors::GitError,
    internal::{
        index::{Index, IndexEntry},
        object::blob::Blob,
    },
};

use crate::{
    command::status::{self, Changes},
    utils::{
        error::{CliError, CliResult},
        lfs,
        object_ext::BlobExt,
        path, util,
    },
};

#[derive(Parser, Debug)]
pub struct AddArgs {
    /// pathspec... files & dir to add content from.
    #[clap(required = false)]
    pub pathspec: Vec<String>,

    /// Update the index not only where the working tree has a file matching pathspec but also where the index already has an entry. This adds, modifies, and removes index entries to match the working tree.
    ///
    /// If no pathspec is given when -A option is used, all files in the entire working tree are updated
    #[clap(short = 'A', long, group = "mode")]
    pub all: bool,

    /// Update the index just where it already has an entry matching **pathspec**.
    /// This removes as well as modifies index entries to match the working tree, but adds no new files.
    #[clap(short, long, group = "mode")]
    pub update: bool,

    /// Refresh index entries for all files currently in the index.
    ///
    /// This updates only the metadata (e.g. file stat information such as
    /// timestamps, file size, etc.) of existing index entries to match
    /// the working tree, without adding new files or removing entries.
    #[clap(long, group = "mode")]
    pub refresh: bool,

    /// more detailed output
    #[clap(short, long)]
    pub verbose: bool,

    /// allow adding otherwise ignored files
    #[clap(short = 'f', long)]
    pub force: bool,

    /// dry run
    #[clap(short, long)]
    pub dry_run: bool,

    /// ignore errors
    #[clap(long)]
    pub ignore_errors: bool,
}

#[derive(thiserror::Error, Debug)]
pub enum AddError {
    #[error("not a libra repository (or any of the parent directories): .libra")]
    NotInRepo,
    #[error("pathspec '{pathspec}' did not match any files")]
    PathspecNotMatched { pathspec: String },
    #[error("'{path}' is outside repository at '{repo_root}'")]
    PathOutsideRepo { path: String, repo_root: PathBuf },
    #[error("unable to read index '{path}': {source}")]
    IndexLoad { path: PathBuf, source: GitError },
    #[error("unable to write index '{path}': {source}")]
    IndexSave { path: PathBuf, source: GitError },
    #[error("failed to refresh '{path}': {source}")]
    RefreshFailed { path: PathBuf, source: GitError },
    #[error("failed to create index entry for '{path}': {source}")]
    CreateIndexEntry { path: PathBuf, source: io::Error },
    #[error("path '{path}' is not valid UTF-8")]
    InvalidPathEncoding { path: PathBuf },
    #[error("failed to determine repository working directory: {source}")]
    Workdir { source: io::Error },
    #[error("failed to inspect repository status: {source}")]
    Status { source: status::StatusError },
}

impl From<AddError> for CliError {
    fn from(error: AddError) -> Self {
        match &error {
            AddError::PathspecNotMatched { .. } => CliError::fatal(error.to_string())
                .with_hint("check the path and try again.")
                .with_hint("use 'libra status' to inspect tracked and untracked files."),
            _ => CliError::fatal(error.to_string()),
        }
    }
}

#[derive(Default)]
struct ValidatedPathspecs {
    files: Vec<PathBuf>,
    ignored: Vec<String>,
}

pub async fn execute(args: AddArgs) {
    if let Err(err) = execute_safe(args).await {
        eprintln!("{}", err.render());
    }
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting. Stages changes by resolving pathspecs, respecting
/// ignore policy, and writing blob objects to storage.
pub async fn execute_safe(args: AddArgs) -> CliResult<()> {
    let workdir = util::try_working_dir().map_err(|source| {
        if source.kind() == io::ErrorKind::NotFound {
            AddError::NotInRepo
        } else {
            AddError::Workdir { source }
        }
    })?;
    let index_path = path::try_index().map_err(|source| {
        if source.kind() == io::ErrorKind::NotFound {
            AddError::NotInRepo
        } else {
            AddError::Workdir { source }
        }
    })?;
    let storage_path = util::try_get_storage_path(None).map_err(|source| {
        if source.kind() == io::ErrorKind::NotFound {
            AddError::NotInRepo
        } else {
            AddError::Workdir { source }
        }
    })?;

    // `String` to `PathBuf`
    let requested_paths: Vec<PathBuf> = if args.pathspec.is_empty() {
        if !args.all && !args.update && !args.refresh {
            eprintln!("Nothing specified, nothing added.");
            eprintln!("hint: maybe you wanted to say 'libra add .'?");
            return Ok(());
        }
        vec![workdir.clone()]
    } else {
        args.pathspec.iter().map(PathBuf::from).collect()
    };

    let mut index = Index::load(&index_path).map_err(|source| AddError::IndexLoad {
        path: index_path.clone(),
        source,
    })?;
    let current_dir = env::current_dir().map_err(|source| AddError::Workdir { source })?;

    let (mut visible_changes, ignored_changes) =
        status::changes_to_be_staged_split_safe().map_err(|source| AddError::Status { source })?;
    if args.force {
        visible_changes.extend(ignored_changes.clone());
    }
    let ignored_changes = if args.force {
        Changes::default()
    } else {
        ignored_changes
    };

    let validated = validate_pathspecs(
        &args.pathspec,
        &requested_paths,
        &workdir,
        &current_dir,
        &visible_changes,
        &ignored_changes,
        &index,
    )?;

    if args.refresh {
        let tracked_modified = filter_refresh_candidates(
            &visible_changes.modified,
            &validated.files,
            &workdir,
            &current_dir,
        );
        if args.dry_run {
            for file in &tracked_modified {
                println!("refresh: {}", file.display());
            }
        } else {
            refresh_files(&mut index, &tracked_modified, &workdir, args.verbose)?;
            index
                .save(&index_path)
                .map_err(|source| AddError::IndexSave {
                    path: index_path.clone(),
                    source,
                })?;
        }

        return finish_ignored(validated.ignored);
    }

    let mut files = visible_changes.modified;
    files.extend(visible_changes.deleted);
    if !args.update {
        files.extend(visible_changes.new);
    }
    files = filter_candidates(&files, &validated.files, &workdir, &current_dir);
    filter_out_current_executable(&mut files);
    files.sort();
    files.dedup();

    if args.dry_run {
        for file in &files {
            println!("add: {}", file.display());
        }
        return finish_ignored(validated.ignored);
    }

    let mut per_file_failures = Vec::new();
    for file in &files {
        if let Err(err) = add_a_file(file, &mut index, &workdir, &storage_path, args.verbose).await
        {
            if !args.ignore_errors {
                return Err(CliError::from(err));
            }
            per_file_failures.push(err.to_string());
        }
    }

    index
        .save(&index_path)
        .map_err(|source| AddError::IndexSave {
            path: index_path.clone(),
            source,
        })?;

    if !per_file_failures.is_empty() {
        return Err(CliError::failure(per_file_failures.join("\n")));
    }

    finish_ignored(validated.ignored)
}

fn finish_ignored(ignored: Vec<String>) -> CliResult<()> {
    if ignored.is_empty() {
        return Ok(());
    }

    let mut sorted = ignored;
    sorted.sort();
    sorted.dedup();
    let mut message =
        String::from("the following paths are ignored by one of your .libraignore files:");
    for path in sorted {
        message.push('\n');
        message.push_str(&path);
    }

    Err(CliError::failure(message).with_hint("use -f if you really want to add them."))
}

fn validate_pathspecs(
    raw_pathspecs: &[String],
    requested_paths: &[PathBuf],
    workdir: &Path,
    current_dir: &Path,
    visible_changes: &Changes,
    ignored_changes: &Changes,
    index: &Index,
) -> Result<ValidatedPathspecs, AddError> {
    if raw_pathspecs.is_empty() {
        return Ok(ValidatedPathspecs {
            files: requested_paths.to_vec(),
            ignored: Vec::new(),
        });
    }

    let tracked_files = index.tracked_files();
    let change_candidates = collect_change_candidates(visible_changes);
    let ignored_candidates = collect_change_candidates(ignored_changes);

    let mut ignored = Vec::new();
    let mut files = Vec::new();
    for (raw, requested_path) in raw_pathspecs.iter().zip(requested_paths.iter()) {
        let requested_abs = resolve_pathspec(requested_path, current_dir);
        if !util::is_sub_path(&requested_abs, workdir) {
            return Err(AddError::PathOutsideRepo {
                path: raw.clone(),
                repo_root: workdir.to_path_buf(),
            });
        }

        let matches_changes = pathspec_matches_any(&requested_abs, &change_candidates, workdir);
        let matches_tracked = pathspec_matches_any(&requested_abs, &tracked_files, workdir);
        let matches_ignored = pathspec_matches_any(&requested_abs, &ignored_candidates, workdir);

        if matches_changes || matches_tracked {
            files.push(requested_path.clone());
            continue;
        }
        if matches_ignored {
            ignored.push(raw.clone());
            continue;
        }

        return Err(AddError::PathspecNotMatched {
            pathspec: raw.clone(),
        });
    }

    Ok(ValidatedPathspecs { files, ignored })
}

fn collect_change_candidates(changes: &Changes) -> Vec<PathBuf> {
    let mut files = Vec::new();
    files.extend(changes.new.iter().cloned());
    files.extend(changes.modified.iter().cloned());
    files.extend(changes.deleted.iter().cloned());
    files
}

fn resolve_pathspec(pathspec: &Path, current_dir: &Path) -> PathBuf {
    if pathspec.is_absolute() {
        pathspec.to_path_buf()
    } else {
        current_dir.join(pathspec)
    }
}

fn pathspec_matches_any(requested_abs: &Path, candidates: &[PathBuf], workdir: &Path) -> bool {
    candidates.iter().any(|candidate| {
        let candidate_abs = workdir.join(candidate);
        util::is_sub_path(&candidate_abs, requested_abs)
    })
}

fn filter_candidates(
    files: &[PathBuf],
    requested_paths: &[PathBuf],
    workdir: &Path,
    current_dir: &Path,
) -> Vec<PathBuf> {
    files
        .iter()
        .filter(|file| {
            let file_abs = workdir.join(file.as_path());
            requested_paths.iter().any(|pathspec| {
                let requested_abs = resolve_pathspec(pathspec, current_dir);
                util::is_sub_path(&file_abs, &requested_abs)
            })
        })
        .cloned()
        .collect()
}

fn filter_refresh_candidates(
    files: &[PathBuf],
    requested_paths: &[PathBuf],
    workdir: &Path,
    current_dir: &Path,
) -> Vec<PathBuf> {
    filter_candidates(files, requested_paths, workdir, current_dir)
}

fn filter_out_current_executable(files: &mut Vec<PathBuf>) {
    if let Some(exe_path) = std::env::current_exe()
        .ok()
        .and_then(|path| path.canonicalize().ok())
    {
        files.retain(|file| {
            util::try_workdir_to_absolute(file)
                .ok()
                .and_then(|path| path.canonicalize().ok())
                .is_none_or(|abs| abs != exe_path)
        });
    }
}

fn refresh_files(
    index: &mut Index,
    files: &[PathBuf],
    workdir: &Path,
    verbose: bool,
) -> Result<(), AddError> {
    for file in files {
        if index
            .refresh(file, workdir)
            .map_err(|source| AddError::RefreshFailed {
                path: file.clone(),
                source,
            })?
            && verbose
        {
            println!("refreshed: {}", file.display());
        }
    }
    Ok(())
}

/// `file` path must relative to the working directory
async fn add_a_file(
    file: &Path,
    index: &mut Index,
    workdir: &Path,
    storage_path: &Path,
    verbose: bool,
) -> Result<(), AddError> {
    let file_abs = workdir.join(file);
    if !util::is_sub_path(&file_abs, workdir) {
        return Err(AddError::PathOutsideRepo {
            path: file.display().to_string(),
            repo_root: workdir.to_path_buf(),
        });
    }
    if util::is_sub_path(&file_abs, storage_path) {
        return Ok(());
    }

    let file_str = file.to_str().ok_or_else(|| AddError::InvalidPathEncoding {
        path: file.to_path_buf(),
    })?;
    let file_status = check_file_status(file, index, workdir)?;
    match file_status {
        FileStatus::New => {
            let blob = gen_blob_from_file(&file_abs);
            blob.save();
            index.add(
                IndexEntry::new_from_file(file, blob.id, workdir).map_err(|source| {
                    AddError::CreateIndexEntry {
                        path: file.to_path_buf(),
                        source,
                    }
                })?,
            );
            if verbose {
                println!("add(new): {}", file.display());
            }
        }
        FileStatus::Modified => {
            if index.is_modified(file_str, 0, workdir) {
                let blob = gen_blob_from_file(&file_abs);
                if !index.verify_hash(file_str, 0, &blob.id) {
                    blob.save();
                    index.update(IndexEntry::new_from_file(file, blob.id, workdir).map_err(
                        |source| AddError::CreateIndexEntry {
                            path: file.to_path_buf(),
                            source,
                        },
                    )?);
                    if verbose {
                        println!("add(modified): {}", file.display());
                    }
                }
            }
        }
        FileStatus::Deleted => {
            index.remove(file_str, 0);
            if verbose {
                println!("removed: {file_str}");
            }
        }
        FileStatus::Unchanged => {}
        FileStatus::NotFound => {
            return Err(AddError::PathspecNotMatched {
                pathspec: file.display().to_string(),
            });
        }
    }
    Ok(())
}

enum FileStatus {
    /// file is new
    New,
    /// file is modified
    Modified,
    /// file is deleted
    Deleted,
    /// file exists or is tracked but has nothing to stage
    Unchanged,
    /// file is not tracked
    NotFound,
}

fn check_file_status(file: &Path, index: &Index, workdir: &Path) -> Result<FileStatus, AddError> {
    let file_str = file.to_str().ok_or_else(|| AddError::InvalidPathEncoding {
        path: file.to_path_buf(),
    })?;
    let file_abs = workdir.join(file);
    if !file_abs.exists() {
        if index.tracked(file_str, 0) {
            Ok(FileStatus::Deleted)
        } else {
            Ok(FileStatus::NotFound)
        }
    } else if !index.tracked(file_str, 0) {
        Ok(FileStatus::New)
    } else if index.is_modified(file_str, 0, workdir) {
        Ok(FileStatus::Modified)
    } else {
        Ok(FileStatus::Unchanged)
    }
}

/// Generate a `Blob` from a file
/// - if the file is tracked by LFS, generate a `Blob` with pointer file
fn gen_blob_from_file(path: impl AsRef<Path>) -> Blob {
    if lfs::is_lfs_tracked(&path) {
        Blob::from_lfs_file(&path)
    } else {
        Blob::from_file(&path)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_args_conflict_with_refresh() {
        // "--refresh" cannot be combined with "-A", "--refresh" or "-u"
        assert!(AddArgs::try_parse_from(["test", "-A", "--refresh"]).is_err());
        assert!(AddArgs::try_parse_from(["test", "-u", "--refresh"]).is_err());
        assert!(AddArgs::try_parse_from(["test", "-A", "-u", "--refresh"]).is_err());
    }

    #[test]
    fn ignored_rendered_as_failure_with_hint() {
        let err = finish_ignored(vec!["ignored.txt".to_string()]).unwrap_err();
        assert!(err.render().contains("ignored.txt"));
        assert!(
            err.render()
                .contains("Hint: use -f if you really want to add them.")
        );
    }
}
