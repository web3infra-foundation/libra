//! Implementation of `git mv` command, which moves/renames files and directories in the working directory and updates the index accordingly.
use crate::utils::{path, util};
use clap::Parser;
use git_internal::internal::index::Index;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
#[derive(Parser, Debug)]
pub struct MvArgs {
    /// Path list: one or more <source> followed by <destination>
    /// The <destination> is required and must be the last argument. It can be either a file or a directory.
    ///  If there are multiple <source>, the <destination> must be an existing directory.
    #[clap(required = false)]
    pub paths: Vec<String>,

    /// more detailed output
    #[clap(short = 'v', long)]
    pub verbose: bool,

    /// dry run
    #[clap(short = 'n', long)]
    pub dry_run: bool,

    /// Force move/rename even if the destination already exists (overwriting it)
    #[clap(short = 'f', long)]
    pub force: bool,
}

pub async fn execute(args: MvArgs) {
    if !util::check_repo_exist() {
        return;
    }
    // If the user just types `git mv` without enough arguments, print usage information instead of an error message.
    if args.paths.len() < 2 {
        eprintln!("usage: libra mv [<options>] <source>... <destination>");
        eprintln!();
        eprintln!("-v, --[no-]verbose    be verbose");
        eprintln!("-n, --[no-]dry-run    dry run");
        eprintln!("-f, --[no-]force      force move/rename even if target exists");
        return;
    }

    let paths: Vec<PathBuf> = args.paths.iter().map(PathBuf::from).collect();
    let sources: Vec<PathBuf> = paths[0..paths.len() - 1]
        .iter()
        .map(to_absolute_path)
        .collect();
    let destination = to_absolute_path(&paths[paths.len() - 1]);
    // Check if the destination is a directory (if it exists), which affects how we handle multiple sources.
    let destination_is_dir = destination.is_dir();
    // If there are multiple sources, the destination must be an existing directory.
    if sources.len() > 1 && !destination_is_dir {
        eprintln!(
            "fatal: destination '{}' is not a directory",
            util::to_workdir_path(&destination).display()
        );
        return;
    }

    // Check the validity of all sources and collect the valid move operations.
    let mut valid_moves: Vec<(PathBuf, PathBuf)> = Vec::new();
    let index_file = path::index();
    let mut index = match Index::load(&index_file) {
        Ok(index) => index,
        Err(err) => {
            eprintln!("fatal: {}", err);
            return;
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
            Ok(moves) => valid_moves.extend(moves),
            Err(err) => {
                eprintln!("{err}");
                return;
            }
        }
    }

    if has_duplicate_target(&valid_moves) {
        eprintln!(
            "fatal: multiple sources moving to the same target path, source={}, destination={}",
            util::to_workdir_path(&sources[sources.len() - 1]).display(),
            util::to_workdir_path(&destination).display()
        );
        return;
    }
    perform_moves(valid_moves, args.verbose, args.dry_run, &mut index).await;
}
/// Validates the source path and collects the move operations (source and target paths) to be performed.
fn validate_source_and_collect_moves(
    src: &Path,
    destination: &Path,
    destination_is_dir: bool,
    index: &Index,
    force: bool,
) -> Result<Vec<(PathBuf, PathBuf)>, String> {
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
/// Validates the source directory and collects the move operations for all tracked files under the directory.
fn validate_source_directory(
    src: &Path,
    destination: &Path,
    destination_is_dir: bool,
    index: &Index,
) -> Result<Vec<(PathBuf, PathBuf)>, String> {
    // For directory move, we require the destination to be an existing directory
    if !destination_is_dir {
        return Err(format!(
            "fatal: destination '{}' is not a directory",
            util::to_workdir_path(destination).display()
        ));
    }

    let src_file_name = src.file_name().ok_or_else(|| {
        format!(
            "fatal: invalid source directory '{}': no file name component",
            util::to_workdir_path(src).display()
        )
    })?;

    if destination.join(src_file_name).exists() {
        return Err(format!(
            "fatal: destination already exists, source={}, destination={}",
            util::to_workdir_path(src).display(),
            util::to_workdir_path(destination).display()
        ));
    }

    resolve_move_directory(src, destination, index)
}
/// Validates the source file and returns the move operation if valid.
fn validate_source_file(
    src: &Path,
    destination: &Path,
    destination_is_dir: bool,
    index: &Index,
    force: bool,
) -> Result<Vec<(PathBuf, PathBuf)>, String> {
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
        let file_name = src.file_name().ok_or_else(|| {
            format!(
                "fatal: invalid source path (no file name), source={}, destination={}",
                util::to_workdir_path(src).display(),
                util::to_workdir_path(destination).display()
            )
        })?;
        destination.join(file_name)
    } else {
        destination.to_path_buf()
    };

    if target.exists() {
        if !force {
            return Err(format!(
                "fatal: destination already exists, source={}, destination={}",
                util::to_workdir_path(src).display(),
                util::to_workdir_path(&target).display()
            ));
        }
        if !(target.is_file() || target.is_symlink()) {
            return Err(format!(
                "fatal: Cannot overwrite, source={}, destination={}",
                util::to_workdir_path(src).display(),
                util::to_workdir_path(&target).display()
            ));
        }
    }

    Ok(vec![(src.to_path_buf(), target)])
}

fn to_absolute_path(path: impl AsRef<Path>) -> PathBuf {
    util::workdir_to_absolute(path.as_ref())
}

/// Checks if there are tracked files in the source directory.
///
/// If there are tracked files, records the moves of these tracked files.
/// The move of the directory itself will be handled by the filesystem
/// and doesn't need to be recorded.
///
/// Returns a vector of (source, destination) pairs for tracked files,
/// or an error if the directory contains no tracked files.
fn resolve_move_directory(
    src: &Path,
    dst: &Path,
    index: &Index,
) -> Result<Vec<(PathBuf, PathBuf)>, String> {
    let files = match util::list_files(src) {
        Ok(files) => files,
        Err(e) => {
            return Err(format!(
                "fatal: failed to list files in source directory, source={}, error={}",
                util::to_workdir_path(src).display(),
                e
            ));
        }
    };

    // Determine the name of the source directory so we can preserve it under the destination,
    // matching `git mv src_dir dest` -> `dest/src_dir/...`.
    let src_dir_name = src.file_name().ok_or_else(|| {
        format!(
            "fatal: invalid source directory path: {}",
            util::to_workdir_path(src).display()
        )
    })?;
    let dst_with_src = dst.join(src_dir_name);

    //only consider tracked files for move.
    //If there are untracked files in the source directory
    let tracked_moves: Vec<(PathBuf, PathBuf)> = files
        .into_iter()
        .filter(|file| index.tracked(&util::path_to_string(file), 0))
        .map(move |file| {
            let relative_path = util::to_relative(&file, src);
            (
                util::workdir_to_absolute(&file),
                dst_with_src.join(relative_path),
            )
        })
        .collect();
    // If there are no tracked files in the source directory, we consider it an error and do not perform the move.
    if tracked_moves.is_empty() {
        return Err(format!(
            "fatal: source directory is empty, source={}, destination={}",
            util::to_workdir_path(src).display(),
            util::to_workdir_path(dst).display()
        ));
    }
    Ok(tracked_moves)
}

/// Checks if the source file is in conflict in the index.
fn is_conflicted_in_index(index: &Index, src: &Path) -> bool {
    let src_str = util::path_to_string(&util::to_workdir_path(src));
    (1..=3).any(|stage| index.tracked(&src_str, stage))
}
/// Checks whether multiple move operations target the same destination path.
fn has_duplicate_target(moves: &[(PathBuf, PathBuf)]) -> bool {
    let mut target_paths: HashSet<&PathBuf> = HashSet::new();
    for (_, target) in moves {
        if !target_paths.insert(target) {
            return true;
        }
    }
    false
}
/// Rolls back the performed moves in case of an error during the move operations.
fn rollback_moves(moved_pairs: &[(PathBuf, PathBuf)]) {
    for (src, dst) in moved_pairs.iter().rev() {
        if let Err(err) = std::fs::rename(dst, src) {
            eprintln!(
                "fatal: rollback failed, source={}, destination={}, error={}",
                util::to_workdir_path(src).display(),
                util::to_workdir_path(dst).display(),
                err
            );
        }
    }
}

async fn perform_moves(
    moves: Vec<(PathBuf, PathBuf)>,
    verbose: bool,
    dry_run: bool,
    index: &mut Index,
) {
    let mut moved_pairs: Vec<(PathBuf, PathBuf)> = Vec::new();

    for (src, dst) in &moves {
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
            rollback_moves(&moved_pairs);
            return;
        }

        // Perform the move operation in the filesystem.
        if let Err(e) = std::fs::rename(src, dst) {
            eprintln!(
                "fatal: failed to move, source={}, destination={}, error={}",
                src_workdir.display(),
                dst_workdir.display(),
                e
            );
            rollback_moves(&moved_pairs);
            return;
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
        return;
    }

    // Update index only after all filesystem moves succeeded.
    for (src, dst) in &moved_pairs {
        let src_rel = util::path_to_string(&util::to_workdir_path(src));
        let dst_rel = util::path_to_string(&util::to_workdir_path(dst));
        match index.remove(&src_rel, 0) {
            Some(mut entry) => {
                entry.name = dst_rel;
                entry.flags.name_length = entry.name.len() as u16;
                index.add(entry);
            }
            None => {
                eprintln!(
                    "warning: source path '{}' not found in index during mv to '{}'; index not updated for this entry",
                    src_rel,
                    dst_rel
                );
            }
        }
    }

    // After performing all moves, save the index if there were any moves.
    if !moved_pairs.is_empty()
        && let Err(e) = index.save(path::index())
    {
        eprintln!("fatal: failed to save index after mv: {e}");
    }
}
