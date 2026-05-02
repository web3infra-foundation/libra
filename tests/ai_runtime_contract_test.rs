//! Wave 1A runtime contract tests.
//!
//! Pin the `TaskExecutor` trait contract so any provider that implements
//! `CompletionModel` can be plugged into the runtime by wrapping it in a thin
//! adapter. Verifies the runtime can build a task prompt, dispatch through a
//! `TaskExecutor`, and surface the response back as a `TaskExecutionResult`.
//!
//! **Layer:** L1 — uses `MockCompletionModel`, no external dependencies.

mod helpers;

use std::path::PathBuf;

use async_trait::async_trait;
use helpers::mock_completion_model::MockCompletionModel;
use libra::internal::ai::{
    completion::{AssistantContent, CompletionModel, CompletionRequest},
    runtime::{
        Runtime, RuntimeConfig,
        contracts::{
            ApprovalMediationState, TaskExecutionContext, TaskExecutionError, TaskExecutionResult,
            TaskExecutionStatus, TaskExecutor,
        },
    },
};
use uuid::Uuid;

/// Generic adapter that turns any `CompletionModel` into a `TaskExecutor`.
///
/// Demonstrates the wiring an integrator would write to plug a custom provider into
/// the runtime: forward the prompt messages, capture the first text response as the
/// summary, fabricate a `run_id` if one was not supplied, and report
/// `TaskExecutionStatus::Completed`.
#[derive(Clone)]
struct CompletionBackedTaskExecutor<M> {
    model: M,
}

#[async_trait]
impl<M> TaskExecutor for CompletionBackedTaskExecutor<M>
where
    M: CompletionModel + Clone + Send + Sync,
{
    async fn execute_task_attempt(
        &self,
        context: TaskExecutionContext,
    ) -> Result<TaskExecutionResult, TaskExecutionError> {
        let response = self
            .model
            .completion(CompletionRequest::new(
                context
                    .prompt
                    .messages
                    .into_iter()
                    .map(Into::into)
                    .collect(),
            ))
            .await
            .map_err(|err| TaskExecutionError::Provider(err.to_string()))?;
        let summary = response.content.first().and_then(|content| match content {
            AssistantContent::Text(text) => Some(text.text.clone()),
            AssistantContent::ToolCall(_) => None,
        });

        Ok(TaskExecutionResult {
            task_id: context.task_id,
            run_id: context.run_id.unwrap_or_else(Uuid::new_v4),
            status: TaskExecutionStatus::Completed,
            evidence: vec![],
            summary,
        })
    }
}

/// Scenario: build the runtime's task prompt with a fixture provider/model pair,
/// dispatch a single attempt through `CompletionBackedTaskExecutor` backed by
/// `MockCompletionModel::text("attempt complete")`, and assert the result preserves
/// the supplied `task_id`, marks the attempt completed, and surfaces the model's
/// text as the summary. Acts as the contract pin proving the runtime actually
/// integrates a generic provider via the `TaskExecutor` trait alone.
#[tokio::test]
async fn generic_provider_can_execute_through_task_executor_contract() {
    let runtime = Runtime::new(RuntimeConfig {
        principal: "contract-test".into(),
    });
    let prompt = runtime
        .task_prompt_builder("mock", "scripted")
        .task("write tests", "prove the runtime contract")
        .build();
    let task_id = Uuid::new_v4();
    let executor = CompletionBackedTaskExecutor {
        model: MockCompletionModel::text("attempt complete"),
    };

    let result = executor
        .execute_task_attempt(TaskExecutionContext {
            thread_id: Uuid::new_v4(),
            task_id,
            run_id: None,
            working_dir: PathBuf::from("."),
            prompt,
            approval: ApprovalMediationState::RuntimeMediatedInteractive,
        })
        .await
        .expect("task attempt");

    assert_eq!(result.task_id, task_id);
    assert_eq!(result.status, TaskExecutionStatus::Completed);
    assert_eq!(result.summary.as_deref(), Some("attempt complete"));
}

// ---------------------------------------------------------------------------
// CEX-00.5: top-level Event / Snapshot trait contract
// ---------------------------------------------------------------------------

mod cex_00_5 {
    use chrono::Utc;
    use libra::internal::ai::{
        hooks::lifecycle::{LifecycleEvent, LifecycleEventKind},
        runtime::{
            Event, Snapshot, audit_action_for,
            contracts::{MaterializedProjection, ProjectionFreshness, ProjectionVersions},
        },
    };
    use uuid::Uuid;

    fn lifecycle(kind: LifecycleEventKind) -> LifecycleEvent {
        LifecycleEvent {
            kind,
            session_id: "test-session".to_string(),
            session_ref: None,
            prompt: None,
            model: None,
            source: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            assistant_message: None,
            timestamp: Utc::now(),
        }
    }

    fn projection(thread_id: Uuid) -> MaterializedProjection {
        MaterializedProjection {
            thread_id,
            versions: ProjectionVersions::default(),
            freshness: ProjectionFreshness::Fresh,
            summary: serde_json::Value::Null,
        }
    }

    #[test]
    fn lifecycle_event_kinds_match_display_strings() {
        // The Event::event_kind impl must mirror the existing Display impl
        // verbatim; drift between them would fork audit / wire / log readers.
        let cases = [
            (LifecycleEventKind::SessionStart, "session_start"),
            (LifecycleEventKind::TurnStart, "turn_start"),
            (LifecycleEventKind::ToolUse, "tool_use"),
            (LifecycleEventKind::ModelUpdate, "model_update"),
            (LifecycleEventKind::Compaction, "compaction"),
            (LifecycleEventKind::TurnEnd, "turn_end"),
            (LifecycleEventKind::SessionEnd, "session_end"),
        ];
        for (kind, expected) in cases {
            let event = lifecycle(kind);
            assert_eq!(event.event_kind(), expected);
            assert_eq!(format!("{}", event.kind), expected);
        }
    }

    /// CEX-00.5 P2 fix (round 3 — byte-for-byte golden): pin the actual
    /// `Uuid::new_v5` output for a fixed `(session_id, timestamp_nanos,
    /// kind)` tuple so that **any** change to either the namespace UUID
    /// (`LIFECYCLE_EVENT_NAMESPACE` in `lifecycle.rs`) or the name-bytes
    /// layout will fail this test. Audit logs may persist `event_id`, so a
    /// silent change to the derivation would break dedupe / correlation
    /// across upgrades.
    ///
    /// To regenerate the golden value (only on a deliberate, versioned
    /// migration to a new namespace / layout):
    /// ```text
    /// $ cargo test --test ai_runtime_contract_test \
    ///     lifecycle_event_id_v5_golden -- --nocapture
    /// ```
    /// then copy the printed value into `EXPECTED_GOLDEN` and document
    /// the migration in the audit closure.
    #[test]
    fn lifecycle_event_id_v5_golden_value_is_stable() {
        const EXPECTED_GOLDEN: &str = "69eaa838-b433-55f6-8068-d943a56cfcb8";

        let event = LifecycleEvent {
            kind: LifecycleEventKind::TurnStart,
            session_id: "golden".to_string(),
            session_ref: None,
            prompt: None,
            model: None,
            source: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            assistant_message: None,
            timestamp: chrono::DateTime::<Utc>::from_timestamp_nanos(1_700_000_000_000_000_000),
        };

        let id = event.event_id();
        let expected = Uuid::parse_str(EXPECTED_GOLDEN).expect("parseable golden UUID");
        assert_eq!(
            id, expected,
            "lifecycle event_id derivation drifted — see test docs for migration steps"
        );

        // Structural sanity (catches the case where someone updates both
        // the golden and the derivation but breaks the version/variant).
        assert_eq!(id.get_version_num(), 5, "must be UUIDv5 (SHA-1 namespaced)");
        assert_eq!(
            id.get_variant(),
            uuid::Variant::RFC4122,
            "must be RFC 4122 variant"
        );
    }

    /// CEX-00.5 P1 (R-A3) test coverage: the `Event` trait does not
    /// enforce envelope-with-typed-payload at compile time, but the
    /// canonical pattern (`tag = "kind", content = "payload"` plus an
    /// `untagged` Known/Unknown wrapper) MUST stay reachable so concrete
    /// implementors keep using it. This test exercises the pattern with a
    /// tiny in-test event hierarchy and proves an unknown future variant
    /// falls through to `Unknown(Value)` instead of erroring.
    #[test]
    fn r_a3_envelope_pattern_round_trips_and_survives_unknown_kinds() {
        use serde::{Deserialize, Serialize};
        use serde_json::json;

        #[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
        #[serde(tag = "kind", content = "payload", rename_all = "snake_case")]
        enum DemoEvent {
            Started { id: u64 },
            Stopped { id: u64, reason: String },
        }

        #[derive(Debug, Serialize, Deserialize)]
        #[serde(untagged)]
        enum DemoEventEnvelope {
            Known(Box<DemoEvent>),
            Unknown(serde_json::Value),
        }

        // 1. A known variant round-trips through the envelope.
        let started = DemoEvent::Started { id: 7 };
        let wire = serde_json::to_value(&started).unwrap();
        assert_eq!(wire["kind"], "started");
        let envelope: DemoEventEnvelope = serde_json::from_value(wire).unwrap();
        match envelope {
            DemoEventEnvelope::Known(event) => assert_eq!(*event, started),
            DemoEventEnvelope::Unknown(_) => panic!("known variant must not fall through"),
        }

        // 2. An unknown future variant falls through to Unknown(Value)
        // and the raw payload is preserved verbatim.
        let future = json!({
            "kind": "future_variant",
            "payload": { "anything": [1, 2, 3] }
        });
        let envelope: DemoEventEnvelope = serde_json::from_value(future.clone())
            .expect("unknown kind must not error — R-A3 / S2-INV-10");
        match envelope {
            DemoEventEnvelope::Known(_) => panic!("unknown kind must not parse as Known"),
            DemoEventEnvelope::Unknown(raw) => assert_eq!(raw, future),
        }
    }

    #[test]
    fn lifecycle_event_id_is_deterministic_and_collision_safe() {
        // CEX-00.5 P2 fix: derive `event_id()` deterministically from
        // (session_id, timestamp_nanos, kind) so the id is stable for an
        // occurrence and distinct events do not silently collide.
        let mut a = lifecycle(LifecycleEventKind::TurnStart);
        a.session_id = "alpha".to_string();
        a.timestamp = chrono::DateTime::<Utc>::from_timestamp_nanos(1_700_000_000_000_000_000);
        let mut b = a.clone();

        // Same input -> same id.
        assert_eq!(a.event_id(), b.event_id());
        // Stable across clones / impl Trait coercion.
        let dyn_ref: &dyn Event = &a;
        assert_eq!(dyn_ref.event_id(), a.event_id());

        // Different session -> different id.
        b.session_id = "beta".to_string();
        assert_ne!(a.event_id(), b.event_id());

        // Different timestamp -> different id.
        let mut c = a.clone();
        c.timestamp = a.timestamp + chrono::Duration::nanoseconds(1);
        assert_ne!(a.event_id(), c.event_id());

        // Different kind -> different id.
        let mut d = a.clone();
        d.kind = LifecycleEventKind::ToolUse;
        assert_ne!(a.event_id(), d.event_id());

        // Never the nil UUID for a real event.
        assert_ne!(a.event_id(), Uuid::nil());
    }

    #[test]
    fn lifecycle_event_summary_includes_kind_and_session() {
        let event = lifecycle(LifecycleEventKind::ToolUse);
        let summary = event.event_summary();
        assert!(summary.contains("kind=tool_use"));
        assert!(summary.contains("session=test-session"));
    }

    #[test]
    fn lifecycle_event_summary_carries_tool_when_present() {
        let mut event = lifecycle(LifecycleEventKind::ToolUse);
        event.tool_name = Some("apply_patch".to_string());
        let summary = event.event_summary();
        assert!(summary.contains("tool=apply_patch"));
    }

    #[test]
    fn audit_action_for_lifecycle_event_produces_event_prefixed_kind() {
        let event = lifecycle(LifecycleEventKind::SessionEnd);
        let dyn_ref: &dyn Event = &event;
        assert_eq!(audit_action_for(dyn_ref), "event/session_end");
    }

    #[test]
    fn event_trait_is_dyn_compatible() {
        // CEX-00.5 contract: Event must be dyn-compatible so callers can
        // pass `&dyn Event` (e.g. into `AuditSink::record_event`).
        let event = lifecycle(LifecycleEventKind::Compaction);
        let dyn_ref: &dyn Event = &event;
        let _kind = dyn_ref.event_kind();
    }

    #[test]
    fn lifecycle_event_kind_strings_are_stable_snake_case() {
        for kind in [
            LifecycleEventKind::SessionStart,
            LifecycleEventKind::TurnStart,
            LifecycleEventKind::ToolUse,
            LifecycleEventKind::ModelUpdate,
            LifecycleEventKind::Compaction,
            LifecycleEventKind::TurnEnd,
            LifecycleEventKind::SessionEnd,
        ] {
            let event = lifecycle(kind);
            let kind_str = event.event_kind();
            assert!(
                kind_str.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "event_kind '{kind_str}' must be snake_case ascii"
            );
            assert!(!kind_str.is_empty());
        }
    }

    #[test]
    fn materialized_projection_snapshot_id_is_thread_id() {
        let id = Uuid::new_v4();
        let snap = projection(id);
        assert_eq!(snap.snapshot_kind(), "materialized_projection");
        assert_eq!(snap.snapshot_id(), id);
    }

    #[test]
    fn snapshot_trait_is_dyn_compatible() {
        let snap = projection(Uuid::nil());
        let dyn_ref: &dyn Snapshot = &snap;
        assert_eq!(dyn_ref.snapshot_kind(), "materialized_projection");
    }

    #[test]
    fn snapshot_id_is_stable_under_clone() {
        let id = Uuid::new_v4();
        let snap = projection(id);
        let cloned = snap.clone();
        assert_eq!(snap.snapshot_id(), cloned.snapshot_id());
        assert_eq!(snap.snapshot_kind(), cloned.snapshot_kind());
    }
}
