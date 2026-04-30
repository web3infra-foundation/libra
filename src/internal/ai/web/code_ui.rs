//! Code UI projection server helpers for exposing AI thread state to the local web UI.
//!
//! Boundary: this file translates internal projection records into HTTP/websocket
//! views; it does not execute tools or mutate repository state. Projection resolver
//! tests cover missing threads, event ordering, and replayed snapshots.

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use anyhow::{Context, anyhow};
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::{Mutex, RwLock, broadcast};
use uuid::Uuid;

use crate::internal::ai::{
    projection::{PlanHeadRef, ThreadBundle},
    runtime::hardening::SecretRedactor,
};

const DEFAULT_BROWSER_CONTROLLER_LEASE_SECS: i64 = 120;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum CodeUiSessionStatus {
    #[default]
    Idle,
    Thinking,
    ExecutingTool,
    AwaitingInteraction,
    Completed,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct CodeUiCapabilities {
    pub message_input: bool,
    pub streaming_text: bool,
    pub plan_updates: bool,
    pub tool_calls: bool,
    pub patchsets: bool,
    pub interactive_approvals: bool,
    pub structured_questions: bool,
    pub provider_session_resume: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct CodeUiProviderInfo {
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default)]
    pub managed: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum CodeUiControllerKind {
    #[default]
    None,
    Browser,
    /// Local automation writer. Automation requires both the process-level
    /// `X-Libra-Control-Token` and the lease-level `X-Code-Controller-Token`;
    /// existing browser controllers keep using only the lease token for
    /// backward compatibility.
    Automation,
    Tui,
    Cli,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct CodeUiControllerState {
    pub kind: CodeUiControllerKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_label: Option<String>,
    pub can_write: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lease_expires_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub loopback_only: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum CodeUiTranscriptEntryKind {
    #[default]
    UserMessage,
    AssistantMessage,
    ToolCall,
    PlanSummary,
    Diff,
    InfoNote,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CodeUiTranscriptEntry {
    pub id: String,
    pub kind: CodeUiTranscriptEntryKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default)]
    pub streaming: bool,
    #[serde(default = "default_metadata")]
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum CodeUiInteractionKind {
    #[default]
    Approval,
    SandboxApproval,
    RequestUserInput,
    IntentReviewChoice,
    PostPlanChoice,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum CodeUiInteractionStatus {
    #[default]
    Pending,
    Resolved,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct CodeUiInteractionOption {
    pub id: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CodeUiInteractionRequest {
    pub id: String,
    pub kind: CodeUiInteractionKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default)]
    pub options: Vec<CodeUiInteractionOption>,
    pub status: CodeUiInteractionStatus,
    #[serde(default = "default_metadata")]
    pub metadata: serde_json::Value,
    pub requested_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum CodeUiApplyToFuture {
    #[default]
    No,
    AcceptAll,
    DeclineAll,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CodeUiInteractionResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approved: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub apply_to_future: Option<CodeUiApplyToFuture>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_option: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default)]
    pub answers: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CodeUiPlanStep {
    pub step: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CodeUiPlanSnapshot {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub status: String,
    #[serde(default)]
    pub steps: Vec<CodeUiPlanStep>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CodeUiTaskSnapshot {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CodeUiToolCallSnapshot {
    pub id: String,
    pub tool_name: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CodeUiPatchChange {
    pub path: String,
    pub change_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CodeUiPatchsetSnapshot {
    pub id: String,
    pub status: String,
    #[serde(default)]
    pub changes: Vec<CodeUiPatchChange>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeUiSessionSnapshot {
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    pub working_dir: String,
    pub provider: CodeUiProviderInfo,
    pub capabilities: CodeUiCapabilities,
    pub controller: CodeUiControllerState,
    pub status: CodeUiSessionStatus,
    pub transcript: Vec<CodeUiTranscriptEntry>,
    pub plans: Vec<CodeUiPlanSnapshot>,
    pub tasks: Vec<CodeUiTaskSnapshot>,
    pub tool_calls: Vec<CodeUiToolCallSnapshot>,
    pub patchsets: Vec<CodeUiPatchsetSnapshot>,
    pub interactions: Vec<CodeUiInteractionRequest>,
    pub updated_at: DateTime<Utc>,
}

impl Default for CodeUiSessionSnapshot {
    fn default() -> Self {
        Self {
            session_id: Uuid::new_v4().to_string(),
            thread_id: None,
            working_dir: String::new(),
            provider: CodeUiProviderInfo::default(),
            capabilities: CodeUiCapabilities::default(),
            controller: CodeUiControllerState::default(),
            status: CodeUiSessionStatus::Idle,
            transcript: Vec::new(),
            plans: Vec::new(),
            tasks: Vec::new(),
            tool_calls: Vec::new(),
            patchsets: Vec::new(),
            interactions: Vec::new(),
            updated_at: Utc::now(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeUiEventEnvelope {
    pub seq: u64,
    #[serde(rename = "type")]
    pub event_type: String,
    pub at: DateTime<Utc>,
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeUiControllerAttachRequest {
    pub client_id: String,
    #[serde(default = "default_controller_attach_kind")]
    pub kind: CodeUiControllerKind,
}

fn default_controller_attach_kind() -> CodeUiControllerKind {
    CodeUiControllerKind::Browser
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeUiControllerAttachResponse {
    pub controller_token: String,
    pub lease_expires_at: DateTime<Utc>,
    pub controller: CodeUiControllerState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeUiControllerDetachRequest {
    pub client_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeUiMessageRequest {
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeUiAckResponse {
    pub accepted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct CodeUiDiagnosticsPorts {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub web: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CodeUiDiagnostics {
    pub pid: u32,
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    pub status: CodeUiSessionStatus,
    pub controller: CodeUiControllerState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ports: Option<CodeUiDiagnosticsPorts>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_interaction_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

impl CodeUiDiagnostics {
    fn redact(mut self, redactor: &SecretRedactor) -> Self {
        redact_string(&mut self.provider, redactor);
        redact_option_string(&mut self.model, redactor);
        redact_option_string(&mut self.thread_id, redactor);
        redact_option_string(&mut self.controller.owner_label, redactor);
        redact_option_string(&mut self.controller.reason, redactor);
        redact_option_string(&mut self.log_file, redactor);
        redact_option_string(&mut self.active_interaction_id, redactor);
        redact_option_string(&mut self.last_error, redactor);
        self
    }
}

fn redact_string(value: &mut String, redactor: &SecretRedactor) {
    let redacted = redactor.redact(value.as_str());
    *value = redacted;
}

fn redact_option_string(value: &mut Option<String>, redactor: &SecretRedactor) {
    if let Some(value) = value.as_mut() {
        redact_string(value, redactor);
    }
}

fn default_metadata() -> serde_json::Value {
    json!({})
}

#[derive(Debug)]
pub struct CodeUiSession {
    snapshot: RwLock<CodeUiSessionSnapshot>,
    tx: broadcast::Sender<CodeUiEventEnvelope>,
    next_seq: AtomicU64,
}

impl CodeUiSession {
    pub fn new(snapshot: CodeUiSessionSnapshot) -> Arc<Self> {
        let (tx, _) = broadcast::channel(256);
        Arc::new(Self {
            snapshot: RwLock::new(snapshot),
            tx,
            next_seq: AtomicU64::new(1),
        })
    }

    pub async fn snapshot(&self) -> CodeUiSessionSnapshot {
        self.snapshot.read().await.clone()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<CodeUiEventEnvelope> {
        self.tx.subscribe()
    }

    pub async fn mutate<F>(&self, event_type: &str, f: F)
    where
        F: FnOnce(&mut CodeUiSessionSnapshot),
    {
        let snapshot = {
            let mut snapshot = self.snapshot.write().await;
            f(&mut snapshot);
            snapshot.updated_at = Utc::now();
            snapshot.clone()
        };
        self.broadcast_snapshot(event_type, &snapshot);
    }

    pub async fn replace_snapshot(&self, event_type: &str, snapshot: CodeUiSessionSnapshot) {
        {
            let mut current = self.snapshot.write().await;
            *current = snapshot;
        }
        let snapshot = self.snapshot().await;
        self.broadcast_snapshot(event_type, &snapshot);
    }

    pub async fn set_controller_state(&self, controller: CodeUiControllerState) {
        self.mutate("controller_changed", |snapshot| {
            snapshot.controller = controller;
        })
        .await;
    }

    pub async fn set_status(&self, status: CodeUiSessionStatus) {
        self.mutate("status_changed", |snapshot| {
            snapshot.status = status;
        })
        .await;
    }

    pub async fn upsert_transcript_entry(&self, entry: CodeUiTranscriptEntry) {
        self.mutate("session_updated", |snapshot| {
            upsert_by_id(&mut snapshot.transcript, entry, |item| item.id.as_str());
        })
        .await;
    }

    pub async fn append_assistant_delta(&self, entry_id: &str, delta: &str) {
        self.mutate("session_updated", |snapshot| {
            if let Some(entry) = snapshot
                .transcript
                .iter_mut()
                .find(|item| item.id == entry_id)
            {
                let content = entry.content.get_or_insert_with(String::new);
                content.push_str(delta);
                entry.streaming = true;
                entry.updated_at = Utc::now();
            }
        })
        .await;
    }

    pub async fn upsert_interaction(&self, request: CodeUiInteractionRequest) {
        self.mutate("session_updated", |snapshot| {
            upsert_by_id(&mut snapshot.interactions, request, |item| item.id.as_str());
        })
        .await;
    }

    pub async fn resolve_interaction(&self, interaction_id: &str) {
        let interaction_id = interaction_id.to_string();
        self.mutate("session_updated", |snapshot| {
            if let Some(interaction) = snapshot
                .interactions
                .iter_mut()
                .find(|item| item.id == interaction_id)
            {
                interaction.status = CodeUiInteractionStatus::Resolved;
                interaction.resolved_at = Some(Utc::now());
            }
        })
        .await;
    }

    pub async fn clear_interaction(&self, interaction_id: &str) {
        let interaction_id = interaction_id.to_string();
        self.mutate("session_updated", |snapshot| {
            snapshot
                .interactions
                .retain(|interaction| interaction.id != interaction_id);
        })
        .await;
    }

    pub async fn upsert_plan(&self, plan: CodeUiPlanSnapshot) {
        self.mutate("session_updated", |snapshot| {
            upsert_by_id(&mut snapshot.plans, plan, |item| item.id.as_str());
        })
        .await;
    }

    pub async fn upsert_task(&self, task: CodeUiTaskSnapshot) {
        self.mutate("session_updated", |snapshot| {
            upsert_by_id(&mut snapshot.tasks, task, |item| item.id.as_str());
        })
        .await;
    }

    pub async fn upsert_tool_call(&self, tool_call: CodeUiToolCallSnapshot) {
        self.mutate("session_updated", |snapshot| {
            upsert_by_id(&mut snapshot.tool_calls, tool_call, |item| item.id.as_str());
        })
        .await;
    }

    pub async fn upsert_patchset(&self, patchset: CodeUiPatchsetSnapshot) {
        self.mutate("session_updated", |snapshot| {
            upsert_by_id(&mut snapshot.patchsets, patchset, |item| item.id.as_str());
        })
        .await;
    }

    pub async fn emit_current_snapshot(&self, event_type: &str) {
        let snapshot = self.snapshot().await;
        self.broadcast_snapshot(event_type, &snapshot);
    }

    fn broadcast_snapshot(&self, event_type: &str, snapshot: &CodeUiSessionSnapshot) {
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        let event = CodeUiEventEnvelope {
            seq,
            event_type: event_type.to_string(),
            at: Utc::now(),
            data: serde_json::to_value(snapshot).unwrap_or_else(|_| json!({})),
        };
        let _ = self.tx.send(event);
    }
}

fn upsert_by_id<T, F>(items: &mut Vec<T>, incoming: T, id_fn: F)
where
    F: Fn(&T) -> &str,
{
    let incoming_id = id_fn(&incoming).to_string();
    if let Some(existing) = items.iter_mut().find(|item| id_fn(item) == incoming_id) {
        *existing = incoming;
    } else {
        items.push(incoming);
    }
}

#[async_trait]
pub trait CodeUiReadModel: Send + Sync {
    fn session(&self) -> Arc<CodeUiSession>;

    async fn snapshot(&self) -> CodeUiSessionSnapshot {
        self.session().snapshot().await
    }

    fn subscribe(&self) -> broadcast::Receiver<CodeUiEventEnvelope> {
        self.session().subscribe()
    }
}

#[async_trait]
pub trait CodeUiCommandAdapter: Send + Sync {
    fn capabilities(&self) -> CodeUiCapabilities;

    async fn submit_message(&self, text: String) -> anyhow::Result<()>;

    async fn respond_interaction(
        &self,
        interaction_id: &str,
        response: CodeUiInteractionResponse,
    ) -> anyhow::Result<()>;

    async fn cancel_turn(&self) -> anyhow::Result<()> {
        Err(anyhow!(
            "This libra code session does not support turn cancel"
        ))
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

pub trait CodeUiProviderAdapter: CodeUiReadModel + CodeUiCommandAdapter {}

impl<T> CodeUiProviderAdapter for T where T: CodeUiReadModel + CodeUiCommandAdapter {}

#[derive(Debug, Clone)]
pub enum CodeUiInitialController {
    Unclaimed,
    Fixed {
        kind: CodeUiControllerKind,
        owner_label: String,
        reason: Option<String>,
    },
    LocalTui {
        owner_label: String,
        reason: Option<String>,
    },
}

#[derive(Debug)]
struct FixedController {
    kind: CodeUiControllerKind,
    owner_label: String,
    reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ControllerLease {
    pub kind: CodeUiControllerKind,
    pub client_id: String,
    pub token: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug)]
struct CodeUiControllerRuntimeState {
    fixed: Option<FixedController>,
    local_tui_owner: Option<FixedController>,
    active_lease: Option<ControllerLease>,
}

#[derive(Clone)]
pub struct CodeUiRuntimeHandle {
    adapter: Arc<dyn CodeUiProviderAdapter>,
    browser_write_enabled: bool,
    automation_write_enabled: bool,
    controller_state: Arc<Mutex<CodeUiControllerRuntimeState>>,
    controller_lease_duration: Duration,
}

impl CodeUiRuntimeHandle {
    pub async fn build(
        adapter: Arc<dyn CodeUiProviderAdapter>,
        browser_write_enabled: bool,
        initial_controller: CodeUiInitialController,
    ) -> Arc<Self> {
        Self::build_with_control(adapter, browser_write_enabled, false, initial_controller).await
    }

    pub async fn build_with_control(
        adapter: Arc<dyn CodeUiProviderAdapter>,
        browser_write_enabled: bool,
        automation_write_enabled: bool,
        initial_controller: CodeUiInitialController,
    ) -> Arc<Self> {
        let (fixed, local_tui_owner) = match initial_controller {
            CodeUiInitialController::Unclaimed => (None, None),
            CodeUiInitialController::Fixed {
                kind,
                owner_label,
                reason,
            } => (
                Some(FixedController {
                    kind,
                    owner_label,
                    reason,
                }),
                None,
            ),
            CodeUiInitialController::LocalTui {
                owner_label,
                reason,
            } => (
                None,
                Some(FixedController {
                    kind: CodeUiControllerKind::Tui,
                    owner_label,
                    reason,
                }),
            ),
        };

        let handle = Arc::new(Self {
            adapter,
            browser_write_enabled,
            automation_write_enabled,
            controller_state: Arc::new(Mutex::new(CodeUiControllerRuntimeState {
                fixed,
                local_tui_owner,
                active_lease: None,
            })),
            controller_lease_duration: Duration::seconds(DEFAULT_BROWSER_CONTROLLER_LEASE_SECS),
        });
        handle.sync_controller_snapshot().await;
        handle
    }

    pub fn adapter(&self) -> Arc<dyn CodeUiProviderAdapter> {
        self.adapter.clone()
    }

    pub async fn snapshot(&self) -> CodeUiSessionSnapshot {
        self.adapter.snapshot().await
    }

    pub async fn diagnostics(&self) -> CodeUiDiagnostics {
        self.sync_controller_snapshot().await;
        let snapshot = self.snapshot().await;
        let redactor = SecretRedactor::default_runtime();
        CodeUiDiagnostics {
            pid: std::process::id(),
            provider: snapshot.provider.provider,
            model: snapshot.provider.model,
            thread_id: snapshot.thread_id,
            status: snapshot.status,
            controller: snapshot.controller,
            ports: None,
            log_file: std::env::var("LIBRA_LOG_FILE")
                .ok()
                .filter(|value| !value.trim().is_empty()),
            active_interaction_id: snapshot
                .interactions
                .iter()
                .find(|interaction| interaction.status == CodeUiInteractionStatus::Pending)
                .map(|interaction| interaction.id.clone()),
            last_error: None,
        }
        .redact(&redactor)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<CodeUiEventEnvelope> {
        self.adapter.subscribe()
    }

    pub async fn attach_browser_controller(
        &self,
        client_id: &str,
    ) -> Result<CodeUiControllerAttachResponse, CodeUiApiError> {
        self.attach_controller(CodeUiControllerKind::Browser, client_id)
            .await
    }

    /// Request a controller lease.
    ///
    /// `kind` may be `Browser` or `Automation`. `Automation` requires
    /// `automation_write_enabled` to be true (i.e. `--control write`).
    ///
    /// Errors:
    /// - `BROWSER_CONTROL_DISABLED` / `CONTROL_DISABLED` when the kind is not enabled.
    /// - `CONTROLLER_CONFLICT` when another client already holds an active lease.
    /// - `INVALID_CONTROLLER_KIND` for `None`, `Tui`, or `Cli`.
    ///
    /// The lease TTL defaults to `DEFAULT_BROWSER_CONTROLLER_LEASE_SECS` (120s).
    /// Renew by calling again with the same `client_id`.
    pub async fn attach_controller(
        &self,
        kind: CodeUiControllerKind,
        client_id: &str,
    ) -> Result<CodeUiControllerAttachResponse, CodeUiApiError> {
        match kind {
            CodeUiControllerKind::Browser if !self.browser_write_enabled => {
                return Err(CodeUiApiError::forbidden(
                    "BROWSER_CONTROL_DISABLED",
                    "Browser control is disabled for this code session",
                ));
            }
            CodeUiControllerKind::Automation if !self.automation_write_enabled => {
                return Err(CodeUiApiError::forbidden(
                    "CONTROL_DISABLED",
                    "Local TUI automation write control is not enabled; start with --control write",
                ));
            }
            CodeUiControllerKind::Browser | CodeUiControllerKind::Automation => {}
            _ => {
                return Err(CodeUiApiError::bad_request(
                    "INVALID_CONTROLLER_KIND",
                    format!("Controller kind '{}' cannot attach", kind.as_str()),
                ));
            }
        }

        let mut state = self.controller_state.lock().await;
        if let Some(fixed) = state.fixed.as_ref() {
            return Err(CodeUiApiError::conflict(
                "CONTROLLER_CONFLICT",
                format!(
                    "The active controller is {} ({})",
                    fixed.kind.as_str(),
                    fixed.owner_label
                ),
            ));
        }

        let now = Utc::now();
        if state
            .active_lease
            .as_ref()
            .is_some_and(|lease| lease.expires_at <= now)
        {
            state.active_lease = None;
        }

        let lease = if let Some(existing) = state.active_lease.as_mut() {
            if existing.client_id != client_id || existing.kind != kind {
                return Err(CodeUiApiError::conflict(
                    "CONTROLLER_CONFLICT",
                    format!(
                        "Another {} currently controls this session",
                        existing.kind.as_str()
                    ),
                ));
            }
            existing.expires_at = now + self.controller_lease_duration;
            existing.clone()
        } else {
            let lease = ControllerLease {
                kind,
                client_id: client_id.to_string(),
                token: Uuid::new_v4().to_string(),
                expires_at: now + self.controller_lease_duration,
            };
            state.active_lease = Some(lease.clone());
            lease
        };
        drop(state);

        self.sync_controller_snapshot().await;

        Ok(CodeUiControllerAttachResponse {
            controller_token: lease.token,
            lease_expires_at: lease.expires_at,
            controller: self.current_controller_state().await,
        })
    }

    pub async fn detach_browser_controller(
        &self,
        client_id: &str,
        token: &str,
    ) -> Result<(), CodeUiApiError> {
        self.detach_controller(CodeUiControllerKind::Browser, client_id, token, false)
            .await
    }

    /// Release an active controller lease.
    ///
    /// `force` is reserved for local TUI reclaim (e.g. `/control reclaim`).
    /// When `force` is `false`, both `client_id` and `token` must match the
    /// active lease. HTTP handlers should not expose `force` to remote clients.
    ///
    /// Thin wrappers (`detach_browser_controller`) hard-code `kind` and `force`
    /// to preserve backward compatibility for existing browser callers.
    pub async fn detach_controller(
        &self,
        kind: CodeUiControllerKind,
        client_id: &str,
        token: &str,
        force: bool,
    ) -> Result<(), CodeUiApiError> {
        let mut state = self.controller_state.lock().await;
        let Some(existing) = state.active_lease.as_ref() else {
            return Ok(());
        };
        if existing.kind != kind {
            return Ok(());
        }
        if !force && (existing.client_id != client_id || existing.token != token) {
            return Err(CodeUiApiError::forbidden(
                "INVALID_CONTROLLER_TOKEN",
                "The controller token does not match the active controller",
            ));
        }
        state.active_lease = None;
        drop(state);
        self.sync_controller_snapshot().await;
        Ok(())
    }

    pub async fn submit_message(
        &self,
        token: Option<&str>,
        text: String,
    ) -> Result<(), CodeUiApiError> {
        self.ensure_controller_write_access(token).await?;
        self.adapter
            .submit_message(text)
            .await
            .map_err(CodeUiApiError::unsupported_from_error)
    }

    pub async fn respond_interaction(
        &self,
        token: Option<&str>,
        interaction_id: &str,
        response: CodeUiInteractionResponse,
    ) -> Result<(), CodeUiApiError> {
        self.ensure_controller_write_access(token).await?;
        self.adapter
            .respond_interaction(interaction_id, response)
            .await
            .map_err(CodeUiApiError::unsupported_from_error)
    }

    pub async fn cancel_turn(&self, token: Option<&str>) -> Result<(), CodeUiApiError> {
        self.ensure_controller_write_access(token).await?;
        self.adapter
            .cancel_turn()
            .await
            .map_err(CodeUiApiError::unsupported_from_error)
    }

    pub async fn shutdown(&self) -> anyhow::Result<()> {
        self.adapter.shutdown().await
    }

    /// Validate a controller token and return the active lease.
    ///
    /// Checks that the token is present, non-empty, matches the active lease,
    /// and that the lease has not expired. Expired leases are cleared on check.
    ///
    /// Errors:
    /// - `MISSING_CONTROLLER_TOKEN` when `token` is missing or empty.
    /// - `CONTROLLER_CONFLICT` when no lease is active.
    /// - `INVALID_CONTROLLER_TOKEN` when the token does not match the active lease.
    ///
    /// Thin wrappers (`ensure_browser_write_access`) hard-code the kind check
    /// for backward compatibility.
    pub async fn ensure_controller_write_access(
        &self,
        token: Option<&str>,
    ) -> Result<ControllerLease, CodeUiApiError> {
        let Some(token) = token.filter(|token| !token.trim().is_empty()) else {
            return Err(CodeUiApiError::forbidden(
                "MISSING_CONTROLLER_TOKEN",
                "A controller token is required for write operations",
            ));
        };

        let lease = {
            let mut state = self.controller_state.lock().await;
            let now = Utc::now();
            if state
                .active_lease
                .as_ref()
                .is_some_and(|lease| lease.expires_at <= now)
            {
                state.active_lease = None;
            }

            let Some(lease) = state.active_lease.as_mut() else {
                return Err(CodeUiApiError::conflict(
                    "CONTROLLER_CONFLICT",
                    "No client currently controls this session",
                ));
            };
            if lease.token != token {
                return Err(CodeUiApiError::forbidden(
                    "INVALID_CONTROLLER_TOKEN",
                    "The controller token does not match the active controller",
                ));
            }
            lease.expires_at = now + self.controller_lease_duration;
            lease.clone()
        };
        self.sync_controller_snapshot().await;
        Ok(lease)
    }

    pub async fn reclaim_local_tui_controller(&self) -> Result<(), CodeUiApiError> {
        let mut state = self.controller_state.lock().await;
        if state.local_tui_owner.is_none() {
            return Err(CodeUiApiError::conflict(
                "CONTROLLER_CONFLICT",
                "This session does not have a local TUI controller to reclaim",
            ));
        }
        state.active_lease = None;
        drop(state);
        self.sync_controller_snapshot().await;
        Ok(())
    }

    async fn current_controller_state(&self) -> CodeUiControllerState {
        let mut state = self.controller_state.lock().await;
        let now = Utc::now();
        if state
            .active_lease
            .as_ref()
            .is_some_and(|lease| lease.expires_at <= now)
        {
            state.active_lease = None;
        }

        if let Some(lease) = state.active_lease.as_ref() {
            return CodeUiControllerState {
                kind: lease.kind,
                owner_label: Some(lease.client_id.clone()),
                can_write: true,
                lease_expires_at: Some(lease.expires_at),
                reason: None,
                loopback_only: true,
            };
        }

        if let Some(local) = state.local_tui_owner.as_ref() {
            return CodeUiControllerState {
                kind: local.kind,
                owner_label: Some(local.owner_label.clone()),
                can_write: false,
                lease_expires_at: None,
                reason: local.reason.clone(),
                loopback_only: true,
            };
        }

        if let Some(fixed) = state.fixed.as_ref() {
            return CodeUiControllerState {
                kind: fixed.kind,
                owner_label: Some(fixed.owner_label.clone()),
                can_write: false,
                lease_expires_at: None,
                reason: fixed.reason.clone(),
                loopback_only: true,
            };
        }

        CodeUiControllerState {
            kind: CodeUiControllerKind::None,
            owner_label: None,
            can_write: false,
            lease_expires_at: None,
            reason: if self.browser_write_enabled {
                Some("No controller attached".to_string())
            } else {
                Some("Browser control is disabled".to_string())
            },
            loopback_only: true,
        }
    }

    async fn sync_controller_snapshot(&self) {
        let controller = self.current_controller_state().await;
        self.adapter
            .session()
            .set_controller_state(controller)
            .await;
    }
}

#[derive(Debug, Clone)]
pub struct CodeUiApiError {
    pub status: u16,
    pub code: String,
    pub message: String,
}

impl CodeUiApiError {
    pub fn not_found(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status: 404,
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn conflict(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status: 409,
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn forbidden(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status: 403,
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn bad_request(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status: 400,
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn unsupported_from_error(error: anyhow::Error) -> Self {
        if let Some(control_error) =
            error.downcast_ref::<crate::internal::tui::control::TuiControlError>()
        {
            return Self {
                status: control_error.status(),
                code: control_error.code().to_string(),
                message: control_error.message(),
            };
        }

        Self {
            status: 422,
            code: "UNSUPPORTED_OPERATION".to_string(),
            message: error.to_string(),
        }
    }

    pub fn unavailable() -> Self {
        Self::not_found(
            "CODE_UI_UNAVAILABLE",
            "No active libra code session is available",
        )
    }
}

#[derive(Clone)]
pub struct ReadOnlyCodeUiAdapter {
    session: Arc<CodeUiSession>,
    capabilities: CodeUiCapabilities,
}

impl ReadOnlyCodeUiAdapter {
    pub fn new(session: Arc<CodeUiSession>, capabilities: CodeUiCapabilities) -> Arc<Self> {
        Arc::new(Self {
            session,
            capabilities,
        })
    }
}

#[async_trait]
impl CodeUiReadModel for ReadOnlyCodeUiAdapter {
    fn session(&self) -> Arc<CodeUiSession> {
        self.session.clone()
    }
}

#[async_trait]
impl CodeUiCommandAdapter for ReadOnlyCodeUiAdapter {
    fn capabilities(&self) -> CodeUiCapabilities {
        self.capabilities.clone()
    }

    async fn submit_message(&self, _text: String) -> anyhow::Result<()> {
        Err(anyhow!(
            "This libra code session is read-only from the browser"
        ))
    }

    async fn respond_interaction(
        &self,
        _interaction_id: &str,
        _response: CodeUiInteractionResponse,
    ) -> anyhow::Result<()> {
        Err(anyhow!(
            "This libra code session is read-only from the browser"
        ))
    }
}

pub fn initial_snapshot(
    working_dir: impl Into<String>,
    provider: CodeUiProviderInfo,
    capabilities: CodeUiCapabilities,
) -> CodeUiSessionSnapshot {
    CodeUiSessionSnapshot {
        session_id: Uuid::new_v4().to_string(),
        thread_id: None,
        working_dir: working_dir.into(),
        provider,
        capabilities,
        controller: CodeUiControllerState::default(),
        status: CodeUiSessionStatus::Idle,
        transcript: Vec::new(),
        plans: Vec::new(),
        tasks: Vec::new(),
        tool_calls: Vec::new(),
        patchsets: Vec::new(),
        interactions: Vec::new(),
        updated_at: Utc::now(),
    }
}

pub fn snapshot_from_thread_bundle(
    working_dir: impl Into<String>,
    provider: CodeUiProviderInfo,
    capabilities: CodeUiCapabilities,
    bundle: &ThreadBundle,
) -> CodeUiSessionSnapshot {
    let mut snapshot = initial_snapshot(working_dir, provider, capabilities);
    apply_thread_bundle_to_snapshot(&mut snapshot, bundle);
    snapshot
}

pub fn apply_thread_bundle_to_snapshot(
    snapshot: &mut CodeUiSessionSnapshot,
    bundle: &ThreadBundle,
) {
    let thread_id = bundle.thread.thread_id.to_string();
    snapshot.session_id = thread_id.clone();
    snapshot.thread_id = Some(thread_id);
    snapshot.status = if bundle.scheduler.active_run_id.is_some() {
        CodeUiSessionStatus::ExecutingTool
    } else if bundle.scheduler.active_task_id.is_some() {
        CodeUiSessionStatus::Thinking
    } else {
        CodeUiSessionStatus::Idle
    };
    snapshot.plans = code_ui_plan_snapshots(&bundle.scheduler.selected_plan_ids);
    snapshot.tasks = bundle
        .scheduler
        .active_task_id
        .map(|task_id| CodeUiTaskSnapshot {
            id: task_id.to_string(),
            title: None,
            status: "active".to_string(),
            details: Some("Active scheduler task".to_string()),
            updated_at: bundle.scheduler.updated_at,
        })
        .into_iter()
        .collect();
    snapshot.updated_at = bundle.thread.updated_at.max(bundle.scheduler.updated_at);
}

fn code_ui_plan_snapshots(plan_heads: &[PlanHeadRef]) -> Vec<CodeUiPlanSnapshot> {
    plan_heads
        .iter()
        .map(|plan| CodeUiPlanSnapshot {
            id: plan.plan_id.to_string(),
            title: None,
            summary: Some(format!("Selected plan ordinal {}", plan.ordinal)),
            status: "selected".to_string(),
            steps: Vec::new(),
            updated_at: Utc::now(),
        })
        .collect()
}

pub fn browser_controller_token_from_headers(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get("x-code-controller-token")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub fn snapshot_from_event(event: &CodeUiEventEnvelope) -> anyhow::Result<CodeUiSessionSnapshot> {
    serde_json::from_value(event.data.clone()).context("failed to parse Code UI event snapshot")
}

impl CodeUiControllerKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Browser => "browser",
            Self::Automation => "automation",
            Self::Tui => "tui",
            Self::Cli => "cli",
        }
    }
}

pub fn ensure_session_updated_event(
    snapshot: &CodeUiSessionSnapshot,
) -> anyhow::Result<CodeUiEventEnvelope> {
    Ok(CodeUiEventEnvelope {
        seq: 0,
        event_type: "session_updated".to_string(),
        at: Utc::now(),
        data: serde_json::to_value(snapshot)
            .map_err(|error| anyhow!("failed to serialize Code UI snapshot: {error}"))?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_session() -> Arc<CodeUiSession> {
        CodeUiSession::new(initial_snapshot(
            "/tmp/libra",
            CodeUiProviderInfo {
                provider: "test".to_string(),
                model: Some("test-model".to_string()),
                mode: Some("test".to_string()),
                managed: false,
            },
            CodeUiCapabilities {
                message_input: true,
                ..CodeUiCapabilities::default()
            },
        ))
    }

    #[test]
    fn attach_request_defaults_to_browser_kind() {
        let request: CodeUiControllerAttachRequest =
            serde_json::from_value(serde_json::json!({ "clientId": "browser-1" })).unwrap();

        assert_eq!(request.kind, CodeUiControllerKind::Browser);
    }

    #[derive(Clone)]
    struct RecordingCodeUiAdapter {
        session: Arc<CodeUiSession>,
        submitted_messages: Arc<Mutex<Vec<String>>>,
    }

    impl RecordingCodeUiAdapter {
        fn new(session: Arc<CodeUiSession>) -> Arc<Self> {
            Arc::new(Self {
                session,
                submitted_messages: Arc::new(Mutex::new(Vec::new())),
            })
        }

        async fn submitted_messages(&self) -> Vec<String> {
            self.submitted_messages.lock().await.clone()
        }
    }

    #[async_trait]
    impl CodeUiReadModel for RecordingCodeUiAdapter {
        fn session(&self) -> Arc<CodeUiSession> {
            self.session.clone()
        }
    }

    #[async_trait]
    impl CodeUiCommandAdapter for RecordingCodeUiAdapter {
        fn capabilities(&self) -> CodeUiCapabilities {
            CodeUiCapabilities {
                message_input: true,
                interactive_approvals: true,
                ..CodeUiCapabilities::default()
            }
        }

        async fn submit_message(&self, text: String) -> anyhow::Result<()> {
            self.submitted_messages.lock().await.push(text);
            Ok(())
        }

        async fn respond_interaction(
            &self,
            _interaction_id: &str,
            _response: CodeUiInteractionResponse,
        ) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn browser_controller_attach_and_detach_updates_snapshot() {
        let session = test_session();
        let runtime = CodeUiRuntimeHandle::build(
            ReadOnlyCodeUiAdapter::new(session.clone(), CodeUiCapabilities::default()),
            true,
            CodeUiInitialController::Unclaimed,
        )
        .await;

        let attach = runtime
            .attach_browser_controller("browser-a")
            .await
            .expect("browser controller should attach");
        assert_eq!(attach.controller.kind, CodeUiControllerKind::Browser);
        assert!(attach.controller.can_write);

        let snapshot = runtime.snapshot().await;
        assert_eq!(snapshot.controller.kind, CodeUiControllerKind::Browser);
        assert_eq!(
            snapshot.controller.owner_label.as_deref(),
            Some("browser-a")
        );

        runtime
            .detach_browser_controller("browser-a", &attach.controller_token)
            .await
            .expect("browser controller should detach");

        let detached_snapshot = runtime.snapshot().await;
        assert_eq!(
            detached_snapshot.controller.kind,
            CodeUiControllerKind::None
        );
        assert!(!detached_snapshot.controller.can_write);
    }

    #[tokio::test]
    async fn expired_browser_controller_lease_is_cleaned_before_attach() {
        let session = test_session();
        let runtime = CodeUiRuntimeHandle::build(
            ReadOnlyCodeUiAdapter::new(session.clone(), CodeUiCapabilities::default()),
            true,
            CodeUiInitialController::Unclaimed,
        )
        .await;

        let expired_attach = runtime
            .attach_browser_controller("browser-a")
            .await
            .expect("browser controller should attach");
        {
            let mut state = runtime.controller_state.lock().await;
            let lease = state
                .active_lease
                .as_mut()
                .expect("browser lease should be active");
            lease.expires_at = Utc::now() - Duration::seconds(1);
        }

        let replacement_attach = runtime
            .attach_browser_controller("browser-b")
            .await
            .expect("expired lease should not block a new browser");

        assert_ne!(
            expired_attach.controller_token,
            replacement_attach.controller_token
        );
        let snapshot = runtime.snapshot().await;
        assert_eq!(snapshot.controller.kind, CodeUiControllerKind::Browser);
        assert_eq!(
            snapshot.controller.owner_label.as_deref(),
            Some("browser-b")
        );

        let stale_error = runtime
            .ensure_controller_write_access(Some(&expired_attach.controller_token))
            .await
            .expect_err("stale token must not keep write access");
        assert_eq!(stale_error.status, 403);
        assert_eq!(stale_error.code, "INVALID_CONTROLLER_TOKEN");
    }

    #[tokio::test]
    async fn concurrent_browser_attach_allows_only_one_owner() {
        let runtime = CodeUiRuntimeHandle::build(
            ReadOnlyCodeUiAdapter::new(test_session(), CodeUiCapabilities::default()),
            true,
            CodeUiInitialController::Unclaimed,
        )
        .await;

        let runtime_a = runtime.clone();
        let runtime_b = runtime.clone();
        let (first, second) = tokio::join!(
            async move { runtime_a.attach_browser_controller("browser-a").await },
            async move { runtime_b.attach_browser_controller("browser-b").await },
        );

        let successes = [first.as_ref().ok(), second.as_ref().ok()]
            .into_iter()
            .flatten()
            .count();
        let conflicts = [first.as_ref().err(), second.as_ref().err()]
            .into_iter()
            .flatten()
            .filter(|error| error.status == 409 && error.code == "CONTROLLER_CONFLICT")
            .count();

        assert_eq!(successes, 1);
        assert_eq!(conflicts, 1);
    }

    #[tokio::test]
    async fn invalid_detach_does_not_clear_browser_controller() {
        let runtime = CodeUiRuntimeHandle::build(
            ReadOnlyCodeUiAdapter::new(test_session(), CodeUiCapabilities::default()),
            true,
            CodeUiInitialController::Unclaimed,
        )
        .await;
        let attach = runtime
            .attach_browser_controller("browser-a")
            .await
            .expect("browser controller should attach");

        let error = runtime
            .detach_browser_controller("browser-b", &attach.controller_token)
            .await
            .expect_err("wrong client id must not detach");
        assert_eq!(error.status, 403);
        assert_eq!(error.code, "INVALID_CONTROLLER_TOKEN");

        let snapshot = runtime.snapshot().await;
        assert_eq!(snapshot.controller.kind, CodeUiControllerKind::Browser);
        assert_eq!(
            snapshot.controller.owner_label.as_deref(),
            Some("browser-a")
        );
    }

    #[tokio::test]
    async fn concurrent_detach_and_submit_message_leaves_stale_token_rejected() {
        let adapter = RecordingCodeUiAdapter::new(test_session());
        let runtime =
            CodeUiRuntimeHandle::build(adapter.clone(), true, CodeUiInitialController::Unclaimed)
                .await;
        let attach = runtime
            .attach_browser_controller("browser-a")
            .await
            .expect("browser controller should attach");

        let detach_token = attach.controller_token.clone();
        let submit_token = attach.controller_token.clone();
        let runtime_for_detach = runtime.clone();
        let runtime_for_submit = runtime.clone();
        let (detach_result, submit_result) = tokio::join!(
            async move {
                runtime_for_detach
                    .detach_browser_controller("browser-a", &detach_token)
                    .await
            },
            async move {
                runtime_for_submit
                    .submit_message(Some(&submit_token), "hello".to_string())
                    .await
            },
        );

        detach_result.expect("detach should succeed");
        if let Err(error) = submit_result {
            assert!(
                error.status == 403 || error.status == 409,
                "submit should either win the race or fail authorization, got {error:?}"
            );
        }

        let stale_error = runtime
            .submit_message(Some(&attach.controller_token), "after detach".to_string())
            .await
            .expect_err("detached token must not submit again");
        assert_eq!(stale_error.status, 409);
        assert_eq!(stale_error.code, "CONTROLLER_CONFLICT");
        assert!(adapter.submitted_messages().await.len() <= 1);
    }

    #[tokio::test]
    async fn fixed_controller_rejects_browser_attach() {
        let runtime = CodeUiRuntimeHandle::build(
            ReadOnlyCodeUiAdapter::new(test_session(), CodeUiCapabilities::default()),
            true,
            CodeUiInitialController::Fixed {
                kind: CodeUiControllerKind::Cli,
                owner_label: "CLI".to_string(),
                reason: Some("Terminal control is active".to_string()),
            },
        )
        .await;

        let error = runtime
            .attach_browser_controller("browser-a")
            .await
            .expect_err("fixed controller must block browser attach");
        assert_eq!(error.status, 409);
        assert_eq!(error.code, "CONTROLLER_CONFLICT");
    }

    #[tokio::test]
    async fn local_tui_owner_allows_automation_takeover_and_reclaim() {
        let runtime = CodeUiRuntimeHandle::build_with_control(
            ReadOnlyCodeUiAdapter::new(test_session(), CodeUiCapabilities::default()),
            false,
            true,
            CodeUiInitialController::LocalTui {
                owner_label: "Terminal UI".to_string(),
                reason: Some("Local TUI owns this session".to_string()),
            },
        )
        .await;

        let initial = runtime.snapshot().await;
        assert_eq!(initial.controller.kind, CodeUiControllerKind::Tui);
        assert!(!initial.controller.can_write);

        let attach = runtime
            .attach_controller(CodeUiControllerKind::Automation, "automation-a")
            .await
            .expect("automation should attach");
        assert_eq!(attach.controller.kind, CodeUiControllerKind::Automation);
        assert!(attach.controller.can_write);

        let lease = runtime
            .ensure_controller_write_access(Some(&attach.controller_token))
            .await
            .expect("automation token should authorize writes");
        assert_eq!(lease.kind, CodeUiControllerKind::Automation);

        runtime
            .reclaim_local_tui_controller()
            .await
            .expect("local TUI should reclaim controller");

        let reclaimed = runtime.snapshot().await;
        assert_eq!(reclaimed.controller.kind, CodeUiControllerKind::Tui);
        assert!(!reclaimed.controller.can_write);

        let stale = runtime
            .ensure_controller_write_access(Some(&attach.controller_token))
            .await
            .expect_err("automation token must be invalid after reclaim");
        assert_eq!(stale.status, 409);
        assert_eq!(stale.code, "CONTROLLER_CONFLICT");
    }

    #[tokio::test]
    async fn automation_attach_is_disabled_without_control_mode() {
        let runtime = CodeUiRuntimeHandle::build(
            ReadOnlyCodeUiAdapter::new(test_session(), CodeUiCapabilities::default()),
            false,
            CodeUiInitialController::LocalTui {
                owner_label: "Terminal UI".to_string(),
                reason: None,
            },
        )
        .await;

        let error = runtime
            .attach_controller(CodeUiControllerKind::Automation, "automation-a")
            .await
            .expect_err("automation should be disabled by default");
        assert_eq!(error.status, 403);
        assert_eq!(error.code, "CONTROL_DISABLED");
    }

    #[tokio::test]
    async fn diagnostics_exposes_snapshot_summary_without_secret_material() {
        let session = test_session();
        session
            .set_controller_state(CodeUiControllerState {
                kind: CodeUiControllerKind::Automation,
                owner_label: Some("local-script".to_string()),
                can_write: true,
                lease_expires_at: Some(Utc::now() + Duration::seconds(60)),
                reason: None,
                loopback_only: true,
            })
            .await;
        session
            .upsert_interaction(CodeUiInteractionRequest {
                id: "interaction-1".to_string(),
                kind: CodeUiInteractionKind::Approval,
                title: Some("Approve command".to_string()),
                status: CodeUiInteractionStatus::Pending,
                requested_at: Utc::now(),
                ..CodeUiInteractionRequest::default()
            })
            .await;
        let runtime = CodeUiRuntimeHandle::build_with_control(
            ReadOnlyCodeUiAdapter::new(session, CodeUiCapabilities::default()),
            false,
            true,
            CodeUiInitialController::Unclaimed,
        )
        .await;
        let attach = runtime
            .attach_controller(CodeUiControllerKind::Automation, "local-script")
            .await
            .expect("automation should attach");
        runtime
            .ensure_controller_write_access(Some(&attach.controller_token))
            .await
            .expect("automation token should refresh lease");

        let diagnostics = runtime.diagnostics().await;
        let serialized = serde_json::to_string(&diagnostics).unwrap();

        assert_eq!(diagnostics.provider, "test");
        assert_eq!(diagnostics.model.as_deref(), Some("test-model"));
        assert_eq!(
            diagnostics.active_interaction_id.as_deref(),
            Some("interaction-1")
        );
        assert_eq!(
            diagnostics.controller.kind,
            CodeUiControllerKind::Automation
        );
        assert!(!serialized.contains(&attach.controller_token));
        assert!(!serialized.contains("x-libra-control-token"));
        assert!(!serialized.contains("authorization"));
        assert!(!serialized.contains("api_key"));
    }
}
