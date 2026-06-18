//! Pull command combining fetch with merge or rebase depending on options, handling fast-forward checks and remote tracking setup.

use std::io::Write;

use clap::Parser;
use git_internal::errors::GitError;
use serde::Serialize;

use super::{fetch, merge, rebase};
use crate::{
    internal::{
        config::{ConfigKv, RemoteConfig},
        head::Head,
    },
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, ProgressMode, emit_json_data},
    },
};

const PULL_EXAMPLES: &str = "\
EXAMPLES:
    libra pull                             Pull from tracking remote
    libra pull origin main                 Pull specific branch from origin
    libra pull --ff-only                   Refuse to create a merge commit
    libra pull --no-ff                     Always create a merge commit
    libra pull --rebase                    Rebase the current branch onto the upstream
    libra pull --depth 1                   Shallow-fetch then integrate
    libra pull --json                      Structured JSON output for agents
    libra pull --quiet                     Suppress progress output

NOTES:
    By default pull uses the same merge engine as `libra merge`, including
    clean three-way merges and merge-state conflicts. Use --ff-only to reject
    divergent histories instead of creating a merge commit, or --no-ff to force
    a merge commit even when a fast-forward is possible. Use --rebase to replay
    local-only commits onto the upstream tip instead, and --depth to limit the
    fetch to a shallow history before integrating.";

/// Fetch from a remote and integrate changes into the current branch.
// EXAMPLES are wired via `#[command(after_help = PULL_EXAMPLES)]` and render
// at the bottom of `libra pull --help`. The meta-commentary that used to live
// here as a `///` line leaked into clap's `--help` body.
#[derive(Parser, Debug)]
#[command(after_help = PULL_EXAMPLES)]
pub struct PullArgs {
    /// The repository to pull from
    repository: Option<String>,

    /// The refspec to pull, usually a branch name
    #[clap(requires("repository"))]
    refspec: Option<String>,

    /// Rebase the current branch onto the upstream after fetching instead of merging
    #[clap(long, short = 'r')]
    rebase: bool,

    /// Refuse to merge unless the upstream can be fast-forwarded
    #[clap(long = "ff-only", conflicts_with_all = ["rebase", "ff", "no_ff"])]
    ff_only: bool,

    /// Allow a fast-forward merge (the default; clears --no-ff)
    #[clap(long, conflicts_with_all = ["no_ff", "ff_only", "rebase"])]
    ff: bool,

    /// Always create a merge commit even when fast-forward is possible
    #[clap(long = "no-ff", conflicts_with_all = ["ff", "ff_only", "rebase"])]
    no_ff: bool,

    /// Limit the fetch to the given number of commits from each tip (shallow fetch)
    #[clap(long, conflicts_with = "rebase")]
    depth: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PullRefUpdate {
    pub remote_ref: String,
    pub old_oid: Option<String>,
    pub new_oid: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PullFetchResult {
    pub remote: String,
    pub url: String,
    pub refs_updated: Vec<PullRefUpdate>,
    pub objects_fetched: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct PullMergeResult {
    pub strategy: String,
    /// The previous HEAD commit before merge (None for root commits).
    pub old_commit: Option<String>,
    pub commit: Option<String>,
    pub files_changed: usize,
    pub up_to_date: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parents: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflicted_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub aborted: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub continued: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PullRebaseResult {
    /// One of `"fast-forwarded"`, `"already-up-to-date"`, `"completed"`, or `"no-commits"`.
    pub status: String,
    /// HEAD before the rebase.
    pub old_commit: String,
    /// HEAD after the rebase.
    pub commit: String,
    /// Number of commits replayed onto the upstream tip.
    pub replay_count: usize,
    /// True when the rebase did not move HEAD.
    pub up_to_date: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PullOutput {
    pub branch: String,
    pub upstream: String,
    pub fetch: PullFetchResult,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merge: Option<PullMergeResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rebase: Option<PullRebaseResult>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum PullError {
    #[error("you are not currently on a branch")]
    NotOnBranch,

    #[error("there is no tracking information for the current branch")]
    NoTrackingInfo { branch: String },

    #[error("remote '{0}' not found")]
    RemoteNotFound(String),

    #[error("pull failed during fetch phase: {0}")]
    Fetch(#[source] fetch::FetchError),

    #[error("pull failed during merge phase: {0}")]
    Merge(#[source] merge::PullMergeError),

    #[error("pull failed during rebase phase: {0}")]
    Rebase(#[source] rebase::RebaseError),
}

impl From<PullError> for CliError {
    fn from(error: PullError) -> Self {
        match error {
            PullError::NotOnBranch => CliError::failure("you are not currently on a branch")
                .with_stable_code(StableErrorCode::RepoStateInvalid)
                .with_hint("checkout a branch before pulling")
                .with_hint("use 'libra switch <branch>' to switch"),
            PullError::NoTrackingInfo { .. } => {
                CliError::failure("there is no tracking information for the current branch")
                    .with_stable_code(StableErrorCode::RepoStateInvalid)
                    .with_hint("specify the remote and branch: 'libra pull <remote> <branch>'")
                    .with_hint(
                        "or set upstream with 'libra branch --set-upstream-to=<remote>/<branch>'",
                    )
            }
            PullError::RemoteNotFound(remote) => {
                CliError::command_usage(format!("remote '{remote}' not found"))
                    .with_stable_code(StableErrorCode::CliInvalidTarget)
                    .with_hint("use 'libra remote -v' to see configured remotes")
            }
            PullError::Fetch(error) => map_fetch_error_to_cli(&error).with_detail("phase", "fetch"),
            PullError::Merge(error) => map_merge_error_to_cli(&error).with_detail("phase", "merge"),
            PullError::Rebase(error) => CliError::from(error).with_detail("phase", "rebase"),
        }
    }
}

impl PullArgs {
    pub fn make(repository: Option<String>, refspec: Option<String>) -> Self {
        Self {
            repository,
            refspec,
            rebase: false,
            ff_only: false,
            ff: false,
            no_ff: false,
            depth: None,
        }
    }
}

#[derive(Debug, Clone)]
struct ResolvedPullTarget {
    branch: String,
    upstream: String,
    merge_target: String,
    remote_branch: String,
    remote_config: RemoteConfig,
}

pub async fn execute(args: PullArgs) {
    if let Err(err) = execute_safe(args, &OutputConfig::default()).await {
        err.print_stderr();
    }
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting.
///
/// # Side Effects
/// - Resolves the remote/upstream target for the current branch or CLI args.
/// - Fetches remote objects and updates remote-tracking refs.
/// - Fast-forwards the current branch and working tree when merge succeeds.
/// - Renders pull summary output.
///
/// # Errors
/// Returns [`CliError`] when the pull target cannot be resolved, fetch fails,
/// histories cannot be merged safely, or refs/worktree updates fail.
pub async fn execute_safe(args: PullArgs, output: &OutputConfig) -> CliResult<()> {
    let result = run_pull(args, output).await.map_err(CliError::from)?;
    render_pull_output(&result, output)
}

pub(crate) async fn run_pull(
    args: PullArgs,
    output: &OutputConfig,
) -> Result<PullOutput, PullError> {
    let target = resolve_pull_target(&args).await?;
    let child_output = child_output_for_pull(output);

    let fetch_result = fetch::fetch_repository_with_result(
        target.remote_config.clone(),
        Some(target.remote_branch.clone()),
        false,
        args.depth,
        false,
        &child_output,
    )
    .await
    .map_err(PullError::Fetch)?;

    let fetch_summary = PullFetchResult {
        remote: fetch_result.remote,
        url: fetch_result.url,
        refs_updated: fetch_result
            .refs_updated
            .into_iter()
            .map(|update| PullRefUpdate {
                remote_ref: update.remote_ref,
                old_oid: update.old_oid,
                new_oid: update.new_oid,
            })
            .collect(),
        objects_fetched: fetch_result.objects_fetched,
    };

    if args.rebase {
        // Rebase resolves `<remote>/<branch>` through the same public ref
        // path used by `libra rebase`, so keep the human-readable upstream form.
        let rebase_summary = rebase::run_rebase_for_pull(&target.upstream)
            .await
            .map_err(PullError::Rebase)?;
        let up_to_date = matches!(
            rebase_summary.status.as_str(),
            "already-up-to-date" | "no-commits"
        ) || rebase_summary.old_commit == rebase_summary.commit;
        return Ok(PullOutput {
            branch: target.branch,
            upstream: target.upstream,
            fetch: fetch_summary,
            merge: None,
            rebase: Some(PullRebaseResult {
                status: rebase_summary.status,
                old_commit: rebase_summary.old_commit,
                commit: rebase_summary.commit,
                replay_count: rebase_summary.replay_count,
                up_to_date,
            }),
        });
    }

    let merge_result = merge::run_merge_for_pull_with_options(
        &target.merge_target,
        &target.upstream,
        &child_output,
        merge::PullMergeOptions {
            ff_only: args.ff_only,
            no_ff: args.no_ff,
            message: None,
            squash: false,
            no_commit: false,
        },
    )
    .await
    .map_err(PullError::Merge)?;

    Ok(PullOutput {
        branch: target.branch,
        upstream: target.upstream,
        fetch: fetch_summary,
        merge: Some(PullMergeResult {
            strategy: merge_result.strategy,
            old_commit: merge_result.old_commit,
            commit: merge_result.commit,
            files_changed: merge_result.files_changed,
            up_to_date: merge_result.up_to_date,
            parents: merge_result.parents,
            conflicted_paths: merge_result.conflicted_paths,
            aborted: merge_result.aborted,
            continued: merge_result.continued,
        }),
        rebase: None,
    })
}

async fn resolve_pull_target(args: &PullArgs) -> Result<ResolvedPullTarget, PullError> {
    let branch = match Head::current().await {
        Head::Branch(name) => name,
        Head::Detached(_) => return Err(PullError::NotOnBranch),
    };

    match (&args.repository, &args.refspec) {
        (Some(remote), Some(refspec)) => {
            let remote_branch = normalize_remote_branch_name(refspec);
            let remote_config = ConfigKv::remote_config(remote)
                .await
                .ok()
                .flatten()
                .ok_or_else(|| PullError::RemoteNotFound(remote.clone()))?;
            Ok(ResolvedPullTarget {
                branch,
                upstream: format!("{remote}/{remote_branch}"),
                merge_target: format!("refs/remotes/{remote}/{remote_branch}"),
                remote_branch,
                remote_config,
            })
        }
        (Some(remote), None) => {
            let remote_config = ConfigKv::remote_config(remote)
                .await
                .ok()
                .flatten()
                .ok_or_else(|| PullError::RemoteNotFound(remote.clone()))?;
            Ok(ResolvedPullTarget {
                upstream: format!("{remote}/{branch}"),
                merge_target: format!("refs/remotes/{remote}/{branch}"),
                remote_branch: branch.clone(),
                branch,
                remote_config,
            })
        }
        (None, None) => {
            let branch_config = ConfigKv::branch_config(&branch)
                .await
                .ok()
                .flatten()
                .ok_or_else(|| PullError::NoTrackingInfo {
                    branch: branch.clone(),
                })?;
            let remote_config = ConfigKv::remote_config(&branch_config.remote)
                .await
                .ok()
                .flatten()
                .ok_or_else(|| PullError::RemoteNotFound(branch_config.remote.clone()))?;
            Ok(ResolvedPullTarget {
                branch,
                upstream: format!("{}/{}", branch_config.remote, branch_config.merge),
                merge_target: format!(
                    "refs/remotes/{}/{}",
                    branch_config.remote, branch_config.merge
                ),
                remote_branch: branch_config.merge,
                remote_config,
            })
        }
        (None, Some(_)) => unreachable!("clap requires repository when refspec is provided"),
    }
}

fn child_output_for_pull(output: &OutputConfig) -> OutputConfig {
    let mut child = output.clone();
    if output.is_json() || output.quiet {
        child.progress = ProgressMode::None;
        child.progress_preference = crate::utils::output::ProgressPreference::None;
    }
    child
}

fn render_pull_output(result: &PullOutput, output: &OutputConfig) -> CliResult<()> {
    if output.is_json() {
        return emit_json_data("pull", result, output);
    }

    if output.quiet {
        return Ok(());
    }

    let stdout = std::io::stdout();
    let mut writer = stdout.lock();
    writeln!(writer, "From {}", result.fetch.url)
        .map_err(|error| CliError::io(format!("failed to write pull summary: {error}")))?;

    for update in &result.fetch.refs_updated {
        let ref_name = update
            .remote_ref
            .strip_prefix("refs/remotes/")
            .unwrap_or(&update.remote_ref);
        let new_short = short_oid(&update.new_oid);
        if let Some(old_oid) = &update.old_oid {
            writeln!(
                writer,
                "   {}..{}  {}",
                short_oid(old_oid),
                new_short,
                ref_name
            )
            .map_err(|error| CliError::io(format!("failed to write pull summary: {error}")))?;
        } else {
            writeln!(writer, " * {}  {}", new_short, ref_name)
                .map_err(|error| CliError::io(format!("failed to write pull summary: {error}")))?;
        }
    }

    if let Some(rebase) = &result.rebase {
        render_pull_rebase_summary(&mut writer, &result.upstream, rebase)?;
        return Ok(());
    }

    let Some(merge) = &result.merge else {
        return Ok(());
    };

    if merge.up_to_date {
        writeln!(writer, "Already up to date.")
            .map_err(|error| CliError::io(format!("failed to write pull summary: {error}")))?;
        return Ok(());
    }

    if let (Some(old), Some(new)) = (&merge.old_commit, &merge.commit) {
        writeln!(writer, "Updating {}..{}", short_oid(old), short_oid(new))
            .map_err(|error| CliError::io(format!("failed to write pull summary: {error}")))?;
    }
    match merge.strategy.as_str() {
        "three-way" => writeln!(writer, "Merge made by the 'three-way' strategy."),
        _ => writeln!(writer, "Fast-forward"),
    }
    .map_err(|error| CliError::io(format!("failed to write pull summary: {error}")))?;
    if merge.files_changed > 0 {
        let noun = if merge.files_changed == 1 {
            "file"
        } else {
            "files"
        };
        writeln!(writer, " {} {} changed", merge.files_changed, noun)
            .map_err(|error| CliError::io(format!("failed to write pull summary: {error}")))?;
    }
    Ok(())
}

fn render_pull_rebase_summary<W: Write>(
    writer: &mut W,
    upstream: &str,
    rebase: &PullRebaseResult,
) -> CliResult<()> {
    let map_io_err =
        |error: std::io::Error| CliError::io(format!("failed to write pull summary: {error}"));
    if rebase.up_to_date {
        writeln!(
            writer,
            "Current branch is already up to date with '{upstream}'."
        )
        .map_err(map_io_err)?;
        return Ok(());
    }
    match rebase.status.as_str() {
        "already-up-to-date" | "no-commits" => {
            writeln!(
                writer,
                "Current branch is already up to date with '{upstream}'."
            )
            .map_err(map_io_err)?;
        }
        "fast-forwarded" => {
            writeln!(
                writer,
                "Fast-forwarded onto '{upstream}' ({}..{}).",
                short_oid(&rebase.old_commit),
                short_oid(&rebase.commit),
            )
            .map_err(map_io_err)?;
        }
        _ => {
            let commits_noun = if rebase.replay_count == 1 {
                "commit"
            } else {
                "commits"
            };
            writeln!(
                writer,
                "Successfully rebased {count} {noun} onto '{upstream}' ({old}..{new}).",
                count = rebase.replay_count,
                noun = commits_noun,
                old = short_oid(&rebase.old_commit),
                new = short_oid(&rebase.commit),
                upstream = upstream,
            )
            .map_err(map_io_err)?;
        }
    }
    Ok(())
}

fn short_oid(oid: &str) -> &str {
    oid.get(..7).unwrap_or(oid)
}

fn normalize_remote_branch_name(branch: &str) -> String {
    branch
        .strip_prefix("refs/heads/")
        .unwrap_or(branch)
        .to_string()
}

fn map_fetch_error_to_cli(error: &fetch::FetchError) -> CliError {
    match error {
        fetch::FetchError::InvalidRemoteSpec { kind, reason, .. } => match kind {
            fetch::RemoteSpecErrorKind::MissingLocalRepo => {
                CliError::fatal(reason.clone()).with_stable_code(StableErrorCode::RepoNotFound)
            }
            fetch::RemoteSpecErrorKind::InvalidLocalRepo
            | fetch::RemoteSpecErrorKind::MalformedUrl
            | fetch::RemoteSpecErrorKind::UnsupportedScheme => {
                CliError::command_usage(reason.clone())
                    .with_stable_code(StableErrorCode::CliInvalidTarget)
            }
        },
        fetch::FetchError::Discovery { source, .. } => {
            map_fetch_discovery_error(error.to_string(), source)
        }
        fetch::FetchError::FetchObjects { source, .. } => map_fetch_io_error(
            error.to_string(),
            source,
            StableErrorCode::NetworkUnavailable,
        )
        .with_hint("check network connectivity and retry"),
        fetch::FetchError::PacketRead { source } => {
            if is_timeout_io_error(source) {
                return CliError::fatal(error.to_string())
                    .with_stable_code(StableErrorCode::NetworkUnavailable)
                    .with_hint("check network connectivity and retry");
            }
            CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::NetworkProtocol)
        }
        fetch::FetchError::RemoteBranchNotFound { .. } => {
            CliError::command_usage(error.to_string())
                .with_stable_code(StableErrorCode::CliInvalidTarget)
                .with_hint("verify the remote branch name and try again")
        }
        fetch::FetchError::ObjectFormatMismatch { .. } => {
            CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::RepoStateInvalid)
        }
        fetch::FetchError::InvalidPktHeader { .. }
        | fetch::FetchError::RemoteSideband { .. }
        | fetch::FetchError::ChecksumMismatch
        | fetch::FetchError::IndexPack { .. } => {
            CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::NetworkProtocol)
        }
        fetch::FetchError::ObjectsDirNotFound { .. } => {
            CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::IoReadFailed)
        }
        fetch::FetchError::PackDirCreate { .. }
        | fetch::FetchError::PackWrite { .. }
        | fetch::FetchError::UpdateRefs { .. } => {
            CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::IoWriteFailed)
        }
        fetch::FetchError::LocalState { .. } => {
            CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::RepoCorrupt)
        }
    }
}

fn map_fetch_discovery_error(message: String, source: &GitError) -> CliError {
    match source {
        GitError::UnAuthorized(_) => CliError::fatal(message)
            .with_stable_code(StableErrorCode::AuthPermissionDenied)
            .with_hint("check SSH key / HTTP credentials and repository access rights"),
        GitError::NetworkError(_) => CliError::fatal(message)
            .with_stable_code(StableErrorCode::NetworkUnavailable)
            .with_hint("check network connectivity and retry"),
        GitError::IOError(error) => {
            map_fetch_io_error(message, error, StableErrorCode::NetworkUnavailable)
                .with_hint("check network connectivity and retry")
        }
        _ => CliError::fatal(message).with_stable_code(StableErrorCode::NetworkProtocol),
    }
}

fn map_fetch_io_error(
    message: String,
    error: &std::io::Error,
    default_code: StableErrorCode,
) -> CliError {
    if is_timeout_io_error(error) {
        CliError::fatal(message).with_stable_code(StableErrorCode::NetworkUnavailable)
    } else {
        CliError::fatal(message).with_stable_code(default_code)
    }
}

fn is_timeout_io_error(error: &std::io::Error) -> bool {
    if error.kind() == std::io::ErrorKind::TimedOut {
        return true;
    }
    let lower = error.to_string().to_lowercase();
    lower.contains("timeout") || lower.contains("timed out")
}

fn map_merge_error_to_cli(error: &merge::PullMergeError) -> CliError {
    match error {
        merge::PullMergeError::MissingAction | merge::PullMergeError::ConflictingAction => {
            CliError::command_usage(error.to_string())
                .with_stable_code(StableErrorCode::CliInvalidArguments)
        }
        merge::PullMergeError::InvalidTarget(..) => CliError::command_usage(error.to_string())
            .with_stable_code(StableErrorCode::CliInvalidTarget),
        merge::PullMergeError::TargetLoad { .. }
        | merge::PullMergeError::CurrentLoad { .. }
        | merge::PullMergeError::History(..)
        | merge::PullMergeError::TreeLoad { .. }
        | merge::PullMergeError::ObjectLoad { .. } => {
            CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::RepoCorrupt)
        }
        merge::PullMergeError::UnrelatedHistories => {
            CliError::failure(error.to_string()).with_stable_code(StableErrorCode::RepoStateInvalid)
        }
        merge::PullMergeError::NonFastForward { .. } => CliError::failure(error.to_string())
            .with_stable_code(StableErrorCode::ConflictOperationBlocked)
            .with_hint("run 'libra pull' without --ff-only to allow a merge commit")
            .with_hint("or run 'libra pull --rebase' to replay local commits"),
        merge::PullMergeError::Conflicts { .. }
        | merge::PullMergeError::DirtyWorktree
        | merge::PullMergeError::UntrackedOverwrite { .. }
        | merge::PullMergeError::MergeInProgress
        | merge::PullMergeError::UnresolvedConflicts => CliError::failure(error.to_string())
            .with_stable_code(StableErrorCode::ConflictOperationBlocked)
            .with_hint("resolve conflicts, then run 'libra merge --continue'")
            .with_hint("or run 'libra merge --abort' to restore the pre-merge state"),
        merge::PullMergeError::NoMergeInProgress => {
            CliError::failure(error.to_string()).with_stable_code(StableErrorCode::RepoStateInvalid)
        }
        merge::PullMergeError::StateLoad(..) | merge::PullMergeError::IndexLoad(..) => {
            CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::IoReadFailed)
        }
        merge::PullMergeError::StateSave(..)
        | merge::PullMergeError::StateCleanup(..)
        | merge::PullMergeError::IndexSave(..)
        | merge::PullMergeError::TreeCreate(..)
        | merge::PullMergeError::CommitSave(..)
        | merge::PullMergeError::WorkdirReset(..) => {
            CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::IoWriteFailed)
        }
        merge::PullMergeError::HeadResolve(..) => {
            CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::IoReadFailed)
        }
        merge::PullMergeError::HeadUpdate(..) | merge::PullMergeError::Restore(..) => {
            CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::IoWriteFailed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depth_and_no_ff_flags_parse() {
        let args = PullArgs::try_parse_from(["pull", "origin", "main", "--depth", "1", "--no-ff"])
            .expect("--depth 1 --no-ff should parse");
        assert_eq!(args.depth, Some(1));
        assert!(args.no_ff);
        assert!(!args.ff);

        // Absent flags keep their defaults (merge path, full-depth fetch).
        let bare = PullArgs::try_parse_from(["pull"]).expect("bare pull should parse");
        assert_eq!(bare.depth, None);
        assert!(!bare.no_ff);
        assert!(!bare.ff);
    }

    #[test]
    fn ff_conflicts_with_no_ff() {
        // `--ff` and `--no-ff` are mutually exclusive at the clap layer.
        assert!(PullArgs::try_parse_from(["pull", "--ff", "--no-ff"]).is_err());
        // `--no-ff` cannot be combined with `--rebase` (different integration paths).
        assert!(PullArgs::try_parse_from(["pull", "--no-ff", "--rebase"]).is_err());
        // `--depth` is fetch-only and rejected on the rebase path's flag set only
        // when combined with rebase; a plain `--depth` still parses.
        assert!(PullArgs::try_parse_from(["pull", "--depth", "2"]).is_ok());
    }

    #[test]
    fn test_map_fetch_discovery_error_unauthorized_matches_clone() {
        let cli = map_fetch_discovery_error(
            "remote discovery failed".to_string(),
            &GitError::UnAuthorized("permission denied".to_string()),
        );

        assert_eq!(cli.stable_code(), StableErrorCode::AuthPermissionDenied);
    }

    /// Pin the `Display` format for the static-message and direct-message
    /// variants of [`PullError`]. These strings are used as the
    /// `CliError` message via `From<PullError> for CliError` and
    /// surface in both human and `--json` envelopes.
    ///
    /// Source-chained variants (Fetch, Merge) wrap upstream
    /// FetchError / PullMergeError types and are intentionally
    /// skipped — their `{0}` slot is owned by the wrapped error.
    #[test]
    fn pull_error_display_pins_static_message_variants() {
        assert_eq!(
            PullError::NotOnBranch.to_string(),
            "you are not currently on a branch",
        );
        assert_eq!(
            PullError::NoTrackingInfo {
                branch: "main".to_string(),
            }
            .to_string(),
            "there is no tracking information for the current branch",
        );
        assert_eq!(
            PullError::RemoteNotFound("origin".to_string()).to_string(),
            "remote 'origin' not found",
        );
    }
}
