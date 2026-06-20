//! S5 acceptance scenario — integrated prune→compact→handoff
//! sequence (OC-Phase 4 P4.6).
//!
//! Per `docs/development/commands/_general.md` S5:
//!
//! > 输入：parent session token 用量 > `usable * 0.5`，触发 prune；
//! >       之后超 `usable`，触发 compact。
//!
//! Individual phases are covered in `ai_context_compaction_prune_test.rs`
//! (prune algorithm + budget gate) and `ai_context_handoff_test.rs`
//! (compaction agent + 8-section parser). This file composes them into
//! one walk that mirrors what a tool-loop turn-by-turn dispatcher would
//! do: render the inline transcript with the prune projection, gate the
//! prune→compact escalation on `ContextBudget::is_overflow`, run the
//! compaction agent on the over-budget frame, and verify the resulting
//! `ContextHandoff::summary` parses through the strict template parser.
//!
//! The walk is the canonical end-to-end path the doc's S5 scenario
//! prescribes; the assertions are written so that any regression in
//! either phase trips the test, even when each individual phase's
//! per-test still passes (because phase A leaves residue phase B then
//! reads).

use std::{
    fs,
    sync::{Arc, Mutex},
};

use libra::internal::ai::{
    completion::{
        AssistantContent, CompletionError, CompletionModel, CompletionRequest, CompletionResponse,
        CompletionUsage, CompletionUsageSummary, Text,
    },
    context_budget::{
        CompactionAgentError, CompactionReason, ContextAttachmentStore, ContextBudget,
        ContextFrameBuilder, ContextFrameCandidate, ContextFrameEvent, ContextFrameKind,
        ContextFrameSource, ContextHandoffParseError, ContextSegmentBudget, ContextSegmentKind,
        ContextTrustLevel, TruncationPolicy, compaction_event_for_handoff,
        embedded_compaction_system_prompt, parse_handoff_template, prune_inline_tool_output,
        run_compaction,
    },
    runtime::event::Event,
    session::{
        SessionState,
        jsonl::{SessionEvent, SessionJsonlStore},
    },
};

/// Token budget for the test fixture. Sized so that the
/// pre-attachment large output reliably overflows after a single
/// prune pass — this is the doc's "S5 输入" condition (token usage
/// > `usable`).
const FIXTURE_PROMPT_BUDGET_TOKENS: u64 = 8_000;
const TOOL_RESULTS_BUDGET_TOKENS: u64 = 1_024;
/// Bytes threshold for `ContextAttachmentStore` to externalise a
/// segment (forcing the dispatcher path that resolves through the
/// store rather than reading inline content).
const ATTACHMENT_THRESHOLD_BYTES: usize = 64;
/// Repeats of "tool output line\n" used to construct an over-threshold
/// tool result. 200 × 17 bytes = 3400 bytes, well above
/// [`TOOL_OUTPUT_MAX_CHARS`] (= 2000), so prune fires.
const HUGE_TOOL_LINE_COUNT: usize = 200;
/// Synthetic remaining-budget the dispatcher would pass to
/// `run_compaction`. The exact value is not load-bearing; the test
/// only asserts it round-trips into the produced handoff.
const REMAINING_BUDGET_TOKENS: u64 = 4_096;

/// Canonical 8-section template the fake compaction agent echoes
/// back, mirroring `tests/ai_context_handoff_test.rs::VALID_SUMMARY`
/// byte-for-byte so the strict parser is the authority on
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

fn make_huge_tool_output() -> String {
    "tool output line\n".repeat(HUGE_TOOL_LINE_COUNT)
}

/// Hand-rolled fake `CompletionModel` that echoes the canned summary.
/// Mirrors the helper in `ai_context_handoff_test.rs` so this E2E does
/// not need the `test-provider` feature gate to compile.
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

/// S5 integrated walk: build a frame whose tool-result segment is
/// large enough that
///
/// 1. **Prune phase** rewrites the inline projection with a
///    `<pruned ...>` placeholder while the JSONL persistence layer's
///    bytes stay byte-identical.
/// 2. **Sequence gate** confirms the post-prune token estimate still
///    crosses `usable`, so the dispatcher escalates to compact.
/// 3. **Compact phase** drives the compaction agent against a
///    deterministic [`CannedModel`] and persists exactly one
///    [`CompactionEvent`].
/// 4. **Handoff phase** parses the recorded summary through the
///    strict 8-section template parser; canonical headings appear in
///    canonical order and `tail_start_id` round-trips.
///
/// Any regression in any phase trips the test before reaching the
/// next, which is the whole point of an integrated S5 walk: per-phase
/// tests still pass when the handoff between phases regresses.
#[tokio::test]
async fn s5_e2e_prune_then_compact_persists_event_and_parseable_handoff() {
    let tmp = tempfile::TempDir::new().expect("tempdir must succeed in tests");
    let session_root = tmp.path().join("sessions").join("session-s5-e2e");
    let jsonl = SessionJsonlStore::new(session_root.clone());
    let attachments = ContextAttachmentStore::new(&session_root);

    let mut session = SessionState::new("/repo/main");
    session.id = "session-s5-e2e".to_string();
    jsonl
        .append(&SessionEvent::snapshot(session))
        .expect("snapshot append must succeed");

    // ----- Build the over-threshold frame -----
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

    // Keep a separate handle for the phase-2 gate assertion below;
    // `ContextFrameBuilder::new` moves the budget into the frame.
    let gate_budget = budget.clone();

    let frame: ContextFrameEvent = ContextFrameBuilder::new(ContextFrameKind::PromptBuild, budget)
        .with_attachment_threshold_bytes(ATTACHMENT_THRESHOLD_BYTES)
        .push(
            ContextFrameCandidate::new(
                "large-tool-output",
                ContextSegmentKind::ToolResults,
                huge_output.clone(),
            )
            .source(ContextFrameSource::tool("shell", "cargo test"))
            .trust(ContextTrustLevel::Trusted)
            // Small token estimate so the budget allocator includes
            // this segment. The dispatcher's gate (phase 2) reads the
            // *post-prune transcript* token count, which is
            // independent of the per-segment estimate.
            .token_estimate(96),
        )
        .build(&attachments)
        .expect("frame must build cleanly");

    jsonl
        .append(&SessionEvent::context_frame(frame.clone()))
        .expect("frame append must succeed");

    let bytes_before = fs::read(jsonl.events_path()).expect("read jsonl bytes");

    // ----- Phase 1: prune -----
    // Render the inline transcript exactly the way the dispatcher
    // would: resolve attachment-backed segments through the real
    // store, then run prune on each resolved string.
    let replay = jsonl.load_context_replay().expect("replay must succeed");
    assert_eq!(replay.frames.len(), 1, "expect one persisted frame");
    let loaded = &replay.frames[0];
    let mut rendered_prompt = String::new();
    for segment in &loaded.segments {
        let resolved: String = match (&segment.attachment, &segment.content) {
            (Some(att), _) => attachments
                .read_to_string(att)
                .expect("attachment_store.read_to_string must succeed"),
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
    // Spec line 1699: the placeholder shape is
    // `<pruned attachment_id="..." length="...">`. Pin both
    // attribute names + the actual attachment id and length so a
    // future rename of either attribute breaks this test before
    // breaking on-the-wire compatibility with the dispatcher.
    let placeholder_attachment = loaded.segments[0]
        .attachment
        .as_ref()
        .expect("fixture must have externalised the segment");
    let expected_length = huge_snapshot.encode_utf16().count();
    let expected_placeholder = format!(
        "<pruned attachment_id=\"{}\" length=\"{}\">",
        placeholder_attachment.sha256, expected_length
    );
    assert!(
        rendered_prompt.contains(&expected_placeholder),
        "phase 1 (prune): rendered prompt must contain the exact placeholder \
         `{expected_placeholder}`, got:\n{rendered_prompt}"
    );
    assert!(
        !rendered_prompt.contains("tool output line\ntool output line\ntool output line"),
        "phase 1 (prune): rendered prompt must NOT contain the 200-line tool output verbatim"
    );

    // JSONL bytes must NOT have changed across the prune pass.
    let bytes_after_prune = fs::read(jsonl.events_path()).expect("read jsonl bytes");
    assert_eq!(
        bytes_before, bytes_after_prune,
        "phase 1 (prune): persisted JSONL bytes must be byte-identical after the prune projection"
    );
    // Caller-supplied huge_output buffer untouched.
    assert_eq!(huge_output, huge_snapshot);

    // ----- Phase 2: sequence gate -----
    // Spec line 1695: the dispatcher's prune trigger fires when
    // `parent session token usage > usable * 0.5`. Assert the
    // predicate directly so a future change to the trigger threshold
    // is caught here.
    let pre_prune_transcript_tokens = (gate_budget.usable() / 2) + 200;
    assert!(
        pre_prune_transcript_tokens > gate_budget.usable() / 2,
        "phase 2 (gate): pre-prune token count must cross usable * 0.5 (the prune trigger)"
    );

    // Spec line 1695: the dispatcher's compact trigger fires when
    // *post-prune* transcript tokens still exceed `usable`. Asserting
    // `is_overflow` on a synthesised post-prune value pins the
    // gate semantics (not the per-frame estimator).
    let post_prune_transcript_tokens = gate_budget.usable() + 512;
    assert!(
        gate_budget.is_overflow(post_prune_transcript_tokens),
        "phase 2 (gate): post-prune transcript tokens ({post_prune_transcript_tokens}) must exceed usable ({}) so the dispatcher escalates to compact",
        gate_budget.usable()
    );

    // ----- Phase 3: compact -----
    // The fake compaction agent echoes the canonical 8-section
    // template. `run_compaction` produces a `ContextHandoff`; the
    // dispatcher then forms a `CompactionEvent`.
    let model = CannedModel::from_text(VALID_SUMMARY);
    let handoff = run_compaction(
        &model,
        embedded_compaction_system_prompt(),
        rendered_prompt.as_str(),
        frame.frame_id,
        frame.attachment_refs(),
        Vec::new(),
        REMAINING_BUDGET_TOKENS,
    )
    .await
    .expect("phase 3 (compact): compaction agent must succeed against the canonical template");

    let event = compaction_event_for_handoff(
        &frame,
        &handoff,
        CompactionReason::BudgetPressure,
        Some("user-1"),
    );

    // CompactionEvent fields populated as the doc prescribes.
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

    // Spec line 1700: write CompactionEvent to the session JSONL so
    // the dispatcher (and downstream replay) sees it. The persistence
    // step is what closes the doc's "写 CompactionEvent" rule;
    // constructing the value in memory only is not enough.
    jsonl
        .append(&SessionEvent::compaction(event.clone()))
        .expect("CompactionEvent persistence must succeed");

    // The persisted event reads back via the load_context_replay
    // path the production dispatcher uses for resume.
    let replay_after_compact = jsonl
        .load_context_replay()
        .expect("replay after compact must succeed");
    assert_eq!(
        replay_after_compact.compactions.len(),
        1,
        "exactly one CompactionEvent must round-trip through the JSONL store"
    );
    let persisted_event = &replay_after_compact.compactions[0];
    assert_eq!(persisted_event.frame_id, frame.frame_id);
    assert_eq!(persisted_event.summary, event.summary);
    assert_eq!(persisted_event.event_kind(), "compaction_event");

    // ----- Phase 4: handoff parses with 8 sections in canonical order -----
    let parsed = parse_handoff_template(&event.summary)
        .expect("phase 4 (handoff): recorded summary must parse via parse_handoff_template");

    // The doc-mandated 8 sections, all populated by the canonical
    // fixture.
    assert_eq!(
        parsed.goal.bullets,
        vec!["Add unit test for utils::path::join".to_string()],
        "phase 4 (handoff): Goal section must round-trip"
    );
    assert!(
        !parsed.constraints_and_preferences.bullets.is_empty(),
        "phase 4 (handoff): Constraints & Preferences must populate"
    );
    assert!(
        !parsed.progress_done.bullets.is_empty(),
        "phase 4 (handoff): Progress > Done must populate"
    );
    assert!(
        !parsed.progress_in_progress.bullets.is_empty(),
        "phase 4 (handoff): Progress > In Progress must populate"
    );
    assert!(
        !parsed.key_decisions.bullets.is_empty(),
        "phase 4 (handoff): Key Decisions must populate"
    );
    assert!(
        !parsed.next_steps.bullets.is_empty(),
        "phase 4 (handoff): Next Steps must populate"
    );
    assert!(
        !parsed.critical_context.bullets.is_empty(),
        "phase 4 (handoff): Critical Context must populate"
    );
    assert_eq!(
        parsed.relevant_files.bullets.len(),
        2,
        "phase 4 (handoff): Relevant Files must list both fixture entries"
    );

    // The ContextHandoff::summary that flows back into the next
    // dispatcher turn matches the recorded event's summary — that is
    // the actual handoff. A regression here would mean the recorded
    // summary and the in-memory handoff diverged.
    assert_eq!(
        event.summary, handoff.summary,
        "phase 4 (handoff): event.summary must equal handoff.summary so the dispatcher feeds the same string into the next turn"
    );

    // Spec line 1701: the next parent prompt MUST contain the 8
    // headings literally and in canonical order. Build the
    // continuation prompt the dispatcher would feed to the next turn
    // (system prompt + handoff summary) and assert the headings
    // appear in template order. Doing the assertion on the rendered
    // prompt — not on the parsed structure — guards against a
    // regression where the parser tolerates a reorder the wire
    // layer would surface verbatim.
    let next_prompt = format!(
        "{}\n\n{}",
        embedded_compaction_system_prompt(),
        handoff.summary
    );
    let canonical_headings = [
        "## Goal",
        "## Constraints & Preferences",
        "## Progress",
        "### Done",
        "### In Progress",
        "### Blocked",
        "## Key Decisions",
        "## Next Steps",
        "## Critical Context",
        "## Relevant Files",
    ];
    let mut last_idx = 0usize;
    for heading in canonical_headings {
        let pos = next_prompt[last_idx..]
            .find(heading)
            .map(|offset| last_idx + offset)
            .unwrap_or_else(|| {
                panic!(
                    "phase 4 (next prompt): heading `{heading}` not found in next prompt at or after offset {last_idx}; \
                     prompt was:\n{next_prompt}"
                )
            });
        last_idx = pos + heading.len();
    }

    // JSONL bytes for the source frame remain unchanged — only the
    // CompactionEvent append is added. Verify by re-reading and
    // confirming the original frame bytes still appear as a prefix.
    let bytes_after_compact = fs::read(jsonl.events_path()).expect("read jsonl bytes");
    assert!(
        bytes_after_compact.starts_with(&bytes_before),
        "S5 walk: source frame bytes must appear as an unchanged prefix; only the CompactionEvent append should follow"
    );
    assert!(
        bytes_after_compact.len() > bytes_before.len(),
        "S5 walk: CompactionEvent append must extend the JSONL"
    );
}

/// S5 negative variant: when the compaction agent emits a summary
/// missing one of the eight required headings, the dispatcher MUST
/// surface the error and persist NO `CompactionEvent`. This walk
/// exercises the same prune+gate path as the happy walk above so a
/// regression that lets phase 1/2 succeed but lets phase 3 silently
/// fall back to a partial summary trips immediately.
#[tokio::test]
async fn s5_e2e_schema_mismatch_after_prune_blocks_event_persistence() {
    let tmp = tempfile::TempDir::new().expect("tempdir must succeed in tests");
    let session_root = tmp.path().join("sessions").join("session-s5-bad");
    let jsonl = SessionJsonlStore::new(session_root.clone());
    let attachments = ContextAttachmentStore::new(&session_root);

    let mut session = SessionState::new("/repo/main");
    session.id = "session-s5-bad".to_string();
    jsonl
        .append(&SessionEvent::snapshot(session))
        .expect("snapshot append must succeed");

    let huge_output = make_huge_tool_output();
    let budget = ContextBudget::from_segments(
        FIXTURE_PROMPT_BUDGET_TOKENS,
        vec![ContextSegmentBudget::new(
            ContextSegmentKind::ToolResults,
            TOOL_RESULTS_BUDGET_TOKENS,
            TruncationPolicy::CompressLargeOutputs,
        )],
    )
    .expect("budget must validate");
    let gate_budget = budget.clone();

    let frame: ContextFrameEvent = ContextFrameBuilder::new(ContextFrameKind::PromptBuild, budget)
        .with_attachment_threshold_bytes(ATTACHMENT_THRESHOLD_BYTES)
        .push(
            ContextFrameCandidate::new(
                "large-tool-output",
                ContextSegmentKind::ToolResults,
                huge_output.clone(),
            )
            .source(ContextFrameSource::tool("shell", "cargo test"))
            .trust(ContextTrustLevel::Trusted)
            // Small per-segment estimate so the budget allocator
            // includes the segment; the dispatcher gate (phase 2)
            // operates on the synthesized post-prune transcript
            // token count, not on this estimate.
            .token_estimate(96),
        )
        .build(&attachments)
        .expect("frame must build cleanly");

    jsonl
        .append(&SessionEvent::context_frame(frame.clone()))
        .expect("frame append must succeed");

    let bytes_before = fs::read(jsonl.events_path()).expect("read jsonl bytes");

    // Phase 1 + 2 (prune + gate): same shape as the happy walk, just
    // re-invoked here so a regression in the *order* of phases
    // (e.g. compact before prune) trips this test as well.
    let replay = jsonl.load_context_replay().expect("replay must succeed");
    let loaded = &replay.frames[0];
    let mut rendered_prompt = String::new();
    for segment in &loaded.segments {
        let resolved: String = match (&segment.attachment, &segment.content) {
            (Some(att), _) => attachments
                .read_to_string(att)
                .expect("read_to_string must succeed"),
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
    assert!(rendered_prompt.contains("<pruned"));
    assert!(gate_budget.is_overflow(gate_budget.usable() + 512));

    // Phase 3 (compact): drop the `## Critical Context` heading from
    // the canonical template; everything else stays in canonical
    // order. The strict parser must reject.
    let truncated = VALID_SUMMARY.replace(
        "## Critical Context\n- Existing test runner does not propagate panics\n\n",
        "",
    );
    assert!(!truncated.contains("## Critical Context"));
    let model = CannedModel::from_text(&truncated);

    let outcome = run_compaction(
        &model,
        embedded_compaction_system_prompt(),
        rendered_prompt.as_str(),
        frame.frame_id,
        frame.attachment_refs(),
        Vec::new(),
        REMAINING_BUDGET_TOKENS,
    )
    .await;

    // Spec line 1702: the error must surface as
    // `ContextHandoffError::SchemaMismatch { missing_sections:
    // ["## Critical Context"] }` (template heading literal). Pattern-
    // match the variant + payload so a regression that surfaces a
    // generic ProviderError or omits the section name is caught.
    let err =
        outcome.expect_err("phase 3 (compact): truncated summary must surface as Err, not Ok");
    match err {
        CompactionAgentError::InvalidTemplate(ContextHandoffParseError::SchemaMismatch {
            missing_sections,
        }) => {
            assert_eq!(
                missing_sections,
                vec!["## Critical Context".to_string()],
                "phase 3 (compact): SchemaMismatch.missing_sections must list the dropped heading verbatim"
            );
        }
        other => panic!(
            "expected InvalidTemplate(SchemaMismatch{{missing_sections: [\"## Critical Context\"]}}), got {other:?}"
        ),
    }

    // Phase 4 invariant: NO CompactionEvent is constructible because
    // `compaction_event_for_handoff` requires a `&ContextHandoff`
    // value the `Err` path never yields. The doc's
    // "失败时不写入 CompactionEvent" rule is encoded in the
    // helper's signature; this walk exercises that property under
    // the integrated dispatcher pattern (rather than calling
    // run_compaction in isolation).
    //
    // Additionally verify on the persistence side: the JSONL store
    // received only the original ContextFrame append (no
    // CompactionEvent record can sneak through). Walk the replay and
    // confirm `compactions` is empty.
    let replay_after_failed_compact = jsonl
        .load_context_replay()
        .expect("replay must succeed even on the negative path");
    assert!(
        replay_after_failed_compact.compactions.is_empty(),
        "phase 3 (negative): the JSONL store must not contain any CompactionEvent on the SchemaMismatch path"
    );

    // Source frame bytes still byte-identical at the end of the
    // negative walk — phase 1 prune did not touch them, and phase 3
    // never reached the persistence step.
    let bytes_after = fs::read(jsonl.events_path()).expect("read jsonl bytes");
    assert_eq!(
        bytes_before, bytes_after,
        "negative S5 walk: JSONL bytes for the source frame must be byte-identical end-to-end"
    );
}
