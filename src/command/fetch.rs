//! Fetch command to negotiate with remotes, download pack data, update
//! remote-tracking refs, and honor prune/depth options.

use std::{
    collections::HashSet,
    fs,
    io::{self, Error as IoError, Write},
    path::PathBuf,
    time::Instant,
};

use clap::Parser;
use git_internal::{
    errors::GitError,
    hash::{HashKind, ObjectHash, get_hash_kind},
    internal::object::commit::Commit,
};
use indicatif::ProgressBar;
use sea_orm::TransactionTrait;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio_util::io::StreamReader;
use url::Url;

use crate::{
    command::{index_pack, load_object},
    git_protocol::ServiceType::{self, UploadPack},
    internal::{
        branch::Branch,
        config::{Config, RemoteConfig},
        db::get_db_conn_instance,
        head::Head,
        protocol::{
            DiscRef, DiscoveryResult, FetchStream, ProtocolClient, git_client::GitClient,
            https_client::HttpsClient, local_client::LocalClient, set_wire_hash_kind,
        },
        reflog::{HEAD, Reflog, ReflogAction, ReflogContext, ReflogError},
    },
    utils::{
        error::{CliError, CliResult},
        path, util,
    },
};

pub(crate) enum RemoteClient {
    Http(HttpsClient),
    Local(LocalClient),
    Git(GitClient),
}

impl RemoteClient {
    pub(crate) fn from_spec(spec: &str) -> Result<Self, String> {
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
                other => Err(format!("unsupported remote scheme '{other}'")),
            }
        } else {
            if spec.contains("://") || spec.contains('@') {
                return Err(format!(
                    "unsupported remote specification '{spec}': protocol not implemented"
                ));
            }
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
        }
    }
}

#[derive(Parser, Debug)]
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

#[derive(thiserror::Error, Debug)]
pub enum FetchError {
    #[error("{reason}")]
    InvalidRemoteSpec { spec: String, reason: String },
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
        CliError::fatal(error.to_string())
    }
}

pub async fn execute(args: FetchArgs) {
    if let Err(err) = execute_safe(args).await {
        eprintln!("{}", err.render());
    }
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting. Negotiates with remotes, downloads pack data, and
/// updates remote-tracking refs.
pub async fn execute_safe(args: FetchArgs) -> CliResult<()> {
    tracing::debug!("`fetch` args: {:?}", args);

    if args.all {
        for remote in Config::all_remote_configs().await {
            fetch_repository_safe(remote, None, false, None)
                .await
                .map_err(CliError::from)?;
        }
        return Ok(());
    }

    let remote = match args.repository {
        Some(remote) => remote,
        None => match Config::get_current_remote().await {
            Ok(Some(remote)) => remote,
            Ok(None) => {
                return Err(CliError::fatal(
                    "no configured remote for the current branch",
                ));
            }
            Err(_) => return Err(CliError::fatal("HEAD is detached")),
        },
    };

    let remote_config = Config::remote_config(&remote).await.ok_or_else(|| {
        CliError::fatal(format!(
            "'{}' does not appear to be a libra repository",
            remote
        ))
    })?;

    fetch_repository_safe(remote_config, args.refspec, false, None)
        .await
        .map_err(CliError::from)
}

pub(crate) async fn discover_remote(
    remote_spec: &str,
) -> Result<(RemoteClient, DiscoveryResult), FetchError> {
    let remote_client =
        RemoteClient::from_spec(remote_spec).map_err(|message| FetchError::InvalidRemoteSpec {
            spec: remote_spec.to_string(),
            reason: format_remote_spec_error(remote_spec, &message),
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

fn format_remote_spec_error(remote_spec: &str, message: &str) -> String {
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
            return format!("repository '{}' does not exist", display);
        }
        return format!("'{}' does not appear to be a libra repository", display);
    }
    message.to_string()
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
        RemoteClient::Http(_) | RemoteClient::Git(_) => remote_input.to_string(),
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
    if let Err(err) = fetch_repository_safe(remote_config, branch, single_branch, depth).await {
        eprintln!("{}", CliError::from(err).render());
    }
}

pub async fn fetch_repository_safe(
    remote_config: RemoteConfig,
    branch: Option<String>,
    single_branch: bool,
    depth: Option<usize>,
) -> Result<(), FetchError> {
    let (remote_client, discovery) = discover_remote(&remote_config.url).await?;
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
        return Ok(());
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

    let pack_data = read_fetch_stream(&mut result_stream).await?;
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

    update_references(&remote_config, &refs, &ref_heads, remote_head, branch).await
}

async fn read_fetch_stream(result_stream: &mut FetchStream) -> Result<Vec<u8>, FetchError> {
    let mut reader = StreamReader::new(result_stream);
    let mut pack_data = Vec::new();
    let mut reach_pack = false;
    let bar = ProgressBar::new_spinner();
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
                        bar.set_message(format!("Receiving objects: {total:.2} | {bps:.2}/s"));
                        bar.tick();
                        pack_data.extend(payload);
                    }
                    2 => print_remote_progress(payload),
                    3 => {
                        bar.finish_and_clear();
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
                2 => print_remote_progress(payload),
                3 => {
                    bar.finish_and_clear();
                    return Err(FetchError::RemoteSideband {
                        message: String::from_utf8_lossy(payload).trim().to_string(),
                    });
                }
                _ => {
                    let text = String::from_utf8_lossy(&data);
                    eprint!("{text}");
                    let _ = io::stderr().flush();
                }
            }
        }
    }
    bar.finish_and_clear();

    Ok(pack_data)
}

fn print_remote_progress(payload: &[u8]) {
    let text = String::from_utf8_lossy(payload);
    eprint!("{text}");
    let _ = io::stderr().flush();
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
) -> Result<(), FetchError> {
    let db = get_db_conn_instance().await;
    let remote_config = remote_config.clone();
    let refs = refs.to_vec();
    let ref_heads = ref_heads.to_vec();
    db.transaction(|txn| {
        Box::pin(async move {
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

                let old_oid =
                    Branch::find_branch_with_conn(txn, &full_ref_name, Some(&remote_config.name))
                        .await
                        .map_or(ObjectHash::zero_str(get_hash_kind()), |branch| {
                            branch.commit.to_string()
                        });

                Branch::update_branch_with_conn(
                    txn,
                    &full_ref_name,
                    &reference._hash,
                    Some(&remote_config.name),
                )
                .await;

                let context = ReflogContext {
                    old_oid: old_oid.to_string(),
                    new_oid: reference._hash.clone(),
                    action: ReflogAction::Fetch,
                };
                Reflog::insert_single_entry(txn, &context, &full_ref_name).await?;
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

            Ok::<_, ReflogError>(())
        })
    })
    .await
    .map_err(|source| FetchError::UpdateRefs {
        message: source.to_string(),
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

    let mut remotes = Config::all_remote_configs()
        .await
        .iter()
        .map(|remote| Some(remote.name.to_owned()))
        .collect::<Vec<_>>();
    remotes.push(None);

    for remote in remotes {
        let branches = Branch::list_branches(remote.as_deref()).await;
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
