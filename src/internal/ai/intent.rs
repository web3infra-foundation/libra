use std::fmt;

use chrono::{DateTime, Utc};
use git_internal::{hash::ObjectHash, internal::object::types::ActorRef};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::utils::storage_ext::Identifiable;

/// Status of the intent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IntentStatus {
    /// Initial state, intent is being defined or refined.
    Draft,
    /// Intent is actively being processed by the AI or user.
    Active,
    /// Intent has been fulfilled (e.g., code generated and committed).
    Completed,
    /// Intent was discarded or abandoned.
    Discarded,
}

impl IntentStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            IntentStatus::Draft => "draft",
            IntentStatus::Active => "active",
            IntentStatus::Completed => "completed",
            IntentStatus::Discarded => "discarded",
        }
    }
}

impl fmt::Display for IntentStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Intent object representing a user prompt or high-level goal.
/// It is stored on the unified AI history branch (`refs/libra/intent`) alongside
/// all other AI process objects (Task, Run, Plan, etc.).
///
/// The `parent_id` field forms a logical chain of intents, allowing traversal
/// of the "What/Why" history independently of implementation details.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Intent {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    /// Pointer to the previous intent in the chain (forming the parallel branch).
    pub parent_id: Option<Uuid>,
    /// The actual prompt or intent content.
    pub content: String,
    /// Optional link to a specific task.
    pub task_id: Option<Uuid>,
    /// The actor who created this intent.
    pub created_by: Option<ActorRef>,
    /// Optional link to a resulting code commit hash (strong typed).
    pub commit_sha: Option<ObjectHash>,
    /// Status of the intent.
    pub status: IntentStatus,
}

impl Intent {
    pub fn new(
        content: String,
        parent_id: Option<Uuid>,
        task_id: Option<Uuid>,
        created_by: Option<ActorRef>,
    ) -> Self {
        Self {
            id: Uuid::now_v7(),
            created_at: Utc::now(),
            parent_id,
            content,
            task_id,
            created_by,
            commit_sha: None,
            status: IntentStatus::Active,
        }
    }

    pub fn set_commit_sha(&mut self, sha: ObjectHash) {
        self.commit_sha = Some(sha);
    }

    pub fn set_status(&mut self, status: IntentStatus) {
        self.status = status;
    }
}

impl Identifiable for Intent {
    fn object_id(&self) -> String {
        self.id.to_string()
    }

    fn object_type(&self) -> String {
        "intent".to_string()
    }
}
