//! Pull command combining fetch with merge or rebase depending on options, handling fast-forward checks and remote tracking setup.

use std::io::Write;

use clap::{Parser, ValueEnum};
use git_internal::errors::GitError;
use serde::Serialize;

use super::{fetch, merge, rebase};
use crate::{
    internal::{
        config::{ConfigKv, LocalIdentityTarget, RemoteConfig, read_cascaded_config_value},
        head::Head,
        protocol::ShallowOptions,
    },
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, ProgressMode, emit_json_data},
    },
};

/// Value of `--rebase[=<when>]` / `pull.rebase`.
///
/// `merges` and `interactive` are accepted as *values* (so the CLI surface
/// matches Git) but rejected at runtime — the rebase engine only does linear
/// rebase. `False` forces the merge path, overriding a `pull.rebase=true`
/// config.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum RebaseChoice {
    #[value(name = "true")]
    True,
    #[value(name = "false")]
    False,
    #[value(name = "merges")]
    Merges,
    #[value(name = "interactive")]
    Interactive,
}

/// The integration path chosen after resolving CLI flags and `pull.rebase`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RebaseDecision {
    Rebase,
    Merge,
}

const PULL_EXAMPLES: &str = "\
EXAMPLES:
    libra pull                             Pull from tracking remote
    libra pull origin main                 Pull specific branch from origin
    libra pull --ff-only                   Refuse to create a merge commit
    libra pull --no-ff                     Always create a merge commit
    libra pull --rebase                    Rebase the current branch onto the upstream
    libra pull --squash                    Stage a squashed merge without committing
    libra pull --no-commit                 Merge but stop before creating the commit
    libra pull --autostash                 Stash a dirty tree, merge, then restore it
    libra pull --depth 1                   Shallow-fetch then integrate
    libra pull --json                      Structured JSON output for agents
    libra pull --quiet                     Suppress progress output

NOTES:
    By default pull uses the same merge engine as `libra merge`, including
    clean three-way merges and merge-state conflicts. Use --ff-only to reject
    divergent histories instead of creating a merge commit. Use --rebase to
    replay local-only commits onto the upstream tip instead. --squash /
    --no-commit / --ff / --no-ff / --autostash are forwarded to the merge
    engine and may not be combined with --rebase. --rebase=merges and
    --rebase=interactive are not supported (only linear rebase).";

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

    /// Rebase the current branch onto the upstream after fetching instead of merging.
    /// Accepts `--rebase=true|false|merges|interactive` (merges/interactive are not yet supported)
    #[clap(
        long,
        short = 'r',
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = "true",
        value_name = "WHEN"
    )]
    rebase: Option<RebaseChoice>,

    /// Refuse to merge unless the upstream can be fast-forwarded
    #[clap(long = "ff-only", conflicts_with_all = ["ff", "no_ff"])]
    ff_only: bool,

    /// Allow a fast-forward merge (clears --no-ff; overrides `pull.ff=false`)
    #[clap(long, conflicts_with_all = ["no_ff", "ff_only"])]
    ff: bool,

    /// Always create a merge commit even when fast-forward is possible
    #[clap(long = "no-ff", conflicts_with_all = ["ff", "ff_only"])]
    no_ff: bool,

    /// Stage a squashed merge result without recording a merge commit (merge path only)
    #[clap(long)]
    squash: bool,

    /// Override a configured squash default back off (no-op when not squashing)
    #[clap(long = "no-squash", conflicts_with = "squash")]
    no_squash: bool,

    /// Merge but stop before creating the merge commit (merge path only)
    #[clap(long = "no-commit", conflicts_with = "commit")]
    no_commit: bool,

    /// Create the merge commit (overrides `merge.commit=false`; merge path only)
    #[clap(long, conflicts_with = "no_commit")]
    commit: bool,

    /// Stash a dirty working tree before integrating, then restore it (merge path only)
    #[clap(long, conflicts_with = "no_autostash")]
    autostash: bool,

    /// Do not autostash even when `merge.autoStash=true` is configured
    #[clap(long = "no-autostash", conflicts_with = "autostash")]
    no_autostash: bool,

    /// Limit the fetch to the given number of commits from each tip (shallow fetch)
    #[clap(long)]
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

    #[error("--rebase={value} is not supported (only linear rebase is available)")]
    UnsupportedRebaseStrategy { value: String },

    #[error("{a} cannot be used with {b}")]
    IncompatibleFlags { a: String, b: String },

    #[error("invalid value for {key}: '{value}'")]
    InvalidConfigValue { key: String, value: String },

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
            PullError::UnsupportedRebaseStrategy { value } => CliError::command_usage(format!(
                "--rebase={value} is not supported (only linear rebase is available)"
            ))
            .with_stable_code(StableErrorCode::CliInvalidTarget)
            .with_hint("omit the strategy or use a plain '--rebase' for a linear rebase"),
            PullError::IncompatibleFlags { a, b } => {
                CliError::command_usage(format!("{a} cannot be used with {b}"))
                    .with_stable_code(StableErrorCode::CliInvalidArguments)
                    .with_hint("remove one of the conflicting options and retry")
            }
            PullError::InvalidConfigValue { key, value } => {
                CliError::command_usage(format!("invalid value for {key}: '{value}'"))
                    .with_stable_code(StableErrorCode::CliInvalidArguments)
                    .with_hint(format!(
                        "set a valid value with 'libra config {key} <value>'"
                    ))
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
            rebase: None,
            ff_only: false,
            ff: false,
            no_ff: false,
            squash: false,
            no_squash: false,
            no_commit: false,
            commit: false,
            autostash: false,
            no_autostash: false,
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
    // Early flag-compatibility gate (no repo / config / network needed): runs
    // before repo preflight so an invalid flag combination is reported even
    // outside a repository and never touches HEAD.
    if let Some(choice) = args.rebase {
        // An explicit unsupported rebase strategy is rejected up front.
        match choice {
            RebaseChoice::Merges => {
                return Err(PullError::UnsupportedRebaseStrategy {
                    value: "merges".to_string(),
                });
            }
            RebaseChoice::Interactive => {
                return Err(PullError::UnsupportedRebaseStrategy {
                    value: "interactive".to_string(),
                });
            }
            RebaseChoice::True | RebaseChoice::False => {}
        }
        // An explicit `--rebase` (anything but `--rebase=false`) cannot be
        // combined with merge-only flags.
        if choice != RebaseChoice::False
            && let Some(flag) = first_conflicting_merge_flag(&args)
        {
            return Err(PullError::IncompatibleFlags {
                a: "--rebase".to_string(),
                b: flag.to_string(),
            });
        }
    }

    // Resolve the integration path (CLI `--rebase` wins over `pull.rebase`).
    // Done before target resolution so an invalid strategy / flag combination
    // is reported without needing tracking config and without any fetch.
    let rebase_decision = resolve_pull_rebase(&args).await?;

    // A config-driven rebase (`pull.rebase=true`, no CLI `--rebase`) combined
    // with a merge-only flag is also rejected.
    if rebase_decision == RebaseDecision::Rebase
        && args.rebase.is_none()
        && let Some(flag) = first_conflicting_merge_flag(&args)
    {
        return Err(PullError::IncompatibleFlags {
            a: "pull.rebase".to_string(),
            b: flag.to_string(),
        });
    }

    // For the merge path, resolve every option and run the squash-combination
    // guard *before* target resolution / fetch, so an invalid combination never
    // reaches the network or the working tree.
    let merge_plan = match rebase_decision {
        RebaseDecision::Rebase => None,
        RebaseDecision::Merge => Some(resolve_merge_plan(&args).await?),
    };

    let target = resolve_pull_target(&args).await?;
    let child_output = child_output_for_pull(output);

    let fetch_result = fetch::fetch_repository_with_result(
        target.remote_config.clone(),
        Some(target.remote_branch.clone()),
        false,
        ShallowOptions {
            depth: args.depth,
            ..Default::default()
        },
        false,
        false,
        false,
        false,
        false,
        false,
        false,
        false,
        Vec::new(),
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

    let Some(merge_plan) = merge_plan else {
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
    };

    let merge_result = merge::run_merge_for_pull_with_autostash(
        &target.merge_target,
        &target.upstream,
        &child_output,
        merge::PullMergeOptions {
            ff_only: merge_plan.ff_only,
            no_ff: merge_plan.no_ff,
            ff_resolved: merge_plan.ff_resolved,
            squash: args.squash,
            no_commit: merge_plan.no_commit,
            autostash: merge_plan.autostash,
            ..Default::default()
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

/// Resolved merge-path options (everything the merge engine needs), computed
/// from CLI flags plus `pull.ff` / `merge.commit` / `merge.autoStash` config.
struct MergePlan {
    ff_only: bool,
    no_ff: bool,
    ff_resolved: bool,
    no_commit: bool,
    autostash: bool,
}

/// The merge-only flags that must not appear on the rebase path. `--no-squash`
/// and `--no-autostash` are benign "off" defaults and are intentionally
/// excluded.
fn first_conflicting_merge_flag(args: &PullArgs) -> Option<&'static str> {
    if args.squash {
        Some("--squash")
    } else if args.commit {
        Some("--commit")
    } else if args.no_commit {
        Some("--no-commit")
    } else if args.ff {
        Some("--ff")
    } else if args.no_ff {
        Some("--no-ff")
    } else if args.ff_only {
        Some("--ff-only")
    } else if args.autostash {
        Some("--autostash")
    } else {
        None
    }
}

/// Resolve the integration path. CLI `--rebase[=…]` wins over `pull.rebase`;
/// `merges`/`interactive` (from either source) are rejected at runtime.
async fn resolve_pull_rebase(args: &PullArgs) -> Result<RebaseDecision, PullError> {
    if let Some(choice) = args.rebase {
        return match choice {
            RebaseChoice::True => Ok(RebaseDecision::Rebase),
            RebaseChoice::False => Ok(RebaseDecision::Merge),
            RebaseChoice::Merges => Err(PullError::UnsupportedRebaseStrategy {
                value: "merges".to_string(),
            }),
            RebaseChoice::Interactive => Err(PullError::UnsupportedRebaseStrategy {
                value: "interactive".to_string(),
            }),
        };
    }

    let Some(value) = read_cascaded_config_value(LocalIdentityTarget::CurrentRepo, "pull.rebase")
        .await
        .ok()
        .flatten()
    else {
        return Ok(RebaseDecision::Merge);
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Ok(RebaseDecision::Rebase),
        "false" | "no" | "off" | "0" => Ok(RebaseDecision::Merge),
        "merges" => Err(PullError::UnsupportedRebaseStrategy {
            value: "merges".to_string(),
        }),
        "interactive" => Err(PullError::UnsupportedRebaseStrategy {
            value: "interactive".to_string(),
        }),
        _ => Err(PullError::InvalidConfigValue {
            key: "pull.rebase".to_string(),
            value,
        }),
    }
}

/// Resolve the full merge plan and run the squash-combination guard on the
/// *resolved* values (so `pull.ff=false` / `merge.autoStash=true` are caught,
/// not just explicit flags).
async fn resolve_merge_plan(args: &PullArgs) -> Result<MergePlan, PullError> {
    let (ff_only, no_ff, ff_resolved) = resolve_pull_ff(args).await?;
    let no_commit = resolve_pull_no_commit(args).await?;
    let autostash = resolve_pull_autostash(args).await;

    if args.squash {
        if no_ff {
            return Err(PullError::IncompatibleFlags {
                a: "--squash".to_string(),
                b: "no-fast-forward (--no-ff or pull.ff=false)".to_string(),
            });
        }
        if args.commit {
            return Err(PullError::IncompatibleFlags {
                a: "--squash".to_string(),
                b: "--commit".to_string(),
            });
        }
        if autostash {
            // A squash conflict saves no MergeState, so an autostash would be
            // popped onto a conflicted tree with no `--continue` recovery point.
            return Err(PullError::IncompatibleFlags {
                a: "--squash".to_string(),
                b: "autostash (--autostash or merge.autoStash=true)".to_string(),
            });
        }
    }

    Ok(MergePlan {
        ff_only,
        no_ff,
        ff_resolved,
        no_commit,
        autostash,
    })
}

/// Resolve fast-forward intent: `--ff-only` > `--no-ff` > `--ff` > `pull.ff`.
/// Returns `(ff_only, no_ff, ff_resolved)`; `ff_resolved` is true once the CLI
/// or `pull.ff` has decided, so the merge engine does not re-read `merge.ff`.
async fn resolve_pull_ff(args: &PullArgs) -> Result<(bool, bool, bool), PullError> {
    if args.ff_only {
        return Ok((true, false, true));
    }
    if args.no_ff {
        return Ok((false, true, true));
    }
    if args.ff {
        return Ok((false, false, true));
    }
    let Some(value) = read_cascaded_config_value(LocalIdentityTarget::CurrentRepo, "pull.ff")
        .await
        .ok()
        .flatten()
    else {
        // Neither CLI nor pull.ff set: let the merge engine fall back to merge.ff.
        return Ok((false, false, false));
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "only" => Ok((true, false, true)),
        "false" | "no" | "off" | "0" => Ok((false, true, true)),
        "true" | "yes" | "on" | "1" => Ok((false, false, true)),
        _ => Err(PullError::InvalidConfigValue {
            key: "pull.ff".to_string(),
            value,
        }),
    }
}

/// Resolve `--commit`/`--no-commit`, falling back to `merge.commit` (mirrors the
/// merge command's `resolve_no_commit`).
async fn resolve_pull_no_commit(args: &PullArgs) -> Result<bool, PullError> {
    if args.commit {
        return Ok(false);
    }
    if args.no_commit {
        return Ok(true);
    }
    let Some(value) = read_cascaded_config_value(LocalIdentityTarget::CurrentRepo, "merge.commit")
        .await
        .ok()
        .flatten()
    else {
        return Ok(false);
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Ok(false),
        "false" | "no" | "off" | "0" => Ok(true),
        _ => Err(PullError::InvalidConfigValue {
            key: "merge.commit".to_string(),
            value,
        }),
    }
}

/// Resolve `--autostash`/`--no-autostash`, falling back to `merge.autoStash`
/// (default off). The merge engine never reads this key on the pull path, so
/// pull must resolve the boolean itself (mirrors the merge command's
/// `resolve_autostash`).
async fn resolve_pull_autostash(args: &PullArgs) -> bool {
    if args.no_autostash {
        return false;
    }
    if args.autostash {
        return true;
    }
    read_cascaded_config_value(LocalIdentityTarget::CurrentRepo, "merge.autoStash")
        .await
        .ok()
        .flatten()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "true" | "yes" | "on" | "1"
            )
        })
        .unwrap_or(false)
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
        merge::PullMergeError::MissingAction
        | merge::PullMergeError::ConflictingAction
        | merge::PullMergeError::SquashNoFf
        | merge::PullMergeError::SquashCommit
        | merge::PullMergeError::InvalidMergeFfConfig { .. }
        | merge::PullMergeError::InvalidMergeCommitConfig { .. }
        | merge::PullMergeError::InvalidRenameSimilarity { .. }
        | merge::PullMergeError::InvalidRenameLimitConfig { .. }
        | merge::PullMergeError::InvalidDiffAlgorithm { .. }
        | merge::PullMergeError::InvalidCleanupMode { .. } => {
            CliError::command_usage(error.to_string())
                .with_stable_code(StableErrorCode::CliInvalidArguments)
        }
        merge::PullMergeError::MessageFileRead { .. } => {
            CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::IoReadFailed)
        }
        merge::PullMergeError::SignoffIdentity => {
            CliError::failure(error.to_string()).with_stable_code(StableErrorCode::RepoStateInvalid)
        }
        merge::PullMergeError::InvalidTarget(..) => CliError::command_usage(error.to_string())
            .with_stable_code(StableErrorCode::CliInvalidTarget),
        merge::PullMergeError::TargetLoad { .. }
        | merge::PullMergeError::CurrentLoad { .. }
        | merge::PullMergeError::History(..)
        | merge::PullMergeError::VirtualMergeBase(..)
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
        // A squash merge records no MergeState, so it cannot be resumed with
        // `libra merge --continue` — the user stages the resolution and runs
        // `libra commit` instead (matches the `libra merge --squash` contract).
        merge::PullMergeError::SquashConflicts => CliError::failure(error.to_string())
            .with_stable_code(StableErrorCode::ConflictOperationBlocked)
            .with_hint("resolve the conflicts, stage the result, then run 'libra commit'")
            .with_hint("squash merges do not support 'libra merge --continue'"),
        merge::PullMergeError::Conflicts { .. }
        | merge::PullMergeError::OctopusConflict { .. }
        | merge::PullMergeError::DirectoryFileConflict { .. }
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
        | merge::PullMergeError::Autostash(..)
        | merge::PullMergeError::Sign(..)
        | merge::PullMergeError::IndexSave(..)
        | merge::PullMergeError::TreeCreate(..)
        | merge::PullMergeError::CommitSave(..)
        | merge::PullMergeError::WorkdirReset(..) => {
            CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::IoWriteFailed)
        }
        merge::PullMergeError::UnsignedTarget { .. } => {
            CliError::failure(error.to_string()).with_stable_code(StableErrorCode::RepoStateInvalid)
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
    use clap::Parser as _;

    use super::*;

    #[test]
    fn rebase_positional_args_not_swallowed() {
        // `require_equals=true` keeps `origin`/`main` as positionals rather than
        // letting `--rebase` consume `origin` as its optional value.
        let args = PullArgs::try_parse_from(["pull", "--rebase", "origin", "main"])
            .expect("--rebase origin main should parse");
        assert_eq!(args.rebase, Some(RebaseChoice::True));
        assert_eq!(args.repository.as_deref(), Some("origin"));
        assert_eq!(args.refspec.as_deref(), Some("main"));

        let short = PullArgs::try_parse_from(["pull", "-r", "origin", "main"])
            .expect("-r origin main should parse");
        assert_eq!(short.rebase, Some(RebaseChoice::True));
        assert_eq!(short.repository.as_deref(), Some("origin"));
        assert_eq!(short.refspec.as_deref(), Some("main"));
    }

    #[test]
    fn rebase_value_variants_parse() {
        assert_eq!(
            PullArgs::try_parse_from(["pull", "--rebase=false"])
                .unwrap()
                .rebase,
            Some(RebaseChoice::False)
        );
        // `merges`/`interactive` parse as values (rejected later at runtime).
        assert_eq!(
            PullArgs::try_parse_from(["pull", "--rebase=merges"])
                .unwrap()
                .rebase,
            Some(RebaseChoice::Merges)
        );
        assert_eq!(
            PullArgs::try_parse_from(["pull", "--rebase=interactive"])
                .unwrap()
                .rebase,
            Some(RebaseChoice::Interactive)
        );
        // Absent flag is None (merge path).
        assert_eq!(PullArgs::try_parse_from(["pull"]).unwrap().rebase, None);
    }

    #[test]
    fn depth_and_merge_flags_parse() {
        let args = PullArgs::try_parse_from(["pull", "--depth", "1", "--squash"])
            .expect("--depth 1 --squash should parse");
        assert_eq!(args.depth, Some(1));
        assert!(args.squash);

        let conflict = PullArgs::try_parse_from(["pull", "--ff", "--no-ff"]);
        assert!(conflict.is_err(), "--ff and --no-ff are clap-conflicting");
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
