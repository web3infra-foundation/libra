use std::{
    io::{self, Read, Write},
    path::PathBuf,
    sync::{Arc, mpsc},
};

use flate2::{Compression, read::ZlibDecoder, write::ZlibEncoder};
use git_internal::{
    errors::GitError,
    hash::ObjectHash,
    internal::object::{commit::Commit, types::ObjectType},
};
use once_cell::sync::Lazy;
use regex::Regex;
use tokio::runtime::Runtime;

use crate::{
    command::load_object,
    internal::{branch::Branch, head::Head},
    utils::storage::{Storage, local::LocalStorage, remote::RemoteStorage, tiered::TieredStorage},
};

// Dedicated runtime for storage operations to avoid blocking/deadlocks in the main runtime
static RUNTIME: Lazy<Runtime> = Lazy::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
});

#[derive(Clone)]
pub struct ClientStorage {
    storage: Arc<dyn Storage>,
    #[allow(dead_code)]
    base_path: PathBuf, // Keep base_path for legacy access if needed
}

impl ClientStorage {
    pub fn init(base_path: PathBuf) -> ClientStorage {
        let storage = Self::create_storage_backend(base_path.clone());
        ClientStorage { storage, base_path }
    }

    fn create_storage_backend(base_path: PathBuf) -> Arc<dyn Storage> {
        // Check for object storage configuration
        if let Ok(storage_type) = std::env::var("LIBRA_STORAGE_TYPE") {
            let bucket =
                std::env::var("LIBRA_STORAGE_BUCKET").unwrap_or_else(|_| "libra".to_string());
            if bucket.is_empty() {
                eprintln!(
                    "Error: LIBRA_STORAGE_BUCKET cannot be empty. Falling back to local storage."
                );
                return Arc::new(LocalStorage::new(base_path));
            }

            // Build ObjectStore
            let object_store: Arc<dyn object_store::ObjectStore> = match storage_type.as_str() {
                "s3" | "r2" => {
                    let mut builder =
                        object_store::aws::AmazonS3Builder::new().with_bucket_name(&bucket);

                    if let Ok(endpoint) = std::env::var("LIBRA_STORAGE_ENDPOINT") {
                        if url::Url::parse(&endpoint).is_err() {
                            eprintln!(
                                "Error: Invalid LIBRA_STORAGE_ENDPOINT URL: {}. Falling back to local storage.",
                                endpoint
                            );
                            return Arc::new(LocalStorage::new(base_path));
                        }
                        builder = builder.with_endpoint(endpoint);
                    }
                    if let Ok(region) = std::env::var("LIBRA_STORAGE_REGION") {
                        builder = builder.with_region(region);
                    }
                    if let Ok(key) = std::env::var("LIBRA_STORAGE_ACCESS_KEY") {
                        if key.is_empty() {
                            eprintln!(
                                "Error: LIBRA_STORAGE_ACCESS_KEY cannot be empty. Falling back to local storage."
                            );
                            return Arc::new(LocalStorage::new(base_path));
                        }
                        builder = builder.with_access_key_id(key);
                    }
                    if let Ok(secret) = std::env::var("LIBRA_STORAGE_SECRET_KEY") {
                        if secret.is_empty() {
                            eprintln!(
                                "Error: LIBRA_STORAGE_SECRET_KEY cannot be empty. Falling back to local storage."
                            );
                            return Arc::new(LocalStorage::new(base_path));
                        }
                        builder = builder.with_secret_access_key(secret);
                    }

                    if std::env::var("LIBRA_STORAGE_ALLOW_HTTP").ok().as_deref() == Some("true") {
                        builder = builder.with_allow_http(true);
                    }

                    Arc::new(builder.build().expect("Failed to build S3 storage"))
                }
                _ => {
                    eprintln!(
                        "Error: Unsupported storage type: {}. Falling back to local storage.",
                        storage_type
                    );
                    return Arc::new(LocalStorage::new(base_path));
                }
            };

            let remote = RemoteStorage::new(object_store);
            let local = LocalStorage::new(base_path.clone());

            let threshold = std::env::var("LIBRA_STORAGE_THRESHOLD")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1024 * 1024); // 1MB default

            // Parse cache size (previously hardcoded/magic number)
            let disk_cache_limit_bytes = std::env::var("LIBRA_STORAGE_CACHE_SIZE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(200 * 1024 * 1024); // 200MB default

            Arc::new(TieredStorage::new(
                local,
                remote,
                threshold,
                disk_cache_limit_bytes,
            ))
        } else {
            // Default to local storage
            Arc::new(LocalStorage::new(base_path))
        }
    }

    /// Helper to execute async task on dedicated runtime and block waiting for result
    fn block_on_storage<F, T>(&self, future: F) -> T
    where
        F: std::future::Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        let (tx, rx) = mpsc::channel();
        RUNTIME.spawn(async move {
            let res = future.await;
            let _ = tx.send(res);
        });
        rx.recv().unwrap()
    }

    pub fn get(&self, object_id: &ObjectHash) -> Result<Vec<u8>, GitError> {
        let storage = self.storage.clone();
        let hash = *object_id;
        self.block_on_storage(async move { storage.get(&hash).await.map(|(data, _)| data) })
    }

    pub fn put(
        &self,
        obj_id: &ObjectHash,
        content: &[u8],
        obj_type: ObjectType,
    ) -> Result<String, io::Error> {
        let storage = self.storage.clone();
        let hash = *obj_id;
        let data = content.to_vec();

        self.block_on_storage(async move {
            storage
                .put(&hash, &data, obj_type)
                .await
                .map_err(|e| io::Error::other(e.to_string()))
        })
    }

    pub fn exist(&self, obj_id: &ObjectHash) -> bool {
        let storage = self.storage.clone();
        let hash = *obj_id;
        self.block_on_storage(async move { storage.exist(&hash).await })
    }

    pub fn get_object_type(&self, obj_id: &ObjectHash) -> Result<ObjectType, GitError> {
        let storage = self.storage.clone();
        let hash = *obj_id;
        self.block_on_storage(async move { storage.get(&hash).await.map(|(_, t)| t) })
    }

    pub fn is_object_type(&self, obj_id: &ObjectHash, obj_type: ObjectType) -> bool {
        match self.get_object_type(obj_id) {
            Ok(t) => t == obj_type,
            Err(_) => false,
        }
    }

    pub async fn search(&self, obj_id: &str) -> Vec<ObjectHash> {
        if obj_id == "HEAD" {
            return vec![Head::current_commit().await.unwrap()];
        }

        let _re = Regex::new(r"(\^|~)(\d*)").unwrap();
        if obj_id.contains('~') || obj_id.contains('^') {
            // Complex navigation - relies on sync methods (load_object)
            // This runs in current thread/runtime.
            // Calls to load_object will trigger self.get() which uses dedicated RUNTIME.
            // Safe.

            let mut split_pos = 0;
            let mut found_special = false;
            for (i, c) in obj_id.char_indices() {
                if c == '~' || c == '^' {
                    found_special = true;
                    split_pos = i;
                    break;
                }
            }

            if found_special {
                let base_ref = &obj_id[..split_pos];
                let path_part = &obj_id[split_pos..];

                let base_commit = match base_ref {
                    "HEAD" => Head::current_commit().await.unwrap(),
                    _ => {
                        if let Some(branch) = Branch::find_branch(base_ref, None).await {
                            branch.commit
                        } else {
                            // Search by prefix
                            let matches = self.storage.search(base_ref).await;
                            let commits: Vec<ObjectHash> = matches
                                .into_iter()
                                .filter(|x| self.is_object_type(x, ObjectType::Commit))
                                .collect();

                            if commits.len() == 1 {
                                commits[0]
                            } else {
                                return Vec::new();
                            }
                        }
                    }
                };
                let target_commit = match self.navigate_commit_path(base_commit, path_part) {
                    Ok(commit) => commit,
                    Err(_) => return Vec::new(),
                };

                return vec![target_commit];
            }
        }

        // Simple prefix search
        self.storage.search(obj_id).await
    }

    fn navigate_commit_path(
        &self,
        base_commit: ObjectHash,
        path: &str,
    ) -> Result<ObjectHash, GitError> {
        let mut current = base_commit;
        let re = Regex::new(r"(\^|~)(\d*)").unwrap();

        if !re.is_match(path) {
            return Err(GitError::InvalidArgument(format!(
                "Invalid reference path: {path}"
            )));
        }
        for cap in re.captures_iter(path) {
            let symbol = cap.get(1).unwrap().as_str();
            let num_str = cap.get(2).map_or("1", |m| m.as_str());
            let num: usize = num_str.parse().unwrap_or(1);

            match symbol {
                "^" => {
                    current = self.get_parent_commit(&current, num)?;
                }
                "~" => {
                    for _ in 0..num {
                        current = self.get_parent_commit(&current, 1)?;
                    }
                }
                _ => unreachable!(),
            }
        }
        Ok(current)
    }

    #[allow(dead_code)]
    async fn parse_head_reference(&self, reference: &str) -> Result<ObjectHash, GitError> {
        let mut current = Head::current_commit().await.unwrap();

        if reference == "HEAD" {
            return Ok(current);
        }

        let re = Regex::new(r"(\^|~)(\d*)").unwrap();
        let path = &reference[4..];
        if !re.is_match(path) {
            return Err(GitError::InvalidArgument(reference.to_string()));
        }

        for cap in re.captures_iter(path) {
            let symbol = cap.get(1).unwrap().as_str();
            let num_str = cap.get(2).map_or("1", |m| m.as_str());
            let num: usize = num_str.parse().unwrap_or(1);

            match symbol {
                "^" => {
                    current = self.get_parent_commit(&current, num)?;
                }
                "~" => {
                    for _ in 0..num {
                        current = self.get_parent_commit(&current, 1)?;
                    }
                }
                _ => unreachable!(),
            }
        }
        Ok(current)
    }

    fn get_parent_commit(&self, commit_id: &ObjectHash, n: usize) -> Result<ObjectHash, GitError> {
        let commit: Commit = load_object(commit_id)?;
        if n == 0 || n > commit.parent_commit_ids.len() {
            return Err(GitError::ObjectNotFound(format!(
                "Parent {n} does not exist"
            )));
        }
        Ok(commit.parent_commit_ids[n - 1])
    }

    // Helper functions exposed for tests/utils
    pub fn compress_zlib(data: &[u8]) -> io::Result<Vec<u8>> {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(data)?;
        let compressed_data = encoder.finish()?;
        Ok(compressed_data)
    }

    pub fn decompress_zlib(data: &[u8]) -> io::Result<Vec<u8>> {
        let mut decoder = ZlibDecoder::new(data);
        let mut decompressed_data = Vec::new();
        decoder.read_to_end(&mut decompressed_data)?;
        Ok(decompressed_data)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use git_internal::{
        errors::GitError,
        hash::{HashKind, get_hash_kind, set_hash_kind, set_hash_kind_for_test},
        internal::{
            metadata::{EntryMeta, MetaAttached},
            object::{ObjectTrait, blob::Blob},
            pack::{encode::PackEncoder, entry::Entry},
        },
    };
    use serial_test::serial;
    use tempfile::tempdir;
    use tokio::sync::mpsc;

    use super::ClientStorage;
    use crate::utils::test;

    // Helper to build packs (copied from previous version for tests)
    async fn encode_entries_to_pack_bytes(entries: Vec<Entry>) -> Result<Vec<u8>, GitError> {
        assert!(!entries.is_empty(), "encode requires at least one entry");
        let (pack_tx, mut pack_rx) = mpsc::channel::<Vec<u8>>(128);
        let (entry_tx, entry_rx) = mpsc::channel::<MetaAttached<Entry, EntryMeta>>(entries.len());
        let mut encoder = PackEncoder::new(entries.len(), 0, pack_tx);
        let kind = get_hash_kind();
        let encode_handle = tokio::spawn(async move {
            set_hash_kind(kind);
            encoder.encode(entry_rx).await
        });

        for entry in entries {
            entry_tx
                .send(MetaAttached {
                    inner: entry,
                    meta: EntryMeta::new(),
                })
                .await
                .map_err(|e| GitError::PackEncodeError(format!("send entry failed: {e}")))?;
        }
        drop(entry_tx);

        let mut pack_bytes = Vec::new();
        while let Some(chunk) = pack_rx.recv().await {
            pack_bytes.extend_from_slice(&chunk);
        }

        let encode_result = encode_handle
            .await
            .map_err(|e| GitError::PackEncodeError(format!("pack encoder task join error: {e}")))?;
        encode_result?;
        Ok(pack_bytes)
    }

    fn build_pack_bytes(entries: Vec<Entry>) -> Result<Vec<u8>, GitError> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        rt.block_on(encode_entries_to_pack_bytes(entries))
    }

    fn write_pack_to_objects(
        pack_bytes: &[u8],
        label: &str,
    ) -> Result<(tempfile::TempDir, PathBuf, PathBuf), GitError> {
        let dir = tempdir()?;
        let objects_dir = dir.path().join("objects");
        let pack_dir = objects_dir.join("pack");
        fs::create_dir_all(&pack_dir)?;
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let pack_path = pack_dir.join(format!("client-storage-{label}-{unique}.pack"));
        fs::write(&pack_path, pack_bytes)?;
        Ok((dir, objects_dir, pack_path))
    }

    #[test]
    #[serial]
    fn client_storage_reads_pack_sha1() -> Result<(), GitError> {
        let _guard = set_hash_kind_for_test(HashKind::Sha1);
        let blob = Blob::from_content("client-storage-sha1");
        let pack_bytes = build_pack_bytes(vec![Entry::from(blob.clone())])?;
        let (_tmp, objects_dir, _) = write_pack_to_objects(&pack_bytes, "sha1")?;

        let storage = ClientStorage::init(objects_dir);
        let data = storage.get(&blob.id)?;
        assert_eq!(data, blob.data);
        Ok(())
    }

    #[test]
    #[serial]
    fn client_storage_reads_pack_sha256() -> Result<(), GitError> {
        let _guard = set_hash_kind_for_test(HashKind::Sha256);
        let blob = Blob::from_content("client-storage-sha256");
        let pack_bytes = build_pack_bytes(vec![Entry::from(blob.clone())])?;
        let (_tmp, objects_dir, _) = write_pack_to_objects(&pack_bytes, "sha256")?;

        let storage = ClientStorage::init(objects_dir);
        let data = storage.get(&blob.id)?;
        assert_eq!(data, blob.data);
        Ok(())
    }

    #[test]
    #[ignore]
    fn test_content_store() {
        let content = "Hello, world!";
        let blob = Blob::from_content(content);

        let mut source = PathBuf::from(test::find_cargo_dir().parent().unwrap());
        source.push("tests/objects");

        let client_storage = ClientStorage::init(source.clone());
        assert!(
            client_storage
                .put(&blob.id, &blob.data, blob.get_type())
                .is_ok()
        );
        assert!(client_storage.exist(&blob.id));

        let data = client_storage.get(&blob.id).unwrap();
        assert_eq!(data, blob.data);
        assert_eq!(String::from_utf8(data).unwrap(), content);
    }

    #[tokio::test]
    async fn test_search() {
        let blob = Blob::from_content("Hello, world!");

        let mut source = PathBuf::from(test::find_cargo_dir().parent().unwrap());
        source.push("tests/objects");

        let client_storage = ClientStorage::init(source.clone());
        assert!(
            client_storage
                .put(&blob.id, &blob.data, blob.get_type())
                .is_ok()
        );

        // Search by full hash should return it
        let objs = client_storage.search(&blob.id.to_string()).await;
        assert!(!objs.is_empty());
    }

    #[test]
    fn test_decompress() {
        let data = b"blob 13\0Hello, world!";
        let compressed_data = ClientStorage::compress_zlib(data).unwrap();
        let decompressed_data = ClientStorage::decompress_zlib(&compressed_data).unwrap();
        assert_eq!(decompressed_data, data);
    }
}
