//! Local protocol client using filesystem paths to run upload-pack/receive-pack locally and stream pack data over async pipes.

use std::{
    env,
    collections::HashSet,
    io::Error as IoError,
    path::{Path, PathBuf},
};

use bytes::Bytes;
use futures_util::stream::{self, StreamExt};
use git_internal::{
    errors::GitError,
    hash::get_hash_kind,
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
use tokio::{io::AsyncWriteExt, process::Command};
use url::Url;

use super::{
    DiscoveryResult, FetchStream, ProtocolClient, generate_upload_pack_content,
    parse_discovered_references,
};
use crate::{
    command::{load_object, log::get_reachable_commits},
    git_protocol::ServiceType,
    internal::{branch::Branch, config::Config, head::Head, protocol::DiscRef, reflog},
    utils::{
        object_ext::TreeExt,
        util::cur_dir,
    },
};

#[derive(Debug, Clone)]
enum RepoType {
    GitRepo,
    LibraRepo,
}

#[derive(Debug, Clone)]
pub struct LocalClient {
    repo_path: PathBuf,
    source_type: RepoType,
}

impl ProtocolClient for LocalClient {
    fn from_url(url: &Url) -> Self {
        let path = url
            .to_file_path()
            .unwrap_or_else(|_| PathBuf::from(url.path()));
        Self {
            repo_path: path.clone(),
            source_type: {
                if path.join("libra.db").try_exists().unwrap_or(false) || path.join(".libra/libra.db").try_exists().unwrap_or(false) {
                    RepoType::LibraRepo
                } else {
                    RepoType::GitRepo
                }
            }
        }
    }
}

impl LocalClient {
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, IoError> {
        let path = path.as_ref();
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            cur_dir().join(path)
        };
        if !absolute.try_exists().unwrap_or(false) {
            return Err(IoError::other(format!(
                "Local repository path does not exist: {}",
                absolute.display()
            )));
        }
        if absolute.join("objects").try_exists().unwrap_or(false) {
            let is_libra_repo = absolute.join("libra.db").try_exists().unwrap_or(false);
            let is_git_repo = absolute.join("HEAD").try_exists().unwrap_or(false);
            match (is_libra_repo, is_git_repo) {
                (true, false) => {
                    Ok(Self {
                        repo_path: absolute,
                        source_type: RepoType::LibraRepo
                    })
                },
                (false, true) => {
                    Ok(Self {
                        repo_path: absolute,
                        source_type: RepoType::GitRepo
                    })
                },
                _ => Err(IoError::other(format!(
                    "No valid Git directory structure found at: {}",
                    absolute.display()
                )))
            }
        } else if absolute.join(".git/HEAD").try_exists().unwrap_or(false) {
            Ok(Self {
                repo_path: absolute.join(".git"),
                source_type: RepoType::GitRepo,
            })
        } else if absolute.join(".libra/libra.db").try_exists().unwrap_or(false) {
            Ok(Self {
                repo_path: absolute.join(".libra"),
                source_type: RepoType::LibraRepo,
            })
        } else {
            Err(IoError::other(format!(
                "No valid Git directory structure found at: {}",
                absolute.display()
            )))
        }
    }

    pub fn repo_path(&self) -> &Path {
        &self.repo_path
    }

    pub async fn discovery_reference(
        &self,
        service: ServiceType,
    ) -> Result<DiscoveryResult, GitError> {
        if service != ServiceType::UploadPack {
            return Err(GitError::NetworkError(
                "Unsupported service type for local protocol".to_string(),
            ));
        }
        match self.source_type {
            RepoType::GitRepo => {
                let output = Command::new("git-upload-pack")
                    .arg("--advertise-refs")
                    .arg(&self.repo_path)
                    .output()
                    .await
                    .map_err(|e| {
                        GitError::NetworkError(format!(
                            "Failed to spawn git-upload-pack for discovery: {}",
                            e
                        ))
                    })?;
                if !output.status.success() {
                    return Err(GitError::NetworkError(format!(
                        "git-upload-pack --advertise-refs failed: {}",
                        String::from_utf8_lossy(&output.stderr)
                    )));
                }
                let bytes = Bytes::from(output.stdout);
                parse_discovered_references(bytes, service)
            }
            RepoType::LibraRepo => {
                let original_dir = cur_dir();
                env::set_current_dir(&self.repo_path)?;
                let local_branches = Branch::list_branches(None).await;

                let remote_configs = Config::all_remote_configs().await;
                let mut remote_branches: Vec<_> = vec![];
                for remote in remote_configs {
                    remote_branches.extend(Branch::list_branches(Some(&remote.name)).await);
                }
                let result =
                    DiscoveryResult {
                        refs: local_branches
                            .into_iter()
                            .chain(remote_branches.into_iter())
                            .map(Into::into)
                            .chain(Head::current_commit().await.map(|x| x.to_string()).map(
                                |hash| DiscRef {
                                    _hash: hash,
                                    _ref: reflog::HEAD.to_string(),
                                },
                            ))
                            .collect::<Vec<_>>(),
                        capabilities: vec![],
                        hash_kind: get_hash_kind(),
                    };
                env::set_current_dir(original_dir)?;
                Ok(result)
            }
        }
    }

    pub async fn fetch_objects(
        &self,
        have: &[String],
        want: &[String],
        depth: Option<usize>,
    ) -> Result<FetchStream, IoError> {
        match self.source_type {
            RepoType::GitRepo => {
                let body = generate_upload_pack_content(have, want, depth);
                let mut child = Command::new("git-upload-pack");
                child.arg("--stateless-rpc");
                child.arg(&self.repo_path);
                child.stdin(std::process::Stdio::piped());
                child.stdout(std::process::Stdio::piped());
                child.stderr(std::process::Stdio::piped());
                let mut child = child
                    .spawn()
                    .map_err(|e| IoError::other(format!("Failed to spawn git-upload-pack: {e}")))?;

                if let Some(mut stdin) = child.stdin.take() {
                    stdin.write_all(&body).await?;
                } else {
                    return Err(IoError::other(
                        "Failed to capture stdin for git-upload-pack process",
                    ));
                }

                let output = child.wait_with_output().await.map_err(|e| {
                    IoError::other(format!("Failed to wait for git-upload-pack: {e}"))
                })?;
                if !output.status.success() {
                    tracing::error!(
                        "git-upload-pack stderr: {}",
                        String::from_utf8_lossy(&output.stderr)
                    );
                    return Err(IoError::other("git-upload-pack exited with failure"));
                }
                let stdout = Bytes::from(output.stdout);
                Ok(stream::once(async move { Ok(stdout) }).boxed())
            }
            RepoType::LibraRepo => {
                let original_dir = cur_dir();
                env::set_current_dir(&self.repo_path)?;

                let mut seen = HashSet::new();
                have.iter()
                    .for_each(|hash| {
                        seen.insert(hash.clone());
                    });

                let commits = stream::iter(want)
                    .then(|branch_hash| async move { get_reachable_commits(branch_hash.to_string(), depth).await })
                    .flat_map(stream::iter)
                    .collect::<Vec<Commit>>()
                    .await
                    .into_iter()
                    .filter(|c| seen.insert(c.id.to_string()))
                    .collect::<Vec<_>>();

                let (tree_hash, blob_hash): (Vec<_>, Vec<_>) = commits
                    .iter()
                    .map(|commit| &commit.tree_id)
                    .map(load_object::<Tree>)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|giterror| match giterror {
                        GitError::IOError(io_error) => io_error,
                        _ => IoError::other(format!("{}", giterror)),
                    })?
                    .into_iter()
                    .flat_map(|t| {
                        t.get_items_with_mode()
                            .into_iter()
                            .map(|(_, hash, mode)| (hash, mode))
                    })
                    .filter(|(hash, _)| seen.insert(hash.to_string()))
                    .partition(|(_, mode)| *mode == TreeItemMode::Tree);

                let trees = tree_hash
                    .into_iter()
                    .map(|(hash, _)| load_object::<Tree>(&hash))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|giterror| match giterror {
                        GitError::IOError(io_error) => io_error,
                        _ => IoError::other(format!("{}", giterror)),
                    })?;

                let blobs = blob_hash
                    .into_iter()
                    .map(|(hash, _)| load_object::<Blob>(&hash))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|giterror| match giterror {
                        GitError::IOError(io_error) => io_error,
                        _ => IoError::other(format!("{}", giterror)),
                    })?;

                let commit_entries: Vec<Entry> = commits.into_iter().map(Entry::from).collect();

                let tree_entries: Vec<Entry> = trees.into_iter().map(Entry::from).collect();

                let blob_entries: Vec<Entry> = blobs.into_iter().map(Entry::from).collect();

                let mut all_entries = Vec::new();
                all_entries.extend(commit_entries);
                all_entries.extend(tree_entries);
                all_entries.extend(blob_entries);

                let (entry_tx, entry_rx) =
                    tokio::sync::mpsc::channel::<MetaAttached<Entry, EntryMeta>>(1_000);
                let (stream_tx, mut stream_rx) = tokio::sync::mpsc::channel(1_000);

                let total_objects = all_entries.len();
                let window_size = 0;

                let encoder = PackEncoder::new(total_objects, window_size, stream_tx);

                let encode_handle = encoder
                    .encode_async(entry_rx)
                    .await
                    .map_err(|e| IoError::other(format!("Failed to start encoding: {}", e)))?;

                for entry in all_entries {
                    let entry_meta = EntryMeta::default();
                    let meta_entry = MetaAttached {
                        inner: entry,
                        meta: entry_meta,
                    };

                    if let Err(e) = entry_tx.send(meta_entry).await {
                        return Err(IoError::other(format!("Failed to send entry: {}", e)));
                    }
                }

                drop(entry_tx);

                let mut pack_data = Vec::new();
                while let Some(chunk) = stream_rx.recv().await {
                    pack_data.extend(chunk);
                }

                encode_handle
                    .await
                    .map_err(|e| IoError::other(format!("Encode task panicked: {}", e)))?;

                if pack_data.len() < 12 {
                    return Err(IoError::other("Pack data too short"));
                }

                if &pack_data[0..4] != b"PACK" {
                    return Err(IoError::other("Invalid pack signature"));
                }

                let mut response_data = Vec::new();

                let nak_line = "NAK\n";
                let nak_len = nak_line.len() + 4;
                let nak_len_hex = format!("{:04x}", nak_len);
                response_data.extend_from_slice(nak_len_hex.as_bytes());
                response_data.extend_from_slice(nak_line.as_bytes());

                let chunk_size = 65500;
                for chunk in pack_data.chunks(chunk_size) {
                    let mut sideband_data = Vec::with_capacity(1 + chunk.len());
                    sideband_data.push(1);
                    sideband_data.extend_from_slice(chunk);

                    let total_len = sideband_data.len() + 4;
                    let len_hex = format!("{:04x}", total_len);

                    response_data.extend_from_slice(len_hex.as_bytes());
                    response_data.extend_from_slice(&sideband_data);

                    // Send progress update every ~10 chunks (approximately 655KB)
                    const PROGRESS_CHUNK_INTERVAL: usize = 10;
                    if response_data.len() % (chunk_size * PROGRESS_CHUNK_INTERVAL) == 0 {
                        let progress_msg =
                            format!("Pack {}/{}...\n", response_data.len(), pack_data.len());
                        let mut progress_data = Vec::with_capacity(1 + progress_msg.len());
                        progress_data.push(2);
                        progress_data.extend_from_slice(progress_msg.as_bytes());

                        let progress_len = progress_data.len() + 4;
                        let progress_len_hex = format!("{:04x}", progress_len);
                        response_data.extend_from_slice(progress_len_hex.as_bytes());
                        response_data.extend_from_slice(&progress_data);
                    }
                }

                response_data.extend_from_slice(b"0000");

                let response_stream = stream::iter(vec![Ok(Bytes::from(response_data))]);

                env::set_current_dir(&original_dir)?;
                Ok(Box::pin(response_stream))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsStr, process::Command as StdCommand};

    use tempfile::tempdir;
    use tokio::io::AsyncReadExt;
    use tokio_util::io::StreamReader;

    use super::*;
    use crate::git_protocol::ServiceType;

    fn run_git<I, S>(cwd: Option<&Path>, args: I) -> StdCommand
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut cmd = StdCommand::new("git");
        if let Some(path) = cwd {
            cmd.current_dir(path);
        }
        cmd.args(args);
        cmd
    }

    #[tokio::test]
    async fn discovery_reference_empty_repo_returns_refs() {
        let dir = tempdir().unwrap();
        let repo_path = dir.path().join("empty.git");
        run_git(None, ["init", "--bare", repo_path.to_str().unwrap()])
            .status()
            .unwrap();

        let client = LocalClient::from_path(&repo_path).unwrap();
        let refs = client
            .discovery_reference(ServiceType::UploadPack)
            .await
            .unwrap();
        assert!(refs.refs.is_empty());
    }

    #[tokio::test]
    async fn fetch_objects_produces_pack_stream() {
        let temp = tempdir().unwrap();
        let remote_path = temp.path().join("remote.git");
        let work_path = temp.path().join("work");

        assert!(
            run_git(None, ["init", "--bare", remote_path.to_str().unwrap()])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            run_git(None, ["init", work_path.to_str().unwrap()])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            run_git(Some(&work_path), ["config", "user.name", "Local Tester"])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            run_git(Some(&work_path), ["config", "user.email", "local@test"])
                .status()
                .unwrap()
                .success()
        );
        std::fs::write(work_path.join("README.md"), "hello world").unwrap();
        assert!(
            run_git(Some(&work_path), ["add", "README.md"])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            run_git(Some(&work_path), ["commit", "-m", "initial commit"])
                .status()
                .unwrap()
                .success()
        );

        let branch = String::from_utf8(
            run_git(Some(&work_path), ["rev-parse", "--abbrev-ref", "HEAD"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();

        assert!(
            run_git(
                Some(&work_path),
                ["remote", "add", "origin", remote_path.to_str().unwrap()],
            )
            .status()
            .unwrap()
            .success()
        );
        assert!(
            run_git(
                Some(&work_path),
                ["push", "origin", &format!("HEAD:refs/heads/{branch}"),],
            )
            .status()
            .unwrap()
            .success()
        );

        let head = String::from_utf8(
            run_git(Some(&work_path), ["rev-parse", "HEAD"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();

        let client = LocalClient::from_path(&remote_path).unwrap();
        let refs = client
            .discovery_reference(ServiceType::UploadPack)
            .await
            .unwrap();
        assert!(!refs.refs.is_empty());

        let want = vec![head];
        let have = Vec::new();
        let stream = client.fetch_objects(&have, &want, None).await.unwrap();
        let mut reader = StreamReader::new(stream);
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await.unwrap();
        assert!(buf.windows(4).any(|w| w == b"PACK"));
    }
}
