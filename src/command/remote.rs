//! Manages remotes by listing, showing, adding, and updating URLs and associated fetch/push metadata.

use std::collections::HashSet;

use clap::Subcommand;
use git_internal::hash::get_hash_kind;

use crate::{
    command::fetch::RemoteClient,
    internal::{branch::Branch, config::Config, protocol::set_wire_hash_kind},
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
    match command {
        RemoteCmds::Add { name, url } => {
            Config::insert("remote", Some(&name), "url", &url).await;
        }
        RemoteCmds::Remove { name } => {
            if let Err(e) = Config::remove_remote(&name).await {
                eprintln!("{e}");
            }
        }
        RemoteCmds::Rename { old, new } => {
            if let Err(e) = Config::rename_remote(&old, &new).await {
                eprintln!("{e}");
            }
        }
        RemoteCmds::List => {
            let remotes = Config::all_remote_configs().await;
            for remote in remotes {
                show_remote_verbose(&remote.name).await;
            }
        }
        RemoteCmds::Show => {
            let remotes = Config::all_remote_configs().await;
            for remote in remotes {
                println!("{}", remote.name);
            }
        }
        RemoteCmds::GetUrl { push, all, name } => {
            if Config::remote_config(&name).await.is_none() {
                eprintln!("fatal: No such remote: {name}");
                return;
            }
            // If --push, prefer explicit pushurl entries; fall back to url if none.
            if push {
                let push_urls = Config::get_all("remote", Some(&name), "pushurl").await;
                if !push_urls.is_empty() {
                    if all {
                        for u in push_urls {
                            println!("{}", u);
                        }
                    } else if let Some(u) = push_urls.first() {
                        println!("{}", u);
                    }
                    return;
                }
                // fall through to read regular url if no pushurl configured
            }

            let urls = Config::get_all("remote", Some(&name), "url").await;
            if urls.is_empty() {
                eprintln!("fatal: no URL configured for remote '{name}'");
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
            if Config::remote_config(&name).await.is_none() {
                eprintln!("fatal: No such remote: {name}");
                return;
            }
            // Determine which config key to operate on
            let key = if push { "pushurl" } else { "url" };

            if add {
                // Insert a new URL entry
                Config::insert("remote", Some(&name), key, &value).await;
                return;
            }

            if delete {
                // Delete matching entries; if --all then delete all matching, else delete first matching
                Config::remove_config("remote", Some(&name), key, Some(&value), all).await;
                return;
            }

            // Default: replace behavior
            if all {
                // Remove all existing entries for this key, then insert the new value once
                Config::remove_config("remote", Some(&name), key, None, true).await;
                Config::insert("remote", Some(&name), key, &value).await;
            } else {
                // Replace first existing entry: remove first occurrence then insert new value.
                // If no URL existed initially, the removal is a no-op and the insert will add the new URL.
                // This handles both the 'replace existing' and 'set new URL when none exists' cases.
                Config::remove_config("remote", Some(&name), key, None, false).await;
                Config::insert("remote", Some(&name), key, &value).await;
            }
        }
        RemoteCmds::Prune { name, dry_run } => {
            prune_remote(&name, dry_run).await;
        }
    }
}

async fn show_remote_verbose(remote: &str) {
    // There can be multiple URLs for a remote, like Gitee & GitHub
    let urls = Config::get_all("remote", Some(remote), "url").await;
    match urls.first() {
        Some(url) => {
            println!("{remote} {url} (fetch)");
        }
        None => {
            eprintln!("fatal: no URL configured for remote '{remote}'");
        }
    }
    for url in urls {
        println!("{remote} {url} (push)");
    }
}

async fn prune_remote(name: &str, dry_run: bool) {
    // Check if the remote exists
    let Some(remote_config) = Config::remote_config(name).await else {
        eprintln!("fatal: No such remote: {}", name);
        return;
    };

    // Get remote client
    let remote_client = match RemoteClient::from_spec(&remote_config.url) {
        Ok(client) => client,
        Err(e) => {
            eprintln!(
                "fatal: Failed to create remote client from '{}': {}",
                remote_config.url, e
            );
            return;
        }
    };

    // Discover remote references
    let discovery = match remote_client
        .discovery_reference(crate::git_protocol::ServiceType::UploadPack)
        .await
    {
        Ok(discovery) => discovery,
        Err(e) => {
            eprintln!(
                "fatal: Failed to discover remote references for '{}' at '{}': {}",
                name, remote_config.url, e
            );
            return;
        }
    };

    // Verify hash kind compatibility
    let local_kind = get_hash_kind();
    if discovery.hash_kind != local_kind {
        eprintln!(
            "fatal: remote object format '{}' does not match local '{}'",
            discovery.hash_kind, local_kind
        );
        return;
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
    let local_remote_branches = Branch::list_branches(Some(name)).await;

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
                Branch::delete_branch(&local_branch.name, Some(name)).await;
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
}
