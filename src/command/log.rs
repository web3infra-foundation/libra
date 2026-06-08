//! Log command rendering commit history with optional decorations, filtering, and custom formatting utilities.

use std::{
    cell::RefCell,
    cmp::min,
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    rc::Rc,
    str::FromStr,
};

use clap::Parser;
use colored::Colorize;
use git_internal::{
    Diff,
    hash::ObjectHash,
    internal::object::{blob::Blob, commit::Commit, tree::Tree},
};
use regex::{Regex, RegexBuilder};
use serde::Serialize;

use crate::{
    command::load_object,
    common_utils::parse_commit_msg,
    internal::{
        branch::{Branch, BranchStoreError},
        config::{ConfigKv, validate_config_regex_pattern},
        head::Head,
        log::{
            date_parser::parse_date,
            formatter::{CommitFormatter, FormatContext, FormatType},
            pickaxe,
        },
        tag::{self, TagObject},
    },
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        object_ext::TreeExt,
        output::{OutputConfig, emit_json_data},
        pager::Pager,
        util::{self, CommitBaseError},
    },
};

const LOG_EXAMPLES: &str = "\
EXAMPLES:
    libra log -n 5                         Show the latest 5 commits
    libra log --oneline --graph            Show a compact commit graph
    libra log --author alice                Filter commits by author (case-insensitive substring)
    libra log --since 24h --until 1h       Time-window filter (relative or RFC3339)
    libra log --grep '^fix' -n 20          Filter commits by message (regex)
    libra log --merges --first-parent      Show merge commits along the first-parent line
    libra log --pretty=format:'%h %s'      Render with a Git-style custom pretty format
    libra log main..feature -- src/        Show range commits that touch src/
    libra log --follow renamed.txt         Follow one file across renames
    libra log -S 'debug_flag' -- src/      Pickaxe literal changes under src/
    libra log --name-status src/           Show changed files under src/
    libra --json log -n 1                  Structured JSON output for agents";

fn log_branch_store_error(context: &str, error: BranchStoreError) -> CliError {
    match error {
        BranchStoreError::Query(detail) => {
            CliError::fatal(format!("failed to {context}: {detail}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        }
        other => CliError::fatal(format!("failed to {context}: {other}"))
            .with_stable_code(StableErrorCode::RepoCorrupt),
    }
}

fn log_no_commits_error(branch_name: Option<&str>) -> CliError {
    let error = match branch_name {
        Some(name) => CliError::fatal(format!(
            "your current branch '{name}' does not have any commits yet"
        )),
        None => CliError::fatal("your current HEAD does not have any commits yet"),
    }
    .with_stable_code(StableErrorCode::RepoStateInvalid);

    error.with_hint("create a commit first before running 'libra log'.")
}

async fn resolve_log_head_commit() -> CliResult<(Option<String>, ObjectHash)> {
    let head = Head::current_result()
        .await
        .map_err(|error| log_branch_store_error("resolve HEAD", error))?;
    let branch_name = match head {
        Head::Branch(name) => Some(name),
        Head::Detached(_) => None,
    };

    if let Some(name) = &branch_name
        && Branch::find_branch_result(name, None)
            .await
            .map_err(|error| log_branch_store_error("inspect the current branch", error))?
            .is_none()
    {
        return Err(log_no_commits_error(Some(name)));
    }

    let current_head_commit = Head::current_commit_result()
        .await
        .map_err(|error| log_branch_store_error("resolve HEAD commit", error))?
        .ok_or_else(|| log_no_commits_error(branch_name.as_deref()))?;

    Ok((branch_name, current_head_commit))
}

fn log_invalid_object_error(object: &str) -> CliError {
    CliError::fatal(format!("invalid object name: {object}"))
        .with_stable_code(StableErrorCode::CliInvalidTarget)
        .with_hint("check the revision name and try again")
}

fn log_repo_corrupt_error(message: impl Into<String>) -> CliError {
    CliError::fatal(message.into()).with_stable_code(StableErrorCode::RepoCorrupt)
}

#[derive(Parser, Debug)]
#[command(after_help = LOG_EXAMPLES)]
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
    /// Number of hex digits for abbreviated commit hash (default: dynamically computed, min 7)
    #[clap(long, value_name = "N")]
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
    /// Filter commits by author name or email (case-insensitive substring match)
    #[clap(long, value_name = "PATTERN")]
    pub author: Option<String>,
    /// Show commits more recent than DATE (RFC3339, `YYYY-MM-DD`, or relative like `24h` / `7d`)
    #[clap(long, value_name = "DATE")]
    pub since: Option<String>,
    /// Show commits older than DATE (RFC3339, `YYYY-MM-DD`, or relative like `1h`)
    #[clap(long, value_name = "DATE")]
    pub until: Option<String>,
    /// Custom pretty format string (e.g. `%h - %s`)
    #[clap(long, value_name = "FORMAT")]
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
    #[clap(skip)]
    revision_tokens: Option<Vec<String>>,

    /// Filter commits whose message matches PATTERN (regular expression, case-sensitive by default)
    #[clap(long, value_name = "PATTERN")]
    pub grep: Option<String>,
    /// Match the `--grep` pattern case-insensitively
    #[clap(short = 'i', long = "regexp-ignore-case")]
    pub regexp_ignore_case: bool,
    /// Filter commits by committer name or email (case-insensitive substring match)
    #[clap(long, value_name = "PATTERN")]
    pub committer: Option<String>,
    /// Show only merge commits (two or more parents); alias for `--min-parents=2`
    #[clap(long)]
    pub merges: bool,
    /// Show only non-merge commits (fewer than two parents); alias for `--max-parents=1`
    #[clap(long = "no-merges")]
    pub no_merges: bool,
    /// Show only commits with at least N parents
    #[clap(long, value_name = "N")]
    pub min_parents: Option<usize>,
    /// Show only commits with at most N parents
    #[clap(long, value_name = "N")]
    pub max_parents: Option<usize>,
    /// Follow only the first parent of each merge commit when walking history
    #[clap(long)]
    pub first_parent: bool,
    /// Pickaxe: show commits that change the number of occurrences of the literal STRING
    #[clap(short = 'S', value_name = "STRING", conflicts_with = "pickaxe_regex")]
    pub pickaxe_string: Option<String>,
    /// Show commits with an added or removed line matching the REGEX
    #[clap(short = 'G', value_name = "REGEX")]
    pub pickaxe_regex: Option<String>,
    /// Follow a single file's history across renames
    #[clap(long)]
    pub follow: bool,
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

#[derive(Debug)]
struct SelectedLogCommit {
    commit: Commit,
    cached_changes: Option<Vec<FileChange>>,
    path_filters: Vec<PathBuf>,
    follow_rename: Option<FollowRename>,
}

#[derive(Debug, Clone)]
struct FollowRename {
    from: PathBuf,
    to: PathBuf,
    score: u32,
}

struct FollowCommitChanges {
    changes: Vec<FileChange>,
    previous_path: Option<PathBuf>,
    rename: Option<FollowRename>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LogFileChange {
    pub path: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LogCommitEntry {
    pub hash: String,
    pub short_hash: String,
    pub author_name: String,
    pub author_email: String,
    pub author_date: String,
    pub committer_name: String,
    pub committer_email: String,
    pub committer_date: String,
    pub subject: String,
    pub body: String,
    pub parents: Vec<String>,
    pub refs: Vec<String>,
    pub files: Vec<LogFileChange>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LogOutput {
    pub commits: Vec<LogCommitEntry>,
    pub total: Option<usize>,
}

/// A pickaxe content filter (`-S` / `-G`). The two have different semantics, so
/// they are distinct variants (see [`pickaxe`]).
enum Pickaxe {
    /// `-S`: literal-string occurrence-count difference across full blobs.
    String(Vec<u8>),
    /// `-G`: regex match against added/removed diff lines.
    Regex(Regex),
}

struct CommitFilter {
    author: Option<String>,
    committer: Option<String>,
    since: Option<i64>,
    until: Option<i64>,
    paths: Vec<PathBuf>,
    grep: Option<Regex>,
    min_parents: Option<usize>,
    max_parents: Option<usize>,
    /// When set, only commits whose OID is in this set (the first-parent chain
    /// from HEAD) pass. `None` means no `--first-parent` restriction.
    first_parents: Option<HashSet<String>>,
    /// `-S`/`-G` content filter (requires a per-commit diff; checked in the
    /// async [`CommitFilter::matches`] path).
    pickaxe: Option<Pickaxe>,
}

/// A positional revision-range expression (only parsed when explicit range/
/// exclude syntax is present among the positionals).
enum RevRange {
    /// `^A B` / `A..B`: `union(reachable(positives)) - union(reachable(negatives))`.
    Include {
        positives: Vec<String>,
        negatives: Vec<String>,
    },
    /// `A...B`: commits reachable from exactly one of `A`, `B` (symmetric difference).
    Symmetric { a: String, b: String },
}

/// Returns whether any positional token uses explicit revision-range/exclude
/// syntax (`..`, `...`, or a leading `^`). Bare tokens never trigger this, so
/// pathspec-only invocations keep their existing behavior.
fn token_has_rev_range_syntax(token: &str) -> bool {
    if token.starts_with('^') {
        return true;
    }
    if let Some((left, right)) = token.split_once("...").or_else(|| token.split_once("..")) {
        return !left.ends_with(['/', '\\']) && !right.starts_with(['/', '\\']);
    }
    false
}

fn has_rev_range_syntax(tokens: &[String]) -> bool {
    tokens.iter().any(|token| token_has_rev_range_syntax(token))
}

pub fn apply_pathspec_separator(args: &mut LogArgs, command_argv: &[String]) {
    let Some(separator_index) = command_argv.iter().position(|arg| arg == "--") else {
        return;
    };
    let trailing_pathspecs = command_argv[separator_index + 1..].to_vec();
    if trailing_pathspecs.len() > args.pathspec.len() {
        return;
    }
    let revision_count = args.pathspec.len() - trailing_pathspecs.len();
    args.revision_tokens = Some(args.pathspec[..revision_count].to_vec());
    args.pathspec = trailing_pathspecs;
}

struct LogPositionals {
    rev_range: Option<RevRange>,
    path_filters: Vec<PathBuf>,
    explicit_revision_query: bool,
}

fn parse_log_positionals(args: &LogArgs) -> CliResult<LogPositionals> {
    let has_separator = args.revision_tokens.is_some();
    let revision_tokens = args.revision_tokens.as_deref().unwrap_or(&args.pathspec);
    let explicit_revision_query =
        (has_separator && !revision_tokens.is_empty()) || has_rev_range_syntax(revision_tokens);

    let rev_range = explicit_revision_query
        .then(|| parse_rev_range(revision_tokens))
        .transpose()?;
    let path_filters = if has_separator || !explicit_revision_query {
        normalize_log_pathspecs(&args.pathspec)?
    } else {
        Vec::new()
    };

    Ok(LogPositionals {
        rev_range,
        path_filters,
        explicit_revision_query,
    })
}

fn invalid_pathspec_error(raw: &str, reason: &str) -> CliError {
    CliError::command_usage(format!("invalid pathspec '{raw}': {reason}"))
        .with_stable_code(StableErrorCode::CliInvalidArguments)
        .with_hint("pathspecs must be relative paths that stay inside the working tree; use `--` to separate paths from revisions.")
}

fn is_windows_absolute_path(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    raw.starts_with("\\\\")
        || raw.starts_with('\\')
        || (bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic())
}

fn validate_log_pathspec_boundary(raw: &str) -> CliResult<()> {
    if Path::new(raw).is_absolute() || is_windows_absolute_path(raw) {
        return Err(invalid_pathspec_error(
            raw,
            "absolute paths are not accepted",
        ));
    }

    let mut depth = 0usize;
    for segment in raw.split(['/', '\\']) {
        match segment {
            "" | "." => {}
            ".." => {
                if depth == 0 {
                    return Err(invalid_pathspec_error(
                        raw,
                        "parent components escape the working tree",
                    ));
                }
                depth -= 1;
            }
            _ => depth += 1,
        }
    }
    Ok(())
}

fn normalize_log_pathspec(raw: &str) -> CliResult<PathBuf> {
    validate_log_pathspec_boundary(raw)?;
    let mut normalized = PathBuf::new();
    for segment in raw.split(['/', '\\']) {
        match segment {
            "" | "." => {}
            ".." => {
                normalized.pop();
            }
            _ => normalized.push(segment),
        }
    }
    if normalized.as_os_str().is_empty() {
        Ok(PathBuf::from("."))
    } else {
        Ok(normalized)
    }
}

fn normalize_log_pathspecs(tokens: &[String]) -> CliResult<Vec<PathBuf>> {
    tokens
        .iter()
        .map(|token| normalize_log_pathspec(token))
        .collect()
}

fn resolve_follow_path(args: &LogArgs, positionals: &LogPositionals) -> CliResult<Option<PathBuf>> {
    if !args.follow {
        return Ok(None);
    }
    match positionals.path_filters.as_slice() {
        [path] if path != Path::new(".") => Ok(Some(path.clone())),
        _ => Err(
            CliError::command_usage("log --follow requires exactly one file pathspec")
                .with_stable_code(StableErrorCode::CliInvalidArguments)
                .with_hint("use `libra log --follow -- <path>` with one file path."),
        ),
    }
}

/// An empty range endpoint defaults to `HEAD` (so `..B` and `A..` work).
fn rev_endpoint(token: &str) -> String {
    if token.is_empty() {
        "HEAD".to_string()
    } else {
        token.to_string()
    }
}

fn rev_range_unsupported(token: &str) -> CliError {
    CliError::command_usage(format!(
        "unsupported revision range expression '{token}': `A...B` must be the only argument"
    ))
    .with_stable_code(StableErrorCode::CliInvalidArguments)
}

/// Parses the positional tokens into a [`RevRange`]. The symmetric form
/// `A...B` must stand alone; otherwise tokens combine as positive refs, `^X`
/// exclusions, and `A..B` (negative `A`, positive `B`).
fn parse_rev_range(tokens: &[String]) -> CliResult<RevRange> {
    if tokens.len() == 1
        && let Some((a, b)) = tokens[0].split_once("...")
    {
        return Ok(RevRange::Symmetric {
            a: rev_endpoint(a),
            b: rev_endpoint(b),
        });
    }

    let mut positives = Vec::new();
    let mut negatives = Vec::new();
    for token in tokens {
        if token.contains("...") {
            return Err(rev_range_unsupported(token));
        } else if let Some((a, b)) = token.split_once("..") {
            negatives.push(rev_endpoint(a));
            positives.push(rev_endpoint(b));
        } else if let Some(rest) = token.strip_prefix('^') {
            negatives.push(rest.to_string());
        } else {
            positives.push(token.clone());
        }
    }
    if positives.is_empty() {
        // `^A` with no positive ref defaults the positive side to HEAD.
        positives.push("HEAD".to_string());
    }
    Ok(RevRange::Include {
        positives,
        negatives,
    })
}

/// Maps a revision-resolution failure to a `libra log` CLI error (mirrors
/// `rev-list`): unknown refs are `CliInvalidTarget` (exit 129).
fn log_rev_target_error(spec: &str, error: CommitBaseError) -> CliError {
    match error {
        CommitBaseError::HeadUnborn => CliError::failure(format!(
            "not a valid object name: '{spec}' (HEAD does not point to a commit)"
        ))
        .with_stable_code(StableErrorCode::CliInvalidTarget)
        .with_hint("create a commit before resolving HEAD."),
        CommitBaseError::InvalidReference(detail) => {
            CliError::failure(format!("not a valid object name: '{spec}' ({detail})"))
                .with_stable_code(StableErrorCode::CliInvalidTarget)
        }
        CommitBaseError::ReadFailure(detail) => {
            CliError::fatal(format!("failed to resolve '{spec}': {detail}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        }
        CommitBaseError::CorruptReference(detail) => {
            CliError::fatal(format!("failed to resolve '{spec}': {detail}"))
                .with_stable_code(StableErrorCode::RepoCorrupt)
        }
    }
}

/// Resolves `spec` and returns the commits reachable from it.
async fn reachable_commits_for_spec(spec: &str) -> CliResult<Vec<Commit>> {
    let commit = util::get_commit_base_typed(spec)
        .await
        .map_err(|e| log_rev_target_error(spec, e))?;
    get_reachable_commits(commit.to_string(), None).await
}

/// Computes the commit set selected by a [`RevRange`].
async fn resolve_rev_range(range: &RevRange) -> CliResult<Vec<Commit>> {
    match range {
        RevRange::Symmetric { a, b } => {
            let ra = reachable_commits_for_spec(a).await?;
            let rb = reachable_commits_for_spec(b).await?;
            let a_oids: HashSet<String> = ra.iter().map(|c| c.id.to_string()).collect();
            let b_oids: HashSet<String> = rb.iter().map(|c| c.id.to_string()).collect();
            let mut seen: HashSet<String> = HashSet::new();
            let mut out = Vec::new();
            for commit in ra.into_iter().chain(rb) {
                let oid = commit.id.to_string();
                // Reachable from exactly one side.
                if (a_oids.contains(&oid) ^ b_oids.contains(&oid)) && seen.insert(oid) {
                    out.push(commit);
                }
            }
            Ok(out)
        }
        RevRange::Include {
            positives,
            negatives,
        } => {
            let mut excluded: HashSet<String> = HashSet::new();
            for negative in negatives {
                for commit in reachable_commits_for_spec(negative).await? {
                    excluded.insert(commit.id.to_string());
                }
            }
            let mut seen: HashSet<String> = HashSet::new();
            let mut included = Vec::new();
            for positive in positives {
                for commit in reachable_commits_for_spec(positive).await? {
                    let oid = commit.id.to_string();
                    if !excluded.contains(&oid) && seen.insert(oid) {
                        included.push(commit);
                    }
                }
            }
            Ok(included)
        }
    }
}

/// Effective minimum parent count: explicit `--min-parents`, else `2` when
/// `--merges` is set (git treats `--merges` as `--min-parents=2`).
fn effective_min_parents(args: &LogArgs) -> Option<usize> {
    args.min_parents.or(args.merges.then_some(2))
}

/// Effective maximum parent count: explicit `--max-parents`, else `1` when
/// `--no-merges` is set (git treats `--no-merges` as `--max-parents=1`).
fn effective_max_parents(args: &LogArgs) -> Option<usize> {
    args.max_parents.or(args.no_merges.then_some(1))
}

/// Compiles a `--grep` pattern into a [`Regex`], enforcing the shared 4 KiB
/// pattern-length cap and mapping every failure to `LBR-CLI-002` (exit 129),
/// mirroring `libra grep`.
fn compile_grep_regex(pattern: &str, ignore_case: bool) -> CliResult<Regex> {
    validate_config_regex_pattern(pattern).map_err(|e| {
        CliError::command_usage(format!("invalid --grep pattern: {e}"))
            .with_stable_code(StableErrorCode::CliInvalidArguments)
    })?;
    RegexBuilder::new(pattern)
        .case_insensitive(ignore_case)
        // Match `^`/`$` at line boundaries within the multi-line commit message,
        // like `git log --grep` (so e.g. `^fix` matches the subject line and
        // `^Signed-off-by` matches a footer).
        .multi_line(true)
        .build()
        .map_err(|e| {
            CliError::command_usage(format!("invalid --grep regex '{pattern}': {e}"))
                .with_stable_code(StableErrorCode::CliInvalidArguments)
        })
}

/// Builds the set of commit OIDs on the first-parent chain from `head`, using
/// the already-loaded reachable `commits` to look up each commit's first parent.
fn first_parent_oids(head: &ObjectHash, commits: &[Commit]) -> HashSet<String> {
    let first_parent: HashMap<String, Option<String>> = commits
        .iter()
        .map(|c| {
            (
                c.id.to_string(),
                c.parent_commit_ids.first().map(|p| p.to_string()),
            )
        })
        .collect();

    let mut chain = HashSet::new();
    let mut current = Some(head.to_string());
    while let Some(oid) = current {
        if !chain.insert(oid.clone()) {
            break; // cycle guard
        }
        current = first_parent.get(&oid).cloned().flatten();
    }
    chain
}

impl CommitFilter {
    #[allow(clippy::too_many_arguments)]
    fn new(
        author: Option<String>,
        committer: Option<String>,
        since: Option<i64>,
        until: Option<i64>,
        paths: Vec<PathBuf>,
        // Pre-compiled so the regex failure (the only fallible part) happens
        // *before* history traversal — see the call sites.
        grep: Option<Regex>,
        min_parents: Option<usize>,
        max_parents: Option<usize>,
        first_parents: Option<HashSet<String>>,
        pickaxe: Option<Pickaxe>,
    ) -> Self {
        Self {
            author: author.map(|s| s.to_lowercase()),
            committer: committer.map(|s| s.to_lowercase()),
            since,
            until,
            paths,
            grep,
            min_parents,
            max_parents,
            first_parents,
            pickaxe,
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

        if let Some(committer_filter) = &self.committer {
            let committer = format!(
                "{} <{}>",
                commit.committer.name.to_lowercase(),
                commit.committer.email.to_lowercase()
            );
            if !committer.contains(committer_filter) {
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

        let parent_count = commit.parent_commit_ids.len();
        if let Some(min) = self.min_parents
            && parent_count < min
        {
            return false;
        }
        if let Some(max) = self.max_parents
            && parent_count > max
        {
            return false;
        }

        if let Some(chain) = &self.first_parents
            && !chain.contains(&commit.id.to_string())
        {
            return false;
        }

        if let Some(pattern) = &self.grep
            && !pattern.is_match(&commit.message)
        {
            return false;
        }

        true
    }

    async fn matches_paths(
        &self,
        commit: &Commit,
        cached_changes: Option<&[FileChange]>,
    ) -> Result<bool, CliError> {
        if self.paths.is_empty() {
            return Ok(true);
        }

        if let Some(changes) = cached_changes {
            Ok(!changes.is_empty())
        } else {
            commit_touches_paths(commit, &self.paths).await
        }
    }

    /// Pickaxe (`-S`/`-G`) predicate. Requires a per-commit diff, so it runs in
    /// the async path. Object-load/diff failures surface as `RepoCorrupt` (128)
    /// rather than silently excluding the commit; only a clean "no match"
    /// excludes it.
    fn matches_pickaxe(&self, commit: &Commit) -> Result<bool, CliError> {
        self.matches_pickaxe_with_paths(commit, &self.paths)
    }

    fn matches_pickaxe_with_paths(
        &self,
        commit: &Commit,
        paths: &[PathBuf],
    ) -> Result<bool, CliError> {
        let Some(pickaxe) = &self.pickaxe else {
            return Ok(true);
        };
        match pickaxe {
            Pickaxe::String(needle) => commit_pickaxe_string(commit, needle, paths),
            Pickaxe::Regex(re) => commit_pickaxe_regex(commit, re, paths),
        }
    }

    async fn matches(
        &self,
        commit: &Commit,
        cached_changes: Option<&[FileChange]>,
    ) -> Result<bool, CliError> {
        if !self.passes_non_path_filters(commit) {
            return Ok(false);
        }

        if !self.matches_paths(commit, cached_changes).await? {
            return Ok(false);
        }

        self.matches_pickaxe(commit)
    }
}

/// Builds the optional pickaxe filter from `--S`/`-G`, compiling the `-G` regex
/// up front (4 KiB cap, `LBR-CLI-002` on failure) so an invalid pattern fails
/// before any history traversal.
fn build_pickaxe(args: &LogArgs) -> CliResult<Option<Pickaxe>> {
    if let Some(string) = &args.pickaxe_string {
        Ok(Some(Pickaxe::String(string.clone().into_bytes())))
    } else if let Some(pattern) = &args.pickaxe_regex {
        Ok(Some(Pickaxe::Regex(compile_grep_regex(pattern, false)?)))
    } else {
        Ok(None)
    }
}

/// A list of `(path, blob-oid)` entries for one tree snapshot.
type BlobList = Vec<(PathBuf, ObjectHash)>;

/// Loads a commit's child-side and first-parent-side blob lists
/// (`(path, oid)`), mirroring `get_changed_files_for_commit`'s tree loading.
fn load_commit_blob_vecs(commit: &Commit) -> Result<(BlobList, BlobList), CliError> {
    let tree = load_object::<Tree>(&commit.tree_id)
        .map_err(|e| log_repo_corrupt_error(format!("failed to load tree object: {e}")))?;
    let new_blobs: Vec<(PathBuf, ObjectHash)> = tree.get_plain_items();

    let shallow_boundaries = crate::command::fetch::read_shallow_boundaries().unwrap_or_default();
    let has_parents = !commit.parent_commit_ids.is_empty()
        && !shallow_boundaries.contains(&commit.id.to_string());

    let old_blobs: Vec<(PathBuf, ObjectHash)> = if has_parents {
        let parent = &commit.parent_commit_ids[0];
        let parent_commit = load_object::<Commit>(parent)
            .map_err(|e| log_repo_corrupt_error(format!("failed to load parent commit: {e}")))?;
        let parent_tree = load_object::<Tree>(&parent_commit.tree_id)
            .map_err(|e| log_repo_corrupt_error(format!("failed to load parent tree: {e}")))?;
        parent_tree.get_plain_items()
    } else {
        Vec::new()
    };

    Ok((old_blobs, new_blobs))
}

fn pickaxe_path_in_scope(path: &Path, filters: &[PathBuf]) -> bool {
    filters.is_empty() || filters.iter().any(|filter| util::is_sub_path(path, filter))
}

/// `-S`: a commit matches when some changed (in-scope) file has a different
/// **occurrence count** of `needle` between its parent-side and child-side full
/// blob. Unchanged files (and changes that leave the count equal) do not match.
fn commit_pickaxe_string(
    commit: &Commit,
    needle: &[u8],
    paths: &[PathBuf],
) -> Result<bool, CliError> {
    let (old_blobs, new_blobs) = load_commit_blob_vecs(commit)?;
    let old_map: HashMap<PathBuf, ObjectHash> = old_blobs.into_iter().collect();
    let new_map: HashMap<PathBuf, ObjectHash> = new_blobs.into_iter().collect();

    let mut considered: HashSet<&PathBuf> = HashSet::new();
    considered.extend(old_map.keys());
    considered.extend(new_map.keys());

    for path in considered {
        if !pickaxe_path_in_scope(path, paths) {
            continue;
        }
        let old_oid = old_map.get(path);
        let new_oid = new_map.get(path);
        if old_oid == new_oid {
            // File unchanged (same blob): occurrence count is identical.
            continue;
        }
        let old_count = match old_oid {
            Some(hash) => pickaxe::count_occurrences(&load_commit_blob_content(hash)?, needle),
            None => 0,
        };
        let new_count = match new_oid {
            Some(hash) => pickaxe::count_occurrences(&load_commit_blob_content(hash)?, needle),
            None => 0,
        };
        if old_count != new_count {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `-G`: a commit matches when any added/removed diff line (within scope) of any
/// changed file matches `re`.
fn commit_pickaxe_regex(commit: &Commit, re: &Regex, paths: &[PathBuf]) -> Result<bool, CliError> {
    let (old_blobs, new_blobs) = load_commit_blob_vecs(commit)?;
    let diffs = build_commit_diff_items(old_blobs, new_blobs, paths.to_vec())?;
    Ok(diffs
        .iter()
        .any(|item| pickaxe::diff_line_matches(&item.data, re)))
}

fn map_by_path(blobs: BlobList) -> HashMap<PathBuf, ObjectHash> {
    blobs.into_iter().collect()
}

fn file_change_for_path(
    path: &Path,
    old_hash: Option<&ObjectHash>,
    new_hash: Option<&ObjectHash>,
) -> Option<FileChange> {
    match (old_hash, new_hash) {
        (None, Some(_)) => Some(FileChange {
            path: path.to_path_buf(),
            status: ChangeType::Added,
        }),
        (Some(_), None) => Some(FileChange {
            path: path.to_path_buf(),
            status: ChangeType::Deleted,
        }),
        (Some(old), Some(new)) if old != new => Some(FileChange {
            path: path.to_path_buf(),
            status: ChangeType::Modified,
        }),
        _ => None,
    }
}

fn best_follow_rename(
    current_path: &Path,
    old_map: &HashMap<PathBuf, ObjectHash>,
    new_map: &HashMap<PathBuf, ObjectHash>,
) -> Result<Option<FollowRename>, CliError> {
    let Some(new_hash) = new_map.get(current_path) else {
        return Ok(None);
    };
    if old_map.contains_key(current_path) {
        return Ok(None);
    }

    let mut best: Option<(PathBuf, f64)> = None;
    let new_content = load_commit_blob_content(new_hash)?;
    for (old_path, old_hash) in old_map {
        if new_map.contains_key(old_path) {
            continue;
        }
        let similarity = if old_hash == new_hash {
            1.0
        } else {
            crate::utils::blob_similarity::blob_line_similarity(
                &load_commit_blob_content(old_hash)?,
                &new_content,
            )
        };
        if similarity >= 0.5 && best.as_ref().is_none_or(|(_, score)| similarity > *score) {
            best = Some((old_path.clone(), similarity));
        }
    }

    Ok(best.map(|(from, similarity)| FollowRename {
        from,
        to: current_path.to_path_buf(),
        score: ((similarity * 100.0).round() as u32).min(100),
    }))
}

fn follow_changes_for_commit(
    commit: &Commit,
    current_path: &Path,
) -> Result<FollowCommitChanges, CliError> {
    let (old_blobs, new_blobs) = load_commit_blob_vecs(commit)?;
    let old_map = map_by_path(old_blobs);
    let new_map = map_by_path(new_blobs);
    if let Some(rename) = best_follow_rename(current_path, &old_map, &new_map)? {
        return Ok(FollowCommitChanges {
            changes: vec![
                FileChange {
                    path: rename.from.clone(),
                    status: ChangeType::Deleted,
                },
                FileChange {
                    path: rename.to.clone(),
                    status: ChangeType::Added,
                },
            ],
            previous_path: Some(rename.from.clone()),
            rename: Some(rename),
        });
    }

    Ok(FollowCommitChanges {
        changes: file_change_for_path(
            current_path,
            old_map.get(current_path),
            new_map.get(current_path),
        )
        .into_iter()
        .collect(),
        previous_path: None,
        rename: None,
    })
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

    if let Some(config_deco) = ConfigKv::get("log.decorate")
        .await
        .ok()
        .flatten()
        .map(|e| e.value)
        .and_then(|s| str_to_decorate_option(&s).ok())
    {
        Ok(config_deco)
    } else {
        str_to_decorate_option("auto")
    }
}

/// Get all reachable commits from the given commit hash, up to a specified depth.
/// **didn't consider the order of the commits**
pub async fn get_reachable_commits(
    commit_hash: String,
    depth: Option<usize>,
) -> Result<Vec<Commit>, CliError> {
    let mut queue = VecDeque::new();
    let mut commit_set: HashSet<ObjectHash> = HashSet::new();
    let mut reachable_commits: Vec<Commit> = Vec::new();

    // Push the initial commit with depth 0
    let initial_hash =
        ObjectHash::from_str(&commit_hash).map_err(|_| log_invalid_object_error(&commit_hash))?;
    queue.push_back((initial_hash, 0)); // (commit_id, current_depth)

    let shallow_boundaries = crate::command::fetch::read_shallow_boundaries().unwrap_or_default();

    while let Some((commit_id, current_depth)) = queue.pop_front() {
        // If we've already seen this commit, skip it
        if !commit_set.insert(commit_id) {
            continue;
        }

        let commit = load_object::<Commit>(&commit_id).map_err(|e| {
            log_repo_corrupt_error(format!("storage broken, object not found: {e}"))
        })?;

        // If depth is limited and the current depth exceeds the limit, skip further processing
        if let Some(max_depth) = depth
            && current_depth >= max_depth
        {
            continue;
        }

        // Add parent commits to the queue with incremented depth (if not a shallow boundary)
        if !shallow_boundaries.contains(&commit_id.to_string()) {
            for parent_commit_id in &commit.parent_commit_ids {
                queue.push_back((*parent_commit_id, current_depth + 1));
            }
        }

        // Add the current commit to the result list
        reachable_commits.push(commit);
    }
    Ok(reachable_commits)
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

fn parse_date_arg(value: &str) -> CliResult<i64> {
    parse_date(value).map_err(|e| {
        CliError::command_usage(e.to_string())
            .with_stable_code(StableErrorCode::CliInvalidArguments)
            .with_hint(r#"supported formats: YYYY-MM-DD, "N days ago", unix timestamp"#)
    })
}

async fn resolve_decorate_option(args: &LogArgs) -> CliResult<DecorateOptions> {
    determine_decorate_option(args).await.map_err(|value| {
        CliError::command_usage(format!("invalid --decorate option: {value}"))
            .with_stable_code(StableErrorCode::CliInvalidArguments)
            .with_hint("valid options: no, short, full, auto")
    })
}

pub async fn execute(args: LogArgs) {
    if let Err(err) = execute_safe(args, &OutputConfig::default()).await {
        err.print_stderr();
    }
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting. Walks commit history applying filters (date range,
/// author, path) and renders formatted log output.
pub async fn execute_safe(args: LogArgs, output: &OutputConfig) -> CliResult<()> {
    let decorate_option = resolve_decorate_option(&args).await?;

    if output.is_json() {
        let result = run_log(&args).await?;
        return emit_json_data("log", &result, output);
    }

    let name_status = args.name_status;
    // Check parameter mutual exclusion: if both name flags and --patch are specified, prioritize the name display flags
    let name_only = args.name_only && !name_status;
    let patch = args.patch && !name_only && !name_status;

    let since = args.since.as_deref().map(parse_date_arg).transpose()?;
    let until = args.until.as_deref().map(parse_date_arg).transpose()?;
    let positionals = parse_log_positionals(&args)?;
    let follow_path = resolve_follow_path(&args, &positionals)?;
    // Compile `--grep` before any traversal so an invalid/oversized pattern
    // fails fast (`LBR-CLI-002`) instead of after loading the whole history.
    let grep = args
        .grep
        .as_deref()
        .map(|p| compile_grep_regex(p, args.regexp_ignore_case))
        .transpose()?;
    let pickaxe = build_pickaxe(&args)?;

    let (branch_name, current_head_commit) = resolve_log_head_commit().await?;
    let commit_hash = current_head_commit.to_string();

    let mut reachable_commits = match &positionals.rev_range {
        Some(range) => resolve_rev_range(range).await?,
        None => get_reachable_commits(commit_hash.clone(), None).await?,
    };
    // newest first
    reachable_commits.sort_by_key(|b| std::cmp::Reverse(b.committer.timestamp));
    let default_abbrev = util::get_min_unique_hash_length(&reachable_commits).max(7);

    // `--first-parent` restricts to the first-parent chain (computed from the
    // already-loaded history so the shared `get_reachable_commits` is untouched).
    // It is anchored at HEAD, so it is skipped for an explicit revision range.
    let first_parents = (args.first_parent && !positionals.explicit_revision_query)
        .then(|| first_parent_oids(&current_head_commit, &reachable_commits));
    let filter = CommitFilter::new(
        args.author.clone(),
        args.committer.clone(),
        since,
        until,
        positionals.path_filters.clone(),
        grep,
        effective_min_parents(&args),
        effective_max_parents(&args),
        first_parents,
        pickaxe,
    );

    let max_output_number = min(args.number.unwrap_or(usize::MAX), reachable_commits.len());
    let reuse_changed_files = name_only || name_status;
    let (selected_commits, _) = select_log_commits(
        reachable_commits,
        &filter,
        &positionals.path_filters,
        max_output_number,
        reuse_changed_files,
        follow_path,
        false,
    )
    .await?;

    if output.quiet {
        return validate_selected_log_commits(
            &selected_commits,
            &positionals.path_filters,
            name_only,
            name_status,
            patch,
            args.stat,
        )
        .await;
    }

    let mut pager = Pager::with_config(output)?;

    let ref_commits = if decorate_option == DecorateOptions::No {
        HashMap::new()
    } else {
        create_reference_commit_map().await
    };
    let full_hash_len = commit_hash.len();

    let format_type = if args.oneline {
        FormatType::Oneline
    } else if let Some(template) = args.pretty.clone() {
        FormatType::Custom(template)
    } else {
        FormatType::Full
    };
    let formatter = CommitFormatter::new(format_type);

    let mut graph_state = if args.graph {
        Some(GraphState::new())
    } else {
        None
    };
    // Decide abbreviated hash length
    let abbrev_len = if args.no_abbrev_commit {
        full_hash_len
    } else if let Some(n) = args.abbrev {
        if n == 0 { default_abbrev } else { n }
    } else if args.abbrev_commit || args.oneline || args.pretty.is_some() {
        default_abbrev
    } else {
        full_hash_len
    };
    for (index, selected) in selected_commits.into_iter().enumerate() {
        let SelectedLogCommit {
            commit,
            mut cached_changes,
            path_filters,
            follow_rename,
        } = selected;
        let ref_msg = if decorate_option != DecorateOptions::No {
            let mut ref_msgs: Vec<String> = vec![];
            if index == 0 {
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
                message.push_str(&format_changes(
                    &changes,
                    name_status,
                    follow_rename.as_ref(),
                ));
            }
        } else if patch {
            let patch_output = generate_diff(&commit, path_filters.clone()).await?;
            if !patch_output.is_empty() {
                if !message.ends_with('\n') {
                    message.push('\n');
                }
                message.push_str(&patch_output);
            }
        } else if args.stat {
            let stats = compute_commit_stat(&commit, path_filters.clone()).await?;
            let stat_output = format_stat_output(&stats);
            if !stat_output.is_empty() {
                if !message.ends_with('\n') {
                    message.push('\n');
                }
                message.push_str(&stat_output);
            }
        }

        pager.write_line(&message)?;
    }

    pager.finish()?;
    Ok(())
}

async fn run_log(args: &LogArgs) -> CliResult<LogOutput> {
    let since = args.since.as_deref().map(parse_date_arg).transpose()?;
    let until = args.until.as_deref().map(parse_date_arg).transpose()?;
    let positionals = parse_log_positionals(args)?;
    let follow_path = resolve_follow_path(args, &positionals)?;
    let grep = args
        .grep
        .as_deref()
        .map(|p| compile_grep_regex(p, args.regexp_ignore_case))
        .transpose()?;
    let pickaxe = build_pickaxe(args)?;

    let (branch_name, current_head_commit) = resolve_log_head_commit().await?;
    let commit_hash = current_head_commit.to_string();

    let mut reachable_commits = match &positionals.rev_range {
        Some(range) => resolve_rev_range(range).await?,
        None => get_reachable_commits(commit_hash, None).await?,
    };
    // newest first
    reachable_commits.sort_by_key(|b| std::cmp::Reverse(b.committer.timestamp));

    let first_parents = (args.first_parent && !positionals.explicit_revision_query)
        .then(|| first_parent_oids(&current_head_commit, &reachable_commits));
    let filter = CommitFilter::new(
        args.author.clone(),
        args.committer.clone(),
        since,
        until,
        positionals.path_filters.clone(),
        grep,
        effective_min_parents(args),
        effective_max_parents(args),
        first_parents,
        pickaxe,
    );

    let max_output_number = min(args.number.unwrap_or(usize::MAX), reachable_commits.len());
    let include_total = args.number.is_none();
    let ref_commits = create_reference_commit_map().await;
    let (selected_commits, total) = select_log_commits(
        reachable_commits,
        &filter,
        &positionals.path_filters,
        max_output_number,
        true,
        follow_path,
        include_total,
    )
    .await?;
    let mut commits = Vec::new();
    for selected in selected_commits {
        let SelectedLogCommit {
            commit,
            cached_changes,
            ..
        } = selected;
        let files = cached_changes.unwrap_or_default();

        let (parsed_message, _) = parse_commit_msg(&commit.message);
        let mut message_lines = parsed_message.lines();
        let subject = message_lines.next().unwrap_or("").to_string();
        let body = message_lines.collect::<Vec<_>>().join("\n");
        let hash = commit.id.to_string();
        let short_hash = hash.get(..7).unwrap_or(&hash).to_string();

        commits.push(LogCommitEntry {
            hash,
            short_hash,
            author_name: commit.author.name.trim().to_string(),
            author_email: commit.author.email.trim().to_string(),
            author_date: format_log_timestamp(commit.author.timestamp as i64),
            committer_name: commit.committer.name.trim().to_string(),
            committer_email: commit.committer.email.trim().to_string(),
            committer_date: format_log_timestamp(commit.committer.timestamp as i64),
            subject,
            body,
            parents: commit
                .parent_commit_ids
                .iter()
                .map(ToString::to_string)
                .collect(),
            refs: collect_log_refs(
                &commit,
                &ref_commits,
                branch_name.as_deref(),
                Some(current_head_commit),
            ),
            files: files
                .into_iter()
                .map(|file| LogFileChange {
                    path: file.path.display().to_string(),
                    status: match file.status {
                        ChangeType::Added => "added",
                        ChangeType::Modified => "modified",
                        ChangeType::Deleted => "deleted",
                    }
                    .to_string(),
                })
                .collect(),
        });
    }

    Ok(LogOutput {
        commits,
        total: include_total.then_some(total),
    })
}

async fn select_log_commits(
    reachable_commits: Vec<Commit>,
    filter: &CommitFilter,
    path_filters: &[PathBuf],
    max_output_number: usize,
    keep_changed_files: bool,
    follow_path: Option<PathBuf>,
    count_all_matches: bool,
) -> Result<(Vec<SelectedLogCommit>, usize), CliError> {
    let mut selected = Vec::new();
    let mut total = 0usize;
    let mut current_follow_path = follow_path;

    for commit in reachable_commits {
        if selected.len() >= max_output_number && !count_all_matches {
            break;
        }
        if !filter.passes_non_path_filters(&commit) {
            continue;
        }

        let (effective_paths, cached_changes, follow_rename) =
            if let Some(current_path) = current_follow_path.clone() {
                let follow = follow_changes_for_commit(&commit, &current_path)?;
                if let Some(previous_path) = follow.previous_path {
                    current_follow_path = Some(previous_path);
                }
                if follow.changes.is_empty()
                    || !filter
                        .matches_pickaxe_with_paths(&commit, std::slice::from_ref(&current_path))?
                {
                    continue;
                }
                (vec![current_path], Some(follow.changes), follow.rename)
            } else if filter.paths.is_empty() && !keep_changed_files {
                if !filter.matches(&commit, None).await? {
                    continue;
                }
                (path_filters.to_vec(), None, None)
            } else {
                let changes = get_changed_files_for_commit(&commit, path_filters).await?;
                if !filter.matches(&commit, Some(&changes)).await? {
                    continue;
                }
                (path_filters.to_vec(), Some(changes), None)
            };

        total += 1;
        if selected.len() >= max_output_number {
            continue;
        }

        selected.push(SelectedLogCommit {
            commit,
            cached_changes,
            path_filters: effective_paths,
            follow_rename,
        });
    }

    Ok((selected, total))
}

async fn validate_selected_log_commits(
    selected_commits: &[SelectedLogCommit],
    path_filters: &[PathBuf],
    name_only: bool,
    name_status: bool,
    patch: bool,
    stat: bool,
) -> CliResult<()> {
    for selected in selected_commits {
        let effective_paths = if selected.path_filters.is_empty() {
            path_filters
        } else {
            &selected.path_filters
        };
        if name_only || name_status {
            if selected.cached_changes.is_none() {
                let _ = get_changed_files_for_commit(&selected.commit, effective_paths).await?;
            }
        } else if patch {
            let _ = generate_diff(&selected.commit, effective_paths.to_vec()).await?;
        } else if stat {
            let _ = compute_commit_stat(&selected.commit, effective_paths.to_vec()).await?;
        }
    }

    Ok(())
}

fn format_log_timestamp(timestamp: i64) -> String {
    chrono::DateTime::from_timestamp(timestamp, 0)
        .map(|date| date.to_rfc3339())
        .unwrap_or_else(|| timestamp.to_string())
}

fn collect_log_refs(
    commit: &Commit,
    ref_commits: &HashMap<ObjectHash, Vec<Reference>>,
    head_branch: Option<&str>,
    current_head_commit: Option<ObjectHash>,
) -> Vec<String> {
    let mut refs = Vec::new();
    if current_head_commit == Some(commit.id) {
        if let Some(branch) = head_branch {
            refs.push(format!("HEAD -> {branch}"));
        } else {
            refs.push("HEAD".to_string());
        }
    }

    let mut extra_refs = ref_commits.get(&commit.id).cloned().unwrap_or_default();
    extra_refs.sort();
    for reference in extra_refs {
        if reference.kind == ReferenceKind::Local && Some(reference.name.as_str()) == head_branch {
            continue;
        }

        refs.push(match reference.kind {
            ReferenceKind::Tag => format!("tag: {}", reference.name),
            ReferenceKind::Remote | ReferenceKind::Local => reference.name,
        });
    }

    refs
}

fn load_commit_blob_content(hash: &ObjectHash) -> Result<Vec<u8>, CliError> {
    load_object::<Blob>(hash)
        .map(|blob| blob.data)
        .map_err(|e| log_repo_corrupt_error(format!("failed to load blob object {hash}: {e}")))
}

fn record_commit_diff_error(slot: &Rc<RefCell<Option<CliError>>>, error: CliError) {
    let mut slot = slot.borrow_mut();
    if slot.is_none() {
        *slot = Some(error);
    }
}

fn build_commit_diff_items(
    old_blobs: Vec<(PathBuf, ObjectHash)>,
    new_blobs: Vec<(PathBuf, ObjectHash)>,
    paths: Vec<PathBuf>,
) -> Result<Vec<git_internal::diff::DiffItem>, CliError> {
    let load_error = Rc::new(RefCell::new(None::<CliError>));
    let load_error_for_read = Rc::clone(&load_error);
    let diffs = Diff::diff(
        old_blobs,
        new_blobs,
        paths.into_iter().collect(),
        move |_file, hash| match load_commit_blob_content(hash) {
            Ok(blob) => blob,
            Err(error) => {
                record_commit_diff_error(&load_error_for_read, error);
                Vec::new()
            }
        },
    );
    if let Some(error) = load_error.borrow_mut().take() {
        return Err(error);
    }
    Ok(diffs)
}

async fn commit_touches_paths(commit: &Commit, filters: &[PathBuf]) -> Result<bool, CliError> {
    if filters.is_empty() {
        return Ok(true);
    }
    let changes = get_changed_files_for_commit(commit, filters).await?;
    Ok(!changes.is_empty())
}

/// Get list of changed files for a commit
pub(crate) async fn get_changed_files_for_commit(
    commit: &Commit,
    paths: &[PathBuf],
) -> Result<Vec<FileChange>, CliError> {
    let tree = load_object::<Tree>(&commit.tree_id)
        .map_err(|e| log_repo_corrupt_error(format!("failed to load tree object: {e}")))?;
    let new_blobs: Vec<(PathBuf, ObjectHash)> = tree.get_plain_items();

    let shallow_boundaries = crate::command::fetch::read_shallow_boundaries().unwrap_or_default();
    let has_parents = !commit.parent_commit_ids.is_empty()
        && !shallow_boundaries.contains(&commit.id.to_string());

    let old_blobs: Vec<(PathBuf, ObjectHash)> = if has_parents {
        let parent = &commit.parent_commit_ids[0];
        let parent_commit = load_object::<Commit>(parent)
            .map_err(|e| log_repo_corrupt_error(format!("failed to load parent commit: {e}")))?;
        let parent_tree = load_object::<Tree>(&parent_commit.tree_id)
            .map_err(|e| log_repo_corrupt_error(format!("failed to load parent tree: {e}")))?;
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
    Ok(changed_files)
}

fn format_changes(
    changes: &[FileChange],
    include_status: bool,
    follow_rename: Option<&FollowRename>,
) -> String {
    let mut out = String::new();
    if include_status && let Some(rename) = follow_rename {
        out.push_str(&format!(
            "R{}\t{}\t{}\n",
            rename.score,
            rename.from.display(),
            rename.to.display()
        ));
        return out;
    }
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
pub async fn compute_commit_stat(
    commit: &Commit,
    paths: Vec<PathBuf>,
) -> Result<Vec<FileStat>, CliError> {
    let tree = load_object::<Tree>(&commit.tree_id)
        .map_err(|e| log_repo_corrupt_error(format!("failed to load tree object: {e}")))?;
    let new_blobs: Vec<(PathBuf, ObjectHash)> = tree.get_plain_items();

    let shallow_boundaries = crate::command::fetch::read_shallow_boundaries().unwrap_or_default();
    let has_parents = !commit.parent_commit_ids.is_empty()
        && !shallow_boundaries.contains(&commit.id.to_string());

    let old_blobs: Vec<(PathBuf, ObjectHash)> = if has_parents {
        let parent = &commit.parent_commit_ids[0];
        let parent_commit = load_object::<Commit>(parent)
            .map_err(|e| log_repo_corrupt_error(format!("failed to load parent commit: {e}")))?;
        let parent_tree = load_object::<Tree>(&parent_commit.tree_id)
            .map_err(|e| log_repo_corrupt_error(format!("failed to load parent tree: {e}")))?;
        parent_tree.get_plain_items()
    } else {
        Vec::new()
    };

    let diffs = build_commit_diff_items(old_blobs, new_blobs, paths)?;

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
    Ok(stats)
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

        let plus_count = (stat.insertions * bar_width)
            .checked_div(changes)
            .unwrap_or(0);
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

/// Returns the rotating color used for graph `column` (mirrors how `git log
/// --graph` tints each branch line differently).
fn graph_column_color(column: usize) -> colored::Color {
    const PALETTE: [colored::Color; 6] = [
        colored::Color::Red,
        colored::Color::Green,
        colored::Color::Yellow,
        colored::Color::Blue,
        colored::Color::Magenta,
        colored::Color::Cyan,
    ];
    PALETTE[column % PALETTE.len()]
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

        // Each branch column is drawn in a rotating color; this is a no-op
        // unless the global `--color` setting enables it (the `colored` crate
        // suppresses ANSI for `--color=never`/non-TTY), so the plain ASCII
        // layout is byte-identical when color is off.
        let glyph = |ch: &str, column: usize| ch.color(graph_column_color(column)).to_string();

        if let Some(pos) = self.columns.iter().position(|&c| c == Some(commit_id)) {
            for (i, col) in self.columns.iter().enumerate() {
                if i == pos {
                    prefix.push_str(&glyph("*", i));
                    prefix.push(' ');
                } else if col.is_some() {
                    prefix.push_str(&glyph("|", i));
                    prefix.push(' ');
                } else {
                    prefix.push_str("  ");
                }
            }

            if parent_ids.is_empty() {
                self.columns[pos] = None;
            } else if parent_ids.len() == 1 {
                self.columns[pos] = Some(parent_ids[0]);
            } else {
                self.columns[pos] = Some(parent_ids[0]);

                for parent_id in parent_ids.iter().skip(1) {
                    self.columns.push(Some(*parent_id));
                }
            }
        } else {
            self.columns.insert(0, None);
            prefix.push_str(&glyph("*", 0));
            prefix.push(' ');
            for i in 1..self.columns.len() {
                prefix.push_str(&glyph("|", i));
                prefix.push(' ');
            }

            if !parent_ids.is_empty() {
                self.columns[0] = Some(parent_ids[0]);

                for parent_id in parent_ids.iter().skip(1) {
                    self.columns.push(Some(*parent_id));
                }
            }
        }

        self.columns.retain(|c| c.is_some());

        prefix
    }
}

async fn create_reference_commit_map() -> HashMap<ObjectHash, Vec<Reference>> {
    let mut commit_to_refs: HashMap<ObjectHash, Vec<Reference>> = HashMap::new();

    let all_branches = Branch::list_branches_best_effort(None).await;
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

    let all_tags = tag::list().await.unwrap_or_else(|e| {
        tracing::warn!("failed to list tags for log decoration: {e}");
        Vec::new()
    });
    for tag in all_tags {
        let commit_id = match tag.object {
            TagObject::Commit(c) => c.id,
            TagObject::Tag(t) => t.object_hash,
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
pub(crate) async fn generate_diff(
    commit: &Commit,
    paths: Vec<PathBuf>,
) -> Result<String, CliError> {
    // prepare old and new blobs
    // new_blobs from commit tree
    let tree = load_object::<Tree>(&commit.tree_id)
        .map_err(|e| log_repo_corrupt_error(format!("failed to load tree object: {e}")))?;
    let new_blobs: Vec<(PathBuf, ObjectHash)> = tree.get_plain_items();

    let shallow_boundaries = crate::command::fetch::read_shallow_boundaries().unwrap_or_default();
    let has_parents = !commit.parent_commit_ids.is_empty()
        && !shallow_boundaries.contains(&commit.id.to_string());

    // old_blobs from first parent if exists
    let old_blobs: Vec<(PathBuf, ObjectHash)> = if has_parents {
        let parent = &commit.parent_commit_ids[0];
        let parent_commit = load_object::<Commit>(parent)
            .map_err(|e| log_repo_corrupt_error(format!("failed to load parent commit: {e}")))?;
        let parent_tree = load_object::<Tree>(&parent_commit.tree_id)
            .map_err(|e| log_repo_corrupt_error(format!("failed to load parent tree: {e}")))?;
        parent_tree.get_plain_items()
    } else {
        Vec::new()
    };

    let diffs = build_commit_diff_items(old_blobs, new_blobs, paths)?;
    let mut out = String::new();
    for d in diffs {
        out.push_str(&d.data);
    }
    Ok(out)
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
        let args = LogArgs::parse_from(["libra", "--name-only"]);
        assert!(args.name_only);

        let args = LogArgs::parse_from(["libra"]);
        assert!(!args.name_only);
    }

    #[test]
    fn test_name_only_precedence_over_patch() {
        // Test --name-only takes precedence over --patch
        let args = LogArgs::parse_from(["libra", "--name-only", "--patch"]);
        assert!(args.name_only);
        assert!(args.patch);
        // In the execute function, patch should be ignored when name_only is true
    }

    #[test]
    fn test_name_only_with_oneline() {
        // Test --name-only and --oneline combination
        let args = LogArgs::parse_from(["libra", "--name-only", "--oneline"]);
        assert!(args.name_only);
        assert!(args.oneline);
    }

    #[test]
    fn test_name_only_with_number_limit() {
        // Test --name-only combined with quantity limit
        let args = LogArgs::parse_from(["libra", "--name-only", "-n", "5"]);
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
        let args = LogArgs::parse_from(["libra", "--name-only", "--oneline", "-n", "10"]);
        assert!(args.name_only);
        assert!(args.oneline);
        assert_eq!(args.number, Some(10));

        let args = LogArgs::parse_from(["libra", "--name-only", "src/main.rs", "src/lib.rs"]);
        assert!(args.name_only);
        // Update expected pathspec value to include "log"
        assert_eq!(args.pathspec, vec!["src/main.rs", "src/lib.rs"]);
    }

    #[test]
    fn test_new_filters_parsing() {
        let args = LogArgs::parse_from([
            "libra",
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
        let args = LogArgs::parse_from(["libra", "--name-status"]);
        assert!(args.name_status);
        assert!(!args.name_only);
    }

    #[test]
    fn test_format_changes_output() {
        let changes = vec![FileChange {
            path: PathBuf::from("src/main.rs"),
            status: ChangeType::Added,
        }];
        let with_status = format_changes(&changes, true, None);
        assert!(with_status.contains("A\tsrc/main.rs"));

        let names_only = format_changes(&changes, false, None);
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
            None,
            Some(1_766_000_000),
            Some(1_766_200_000),
            Vec::new(),
            None,
            None,
            None,
            None,
            None,
        );

        assert!(filter.matches(&commit, None).await.unwrap());
    }

    // Test parameter mutual exclusion logic
    #[test]
    fn test_parameter_mutual_exclusion() {
        let args = LogArgs::parse_from(["libra", "--name-only", "--patch"]);

        let name_status = args.name_status;
        let name_only = args.name_only && !name_status;
        let patch = args.patch && !name_only && !name_status;

        assert!(name_only);
        assert!(!patch);

        let args = LogArgs::parse_from(["libra", "--name-status", "--patch"]);
        let name_status = args.name_status;
        let name_only = args.name_only && !name_status;
        let patch = args.patch && !name_only && !name_status;

        assert!(name_status);
        assert!(!patch);
    }

    // Test grep parameter parsing
    #[test]
    fn test_log_args_grep() {
        let args = LogArgs::parse_from(["libra", "--grep", "fix"]);
        assert_eq!(args.grep, Some("fix".to_string()));
        assert!(args.pathspec.is_empty());

        let args = LogArgs::parse_from(["libra"]);
        assert_eq!(args.grep, None);
        assert!(args.pathspec.is_empty());
    }

    // Test grep combined with other arguments
    #[test]
    fn test_grep_with_other_args() {
        let args = LogArgs::parse_from(["libra", "--grep", "feature", "--oneline", "-n", "5"]);
        assert_eq!(args.grep, Some("feature".to_string()));
        assert!(args.oneline);
        assert_eq!(args.number, Some(5));
        assert!(args.pathspec.is_empty());
    }

    // Test case-sensitive matching
    #[test]
    fn test_grep_case_sensitive() {
        let args = LogArgs::parse_from(["libra", "--grep", "FIX"]);
        assert_eq!(args.grep, Some("FIX".to_string()));
        assert!(args.pathspec.is_empty());
    }

    // Test empty string grep
    #[test]
    fn test_grep_empty_string() {
        let args = LogArgs::parse_from(["libra", "--grep", ""]);
        assert_eq!(args.grep, Some("".to_string()));
        assert!(args.pathspec.is_empty());
    }

    // Test graph with grep combination
    #[test]
    fn test_graph_with_grep() {
        let args = LogArgs::parse_from(["libra", "--graph", "--grep", "fix"]);
        assert!(args.graph);
        assert_eq!(args.grep, Some("fix".to_string()));
        assert!(args.pathspec.is_empty());
    }
}
