use std::fs;

use libra::internal::ai::{
    context_budget::{
        AllocationOmissionReason, CompactionEvent, CompactionReason, ContextAttachmentStore,
        ContextBudget, ContextFrameBuilder, ContextFrameCandidate, ContextFrameKind,
        ContextFrameSource, ContextSegmentBudget, ContextSegmentKind, ContextTrustLevel,
        TruncationPolicy,
    },
    runtime::event::Event,
    session::{
        SessionState,
        jsonl::{SessionEvent, SessionJsonlStore},
    },
};

#[test]
fn context_frame_events_roundtrip_through_session_jsonl() {
    let tmp = tempfile::TempDir::new().unwrap();
    let session_root = tmp.path().join("sessions").join("session-1");
    let jsonl = SessionJsonlStore::new(session_root.clone());
    let attachments = ContextAttachmentStore::new(&session_root);

    let mut session = SessionState::new("/repo/main");
    session.id = "session-1".to_string();
    jsonl.append(&SessionEvent::snapshot(session)).unwrap();

    let frame = ContextFrameBuilder::new(
        ContextFrameKind::PromptBuild,
        ContextBudget::from_segments(
            500,
            vec![
                ContextSegmentBudget::new(
                    ContextSegmentKind::SystemRules,
                    128,
                    TruncationPolicy::Never,
                ),
                ContextSegmentBudget::new(
                    ContextSegmentKind::ToolResults,
                    128,
                    TruncationPolicy::CompressLargeOutputs,
                ),
            ],
        )
        .unwrap(),
    )
    .with_prompt_id("turn-1")
    .with_attachment_threshold_bytes(64)
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
            "tool-output",
            ContextSegmentKind::ToolResults,
            long_output(),
        )
        .source(ContextFrameSource::tool("shell", "cargo test"))
        .trust(ContextTrustLevel::Trusted)
        .token_estimate(96),
    )
    .build(&attachments)
    .unwrap();

    assert_eq!(frame.event_kind(), "context_frame");
    assert_eq!(frame.prompt_id.as_deref(), Some("turn-1"));
    assert_eq!(frame.segments.len(), 2);
    let tool_segment = frame
        .segments
        .iter()
        .find(|segment| segment.id == "tool-output")
        .unwrap();
    assert!(tool_segment.content.is_none());
    let attachment = tool_segment
        .attachment
        .as_ref()
        .expect("large tool output should be externalized");
    assert_eq!(attachment.line_count, 12);
    assert!(
        attachment.read_hint.contains("attachments/"),
        "prompt should get an actionable attachment read hint"
    );
    assert_eq!(
        attachments.read_to_string(attachment).unwrap(),
        long_output()
    );

    jsonl
        .append(&SessionEvent::context_frame(frame.clone()))
        .unwrap();
    let compaction = CompactionEvent::from_frame(
        &frame,
        CompactionReason::BudgetPressure,
        "tool result moved to attachment",
    );
    jsonl.append(&SessionEvent::compaction(compaction)).unwrap();

    let replay = jsonl.load_context_replay().unwrap();
    assert_eq!(replay.frames.len(), 1);
    assert_eq!(replay.compactions.len(), 1);
    assert_eq!(replay.frames[0].segments[0].id, "rules");

    let line = fs::read_to_string(jsonl.events_path()).unwrap();
    assert!(line.contains("\"kind\":\"context_frame\""));
    assert!(line.contains("\"kind\":\"compaction_event\""));
    assert!(!line.contains("line 11: xxxxxxxxxxxxxxxxxxxx"));
}

#[test]
fn context_frame_records_budget_omissions_without_dropping_safety_rules() {
    let tmp = tempfile::TempDir::new().unwrap();
    let attachments = ContextAttachmentStore::new(tmp.path());
    let budget = ContextBudget::from_segments(
        80,
        vec![
            ContextSegmentBudget::new(ContextSegmentKind::SystemRules, 40, TruncationPolicy::Never),
            ContextSegmentBudget::new(
                ContextSegmentKind::SourceContext,
                20,
                TruncationPolicy::PreserveSourceLabels,
            ),
        ],
    )
    .unwrap();

    let frame = ContextFrameBuilder::new(ContextFrameKind::PromptBuild, budget)
        .push(
            ContextFrameCandidate::new(
                "safety",
                ContextSegmentKind::SystemRules,
                "Protected branch and approval state must remain visible.",
            )
            .source(ContextFrameSource::runtime("approval_state"))
            .trust(ContextTrustLevel::Trusted)
            .token_estimate(120)
            .non_compressible(true),
        )
        .push(
            ContextFrameCandidate::new(
                "untrusted-source",
                ContextSegmentKind::SourceContext,
                "Low-priority external context.",
            )
            .source(ContextFrameSource::file("README.md"))
            .trust(ContextTrustLevel::Untrusted)
            .token_estimate(20),
        )
        .build(&attachments)
        .unwrap();

    assert_eq!(frame.segments.len(), 1);
    assert_eq!(frame.segments[0].id, "safety");
    assert!(frame.segments[0].non_compressible);
    assert_eq!(frame.budget_exceeded_by, 40);

    let omission = frame
        .omissions
        .iter()
        .find(|omission| omission.id == "untrusted-source")
        .unwrap();
    assert_eq!(
        omission.reason,
        AllocationOmissionReason::TotalBudgetExceeded
    );

    let compaction = CompactionEvent::from_frame(
        &frame,
        CompactionReason::BudgetPressure,
        "source context omitted after safety overrun",
    );
    assert_eq!(compaction.protected_segment_ids, vec!["safety"]);
    assert_eq!(compaction.tokens_before, 140);
    assert_eq!(compaction.tokens_after, 120);
    assert_eq!(compaction.omitted_segment_ids, vec!["untrusted-source"]);
}

fn long_output() -> String {
    (0..12)
        .map(|line| format!("line {line}: xxxxxxxxxxxxxxxxxxxx"))
        .collect::<Vec<_>>()
        .join("\n")
}
