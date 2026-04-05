//! Fetch command to negotiate with remotes, download pack data, update
//! remote-tracking refs, and honor prune/depth options.

use std::{
    collections::HashSet,
    fs,
    io::{self, Error as IoError, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

use clap::Parser;
use git_internal::{
    errors::GitError,
    hash::{HashKind, ObjectHash, get_hash_kind},
    internal::object::commit::Commit,
};
use indicatif::ProgressBar;
use sea_orm::{TransactionError, TransactionTrait};
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio_util::io::StreamReader;
use url::Url;

use crate::{
    command::{index_pack, load_object},
    git_protocol::ServiceType::{self, UploadPack},
    internal::{
        branch::Branch,
        config::{ConfigKv, RemoteConfig},
        db::get_db_conn_instance,
        head::Head,
        protocol::{
            DiscRef, DiscoveryResult, FetchStream, ProtocolClient,
            git_client::GitClient,
            https_client::HttpsClient,
            local_client::LocalClient,
            set_wire_hash_kind,
            ssh_client::{SshClient, is_ssh_spec},
        },
        reflog::{HEAD, Reflog, ReflogAction, ReflogContext},
    },
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, ProgressMode, ProgressReporter, emit_json_data},
        path, util,
    },
};

const FETCH_EXAMPLES: &str = "\
EXAMPLES:
    libra fetch                            Fetch the current branch's upstream
    libra fetch origin                     Fetch from a specific remote
    libra fetch origin main                Fetch only one branch from a remote
    libra fetch --all                      Fetch every configured remote
    libra --json fetch origin              Structured JSON output for agents";

pub(crate) enum RemoteClient {
    Http(HttpsClient),
    Local(LocalClient),
    Git(GitClient),
    Ssh(SshClient),
}

impl RemoteClient {
    /// Create a `RemoteClient` from a URL spec, optionally providing the
    /// logical remote name so that vault-backed SSH keys can be resolved
    /// via `vault.ssh.<remote>.privkey`.
    pub(crate) fn from_spec_with_remote(spec: &str, remote: Option<&str>) -> Result<Self, String> {
        // Check for SSH-style URLs first (before Url::parse which doesn't handle SCP-style)
        if is_ssh_spec(spec) {
            let client = configure_ssh_client(SshClient::from_ssh_spec(spec)?, remote)?;
            return Ok(Self::Ssh(client));
        }

        if let Ok(mut url) = Url::parse(spec) {
            // Convert Windows path like "D:\test\1" to "file:///d:/test/1"
            if url.scheme().len() == 1 {
                url = Url::parse(&format!("file:///{}:{}", url.scheme(), url.path()))
                    .map_err(|_| format!("invalid Windows file url: {spec}"))?;
            }
            match url.scheme() {
                "http" | "https" => Ok(Self::Http(HttpsClient::from_url(&url))),
                "file" => {
                    let path = url
                        .to_file_path()
                        .map_err(|_| format!("invalid file url: {spec}"))?;
                    let client = LocalClient::from_path(path)
                        .map_err(|e| format!("invalid local repository '{}': {}", spec, e))?;
                    Ok(Self::Local(client))
                }
                "git" => {
                    if url.host_str().is_none() {
                        return Err(format!("invalid git url '{spec}': missing host"));
                    }
                    Ok(Self::Git(GitClient::from_url(&url)))
                }
                "ssh" => {
                    let client = configure_ssh_client(SshClient::from_ssh_spec(spec)?, remote)?;
                    Ok(Self::Ssh(client))
                }
                other => Err(format!("unsupported remote scheme '{other}'")),
            }
        } else {
            let normalized = spec.trim_end_matches('/');
            let normalized = if normalized.is_empty() && spec.starts_with('/') {
                "/"
            } else {
                normalized
            };
            let client = LocalClient::from_path(normalized)
                .map_err(|e| format!("invalid local repository '{}': {}", spec, e))?;
            Ok(Self::Local(client))
        }
    }

    pub(crate) async fn discovery_reference(
        &self,
        service: ServiceType,
    ) -> Result<DiscoveryResult, GitError> {
        match self {
            RemoteClient::Http(client) => client.discovery_reference(service).await,
            RemoteClient::Local(client) => client.discovery_reference(service).await,
            RemoteClient::Git(client) => client.discovery_reference(service).await,
            RemoteClient::Ssh(client) => client.discovery_reference(service).await,
        }
    }

    async fn fetch_objects(
        &self,
        have: &[String],
        want: &[String],
        depth: Option<usize>,
    ) -> Result<FetchStream, IoError> {
        match self {
            RemoteClient::Http(client) => client.fetch_objects(have, want, depth).await,
            RemoteClient::Local(client) => client.fetch_objects(have, want, depth).await,
            RemoteClient::Git(client) => client.fetch_objects(have, want, depth).await,
            RemoteClient::Ssh(client) => client.fetch_objects(have, want, depth).await,
        }
    }
}

const SSH_KEY_TEMP_FILE_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

fn configure_ssh_client(mut client: SshClient, remote: Option<&str>) -> Result<SshClient, String> {
    if let Err(error) = cleanup_expired_vault_ssh_temp_files() {
        tracing::warn!("failed to clean up expired SSH key temp files: {error}");
    }
    if let Some(mode) = load_ssh_host_key_checking_mode() {
        client = client.with_strict_host_key_checking(mode)?;
    }
    // Try to load vault SSH key for authentication.
    // Priority:
    // 1. vault.ssh.<remote>.privkey (vault-encrypted, decrypted to temp file)
    // 2. Legacy filesystem path ~/.libra/ssh-keys/<repo-id>/id_ed25519
    // 3. No explicit key (fall back to system default SSH agent/keys)
    if let Some(key_file) = try_load_vault_ssh_key_for_remote(remote)? {
        client = client.with_temp_key_file(key_file);
    } else if let Some(key_path) = try_load_legacy_ssh_key_path() {
        client = client.with_key_path(key_path);
    }
    Ok(client)
}

/// Try to load SSH private key for a specific remote from vault config.
///
/// Reads `vault.ssh.<remote>.privkey` from config, decrypts it, writes
/// to a secure temporary file, and keeps that file alive for the lifetime
/// of the SSH client. On abnormal process termination, the 24h GC pass will
/// clean up stale `.tmp` files under `~/.libra/tmp/`.
fn try_load_vault_ssh_key_for_remote(
    remote: Option<&str>,
) -> Result<Option<tempfile::NamedTempFile>, String> {
    let Some(remote) = remote else {
        return Ok(None);
    };

    // Only try vault key lookup inside a Libra repository.
    if crate::utils::util::try_get_storage_path(None).is_err() {
        return Ok(None);
    }

    let privkey_key = format!("vault.ssh.{remote}.privkey");
    let Some(entry) = load_config_entry_sync(&privkey_key)? else {
        return Ok(None);
    };

    if !entry.encrypted {
        return Err(format!(
            "vault SSH private key '{privkey_key}' must be encrypted"
        ));
    }

    // Decrypt the private key using the vault unseal key.
    let unseal_key = load_vault_unseal_key_sync()?
        .ok_or_else(|| format!("failed to load vault unseal key for remote '{remote}'"))?;
    let ciphertext = hex::decode(&entry.value)
        .map_err(|e| format!("failed to decode vault SSH private key '{privkey_key}': {e}"))?;
    let private_key = crate::internal::vault::decrypt_token(&unseal_key, &ciphertext)
        .map_err(|e| format!("failed to decrypt vault SSH private key '{privkey_key}': {e}"))?;

    // Write to a secure temporary file in ~/.libra/tmp/
    let tmp_dir = ensure_vault_ssh_tmp_dir()?;
    let mut tmp_file = tempfile::Builder::new()
        .prefix("ssh-key-")
        .suffix(".tmp")
        .tempfile_in(&tmp_dir)
        .map_err(|e| {
            format!(
                "failed to create temporary SSH key file in '{}': {e}",
                tmp_dir.display()
            )
        })?;
    tmp_file.write_all(private_key.as_bytes()).map_err(|e| {
        format!(
            "failed to write temporary SSH key file '{}': {e}",
            tmp_file.path().display()
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(tmp_file.path(), std::fs::Permissions::from_mode(0o600)).map_err(
            |e| {
                format!(
                    "failed to set permissions on temporary SSH key file '{}': {e}",
                    tmp_file.path().display()
                )
            },
        )?;
    }

    Ok(Some(tmp_file))
}

/// Load a full config entry (including the `encrypted` flag) synchronously.
fn load_config_entry_sync(
    dotted_key: &str,
) -> Result<Option<crate::internal::config::ConfigKvEntry>, String> {
    use crate::internal::config::ConfigKv;

    fn read_entry_sync(
        dotted_key: &str,
    ) -> Result<Option<crate::internal::config::ConfigKvEntry>, String> {
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| format!("failed to create tokio runtime for config read: {e}"))?;
        rt.block_on(ConfigKv::get(dotted_key))
            .map_err(|e| format!("failed to read config key '{dotted_key}': {e}"))
    }

    let key = dotted_key.to_string();
    match tokio::runtime::Handle::try_current() {
        Ok(_) => std::thread::scope(|s| {
            s.spawn(|| read_entry_sync(&key))
                .join()
                .map_err(|_| format!("failed to join config read thread for key '{key}'"))?
        }),
        Err(_) => read_entry_sync(&key),
    }
}

/// Load the vault unseal key synchronously.
fn load_vault_unseal_key_sync() -> Result<Option<Vec<u8>>, String> {
    fn read_unseal_key_sync() -> Result<Option<Vec<u8>>, String> {
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| format!("failed to create tokio runtime for vault read: {e}"))?;
        Ok(rt.block_on(crate::internal::vault::load_unseal_key()))
    }

    match tokio::runtime::Handle::try_current() {
        Ok(_) => std::thread::scope(|s| {
            s.spawn(read_unseal_key_sync)
                .join()
                .map_err(|_| "failed to join vault read thread".to_string())?
        }),
        Err(_) => read_unseal_key_sync(),
    }
}

fn ensure_vault_ssh_tmp_dir() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or_else(|| "cannot determine home directory".to_string())?;
    let tmp_dir = home.join(".libra").join("tmp");
    std::fs::create_dir_all(&tmp_dir).map_err(|e| {
        format!(
            "failed to create SSH temp directory '{}': {e}",
            tmp_dir.display()
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp_dir, std::fs::Permissions::from_mode(0o700)).map_err(
            |e| {
                format!(
                    "failed to set permissions on SSH temp directory '{}': {e}",
                    tmp_dir.display()
                )
            },
        )?;
    }
    Ok(tmp_dir)
}

fn cleanup_expired_vault_ssh_temp_files() -> Result<usize, String> {
    let home = match dirs::home_dir() {
        Some(home) => home,
        None => return Ok(0),
    };
    cleanup_expired_vault_ssh_temp_files_in(&home.join(".libra").join("tmp"), SystemTime::now())
}

fn cleanup_expired_vault_ssh_temp_files_in(
    tmp_dir: &Path,
    now: SystemTime,
) -> Result<usize, String> {
    if !tmp_dir.exists() {
        return Ok(0);
    }

    let entries = fs::read_dir(tmp_dir).map_err(|e| {
        format!(
            "failed to read SSH temp directory '{}': {e}",
            tmp_dir.display()
        )
    })?;

    let mut removed = 0;
    for entry in entries {
        let entry = entry.map_err(|e| {
            format!(
                "failed to iterate SSH temp directory '{}': {e}",
                tmp_dir.display()
            )
        })?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|e| format!("failed to inspect SSH temp entry '{}': {e}", path.display()))?;
        if !file_type.is_file() {
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("tmp") {
            continue;
        }

        let metadata = entry.metadata().map_err(|e| {
            format!(
                "failed to read metadata for SSH temp entry '{}': {e}",
                path.display()
            )
        })?;
        let modified = metadata.modified().map_err(|e| {
            format!(
                "failed to read modification time for SSH temp entry '{}': {e}",
                path.display()
            )
        })?;
        let age = now.duration_since(modified).unwrap_or_default();
        if age < SSH_KEY_TEMP_FILE_MAX_AGE {
            continue;
        }

        fs::remove_file(&path).map_err(|e| {
            format!(
                "failed to remove expired SSH temp file '{}': {e}",
                path.display()
            )
        })?;
        removed += 1;
    }

    Ok(removed)
}

/// Try to load SSH key from the legacy filesystem path
/// `~/.libra/ssh-keys/<repo-id>/id_ed25519`.
fn try_load_legacy_ssh_key_path() -> Option<String> {
    // Only try vault key lookup inside a Libra repository.
    if crate::utils::util::try_get_storage_path(None).is_err() {
        return None;
    }

    let repo_id = load_repo_id_sync()?;
    let home = dirs::home_dir()?;
    let key_path = home
        .join(".libra")
        .join("ssh-keys")
        .join(repo_id)
        .join("id_ed25519");

    if key_path.exists() {
        Some(key_path.to_string_lossy().to_string())
    } else {
        None
    }
}

fn load_repo_id_sync() -> Option<String> {
    load_config_sync("libra", None, "repoid")
}

/// Load host key checking mode from env/config for SSH transport.
///
/// Precedence:
/// 1) `LIBRA_SSH_STRICT_HOST_KEY_CHECKING`
/// 2) repo config `ssh.strictHostKeyChecking`
fn load_ssh_host_key_checking_mode() -> Option<String> {
    if let Ok(raw) = std::env::var("LIBRA_SSH_STRICT_HOST_KEY_CHECKING") {
        let mode = raw.trim();
        if !mode.is_empty() {
            return Some(mode.to_string());
        }
    }

    use crate::utils::util;
    if util::try_get_storage_path(None).is_err() {
        return None;
    }
    load_config_sync("ssh", None, "strictHostKeyChecking")
}

fn load_config_sync(configuration: &str, name: Option<&str>, key: &str) -> Option<String> {
    use crate::internal::config::ConfigKv;

    let dotted_key = match name {
        Some(n) => format!("{configuration}.{n}.{key}"),
        None => format!("{configuration}.{key}"),
    };

    match tokio::runtime::Handle::try_current() {
        Ok(_) => std::thread::scope(|s| {
            s.spawn(|| {
                let rt = tokio::runtime::Runtime::new().ok()?;
                rt.block_on(ConfigKv::get(&dotted_key))
                    .ok()
                    .flatten()
                    .map(|e| e.value)
            })
            .join()
            .ok()
            .flatten()
        }),
        Err(_) => {
            let rt = tokio::runtime::Runtime::new().ok()?;
            rt.block_on(ConfigKv::get(&dotted_key))
                .ok()
                .flatten()
                .map(|e| e.value)
        }
    }
}

#[derive(Parser, Debug)]
#[command(after_help = FETCH_EXAMPLES)]
pub struct FetchArgs {
    /// Repository to fetch from
    pub repository: Option<String>,

    /// Refspec to fetch, usually a branch name
    #[clap(requires("repository"))]
    pub refspec: Option<String>,

    /// Fetch all remotes.
    #[clap(long, short, conflicts_with("repository"))]
    pub all: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct FetchRefUpdate {
    pub remote_ref: String,
    pub old_oid: Option<String>,
    pub new_oid: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FetchRepositoryResult {
    pub remote: String,
    pub url: String,
    pub refs_updated: Vec<FetchRefUpdate>,
    pub objects_fetched: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct FetchOutput {
    pub all: bool,
    pub requested_remote: Option<String>,
    pub refspec: Option<String>,
    pub remotes: Vec<FetchRepositoryResult>,
}

/// Typed classification for [`FetchError::InvalidRemoteSpec`] so that callers
/// can map each sub-category to a distinct stable error code without parsing
/// the `reason` string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteSpecErrorKind {
    /// The local path does not exist.
    MissingLocalRepo,
    /// The local path exists but is not a valid libra/git repository.
    InvalidLocalRepo,
    /// The URL is syntactically malformed.
    MalformedUrl,
    /// The URL scheme is not supported (e.g. `ftp://`).
    UnsupportedScheme,
}

#[derive(thiserror::Error, Debug)]
pub enum FetchError {
    #[error("{reason}")]
    InvalidRemoteSpec {
        spec: String,
        kind: RemoteSpecErrorKind,
        reason: String,
    },
    #[error("failed to discover references from '{remote}': {source}")]
    Discovery { remote: String, source: GitError },
    #[error("remote object format '{remote}' does not match local '{local}'")]
    ObjectFormatMismatch { remote: HashKind, local: HashKind },
    #[error("remote branch {branch} not found in upstream {remote}")]
    RemoteBranchNotFound { branch: String, remote: String },
    #[error("failed to fetch objects from '{remote}': {source}")]
    FetchObjects { remote: String, source: io::Error },
    #[error("failed to read fetch stream: {source}")]
    PacketRead { source: io::Error },
    #[error("invalid packet line header '{header}'")]
    InvalidPktHeader { header: String },
    #[error("remote reported an error: {message}")]
    RemoteSideband { message: String },
    #[error("pack checksum mismatch")]
    ChecksumMismatch,
    #[error("failed to locate objects directory: {source}")]
    ObjectsDirNotFound { source: io::Error },
    #[error("failed to create pack directory '{path}': {source}")]
    PackDirCreate { path: PathBuf, source: io::Error },
    #[error("failed to write pack file '{path}': {source}")]
    PackWrite { path: PathBuf, source: io::Error },
    #[error("failed to build pack index for '{path}': {source}")]
    IndexPack { path: String, source: GitError },
    #[error("failed to update references after fetch: {message}")]
    UpdateRefs { message: String },
    #[error("failed to inspect local repository state: {message}")]
    LocalState { message: String },
}

impl From<FetchError> for CliError {
    fn from(error: FetchError) -> Self {
        match &error {
            FetchError::InvalidRemoteSpec { kind, reason, .. } => match kind {
                RemoteSpecErrorKind::MissingLocalRepo => CliError::fatal(reason.clone())
                    .with_stable_code(StableErrorCode::RepoNotFound)
                    .with_hint("check that the remote path exists"),
                RemoteSpecErrorKind::InvalidLocalRepo
                | RemoteSpecErrorKind::MalformedUrl
                | RemoteSpecErrorKind::UnsupportedScheme => CliError::command_usage(reason.clone())
                    .with_stable_code(StableErrorCode::CliInvalidTarget)
                    .with_hint("check the remote URL with 'libra remote get-url <name>'"),
            },
            FetchError::Discovery { source, .. } => {
                map_fetch_discovery_error(error.to_string(), source)
            }
            FetchError::FetchObjects { source, .. } => map_fetch_io_error(
                error.to_string(),
                source,
                StableErrorCode::NetworkUnavailable,
            )
            .with_hint("check network connectivity and retry"),
            FetchError::PacketRead { source } => {
                if is_timeout_io_error(source) {
                    CliError::fatal(error.to_string())
                        .with_stable_code(StableErrorCode::NetworkUnavailable)
                        .with_hint("check network connectivity and retry")
                } else {
                    CliError::fatal(error.to_string())
                        .with_stable_code(StableErrorCode::NetworkProtocol)
                }
            }
            FetchError::RemoteBranchNotFound { .. } => CliError::command_usage(error.to_string())
                .with_stable_code(StableErrorCode::CliInvalidTarget)
                .with_hint("verify the remote branch name and try again"),
            FetchError::ObjectFormatMismatch { .. } => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::RepoStateInvalid),
            FetchError::InvalidPktHeader { .. }
            | FetchError::RemoteSideband { .. }
            | FetchError::ChecksumMismatch
            | FetchError::IndexPack { .. } => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::NetworkProtocol),
            FetchError::ObjectsDirNotFound { .. } => {
                CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::IoReadFailed)
            }
            FetchError::PackDirCreate { .. }
            | FetchError::PackWrite { .. }
            | FetchError::UpdateRefs { .. } => {
                CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::IoWriteFailed)
            }
            FetchError::LocalState { .. } => {
                CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::RepoCorrupt)
            }
        }
    }
}

fn map_fetch_discovery_error(message: String, source: &GitError) -> CliError {
    match source {
        GitError::UnAuthorized(_) => CliError::fatal(message)
            .with_stable_code(StableErrorCode::AuthPermissionDenied)
            .with_hint("check SSH key / HTTP credentials and repository access rights"),
        GitError::NetworkError(_) => CliError::fatal(message)
            .with_stable_code(StableErrorCode::NetworkUnavailable)
            .with_hint("check network connectivity and retry"),
        GitError::IOError(error) => {
            map_fetch_io_error(message, error, StableErrorCode::NetworkUnavailable)
                .with_hint("check network connectivity and retry")
        }
        _ => CliError::fatal(message).with_stable_code(StableErrorCode::NetworkProtocol),
    }
}

fn map_fetch_io_error(
    message: String,
    error: &std::io::Error,
    default_code: StableErrorCode,
) -> CliError {
    if is_timeout_io_error(error) {
        CliError::fatal(message).with_stable_code(StableErrorCode::NetworkUnavailable)
    } else {
        CliError::fatal(message).with_stable_code(default_code)
    }
}

/// Strip embedded credentials (userinfo) from a URL before printing it to
/// the terminal.  Falls back to the original string if the URL cannot be
/// parsed (e.g. SCP-style `git@host:path`).
fn redact_url_credentials(raw: &str) -> String {
    match Url::parse(raw) {
        Ok(mut url) => {
            if !url.username().is_empty() || url.password().is_some() {
                // `set_username`/`set_password` return Err only for
                // cannot-be-a-base URLs, which have no authority – safe to
                // ignore.
                let _ = url.set_username("");
                let _ = url.set_password(None);
            }
            url.to_string()
        }
        Err(_) => raw.to_string(),
    }
}

fn is_timeout_io_error(error: &std::io::Error) -> bool {
    if error.kind() == std::io::ErrorKind::TimedOut {
        return true;
    }
    let lower = error.to_string().to_lowercase();
    lower.contains("timeout") || lower.contains("timed out")
}

pub async fn execute(args: FetchArgs) {
    if let Err(err) = execute_safe(args, &OutputConfig::default()).await {
        err.print_stderr();
    }
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting. Negotiates with remotes, downloads pack data, and
/// updates remote-tracking refs.
pub async fn execute_safe(args: FetchArgs, output: &OutputConfig) -> CliResult<()> {
    let result = run_fetch(args, output).await?;
    render_fetch_output(&result, output)
}

async fn run_fetch(args: FetchArgs, output: &OutputConfig) -> CliResult<FetchOutput> {
    tracing::debug!("`fetch` args: {:?}", args);

    let FetchArgs {
        repository,
        refspec,
        all,
    } = args;

    if all {
        let remotes = ConfigKv::all_remote_configs().await.map_err(|error| {
            CliError::fatal(format!("failed to read remote configuration: {error}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        })?;

        let mut results = Vec::with_capacity(remotes.len());
        for remote in remotes {
            results.push(
                fetch_repository_with_result(remote, None, false, None, output)
                    .await
                    .map_err(CliError::from)?,
            );
        }

        return Ok(FetchOutput {
            all: true,
            requested_remote: None,
            refspec: None,
            remotes: results,
        });
    }

    let remote = match repository {
        Some(remote) => remote,
        None => match ConfigKv::get_current_remote().await {
            Ok(Some(remote)) => remote,
            Ok(None) => {
                return Err(
                    CliError::fatal("no configured remote for the current branch")
                        .with_stable_code(StableErrorCode::RepoStateInvalid)
                        .with_hint("use 'libra remote add <name> <url>' to configure a remote"),
                );
            }
            Err(_) => {
                return Err(CliError::fatal("HEAD is detached")
                    .with_stable_code(StableErrorCode::RepoStateInvalid)
                    .with_hint("switch to a branch before fetching its upstream"));
            }
        },
    };

    let remote_config = ConfigKv::remote_config(&remote)
        .await
        .map_err(|error| {
            CliError::fatal(format!("failed to read remote configuration: {error}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        })?
        .ok_or_else(|| {
            CliError::fatal(format!("remote '{remote}' not found"))
                .with_stable_code(StableErrorCode::CliInvalidTarget)
                .with_hint("use 'libra remote -v' to inspect configured remotes")
        })?;

    let result = fetch_repository_with_result(remote_config, refspec.clone(), false, None, output)
        .await
        .map_err(CliError::from)?;

    Ok(FetchOutput {
        all: false,
        requested_remote: Some(remote),
        refspec,
        remotes: vec![result],
    })
}

fn render_fetch_output(result: &FetchOutput, output: &OutputConfig) -> CliResult<()> {
    if output.is_json() {
        return emit_json_data("fetch", result, output);
    }

    if output.quiet {
        return Ok(());
    }

    let stdout = io::stdout();
    let mut writer = stdout.lock();

    if result.remotes.is_empty() {
        writeln!(writer, "No remotes configured")
            .map_err(|error| CliError::io(format!("failed to write fetch output: {error}")))?;
        return Ok(());
    }

    for (index, remote) in result.remotes.iter().enumerate() {
        if index > 0 {
            writeln!(writer)
                .map_err(|error| CliError::io(format!("failed to write fetch output: {error}")))?;
        }

        writeln!(writer, "From {}", redact_url_credentials(&remote.url))
            .map_err(|error| CliError::io(format!("failed to write fetch output: {error}")))?;

        if remote.refs_updated.is_empty() {
            writeln!(writer, "Already up to date with '{}'", remote.remote)
                .map_err(|error| CliError::io(format!("failed to write fetch output: {error}")))?;
            continue;
        }

        for update in &remote.refs_updated {
            let ref_name = update
                .remote_ref
                .strip_prefix("refs/remotes/")
                .unwrap_or(&update.remote_ref);
            match &update.old_oid {
                None => writeln!(writer, " * [new ref]         {}", ref_name).map_err(|error| {
                    CliError::io(format!("failed to write fetch output: {error}"))
                })?,
                Some(old_oid) => {
                    let old_short = &old_oid[..7.min(old_oid.len())];
                    let new_short = &update.new_oid[..7.min(update.new_oid.len())];
                    writeln!(writer, "   {}..{}  {}", old_short, new_short, ref_name).map_err(
                        |error| CliError::io(format!("failed to write fetch output: {error}")),
                    )?;
                }
            }
        }

        writeln!(writer, " {} objects fetched", remote.objects_fetched)
            .map_err(|error| CliError::io(format!("failed to write fetch output: {error}")))?;
    }

    Ok(())
}

pub(crate) async fn discover_remote(
    remote_spec: &str,
) -> Result<(RemoteClient, DiscoveryResult), FetchError> {
    discover_remote_with_name(remote_spec, None).await
}

/// Like [`discover_remote`] but accepts an optional logical remote name
/// so that vault-backed SSH keys (`vault.ssh.<remote>.privkey`) can be
/// resolved during transport setup.
pub(crate) async fn discover_remote_with_name(
    remote_spec: &str,
    remote_name: Option<&str>,
) -> Result<(RemoteClient, DiscoveryResult), FetchError> {
    let remote_client =
        RemoteClient::from_spec_with_remote(remote_spec, remote_name).map_err(|message| {
            let (kind, reason) = classify_remote_spec_error(remote_spec, &message);
            FetchError::InvalidRemoteSpec {
                spec: remote_spec.to_string(),
                kind,
                reason,
            }
        })?;
    let discovery = remote_client
        .discovery_reference(UploadPack)
        .await
        .map_err(|source| FetchError::Discovery {
            remote: remote_spec.to_string(),
            source,
        })?;
    Ok((remote_client, discovery))
}

/// Classify a remote-spec construction failure into a typed kind and a
/// human-readable reason string.
fn classify_remote_spec_error(remote_spec: &str, message: &str) -> (RemoteSpecErrorKind, String) {
    if message.starts_with("invalid local repository") {
        let display = if remote_spec == "/" {
            "/".to_string()
        } else {
            remote_spec.trim_end_matches('/').to_string()
        };
        let lower = message.to_ascii_lowercase();
        if lower.contains("no such file or directory")
            || lower.contains("does not exist")
            || lower.contains("not found")
        {
            return (
                RemoteSpecErrorKind::MissingLocalRepo,
                format!("repository '{}' does not exist", display),
            );
        }
        return (
            RemoteSpecErrorKind::InvalidLocalRepo,
            format!("'{}' does not appear to be a libra repository", display),
        );
    }
    let lower = message.to_ascii_lowercase();
    if lower.contains("unsupported") && lower.contains("scheme") {
        return (RemoteSpecErrorKind::UnsupportedScheme, message.to_string());
    }
    // Default to MalformedUrl for other spec errors (bad syntax, etc.)
    (RemoteSpecErrorKind::MalformedUrl, message.to_string())
}

pub(crate) fn normalize_branch_ref(branch: &str) -> String {
    if branch.starts_with("refs/") {
        branch.to_string()
    } else {
        format!("refs/heads/{branch}")
    }
}

pub(crate) fn remote_has_branch(refs: &[DiscRef], branch: &str) -> bool {
    let normalized = normalize_branch_ref(branch);
    refs.iter().any(|reference| reference._ref == normalized)
}

pub(crate) fn normalize_remote_url(remote_input: &str, remote_client: &RemoteClient) -> String {
    match remote_client {
        RemoteClient::Http(_) | RemoteClient::Git(_) | RemoteClient::Ssh(_) => {
            remote_input.to_string()
        }
        RemoteClient::Local(client) => client.repo_path().to_string_lossy().to_string(),
    }
}

/// Fetch from remote repository
/// - `branch` is optional, if `None`, fetch all branches
/// - `single_branch` is bool, if `true`, fetch only the specified branch
/// - `depth` is optional, if `Some(n)`, create a shallow clone with history truncated to n commits
pub async fn fetch_repository(
    remote_config: RemoteConfig,
    branch: Option<String>,
    single_branch: bool,
    depth: Option<usize>,
) {
    if let Err(err) = fetch_repository_safe(
        remote_config,
        branch,
        single_branch,
        depth,
        &OutputConfig::default(),
    )
    .await
    {
        CliError::from(err).print_stderr();
    }
}

pub async fn fetch_repository_safe(
    remote_config: RemoteConfig,
    branch: Option<String>,
    single_branch: bool,
    depth: Option<usize>,
    output: &OutputConfig,
) -> Result<(), FetchError> {
    fetch_repository_with_result(remote_config, branch, single_branch, depth, output)
        .await
        .map(|_| ())
}

pub(crate) async fn fetch_repository_with_result(
    remote_config: RemoteConfig,
    branch: Option<String>,
    single_branch: bool,
    depth: Option<usize>,
    output: &OutputConfig,
) -> Result<FetchRepositoryResult, FetchError> {
    let (remote_client, discovery) =
        discover_remote_with_name(&remote_config.url, Some(&remote_config.name)).await?;
    let normalized_url = normalize_remote_url(&remote_config.url, &remote_client);
    let local_kind = get_hash_kind();
    if discovery.hash_kind != local_kind {
        return Err(FetchError::ObjectFormatMismatch {
            remote: discovery.hash_kind,
            local: local_kind,
        });
    }
    set_wire_hash_kind(discovery.hash_kind);

    if let Some(branch_name) = &branch
        && !remote_has_branch(&discovery.refs, branch_name)
    {
        return Err(FetchError::RemoteBranchNotFound {
            branch: branch_name.clone(),
            remote: remote_config.name.clone(),
        });
    }

    let mut refs = discovery.refs.clone();
    if refs.is_empty() {
        tracing::debug!("fetch skipped because remote has no refs");
        return Ok(FetchRepositoryResult {
            remote: remote_config.name,
            url: normalized_url,
            refs_updated: Vec::new(),
            objects_fetched: 0,
        });
    }

    let remote_head = refs
        .iter()
        .find(|reference| reference._ref == HEAD)
        .cloned();
    let ref_heads = refs
        .iter()
        .filter(|reference| reference._ref.starts_with("refs/heads/"))
        .cloned()
        .collect::<Vec<_>>();

    if let Some(branch_name) = &branch
        && single_branch
    {
        let normalized = normalize_branch_ref(branch_name);
        refs.retain(|reference| reference._ref == normalized);
    }

    let want = refs
        .iter()
        .map(|reference| reference._hash.clone())
        .collect::<Vec<_>>();
    let have = current_have_safe().await?;
    let mut result_stream = remote_client
        .fetch_objects(&have, &want, depth)
        .await
        .map_err(|source| FetchError::FetchObjects {
            remote: remote_config.url.clone(),
            source,
        })?;

    let task = format!("fetch {}", remote_config.name);
    let pack_data = read_fetch_stream(&mut result_stream, output, &task).await?;
    let objects_fetched = pack_object_count(&pack_data);
    let pack_file = write_pack_and_index(&pack_data)?;
    if let Some(pack_file) = pack_file {
        let index_version = match get_hash_kind() {
            HashKind::Sha1 => None,
            HashKind::Sha256 => Some(2),
        };
        match index_version {
            Some(2) => index_pack::build_index_v2(&pack_file, &pack_file.replace(".pack", ".idx"))
                .map_err(|source| FetchError::IndexPack {
                    path: pack_file.clone(),
                    source,
                })?,
            _ => index_pack::build_index_v1(&pack_file, &pack_file.replace(".pack", ".idx"))
                .map_err(|source| FetchError::IndexPack {
                    path: pack_file.clone(),
                    source,
                })?,
        }
    }

    let refs_updated =
        update_references(&remote_config, &refs, &ref_heads, remote_head, branch).await?;
    Ok(FetchRepositoryResult {
        remote: remote_config.name,
        url: normalized_url,
        refs_updated,
        objects_fetched,
    })
}

async fn read_fetch_stream(
    result_stream: &mut FetchStream,
    output: &OutputConfig,
    task: &str,
) -> Result<Vec<u8>, FetchError> {
    let mut reader = StreamReader::new(result_stream);
    let mut pack_data = Vec::new();
    let mut reach_pack = false;
    let render_progress = matches!(output.progress, ProgressMode::Text);
    let json_progress = matches!(output.progress, ProgressMode::Json);
    let bar = render_progress.then(ProgressBar::new_spinner);
    let progress = json_progress.then(|| ProgressReporter::new(task, None, output));
    let time = Instant::now();

    loop {
        let (len, data) = read_pkt_line(&mut reader)
            .await
            .map_err(|source| FetchError::PacketRead { source })?;
        if len == 0 {
            break;
        }
        if data.len() >= 5 && data[0] == 1 && &data[1..5] == b"PACK" {
            reach_pack = true;
        }

        if reach_pack {
            if let Some((&code, payload)) = data.split_first() {
                match code {
                    1 => {
                        let bytes_per_sec = pack_data.len() as f64 / time.elapsed().as_secs_f64();
                        let total = util::auto_unit_bytes(pack_data.len() as u64);
                        let bps = util::auto_unit_bytes(bytes_per_sec as u64);
                        if let Some(bar) = &bar {
                            bar.set_message(format!("Receiving objects: {total:.2} | {bps:.2}/s"));
                            bar.tick();
                        }
                        pack_data.extend(payload);
                        if let Some(progress) = &progress {
                            progress.tick(pack_data.len() as u64);
                        }
                    }
                    2 => print_remote_progress(payload, render_progress),
                    3 => {
                        if let Some(bar) = &bar {
                            bar.finish_and_clear();
                        }
                        return Err(FetchError::RemoteSideband {
                            message: String::from_utf8_lossy(payload).trim().to_string(),
                        });
                    }
                    _ => {
                        tracing::debug!("ignoring unknown side-band code {code}");
                    }
                }
            }
        } else if data != b"NAK\n"
            && let Some((&code, payload)) = data.split_first()
        {
            match code {
                2 => print_remote_progress(payload, render_progress),
                3 => {
                    if let Some(bar) = &bar {
                        bar.finish_and_clear();
                    }
                    return Err(FetchError::RemoteSideband {
                        message: String::from_utf8_lossy(payload).trim().to_string(),
                    });
                }
                _ => {
                    if render_progress {
                        let text = String::from_utf8_lossy(&data);
                        eprint!("{text}");
                        let _ = io::stderr().flush();
                    }
                }
            }
        }
    }
    if let Some(bar) = &bar {
        bar.finish_and_clear();
    }
    if let Some(progress) = &progress {
        progress.finish();
    }

    Ok(pack_data)
}

fn print_remote_progress(payload: &[u8], render_progress: bool) {
    if render_progress {
        let text = String::from_utf8_lossy(payload);
        eprint!("{text}");
        let _ = io::stderr().flush();
    }
}

fn pack_object_count(pack_data: &[u8]) -> usize {
    if pack_data.len() < 12 || &pack_data[..4] != b"PACK" {
        return 0;
    }
    let mut count = [0u8; 4];
    count.copy_from_slice(&pack_data[8..12]);
    u32::from_be_bytes(count) as usize
}

fn write_pack_and_index(pack_data: &[u8]) -> Result<Option<String>, FetchError> {
    let hash_len = get_hash_kind().size();
    if pack_data.len() < hash_len {
        tracing::debug!("No pack data returned from remote");
        return Ok(None);
    }

    let payload_len = pack_data.len() - hash_len;
    let hash = ObjectHash::new(&pack_data[..payload_len]);
    let checksum = ObjectHash::from_bytes(&pack_data[payload_len..])
        .map_err(|_| FetchError::ChecksumMismatch)?;
    if hash != checksum {
        return Err(FetchError::ChecksumMismatch);
    }

    if pack_data.len() <= 12 + hash_len {
        tracing::debug!("Empty pack file");
        return Ok(None);
    }

    let pack_dir = path::try_objects()
        .map_err(|source| FetchError::ObjectsDirNotFound { source })?
        .join("pack");
    fs::create_dir_all(&pack_dir).map_err(|source| FetchError::PackDirCreate {
        path: pack_dir.clone(),
        source,
    })?;

    let checksum = checksum.to_string();
    let pack_file = pack_dir.join(format!("pack-{checksum}.pack"));
    let mut file = fs::File::create(&pack_file).map_err(|source| FetchError::PackWrite {
        path: pack_file.clone(),
        source,
    })?;
    file.write_all(pack_data)
        .map_err(|source| FetchError::PackWrite {
            path: pack_file.clone(),
            source,
        })?;

    Ok(Some(pack_file.to_string_lossy().into_owned()))
}

async fn update_references(
    remote_config: &RemoteConfig,
    refs: &[DiscRef],
    ref_heads: &[DiscRef],
    remote_head: Option<DiscRef>,
    branch: Option<String>,
) -> Result<Vec<FetchRefUpdate>, FetchError> {
    let db = get_db_conn_instance().await;
    let remote_config = remote_config.clone();
    let refs = refs.to_vec();
    let ref_heads = ref_heads.to_vec();
    db.transaction(|txn| {
        Box::pin(async move {
            let mut updates = Vec::new();
            for reference in &refs {
                let full_ref_name: String;
                if let Some(branch_name) = reference._ref.strip_prefix("refs/heads/") {
                    full_ref_name = format!("refs/remotes/{}/{}", remote_config.name, branch_name);
                } else if let Some(mr_name) = reference._ref.strip_prefix("refs/mr/") {
                    full_ref_name = format!("refs/remotes/{}/mr/{}", remote_config.name, mr_name);
                } else {
                    tracing::debug!(
                        "Skipping unsupported ref type during fetch: {}",
                        reference._ref
                    );
                    continue;
                }

                let old_oid = Branch::find_branch_result_with_conn(
                    txn,
                    &full_ref_name,
                    Some(&remote_config.name),
                )
                .await
                .map_err(|error| FetchError::UpdateRefs {
                    message: format!(
                        "failed to inspect existing remote-tracking ref '{full_ref_name}': {error}"
                    ),
                })?
                .map(|branch| branch.commit.to_string());

                if old_oid.as_deref() == Some(reference._hash.as_str()) {
                    continue;
                }

                Branch::update_branch_with_conn(
                    txn,
                    &full_ref_name,
                    &reference._hash,
                    Some(&remote_config.name),
                )
                .await
                .map_err(|source| FetchError::UpdateRefs {
                    message: format!(
                        "failed to persist remote-tracking ref '{full_ref_name}': {source}"
                    ),
                })?;

                let context = ReflogContext {
                    old_oid: old_oid
                        .clone()
                        .unwrap_or_else(|| ObjectHash::zero_str(get_hash_kind()).to_string()),
                    new_oid: reference._hash.clone(),
                    action: ReflogAction::Fetch,
                };
                Reflog::insert_single_entry(txn, &context, &full_ref_name)
                    .await
                    .map_err(|source| FetchError::UpdateRefs {
                        message: format!(
                            "failed to record reflog for remote-tracking ref '{full_ref_name}': {source}"
                        ),
                    })?;
                updates.push(FetchRefUpdate {
                    remote_ref: full_ref_name,
                    old_oid,
                    new_oid: reference._hash.clone(),
                });
            }

            // Determine the remote default branch.
            // When the remote HEAD is advertised, match it by hash against fetched
            // branches.  When it is absent (e.g. the remote HEAD symref points to
            // an unborn branch), fall back to the first available branch, preferring
            // "main" then "master" – mirroring the heuristic used by git itself.
            let resolved_remote_head: Option<&str> = if let Some(ref remote_head) = remote_head {
                ref_heads
                    .iter()
                    .find(|reference| reference._hash == remote_head._hash)
                    .and_then(|r| r._ref.strip_prefix("refs/heads/"))
            } else {
                None
            };

            let remote_default_branch = resolved_remote_head.map(str::to_owned).or_else(|| {
                if ref_heads.is_empty() {
                    return None;
                }
                ref_heads
                    .iter()
                    .find(|r| r._ref == "refs/heads/main")
                    .or_else(|| ref_heads.iter().find(|r| r._ref == "refs/heads/master"))
                    .or(ref_heads.first())
                    .and_then(|r| r._ref.strip_prefix("refs/heads/"))
                    .map(str::to_owned)
            });

            if let Some(branch_name) = remote_default_branch {
                Head::update_with_conn(txn, Head::Branch(branch_name), Some(&remote_config.name))
                    .await;
            } else if branch.is_none() && remote_head.is_some() {
                tracing::debug!("remote HEAD does not point to a branch ref");
            }

            Ok::<_, FetchError>(updates)
        })
    })
    .await
    .map_err(|source| FetchError::UpdateRefs {
        message: match source {
            TransactionError::Connection(error) => error.to_string(),
            TransactionError::Transaction(error) => error.to_string(),
        },
    })
}

async fn current_have_safe() -> Result<Vec<String>, FetchError> {
    #[derive(PartialEq, Eq, PartialOrd, Ord)]
    struct QueueItem {
        priority: usize,
        commit: ObjectHash,
    }

    let mut c_pending = std::collections::BinaryHeap::new();
    let mut inserted = HashSet::new();
    let check_and_insert =
        |commit: &Commit,
         inserted: &mut HashSet<String>,
         c_pending: &mut std::collections::BinaryHeap<QueueItem>| {
            if inserted.contains(&commit.id.to_string()) {
                return;
            }
            inserted.insert(commit.id.to_string());
            c_pending.push(QueueItem {
                priority: commit.committer.timestamp,
                commit: commit.id,
            });
        };

    let mut remotes = ConfigKv::all_remote_configs()
        .await
        .map_err(|source| FetchError::LocalState {
            message: format!("failed to read remote configuration: {source}"),
        })?
        .iter()
        .map(|remote| Some(remote.name.to_owned()))
        .collect::<Vec<_>>();
    remotes.push(None);

    for remote in remotes {
        let branches = Branch::list_branches_result(remote.as_deref())
            .await
            .map_err(|source| FetchError::LocalState {
                message: format!("failed to list local branches: {source}"),
            })?;
        for branch in branches {
            let commit: Commit =
                load_object(&branch.commit).map_err(|source| FetchError::LocalState {
                    message: format!(
                        "failed to load local commit '{}': {}",
                        branch.commit, source
                    ),
                })?;
            check_and_insert(&commit, &mut inserted, &mut c_pending);
        }
    }

    let mut have = Vec::new();
    while have.len() < 32 && !c_pending.is_empty() {
        let Some(item) = c_pending.pop() else {
            break;
        };
        have.push(item.commit.to_string());

        let commit: Commit =
            load_object(&item.commit).map_err(|source| FetchError::LocalState {
                message: format!("failed to load local commit '{}': {}", item.commit, source),
            })?;
        for parent in commit.parent_commit_ids {
            let parent_commit: Commit =
                load_object(&parent).map_err(|source| FetchError::LocalState {
                    message: format!("failed to load parent commit '{}': {}", parent, source),
                })?;
            check_and_insert(&parent_commit, &mut inserted, &mut c_pending);
        }
    }

    Ok(have)
}

/// Read 4 bytes hex number
async fn read_hex_4(reader: &mut (impl AsyncRead + Unpin)) -> io::Result<u32> {
    let mut buf = [0u8; 4];
    reader.read_exact(&mut buf).await?;
    let hex_str = std::str::from_utf8(&buf).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "invalid packet line header '{}'",
                String::from_utf8_lossy(&buf)
            ),
        )
    })?;
    u32::from_str_radix(hex_str, 16).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid packet line header '{hex_str}'"),
        )
    })
}

/// async version of `read_pkt_line`
/// - return (raw length, data)
async fn read_pkt_line(reader: &mut (impl AsyncRead + Unpin)) -> io::Result<(usize, Vec<u8>)> {
    let len = read_hex_4(reader).await?;
    if len == 0 {
        return Ok((0, Vec::new()));
    }
    let mut data = vec![0u8; (len - 4) as usize];
    reader.read_exact(&mut data).await?;
    Ok((len as usize, data))
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{Duration, SystemTime},
    };

    use tempfile::tempdir;

    use super::{
        FetchError, SSH_KEY_TEMP_FILE_MAX_AGE, cleanup_expired_vault_ssh_temp_files_in,
        ensure_vault_ssh_tmp_dir,
    };
    use crate::utils::test::ScopedEnvVar;

    #[test]
    fn test_cleanup_expired_vault_ssh_temp_files_removes_old_tmp_files() {
        let temp_home = tempdir().expect("failed to create temp home");
        let tmp_dir = temp_home.path().join(".libra").join("tmp");
        fs::create_dir_all(&tmp_dir).expect("failed to create SSH temp dir");

        let expired = tmp_dir.join("ssh-key-old.tmp");
        fs::write(&expired, "secret").expect("failed to write expired temp file");

        let removed = cleanup_expired_vault_ssh_temp_files_in(
            &tmp_dir,
            SystemTime::now() + SSH_KEY_TEMP_FILE_MAX_AGE + Duration::from_secs(1),
        )
        .expect("cleanup should succeed");

        assert_eq!(removed, 1);
        assert!(!expired.exists(), "expired temp file should be removed");
    }

    #[test]
    fn test_cleanup_expired_vault_ssh_temp_files_keeps_fresh_and_non_tmp_files() {
        let temp_home = tempdir().expect("failed to create temp home");
        let tmp_dir = temp_home.path().join(".libra").join("tmp");
        fs::create_dir_all(&tmp_dir).expect("failed to create SSH temp dir");

        let fresh = tmp_dir.join("ssh-key-fresh.tmp");
        let keep = tmp_dir.join("note.txt");
        fs::write(&fresh, "secret").expect("failed to write fresh temp file");
        fs::write(&keep, "keep").expect("failed to write non-temp file");

        let removed = cleanup_expired_vault_ssh_temp_files_in(&tmp_dir, SystemTime::now())
            .expect("cleanup should succeed");

        assert_eq!(removed, 0);
        assert!(fresh.exists(), "fresh temp file should remain");
        assert!(keep.exists(), "non-temp file should remain");
    }

    #[test]
    fn test_ensure_vault_ssh_tmp_dir_uses_home_directory() {
        let temp_home = tempdir().expect("failed to create temp home");
        let _home = ScopedEnvVar::set("HOME", temp_home.path());
        let _userprofile = ScopedEnvVar::set("USERPROFILE", temp_home.path());

        let tmp_dir = ensure_vault_ssh_tmp_dir().expect("tmp dir should be created");

        assert_eq!(tmp_dir, temp_home.path().join(".libra").join("tmp"));
        assert!(tmp_dir.exists(), "tmp dir should exist");
    }

    #[test]
    fn test_update_refs_branch_lookup_error_is_preserved_in_message() {
        let error = FetchError::UpdateRefs {
            message: format!(
                "failed to inspect existing remote-tracking ref 'refs/remotes/origin/main': {}",
                crate::internal::branch::BranchStoreError::Corrupt {
                    name: "refs/remotes/origin/main".to_string(),
                    detail: "invalid object id".to_string(),
                }
            ),
        };

        assert!(
            error
                .to_string()
                .contains("stored branch reference 'refs/remotes/origin/main' is corrupt"),
            "unexpected fetch error: {error}"
        );
    }
}
