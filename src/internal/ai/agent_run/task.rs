//! `AgentTask[S]` snapshot: a Phase-2 dispatch unit derived from a confirmed
//! `Task`. References — does not copy — the persistent `Task` business fields.
//!
//! `AgentTask[S]` 快照：从已确认 `Task` 派生的第 2 阶段调度单元。引用（不复制）持久化 `Task` 业务字段。

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{AgentRunId, AgentTaskId};

/// A sub-agent dispatch unit. Layer 1 generates one `AgentTask` per confirmed
/// `Task` it wants to delegate. The `AgentTask` is immutable once written;
/// further state lives on `AgentRun` (run.rs).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentTask {
    pub id: AgentTaskId,

    /// `thread_id` of the owning Layer 1 thread (stable across resume).
    pub thread_id: Uuid,

    /// Confirmed `Task` snapshot id this dispatch derives from.
    /// References `git_internal::internal::object::task::Task`.
    pub source_task_id: Uuid,

    /// Confirmed `Plan` snapshot id (the plan that contains the source task).
    pub source_plan_id: Uuid,

    /// Confirmed `IntentSpec` id, if available, for prompt context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_intent_id: Option<Uuid>,

    /// `agent_run_id` once the task has been picked up by an `AgentRun`.
    /// `None` while queued.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assigned_run: Option<AgentRunId>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CEX-S2-10 freezes the `AgentTask` wire contract
    /// (`#[serde(deny_unknown_fields)]`). Pin the required field set, the
    /// `skip_serializing_if = Option::is_none` behaviour of the optional
    /// fields, the `deny_unknown_fields` rejection, and the round-trip —
    /// so a rename / added field / dropped-skip silently breaking
    /// persisted `agents/{run_id}.jsonl` readers trips here.
    #[test]
    fn agent_task_wire_contract_is_frozen() {
        // Queued task: optional fields `None` are omitted by skip_if.
        let queued = AgentTask {
            id: AgentTaskId::new(),
            thread_id: Uuid::new_v4(),
            source_task_id: Uuid::new_v4(),
            source_plan_id: Uuid::new_v4(),
            source_intent_id: None,
            assigned_run: None,
        };
        let queued_json = serde_json::to_value(&queued).expect("serialize queued AgentTask");
        let obj = queued_json
            .as_object()
            .expect("AgentTask serializes to an object");
        for key in ["id", "thread_id", "source_task_id", "source_plan_id"] {
            assert!(obj.contains_key(key), "AgentTask must serialize `{key}`");
        }
        assert!(
            !obj.contains_key("source_intent_id") && !obj.contains_key("assigned_run"),
            "None optionals must be omitted (skip_serializing_if), got: {queued_json}",
        );

        // Assigned task: optional fields present when `Some`.
        let assigned = AgentTask {
            source_intent_id: Some(Uuid::new_v4()),
            assigned_run: Some(AgentRunId::new()),
            ..queued
        };
        let assigned_json = serde_json::to_value(&assigned).expect("serialize assigned AgentTask");
        assert!(
            assigned_json.get("source_intent_id").is_some()
                && assigned_json.get("assigned_run").is_some(),
            "Some optionals must serialize, got: {assigned_json}",
        );

        // deny_unknown_fields: an unknown field is rejected on read.
        let mut with_extra = assigned_json.as_object().expect("object").clone();
        with_extra.insert("bogus".to_string(), serde_json::Value::Bool(true));
        assert!(
            serde_json::from_value::<AgentTask>(serde_json::Value::Object(with_extra)).is_err(),
            "deny_unknown_fields must reject an unknown field",
        );

        // Round-trip: the wire shape deserializes and re-serializes intact.
        let back: AgentTask =
            serde_json::from_value(assigned_json.clone()).expect("deserialize AgentTask");
        assert_eq!(
            serde_json::to_value(&back).expect("re-serialize"),
            assigned_json,
            "AgentTask must round-trip its wire shape",
        );
    }
}
