use std::{
    collections::HashMap,
    io::{self, IsTerminal, Write as _},
    time::Duration,
};

use crossterm::{
    cursor,
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
        PushKeyboardEnhancementFlags,
    },
    execute, terminal,
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Layout},
    prelude::{Line, Position, Span, Style, Text},
    style::{Color, Modifier},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    sync::mpsc::{UnboundedReceiver, UnboundedSender, error::TryRecvError, unbounded_channel},
    task::JoinHandle,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::*;
use crate::{
    internal::ai::mcp::resource::CreateToolInvocationParams,
    utils::output::{JsonFormat, OutputConfig},
};

const TOOL_INVOCATION_BINDINGS_DIR: &str = "claude-tool-invocation-bindings";
const DEFAULT_CHAT_TIMEOUT_SECONDS: u64 = 60;
const DEFAULT_CHAT_TOOLS: [&str; 5] = ["Read", "Edit", "Write", "Glob", "Grep"];
const CHAT_PROMPT_PREFIX: &str = "you> ";
const CHAT_CONTINUATION_PREFIX: &str = "... ";
const DEFAULT_CHAT_TERMINAL_COLUMNS: usize = 80;

#[derive(Args, Debug)]
pub(super) struct ImportArtifactArgs {
    #[arg(long, help = "Path to a raw Claude managed artifact JSON file")]
    pub(super) artifact: PathBuf,
}

#[derive(Args, Debug)]
pub(super) struct RunManagedArgs {
    #[arg(long, help = "Prompt text for the managed Claude SDK session")]
    pub(super) prompt: Option<String>,
    #[arg(long, help = "Read the prompt text from a UTF-8 file")]
    pub(super) prompt_file: Option<PathBuf>,
    #[arg(long, help = "Working directory for the Claude SDK session")]
    pub(super) cwd: Option<PathBuf>,
    #[arg(long, default_value = DEFAULT_MODEL, help = "Claude model identifier")]
    pub(super) model: String,
    #[arg(
        long,
        default_value = "default",
        help = "Claude SDK permission mode passed to query()"
    )]
    pub(super) permission_mode: String,
    #[arg(
        long,
        help = "Optional helper timeout in seconds; when reached, Libra persists a partial managed artifact if available"
    )]
    pub(super) timeout_seconds: Option<u64>,
    #[arg(
        long = "tool",
        help = "Tool name to enable and allow for the managed Claude SDK session"
    )]
    pub(super) tools: Vec<String>,
    #[arg(
        long,
        default_value_t = true,
        action = clap::ArgAction::Set,
        help = "Whether the helper should auto-approve requested tools; set to false for live permission/decision probing"
    )]
    pub(super) auto_approve_tools: bool,
    #[arg(
        long,
        default_value_t = false,
        action = clap::ArgAction::Set,
        help = "Whether the helper should request SDKPartialAssistantMessage stream_event messages"
    )]
    pub(super) include_partial_messages: bool,
    #[arg(
        long,
        default_value_t = false,
        action = clap::ArgAction::Set,
        help = "Whether the helper should request prompt_suggestion messages after result events"
    )]
    pub(super) prompt_suggestions: bool,
    #[arg(
        long,
        default_value_t = false,
        action = clap::ArgAction::Set,
        help = "Whether the helper should request agent-generated task_progress summaries for subagents"
    )]
    pub(super) agent_progress_summaries: bool,
    #[arg(
        long,
        default_value_t = false,
        action = clap::ArgAction::Set,
        help = "Whether the helper should handle tool approvals and AskUserQuestion prompts inline through an interactive terminal"
    )]
    pub(super) interactive_approvals: bool,
    #[arg(
        long,
        default_value_t = false,
        help = "Use the legacy buffered helper flow and emit the final managed artifact summary JSON only"
    )]
    pub(super) batch: bool,
    #[arg(
        long,
        default_value_t = false,
        action = clap::ArgAction::Set,
        help = "Whether Claude SDK should enable file checkpointing and emit files_persisted facts for managed runs"
    )]
    pub(super) enable_file_checkpointing: bool,
    #[arg(
        long = "continue",
        default_value_t = false,
        action = clap::ArgAction::Set,
        help = "Continue the most recent Claude SDK session in the selected working directory on the first turn"
    )]
    pub(super) continue_session: bool,
    #[arg(
        long,
        help = "Resume a specific Claude SDK provider session by UUID on the first turn"
    )]
    pub(super) resume: Option<String>,
    #[arg(
        long,
        default_value_t = false,
        action = clap::ArgAction::Set,
        help = "When resuming on the first turn, fork into a new Claude SDK session instead of continuing the original session"
    )]
    pub(super) fork_session: bool,
    #[arg(
        long,
        help = "Use a specific UUID for the Claude SDK session on the first turn; when resuming, this requires --fork-session"
    )]
    pub(super) session_id: Option<String>,
    #[arg(
        long,
        help = "Resume only up to and including a specific assistant message UUID on the first turn; requires --resume"
    )]
    pub(super) resume_session_at: Option<String>,
    #[arg(
        long,
        help = "Optional path to a custom helper script; defaults to the embedded helper"
    )]
    pub(super) helper_path: Option<PathBuf>,
    #[arg(
        long,
        default_value = DEFAULT_PYTHON_BINARY,
        help = "Python executable used to run the helper"
    )]
    pub(super) python_binary: String,
}

#[derive(Args, Debug, Clone)]
pub(super) struct ChatManagedArgs {
    #[arg(long, help = "Working directory for the Claude SDK session")]
    pub(super) cwd: Option<PathBuf>,
    #[arg(long, default_value = DEFAULT_MODEL, help = "Claude model identifier")]
    pub(super) model: String,
    #[arg(
        long,
        default_value = "default",
        help = "Claude SDK permission mode passed to query()"
    )]
    pub(super) permission_mode: String,
    #[arg(
        long,
        default_value_t = DEFAULT_CHAT_TIMEOUT_SECONDS,
        help = "Optional helper timeout in seconds; when reached, Libra persists a partial managed artifact if available"
    )]
    pub(super) timeout_seconds: u64,
    #[arg(
        long = "tool",
        default_values = DEFAULT_CHAT_TOOLS,
        help = "Tool name to enable and allow for the managed Claude SDK session"
    )]
    pub(super) tools: Vec<String>,
    #[arg(
        long,
        default_value_t = false,
        action = clap::ArgAction::Set,
        help = "Whether the helper should request prompt_suggestion messages after result events"
    )]
    pub(super) prompt_suggestions: bool,
    #[arg(
        long,
        default_value_t = false,
        action = clap::ArgAction::Set,
        help = "Whether the helper should request agent-generated task_progress summaries for subagents"
    )]
    pub(super) agent_progress_summaries: bool,
    #[arg(
        long,
        default_value_t = false,
        action = clap::ArgAction::Set,
        help = "Whether the helper should handle tool approvals and AskUserQuestion prompts inline through an interactive terminal"
    )]
    pub(super) interactive_approvals: bool,
    #[arg(
        long,
        default_value_t = true,
        action = clap::ArgAction::Set,
        help = "Whether Claude SDK should enable file checkpointing and emit files_persisted facts for managed runs"
    )]
    pub(super) enable_file_checkpointing: bool,
    #[arg(
        long = "continue",
        default_value_t = false,
        action = clap::ArgAction::Set,
        help = "Continue the most recent Claude SDK session in the selected working directory"
    )]
    pub(super) continue_session: bool,
    #[arg(long, help = "Resume a specific Claude SDK provider session by UUID")]
    pub(super) resume: Option<String>,
    #[arg(
        long,
        default_value_t = false,
        action = clap::ArgAction::Set,
        help = "When resuming, fork into a new Claude SDK session instead of continuing the original session"
    )]
    pub(super) fork_session: bool,
    #[arg(
        long,
        help = "Use a specific UUID for the Claude SDK session; when resuming, this requires --fork-session"
    )]
    pub(super) session_id: Option<String>,
    #[arg(
        long,
        help = "Resume only up to and including a specific assistant message UUID; requires --resume"
    )]
    pub(super) resume_session_at: Option<String>,
    #[arg(
        long,
        help = "Optional path to a custom helper script; defaults to the embedded helper"
    )]
    pub(super) helper_path: Option<PathBuf>,
    #[arg(
        long,
        default_value = DEFAULT_PYTHON_BINARY,
        help = "Python executable used to run the helper"
    )]
    pub(super) python_binary: String,
}

#[derive(Debug, Serialize)]
pub(super) struct ClaudeSdkCommandOutput {
    ok: bool,
    #[serde(rename = "mode")]
    command_mode: &'static str,
    #[serde(rename = "providerSessionId")]
    provider_session_id: String,
    #[serde(rename = "aiSessionId")]
    ai_session_id: String,
    #[serde(rename = "aiSessionObjectHash")]
    ai_session_object_hash: String,
    #[serde(rename = "alreadyPersisted")]
    already_persisted: bool,
    #[serde(
        rename = "intentExtractionPath",
        skip_serializing_if = "Option::is_none"
    )]
    intent_extraction_path: Option<String>,
    #[serde(rename = "rawArtifactPath")]
    raw_artifact_path: String,
    #[serde(rename = "auditBundlePath")]
    audit_bundle_path: String,
}

#[derive(Debug, Serialize)]
pub(super) struct ManagedHelperRequest {
    pub(super) mode: &'static str,
    pub(super) prompt: String,
    pub(super) cwd: String,
    pub(super) model: String,
    #[serde(rename = "permissionMode")]
    pub(super) permission_mode: String,
    #[serde(rename = "timeoutSeconds", skip_serializing_if = "Option::is_none")]
    pub(super) timeout_seconds: Option<u64>,
    #[serde(rename = "idleTimeoutSeconds", skip_serializing_if = "Option::is_none")]
    pub(super) idle_timeout_seconds: Option<u64>,
    pub(super) tools: Vec<String>,
    #[serde(rename = "allowedTools")]
    pub(super) allowed_tools: Vec<String>,
    #[serde(rename = "autoApproveTools")]
    pub(super) auto_approve_tools: bool,
    #[serde(rename = "includePartialMessages")]
    pub(super) include_partial_messages: bool,
    #[serde(rename = "promptSuggestions")]
    pub(super) prompt_suggestions: bool,
    #[serde(rename = "agentProgressSummaries")]
    pub(super) agent_progress_summaries: bool,
    #[serde(rename = "interactiveApprovals", skip_serializing_if = "is_false")]
    pub(super) interactive_approvals: bool,
    #[serde(rename = "enableFileCheckpointing", skip_serializing_if = "is_false")]
    pub(super) enable_file_checkpointing: bool,
    #[serde(rename = "continue", skip_serializing_if = "is_false")]
    pub(super) continue_session: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) resume: Option<String>,
    #[serde(rename = "forkSession", skip_serializing_if = "is_false")]
    pub(super) fork_session: bool,
    #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
    pub(super) session_id: Option<String>,
    #[serde(rename = "resumeSessionAt", skip_serializing_if = "Option::is_none")]
    pub(super) resume_session_at: Option<String>,
    #[serde(rename = "systemPrompt", skip_serializing_if = "Option::is_none")]
    pub(super) system_prompt: Option<ManagedSystemPrompt>,
    #[serde(rename = "outputSchema", skip_serializing_if = "Option::is_none")]
    pub(super) output_schema: Option<Value>,
}

#[derive(Debug, Serialize)]
pub(super) struct ManagedSystemPrompt {
    #[serde(rename = "type")]
    pub(super) kind: &'static str,
    pub(super) preset: &'static str,
    pub(super) append: String,
}

#[derive(Debug, Clone, Default, Serialize)]
struct StreamingFinalizeSummary {
    #[serde(rename = "resolvedExtraction", skip_serializing_if = "Option::is_none")]
    resolved_extraction_path: Option<String>,
    #[serde(rename = "intentId", skip_serializing_if = "Option::is_none")]
    intent_id: Option<String>,
    #[serde(rename = "runId", skip_serializing_if = "Option::is_none")]
    run_id: Option<String>,
    #[serde(rename = "patchsetId", skip_serializing_if = "Option::is_none")]
    patchset_id: Option<String>,
    #[serde(rename = "decisionId", skip_serializing_if = "Option::is_none")]
    decision_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
struct StreamingRunResult {
    ok: bool,
    event: &'static str,
    #[serde(rename = "providerSessionId")]
    provider_session_id: String,
    #[serde(rename = "aiSessionId")]
    ai_session_id: String,
    #[serde(rename = "rawArtifactPath")]
    raw_artifact_path: String,
    #[serde(rename = "auditBundlePath")]
    audit_bundle_path: String,
    #[serde(rename = "alreadyPersisted")]
    already_persisted: bool,
    #[serde(rename = "autoFinalize")]
    auto_finalize: StreamingFinalizeSummary,
}

#[derive(Debug)]
struct ManagedStreamingTurnArgs {
    prompt: String,
    cwd: PathBuf,
    model: String,
    permission_mode: String,
    timeout_seconds: Option<u64>,
    tools: Vec<String>,
    prompt_suggestions: bool,
    agent_progress_summaries: bool,
    interactive_approvals: bool,
    enable_file_checkpointing: bool,
    continue_session: bool,
    resume: Option<String>,
    fork_session: bool,
    session_id: Option<String>,
    resume_session_at: Option<String>,
}

#[derive(Debug)]
struct ManagedStreamingTurnOutcome {
    outcome: PersistedManagedArtifactOutcome,
    auto_finalize: StreamingFinalizeSummary,
    assistant_text: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManagedStreamingTurnKind {
    Run,
    Chat,
}

#[derive(Debug, Clone, Copy)]
enum StreamingRenderMode {
    Ndjson,
    Human { print_completion: bool },
    Quiet,
}

#[derive(Debug)]
enum ChatTurnUiEvent {
    AssistantDelta(String),
    AssistantMessage(String),
    ToolCall(String),
    ToolResult(String),
    Completed(Box<Result<ManagedStreamingTurnOutcome, String>>),
}

#[derive(Debug, Clone)]
struct ManagedSessionControl {
    continue_session: bool,
    resume: Option<String>,
    fork_session: bool,
    session_id: Option<String>,
    resume_session_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ClaudeToolInvocationBindingArtifact {
    schema: String,
    #[serde(rename = "aiSessionId")]
    ai_session_id: String,
    #[serde(rename = "providerSessionId")]
    provider_session_id: String,
    #[serde(rename = "runId")]
    run_id: String,
    #[serde(rename = "runBindingPath")]
    run_binding_path: String,
    invocations: Vec<ClaudeToolInvocationBindingEntry>,
    #[serde(rename = "createdAt")]
    created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ClaudeToolInvocationBindingEntry {
    #[serde(rename = "toolUseId")]
    tool_use_id: String,
    #[serde(rename = "toolName")]
    tool_name: String,
    status: String,
    #[serde(rename = "toolInvocationId")]
    tool_invocation_id: String,
    #[serde(rename = "sourcePath")]
    source_path: String,
}

async fn sync_incremental_managed_inputs(
    storage_path: &Path,
    outcome: &PersistedManagedArtifactOutcome,
) -> Result<()> {
    let ai_session_id = outcome.ai_session_id.as_str();
    let raw_artifact_path = PathBuf::from(&outcome.raw_artifact_path);
    let (audit_bundle_path, audit_bundle) =
        load_managed_audit_bundle_for_ai_session(storage_path, ai_session_id).await?;

    let provider_session_object_id =
        build_provider_session_object_id(&audit_bundle.provider_session_id)?;
    let provider_session_path =
        provider_session_artifact_path(storage_path, &provider_session_object_id);
    let provider_evidence_input_object_id =
        build_evidence_input_object_id(&audit_bundle.provider_session_id)?;
    let provider_evidence_input_path =
        evidence_input_artifact_path(storage_path, &provider_evidence_input_object_id);

    let managed_object_id = build_managed_evidence_input_object_id(ai_session_id)?;
    let managed_artifact_path =
        managed_evidence_input_artifact_path(storage_path, &managed_object_id);
    let mut managed_artifact = build_managed_evidence_input_artifact(
        &audit_bundle,
        ManagedEvidenceInputBuildContext {
            ai_session_id,
            raw_artifact_path: &raw_artifact_path,
            audit_bundle_path: &audit_bundle_path,
            provider_session_path: provider_session_path
                .exists()
                .then_some(provider_session_path.as_path()),
            provider_evidence_input_path: provider_evidence_input_path
                .exists()
                .then_some(provider_evidence_input_path.as_path()),
            captured_at: Utc::now().to_rfc3339(),
        },
        managed_object_id,
    );
    if let Some(existing) =
        read_existing_managed_evidence_input_artifact(&managed_artifact_path).await?
        && managed_evidence_input_artifact_matches(&existing, &managed_artifact)
    {
        managed_artifact.captured_at = existing.captured_at;
    }
    let _ = persist_managed_evidence_input_artifact(
        storage_path,
        &managed_artifact_path,
        &managed_artifact,
    )
    .await?;

    let decision_object_id = build_decision_input_object_id(ai_session_id)?;
    let decision_artifact_path = decision_input_artifact_path(storage_path, &decision_object_id);
    let mut decision_artifact = build_decision_input_artifact(
        ai_session_id,
        &audit_bundle_path,
        &audit_bundle,
        managed_artifact_path
            .exists()
            .then_some(managed_artifact_path.as_path()),
        decision_object_id,
        Utc::now().to_rfc3339(),
    );
    if let Some(existing) = read_existing_decision_input_artifact(&decision_artifact_path).await?
        && decision_input_artifact_matches(&existing, &decision_artifact)
    {
        decision_artifact.captured_at = existing.captured_at;
    }
    let _ =
        persist_decision_input_artifact(storage_path, &decision_artifact_path, &decision_artifact)
            .await?;

    if let Some((run_binding_path, run_binding)) =
        ensure_streaming_formal_run_binding(storage_path, ai_session_id).await?
    {
        sync_streaming_tool_invocations(
            storage_path,
            &audit_bundle_path,
            &audit_bundle,
            &run_binding_path,
            &run_binding,
        )
        .await?;
        ensure_formal_runtime_side_objects(storage_path, &run_binding, &audit_bundle).await?;
        ensure_formal_derived_audit_objects(storage_path, &run_binding, &audit_bundle).await?;
    }

    Ok(())
}

async fn ensure_streaming_formal_run_binding(
    storage_path: &Path,
    ai_session_id: &str,
) -> Result<Option<(PathBuf, ClaudeFormalRunBindingArtifact)>> {
    let binding_path = formal_run_binding_path(storage_path, ai_session_id);
    if let Some(existing) = read_existing_binding_if_live::<ClaudeFormalRunBindingArtifact>(
        storage_path,
        &binding_path,
        "Claude formal run binding",
        &[
            ("task", |binding| binding.task_id.as_str()),
            ("run", |binding| binding.run_id.as_str()),
        ],
    )
    .await?
    {
        validate_formal_run_binding_consistency(&existing, ai_session_id)?;
        return Ok(Some((binding_path, existing)));
    }

    let (audit_bundle_path, audit_bundle) =
        load_managed_audit_bundle_for_ai_session(storage_path, ai_session_id).await?;
    let summary = derive_formal_task_summary(&audit_bundle, None);
    let description = derive_formal_task_description(&audit_bundle);
    let goal_type = derive_goal_type(&audit_bundle);
    let managed_run_status = audit_bundle
        .bridge
        .object_candidates
        .run_event
        .status
        .clone();
    let intent_extraction_status = audit_bundle.bridge.intent_extraction.status.clone();

    let mcp_server = init_local_mcp_server(storage_path).await?;
    let actor = mcp_server
        .resolve_actor_from_params(Some("system"), Some("claude-sdk-stream"))
        .map_err(|error| anyhow!("failed to resolve Claude SDK stream actor: {error:?}"))?;
    let context_snapshot_id =
        create_context_snapshot_for_audit_bundle(&mcp_server, &actor, &audit_bundle).await?;
    let task_id = parse_created_id(
        "task",
        &mcp_server
            .create_task_impl(
                CreateTaskParams {
                    title: summary.clone(),
                    description: Some(description),
                    goal_type,
                    constraints: Some(vec![format!("claude-sdk ai_session_id={ai_session_id}")]),
                    acceptance_criteria: None,
                    requested_by_kind: None,
                    requested_by_id: None,
                    dependencies: None,
                    intent_id: None,
                    parent_task_id: None,
                    origin_step_id: None,
                    status: Some(task_status_for_managed_run(&managed_run_status).to_string()),
                    reason: Some(format!(
                        "Claude SDK streaming session {ai_session_id} bridged into formal task"
                    )),
                    tags: None,
                    external_ids: None,
                    actor_kind: Some("system".to_string()),
                    actor_id: Some("claude-sdk-stream".to_string()),
                },
                actor.clone(),
            )
            .await
            .map_err(|error| anyhow!("failed to create streaming formal Claude task: {error:?}"))?,
    )?;
    let run_id = parse_created_id(
        "run",
        &mcp_server
            .create_run_impl(
                CreateRunParams {
                    task_id: task_id.clone(),
                    base_commit_sha: current_head_sha().await,
                    plan_id: None,
                    status: Some(run_status_for_managed_run(&managed_run_status).to_string()),
                    context_snapshot_id,
                    error: run_error_for_managed_status(&managed_run_status),
                    agent_instances: None,
                    metrics_json: Some(
                        json!({
                            "provider": "claude",
                            "aiSessionId": ai_session_id,
                            "providerSessionId": audit_bundle.provider_session_id,
                            "intentExtractionStatus": intent_extraction_status,
                            "provisional": true,
                        })
                        .to_string(),
                    ),
                    reason: Some(format!(
                        "Claude SDK streaming session {ai_session_id} bridged into formal run"
                    )),
                    orchestrator_version: None,
                    tags: None,
                    external_ids: None,
                    actor_kind: Some("system".to_string()),
                    actor_id: Some("claude-sdk-stream".to_string()),
                },
                actor.clone(),
            )
            .await
            .map_err(|error| anyhow!("failed to create streaming formal Claude run: {error:?}"))?,
    )?;

    let binding = ClaudeFormalRunBindingArtifact {
        schema: "libra.claude_formal_run_binding.v1".to_string(),
        ai_session_id: ai_session_id.to_string(),
        provider_session_id: audit_bundle.provider_session_id.clone(),
        task_id,
        run_id,
        audit_bundle_path: audit_bundle_path.to_string_lossy().to_string(),
        intent_binding_path: None,
        intent_id: None,
        plan_id: None,
        managed_run_status,
        intent_extraction_status,
        summary,
        created_at: Utc::now().to_rfc3339(),
    };
    write_pretty_json_file(&binding_path, &binding).await?;
    create_context_frames_for_audit_bundle(
        &mcp_server,
        &actor,
        &audit_bundle,
        None,
        Some(&binding.run_id),
    )
    .await?;
    Ok(Some((binding_path, binding)))
}

async fn sync_streaming_tool_invocations(
    storage_path: &Path,
    audit_bundle_path: &Path,
    audit_bundle: &ManagedAuditBundle,
    run_binding_path: &Path,
    run_binding: &ClaudeFormalRunBindingArtifact,
) -> Result<()> {
    let binding_path = storage_path
        .join(TOOL_INVOCATION_BINDINGS_DIR)
        .join(format!("{}.json", run_binding.ai_session_id));
    let mut binding = if binding_path.exists() {
        read_json_artifact::<ClaudeToolInvocationBindingArtifact>(
            &binding_path,
            "Claude tool invocation binding",
        )
        .await?
    } else {
        ClaudeToolInvocationBindingArtifact {
            schema: "libra.claude_tool_invocation_binding.v1".to_string(),
            ai_session_id: run_binding.ai_session_id.clone(),
            provider_session_id: run_binding.provider_session_id.clone(),
            run_id: run_binding.run_id.clone(),
            run_binding_path: run_binding_path.to_string_lossy().to_string(),
            invocations: Vec::new(),
            created_at: Utc::now().to_rfc3339(),
        }
    };

    let mut existing_tool_use_ids = binding
        .invocations
        .iter()
        .map(|entry| entry.tool_use_id.clone())
        .collect::<BTreeSet<_>>();
    let tool_status = audit_bundle
        .bridge
        .object_candidates
        .tool_invocation_events
        .iter()
        .map(|event| (event.id.clone(), event.status.clone()))
        .collect::<BTreeMap<_, _>>();

    let mcp_server = init_local_mcp_server(storage_path).await?;
    let actor = mcp_server
        .resolve_actor_from_params(Some("system"), Some("claude-sdk-stream"))
        .map_err(|error| anyhow!("failed to resolve Claude SDK stream actor: {error:?}"))?;

    for invocation in &audit_bundle.bridge.tool_invocations {
        let Some(status) = tool_status.get(&invocation.tool_use_id) else {
            continue;
        };
        if !matches!(status.as_str(), "completed" | "error")
            || !existing_tool_use_ids.insert(invocation.tool_use_id.clone())
        {
            continue;
        }
        let mcp_status = if status == "error" { "error" } else { "ok" };

        let io_footprint = infer_tool_invocation_io_footprint(invocation);
        let result_summary = invocation
            .tool_response
            .as_ref()
            .map(summarize_tool_response_for_mcp)
            .filter(|summary| !summary.is_empty());

        let tool_invocation_id = parse_created_id(
            "tool_invocation",
            &mcp_server
                .create_tool_invocation_impl(
                    CreateToolInvocationParams {
                        run_id: run_binding.run_id.clone(),
                        tool_name: invocation
                            .tool_name
                            .clone()
                            .unwrap_or_else(|| "unknown".to_string()),
                        status: Some(mcp_status.to_string()),
                        args_json: invocation
                            .tool_input
                            .as_ref()
                            .map(|value| value.to_string()),
                        io_footprint,
                        result_summary,
                        artifacts: None,
                        tags: None,
                        external_ids: Some(HashMap::from([(
                            "claude_tool_use_id".to_string(),
                            invocation.tool_use_id.clone(),
                        )])),
                        actor_kind: Some("system".to_string()),
                        actor_id: Some("claude-sdk-stream".to_string()),
                    },
                    actor.clone(),
                )
                .await
                .map_err(|error| {
                    anyhow!("failed to create streaming tool invocation: {error:?}")
                })?,
        )?;

        binding.invocations.push(ClaudeToolInvocationBindingEntry {
            tool_use_id: invocation.tool_use_id.clone(),
            tool_name: invocation
                .tool_name
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            status: status.clone(),
            tool_invocation_id,
            source_path: audit_bundle_path.to_string_lossy().to_string(),
        });
    }

    write_pretty_json_file(&binding_path, &binding).await?;
    Ok(())
}

fn infer_tool_invocation_io_footprint(
    invocation: &crate::internal::ai::providers::claude_sdk::managed::ManagedToolInvocation,
) -> Option<crate::internal::ai::mcp::resource::IoFootprintParams> {
    let paths_written = invocation
        .tool_input
        .as_ref()
        .and_then(|input| input.get("file_path"))
        .and_then(Value::as_str)
        .map(|path| vec![path.to_string()]);
    let has_data = paths_written
        .as_ref()
        .is_some_and(|paths| !paths.is_empty());
    has_data.then_some(crate::internal::ai::mcp::resource::IoFootprintParams {
        paths_read: None,
        paths_written,
    })
}

fn summarize_tool_response_for_mcp(value: &Value) -> String {
    if let Some(file_path) = value
        .get("file")
        .and_then(|file| file.get("filePath"))
        .and_then(Value::as_str)
    {
        return format!("file={file_path}");
    }
    value.as_str().map(ToString::to_string).unwrap_or_else(|| {
        value
            .as_object()
            .map(|object| object.keys().cloned().collect::<Vec<_>>().join(","))
            .filter(|summary| !summary.is_empty())
            .unwrap_or_else(|| "ok".to_string())
    })
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn ignore_incomplete_runtime_snapshot_error(error: &anyhow::Error) -> bool {
    error
        .to_string()
        .contains("managed artifact does not contain a valid system init message")
}

impl HelperResponse for ManagedHelperRequest {
    type Output = ClaudeManagedArtifact;

    fn parse_response(stdout: &str, stderr: &str) -> Result<Self::Output> {
        serde_json::from_str(stdout.trim()).with_context(|| {
            format!(
                "failed to parse Claude SDK helper output as a managed artifact (stderr: {})",
                stderr.trim()
            )
        })
    }
}

pub(super) async fn import_artifact(args: ImportArtifactArgs) -> Result<()> {
    let storage_path = util::try_get_storage_path(None)
        .context("claude-sdk commands must be run inside a Libra repository")?;
    let artifact = read_artifact(&args.artifact).await?;
    let outcome = persist_managed_artifact(&storage_path, &artifact).await?;
    print_result("import", &outcome)?;
    Ok(())
}

fn resolve_managed_cwd(cwd: Option<&PathBuf>) -> Result<PathBuf> {
    cwd.cloned()
        .map(Ok)
        .unwrap_or_else(|| std::env::current_dir().context("failed to read current directory"))
}

fn build_run_streaming_helper_request(args: ManagedStreamingTurnArgs) -> ManagedHelperRequest {
    let permission_mode = args.permission_mode.clone();
    ManagedHelperRequest {
        mode: "queryStream",
        prompt: args.prompt,
        cwd: args.cwd.to_string_lossy().to_string(),
        model: args.model,
        permission_mode: permission_mode.clone(),
        timeout_seconds: args.timeout_seconds,
        idle_timeout_seconds: None,
        tools: args.tools.clone(),
        allowed_tools: args.tools,
        auto_approve_tools: false,
        include_partial_messages: true,
        prompt_suggestions: args.prompt_suggestions,
        agent_progress_summaries: args.agent_progress_summaries,
        interactive_approvals: interactive_approvals_enabled(
            &permission_mode,
            args.interactive_approvals,
        ),
        enable_file_checkpointing: args.enable_file_checkpointing,
        continue_session: args.continue_session,
        resume: args.resume,
        fork_session: args.fork_session,
        session_id: args.session_id,
        resume_session_at: args.resume_session_at,
        system_prompt: Some(default_managed_system_prompt()),
        output_schema: Some(managed_output_schema()),
    }
}

fn build_chat_streaming_helper_request(args: ManagedStreamingTurnArgs) -> ManagedHelperRequest {
    let permission_mode = args.permission_mode.clone();
    ManagedHelperRequest {
        mode: "queryStream",
        prompt: args.prompt,
        cwd: args.cwd.to_string_lossy().to_string(),
        model: args.model,
        permission_mode: permission_mode.clone(),
        timeout_seconds: None,
        idle_timeout_seconds: args.timeout_seconds,
        tools: args.tools.clone(),
        allowed_tools: args.tools,
        auto_approve_tools: false,
        include_partial_messages: true,
        prompt_suggestions: args.prompt_suggestions,
        agent_progress_summaries: args.agent_progress_summaries,
        interactive_approvals: interactive_approvals_enabled(
            &permission_mode,
            args.interactive_approvals,
        ),
        enable_file_checkpointing: args.enable_file_checkpointing,
        continue_session: args.continue_session,
        resume: args.resume,
        fork_session: args.fork_session,
        session_id: args.session_id,
        resume_session_at: args.resume_session_at,
        system_prompt: None,
        output_schema: None,
    }
}

fn interactive_approvals_enabled(
    permission_mode: &str,
    explicit_interactive_approvals: bool,
) -> bool {
    explicit_interactive_approvals || permission_mode == "default"
}

fn should_use_fullscreen_chat_tui(args: &ChatManagedArgs, stdout_is_terminal: bool) -> bool {
    stdout_is_terminal
        && !interactive_approvals_enabled(&args.permission_mode, args.interactive_approvals)
}

fn streaming_render_mode(output: &OutputConfig) -> StreamingRenderMode {
    if output.json_format == Some(JsonFormat::Ndjson) {
        StreamingRenderMode::Ndjson
    } else if output.quiet {
        StreamingRenderMode::Quiet
    } else {
        StreamingRenderMode::Human {
            print_completion: true,
        }
    }
}

pub(super) async fn run_managed(args: RunManagedArgs, output: &OutputConfig) -> Result<()> {
    let storage_path = util::try_get_storage_path(None)
        .context("claude-sdk commands must be run inside a Libra repository")?;
    validate_run_managed_args(&args)?;
    if !args.batch
        && matches!(
            output.json_format,
            Some(JsonFormat::Pretty | JsonFormat::Compact)
        )
    {
        bail!(
            "interactive claude-sdk run supports only --json=ndjson; use --batch for pretty or compact JSON output"
        );
    }

    let prompt = resolve_prompt(&args)?;
    let cwd = resolve_managed_cwd(args.cwd.as_ref())?;
    let (_temp_helper_dir, helper_path) = materialize_helper(args.helper_path.as_deref()).await?;
    let custom_helper = args.helper_path.is_some();
    if args.batch {
        let permission_mode = args.permission_mode.clone();
        let helper_request = ManagedHelperRequest {
            mode: "query",
            prompt,
            cwd: cwd.to_string_lossy().to_string(),
            model: args.model,
            permission_mode: permission_mode.clone(),
            timeout_seconds: args.timeout_seconds,
            idle_timeout_seconds: None,
            tools: args.tools.clone(),
            allowed_tools: args.tools.clone(),
            auto_approve_tools: args.auto_approve_tools
                && !args.tools.is_empty()
                && !args.interactive_approvals,
            include_partial_messages: args.include_partial_messages,
            prompt_suggestions: args.prompt_suggestions,
            agent_progress_summaries: args.agent_progress_summaries,
            interactive_approvals: args.interactive_approvals,
            enable_file_checkpointing: args.enable_file_checkpointing,
            continue_session: args.continue_session,
            resume: args.resume.clone(),
            fork_session: args.fork_session,
            session_id: args.session_id.clone(),
            resume_session_at: args.resume_session_at.clone(),
            system_prompt: Some(default_managed_system_prompt()),
            output_schema: Some(managed_output_schema()),
        };
        let artifact = invoke_helper(
            custom_helper,
            &args.python_binary,
            &helper_path,
            &helper_request,
        )
        .await?;
        let outcome = persist_managed_artifact(&storage_path, &artifact).await?;
        ensure_managed_artifact_succeeded(&artifact)?;
        print_result("run", &outcome)?;
        return Ok(());
    }

    let helper_request = build_run_streaming_helper_request(ManagedStreamingTurnArgs {
        prompt,
        cwd,
        model: args.model,
        permission_mode: args.permission_mode,
        timeout_seconds: args.timeout_seconds,
        tools: args.tools,
        prompt_suggestions: args.prompt_suggestions,
        agent_progress_summaries: args.agent_progress_summaries,
        interactive_approvals: args.interactive_approvals,
        enable_file_checkpointing: args.enable_file_checkpointing,
        continue_session: args.continue_session,
        resume: args.resume,
        fork_session: args.fork_session,
        session_id: args.session_id,
        resume_session_at: args.resume_session_at,
    });

    execute_managed_streaming_turn(
        &storage_path,
        ManagedStreamingTurnKind::Run,
        custom_helper,
        &args.python_binary,
        &helper_path,
        &helper_request,
        streaming_render_mode(output),
        None,
    )
    .await?;
    Ok(())
}

pub(super) async fn chat_managed(args: ChatManagedArgs, output: &OutputConfig) -> Result<()> {
    let storage_path = util::try_get_storage_path(None)
        .context("claude-sdk commands must be run inside a Libra repository")?;
    validate_chat_managed_args(&args, output)?;
    let cwd = resolve_managed_cwd(args.cwd.as_ref())?;
    let (_temp_helper_dir, helper_path) = materialize_helper(args.helper_path.as_deref()).await?;
    let custom_helper = args.helper_path.is_some();
    let stdout_is_terminal = io::stdout().is_terminal();
    if should_use_fullscreen_chat_tui(&args, stdout_is_terminal) {
        return chat_managed_fullscreen_tui(
            &args,
            &storage_path,
            &cwd,
            &helper_path,
            custom_helper,
        )
        .await;
    }

    if stdout_is_terminal
        && interactive_approvals_enabled(&args.permission_mode, args.interactive_approvals)
    {
        eprintln!(
            "interactive approvals are enabled; falling back to stdio chat so tool approval prompts can read from the terminal"
        );
    }

    chat_managed_stdio(&args, &storage_path, &cwd, &helper_path, custom_helper).await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChatTuiRole {
    User,
    Assistant,
    System,
    Error,
}

#[derive(Debug, Clone)]
struct ChatTuiEntry {
    role: ChatTuiRole,
    text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChatTuiMode {
    Ready,
    Processing,
}

struct ChatTuiState {
    entries: Vec<ChatTuiEntry>,
    input: String,
    cursor_pos: usize,
    mode: ChatTuiMode,
    scroll_from_bottom_lines: usize,
    streaming_assistant_index: Option<usize>,
    processing_note: Option<String>,
    model: String,
    cwd: String,
}

struct PendingChatTurn {
    rx: UnboundedReceiver<ChatTurnUiEvent>,
    task: JoinHandle<()>,
}

impl ChatTuiState {
    fn new(model: String, cwd: &Path) -> Self {
        Self {
            entries: vec![ChatTuiEntry {
                role: ChatTuiRole::System,
                text: "Type /help for commands, /exit to quit.\nEnter sends; Ctrl+J inserts a newline.".to_string(),
            }],
            input: String::new(),
            cursor_pos: 0,
            mode: ChatTuiMode::Ready,
            scroll_from_bottom_lines: 0,
            streaming_assistant_index: None,
            processing_note: None,
            model,
            cwd: cwd.display().to_string(),
        }
    }

    fn is_ready(&self) -> bool {
        self.mode == ChatTuiMode::Ready
    }

    fn set_processing(&mut self, processing: bool) {
        self.mode = if processing {
            ChatTuiMode::Processing
        } else {
            ChatTuiMode::Ready
        };
        if !processing {
            self.processing_note = None;
        }
    }

    fn set_processing_note(&mut self, note: impl Into<String>) {
        self.processing_note = Some(note.into());
    }

    fn push_entry(&mut self, role: ChatTuiRole, text: impl Into<String>) {
        self.entries.push(ChatTuiEntry {
            role,
            text: text.into(),
        });
        self.scroll_from_bottom_lines = 0;
    }

    fn start_streaming_assistant(&mut self) {
        if self.streaming_assistant_index.is_none() {
            let idx = self.entries.len();
            self.entries.push(ChatTuiEntry {
                role: ChatTuiRole::Assistant,
                text: String::new(),
            });
            self.streaming_assistant_index = Some(idx);
            self.scroll_from_bottom_lines = 0;
        }
    }

    fn append_stream_delta(&mut self, delta: &str) {
        if delta.is_empty() {
            return;
        }
        self.start_streaming_assistant();
        if let Some(idx) = self.streaming_assistant_index
            && let Some(entry) = self.entries.get_mut(idx)
        {
            entry.text.push_str(delta);
        }
    }

    fn note_stream_assistant_message(&mut self, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        self.start_streaming_assistant();
        if let Some(idx) = self.streaming_assistant_index
            && let Some(entry) = self.entries.get_mut(idx)
            && entry.text.trim().is_empty()
        {
            entry.text = text.to_string();
        }
    }

    fn finish_streaming_assistant(&mut self, fallback: Option<String>) {
        if let Some(idx) = self.streaming_assistant_index.take() {
            let mut remove_entry = false;
            if let Some(entry) = self.entries.get_mut(idx)
                && entry.text.trim().is_empty()
            {
                if let Some(text) = fallback.filter(|text| !text.trim().is_empty()) {
                    entry.text = text;
                } else {
                    remove_entry = true;
                }
            }
            if remove_entry && idx < self.entries.len() {
                self.entries.remove(idx);
            }
        }
    }

    fn insert_char(&mut self, c: char) {
        self.input.insert(self.cursor_pos, c);
        self.cursor_pos += c.len_utf8();
    }

    fn insert_text(&mut self, text: &str) {
        for c in text.chars() {
            self.insert_char(c);
        }
    }

    fn backspace(&mut self) {
        if self.cursor_pos > 0 {
            let prev_pos = prev_char_pos(&self.input, self.cursor_pos);
            self.input.remove(prev_pos);
            self.cursor_pos = prev_pos;
        }
    }

    fn delete(&mut self) {
        if self.cursor_pos < self.input.len() {
            self.input.remove(self.cursor_pos);
        }
    }

    fn cursor_left(&mut self) {
        if self.cursor_pos > 0 {
            self.cursor_pos = prev_char_pos(&self.input, self.cursor_pos);
        }
    }

    fn cursor_right(&mut self) {
        if self.cursor_pos < self.input.len() {
            self.cursor_pos = next_char_pos(&self.input, self.cursor_pos);
        }
    }

    fn cursor_home(&mut self) {
        self.cursor_pos = 0;
    }

    fn cursor_end(&mut self) {
        self.cursor_pos = self.input.len();
    }

    fn take_input(&mut self) -> String {
        let value = std::mem::take(&mut self.input);
        self.cursor_pos = 0;
        value
    }

    fn clear_input(&mut self) {
        self.input.clear();
        self.cursor_pos = 0;
    }

    fn scroll_up(&mut self, lines: usize) {
        self.scroll_from_bottom_lines = self.scroll_from_bottom_lines.saturating_add(lines);
    }

    fn scroll_down(&mut self, lines: usize) {
        self.scroll_from_bottom_lines = self.scroll_from_bottom_lines.saturating_sub(lines);
    }

    fn draw(&self, terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
        terminal
            .draw(|frame| {
                let area = frame.area();
                let layout = Layout::vertical([
                    Constraint::Min(3),
                    Constraint::Length(5),
                    Constraint::Length(1),
                ])
                .split(area);

                let mut transcript_lines = Vec::new();
                for entry in &self.entries {
                    push_transcript_entry_lines(&mut transcript_lines, entry);
                }
                if transcript_lines.is_empty() {
                    transcript_lines.push(Line::styled(
                        "No messages yet.",
                        Style::default().fg(Color::DarkGray),
                    ));
                }

                let transcript_block =
                    Block::default()
                        .borders(Borders::ALL)
                        .title(Line::from(vec![
                            Span::styled(
                                " Claude SDK Chat ",
                                Style::default().add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                format!("{}  cwd: {}", self.model, self.cwd),
                                Style::default().fg(Color::DarkGray),
                            ),
                        ]));
                let transcript_inner = transcript_block.inner(layout[0]);
                let visible_lines = transcript_inner.height as usize;
                let total_lines = transcript_lines.len();
                let max_scroll = total_lines.saturating_sub(visible_lines);
                let from_bottom = self.scroll_from_bottom_lines.min(max_scroll);
                let start_line = total_lines
                    .saturating_sub(visible_lines)
                    .saturating_sub(from_bottom);

                let transcript = Paragraph::new(Text::from(transcript_lines))
                    .block(transcript_block)
                    .wrap(Wrap { trim: false })
                    .scroll((start_line.min(u16::MAX as usize) as u16, 0));
                frame.render_widget(transcript, layout[0]);

                let input_title = if self.is_ready() {
                    " Input "
                } else {
                    " Input (locked while processing) "
                };
                let input_border_style = if self.is_ready() {
                    Style::default().fg(Color::Cyan)
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                let input_block = Block::default()
                    .borders(Borders::ALL)
                    .border_style(input_border_style)
                    .title(input_title);
                let input_inner = input_block.inner(layout[1]);
                let input_height = input_inner.height as usize;
                let input_width = input_inner.width as usize;
                let (input_lines, cursor_x, cursor_y) = if self.input.is_empty() {
                    (
                        vec![Line::styled(
                            "Type your message...",
                            Style::default().fg(Color::DarkGray),
                        )],
                        0u16,
                        0u16,
                    )
                } else {
                    let (lines, cursor_x, cursor_y) = visible_input_and_cursor(
                        &self.input,
                        self.cursor_pos,
                        input_width,
                        input_height,
                    );
                    (
                        lines
                            .into_iter()
                            .map(|line| Line::styled(line, Style::default()))
                            .collect::<Vec<_>>(),
                        cursor_x,
                        cursor_y,
                    )
                };
                frame.render_widget(
                    Paragraph::new(Text::from(input_lines)).block(input_block),
                    layout[1],
                );

                let status_text = if self.is_ready() {
                    "[Enter: Send] [Ctrl+J: Newline] [Up/Down: Scroll] [Ctrl+C: Exit]"
                } else {
                    self.processing_note
                        .as_deref()
                        .unwrap_or("Claude is processing this turn... [Up/Down/PgUp/PgDn: Scroll]")
                };
                frame.render_widget(
                    Paragraph::new(Line::styled(
                        status_text,
                        Style::default().fg(Color::DarkGray),
                    )),
                    layout[2],
                );

                if self.is_ready() && input_inner.width > 0 && input_inner.height > 0 {
                    frame.set_cursor_position(Position {
                        x: input_inner.x.saturating_add(cursor_x),
                        y: input_inner.y.saturating_add(cursor_y),
                    });
                }
            })
            .context("failed to draw claude-sdk chat TUI frame")?;
        Ok(())
    }
}

fn apply_chat_turn_ui_event(
    state: &mut ChatTuiState,
    session_control: &mut ManagedSessionControl,
    event: ChatTurnUiEvent,
) -> bool {
    match event {
        ChatTurnUiEvent::AssistantDelta(delta) => {
            state.set_processing_note("Claude is responding... [Up/Down/PgUp/PgDn: Scroll]");
            state.append_stream_delta(&delta);
            false
        }
        ChatTurnUiEvent::AssistantMessage(text) => {
            state.set_processing_note("Claude is responding... [Up/Down/PgUp/PgDn: Scroll]");
            state.note_stream_assistant_message(&text);
            false
        }
        ChatTurnUiEvent::ToolCall(tool_name) => {
            state.set_processing_note(format!("Tool: {tool_name} [Up/Down/PgUp/PgDn: Scroll]"));
            false
        }
        ChatTurnUiEvent::ToolResult(tool_name) => {
            state.set_processing_note(format!(
                "Tool finished: {tool_name} [Up/Down/PgUp/PgDn: Scroll]"
            ));
            false
        }
        ChatTurnUiEvent::Completed(result) => {
            match *result {
                Ok(result) => {
                    *session_control =
                        ManagedSessionControl::followup(result.outcome.provider_session_id);
                    state.finish_streaming_assistant(result.assistant_text);
                    for warning in result.auto_finalize.warnings {
                        state.push_entry(ChatTuiRole::System, format!("warning: {warning}"));
                    }
                }
                Err(error) => {
                    state.finish_streaming_assistant(None);
                    state.push_entry(ChatTuiRole::Error, error);
                }
            }
            state.set_processing(false);
            true
        }
    }
}

fn push_transcript_entry_lines(lines: &mut Vec<Line<'static>>, entry: &ChatTuiEntry) {
    let (prefix, prefix_style, body_style) = match entry.role {
        ChatTuiRole::User => (
            "you>",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
            Style::default(),
        ),
        ChatTuiRole::Assistant => (
            "claude>",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
            Style::default(),
        ),
        ChatTuiRole::System => (
            "info>",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
            Style::default().fg(Color::Gray),
        ),
        ChatTuiRole::Error => (
            "error>",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            Style::default().fg(Color::Red),
        ),
    };

    let mut had_line = false;
    for (idx, line) in entry.text.lines().enumerate() {
        had_line = true;
        let display_prefix = if idx == 0 { prefix } else { "....>" };
        lines.push(Line::from(vec![
            Span::styled(format!("{display_prefix:>7} "), prefix_style),
            Span::styled(line.to_string(), body_style),
        ]));
    }
    if !had_line {
        lines.push(Line::from(vec![
            Span::styled(format!("{prefix:>7} "), prefix_style),
            Span::styled(String::new(), body_style),
        ]));
    }
    lines.push(Line::raw(""));
}

struct ChatFullscreenModeGuard {
    active: bool,
}

impl ChatFullscreenModeGuard {
    fn enter() -> Result<Self> {
        terminal::enable_raw_mode().context("failed to enable raw mode for chat TUI")?;
        execute!(
            io::stdout(),
            terminal::EnterAlternateScreen,
            EnableBracketedPaste
        )
        .context("failed to enter alternate screen for chat TUI")?;
        let _ = execute!(
            io::stdout(),
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
            )
        );
        Ok(Self { active: true })
    }
}

impl Drop for ChatFullscreenModeGuard {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
        let _ = execute!(
            io::stdout(),
            DisableBracketedPaste,
            terminal::LeaveAlternateScreen
        );
        let _ = terminal::disable_raw_mode();
        let _ = execute!(io::stdout(), cursor::Show);
    }
}

fn prev_char_pos(text: &str, cursor_pos: usize) -> usize {
    text[..cursor_pos]
        .char_indices()
        .next_back()
        .map(|(i, _)| i)
        .unwrap_or(0)
}

fn next_char_pos(text: &str, cursor_pos: usize) -> usize {
    text[cursor_pos..]
        .char_indices()
        .nth(1)
        .map(|(i, _)| cursor_pos + i)
        .unwrap_or(text.len())
}

fn visible_input_and_cursor(
    input: &str,
    cursor_pos: usize,
    content_width: usize,
    content_height: usize,
) -> (Vec<String>, u16, u16) {
    if content_width == 0 || content_height == 0 {
        return (Vec::new(), 0, 0);
    }

    let mut wrapped_lines = Vec::new();
    let mut current_line = String::new();
    let mut current_col = 0usize;
    let mut cursor_row = 0usize;
    let mut cursor_col = 0usize;
    let mut line_index = 0usize;

    for (idx, ch) in input.char_indices() {
        if ch == '\n' {
            if idx == cursor_pos {
                cursor_row = line_index;
                cursor_col = current_col;
            }
            wrapped_lines.push(std::mem::take(&mut current_line));
            line_index += 1;
            current_col = 0;
            continue;
        }

        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(1).max(1);
        if current_col + ch_width > content_width && current_col > 0 {
            wrapped_lines.push(std::mem::take(&mut current_line));
            line_index += 1;
            current_col = 0;
        }

        if idx == cursor_pos {
            cursor_row = line_index;
            cursor_col = current_col;
        }

        current_line.push(ch);
        current_col += ch_width;
    }

    if cursor_pos == input.len() {
        cursor_row = line_index;
        cursor_col = current_col;
    }
    wrapped_lines.push(current_line);

    let start_row = cursor_row.saturating_sub(content_height.saturating_sub(1));
    let end_row = (start_row + content_height).min(wrapped_lines.len());
    let mut visible_lines = wrapped_lines[start_row..end_row].to_vec();
    while visible_lines.len() < content_height {
        visible_lines.push(String::new());
    }

    let cursor_y = cursor_row.saturating_sub(start_row).min(u16::MAX as usize) as u16;
    let max_cursor_x = content_width.saturating_sub(1);
    let cursor_x = cursor_col.min(max_cursor_x).min(u16::MAX as usize) as u16;
    (visible_lines, cursor_x, cursor_y)
}

#[allow(clippy::too_many_arguments)]
async fn execute_chat_turn(
    args: &ChatManagedArgs,
    storage_path: &Path,
    cwd: &Path,
    helper_path: &Path,
    custom_helper: bool,
    session_control: &ManagedSessionControl,
    prompt: String,
    render_mode: StreamingRenderMode,
    ui_event_tx: Option<UnboundedSender<ChatTurnUiEvent>>,
) -> Result<ManagedStreamingTurnOutcome> {
    let helper_request = build_chat_streaming_helper_request(ManagedStreamingTurnArgs {
        prompt,
        cwd: cwd.to_path_buf(),
        model: args.model.clone(),
        permission_mode: args.permission_mode.clone(),
        timeout_seconds: Some(args.timeout_seconds),
        tools: args.tools.clone(),
        prompt_suggestions: args.prompt_suggestions,
        agent_progress_summaries: args.agent_progress_summaries,
        interactive_approvals: args.interactive_approvals,
        enable_file_checkpointing: args.enable_file_checkpointing,
        continue_session: session_control.continue_session,
        resume: session_control.resume.clone(),
        fork_session: session_control.fork_session,
        session_id: session_control.session_id.clone(),
        resume_session_at: session_control.resume_session_at.clone(),
    });

    execute_managed_streaming_turn(
        storage_path,
        ManagedStreamingTurnKind::Chat,
        custom_helper,
        &args.python_binary,
        helper_path,
        &helper_request,
        render_mode,
        ui_event_tx,
    )
    .await
}

async fn chat_managed_stdio(
    args: &ChatManagedArgs,
    storage_path: &Path,
    cwd: &Path,
    helper_path: &Path,
    custom_helper: bool,
) -> Result<()> {
    let stdin = io::stdin();
    let mut session_control = ManagedSessionControl::from_chat_args(args);
    loop {
        let Some(prompt) = read_chat_turn(&stdin, false)? else {
            return Ok(());
        };
        let command = prompt.trim();
        if command.is_empty() {
            continue;
        }
        match command {
            "/help" => {
                print_chat_help();
                continue;
            }
            "/exit" | "/quit" => return Ok(()),
            _ => {}
        }

        match execute_chat_turn(
            args,
            storage_path,
            cwd,
            helper_path,
            custom_helper,
            &session_control,
            prompt,
            StreamingRenderMode::Human {
                print_completion: false,
            },
            None,
        )
        .await
        {
            Ok(result) => {
                session_control =
                    ManagedSessionControl::followup(result.outcome.provider_session_id);
            }
            Err(error) => eprintln!("error: {error}"),
        }
    }
}

async fn chat_managed_fullscreen_tui(
    args: &ChatManagedArgs,
    storage_path: &Path,
    cwd: &Path,
    helper_path: &Path,
    custom_helper: bool,
) -> Result<()> {
    let _guard = ChatFullscreenModeGuard::enter()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal =
        Terminal::new(backend).context("failed to initialize terminal backend for chat TUI")?;
    terminal
        .clear()
        .context("failed to clear chat TUI screen")?;

    let mut state = ChatTuiState::new(args.model.clone(), cwd);
    let mut session_control = ManagedSessionControl::from_chat_args(args);
    let mut pending_turn: Option<PendingChatTurn> = None;

    loop {
        let mut completed_turn = false;
        if let Some(turn) = pending_turn.as_mut() {
            loop {
                match turn.rx.try_recv() {
                    Ok(event) => {
                        if apply_chat_turn_ui_event(&mut state, &mut session_control, event) {
                            completed_turn = true;
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        completed_turn = true;
                        break;
                    }
                }
            }
        }
        if completed_turn {
            pending_turn = None;
        }

        state.draw(&mut terminal)?;
        if !event::poll(Duration::from_millis(80)).context("failed to poll chat TUI event")? {
            continue;
        }

        match event::read().context("failed to read chat TUI event")? {
            Event::Key(KeyEvent {
                code,
                modifiers,
                kind: KeyEventKind::Press,
                ..
            }) => {
                if modifiers.contains(KeyModifiers::CONTROL) && code == KeyCode::Char('c') {
                    if let Some(turn) = pending_turn.take() {
                        turn.task.abort();
                    }
                    break;
                }

                match code {
                    KeyCode::Up => {
                        state.scroll_up(2);
                        continue;
                    }
                    KeyCode::PageUp => {
                        state.scroll_up(10);
                        continue;
                    }
                    KeyCode::Down => {
                        state.scroll_down(2);
                        continue;
                    }
                    KeyCode::PageDown => {
                        state.scroll_down(10);
                        continue;
                    }
                    _ => {}
                }

                if !state.is_ready() {
                    continue;
                }

                match code {
                    KeyCode::Left => state.cursor_left(),
                    KeyCode::Right => state.cursor_right(),
                    KeyCode::Home => state.cursor_home(),
                    KeyCode::End => state.cursor_end(),
                    KeyCode::Backspace => state.backspace(),
                    KeyCode::Delete => state.delete(),
                    KeyCode::Esc => state.clear_input(),
                    KeyCode::Char('j') if modifiers.contains(KeyModifiers::CONTROL) => {
                        state.insert_char('\n');
                    }
                    KeyCode::Enter => {
                        let prompt = state.take_input();
                        let command = prompt.trim();
                        if command.is_empty() {
                            continue;
                        }

                        match command {
                            "/help" => {
                                state.push_entry(
                                    ChatTuiRole::System,
                                    "Commands: /help, /exit, /quit\nEnter sends; Ctrl+J inserts a newline.",
                                );
                                continue;
                            }
                            "/exit" | "/quit" => break,
                            _ => {}
                        }

                        state.push_entry(ChatTuiRole::User, prompt.clone());
                        state.start_streaming_assistant();
                        state.set_processing(true);
                        state.draw(&mut terminal)?;
                        let (tx, rx) = unbounded_channel();
                        let args_clone = args.clone();
                        let storage_path = storage_path.to_path_buf();
                        let cwd = cwd.to_path_buf();
                        let helper_path = helper_path.to_path_buf();
                        let session_control = session_control.clone();
                        let task = tokio::spawn(async move {
                            let result = execute_chat_turn(
                                &args_clone,
                                &storage_path,
                                &cwd,
                                &helper_path,
                                custom_helper,
                                &session_control,
                                prompt,
                                StreamingRenderMode::Quiet,
                                Some(tx.clone()),
                            )
                            .await
                            .map_err(|error| error.to_string());
                            let _ = tx.send(ChatTurnUiEvent::Completed(Box::new(result)));
                        });
                        pending_turn = Some(PendingChatTurn { rx, task });
                    }
                    KeyCode::Char(c)
                        if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                    {
                        state.insert_char(c);
                    }
                    _ => {}
                }
            }
            Event::Paste(text) => {
                if state.is_ready() {
                    state.insert_text(&text);
                }
            }
            Event::Resize(_, _) => {}
            _ => {}
        }
    }

    terminal
        .show_cursor()
        .context("failed to restore cursor visibility")?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn execute_managed_streaming_turn(
    storage_path: &Path,
    turn_kind: ManagedStreamingTurnKind,
    custom_helper: bool,
    python_binary: &str,
    helper_path: &Path,
    helper_request: &ManagedHelperRequest,
    render_mode: StreamingRenderMode,
    ui_event_tx: Option<UnboundedSender<ChatTurnUiEvent>>,
) -> Result<ManagedStreamingTurnOutcome> {
    let serialized_request = serde_json::to_vec(helper_request)
        .context("failed to serialize Claude SDK helper streaming request")?;
    let executable = if custom_helper {
        helper_path.display().to_string()
    } else {
        python_binary.to_string()
    };
    let mut child = build_helper_command(custom_helper, python_binary, helper_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| {
            format!(
                "failed to start Claude SDK helper with '{}' '{}'",
                executable,
                helper_path.display()
            )
        })?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(&serialized_request)
            .await
            .context("failed to send streaming request to Claude SDK helper")?;
    }

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("Claude SDK helper stdout was not captured"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("Claude SDK helper stderr was not captured"))?;

    let stderr_task = tokio::spawn(async move {
        let mut stderr = stderr;
        let mut bytes = Vec::new();
        stderr
            .read_to_end(&mut bytes)
            .await
            .map(|_| bytes)
            .map_err(anyhow::Error::from)
    });

    let mut final_artifact = None;
    let mut latest_persisted_outcome = None;
    let mut persistence_warnings = Vec::new();
    let mut assistant_line_open = false;
    let mut streamed_assistant_text = String::new();
    let mut stdout_lines = BufReader::new(stdout).lines();
    while let Some(line) = stdout_lines
        .next_line()
        .await
        .context("failed to read Claude SDK helper stream")?
    {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let event: Value = serde_json::from_str(trimmed).with_context(|| {
            format!("failed to parse Claude SDK helper NDJSON event: {trimmed}")
        })?;
        match render_mode {
            StreamingRenderMode::Ndjson => println!("{trimmed}"),
            StreamingRenderMode::Human { .. } => {
                render_stream_event_human(&event, &mut assistant_line_open)?;
            }
            StreamingRenderMode::Quiet => {}
        }
        maybe_emit_chat_turn_ui_event(&event, ui_event_tx.as_ref());
        capture_assistant_text_from_stream_event(&event, &mut streamed_assistant_text);

        if event.get("event").and_then(Value::as_str) == Some("final_artifact")
            && let Some(artifact_value) = event.get("artifact")
        {
            let artifact = serde_json::from_value::<ClaudeManagedArtifact>(artifact_value.clone())
                .context("failed to parse final managed artifact from helper stream")?;
            match persist_managed_artifact(storage_path, &artifact).await {
                Ok(outcome) => {
                    if let Err(error) =
                        sync_incremental_managed_inputs(storage_path, &outcome).await
                    {
                        persistence_warnings.push(format!("incremental managed inputs: {error}"));
                    }
                    latest_persisted_outcome = Some(outcome);
                }
                Err(error) => {
                    persistence_warnings.push(format!("incremental artifact persist: {error}"))
                }
            }
            final_artifact = Some(artifact);
        } else if event.get("event").and_then(Value::as_str) == Some("runtime_snapshot")
            && let Some(artifact_value) = event.get("artifact")
        {
            let artifact = serde_json::from_value::<ClaudeManagedArtifact>(artifact_value.clone())
                .context("failed to parse runtime snapshot managed artifact")?;
            match persist_managed_artifact(storage_path, &artifact).await {
                Ok(outcome) => {
                    if let Err(error) =
                        sync_incremental_managed_inputs(storage_path, &outcome).await
                    {
                        persistence_warnings.push(format!("incremental managed inputs: {error}"));
                    }
                    latest_persisted_outcome = Some(outcome);
                }
                Err(error) => {
                    if !ignore_incomplete_runtime_snapshot_error(&error) {
                        persistence_warnings.push(format!("incremental artifact persist: {error}"));
                    }
                }
            }
        }
    }

    if assistant_line_open && matches!(render_mode, StreamingRenderMode::Human { .. }) {
        println!();
    }

    let status = child
        .wait()
        .await
        .context("failed to wait for Claude SDK helper process")?;
    let stderr = stderr_task
        .await
        .context("failed to join Claude SDK helper stderr reader")??;
    let stderr = String::from_utf8(stderr).context("helper stderr is not valid UTF-8")?;

    if !status.success() {
        let detail = if stderr.trim().is_empty() {
            "helper exited with a non-zero status".to_string()
        } else {
            stderr.trim().to_string()
        };
        return Err(anyhow!("Claude SDK helper failed: {detail}"));
    }

    let artifact = final_artifact
        .ok_or_else(|| anyhow!("Claude SDK helper stream ended without a final_artifact event"))?;
    let outcome = if let Some(outcome) = latest_persisted_outcome {
        outcome
    } else {
        let outcome = persist_managed_artifact(storage_path, &artifact).await?;
        if let Err(error) = sync_incremental_managed_inputs(storage_path, &outcome).await {
            persistence_warnings.push(format!("incremental managed inputs: {error}"));
        }
        outcome
    };
    let mut auto_finalize =
        auto_finalize_streaming_turn(storage_path, &outcome.ai_session_id, turn_kind).await;
    auto_finalize.warnings.extend(persistence_warnings);
    ensure_managed_artifact_succeeded(&artifact)?;
    let assistant_text = if streamed_assistant_text.trim().is_empty() {
        extract_latest_assistant_text(&artifact)
    } else {
        Some(streamed_assistant_text.trim().to_string())
    };

    let result = ManagedStreamingTurnOutcome {
        outcome,
        auto_finalize,
        assistant_text,
    };
    match render_mode {
        StreamingRenderMode::Ndjson => {
            println!(
                "{}",
                serde_json::to_string(&StreamingRunResult {
                    ok: true,
                    event: "libra_result",
                    provider_session_id: result.outcome.provider_session_id.clone(),
                    ai_session_id: result.outcome.ai_session_id.clone(),
                    raw_artifact_path: result.outcome.raw_artifact_path.clone(),
                    audit_bundle_path: result.outcome.audit_bundle_path.clone(),
                    already_persisted: result.outcome.already_persisted,
                    auto_finalize: result.auto_finalize.clone(),
                })
                .context("failed to serialize streaming Claude SDK result")?
            );
        }
        StreamingRenderMode::Human { print_completion } => {
            print_streaming_turn_human_result(&result, print_completion);
        }
        StreamingRenderMode::Quiet => {}
    }

    Ok(result)
}

fn print_streaming_turn_human_result(result: &ManagedStreamingTurnOutcome, print_completion: bool) {
    if print_completion {
        println!(
            "Claude SDK session persisted: ai_session_id={} provider_session_id={}",
            result.outcome.ai_session_id, result.outcome.provider_session_id
        );
        println!("Managed artifact: {}", result.outcome.raw_artifact_path);
        println!("Audit bundle: {}", result.outcome.audit_bundle_path);
    }
    for warning in &result.auto_finalize.warnings {
        eprintln!("warning: {warning}");
    }
}

fn print_chat_help() {
    println!("Commands:");
    println!("  /help  Show chat commands");
    println!("  /exit  Exit chat");
    println!("  /quit  Exit chat");
    println!("Interactive input:");
    println!("  Enter   Send the current prompt");
    println!("  Ctrl+J  Insert a newline");
    println!("Any other input, including unknown /commands, is sent to Claude.");
}

struct ChatInputModeGuard {
    active: bool,
}

impl ChatInputModeGuard {
    fn new(active: bool) -> Result<Self> {
        if !active {
            return Ok(Self { active: false });
        }

        terminal::enable_raw_mode().context("failed to enable raw mode for chat input")?;
        execute!(io::stdout(), EnableBracketedPaste)
            .context("failed to enable bracketed paste for chat input")?;
        let _ = execute!(
            io::stdout(),
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
            )
        );
        Ok(Self { active: true })
    }
}

impl Drop for ChatInputModeGuard {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
        let _ = execute!(io::stdout(), DisableBracketedPaste);
        let _ = terminal::disable_raw_mode();
    }
}

fn render_chat_buffer(previous_lines: usize, buffer: &str) -> Result<usize> {
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    let terminal_columns = current_terminal_columns();

    if previous_lines > 0 {
        let rows_to_rewind = previous_lines.saturating_sub(1).min(u16::MAX as usize) as u16;
        execute!(
            stdout,
            cursor::MoveToColumn(0),
            cursor::MoveUp(rows_to_rewind),
            terminal::Clear(terminal::ClearType::FromCursorDown)
        )
        .context("failed to redraw chat input")?;
    }

    let logical_lines = buffer.split('\n').collect::<Vec<_>>();
    for (index, line) in logical_lines.iter().enumerate() {
        if index == 0 {
            write!(stdout, "{CHAT_PROMPT_PREFIX}{line}").context("failed to render chat prompt")?;
        } else {
            write!(stdout, "\r\n{CHAT_CONTINUATION_PREFIX}{line}")
                .context("failed to render chat continuation")?;
        }
    }
    stdout
        .flush()
        .context("failed to flush chat input render")?;
    Ok(rendered_chat_rows(buffer, terminal_columns))
}

fn current_terminal_columns() -> usize {
    terminal::size()
        .map(|(columns, _)| usize::from(columns.max(1)))
        .unwrap_or(DEFAULT_CHAT_TERMINAL_COLUMNS)
}

fn rendered_chat_rows(buffer: &str, terminal_columns: usize) -> usize {
    let columns = terminal_columns.max(1);
    let rows = buffer
        .split('\n')
        .enumerate()
        .map(|(index, line)| {
            let prefix = if index == 0 {
                CHAT_PROMPT_PREFIX
            } else {
                CHAT_CONTINUATION_PREFIX
            };
            let visual_width = prefix.width().saturating_add(line.width());
            visual_width.max(1).div_ceil(columns)
        })
        .sum::<usize>();
    rows.max(1)
}

fn read_chat_turn(stdin: &io::Stdin, show_prompt: bool) -> Result<Option<String>> {
    if !show_prompt {
        let mut input = String::new();
        let bytes_read = stdin
            .read_line(&mut input)
            .context("failed to read chat input")?;
        if bytes_read == 0 {
            return Ok(None);
        }
        return Ok(Some(input.trim_end_matches(['\n', '\r']).to_string()));
    }

    let _guard = ChatInputModeGuard::new(true)?;
    let mut buffer = String::new();
    let mut rendered_lines = render_chat_buffer(0, &buffer)?;
    loop {
        match event::read().context("failed to read chat input event")? {
            Event::Key(KeyEvent {
                code,
                modifiers,
                kind: KeyEventKind::Press,
                ..
            }) => match code {
                KeyCode::Enter => {
                    let trimmed = buffer.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    println!();
                    return Ok(Some(buffer));
                }
                KeyCode::Char('j') if modifiers.contains(KeyModifiers::CONTROL) => {
                    buffer.push('\n');
                    rendered_lines = render_chat_buffer(rendered_lines, &buffer)?;
                }
                KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                    println!();
                    return Ok(None);
                }
                KeyCode::Esc => {
                    if buffer.is_empty() {
                        println!();
                        return Ok(None);
                    }
                    buffer.clear();
                    rendered_lines = render_chat_buffer(rendered_lines, &buffer)?;
                }
                KeyCode::Backspace => {
                    if buffer.pop().is_some() {
                        rendered_lines = render_chat_buffer(rendered_lines, &buffer)?;
                    }
                }
                KeyCode::Char(c)
                    if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    buffer.push(c);
                    rendered_lines = render_chat_buffer(rendered_lines, &buffer)?;
                }
                _ => {}
            },
            Event::Paste(text) => {
                buffer.push_str(&text);
                rendered_lines = render_chat_buffer(rendered_lines, &buffer)?;
            }
            Event::Resize(_, _) => {
                rendered_lines = render_chat_buffer(rendered_lines, &buffer)?;
            }
            _ => {}
        }
    }
}

async fn auto_finalize_streaming_turn(
    storage_path: &Path,
    ai_session_id: &str,
    turn_kind: ManagedStreamingTurnKind,
) -> StreamingFinalizeSummary {
    let mut summary = StreamingFinalizeSummary::default();
    if turn_kind == ManagedStreamingTurnKind::Run {
        let resolve_result = resolve_extraction_internal(ResolveExtractionArgs {
            extraction: None,
            ai_session_id: Some(ai_session_id.to_string()),
            risk_level: None,
            created_by_id: "claude-sdk".to_string(),
            output: None,
        })
        .await;
        if let Ok(resolve_result) = &resolve_result {
            summary.resolved_extraction_path = Some(resolve_result.resolved_spec_path.clone());
            match persist_intent_internal(PersistIntentArgs {
                resolution: None,
                ai_session_id: Some(ai_session_id.to_string()),
                output: None,
            })
            .await
            {
                Ok(intent_result) => {
                    summary.intent_id = Some(intent_result.intent_id);
                }
                Err(error) => summary.warnings.push(format!("persist-intent: {error}")),
            }
        } else if let Err(error) = resolve_result {
            summary
                .warnings
                .push(format!("resolve-extraction: {error}"));
        }
    }

    if let Ok(Some(existing)) = read_existing_binding_if_live::<ClaudeFormalRunBindingArtifact>(
        storage_path,
        &formal_run_binding_path(storage_path, ai_session_id),
        "Claude formal run binding",
        &[
            ("task", |binding| binding.task_id.as_str()),
            ("run", |binding| binding.run_id.as_str()),
        ],
    )
    .await
    {
        summary.run_id = Some(existing.run_id);
    } else {
        match bridge_run_internal(BridgeRunArgs {
            ai_session_id: ai_session_id.to_string(),
            intent_binding: None,
            intent_id: None,
        })
        .await
        {
            Ok(bridge_result) => {
                summary.run_id = Some(bridge_result.binding.run_id);
            }
            Err(error) => {
                summary.warnings.push(format!("bridge-run: {error}"));
                return summary;
            }
        }
    }

    if let Ok((_, audit_bundle)) =
        load_managed_audit_bundle_for_ai_session(storage_path, ai_session_id).await
    {
        let provider_session_object_id =
            build_provider_session_object_id(&audit_bundle.provider_session_id);
        if let Ok(provider_session_object_id) = provider_session_object_id {
            let provider_session_path =
                provider_session_artifact_path(storage_path, &provider_session_object_id);
            let provider_evidence_input_object_id =
                build_evidence_input_object_id(&audit_bundle.provider_session_id);
            if let Ok(provider_evidence_input_object_id) = provider_evidence_input_object_id {
                let provider_evidence_input_path =
                    evidence_input_artifact_path(storage_path, &provider_evidence_input_object_id);
                let managed_object_id = build_managed_evidence_input_object_id(ai_session_id);
                if let Ok(managed_object_id) = managed_object_id {
                    let managed_artifact_path =
                        managed_evidence_input_artifact_path(storage_path, &managed_object_id);
                    let managed_artifact = build_managed_evidence_input_artifact(
                        &audit_bundle,
                        ManagedEvidenceInputBuildContext {
                            ai_session_id,
                            raw_artifact_path:
                                &crate::command::claude_sdk::common::managed_artifact_path(
                                    storage_path,
                                    ai_session_id,
                                ),
                            audit_bundle_path: &managed_audit_bundle_path(
                                storage_path,
                                ai_session_id,
                            ),
                            provider_session_path: provider_session_path
                                .exists()
                                .then_some(provider_session_path.as_path()),
                            provider_evidence_input_path: provider_evidence_input_path
                                .exists()
                                .then_some(provider_evidence_input_path.as_path()),
                            captured_at: Utc::now().to_rfc3339(),
                        },
                        managed_object_id,
                    );
                    if let Err(error) = persist_managed_evidence_input_artifact(
                        storage_path,
                        &managed_artifact_path,
                        &managed_artifact,
                    )
                    .await
                    {
                        summary
                            .warnings
                            .push(format!("build-managed-evidence-input: {error}"));
                    }
                }
            }
        }
    }

    match persist_patchset_internal(PersistPatchSetArgs {
        ai_session_id: ai_session_id.to_string(),
        output: None,
    })
    .await
    {
        Ok(result) => {
            summary.patchset_id = Some(result.binding.patchset_id);
        }
        Err(error) => {
            if !error.to_string().contains("contains no touched files") {
                summary.warnings.push(format!("persist-patchset: {error}"));
            }
        }
    }

    match persist_evidence_internal(PersistEvidenceArgs {
        ai_session_id: ai_session_id.to_string(),
    })
    .await
    {
        Ok(_result) => {}
        Err(error) => {
            summary.warnings.push(format!("persist-evidence: {error}"));
            return summary;
        }
    }

    if let Ok((audit_bundle_path, audit_bundle)) =
        load_managed_audit_bundle_for_ai_session(storage_path, ai_session_id).await
    {
        let decision_object_id = build_decision_input_object_id(ai_session_id);
        if let Ok(decision_object_id) = decision_object_id {
            let decision_artifact_path =
                decision_input_artifact_path(storage_path, &decision_object_id);
            let managed_input_path = managed_evidence_input_artifact_path(
                storage_path,
                &build_managed_evidence_input_object_id(ai_session_id).unwrap_or_default(),
            );
            let decision_artifact = build_decision_input_artifact(
                ai_session_id,
                &audit_bundle_path,
                &audit_bundle,
                managed_input_path
                    .exists()
                    .then_some(managed_input_path.as_path()),
                decision_object_id,
                Utc::now().to_rfc3339(),
            );
            if let Err(error) = persist_decision_input_artifact(
                storage_path,
                &decision_artifact_path,
                &decision_artifact,
            )
            .await
            {
                summary
                    .warnings
                    .push(format!("build-decision-input: {error}"));
            }
        }
    }

    match persist_decision_internal(PersistDecisionArgs {
        ai_session_id: ai_session_id.to_string(),
    })
    .await
    {
        Ok(result) => {
            summary.decision_id = Some(result.binding.decision_id);
            if summary.run_id.is_none() {
                summary.run_id = Some(result.binding.run_id);
            }
        }
        Err(error) => {
            summary.warnings.push(format!("persist-decision: {error}"));
        }
    }

    summary
}

fn render_stream_event_human(event: &Value, assistant_line_open: &mut bool) -> Result<()> {
    let kind = event
        .get("event")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("helper stream event is missing string field 'event'"))?;

    match kind {
        "session_init" => {
            finish_assistant_line(assistant_line_open)?;
            if let Some(message) = event.get("message") {
                let session_id = message
                    .get("session_id")
                    .and_then(Value::as_str)
                    .unwrap_or("-");
                let model = message.get("model").and_then(Value::as_str).unwrap_or("-");
                eprintln!("Claude session started: {} ({})", session_id, model);
            }
        }
        "assistant_delta" => {
            if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                print!("{delta}");
                io::stdout()
                    .flush()
                    .context("failed to flush assistant delta output")?;
                *assistant_line_open = true;
            }
        }
        "assistant_message" => {
            if !*assistant_line_open
                && let Some(text) = extract_assistant_text(event.get("message"))
                && !text.trim().is_empty()
            {
                println!("{text}");
            }
        }
        "tool_call" => {
            finish_assistant_line(assistant_line_open)?;
            if let Some(tool_name) = event
                .get("input")
                .and_then(|value| value.get("tool_name"))
                .and_then(Value::as_str)
            {
                eprintln!("tool: {tool_name}");
            }
        }
        "tool_result" => {
            finish_assistant_line(assistant_line_open)?;
            if let Some(tool_name) = event
                .get("input")
                .and_then(|value| value.get("tool_name"))
                .and_then(Value::as_str)
            {
                eprintln!("tool-result: {tool_name}");
            }
        }
        "permission_mode_changed" => {
            finish_assistant_line(assistant_line_open)?;
            if let Some(mode) = event.get("mode").and_then(Value::as_str) {
                eprintln!("permission mode -> {mode}");
            }
        }
        "result" => finish_assistant_line(assistant_line_open)?,
        "error" => {
            finish_assistant_line(assistant_line_open)?;
            if let Some(error) = event.get("error").and_then(Value::as_str) {
                eprintln!("error: {error}");
            }
        }
        _ => {}
    }

    Ok(())
}

fn maybe_emit_chat_turn_ui_event(event: &Value, tx: Option<&UnboundedSender<ChatTurnUiEvent>>) {
    let Some(tx) = tx else {
        return;
    };
    let Some(kind) = event.get("event").and_then(Value::as_str) else {
        return;
    };

    let ui_event = match kind {
        "assistant_delta" => event
            .get("delta")
            .and_then(Value::as_str)
            .map(|delta| ChatTurnUiEvent::AssistantDelta(delta.to_string())),
        "assistant_message" => {
            extract_assistant_text(event.get("message")).map(ChatTurnUiEvent::AssistantMessage)
        }
        "tool_call" => event
            .get("input")
            .and_then(|value| value.get("tool_name"))
            .and_then(Value::as_str)
            .map(|name| ChatTurnUiEvent::ToolCall(name.to_string())),
        "tool_result" => event
            .get("input")
            .and_then(|value| value.get("tool_name"))
            .and_then(Value::as_str)
            .map(|name| ChatTurnUiEvent::ToolResult(name.to_string())),
        _ => None,
    };

    if let Some(ui_event) = ui_event {
        let _ = tx.send(ui_event);
    }
}

fn finish_assistant_line(assistant_line_open: &mut bool) -> Result<()> {
    if *assistant_line_open {
        println!();
        io::stdout()
            .flush()
            .context("failed to flush assistant output line")?;
        *assistant_line_open = false;
    }
    Ok(())
}

fn extract_assistant_text(message: Option<&Value>) -> Option<String> {
    let content = message
        .and_then(|value| value.get("message"))
        .and_then(|value| value.get("content"))
        .and_then(Value::as_array)?;
    let mut parts = Vec::new();
    for block in content {
        if block.get("type").and_then(Value::as_str) == Some("text")
            && let Some(text) = block.get("text").and_then(Value::as_str)
        {
            parts.push(text.to_string());
        }
    }
    (!parts.is_empty()).then(|| parts.join(""))
}

fn capture_assistant_text_from_stream_event(event: &Value, sink: &mut String) {
    let Some(kind) = event.get("event").and_then(Value::as_str) else {
        return;
    };

    match kind {
        "assistant_delta" => {
            if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                sink.push_str(delta);
            }
        }
        "assistant_message" if sink.trim().is_empty() => {
            if let Some(text) = extract_assistant_text(event.get("message"))
                && !text.trim().is_empty()
            {
                sink.push_str(text.trim());
            }
        }
        _ => {}
    }
}

fn extract_latest_assistant_text(artifact: &ClaudeManagedArtifact) -> Option<String> {
    artifact
        .messages
        .iter()
        .rev()
        .find(|message| message.get("type").and_then(Value::as_str) == Some("assistant"))
        .and_then(|message| extract_assistant_text(Some(message)))
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

impl ManagedSessionControl {
    fn from_chat_args(args: &ChatManagedArgs) -> Self {
        Self {
            continue_session: args.continue_session,
            resume: args.resume.clone(),
            fork_session: args.fork_session,
            session_id: args.session_id.clone(),
            resume_session_at: args.resume_session_at.clone(),
        }
    }

    fn followup(provider_session_id: String) -> Self {
        Self {
            continue_session: false,
            resume: Some(provider_session_id),
            fork_session: false,
            session_id: None,
            resume_session_at: None,
        }
    }
}

fn validate_run_managed_args(args: &RunManagedArgs) -> Result<()> {
    validate_managed_session_control_args(
        args.continue_session,
        args.resume.as_deref(),
        args.fork_session,
        args.session_id.as_deref(),
        args.resume_session_at.as_deref(),
    )
}

fn validate_chat_managed_args(args: &ChatManagedArgs, output: &OutputConfig) -> Result<()> {
    if output.is_json() || output.quiet {
        bail!(
            "claude-sdk chat is interactive and does not support --json, --machine, or --quiet; use claude-sdk run for scripted output"
        );
    }

    validate_managed_session_control_args(
        args.continue_session,
        args.resume.as_deref(),
        args.fork_session,
        args.session_id.as_deref(),
        args.resume_session_at.as_deref(),
    )
}

fn validate_managed_session_control_args(
    continue_session: bool,
    resume: Option<&str>,
    fork_session: bool,
    session_id: Option<&str>,
    resume_session_at: Option<&str>,
) -> Result<()> {
    if continue_session && resume.is_some() {
        bail!("--continue cannot be combined with --resume");
    }
    if resume_session_at.is_some() && resume.is_none() {
        bail!("--resume-session-at requires --resume");
    }
    if fork_session && resume.is_none() {
        bail!("--fork-session requires --resume");
    }
    if session_id.is_some() && (continue_session || resume.is_some()) && !fork_session {
        bail!("--session-id requires --fork-session when combined with --continue or --resume");
    }

    if let Some(resume) = resume {
        validate_uuid_flag_value(resume, "--resume")?;
    }
    if let Some(session_id) = session_id {
        validate_uuid_flag_value(session_id, "--session-id")?;
    }
    if let Some(resume_session_at) = resume_session_at {
        validate_uuid_flag_value(resume_session_at, "--resume-session-at")?;
    }

    Ok(())
}

fn validate_uuid_flag_value(value: &str, flag: &str) -> Result<()> {
    Uuid::parse_str(value).with_context(|| format!("{flag} must be a valid UUID"))?;
    Ok(())
}

fn managed_artifact_terminal_error(artifact: &ClaudeManagedArtifact) -> Option<String> {
    if let Some(result) = artifact.result_message.as_ref()
        && (result.is_error == Some(true)
            || matches!(result.subtype.as_deref(), Some("error" | "failed")))
    {
        let detail = result
            .result
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "managed Claude SDK run reported an error".to_string());
        return Some(format!("Claude Code returned an error result: {detail}"));
    }
    if artifact.helper_timed_out {
        return Some("Claude SDK helper timed out".to_string());
    }
    artifact.helper_error.clone()
}

fn ensure_managed_artifact_succeeded(artifact: &ClaudeManagedArtifact) -> Result<()> {
    if let Some(detail) = managed_artifact_terminal_error(artifact) {
        bail!("{detail}");
    }
    Ok(())
}

fn resolve_prompt(args: &RunManagedArgs) -> Result<String> {
    match (&args.prompt, &args.prompt_file) {
        (Some(_), Some(_)) => bail!("pass either --prompt or --prompt-file, not both"),
        (None, None) => bail!("one of --prompt or --prompt-file is required"),
        (Some(prompt), None) => Ok(prompt.clone()),
        (None, Some(path)) => std::fs::read_to_string(path)
            .with_context(|| format!("failed to read prompt file '{}'", path.display())),
    }
}

fn managed_output_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": [
            "summary",
            "problemStatement",
            "changeType",
            "objectives",
            "successCriteria",
            "riskRationale"
        ],
        "properties": {
            "summary": { "type": "string" },
            "problemStatement": { "type": "string" },
            "changeType": {
                "type": "string",
                "enum": [
                    "bugfix",
                    "feature",
                    "test",
                    "refactor",
                    "performance",
                    "security",
                    "docs",
                    "chore",
                    "unknown"
                ]
            },
            "objectives": {
                "type": "array",
                "minItems": 1,
                "items": {
                    "oneOf": [
                        { "type": "string" },
                        {
                            "type": "object",
                            "additionalProperties": false,
                            "required": ["title", "kind"],
                            "properties": {
                                "title": { "type": "string" },
                                "kind": {
                                    "type": "string",
                                    "enum": ["implementation", "analysis"]
                                }
                            }
                        }
                    ]
                }
            },
            "outOfScope": {
                "type": "array",
                "items": { "type": "string" }
            },
            "planningSummary": {
                "type": "string"
            },
            "successCriteria": {
                "type": "array",
                "minItems": 1,
                "items": { "type": "string" }
            },
            "riskRationale": { "type": "string" }
        }
    })
}

fn default_managed_system_prompt() -> ManagedSystemPrompt {
    ManagedSystemPrompt {
        kind: "preset",
        preset: "claude_code",
        append: EMBEDDED_MANAGED_SYSTEM_PROMPT_APPEND.trim().to_string(),
    }
}

fn print_result(mode: &'static str, outcome: &PersistedManagedArtifactOutcome) -> Result<()> {
    let payload = ClaudeSdkCommandOutput {
        ok: true,
        command_mode: mode,
        provider_session_id: outcome.provider_session_id.clone(),
        ai_session_id: outcome.ai_session_id.clone(),
        ai_session_object_hash: outcome.ai_session_object_hash.clone(),
        already_persisted: outcome.already_persisted,
        intent_extraction_path: outcome.intent_extraction_path.clone(),
        raw_artifact_path: outcome.raw_artifact_path.clone(),
        audit_bundle_path: outcome.audit_bundle_path.clone(),
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&payload)
            .context("failed to serialize managed Claude SDK output")?
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::providers::claude_sdk::managed::ClaudeManagedResultMessage;

    fn base_chat_args() -> ChatManagedArgs {
        ChatManagedArgs {
            cwd: None,
            model: DEFAULT_MODEL.to_string(),
            permission_mode: "default".to_string(),
            timeout_seconds: DEFAULT_CHAT_TIMEOUT_SECONDS,
            tools: DEFAULT_CHAT_TOOLS.iter().map(ToString::to_string).collect(),
            prompt_suggestions: false,
            agent_progress_summaries: false,
            interactive_approvals: false,
            enable_file_checkpointing: true,
            continue_session: false,
            resume: None,
            fork_session: false,
            session_id: None,
            resume_session_at: None,
            helper_path: None,
            python_binary: DEFAULT_PYTHON_BINARY.to_string(),
        }
    }

    #[test]
    fn chat_validation_rejects_json_or_quiet_output() {
        let args = base_chat_args();
        let json_output = OutputConfig {
            json_format: Some(JsonFormat::Ndjson),
            ..OutputConfig::default()
        };
        let json_error = validate_chat_managed_args(&args, &json_output)
            .expect_err("chat should reject JSON output");
        assert!(
            json_error
                .to_string()
                .contains("claude-sdk chat is interactive and does not support")
        );

        let quiet_output = OutputConfig {
            quiet: true,
            ..OutputConfig::default()
        };
        let quiet_error = validate_chat_managed_args(&args, &quiet_output)
            .expect_err("chat should reject quiet output");
        assert!(
            quiet_error
                .to_string()
                .contains("claude-sdk chat is interactive and does not support")
        );
    }

    #[test]
    fn chat_followup_session_control_switches_to_explicit_resume() {
        let control = ManagedSessionControl::followup("provider-session-123".to_string());
        assert!(!control.continue_session);
        assert_eq!(control.resume.as_deref(), Some("provider-session-123"));
        assert!(!control.fork_session);
        assert!(control.session_id.is_none());
        assert!(control.resume_session_at.is_none());
    }

    #[test]
    fn streaming_helper_request_promotes_default_permission_mode_to_interactive() {
        let request = build_run_streaming_helper_request(ManagedStreamingTurnArgs {
            prompt: "hello".to_string(),
            cwd: PathBuf::from("/tmp/repo"),
            model: DEFAULT_MODEL.to_string(),
            permission_mode: "default".to_string(),
            timeout_seconds: None,
            tools: vec!["Read".to_string()],
            prompt_suggestions: false,
            agent_progress_summaries: false,
            interactive_approvals: false,
            enable_file_checkpointing: false,
            continue_session: false,
            resume: None,
            fork_session: false,
            session_id: None,
            resume_session_at: None,
        });

        assert_eq!(request.mode, "queryStream");
        assert!(request.interactive_approvals);
        assert_eq!(request.timeout_seconds, None);
        assert_eq!(request.idle_timeout_seconds, None);
        assert_eq!(request.allowed_tools, vec!["Read".to_string()]);
        assert!(!request.auto_approve_tools);
        assert!(request.system_prompt.is_some());
        assert!(request.output_schema.is_some());
    }

    #[test]
    fn chat_helper_request_omits_managed_structured_output_contract() {
        let request = build_chat_streaming_helper_request(ManagedStreamingTurnArgs {
            prompt: "hello".to_string(),
            cwd: PathBuf::from("/tmp/repo"),
            model: DEFAULT_MODEL.to_string(),
            permission_mode: "acceptEdits".to_string(),
            timeout_seconds: Some(DEFAULT_CHAT_TIMEOUT_SECONDS),
            tools: vec!["Read".to_string(), "Edit".to_string()],
            prompt_suggestions: false,
            agent_progress_summaries: false,
            interactive_approvals: false,
            enable_file_checkpointing: true,
            continue_session: false,
            resume: None,
            fork_session: false,
            session_id: None,
            resume_session_at: None,
        });

        assert_eq!(request.mode, "queryStream");
        assert_eq!(request.timeout_seconds, None);
        assert_eq!(
            request.idle_timeout_seconds,
            Some(DEFAULT_CHAT_TIMEOUT_SECONDS)
        );
        assert!(request.system_prompt.is_none());
        assert!(request.output_schema.is_none());
        assert!(request.include_partial_messages);
    }

    #[test]
    fn default_permission_mode_enables_interactive_approvals() {
        assert!(interactive_approvals_enabled("default", false));
        assert!(interactive_approvals_enabled("acceptEdits", true));
        assert!(!interactive_approvals_enabled("acceptEdits", false));
    }

    #[test]
    fn fullscreen_chat_disables_when_interactive_approvals_are_needed() {
        let mut args = base_chat_args();
        args.permission_mode = "acceptEdits".to_string();

        assert!(should_use_fullscreen_chat_tui(&args, true));

        args.permission_mode = "default".to_string();
        assert!(!should_use_fullscreen_chat_tui(&args, true));

        args.permission_mode = "acceptEdits".to_string();
        args.interactive_approvals = true;
        assert!(!should_use_fullscreen_chat_tui(&args, true));
        assert!(!should_use_fullscreen_chat_tui(&args, false));
    }

    #[test]
    fn chat_base_args_use_demo_friendly_defaults() {
        let args = base_chat_args();
        assert_eq!(args.timeout_seconds, DEFAULT_CHAT_TIMEOUT_SECONDS);
        assert_eq!(
            args.tools,
            DEFAULT_CHAT_TOOLS
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        );
        assert!(args.enable_file_checkpointing);
    }

    #[test]
    fn rendered_chat_rows_counts_wrapped_single_line_prompt() {
        // Prefix is 5 columns ("you> ").
        assert_eq!(rendered_chat_rows("abcd", 10), 1);
        assert_eq!(rendered_chat_rows("abcde", 10), 1);
        assert_eq!(rendered_chat_rows("abcdef", 10), 2);
    }

    #[test]
    fn rendered_chat_rows_counts_wrapped_multiline_prompt() {
        // First line: 5 + 5 = 10 => 2 rows at width 8.
        // Second line: 4 + 6 = 10 => 2 rows at width 8.
        assert_eq!(rendered_chat_rows("hello\nsecond", 8), 4);
    }

    #[test]
    fn rendered_chat_rows_respects_wide_characters() {
        // "你" and "好" are full-width characters (width 2 each).
        // 5 + 4 = 9 => 2 rows at width 8.
        assert_eq!(rendered_chat_rows("你好", 8), 2);
    }

    #[test]
    fn capture_assistant_text_collects_stream_deltas() {
        let mut sink = String::new();
        let delta_event = json!({
            "event": "assistant_delta",
            "delta": "hello"
        });
        capture_assistant_text_from_stream_event(&delta_event, &mut sink);
        let delta_event_2 = json!({
            "event": "assistant_delta",
            "delta": " world"
        });
        capture_assistant_text_from_stream_event(&delta_event_2, &mut sink);
        assert_eq!(sink, "hello world");
    }

    #[test]
    fn extract_latest_assistant_text_uses_last_assistant_message() {
        let artifact = ClaudeManagedArtifact {
            cwd: "/tmp/repo".to_string(),
            prompt: Some("test".to_string()),
            request_context: None,
            helper_timed_out: false,
            helper_error: None,
            hook_events: Vec::new(),
            messages: vec![
                json!({
                    "type": "assistant",
                    "message": {
                        "content": [
                            { "type": "text", "text": "first" }
                        ]
                    }
                }),
                json!({
                    "type": "assistant",
                    "message": {
                        "content": [
                            { "type": "text", "text": "latest" }
                        ]
                    }
                }),
            ],
            result_message: None,
        };

        assert_eq!(
            extract_latest_assistant_text(&artifact),
            Some("latest".to_string())
        );
    }

    #[test]
    fn managed_artifact_success_check_rejects_terminal_failures() {
        let timeout_error = ensure_managed_artifact_succeeded(&ClaudeManagedArtifact {
            cwd: "/tmp/repo".to_string(),
            prompt: Some("test".to_string()),
            request_context: None,
            helper_timed_out: true,
            helper_error: None,
            hook_events: Vec::new(),
            messages: Vec::new(),
            result_message: None,
        })
        .expect_err("timed out artifacts should fail");
        assert!(timeout_error.to_string().contains("timed out"));

        let helper_error = ensure_managed_artifact_succeeded(&ClaudeManagedArtifact {
            cwd: "/tmp/repo".to_string(),
            prompt: Some("test".to_string()),
            request_context: None,
            helper_timed_out: false,
            helper_error: Some("authentication_failed".to_string()),
            hook_events: Vec::new(),
            messages: Vec::new(),
            result_message: None,
        })
        .expect_err("helper errors should fail");
        assert!(helper_error.to_string().contains("authentication_failed"));

        let result_error = ensure_managed_artifact_succeeded(&ClaudeManagedArtifact {
            cwd: "/tmp/repo".to_string(),
            prompt: Some("test".to_string()),
            request_context: None,
            helper_timed_out: false,
            helper_error: None,
            hook_events: Vec::new(),
            messages: Vec::new(),
            result_message: Some(ClaudeManagedResultMessage {
                r#type: Some("result".to_string()),
                subtype: Some("error".to_string()),
                is_error: Some(true),
                session_id: Some("session-1".to_string()),
                stop_reason: None,
                duration_ms: None,
                duration_api_ms: None,
                num_turns: None,
                result: Some("authentication_failed".to_string()),
                total_cost_usd: None,
                usage: None,
                model_usage: None,
                permission_denials: None,
                structured_output: None,
                fast_mode_state: None,
                uuid: None,
            }),
        })
        .expect_err("result errors should fail");
        assert!(
            result_error
                .to_string()
                .contains("Claude Code returned an error result: authentication_failed")
        );
    }
}
