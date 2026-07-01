//! Tiered storage controller for Git objects. This module implements a tiered storage system that combines a local filesystem backend (LocalStorage) and a remote storage backend (RemoteStorage). The TieredStorage struct manages the logic for storing and retrieving Git objects based on their size, using an LRU cache to manage large objects stored locally as a cache layer.
use std::{
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use git_internal::{
    errors::GitError,
    hash::{HashKind, ObjectHash},
    internal::object::types::ObjectType,
};
use lru_mem::{HeapSize, LruCache};

use super::{Storage, local::LocalStorage, remote::RemoteStorage};

/// Verify that a fetched object's bytes hash back to their claimed OID, before
/// the object is cached locally or returned.
///
/// lore.md §0.3 (取数即校验): a remote/durable-tier object must never be blindly
/// trusted — a corrupted or tampered payload must not poison the local cache or
/// reach the caller. The payload is reframed as a git object
/// `"<type> <len>\0<content>"` and hashed.
///
/// The hash algorithm is chosen from **`expected.kind()`**, NOT the ambient
/// thread-local `HashKind`. `ClientStorage::get` runs storage futures on a
/// spawned static-runtime worker thread (`client_storage.rs`) whose thread-local
/// `HashKind` is never set and defaults to SHA-1; hashing there via the
/// thread-local would recompute a SHA-1 OID and falsely reject a valid SHA-256
/// object. Deriving the algorithm from the requested OID is correct for both
/// SHA-1 and SHA-256 repositories regardless of which thread runs the check.
///
/// # Arguments
/// * `expected` - the OID the caller asked for.
/// * `obj_type` - the object type parsed from the fetched header.
/// * `data` - the fetched, header-stripped object content.
pub(crate) fn verify_fetched_object(
    expected: &ObjectHash,
    obj_type: ObjectType,
    data: &[u8],
) -> Result<(), GitError> {
    let type_bytes = obj_type.to_data().map_err(|e| {
        GitError::InvalidObjectInfo(format!("unknown object type for fetched object: {e}"))
    })?;
    // Reframe as a git object: "<type> <len>\0<content>".
    let mut framed = Vec::with_capacity(type_bytes.len() + data.len() + 24);
    framed.extend_from_slice(&type_bytes);
    framed.push(b' ');
    framed.extend_from_slice(data.len().to_string().as_bytes());
    framed.push(0);
    framed.extend_from_slice(data);

    let computed = match expected.kind() {
        HashKind::Sha1 => {
            use sha1::{Digest, Sha1};
            ObjectHash::Sha1(Sha1::digest(&framed).into())
        }
        HashKind::Sha256 => {
            use sha2::{Digest, Sha256};
            ObjectHash::Sha256(Sha256::digest(&framed).into())
        }
    };

    if &computed == expected {
        Ok(())
    } else {
        Err(GitError::InvalidObjectInfo(format!(
            "remote object {expected} failed integrity check: {obj_type} payload hashes to {computed}"
        )))
    }
}

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

        // 2b. Verify-on-cache: reject a payload that does not hash to the
        // requested OID before it can poison the local cache (lore.md §0.3).
        verify_fetched_object(hash, obj_type, &data)?;

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

    /// Re-fetch a missing or corrupted object from the durable (remote) tier,
    /// verify it, and (over)write it into the local store. lore.md §0.4.
    ///
    /// This deliberately bypasses the local-first short-circuit in [`Self::get`]:
    /// a corrupted local object must be replaced with a fresh, verified copy, so
    /// the fetch always goes to the durable tier. The local write only happens
    /// after verification succeeds, so a failed or absent remote never destroys
    /// the existing (even if corrupt) local object — and a bad payload is never
    /// persisted (no fabrication). `remote.get` inherits object_store's bounded
    /// 429/`SlowDown`/5xx backoff (lore.md §0.2); `verify_fetched_object` is the
    /// same integrity check as verify-on-cache (lore.md §0.3).
    async fn heal(&self, hash: &ObjectHash) -> Result<bool, GitError> {
        let (data, obj_type) = match self.remote.get(hash).await {
            Ok(pair) => pair,
            // Not present in the durable tier: unrecoverable, but not an error —
            // the caller reports it rather than fabricating anything.
            Err(GitError::ObjectNotFound(_)) => return Ok(false),
            Err(err) => return Err(err),
        };
        verify_fetched_object(hash, obj_type, &data)?;
        // LocalStorage::put truncates, so this repairs a corrupt object in place
        // and creates a missing one.
        self.local.put(hash, &data, obj_type).await?;
        // Track a large healed object in the LRU exactly like `get` does, so it
        // stays subject to `LIBRA_STORAGE_CACHE_SIZE` eviction — otherwise
        // healing many large objects would grow the local cache unboundedly.
        if data.len() >= self.threshold {
            let path = self.local.get_obj_path(hash);
            let mut lru = self.lru.lock().expect("TieredStorage LRU mutex poisoned");
            let _ = lru.insert(
                *hash,
                CachedFile {
                    path,
                    disk_size: data.len(),
                },
            );
        }
        Ok(true)
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

    /// Verify-on-cache accepts a payload that hashes to the requested OID and
    /// rejects any mismatch, under SHA-1. Covers the lore.md §0.3 requirement
    /// that both hash formats be exercised.
    #[test]
    fn verify_fetched_object_matches_and_mismatches_sha1() {
        use git_internal::hash::{HashKind, set_hash_kind_for_test};

        let _guard = set_hash_kind_for_test(HashKind::Sha1);
        let data = b"hello libra";
        let expected = ObjectHash::from_type_and_data(ObjectType::Blob, data);

        assert!(verify_fetched_object(&expected, ObjectType::Blob, data).is_ok());
        // Tampered content no longer hashes to the requested OID.
        assert!(verify_fetched_object(&expected, ObjectType::Blob, b"HELLO libra").is_err());
        // Wrong object type changes the `<type> <len>\0` framing → mismatch.
        assert!(verify_fetched_object(&expected, ObjectType::Commit, data).is_err());
    }

    /// Same contract under SHA-256, so an object-format-256 repository is also
    /// protected against a poisoned cache write.
    #[test]
    fn verify_fetched_object_matches_and_mismatches_sha256() {
        use git_internal::hash::{HashKind, set_hash_kind_for_test};

        let _guard = set_hash_kind_for_test(HashKind::Sha256);
        let data = b"hello libra";
        let expected = ObjectHash::from_type_and_data(ObjectType::Blob, data);

        assert!(verify_fetched_object(&expected, ObjectType::Blob, data).is_ok());
        assert!(verify_fetched_object(&expected, ObjectType::Blob, b"tampered").is_err());
    }

    /// Regression: `ClientStorage::get` runs the tiered fetch on a spawned
    /// static-runtime worker thread whose thread-local `HashKind` defaults to
    /// SHA-1. Verification MUST derive the algorithm from the requested OID, not
    /// the ambient thread-local — otherwise a valid SHA-256 object would be
    /// hashed as SHA-1 and falsely rejected. This test forces exactly that
    /// mismatch (ambient SHA-1, SHA-256 OID) and asserts the object still passes.
    #[test]
    fn verify_uses_requested_oid_kind_not_ambient_hash_kind() {
        use git_internal::hash::{HashKind, set_hash_kind_for_test};

        let data = b"hello libra";
        // Compute the SHA-256 OID under a scoped SHA-256 ambient kind.
        let expected_sha256 = {
            let _sha256 = set_hash_kind_for_test(HashKind::Sha256);
            ObjectHash::from_type_and_data(ObjectType::Blob, data)
        };
        assert!(matches!(expected_sha256, ObjectHash::Sha256(_)));

        // Now pin the ambient kind to SHA-1 (the spawned-worker default) and
        // confirm the SHA-256 object still verifies, and a tamper still fails.
        let _ambient_sha1 = set_hash_kind_for_test(HashKind::Sha1);
        assert!(verify_fetched_object(&expected_sha256, ObjectType::Blob, data).is_ok());
        assert!(verify_fetched_object(&expected_sha256, ObjectType::Blob, b"tampered").is_err());
    }

    /// End-to-end through `TieredStorage::get`: an object present only in the
    /// remote tier is fetched, verified, cached, and returned unchanged.
    #[tokio::test]
    async fn tiered_get_verifies_and_caches_valid_remote_object() {
        use std::sync::Arc;

        use git_internal::hash::{HashKind, set_hash_kind_for_test};

        let _kind = set_hash_kind_for_test(HashKind::Sha1);
        let obj_type = ObjectType::Blob;
        let data = b"tiered verify happy path".to_vec();
        let hash = ObjectHash::from_type_and_data(obj_type, &data);

        let remote = RemoteStorage::new(Arc::new(object_store::memory::InMemory::new()));
        remote
            .put(&hash, &data, obj_type)
            .await
            .expect("seed remote");

        let local_dir = tempdir().expect("tempdir");
        let tiered = TieredStorage::new(
            LocalStorage::new(local_dir.path().to_path_buf()),
            remote,
            1 << 20,
            1 << 20,
        );

        // Empty local cache → fetch from remote, verify, cache, return.
        let (got, got_type) = tiered.get(&hash).await.expect("get should succeed");
        assert_eq!(got, data);
        assert_eq!(got_type, obj_type);
        assert!(tiered.local.exist(&hash).await, "object should be cached");
    }

    /// End-to-end: a remote object whose bytes do not hash to the requested OID
    /// (corruption/tampering in the durable tier) is rejected by `get`, and is
    /// NOT written into the local cache.
    #[tokio::test]
    async fn tiered_get_rejects_and_does_not_cache_corrupted_remote_object() {
        use std::sync::Arc;

        use git_internal::hash::{HashKind, set_hash_kind_for_test};

        let _kind = set_hash_kind_for_test(HashKind::Sha1);
        let obj_type = ObjectType::Blob;
        let good = b"the original bytes".to_vec();
        let hash = ObjectHash::from_type_and_data(obj_type, &good);

        // Store DIFFERENT bytes at the requested OID's location.
        let remote = RemoteStorage::new(Arc::new(object_store::memory::InMemory::new()));
        remote
            .put(&hash, b"tampered payload", obj_type)
            .await
            .expect("seed remote");

        let local_dir = tempdir().expect("tempdir");
        let tiered = TieredStorage::new(
            LocalStorage::new(local_dir.path().to_path_buf()),
            remote,
            1 << 20,
            1 << 20,
        );

        assert!(
            tiered.get(&hash).await.is_err(),
            "corrupted object must be rejected"
        );
        assert!(
            !tiered.local.exist(&hash).await,
            "corrupted object must not be cached"
        );
    }

    /// `heal` fetches a missing object from the durable (remote) tier, verifies
    /// it, and writes it into the local store.
    #[tokio::test]
    async fn heal_recreates_missing_object_from_remote() {
        use std::sync::Arc;

        use git_internal::hash::{HashKind, set_hash_kind_for_test};

        let _kind = set_hash_kind_for_test(HashKind::Sha1);
        let obj_type = ObjectType::Blob;
        let data = b"heal me".to_vec();
        let hash = ObjectHash::from_type_and_data(obj_type, &data);

        let remote = RemoteStorage::new(Arc::new(object_store::memory::InMemory::new()));
        remote
            .put(&hash, &data, obj_type)
            .await
            .expect("seed remote");

        let local_dir = tempdir().expect("tempdir");
        let tiered = TieredStorage::new(
            LocalStorage::new(local_dir.path().to_path_buf()),
            remote,
            1 << 20,
            1 << 20,
        );

        assert!(
            !tiered.local.exist(&hash).await,
            "precondition: absent local"
        );
        assert!(tiered.heal(&hash).await.expect("heal"), "should heal");
        assert!(tiered.local.exist(&hash).await, "healed into local store");
        let (got, _) = tiered.local.get(&hash).await.expect("local get");
        assert_eq!(got, data);
    }

    /// `heal` replaces a corrupt local object with a fresh verified copy from the
    /// durable tier (overwrite, not skip).
    #[tokio::test]
    async fn heal_overwrites_corrupt_local_object() {
        use std::sync::Arc;

        use git_internal::hash::{HashKind, set_hash_kind_for_test};

        let _kind = set_hash_kind_for_test(HashKind::Sha1);
        let obj_type = ObjectType::Blob;
        let good = b"the good bytes".to_vec();
        let hash = ObjectHash::from_type_and_data(obj_type, &good);

        let remote = RemoteStorage::new(Arc::new(object_store::memory::InMemory::new()));
        remote
            .put(&hash, &good, obj_type)
            .await
            .expect("seed remote");

        let local_dir = tempdir().expect("tempdir");
        let local = LocalStorage::new(local_dir.path().to_path_buf());
        // Corrupt the local copy: wrong bytes stored under the correct OID path.
        local
            .put(&hash, b"corrupt bytes", obj_type)
            .await
            .expect("seed corrupt local");
        let tiered = TieredStorage::new(local, remote, 1 << 20, 1 << 20);

        assert!(tiered.heal(&hash).await.expect("heal"), "should heal");
        let (got, _) = tiered.local.get(&hash).await.expect("local get");
        assert_eq!(got, good, "corrupt local object replaced with good bytes");
    }

    /// `heal` returns `Ok(false)` (unrecoverable, no fabrication) when the object
    /// is absent from the durable tier.
    #[tokio::test]
    async fn heal_returns_false_when_absent_from_remote() {
        use std::sync::Arc;

        use git_internal::hash::{HashKind, set_hash_kind_for_test};

        let _kind = set_hash_kind_for_test(HashKind::Sha1);
        let obj_type = ObjectType::Blob;
        let data = b"never uploaded".to_vec();
        let hash = ObjectHash::from_type_and_data(obj_type, &data);

        let remote = RemoteStorage::new(Arc::new(object_store::memory::InMemory::new()));
        let local_dir = tempdir().expect("tempdir");
        let tiered = TieredStorage::new(
            LocalStorage::new(local_dir.path().to_path_buf()),
            remote,
            1 << 20,
            1 << 20,
        );

        assert!(!tiered.heal(&hash).await.expect("heal"), "unrecoverable");
        assert!(!tiered.local.exist(&hash).await, "nothing fabricated");
    }

    /// A local-only backend has no durable tier and uses the default `heal`,
    /// which cannot repair anything.
    #[tokio::test]
    async fn local_storage_cannot_heal() {
        use git_internal::hash::{HashKind, set_hash_kind_for_test};

        let _kind = set_hash_kind_for_test(HashKind::Sha1);
        let hash = ObjectHash::from_type_and_data(ObjectType::Blob, b"x");
        let local_dir = tempdir().expect("tempdir");
        let local = LocalStorage::new(local_dir.path().to_path_buf());
        assert!(!local.heal(&hash).await.expect("heal"));
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
