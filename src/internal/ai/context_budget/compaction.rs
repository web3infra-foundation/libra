//! Append-only compaction records for context-frame replay.
//!
//! Carries the [`CompactionEvent`] schema, OC-Phase 4 P4.5
//! `tail_start_id` field, and the canonical prune / preserve-recent
//! constants the doc requires (`docs/improvement/opencode.md`
//! "Libra Compaction 默认常量"). The constants are exported so the
//! prune projection in [`super::projection`] and the budget
//! calculator in [`super::budget`] read the same values; OC-Phase 5
//! adds a `[code.compaction]` TOML section that can override them
//! per-project.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{ContextAttachmentRef, ContextFrameEvent, ContextSegmentKind};
use crate::internal::ai::runtime::event::Event;

/// Below this token count the runtime will not run a prune pass.
/// Mirrors `session/compaction.ts:36` PRUNE_MINIMUM.
pub const PRUNE_MINIMUM: u64 = 20_000;

/// Above this token count the runtime escalates from prune to
/// full compaction. Mirrors `session/compaction.ts:37`
/// PRUNE_PROTECT.
pub const PRUNE_PROTECT: u64 = 40_000;

/// Maximum inline character count for a single tool output before
/// the prune projection swaps it for a `<pruned>` placeholder.
/// Mirrors `session/compaction.ts:38` TOOL_OUTPUT_MAX_CHARS.
pub const TOOL_OUTPUT_MAX_CHARS: usize = 2_000;

/// Tool names whose outputs the prune projection refuses to swap
/// even when oversized. The opencode upstream protects only
/// `["skill"]`; Libra also protects the two intent / plan draft
/// submission tools because they carry the only durable record of
/// the user's stated intent and dropping them mid-session would
/// erase that signal. Mirrors `session/compaction.ts:39`
/// PRUNE_PROTECTED_TOOLS.
pub const PRUNE_PROTECTED_TOOLS: &[&str] = &["skill", "submit_intent_draft", "submit_plan_draft"];

/// Default number of conversation turns the runtime keeps as the
/// "retained tail" past the compaction marker. Mirrors
/// `session/compaction.ts:40` DEFAULT_TAIL_TURNS.
pub const DEFAULT_TAIL_TURNS: usize = 2;

/// Lower bound on the token budget reserved for the retained tail
/// after compaction. Mirrors `session/compaction.ts:41`
/// MIN_PRESERVE_RECENT_TOKENS.
pub const MIN_PRESERVE_RECENT_TOKENS: u64 = 2_000;

/// Upper bound on the token budget reserved for the retained tail
/// after compaction. Mirrors `session/compaction.ts:42`
/// MAX_PRESERVE_RECENT_TOKENS.
pub const MAX_PRESERVE_RECENT_TOKENS: u64 = 8_000;

/// Compute the preserve-recent budget for a given usable-token
/// count. Mirrors the formula at `session/compaction.ts:137-142`:
/// `min(MAX, max(MIN, floor(usable * 0.25)))`. Saturating math
/// keeps the function panic-free on overflow.
pub fn preserve_recent_budget(usable: u64) -> u64 {
    (usable / 4).clamp(MIN_PRESERVE_RECENT_TOKENS, MAX_PRESERVE_RECENT_TOKENS)
}

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
///
/// `tail_start_id` aligns with opencode `CompactionPart.tailStartID`
/// (PR #25851, 2026-05-05). It is the segment id of the first
/// message the runtime kept as the post-compaction "retained tail";
/// without it the [`super::projection::filter_compacted`] reorder
/// rule cannot determine where the tail begins. `None` means the
/// compaction discarded the tail entirely (the retained-tail budget
/// was fully consumed by the summary). Field defaults to `None`
/// during deserialization so JSONL written before the field landed
/// keeps replaying.
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tail_start_id: Option<String>,
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
            tail_start_id: None,
        }
    }

    /// Return a clone of `self` with `tail_start_id` set. Builder-style
    /// helper so the dispatcher can decorate a frame-derived event
    /// without re-deriving every field.
    #[must_use]
    pub fn with_tail_start_id(mut self, tail_start_id: impl Into<String>) -> Self {
        self.tail_start_id = Some(tail_start_id.into());
        self
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
