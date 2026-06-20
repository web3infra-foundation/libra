//! AI workflow history persistence backed by an orphan Git branch.
//!
//! Libra records every AI process artefact (Intent, Task, Run, Plan,
//! PatchSet, Evidence, ToolInvocation, Provenance, Decision, ContextFrame,
//! ...) on a parallel branch named [`AI_REF`] (`libra/intent`). The branch
//! is *orphan*: it shares no history with the user's code branches but
//! lives inside the same object database, which means:
//!
//! * The same `git gc` policy keeps both AI history and code history
//!   reachable.
//! * AI artefacts are content-addressed under standard Git rules and can be
//!   transferred via the same protocol as the rest of the repository.
//!
//! Each commit on this ref points to a tree that is partitioned by object
//! type (`intent/`, `task/`, `plan/`, ...), with one blob per object id
//! beneath the type subtree. The flow for `append` is:
//!
//! 1. Read the current head (with retry on a busy SQLite) — see
//!    [`HistoryManager::resolve_history_head`].
//! 2. Load that head's root tree, splice the new entry in beneath its type
//!    subtree, write a fresh root tree, and create a child commit — see
//!    [`HistoryManager::create_append_commit`].
//! 3. Compare-and-swap the ref forward, retrying on a stale head — see
//!    [`HistoryManager::update_ref_if_matches`].
//!
//! Concurrency is handled via two retry loops: a SQLite-busy retry that
//! covers transient lock contention, and a head-conflict retry that re-reads
//! the head and retries the splice when another process advanced the ref.
//! Both loops have bounded iteration counts so misuse cannot deadlock the
//! caller.

use std::{collections::HashSet, path::PathBuf, str::FromStr, sync::Arc, time::Duration};

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
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbErr,
    EntityTrait, QueryFilter, QueryResult, Set, SqlErr, Statement, TransactionTrait, Value,
    sea_query::Expr,
};
use tokio::time::sleep;

use crate::{
    internal::{
        ai::observed_agents::RedactedBytes,
        model::reference::{self, ConfigKind},
    },
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
/// Maximum attempts to retry a SQLite operation that returns a transient
/// "database is locked" error before propagating the failure.
const SQLITE_BUSY_MAX_RETRIES: usize = 15;
/// Base delay (ms) for the linear backoff applied between SQLite-busy retries.
/// The actual delay is `BASE * attempt`, so the worst-case wait is roughly
/// `BASE * SUM(1..=MAX_RETRIES)` which keeps total time bounded.
const SQLITE_BUSY_RETRY_BASE_MS: u64 = 100;
/// Maximum attempts to re-read the history head and retry a splice when a
/// concurrent writer advances the ref between read and CAS. The bound is
/// generous because each retry is purely local (no network I/O).
const HISTORY_HEAD_CONFLICT_MAX_RETRIES: usize = 32;

/// Outcome of a compare-and-swap reference update.
///
/// Used by [`HistoryManager::update_ref_if_matches`] to communicate whether
/// the ref moved successfully (`Updated`) or whether the expected head was
/// stale and the caller must restart the splice (`HeadChanged`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RefUpdateOutcome {
    /// The ref was atomically advanced to the new commit.
    Updated,
    /// Another writer advanced the ref before our CAS — caller should
    /// re-read the head and rebuild the commit on top of it.
    HeadChanged,
}

/// Detect transient SQLite contention that should trigger a retry.
///
/// Functional scope:
/// - Inspects the error message for the well-known "database is locked" or
///   "database schema is locked" substrings emitted by SQLite under busy
///   contention.
///
/// Boundary conditions:
/// - This is intentionally a string match: the SeaORM error wraps the
///   underlying SQLite text, and there is no stable error-code variant for
///   busy/lock conditions in the wrapping layer.
fn is_sqlite_busy(err: &DbErr) -> bool {
    let message = err.to_string();
    message.contains("database is locked") || message.contains("database schema is locked")
}

/// Detect unique-constraint violations on the `reference` table.
///
/// Functional scope:
/// - Used by the optimistic CAS path: when two writers race to insert the
///   same ref name, one will see a unique-constraint violation; we treat
///   that as a `HeadChanged` outcome rather than a hard error.
fn is_sqlite_unique_violation(err: &DbErr) -> bool {
    matches!(err.sql_err(), Some(SqlErr::UniqueConstraintViolation(_)))
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
///
/// The manager is cheap to clone (all state lives behind `Arc` or owned
/// `String`/`PathBuf`) and is safe to share across async tasks. Concurrent
/// `append` calls on the same manager are serialised via the SQLite-side
/// CAS in [`Self::update_ref_if_matches`].
pub struct HistoryManager {
    #[allow(dead_code)]
    storage: Arc<dyn Storage + Send + Sync>,
    repo_path: PathBuf,
    db_conn: Arc<DatabaseConnection>,
    /// The reference name this manager writes to (e.g. "libra/intent").
    ref_name: String,
}

impl HistoryManager {
    /// Build a manager bound to the canonical [`AI_REF`].
    ///
    /// Functional scope:
    /// - Convenience constructor that delegates to [`Self::new_with_ref`]
    ///   with the standard `libra/intent` branch.
    pub fn new(
        storage: Arc<dyn Storage + Send + Sync>,
        repo_path: PathBuf,
        db_conn: Arc<DatabaseConnection>,
    ) -> Self {
        Self::new_with_ref(storage, repo_path, db_conn, AI_REF)
    }

    /// Build a manager bound to an arbitrary ref name.
    ///
    /// Functional scope:
    /// - Used by tests and tooling that need to write a parallel AI history
    ///   under a custom ref (e.g. for staging, comparison, or namespace
    ///   isolation).
    ///
    /// Boundary conditions:
    /// - The ref name is not validated here; callers must ensure it is a
    ///   legal Git ref. The CAS path will fail loudly if the database
    ///   constraint rejects it.
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

    /// Hand back a clone of the underlying SeaORM connection.
    ///
    /// Functional scope:
    /// - Convenience accessor for callers that need to issue auxiliary
    ///   queries against the same database (e.g. listing references for the
    ///   TUI) without having to thread a separate `Arc` around.
    pub fn database_connection(&self) -> DatabaseConnection {
        self.db_conn.as_ref().clone()
    }

    /// Initialise the AI orphan branch with an empty tree commit.
    ///
    /// This should be called once during `libra init` so that the AI ref
    /// exists from the start (parallel to `refs/heads/<branch>`).
    /// If the ref already exists this is a no-op.
    ///
    /// Functional scope:
    /// - Writes a single empty-tree commit and points the ref at it. The
    ///   commit has no parents (it is the root of the orphan branch) and
    ///   uses the canonical `Libra <ai@libra>` signatures so authorship is
    ///   traceable.
    ///
    /// Boundary conditions:
    /// - Returns early if the ref already exists; this makes the call
    ///   idempotent and safe to invoke from `libra init` regardless of
    ///   whether previous initialisations completed.
    /// - Surfaces errors from object serialisation, blob writing, or the
    ///   ref CAS so the caller can present an actionable message.
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
    ///
    /// Functional scope:
    /// - Useful for diagnostics, log messages, and TUI labels that need to
    ///   present the active AI history branch to the user.
    pub fn ref_name(&self) -> &str {
        &self.ref_name
    }

    /// Append an object to the history log.
    /// This operation is synchronous (commits immediately) for the MVP.
    ///
    /// Functional scope:
    /// - Implements the read-merge-CAS loop:
    ///   1. Read the current head.
    ///   2. Write a new commit that adds `<object_type>/<object_id>`
    ///      (replacing any prior entry under that path).
    ///   3. CAS the ref forward.
    /// - Reuses [`Self::create_append_commit`] for splice logic and
    ///   [`Self::update_ref_if_matches`] for the optimistic ref update.
    ///
    /// Boundary conditions:
    /// - Retries up to [`HISTORY_HEAD_CONFLICT_MAX_RETRIES`] times when a
    ///   concurrent writer advances the ref between read and CAS. After the
    ///   bound is exhausted the call fails with a contextual error so the
    ///   caller can decide whether to back off and retry.
    /// - The intermediate commit objects from failed CAS attempts remain in
    ///   the object database as garbage; they are unreachable and will be
    ///   collected by the next `libra gc` cycle.
    ///
    /// See: `tests::test_history_append_simple` and
    /// `tests::test_update_ref_if_matches_rejects_stale_history_head`.
    pub async fn append(
        &self,
        object_type: &str,
        object_id: &str,
        blob_hash: ObjectHash,
    ) -> Result<()> {
        for attempt in 0..=HISTORY_HEAD_CONFLICT_MAX_RETRIES {
            // Phase 1: snapshot the head we are racing against.
            let parent_commit_id = self.resolve_history_head().await?;
            // Phase 2: build the new commit on top of the snapshot.
            let commit_hash =
                self.create_append_commit(parent_commit_id, object_type, object_id, blob_hash)?;

            // Phase 3: atomically advance the ref iff its current value still
            // equals the snapshot. On `HeadChanged`, restart from phase 1.
            match self
                .update_ref_if_matches(&self.ref_name, parent_commit_id, commit_hash)
                .await?
            {
                RefUpdateOutcome::Updated => return Ok(()),
                RefUpdateOutcome::HeadChanged if attempt < HISTORY_HEAD_CONFLICT_MAX_RETRIES => {
                    continue;
                }
                RefUpdateOutcome::HeadChanged => {
                    return Err(anyhow!(
                        "history head changed repeatedly while appending {}/{}",
                        object_type,
                        object_id
                    ));
                }
            }
        }

        unreachable!("head conflict retry loop must return on success or terminal error")
    }

    /// Retrieve the object hash for a given type and ID from the current history.
    ///
    /// Functional scope:
    /// - Resolves the head commit, walks `<root_tree>/<object_type>/<object_id>`,
    ///   and returns the leaf blob hash if it exists.
    ///
    /// Boundary conditions:
    /// - Returns `Ok(None)` when the ref is not initialised, when no
    ///   subtree exists for `object_type`, or when the `object_id` entry is
    ///   missing under that subtree.
    /// - Surfaces `Err` only for object-store / parse failures.
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
    ///
    /// Functional scope:
    /// - Convenience wrapper around [`Self::find_object_hashes`] that
    ///   returns only the first match.
    ///
    /// Boundary conditions:
    /// - When the same object id exists under multiple type subtrees the
    ///   caller has no control over which is chosen; use
    ///   [`Self::find_object_hashes`] when a deterministic tie-break is
    ///   required.
    pub async fn find_object_hash(&self, object_id: &str) -> Result<Option<(ObjectHash, String)>> {
        Ok(self.find_object_hashes(object_id).await?.into_iter().next())
    }

    /// Find all objects that share the same object ID across history types.
    ///
    /// Functional scope:
    /// - Walks every type subtree under the head root tree and collects
    ///   `(blob_hash, type_name)` tuples for every subtree containing
    ///   `object_id`.
    ///
    /// Boundary conditions:
    /// - Returns an empty vector when the ref is not initialised or the id
    ///   does not appear under any type.
    ///
    /// See: `tests::test_find_object_hashes_returns_all_matching_types`.
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
    ///
    /// Functional scope:
    /// - Loads the head commit's `<object_type>` subtree and yields its
    ///   contents as `(name, blob_hash)` pairs in tree-order.
    ///
    /// Boundary conditions:
    /// - Returns an empty vector when the ref is not initialised or no
    ///   subtree exists for `object_type`.
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
    ///
    /// Functional scope:
    /// - Returns the names of every top-level subtree under the head root,
    ///   sorted lexicographically for stable output.
    ///
    /// Boundary conditions:
    /// - Returns an empty vector when the ref is not initialised. The empty
    ///   tree case (initialised ref with no objects) likewise yields an
    ///   empty vector.
    ///
    /// See: `tests::test_list_object_types_returns_sorted_types`.
    pub async fn list_object_types(&self) -> Result<Vec<String>> {
        let parent_commit_id = self.resolve_history_head().await?;
        if let Some(parent_id) = parent_commit_id {
            let mut root_items = self.load_commit_tree(&parent_id)?;
            root_items.sort_by(|a, b| a.name.cmp(&b.name));
            return Ok(root_items.into_iter().map(|item| item.name).collect());
        }
        Ok(Vec::new())
    }

    /// Resolve the current head commit of the AI history ref.
    ///
    /// Functional scope:
    /// - Queries the `reference` table for the row that matches
    ///   `(name=ref_name, kind=Branch)` and parses its `commit` column into
    ///   an [`ObjectHash`].
    /// - Tolerates transient SQLite-busy errors with a bounded linear
    ///   backoff governed by [`SQLITE_BUSY_MAX_RETRIES`] /
    ///   [`SQLITE_BUSY_RETRY_BASE_MS`].
    ///
    /// Boundary conditions:
    /// - Returns `Ok(None)` when the ref row is missing or its `commit`
    ///   column is `NULL` (the ref exists but points nowhere yet).
    /// - Returns `Err` if the stored commit string is not a valid object
    ///   hash — this indicates database corruption and the caller should
    ///   surface it rather than silently treating it as missing.
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
                    // Linear backoff (BASE * attempt) — see SQLITE_BUSY_* constants.
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

    /// Load the root tree of a commit by parsing its `tree <hash>` header
    /// line.
    ///
    /// Functional scope:
    /// - Reads the commit blob, scans its text lines for the leading
    ///   `tree ` header, parses the referenced tree, and returns its items.
    ///
    /// Boundary conditions:
    /// - Returns an error when the commit blob is missing the `tree`
    ///   header. That should never happen for objects we wrote ourselves
    ///   but we guard against repository corruption.
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

    /// Load and parse a tree object's items.
    ///
    /// Functional scope:
    /// - Thin wrapper around `Tree::from_bytes` for the AI-history call
    ///   sites; centralised so all tree reads go through the same error
    ///   path.
    fn load_tree(&self, tree_id: &ObjectHash) -> Result<Vec<TreeItem>> {
        let data = read_git_object(&self.repo_path, tree_id)?;

        let tree = Tree::from_bytes(&data, *tree_id)?;
        Ok(tree.tree_items)
    }

    /// Serialise tree items into Git's binary tree format and persist as
    /// an object.
    ///
    /// Functional scope:
    /// - Encodes each item as `<mode> <name>\0<binary_hash>` per the Git
    ///   tree spec, concatenates them in caller-provided order, and writes
    ///   the bytes to the object database under type `tree`.
    ///
    /// Boundary conditions:
    /// - Items must already be sorted by the caller (`append`/the splice
    ///   helpers do this). Unsorted items would still parse but would
    ///   produce a different tree hash than canonical Git.
    /// - Rejects hashes whose binary length is not 20 (SHA-1) or 32
    ///   (SHA-256) — protection against malformed inputs that would
    ///   otherwise corrupt the object store.
    fn write_tree(&self, tree_items: &[TreeItem]) -> Result<ObjectHash> {
        Ok(self.write_tree_with_size(tree_items)?.0)
    }

    /// Encode `tree_items` as a Git tree, write the object, and return
    /// `(hash, encoded_size)`. The size is the *content* length (no Git
    /// header) — same convention as `object_index.o_size`.
    ///
    /// Used by the agent capture path (Phase 3.5c) which needs the byte
    /// count to pair with [`crate::utils::client_storage::enqueue_agent_blob_object_index_update`].
    /// All other callers go through [`Self::write_tree`] and discard the
    /// size.
    fn write_tree_with_size(&self, tree_items: &[TreeItem]) -> Result<(ObjectHash, usize)> {
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
            // 20 bytes for SHA-1, 32 for SHA-256. Anything else is a
            // signal that we are about to corrupt the object database.
            if hash_bytes.len() != 20 && hash_bytes.len() != 32 {
                return Err(anyhow!("Invalid object hash length: {}", hash_bytes.len()));
            }
            data.extend_from_slice(&hash_bytes);
        }

        let size = data.len();
        let hash = write_git_object(&self.repo_path, "tree", &data)?;
        Ok((hash, size))
    }

    /// Write a tree object and stamp it into `object_index` with the
    /// given `o_type`. Used by the agent capture path so cloud sync
    /// uploads the trees that compose `refs/libra/agent-traces`.
    fn write_tree_indexed(&self, tree_items: &[TreeItem], o_type: &str) -> Result<ObjectHash> {
        let (hash, size) = self.write_tree_with_size(tree_items)?;
        crate::utils::client_storage::enqueue_agent_blob_object_index_update(
            &self.repo_path,
            &hash.to_string(),
            o_type,
            size as i64,
        );
        Ok(hash)
    }

    fn create_append_commit(
        &self,
        parent_commit_id: Option<ObjectHash>,
        object_type: &str,
        object_id: &str,
        blob_hash: ObjectHash,
    ) -> Result<ObjectHash> {
        let mut root_items = if let Some(parent_id) = parent_commit_id {
            self.load_commit_tree(&parent_id)?
        } else {
            Vec::new()
        };

        let type_tree_entry = root_items
            .iter()
            .find(|item| item.name == object_type)
            .cloned();

        let mut type_items = if let Some(entry) = type_tree_entry {
            self.load_tree(&entry.id)?
        } else {
            Vec::new()
        };

        let new_item = TreeItem::new(TreeItemMode::Blob, blob_hash, object_id.to_string());
        type_items.retain(|item| item.name != object_id);
        type_items.push(new_item);
        type_items.sort_by(|a, b| a.name.cmp(&b.name));

        let type_tree_hash = self.write_tree(&type_items)?;

        let new_root_item =
            TreeItem::new(TreeItemMode::Tree, type_tree_hash, object_type.to_string());
        root_items.retain(|item| item.name != object_type);
        root_items.push(new_root_item);
        root_items.sort_by(|a, b| a.name.cmp(&b.name));

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
        let parents = parent_commit_id.into_iter().collect::<Vec<_>>();
        let commit = Commit::new(author, signature, root_tree_hash, parents, &message);
        let commit_data = commit
            .to_data()
            .context("Failed to serialize AI history commit")?;
        write_git_object(&self.repo_path, "commit", &commit_data)
            .context("Failed to write AI history commit")
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

    async fn update_ref_if_matches(
        &self,
        ref_name: &str,
        expected_head: Option<ObjectHash>,
        new_hash: ObjectHash,
    ) -> Result<RefUpdateOutcome> {
        let expected_commit = expected_head.map(|hash| hash.to_string());
        let new_commit = new_hash.to_string();

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

            let write_result = match existing {
                Some(model) if model.commit != expected_commit => {
                    let _ = txn.rollback().await;
                    return Ok(RefUpdateOutcome::HeadChanged);
                }
                Some(model) => {
                    let mut update = reference::Entity::update_many()
                        .filter(reference::Column::Id.eq(model.id))
                        .filter(reference::Column::Name.eq(ref_name))
                        .filter(reference::Column::Kind.eq(ConfigKind::Branch));
                    update = match expected_commit.as_ref() {
                        Some(commit) => update.filter(reference::Column::Commit.eq(commit.clone())),
                        None => update.filter(reference::Column::Commit.is_null()),
                    };

                    update
                        .col_expr(
                            reference::Column::Commit,
                            Expr::value(Some(new_commit.clone())),
                        )
                        .exec(&txn)
                        .await
                        .map(Some)
                }
                None if expected_commit.is_some() => {
                    let _ = txn.rollback().await;
                    return Ok(RefUpdateOutcome::HeadChanged);
                }
                None => {
                    let new_ref = reference::ActiveModel {
                        name: Set(Some(ref_name.to_string())),
                        kind: Set(ConfigKind::Branch),
                        commit: Set(Some(new_commit.clone())),
                        remote: Set(None),
                        ..Default::default()
                    };
                    match new_ref.insert(&txn).await {
                        Ok(_) => Ok(None),
                        Err(err) if is_sqlite_unique_violation(&err) => {
                            let _ = txn.rollback().await;
                            return Ok(RefUpdateOutcome::HeadChanged);
                        }
                        Err(err) => Err(err),
                    }
                }
            };

            let rows_affected = match write_result {
                Ok(rows_affected) => rows_affected,
                Err(err) if is_sqlite_busy(&err) && attempt < SQLITE_BUSY_MAX_RETRIES => {
                    let _ = txn.rollback().await;
                    sleep(Duration::from_millis(
                        SQLITE_BUSY_RETRY_BASE_MS * (attempt as u64 + 1),
                    ))
                    .await;
                    continue;
                }
                Err(err) => return Err(err).context("Failed to compare-and-swap history head"),
            };

            if rows_affected.is_some_and(|result| result.rows_affected != 1) {
                let _ = txn.rollback().await;
                return Ok(RefUpdateOutcome::HeadChanged);
            }

            match txn.commit().await {
                Ok(()) => return Ok(RefUpdateOutcome::Updated),
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

    /// Append a checkpoint commit to this manager's ref.
    ///
    /// CEX-EntireIO Phase 2.1. Builds the layered tree
    /// `checkpoint/<id[:2]>/<id[2:]>/{metadata.json, transcript/<provider>,
    /// events/<provider>.jsonl}` and merges it into the parent commit's tree
    /// so successive checkpoints accumulate (rather than overwrite). The
    /// resulting commit message carries `Libra-*` trailers per the design
    /// spec (see `docs/development/commands/_general.md` §3.3).
    ///
    /// Returns the freshly-written commit hash plus the OIDs callers need to
    /// stamp onto `agent_checkpoint` (root tree OID and metadata blob OID).
    pub async fn append_checkpoint_commit(
        &self,
        params: CheckpointCommitParams<'_>,
    ) -> Result<CheckpointCommit> {
        // Phase 1: write content blobs once. They are content-addressed, so
        // re-running a CAS retry loop never duplicates them.
        let metadata_blob_oid = write_git_object(&self.repo_path, "blob", params.metadata_json)
            .context("failed to write checkpoint metadata.json blob")?;
        let transcript_blob_oid =
            write_git_object(&self.repo_path, "blob", params.transcript_redacted.bytes())
                .context("failed to write checkpoint transcript blob")?;
        let events_blob_oid = match params.events_jsonl {
            Some(bytes) => Some(
                write_git_object(&self.repo_path, "blob", bytes)
                    .context("failed to write checkpoint events.jsonl blob")?,
            ),
            None => None,
        };

        // CEX-EntireIO §14.3 phase-3 item 3: tag the just-written agent
        // blobs in `object_index` so `libra cloud sync` uploads them to
        // R2. Before this hook the orphan `refs/libra/agent-traces`
        // history was "Git-side present, cloud-side invisible" — D1
        // carried the catalogue (`agent_session` / `agent_checkpoint`)
        // but R2 never saw the actual blob bytes, so a fresh
        // `cloud restore` resolved commit OIDs against missing blobs.
        //
        // Only the transcript blob carries a distinguished o_type
        // ("agent_transcript") per the spec line "object_index
        // .o_type='agent_transcript' 走正常 R2 同步". The metadata and
        // events JSON blobs use the standard "blob" tag because cloud
        // sync doesn't filter by o_type — the sole motivation for a
        // custom tag is downstream tooling that wants to enumerate
        // captured transcripts.
        crate::utils::client_storage::enqueue_agent_blob_object_index_update(
            &self.repo_path,
            &metadata_blob_oid.to_string(),
            "blob",
            params.metadata_json.len() as i64,
        );
        crate::utils::client_storage::enqueue_agent_blob_object_index_update(
            &self.repo_path,
            &transcript_blob_oid.to_string(),
            "agent_transcript",
            params.transcript_redacted.len() as i64,
        );
        if let (Some(oid), Some(bytes)) = (events_blob_oid.as_ref(), params.events_jsonl) {
            crate::utils::client_storage::enqueue_agent_blob_object_index_update(
                &self.repo_path,
                &oid.to_string(),
                "blob",
                bytes.len() as i64,
            );
        }

        // Phase 2: build the leaf trees (transcript/, optional events/).
        // All trees written under the agent capture path go through
        // `write_tree_indexed` so they reach `object_index` and the
        // standard cloud sync path; otherwise the orphan ref's commits
        // would dereference to missing trees on a fresh `cloud restore`.
        let transcript_subtree = self.write_tree_indexed(
            &[TreeItem::new(
                TreeItemMode::Blob,
                transcript_blob_oid,
                params.provider_name.to_string(),
            )],
            "tree",
        )?;

        let mut inner_items = vec![
            TreeItem::new(
                TreeItemMode::Blob,
                metadata_blob_oid,
                "metadata.json".to_string(),
            ),
            TreeItem::new(
                TreeItemMode::Tree,
                transcript_subtree,
                "transcript".to_string(),
            ),
        ];
        if let Some(events_oid) = events_blob_oid {
            let events_subtree = self.write_tree_indexed(
                &[TreeItem::new(
                    TreeItemMode::Blob,
                    events_oid,
                    format!("{}.jsonl", params.provider_name),
                )],
                "tree",
            )?;
            inner_items.push(TreeItem::new(
                TreeItemMode::Tree,
                events_subtree,
                "events".to_string(),
            ));
        }
        inner_items.sort_by(|a, b| a.name.cmp(&b.name));
        let inner_tree = self.write_tree_indexed(&inner_items, "tree")?;

        // Phase 3: CAS loop. Read parent, splice
        // `checkpoint/<prefix>/<rest>` into its tree, write the new commit,
        // and update the ref atomically. Retries on head conflict, mirroring
        // the existing `append` flow.
        let prefix = params
            .checkpoint_id
            .get(..2)
            .ok_or_else(|| anyhow!("checkpoint_id must be at least 2 characters"))?
            .to_string();
        let rest = params.checkpoint_id[2..].to_string();
        for attempt in 0..=HISTORY_HEAD_CONFLICT_MAX_RETRIES {
            let parent = self.resolve_history_head().await?;
            let new_root = self.splice_checkpoint_tree(parent, &prefix, &rest, inner_tree)?;

            let trailer = format_libra_trailers(&params);
            let message = format!(
                "agent-traces: {} checkpoint {}\n\n{trailer}",
                params.scope.as_str(),
                params.checkpoint_id,
            );
            let author = Signature::new(
                SignatureType::Author,
                "Libra".to_string(),
                "agent-traces@libra".to_string(),
            );
            let committer = Signature::new(
                SignatureType::Committer,
                "Libra".to_string(),
                "agent-traces@libra".to_string(),
            );
            let parents = parent.into_iter().collect::<Vec<_>>();
            let commit = Commit::new(author, committer, new_root, parents, &message);
            let commit_data = commit
                .to_data()
                .context("failed to serialize checkpoint commit")?;
            let commit_hash = write_git_object(&self.repo_path, "commit", &commit_data)?;
            // Phase 3.5c: agent capture commits must reach R2 too.
            // Tagging at every CAS retry is idempotent because
            // `update_object_index_once` does an existence check before
            // inserting, and the same `commit_data` produces the same
            // OID across retries.
            crate::utils::client_storage::enqueue_agent_blob_object_index_update(
                &self.repo_path,
                &commit_hash.to_string(),
                "commit",
                commit_data.len() as i64,
            );

            match self
                .update_ref_if_matches(&self.ref_name, parent, commit_hash)
                .await?
            {
                RefUpdateOutcome::Updated => {
                    return Ok(CheckpointCommit {
                        commit_hash,
                        tree_oid: new_root,
                        metadata_blob_oid,
                    });
                }
                RefUpdateOutcome::HeadChanged if attempt < HISTORY_HEAD_CONFLICT_MAX_RETRIES => {
                    continue;
                }
                RefUpdateOutcome::HeadChanged => {
                    return Err(anyhow!(
                        "history head changed repeatedly while appending checkpoint {}",
                        params.checkpoint_id
                    ));
                }
            }
        }
        unreachable!("CAS retry loop must return on success or terminal error")
    }

    /// Remove checkpoint commits from this manager's ref and delete their
    /// `agent_checkpoint` rows.
    ///
    /// This is the `libra agent clean` counterpart to
    /// [`Self::append_checkpoint_commit`]. It rewrites the orphan
    /// `refs/libra/agent-traces` chain from the checkpoint catalog, omitting
    /// the supplied checkpoint IDs. Rewriting is necessary because later
    /// committed checkpoints may descend from temporary checkpoints; simply
    /// moving the ref to an ancestor would either keep those temporary commits
    /// reachable or discard later retained checkpoints.
    ///
    /// Repositories that only have catalog rows and an empty agent-traces ref
    /// (older fixtures, partial migrations, or pre-Phase-2 data) still get the
    /// catalog deletion without a ref rewrite.
    pub async fn prune_checkpoint_commits(
        &self,
        checkpoint_ids_to_remove: &[String],
    ) -> Result<CheckpointPruneOutcome> {
        let remove_set: HashSet<&str> = checkpoint_ids_to_remove
            .iter()
            .map(String::as_str)
            .collect();
        if remove_set.is_empty() {
            return Ok(CheckpointPruneOutcome {
                removed_checkpoints: 0,
                rewritten_checkpoints: 0,
                ref_rewritten: false,
            });
        }

        for attempt in 0..=HISTORY_HEAD_CONFLICT_MAX_RETRIES {
            let expected_head = self.resolve_history_head().await?;
            let rows = self.load_checkpoint_history_rows().await?;
            let existing_remove_ids = rows
                .iter()
                .filter(|row| remove_set.contains(row.checkpoint_id.as_str()))
                .map(|row| row.checkpoint_id.clone())
                .collect::<HashSet<_>>();

            if existing_remove_ids.is_empty() {
                return Ok(CheckpointPruneOutcome {
                    removed_checkpoints: 0,
                    rewritten_checkpoints: 0,
                    ref_rewritten: false,
                });
            }

            let retained_rows = rows
                .into_iter()
                .filter(|row| !existing_remove_ids.contains(&row.checkpoint_id))
                .collect::<Vec<_>>();

            let (new_head, rewritten) = match expected_head {
                Some(head) => self.rebuild_checkpoint_history(head, &retained_rows)?,
                None => (None, Vec::new()),
            };

            match self
                .commit_checkpoint_prune(expected_head, new_head, &rewritten, &existing_remove_ids)
                .await?
            {
                (RefUpdateOutcome::Updated, removed_checkpoints) => {
                    return Ok(CheckpointPruneOutcome {
                        removed_checkpoints,
                        rewritten_checkpoints: rewritten.len(),
                        ref_rewritten: expected_head != new_head,
                    });
                }
                (RefUpdateOutcome::HeadChanged, _)
                    if attempt < HISTORY_HEAD_CONFLICT_MAX_RETRIES =>
                {
                    continue;
                }
                (RefUpdateOutcome::HeadChanged, _) => {
                    return Err(anyhow!(
                        "agent-traces head changed repeatedly while pruning checkpoints"
                    ));
                }
            }
        }

        unreachable!("checkpoint prune retry loop must return on success or terminal error")
    }

    async fn load_checkpoint_history_rows(&self) -> Result<Vec<CheckpointHistoryRow>> {
        let backend = self.db_conn.get_database_backend();
        let rows = self
            .db_conn
            .query_all(Statement::from_string(
                backend,
                "SELECT cp.checkpoint_id, cp.session_id, cp.scope, cp.parent_commit, \
                        cp.traces_commit, cp.created_at, COALESCE(s.agent_kind, 'unknown') AS agent_kind \
                 FROM agent_checkpoint cp \
                 LEFT JOIN agent_session s ON s.session_id = cp.session_id \
                 ORDER BY cp.created_at ASC, cp.checkpoint_id ASC"
                    .to_string(),
            ))
            .await
            .context("failed to load agent_checkpoint rows for agent-traces rewrite")?;

        rows.into_iter()
            .map(CheckpointHistoryRow::from_query_result)
            .collect()
    }

    fn rebuild_checkpoint_history(
        &self,
        current_head: ObjectHash,
        retained_rows: &[CheckpointHistoryRow],
    ) -> Result<(Option<ObjectHash>, Vec<RewrittenCheckpoint>)> {
        if retained_rows.is_empty() {
            return Ok((None, Vec::new()));
        }

        let current_root = self.load_commit_tree(&current_head)?;
        let mut parent = None;
        let mut rewritten = Vec::with_capacity(retained_rows.len());

        for row in retained_rows {
            let inner_tree = self
                .checkpoint_inner_tree_from_root(&current_root, &row.checkpoint_id)?
                .ok_or_else(|| {
                    anyhow!(
                        "agent-traces tree is missing retained checkpoint {}",
                        row.checkpoint_id
                    )
                })?;
            let (prefix, rest) = checkpoint_tree_path(&row.checkpoint_id)?;
            let root_tree = self.splice_checkpoint_tree(parent, &prefix, &rest, inner_tree)?;
            let commit_hash = self.write_rewritten_checkpoint_commit(parent, root_tree, row)?;
            rewritten.push(RewrittenCheckpoint {
                checkpoint_id: row.checkpoint_id.clone(),
                traces_commit: commit_hash,
                tree_oid: root_tree,
            });
            parent = Some(commit_hash);
        }

        Ok((parent, rewritten))
    }

    fn checkpoint_inner_tree_from_root(
        &self,
        root_items: &[TreeItem],
        checkpoint_id: &str,
    ) -> Result<Option<ObjectHash>> {
        let (prefix, rest) = checkpoint_tree_path(checkpoint_id)?;
        let Some(checkpoint_entry) = root_items.iter().find(|item| item.name == "checkpoint")
        else {
            return Ok(None);
        };
        if checkpoint_entry.mode != TreeItemMode::Tree {
            return Err(anyhow!(
                "agent-traces tree corruption: 'checkpoint' entry expected to be a tree, got mode {:?}",
                checkpoint_entry.mode
            ));
        }

        let checkpoint_items = self.load_tree(&checkpoint_entry.id)?;
        let Some(prefix_entry) = checkpoint_items.iter().find(|item| item.name == prefix) else {
            return Ok(None);
        };
        if prefix_entry.mode != TreeItemMode::Tree {
            return Err(anyhow!(
                "agent-traces tree corruption: 'checkpoint/{prefix}' entry expected to be a tree, got mode {:?}",
                prefix_entry.mode
            ));
        }

        let prefix_items = self.load_tree(&prefix_entry.id)?;
        let Some(rest_entry) = prefix_items.iter().find(|item| item.name == rest) else {
            return Ok(None);
        };
        if rest_entry.mode != TreeItemMode::Tree {
            return Err(anyhow!(
                "agent-traces tree corruption: 'checkpoint/{prefix}/{rest}' entry expected to be a tree, got mode {:?}",
                rest_entry.mode
            ));
        }
        Ok(Some(rest_entry.id))
    }

    fn write_rewritten_checkpoint_commit(
        &self,
        parent: Option<ObjectHash>,
        root_tree: ObjectHash,
        row: &CheckpointHistoryRow,
    ) -> Result<ObjectHash> {
        let message = format!(
            "agent-traces: {} checkpoint {}\n\n{}",
            row.scope,
            row.checkpoint_id,
            format_rewritten_checkpoint_trailers(row)
        );
        let author = Signature::new(
            SignatureType::Author,
            "Libra".to_string(),
            "agent-traces@libra".to_string(),
        );
        let committer = Signature::new(
            SignatureType::Committer,
            "Libra".to_string(),
            "agent-traces@libra".to_string(),
        );
        let parents = parent.into_iter().collect::<Vec<_>>();
        let commit = Commit::new(author, committer, root_tree, parents, &message);
        let commit_data = commit
            .to_data()
            .context("failed to serialize rewritten checkpoint commit")?;
        let commit_hash = write_git_object(&self.repo_path, "commit", &commit_data)?;
        crate::utils::client_storage::enqueue_agent_blob_object_index_update(
            &self.repo_path,
            &commit_hash.to_string(),
            "commit",
            commit_data.len() as i64,
        );
        Ok(commit_hash)
    }

    async fn commit_checkpoint_prune(
        &self,
        expected_head: Option<ObjectHash>,
        new_head: Option<ObjectHash>,
        rewritten: &[RewrittenCheckpoint],
        remove_ids: &HashSet<String>,
    ) -> Result<(RefUpdateOutcome, u64)> {
        let expected_commit = expected_head.map(|hash| hash.to_string());
        let new_commit = new_head.map(|hash| hash.to_string());

        'retry_sqlite: for attempt in 0..=SQLITE_BUSY_MAX_RETRIES {
            let txn: DatabaseTransaction = match self.db_conn.begin().await {
                Ok(txn) => txn,
                Err(err) if is_sqlite_busy(&err) && attempt < SQLITE_BUSY_MAX_RETRIES => {
                    sleep(Duration::from_millis(
                        SQLITE_BUSY_RETRY_BASE_MS * (attempt as u64 + 1),
                    ))
                    .await;
                    continue;
                }
                Err(err) => {
                    return Err(err).context("Failed to begin checkpoint prune transaction");
                }
            };

            let existing = match reference::Entity::find()
                .filter(reference::Column::Name.eq(&self.ref_name))
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
                Err(err) => return Err(err).context("Failed to query checkpoint prune ref"),
            };

            let write_ref = match existing {
                Some(model) if model.commit != expected_commit => {
                    let _ = txn.rollback().await;
                    return Ok((RefUpdateOutcome::HeadChanged, 0));
                }
                Some(model) => {
                    let mut active: reference::ActiveModel = model.into();
                    active.commit = Set(new_commit.clone());
                    active.update(&txn).await.map(|_| ())
                }
                None if expected_commit.is_some() => {
                    let _ = txn.rollback().await;
                    return Ok((RefUpdateOutcome::HeadChanged, 0));
                }
                None => {
                    let new_ref = reference::ActiveModel {
                        name: Set(Some(self.ref_name.clone())),
                        kind: Set(ConfigKind::Branch),
                        commit: Set(new_commit.clone()),
                        remote: Set(None),
                        ..Default::default()
                    };
                    new_ref.insert(&txn).await.map(|_| ())
                }
            };

            if let Err(err) = write_ref {
                if is_sqlite_busy(&err) && attempt < SQLITE_BUSY_MAX_RETRIES {
                    let _ = txn.rollback().await;
                    sleep(Duration::from_millis(
                        SQLITE_BUSY_RETRY_BASE_MS * (attempt as u64 + 1),
                    ))
                    .await;
                    continue;
                }
                return Err(err).context("Failed to update checkpoint prune ref");
            }

            let backend = txn.get_database_backend();
            for item in rewritten {
                if let Err(err) = txn
                    .execute(Statement::from_sql_and_values(
                        backend,
                        "UPDATE agent_checkpoint SET traces_commit = ?, tree_oid = ? \
                         WHERE checkpoint_id = ?",
                        vec![
                            Value::from(item.traces_commit.to_string()),
                            Value::from(item.tree_oid.to_string()),
                            Value::from(item.checkpoint_id.clone()),
                        ],
                    ))
                    .await
                {
                    if is_sqlite_busy(&err) && attempt < SQLITE_BUSY_MAX_RETRIES {
                        let _ = txn.rollback().await;
                        sleep(Duration::from_millis(
                            SQLITE_BUSY_RETRY_BASE_MS * (attempt as u64 + 1),
                        ))
                        .await;
                        continue 'retry_sqlite;
                    }
                    return Err(err).context("Failed to update rewritten checkpoint row");
                }
            }

            let mut removed = 0;
            for id in remove_ids {
                match txn
                    .execute(Statement::from_sql_and_values(
                        backend,
                        "DELETE FROM agent_checkpoint WHERE checkpoint_id = ?",
                        [Value::from(id.clone())],
                    ))
                    .await
                {
                    Ok(result) => removed += result.rows_affected(),
                    Err(err) if is_sqlite_busy(&err) && attempt < SQLITE_BUSY_MAX_RETRIES => {
                        let _ = txn.rollback().await;
                        sleep(Duration::from_millis(
                            SQLITE_BUSY_RETRY_BASE_MS * (attempt as u64 + 1),
                        ))
                        .await;
                        continue 'retry_sqlite;
                    }
                    Err(err) => return Err(err).context("Failed to delete pruned checkpoint row"),
                }
            }

            match txn.commit().await {
                Ok(()) => return Ok((RefUpdateOutcome::Updated, removed)),
                Err(err) if is_sqlite_busy(&err) && attempt < SQLITE_BUSY_MAX_RETRIES => {
                    sleep(Duration::from_millis(
                        SQLITE_BUSY_RETRY_BASE_MS * (attempt as u64 + 1),
                    ))
                    .await;
                }
                Err(err) => {
                    return Err(err).context("Failed to commit checkpoint prune transaction");
                }
            }
        }

        unreachable!("sqlite busy retry loop must return on success or terminal error")
    }

    /// Splice `inner_tree` into `parent`'s tree at the path
    /// `checkpoint/<prefix>/<rest>`, preserving any existing entries in the
    /// surrounding subtrees. Phase 2.1 helper for [`append_checkpoint_commit`].
    fn splice_checkpoint_tree(
        &self,
        parent: Option<ObjectHash>,
        prefix: &str,
        rest: &str,
        inner_tree: ObjectHash,
    ) -> Result<ObjectHash> {
        let mut root_items = match parent {
            Some(parent_id) => self.load_commit_tree(&parent_id)?,
            None => Vec::new(),
        };
        let checkpoint_entry = root_items
            .iter()
            .find(|item| item.name == "checkpoint")
            .cloned();
        let mut checkpoint_items = match checkpoint_entry {
            Some(entry) if entry.mode == TreeItemMode::Tree => self.load_tree(&entry.id)?,
            Some(entry) => {
                return Err(anyhow!(
                    "agent-traces tree corruption: 'checkpoint' entry expected to be a tree, \
                     got mode {:?} (oid {})",
                    entry.mode,
                    entry.id
                ));
            }
            None => Vec::new(),
        };

        let prefix_entry = checkpoint_items
            .iter()
            .find(|item| item.name == prefix)
            .cloned();
        let mut prefix_items = match prefix_entry {
            Some(entry) if entry.mode == TreeItemMode::Tree => self.load_tree(&entry.id)?,
            Some(entry) => {
                return Err(anyhow!(
                    "agent-traces tree corruption: 'checkpoint/{prefix}' entry expected to be a \
                     tree, got mode {:?} (oid {})",
                    entry.mode,
                    entry.id
                ));
            }
            None => Vec::new(),
        };

        prefix_items.retain(|item| item.name != rest);
        prefix_items.push(TreeItem::new(
            TreeItemMode::Tree,
            inner_tree,
            rest.to_string(),
        ));
        prefix_items.sort_by(|a, b| a.name.cmp(&b.name));
        // Phase 3.5c: tag every tree spliced into the agent capture
        // history so cloud sync uploads the full reachability set.
        let prefix_tree = self.write_tree_indexed(&prefix_items, "tree")?;

        checkpoint_items.retain(|item| item.name != prefix);
        checkpoint_items.push(TreeItem::new(
            TreeItemMode::Tree,
            prefix_tree,
            prefix.to_string(),
        ));
        checkpoint_items.sort_by(|a, b| a.name.cmp(&b.name));
        let checkpoint_tree = self.write_tree_indexed(&checkpoint_items, "tree")?;

        root_items.retain(|item| item.name != "checkpoint");
        root_items.push(TreeItem::new(
            TreeItemMode::Tree,
            checkpoint_tree,
            "checkpoint".to_string(),
        ));
        root_items.sort_by(|a, b| a.name.cmp(&b.name));
        self.write_tree_indexed(&root_items, "tree")
    }

    #[cfg(test)]
    pub fn get_storage(&self) -> Arc<dyn Storage + Send + Sync> {
        self.storage.clone()
    }
}

/// Inputs to [`HistoryManager::append_checkpoint_commit`].
///
/// All byte slices live for the duration of the call; the function does not
/// retain references after returning.
#[derive(Debug)]
pub struct CheckpointCommitParams<'a> {
    /// UUIDv4 of the checkpoint, used both as the row primary key and as
    /// the leaf path under `checkpoint/<id[:2]>/<id[2:]>/...`.
    pub checkpoint_id: &'a str,
    /// `agent_session.session_id` this checkpoint belongs to.
    pub session_id: &'a str,
    /// `agent_session.agent_kind` (snake_case form, e.g. `claude_code`).
    pub agent_kind: &'a str,
    /// User-branch HEAD oid at the moment the checkpoint was taken.
    pub parent_commit: Option<&'a str>,
    /// Scope category: temporary, committed, or subagent.
    pub scope: CheckpointScope,
    /// Optional tool-use id when the checkpoint was triggered by a tool call.
    pub tool_use_id: Option<&'a str>,
    /// Pre-serialised metadata JSON to land at `metadata.json`.
    pub metadata_json: &'a [u8],
    /// Already-redacted transcript bytes. Typed as [`RedactedBytes`]
    /// (not `&[u8]`) so the agent-traces write path can only ever receive
    /// bytes that passed through the redaction type — entire.md §8.1 /
    /// §13 P0: every transcript blob written to `agent-traces` must go
    /// through `RedactedBytes`.
    pub transcript_redacted: &'a RedactedBytes,
    /// File-name component used inside `transcript/<name>` and
    /// `events/<name>.jsonl`. Conventionally the agent's slug
    /// (`claude_code`, `gemini`, …).
    pub provider_name: &'a str,
    /// Optional events JSONL stream (`SessionStore`-backed).
    pub events_jsonl: Option<&'a [u8]>,
}

/// Scope tag stamped on each checkpoint, mirroring the
/// `agent_checkpoint.scope` CHECK constraint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointScope {
    Temporary,
    Committed,
    Subagent,
}

impl CheckpointScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Temporary => "temporary",
            Self::Committed => "committed",
            Self::Subagent => "subagent",
        }
    }
}

/// Output from [`HistoryManager::append_checkpoint_commit`]; what the caller
/// stores in `agent_checkpoint`.
#[derive(Debug, Clone)]
pub struct CheckpointCommit {
    pub commit_hash: ObjectHash,
    pub tree_oid: ObjectHash,
    pub metadata_blob_oid: ObjectHash,
}

/// Result of pruning checkpoint commits from `refs/libra/agent-traces`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointPruneOutcome {
    pub removed_checkpoints: u64,
    pub rewritten_checkpoints: usize,
    pub ref_rewritten: bool,
}

#[derive(Debug, Clone)]
struct CheckpointHistoryRow {
    checkpoint_id: String,
    session_id: String,
    agent_kind: String,
    scope: String,
    parent_commit: Option<String>,
}

impl CheckpointHistoryRow {
    fn from_query_result(row: QueryResult) -> Result<Self> {
        Ok(Self {
            checkpoint_id: row
                .try_get_by("checkpoint_id")
                .context("decode agent_checkpoint.checkpoint_id")?,
            session_id: row
                .try_get_by("session_id")
                .context("decode agent_checkpoint.session_id")?,
            agent_kind: row
                .try_get_by("agent_kind")
                .context("decode agent_session.agent_kind")?,
            scope: row
                .try_get_by("scope")
                .context("decode agent_checkpoint.scope")?,
            parent_commit: row.try_get_by("parent_commit").ok().flatten(),
        })
    }
}

#[derive(Debug, Clone)]
struct RewrittenCheckpoint {
    checkpoint_id: String,
    traces_commit: ObjectHash,
    tree_oid: ObjectHash,
}

fn checkpoint_tree_path(checkpoint_id: &str) -> Result<(String, String)> {
    let prefix = checkpoint_id
        .get(..2)
        .ok_or_else(|| anyhow!("checkpoint_id must be at least 2 characters"))?
        .to_string();
    let rest = checkpoint_id
        .get(2..)
        .ok_or_else(|| anyhow!("checkpoint_id must be valid UTF-8 at byte 2"))?
        .to_string();
    Ok((prefix, rest))
}

fn format_libra_trailers(params: &CheckpointCommitParams<'_>) -> String {
    let mut buf = String::new();
    buf.push_str(&format!("Libra-Session: {}\n", params.session_id));
    buf.push_str(&format!("Libra-Agent: {}\n", params.agent_kind));
    if let Some(commit) = params.parent_commit {
        buf.push_str(&format!("Libra-Parent-Commit: {commit}\n"));
    }
    buf.push_str(&format!("Libra-Checkpoint-ID: {}\n", params.checkpoint_id));
    buf.push_str(&format!("Libra-Scope: {}\n", params.scope.as_str()));
    if let Some(tool) = params.tool_use_id {
        buf.push_str(&format!("Libra-Tool-Use-ID: {tool}\n"));
    }
    buf
}

fn format_rewritten_checkpoint_trailers(row: &CheckpointHistoryRow) -> String {
    let mut buf = String::new();
    buf.push_str(&format!("Libra-Session: {}\n", row.session_id));
    buf.push_str(&format!("Libra-Agent: {}\n", row.agent_kind));
    if let Some(commit) = &row.parent_commit {
        buf.push_str(&format!("Libra-Parent-Commit: {commit}\n"));
    }
    buf.push_str(&format!("Libra-Checkpoint-ID: {}\n", row.checkpoint_id));
    buf.push_str(&format!("Libra-Scope: {}\n", row.scope));
    buf
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

    #[tokio::test]
    async fn test_update_ref_if_matches_rejects_stale_history_head() {
        let dir = tempdir().unwrap();
        let repo_path = dir.path().join(".libra");
        std::fs::create_dir(&repo_path).unwrap();
        let objects_dir = repo_path.join("objects");

        let storage = Arc::new(LocalStorage::new(objects_dir));
        let db_conn = Arc::new(setup_test_db().await);
        let manager = HistoryManager::new(storage, repo_path, db_conn);

        let task_hash = ObjectHash::from_str("e69de29bb2d1d6434b8b29ae775ad8c2e48c5391").unwrap();
        let plan_hash = ObjectHash::from_str("f4e6d0434b8b29ae775ad8c2e48c5391e69de29b").unwrap();
        let frame_hash = ObjectHash::from_str("a4e6d0434b8b29ae775ad8c2e48c5391e69de29b").unwrap();

        manager.append("task", "task-1", task_hash).await.unwrap();
        let stale_head = manager.resolve_history_head().await.unwrap();
        let stale_commit = manager
            .create_append_commit(stale_head, "plan", "plan-1", plan_hash)
            .expect("stale append commit should be created");

        manager
            .append("context_frame", "frame-1", frame_hash)
            .await
            .unwrap();

        let outcome = manager
            .update_ref_if_matches(AI_REF, stale_head, stale_commit)
            .await
            .expect("stale ref update should not error");
        assert_eq!(outcome, RefUpdateOutcome::HeadChanged);

        manager.append("plan", "plan-1", plan_hash).await.unwrap();

        assert!(
            manager
                .get_object_hash("context_frame", "frame-1")
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            manager
                .get_object_hash("plan", "plan-1")
                .await
                .unwrap()
                .is_some()
        );
    }
}
