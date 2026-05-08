//! Implements `clean` to remove untracked files from the working tree.

use std::{
    fs,
    path::{Path, PathBuf},
};

use clap::Parser;
use git_internal::internal::index::Index;
use serde::Serialize;

use crate::utils::{
    error::{CliError, CliResult, StableErrorCode},
    ignore::{self, IgnorePolicy},
    output::{OutputConfig, emit_json_data},
    path, util, worktree,
};

const CLEAN_EXAMPLES: &str = "\
EXAMPLES:
  libra clean -n
  libra clean -f
  libra clean -fd
  libra clean -fx
  libra clean -fX
  libra clean -f --exclude '*.log'
  libra clean -n --json
";

#[derive(Parser, Debug, Clone)]
#[command(after_help = CLEAN_EXAMPLES)]
pub struct CleanArgs {
    /// Show what would be removed without actually removing
    #[clap(short = 'n', long)]
    pub dry_run: bool,
    /// Force removal of untracked files
    #[clap(short, long)]
    pub force: bool,
    /// Remove untracked directories in addition to untracked files
    #[clap(short = 'd', long = "dir")]
    pub directories: bool,
    /// Remove all untracked files, including those in .gitignore/.libraignore
    #[clap(short = 'x')]
    pub ignored: bool,
    /// Remove only untracked files that are in .gitignore/.libraignore
    #[clap(short = 'X')]
    pub only_ignored: bool,
    /// Exclude files matching the given pattern (can be repeated)
    #[clap(long = "exclude", value_name = "pattern")]
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct CleanOutput {
    dry_run: bool,
    removed: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
enum CleanError {
    #[error("clean requires -f or -n (use -f to remove files, -n to dry-run)")]
    MissingMode,
    #[error("invalid arguments: {0}")]
    InvalidArgs(String),
    #[error("failed to load index: {0}")]
    LoadIndex(String),
    #[error("{0}")]
    ScanUntracked(String),
    #[error("failed to resolve working directory: {0}")]
    ResolveWorkdir(String),
    #[error("failed to resolve path {path}: {detail}")]
    ResolvePath { path: String, detail: String },
    #[error("refusing to remove path outside workdir: {0}")]
    OutsideWorkdir(String),
    #[error("failed to remove {path}: {detail}")]
    RemoveFile { path: String, detail: String },
}

pub async fn execute(args: CleanArgs) {
    if let Err(err) = execute_safe(args, &OutputConfig::default()).await {
        err.print_stderr();
    }
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting.
///
/// # Side Effects
/// - Scans the working tree for untracked files.
/// - Removes matching files unless `--dry-run` is active.
/// - Renders removed or would-remove paths in human or JSON form.
///
/// # Errors
/// Returns [`CliError`] when the command is run outside a repository, candidate
/// paths cannot be resolved safely, a path escapes the worktree, or removal
/// fails.
pub async fn execute_safe(args: CleanArgs, output: &OutputConfig) -> CliResult<()> {
    util::require_repo().map_err(|_| CliError::repo_not_found())?;
    let clean_output = run_clean(args).map_err(clean_cli_error)?;

    if output.is_json() {
        emit_json_data("clean", &clean_output, output)?;
    } else if !output.quiet {
        for path in &clean_output.removed {
            if clean_output.dry_run {
                println!("Would remove {path}");
            } else {
                println!("Removing {path}");
            }
        }
    }

    Ok(())
}

fn run_clean(args: CleanArgs) -> Result<CleanOutput, CleanError> {
    if !args.force && !args.dry_run {
        return Err(CleanError::MissingMode);
    }

    // Validate mutually exclusive flags
    if args.ignored && args.only_ignored {
        return Err(CleanError::InvalidArgs(
            "cannot use -x and -X together".to_string(),
        ));
    }

    let index_path = path::index();
    let index = match Index::load(&index_path) {
        Ok(index) => index,
        Err(e) => {
            if !index_path.exists() {
                Index::new()
            } else {
                return Err(CleanError::LoadIndex(e.to_string()));
            }
        }
    };

    // Determine the ignore policy based on flags
    let policy = if args.only_ignored {
        IgnorePolicy::OnlyIgnored
    } else if args.ignored {
        IgnorePolicy::IncludeIgnored
    } else {
        IgnorePolicy::Respect
    };

    // Collect all workdir files and apply ignore policy
    let workdir_files =
        util::list_workdir_files().map_err(|e| CleanError::ScanUntracked(e.to_string()))?;
    let filtered_files = ignore::filter_workdir_paths(workdir_files, policy, &index);

    // Find untracked files
    let mut untracked: Vec<PathBuf> = Vec::new();
    for path in filtered_files {
        let path_str = path.to_str().ok_or_else(|| {
            CleanError::ScanUntracked(format!("path {:?} is not valid UTF-8", path))
        })?;
        if !worktree::index_has_any_stage(&index, path_str) {
            untracked.push(path);
        }
    }

    // If -d, also find untracked directories
    if args.directories {
        let untracked_dirs = find_untracked_dirs(&index, policy)?;
        for dir in untracked_dirs {
            // Skip the root directory (empty path)
            if dir.as_os_str().is_empty() {
                continue;
            }
            // Remove any files that are inside this directory from the untracked list
            // since the directory itself will be removed
            untracked.retain(|p| !p.starts_with(&dir));
            // Add the directory if it's not already covered by a parent directory
            if !untracked.iter().any(|p| dir.starts_with(p)) {
                untracked.push(dir);
            }
        }
    }

    // Apply --exclude patterns
    if !args.exclude.is_empty() {
        untracked.retain(|path| {
            let path_str = path.display().to_string();
            !args
                .exclude
                .iter()
                .any(|pattern| matches_exclude_pattern(&path_str, pattern))
        });
    }

    if untracked.is_empty() {
        return Ok(CleanOutput {
            dry_run: args.dry_run,
            removed: Vec::new(),
        });
    }

    if args.dry_run {
        return Ok(CleanOutput {
            dry_run: true,
            removed: untracked
                .iter()
                .map(|path| path.display().to_string())
                .collect(),
        });
    }

    let workdir = fs::canonicalize(util::working_dir())
        .map_err(|e| CleanError::ResolveWorkdir(e.to_string()))?;
    let mut removed = Vec::new();
    for path in untracked {
        let abs_path = util::workdir_to_absolute(&path);
        if abs_path.exists() {
            let resolved = fs::canonicalize(&abs_path).map_err(|e| CleanError::ResolvePath {
                path: abs_path.display().to_string(),
                detail: e.to_string(),
            })?;
            if !resolved.starts_with(&workdir) {
                return Err(CleanError::OutsideWorkdir(abs_path.display().to_string()));
            }
            if abs_path.is_dir() {
                fs::remove_dir_all(&abs_path).map_err(|e| CleanError::RemoveFile {
                    path: abs_path.display().to_string(),
                    detail: e.to_string(),
                })?;
            } else {
                fs::remove_file(&abs_path).map_err(|e| CleanError::RemoveFile {
                    path: abs_path.display().to_string(),
                    detail: e.to_string(),
                })?;
            }
            removed.push(path.display().to_string());
        }
    }
    Ok(CleanOutput {
        dry_run: false,
        removed,
    })
}

/// Find untracked directories based on the ignore policy.
/// A directory is considered untracked if it does not contain any tracked files.
fn find_untracked_dirs(index: &Index, policy: IgnorePolicy) -> Result<Vec<PathBuf>, CleanError> {
    let workdir = util::working_dir();
    let mut untracked_dirs = Vec::new();

    fn scan_dir(
        dir: &Path,
        workdir: &Path,
        index: &Index,
        policy: IgnorePolicy,
        untracked_dirs: &mut Vec<PathBuf>,
    ) -> Result<(), CleanError> {
        let entries = fs::read_dir(dir).map_err(|e| CleanError::ScanUntracked(e.to_string()))?;
        let mut has_tracked = false;
        let mut subdirs = Vec::new();

        for entry in entries {
            let entry = entry.map_err(|e| CleanError::ScanUntracked(e.to_string()))?;
            let path = entry.path();
            let relative = path
                .strip_prefix(workdir)
                .map_err(|e| CleanError::ScanUntracked(e.to_string()))?;

            if path.is_dir() {
                let name = path.file_name().unwrap_or_default();
                if name == ".git" || name == util::ROOT_DIR {
                    continue;
                }
                subdirs.push(path.clone());
            } else if let Some(path_str) = relative.to_str() {
                // Check if this file is tracked
                if index.tracked(path_str, 0) {
                    has_tracked = true;
                }
            }
        }

        if !has_tracked {
            // Check if this directory should be ignored
            let relative = dir
                .strip_prefix(workdir)
                .map_err(|e| CleanError::ScanUntracked(e.to_string()))?;
            let should_include = match policy {
                IgnorePolicy::Respect => {
                    // Only include if not ignored
                    !ignore::should_ignore(relative, policy, index)
                }
                IgnorePolicy::IncludeIgnored => true,
                IgnorePolicy::OnlyIgnored => {
                    // Only include if ignored
                    ignore::should_ignore(relative, IgnorePolicy::Respect, index)
                }
            };
            if should_include {
                untracked_dirs.push(relative.to_path_buf());
            }
        }

        // Recurse into subdirs
        for subdir in subdirs {
            scan_dir(&subdir, workdir, index, policy, untracked_dirs)?;
        }

        Ok(())
    }

    scan_dir(&workdir, &workdir, index, policy, &mut untracked_dirs)?;
    Ok(untracked_dirs)
}

/// Check if a path matches an exclude pattern using glob-style matching.
/// Supports * (match any characters) and ? (match single character).
fn matches_exclude_pattern(path: &str, pattern: &str) -> bool {
    // Escape special regex characters, then convert glob patterns
    let mut regex_pattern = String::new();
    regex_pattern.push('^');
    let chars = pattern.chars();
    for c in chars {
        match c {
            '*' => regex_pattern.push_str(".*"),
            '?' => regex_pattern.push('.'),
            '.' | '+' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '^' | '$' | '\\' => {
                regex_pattern.push('\\');
                regex_pattern.push(c);
            }
            _ => regex_pattern.push(c),
        }
    }
    regex_pattern.push('$');

    if let Ok(re) = regex::Regex::new(&regex_pattern) {
        re.is_match(path)
    } else {
        // Fallback to simple string matching
        path.contains(pattern)
    }
}

fn clean_cli_error(error: CleanError) -> CliError {
    match error {
        CleanError::MissingMode => CliError::fatal(error.to_string())
            .with_stable_code(StableErrorCode::CliInvalidArguments)
            .with_hint("use 'libra clean -n' to preview removals.")
            .with_hint("use 'libra clean -f' to remove untracked files."),
        CleanError::InvalidArgs(message) => {
            CliError::fatal(format!("invalid arguments: {message}"))
                .with_stable_code(StableErrorCode::CliInvalidArguments)
        }
        CleanError::LoadIndex(message) => {
            CliError::fatal(format!("failed to load index: {message}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        }
        CleanError::ScanUntracked(message) => {
            CliError::fatal(message).with_stable_code(StableErrorCode::IoReadFailed)
        }
        CleanError::ResolveWorkdir(message) => {
            CliError::fatal(format!("failed to resolve working directory: {message}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        }
        CleanError::ResolvePath { path, detail } => {
            CliError::fatal(format!("failed to resolve path {path}: {detail}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        }
        CleanError::OutsideWorkdir(path) => {
            CliError::fatal(format!("refusing to remove path outside workdir: {path}"))
                .with_stable_code(StableErrorCode::ConflictOperationBlocked)
        }
        CleanError::RemoveFile { path, detail } => {
            CliError::fatal(format!("failed to remove {path}: {detail}"))
                .with_stable_code(StableErrorCode::IoWriteFailed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CleanError, clean_cli_error};
    use crate::utils::error::StableErrorCode;

    #[test]
    fn resolve_workdir_cli_error_keeps_context() {
        let error = clean_cli_error(CleanError::ResolveWorkdir("permission denied".to_string()));

        assert_eq!(error.stable_code(), StableErrorCode::IoReadFailed);
        assert!(
            error
                .message()
                .contains("failed to resolve working directory"),
            "unexpected error message: {}",
            error.message()
        );
    }
}
