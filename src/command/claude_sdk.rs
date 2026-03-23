//! Claude Agent SDK managed-mode command surface.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
};

use anyhow::{Context, Result, anyhow, bail};
use chrono::Utc;
use clap::{Args, Parser, Subcommand};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::{fs, io::AsyncWriteExt, process::Command};

use crate::{
    internal::{
        ai::{
            history::HistoryManager,
            intentspec::{
                IntentDraft, ResolveContext, RiskLevel, persist_intentspec, render_summary,
                repair_intentspec, resolve_intentspec, validate_intentspec,
            },
            mcp::{
                resource::{
                    CreateDecisionParams, CreateEvidenceParams, CreateRunParams, CreateTaskParams,
                },
                server::LibraMcpServer,
            },
            providers::claude_sdk::managed::{
                ClaudeManagedArtifact, ManagedAuditBundle, ManagedRunUsageEvent,
                ManagedSemanticRuntimeEvent, PersistedManagedArtifactOutcome,
                persist_managed_artifact,
            },
        },
        db,
        head::Head,
    },
    utils::{object::write_git_object, storage::local::LocalStorage, util},
};

const DEFAULT_MODEL: &str = "claude-sonnet-4-5-20250929";
const INTENT_EXTRACTIONS_DIR: &str = "intent-extractions";
const INTENT_RESOLUTIONS_DIR: &str = "intent-resolutions";
const INTENT_INPUTS_DIR: &str = "intent-inputs";
const PROVIDER_SESSIONS_DIR: &str = "provider-sessions";
const EVIDENCE_INPUTS_DIR: &str = "evidence-inputs";
const FORMAL_RUN_BINDINGS_DIR: &str = "claude-run-bindings";
const EVIDENCE_BINDINGS_DIR: &str = "claude-evidence-bindings";
const DECISION_BINDINGS_DIR: &str = "claude-decision-bindings";
const ZERO_COMMIT_SHA: &str = "0000000000000000000000000000000000000000";
const EMBEDDED_HELPER_SOURCE: &str = include_str!("../internal/ai/providers/claude_sdk/helper.cjs");

#[derive(Parser, Debug)]
pub struct ClaudeSdkArgs {
    #[command(subcommand)]
    command: ClaudeSdkSubcommand,
}

#[derive(Subcommand, Debug)]
enum ClaudeSdkSubcommand {
    #[command(
        about = "Import a raw Claude SDK managed artifact and persist Libra bridge artifacts"
    )]
    Import(ImportArtifactArgs),
    #[command(about = "Run a Claude Agent SDK managed session through the bundled Node helper")]
    Run(RunManagedArgs),
    #[command(
        name = "sync-sessions",
        about = "Sync Claude SDK provider session metadata into Libra-managed provider session snapshots"
    )]
    SyncSessions(SyncSessionsArgs),
    #[command(
        name = "hydrate-session",
        about = "Fetch Claude SDK session messages and update a Libra-managed provider session snapshot"
    )]
    HydrateSession(HydrateSessionArgs),
    #[command(
        name = "build-evidence-input",
        about = "Build an EvidenceInput-style artifact from a hydrated Claude provider session"
    )]
    BuildEvidenceInput(BuildEvidenceInputArgs),
    #[command(
        name = "resolve-extraction",
        about = "Resolve a persisted intent extraction artifact into a validated IntentSpec preview artifact"
    )]
    ResolveExtraction(ResolveExtractionArgs),
    #[command(
        name = "persist-intent",
        about = "Persist a resolved IntentSpec preview into Libra intent history"
    )]
    PersistIntent(PersistIntentArgs),
    #[command(
        name = "bridge-run",
        about = "Create or reuse formal Task/Run objects for a Claude SDK ai_session"
    )]
    BridgeRun(BridgeRunArgs),
    #[command(
        name = "persist-evidence",
        about = "Persist formal Evidence objects for a bridged Claude SDK ai_session"
    )]
    PersistEvidence(PersistEvidenceArgs),
    #[command(
        name = "persist-decision",
        about = "Persist a formal terminal Decision for a bridged Claude SDK ai_session"
    )]
    PersistDecision(PersistDecisionArgs),
}

#[derive(Args, Debug)]
struct ImportArtifactArgs {
    #[arg(long, help = "Path to a raw Claude managed artifact JSON file")]
    artifact: PathBuf,
}

#[derive(Args, Debug)]
struct RunManagedArgs {
    #[arg(long, help = "Prompt text for the managed Claude SDK session")]
    prompt: Option<String>,
    #[arg(long, help = "Read the prompt text from a UTF-8 file")]
    prompt_file: Option<PathBuf>,
    #[arg(long, help = "Working directory for the Claude SDK session")]
    cwd: Option<PathBuf>,
    #[arg(long, default_value = DEFAULT_MODEL, help = "Claude model identifier")]
    model: String,
    #[arg(
        long,
        default_value = "default",
        help = "Claude SDK permission mode passed to query()"
    )]
    permission_mode: String,
    #[arg(
        long,
        help = "Optional helper timeout in seconds; when reached, Libra persists a partial managed artifact if available"
    )]
    timeout_seconds: Option<u64>,
    #[arg(
        long = "tool",
        help = "Tool name to enable and allow for the managed Claude SDK session"
    )]
    tools: Vec<String>,
    #[arg(
        long,
        default_value_t = true,
        action = clap::ArgAction::Set,
        help = "Whether the helper should auto-approve requested tools; set to false for live permission/decision probing"
    )]
    auto_approve_tools: bool,
    #[arg(
        long,
        default_value_t = false,
        action = clap::ArgAction::Set,
        help = "Whether the helper should request SDKPartialAssistantMessage stream_event messages"
    )]
    include_partial_messages: bool,
    #[arg(
        long,
        default_value_t = false,
        action = clap::ArgAction::Set,
        help = "Whether the helper should request prompt_suggestion messages after result events"
    )]
    prompt_suggestions: bool,
    #[arg(
        long,
        default_value_t = false,
        action = clap::ArgAction::Set,
        help = "Whether the helper should request agent-generated task_progress summaries for subagents"
    )]
    agent_progress_summaries: bool,
    #[arg(
        long,
        help = "Optional path to a custom helper script; defaults to the embedded helper"
    )]
    helper_path: Option<PathBuf>,
    #[arg(
        long,
        default_value = "node",
        help = "Node.js executable used to run the helper"
    )]
    node_binary: String,
}

#[derive(Args, Debug)]
struct SyncSessionsArgs {
    #[arg(long, help = "Working directory used as the Claude SDK project dir")]
    cwd: Option<PathBuf>,
    #[arg(
        long,
        help = "Optional provider session id to sync; defaults to all sessions in the project"
    )]
    provider_session_id: Option<String>,
    #[arg(long, help = "Maximum number of sessions to request from Claude SDK")]
    limit: Option<usize>,
    #[arg(
        long,
        default_value_t = 0,
        help = "Number of sessions to skip before syncing"
    )]
    offset: usize,
    #[arg(
        long,
        default_value_t = true,
        action = clap::ArgAction::Set,
        help = "Whether Claude SDK should include sessions from git worktrees when cwd is inside a repo"
    )]
    include_worktrees: bool,
    #[arg(
        long,
        help = "Optional path to a custom helper script; defaults to the embedded helper"
    )]
    helper_path: Option<PathBuf>,
    #[arg(
        long,
        default_value = "node",
        help = "Node.js executable used to run the helper"
    )]
    node_binary: String,
}

#[derive(Args, Debug)]
struct HydrateSessionArgs {
    #[arg(long, help = "Working directory used as the Claude SDK project dir")]
    cwd: Option<PathBuf>,
    #[arg(long, help = "Provider session id to hydrate from Claude SDK")]
    provider_session_id: String,
    #[arg(long, help = "Maximum number of session messages to request")]
    limit: Option<usize>,
    #[arg(
        long,
        default_value_t = 0,
        help = "Number of session messages to skip before hydrating"
    )]
    offset: usize,
    #[arg(
        long,
        help = "Optional path to a custom helper script; defaults to the embedded helper"
    )]
    helper_path: Option<PathBuf>,
    #[arg(
        long,
        default_value = "node",
        help = "Node.js executable used to run the helper"
    )]
    node_binary: String,
}

#[derive(Args, Debug)]
struct BuildEvidenceInputArgs {
    #[arg(long, help = "Provider session id to derive an evidence input from")]
    provider_session_id: String,
    #[arg(
        long,
        help = "Optional output path for the evidence input artifact; defaults to .libra/evidence-inputs/<object-id>.json"
    )]
    output: Option<PathBuf>,
}

#[derive(Args, Debug)]
struct ResolveExtractionArgs {
    #[arg(long, help = "Path to a persisted intent extraction JSON file")]
    extraction: Option<PathBuf>,
    #[arg(
        long,
        help = "Resolve the extraction stored at .libra/intent-extractions/<ai-session-id>.json"
    )]
    ai_session_id: Option<String>,
    #[arg(
        long,
        help = "Override risk level (low|medium|high); defaults to extraction risk level or medium"
    )]
    risk_level: Option<String>,
    #[arg(
        long,
        default_value = "claude-sdk",
        help = "createdBy.id used in the resolved IntentSpec preview"
    )]
    created_by_id: String,
    #[arg(
        long,
        help = "Optional output path for the resolved artifact; defaults to .libra/intent-resolutions/<ai-session-id>.json"
    )]
    output: Option<PathBuf>,
}

#[derive(Args, Debug)]
struct PersistIntentArgs {
    #[arg(long, help = "Path to a resolved intent preview JSON file")]
    resolution: Option<PathBuf>,
    #[arg(
        long,
        help = "Persist the resolution stored at .libra/intent-resolutions/<ai-session-id>.json"
    )]
    ai_session_id: Option<String>,
    #[arg(
        long,
        help = "Optional output path for the persisted-intent binding artifact; defaults to .libra/intent-inputs/<ai-session-id>.json"
    )]
    output: Option<PathBuf>,
}

#[derive(Args, Debug)]
struct BridgeRunArgs {
    #[arg(
        long,
        help = "Claude SDK ai_session_id to bridge into formal Task/Run objects"
    )]
    ai_session_id: String,
    #[arg(
        long,
        help = "Optional persisted intent binding artifact path; defaults to .libra/intent-inputs/<ai-session-id>.json when present"
    )]
    intent_binding: Option<PathBuf>,
    #[arg(
        long,
        help = "Optional intent UUID override; when set, skip intent binding artifact lookup"
    )]
    intent_id: Option<String>,
}

#[derive(Args, Debug)]
struct PersistEvidenceArgs {
    #[arg(
        long,
        help = "Claude SDK ai_session_id whose formal run should receive Evidence"
    )]
    ai_session_id: String,
}

#[derive(Args, Debug)]
struct PersistDecisionArgs {
    #[arg(
        long,
        help = "Claude SDK ai_session_id whose formal run should receive a terminal Decision"
    )]
    ai_session_id: String,
}

#[derive(Debug, Serialize)]
struct ClaudeSdkCommandOutput {
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
struct ResolveExtractionCommandOutput {
    ok: bool,
    #[serde(rename = "mode")]
    command_mode: &'static str,
    #[serde(rename = "aiSessionId", skip_serializing_if = "Option::is_none")]
    ai_session_id: Option<String>,
    #[serde(rename = "extractionPath")]
    extraction_path: String,
    #[serde(rename = "resolvedSpecPath")]
    resolved_spec_path: String,
    #[serde(rename = "riskLevel")]
    risk_level: String,
    summary: String,
}

#[derive(Debug, Serialize)]
struct PersistIntentCommandOutput {
    ok: bool,
    #[serde(rename = "mode")]
    command_mode: &'static str,
    #[serde(rename = "aiSessionId", skip_serializing_if = "Option::is_none")]
    ai_session_id: Option<String>,
    #[serde(rename = "resolutionPath")]
    resolution_path: String,
    #[serde(rename = "intentId")]
    intent_id: String,
    #[serde(rename = "bindingPath")]
    binding_path: String,
    summary: String,
}

#[derive(Debug, Serialize)]
struct BridgeRunCommandOutput {
    ok: bool,
    #[serde(rename = "mode")]
    command_mode: &'static str,
    #[serde(rename = "aiSessionId")]
    ai_session_id: String,
    #[serde(rename = "providerSessionId")]
    provider_session_id: String,
    #[serde(rename = "taskId")]
    task_id: String,
    #[serde(rename = "runId")]
    run_id: String,
    #[serde(rename = "bindingPath")]
    binding_path: String,
    #[serde(rename = "intentId", skip_serializing_if = "Option::is_none")]
    intent_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct PersistEvidenceCommandOutput {
    ok: bool,
    #[serde(rename = "mode")]
    command_mode: &'static str,
    #[serde(rename = "aiSessionId")]
    ai_session_id: String,
    #[serde(rename = "runId")]
    run_id: String,
    #[serde(rename = "evidenceIds")]
    evidence_ids: Vec<String>,
    #[serde(rename = "bindingPath")]
    binding_path: String,
}

#[derive(Debug, Serialize)]
struct PersistDecisionCommandOutput {
    ok: bool,
    #[serde(rename = "mode")]
    command_mode: &'static str,
    #[serde(rename = "aiSessionId")]
    ai_session_id: String,
    #[serde(rename = "runId")]
    run_id: String,
    #[serde(rename = "decisionId")]
    decision_id: String,
    #[serde(rename = "decisionType")]
    decision_type: String,
    #[serde(rename = "bindingPath")]
    binding_path: String,
}

#[derive(Debug, Serialize)]
struct HydrateSessionCommandOutput {
    ok: bool,
    #[serde(rename = "mode")]
    command_mode: &'static str,
    #[serde(rename = "providerSessionId")]
    provider_session_id: String,
    #[serde(rename = "objectId")]
    object_id: String,
    #[serde(rename = "artifactPath")]
    artifact_path: String,
    #[serde(rename = "messagesArtifactPath")]
    messages_artifact_path: String,
    #[serde(rename = "messageCount")]
    message_count: usize,
}

#[derive(Debug, Serialize)]
struct BuildEvidenceInputCommandOutput {
    ok: bool,
    #[serde(rename = "mode")]
    command_mode: &'static str,
    #[serde(rename = "providerSessionId")]
    provider_session_id: String,
    #[serde(rename = "providerSessionObjectId")]
    provider_session_object_id: String,
    #[serde(rename = "objectId")]
    object_id: String,
    #[serde(rename = "artifactPath")]
    artifact_path: String,
    #[serde(rename = "objectHash")]
    object_hash: String,
    #[serde(rename = "messageCount")]
    message_count: usize,
}

#[derive(Debug, Serialize)]
struct ManagedHelperRequest {
    mode: &'static str,
    prompt: String,
    cwd: String,
    model: String,
    #[serde(rename = "permissionMode")]
    permission_mode: String,
    #[serde(rename = "timeoutSeconds", skip_serializing_if = "Option::is_none")]
    timeout_seconds: Option<u64>,
    tools: Vec<String>,
    #[serde(rename = "allowedTools")]
    allowed_tools: Vec<String>,
    #[serde(rename = "autoApproveTools")]
    auto_approve_tools: bool,
    #[serde(rename = "includePartialMessages")]
    include_partial_messages: bool,
    #[serde(rename = "promptSuggestions")]
    prompt_suggestions: bool,
    #[serde(rename = "agentProgressSummaries")]
    agent_progress_summaries: bool,
    #[serde(rename = "outputSchema")]
    output_schema: Value,
}

#[derive(Debug, Serialize)]
struct SessionCatalogHelperRequest {
    mode: &'static str,
    cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    limit: Option<usize>,
    offset: usize,
    #[serde(rename = "includeWorktrees")]
    include_worktrees: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionMessagesHelperRequest {
    mode: &'static str,
    cwd: String,
    #[serde(rename = "providerSessionId")]
    provider_session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    limit: Option<usize>,
    offset: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ClaudeSdkSessionInfo {
    #[serde(rename = "sessionId")]
    session_id: String,
    summary: String,
    #[serde(rename = "lastModified")]
    last_modified: i64,
    #[serde(rename = "fileSize", default)]
    file_size: Option<u64>,
    #[serde(rename = "customTitle", default)]
    custom_title: Option<String>,
    #[serde(rename = "firstPrompt", default)]
    first_prompt: Option<String>,
    #[serde(rename = "gitBranch", default)]
    git_branch: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    tag: Option<String>,
    #[serde(rename = "createdAt", default)]
    created_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedProviderSessionSnapshot {
    schema: String,
    object_type: String,
    provider: String,
    #[serde(rename = "providerSessionId")]
    provider_session_id: String,
    #[serde(rename = "objectId")]
    object_id: String,
    summary: String,
    #[serde(
        rename = "customTitle",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    custom_title: Option<String>,
    #[serde(
        rename = "firstPrompt",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    first_prompt: Option<String>,
    #[serde(rename = "gitBranch", default, skip_serializing_if = "Option::is_none")]
    git_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tag: Option<String>,
    #[serde(rename = "createdAt", default, skip_serializing_if = "Option::is_none")]
    created_at: Option<i64>,
    #[serde(rename = "lastModified")]
    last_modified: i64,
    #[serde(rename = "fileSize", default, skip_serializing_if = "Option::is_none")]
    file_size: Option<u64>,
    #[serde(rename = "capturedAt")]
    captured_at: String,
    #[serde(
        rename = "messageSync",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    message_sync: Option<ProviderSessionMessageSync>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ProviderSessionMessageSync {
    #[serde(rename = "artifactPath")]
    artifact_path: String,
    #[serde(rename = "messageCount")]
    message_count: usize,
    #[serde(rename = "kindCounts")]
    kind_counts: BTreeMap<String, usize>,
    #[serde(
        rename = "firstMessageKind",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    first_message_kind: Option<String>,
    #[serde(
        rename = "lastMessageKind",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    last_message_kind: Option<String>,
    offset: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    limit: Option<usize>,
    #[serde(rename = "capturedAt")]
    captured_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ProviderSessionMessagesArtifact {
    schema: String,
    #[serde(rename = "providerSessionId")]
    provider_session_id: String,
    #[serde(rename = "objectId")]
    object_id: String,
    offset: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    limit: Option<usize>,
    #[serde(rename = "capturedAt")]
    captured_at: String,
    messages: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedEvidenceInputArtifact {
    schema: String,
    object_type: String,
    provider: String,
    #[serde(rename = "objectId")]
    object_id: String,
    #[serde(rename = "providerSessionId")]
    provider_session_id: String,
    #[serde(rename = "providerSessionObjectId")]
    provider_session_object_id: String,
    summary: String,
    #[serde(rename = "sourceArtifacts")]
    source_artifacts: EvidenceInputSourceArtifacts,
    #[serde(rename = "messageOverview")]
    message_overview: EvidenceInputMessageOverview,
    #[serde(rename = "contentOverview")]
    content_overview: EvidenceInputContentOverview,
    #[serde(rename = "runtimeSignals")]
    runtime_signals: EvidenceInputRuntimeSignals,
    #[serde(rename = "latestResult", skip_serializing_if = "Option::is_none")]
    latest_result: Option<EvidenceInputLatestResult>,
    #[serde(rename = "capturedAt")]
    captured_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct EvidenceInputSourceArtifacts {
    #[serde(rename = "providerSessionPath")]
    provider_session_path: String,
    #[serde(rename = "messagesPath")]
    messages_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct EvidenceInputMessageOverview {
    #[serde(rename = "messageCount")]
    message_count: usize,
    #[serde(rename = "kindCounts")]
    kind_counts: BTreeMap<String, usize>,
    #[serde(rename = "firstMessageKind", skip_serializing_if = "Option::is_none")]
    first_message_kind: Option<String>,
    #[serde(rename = "lastMessageKind", skip_serializing_if = "Option::is_none")]
    last_message_kind: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct EvidenceInputContentOverview {
    #[serde(rename = "assistantMessageCount")]
    assistant_message_count: usize,
    #[serde(rename = "userMessageCount")]
    user_message_count: usize,
    #[serde(rename = "observedTools")]
    observed_tools: BTreeMap<String, usize>,
    #[serde(rename = "observedPaths")]
    observed_paths: Vec<String>,
    #[serde(rename = "assistantTextPreviews")]
    assistant_text_previews: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct EvidenceInputRuntimeSignals {
    #[serde(rename = "resultMessageCount")]
    result_message_count: usize,
    #[serde(rename = "toolRuntimeCount")]
    tool_runtime_count: usize,
    #[serde(rename = "taskRuntimeCount")]
    task_runtime_count: usize,
    #[serde(rename = "partialAssistantEventCount")]
    partial_assistant_event_count: usize,
    #[serde(rename = "hasStructuredOutput")]
    has_structured_output: bool,
    #[serde(rename = "hasPermissionDenials")]
    has_permission_denials: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct EvidenceInputLatestResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    subtype: Option<String>,
    #[serde(rename = "stopReason", skip_serializing_if = "Option::is_none")]
    stop_reason: Option<String>,
    #[serde(rename = "durationMs", skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u64>,
    #[serde(rename = "durationApiMs", skip_serializing_if = "Option::is_none")]
    duration_api_ms: Option<u64>,
    #[serde(rename = "totalCostUsd", skip_serializing_if = "Option::is_none")]
    total_cost_usd: Option<f64>,
    #[serde(rename = "numTurns", skip_serializing_if = "Option::is_none")]
    num_turns: Option<u64>,
    #[serde(rename = "permissionDenialCount")]
    permission_denial_count: usize,
}

#[derive(Debug, Serialize)]
struct SyncSessionsCommandOutput {
    ok: bool,
    #[serde(rename = "mode")]
    command_mode: &'static str,
    #[serde(rename = "syncedCount")]
    synced_count: usize,
    sessions: Vec<SyncSessionRecord>,
}

#[derive(Debug, Serialize)]
struct SyncSessionRecord {
    #[serde(rename = "providerSessionId")]
    provider_session_id: String,
    #[serde(rename = "objectId")]
    object_id: String,
    #[serde(rename = "artifactPath")]
    artifact_path: String,
    #[serde(rename = "objectHash")]
    object_hash: String,
}

trait HelperResponse {
    type Output;

    fn parse_response(stdout: &str, stderr: &str) -> Result<Self::Output>;
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

impl HelperResponse for SessionCatalogHelperRequest {
    type Output = Vec<ClaudeSdkSessionInfo>;

    fn parse_response(stdout: &str, stderr: &str) -> Result<Self::Output> {
        serde_json::from_str(stdout.trim()).with_context(|| {
            format!(
                "failed to parse Claude SDK helper output as a session catalog response (stderr: {})",
                stderr.trim()
            )
        })
    }
}

impl HelperResponse for SessionMessagesHelperRequest {
    type Output = Vec<Value>;

    fn parse_response(stdout: &str, stderr: &str) -> Result<Self::Output> {
        serde_json::from_str(stdout.trim()).with_context(|| {
            format!(
                "failed to parse Claude SDK helper output as a session messages response (stderr: {})",
                stderr.trim()
            )
        })
    }
}

#[derive(Debug, Deserialize)]
struct PersistedIntentExtractionArtifact {
    schema: String,
    #[serde(rename = "ai_session_id")]
    ai_session_id: String,
    source: String,
    extraction: IntentDraft,
}

#[derive(Debug, Serialize)]
struct ResolvedIntentSpecArtifact {
    schema: &'static str,
    #[serde(rename = "aiSessionId", skip_serializing_if = "Option::is_none")]
    ai_session_id: Option<String>,
    #[serde(rename = "extractionPath")]
    extraction_path: String,
    #[serde(rename = "extractionSource")]
    extraction_source: String,
    #[serde(rename = "riskLevel")]
    risk_level: RiskLevel,
    summary: String,
    intentspec: crate::internal::ai::intentspec::IntentSpec,
}

#[derive(Debug, Deserialize)]
struct PersistedIntentResolutionArtifact {
    schema: String,
    #[serde(rename = "aiSessionId", default)]
    ai_session_id: Option<String>,
    #[serde(rename = "extractionPath")]
    extraction_path: String,
    #[serde(rename = "extractionSource")]
    extraction_source: String,
    #[serde(rename = "riskLevel")]
    risk_level: RiskLevel,
    summary: String,
    intentspec: crate::internal::ai::intentspec::IntentSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedIntentInputBindingArtifact {
    schema: String,
    #[serde(rename = "aiSessionId", skip_serializing_if = "Option::is_none")]
    ai_session_id: Option<String>,
    #[serde(rename = "resolutionPath")]
    resolution_path: String,
    #[serde(rename = "extractionPath")]
    extraction_path: String,
    #[serde(rename = "extractionSource")]
    extraction_source: String,
    #[serde(rename = "riskLevel")]
    risk_level: RiskLevel,
    #[serde(rename = "intentId")]
    intent_id: String,
    summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ClaudeFormalRunBindingArtifact {
    schema: String,
    #[serde(rename = "aiSessionId")]
    ai_session_id: String,
    #[serde(rename = "providerSessionId")]
    provider_session_id: String,
    #[serde(rename = "taskId")]
    task_id: String,
    #[serde(rename = "runId")]
    run_id: String,
    #[serde(rename = "auditBundlePath")]
    audit_bundle_path: String,
    #[serde(rename = "intentBindingPath", skip_serializing_if = "Option::is_none")]
    intent_binding_path: Option<String>,
    #[serde(rename = "intentId", skip_serializing_if = "Option::is_none")]
    intent_id: Option<String>,
    #[serde(rename = "managedRunStatus")]
    managed_run_status: String,
    #[serde(rename = "intentExtractionStatus")]
    intent_extraction_status: String,
    summary: String,
    #[serde(rename = "createdAt")]
    created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ClaudeEvidenceBindingArtifact {
    schema: String,
    #[serde(rename = "aiSessionId")]
    ai_session_id: String,
    #[serde(rename = "providerSessionId")]
    provider_session_id: String,
    #[serde(rename = "runId")]
    run_id: String,
    #[serde(rename = "runBindingPath")]
    run_binding_path: String,
    #[serde(rename = "evidenceIds")]
    evidence_ids: Vec<String>,
    evidences: Vec<ClaudeEvidenceBindingEntry>,
    #[serde(rename = "createdAt")]
    created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ClaudeEvidenceBindingEntry {
    kind: String,
    #[serde(rename = "evidenceId")]
    evidence_id: String,
    #[serde(rename = "sourcePath")]
    source_path: String,
    summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ClaudeDecisionBindingArtifact {
    schema: String,
    #[serde(rename = "aiSessionId")]
    ai_session_id: String,
    #[serde(rename = "providerSessionId")]
    provider_session_id: String,
    #[serde(rename = "runId")]
    run_id: String,
    #[serde(rename = "decisionId")]
    decision_id: String,
    #[serde(rename = "decisionType")]
    decision_type: String,
    rationale: String,
    #[serde(rename = "runBindingPath")]
    run_binding_path: String,
    #[serde(rename = "evidenceBindingPath")]
    evidence_binding_path: String,
    #[serde(rename = "evidenceIds", default, skip_serializing_if = "Vec::is_empty")]
    evidence_ids: Vec<String>,
    #[serde(rename = "createdAt")]
    created_at: String,
}

#[derive(Debug)]
struct EmbeddedHelperDir {
    _temp_dir: TempDir,
}

pub async fn execute(args: ClaudeSdkArgs) -> Result<()> {
    match args.command {
        ClaudeSdkSubcommand::Import(args) => import_artifact(args).await,
        ClaudeSdkSubcommand::Run(args) => run_managed(args).await,
        ClaudeSdkSubcommand::SyncSessions(args) => sync_sessions(args).await,
        ClaudeSdkSubcommand::HydrateSession(args) => hydrate_session(args).await,
        ClaudeSdkSubcommand::BuildEvidenceInput(args) => build_evidence_input(args).await,
        ClaudeSdkSubcommand::ResolveExtraction(args) => resolve_extraction(args).await,
        ClaudeSdkSubcommand::PersistIntent(args) => persist_intent(args).await,
        ClaudeSdkSubcommand::BridgeRun(args) => bridge_run(args).await,
        ClaudeSdkSubcommand::PersistEvidence(args) => persist_evidence(args).await,
        ClaudeSdkSubcommand::PersistDecision(args) => persist_decision(args).await,
    }
}

async fn import_artifact(args: ImportArtifactArgs) -> Result<()> {
    let storage_path = util::try_get_storage_path(None)
        .context("claude-sdk commands must be run inside a Libra repository")?;
    let artifact = read_artifact(&args.artifact).await?;
    let outcome = persist_managed_artifact(&storage_path, &artifact).await?;
    print_result("import", &outcome)?;
    Ok(())
}

async fn run_managed(args: RunManagedArgs) -> Result<()> {
    let storage_path = util::try_get_storage_path(None)
        .context("claude-sdk commands must be run inside a Libra repository")?;
    let prompt = resolve_prompt(&args)?;
    let cwd = args
        .cwd
        .unwrap_or(std::env::current_dir().context("failed to read current directory")?);
    let helper_request = ManagedHelperRequest {
        mode: "query",
        prompt,
        cwd: cwd.to_string_lossy().to_string(),
        model: args.model,
        permission_mode: args.permission_mode,
        timeout_seconds: args.timeout_seconds,
        tools: args.tools.clone(),
        allowed_tools: args.tools.clone(),
        auto_approve_tools: args.auto_approve_tools && !args.tools.is_empty(),
        include_partial_messages: args.include_partial_messages,
        prompt_suggestions: args.prompt_suggestions,
        agent_progress_summaries: args.agent_progress_summaries,
        output_schema: managed_output_schema(),
    };

    let (_temp_helper_dir, helper_path) = materialize_helper(args.helper_path.as_deref()).await?;
    let artifact = invoke_helper(&args.node_binary, &helper_path, &helper_request).await?;
    let outcome = persist_managed_artifact(&storage_path, &artifact).await?;
    print_result("run", &outcome)?;
    Ok(())
}

async fn sync_sessions(args: SyncSessionsArgs) -> Result<()> {
    let storage_path = util::try_get_storage_path(None)
        .context("claude-sdk commands must be run inside a Libra repository")?;
    let cwd = args
        .cwd
        .unwrap_or(std::env::current_dir().context("failed to read current directory")?);
    let helper_request = SessionCatalogHelperRequest {
        mode: "listSessions",
        cwd: cwd.to_string_lossy().to_string(),
        limit: args.limit,
        offset: args.offset,
        include_worktrees: args.include_worktrees,
    };

    let (_temp_helper_dir, helper_path) = materialize_helper(args.helper_path.as_deref()).await?;
    let sessions: Vec<ClaudeSdkSessionInfo> =
        invoke_helper_json(&args.node_binary, &helper_path, &helper_request)
            .await
            .context("failed to fetch Claude SDK session catalog")?;

    let filtered_sessions = if let Some(session_id) = args.provider_session_id.as_deref() {
        sessions
            .into_iter()
            .filter(|session| session.session_id == session_id)
            .collect::<Vec<_>>()
    } else {
        sessions
    };

    let mut synced = Vec::new();
    for session in filtered_sessions {
        synced.push(persist_provider_session_snapshot(&storage_path, session).await?);
    }

    println!(
        "{}",
        serde_json::to_string_pretty(&SyncSessionsCommandOutput {
            ok: true,
            command_mode: "sync-sessions",
            synced_count: synced.len(),
            sessions: synced,
        })
        .context("failed to serialize sync-sessions output")?
    );

    Ok(())
}

async fn hydrate_session(args: HydrateSessionArgs) -> Result<()> {
    let storage_path = util::try_get_storage_path(None)
        .context("claude-sdk commands must be run inside a Libra repository")?;
    let cwd = args
        .cwd
        .unwrap_or(std::env::current_dir().context("failed to read current directory")?);
    let object_id = build_provider_session_object_id(&args.provider_session_id)?;
    let artifact_path = provider_session_artifact_path(&storage_path, &object_id);
    let mut snapshot = read_persisted_provider_session_snapshot(&artifact_path)
        .await
        .with_context(|| {
            format!(
                "failed to read provider session snapshot '{}'; run `claude-sdk sync-sessions --provider-session-id {}` first",
                artifact_path.display(),
                args.provider_session_id
            )
        })?;

    let helper_request = SessionMessagesHelperRequest {
        mode: "getSessionMessages",
        cwd: cwd.to_string_lossy().to_string(),
        provider_session_id: args.provider_session_id.clone(),
        limit: args.limit,
        offset: args.offset,
    };
    let (_temp_helper_dir, helper_path) = materialize_helper(args.helper_path.as_deref()).await?;
    let messages: Vec<Value> = invoke_helper_json(&args.node_binary, &helper_path, &helper_request)
        .await
        .context("failed to fetch Claude SDK session messages")?;

    let captured_at = Utc::now().to_rfc3339();
    let messages_artifact_path = provider_session_messages_artifact_path(&storage_path, &object_id);
    let messages_artifact = ProviderSessionMessagesArtifact {
        schema: "libra.provider_session_messages.v1".to_string(),
        provider_session_id: args.provider_session_id.clone(),
        object_id: object_id.clone(),
        offset: args.offset,
        limit: args.limit,
        captured_at: captured_at.clone(),
        messages,
    };
    write_pretty_json_file(&messages_artifact_path, &messages_artifact)
        .await
        .with_context(|| {
            format!(
                "failed to write provider session messages artifact '{}'",
                messages_artifact_path.display()
            )
        })?;

    snapshot.captured_at = captured_at.clone();
    snapshot.message_sync = Some(build_provider_session_message_sync(
        &messages_artifact_path,
        &messages_artifact.messages,
        args.offset,
        args.limit,
        captured_at,
    ));
    let sync_record = upsert_provider_session_snapshot(&storage_path, &snapshot).await?;

    println!(
        "{}",
        serde_json::to_string_pretty(&HydrateSessionCommandOutput {
            ok: true,
            command_mode: "hydrate-session",
            provider_session_id: snapshot.provider_session_id,
            object_id: snapshot.object_id,
            artifact_path: sync_record.artifact_path,
            messages_artifact_path: messages_artifact_path.to_string_lossy().to_string(),
            message_count: messages_artifact.messages.len(),
        })
        .context("failed to serialize hydrate-session output")?
    );

    Ok(())
}

async fn build_evidence_input(args: BuildEvidenceInputArgs) -> Result<()> {
    let storage_path = util::try_get_storage_path(None)
        .context("claude-sdk commands must be run inside a Libra repository")?;
    let provider_session_object_id = build_provider_session_object_id(&args.provider_session_id)?;
    let provider_session_path =
        provider_session_artifact_path(&storage_path, &provider_session_object_id);
    let snapshot = read_persisted_provider_session_snapshot(&provider_session_path)
        .await
        .with_context(|| {
            format!(
                "failed to read provider session snapshot '{}'; run `claude-sdk sync-sessions --provider-session-id {}` first",
                provider_session_path.display(),
                args.provider_session_id
            )
        })?;
    let message_sync = snapshot.message_sync.as_ref().ok_or_else(|| {
        anyhow!(
            "provider session '{}' has no hydrated messages; run `claude-sdk hydrate-session --provider-session-id {}` first",
            snapshot.object_id,
            args.provider_session_id
        )
    })?;
    let messages_path = PathBuf::from(&message_sync.artifact_path);
    let messages_artifact = read_provider_session_messages_artifact(&messages_path)
        .await
        .with_context(|| {
            format!(
                "failed to read provider session messages artifact '{}'",
                messages_path.display()
            )
        })?;

    let object_id = build_evidence_input_object_id(&args.provider_session_id)?;
    let default_artifact_path = evidence_input_artifact_path(&storage_path, &object_id);
    let comparison_path = args.output.as_deref().unwrap_or(&default_artifact_path);
    let mut artifact = build_evidence_input_artifact(
        &snapshot,
        &provider_session_path,
        &messages_artifact,
        &messages_path,
        object_id,
        Utc::now().to_rfc3339(),
    );
    if let Some(existing_artifact) = read_existing_evidence_input_artifact(comparison_path).await?
        && evidence_input_artifact_matches(&existing_artifact, &artifact)
    {
        artifact.captured_at = existing_artifact.captured_at;
    }
    let artifact_path = args.output.unwrap_or(default_artifact_path);
    let record = persist_evidence_input_artifact(&storage_path, &artifact_path, &artifact).await?;

    println!(
        "{}",
        serde_json::to_string_pretty(&BuildEvidenceInputCommandOutput {
            ok: true,
            command_mode: "build-evidence-input",
            provider_session_id: artifact.provider_session_id.clone(),
            provider_session_object_id: artifact.provider_session_object_id.clone(),
            object_id: artifact.object_id.clone(),
            artifact_path: record.artifact_path,
            object_hash: record.object_hash,
            message_count: artifact.message_overview.message_count,
        })
        .context("failed to serialize build-evidence-input output")?
    );

    Ok(())
}

async fn resolve_extraction(args: ResolveExtractionArgs) -> Result<()> {
    let storage_path = util::try_get_storage_path(None)
        .context("claude-sdk commands must be run inside a Libra repository")?;
    let extraction_path = resolve_extraction_path(&storage_path, &args)?;
    let persisted = read_persisted_extraction(&extraction_path).await?;
    if persisted.schema != "libra.intent_extraction.v1" {
        bail!(
            "unsupported extraction schema '{}' in '{}'",
            persisted.schema,
            extraction_path.display()
        );
    }

    let risk_level = select_risk_level(
        args.risk_level.as_deref(),
        persisted.extraction.risk.level.clone(),
    )?;
    let working_dir =
        util::try_working_dir().context("failed to resolve repository working directory")?;
    let base_ref = current_head_sha().await;

    let mut spec = resolve_intentspec(
        persisted.extraction,
        risk_level.clone(),
        ResolveContext {
            working_dir: working_dir.display().to_string(),
            base_ref,
            created_by_id: args.created_by_id,
        },
    );

    let mut issues = validate_intentspec(&spec);
    for _ in 0..3 {
        if issues.is_empty() {
            break;
        }
        repair_intentspec(&mut spec, &issues);
        issues = validate_intentspec(&spec);
    }
    if !issues.is_empty() {
        let report = issues
            .iter()
            .map(|issue| format!("{}: {}", issue.path, issue.message))
            .collect::<Vec<_>>()
            .join("; ");
        bail!("resolved draft is still invalid after repair: {report}");
    }

    let summary = render_summary(&spec, None);
    let resolved_artifact = ResolvedIntentSpecArtifact {
        schema: "libra.intent_resolution.v1",
        ai_session_id: Some(persisted.ai_session_id.clone()),
        extraction_path: extraction_path.to_string_lossy().to_string(),
        extraction_source: persisted.source,
        risk_level: risk_level.clone(),
        summary: summary.clone(),
        intentspec: spec,
    };
    let output_path = match args.output {
        Some(path) => path,
        None => storage_path
            .join(INTENT_RESOLUTIONS_DIR)
            .join(format!("{}.json", persisted.ai_session_id)),
    };
    write_pretty_json_file(&output_path, &resolved_artifact).await?;

    let payload = ResolveExtractionCommandOutput {
        ok: true,
        command_mode: "resolve-extraction",
        ai_session_id: Some(persisted.ai_session_id),
        extraction_path: extraction_path.to_string_lossy().to_string(),
        resolved_spec_path: output_path.to_string_lossy().to_string(),
        risk_level: risk_level_label(&risk_level).to_string(),
        summary,
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&payload)
            .context("failed to serialize resolve-extraction output")?
    );
    Ok(())
}

async fn persist_intent(args: PersistIntentArgs) -> Result<()> {
    let storage_path = util::try_get_storage_path(None)
        .context("claude-sdk commands must be run inside a Libra repository")?;
    let resolution_path = resolve_resolution_path(&storage_path, &args)?;
    let resolved = read_persisted_resolution(&resolution_path).await?;
    if resolved.schema != "libra.intent_resolution.v1" {
        bail!(
            "unsupported resolution schema '{}' in '{}'",
            resolved.schema,
            resolution_path.display()
        );
    }

    let mcp_server = init_local_mcp_server(&storage_path).await?;
    let intent_id = persist_intentspec(&resolved.intentspec, mcp_server.as_ref()).await?;

    let binding_artifact = PersistedIntentInputBindingArtifact {
        schema: "libra.intent_input_binding.v1".to_string(),
        ai_session_id: resolved.ai_session_id.clone(),
        resolution_path: resolution_path.to_string_lossy().to_string(),
        extraction_path: resolved.extraction_path.clone(),
        extraction_source: resolved.extraction_source.clone(),
        risk_level: resolved.risk_level,
        intent_id: intent_id.clone(),
        summary: resolved.summary.clone(),
    };
    let binding_path = match args.output {
        Some(path) => path,
        None => storage_path.join(INTENT_INPUTS_DIR).join(format!(
            "{}.json",
            resolved
                .ai_session_id
                .clone()
                .unwrap_or_else(|| intent_id.clone())
        )),
    };
    write_pretty_json_file(&binding_path, &binding_artifact).await?;

    let payload = PersistIntentCommandOutput {
        ok: true,
        command_mode: "persist-intent",
        ai_session_id: resolved.ai_session_id,
        resolution_path: resolution_path.to_string_lossy().to_string(),
        intent_id,
        binding_path: binding_path.to_string_lossy().to_string(),
        summary: binding_artifact.summary,
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&payload)
            .context("failed to serialize persist-intent output")?
    );
    Ok(())
}

async fn bridge_run(args: BridgeRunArgs) -> Result<()> {
    let storage_path = util::try_get_storage_path(None)
        .context("claude-sdk commands must be run inside a Libra repository")?;
    validate_ai_session_id(&args.ai_session_id)?;
    if args.intent_binding.is_some() && args.intent_id.is_some() {
        bail!("pass either --intent-binding or --intent-id, not both");
    }

    let intent_binding = resolve_intent_binding(&storage_path, &args).await?;
    let requested_intent_id = args.intent_id.clone().or_else(|| {
        intent_binding
            .as_ref()
            .map(|binding| binding.artifact.intent_id.clone())
    });
    let binding_path = formal_run_binding_path(&storage_path, &args.ai_session_id);
    if let Some(existing) = read_existing_binding_if_live::<ClaudeFormalRunBindingArtifact>(
        &storage_path,
        &binding_path,
        "Claude formal run binding",
        &[
            ("task", |binding| binding.task_id.as_str()),
            ("run", |binding| binding.run_id.as_str()),
        ],
    )
    .await?
    {
        validate_formal_run_binding_consistency(&existing, &args.ai_session_id)?;
        load_audit_bundle_for_run_binding(&storage_path, &existing, &args.ai_session_id).await?;
        if let Some(intent_id) = requested_intent_id.as_deref()
            && existing.intent_id.as_deref() != Some(intent_id)
        {
            bail!(
                "existing formal run binding '{}' is linked to intent {:?}, but '{}' was requested; remove the stale binding to rebuild intentionally",
                binding_path.display(),
                existing.intent_id,
                intent_id
            );
        }
        print_bridge_run_output(&binding_path, &existing)?;
        return Ok(());
    }

    let audit_bundle_path = managed_audit_bundle_path(&storage_path, &args.ai_session_id);
    let audit_bundle: ManagedAuditBundle =
        read_json_artifact(&audit_bundle_path, "managed audit bundle").await?;
    if audit_bundle.schema != "libra.claude_managed_audit_bundle.v1" {
        bail!(
            "unsupported managed audit bundle schema '{}' in '{}'",
            audit_bundle.schema,
            audit_bundle_path.display()
        );
    }
    let summary = derive_formal_task_summary(&audit_bundle, intent_binding.as_ref());
    let description = derive_formal_task_description(&audit_bundle);
    let goal_type = derive_goal_type(&audit_bundle);
    let managed_run_status = audit_bundle
        .bridge
        .object_candidates
        .run_event
        .status
        .clone();
    let intent_extraction_status = audit_bundle.bridge.intent_extraction.status.clone();

    let mcp_server = init_local_mcp_server(&storage_path).await?;
    let actor = mcp_server
        .resolve_actor_from_params(Some("system"), Some("claude-sdk-bridge"))
        .map_err(|error| anyhow!("failed to resolve Claude SDK bridge actor: {error:?}"))?;
    let task_id = parse_created_id(
        "task",
        &mcp_server
            .create_task_impl(
                CreateTaskParams {
                    title: summary.clone(),
                    description: Some(description),
                    goal_type,
                    constraints: Some(vec![format!(
                        "claude-sdk ai_session_id={}",
                        args.ai_session_id
                    )]),
                    acceptance_criteria: None,
                    requested_by_kind: None,
                    requested_by_id: None,
                    dependencies: None,
                    intent_id: requested_intent_id.clone(),
                    parent_task_id: None,
                    origin_step_id: None,
                    status: Some(task_status_for_managed_run(&managed_run_status).to_string()),
                    reason: Some(format!(
                        "Claude SDK managed session {} bridged into formal task",
                        args.ai_session_id
                    )),
                    tags: None,
                    external_ids: None,
                    actor_kind: Some("system".to_string()),
                    actor_id: Some("claude-sdk-bridge".to_string()),
                },
                actor.clone(),
            )
            .await
            .map_err(|error| anyhow!("failed to create formal Claude task: {error:?}"))?,
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
                    context_snapshot_id: None,
                    error: run_error_for_managed_status(&managed_run_status),
                    agent_instances: None,
                    metrics_json: Some(
                        json!({
                            "provider": "claude",
                            "aiSessionId": args.ai_session_id,
                            "providerSessionId": audit_bundle.provider_session_id,
                            "intentExtractionStatus": intent_extraction_status,
                        })
                        .to_string(),
                    ),
                    reason: Some(format!(
                        "Claude SDK managed session {} bridged into formal run",
                        args.ai_session_id
                    )),
                    orchestrator_version: None,
                    tags: None,
                    external_ids: None,
                    actor_kind: Some("system".to_string()),
                    actor_id: Some("claude-sdk-bridge".to_string()),
                },
                actor,
            )
            .await
            .map_err(|error| anyhow!("failed to create formal Claude run: {error:?}"))?,
    )?;

    let binding = ClaudeFormalRunBindingArtifact {
        schema: "libra.claude_formal_run_binding.v1".to_string(),
        ai_session_id: args.ai_session_id,
        provider_session_id: audit_bundle.provider_session_id,
        task_id,
        run_id,
        audit_bundle_path: audit_bundle_path.to_string_lossy().to_string(),
        intent_binding_path: intent_binding
            .as_ref()
            .map(|resolved| resolved.path.to_string_lossy().to_string()),
        intent_id: requested_intent_id,
        managed_run_status,
        intent_extraction_status,
        summary,
        created_at: Utc::now().to_rfc3339(),
    };
    write_pretty_json_file(&binding_path, &binding).await?;
    print_bridge_run_output(&binding_path, &binding)?;
    Ok(())
}

async fn persist_evidence(args: PersistEvidenceArgs) -> Result<()> {
    let storage_path = util::try_get_storage_path(None)
        .context("claude-sdk commands must be run inside a Libra repository")?;
    validate_ai_session_id(&args.ai_session_id)?;

    let run_binding_path = formal_run_binding_path(&storage_path, &args.ai_session_id);
    let run_binding: ClaudeFormalRunBindingArtifact =
        read_typed_json_artifact(&run_binding_path, "formal Claude run binding")
            .await
            .with_context(|| {
                format!(
                    "run 'claude-sdk bridge-run --ai-session-id {}' first",
                    args.ai_session_id
                )
            })?;
    validate_formal_run_binding_consistency(&run_binding, &args.ai_session_id)?;
    let (resolved_audit_bundle_path, audit_bundle) =
        load_audit_bundle_for_run_binding(&storage_path, &run_binding, &args.ai_session_id).await?;
    let provider_session_object_id =
        build_provider_session_object_id(&run_binding.provider_session_id)?;
    let provider_session_path =
        provider_session_artifact_path(&storage_path, &provider_session_object_id);
    let evidence_input_object_id =
        build_evidence_input_object_id(&run_binding.provider_session_id)?;
    let evidence_input_path =
        evidence_input_artifact_path(&storage_path, &evidence_input_object_id);

    let mut entries = Vec::new();
    if provider_session_path.exists() {
        let snapshot: PersistedProviderSessionSnapshot =
            read_json_artifact(&provider_session_path, "provider session snapshot").await?;
        let summary = format!(
            "provider_session summary='{}'; message_count={}; first_kind={}; last_kind={}",
            snapshot.summary,
            snapshot
                .message_sync
                .as_ref()
                .map(|sync| sync.message_count)
                .unwrap_or(0),
            snapshot
                .message_sync
                .as_ref()
                .and_then(|sync| sync.first_message_kind.as_deref())
                .unwrap_or("-"),
            snapshot
                .message_sync
                .as_ref()
                .and_then(|sync| sync.last_message_kind.as_deref())
                .unwrap_or("-"),
        );
        entries.push(PendingEvidence {
            kind: "provider_session_snapshot".to_string(),
            source_path: provider_session_path.to_string_lossy().to_string(),
            summary,
        });
    }

    if evidence_input_path.exists() {
        let evidence_input: PersistedEvidenceInputArtifact =
            read_json_artifact(&evidence_input_path, "evidence input artifact").await?;
        let summary = format!(
            "evidence_input messages={}; assistant_messages={}; observed_tools={}; has_structured_output={}; has_permission_denials={}",
            evidence_input.message_overview.message_count,
            evidence_input.content_overview.assistant_message_count,
            evidence_input.content_overview.observed_tools.len(),
            evidence_input.runtime_signals.has_structured_output,
            evidence_input.runtime_signals.has_permission_denials,
        );
        entries.push(PendingEvidence {
            kind: "evidence_input_summary".to_string(),
            source_path: evidence_input_path.to_string_lossy().to_string(),
            summary,
        });
    }

    let extraction_summary = format!(
        "intent_extraction status={}; source={}; structured_output={}",
        audit_bundle.bridge.intent_extraction.status,
        audit_bundle.bridge.intent_extraction.source,
        audit_bundle
            .raw_artifact
            .result_message
            .as_ref()
            .and_then(|result| result.structured_output.as_ref())
            .is_some(),
    );
    entries.push(PendingEvidence {
        kind: "intent_extraction_result".to_string(),
        source_path: resolved_audit_bundle_path.to_string_lossy().to_string(),
        summary: extraction_summary,
    });
    entries.extend(build_managed_runtime_evidence_entries(
        AuditBundleSummaryContext {
            audit_bundle_path: &resolved_audit_bundle_path,
            audit_bundle: &audit_bundle,
        },
    ));
    let expected_entries = entries.clone();

    let binding_path = evidence_binding_path(&storage_path, &args.ai_session_id);
    if let Some(existing) = read_existing_binding_if_live::<ClaudeEvidenceBindingArtifact>(
        &storage_path,
        &binding_path,
        "Claude evidence binding",
        &[("run", |binding| binding.run_id.as_str())],
    )
    .await?
        && existing.run_id == run_binding.run_id
        && evidence_binding_objects_exist(&storage_path, &existing).await?
    {
        validate_evidence_binding_consistency(&existing, &args.ai_session_id, &run_binding)?;
        if evidence_binding_matches_expected(&existing, &expected_entries) {
            print_persist_evidence_output(&binding_path, &existing)?;
            return Ok(());
        }
    }

    let mcp_server = init_local_mcp_server(&storage_path).await?;
    let actor = mcp_server
        .resolve_actor_from_params(Some("system"), Some("claude-sdk-evidence"))
        .map_err(|error| anyhow!("failed to resolve Claude SDK evidence actor: {error:?}"))?;
    let mut evidence_entries = Vec::new();
    for entry in entries {
        let evidence_id = parse_created_id(
            "evidence",
            &mcp_server
                .create_evidence_impl(
                    CreateEvidenceParams {
                        run_id: run_binding.run_id.clone(),
                        patchset_id: None,
                        kind: entry.kind.clone(),
                        tool: "claude-sdk".to_string(),
                        command: None,
                        exit_code: None,
                        summary: Some(entry.summary.clone()),
                        report_artifacts: None,
                        tags: None,
                        external_ids: None,
                        actor_kind: Some("system".to_string()),
                        actor_id: Some("claude-sdk-evidence".to_string()),
                    },
                    actor.clone(),
                )
                .await
                .map_err(|error| {
                    anyhow!(
                        "failed to create Claude evidence '{}': {error:?}",
                        entry.kind
                    )
                })?,
        )?;
        evidence_entries.push(ClaudeEvidenceBindingEntry {
            kind: entry.kind,
            evidence_id,
            source_path: entry.source_path,
            summary: entry.summary,
        });
    }

    let binding = ClaudeEvidenceBindingArtifact {
        schema: "libra.claude_evidence_binding.v1".to_string(),
        ai_session_id: args.ai_session_id,
        provider_session_id: run_binding.provider_session_id,
        run_id: run_binding.run_id.clone(),
        run_binding_path: run_binding_path.to_string_lossy().to_string(),
        evidence_ids: evidence_entries
            .iter()
            .map(|entry| entry.evidence_id.clone())
            .collect(),
        evidences: evidence_entries,
        created_at: Utc::now().to_rfc3339(),
    };
    write_pretty_json_file(&binding_path, &binding).await?;
    print_persist_evidence_output(&binding_path, &binding)?;
    Ok(())
}

async fn persist_decision(args: PersistDecisionArgs) -> Result<()> {
    let storage_path = util::try_get_storage_path(None)
        .context("claude-sdk commands must be run inside a Libra repository")?;
    validate_ai_session_id(&args.ai_session_id)?;

    let run_binding_path = formal_run_binding_path(&storage_path, &args.ai_session_id);
    let run_binding: ClaudeFormalRunBindingArtifact =
        read_typed_json_artifact(&run_binding_path, "formal Claude run binding")
            .await
            .with_context(|| {
                format!(
                    "run 'claude-sdk bridge-run --ai-session-id {}' first",
                    args.ai_session_id
                )
            })?;
    validate_formal_run_binding_consistency(&run_binding, &args.ai_session_id)?;
    load_audit_bundle_for_run_binding(&storage_path, &run_binding, &args.ai_session_id).await?;
    let evidence_binding_path = evidence_binding_path(&storage_path, &args.ai_session_id);
    let evidence_binding: ClaudeEvidenceBindingArtifact =
        read_typed_json_artifact(&evidence_binding_path, "Claude evidence binding")
            .await
            .with_context(|| {
                format!(
                    "run 'claude-sdk persist-evidence --ai-session-id {}' first",
                    args.ai_session_id
                )
            })?;
    validate_evidence_binding_consistency(&evidence_binding, &args.ai_session_id, &run_binding)?;
    if !evidence_binding_objects_exist(&storage_path, &evidence_binding).await? {
        bail!(
            "Claude evidence binding references missing Evidence objects; run 'claude-sdk persist-evidence --ai-session-id {}' again",
            args.ai_session_id
        );
    }
    let decision_type = decision_type_for_binding(&run_binding, &evidence_binding);
    let rationale = format!(
        "managed_run_status={}; intent_extraction_status={}; evidence_count={}",
        run_binding.managed_run_status,
        run_binding.intent_extraction_status,
        evidence_binding.evidence_ids.len()
    );
    let binding_path = decision_binding_path(&storage_path, &args.ai_session_id);
    if let Some(existing) = read_existing_binding_if_live::<ClaudeDecisionBindingArtifact>(
        &storage_path,
        &binding_path,
        "Claude decision binding",
        &[
            ("run", |binding| binding.run_id.as_str()),
            ("decision", |binding| binding.decision_id.as_str()),
        ],
    )
    .await?
        && existing.run_id == run_binding.run_id
    {
        validate_decision_binding_consistency(&existing, &args.ai_session_id, &run_binding)?;
        if decision_binding_matches_expected(
            &existing,
            decision_type,
            &rationale,
            &evidence_binding.evidence_ids,
        ) {
            print_persist_decision_output(&binding_path, &existing)?;
            return Ok(());
        }
    }

    let mcp_server = init_local_mcp_server(&storage_path).await?;
    let actor = mcp_server
        .resolve_actor_from_params(Some("system"), Some("claude-sdk-decision"))
        .map_err(|error| anyhow!("failed to resolve Claude SDK decision actor: {error:?}"))?;
    let decision_id = parse_created_id(
        "decision",
        &mcp_server
            .create_decision_impl(
                CreateDecisionParams {
                    run_id: run_binding.run_id.clone(),
                    decision_type: decision_type.to_string(),
                    chosen_patchset_id: None,
                    result_commit_sha: None,
                    checkpoint_id: None,
                    rationale: Some(rationale.clone()),
                    tags: None,
                    external_ids: None,
                    actor_kind: Some("system".to_string()),
                    actor_id: Some("claude-sdk-decision".to_string()),
                },
                actor,
            )
            .await
            .map_err(|error| anyhow!("failed to create Claude decision: {error:?}"))?,
    )?;

    let binding = ClaudeDecisionBindingArtifact {
        schema: "libra.claude_decision_binding.v1".to_string(),
        ai_session_id: args.ai_session_id,
        provider_session_id: run_binding.provider_session_id,
        run_id: run_binding.run_id,
        decision_id,
        decision_type: decision_type.to_string(),
        rationale,
        run_binding_path: run_binding_path.to_string_lossy().to_string(),
        evidence_binding_path: evidence_binding_path.to_string_lossy().to_string(),
        evidence_ids: evidence_binding.evidence_ids.clone(),
        created_at: Utc::now().to_rfc3339(),
    };
    write_pretty_json_file(&binding_path, &binding).await?;
    print_persist_decision_output(&binding_path, &binding)?;
    Ok(())
}

async fn read_artifact(path: &Path) -> Result<ClaudeManagedArtifact> {
    let content = fs::read_to_string(path)
        .await
        .with_context(|| format!("failed to read managed artifact '{}'", path.display()))?;
    serde_json::from_str(&content)
        .with_context(|| format!("failed to parse managed artifact '{}'", path.display()))
}

async fn read_persisted_extraction(path: &Path) -> Result<PersistedIntentExtractionArtifact> {
    let content = fs::read_to_string(path)
        .await
        .with_context(|| format!("failed to read persisted extraction '{}'", path.display()))?;
    serde_json::from_str(&content)
        .with_context(|| format!("failed to parse persisted extraction '{}'", path.display()))
}

async fn read_persisted_resolution(path: &Path) -> Result<PersistedIntentResolutionArtifact> {
    let content = fs::read_to_string(path)
        .await
        .with_context(|| format!("failed to read persisted resolution '{}'", path.display()))?;
    serde_json::from_str(&content)
        .with_context(|| format!("failed to parse persisted resolution '{}'", path.display()))
}

fn resolve_prompt(args: &RunManagedArgs) -> Result<String> {
    match (&args.prompt, &args.prompt_file) {
        (Some(prompt), None) => Ok(prompt.clone()),
        (None, Some(path)) => std::fs::read_to_string(path)
            .with_context(|| format!("failed to read prompt file '{}'", path.display())),
        (Some(_), Some(_)) => {
            bail!("pass either --prompt or --prompt-file, not both")
        }
        (None, None) => {
            bail!("missing prompt; pass --prompt or --prompt-file")
        }
    }
}

fn resolve_extraction_path(storage_path: &Path, args: &ResolveExtractionArgs) -> Result<PathBuf> {
    match (&args.extraction, &args.ai_session_id) {
        (Some(path), None) => Ok(path.clone()),
        (None, Some(ai_session_id)) => {
            validate_ai_session_id(ai_session_id)?;
            Ok(storage_path
                .join(INTENT_EXTRACTIONS_DIR)
                .join(format!("{ai_session_id}.json")))
        }
        (Some(_), Some(_)) => bail!("pass either --extraction or --ai-session-id, not both"),
        (None, None) => bail!("missing extraction input; pass --extraction or --ai-session-id"),
    }
}

fn resolve_resolution_path(storage_path: &Path, args: &PersistIntentArgs) -> Result<PathBuf> {
    match (&args.resolution, &args.ai_session_id) {
        (Some(path), None) => Ok(path.clone()),
        (None, Some(ai_session_id)) => {
            validate_ai_session_id(ai_session_id)?;
            Ok(storage_path
                .join(INTENT_RESOLUTIONS_DIR)
                .join(format!("{ai_session_id}.json")))
        }
        (Some(_), Some(_)) => bail!("pass either --resolution or --ai-session-id, not both"),
        (None, None) => bail!("missing resolution input; pass --resolution or --ai-session-id"),
    }
}

fn select_risk_level(
    override_value: Option<&str>,
    draft_level: Option<RiskLevel>,
) -> Result<RiskLevel> {
    if let Some(raw) = override_value {
        return parse_risk_level(raw);
    }
    Ok(draft_level.unwrap_or(RiskLevel::Medium))
}

fn parse_risk_level(raw: &str) -> Result<RiskLevel> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "low" => Ok(RiskLevel::Low),
        "medium" => Ok(RiskLevel::Medium),
        "high" => Ok(RiskLevel::High),
        other => bail!("unsupported risk level '{other}'; expected one of low, medium, high"),
    }
}

fn risk_level_label(level: &RiskLevel) -> &'static str {
    match level {
        RiskLevel::Low => "low",
        RiskLevel::Medium => "medium",
        RiskLevel::High => "high",
    }
}

async fn current_head_sha() -> String {
    Head::current_commit()
        .await
        .map(|hash| hash.to_string())
        .unwrap_or_else(|| ZERO_COMMIT_SHA.to_string())
}

async fn init_local_mcp_server(storage_dir: &Path) -> Result<Arc<LibraMcpServer>> {
    let objects_dir = storage_dir.join("objects");

    fs::create_dir_all(&objects_dir).await.with_context(|| {
        format!(
            "failed to create local MCP storage directory '{}'",
            objects_dir.display()
        )
    })?;

    let db_path = storage_dir.join("libra.db");
    let db_path_str = db_path
        .to_str()
        .ok_or_else(|| anyhow!("database path '{}' is not valid UTF-8", db_path.display()))?;
    #[cfg(target_os = "windows")]
    let db_path_string = db_path_str.replace("\\", "/");
    #[cfg(target_os = "windows")]
    let db_path_str = db_path_string.as_str();

    let db_conn = Arc::new(
        db::establish_connection(db_path_str)
            .await
            .with_context(|| format!("failed to connect to database '{}'", db_path.display()))?,
    );
    let storage = Arc::new(LocalStorage::new(objects_dir));
    let history_manager = Arc::new(HistoryManager::new(
        storage.clone(),
        storage_dir.to_path_buf(),
        db_conn,
    ));

    Ok(Arc::new(LibraMcpServer::new(
        Some(history_manager),
        Some(storage),
    )))
}

async fn write_pretty_json_file<T>(path: &Path, value: &T) -> Result<()>
where
    T: Serialize,
{
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await.with_context(|| {
            format!(
                "failed to create directory for resolved artifact '{}'",
                parent.display()
            )
        })?;
    }
    let payload = serde_json::to_vec_pretty(value).context("failed to serialize JSON artifact")?;
    fs::write(path, payload)
        .await
        .with_context(|| format!("failed to write JSON artifact '{}'", path.display()))
}

#[derive(Debug, Clone)]
struct ResolvedIntentBinding {
    path: PathBuf,
    artifact: PersistedIntentInputBindingArtifact,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PendingEvidence {
    kind: String,
    source_path: String,
    summary: String,
}

struct AuditBundleSummaryContext<'a> {
    audit_bundle_path: &'a Path,
    audit_bundle: &'a ManagedAuditBundle,
}

type BindingObjectSelector<T> = (&'static str, fn(&T) -> &str);

trait BindingArtifactSchema {
    const SCHEMA: &'static str;

    fn schema(&self) -> &str;
}

impl BindingArtifactSchema for PersistedIntentInputBindingArtifact {
    const SCHEMA: &'static str = "libra.intent_input_binding.v1";

    fn schema(&self) -> &str {
        &self.schema
    }
}

impl BindingArtifactSchema for ClaudeFormalRunBindingArtifact {
    const SCHEMA: &'static str = "libra.claude_formal_run_binding.v1";

    fn schema(&self) -> &str {
        &self.schema
    }
}

impl BindingArtifactSchema for ClaudeEvidenceBindingArtifact {
    const SCHEMA: &'static str = "libra.claude_evidence_binding.v1";

    fn schema(&self) -> &str {
        &self.schema
    }
}

impl BindingArtifactSchema for ClaudeDecisionBindingArtifact {
    const SCHEMA: &'static str = "libra.claude_decision_binding.v1";

    fn schema(&self) -> &str {
        &self.schema
    }
}

async fn read_json_artifact<T>(path: &Path, label: &str) -> Result<T>
where
    T: DeserializeOwned,
{
    let content = fs::read_to_string(path)
        .await
        .with_context(|| format!("failed to read {label} '{}'", path.display()))?;
    serde_json::from_str(&content)
        .with_context(|| format!("failed to parse {label} '{}'", path.display()))
}

fn validate_binding_schema<T>(binding: &T, path: &Path, label: &str) -> Result<()>
where
    T: BindingArtifactSchema,
{
    if binding.schema() != T::SCHEMA {
        bail!(
            "unsupported {label} schema '{}' in '{}'",
            binding.schema(),
            path.display()
        );
    }
    Ok(())
}

async fn read_typed_json_artifact<T>(path: &Path, label: &str) -> Result<T>
where
    T: DeserializeOwned + BindingArtifactSchema,
{
    let artifact: T = read_json_artifact(path, label).await?;
    validate_binding_schema(&artifact, path, label)?;
    Ok(artifact)
}

async fn local_object_exists(
    storage_path: &Path,
    object_type: &str,
    object_id: &str,
) -> Result<bool> {
    let mcp_server = init_local_mcp_server(storage_path).await?;
    let history = mcp_server
        .intent_history_manager
        .as_ref()
        .ok_or_else(|| anyhow!("local MCP history manager is unavailable"))?;
    Ok(history
        .get_object_hash(object_type, object_id)
        .await
        .with_context(|| format!("failed to inspect {object_type} history for '{object_id}'"))?
        .is_some())
}

async fn read_existing_binding_if_live<T>(
    storage_path: &Path,
    binding_path: &Path,
    label: &str,
    required_objects: &[BindingObjectSelector<T>],
) -> Result<Option<T>>
where
    T: DeserializeOwned + BindingArtifactSchema,
{
    if !binding_path.exists() {
        return Ok(None);
    }

    let binding: T = read_typed_json_artifact(binding_path, label).await?;
    for (object_type, selector) in required_objects {
        if !local_object_exists(storage_path, object_type, selector(&binding)).await? {
            return Ok(None);
        }
    }

    Ok(Some(binding))
}

async fn evidence_binding_objects_exist(
    storage_path: &Path,
    binding: &ClaudeEvidenceBindingArtifact,
) -> Result<bool> {
    for evidence_id in &binding.evidence_ids {
        if !local_object_exists(storage_path, "evidence", evidence_id).await? {
            return Ok(false);
        }
    }
    Ok(true)
}

async fn resolve_intent_binding(
    storage_path: &Path,
    args: &BridgeRunArgs,
) -> Result<Option<ResolvedIntentBinding>> {
    if args.intent_id.is_some() {
        return Ok(None);
    }

    let path = args
        .intent_binding
        .clone()
        .unwrap_or_else(|| default_intent_binding_path(storage_path, &args.ai_session_id));
    if !path.exists() {
        if args.intent_binding.is_some() {
            bail!("intent binding '{}' does not exist", path.display());
        }
        return Ok(None);
    }

    let artifact: PersistedIntentInputBindingArtifact =
        read_typed_json_artifact(&path, "persisted intent binding").await?;
    if let Some(binding_ai_session_id) = artifact.ai_session_id.as_deref()
        && binding_ai_session_id != args.ai_session_id
    {
        bail!(
            "intent binding '{}' belongs to ai session '{}', not '{}'",
            path.display(),
            binding_ai_session_id,
            args.ai_session_id
        );
    }

    Ok(Some(ResolvedIntentBinding { path, artifact }))
}

fn derive_formal_task_summary(
    audit_bundle: &ManagedAuditBundle,
    intent_binding: Option<&ResolvedIntentBinding>,
) -> String {
    if let Some(binding) = intent_binding {
        return binding.artifact.summary.clone();
    }

    if let Some(extraction) = audit_bundle.bridge.intent_extraction_artifact.as_ref() {
        return extraction.extraction.intent.summary.clone();
    }

    let native_summary = audit_bundle.bridge.session_state.summary.trim();
    if !native_summary.is_empty() {
        return native_summary.to_string();
    }

    audit_bundle
        .raw_artifact
        .result_message
        .as_ref()
        .and_then(|result| result.structured_output.as_ref())
        .and_then(|value| value.get("summary"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .unwrap_or_else(|| format!("Claude SDK session {}", audit_bundle.provider_session_id))
}

fn derive_formal_task_description(audit_bundle: &ManagedAuditBundle) -> String {
    if let Some(extraction) = audit_bundle.bridge.intent_extraction_artifact.as_ref() {
        return extraction.extraction.intent.problem_statement.clone();
    }

    format!(
        "Formalized Claude SDK session {} from managed audit bundle.",
        audit_bundle.provider_session_id
    )
}

fn derive_goal_type(audit_bundle: &ManagedAuditBundle) -> Option<String> {
    let extraction = audit_bundle.bridge.intent_extraction_artifact.as_ref()?;
    let value = match extraction.extraction.intent.change_type {
        crate::internal::ai::intentspec::types::ChangeType::Feature => "feature",
        crate::internal::ai::intentspec::types::ChangeType::Bugfix => "bugfix",
        crate::internal::ai::intentspec::types::ChangeType::Test => "test",
        crate::internal::ai::intentspec::types::ChangeType::Refactor => "refactor",
        crate::internal::ai::intentspec::types::ChangeType::Performance => "perf",
        crate::internal::ai::intentspec::types::ChangeType::Security => "security",
        crate::internal::ai::intentspec::types::ChangeType::Docs => "docs",
        crate::internal::ai::intentspec::types::ChangeType::Chore => "chore",
        crate::internal::ai::intentspec::types::ChangeType::Unknown => return None,
    };
    Some(value.to_string())
}

fn task_status_for_managed_run(managed_run_status: &str) -> &'static str {
    match managed_run_status {
        "completed" => "done",
        "failed" | "timed_out" => "failed",
        _ => "running",
    }
}

fn run_status_for_managed_run(managed_run_status: &str) -> &'static str {
    match managed_run_status {
        "completed" => "completed",
        "failed" | "timed_out" => "failed",
        _ => "created",
    }
}

fn run_error_for_managed_status(managed_run_status: &str) -> Option<String> {
    match managed_run_status {
        "failed" => Some("Claude SDK managed session ended in failed state".to_string()),
        "timed_out" => Some("Claude SDK managed helper timed out".to_string()),
        _ => None,
    }
}

fn decision_type_for_binding(
    run_binding: &ClaudeFormalRunBindingArtifact,
    evidence_binding: &ClaudeEvidenceBindingArtifact,
) -> &'static str {
    match run_binding.managed_run_status.as_str() {
        "failed" | "timed_out" | "running" => "retry",
        _ if run_binding.intent_extraction_status == "accepted"
            && !evidence_binding.evidence_ids.is_empty() =>
        {
            "checkpoint"
        }
        _ => "abandon",
    }
}

async fn load_audit_bundle_for_run_binding(
    storage_path: &Path,
    run_binding: &ClaudeFormalRunBindingArtifact,
    expected_ai_session_id: &str,
) -> Result<(PathBuf, ManagedAuditBundle)> {
    let preferred_path = managed_audit_bundle_path(storage_path, expected_ai_session_id);
    let stored_path = PathBuf::from(&run_binding.audit_bundle_path);
    let audit_bundle_path = if preferred_path.exists() {
        preferred_path
    } else {
        stored_path
    };
    let audit_bundle: ManagedAuditBundle =
        read_json_artifact(&audit_bundle_path, "managed audit bundle")
            .await
            .with_context(|| {
                format!(
                    "failed to load managed audit bundle at '{}'",
                    audit_bundle_path.display()
                )
            })?;
    if audit_bundle.schema != "libra.claude_managed_audit_bundle.v1" {
        bail!(
            "unsupported managed audit bundle schema '{}' in '{}'",
            audit_bundle.schema,
            audit_bundle_path.display()
        );
    }
    if audit_bundle.ai_session_id != expected_ai_session_id {
        bail!(
            "managed audit bundle '{}' belongs to ai session '{}', not '{}'",
            audit_bundle_path.display(),
            audit_bundle.ai_session_id,
            expected_ai_session_id
        );
    }
    if audit_bundle.provider_session_id != run_binding.provider_session_id {
        bail!(
            "managed audit bundle '{}' belongs to provider session '{}', not '{}'",
            audit_bundle_path.display(),
            audit_bundle.provider_session_id,
            run_binding.provider_session_id
        );
    }
    Ok((audit_bundle_path, audit_bundle))
}

fn validate_formal_run_binding_consistency(
    binding: &ClaudeFormalRunBindingArtifact,
    expected_ai_session_id: &str,
) -> Result<()> {
    if binding.ai_session_id != expected_ai_session_id {
        bail!(
            "Claude formal run binding belongs to ai session '{}', not '{}'",
            binding.ai_session_id,
            expected_ai_session_id
        );
    }
    validate_provider_session_id(&binding.provider_session_id)
        .context("formal run binding contains an invalid provider session id")?;
    Ok(())
}

fn validate_evidence_binding_consistency(
    binding: &ClaudeEvidenceBindingArtifact,
    expected_ai_session_id: &str,
    run_binding: &ClaudeFormalRunBindingArtifact,
) -> Result<()> {
    if binding.ai_session_id != expected_ai_session_id {
        bail!(
            "Claude evidence binding belongs to ai session '{}', not '{}'",
            binding.ai_session_id,
            expected_ai_session_id
        );
    }
    if binding.provider_session_id != run_binding.provider_session_id {
        bail!(
            "Claude evidence binding belongs to provider session '{}', not '{}'",
            binding.provider_session_id,
            run_binding.provider_session_id
        );
    }
    if binding.run_id != run_binding.run_id {
        bail!(
            "Claude evidence binding belongs to run '{}', not '{}'",
            binding.run_id,
            run_binding.run_id
        );
    }
    Ok(())
}

fn validate_decision_binding_consistency(
    binding: &ClaudeDecisionBindingArtifact,
    expected_ai_session_id: &str,
    run_binding: &ClaudeFormalRunBindingArtifact,
) -> Result<()> {
    if binding.ai_session_id != expected_ai_session_id {
        bail!(
            "Claude decision binding belongs to ai session '{}', not '{}'",
            binding.ai_session_id,
            expected_ai_session_id
        );
    }
    if binding.provider_session_id != run_binding.provider_session_id {
        bail!(
            "Claude decision binding belongs to provider session '{}', not '{}'",
            binding.provider_session_id,
            run_binding.provider_session_id
        );
    }
    if binding.run_id != run_binding.run_id {
        bail!(
            "Claude decision binding belongs to run '{}', not '{}'",
            binding.run_id,
            run_binding.run_id
        );
    }
    Ok(())
}

fn evidence_binding_matches_expected(
    binding: &ClaudeEvidenceBindingArtifact,
    expected_entries: &[PendingEvidence],
) -> bool {
    if binding.evidence_ids.len() != binding.evidences.len() {
        return false;
    }

    let binding_entry_ids = binding
        .evidences
        .iter()
        .map(|entry| entry.evidence_id.clone())
        .collect::<Vec<_>>();
    if binding.evidence_ids != binding_entry_ids {
        return false;
    }

    let existing_entries = binding
        .evidences
        .iter()
        .map(|entry| PendingEvidence {
            kind: entry.kind.clone(),
            source_path: entry.source_path.clone(),
            summary: entry.summary.clone(),
        })
        .collect::<Vec<_>>();
    let mut expected_sorted = expected_entries.to_vec();
    let mut existing_sorted = existing_entries;
    expected_sorted.sort();
    existing_sorted.sort();
    existing_sorted == expected_sorted
}

fn decision_binding_matches_expected(
    binding: &ClaudeDecisionBindingArtifact,
    decision_type: &str,
    rationale: &str,
    evidence_ids: &[String],
) -> bool {
    binding.decision_type == decision_type
        && binding.rationale == rationale
        && binding.evidence_ids == evidence_ids
}

fn build_managed_runtime_evidence_entries(
    context: AuditBundleSummaryContext<'_>,
) -> Vec<PendingEvidence> {
    let object_candidates = &context.audit_bundle.bridge.object_candidates;
    let source_path = context.audit_bundle_path.to_string_lossy().to_string();
    let mut entries = vec![PendingEvidence {
        kind: "managed_provenance_summary".to_string(),
        source_path: source_path.clone(),
        summary: summarize_managed_provenance(context.audit_bundle),
    }];

    if let Some(run_usage_event) = object_candidates.run_usage_event.as_ref() {
        entries.push(PendingEvidence {
            kind: "managed_usage_summary".to_string(),
            source_path: source_path.clone(),
            summary: summarize_run_usage(run_usage_event),
        });
    }

    if let Some(summary) = summarize_tool_runtime(context.audit_bundle) {
        entries.push(PendingEvidence {
            kind: "managed_tool_runtime_summary".to_string(),
            source_path: source_path.clone(),
            summary,
        });
    }

    if !object_candidates.task_runtime_events.is_empty() {
        entries.push(PendingEvidence {
            kind: "managed_task_runtime_summary".to_string(),
            source_path: source_path.clone(),
            summary: summarize_semantic_runtime_events(
                "task_events",
                &object_candidates.task_runtime_events,
            ),
        });
    }

    if !object_candidates.decision_runtime_events.is_empty() {
        entries.push(PendingEvidence {
            kind: "managed_decision_runtime_summary".to_string(),
            source_path: source_path.clone(),
            summary: summarize_semantic_runtime_events(
                "decision_events",
                &object_candidates.decision_runtime_events,
            ),
        });
    }

    if !object_candidates.context_runtime_events.is_empty() {
        entries.push(PendingEvidence {
            kind: "managed_context_runtime_summary".to_string(),
            source_path,
            summary: summarize_semantic_runtime_events(
                "context_events",
                &object_candidates.context_runtime_events,
            ),
        });
    }

    entries
}

fn summarize_managed_provenance(audit_bundle: &ManagedAuditBundle) -> String {
    let object_candidates = &audit_bundle.bridge.object_candidates;
    let provider_init = &object_candidates.provider_init_snapshot;
    let provenance = &object_candidates.provenance_snapshot;
    format!(
        "provider=claude; model={}; permission_mode={}; agents={}; skills={}; mcp_servers={}; plugins={}",
        provenance.model.as_deref().unwrap_or("-"),
        provenance
            .parameters
            .get("permissionMode")
            .and_then(Value::as_str)
            .unwrap_or("-"),
        provider_init.agents.len(),
        provider_init.skills.len(),
        provider_init.mcp_servers.len(),
        provider_init.plugins.len(),
    )
}

fn summarize_run_usage(run_usage_event: &ManagedRunUsageEvent) -> String {
    let input_tokens = usage_counter(&run_usage_event.usage, &["input_tokens", "inputTokens"]);
    let output_tokens = usage_counter(&run_usage_event.usage, &["output_tokens", "outputTokens"]);
    let total_tokens = usage_counter(
        &run_usage_event.usage,
        &["total_tokens", "totalTokens", "total"],
    );
    format!(
        "usage input_tokens={}; output_tokens={}; total_tokens={}",
        input_tokens, output_tokens, total_tokens
    )
}

fn summarize_tool_runtime(audit_bundle: &ManagedAuditBundle) -> Option<String> {
    let object_candidates = &audit_bundle.bridge.object_candidates;
    let repo_root = audit_bundle.bridge.session_state.working_dir.as_str();
    let tool_names = audit_bundle
        .bridge
        .tool_invocations
        .iter()
        .filter_map(|invocation| invocation.tool_name.clone())
        .collect::<BTreeSet<_>>();
    let touched_paths = audit_bundle
        .bridge
        .touch_hints
        .iter()
        .filter_map(|hint| persistable_touch_hint(hint, repo_root))
        .collect::<BTreeSet<_>>();
    if object_candidates.tool_invocation_events.is_empty()
        && object_candidates.tool_runtime_events.is_empty()
        && tool_names.is_empty()
    {
        return None;
    }

    Some(format!(
        "tool_invocations={}; tool_runtime_events={}; tools={}; touched_paths={}",
        object_candidates.tool_invocation_events.len(),
        object_candidates.tool_runtime_events.len(),
        join_set(&tool_names),
        join_set(&touched_paths),
    ))
}

fn summarize_semantic_runtime_events(
    label: &str,
    events: &[ManagedSemanticRuntimeEvent],
) -> String {
    let kinds = events
        .iter()
        .map(|event| event.kind.clone())
        .collect::<BTreeSet<_>>();
    format!("{label} count={}; kinds={}", events.len(), join_set(&kinds))
}

fn join_set(values: &BTreeSet<String>) -> String {
    if values.is_empty() {
        "-".to_string()
    } else {
        values.iter().cloned().collect::<Vec<_>>().join(",")
    }
}

fn usage_counter(value: &Value, keys: &[&str]) -> u64 {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_u64))
        .unwrap_or(0)
}

fn persistable_touch_hint(hint: &str, repo_root: &str) -> Option<String> {
    let normalized_repo_root = normalize_portable_path(repo_root)?;
    if !is_platform_agnostic_absolute_hint(&normalized_repo_root) {
        return None;
    }

    let trimmed = hint.trim();
    if trimmed.is_empty() {
        return None;
    }

    let normalized_hint = if is_platform_agnostic_absolute_hint(trimmed) {
        normalize_portable_path(trimmed)?
    } else {
        join_portable_path(&normalized_repo_root, trimmed)?
    };
    if normalized_hint == normalized_repo_root {
        return None;
    }

    strip_portable_prefix(&normalized_hint, &normalized_repo_root)
}

fn normalize_portable_path(path: &str) -> Option<String> {
    let normalized = path.trim().replace('\\', "/");
    if normalized.is_empty() {
        return None;
    }

    let (prefix, absolute, rest) = if let Some(rest) = normalized.strip_prefix("//") {
        let mut parts = rest.split('/').filter(|part| !part.is_empty());
        let server = parts.next()?;
        let share = parts.next()?;
        (
            format!("//{server}/{share}"),
            true,
            parts.collect::<Vec<_>>().join("/"),
        )
    } else if normalized.len() >= 2
        && normalized.as_bytes()[0].is_ascii_alphabetic()
        && normalized.as_bytes()[1] == b':'
    {
        let prefix = normalized[..2].to_string();
        let rest = normalized[2..].trim_start_matches('/').to_string();
        (prefix, true, rest)
    } else if let Some(rest) = normalized.strip_prefix('/') {
        (String::new(), true, rest.to_string())
    } else {
        (String::new(), false, normalized)
    };

    let mut components = Vec::new();
    for component in rest.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                if !components.is_empty() {
                    components.pop();
                }
            }
            part => components.push(part),
        }
    }

    let mut result = prefix;
    if absolute && !result.ends_with('/') {
        result.push('/');
    }
    if !components.is_empty() {
        result.push_str(&components.join("/"));
    }

    if result.is_empty() && absolute {
        Some("/".to_string())
    } else if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

fn join_portable_path(base: &str, relative: &str) -> Option<String> {
    let normalized_base = normalize_portable_path(base)?;
    let normalized_relative = relative.trim().replace('\\', "/");
    if normalized_relative.is_empty() {
        return None;
    }

    let joined = if normalized_base.ends_with('/') {
        format!("{normalized_base}{normalized_relative}")
    } else {
        format!("{normalized_base}/{normalized_relative}")
    };
    normalize_portable_path(&joined)
}

fn is_platform_agnostic_absolute_hint(path: &str) -> bool {
    let normalized = path.trim().replace('\\', "/");
    if normalized.starts_with('/') {
        return true;
    }

    let bytes = normalized.as_bytes();
    bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'/'
}

fn strip_portable_prefix(path: &str, root: &str) -> Option<String> {
    if portable_path_eq(path, root) {
        return None;
    }

    let prefix = if root.ends_with('/') {
        root.to_string()
    } else {
        format!("{root}/")
    };
    if should_compare_portable_paths_case_insensitively(root) {
        let path_lower = path.to_ascii_lowercase();
        let prefix_lower = prefix.to_ascii_lowercase();
        return path_lower.strip_prefix(&prefix_lower).and_then(|relative| {
            (!relative.is_empty()).then_some(path[prefix.len()..].to_string())
        });
    }

    path.strip_prefix(&prefix)
        .and_then(|relative| (!relative.is_empty()).then_some(relative.to_string()))
}

fn portable_path_eq(left: &str, right: &str) -> bool {
    if should_compare_portable_paths_case_insensitively(left)
        || should_compare_portable_paths_case_insensitively(right)
    {
        left.eq_ignore_ascii_case(right)
    } else {
        left == right
    }
}

fn should_compare_portable_paths_case_insensitively(path: &str) -> bool {
    let normalized = path.trim().replace('\\', "/");
    normalized.starts_with("//")
        || (normalized.len() >= 2
            && normalized.as_bytes()[0].is_ascii_alphabetic()
            && normalized.as_bytes()[1] == b':')
}

fn parse_created_id(kind: &str, result: &rmcp::model::CallToolResult) -> Result<String> {
    if result.is_error.unwrap_or(false) {
        bail!("MCP create_{kind} returned an error result");
    }

    for content in &result.content {
        if let Some(text) = content.as_text().map(|value| value.text.as_str())
            && let Some(id) = text.split("ID:").nth(1)
        {
            let id = id.trim();
            if !id.is_empty() {
                return Ok(id.to_string());
            }
        }
    }

    bail!("failed to parse {kind} id from MCP response")
}

fn managed_audit_bundle_path(storage_path: &Path, ai_session_id: &str) -> PathBuf {
    storage_path
        .join("audit-bundles")
        .join(format!("{ai_session_id}.json"))
}

fn default_intent_binding_path(storage_path: &Path, ai_session_id: &str) -> PathBuf {
    storage_path
        .join(INTENT_INPUTS_DIR)
        .join(format!("{ai_session_id}.json"))
}

fn formal_run_binding_path(storage_path: &Path, ai_session_id: &str) -> PathBuf {
    storage_path
        .join(FORMAL_RUN_BINDINGS_DIR)
        .join(format!("{ai_session_id}.json"))
}

fn evidence_binding_path(storage_path: &Path, ai_session_id: &str) -> PathBuf {
    storage_path
        .join(EVIDENCE_BINDINGS_DIR)
        .join(format!("{ai_session_id}.json"))
}

fn decision_binding_path(storage_path: &Path, ai_session_id: &str) -> PathBuf {
    storage_path
        .join(DECISION_BINDINGS_DIR)
        .join(format!("{ai_session_id}.json"))
}

fn print_bridge_run_output(path: &Path, binding: &ClaudeFormalRunBindingArtifact) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(&BridgeRunCommandOutput {
            ok: true,
            command_mode: "bridge-run",
            ai_session_id: binding.ai_session_id.clone(),
            provider_session_id: binding.provider_session_id.clone(),
            task_id: binding.task_id.clone(),
            run_id: binding.run_id.clone(),
            binding_path: path.to_string_lossy().to_string(),
            intent_id: binding.intent_id.clone(),
        })
        .context("failed to serialize bridge-run output")?
    );
    Ok(())
}

fn print_persist_evidence_output(
    path: &Path,
    binding: &ClaudeEvidenceBindingArtifact,
) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(&PersistEvidenceCommandOutput {
            ok: true,
            command_mode: "persist-evidence",
            ai_session_id: binding.ai_session_id.clone(),
            run_id: binding.run_id.clone(),
            evidence_ids: binding.evidence_ids.clone(),
            binding_path: path.to_string_lossy().to_string(),
        })
        .context("failed to serialize persist-evidence output")?
    );
    Ok(())
}

fn print_persist_decision_output(
    path: &Path,
    binding: &ClaudeDecisionBindingArtifact,
) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(&PersistDecisionCommandOutput {
            ok: true,
            command_mode: "persist-decision",
            ai_session_id: binding.ai_session_id.clone(),
            run_id: binding.run_id.clone(),
            decision_id: binding.decision_id.clone(),
            decision_type: binding.decision_type.clone(),
            binding_path: path.to_string_lossy().to_string(),
        })
        .context("failed to serialize persist-decision output")?
    );
    Ok(())
}

fn validate_provider_session_id(provider_session_id: &str) -> Result<()> {
    if provider_session_id.len() > 128 {
        bail!("invalid provider session id: exceeds 128 characters");
    }
    if !provider_session_id
        .chars()
        .all(|char| char.is_ascii_alphanumeric() || matches!(char, '.' | '_' | '-'))
    {
        bail!("invalid provider session id: only [A-Za-z0-9._-] is allowed");
    }
    Ok(())
}

fn validate_ai_session_id(ai_session_id: &str) -> Result<()> {
    if ai_session_id.len() > 128 {
        bail!("invalid ai session id: exceeds 128 characters");
    }
    if !ai_session_id
        .chars()
        .all(|char| char.is_ascii_alphanumeric() || matches!(char, '.' | '_' | '-'))
    {
        bail!("invalid ai session id: only [A-Za-z0-9._-] is allowed");
    }
    Ok(())
}

fn build_provider_session_object_id(provider_session_id: &str) -> Result<String> {
    validate_provider_session_id(provider_session_id)?;
    Ok(format!("claude_provider_session__{provider_session_id}"))
}

fn provider_session_artifact_path(storage_path: &Path, object_id: &str) -> PathBuf {
    storage_path
        .join(PROVIDER_SESSIONS_DIR)
        .join(format!("{object_id}.json"))
}

fn provider_session_messages_artifact_path(storage_path: &Path, object_id: &str) -> PathBuf {
    storage_path
        .join(PROVIDER_SESSIONS_DIR)
        .join(format!("{object_id}.messages.json"))
}

fn build_evidence_input_object_id(provider_session_id: &str) -> Result<String> {
    validate_provider_session_id(provider_session_id)?;
    Ok(format!("claude_evidence_input__{provider_session_id}"))
}

fn evidence_input_artifact_path(storage_path: &Path, object_id: &str) -> PathBuf {
    storage_path
        .join(EVIDENCE_INPUTS_DIR)
        .join(format!("{object_id}.json"))
}

async fn read_persisted_provider_session_snapshot(
    path: &Path,
) -> Result<PersistedProviderSessionSnapshot> {
    let content = fs::read_to_string(path).await.with_context(|| {
        format!(
            "failed to read provider session snapshot '{}'",
            path.display()
        )
    })?;
    serde_json::from_str(&content).with_context(|| {
        format!(
            "failed to parse provider session snapshot '{}'",
            path.display()
        )
    })
}

async fn read_existing_evidence_input_artifact(
    path: &Path,
) -> Result<Option<PersistedEvidenceInputArtifact>> {
    if !path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(path).await.with_context(|| {
        format!(
            "failed to read evidence input artifact '{}'",
            path.display()
        )
    })?;
    let artifact = serde_json::from_str(&content).with_context(|| {
        format!(
            "failed to parse evidence input artifact '{}'",
            path.display()
        )
    })?;
    Ok(Some(artifact))
}

fn evidence_input_artifact_matches(
    existing: &PersistedEvidenceInputArtifact,
    candidate: &PersistedEvidenceInputArtifact,
) -> bool {
    existing.schema == candidate.schema
        && existing.object_type == candidate.object_type
        && existing.provider == candidate.provider
        && existing.object_id == candidate.object_id
        && existing.provider_session_id == candidate.provider_session_id
        && existing.provider_session_object_id == candidate.provider_session_object_id
        && existing.summary == candidate.summary
        && existing.source_artifacts == candidate.source_artifacts
        && existing.message_overview == candidate.message_overview
        && existing.content_overview == candidate.content_overview
        && existing.runtime_signals == candidate.runtime_signals
        && existing.latest_result == candidate.latest_result
}

async fn read_provider_session_messages_artifact(
    path: &Path,
) -> Result<ProviderSessionMessagesArtifact> {
    let content = fs::read_to_string(path).await.with_context(|| {
        format!(
            "failed to read provider session messages artifact '{}'",
            path.display()
        )
    })?;
    serde_json::from_str(&content).with_context(|| {
        format!(
            "failed to parse provider session messages artifact '{}'",
            path.display()
        )
    })
}

fn build_provider_session_message_sync(
    artifact_path: &Path,
    messages: &[Value],
    offset: usize,
    limit: Option<usize>,
    captured_at: String,
) -> ProviderSessionMessageSync {
    let mut kind_counts = BTreeMap::new();
    let mut kinds = Vec::new();
    for message in messages {
        if let Some(kind) = provider_message_kind(message) {
            *kind_counts.entry(kind.clone()).or_insert(0) += 1;
            kinds.push(kind);
        }
    }

    ProviderSessionMessageSync {
        artifact_path: artifact_path.to_string_lossy().to_string(),
        message_count: messages.len(),
        kind_counts,
        first_message_kind: kinds.first().cloned(),
        last_message_kind: kinds.last().cloned(),
        offset,
        limit,
        captured_at,
    }
}

fn provider_message_kind(message: &Value) -> Option<String> {
    let message_type = message.get("type").and_then(Value::as_str)?;
    match message.get("subtype").and_then(Value::as_str) {
        Some(subtype) => Some(format!("{message_type}:{subtype}")),
        None => Some(message_type.to_string()),
    }
}

fn build_evidence_input_artifact(
    snapshot: &PersistedProviderSessionSnapshot,
    provider_session_path: &Path,
    messages_artifact: &ProviderSessionMessagesArtifact,
    messages_path: &Path,
    object_id: String,
    captured_at: String,
) -> PersistedEvidenceInputArtifact {
    let mut assistant_message_count = 0usize;
    let mut user_message_count = 0usize;
    let mut observed_tools = BTreeMap::new();
    let mut observed_paths = BTreeSet::new();
    let mut assistant_text_previews = Vec::new();
    let mut result_message_count = 0usize;
    let mut tool_runtime_count = 0usize;
    let mut task_runtime_count = 0usize;
    let mut partial_assistant_event_count = 0usize;
    let mut has_structured_output = false;
    let mut has_permission_denials = false;
    let mut latest_result = None;

    for message in &messages_artifact.messages {
        match message.get("type").and_then(Value::as_str) {
            Some("assistant") => {
                assistant_message_count += 1;
                collect_message_content_evidence(
                    message,
                    &mut observed_tools,
                    &mut observed_paths,
                    &mut assistant_text_previews,
                );
            }
            Some("user") => {
                user_message_count += 1;
                collect_message_content_evidence(
                    message,
                    &mut observed_tools,
                    &mut observed_paths,
                    &mut Vec::new(),
                );
            }
            Some("result") => {
                result_message_count += 1;
                has_structured_output |= message.get("structured_output").is_some();
                let permission_denial_count = message
                    .get("permission_denials")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len);
                has_permission_denials |= permission_denial_count > 0;
                latest_result = Some(EvidenceInputLatestResult {
                    subtype: message
                        .get("subtype")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                    stop_reason: message
                        .get("stop_reason")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                    duration_ms: message.get("duration_ms").and_then(Value::as_u64),
                    duration_api_ms: message.get("duration_api_ms").and_then(Value::as_u64),
                    total_cost_usd: message.get("total_cost_usd").and_then(Value::as_f64),
                    num_turns: message.get("num_turns").and_then(Value::as_u64),
                    permission_denial_count,
                });
            }
            Some("tool_progress" | "tool_use_summary") => {
                tool_runtime_count += 1;
                if let Some(tool_name) = message.get("tool_name").and_then(Value::as_str) {
                    *observed_tools.entry(tool_name.to_string()).or_insert(0) += 1;
                }
            }
            _ if matches!(
                message.get("subtype").and_then(Value::as_str),
                Some("task_started" | "task_progress" | "task_notification")
            ) =>
            {
                task_runtime_count += 1;
            }
            Some("stream_event") => {
                partial_assistant_event_count += 1;
            }
            _ => {}
        }
    }

    PersistedEvidenceInputArtifact {
        schema: "libra.evidence_input.v1".to_string(),
        object_type: "evidence_input".to_string(),
        provider: "claude".to_string(),
        object_id,
        provider_session_id: snapshot.provider_session_id.clone(),
        provider_session_object_id: snapshot.object_id.clone(),
        summary: snapshot.summary.clone(),
        source_artifacts: EvidenceInputSourceArtifacts {
            provider_session_path: provider_session_path.to_string_lossy().to_string(),
            messages_path: messages_path.to_string_lossy().to_string(),
        },
        message_overview: EvidenceInputMessageOverview {
            message_count: messages_artifact.messages.len(),
            kind_counts: snapshot
                .message_sync
                .as_ref()
                .map(|sync| sync.kind_counts.clone())
                .unwrap_or_default(),
            first_message_kind: snapshot
                .message_sync
                .as_ref()
                .and_then(|sync| sync.first_message_kind.clone()),
            last_message_kind: snapshot
                .message_sync
                .as_ref()
                .and_then(|sync| sync.last_message_kind.clone()),
        },
        content_overview: EvidenceInputContentOverview {
            assistant_message_count,
            user_message_count,
            observed_tools,
            observed_paths: observed_paths.into_iter().collect(),
            assistant_text_previews,
        },
        runtime_signals: EvidenceInputRuntimeSignals {
            result_message_count,
            tool_runtime_count,
            task_runtime_count,
            partial_assistant_event_count,
            has_structured_output,
            has_permission_denials,
        },
        latest_result,
        captured_at,
    }
}

fn collect_message_content_evidence(
    message: &Value,
    observed_tools: &mut BTreeMap<String, usize>,
    observed_paths: &mut BTreeSet<String>,
    assistant_text_previews: &mut Vec<String>,
) {
    let blocks = message
        .get("message")
        .and_then(|inner| inner.get("content"))
        .and_then(Value::as_array);
    let Some(blocks) = blocks else {
        return;
    };

    for block in blocks {
        if let Some(text) = block.get("text").and_then(Value::as_str)
            && assistant_text_previews.len() < 3
        {
            let normalized = normalize_text_preview(text);
            if !normalized.is_empty() {
                assistant_text_previews.push(normalized);
            }
        }

        if block.get("type").and_then(Value::as_str) == Some("tool_use") {
            if let Some(tool_name) = block.get("name").and_then(Value::as_str) {
                *observed_tools.entry(tool_name.to_string()).or_insert(0) += 1;
            }
            if let Some(input) = block.get("input") {
                collect_path_candidates(input, observed_paths);
            }
        }
    }
}

fn collect_path_candidates(value: &Value, observed_paths: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            for (key, nested) in map {
                if matches!(
                    key.as_str(),
                    "file_path"
                        | "path"
                        | "cwd"
                        | "worktree_path"
                        | "trigger_file_path"
                        | "parent_file_path"
                ) {
                    if let Some(path) = nested.as_str() {
                        observed_paths.insert(path.to_string());
                    }
                } else if matches!(key.as_str(), "paths" | "files") {
                    if let Some(items) = nested.as_array() {
                        for item in items {
                            if let Some(path) = item.as_str() {
                                observed_paths.insert(path.to_string());
                            }
                        }
                    }
                } else {
                    collect_path_candidates(nested, observed_paths);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_path_candidates(item, observed_paths);
            }
        }
        _ => {}
    }
}

fn normalize_text_preview(text: &str) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= 180 {
        compact
    } else {
        let preview = compact.chars().take(180).collect::<String>();
        format!("{preview}...")
    }
}

async fn persist_provider_session_snapshot(
    storage_path: &Path,
    session: ClaudeSdkSessionInfo,
) -> Result<SyncSessionRecord> {
    let existing_snapshot =
        read_existing_provider_session_snapshot(storage_path, &session.session_id).await?;
    let object_id = build_provider_session_object_id(&session.session_id)?;
    let mut snapshot = PersistedProviderSessionSnapshot {
        schema: "libra.provider_session.v3".to_string(),
        object_type: "provider_session".to_string(),
        provider: "claude".to_string(),
        object_id,
        provider_session_id: session.session_id,
        summary: session.summary,
        custom_title: session.custom_title,
        first_prompt: session.first_prompt,
        git_branch: session.git_branch,
        cwd: session.cwd,
        tag: session.tag,
        created_at: session.created_at,
        last_modified: session.last_modified,
        file_size: session.file_size,
        captured_at: Utc::now().to_rfc3339(),
        message_sync: existing_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.message_sync.clone()),
    };
    if let Some(existing_snapshot) = &existing_snapshot
        && provider_session_snapshot_matches(existing_snapshot, &snapshot)
    {
        snapshot.captured_at = existing_snapshot.captured_at.clone();
    }

    upsert_provider_session_snapshot(storage_path, &snapshot).await
}

async fn upsert_provider_session_snapshot(
    storage_path: &Path,
    snapshot: &PersistedProviderSessionSnapshot,
) -> Result<SyncSessionRecord> {
    let artifact_path = provider_session_artifact_path(storage_path, &snapshot.object_id);
    write_pretty_json_file(&artifact_path, &snapshot)
        .await
        .with_context(|| {
            format!(
                "failed to write provider session snapshot '{}'",
                artifact_path.display()
            )
        })?;

    let payload = serde_json::to_vec_pretty(&snapshot)
        .context("failed to serialize provider session snapshot")?;
    let object_hash = write_git_object(storage_path, "blob", &payload)
        .context("failed to write provider session snapshot object")?;
    let mcp_server = init_local_mcp_server(storage_path).await?;
    let history = mcp_server
        .intent_history_manager
        .as_ref()
        .ok_or_else(|| anyhow!("local MCP history manager is unavailable"))?;
    let existing_hash = history
        .get_object_hash("provider_session", &snapshot.object_id)
        .await
        .context("failed to inspect existing provider session history")?;
    if existing_hash != Some(object_hash) {
        history
            .append("provider_session", &snapshot.object_id, object_hash)
            .await
            .context("failed to append provider session snapshot to history")?;
    }

    Ok(SyncSessionRecord {
        provider_session_id: snapshot.provider_session_id.clone(),
        object_id: snapshot.object_id.clone(),
        artifact_path: artifact_path.to_string_lossy().to_string(),
        object_hash: object_hash.to_string(),
    })
}

async fn persist_evidence_input_artifact(
    storage_path: &Path,
    artifact_path: &Path,
    artifact: &PersistedEvidenceInputArtifact,
) -> Result<SyncSessionRecord> {
    write_pretty_json_file(artifact_path, artifact)
        .await
        .with_context(|| {
            format!(
                "failed to write evidence input artifact '{}'",
                artifact_path.display()
            )
        })?;

    let payload = serde_json::to_vec_pretty(artifact)
        .context("failed to serialize evidence input artifact")?;
    let object_hash = write_git_object(storage_path, "blob", &payload)
        .context("failed to write evidence input object")?;
    let mcp_server = init_local_mcp_server(storage_path).await?;
    let history = mcp_server
        .intent_history_manager
        .as_ref()
        .ok_or_else(|| anyhow!("local MCP history manager is unavailable"))?;
    let existing_hash = history
        .get_object_hash("evidence_input", &artifact.object_id)
        .await
        .context("failed to inspect existing evidence input history")?;
    if existing_hash != Some(object_hash) {
        history
            .append("evidence_input", &artifact.object_id, object_hash)
            .await
            .context("failed to append evidence input to history")?;
    }

    Ok(SyncSessionRecord {
        provider_session_id: artifact.provider_session_id.clone(),
        object_id: artifact.object_id.clone(),
        artifact_path: artifact_path.to_string_lossy().to_string(),
        object_hash: object_hash.to_string(),
    })
}

async fn read_existing_provider_session_snapshot(
    storage_path: &Path,
    provider_session_id: &str,
) -> Result<Option<PersistedProviderSessionSnapshot>> {
    let object_id = build_provider_session_object_id(provider_session_id)?;
    let artifact_path = provider_session_artifact_path(storage_path, &object_id);
    if !artifact_path.exists() {
        return Ok(None);
    }

    let snapshot = read_persisted_provider_session_snapshot(&artifact_path)
        .await
        .with_context(|| {
            format!(
                "failed to refresh provider session snapshot '{}'",
                artifact_path.display()
            )
        })?;
    Ok(Some(snapshot))
}

fn provider_session_snapshot_matches(
    existing: &PersistedProviderSessionSnapshot,
    candidate: &PersistedProviderSessionSnapshot,
) -> bool {
    existing.schema == candidate.schema
        && existing.object_type == candidate.object_type
        && existing.provider == candidate.provider
        && existing.provider_session_id == candidate.provider_session_id
        && existing.object_id == candidate.object_id
        && existing.summary == candidate.summary
        && existing.custom_title == candidate.custom_title
        && existing.first_prompt == candidate.first_prompt
        && existing.git_branch == candidate.git_branch
        && existing.cwd == candidate.cwd
        && existing.tag == candidate.tag
        && existing.created_at == candidate.created_at
        && existing.last_modified == candidate.last_modified
        && existing.file_size == candidate.file_size
        && existing.message_sync == candidate.message_sync
}

async fn materialize_helper(
    helper_path: Option<&Path>,
) -> Result<(Option<EmbeddedHelperDir>, PathBuf)> {
    if let Some(path) = helper_path {
        return Ok((None, path.to_path_buf()));
    }

    let temp_dir = tempfile::Builder::new()
        .prefix("libra-claude-sdk-helper-")
        .tempdir()
        .context("failed to create temporary helper directory")?;
    let temp_dir_path = temp_dir.path().to_path_buf();
    let helper_path = temp_dir_path.join("libra-claude-managed-helper.cjs");
    let mut helper_file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&helper_path)
        .await
        .with_context(|| format!("failed to create helper '{}'", helper_path.display()))?;
    helper_file
        .write_all(EMBEDDED_HELPER_SOURCE.as_bytes())
        .await
        .with_context(|| format!("failed to write helper '{}'", helper_path.display()))?;
    Ok((
        Some(EmbeddedHelperDir {
            _temp_dir: temp_dir,
        }),
        helper_path,
    ))
}

async fn invoke_helper(
    node_binary: &str,
    helper_path: &Path,
    request: &ManagedHelperRequest,
) -> Result<ClaudeManagedArtifact> {
    invoke_helper_json(node_binary, helper_path, request)
        .await
        .context("failed to invoke Claude SDK managed helper")
}

async fn invoke_helper_json<T>(
    node_binary: &str,
    helper_path: &Path,
    request: &T,
) -> Result<T::Output>
where
    T: Serialize + HelperResponse,
{
    let serialized_request =
        serde_json::to_vec(request).context("failed to serialize helper request")?;
    let mut child = Command::new(node_binary)
        .arg(helper_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| {
            format!(
                "failed to start Claude SDK helper with '{}' '{}'",
                node_binary,
                helper_path.display()
            )
        })?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(&serialized_request)
            .await
            .context("failed to send request to Claude SDK helper")?;
    }

    let output = child
        .wait_with_output()
        .await
        .context("failed to wait for Claude SDK helper process")?;
    let stdout = String::from_utf8(output.stdout).context("helper stdout is not valid UTF-8")?;
    let stderr = String::from_utf8(output.stderr).context("helper stderr is not valid UTF-8")?;

    if !output.status.success() {
        let detail = if stderr.trim().is_empty() {
            "helper exited with a non-zero status".to_string()
        } else {
            stderr.trim().to_string()
        };
        return Err(anyhow!("Claude SDK helper failed: {detail}"));
    }

    T::parse_response(stdout.trim(), stderr.trim())
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
            "changeType": { "type": "string" },
            "objectives": {
                "type": "array",
                "minItems": 1,
                "items": { "type": "string" }
            },
            "inScope": {
                "type": "array",
                "items": { "type": "string" }
            },
            "outOfScope": {
                "type": "array",
                "items": { "type": "string" }
            },
            "touchHints": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "files": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "symbols": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "apis": {
                        "type": "array",
                        "items": { "type": "string" }
                    }
                }
            },
            "successCriteria": {
                "type": "array",
                "minItems": 1,
                "items": { "type": "string" }
            },
            "fastChecks": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["id", "kind"],
                    "properties": {
                        "id": { "type": "string" },
                        "kind": {
                            "type": "string",
                            "enum": ["command", "testSuite", "policy"]
                        },
                        "command": { "type": "string" },
                        "timeoutSeconds": { "type": "integer", "minimum": 1 },
                        "expectedExitCode": { "type": "integer" },
                        "required": { "type": "boolean" },
                        "artifactsProduced": {
                            "type": "array",
                            "items": { "type": "string" }
                        }
                    }
                }
            },
            "integrationChecks": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["id", "kind"],
                    "properties": {
                        "id": { "type": "string" },
                        "kind": {
                            "type": "string",
                            "enum": ["command", "testSuite", "policy"]
                        },
                        "command": { "type": "string" },
                        "timeoutSeconds": { "type": "integer", "minimum": 1 },
                        "expectedExitCode": { "type": "integer" },
                        "required": { "type": "boolean" },
                        "artifactsProduced": {
                            "type": "array",
                            "items": { "type": "string" }
                        }
                    }
                }
            },
            "securityChecks": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["id", "kind"],
                    "properties": {
                        "id": { "type": "string" },
                        "kind": {
                            "type": "string",
                            "enum": ["command", "testSuite", "policy"]
                        },
                        "command": { "type": "string" },
                        "timeoutSeconds": { "type": "integer", "minimum": 1 },
                        "expectedExitCode": { "type": "integer" },
                        "required": { "type": "boolean" },
                        "artifactsProduced": {
                            "type": "array",
                            "items": { "type": "string" }
                        }
                    }
                }
            },
            "releaseChecks": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["id", "kind"],
                    "properties": {
                        "id": { "type": "string" },
                        "kind": {
                            "type": "string",
                            "enum": ["command", "testSuite", "policy"]
                        },
                        "command": { "type": "string" },
                        "timeoutSeconds": { "type": "integer", "minimum": 1 },
                        "expectedExitCode": { "type": "integer" },
                        "required": { "type": "boolean" },
                        "artifactsProduced": {
                            "type": "array",
                            "items": { "type": "string" }
                        }
                    }
                }
            },
            "riskRationale": { "type": "string" },
            "riskFactors": {
                "type": "array",
                "items": { "type": "string" }
            },
            "riskLevel": {
                "type": "string",
                "enum": ["low", "medium", "high"]
            }
        }
    })
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
            .context("failed to serialize Claude SDK command output")?
    );
    Ok(())
}
