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

use super::fetch::{self, RemoteSpecErrorKind};
use crate::{
    command::{
        self,
        init::InitError,
        restore::{RestoreArgs, RestoreError},
    },
    internal::{
        branch::Branch,
        config::{ConfigKv, RemoteConfig},
        head::Head,
        reflog::{ReflogAction, ReflogContext, with_reflog},
    },
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, ProgressMode, emit_json_data},
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
    #[error("failed to inspect local branch state after fetch: {message}")]
    LocalBranchState { message: String },
    #[error("fetch failed: {source}")]
    FetchFailed { source: fetch::FetchError },
    #[error("failed to checkout working tree")]
    CheckoutFailed { source: RestoreError },
    #[error("failed to complete clone setup: {message}")]
    SetupFailed { message: String },
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
            CloneError::LocalBranchState { message } => CliError::fatal(format!(
                "failed to inspect local branch state after fetch: {message}"
            ))
            .with_stable_code(StableErrorCode::RepoCorrupt)
            .with_hint("run 'libra status' to verify the local repository state"),
            CloneError::FetchFailed { source } => map_fetch_error(source),
            CloneError::CheckoutFailed { source } => map_checkout_error(source),
            CloneError::SetupFailed { .. } => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::InternalInvariant)
                .with_hint(format!("please report this issue at: {ISSUE_URL}")),
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
        RestoreError::ReadIndex | RestoreError::ReadObject | RestoreError::InvalidPathEncoding => {
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

/// Build a child `OutputConfig` that suppresses all output from nested
/// operations (init, fetch) when the parent is in JSON or machine mode.
fn child_output_config(output: &OutputConfig) -> OutputConfig {
    if output.is_json() || output.quiet {
        OutputConfig {
            json_format: None,
            quiet: true,
            progress: ProgressMode::None,
            ..output.clone()
        }
    } else {
        output.clone()
    }
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
/// errors and exiting. Fetches objects from a remote URL, writes refs/config,
/// and checks out the working tree. Restores the original working directory on
/// failure.
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

async fn clone_into_destination(
    args: &CloneArgs,
    remote_url: &str,
    _remote_client: &fetch::RemoteClient,
    discovery: &crate::internal::protocol::DiscoveryResult,
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

    let child_output = child_output_config(output);
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

    // Restore original directory before returning.
    env::set_current_dir(original_dir).map_err(|source| CloneError::RestoreDirectory {
        path: original_dir.to_path_buf(),
        source,
    })?;

    // Build CloneOutput.
    let mut warnings = init_output.warnings.clone();
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
    let db = crate::internal::db::get_db_conn_instance().await;
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
        .map_err(|error| CloneError::LocalBranchState {
            message: error.to_string(),
        })?
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
}
