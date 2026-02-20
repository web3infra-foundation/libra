//! Implementation of `git mv` command, which moves/renames files and directories in the working directory and updates the index accordingly.
use crate::utils::{path, util};
use clap::Parser;
use git_internal::internal::index::Index;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
// use std::env;
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

    // check the validity of all sources and the destination,
    // and collect the valid moves to perform.
    // Report all errors at once without performing any move if there are invalid arguments.
    let mut valid_moves: Vec<(PathBuf, PathBuf)> = Vec::new();
    let index_file = path::index();
    let mut index = Index::load(&index_file).unwrap();
    for src in &sources {
        // Check if the source exists before attempting to move it.
        if !src.exists() {
            eprintln!(
                "fatal: bad source, source={}, destination={}",
                util::to_workdir_path(src).display(),
                util::to_workdir_path(&destination).display()
            );
            return;
        } else if src == &destination {
            // Moving a file/directory to itself is not allowed.
            eprintln!(
                "fatal: can not move directory into itself, source={}, destination={}",
                util::to_workdir_path(src).display(),
                util::to_workdir_path(&destination).display()
            );
            return;
        } else if src.is_dir() {
            // If the destination is a directory and there is already a directory with the same name as the source in the destination, it's an error
            if destination.join(src.file_name().unwrap()).exists() && destination_is_dir {
                eprintln!(
                    "fatal: destination already exists, source={}, destination={}",
                    util::to_workdir_path(src).display(),
                    util::to_workdir_path(&destination).display()
                );
                return;
            } else if !destination.join(src.file_name().unwrap()).exists() && destination_is_dir {
                // If the source is a directory, we need to check if there are tracked files in the directory and record their moves.
                match resolve_move_directory(src, &destination, &index) {
                    Ok(tracked_moves) => {
                        valid_moves.extend(tracked_moves);
                    }
                    Err(e) => {
                        eprintln!("{}", e);
                        return;
                    }
                }
            }
        } else if !index.tracked(&util::path_to_string(&util::to_workdir_path(src)), 0) {
            // If the source file is not tracked in the index, we consider it an error and do not perform the move.
            eprintln!(
                "fatal: not under version control, source={}, destination={}",
                util::to_workdir_path(src).display(),
                util::to_workdir_path(&destination).display()
            );
            return;
        } else if is_conflicted_in_index(&index, src) {
            // If the source file is in conflict in the index, we consider it an error and do not perform the move.
            eprintln!(
                "fatal: conflicted, source={}, destination={}",
                util::to_workdir_path(src).display(),
                util::to_workdir_path(&destination).display()
            );
            return;
        } else {
            // Determine the target path for the move.
            //If the destination is a directory, the target will be destination/src.file_name(),
            //otherwise it will be the destination itself.
            let target = if destination_is_dir {
                destination.join(src.file_name().unwrap())
            } else {
                destination.clone()
            };
            // If the target already exists, it's an error unless --force is specified.
            if target.exists() {
                if !args.force {
                    eprintln!(
                        "fatal: destination already exists, source={}, destination={}",
                        util::to_workdir_path(src).display(),
                        util::to_workdir_path(&target).display()
                    );
                    return;
                }
                //only allow overwriting files or symlinks, not directories.
                if !(target.is_file() || target.is_symlink()) {
                    eprintln!(
                        "fatal: Cannot overwrite, source={}, destination={}",
                        util::to_workdir_path(src).display(),
                        util::to_workdir_path(&target).display()
                    );
                    return;
                }
            }
            valid_moves.push((src.clone(), target));
        }
    }

    // Check if there are multiple sources moving to the same target path, which is not allowed.
    if check_multiple_sources_to_same_target(&sources, &destination) {
        eprintln!(
            "fatal: multiple sources moving to the same target path,source={}, destination={}",
            util::to_workdir_path(&sources[sources.len() - 1]).display(),
            util::to_workdir_path(&destination).display()
        );
        return;
    }
    perform_moves(valid_moves, args.verbose, args.dry_run, &mut index).await;
}

fn to_absolute_path(path: impl AsRef<Path>) -> PathBuf {
    util::workdir_to_absolute(path.as_ref())
}

///check if there are tracked files in the source directory
/// if there are tracked files, we will record the moves of these tracked files,
///  the move of the directory itself will be handled by filesystem and we don't need to record it.
fn resolve_move_directory(
    src: &Path,
    dst: &Path,
    index: &Index,
) -> Result<Vec<(PathBuf, PathBuf)>, String> {
    let files = util::list_files(src).unwrap();
    //only consider tracked files for move.
    //If there are untracked files in the source directory
    let tracked_moves: Vec<(PathBuf, PathBuf)> = files
        .into_iter()
        .filter(|file| index.tracked(&util::path_to_string(file), 0))
        .map(|file| {
            let relative_path = util::to_relative(&file, src);
            (to_absolute_path(&file), dst.join(relative_path))
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

/// Check if the source file is in conflict in the index
fn is_conflicted_in_index(index: &Index, src: &Path) -> bool {
    let src_str = util::path_to_string(&util::to_workdir_path(src));
    (1..=3).any(|stage| index.tracked(&src_str, stage))
}
/// Check if there are multiple sources moving to the same target path, which is not allowed.
fn check_multiple_sources_to_same_target(sources: &[PathBuf], destination: &Path) -> bool {
    let mut target_paths = HashSet::new();
    for src in sources {
        let target = if destination.is_dir() {
            destination.join(src.file_name().unwrap())
        } else {
            destination.to_path_buf()
        };
        if !target_paths.insert(target) {
            return true; // Found a duplicate target path
        }
    }
    false
}

async fn perform_moves(
    moves: Vec<(PathBuf, PathBuf)>,
    verbose: bool,
    dry_run: bool,
    index: &mut Index,
) {
    let mut moved_any = false;
    for (src, dst) in moves {
        //relative path used for index update
        let src_rel = util::path_to_string(&util::to_workdir_path(&src));
        let dst_rel = util::path_to_string(&util::to_workdir_path(&dst));
        // If it's a dry run, we just print the move operations without performing them.
        if dry_run {
            println!(
                "Checking rename of '{}' to '{}'",
                util::to_workdir_path(&src).display(),
                util::to_workdir_path(&dst).display()
            );
            println!(
                "Renaming {} to {}",
                util::to_workdir_path(&src).display(),
                util::to_workdir_path(&dst).display()
            );
            continue;
        }
        // Perform the move operation in the filesystem.
        if let Err(e) = std::fs::rename(&src, &dst) {
            eprintln!(
                "fatal: failed to move, source={}, destination={}, error={}",
                util::to_workdir_path(&src).display(),
                util::to_workdir_path(&dst).display(),
                e
            );
            return;
        }
        // Update the index: remove the old path and add the new path with the same blob hash.
        if let Some(mut entry) = index.remove(&src_rel, 0) {
            entry.name = dst_rel.clone();
            entry.flags.name_length = entry.name.len() as u16;
            index.add(entry);
        }

        moved_any = true;
        // Print the move operation if verbose is enabled.
        if verbose {
            println!(
                "Renaming {} to {}",
                util::to_workdir_path(&src).display(),
                util::to_workdir_path(&dst).display()
            );
        }
    }
    // After performing all moves, save the index if there were any moves.
    if moved_any && let Err(e) = index.save(path::index()) {
        eprintln!("fatal: failed to save index after mv: {e}");
    }
}
