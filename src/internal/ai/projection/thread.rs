use chrono::{DateTime, Utc};
use git_internal::internal::object::types::ActorRef;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub type ThreadId = Uuid;

/// Current conversational projection over a related Intent DAG.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThreadProjection {
    pub thread_id: ThreadId,
    pub title: Option<String>,
    pub owner: ActorRef,
    #[serde(default)]
    pub participants: Vec<ThreadParticipant>,
    pub current_intent_id: Option<Uuid>,
    pub latest_intent_id: Option<Uuid>,
    #[serde(default)]
    pub intents: Vec<ThreadIntentRef>,
    pub metadata: Option<Value>,
    pub archived: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub version: i64,
}

/// Actor membership in a thread projection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadParticipant {
    pub actor: ActorRef,
    pub role: ThreadParticipantRole,
    pub joined_at: DateTime<Utc>,
}

/// Intent membership state within a thread projection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadIntentRef {
    pub intent_id: Uuid,
    pub ordinal: i64,
    pub is_head: bool,
    pub linked_at: DateTime<Utc>,
    pub link_reason: ThreadIntentLinkReason,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThreadParticipantRole {
    Owner,
    Member,
    Reviewer,
    Observer,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThreadIntentLinkReason {
    Seed,
    Revision,
    Split,
    Merge,
    Followup,
    Imported,
}
