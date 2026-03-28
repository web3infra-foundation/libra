//! Push command wiring that reads remote configuration, negotiates with servers, and sends local refs and pack data for update.

use std::{
    collections::{HashSet, VecDeque},
    io::Write,
    path::Path,
    str::FromStr,
    time::Duration,
};

use bytes::BytesMut;
use clap::Parser;
use git_internal::{
    errors::GitError,
    hash::{HashKind, ObjectHash, get_hash_kind},
    internal::{
        metadata::{EntryMeta, MetaAttached},
        object::{
            blob::Blob,
            commit::Commit,
            tree::{Tree, TreeItemMode},
        },
        pack::{encode::PackEncoder, entry::Entry},
    },
};
use sea_orm::TransactionTrait;
use serde::Serialize;
use tokio::sync::mpsc;
use url::Url;

use crate::{
    command::{branch, fetch::RemoteClient},
    git_protocol::{ServiceType::ReceivePack, add_pkt_line_string, read_pkt_line},
    info_println,
    internal::{
        branch::Branch,
        config::ConfigKv,
        db::get_db_conn_instance,
        head::Head,
        protocol::{
            ProtocolClient, get_wire_hash_kind, set_wire_hash_kind, ssh_client::is_ssh_spec,
        },
        reflog::{Reflog, ReflogAction, ReflogContext, ReflogError},
    },
    utils::{
        error::{CliError, CliResult, StableErrorCode, emit_warning},
        object_ext::{BlobExt, CommitExt, TreeExt},
        output::{OutputConfig, ProgressMode, ProgressReporter, emit_json_data},
    },
};

const ISSUE_URL: &str = "https://github.com/web3infra-foundation/libra/issues";

/// Connection/idle timeout for push network operations (discovery, send-pack, receive-pack).
const PUSH_TIMEOUT: Duration = Duration::from_secs(10);

/// Push local refs and objects to a remote repository.
///
/// # Examples
///
/// ```text
/// libra push                             Push current branch to tracking remote
/// libra push origin main                 Push main branch to origin
/// libra push -u origin feature-x         Push and set upstream tracking
/// libra push --force origin main         Force push (overwrites remote history)
/// libra push --dry-run                   Preview what would be pushed
/// libra push --json                      Structured JSON output for agents
/// ```
#[derive(Parser, Debug)]
pub struct PushArgs {
    /// repository, e.g. origin
    #[clap(requires("refspec"))]
    repository: Option<String>,
    /// ref to push, e.g. master or local_branch:remote_branch
    #[clap(requires("repository"))]
    refspec: Option<String>,

    #[clap(long, short = 'u', requires("refspec"), requires("repository"))]
    set_upstream: bool,

    /// force push to remote repository
    #[clap(long, short = 'f')]
    pub force: bool,

    /// Do everything except actually send the updates
    #[clap(long, short = 'n')]
    pub dry_run: bool,
}

// ---------------------------------------------------------------------------
// Structured error types
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum PushError {
    #[error("HEAD is detached; cannot determine what to push")]
    DetachedHead,

    #[error("no configured push destination")]
    NoRemoteConfigured,

    #[error("remote '{name}' not found")]
    RemoteNotFound {
        name: String,
        suggestion: Option<String>,
    },

    #[error("invalid refspec '{0}'")]
    InvalidRefspec(String),

    #[error("source ref '{0}' not found")]
    SourceRefNotFound(String),

    #[error("pushing to local file repositories is not supported")]
    UnsupportedLocalFileRemote,

    #[error("invalid remote URL '{url}': {detail}")]
    InvalidRemoteUrl { url: String, detail: String },

    #[error("authentication failed for '{url}'")]
    AuthenticationFailed { url: String },

    #[error("failed to discover references from '{url}': {detail}")]
    DiscoveryFailed { url: String, detail: String },

    #[error("network timeout during {phase} after {seconds}s")]
    Timeout { phase: String, seconds: u64 },

    #[error("cannot push to '{remote_ref}': non-fast-forward update")]
    NonFastForward {
        local_ref: String,
        remote_ref: String,
    },

    #[error("remote object format '{remote}' does not match local '{local}'")]
    HashKindMismatch { remote: String, local: String },

    #[error("failed to collect objects for push: {0}")]
    ObjectCollection(String),

    #[error("pack encoding failed: {0}")]
    PackEncoding(String),

    #[error("remote rejected push: unpack failed")]
    RemoteUnpackFailed,

    #[error("remote rejected ref update for '{refname}': {reason}")]
    RemoteRefUpdateFailed { refname: String, reason: String },

    #[error("network error: {0}")]
    Network(String),

    #[error("LFS upload failed for '{path}': {detail}")]
    LfsUploadFailed {
        path: String,
        oid: String,
        detail: String,
    },

    #[error("failed to update local tracking ref: {0}")]
    TrackingRefUpdate(String),

    #[error("failed to read repository state: {0}")]
    RepoState(String),
}

impl From<PushError> for CliError {
    fn from(error: PushError) -> Self {
        match &error {
            PushError::DetachedHead => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::RepoStateInvalid)
                .with_hint("checkout a branch before pushing")
                .with_hint("use 'libra switch <branch>' to switch"),
            PushError::NoRemoteConfigured => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::RepoStateInvalid)
                .with_hint("use 'libra remote add <name> <url>' to configure a remote")
                .with_hint("or specify the remote explicitly: 'libra push <remote> <branch>'"),
            PushError::RemoteNotFound { suggestion, .. } => {
                let mut err = CliError::fatal(error.to_string())
                    .with_stable_code(StableErrorCode::CliInvalidTarget)
                    .with_hint("use 'libra remote -v' to see configured remotes");
                if let Some(s) = suggestion {
                    err = err.with_priority_hint(format!("did you mean '{s}'?"));
                }
                err
            }
            PushError::InvalidRefspec(..) => CliError::command_usage(error.to_string())
                .with_stable_code(StableErrorCode::CliInvalidArguments)
                .with_hint("use '<name>' or '<src>:<dst>'"),
            PushError::SourceRefNotFound(..) => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::CliInvalidTarget)
                .with_hint("verify the local branch/ref exists before pushing"),
            PushError::UnsupportedLocalFileRemote => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::CliInvalidTarget)
                .with_hint(
                    "use fetch/clone for local-path repositories; push currently supports network remotes only",
                ),
            PushError::InvalidRemoteUrl { .. } => CliError::command_usage(error.to_string())
                .with_stable_code(StableErrorCode::CliInvalidArguments)
                .with_hint("check the remote URL with 'libra remote get-url <name>'"),
            PushError::AuthenticationFailed { .. } => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::AuthMissingCredentials)
                .with_hint("check SSH key or HTTP credentials")
                .with_hint("use 'libra config --list' to verify auth settings"),
            PushError::DiscoveryFailed { .. } => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::NetworkUnavailable)
                .with_hint("check the remote URL and network connectivity"),
            PushError::Timeout { .. } => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::NetworkUnavailable)
                .with_hint("check network connectivity and retry"),
            PushError::NonFastForward { .. } => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::ConflictOperationBlocked)
                .with_hint("pull and integrate remote changes first: 'libra pull'")
                .with_hint("or use --force to overwrite (data loss risk)"),
            PushError::HashKindMismatch { .. } => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::NetworkProtocol),
            PushError::ObjectCollection(..) => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::InternalInvariant)
                .with_hint(format!("this is a bug; please report it at {ISSUE_URL}")),
            PushError::PackEncoding(..) => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::InternalInvariant)
                .with_hint(format!("this is a bug; please report it at {ISSUE_URL}")),
            PushError::RemoteUnpackFailed => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::NetworkProtocol)
                .with_hint("the remote server failed to process the pack; retry or check server logs"),
            PushError::RemoteRefUpdateFailed { .. } => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::NetworkProtocol)
                .with_hint("the remote rejected the update; check branch protection rules"),
            PushError::Network(..) => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::NetworkUnavailable)
                .with_hint("check network connectivity and retry"),
            PushError::LfsUploadFailed { path, oid, .. } => {
                let mut err = CliError::fatal(error.to_string())
                    .with_stable_code(StableErrorCode::NetworkUnavailable)
                    .with_hint("check LFS endpoint configuration");
                if oid != "(unknown)" {
                    err = err.with_detail("oid", oid.clone());
                }
                if path != "(unknown)" {
                    err = err.with_detail("path", path.clone());
                }
                err
            }
            PushError::TrackingRefUpdate(..) => {
                CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::IoWriteFailed)
            }
            PushError::RepoState(..) => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::RepoCorrupt)
                .with_hint("try 'libra status' to verify repository state"),
        }
    }
}

// ---------------------------------------------------------------------------
// Structured output types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct PushRefUpdate {
    pub local_ref: String,
    pub remote_ref: String,
    pub old_oid: Option<String>,
    pub new_oid: String,
    pub forced: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PushOutput {
    /// Push target remote name
    pub remote: String,
    /// Push target URL
    pub url: String,
    /// Ref updates performed
    pub updates: Vec<PushRefUpdate>,
    /// Number of objects pushed
    pub objects_pushed: usize,
    /// Bytes of pack data pushed
    pub bytes_pushed: u64,
    /// Number of LFS files uploaded
    pub lfs_files_uploaded: usize,
    /// Whether this was a dry-run
    pub dry_run: bool,
    /// Whether everything was already up-to-date
    pub up_to_date: bool,
    /// Upstream tracking branch set (if --set-upstream)
    pub upstream_set: Option<String>,
    /// Warning messages (e.g. force push)
    pub warnings: Vec<String>,
}

// ---------------------------------------------------------------------------
// Refspec parsing
// ---------------------------------------------------------------------------

/// Parsed refspec: local source branch and remote destination branch.
struct ParsedRefspec {
    /// Local branch name to push from
    src: String,
    /// Remote branch name to push to
    dst: String,
}

/// Parse a refspec string into source and destination.
///
/// Supported forms:
/// - `<name>` — push local `<name>` to remote `<name>`
/// - `<src>:<dst>` — push local `<src>` to remote `<dst>`
///
/// Empty src or dst (e.g. `:dst`, `src:`) is not supported.
fn parse_refspec(refspec: &str) -> Result<ParsedRefspec, PushError> {
    if refspec.is_empty() {
        return Err(PushError::InvalidRefspec(refspec.to_string()));
    }

    // Only 0 or 1 colon is valid; reject multi-colon forms like "a:b:c"
    if refspec.matches(':').count() > 1 {
        return Err(PushError::InvalidRefspec(refspec.to_string()));
    }

    if let Some((src, dst)) = refspec.split_once(':') {
        if src.is_empty() || dst.is_empty() {
            return Err(PushError::InvalidRefspec(refspec.to_string()));
        }
        Ok(ParsedRefspec {
            src: src.to_string(),
            dst: dst.to_string(),
        })
    } else {
        Ok(ParsedRefspec {
            src: refspec.to_string(),
            dst: refspec.to_string(),
        })
    }
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

pub async fn execute(args: PushArgs) {
    if let Err(err) = execute_safe(args, &OutputConfig::default()).await {
        err.print_stderr();
    }
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting. Validates arguments, reads remote configuration,
/// negotiates with the server, and sends local refs and pack data.
pub async fn execute_safe(args: PushArgs, output: &OutputConfig) -> CliResult<()> {
    if args.repository.is_some() ^ args.refspec.is_some() {
        return Err(CliError::command_usage(
            "both repository and refspec should be provided",
        ));
    }
    if args.set_upstream && args.refspec.is_none() {
        return Err(CliError::command_usage(
            "--set-upstream requires a branch name",
        ));
    }

    let result = run_push(args, output).await.map_err(CliError::from)?;
    render_push_output(&result, output)
}

// ---------------------------------------------------------------------------
// Pure execution
// ---------------------------------------------------------------------------

/// Pure execution entry point. Does NOT render output — returns [`PushOutput`]
/// on success for the caller to render.
pub async fn run_push(args: PushArgs, output: &OutputConfig) -> Result<PushOutput, PushError> {
    let current_branch = match Head::current().await {
        Head::Branch(name) => name,
        Head::Detached(_) => return Err(PushError::DetachedHead),
    };
    let explicit_refspec = args.refspec.is_some();

    let repository = match args.repository {
        Some(repo) => repo,
        None => {
            let remote = ConfigKv::get_remote(&current_branch).await.ok().flatten();
            match remote {
                Some(remote) => remote,
                None => return Err(PushError::NoRemoteConfigured),
            }
        }
    };

    let repo_url = match ConfigKv::get_remote_url(&repository).await {
        Ok(url) => url,
        Err(_) => {
            // Cross-Cutting F: fuzzy match for remote name suggestion
            let suggestion = suggest_remote_name(&repository).await;
            return Err(PushError::RemoteNotFound {
                name: repository.clone(),
                suggestion,
            });
        }
    };

    // Parse refspec: supports <name> and <src>:<dst>
    let (local_branch, remote_branch) = match args.refspec {
        Some(ref refspec) => {
            let parsed = parse_refspec(refspec)?;
            (parsed.src, parsed.dst)
        }
        None => (current_branch.clone(), current_branch.clone()),
    };

    let commit_hash = match Branch::find_branch(&local_branch, None).await {
        Some(branch_info) => branch_info.commit.to_string(),
        None => return Err(PushError::SourceRefNotFound(local_branch.clone())),
    };

    // Local file path remotes are not supported for push
    if is_local_file_remote(&repo_url) {
        return Err(PushError::UnsupportedLocalFileRemote);
    }

    // Determine transport: SSH or HTTPS
    let is_ssh = is_ssh_spec(&repo_url);

    let remote_client =
        RemoteClient::from_spec_with_remote(&repo_url, Some(&repository)).map_err(|e| {
            PushError::InvalidRemoteUrl {
                url: repo_url.clone(),
                detail: e.to_string(),
            }
        })?;

    let discovery =
        tokio::time::timeout(PUSH_TIMEOUT, remote_client.discovery_reference(ReceivePack))
            .await
            .map_err(|_| PushError::Timeout {
                phase: "discovery".to_string(),
                seconds: PUSH_TIMEOUT.as_secs(),
            })?
            .map_err(|e| match e {
                GitError::UnAuthorized(_) => PushError::AuthenticationFailed {
                    url: repo_url.clone(),
                },
                GitError::NetworkError(detail) => {
                    let lower = detail.to_lowercase();
                    if lower.contains("timeout") || lower.contains("timed out") {
                        PushError::Timeout {
                            phase: "discovery".to_string(),
                            seconds: PUSH_TIMEOUT.as_secs(),
                        }
                    } else {
                        PushError::DiscoveryFailed {
                            url: repo_url.clone(),
                            detail,
                        }
                    }
                }
                other => PushError::DiscoveryFailed {
                    url: repo_url.clone(),
                    detail: other.to_string(),
                },
            })?;

    let local_kind = get_hash_kind();
    if discovery.hash_kind != local_kind {
        return Err(PushError::HashKindMismatch {
            remote: discovery.hash_kind.to_string(),
            local: local_kind.to_string(),
        });
    }
    set_wire_hash_kind(discovery.hash_kind);
    let refs = discovery.refs;

    let tracked_branch = if explicit_refspec {
        format!("refs/heads/{remote_branch}")
    } else {
        ConfigKv::get(&format!("branch.{current_branch}.merge"))
            .await
            .ok()
            .flatten()
            .map(|e| e.value)
            .unwrap_or_else(|| format!("refs/heads/{remote_branch}"))
    };

    let tracked_ref = refs.iter().find(|r| r._ref == tracked_branch);
    let remote_hash = tracked_ref
        .map(|r| r._hash.clone())
        .unwrap_or(ObjectHash::zero_str(get_hash_kind()));

    // Up-to-date check
    if remote_hash == commit_hash {
        return Ok(PushOutput {
            remote: repository.clone(),
            url: repo_url,
            updates: vec![],
            objects_pushed: 0,
            bytes_pushed: 0,
            lfs_files_uploaded: 0,
            dry_run: args.dry_run,
            up_to_date: true,
            upstream_set: None,
            warnings: vec![],
        });
    }

    // Fast-forward check
    let remote_oid = ObjectHash::from_str(&remote_hash)
        .map_err(|_| PushError::RepoState(format!("invalid remote hash: {remote_hash}")))?;
    let local_oid = ObjectHash::from_str(&commit_hash)
        .map_err(|_| PushError::RepoState(format!("invalid local hash: {commit_hash}")))?;
    let zero_oid = zero_object_hash();
    let can_fast_forward = if remote_oid == zero_oid {
        true
    } else {
        is_ancestor(&remote_oid, &local_oid)
    };

    let mut warnings = Vec::new();

    if !can_fast_forward && !args.force {
        return Err(PushError::NonFastForward {
            local_ref: local_branch.clone(),
            remote_ref: tracked_branch.clone(),
        });
    } else if !can_fast_forward && args.force {
        warnings.push("force push overwrites remote history".to_string());
    }

    let is_forced = !can_fast_forward && args.force;
    let old_oid_str = if remote_oid == zero_oid {
        None
    } else {
        Some(remote_hash.clone())
    };

    // Dry-run: compute what would be pushed but do not send
    if args.dry_run {
        let result = incremental_objs(
            ObjectHash::from_str(&commit_hash).map_err(|_| {
                PushError::ObjectCollection(format!("invalid commit hash: {commit_hash}"))
            })?,
            ObjectHash::from_str(&remote_hash).map_err(|_| {
                PushError::ObjectCollection(format!("invalid remote hash: {remote_hash}"))
            })?,
        );
        warnings.extend(result.warnings);
        return Ok(PushOutput {
            remote: repository.clone(),
            url: repo_url,
            updates: vec![PushRefUpdate {
                local_ref: format!("refs/heads/{local_branch}"),
                remote_ref: tracked_branch.clone(),
                old_oid: old_oid_str,
                new_oid: commit_hash,
                forced: is_forced,
            }],
            objects_pushed: result.objs.len(),
            bytes_pushed: 0,
            lfs_files_uploaded: 0,
            dry_run: true,
            up_to_date: false,
            upstream_set: None,
            warnings,
        });
    }

    let mut data = BytesMut::new();
    let mut capabilities = vec!["report-status"];
    if get_wire_hash_kind() == HashKind::Sha256 {
        capabilities.push("object-format=sha256");
    }
    let capability = capabilities.join(" ");
    add_pkt_line_string(
        &mut data,
        format!("{remote_hash} {commit_hash} {tracked_branch}\0{capability}\n"),
    );
    data.extend_from_slice(b"0000");
    tracing::debug!("{:?}", data);

    let obj_result = incremental_objs(
        ObjectHash::from_str(&commit_hash).map_err(|_| {
            PushError::ObjectCollection(format!("invalid commit hash: {commit_hash}"))
        })?,
        ObjectHash::from_str(&remote_hash).map_err(|_| {
            PushError::ObjectCollection(format!("invalid remote hash: {remote_hash}"))
        })?,
    );
    let objs = obj_result.objs;
    warnings.extend(obj_result.warnings);

    // Upload LFS files (only for HTTP remotes)
    let mut lfs_files_uploaded = 0;
    if !is_ssh {
        let url = Url::parse(&repo_url).map_err(|e| PushError::InvalidRemoteUrl {
            url: repo_url.clone(),
            detail: e.to_string(),
        })?;
        let lfs_client = crate::internal::protocol::lfs_client::LFSClient::from_url(&url);
        lfs_files_uploaded =
            lfs_client
                .push_objects(&objs)
                .await
                .map_err(|error| PushError::LfsUploadFailed {
                    path: error.path.unwrap_or_else(|| "(unknown)".to_string()),
                    oid: error.oid.unwrap_or_else(|| "(unknown)".to_string()),
                    detail: error.detail,
                })?;
    }

    let obj_count = objs.len();
    let (entry_tx, entry_rx) = mpsc::channel::<MetaAttached<Entry, EntryMeta>>(1_000_000);
    let (stream_tx, mut stream_rx) = mpsc::channel(1_000_000);

    let encoder = PackEncoder::new(objs.len(), 0, stream_tx);
    encoder
        .encode_async(entry_rx)
        .await
        .map_err(|e| PushError::PackEncoding(e.to_string()))?;

    let progress_output = progress_output_config(output);
    let progress = ProgressReporter::new(
        "Compressing objects",
        Some(objs.len() as u64),
        &progress_output,
    );
    for (i, obj) in objs.iter().cloned().enumerate() {
        let meta_entry = MetaAttached {
            inner: obj,
            meta: EntryMeta::default(),
        };
        if let Err(e) = entry_tx.send(meta_entry).await {
            return Err(PushError::PackEncoding(format!(
                "failed to send entry: {e}"
            )));
        }
        progress.tick((i + 1) as u64);
    }
    drop(entry_tx);
    progress.finish();

    let progress = ProgressReporter::new("Writing objects", None, &progress_output);
    let mut pack_data = Vec::new();
    while let Some(chunk) = stream_rx.recv().await {
        pack_data.extend(chunk);
        progress.tick(pack_data.len() as u64);
    }
    progress.finish();

    let bytes_pushed = pack_data.len() as u64;
    data.extend_from_slice(&pack_data);

    // Send pack via the appropriate transport (with timeout)
    match &remote_client {
        RemoteClient::Ssh(ssh_client) => {
            let response_bytes =
                tokio::time::timeout(PUSH_TIMEOUT, ssh_client.send_pack(data.freeze()))
                    .await
                    .map_err(|_| PushError::Timeout {
                        phase: "send-pack".to_string(),
                        seconds: PUSH_TIMEOUT.as_secs(),
                    })?
                    .map_err(|e| PushError::Network(format!("SSH send_pack failed: {e}")))?;
            let mut response_data = response_bytes;
            let (_, pkt_line) = read_pkt_line(&mut response_data);
            if pkt_line != "unpack ok\n" {
                return Err(PushError::RemoteUnpackFailed);
            }
            let (_, pkt_line) = read_pkt_line(&mut response_data);
            if !pkt_line.starts_with("ok".as_ref()) {
                let detail = String::from_utf8_lossy(&pkt_line).trim().to_string();
                return Err(PushError::RemoteRefUpdateFailed {
                    refname: tracked_branch.clone(),
                    reason: detail,
                });
            }
        }
        RemoteClient::Http(http_client) => {
            let res = tokio::time::timeout(PUSH_TIMEOUT, http_client.send_pack(data.freeze()))
                .await
                .map_err(|_| PushError::Timeout {
                    phase: "send-pack".to_string(),
                    seconds: PUSH_TIMEOUT.as_secs(),
                })?
                .map_err(|e| PushError::Network(format!("failed to send pack data: {e}")))?;
            if res.status() != 200 {
                return Err(PushError::Network(format!(
                    "unexpected server response (status {})",
                    res.status()
                )));
            }
            let mut data = tokio::time::timeout(PUSH_TIMEOUT, res.bytes())
                .await
                .map_err(|_| PushError::Timeout {
                    phase: "receive-pack".to_string(),
                    seconds: PUSH_TIMEOUT.as_secs(),
                })?
                .map_err(|e| PushError::Network(format!("failed to read server response: {e}")))?;
            let (_, pkt_line) = read_pkt_line(&mut data);
            if pkt_line != "unpack ok\n" {
                return Err(PushError::RemoteUnpackFailed);
            }
            let (_, pkt_line) = read_pkt_line(&mut data);
            if !pkt_line.starts_with("ok".as_ref()) {
                let detail = String::from_utf8_lossy(&pkt_line).trim().to_string();
                return Err(PushError::RemoteRefUpdateFailed {
                    refname: tracked_branch.clone(),
                    reason: detail,
                });
            }
            let (len, _) = read_pkt_line(&mut data);
            if len != 0 {
                return Err(PushError::Network(
                    "unexpected trailing data in server response".to_string(),
                ));
            }
        }
        _ => {
            return Err(PushError::UnsupportedLocalFileRemote);
        }
    }

    // Update remote tracking branch
    let remote_tracking_branch = format!("refs/remotes/{}/{}", repository, remote_branch);
    update_remote_tracking(&remote_tracking_branch, &commit_hash, &repository)
        .await
        .map_err(|e| PushError::TrackingRefUpdate(e.message().to_string()))?;

    // Set upstream if requested
    let upstream_set = if args.set_upstream {
        let upstream = format!("{repository}/{remote_branch}");
        let silent_output = silent_output_config(output);
        branch::set_upstream_safe_with_output(&local_branch, &upstream, &silent_output)
            .await
            .map_err(|e| PushError::TrackingRefUpdate(e.message().to_string()))?;
        Some(upstream)
    } else {
        None
    };

    Ok(PushOutput {
        remote: repository,
        url: repo_url,
        updates: vec![PushRefUpdate {
            local_ref: format!("refs/heads/{local_branch}"),
            remote_ref: tracked_branch,
            old_oid: old_oid_str,
            new_oid: commit_hash,
            forced: is_forced,
        }],
        objects_pushed: obj_count,
        bytes_pushed,
        lfs_files_uploaded,
        dry_run: false,
        up_to_date: false,
        upstream_set,
        warnings,
    })
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render push output according to OutputConfig (human / JSON / machine).
fn render_push_output(result: &PushOutput, output: &OutputConfig) -> CliResult<()> {
    if output.is_json() {
        return emit_json_data("push", result, output);
    }

    if result.up_to_date {
        info_println!(output, "Everything up-to-date");
        return Ok(());
    }

    if output.quiet {
        emit_push_warnings(&result.warnings);
        return Ok(());
    }

    let stdout = std::io::stdout();
    let mut w = stdout.lock();

    writeln!(w, "To {}", result.url)
        .map_err(|e| CliError::io(format!("failed to write push output: {e}")))?;

    for update in &result.updates {
        let remote_short_name = update
            .remote_ref
            .strip_prefix("refs/heads/")
            .unwrap_or(&update.remote_ref);
        let local_short_name = update
            .local_ref
            .strip_prefix("refs/heads/")
            .unwrap_or(&update.local_ref);

        match &update.old_oid {
            None => {
                if result.dry_run {
                    writeln!(
                        w,
                        " * [new branch]      {} -> {} (dry run)",
                        local_short_name, remote_short_name
                    )
                    .map_err(|e| CliError::io(format!("failed to write push output: {e}")))?;
                } else {
                    writeln!(
                        w,
                        " * [new branch]      {} -> {}",
                        local_short_name, remote_short_name
                    )
                    .map_err(|e| CliError::io(format!("failed to write push output: {e}")))?;
                }
            }
            Some(old_oid) => {
                let old_short = &old_oid[..7.min(old_oid.len())];
                let new_short = &update.new_oid[..7.min(update.new_oid.len())];
                if update.forced {
                    writeln!(
                        w,
                        " + {}...{} {} -> {} (forced update)",
                        old_short, new_short, local_short_name, remote_short_name
                    )
                    .map_err(|e| CliError::io(format!("failed to write push output: {e}")))?;
                } else if result.dry_run {
                    writeln!(
                        w,
                        "   {}..{}  {} -> {} (dry run)",
                        old_short, new_short, local_short_name, remote_short_name
                    )
                    .map_err(|e| CliError::io(format!("failed to write push output: {e}")))?;
                } else {
                    writeln!(
                        w,
                        "   {}..{}  {} -> {}",
                        old_short, new_short, local_short_name, remote_short_name
                    )
                    .map_err(|e| CliError::io(format!("failed to write push output: {e}")))?;
                }
            }
        }
    }

    if result.lfs_files_uploaded > 0 {
        let files_word = if result.lfs_files_uploaded == 1 {
            "file"
        } else {
            "files"
        };
        writeln!(
            w,
            " {} {} changed via LFS",
            result.lfs_files_uploaded, files_word
        )
        .map_err(|e| CliError::io(format!("failed to write push output: {e}")))?;
    }

    if result.objects_pushed > 0 {
        if result.dry_run {
            writeln!(w, " {} objects would be pushed", result.objects_pushed)
                .map_err(|e| CliError::io(format!("failed to write push output: {e}")))?;
        } else {
            let size_str = format_bytes(result.bytes_pushed);
            writeln!(
                w,
                " {} objects pushed ({})",
                result.objects_pushed, size_str
            )
            .map_err(|e| CliError::io(format!("failed to write push output: {e}")))?;
        }
    }

    emit_push_warnings(&result.warnings);

    // Print upstream tracking info
    if let Some(upstream) = &result.upstream_set {
        let branch_name = result
            .updates
            .first()
            .map(|u| {
                u.local_ref
                    .strip_prefix("refs/heads/")
                    .unwrap_or(&u.local_ref)
            })
            .unwrap_or("?");
        writeln!(w, "branch '{}' set up to track '{}'", branch_name, upstream)
            .map_err(|e| CliError::io(format!("failed to write push output: {e}")))?;
    }

    Ok(())
}

fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn progress_output_config(output: &OutputConfig) -> OutputConfig {
    let mut config = output.clone();
    if config.is_json() {
        config.progress = ProgressMode::None;
    }
    config
}

fn silent_output_config(output: &OutputConfig) -> OutputConfig {
    let mut config = output.clone();
    config.quiet = true;
    config.progress = ProgressMode::None;
    config
}

/// Cross-Cutting F: suggest the closest configured remote name using edit distance.
async fn suggest_remote_name(input: &str) -> Option<String> {
    let entries = ConfigKv::get_by_prefix("remote.").await.ok()?;
    let mut names: HashSet<String> = HashSet::new();
    for entry in &entries {
        // Keys look like "remote.origin.url", "remote.origin.fetch", etc.
        if let Some(rest) = entry.key.strip_prefix("remote.")
            && let Some(name) = rest.split('.').next()
        {
            names.insert(name.to_string());
        }
    }
    let mut best: Option<(String, usize)> = None;
    for name in &names {
        let dist = levenshtein(input, name);
        // Only suggest if edit distance is at most 2 and less than the input length
        if dist <= 2 && dist < input.len() && best.as_ref().is_none_or(|(_, d)| dist < *d) {
            best = Some((name.clone(), dist));
        }
    }
    best.map(|(name, _)| name)
}

fn levenshtein(a: &str, b: &str) -> usize {
    let m = a.len();
    let n = b.len();
    let mut dp = vec![vec![0usize; n + 1]; m + 1];
    for (i, row) in dp.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, val) in dp[0].iter_mut().enumerate() {
        *val = j;
    }
    for (i, ca) in a.chars().enumerate() {
        for (j, cb) in b.chars().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            dp[i + 1][j + 1] = (dp[i][j + 1] + 1)
                .min(dp[i + 1][j] + 1)
                .min(dp[i][j] + cost);
        }
    }
    dp[m][n]
}

fn emit_push_warnings(warnings: &[String]) {
    for warning in warnings {
        emit_warning(warning);
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Updates the remote tracking branch reference and records a Push action in the reflog.
///
/// This operation is performed atomically within a database transaction to keep the branch
/// pointer and reflog entry consistent.
async fn update_remote_tracking(
    remote_tracking_branch: &str,
    commit_hash: &str,
    remote_name: &str,
) -> CliResult<()> {
    let remote_tracking_branch = remote_tracking_branch.to_string();
    let commit_hash = commit_hash.to_string();
    let remote_name = remote_name.to_string();

    let db = get_db_conn_instance().await;
    let transaction_result = db
        .transaction(|txn| {
            Box::pin(async move {
                let old_oid =
                    Branch::find_branch_with_conn(txn, &remote_tracking_branch, Some(&remote_name))
                        .await
                        .map_or(ObjectHash::zero_str(get_hash_kind()).to_string(), |b| {
                            b.commit.to_string()
                        });

                Branch::update_branch_with_conn(
                    txn,
                    &remote_tracking_branch,
                    &commit_hash,
                    Some(&remote_name),
                )
                .await
                .map_err(ReflogError::from)?;

                let context = ReflogContext {
                    old_oid,
                    new_oid: commit_hash.clone(),
                    action: ReflogAction::Push,
                };
                Reflog::insert_single_entry(txn, &context, &remote_tracking_branch).await?;
                Ok::<_, ReflogError>(())
            })
        })
        .await;

    if let Err(e) = transaction_result {
        return Err(CliError::fatal(format!(
            "failed to update remote tracking branch: {}",
            e
        )));
    }
    Ok(())
}

fn is_local_file_remote(spec: &str) -> bool {
    if let Ok(url) = Url::parse(spec) {
        if url.scheme() == "file" || url.scheme().len() == 1 {
            return true;
        }
        return false;
    }
    Path::new(spec).exists()
}

/// collect all commits from `commit_id` to root commit
fn collect_history_commits(commit_id: &ObjectHash) -> HashSet<ObjectHash> {
    let zero_oid = zero_object_hash();
    if commit_id == &zero_oid {
        return HashSet::new();
    }

    let mut commits = HashSet::new();
    let mut queue = VecDeque::new();
    queue.push_back(*commit_id);
    while let Some(commit) = queue.pop_front() {
        commits.insert(commit);

        let commit = match Commit::try_load(&commit) {
            Some(c) => c,
            None => continue,
        };

        for parent in commit.parent_commit_ids.iter() {
            queue.push_back(*parent);
        }
    }
    commits
}

/// Collected objects and any warnings emitted during object traversal.
struct IncrementalObjsResult {
    objs: HashSet<Entry>,
    warnings: Vec<String>,
}

fn incremental_objs(local_ref: ObjectHash, remote_ref: ObjectHash) -> IncrementalObjsResult {
    tracing::debug!("local_ref: {}, remote_ref: {}", local_ref, remote_ref);

    let empty = IncrementalObjsResult {
        objs: HashSet::new(),
        warnings: vec![],
    };
    let mut warnings = Vec::new();

    let zero_oid = zero_object_hash();
    if remote_ref != zero_oid {
        let mut commit = match Commit::try_load(&local_ref) {
            Some(c) => c,
            None => return empty,
        };
        let mut commits = Vec::new();
        let mut ok = true;
        loop {
            commits.push(commit.id);
            if commit.id == remote_ref {
                break;
            }
            if commit.parent_commit_ids.len() != 1 {
                ok = false;
                break;
            }
            commit = match Commit::try_load(&commit.parent_commit_ids[0]) {
                Some(c) => c,
                None => {
                    ok = false;
                    break;
                }
            };
        }
        if ok {
            let mut objs = HashSet::new();
            commits.reverse();
            for i in 0..commits.len() - 1 {
                let old_commit = match Commit::try_load(&commits[i]) {
                    Some(c) => c,
                    None => {
                        tracing::error!(
                            "Commit {} became inaccessible during push (fast-forward object collection)",
                            commits[i]
                        );
                        return empty;
                    }
                };
                let old_tree = old_commit.tree_id;
                let new_commit = match Commit::try_load(&commits[i + 1]) {
                    Some(c) => c,
                    None => {
                        tracing::error!(
                            "Commit {} became inaccessible during push (fast-forward object collection)",
                            commits[i + 1]
                        );
                        return empty;
                    }
                };
                objs.extend(diff_tree_objs(
                    Some(&old_tree),
                    &new_commit.tree_id,
                    &mut warnings,
                ));
                objs.insert(new_commit.into());
            }
            return IncrementalObjsResult { objs, warnings };
        }
    }

    let mut objs = HashSet::new();
    let mut visit = HashSet::new();
    let exist_commits = collect_history_commits(&remote_ref);
    let mut queue = VecDeque::new();
    if !exist_commits.contains(&local_ref) {
        queue.push_back(local_ref);
        visit.insert(local_ref);
    }
    let mut root_commit = None;

    while let Some(commit_id) = queue.pop_front() {
        let commit = match Commit::try_load(&commit_id) {
            Some(c) => c,
            None => continue,
        };
        let parents = &commit.parent_commit_ids;
        if parents.is_empty() {
            if root_commit.is_none() {
                root_commit = Some(commit.id);
            } else if root_commit != Some(commit.id) {
                tracing::warn!("multiple root commits detected during push object collection");
            }
        }
        for parent in parents.iter() {
            let parent_commit = match Commit::try_load(parent) {
                Some(c) => c,
                None => continue,
            };
            let parent_tree = parent_commit.tree_id;
            objs.extend(diff_tree_objs(
                Some(&parent_tree),
                &commit.tree_id,
                &mut warnings,
            ));
            if !exist_commits.contains(parent) && !visit.contains(parent) {
                queue.push_back(*parent);
                visit.insert(*parent);
            }
        }
        objs.insert(commit.into());

        tracing::debug!("counting objects: {}", objs.len());
    }

    // root commit has no parent
    if let Some(root_commit) = root_commit {
        let root_tree = Commit::load(&root_commit).tree_id;
        objs.extend(diff_tree_objs(None, &root_tree, &mut warnings));
    }

    tracing::debug!("counting objects: {} done", objs.len());
    IncrementalObjsResult { objs, warnings }
}

fn zero_object_hash() -> ObjectHash {
    ObjectHash::from_bytes(&vec![0u8; get_hash_kind().size()])
        .expect("zero hash should match hash kind size")
}

/// Check if `ancestor` is an ancestor of `descendant` using breadth-first search.
///
/// Returns `true` if `ancestor` is reachable by traversing parent commits from `descendant`,
/// or if `ancestor` and `descendant` are the same commit. Returns `false` otherwise.
fn is_ancestor(ancestor: &ObjectHash, descendant: &ObjectHash) -> bool {
    if ancestor == descendant {
        return true;
    }

    let mut queue = VecDeque::new();
    let mut visited = HashSet::new();

    queue.push_back(*descendant);
    visited.insert(*descendant);

    while let Some(commit_id) = queue.pop_front() {
        if &commit_id == ancestor {
            return true;
        }

        let commit = match Commit::try_load(&commit_id) {
            Some(c) => c,
            None => continue,
        };

        for parent_id in &commit.parent_commit_ids {
            if parent_id == ancestor {
                return true;
            }
            if !visited.contains(parent_id) {
                visited.insert(*parent_id);
                queue.push_back(*parent_id);
            }
        }
    }

    false
}

/// calc objects that in `new_tree` but not in `old_tree`
///
/// Warnings (e.g. unsupported submodule entries) are collected into `warnings`
/// instead of being emitted directly, so callers can render them under
/// `OutputConfig` control without polluting stderr in JSON/machine mode.
fn diff_tree_objs(
    old_tree: Option<&ObjectHash>,
    new_tree: &ObjectHash,
    warnings: &mut Vec<String>,
) -> HashSet<Entry> {
    let mut objs = HashSet::new();
    if let Some(old_tree) = old_tree
        && old_tree == new_tree
    {
        return objs;
    }

    let new_tree = Tree::load(new_tree);
    objs.insert(new_tree.clone().into());

    let old_items = match old_tree {
        Some(tree) => {
            let tree = Tree::load(tree);
            tree.tree_items
                .iter()
                .map(|item| item.id)
                .collect::<HashSet<_>>()
        }
        None => HashSet::new(),
    };

    for item in new_tree.tree_items.iter() {
        if !old_items.contains(&item.id) {
            match item.mode {
                TreeItemMode::Tree => {
                    objs.extend(diff_tree_objs(None, &item.id, warnings));
                }
                _ => {
                    if item.mode == TreeItemMode::Commit {
                        warnings.push("submodule is not supported yet".to_string());
                    }
                    let blob = Blob::load(&item.id);
                    objs.insert(blob.into());
                }
            }
        }
    }

    objs
}

#[cfg(test)]
mod test {
    use std::str::FromStr;

    use git_internal::hash::ObjectHash;

    use super::*;

    #[test]
    /// Tests successful parsing of push command arguments with different parameter combinations.
    fn test_parse_args_success() {
        let args = vec!["push"];
        let args = PushArgs::parse_from(args);
        assert_eq!(args.repository, None);
        assert_eq!(args.refspec, None);
        assert!(!args.set_upstream);
        assert!(!args.force);
        assert!(!args.dry_run);

        let args = vec!["push", "origin", "master"];
        let args = PushArgs::parse_from(args);
        assert_eq!(args.repository, Some("origin".to_string()));
        assert_eq!(args.refspec, Some("master".to_string()));
        assert!(!args.set_upstream);
        assert!(!args.force);
        assert!(!args.dry_run);

        let args = vec!["push", "-u", "origin", "master"];
        let args = PushArgs::parse_from(args);
        assert_eq!(args.repository, Some("origin".to_string()));
        assert_eq!(args.refspec, Some("master".to_string()));
        assert!(args.set_upstream);
        assert!(!args.force);

        let args = vec!["push", "--force", "origin", "master"];
        let args = PushArgs::parse_from(args);
        assert_eq!(args.repository, Some("origin".to_string()));
        assert_eq!(args.refspec, Some("master".to_string()));
        assert!(!args.set_upstream);
        assert!(args.force);
        assert!(!args.dry_run);

        let args = vec!["push", "-f", "origin", "master"];
        let args = PushArgs::parse_from(args);
        assert_eq!(args.repository, Some("origin".to_string()));
        assert_eq!(args.refspec, Some("master".to_string()));
        assert!(!args.set_upstream);
        assert!(args.force);
        assert!(!args.dry_run);
    }

    #[test]
    /// Tests parsing of --dry-run/-n argument for push command.
    fn test_parse_dry_run_args() {
        let args = vec!["push", "--dry-run", "origin", "master"];
        let args = PushArgs::parse_from(args);
        assert!(args.dry_run);
        assert_eq!(args.repository, Some("origin".to_string()));
        assert_eq!(args.refspec, Some("master".to_string()));

        let args = vec!["push", "-n", "origin", "master"];
        let args = PushArgs::parse_from(args);
        assert!(args.dry_run);

        let args = vec!["push", "--dry-run"];
        let args = PushArgs::parse_from(args);
        assert!(args.dry_run);
        assert_eq!(args.repository, None);

        let args = vec!["push", "-n", "-f", "origin", "master"];
        let args = PushArgs::parse_from(args);
        assert!(args.dry_run);
        assert!(args.force);

        let args = vec!["push", "--dry-run", "--force", "-u", "origin", "master"];
        let args = PushArgs::parse_from(args);
        assert!(args.dry_run);
        assert!(args.force);
        assert!(args.set_upstream);
    }

    #[test]
    /// Tests failure cases for push command argument parsing.
    fn test_parse_args_fail() {
        let args = vec!["push", "-u"];
        let args = PushArgs::try_parse_from(args);
        assert!(args.is_err());

        let args = vec!["push", "-u", "origin"];
        let args = PushArgs::try_parse_from(args);
        assert!(args.is_err());

        let args = vec!["push", "-u", "master"];
        let args = PushArgs::try_parse_from(args);
        assert!(args.is_err());

        let args = vec!["push", "origin"];
        let args = PushArgs::try_parse_from(args);
        assert!(args.is_err());
    }

    #[test]
    fn test_is_ancestor() {
        let commit_id = ObjectHash::from_str("a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0").unwrap();
        assert!(is_ancestor(&commit_id, &commit_id));
    }

    #[test]
    fn test_parse_refspec_simple_name() {
        let parsed = parse_refspec("main").unwrap();
        assert_eq!(parsed.src, "main");
        assert_eq!(parsed.dst, "main");
    }

    #[test]
    fn test_parse_refspec_src_dst() {
        let parsed = parse_refspec("local_branch:release").unwrap();
        assert_eq!(parsed.src, "local_branch");
        assert_eq!(parsed.dst, "release");
    }

    #[test]
    fn test_parse_refspec_empty_rejected() {
        assert!(parse_refspec("").is_err());
    }

    #[test]
    fn test_parse_refspec_empty_src_rejected() {
        assert!(parse_refspec(":dst").is_err());
    }

    #[test]
    fn test_parse_refspec_empty_dst_rejected() {
        assert!(parse_refspec("src:").is_err());
    }

    #[test]
    fn test_parse_refspec_multi_colon_rejected() {
        assert!(parse_refspec("a:b:c").is_err());
        assert!(parse_refspec("a::b").is_err());
        assert!(parse_refspec(":a:b").is_err());
    }

    #[test]
    fn test_push_error_to_cli_error_detached_head() {
        let err: CliError = PushError::DetachedHead.into();
        assert_eq!(err.stable_code(), StableErrorCode::RepoStateInvalid);
        assert_eq!(err.exit_code(), 128);
    }

    #[test]
    fn test_push_error_to_cli_error_no_remote() {
        let err: CliError = PushError::NoRemoteConfigured.into();
        assert_eq!(err.stable_code(), StableErrorCode::RepoStateInvalid);
        assert_eq!(err.exit_code(), 128);
        assert!(!err.hints().is_empty());
    }

    #[test]
    fn test_push_error_to_cli_error_invalid_refspec() {
        let err: CliError = PushError::InvalidRefspec(":bad".to_string()).into();
        assert_eq!(err.stable_code(), StableErrorCode::CliInvalidArguments);
        assert_eq!(err.exit_code(), 129);
    }

    #[test]
    fn test_push_error_to_cli_error_non_fast_forward() {
        let err: CliError = PushError::NonFastForward {
            local_ref: "main".to_string(),
            remote_ref: "refs/heads/main".to_string(),
        }
        .into();
        assert_eq!(err.stable_code(), StableErrorCode::ConflictOperationBlocked);
        assert_eq!(err.exit_code(), 128);
    }

    #[test]
    fn test_push_error_to_cli_error_auth_failed() {
        let err: CliError = PushError::AuthenticationFailed {
            url: "https://example.com".to_string(),
        }
        .into();
        assert_eq!(err.stable_code(), StableErrorCode::AuthMissingCredentials);
    }

    #[test]
    fn test_push_error_to_cli_error_timeout() {
        let err: CliError = PushError::Timeout {
            phase: "discovery".to_string(),
            seconds: 10,
        }
        .into();
        assert_eq!(err.stable_code(), StableErrorCode::NetworkUnavailable);
    }

    #[test]
    fn test_push_error_to_cli_error_source_ref_not_found() {
        let err: CliError = PushError::SourceRefNotFound("missing-branch".to_string()).into();
        assert_eq!(err.stable_code(), StableErrorCode::CliInvalidTarget);
        assert_eq!(err.exit_code(), 129);
    }

    #[test]
    fn test_push_error_to_cli_error_unsupported_local_remote() {
        let err: CliError = PushError::UnsupportedLocalFileRemote.into();
        assert_eq!(err.stable_code(), StableErrorCode::CliInvalidTarget);
    }

    #[test]
    fn test_push_error_to_cli_error_remote_not_found() {
        let err: CliError = PushError::RemoteNotFound {
            name: "upstream".to_string(),
            suggestion: None,
        }
        .into();
        assert_eq!(err.stable_code(), StableErrorCode::CliInvalidTarget);
        assert_eq!(err.exit_code(), 129);
    }

    #[test]
    fn test_push_error_to_cli_error_remote_not_found_with_suggestion() {
        let err: CliError = PushError::RemoteNotFound {
            name: "origni".to_string(),
            suggestion: Some("origin".to_string()),
        }
        .into();
        assert_eq!(err.stable_code(), StableErrorCode::CliInvalidTarget);
        assert!(
            err.hints()
                .iter()
                .any(|h| h.as_str().contains("did you mean"))
        );
    }

    #[test]
    fn test_push_error_to_cli_error_object_collection_has_issue_url() {
        let err: CliError = PushError::ObjectCollection("test failure".to_string()).into();
        assert_eq!(err.stable_code(), StableErrorCode::InternalInvariant);
        assert!(err.hints().iter().any(|h| h.as_str().contains("issues")));
    }

    #[test]
    fn test_push_error_to_cli_error_pack_encoding_has_issue_url() {
        let err: CliError = PushError::PackEncoding("test failure".to_string()).into();
        assert_eq!(err.stable_code(), StableErrorCode::InternalInvariant);
        assert!(err.hints().iter().any(|h| h.as_str().contains("issues")));
    }

    #[test]
    fn test_levenshtein_basic() {
        assert_eq!(levenshtein("origin", "origin"), 0);
        assert_eq!(levenshtein("origni", "origin"), 2);
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("abc", ""), 3);
    }
}
