//! CEX-S2-10 schema contract tests.
//!
//! These tests cover only the **schema-only scaffold**:
//! - Type construction with sane defaults
//! - Round-trip JSON serialization for the events / snapshots that will be
//!   persisted in `agents/{run_id}.jsonl`
//! - Unknown-event-safe behaviour for `AgentRunEvent` (S2-INV-10)
//! - The R-A4 disambiguation: persistent `EvidenceKind` vs runtime
//!   `EvidenceKind` can be aliased and used in the same scope without
//!   collisions
//!
//! Behaviour-level tests (dispatcher, hook execution, parallel scheduler,
//! merge candidate flow) belong to CEX-S2-11..18 and live in separate test
//! files.

#![cfg(feature = "subagent-scaffold")]

use libra::internal::ai::agent_run::{
    AgentBudget, AgentEvidence, AgentPermissionProfile, AgentRunEvent, AgentRunEventEnvelope,
    AgentRunId, AgentTaskId, AgentType, AnchorScope, BudgetDimension, Confidence, EventId,
    EvidenceId, HookFailureReason, HookInvocationPayload, HookKind, HookPhase, MergeCandidate,
    MergeCandidateId, MergeDecision, MergeDecisionPayloadV0, PackageId, PostToolReason,
    ReviewState, RunUsage, Sha256, ToolCallId, WorkspaceMaterialized, WorkspaceStrategy,
};
use serde_json::json;

// ---------------------------------------------------------------------------
// Construction defaults (S2-INV-05 / S2-INV-07)
// ---------------------------------------------------------------------------

#[test]
fn agent_permission_profile_default_denies_everything() {
    let profile = AgentPermissionProfile::default();
    assert!(profile.allowed_tools.is_empty(), "S2-INV-05 default deny");
    assert!(profile.allowed_source_slugs.is_empty());
    assert!(!profile.may_spawn_sub_agents, "S2-INV-09 default no spawn");
}

#[test]
fn merge_candidate_new_defaults_to_needs_human_review() {
    let id = MergeCandidateId::new();
    let candidate = MergeCandidate::new(id, vec![], vec![]);
    // INVARIANT: must default to needs_human_review per S2-INV-07.
    assert_eq!(candidate.review_state, ReviewState::NeedsHumanReview);
    assert!(candidate.review_evidence.is_empty());
}

#[test]
fn budget_default_is_unlimited_on_all_dimensions() {
    let budget = AgentBudget::default();
    assert!(budget.max_tokens.is_none());
    assert!(budget.max_tool_calls.is_none());
    assert!(budget.max_wall_clock_ms.is_none());
    assert!(budget.max_source_calls.is_none());
    assert!(budget.max_cost_micro_dollars.is_none());
}

// ---------------------------------------------------------------------------
// Round-trip serialization for persistent events
// ---------------------------------------------------------------------------

#[test]
fn agent_run_event_started_round_trip() {
    let agent_run_id = AgentRunId::new();
    let event = AgentRunEvent::Started { agent_run_id };
    let json = serde_json::to_value(&event).expect("serialize");
    assert_eq!(json["kind"], "started");
    let back: AgentRunEvent = serde_json::from_value(json).expect("deserialize");
    assert_eq!(event, back);
}

#[test]
fn agent_run_event_budget_exceeded_carries_dimension() {
    let event = AgentRunEvent::BudgetExceeded {
        agent_run_id: AgentRunId::new(),
        dimension: BudgetDimension::Token,
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["kind"], "budget_exceeded");
    assert_eq!(json["payload"]["dimension"], "token");
    let back: AgentRunEvent = serde_json::from_value(json).unwrap();
    assert_eq!(event, back);
}

#[test]
fn workspace_materialized_carries_required_fields() {
    let event = AgentRunEvent::WorkspaceMaterialized {
        agent_run_id: AgentRunId::new(),
        materialization: WorkspaceMaterialized {
            strategy: WorkspaceStrategy::Sparse,
            elapsed_ms: 1234,
            materialized_file_count: 50_000,
            source_repo_size: 500_000_000,
            fallback_reason: String::new(),
        },
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["payload"]["materialization"]["strategy"], "sparse");
    let back: AgentRunEvent = serde_json::from_value(json).unwrap();
    assert_eq!(event, back);
}

#[test]
fn run_usage_event_round_trip() {
    let event = AgentRunEvent::RunUsage {
        agent_run_id: AgentRunId::new(),
        usage: RunUsage {
            prompt_tokens: 1000,
            completion_tokens: 500,
            cached_tokens: 200,
            reasoning_tokens: 100,
            wall_clock_ms: 12_000,
            provider_latency_ms: 9_000,
            cost_estimate_micro_dollars: 4_200,
            tool_call_count: 7,
        },
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["kind"], "run_usage");
    assert_eq!(json["payload"]["usage"]["prompt_tokens"], 1000);
    let back: AgentRunEvent = serde_json::from_value(json).unwrap();
    assert_eq!(event, back);
}

#[test]
fn merge_decision_round_trip_with_default_payload() {
    let decision = MergeDecision {
        id: libra::internal::ai::agent_run::DecisionId::new(),
        merge_candidate_id: MergeCandidateId::new(),
        agent_run_ids: vec![AgentRunId::new()],
        resulting_state: ReviewState::Accepted,
        payload: MergeDecisionPayloadV0::default(),
    };
    let json = serde_json::to_value(&decision).unwrap();
    // CEX-S2-13 ownership: payload fields exist with empty defaults.
    assert!(json["payload"]["risk_score"].is_null());
    assert_eq!(
        json["payload"]["conflict_list"].as_array().unwrap().len(),
        0
    );
    assert_eq!(
        json["payload"]["test_evidence"].as_array().unwrap().len(),
        0
    );
    assert_eq!(
        json["payload"]["distillable_evidence_ids"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    let back: MergeDecision = serde_json::from_value(json).unwrap();
    assert_eq!(back.resulting_state, ReviewState::Accepted);
}

// ---------------------------------------------------------------------------
// Hook dispatch schema (CEX-S2-10 (5))
// ---------------------------------------------------------------------------

fn dummy_hook_invocation() -> HookInvocationPayload {
    HookInvocationPayload {
        phase: HookPhase::PreToolUse,
        tool_name: "shell".to_string(),
        tool_call_id: ToolCallId::new(),
        agent_run_id: AgentRunId::new(),
        hook_path: std::path::PathBuf::from("/etc/libra/hooks/secret-detector"),
        hook_checksum: Sha256("0".repeat(64)),
        hook_kind: HookKind::ProjectLocal,
        stdin_event_json: "{}".to_string(),
        timeout_ms: 30_000,
    }
}

#[test]
fn hook_passed_event_round_trip_is_flat() {
    // Hook events embed the outcome shape inline (no `HookOutcome` wrapper),
    // so the variant tag and payload always match. Codex r1 finding 7:
    // a `kind=hook_passed` cannot now contain a `BlockedByHook` body.
    let event = AgentRunEvent::HookPassed {
        agent_run_id: AgentRunId::new(),
        invocation: dummy_hook_invocation(),
        empty_stdout: false,
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["kind"], "hook_passed");
    assert!(json["payload"].get("invocation").is_some());
    let back: AgentRunEvent = serde_json::from_value(json).unwrap();
    assert_eq!(event, back);
}

#[test]
fn blocked_by_hook_event_uses_none_for_empty_reason() {
    // Per Step 2.2 "exit 2 + 空 stdout" row: hook_reason must be None, never
    // a sentinel string.
    let event = AgentRunEvent::BlockedByHook {
        agent_run_id: AgentRunId::new(),
        invocation: dummy_hook_invocation(),
        exit_code: 2,
        stdout_truncated: String::new(),
        stderr_truncated: String::new(),
        hook_reason: None,
    };
    let json = serde_json::to_value(&event).unwrap();
    assert!(json["payload"]["hook_reason"].is_null());
    let back: AgentRunEvent = serde_json::from_value(json).unwrap();
    assert_eq!(event, back);
}

#[test]
fn post_tool_review_required_reason_is_flat_union() {
    // Codex r1 finding 4: `PostToolReason` must be flat — the failure variants
    // are direct, NOT wrapped in `Failure(HookFailureReason)`. Verify by
    // round-tripping a representative subset and checking JSON shape.
    let reasons = [
        PostToolReason::HookDeny,
        PostToolReason::HookNeedsHuman,
        PostToolReason::UnknownExitCode { exit_code: 1 },
        PostToolReason::Panic,
        PostToolReason::Timeout,
        PostToolReason::KilledBySignal { signo: 9 },
        PostToolReason::SpawnEnoent,
        PostToolReason::SpawnEacces,
        PostToolReason::NeedsHumanTimeout,
        PostToolReason::Unspecified,
    ];
    for reason in reasons {
        let json = serde_json::to_value(&reason).unwrap();
        // Flat: no `Failure` wrapper key.
        assert!(json.get("failure").is_none(), "must not be wrapped");
        assert!(json.get("reason").is_some(), "tag = \"reason\" present");
        let back: PostToolReason = serde_json::from_value(json).unwrap();
        assert_eq!(reason, back);
    }
}

#[test]
fn hook_failure_reasons_cover_authoritative_table() {
    // The full list of failure reasons per Step 2.2 Hook exit-code table.
    let reasons = [
        HookFailureReason::UnknownExitCode { exit_code: 1 },
        HookFailureReason::Panic,
        HookFailureReason::Timeout,
        HookFailureReason::KilledBySignal { signo: 9 },
        HookFailureReason::SpawnEnoent,
        HookFailureReason::SpawnEacces,
        HookFailureReason::NeedsHumanTimeout,
        HookFailureReason::Unspecified,
    ];
    for reason in reasons {
        let json = serde_json::to_value(&reason).unwrap();
        let back: HookFailureReason = serde_json::from_value(json).unwrap();
        assert_eq!(reason, back);
    }
}

#[test]
fn capability_package_hook_kind_includes_package_id() {
    let kind = HookKind::CapabilityPackage {
        package_id: PackageId("local-test-package".to_string()),
    };
    let json = serde_json::to_value(&kind).unwrap();
    assert_eq!(json["source"], "capability_package");
    assert_eq!(json["package_id"], "local-test-package");
}

// ---------------------------------------------------------------------------
// AgentEvidence raw fact chain (S2-INV-12)
// ---------------------------------------------------------------------------

#[test]
fn agent_evidence_carries_raw_fact_chain_fields() {
    let evidence = AgentEvidence {
        id: EvidenceId::new(),
        agent_run_id: AgentRunId::new(),
        source_agent_type: AgentType::Worker,
        source_event_id: EventId::new(),
        tool_call_id: Some(ToolCallId::new()),
        source_call_id: None,
        confidence: Confidence::new(0.87),
        applies_to_scope: AnchorScope::AgentRun,
        distillable: true,
        evidence_snapshot_id: uuid::Uuid::new_v4(),
    };
    let json = serde_json::to_value(&evidence).unwrap();
    // S2-INV-12 raw fact chain fields are present and required.
    assert!(json.get("source_event_id").is_some());
    assert!(json.get("agent_run_id").is_some());
    assert_eq!(json["source_agent_type"], "worker");
    assert_eq!(json["distillable"], true);
    // R-A4 (Codex r1 finding 1): no `kind` field is duplicated from the
    // wrapped persistent snapshot. Consumers resolve via `evidence_snapshot_id`.
    assert!(
        json.get("kind").is_none(),
        "kind must not be duplicated from snapshot"
    );
}

// ---------------------------------------------------------------------------
// R-A4 disambiguation: both EvidenceKind types coexist with `as` aliases
// ---------------------------------------------------------------------------

#[test]
fn r_a4_persistent_and_runtime_evidence_kinds_coexist_with_aliases() {
    use git_internal::internal::object::evidence::EvidenceKind as PersistentEvidenceKind;
    use libra::internal::ai::runtime::contracts::EvidenceKind as RuntimeEvidenceKind;

    let persistent = PersistentEvidenceKind::Test;
    let runtime = RuntimeEvidenceKind::Test;

    // The two enums coexist when imported with `as` aliases (R-A4 audit
    // closure note). They are distinct types and must be addressed
    // separately; this test exists to fail compilation if the public
    // surface drifts.
    assert_eq!(format!("{:?}", persistent), "Test");
    assert!(matches!(runtime, RuntimeEvidenceKind::Test));
}

// ---------------------------------------------------------------------------
// Unknown-event-safe envelope (S2-INV-10 / R-A3)
// ---------------------------------------------------------------------------

#[test]
fn envelope_unknown_kind_falls_back_to_unknown_branch() {
    // Future variants, capability-package events, and typos all parse into
    // the `Unknown(Value)` branch via `untagged` fallback in
    // `AgentRunEventEnvelope`. The reader logs and continues replay rather
    // than failing.
    let future_event = json!({
        "kind": "future_variant_added_in_step_3",
        "payload": { "anything": [1, 2, 3] }
    });
    let parsed: AgentRunEventEnvelope =
        serde_json::from_value(future_event.clone()).expect("unknown kind must not error");
    assert!(parsed.is_unknown());
    // Raw payload preserved verbatim for audit / replay.
    if let AgentRunEventEnvelope::Unknown(value) = &parsed {
        assert_eq!(value, &future_event);
    } else {
        panic!("expected Unknown branch");
    }
}

#[test]
fn envelope_round_trip_preserves_unknown_payload() {
    // An old reader parses a future event into `Unknown(Value)`, and
    // re-serialization preserves the original JSON byte-equivalent (modulo
    // whitespace / key ordering). This is the documented contract — we keep
    // unknown payloads verbatim.
    let raw = json!({
        "kind": "future_kind",
        "payload": { "x": 1, "y": "two" }
    });
    let parsed: AgentRunEventEnvelope = serde_json::from_value(raw.clone()).unwrap();
    let back = serde_json::to_value(&parsed).unwrap();
    assert_eq!(back, raw);
}

#[test]
fn envelope_known_kind_parses_through_to_inner_event() {
    let ev = AgentRunEvent::Started {
        agent_run_id: AgentRunId::new(),
    };
    let json = serde_json::to_value(&ev).unwrap();
    let envelope: AgentRunEventEnvelope = serde_json::from_value(json).unwrap();
    match envelope.known() {
        Some(AgentRunEvent::Started { .. }) => {}
        other => panic!("expected Known(Started), got {other:?}"),
    }
}

#[test]
fn known_variants_still_parse_when_unknown_keys_appear_at_payload_level() {
    // Add a foreign sibling key at the same level as `kind` / `payload`.
    // For internally-tagged adjacent envelopes serde_json tolerates extra
    // keys; this test pins that behaviour at the inner-enum level (without
    // the envelope wrapper).
    let event_with_extra = json!({
        "kind": "started",
        "payload": { "agent_run_id": uuid::Uuid::nil() },
        "extension_field_from_future_release": 42
    });
    let parsed: AgentRunEvent = serde_json::from_value(event_with_extra)
        .expect("unknown sibling keys must not break envelope parsing");
    assert!(matches!(parsed, AgentRunEvent::Started { .. }));
}

// ---------------------------------------------------------------------------
// Sanity: AgentTaskId / AgentRunId / id newtypes round-trip through serde
// ---------------------------------------------------------------------------

#[test]
fn id_newtypes_serialize_as_transparent_uuids() {
    let task_id = AgentTaskId::new();
    let json = serde_json::to_value(task_id).unwrap();
    assert!(json.is_string(), "transparent serde for newtype");
    let back: AgentTaskId = serde_json::from_value(json).unwrap();
    assert_eq!(task_id, back);
}
