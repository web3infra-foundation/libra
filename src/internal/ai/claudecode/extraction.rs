#![allow(dead_code)]

use super::*;
use crate::internal::ai::intentspec::IntentSpec;

#[derive(Args, Debug)]
pub(super) struct ResolveExtractionArgs {
    #[arg(long, help = "Path to a persisted intent extraction JSON file")]
    pub(super) extraction: Option<PathBuf>,
    #[arg(
        long,
        help = "Resolve the extraction stored at .libra/intent-extractions/<ai-session-id>.json"
    )]
    pub(super) ai_session_id: Option<String>,
    #[arg(
        long,
        help = "Override risk level (low|medium|high); defaults to extraction risk level or medium"
    )]
    pub(super) risk_level: Option<String>,
    #[arg(
        long,
        default_value = "claudecode",
        help = "createdBy.id used in the resolved IntentSpec preview"
    )]
    pub(super) created_by_id: String,
    #[arg(
        long,
        help = "Optional output path for the resolved artifact; defaults to .libra/intent-resolutions/<ai-session-id>.json"
    )]
    pub(super) output: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub(super) struct PersistIntentArgs {
    #[arg(long, help = "Path to a resolved intent preview JSON file")]
    pub(super) resolution: Option<PathBuf>,
    #[arg(
        long,
        help = "Persist the resolution stored at .libra/intent-resolutions/<ai-session-id>.json"
    )]
    pub(super) ai_session_id: Option<String>,
    #[arg(
        long,
        help = "Optional output path for the persisted-intent binding artifact; defaults to .libra/intent-inputs/<ai-session-id>.json"
    )]
    pub(super) output: Option<PathBuf>,
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

pub(super) struct ResolveExtractionResult {
    pub(super) ai_session_id: Option<String>,
    pub(super) extraction_path: String,
    pub(super) resolved_spec_path: String,
    pub(super) risk_level: String,
    pub(super) summary: String,
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

pub(super) struct PersistIntentResult {
    pub(super) ai_session_id: Option<String>,
    pub(super) resolution_path: String,
    pub(super) intent_id: String,
    pub(super) binding_path: String,
    pub(super) summary: String,
}

#[derive(Debug, Deserialize)]
struct PersistedIntentExtractionArtifact {
    schema: String,
    #[serde(rename = "ai_session_id")]
    ai_session_id: String,
    source: String,
    #[serde(rename = "planningSummary", default)]
    _planning_summary: Option<String>,
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
    intentspec: IntentSpec,
}

#[derive(Debug, Serialize, Deserialize)]
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
    intentspec: IntentSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct PersistedIntentInputBindingArtifact {
    schema: String,
    #[serde(rename = "aiSessionId", skip_serializing_if = "Option::is_none")]
    pub(super) ai_session_id: Option<String>,
    #[serde(rename = "resolutionPath")]
    pub(super) resolution_path: String,
    #[serde(rename = "extractionPath")]
    pub(super) extraction_path: String,
    #[serde(rename = "extractionSource")]
    pub(super) extraction_source: String,
    #[serde(rename = "riskLevel")]
    pub(super) risk_level: RiskLevel,
    #[serde(rename = "intentId")]
    pub(super) intent_id: String,
    pub(super) summary: String,
}

impl BindingArtifactSchema for PersistedIntentInputBindingArtifact {
    const SCHEMA: &'static str = "libra.intent_input_binding.v1";

    fn schema(&self) -> &str {
        &self.schema
    }
}

pub(super) async fn resolve_extraction(args: ResolveExtractionArgs) -> Result<()> {
    let result = resolve_extraction_internal(args).await?;
    let payload = ResolveExtractionCommandOutput {
        ok: true,
        command_mode: "resolve-extraction",
        ai_session_id: result.ai_session_id,
        extraction_path: result.extraction_path,
        resolved_spec_path: result.resolved_spec_path,
        risk_level: result.risk_level,
        summary: result.summary,
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&payload)
            .context("failed to serialize resolve-extraction output")?
    );
    Ok(())
}

pub(super) async fn resolve_extraction_internal(
    args: ResolveExtractionArgs,
) -> Result<ResolveExtractionResult> {
    let storage_path = util::try_get_storage_path(None)
        .context("Claude Code managed commands must be run inside a Libra repository")?;
    let working_dir =
        util::try_working_dir().context("failed to resolve repository working directory")?;
    resolve_extraction_internal_with_paths(&storage_path, &working_dir, args).await
}

pub(super) async fn resolve_extraction_internal_with_paths(
    storage_path: &Path,
    working_dir: &Path,
    args: ResolveExtractionArgs,
) -> Result<ResolveExtractionResult> {
    let extraction_path = resolve_extraction_path(storage_path, &args)?;
    let persisted = read_persisted_extraction(&extraction_path).await?;
    if persisted.schema != "libra.intent_extraction.v1"
        && persisted.schema != "libra.intent_extraction.v2"
    {
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

    Ok(ResolveExtractionResult {
        ai_session_id: Some(persisted.ai_session_id),
        extraction_path: extraction_path.to_string_lossy().to_string(),
        resolved_spec_path: output_path.to_string_lossy().to_string(),
        risk_level: risk_level_label(&risk_level).to_string(),
        summary,
    })
}

pub(super) async fn persist_intent(args: PersistIntentArgs) -> Result<()> {
    let result = persist_intent_internal(args).await?;
    let payload = PersistIntentCommandOutput {
        ok: true,
        command_mode: "persist-intent",
        ai_session_id: result.ai_session_id,
        resolution_path: result.resolution_path,
        intent_id: result.intent_id,
        binding_path: result.binding_path,
        summary: result.summary,
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&payload)
            .context("failed to serialize persist-intent output")?
    );
    Ok(())
}

pub(super) async fn persist_intent_internal(
    args: PersistIntentArgs,
) -> Result<PersistIntentResult> {
    let storage_path = util::try_get_storage_path(None)
        .context("Claude Code managed commands must be run inside a Libra repository")?;
    persist_intent_internal_with_storage(&storage_path, args).await
}

pub(super) async fn persist_intent_internal_with_storage(
    storage_path: &Path,
    args: PersistIntentArgs,
) -> Result<PersistIntentResult> {
    let resolution_path = resolve_resolution_path(storage_path, &args)?;
    let resolved = read_persisted_resolution(&resolution_path).await?;
    if resolved.schema != "libra.intent_resolution.v1" {
        bail!(
            "unsupported resolution schema '{}' in '{}'",
            resolved.schema,
            resolution_path.display()
        );
    }

    let mcp_server = init_local_mcp_server(storage_path).await?;
    let intent_id = persist_intentspec(&resolved.intentspec, mcp_server.as_ref()).await?;
    let persisted_summary = render_summary(&resolved.intentspec, Some(&intent_id));

    let PersistedIntentResolutionArtifact {
        schema,
        ai_session_id,
        extraction_path,
        extraction_source,
        risk_level,
        summary: _,
        intentspec,
    } = resolved;

    let updated_resolution = PersistedIntentResolutionArtifact {
        schema,
        ai_session_id: ai_session_id.clone(),
        extraction_path: extraction_path.clone(),
        extraction_source: extraction_source.clone(),
        risk_level: risk_level.clone(),
        summary: persisted_summary.clone(),
        intentspec,
    };
    write_pretty_json_file(&resolution_path, &updated_resolution).await?;

    let binding_artifact = PersistedIntentInputBindingArtifact {
        schema: "libra.intent_input_binding.v1".to_string(),
        ai_session_id: ai_session_id.clone(),
        resolution_path: resolution_path.to_string_lossy().to_string(),
        extraction_path: extraction_path.clone(),
        extraction_source: extraction_source.clone(),
        risk_level,
        intent_id: intent_id.clone(),
        summary: persisted_summary.clone(),
    };
    let binding_path = match args.output {
        Some(path) => path,
        None => storage_path.join(INTENT_INPUTS_DIR).join(format!(
            "{}.json",
            ai_session_id.clone().unwrap_or_else(|| intent_id.clone())
        )),
    };
    write_pretty_json_file(&binding_path, &binding_artifact).await?;
    ensure_full_family_intent_created(storage_path, ai_session_id.as_deref(), &intent_id).await?;

    Ok(PersistIntentResult {
        ai_session_id,
        resolution_path: resolution_path.to_string_lossy().to_string(),
        intent_id,
        binding_path: binding_path.to_string_lossy().to_string(),
        summary: persisted_summary,
    })
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
        (None, None) => bail!("one of --extraction or --ai-session-id is required"),
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
        (None, None) => bail!("one of --resolution or --ai-session-id is required"),
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
