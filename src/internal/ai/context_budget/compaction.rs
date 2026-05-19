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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::context_budget::frame::{
        ContextFrameEvent, ContextFrameKind, ContextFrameOmission, ContextFrameSegment,
        ContextFrameSource, ContextTrustLevel,
    };

    fn frame_segment(
        id: &str,
        segment: ContextSegmentKind,
        non_compressible: bool,
        attachment: Option<ContextAttachmentRef>,
    ) -> ContextFrameSegment {
        ContextFrameSegment {
            id: id.to_string(),
            segment,
            source: ContextFrameSource::runtime("test"),
            trust: ContextTrustLevel::Trusted,
            token_estimate: 10,
            content: Some("body".to_string()),
            summary: None,
            attachment,
            non_compressible,
        }
    }

    fn attachment_ref(label: &str) -> ContextAttachmentRef {
        ContextAttachmentRef {
            sha256: format!("{label}-sha"),
            bytes: 42,
            line_count: 3,
            relative_path: format!("{label}.txt"),
            read_hint: "noop".to_string(),
        }
    }

    fn sample_frame() -> ContextFrameEvent {
        ContextFrameEvent {
            event_id: Uuid::nil(),
            recorded_at: Utc::now(),
            frame_id: Uuid::nil(),
            kind: ContextFrameKind::PromptBuild,
            prompt_id: None,
            segments: vec![
                frame_segment("rules", ContextSegmentKind::SystemRules, false, None),
                frame_segment(
                    "nc",
                    ContextSegmentKind::ToolResults,
                    true,
                    Some(attachment_ref("att-nc")),
                ),
                frame_segment(
                    "plain",
                    ContextSegmentKind::RecentMessages,
                    false,
                    Some(attachment_ref("att-plain")),
                ),
            ],
            omissions: vec![ContextFrameOmission {
                id: "dropped".to_string(),
                segment: ContextSegmentKind::SourceContext,
                token_estimate: 20,
                reason: super::super::allocator::AllocationOmissionReason::TotalBudgetExceeded,
            }],
            total_candidate_tokens: 100,
            total_selected_tokens: 60,
            budget_exceeded_by: 0,
        }
    }

    #[test]
    fn prune_constants_pin_compaction_thresholds() {
        // INVARIANT: these mirror opencode `session/compaction.ts` and
        // double as the contract the prune projection in
        // `super::projection` and the budget calculator in
        // `super::budget` rely on. A silent drift would diverge from
        // the upstream behaviour the doc says we replicate.
        assert_eq!(PRUNE_MINIMUM, 20_000);
        assert_eq!(PRUNE_PROTECT, 40_000);
        assert_eq!(TOOL_OUTPUT_MAX_CHARS, 2_000);
        assert_eq!(DEFAULT_TAIL_TURNS, 2);
        assert_eq!(MIN_PRESERVE_RECENT_TOKENS, 2_000);
        assert_eq!(MAX_PRESERVE_RECENT_TOKENS, 8_000);
    }

    #[test]
    fn prune_protected_tools_pins_the_three_member_allowlist() {
        // INVARIANT: prune must never drop output for these three
        // tools — `skill` (mirrors upstream) plus the two intent /
        // plan draft submission tools that carry the only durable
        // user-intent record. Re-ordering or shortening the list is
        // a behaviour change.
        assert_eq!(
            PRUNE_PROTECTED_TOOLS,
            &["skill", "submit_intent_draft", "submit_plan_draft"]
        );
    }

    #[test]
    fn preserve_recent_budget_matches_opencode_formula() {
        // INVARIANT: `min(MAX, max(MIN, floor(usable * 0.25)))`.
        // The constants are 2_000 / 8_000 — the boundary tests pin
        // both clamps and the divider math.
        assert_eq!(preserve_recent_budget(0), MIN_PRESERVE_RECENT_TOKENS);
        // Just below the lower clamp: 4 * MIN - 1 = 7999, /4 = 1999,
        // clamped up to MIN.
        assert_eq!(preserve_recent_budget(7_999), MIN_PRESERVE_RECENT_TOKENS);
        // Exact lower boundary: usable / 4 == MIN.
        assert_eq!(
            preserve_recent_budget(8_000),
            MIN_PRESERVE_RECENT_TOKENS,
            "8_000 / 4 lands at the MIN clamp"
        );
        // Mid-range value passes through unmodified.
        assert_eq!(preserve_recent_budget(20_000), 5_000);
        // Just below the upper clamp.
        assert_eq!(
            preserve_recent_budget(31_999),
            7_999,
            "31_999 / 4 = 7_999, still within [MIN, MAX]"
        );
        // Exact upper boundary: usable / 4 == MAX.
        assert_eq!(
            preserve_recent_budget(32_000),
            MAX_PRESERVE_RECENT_TOKENS,
            "32_000 / 4 lands exactly at MAX clamp"
        );
        // Saturating: large usable counts must clamp at MAX rather
        // than overflow or panic.
        assert_eq!(preserve_recent_budget(u64::MAX), MAX_PRESERVE_RECENT_TOKENS);
    }

    #[test]
    fn compaction_reason_as_str_pins_wire_strings() {
        // INVARIANT: these strings are persisted in JSONL alongside
        // every compaction event. Renaming would orphan history.
        assert_eq!(CompactionReason::BudgetPressure.as_str(), "budget_pressure");
        assert_eq!(CompactionReason::ResumeReplay.as_str(), "resume_replay");
        assert_eq!(CompactionReason::Manual.as_str(), "manual");
        assert_eq!(CompactionReason::Maintenance.as_str(), "maintenance");
    }

    #[test]
    fn from_frame_collects_omitted_ids_from_omissions_list() {
        let frame = sample_frame();
        let event =
            CompactionEvent::from_frame(&frame, CompactionReason::BudgetPressure, "summarised");
        assert_eq!(event.omitted_segment_ids, vec!["dropped"]);
    }

    #[test]
    fn from_frame_records_token_counts_from_frame() {
        let frame = sample_frame();
        let event = CompactionEvent::from_frame(&frame, CompactionReason::Manual, "manual run");
        assert_eq!(event.tokens_before, frame.total_candidate_tokens);
        assert_eq!(event.tokens_after, frame.total_selected_tokens);
        assert_eq!(event.frame_id, frame.frame_id);
        assert_eq!(event.summary, "manual run");
        assert_eq!(event.reason, CompactionReason::Manual);
        assert!(
            event.tail_start_id.is_none(),
            "from_frame must default tail_start_id to None"
        );
    }

    #[test]
    fn from_frame_protects_system_rules_and_non_compressible_segments() {
        // INVARIANT: protected_segment_ids is the union of
        // (non_compressible == true) and (segment ==
        // ContextSegmentKind::SystemRules). A silent narrowing
        // would let compaction drop SystemRules.
        let frame = sample_frame();
        let event =
            CompactionEvent::from_frame(&frame, CompactionReason::BudgetPressure, "summarised");
        // Order of `protected_segment_ids` follows iteration order
        // over `frame.segments`, so `rules` comes before `nc`.
        assert_eq!(event.protected_segment_ids, vec!["rules", "nc"]);
        assert!(!event.protected_segment_ids.contains(&"plain".to_string()));
    }

    #[test]
    fn from_frame_propagates_attachment_refs_in_segment_order() {
        // INVARIANT: `attachment_refs` filters `segments` for
        // `Some(attachment)` in input order. Reordering would change
        // how replay tooling correlates attachments with summaries.
        let frame = sample_frame();
        let event =
            CompactionEvent::from_frame(&frame, CompactionReason::BudgetPressure, "summarised");
        let labels: Vec<&str> = event
            .attachment_refs
            .iter()
            .map(|attachment| attachment.relative_path.as_str())
            .collect();
        assert_eq!(labels, vec!["att-nc.txt", "att-plain.txt"]);
    }

    #[test]
    fn from_frame_generates_a_fresh_event_id() {
        let frame = sample_frame();
        let a = CompactionEvent::from_frame(&frame, CompactionReason::BudgetPressure, "first");
        let b = CompactionEvent::from_frame(&frame, CompactionReason::BudgetPressure, "second");
        assert_ne!(
            a.event_id, b.event_id,
            "each compaction must get a new uuid"
        );
    }

    #[test]
    fn with_tail_start_id_replaces_only_the_tail_field() {
        let frame = sample_frame();
        let event = CompactionEvent::from_frame(&frame, CompactionReason::ResumeReplay, "resume");
        let original_event_id = event.event_id;
        let updated = event.clone().with_tail_start_id("seg_42");
        assert_eq!(updated.tail_start_id.as_deref(), Some("seg_42"));
        // The builder must not regenerate the event id or change any
        // other field; replay relies on stable correlation.
        assert_eq!(updated.event_id, original_event_id);
        assert_eq!(updated.tokens_before, event.tokens_before);
        assert_eq!(updated.tokens_after, event.tokens_after);
        assert_eq!(updated.reason, event.reason);
        assert_eq!(updated.summary, event.summary);
    }

    #[test]
    fn event_trait_kind_string_pins_compaction_event() {
        let frame = sample_frame();
        let event = CompactionEvent::from_frame(&frame, CompactionReason::Manual, "summary");
        // INVARIANT: the JSONL discriminator string is
        // `compaction_event` — renaming would break every replay
        // consumer that filters by event_kind.
        assert_eq!(event.event_kind(), "compaction_event");
        assert_eq!(event.event_id(), event.event_id);
    }

    #[test]
    fn event_trait_summary_includes_reason_and_token_delta() {
        let frame = sample_frame();
        let event =
            CompactionEvent::from_frame(&frame, CompactionReason::BudgetPressure, "summary");
        let summary = event.event_summary();
        assert!(
            summary.starts_with("budget_pressure compaction for frame "),
            "summary must lead with the reason string: {summary}"
        );
        assert!(
            summary.ends_with("100 -> 60 token(s)"),
            "summary must show the token-count delta: {summary}"
        );
    }
}
