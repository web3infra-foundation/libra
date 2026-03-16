//! Claude Agent SDK managed-mode command surface.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use chrono::Utc;
use clap::{Args, Parser, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{fs, io::AsyncWriteExt, process::Command};

use crate::{
    internal::{
        ai::{
            history::HistoryManager,
            intentspec::{
                IntentDraft, ResolveContext, RiskLevel, persist_intentspec, render_summary,
                repair_intentspec, resolve_intentspec, validate_intentspec,
            },
            mcp::server::LibraMcpServer,
            providers::claude_sdk::managed::{
                ClaudeManagedArtifact, PersistedManagedArtifactOutcome, persist_managed_artifact,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Serialize, Deserialize)]
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

#[derive(Debug, Serialize, Deserialize)]
struct EvidenceInputSourceArtifacts {
    #[serde(rename = "providerSessionPath")]
    provider_session_path: String,
    #[serde(rename = "messagesPath")]
    messages_path: String,
}

#[derive(Debug, Serialize, Deserialize)]
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

#[derive(Debug, Serialize, Deserialize)]
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

#[derive(Debug, Serialize, Deserialize)]
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

#[derive(Debug, Serialize, Deserialize)]
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

#[derive(Debug, Serialize)]
struct PersistedIntentInputBindingArtifact {
    schema: &'static str,
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

#[derive(Debug)]
struct EmbeddedHelperDir {
    path: PathBuf,
}

impl Drop for EmbeddedHelperDir {
    fn drop(&mut self) {
        if let Err(err) = std::fs::remove_dir_all(&self.path)
            && err.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(
                path = %self.path.display(),
                error = %err,
                "failed to remove temporary Claude SDK helper directory"
            );
        }
    }
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
        synced.push(
            persist_provider_session_snapshot(&storage_path, &cwd, session)
                .await
                .context("failed to persist Claude provider session snapshot")?,
        );
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
    let object_id = build_provider_session_object_id(&args.provider_session_id);
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
    let sync_record = upsert_provider_session_snapshot(&storage_path, &cwd, &snapshot).await?;

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
    let working_dir =
        util::try_working_dir().context("failed to resolve repository working directory")?;
    let provider_session_object_id = build_provider_session_object_id(&args.provider_session_id);
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

    let object_id = build_evidence_input_object_id(&args.provider_session_id);
    let captured_at = Utc::now().to_rfc3339();
    let artifact = build_evidence_input_artifact(
        &snapshot,
        &provider_session_path,
        &messages_artifact,
        &messages_path,
        object_id,
        captured_at,
    );
    let artifact_path = args
        .output
        .unwrap_or_else(|| evidence_input_artifact_path(&storage_path, &artifact.object_id));
    let record = persist_evidence_input_artifact(&artifact_path, &working_dir, &artifact).await?;

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

    let working_dir =
        util::try_working_dir().context("failed to resolve repository working directory")?;
    let mcp_server = init_local_mcp_server(&working_dir).await?;
    let intent_id = persist_intentspec(&resolved.intentspec, mcp_server.as_ref()).await?;

    let binding_artifact = PersistedIntentInputBindingArtifact {
        schema: "libra.intent_input_binding.v1",
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
        (None, Some(ai_session_id)) => Ok(storage_path
            .join(INTENT_EXTRACTIONS_DIR)
            .join(format!("{ai_session_id}.json"))),
        (Some(_), Some(_)) => bail!("pass either --extraction or --ai-session-id, not both"),
        (None, None) => bail!("missing extraction input; pass --extraction or --ai-session-id"),
    }
}

fn resolve_resolution_path(storage_path: &Path, args: &PersistIntentArgs) -> Result<PathBuf> {
    match (&args.resolution, &args.ai_session_id) {
        (Some(path), None) => Ok(path.clone()),
        (None, Some(ai_session_id)) => Ok(storage_path
            .join(INTENT_RESOLUTIONS_DIR)
            .join(format!("{ai_session_id}.json"))),
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
        .unwrap_or_else(|| "HEAD".to_string())
}

async fn init_local_mcp_server(working_dir: &Path) -> Result<Arc<LibraMcpServer>> {
    let storage_dir = util::try_get_storage_path(Some(working_dir.to_path_buf()))
        .unwrap_or_else(|_| working_dir.join(".libra"));
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
    let history_manager = Arc::new(HistoryManager::new(storage.clone(), storage_dir, db_conn));

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

fn build_provider_session_object_id(provider_session_id: &str) -> String {
    format!("claude_provider_session__{provider_session_id}")
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

fn build_evidence_input_object_id(provider_session_id: &str) -> String {
    format!("claude_evidence_input__{provider_session_id}")
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
            Some("task_started" | "task_progress" | "task_notification") => {
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
    working_dir: &Path,
    session: ClaudeSdkSessionInfo,
) -> Result<SyncSessionRecord> {
    let snapshot = PersistedProviderSessionSnapshot {
        schema: "libra.provider_session.v3".to_string(),
        object_type: "provider_session".to_string(),
        provider: "claude".to_string(),
        object_id: build_provider_session_object_id(&session.session_id),
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
        message_sync: None,
    };

    upsert_provider_session_snapshot(storage_path, working_dir, &snapshot).await
}

async fn upsert_provider_session_snapshot(
    storage_path: &Path,
    working_dir: &Path,
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
    let mcp_server = init_local_mcp_server(working_dir).await?;
    let history = mcp_server
        .intent_history_manager
        .as_ref()
        .ok_or_else(|| anyhow!("local MCP history manager is unavailable"))?;
    history
        .append("provider_session", &snapshot.object_id, object_hash)
        .await
        .context("failed to append provider session snapshot to history")?;

    Ok(SyncSessionRecord {
        provider_session_id: snapshot.provider_session_id.clone(),
        object_id: snapshot.object_id.clone(),
        artifact_path: artifact_path.to_string_lossy().to_string(),
        object_hash: object_hash.to_string(),
    })
}

async fn persist_evidence_input_artifact(
    artifact_path: &Path,
    working_dir: &Path,
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
    let object_hash = write_git_object(
        &util::try_get_storage_path(Some(working_dir.to_path_buf()))?,
        "blob",
        &payload,
    )
    .context("failed to write evidence input object")?;
    let mcp_server = init_local_mcp_server(working_dir).await?;
    let history = mcp_server
        .intent_history_manager
        .as_ref()
        .ok_or_else(|| anyhow!("local MCP history manager is unavailable"))?;
    history
        .append("evidence_input", &artifact.object_id, object_hash)
        .await
        .context("failed to append evidence input to history")?;

    Ok(SyncSessionRecord {
        provider_session_id: artifact.provider_session_id.clone(),
        object_id: artifact.object_id.clone(),
        artifact_path: artifact_path.to_string_lossy().to_string(),
        object_hash: object_hash.to_string(),
    })
}

async fn materialize_helper(
    helper_path: Option<&Path>,
) -> Result<(Option<EmbeddedHelperDir>, PathBuf)> {
    if let Some(path) = helper_path {
        return Ok((None, path.to_path_buf()));
    }

    let unique_suffix = format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let temp_dir_path =
        std::env::temp_dir().join(format!("libra-claude-sdk-helper-{unique_suffix}"));
    fs::create_dir_all(&temp_dir_path).await.with_context(|| {
        format!(
            "failed to create temporary helper directory '{}'",
            temp_dir_path.display()
        )
    })?;
    let helper_path = temp_dir_path.join("libra-claude-managed-helper.cjs");
    fs::write(&helper_path, EMBEDDED_HELPER_SOURCE)
        .await
        .with_context(|| format!("failed to write helper '{}'", helper_path.display()))?;
    Ok((
        Some(EmbeddedHelperDir {
            path: temp_dir_path,
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
