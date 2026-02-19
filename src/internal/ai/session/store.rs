//! Session storage: save and load sessions from disk.

use std::path::{Path, PathBuf};

use super::state::SessionState;

/// Manages session persistence on disk.
///
/// Sessions are stored as JSON files in a sessions directory.
pub struct SessionStore {
    sessions_dir: PathBuf,
}

impl SessionStore {
    /// Create a store rooted at `{working_dir}/.libra/sessions/`.
    pub fn new(working_dir: &Path) -> Self {
        Self {
            sessions_dir: working_dir.join(".libra").join("sessions"),
        }
    }

    /// Create the sessions directory if it doesn't exist.
    fn ensure_dir(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.sessions_dir)
    }

    /// Save a session to disk.
    pub fn save(&self, session: &SessionState) -> std::io::Result<()> {
        self.ensure_dir()?;
        let path = self.session_path(&session.id);
        let json = serde_json::to_string_pretty(session)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(path, json)
    }

    /// Load a session by ID.
    pub fn load(&self, id: &str) -> std::io::Result<SessionState> {
        let path = self.session_path(id);
        let content = std::fs::read_to_string(path)?;
        serde_json::from_str(&content)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    /// Load the most recently updated session.
    pub fn load_latest(&self) -> std::io::Result<Option<SessionState>> {
        let sessions = self.list()?;
        if sessions.is_empty() {
            return Ok(None);
        }

        // Find the most recently modified session file
        let mut latest: Option<(SessionState, std::time::SystemTime)> = None;

        for info in sessions {
            match self.load(&info.id) {
                Ok(session) => {
                    let path = self.session_path(&info.id);
                    let modified = std::fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);

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

    /// List all saved sessions (basic info only).
    pub fn list(&self) -> std::io::Result<Vec<SessionInfo>> {
        if !self.sessions_dir.exists() {
            return Ok(Vec::new());
        }

        let mut sessions = Vec::new();
        for entry in std::fs::read_dir(&self.sessions_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "json") {
                match std::fs::read_to_string(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|content| {
                        serde_json::from_str::<SessionState>(&content)
                            .map_err(|e| e.to_string())
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
    pub fn delete(&self, id: &str) -> std::io::Result<()> {
        let path = self.session_path(id);
        std::fs::remove_file(path)
    }

    fn session_path(&self, id: &str) -> PathBuf {
        self.sessions_dir.join(format!("{id}.json"))
    }
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
        session.metadata.insert(
            "model".to_string(),
            serde_json::json!("claude-3-5-sonnet"),
        );

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
