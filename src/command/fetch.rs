//! Fetch command to negotiate with remotes, download pack data, update
//! remote-tracking refs, and honor prune/depth options.

use std::{
    collections::{BTreeSet, HashSet},
    fs,
    io::{self, Error as IoError, Write},
    path::{Path, PathBuf},
    str::FromStr,
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
        config::{ConfigKv, ConfigKvEntry, RemoteConfig},
        db::get_db_conn_instance,
        head::Head,
        log::date_parser::parse_date,
        protocol::{
            DiscRef, DiscoveryResult, FetchStream, ProtocolClient, ShallowOptions,
            git_client::GitClient,
            https_client::HttpsClient,
            local_client::LocalClient,
            set_wire_hash_kind,
            ssh_client::{SshClient, is_ssh_spec},
        },
        reflog::{HEAD, Reflog, ReflogAction, ReflogContext},
        vault::{decrypt_token, load_unseal_key},
    },
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, ProgressMode, ProgressReporter, emit_json_data},
        path, util,
        util::try_get_storage_path,
    },
};

const FETCH_EXAMPLES: &str = "\
EXAMPLES:
    libra fetch                            Fetch the current branch's upstream
    libra fetch origin                     Fetch from a specific remote
    libra fetch origin main                Fetch only one branch from a remote
    libra fetch --all                      Fetch every configured remote
    libra fetch origin --depth 1           Shallow fetch (latest commit only)
    libra fetch --all --depth 3            Shallow fetch across all remotes
    libra fetch origin --unshallow         Convert a shallow clone to full history
    libra fetch origin --prune             Drop tracking refs deleted on the remote
    libra fetch origin --dry-run           Preview updates without fetching objects
    libra fetch origin --porcelain         Machine-readable per-ref output
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
        shallow: &[String],
        options: &ShallowOptions,
    ) -> Result<FetchStream, IoError> {
        match self {
            RemoteClient::Http(client) => client.fetch_objects(have, want, shallow, options).await,
            RemoteClient::Local(client) => client.fetch_objects(have, want, shallow, options).await,
            RemoteClient::Git(client) => client.fetch_objects(have, want, shallow, options).await,
            RemoteClient::Ssh(client) => client.fetch_objects(have, want, shallow, options).await,
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
    if try_get_storage_path(None).is_err() {
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
    let private_key = decrypt_token(&unseal_key, &ciphertext)
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
fn load_config_entry_sync(dotted_key: &str) -> Result<Option<ConfigKvEntry>, String> {
    use crate::internal::config::ConfigKv;

    fn read_entry_sync(dotted_key: &str) -> Result<Option<ConfigKvEntry>, String> {
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
        Ok(rt.block_on(load_unseal_key()))
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

fn resolve_home_directory() -> Result<PathBuf, String> {
    #[cfg(windows)]
    let env_keys = ["USERPROFILE", "HOME"];
    #[cfg(not(windows))]
    let env_keys = ["HOME", "USERPROFILE"];

    for key in env_keys {
        if let Some(value) = std::env::var_os(key)
            && !value.is_empty()
        {
            return Ok(PathBuf::from(value));
        }
    }

    dirs::home_dir().ok_or_else(|| "cannot determine home directory".to_string())
}

fn ensure_vault_ssh_tmp_dir() -> Result<PathBuf, String> {
    let home = resolve_home_directory()?;
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
    if try_get_storage_path(None).is_err() {
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

    /// Limit fetching to the specified number of commits from the tip of each remote branch
    #[clap(long, value_name = "N")]
    pub depth: Option<usize>,

    /// Deepen the history of a shallow repository by N commits beyond the current
    /// boundary (only meaningful when the repository was cloned with `--depth`).
    #[clap(long, value_name = "N", conflicts_with = "unshallow")]
    pub deepen: Option<usize>,

    /// Convert a shallow repository into a complete one by fetching all missing
    /// history, then removing the `.libra/shallow` boundary file.
    #[clap(long, conflicts_with = "depth")]
    pub unshallow: bool,

    /// Declined: Libra does not manage submodules with the stock Git layout.
    /// Declared (rather than left unknown) so any value yields a friendly usage
    /// error (exit 129) instead of a clap parse error (exit 2).
    #[clap(long = "recurse-submodules", value_name = "MODE", num_args = 0..=1, require_equals = true)]
    pub recurse_submodules: Option<Option<String>>,

    /// Print a machine-readable, single-space-separated line per ref update:
    /// `<flag> <old-oid> <new-oid> <local-ref>`. Mutually exclusive with `--json`.
    #[clap(long)]
    pub porcelain: bool,

    /// Deepen or shape shallow history to commits more recent than <date>
    /// (`deepen-since`). Accepts the date formats `libra log --since` understands.
    #[clap(
        long = "shallow-since",
        value_name = "DATE",
        conflicts_with = "unshallow"
    )]
    pub shallow_since: Option<String>,

    /// Deepen or shape shallow history to exclude commits reachable from <ref>
    /// (`deepen-not`); repeatable.
    #[clap(
        long = "shallow-exclude",
        value_name = "REF",
        conflicts_with = "unshallow"
    )]
    pub shallow_exclude: Vec<String>,

    /// After fetching, delete local `refs/remotes/<remote>/*` tracking branches
    /// that no longer exist on the remote. Can also be enabled via `fetch.prune`.
    #[clap(long, short = 'p')]
    pub prune: bool,

    /// Show what would be fetched/pruned without downloading objects or writing
    /// any refs, reflog, or shallow metadata.
    #[clap(long = "dry-run")]
    pub dry_run: bool,

    /// Append fetched ref records to `.libra/FETCH_HEAD` instead of overwriting
    /// it. Long-only: `-a` is reserved for `--all` (Git's `-a` is `--append`).
    #[clap(long)]
    pub append: bool,

    /// Print extra diagnostics (the remote being contacted) to stderr, leaving
    /// the stdout result contract unchanged.
    #[clap(long, short = 'v')]
    pub verbose: bool,

    /// Fetch all tags from the remote into the global `refs/tags/*` namespace
    /// (in addition to branches), pulling each tag's object into the pack.
    /// Overrides the `remote.<name>.tagOpt` configuration. Existing local tags
    /// are preserved (tags are immutable by default).
    #[clap(long, short = 't', conflicts_with = "no_tags")]
    pub tags: bool,

    /// Do not import any tags, overriding `remote.<name>.tagOpt`. Long-only:
    /// Git's `-n` short form is intentionally not exposed (it stays free).
    #[clap(long = "no-tags")]
    pub no_tags: bool,

    /// Allow non-fast-forward updates: overwrite an existing local tag with the
    /// remote's value (tags are otherwise immutable). Remote-tracking refs are
    /// always updated regardless of this flag.
    #[clap(long, short = 'f')]
    pub force: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, Serialize)]
pub struct FetchRefUpdate {
    pub remote_ref: String,
    pub old_oid: Option<String>,
    pub new_oid: String,
    /// True when this update overwrote a non-fast-forward target that required
    /// `--force` (currently a clobbered existing tag); drives the porcelain `+`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub forced: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct FetchRepositoryResult {
    pub remote: String,
    pub url: String,
    pub refs_updated: Vec<FetchRefUpdate>,
    pub objects_fetched: usize,
    /// Local `refs/remotes/<remote>/*` tracking branches removed by `--prune`
    /// (full ref names). Empty unless pruning ran.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pruned: Vec<String>,
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
///
/// For SSH URLs, a bare username without a password (e.g. `git@`) is the
/// standard convention and is NOT redacted.  Only URLs that carry a password
/// component or an HTTP(S) username (which is typically a token) are stripped.
pub(crate) fn redact_url_credentials(raw: &str) -> String {
    match Url::parse(raw) {
        Ok(mut url) => {
            let raw_userinfo = url_userinfo(raw);
            let has_password = url.password().is_some()
                || raw_userinfo.is_some_and(|userinfo| userinfo.contains(':'));
            let is_http = matches!(url.scheme(), "http" | "https");
            let has_http_username =
                is_http && (!url.username().is_empty() || raw_userinfo.is_some());
            // Redact when there is a password (always sensitive) or when the
            // scheme is HTTP(S) and a username is present (likely a token).
            // For SSH, a bare username like "git" is conventional and harmless.
            if has_password || has_http_username {
                let _ = url.set_username("");
                let _ = url.set_password(None);
                return strip_url_userinfo(url.as_str()).unwrap_or_else(|| url.to_string());
            }
            url.to_string()
        }
        Err(_) => {
            let raw_userinfo = url_userinfo(raw);
            let is_http = url_scheme(raw).is_some_and(|scheme| {
                scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")
            });
            if raw_userinfo.is_some_and(|userinfo| userinfo.contains(':'))
                || (is_http && raw_userinfo.is_some())
            {
                strip_url_userinfo(raw).unwrap_or_else(|| raw.to_string())
            } else {
                raw.to_string()
            }
        }
    }
}

fn url_scheme(url: &str) -> Option<&str> {
    url.find("://").map(|scheme_end| &url[..scheme_end])
}

fn url_userinfo(url: &str) -> Option<&str> {
    let authority_start = url.find("://")? + 3;
    let authority_len = url[authority_start..]
        .find(['/', '?', '#'])
        .unwrap_or(url.len() - authority_start);
    let authority_end = authority_start + authority_len;
    let userinfo_end = url[authority_start..authority_end].rfind('@')?;

    Some(&url[authority_start..authority_start + userinfo_end])
}

fn strip_url_userinfo(url: &str) -> Option<String> {
    let authority_start = url.find("://")? + 3;
    let authority_len = url[authority_start..]
        .find(['/', '?', '#'])
        .unwrap_or(url.len() - authority_start);
    let authority_end = authority_start + authority_len;
    let userinfo_end = url[authority_start..authority_end].rfind('@')?;
    let host_start = authority_start + userinfo_end + 1;
    if host_start == authority_end {
        return None;
    }

    let mut redacted = String::with_capacity(url.len());
    redacted.push_str(&url[..authority_start]);
    redacted.push_str(&url[host_start..]);
    Some(redacted)
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
/// errors and exiting.
///
/// # Side Effects
/// - Reads remote configuration and negotiates refs with one or more remotes.
/// - Downloads pack data and writes received objects into local storage.
/// - Updates remote-tracking refs for fetched branches.
/// - Renders fetch status in the requested output format.
///
/// # Errors
/// Returns [`CliError`] when remote configuration is invalid or missing,
/// authentication/network/pack negotiation fails, object writes fail, or
/// remote-tracking refs cannot be updated.
pub async fn execute_safe(args: FetchArgs, output: &OutputConfig) -> CliResult<()> {
    // `--recurse-submodules` is declared only so it produces a friendly usage
    // error (129) rather than a clap "unknown argument" (2).
    if args.recurse_submodules.is_some() {
        return Err(CliError::command_usage(
            "libra fetch does not support submodule recursion",
        )
        .with_stable_code(StableErrorCode::CliInvalidArguments)
        .with_hint(
            "Libra does not manage submodules with the stock Git layout; fetch them as separate repositories",
        ));
    }
    // `--porcelain` and `--json` are both machine formats; `--json` is a global
    // flag so this exclusion is enforced here (usage error 129), not by clap.
    if args.porcelain && output.is_json() {
        return Err(
            CliError::command_usage("--porcelain and --json are mutually exclusive")
                .with_stable_code(StableErrorCode::CliInvalidArguments),
        );
    }
    let porcelain = args.porcelain;
    let dry_run = args.dry_run;
    let append = args.append;
    let result = run_fetch(args, output).await?;
    // FETCH_HEAD records the fetched refs; `--dry-run` writes nothing.
    if !dry_run {
        write_fetch_head(&result, append).map_err(CliError::from)?;
    }
    if porcelain {
        render_fetch_porcelain(&result, output)
    } else {
        render_fetch_output(&result, output)
    }
}

/// Render Git's `--porcelain` format: one `<flag> <old-oid> <new-oid>
/// <local-ref>` line per ref update, single-space separated, with no human
/// summary columns.
fn render_fetch_porcelain(result: &FetchOutput, output: &OutputConfig) -> CliResult<()> {
    if output.quiet {
        return Ok(());
    }
    let rendered = format_fetch_porcelain(result);
    if rendered.is_empty() {
        return Ok(());
    }
    let stdout = io::stdout();
    let mut writer = stdout.lock();
    writeln!(writer, "{rendered}")
        .map_err(|error| CliError::io(format!("failed to write fetch output: {error}")))
}

fn format_fetch_porcelain(result: &FetchOutput) -> String {
    let mut lines = Vec::new();
    for remote in &result.remotes {
        for update in &remote.refs_updated {
            let (flag, old_oid) = if update.forced {
                // Forced (non-fast-forward) update, e.g. a `--force` tag clobber.
                (
                    '+',
                    update
                        .old_oid
                        .clone()
                        .unwrap_or_else(|| "0".repeat(update.new_oid.len())),
                )
            } else {
                match &update.old_oid {
                    // New ref: space-flag is reserved for fast-forward; new refs
                    // use `*` with an all-zero old object id sized to the hash kind.
                    None => ('*', "0".repeat(update.new_oid.len())),
                    Some(old) => (' ', old.clone()),
                }
            };
            lines.push(format!(
                "{flag} {old_oid} {} {}",
                update.new_oid, update.remote_ref
            ));
        }
    }
    lines.join("\n")
}

/// Whether `fetch.prune` is configured truthy, providing the default for
/// `--prune` when the flag is absent.
async fn read_fetch_prune_config() -> bool {
    matches!(
        ConfigKv::get("fetch.prune").await,
        Ok(Some(entry)) if matches!(
            entry.value.trim().to_ascii_lowercase().as_str(),
            "true" | "yes" | "on" | "1"
        )
    )
}

async fn run_fetch(args: FetchArgs, output: &OutputConfig) -> CliResult<FetchOutput> {
    tracing::debug!("`fetch` args: {:?}", args);

    let FetchArgs {
        repository,
        refspec,
        all,
        depth,
        deepen,
        unshallow,
        recurse_submodules: _,
        porcelain: _,
        shallow_since,
        shallow_exclude,
        prune,
        dry_run,
        append: _,
        verbose,
        tags,
        no_tags,
        force,
    } = args;

    // `--prune` is enabled by the flag or the `fetch.prune` config (flag wins).
    let prune = prune || read_fetch_prune_config().await;

    // `--shallow-since` is parsed at the command layer so an unparseable date is
    // a usage error (129) rather than a deep protocol failure.
    let deepen_since = match &shallow_since {
        Some(date) => Some(parse_date(date).map_err(|error| {
            CliError::command_usage(format!("invalid --shallow-since date '{date}': {error}"))
                .with_stable_code(StableErrorCode::CliInvalidArguments)
        })?),
        None => None,
    };
    let shallow =
        build_fetch_shallow_options(depth, deepen, unshallow, deepen_since, shallow_exclude);

    if all {
        let remotes = ConfigKv::all_remote_configs().await.map_err(|error| {
            CliError::fatal(format!("failed to read remote configuration: {error}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        })?;

        let mut results = Vec::with_capacity(remotes.len());
        for remote in remotes {
            if verbose {
                eprintln!(
                    "Fetching {} from {}",
                    remote.name,
                    redact_url_credentials(&remote.url)
                );
            }
            results.push(
                fetch_repository_with_result(
                    remote,
                    None,
                    false,
                    shallow.clone(),
                    unshallow,
                    prune,
                    dry_run,
                    tags,
                    no_tags,
                    force,
                    output,
                )
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

    if verbose {
        eprintln!(
            "Fetching {} from {}",
            remote_config.name,
            redact_url_credentials(&remote_config.url)
        );
    }

    let result = fetch_repository_with_result(
        remote_config,
        refspec.clone(),
        false,
        shallow,
        unshallow,
        prune,
        dry_run,
        tags,
        no_tags,
        force,
        output,
    )
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

        // `remote.url` is already credential-redacted at construction time in
        // `fetch_repository_with_result`, so no additional redaction needed here.
        writeln!(writer, "From {}", remote.url)
            .map_err(|error| CliError::io(format!("failed to write fetch output: {error}")))?;

        if remote.refs_updated.is_empty() && remote.pruned.is_empty() {
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

        for pruned_ref in &remote.pruned {
            let ref_name = pruned_ref
                .strip_prefix("refs/remotes/")
                .unwrap_or(pruned_ref);
            writeln!(writer, " - [deleted]         (none)     -> {ref_name}")
                .map_err(|error| CliError::io(format!("failed to write fetch output: {error}")))?;
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
            // Classify against a credential-redacted spec so neither the `spec`
            // field nor the `reason` string (which may interpolate the spec for
            // malformed URLs) can leak a token/password.
            let redacted_spec = redact_url_credentials(remote_spec);
            let (kind, reason) = classify_remote_spec_error(&redacted_spec, &message);
            FetchError::InvalidRemoteSpec {
                spec: redacted_spec,
                kind,
                reason,
            }
        })?;
    let discovery = remote_client
        .discovery_reference(UploadPack)
        .await
        .map_err(|source| FetchError::Discovery {
            // Redact credentials so a token/password never reaches error output.
            remote: redact_url_credentials(remote_spec),
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
        // The scheme-only message carries no userinfo.
        return (RemoteSpecErrorKind::UnsupportedScheme, message.to_string());
    }
    // Default to MalformedUrl. The raw `message` may interpolate the original
    // spec (e.g. "invalid file url: file://user:pass@host/…"), so build the
    // reason from the already-redacted `remote_spec` instead of the raw message.
    (
        RemoteSpecErrorKind::MalformedUrl,
        format!("'{remote_spec}' is not a valid remote URL or local path"),
    )
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
        ShallowOptions::from_depth(depth),
        &OutputConfig::default(),
    )
    .await
    {
        CliError::from(err).print_stderr();
    }
}

/// Git uses a very large deepen value to request the complete history when
/// unshallowing; mirror that here so a shallow source is fully materialized.
pub(crate) const UNSHALLOW_DEPTH: usize = 0x7fff_ffff;

/// Translate the `fetch` CLI depth/deepen/unshallow flags into a single
/// [`ShallowOptions`] request. `--unshallow` requests the complete history,
/// `--deepen N` takes precedence over `--depth N`, otherwise `--depth N` is used.
pub(crate) fn build_fetch_shallow_options(
    depth: Option<usize>,
    deepen: Option<usize>,
    unshallow: bool,
    deepen_since: Option<i64>,
    deepen_not: Vec<String>,
) -> ShallowOptions {
    let effective_depth = if unshallow {
        Some(UNSHALLOW_DEPTH)
    } else {
        deepen.or(depth)
    };
    ShallowOptions {
        depth: effective_depth,
        deepen_since,
        deepen_not,
        filter: None,
    }
}

pub async fn fetch_repository_safe(
    remote_config: RemoteConfig,
    branch: Option<String>,
    single_branch: bool,
    shallow: ShallowOptions,
    output: &OutputConfig,
) -> Result<(), FetchError> {
    fetch_repository_with_result(
        remote_config,
        branch,
        single_branch,
        shallow,
        false,
        false,
        false,
        false,
        false,
        false,
        output,
    )
    .await
    .map(|_| ())
}

/// How `fetch` should treat the remote's tags. Git's auto-follow default — a
/// second negotiation round that pulls the objects of tags pointing at
/// already-fetched commits — is deferred; the current default imports no tags
/// unless `--tags`/`-t` (or `remote.<name>.tagOpt = --tags`) asks for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TagFetchMode {
    /// Import no tags (the default and `--no-tags`).
    None,
    /// Import every advertised tag, pulling each tag object into the pack.
    All,
}

/// A remote tag advertised during ref discovery that fetch may import.
#[derive(Debug, Clone)]
struct TagCandidate {
    /// Fully-qualified local ref name, e.g. `refs/tags/v1.0`.
    ref_name: String,
    /// Advertised object hash: the tag-object hash for an annotated tag, or the
    /// commit hash for a lightweight tag. This is both the `want` to request and
    /// the target the local tag ref is written to (matching `libra tag`).
    object_hash: String,
}

/// Collect importable tag candidates from the advertised refs, dropping the
/// peeled `refs/tags/<name>^{}` companions (whose target is reached through the
/// annotated tag object itself).
fn collect_tag_candidates(refs: &[DiscRef]) -> Vec<TagCandidate> {
    refs.iter()
        .filter(|reference| {
            reference._ref.starts_with("refs/tags/") && !reference._ref.ends_with("^{}")
        })
        .map(|reference| TagCandidate {
            ref_name: reference._ref.clone(),
            object_hash: reference._hash.clone(),
        })
        .collect()
}

/// Read `remote.<name>.tagOpt`, tolerating either the Git-canonical lowercase
/// `tagopt` key or a verbatim `tagOpt` spelling.
async fn read_remote_tagopt(remote_name: &str) -> Option<String> {
    for key in [
        format!("remote.{remote_name}.tagopt"),
        format!("remote.{remote_name}.tagOpt"),
    ] {
        if let Ok(Some(entry)) = ConfigKv::get(&key).await {
            return Some(entry.value);
        }
    }
    None
}

/// Resolve the effective tag-import behavior for a remote: an explicit
/// `--no-tags`/`--tags` flag wins, otherwise `remote.<name>.tagOpt` (`--tags` /
/// `--no-tags`) applies, otherwise the default imports no tags.
async fn resolve_tag_mode(remote_name: &str, tags_flag: bool, no_tags_flag: bool) -> TagFetchMode {
    if no_tags_flag {
        return TagFetchMode::None;
    }
    if tags_flag {
        return TagFetchMode::All;
    }
    match read_remote_tagopt(remote_name).await.as_deref() {
        Some("--tags") => TagFetchMode::All,
        // `--no-tags` or any unrecognised value imports no tags.
        _ => TagFetchMode::None,
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn fetch_repository_with_result(
    remote_config: RemoteConfig,
    branch: Option<String>,
    single_branch: bool,
    shallow: ShallowOptions,
    unshallow: bool,
    prune: bool,
    dry_run: bool,
    tags_flag: bool,
    no_tags_flag: bool,
    force: bool,
    output: &OutputConfig,
) -> Result<FetchRepositoryResult, FetchError> {
    let (remote_client, discovery) =
        discover_remote_with_name(&remote_config.url, Some(&remote_config.name)).await?;
    // Redact credentials from the URL before storing it in the result to
    // prevent secret leakage in both human and JSON output.
    let normalized_url =
        redact_url_credentials(&normalize_remote_url(&remote_config.url, &remote_client));
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
            pruned: Vec::new(),
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

    // Resolve how tags should be imported (CLI flag > `remote.<name>.tagOpt` >
    // default of none) and capture the advertised tag candidates *before* the
    // ref set is narrowed to branches below — tags live in the global
    // `refs/tags/*` namespace, not under `refs/remotes/<remote>/*`.
    let tag_mode = resolve_tag_mode(&remote_config.name, tags_flag, no_tags_flag).await;
    let tag_candidates = match tag_mode {
        TagFetchMode::None => Vec::new(),
        TagFetchMode::All => collect_tag_candidates(&discovery.refs),
    };

    // Only request refs we will actually persist as remote-tracking refs.
    // `update_references` saves `refs/heads/*` and `refs/mr/*`; asking for
    // anything else (HEAD symref, `refs/pull/*`, `refs/tags/*`) makes the
    // server include unreachable objects that the next fetch's `have` cannot
    // cover, which forces the same pack to be re-downloaded every time. Tags
    // are requested explicitly via `tag_candidates` when `--tags` is set.
    refs.retain(|reference| {
        reference._ref.starts_with("refs/heads/") || reference._ref.starts_with("refs/mr/")
    });

    if let Some(branch_name) = &branch
        && single_branch
    {
        let normalized = normalize_branch_ref(branch_name);
        refs.retain(|reference| reference._ref == normalized);
    }

    // `--dry-run`: compute the would-be ref updates (and prune list) from the
    // discovered refs and return before downloading any pack or writing anything
    // (no `.pack`/`.idx`, no shallow update, no ref/reflog writes, no FETCH_HEAD).
    if dry_run {
        let mut refs_updated = compute_fetch_ref_preview(&remote_config, &refs).await?;
        refs_updated.extend(preview_tag_updates(&tag_candidates, force).await?);
        let pruned = if prune {
            let remote_branch_names =
                crate::command::remote::collect_remote_branch_names(&discovery.refs);
            crate::command::remote::prune_stale_tracking_branches(
                &remote_config.name,
                &remote_branch_names,
                true,
            )
            .await
            .map_err(|error| FetchError::LocalState {
                message: format!("failed to preview prune: {error}"),
            })?
            .into_iter()
            .map(|entry| entry.remote_ref)
            .collect()
        } else {
            Vec::new()
        };
        return Ok(FetchRepositoryResult {
            remote: remote_config.name,
            url: normalized_url,
            refs_updated,
            objects_fetched: 0,
            pruned,
        });
    }

    let mut want = refs
        .iter()
        .map(|reference| reference._hash.clone())
        .collect::<Vec<_>>();
    // `--tags` pulls each advertised tag object into the pack so the imported
    // `refs/tags/*` always resolve to present objects.
    want.extend(
        tag_candidates
            .iter()
            .map(|candidate| candidate.object_hash.clone()),
    );
    want.sort();
    want.dedup();
    let have = current_have_safe().await?;
    let shallow_boundaries = read_shallow_boundaries()?;
    let shallow_boundary_oids = shallow_boundaries.iter().cloned().collect::<Vec<_>>();
    let mut result_stream = remote_client
        .fetch_objects(&have, &want, &shallow_boundary_oids, &shallow)
        .await
        .map_err(|source| FetchError::FetchObjects {
            // Redact credentials so a token/password never reaches error output.
            remote: redact_url_credentials(&remote_config.url),
            source,
        })?;

    let task = format!("fetch {}", remote_config.name);
    // When any deepen/shallow request is made (depth/since/exclude, an existing
    // shallow boundary, or unshallow), upload-pack always prefixes the pack with
    // a shallow section terminated by a flush packet — even when no boundary is
    // cut. The reader must consume that leading flush instead of stopping early.
    let shallow_requested = shallow.is_requested() || !shallow_boundary_oids.is_empty();
    let fetch_data =
        read_fetch_stream(&mut result_stream, output, &task, shallow_requested).await?;
    let objects_fetched = pack_object_count(&fetch_data.pack_data);
    let pack_file = write_pack_and_index(&fetch_data.pack_data)?;
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
    apply_shallow_updates(&fetch_data.shallow, &fetch_data.unshallow)?;
    if unshallow {
        // `--unshallow` requests the complete history; once the pack is written,
        // drop every shallow boundary so the repository is no longer shallow.
        write_shallow_boundaries(&BTreeSet::new())?;
    }

    let refs_updated = update_references(
        &remote_config,
        &refs,
        &ref_heads,
        remote_head,
        branch,
        &tag_candidates,
        force,
    )
    .await?;

    // `--prune` removes local tracking branches whose remote counterpart is gone;
    // it compares against the remote's advertised refs (`discovery.refs`), never
    // the locally-filtered set, and never touches `refs/heads/*`.
    let pruned = if prune {
        let remote_branch_names =
            crate::command::remote::collect_remote_branch_names(&discovery.refs);
        crate::command::remote::prune_stale_tracking_branches(
            &remote_config.name,
            &remote_branch_names,
            false,
        )
        .await
        .map_err(|error| FetchError::LocalState {
            message: format!("failed to prune stale remote-tracking branches: {error}"),
        })?
        .into_iter()
        .map(|entry| entry.remote_ref)
        .collect()
    } else {
        Vec::new()
    };

    Ok(FetchRepositoryResult {
        remote: remote_config.name,
        url: normalized_url,
        refs_updated,
        objects_fetched,
        pruned,
    })
}

#[derive(Default)]
struct FetchStreamData {
    pack_data: Vec<u8>,
    shallow: Vec<String>,
    unshallow: Vec<String>,
}

/// Tracks packfile boundaries so fetch can finish once the pack checksum is
/// present, even if the SSH transport stays open after `git-upload-pack` is done.
#[derive(Default)]
struct PackCompletionTracker {
    object_count: Option<usize>,
    objects_seen: usize,
    offset: usize,
    current_object: Option<PackObjectInflate>,
    complete: bool,
}

struct PackObjectInflate {
    start: usize,
    inflater: flate2::Decompress,
}

impl PackCompletionTracker {
    fn observe(&mut self, pack_data: &[u8]) -> bool {
        if self.complete {
            return true;
        }

        if self.object_count.is_none() && !self.read_header(pack_data) {
            return false;
        }

        let Some(object_count) = self.object_count else {
            return false;
        };

        while self.objects_seen < object_count {
            if !self.advance_object(pack_data) {
                return false;
            }
        }

        self.complete = self.has_valid_trailing_checksum(pack_data);
        self.complete
    }

    fn read_header(&mut self, pack_data: &[u8]) -> bool {
        if pack_data.len() < 12 || &pack_data[..4] != b"PACK" {
            return false;
        }
        let Some(version) = read_be_u32(pack_data, 4) else {
            return false;
        };
        if version != 2 && version != 3 {
            return false;
        }
        let Some(object_count) = read_be_u32(pack_data, 8) else {
            return false;
        };
        self.object_count = Some(object_count as usize);
        self.offset = 12;
        true
    }

    fn advance_object(&mut self, pack_data: &[u8]) -> bool {
        if self.current_object.is_none() {
            let Some(data_offset) =
                parse_pack_entry_data_offset(pack_data, self.offset, get_hash_kind().size())
            else {
                return false;
            };
            self.current_object = Some(PackObjectInflate {
                start: data_offset,
                inflater: flate2::Decompress::new(true),
            });
        }

        let complete_offset = {
            let Some(current) = self.current_object.as_mut() else {
                return false;
            };
            let mut output = [0_u8; 8192];
            loop {
                let consumed = current.inflater.total_in() as usize;
                let Some(input_offset) = current.start.checked_add(consumed) else {
                    return false;
                };
                let Some(input) = pack_data.get(input_offset..) else {
                    return false;
                };
                if input.is_empty() {
                    return false;
                }

                let before_in = current.inflater.total_in();
                let before_out = current.inflater.total_out();
                let status = match current.inflater.decompress(
                    input,
                    &mut output,
                    flate2::FlushDecompress::None,
                ) {
                    Ok(status) => status,
                    Err(_) => return false,
                };
                if matches!(status, flate2::Status::StreamEnd) {
                    break current
                        .start
                        .checked_add(current.inflater.total_in() as usize);
                }
                if before_in == current.inflater.total_in()
                    && before_out == current.inflater.total_out()
                {
                    return false;
                }
            }
        };

        let Some(complete_offset) = complete_offset else {
            return false;
        };
        self.offset = complete_offset;
        self.current_object = None;
        self.objects_seen += 1;
        true
    }

    fn has_valid_trailing_checksum(&self, pack_data: &[u8]) -> bool {
        let hash_len = get_hash_kind().size();
        let Some(end) = self.offset.checked_add(hash_len) else {
            return false;
        };
        if pack_data.len() != end {
            return false;
        }
        let expected = ObjectHash::new(&pack_data[..self.offset]);
        ObjectHash::from_bytes(&pack_data[self.offset..end]).is_ok_and(|actual| actual == expected)
    }
}

fn read_be_u32(data: &[u8], offset: usize) -> Option<u32> {
    let bytes = data.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn parse_pack_entry_data_offset(
    pack_data: &[u8],
    mut offset: usize,
    hash_len: usize,
) -> Option<usize> {
    let first = *pack_data.get(offset)?;
    offset += 1;
    let object_type = (first >> 4) & 0b111;
    let mut byte = first;
    while byte & 0x80 != 0 {
        byte = *pack_data.get(offset)?;
        offset += 1;
    }

    match object_type {
        1..=4 => Some(offset),
        6 => {
            byte = *pack_data.get(offset)?;
            offset += 1;
            while byte & 0x80 != 0 {
                byte = *pack_data.get(offset)?;
                offset += 1;
            }
            Some(offset)
        }
        7 => offset
            .checked_add(hash_len)
            .filter(|end| *end <= pack_data.len()),
        _ => None,
    }
}

async fn read_fetch_stream(
    result_stream: &mut FetchStream,
    output: &OutputConfig,
    task: &str,
    shallow_requested: bool,
) -> Result<FetchStreamData, FetchError> {
    let mut reader = StreamReader::new(result_stream);
    let mut data_out = FetchStreamData::default();
    let mut pack_completion = PackCompletionTracker::default();
    let mut reach_pack = false;
    let mut saw_shallow_response = false;
    // upload-pack emits a shallow section (possibly empty) terminated by a flush
    // packet before the pack whenever a deepen/shallow request was made. Expect to
    // consume that one leading flush so an empty shallow section is not mistaken
    // for end-of-stream.
    let mut expecting_shallow_terminator = shallow_requested;
    let render_progress = matches!(output.progress, ProgressMode::Text);
    let json_progress = matches!(output.progress, ProgressMode::Json);
    let bar = render_progress.then(ProgressBar::new_spinner);
    let progress = json_progress.then(|| ProgressReporter::new(task, None, output));
    let mut remote_progress = RemoteProgressBuffer::default();
    let time = Instant::now();

    loop {
        let (len, data) = match read_pkt_line(&mut reader).await {
            Ok(packet) => packet,
            Err(source) if source.kind() == io::ErrorKind::UnexpectedEof && reach_pack => break,
            Err(source) => return Err(FetchError::PacketRead { source }),
        };
        if len == 0 {
            if !reach_pack && (saw_shallow_response || expecting_shallow_terminator) {
                // End of the (possibly empty) shallow section that always precedes
                // the pack for a deepen/shallow request. Consume it exactly once.
                saw_shallow_response = false;
                expecting_shallow_terminator = false;
                continue;
            }
            break;
        }
        if !reach_pack {
            if let Some(oid) = parse_shallow_packet(&data, b"shallow ") {
                data_out.shallow.push(oid);
                saw_shallow_response = true;
                continue;
            }
            if let Some(oid) = parse_shallow_packet(&data, b"unshallow ") {
                data_out.unshallow.push(oid);
                saw_shallow_response = true;
                continue;
            }
            if data.starts_with(b"PACK") {
                reach_pack = true;
                data_out.pack_data.extend(&data);
                if let Some(progress) = &progress {
                    progress.tick(data_out.pack_data.len() as u64);
                }
                if pack_completion.observe(&data_out.pack_data) {
                    break;
                }
                continue;
            }
        }
        if data.len() >= 5 && data[0] == 1 && &data[1..5] == b"PACK" {
            reach_pack = true;
        }

        if reach_pack {
            if let Some((&code, payload)) = data.split_first() {
                match code {
                    1 => {
                        let bytes_per_sec =
                            data_out.pack_data.len() as f64 / time.elapsed().as_secs_f64();
                        let total = util::auto_unit_bytes(data_out.pack_data.len() as u64);
                        let bps = util::auto_unit_bytes(bytes_per_sec as u64);
                        if let Some(bar) = &bar {
                            bar.set_message(format!("Receiving objects: {total:.2} | {bps:.2}/s"));
                            bar.tick();
                        }
                        data_out.pack_data.extend(payload);
                        if let Some(progress) = &progress {
                            progress.tick(data_out.pack_data.len() as u64);
                        }
                        if pack_completion.observe(&data_out.pack_data) {
                            break;
                        }
                    }
                    2 => handle_remote_progress(
                        payload,
                        render_progress,
                        bar.as_ref(),
                        &mut remote_progress,
                    ),
                    3 => {
                        flush_remote_progress(render_progress, bar.as_ref(), &mut remote_progress);
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
            && !data.starts_with(b"ACK ")
            && !data.starts_with(b"shallow ")
            && !data.starts_with(b"unshallow ")
            && let Some((&code, payload)) = data.split_first()
        {
            match code {
                2 => handle_remote_progress(
                    payload,
                    render_progress,
                    bar.as_ref(),
                    &mut remote_progress,
                ),
                3 => {
                    flush_remote_progress(render_progress, bar.as_ref(), &mut remote_progress);
                    if let Some(bar) = &bar {
                        bar.finish_and_clear();
                    }
                    return Err(FetchError::RemoteSideband {
                        message: String::from_utf8_lossy(payload).trim().to_string(),
                    });
                }
                _ => {
                    tracing::debug!(
                        "ignoring pre-pack frame: {:?}",
                        String::from_utf8_lossy(&data)
                    );
                }
            }
        }
    }
    flush_remote_progress(render_progress, bar.as_ref(), &mut remote_progress);
    if let Some(bar) = &bar {
        bar.finish_and_clear();
    }
    if let Some(progress) = &progress {
        progress.finish();
    }

    Ok(data_out)
}

fn parse_shallow_packet(data: &[u8], prefix: &[u8]) -> Option<String> {
    let raw = data.strip_prefix(prefix)?;
    let text = std::str::from_utf8(raw).ok()?.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// Line-buffers raw sideband progress bytes so the indicatif spinner and the
/// remote's `\r`-overwriting progress text do not stomp on each other.
///
/// Git's smart protocol delivers human-readable progress on side-band 2 in
/// arbitrarily small chunks that may split mid-word. The remote uses `\r` for
/// in-place updates (e.g. `Counting objects:  5%\rCounting objects: 10%\r…`)
/// and `\n` to commit a line (e.g. `Counting objects: 100% (38/38), done.\n`).
/// Forwarding raw bytes straight to `eprint!` while the local spinner is also
/// being redrawn produces interleaved fragments separated by spinner ticks.
#[derive(Default)]
struct RemoteProgressBuffer {
    buf: String,
}

impl RemoteProgressBuffer {
    /// Append `payload` and dispatch any complete lines.
    ///
    /// - `\n`-terminated (and `\r\n`-terminated) lines are emitted to
    ///   `on_permanent` so the caller can promote them to a log line above
    ///   the bar.
    /// - `\r`-terminated lines are emitted to `on_transient`, which typically
    ///   maps to `bar.set_message` so the latest progress replaces the prior.
    /// - Any trailing partial content stays in the buffer for the next call.
    fn push<P, T>(&mut self, payload: &[u8], mut on_permanent: P, mut on_transient: T)
    where
        P: FnMut(&str),
        T: FnMut(&str),
    {
        if payload.is_empty() {
            return;
        }
        self.buf.push_str(&String::from_utf8_lossy(payload));
        while let Some(pos) = self.buf.find(['\r', '\n']) {
            // ASCII terminators are always at char boundaries, so split_off is safe.
            let terminator = self.buf.as_bytes()[pos];
            let line: String = self.buf.drain(..pos).collect();
            self.buf.drain(..1);

            // Treat CRLF as a single newline so we don't emit an extra empty transient.
            let is_permanent =
                terminator == b'\n' || (terminator == b'\r' && self.buf.starts_with('\n'));
            if terminator == b'\r' && self.buf.starts_with('\n') {
                self.buf.drain(..1);
            }
            if is_permanent {
                on_permanent(&line);
            } else {
                on_transient(&line);
            }
        }
    }

    /// Emit any unterminated trailing bytes as a permanent line.
    ///
    /// Called once the sideband stream has ended so we never silently drop
    /// the last fragment when the remote closed without a final newline.
    fn flush_remaining<P>(&mut self, mut on_permanent: P)
    where
        P: FnMut(&str),
    {
        if !self.buf.is_empty() {
            let line = std::mem::take(&mut self.buf);
            on_permanent(&line);
        }
    }
}

fn handle_remote_progress(
    payload: &[u8],
    render_progress: bool,
    bar: Option<&ProgressBar>,
    buffer: &mut RemoteProgressBuffer,
) {
    if !render_progress {
        return;
    }
    buffer.push(
        payload,
        |line| emit_permanent_progress_line(line, bar),
        |line| emit_transient_progress_line(line, bar),
    );
}

fn flush_remote_progress(
    render_progress: bool,
    bar: Option<&ProgressBar>,
    buffer: &mut RemoteProgressBuffer,
) {
    if !render_progress {
        return;
    }
    buffer.flush_remaining(|line| emit_permanent_progress_line(line, bar));
}

fn emit_permanent_progress_line(line: &str, bar: Option<&ProgressBar>) {
    if let Some(bar) = bar {
        // `println` clears the bar, prints the line, then redraws the bar
        // below — the canonical way to interleave logs with an indicatif spinner.
        bar.println(line);
    } else {
        let mut stderr = io::stderr().lock();
        let _ = writeln!(stderr, "{line}");
    }
}

fn emit_transient_progress_line(line: &str, bar: Option<&ProgressBar>) {
    if let Some(bar) = bar {
        bar.set_message(line.to_owned());
        bar.tick();
    } else {
        let mut stderr = io::stderr().lock();
        let _ = write!(stderr, "\r{line}");
        let _ = stderr.flush();
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

fn shallow_file_path() -> Result<PathBuf, FetchError> {
    util::try_get_storage_path(None)
        .map(|storage| storage.join("shallow"))
        .map_err(|source| FetchError::LocalState {
            message: format!("failed to locate repository storage for shallow metadata: {source}"),
        })
}

pub(crate) fn read_shallow_boundaries() -> Result<BTreeSet<String>, FetchError> {
    let path = shallow_file_path()?;
    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(source) => {
            return Err(FetchError::LocalState {
                message: format!(
                    "failed to read shallow metadata '{}': {source}",
                    path.display()
                ),
            });
        }
    };

    let mut boundaries = BTreeSet::new();
    for (line_no, line) in content.lines().enumerate() {
        let oid = line.trim();
        if oid.is_empty() {
            continue;
        }
        ObjectHash::from_str(oid).map_err(|source| FetchError::LocalState {
            message: format!(
                "invalid shallow metadata entry at '{}:{}': {source}",
                path.display(),
                line_no + 1
            ),
        })?;
        boundaries.insert(oid.to_string());
    }
    Ok(boundaries)
}

fn write_shallow_boundaries(boundaries: &BTreeSet<String>) -> Result<(), FetchError> {
    let path = shallow_file_path()?;
    if boundaries.is_empty() {
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(FetchError::LocalState {
                    message: format!(
                        "failed to remove shallow metadata '{}': {source}",
                        path.display()
                    ),
                });
            }
        }
        return Ok(());
    }

    let mut content = String::new();
    for oid in boundaries {
        content.push_str(oid);
        content.push('\n');
    }
    fs::write(&path, content).map_err(|source| FetchError::LocalState {
        message: format!(
            "failed to write shallow metadata '{}': {source}",
            path.display()
        ),
    })?;

    // The shallow boundary file records repository history limits; keep it owner
    // read/write only on Unix (Windows has no comparable mode bits to set).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).map_err(|source| {
            FetchError::LocalState {
                message: format!(
                    "failed to set permissions on shallow metadata '{}': {source}",
                    path.display()
                ),
            }
        })?;
    }
    Ok(())
}

fn fetch_head_path() -> Result<PathBuf, FetchError> {
    util::try_get_storage_path(None)
        .map(|storage| storage.join("FETCH_HEAD"))
        .map_err(|source| FetchError::LocalState {
            message: format!("failed to locate repository storage for FETCH_HEAD: {source}"),
        })
}

/// Render the `FETCH_HEAD` body: one `<oid>\t<not-for-merge>\t<desc>` line per
/// fetched ref. Libra fetch never designates a merge target (merge with
/// `libra pull`), so every line is marked `not-for-merge`.
fn format_fetch_head(result: &FetchOutput) -> String {
    let mut lines = Vec::new();
    for remote in &result.remotes {
        let tracking_prefix = format!("refs/remotes/{}/", remote.remote);
        for update in &remote.refs_updated {
            let branch = update
                .remote_ref
                .strip_prefix(&tracking_prefix)
                .unwrap_or(&update.remote_ref);
            lines.push(format!(
                "{}\tnot-for-merge\tbranch '{}' of {}",
                update.new_oid, branch, remote.url
            ));
        }
    }
    lines.join("\n")
}

/// Write (or, with `append`, accumulate into) `.libra/FETCH_HEAD` via an atomic
/// temp-file + rename, owner-only on Unix.
fn write_fetch_head(result: &FetchOutput, append: bool) -> Result<(), FetchError> {
    let path = fetch_head_path()?;
    let body = format_fetch_head(result);

    let mut content = String::new();
    if append && let Ok(existing) = fs::read_to_string(&path) {
        content.push_str(&existing);
        if !content.is_empty() && !content.ends_with('\n') {
            content.push('\n');
        }
    }
    if !body.is_empty() {
        content.push_str(&body);
        content.push('\n');
    }

    let tmp = path.with_extension("tmp");
    fs::write(&tmp, &content).map_err(|source| FetchError::LocalState {
        message: format!(
            "failed to write FETCH_HEAD temp '{}': {source}",
            tmp.display()
        ),
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600)).map_err(|source| {
            FetchError::LocalState {
                message: format!("failed to set permissions on FETCH_HEAD: {source}"),
            }
        })?;
    }
    fs::rename(&tmp, &path).map_err(|source| FetchError::LocalState {
        message: format!(
            "failed to finalize FETCH_HEAD '{}': {source}",
            path.display()
        ),
    })
}

fn apply_shallow_updates(shallow: &[String], unshallow: &[String]) -> Result<(), FetchError> {
    if shallow.is_empty() && unshallow.is_empty() {
        return Ok(());
    }

    let mut boundaries = read_shallow_boundaries()?;
    for oid in shallow {
        ObjectHash::from_str(oid).map_err(|source| FetchError::LocalState {
            message: format!("remote sent invalid shallow boundary '{oid}': {source}"),
        })?;
        boundaries.insert(oid.clone());
    }
    for oid in unshallow {
        ObjectHash::from_str(oid).map_err(|source| FetchError::LocalState {
            message: format!("remote sent invalid unshallow boundary '{oid}': {source}"),
        })?;
        boundaries.remove(oid);
    }
    write_shallow_boundaries(&boundaries)
}

/// Read-only counterpart of [`update_references`] for `--dry-run`: report the
/// ref updates the discovered refs would produce, without any database writes.
async fn compute_fetch_ref_preview(
    remote_config: &RemoteConfig,
    refs: &[DiscRef],
) -> Result<Vec<FetchRefUpdate>, FetchError> {
    let mut updates = Vec::new();
    for reference in refs {
        let full_ref_name = if let Some(branch_name) = reference._ref.strip_prefix("refs/heads/") {
            format!("refs/remotes/{}/{}", remote_config.name, branch_name)
        } else if let Some(mr_name) = reference._ref.strip_prefix("refs/mr/") {
            format!("refs/remotes/{}/mr/{}", remote_config.name, mr_name)
        } else {
            continue;
        };

        let old_oid = Branch::find_branch_result(&full_ref_name, Some(&remote_config.name))
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
        updates.push(FetchRefUpdate {
            remote_ref: full_ref_name,
            old_oid,
            new_oid: reference._hash.clone(),
            forced: false,
        });
    }
    Ok(updates)
}

/// Compute the would-be tag imports for `--dry-run`. A candidate whose local
/// tag does not exist previews as a new ref; an existing tag is skipped unless
/// `force` is set, in which case it previews as a forced (clobbering) update.
async fn preview_tag_updates(
    candidates: &[TagCandidate],
    force: bool,
) -> Result<Vec<FetchRefUpdate>, FetchError> {
    let mut updates = Vec::new();
    for candidate in candidates {
        let short = candidate
            .ref_name
            .strip_prefix("refs/tags/")
            .unwrap_or(&candidate.ref_name);
        let existing = crate::internal::tag::find_tag_ref(short)
            .await
            .map_err(|error| FetchError::UpdateRefs {
                message: format!(
                    "failed to inspect existing tag '{}': {error}",
                    candidate.ref_name
                ),
            })?;
        match existing {
            Some(_) if !force => continue,
            Some(tag_ref) => updates.push(FetchRefUpdate {
                remote_ref: candidate.ref_name.clone(),
                old_oid: tag_ref.target,
                new_oid: candidate.object_hash.clone(),
                forced: true,
            }),
            None => updates.push(FetchRefUpdate {
                remote_ref: candidate.ref_name.clone(),
                old_oid: None,
                new_oid: candidate.object_hash.clone(),
                forced: false,
            }),
        }
    }
    Ok(updates)
}

async fn update_references(
    remote_config: &RemoteConfig,
    refs: &[DiscRef],
    ref_heads: &[DiscRef],
    remote_head: Option<DiscRef>,
    branch: Option<String>,
    tags_to_import: &[TagCandidate],
    force: bool,
) -> Result<Vec<FetchRefUpdate>, FetchError> {
    let db = get_db_conn_instance().await;
    let remote_config = remote_config.clone();
    let refs = refs.to_vec();
    let ref_heads = ref_heads.to_vec();
    let tags_to_import = tags_to_import.to_vec();
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
                    forced: false,
                });
            }

            // Import fetched tags into the global `refs/tags/*` namespace, in
            // the same transaction so a failure rolls back the whole batch.
            // Tags are immutable by default: an existing local tag is preserved
            // unless `--force` is given, in which case it is clobbered and the
            // update is flagged as forced (porcelain `+`).
            for tag in &tags_to_import {
                let outcome = crate::internal::tag::import_fetched_tag_with_conn(
                    txn,
                    &tag.ref_name,
                    &tag.object_hash,
                    force,
                )
                .await
                .map_err(|source| FetchError::UpdateRefs {
                    message: format!("failed to persist fetched tag '{}': {source}", tag.ref_name),
                })?;
                match outcome {
                    crate::internal::tag::TagImportOutcome::Created => {
                        updates.push(FetchRefUpdate {
                            remote_ref: tag.ref_name.clone(),
                            old_oid: None,
                            new_oid: tag.object_hash.clone(),
                            forced: false,
                        });
                    }
                    crate::internal::tag::TagImportOutcome::Updated { previous } => {
                        updates.push(FetchRefUpdate {
                            remote_ref: tag.ref_name.clone(),
                            old_oid: previous,
                            new_oid: tag.object_hash.clone(),
                            forced: true,
                        });
                    }
                    crate::internal::tag::TagImportOutcome::Preserved => {}
                }
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

/// Soft cap on the number of commits we walk back from each branch tip when
/// constructing the `have` list. Each `have` line is small, but we still want
/// to keep the request bounded for repos with deep history. Tips themselves
/// always go into `have` regardless of this limit so that the server can
/// recognise every local/remote-tracking branch as a potential common ancestor.
const HAVE_HISTORY_LIMIT: usize = 256;

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

    let mut have = Vec::new();
    let mut have_set: HashSet<String> = HashSet::new();
    let shallow_boundaries = read_shallow_boundaries()?;

    // Phase 1: every local + remote-tracking branch tip becomes a `have`,
    // unconditionally. These are the commits the server is most likely to
    // recognise as a common ancestor; dropping any of them forces the server
    // to re-send the pack regions reachable from those tips on every fetch
    // (the bug that made `libra pull` re-download the same pack repeatedly
    // on repos with more active branches than the previous traversal limit).
    for remote in &remotes {
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
            let oid = branch.commit.to_string();
            if have_set.insert(oid.clone()) {
                have.push(oid);
            }
        }
    }

    // Phase 2: walk parents in newest-first order to provide additional
    // common-ancestor candidates for divergent histories, bounded by
    // `HAVE_HISTORY_LIMIT` so very deep repos don't produce an unbounded
    // request body.
    while have.len() < HAVE_HISTORY_LIMIT && !c_pending.is_empty() {
        let Some(item) = c_pending.pop() else {
            break;
        };
        let oid = item.commit.to_string();
        if have_set.insert(oid.clone()) {
            have.push(oid);
        }
        if shallow_boundaries.contains(&item.commit.to_string()) {
            continue;
        }

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
    // Reject malformed/short lengths (1..=3) before subtracting the 4-byte
    // header, so a hostile server cannot underflow the length into a huge
    // allocation. (`0001`/`0002`/`0003` are not valid data frames here.)
    if len < 4 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid pkt-line length {len}"),
        ));
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

    use bytes::{Bytes, BytesMut};
    use futures_util::{StreamExt, stream};
    use git_internal::hash::ObjectHash;
    use tempfile::tempdir;

    use super::{
        FetchError, PackCompletionTracker, RemoteProgressBuffer, RemoteSpecErrorKind,
        SSH_KEY_TEMP_FILE_MAX_AGE, cleanup_expired_vault_ssh_temp_files_in,
        ensure_vault_ssh_tmp_dir, parse_pack_entry_data_offset, read_be_u32, read_fetch_stream,
        redact_url_credentials,
    };
    use crate::{
        internal::protocol::FetchStream,
        utils::{output::OutputConfig, test::ScopedEnvVar},
    };

    #[test]
    fn format_fetch_porcelain_layout_is_space_separated() {
        use super::{FetchOutput, FetchRefUpdate, FetchRepositoryResult, format_fetch_porcelain};

        let output = FetchOutput {
            all: false,
            requested_remote: Some("origin".to_string()),
            refspec: None,
            remotes: vec![FetchRepositoryResult {
                remote: "origin".to_string(),
                url: "https://example.com/x.git".to_string(),
                objects_fetched: 2,
                refs_updated: vec![
                    FetchRefUpdate {
                        remote_ref: "refs/remotes/origin/main".to_string(),
                        old_oid: Some("a".repeat(40)),
                        new_oid: "b".repeat(40),
                        forced: false,
                    },
                    FetchRefUpdate {
                        remote_ref: "refs/remotes/origin/dev".to_string(),
                        old_oid: None,
                        new_oid: "c".repeat(40),
                        forced: false,
                    },
                ],
                pruned: Vec::new(),
            }],
        };

        let rendered = format_fetch_porcelain(&output);
        let lines: Vec<&str> = rendered.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[0],
            format!(
                "  {} {} refs/remotes/origin/main",
                "a".repeat(40),
                "b".repeat(40)
            ),
            "fast-forward uses a space flag, then the column separator (two leading spaces)"
        );
        assert_eq!(
            lines[1],
            format!(
                "* {} {} refs/remotes/origin/dev",
                "0".repeat(40),
                "c".repeat(40)
            ),
            "new ref uses `*` and an all-zero old object id"
        );
    }

    #[test]
    fn format_fetch_porcelain_marks_forced_update_with_plus() {
        use super::{FetchOutput, FetchRefUpdate, FetchRepositoryResult, format_fetch_porcelain};

        let output = FetchOutput {
            all: false,
            requested_remote: Some("origin".to_string()),
            refspec: None,
            remotes: vec![FetchRepositoryResult {
                remote: "origin".to_string(),
                url: "https://example.com/x.git".to_string(),
                objects_fetched: 1,
                refs_updated: vec![FetchRefUpdate {
                    remote_ref: "refs/tags/v1.0".to_string(),
                    old_oid: Some("a".repeat(40)),
                    new_oid: "b".repeat(40),
                    forced: true,
                }],
                pruned: Vec::new(),
            }],
        };

        assert_eq!(
            format_fetch_porcelain(&output),
            format!("+ {} {} refs/tags/v1.0", "a".repeat(40), "b".repeat(40)),
            "a forced (clobbering) update uses the `+` flag with the previous oid"
        );
    }

    #[test]
    fn format_fetch_head_marks_not_for_merge() {
        use super::{FetchOutput, FetchRefUpdate, FetchRepositoryResult, format_fetch_head};

        let output = FetchOutput {
            all: false,
            requested_remote: Some("origin".to_string()),
            refspec: None,
            remotes: vec![FetchRepositoryResult {
                remote: "origin".to_string(),
                url: "https://example.com/x.git".to_string(),
                objects_fetched: 1,
                pruned: Vec::new(),
                refs_updated: vec![FetchRefUpdate {
                    remote_ref: "refs/remotes/origin/main".to_string(),
                    old_oid: None,
                    new_oid: "a".repeat(40),
                    forced: false,
                }],
            }],
        };

        assert_eq!(
            format_fetch_head(&output),
            format!(
                "{}\tnot-for-merge\tbranch 'main' of https://example.com/x.git",
                "a".repeat(40)
            )
        );
    }

    #[test]
    fn build_fetch_shallow_options_wires_since_and_exclude() {
        let opts = super::build_fetch_shallow_options(
            None,
            None,
            false,
            Some(1_700_000_000),
            vec!["v1.0".to_string()],
        );
        assert_eq!(opts.deepen_since, Some(1_700_000_000));
        assert_eq!(opts.deepen_not, vec!["v1.0".to_string()]);
        assert_eq!(opts.depth, None);

        // `--unshallow` still overrides everything to the max-depth request.
        let unshallow = super::build_fetch_shallow_options(None, None, true, None, vec![]);
        assert_eq!(unshallow.depth, Some(super::UNSHALLOW_DEPTH));
    }

    /// The `.libra/shallow` boundary file must be owner read/write only on Unix.
    #[cfg(unix)]
    #[tokio::test]
    #[serial_test::serial]
    async fn write_shallow_boundaries_sets_0600_permissions() {
        use std::{collections::BTreeSet, os::unix::fs::PermissionsExt};

        let temp = tempdir().unwrap();
        crate::utils::test::setup_with_new_libra_in(temp.path()).await;
        let _guard = crate::utils::test::ChangeDirGuard::new(temp.path());

        let mut boundaries = BTreeSet::new();
        boundaries.insert("a".repeat(40));
        super::write_shallow_boundaries(&boundaries).expect("write shallow boundaries");

        let path = super::shallow_file_path().expect("resolve shallow path");
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "shallow metadata must be 0o600");
    }

    /// Pin the `Display` format for the static-message and direct-message
    /// variants of [`FetchError`]. These strings are used as the
    /// `CliError` message via `From<FetchError> for CliError` and
    /// surface in both human and `--json` envelopes for `fetch`, `clone`,
    /// and `pull`.
    ///
    /// Source-chained variants (Discovery, FetchObjects, PacketRead,
    /// ObjectsDirNotFound, PackDirCreate, PackWrite, IndexPack) wrap
    /// upstream io::Error / GitError types and are intentionally
    /// skipped — their `{source}` slot is owned by the wrapped type.
    #[test]
    fn fetch_error_display_pins_static_message_variants() {
        // InvalidRemoteSpec echoes the `reason` field verbatim.
        assert_eq!(
            FetchError::InvalidRemoteSpec {
                spec: "/missing/repo".to_string(),
                kind: RemoteSpecErrorKind::MissingLocalRepo,
                reason: "local path does not exist".to_string(),
            }
            .to_string(),
            "local path does not exist",
        );
        assert_eq!(
            FetchError::ObjectFormatMismatch {
                remote: git_internal::hash::HashKind::Sha1,
                local: git_internal::hash::HashKind::Sha256,
            }
            .to_string(),
            "remote object format 'sha1' does not match local 'sha256'",
        );
        assert_eq!(
            FetchError::RemoteBranchNotFound {
                branch: "feature".to_string(),
                remote: "origin".to_string(),
            }
            .to_string(),
            "remote branch feature not found in upstream origin",
        );
        assert_eq!(
            FetchError::InvalidPktHeader {
                header: "zzzz".to_string(),
            }
            .to_string(),
            "invalid packet line header 'zzzz'",
        );
        assert_eq!(
            FetchError::RemoteSideband {
                message: "access denied".to_string(),
            }
            .to_string(),
            "remote reported an error: access denied",
        );
        assert_eq!(
            FetchError::ChecksumMismatch.to_string(),
            "pack checksum mismatch",
        );
        assert_eq!(
            FetchError::UpdateRefs {
                message: "ref database is read-only".to_string(),
            }
            .to_string(),
            "failed to update references after fetch: ref database is read-only",
        );
        assert_eq!(
            FetchError::LocalState {
                message: "missing object directory".to_string(),
            }
            .to_string(),
            "failed to inspect local repository state: missing object directory",
        );
    }

    fn append_pkt_line(buf: &mut BytesMut, payload: &[u8]) {
        let len = payload.len() + 4;
        buf.extend_from_slice(format!("{len:04x}").as_bytes());
        buf.extend_from_slice(payload);
    }

    fn empty_pack_bytes() -> Vec<u8> {
        let mut pack = Vec::new();
        pack.extend_from_slice(b"PACK");
        pack.extend_from_slice(&2_u32.to_be_bytes());
        pack.extend_from_slice(&0_u32.to_be_bytes());
        let checksum = ObjectHash::new(&pack);
        pack.extend_from_slice(checksum.as_ref());
        pack
    }

    #[tokio::test]
    async fn read_fetch_stream_accepts_eof_after_complete_pack_without_flush() {
        let pack = empty_pack_bytes();
        let mut response = BytesMut::new();
        append_pkt_line(&mut response, b"NAK\n");

        let mut sideband = Vec::with_capacity(pack.len() + 1);
        sideband.push(1);
        sideband.extend_from_slice(&pack);
        append_pkt_line(&mut response, &sideband);

        let mut stream: FetchStream =
            stream::iter(vec![Ok::<Bytes, std::io::Error>(response.freeze())]).boxed();
        let output = OutputConfig::default();

        let data = read_fetch_stream(&mut stream, &output, "fetch origin", false)
            .await
            .expect("EOF after a complete pack should finish the fetch stream");

        assert_eq!(data.pack_data, pack);
    }

    #[tokio::test]
    async fn read_fetch_stream_consumes_empty_shallow_section_before_pack() {
        // A deepen/shallow request whose result cuts no boundary still yields a
        // shallow section: a leading flush packet before NAK + pack. The reader
        // must consume it instead of treating it as end-of-stream.
        let pack = empty_pack_bytes();
        let mut response = BytesMut::new();
        // Empty shallow section terminator.
        response.extend_from_slice(b"0000");
        append_pkt_line(&mut response, b"NAK\n");

        let mut sideband = Vec::with_capacity(pack.len() + 1);
        sideband.push(1);
        sideband.extend_from_slice(&pack);
        append_pkt_line(&mut response, &sideband);

        let mut stream: FetchStream =
            stream::iter(vec![Ok::<Bytes, std::io::Error>(response.freeze())]).boxed();
        let output = OutputConfig::default();

        let data = read_fetch_stream(&mut stream, &output, "fetch origin", true)
            .await
            .expect("empty shallow section must not be mistaken for end-of-stream");

        assert_eq!(data.pack_data, pack);
    }

    #[tokio::test]
    async fn read_fetch_stream_finishes_complete_pack_when_transport_stays_open() {
        let pack = empty_pack_bytes();
        let mut response = BytesMut::new();
        append_pkt_line(&mut response, b"NAK\n");

        let mut sideband = Vec::with_capacity(pack.len() + 1);
        sideband.push(1);
        sideband.extend_from_slice(&pack);
        append_pkt_line(&mut response, &sideband);

        let mut stream: FetchStream =
            stream::iter(vec![Ok::<Bytes, std::io::Error>(response.freeze())])
                .chain(stream::pending())
                .boxed();
        let output = OutputConfig::default();

        let data = tokio::time::timeout(
            Duration::from_millis(250),
            read_fetch_stream(&mut stream, &output, "fetch origin", false),
        )
        .await
        .expect("complete pack should not wait for transport EOF or flush")
        .expect("complete pack should finish the fetch stream");

        assert_eq!(data.pack_data, pack);
    }

    #[tokio::test]
    async fn read_fetch_stream_finishes_non_empty_pack_when_transport_stays_open() {
        let pack = include_bytes!("../../tests/data/packs/small-sha1.pack").to_vec();
        let mut response = BytesMut::new();
        append_pkt_line(&mut response, b"NAK\n");

        let mut sideband = Vec::with_capacity(pack.len() + 1);
        sideband.push(1);
        sideband.extend_from_slice(&pack);
        append_pkt_line(&mut response, &sideband);

        let mut stream: FetchStream =
            stream::iter(vec![Ok::<Bytes, std::io::Error>(response.freeze())])
                .chain(stream::pending())
                .boxed();
        let output = OutputConfig::default();

        let data = tokio::time::timeout(
            Duration::from_millis(250),
            read_fetch_stream(&mut stream, &output, "fetch origin", false),
        )
        .await
        .expect("complete non-empty pack should not wait for transport EOF or flush")
        .expect("complete non-empty pack should finish the fetch stream");

        assert_eq!(data.pack_data, pack);
    }

    /// Drive `RemoteProgressBuffer` with `payload` and return
    /// `(permanent_lines, transient_lines)` in dispatch order.
    fn collect_buffered_progress(
        buffer: &mut RemoteProgressBuffer,
        payload: &[u8],
    ) -> (Vec<String>, Vec<String>) {
        let mut perm = Vec::new();
        let mut trans = Vec::new();
        buffer.push(
            payload,
            |line| perm.push(line.to_string()),
            |line| trans.push(line.to_string()),
        );
        (perm, trans)
    }

    /// `\n`-terminated chunks are promoted to permanent log lines so the
    /// remote's `Counting objects: 100% (38/38), done.` survives above the bar.
    #[test]
    fn remote_progress_buffer_promotes_newline_terminated_lines() {
        let mut buffer = RemoteProgressBuffer::default();
        let (perm, trans) =
            collect_buffered_progress(&mut buffer, b"Counting objects: 100% (38/38), done.\n");

        assert_eq!(perm, vec!["Counting objects: 100% (38/38), done."]);
        assert!(trans.is_empty());
    }

    /// `\r`-terminated chunks update the bar message in place so successive
    /// `Counting objects:  5%\rCounting objects: 10%\r…` updates replace each
    /// other instead of stacking as separate lines.
    #[test]
    fn remote_progress_buffer_routes_carriage_returns_to_transient() {
        let mut buffer = RemoteProgressBuffer::default();
        let (perm, trans) = collect_buffered_progress(
            &mut buffer,
            b"Counting objects:  5%\rCounting objects: 10%\r",
        );

        assert!(perm.is_empty());
        assert_eq!(
            trans,
            vec!["Counting objects:  5%", "Counting objects: 10%"]
        );
    }

    /// Side-band chunks may split mid-word; partial bytes must survive until
    /// the next push delivers the terminator.
    #[test]
    fn remote_progress_buffer_holds_partial_bytes_across_pushes() {
        let mut buffer = RemoteProgressBuffer::default();
        let (perm1, trans1) = collect_buffered_progress(&mut buffer, b"Counting");
        assert!(perm1.is_empty());
        assert!(trans1.is_empty());

        let (perm2, trans2) = collect_buffered_progress(&mut buffer, b" objects: 100%, done.\n");
        assert_eq!(perm2, vec!["Counting objects: 100%, done."]);
        assert!(trans2.is_empty());
    }

    /// CRLF must collapse to a single permanent line so we don't emit a
    /// spurious empty transient followed by an empty permanent.
    #[test]
    fn remote_progress_buffer_treats_crlf_as_single_newline() {
        let mut buffer = RemoteProgressBuffer::default();
        let (perm, trans) = collect_buffered_progress(&mut buffer, b"Compressing done.\r\n");

        assert_eq!(perm, vec!["Compressing done."]);
        assert!(trans.is_empty());
    }

    /// At end of stream any unterminated tail must be flushed so a remote
    /// that closes mid-line still surfaces the partial message.
    #[test]
    fn remote_progress_buffer_flush_remaining_emits_trailing_partial() {
        let mut buffer = RemoteProgressBuffer::default();
        collect_buffered_progress(&mut buffer, b"Resolving deltas: 99%");
        let mut tail = Vec::new();
        buffer.flush_remaining(|line| tail.push(line.to_string()));

        assert_eq!(tail, vec!["Resolving deltas: 99%"]);
    }

    /// Empty payloads (e.g. a bare side-band code with no body) must not push
    /// anything through the line splitter.
    #[test]
    fn remote_progress_buffer_ignores_empty_payload() {
        let mut buffer = RemoteProgressBuffer::default();
        let (perm, trans) = collect_buffered_progress(&mut buffer, b"");
        assert!(perm.is_empty());
        assert!(trans.is_empty());
    }

    #[test]
    fn redact_url_credentials_strips_file_url_userinfo() {
        let redacted = redact_url_credentials("file://user:secret@example.com/repo.git");

        assert_eq!(redacted, "file://example.com/repo.git");
    }

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

    /// `read_be_u32` returns `None` for any range that would overflow
    /// the input slice; it must not panic. Pins the `offset + 4 > len`
    /// short-circuit added with `PackCompletionTracker` in v0.17.1060.
    #[test]
    fn read_be_u32_decodes_big_endian_and_short_circuits_on_overflow() {
        // Happy path: 4 BE bytes at offset 0.
        assert_eq!(read_be_u32(&[0x00, 0x00, 0x00, 0x05], 0), Some(5));
        assert_eq!(read_be_u32(&[0xDE, 0xAD, 0xBE, 0xEF], 0), Some(0xDEAD_BEEF));
        // Happy path: 4 BE bytes at a non-zero offset.
        assert_eq!(read_be_u32(&[0xAA, 0x00, 0x00, 0x00, 0x07], 1), Some(7));
        // Short input: only 3 bytes available at offset 0.
        assert_eq!(read_be_u32(&[0x00, 0x00, 0x00], 0), None);
        // Offset past end.
        assert_eq!(read_be_u32(&[0x00, 0x00, 0x00, 0x00], 4), None);
        // Empty input.
        assert_eq!(read_be_u32(&[], 0), None);
    }

    /// `PackCompletionTracker::read_header` accepts well-formed PACK v2
    /// and v3 headers and rejects everything else without panicking.
    /// The state mutations (`object_count`, `offset`) are part of the
    /// public contract that `observe` relies on, so pin them here.
    #[test]
    fn pack_completion_tracker_read_header_validates_magic_version_and_state() {
        // Reject: empty input.
        let mut tracker = PackCompletionTracker::default();
        assert!(!tracker.read_header(&[]));
        assert_eq!(tracker.object_count, None);

        // Reject: less than 12 bytes (header is exactly 12).
        let mut tracker = PackCompletionTracker::default();
        let short = [b'P', b'A', b'C', b'K', 0, 0, 0, 2, 0, 0, 0];
        assert!(!tracker.read_header(&short));
        assert_eq!(tracker.object_count, None);

        // Reject: wrong magic bytes.
        let mut tracker = PackCompletionTracker::default();
        let mut bad_magic = b"PACX".to_vec();
        bad_magic.extend_from_slice(&2_u32.to_be_bytes());
        bad_magic.extend_from_slice(&0_u32.to_be_bytes());
        assert!(!tracker.read_header(&bad_magic));

        // Reject: unsupported version (1 — packs predate widespread use).
        let mut tracker = PackCompletionTracker::default();
        let mut bad_version = b"PACK".to_vec();
        bad_version.extend_from_slice(&1_u32.to_be_bytes());
        bad_version.extend_from_slice(&0_u32.to_be_bytes());
        assert!(!tracker.read_header(&bad_version));

        // Reject: unsupported version (4).
        let mut tracker = PackCompletionTracker::default();
        let mut bad_version = b"PACK".to_vec();
        bad_version.extend_from_slice(&4_u32.to_be_bytes());
        bad_version.extend_from_slice(&0_u32.to_be_bytes());
        assert!(!tracker.read_header(&bad_version));

        // Accept: PACK v2 with 0 objects; `offset` advances to 12.
        let mut tracker = PackCompletionTracker::default();
        let mut empty_v2 = b"PACK".to_vec();
        empty_v2.extend_from_slice(&2_u32.to_be_bytes());
        empty_v2.extend_from_slice(&0_u32.to_be_bytes());
        assert!(tracker.read_header(&empty_v2));
        assert_eq!(tracker.object_count, Some(0));
        assert_eq!(tracker.offset, 12);

        // Accept: PACK v3 with 7 objects.
        let mut tracker = PackCompletionTracker::default();
        let mut seven_v3 = b"PACK".to_vec();
        seven_v3.extend_from_slice(&3_u32.to_be_bytes());
        seven_v3.extend_from_slice(&7_u32.to_be_bytes());
        assert!(tracker.read_header(&seven_v3));
        assert_eq!(tracker.object_count, Some(7));
        assert_eq!(tracker.offset, 12);
    }

    /// `parse_pack_entry_data_offset` returns `None` when the entry
    /// header runs past the end of the slice and `Some(offset)`
    /// pointing past the variable-length size header for the
    /// happy-path object types (1..=4 = commit/tree/blob/tag).
    #[test]
    fn parse_pack_entry_data_offset_returns_data_start_for_simple_object() {
        // Single byte header: type=3 (blob = 0b011), size <= 15, no
        // continuation bit. First byte: 0b0_011_0000 = 0x30 (size 0).
        // `data_offset` should equal `offset + 1`.
        let entry = [0x30_u8, 0x78, 0x9C]; // 0x78 0x9C = zlib stream begin
        assert_eq!(parse_pack_entry_data_offset(&entry, 0, 20), Some(1));

        // Two-byte size header: first byte has continuation bit set
        // (0b1_011_0000 = 0xB0), second byte is the last size chunk
        // (0b0_0000001 = 0x01). data_offset = 2.
        let entry = [0xB0_u8, 0x01, 0x78, 0x9C];
        assert_eq!(parse_pack_entry_data_offset(&entry, 0, 20), Some(2));

        // Reject: header truncated mid-continuation.
        let entry = [0xB0_u8]; // says "continue" but nothing follows
        assert_eq!(parse_pack_entry_data_offset(&entry, 0, 20), None);

        // Reject: unknown object type (5 is reserved, not 1..=4 / 6 / 7).
        let entry = [0x50_u8]; // 0b0_101_0000 = type 5
        assert_eq!(parse_pack_entry_data_offset(&entry, 0, 20), None);
    }
}
