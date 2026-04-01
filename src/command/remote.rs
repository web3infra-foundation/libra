//! Manages remotes by listing, showing, adding, and updating URLs and associated fetch/push metadata.

use std::collections::HashSet;

use clap::Subcommand;
use git_internal::hash::get_hash_kind;

use crate::{
    command::fetch::RemoteClient,
    internal::{
        branch::{Branch, BranchStoreError},
        config::ConfigKv,
        protocol::set_wire_hash_kind,
    },
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::OutputConfig,
    },
};

#[derive(Subcommand, Debug)]
pub enum RemoteCmds {
    /// Add a remote
    Add {
        /// The name of the remote
        name: String,
        /// The URL of the remote
        url: String,
    },
    /// Remove a remote
    Remove {
        /// The name of the remote
        name: String,
    },
    /// Rename a remote
    Rename {
        /// The current name of the remote
        old: String,
        /// The new name of the remote
        new: String,
    },
    /// List remotes
    #[command(name = "-v")]
    List,
    /// Show current remote repository
    Show,
    /// Print URLs for the given remote
    ///
    /// Examples:
    /// `libra remote get-url origin` - print the fetch URL (first)
    /// `libra remote get-url --push origin` - print push URLs
    /// `libra remote get-url --all origin` - print all configured URLs
    GetUrl {
        /// Print push URLs instead of fetch URL
        #[arg(long)]
        push: bool,
        /// Print all URLs
        #[arg(long)]
        all: bool,
        /// Remote name
        name: String,
    },
    /// Set or modify URLs for the given remote
    ///
    /// Examples:
    /// `libra remote set-url origin newurl` - replace first url
    /// `libra remote set-url --all origin newurl` - replace all urls
    /// `libra remote set-url --add origin newurl` - add a new url
    /// `libra remote set-url --delete origin urlpattern` - delete matching url(s)
    SetUrl {
        /// Add the new URL instead of replacing
        #[arg(long)]
        add: bool,
        /// Delete the URL instead of adding/replacing
        #[arg(long)]
        delete: bool,
        /// Operate on push URLs (pushurl) instead of fetch URLs (url)
        #[arg(long)]
        push: bool,
        /// Apply to all matching entries
        #[arg(long)]
        all: bool,
        /// Remote name
        name: String,
        /// URL value (or pattern for --delete)
        value: String,
    },

    /// Delete stale remote-tracking branches
    ///
    /// Examples:
    /// `libra remote prune origin` - prune stale branches for origin
    /// `libra remote prune --dry-run origin` - preview what would be pruned
    Prune {
        /// Remote name
        name: String,
        /// Dry run - show what would be pruned without actually deleting
        #[arg(long)]
        dry_run: bool,
    },
}

pub async fn execute(command: RemoteCmds) {
    if let Err(e) = execute_safe(command, &OutputConfig::default()).await {
        e.print_stderr();
    }
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting. Dispatches to remote sub-commands (add, remove, rename,
/// set-url, list, show).
pub async fn execute_safe(command: RemoteCmds, _output: &OutputConfig) -> CliResult<()> {
    match command {
        RemoteCmds::Add { name, url } => {
            if ConfigKv::remote_config(&name)
                .await
                .ok()
                .flatten()
                .is_some()
            {
                return Err(CliError::fatal(format!("remote {name} already exists")));
            }
            let _ = ConfigKv::set(&format!("remote.{name}.url"), &url, false).await;
        }
        RemoteCmds::Remove { name } => {
            ConfigKv::remove_remote(&name)
                .await
                .map_err(|e| CliError::failure(e.to_string()))?;
        }
        RemoteCmds::Rename { old, new } => {
            ConfigKv::rename_remote(&old, &new)
                .await
                .map_err(|e| CliError::failure(e.to_string()))?;
        }
        RemoteCmds::List => {
            let remotes = ConfigKv::all_remote_configs().await.unwrap_or_default();
            for remote in remotes {
                show_remote_verbose(&remote.name).await?;
            }
        }
        RemoteCmds::Show => {
            let remotes = ConfigKv::all_remote_configs().await.unwrap_or_default();
            for remote in remotes {
                println!("{}", remote.name);
            }
        }
        RemoteCmds::GetUrl { push, all, name } => {
            if ConfigKv::remote_config(&name)
                .await
                .ok()
                .flatten()
                .is_none()
            {
                return Err(CliError::fatal(format!("no such remote: {name}")));
            }
            // If --push, prefer explicit pushurl entries; fall back to url if none.
            if push {
                let push_urls = ConfigKv::get_all(&format!("remote.{name}.pushurl"))
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .map(|e| e.value)
                    .collect::<Vec<_>>();
                if !push_urls.is_empty() {
                    if all {
                        for u in push_urls {
                            println!("{}", u);
                        }
                    } else if let Some(u) = push_urls.first() {
                        println!("{}", u);
                    }
                    return Ok(());
                }
                // fall through to read regular url if no pushurl configured
            }

            let urls = ConfigKv::get_all(&format!("remote.{name}.url"))
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|e| e.value)
                .collect::<Vec<_>>();
            if urls.is_empty() {
                return Err(CliError::fatal(format!(
                    "no URL configured for remote '{name}'"
                )));
            } else if all || push {
                // --all prints all URLs; --push with no pushurl also prints all regular urls
                for url in urls {
                    println!("{}", url);
                }
            } else if let Some(url) = urls.first() {
                println!("{}", url);
            }
        }
        RemoteCmds::SetUrl {
            add,
            delete,
            push,
            all,
            name,
            value,
        } => {
            if ConfigKv::remote_config(&name)
                .await
                .ok()
                .flatten()
                .is_none()
            {
                return Err(CliError::fatal(format!("no such remote: {name}")));
            }
            // Determine which config key to operate on
            let key = if push { "pushurl" } else { "url" };

            if add {
                // Insert a new URL entry
                let _ = ConfigKv::add(&format!("remote.{name}.{key}"), &value, false).await;
                return Ok(());
            }

            if delete {
                // Delete only entries whose value contains the pattern
                let full_key = format!("remote.{name}.{key}");
                let entries = ConfigKv::get_all(&full_key).await.unwrap_or_default();
                let remaining: Vec<_> = entries
                    .into_iter()
                    .filter(|e| !e.value.contains(&value))
                    .collect();
                let _ = ConfigKv::unset_all(&full_key).await;
                for r in remaining {
                    let _ = ConfigKv::add(&full_key, &r.value, r.encrypted).await;
                }
                return Ok(());
            }

            // Default: replace behavior
            if all {
                // Remove all existing entries for this key, then insert the new value once
                let _ = ConfigKv::unset_all(&format!("remote.{name}.{key}")).await;
                let _ = ConfigKv::set(&format!("remote.{name}.{key}"), &value, false).await;
            } else {
                // Replace first existing entry: remove all then set new value.
                let _ = ConfigKv::unset_all(&format!("remote.{name}.{key}")).await;
                let _ = ConfigKv::set(&format!("remote.{name}.{key}"), &value, false).await;
            }
        }
        RemoteCmds::Prune { name, dry_run } => {
            prune_remote(&name, dry_run).await?;
        }
    }
    Ok(())
}

async fn show_remote_verbose(remote: &str) -> CliResult<()> {
    // There can be multiple URLs for a remote, like Gitee & GitHub
    let urls = ConfigKv::get_all(&format!("remote.{remote}.url"))
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|e| e.value)
        .collect::<Vec<_>>();
    match urls.first() {
        Some(url) => {
            println!("{remote} {url} (fetch)");
        }
        None => {
            return Err(CliError::fatal(format!(
                "no URL configured for remote '{remote}'"
            )));
        }
    }
    for url in urls {
        println!("{remote} {url} (push)");
    }
    Ok(())
}

fn remote_branch_store_error(context: &str, error: BranchStoreError) -> CliError {
    match error {
        BranchStoreError::Query(detail) => {
            CliError::fatal(format!("failed to {context}: {detail}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        }
        BranchStoreError::Delete { name, detail } => CliError::fatal(format!(
            "failed to delete branch '{name}' while {context}: {detail}"
        ))
        .with_stable_code(StableErrorCode::IoWriteFailed),
        other => CliError::fatal(format!("failed to {context}: {other}"))
            .with_stable_code(StableErrorCode::RepoCorrupt),
    }
}

async fn prune_remote(name: &str, dry_run: bool) -> Result<(), CliError> {
    // Check if the remote exists
    let Some(remote_config) = ConfigKv::remote_config(name).await.ok().flatten() else {
        return Err(CliError::fatal(format!("no such remote: {}", name)));
    };

    // Get remote client
    let remote_client = RemoteClient::from_spec_with_remote(&remote_config.url, Some(name))
        .map_err(|e| {
            CliError::fatal(format!(
                "Failed to create remote client from '{}': {}",
                remote_config.url, e
            ))
        })?;

    // Discover remote references
    let discovery = remote_client
        .discovery_reference(crate::git_protocol::ServiceType::UploadPack)
        .await
        .map_err(|e| {
            CliError::fatal(format!(
                "Failed to discover remote references for '{}' at '{}': {}",
                name, remote_config.url, e
            ))
        })?;

    // Verify hash kind compatibility
    let local_kind = get_hash_kind();
    if discovery.hash_kind != local_kind {
        return Err(CliError::fatal(format!(
            "remote object format '{}' does not match local '{}'",
            discovery.hash_kind, local_kind
        )));
    }

    set_wire_hash_kind(discovery.hash_kind);

    // Get remote branch names from discovery (format: refs/heads/branch_name)
    let remote_branch_names: HashSet<String> = discovery
        .refs
        .iter()
        .filter_map(|r| {
            r._ref
                .strip_prefix("refs/heads/")
                .map(String::from)
                .or_else(|| {
                    r._ref
                        .strip_prefix("refs/mr/")
                        .map(|mr| format!("mr/{}", mr))
                })
        })
        .collect();
    // Get local remote-tracking branches (format: "refs/remotes/{remote}/branch_name")
    let local_remote_branches = Branch::list_branches_result(Some(name))
        .await
        .map_err(|error| remote_branch_store_error("list remote-tracking branches", error))?;

    // Find and prune stale branches
    let mut pruned_count = 0;
    let head_ref = format!("refs/remotes/{}/HEAD", name);
    let prefix = format!("refs/remotes/{}/", name);

    for local_branch in &local_remote_branches {
        // Skip HEAD reference
        if local_branch.name == head_ref {
            continue;
        }
        // Extract branch name from "refs/remotes/{remote}/branch_name"
        let Some(branch_name) = local_branch.name.strip_prefix(&prefix) else {
            continue;
        };

        // Check if this branch still exists on remote
        if !remote_branch_names.contains(branch_name) {
            if dry_run {
                println!(" * [would prune] {}/{}", name, branch_name);
            } else {
                Branch::delete_branch_result(&local_branch.name, Some(name))
                    .await
                    .map_err(|error| {
                        remote_branch_store_error("pruning remote-tracking branch", error)
                    })?;
                println!(" * [pruned] {}/{}", name, branch_name);
            }
            pruned_count += 1;
        }
    }

    // Print summary
    match pruned_count {
        0 => println!("Everything up-to-date"),
        n if dry_run => println!("\nWould prune {} stale remote-tracking branch(es).", n),
        n => println!("\nPruned {} stale remote-tracking branch(es).", n),
    }
    Ok(())
}
