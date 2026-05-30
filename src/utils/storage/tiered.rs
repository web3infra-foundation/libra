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
    /// LRU accounting size in bytes — the **uncompressed** object
    /// length (`data.len()` at insert time), used as the resource cost
    /// for the `LruCache` budget.
    ///
    /// NOTE: this is *not* the literal on-disk byte count.
    /// [`LocalStorage::put`] writes zlib-compressed loose objects, so
    /// the actual file is typically smaller than `disk_size`. Using the
    /// uncompressed length makes the LRU budget a **conservative
    /// (over-estimating) upper bound** on real disk use — the cache
    /// evicts at or before the configured limit, never after, so the
    /// disk footprint stays bounded. Switching to the true compressed
    /// size would require stat-ing the file after each write.
    disk_size: usize,
}

impl HeapSize for CachedFile {
    fn heap_size(&self) -> usize {
        // The LRU cache bounds cached-object resource cost; we report
        // `disk_size` (the uncompressed object length — see the field
        // doc) as that cost, not the struct's in-memory size.
        self.disk_size
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
    // Note: This tracks disk usage of cached files, not memory usage of the struct itself.
    lru: Arc<Mutex<LruCache<ObjectHash, CachedFile>>>,
}

impl TieredStorage {
    pub fn new(
        local: LocalStorage,
        remote: RemoteStorage,
        threshold: usize,
        disk_usage_limit: usize,
    ) -> Self {
        Self {
            local,
            remote,
            threshold,
            lru: Arc::new(Mutex::new(LruCache::new(disk_usage_limit))),
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
                let mut lru = self.lru.lock().expect("TieredStorage LRU mutex poisoned");
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

            let mut lru = self.lru.lock().expect("TieredStorage LRU mutex poisoned");
            // insert returns the evicted value (if any). The CachedFile drop impl will delete the file.
            let _ = lru.insert(
                *hash,
                CachedFile {
                    path,
                    disk_size: data.len(),
                },
            );
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

            let mut lru = self.lru.lock().expect("TieredStorage LRU mutex poisoned");
            let _ = lru.insert(
                *hash,
                CachedFile {
                    path,
                    disk_size: size,
                },
            );
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

    async fn search(&self, prefix: &str) -> Vec<ObjectHash> {
        let (local_res, remote_res) =
            futures::future::join(self.local.search(prefix), self.remote.search(prefix)).await;

        let mut results = std::collections::HashSet::new();
        results.extend(local_res);
        results.extend(remote_res);

        results.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::tempdir;

    use super::*;

    /// Create a real file of `size` bytes and wrap it in a `CachedFile`
    /// whose `disk_size` matches. Returns the file path so the test can
    /// assert presence/deletion.
    fn cached_file(dir: &std::path::Path, name: &str, size: usize) -> (PathBuf, CachedFile) {
        let path = dir.join(name);
        let mut f = fs::File::create(&path).expect("create cache file");
        f.write_all(&vec![0u8; size]).expect("write cache file");
        (
            path.clone(),
            CachedFile {
                path,
                disk_size: size,
            },
        )
    }

    /// `HeapSize::heap_size` MUST report the `disk_size` accounting
    /// field (the uncompressed object length — see the field doc) so
    /// the `LruCache`'s budget bounds cached-object resource cost. If
    /// this returned the struct's in-memory size instead, the cache
    /// would never evict on the intended threshold and the local cache
    /// dir would grow unbounded. (`disk_size` over-estimates the true
    /// compressed on-disk size, making the bound conservative.)
    #[test]
    fn cached_file_heap_size_reports_disk_size() {
        let dir = tempdir().expect("tempdir");
        let (_path, cf) = cached_file(dir.path(), "obj", 4096);
        assert_eq!(cf.heap_size(), 4096);
    }

    /// Dropping a `CachedFile` MUST delete its backing file — this is
    /// how an LRU eviction reclaims disk. Without it, evicted cache
    /// entries leak on disk forever.
    #[test]
    fn dropping_cached_file_deletes_backing_file() {
        let dir = tempdir().expect("tempdir");
        let (path, cf) = cached_file(dir.path(), "obj", 16);
        assert!(path.exists());
        drop(cf);
        assert!(
            !path.exists(),
            "CachedFile drop must delete its backing file",
        );
    }

    /// The combined resource-bounding contract: inserting past the
    /// `LruCache` disk budget evicts the least-recently-used entry AND
    /// its `Drop` deletes that entry's file, while the retained entry's
    /// file survives. This is what keeps the local large-object cache
    /// bounded on disk.
    #[test]
    fn lru_eviction_deletes_evicted_cache_file() {
        let dir = tempdir().expect("tempdir");
        // `LruCache` charges key + value + struct overhead per entry
        // (not just `heap_size`), so an entry for a 1000-byte file is
        // ~1096 bytes. A 1500-byte budget therefore holds exactly one
        // such entry but not two — the headroom keeps the test robust
        // against the exact per-entry overhead.
        let mut lru: LruCache<ObjectHash, CachedFile> = LruCache::new(1500);

        let key_a = ObjectHash::new(&[1; 20]);
        let key_b = ObjectHash::new(&[2; 20]);
        let (path_a, cf_a) = cached_file(dir.path(), "a", 1000);
        let (path_b, cf_b) = cached_file(dir.path(), "b", 1000);

        lru.insert(key_a, cf_a).expect("insert a within budget");
        assert!(path_a.exists());

        // Inserting B exceeds the budget (two ~1096-byte entries), so
        // the LRU evicts A; A's CachedFile drop deletes A's file.
        lru.insert(key_b, cf_b).expect("insert b evicts a");

        assert!(
            !path_a.exists(),
            "evicted entry A's backing file must be deleted on eviction",
        );
        assert!(path_b.exists(), "retained entry B's file must survive");
        assert!(lru.get(&key_b).is_some(), "B must remain cached");
        assert!(lru.get(&key_a).is_none(), "A must have been evicted");
    }
}
