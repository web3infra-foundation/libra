//! Storage trait and implementations for Git object storage.
//!
//! `publish_storage` is the publish-specific arbitrary-object
//! wrapper (Phase 2 of `docs/development/commands/publish.md`); it does NOT
//! implement the Git-only `Storage` trait below so callers cannot
//! accidentally route publish JSON / bytes through Git zlib/header
//! packing.
pub mod local;
pub mod publish_storage;
pub mod remote;
pub mod tiered;

use async_trait::async_trait;
use git_internal::{errors::GitError, hash::ObjectHash, internal::object::types::ObjectType};

/// Abstract storage backend interface for Git objects
#[async_trait]
pub trait Storage: Send + Sync {
    /// Retrieve an object by its hash
    /// Returns the raw decompressed data and the object type.
    /// If the object is not found, returns `GitError::ObjectNotFound`.
    async fn get(&self, hash: &ObjectHash) -> Result<(Vec<u8>, ObjectType), GitError>;

    /// Store an object
    /// Takes the object hash, raw decompressed data, and object type.
    /// Returns the storage path or identifier.
    /// This operation should be idempotent.
    async fn put(
        &self,
        hash: &ObjectHash,
        data: &[u8],
        obj_type: ObjectType,
    ) -> Result<String, GitError>;

    /// Check if an object exists
    /// Returns true if the object exists in storage.
    async fn exist(&self, hash: &ObjectHash) -> bool;

    /// Search for objects by hash prefix
    /// Returns a list of object hashes that match the given prefix.
    /// Note: Performance may vary significantly between backends (fast locally, potentially slow remotely).
    async fn search(&self, prefix: &str) -> Vec<ObjectHash>;

    /// Batch existence check — returns one `bool` per input hash, in the same
    /// order (`lore.md` §0.6). Used as a dedup pre-check (e.g. "which of these
    /// objects does the remote already have before I upload?").
    ///
    /// The default runs `exist` sequentially: a correctness fallback with no
    /// speedup. The value is in backend overrides that probe in parallel —
    /// [`remote::RemoteStorage`] fires bounded-concurrency HEAD requests and
    /// [`tiered::TieredStorage`] answers local hits without any round trip and
    /// batches only the remote misses.
    async fn exist_batch(&self, hashes: &[ObjectHash]) -> Vec<bool> {
        let mut results = Vec::with_capacity(hashes.len());
        for hash in hashes {
            results.push(self.exist(hash).await);
        }
        results
    }

    /// Attempt to repair a missing or corrupted local object by re-fetching it
    /// from a durable tier, verifying that the fetched bytes hash to `hash`, and
    /// writing the object into the local store (`libra fsck --heal`, lore.md §0.4).
    ///
    /// # Returns
    /// * `Ok(true)` — the object was fetched, verified, and healed.
    /// * `Ok(false)` — this backend has no durable tier to heal from, or the
    ///   object is absent from that tier (unrecoverable). Backends MUST NOT
    ///   fabricate objects; only a payload that verifies against `hash` may be
    ///   written.
    ///
    /// The default implementation cannot heal (backends without a paired durable
    /// tier — local-only, remote-only, publish — return `Ok(false)`). Only
    /// [`tiered::TieredStorage`] overrides this.
    async fn heal(&self, _hash: &ObjectHash) -> Result<bool, GitError> {
        Ok(false)
    }
}
