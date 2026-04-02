//! Internal Claude Code managed runtime and maintenance helpers.

mod audit_objects;
mod common;
mod extraction;
pub(crate) mod managed_artifacts;
mod managed_inputs;
pub(crate) mod managed_run;
mod plan_checkpoint;
mod project_settings;
mod provider_session;
mod snapshot_family;

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex, MutexGuard},
};

use anyhow::{Context, Result, anyhow, bail};
use audit_objects::*;
use chrono::Utc;
use clap::Args;
use common::*;
use extraction::*;
use git_internal::internal::object::{
    plan::Plan, provenance::Provenance, run::Run, run_usage::RunUsage,
};
use managed_artifacts::*;
use managed_inputs::*;
use managed_run::*;
use plan_checkpoint::{PlanCheckpointDecision, prompt_for_plan_checkpoint_decision};
use project_settings::*;
use provider_session::*;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use snapshot_family::*;
use tempfile::TempDir;
use tokio::{fs, io::AsyncWriteExt, process::Command};
use uuid::Uuid;

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
                    ArtifactParams, ContextItemParams, CreateContextFrameParams,
                    CreateContextSnapshotParams, CreateDecisionParams, CreateEvidenceParams,
                    CreatePatchSetParams, CreatePlanParams, CreatePlanStepEventParams,
                    CreateProvenanceParams, CreateRunParams, CreateRunUsageParams,
                    CreateTaskParams, PlanStepParams, TouchedFileParams, UpdateIntentParams,
                },
                server::LibraMcpServer,
            },
        },
        db,
        head::Head,
        tui::{AssistantHistoryCell, HistoryCell, PlanUpdateHistoryCell},
    },
    utils::{
        object::write_git_object, output::OutputConfig, storage::local::LocalStorage,
        storage_ext::StorageExt, util,
    },
};

const DEFAULT_MODEL: &str = "claude-sonnet-4-6";
const DEFAULT_PYTHON_BINARY: &str = "python3";
const INTENT_EXTRACTIONS_DIR: &str = "intent-extractions";
const INTENT_RESOLUTIONS_DIR: &str = "intent-resolutions";
const INTENT_INPUTS_DIR: &str = "intent-inputs";
const PROVIDER_SESSIONS_DIR: &str = "provider-sessions";
const EVIDENCE_INPUTS_DIR: &str = "evidence-inputs";
const MANAGED_EVIDENCE_INPUTS_DIR: &str = "managed-evidence-inputs";
const DECISION_INPUTS_DIR: &str = "decision-inputs";
const FORMAL_RUN_BINDINGS_DIR: &str = "claude-run-bindings";
const PATCHSET_BINDINGS_DIR: &str = "claude-patchset-bindings";
const EVIDENCE_BINDINGS_DIR: &str = "claude-evidence-bindings";
const DECISION_BINDINGS_DIR: &str = "claude-decision-bindings";
const ZERO_COMMIT_SHA: &str = "0000000000000000000000000000000000000000";
const EMBEDDED_PYTHON_HELPER_SOURCE: &str = include_str!("helper.py");
const APPROVE_PLAN_PROMPT: &str = "The plan is approved. Exit plan mode and start implementation now. Use the approved structured plan as your execution guide. Do not rewrite the plan unless you hit a real blocker.";
const PLAN_APPROVED_NOTE: &str = "Plan approved. Starting execution.";
const PLAN_REFINING_NOTE: &str = "Refining plan based on your feedback.";
const PLAN_CANCELLED_NOTE: &str = "Plan saved. Execution not started.";
const PLAN_APPROVAL_MISSING_PLAN_WARNING: &str =
    "Claude requested plan approval, but no structured plan was available.";

#[derive(Debug, Clone, Default)]
pub struct ClaudecodeCodeArgs {
    pub working_dir: PathBuf,
    pub model: Option<String>,
    pub python_binary: Option<String>,
    pub helper_path: Option<PathBuf>,
    pub timeout_seconds: Option<u64>,
    pub interactive_approvals: bool,
    pub permission_mode: Option<String>,
    pub continue_session: bool,
    pub resume: Option<String>,
    pub fork_session: bool,
    pub session_id: Option<String>,
    pub resume_session_at: Option<String>,
}

pub async fn execute(args: ClaudecodeCodeArgs) -> Result<()> {
    let chat_args = build_chat_managed_args(&args);
    chat_managed(chat_args, &OutputConfig::default()).await
}

pub(crate) fn is_auth_error(error: &anyhow::Error) -> bool {
    project_settings::is_auth_error(error)
}

pub(crate) fn validate_code_args(args: &ClaudecodeCodeArgs, output: &OutputConfig) -> Result<()> {
    let chat_args = build_chat_managed_args(args);
    validate_chat_managed_args(&chat_args, output)
}

fn build_chat_managed_args(args: &ClaudecodeCodeArgs) -> ChatManagedArgs {
    let mut chat_args = default_chat_managed_args();
    chat_args.cwd = Some(args.working_dir.clone());
    if let Some(model) = args.model.as_ref() {
        chat_args.model = model.clone();
    }
    if let Some(python_binary) = args.python_binary.as_ref() {
        chat_args.python_binary = python_binary.clone();
    }
    chat_args.helper_path = args.helper_path.clone();
    if let Some(timeout_seconds) = args.timeout_seconds {
        chat_args.timeout_seconds = timeout_seconds;
    }
    chat_args.interactive_approvals = args.interactive_approvals;
    if let Some(permission_mode) = args.permission_mode.as_ref() {
        chat_args.permission_mode = permission_mode.clone();
    }
    chat_args.continue_session = args.continue_session;
    chat_args.resume = args.resume.clone();
    chat_args.fork_session = args.fork_session;
    chat_args.session_id = args.session_id.clone();
    chat_args.resume_session_at = args.resume_session_at.clone();
    chat_args
}

#[derive(Debug, Clone)]
pub(crate) struct ClaudecodeTuiRuntime {
    driver: ManagedClaudecodeTuiDriver,
    session_control: Arc<Mutex<ManagedSessionControl>>,
    latest_structured_plan: Arc<Mutex<Option<Vec<String>>>>,
}

impl ClaudecodeTuiRuntime {
    pub(crate) fn model_name(&self) -> &str {
        self.driver.model_name()
    }

    pub(crate) fn session_control(&self) -> ManagedSessionControl {
        lock_unpoisoned(&self.session_control).clone()
    }

    pub(crate) fn reset_for_new_conversation(&mut self) {
        *lock_unpoisoned(&self.session_control) = self.driver.initial_session_control();
        *lock_unpoisoned(&self.latest_structured_plan) = None;
    }

    fn resolved_permission_mode(&self, session_control: &ManagedSessionControl) -> String {
        session_control
            .permission_mode_override
            .clone()
            .unwrap_or_else(|| self.driver.default_permission_mode().to_string())
    }

    #[allow(dead_code)]
    pub(crate) fn note_followup_provider_session(&mut self, provider_session_id: String) {
        let current = self.session_control();
        let permission_mode = self.resolved_permission_mode(&current);
        self.note_followup_provider_session_with_mode(
            provider_session_id,
            permission_mode,
            current.libra_plan_mode,
        );
    }

    pub(crate) fn note_followup_provider_session_with_mode(
        &mut self,
        provider_session_id: String,
        permission_mode: String,
        libra_plan_mode: bool,
    ) {
        *lock_unpoisoned(&self.session_control) =
            ManagedSessionControl::followup(provider_session_id, permission_mode, libra_plan_mode);
    }

    fn plan_update_cell(
        &self,
        turn_id: u64,
        checkpoint_index: usize,
        structured_plan: &[String],
    ) -> Option<PlanUpdateHistoryCell> {
        let mut latest_plan = lock_unpoisoned(&self.latest_structured_plan);
        let cell = build_plan_update_cell(
            turn_id,
            checkpoint_index,
            latest_plan.as_deref(),
            structured_plan,
        );
        if cell.is_some() {
            *latest_plan = Some(structured_plan.to_vec());
        }
        cell
    }
}

pub(crate) async fn prepare_tui_runtime(args: ClaudecodeCodeArgs) -> Result<ClaudecodeTuiRuntime> {
    let chat_args = build_chat_managed_args(&args);
    validate_chat_managed_args(&chat_args, &OutputConfig::default())?;
    let session_control = ManagedSessionControl::from_chat_args(&chat_args);
    let (user_input_tx, _user_input_rx) = tokio::sync::mpsc::unbounded_channel();
    let (exec_approval_tx, _exec_approval_rx) = tokio::sync::mpsc::unbounded_channel();
    let driver = prepare_managed_tui_driver(chat_args, user_input_tx, exec_approval_tx).await?;

    Ok(ClaudecodeTuiRuntime {
        driver,
        session_control: Arc::new(Mutex::new(session_control)),
        latest_structured_plan: Arc::new(Mutex::new(None)),
    })
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn trimmed_nonempty_text(text: Option<String>) -> Option<String> {
    text.map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

fn plan_steps_from_strings(steps: &[String]) -> Vec<crate::internal::ai::tools::context::PlanStep> {
    steps
        .iter()
        .map(|step| crate::internal::ai::tools::context::PlanStep {
            step: step.clone(),
            status: crate::internal::ai::tools::context::StepStatus::Pending,
        })
        .collect()
}

fn build_plan_update_cell(
    turn_id: u64,
    checkpoint_index: usize,
    previous_plan: Option<&[String]>,
    structured_plan: &[String],
) -> Option<PlanUpdateHistoryCell> {
    if structured_plan.is_empty() || previous_plan == Some(structured_plan) {
        return None;
    }

    let explanation = if previous_plan.is_some() {
        Some("Claude updated the structured execution plan.".to_string())
    } else {
        Some("Claude proposed a structured execution plan.".to_string())
    };
    let mut cell = PlanUpdateHistoryCell::new(
        format!("claudecode-plan-{turn_id}-{checkpoint_index}"),
        explanation,
        plan_steps_from_strings(structured_plan),
    );
    cell.complete();
    Some(cell)
}

fn emit_history_cell(
    app_event_tx: &tokio::sync::mpsc::UnboundedSender<crate::internal::tui::AppEvent>,
    turn_id: u64,
    cell: Box<dyn HistoryCell>,
) {
    let _ = app_event_tx.send(crate::internal::tui::AppEvent::InsertHistoryCell { turn_id, cell });
}

fn emit_managed_note(
    app_event_tx: &tokio::sync::mpsc::UnboundedSender<crate::internal::tui::AppEvent>,
    turn_id: u64,
    message: impl Into<String>,
) {
    let _ = app_event_tx.send(crate::internal::tui::AppEvent::ManagedInfoNote {
        turn_id,
        message: message.into(),
    });
}

fn emit_agent_status(
    app_event_tx: &tokio::sync::mpsc::UnboundedSender<crate::internal::tui::AppEvent>,
    turn_id: u64,
    status: crate::internal::tui::AgentStatus,
) {
    let _ =
        app_event_tx.send(crate::internal::tui::AppEvent::AgentStatusUpdate { turn_id, status });
}

fn emit_intermediate_assistant_text(
    app_event_tx: &tokio::sync::mpsc::UnboundedSender<crate::internal::tui::AppEvent>,
    turn_id: u64,
    text: Option<String>,
) {
    if let Some(text) = trimmed_nonempty_text(text) {
        emit_history_cell(
            app_event_tx,
            turn_id,
            Box::new(AssistantHistoryCell::new(text)),
        );
    }
}

async fn prompt_for_visible_plan_checkpoint_decision(
    app_event_tx: &tokio::sync::mpsc::UnboundedSender<crate::internal::tui::AppEvent>,
    user_input_tx: &tokio::sync::mpsc::UnboundedSender<
        crate::internal::ai::tools::context::UserInputRequest,
    >,
    turn_id: u64,
    checkpoint_index: usize,
    assistant_text: Option<String>,
) -> Result<PlanCheckpointDecision> {
    emit_intermediate_assistant_text(app_event_tx, turn_id, assistant_text);
    prompt_for_plan_checkpoint_decision(user_input_tx, turn_id, checkpoint_index).await
}

pub(crate) async fn run_tui_turn(
    mut runtime: ClaudecodeTuiRuntime,
    turn_id: u64,
    app_event_tx: tokio::sync::mpsc::UnboundedSender<crate::internal::tui::AppEvent>,
    user_input_tx: tokio::sync::mpsc::UnboundedSender<
        crate::internal::ai::tools::context::UserInputRequest,
    >,
    exec_approval_tx: tokio::sync::mpsc::UnboundedSender<
        crate::internal::ai::sandbox::ExecApprovalRequest,
    >,
    prompt: String,
) -> Result<()> {
    let checkpoint_user_input_tx = user_input_tx.clone();
    runtime
        .driver
        .bind_tui_channels(user_input_tx, exec_approval_tx);
    let mut session_control = runtime.session_control();
    let mut prompt = prompt;
    let mut checkpoint_index = 0usize;

    loop {
        let stream_assistant_deltas = !session_control.libra_plan_mode;
        let result = runtime
            .driver
            .execute_turn(session_control.clone(), prompt, |event| match event {
                ClaudecodeTuiEvent::AssistantDelta(delta) => {
                    if stream_assistant_deltas {
                        let _ = app_event_tx.send(crate::internal::tui::AppEvent::AgentEvent {
                            turn_id,
                            event: crate::internal::tui::AgentEvent::ResponseDelta { delta },
                        });
                    }
                }
                ClaudecodeTuiEvent::AssistantMessage(_text) => {}
                ClaudecodeTuiEvent::ToolCallBegin {
                    call_id,
                    tool_name,
                    arguments,
                } => {
                    let _ = app_event_tx.send(crate::internal::tui::AppEvent::ToolCallBegin {
                        turn_id,
                        call_id,
                        tool_name,
                        arguments,
                    });
                }
                ClaudecodeTuiEvent::ToolCallEnd {
                    call_id,
                    tool_name,
                    result,
                } => {
                    let _ = app_event_tx.send(crate::internal::tui::AppEvent::ToolCallEnd {
                        turn_id,
                        call_id,
                        tool_name,
                        result,
                    });
                }
                ClaudecodeTuiEvent::Info(message) => {
                    emit_managed_note(&app_event_tx, turn_id, message);
                }
            })
            .await?;

        let ClaudecodeTuiTurnOutcome {
            provider_session_id,
            assistant_text,
            structured_plan,
            awaiting_plan_approval,
            warnings,
        } = result;

        let current_permission_mode = runtime.resolved_permission_mode(&session_control);
        runtime.note_followup_provider_session_with_mode(
            provider_session_id.clone(),
            current_permission_mode,
            session_control.libra_plan_mode,
        );

        for warning in warnings {
            emit_managed_note(&app_event_tx, turn_id, format!("Claude warning: {warning}"));
        }

        let structured_plan = structured_plan.filter(|plan| !plan.is_empty());
        if let Some(plan) = structured_plan.as_deref()
            && let Some(cell) = runtime.plan_update_cell(turn_id, checkpoint_index, plan)
        {
            emit_history_cell(&app_event_tx, turn_id, Box::new(cell));
        }

        if awaiting_plan_approval {
            if structured_plan.is_none() {
                emit_managed_note(&app_event_tx, turn_id, PLAN_APPROVAL_MISSING_PLAN_WARNING);
                let _ = app_event_tx.send(crate::internal::tui::AppEvent::AgentEvent {
                    turn_id,
                    event: crate::internal::tui::AgentEvent::ManagedResponseComplete {
                        text: assistant_text.unwrap_or_default(),
                        provider_session_id: provider_session_id.clone(),
                    },
                });
                return Ok(());
            }

            let decision = prompt_for_visible_plan_checkpoint_decision(
                &app_event_tx,
                &checkpoint_user_input_tx,
                turn_id,
                checkpoint_index,
                assistant_text,
            )
            .await?;
            match decision {
                PlanCheckpointDecision::Approve => {
                    emit_managed_note(&app_event_tx, turn_id, PLAN_APPROVED_NOTE);
                    emit_agent_status(
                        &app_event_tx,
                        turn_id,
                        crate::internal::tui::AgentStatus::Thinking,
                    );
                    session_control = ManagedSessionControl::followup(
                        provider_session_id.clone(),
                        "acceptEdits",
                        false,
                    );
                    runtime.note_followup_provider_session_with_mode(
                        provider_session_id,
                        "acceptEdits".to_string(),
                        false,
                    );
                    prompt = APPROVE_PLAN_PROMPT.to_string();
                    checkpoint_index += 1;
                    continue;
                }
                PlanCheckpointDecision::Refine { note } => {
                    emit_managed_note(&app_event_tx, turn_id, PLAN_REFINING_NOTE);
                    emit_agent_status(
                        &app_event_tx,
                        turn_id,
                        crate::internal::tui::AgentStatus::Thinking,
                    );
                    session_control =
                        ManagedSessionControl::followup(provider_session_id.clone(), "plan", true);
                    runtime.note_followup_provider_session_with_mode(
                        provider_session_id,
                        "plan".to_string(),
                        true,
                    );
                    prompt = format!(
                        "Refine the current structured plan based on this user feedback:\n\n{}\n\nStay in plan mode. Update the plan and planningSummary as needed, but do not start implementation or use mutating tools.",
                        note
                    );
                    checkpoint_index += 1;
                    continue;
                }
                PlanCheckpointDecision::Cancel => {
                    let _ = app_event_tx.send(crate::internal::tui::AppEvent::AgentEvent {
                        turn_id,
                        event: crate::internal::tui::AgentEvent::ManagedResponseComplete {
                            text: PLAN_CANCELLED_NOTE.to_string(),
                            provider_session_id,
                        },
                    });
                    return Ok(());
                }
            }
        }

        let _ = app_event_tx.send(crate::internal::tui::AppEvent::AgentEvent {
            turn_id,
            event: crate::internal::tui::AgentEvent::ManagedResponseComplete {
                text: assistant_text.unwrap_or_default(),
                provider_session_id,
            },
        });
        return Ok(());
    }
}

#[derive(Args, Debug)]
pub(super) struct BridgeRunArgs {
    #[arg(
        long,
        help = "Claude Code ai_session_id to bridge into formal Task/Run objects"
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
    #[arg(skip)]
    existing_plan_id: Option<String>,
}

#[derive(Args, Debug)]
struct PersistEvidenceArgs {
    #[arg(
        long,
        help = "Claude Code ai_session_id whose formal run should receive Evidence"
    )]
    ai_session_id: String,
}

#[derive(Args, Debug)]
struct PersistPatchSetArgs {
    #[arg(
        long,
        help = "Claude Code ai_session_id whose formal run should receive a PatchSet"
    )]
    ai_session_id: String,
    #[arg(
        long,
        help = "Optional output path for the Claude patchset binding; defaults to .libra/claude-patchset-bindings/<ai-session-id>.json"
    )]
    output: Option<PathBuf>,
}

#[derive(Args, Debug)]
struct PersistDecisionArgs {
    #[arg(
        long,
        help = "Claude Code ai_session_id whose formal run should receive a terminal Decision"
    )]
    ai_session_id: String,
}

#[derive(Debug, Serialize)]
#[allow(dead_code)]
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
    #[serde(rename = "planId", skip_serializing_if = "Option::is_none")]
    plan_id: Option<String>,
}

pub(super) struct BridgeRunResult {
    pub(super) binding_path: PathBuf,
    pub(super) binding: ClaudeFormalRunBindingArtifact,
}

#[derive(Debug, Serialize)]
#[allow(dead_code)]
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

#[allow(dead_code)]
struct PersistEvidenceResult {
    binding_path: PathBuf,
    binding: ClaudeEvidenceBindingArtifact,
}

#[derive(Debug, Serialize)]
#[allow(dead_code)]
struct PersistPatchSetCommandOutput {
    ok: bool,
    #[serde(rename = "mode")]
    command_mode: &'static str,
    #[serde(rename = "aiSessionId")]
    ai_session_id: String,
    #[serde(rename = "runId")]
    run_id: String,
    #[serde(rename = "patchsetId")]
    patchset_id: String,
    #[serde(rename = "bindingPath")]
    binding_path: String,
}

#[allow(dead_code)]
struct PersistPatchSetResult {
    binding_path: PathBuf,
    binding: ClaudePatchSetBindingArtifact,
}

#[derive(Debug, Serialize)]
#[allow(dead_code)]
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

#[allow(dead_code)]
struct PersistDecisionResult {
    binding_path: PathBuf,
    binding: ClaudeDecisionBindingArtifact,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PersistedManagedEvidenceInputArtifact {
    schema: String,
    object_type: String,
    provider: String,
    #[serde(rename = "objectId")]
    object_id: String,
    #[serde(rename = "aiSessionId")]
    ai_session_id: String,
    #[serde(rename = "providerSessionId")]
    provider_session_id: String,
    summary: String,
    #[serde(rename = "sourceArtifacts")]
    source_artifacts: ManagedEvidenceInputSourceArtifacts,
    #[serde(rename = "patchOverview")]
    patch_overview: ManagedEvidencePatchOverview,
    #[serde(rename = "runtimeOverview")]
    runtime_overview: ManagedEvidenceRuntimeOverview,
    #[serde(rename = "capturedAt")]
    captured_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ManagedEvidenceInputSourceArtifacts {
    #[serde(rename = "rawArtifactPath")]
    raw_artifact_path: String,
    #[serde(rename = "auditBundlePath")]
    audit_bundle_path: String,
    #[serde(
        rename = "providerSessionPath",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    provider_session_path: Option<String>,
    #[serde(
        rename = "providerEvidenceInputPath",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    provider_evidence_input_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ManagedEvidencePatchOverview {
    #[serde(rename = "touchedFiles")]
    touched_files: Vec<String>,
    #[serde(rename = "observedTools")]
    observed_tools: BTreeMap<String, usize>,
    #[serde(rename = "filesPersisted")]
    files_persisted: Vec<ManagedPersistedFile>,
    #[serde(rename = "failedFilesPersisted")]
    failed_files_persisted: Vec<ManagedFailedPersistedFile>,
    #[serde(rename = "checkpointingEnabled")]
    checkpointing_enabled: bool,
    #[serde(rename = "rewindSupported")]
    rewind_supported: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ManagedPersistedFile {
    filename: String,
    #[serde(rename = "fileId")]
    file_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ManagedFailedPersistedFile {
    filename: String,
    error: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ManagedEvidenceRuntimeOverview {
    #[serde(rename = "toolInvocationCount")]
    tool_invocation_count: usize,
    #[serde(rename = "toolRuntimeCount")]
    tool_runtime_count: usize,
    #[serde(rename = "assistantRuntimeCount")]
    assistant_runtime_count: usize,
    #[serde(rename = "taskRuntimeCount")]
    task_runtime_count: usize,
    #[serde(rename = "decisionRuntimeCount")]
    decision_runtime_count: usize,
    #[serde(rename = "contextRuntimeCount")]
    context_runtime_count: usize,
    #[serde(rename = "hasStructuredOutput")]
    has_structured_output: bool,
    #[serde(rename = "hasPermissionDenials")]
    has_permission_denials: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PersistedDecisionInputArtifact {
    schema: String,
    object_type: String,
    provider: String,
    #[serde(rename = "objectId")]
    object_id: String,
    #[serde(rename = "aiSessionId")]
    ai_session_id: String,
    #[serde(rename = "providerSessionId")]
    provider_session_id: String,
    summary: String,
    #[serde(rename = "sourceArtifacts")]
    source_artifacts: DecisionInputSourceArtifacts,
    #[serde(rename = "decisionOverview")]
    decision_overview: DecisionInputOverview,
    signals: Vec<DecisionInputSignal>,
    #[serde(rename = "capturedAt")]
    captured_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct DecisionInputSourceArtifacts {
    #[serde(rename = "auditBundlePath")]
    audit_bundle_path: String,
    #[serde(
        rename = "managedEvidenceInputPath",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    managed_evidence_input_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct DecisionInputOverview {
    #[serde(rename = "runtimeEventCount")]
    runtime_event_count: usize,
    #[serde(rename = "permissionRequestCount")]
    permission_request_count: usize,
    #[serde(rename = "canUseToolCount")]
    can_use_tool_count: usize,
    #[serde(rename = "elicitationCount")]
    elicitation_count: usize,
    #[serde(rename = "elicitationResultCount")]
    elicitation_result_count: usize,
    #[serde(rename = "permissionDenialCount")]
    permission_denial_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct DecisionInputSignal {
    id: String,
    kind: String,
    source: String,
    #[serde(rename = "toolName", default, skip_serializing_if = "Option::is_none")]
    tool_name: Option<String>,
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
    #[serde(
        rename = "mcpServerName",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    mcp_server_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    action: Option<String>,
    #[serde(
        rename = "permissionDenialCount",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    permission_denial_count: Option<usize>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct ClaudeFormalRunBindingArtifact {
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
    #[serde(rename = "planId", skip_serializing_if = "Option::is_none")]
    plan_id: Option<String>,
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
    #[serde(
        rename = "managedEvidenceInputPath",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    managed_evidence_input_path: Option<String>,
    #[serde(
        rename = "patchsetBindingPath",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    patchset_binding_path: Option<String>,
    #[serde(
        rename = "patchsetId",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    patchset_id: Option<String>,
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
    #[serde(
        rename = "decisionInputPath",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    decision_input_path: Option<String>,
    #[serde(
        rename = "patchsetBindingPath",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    patchset_binding_path: Option<String>,
    #[serde(
        rename = "patchsetId",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    patchset_id: Option<String>,
    #[serde(rename = "evidenceIds", default, skip_serializing_if = "Vec::is_empty")]
    evidence_ids: Vec<String>,
    #[serde(rename = "createdAt")]
    created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ClaudePatchSetBindingArtifact {
    schema: String,
    #[serde(rename = "aiSessionId")]
    ai_session_id: String,
    #[serde(rename = "providerSessionId")]
    provider_session_id: String,
    #[serde(rename = "runId")]
    run_id: String,
    #[serde(rename = "patchsetId")]
    patchset_id: String,
    #[serde(rename = "runBindingPath")]
    run_binding_path: String,
    #[serde(rename = "managedEvidenceInputPath")]
    managed_evidence_input_path: String,
    summary: String,
    #[serde(rename = "touchedFiles")]
    touched_files: Vec<String>,
    #[serde(
        rename = "diffArtifactStore",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    diff_artifact_store: Option<String>,
    #[serde(
        rename = "diffArtifactKey",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    diff_artifact_key: Option<String>,
    #[serde(rename = "createdAt")]
    created_at: String,
}

struct DerivedPatchSetDiffArtifact {
    store: String,
    key: String,
    file_line_counts: BTreeMap<String, (u32, u32)>,
}

struct RenderedManagedPatchDiff {
    path: String,
    content: String,
    lines_added: u32,
    lines_deleted: u32,
}

type ManagedPatchDiffRender = (String, BTreeMap<String, (u32, u32)>);

#[allow(dead_code)]
async fn bridge_run(args: BridgeRunArgs) -> Result<()> {
    let result = bridge_run_internal(args).await?;
    print_bridge_run_output(&result.binding_path, &result.binding)?;
    Ok(())
}

pub(super) async fn bridge_run_internal(args: BridgeRunArgs) -> Result<BridgeRunResult> {
    let storage_path = util::try_get_storage_path(None)
        .context("Claude Code managed commands must be run inside a Libra repository")?;
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
    let mut existing_plan_id = args.existing_plan_id.clone();

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
        if !formal_run_binding_objects_exist(&storage_path, &existing).await? {
            // Fall through and rebuild a stale binding whose optional plan object disappeared.
        } else {
            validate_formal_run_binding_consistency(&existing, &args.ai_session_id)?;
            existing_plan_id = existing.plan_id.clone();
            if formal_run_binding_matches_current_audit_bundle(
                &storage_path,
                &existing,
                &audit_bundle,
                requested_intent_id.as_deref(),
                &summary,
                &managed_run_status,
                &intent_extraction_status,
            )
            .await?
            {
                ensure_formal_runtime_side_objects(&storage_path, &existing, &audit_bundle).await?;
                ensure_formal_derived_audit_objects(&storage_path, &existing, &audit_bundle)
                    .await?;
                ensure_full_family_run_objects(&storage_path, &existing, &audit_bundle).await?;
                ensure_full_family_plan_objects(&storage_path, &existing.ai_session_id, &existing)
                    .await?;
                return Ok(BridgeRunResult {
                    binding_path,
                    binding: existing,
                });
            }
        }
    }

    let mcp_server = init_local_mcp_server(&storage_path).await?;
    let actor = mcp_server
        .resolve_actor_from_params(Some("system"), Some("claudecode-bridge"))
        .map_err(|error| anyhow!("failed to resolve Claude Code bridge actor: {error:?}"))?;
    let planning_context_frame_ids = if let Some(intent_id) = requested_intent_id.as_deref() {
        create_context_frames_for_audit_bundle(
            &mcp_server,
            &actor,
            &audit_bundle,
            Some(intent_id),
            None,
        )
        .await?
    } else {
        Vec::new()
    };
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
                    constraints: Some(vec!["claudecode managed bridge".to_string()]),
                    acceptance_criteria: None,
                    requested_by_kind: None,
                    requested_by_id: None,
                    dependencies: None,
                    intent_id: requested_intent_id.clone(),
                    parent_task_id: None,
                    origin_step_id: None,
                    status: Some(task_status_for_managed_run(&managed_run_status).to_string()),
                    reason: Some("Claude Code managed bridge created a formal task".to_string()),
                    tags: None,
                    external_ids: None,
                    actor_kind: Some("system".to_string()),
                    actor_id: Some("claudecode-bridge".to_string()),
                },
                actor.clone(),
            )
            .await
            .map_err(|error| anyhow!("failed to create formal Claude task: {error:?}"))?,
    )?;
    let plan_id = if let Some(intent_id) = requested_intent_id.clone() {
        bridge_run_plan_id(
            &mcp_server,
            &actor,
            &intent_id,
            &audit_bundle,
            &planning_context_frame_ids,
            existing_plan_id.as_deref(),
        )
        .await?
    } else {
        None
    };
    let run_id = parse_created_id(
        "run",
        &mcp_server
            .create_run_impl(
                CreateRunParams {
                    task_id: task_id.clone(),
                    base_commit_sha: current_head_sha().await,
                    plan_id: plan_id.clone(),
                    status: Some(run_status_for_managed_run(&managed_run_status).to_string()),
                    context_snapshot_id: context_snapshot_id.clone(),
                    error: run_error_for_managed_status(&managed_run_status),
                    agent_instances: None,
                    metrics_json: Some(
                        json!({
                            "provider": "claude",
                            "intentExtractionStatus": intent_extraction_status,
                        })
                        .to_string(),
                    ),
                    reason: Some("Claude Code managed bridge created a formal run".to_string()),
                    orchestrator_version: None,
                    tags: None,
                    external_ids: None,
                    actor_kind: Some("system".to_string()),
                    actor_id: Some("claudecode-bridge".to_string()),
                },
                actor.clone(),
            )
            .await
            .map_err(|error| anyhow!("failed to create formal Claude run: {error:?}"))?,
    )?;

    let binding = ClaudeFormalRunBindingArtifact {
        schema: "libra.claude_formal_run_binding.v1".to_string(),
        ai_session_id: args.ai_session_id,
        provider_session_id: audit_bundle.provider_session_id.clone(),
        task_id,
        run_id,
        audit_bundle_path: audit_bundle_path.to_string_lossy().to_string(),
        intent_binding_path: intent_binding
            .as_ref()
            .map(|resolved| resolved.path.to_string_lossy().to_string()),
        intent_id: requested_intent_id,
        plan_id,
        managed_run_status,
        intent_extraction_status,
        summary,
        created_at: Utc::now().to_rfc3339(),
    };
    write_pretty_json_file(&binding_path, &binding).await?;
    if planning_context_frame_ids.is_empty() {
        create_context_frames_for_audit_bundle(
            &mcp_server,
            &actor,
            &audit_bundle,
            binding.intent_id.as_deref(),
            Some(&binding.run_id),
        )
        .await?;
    }
    ensure_formal_runtime_side_objects(&storage_path, &binding, &audit_bundle).await?;
    ensure_formal_derived_audit_objects(&storage_path, &binding, &audit_bundle).await?;
    ensure_full_family_run_objects(&storage_path, &binding, &audit_bundle).await?;
    ensure_full_family_plan_objects(&storage_path, &binding.ai_session_id, &binding).await?;
    Ok(BridgeRunResult {
        binding_path,
        binding,
    })
}

async fn ensure_formal_runtime_side_objects(
    storage_path: &Path,
    run_binding: &ClaudeFormalRunBindingArtifact,
    audit_bundle: &ManagedAuditBundle,
) -> Result<()> {
    let mcp_server = init_local_mcp_server(storage_path).await?;
    let actor = mcp_server
        .resolve_actor_from_params(Some("system"), Some("claudecode-runtime"))
        .map_err(|error| anyhow!("failed to resolve Claude Code runtime actor: {error:?}"))?;

    ensure_formal_provenance_object(storage_path, &mcp_server, &actor, run_binding, audit_bundle)
        .await?;
    ensure_formal_run_usage_object(storage_path, &mcp_server, &actor, run_binding, audit_bundle)
        .await?;
    Ok(())
}

async fn create_context_snapshot_for_audit_bundle(
    mcp_server: &LibraMcpServer,
    actor: &git_internal::internal::object::types::ActorRef,
    audit_bundle: &ManagedAuditBundle,
) -> Result<Option<String>> {
    let items = build_bridge_context_snapshot_items(audit_bundle);

    let result = mcp_server
        .create_context_snapshot_impl(
            CreateContextSnapshotParams {
                selection_strategy: if items.is_empty() {
                    "heuristic".to_string()
                } else {
                    "explicit".to_string()
                },
                items: (!items.is_empty()).then_some(items),
                summary: Some(build_bridge_context_snapshot_summary(audit_bundle)),
                tags: None,
                external_ids: None,
                actor_kind: Some("system".to_string()),
                actor_id: Some("claudecode-context".to_string()),
            },
            actor.clone(),
        )
        .await
        .map_err(|error| anyhow!("failed to create Claude context snapshot: {error:?}"))?;
    Ok(Some(parse_created_id("context snapshot", &result)?))
}

async fn create_context_frames_for_audit_bundle(
    mcp_server: &LibraMcpServer,
    actor: &git_internal::internal::object::types::ActorRef,
    audit_bundle: &ManagedAuditBundle,
    intent_id: Option<&str>,
    run_id: Option<&str>,
) -> Result<Vec<String>> {
    let mut specs = Vec::new();
    if !audit_bundle.bridge.touch_hints.is_empty() {
        let touched_files = audit_bundle
            .bridge
            .touch_hints
            .iter()
            .filter_map(|hint| {
                persistable_touch_hint(hint, &audit_bundle.bridge.session_state.working_dir)
            })
            .collect::<Vec<_>>();
        if !touched_files.is_empty() {
            specs.push((
                "code_change".to_string(),
                format!("Observed touched files: {}", touched_files.join(", ")),
                json!({ "touched_files": touched_files }),
            ));
        }
    }

    for event in &audit_bundle.bridge.object_candidates.context_runtime_events {
        specs.push((
            context_frame_kind_for_event(event).to_string(),
            summarize_context_runtime_event(event),
            event.payload.clone(),
        ));
    }

    if specs.is_empty() {
        specs.push((
            "system_state".to_string(),
            "Claude bridge context prepared".to_string(),
            json!({
                "provider": "claude",
                "ai_session_id": audit_bundle.ai_session_id,
                "provider_session_id": audit_bundle.provider_session_id,
            }),
        ));
    }

    let mut ids = Vec::new();
    for (kind, summary, data) in specs {
        let result = mcp_server
            .create_context_frame_impl(
                CreateContextFrameParams {
                    kind,
                    summary: summary.clone(),
                    intent_id: intent_id.map(ToString::to_string),
                    run_id: run_id.map(ToString::to_string),
                    plan_id: None,
                    step_id: None,
                    data: Some(data),
                    token_estimate: Some(token_estimate_for_summary(&summary)),
                    actor_kind: Some("system".to_string()),
                    actor_id: Some("claudecode-context".to_string()),
                },
                actor.clone(),
            )
            .await
            .map_err(|error| anyhow!("failed to create Claude context frame: {error:?}"))?;
        ids.push(parse_created_id("context frame", &result)?);
    }
    Ok(ids)
}

fn build_bridge_context_snapshot_items(
    audit_bundle: &ManagedAuditBundle,
) -> Vec<ContextItemParams> {
    audit_bundle
        .bridge
        .touch_hints
        .iter()
        .filter_map(|hint| {
            persistable_touch_hint(hint, &audit_bundle.bridge.session_state.working_dir)
        })
        .map(|path| ContextItemParams {
            kind: Some("file".to_string()),
            path,
            preview: None,
            content_hash: None,
            blob_hash: None,
        })
        .collect()
}

fn build_bridge_context_snapshot_summary(audit_bundle: &ManagedAuditBundle) -> String {
    format!(
        "Claude bridge context snapshot: touched_files={}; context_events={}",
        audit_bundle.bridge.touch_hints.len(),
        audit_bundle
            .bridge
            .object_candidates
            .context_runtime_events
            .len(),
    )
}

fn context_frame_kind_for_event(event: &ManagedSemanticRuntimeEvent) -> &'static str {
    match event.kind.as_str() {
        "files_persisted" | "WorktreeCreate" | "WorktreeRemove" => "code_change",
        "compact_boundary" | "PreCompact" | "PostCompact" => "checkpoint",
        "prompt_suggestion" => "step_summary",
        _ => "system_state",
    }
}

fn summarize_context_runtime_event(event: &ManagedSemanticRuntimeEvent) -> String {
    match event.kind.as_str() {
        "status" => format!(
            "status: {}",
            event
                .payload
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
        ),
        "files_persisted" => {
            let count = event
                .payload
                .get("files")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            format!("files_persisted: {count} file(s)")
        }
        "rate_limit_event" => format!(
            "rate_limit_event: {}",
            event
                .payload
                .get("rate_limit_info")
                .and_then(|info| info.get("status"))
                .and_then(Value::as_str)
                .unwrap_or("observed")
        ),
        other => format!("context event observed: {other}"),
    }
}

fn token_estimate_for_summary(summary: &str) -> u64 {
    let chars = summary.chars().count();
    ((chars.max(1) as u64) / 4).max(1)
}

async fn ensure_formal_provenance_object(
    storage_path: &Path,
    mcp_server: &LibraMcpServer,
    actor: &git_internal::internal::object::types::ActorRef,
    run_binding: &ClaudeFormalRunBindingArtifact,
    audit_bundle: &ManagedAuditBundle,
) -> Result<()> {
    let provenance = &audit_bundle.bridge.object_candidates.provenance_snapshot;
    let Some(model) = provenance.model.as_deref() else {
        return Ok(());
    };
    if find_matching_provenance(
        storage_path,
        &run_binding.run_id,
        &provenance.provider,
        model,
        &provenance.parameters,
    )
    .await?
    .is_some()
    {
        return Ok(());
    }

    mcp_server
        .create_provenance_impl(
            CreateProvenanceParams {
                run_id: run_binding.run_id.clone(),
                provider: provenance.provider.clone(),
                model: model.to_string(),
                parameters_json: Some(provenance.parameters.to_string()),
                temperature: provenance
                    .parameters
                    .get("temperature")
                    .and_then(Value::as_f64),
                max_tokens: provenance
                    .parameters
                    .get("max_tokens")
                    .or_else(|| provenance.parameters.get("maxTokens"))
                    .and_then(Value::as_u64),
                tags: None,
                external_ids: None,
                actor_kind: Some("system".to_string()),
                actor_id: Some("claudecode-runtime".to_string()),
            },
            actor.clone(),
        )
        .await
        .map_err(|error| anyhow!("failed to create Claude provenance: {error:?}"))?;
    Ok(())
}

async fn ensure_formal_run_usage_object(
    storage_path: &Path,
    mcp_server: &LibraMcpServer,
    actor: &git_internal::internal::object::types::ActorRef,
    run_binding: &ClaudeFormalRunBindingArtifact,
    audit_bundle: &ManagedAuditBundle,
) -> Result<()> {
    let Some(run_usage_event) = audit_bundle
        .bridge
        .object_candidates
        .run_usage_event
        .as_ref()
    else {
        return Ok(());
    };
    let input_tokens = usage_counter(&run_usage_event.usage, &["input_tokens", "inputTokens"]);
    let output_tokens = usage_counter(&run_usage_event.usage, &["output_tokens", "outputTokens"]);
    let cost_usd = run_usage_event
        .usage
        .get("cost_usd")
        .or_else(|| run_usage_event.usage.get("costUSD"))
        .and_then(Value::as_f64);
    if find_matching_run_usage(
        storage_path,
        &run_binding.run_id,
        input_tokens,
        output_tokens,
        cost_usd,
    )
    .await?
    .is_some()
    {
        return Ok(());
    }

    mcp_server
        .create_run_usage_impl(
            CreateRunUsageParams {
                run_id: run_binding.run_id.clone(),
                input_tokens,
                output_tokens,
                cost_usd,
                actor_kind: Some("system".to_string()),
                actor_id: Some("claudecode-runtime".to_string()),
            },
            actor.clone(),
        )
        .await
        .map_err(|error| anyhow!("failed to create Claude run usage: {error:?}"))?;
    Ok(())
}

async fn find_matching_provenance(
    storage_path: &Path,
    run_id: &str,
    provider: &str,
    model: &str,
    parameters: &Value,
) -> Result<Option<String>> {
    let history = init_local_mcp_server(storage_path).await?;
    let history = history
        .intent_history_manager
        .as_ref()
        .ok_or_else(|| anyhow!("local MCP history manager is unavailable"))?;
    let storage = LocalStorage::new(storage_path.join("objects"));
    for (object_id, object_hash) in history
        .list_objects("provenance")
        .await
        .context("failed to list provenance objects")?
    {
        let object = storage
            .get_json::<Provenance>(&object_hash)
            .await
            .with_context(|| format!("failed to load provenance '{object_id}'"))?;
        if object.run_id().to_string() == run_id
            && object.provider() == provider
            && object.model() == model
            && object.parameters() == Some(parameters)
        {
            return Ok(Some(object_id));
        }
    }
    Ok(None)
}

async fn find_matching_run_usage(
    storage_path: &Path,
    run_id: &str,
    input_tokens: u64,
    output_tokens: u64,
    cost_usd: Option<f64>,
) -> Result<Option<String>> {
    let history = init_local_mcp_server(storage_path).await?;
    let history = history
        .intent_history_manager
        .as_ref()
        .ok_or_else(|| anyhow!("local MCP history manager is unavailable"))?;
    let storage = LocalStorage::new(storage_path.join("objects"));
    for (object_id, object_hash) in history
        .list_objects("run_usage")
        .await
        .context("failed to list run usage objects")?
    {
        let object = storage
            .get_json::<RunUsage>(&object_hash)
            .await
            .with_context(|| format!("failed to load run usage '{object_id}'"))?;
        if object.run_id().to_string() == run_id
            && object.input_tokens() == input_tokens
            && object.output_tokens() == output_tokens
            && object.cost_usd() == cost_usd
        {
            return Ok(Some(object_id));
        }
    }
    Ok(None)
}

#[allow(dead_code)]
async fn persist_evidence(args: PersistEvidenceArgs) -> Result<()> {
    let result = persist_evidence_internal(args).await?;
    print_persist_evidence_output(&result.binding_path, &result.binding)?;
    Ok(())
}

async fn persist_evidence_internal(args: PersistEvidenceArgs) -> Result<PersistEvidenceResult> {
    let storage_path = util::try_get_storage_path(None)
        .context("Claude Code managed commands must be run inside a Libra repository")?;
    validate_ai_session_id(&args.ai_session_id)?;

    let run_binding_path = formal_run_binding_path(&storage_path, &args.ai_session_id);
    let run_binding: ClaudeFormalRunBindingArtifact =
        read_typed_json_artifact(&run_binding_path, "formal Claude run binding")
            .await
            .with_context(|| {
                format!(
                    "run a managed Claude Code turn for ai_session_id '{}' first",
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
    let managed_evidence_input_object_id =
        build_managed_evidence_input_object_id(&args.ai_session_id)?;
    let managed_evidence_input_path =
        managed_evidence_input_artifact_path(&storage_path, &managed_evidence_input_object_id);
    let resolved_patchset_binding =
        load_patchset_binding_for_ai_session(&storage_path, &args.ai_session_id, &run_binding)
            .await?;
    let patchset_binding_artifact_path = resolved_patchset_binding
        .as_ref()
        .map(|(path, _)| path.to_string_lossy().to_string());
    let patchset_id = resolved_patchset_binding
        .as_ref()
        .map(|(_, binding)| binding.patchset_id.clone());

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

    let managed_evidence_input_path = if managed_evidence_input_path.exists() {
        let managed_evidence_input: PersistedManagedEvidenceInputArtifact = read_json_artifact(
            &managed_evidence_input_path,
            "managed evidence input artifact",
        )
        .await?;
        entries.push(PendingEvidence {
            kind: "managed_evidence_input_summary".to_string(),
            source_path: managed_evidence_input_path.to_string_lossy().to_string(),
            summary: managed_evidence_input.summary.clone(),
        });
        Some(managed_evidence_input_path.to_string_lossy().to_string())
    } else {
        None
    };

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
        && evidence_binding_patchset_exists(&storage_path, &existing).await?
    {
        validate_evidence_binding_consistency(&existing, &args.ai_session_id, &run_binding)?;
        if evidence_binding_matches_expected(
            &existing,
            &expected_entries,
            managed_evidence_input_path.as_deref(),
            patchset_binding_artifact_path.as_deref(),
            patchset_id.as_deref(),
        ) {
            return Ok(PersistEvidenceResult {
                binding_path,
                binding: existing,
            });
        }
    }

    let mcp_server = init_local_mcp_server(&storage_path).await?;
    let actor = mcp_server
        .resolve_actor_from_params(Some("system"), Some("claudecode-evidence"))
        .map_err(|error| anyhow!("failed to resolve Claude Code evidence actor: {error:?}"))?;
    let mut evidence_entries = Vec::new();
    for entry in entries {
        let evidence_id = parse_created_id(
            "evidence",
            &mcp_server
                .create_evidence_impl(
                    CreateEvidenceParams {
                        run_id: run_binding.run_id.clone(),
                        patchset_id: patchset_id.clone(),
                        kind: entry.kind.clone(),
                        tool: "claudecode".to_string(),
                        command: None,
                        exit_code: None,
                        summary: Some(entry.summary.clone()),
                        report_artifacts: None,
                        tags: None,
                        external_ids: None,
                        actor_kind: Some("system".to_string()),
                        actor_id: Some("claudecode-evidence".to_string()),
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
        managed_evidence_input_path,
        patchset_binding_path: patchset_binding_artifact_path,
        patchset_id,
        evidence_ids: evidence_entries
            .iter()
            .map(|entry| entry.evidence_id.clone())
            .collect(),
        evidences: evidence_entries,
        created_at: Utc::now().to_rfc3339(),
    };
    write_pretty_json_file(&binding_path, &binding).await?;
    Ok(PersistEvidenceResult {
        binding_path,
        binding,
    })
}

#[allow(dead_code)]
async fn persist_patchset(args: PersistPatchSetArgs) -> Result<()> {
    let result = persist_patchset_internal(args).await?;
    print_persist_patchset_output(&result.binding_path, &result.binding)?;
    Ok(())
}

async fn persist_patchset_internal(args: PersistPatchSetArgs) -> Result<PersistPatchSetResult> {
    let storage_path = util::try_get_storage_path(None)
        .context("Claude Code managed commands must be run inside a Libra repository")?;
    validate_ai_session_id(&args.ai_session_id)?;

    let run_binding_path = formal_run_binding_path(&storage_path, &args.ai_session_id);
    let run_binding: ClaudeFormalRunBindingArtifact =
        read_typed_json_artifact(&run_binding_path, "formal Claude run binding")
            .await
            .with_context(|| {
                format!(
                    "run a managed Claude Code turn for ai_session_id '{}' first",
                    args.ai_session_id
                )
            })?;
    validate_formal_run_binding_consistency(&run_binding, &args.ai_session_id)?;

    let managed_evidence_input_object_id =
        build_managed_evidence_input_object_id(&args.ai_session_id)?;
    let managed_evidence_input_path =
        managed_evidence_input_artifact_path(&storage_path, &managed_evidence_input_object_id);
    let managed_evidence_input: PersistedManagedEvidenceInputArtifact = read_json_artifact(
        &managed_evidence_input_path,
        "managed evidence input artifact",
    )
    .await
    .with_context(|| {
        format!(
            "build the managed Claude Code evidence input for ai_session_id '{}' first",
            args.ai_session_id
        )
    })?;
    if managed_evidence_input.ai_session_id != args.ai_session_id {
        bail!(
            "managed evidence input '{}' belongs to ai session '{}', not '{}'",
            managed_evidence_input_path.display(),
            managed_evidence_input.ai_session_id,
            args.ai_session_id
        );
    }
    if managed_evidence_input.provider_session_id != run_binding.provider_session_id {
        bail!(
            "managed evidence input '{}' belongs to provider session '{}', not '{}'",
            managed_evidence_input_path.display(),
            managed_evidence_input.provider_session_id,
            run_binding.provider_session_id
        );
    }
    if managed_evidence_input
        .patch_overview
        .touched_files
        .is_empty()
    {
        bail!(
            "managed evidence input '{}' contains no touched files; no formal patchset can be created",
            managed_evidence_input_path.display()
        );
    }

    let formal_run: Run =
        read_tracked_object(&storage_path, "run", &run_binding.run_id, "formal run").await?;
    let (_, audit_bundle) =
        load_audit_bundle_for_run_binding(&storage_path, &run_binding, &args.ai_session_id).await?;
    let derived_diff_artifact =
        persist_patchset_diff_artifact(&storage_path, &audit_bundle).await?;
    let binding_path = match args.output {
        Some(path) => path,
        None => patchset_binding_path(&storage_path, &args.ai_session_id),
    };
    if let Some(existing) = read_existing_binding_if_live::<ClaudePatchSetBindingArtifact>(
        &storage_path,
        &binding_path,
        "Claude patchset binding",
        &[
            ("run", |binding| binding.run_id.as_str()),
            ("patchset", |binding| binding.patchset_id.as_str()),
        ],
    )
    .await?
        && existing.run_id == run_binding.run_id
        && patchset_binding_matches_expected(
            &existing,
            &managed_evidence_input.summary,
            &managed_evidence_input.patch_overview.touched_files,
            &managed_evidence_input_path,
            derived_diff_artifact.as_ref(),
        )
    {
        validate_patchset_binding_consistency(&existing, &args.ai_session_id, &run_binding)?;
        return Ok(PersistPatchSetResult {
            binding_path,
            binding: existing,
        });
    }

    let mcp_server = init_local_mcp_server(&storage_path).await?;
    let actor = mcp_server
        .resolve_actor_from_params(Some("system"), Some("claudecode-patchset"))
        .map_err(|error| anyhow!("failed to resolve Claude Code patchset actor: {error:?}"))?;
    let touched_files = managed_evidence_input
        .patch_overview
        .touched_files
        .iter()
        .map(|path| TouchedFileParams {
            path: path.clone(),
            change_type: "modify".to_string(),
            lines_added: derived_diff_artifact
                .as_ref()
                .and_then(|artifact| artifact.file_line_counts.get(path).map(|(added, _)| *added))
                .unwrap_or(0),
            lines_deleted: derived_diff_artifact
                .as_ref()
                .and_then(|artifact| {
                    artifact
                        .file_line_counts
                        .get(path)
                        .map(|(_, deleted)| *deleted)
                })
                .unwrap_or(0),
        })
        .collect::<Vec<_>>();
    let patchset_id = parse_created_id(
        "patchset",
        &mcp_server
            .create_patchset_impl(
                CreatePatchSetParams {
                    run_id: run_binding.run_id.clone(),
                    generation: 0,
                    sequence: Some(0),
                    base_commit_sha: formal_run.commit().to_string(),
                    touched_files: Some(touched_files),
                    rationale: Some(managed_evidence_input.summary.clone()),
                    diff_format: derived_diff_artifact
                        .as_ref()
                        .map(|_| "unified_diff".to_string()),
                    diff_artifact: derived_diff_artifact
                        .as_ref()
                        .map(|artifact| ArtifactParams {
                            store: artifact.store.clone(),
                            key: artifact.key.clone(),
                            content_type: Some("text/x-diff".to_string()),
                            size_bytes: None,
                            hash: Some(artifact.key.clone()),
                        }),
                    tags: None,
                    external_ids: None,
                    actor_kind: Some("system".to_string()),
                    actor_id: Some("claudecode-patchset".to_string()),
                },
                actor,
            )
            .await
            .map_err(|error| anyhow!("failed to create Claude patchset: {error:?}"))?,
    )?;

    let binding = ClaudePatchSetBindingArtifact {
        schema: "libra.claude_patchset_binding.v1".to_string(),
        ai_session_id: args.ai_session_id,
        provider_session_id: run_binding.provider_session_id.clone(),
        run_id: run_binding.run_id.clone(),
        patchset_id,
        run_binding_path: run_binding_path.to_string_lossy().to_string(),
        managed_evidence_input_path: managed_evidence_input_path.to_string_lossy().to_string(),
        summary: managed_evidence_input.summary,
        touched_files: managed_evidence_input.patch_overview.touched_files,
        diff_artifact_store: derived_diff_artifact
            .as_ref()
            .map(|artifact| artifact.store.clone()),
        diff_artifact_key: derived_diff_artifact
            .as_ref()
            .map(|artifact| artifact.key.clone()),
        created_at: Utc::now().to_rfc3339(),
    };
    write_pretty_json_file(&binding_path, &binding).await?;
    ensure_full_family_patchset_snapshot(&storage_path, &run_binding, &binding.patchset_id).await?;
    Ok(PersistPatchSetResult {
        binding_path,
        binding,
    })
}

async fn persist_patchset_diff_artifact(
    storage_path: &Path,
    audit_bundle: &ManagedAuditBundle,
) -> Result<Option<DerivedPatchSetDiffArtifact>> {
    let Some((diff, file_line_counts)) = render_managed_patchset_diff(audit_bundle) else {
        return Ok(None);
    };
    let storage = LocalStorage::new(storage_path.join("objects"));
    let artifact = storage
        .put_artifact(diff.as_bytes())
        .await
        .context("failed to persist managed patchset diff artifact")?;
    Ok(Some(DerivedPatchSetDiffArtifact {
        store: artifact.store().to_string(),
        key: artifact.key().to_string(),
        file_line_counts,
    }))
}

fn render_managed_patchset_diff(
    audit_bundle: &ManagedAuditBundle,
) -> Option<ManagedPatchDiffRender> {
    let repo_root = audit_bundle.bridge.session_state.working_dir.as_str();
    let mut rendered = Vec::new();
    let mut file_line_counts = BTreeMap::new();

    for invocation in &audit_bundle.bridge.tool_invocations {
        let Some(diff) = render_managed_tool_invocation_diff(invocation, repo_root) else {
            continue;
        };
        let entry = file_line_counts
            .entry(diff.path.clone())
            .or_insert((0_u32, 0_u32));
        entry.0 = entry.0.saturating_add(diff.lines_added);
        entry.1 = entry.1.saturating_add(diff.lines_deleted);
        rendered.push(diff.content);
    }

    (!rendered.is_empty()).then_some((rendered.join("\n"), file_line_counts))
}

fn render_managed_tool_invocation_diff(
    invocation: &ManagedToolInvocation,
    repo_root: &str,
) -> Option<RenderedManagedPatchDiff> {
    let tool_name = invocation.tool_name.as_deref()?;
    let tool_input = invocation.tool_input.as_ref()?;
    match tool_name {
        "Edit" => render_edit_tool_diff(tool_input, repo_root),
        "Write" => render_write_tool_diff(tool_input, repo_root),
        _ => None,
    }
}

fn render_edit_tool_diff(input: &Value, repo_root: &str) -> Option<RenderedManagedPatchDiff> {
    let file_path = input
        .get("file_path")
        .or_else(|| input.get("path"))
        .and_then(Value::as_str)?;
    let old_text = input.get("old_string").and_then(Value::as_str)?;
    let new_text = input.get("new_string").and_then(Value::as_str)?;
    let display_path = diff_display_path(file_path, repo_root);
    Some(RenderedManagedPatchDiff {
        path: display_path.clone(),
        lines_added: count_diff_lines(new_text),
        lines_deleted: count_diff_lines(old_text),
        content: format!(
            "diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@\n{removed}\n{added}\n",
            path = display_path,
            removed = render_diff_body(old_text, '-'),
            added = render_diff_body(new_text, '+'),
        ),
    })
}

fn render_write_tool_diff(input: &Value, repo_root: &str) -> Option<RenderedManagedPatchDiff> {
    let file_path = input
        .get("file_path")
        .or_else(|| input.get("path"))
        .and_then(Value::as_str)?;
    let new_text = input
        .get("content")
        .or_else(|| input.get("file_text"))
        .or_else(|| input.get("text"))
        .and_then(Value::as_str)?;
    let display_path = diff_display_path(file_path, repo_root);
    Some(RenderedManagedPatchDiff {
        path: display_path.clone(),
        lines_added: count_diff_lines(new_text),
        lines_deleted: 0,
        content: format!(
            "diff --git a/{path} b/{path}\nnew file mode 100644\n--- /dev/null\n+++ b/{path}\n@@\n{added}\n",
            path = display_path,
            added = render_diff_body(new_text, '+'),
        ),
    })
}

fn diff_display_path(file_path: &str, repo_root: &str) -> String {
    persistable_touch_hint(file_path, repo_root).unwrap_or_else(|| file_path.replace('\\', "/"))
}

fn render_diff_body(text: &str, prefix: char) -> String {
    let normalized = text.replace("\r\n", "\n");
    let mut segments = normalized.split('\n').collect::<Vec<_>>();
    if normalized.ends_with('\n') {
        segments.pop();
    }
    if segments.is_empty() {
        return prefix.to_string();
    }
    segments
        .into_iter()
        .map(|line| format!("{prefix}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn count_diff_lines(text: &str) -> u32 {
    let normalized = text.replace("\r\n", "\n");
    let mut segments = normalized.split('\n').collect::<Vec<_>>();
    if normalized.ends_with('\n') {
        segments.pop();
    }
    segments.len() as u32
}

#[allow(dead_code)]
async fn persist_decision(args: PersistDecisionArgs) -> Result<()> {
    let result = persist_decision_internal(args).await?;
    print_persist_decision_output(&result.binding_path, &result.binding)?;
    Ok(())
}

async fn persist_decision_internal(args: PersistDecisionArgs) -> Result<PersistDecisionResult> {
    let storage_path = util::try_get_storage_path(None)
        .context("Claude Code managed commands must be run inside a Libra repository")?;
    validate_ai_session_id(&args.ai_session_id)?;

    let run_binding_path = formal_run_binding_path(&storage_path, &args.ai_session_id);
    let run_binding: ClaudeFormalRunBindingArtifact =
        read_typed_json_artifact(&run_binding_path, "formal Claude run binding")
            .await
            .with_context(|| {
                format!(
                    "run a managed Claude Code turn for ai_session_id '{}' first",
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
                    "persist Evidence for ai_session_id '{}' first",
                    args.ai_session_id
                )
            })?;
    validate_evidence_binding_consistency(&evidence_binding, &args.ai_session_id, &run_binding)?;
    if !evidence_binding_objects_exist(&storage_path, &evidence_binding).await? {
        bail!(
            "Claude evidence binding references missing Evidence objects; persist Evidence for ai_session_id '{}' again",
            args.ai_session_id
        );
    }
    if !evidence_binding_patchset_exists(&storage_path, &evidence_binding).await? {
        bail!(
            "Claude evidence binding references a missing PatchSet object; persist the PatchSet for ai_session_id '{}' and then persist Evidence again",
            args.ai_session_id,
        );
    }
    let resolved_patchset_binding =
        load_patchset_binding_for_ai_session(&storage_path, &args.ai_session_id, &run_binding)
            .await?;
    let patchset_binding_artifact_path = resolved_patchset_binding
        .as_ref()
        .map(|(path, _)| path.to_string_lossy().to_string());
    let patchset_id = resolved_patchset_binding
        .as_ref()
        .map(|(_, binding)| binding.patchset_id.clone());
    if evidence_binding.patchset_binding_path.as_deref()
        != patchset_binding_artifact_path.as_deref()
        || evidence_binding.patchset_id.as_deref() != patchset_id.as_deref()
    {
        bail!(
            "Claude evidence binding has stale patchset references; persist Evidence for ai_session_id '{}' again",
            args.ai_session_id
        );
    }
    let decision_input_object_id = build_decision_input_object_id(&args.ai_session_id)?;
    let decision_input_path =
        decision_input_artifact_path(&storage_path, &decision_input_object_id);
    let decision_input_summary = if decision_input_path.exists() {
        let decision_input: PersistedDecisionInputArtifact =
            read_json_artifact(&decision_input_path, "decision input artifact").await?;
        Some((
            decision_input_path.to_string_lossy().to_string(),
            decision_input.summary,
            decision_input.decision_overview.runtime_event_count,
        ))
    } else {
        None
    };
    let decision_type = decision_type_for_binding(&run_binding, &evidence_binding);
    let mut rationale = format!(
        "managed_run_status={}; intent_extraction_status={}; evidence_count={}",
        run_binding.managed_run_status,
        run_binding.intent_extraction_status,
        evidence_binding.evidence_ids.len()
    );
    if let Some((_, summary, runtime_event_count)) = &decision_input_summary {
        rationale.push_str(&format!(
            "; decision_input_runtime_events={runtime_event_count}; decision_input_summary={summary}"
        ));
    }
    if let Some(current_patchset_id) = patchset_id.as_deref() {
        rationale.push_str(&format!("; patchset_id={current_patchset_id}"));
    }
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
            decision_input_summary
                .as_ref()
                .map(|(path, _, _)| path.as_str()),
            patchset_binding_artifact_path.as_deref(),
            patchset_id.as_deref(),
        ) {
            return Ok(PersistDecisionResult {
                binding_path,
                binding: existing,
            });
        }
    }

    let mcp_server = init_local_mcp_server(&storage_path).await?;
    let actor = mcp_server
        .resolve_actor_from_params(Some("system"), Some("claudecode-decision"))
        .map_err(|error| anyhow!("failed to resolve Claude Code decision actor: {error:?}"))?;
    let decision_id = parse_created_id(
        "decision",
        &mcp_server
            .create_decision_impl(
                CreateDecisionParams {
                    run_id: run_binding.run_id.clone(),
                    decision_type: decision_type.to_string(),
                    chosen_patchset_id: patchset_id.clone(),
                    result_commit_sha: None,
                    checkpoint_id: None,
                    rationale: Some(rationale.clone()),
                    tags: None,
                    external_ids: None,
                    actor_kind: Some("system".to_string()),
                    actor_id: Some("claudecode-decision".to_string()),
                },
                actor,
            )
            .await
            .map_err(|error| anyhow!("failed to create Claude decision: {error:?}"))?,
    )?;

    let binding = ClaudeDecisionBindingArtifact {
        schema: "libra.claude_decision_binding.v1".to_string(),
        ai_session_id: args.ai_session_id.clone(),
        provider_session_id: run_binding.provider_session_id,
        run_id: run_binding.run_id.clone(),
        decision_id,
        decision_type: decision_type.to_string(),
        rationale,
        run_binding_path: run_binding_path.to_string_lossy().to_string(),
        evidence_binding_path: evidence_binding_path.to_string_lossy().to_string(),
        decision_input_path: decision_input_summary
            .as_ref()
            .map(|(path, _, _)| path.clone()),
        patchset_binding_path: patchset_binding_artifact_path,
        patchset_id,
        evidence_ids: evidence_binding.evidence_ids.clone(),
        created_at: Utc::now().to_rfc3339(),
    };
    write_pretty_json_file(&binding_path, &binding).await?;
    ensure_full_family_intent_completed(
        &storage_path,
        &args.ai_session_id,
        run_binding.intent_id.as_deref(),
    )
    .await?;
    Ok(PersistDecisionResult {
        binding_path,
        binding,
    })
}

#[allow(dead_code)]
async fn read_artifact(path: &Path) -> Result<ClaudeManagedArtifact> {
    let content = fs::read_to_string(path)
        .await
        .with_context(|| format!("failed to read managed artifact '{}'", path.display()))?;
    serde_json::from_str(&content)
        .with_context(|| format!("failed to parse managed artifact '{}'", path.display()))
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

impl BindingArtifactSchema for ClaudePatchSetBindingArtifact {
    const SCHEMA: &'static str = "libra.claude_patchset_binding.v1";

    fn schema(&self) -> &str {
        &self.schema
    }
}

async fn formal_run_binding_objects_exist(
    storage_path: &Path,
    binding: &ClaudeFormalRunBindingArtifact,
) -> Result<bool> {
    if let Some(plan_id) = binding.plan_id.as_deref()
        && !local_object_exists(storage_path, "plan", plan_id).await?
    {
        return Ok(false);
    }
    Ok(true)
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

async fn evidence_binding_patchset_exists(
    storage_path: &Path,
    binding: &ClaudeEvidenceBindingArtifact,
) -> Result<bool> {
    let Some(patchset_id) = binding.patchset_id.as_deref() else {
        return Ok(true);
    };
    local_object_exists(storage_path, "patchset", patchset_id).await
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
        .unwrap_or_else(|| format!("Claude Code session {}", audit_bundle.provider_session_id))
}

fn derive_formal_task_description(audit_bundle: &ManagedAuditBundle) -> String {
    if let Some(extraction) = audit_bundle.bridge.intent_extraction_artifact.as_ref() {
        return extraction.extraction.intent.problem_statement.clone();
    }

    format!(
        "Formalized Claude Code session {} from managed audit bundle.",
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

fn bridge_plan_steps_from_audit_bundle(audit_bundle: &ManagedAuditBundle) -> Option<Vec<String>> {
    (!audit_bundle.bridge.intent_extraction.plan.is_empty())
        .then(|| audit_bundle.bridge.intent_extraction.plan.clone())
        .or_else(|| extract_bridge_plan_steps(&audit_bundle.raw_artifact.messages))
}

async fn formal_run_binding_matches_current_audit_bundle(
    storage_path: &Path,
    binding: &ClaudeFormalRunBindingArtifact,
    audit_bundle: &ManagedAuditBundle,
    requested_intent_id: Option<&str>,
    summary: &str,
    managed_run_status: &str,
    intent_extraction_status: &str,
) -> Result<bool> {
    if binding.provider_session_id != audit_bundle.provider_session_id {
        return Ok(false);
    }
    if binding.summary != summary
        || binding.managed_run_status != managed_run_status
        || binding.intent_extraction_status != intent_extraction_status
    {
        return Ok(false);
    }
    if let Some(intent_id) = requested_intent_id
        && binding.intent_id.as_deref() != Some(intent_id)
    {
        return Ok(false);
    }

    let provenance = &audit_bundle.bridge.object_candidates.provenance_snapshot;
    if let Some(model) = provenance.model.as_deref()
        && find_matching_provenance(
            storage_path,
            &binding.run_id,
            &provenance.provider,
            model,
            &provenance.parameters,
        )
        .await?
        .is_none()
    {
        return Ok(false);
    }

    if let Some(run_usage_event) = audit_bundle
        .bridge
        .object_candidates
        .run_usage_event
        .as_ref()
    {
        let input_tokens = usage_counter(&run_usage_event.usage, &["input_tokens", "inputTokens"]);
        let output_tokens =
            usage_counter(&run_usage_event.usage, &["output_tokens", "outputTokens"]);
        let cost_usd = run_usage_event
            .usage
            .get("cost_usd")
            .or_else(|| run_usage_event.usage.get("costUSD"))
            .and_then(Value::as_f64);
        if find_matching_run_usage(
            storage_path,
            &binding.run_id,
            input_tokens,
            output_tokens,
            cost_usd,
        )
        .await?
        .is_none()
        {
            return Ok(false);
        }
    }

    let Some(expected_steps) = bridge_plan_steps_from_audit_bundle(audit_bundle) else {
        return Ok(true);
    };
    let Some(plan_id) = binding.plan_id.as_deref() else {
        return Ok(false);
    };
    let plan = match read_tracked_object::<Plan>(storage_path, "plan", plan_id, "formal plan").await
    {
        Ok(plan) => plan,
        Err(_) => return Ok(false),
    };
    let actual_steps = plan
        .steps()
        .iter()
        .map(|step| step.description().to_string())
        .collect::<Vec<_>>();
    Ok(actual_steps == expected_steps)
}

async fn bridge_run_plan_id(
    mcp_server: &LibraMcpServer,
    actor: &git_internal::internal::object::types::ActorRef,
    intent_id: &str,
    audit_bundle: &ManagedAuditBundle,
    context_frame_ids: &[String],
    existing_plan_id: Option<&str>,
) -> Result<Option<String>> {
    let Some(plan_steps) = bridge_plan_steps_from_audit_bundle(audit_bundle) else {
        return Ok(existing_plan_id.map(ToOwned::to_owned));
    };

    let result = mcp_server
        .create_plan_impl(
            CreatePlanParams {
                intent_id: intent_id.to_string(),
                parent_plan_ids: None,
                context_frame_ids: (!context_frame_ids.is_empty())
                    .then_some(context_frame_ids.to_vec()),
                steps: Some(
                    plan_steps
                        .into_iter()
                        .map(|description| PlanStepParams {
                            description,
                            inputs: None,
                            checks: None,
                        })
                        .collect(),
                ),
                tags: None,
                external_ids: None,
                actor_kind: Some("system".to_string()),
                actor_id: Some("claudecode-bridge".to_string()),
            },
            actor.clone(),
        )
        .await
        .map_err(|error| anyhow!("failed to create formal Claude plan: {error:?}"))?;
    let plan_id = parse_created_id("plan", &result)?;
    Ok(Some(plan_id))
}

fn extract_bridge_plan_steps(messages: &[Value]) -> Option<Vec<String>> {
    messages
        .iter()
        .filter(|message| message.get("type").and_then(Value::as_str) == Some("assistant"))
        .filter_map(|message| {
            message
                .get("message")
                .and_then(|inner| inner.get("content"))
                .and_then(Value::as_array)
        })
        .flat_map(|blocks| blocks.iter())
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .find_map(extract_numbered_plan_steps)
}

fn extract_numbered_plan_steps(text: &str) -> Option<Vec<String>> {
    let mut collected = Vec::new();
    let mut expected_number = 1usize;

    for line in text.lines() {
        let Some((number, description)) = parse_numbered_plan_line(line) else {
            continue;
        };

        if number == 1 && !collected.is_empty() && collected.len() < 3 {
            collected.clear();
            expected_number = 1;
        }

        if number != expected_number {
            if number == 1 {
                collected.clear();
                expected_number = 1;
            } else {
                continue;
            }
        }

        collected.push(description);
        expected_number += 1;
    }

    (collected.len() >= 3).then_some(collected)
}

fn parse_numbered_plan_line(line: &str) -> Option<(usize, String)> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    let digits_end = trimmed
        .char_indices()
        .find_map(|(index, ch)| (!ch.is_ascii_digit()).then_some(index))?;
    let number = trimmed[..digits_end].parse::<usize>().ok()?;
    let rest = trimmed[digits_end..].trim_start();
    let rest = rest.strip_prefix('.').or_else(|| rest.strip_prefix(')'))?;
    let description = rest.trim();
    if description.is_empty() {
        return None;
    }

    Some((number, description.to_string()))
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
        "failed" => Some("Claude Code managed session ended in failed state".to_string()),
        "timed_out" => Some("Claude Code managed helper timed out".to_string()),
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
    if binding.patchset_binding_path.is_some() != binding.patchset_id.is_some() {
        bail!("Claude evidence binding patchset fields are inconsistent");
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
    if binding.patchset_binding_path.is_some() != binding.patchset_id.is_some() {
        bail!("Claude decision binding patchset fields are inconsistent");
    }
    Ok(())
}

fn validate_patchset_binding_consistency(
    binding: &ClaudePatchSetBindingArtifact,
    expected_ai_session_id: &str,
    run_binding: &ClaudeFormalRunBindingArtifact,
) -> Result<()> {
    if binding.ai_session_id != expected_ai_session_id {
        bail!(
            "Claude patchset binding belongs to ai session '{}', not '{}'",
            binding.ai_session_id,
            expected_ai_session_id
        );
    }
    if binding.provider_session_id != run_binding.provider_session_id {
        bail!(
            "Claude patchset binding belongs to provider session '{}', not '{}'",
            binding.provider_session_id,
            run_binding.provider_session_id
        );
    }
    if binding.run_id != run_binding.run_id {
        bail!(
            "Claude patchset binding belongs to run '{}', not '{}'",
            binding.run_id,
            run_binding.run_id
        );
    }
    Ok(())
}

fn evidence_binding_matches_expected(
    binding: &ClaudeEvidenceBindingArtifact,
    expected_entries: &[PendingEvidence],
    managed_evidence_input_path: Option<&str>,
    patchset_binding_path: Option<&str>,
    patchset_id: Option<&str>,
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
        && binding.managed_evidence_input_path.as_deref() == managed_evidence_input_path
        && binding.patchset_binding_path.as_deref() == patchset_binding_path
        && binding.patchset_id.as_deref() == patchset_id
}

fn decision_binding_matches_expected(
    binding: &ClaudeDecisionBindingArtifact,
    decision_type: &str,
    rationale: &str,
    evidence_ids: &[String],
    decision_input_path: Option<&str>,
    patchset_binding_path: Option<&str>,
    patchset_id: Option<&str>,
) -> bool {
    binding.decision_type == decision_type
        && binding.rationale == rationale
        && binding.evidence_ids == evidence_ids
        && binding.decision_input_path.as_deref() == decision_input_path
        && binding.patchset_binding_path.as_deref() == patchset_binding_path
        && binding.patchset_id.as_deref() == patchset_id
}

fn patchset_binding_matches_expected(
    binding: &ClaudePatchSetBindingArtifact,
    summary: &str,
    touched_files: &[String],
    managed_evidence_input_path: &Path,
    diff_artifact: Option<&DerivedPatchSetDiffArtifact>,
) -> bool {
    binding.summary == summary
        && binding.touched_files == touched_files
        && binding.managed_evidence_input_path == managed_evidence_input_path.to_string_lossy()
        && binding.diff_artifact_store.as_deref()
            == diff_artifact.map(|artifact| artifact.store.as_str())
        && binding.diff_artifact_key.as_deref()
            == diff_artifact.map(|artifact| artifact.key.as_str())
}

async fn load_patchset_binding_for_ai_session(
    storage_path: &Path,
    ai_session_id: &str,
    run_binding: &ClaudeFormalRunBindingArtifact,
) -> Result<Option<(PathBuf, ClaudePatchSetBindingArtifact)>> {
    let binding_path = patchset_binding_path(storage_path, ai_session_id);
    if !binding_path.exists() {
        return Ok(None);
    }

    let binding: ClaudePatchSetBindingArtifact =
        read_typed_json_artifact(&binding_path, "Claude patchset binding").await?;
    validate_patchset_binding_consistency(&binding, ai_session_id, run_binding)?;
    if !local_object_exists(storage_path, "patchset", &binding.patchset_id).await? {
        bail!(
            "Claude patchset binding references missing PatchSet object; rerun the managed Claude Code flow for ai_session_id '{}' to rebuild it",
            ai_session_id
        );
    }

    Ok(Some((binding_path, binding)))
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
    let total_tokens = ["total_tokens", "totalTokens", "total"]
        .iter()
        .find_map(|key| run_usage_event.usage.get(*key).and_then(Value::as_u64))
        .unwrap_or_else(|| input_tokens.saturating_add(output_tokens));
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

#[allow(dead_code)]
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
            plan_id: binding.plan_id.clone(),
        })
        .context("failed to serialize bridge-run output")?
    );
    Ok(())
}

#[allow(dead_code)]
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

#[allow(dead_code)]
fn print_persist_patchset_output(
    path: &Path,
    binding: &ClaudePatchSetBindingArtifact,
) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(&PersistPatchSetCommandOutput {
            ok: true,
            command_mode: "persist-patchset",
            ai_session_id: binding.ai_session_id.clone(),
            run_id: binding.run_id.clone(),
            patchset_id: binding.patchset_id.clone(),
            binding_path: path.to_string_lossy().to_string(),
        })
        .context("failed to serialize persist-patchset output")?
    );
    Ok(())
}

#[allow(dead_code)]
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

fn build_provider_session_object_id(provider_session_id: &str) -> Result<String> {
    validate_provider_session_id(provider_session_id)?;
    Ok(format!("claude_provider_session__{provider_session_id}"))
}

fn provider_session_artifact_path(storage_path: &Path, object_id: &str) -> PathBuf {
    storage_path
        .join(PROVIDER_SESSIONS_DIR)
        .join(format!("{object_id}.json"))
}

#[allow(dead_code)]
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::internal::{
        ai::tools::context::{StepStatus, UserInputAnswer, UserInputResponse},
        tui::AppEvent,
    };

    #[test]
    fn summarize_run_usage_falls_back_to_input_plus_output_when_total_is_missing() {
        let summary = summarize_run_usage(&ManagedRunUsageEvent {
            run_id: "run-1".to_string(),
            thread_id: "thread-1".to_string(),
            at: "2026-03-29T00:00:00Z".to_string(),
            usage: json!({
                "input_tokens": 12,
                "output_tokens": 8
            }),
        });

        assert_eq!(
            summary,
            "usage input_tokens=12; output_tokens=8; total_tokens=20"
        );
    }

    #[test]
    fn summarize_run_usage_preserves_explicit_total_tokens() {
        let summary = summarize_run_usage(&ManagedRunUsageEvent {
            run_id: "run-1".to_string(),
            thread_id: "thread-1".to_string(),
            at: "2026-03-29T00:00:00Z".to_string(),
            usage: json!({
                "input_tokens": 12,
                "output_tokens": 8,
                "total_tokens": 99
            }),
        });

        assert_eq!(
            summary,
            "usage input_tokens=12; output_tokens=8; total_tokens=99"
        );
    }

    #[test]
    fn build_plan_update_cell_creates_initial_plan_cell() {
        let plan = vec!["Inspect files".to_string(), "Implement change".to_string()];
        let cell = build_plan_update_cell(7, 2, None, &plan).expect("expected plan cell");

        assert_eq!(cell.call_id, "claudecode-plan-7-2");
        assert_eq!(
            cell.explanation.as_deref(),
            Some("Claude proposed a structured execution plan.")
        );
        assert!(!cell.is_running);
        assert_eq!(cell.steps.len(), 2);
        assert!(
            cell.steps
                .iter()
                .all(|step| step.status == StepStatus::Pending)
        );
    }

    #[test]
    fn build_plan_update_cell_skips_unchanged_plan() {
        let plan = vec!["Inspect files".to_string(), "Implement change".to_string()];
        assert!(build_plan_update_cell(7, 0, Some(&plan), &plan).is_none());
    }

    #[test]
    fn reset_for_new_conversation_restores_initial_session_control() {
        let args = ClaudecodeCodeArgs {
            working_dir: PathBuf::from("/tmp/repo"),
            model: Some("claude-sonnet-4-6".to_string()),
            permission_mode: Some("plan".to_string()),
            continue_session: true,
            ..ClaudecodeCodeArgs::default()
        };
        let chat_args = build_chat_managed_args(&args);
        let initial_control = ManagedSessionControl::from_chat_args(&chat_args);
        let mut runtime = ClaudecodeTuiRuntime {
            driver: build_test_tui_driver(chat_args),
            session_control: Arc::new(Mutex::new(ManagedSessionControl::followup(
                "provider-session-123".to_string(),
                "acceptEdits",
                false,
            ))),
            latest_structured_plan: Arc::new(Mutex::new(Some(vec![
                "Inspect files".to_string(),
                "Implement change".to_string(),
            ]))),
        };

        runtime.reset_for_new_conversation();

        assert_eq!(runtime.session_control(), initial_control);
        assert!(lock_unpoisoned(&runtime.latest_structured_plan).is_none());
    }

    #[tokio::test]
    async fn plan_checkpoint_prompt_emits_assistant_text_before_request() {
        let (app_tx, mut app_rx) = tokio::sync::mpsc::unbounded_channel();
        let (user_input_tx, mut user_input_rx) = tokio::sync::mpsc::unbounded_channel();

        let decision_task = tokio::spawn(async move {
            prompt_for_visible_plan_checkpoint_decision(
                &app_tx,
                &user_input_tx,
                7,
                0,
                Some("Need your approval before executing.".to_string()),
            )
            .await
        });

        let event = app_rx.recv().await.expect("assistant text event");
        match event {
            AppEvent::InsertHistoryCell { turn_id, cell } => {
                assert_eq!(turn_id, 7);
                let assistant = cell
                    .as_any()
                    .downcast_ref::<AssistantHistoryCell>()
                    .expect("assistant history cell");
                assert_eq!(assistant.content, "Need your approval before executing.");
                assert!(!assistant.is_streaming);
            }
            other => {
                panic!("expected assistant history event before plan checkpoint, got {other:?}")
            }
        }

        let request = user_input_rx.recv().await.expect("plan checkpoint request");
        assert_eq!(request.call_id, "claudecode-plan-checkpoint-7-0");
        let _ = request.response_tx.send(UserInputResponse {
            answers: HashMap::from([(
                "plan_action".to_string(),
                UserInputAnswer {
                    answers: vec!["Approve".to_string()],
                },
            )]),
        });

        let decision = decision_task.await.expect("task join").expect("decision");
        assert_eq!(decision, PlanCheckpointDecision::Approve);
    }
}
