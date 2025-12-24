use super::fetch::{self};
use crate::command::restore::RestoreArgs;
use crate::command::{self, branch};
use crate::internal::branch::Branch;
use crate::internal::config::{Config, RemoteConfig};
use crate::internal::head::Head;
use crate::internal::reflog::{ReflogAction, ReflogContext, with_reflog};
use crate::utils::path_ext::PathExt;
use crate::utils::util;
use clap::Parser;
use colored::Colorize;
use git_internal::hash::{ObjectHash, get_hash_kind};
use scopeguard::defer;
use sea_orm::DatabaseTransaction;
use std::cell::Cell;
use std::path::PathBuf;
use std::{env, fs};

#[derive(Parser, Debug)]
pub struct CloneArgs {
    /// The remote repository location to clone from, usually a URL with HTTPS or SSH
    pub remote_repo: String,

    /// The local path to clone the repository to
    pub local_path: Option<String>,

    /// The branch to clone
    #[clap(short = 'b', long, required = false)]
    pub branch: Option<String>,
}

pub async fn execute(args: CloneArgs) {
    let mut remote_repo = args.remote_repo;
    // ensure URL ends with a slash for correct URL joining
    if !remote_repo.ends_with('/') {
        remote_repo.push('/');
    }
    
    let local_path = args.local_path.unwrap_or_else(|| {
        let repo_name = util::get_repo_name_from_url(&remote_repo).unwrap();
        util::cur_dir().join(repo_name).to_string_or_panic()
    });

    let local_path = PathBuf::from(local_path);
    {
        if local_path.exists() && !util::is_empty_dir(&local_path) {
            eprintln!(
                "fatal: destination path '{}' already exists and is not an empty directory.",
                local_path.display()
            );
            return;
        }

        // create the directory if it doesn't exist
        if let Err(e) = fs::create_dir_all(&local_path) {
            eprintln!(
                "fatal: could not create directory '{}': {}",
                local_path.display(),
                e
            );
            return;
        }
        let repo_name = local_path.file_name().unwrap().to_str().unwrap();
        println!("Cloning into '{repo_name}'...");
    }

    let is_success = Cell::new(false);
    defer! {
        if !is_success.get() {
            if let Err(e) = fs::remove_dir_all(&local_path) {
                eprintln!("fatal: failed to clean up after clone failure: {}", e);
            } else {
                eprintln!("{}", "fatal: clone failed, repo directory deleted".red());
            }
        }
    }

    // Validate the branch name if specified
    if let Some(ref branch) = args.branch
        && !branch::is_valid_git_branch_name(branch)
    {
        eprintln!(
            "fatal: invalid branch name: '{branch}'.\nBranch names must:\n\
            - Not contain spaces, control characters, or any of these characters: \\ : \" ? * [\n\
            - Not start or end with a slash ('/'), or end with a dot ('.')\n\
            - Not contain consecutive slashes ('//') or dots ('..')\n\
            - Not be reserved names like 'HEAD' or contain '@{{'\n\
            - Not be empty or just a dot ('.')."
        );
        return;
    }

    // Set the current directory to the new local path for repo cloning
    env::set_current_dir(&local_path).unwrap();
    let init_args = command::init::InitArgs {
        bare: false,
        initial_branch: args.branch.clone(),
        repo_directory: local_path.to_str().unwrap().to_string(),
        quiet: false,
        template: None,
        shared: None,
        object_format: None,
        separate_git_dir: None,  // Keeping separate_git_dir as None for simplicity
    };
    command::init::execute(init_args).await;

    // Fetch the remote repository
    let remote_config = RemoteConfig {
        name: "origin".to_string(),
        url: remote_repo.clone(),
    };
    fetch::fetch_repository(remote_config.clone(), args.branch.clone()).await;

    // Set up the repository (create branches, configure remote tracking)
    if let Err(e) = setup_repository(remote_config, args.branch.clone()).await {
        eprintln!("fatal: {}", e);
        return;
    }

    // Mark the clone operation as successful
    is_success.set(true);
}

/// Sets up the local repository after a clone by configuring the remote,
/// setting up the initial branch and HEAD, and creating the first reflog entry.
async fn setup_repository(
    remote_config: RemoteConfig,
    specified_branch: Option<String>,
) -> Result<(), String> {
    let db = crate::internal::db::get_db_conn_instance().await;
    let remote_head = Head::remote_current_with_conn(db, &remote_config.name).await;

    let branch_to_checkout = match specified_branch {
        Some(b_name) => Some(b_name),
        None => {
            if let Some(Head::Branch(name)) = remote_head {
                Some(name)
            } else {
                None // For empty repos or detached HEADs
            }
        }
    };

    if let Some(branch_name) = branch_to_checkout {
        let remote_tracking_ref = format!("refs/remotes/{}/{}", remote_config.name, branch_name);
        let origin_branch = Branch::find_branch_with_conn(db, &remote_tracking_ref, None)
            .await
            .ok_or_else(|| format!("fatal: remote branch '{}' not found.", branch_name))?;

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
        .map_err(|e| e.to_string())?;

        // Restore working directory after setup
        command::restore::execute(RestoreArgs {
            worktree: true,
            staged: true,
            source: None,
            pathspec: vec![util::working_dir_string()],
        })
        .await;
    } else {
        println!("warning: You appear to have cloned an empty repository.");

        Config::insert(
            "remote",
            Some(&remote_config.name),
            "url",
            &remote_config.url,
        )
        .await;

        let default_branch = "master";
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
