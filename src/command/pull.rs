//! Pull command combining fetch with merge or rebase depending on options, handling fast-forward checks and remote tracking setup.

use std::io::Write;

use clap::Parser;
use git_internal::errors::GitError;
use serde::Serialize;

use super::{fetch, merge};
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

/// Fetch from a remote and integrate changes into the current branch.
///
/// # Examples
///
/// ```text
/// libra pull                             Pull from tracking remote
/// libra pull origin main                 Pull specific branch from origin
/// libra pull --json                      Structured JSON output for agents
/// libra pull --quiet                     Suppress progress output
/// ```
#[derive(Parser, Debug)]
pub struct PullArgs {
    /// The repository to pull from
    repository: Option<String>,

    /// The refspec to pull, usually a branch name
    #[clap(requires("repository"))]
    refspec: Option<String>,
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
}

#[derive(Debug, Clone, Serialize)]
pub struct PullOutput {
    pub branch: String,
    pub upstream: String,
    pub fetch: PullFetchResult,
    pub merge: PullMergeResult,
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

    #[error("pull requires a non-fast-forward merge from '{upstream}', which is not yet supported")]
    ManualMergeRequired { upstream: String },

    #[error("pull failed during merge phase: {0}")]
    Merge(#[source] merge::PullMergeError),
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
            PullError::RemoteNotFound(remote) => CliError::command_usage(format!(
                "remote '{remote}' not found"
            ))
            .with_stable_code(StableErrorCode::CliInvalidTarget)
            .with_hint("use 'libra remote -v' to see configured remotes"),
            PullError::Fetch(error) => map_fetch_error_to_cli(&error).with_detail("phase", "fetch"),
            PullError::ManualMergeRequired { upstream } => CliError::failure(format!(
                "pull requires a non-fast-forward merge from '{upstream}', which is not yet supported"
            ))
            .with_stable_code(StableErrorCode::ConflictOperationBlocked)
            .with_hint(format!(
                "run 'libra fetch' first if needed, then merge manually with 'libra merge {upstream}'"
            ))
            .with_detail("phase", "merge"),
            PullError::Merge(error) => map_merge_error_to_cli(&error).with_detail("phase", "merge"),
        }
    }
}

impl PullArgs {
    pub fn make(repository: Option<String>, refspec: Option<String>) -> Self {
        Self {
            repository,
            refspec,
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
/// errors and exiting. Fetches from the remote then merges into the current
/// branch.
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
        None,
        &child_output,
    )
    .await
    .map_err(PullError::Fetch)?;

    let merge_result =
        merge::run_merge_for_pull(&target.merge_target, &target.upstream, &child_output)
            .await
            .map_err(|error| match error {
                merge::PullMergeError::ManualMergeRequired { upstream } => {
                    PullError::ManualMergeRequired { upstream }
                }
                other => PullError::Merge(other),
            })?;

    Ok(PullOutput {
        branch: target.branch,
        upstream: target.upstream,
        fetch: PullFetchResult {
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
        },
        merge: PullMergeResult {
            strategy: merge_result.strategy,
            old_commit: merge_result.old_commit,
            commit: merge_result.commit,
            files_changed: merge_result.files_changed,
            up_to_date: merge_result.up_to_date,
        },
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

    if result.merge.up_to_date {
        writeln!(writer, "Already up to date.")
            .map_err(|error| CliError::io(format!("failed to write pull summary: {error}")))?;
        return Ok(());
    }

    if let (Some(old), Some(new)) = (&result.merge.old_commit, &result.merge.commit) {
        writeln!(writer, "Updating {}..{}", short_oid(old), short_oid(new))
            .map_err(|error| CliError::io(format!("failed to write pull summary: {error}")))?;
    }
    writeln!(writer, "Fast-forward")
        .map_err(|error| CliError::io(format!("failed to write pull summary: {error}")))?;
    if result.merge.files_changed > 0 {
        let noun = if result.merge.files_changed == 1 {
            "file"
        } else {
            "files"
        };
        writeln!(writer, " {} {} changed", result.merge.files_changed, noun)
            .map_err(|error| CliError::io(format!("failed to write pull summary: {error}")))?;
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
            .with_stable_code(StableErrorCode::AuthMissingCredentials)
            .with_hint("check SSH key or HTTP credentials"),
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
        merge::PullMergeError::InvalidTarget(..) => CliError::command_usage(error.to_string())
            .with_stable_code(StableErrorCode::CliInvalidTarget),
        merge::PullMergeError::TargetLoad { .. }
        | merge::PullMergeError::CurrentLoad { .. }
        | merge::PullMergeError::History(..)
        | merge::PullMergeError::TreeLoad { .. } => {
            CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::RepoCorrupt)
        }
        merge::PullMergeError::UnrelatedHistories => {
            CliError::failure(error.to_string()).with_stable_code(StableErrorCode::RepoStateInvalid)
        }
        // ManualMergeRequired is extracted into PullError::ManualMergeRequired in run_pull(),
        // so this arm is unreachable via PullError::Merge. Keep it for exhaustiveness in case
        // map_merge_error_to_cli is called from other contexts.
        merge::PullMergeError::ManualMergeRequired { upstream } => CliError::failure(format!(
            "pull requires a non-fast-forward merge from '{upstream}', which is not yet supported"
        ))
        .with_stable_code(StableErrorCode::ConflictOperationBlocked),
        merge::PullMergeError::HeadUpdate(..) | merge::PullMergeError::Restore(..) => {
            CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::IoWriteFailed)
        }
    }
}
