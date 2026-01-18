//! Tiered storage controller for Git objects. This module implements a tiered storage system that combines a local filesystem backend (LocalStorage) and a remote storage backend (RemoteStorage). The TieredStorage struct manages the logic for storing and retrieving Git objects based on their size, using an LRU cache to manage large objects stored locally as a cache layer.
use std::{
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use git_internal::{errors::GitError, hash::ObjectHash, internal::object::types::ObjectType};
use lru_mem::{HeapSize, LruCache};

use super::{Storage, local::LocalStorage, remote::RemoteStorage};

/// Wrapper for cached file to handle deletion on eviction
#[derive(Debug)]
struct CachedFile {
    path: PathBuf,
}

impl HeapSize for CachedFile {
    fn heap_size(&self) -> usize {
        // Return 0 so we just limit by count (metadata overhead is small)
        0
    }
}

impl Drop for CachedFile {
    fn drop(&mut self) {
        // Delete file when removed from LRU (or when TieredStorage is dropped)
        // Note: This might be dangerous if we are shutting down and want to keep cache?
        // But for "Cache", it's ephemeral.
        let _ = fs::remove_file(&self.path);
    }
}

/// Tiered storage controller
pub struct TieredStorage {
    local: LocalStorage,
    remote: RemoteStorage,
    threshold: usize,
    // LRU cache for tracking large files stored locally
    // Key: ObjectHash
    // Value: CachedFile (owns the cleanup responsibility)
    lru: Arc<Mutex<LruCache<ObjectHash, CachedFile>>>,
}

impl TieredStorage {
    pub fn new(
        local: LocalStorage,
        remote: RemoteStorage,
        threshold: usize,
        cache_size: usize,
    ) -> Self {
        Self {
            local,
            remote,
            threshold,
            lru: Arc::new(Mutex::new(LruCache::new(cache_size))),
        }
    }
}

#[async_trait]
impl Storage for TieredStorage {
    async fn get(&self, hash: &ObjectHash) -> Result<(Vec<u8>, ObjectType), GitError> {
        // 1. Check local (Permanent or Cached)
        if self.local.exist(hash).await {
            // If it's in LRU, access it to update recency
            {
                let mut lru = self.lru.lock().unwrap();
                let _ = lru.get(hash);
            }
            return self.local.get(hash).await;
        }

        // 2. Fetch from remote
        let (data, obj_type) = self.remote.get(hash).await?;

        // 3. Store locally based on size
        if data.len() < self.threshold {
            // Small: Permanent local store
            // We don't track it in LRU
            self.local.put(hash, &data, obj_type).await?;
        } else {
            // Large: Cache locally
            self.local.put(hash, &data, obj_type).await?;
            let path = self.local.get_obj_path(hash);

            let mut lru = self.lru.lock().unwrap();
            // insert returns the evicted value (if any). The CachedFile drop impl will delete the file.
            let _ = lru.insert(*hash, CachedFile { path });
        }

        Ok((data, obj_type))
    }

    async fn put(
        &self,
        hash: &ObjectHash,
        data: &[u8],
        obj_type: ObjectType,
    ) -> Result<String, GitError> {
        let size = data.len();

        // Always write to remote (Persistence)
        let remote_res = self.remote.put(hash, data, obj_type).await?;

        if size < self.threshold {
            // Small object: Write to local (Permanent)
            self.local.put(hash, data, obj_type).await?;
        } else {
            // Large object: Write to remote only (initially)
            // But if we want to cache it, we can write to local too.
            // Prompt says: "For > threshold: only store to remote, local uses LRU".
            // If we are writing, we probably have the data in memory anyway.
            // But if we don't write to local, subsequent reads will need to fetch.
            // Let's write to local as cache.
            self.local.put(hash, data, obj_type).await?;
            let path = self.local.get_obj_path(hash);

            let mut lru = self.lru.lock().unwrap();
            let _ = lru.insert(*hash, CachedFile { path });
        }

        Ok(remote_res)
    }

    async fn exist(&self, hash: &ObjectHash) -> bool {
        // Check local first (fast)
        if self.local.exist(hash).await {
            return true;
        }
        // Then remote
        self.remote.exist(hash).await
    }

    /// Search for objects by prefix. Only checks local storage since remote may not support listing.
    async fn search(&self, prefix: &str) -> Vec<ObjectHash> {
        self.local.search(prefix).await
    }
}
