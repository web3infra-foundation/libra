//! Storage trait and implementations for Git object storage.
pub mod local;
pub mod remote;
pub mod tiered;

use async_trait::async_trait;
use git_internal::{errors::GitError, hash::ObjectHash, internal::object::types::ObjectType};

/// Storage backend abstraction interface.
/// Defines basic object storage operations.
#[async_trait]
pub trait Storage: Send + Sync {
    /// Get object data by hash (decompressed, no header)
    /// Returns (Content, Type)
    async fn get(&self, hash: &ObjectHash) -> Result<(Vec<u8>, ObjectType), GitError>;

    /// Put object data
    /// Returns the path/location of the stored object
    async fn put(
        &self,
        hash: &ObjectHash,
        data: &[u8],
        obj_type: ObjectType,
    ) -> Result<String, GitError>;

    /// Check if object exists
    async fn exist(&self, hash: &ObjectHash) -> bool;

    /// List/Search objects by prefix
    async fn search(&self, prefix: &str) -> Vec<ObjectHash>;
}
