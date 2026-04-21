use std::{path::PathBuf, str::FromStr, sync::Arc, time::Duration};

use anyhow::{Context, Result, anyhow};
use git_internal::{
    hash::ObjectHash,
    internal::object::{
        ObjectTrait,
        commit::Commit,
        signature::{Signature, SignatureType},
        tree::{Tree, TreeItem, TreeItemMode},
    },
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DatabaseTransaction, DbErr, EntityTrait,
    QueryFilter, Set, TransactionTrait,
};
use tokio::time::sleep;

use crate::{
    internal::model::reference::{self, ConfigKind},
    utils::{
        object::{read_git_object, write_git_object},
        storage::Storage,
    },
};

/// Default Git reference for the AI history orphan branch.
///
/// All AI process objects (Intent, Task, Run, Plan, PatchSet, Evidence,
/// ToolInvocation, Provenance, Decision) live on this single branch,
/// running in parallel with the normal code branch (`refs/heads/*`).
///
/// By keeping AI objects reachable from this ref, they are protected
/// from `git gc` — the branch acts as a GC root.
///
/// In the database, this is stored with kind='Branch' and name='libra/intent'.
pub const AI_REF: &str = "libra/intent";
const SQLITE_BUSY_MAX_RETRIES: usize = 15;
const SQLITE_BUSY_RETRY_BASE_MS: u64 = 100;

fn is_sqlite_busy(err: &DbErr) -> bool {
    let message = err.to_string();
    message.contains("database is locked") || message.contains("database schema is locked")
}

/// Manages object history using an orphan branch and Git Tree structure.
///
/// The default branch (`libra/intent`) stores **all** AI workflow objects,
/// running in parallel with the normal code history (`refs/heads/*`).
/// This is initialised during `libra init` so both branches exist from the start.
///
/// Structure (Commit -> Tree):
///   ├── intent/
///   │   └── <intent_id>
///   ├── task/
///   │   └── <task_id>
///   ├── run/
///   │   └── <run_id>
///   ├── plan/
///   │   └── <plan_id>
///   └── …
pub struct HistoryManager {
    #[allow(dead_code)]
    storage: Arc<dyn Storage + Send + Sync>,
    repo_path: PathBuf,
    db_conn: Arc<DatabaseConnection>,
    /// The reference name this manager writes to (e.g. "libra/intent").
    ref_name: String,
}

impl HistoryManager {
    pub fn new(
        storage: Arc<dyn Storage + Send + Sync>,
        repo_path: PathBuf,
        db_conn: Arc<DatabaseConnection>,
    ) -> Self {
        Self::new_with_ref(storage, repo_path, db_conn, AI_REF)
    }

    pub fn new_with_ref(
        storage: Arc<dyn Storage + Send + Sync>,
        repo_path: PathBuf,
        db_conn: Arc<DatabaseConnection>,
        ref_name: impl Into<String>,
    ) -> Self {
        Self {
            storage,
            repo_path,
            db_conn,
            ref_name: ref_name.into(),
        }
    }

    pub fn database_connection(&self) -> DatabaseConnection {
        self.db_conn.as_ref().clone()
    }

    /// Initialise the AI orphan branch with an empty tree commit.
    ///
    /// This should be called once during `libra init` so that the AI ref
    /// exists from the start (parallel to `refs/heads/<branch>`).
    /// If the ref already exists this is a no-op.
    pub async fn init_branch(&self) -> Result<()> {
        // Already initialised — nothing to do.
        if self.resolve_history_head().await?.is_some() {
            return Ok(());
        }

        // Write an empty tree.
        let empty_tree_hash = self.write_tree(&[])?;

        let author = Signature::new(
            SignatureType::Author,
            "Libra".to_string(),
            "ai@libra".to_string(),
        );
        let committer = Signature::new(
            SignatureType::Committer,
            "Libra".to_string(),
            "ai@libra".to_string(),
        );

        let commit = Commit::new(
            author,
            committer,
            empty_tree_hash,
            vec![],
            "Initialize AI history branch",
        );

        let commit_data = commit
            .to_data()
            .context("Failed to serialize AI history init commit")?;
        let commit_hash = write_git_object(&self.repo_path, "commit", &commit_data)?;
        self.update_ref(&self.ref_name, commit_hash).await?;

        Ok(())
    }

    /// Return the ref name this manager writes to.
    pub fn ref_name(&self) -> &str {
        &self.ref_name
    }

    /// Append an object to the history log.
    /// This operation is synchronous (commits immediately) for the MVP.
    pub async fn append(
        &self,
        object_type: &str,
        object_id: &str,
        blob_hash: ObjectHash,
    ) -> Result<()> {
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
            self.load_tree(&entry.id)?
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

        let commit = Commit::new(author, signature, root_tree_hash, parents, &message);

        // Serialize and write commit
        let commit_data = commit
            .to_data()
            .context("Failed to serialize AI history commit")?;
        let commit_hash = write_git_object(&self.repo_path, "commit", &commit_data)?;

        // 4. Update Ref
        self.update_ref(&self.ref_name, commit_hash).await?;

        Ok(())
    }

    /// Retrieve the object hash for a given type and ID from the current history.
    pub async fn get_object_hash(
        &self,
        object_type: &str,
        object_id: &str,
    ) -> Result<Option<ObjectHash>> {
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
    pub async fn find_object_hash(&self, object_id: &str) -> Result<Option<(ObjectHash, String)>> {
        Ok(self.find_object_hashes(object_id).await?.into_iter().next())
    }

    /// Find all objects that share the same object ID across history types.
    pub async fn find_object_hashes(&self, object_id: &str) -> Result<Vec<(ObjectHash, String)>> {
        let parent_commit_id = self.resolve_history_head().await?;
        if let Some(parent_id) = parent_commit_id {
            let root_items = self.load_commit_tree(&parent_id)?;
            let mut matches = Vec::new();
            for type_entry in root_items {
                let type_items = self.load_tree(&type_entry.id)?;
                if let Some(item) = type_items.iter().find(|item| item.name == object_id) {
                    matches.push((item.id, type_entry.name.clone()));
                }
            }
            return Ok(matches);
        }
        Ok(Vec::new())
    }

    /// List all objects of a specific type from the current history.
    /// Returns a list of (object_id, object_hash).
    pub async fn list_objects(&self, object_type: &str) -> Result<Vec<(String, ObjectHash)>> {
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

    /// List all object types present at the current history head.
    pub async fn list_object_types(&self) -> Result<Vec<String>> {
        let parent_commit_id = self.resolve_history_head().await?;
        if let Some(parent_id) = parent_commit_id {
            let mut root_items = self.load_commit_tree(&parent_id)?;
            root_items.sort_by(|a, b| a.name.cmp(&b.name));
            return Ok(root_items.into_iter().map(|item| item.name).collect());
        }
        Ok(Vec::new())
    }

    pub async fn resolve_history_head(&self) -> Result<Option<ObjectHash>> {
        let mut attempt = 0;
        let ref_model = loop {
            match reference::Entity::find()
                .filter(reference::Column::Name.eq(&self.ref_name))
                .filter(reference::Column::Kind.eq(ConfigKind::Branch))
                .one(&*self.db_conn)
                .await
            {
                Ok(found) => break found,
                Err(err) if is_sqlite_busy(&err) && attempt < SQLITE_BUSY_MAX_RETRIES => {
                    attempt += 1;
                    sleep(Duration::from_millis(
                        SQLITE_BUSY_RETRY_BASE_MS * attempt as u64,
                    ))
                    .await;
                }
                Err(err) => return Err(err).context("Failed to query history head"),
            }
        };

        match ref_model {
            Some(model) => match model.commit {
                Some(commit_hash) => ObjectHash::from_str(&commit_hash)
                    .map(Some)
                    .map_err(|e| anyhow!("Invalid commit hash in DB: {}", e)),
                None => Ok(None),
            },
            None => Ok(None),
        }
    }

    fn load_commit_tree(&self, commit_id: &ObjectHash) -> Result<Vec<TreeItem>> {
        let data = read_git_object(&self.repo_path, commit_id)?;
        // Commit format: tree <hash>\nparent...
        let content = String::from_utf8_lossy(&data);
        for line in content.lines() {
            if let Some(hash_str) = line.strip_prefix("tree ") {
                let tree_hash = ObjectHash::from_str(hash_str)
                    .map_err(|e| anyhow!("Invalid tree hash in commit: {}", e))?;
                return self.load_tree(&tree_hash);
            }
        }
        Err(anyhow!("Commit has no tree"))
    }

    fn load_tree(&self, tree_id: &ObjectHash) -> Result<Vec<TreeItem>> {
        let data = read_git_object(&self.repo_path, tree_id)?;

        let tree = Tree::from_bytes(&data, *tree_id)?;
        Ok(tree.tree_items)
    }

    fn write_tree(&self, tree_items: &[TreeItem]) -> Result<ObjectHash> {
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
                hex::decode(&hash_hex).map_err(|e| anyhow!("Invalid hash hex: {}", e))?;
            if hash_bytes.len() != 20 && hash_bytes.len() != 32 {
                return Err(anyhow!("Invalid object hash length: {}", hash_bytes.len()));
            }
            data.extend_from_slice(&hash_bytes);
        }

        Ok(write_git_object(&self.repo_path, "tree", &data)?)
    }

    async fn update_ref(&self, ref_name: &str, hash: ObjectHash) -> Result<()> {
        for attempt in 0..=SQLITE_BUSY_MAX_RETRIES {
            let txn: DatabaseTransaction = match self.db_conn.begin().await {
                Ok(txn) => txn,
                Err(err) if is_sqlite_busy(&err) && attempt < SQLITE_BUSY_MAX_RETRIES => {
                    sleep(Duration::from_millis(
                        SQLITE_BUSY_RETRY_BASE_MS * (attempt as u64 + 1),
                    ))
                    .await;
                    continue;
                }
                Err(err) => return Err(err).context("Failed to begin transaction"),
            };

            let existing = match reference::Entity::find()
                .filter(reference::Column::Name.eq(ref_name))
                .filter(reference::Column::Kind.eq(ConfigKind::Branch))
                .one(&txn)
                .await
            {
                Ok(existing) => existing,
                Err(err) if is_sqlite_busy(&err) && attempt < SQLITE_BUSY_MAX_RETRIES => {
                    let _ = txn.rollback().await;
                    sleep(Duration::from_millis(
                        SQLITE_BUSY_RETRY_BASE_MS * (attempt as u64 + 1),
                    ))
                    .await;
                    continue;
                }
                Err(err) => return Err(err).context("Failed to query reference"),
            };

            let had_existing = existing.is_some();
            let write_result = if let Some(model) = existing {
                let mut active: reference::ActiveModel = model.into();
                active.commit = Set(Some(hash.to_string()));
                active.update(&txn).await.map(|_| ())
            } else {
                let new_ref = reference::ActiveModel {
                    name: Set(Some(ref_name.to_string())),
                    kind: Set(ConfigKind::Branch),
                    commit: Set(Some(hash.to_string())),
                    remote: Set(None),
                    ..Default::default()
                };
                new_ref.insert(&txn).await.map(|_| ())
            };

            match write_result {
                Ok(()) => {}
                Err(err) if is_sqlite_busy(&err) && attempt < SQLITE_BUSY_MAX_RETRIES => {
                    let _ = txn.rollback().await;
                    sleep(Duration::from_millis(
                        SQLITE_BUSY_RETRY_BASE_MS * (attempt as u64 + 1),
                    ))
                    .await;
                    continue;
                }
                Err(err) => {
                    let context = if had_existing {
                        "Failed to update reference"
                    } else {
                        "Failed to insert reference"
                    };
                    return Err(err).context(context);
                }
            }

            match txn.commit().await {
                Ok(()) => return Ok(()),
                Err(err) if is_sqlite_busy(&err) && attempt < SQLITE_BUSY_MAX_RETRIES => {
                    sleep(Duration::from_millis(
                        SQLITE_BUSY_RETRY_BASE_MS * (attempt as u64 + 1),
                    ))
                    .await;
                }
                Err(err) => return Err(err).context("Failed to commit transaction"),
            }
        }

        unreachable!("sqlite busy retry loop must return on success or terminal error")
    }

    #[cfg(test)]
    pub fn get_storage(&self) -> Arc<dyn Storage + Send + Sync> {
        self.storage.clone()
    }
}

#[cfg(test)]
mod tests {
    use sea_orm::{ConnectionTrait, Database, Schema, Statement};
    use tempfile::tempdir;
    use tokio::time::sleep;

    use super::*;
    use crate::{internal::db, utils::storage::local::LocalStorage};

    async fn setup_test_db() -> DatabaseConnection {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        let builder = db.get_database_backend();
        let schema = Schema::new(builder);
        let stmt = schema.create_table_from_entity(reference::Entity);
        db.execute(builder.build(&stmt)).await.unwrap();
        db
    }

    #[tokio::test]
    async fn test_history_append_simple() {
        let dir = tempdir().unwrap();
        let repo_path = dir.path().join(".libra");
        std::fs::create_dir(&repo_path).unwrap();
        let objects_dir = repo_path.join("objects");

        let storage = Arc::new(LocalStorage::new(objects_dir));
        let db_conn = Arc::new(setup_test_db().await);
        let manager = HistoryManager::new(storage.clone(), repo_path.clone(), db_conn.clone());

        // 1. Append first object
        let blob_hash = ObjectHash::from_str("e69de29bb2d1d6434b8b29ae775ad8c2e48c5391").unwrap();
        manager.append("task", "task-1", blob_hash).await.unwrap();

        // Verify ref exists in DB
        let ref_model = reference::Entity::find()
            .filter(reference::Column::Name.eq(AI_REF))
            .filter(reference::Column::Kind.eq(ConfigKind::Branch))
            .one(&*db_conn)
            .await
            .unwrap()
            .expect("Reference should exist");

        let commit_hash_str = ref_model.commit.expect("Commit hash should exist");
        let commit_hash = ObjectHash::from_str(&commit_hash_str).unwrap();

        // Verify we can load commit
        let data = read_git_object(&repo_path, &commit_hash).unwrap();
        let content = String::from_utf8_lossy(&data);
        assert!(content.contains("tree "));
        assert!(content.contains("Update task/task-1"));

        // 2. Append second object (same type)
        let blob_hash_2 = ObjectHash::from_str("f4e6d0434b8b29ae775ad8c2e48c5391e69de29b").unwrap();
        manager.append("task", "task-2", blob_hash_2).await.unwrap();

        // 3. Append third object (different type)
        manager.append("run", "run-1", blob_hash).await.unwrap();

        // Load Head Commit from DB
        let head = manager.resolve_history_head().await.unwrap().unwrap();

        // Verify we can load commit
        let data = read_git_object(&repo_path, &head).unwrap();
        let content = String::from_utf8_lossy(&data);
        assert!(content.contains("tree "));
        assert!(content.contains("Update run/run-1"));
    }

    #[tokio::test]
    async fn test_find_object_hashes_returns_all_matching_types() {
        let dir = tempdir().unwrap();
        let repo_path = dir.path().join(".libra");
        std::fs::create_dir(&repo_path).unwrap();
        let objects_dir = repo_path.join("objects");

        let storage = Arc::new(LocalStorage::new(objects_dir));
        let db_conn = Arc::new(setup_test_db().await);
        let manager = HistoryManager::new(storage.clone(), repo_path.clone(), db_conn.clone());

        let blob_hash = ObjectHash::from_str("e69de29bb2d1d6434b8b29ae775ad8c2e48c5391").unwrap();
        let other_hash = ObjectHash::from_str("f4e6d0434b8b29ae775ad8c2e48c5391e69de29b").unwrap();

        manager
            .append("patchset", "shared-id", blob_hash)
            .await
            .unwrap();
        manager
            .append("event", "shared-id", other_hash)
            .await
            .unwrap();

        let matches = manager.find_object_hashes("shared-id").await.unwrap();
        assert_eq!(matches.len(), 2);
        assert!(matches.iter().any(|(_, kind)| kind == "patchset"));
        assert!(matches.iter().any(|(_, kind)| kind == "event"));
    }

    #[tokio::test]
    async fn test_list_object_types_returns_sorted_types() {
        let dir = tempdir().unwrap();
        let repo_path = dir.path().join(".libra");
        std::fs::create_dir(&repo_path).unwrap();
        let objects_dir = repo_path.join("objects");

        let storage = Arc::new(LocalStorage::new(objects_dir));
        let db_conn = Arc::new(setup_test_db().await);
        let manager = HistoryManager::new(storage.clone(), repo_path.clone(), db_conn.clone());

        let blob_hash = ObjectHash::from_str("e69de29bb2d1d6434b8b29ae775ad8c2e48c5391").unwrap();
        manager
            .append("run_event", "run-event-1", blob_hash)
            .await
            .unwrap();
        manager
            .append("patchset", "patchset-1", blob_hash)
            .await
            .unwrap();

        let types = manager.list_object_types().await.unwrap();
        assert_eq!(types, vec!["patchset".to_string(), "run_event".to_string()]);
    }

    #[tokio::test]
    async fn test_update_ref_retries_when_sqlite_is_locked() {
        let dir = tempdir().unwrap();
        let repo_path = dir.path().join(".libra");
        std::fs::create_dir(&repo_path).unwrap();
        let objects_dir = repo_path.join("objects");
        std::fs::create_dir(&objects_dir).unwrap();
        let db_path = repo_path.join("libra.db");

        let db_conn = Arc::new(
            db::create_database(db_path.to_str().unwrap())
                .await
                .expect("failed to create sqlite database"),
        );
        let storage = Arc::new(LocalStorage::new(objects_dir));
        let manager = HistoryManager::new(storage, repo_path.clone(), db_conn.clone());

        let locker = db::establish_connection_with_busy_timeout(
            db_path.to_str().unwrap(),
            Duration::from_millis(50),
        )
        .await
        .expect("failed to open lock holder connection");
        let backend = locker.get_database_backend();
        locker
            .execute(Statement::from_string(backend, "BEGIN EXCLUSIVE"))
            .await
            .expect("failed to acquire sqlite exclusive lock");

        let release = {
            let locker = locker.clone();
            tokio::spawn(async move {
                sleep(Duration::from_millis(250)).await;
                let backend = locker.get_database_backend();
                locker
                    .execute(Statement::from_string(backend, "COMMIT"))
                    .await
                    .expect("failed to release sqlite exclusive lock");
            })
        };

        let hash = ObjectHash::from_str("e69de29bb2d1d6434b8b29ae775ad8c2e48c5391").unwrap();
        manager
            .update_ref(AI_REF, hash)
            .await
            .expect("update_ref should retry through a transient sqlite lock");
        release.await.unwrap();

        let resolved = manager
            .resolve_history_head()
            .await
            .expect("history head should be readable after retry")
            .expect("history head should exist");
        assert_eq!(resolved, hash);
    }
}
