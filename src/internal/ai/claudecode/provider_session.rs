#![allow(dead_code)]

use super::*;

#[derive(Args, Debug)]
pub(super) struct SyncSessionsArgs {
    #[arg(long, help = "Working directory used as the Claude SDK project dir")]
    pub(super) cwd: Option<PathBuf>,
    #[arg(
        long,
        help = "Optional provider session id to sync; defaults to all sessions in the project"
    )]
    pub(super) provider_session_id: Option<String>,
    #[arg(long, help = "Maximum number of sessions to request from Claude SDK")]
    pub(super) limit: Option<usize>,
    #[arg(
        long,
        default_value_t = 0,
        help = "Number of sessions to skip before syncing"
    )]
    pub(super) offset: usize,
    #[arg(
        long,
        default_value_t = true,
        action = clap::ArgAction::Set,
        help = "Whether Claude SDK should include sessions from git worktrees when cwd is inside a repo"
    )]
    pub(super) include_worktrees: bool,
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

#[derive(Args, Debug)]
pub(super) struct HydrateSessionArgs {
    #[arg(long, help = "Working directory used as the Claude SDK project dir")]
    pub(super) cwd: Option<PathBuf>,
    #[arg(long, help = "Provider session id to hydrate from Claude SDK")]
    pub(super) provider_session_id: String,
    #[arg(long, help = "Maximum number of session messages to request")]
    pub(super) limit: Option<usize>,
    #[arg(
        long,
        default_value_t = 0,
        help = "Number of session messages to skip before hydrating"
    )]
    pub(super) offset: usize,
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

#[derive(Args, Debug)]
pub(super) struct BuildEvidenceInputArgs {
    #[arg(long, help = "Provider session id to derive an evidence input from")]
    pub(super) provider_session_id: String,
    #[arg(
        long,
        help = "Optional output path for the evidence input artifact; defaults to .libra/evidence-inputs/<object-id>.json"
    )]
    pub(super) output: Option<PathBuf>,
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
pub(super) struct SyncSessionRecord {
    #[serde(rename = "providerSessionId")]
    pub(super) provider_session_id: String,
    #[serde(rename = "objectId")]
    pub(super) object_id: String,
    #[serde(rename = "artifactPath")]
    pub(super) artifact_path: String,
    #[serde(rename = "objectHash")]
    pub(super) object_hash: String,
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
pub(super) struct SessionCatalogHelperRequest {
    pub(super) mode: &'static str,
    pub(super) cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) limit: Option<usize>,
    pub(super) offset: usize,
    #[serde(rename = "includeWorktrees")]
    pub(super) include_worktrees: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct SessionMessagesHelperRequest {
    pub(super) mode: &'static str,
    pub(super) cwd: String,
    #[serde(rename = "providerSessionId")]
    pub(super) provider_session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) limit: Option<usize>,
    pub(super) offset: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct ClaudeSdkSessionInfo {
    #[serde(rename = "sessionId")]
    pub(super) session_id: String,
    pub(super) summary: String,
    #[serde(rename = "lastModified")]
    pub(super) last_modified: i64,
    #[serde(rename = "fileSize", default)]
    pub(super) file_size: Option<u64>,
    #[serde(rename = "customTitle", default)]
    pub(super) custom_title: Option<String>,
    #[serde(rename = "firstPrompt", default)]
    pub(super) first_prompt: Option<String>,
    #[serde(rename = "gitBranch", default)]
    pub(super) git_branch: Option<String>,
    #[serde(default)]
    pub(super) cwd: Option<String>,
    #[serde(default)]
    pub(super) tag: Option<String>,
    #[serde(rename = "createdAt", default)]
    pub(super) created_at: Option<i64>,
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

pub(super) async fn sync_sessions(args: SyncSessionsArgs) -> Result<()> {
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

    let python_binary = resolve_helper_python_binary(&cwd, &args.python_binary);
    ensure_helper_python_environment(args.helper_path.is_some(), &python_binary, &cwd).await?;
    let (_temp_helper_dir, helper_path) = materialize_helper(args.helper_path.as_deref()).await?;
    let custom_helper = args.helper_path.is_some();
    let sessions: Vec<ClaudeSdkSessionInfo> =
        invoke_helper_json(custom_helper, &python_binary, &helper_path, &helper_request)
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

pub(super) async fn hydrate_session(args: HydrateSessionArgs) -> Result<()> {
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
    let python_binary = resolve_helper_python_binary(&cwd, &args.python_binary);
    ensure_helper_python_environment(args.helper_path.is_some(), &python_binary, &cwd).await?;
    let (_temp_helper_dir, helper_path) = materialize_helper(args.helper_path.as_deref()).await?;
    let custom_helper = args.helper_path.is_some();
    let messages: Vec<Value> =
        invoke_helper_json(custom_helper, &python_binary, &helper_path, &helper_request)
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

pub(super) async fn build_evidence_input(args: BuildEvidenceInputArgs) -> Result<()> {
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct PersistedProviderSessionSnapshot {
    pub(super) schema: String,
    pub(super) object_type: String,
    pub(super) provider: String,
    #[serde(rename = "providerSessionId")]
    pub(super) provider_session_id: String,
    #[serde(rename = "objectId")]
    pub(super) object_id: String,
    pub(super) summary: String,
    #[serde(
        rename = "customTitle",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub(super) custom_title: Option<String>,
    #[serde(
        rename = "firstPrompt",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub(super) first_prompt: Option<String>,
    #[serde(rename = "gitBranch", default, skip_serializing_if = "Option::is_none")]
    pub(super) git_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) tag: Option<String>,
    #[serde(rename = "createdAt", default, skip_serializing_if = "Option::is_none")]
    pub(super) created_at: Option<i64>,
    #[serde(rename = "lastModified")]
    pub(super) last_modified: i64,
    #[serde(rename = "fileSize", default, skip_serializing_if = "Option::is_none")]
    pub(super) file_size: Option<u64>,
    #[serde(rename = "capturedAt")]
    pub(super) captured_at: String,
    #[serde(
        rename = "messageSync",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub(super) message_sync: Option<ProviderSessionMessageSync>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct ProviderSessionMessageSync {
    #[serde(rename = "artifactPath")]
    pub(super) artifact_path: String,
    #[serde(rename = "messageCount")]
    pub(super) message_count: usize,
    #[serde(rename = "kindCounts")]
    pub(super) kind_counts: BTreeMap<String, usize>,
    #[serde(rename = "firstMessageKind", skip_serializing_if = "Option::is_none")]
    pub(super) first_message_kind: Option<String>,
    #[serde(rename = "lastMessageKind", skip_serializing_if = "Option::is_none")]
    pub(super) last_message_kind: Option<String>,
    pub(super) offset: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) limit: Option<usize>,
    #[serde(rename = "capturedAt")]
    pub(super) captured_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct ProviderSessionMessagesArtifact {
    pub(super) schema: String,
    #[serde(rename = "providerSessionId")]
    pub(super) provider_session_id: String,
    #[serde(rename = "objectId")]
    pub(super) object_id: String,
    pub(super) offset: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) limit: Option<usize>,
    #[serde(rename = "capturedAt")]
    pub(super) captured_at: String,
    pub(super) messages: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct PersistedEvidenceInputArtifact {
    pub(super) schema: String,
    pub(super) object_type: String,
    pub(super) provider: String,
    #[serde(rename = "objectId")]
    pub(super) object_id: String,
    #[serde(rename = "providerSessionId")]
    pub(super) provider_session_id: String,
    #[serde(rename = "providerSessionObjectId")]
    pub(super) provider_session_object_id: String,
    pub(super) summary: String,
    #[serde(rename = "sourceArtifacts")]
    pub(super) source_artifacts: EvidenceInputSourceArtifacts,
    #[serde(rename = "messageOverview")]
    pub(super) message_overview: EvidenceInputMessageOverview,
    #[serde(rename = "contentOverview")]
    pub(super) content_overview: EvidenceInputContentOverview,
    #[serde(rename = "runtimeSignals")]
    pub(super) runtime_signals: EvidenceInputRuntimeSignals,
    #[serde(rename = "latestResult", skip_serializing_if = "Option::is_none")]
    pub(super) latest_result: Option<EvidenceInputLatestResult>,
    #[serde(rename = "capturedAt")]
    pub(super) captured_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct EvidenceInputSourceArtifacts {
    #[serde(rename = "providerSessionPath")]
    pub(super) provider_session_path: String,
    #[serde(rename = "messagesPath")]
    pub(super) messages_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct EvidenceInputMessageOverview {
    #[serde(rename = "messageCount")]
    pub(super) message_count: usize,
    #[serde(rename = "kindCounts")]
    pub(super) kind_counts: BTreeMap<String, usize>,
    #[serde(rename = "firstMessageKind", skip_serializing_if = "Option::is_none")]
    pub(super) first_message_kind: Option<String>,
    #[serde(rename = "lastMessageKind", skip_serializing_if = "Option::is_none")]
    pub(super) last_message_kind: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct EvidenceInputContentOverview {
    #[serde(rename = "assistantMessageCount")]
    pub(super) assistant_message_count: usize,
    #[serde(rename = "userMessageCount")]
    pub(super) user_message_count: usize,
    #[serde(rename = "observedTools")]
    pub(super) observed_tools: BTreeMap<String, usize>,
    #[serde(rename = "observedPaths")]
    pub(super) observed_paths: Vec<String>,
    #[serde(rename = "assistantTextPreviews")]
    pub(super) assistant_text_previews: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct EvidenceInputRuntimeSignals {
    #[serde(rename = "resultMessageCount")]
    pub(super) result_message_count: usize,
    #[serde(rename = "toolRuntimeCount")]
    pub(super) tool_runtime_count: usize,
    #[serde(rename = "taskRuntimeCount")]
    pub(super) task_runtime_count: usize,
    #[serde(rename = "partialAssistantEventCount")]
    pub(super) partial_assistant_event_count: usize,
    #[serde(rename = "hasStructuredOutput")]
    pub(super) has_structured_output: bool,
    #[serde(rename = "hasPermissionDenials")]
    pub(super) has_permission_denials: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct EvidenceInputLatestResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) subtype: Option<String>,
    #[serde(rename = "stopReason", skip_serializing_if = "Option::is_none")]
    pub(super) stop_reason: Option<String>,
    #[serde(rename = "durationMs", skip_serializing_if = "Option::is_none")]
    pub(super) duration_ms: Option<u64>,
    #[serde(rename = "durationApiMs", skip_serializing_if = "Option::is_none")]
    pub(super) duration_api_ms: Option<u64>,
    #[serde(rename = "totalCostUsd", skip_serializing_if = "Option::is_none")]
    pub(super) total_cost_usd: Option<f64>,
    #[serde(rename = "numTurns", skip_serializing_if = "Option::is_none")]
    pub(super) num_turns: Option<u64>,
    #[serde(rename = "permissionDenialCount")]
    pub(super) permission_denial_count: usize,
}

pub(super) async fn read_persisted_provider_session_snapshot(
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

pub(super) async fn read_existing_evidence_input_artifact(
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

pub(super) fn evidence_input_artifact_matches(
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

pub(super) async fn read_provider_session_messages_artifact(
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

pub(super) fn build_provider_session_message_sync(
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

pub(super) fn build_evidence_input_artifact(
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
                let key = key.as_str();
                if matches!(
                    key,
                    "file_path"
                        | "path"
                        | "cwd"
                        | "worktree_path"
                        | "trigger_file_path"
                        | "parent_file_path"
                ) && let Some(path) = nested.as_str()
                {
                    observed_paths.insert(path.to_string());
                }
                collect_path_candidates(nested, observed_paths);
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

pub(super) async fn persist_provider_session_snapshot(
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

pub(super) async fn upsert_provider_session_snapshot(
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

pub(super) async fn persist_evidence_input_artifact(
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

pub(super) async fn read_existing_provider_session_snapshot(
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
