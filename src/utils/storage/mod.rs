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
