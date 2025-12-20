//! Log command rendering commit history with optional decorations, filtering, and custom formatting utilities.

#[cfg(unix)]
use std::io::Write;
#[cfg(unix)]
use std::process::{Command, Stdio};
use std::{
    cmp::min,
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    str::FromStr,
};

use clap::Parser;
use colored::Colorize;
use git_internal::{
    Diff,
    hash::ObjectHash,
    internal::object::{blob::Blob, commit::Commit, tree::Tree},
};

use crate::{
    command::load_object,
    internal::{
        branch::Branch,
        config::Config,
        head::Head,
        log::{
            date_parser::parse_date,
            formatter::{CommitFormatter, FormatContext, FormatType},
        },
    },
    utils::{object_ext::TreeExt, util},
};

#[derive(Parser, Debug)]
pub struct LogArgs {
    /// Limit the number of output
    #[clap(short, long)]
    pub number: Option<usize>,
    /// Shorthand for --pretty=oneline --abbrev-commit
    #[clap(long)]
    pub oneline: bool,

    /// Show abbreviated commit hash instead of full hash
    #[clap(long)]
    pub abbrev_commit: bool,
    /// Length of abbreviated commit hash
    #[clap(long)]
    pub abbrev: Option<usize>,
    /// Show full hash
    #[clap(long)]
    pub no_abbrev_commit: bool,

    /// Show diffs for each commit (like git -p)
    #[clap(short = 'p', long = "patch")]
    pub patch: bool,
    /// Show only names of changed files
    #[clap(long)]
    pub name_only: bool,
    /// Show names and status of changed files
    #[clap(long)]
    pub name_status: bool,
    /// Filter commits by author name or email
    #[clap(long)]
    pub author: Option<String>,
    /// Show commits more recent than a specific date
    #[clap(long)]
    pub since: Option<String>,
    /// Show commits older than a specific date
    #[clap(long)]
    pub until: Option<String>,
    /// Custom pretty format (e.g. `%h - %s`)
    #[clap(long)]
    pub pretty: Option<String>,
    /// Print out ref names of any commits that are shown
    #[clap(
        long,
        default_missing_value = "short",
        require_equals = true,
        num_args = 0..=1,
    )]
    pub decorate: Option<String>,
    /// Do not print out ref names of any commits that are shown
    #[clap(long)]
    pub no_decorate: bool,
    /// Draw a text-based graphical representation of the commit history
    #[clap(long)]
    pub graph: bool,
    /// Show diffstat (file change statistics) for each commit
    #[clap(long)]
    pub stat: bool,

    /// Files to limit diff output (used with -p, --name-only, or --stat)
    #[clap(value_name = "PATHS", num_args = 0..)]
    pathspec: Vec<String>,
}

#[derive(PartialEq, Debug)]
enum DecorateOptions {
    No,
    Short,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeType {
    Added,
    Modified,
    Deleted,
}

#[derive(Debug, Clone)]
pub struct FileChange {
    pub path: PathBuf,
    pub status: ChangeType,
}

struct CommitFilter {
    author: Option<String>,
    since: Option<i64>,
    until: Option<i64>,
    paths: Vec<PathBuf>,
}

impl CommitFilter {
    fn new(
        author: Option<String>,
        since: Option<i64>,
        until: Option<i64>,
        paths: Vec<PathBuf>,
    ) -> Self {
        Self {
            author: author.map(|s| s.to_lowercase()),
            since,
            until,
            paths,
        }
    }

    fn passes_non_path_filters(&self, commit: &Commit) -> bool {
        if let Some(author_filter) = &self.author {
            let author = format!(
                "{} <{}>",
                commit.author.name.to_lowercase(),
                commit.author.email.to_lowercase()
            );
            if !author.contains(author_filter) {
                return false;
            }
        }

        let ts = commit.committer.timestamp as i64;
        if let Some(since) = self.since
            && ts < since
        {
            return false;
        }
        if let Some(until) = self.until
            && ts > until
        {
            return false;
        }

        true
    }

    async fn matches_paths(&self, commit: &Commit, cached_changes: Option<&[FileChange]>) -> bool {
        if self.paths.is_empty() {
            return true;
        }

        if let Some(changes) = cached_changes {
            !changes.is_empty()
        } else {
            commit_touches_paths(commit, &self.paths).await
        }
    }

    async fn matches(&self, commit: &Commit, cached_changes: Option<&[FileChange]>) -> bool {
        if !self.passes_non_path_filters(commit) {
            return false;
        }

        self.matches_paths(commit, cached_changes).await
    }
}

fn str_to_decorate_option(s: &str) -> Result<DecorateOptions, String> {
    match s {
        "no" => Ok(DecorateOptions::No),
        "short" => Ok(DecorateOptions::Short),
        "full" => Ok(DecorateOptions::Full),
        "auto" => {
            if std::io::IsTerminal::is_terminal(&std::io::stdout()) {
                Ok(DecorateOptions::Short)
            } else {
                Ok(DecorateOptions::No)
            }
        }
        _ => Err(s.to_owned()),
    }
}

async fn determine_decorate_option(args: &LogArgs) -> Result<DecorateOptions, String> {
    let arg_deco = args
        .decorate
        .as_ref()
        .map(|s| str_to_decorate_option(s))
        .transpose()?;

    match arg_deco {
        Some(a) => {
            if args.no_decorate {
                let args_os = std::env::args_os().peekable();
                for arg in args_os {
                    if arg == "--no-decorate" {
                        return Ok(a);
                    } else if arg.to_str().unwrap_or_default().starts_with("--decorate") {
                        return Ok(DecorateOptions::No);
                    };
                }
            } else {
                return Ok(a);
            }
        }
        None => {
            if args.no_decorate {
                return Ok(DecorateOptions::No);
            }
        }
    };

    if let Some(config_deco) = Config::get("log", None, "decorate")
        .await
        .and_then(|s| str_to_decorate_option(&s).ok())
    {
        Ok(config_deco)
    } else {
        str_to_decorate_option("auto")
    }
}

/// Get all reachable commits from the given commit hash
/// **didn't consider the order of the commits**
pub async fn get_reachable_commits(commit_hash: String) -> Vec<Commit> {
    let mut queue = VecDeque::new();
    let mut commit_set: HashSet<String> = HashSet::new(); // to avoid duplicate commits because of circular reference
    let mut reachable_commits: Vec<Commit> = Vec::new();
    queue.push_back(commit_hash);

    while !queue.is_empty() {
        let commit_id = queue.pop_front().unwrap();
        let commit_id_hash = ObjectHash::from_str(&commit_id).unwrap();
        let commit = load_object::<Commit>(&commit_id_hash)
            .expect("fatal: storage broken, object not found");
        if commit_set.contains(&commit_id) {
            continue;
        }
        commit_set.insert(commit_id);

        let parent_commit_ids = commit.parent_commit_ids.clone();
        for parent_commit_id in parent_commit_ids {
            queue.push_back(parent_commit_id.to_string());
        }
        reachable_commits.push(commit);
    }
    reachable_commits
}

// Ordered as they should appear in log
#[derive(PartialEq, Eq, PartialOrd, Ord, Clone)]
enum ReferenceKind {
    Tag,    // decorate color = yellow
    Remote, // red
    Local,  // green
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone)]
struct Reference {
    kind: ReferenceKind,
    name: String,
}

pub async fn execute(args: LogArgs) {
    let name_status = args.name_status;
    // Check parameter mutual exclusion: if both name flags and --patch are specified, prioritize the name display flags
    let name_only = args.name_only && !name_status;
    let patch = args.patch && !name_only && !name_status;

    let since = match args.since.as_deref().map(parse_date).transpose() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("fatal: {}", e);
            return;
        }
    };
    let until = match args.until.as_deref().map(parse_date).transpose() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("fatal: {}", e);
            return;
        }
    };
    let path_filters: Vec<PathBuf> = args.pathspec.iter().map(util::to_workdir_path).collect();
    let filter = CommitFilter::new(args.author.clone(), since, until, path_filters.clone());

    let decorate_option = determine_decorate_option(&args)
        .await
        .expect("fatal: invalid --decorate option");

    #[cfg(unix)]
    let mut process = Command::new("less")
        .arg("-R")
        .arg("-F")
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .spawn()
        .expect("failed to execute process");

    let head = Head::current().await;
    // check if the current branch has any commits
    let branch_name = if let Head::Branch(n) = head.to_owned() {
        Some(n)
    } else {
        None
    };
    if let Some(n) = &branch_name {
        let branch = Branch::find_branch(n, None).await;
        if branch.is_none() {
            panic!("fatal: your current branch '{n}' does not have any commits yet ");
        };
    };

    let commit_hash = Head::current_commit().await.unwrap().to_string();

    let mut reachable_commits = get_reachable_commits(commit_hash.clone()).await;
    // default sort with signature time
    reachable_commits.sort_by(|a, b| b.committer.timestamp.cmp(&a.committer.timestamp));

    let ref_commits = create_reference_commit_map().await;
    let full_hash_len = commit_hash.len();

    let format_type = if args.oneline {
        FormatType::Oneline
    } else if let Some(template) = args.pretty.clone() {
        FormatType::Custom(template)
    } else {
        FormatType::Full
    };
    let formatter = CommitFormatter::new(format_type);

    let max_output_number = min(args.number.unwrap_or(usize::MAX), reachable_commits.len());
    let mut output_number = 0;
    let mut graph_state = if args.graph {
        Some(GraphState::new())
    } else {
        None
    };
    // Decide abbreviated hash length
    let default_abbrev = util::get_min_unique_hash_length(&reachable_commits).max(7);
    let abbrev_len = if args.no_abbrev_commit {
        full_hash_len
    } else if let Some(n) = args.abbrev {
        if n == 0 { default_abbrev } else { n }
    } else if args.abbrev_commit || args.oneline || args.pretty.is_some() {
        default_abbrev
    } else {
        full_hash_len
    };
    for commit in reachable_commits {
        if output_number >= max_output_number {
            break;
        }
        if !filter.passes_non_path_filters(&commit) {
            continue;
        }

        let mut cached_changes = if filter.paths.is_empty() && !name_only && !name_status {
            None
        } else {
            Some(get_changed_files_for_commit(&commit, &path_filters).await)
        };

        if !filter.matches(&commit, cached_changes.as_deref()).await {
            continue;
        }

        output_number += 1;

        let ref_msg = if decorate_option != DecorateOptions::No {
            let mut ref_msgs: Vec<String> = vec![];
            if output_number == 1 {
                ref_msgs.push(if let Some(b_name) = &branch_name {
                    format!(
                        "{} -> {}{}",
                        "HEAD".cyan(),
                        (if decorate_option == DecorateOptions::Full {
                            "refs/heads/"
                        } else {
                            ""
                        })
                        .green(),
                        b_name.green()
                    )
                } else {
                    "HEAD".cyan().to_string()
                });
            };

            let mut refs = ref_commits.get(&commit.id).cloned().unwrap_or_default();
            refs.sort();

            ref_msgs.append(
                &mut refs
                    .iter()
                    .filter_map(|r| {
                        if r.kind == ReferenceKind::Local && Some(r.name.to_owned()) == branch_name
                        {
                            None
                        } else {
                            Some(match r.kind {
                                ReferenceKind::Tag => format!(
                                    "tag: {}{}",
                                    if decorate_option == DecorateOptions::Full {
                                        "refs/tags/"
                                    } else {
                                        ""
                                    },
                                    r.name
                                )
                                .yellow()
                                .to_string(),
                                ReferenceKind::Remote => format!(
                                    "{}{}",
                                    if decorate_option == DecorateOptions::Full {
                                        "refs/remotes/"
                                    } else {
                                        ""
                                    },
                                    r.name
                                )
                                .red()
                                .to_string(),
                                ReferenceKind::Local => format!(
                                    "{}{}",
                                    if decorate_option == DecorateOptions::Full {
                                        "refs/heads/"
                                    } else {
                                        ""
                                    },
                                    r.name
                                )
                                .green()
                                .to_string(),
                            })
                        }
                    })
                    .collect(),
            );
            ref_msgs.join(", ")
        } else {
            String::new()
        };

        let graph_prefix = if let Some(ref mut gs) = graph_state {
            gs.render(&commit)
        } else {
            String::new()
        };

        let ctx = FormatContext {
            graph_prefix: &graph_prefix,
            decoration: &ref_msg,
            abbrev_len,
        };
        let mut message = formatter.format(&commit, &ctx);

        if name_only || name_status {
            if let Some(changes) = cached_changes.take()
                && !changes.is_empty()
            {
                if !message.ends_with('\n') {
                    message.push('\n');
                }
                message.push_str(&format_changes(&changes, name_status));
            }
        } else if patch {
            let patch_output = generate_diff(&commit, path_filters.clone()).await;
            if !patch_output.is_empty() {
                if !message.ends_with('\n') {
                    message.push('\n');
                }
                message.push_str(&patch_output);
            }
        } else if args.stat {
            let stats = compute_commit_stat(&commit, path_filters.clone()).await;
            let stat_output = format_stat_output(&stats);
            if !stat_output.is_empty() {
                if !message.ends_with('\n') {
                    message.push('\n');
                }
                message.push_str(&stat_output);
            }
        }

        #[cfg(unix)]
        {
            if let Some(ref mut stdin) = process.stdin {
                writeln!(stdin, "{message}").unwrap();
            } else {
                eprintln!("Failed to capture stdin");
            }
        }
        #[cfg(not(unix))]
        {
            println!("{message}");
        }
    }

    #[cfg(unix)]
    {
        let _ = process.wait().expect("failed to wait on child");
    }
}

async fn commit_touches_paths(commit: &Commit, filters: &[PathBuf]) -> bool {
    if filters.is_empty() {
        return true;
    }
    let changes = get_changed_files_for_commit(commit, filters).await;
    !changes.is_empty()
}

/// Get list of changed files for a commit
pub(crate) async fn get_changed_files_for_commit(
    commit: &Commit,
    paths: &[PathBuf],
) -> Vec<FileChange> {
    let tree = load_object::<Tree>(&commit.tree_id).unwrap();
    let new_blobs: Vec<(PathBuf, ObjectHash)> = tree.get_plain_items();

    let old_blobs: Vec<(PathBuf, ObjectHash)> = if !commit.parent_commit_ids.is_empty() {
        let parent = &commit.parent_commit_ids[0];
        let parent_hash = ObjectHash::from_str(&parent.to_string()).unwrap();
        let parent_commit = load_object::<Commit>(&parent_hash).unwrap();
        let parent_tree = load_object::<Tree>(&parent_commit.tree_id).unwrap();
        parent_tree.get_plain_items()
    } else {
        Vec::new()
    };

    let matches_filter = |path: &PathBuf, filters: &[PathBuf]| -> bool {
        if filters.is_empty() {
            return true;
        }
        filters.iter().any(|filter| util::is_sub_path(path, filter))
    };

    let old_files: HashSet<PathBuf> = old_blobs.iter().map(|(path, _)| path.clone()).collect();
    let new_files: HashSet<PathBuf> = new_blobs.iter().map(|(path, _)| path.clone()).collect();

    let mut changed_files = Vec::new();

    for file in &new_files {
        if !old_files.contains(file) && matches_filter(file, paths) {
            changed_files.push(FileChange {
                path: file.clone(),
                status: ChangeType::Added,
            });
        }
    }

    for (file, new_hash) in &new_blobs {
        if let Some((_, old_hash)) = old_blobs.iter().find(|(old_file, _)| old_file == file)
            && new_hash != old_hash
            && matches_filter(file, paths)
        {
            changed_files.push(FileChange {
                path: file.clone(),
                status: ChangeType::Modified,
            });
        }
    }

    for file in &old_files {
        if !new_files.contains(file) && matches_filter(file, paths) {
            changed_files.push(FileChange {
                path: file.clone(),
                status: ChangeType::Deleted,
            });
        }
    }

    changed_files.sort_by(|a, b| a.path.cmp(&b.path));
    changed_files
}

fn format_changes(changes: &[FileChange], include_status: bool) -> String {
    let mut out = String::new();
    for change in changes {
        if include_status {
            let status = match change.status {
                ChangeType::Added => "A",
                ChangeType::Modified => "M",
                ChangeType::Deleted => "D",
            };
            out.push_str(&format!("{}\t{}\n", status, change.path.display()));
        } else {
            out.push_str(&format!("{}\n", change.path.display()));
        }
    }
    out
}

/// Represents statistics about changes to a file in a commit.
///
/// This struct is used to report the number of lines inserted and deleted for a file
/// as part of a commit's diff. It is typically returned by functions that compute
/// per-file change statistics for a commit.
#[derive(Debug)]
pub struct FileStat {
    /// The path to the file relative to the repository root.
    pub path: String,
    /// The number of lines inserted in this file by the commit.
    pub insertions: usize,
    /// The number of lines deleted from this file by the commit.
    pub deletions: usize,
}

/// Computes file statistics (insertions and deletions) for a given commit by comparing it with its parent commit.
///
/// # Parameters
/// - `commit`: The commit to analyze.
/// - `paths`: A list of path filters (files or directories) to restrict the analysis; pass an empty vector for no filtering.
///
/// # Returns
/// A vector of [`FileStat`] structs, each containing the file path, number of insertions, and number of deletions.
pub async fn compute_commit_stat(commit: &Commit, paths: Vec<PathBuf>) -> Vec<FileStat> {
    let tree = load_object::<Tree>(&commit.tree_id).expect("failed to load tree object");
    let new_blobs: Vec<(PathBuf, ObjectHash)> = tree.get_plain_items();

    let old_blobs: Vec<(PathBuf, ObjectHash)> = if !commit.parent_commit_ids.is_empty() {
        let parent = &commit.parent_commit_ids[0];
        let parent_hash =
            ObjectHash::from_str(&parent.to_string()).expect("failed to parse parent ObjectHash");
        let parent_commit =
            load_object::<Commit>(&parent_hash).expect("failed to load parent commit object");
        let parent_tree =
            load_object::<Tree>(&parent_commit.tree_id).expect("failed to load parent tree object");
        parent_tree.get_plain_items()
    } else {
        Vec::new()
    };

    let read_content = |file: &PathBuf, hash: &ObjectHash| match load_object::<Blob>(hash) {
        Ok(blob) => blob.data,
        Err(_) => {
            let file = util::to_workdir_path(file);
            std::fs::read(&file).unwrap_or_default()
        }
    };

    let diffs = Diff::diff(
        old_blobs,
        new_blobs,
        paths.into_iter().collect(),
        read_content,
    );

    let mut stats = Vec::new();
    for diff_item in diffs {
        let mut insertions = 0;
        let mut deletions = 0;
        for line in diff_item.data.lines() {
            if line.starts_with('+') && !line.starts_with("+++") {
                insertions += 1;
            } else if line.starts_with('-') && !line.starts_with("---") {
                deletions += 1;
            }
        }
        if insertions > 0 || deletions > 0 {
            stats.push(FileStat {
                path: diff_item.path,
                insertions,
                deletions,
            });
        }
    }
    stats
}

/// Formats a list of file statistics into a Git-style summary with colored bars.
///
/// Each file is displayed on its own line, showing the file path, the total number of changes,
/// and a visual bar: green `+` for insertions and red `-` for deletions. The bar's length is
/// proportional to the number of changes, up to a maximum width. At the end, a summary line
/// shows the total number of files changed, insertions, and deletions.
///
/// If `stats` is empty, returns an empty string.
pub fn format_stat_output(stats: &[FileStat]) -> String {
    const MAX_STAT_BAR_WIDTH: usize = 40;

    if stats.is_empty() {
        return String::new();
    }

    let mut output = String::new();
    let total_insertions: usize = stats.iter().map(|s| s.insertions).sum();
    let total_deletions: usize = stats.iter().map(|s| s.deletions).sum();
    let total_files = stats.len();

    for stat in stats {
        let changes = stat.insertions + stat.deletions;
        let bar_width = if changes > MAX_STAT_BAR_WIDTH {
            MAX_STAT_BAR_WIDTH
        } else {
            changes
        };

        let plus_count = if changes > 0 {
            (stat.insertions * bar_width) / changes
        } else {
            0
        };
        let minus_count = bar_width.saturating_sub(plus_count);

        output.push_str(&format!(
            " {} | {:>3} {}{}\n",
            stat.path,
            changes,
            "+".repeat(plus_count).green(),
            "-".repeat(minus_count).red()
        ));
    }

    output.push_str(&format!(
        " {} file{} changed, {} insertion{}({}), {} deletion{}({})\n",
        total_files,
        if total_files == 1 { "" } else { "s" },
        total_insertions,
        if total_insertions == 1 { "" } else { "s" },
        "+".green(),
        total_deletions,
        if total_deletions == 1 { "" } else { "s" },
        "-".red()
    ));

    output
}

/// Maintains state for rendering an ASCII commit graph visualization.
///
/// `GraphState` tracks the columns representing active branches and parent/child relationships
/// as the commit history is traversed. It is designed to be created once and used to render
/// each commit in traversal order (e.g., topological or chronological), producing the correct
/// graph prefix for each commit line. The internal algorithm updates the columns vector to
/// reflect merges and branchings, ensuring the visual structure matches the commit graph.
#[derive(Default)]
pub struct GraphState {
    columns: Vec<Option<ObjectHash>>,
}

impl GraphState {
    /// Creates a new, empty `GraphState` for rendering a commit graph.
    pub fn new() -> Self {
        Self::default()
    }

    /// Renders the ASCII graph prefix for a given commit, updating internal state.
    ///
    /// Call this method for each commit in traversal order. It returns a string representing
    /// the graph structure (e.g., `* | |`) for the current commit, updating the internal
    /// columns to reflect parent/child relationships and merges.
    ///
    /// # Arguments
    ///
    /// * `commit` - The commit to render in the graph.
    ///
    /// # Returns
    ///
    /// A string containing the ASCII graph prefix for the commit.
    pub fn render(&mut self, commit: &Commit) -> String {
        let commit_id = commit.id;
        let parent_ids = &commit.parent_commit_ids;

        let mut prefix = String::new();

        if let Some(pos) = self.columns.iter().position(|&c| c == Some(commit_id)) {
            for (i, col) in self.columns.iter().enumerate() {
                if i == pos {
                    prefix.push_str("* ");
                } else if col.is_some() {
                    prefix.push_str("| ");
                } else {
                    prefix.push_str("  ");
                }
            }

            if parent_ids.is_empty() {
                self.columns[pos] = None;
            } else if parent_ids.len() == 1 {
                let parent_hash =
                    ObjectHash::from_str(&parent_ids[0].to_string()).unwrap_or_else(|_| {
                        panic!("failed to parse parent ObjectHash for commit {}", commit_id)
                    });
                self.columns[pos] = Some(parent_hash);
            } else {
                let first_parent = ObjectHash::from_str(&parent_ids[0].to_string())
                    .expect("failed to parse first parent ObjectHash");
                self.columns[pos] = Some(first_parent);

                for parent_id in parent_ids.iter().skip(1) {
                    let parent_hash =
                        ObjectHash::from_str(&parent_id.to_string()).unwrap_or_else(|_| {
                            panic!(
                                "failed to parse parent ObjectHash {} for commit {}",
                                parent_id, commit_id
                            )
                        });
                    self.columns.push(Some(parent_hash));
                }
            }
        } else {
            self.columns.insert(0, None);
            prefix.push_str("* ");
            for _ in 1..self.columns.len() {
                prefix.push_str("| ");
            }

            if !parent_ids.is_empty() {
                let parent_hash = ObjectHash::from_str(&parent_ids[0].to_string())
                    .expect("failed to parse parent ObjectHash");
                self.columns[0] = Some(parent_hash);

                for parent_id in parent_ids.iter().skip(1) {
                    let parent_hash =
                        ObjectHash::from_str(&parent_id.to_string()).unwrap_or_else(|_| {
                            panic!(
                                "failed to parse parent ObjectHash {} for commit {}",
                                parent_id, commit_id
                            )
                        });
                    self.columns.push(Some(parent_hash));
                }
            }
        }

        self.columns.retain(|c| c.is_some());

        prefix
    }
}

async fn create_reference_commit_map() -> HashMap<ObjectHash, Vec<Reference>> {
    let mut commit_to_refs: HashMap<ObjectHash, Vec<Reference>> = HashMap::new();

    let all_branches = Branch::list_branches(None).await;
    for branch in all_branches {
        commit_to_refs
            .entry(branch.commit)
            .or_default()
            .push(match &branch.remote {
                Some(remote) => Reference {
                    name: format!("{}/{}", remote, branch.name),
                    kind: ReferenceKind::Remote,
                },
                None => Reference {
                    name: branch.name,
                    kind: ReferenceKind::Local,
                },
            });
    }

    let all_tags = crate::internal::tag::list().await.expect("fatal: ");
    for tag in all_tags {
        let commit_id = match tag.object {
            crate::internal::tag::TagObject::Commit(c) => c.id,
            crate::internal::tag::TagObject::Tag(t) => t.object_hash,
            _ => continue,
        };
        commit_to_refs
            .entry(commit_id)
            .or_default()
            .push(Reference {
                name: tag.name,
                kind: ReferenceKind::Tag,
            });
    }

    commit_to_refs
}

/// Generate unified diff between commit and its first parent (or empty tree)
pub(crate) async fn generate_diff(commit: &Commit, paths: Vec<PathBuf>) -> String {
    // prepare old and new blobs
    // new_blobs from commit tree
    let tree = load_object::<Tree>(&commit.tree_id).unwrap();
    let new_blobs: Vec<(PathBuf, ObjectHash)> = tree.get_plain_items();

    // old_blobs from first parent if exists
    let old_blobs: Vec<(PathBuf, ObjectHash)> = if !commit.parent_commit_ids.is_empty() {
        let parent = &commit.parent_commit_ids[0];
        let parent_hash = ObjectHash::from_str(&parent.to_string()).unwrap();
        let parent_commit = load_object::<Commit>(&parent_hash).unwrap();
        let parent_tree = load_object::<Tree>(&parent_commit.tree_id).unwrap();
        parent_tree.get_plain_items()
    } else {
        Vec::new()
    };

    let read_content = |file: &PathBuf, hash: &ObjectHash| match load_object::<Blob>(hash) {
        Ok(blob) => blob.data,
        Err(_) => {
            let file = util::to_workdir_path(file);
            std::fs::read(&file).unwrap()
        }
    };

    let diffs = Diff::diff(
        old_blobs,
        new_blobs,
        paths.into_iter().collect(),
        read_content,
    );
    let mut out = String::new();
    for d in diffs {
        out.push_str(&d.data);
    }
    out
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use clap::Parser;

    use super::*;

    // Test parameter parsing
    #[test]
    fn test_log_args_name_only() {
        // Test that the --name-only parameter is parsed correctly
        let args = LogArgs::parse_from(["libra", "log", "--name-only"]);
        assert!(args.name_only);

        let args = LogArgs::parse_from(["libra", "log"]);
        assert!(!args.name_only);
    }

    #[test]
    fn test_name_only_precedence_over_patch() {
        // Test --name-only takes precedence over --patch
        let args = LogArgs::parse_from(["libra", "log", "--name-only", "--patch"]);
        assert!(args.name_only);
        assert!(args.patch);
        // In the execute function, patch should be ignored when name_only is true
    }

    #[test]
    fn test_name_only_with_oneline() {
        // Test --name-only and --oneline combination
        let args = LogArgs::parse_from(["libra", "log", "--name-only", "--oneline"]);
        assert!(args.name_only);
        assert!(args.oneline);
    }

    #[test]
    fn test_name_only_with_number_limit() {
        // Test --name-only combined with quantity limit
        let args = LogArgs::parse_from(["libra", "log", "--name-only", "-n", "5"]);
        assert!(args.name_only);
        assert_eq!(args.number, Some(5));
    }

    // Test decoration option parsing
    #[test]
    fn test_str_to_decorate_option() {
        assert_eq!(str_to_decorate_option("no").unwrap(), DecorateOptions::No);
        assert_eq!(
            str_to_decorate_option("short").unwrap(),
            DecorateOptions::Short
        );
        assert_eq!(
            str_to_decorate_option("full").unwrap(),
            DecorateOptions::Full
        );
        assert!(str_to_decorate_option("auto").is_ok());
        assert!(str_to_decorate_option("invalid").is_err());
    }

    // Test parameter combination
    #[test]
    fn test_complex_arg_combinations() {
        // Test multiple parameter combinations
        let args = LogArgs::parse_from(["libra", "log", "--name-only", "--oneline", "-n", "10"]);
        assert!(args.name_only);
        assert!(args.oneline);
        assert_eq!(args.number, Some(10));

        let args =
            LogArgs::parse_from(["libra", "log", "--name-only", "src/main.rs", "src/lib.rs"]);
        assert!(args.name_only);
        // Update expected pathspec value to include "log"
        assert_eq!(args.pathspec, vec!["log", "src/main.rs", "src/lib.rs"]);
    }

    #[test]
    fn test_new_filters_parsing() {
        let args = LogArgs::parse_from([
            "libra",
            "log",
            "--author",
            "lvy",
            "--since",
            "2025-12-19",
            "--until",
            "2025-12-19",
        ]);
        assert_eq!(args.author.as_deref(), Some("lvy"));
        assert_eq!(args.since.as_deref(), Some("2025-12-19"));
        assert_eq!(args.until.as_deref(), Some("2025-12-19"));
    }

    #[test]
    fn test_name_status_parsing() {
        let args = LogArgs::parse_from(["libra", "log", "--name-status"]);
        assert!(args.name_status);
        assert!(!args.name_only);
    }

    #[test]
    fn test_format_changes_output() {
        let changes = vec![FileChange {
            path: PathBuf::from("src/main.rs"),
            status: ChangeType::Added,
        }];
        let with_status = format_changes(&changes, true);
        assert!(with_status.contains("A\tsrc/main.rs"));

        let names_only = format_changes(&changes, false);
        assert!(names_only.contains("src/main.rs"));
        assert!(!names_only.contains("A\t"));
    }

    #[tokio::test]
    async fn test_commit_filter_author_and_time() {
        let mut commit = Commit::from_tree_id(ObjectHash::new(&[1; 20]), vec![], "msg");
        commit.author.name = "lvy".into();
        commit.author.email = "lvy@test.com".into();
        commit.committer.timestamp = 1_766_102_400; // 2025-12-19 00:00:00 UTC

        let filter = CommitFilter::new(
            Some("lvy".to_string()),
            Some(1_766_000_000),
            Some(1_766_200_000),
            Vec::new(),
        );

        assert!(filter.matches(&commit, None).await);
    }

    // Test parameter mutual exclusion logic
    #[test]
    fn test_parameter_mutual_exclusion() {
        let args = LogArgs::parse_from(["libra", "log", "--name-only", "--patch"]);

        let name_status = args.name_status;
        let name_only = args.name_only && !name_status;
        let patch = args.patch && !name_only && !name_status;

        assert!(name_only);
        assert!(!patch);

        let args = LogArgs::parse_from(["libra", "log", "--name-status", "--patch"]);
        let name_status = args.name_status;
        let name_only = args.name_only && !name_status;
        let patch = args.patch && !name_only && !name_status;

        assert!(name_status);
        assert!(!patch);
    }
}
