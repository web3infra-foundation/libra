//! Manages remotes by listing, adding, removing, renaming, mutating URLs, and
//! pruning stale remote-tracking branches.

use std::{
    collections::HashSet,
    io::{self, Write},
};

use clap::Subcommand;
use git_internal::hash::get_hash_kind;
use serde::Serialize;

use crate::{
    command::fetch,
    internal::{
        branch::{Branch, BranchStoreError},
        config::ConfigKv,
        protocol::set_wire_hash_kind,
    },
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, emit_json_data},
    },
};

/// Whether a URL entry targets the fetch or push side of a remote.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum UrlRole {
    Fetch,
    Push,
}

impl std::fmt::Display for UrlRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UrlRole::Fetch => f.write_str("fetch"),
            UrlRole::Push => f.write_str("push"),
        }
    }
}

/// The mutation performed by `set-url`.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SetUrlMode {
    Add,
    Delete,
    Set,
}

impl std::fmt::Display for SetUrlMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SetUrlMode::Add => f.write_str("add"),
            SetUrlMode::Delete => f.write_str("delete"),
            SetUrlMode::Set => f.write_str("set"),
        }
    }
}

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
    /// List remotes verbosely
    #[command(name = "-v")]
    List,
    /// List configured remote names
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

#[derive(Debug, thiserror::Error)]
enum RemoteError {
    #[error("remote '{name}' already exists")]
    AlreadyExists { name: String },

    #[error("no such remote: {name}")]
    NotFound { name: String },

    #[error("no URL configured for remote '{name}'")]
    NoUrlConfigured { name: String },

    #[error("no matching {role} URL found for remote '{name}': {pattern}")]
    UrlPatternNotMatched {
        name: String,
        role: UrlRole,
        pattern: String,
    },

    #[error("failed to read remote configuration: {detail}")]
    ConfigRead { detail: String },

    #[error("failed to update remote configuration: {detail}")]
    ConfigWrite { detail: String },

    #[error("failed to list remote-tracking branches: {detail}")]
    BranchList { detail: String },

    #[error("corrupt remote-tracking branch '{name}': {detail}")]
    BranchCorrupt { name: String, detail: String },

    #[error("failed to prune remote-tracking branch '{name}': {detail}")]
    BranchDelete { name: String, detail: String },

    #[error("remote object format '{remote}' does not match local '{local}'")]
    ObjectFormatMismatch { remote: String, local: String },

    #[error(transparent)]
    Fetch(#[from] fetch::FetchError),
}

impl From<RemoteError> for CliError {
    fn from(error: RemoteError) -> Self {
        match error {
            RemoteError::AlreadyExists { name } => {
                CliError::fatal(format!("remote '{name}' already exists"))
                    .with_stable_code(StableErrorCode::ConflictOperationBlocked)
                    .with_hint("use 'libra remote -v' to inspect configured remotes")
            }
            RemoteError::NotFound { name } => CliError::fatal(format!("no such remote: {name}"))
                .with_stable_code(StableErrorCode::CliInvalidTarget)
                .with_hint("use 'libra remote -v' to inspect configured remotes"),
            RemoteError::NoUrlConfigured { name } => {
                CliError::fatal(format!("no URL configured for remote '{name}'"))
                    .with_stable_code(StableErrorCode::CliInvalidTarget)
                    .with_hint("use 'libra remote get-url --all <name>' to inspect configured URLs")
            }
            RemoteError::UrlPatternNotMatched {
                name,
                role,
                pattern,
            } => CliError::fatal(format!(
                "no matching {role} URL found for remote '{name}': {pattern}"
            ))
            .with_stable_code(StableErrorCode::CliInvalidTarget)
            .with_hint("use 'libra remote get-url --all <name>' to inspect configured URLs"),
            RemoteError::ConfigRead { detail } => {
                CliError::fatal(format!("failed to read remote configuration: {detail}"))
                    .with_stable_code(StableErrorCode::IoReadFailed)
            }
            RemoteError::BranchList { detail } => {
                CliError::fatal(format!("failed to list remote-tracking branches: {detail}"))
                    .with_stable_code(StableErrorCode::IoReadFailed)
            }
            RemoteError::BranchCorrupt { name, detail } => {
                CliError::fatal(format!("corrupt remote-tracking branch '{name}': {detail}"))
                    .with_stable_code(StableErrorCode::RepoCorrupt)
            }
            RemoteError::ConfigWrite { detail } => {
                CliError::fatal(format!("failed to update remote configuration: {detail}"))
                    .with_stable_code(StableErrorCode::IoWriteFailed)
            }
            RemoteError::BranchDelete { name, detail } => CliError::fatal(format!(
                "failed to prune remote-tracking branch '{name}': {detail}"
            ))
            .with_stable_code(StableErrorCode::IoWriteFailed),
            RemoteError::ObjectFormatMismatch { remote, local } => CliError::fatal(format!(
                "remote object format '{remote}' does not match local '{local}'"
            ))
            .with_stable_code(StableErrorCode::RepoStateInvalid),
            RemoteError::Fetch(source) => CliError::from(source),
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RemoteListEntry {
    pub name: String,
    pub fetch_urls: Vec<String>,
    pub push_urls: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RemotePruneEntry {
    pub remote_ref: String,
    pub branch: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum RemoteOutput {
    Add {
        name: String,
        url: String,
    },
    Remove {
        name: String,
    },
    Rename {
        old_name: String,
        new_name: String,
    },
    List {
        verbose: bool,
        remotes: Vec<RemoteListEntry>,
    },
    Urls {
        name: String,
        push: bool,
        all: bool,
        urls: Vec<String>,
    },
    SetUrl {
        name: String,
        role: UrlRole,
        mode: SetUrlMode,
        urls: Vec<String>,
        removed: usize,
    },
    Prune {
        name: String,
        dry_run: bool,
        stale_branches: Vec<RemotePruneEntry>,
    },
}

pub async fn execute(command: RemoteCmds) {
    if let Err(error) = execute_safe(command, &OutputConfig::default()).await {
        error.print_stderr();
    }
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting.
pub async fn execute_safe(command: RemoteCmds, output: &OutputConfig) -> CliResult<()> {
    let result = run_remote(command).await.map_err(CliError::from)?;
    render_remote_output(&result, output)
}

async fn run_remote(command: RemoteCmds) -> Result<RemoteOutput, RemoteError> {
    match command {
        RemoteCmds::Add { name, url } => run_add_remote(name, url).await,
        RemoteCmds::Remove { name } => run_remove_remote(name).await,
        RemoteCmds::Rename { old, new } => run_rename_remote(old, new).await,
        RemoteCmds::List => run_list_remotes(true).await,
        RemoteCmds::Show => run_list_remotes(false).await,
        RemoteCmds::GetUrl { push, all, name } => run_get_url(name, push, all).await,
        RemoteCmds::SetUrl {
            add,
            delete,
            push,
            all,
            name,
            value,
        } => run_set_url(name, value, push, add, delete, all).await,
        RemoteCmds::Prune { name, dry_run } => run_prune_remote(name, dry_run).await,
    }
}

async fn run_add_remote(name: String, url: String) -> Result<RemoteOutput, RemoteError> {
    if remote_exists(&name).await? {
        return Err(RemoteError::AlreadyExists { name });
    }

    ConfigKv::set(&format!("remote.{name}.url"), &url, false)
        .await
        .map_err(|error| RemoteError::ConfigWrite {
            detail: error.to_string(),
        })?;

    Ok(RemoteOutput::Add { name, url })
}

async fn run_remove_remote(name: String) -> Result<RemoteOutput, RemoteError> {
    ensure_remote_exists(&name).await?;
    ConfigKv::remove_remote(&name)
        .await
        .map_err(|error| RemoteError::ConfigWrite {
            detail: error.to_string(),
        })?;
    Ok(RemoteOutput::Remove { name })
}

async fn run_rename_remote(old: String, new: String) -> Result<RemoteOutput, RemoteError> {
    ensure_remote_exists(&old).await?;
    if remote_exists(&new).await? {
        return Err(RemoteError::AlreadyExists { name: new });
    }

    ConfigKv::rename_remote(&old, &new)
        .await
        .map_err(|error| RemoteError::ConfigWrite {
            detail: error.to_string(),
        })?;
    Ok(RemoteOutput::Rename {
        old_name: old,
        new_name: new,
    })
}

async fn run_list_remotes(verbose: bool) -> Result<RemoteOutput, RemoteError> {
    let remote_names = list_remote_names().await?;

    let mut entries = Vec::with_capacity(remote_names.len());
    for name in remote_names {
        entries.push(load_remote_entry(&name).await?);
    }

    Ok(RemoteOutput::List {
        verbose,
        remotes: entries,
    })
}

/// Discover all remote names by scanning `remote.<name>.*` config keys.
/// Unlike `ConfigKv::all_remote_configs()` (which only recognises remotes with
/// a `.url` entry), this finds any remote that has *any* configuration key.
async fn list_remote_names() -> Result<Vec<String>, RemoteError> {
    let entries =
        ConfigKv::get_by_prefix("remote.")
            .await
            .map_err(|error| RemoteError::ConfigRead {
                detail: error.to_string(),
            })?;
    let mut names = HashSet::new();
    for entry in entries {
        // key format: "remote.<name>.<subkey>" — use `rsplit_once` so that
        // dotted remote names (e.g. "remote.corp.prod.url") are parsed as
        // name="corp.prod", matching `ConfigKv::all_remote_configs`.
        if let Some(rest) = entry.key.strip_prefix("remote.")
            && let Some((name, _subkey)) = rest.rsplit_once('.')
            && !name.is_empty()
        {
            names.insert(name.to_owned());
        }
    }
    let mut names: Vec<String> = names.into_iter().collect();
    names.sort();
    Ok(names)
}

async fn run_get_url(name: String, push: bool, all: bool) -> Result<RemoteOutput, RemoteError> {
    ensure_remote_exists(&name).await?;
    let fetch_urls = load_config_urls(&name, "url").await?;
    let configured_push_urls = load_config_urls(&name, "pushurl").await?;
    let push_urls = effective_push_urls(&fetch_urls, &configured_push_urls);

    let source = if push { &push_urls } else { &fetch_urls };
    let urls: Vec<String> = if all {
        source.clone()
    } else {
        source.iter().take(1).cloned().collect()
    };

    if urls.is_empty() {
        return Err(RemoteError::NoUrlConfigured { name });
    }

    Ok(RemoteOutput::Urls {
        name,
        push,
        all,
        urls,
    })
}

async fn run_set_url(
    name: String,
    value: String,
    push: bool,
    add: bool,
    delete: bool,
    // `--all` and default replace both perform unset-all-then-set, so the
    // behavior is identical today.  We accept the flag for CLI compatibility
    // with Git but do not branch on it.
    #[allow(unused_variables)] all: bool,
) -> Result<RemoteOutput, RemoteError> {
    ensure_remote_exists(&name).await?;

    let key = if push { "pushurl" } else { "url" };
    let role = if push { UrlRole::Push } else { UrlRole::Fetch };
    let full_key = format!("remote.{name}.{key}");

    let mode = if add {
        ConfigKv::add(&full_key, &value, false)
            .await
            .map_err(|error| RemoteError::ConfigWrite {
                detail: error.to_string(),
            })?;
        SetUrlMode::Add
    } else if delete {
        let entries =
            ConfigKv::get_all(&full_key)
                .await
                .map_err(|error| RemoteError::ConfigRead {
                    detail: error.to_string(),
                })?;
        let removed = entries
            .iter()
            .filter(|entry| entry.value.contains(&value))
            .count();
        if removed == 0 {
            return Err(RemoteError::UrlPatternNotMatched {
                name,
                role,
                pattern: value,
            });
        }

        ConfigKv::unset_all(&full_key)
            .await
            .map_err(|error| RemoteError::ConfigWrite {
                detail: error.to_string(),
            })?;
        for entry in entries
            .into_iter()
            .filter(|entry| !entry.value.contains(&value))
        {
            ConfigKv::add(&full_key, &entry.value, entry.encrypted)
                .await
                .map_err(|error| RemoteError::ConfigWrite {
                    detail: error.to_string(),
                })?;
        }

        let urls = load_config_urls(&name, key).await?;
        return Ok(RemoteOutput::SetUrl {
            name,
            role,
            mode: SetUrlMode::Delete,
            urls,
            removed,
        });
    } else {
        ConfigKv::unset_all(&full_key)
            .await
            .map_err(|error| RemoteError::ConfigWrite {
                detail: error.to_string(),
            })?;
        ConfigKv::set(&full_key, &value, false)
            .await
            .map_err(|error| RemoteError::ConfigWrite {
                detail: error.to_string(),
            })?;
        SetUrlMode::Set
    };

    let urls = load_config_urls(&name, key).await?;
    Ok(RemoteOutput::SetUrl {
        name,
        role,
        mode,
        urls,
        removed: 0,
    })
}

async fn run_prune_remote(name: String, dry_run: bool) -> Result<RemoteOutput, RemoteError> {
    ensure_remote_exists(&name).await?;
    let remote_config = ConfigKv::remote_config(&name)
        .await
        .map_err(|error| RemoteError::ConfigRead {
            detail: error.to_string(),
        })?
        .ok_or_else(|| RemoteError::NoUrlConfigured { name: name.clone() })?;

    let (_remote_client, discovery) =
        fetch::discover_remote_with_name(&remote_config.url, Some(&remote_config.name)).await?;

    let local_kind = get_hash_kind();
    if discovery.hash_kind != local_kind {
        return Err(RemoteError::ObjectFormatMismatch {
            remote: discovery.hash_kind.to_string(),
            local: local_kind.to_string(),
        });
    }

    set_wire_hash_kind(discovery.hash_kind);

    let remote_branch_names: HashSet<String> = discovery
        .refs
        .iter()
        .filter_map(|reference| {
            reference
                ._ref
                .strip_prefix("refs/heads/")
                .map(String::from)
                .or_else(|| {
                    reference
                        ._ref
                        .strip_prefix("refs/mr/")
                        .map(|mr| format!("mr/{mr}"))
                })
        })
        .collect();

    let local_remote_branches =
        Branch::list_branches_result(Some(&name))
            .await
            .map_err(|error| match error {
                BranchStoreError::Query(detail) => RemoteError::BranchList { detail },
                BranchStoreError::Corrupt { name, detail } => {
                    RemoteError::BranchCorrupt { name, detail }
                }
                other => RemoteError::BranchList {
                    detail: other.to_string(),
                },
            })?;

    let head_ref = format!("refs/remotes/{name}/HEAD");
    let prefix = format!("refs/remotes/{name}/");
    let mut stale_branches = Vec::new();

    for local_branch in &local_remote_branches {
        if local_branch.name == head_ref {
            continue;
        }

        let Some(branch_name) = local_branch.name.strip_prefix(&prefix) else {
            continue;
        };

        if remote_branch_names.contains(branch_name) {
            continue;
        }

        if !dry_run {
            Branch::delete_branch_result(&local_branch.name, Some(&name))
                .await
                .map_err(|error| match error {
                    BranchStoreError::Delete { name, detail } => {
                        RemoteError::BranchDelete { name, detail }
                    }
                    BranchStoreError::Corrupt { name, detail } => {
                        RemoteError::BranchCorrupt { name, detail }
                    }
                    BranchStoreError::Query(detail) => RemoteError::BranchList { detail },
                    other => RemoteError::ConfigWrite {
                        detail: other.to_string(),
                    },
                })?;
        }

        stale_branches.push(RemotePruneEntry {
            remote_ref: local_branch.name.clone(),
            branch: format!("{name}/{branch_name}"),
        });
    }

    Ok(RemoteOutput::Prune {
        name,
        dry_run,
        stale_branches,
    })
}

/// A remote is considered to exist if **any** `remote.<name>.*` key is
/// present, not only `remote.<name>.url`.  This handles the edge case where
/// `set-url --delete` removed the last fetch URL but other keys (e.g.
/// `pushurl`, vault SSH keys) still remain.
///
/// Uses `rsplit_once('.')` name extraction to avoid prefix collisions with
/// dotted remote names (e.g. querying "corp" must not match a key belonging
/// to remote "corp.prod").
async fn remote_exists(name: &str) -> Result<bool, RemoteError> {
    let prefix = format!("remote.{name}.");
    let entries =
        ConfigKv::get_by_prefix(&prefix)
            .await
            .map_err(|error| RemoteError::ConfigRead {
                detail: error.to_string(),
            })?;
    // Verify that at least one entry actually parses as belonging to this
    // exact remote name, not a longer dotted name that shares the prefix.
    Ok(entries.iter().any(|e| {
        e.key
            .strip_prefix("remote.")
            .and_then(|rest| rest.rsplit_once('.'))
            .is_some_and(|(parsed_name, _)| parsed_name == name)
    }))
}

async fn ensure_remote_exists(name: &str) -> Result<(), RemoteError> {
    if remote_exists(name).await? {
        Ok(())
    } else {
        Err(RemoteError::NotFound {
            name: name.to_string(),
        })
    }
}

/// Load a remote's URL configuration.  Tolerates missing fetch URLs so that
/// remotes that only have `pushurl` (e.g. after `set-url --delete` removed the
/// last fetch URL) are still visible in listings and accessible to `get-url
/// --push`.
async fn load_remote_entry(name: &str) -> Result<RemoteListEntry, RemoteError> {
    ensure_remote_exists(name).await?;
    let fetch_urls = load_config_urls(name, "url").await?;
    let configured_push_urls = load_config_urls(name, "pushurl").await?;
    let push_urls = effective_push_urls(&fetch_urls, &configured_push_urls);

    Ok(RemoteListEntry {
        name: name.to_string(),
        fetch_urls,
        push_urls,
    })
}

async fn load_config_urls(name: &str, key: &str) -> Result<Vec<String>, RemoteError> {
    ConfigKv::get_all(&format!("remote.{name}.{key}"))
        .await
        .map_err(|error| RemoteError::ConfigRead {
            detail: error.to_string(),
        })
        .map(|entries| entries.into_iter().map(|entry| entry.value).collect())
}

fn effective_push_urls(fetch_urls: &[String], push_urls: &[String]) -> Vec<String> {
    if push_urls.is_empty() {
        fetch_urls.to_vec()
    } else {
        push_urls.to_vec()
    }
}

fn render_remote_output(result: &RemoteOutput, output: &OutputConfig) -> CliResult<()> {
    if output.is_json() {
        return emit_json_data("remote", result, output);
    }

    if output.quiet {
        return Ok(());
    }

    let stdout = io::stdout();
    let mut writer = stdout.lock();
    let write_err =
        |error: io::Error| CliError::io(format!("failed to write remote output: {error}"));

    match result {
        RemoteOutput::Add { name, url } => writeln!(
            writer,
            "Added remote '{name}' -> {}",
            fetch::redact_url_credentials(url)
        )
        .map_err(write_err),
        RemoteOutput::Remove { name } => {
            writeln!(writer, "Removed remote '{name}'").map_err(write_err)
        }
        RemoteOutput::Rename { old_name, new_name } => {
            writeln!(writer, "Renamed remote '{old_name}' to '{new_name}'").map_err(write_err)
        }
        RemoteOutput::List { verbose, remotes } => {
            if *verbose {
                for remote in remotes {
                    for url in &remote.fetch_urls {
                        writeln!(
                            writer,
                            "{}\t{} (fetch)",
                            remote.name,
                            fetch::redact_url_credentials(url)
                        )
                        .map_err(write_err)?;
                    }
                    for url in &remote.push_urls {
                        writeln!(
                            writer,
                            "{}\t{} (push)",
                            remote.name,
                            fetch::redact_url_credentials(url)
                        )
                        .map_err(write_err)?;
                    }
                }
            } else {
                for remote in remotes {
                    writeln!(writer, "{}", remote.name).map_err(write_err)?;
                }
            }
            Ok(())
        }
        RemoteOutput::Urls { urls, .. } => {
            for url in urls {
                writeln!(writer, "{}", fetch::redact_url_credentials(url)).map_err(write_err)?;
            }
            Ok(())
        }
        RemoteOutput::SetUrl {
            name,
            role,
            mode,
            urls,
            removed,
        } => match mode {
            SetUrlMode::Add => writeln!(
                writer,
                "Added {role} URL for remote '{name}': {}",
                fetch::redact_url_credentials(&urls.last().cloned().unwrap_or_default())
            )
            .map_err(write_err),
            SetUrlMode::Delete => writeln!(
                writer,
                "Removed {removed} {role} URL(s) from remote '{name}'"
            )
            .map_err(write_err),
            SetUrlMode::Set => writeln!(
                writer,
                "Set {role} URL for remote '{name}' to {}",
                fetch::redact_url_credentials(&urls.first().cloned().unwrap_or_default())
            )
            .map_err(write_err),
        },
        RemoteOutput::Prune {
            name: _,
            dry_run,
            stale_branches,
        } => {
            for entry in stale_branches {
                if *dry_run {
                    writeln!(writer, " * [would prune] {}", entry.branch).map_err(write_err)?;
                } else {
                    writeln!(writer, " * [pruned] {}", entry.branch).map_err(write_err)?;
                }
            }

            if stale_branches.is_empty() {
                writeln!(writer, "Everything up-to-date").map_err(write_err)?;
            } else if *dry_run {
                writeln!(
                    writer,
                    "\nWould prune {} stale remote-tracking branch(es).",
                    stale_branches.len()
                )
                .map_err(write_err)?;
            } else {
                writeln!(
                    writer,
                    "\nPruned {} stale remote-tracking branch(es).",
                    stale_branches.len()
                )
                .map_err(write_err)?;
            }
            Ok(())
        }
    }
}
