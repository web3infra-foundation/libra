//! Branch management utilities for creating, deleting, listing, and switching branches while handling upstream metadata.

use std::collections::{HashSet, VecDeque};

use clap::{ArgGroup, Parser};
use colored::Colorize;
use git_internal::{hash::ObjectHash, internal::object::commit::Commit};
use sea_orm::ConnectionTrait;
use serde::Serialize;

use crate::{
    command::{get_target_commit, load_object},
    internal::{
        branch::{self, Branch},
        config::ConfigKv,
        db::get_db_conn_instance,
        head::Head,
    },
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, emit_json_data},
        text::{levenshtein, short_display_hash},
    },
};

pub enum BranchListMode {
    Local,
    Remote,
    All,
}

const BRANCH_AFTER_HELP: &str = "\
Compatibility Notes:
  Libra's global --quiet suppresses the branch listing itself.
  This differs from `git branch --quiet`, which still prints the primary list.

EXAMPLES:
  libra branch feature-x                  Create a branch from HEAD
  libra branch feature-x main             Create a branch from another branch
  libra branch -d topic                   Delete a fully merged branch
  libra branch -D topic                   Force-delete a branch
  libra branch --set-upstream-to origin/main
                                          Set upstream for the current branch
  libra branch --json --show-current      Structured JSON output for agents";

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "action")]
pub enum BranchOutput {
    #[serde(rename = "list")]
    List {
        branches: Vec<BranchListEntry>,
        #[serde(skip_serializing)]
        head_name: Option<String>,
        #[serde(skip_serializing)]
        detached_head: Option<String>,
        #[serde(skip_serializing)]
        show_unborn_head: bool,
    },
    #[serde(rename = "create")]
    Create { name: String, commit: String },
    #[serde(rename = "delete")]
    Delete {
        name: String,
        commit: String,
        force: bool,
    },
    #[serde(rename = "rename")]
    Rename { old_name: String, new_name: String },
    #[serde(rename = "set-upstream")]
    SetUpstream { branch: String, upstream: String },
    #[serde(rename = "show-current")]
    ShowCurrent {
        name: Option<String>,
        detached: bool,
        commit: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct BranchListEntry {
    pub name: String,
    pub current: bool,
    pub commit: String,
    #[serde(skip_serializing)]
    pub display_name: String,
}

// action options are mutually exclusive with query options
// query options can be combined
#[derive(Parser, Debug)]
#[command(after_help = BRANCH_AFTER_HELP)]
#[command(group(
    ArgGroup::new("action")
        .multiple(false)
        .conflicts_with("query")
))]
#[command(group(
    ArgGroup::new("query")
        .required(false)
        .multiple(true)
        .conflicts_with("action")
))]
pub struct BranchArgs {
    /// new branch name
    #[clap(group = "action")]
    pub new_branch: Option<String>,

    /// base branch name or commit hash
    #[clap(requires = "new_branch")]
    pub commit_hash: Option<String>,

    /// list all branches, don't include remote branches
    #[clap(short, long, group = "query")]
    pub list: bool,

    /// force delete branch
    #[clap(short = 'D', long = "delete-force", group = "action")]
    pub delete: Option<String>,

    /// safe delete branch (checks if merged before deletion)
    #[clap(short = 'd', long = "delete", group = "action")]
    pub delete_safe: Option<String>,

    ///  Set up `branchname`>`'s tracking information so `<`upstream`>` is considered `<`branchname`>`'s upstream branch.
    #[clap(short = 'u', long, group = "action")]
    pub set_upstream_to: Option<String>,

    /// show current branch
    #[clap(long, group = "action")]
    pub show_current: bool,

    /// Rename a branch. With one argument, renames the current branch. With two arguments, renames OLD_BRANCH to NEW_BRANCH.
    #[clap(short = 'm', long = "move", group = "action", value_names = ["OLD_BRANCH", "NEW_BRANCH"], num_args = 1..=2)]
    pub rename: Vec<String>,

    /// show remote branches
    #[clap(short, long, group = "query")]
    // TODO limit to required `list` option, even in default
    pub remotes: bool,

    /// show all branches (includes local and remote)
    #[clap(short, long, group = "query")]
    pub all: bool,

    /// Only list branches which contain the specified commit (HEAD if not specified). Implies --list.
    #[clap(long, group = "query", alias = "with", value_name = "commit", num_args = 0..=1, default_missing_value = "HEAD", action = clap::ArgAction::Append)]
    pub contains: Vec<String>,

    /// Only list branches which don’t contain the specified commit (HEAD if not specified). Implies --list.
    #[clap(long, group = "query", alias = "without", value_name = "commit", num_args = 0..=1, default_missing_value = "HEAD", action = clap::ArgAction::Append)]
    pub no_contains: Vec<String>,
}
pub async fn execute(args: BranchArgs) {
    if let Err(err) = execute_safe(args, &OutputConfig::default()).await {
        err.print_stderr();
    }
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting. Creates, deletes, renames, or lists branches depending
/// on the provided arguments.
pub async fn execute_safe(args: BranchArgs, output: &OutputConfig) -> CliResult<()> {
    let result = run_branch(&args).await.map_err(CliError::from)?;
    render_branch_output(&result, output)
}

#[derive(Debug, thiserror::Error)]
enum BranchError {
    #[error("not a libra repository")]
    NotInRepo,

    #[error("'{0}' is not a valid branch name")]
    InvalidName(String),

    #[error("a branch named '{0}' already exists")]
    AlreadyExists(String),

    #[error("branch '{name}' not found")]
    NotFound { name: String, similar: Vec<String> },

    #[error("Cannot delete the branch '{0}' which you are currently on")]
    DeleteCurrent(String),

    #[error("The branch '{0}' is not fully merged.")]
    NotFullyMerged(String),

    #[error("the '{0}' branch is locked and cannot be modified")]
    Locked(String),

    #[error("HEAD is detached")]
    DetachedHead,

    #[error("not a valid object name: '{0}'")]
    InvalidCommit(String),

    #[error("invalid upstream '{0}'")]
    InvalidUpstream(String),

    #[error("{0}")]
    ConfigReadFailed(String),

    #[error("failed to persist branch config '{key}': {detail}")]
    ConfigWriteFailed { key: String, detail: String },

    #[error("failed to query branch storage: {0}")]
    StorageQueryFailed(String),

    #[error("stored branch reference is corrupt: {0}")]
    StoredReferenceCorrupt(String),

    #[error("failed to create branch '{branch}': {detail}")]
    CreateFailed { branch: String, detail: String },

    #[error("failed to delete branch '{branch}': {detail}")]
    DeleteFailed { branch: String, detail: String },

    #[error("too many arguments")]
    RenameTooManyArgs,

    #[error(transparent)]
    DelegatedCli(#[from] CliError),
}

impl From<BranchError> for CliError {
    fn from(error: BranchError) -> Self {
        match error {
            BranchError::NotInRepo => CliError::repo_not_found(),
            BranchError::InvalidName(name) => {
                CliError::fatal(format!("'{name}' is not a valid branch name"))
                    .with_stable_code(StableErrorCode::CliInvalidArguments)
                    .with_hint(
                        "branch names cannot contain spaces, '..', '@{', or control characters.",
                    )
            }
            BranchError::AlreadyExists(name) => {
                CliError::fatal(format!("a branch named '{name}' already exists"))
                    .with_stable_code(StableErrorCode::ConflictOperationBlocked)
                    .with_hint("delete it first or choose a different name.")
            }
            BranchError::NotFound { name, similar } => {
                let mut err = CliError::fatal(format!("branch '{name}' not found"))
                    .with_stable_code(StableErrorCode::CliInvalidTarget)
                    .with_hint("use 'libra branch -l' to list branches");
                for suggestion in similar {
                    err = err.with_hint(format!("did you mean '{suggestion}'?"));
                }
                err
            }
            BranchError::DeleteCurrent(name) => CliError::fatal(format!(
                "Cannot delete the branch '{name}' which you are currently on"
            ))
            .with_stable_code(StableErrorCode::RepoStateInvalid)
            .with_hint("switch to another branch first."),
            BranchError::NotFullyMerged(name) => {
                CliError::failure(format!("The branch '{name}' is not fully merged."))
                    .with_stable_code(StableErrorCode::RepoStateInvalid)
                    .with_hint(format!(
                        "If you are sure you want to delete it, run 'libra branch -D {name}'."
                    ))
            }
            BranchError::Locked(name) => CliError::fatal(format!(
                "the '{name}' branch is locked and cannot be modified"
            ))
            .with_stable_code(StableErrorCode::ConflictOperationBlocked),
            BranchError::DetachedHead => CliError::fatal("HEAD is detached")
                .with_stable_code(StableErrorCode::RepoStateInvalid)
                .with_hint("checkout a branch first"),
            BranchError::InvalidCommit(target) => {
                CliError::fatal(format!("not a valid object name: '{target}'"))
                    .with_stable_code(StableErrorCode::CliInvalidTarget)
                    .with_hint("use 'libra log --oneline' to see available commits.")
            }
            BranchError::InvalidUpstream(upstream) => {
                CliError::fatal(format!("invalid upstream '{upstream}'"))
                    .with_stable_code(StableErrorCode::CliInvalidTarget)
                    .with_hint("expected format: 'remote/branch'")
            }
            BranchError::ConfigReadFailed(detail) => CliError::fatal(detail)
                .with_stable_code(StableErrorCode::IoReadFailed)
                .with_hint("check whether the repository database is readable."),
            BranchError::ConfigWriteFailed { key, detail } => {
                CliError::fatal(format!("failed to persist branch config '{key}': {detail}"))
                    .with_stable_code(StableErrorCode::IoWriteFailed)
                    .with_hint("check whether the repository database is writable.")
            }
            BranchError::StorageQueryFailed(detail) => {
                CliError::fatal(format!("failed to query branch storage: {detail}"))
                    .with_stable_code(StableErrorCode::IoReadFailed)
            }
            BranchError::StoredReferenceCorrupt(detail) => {
                CliError::fatal(format!("stored branch reference is corrupt: {detail}"))
                    .with_stable_code(StableErrorCode::RepoCorrupt)
            }
            BranchError::CreateFailed { branch, detail } => {
                CliError::fatal(format!("failed to create branch '{branch}': {detail}"))
                    .with_stable_code(StableErrorCode::IoWriteFailed)
            }
            BranchError::DeleteFailed { branch, detail } => {
                CliError::fatal(format!("failed to delete branch '{branch}': {detail}"))
                    .with_stable_code(StableErrorCode::IoWriteFailed)
            }
            BranchError::RenameTooManyArgs => CliError::command_usage("too many arguments")
                .with_stable_code(StableErrorCode::CliInvalidArguments)
                .with_hint("usage: libra branch -m [old-name] new-name"),
            BranchError::DelegatedCli(cli_error) => cli_error,
        }
    }
}

fn detached_head_branch_error() -> BranchError {
    BranchError::DetachedHead
}

fn map_branch_store_error(error: branch::BranchStoreError) -> BranchError {
    match error {
        branch::BranchStoreError::Query(detail) => BranchError::StorageQueryFailed(detail),
        branch::BranchStoreError::Corrupt { detail, .. } => {
            BranchError::StoredReferenceCorrupt(detail)
        }
        branch::BranchStoreError::NotFound(name) => BranchError::NotFound {
            name,
            similar: Vec::new(),
        },
        branch::BranchStoreError::Delete { name, detail } => BranchError::DeleteFailed {
            branch: name,
            detail,
        },
    }
}

fn map_head_commit_store_error(error: branch::BranchStoreError) -> BranchError {
    let cli_error = match error {
        branch::BranchStoreError::Query(detail) => {
            CliError::fatal(format!("failed to resolve HEAD commit: {detail}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        }
        other => CliError::fatal(format!("failed to resolve HEAD commit: {other}"))
            .with_stable_code(StableErrorCode::RepoCorrupt),
    };
    BranchError::DelegatedCli(cli_error)
}

fn find_similar_branch_names(branch_name: &str, branches: &[Branch]) -> Vec<String> {
    let target_len = branch_name.chars().count();
    let mut best: Option<(usize, String)> = None;

    for branch in branches {
        if branch.name.chars().count().abs_diff(target_len) > 2 {
            continue;
        }

        let distance = levenshtein(&branch.name, branch_name);
        if distance > 2 {
            continue;
        }

        match &mut best {
            Some((best_distance, best_name))
                if distance < *best_distance
                    || (distance == *best_distance && branch.name < *best_name) =>
            {
                *best_distance = distance;
                *best_name = branch.name.clone();
            }
            None => best = Some((distance, branch.name.clone())),
            _ => {}
        }
    }

    best.into_iter().map(|(_, name)| name).collect()
}

async fn branch_not_found_error(branch_name: &str) -> BranchError {
    match Branch::list_branches_result(None).await {
        Ok(branches) => BranchError::NotFound {
            name: branch_name.to_string(),
            similar: find_similar_branch_names(branch_name, &branches),
        },
        Err(error) => map_branch_store_error(error),
    }
}

async fn require_existing_local_branch(branch_name: &str) -> Result<Branch, BranchError> {
    match Branch::find_branch_result(branch_name, None)
        .await
        .map_err(map_branch_store_error)?
    {
        Some(branch) => Ok(branch),
        None => Err(branch_not_found_error(branch_name).await),
    }
}

fn branch_config_read_error(scope: impl Into<String>, error: impl ToString) -> BranchError {
    let scope = scope.into();
    BranchError::ConfigReadFailed(format!("failed to read {scope}: {}", error.to_string()))
}

fn branch_config_write_error(key: &str, error: impl ToString) -> BranchError {
    BranchError::ConfigWriteFailed {
        key: key.to_string(),
        detail: error.to_string(),
    }
}

async fn set_upstream_with_conn<C: ConnectionTrait>(
    db: &C,
    branch: &str,
    upstream: &str,
) -> Result<(), BranchError> {
    let (remote, remote_branch) = upstream
        .split_once('/')
        .ok_or_else(|| BranchError::InvalidUpstream(upstream.to_string()))?;
    let branch_config = ConfigKv::branch_config_with_conn(db, branch)
        .await
        .map_err(|e| {
            branch_config_read_error(format!("upstream config for branch '{branch}'"), e)
        })?;
    let merge_ref = format!("refs/heads/{remote_branch}");
    // `branch_config_with_conn()` normalizes `refs/heads/<name>` to `<name>`,
    // so the idempotency check must compare against the short branch name.
    let should_write = branch_config
        .as_ref()
        .map(|config| config.remote != remote || config.merge != remote_branch)
        .unwrap_or(true);

    if should_write {
        let remote_key = format!("branch.{branch}.remote");
        ConfigKv::set_with_conn(db, &remote_key, remote, false)
            .await
            .map_err(|e| branch_config_write_error(&remote_key, e))?;
        let merge_key = format!("branch.{branch}.merge");
        ConfigKv::set_with_conn(db, &merge_key, &merge_ref, false)
            .await
            .map_err(|e| branch_config_write_error(&merge_key, e))?;
    }

    Ok(())
}

async fn set_upstream_impl(branch: &str, upstream: &str) -> Result<(), BranchError> {
    let db = get_db_conn_instance().await;
    set_upstream_with_conn(&db, branch, upstream).await
}

async fn load_remote_branches_with_conn<C: ConnectionTrait>(
    db: &C,
) -> Result<Vec<Branch>, BranchError> {
    let remote_configs = ConfigKv::all_remote_configs_with_conn(db)
        .await
        .map_err(|e| branch_config_read_error("remote configuration", e))?;
    let mut remote_branches = Vec::new();
    for remote in remote_configs {
        remote_branches.extend(
            Branch::list_branches_result_with_conn(db, Some(&remote.name))
                .await
                .map_err(map_branch_store_error)?,
        );
    }
    Ok(remote_branches)
}

async fn load_remote_branches() -> Result<Vec<Branch>, BranchError> {
    let db = get_db_conn_instance().await;
    load_remote_branches_with_conn(&db).await
}

async fn create_branch_impl(
    new_branch: String,
    branch_or_commit: Option<String>,
) -> Result<BranchOutput, BranchError> {
    tracing::debug!("create branch: {} from {:?}", new_branch, branch_or_commit);

    if !is_valid_git_branch_name(&new_branch) {
        return Err(BranchError::InvalidName(new_branch));
    }
    if branch::is_locked_branch(&new_branch) {
        return Err(BranchError::Locked(new_branch));
    }

    if Branch::find_branch_result(&new_branch, None)
        .await
        .map_err(map_branch_store_error)?
        .is_some()
    {
        return Err(BranchError::AlreadyExists(new_branch));
    }

    let base_name = branch_or_commit.clone();
    let commit_id = match branch_or_commit {
        Some(branch_or_commit) => get_target_commit(&branch_or_commit)
            .await
            .map_err(|_| BranchError::InvalidCommit(branch_or_commit))?,
        None => {
            if let Some(commit_id) = Head::current_commit_result()
                .await
                .map_err(map_head_commit_store_error)?
            {
                commit_id
            } else {
                let current = match Head::current().await {
                    Head::Branch(name) => name,
                    Head::Detached(commit_hash) => commit_hash.to_string(),
                };
                return Err(BranchError::InvalidCommit(current));
            }
        }
    };

    let commit_id_display = commit_id.to_string();
    load_object::<Commit>(&commit_id).map_err(|_| {
        BranchError::InvalidCommit(
            base_name
                .as_deref()
                .unwrap_or(commit_id_display.as_str())
                .to_string(),
        )
    })?;

    Branch::update_branch(&new_branch, &commit_id.to_string(), None)
        .await
        .map_err(|e| BranchError::CreateFailed {
            branch: new_branch.clone(),
            detail: e.to_string(),
        })?;

    Ok(BranchOutput::Create {
        name: new_branch,
        commit: commit_id_display,
    })
}

async fn delete_branch_impl(branch_name: String, force: bool) -> Result<BranchOutput, BranchError> {
    if branch::is_locked_branch(&branch_name) {
        return Err(BranchError::Locked(branch_name));
    }

    let branch = require_existing_local_branch(&branch_name).await?;
    let head = Head::current().await;
    if let Head::Branch(name) = &head
        && name == &branch_name
    {
        return Err(BranchError::DeleteCurrent(branch_name));
    }

    if !force {
        let head_commit = match head {
            Head::Branch(_) => Head::current_commit_result()
                .await
                .map_err(map_head_commit_store_error)?
                .ok_or_else(|| {
                    BranchError::DelegatedCli(
                        CliError::fatal("cannot get HEAD commit")
                            .with_stable_code(StableErrorCode::RepoStateInvalid),
                    )
                })?,
            Head::Detached(commit_hash) => commit_hash,
        };

        let head_reachable =
            crate::command::log::get_reachable_commits(head_commit.to_string(), None)
                .await
                .map_err(BranchError::DelegatedCli)?;
        let head_commit_ids: std::collections::HashSet<_> =
            head_reachable.iter().map(|c| c.id).collect();
        if !head_commit_ids.contains(&branch.commit) {
            return Err(BranchError::NotFullyMerged(branch_name));
        }
    }

    Branch::delete_branch_result(&branch_name, None)
        .await
        .map_err(map_branch_store_error)?;

    Ok(BranchOutput::Delete {
        name: branch_name,
        commit: branch.commit.to_string(),
        force,
    })
}

async fn rename_branch_impl(args: &[String]) -> Result<BranchOutput, BranchError> {
    let (old_name, new_name) = match args.len() {
        1 => match Head::current().await {
            Head::Branch(name) => (name, args[0].clone()),
            Head::Detached(_) => return Err(detached_head_branch_error()),
        },
        2 => (args[0].clone(), args[1].clone()),
        _ => return Err(BranchError::RenameTooManyArgs),
    };

    if !is_valid_git_branch_name(&new_name) {
        return Err(BranchError::InvalidName(new_name));
    }
    if branch::is_locked_branch(&new_name) {
        return Err(BranchError::Locked(new_name));
    }
    if branch::is_locked_branch(&old_name) {
        return Err(BranchError::Locked(old_name));
    }

    let old_branch = require_existing_local_branch(&old_name).await?;
    if Branch::find_branch_result(&new_name, None)
        .await
        .map_err(map_branch_store_error)?
        .is_some()
    {
        return Err(BranchError::AlreadyExists(new_name));
    }

    let commit_hash = old_branch.commit.to_string();
    Branch::update_branch(&new_name, &commit_hash, None)
        .await
        .map_err(|e| BranchError::CreateFailed {
            branch: new_name.clone(),
            detail: e.to_string(),
        })?;

    if let Head::Branch(name) = Head::current().await
        && name == old_name
    {
        Head::update(Head::Branch(new_name.clone()), None).await;
    }

    Branch::delete_branch_result(&old_name, None)
        .await
        .map_err(map_branch_store_error)?;

    Ok(BranchOutput::Rename { old_name, new_name })
}

async fn collect_branch_output(args: &BranchArgs) -> Result<BranchOutput, BranchError> {
    let list_mode = if args.all {
        BranchListMode::All
    } else if args.remotes {
        BranchListMode::Remote
    } else {
        BranchListMode::Local
    };
    let has_commit_filters = !args.contains.is_empty() || !args.no_contains.is_empty();
    let (head_name, detached_head) = match Head::current().await {
        Head::Branch(name) => (Some(name), None),
        Head::Detached(commit_hash) => (None, Some(commit_hash.to_string())),
    };

    let mut local_branches = match list_mode {
        BranchListMode::Local | BranchListMode::All => Branch::list_branches_result(None)
            .await
            .map_err(map_branch_store_error)?,
        BranchListMode::Remote => vec![],
    };
    let mut remote_branches = if matches!(list_mode, BranchListMode::Remote | BranchListMode::All) {
        load_remote_branches().await?
    } else {
        vec![]
    };

    let contains_set = resolve_commits(&args.contains)
        .await
        .map_err(BranchError::DelegatedCli)?;
    let no_contains_set = resolve_commits(&args.no_contains)
        .await
        .map_err(BranchError::DelegatedCli)?;
    for branches in [&mut local_branches, &mut remote_branches] {
        filter_branches(branches, &contains_set, &no_contains_set)
            .map_err(BranchError::DelegatedCli)?;
    }

    let current_name = head_name.as_deref();
    let mut entries = Vec::new();
    for branch in local_branches {
        entries.push(BranchListEntry {
            current: current_name == Some(branch.name.as_str()),
            commit: branch.commit.to_string(),
            display_name: branch.name.clone(),
            name: branch.name,
        });
    }
    for branch in remote_branches {
        entries.push(BranchListEntry {
            current: false,
            commit: branch.commit.to_string(),
            display_name: format_branch_name(&branch),
            name: branch.name,
        });
    }

    let show_unborn_head = entries.is_empty()
        && detached_head.is_none()
        && !has_commit_filters
        && matches!(list_mode, BranchListMode::Local | BranchListMode::All)
        && head_name.is_some();

    Ok(BranchOutput::List {
        branches: entries,
        head_name,
        detached_head,
        show_unborn_head,
    })
}

async fn run_branch(args: &BranchArgs) -> Result<BranchOutput, BranchError> {
    crate::utils::util::require_repo().map_err(|_| BranchError::NotInRepo)?;

    if let Some(new_branch) = args.new_branch.clone() {
        create_branch_impl(new_branch, args.commit_hash.clone()).await
    } else if let Some(branch_to_delete) = args.delete.clone() {
        delete_branch_impl(branch_to_delete, true).await
    } else if let Some(branch_to_delete) = args.delete_safe.clone() {
        delete_branch_impl(branch_to_delete, false).await
    } else if args.show_current {
        let head = Head::current().await;
        let output = match head {
            Head::Branch(name) => BranchOutput::ShowCurrent {
                name: Some(name),
                detached: false,
                commit: Head::current_commit_result()
                    .await
                    .map_err(map_head_commit_store_error)?
                    .map(|hash| hash.to_string()),
            },
            Head::Detached(hash) => BranchOutput::ShowCurrent {
                name: None,
                detached: true,
                commit: Some(hash.to_string()),
            },
        };
        Ok(output)
    } else if let Some(upstream) = args.set_upstream_to.as_deref() {
        let branch = match Head::current().await {
            Head::Branch(name) => name,
            Head::Detached(_) => return Err(detached_head_branch_error()),
        };
        set_upstream_impl(&branch, upstream).await?;
        Ok(BranchOutput::SetUpstream {
            branch,
            upstream: upstream.to_string(),
        })
    } else if !args.rename.is_empty() {
        rename_branch_impl(&args.rename).await
    } else {
        collect_branch_output(args).await
    }
}

fn render_branch_output(result: &BranchOutput, output: &OutputConfig) -> CliResult<()> {
    if output.is_json() {
        return emit_json_data("branch", result, output);
    }
    if output.quiet {
        return Ok(());
    }

    match result {
        BranchOutput::List {
            branches,
            head_name,
            detached_head,
            show_unborn_head,
        } => {
            if let Some(detached_head) = detached_head {
                println!(
                    "HEAD detached at {}",
                    short_display_hash(detached_head).green()
                );
            }
            if branches.is_empty() {
                if *show_unborn_head && let Some(head_name) = head_name {
                    println!("* {}", head_name.green());
                }
                return Ok(());
            }

            let mut sorted = branches.clone();
            sorted.sort_by(|a, b| {
                if a.current {
                    std::cmp::Ordering::Less
                } else if b.current {
                    std::cmp::Ordering::Greater
                } else {
                    a.name.cmp(&b.name)
                }
            });

            for branch in sorted {
                if branch.current {
                    println!("* {}", branch.display_name.green());
                } else {
                    println!("  {}", branch.display_name);
                }
            }
        }
        BranchOutput::Create { name, commit } => {
            println!("Created branch '{name}' at {}", short_display_hash(commit));
        }
        BranchOutput::Delete {
            name,
            commit,
            force: _,
        } => {
            println!(
                "Deleted branch {name} (was {}).",
                short_display_hash(commit)
            );
        }
        BranchOutput::Rename { old_name, new_name } => {
            println!("Renamed branch '{old_name}' to '{new_name}'");
        }
        BranchOutput::SetUpstream { branch, upstream } => {
            println!("Branch '{branch}' set up to track remote branch '{upstream}'");
        }
        BranchOutput::ShowCurrent {
            name,
            detached,
            commit,
        } => {
            if *detached {
                if let Some(commit) = commit {
                    println!("HEAD detached at {}", short_display_hash(commit));
                }
            } else if let Some(name) = name {
                println!("{name}");
            }
        }
    }

    Ok(())
}

pub async fn set_upstream(branch: &str, upstream: &str) {
    if let Err(err) = set_upstream_safe(branch, upstream).await {
        err.print_stderr();
    }
}

pub async fn set_upstream_safe(branch: &str, upstream: &str) -> CliResult<()> {
    set_upstream_safe_with_output(branch, upstream, &OutputConfig::default()).await
}

pub async fn set_upstream_safe_with_output(
    branch: &str,
    upstream: &str,
    output: &OutputConfig,
) -> CliResult<()> {
    set_upstream_impl(branch, upstream)
        .await
        .map_err(CliError::from)?;
    crate::info_println!(
        output,
        "Branch '{branch}' set up to track remote branch '{upstream}'"
    );
    Ok(())
}

pub async fn create_branch(new_branch: String, branch_or_commit: Option<String>) {
    if let Err(err) = create_branch_safe(new_branch, branch_or_commit).await {
        err.print_stderr();
    }
}

pub async fn create_branch_safe(
    new_branch: String,
    branch_or_commit: Option<String>,
) -> CliResult<()> {
    create_branch_impl(new_branch, branch_or_commit)
        .await
        .map(|_| ())
        .map_err(CliError::from)?;
    Ok(())
}

fn format_branch_name(branch: &Branch) -> String {
    let display_name = if let Some(stripped) = branch.name.strip_prefix("refs/remotes/") {
        stripped.to_string()
    } else {
        branch
            .remote
            .as_ref()
            .map(|remote| format!("{remote}/{}", branch.name))
            .unwrap_or_else(|| branch.name.clone())
    };
    display_name.red().to_string()
}

/// List branches with the given mode and commit filters, rendering directly to stdout.
///
/// This is a convenience wrapper around the structured `run_branch` path,
/// kept for backward compatibility with callers that need a simple
/// "print branches" operation.
pub async fn list_branches(
    list_mode: BranchListMode,
    commits_contains: &[String],
    commits_no_contains: &[String],
) -> CliResult<()> {
    let args = BranchArgs {
        new_branch: None,
        commit_hash: None,
        list: true,
        delete: None,
        delete_safe: None,
        set_upstream_to: None,
        show_current: false,
        rename: vec![],
        remotes: matches!(list_mode, BranchListMode::Remote),
        all: matches!(list_mode, BranchListMode::All),
        contains: commits_contains.to_vec(),
        no_contains: commits_no_contains.to_vec(),
    };
    let result = collect_branch_output(&args).await.map_err(CliError::from)?;
    render_branch_output(&result, &OutputConfig::default())
}

/// Filter given branches by whether they contain or don't contain certain commits.
///
/// Internal test helper — not part of the stable public API.
#[doc(hidden)]
pub fn filter_branches(
    branches: &mut Vec<Branch>,
    contains_set: &HashSet<ObjectHash>,
    no_contains_set: &HashSet<ObjectHash>,
) -> CliResult<()> {
    // Filter branches, propagating errors.
    // `retain` doesn't support fallible predicates, so we capture the first
    // error and short-circuit the remaining iterations.
    let mut error: Option<CliError> = None;
    branches.retain(|branch| {
        if error.is_some() {
            return false;
        }
        let contains_ok = contains_set.is_empty()
            || match commit_contains(branch, contains_set) {
                Ok(v) => v,
                Err(e) => {
                    error = Some(e);
                    return false;
                }
            };
        let no_contains_ok = no_contains_set.is_empty()
            || match commit_contains(branch, no_contains_set) {
                Ok(v) => !v,
                Err(e) => {
                    error = Some(e);
                    return false;
                }
            };
        contains_ok && no_contains_ok
    });
    if let Some(e) = error {
        return Err(e);
    }
    Ok(())
}

/// Resolve commit references to ObjectHash set.
async fn resolve_commits(commits: &[String]) -> CliResult<HashSet<ObjectHash>> {
    let mut set = HashSet::new();
    for commit in commits {
        let target_commit = get_target_commit(commit).await.map_err(|e| {
            CliError::fatal(format!("{}", e)).with_stable_code(StableErrorCode::CliInvalidTarget)
        })?;
        set.insert(target_commit);
    }
    Ok(set)
}

/// check if a branch contains at least one of the commits
///
/// NOTE: returns `false` if `commits` is empty
fn commit_contains(
    branch: &Branch,
    target_commits: &HashSet<ObjectHash>,
) -> Result<bool, CliError> {
    // do BFS to find out whether `branch` contains `target_commit` or not
    let mut q = VecDeque::new();
    let mut visited = HashSet::new();

    q.push_back(branch.commit);
    visited.insert(branch.commit);

    while let Some(current_commit) = q.pop_front() {
        // found target commit
        if target_commits.contains(&current_commit) {
            return Ok(true);
        }

        // enqueue all parent commits of `current_commit`
        let current_commit_object: Commit = load_object(&current_commit).map_err(|e| {
            CliError::fatal(format!("failed to load commit {}: {}", current_commit, e))
                .with_stable_code(StableErrorCode::RepoCorrupt)
        })?;
        for parent_commit in current_commit_object.parent_commit_ids {
            if !visited.contains(&parent_commit) {
                visited.insert(parent_commit);
                q.push_back(parent_commit);
            }
        }
    }

    // contains no commits
    Ok(false)
}

pub fn is_valid_git_branch_name(name: &str) -> bool {
    // Validate branch name
    // Not contain spaces, control characters or special characters
    if name.contains(&[' ', '\t', '\\', ':', '"', '?', '*', '['][..])
        || name.chars().any(|c| c.is_ascii_control())
    {
        return false;
    }

    // Not start or end with a slash ('/'), or end with a dot ('.')
    // Not contain consecutive slashes ('//') or dots ('..')
    if name.starts_with('/')
        || name.ends_with('/')
        || name.ends_with('.')
        || name.contains("//")
        || name.contains("..")
    {
        return false;
    }

    // Not be reserved names like 'HEAD' or contain '@{'
    if name == "HEAD" || name.contains("@{") {
        return false;
    }

    // Not be empty or just a dot ('.')
    if name.trim().is_empty() || name.trim() == "." {
        return false;
    }

    true
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use git_internal::hash::{ObjectHash, get_hash_kind};
    use sea_orm::Database;

    use super::{
        Branch, BranchError, format_branch_name, load_remote_branches_with_conn,
        map_head_commit_store_error,
    };
    use crate::utils::error::{CliError, StableErrorCode};

    fn any_hash() -> ObjectHash {
        ObjectHash::from_str(&ObjectHash::zero_str(get_hash_kind())).unwrap()
    }

    #[test]
    fn test_format_branch_name_with_full_remote_ref() {
        colored::control::set_override(false);
        let branch = Branch {
            name: "refs/remotes/origin/main".to_string(),
            commit: any_hash(),
            remote: Some("origin".to_string()),
        };

        assert_eq!(format_branch_name(&branch), "origin/main");
    }

    #[test]
    fn test_format_branch_name_with_short_remote_ref() {
        colored::control::set_override(false);
        let branch = Branch {
            name: "main".to_string(),
            commit: any_hash(),
            remote: Some("origin".to_string()),
        };

        assert_eq!(format_branch_name(&branch), "origin/main");
    }

    #[tokio::test]
    async fn test_load_remote_branches_with_conn_surfaces_config_read_failure() {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        db.clone().close().await.unwrap();

        let error = load_remote_branches_with_conn(&db).await.unwrap_err();
        match error {
            BranchError::ConfigReadFailed(detail) => {
                assert!(detail.contains("failed to read remote configuration"));
            }
            other => panic!("expected config read failure, got {other:?}"),
        }
    }

    #[test]
    fn test_head_commit_query_error_maps_to_io_read_failed() {
        let cli_error = CliError::from(map_head_commit_store_error(
            crate::internal::branch::BranchStoreError::Query("database is locked".into()),
        ));
        assert_eq!(cli_error.stable_code(), StableErrorCode::IoReadFailed);
    }
}
