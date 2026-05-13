//! Supports cloning repositories by parsing URLs, fetching objects via protocol
//! clients, checking out the working tree, and writing initial refs/config.
//!
//! The execution layer (`execute_clone`) produces a structured [`CloneOutput`]
//! and the rendering layer (`execute_safe`) converts it to human / JSON /
//! machine output according to the global [`OutputConfig`].

use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

use clap::Parser;
use git_internal::{
    errors::GitError,
    hash::{ObjectHash, get_hash_kind},
};
use sea_orm::DatabaseTransaction;
use serde::Serialize;
use url::Url;

use super::fetch::{self, RemoteSpecErrorKind};
use crate::{
    command::{
        self,
        init::InitError,
        restore::{RestoreArgs, RestoreError},
    },
    internal::{
        branch::{self, Branch},
        config::{
            ConfigKv, LocalIdentityTarget, RemoteConfig, read_cascaded_config_value_decrypted,
            resolve_env_for_target,
        },
        db::get_db_conn_instance,
        head::Head,
        protocol::DiscoveryResult,
        reflog::{ReflogAction, ReflogContext, with_reflog},
    },
    utils::{
        d1_client::{D1Client, PublishSiteRow},
        error::{CliError, CliResult, StableErrorCode},
        ignore as ignore_utils,
        output::{OutputConfig, emit_json_data},
        util,
    },
};

const ISSUE_URL: &str = "https://github.com/web3infra-foundation/libra/issues";

/// Clone a repository into a new directory.
///
/// # Examples
///
/// ```text
/// libra clone git@github.com:user/repo.git             Clone via SSH
/// libra clone https://github.com/user/repo.git          Clone via HTTPS
/// libra clone git@github.com:user/repo.git my-dir       Clone to specific directory
/// libra clone --bare git@github.com:user/repo.git       Create bare clone
/// libra clone -b develop git@github.com:user/repo.git   Clone specific branch
/// libra clone --single-branch -b main <url>             Clone only one branch
/// libra clone --depth 1 <url>                           Shallow clone (latest commit only)
/// ```
#[derive(Parser, Debug, Clone)]
#[clap(after_help = "EXAMPLES:\n    \
    libra clone git@github.com:user/repo.git             Clone via SSH\n    \
    libra clone https://github.com/user/repo.git          Clone via HTTPS\n    \
    libra clone git@github.com:user/repo.git my-dir       Clone to specific directory\n    \
    libra clone --bare git@github.com:user/repo.git       Create bare clone\n    \
    libra clone -b develop git@github.com:user/repo.git   Clone specific branch\n    \
    libra clone --single-branch -b main <url>             Clone only one branch\n    \
    libra clone --depth 1 <url>                           Shallow clone (latest commit only)")]
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

// ---------------------------------------------------------------------------
// CloneOutput — structured result of a successful clone
// ---------------------------------------------------------------------------

/// Structured output for a successful clone operation. Rendered as JSON
/// envelope by `emit_json_data("clone", &output, config)` in `--json` mode,
/// or as a human-readable summary otherwise.
#[derive(Debug, Clone, Serialize)]
pub struct CloneOutput {
    /// Repository absolute path (worktree root for non-bare, `.libra` dir for bare).
    pub path: String,
    pub bare: bool,
    /// Normalized remote URL.
    pub remote_url: String,
    /// Actual checked-out branch; `None` for empty remotes.
    pub branch: Option<String>,
    /// `sha1` or `sha256` (from `InitOutput.object_format`).
    pub object_format: String,
    /// From `InitOutput.repo_id`.
    pub repo_id: String,
    /// From `InitOutput.vault_signing`.
    pub vault_signing: bool,
    /// From `InitOutput.ssh_key_detected`.
    pub ssh_key_detected: Option<String>,
    /// Whether `--depth` produced a shallow clone.
    pub shallow: bool,
    /// Non-fatal warnings (empty remote, init warnings, etc.).
    pub warnings: Vec<String>,
    /// Worktree-relative paths of `.libraignore` files written by converting
    /// `.gitignore` files from the source repository.  Empty for bare clones.
    pub gitignore_converted: Vec<String>,
}

// ---------------------------------------------------------------------------
// CloneError
// ---------------------------------------------------------------------------

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
    #[error("remote discovery failed")]
    DiscoverRemote { source: fetch::FetchError },
    #[error("failed to change working directory to '{path}': {source}")]
    ChangeDirectory { path: PathBuf, source: io::Error },
    #[error("failed to restore working directory to '{path}': {source}")]
    RestoreDirectory { path: PathBuf, source: io::Error },
    #[error("failed to initialize repository")]
    InitializeRepository { source: InitError },
    #[error("remote branch {branch} not found in upstream origin")]
    RemoteBranchNotFound { branch: String },
    #[error("failed to inspect local branch state after fetch: {source}")]
    LocalBranchState { source: branch::BranchStoreError },
    #[error("fetch failed: {source}")]
    FetchFailed { source: fetch::FetchError },
    #[error("failed to checkout working tree")]
    CheckoutFailed { source: RestoreError },
    #[error("failed to convert ignore files")]
    IgnoreFile {
        source: ignore_utils::IgnoreFileError,
    },
    #[error("failed to complete clone setup: {message}")]
    SetupFailed { message: String },
    #[error("clone domain '{domain}' is not configured for libra+cloud restore")]
    CloudCloneDomainNotConfigured {
        domain: String,
        missing_keys: String,
    },
    #[error("D1 API token is not configured for clone domain '{domain}'")]
    CloudCloneD1ApiTokenNotConfigured { domain: String },
    #[error("failed to read clone domain '{domain}' configuration: {source}")]
    CloudCloneDomainConfigRead {
        domain: String,
        #[source]
        source: anyhow::Error,
    },
    #[error("{option} is not supported with libra+cloud:// clone sources: {reason}")]
    UnsupportedCloudCloneOption {
        option: &'static str,
        reason: &'static str,
        hint: &'static str,
    },
    #[error(
        "libra+cloud:// clone source recognised, but Phase 5 of \
         docs/improvement/publish.md is not yet implemented; tracking \
         the v1 release window for `libra clone {input}`"
    )]
    CloudPublishSourceNotYetImplemented {
        input: String,
        target: String,
        selector: Option<String>,
        config_details: Vec<(&'static str, String)>,
    },
    #[error(
        "failed to resolve libra+cloud site {target} in clone domain '{domain}' \
         via D1 (code {code}): {message}"
    )]
    CloudPublishSiteLookupFailed {
        domain: String,
        target: String,
        code: i32,
        message: String,
    },
    #[error("libra+cloud site {target} was not found in clone domain '{domain}'")]
    CloudPublishSiteNotFound { domain: String, target: String },
    #[error("libra+cloud site {target} in clone domain '{domain}' is not active: {status}")]
    CloudPublishSiteUnavailable {
        domain: String,
        target: String,
        status: String,
    },
    #[error("invalid libra+cloud clone source '{input}': {reason}")]
    InvalidCloudPublishSource { input: String, reason: String },
}

// ---------------------------------------------------------------------------
// CloneError → CliError — explicit StableErrorCode mapping
// ---------------------------------------------------------------------------

impl From<CloneError> for CliError {
    fn from(error: CloneError) -> Self {
        match error {
            CloneError::CannotInferDestination => {
                CliError::command_usage("please specify the destination path explicitly")
                    .with_stable_code(StableErrorCode::CliInvalidArguments)
                    .with_hint("please specify the destination path explicitly")
            }
            CloneError::DestinationExistsNonEmpty { ref path } => CliError::command_usage(format!(
                "destination path '{}' already exists and is not an empty directory",
                path.display()
            ))
            .with_stable_code(StableErrorCode::CliInvalidTarget)
            .with_hint("choose a different path or empty the directory first"),
            CloneError::DestinationAlreadyRepo { ref path } => CliError::fatal(format!(
                "destination path '{}' already contains a libra repository",
                path.display()
            ))
            .with_stable_code(StableErrorCode::RepoStateInvalid)
            .with_hint("the destination already contains a libra repository"),
            CloneError::CreateDestinationFailed { .. } => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::IoWriteFailed)
                .with_hint("check directory permissions and disk space"),
            CloneError::DiscoverRemote { source } => map_discover_remote_error(source),
            CloneError::ChangeDirectory { .. } | CloneError::RestoreDirectory { .. } => {
                CliError::fatal(error.to_string())
                    .with_stable_code(StableErrorCode::InternalInvariant)
                    .with_hint(format!("please report this issue at: {ISSUE_URL}"))
            }
            CloneError::InitializeRepository { source } => {
                // Transparently reuse init's complete error mapping.
                source.into()
            }
            CloneError::RemoteBranchNotFound { ref branch } => CliError::fatal(format!(
                "remote branch '{branch}' not found in upstream origin"
            ))
            .with_stable_code(StableErrorCode::RepoStateInvalid)
            .with_hint(
                "use `-b <branch>` to specify an existing branch, or omit to use remote HEAD",
            ),
            CloneError::LocalBranchState { source } => map_local_branch_state_error(source)
                .with_hint("run 'libra status' to verify the local repository state"),
            CloneError::FetchFailed { source } => map_fetch_error(source),
            CloneError::CheckoutFailed { source } => map_checkout_error(source),
            CloneError::IgnoreFile { source } => {
                let stable_code = if source.is_write() {
                    StableErrorCode::IoWriteFailed
                } else {
                    StableErrorCode::IoReadFailed
                };
                CliError::fatal(source.to_string())
                    .with_stable_code(stable_code)
                    .with_hint(source.recovery_hint())
            }
            CloneError::SetupFailed { .. } => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::InternalInvariant)
                .with_hint(format!("please report this issue at: {ISSUE_URL}")),
            CloneError::CloudCloneDomainNotConfigured {
                ref domain,
                ref missing_keys,
            } => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::AuthMissingCredentials)
                .with_detail("clone_domain", domain.clone())
                .with_detail("missing_keys", missing_keys.clone())
                .with_hint(format!(
                    "configure cloud.clone_domains.{domain}.account_id, \
                     cloud.clone_domains.{domain}.d1_database_id, and \
                     cloud.clone_domains.{domain}.r2_bucket, and set LIBRA_D1_API_TOKEN \
                     before cloning this source."
                )),
            CloneError::CloudCloneD1ApiTokenNotConfigured { ref domain } => {
                CliError::fatal(error.to_string())
                    .with_stable_code(StableErrorCode::AuthMissingCredentials)
                    .with_detail("clone_domain", domain.clone())
                    .with_detail("missing_keys", "LIBRA_D1_API_TOKEN (env or vault)")
                    .with_hint(
                        "set LIBRA_D1_API_TOKEN in the environment or Libra vault config so \
                         the CLI can query the configured D1 database.",
                    )
            }
            CloneError::CloudCloneDomainConfigRead { ref domain, .. } => {
                CliError::fatal(error.to_string())
                    .with_stable_code(StableErrorCode::IoReadFailed)
                    .with_detail("clone_domain", domain.clone())
                    .with_hint("check the local/global Libra config database and vault state.")
            }
            CloneError::UnsupportedCloudCloneOption {
                ref option,
                ref hint,
                ..
            } => CliError::command_usage(error.to_string())
                .with_stable_code(StableErrorCode::CliInvalidArguments)
                .with_detail("option", option.to_string())
                .with_hint(hint.to_string()),
            CloneError::CloudPublishSourceNotYetImplemented {
                ref target,
                ref selector,
                ref config_details,
                ..
            } => {
                // Codex pass-7 P1: surface the recognised but
                // unimplemented Cloudflare publish clone source as a
                // fatal CLI error so the user sees a precise message
                // instead of falling through to the generic remote
                // discovery path. Phase 5 of
                // docs/improvement/publish.md lands the actual
                // implementation; until then this acts as a clean
                // surface for forward compatibility.
                let mut cli = CliError::fatal(error.to_string())
                    .with_stable_code(StableErrorCode::CliInvalidArguments)
                    .with_detail("cloud_target", target.clone())
                    .with_hint(
                        "Phase 5 of docs/improvement/publish.md is in progress; \
                         use the local CLI's existing `libra cloud restore` flow \
                         (or wait for the v1 release) to recover the repository.",
                    );
                if let Some(selector) = selector {
                    cli = cli.with_detail("cloud_selector", selector.clone());
                }
                for (key, value) in config_details {
                    cli = cli.with_detail(*key, value.clone());
                }
                cli
            }
            CloneError::CloudPublishSiteLookupFailed {
                ref domain,
                ref target,
                code,
                ref message,
            } => {
                let stable_code = if matches!(code, 401 | 403 | 1002) {
                    StableErrorCode::AuthPermissionDenied
                } else {
                    StableErrorCode::NetworkUnavailable
                };
                CliError::fatal(error.to_string())
                    .with_stable_code(stable_code)
                    .with_detail("clone_domain", domain.clone())
                    .with_detail("cloud_target", target.clone())
                    .with_detail("d1_error_code", code.to_string())
                    .with_detail("d1_error_message", message.clone())
                    .with_hint(
                        "check the D1 database id, API token, account id, and network access.",
                    )
            }
            CloneError::CloudPublishSiteNotFound {
                ref domain,
                ref target,
            } => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::RepoNotFound)
                .with_detail("clone_domain", domain.clone())
                .with_detail("cloud_target", target.clone())
                .with_hint(
                    "check the clone domain, slug/repo id, or publish the site before cloning.",
                ),
            CloneError::CloudPublishSiteUnavailable {
                ref domain,
                ref target,
                ref status,
            } => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::RepoStateInvalid)
                .with_detail("clone_domain", domain.clone())
                .with_detail("cloud_target", target.clone())
                .with_detail("cloud_site_status", status.clone())
                .with_hint("enable the publish site before cloning it from libra+cloud://."),
            CloneError::InvalidCloudPublishSource { .. } => {
                CliError::command_usage(error.to_string())
                    .with_stable_code(StableErrorCode::CliInvalidArguments)
                    .with_hint(
                        "use `libra+cloud://<clone-domain>/<slug>` or \
                         `libra+cloud://<clone-domain>/repo/<repo_id>`; pass only one of \
                         `?ref=<branch|tag|full-ref>` or `?revision=<oid|latest>`",
                    )
            }
        }
    }
}

/// Map a `FetchError` from the discovery phase into a `CliError`.
fn map_discover_remote_error(source: fetch::FetchError) -> CliError {
    match &source {
        fetch::FetchError::InvalidRemoteSpec { kind, .. } => match kind {
            RemoteSpecErrorKind::MissingLocalRepo | RemoteSpecErrorKind::InvalidLocalRepo => {
                CliError::fatal(source.to_string())
                    .with_stable_code(StableErrorCode::RepoNotFound)
                    .with_hint("use a valid libra repository path or a reachable remote URL")
            }
            RemoteSpecErrorKind::MalformedUrl | RemoteSpecErrorKind::UnsupportedScheme => {
                CliError::command_usage(source.to_string())
                    .with_stable_code(StableErrorCode::CliInvalidTarget)
                    .with_hint(
                        "check the clone URL or scheme, for example `https://`, `ssh`, or a local path",
                    )
            }
        },
        fetch::FetchError::Discovery {
            source: git_error, ..
        } => match git_error {
            GitError::UnAuthorized(_) => {
                CliError::fatal(format!("remote discovery failed: {source}"))
                    .with_stable_code(StableErrorCode::AuthPermissionDenied)
                    .with_hint("check SSH key / HTTP credentials and repository access rights")
            }
            GitError::NetworkError(_) => {
                CliError::fatal(format!("remote discovery failed: {source}"))
                    .with_stable_code(StableErrorCode::NetworkUnavailable)
                    .with_hint(
                        "check the remote host, DNS, VPN/proxy, and network connectivity",
                    )
            }
            GitError::IOError(_) => CliError::fatal(format!("remote discovery failed: {source}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
                .with_hint("check filesystem permissions and repository integrity"),
            _ => CliError::fatal(format!("remote discovery failed: {source}"))
                .with_stable_code(StableErrorCode::NetworkProtocol)
                .with_hint(
                    "the remote did not complete discovery successfully; retry and inspect server/protocol settings",
                ),
        },
        _ => CliError::fatal(format!("remote discovery failed: {source}"))
            .with_stable_code(StableErrorCode::NetworkProtocol)
            .with_hint(
                "the remote did not complete discovery successfully; retry and inspect server/protocol settings",
            ),
    }
}

/// Map a `FetchError` from the fetch phase into a `CliError`.
fn map_fetch_error(source: fetch::FetchError) -> CliError {
    match &source {
        fetch::FetchError::ObjectFormatMismatch { .. } => CliError::fatal(source.to_string())
            .with_stable_code(StableErrorCode::RepoStateInvalid)
            .with_hint("the remote and local repository use different object formats"),
        fetch::FetchError::FetchObjects { .. } | fetch::FetchError::PacketRead { .. } => {
            CliError::fatal(source.to_string())
                .with_stable_code(StableErrorCode::NetworkUnavailable)
                .with_hint("network error during transfer; check connectivity and retry")
        }
        fetch::FetchError::RemoteSideband { .. } | fetch::FetchError::ChecksumMismatch => {
            CliError::fatal(source.to_string())
                .with_stable_code(StableErrorCode::NetworkProtocol)
                .with_hint("the remote transfer failed or returned corrupted data; retry the clone")
        }
        fetch::FetchError::RemoteBranchNotFound { .. } => CliError::fatal(source.to_string())
            .with_stable_code(StableErrorCode::RepoStateInvalid)
            .with_hint("the specified branch does not exist on the remote"),
        _ => CliError::fatal(source.to_string())
            .with_stable_code(StableErrorCode::NetworkUnavailable)
            .with_hint("network error during transfer; check connectivity and retry"),
    }
}

/// Map a `RestoreError` from the checkout phase into a `CliError`.
fn map_checkout_error(source: RestoreError) -> CliError {
    match source {
        RestoreError::ResolveSource | RestoreError::ReferenceNotCommit => {
            CliError::fatal("working tree checkout target could not be resolved")
                .with_stable_code(StableErrorCode::RepoStateInvalid)
                .with_hint("working tree checkout target could not be resolved")
        }
        RestoreError::PathspecNotMatched(_) => {
            CliError::fatal("working tree checkout referenced a path that was not present")
                .with_stable_code(StableErrorCode::RepoCorrupt)
                .with_hint(
                    "the fetched tree is inconsistent; retry the clone or inspect the remote",
                )
        }
        RestoreError::ReadIndex
        | RestoreError::ReadObject
        | RestoreError::ReadWorktree
        | RestoreError::InvalidPathEncoding => {
            CliError::fatal("failed to read repository state while checking out the working tree")
                .with_stable_code(StableErrorCode::IoReadFailed)
                .with_hint("failed to read repository state while checking out the working tree")
        }
        RestoreError::WriteWorktree => CliError::fatal(
            "working tree checkout did not complete because files could not be written",
        )
        .with_stable_code(StableErrorCode::IoWriteFailed)
        .with_hint("working tree checkout did not complete because files could not be written"),
        RestoreError::LfsDownload => {
            CliError::fatal("checkout required downloading LFS content, but the transfer failed")
                .with_stable_code(StableErrorCode::NetworkUnavailable)
                .with_hint("checkout required downloading LFS content, but the transfer failed")
        }
        // `clone` never resolves user revisions, so the locked-source guard
        // in `restore::run_restore` is unreachable here. Surface a fatal
        // diagnostic rather than panicking on the unreachable branch — keeps
        // the match exhaustive without burying the case.
        RestoreError::LockedSource(name) => CliError::fatal(format!(
            "internal error: clone checkout attempted to restore from locked branch '{name}'"
        ))
        .with_stable_code(StableErrorCode::RepoStateInvalid),
    }
}

fn map_local_branch_state_error(source: branch::BranchStoreError) -> CliError {
    match source {
        branch::BranchStoreError::Query(detail) => {
            CliError::fatal(format!(
                "failed to inspect local branch state after fetch: {detail}"
            ))
            .with_stable_code(StableErrorCode::IoReadFailed)
        }
        branch::BranchStoreError::Corrupt { .. } => {
            CliError::fatal(format!(
                "failed to inspect local branch state after fetch: {source}"
            ))
            .with_stable_code(StableErrorCode::RepoCorrupt)
        }
        branch::BranchStoreError::NotFound(name) => {
            CliError::fatal(format!(
                "failed to inspect local branch state after fetch: branch '{name}' not found"
            ))
            .with_stable_code(StableErrorCode::RepoStateInvalid)
        }
        branch::BranchStoreError::Delete { name, detail } => CliError::fatal(format!(
            "failed to inspect local branch state after fetch: failed to delete branch '{name}': {detail}"
        ))
        .with_stable_code(StableErrorCode::IoWriteFailed),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn contains_initialized_repo(metadata_root: &Path) -> bool {
    REPO_MARKERS
        .iter()
        .any(|marker| metadata_root.join(marker).exists())
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

fn display_home_relative(path: &str) -> String {
    let Some(home) = dirs::home_dir() else {
        return path.to_string();
    };
    let home = home.to_string_lossy().to_string();
    if let Some(rest) = path.strip_prefix(&home) {
        return format!("~{rest}");
    }
    path.to_string()
}

// ---------------------------------------------------------------------------
// Cleanup
// ---------------------------------------------------------------------------

/// Attempt to clean up a failed clone. Returns a warning string if cleanup
/// itself fails, so the caller can surface it via `CliError.hints`.
fn cleanup_failed_clone(local_path: &Path, created_by_clone: bool) -> Option<String> {
    let cleanup_result = if created_by_clone {
        fs::remove_dir_all(local_path)
    } else {
        clear_directory_contents(local_path)
    };

    match cleanup_result {
        Ok(()) => None,
        Err(error) => {
            let warning = format!(
                "warning: failed to clean up '{}': {}",
                local_path.display(),
                error
            );
            tracing::error!("{}", warning);
            Some(warning)
        }
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

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

pub async fn execute(args: CloneArgs) {
    if let Err(err) = execute_safe(args, &OutputConfig::default()).await {
        err.print_stderr();
    }
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting.
///
/// # Side Effects
/// - Creates the destination repository layout and object storage.
/// - Fetches objects from the remote URL and writes refs/config.
/// - Checks out the working tree for non-bare clones.
/// - Restores the original process working directory after success or failure.
/// - May remove the partially created destination when clone cleanup is needed.
///
/// # Errors
/// Returns [`CliError`] when destination validation fails, remote negotiation or
/// object transfer fails, refs/config cannot be written, checkout fails, cleanup
/// fails, or the original working directory cannot be restored.
///
/// This is the **rendering layer**: it calls `execute_clone()` to get a
/// `CloneOutput` and then renders it according to the `OutputConfig`.
pub async fn execute_safe(args: CloneArgs, output: &OutputConfig) -> CliResult<()> {
    let original_dir = util::cur_dir();
    let (result, cleanup_warning) = execute_clone(&args, &original_dir, output).await;

    // Always restore the working directory.
    if env::current_dir().ok().as_ref() != Some(&original_dir) {
        env::set_current_dir(&original_dir).map_err(|source| {
            CliError::from(CloneError::RestoreDirectory {
                path: original_dir.clone(),
                source,
            })
        })?;
    }

    match result {
        Ok(clone_output) => render_clone_result(&clone_output, output),
        Err(error) => {
            let mut cli_error = CliError::from(error);
            if let Some(warning) = cleanup_warning {
                cli_error = cli_error.with_priority_hint(warning);
            }
            Err(cli_error)
        }
    }
}

/// Render the successful clone result to stdout / stderr.
fn render_clone_result(result: &CloneOutput, output: &OutputConfig) -> CliResult<()> {
    if output.is_json() {
        return emit_json_data("clone", result, output);
    }
    if output.quiet {
        return Ok(());
    }

    // Human-readable success summary on stdout.
    if result.bare {
        println!("Cloned into bare repository '{}'", result.path);
    } else {
        // Show just the directory name, not the full path.
        let display_path = Path::new(&result.path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&result.path);
        println!("Cloned into '{display_path}'");
    }
    println!("  remote: origin → {}", result.remote_url);
    if let Some(branch) = &result.branch {
        println!("  branch: {branch}");
    }
    println!(
        "  signing: {}",
        if result.vault_signing {
            "enabled"
        } else {
            "disabled"
        }
    );

    // SSH key tip.
    if let Some(key_path) = &result.ssh_key_detected {
        println!();
        println!(
            "Tip: using existing SSH key at {}",
            display_home_relative(key_path)
        );
    }

    // .gitignore → .libraignore conversion tip.
    if !result.gitignore_converted.is_empty() {
        println!();
        let n = result.gitignore_converted.len();
        let plural = if n == 1 { "" } else { "s" };
        println!(
            "Tip: {n} .gitignore file{plural} converted to .libraignore — \
             run 'libra add .libraignore' (or 'libra add -A') to track them, \
             then 'libra commit' to record the change."
        );
    }

    // Warnings on stderr.
    for w in &result.warnings {
        eprintln!("warning: {w}");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Execution layer — produces CloneOutput, no rendering
// ---------------------------------------------------------------------------

/// Returns `(result, cleanup_warning)`. The cleanup warning is `Some` only
/// when the clone fails **and** the subsequent directory cleanup also fails.
async fn execute_clone(
    args: &CloneArgs,
    original_dir: &Path,
    output: &OutputConfig,
) -> (Result<CloneOutput, CloneError>, Option<String>) {
    match execute_clone_inner(args, original_dir, output).await {
        Ok(clone_output) => (Ok(clone_output), None),
        Err((error, cleanup_warning)) => (Err(error), cleanup_warning),
    }
}

/// Inner implementation that returns an error tuple containing the clone error
/// and an optional cleanup warning.
async fn execute_clone_inner(
    args: &CloneArgs,
    original_dir: &Path,
    output: &OutputConfig,
) -> Result<CloneOutput, (CloneError, Option<String>)> {
    // Codex pass-7 P1: intercept the Cloudflare publish source scheme
    // before generic remote discovery. Phase 5 of
    // `docs/improvement/publish.md` lands the actual D1/R2 restore
    // path; until then we fail with a clear "not yet implemented"
    // error pointing the user at the Phase 5 release window. This
    // prevents a `libra+cloud://...` URL from falling into the
    // generic Git fetch path and emitting a confusing protocol
    // error.
    if args.remote_repo.starts_with("libra+cloud://") {
        let source =
            parse_cloud_publish_source(&args.remote_repo).map_err(|error| (error, None))?;
        validate_cloud_clone_option_compatibility(args).map_err(|error| (error, None))?;
        let clone_config = load_cloud_clone_domain_config(&source)
            .await
            .map_err(|error| (error, None))?;
        let publish_site = resolve_cloud_publish_site(&source, &clone_config)
            .await
            .map_err(|error| (error, None))?;
        let mut config_details = clone_config.into_error_details(source.clone_domain.clone());
        config_details.extend(cloud_publish_site_error_details(&publish_site));
        return Err((
            CloneError::CloudPublishSourceNotYetImplemented {
                input: args.remote_repo.clone(),
                target: source.target_label(),
                selector: source.selector_label(),
                config_details,
            },
            None,
        ));
    }

    let mut remote_repo = args.remote_repo.clone();
    if !remote_repo.ends_with('/') {
        remote_repo.push('/');
    }

    // --- Step 1: Resolve local path ---
    let local_path = match &args.local_path {
        Some(path) => path.clone(),
        None => {
            let repo_name = util::get_repo_name_from_url(&remote_repo)
                .ok_or((CloneError::CannotInferDestination, None))?;
            original_dir.join(repo_name).to_string_lossy().into_owned()
        }
    };

    let local_path = PathBuf::from(&local_path);
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

    // --- Step 2: Remote discovery ---
    if !output.quiet && !output.is_json() {
        eprintln!("Connecting to {} ...", args.remote_repo);
    }

    let (remote_client, discovery) = fetch::discover_remote(&remote_repo)
        .await
        .map_err(|source| (CloneError::DiscoverRemote { source }, None))?;

    // --- Step 3: Destination pre-checks ---
    if metadata_root.exists() && contains_initialized_repo(&metadata_root) {
        return Err((
            CloneError::DestinationAlreadyRepo {
                path: local_path.clone(),
            },
            None,
        ));
    }
    if local_path.exists() && !util::is_empty_dir(&local_path) {
        return Err((
            CloneError::DestinationExistsNonEmpty {
                path: local_path.clone(),
            },
            None,
        ));
    }

    let created_by_clone = if local_path.exists() {
        false
    } else {
        fs::create_dir_all(&local_path).map_err(|source| {
            (
                CloneError::CreateDestinationFailed {
                    path: local_path.clone(),
                    source,
                },
                None,
            )
        })?;
        true
    };

    // --- Pre-check: specified branch exists on remote ---
    if let Some(branch) = &args.branch
        && !fetch::remote_has_branch(&discovery.refs, branch)
    {
        let cleanup_warning = cleanup_failed_clone(&local_path, created_by_clone);
        return Err((
            CloneError::RemoteBranchNotFound {
                branch: branch.clone(),
            },
            cleanup_warning,
        ));
    }

    // --- Step 4–7: clone into destination ---
    let remote_url = fetch::normalize_remote_url(&remote_repo, &remote_client);

    clone_into_destination(
        args,
        &remote_url,
        &remote_client,
        &discovery,
        &local_path,
        original_dir,
        output,
    )
    .await
    .map_err(|error| {
        if env::current_dir().ok().as_deref() != Some(original_dir) {
            let _ = env::set_current_dir(original_dir);
        }
        let cleanup_warning = cleanup_failed_clone(&local_path, created_by_clone);
        (error, cleanup_warning)
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CloudPublishSource {
    clone_domain: String,
    target: CloudPublishTarget,
    selector: Option<CloudPublishSelector>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum CloudPublishTarget {
    Slug(String),
    RepoId(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum CloudPublishSelector {
    Ref(String),
    Revision(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CloudCloneDomainConfig {
    account_id: String,
    api_token: String,
    d1_database_id: String,
    r2_bucket: String,
    credential_profile: Option<String>,
}

impl CloudCloneDomainConfig {
    fn into_error_details(self, clone_domain: String) -> Vec<(&'static str, String)> {
        let mut details = vec![
            ("clone_domain", clone_domain),
            ("cloud_account_id", self.account_id),
            ("cloud_d1_database_id", self.d1_database_id),
            ("cloud_r2_bucket", self.r2_bucket),
        ];
        if let Some(credential_profile) = self.credential_profile {
            details.push(("cloud_credential_profile", credential_profile));
        }
        details
    }
}

impl CloudPublishSource {
    fn target_label(&self) -> String {
        match &self.target {
            CloudPublishTarget::Slug(slug) => format!("slug:{slug}"),
            CloudPublishTarget::RepoId(repo_id) => format!("repo:{repo_id}"),
        }
    }

    fn selector_label(&self) -> Option<String> {
        self.selector.as_ref().map(|selector| match selector {
            CloudPublishSelector::Ref(value) => format!("ref:{value}"),
            CloudPublishSelector::Revision(value) => format!("revision:{value}"),
        })
    }
}

fn parse_cloud_publish_source(input: &str) -> Result<CloudPublishSource, CloneError> {
    let url = Url::parse(input).map_err(|source| CloneError::InvalidCloudPublishSource {
        input: input.to_string(),
        reason: format!("URL parse failed: {source}"),
    })?;
    if url.scheme() != "libra+cloud" {
        return Err(invalid_cloud_source(input, "scheme must be libra+cloud"));
    }

    let clone_domain = url
        .host_str()
        .ok_or_else(|| invalid_cloud_source(input, "clone domain is required"))?;
    validate_cloud_clone_domain(input, clone_domain)?;
    let clone_domain = clone_domain.to_ascii_lowercase();

    let segments = url
        .path_segments()
        .map(|segments| segments.collect::<Vec<_>>())
        .unwrap_or_default();
    let target = match segments.as_slice() {
        [] | [""] => return Err(invalid_cloud_source(input, "slug or repo_id is required")),
        [slug] => {
            validate_cloud_slug(input, slug, "slug")?;
            CloudPublishTarget::Slug((*slug).to_string())
        }
        ["repo", repo_id] => {
            validate_cloud_slug(input, repo_id, "repo_id")?;
            CloudPublishTarget::RepoId((*repo_id).to_string())
        }
        _ => {
            return Err(invalid_cloud_source(
                input,
                "path must be /<slug> or /repo/<repo_id>",
            ));
        }
    };

    let mut ref_selector: Option<String> = None;
    let mut revision_selector: Option<String> = None;
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "ref" => {
                if ref_selector.replace(value.into_owned()).is_some() {
                    return Err(invalid_cloud_source(
                        input,
                        "ref selector appears more than once",
                    ));
                }
            }
            "revision" => {
                if revision_selector.replace(value.into_owned()).is_some() {
                    return Err(invalid_cloud_source(
                        input,
                        "revision selector appears more than once",
                    ));
                }
            }
            other => {
                return Err(invalid_cloud_source(
                    input,
                    &format!("unsupported query parameter '{other}'"),
                ));
            }
        }
    }
    if ref_selector.is_some() && revision_selector.is_some() {
        return Err(invalid_cloud_source(
            input,
            "ref and revision selectors are mutually exclusive",
        ));
    }
    let selector = if let Some(selector) = ref_selector {
        validate_cloud_ref_selector(input, &selector)?;
        Some(CloudPublishSelector::Ref(selector))
    } else if let Some(selector) = revision_selector {
        validate_cloud_revision_selector(input, &selector)?;
        Some(CloudPublishSelector::Revision(selector))
    } else {
        None
    };

    Ok(CloudPublishSource {
        clone_domain,
        target,
        selector,
    })
}

fn validate_cloud_clone_option_compatibility(args: &CloneArgs) -> Result<(), CloneError> {
    if args.branch.is_some() {
        return Err(CloneError::UnsupportedCloudCloneOption {
            option: "--branch",
            reason: "cloud source refs are selected in the source URL, not with Git branch flags",
            hint: "use `?ref=<branch|tag|full-ref>` on the libra+cloud:// URL instead of `--branch`.",
        });
    }
    if args.depth.is_some() {
        return Err(CloneError::UnsupportedCloudCloneOption {
            option: "--depth",
            reason: "Cloudflare restore must download the complete published object set",
            hint: "`--depth` is only supported for Git remotes; omit it for libra+cloud:// sources.",
        });
    }
    if args.single_branch {
        return Err(CloneError::UnsupportedCloudCloneOption {
            option: "--single-branch",
            reason: "Cloudflare restore must preserve all published refs in the local repository",
            hint: "`--single-branch` is only supported for Git remotes; use `?ref=<branch|tag|full-ref>` to select the checkout target.",
        });
    }
    if args.bare {
        return Err(CloneError::UnsupportedCloudCloneOption {
            option: "--bare",
            reason: "Cloudflare restore currently targets a non-bare working repository",
            hint: "`--bare` is only supported for Git remotes until libra+cloud:// restore grows bare-repository support.",
        });
    }
    Ok(())
}

async fn load_cloud_clone_domain_config(
    source: &CloudPublishSource,
) -> Result<CloudCloneDomainConfig, CloneError> {
    let required_suffixes = ["account_id", "d1_database_id", "r2_bucket"];
    let mut missing_keys = Vec::new();
    let local_target = clone_config_local_target();
    let mut account_id = None;
    let mut d1_database_id = None;
    let mut r2_bucket = None;

    for suffix in required_suffixes {
        let key = format!("cloud.clone_domains.{}.{}", source.clone_domain, suffix);
        match read_cascaded_config_value_decrypted(local_target, &key).await {
            Ok(Some(value)) => match suffix {
                "account_id" => account_id = Some(value),
                "d1_database_id" => d1_database_id = Some(value),
                "r2_bucket" => r2_bucket = Some(value),
                _ => {}
            },
            Ok(None) => missing_keys.push(key),
            Err(source_error) => {
                return Err(CloneError::CloudCloneDomainConfigRead {
                    domain: source.clone_domain.clone(),
                    source: source_error,
                });
            }
        }
    }

    if !missing_keys.is_empty() {
        return Err(CloneError::CloudCloneDomainNotConfigured {
            domain: source.clone_domain.clone(),
            missing_keys: missing_keys.join(", "),
        });
    }

    let credential_profile_key = format!(
        "cloud.clone_domains.{}.credential_profile",
        source.clone_domain
    );
    let credential_profile =
        read_cascaded_config_value_decrypted(local_target, &credential_profile_key)
            .await
            .map_err(|source_error| CloneError::CloudCloneDomainConfigRead {
                domain: source.clone_domain.clone(),
                source: source_error,
            })?;

    Ok(CloudCloneDomainConfig {
        account_id: account_id.ok_or_else(|| CloneError::CloudCloneDomainNotConfigured {
            domain: source.clone_domain.clone(),
            missing_keys: format!("cloud.clone_domains.{}.account_id", source.clone_domain),
        })?,
        api_token: resolve_env_for_target("LIBRA_D1_API_TOKEN", local_target)
            .await
            .map_err(|source_error| CloneError::CloudCloneDomainConfigRead {
                domain: source.clone_domain.clone(),
                source: source_error,
            })?
            .filter(|value| !value.is_empty())
            .ok_or_else(|| CloneError::CloudCloneD1ApiTokenNotConfigured {
                domain: source.clone_domain.clone(),
            })?,
        d1_database_id: d1_database_id.ok_or_else(|| {
            CloneError::CloudCloneDomainNotConfigured {
                domain: source.clone_domain.clone(),
                missing_keys: format!("cloud.clone_domains.{}.d1_database_id", source.clone_domain),
            }
        })?,
        r2_bucket: r2_bucket.ok_or_else(|| CloneError::CloudCloneDomainNotConfigured {
            domain: source.clone_domain.clone(),
            missing_keys: format!("cloud.clone_domains.{}.r2_bucket", source.clone_domain),
        })?,
        credential_profile,
    })
}

async fn resolve_cloud_publish_site(
    source: &CloudPublishSource,
    config: &CloudCloneDomainConfig,
) -> Result<PublishSiteRow, CloneError> {
    let d1_client = D1Client::new(
        config.account_id.clone(),
        config.api_token.clone(),
        config.d1_database_id.clone(),
    );
    let target = source.target_label();
    let result = match &source.target {
        CloudPublishTarget::Slug(slug) => {
            d1_client
                .find_publish_site_by_clone_slug(&source.clone_domain, slug)
                .await
        }
        CloudPublishTarget::RepoId(repo_id) => {
            d1_client
                .find_publish_site_by_clone_repo_id(&source.clone_domain, repo_id)
                .await
        }
    }
    .map_err(|source_error| CloneError::CloudPublishSiteLookupFailed {
        domain: source.clone_domain.clone(),
        target: target.clone(),
        code: source_error.code,
        message: source_error.message,
    })?;

    let site = result.ok_or_else(|| CloneError::CloudPublishSiteNotFound {
        domain: source.clone_domain.clone(),
        target: target.clone(),
    })?;
    if site.status != "active" {
        return Err(CloneError::CloudPublishSiteUnavailable {
            domain: source.clone_domain.clone(),
            target,
            status: site.status,
        });
    }
    Ok(site)
}

fn cloud_publish_site_error_details(site: &PublishSiteRow) -> Vec<(&'static str, String)> {
    let mut details = vec![
        ("cloud_site_id", site.site_id.clone()),
        ("cloud_repo_id", site.repo_id.clone()),
        ("cloud_slug", site.slug.clone()),
        ("cloud_site_status", site.status.clone()),
        ("cloud_refs_generation", site.refs_generation.to_string()),
    ];
    if let Some(default_ref) = &site.default_ref {
        details.push(("cloud_default_ref", default_ref.clone()));
    }
    if let Some(latest_revision_oid) = &site.latest_revision_oid {
        details.push(("cloud_latest_revision_oid", latest_revision_oid.clone()));
    }
    details
}

fn clone_config_local_target() -> LocalIdentityTarget<'static> {
    if util::try_get_storage_path(None).is_ok() {
        LocalIdentityTarget::CurrentRepo
    } else {
        LocalIdentityTarget::None
    }
}

fn invalid_cloud_source(input: &str, reason: &str) -> CloneError {
    CloneError::InvalidCloudPublishSource {
        input: input.to_string(),
        reason: reason.to_string(),
    }
}

fn validate_cloud_clone_domain(input: &str, domain: &str) -> Result<(), CloneError> {
    if domain.is_empty() || domain.len() > 253 {
        return Err(invalid_cloud_source(input, "clone domain is invalid"));
    }
    for label in domain.split('.') {
        if label.is_empty()
            || label.len() > 63
            || label.starts_with('-')
            || label.ends_with('-')
            || !label
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
        {
            return Err(invalid_cloud_source(input, "clone domain is invalid"));
        }
    }
    Ok(())
}

fn validate_cloud_slug(input: &str, value: &str, label: &str) -> Result<(), CloneError> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.len() > 128
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        return Err(invalid_cloud_source(input, &format!("{label} is invalid")));
    }
    Ok(())
}

fn validate_cloud_ref_selector(input: &str, value: &str) -> Result<(), CloneError> {
    if value.is_empty()
        || value.trim() != value
        || value.contains("..")
        || value.starts_with('/')
        || value.ends_with('/')
        || value
            .chars()
            .any(|ch| ch.is_control() || matches!(ch, '?' | '#'))
    {
        return Err(invalid_cloud_source(input, "ref selector is invalid"));
    }
    Ok(())
}

fn validate_cloud_revision_selector(input: &str, value: &str) -> Result<(), CloneError> {
    if value == "latest" {
        return Ok(());
    }
    if value.len() < 4
        || value.len() > 64
        || !value
            .chars()
            .all(|ch| ch.is_ascii_digit() || matches!(ch, 'a'..='f'))
    {
        return Err(invalid_cloud_source(input, "revision selector is invalid"));
    }
    Ok(())
}

async fn clone_into_destination(
    args: &CloneArgs,
    remote_url: &str,
    _remote_client: &fetch::RemoteClient,
    discovery: &DiscoveryResult,
    local_path: &Path,
    original_dir: &Path,
    output: &OutputConfig,
) -> Result<CloneOutput, CloneError> {
    env::set_current_dir(local_path).map_err(|source| CloneError::ChangeDirectory {
        path: local_path.to_path_buf(),
        source,
    })?;

    let object_format = match discovery.hash_kind {
        git_internal::hash::HashKind::Sha1 => "sha1".to_string(),
        git_internal::hash::HashKind::Sha256 => "sha256".to_string(),
    };

    // --- Step 4: Initialize repository ---
    if !output.quiet && !output.is_json() {
        eprintln!("Initializing repository ...");
    }

    let init_output = command::init::run_init(command::init::InitArgs {
        bare: args.bare,
        template: None,
        initial_branch: args.branch.clone(),
        repo_directory: local_path.to_string_lossy().into_owned(),
        quiet: true,
        shared: None,
        object_format: Some(object_format.clone()),
        ref_format: None,
        from_git_repository: None,
        vault: true,
    })
    .await
    .map_err(|source| CloneError::InitializeRepository { source })?;

    // --- Step 5: Fetch objects ---
    if !output.quiet && !output.is_json() {
        eprintln!("Fetching objects ...");
    }

    let child_output = output.child_output_config();
    let remote_config = RemoteConfig {
        name: "origin".to_string(),
        url: remote_url.to_string(),
    };
    fetch::fetch_repository_safe(
        remote_config.clone(),
        args.branch.clone(),
        args.single_branch,
        args.depth,
        &child_output,
    )
    .await
    .map_err(|source| CloneError::FetchFailed { source })?;

    // --- Step 6–7: Configure repository + checkout ---
    if !output.quiet && !output.is_json() {
        eprintln!("Configuring repository ...");
    }

    if !args.bare && !output.quiet && !output.is_json() {
        eprintln!("Checking out working copy ...");
    }

    let setup_result =
        setup_repository(remote_config.clone(), args.branch.clone(), !args.bare).await?;

    let mut warnings = init_output.warnings.clone();
    let mut gitignore_converted = Vec::new();
    if !args.bare {
        let summary = ignore_utils::convert_gitignore_files_to_libraignore(local_path, local_path)
            .map_err(|source| CloneError::IgnoreFile { source })?;
        warnings.extend(summary.warnings);
        gitignore_converted = summary
            .converted
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
    }

    // Restore original directory before returning.
    env::set_current_dir(original_dir).map_err(|source| CloneError::RestoreDirectory {
        path: original_dir.to_path_buf(),
        source,
    })?;

    // Build CloneOutput.
    if setup_result.branch_name.is_none() {
        warnings.push("You appear to have cloned an empty repository.".to_string());
    }

    Ok(CloneOutput {
        path: local_path.to_string_lossy().into_owned(),
        bare: args.bare,
        remote_url: remote_url.to_string(),
        branch: setup_result.branch_name,
        object_format,
        repo_id: init_output.repo_id,
        vault_signing: init_output.vault_signing,
        ssh_key_detected: init_output.ssh_key_detected,
        shallow: args.depth.is_some(),
        warnings,
        gitignore_converted,
    })
}

// ---------------------------------------------------------------------------
// Setup — configures remote, branch, HEAD, reflog, and checkout
// ---------------------------------------------------------------------------

/// Result of `setup_repository`, carrying the branch that was checked out
/// (if any) so that `CloneOutput` can report it.
pub(crate) struct SetupResult {
    pub branch_name: Option<String>,
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
) -> Result<SetupResult, CloneError> {
    let db = get_db_conn_instance().await;
    let remote_head = Head::remote_current_with_conn(&db, &remote_config.name).await;

    let branch_to_checkout = match specified_branch {
        Some(branch_name) => Some(branch_name),
        None => match remote_head {
            Some(Head::Branch(name)) => Some(name),
            _ => None,
        },
    };

    if let Some(branch_name) = branch_to_checkout {
        let remote_tracking_ref = format!("refs/remotes/{}/{}", remote_config.name, branch_name);
        let origin_branch = Branch::find_branch_result_with_conn(
            &db,
            &remote_tracking_ref,
            Some(&remote_config.name),
        )
        .await
        .map_err(|source| CloneError::LocalBranchState { source })?
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

        // Clone the branch name before moving it into the closure.
        let branch_name_for_result = branch_name.clone();
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
                    .await?;
                    Head::update_with_conn(txn, Head::Branch(branch_name.to_owned()), None).await;

                    let merge_ref = format!("refs/heads/{}", branch_name);
                    let _ = ConfigKv::set_with_conn(
                        txn,
                        &format!("branch.{}.merge", branch_name),
                        &merge_ref,
                        false,
                    )
                    .await;
                    let _ = ConfigKv::set_with_conn(
                        txn,
                        &format!("branch.{}.remote", branch_name),
                        &remote_config.name,
                        false,
                    )
                    .await;
                    let _ = ConfigKv::set_with_conn(
                        txn,
                        &format!("remote.{}.url", remote_config.name),
                        &remote_config.url,
                        false,
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
            command::restore::execute_checked_typed(RestoreArgs {
                worktree: true,
                staged: true,
                source: None,
                pathspec: vec![util::working_dir_string()],
            })
            .await
            .map_err(|source| CloneError::CheckoutFailed { source })?;
        }

        Ok(SetupResult {
            branch_name: Some(branch_name_for_result),
        })
    } else {
        let _ = ConfigKv::set(
            &format!("remote.{}.url", remote_config.name),
            &remote_config.url,
            false,
        )
        .await;

        let default_branch = "main";
        let merge_ref = format!("refs/heads/{}", default_branch);
        let _ = ConfigKv::set(&format!("branch.{default_branch}.merge"), &merge_ref, false).await;
        let _ = ConfigKv::set(
            &format!("branch.{default_branch}.remote"),
            &remote_config.name,
            false,
        )
        .await;

        Ok(SetupResult { branch_name: None })
    }
}

/// Unit tests for the clone module
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_remote_unauthorized_maps_to_auth_permission_denied() {
        let cli = map_discover_remote_error(fetch::FetchError::Discovery {
            remote: "ssh://example.com/repo.git".to_string(),
            source: GitError::UnAuthorized("permission denied".to_string()),
        });

        assert_eq!(cli.stable_code(), StableErrorCode::AuthPermissionDenied);
        assert_eq!(cli.exit_code(), 128);
        assert_eq!(
            cli.hints()[0].as_str(),
            "check SSH key / HTTP credentials and repository access rights"
        );
    }

    #[test]
    fn discover_remote_unsupported_scheme_maps_to_cli_invalid_target() {
        let cli = map_discover_remote_error(fetch::FetchError::InvalidRemoteSpec {
            spec: "ftp://example.com/repo.git".to_string(),
            kind: RemoteSpecErrorKind::UnsupportedScheme,
            reason: "unsupported remote scheme 'ftp'".to_string(),
        });

        assert_eq!(cli.stable_code(), StableErrorCode::CliInvalidTarget);
        assert_eq!(cli.exit_code(), 129);
        assert_eq!(
            cli.hints()[0].as_str(),
            "check the clone URL or scheme, for example `https://`, `ssh`, or a local path"
        );
    }

    #[test]
    fn discover_remote_network_error_maps_to_network_unavailable() {
        let cli = map_discover_remote_error(fetch::FetchError::Discovery {
            remote: "https://example.com/repo.git".to_string(),
            source: GitError::NetworkError("timed out".to_string()),
        });

        assert_eq!(cli.stable_code(), StableErrorCode::NetworkUnavailable);
        assert_eq!(cli.exit_code(), 128);
        assert_eq!(
            cli.hints()[0].as_str(),
            "check the remote host, DNS, VPN/proxy, and network connectivity"
        );
    }

    #[test]
    fn discover_remote_io_error_maps_to_io_read_failed() {
        let cli = map_discover_remote_error(fetch::FetchError::Discovery {
            remote: "/local/repo".to_string(),
            source: GitError::IOError(std::io::Error::other("permission denied")),
        });

        assert_eq!(cli.stable_code(), StableErrorCode::IoReadFailed);
        assert_eq!(cli.exit_code(), 128);
        assert_eq!(
            cli.hints()[0].as_str(),
            "check filesystem permissions and repository integrity"
        );
    }

    #[test]
    fn checkout_read_index_maps_to_io_read_failed() {
        let cli = map_checkout_error(RestoreError::ReadIndex);

        assert_eq!(cli.stable_code(), StableErrorCode::IoReadFailed);
        assert_eq!(cli.exit_code(), 128);
    }

    #[test]
    fn checkout_resolve_source_maps_to_repo_state_invalid() {
        let cli = map_checkout_error(RestoreError::ResolveSource);

        assert_eq!(cli.stable_code(), StableErrorCode::RepoStateInvalid);
        assert_eq!(cli.exit_code(), 128);
    }

    #[test]
    fn checkout_write_worktree_maps_to_io_write_failed() {
        let cli = map_checkout_error(RestoreError::WriteWorktree);

        assert_eq!(cli.stable_code(), StableErrorCode::IoWriteFailed);
        assert_eq!(cli.exit_code(), 128);
    }

    #[test]
    fn local_branch_state_query_maps_to_io_read_failed() {
        let cli = map_local_branch_state_error(branch::BranchStoreError::Query(
            "database is locked".into(),
        ));

        assert_eq!(cli.stable_code(), StableErrorCode::IoReadFailed);
        assert_eq!(cli.exit_code(), 128);
    }

    #[test]
    fn local_branch_state_corrupt_maps_to_repo_corrupt() {
        let cli = map_local_branch_state_error(branch::BranchStoreError::Corrupt {
            name: "refs/remotes/origin/main".into(),
            detail: "invalid object id".into(),
        });

        assert_eq!(cli.stable_code(), StableErrorCode::RepoCorrupt);
        assert_eq!(cli.exit_code(), 128);
    }

    #[test]
    fn cloud_clone_source_parse_test_accepts_slug_repo_and_selectors() {
        for (input, expected) in [
            (
                "libra+cloud://code.example.com/kepler-ledger",
                CloudPublishSource {
                    clone_domain: "code.example.com".to_string(),
                    target: CloudPublishTarget::Slug("kepler-ledger".to_string()),
                    selector: None,
                },
            ),
            (
                "libra+cloud://code.example.com/repo/rp_8f4c1b",
                CloudPublishSource {
                    clone_domain: "code.example.com".to_string(),
                    target: CloudPublishTarget::RepoId("rp_8f4c1b".to_string()),
                    selector: None,
                },
            ),
            (
                "libra+cloud://code.example.com/kepler-ledger?ref=refs/tags/v1.0.0",
                CloudPublishSource {
                    clone_domain: "code.example.com".to_string(),
                    target: CloudPublishTarget::Slug("kepler-ledger".to_string()),
                    selector: Some(CloudPublishSelector::Ref("refs/tags/v1.0.0".to_string())),
                },
            ),
            (
                "libra+cloud://code.example.com/kepler-ledger?ref=feature/branch",
                CloudPublishSource {
                    clone_domain: "code.example.com".to_string(),
                    target: CloudPublishTarget::Slug("kepler-ledger".to_string()),
                    selector: Some(CloudPublishSelector::Ref("feature/branch".to_string())),
                },
            ),
            (
                "libra+cloud://code.example.com/kepler-ledger?revision=latest",
                CloudPublishSource {
                    clone_domain: "code.example.com".to_string(),
                    target: CloudPublishTarget::Slug("kepler-ledger".to_string()),
                    selector: Some(CloudPublishSelector::Revision("latest".to_string())),
                },
            ),
            (
                "libra+cloud://CODE.EXAMPLE.COM/kepler-ledger?revision=9a1f3e2c",
                CloudPublishSource {
                    clone_domain: "code.example.com".to_string(),
                    target: CloudPublishTarget::Slug("kepler-ledger".to_string()),
                    selector: Some(CloudPublishSelector::Revision("9a1f3e2c".to_string())),
                },
            ),
        ] {
            let parsed = parse_cloud_publish_source(input).unwrap_or_else(|error| {
                panic!("{input} should parse as a valid cloud publish source: {error}")
            });
            assert_eq!(
                parsed, expected,
                "{input} should preserve restore selectors"
            );
        }
    }

    #[test]
    fn cloud_clone_source_parse_test_rejects_invalid_inputs() {
        for (input, needle) in [
            ("libra+cloud://bad_domain/repo", "clone domain"),
            ("libra+cloud://code.example.com/", "slug or repo_id"),
            ("libra+cloud://code.example.com/repo/", "repo_id"),
            ("libra+cloud://code.example.com/bad slug", "slug is invalid"),
            (
                "libra+cloud://code.example.com/slug?ref=main&revision=latest",
                "mutually exclusive",
            ),
            (
                "libra+cloud://code.example.com/slug?revision=ABCDEF",
                "revision selector",
            ),
            (
                "libra+cloud://code.example.com/slug?ref=../main",
                "ref selector",
            ),
            (
                "libra+cloud://code.example.com/slug?foo=bar",
                "unsupported query parameter",
            ),
        ] {
            let error =
                parse_cloud_publish_source(input).expect_err("invalid cloud source rejected");
            assert!(
                error.to_string().contains(needle),
                "{input} should mention {needle:?}, got {error}",
            );
            let cli: CliError = error.into();
            assert_eq!(cli.stable_code(), StableErrorCode::CliInvalidArguments);
        }
    }

    #[test]
    fn cloud_clone_domain_resolve_test_site_details_are_carried_to_restore_stub() {
        let site = PublishSiteRow {
            site_id: "site_123".to_string(),
            repo_id: "repo_456".to_string(),
            clone_domain: "code.example.com".to_string(),
            slug: "kepler-ledger".to_string(),
            display_origin: "https://code.example.com".to_string(),
            name: "Kepler Ledger".to_string(),
            visibility: "public".to_string(),
            status: "active".to_string(),
            worker_name: "libra-publish".to_string(),
            default_ref: Some("refs/heads/main".to_string()),
            latest_revision_oid: Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string()),
            refs_generation: 7,
            max_preview_bytes: 1024,
            schema_version: 1,
            created_at: "2026-05-13T00:00:00Z".to_string(),
            updated_at: "2026-05-13T00:00:00Z".to_string(),
        };

        let details = cloud_publish_site_error_details(&site);
        assert!(details.contains(&("cloud_site_id", "site_123".to_string())));
        assert!(details.contains(&("cloud_repo_id", "repo_456".to_string())));
        assert!(details.contains(&("cloud_slug", "kepler-ledger".to_string())));
        assert!(details.contains(&("cloud_default_ref", "refs/heads/main".to_string())));
        assert!(details.contains(&(
            "cloud_latest_revision_oid",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        )));
        assert!(details.contains(&("cloud_refs_generation", "7".to_string())));
    }
}
