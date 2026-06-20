//! Implements `ls-files` to list files in the index with basic filters.

use std::{collections::HashSet, fs, path::PathBuf};

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
    libra ls-files --exclude-standard   Exclude files matching .gitignore
    libra ls-files -s                   Short output with stage info";

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

    /// Exclude files matching .gitignore patterns
    #[clap(long)]
    pub exclude_standard: bool,

    /// Short output format with mode and hash
    #[clap(short = 's')]
    pub short: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileEntry {
    path: String,
    hash: Option<String>,
    mode: Option<String>,
    stage: Option<u32>,
    status: String,
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
                let worktree_path = PathBuf::from(&entry.name);
                let exists = worktree_path.exists();
                let is_deleted = !exists;
                let is_modified = exists && entry_modified(&entry.name, &entry.hash.to_string())?;

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

    entries.sort_by(|a, b| a.path.cmp(&b.path).then(a.stage.cmp(&b.stage)));
    Ok(entries)
}

fn entry_modified(path: &str, indexed_hash: &str) -> CliResult<bool> {
    let data = match fs::read(path) {
        Ok(data) => data,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(source) => {
            return Err(CliError::fatal(format!(
                "failed to read working tree file '{path}': {source}"
            ))
            .with_stable_code(StableErrorCode::IoReadFailed));
        }
    };
    let blob = Blob::from_content_bytes(data);
    Ok(blob.id.to_string() != indexed_hash)
}

fn render_output(
    entries: &[FileEntry],
    args: &LsFilesArgs,
    output: &OutputConfig,
) -> CliResult<()> {
    if output.is_json() {
        return emit_json_data("ls-files", &entries.to_vec(), output);
    }
    if output.quiet {
        return Ok(());
    }

    for entry in entries {
        if args.short || args.stage {
            println!(
                "{} {} {}\t{}",
                entry.mode.as_deref().unwrap_or("000000"),
                entry
                    .hash
                    .as_deref()
                    .unwrap_or("0000000000000000000000000000000000000000"),
                entry.stage.unwrap_or(0),
                entry.path
            );
        } else {
            println!("{}", entry.path);
        }
    }

    Ok(())
}
