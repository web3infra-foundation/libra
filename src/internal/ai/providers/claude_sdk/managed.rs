//! Claude Agent SDK managed-mode artifact parsing and bridge conversion.

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    internal::{
        ai::{
            history::HistoryManager,
            hooks::{
                lifecycle::normalize_json_value,
                runtime::{AI_SESSION_SCHEMA, AI_SESSION_TYPE, build_ai_session_id},
            },
            intentspec::{
                DraftAcceptance, DraftCheck, DraftIntent, DraftRisk, IntentDraft, RiskLevel,
                types::{ChangeType, TouchHints},
            },
            session::SessionState,
        },
        db,
    },
    utils::{object::write_git_object, storage::local::LocalStorage},
};

const MANAGED_SOURCE_NAME: &str = "claude_agent_sdk_managed";
const MANAGED_AUDIT_BUNDLE_SCHEMA: &str = "libra.claude_managed_audit_bundle.v1";
const MANAGED_INTENT_EXTRACTION_SOURCE: &str = "claude_agent_sdk_managed.structured_output";
const MANAGED_ARTIFACTS_DIR: &str = "managed-artifacts";
const AUDIT_BUNDLES_DIR: &str = "audit-bundles";
const INTENT_EXTRACTIONS_DIR: &str = "intent-extractions";
const NORMALIZED_EVENTS_KEY: &str = "normalized_events";
const RAW_HOOK_EVENTS_KEY: &str = "raw_hook_events";
const SESSION_PHASE_METADATA_KEY: &str = "session_phase";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeManagedArtifact {
    pub cwd: String,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(rename = "helperTimedOut", default)]
    pub helper_timed_out: bool,
    #[serde(rename = "helperError", default)]
    pub helper_error: Option<String>,
    #[serde(rename = "hookEvents", default)]
    pub hook_events: Vec<ClaudeManagedHookEvent>,
    #[serde(default)]
    pub messages: Vec<Value>,
    #[serde(rename = "resultMessage", default)]
    pub result_message: Option<ClaudeManagedResultMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeManagedHookEvent {
    pub hook: String,
    #[serde(default)]
    pub input: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeManagedResultMessage {
    #[serde(default)]
    pub r#type: Option<String>,
    #[serde(default)]
    pub subtype: Option<String>,
    #[serde(default)]
    pub is_error: Option<bool>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub stop_reason: Option<String>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub duration_api_ms: Option<u64>,
    #[serde(default)]
    pub num_turns: Option<u64>,
    #[serde(default)]
    pub result: Option<String>,
    #[serde(default)]
    pub total_cost_usd: Option<f64>,
    #[serde(default)]
    pub usage: Option<Value>,
    #[serde(rename = "modelUsage", default)]
    pub model_usage: Option<Value>,
    #[serde(default)]
    pub permission_denials: Option<Value>,
    #[serde(default)]
    pub structured_output: Option<Value>,
    #[serde(default)]
    pub fast_mode_state: Option<Value>,
    #[serde(default)]
    pub uuid: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ManagedArtifactIngestion {
    pub session: SessionState,
    pub intent_extraction: Option<IntentDraft>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedManagedArtifactOutcome {
    #[serde(rename = "providerSessionId")]
    pub provider_session_id: String,
    #[serde(rename = "aiSessionId")]
    pub ai_session_id: String,
    #[serde(rename = "aiSessionObjectHash")]
    pub ai_session_object_hash: String,
    #[serde(rename = "alreadyPersisted")]
    pub already_persisted: bool,
    #[serde(
        rename = "intentExtractionPath",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub intent_extraction_path: Option<String>,
    #[serde(rename = "rawArtifactPath")]
    pub raw_artifact_path: String,
    #[serde(rename = "auditBundlePath")]
    pub audit_bundle_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedAuditBundle {
    pub schema: String,
    pub provider: String,
    #[serde(rename = "managedSource")]
    pub managed_source: String,
    #[serde(rename = "aiSessionId")]
    pub ai_session_id: String,
    #[serde(rename = "providerSessionId")]
    pub provider_session_id: String,
    #[serde(rename = "generatedAt")]
    pub generated_at: String,
    #[serde(rename = "rawArtifact")]
    pub raw_artifact: ClaudeManagedArtifact,
    pub bridge: ManagedBridgeArtifacts,
    #[serde(rename = "fieldProvenance", default)]
    pub field_provenance: Vec<ManagedFieldProvenance>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedBridgeArtifacts {
    #[serde(rename = "sessionState")]
    pub session_state: SessionState,
    #[serde(rename = "aiSession")]
    pub ai_session: Value,
    #[serde(rename = "objectCandidates")]
    pub object_candidates: ManagedObjectCandidates,
    #[serde(rename = "intentExtraction")]
    pub intent_extraction: ManagedDraftExtractionReport,
    #[serde(
        rename = "intentExtractionArtifact",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub intent_extraction_artifact: Option<PersistedManagedIntentExtraction>,
    #[serde(rename = "toolInvocations", default)]
    pub tool_invocations: Vec<ManagedToolInvocation>,
    #[serde(rename = "touchHints", default)]
    pub touch_hints: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedObjectCandidates {
    #[serde(rename = "threadId")]
    pub thread_id: String,
    #[serde(rename = "runSnapshot")]
    pub run_snapshot: ManagedRunSnapshot,
    #[serde(rename = "runEvent")]
    pub run_event: ManagedRunEvent,
    #[serde(rename = "provenanceSnapshot")]
    pub provenance_snapshot: ManagedProvenanceSnapshot,
    #[serde(rename = "providerInitSnapshot")]
    pub provider_init_snapshot: ManagedProviderInitSnapshot,
    #[serde(
        rename = "runUsageEvent",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub run_usage_event: Option<ManagedRunUsageEvent>,
    #[serde(rename = "toolInvocationEvents", default)]
    pub tool_invocation_events: Vec<ManagedToolInvocationEvent>,
    #[serde(rename = "toolRuntimeEvents", default)]
    pub tool_runtime_events: Vec<ManagedSemanticRuntimeEvent>,
    #[serde(rename = "assistantRuntimeEvents", default)]
    pub assistant_runtime_events: Vec<ManagedSemanticRuntimeEvent>,
    #[serde(rename = "taskRuntimeEvents", default)]
    pub task_runtime_events: Vec<ManagedSemanticRuntimeEvent>,
    #[serde(rename = "decisionRuntimeEvents", default)]
    pub decision_runtime_events: Vec<ManagedSemanticRuntimeEvent>,
    #[serde(rename = "contextRuntimeEvents", default)]
    pub context_runtime_events: Vec<ManagedSemanticRuntimeEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedRunSnapshot {
    pub id: String,
    #[serde(rename = "threadId")]
    pub thread_id: String,
    #[serde(rename = "startedAt")]
    pub started_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedRunEvent {
    pub id: String,
    #[serde(rename = "runId")]
    pub run_id: String,
    pub status: String,
    pub at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedProvenanceSnapshot {
    pub id: String,
    #[serde(rename = "runId")]
    pub run_id: String,
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub parameters: Value,
    #[serde(rename = "createdAt")]
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedProviderInitSnapshot {
    pub id: String,
    #[serde(rename = "runId")]
    pub run_id: String,
    #[serde(rename = "threadId")]
    pub thread_id: String,
    #[serde(
        rename = "apiKeySource",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub api_key_source: Option<String>,
    #[serde(
        rename = "claudeCodeVersion",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub claude_code_version: Option<String>,
    #[serde(
        rename = "outputStyle",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub output_style: Option<String>,
    #[serde(default)]
    pub agents: Vec<String>,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(rename = "slashCommands", default)]
    pub slash_commands: Vec<String>,
    #[serde(rename = "mcpServers", default)]
    pub mcp_servers: Vec<Value>,
    #[serde(default)]
    pub plugins: Vec<Value>,
    #[serde(
        rename = "fastModeState",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub fast_mode_state: Option<String>,
    #[serde(rename = "capturedAt")]
    pub captured_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedRunUsageEvent {
    #[serde(rename = "runId")]
    pub run_id: String,
    #[serde(rename = "threadId")]
    pub thread_id: String,
    pub at: String,
    pub usage: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedToolInvocationEvent {
    pub id: String,
    #[serde(rename = "runId")]
    pub run_id: String,
    #[serde(rename = "threadId")]
    pub thread_id: String,
    pub tool: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    pub status: String,
    pub at: String,
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedSemanticRuntimeEvent {
    pub id: String,
    #[serde(rename = "runId")]
    pub run_id: String,
    #[serde(rename = "threadId")]
    pub thread_id: String,
    #[serde(rename = "semanticObject")]
    pub semantic_object: String,
    pub kind: String,
    pub source: String,
    pub at: String,
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedDraftExtractionReport {
    pub status: String,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedToolInvocation {
    #[serde(rename = "toolUseId")]
    pub tool_use_id: String,
    #[serde(rename = "toolName", default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(rename = "toolInput", default, skip_serializing_if = "Option::is_none")]
    pub tool_input: Option<Value>,
    #[serde(
        rename = "toolResponse",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub tool_response: Option<Value>,
    #[serde(
        rename = "transcriptPath",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub transcript_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedFieldProvenance {
    #[serde(rename = "fieldPath")]
    pub field_path: String,
    #[serde(rename = "sourceLayer")]
    pub source_layer: String,
    #[serde(rename = "sourcePath")]
    pub source_path: String,
    pub value: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct StructuredIntentExtractionOutput {
    summary: String,
    #[serde(rename = "problemStatement")]
    problem_statement: String,
    #[serde(rename = "changeType")]
    change_type: ChangeType,
    objectives: Vec<String>,
    #[serde(rename = "inScope", default)]
    in_scope: Vec<String>,
    #[serde(rename = "outOfScope", default)]
    out_of_scope: Vec<String>,
    #[serde(rename = "touchHints", default)]
    touch_hints: Option<TouchHints>,
    #[serde(rename = "successCriteria")]
    success_criteria: Vec<String>,
    #[serde(rename = "fastChecks", default)]
    fast_checks: Vec<DraftCheck>,
    #[serde(rename = "integrationChecks", default)]
    integration_checks: Vec<DraftCheck>,
    #[serde(rename = "securityChecks", default)]
    security_checks: Vec<DraftCheck>,
    #[serde(rename = "releaseChecks", default)]
    release_checks: Vec<DraftCheck>,
    #[serde(rename = "riskRationale")]
    risk_rationale: String,
    #[serde(rename = "riskFactors", default)]
    risk_factors: Vec<String>,
    #[serde(rename = "riskLevel", default)]
    risk_level: Option<RiskLevel>,
}

#[derive(Debug, Clone)]
struct ManagedSystemInit {
    session_id: String,
    cwd: String,
    model: Option<String>,
    permission_mode: Option<String>,
    tools: Vec<String>,
    api_key_source: Option<String>,
    claude_code_version: Option<String>,
    output_style: Option<String>,
    agents: Vec<String>,
    skills: Vec<String>,
    slash_commands: Vec<String>,
    mcp_servers: Vec<Value>,
    plugins: Vec<Value>,
    fast_mode_state: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct ToolHookPair {
    tool_use_id: String,
    tool_name: Option<String>,
    tool_input: Option<Value>,
    tool_response: Option<Value>,
    transcript_path: Option<String>,
    saw_pre: bool,
    saw_post: bool,
}

#[derive(Debug, Clone)]
struct IntentExtractionOutcome {
    extraction: Option<IntentDraft>,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedManagedIntentExtraction {
    pub schema: String,
    #[serde(rename = "ai_session_id")]
    pub ai_session_id: String,
    pub source: String,
    pub extraction: IntentDraft,
}

pub fn ingest_managed_artifact(
    artifact: &ClaudeManagedArtifact,
) -> Result<ManagedArtifactIngestion> {
    let mut session = build_bridge_session(artifact)?;
    let intent_extraction =
        extract_intent_extraction_from_result(artifact.result_message.as_ref())?;
    if intent_extraction.is_some() {
        session.metadata.insert(
            "intent_extraction_source".to_string(),
            json!("structured_output"),
        );
    }

    Ok(ManagedArtifactIngestion {
        session,
        intent_extraction,
    })
}

pub fn build_managed_audit_bundle(artifact: &ClaudeManagedArtifact) -> Result<ManagedAuditBundle> {
    let system_init = extract_system_init(artifact)
        .context("managed artifact does not contain a valid system init message")?;
    let session = build_bridge_session(artifact)?;
    let extraction_outcome = extract_intent_extraction_outcome(artifact.result_message.as_ref());
    let tool_invocations = merge_tool_hook_events(&artifact.hook_events)
        .into_iter()
        .map(ManagedToolInvocation::from)
        .collect::<Vec<_>>();
    let object_candidates = build_object_candidates(&session, &system_init, artifact)?;
    let touch_hints = collect_touch_hints(&tool_invocations, &system_init.cwd);
    let ai_session = build_managed_ai_session_payload(&session);
    let intent_extraction_artifact = extraction_outcome
        .extraction
        .clone()
        .map(|extraction| PersistedManagedIntentExtraction::new(session.id.clone(), extraction));
    let intent_extraction = ManagedDraftExtractionReport {
        status: extraction_outcome.status_label().to_string(),
        source: "result.structured_output".to_string(),
        error: extraction_outcome.error.clone(),
    };
    let field_provenance = build_field_provenance(
        &system_init,
        artifact,
        &extraction_outcome,
        &tool_invocations,
        &touch_hints,
    )?;

    Ok(ManagedAuditBundle {
        schema: MANAGED_AUDIT_BUNDLE_SCHEMA.to_string(),
        provider: "claude".to_string(),
        managed_source: MANAGED_SOURCE_NAME.to_string(),
        ai_session_id: session.id.clone(),
        provider_session_id: system_init.session_id,
        generated_at: Utc::now().to_rfc3339(),
        raw_artifact: artifact.clone(),
        bridge: ManagedBridgeArtifacts {
            session_state: session,
            ai_session,
            object_candidates,
            intent_extraction,
            intent_extraction_artifact,
            tool_invocations,
            touch_hints,
        },
        field_provenance,
    })
}

pub fn extract_intent_extraction_from_result(
    result_message: Option<&ClaudeManagedResultMessage>,
) -> Result<Option<IntentDraft>> {
    let outcome = extract_intent_extraction_outcome(result_message);
    if let Some(error) = outcome.error {
        return Err(anyhow!(error));
    }
    Ok(outcome.extraction)
}

pub async fn persist_managed_artifact(
    storage_path: &Path,
    artifact: &ClaudeManagedArtifact,
) -> Result<PersistedManagedArtifactOutcome> {
    let bundle = build_managed_audit_bundle(artifact)?;
    let ai_session_id = bundle.ai_session_id.clone();
    let provider_session_id = bundle.provider_session_id.clone();

    let objects_dir = storage_path.join("objects");
    fs::create_dir_all(&objects_dir).with_context(|| {
        format!(
            "failed to create objects directory '{}'",
            objects_dir.display()
        )
    })?;

    let storage = Arc::new(LocalStorage::new(objects_dir));
    let db_conn = Arc::new(db::get_db_conn_instance().await.clone());
    let history_manager = HistoryManager::new(storage, storage_path.to_path_buf(), db_conn);

    let ai_session_payload = normalize_json_value(bundle.bridge.ai_session.clone());
    let blob_data = serde_json::to_vec(&ai_session_payload)
        .context("failed to serialize ai_session payload")?;
    let blob_hash = write_git_object(storage_path, "blob", &blob_data)
        .context("failed to write ai_session git blob")?;
    let existing_object_hash = history_manager
        .get_object_hash(AI_SESSION_TYPE, &ai_session_id)
        .await?;

    let (ai_session_object_hash, already_persisted) = if let Some(existing) = existing_object_hash {
        (existing.to_string(), true)
    } else {
        history_manager
            .append(AI_SESSION_TYPE, &ai_session_id, blob_hash)
            .await
            .with_context(|| {
                format!(
                    "failed to append ai_session '{}' to '{}'",
                    ai_session_id,
                    history_manager.ref_name()
                )
            })?;
        (blob_hash.to_string(), false)
    };

    let raw_artifact_path = write_pretty_json_artifact(
        &storage_path.join(MANAGED_ARTIFACTS_DIR),
        &ai_session_id,
        artifact,
    )?;
    let audit_bundle_path = write_pretty_json_artifact(
        &storage_path.join(AUDIT_BUNDLES_DIR),
        &ai_session_id,
        &bundle,
    )?;
    let intent_extraction_path = match &bundle.bridge.intent_extraction_artifact {
        Some(intent_extraction_artifact) => Some(
            write_pretty_json_artifact(
                &storage_path.join(INTENT_EXTRACTIONS_DIR),
                &ai_session_id,
                intent_extraction_artifact,
            )?
            .to_string_lossy()
            .to_string(),
        ),
        None => {
            delete_generated_artifact_if_exists(
                &storage_path.join(INTENT_EXTRACTIONS_DIR),
                &ai_session_id,
            )?;
            None
        }
    };
    Ok(PersistedManagedArtifactOutcome {
        provider_session_id,
        ai_session_id,
        ai_session_object_hash,
        already_persisted,
        intent_extraction_path,
        raw_artifact_path: raw_artifact_path.to_string_lossy().to_string(),
        audit_bundle_path: audit_bundle_path.to_string_lossy().to_string(),
    })
}

impl ClaudeManagedArtifact {
    fn prompt_text(&self) -> Option<String> {
        self.hook_events
            .iter()
            .find(|event| event.hook == "UserPromptSubmit")
            .and_then(|event| event.input.get("prompt"))
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .or_else(|| self.prompt.clone())
    }

    fn transcript_path(&self) -> Option<String> {
        self.hook_events.iter().find_map(|event| {
            event
                .input
                .get("transcript_path")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
    }

    fn assistant_text_messages(&self) -> Vec<String> {
        self.messages
            .iter()
            .filter(|message| message.get("type").and_then(Value::as_str) == Some("assistant"))
            .flat_map(|message| {
                message
                    .get("message")
                    .and_then(|value| value.get("content"))
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|content| {
                        if content.get("type").and_then(Value::as_str) != Some("text") {
                            return None;
                        }
                        content
                            .get("text")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|text| !text.is_empty())
                            .map(ToString::to_string)
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn has_stop_hook(&self) -> bool {
        self.hook_events.iter().any(|event| event.hook == "Stop")
    }
}

fn extract_system_init(artifact: &ClaudeManagedArtifact) -> Result<ManagedSystemInit> {
    let message = artifact
        .messages
        .iter()
        .find(|message| {
            message.get("type").and_then(Value::as_str) == Some("system")
                && message.get("subtype").and_then(Value::as_str) == Some("init")
        })
        .ok_or_else(|| anyhow!("system init message is missing"))?;

    let session_id = message
        .get("session_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("system init message is missing session_id"))?
        .to_string();
    let cwd = message
        .get("cwd")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| artifact.cwd.clone());
    let model = message
        .get("model")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let permission_mode = message
        .get("permissionMode")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let tools = message
        .get("tools")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let api_key_source = message
        .get("apiKeySource")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let claude_code_version = message
        .get("claude_code_version")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let output_style = message
        .get("output_style")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let agents = message
        .get("agents")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let skills = message
        .get("skills")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let slash_commands = message
        .get("slash_commands")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mcp_servers = message
        .get("mcp_servers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let plugins = message
        .get("plugins")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let fast_mode_state = message
        .get("fast_mode_state")
        .and_then(Value::as_str)
        .map(ToString::to_string);

    Ok(ManagedSystemInit {
        session_id,
        cwd,
        model,
        permission_mode,
        tools,
        api_key_source,
        claude_code_version,
        output_style,
        agents,
        skills,
        slash_commands,
        mcp_servers,
        plugins,
        fast_mode_state,
    })
}

fn append_raw_hook_events(session: &mut SessionState, hook_events: &[ClaudeManagedHookEvent]) {
    let items = hook_events
        .iter()
        .map(|event| {
            json!({
                "hook_event_name": event.hook,
                "session_id": event.input.get("session_id"),
                "cwd": event.input.get("cwd"),
                "transcript_path": event.input.get("transcript_path"),
                "extra": event.input,
                "timestamp": Utc::now().to_rfc3339(),
            })
        })
        .collect::<Vec<_>>();
    session
        .metadata
        .insert(RAW_HOOK_EVENTS_KEY.to_string(), Value::Array(items));
}

fn merge_tool_hook_events(hook_events: &[ClaudeManagedHookEvent]) -> Vec<ToolHookPair> {
    let mut order = Vec::new();
    let mut seen = HashSet::new();
    let mut pairs = HashMap::<String, ToolHookPair>::new();

    for event in hook_events {
        let Some(tool_use_id) = event
            .input
            .get("tool_use_id")
            .and_then(Value::as_str)
            .map(ToString::to_string)
        else {
            continue;
        };

        if seen.insert(tool_use_id.clone()) {
            order.push(tool_use_id.clone());
        }

        let pair = pairs
            .entry(tool_use_id.clone())
            .or_insert_with(|| ToolHookPair {
                tool_use_id: tool_use_id.clone(),
                ..ToolHookPair::default()
            });
        match event.hook.as_str() {
            "PreToolUse" => {
                pair.saw_pre = true;
                pair.tool_name = event
                    .input
                    .get("tool_name")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                pair.tool_input = event.input.get("tool_input").cloned();
                pair.transcript_path = event
                    .input
                    .get("transcript_path")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
            }
            "PostToolUse" => {
                pair.saw_post = true;
                if pair.tool_name.is_none() {
                    pair.tool_name = event
                        .input
                        .get("tool_name")
                        .and_then(Value::as_str)
                        .map(ToString::to_string);
                }
                if pair.tool_input.is_none() {
                    pair.tool_input = event.input.get("tool_input").cloned();
                }
                pair.tool_response = event.input.get("tool_response").cloned();
                if pair.transcript_path.is_none() {
                    pair.transcript_path = event
                        .input
                        .get("transcript_path")
                        .and_then(Value::as_str)
                        .map(ToString::to_string);
                }
            }
            _ => {}
        }
    }

    order
        .into_iter()
        .filter_map(|id| pairs.remove(&id))
        .collect::<Vec<_>>()
}

fn build_object_candidates(
    session: &SessionState,
    system_init: &ManagedSystemInit,
    artifact: &ClaudeManagedArtifact,
) -> Result<ManagedObjectCandidates> {
    let thread_id = session.id.clone();
    let run_id = format!("{thread_id}::run");
    let run_started_at = session.created_at.to_rfc3339();
    let observed_at = session.updated_at.to_rfc3339();
    let run_status = managed_run_status(artifact);
    let run_error = managed_run_error(artifact);
    let provenance_parameters = build_provenance_parameters(system_init, artifact);
    let tool_invocation_events = merge_tool_hook_events(&artifact.hook_events)
        .into_iter()
        .map(|tool| {
            ManagedToolInvocationEvent::from_tool_hook_pair(&thread_id, &run_id, &observed_at, tool)
        })
        .collect::<Vec<_>>();
    let provider_init_snapshot =
        build_provider_init_snapshot(&thread_id, &run_id, &observed_at, system_init);
    let run_usage_event = artifact
        .result_message
        .as_ref()
        .and_then(|result| result.usage.clone())
        .map(|usage| ManagedRunUsageEvent {
            run_id: run_id.clone(),
            thread_id: thread_id.clone(),
            at: observed_at.clone(),
            usage,
        });
    let tool_runtime_events =
        build_tool_runtime_events(&thread_id, &run_id, &observed_at, artifact);
    let assistant_runtime_events =
        build_assistant_runtime_events(&thread_id, &run_id, &observed_at, artifact);
    let task_runtime_events =
        build_task_runtime_events(&thread_id, &run_id, &observed_at, artifact);
    let decision_runtime_events =
        build_decision_runtime_events(&thread_id, &run_id, &observed_at, artifact);
    let context_runtime_events =
        build_context_runtime_events(&thread_id, &run_id, &observed_at, artifact);

    Ok(ManagedObjectCandidates {
        thread_id: thread_id.clone(),
        run_snapshot: ManagedRunSnapshot {
            id: run_id.clone(),
            thread_id: thread_id.clone(),
            started_at: run_started_at,
        },
        run_event: ManagedRunEvent {
            id: format!("{run_id}::status"),
            run_id: run_id.clone(),
            status: run_status,
            at: observed_at.clone(),
            error: run_error,
        },
        provenance_snapshot: ManagedProvenanceSnapshot {
            id: format!("{run_id}::provenance"),
            run_id,
            provider: "claude".to_string(),
            model: system_init.model.clone(),
            parameters: provenance_parameters,
            created_at: session.created_at.to_rfc3339(),
        },
        provider_init_snapshot,
        run_usage_event,
        tool_invocation_events,
        tool_runtime_events,
        assistant_runtime_events,
        task_runtime_events,
        decision_runtime_events,
        context_runtime_events,
    })
}

fn build_tool_runtime_events(
    thread_id: &str,
    run_id: &str,
    observed_at: &str,
    artifact: &ClaudeManagedArtifact,
) -> Vec<ManagedSemanticRuntimeEvent> {
    let mut events = Vec::new();

    for (index, message) in artifact.messages.iter().enumerate() {
        let Some(message_type) = message.get("type").and_then(Value::as_str) else {
            continue;
        };
        if !matches!(message_type, "tool_progress" | "tool_use_summary") {
            continue;
        }

        let id = message
            .get("uuid")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .or_else(|| {
                message
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            })
            .unwrap_or_else(|| format!("{run_id}::{message_type}::{index}"));
        events.push(ManagedSemanticRuntimeEvent {
            id,
            run_id: run_id.to_string(),
            thread_id: thread_id.to_string(),
            semantic_object: "Tool".to_string(),
            kind: message_type.to_string(),
            source: "stream".to_string(),
            at: observed_at.to_string(),
            payload: message.clone(),
        });
    }

    events
}

fn build_assistant_runtime_events(
    thread_id: &str,
    run_id: &str,
    observed_at: &str,
    artifact: &ClaudeManagedArtifact,
) -> Vec<ManagedSemanticRuntimeEvent> {
    let mut events = Vec::new();

    for (index, message) in artifact.messages.iter().enumerate() {
        if message.get("type").and_then(Value::as_str) != Some("stream_event") {
            continue;
        }

        let kind = message
            .get("event")
            .and_then(|event| event.get("type"))
            .and_then(Value::as_str)
            .unwrap_or("stream_event");
        events.push(ManagedSemanticRuntimeEvent {
            id: message
                .get("uuid")
                .and_then(Value::as_str)
                .map(ToString::to_string)
                .unwrap_or_else(|| format!("{run_id}::{kind}::{index}")),
            run_id: run_id.to_string(),
            thread_id: thread_id.to_string(),
            semantic_object: "Assistant".to_string(),
            kind: kind.to_string(),
            source: "stream".to_string(),
            at: observed_at.to_string(),
            payload: message.clone(),
        });
    }

    events
}

fn build_provider_init_snapshot(
    thread_id: &str,
    run_id: &str,
    observed_at: &str,
    system_init: &ManagedSystemInit,
) -> ManagedProviderInitSnapshot {
    ManagedProviderInitSnapshot {
        id: format!("{run_id}::provider-init"),
        run_id: run_id.to_string(),
        thread_id: thread_id.to_string(),
        api_key_source: system_init.api_key_source.clone(),
        claude_code_version: system_init.claude_code_version.clone(),
        output_style: system_init.output_style.clone(),
        agents: system_init.agents.clone(),
        skills: system_init.skills.clone(),
        slash_commands: system_init.slash_commands.clone(),
        mcp_servers: system_init.mcp_servers.clone(),
        plugins: system_init.plugins.clone(),
        fast_mode_state: system_init.fast_mode_state.clone(),
        captured_at: observed_at.to_string(),
    }
}

fn build_task_runtime_events(
    thread_id: &str,
    run_id: &str,
    observed_at: &str,
    artifact: &ClaudeManagedArtifact,
) -> Vec<ManagedSemanticRuntimeEvent> {
    let mut events = Vec::new();

    for (index, message) in artifact.messages.iter().enumerate() {
        let Some(subtype) = message.get("subtype").and_then(Value::as_str) else {
            continue;
        };
        if !matches!(
            subtype,
            "task_started" | "task_progress" | "task_notification"
        ) {
            continue;
        }

        events.push(ManagedSemanticRuntimeEvent {
            id: message
                .get("uuid")
                .and_then(Value::as_str)
                .map(ToString::to_string)
                .unwrap_or_else(|| format!("{run_id}::{subtype}::{index}")),
            run_id: run_id.to_string(),
            thread_id: thread_id.to_string(),
            semantic_object: "Task".to_string(),
            kind: subtype.to_string(),
            source: "stream".to_string(),
            at: observed_at.to_string(),
            payload: message.clone(),
        });
    }

    for (index, hook_event) in artifact.hook_events.iter().enumerate() {
        if !matches!(
            hook_event.hook.as_str(),
            "SubagentStart" | "SubagentStop" | "TaskCompleted" | "TeammateIdle"
        ) {
            continue;
        }

        events.push(ManagedSemanticRuntimeEvent {
            id: hook_event
                .input
                .get("task_id")
                .or_else(|| hook_event.input.get("agent_id"))
                .and_then(Value::as_str)
                .map(ToString::to_string)
                .unwrap_or_else(|| format!("{run_id}::{}::{index}", hook_event.hook)),
            run_id: run_id.to_string(),
            thread_id: thread_id.to_string(),
            semantic_object: "Task".to_string(),
            kind: hook_event.hook.to_string(),
            source: "hook".to_string(),
            at: observed_at.to_string(),
            payload: hook_event.input.clone(),
        });
    }

    events
}

fn build_decision_runtime_events(
    thread_id: &str,
    run_id: &str,
    observed_at: &str,
    artifact: &ClaudeManagedArtifact,
) -> Vec<ManagedSemanticRuntimeEvent> {
    let mut events = Vec::new();

    for (index, hook_event) in artifact.hook_events.iter().enumerate() {
        if !matches!(
            hook_event.hook.as_str(),
            "PermissionRequest" | "Elicitation" | "ElicitationResult" | "CanUseTool"
        ) {
            continue;
        }

        events.push(ManagedSemanticRuntimeEvent {
            id: hook_event
                .input
                .get("tool_use_id")
                .or_else(|| hook_event.input.get("elicitation_id"))
                .and_then(Value::as_str)
                .map(ToString::to_string)
                .unwrap_or_else(|| format!("{run_id}::{}::{index}", hook_event.hook)),
            run_id: run_id.to_string(),
            thread_id: thread_id.to_string(),
            semantic_object: "Decision".to_string(),
            kind: hook_event.hook.to_string(),
            source: "hook".to_string(),
            at: observed_at.to_string(),
            payload: hook_event.input.clone(),
        });
    }

    if let Some(permission_denials) = artifact
        .result_message
        .as_ref()
        .and_then(|result| result.permission_denials.clone())
    {
        let denial_count = permission_denials.as_array().map(Vec::len).unwrap_or(0);
        if denial_count > 0 {
            events.push(ManagedSemanticRuntimeEvent {
                id: format!("{run_id}::permission-denials"),
                run_id: run_id.to_string(),
                thread_id: thread_id.to_string(),
                semantic_object: "Decision".to_string(),
                kind: "permission_denials".to_string(),
                source: "result".to_string(),
                at: observed_at.to_string(),
                payload: permission_denials,
            });
        }
    }

    events
}

fn build_context_runtime_events(
    thread_id: &str,
    run_id: &str,
    observed_at: &str,
    artifact: &ClaudeManagedArtifact,
) -> Vec<ManagedSemanticRuntimeEvent> {
    let mut events = Vec::new();

    for (index, message) in artifact.messages.iter().enumerate() {
        let message_type = message.get("type").and_then(Value::as_str);
        let subtype = message.get("subtype").and_then(Value::as_str);
        let is_context_message =
            matches!(
                (message_type, subtype),
                (
                    Some("system"),
                    Some("status" | "compact_boundary" | "files_persisted")
                )
            ) || matches!(message_type, Some("rate_limit_event" | "prompt_suggestion"));
        if !is_context_message {
            continue;
        }

        let kind = subtype.unwrap_or_else(|| message_type.unwrap_or("unknown"));
        events.push(ManagedSemanticRuntimeEvent {
            id: message
                .get("uuid")
                .and_then(Value::as_str)
                .map(ToString::to_string)
                .unwrap_or_else(|| format!("{run_id}::{kind}::{index}")),
            run_id: run_id.to_string(),
            thread_id: thread_id.to_string(),
            semantic_object: "Context".to_string(),
            kind: kind.to_string(),
            source: "stream".to_string(),
            at: observed_at.to_string(),
            payload: message.clone(),
        });
    }

    for (index, hook_event) in artifact.hook_events.iter().enumerate() {
        if !matches!(
            hook_event.hook.as_str(),
            "PreCompact"
                | "PostCompact"
                | "InstructionsLoaded"
                | "ConfigChange"
                | "WorktreeCreate"
                | "WorktreeRemove"
        ) {
            continue;
        }

        events.push(ManagedSemanticRuntimeEvent {
            id: hook_event
                .input
                .get("file_path")
                .or_else(|| hook_event.input.get("worktree_path"))
                .and_then(Value::as_str)
                .map(ToString::to_string)
                .unwrap_or_else(|| format!("{run_id}::{}::{index}", hook_event.hook)),
            run_id: run_id.to_string(),
            thread_id: thread_id.to_string(),
            semantic_object: "Context".to_string(),
            kind: hook_event.hook.to_string(),
            source: "hook".to_string(),
            at: observed_at.to_string(),
            payload: hook_event.input.clone(),
        });
    }

    events
}

fn build_provenance_parameters(
    system_init: &ManagedSystemInit,
    artifact: &ClaudeManagedArtifact,
) -> Value {
    json!({
        "cwd": system_init.cwd,
        "permissionMode": system_init.permission_mode,
        "tools": system_init.tools,
        "apiKeySource": system_init.api_key_source,
        "claudeCodeVersion": system_init.claude_code_version,
        "outputStyle": system_init.output_style,
        "agents": system_init.agents,
        "skills": system_init.skills,
        "slashCommands": system_init.slash_commands,
        "mcpServers": system_init.mcp_servers,
        "plugins": system_init.plugins,
        "fastModeState": system_init.fast_mode_state,
        "durationMs": artifact.result_message.as_ref().and_then(|result| result.duration_ms),
        "durationApiMs": artifact.result_message.as_ref().and_then(|result| result.duration_api_ms),
        "numTurns": artifact.result_message.as_ref().and_then(|result| result.num_turns),
        "stopReason": artifact.result_message.as_ref().and_then(|result| result.stop_reason.clone()),
        "totalCostUsd": artifact.result_message.as_ref().and_then(|result| result.total_cost_usd),
    })
}

fn managed_run_status(artifact: &ClaudeManagedArtifact) -> String {
    match artifact.result_message.as_ref() {
        Some(result)
            if result.is_error == Some(true)
                || matches!(result.subtype.as_deref(), Some("error" | "failed")) =>
        {
            "failed".to_string()
        }
        Some(_) => "completed".to_string(),
        None if artifact.helper_timed_out => "timed_out".to_string(),
        None if artifact.helper_error.is_some() => "failed".to_string(),
        None => "running".to_string(),
    }
}

fn managed_run_error(artifact: &ClaudeManagedArtifact) -> Option<String> {
    if let Some(result) = artifact.result_message.as_ref() {
        if result.is_error != Some(true)
            && !matches!(result.subtype.as_deref(), Some("error" | "failed"))
        {
            return artifact.helper_error.clone();
        }
        return result
            .result
            .clone()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| Some("managed Claude SDK run reported an error".to_string()));
    }
    artifact.helper_error.clone()
}

fn append_tool_event(session: &mut SessionState, tool_event: &ToolHookPair) {
    let entry = session
        .metadata
        .entry("tool_events".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    let Value::Array(items) = entry else {
        session.metadata.insert(
            "tool_events".to_string(),
            Value::Array(vec![json!({
                "tool_use_id": tool_event.tool_use_id,
                "name": tool_event.tool_name,
                "input": tool_event.tool_input,
                "response": tool_event.tool_response,
                "transcript_path": tool_event.transcript_path,
                "timestamp": Utc::now().to_rfc3339(),
            })]),
        );
        return;
    };

    items.push(json!({
        "tool_use_id": tool_event.tool_use_id,
        "name": tool_event.tool_name,
        "input": tool_event.tool_input,
        "response": tool_event.tool_response,
        "transcript_path": tool_event.transcript_path,
        "timestamp": Utc::now().to_rfc3339(),
    }));
}

fn append_normalized_event(session: &mut SessionState, event: Value) {
    let entry = session
        .metadata
        .entry(NORMALIZED_EVENTS_KEY.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    let Value::Array(items) = entry else {
        session
            .metadata
            .insert(NORMALIZED_EVENTS_KEY.to_string(), Value::Array(vec![event]));
        return;
    };
    items.push(event);
}

fn build_bridge_session(artifact: &ClaudeManagedArtifact) -> Result<SessionState> {
    let system_init = extract_system_init(artifact)
        .context("managed artifact does not contain a valid system init message")?;
    let provider_session_id = system_init.session_id.clone();
    let mut session = SessionState::new(&system_init.cwd);
    session.id = build_ai_session_id("claude", &provider_session_id);
    session.working_dir = system_init.cwd.clone();
    session.summary = artifact
        .result_message
        .as_ref()
        .and_then(|result| result.structured_output.as_ref())
        .and_then(|output| output.get("summary"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    session
        .metadata
        .insert("provider".to_string(), json!("claude"));
    session.metadata.insert(
        "provider_session_id".to_string(),
        json!(provider_session_id.clone()),
    );
    session
        .metadata
        .insert("managed_source".to_string(), json!(MANAGED_SOURCE_NAME));

    if let Some(model) = &system_init.model {
        session.metadata.insert("model".to_string(), json!(model));
    }
    if let Some(permission_mode) = &system_init.permission_mode {
        session
            .metadata
            .insert("permission_mode".to_string(), json!(permission_mode));
    }
    if !system_init.tools.is_empty() {
        session
            .metadata
            .insert("available_tools".to_string(), json!(system_init.tools));
    }

    if let Some(transcript_path) = artifact.transcript_path() {
        session
            .metadata
            .insert("transcript_path".to_string(), json!(transcript_path));
    }

    append_normalized_event(
        &mut session,
        json!({
            "provider": "claude",
            "kind": "session_start",
            "timestamp": Utc::now().to_rfc3339(),
            "prompt": Value::Null,
            "tool_name": Value::Null,
            "assistant_message": Value::Null,
            "has_model": system_init.model.is_some(),
            "has_tool_input": false,
            "has_tool_response": false,
        }),
    );

    append_raw_hook_events(&mut session, &artifact.hook_events);

    if let Some(prompt) = artifact.prompt_text() {
        session.add_user_message(&prompt);
        append_normalized_event(
            &mut session,
            json!({
                "provider": "claude",
                "kind": "turn_start",
                "timestamp": Utc::now().to_rfc3339(),
                "prompt": prompt,
                "tool_name": Value::Null,
                "assistant_message": Value::Null,
                "has_model": false,
                "has_tool_input": false,
                "has_tool_response": false,
            }),
        );
    }

    for assistant_text in artifact.assistant_text_messages() {
        session.add_assistant_message(&assistant_text);
        session
            .metadata
            .insert("last_assistant_message".to_string(), json!(assistant_text));
    }

    for tool_event in merge_tool_hook_events(&artifact.hook_events) {
        append_tool_event(&mut session, &tool_event);
        append_normalized_event(
            &mut session,
            json!({
                "provider": "claude",
                "kind": "tool_use",
                "timestamp": Utc::now().to_rfc3339(),
                "prompt": Value::Null,
                "tool_name": tool_event.tool_name,
                "assistant_message": Value::Null,
                "has_model": false,
                "has_tool_input": tool_event.tool_input.is_some(),
                "has_tool_response": tool_event.tool_response.is_some(),
            }),
        );
    }

    if artifact.has_stop_hook() {
        let last_assistant_message = session
            .metadata
            .get("last_assistant_message")
            .cloned()
            .unwrap_or(Value::Null);
        append_normalized_event(
            &mut session,
            json!({
                "provider": "claude",
                "kind": "turn_end",
                "timestamp": Utc::now().to_rfc3339(),
                "prompt": Value::Null,
                "tool_name": Value::Null,
                "assistant_message": last_assistant_message,
                "has_model": false,
                "has_tool_input": false,
                "has_tool_response": false,
            }),
        );
    }

    if let Some(result) = &artifact.result_message {
        if let Some(usage) = &result.usage {
            session.metadata.insert("usage".to_string(), usage.clone());
        }
        if let Some(total_cost_usd) = result.total_cost_usd {
            session
                .metadata
                .insert("total_cost_usd".to_string(), json!(total_cost_usd));
        }
        if let Some(duration_ms) = result.duration_ms {
            session
                .metadata
                .insert("duration_ms".to_string(), json!(duration_ms));
        }
        if let Some(stop_reason) = &result.stop_reason {
            session
                .metadata
                .insert("stop_reason".to_string(), json!(stop_reason));
        }

        append_normalized_event(
            &mut session,
            json!({
                "provider": "claude",
                "kind": "session_end",
                "timestamp": Utc::now().to_rfc3339(),
                "prompt": Value::Null,
                "tool_name": Value::Null,
                "assistant_message": Value::Null,
                "has_model": false,
                "has_tool_input": false,
                "has_tool_response": false,
            }),
        );
        session
            .metadata
            .insert(SESSION_PHASE_METADATA_KEY.to_string(), json!("ended"));
    } else if artifact.helper_timed_out || artifact.helper_error.is_some() {
        session
            .metadata
            .insert(SESSION_PHASE_METADATA_KEY.to_string(), json!("stopped"));
    } else {
        session
            .metadata
            .insert(SESSION_PHASE_METADATA_KEY.to_string(), json!("active"));
    }

    if artifact.helper_timed_out {
        session
            .metadata
            .insert("managed_helper_timed_out".to_string(), json!(true));
    }
    if let Some(helper_error) = &artifact.helper_error {
        session
            .metadata
            .insert("managed_helper_error".to_string(), json!(helper_error));
    }

    session.metadata.insert(
        "provider_runtime".to_string(),
        build_provider_runtime_metadata(&session, &system_init, artifact),
    );

    Ok(session)
}

fn build_provider_runtime_metadata(
    session: &SessionState,
    system_init: &ManagedSystemInit,
    artifact: &ClaudeManagedArtifact,
) -> Value {
    let thread_id = session.id.clone();
    let run_id = format!("{thread_id}::run");
    let observed_at = session.updated_at.to_rfc3339();

    json!({
        "providerInit": build_provider_init_snapshot(&thread_id, &run_id, &observed_at, system_init),
        "taskRuntimeEvents": build_task_runtime_events(&thread_id, &run_id, &observed_at, artifact),
        "decisionRuntimeEvents": build_decision_runtime_events(&thread_id, &run_id, &observed_at, artifact),
        "contextRuntimeEvents": build_context_runtime_events(&thread_id, &run_id, &observed_at, artifact),
    })
}

fn extract_intent_extraction_outcome(
    result_message: Option<&ClaudeManagedResultMessage>,
) -> IntentExtractionOutcome {
    let Some(result_message) = result_message else {
        return IntentExtractionOutcome {
            extraction: None,
            error: None,
        };
    };
    let Some(structured_output) = result_message.structured_output.clone() else {
        return IntentExtractionOutcome {
            extraction: None,
            error: None,
        };
    };

    match serde_json::from_value::<StructuredIntentExtractionOutput>(structured_output) {
        Ok(output) => IntentExtractionOutcome {
            extraction: Some(IntentDraft {
                intent: DraftIntent {
                    summary: output.summary,
                    problem_statement: output.problem_statement,
                    change_type: output.change_type,
                    objectives: output.objectives,
                    in_scope: output.in_scope,
                    out_of_scope: output.out_of_scope,
                    touch_hints: output.touch_hints,
                },
                acceptance: DraftAcceptance {
                    success_criteria: output.success_criteria,
                    fast_checks: output.fast_checks,
                    integration_checks: output.integration_checks,
                    security_checks: output.security_checks,
                    release_checks: output.release_checks,
                },
                risk: DraftRisk {
                    rationale: output.risk_rationale,
                    factors: output.risk_factors,
                    level: output.risk_level,
                },
            }),
            error: None,
        },
        Err(err) => IntentExtractionOutcome {
            extraction: None,
            error: Some(format!(
                "managed result structured_output does not match the intent extraction bridge schema: {err}"
            )),
        },
    }
}

fn build_managed_ai_session_payload(session: &SessionState) -> Value {
    let events = session
        .metadata
        .get(NORMALIZED_EVENTS_KEY)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let raw_events = session
        .metadata
        .get(RAW_HOOK_EVENTS_KEY)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let phase = session
        .metadata
        .get(SESSION_PHASE_METADATA_KEY)
        .and_then(Value::as_str)
        .unwrap_or("active");
    let provider_session_id = session
        .metadata
        .get("provider_session_id")
        .and_then(Value::as_str)
        .unwrap_or(&session.id);
    let transcript_path = session
        .metadata
        .get("transcript_path")
        .and_then(Value::as_str);
    let last_assistant_message = session
        .metadata
        .get("last_assistant_message")
        .and_then(Value::as_str);

    json!({
        "schema": AI_SESSION_SCHEMA,
        "object_type": AI_SESSION_TYPE,
        "provider": "claude",
        "ai_session_id": session.id,
        "provider_session_id": provider_session_id,
        "state_machine": {
            "phase": phase,
            "status": phase_status_label(phase),
            "event_count": events.len(),
            "tool_use_count": count_events(&events, "tool_use"),
            "compaction_count": count_events(&events, "compaction"),
            "started_at": first_event_timestamp(&events, "session_start"),
            "ended_at": first_event_timestamp(&events, "session_end"),
            "updated_at": session.updated_at.to_rfc3339(),
        },
        "summary": {
            "message_count": session.messages.len(),
            "user_message_count": session.messages.iter().filter(|message| message.role == "user").count(),
            "assistant_message_count": session.messages.iter().filter(|message| message.role == "assistant").count(),
            "last_assistant_message": last_assistant_message,
        },
        "transcript": {
            "path": transcript_path,
            "raw_event_count": raw_events.len(),
        },
        "events": events,
        "raw_hook_events": raw_events,
        "session": session,
        "ingest_meta": {
            "source": MANAGED_SOURCE_NAME,
            "provider": "claude",
            "ingested_at": Utc::now().to_rfc3339(),
        }
    })
}

fn phase_status_label(phase: &str) -> &'static str {
    match phase {
        "active" => "running",
        "stopped" => "idle",
        "ended" => "ended",
        _ => "running",
    }
}

fn count_events(events: &[Value], kind: &str) -> usize {
    events
        .iter()
        .filter(|value| value.get("kind").and_then(Value::as_str) == Some(kind))
        .count()
}

fn first_event_timestamp(events: &[Value], kind: &str) -> Option<String> {
    events
        .iter()
        .find(|value| value.get("kind").and_then(Value::as_str) == Some(kind))
        .and_then(|value| value.get("timestamp"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn collect_touch_hints(tool_invocations: &[ManagedToolInvocation], cwd: &str) -> Vec<String> {
    let mut hints = Vec::new();
    let mut seen = HashSet::new();

    for tool in tool_invocations {
        for candidate in extract_file_candidates(tool) {
            let normalized = normalize_hint_path(&candidate, cwd);
            if normalized.is_empty() || !seen.insert(normalized.clone()) {
                continue;
            }
            hints.push(normalized);
        }
    }

    hints
}

fn extract_file_candidates(tool: &ManagedToolInvocation) -> Vec<String> {
    let mut candidates = Vec::new();

    if let Some(input) = &tool.tool_input
        && let Some(file_path) = input.get("file_path").and_then(Value::as_str)
    {
        candidates.push(file_path.to_string());
    }

    if let Some(response) = &tool.tool_response
        && let Some(file_path) = response
            .get("file")
            .and_then(|value| value.get("filePath"))
            .and_then(Value::as_str)
    {
        candidates.push(file_path.to_string());
    }

    candidates
}

fn normalize_hint_path(path: &str, cwd: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed == cwd {
        return String::new();
    }

    if let Some(relative) = trimmed.strip_prefix(cwd) {
        return relative.trim_start_matches('/').to_string();
    }

    trimmed.to_string()
}

fn build_field_provenance(
    system_init: &ManagedSystemInit,
    artifact: &ClaudeManagedArtifact,
    extraction_outcome: &IntentExtractionOutcome,
    tool_invocations: &[ManagedToolInvocation],
    touch_hints: &[String],
) -> Result<Vec<ManagedFieldProvenance>> {
    let mut entries = Vec::new();

    push_field_provenance(
        &mut entries,
        "meta.providerSessionId",
        "system(init)",
        "$.messages[type=system,subtype=init].session_id",
        json!(system_init.session_id),
        None,
    );
    push_field_provenance(
        &mut entries,
        "meta.cwd",
        "system(init)",
        "$.messages[type=system,subtype=init].cwd",
        json!(system_init.cwd),
        None,
    );
    if let Some(model) = &system_init.model {
        push_field_provenance(
            &mut entries,
            "meta.model",
            "system(init)",
            "$.messages[type=system,subtype=init].model",
            json!(model),
            None,
        );
    }
    if let Some(permission_mode) = &system_init.permission_mode {
        push_field_provenance(
            &mut entries,
            "meta.permissionMode",
            "system(init)",
            "$.messages[type=system,subtype=init].permissionMode",
            json!(permission_mode),
            None,
        );
    }
    if !system_init.tools.is_empty() {
        push_field_provenance(
            &mut entries,
            "meta.availableTools",
            "system(init)",
            "$.messages[type=system,subtype=init].tools",
            json!(system_init.tools),
            None,
        );
    }
    if let Some(api_key_source) = &system_init.api_key_source {
        push_field_provenance(
            &mut entries,
            "meta.apiKeySource",
            "system(init)",
            "$.messages[type=system,subtype=init].apiKeySource",
            json!(api_key_source),
            Some("Provider-native runtime fact for auth/provenance classification.".to_string()),
        );
    }
    if let Some(claude_code_version) = &system_init.claude_code_version {
        push_field_provenance(
            &mut entries,
            "meta.claudeCodeVersion",
            "system(init)",
            "$.messages[type=system,subtype=init].claude_code_version",
            json!(claude_code_version),
            Some(
                "Provider-native runtime fact for provider capability/runtime debugging."
                    .to_string(),
            ),
        );
    }
    if let Some(output_style) = &system_init.output_style {
        push_field_provenance(
            &mut entries,
            "meta.outputStyle",
            "system(init)",
            "$.messages[type=system,subtype=init].output_style",
            json!(output_style),
            Some("Maps to provider capability/runtime presentation facts rather than formal intent semantics.".to_string()),
        );
    }
    if !system_init.skills.is_empty() {
        push_field_provenance(
            &mut entries,
            "meta.skills",
            "system(init)",
            "$.messages[type=system,subtype=init].skills",
            json!(system_init.skills),
            Some(
                "Provider-native runtime fact; supports provenance/capability reconstruction."
                    .to_string(),
            ),
        );
    }
    if !system_init.agents.is_empty() {
        push_field_provenance(
            &mut entries,
            "meta.agents",
            "system(init)",
            "$.messages[type=system,subtype=init].agents",
            json!(system_init.agents),
            Some(
                "Provider-native runtime fact; supports Task/subagent runtime mapping.".to_string(),
            ),
        );
    }
    if let Some(transcript_path) = artifact.transcript_path() {
        push_field_provenance(
            &mut entries,
            "transcript.path",
            "hooks",
            "$.hookEvents[*].input.transcript_path",
            json!(transcript_path),
            None,
        );
    }

    if let Some(result) = &artifact.result_message {
        if let Some(usage) = &result.usage {
            push_field_provenance(
                &mut entries,
                "usage",
                "result",
                "$.resultMessage.usage",
                usage.clone(),
                None,
            );
        }
        if let Some(total_cost_usd) = result.total_cost_usd {
            push_field_provenance(
                &mut entries,
                "usage.totalCostUsd",
                "result",
                "$.resultMessage.total_cost_usd",
                json!(total_cost_usd),
                None,
            );
        }
        if let Some(duration_ms) = result.duration_ms {
            push_field_provenance(
                &mut entries,
                "usage.durationMs",
                "result",
                "$.resultMessage.duration_ms",
                json!(duration_ms),
                None,
            );
        }
        if let Some(stop_reason) = &result.stop_reason {
            push_field_provenance(
                &mut entries,
                "usage.stopReason",
                "result",
                "$.resultMessage.stop_reason",
                json!(stop_reason),
                None,
            );
        }
    }

    if let Some(extraction) = &extraction_outcome.extraction {
        push_field_provenance(
            &mut entries,
            "intent.summary",
            "result.structured_output",
            "$.resultMessage.structured_output.summary",
            json!(extraction.intent.summary),
            None,
        );
        push_field_provenance(
            &mut entries,
            "intent.problemStatement",
            "result.structured_output",
            "$.resultMessage.structured_output.problemStatement",
            json!(extraction.intent.problem_statement),
            None,
        );
        push_field_provenance(
            &mut entries,
            "intent.changeType",
            "result.structured_output",
            "$.resultMessage.structured_output.changeType",
            json!(extraction.intent.change_type),
            None,
        );
        push_field_provenance(
            &mut entries,
            "intent.objectives",
            "result.structured_output",
            "$.resultMessage.structured_output.objectives",
            serde_json::to_value(&extraction.intent.objectives)
                .context("failed to serialize managed objectives provenance")?,
            None,
        );
        push_field_provenance(
            &mut entries,
            "intent.inScope",
            "result.structured_output",
            "$.resultMessage.structured_output.inScope",
            serde_json::to_value(&extraction.intent.in_scope)
                .context("failed to serialize managed inScope provenance")?,
            None,
        );
        push_field_provenance(
            &mut entries,
            "intent.outOfScope",
            "result.structured_output",
            "$.resultMessage.structured_output.outOfScope",
            serde_json::to_value(&extraction.intent.out_of_scope)
                .context("failed to serialize managed outOfScope provenance")?,
            None,
        );
        push_field_provenance(
            &mut entries,
            "acceptance.successCriteria",
            "result.structured_output",
            "$.resultMessage.structured_output.successCriteria",
            serde_json::to_value(&extraction.acceptance.success_criteria)
                .context("failed to serialize managed successCriteria provenance")?,
            None,
        );
        push_field_provenance(
            &mut entries,
            "risk.rationale",
            "result.structured_output",
            "$.resultMessage.structured_output.riskRationale",
            json!(extraction.risk.rationale),
            None,
        );
        if !extraction.risk.factors.is_empty() {
            push_field_provenance(
                &mut entries,
                "risk.factors",
                "result.structured_output",
                "$.resultMessage.structured_output.riskFactors",
                serde_json::to_value(&extraction.risk.factors)
                    .context("failed to serialize managed riskFactors provenance")?,
                None,
            );
        }
        if let Some(level) = &extraction.risk.level {
            push_field_provenance(
                &mut entries,
                "risk.level",
                "result.structured_output",
                "$.resultMessage.structured_output.riskLevel",
                json!(level),
                None,
            );
        }
    }

    if !tool_invocations.is_empty() {
        push_field_provenance(
            &mut entries,
            "evidence.toolInvocations",
            "hooks",
            "$.hookEvents[hook=PreToolUse|PostToolUse]",
            serde_json::to_value(tool_invocations)
                .context("failed to serialize managed tool invocation provenance")?,
            Some("Merged by tool_use_id from paired PreToolUse/PostToolUse hooks.".to_string()),
        );
    }

    if !touch_hints.is_empty() {
        push_field_provenance(
            &mut entries,
            "intent.touchHints",
            "hooks+tool_evidence",
            "$.hookEvents[*].input.tool_input.file_path | $.hookEvents[*].input.tool_response.file.filePath",
            serde_json::to_value(touch_hints)
                .context("failed to serialize managed touchHints provenance")?,
            Some("Derived from explicit file targets observed in tool input/response.".to_string()),
        );
    }

    let task_runtime_events =
        build_task_runtime_events("thread", "run", &Utc::now().to_rfc3339(), artifact);
    if !task_runtime_events.is_empty() {
        push_field_provenance(
            &mut entries,
            "runtime.taskEvents",
            "stream+hooks",
            "$.messages[subtype=task_*] | $.hookEvents[hook=SubagentStart|SubagentStop|TaskCompleted|TeammateIdle]",
            serde_json::to_value(&task_runtime_events)
                .context("failed to serialize managed task runtime provenance")?,
            Some("Provider-native runtime facts that can currently map onto Task/subagent lifecycle, but are not formal Task snapshots.".to_string()),
        );
    }

    let tool_runtime_events =
        build_tool_runtime_events("thread", "run", &Utc::now().to_rfc3339(), artifact);
    if !tool_runtime_events.is_empty() {
        push_field_provenance(
            &mut entries,
            "runtime.toolEvents",
            "stream",
            "$.messages[type=tool_progress|tool_use_summary]",
            serde_json::to_value(&tool_runtime_events)
                .context("failed to serialize managed tool runtime provenance")?,
            Some("Provider-native tool execution progress and summary events; these complement hook-based tool invocation facts.".to_string()),
        );
    }

    let assistant_runtime_events =
        build_assistant_runtime_events("thread", "run", &Utc::now().to_rfc3339(), artifact);
    if !assistant_runtime_events.is_empty() {
        push_field_provenance(
            &mut entries,
            "runtime.assistantEvents",
            "stream",
            "$.messages[type=stream_event]",
            serde_json::to_value(&assistant_runtime_events)
                .context("failed to serialize managed assistant runtime provenance")?,
            Some("Provider-native partial assistant stream events; these are raw incremental output facts, not finalized assistant messages.".to_string()),
        );
    }

    let decision_runtime_events =
        build_decision_runtime_events("thread", "run", &Utc::now().to_rfc3339(), artifact);
    if !decision_runtime_events.is_empty() {
        push_field_provenance(
            &mut entries,
            "runtime.decisionEvents",
            "hooks+result",
            "$.hookEvents[hook=PermissionRequest|CanUseTool|Elicitation|ElicitationResult] | $.resultMessage.permission_denials",
            serde_json::to_value(&decision_runtime_events)
                .context("failed to serialize managed decision runtime provenance")?,
            Some("Provider-native runtime facts for permission/human-gate surfaces; they are pre-decision evidence, not formal Decision objects.".to_string()),
        );
    }

    let context_runtime_events =
        build_context_runtime_events("thread", "run", &Utc::now().to_rfc3339(), artifact);
    if !context_runtime_events.is_empty() {
        push_field_provenance(
            &mut entries,
            "runtime.contextEvents",
            "stream+hooks",
            "$.messages[type=system,subtype=status|compact_boundary|files_persisted] | $.messages[type=rate_limit_event|prompt_suggestion] | $.hookEvents[hook=PreCompact|PostCompact|InstructionsLoaded|ConfigChange|WorktreeCreate|WorktreeRemove]",
            serde_json::to_value(&context_runtime_events)
                .context("failed to serialize managed context runtime provenance")?,
            Some("Provider-native runtime facts for context maintenance and environment mutation; these support ContextFrame/ContextSnapshot reasoning later.".to_string()),
        );
    }

    Ok(entries)
}

fn push_field_provenance(
    entries: &mut Vec<ManagedFieldProvenance>,
    field_path: &str,
    source_layer: &str,
    source_path: &str,
    value: Value,
    note: Option<String>,
) {
    entries.push(ManagedFieldProvenance {
        field_path: field_path.to_string(),
        source_layer: source_layer.to_string(),
        source_path: source_path.to_string(),
        value,
        note,
    });
}

impl IntentExtractionOutcome {
    fn status_label(&self) -> &'static str {
        if self.extraction.is_some() {
            "accepted"
        } else if self.error.is_some() {
            "invalid"
        } else {
            "missing"
        }
    }
}

impl PersistedManagedIntentExtraction {
    fn new(ai_session_id: String, extraction: IntentDraft) -> Self {
        Self {
            schema: "libra.intent_extraction.v1".to_string(),
            ai_session_id,
            source: MANAGED_INTENT_EXTRACTION_SOURCE.to_string(),
            extraction,
        }
    }
}

fn write_pretty_json_artifact<T>(directory: &Path, artifact_id: &str, value: &T) -> Result<PathBuf>
where
    T: Serialize,
{
    fs::create_dir_all(directory).with_context(|| {
        format!(
            "failed to create managed artifact directory '{}'",
            directory.display()
        )
    })?;
    let destination = directory.join(format!("{artifact_id}.json"));
    let payload =
        serde_json::to_vec_pretty(value).context("failed to serialize managed JSON artifact")?;
    write_atomic_file(&destination, &payload)?;
    Ok(destination)
}

fn delete_generated_artifact_if_exists(directory: &Path, artifact_id: &str) -> Result<()> {
    let destination = directory.join(format!("{artifact_id}.json"));
    match fs::remove_file(&destination) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| {
            format!(
                "failed to remove stale managed artifact '{}'",
                destination.display()
            )
        }),
    }
}

fn write_atomic_file(destination: &Path, data: &[u8]) -> Result<()> {
    let parent = destination.parent().ok_or_else(|| {
        anyhow!(
            "managed artifact path '{}' does not have a parent directory",
            destination.display()
        )
    })?;
    fs::create_dir_all(parent)
        .with_context(|| format!("failed to create parent directory '{}'", parent.display()))?;

    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            anyhow!(
                "managed artifact path '{}' does not have a valid file name",
                destination.display()
            )
        })?;
    let unique_suffix = format!(
        "{}.{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let temp_path = parent.join(format!(".{file_name}.{unique_suffix}.tmp"));

    fs::write(&temp_path, data).with_context(|| {
        format!(
            "failed to write temporary managed artifact '{}'",
            temp_path.display()
        )
    })?;

    #[cfg(windows)]
    {
        if destination.exists() {
            match fs::remove_file(destination) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => {
                    let _ = fs::remove_file(&temp_path);
                    return Err(err).with_context(|| {
                        format!(
                            "failed to replace existing managed artifact '{}'",
                            destination.display()
                        )
                    });
                }
            }
        }
    }

    fs::rename(&temp_path, destination)
        .inspect_err(|_err| {
            let _ = fs::remove_file(&temp_path);
        })
        .with_context(|| {
            format!(
                "failed to finalize managed artifact '{}' -> '{}'",
                temp_path.display(),
                destination.display()
            )
        })?;

    Ok(())
}

impl From<ToolHookPair> for ManagedToolInvocation {
    fn from(value: ToolHookPair) -> Self {
        Self {
            tool_use_id: value.tool_use_id,
            tool_name: value.tool_name,
            tool_input: value.tool_input,
            tool_response: value.tool_response,
            transcript_path: value.transcript_path,
        }
    }
}

impl ManagedToolInvocationEvent {
    fn from_tool_hook_pair(thread_id: &str, run_id: &str, at: &str, value: ToolHookPair) -> Self {
        let status = if value.saw_post {
            "completed"
        } else if value.saw_pre {
            "in_progress"
        } else {
            "pending"
        };

        Self {
            id: value.tool_use_id,
            run_id: run_id.to_string(),
            thread_id: thread_id.to_string(),
            tool: value.tool_name.unwrap_or_else(|| "unknown".to_string()),
            server: None,
            status: status.to_string(),
            at: at.to_string(),
            payload: json!({
                "input": value.tool_input,
                "response": value.tool_response,
                "transcriptPath": value.transcript_path,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingest_managed_artifact_builds_bridge_session_and_draft() {
        let artifact: ClaudeManagedArtifact = serde_json::from_value(json!({
            "cwd": "/repo",
            "prompt": "Implement the managed mode bridge",
            "hookEvents": [
                {
                    "hook": "UserPromptSubmit",
                    "input": {
                        "session_id": "sdk-session-1",
                        "transcript_path": "/tmp/managed-transcript.jsonl",
                        "cwd": "/repo",
                        "permission_mode": "plan",
                        "hook_event_name": "UserPromptSubmit",
                        "prompt": "Implement the managed mode bridge"
                    }
                },
                {
                    "hook": "PreToolUse",
                    "input": {
                        "session_id": "sdk-session-1",
                        "transcript_path": "/tmp/managed-transcript.jsonl",
                        "cwd": "/repo",
                        "hook_event_name": "PreToolUse",
                        "tool_name": "Read",
                        "tool_input": {"file_path": "/repo/src/lib.rs"},
                        "tool_use_id": "tool-1"
                    }
                },
                {
                    "hook": "PostToolUse",
                    "input": {
                        "session_id": "sdk-session-1",
                        "transcript_path": "/tmp/managed-transcript.jsonl",
                        "cwd": "/repo",
                        "hook_event_name": "PostToolUse",
                        "tool_name": "Read",
                        "tool_input": {"file_path": "/repo/src/lib.rs"},
                        "tool_response": {"file":{"filePath":"/repo/src/lib.rs"}},
                        "tool_use_id": "tool-1"
                    }
                },
                {
                    "hook": "Stop",
                    "input": {
                        "session_id": "sdk-session-1",
                        "transcript_path": "/tmp/managed-transcript.jsonl",
                        "cwd": "/repo",
                        "hook_event_name": "Stop"
                    }
                }
            ],
            "messages": [
                {
                    "type": "system",
                    "subtype": "init",
                    "cwd": "/repo",
                    "session_id": "sdk-session-1",
                    "tools": ["Read", "StructuredOutput"],
                    "model": "claude-sonnet-4-5-20250929",
                    "permissionMode": "plan"
                },
                {
                    "type": "assistant",
                    "message": {
                        "role": "assistant",
                        "content": [
                            {"type": "text", "text": "I will inspect the repository and prepare the bridge."}
                        ]
                    }
                }
            ],
            "resultMessage": {
                "type": "result",
                "subtype": "success",
                "is_error": false,
                "session_id": "sdk-session-1",
                "stop_reason": "end_turn",
                "duration_ms": 1200,
                "duration_api_ms": 1000,
                "num_turns": 1,
                "result": "ok",
                "total_cost_usd": 0.02,
                "usage": {"input_tokens": 100, "output_tokens": 50},
                "modelUsage": {"claude-sonnet-4-5-20250929": {"inputTokens": 100, "outputTokens": 50}},
                "permission_denials": [],
                "structured_output": {
                    "summary": "Build the managed mode bridge",
                    "problemStatement": "Libra needs a stable SDK-managed ingestion bridge",
                    "changeType": "feature",
                    "objectives": ["Bridge SDK events", "Persist ai_session"],
                    "inScope": ["src/internal/ai/providers/claude_sdk"],
                    "outOfScope": ["UI redesign"],
                    "successCriteria": ["Session is persisted", "Intent extraction is derived"],
                    "riskRationale": "Low risk because the bridge is additive",
                    "riskFactors": ["new adapter path"],
                    "riskLevel": "low"
                },
                "fast_mode_state": null,
                "uuid": "result-1"
            }
        }))
        .expect("fixture should deserialize");

        let ingested = ingest_managed_artifact(&artifact).expect("ingestion should succeed");

        assert_eq!(ingested.session.id, "claude__sdk-session-1");
        assert_eq!(ingested.session.summary, "Build the managed mode bridge");
        assert_eq!(
            ingested.session.metadata.get("transcript_path"),
            Some(&json!("/tmp/managed-transcript.jsonl"))
        );
        assert_eq!(
            ingested.session.metadata.get("session_phase"),
            Some(&json!("ended"))
        );
        assert_eq!(ingested.session.messages.len(), 2);
        assert_eq!(ingested.session.messages[0].role, "user");
        assert_eq!(ingested.session.messages[1].role, "assistant");
        assert_eq!(
            ingested
                .session
                .metadata
                .get("tool_events")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(1)
        );
        assert_eq!(
            ingested
                .session
                .metadata
                .get("tool_events")
                .and_then(Value::as_array)
                .and_then(|items| items.first())
                .and_then(|value| value.get("tool_use_id"))
                .and_then(Value::as_str),
            Some("tool-1")
        );
        assert_eq!(
            ingested
                .session
                .metadata
                .get(NORMALIZED_EVENTS_KEY)
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(5)
        );

        let extraction = ingested
            .intent_extraction
            .expect("intent extraction should be present");
        assert_eq!(extraction.intent.summary, "Build the managed mode bridge");
        assert_eq!(extraction.intent.change_type, ChangeType::Feature);
        assert_eq!(extraction.acceptance.success_criteria.len(), 2);
        assert_eq!(extraction.risk.level, Some(RiskLevel::Low));
    }

    #[test]
    fn extract_intent_extraction_from_result_returns_none_when_output_missing() {
        let result = ClaudeManagedResultMessage {
            r#type: Some("result".to_string()),
            subtype: Some("success".to_string()),
            is_error: Some(false),
            session_id: Some("sdk-session-2".to_string()),
            stop_reason: Some("end_turn".to_string()),
            duration_ms: None,
            duration_api_ms: None,
            num_turns: None,
            result: None,
            total_cost_usd: None,
            usage: None,
            model_usage: None,
            permission_denials: None,
            structured_output: None,
            fast_mode_state: None,
            uuid: None,
        };

        let extraction = extract_intent_extraction_from_result(Some(&result))
            .expect("extraction should succeed");
        assert!(extraction.is_none());
    }

    #[test]
    fn structured_output_accepts_optional_future_fields() {
        let result = ClaudeManagedResultMessage {
            r#type: Some("result".to_string()),
            subtype: Some("success".to_string()),
            is_error: Some(false),
            session_id: Some("sdk-session-3".to_string()),
            stop_reason: Some("end_turn".to_string()),
            duration_ms: Some(10),
            duration_api_ms: Some(8),
            num_turns: Some(1),
            result: Some("ok".to_string()),
            total_cost_usd: Some(0.001),
            usage: Some(json!({"input_tokens": 1, "output_tokens": 1})),
            model_usage: None,
            permission_denials: None,
            structured_output: Some(json!({
                "summary": "Harden adapter",
                "problemStatement": "Need a stronger managed ingestion contract",
                "changeType": "security",
                "objectives": ["Validate fields"],
                "successCriteria": ["Contract stays strict"],
                "riskRationale": "Moderate because ingestion bugs can hide data",
                "fastChecks": [
                    {
                        "id": "unit",
                        "kind": "command",
                        "command": "cargo test managed",
                        "required": true,
                        "artifactsProduced": ["test-log"]
                    }
                ]
            })),
            fast_mode_state: None,
            uuid: None,
        };

        let extraction = extract_intent_extraction_from_result(Some(&result))
            .expect("extraction should succeed")
            .expect("extraction should exist");

        assert_eq!(extraction.intent.change_type, ChangeType::Security);
        assert_eq!(extraction.acceptance.fast_checks.len(), 1);
        assert_eq!(
            extraction.acceptance.fast_checks[0].kind,
            crate::internal::ai::intentspec::types::CheckKind::Command
        );
        assert!(extraction.intent.in_scope.is_empty());
        assert!(extraction.intent.out_of_scope.is_empty());
    }

    #[test]
    fn build_managed_audit_bundle_includes_ai_session_and_provenance() {
        let artifact: ClaudeManagedArtifact = serde_json::from_value(json!({
            "cwd": "/repo",
            "prompt": "Implement the managed mode bridge",
            "hookEvents": [
                {
                    "hook": "UserPromptSubmit",
                    "input": {
                        "session_id": "sdk-session-1",
                        "transcript_path": "/tmp/managed-transcript.jsonl",
                        "cwd": "/repo",
                        "permission_mode": "plan",
                        "hook_event_name": "UserPromptSubmit",
                        "prompt": "Implement the managed mode bridge"
                    }
                },
                {
                    "hook": "PreToolUse",
                    "input": {
                        "session_id": "sdk-session-1",
                        "transcript_path": "/tmp/managed-transcript.jsonl",
                        "cwd": "/repo",
                        "hook_event_name": "PreToolUse",
                        "tool_name": "Read",
                        "tool_input": {"file_path": "/repo/src/lib.rs"},
                        "tool_use_id": "tool-1"
                    }
                },
                {
                    "hook": "PostToolUse",
                    "input": {
                        "session_id": "sdk-session-1",
                        "transcript_path": "/tmp/managed-transcript.jsonl",
                        "cwd": "/repo",
                        "hook_event_name": "PostToolUse",
                        "tool_name": "Read",
                        "tool_input": {"file_path": "/repo/src/lib.rs"},
                        "tool_response": {"file":{"filePath":"/repo/src/lib.rs"}},
                        "tool_use_id": "tool-1"
                    }
                },
                {
                    "hook": "Stop",
                    "input": {
                        "session_id": "sdk-session-1",
                        "transcript_path": "/tmp/managed-transcript.jsonl",
                        "cwd": "/repo",
                        "hook_event_name": "Stop"
                    }
                }
            ],
            "messages": [
                {
                    "type": "system",
                    "subtype": "init",
                    "cwd": "/repo",
                    "session_id": "sdk-session-1",
                    "tools": ["Read", "StructuredOutput"],
                    "model": "claude-sonnet-4-5-20250929",
                    "permissionMode": "plan"
                },
                {
                    "type": "assistant",
                    "message": {
                        "role": "assistant",
                        "content": [
                            {"type": "text", "text": "I will inspect the repository and prepare the bridge."}
                        ]
                    }
                }
            ],
            "resultMessage": {
                "type": "result",
                "subtype": "success",
                "is_error": false,
                "session_id": "sdk-session-1",
                "stop_reason": "end_turn",
                "duration_ms": 1200,
                "duration_api_ms": 1000,
                "num_turns": 1,
                "result": "ok",
                "total_cost_usd": 0.02,
                "usage": {"input_tokens": 100, "output_tokens": 50},
                "modelUsage": {"claude-sonnet-4-5-20250929": {"inputTokens": 100, "outputTokens": 50}},
                "permission_denials": [],
                "structured_output": {
                    "summary": "Build the managed mode bridge",
                    "problemStatement": "Libra needs a stable SDK-managed ingestion bridge",
                    "changeType": "feature",
                    "objectives": ["Bridge SDK events", "Persist ai_session"],
                    "inScope": ["src/internal/ai/providers/claude_sdk"],
                    "outOfScope": ["UI redesign"],
                    "successCriteria": ["Session is persisted", "Intent extraction is derived"],
                    "riskRationale": "Low risk because the bridge is additive",
                    "riskFactors": ["new adapter path"],
                    "riskLevel": "low"
                },
                "fast_mode_state": null,
                "uuid": "result-1"
            }
        }))
        .expect("fixture should deserialize");

        let bundle = build_managed_audit_bundle(&artifact).expect("bundle should build");

        assert_eq!(bundle.schema, MANAGED_AUDIT_BUNDLE_SCHEMA);
        assert_eq!(bundle.ai_session_id, "claude__sdk-session-1");
        assert_eq!(bundle.provider_session_id, "sdk-session-1");
        assert_eq!(bundle.bridge.intent_extraction.status, "accepted");
        assert_eq!(bundle.bridge.touch_hints, vec!["src/lib.rs".to_string()]);
        assert_eq!(bundle.bridge.tool_invocations.len(), 1);
        assert_eq!(bundle.bridge.tool_invocations[0].tool_use_id, "tool-1");
        assert_eq!(
            bundle.bridge.object_candidates.thread_id,
            "claude__sdk-session-1"
        );
        assert_eq!(
            bundle.bridge.object_candidates.run_snapshot.id,
            "claude__sdk-session-1::run"
        );
        assert_eq!(
            bundle.bridge.object_candidates.run_snapshot.started_at,
            bundle.bridge.session_state.created_at.to_rfc3339()
        );
        assert_eq!(
            bundle.bridge.object_candidates.run_event.status,
            "completed"
        );
        assert_eq!(
            bundle.bridge.object_candidates.run_event.at,
            bundle.bridge.session_state.updated_at.to_rfc3339()
        );
        assert_eq!(
            bundle.bridge.object_candidates.provenance_snapshot.provider,
            "claude"
        );
        assert_eq!(
            bundle
                .bridge
                .object_candidates
                .provenance_snapshot
                .created_at,
            bundle.bridge.session_state.created_at.to_rfc3339()
        );
        assert_eq!(
            bundle
                .bridge
                .object_candidates
                .provenance_snapshot
                .model
                .as_deref(),
            Some("claude-sonnet-4-5-20250929")
        );
        assert_eq!(
            bundle
                .bridge
                .object_candidates
                .run_usage_event
                .as_ref()
                .map(|event| event.run_id.as_str()),
            Some("claude__sdk-session-1::run")
        );
        assert_eq!(
            bundle.bridge.object_candidates.tool_invocation_events.len(),
            1
        );
        assert_eq!(
            bundle.bridge.object_candidates.tool_invocation_events[0].status,
            "completed"
        );
        assert_eq!(
            bundle.bridge.object_candidates.tool_invocation_events[0].at,
            bundle.bridge.session_state.updated_at.to_rfc3339()
        );
        assert_eq!(bundle.bridge.intent_extraction.status, "accepted");
        assert_eq!(
            bundle
                .bridge
                .intent_extraction_artifact
                .as_ref()
                .map(|artifact| artifact.schema.as_str()),
            Some("libra.intent_extraction.v1")
        );
        assert_eq!(bundle.bridge.ai_session["schema"], json!(AI_SESSION_SCHEMA));
        assert!(
            bundle.field_provenance.iter().any(|entry| {
                entry.field_path == "intent.summary"
                    && entry.source_layer == "result.structured_output"
                    && entry.value == json!("Build the managed mode bridge")
            }),
            "expected intent.summary provenance from structured_output"
        );
        assert!(
            bundle.field_provenance.iter().any(|entry| {
                entry.field_path == "intent.touchHints"
                    && entry.source_layer == "hooks+tool_evidence"
            }),
            "expected derived touchHints provenance"
        );
    }

    #[test]
    fn build_managed_audit_bundle_surfaces_invalid_structured_output_without_failing() {
        let artifact: ClaudeManagedArtifact = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/data/ai/claude_managed_probe_like.json"
        )))
        .expect("fixture should deserialize");

        let bundle = build_managed_audit_bundle(&artifact).expect("bundle should build");

        assert_eq!(
            bundle.provider_session_id,
            "6dcf708f-88f2-4e9d-be07-1fbb1ab1b5a8"
        );
        assert_eq!(
            bundle.ai_session_id,
            "claude__6dcf708f-88f2-4e9d-be07-1fbb1ab1b5a8"
        );
        assert_eq!(bundle.bridge.intent_extraction.status, "invalid");
        assert!(bundle.bridge.intent_extraction_artifact.is_none());
        assert_eq!(
            bundle.bridge.object_candidates.run_snapshot.id,
            "claude__6dcf708f-88f2-4e9d-be07-1fbb1ab1b5a8::run"
        );
        assert_eq!(
            bundle.bridge.object_candidates.run_event.status,
            "completed"
        );
        assert_eq!(
            bundle.bridge.object_candidates.tool_invocation_events.len(),
            3
        );
        assert!(
            bundle
                .bridge
                .intent_extraction
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("intent extraction bridge schema")
        );
        assert_eq!(bundle.bridge.tool_invocations.len(), 3);
        assert!(
            bundle
                .bridge
                .touch_hints
                .contains(&"package.json".to_string()),
            "expected package.json hint from probe-like Read tool evidence"
        );
        assert_eq!(bundle.bridge.ai_session["schema"], json!(AI_SESSION_SCHEMA));
        assert!(
            bundle.field_provenance.iter().any(|entry| {
                entry.field_path == "usage.durationMs" && entry.value == json!(3479)
            }),
            "expected duration provenance from result payload"
        );
    }

    #[test]
    fn build_managed_audit_bundle_maps_official_runtime_facts_to_semantic_candidates() {
        let artifact: ClaudeManagedArtifact = serde_json::from_value(json!({
            "cwd": "/repo",
            "hookEvents": [
                {
                    "hook": "PermissionRequest",
                    "input": {
                        "session_id": "sdk-session-runtime",
                        "cwd": "/repo",
                        "hook_event_name": "PermissionRequest",
                        "tool_name": "Bash",
                        "tool_input": {"command": "cargo test"}
                    }
                },
                {
                    "hook": "CanUseTool",
                    "input": {
                        "tool_name": "Edit",
                        "tool_input": {"file_path": "src/lib.rs"},
                        "tool_use_id": "tool-edit-1",
                        "agent_id": "general-purpose",
                        "blocked_path": null,
                        "decision_reason": "auto-approved by Libra managed helper",
                        "suggestions": []
                    }
                },
                {
                    "hook": "Elicitation",
                    "input": {
                        "session_id": "sdk-session-runtime",
                        "cwd": "/repo",
                        "hook_event_name": "Elicitation",
                        "mcp_server_name": "review-gate",
                        "message": "Approve release checks?",
                        "elicitation_id": "elic-1"
                    }
                },
                {
                    "hook": "InstructionsLoaded",
                    "input": {
                        "session_id": "sdk-session-runtime",
                        "cwd": "/repo",
                        "hook_event_name": "InstructionsLoaded",
                        "file_path": ".claude/CLAUDE.md",
                        "memory_type": "Project",
                        "load_reason": "session_start"
                    }
                },
                {
                    "hook": "TaskCompleted",
                    "input": {
                        "session_id": "sdk-session-runtime",
                        "cwd": "/repo",
                        "hook_event_name": "TaskCompleted",
                        "task_id": "task-1",
                        "task_subject": "inspect bridge"
                    }
                }
            ],
            "messages": [
                {
                    "type": "system",
                    "subtype": "init",
                    "cwd": "/repo",
                    "session_id": "sdk-session-runtime",
                    "tools": ["Read", "StructuredOutput"],
                    "model": "claude-sonnet-4-5-20250929",
                    "permissionMode": "default",
                    "apiKeySource": "oauth",
                    "claude_code_version": "2.1.76",
                    "output_style": "default",
                    "agents": ["general-purpose", "Plan"],
                    "skills": ["context7"],
                    "slash_commands": ["review"],
                    "mcp_servers": [{"name":"review-gate","status":"connected"}],
                    "plugins": [{"name":"team-plugin","path":"/plugins/team-plugin"}],
                    "fast_mode_state": "off"
                },
                {
                    "type": "system",
                    "subtype": "task_started",
                    "session_id": "sdk-session-runtime",
                    "task_id": "task-1",
                    "description": "Inspect bridge"
                },
                {
                    "type": "system",
                    "subtype": "status",
                    "session_id": "sdk-session-runtime",
                    "status": "compacting"
                },
                {
                    "type": "tool_progress",
                    "session_id": "sdk-session-runtime",
                    "uuid": "tool-progress-1",
                    "tool_use_id": "tool-edit-1",
                    "tool_name": "Edit",
                    "parent_tool_use_id": null,
                    "elapsed_time_seconds": 0.8
                },
                {
                    "type": "stream_event",
                    "session_id": "sdk-session-runtime",
                    "uuid": "stream-runtime-1",
                    "parent_tool_use_id": null,
                    "event": {
                        "type": "content_block_delta",
                        "delta": {"type": "text_delta", "text": "Inspecting bridge"}
                    }
                },
                {
                    "type": "rate_limit_event",
                    "session_id": "sdk-session-runtime",
                    "rate_limit_info": {"status":"allowed_warning"}
                }
            ],
            "resultMessage": {
                "type": "result",
                "subtype": "success",
                "is_error": false,
                "session_id": "sdk-session-runtime",
                "stop_reason": "end_turn",
                "permission_denials": [
                    {
                        "tool_name": "Bash",
                        "tool_use_id": "tool-bash-1",
                        "tool_input": {"command": "cargo test"}
                    }
                ]
            }
        }))
        .expect("fixture should deserialize");

        let bundle = build_managed_audit_bundle(&artifact).expect("bundle should build");

        assert_eq!(
            bundle
                .bridge
                .object_candidates
                .provider_init_snapshot
                .api_key_source
                .as_deref(),
            Some("oauth")
        );
        assert_eq!(
            bundle
                .bridge
                .object_candidates
                .provider_init_snapshot
                .claude_code_version
                .as_deref(),
            Some("2.1.76")
        );
        assert_eq!(
            bundle.bridge.object_candidates.task_runtime_events.len(),
            2,
            "task_started stream + TaskCompleted hook should both map to Task runtime candidates"
        );
        assert!(
            bundle
                .bridge
                .object_candidates
                .tool_runtime_events
                .iter()
                .all(|event| event.semantic_object == "Tool")
        );
        assert_eq!(
            bundle.bridge.object_candidates.tool_runtime_events.len(),
            1,
            "tool_progress should map to Tool runtime candidates"
        );
        assert!(
            bundle
                .bridge
                .object_candidates
                .assistant_runtime_events
                .iter()
                .all(|event| event.semantic_object == "Assistant")
        );
        assert_eq!(
            bundle
                .bridge
                .object_candidates
                .assistant_runtime_events
                .len(),
            1,
            "stream_event should map to Assistant runtime candidates"
        );
        assert!(
            bundle
                .bridge
                .object_candidates
                .task_runtime_events
                .iter()
                .all(|event| event.semantic_object == "Task")
        );
        assert_eq!(
            bundle
                .bridge
                .object_candidates
                .decision_runtime_events
                .len(),
            4,
            "PermissionRequest + CanUseTool + Elicitation + permission_denials should map to Decision runtime candidates"
        );
        assert!(
            bundle
                .bridge
                .object_candidates
                .decision_runtime_events
                .iter()
                .all(|event| event.semantic_object == "Decision")
        );
        assert_eq!(
            bundle.bridge.object_candidates.context_runtime_events.len(),
            3,
            "status + rate_limit_event + InstructionsLoaded should map to Context runtime candidates"
        );
        assert!(
            bundle
                .bridge
                .object_candidates
                .context_runtime_events
                .iter()
                .all(|event| event.semantic_object == "Context")
        );
        assert_eq!(
            bundle.bridge.session_state.metadata["provider_runtime"]["providerInit"]["apiKeySource"],
            json!("oauth")
        );
        assert!(
            bundle
                .field_provenance
                .iter()
                .any(|entry| entry.field_path == "runtime.toolEvents"),
            "expected tool runtime provenance"
        );
        assert!(
            bundle
                .field_provenance
                .iter()
                .any(|entry| entry.field_path == "runtime.assistantEvents"),
            "expected assistant runtime provenance"
        );
        assert!(
            bundle
                .field_provenance
                .iter()
                .any(|entry| entry.field_path == "runtime.taskEvents"),
            "expected task runtime provenance"
        );
        assert!(
            bundle
                .field_provenance
                .iter()
                .any(|entry| entry.field_path == "runtime.decisionEvents"),
            "expected decision runtime provenance"
        );
        assert!(
            bundle
                .field_provenance
                .iter()
                .any(|entry| entry.field_path == "runtime.contextEvents"),
            "expected context runtime provenance"
        );
    }
}
