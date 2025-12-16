//! Implements status reporting with ignore policy support, computing staged/unstaged/untracked sets and printing concise summaries.

use std::{
    collections::{HashMap, HashSet},
    io::Write,
    path::PathBuf,
};

use clap::{Parser, ValueEnum};
use colored::Colorize;
use git_internal::{
    hash::{ObjectHash, get_hash_kind},
    internal::{
        index::Index,
        object::{
            commit::Commit,
            tree::{Tree, TreeItemMode},
        },
    },
};

use super::stash;
use crate::{
    command::calc_file_blob_hash,
    internal::head::Head,
    utils::{
        ignore::{self, IgnorePolicy},
        object_ext::{CommitExt, TreeExt},
        path, util,
    },
};

#[derive(Parser, Debug, Default)]
pub struct StatusArgs {
    /// Output in a machine-readable format (default v1). Use v2 for extended format.
    #[clap(
        long = "porcelain",
        value_name = "VERSION",
        num_args = 0..=1,
        default_missing_value = "v1",
        conflicts_with = "short"
    )]
    pub porcelain: Option<PorcelainVersion>,

    /// Give the output in the short-format
    #[clap(short = 's', long = "short", conflicts_with = "porcelain")]
    pub short: bool,

    /// Output with branch info (short or porcelain mode)
    #[clap(long = "branch")]
    pub branch: bool,

    /// Output with stash info (only in standard mode)
    #[clap(long = "show-stash")]
    pub show_stash: bool,

    /// Show ignored files
    #[clap(long = "ignored")]
    pub ignored: bool,

    /// Control untracked files display (normal|all|no)
    #[clap(
        long = "untracked-files",
        value_name = "MODE",
        default_value = "normal"
    )]
    pub untracked_files: UntrackedFiles,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum PorcelainVersion {
    #[clap(name = "v1")]
    V1,
    #[clap(name = "v2")]
    V2,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum, Default)]
pub enum UntrackedFiles {
    /// Show untracked files (default): only list untracked directories, not their contents.
    #[default]
    Normal,
    /// Show all untracked files, recursively listing files within untracked directories.
    All,
    /// Do not show untracked files
    No,
}

/// path: to workdir
#[derive(Debug, Default, Clone)]
pub struct Changes {
    pub new: Vec<PathBuf>,
    pub modified: Vec<PathBuf>,
    pub deleted: Vec<PathBuf>,
}

/// Collapse untracked files into their parent directories when possible.
///
/// For `--untracked-files=normal` mode, if all files in a directory are untracked,
/// we display just the directory name instead of listing each file.
///
/// # Arguments
/// * `untracked_files` - List of untracked file paths
/// * `index` - The index to check if any files in directories are tracked
///
/// # Returns
/// A list where fully-untracked directories are collapsed to just the directory path
fn collapse_untracked_directories(untracked_files: Vec<PathBuf>, index: &Index) -> Vec<PathBuf> {
    use std::collections::BTreeSet;

    if untracked_files.is_empty() {
        return untracked_files;
    }

    // Group files by their top-level directory
    let mut dir_files: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    let mut root_files: Vec<PathBuf> = Vec::new();

    for file in &untracked_files {
        let components: Vec<_> = file.components().collect();
        if components.len() > 1 {
            // File is in a subdirectory
            let top_dir = PathBuf::from(components[0].as_os_str());
            dir_files.entry(top_dir).or_default().push(file.clone());
        } else {
            // File is in root
            root_files.push(file.clone());
        }
    }

    let mut result: BTreeSet<PathBuf> = BTreeSet::new();

    // Add root files directly
    for file in root_files {
        result.insert(file);
    }

    // For each directory, check if any file inside is tracked
    for (dir, files) in dir_files {
        // Check if any file in this directory (or subdirectories) is tracked
        let dir_prefix = format!("{}/", dir.display());
        let has_tracked_files = index.tracked_files().iter().any(|f| {
            f.to_str()
                .map(|s| s.starts_with(&dir_prefix))
                .unwrap_or(false)
        });

        if has_tracked_files {
            // Directory has some tracked files, show individual untracked files
            for file in files {
                result.insert(file);
            }
        } else {
            // Directory is completely untracked, show just the directory
            let mut dir_path = dir;
            // Add trailing separator to indicate it's a directory
            let dir_str = format!("{}/", dir_path.display());
            dir_path = PathBuf::from(dir_str);
            result.insert(dir_path);
        }
    }

    result.into_iter().collect()
}
impl Changes {
    pub fn is_empty(&self) -> bool {
        self.new.is_empty() && self.modified.is_empty() && self.deleted.is_empty()
    }

    /// to relative path(to cur_dir)
    pub fn to_relative(&self) -> Changes {
        let mut change = self.clone();
        [&mut change.new, &mut change.modified, &mut change.deleted]
            .into_iter()
            .for_each(|paths| {
                *paths = paths.iter().map(util::workdir_to_current).collect();
            });
        change
    }
    pub fn polymerization(&self) -> Vec<PathBuf> {
        let mut poly = self.new.clone();
        poly.extend(self.modified.clone());
        poly.extend(self.deleted.clone());
        poly
    }
}

/**
 * Two parts:
 * 1. unstaged
 * 2. staged to be committed
 */
pub async fn execute_to(args: StatusArgs, writer: &mut impl Write) {
    if !util::check_repo_exist() {
        return;
    }

    let is_porcelain = args.porcelain.is_some();
    let is_standard_mode = !is_porcelain && !args.short;

    // Do not output branch info in porcelain or short mode
    if is_standard_mode {
        match Head::current().await {
            Head::Detached(commit_hash) => {
                writeln!(writer, "HEAD detached at {}", &commit_hash.to_string()[..8]).unwrap();
            }
            Head::Branch(branch) => {
                writeln!(writer, "On branch {branch}").unwrap();
            }
        }

        if Head::current_commit().await.is_none() {
            writeln!(writer, "\nNo commits yet\n").unwrap();
        }
    }

    if is_standard_mode && args.show_stash {
        let stash_num = stash::get_stash_num().unwrap_or(0);
        let entry_text = if stash_num == 1 { "entry" } else { "entries" };
        if stash_num > 0 {
            writeln!(writer, "Your stash currently has {stash_num} {entry_text}").unwrap();
        }
    }

    // to cur_dir relative path
    let staged = changes_to_be_committed().await.to_relative();
    let mut unstaged = changes_to_be_staged().to_relative();
    let mut ignored_files = if args.ignored && !matches!(args.untracked_files, UntrackedFiles::No) {
        list_ignored_files().to_relative().new
    } else {
        vec![]
    };

    // Handle untracked-files option
    match args.untracked_files {
        UntrackedFiles::No => {
            unstaged.new.clear();
            ignored_files.clear();
        }
        UntrackedFiles::Normal => {
            // Collapse fully-untracked directories into single entries
            let index = Index::load(path::index()).unwrap();
            unstaged.new = collapse_untracked_directories(unstaged.new, &index);
            ignored_files = collapse_untracked_directories(ignored_files, &index);
        }
        UntrackedFiles::All => {
            // Show all untracked files (current behavior, no collapsing)
        }
    }

    // Use machine-readable output in porcelain mode
    match args.porcelain {
        Some(PorcelainVersion::V2) => {
            if args.branch {
                write_branch_info_v2(writer).await;
            }
            output_porcelain_v2(&staged, &unstaged, &ignored_files, writer).await;
            return;
        }
        Some(PorcelainVersion::V1) => {
            if args.branch {
                print_branch_info(writer).await;
            }
            output_porcelain(&staged, &unstaged, writer);
            // Porcelain: ignored files prefixed with "!!"
            if args.ignored && !ignored_files.is_empty() {
                for file in &ignored_files {
                    writeln!(writer, "!! {}", file.display()).unwrap();
                }
            }
            return;
        }
        None => {}
    };

    // Use short format output
    if args.short {
        // if branch option is specified, print the branch info
        if args.branch {
            print_branch_info(writer).await;
        }
        output_short_format(&staged, &unstaged, writer).await;
        // Short: append ignored files with "!!"
        if args.ignored {
            for file in &ignored_files {
                writeln!(writer, "!! {}", file.display()).unwrap();
            }
        }
        return;
    }

    if staged.is_empty() && unstaged.is_empty() {
        writeln!(writer, "nothing to commit, working tree clean").unwrap();
        return;
    }

    if !staged.is_empty() {
        println!("Changes to be committed:");
        println!("  use \"libra restore --staged <file>...\" to unstage");
        staged.deleted.iter().for_each(|f| {
            let str = format!("\tdeleted: {}", f.display());
            writeln!(writer, "{}", str.bright_green()).unwrap();
        });
        staged.modified.iter().for_each(|f| {
            let str = format!("\tmodified: {}", f.display());
            writeln!(writer, "{}", str.bright_green()).unwrap();
        });
        staged.new.iter().for_each(|f| {
            let str = format!("\tnew file: {}", f.display());
            writeln!(writer, "{}", str.bright_green()).unwrap();
        });
    }

    if !unstaged.deleted.is_empty() || !unstaged.modified.is_empty() {
        println!("Changes not staged for commit:");
        println!("  use \"libra add <file>...\" to update what will be committed");
        println!("  use \"libra restore <file>...\" to discard changes in working directory");
        unstaged.deleted.iter().for_each(|f| {
            let str = format!("\tdeleted: {}", f.display());
            writeln!(writer, "{}", str.bright_red()).unwrap();
        });
        unstaged.modified.iter().for_each(|f| {
            let str = format!("\tmodified: {}", f.display());
            writeln!(writer, "{}", str.bright_red()).unwrap();
        });
    }
    if !unstaged.new.is_empty() {
        println!("Untracked files:");
        println!("  use \"libra add <file>...\" to include in what will be committed");
        unstaged.new.iter().for_each(|f| {
            let str = format!("\t{}", f.display());
            writeln!(writer, "{}", str.bright_red()).unwrap();
        });
    }

    if args.ignored && !ignored_files.is_empty() {
        println!("Ignored files:");
        println!("  (modify .libraignore to change which files are ignored)");
        for f in &ignored_files {
            let str = format!("\t{}", f.display());
            writeln!(writer, "{}", str.bright_red()).unwrap();
        }
    }
}

pub fn output_porcelain(staged: &Changes, unstaged: &Changes, writer: &mut impl Write) {
    // Use generate_short_format_status to correctly merge staged and unstaged states
    // e.g., a file that is staged then modified should show "MM" not two separate lines
    let status_list = generate_short_format_status(staged, unstaged);
    for (file, staged_status, unstaged_status) in status_list {
        writeln!(
            writer,
            "{}{} {}",
            staged_status,
            unstaged_status,
            file.display()
        )
        .unwrap();
    }
}

/// File information from HEAD tree for porcelain v2 output.
/// Stores the mode (as octal u32) and hash from HEAD tree entries.
struct FileInfo {
    /// File mode from HEAD tree (e.g., 0o100644 for regular file)
    mode: u32,
    /// Object hash from HEAD tree as string
    hash: String,
}

/// Get file mode from TreeItemMode
fn tree_item_mode_to_u32(mode: TreeItemMode) -> u32 {
    match mode {
        TreeItemMode::Blob => 0o100644,
        TreeItemMode::BlobExecutable => 0o100755,
        TreeItemMode::Link => 0o120000,
        TreeItemMode::Tree => 0o040000,
        TreeItemMode::Commit => 0o160000, // submodule
    }
}

/// Format file mode as 6-digit octal string for porcelain v2 output.
///
/// # Examples
/// - Regular file: `0o100644` → `"100644"`
/// - Executable file: `0o100755` → `"100755"`
/// - Symlink: `0o120000` → `"120000"`
/// - Directory/tree: `0o040000` → `"040000"`
/// - Submodule/commit: `0o160000` → `"160000"`
/// - Deleted/missing file: `0` → `"000000"`
fn format_mode(mode: u32) -> String {
    format!("{:06o}", mode)
}

/// Convert a current-directory-relative path to a workdir-relative path
fn current_to_workdir(path: &std::path::Path) -> PathBuf {
    // Get the absolute path first
    let abs_path = util::cur_dir().join(path);
    // Then convert to workdir-relative path
    util::to_workdir_path(&abs_path)
}

/// Detect working tree file mode
#[cfg(unix)]
fn get_worktree_mode(file_path: &std::path::Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    let workdir_path = current_to_workdir(file_path);
    let abs_path = util::workdir_to_absolute(&workdir_path);
    if let Ok(metadata) = std::fs::symlink_metadata(&abs_path) {
        if metadata.file_type().is_symlink() {
            0o120000
        } else if metadata.permissions().mode() & 0o111 != 0 {
            0o100755
        } else {
            0o100644
        }
    } else {
        0o100644
    }
}

#[cfg(not(unix))]
fn get_worktree_mode(_file_path: &std::path::Path) -> u32 {
    0o100644
}

/// Returns true if the given file mode represents a submodule (gitlink) entry.
///
/// In Git, submodules are stored in the index and tree with mode `0o160000`.
/// This function checks for that specific mode to identify submodules.
fn is_submodule_mode(mode: u32) -> bool {
    mode == 0o160000
}

/// Generate submodule status string (placeholder implementation).
///
/// Currently returns `"S..."` as a placeholder since full submodule support
/// is not yet implemented.
///
/// # TODO
///
/// Full format should be `S<c><m><u>` where:
/// - `c`: commit changed (`C`) or not (`.`)
/// - `m`: tracked changes (`M`) or not (`.`)
/// - `u`: untracked changes (`U`) or not (`.`)
fn get_submodule_status(_file_path: &std::path::Path) -> String {
    "S...".to_string()
}

/// Output porcelain v2 format
pub async fn output_porcelain_v2(
    staged: &Changes,
    unstaged: &Changes,
    ignored: &[PathBuf],
    writer: &mut impl Write,
) {
    let zero_hash = zero_hash_str();
    let index = match Index::load(path::index()) {
        Ok(idx) => idx,
        Err(e) => {
            writeln!(writer, "error: failed to load index: {}", e).ok();
            return;
        }
    };
    let head_commit = Head::current_commit().await;

    // Build a map of HEAD tree items with mode info
    let head_tree_items: HashMap<PathBuf, FileInfo> = if let Some(commit_hash) = head_commit {
        let commit = Commit::load(&commit_hash);
        let tree = Tree::load(&commit.tree_id);
        tree.get_plain_items_with_mode()
            .into_iter()
            .map(|(path, hash, mode)| {
                (
                    path,
                    FileInfo {
                        mode: tree_item_mode_to_u32(mode),
                        hash: hash.to_string(),
                    },
                )
            })
            .collect()
    } else {
        HashMap::new()
    };

    let status_list = generate_short_format_status(staged, unstaged);
    for (file, staged_status, unstaged_status) in status_list {
        if staged_status == '?' && unstaged_status == '?' {
            writeln!(writer, "? {}", file.display()).unwrap();
            continue;
        }

        // Convert relative path (to current dir) back to workdir-relative path for index lookup
        let workdir_path = current_to_workdir(&file);
        let file_str = workdir_path.to_str().unwrap_or_default();

        // Get index info (mI, hI)
        let (mode_index, hash_index) = if let Some(entry) = index.get(file_str, 0) {
            (entry.mode, entry.hash.to_string())
        } else {
            // File not in index (shouldn't happen for tracked files, but handle gracefully)
            (0o100644, zero_hash.clone())
        };

        // Get HEAD tree info (mH, hH)
        let (mode_head, hash_head) = if staged_status == 'A' {
            // New file: use 000000 and zero hash for HEAD
            (0, zero_hash.clone())
        } else if let Some(info) = head_tree_items.get(&workdir_path) {
            (info.mode, info.hash.clone())
        } else {
            // File not in HEAD tree
            (0, zero_hash.clone())
        };

        // Get worktree mode (mW)
        let mode_worktree = if unstaged_status == 'D' {
            // Deleted in worktree
            0
        } else {
            get_worktree_mode(&file)
        };

        // Determine submodule status
        let sub = if is_submodule_mode(mode_index) || is_submodule_mode(mode_head) {
            get_submodule_status(&file)
        } else {
            "N...".to_string()
        };

        // Format: 1 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>
        writeln!(
            writer,
            "1 {}{} {} {} {} {} {} {} {}",
            staged_status,
            unstaged_status,
            sub,
            format_mode(mode_head),
            format_mode(mode_index),
            format_mode(mode_worktree),
            hash_head,
            hash_index,
            file.display()
        )
        .unwrap();
    }

    for file in ignored {
        writeln!(writer, "! {}", file.display()).unwrap();
    }
}

fn zero_hash_str() -> String {
    ObjectHash::zero_str(get_hash_kind())
}

/// Core logic for generating short format status without color (for testing)
pub fn generate_short_format_status(
    staged: &Changes,
    unstaged: &Changes,
) -> Vec<(std::path::PathBuf, char, char)> {
    use std::collections::HashMap;

    // Create a map to track all files and their status
    let mut file_status: HashMap<PathBuf, (char, char)> = HashMap::new();

    // Process staged changes
    for file in &staged.new {
        file_status.insert(file.clone(), ('A', ' '));
    }
    for file in &staged.modified {
        file_status.insert(file.clone(), ('M', ' '));
    }
    for file in &staged.deleted {
        file_status.insert(file.clone(), ('D', ' '));
    }

    // Helper to process unstaged changes (modified/deleted)
    fn process_unstaged_changes(
        files: &[PathBuf],
        file_status: &mut std::collections::HashMap<PathBuf, (char, char)>,
        unstaged_char: char,
    ) {
        for file in files {
            let staged_status = file_status.get(file).map(|(s, _)| *s);
            if let Some(status) = staged_status {
                // File is both staged and unstaged - keep staged status, update unstaged
                file_status.insert(file.clone(), (status, unstaged_char));
            } else {
                // File is only unstaged
                file_status.insert(file.clone(), (' ', unstaged_char));
            }
        }
    }

    // Process unstaged changes
    process_unstaged_changes(&unstaged.modified, &mut file_status, 'M');
    process_unstaged_changes(&unstaged.deleted, &mut file_status, 'D');

    for file in &unstaged.new {
        // Untracked files
        file_status.insert(file.clone(), ('?', '?'));
    }

    // Sort files by path for consistent output
    let mut sorted_files: Vec<_> = file_status.iter().collect();
    sorted_files.sort_by(|a, b| a.0.cmp(b.0));

    sorted_files
        .into_iter()
        .map(|(file, (staged_status, unstaged_status))| {
            (file.clone(), *staged_status, *unstaged_status)
        })
        .collect()
}

pub async fn output_short_format(staged: &Changes, unstaged: &Changes, writer: &mut impl Write) {
    // Check if colors should be used
    let use_colors = should_use_colors().await;

    // Get the status information using the core logic
    let status_list = generate_short_format_status(staged, unstaged);

    // Output the short format
    for (file, staged_status, unstaged_status) in status_list {
        if use_colors {
            let colored_output = format_colored_status(staged_status, unstaged_status, &file);
            writeln!(writer, "{}", colored_output).unwrap();
        } else {
            writeln!(
                writer,
                "{}{} {}",
                staged_status,
                unstaged_status,
                file.display()
            )
            .unwrap();
        }
    }
}

/// Check if colors should be used based on configuration
async fn should_use_colors() -> bool {
    use std::io::{self, IsTerminal};

    use crate::internal::config::Config;

    // Check color.status.short configuration
    if let Some(color_setting) = Config::get("color", Some("status"), "short").await {
        match color_setting.as_str() {
            "always" => true,
            "never" | "false" => false,
            "auto" | "true" => {
                // Check if output is to a terminal
                io::stdout().is_terminal()
            }
            _ => false,
        }
    } else {
        // Check color.ui configuration as fallback
        if let Some(color_setting) = Config::get("color", None, "ui").await {
            match color_setting.as_str() {
                "always" => true,
                "never" | "false" => false,
                "auto" | "true" => {
                    // Check if output is to a terminal
                    io::stdout().is_terminal()
                }
                _ => false,
            }
        } else {
            // Default to auto (check if terminal)
            io::stdout().is_terminal()
        }
    }
}

/// Format the status with colors according to Git conventions
fn format_colored_status(
    staged_status: char,
    unstaged_status: char,
    file: &std::path::Path,
) -> String {
    use colored::Colorize;

    // Color the status characters based on Git conventions
    let colored_staged = match staged_status {
        'A' => staged_status.to_string().green(),
        'M' => staged_status.to_string().green(),
        'D' => staged_status.to_string().red(),
        'R' => staged_status.to_string().yellow(),
        'C' => staged_status.to_string().yellow(),
        'U' => staged_status.to_string().red(),
        '?' => staged_status.to_string().bright_red(),
        ' ' => staged_status.to_string().into(),
        _ => staged_status.to_string().into(),
    };

    let colored_unstaged = match unstaged_status {
        'M' => unstaged_status.to_string().red(),
        'D' => unstaged_status.to_string().red(),
        'U' => unstaged_status.to_string().red(),
        '?' => unstaged_status.to_string().bright_red(),
        '!' => unstaged_status.to_string().bright_red(),
        ' ' => unstaged_status.to_string().into(),
        _ => unstaged_status.to_string().into(),
    };

    format!("{}{} {}", colored_staged, colored_unstaged, file.display())
}

pub async fn execute(args: StatusArgs) {
    execute_to(args, &mut std::io::stdout()).await
}

/// Check if the working tree is clean
pub async fn is_clean() -> bool {
    let staged = changes_to_be_committed().await;
    let unstaged = changes_to_be_staged();
    staged.is_empty() && unstaged.is_empty()
}

/**
 * Compare the difference between `index` and the last `Commit Tree`
 */
pub async fn changes_to_be_committed() -> Changes {
    let mut changes = Changes::default();
    let index = Index::load(path::index()).unwrap();
    let head_commit = Head::current_commit().await;
    let tracked_files = index.tracked_files();

    if head_commit.is_none() {
        // no commit yet
        changes.new = tracked_files;
        return changes;
    }

    let head_commit = head_commit.unwrap();
    let commit = Commit::load(&head_commit);
    let tree = Tree::load(&commit.tree_id);
    let tree_files = tree.get_plain_items();

    for (item_path, item_hash) in tree_files.iter() {
        let item_str = item_path.to_str().unwrap();
        if index.tracked(item_str, 0) {
            if !index.verify_hash(item_str, 0, item_hash) {
                changes.modified.push(item_path.clone());
            }
        } else {
            // in the last commit but not in the index
            changes.deleted.push(item_path.clone());
        }
    }
    let tree_files_set: HashSet<PathBuf> = tree_files.into_iter().map(|(path, _)| path).collect();
    // `new` means the files in index but not in the last commit
    changes.new = tracked_files
        .into_iter()
        .filter(|path| !tree_files_set.contains(path))
        .collect();

    changes
}

/// Compare the difference between `index` and the `workdir` using the default ignore rules.
pub fn changes_to_be_staged() -> Changes {
    changes_to_be_staged_with_policy(IgnorePolicy::Respect)
}

/// Variant of [`changes_to_be_staged`] that lets callers pick the ignore strategy explicitly.
/// Commands such as `add --force` or `status --ignored` can switch policies as needed.
pub fn changes_to_be_staged_with_policy(policy: IgnorePolicy) -> Changes {
    let mut changes = Changes::default();
    let workdir = util::working_dir();
    let index = Index::load(path::index()).unwrap();
    let tracked_files = index.tracked_files();
    for file in tracked_files.iter() {
        if ignore::should_ignore(file, policy, &index) {
            continue;
        }
        let file_str = file.to_str().unwrap();
        let file_abs = util::workdir_to_absolute(file);
        if !file_abs.exists() {
            changes.deleted.push(file.clone());
        } else if index.is_modified(file_str, 0, &workdir) {
            // only calc the hash if the file is modified (metadata), for optimization
            let file_hash = calc_file_blob_hash(&file_abs).unwrap();
            if !index.verify_hash(file_str, 0, &file_hash) {
                changes.modified.push(file.clone());
            }
        }
    }
    let files = util::list_workdir_files().unwrap(); // to workdir
    for file in ignore::filter_workdir_paths(files.into_iter(), policy, &index) {
        let file_str = file.to_str().unwrap();
        if !index.tracked(file_str, 0) {
            // file not tracked in `index`
            changes.new.push(file);
        }
    }
    changes
}

/// List ignored files (not tracked by index, but ignored by .libraignore) under workdir
pub fn list_ignored_files() -> Changes {
    changes_to_be_staged_with_policy(IgnorePolicy::OnlyIgnored)
}

/// Helper function for printing branch info when `branch` flag is enabled
async fn print_branch_info(writer: &mut impl Write) {
    match Head::current().await {
        Head::Detached(commit_hash) => {
            writeln!(
                writer,
                "## HEAD (detached at {})",
                &commit_hash.to_string()[..8]
            )
            .unwrap();
        }
        Head::Branch(branch) => {
            writeln!(writer, "## {branch}").unwrap();
        }
    }
}

/// Write branch information in porcelain v2 style
async fn write_branch_info_v2(writer: &mut impl Write) {
    let head = Head::current().await;
    let head_commit = Head::current_commit().await;
    let oid = head_commit
        .map(|c| c.to_string())
        .unwrap_or_else(|| "(initial)".to_string());

    match head {
        Head::Detached(_) => {
            writeln!(writer, "# branch.head (detached)").unwrap();
        }
        Head::Branch(name) => {
            writeln!(writer, "# branch.head {}", name).unwrap();
        }
    }
    writeln!(writer, "# branch.oid {}", oid).unwrap();
}

#[cfg(test)]
mod test {}
