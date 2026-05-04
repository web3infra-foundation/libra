//! Memory anchors used to carry confirmed semantic constraints across turns.

use std::{collections::BTreeMap, fmt};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::internal::ai::runtime::event::Event;

/// Type of semantic memory captured by an anchor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryAnchorKind {
    UserConstraint,
    ProjectInvariant,
    ArchitectureDecision,
    VerifiedFinding,
    LongRunningTodo,
}

impl MemoryAnchorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UserConstraint => "user_constraint",
            Self::ProjectInvariant => "project_invariant",
            Self::ArchitectureDecision => "architecture_decision",
            Self::VerifiedFinding => "verified_finding",
            Self::LongRunningTodo => "long_running_todo",
        }
    }
}

impl fmt::Display for MemoryAnchorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Scope where an anchor may influence future behavior.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryAnchorScope {
    Session,
    AgentRun,
    Project,
}

impl MemoryAnchorScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::AgentRun => "agent_run",
            Self::Project => "project",
        }
    }
}

impl fmt::Display for MemoryAnchorScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Confidence assigned to an anchor at draft time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryAnchorConfidence {
    Low,
    Medium,
    High,
}

impl MemoryAnchorConfidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

impl fmt::Display for MemoryAnchorConfidence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Review state that controls whether an anchor enters the prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryAnchorReviewState {
    Draft,
    Confirmed,
    Revoked,
    Superseded,
}

impl MemoryAnchorReviewState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Confirmed => "confirmed",
            Self::Revoked => "revoked",
            Self::Superseded => "superseded",
        }
    }
}

impl fmt::Display for MemoryAnchorReviewState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Append-only lifecycle action for a memory anchor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryAnchorAction {
    Drafted,
    Confirmed,
    Revoked,
    Superseded,
}

impl MemoryAnchorAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Drafted => "drafted",
            Self::Confirmed => "confirmed",
            Self::Revoked => "revoked",
            Self::Superseded => "superseded",
        }
    }
}

/// Current replayed view of a memory anchor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryAnchor {
    pub anchor_id: Uuid,
    pub kind: MemoryAnchorKind,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_event_id: Option<Uuid>,
    pub confidence: MemoryAnchorConfidence,
    pub scope: MemoryAnchorScope,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    pub review_state: MemoryAnchorReviewState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl MemoryAnchor {
    pub fn is_active_at(&self, now: DateTime<Utc>) -> bool {
        self.review_state == MemoryAnchorReviewState::Confirmed
            && self.superseded_by.is_none()
            && self.expires_at.is_none_or(|expires_at| expires_at > now)
    }

    pub fn short_id(&self) -> String {
        self.anchor_id
            .to_string()
            .chars()
            .take(8)
            .collect::<String>()
    }

    fn from_event(event: &MemoryAnchorEvent) -> Self {
        Self {
            anchor_id: event.anchor_id,
            kind: event.kind,
            content: event.content.clone(),
            source_event_id: event.source_event_id,
            confidence: event.confidence,
            scope: event.scope,
            created_by: event.created_by.clone(),
            created_at: event.created_at,
            updated_at: event.updated_at,
            expires_at: event.expires_at,
            review_state: event.review_state,
            superseded_by: event.superseded_by,
            reason: event.reason.clone(),
        }
    }
}

/// Input for a new draft anchor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryAnchorDraft {
    pub kind: MemoryAnchorKind,
    pub content: String,
    pub source_event_id: Option<Uuid>,
    pub confidence: MemoryAnchorConfidence,
    pub scope: MemoryAnchorScope,
    pub created_by: String,
    pub expires_at: Option<DateTime<Utc>>,
}

impl MemoryAnchorDraft {
    pub fn session_user_constraint(
        content: impl Into<String>,
        created_by: impl Into<String>,
    ) -> Self {
        Self {
            kind: MemoryAnchorKind::UserConstraint,
            content: content.into(),
            source_event_id: None,
            confidence: MemoryAnchorConfidence::Medium,
            scope: MemoryAnchorScope::Session,
            created_by: created_by.into(),
            expires_at: None,
        }
    }
}

/// Append-only memory-anchor event persisted in session JSONL.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryAnchorEvent {
    pub event_id: Uuid,
    pub recorded_at: DateTime<Utc>,
    pub anchor_id: Uuid,
    pub action: MemoryAnchorAction,
    pub kind: MemoryAnchorKind,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_event_id: Option<Uuid>,
    pub confidence: MemoryAnchorConfidence,
    pub scope: MemoryAnchorScope,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    pub review_state: MemoryAnchorReviewState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl MemoryAnchorEvent {
    pub fn draft(draft: MemoryAnchorDraft) -> Self {
        let now = Utc::now();
        Self {
            event_id: Uuid::new_v4(),
            recorded_at: now,
            anchor_id: Uuid::new_v4(),
            action: MemoryAnchorAction::Drafted,
            kind: draft.kind,
            content: draft.content,
            source_event_id: draft.source_event_id,
            confidence: draft.confidence,
            scope: draft.scope,
            created_by: draft.created_by,
            created_at: now,
            updated_at: now,
            expires_at: draft.expires_at,
            review_state: MemoryAnchorReviewState::Draft,
            superseded_by: None,
            reason: None,
        }
    }

    pub fn confirm(anchor: &MemoryAnchor, reason: impl Into<Option<String>>) -> Self {
        Self::from_anchor(
            anchor,
            MemoryAnchorAction::Confirmed,
            MemoryAnchorReviewState::Confirmed,
            None,
            reason.into(),
        )
    }

    pub fn revoke(anchor: &MemoryAnchor, reason: impl Into<Option<String>>) -> Self {
        Self::from_anchor(
            anchor,
            MemoryAnchorAction::Revoked,
            MemoryAnchorReviewState::Revoked,
            None,
            reason.into(),
        )
    }

    pub fn supersede(
        anchor: &MemoryAnchor,
        superseded_by: Uuid,
        reason: impl Into<Option<String>>,
    ) -> Self {
        Self::from_anchor(
            anchor,
            MemoryAnchorAction::Superseded,
            MemoryAnchorReviewState::Superseded,
            Some(superseded_by),
            reason.into(),
        )
    }

    fn from_anchor(
        anchor: &MemoryAnchor,
        action: MemoryAnchorAction,
        review_state: MemoryAnchorReviewState,
        superseded_by: Option<Uuid>,
        reason: Option<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            event_id: Uuid::new_v4(),
            recorded_at: now,
            anchor_id: anchor.anchor_id,
            action,
            kind: anchor.kind,
            content: anchor.content.clone(),
            source_event_id: anchor.source_event_id,
            confidence: anchor.confidence,
            scope: anchor.scope,
            created_by: anchor.created_by.clone(),
            created_at: anchor.created_at,
            updated_at: now,
            expires_at: anchor.expires_at,
            review_state,
            superseded_by,
            reason,
        }
    }
}

impl Event for MemoryAnchorEvent {
    fn event_kind(&self) -> &'static str {
        "memory_anchor"
    }

    fn event_id(&self) -> Uuid {
        self.event_id
    }

    fn event_summary(&self) -> String {
        format!(
            "{} memory anchor {} ({})",
            self.action.as_str(),
            self.anchor_id,
            self.review_state.as_str()
        )
    }
}

/// Replayed memory-anchor projection for a session JSONL stream.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemoryAnchorReplay {
    anchors: BTreeMap<Uuid, MemoryAnchor>,
}

impl MemoryAnchorReplay {
    pub fn apply_event(&mut self, event: MemoryAnchorEvent) {
        self.anchors
            .insert(event.anchor_id, MemoryAnchor::from_event(&event));
    }

    pub fn anchors(&self) -> Vec<MemoryAnchor> {
        self.anchors.values().cloned().collect()
    }

    pub fn active_anchors_at(&self, now: DateTime<Utc>) -> Vec<MemoryAnchor> {
        self.anchors
            .values()
            .filter(|anchor| anchor.is_active_at(now))
            .cloned()
            .collect()
    }

    pub fn find_unique_by_prefix(
        &self,
        prefix: &str,
    ) -> Result<MemoryAnchor, MemoryAnchorLookupError> {
        let prefix = prefix.trim();
        if prefix.is_empty() {
            return Err(MemoryAnchorLookupError::EmptyPrefix);
        }

        let matches = self
            .anchors
            .values()
            .filter(|anchor| anchor.anchor_id.to_string().starts_with(prefix))
            .cloned()
            .collect::<Vec<_>>();

        match matches.as_slice() {
            [] => Err(MemoryAnchorLookupError::NotFound(prefix.to_string())),
            [anchor] => Ok(anchor.clone()),
            _ => Err(MemoryAnchorLookupError::Ambiguous(prefix.to_string())),
        }
    }
}

/// Lookup errors for `/anchors <action> <id-prefix>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MemoryAnchorLookupError {
    EmptyPrefix,
    NotFound(String),
    Ambiguous(String),
}

impl fmt::Display for MemoryAnchorLookupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyPrefix => formatter.write_str("missing memory anchor id prefix"),
            Self::NotFound(prefix) => write!(formatter, "no memory anchor matches `{prefix}`"),
            Self::Ambiguous(prefix) => {
                write!(formatter, "memory anchor prefix `{prefix}` is ambiguous")
            }
        }
    }
}

/// Render confirmed active anchors for prompt injection.
pub fn build_memory_anchor_prompt_section(
    anchors: &[MemoryAnchor],
    now: DateTime<Utc>,
) -> Option<String> {
    let mut active = anchors
        .iter()
        .filter(|anchor| anchor.is_active_at(now))
        .cloned()
        .collect::<Vec<_>>();
    if active.is_empty() {
        return None;
    }

    active.sort_by_key(|anchor| {
        (
            anchor.scope,
            anchor.kind,
            anchor.confidence,
            anchor.created_at,
            anchor.anchor_id,
        )
    });

    let mut lines = vec!["## Memory Anchors".to_string(), String::new()];
    for anchor in active {
        lines.push(format!(
            "- id={} kind={} scope={} confidence={} source_event={}{}",
            anchor.short_id(),
            anchor.kind,
            anchor.scope,
            anchor.confidence,
            anchor
                .source_event_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "none".to_string()),
            anchor
                .expires_at
                .map(|expires| format!(" expires_at={}", expires.to_rfc3339()))
                .unwrap_or_default()
        ));
        lines.push(format!("  {}", anchor.content));
    }

    Some(lines.join("\n"))
}
