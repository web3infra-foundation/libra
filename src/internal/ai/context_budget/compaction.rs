//! Append-only compaction records for context-frame replay.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{ContextAttachmentRef, ContextFrameEvent, ContextSegmentKind};
use crate::internal::ai::runtime::event::Event;

/// Deterministic reason a compaction event was recorded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionReason {
    BudgetPressure,
    ResumeReplay,
    Manual,
    Maintenance,
}

impl CompactionReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::BudgetPressure => "budget_pressure",
            Self::ResumeReplay => "resume_replay",
            Self::Manual => "manual",
            Self::Maintenance => "maintenance",
        }
    }
}

/// Append-only compaction event used to replay why context changed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompactionEvent {
    pub event_id: Uuid,
    pub recorded_at: DateTime<Utc>,
    pub frame_id: Uuid,
    pub reason: CompactionReason,
    pub summary: String,
    pub tokens_before: u64,
    pub tokens_after: u64,
    pub omitted_segment_ids: Vec<String>,
    pub protected_segment_ids: Vec<String>,
    pub attachment_refs: Vec<ContextAttachmentRef>,
}

impl CompactionEvent {
    pub fn from_frame(
        frame: &ContextFrameEvent,
        reason: CompactionReason,
        summary: impl Into<String>,
    ) -> Self {
        let protected_segment_ids = frame
            .segments
            .iter()
            .filter(|segment| {
                segment.non_compressible || segment.segment == ContextSegmentKind::SystemRules
            })
            .map(|segment| segment.id.clone())
            .collect();

        Self {
            event_id: Uuid::new_v4(),
            recorded_at: Utc::now(),
            frame_id: frame.frame_id,
            reason,
            summary: summary.into(),
            tokens_before: frame.total_candidate_tokens,
            tokens_after: frame.total_selected_tokens,
            omitted_segment_ids: frame
                .omissions
                .iter()
                .map(|omission| omission.id.clone())
                .collect(),
            protected_segment_ids,
            attachment_refs: frame.attachment_refs(),
        }
    }
}

impl Event for CompactionEvent {
    fn event_kind(&self) -> &'static str {
        "compaction_event"
    }

    fn event_id(&self) -> Uuid {
        self.event_id
    }

    fn event_summary(&self) -> String {
        format!(
            "{} compaction for frame {}: {} -> {} token(s)",
            self.reason.as_str(),
            self.frame_id,
            self.tokens_before,
            self.tokens_after
        )
    }
}
