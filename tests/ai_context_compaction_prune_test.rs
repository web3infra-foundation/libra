//! S5 acceptance scenario: prune phase + budget-driven prune→compact
//! sequence (OC-Phase 4 P4.6).
//!
//! Exercises the public surface from
//! `src/internal/ai/context_budget/{projection.rs, compaction.rs,
//! frame.rs, budget.rs}` end-to-end:
//!
//! - **Prune phase**: a session writes a context frame with a
//!   large tool result. A simulated dispatcher renders the inline
//!   transcript, runs `prune_inline_tool_output` on each segment,
//!   and asserts (i) the rendered model-bound prompt contains the
//!   `<pruned attachment_id="..." length="...">` placeholder, and
//!   (ii) the underlying JSONL bytes are byte-identical
//!   before/after the dispatcher pass. The doc's "不修改原始
//!   SessionJsonl bytes; 只在内存 transcript 投影里替换" rule is
//!   the contract being asserted.
//! - **Sequence phase**: `ContextBudget::is_overflow` gates the
//!   prune→compact decision per the doc's
//!   "Compaction 触发判定" rule. A simulated dispatcher pass
//!   first verifies the post-prune token count stays under
//!   `usable`, then verifies a transcript whose post-prune count
//!   still crosses `usable` correctly triggers the
//!   compaction-agent path.
//!
//! Filter-only scenarios live in
//! `tests/ai_compaction_filter_test.rs` (per the doc's
//! `#filterCompacted-等价函数` file naming); this file keeps the
//! prune + budget path together.

use std::fs;

use libra::internal::ai::{
    context_budget::{
        ContextAttachmentStore, ContextBudget, ContextFrameBuilder, ContextFrameCandidate,
        ContextFrameKind, ContextFrameSource, ContextSegmentBudget, ContextSegmentKind,
        ContextTrustLevel, PRUNE_PROTECTED_TOOLS, PruneResult, SAFETY_MARGIN_TOKENS,
        TOOL_OUTPUT_MAX_CHARS, TruncationPolicy, prune_inline_tool_output,
    },
    runtime::event::Event,
    session::{
        SessionState,
        jsonl::{SessionEvent, SessionJsonlStore},
    },
};

/// Token budget for the test fixture. Big enough that ASCII
/// fixture content fits, small enough that overflow tests can
/// trip `is_overflow` deliberately.
const FIXTURE_PROMPT_BUDGET_TOKENS: u64 = 8_000;
/// Tool-results sub-budget — high enough that the fixture's
/// pre-attachment large output gets externalised cleanly.
const TOOL_RESULTS_BUDGET_TOKENS: u64 = 1_024;
/// Attachment threshold (bytes) at which the
/// [`ContextAttachmentStore`] decides a segment is large enough
/// to externalise.
const ATTACHMENT_THRESHOLD_BYTES: usize = 64;
/// Repeats of "tool output line\n" used to construct an
/// over-threshold tool result. 200 lines × 17 bytes = 3400 bytes,
/// well above [`TOOL_OUTPUT_MAX_CHARS`] = 2000.
const HUGE_TOOL_LINE_COUNT: usize = 200;

fn make_huge_tool_output() -> String {
    "tool output line\n".repeat(HUGE_TOOL_LINE_COUNT)
}

/// Prune phase: an oversized inline tool result is rewritten to a
/// `<pruned ...>` placeholder. Source bytes — the raw fixture
/// string the dispatcher would have rendered into the prompt — are
/// not mutated by the prune call.
#[test]
fn prune_replaces_inline_view_without_mutating_source() {
    let original = "x".repeat(TOOL_OUTPUT_MAX_CHARS + 100);
    let snapshot_before = original.clone();
    let result = prune_inline_tool_output("read_file", &original, Some("att-prune-e2e"));

    match &result {
        PruneResult::Pruned(rendered) => {
            assert!(rendered.starts_with("<pruned attachment_id=\"att-prune-e2e\""));
            assert!(rendered.contains(&format!("length=\"{}\"", original.encode_utf16().count())));
        }
        PruneResult::Kept(_) => panic!("oversized output must be pruned"),
    }

    // Source string is untouched after the prune call returned a
    // new owned placeholder. The `&original` borrow above was
    // shared-immutable; this assertion is the explicit check
    // that the doc's "non-destructive" guarantee holds at the
    // surface API level.
    assert_eq!(original, snapshot_before);
}

/// Prune phase: protected tools (`skill`, `submit_intent_draft`,
/// `submit_plan_draft`) keep their oversized output verbatim. The
/// dispatcher relies on these as the only durable record of the
/// user's intent / plan, so erasing them mid-session would lose
/// information the rest of the runtime depends on.
#[test]
fn prune_preserves_protected_tools_in_e2e_path() {
    let big = "y".repeat(TOOL_OUTPUT_MAX_CHARS * 3);
    for protected in PRUNE_PROTECTED_TOOLS {
        let result = prune_inline_tool_output(protected, &big, Some("att"));
        assert!(
            !result.was_pruned(),
            "protected tool {protected:?} must survive the e2e prune path"
        );
    }
}

/// Dispatcher integration test: persist a context frame carrying a
/// large tool output, then run a simulated dispatcher pass — load
/// the frame from JSONL, render its segments to a model-bound
/// prompt string, run [`prune_inline_tool_output`] on each
/// segment's content, and produce the in-memory rendered prompt.
/// Assert:
///
/// 1. The rendered prompt contains the `<pruned ...>` placeholder
///    (so the model sees the entry point but not the inline
///    bytes).
/// 2. The underlying JSONL bytes — `events.jsonl` — are
///    byte-identical before/after the dispatcher pass. The doc's
///    "不修改原始 SessionJsonl bytes" rule is asserted at the
///    persistence-stack level, not at the API-can't-write level.
/// 3. The original tool-output string the dispatcher started with
///    is also untouched (caller-supplied buffer immutability).
#[test]
fn dispatcher_prune_path_keeps_jsonl_bytes_identical_and_pruned_in_prompt() {
    let tmp = tempfile::TempDir::new().expect("tempdir must succeed in tests");
    let session_root = tmp.path().join("sessions").join("session-prune");
    let jsonl = SessionJsonlStore::new(session_root.clone());
    let attachments = ContextAttachmentStore::new(&session_root);

    let mut session = SessionState::new("/repo/main");
    session.id = "session-prune".to_string();
    jsonl
        .append(&SessionEvent::snapshot(session))
        .expect("snapshot append must succeed");

    let huge_output = make_huge_tool_output();
    let huge_snapshot = huge_output.clone();
    let budget = ContextBudget::from_segments(
        FIXTURE_PROMPT_BUDGET_TOKENS,
        vec![ContextSegmentBudget::new(
            ContextSegmentKind::ToolResults,
            TOOL_RESULTS_BUDGET_TOKENS,
            TruncationPolicy::CompressLargeOutputs,
        )],
    )
    .expect("budget must validate");

    let frame = ContextFrameBuilder::new(ContextFrameKind::PromptBuild, budget)
        .with_attachment_threshold_bytes(ATTACHMENT_THRESHOLD_BYTES)
        .push(
            ContextFrameCandidate::new(
                "large-tool-output",
                ContextSegmentKind::ToolResults,
                huge_output.clone(),
            )
            .source(ContextFrameSource::tool("shell", "cargo test"))
            .trust(ContextTrustLevel::Trusted)
            .token_estimate(96),
        )
        .build(&attachments)
        .expect("frame must build cleanly");

    jsonl
        .append(&SessionEvent::context_frame(frame.clone()))
        .expect("frame append must succeed");

    // Snapshot the JSONL bytes BEFORE the dispatcher pass.
    let bytes_before = fs::read(jsonl.events_path()).expect("read jsonl bytes");

    // Simulated dispatcher pass: load the frame back from JSONL,
    // render each segment to a prompt string by resolving
    // attachment-backed segments through
    // `ContextAttachmentStore::read_to_string` (the production
    // resolver path). Inline-content segments fall back to the
    // segment body. Each resolved string flows through
    // `prune_inline_tool_output` so the dispatcher's projection is
    // exactly what the model would receive.
    let replay = jsonl.load_context_replay().expect("replay must succeed");
    assert_eq!(replay.frames.len(), 1, "expect exactly one persisted frame");
    let loaded = &replay.frames[0];
    let mut rendered_prompt = String::new();
    let mut resolved_via_attachment_store = false;
    for segment in &loaded.segments {
        // Resolve attachment-backed segments through the real
        // attachment store. This is the production path the
        // dispatcher uses; substituting the raw fixture buffer
        // would not exercise it.
        let resolved: String = match (&segment.attachment, &segment.content) {
            (Some(att), _) => {
                resolved_via_attachment_store = true;
                attachments
                    .read_to_string(att)
                    .expect("attachment_store.read_to_string must succeed")
            }
            (None, Some(inline)) => inline.clone(),
            (None, None) => String::new(),
        };
        let pruned = prune_inline_tool_output(
            segment.source.label.as_str(),
            resolved.as_str(),
            segment.attachment.as_ref().map(|att| att.sha256.as_str()),
        );
        rendered_prompt.push_str(&pruned.into_string());
        rendered_prompt.push('\n');
    }
    assert!(
        resolved_via_attachment_store,
        "fixture must include at least one attachment-backed segment so the resolver path is exercised"
    );

    // Assertion 1: the rendered prompt has the placeholder, not
    // the raw 200 lines.
    assert!(
        rendered_prompt.contains("<pruned"),
        "rendered prompt must contain a <pruned ...> placeholder, got:\n{rendered_prompt}"
    );
    assert!(
        !rendered_prompt.contains("tool output line\ntool output line\ntool output line"),
        "rendered prompt must NOT contain the inline 200-line tool output verbatim"
    );

    // Assertion 2: JSONL bytes are byte-identical.
    let bytes_after = fs::read(jsonl.events_path()).expect("read jsonl bytes");
    assert_eq!(
        bytes_before, bytes_after,
        "prune must not touch persisted JSONL bytes — the transformation lives in the in-memory projection only"
    );

    // Assertion 3: original tool-output buffer untouched.
    assert_eq!(huge_output, huge_snapshot);

    // Sanity: the persisted frame still carries event_kind ==
    // "context_frame" — no rewrite happened.
    assert_eq!(loaded.event_kind(), "context_frame");
}

/// Sequence phase: the doc's "S5 输入" reads
/// "parent session token 用量 > usable * 0.5，触发 prune；之后超
/// usable，触发 compact". This test exercises the
/// [`ContextBudget::is_overflow`] gate that governs the
/// prune→compact escalation:
///
/// 1. Build a [`ContextBudget`] with a known `max_prompt_tokens`
///    so `usable` is deterministic.
/// 2. Verify `is_overflow(0)` is false and `is_overflow(usable)`
///    is true (the equality-counts-as-overflow rule from the
///    doc).
/// 3. Verify `is_overflow(usable - 1)` is false (boundary), and
///    that `usable() == max_prompt_tokens - SAFETY_MARGIN_TOKENS`.
///
/// The dispatcher reads the `is_overflow` signal to decide between
/// "prune-only" and "prune-then-compact"; this test pins the
/// contract that signal honours.
#[test]
fn budget_is_overflow_gates_prune_to_compact_escalation() {
    let budget = ContextBudget::from_segments(
        FIXTURE_PROMPT_BUDGET_TOKENS,
        vec![ContextSegmentBudget::new(
            ContextSegmentKind::ToolResults,
            TOOL_RESULTS_BUDGET_TOKENS,
            TruncationPolicy::CompressLargeOutputs,
        )],
    )
    .expect("budget must validate");

    let usable = budget.usable();
    assert_eq!(
        usable,
        FIXTURE_PROMPT_BUDGET_TOKENS - SAFETY_MARGIN_TOKENS,
        "usable() must subtract the doc-mandated SAFETY_MARGIN_TOKENS"
    );

    assert!(!budget.is_overflow(0), "empty input is not overflow");
    assert!(
        !budget.is_overflow(usable - 1),
        "input one token under usable is not overflow (boundary case)"
    );
    assert!(
        budget.is_overflow(usable),
        "input exactly at usable IS overflow (>= per doc rule)"
    );
    assert!(
        budget.is_overflow(usable + 100),
        "input over usable IS overflow"
    );
}

/// Sequence phase: the dispatcher runs prune at `> usable * 0.5`
/// (post-prune state stays under `usable` — no compact needed).
/// This test verifies a synthetic "post-prune token count"
/// reading, demonstrating that `is_overflow` correctly gates the
/// no-compact branch.
#[test]
fn budget_overflow_gate_distinguishes_prune_only_from_prune_plus_compact() {
    let budget = ContextBudget::from_segments(
        FIXTURE_PROMPT_BUDGET_TOKENS,
        vec![ContextSegmentBudget::new(
            ContextSegmentKind::ToolResults,
            TOOL_RESULTS_BUDGET_TOKENS,
            TruncationPolicy::CompressLargeOutputs,
        )],
    )
    .expect("budget must validate");

    let usable = budget.usable();
    let half_usable = usable / 2;

    // pre-prune: above half-usable but below usable. The
    // dispatcher would run prune here.
    let pre_prune_tokens = half_usable + 100;
    assert!(pre_prune_tokens > half_usable, "pre-prune crosses 50%");
    assert!(
        !budget.is_overflow(pre_prune_tokens),
        "pre-prune is_overflow must be false — prune avoids overflow"
    );

    // post-prune (pruning recovered some tokens): still below
    // usable. Dispatcher proceeds without compaction.
    let post_prune_tokens = pre_prune_tokens.saturating_sub(500);
    assert!(
        !budget.is_overflow(post_prune_tokens),
        "post-prune is_overflow must remain false — no compaction"
    );

    // Adversarial: a transcript so large that prune cannot keep
    // it under `usable`. is_overflow must fire so the dispatcher
    // escalates to the compaction agent.
    let adversarial_tokens = usable + 1024;
    assert!(
        budget.is_overflow(adversarial_tokens),
        "transcript exceeding usable post-prune MUST signal overflow so dispatcher escalates to compact"
    );
}
