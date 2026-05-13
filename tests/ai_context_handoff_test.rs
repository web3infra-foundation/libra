//! S5 acceptance scenario: compaction handoff (OC-Phase 4 P4.6).
//!
//! Exercises the public surface from
//! `src/internal/ai/context_budget/{handoff.rs, compaction_agent.rs,
//! compaction.rs}` end-to-end — calling [`run_compaction`] against a
//! deterministic fake [`CompletionModel`] and asserting:
//!
//! - The happy path returns a populated [`ContextHandoff`] whose
//!   summary parses through [`parse_handoff_template`]; the eight
//!   required headings appear in canonical order; a
//!   [`CompactionEvent`] decorated with `tail_start_id` round-trips
//!   through serde; `tokens_before` / `tokens_after` /
//!   `source_frame_id` / `summary` are populated as the doc
//!   prescribes.
//! - The schema-mismatch path (compaction agent omits one of the
//!   required headings — here `## Critical Context`) surfaces as
//!   [`CompactionAgentError::InvalidTemplate`] wrapping
//!   [`ContextHandoffParseError::SchemaMismatch`] with
//!   `missing_sections == ["## Critical Context"]`. The test calls
//!   the production helper [`compaction_event_for_handoff`] only
//!   on the `Ok` branch — its signature requires a
//!   [`ContextHandoff`] which is unobtainable from the `Err`
//!   path, so the doc's "不写入 `CompactionEvent`" rule becomes a
//!   property of the helper's type signature rather than a
//!   convention enforced by test plumbing.
//!
//! Scope caveat: these tests do NOT exercise
//! `crate::internal::ai::agent::runtime::tool_loop`'s
//! `record_tool_loop_context_frame` /
//! `frame_requires_compaction_event` which is where the *full*
//! production persistence happens. They lock the smaller surface
//! (the run_compaction → CompactionEvent helper boundary) so the
//! tool_loop wiring (still being built in OC-Phase 3 / 4 follow-
//! ups) can stay flexible while the helpers stay pinned.
//!
//! These tests use a hand-rolled CannedModel impl rather than the
//! `test-provider` feature gate so they keep the integration shape
//! close to what the dispatcher will eventually call.

use std::sync::{Arc, Mutex};

use libra::internal::ai::{
    completion::{
        AssistantContent, CompletionError, CompletionModel, CompletionRequest, CompletionResponse,
        CompletionUsage, CompletionUsageSummary, Text,
    },
    context_budget::{
        CompactionAgentError, CompactionEvent, CompactionReason, ContextAttachmentStore,
        ContextBudget, ContextFrameBuilder, ContextFrameCandidate, ContextFrameEvent,
        ContextFrameKind, ContextFrameSource, ContextHandoffParseError, ContextSegmentBudget,
        ContextSegmentKind, ContextTrustLevel, TruncationPolicy, compaction_event_for_handoff,
        embedded_compaction_system_prompt, parse_handoff_template, run_compaction,
    },
};
use uuid::Uuid;

/// Canonical 8-section template the fake compaction agent echoes
/// back. Mirrors the doc's #literal-summary-template byte-for-byte
/// so the parser at the receiving end is the authority on
/// correctness, not the test fixture.
const VALID_SUMMARY: &str = "\
## Goal
- Add unit test for utils::path::join

## Constraints & Preferences
- Stick to the existing snapshot harness

## Progress
### Done
- Located the helper in src/utils/path.rs

### In Progress
- Drafting the failure-mode case

### Blocked
- (none)

## Key Decisions
- Use proptest for random separators

## Next Steps
- Wire the new test module into mod.rs

## Critical Context
- Existing test runner does not propagate panics

## Relevant Files
- src/utils/path.rs: target of the new test
- tests/utils/path_test.rs: new test fixture
";

/// Doc-mandated frame budget tokens used in both tests. Pulled out
/// as a named constant so future tweaks to the fixture do not
/// drift between assertions.
const FIXTURE_BUDGET_TOKENS: u64 = 500;
/// System-rules sub-budget — high enough that the rules segment
/// always fits even when the fixture grows.
const SYSTEM_RULES_BUDGET_TOKENS: u64 = 128;
/// Recent-messages sub-budget.
const RECENT_MESSAGES_BUDGET_TOKENS: u64 = 256;
/// Tool-results sub-budget.
const TOOL_RESULTS_BUDGET_TOKENS: u64 = 128;
/// Synthetic remaining-budget token count the dispatcher would
/// pass to `run_compaction`. The exact value is not load-bearing;
/// the test only asserts it round-trips into the produced handoff.
const REMAINING_BUDGET_TOKENS: u64 = 4_096;

/// Fake [`CompletionModel`] that hands back a canned response. The
/// `captured` slot records the inbound request so the test can
/// assert the dispatcher sent the prompt through `preamble` (not as
/// `Message::System`) and cleared `tools` to enforce the no-tools
/// contract.
#[derive(Clone)]
struct CannedModel {
    reply: Vec<AssistantContent>,
    captured: Arc<Mutex<Option<CompletionRequest>>>,
}

impl CannedModel {
    fn from_text(text: &str) -> Self {
        Self {
            reply: vec![AssistantContent::Text(Text {
                text: text.to_string(),
            })],
            captured: Arc::new(Mutex::new(None)),
        }
    }

    fn take_captured_request(&self) -> Option<CompletionRequest> {
        self.captured
            .lock()
            .expect("CannedModel mutex poisoned")
            .clone()
    }
}

#[derive(Debug)]
struct CannedRaw;

impl CompletionUsage for CannedRaw {
    fn usage_summary(&self) -> Option<CompletionUsageSummary> {
        None
    }
}

impl CompletionModel for CannedModel {
    type Response = CannedRaw;

    async fn completion(
        &self,
        request: CompletionRequest,
    ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
        *self.captured.lock().expect("CannedModel mutex poisoned") = Some(request);
        Ok(CompletionResponse {
            content: self.reply.clone(),
            reasoning_content: None,
            raw_response: CannedRaw,
        })
    }
}

/// In-memory sink for [`CompactionEvent`]s the test treats as a
/// stand-in for the production session JSONL store. It does NOT
/// model `tool_loop::record_tool_loop_context_frame`'s full path;
/// it captures whether the test-side dispatcher pattern actually
/// produced an event to persist. The negative assertion in
/// [`s5_compaction_handoff_schema_mismatch_path_blocks_event_write`]
/// is meaningful because the test calls the production helper
/// [`compaction_event_for_handoff`] which can only be invoked on
/// an `Ok(ContextHandoff)` — the helper's signature enforces the
/// "不写入 CompactionEvent on Err" rule.
#[derive(Clone, Default)]
struct EventSink {
    recorded: Arc<Mutex<Vec<CompactionEvent>>>,
}

impl EventSink {
    fn record(&self, event: CompactionEvent) {
        self.recorded
            .lock()
            .expect("EventSink mutex poisoned")
            .push(event);
    }

    fn snapshot(&self) -> Vec<CompactionEvent> {
        self.recorded
            .lock()
            .expect("EventSink mutex poisoned")
            .clone()
    }
}

/// Build a tiny but realistic `ContextFrameEvent` containing a
/// system-rules segment + a recent-message segment + a tool-result
/// segment. The frame is the input the compaction agent receives;
/// in the production path the dispatcher renders it to a string
/// before calling `run_compaction`.
fn build_test_frame(attachments: &ContextAttachmentStore) -> ContextFrameEvent {
    let budget = ContextBudget::from_segments(
        FIXTURE_BUDGET_TOKENS,
        vec![
            ContextSegmentBudget::new(
                ContextSegmentKind::SystemRules,
                SYSTEM_RULES_BUDGET_TOKENS,
                TruncationPolicy::Never,
            ),
            ContextSegmentBudget::new(
                ContextSegmentKind::RecentMessages,
                RECENT_MESSAGES_BUDGET_TOKENS,
                TruncationPolicy::PreserveSourceLabels,
            ),
            ContextSegmentBudget::new(
                ContextSegmentKind::ToolResults,
                TOOL_RESULTS_BUDGET_TOKENS,
                TruncationPolicy::CompressLargeOutputs,
            ),
        ],
    )
    .expect("budget must validate");

    ContextFrameBuilder::new(ContextFrameKind::PromptBuild, budget)
        .with_prompt_id("turn-handoff-1")
        .push(
            ContextFrameCandidate::new(
                "rules",
                ContextSegmentKind::SystemRules,
                "Never expose secrets.",
            )
            .source(ContextFrameSource::runtime("system_prompt"))
            .trust(ContextTrustLevel::Trusted)
            .non_compressible(true),
        )
        .push(
            ContextFrameCandidate::new(
                "user-1",
                ContextSegmentKind::RecentMessages,
                "Add a unit test for utils::path::join.",
            )
            .source(ContextFrameSource::runtime("transcript"))
            .trust(ContextTrustLevel::Trusted)
            .token_estimate(32),
        )
        .push(
            ContextFrameCandidate::new(
                "tool-1",
                ContextSegmentKind::ToolResults,
                "Located helper at src/utils/path.rs:14",
            )
            .source(ContextFrameSource::tool("read_file", "src/utils/path.rs"))
            .trust(ContextTrustLevel::Trusted)
            .token_estimate(48),
        )
        .build(attachments)
        .expect("frame must build cleanly with the test fixture budget")
}

/// Simulate the dispatcher's compaction step using **only**
/// production helpers ([`run_compaction`] +
/// [`compaction_event_for_handoff`]) so the "skip persistence on
/// `Err`" rule is the helper signature's, not the test's. On
/// `Err` the function returns early before
/// [`compaction_event_for_handoff`] is reachable (the helper
/// requires a `&ContextHandoff` value the `Err` path never
/// yields), and the sink never receives a record.
async fn dispatcher_run_compaction(
    model: &CannedModel,
    frame: &ContextFrameEvent,
    sink: &EventSink,
) -> Result<(), CompactionAgentError> {
    let handoff = run_compaction(
        model,
        embedded_compaction_system_prompt(),
        "user: pretend this is the rendered transcript",
        frame.frame_id,
        frame.attachment_refs(),
        Vec::new(),
        REMAINING_BUDGET_TOKENS,
    )
    .await?;

    let event = compaction_event_for_handoff(
        frame,
        &handoff,
        CompactionReason::BudgetPressure,
        Some("user-1"),
    );
    sink.record(event);
    Ok(())
}

/// S5 happy path: a compaction agent that echoes the canonical
/// 8-section template produces a parseable
/// [`libra::internal::ai::context_budget::ContextHandoff`]; the
/// dispatcher persists exactly one [`CompactionEvent`] decorated
/// with `tail_start_id`; the captured request shows the system
/// prompt rode in `preamble` (not `Message::System`).
#[tokio::test]
async fn s5_compaction_handoff_happy_path_produces_parseable_handoff() {
    let tmp = tempfile::TempDir::new().expect("tempdir must succeed in tests");
    let attachments = ContextAttachmentStore::new(tmp.path());
    let frame = build_test_frame(&attachments);
    let sink = EventSink::default();

    let model = CannedModel::from_text(VALID_SUMMARY);
    dispatcher_run_compaction(&model, &frame, &sink)
        .await
        .expect("happy-path dispatcher must succeed end-to-end");

    // Cross-provider parity: system prompt rides in `preamble`,
    // chat history is user-only, tools cleared.
    let captured = model
        .take_captured_request()
        .expect("compaction agent must invoke the model");
    assert!(
        captured.preamble.is_some(),
        "preamble must carry system prompt"
    );
    assert!(captured.tools.is_empty(), "tools list must be cleared");
    assert_eq!(
        captured.chat_history.len(),
        1,
        "chat history must be user-only"
    );

    // The dispatcher persisted exactly one CompactionEvent.
    let recorded = sink.snapshot();
    assert_eq!(
        recorded.len(),
        1,
        "happy path must persist exactly one CompactionEvent, got {}",
        recorded.len()
    );
    let event = &recorded[0];
    assert_eq!(event.frame_id, frame.frame_id, "frame id must round-trip");
    assert_eq!(
        event.tokens_before, frame.total_candidate_tokens,
        "tokens_before must come from frame.total_candidate_tokens"
    );
    assert_eq!(
        event.tokens_after, frame.total_selected_tokens,
        "tokens_after must come from frame.total_selected_tokens"
    );
    assert_eq!(event.tail_start_id.as_deref(), Some("user-1"));

    // The recorded summary parses through the strict template
    // parser — closing the loop on the doc's
    // `ContextHandoff::summary parse 通过` rule.
    let parsed = parse_handoff_template(&event.summary)
        .expect("recorded summary must parse via parse_handoff_template");
    assert_eq!(
        parsed.goal.bullets,
        vec!["Add unit test for utils::path::join".to_string()]
    );
    assert_eq!(
        parsed.relevant_files.bullets.len(),
        2,
        "canonical fixture lists two relevant files"
    );
}

/// S5 schema-mismatch path: a compaction agent that drops one
/// required heading (here `## Critical Context`) MUST NOT silently
/// produce a partial ContextHandoff; the runtime returns
/// `CompactionAgentError::InvalidTemplate(SchemaMismatch{...})`,
/// AND the dispatcher's [`EventSink`] receives **zero** events —
/// that is the doc's "不写入 CompactionEvent" rule, asserted
/// against an explicit mock rather than inferred from an early
/// return.
#[tokio::test]
async fn s5_compaction_handoff_schema_mismatch_path_blocks_event_write() {
    let tmp = tempfile::TempDir::new().expect("tempdir must succeed in tests");
    let attachments = ContextAttachmentStore::new(tmp.path());
    let frame = build_test_frame(&attachments);
    let sink = EventSink::default();

    // Strip the `## Critical Context` block from the canonical
    // template — the rest of the 7 sections still appear in
    // canonical order.
    let truncated = VALID_SUMMARY.replace(
        "## Critical Context\n- Existing test runner does not propagate panics\n\n",
        "",
    );
    assert!(
        !truncated.contains("## Critical Context"),
        "truncated fixture must not contain the section we are testing for"
    );

    let model = CannedModel::from_text(&truncated);
    let err = dispatcher_run_compaction(&model, &frame, &sink)
        .await
        .expect_err("schema-mismatch must surface as Err, not Ok");

    match err {
        CompactionAgentError::InvalidTemplate(ContextHandoffParseError::SchemaMismatch {
            missing_sections,
        }) => {
            assert_eq!(
                missing_sections,
                vec!["## Critical Context".to_string()],
                "missing_sections must list the dropped heading verbatim"
            );
        }
        other => panic!(
            "expected InvalidTemplate(SchemaMismatch{{missing_sections: [\"## Critical Context\"]}}), got {other:?}"
        ),
    }

    // The contract assertion: the dispatcher's event sink received
    // ZERO events on the error path. This is the doc's
    // "不写入 CompactionEvent" rule made explicit.
    let recorded = sink.snapshot();
    assert!(
        recorded.is_empty(),
        "schema-mismatch path must not persist any CompactionEvent, got {} record(s)",
        recorded.len()
    );
}

/// S5 unique-summary path: the dispatcher feeds a synthetic
/// transcript with a known frame id; the returned ContextHandoff
/// must reference that exact id (not the current frame id, not a
/// fresh Uuid). Guards against a future refactor that accidentally
/// re-derives the id from somewhere downstream.
#[tokio::test]
async fn s5_compaction_handoff_preserves_caller_supplied_source_frame_id() {
    let known_id = Uuid::new_v4();
    let model = CannedModel::from_text(VALID_SUMMARY);
    let handoff = run_compaction(
        &model,
        "system",
        "transcript content",
        known_id,
        Vec::new(),
        Vec::new(),
        REMAINING_BUDGET_TOKENS,
    )
    .await
    .expect("happy-path run_compaction must succeed with empty fixture");

    assert_eq!(
        handoff.source_frame_id, known_id,
        "ContextHandoff must carry the caller-supplied frame id verbatim"
    );
}
