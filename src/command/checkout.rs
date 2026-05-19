//! Handles checkout-style flows to show the current branch, switch to existing branches, or create and switch to a new one using restore utilities.

use clap::Parser;
use git_internal::hash::ObjectHash;
use serde::Serialize;

use crate::{
    command::{
        branch, pull,
        restore::{self, RestoreArgs},
        switch,
    },
    info_println,
    internal::{
        branch::{self as repo_branch, Branch, BranchStoreError},
        head::Head,
    },
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, emit_json_data},
        util,
    },
};

const CHECKOUT_EXAMPLES: &str = "\
NOTE:
    libra checkout is a branch compatibility surface. New code paths
    should prefer:
      - `libra switch <branch>` / `libra switch -c <branch>` for branch
        navigation and creation
      - `libra restore <path>` to restore files from the index or HEAD

EXAMPLES:
    libra checkout                         Show the current branch
    libra checkout main                    Switch to a branch (prefer: libra switch main)
    libra checkout feature-x               Switch to another branch (prefer: libra switch feature-x)
    libra checkout -b feature-x            Create + switch to a new branch (prefer: libra switch -c feature-x)
    libra --json checkout main             Structured compatibility output
    libra checkout --quiet main            Switch without informational stdout";

#[derive(Parser, Debug)]
#[command(after_help = CHECKOUT_EXAMPLES)]
pub struct CheckoutArgs {
    /// Target branch name
    branch: Option<String>,

    /// Create and switch to a new branch with the same content as the current branch
    #[clap(short = 'b', group = "sub")]
    new_branch: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct CheckoutOutput {
    action: String,
    previous_branch: Option<String>,
    previous_commit: Option<String>,
    branch: Option<String>,
    commit: Option<String>,
    short_commit: Option<String>,
    switched: bool,
    created: bool,
    pulled: bool,
    already_on: bool,
    detached: bool,
    tracking: Option<CheckoutTrackingOutput>,
}

#[derive(Debug, Clone, Serialize)]
struct CheckoutTrackingOutput {
    remote: String,
    remote_branch: String,
}

#[derive(Debug, thiserror::Error)]
enum CheckoutError {
    #[error("checking out '{0}' branch is not allowed")]
    CheckingOutBranchBlocked(String),

    #[error("creating/switching to '{0}' branch is not allowed")]
    CreatingBranchBlocked(String),

    #[error("switching to '{0}' branch is not allowed")]
    SwitchingToBranchBlocked(String),

    #[error("branch '{0}' not found")]
    BranchNotFound(String),

    #[error("path specification '{0}' did not match any files known to libra")]
    PathSpecNotMatched(String),

    #[error("unstaged changes, can't switch branch")]
    DirtyUnstaged,

    #[error("uncommitted changes, can't switch branch")]
    DirtyUncommitted,

    #[error("untracked working tree file would be overwritten by checkout: {0}")]
    UntrackedOverwrite(String),

    #[error("failed to {context}: {detail}")]
    BranchStoreRead { context: String, detail: String },

    #[error("failed to {context}: {detail}")]
    BranchStoreCorrupt { context: String, detail: String },

    #[error("checkout remote branch left HEAD without a commit")]
    RemoteHeadMissing,

    #[error("failed to {stage} during remote branch checkout: {}", source.message())]
    RemoteSyncFailed {
        stage: &'static str,
        #[source]
        source: Box<CliError>,
    },

    #[error(transparent)]
    DelegatedCli(#[from] CliError),
}

impl From<CheckoutError> for CliError {
    fn from(error: CheckoutError) -> Self {
        match error {
            CheckoutError::CheckingOutBranchBlocked(branch) => {
                CliError::fatal(format!("checking out '{}' branch is not allowed", branch))
                    .with_stable_code(StableErrorCode::CliInvalidTarget)
            }

            CheckoutError::CreatingBranchBlocked(branch) => CliError::fatal(format!(
                "creating/switching to '{}' branch is not allowed",
                branch
            ))
            .with_stable_code(StableErrorCode::CliInvalidTarget),

            CheckoutError::SwitchingToBranchBlocked(branch) => {
                CliError::fatal(format!("switching to '{}' branch is not allowed", branch))
                    .with_stable_code(StableErrorCode::CliInvalidTarget)
            }

            CheckoutError::BranchNotFound(branch) => {
                CliError::fatal(format!("branch '{}' not found", branch))
                    .with_stable_code(StableErrorCode::CliInvalidTarget)
            }

            CheckoutError::PathSpecNotMatched(spec) => CliError::fatal(format!(
                "path specification '{}' did not match any files known to libra",
                spec
            ))
            .with_stable_code(StableErrorCode::CliInvalidTarget),

            CheckoutError::DirtyUnstaged | CheckoutError::DirtyUncommitted => {
                CliError::failure("local changes would be overwritten by checkout")
                    .with_stable_code(StableErrorCode::RepoStateInvalid)
            }

            CheckoutError::UntrackedOverwrite(path) => CliError::failure(format!(
                "local changes would be overwritten by checkout: {path}"
            ))
            .with_stable_code(StableErrorCode::ConflictOperationBlocked),

            CheckoutError::BranchStoreRead { context, detail } => {
                CliError::fatal(format!("failed to {context}: {detail}"))
                    .with_stable_code(StableErrorCode::IoReadFailed)
            }
            CheckoutError::BranchStoreCorrupt { context, detail } => {
                CliError::fatal(format!("failed to {context}: {detail}"))
                    .with_stable_code(StableErrorCode::RepoCorrupt)
            }
            CheckoutError::RemoteHeadMissing => {
                CliError::fatal("checkout remote branch left HEAD without a commit")
                    .with_stable_code(StableErrorCode::RepoStateInvalid)
            }
            CheckoutError::RemoteSyncFailed { stage, source } => {
                let inner = *source;
                let stable_code = inner.stable_code();
                let message = format!(
                    "failed to {stage} during remote branch checkout: {}",
                    inner.message()
                );
                let wrapped = match inner.kind() {
                    crate::utils::error::CliErrorKind::Fatal => CliError::fatal(message),
                    _ => CliError::failure(message),
                };
                wrapped.with_stable_code(stable_code)
            }
            CheckoutError::DelegatedCli(err) => err,
        }
    }
}

pub async fn execute(args: CheckoutArgs) {
    if let Err(e) = execute_safe(args, &OutputConfig::default()).await {
        e.print_stderr();
    }
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting.
///
/// # Side Effects
/// - Validates target branch names and blocks the internal `intent` branch.
/// - May create a branch when `-b` is supplied.
/// - Switches HEAD/current branch and restores the working tree to the target.
/// - Emits status messages through [`OutputConfig`].
///
/// # Errors
/// Returns [`CliError`] when the target branch is invalid or missing, local
/// changes would be overwritten, branch creation fails, or checkout/restore
/// writes fail.
pub async fn execute_safe(args: CheckoutArgs, output: &OutputConfig) -> CliResult<()> {
    let result = run_checkout(args, output).await.map_err(CliError::from)?;
    render_checkout_output(&result, output)
}

async fn run_checkout(
    args: CheckoutArgs,
    output: &OutputConfig,
) -> Result<CheckoutOutput, CheckoutError> {
    if let Some(ref branch_name) = args.branch
        && repo_branch::is_libra_internal_branch(branch_name)
    {
        return Err(CheckoutError::CheckingOutBranchBlocked(branch_name.clone()));
    }
    if let Some(ref new_branch_name) = args.new_branch
        && repo_branch::is_libra_internal_branch(new_branch_name)
    {
        return Err(CheckoutError::CreatingBranchBlocked(
            new_branch_name.clone(),
        ));
    }

    let previous_branch = get_current_branch().await;
    let previous_commit = current_commit_string().await?;

    // Match Git behavior: checking out the current branch is a no-op and should
    // not be blocked by unrelated local changes.
    if let Some(ref target_branch) = args.branch
        && previous_branch.as_ref() == Some(target_branch)
    {
        return Ok(CheckoutOutput {
            action: "already-on".to_string(),
            previous_branch,
            previous_commit: previous_commit.clone(),
            branch: Some(target_branch.clone()),
            commit: previous_commit.clone(),
            short_commit: previous_commit.as_deref().map(short_oid),
            switched: false,
            created: false,
            pulled: false,
            already_on: true,
            detached: false,
            tracking: None,
        });
    }

    let target_commit = if let Some(ref branch_name) = args.branch {
        Branch::find_branch_result(branch_name, None)
            .await
            .map_err(|error| map_checkout_branch_store_error("resolve checkout target", error))?
            .map(|branch| branch.commit)
    } else {
        None
    };

    let clean_status = match target_commit {
        Some(target_commit) => switch::ensure_clean_status_for_commit(target_commit, output).await,
        None => switch::ensure_clean_status(output).await,
    };

    match clean_status {
        Ok(()) => {}
        Err(switch::SwitchError::DirtyUnstaged) => {
            return Err(CheckoutError::DirtyUnstaged);
        }
        Err(switch::SwitchError::DirtyUncommitted) => {
            return Err(CheckoutError::DirtyUncommitted);
        }
        Err(switch::SwitchError::UntrackedOverwrite(path)) => {
            return Err(CheckoutError::UntrackedOverwrite(path));
        }
        Err(err) => return Err(CheckoutError::DelegatedCli(CliError::from(err))),
    }

    match (args.branch, args.new_branch) {
        (Some(target_branch), _) => {
            check_and_switch_branch(&target_branch, previous_branch, previous_commit, output).await
        }
        (None, Some(new_branch)) => {
            let child_output = silent_child_output(output);
            let commit = create_and_switch_new_branch(&new_branch, &child_output).await?;
            let commit = commit.to_string();
            Ok(CheckoutOutput {
                action: "create".to_string(),
                previous_branch,
                previous_commit,
                branch: Some(new_branch),
                short_commit: Some(short_oid(&commit)),
                commit: Some(commit),
                switched: true,
                created: true,
                pulled: false,
                already_on: false,
                detached: false,
                tracking: None,
            })
        }
        (None, None) => show_current_branch(previous_branch, previous_commit).await,
    }
}

fn map_checkout_branch_store_error(context: &str, error: BranchStoreError) -> CheckoutError {
    match error {
        BranchStoreError::Query(detail) => CheckoutError::BranchStoreRead {
            context: context.to_string(),
            detail,
        },
        other => CheckoutError::BranchStoreCorrupt {
            context: context.to_string(),
            detail: other.to_string(),
        },
    }
}

pub async fn get_current_branch() -> Option<String> {
    match Head::current().await {
        Head::Detached(_) => None,
        Head::Branch(name) => Some(name),
    }
}

async fn current_commit_string() -> Result<Option<String>, CheckoutError> {
    Head::current_commit_result()
        .await
        .map(|commit| commit.map(|hash| hash.to_string()))
        .map_err(|error| CheckoutError::BranchStoreCorrupt {
            context: "resolve HEAD commit".to_string(),
            detail: error.to_string(),
        })
}

pub async fn switch_branch(branch_name: &str) -> CliResult<()> {
    switch_branch_with_output(branch_name, &OutputConfig::default())
        .await
        .map(|_| ())
        .map_err(CliError::from)
}

async fn switch_branch_with_output(
    branch_name: &str,
    output: &OutputConfig,
) -> Result<ObjectHash, CheckoutError> {
    if repo_branch::is_libra_internal_branch(branch_name) {
        return Err(CheckoutError::SwitchingToBranchBlocked(
            branch_name.to_string(),
        ));
    }
    let target_branch = Branch::find_branch_result(branch_name, None)
        .await
        .map_err(|error| map_checkout_branch_store_error("resolve branch", error))?
        .ok_or_else(|| CheckoutError::BranchNotFound(branch_name.to_string()))?;
    let target_commit = target_branch.commit;
    restore_to_commit(target_branch.commit, output)
        .await
        .map_err(CheckoutError::DelegatedCli)?;
    let head = Head::Branch(branch_name.to_string());
    Head::update(head, None).await;
    Ok(target_commit)
}

async fn create_and_switch_new_branch(
    new_branch: &str,
    output: &OutputConfig,
) -> Result<ObjectHash, CheckoutError> {
    branch::create_branch_safe(new_branch.to_string(), get_current_branch().await)
        .await
        .map_err(CheckoutError::DelegatedCli)?;
    switch_branch_with_output(new_branch, output).await
}

async fn get_remote(branch_name: &str, output: &OutputConfig) -> Result<ObjectHash, CheckoutError> {
    let remote_branch_name: String = format!("origin/{branch_name}");
    let child_output = silent_child_output(output);

    create_and_switch_new_branch(branch_name, &child_output)
        .await
        .map_err(|err| wrap_remote_proxy_error("create local tracking branch", err))?;
    // Set branch upstream
    branch::set_upstream_safe_with_output(branch_name, &remote_branch_name, &child_output)
        .await
        .map_err(|err| CheckoutError::RemoteSyncFailed {
            stage: "set upstream",
            source: Box::new(err),
        })?;
    // Synchronous branches
    // Use the pull command to update the local branch with the latest changes from the remote branch
    pull::execute_safe(pull::PullArgs::make(None, None), &child_output)
        .await
        .map_err(|err| CheckoutError::RemoteSyncFailed {
            stage: "pull from remote",
            source: Box::new(err),
        })?;
    Head::current_commit_result()
        .await
        .map_err(|error| map_checkout_branch_store_error("resolve checkout result", error))?
        .ok_or(CheckoutError::RemoteHeadMissing)
}

/// Converts a [`CheckoutError`] surfaced by the local-creation step of remote tracking
/// into a [`CheckoutError::RemoteSyncFailed`] envelope so downstream callers see a single
/// proxy-error variant regardless of which sub-step failed.
fn wrap_remote_proxy_error(stage: &'static str, err: CheckoutError) -> CheckoutError {
    match err {
        already @ CheckoutError::RemoteSyncFailed { .. } => already,
        other => CheckoutError::RemoteSyncFailed {
            stage,
            source: Box::new(CliError::from(other)),
        },
    }
}

/// Returns `Ok(Some(true))` if remote branch found, `Ok(Some(false))` if local branch found,
/// `Ok(None)` if already on the branch.
pub async fn check_branch(branch_name: &str) -> CliResult<Option<bool>> {
    check_branch_with_output(branch_name, &OutputConfig::default())
        .await
        .map_err(CliError::from)
}

async fn check_branch_with_output(
    branch_name: &str,
    output: &OutputConfig,
) -> Result<Option<bool>, CheckoutError> {
    if get_current_branch().await == Some(branch_name.to_string()) {
        info_println!(output, "Already on {branch_name}");
        return Ok(None);
    }

    let target_branch: Option<Branch> = Branch::find_branch_result(branch_name, None)
        .await
        .map_err(|error| map_checkout_branch_store_error("resolve branch", error))?;
    if target_branch.is_none() {
        let remote_branch_name: String = format!("origin/{branch_name}");
        if !Branch::search_branch_result(&remote_branch_name)
            .await
            .map_err(|error| {
                map_checkout_branch_store_error("search remote tracking branches", error)
            })?
            .is_empty()
        {
            info_println!(
                output,
                "branch '{branch_name}' set up to track '{remote_branch_name}'."
            );
            Ok(Some(true))
        } else {
            Err(CheckoutError::PathSpecNotMatched(branch_name.to_string()))
        }
    } else {
        info_println!(output, "Switched to branch '{branch_name}'");
        Ok(Some(false))
    }
}

async fn check_and_switch_branch(
    branch_name: &str,
    previous_branch: Option<String>,
    previous_commit: Option<String>,
    output: &OutputConfig,
) -> Result<CheckoutOutput, CheckoutError> {
    let child_output = silent_child_output(output);
    match check_branch_with_output(branch_name, &child_output).await? {
        Some(true) => {
            let commit = get_remote(branch_name, output).await?.to_string();
            Ok(CheckoutOutput {
                action: "track".to_string(),
                previous_branch,
                previous_commit,
                branch: Some(branch_name.to_string()),
                commit: Some(commit.clone()),
                short_commit: Some(short_oid(&commit)),
                switched: true,
                created: true,
                pulled: true,
                already_on: false,
                detached: false,
                tracking: Some(CheckoutTrackingOutput {
                    remote: "origin".to_string(),
                    remote_branch: format!("origin/{branch_name}"),
                }),
            })
        }
        Some(false) => {
            let commit = switch_branch_with_output(branch_name, &child_output)
                .await?
                .to_string();
            Ok(CheckoutOutput {
                action: "switch".to_string(),
                previous_branch,
                previous_commit,
                branch: Some(branch_name.to_string()),
                commit: Some(commit.clone()),
                short_commit: Some(short_oid(&commit)),
                switched: true,
                created: false,
                pulled: false,
                already_on: false,
                detached: false,
                tracking: None,
            })
        }
        None => Ok(CheckoutOutput {
            action: "already-on".to_string(),
            previous_branch: previous_branch.clone(),
            previous_commit: previous_commit.clone(),
            branch: Some(branch_name.to_string()),
            commit: previous_commit.clone(),
            short_commit: previous_commit.as_deref().map(short_oid),
            switched: false,
            created: false,
            pulled: false,
            already_on: true,
            detached: false,
            tracking: None,
        }),
    }
}

async fn restore_to_commit(commit_id: ObjectHash, output: &OutputConfig) -> CliResult<()> {
    let restore_args = RestoreArgs {
        worktree: true,
        staged: true,
        source: Some(commit_id.to_string()),
        pathspec: vec![util::working_dir_string()],
    };
    restore::execute_safe(restore_args, &output.child_output_config()).await
}

async fn show_current_branch(
    current_branch: Option<String>,
    current_commit: Option<String>,
) -> Result<CheckoutOutput, CheckoutError> {
    match Head::current().await {
        Head::Detached(commit_hash) => {
            let commit = commit_hash.to_string();
            Ok(CheckoutOutput {
                action: "show-current".to_string(),
                previous_branch: current_branch,
                previous_commit: current_commit,
                branch: None,
                commit: Some(commit.clone()),
                short_commit: Some(short_oid(&commit)),
                switched: false,
                created: false,
                pulled: false,
                already_on: false,
                detached: true,
                tracking: None,
            })
        }
        Head::Branch(current_branch) => Ok(CheckoutOutput {
            action: "show-current".to_string(),
            previous_branch: Some(current_branch.clone()),
            previous_commit: current_commit.clone(),
            branch: Some(current_branch),
            commit: current_commit.clone(),
            short_commit: current_commit.as_deref().map(short_oid),
            switched: false,
            created: false,
            pulled: false,
            already_on: false,
            detached: false,
            tracking: None,
        }),
    }
}

fn render_checkout_output(result: &CheckoutOutput, output: &OutputConfig) -> CliResult<()> {
    if output.is_json() {
        return emit_json_data("checkout", result, output);
    }
    if output.quiet {
        return Ok(());
    }

    match result.action.as_str() {
        "show-current" if result.detached => {
            if let Some(short_commit) = &result.short_commit {
                println!("HEAD detached at {short_commit}");
            }
        }
        "show-current" => {
            if let Some(branch) = &result.branch {
                println!("Current branch is {branch}.");
            }
        }
        "already-on" => {
            if let Some(branch) = &result.branch {
                println!("Already on {branch}");
            }
        }
        "create" => {
            if let Some(branch) = &result.branch {
                println!("Switched to a new branch '{branch}'");
            }
        }
        "switch" => {
            if let Some(branch) = &result.branch {
                println!("Switched to branch '{branch}'");
            }
        }
        "track" => {
            if let (Some(branch), Some(tracking)) = (&result.branch, &result.tracking) {
                println!(
                    "branch '{branch}' set up to track '{}'.",
                    tracking.remote_branch
                );
                println!("Switched to a new branch '{branch}'");
            }
        }
        _ => {}
    }

    Ok(())
}

fn short_oid(oid: &str) -> String {
    oid.chars().take(8).collect()
}

fn silent_child_output(output: &OutputConfig) -> OutputConfig {
    let mut child = output.child_output_config();
    child.quiet = true;
    child
}

/// Unit tests for the checkout module
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkout_error_display_pins_owned_variants() {
        assert_eq!(
            CheckoutError::CheckingOutBranchBlocked("HEAD".to_string()).to_string(),
            "checking out 'HEAD' branch is not allowed",
        );
        assert_eq!(
            CheckoutError::CreatingBranchBlocked("HEAD".to_string()).to_string(),
            "creating/switching to 'HEAD' branch is not allowed",
        );
        assert_eq!(
            CheckoutError::SwitchingToBranchBlocked("intent".to_string()).to_string(),
            "switching to 'intent' branch is not allowed",
        );
        assert_eq!(
            CheckoutError::BranchNotFound("feature".to_string()).to_string(),
            "branch 'feature' not found",
        );
        assert_eq!(
            CheckoutError::PathSpecNotMatched("nonexistent".to_string()).to_string(),
            "path specification 'nonexistent' did not match any files known to libra",
        );
        assert_eq!(
            CheckoutError::DirtyUnstaged.to_string(),
            "unstaged changes, can't switch branch",
        );
        assert_eq!(
            CheckoutError::DirtyUncommitted.to_string(),
            "uncommitted changes, can't switch branch",
        );
        assert_eq!(
            CheckoutError::UntrackedOverwrite("src/new.rs".to_string()).to_string(),
            "untracked working tree file would be overwritten by checkout: src/new.rs",
        );
        assert_eq!(
            CheckoutError::BranchStoreRead {
                context: "load branch 'main'".to_string(),
                detail: "database is locked".to_string(),
            }
            .to_string(),
            "failed to load branch 'main': database is locked",
        );
        assert_eq!(
            CheckoutError::BranchStoreCorrupt {
                context: "resolve branch 'feature'".to_string(),
                detail: "ref points to non-commit object".to_string(),
            }
            .to_string(),
            "failed to resolve branch 'feature': ref points to non-commit object",
        );
        assert_eq!(
            CheckoutError::RemoteHeadMissing.to_string(),
            "checkout remote branch left HEAD without a commit",
        );
        let proxy_err = CliError::failure("remote not configured")
            .with_stable_code(StableErrorCode::NetworkUnavailable);
        assert_eq!(
            CheckoutError::RemoteSyncFailed {
                stage: "pull from remote",
                source: Box::new(proxy_err),
            }
            .to_string(),
            "failed to pull from remote during remote branch checkout: remote not configured",
        );
    }

    #[test]
    fn checkout_error_maps_owned_variants_to_stable_codes() {
        let cases: Vec<(CheckoutError, StableErrorCode)> = vec![
            (
                CheckoutError::CheckingOutBranchBlocked("intent".to_string()),
                StableErrorCode::CliInvalidTarget,
            ),
            (
                CheckoutError::CreatingBranchBlocked("intent".to_string()),
                StableErrorCode::CliInvalidTarget,
            ),
            (
                CheckoutError::SwitchingToBranchBlocked("intent".to_string()),
                StableErrorCode::CliInvalidTarget,
            ),
            (
                CheckoutError::BranchNotFound("feature".to_string()),
                StableErrorCode::CliInvalidTarget,
            ),
            (
                CheckoutError::PathSpecNotMatched("nope".to_string()),
                StableErrorCode::CliInvalidTarget,
            ),
            (
                CheckoutError::DirtyUnstaged,
                StableErrorCode::RepoStateInvalid,
            ),
            (
                CheckoutError::DirtyUncommitted,
                StableErrorCode::RepoStateInvalid,
            ),
            (
                CheckoutError::UntrackedOverwrite("a.txt".to_string()),
                StableErrorCode::ConflictOperationBlocked,
            ),
            (
                CheckoutError::RemoteHeadMissing,
                StableErrorCode::RepoStateInvalid,
            ),
        ];

        for (err, expected) in cases {
            let cli: CliError = err.into();
            assert_eq!(cli.stable_code(), expected);
        }
    }

    #[test]
    fn checkout_remote_sync_failed_preserves_inner_stable_code() {
        let inner = CliError::fatal("upstream missing")
            .with_stable_code(StableErrorCode::NetworkUnavailable);
        let wrapped = CheckoutError::RemoteSyncFailed {
            stage: "set upstream",
            source: Box::new(inner),
        };
        let cli: CliError = wrapped.into();
        assert_eq!(cli.stable_code(), StableErrorCode::NetworkUnavailable);
        assert!(
            cli.message()
                .contains("failed to set upstream during remote branch checkout"),
            "got: {}",
            cli.message()
        );
        assert!(
            cli.message().contains("upstream missing"),
            "got: {}",
            cli.message()
        );
    }
}
