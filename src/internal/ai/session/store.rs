//! Session storage: save and load sessions from disk.

use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use super::state::SessionState;

const SESSION_LOCK_TIMEOUT: Duration = Duration::from_secs(5);
const SESSION_LOCK_POLL_INTERVAL: Duration = Duration::from_millis(10);
const STALE_SESSION_LOCK_AGE: Duration = Duration::from_secs(30);
const THREAD_ID_METADATA_KEYS: &[&str] = &["thread_id", "threadId", "canonical_thread_id"];

/// Manages session persistence on disk.
///
/// Sessions are stored as JSON files in a sessions directory.
pub struct SessionStore {
    sessions_dir: PathBuf,
}

#[derive(Debug)]
pub struct SessionFileLock {
    lock_path: PathBuf,
}

impl Drop for SessionFileLock {
    fn drop(&mut self) {
        if let Err(err) = fs::remove_file(&self.lock_path)
            && err.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!(
                path = %self.lock_path.display(),
                error = %err,
                "failed to release session lock"
            );
        }
    }
}

impl SessionStore {
    /// Create a store rooted at `{working_dir}/.libra/sessions/`.
    pub fn new(working_dir: &Path) -> Self {
        Self {
            sessions_dir: working_dir.join(".libra").join("sessions"),
        }
    }

    /// Create a store rooted at `{storage_path}/sessions/`.
    pub fn from_storage_path(storage_path: &Path) -> Self {
        Self {
            sessions_dir: storage_path.join("sessions"),
        }
    }

    /// Create the sessions directory if it doesn't exist.
    fn ensure_dir(&self) -> io::Result<()> {
        fs::create_dir_all(&self.sessions_dir)
    }

    /// Save a session to disk.
    pub fn save(&self, session: &SessionState) -> io::Result<()> {
        self.ensure_dir()?;
        let path = self.session_path(&session.id);
        let json = serde_json::to_string_pretty(session)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        self.write_atomic(&path, json.as_bytes())
    }

    /// Load a session by ID.
    pub fn load(&self, id: &str) -> io::Result<SessionState> {
        let path = self.session_path(id);
        let content = fs::read_to_string(path)?;
        serde_json::from_str(&content).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    /// Load the most recently updated session.
    pub fn load_latest(&self) -> io::Result<Option<SessionState>> {
        let sessions = self.list()?;
        if sessions.is_empty() {
            return Ok(None);
        }

        // Find the most recently modified session file
        let mut latest: Option<(SessionState, SystemTime)> = None;

        for info in sessions {
            match self.load(&info.id) {
                Ok(session) => {
                    let path = self.session_path(&info.id);
                    let modified = fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .unwrap_or(SystemTime::UNIX_EPOCH);

                    if latest
                        .as_ref()
                        .is_none_or(|(_, best_time)| modified > *best_time)
                    {
                        latest = Some((session, modified));
                    }
                }
                Err(e) => {
                    tracing::warn!(session_id = %info.id, error = %e, "skipping corrupt session file");
                }
            }
        }

        Ok(latest.map(|(session, _)| session))
    }

    /// Load the most recently updated session for a specific working directory.
    pub fn load_latest_for_working_dir(
        &self,
        working_dir: &str,
    ) -> io::Result<Option<SessionState>> {
        let sessions = self.list()?;
        if sessions.is_empty() {
            return Ok(None);
        }

        let mut latest: Option<(SessionState, SystemTime)> = None;

        for info in sessions {
            match self.load(&info.id) {
                Ok(session) if session.working_dir == working_dir => {
                    let path = self.session_path(&info.id);
                    let modified = fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .unwrap_or(SystemTime::UNIX_EPOCH);

                    if latest
                        .as_ref()
                        .is_none_or(|(_, best_time)| modified > *best_time)
                    {
                        latest = Some((session, modified));
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(session_id = %info.id, error = %e, "skipping corrupt session file");
                }
            }
        }

        Ok(latest.map(|(session, _)| session))
    }

    /// Load the most recently updated session for a canonical Libra thread_id.
    pub fn load_for_thread_id(
        &self,
        thread_id: &str,
        working_dir: &str,
    ) -> io::Result<Option<SessionState>> {
        let sessions = self.list()?;
        if sessions.is_empty() {
            return Ok(None);
        }

        let mut latest: Option<(SessionState, SystemTime)> = None;
        for info in sessions {
            match self.load(&info.id) {
                Ok(session)
                    if session.working_dir == working_dir
                        && session_matches_thread_id(&session, thread_id) =>
                {
                    let path = self.session_path(&info.id);
                    let modified = fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .unwrap_or(SystemTime::UNIX_EPOCH);

                    if latest
                        .as_ref()
                        .is_none_or(|(_, best_time)| modified > *best_time)
                    {
                        latest = Some((session, modified));
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(session_id = %info.id, error = %e, "skipping corrupt session file");
                }
            }
        }

        Ok(latest.map(|(session, _)| session))
    }

    /// List all saved sessions (basic info only).
    pub fn list(&self) -> io::Result<Vec<SessionInfo>> {
        if !self.sessions_dir.exists() {
            return Ok(Vec::new());
        }

        let mut sessions = Vec::new();
        for entry in fs::read_dir(&self.sessions_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "json") {
                match fs::read_to_string(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|content| {
                        serde_json::from_str::<SessionState>(&content).map_err(|e| e.to_string())
                    }) {
                    Ok(session) => {
                        sessions.push(SessionInfo {
                            id: session.id,
                            created_at: session.created_at.to_string(),
                            summary: session.summary,
                            message_count: session.messages.len(),
                        });
                    }
                    Err(e) => {
                        tracing::warn!(
                            path = %path.display(),
                            error = %e,
                            "skipping malformed session file"
                        );
                    }
                }
            }
        }

        Ok(sessions)
    }

    /// Delete a session by ID.
    pub fn delete(&self, id: &str) -> io::Result<()> {
        let path = self.session_path(id);
        fs::remove_file(path)
    }

    /// Acquire an exclusive lock for one session ID.
    ///
    /// The lock is implemented via a lock file in the sessions directory and
    /// is automatically released when the returned guard is dropped.
    pub fn lock_session(&self, id: &str) -> io::Result<SessionFileLock> {
        self.ensure_dir()?;
        let lock_path = self.session_lock_path(id);
        let start = Instant::now();

        loop {
            match OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&lock_path)
            {
                Ok(mut file) => {
                    let lock_content = format!(
                        "pid={}\ncreated_at_ns={}\n",
                        std::process::id(),
                        SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_nanos()
                    );
                    file.write_all(lock_content.as_bytes())?;
                    return Ok(SessionFileLock { lock_path });
                }
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                    if self.is_stale_lock(&lock_path) {
                        match fs::remove_file(&lock_path) {
                            Ok(()) => continue,
                            Err(remove_err) if remove_err.kind() == io::ErrorKind::NotFound => {
                                continue;
                            }
                            Err(remove_err) => {
                                return Err(io::Error::new(
                                    remove_err.kind(),
                                    format!(
                                        "failed to clear stale session lock '{}': {remove_err}",
                                        lock_path.display()
                                    ),
                                ));
                            }
                        }
                    }

                    if start.elapsed() >= SESSION_LOCK_TIMEOUT {
                        return Err(io::Error::new(
                            io::ErrorKind::WouldBlock,
                            format!(
                                "timed out waiting for session lock '{}'",
                                lock_path.display()
                            ),
                        ));
                    }
                    thread::sleep(SESSION_LOCK_POLL_INTERVAL);
                }
                Err(err) => {
                    return Err(io::Error::new(
                        err.kind(),
                        format!(
                            "failed to open session lock '{}': {err}",
                            lock_path.display()
                        ),
                    ));
                }
            }
        }
    }

    /// Move a malformed session file out of the way so ingestion can continue.
    pub fn archive_corrupt_session(&self, id: &str) -> io::Result<Option<PathBuf>> {
        let source = self.session_path(id);
        let archived = self.sessions_dir.join(format!(
            "{id}.corrupt.{}.json",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));

        match fs::rename(&source, &archived) {
            Ok(()) => Ok(Some(archived)),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(io::Error::new(
                err.kind(),
                format!(
                    "failed to archive corrupt session file '{}' -> '{}': {err}",
                    source.display(),
                    archived.display()
                ),
            )),
        }
    }

    fn session_path(&self, id: &str) -> PathBuf {
        self.sessions_dir.join(format!("{id}.json"))
    }

    fn session_lock_path(&self, id: &str) -> PathBuf {
        self.sessions_dir.join(format!("{id}.lock"))
    }

    fn is_stale_lock(&self, lock_path: &Path) -> bool {
        let Ok(metadata) = fs::metadata(lock_path) else {
            return false;
        };
        let Ok(modified_at) = metadata.modified() else {
            return false;
        };
        let Ok(elapsed) = modified_at.elapsed() else {
            return false;
        };
        elapsed >= STALE_SESSION_LOCK_AGE
    }

    fn write_atomic(&self, destination: &Path, data: &[u8]) -> io::Result<()> {
        let parent = destination.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "invalid session file path without parent: '{}'",
                    destination.display()
                ),
            )
        })?;

        let file_name = destination
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("invalid session file name: '{}'", destination.display()),
                )
            })?;

        let unique_suffix = format!(
            "{}.{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let temp_path = parent.join(format!(".{file_name}.{unique_suffix}.tmp"));

        fs::write(&temp_path, data).map_err(|err| {
            io::Error::new(
                err.kind(),
                format!(
                    "failed to write temporary session file '{}': {err}",
                    temp_path.display()
                ),
            )
        })?;

        #[cfg(windows)]
        {
            if destination.exists() {
                match fs::remove_file(destination) {
                    Ok(()) => {}
                    Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                    Err(err) => {
                        let _ = fs::remove_file(&temp_path);
                        return Err(io::Error::new(
                            err.kind(),
                            format!(
                                "failed to replace session file '{}': {err}",
                                destination.display()
                            ),
                        ));
                    }
                }
            }
        }

        fs::rename(&temp_path, destination).map_err(|err| {
            let _ = fs::remove_file(&temp_path);
            io::Error::new(
                err.kind(),
                format!(
                    "failed to replace session file '{}' with '{}': {err}",
                    destination.display(),
                    temp_path.display()
                ),
            )
        })
    }
}

fn session_matches_thread_id(session: &SessionState, thread_id: &str) -> bool {
    if session.id == thread_id {
        return true;
    }

    THREAD_ID_METADATA_KEYS.iter().any(|key| {
        session
            .metadata
            .get(*key)
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| value == thread_id)
    })
}

/// Brief info about a saved session.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub id: String,
    pub created_at: String,
    pub summary: String,
    pub message_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_save_and_load() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = SessionStore::new(tmp.path());

        let mut session = SessionState::new("/tmp/test");
        session.summary = "Test session".to_string();
        session.add_user_message("hello");

        store.save(&session).unwrap();

        let loaded = store.load(&session.id).unwrap();
        assert_eq!(loaded.id, session.id);
        assert_eq!(loaded.summary, "Test session");
        assert_eq!(loaded.message_count(), 1);
    }

    #[test]
    fn test_list_sessions() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = SessionStore::new(tmp.path());

        let s1 = SessionState::new("/tmp/a");
        let s2 = SessionState::new("/tmp/b");
        store.save(&s1).unwrap();
        store.save(&s2).unwrap();

        let list = store.list().unwrap();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn test_list_empty() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = SessionStore::new(tmp.path());

        let list = store.list().unwrap();
        assert!(list.is_empty());
    }

    #[test]
    fn test_delete_session() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = SessionStore::new(tmp.path());

        let session = SessionState::new("/tmp/test");
        store.save(&session).unwrap();
        assert!(store.load(&session.id).is_ok());

        store.delete(&session.id).unwrap();
        assert!(store.load(&session.id).is_err());
    }

    #[test]
    fn test_from_storage_path() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = SessionStore::from_storage_path(tmp.path());

        let session = SessionState::new("/tmp/test");
        store.save(&session).unwrap();
        assert!(
            tmp.path()
                .join("sessions")
                .join(format!("{}.json", session.id))
                .exists()
        );
    }

    #[test]
    fn test_load_latest() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = SessionStore::new(tmp.path());

        let s1 = SessionState::new("/tmp/first");
        store.save(&s1).unwrap();

        // Brief sleep to ensure different modified times
        std::thread::sleep(std::time::Duration::from_millis(10));

        let mut s2 = SessionState::new("/tmp/second");
        s2.summary = "latest".to_string();
        store.save(&s2).unwrap();

        let latest = store.load_latest().unwrap().unwrap();
        assert_eq!(latest.summary, "latest");
    }

    #[test]
    fn test_load_latest_for_working_dir_filters_shared_storage() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = SessionStore::from_storage_path(tmp.path());

        let mut worktree_a_old = SessionState::new("/repo/.worktrees/a");
        worktree_a_old.summary = "a-old".to_string();
        store.save(&worktree_a_old).unwrap();

        std::thread::sleep(std::time::Duration::from_millis(10));

        let mut worktree_b = SessionState::new("/repo/.worktrees/b");
        worktree_b.summary = "b-latest".to_string();
        store.save(&worktree_b).unwrap();

        std::thread::sleep(std::time::Duration::from_millis(10));

        let mut worktree_a_new = SessionState::new("/repo/.worktrees/a");
        worktree_a_new.summary = "a-latest".to_string();
        store.save(&worktree_a_new).unwrap();

        let latest_for_a = store
            .load_latest_for_working_dir("/repo/.worktrees/a")
            .unwrap()
            .unwrap();
        assert_eq!(latest_for_a.summary, "a-latest");

        let latest_for_b = store
            .load_latest_for_working_dir("/repo/.worktrees/b")
            .unwrap()
            .unwrap();
        assert_eq!(latest_for_b.summary, "b-latest");
    }

    #[test]
    fn test_load_for_thread_id_uses_canonical_metadata_and_working_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = SessionStore::from_storage_path(tmp.path());
        let thread_id = "11111111-1111-4111-8111-111111111111";

        let mut wrong_worktree = SessionState::new("/repo/other");
        wrong_worktree.summary = "wrong".to_string();
        wrong_worktree
            .metadata
            .insert("thread_id".to_string(), serde_json::json!(thread_id));
        store.save(&wrong_worktree).unwrap();

        std::thread::sleep(std::time::Duration::from_millis(10));

        let mut canonical = SessionState::new("/repo/main");
        canonical.summary = "canonical".to_string();
        canonical
            .metadata
            .insert("thread_id".to_string(), serde_json::json!(thread_id));
        store.save(&canonical).unwrap();

        let loaded = store
            .load_for_thread_id(thread_id, "/repo/main")
            .unwrap()
            .unwrap();
        assert_eq!(loaded.summary, "canonical");
    }

    #[test]
    fn test_save_load_roundtrip_with_messages_and_metadata() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = SessionStore::new(tmp.path());

        let mut session = SessionState::new("/project/dir");
        session.summary = "Implemented auth feature".to_string();
        session.context_mode = Some("dev".to_string());
        session.add_user_message("add JWT authentication");
        session.add_assistant_message("I'll implement JWT auth using jsonwebtoken crate");
        session.add_user_message("looks good, proceed");
        session.add_assistant_message("Done. Created auth module with login/verify endpoints.");
        session
            .metadata
            .insert("provider".to_string(), serde_json::json!("anthropic"));
        session
            .metadata
            .insert("model".to_string(), serde_json::json!("claude-3-5-sonnet"));

        store.save(&session).unwrap();

        let loaded = store.load(&session.id).unwrap();
        assert_eq!(loaded.id, session.id);
        assert_eq!(loaded.working_dir, "/project/dir");
        assert_eq!(loaded.summary, "Implemented auth feature");
        assert_eq!(loaded.context_mode.as_deref(), Some("dev"));
        assert_eq!(loaded.message_count(), 4);
        assert_eq!(loaded.messages[0].role, "user");
        assert_eq!(loaded.messages[0].content, "add JWT authentication");
        assert_eq!(loaded.messages[3].role, "assistant");
        assert_eq!(loaded.metadata.get("provider").unwrap(), "anthropic");
        assert_eq!(loaded.metadata.get("model").unwrap(), "claude-3-5-sonnet");
    }

    #[test]
    fn test_load_latest_empty() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = SessionStore::new(tmp.path());

        assert!(store.load_latest().unwrap().is_none());
    }
}
