//! Claude-native audit read models derived from managed runtime artifacts.
//!
//! These objects are complementary projections for inspection and demo flows.
//! They do not replace formal lifecycle objects such as `intent_event`,
//! `run_event`, or `plan_step_event`.

use git_internal::{hash::ObjectHash, internal::object::types::ObjectType};
use serde::{Deserialize, Serialize};

use super::*;

const DERIVED_SOURCE_KIND: &str = "derived_from_claude_native_signal";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ClaudeApprovalRequestObject {
    schema: String,
    object_type: String,
    #[serde(rename = "sourceKind")]
    source_kind: String,
    #[serde(rename = "sourceSignal")]
    source_signal: String,
    #[serde(rename = "aiSessionId")]
    ai_session_id: String,
    #[serde(rename = "providerSessionId")]
    provider_session_id: String,
    #[serde(rename = "threadId")]
    thread_id: String,
    #[serde(rename = "runId")]
    run_id: String,
    id: String,
    #[serde(rename = "approvalType")]
    approval_type: String,
    #[serde(rename = "itemId")]
    item_id: String,
    #[serde(rename = "toolUseId", default, skip_serializing_if = "Option::is_none")]
    tool_use_id: Option<String>,
    #[serde(rename = "toolName", default, skip_serializing_if = "Option::is_none")]
    tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(
        rename = "displayName",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    command: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    changes: Vec<String>,
    #[serde(
        rename = "blockedPath",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    blocked_path: Option<String>,
    #[serde(
        rename = "decisionReason",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    decision_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    suggestions: Vec<Value>,
    status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    decision: Option<bool>,
    #[serde(rename = "requestedAt")]
    requested_at: String,
    #[serde(
        rename = "resolvedAt",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    resolved_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ClaudeReasoningObject {
    schema: String,
    object_type: String,
    #[serde(rename = "sourceKind")]
    source_kind: String,
    #[serde(rename = "sourceSignal")]
    source_signal: String,
    #[serde(rename = "aiSessionId")]
    ai_session_id: String,
    #[serde(rename = "providerSessionId")]
    provider_session_id: String,
    #[serde(rename = "threadId")]
    thread_id: String,
    #[serde(rename = "runId")]
    run_id: String,
    id: String,
    summary: Vec<String>,
    text: Option<String>,
    #[serde(
        rename = "messageUuid",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    message_uuid: Option<String>,
    #[serde(
        rename = "assistantMessageId",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    assistant_message_id: Option<String>,
    #[serde(rename = "blockIndex")]
    block_index: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    signature: Option<String>,
    #[serde(rename = "createdAt")]
    created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ClaudeToolInvocationEventObject {
    schema: String,
    object_type: String,
    #[serde(rename = "sourceKind")]
    source_kind: String,
    #[serde(rename = "sourceSignal")]
    source_signal: String,
    #[serde(rename = "aiSessionId")]
    ai_session_id: String,
    #[serde(rename = "providerSessionId")]
    provider_session_id: String,
    #[serde(rename = "threadId")]
    thread_id: String,
    #[serde(rename = "runId")]
    run_id: String,
    id: String,
    #[serde(rename = "toolUseId")]
    tool_use_id: String,
    #[serde(rename = "toolName", default, skip_serializing_if = "Option::is_none")]
    tool_name: Option<String>,
    status: String,
    #[serde(
        rename = "sourcePath",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    source_path: Option<String>,
    payload: Value,
    #[serde(rename = "createdAt")]
    created_at: String,
}

#[derive(Default)]
struct ApprovalRequestAccumulator {
    source_signal: String,
    tool_use_id: Option<String>,
    tool_name: Option<String>,
    title: Option<String>,
    display_name: Option<String>,
    description: Option<String>,
    command: Option<String>,
    changes: Vec<String>,
    blocked_path: Option<String>,
    decision_reason: Option<String>,
    suggestions: Vec<Value>,
    status: String,
    decision: Option<bool>,
    requested_at: Option<String>,
    resolved_at: Option<String>,
}

pub(super) async fn ensure_formal_derived_audit_objects(
    storage_path: &Path,
    run_binding: &ClaudeFormalRunBindingArtifact,
    audit_bundle: &ManagedAuditBundle,
) -> Result<()> {
    persist_approval_requests(storage_path, run_binding, audit_bundle).await?;
    persist_reasoning_objects(storage_path, run_binding, audit_bundle).await?;
    persist_tool_invocation_events(storage_path, run_binding, audit_bundle).await?;
    Ok(())
}

async fn persist_approval_requests(
    storage_path: &Path,
    run_binding: &ClaudeFormalRunBindingArtifact,
    audit_bundle: &ManagedAuditBundle,
) -> Result<()> {
    let mut grouped = BTreeMap::<String, ApprovalRequestAccumulator>::new();
    for event in &audit_bundle
        .bridge
        .object_candidates
        .decision_runtime_events
    {
        if !matches!(event.kind.as_str(), "PermissionRequest" | "CanUseTool") {
            continue;
        }
        let key = approval_group_key(event);
        let entry = grouped.entry(key.clone()).or_default();
        if entry.source_signal.is_empty() {
            entry.source_signal = event.kind.clone();
        }
        if entry.requested_at.is_none() {
            entry.requested_at = Some(event.at.clone());
        }
        entry.tool_use_id = entry.tool_use_id.clone().or_else(|| {
            event
                .payload
                .get("tool_use_id")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        });
        entry.tool_name = entry.tool_name.clone().or_else(|| {
            event
                .payload
                .get("tool_name")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        });
        entry.title = entry.title.clone().or_else(|| {
            event
                .payload
                .get("title")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        });
        entry.display_name = entry.display_name.clone().or_else(|| {
            event
                .payload
                .get("display_name")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        });
        entry.description = entry.description.clone().or_else(|| {
            event
                .payload
                .get("description")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        });
        entry.command = entry.command.clone().or_else(|| {
            event
                .payload
                .get("tool_input")
                .and_then(|value| value.get("command"))
                .and_then(Value::as_str)
                .map(ToString::to_string)
        });
        entry.blocked_path = entry.blocked_path.clone().or_else(|| {
            event
                .payload
                .get("blocked_path")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        });
        entry.decision_reason = entry.decision_reason.clone().or_else(|| {
            event
                .payload
                .get("decision_reason")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        });
        if entry.suggestions.is_empty() {
            entry.suggestions = event
                .payload
                .get("suggestions")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
        }
        if entry.changes.is_empty() {
            entry.changes = extract_approval_changes(&event.payload);
        }
        if event.kind == "CanUseTool" {
            entry.source_signal = "CanUseTool".to_string();
            entry.status = approval_status_from_payload(&event.payload);
            entry.decision = approval_decision_flag(&entry.status);
            entry.resolved_at = Some(event.at.clone());
        } else if entry.status.is_empty() {
            entry.status = "requested".to_string();
        }
    }

    for (key, entry) in grouped {
        let object_id = stable_derived_object_id(
            "claude_approval_request",
            &json!({
                "ai_session_id": run_binding.ai_session_id,
                "key": key,
            }),
        )?;
        let approval = ClaudeApprovalRequestObject {
            schema: "libra.claude_approval_request.v1".to_string(),
            object_type: "approval_request".to_string(),
            source_kind: DERIVED_SOURCE_KIND.to_string(),
            source_signal: entry.source_signal,
            ai_session_id: run_binding.ai_session_id.clone(),
            provider_session_id: run_binding.provider_session_id.clone(),
            thread_id: audit_bundle.bridge.object_candidates.thread_id.clone(),
            run_id: run_binding.run_id.clone(),
            id: object_id.clone(),
            approval_type: "tool_permission".to_string(),
            item_id: entry.tool_use_id.clone().unwrap_or_else(|| key.clone()),
            tool_use_id: entry.tool_use_id,
            tool_name: entry.tool_name,
            title: entry.title,
            display_name: entry.display_name,
            description: entry.description,
            command: entry.command,
            changes: entry.changes,
            blocked_path: entry.blocked_path,
            decision_reason: entry.decision_reason,
            suggestions: entry.suggestions,
            status: entry.status,
            decision: entry.decision,
            requested_at: entry
                .requested_at
                .unwrap_or_else(|| audit_bundle.generated_at.clone()),
            resolved_at: entry.resolved_at,
        };
        upsert_tracked_json_object(storage_path, "approval_request", &object_id, &approval).await?;
    }

    Ok(())
}

async fn persist_reasoning_objects(
    storage_path: &Path,
    run_binding: &ClaudeFormalRunBindingArtifact,
    audit_bundle: &ManagedAuditBundle,
) -> Result<()> {
    for (message_index, message) in audit_bundle.raw_artifact.messages.iter().enumerate() {
        if message.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let blocks = message
            .get("message")
            .and_then(|inner| inner.get("content"))
            .and_then(Value::as_array);
        let Some(blocks) = blocks else {
            continue;
        };
        for (block_index, block) in blocks.iter().enumerate() {
            if block.get("type").and_then(Value::as_str) != Some("thinking") {
                continue;
            }
            let Some(text) = non_empty_reasoning_text(block) else {
                continue;
            };
            let object_id = stable_derived_object_id(
                "claude_reasoning",
                &json!({
                    "ai_session_id": run_binding.ai_session_id,
                    "message_uuid": message.get("uuid"),
                    "message_id": message.get("message").and_then(|inner| inner.get("id")),
                    "block_index": block_index,
                    "message_index": message_index,
                }),
            )?;
            let reasoning = ClaudeReasoningObject {
                schema: "libra.claude_reasoning.v1".to_string(),
                object_type: "reasoning".to_string(),
                source_kind: DERIVED_SOURCE_KIND.to_string(),
                source_signal: "assistant_thinking_block".to_string(),
                ai_session_id: run_binding.ai_session_id.clone(),
                provider_session_id: run_binding.provider_session_id.clone(),
                thread_id: audit_bundle.bridge.object_candidates.thread_id.clone(),
                run_id: run_binding.run_id.clone(),
                id: object_id.clone(),
                summary: summarize_reasoning_text(Some(text.as_str())),
                text: Some(text),
                message_uuid: message
                    .get("uuid")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                assistant_message_id: message
                    .get("message")
                    .and_then(|inner| inner.get("id"))
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                block_index,
                signature: block
                    .get("signature")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                created_at: message
                    .get("timestamp")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
                    .unwrap_or_else(|| audit_bundle.generated_at.clone()),
            };
            upsert_tracked_json_object(storage_path, "reasoning", &object_id, &reasoning).await?;
        }
    }
    Ok(())
}

fn non_empty_reasoning_text(block: &Value) -> Option<String> {
    block
        .get("thinking")
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .map(ToString::to_string)
}

async fn persist_tool_invocation_events(
    storage_path: &Path,
    run_binding: &ClaudeFormalRunBindingArtifact,
    audit_bundle: &ManagedAuditBundle,
) -> Result<()> {
    for (index, hook_event) in audit_bundle.raw_artifact.hook_events.iter().enumerate() {
        let Some(status) = tool_invocation_status_from_hook(&hook_event.hook) else {
            continue;
        };
        let tool_use_id = hook_event
            .input
            .get("tool_use_id")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .unwrap_or_else(|| format!("tool-event-{index}"));
        let object_id = stable_derived_object_id(
            "claude_tool_invocation_event",
            &json!({
                "ai_session_id": run_binding.ai_session_id,
                "tool_use_id": tool_use_id,
                "status": status,
                "index": index,
            }),
        )?;
        let event = ClaudeToolInvocationEventObject {
            schema: "libra.claude_tool_invocation_event.v1".to_string(),
            object_type: "tool_invocation_event".to_string(),
            source_kind: DERIVED_SOURCE_KIND.to_string(),
            source_signal: hook_event.hook.clone(),
            ai_session_id: run_binding.ai_session_id.clone(),
            provider_session_id: run_binding.provider_session_id.clone(),
            thread_id: audit_bundle.bridge.object_candidates.thread_id.clone(),
            run_id: run_binding.run_id.clone(),
            id: object_id.clone(),
            tool_use_id,
            tool_name: hook_event
                .input
                .get("tool_name")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            status: status.to_string(),
            source_path: hook_event
                .input
                .get("transcript_path")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            payload: hook_event.input.clone(),
            created_at: audit_bundle.generated_at.clone(),
        };
        upsert_tracked_json_object(storage_path, "tool_invocation_event", &object_id, &event)
            .await?;
    }
    Ok(())
}

fn approval_group_key(event: &ManagedSemanticRuntimeEvent) -> String {
    event
        .payload
        .get("tool_use_id")
        .or_else(|| event.payload.get("permission_request_id"))
        .or_else(|| event.payload.get("uuid"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .unwrap_or_else(|| event.id.clone())
}

fn extract_approval_changes(payload: &Value) -> Vec<String> {
    let mut changes = BTreeSet::new();
    if let Some(path) = payload
        .get("tool_input")
        .and_then(|value| value.get("file_path"))
        .and_then(Value::as_str)
    {
        changes.insert(path.to_string());
    }
    if let Some(path) = payload
        .get("tool_input")
        .and_then(|value| value.get("path"))
        .and_then(Value::as_str)
    {
        changes.insert(path.to_string());
    }
    changes.into_iter().collect()
}

fn approval_status_from_payload(payload: &Value) -> String {
    match payload.get("approval_decision").and_then(Value::as_str) {
        Some("allow")
            if payload.get("approval_scope").and_then(Value::as_str) == Some("session_mode") =>
        {
            "approved_session_mode".to_string()
        }
        Some("allow")
            if payload.get("approval_scope").and_then(Value::as_str) == Some("session") =>
        {
            "approved_session".to_string()
        }
        Some("allow") => "approved_once".to_string(),
        Some("deny") => "denied".to_string(),
        Some("abort") => "aborted".to_string(),
        _ => "requested".to_string(),
    }
}

fn approval_decision_flag(status: &str) -> Option<bool> {
    match status {
        "approved_once" | "approved_session" | "approved_session_mode" => Some(true),
        "denied" | "aborted" => Some(false),
        _ => None,
    }
}

fn summarize_reasoning_text(text: Option<&str>) -> Vec<String> {
    let Some(text) = text else {
        return Vec::new();
    };
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(3)
        .map(|line| {
            let truncated = line.chars().take(180).collect::<String>();
            if line.chars().count() > 180 {
                format!("{truncated}...")
            } else {
                truncated
            }
        })
        .collect()
}

fn tool_invocation_status_from_hook(hook_name: &str) -> Option<&'static str> {
    match hook_name {
        "PreToolUse" => Some("in_progress"),
        "PostToolUse" => Some("completed"),
        "PostToolUseFailure" => Some("failed"),
        _ => None,
    }
}

fn stable_derived_object_id(prefix: &str, seed: &Value) -> Result<String> {
    let seed_bytes = serde_json::to_vec(seed).context("failed to serialize derived object seed")?;
    let hash = ObjectHash::from_type_and_data(ObjectType::Blob, &seed_bytes);
    Ok(format!("{prefix}__{hash}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_empty_reasoning_text_rejects_blank_thinking_blocks() {
        assert!(non_empty_reasoning_text(&json!({"thinking": ""})).is_none());
        assert!(non_empty_reasoning_text(&json!({"thinking": "  \n\t  "})).is_none());
        assert_eq!(
            non_empty_reasoning_text(&json!({"thinking": "Investigate the failing hook"})),
            Some("Investigate the failing hook".to_string())
        );
    }
}
