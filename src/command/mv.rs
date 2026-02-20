//! Implementation of `git mv` command, which moves/renames files and directories in the working directory and updates the index accordingly.
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use clap::Parser;
use git_internal::internal::index::{Index, IndexEntry};

use crate::{
    command::calc_file_blob_hash,
    utils::{path, util},
};

#[derive(Parser, Debug)]
pub struct MvArgs {
    /// Path list: one or more <source> followed by <destination>
    /// The <destination> is required and must be the last argument. It can be either a file or a directory.
    /// If there are multiple <source>, the <destination> must be an existing directory.
    pub paths: Vec<String>,

    /// Enable verbose output.
    #[clap(short = 'v', long)]
    pub verbose: bool,

    /// Perform a dry run.
    #[clap(short = 'n', long)]
    pub dry_run: bool,

    /// Force move/rename even if the destination already exists (overwriting it)
    #[clap(short = 'f', long)]
    pub force: bool,
}

#[derive(Default)]
struct MovePlan {
    fs_moves: Vec<(PathBuf, PathBuf)>,
    index_updates: Vec<(PathBuf, PathBuf)>,
}

#[derive(Default)]
struct RollbackReport {
    restored_pairs: Vec<(PathBuf, PathBuf)>,
    failed_pairs: Vec<(PathBuf, PathBuf, String)>,
}

impl MovePlan {
    fn extend(&mut self, mut other: MovePlan) {
        self.fs_moves.append(&mut other.fs_moves);
        self.index_updates.append(&mut other.index_updates);
    }
}

pub async fn execute(args: MvArgs) -> bool {
    if !util::check_repo_exist() {
        return false;
    }
    // If the user just types `git mv` without enough arguments, print usage information instead of an error message.
    if args.paths.len() < 2 {
        eprintln!("usage: libra mv [<options>] <source>... <destination>");
        eprintln!();
        eprintln!("-v, --verbose    be verbose");
        eprintln!("-n, --dry-run    dry run");
        eprintln!("-f, --force      force move/rename even if target exists");
        return false;
    }

    let paths: Vec<PathBuf> = args.paths.iter().map(PathBuf::from).collect();
    let sources: Vec<PathBuf> = paths[0..paths.len() - 1]
        .iter()
        .map(to_absolute_path)
        .collect();
    let destination = to_absolute_path(&paths[paths.len() - 1]);

    for src in &sources {
        if let Err(err) = validate_path_within_workdir(src) {
            eprintln!("{err}");
            return false;
        }
    }
    if let Err(err) = validate_path_within_workdir(&destination) {
        eprintln!("{err}");
        return false;
    }

    // Check if the destination is a directory (if it exists), which affects how we handle multiple sources.
    let destination_is_dir = destination.is_dir();
    // If there are multiple sources, the destination must be an existing directory.
    if sources.len() > 1 && !destination_is_dir {
        eprintln!(
            "fatal: destination '{}' is not a directory",
            util::to_workdir_path(&destination).display()
        );
        return false;
    }

    // Check the validity of all sources and collect the valid move operations.
    let mut move_plan = MovePlan::default();
    let index_file = path::index();
    let mut index = match Index::load(&index_file) {
        Ok(index) => index,
        Err(err) => {
            eprintln!("fatal: failed to load index: {err}");
            return false;
        }
    };
    for src in &sources {
        match validate_source_and_collect_moves(
            src,
            &destination,
            destination_is_dir,
            &index,
            args.force,
        ) {
            Ok(plan) => move_plan.extend(plan),
            Err(err) => {
                eprintln!("{err}");
                return false;
            }
        }
    }

    if has_duplicate_target(&move_plan.fs_moves) {
        eprintln!(
            "fatal: multiple sources moving to the same target path, source={}, destination={}",
            util::to_workdir_path(&sources[sources.len() - 1]).display(),
            util::to_workdir_path(&destination).display()
        );
        return false;
    }
    perform_moves(
        move_plan,
        args.verbose,
        args.dry_run,
        args.force,
        &mut index,
    )
}
/// Validates a source path and builds the move plan.
///
/// Returns:
/// - `Ok(MovePlan)`: a move plan with move pairs.
///   - `fs_moves`: filesystem move pairs `(src_abs, dst_abs)`.
///   - `index_updates`: index update pairs `(src_abs, dst_abs)`.
///   - Both source and destination paths in pairs are absolute paths.
/// - `Err(String)`: a formatted fatal error message for invalid input or unsupported move.
fn validate_source_and_collect_moves(
    src: &Path,
    destination: &Path,
    destination_is_dir: bool,
    index: &Index,
    force: bool,
) -> Result<MovePlan, String> {
    if !src.exists() {
        return Err(format!(
            "fatal: bad source, source={}, destination={}",
            util::to_workdir_path(src).display(),
            util::to_workdir_path(destination).display()
        ));
    }

    if src == destination {
        return Err(format!(
            "fatal: can not move directory into itself, source={}, destination={}",
            util::to_workdir_path(src).display(),
            util::to_workdir_path(destination).display()
        ));
    }

    if src.is_dir() {
        return validate_source_directory(src, destination, destination_is_dir, index);
    }

    validate_source_file(src, destination, destination_is_dir, index, force)
}
/// Validates a source directory and builds the directory move plan.
///
/// Returns:
/// - `Ok(MovePlan)`: directory move plan where each pair is `(src_abs, dst_abs)`.
/// - `Err(String)`: a formatted fatal error message.
fn validate_source_directory(
    src: &Path,
    destination: &Path,
    destination_is_dir: bool,
    index: &Index,
) -> Result<MovePlan, String> {
    // For directory move, we require the destination to be an existing directory
    if !destination_is_dir {
        return Err(format!(
            "fatal: destination '{}' is not a directory",
            util::to_workdir_path(destination).display()
        ));
    }

    let src_name = src.file_name().ok_or_else(|| {
        format!(
            "fatal: bad source, source={}, destination={}",
            util::to_workdir_path(src).display(),
            util::to_workdir_path(destination).display()
        )
    })?;

    if destination.starts_with(src) {
        return Err(format!(
            "fatal: can not move directory into itself, source={}, destination={}",
            util::to_workdir_path(src).display(),
            util::to_workdir_path(destination).display()
        ));
    }

    if destination.join(src_name).exists() {
        return Err(format!(
            "fatal: destination already exists, source={}, destination={}",
            util::to_workdir_path(src).display(),
            util::to_workdir_path(destination).display()
        ));
    }

    resolve_move_directory(src, destination, index)
}
/// Validates a source file and builds the file move plan.
///
/// Returns:
/// - `Ok(MovePlan)`: file move plan where each pair is `(src_abs, dst_abs)`.
/// - `Err(String)`: a formatted fatal error message.
fn validate_source_file(
    src: &Path,
    destination: &Path,
    destination_is_dir: bool,
    index: &Index,
    force: bool,
) -> Result<MovePlan, String> {
    if !index.tracked(&util::path_to_string(&util::to_workdir_path(src)), 0) {
        return Err(format!(
            "fatal: not under version control, source={}, destination={}",
            util::to_workdir_path(src).display(),
            util::to_workdir_path(destination).display()
        ));
    }

    if is_conflicted_in_index(index, src) {
        return Err(format!(
            "fatal: conflicted, source={}, destination={}",
            util::to_workdir_path(src).display(),
            util::to_workdir_path(destination).display()
        ));
    }

    let target = if destination_is_dir {
        let src_name = src.file_name().ok_or_else(|| {
            format!(
                "fatal: bad source, source={}, destination={}",
                util::to_workdir_path(src).display(),
                util::to_workdir_path(destination).display()
            )
        })?;
        destination.join(src_name)
    } else {
        destination.to_path_buf()
    };

    if let Ok(meta) = std::fs::symlink_metadata(&target) {
        if !force {
            return Err(format!(
                "fatal: destination already exists, source={}, destination={}",
                util::to_workdir_path(src).display(),
                util::to_workdir_path(&target).display()
            ));
        }
        let file_type = meta.file_type();
        if !(file_type.is_file() || file_type.is_symlink()) {
            return Err(format!(
                "fatal: cannot overwrite, source={}, destination={}",
                util::to_workdir_path(src).display(),
                util::to_workdir_path(&target).display()
            ));
        }
    }

    Ok(MovePlan {
        fs_moves: vec![(src.to_path_buf(), target.clone())],
        index_updates: vec![(src.to_path_buf(), target)],
    })
}

fn to_absolute_path(path: impl AsRef<Path>) -> PathBuf {
    util::workdir_to_absolute(path.as_ref())
}

fn validate_path_within_workdir(path: &Path) -> Result<(), String> {
    let workdir = util::working_dir();
    if !util::is_sub_path(path, &workdir) {
        return Err(format!(
            "fatal: '{}' is outside of the respository at '{}'",
            path.display(),
            workdir.display()
        ));
    }
    Ok(())
}

/// Builds a move plan for a directory source.
/// - Moves the whole directory in the filesystem (tracked + untracked + empty dirs).
/// - Updates the index only for tracked files under the source directory.
/// - Untracked files are moved with the directory rename and are not added to the index.
///
/// Returns:
/// - `Ok(MovePlan)`: move plan with absolute-path pairs `(src_abs, dst_abs)`.
/// - `Err(String)`: a formatted fatal error message.
fn resolve_move_directory(src: &Path, dst: &Path, index: &Index) -> Result<MovePlan, String> {
    let src_name = src.file_name().ok_or_else(|| {
        format!(
            "fatal: bad source, source={}, destination={}",
            util::to_workdir_path(src).display(),
            util::to_workdir_path(dst).display()
        )
    })?;
    let target_dir = dst.join(src_name);

    let files = util::list_files(src).map_err(|err| {
        format!(
            "fatal: failed to list source directory, source={}, destination={}, error={}",
            util::to_workdir_path(src).display(),
            util::to_workdir_path(dst).display(),
            err
        )
    })?;

    let tracked_updates: Vec<(PathBuf, PathBuf)> = files
        .into_iter()
        .filter(|file| index.tracked(&util::path_to_string(file), 0))
        .map(|file| {
            let relative_path = util::to_relative(&file, src);
            (
                util::workdir_to_absolute(&file),
                util::workdir_to_absolute(target_dir.join(relative_path)),
            )
        })
        .collect();

    Ok(MovePlan {
        fs_moves: vec![(src.to_path_buf(), target_dir)],
        index_updates: tracked_updates,
    })
}

/// Checks whether the source file is conflicted in the index.
fn is_conflicted_in_index(index: &Index, src: &Path) -> bool {
    let src_str = util::path_to_string(&util::to_workdir_path(src));
    (1..=3).any(|stage| index.tracked(&src_str, stage))
}
/// Checks whether multiple move operations target the same destination path.
fn has_duplicate_target(moves: &[(PathBuf, PathBuf)]) -> bool {
    let mut target_paths = HashSet::new();
    for (_, target) in moves {
        if !target_paths.insert(target.clone()) {
            return true;
        }
    }
    false
}

fn remove_index_entry_all_stages(index: &mut Index, path: &str) {
    for stage in 0..=3 {
        let _ = index.remove(path, stage);
    }
}

/// Rolls back performed moves in reverse order and reports rollback outcomes.
fn rollback_moves(moved_pairs: &[(PathBuf, PathBuf)]) -> RollbackReport {
    let mut report = RollbackReport::default();
    for (src, dst) in moved_pairs.iter().rev() {
        if let Err(err) = std::fs::rename(dst, src) {
            report
                .failed_pairs
                .push((src.clone(), dst.clone(), err.to_string()));
        } else {
            report.restored_pairs.push((src.clone(), dst.clone()));
        }
    }
    report
}

fn print_rollback_report(report: RollbackReport) {
    if report.restored_pairs.is_empty() && report.failed_pairs.is_empty() {
        return;
    }

    if !report.restored_pairs.is_empty() {
        eprintln!(
            "rollback: restored {} moved path(s)",
            report.restored_pairs.len()
        );
        for (src, dst) in &report.restored_pairs {
            eprintln!(
                "rollback: restored source={} from destination={}",
                util::to_workdir_path(src).display(),
                util::to_workdir_path(dst).display()
            );
        }
    }

    if !report.failed_pairs.is_empty() {
        eprintln!(
            "fatal: rollback incomplete, {} path(s) could not be restored",
            report.failed_pairs.len()
        );
        for (src, dst, err) in &report.failed_pairs {
            eprintln!(
                "fatal: rollback failed, source={}, destination={}, error={}",
                util::to_workdir_path(src).display(),
                util::to_workdir_path(dst).display(),
                err
            );
        }
        eprintln!(
            "fatal: repository may be in a partially moved state; please inspect and recover manually"
        );
    }
}

fn perform_moves(
    plan: MovePlan,
    verbose: bool,
    dry_run: bool,
    force: bool,
    index: &mut Index,
) -> bool {
    let mut moved_pairs: Vec<(PathBuf, PathBuf)> = Vec::new();

    for (src, dst) in &plan.fs_moves {
        let src_workdir = util::to_workdir_path(src);
        let dst_workdir = util::to_workdir_path(dst);

        // If it's a dry run, we just print the move operations without performing them.
        if dry_run {
            println!(
                "Checking rename of '{}' to '{}'",
                src_workdir.display(),
                dst_workdir.display()
            );
            println!(
                "Renaming {} to {}",
                src_workdir.display(),
                dst_workdir.display()
            );
            continue;
        }
        // For actual move, we first check if the parent directory of the destination exists, if not, we try to create it.
        if let Some(parent) = dst.parent()
            && let Err(err) = std::fs::create_dir_all(parent)
        {
            eprintln!(
                "fatal: failed to create destination directory, source={}, destination={}, error={}",
                src_workdir.display(),
                dst_workdir.display(),
                err
            );
            print_rollback_report(rollback_moves(&moved_pairs));
            return false;
        }

        if force && let Ok(meta) = std::fs::symlink_metadata(dst) {
            let file_type = meta.file_type();
            if (file_type.is_file() || file_type.is_symlink())
                && let Err(err) = std::fs::remove_file(dst)
            {
                eprintln!(
                    "fatal: failed to remove destination before force move, source={}, destination={}, error={}",
                    src_workdir.display(),
                    dst_workdir.display(),
                    err
                );
                print_rollback_report(rollback_moves(&moved_pairs));
                return false;
            }
        }

        // Perform the move operation in the filesystem.
        if let Err(e) = std::fs::rename(src, dst) {
            eprintln!(
                "fatal: failed to move, source={}, destination={}, error={}",
                src_workdir.display(),
                dst_workdir.display(),
                e
            );
            print_rollback_report(rollback_moves(&moved_pairs));
            return false;
        }

        moved_pairs.push((src.clone(), dst.clone()));

        // Print the move operation if verbose is enabled.
        if verbose {
            println!(
                "Renaming {} to {}",
                src_workdir.display(),
                dst_workdir.display()
            );
        }
    }

    if dry_run {
        return true;
    }

    // Update index only after all filesystem moves succeeded.
    for (src, dst) in &plan.index_updates {
        let src_rel = util::path_to_string(&util::to_workdir_path(src));
        let dst_workdir = util::to_workdir_path(dst);
        let dst_rel = util::path_to_string(&dst_workdir);

        remove_index_entry_all_stages(index, &dst_rel);

        if index.remove(&src_rel, 0).is_some() {
            let new_entry = calc_file_blob_hash(dst)
                .map_err(|err| {
                    format!(
                        "failed to calculate hash for moved file, source={}, destination={}, error={}",
                        src_rel, dst_rel, err
                    )
                })
                .and_then(|hash| {
                    IndexEntry::new_from_file(&dst_workdir, hash, &util::working_dir()).map_err(
                        |err| {
                            format!(
                                "failed to build index entry for moved file, source={}, destination={}, error={}",
                                src_rel, dst_rel, err
                            )
                        },
                    )
                });

            match new_entry {
                Ok(entry) => index.add(entry),
                Err(err) => {
                    eprintln!("fatal: {err}");
                    return false;
                }
            }
        }
    }

    // After performing all moves, save the index if there were any moves.
    if !moved_pairs.is_empty()
        && let Err(e) = index.save(path::index())
    {
        eprintln!("fatal: failed to save index after mv: {e}");
        return false;
    }

    true
}
