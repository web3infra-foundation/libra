//! Implements `ls-files` to list files in the index with basic filters.

use std::{
    collections::HashSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use clap::Parser;
use git_internal::internal::{index::Index, object::blob::Blob};
use serde::Serialize;

use crate::utils::{
    error::{CliError, CliResult, StableErrorCode},
    output::{OutputConfig, emit_json_data},
    path, util,
};

/// `--help` examples for ls-files
pub const LS_FILES_EXAMPLES: &str = "\
EXAMPLES:
    libra ls-files                      List all files in the index (cached)
    libra ls-files --cached             Show only files staged in the index
    libra ls-files --deleted            Show only deleted files
    libra ls-files --modified           Show only modified files
    libra ls-files --stage              Include stage information (for conflicts)
    libra ls-files --others             Show untracked files
    libra ls-files --exclude-standard   Exclude files matching .libraignore
    libra ls-files tracked-dir          Limit output to a pathspec
    libra ls-files --error-unmatch src  Fail if a pathspec matches nothing
    libra ls-files -z --others          Emit NUL-delimited records for scripts
    libra ls-files -s                   Short output with stage info
    libra ls-files -t                   Prefix each path with a status tag";

#[derive(Parser, Debug)]
#[command(after_help = LS_FILES_EXAMPLES)]
pub struct LsFilesArgs {
    /// Show only staged (cached) files in the index
    #[clap(long)]
    pub cached: bool,

    /// Show only deleted files
    #[clap(long)]
    pub deleted: bool,

    /// Show only modified files
    #[clap(long)]
    pub modified: bool,

    /// Include stage information for conflict resolution
    #[clap(long)]
    pub stage: bool,

    /// Show untracked files (not in index)
    #[clap(long)]
    pub others: bool,

    /// Exclude files matching .libraignore patterns
    #[clap(long)]
    pub exclude_standard: bool,

    /// Exit with an error when any pathspec matches no files
    #[clap(long)]
    pub error_unmatch: bool,

    /// Separate records with NUL instead of newline
    #[clap(short = 'z')]
    pub nul_terminate: bool,

    /// Short output format with mode and hash
    #[clap(short = 's')]
    pub short: bool,

    /// Prefix each path with a status tag (H=cached, R=removed/deleted,
    /// C=modified/changed, ?=other/untracked)
    #[clap(short = 't')]
    pub tag: bool,

    /// Limit output to files matching the given pathspec(s)
    #[clap(value_name = "pathspec")]
    pub pathspec: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileEntry {
    path: String,
    hash: Option<String>,
    mode: Option<String>,
    stage: Option<u32>,
    status: String,
}

#[derive(Debug, Clone)]
struct ResolvedPathspec {
    raw: String,
    absolute: PathBuf,
}

pub async fn execute(args: LsFilesArgs) -> CliResult<()> {
    let output = OutputConfig::default();
    let result = run_ls_files(&args)?;
    render_output(&result, &args, &output)?;
    Ok(())
}

pub async fn execute_safe(args: LsFilesArgs, output: &OutputConfig) -> CliResult<()> {
    let result = run_ls_files(&args)?;
    render_output(&result, &args, output)?;
    Ok(())
}

fn run_ls_files(_args: &LsFilesArgs) -> CliResult<Vec<FileEntry>> {
    util::require_repo().map_err(|_| CliError::repo_not_found())?;
    let workdir = util::working_dir();
    let current_dir = util::cur_dir();
    let pathspecs = resolve_ls_files_pathspecs(_args, &workdir, &current_dir)?;

    let index = Index::load(path::index()).map_err(|source| {
        CliError::fatal(format!("failed to load index: {source}"))
            .with_stable_code(StableErrorCode::RepoCorrupt)
    })?;

    let mut entries = Vec::new();
    let include_cached = _args.cached || (!_args.deleted && !_args.modified && !_args.others);

    if include_cached || _args.deleted || _args.modified || _args.stage || _args.short {
        let stages: &[u8] = if _args.stage || _args.short {
            &[0, 1, 2, 3]
        } else {
            &[0]
        };
        for stage in stages {
            for entry in index.tracked_entries(*stage) {
                let worktree_path = workdir.join(&entry.name);
                let exists = worktree_path.exists();
                let is_deleted = !exists;
                let is_modified =
                    exists && entry_modified(&worktree_path, &entry.name, &entry.hash.to_string())?;

                if _args.deleted && !is_deleted {
                    continue;
                }
                if _args.modified && !is_modified {
                    continue;
                }

                let status = if is_deleted {
                    "deleted"
                } else if is_modified {
                    "modified"
                } else {
                    "cached"
                };
                entries.push(FileEntry {
                    path: entry.name.clone(),
                    hash: Some(entry.hash.to_string()),
                    mode: Some(format!("{:06o}", entry.mode)),
                    stage: Some(*stage as u32),
                    status: status.to_string(),
                });
            }
        }
    }

    if _args.others {
        let tracked: HashSet<String> = index
            .tracked_entries(0)
            .into_iter()
            .map(|entry| entry.name.clone())
            .collect();
        let files = if _args.exclude_standard {
            util::list_workdir_files()
        } else {
            util::list_workdir_files_unfiltered()
        }
        .map_err(|source| {
            CliError::fatal(format!("failed to list working tree files: {source}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        })?;

        for file in files {
            let display = file.to_string_lossy().replace('\\', "/");
            if tracked.contains(&display) {
                continue;
            }
            entries.push(FileEntry {
                path: display,
                hash: None,
                mode: None,
                stage: None,
                status: "other".to_string(),
            });
        }
    }

    entries = filter_entries_by_pathspec(entries, &pathspecs, &workdir);
    if _args.error_unmatch {
        ensure_error_unmatch(&pathspecs, &entries, &workdir)?;
    }

    entries.sort_by(|a, b| a.path.cmp(&b.path).then(a.stage.cmp(&b.stage)));
    Ok(entries)
}

fn resolve_ls_files_pathspecs(
    args: &LsFilesArgs,
    workdir: &Path,
    current_dir: &Path,
) -> CliResult<Vec<ResolvedPathspec>> {
    args.pathspec
        .iter()
        .map(|raw| {
            let absolute = resolve_pathspec(Path::new(raw), current_dir);
            if !util::is_sub_path(&absolute, workdir) {
                return Err(CliError::fatal(format!(
                    "'{raw}' is outside repository at '{}'",
                    workdir.display()
                ))
                .with_stable_code(StableErrorCode::CliInvalidTarget)
                .with_hint("all paths must be within the repository working tree"));
            }
            Ok(ResolvedPathspec {
                raw: raw.clone(),
                absolute,
            })
        })
        .collect()
}

fn resolve_pathspec(pathspec: &Path, current_dir: &Path) -> PathBuf {
    if pathspec.is_absolute() {
        pathspec.to_path_buf()
    } else {
        current_dir.join(pathspec)
    }
}

fn filter_entries_by_pathspec(
    entries: Vec<FileEntry>,
    pathspecs: &[ResolvedPathspec],
    workdir: &Path,
) -> Vec<FileEntry> {
    if pathspecs.is_empty() {
        return entries;
    }

    entries
        .into_iter()
        .filter(|entry| {
            pathspecs
                .iter()
                .any(|pathspec| entry_matches_pathspec(entry, pathspec, workdir))
        })
        .collect()
}

fn ensure_error_unmatch(
    pathspecs: &[ResolvedPathspec],
    entries: &[FileEntry],
    workdir: &Path,
) -> CliResult<()> {
    if let Some(unmatched) = pathspecs.iter().find(|pathspec| {
        !entries
            .iter()
            .any(|entry| entry_matches_pathspec(entry, pathspec, workdir))
    }) {
        return Err(CliError::fatal(format!(
            "pathspec '{}' did not match any files",
            unmatched.raw
        ))
        .with_stable_code(StableErrorCode::CliInvalidTarget)
        .with_hint("check the path and try again.")
        .with_hint("use 'libra ls-files' to inspect visible paths."));
    }

    Ok(())
}

fn entry_matches_pathspec(entry: &FileEntry, pathspec: &ResolvedPathspec, workdir: &Path) -> bool {
    let entry_abs = workdir.join(Path::new(&entry.path));
    util::is_sub_path(&entry_abs, &pathspec.absolute)
}

fn entry_modified(worktree_path: &Path, display_path: &str, indexed_hash: &str) -> CliResult<bool> {
    let data = match fs::read(worktree_path) {
        Ok(data) => data,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(source) => {
            return Err(CliError::fatal(format!(
                "failed to read working tree file '{display_path}': {source}"
            ))
            .with_stable_code(StableErrorCode::IoReadFailed));
        }
    };
    let blob = Blob::from_content_bytes(data);
    Ok(blob.id.to_string() != indexed_hash)
}

/// Map an entry's `status` to its `git ls-files -t` tag letter.
fn status_tag(status: &str) -> char {
    match status {
        "deleted" => 'R',
        "modified" => 'C',
        "other" => '?',
        "unmerged" => 'M',
        // "cached" and anything else default to H (in the index).
        _ => 'H',
    }
}

fn render_output(
    entries: &[FileEntry],
    args: &LsFilesArgs,
    output: &OutputConfig,
) -> CliResult<()> {
    if args.nul_terminate && output.is_json() {
        return Err(
            CliError::fatal("ls-files -z cannot be combined with --json or --machine")
                .with_stable_code(StableErrorCode::CliInvalidArguments)
                .with_hint("choose either NUL-delimited text output or JSON/machine output"),
        );
    }
    if output.is_json() {
        return emit_json_data("ls-files", &entries.to_vec(), output);
    }
    if output.quiet {
        return Ok(());
    }

    let mut stdout = std::io::stdout().lock();
    for entry in entries {
        let mut record = if args.short || args.stage {
            format!(
                "{} {} {}\t{}",
                entry.mode.as_deref().unwrap_or("000000"),
                entry
                    .hash
                    .as_deref()
                    .unwrap_or("0000000000000000000000000000000000000000"),
                entry.stage.unwrap_or(0),
                entry.path
            )
        } else {
            entry.path.clone()
        };
        // `-t` prefixes a status tag (matching `git ls-files -t`).
        if args.tag {
            record = format!("{} {}", status_tag(&entry.status), record);
        }

        stdout.write_all(record.as_bytes()).map_err(|source| {
            CliError::fatal(format!("failed to write ls-files output: {source}"))
                .with_stable_code(StableErrorCode::IoWriteFailed)
        })?;
        stdout
            .write_all(if args.nul_terminate { b"\0" } else { b"\n" })
            .map_err(|source| {
                CliError::fatal(format!("failed to write ls-files output: {source}"))
                    .with_stable_code(StableErrorCode::IoWriteFailed)
            })?;
    }

    Ok(())
}
