//! Implements `rev-list` to enumerate commits reachable from a revision.

use std::{collections::HashSet, io::Write};

use clap::Parser;
use git_internal::{hash::ObjectHash, internal::object::commit::Commit};
use serde::Serialize;

use crate::{
    command::merge_base::{self, MergeBaseError},
    utils::{
        error::{CliError, CliResult, StableErrorCode, emit_warning},
        graph::{CommitWalker, TreeWalkObject, TreeWalkObjectKind, TreeWalker},
        output::{OutputConfig, emit_json_data},
        util::{self, CommitBaseError},
    },
};

/// `--help` examples shown in `libra rev-list --help` output.
///
/// `rev-list` walks the commit graph from the given spec (default
/// `HEAD`) and prints each reachable commit hash on its own line. The
/// banner pins the default `HEAD` walk, an explicit branch walk, a
/// quiet form, and a JSON variant for agents so users see all
/// supported forms without reading the design doc. Cross-cutting
/// `--help` EXAMPLES rollout per `docs/improvement/README.md` item B.
pub const REV_LIST_EXAMPLES: &str = "\
EXAMPLES:
    libra rev-list                  Walk ancestry from HEAD (one hash per line)
    libra rev-list main             Walk ancestry from refs/heads/main
    libra rev-list HEAD~5           Walk ancestry from a relative ref
    libra rev-list main..HEAD       Commits reachable from HEAD but not main (range)
    libra rev-list -n 10 HEAD       Limit output to the 10 newest commits
    libra rev-list --count HEAD     Print only the count of reachable commits
    libra rev-list --objects HEAD   Include reachable tree/blob object IDs
    libra rev-list --json HEAD      Structured JSON output (input + commits[] + total)
    libra rev-list --quiet HEAD     Suppress stdout (use exit code as truthy probe)";

#[derive(Parser, Debug)]
#[command(after_help = REV_LIST_EXAMPLES)]
pub struct RevListArgs {
    /// Revisions to list from. Defaults to HEAD when omitted. Accepts multiple
    /// specs, exclusions (`^<rev>`), and ranges (`A..B`, `A...B`).
    #[clap(value_name = "SPEC")]
    pub specs: Vec<String>,

    /// Limit the number of commits output (`-n`/`--max-count`).
    #[clap(short = 'n', long = "max-count", value_name = "N")]
    pub max_count: Option<usize>,

    /// Skip the first N commits of the filtered output.
    #[clap(long, value_name = "N")]
    pub skip: Option<usize>,

    /// Print only the count of commits, not the hashes.
    #[clap(long)]
    pub count: bool,

    /// Show only merge commits (two or more parents).
    #[clap(long, conflicts_with = "no_merges")]
    pub merges: bool,

    /// Show only non-merge commits (fewer than two parents).
    #[clap(long)]
    pub no_merges: bool,

    /// Show only commits with at least N parents.
    #[clap(long = "min-parents", value_name = "N")]
    pub min_parents: Option<usize>,

    /// Show only commits with at most N parents.
    #[clap(long = "max-parents", value_name = "N")]
    pub max_parents: Option<usize>,

    /// Print parent hashes after each commit hash.
    #[clap(long)]
    pub parents: bool,

    /// Prefix each line with the commit's Unix timestamp.
    #[clap(long)]
    pub timestamp: bool,

    /// Print reachable tree/blob object IDs after commit IDs.
    #[clap(long)]
    pub objects: bool,
}

#[derive(Debug, Clone, Serialize)]
struct RevListOutput {
    input: String,
    commits: Vec<String>,
    total: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    objects: Vec<RevListObject>,
}

#[derive(Debug, Clone, Serialize)]
struct RevListObject {
    hash: String,
    path: String,
    #[serde(rename = "type")]
    object_type: String,
}

pub async fn execute(args: RevListArgs) -> Result<(), String> {
    execute_safe(args, &OutputConfig::default())
        .await
        .map_err(|err| err.render())
}

pub async fn execute_safe(args: RevListArgs, output: &OutputConfig) -> CliResult<()> {
    util::require_repo().map_err(|_| CliError::repo_not_found())?;
    let resolved = resolve_rev_list(&args).await?;

    if output.is_json() {
        // JSON envelope always carries the bare commit-hash list (`--parents`
        // /`--timestamp` only affect text rendering, and `--count` does not
        // suppress the list in JSON mode), keeping the schema stable.
        let commits: Vec<String> = resolved.commits.iter().map(|c| c.id.to_string()).collect();
        let total = commits.len();
        let objects = resolved
            .objects_by_commit
            .iter()
            .flat_map(|objects| objects.iter().cloned())
            .collect();
        let result = RevListOutput {
            input: resolved.input,
            commits,
            total,
            objects,
        };
        return emit_json_data("rev-list", &result, output);
    }
    if output.quiet {
        return Ok(());
    }
    if args.count {
        println!("{}", resolved.commits.len());
        return Ok(());
    }

    let stdout = std::io::stdout();
    let mut writer = stdout.lock();
    write_rev_list_lines(
        &mut writer,
        &resolved.commits,
        &resolved.objects_by_commit,
        args.parents,
        args.timestamp,
        args.objects,
    )
}

/// Render one commit line honoring `--timestamp` (`<ts> <hash>`) and
/// `--parents` (`<hash> <p1> <p2>…`); combined yields `<ts> <hash> <parents>`.
fn format_rev_list_line(commit: &Commit, parents: bool, timestamp: bool) -> String {
    let mut line = String::new();
    if timestamp {
        line.push_str(&commit.committer.timestamp.to_string());
        line.push(' ');
    }
    line.push_str(&commit.id.to_string());
    if parents {
        for parent in &commit.parent_commit_ids {
            line.push(' ');
            line.push_str(&parent.to_string());
        }
    }
    line
}

fn write_rev_list_lines<W: Write>(
    writer: &mut W,
    commits: &[Commit],
    objects_by_commit: &[Vec<RevListObject>],
    parents: bool,
    timestamp: bool,
    objects: bool,
) -> CliResult<()> {
    for (index, commit) in commits.iter().enumerate() {
        let line = format_rev_list_line(commit, parents, timestamp);
        match writeln!(writer, "{line}") {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => return Ok(()),
            Err(error) => {
                return Err(
                    CliError::fatal(format!("failed to write rev-list output: {error}"))
                        .with_stable_code(StableErrorCode::IoWriteFailed),
                );
            }
        }
        if objects && let Some(commit_objects) = objects_by_commit.get(index) {
            for object in commit_objects {
                write_rev_list_object_line(writer, object)?;
            }
        }
    }
    Ok(())
}

fn write_rev_list_object_line<W: Write>(writer: &mut W, object: &RevListObject) -> CliResult<()> {
    let line = if object.path.is_empty() {
        object.hash.clone()
    } else {
        format!("{} {}", object.hash, object.path)
    };
    match writeln!(writer, "{line}") {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(
            CliError::fatal(format!("failed to write rev-list output: {error}"))
                .with_stable_code(StableErrorCode::IoWriteFailed),
        ),
    }
}

/// Resolved rev-list output: the filtered, sorted, and sliced commit set plus
/// the human-readable input label.
struct ResolvedRevList {
    input: String,
    commits: Vec<Commit>,
    objects_by_commit: Vec<Vec<RevListObject>>,
}

/// Resolve a single endpoint of a spec (empty defaults to `HEAD`).
async fn resolve_endpoint(endpoint: &str) -> CliResult<ObjectHash> {
    let spec = if endpoint.is_empty() {
        "HEAD"
    } else {
        endpoint
    };
    util::get_commit_base_typed(spec)
        .await
        .map_err(|err| rev_list_target_error(spec, err))
}

/// Translate the `--merges`/`--no-merges`/`--min-parents`/`--max-parents`
/// flags into an inclusive `(min, max)` parent-count window. `--merges` raises
/// the floor to 2; `--no-merges` caps the ceiling at 1.
fn parent_bounds(args: &RevListArgs) -> (usize, Option<usize>) {
    let mut min = args.min_parents.unwrap_or(0);
    if args.merges {
        min = min.max(2);
    }
    let mut max = args.max_parents;
    if args.no_merges {
        max = Some(max.map_or(1, |existing| existing.min(1)));
    }
    (min, max)
}

fn map_merge_base_error(error: MergeBaseError) -> CliError {
    // `find_best_merge_bases` only fails while loading an ancestor commit; the
    // flattened `detail` cannot distinguish corruption from I/O, so classify
    // conservatively as RepoCorrupt (matching `rev_list_target_error`).
    CliError::fatal(format!("failed to compute symmetric range: {error}"))
        .with_stable_code(StableErrorCode::RepoCorrupt)
}

async fn resolve_rev_list(args: &RevListArgs) -> CliResult<ResolvedRevList> {
    let specs: Vec<String> = if args.specs.is_empty() {
        vec!["HEAD".to_string()]
    } else {
        args.specs.clone()
    };
    let input = specs.join(" ");

    // Desugar each spec into positive (included) and negative (excluded) tips:
    //   ^A        → exclude A
    //   A..B      → B, exclude A
    //   A...B     → A, B, exclude every best merge base of A and B
    //   A         → include A
    let mut positive_tips: Vec<ObjectHash> = Vec::new();
    let mut negative_tips: Vec<ObjectHash> = Vec::new();
    for spec in &specs {
        if let Some((lhs, rhs)) = spec.split_once("...") {
            let a = resolve_endpoint(lhs).await?;
            let b = resolve_endpoint(rhs).await?;
            positive_tips.push(a);
            positive_tips.push(b);
            for base in merge_base::find_best_merge_bases(a, b).map_err(map_merge_base_error)? {
                negative_tips.push(base);
            }
        } else if let Some((lhs, rhs)) = spec.split_once("..") {
            negative_tips.push(resolve_endpoint(lhs).await?);
            positive_tips.push(resolve_endpoint(rhs).await?);
        } else if let Some(rest) = spec.strip_prefix('^') {
            negative_tips.push(resolve_endpoint(rest).await?);
        } else {
            positive_tips.push(resolve_endpoint(spec).await?);
        }
    }

    // Excluded = everything reachable from any negative tip.
    let mut excluded: HashSet<ObjectHash> = HashSet::new();
    if !negative_tips.is_empty() {
        for commit in CommitWalker::new(&negative_tips, HashSet::new())?.collect()? {
            excluded.insert(commit.id);
        }
    }

    // Included = everything reachable from a positive tip and not excluded.
    let mut commits = CommitWalker::new(&positive_tips, excluded)?.collect()?;

    // Parent-count predicate first, then skip/limit on the surviving stream so
    // `--skip N` counts post-filter commits (matching Git).
    let (min_parents, max_parents) = parent_bounds(args);
    commits.retain(|commit| {
        let n = commit.parent_commit_ids.len();
        n >= min_parents && max_parents.is_none_or(|max| n <= max)
    });

    if let Some(skip) = args.skip {
        let skip = skip.min(commits.len());
        commits.drain(0..skip);
    }
    if let Some(max) = args.max_count {
        commits.truncate(max);
    }

    let objects_by_commit = if args.objects {
        collect_objects_by_commit(&commits)?
    } else {
        Vec::new()
    };

    Ok(ResolvedRevList {
        input,
        commits,
        objects_by_commit,
    })
}

#[cfg(test)]
fn sort_rev_list_commits(commits: &mut [git_internal::internal::object::commit::Commit]) {
    // `sort_by_key` is stable, so equal timestamps keep the traversal order
    // returned by `get_reachable_commits` (HEAD before parent in linear history).
    commits.sort_by_key(|commit| std::cmp::Reverse(commit.committer.timestamp));
}

fn collect_objects_by_commit(commits: &[Commit]) -> CliResult<Vec<Vec<RevListObject>>> {
    let mut seen_objects: HashSet<ObjectHash> = HashSet::new();
    let mut by_commit = Vec::with_capacity(commits.len());
    for commit in commits {
        let mut walker = TreeWalker::new(commit.tree_id);
        let mut objects = Vec::new();
        while let Some(object) = walker.next_object()? {
            for warning in walker.take_warnings() {
                emit_warning(warning);
            }
            if seen_objects.insert(object.id) {
                objects.push(rev_list_object_from_tree_walk(object));
            }
        }
        for warning in walker.take_warnings() {
            emit_warning(warning);
        }
        by_commit.push(objects);
    }
    Ok(by_commit)
}

fn rev_list_object_from_tree_walk(object: TreeWalkObject) -> RevListObject {
    let object_type = match object.kind {
        TreeWalkObjectKind::Tree => TreeWalkObjectKind::Tree.as_str(),
        TreeWalkObjectKind::Blob => TreeWalkObjectKind::Blob.as_str(),
    };
    RevListObject {
        hash: object.id.to_string(),
        path: object.path,
        object_type: object_type.to_string(),
    }
}

fn rev_list_target_error(spec: &str, error: CommitBaseError) -> CliError {
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

#[cfg(test)]
mod tests {
    use std::io::{self, Write};

    use clap::Parser;
    use git_internal::{
        hash::{ObjectHash, get_hash_kind},
        internal::object::{
            commit::Commit,
            signature::{Signature, SignatureType},
        },
    };

    use super::{
        RevListArgs, format_rev_list_line, parent_bounds, sort_rev_list_commits,
        write_rev_list_lines,
    };
    use crate::utils::error::StableErrorCode;

    struct FailingWriter {
        kind: io::ErrorKind,
    }

    impl Write for FailingWriter {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(self.kind, "test write failure"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn test_signature(timestamp: usize) -> Signature {
        Signature {
            signature_type: SignatureType::Committer,
            name: "tester".to_string(),
            email: "tester@example.com".to_string(),
            timestamp,
            timezone: "+0000".to_string(),
        }
    }

    fn test_hash(byte: u8) -> ObjectHash {
        ObjectHash::from_bytes(&vec![byte; get_hash_kind().size()])
            .expect("test hash bytes should match active hash kind")
    }

    fn test_commit(id: ObjectHash, timestamp: usize) -> Commit {
        Commit {
            id,
            tree_id: id,
            parent_commit_ids: Vec::new(),
            author: test_signature(timestamp),
            committer: test_signature(timestamp),
            message: "test".to_string(),
        }
    }

    #[test]
    fn test_rev_list_args_default() {
        let args = RevListArgs::try_parse_from(["rev-list"]).unwrap();
        assert!(args.specs.is_empty());
    }

    #[test]
    fn test_rev_list_args_with_spec() {
        let args = RevListArgs::try_parse_from(["rev-list", "HEAD~1"]).unwrap();
        assert_eq!(args.specs, vec!["HEAD~1".to_string()]);
    }

    #[test]
    fn test_rev_list_args_multi_spec() {
        let args = RevListArgs::try_parse_from(["rev-list", "main", "^origin/main"]).unwrap();
        assert_eq!(
            args.specs,
            vec!["main".to_string(), "^origin/main".to_string()]
        );
    }

    #[test]
    fn test_rev_list_args_filter_flags() {
        let args = RevListArgs::try_parse_from([
            "rev-list",
            "-n",
            "3",
            "--skip",
            "1",
            "--count",
            "--objects",
            "HEAD",
        ])
        .unwrap();
        assert_eq!(args.max_count, Some(3));
        assert_eq!(args.skip, Some(1));
        assert!(args.count);
        assert!(args.objects);
        assert_eq!(args.specs, vec!["HEAD".to_string()]);
    }

    #[test]
    fn test_rev_list_merges_conflicts_with_no_merges() {
        let err = RevListArgs::try_parse_from(["rev-list", "--merges", "--no-merges", "HEAD"]);
        assert!(err.is_err(), "--merges and --no-merges must conflict");
    }

    #[test]
    fn test_parent_bounds_merges_and_no_merges() {
        let merges = RevListArgs::try_parse_from(["rev-list", "--merges"]).unwrap();
        assert_eq!(parent_bounds(&merges), (2, None));

        let no_merges = RevListArgs::try_parse_from(["rev-list", "--no-merges"]).unwrap();
        assert_eq!(parent_bounds(&no_merges), (0, Some(1)));

        let explicit =
            RevListArgs::try_parse_from(["rev-list", "--min-parents", "1", "--max-parents", "2"])
                .unwrap();
        assert_eq!(parent_bounds(&explicit), (1, Some(2)));
    }

    #[test]
    fn test_format_rev_list_line_variants() {
        let id = test_hash(0xab);
        let parent = test_hash(0x01);
        let mut commit = test_commit(id, 1234);
        commit.parent_commit_ids = vec![parent];

        assert_eq!(format_rev_list_line(&commit, false, false), id.to_string());
        assert_eq!(
            format_rev_list_line(&commit, true, false),
            format!("{id} {parent}"),
        );
        assert_eq!(
            format_rev_list_line(&commit, false, true),
            format!("1234 {id}"),
        );
        assert_eq!(
            format_rev_list_line(&commit, true, true),
            format!("1234 {id} {parent}"),
        );
    }

    #[test]
    fn test_sort_rev_list_commits_preserves_equal_timestamp_order() {
        let high = test_hash(0xff);
        let low = test_hash(0x01);
        let mut commits = vec![test_commit(high, 1), test_commit(low, 1)];

        sort_rev_list_commits(&mut commits);

        assert_eq!(commits[0].id, high);
        assert_eq!(commits[1].id, low);
    }

    #[test]
    fn test_sort_rev_list_commits_orders_newest_first() {
        let old = test_hash(0x01);
        let new = test_hash(0xff);
        let mut commits = vec![test_commit(old, 1), test_commit(new, 2)];

        sort_rev_list_commits(&mut commits);

        assert_eq!(commits[0].id, new);
        assert_eq!(commits[1].id, old);
    }

    #[test]
    fn test_write_rev_list_output_maps_write_failure_to_write_code() {
        let mut writer = FailingWriter {
            kind: io::ErrorKind::PermissionDenied,
        };
        let commits = vec![test_commit(test_hash(0x01), 1)];

        let error = write_rev_list_lines(&mut writer, &commits, &[], false, false, false)
            .expect_err("write should fail");

        assert_eq!(error.stable_code(), StableErrorCode::IoWriteFailed);
    }

    #[test]
    fn test_write_rev_list_output_ignores_broken_pipe() {
        let mut writer = FailingWriter {
            kind: io::ErrorKind::BrokenPipe,
        };
        let commits = vec![test_commit(test_hash(0x01), 1)];

        write_rev_list_lines(&mut writer, &commits, &[], false, false, false)
            .expect("broken pipe should be ignored");
    }
}
