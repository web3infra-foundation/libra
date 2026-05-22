//! Implementation of `describe` command, which finds the most recent tag reachable from a commit.
use std::collections::{HashMap, HashSet, VecDeque};

use clap::Parser;
use git_internal::{
    hash::ObjectHash,
    internal::object::{commit::Commit, types::ObjectType},
};
use serde::Serialize;

use crate::{
    command::{
        load_object,
        status::{changes_to_be_committed_safe, changes_to_be_staged},
    },
    internal::{
        branch::Branch,
        config::ConfigKv,
        db::get_db_conn_instance_for_path,
        tag::{self, TagObject},
    },
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, emit_json_data},
        util::{self, CommitBaseError},
    },
};

const DESCRIBE_EXAMPLES: &str = "\
EXAMPLES:
    libra describe                  Describe HEAD using the nearest annotated tag
    libra describe --tags           Include lightweight tags (not just annotated ones) in the search
    libra describe --always         Fall back to abbreviated commit hash when no tag matches
    libra describe HEAD~1           Describe a specific commit-ish (hash, ref, or HEAD~N)
    libra describe --abbrev 12      Use 12 hex digits instead of the default 7 in the hash portion
    libra describe --exact-match    Only succeed when a tag points at the commit exactly
    libra describe --first-parent   Follow only the first parent of merge commits
    libra describe --match 'v1.*'   Only consider tags whose name matches the glob
    libra describe --exclude '*rc*' Skip tags whose name matches the glob
    libra describe --dirty          Append '-dirty' when the worktree has tracked changes
    libra describe --contains HEAD  Find which ref contains the commit (refname~N)
    libra describe --candidates 5   Consider at most 5 candidate tags
    libra describe --all --contains Search branches and remotes, not just tags
    libra describe --json           Structured JSON output for agents";

#[derive(Parser, Debug)]
#[command(after_help = DESCRIBE_EXAMPLES)]
pub struct DescribeArgs {
    /// Commit-ish (hash, ref, or tag) to describe. Defaults to HEAD
    pub commit: Option<String>,

    /// Consider any tag in refs/tags (not just annotated tags) when describing
    #[clap(long)]
    pub tags: bool,

    /// Use N hex digits for the abbreviated commit hash (default: 7)
    #[clap(long, value_name = "N")]
    pub abbrev: Option<usize>,

    /// Show an abbreviated commit hash when no tag can describe the target.
    #[clap(long)]
    pub always: bool,

    /// Only describe the target when a tag points at it exactly (distance 0)
    #[clap(long = "exact-match")]
    pub exact_match: bool,

    /// Follow only the first parent of merge commits when walking history
    #[clap(long = "first-parent")]
    pub first_parent: bool,

    /// Only consider tags whose name matches the glob (repeatable; OR semantics)
    #[clap(long = "match", value_name = "PATTERN")]
    pub match_patterns: Vec<String>,

    /// Exclude tags whose name matches the glob (repeatable; takes precedence over --match)
    #[clap(long, value_name = "PATTERN")]
    pub exclude: Vec<String>,

    /// Append a marker (default `-dirty`) when the worktree has tracked changes
    #[clap(long, value_name = "MARK", num_args = 0..=1, require_equals = true)]
    pub dirty: Option<Option<String>>,

    /// Find which ref contains the commit and print `<refname>~<offset>`
    #[clap(long)]
    pub contains: bool,

    /// Consider at most N candidate tags (default: describe.maxCandidates or 10; rejects 0)
    #[clap(long, value_name = "N")]
    pub candidates: Option<usize>,

    /// Consider all refs (local branches and remote-tracking), not just tags (with --contains)
    #[clap(long)]
    pub all: bool,
}

/// Maximum number of commits the describe walk visits before failing. Guards
/// against unbounded traversal (and OOM) on very deep histories; when the cap is
/// hit without `--always`, the command fails with [`DescribeError::TraversalLimitExceeded`].
const MAX_WALK: usize = 10_000;
const DEFAULT_CANDIDATES: usize = 10;

/// Maximum byte length accepted for a `--match`/`--exclude` glob pattern, guarding
/// against pathological inputs. Longer patterns are rejected up front with
/// [`DescribeError::InvalidArgument`] (`CliInvalidArguments`, exit 129).
const MAX_GLOB_LEN: usize = 256;

// Entry in tag lookup map
struct TagInfo {
    name: String,
    is_annotated: bool,
}

/// Kind of ref considered by `--contains` (and `--all`). Ordering encodes the
/// deterministic tie-break preference: tag beats head beats remote.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RefKind {
    Tag,
    Head,
    Remote,
}

impl RefKind {
    fn as_str(self) -> &'static str {
        match self {
            RefKind::Tag => "tag",
            RefKind::Head => "head",
            RefKind::Remote => "remote",
        }
    }

    fn priority(self) -> u8 {
        match self {
            RefKind::Tag => 0,
            RefKind::Head => 1,
            RefKind::Remote => 2,
        }
    }
}

/// A candidate ref tip for `--contains`: its display name, kind, and commit.
struct RefTip {
    name: String,
    kind: RefKind,
    commit: ObjectHash,
}

#[derive(Debug, Clone, Serialize)]
struct DescribeOutput {
    input: String,
    resolved_commit: String,
    result: String,
    tag: Option<String>,
    distance: Option<usize>,
    abbreviated_commit: Option<String>,
    exact_match: bool,
    used_always: bool,
    dirty: bool,
    dirty_suffix: Option<String>,
    contains_offset: Option<usize>,
    ref_kind: Option<String>,
    ref_name: Option<String>,
}

#[derive(Debug, thiserror::Error)]
enum DescribeError {
    #[error("HEAD does not point to a commit")]
    HeadUnborn,
    #[error("{0}")]
    InvalidReference(String),
    #[error("{0}")]
    ReadFailure(String),
    #[error("{0}")]
    CorruptReference(String),
    #[error("failed to load commit '{commit_id}': {detail}")]
    LoadCommit { commit_id: String, detail: String },
    #[error("no names found, cannot describe anything")]
    NoNamesFound,
    #[error(
        "history too deep: walked more than {limit} commits; pass --always or narrow the range"
    )]
    TraversalLimitExceeded { limit: usize },
    #[error("{0}")]
    InvalidArgument(String),
}

impl From<CommitBaseError> for DescribeError {
    fn from(error: CommitBaseError) -> Self {
        match error {
            CommitBaseError::HeadUnborn => Self::HeadUnborn,
            CommitBaseError::InvalidReference(message) => Self::InvalidReference(message),
            CommitBaseError::ReadFailure(message) => Self::ReadFailure(message),
            CommitBaseError::CorruptReference(message) => Self::CorruptReference(message),
        }
    }
}

pub async fn execute(args: DescribeArgs) {
    if let Err(e) = execute_safe(args, &OutputConfig::default()).await {
        e.print_stderr();
    }
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting.
pub async fn execute_safe(args: DescribeArgs, output: &OutputConfig) -> CliResult<()> {
    util::require_repo().map_err(|_| CliError::repo_not_found())?;
    let describe_output = run_describe(args).await.map_err(describe_cli_error)?;

    if output.is_json() {
        emit_json_data("describe", &describe_output, output)?;
    } else if !output.quiet {
        println!("{}", describe_output.result);
    }

    Ok(())
}

async fn run_describe(args: DescribeArgs) -> Result<DescribeOutput, DescribeError> {
    let input = args.commit.unwrap_or_else(|| "HEAD".to_string());
    let start_hash = util::get_commit_base_typed(&input)
        .await
        .map_err(DescribeError::from)?;
    let resolved_commit = start_hash.to_string();
    let abbrev = args.abbrev.unwrap_or(7);

    // Compile the --match / --exclude name filters once. Overly long or malformed
    // patterns are rejected up front as usage errors (CliInvalidArguments, 129).
    let matchers = compile_globs(&args.match_patterns)?;
    let excluders = compile_globs(&args.exclude)?;

    // Resolve and validate the candidate cap (rejects --candidates=0 with 129).
    let max_candidates = resolve_max_candidates(args.candidates).await?;

    // `--contains` is the reverse search: which ref's history contains the target?
    if args.contains {
        let mut output = run_contains(
            input,
            start_hash,
            resolved_commit,
            abbrev,
            args.all,
            args.first_parent,
            args.always,
            args.exact_match,
            &matchers,
            &excluders,
        )
        .await?;
        apply_dirty(&mut output, &args.dirty).await;
        return Ok(output);
    }

    // Load all tags and build a mapping table: commit hash -> tag info (name, is_annotated)
    let all_tags = tag::list()
        .await
        .map_err(|e| DescribeError::CorruptReference(e.to_string()))?;
    let mut tag_map: HashMap<ObjectHash, TagInfo> = HashMap::new();

    for t in all_tags {
        let is_annotated = t.object.get_type() == ObjectType::Tag;

        // Only include light-weight tags if --tags is specified
        if is_annotated || args.tags {
            let tag_name = t.name;
            // Apply --match / --exclude name filters (exclude wins over match).
            if !tag_passes_filters(&tag_name, &matchers, &excluders) {
                continue;
            }
            let target_commit_hash = match t.object {
                TagObject::Commit(c) => c.id,
                TagObject::Tag(tg) => tg.object_hash,
                _ => continue,
            };

            let should_replace = tag_map
                .get(&target_commit_hash)
                .is_none_or(|existing| prefer_tag(existing, &tag_name, is_annotated));
            if should_replace {
                tag_map.insert(
                    target_commit_hash,
                    TagInfo {
                        name: tag_name,
                        is_annotated,
                    },
                );
            }
        }
    }

    // Build the base description. `--exact-match` short-circuits: only a distance-0
    // tag describes the target; anything else is a (run-time) failure, matching Git.
    let mut output = if args.exact_match {
        match tag_map.get(&start_hash) {
            Some(tag_info) => describe_output(input, resolved_commit, &tag_info.name, 0, abbrev),
            None => return Err(DescribeError::NoNamesFound),
        }
    } else {
        // Search for the closest tag using a bounded BFS. The commit loader is
        // injected so the walk can be unit-tested against an in-memory graph.
        let mut load = |hash: &ObjectHash| -> Result<Vec<ObjectHash>, DescribeError> {
            let commit =
                load_object::<Commit>(hash).map_err(|error| DescribeError::LoadCommit {
                    commit_id: hash.to_string(),
                    detail: error.to_string(),
                })?;
            Ok(commit.parent_commit_ids)
        };

        // With a candidate cap (`--candidates` / `describe.maxCandidates`), bound the
        // collection; otherwise early-return at the nearest tag for speed.
        let nearest = match max_candidates {
            Some(cap) => find_best_candidate_tag(
                start_hash,
                &tag_map,
                args.first_parent,
                MAX_WALK,
                cap,
                &mut load,
            ),
            None => find_nearest_tag(start_hash, &tag_map, args.first_parent, MAX_WALK, &mut load),
        };

        match nearest {
            Ok(Some((tag_name, dist))) => {
                describe_output(input, resolved_commit, &tag_name, dist, abbrev)
            }
            Ok(None) => {
                if args.always {
                    always_output(input, resolved_commit, abbrev)
                } else {
                    return Err(DescribeError::NoNamesFound);
                }
            }
            // A traversal-cap hit still honors `--always` (abbreviated fallback);
            // without it, surface the dedicated deep-history error.
            Err(DescribeError::TraversalLimitExceeded { limit }) => {
                if args.always {
                    always_output(input, resolved_commit, abbrev)
                } else {
                    return Err(DescribeError::TraversalLimitExceeded { limit });
                }
            }
            Err(other) => return Err(other),
        }
    };

    // Apply the `--dirty` suffix when the worktree carries tracked changes.
    apply_dirty(&mut output, &args.dirty).await;

    Ok(output)
}

/// Append the `--dirty` suffix to `output` when the worktree carries tracked
/// changes. A clean worktree, untracked-only changes, or a status read error all
/// leave `output` untouched.
async fn apply_dirty(output: &mut DescribeOutput, dirty: &Option<Option<String>>) {
    if let Some(mark) = dirty.as_ref()
        && worktree_is_dirty().await
    {
        let suffix = mark.clone().unwrap_or_else(|| "-dirty".to_string());
        output.result.push_str(&suffix);
        output.dirty = true;
        output.dirty_suffix = Some(suffix);
    }
}

/// Reverse `--contains` search: walk from each candidate ref tip toward the target
/// and report the topologically nearest ref as `<refname>~<offset>` (or just the
/// refname at offset 0). Honors `--all` (branches + remotes), `--first-parent`,
/// `--match`/`--exclude`, and `--always`.
#[allow(clippy::too_many_arguments)]
async fn run_contains(
    input: String,
    start_hash: ObjectHash,
    resolved_commit: String,
    abbrev: usize,
    include_all: bool,
    first_parent: bool,
    always: bool,
    exact_match: bool,
    matchers: &[wax::Glob<'_>],
    excluders: &[wax::Glob<'_>],
) -> Result<DescribeOutput, DescribeError> {
    let tips = collect_ref_tips(include_all, matchers, excluders).await?;

    let mut load = |hash: &ObjectHash| -> Result<Vec<ObjectHash>, DescribeError> {
        let commit = load_object::<Commit>(hash).map_err(|error| DescribeError::LoadCommit {
            commit_id: hash.to_string(),
            detail: error.to_string(),
        })?;
        Ok(commit.parent_commit_ids)
    };

    let mut best: Option<(usize, &RefTip)> = None;
    let mut cap_hit = false;
    for tip in &tips {
        match distance_to_target(tip.commit, start_hash, first_parent, MAX_WALK, &mut load) {
            Ok(Some(distance)) => {
                let replace = match best {
                    None => true,
                    Some((best_distance, best_tip)) => {
                        distance < best_distance
                            || (distance == best_distance && tie_break_better(tip, best_tip))
                    }
                };
                if replace {
                    best = Some((distance, tip));
                }
            }
            Ok(None) => {}
            Err(DescribeError::TraversalLimitExceeded { .. }) => cap_hit = true,
            Err(other) => return Err(other),
        }
    }

    match best {
        // `--exact-match` requires a ref that points directly at the target (offset
        // 0); any positive offset is a run-time failure, matching the non-`--contains`
        // exact-match path (which returns `NoNamesFound` without honoring `--always`).
        Some((distance, _)) if exact_match && distance > 0 => Err(DescribeError::NoNamesFound),
        Some((distance, tip)) => Ok(contains_output(input, resolved_commit, tip, distance)),
        None if exact_match => Err(DescribeError::NoNamesFound),
        None if always => Ok(always_output(input, resolved_commit, abbrev)),
        None if cap_hit => Err(DescribeError::TraversalLimitExceeded { limit: MAX_WALK }),
        None => Err(DescribeError::NoNamesFound),
    }
}

/// Enumerate the candidate ref tips for `--contains`: all tags (including
/// lightweight, matching Git's `describe --contains`) filtered by
/// `--match`/`--exclude`, plus local branch heads and remote-tracking branches
/// when `--all` is set.
async fn collect_ref_tips(
    include_all: bool,
    matchers: &[wax::Glob<'_>],
    excluders: &[wax::Glob<'_>],
) -> Result<Vec<RefTip>, DescribeError> {
    let mut tips = Vec::new();

    let all_tags = tag::list()
        .await
        .map_err(|e| DescribeError::CorruptReference(e.to_string()))?;
    for t in all_tags {
        if !tag_passes_filters(&t.name, matchers, excluders) {
            continue;
        }
        let commit = match t.object {
            TagObject::Commit(c) => c.id,
            TagObject::Tag(tg) => tg.object_hash,
            _ => continue,
        };
        tips.push(RefTip {
            name: t.name,
            kind: RefKind::Tag,
            commit,
        });
    }

    if include_all {
        let heads = Branch::list_branches_result(None)
            .await
            .map_err(|e| DescribeError::ReadFailure(format!("failed to list branches: {e}")))?;
        for branch in heads {
            tips.push(RefTip {
                name: format!("heads/{}", branch.name),
                kind: RefKind::Head,
                commit: branch.commit,
            });
        }

        let remotes = ConfigKv::all_remote_configs()
            .await
            .map_err(|e| DescribeError::ReadFailure(format!("failed to list remotes: {e}")))?;
        for remote in remotes {
            let remote_branches = Branch::list_branches_result(Some(&remote.name))
                .await
                .map_err(|e| {
                    DescribeError::ReadFailure(format!(
                        "failed to list remote-tracking branches for '{}': {e}",
                        remote.name
                    ))
                })?;
            for branch in remote_branches {
                tips.push(RefTip {
                    name: format!("remotes/{}/{}", remote.name, branch.name),
                    kind: RefKind::Remote,
                    commit: branch.commit,
                });
            }
        }
    }

    Ok(tips)
}

/// Reverse-walk breadth-first from `tip` toward `target`, returning the shortest
/// topological distance when `target` is reachable, or `None` otherwise. Bounded
/// by `max_walk` and deduplicated via `visited`; with `first_parent`, only the
/// first parent of each commit is followed. The loader is injected for testing.
fn distance_to_target<F>(
    tip: ObjectHash,
    target: ObjectHash,
    first_parent: bool,
    max_walk: usize,
    mut load: F,
) -> Result<Option<usize>, DescribeError>
where
    F: FnMut(&ObjectHash) -> Result<Vec<ObjectHash>, DescribeError>,
{
    let mut queue: VecDeque<(ObjectHash, usize)> = VecDeque::new();
    let mut visited: HashSet<ObjectHash> = HashSet::new();
    let mut walked = 0usize;

    queue.push_back((tip, 0));
    visited.insert(tip);

    while let Some((curr, dist)) = queue.pop_front() {
        if curr == target {
            return Ok(Some(dist));
        }

        walked += 1;
        if walked > max_walk {
            return Err(DescribeError::TraversalLimitExceeded { limit: max_walk });
        }

        let parents = load(&curr)?;
        if first_parent {
            if let Some(parent) = parents.first().copied()
                && visited.insert(parent)
            {
                queue.push_back((parent, dist + 1));
            }
        } else {
            for parent in parents {
                if visited.insert(parent) {
                    queue.push_back((parent, dist + 1));
                }
            }
        }
    }

    Ok(None)
}

/// Deterministic tie-break for equal-distance `--contains` matches: prefer the
/// ref kind (tag > head > remote), then the lexicographically smaller refname.
fn tie_break_better(candidate: &RefTip, current: &RefTip) -> bool {
    let candidate_priority = candidate.kind.priority();
    let current_priority = current.kind.priority();
    candidate_priority < current_priority
        || (candidate_priority == current_priority && candidate.name < current.name)
}

async fn resolve_max_candidates(flag: Option<usize>) -> Result<Option<usize>, DescribeError> {
    if let Some(n) = flag {
        if n == 0 {
            return Err(DescribeError::InvalidArgument(
                "candidates must be >= 1".to_string(),
            ));
        }
        return Ok(Some(n));
    }
    let Ok(storage_path) = util::try_get_storage_path(None) else {
        return Ok(Some(DEFAULT_CANDIDATES));
    };
    let db_path = storage_path.join(util::DATABASE);
    let Ok(db) = get_db_conn_instance_for_path(&db_path).await else {
        return Ok(Some(DEFAULT_CANDIDATES));
    };

    match ConfigKv::get_with_conn(&db, "describe.maxCandidates").await {
        Ok(Some(entry)) => match entry.value.trim().parse::<usize>() {
            Ok(n) if n >= 1 => Ok(Some(n)),
            _ => Ok(Some(DEFAULT_CANDIDATES)),
        },
        _ => Ok(Some(DEFAULT_CANDIDATES)),
    }
}

/// Walk the commit DAG breadth-first from `start`, returning the nearest tag
/// (name + shortest topological distance) or `None` when none is reachable.
///
/// `load` returns the parent hashes of a commit; injecting it keeps this walker
/// unit-testable against a small in-memory graph (no real object store). The
/// already-existing `visited` set deduplicates diamond merges, and the walk
/// aborts with [`DescribeError::TraversalLimitExceeded`] once it has visited
/// `max_walk` commits without a hit. With `first_parent`, only the first parent
/// of each commit is followed.
fn find_nearest_tag<F>(
    start: ObjectHash,
    tag_map: &HashMap<ObjectHash, TagInfo>,
    first_parent: bool,
    max_walk: usize,
    mut load: F,
) -> Result<Option<(String, usize)>, DescribeError>
where
    F: FnMut(&ObjectHash) -> Result<Vec<ObjectHash>, DescribeError>,
{
    let mut queue: VecDeque<(ObjectHash, usize)> = VecDeque::new();
    let mut visited: HashSet<ObjectHash> = HashSet::new();
    let mut walked = 0usize;

    queue.push_back((start, 0));
    visited.insert(start);

    while let Some((curr_hash, dist)) = queue.pop_front() {
        if let Some(tag_info) = tag_map.get(&curr_hash) {
            return Ok(Some((tag_info.name.clone(), dist)));
        }

        walked += 1;
        if walked > max_walk {
            return Err(DescribeError::TraversalLimitExceeded { limit: max_walk });
        }

        let parents = load(&curr_hash)?;
        if first_parent {
            if let Some(parent) = parents.first().copied()
                && visited.insert(parent)
            {
                queue.push_back((parent, dist + 1));
            }
        } else {
            for parent in parents {
                if visited.insert(parent) {
                    queue.push_back((parent, dist + 1));
                }
            }
        }
    }

    Ok(None)
}

/// Like [`find_nearest_tag`] but bounded by `max_candidates`: it collects up to
/// that many reachable tags (continuing past each hit) and returns the
/// topologically nearest, ties broken by annotated-over-lightweight then name.
/// Used when `--candidates` / `describe.maxCandidates` bounds the search.
fn find_best_candidate_tag<F>(
    start: ObjectHash,
    tag_map: &HashMap<ObjectHash, TagInfo>,
    first_parent: bool,
    max_walk: usize,
    max_candidates: usize,
    mut load: F,
) -> Result<Option<(String, usize)>, DescribeError>
where
    F: FnMut(&ObjectHash) -> Result<Vec<ObjectHash>, DescribeError>,
{
    let mut queue: VecDeque<(ObjectHash, usize)> = VecDeque::new();
    let mut visited: HashSet<ObjectHash> = HashSet::new();
    let mut walked = 0usize;
    let mut best: Option<(String, usize, bool)> = None;
    let mut found = 0usize;

    queue.push_back((start, 0));
    visited.insert(start);

    while let Some((curr, dist)) = queue.pop_front() {
        if let Some(info) = tag_map.get(&curr) {
            let replace = match &best {
                None => true,
                Some((best_name, best_dist, best_annotated)) => {
                    dist < *best_dist
                        || (dist == *best_dist
                            && better_candidate(
                                info.is_annotated,
                                &info.name,
                                *best_annotated,
                                best_name,
                            ))
                }
            };
            if replace {
                best = Some((info.name.clone(), dist, info.is_annotated));
            }
            found += 1;
            if found >= max_candidates {
                break;
            }
        }

        walked += 1;
        if walked > max_walk {
            return Err(DescribeError::TraversalLimitExceeded { limit: max_walk });
        }

        let parents = load(&curr)?;
        if first_parent {
            if let Some(parent) = parents.first().copied()
                && visited.insert(parent)
            {
                queue.push_back((parent, dist + 1));
            }
        } else {
            for parent in parents {
                if visited.insert(parent) {
                    queue.push_back((parent, dist + 1));
                }
            }
        }
    }

    Ok(best.map(|(name, dist, _)| (name, dist)))
}

/// Whether `candidate` should replace `current` at the same distance: annotated
/// tags win over lightweight, then the lexicographically smaller name wins.
fn better_candidate(
    candidate_annotated: bool,
    candidate_name: &str,
    current_annotated: bool,
    current_name: &str,
) -> bool {
    match (candidate_annotated, current_annotated) {
        (true, false) => true,
        (false, true) => false,
        _ => candidate_name < current_name,
    }
}

/// Build the `--always` abbreviated-hash fallback output.
fn always_output(input: String, resolved_commit: String, abbrev: usize) -> DescribeOutput {
    let abbreviated = abbreviate_hash(&resolved_commit, abbrev);
    DescribeOutput {
        input,
        resolved_commit,
        result: abbreviated.clone(),
        tag: None,
        distance: None,
        abbreviated_commit: Some(abbreviated),
        exact_match: false,
        used_always: true,
        dirty: false,
        dirty_suffix: None,
        contains_offset: None,
        ref_kind: None,
        ref_name: None,
    }
}

/// Build the `--contains` output (`<refname>~<offset>`, or just the refname at
/// offset 0). For tag tips, the `tag` field is populated; for branch/remote tips
/// it is null and `ref_kind`/`ref_name` carry the ref identity.
fn contains_output(
    input: String,
    resolved_commit: String,
    tip: &RefTip,
    distance: usize,
) -> DescribeOutput {
    let result = if distance == 0 {
        tip.name.clone()
    } else {
        format!("{}~{}", tip.name, distance)
    };
    DescribeOutput {
        input,
        resolved_commit,
        result,
        tag: (tip.kind == RefKind::Tag).then(|| tip.name.clone()),
        distance: None,
        abbreviated_commit: None,
        exact_match: distance == 0,
        used_always: false,
        dirty: false,
        dirty_suffix: None,
        contains_offset: Some(distance),
        ref_kind: Some(tip.kind.as_str().to_string()),
        ref_name: Some(tip.name.clone()),
    }
}

// Formats the output string based on Git's describe rules.
fn format_describe_result(tag_name: &str, dist: usize, full_sha: &str, abbrev: usize) -> String {
    if dist == 0 || abbrev == 0 {
        // If the current commit is exactly at the tag, just return the tag name
        tag_name.to_string()
    } else {
        // Extract the abbreviated hash based on the specified length (default 7)
        let short_sha = abbreviate_hash(full_sha, abbrev);
        // format: <tag_name>-<distance>-g<abbreviated_sha>
        format!("{}-{}-g{}", tag_name, dist, short_sha)
    }
}

fn describe_output(
    input: String,
    resolved_commit: String,
    tag_name: &str,
    distance: usize,
    abbrev: usize,
) -> DescribeOutput {
    let abbreviated_commit =
        (distance > 0 && abbrev > 0).then(|| abbreviate_hash(&resolved_commit, abbrev));
    DescribeOutput {
        input,
        resolved_commit: resolved_commit.clone(),
        result: format_describe_result(tag_name, distance, &resolved_commit, abbrev),
        tag: Some(tag_name.to_string()),
        distance: Some(distance),
        abbreviated_commit,
        exact_match: distance == 0,
        used_always: false,
        dirty: false,
        dirty_suffix: None,
        contains_offset: None,
        ref_kind: None,
        ref_name: None,
    }
}

fn abbreviate_hash(full_sha: &str, abbrev: usize) -> String {
    if abbrev == 0 || abbrev >= full_sha.len() {
        full_sha.to_string()
    } else {
        full_sha[..abbrev].to_string()
    }
}

fn prefer_tag(existing: &TagInfo, candidate_name: &str, candidate_is_annotated: bool) -> bool {
    match (existing.is_annotated, candidate_is_annotated) {
        (false, true) => true,
        (true, false) => false,
        _ => candidate_name < existing.name.as_str(),
    }
}

/// Compile `--match`/`--exclude` glob patterns, rejecting overly long or malformed
/// patterns with [`DescribeError::InvalidArgument`] (`CliInvalidArguments`, exit 129).
/// Returned globs borrow `patterns`, so the slice must outlive the filter loop.
fn compile_globs(patterns: &[String]) -> Result<Vec<wax::Glob<'_>>, DescribeError> {
    let mut globs = Vec::with_capacity(patterns.len());
    for pattern in patterns {
        if pattern.len() > MAX_GLOB_LEN {
            return Err(DescribeError::InvalidArgument(format!(
                "glob pattern too long ({} chars); the limit is {MAX_GLOB_LEN}",
                pattern.len()
            )));
        }
        let glob = wax::Glob::new(pattern.as_str()).map_err(|error| {
            DescribeError::InvalidArgument(format!("invalid glob pattern '{pattern}': {error}"))
        })?;
        globs.push(glob);
    }
    Ok(globs)
}

/// Whether a tag name survives the `--match`/`--exclude` filters. An exclude match
/// always rejects; with no `--match` patterns every non-excluded name passes,
/// otherwise the name must match at least one `--match` glob.
fn tag_passes_filters(name: &str, matchers: &[wax::Glob<'_>], excluders: &[wax::Glob<'_>]) -> bool {
    if excluders
        .iter()
        .any(|glob| wax::Program::is_match(glob, name))
    {
        return false;
    }
    if matchers.is_empty() {
        return true;
    }
    matchers
        .iter()
        .any(|glob| wax::Program::is_match(glob, name))
}

/// Read-only worktree dirtiness probe for `--dirty`. Tracked modifications,
/// deletions, and staged additions count; untracked files never do. Any status
/// read error degrades to "clean" rather than panicking, so a status hiccup never
/// fabricates a `-dirty` suffix.
async fn worktree_is_dirty() -> bool {
    // Staged (index-vs-HEAD): any tracked new/modified/deleted entry is dirty.
    match changes_to_be_committed_safe().await {
        Ok(changes) => {
            if !changes.new.is_empty()
                || !changes.modified.is_empty()
                || !changes.deleted.is_empty()
            {
                return true;
            }
        }
        Err(error) => {
            tracing::warn!(%error, "describe --dirty: staged status scan failed; treating as clean");
        }
    }
    // Unstaged (worktree-vs-index): only modified/deleted count. The `new` set here
    // includes untracked files, which must never mark the worktree dirty.
    match changes_to_be_staged() {
        Ok(changes) => !changes.modified.is_empty() || !changes.deleted.is_empty(),
        Err(error) => {
            tracing::warn!(%error, "describe --dirty: unstaged status scan failed; treating as clean");
            false
        }
    }
}

fn describe_cli_error(error: DescribeError) -> CliError {
    match error {
        DescribeError::HeadUnborn => CliError::fatal(error.to_string())
            .with_stable_code(StableErrorCode::RepoStateInvalid)
            .with_hint("create a commit before running 'libra describe'."),
        DescribeError::InvalidReference(message) => CliError::command_usage(message)
            .with_stable_code(StableErrorCode::CliInvalidTarget)
            .with_hint("check the revision and try again."),
        DescribeError::ReadFailure(message) => {
            CliError::fatal(message).with_stable_code(StableErrorCode::IoReadFailed)
        }
        DescribeError::CorruptReference(message) => {
            CliError::fatal(message).with_stable_code(StableErrorCode::RepoCorrupt)
        }
        DescribeError::NoNamesFound => CliError::fatal("no names found, cannot describe anything")
            .with_stable_code(StableErrorCode::RepoStateInvalid)
            .with_hint(
                "create a tag, pass '--tags' to include lightweight tags, or use '--always'.",
            ),
        DescribeError::LoadCommit { commit_id, detail } => {
            CliError::fatal(format!("failed to load commit '{commit_id}': {detail}"))
                .with_stable_code(StableErrorCode::RepoCorrupt)
        }
        DescribeError::TraversalLimitExceeded { .. } => CliError::fatal(error.to_string())
            .with_stable_code(StableErrorCode::RepoStateInvalid)
            .with_hint("history is very deep; pass '--always' for an abbreviated hash or narrow the range."),
        DescribeError::InvalidArgument(message) => CliError::command_usage(message)
            .with_hint("check the --match/--exclude glob syntax and any --candidates value."),
    }
}

#[cfg(test)]
mod tests {
    use git_internal::hash::{HashKind, set_hash_kind_for_test};

    use super::*;

    /// Pin the `Display` format for every variant of [`DescribeError`].
    /// These strings are used directly as the CliError message via
    /// `describe_cli_error` (lines above) and surface in both human
    /// and `--json` envelopes.
    #[test]
    fn describe_error_display_pins_each_variant() {
        assert_eq!(
            DescribeError::HeadUnborn.to_string(),
            "HEAD does not point to a commit",
        );
        // `{0}`-only variants echo the inner string verbatim.
        assert_eq!(
            DescribeError::InvalidReference("bad-ref".to_string()).to_string(),
            "bad-ref",
        );
        assert_eq!(
            DescribeError::ReadFailure("db locked".to_string()).to_string(),
            "db locked",
        );
        assert_eq!(
            DescribeError::CorruptReference("bad commit hash".to_string()).to_string(),
            "bad commit hash",
        );
        assert_eq!(
            DescribeError::LoadCommit {
                commit_id: "deadbeef".to_string(),
                detail: "object not found".to_string(),
            }
            .to_string(),
            "failed to load commit 'deadbeef': object not found",
        );
        assert_eq!(
            DescribeError::NoNamesFound.to_string(),
            "no names found, cannot describe anything",
        );
        assert_eq!(
            DescribeError::TraversalLimitExceeded { limit: 10_000 }.to_string(),
            "history too deep: walked more than 10000 commits; pass --always or narrow the range",
        );
        assert_eq!(
            DescribeError::InvalidArgument("candidates must be >= 1".to_string()).to_string(),
            "candidates must be >= 1",
        );
    }

    #[test]
    fn test_tag_passes_filters_match_exclude_semantics() {
        let no_globs: [wax::Glob<'_>; 0] = [];
        // No filters: every name passes.
        assert!(tag_passes_filters("v1.0", &no_globs, &no_globs));
        // --match only: name must match at least one glob.
        let match_pats = ["v1.*".to_string()];
        let m = compile_globs(&match_pats).expect("valid glob");
        assert!(tag_passes_filters("v1.2", &m, &no_globs));
        assert!(!tag_passes_filters("v2.0", &m, &no_globs));
        // --exclude wins over --match.
        let exclude_pats = ["*rc*".to_string()];
        let e = compile_globs(&exclude_pats).expect("valid glob");
        assert!(!tag_passes_filters("v1.0rc1", &m, &e));
        assert!(tag_passes_filters("v1.0", &m, &e));
    }

    #[test]
    fn test_compile_globs_rejects_overlong_and_invalid() {
        let long = "a".repeat(MAX_GLOB_LEN + 1);
        assert!(matches!(
            compile_globs(&[long]),
            Err(DescribeError::InvalidArgument(_))
        ));
        // An unterminated alternation `{` is not a valid glob.
        assert!(matches!(
            compile_globs(&["v{1".to_string()]),
            Err(DescribeError::InvalidArgument(_))
        ));
    }

    #[tokio::test]
    async fn resolve_max_candidates_defaults_to_ten() {
        assert_eq!(
            resolve_max_candidates(None).await.unwrap(),
            Some(DEFAULT_CANDIDATES),
        );
    }

    /// Build a distinct in-memory commit hash for graph-shaped unit tests.
    /// SHA-1 (20-byte) is forced so `from_bytes` matches the active hash kind.
    fn test_hash(seed: u8) -> ObjectHash {
        ObjectHash::from_bytes(&[seed; 20]).expect("20-byte SHA-1 hash is valid")
    }

    fn tag_at(hash: ObjectHash, name: &str) -> HashMap<ObjectHash, TagInfo> {
        let mut map = HashMap::new();
        map.insert(
            hash,
            TagInfo {
                name: name.to_string(),
                is_annotated: true,
            },
        );
        map
    }

    /// A diamond history A→{B,C}→D must load the shared ancestor D exactly once,
    /// proving the `visited` set deduplicates merge re-convergence.
    #[test]
    fn test_find_nearest_tag_visited_dedups_diamond() {
        let _guard = set_hash_kind_for_test(HashKind::Sha1);
        let (a, b, c, d) = (test_hash(1), test_hash(2), test_hash(3), test_hash(4));
        // a (start) → b, c ; b → d ; c → d ; d tagged "base".
        let graph: HashMap<ObjectHash, Vec<ObjectHash>> =
            HashMap::from([(a, vec![b, c]), (b, vec![d]), (c, vec![d]), (d, vec![])]);
        let tag_map = tag_at(d, "base");

        let mut load_counts: HashMap<ObjectHash, usize> = HashMap::new();
        let result = find_nearest_tag(a, &tag_map, false, 100, |hash| {
            *load_counts.entry(*hash).or_insert(0) += 1;
            Ok(graph.get(hash).cloned().unwrap_or_default())
        })
        .expect("walk should succeed");

        assert_eq!(result, Some(("base".to_string(), 2)));
        // D is reachable via both B and C, but must be loaded at most once.
        assert!(
            load_counts.get(&d).copied().unwrap_or(0) <= 1,
            "shared ancestor D should not be loaded more than once: {load_counts:?}"
        );
    }

    /// A graph deeper than `max_walk` with no reachable tag aborts with
    /// `TraversalLimitExceeded` rather than walking the whole history.
    #[test]
    fn test_find_nearest_tag_traversal_cap() {
        let _guard = set_hash_kind_for_test(HashKind::Sha1);
        // A linear chain 1→2→3→4→5 with no tags; cap the walk at 3.
        let chain: Vec<ObjectHash> = (1..=5).map(test_hash).collect();
        let mut graph: HashMap<ObjectHash, Vec<ObjectHash>> = HashMap::new();
        for pair in chain.windows(2) {
            graph.insert(pair[0], vec![pair[1]]);
        }
        graph.insert(chain[4], vec![]);
        let empty_tags: HashMap<ObjectHash, TagInfo> = HashMap::new();

        let result = find_nearest_tag(chain[0], &empty_tags, false, 3, |hash| {
            Ok(graph.get(hash).cloned().unwrap_or_default())
        });

        assert!(
            matches!(
                result,
                Err(DescribeError::TraversalLimitExceeded { limit: 3 })
            ),
            "expected TraversalLimitExceeded, got {result:?}"
        );
    }

    /// `--first-parent` must ignore the second parent of a merge commit: from a
    /// merge M with parents [P1, P2], only P1's chain is followed.
    #[test]
    fn test_find_nearest_tag_first_parent_skips_second_parent() {
        let _guard = set_hash_kind_for_test(HashKind::Sha1);
        let (m, p1, p2) = (test_hash(10), test_hash(11), test_hash(12));
        // M → [P1, P2]; P1 tagged "first", P2 tagged "second".
        let graph: HashMap<ObjectHash, Vec<ObjectHash>> =
            HashMap::from([(m, vec![p1, p2]), (p1, vec![]), (p2, vec![])]);
        let mut tag_map = HashMap::new();
        tag_map.insert(
            p1,
            TagInfo {
                name: "first".to_string(),
                is_annotated: true,
            },
        );
        tag_map.insert(
            p2,
            TagInfo {
                name: "second".to_string(),
                is_annotated: true,
            },
        );

        let mut visited_second = false;
        let result = find_nearest_tag(m, &tag_map, true, 100, |hash| {
            if *hash == p2 {
                visited_second = true;
            }
            Ok(graph.get(hash).cloned().unwrap_or_default())
        })
        .expect("walk should succeed");

        assert_eq!(result, Some(("first".to_string(), 1)));
        assert!(
            !visited_second,
            "--first-parent must not visit the second parent"
        );
    }

    /// The `--contains` reverse walk must deduplicate a diamond: the shared
    /// ancestor (the target) is reached via both paths but loaded at most once.
    #[test]
    fn test_contains_walker_visited_dedup_on_diamond() {
        let _guard = set_hash_kind_for_test(HashKind::Sha1);
        let (tip, b, c, target) = (test_hash(1), test_hash(2), test_hash(3), test_hash(4));
        // tip → b, c ; b → target ; c → target.
        let graph: HashMap<ObjectHash, Vec<ObjectHash>> =
            HashMap::from([(tip, vec![b, c]), (b, vec![target]), (c, vec![target])]);

        let mut load_counts: HashMap<ObjectHash, usize> = HashMap::new();
        let distance = distance_to_target(tip, target, false, 100, |hash| {
            *load_counts.entry(*hash).or_insert(0) += 1;
            Ok(graph.get(hash).cloned().unwrap_or_default())
        })
        .expect("walk should succeed");

        assert_eq!(distance, Some(2));
        // The target is found (curr == target) before being loaded, so it is
        // never expanded; intermediate nodes load exactly once.
        assert!(
            load_counts.get(&b).copied().unwrap_or(0) <= 1
                && load_counts.get(&c).copied().unwrap_or(0) <= 1,
            "diamond ancestors must not be loaded more than once: {load_counts:?}"
        );
    }

    /// A target deeper than `max_walk` aborts with `TraversalLimitExceeded`
    /// instead of walking the whole history.
    #[test]
    fn test_contains_walker_traversal_cap() {
        let _guard = set_hash_kind_for_test(HashKind::Sha1);
        // Linear chain 1→2→3→4→5; target is the deepest, cap the walk at 3.
        let chain: Vec<ObjectHash> = (1..=5).map(test_hash).collect();
        let mut graph: HashMap<ObjectHash, Vec<ObjectHash>> = HashMap::new();
        for pair in chain.windows(2) {
            graph.insert(pair[0], vec![pair[1]]);
        }
        let target = chain[4];

        let result = distance_to_target(chain[0], target, false, 3, |hash| {
            Ok(graph.get(hash).cloned().unwrap_or_default())
        });

        assert!(
            matches!(
                result,
                Err(DescribeError::TraversalLimitExceeded { limit: 3 })
            ),
            "expected TraversalLimitExceeded, got {result:?}"
        );
    }

    /// `find_best_candidate_tag` returns the nearest tag and honors the cap.
    #[test]
    fn test_find_best_candidate_tag_picks_nearest() {
        let _guard = set_hash_kind_for_test(HashKind::Sha1);
        // start → a → b ; a tagged "near" (dist 1), b tagged "far" (dist 2).
        let (start, a, b) = (test_hash(1), test_hash(2), test_hash(3));
        let graph: HashMap<ObjectHash, Vec<ObjectHash>> =
            HashMap::from([(start, vec![a]), (a, vec![b])]);
        let mut tag_map = HashMap::new();
        tag_map.insert(
            a,
            TagInfo {
                name: "near".to_string(),
                is_annotated: true,
            },
        );
        tag_map.insert(
            b,
            TagInfo {
                name: "far".to_string(),
                is_annotated: true,
            },
        );

        let result = find_best_candidate_tag(start, &tag_map, false, 100, 5, |hash| {
            Ok(graph.get(hash).cloned().unwrap_or_default())
        })
        .expect("walk should succeed");
        assert_eq!(result, Some(("near".to_string(), 1)));
    }
}
