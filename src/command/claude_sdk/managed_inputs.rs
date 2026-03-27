use super::*;

#[derive(Args, Debug)]
pub(super) struct BuildManagedEvidenceInputArgs {
    #[arg(
        long,
        help = "Claude SDK ai_session_id to derive a managed evidence input from"
    )]
    pub(super) ai_session_id: String,
    #[arg(
        long,
        help = "Optional output path for the managed evidence input artifact; defaults to .libra/managed-evidence-inputs/<object-id>.json"
    )]
    pub(super) output: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub(super) struct BuildDecisionInputArgs {
    #[arg(
        long,
        help = "Claude SDK ai_session_id to derive a decision input from"
    )]
    pub(super) ai_session_id: String,
    #[arg(
        long,
        help = "Optional output path for the decision input artifact; defaults to .libra/decision-inputs/<object-id>.json"
    )]
    pub(super) output: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
struct BuildManagedEvidenceInputCommandOutput {
    ok: bool,
    #[serde(rename = "mode")]
    command_mode: &'static str,
    #[serde(rename = "aiSessionId")]
    ai_session_id: String,
    #[serde(rename = "providerSessionId")]
    provider_session_id: String,
    #[serde(rename = "objectId")]
    object_id: String,
    #[serde(rename = "artifactPath")]
    artifact_path: String,
    #[serde(rename = "objectHash")]
    object_hash: String,
}

#[derive(Debug, Serialize)]
struct BuildDecisionInputCommandOutput {
    ok: bool,
    #[serde(rename = "mode")]
    command_mode: &'static str,
    #[serde(rename = "aiSessionId")]
    ai_session_id: String,
    #[serde(rename = "providerSessionId")]
    provider_session_id: String,
    #[serde(rename = "objectId")]
    object_id: String,
    #[serde(rename = "artifactPath")]
    artifact_path: String,
    #[serde(rename = "objectHash")]
    object_hash: String,
}

#[derive(Debug)]
pub(super) struct ManagedInputArtifactRecord {
    pub(super) ai_session_id: String,
    pub(super) provider_session_id: String,
    pub(super) object_id: String,
    pub(super) artifact_path: String,
    pub(super) object_hash: String,
}

pub(super) struct ManagedEvidenceInputBuildContext<'a> {
    pub(super) ai_session_id: &'a str,
    pub(super) raw_artifact_path: &'a Path,
    pub(super) audit_bundle_path: &'a Path,
    pub(super) provider_session_path: Option<&'a Path>,
    pub(super) provider_evidence_input_path: Option<&'a Path>,
    pub(super) captured_at: String,
}

pub(super) async fn build_managed_evidence_input(
    args: BuildManagedEvidenceInputArgs,
) -> Result<()> {
    let storage_path = util::try_get_storage_path(None)
        .context("claude-sdk commands must be run inside a Libra repository")?;
    validate_ai_session_id(&args.ai_session_id)?;

    let raw_artifact_path = managed_artifact_path(&storage_path, &args.ai_session_id);
    let (audit_bundle_path, audit_bundle) =
        load_managed_audit_bundle_for_ai_session(&storage_path, &args.ai_session_id).await?;
    let provider_session_object_id =
        build_provider_session_object_id(&audit_bundle.provider_session_id)?;
    let provider_session_path =
        provider_session_artifact_path(&storage_path, &provider_session_object_id);
    let provider_evidence_input_object_id =
        build_evidence_input_object_id(&audit_bundle.provider_session_id)?;
    let provider_evidence_input_path =
        evidence_input_artifact_path(&storage_path, &provider_evidence_input_object_id);

    let object_id = build_managed_evidence_input_object_id(&args.ai_session_id)?;
    let default_artifact_path = managed_evidence_input_artifact_path(&storage_path, &object_id);
    let comparison_path = args.output.as_deref().unwrap_or(&default_artifact_path);
    let mut artifact = build_managed_evidence_input_artifact(
        &audit_bundle,
        ManagedEvidenceInputBuildContext {
            ai_session_id: &args.ai_session_id,
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
        object_id,
    );
    if let Some(existing_artifact) =
        read_existing_managed_evidence_input_artifact(comparison_path).await?
        && managed_evidence_input_artifact_matches(&existing_artifact, &artifact)
    {
        artifact.captured_at = existing_artifact.captured_at;
    }
    let artifact_path = args.output.unwrap_or(default_artifact_path);
    let record =
        persist_managed_evidence_input_artifact(&storage_path, &artifact_path, &artifact).await?;

    println!(
        "{}",
        serde_json::to_string_pretty(&BuildManagedEvidenceInputCommandOutput {
            ok: true,
            command_mode: "build-managed-evidence-input",
            ai_session_id: record.ai_session_id,
            provider_session_id: record.provider_session_id,
            object_id: record.object_id,
            artifact_path: record.artifact_path,
            object_hash: record.object_hash,
        })
        .context("failed to serialize build-managed-evidence-input output")?
    );

    Ok(())
}

pub(super) async fn build_decision_input(args: BuildDecisionInputArgs) -> Result<()> {
    let storage_path = util::try_get_storage_path(None)
        .context("claude-sdk commands must be run inside a Libra repository")?;
    validate_ai_session_id(&args.ai_session_id)?;

    let (audit_bundle_path, audit_bundle) =
        load_managed_audit_bundle_for_ai_session(&storage_path, &args.ai_session_id).await?;
    let managed_evidence_input_object_id =
        build_managed_evidence_input_object_id(&args.ai_session_id)?;
    let managed_evidence_input_path =
        managed_evidence_input_artifact_path(&storage_path, &managed_evidence_input_object_id);

    let object_id = build_decision_input_object_id(&args.ai_session_id)?;
    let default_artifact_path = decision_input_artifact_path(&storage_path, &object_id);
    let comparison_path = args.output.as_deref().unwrap_or(&default_artifact_path);
    let mut artifact = build_decision_input_artifact(
        &args.ai_session_id,
        &audit_bundle_path,
        &audit_bundle,
        managed_evidence_input_path
            .exists()
            .then_some(managed_evidence_input_path.as_path()),
        object_id,
        Utc::now().to_rfc3339(),
    );
    if let Some(existing_artifact) = read_existing_decision_input_artifact(comparison_path).await?
        && decision_input_artifact_matches(&existing_artifact, &artifact)
    {
        artifact.captured_at = existing_artifact.captured_at;
    }
    let artifact_path = args.output.unwrap_or(default_artifact_path);
    let record = persist_decision_input_artifact(&storage_path, &artifact_path, &artifact).await?;

    println!(
        "{}",
        serde_json::to_string_pretty(&BuildDecisionInputCommandOutput {
            ok: true,
            command_mode: "build-decision-input",
            ai_session_id: record.ai_session_id,
            provider_session_id: record.provider_session_id,
            object_id: record.object_id,
            artifact_path: record.artifact_path,
            object_hash: record.object_hash,
        })
        .context("failed to serialize build-decision-input output")?
    );

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct PersistedManagedEvidenceInputArtifact {
    pub(super) schema: String,
    pub(super) object_type: String,
    pub(super) provider: String,
    #[serde(rename = "objectId")]
    pub(super) object_id: String,
    #[serde(rename = "aiSessionId")]
    pub(super) ai_session_id: String,
    #[serde(rename = "providerSessionId")]
    pub(super) provider_session_id: String,
    pub(super) summary: String,
    #[serde(rename = "sourceArtifacts")]
    pub(super) source_artifacts: ManagedEvidenceInputSourceArtifacts,
    #[serde(rename = "patchOverview")]
    pub(super) patch_overview: ManagedEvidencePatchOverview,
    #[serde(rename = "runtimeOverview")]
    pub(super) runtime_overview: ManagedEvidenceRuntimeOverview,
    #[serde(rename = "capturedAt")]
    pub(super) captured_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct ManagedEvidenceInputSourceArtifacts {
    #[serde(rename = "rawArtifactPath")]
    pub(super) raw_artifact_path: String,
    #[serde(rename = "auditBundlePath")]
    pub(super) audit_bundle_path: String,
    #[serde(
        rename = "providerSessionPath",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub(super) provider_session_path: Option<String>,
    #[serde(
        rename = "providerEvidenceInputPath",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub(super) provider_evidence_input_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct ManagedEvidencePatchOverview {
    #[serde(rename = "touchedFiles")]
    pub(super) touched_files: Vec<String>,
    #[serde(rename = "observedTools")]
    pub(super) observed_tools: BTreeMap<String, usize>,
    #[serde(rename = "filesPersisted")]
    pub(super) files_persisted: Vec<ManagedPersistedFile>,
    #[serde(rename = "failedFilesPersisted")]
    pub(super) failed_files_persisted: Vec<ManagedFailedPersistedFile>,
    #[serde(rename = "checkpointingEnabled")]
    pub(super) checkpointing_enabled: bool,
    #[serde(rename = "rewindSupported")]
    pub(super) rewind_supported: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct ManagedPersistedFile {
    pub(super) filename: String,
    #[serde(rename = "fileId")]
    pub(super) file_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct ManagedFailedPersistedFile {
    pub(super) filename: String,
    pub(super) error: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct ManagedEvidenceRuntimeOverview {
    #[serde(rename = "toolInvocationCount")]
    pub(super) tool_invocation_count: usize,
    #[serde(rename = "toolRuntimeCount")]
    pub(super) tool_runtime_count: usize,
    #[serde(rename = "assistantRuntimeCount")]
    pub(super) assistant_runtime_count: usize,
    #[serde(rename = "taskRuntimeCount")]
    pub(super) task_runtime_count: usize,
    #[serde(rename = "decisionRuntimeCount")]
    pub(super) decision_runtime_count: usize,
    #[serde(rename = "contextRuntimeCount")]
    pub(super) context_runtime_count: usize,
    #[serde(rename = "hasStructuredOutput")]
    pub(super) has_structured_output: bool,
    #[serde(rename = "hasPermissionDenials")]
    pub(super) has_permission_denials: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct PersistedDecisionInputArtifact {
    pub(super) schema: String,
    pub(super) object_type: String,
    pub(super) provider: String,
    #[serde(rename = "objectId")]
    pub(super) object_id: String,
    #[serde(rename = "aiSessionId")]
    pub(super) ai_session_id: String,
    #[serde(rename = "providerSessionId")]
    pub(super) provider_session_id: String,
    pub(super) summary: String,
    #[serde(rename = "sourceArtifacts")]
    pub(super) source_artifacts: DecisionInputSourceArtifacts,
    #[serde(rename = "decisionOverview")]
    pub(super) decision_overview: DecisionInputOverview,
    pub(super) signals: Vec<DecisionInputSignal>,
    #[serde(rename = "capturedAt")]
    pub(super) captured_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct DecisionInputSourceArtifacts {
    #[serde(rename = "auditBundlePath")]
    pub(super) audit_bundle_path: String,
    #[serde(
        rename = "managedEvidenceInputPath",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub(super) managed_evidence_input_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct DecisionInputOverview {
    #[serde(rename = "runtimeEventCount")]
    pub(super) runtime_event_count: usize,
    #[serde(rename = "permissionRequestCount")]
    pub(super) permission_request_count: usize,
    #[serde(rename = "canUseToolCount")]
    pub(super) can_use_tool_count: usize,
    #[serde(rename = "elicitationCount")]
    pub(super) elicitation_count: usize,
    #[serde(rename = "elicitationResultCount")]
    pub(super) elicitation_result_count: usize,
    #[serde(rename = "permissionDenialCount")]
    pub(super) permission_denial_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct DecisionInputSignal {
    pub(super) id: String,
    pub(super) kind: String,
    pub(super) source: String,
    #[serde(
        rename = "interactionKind",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub(super) interaction_kind: Option<String>,
    #[serde(
        rename = "approvalDecision",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub(super) approval_decision: Option<String>,
    #[serde(
        rename = "approvalScope",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub(super) approval_scope: Option<String>,
    #[serde(
        rename = "promptSource",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub(super) prompt_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) cached: Option<bool>,
    #[serde(
        rename = "questionCount",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub(super) question_count: Option<usize>,
    #[serde(
        rename = "answerCount",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub(super) answer_count: Option<usize>,
    #[serde(rename = "answerKeys", default, skip_serializing_if = "Vec::is_empty")]
    pub(super) answer_keys: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) answers: Option<Value>,
    #[serde(
        rename = "suggestedMode",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub(super) suggested_mode: Option<String>,
    #[serde(
        rename = "currentPermissionMode",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub(super) current_permission_mode: Option<String>,
    #[serde(
        rename = "previousPermissionMode",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub(super) previous_permission_mode: Option<String>,
    #[serde(
        rename = "permissionSuggestionCount",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub(super) permission_suggestion_count: Option<usize>,
    #[serde(rename = "toolName", default, skip_serializing_if = "Option::is_none")]
    pub(super) tool_name: Option<String>,
    #[serde(
        rename = "blockedPath",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub(super) blocked_path: Option<String>,
    #[serde(
        rename = "decisionReason",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub(super) decision_reason: Option<String>,
    #[serde(
        rename = "mcpServerName",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub(super) mcp_server_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) action: Option<String>,
    #[serde(
        rename = "permissionDenialCount",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub(super) permission_denial_count: Option<usize>,
}

pub(super) async fn read_existing_managed_evidence_input_artifact(
    path: &Path,
) -> Result<Option<PersistedManagedEvidenceInputArtifact>> {
    if !path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(path).await.with_context(|| {
        format!(
            "failed to read managed evidence input artifact '{}'",
            path.display()
        )
    })?;
    let artifact = serde_json::from_str(&content).with_context(|| {
        format!(
            "failed to parse managed evidence input artifact '{}'",
            path.display()
        )
    })?;
    Ok(Some(artifact))
}

pub(super) async fn read_existing_decision_input_artifact(
    path: &Path,
) -> Result<Option<PersistedDecisionInputArtifact>> {
    if !path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(path).await.with_context(|| {
        format!(
            "failed to read decision input artifact '{}'",
            path.display()
        )
    })?;
    let artifact = serde_json::from_str(&content).with_context(|| {
        format!(
            "failed to parse decision input artifact '{}'",
            path.display()
        )
    })?;
    Ok(Some(artifact))
}

pub(super) fn managed_evidence_input_artifact_matches(
    existing: &PersistedManagedEvidenceInputArtifact,
    candidate: &PersistedManagedEvidenceInputArtifact,
) -> bool {
    existing.schema == candidate.schema
        && existing.object_type == candidate.object_type
        && existing.provider == candidate.provider
        && existing.object_id == candidate.object_id
        && existing.ai_session_id == candidate.ai_session_id
        && existing.provider_session_id == candidate.provider_session_id
        && existing.summary == candidate.summary
        && existing.source_artifacts == candidate.source_artifacts
        && existing.patch_overview == candidate.patch_overview
        && existing.runtime_overview == candidate.runtime_overview
}

pub(super) fn decision_input_artifact_matches(
    existing: &PersistedDecisionInputArtifact,
    candidate: &PersistedDecisionInputArtifact,
) -> bool {
    existing.schema == candidate.schema
        && existing.object_type == candidate.object_type
        && existing.provider == candidate.provider
        && existing.object_id == candidate.object_id
        && existing.ai_session_id == candidate.ai_session_id
        && existing.provider_session_id == candidate.provider_session_id
        && existing.summary == candidate.summary
        && existing.source_artifacts == candidate.source_artifacts
        && existing.decision_overview == candidate.decision_overview
        && existing.signals == candidate.signals
}

pub(super) fn build_managed_evidence_input_artifact(
    audit_bundle: &ManagedAuditBundle,
    context: ManagedEvidenceInputBuildContext<'_>,
    object_id: String,
) -> PersistedManagedEvidenceInputArtifact {
    let observed_tools = audit_bundle
        .bridge
        .tool_invocations
        .iter()
        .filter_map(|invocation| invocation.tool_name.clone())
        .fold(BTreeMap::new(), |mut acc, tool_name| {
            *acc.entry(tool_name).or_insert(0) += 1;
            acc
        });
    let (files_persisted, failed_files_persisted) = extract_files_persisted_facts(
        &audit_bundle.bridge.object_candidates.context_runtime_events,
    );
    let checkpointing_enabled = audit_bundle
        .raw_artifact
        .request_context
        .as_ref()
        .map(|context| context.enable_file_checkpointing)
        .unwrap_or(false);
    let runtime_overview = ManagedEvidenceRuntimeOverview {
        tool_invocation_count: audit_bundle
            .bridge
            .object_candidates
            .tool_invocation_events
            .len(),
        tool_runtime_count: audit_bundle
            .bridge
            .object_candidates
            .tool_runtime_events
            .len(),
        assistant_runtime_count: audit_bundle
            .bridge
            .object_candidates
            .assistant_runtime_events
            .len(),
        task_runtime_count: audit_bundle
            .bridge
            .object_candidates
            .task_runtime_events
            .len(),
        decision_runtime_count: audit_bundle
            .bridge
            .object_candidates
            .decision_runtime_events
            .len(),
        context_runtime_count: audit_bundle
            .bridge
            .object_candidates
            .context_runtime_events
            .len(),
        has_structured_output: audit_bundle
            .raw_artifact
            .result_message
            .as_ref()
            .and_then(|result| result.structured_output.as_ref())
            .is_some(),
        has_permission_denials: audit_bundle
            .raw_artifact
            .result_message
            .as_ref()
            .and_then(|result| result.permission_denials.as_ref())
            .and_then(Value::as_array)
            .is_some_and(|items| !items.is_empty()),
    };

    PersistedManagedEvidenceInputArtifact {
        schema: "libra.claude_managed_evidence_input.v1".to_string(),
        object_type: "claude_managed_evidence_input".to_string(),
        provider: "claude".to_string(),
        object_id,
        ai_session_id: context.ai_session_id.to_string(),
        provider_session_id: audit_bundle.provider_session_id.clone(),
        summary: summarize_managed_evidence_input(
            &audit_bundle.bridge.touch_hints,
            &observed_tools,
            &files_persisted,
            failed_files_persisted.len(),
            checkpointing_enabled,
        ),
        source_artifacts: ManagedEvidenceInputSourceArtifacts {
            raw_artifact_path: context.raw_artifact_path.to_string_lossy().to_string(),
            audit_bundle_path: context.audit_bundle_path.to_string_lossy().to_string(),
            provider_session_path: context
                .provider_session_path
                .map(|path| path.to_string_lossy().to_string()),
            provider_evidence_input_path: context
                .provider_evidence_input_path
                .map(|path| path.to_string_lossy().to_string()),
        },
        patch_overview: ManagedEvidencePatchOverview {
            touched_files: audit_bundle.bridge.touch_hints.clone(),
            observed_tools,
            files_persisted,
            failed_files_persisted,
            checkpointing_enabled,
            rewind_supported: checkpointing_enabled,
        },
        runtime_overview,
        captured_at: context.captured_at,
    }
}

pub(super) fn build_decision_input_artifact(
    ai_session_id: &str,
    audit_bundle_path: &Path,
    audit_bundle: &ManagedAuditBundle,
    managed_evidence_input_path: Option<&Path>,
    object_id: String,
    captured_at: String,
) -> PersistedDecisionInputArtifact {
    let signals = collect_decision_input_signals(
        &audit_bundle
            .bridge
            .object_candidates
            .decision_runtime_events,
    );
    let decision_overview = summarize_decision_input_overview(&signals);
    PersistedDecisionInputArtifact {
        schema: "libra.claude_decision_input.v1".to_string(),
        object_type: "claude_decision_input".to_string(),
        provider: "claude".to_string(),
        object_id,
        ai_session_id: ai_session_id.to_string(),
        provider_session_id: audit_bundle.provider_session_id.clone(),
        summary: summarize_decision_input(&decision_overview),
        source_artifacts: DecisionInputSourceArtifacts {
            audit_bundle_path: audit_bundle_path.to_string_lossy().to_string(),
            managed_evidence_input_path: managed_evidence_input_path
                .map(|path| path.to_string_lossy().to_string()),
        },
        decision_overview,
        signals,
        captured_at,
    }
}

fn extract_files_persisted_facts(
    events: &[ManagedSemanticRuntimeEvent],
) -> (Vec<ManagedPersistedFile>, Vec<ManagedFailedPersistedFile>) {
    let mut files = Vec::new();
    let mut failed = Vec::new();
    let mut seen_files = BTreeSet::new();
    let mut seen_failed = BTreeSet::new();

    for event in events {
        if event.kind != "files_persisted" {
            continue;
        }

        if let Some(items) = event.payload.get("files").and_then(Value::as_array) {
            for item in items {
                let Some(filename) = item.get("filename").and_then(Value::as_str) else {
                    continue;
                };
                let Some(file_id) = item.get("file_id").and_then(Value::as_str) else {
                    continue;
                };
                if seen_files.insert((filename.to_string(), file_id.to_string())) {
                    files.push(ManagedPersistedFile {
                        filename: filename.to_string(),
                        file_id: file_id.to_string(),
                    });
                }
            }
        }

        if let Some(items) = event.payload.get("failed").and_then(Value::as_array) {
            for item in items {
                let Some(filename) = item.get("filename").and_then(Value::as_str) else {
                    continue;
                };
                let Some(error) = item.get("error").and_then(Value::as_str) else {
                    continue;
                };
                if seen_failed.insert((filename.to_string(), error.to_string())) {
                    failed.push(ManagedFailedPersistedFile {
                        filename: filename.to_string(),
                        error: error.to_string(),
                    });
                }
            }
        }
    }

    (files, failed)
}

fn collect_decision_input_signals(
    events: &[ManagedSemanticRuntimeEvent],
) -> Vec<DecisionInputSignal> {
    events
        .iter()
        .map(|event| DecisionInputSignal {
            id: event.id.clone(),
            kind: event.kind.clone(),
            source: event.source.clone(),
            interaction_kind: event
                .payload
                .get("interaction_kind")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            approval_decision: event
                .payload
                .get("approval_decision")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            approval_scope: event
                .payload
                .get("approval_scope")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            prompt_source: event
                .payload
                .get("prompt_source")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            cached: event.payload.get("cached").and_then(Value::as_bool),
            question_count: event
                .payload
                .get("question_count")
                .and_then(Value::as_u64)
                .map(|value| value as usize),
            answer_count: event
                .payload
                .get("answer_count")
                .and_then(Value::as_u64)
                .map(|value| value as usize),
            answer_keys: event
                .payload
                .get("answer_keys")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
            answers: event.payload.get("answers").cloned(),
            suggested_mode: event
                .payload
                .get("suggested_mode")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            current_permission_mode: event
                .payload
                .get("current_permission_mode")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            previous_permission_mode: event
                .payload
                .get("previous_permission_mode")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            permission_suggestion_count: event
                .payload
                .get("permission_suggestion_count")
                .and_then(Value::as_u64)
                .map(|value| value as usize),
            tool_name: event
                .payload
                .get("tool_name")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            blocked_path: event
                .payload
                .get("blocked_path")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            decision_reason: event
                .payload
                .get("decision_reason")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            mcp_server_name: event
                .payload
                .get("mcp_server_name")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            mode: event
                .payload
                .get("mode")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            action: event
                .payload
                .get("action")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            permission_denial_count: (event.kind == "permission_denials")
                .then(|| event.payload.as_array().map_or(0, Vec::len)),
        })
        .collect()
}

fn summarize_decision_input_overview(signals: &[DecisionInputSignal]) -> DecisionInputOverview {
    DecisionInputOverview {
        runtime_event_count: signals.len(),
        permission_request_count: signals
            .iter()
            .filter(|signal| signal.kind == "PermissionRequest")
            .count(),
        can_use_tool_count: signals
            .iter()
            .filter(|signal| signal.kind == "CanUseTool")
            .count(),
        elicitation_count: signals
            .iter()
            .filter(|signal| signal.kind == "Elicitation")
            .count(),
        elicitation_result_count: signals
            .iter()
            .filter(|signal| signal.kind == "ElicitationResult")
            .count(),
        permission_denial_count: signals
            .iter()
            .filter_map(|signal| signal.permission_denial_count)
            .sum(),
    }
}

fn summarize_managed_evidence_input(
    touched_files: &[String],
    observed_tools: &BTreeMap<String, usize>,
    files_persisted: &[ManagedPersistedFile],
    failed_persisted_count: usize,
    checkpointing_enabled: bool,
) -> String {
    format!(
        "touched_files={}; observed_tools={}; files_persisted={}; files_persist_failed={}; checkpointing_enabled={}",
        touched_files.len(),
        observed_tools.len(),
        files_persisted.len(),
        failed_persisted_count,
        checkpointing_enabled
    )
}

fn summarize_decision_input(overview: &DecisionInputOverview) -> String {
    format!(
        "decision_runtime_events={}; permission_requests={}; can_use_tool={}; elicitations={}; elicitation_results={}; permission_denials={}",
        overview.runtime_event_count,
        overview.permission_request_count,
        overview.can_use_tool_count,
        overview.elicitation_count,
        overview.elicitation_result_count,
        overview.permission_denial_count,
    )
}

pub(super) async fn persist_managed_evidence_input_artifact(
    storage_path: &Path,
    artifact_path: &Path,
    artifact: &PersistedManagedEvidenceInputArtifact,
) -> Result<ManagedInputArtifactRecord> {
    write_pretty_json_file(artifact_path, artifact)
        .await
        .with_context(|| {
            format!(
                "failed to write managed evidence input artifact '{}'",
                artifact_path.display()
            )
        })?;

    let payload = serde_json::to_vec_pretty(artifact)
        .context("failed to serialize managed evidence input artifact")?;
    let object_hash = write_git_object(storage_path, "blob", &payload)
        .context("failed to write managed evidence input object")?;
    let mcp_server = init_local_mcp_server(storage_path).await?;
    let history = mcp_server
        .intent_history_manager
        .as_ref()
        .ok_or_else(|| anyhow!("local MCP history manager is unavailable"))?;
    let existing_hash = history
        .get_object_hash("claude_managed_evidence_input", &artifact.object_id)
        .await
        .context("failed to inspect existing managed evidence input history")?;
    if existing_hash != Some(object_hash) {
        history
            .append(
                "claude_managed_evidence_input",
                &artifact.object_id,
                object_hash,
            )
            .await
            .context("failed to append managed evidence input to history")?;
    }

    Ok(ManagedInputArtifactRecord {
        ai_session_id: artifact.ai_session_id.clone(),
        provider_session_id: artifact.provider_session_id.clone(),
        object_id: artifact.object_id.clone(),
        artifact_path: artifact_path.to_string_lossy().to_string(),
        object_hash: object_hash.to_string(),
    })
}

pub(super) async fn persist_decision_input_artifact(
    storage_path: &Path,
    artifact_path: &Path,
    artifact: &PersistedDecisionInputArtifact,
) -> Result<ManagedInputArtifactRecord> {
    write_pretty_json_file(artifact_path, artifact)
        .await
        .with_context(|| {
            format!(
                "failed to write decision input artifact '{}'",
                artifact_path.display()
            )
        })?;

    let payload = serde_json::to_vec_pretty(artifact)
        .context("failed to serialize decision input artifact")?;
    let object_hash = write_git_object(storage_path, "blob", &payload)
        .context("failed to write decision input object")?;
    let mcp_server = init_local_mcp_server(storage_path).await?;
    let history = mcp_server
        .intent_history_manager
        .as_ref()
        .ok_or_else(|| anyhow!("local MCP history manager is unavailable"))?;
    let existing_hash = history
        .get_object_hash("claude_decision_input", &artifact.object_id)
        .await
        .context("failed to inspect existing decision input history")?;
    if existing_hash != Some(object_hash) {
        history
            .append("claude_decision_input", &artifact.object_id, object_hash)
            .await
            .context("failed to append decision input to history")?;
    }

    Ok(ManagedInputArtifactRecord {
        ai_session_id: artifact.ai_session_id.clone(),
        provider_session_id: artifact.provider_session_id.clone(),
        object_id: artifact.object_id.clone(),
        artifact_path: artifact_path.to_string_lossy().to_string(),
        object_hash: object_hash.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_decision_input_signals_preserves_mode_and_answer_metadata() {
        let events = vec![ManagedSemanticRuntimeEvent {
            id: "event-1".to_string(),
            run_id: "run".to_string(),
            thread_id: "thread".to_string(),
            semantic_object: "Decision".to_string(),
            kind: "CanUseTool".to_string(),
            source: "hook".to_string(),
            at: "2026-03-25T00:00:00Z".to_string(),
            payload: json!({
                "interaction_kind": "ask_user_question",
                "approval_decision": "allow",
                "approval_scope": "session",
                "prompt_source": "interactive_tty",
                "cached": false,
                "question_count": 1,
                "answer_count": 1,
                "answer_keys": ["Which stack should we use?"],
                "answers": {"Which stack should we use?": "Rust"},
                "suggested_mode": "acceptEdits",
                "previous_permission_mode": "default",
                "current_permission_mode": "acceptEdits",
                "permission_suggestion_count": 1,
                "tool_name": "AskUserQuestion"
            }),
        }];

        let signals = collect_decision_input_signals(&events);
        assert_eq!(signals.len(), 1);
        assert_eq!(
            signals[0].interaction_kind.as_deref(),
            Some("ask_user_question")
        );
        assert_eq!(signals[0].approval_decision.as_deref(), Some("allow"));
        assert_eq!(signals[0].approval_scope.as_deref(), Some("session"));
        assert_eq!(signals[0].prompt_source.as_deref(), Some("interactive_tty"));
        assert_eq!(signals[0].question_count, Some(1));
        assert_eq!(signals[0].answer_count, Some(1));
        assert_eq!(
            signals[0].answer_keys,
            vec!["Which stack should we use?".to_string()]
        );
        assert_eq!(
            signals[0].answers,
            Some(json!({"Which stack should we use?": "Rust"}))
        );
        assert_eq!(signals[0].suggested_mode.as_deref(), Some("acceptEdits"));
        assert_eq!(
            signals[0].previous_permission_mode.as_deref(),
            Some("default")
        );
        assert_eq!(
            signals[0].current_permission_mode.as_deref(),
            Some("acceptEdits")
        );
        assert_eq!(signals[0].permission_suggestion_count, Some(1));
    }
}
