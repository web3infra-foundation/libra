use std::{
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};

use flate2::{Compression, read::ZlibDecoder, write::ZlibEncoder};
use futures::FutureExt; // Import for catch_unwind
use git_internal::{
    errors::GitError,
    hash::ObjectHash,
    internal::object::{commit::Commit, types::ObjectType},
};
use once_cell::sync::Lazy;
use regex::Regex;
use sea_orm::{ActiveModelTrait, Set};
use tokio::{
    runtime::Runtime,
    sync::mpsc::{Sender, channel, error::TrySendError},
};
use uuid::Uuid;

use crate::{
    command::load_object,
    internal::{
        branch::Branch,
        config::Config,
        db,
        head::Head,
        model::{config as config_model, object_index},
    },
    utils::storage::{Storage, local::LocalStorage, remote::RemoteStorage, tiered::TieredStorage},
};

// Dedicated runtime for storage operations to avoid blocking/deadlocks in the main runtime
static RUNTIME: Lazy<Runtime> = Lazy::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
});

// Index update message
struct IndexUpdateMsg {
    hash: String,
    obj_type: String,
    size: i64,
    db_path: PathBuf,
}

// Helper guard to ensure PENDING_TASKS is decremented even if task panics
struct TaskGuard;
impl Drop for TaskGuard {
    fn drop(&mut self) {
        PENDING_TASKS.fetch_sub(1, Ordering::Relaxed);
    }
}

// Global channel for index updates
// Using Bounded channel to apply backpressure
// The consumer will process updates sequentially to avoid DB lock contention.
static INDEX_UPDATE_CHANNEL: Lazy<Sender<IndexUpdateMsg>> = Lazy::new(|| {
    let (tx, mut rx) = channel::<IndexUpdateMsg>(1000);

    RUNTIME.spawn(async move {
        while let Some(msg) = rx.recv().await {
            // Guard ensures decrement happens on drop (scope exit or panic)
            let _guard = TaskGuard;

            // Wrap in AssertUnwindSafe to catch panics from DB operations
            // This prevents the consumer loop from dying if one update fails hard
            let future = async {
                if let Err(e) =
                    update_object_index(&msg.db_path, &msg.hash, &msg.obj_type, msg.size).await
                {
                    tracing::warn!("Failed to update object index for {}: {}", msg.hash, e);
                }
            };
            let result = std::panic::AssertUnwindSafe(future).catch_unwind().await;

            if let Err(payload) = result {
                tracing::error!("Panic in background index update task: {:?}", payload);
            }
        }
    });

    tx
});

// Counter for active background tasks
static PENDING_TASKS: AtomicUsize = AtomicUsize::new(0);

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

    /// Create a storage backend.
    ///
    /// # Remote Storage
    /// If `LIBRA_STORAGE_TYPE` is set to "s3" or "r2", it configures a tiered storage
    /// with local cache and remote persistence.
    ///
    /// ## Repo ID Isolation
    /// When remote storage is enabled, it attempts to read `libra.repoid` from the configuration.
    /// If found, it uses `repo_id` as a key prefix (`<repo_id>/objects/...`) for isolation.
    /// If not found (e.g., during init before config exists), it defaults to no prefix (root of bucket),
    /// which might be risky for multi-tenant buckets but acceptable for single-repo buckets.
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

            let remote = match get_or_create_repo_id_for_prefix() {
                Some(repo_id) => RemoteStorage::new_with_prefix(object_store, repo_id),
                None => RemoteStorage::new(object_store),
            };
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

    /// Wait for all background tasks (e.g. indexing) to complete
    pub fn wait_for_background_tasks() {
        // Wait until all tasks finish
        let mut waited = 0;
        loop {
            let pending = PENDING_TASKS.load(Ordering::Relaxed);
            if pending == 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
            waited += 100;
            if waited >= 5000 {
                tracing::info!("Waiting for {} background tasks to complete...", pending);
                waited = 0;
            }
        }
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
        let data_len = data.len();
        let hash_str = hash.to_string();
        let type_str = obj_type.to_string();

        // First, store the object
        let result = self.block_on_storage(async move {
            storage
                .put(&hash, &data, obj_type)
                .await
                .map_err(|e| io::Error::other(e.to_string()))
        })?;

        // Update object index asynchronously (via sequential queue)
        // This ensures CLI commands don't block on indexing, and avoids DB lock contention.
        if let Some(db_path) = Self::index_db_path_from_base(&self.base_path)
            && db_path.exists()
        {
            let hash_str = hash_str.clone();
            let type_str = type_str.clone();

            PENDING_TASKS.fetch_add(1, Ordering::Relaxed);

            // Send to global channel
            // If channel is closed (runtime shutting down), we can't do much, but that's unlikely in normal CLI flow.
            let msg = IndexUpdateMsg {
                hash: hash_str,
                obj_type: type_str,
                size: data_len as i64,
                db_path,
            };

            match INDEX_UPDATE_CHANNEL.try_send(msg) {
                Ok(_) => {}
                Err(TrySendError::Full(msg)) => {
                    // Avoid blocking the caller thread if the bounded queue is
                    // full; wait for capacity on the dedicated storage runtime.
                    RUNTIME.spawn(async move {
                        if INDEX_UPDATE_CHANNEL.send(msg).await.is_err() {
                            PENDING_TASKS.fetch_sub(1, Ordering::Relaxed);
                            tracing::warn!("Failed to queue object index update: channel closed");
                        }
                    });
                }
                Err(TrySendError::Closed(_)) => {
                    PENDING_TASKS.fetch_sub(1, Ordering::Relaxed);
                    tracing::warn!("Failed to queue object index update: channel closed");
                }
            }
        }

        Ok(result)
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

    fn index_db_path_from_base(base_path: &Path) -> Option<PathBuf> {
        base_path
            .parent()
            .map(|storage_path| storage_path.join(crate::utils::util::DATABASE))
    }
}

fn get_or_create_repo_id_for_prefix() -> Option<String> {
    let storage_path = crate::utils::util::try_get_storage_path(None).ok()?;
    let db_path = storage_path.join(crate::utils::util::DATABASE);
    if !db_path.exists() {
        return None;
    }

    let (tx, rx) = mpsc::channel();
    RUNTIME.spawn(async move {
        let mut repo_id = Config::get("libra", None, "repoid").await;
        let needs_init = repo_id
            .as_deref()
            .map(|s| s.is_empty() || s == "unknown-repo")
            .unwrap_or(true);
        if needs_init {
            let new_id = Uuid::new_v4().to_string();
            Config::insert("libra", None, "repoid", &new_id).await;
            repo_id = Some(new_id);
        }
        let _ = tx.send(repo_id);
    });

    rx.recv().ok().flatten()
}

async fn load_repo_id_with_conn<C: sea_orm::ConnectionTrait>(
    db: &C,
) -> Result<String, sea_orm::DbErr> {
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

    let repo_id = config_model::Entity::find()
        .filter(config_model::Column::Configuration.eq("libra"))
        .filter(config_model::Column::Name.is_null())
        .filter(config_model::Column::Key.eq("repoid"))
        .one(db)
        .await?
        .map(|entry| entry.value)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown-repo".to_string());

    Ok(repo_id)
}

async fn update_object_index(
    db_path: &Path,
    o_id: &str,
    o_type: &str,
    o_size: i64,
) -> Result<(), String> {
    match update_object_index_once(db_path, o_id, o_type, o_size).await {
        Ok(()) => Ok(()),
        Err(_err) if !db_path.exists() => Ok(()),
        Err(first_err) => {
            tracing::debug!(
                db_path = %db_path.display(),
                object_id = o_id,
                error = %first_err,
                "Retrying object index update after resetting cached database connection"
            );
            db::reset_db_conn_instance_for_path(db_path).await;
            match update_object_index_once(db_path, o_id, o_type, o_size).await {
                Ok(()) => Ok(()),
                Err(_err) if !db_path.exists() => Ok(()),
                Err(err) => Err(err),
            }
        }
    }
}

/// Update object_index table for cloud backup tracking.
async fn update_object_index_once(
    db_path: &Path,
    o_id: &str,
    o_type: &str,
    o_size: i64,
) -> Result<(), String> {
    if !db_path.exists() {
        return Ok(());
    }

    let db_conn = match db::get_db_conn_instance_for_path(db_path).await {
        Ok(conn) => conn,
        Err(err) if err.kind() == io::ErrorKind::NotFound || !db_path.exists() => return Ok(()),
        Err(err) => {
            return Err(format!(
                "Failed to connect to object index database {}: {}",
                db_path.display(),
                err
            ));
        }
    };

    let repo_id = match load_repo_id_with_conn(&db_conn).await {
        Ok(repo_id) => repo_id,
        Err(_err) if !db_path.exists() => return Ok(()),
        Err(err) => {
            return Err(format!(
                "Failed to load repo id from {}: {}",
                db_path.display(),
                err
            ));
        }
    };

    let created_at = chrono::Utc::now().timestamp();

    // Check if object already exists
    // With multi-repo support, we must check (o_id, repo_id)
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    let existing = object_index::Entity::find()
        .filter(object_index::Column::OId.eq(o_id))
        .filter(object_index::Column::RepoId.eq(&repo_id))
        .one(&db_conn)
        .await;

    let existing = match existing {
        Ok(existing) => existing,
        Err(err) => {
            if !db_path.exists() {
                return Ok(());
            }
            return Err(format!("Database query failed: {}", err));
        }
    };

    if existing.is_some() {
        return Ok(());
    }

    // Insert new object index entry
    let entry = object_index::ActiveModel {
        o_id: Set(o_id.to_string()),
        o_type: Set(o_type.to_string()),
        o_size: Set(o_size),
        repo_id: Set(repo_id),
        created_at: Set(created_at),
        is_synced: Set(0), // Not synced to cloud yet
        ..Default::default()
    };

    if let Err(err) = entry.insert(&db_conn).await {
        if !db_path.exists() {
            return Ok(());
        }
        return Err(format!("Failed to insert object index: {}", err));
    }

    Ok(())
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
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    use serial_test::serial;
    use tempfile::tempdir;
    use tokio::sync::mpsc;

    use super::{ClientStorage, update_object_index};
    use crate::{
        internal::{config::Config, db, model::object_index},
        utils::test::ChangeDirGuard,
    };

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
    #[serial]
    fn test_content_store() {
        let content = "Hello, world!";
        let blob = Blob::from_content(content);

        let _tmp = tempdir().unwrap();
        let source = _tmp.path().join("objects");

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

        let _tmp = tempdir().unwrap();
        let source = _tmp.path().join("objects");

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

    #[tokio::test]
    #[serial]
    async fn background_index_update_uses_storage_database_instead_of_cwd() {
        let storage_root = tempdir().unwrap();
        let unrelated_dir = tempdir().unwrap();
        let storage_path = storage_root.path();
        let objects_dir = storage_path.join("objects");
        fs::create_dir_all(&objects_dir).unwrap();

        let db_path = storage_path.join(crate::utils::util::DATABASE);
        let db_conn = db::create_database(db_path.to_str().unwrap())
            .await
            .unwrap();
        Config::insert_with_conn(&db_conn, "libra", None, "repoid", "repo-from-storage").await;

        let _guard = ChangeDirGuard::new(unrelated_dir.path());

        let blob = Blob::from_content("index from explicit storage db");
        let storage = ClientStorage::init(objects_dir);
        storage.put(&blob.id, &blob.data, blob.get_type()).unwrap();
        ClientStorage::wait_for_background_tasks();

        let row = object_index::Entity::find()
            .filter(object_index::Column::OId.eq(blob.id.to_string()))
            .filter(object_index::Column::RepoId.eq("repo-from-storage"))
            .one(&db_conn)
            .await
            .unwrap();
        assert!(row.is_some());
    }

    #[tokio::test]
    #[serial]
    async fn update_object_index_skips_missing_database_without_error() {
        let missing_root = tempdir().unwrap();
        let missing_db = missing_root.path().join(crate::utils::util::DATABASE);

        let result = update_object_index(&missing_db, "deadbeef", "blob", 12).await;
        assert!(result.is_ok());
    }
}
