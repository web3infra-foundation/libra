//! Integration tests for [`filter_compacted`], the OC-Phase 4 P4.5
//! projection that implements PR #25851's compaction reorder rule.
//!
//! Doc reference:
//! `docs/improvement/opencode.md#filterCompacted-等价函数`. The doc
//! prescribes three reordering scenarios via a table; the unit
//! tests in `src/internal/ai/context_budget/projection.rs` cover
//! those scenarios on hand-built [`MessageProjection`] fixtures,
//! while this integration test covers the **dispatcher path**:
//! converting a persisted [`CompactionEvent`] (with `tail_start_id`
//! set per P4.5 schema migration) into the projection input the
//! reorder rule consumes.
//!
//! Why a dedicated file: OC-Phase 4 acceptance scenario S5 names
//! this test path explicitly. Keeping filter scenarios separate
//! from the prune scenarios in
//! `tests/ai_context_compaction_prune_test.rs` mirrors the
//! upstream split between `session/compaction.ts` (prune) and
//! `session/message-v2.ts` (filterCompacted).

use libra::internal::ai::context_budget::{
    CompactionEvent, CompactionReason, ContextAttachmentStore, ContextBudget, ContextFrameBuilder,
    ContextFrameCandidate, ContextFrameKind, ContextFrameSource, ContextSegmentBudget,
    ContextSegmentKind, ContextTrustLevel, MessageProjection, ProjectionKind, TruncationPolicy,
    compaction_event_to_projection, filter_compacted,
};

/// Build a tiny ContextFrameEvent so a real CompactionEvent (with
/// tokens_before / tokens_after / frame_id wired) can be derived
/// via `CompactionEvent::from_frame`. Keeps the filter test honest
/// — the input projection pulls its compaction id and tail pointer
/// from a frame-derived event, not from a literal struct.
fn build_minimal_frame(
    attachments: &ContextAttachmentStore,
) -> libra::internal::ai::context_budget::ContextFrameEvent {
    let budget = ContextBudget::from_segments(
        500,
        vec![ContextSegmentBudget::new(
            ContextSegmentKind::RecentMessages,
            128,
            TruncationPolicy::PreserveSourceLabels,
        )],
    )
    .expect("budget must validate");
    ContextFrameBuilder::new(ContextFrameKind::PromptBuild, budget)
        .with_prompt_id("turn-filter-1")
        .push(
            ContextFrameCandidate::new(
                "user-1",
                ContextSegmentKind::RecentMessages,
                "Implement a sort function.",
            )
            .source(ContextFrameSource::runtime("transcript"))
            .trust(ContextTrustLevel::Trusted)
            .token_estimate(32),
        )
        .build(attachments)
        .expect("frame must build cleanly")
}

// Conversion CompactionEvent → MessageProjection is now a
// production helper at
// `libra::internal::ai::context_budget::compaction_event_to_projection`.
// Tests below call that helper directly so the id-space contract
// (event UUID stringified) is locked in production code, not in
// test-only plumbing.

/// Doc table scenario 1 with **real** CompactionEvent input: a
/// transcript whose compaction marker comes from a frame-derived
/// `CompactionEvent::from_frame(...).with_tail_start_id(...)` call,
/// not a literal MessageProjection. The reorder rule must pair the
/// compaction's event id with a Summary message whose
/// parent_compaction_id matches it, and emit the
/// `[marker, summary, tail, post]` ordering per PR #25851.
#[test]
fn filter_compacted_uses_real_compaction_event_id_to_pair_summary() {
    let tmp = tempfile::TempDir::new().expect("tempdir must succeed in tests");
    let attachments = ContextAttachmentStore::new(tmp.path());
    let frame = build_minimal_frame(&attachments);
    let compaction_event = CompactionEvent::from_frame(
        &frame,
        CompactionReason::BudgetPressure,
        "summary text body".to_string(),
    )
    .with_tail_start_id("user-pre-1");
    let comp_id = compaction_event.event_id.to_string();

    let msgs = vec![
        MessageProjection::new("user-pre-0", ProjectionKind::User),
        MessageProjection::new("user-pre-1", ProjectionKind::Assistant),
        compaction_event_to_projection(&compaction_event),
        MessageProjection::new(
            "summary-1",
            ProjectionKind::Summary {
                parent_compaction_id: comp_id.clone(),
            },
        ),
        MessageProjection::new("user-post-0", ProjectionKind::User),
        MessageProjection::new("assistant-post-0", ProjectionKind::Assistant),
    ];

    let out = filter_compacted(&msgs);
    let observed_ids: Vec<&str> = out.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(
        observed_ids,
        vec![
            comp_id.as_str(),
            "summary-1",
            "user-pre-1",
            "user-post-0",
            "assistant-post-0",
        ],
        "reorder must place [marker, summary, tail, post] per PR #25851 even when the marker id comes from a real CompactionEvent"
    );
}

/// Doc table scenario 2 with real CompactionEvent input: when the
/// CompactionEvent persisted with `tail_start_id = None` (i.e. the
/// compaction kept no retained tail), the reorder rule does NOT
/// reshuffle anything — the model gets chronological order. This
/// is the documented "无 tail，保持原序" behaviour.
#[test]
fn filter_compacted_with_real_compaction_event_no_tail_preserves_order() {
    let tmp = tempfile::TempDir::new().expect("tempdir must succeed in tests");
    let attachments = ContextAttachmentStore::new(tmp.path());
    let frame = build_minimal_frame(&attachments);
    // tail_start_id stays None — `from_frame` defaults to None and
    // we deliberately skip the builder helper here.
    let compaction_event = CompactionEvent::from_frame(
        &frame,
        CompactionReason::BudgetPressure,
        "summary text body".to_string(),
    );
    assert!(compaction_event.tail_start_id.is_none());
    let comp_id = compaction_event.event_id.to_string();

    let msgs = vec![
        MessageProjection::new("user-0", ProjectionKind::User),
        MessageProjection::new("user-1", ProjectionKind::Assistant),
        compaction_event_to_projection(&compaction_event),
        MessageProjection::new(
            "summary-1",
            ProjectionKind::Summary {
                parent_compaction_id: comp_id.clone(),
            },
        ),
    ];

    let out = filter_compacted(&msgs);
    let observed_ids: Vec<&str> = out.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(
        observed_ids,
        vec!["user-0", "user-1", comp_id.as_str(), "summary-1"],
        "no tail means no reorder — chronological order preserved"
    );
}

/// Doc table scenario 3 with real CompactionEvent input: a Summary
/// message whose `parent_compaction_id` does NOT match any
/// post-compaction event triggers the defensive fallback —
/// chronological order is preserved without panic. This guards
/// against a transcript where the dispatcher's Summary linkage is
/// inconsistent (e.g. message-v2 schema migration left a stale
/// parent id).
#[test]
fn filter_compacted_with_mismatched_summary_parent_falls_back_to_chronological() {
    let tmp = tempfile::TempDir::new().expect("tempdir must succeed in tests");
    let attachments = ContextAttachmentStore::new(tmp.path());
    let frame = build_minimal_frame(&attachments);
    let compaction_event = CompactionEvent::from_frame(
        &frame,
        CompactionReason::BudgetPressure,
        "summary text body".to_string(),
    )
    .with_tail_start_id("user-1");
    let comp_id = compaction_event.event_id.to_string();

    let msgs = vec![
        MessageProjection::new("user-0", ProjectionKind::User),
        MessageProjection::new("user-1", ProjectionKind::Assistant),
        compaction_event_to_projection(&compaction_event),
        MessageProjection::new(
            "summary-1",
            ProjectionKind::Summary {
                parent_compaction_id: "completely-different-id".to_string(),
            },
        ),
    ];

    let out = filter_compacted(&msgs);
    let observed_ids: Vec<&str> = out.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(
        observed_ids,
        vec!["user-0", "user-1", comp_id.as_str(), "summary-1"],
        "mismatched summary parent must trigger fallback to chronological order"
    );
}
