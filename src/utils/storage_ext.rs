use async_trait::async_trait;
use git_internal::{
    errors::GitError,
    hash::ObjectHash,
    internal::object::{
        context::ContextSnapshot,
        decision::Decision,
        evidence::Evidence,
        integrity::IntegrityHash,
        patchset::PatchSet,
        plan::Plan,
        provenance::Provenance,
        run::Run,
        task::Task,
        tool::ToolInvocation,
        types::{ArtifactRef, ObjectType},
    },
};
use serde::{Serialize, de::DeserializeOwned};

use crate::{internal::ai::history::HistoryManager, utils::storage::Storage};

/// Trait for objects that have a unique ID and Type, used for Ref creation.
pub trait Identifiable {
    fn object_id(&self) -> String;
    fn object_type(&self) -> String;
}

impl Identifiable for Task {
    fn object_id(&self) -> String {
        self.header().object_id().to_string()
    }
    fn object_type(&self) -> String {
        self.header().object_type().to_string()
    }
}

impl Identifiable for Run {
    fn object_id(&self) -> String {
        self.header().object_id().to_string()
    }
    fn object_type(&self) -> String {
        self.header().object_type().to_string()
    }
}

impl Identifiable for Plan {
    fn object_id(&self) -> String {
        self.header().object_id().to_string()
    }
    fn object_type(&self) -> String {
        self.header().object_type().to_string()
    }
}

impl Identifiable for ContextSnapshot {
    fn object_id(&self) -> String {
        self.header().object_id().to_string()
    }
    fn object_type(&self) -> String {
        self.header().object_type().to_string()
    }
}

impl Identifiable for PatchSet {
    fn object_id(&self) -> String {
        self.header().object_id().to_string()
    }
    fn object_type(&self) -> String {
        self.header().object_type().to_string()
    }
}

impl Identifiable for Evidence {
    fn object_id(&self) -> String {
        self.header().object_id().to_string()
    }
    fn object_type(&self) -> String {
        self.header().object_type().to_string()
    }
}

impl Identifiable for ToolInvocation {
    fn object_id(&self) -> String {
        self.header().object_id().to_string()
    }
    fn object_type(&self) -> String {
        self.header().object_type().to_string()
    }
}

impl Identifiable for Provenance {
    fn object_id(&self) -> String {
        self.header().object_id().to_string()
    }
    fn object_type(&self) -> String {
        self.header().object_type().to_string()
    }
}

impl Identifiable for Decision {
    fn object_id(&self) -> String {
        self.header().object_id().to_string()
    }
    fn object_type(&self) -> String {
        self.header().object_type().to_string()
    }
}

/// Extension trait for Storage to support Structured Objects (JSON) and Artifacts
#[async_trait]
pub trait StorageExt: Storage + Send + Sync {
    /// Store a serializable object (Task, Run, etc.) as a Git Blob.
    /// Returns the Git Object Hash.
    async fn put_json<T: Serialize + Send + Sync>(
        &self,
        object: &T,
    ) -> Result<ObjectHash, GitError>;

    /// Store an object and automatically add it to the history log (Orphan Branch).
    /// This prevents GC and organizes objects in a time-series tree.
    /// Requires an explicit `HistoryManager` to decouple tracking from process CWD.
    async fn put_tracked<T: Serialize + Send + Sync + Identifiable>(
        &self,
        object: &T,
        history_manager: &HistoryManager,
    ) -> Result<ObjectHash, GitError>;

    /// Retrieve and deserialize an object from a Git Blob hash.
    async fn get_json<T: DeserializeOwned + Send + Sync>(
        &self,
        hash: &ObjectHash,
    ) -> Result<T, GitError>;

    /// Store raw content as an Artifact.
    /// Returns an ArtifactRef pointing to the stored content.
    async fn put_artifact(&self, data: &[u8]) -> Result<ArtifactRef, GitError>;
}

#[async_trait]
impl<S: Storage + Send + Sync + ?Sized> StorageExt for S {
    async fn put_json<T: Serialize + Send + Sync>(
        &self,
        object: &T,
    ) -> Result<ObjectHash, GitError> {
        // Serialize to JSON
        let data = serde_json::to_vec(object).map_err(|e| {
            GitError::InvalidObjectInfo(format!("Failed to serialize object: {}", e))
        })?;

        // Compute hash for Blob type
        let hash = ObjectHash::from_type_and_data(ObjectType::Blob, &data);

        // Store in backend
        self.put(&hash, &data, ObjectType::Blob).await?;

        Ok(hash)
    }

    async fn put_tracked<T: Serialize + Send + Sync + Identifiable>(
        &self,
        object: &T,
        history_manager: &HistoryManager,
    ) -> Result<ObjectHash, GitError> {
        let hash = self.put_json(object).await?;

        history_manager
            .append(&object.object_type(), &object.object_id(), hash)
            .await?;

        Ok(hash)
    }

    async fn get_json<T: DeserializeOwned + Send + Sync>(
        &self,
        hash: &ObjectHash,
    ) -> Result<T, GitError> {
        let (data, obj_type) = self.get(hash).await?;

        if obj_type != ObjectType::Blob {
            return Err(GitError::InvalidObjectType(format!(
                "Expected Blob for object, found {}",
                obj_type
            )));
        }

        let object = serde_json::from_slice(&data).map_err(|e| {
            GitError::InvalidObjectInfo(format!("Failed to deserialize object: {}", e))
        })?;

        Ok(object)
    }

    async fn put_artifact(&self, data: &[u8]) -> Result<ArtifactRef, GitError> {
        // Compute Git Object Hash (SHA1/SHA256) for storage addressing
        let object_hash = ObjectHash::from_type_and_data(ObjectType::Blob, data);

        // Store as Blob
        self.put(&object_hash, data, ObjectType::Blob).await?;

        // Compute Integrity Hash (SHA256) for ArtifactRef
        let integrity_hash = IntegrityHash::compute(data);

        // Create ArtifactRef
        // Key is the Git Object Hash string
        // Store is unified as "libra"
        let artifact = ArtifactRef::new("libra", object_hash.to_string())
            .map_err(GitError::InvalidObjectInfo)?
            .with_hash(integrity_hash);

        Ok(artifact)
    }
}

#[cfg(test)]
mod tests {
    use std::{str::FromStr, sync::Arc};

    use git_internal::internal::object::{
        task::{GoalType, Task},
        types::ActorRef,
    };
    use tempfile::tempdir;
    use uuid::Uuid;

    use super::*;
    use crate::utils::storage::local::LocalStorage;

    #[tokio::test]
    async fn test_storage_ext() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalStorage::new(dir.path().to_path_buf()));

        // 1. Test Task Storage
        let repo_id = Uuid::new_v4();
        let actor = ActorRef::human("tester").unwrap();
        let task = Task::new(repo_id, actor, "Test Task", Some(GoalType::Feature)).unwrap();

        let hash = storage.put_json(&task).await.unwrap();
        let loaded_task: Task = storage.get_json(&hash).await.unwrap();

        assert_eq!(task.header().object_id(), loaded_task.header().object_id());
        assert_eq!(task.title(), loaded_task.title());

        // 2. Test Artifact Storage
        let content = b"Hello Libra Artifact";
        let artifact = storage.put_artifact(content).await.unwrap();

        assert_eq!(artifact.store(), "libra");
        assert!(artifact.hash().is_some());

        // Verify retrieval using standard storage get (simulating Artifact resolution)
        let key_hash = ObjectHash::from_str(artifact.key()).unwrap();
        let (data, _) = storage.get(&key_hash).await.unwrap();
        assert_eq!(data, content);
    }
}
