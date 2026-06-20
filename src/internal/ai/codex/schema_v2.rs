//! Hand-maintained Codex protocol schema types that complement the generated v2
//! schema module.
//!
//! Boundary: these types must stay wire-compatible with the generated protocol and
//! tolerate unknown provider fields where possible. Mock Codex integration tests cover
//! tool-call, message, and streaming edge cases.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Serialize, Deserialize, Clone, Copy)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxMode {
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ThreadRef {
    #[serde(alias = "id")]
    pub thread_id: String,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TurnRef {
    pub id: String,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ThreadStartedParams {
    pub thread: ThreadRef,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ThreadStatusChangedParams {
    pub thread_id: String,
    pub status: String,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ThreadNameUpdatedParams {
    pub thread_id: String,
    pub name: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ThreadArchivedParams {
    pub thread_id: String,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ThreadClosedParams {
    pub thread_id: String,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TurnStartedParams {
    pub thread_id: String,
    pub turn: TurnRef,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TurnCompletedParams {
    pub turn: TurnRef,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ThreadCompactedParams {
    pub thread_id: String,
    pub turn_id: String,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TurnPlanStep {
    pub status: String,
    pub step: String,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TurnPlanUpdatedParams {
    pub thread_id: String,
    pub turn_id: String,
    pub plan: Vec<TurnPlanStep>,
    pub explanation: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsageBreakdown {
    pub cached_input_tokens: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_output_tokens: i64,
    pub total_tokens: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ThreadTokenUsage {
    pub last: TokenUsageBreakdown,
    pub total: TokenUsageBreakdown,
    pub model_context_window: Option<i64>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ThreadTokenUsageUpdatedParams {
    pub thread_id: String,
    pub turn_id: String,
    pub token_usage: ThreadTokenUsage,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DeltaNotificationParams {
    pub delta: String,
    pub item_id: String,
    pub thread_id: String,
    pub turn_id: String,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ItemStartedParams {
    pub thread_id: String,
    pub turn_id: String,
    pub item: Value,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ItemCompletedParams {
    pub thread_id: String,
    pub turn_id: String,
    pub item: Value,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalBaseParams {
    pub request_id: Option<String>,
}

pub fn parse_params<T: for<'de> Deserialize<'de>>(params: &Value) -> Option<T> {
    serde_json::from_value::<T>(params.clone()).ok()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn sandbox_mode_serialises_as_kebab_case() {
        // INVARIANT: the rename_all = "kebab-case" tag is what the
        // Codex server expects on the wire. A change to snake_case or
        // PascalCase would make every sandbox-mode request unparseable.
        assert_eq!(
            serde_json::to_string(&SandboxMode::ReadOnly).unwrap(),
            "\"read-only\""
        );
        assert_eq!(
            serde_json::to_string(&SandboxMode::WorkspaceWrite).unwrap(),
            "\"workspace-write\""
        );
        assert_eq!(
            serde_json::to_string(&SandboxMode::DangerFullAccess).unwrap(),
            "\"danger-full-access\""
        );
    }

    #[test]
    fn thread_ref_accepts_thread_id_or_legacy_id_alias() {
        // INVARIANT: both the new `threadId` field and the legacy
        // `id` alias must deserialise. Dropping the alias would break
        // every older Codex build still in use.
        let from_new: ThreadRef = serde_json::from_value(json!({"threadId": "abc"})).unwrap();
        assert_eq!(from_new.thread_id, "abc");

        let from_legacy: ThreadRef = serde_json::from_value(json!({"id": "abc"})).unwrap();
        assert_eq!(from_legacy.thread_id, "abc");
    }

    #[test]
    fn thread_started_params_inlines_nested_thread_ref() {
        let value = json!({"thread": {"threadId": "t1"}});
        let params: ThreadStartedParams = serde_json::from_value(value).unwrap();
        assert_eq!(params.thread.thread_id, "t1");
    }

    #[test]
    fn thread_status_changed_params_uses_camel_case_keys() {
        let value = json!({"threadId": "t1", "status": "running"});
        let params: ThreadStatusChangedParams = serde_json::from_value(value).unwrap();
        assert_eq!(params.thread_id, "t1");
        assert_eq!(params.status, "running");
    }

    #[test]
    fn thread_name_updated_params_accepts_null_name() {
        // INVARIANT: name is Option<String>; a null payload must
        // round-trip as None, not deserialise-fail.
        let with_name: ThreadNameUpdatedParams =
            serde_json::from_value(json!({"threadId": "t1", "name": "alpha"})).unwrap();
        assert_eq!(with_name.name.as_deref(), Some("alpha"));
        let with_null: ThreadNameUpdatedParams =
            serde_json::from_value(json!({"threadId": "t1", "name": null})).unwrap();
        assert!(with_null.name.is_none());
    }

    #[test]
    fn turn_started_and_completed_params_thread_their_ids() {
        let started: TurnStartedParams = serde_json::from_value(json!({
            "threadId": "t1",
            "turn": {"id": "u1"}
        }))
        .unwrap();
        assert_eq!(started.thread_id, "t1");
        assert_eq!(started.turn.id, "u1");

        let completed: TurnCompletedParams =
            serde_json::from_value(json!({"turn": {"id": "u1"}})).unwrap();
        assert_eq!(completed.turn.id, "u1");
    }

    #[test]
    fn thread_compacted_params_carries_thread_and_turn_ids() {
        let params: ThreadCompactedParams = serde_json::from_value(json!({
            "threadId": "t1",
            "turnId": "u1"
        }))
        .unwrap();
        assert_eq!(params.thread_id, "t1");
        assert_eq!(params.turn_id, "u1");
    }

    #[test]
    fn turn_plan_updated_params_deserialises_with_optional_explanation() {
        let with_explanation: TurnPlanUpdatedParams = serde_json::from_value(json!({
            "threadId": "t1",
            "turnId": "u1",
            "plan": [{"status": "todo", "step": "draft"}, {"status": "done", "step": "scan"}],
            "explanation": "why this plan"
        }))
        .unwrap();
        assert_eq!(with_explanation.plan.len(), 2);
        assert_eq!(with_explanation.plan[0].status, "todo");
        assert_eq!(with_explanation.plan[0].step, "draft");
        assert_eq!(
            with_explanation.explanation.as_deref(),
            Some("why this plan")
        );

        let without: TurnPlanUpdatedParams = serde_json::from_value(json!({
            "threadId": "t1",
            "turnId": "u1",
            "plan": [],
            "explanation": null
        }))
        .unwrap();
        assert!(without.plan.is_empty());
        assert!(without.explanation.is_none());
    }

    #[test]
    fn token_usage_breakdown_round_trips_through_serde() {
        let breakdown = TokenUsageBreakdown {
            cached_input_tokens: 100,
            input_tokens: 200,
            output_tokens: 300,
            reasoning_output_tokens: 400,
            total_tokens: 1_000,
        };
        let json = serde_json::to_value(&breakdown).unwrap();
        // INVARIANT: every field must serialise under its camelCase
        // wire name. A regression to snake_case here would break the
        // usage dashboard.
        assert_eq!(json["cachedInputTokens"], 100);
        assert_eq!(json["inputTokens"], 200);
        assert_eq!(json["outputTokens"], 300);
        assert_eq!(json["reasoningOutputTokens"], 400);
        assert_eq!(json["totalTokens"], 1_000);
        let parsed: TokenUsageBreakdown = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.total_tokens, breakdown.total_tokens);
    }

    #[test]
    fn thread_token_usage_round_trip_preserves_optional_context_window() {
        let usage = ThreadTokenUsage {
            last: TokenUsageBreakdown {
                cached_input_tokens: 1,
                input_tokens: 2,
                output_tokens: 3,
                reasoning_output_tokens: 4,
                total_tokens: 10,
            },
            total: TokenUsageBreakdown {
                cached_input_tokens: 10,
                input_tokens: 20,
                output_tokens: 30,
                reasoning_output_tokens: 40,
                total_tokens: 100,
            },
            model_context_window: Some(128_000),
        };
        let json = serde_json::to_value(&usage).unwrap();
        assert_eq!(json["modelContextWindow"], 128_000);
        let with_null: ThreadTokenUsage = serde_json::from_value(json!({
            "last": json["last"],
            "total": json["total"],
            "modelContextWindow": null,
        }))
        .unwrap();
        assert!(with_null.model_context_window.is_none());
    }

    #[test]
    fn thread_token_usage_updated_params_parses_full_envelope() {
        let value = json!({
            "threadId": "t1",
            "turnId": "u1",
            "tokenUsage": {
                "last": {"cachedInputTokens": 0, "inputTokens": 1, "outputTokens": 2, "reasoningOutputTokens": 3, "totalTokens": 6},
                "total": {"cachedInputTokens": 0, "inputTokens": 10, "outputTokens": 20, "reasoningOutputTokens": 30, "totalTokens": 60},
                "modelContextWindow": 64000
            }
        });
        let parsed: ThreadTokenUsageUpdatedParams = serde_json::from_value(value).unwrap();
        assert_eq!(parsed.thread_id, "t1");
        assert_eq!(parsed.token_usage.total.total_tokens, 60);
        assert_eq!(parsed.token_usage.model_context_window, Some(64_000));
    }

    #[test]
    fn delta_notification_params_threads_all_four_ids() {
        let value = json!({
            "delta": "chunk",
            "itemId": "i1",
            "threadId": "t1",
            "turnId": "u1"
        });
        let parsed: DeltaNotificationParams = serde_json::from_value(value).unwrap();
        assert_eq!(parsed.delta, "chunk");
        assert_eq!(parsed.item_id, "i1");
        assert_eq!(parsed.thread_id, "t1");
        assert_eq!(parsed.turn_id, "u1");
    }

    #[test]
    fn item_started_and_completed_params_keep_opaque_item_payload() {
        // INVARIANT: `item` is held as `serde_json::Value` so the
        // wrapper does not have to know every item subtype. A change
        // to `Map<String, Value>` would break payloads where the
        // server emits null for unknown subtypes.
        let value = json!({
            "threadId": "t1",
            "turnId": "u1",
            "item": {"kind": "agent_message", "details": {"chunks": []}}
        });
        let started: ItemStartedParams = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(started.item["kind"], "agent_message");
        let completed: ItemCompletedParams = serde_json::from_value(value).unwrap();
        assert_eq!(completed.item["kind"], "agent_message");
    }

    #[test]
    fn approval_base_params_accepts_present_or_missing_request_id() {
        let with_id: ApprovalBaseParams =
            serde_json::from_value(json!({"requestId": "req-1"})).unwrap();
        assert_eq!(with_id.request_id.as_deref(), Some("req-1"));
        let without: ApprovalBaseParams = serde_json::from_value(json!({})).unwrap();
        assert!(without.request_id.is_none());
    }

    #[test]
    fn parse_params_returns_some_when_value_matches_schema() {
        let value = json!({"thread": {"threadId": "t1"}});
        let params: Option<ThreadStartedParams> = parse_params(&value);
        assert!(params.is_some());
        assert_eq!(params.unwrap().thread.thread_id, "t1");
    }

    #[test]
    fn parse_params_returns_none_on_schema_mismatch() {
        // INVARIANT: silent fallback to None (rather than panic) is
        // what keeps the dispatcher resilient to schema drift —
        // unknown payload shapes must not crash the WebSocket reader.
        let value = json!({"completely": "unrelated"});
        let params: Option<ThreadStartedParams> = parse_params(&value);
        assert!(params.is_none());
        let bad_type: Option<ThreadStartedParams> = parse_params(&json!(42));
        assert!(bad_type.is_none());
    }
}
