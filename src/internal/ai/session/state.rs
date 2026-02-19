//! Session state types.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Unique session identifier.
pub type SessionId = String;

/// Persistent session state for save/restore across sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    /// Unique session identifier.
    pub id: SessionId,
    /// When the session was created.
    pub created_at: DateTime<Utc>,
    /// When the session was last updated.
    pub updated_at: DateTime<Utc>,
    /// Working directory for this session.
    pub working_dir: String,
    /// Context mode active during this session (if any).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_mode: Option<String>,
    /// Summary of what was done in this session.
    #[serde(default)]
    pub summary: String,
    /// Conversation history (serialized messages).
    #[serde(default)]
    pub messages: Vec<SessionMessage>,
    /// Arbitrary metadata.
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// A serializable message in the session history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMessage {
    /// "user", "assistant", or "system".
    pub role: String,
    /// The message content.
    pub content: String,
    /// Timestamp.
    pub timestamp: DateTime<Utc>,
}

impl SessionState {
    /// Create a new session with a generated ID.
    pub fn new(working_dir: &str) -> Self {
        let now = Utc::now();
        Self {
            id: generate_session_id(),
            created_at: now,
            updated_at: now,
            working_dir: working_dir.to_string(),
            context_mode: None,
            summary: String::new(),
            messages: Vec::new(),
            metadata: HashMap::new(),
        }
    }

    /// Add a user message to the session.
    pub fn add_user_message(&mut self, content: &str) {
        self.messages.push(SessionMessage {
            role: "user".to_string(),
            content: content.to_string(),
            timestamp: Utc::now(),
        });
        self.updated_at = Utc::now();
    }

    /// Add an assistant message to the session.
    pub fn add_assistant_message(&mut self, content: &str) {
        self.messages.push(SessionMessage {
            role: "assistant".to_string(),
            content: content.to_string(),
            timestamp: Utc::now(),
        });
        self.updated_at = Utc::now();
    }

    /// Get the number of messages.
    pub fn message_count(&self) -> usize {
        self.messages.len()
    }
}

/// Generate a short, unique session ID.
fn generate_session_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    format!("{timestamp:x}-{pid:04x}-{count:04x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_session() {
        let session = SessionState::new("/tmp/test");
        assert!(!session.id.is_empty());
        assert_eq!(session.working_dir, "/tmp/test");
        assert!(session.messages.is_empty());
        assert!(session.summary.is_empty());
    }

    #[test]
    fn test_add_messages() {
        let mut session = SessionState::new("/tmp/test");
        session.add_user_message("hello");
        session.add_assistant_message("hi there");

        assert_eq!(session.message_count(), 2);
        assert_eq!(session.messages[0].role, "user");
        assert_eq!(session.messages[0].content, "hello");
        assert_eq!(session.messages[1].role, "assistant");
        assert_eq!(session.messages[1].content, "hi there");
    }

    #[test]
    fn test_session_serialization() {
        let mut session = SessionState::new("/tmp/test");
        session.add_user_message("test message");
        session.summary = "Test session".to_string();

        let json = serde_json::to_string(&session).unwrap();
        let restored: SessionState = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.id, session.id);
        assert_eq!(restored.working_dir, session.working_dir);
        assert_eq!(restored.summary, "Test session");
        assert_eq!(restored.message_count(), 1);
    }

    #[test]
    fn test_unique_session_ids() {
        let s1 = SessionState::new("/tmp");
        let s2 = SessionState::new("/tmp");
        assert_ne!(s1.id, s2.id);
    }
}
