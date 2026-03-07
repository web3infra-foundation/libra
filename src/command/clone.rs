//! Supports cloning repositories by parsing URLs, fetching objects via protocol
//! clients, checking out the working tree, and writing initial refs/config.

use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

use clap::Parser;
use git_internal::hash::{ObjectHash, get_hash_kind};
use sea_orm::DatabaseTransaction;

use super::fetch;
use crate::{
    command::{self, restore::RestoreArgs},
    internal::{
        branch::Branch,
        config::{Config, RemoteConfig},
        head::Head,
        reflog::{ReflogAction, ReflogContext, with_reflog},
    },
    utils::{
        error::{CliError, CliResult},
        util,
    },
};

#[derive(Parser, Debug, Clone)]
pub struct CloneArgs {
    /// The remote repository location to clone from, usually a URL with HTTPS or SSH
    pub remote_repo: String,

    /// The local path to clone the repository to
    pub local_path: Option<String>,

    /// Checkout <BRANCH> instead of the remote's HEAD
    #[clap(short = 'b', long, required = false)]
    pub branch: Option<String>,

    /// Clone only one branch, HEAD or --branch
    #[clap(long)]
    pub single_branch: bool,

    /// Create a bare repository without checking out a working tree
    #[clap(long)]
    pub bare: bool,

    /// Create a shallow clone with a history truncated to the specified number of commits
    #[clap(long, value_name = "DEPTH", value_parser = validate_depth)]
    pub depth: Option<usize>,
}

const REPO_MARKERS: &[&str] = &["description", "libra.db", "info/exclude", "objects"];

#[derive(thiserror::Error, Debug)]
pub enum CloneError {
    #[error("please specify the destination path explicitly")]
    CannotInferDestination,
    #[error("destination path '{path}' already exists and is not an empty directory")]
    DestinationExistsNonEmpty { path: PathBuf },
    #[error("destination path '{path}' already contains a libra repository")]
    DestinationAlreadyRepo { path: PathBuf },
    #[error("could not create directory '{path}': {source}")]
    CreateDestinationFailed { path: PathBuf, source: io::Error },
    #[error("{message}")]
    InvalidRemote { message: String },
    #[error("failed to change working directory to '{path}': {source}")]
    ChangeDirectory { path: PathBuf, source: io::Error },
    #[error("failed to restore working directory to '{path}': {source}")]
    RestoreDirectory { path: PathBuf, source: io::Error },
    #[error("failed to initialize repository: {message}")]
    InitializeRepository { message: String },
    #[error("remote branch {branch} not found in upstream origin")]
    RemoteBranchNotFound { branch: String },
    #[error("fetch failed: {source}")]
    FetchFailed { source: fetch::FetchError },
    #[error("failed to complete clone setup: {message}")]
    SetupFailed { message: String },
}

impl From<CloneError> for CliError {
    fn from(error: CloneError) -> Self {
        match &error {
            CloneError::CannotInferDestination => CliError::fatal(error.to_string())
                .with_hint("please specify the destination path explicitly."),
            _ => CliError::fatal(error.to_string()),
        }
    }
}

fn contains_initialized_repo(metadata_root: &Path) -> bool {
    REPO_MARKERS
        .iter()
        .any(|marker| metadata_root.join(marker).exists())
}

pub async fn execute(args: CloneArgs) {
    if let Err(err) = execute_safe(args).await {
        eprintln!("{}", err.render());
    }
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting. Fetches objects from a remote URL, writes refs/config,
/// and checks out the working tree. Restores the original working directory on
/// failure.
pub async fn execute_safe(args: CloneArgs) -> CliResult<()> {
    let original_dir = util::cur_dir();
    let result = execute_clone(args, &original_dir).await;

    if env::current_dir().ok().as_ref() != Some(&original_dir) {
        env::set_current_dir(&original_dir).map_err(|source| {
            CliError::from(CloneError::RestoreDirectory {
                path: original_dir.clone(),
                source,
            })
        })?;
    }

    result.map_err(CliError::from)
}

async fn execute_clone(args: CloneArgs, original_dir: &Path) -> Result<(), CloneError> {
    let mut remote_repo = args.remote_repo.clone();
    if !remote_repo.ends_with('/') {
        remote_repo.push('/');
    }

    let (remote_client, discovery) =
        fetch::discover_remote(&remote_repo)
            .await
            .map_err(|error| CloneError::InvalidRemote {
                message: error.to_string(),
            })?;

    let local_path = match args.local_path.clone() {
        Some(path) => path,
        None => {
            let repo_name = util::get_repo_name_from_url(&remote_repo)
                .ok_or(CloneError::CannotInferDestination)?;
            original_dir.join(repo_name).to_string_lossy().into_owned()
        }
    };

    let local_path = PathBuf::from(local_path);
    let local_path = if local_path.is_absolute() {
        local_path
    } else {
        original_dir.join(&local_path)
    };
    let metadata_root = if args.bare {
        local_path.clone()
    } else {
        local_path.join(util::ROOT_DIR)
    };

    if metadata_root.exists() && contains_initialized_repo(&metadata_root) {
        return Err(CloneError::DestinationAlreadyRepo {
            path: local_path.clone(),
        });
    }
    if local_path.exists() && !util::is_empty_dir(&local_path) {
        return Err(CloneError::DestinationExistsNonEmpty {
            path: local_path.clone(),
        });
    }

    let created_by_clone = if local_path.exists() {
        false
    } else {
        fs::create_dir_all(&local_path).map_err(|source| CloneError::CreateDestinationFailed {
            path: local_path.clone(),
            source,
        })?;
        true
    };

    let repo_name = local_path
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| local_path.to_string_lossy().into_owned());
    if args.bare {
        eprintln!("Cloning into bare repository '{repo_name}'...");
    } else {
        eprintln!("Cloning into '{repo_name}'...");
    }

    if let Some(branch) = &args.branch
        && !fetch::remote_has_branch(&discovery.refs, branch)
    {
        return Err(CloneError::RemoteBranchNotFound {
            branch: branch.clone(),
        });
    }

    if let Err(error) = clone_into_destination(
        args,
        &remote_repo,
        &remote_client,
        &discovery,
        &local_path,
        original_dir,
    )
    .await
    {
        cleanup_failed_clone(&local_path, created_by_clone);
        return Err(error);
    }

    eprintln!("done.");

    Ok(())
}

async fn clone_into_destination(
    args: CloneArgs,
    remote_repo: &str,
    remote_client: &fetch::RemoteClient,
    discovery: &crate::internal::protocol::DiscoveryResult,
    local_path: &Path,
    original_dir: &Path,
) -> Result<(), CloneError> {
    env::set_current_dir(local_path).map_err(|source| CloneError::ChangeDirectory {
        path: local_path.to_path_buf(),
        source,
    })?;

    let object_format = match discovery.hash_kind {
        git_internal::hash::HashKind::Sha1 => "sha1".to_string(),
        git_internal::hash::HashKind::Sha256 => "sha256".to_string(),
    };

    command::init::init(command::init::InitArgs {
        bare: args.bare,
        template: None,
        initial_branch: args.branch.clone(),
        repo_directory: local_path.to_string_lossy().into_owned(),
        quiet: true,
        shared: None,
        object_format: Some(object_format),
        ref_format: None,
        from_git_repository: None,
        separate_libra_dir: None,
    })
    .await
    .map_err(|error| CloneError::InitializeRepository {
        message: error.to_string(),
    })?;

    let remote_config = RemoteConfig {
        name: "origin".to_string(),
        url: fetch::normalize_remote_url(remote_repo, remote_client),
    };
    fetch::fetch_repository_safe(
        remote_config.clone(),
        args.branch.clone(),
        args.single_branch,
        args.depth,
    )
    .await
    .map_err(|source| CloneError::FetchFailed { source })?;

    setup_repository(remote_config, args.branch.clone(), !args.bare).await?;

    env::set_current_dir(original_dir).map_err(|source| CloneError::RestoreDirectory {
        path: original_dir.to_path_buf(),
        source,
    })?;

    Ok(())
}

fn cleanup_failed_clone(local_path: &Path, created_by_clone: bool) {
    let cleanup_result = if created_by_clone {
        fs::remove_dir_all(local_path)
    } else {
        clear_directory_contents(local_path)
    };

    if let Err(error) = cleanup_result {
        tracing::error!(
            "failed to clean up clone destination '{}': {}",
            local_path.display(),
            error
        );
    }
}

fn clear_directory_contents(dir: &Path) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            fs::remove_dir_all(path)?;
        } else {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

/// Custom validation function, ensuring depth >= 1
fn validate_depth(s: &str) -> Result<usize, String> {
    s.parse::<usize>()
        .map_err(|_| "DEPTH must be a valid integer".to_string())
        .and_then(|val| {
            if val >= 1 {
                Ok(val)
            } else {
                Err("DEPTH must be greater than or equal to 1".to_string())
            }
        })
}

/// Sets up the local repository after a clone by configuring the remote,
/// setting up the initial branch and HEAD, and creating the first reflog entry.
/// Skips checking out the worktree when `checkout_worktree` is `false` (bare clone).
/// This function is `pub(crate)` to allow reuse by the `convert` module for
/// importing existing Git repositories during `libra init --from-git-repository`.
pub(crate) async fn setup_repository(
    remote_config: RemoteConfig,
    specified_branch: Option<String>,
    checkout_worktree: bool,
) -> Result<(), CloneError> {
    let db = crate::internal::db::get_db_conn_instance().await;
    let remote_head = Head::remote_current_with_conn(db, &remote_config.name).await;

    let branch_to_checkout = match specified_branch {
        Some(branch_name) => Some(branch_name),
        None => match remote_head {
            Some(Head::Branch(name)) => Some(name),
            _ => None,
        },
    };

    if let Some(branch_name) = branch_to_checkout {
        let remote_tracking_ref = format!("refs/remotes/{}/{}", remote_config.name, branch_name);
        let origin_branch =
            Branch::find_branch_with_conn(db, &remote_tracking_ref, Some(&remote_config.name))
                .await
                .ok_or_else(|| CloneError::RemoteBranchNotFound {
                    branch: branch_name.clone(),
                })?;

        let action = ReflogAction::Clone {
            from: remote_config.url.clone(),
        };
        let context = ReflogContext {
            old_oid: ObjectHash::zero_str(get_hash_kind()).to_string(),
            new_oid: origin_branch.commit.to_string(),
            action,
        };

        with_reflog(
            context,
            move |txn: &DatabaseTransaction| {
                Box::pin(async move {
                    Branch::update_branch_with_conn(
                        txn,
                        &branch_name,
                        &origin_branch.commit.to_string(),
                        None,
                    )
                    .await;
                    Head::update_with_conn(txn, Head::Branch(branch_name.to_owned()), None).await;

                    let merge_ref = format!("refs/heads/{}", branch_name);
                    Config::insert_with_conn(
                        txn,
                        "branch",
                        Some(&branch_name),
                        "merge",
                        &merge_ref,
                    )
                    .await;
                    Config::insert_with_conn(
                        txn,
                        "branch",
                        Some(&branch_name),
                        "remote",
                        &remote_config.name,
                    )
                    .await;
                    Config::insert_with_conn(
                        txn,
                        "remote",
                        Some(&remote_config.name),
                        "url",
                        &remote_config.url,
                    )
                    .await;
                    Ok(())
                })
            },
            true,
        )
        .await
        .map_err(|error| CloneError::SetupFailed {
            message: error.to_string(),
        })?;

        if checkout_worktree {
            command::restore::execute(RestoreArgs {
                worktree: true,
                staged: true,
                source: None,
                pathspec: vec![util::working_dir_string()],
            })
            .await;
        }
    } else {
        eprintln!("warning: You appear to have cloned an empty repository.");

        Config::insert(
            "remote",
            Some(&remote_config.name),
            "url",
            &remote_config.url,
        )
        .await;

        let default_branch = "main";
        let merge_ref = format!("refs/heads/{}", default_branch);
        Config::insert("branch", Some(default_branch), "merge", &merge_ref).await;
        Config::insert(
            "branch",
            Some(default_branch),
            "remote",
            &remote_config.name,
        )
        .await;
    }

    Ok(())
}

/// Unit tests for the clone module
#[cfg(test)]
mod tests {}
