use std::{path::PathBuf, str::FromStr, sync::Arc};

use git_internal::{
    errors::GitError,
    hash::ObjectHash,
    internal::object::{
        ObjectTrait,
        signature::{Signature, SignatureType},
        tree::{Tree, TreeItem, TreeItemMode},
    },
};

use crate::utils::{
    object::{read_git_object, write_git_object},
    storage::Storage,
};

const HISTORY_REF: &str = "refs/libra/history";

/// Manages object history using an orphan branch and Git Tree structure.
///
/// This manager can operate on any given Git reference (e.g., `refs/libra/history` for code history,
/// `refs/libra/intent` for intent history), allowing parallel history timelines.
///
/// Structure (Commit -> Tree):
///   ├── task/
///   │   └── <task_id>
///   ├── run/
///   │   └── <run_id>
///   ├── plan/
///   │   └── <plan_id>
///   └── artifact/
///       └── <artifact_hash>
pub struct HistoryManager {
    #[allow(dead_code)]
    storage: Arc<dyn Storage + Send + Sync>,
    repo_path: PathBuf,
    /// The Git reference name this manager writes to (e.g. "refs/libra/history").
    ref_name: String,
}

impl HistoryManager {
    pub fn new(storage: Arc<dyn Storage + Send + Sync>, repo_path: PathBuf) -> Self {
        Self::new_with_ref(storage, repo_path, HISTORY_REF)
    }

    pub fn new_with_ref(
        storage: Arc<dyn Storage + Send + Sync>,
        repo_path: PathBuf,
        ref_name: impl Into<String>,
    ) -> Self {
        Self {
            storage,
            repo_path,
            ref_name: ref_name.into(),
        }
    }

    /// Append an object to the history log.
    /// This operation is synchronous (commits immediately) for the MVP.
    pub async fn append(
        &self,
        object_type: &str,
        object_id: &str,
        blob_hash: ObjectHash,
    ) -> Result<(), GitError> {
        // 1. Resolve current history head
        let parent_commit_id = self.resolve_history_head().await?;
        let mut root_items = if let Some(parent_id) = parent_commit_id {
            self.load_commit_tree(&parent_id)?
        } else {
            Vec::new()
        };

        // 2. Update Tree
        // Structure: <type>/<id> -> blob
        let type_tree_entry = root_items
            .iter()
            .find(|item| item.name == object_type)
            .cloned();

        let mut type_items = if let Some(entry) = type_tree_entry {
            self.load_tree(&entry.id).unwrap_or_default()
        } else {
            Vec::new()
        };

        // Add/Update the object in the sub-tree
        let new_item = TreeItem::new(TreeItemMode::Blob, blob_hash, object_id.to_string());
        // Remove existing if any (to support updates)
        type_items.retain(|item| item.name != object_id);
        type_items.push(new_item);
        type_items.sort_by(|a, b| a.name.cmp(&b.name));

        // Write sub-tree
        let type_tree_hash = self.write_tree(&type_items)?;

        // Update root tree
        let new_root_item =
            TreeItem::new(TreeItemMode::Tree, type_tree_hash, object_type.to_string());
        root_items.retain(|item| item.name != object_type);
        root_items.push(new_root_item);
        root_items.sort_by(|a, b| a.name.cmp(&b.name));

        // Write root tree
        let root_tree_hash = self.write_tree(&root_items)?;

        let author = Signature::new(
            SignatureType::Author,
            "Libra".to_string(),
            "history@libra".to_string(),
        );

        let signature = Signature::new(
            SignatureType::Committer,
            "Libra".to_string(),
            "history@libra".to_string(),
        );

        let message = format!("Update {}/{}", object_type, object_id);

        let parents = if let Some(p) = parent_commit_id {
            vec![p]
        } else {
            vec![]
        };

        // Manual Commit Serialization to ensure correct Git object format
        // Format:
        // tree <tree_hash>
        // parent <parent_hash>
        // author <author_sig>
        // committer <committer_sig>
        //
        // <message>
        let mut commit_content = String::new();
        commit_content.push_str(&format!("tree {}\n", root_tree_hash));
        for parent in &parents {
            commit_content.push_str(&format!("parent {}\n", parent));
        }
        commit_content.push_str(&format!("author {}\n", author));
        commit_content.push_str(&format!("committer {}\n", signature));
        commit_content.push('\n');
        commit_content.push_str(&message);

        // Serialize and write commit
        let commit_hash = write_git_object(&self.repo_path, "commit", commit_content.as_bytes())?;

        // 4. Update Ref
        self.update_ref(&self.ref_name, commit_hash)?;

        Ok(())
    }

    /// Retrieve the object hash for a given type and ID from the current history.
    pub async fn get_object_hash(
        &self,
        object_type: &str,
        object_id: &str,
    ) -> Result<Option<ObjectHash>, GitError> {
        let parent_commit_id = self.resolve_history_head().await?;
        if let Some(parent_id) = parent_commit_id {
            let root_items = self.load_commit_tree(&parent_id)?;
            if let Some(type_entry) = root_items.iter().find(|item| item.name == object_type) {
                let type_items = self.load_tree(&type_entry.id)?;
                if let Some(item) = type_items.iter().find(|item| item.name == object_id) {
                    return Ok(Some(item.id));
                }
            }
        }
        Ok(None)
    }

    /// Find an object by ID across all types in the history.
    /// Returns (hash, type).
    pub async fn find_object_hash(
        &self,
        object_id: &str,
    ) -> Result<Option<(ObjectHash, String)>, GitError> {
        let parent_commit_id = self.resolve_history_head().await?;
        if let Some(parent_id) = parent_commit_id {
            let root_items = self.load_commit_tree(&parent_id)?;
            for type_entry in root_items {
                let type_items = self.load_tree(&type_entry.id)?;
                if let Some(item) = type_items.iter().find(|item| item.name == object_id) {
                    return Ok(Some((item.id, type_entry.name.clone())));
                }
            }
        }
        Ok(None)
    }

    /// List all objects of a specific type from the current history.
    /// Returns a list of (object_id, object_hash).
    pub async fn list_objects(
        &self,
        object_type: &str,
    ) -> Result<Vec<(String, ObjectHash)>, GitError> {
        let parent_commit_id = self.resolve_history_head().await?;
        if let Some(parent_id) = parent_commit_id {
            let root_items = self.load_commit_tree(&parent_id)?;
            if let Some(type_entry) = root_items.iter().find(|item| item.name == object_type) {
                let type_items = self.load_tree(&type_entry.id)?;
                return Ok(type_items
                    .into_iter()
                    .map(|item| (item.name, item.id))
                    .collect());
            }
        }
        Ok(Vec::new())
    }

    pub async fn resolve_history_head(&self) -> Result<Option<ObjectHash>, GitError> {
        let ref_path = self.repo_path.join(&self.ref_name);
        if !ref_path.exists() {
            return Ok(None);
        }

        let content = std::fs::read_to_string(&ref_path).map_err(GitError::IOError)?;
        let hash_str = content.trim();
        if hash_str.is_empty() {
            return Ok(None);
        }

        ObjectHash::from_str(hash_str)
            .map(Some)
            .map_err(|e| GitError::InvalidObjectInfo(e.to_string()))
    }

    fn load_commit_tree(&self, commit_id: &ObjectHash) -> Result<Vec<TreeItem>, GitError> {
        let data = read_git_object(&self.repo_path, commit_id)?;
        // Commit format: tree <hash>\nparent...
        let content = String::from_utf8_lossy(&data);
        for line in content.lines() {
            if let Some(hash_str) = line.strip_prefix("tree ") {
                let tree_hash = ObjectHash::from_str(hash_str)
                    .map_err(|e| GitError::InvalidObjectInfo(e.to_string()))?;
                return self.load_tree(&tree_hash);
            }
        }
        Err(GitError::InvalidObjectInfo("Commit has no tree".into()))
    }

    fn load_tree(&self, tree_id: &ObjectHash) -> Result<Vec<TreeItem>, GitError> {
        let data = read_git_object(&self.repo_path, tree_id)?;

        let tree = Tree::from_bytes(&data, *tree_id)?;
        Ok(tree.tree_items)
    }

    fn write_tree(&self, tree_items: &[TreeItem]) -> Result<ObjectHash, GitError> {
        let mut data = Vec::new();
        for item in tree_items {
            let mode_str = match item.mode {
                TreeItemMode::Tree => "40000",
                TreeItemMode::Blob => "100644",
                TreeItemMode::BlobExecutable => "100755",
                TreeItemMode::Link => "120000",
                TreeItemMode::Commit => "160000",
            };
            data.extend_from_slice(mode_str.as_bytes());
            data.push(b' ');
            data.extend_from_slice(item.name.as_bytes());
            data.push(0);
            let hash_hex = item.id.to_string();
            let hash_bytes =
                hex::decode(&hash_hex).map_err(|e| GitError::InvalidObjectInfo(e.to_string()))?;
            if hash_bytes.len() != 20 && hash_bytes.len() != 32 {
                return Err(GitError::InvalidObjectInfo(format!(
                    "Invalid object hash length: {}",
                    hash_bytes.len()
                )));
            }
            data.extend_from_slice(&hash_bytes);
        }

        write_git_object(&self.repo_path, "tree", &data)
    }

    fn update_ref(&self, ref_name: &str, hash: ObjectHash) -> Result<(), GitError> {
        let path = self.repo_path.join(ref_name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(GitError::IOError)?;
        }
        std::fs::write(path, hash.to_string()).map_err(GitError::IOError)?;
        Ok(())
    }

    #[cfg(test)]
    pub fn get_storage(&self) -> Arc<dyn Storage + Send + Sync> {
        self.storage.clone()
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::utils::storage::local::LocalStorage;

    #[tokio::test]
    async fn test_history_append_simple() {
        let dir = tempdir().unwrap();
        let repo_path = dir.path().join(".libra");
        std::fs::create_dir(&repo_path).unwrap();
        let objects_dir = repo_path.join("objects");

        let storage = Arc::new(LocalStorage::new(objects_dir));
        let manager = HistoryManager::new(storage.clone(), repo_path.clone());

        // 1. Append first object
        let blob_hash = ObjectHash::from_str("e69de29bb2d1d6434b8b29ae775ad8c2e48c5391").unwrap();
        manager.append("task", "task-1", blob_hash).await.unwrap();

        // Verify ref exists
        let history_ref = repo_path.join("refs/libra/history");
        assert!(history_ref.exists());

        // 2. Append second object (same type)
        let blob_hash_2 = ObjectHash::from_str("f4e6d0434b8b29ae775ad8c2e48c5391e69de29b").unwrap();
        manager.append("task", "task-2", blob_hash_2).await.unwrap();

        // 3. Append third object (different type)
        manager.append("run", "run-1", blob_hash).await.unwrap();

        // Load Head Commit
        let commit_hash_str = std::fs::read_to_string(history_ref).unwrap();
        let commit_hash = ObjectHash::from_str(commit_hash_str.trim()).unwrap();

        // Verify we can load commit
        let data = read_git_object(&repo_path, &commit_hash).unwrap();
        let content = String::from_utf8_lossy(&data);
        assert!(content.contains("tree "));
        assert!(content.contains("Update run/run-1"));
    }
}
