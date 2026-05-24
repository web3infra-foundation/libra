//! Headless web-only runtime for non-Codex providers.
//!
//! `--web-only --provider <X>` (X != codex) used to fall back to a read-only
//! placeholder snapshot, leaving the browser unable to drive the agent. This
//! module provides the minimum-viable replacement: a [`HeadlessCodeRuntime`]
//! that owns a [`CodeUiSession`], spawns a tokio task per submitted message
//! that runs the agent's tool loop, and streams the model's output back into
//! the session transcript.
//!
//! # v0 scope (Phase 3 minimum)
//!
//! - `submitMessage` queues a user message and starts a turn — the agent runs
//!   the standard `run_tool_loop_with_history_and_observer` and the assistant
//!   reply lands in the live snapshot, streamed delta-by-delta.
//! - `cancelTurn` aborts the in-flight turn and marks the assistant entry as
//!   cancelled.
//! - The runtime reuses the caller-provided [`ToolRegistry`] and
//!   [`ToolLoopConfig`], so the same allow-list / hooks / sandbox boundaries
//!   that protect the TUI agent also apply here.
//!
//! # Phase 3 follow-up target
//!
//! - IntentSpec / Plan workflow integration. The TUI's Phase 0/1 review loop
//!   is deeply coupled to the ratatui [`crate::internal::tui::app::App`]; this
//!   runtime treats every browser submit as a single direct turn instead.
//! - `approval` / `sandbox_approval` interactions are routed through the
//!   shared approval channel and surfaced as interactive browser interactions.
//! - Multi-turn conversation history persistence via `SessionStore`.
//!
//! These follow-ups are explicitly called out in
//! `docs/improvement/web.md` and will land in subsequent phases.

use std::{collections::HashMap, sync::Arc, sync::atomic::AtomicU64, sync::atomic::Ordering};

use anyhow::anyhow;
use async_trait::async_trait;
use chrono::Utc;
use tokio::{
    sync::{mpsc, oneshot, Mutex},
    task::JoinHandle,
};

use super::code_ui::{
    CodeUiCapabilities, CodeUiCommandAdapter, CodeUiInteractionKind, CodeUiInteractionOption,
    CodeUiInteractionRequest, CodeUiInteractionResponse, CodeUiInteractionStatus,
    CodeUiReadModel, CodeUiSession, CodeUiSessionStatus, CodeUiTranscriptEntry,
    CodeUiTranscriptEntryKind,
    CodeUiApplyToFuture,
};
use crate::internal::ai::{
    agent::runtime::run_tool_loop_with_history_and_observer,
    completion::{
        CompletionError, CompletionModel, CompletionStreamEvent, CompletionUsage,
        CompletionUsageSummary, Message,
    },
    sandbox::{ExecApprovalRequest, ReviewDecision},
    tools::ToolRegistry,
    tools::context::{UserInputAnswer, UserInputQuestion, UserInputRequest, UserInputResponse},
};

/// Capabilities advertised by the headless runtime.
///
/// `messageInput` and `streamingText` are the only flags this v0 implementation
/// can actually deliver. Plan / patchset / interaction surfaces light up once
/// the corresponding workflow integrations are wired in.
pub fn headless_capabilities() -> CodeUiCapabilities {
    CodeUiCapabilities {
        message_input: true,
        streaming_text: true,
        plan_updates: false,
        tool_calls: true,
        patchsets: false,
        interactive_approvals: true,
        structured_questions: true,
        provider_session_resume: false,
    }
}

struct PendingHeadlessUserInput {
    questions: Vec<UserInputQuestion>,
    response_tx: oneshot::Sender<UserInputResponse>,
}

struct PendingHeadlessExecApproval {
    request: ExecApprovalRequest,
}

/// Adapter that runs an agent tool loop in response to browser-driven messages.
///
/// Generic over a [`CompletionModel`] so each provider (Ollama, OpenAI, Gemini,
/// …) can plug in its own client. The model is held inside an `Arc<Mutex<…>>`
/// so the spawned turn task can take exclusive access while the next submit
/// waits in the queue.
/// Bookkeeping for the active turn so the runtime can finalize its
/// transcript entry on cancel and so the spawned task can avoid clobbering
/// a successor turn's slot when it eventually clears itself out.
struct InFlightTurn {
    /// Stable id assigned per-turn; the spawned task uses it as a generation
    /// counter when releasing its slot at the end of the turn.
    id: u64,
    /// Transcript entry that needs `streaming -> false` + `status` finalized
    /// when the turn ends (success, error, or cancellation).
    assistant_entry_id: String,
    handle: JoinHandle<()>,
}

pub struct HeadlessCodeRuntime<M: CompletionModel + 'static> {
    session: Arc<CodeUiSession>,
    capabilities: CodeUiCapabilities,
    /// Conversation history accumulated across turns.
    history: Arc<Mutex<Vec<Message>>>,
    model: Arc<M>,
    registry: Arc<ToolRegistry>,
    config_factory:
        Arc<dyn Fn() -> super::super::agent::runtime::tool_loop::ToolLoopConfig + Send + Sync>,
    /// Active turn slot. `submit_message` holds the lock while it spawns and
    /// stores the new turn so two concurrent submits can never both see an
    /// empty slot. `cancel_turn` and the spawned task itself acquire the
    /// lock to release / finalize the slot.
    in_flight: Arc<Mutex<Option<InFlightTurn>>>,
    /// Monotonic turn id; used by spawned tasks to detect that a successor
    /// turn has claimed the slot before they cleared their own entry.
    next_turn_id: Arc<AtomicU64>,
    /// Pending `request_user_input` flows keyed by tool call id.
    pending_user_inputs: Arc<Mutex<HashMap<String, PendingHeadlessUserInput>>>,
    /// Pending exec approval flows keyed by tool call id.
    pending_exec_approvals: Arc<Mutex<HashMap<String, PendingHeadlessExecApproval>>>,
}

impl<M> HeadlessCodeRuntime<M>
where
    M: CompletionModel + Clone + Send + Sync + 'static,
    M::Response: CompletionUsage,
{
    /// Build a new headless runtime around an existing [`CodeUiSession`].
    ///
    /// `config_factory` is invoked once per turn so per-call `usage_context`
    /// fields (turn id, etc.) can be refreshed without mutating the original
    /// config in place.
    pub fn new(
        session: Arc<CodeUiSession>,
        capabilities: CodeUiCapabilities,
        model: M,
        registry: Arc<ToolRegistry>,
        mut user_input_rx: mpsc::UnboundedReceiver<UserInputRequest>,
        mut exec_approval_rx: mpsc::UnboundedReceiver<ExecApprovalRequest>,
        config_factory: Arc<
            dyn Fn() -> super::super::agent::runtime::tool_loop::ToolLoopConfig + Send + Sync,
        >,
    ) -> Arc<Self> {
        let runtime = Arc::new(Self {
            session,
            capabilities,
            history: Arc::new(Mutex::new(Vec::new())),
            model: Arc::new(model),
            registry,
            config_factory,
            in_flight: Arc::new(Mutex::new(None)),
            next_turn_id: Arc::new(AtomicU64::new(1)),
            pending_user_inputs: Arc::new(Mutex::new(HashMap::new())),
            pending_exec_approvals: Arc::new(Mutex::new(HashMap::new())),
        });

        let listener = runtime.clone();
        tokio::spawn(async move {
            listener
                .run_user_and_exec_approval_request_listener(
                    &mut user_input_rx,
                    &mut exec_approval_rx,
                )
                .await;
        });

        runtime
    }
}

#[async_trait]
impl<M> CodeUiReadModel for HeadlessCodeRuntime<M>
where
    M: CompletionModel + Clone + Send + Sync + 'static,
    M::Response: CompletionUsage,
{
    fn session(&self) -> Arc<CodeUiSession> {
        self.session.clone()
    }
}

#[async_trait]
impl<M> CodeUiCommandAdapter for HeadlessCodeRuntime<M>
where
    M: CompletionModel + Clone + Send + Sync + 'static,
    M::Response: CompletionUsage,
{
    fn capabilities(&self) -> CodeUiCapabilities {
        self.capabilities.clone()
    }

    async fn submit_message(&self, text: String) -> anyhow::Result<()> {
        if text.trim().is_empty() {
            return Err(anyhow!("Empty messages are not accepted by libra code"));
        }

        // Hold the in_flight lock continuously across the check + spawn + slot
        // assignment. Two concurrent submits cannot both observe an empty slot
        // because the second waiter blocks on `lock().await` until the first
        // finishes installing its task.
        let mut slot = self.in_flight.lock().await;
        if slot.as_ref().is_some_and(|turn| !turn.handle.is_finished()) {
            return Err(anyhow!(
                "A turn is already running; cancel it or wait for the assistant to finish before sending another message"
            ));
        }

        let user_entry_id = format!("user-{}", uuid::Uuid::new_v4());
        let assistant_entry_id = format!("assistant-{}", uuid::Uuid::new_v4());
        let now = Utc::now();
        let user_entry = CodeUiTranscriptEntry {
            id: user_entry_id,
            kind: CodeUiTranscriptEntryKind::UserMessage,
            title: None,
            content: Some(text.clone()),
            status: Some("submitted".to_string()),
            streaming: false,
            metadata: serde_json::json!({}),
            created_at: now,
            updated_at: now,
        };
        let assistant_entry = CodeUiTranscriptEntry {
            id: assistant_entry_id.clone(),
            kind: CodeUiTranscriptEntryKind::AssistantMessage,
            title: None,
            content: Some(String::new()),
            status: Some("streaming".to_string()),
            streaming: true,
            metadata: serde_json::json!({}),
            created_at: now,
            updated_at: now,
        };
        self.session.upsert_transcript_entry(user_entry).await;
        self.session.upsert_transcript_entry(assistant_entry).await;
        self.session.set_status(CodeUiSessionStatus::Thinking).await;

        let session = self.session.clone();
        let history = self.history.clone();
        let model = self.model.clone();
        let registry = self.registry.clone();
        let config = (self.config_factory)();
        let in_flight_for_task = self.in_flight.clone();
        let user_text = text;
        let task_assistant_entry_id = assistant_entry_id.clone();
        let turn_id = self
            .next_turn_id
            .fetch_add(1, Ordering::Relaxed);

        let task = tokio::spawn(async move {
            let mut observer = HeadlessTurnObserver {
                session: session.clone(),
                assistant_entry_id: task_assistant_entry_id.clone(),
            };

            let prior_history = {
                let guard = history.lock().await;
                guard.clone()
            };

            let result = run_tool_loop_with_history_and_observer(
                model.as_ref(),
                prior_history,
                user_text,
                registry.as_ref(),
                config,
                &mut observer,
            )
            .await;

            match result {
                Ok(turn) => {
                    {
                        let mut guard = history.lock().await;
                        *guard = turn.history;
                    }
                    finalize_assistant_entry(
                        &session,
                        &task_assistant_entry_id,
                        &turn.final_text,
                        "completed",
                    )
                    .await;
                    session.set_status(CodeUiSessionStatus::Idle).await;
                }
                Err(error) => {
                    let message = format_completion_error(&error);
                    finalize_assistant_entry(&session, &task_assistant_entry_id, &message, "error")
                        .await;
                    session.set_status(CodeUiSessionStatus::Error).await;
                }
            }

            // Only clear the slot if it still holds *our* turn — a successor
            // submit may have already claimed the slot via cancel + resubmit
            // and we would otherwise wipe its handle out from under it.
            let mut slot = in_flight_for_task.lock().await;
            if slot.as_ref().is_some_and(|t| t.id == turn_id) {
                *slot = None;
            }
        });

        *slot = Some(InFlightTurn {
            id: turn_id,
            assistant_entry_id,
            handle: task,
        });
        Ok(())
    }

    async fn respond_interaction(
        &self,
        interaction_id: &str,
        response: CodeUiInteractionResponse,
    ) -> anyhow::Result<()> {
        if let Some(mut pending) = {
            let mut pending = self.pending_exec_approvals.lock().await;
            pending.remove(interaction_id)
        } {
            let decision = review_decision_from_interaction_response(response)?;
            pending.request.response_tx.send(decision).map_err(|_| {
                anyhow!("The pending execution approval request is no longer awaiting a response")
            })?;

            self.session
                .resolve_interaction(interaction_id)
                .await;
            self.session
                .set_status(CodeUiSessionStatus::ExecutingTool)
                .await;
            return Ok(());
        }

        let pending = {
            let mut pending = self.pending_user_inputs.lock().await;
            pending
                .remove(interaction_id)
                .ok_or_else(|| anyhow!("Unknown pending interaction: {interaction_id}"))?
        };

        let user_input_response =
            user_input_response_from_code_ui_request(&pending.questions, response)?;
        pending
            .response_tx
            .send(user_input_response)
            .map_err(|_| anyhow!("The pending user input request is no longer awaiting a response"))?;

        self.session
            .resolve_interaction(interaction_id)
            .await;
        self.session
            .set_status(CodeUiSessionStatus::ExecutingTool)
            .await;
        Ok(())
    }

    async fn cancel_turn(&self) -> anyhow::Result<()> {
        let active = {
            let mut slot = self.in_flight.lock().await;
            slot.take()
        };
        if let Some(turn) = active {
            if !turn.handle.is_finished() {
                turn.handle.abort();
            }
            // Finalize the streaming assistant entry so the browser sees a
            // terminal state instead of a perpetually streaming row.
            finalize_assistant_entry(
                &self.session,
                &turn.assistant_entry_id,
                "(turn cancelled by user)",
                "cancelled",
            )
            .await;
        }
        self.session.set_status(CodeUiSessionStatus::Idle).await;
        self.clear_pending_user_inputs().await;
        Ok(())
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        let active = {
            let mut slot = self.in_flight.lock().await;
            slot.take()
        };
        if let Some(turn) = active {
            turn.handle.abort();
            finalize_assistant_entry(
                &self.session,
                &turn.assistant_entry_id,
                "(libra code shutting down)",
                "cancelled",
            )
            .await;
        }
        self.clear_pending_user_inputs().await;
        Ok(())
    }
}

impl<M> HeadlessCodeRuntime<M>
where
    M: CompletionModel + Clone + Send + Sync + 'static,
    M::Response: CompletionUsage,
{
    async fn run_user_and_exec_approval_request_listener(
        &self,
        user_input_rx: &mut mpsc::UnboundedReceiver<UserInputRequest>,
        exec_approval_rx: &mut mpsc::UnboundedReceiver<ExecApprovalRequest>,
    ) {
        let mut user_input_open = true;
        let mut exec_approval_open = true;

        while user_input_open || exec_approval_open {
            tokio::select! {
                request = user_input_rx.recv(), if user_input_open => {
                    if let Some(request) = request {
                        self.handle_user_input_request(request).await;
                    } else {
                        user_input_open = false;
                    }
                }
                request = exec_approval_rx.recv(), if exec_approval_open => {
                    if let Some(request) = request {
                        self.handle_exec_approval_request(request).await;
                    } else {
                        exec_approval_open = false;
                    }
                }
            }
        }
    }

    async fn handle_user_input_request(&self, request: UserInputRequest) {
        let interaction_id = request.call_id.clone();
        let questions_for_ui = request
            .questions
            .iter()
            .map(request_user_input_question_to_metadata)
            .collect::<Vec<_>>();

        {
            let mut pending = self.pending_user_inputs.lock().await;
            pending.insert(
                interaction_id.clone(),
                PendingHeadlessUserInput {
                    questions: request.questions,
                    response_tx: request.response_tx,
                },
            );
        }

        let interaction = CodeUiInteractionRequest {
            id: interaction_id,
            kind: crate::internal::ai::web::code_ui::CodeUiInteractionKind::RequestUserInput,
            title: Some("User input required".to_string()),
            description: None,
            prompt: None,
            options: Vec::new(),
            status: crate::internal::ai::web::code_ui::CodeUiInteractionStatus::Pending,
            metadata: serde_json::json!({ "questions": questions_for_ui }),
            requested_at: Utc::now(),
            resolved_at: None,
        };

        self.session.upsert_interaction(interaction).await;
        self.session
            .set_status(CodeUiSessionStatus::AwaitingInteraction)
            .await;
    }

    async fn handle_exec_approval_request(&self, request: ExecApprovalRequest) {
        let interaction_id = request.call_id.clone();
        let interaction_kind = if request.sandbox_label == "outside sandbox" {
            CodeUiInteractionKind::SandboxApproval
        } else {
            CodeUiInteractionKind::Approval
        };

        let interaction = interaction_request_for_exec_approval(
            interaction_id.clone(),
            interaction_kind,
            &request,
        );

        {
            let mut pending = self.pending_exec_approvals.lock().await;
            pending.insert(
                interaction_id.clone(),
                PendingHeadlessExecApproval { request },
            );
        }

        self.session.upsert_interaction(interaction).await;
        self.session
            .set_status(CodeUiSessionStatus::AwaitingInteraction)
            .await;
    }

    async fn clear_pending_user_inputs(&self) {
        let pending_ids = {
            let mut pending = self.pending_user_inputs.lock().await;
            let ids = pending.keys().cloned().collect::<Vec<_>>();
            pending.clear();
            ids
        };

        for interaction_id in pending_ids {
            self.session.clear_interaction(&interaction_id).await;
        }

        let pending_ids = {
            let mut pending = self.pending_exec_approvals.lock().await;
            let ids = pending.keys().cloned().collect::<Vec<_>>();
            pending.clear();
            ids
        };

        for interaction_id in pending_ids {
            self.session.clear_interaction(&interaction_id).await;
        }
    }
}

// `CodeUiProviderAdapter` is automatically implemented for any `T` that
// satisfies `CodeUiReadModel + CodeUiCommandAdapter` via the blanket impl in
// `code_ui.rs`. `Arc<HeadlessCodeRuntime<M>>` picks that up directly because
// `HeadlessCodeRuntime` itself implements both halves.

/// Replace the streaming assistant entry with the finalized text, mark the
/// streaming flag false, and stamp the supplied status (`completed`,
/// `error`, or `cancelled`).
async fn finalize_assistant_entry(
    session: &Arc<CodeUiSession>,
    entry_id: &str,
    text: &str,
    status: &str,
) {
    let entry_id = entry_id.to_string();
    let text = text.to_string();
    let status = status.to_string();
    session
        .mutate("session_updated", |snapshot| {
            if let Some(entry) = snapshot.transcript.iter_mut().find(|e| e.id == entry_id) {
                entry.content = Some(text.clone());
                entry.status = Some(status.clone());
                entry.streaming = false;
                entry.updated_at = Utc::now();
            }
        })
        .await;
}

fn format_completion_error(error: &CompletionError) -> String {
    format!("Agent turn failed: {error}")
}

fn request_user_input_question_to_metadata(question: &UserInputQuestion) -> serde_json::Value {
    let has_options = question
        .options
        .as_ref()
        .is_some_and(|options| !options.is_empty());

    let options = question
        .options
        .as_ref()
        .map(|options| {
            options
                .iter()
                .map(|option| serde_json::json!({ "id": option.label, "label": option.label }))
                .collect::<Vec<_>>()
        })
        .filter(|options| !options.is_empty())
        .unwrap_or_default();

    let metadata = serde_json::json!({
        "id": question.id,
        "prompt": question.question,
        "kind": if has_options { "single" } else { "text" },
        "options": options,
    });

    metadata
}

fn interaction_request_for_exec_approval(
    interaction_id: String,
    kind: CodeUiInteractionKind,
    request: &ExecApprovalRequest,
) -> CodeUiInteractionRequest {
    let command = request.command.clone();
    let reason = request
        .reason
        .clone()
        .unwrap_or_else(|| String::from("Command execution"))
        .trim()
        .to_string();

    let title = match kind {
        CodeUiInteractionKind::Approval => "Approve command execution",
        CodeUiInteractionKind::SandboxApproval => "Approve sandbox-executed command",
        _ => "Approval request",
    };

    let prompt = format!("{command}");
    CodeUiInteractionRequest {
        id: interaction_id,
        kind,
        title: Some(title.to_string()),
        description: Some(reason),
        prompt: Some(prompt),
        options: vec![
            CodeUiInteractionOption {
                id: "approve".to_string(),
                label: "Approve".to_string(),
                description: Some("Allow this command once".to_string()),
            },
            CodeUiInteractionOption {
                id: "deny".to_string(),
                label: "Deny".to_string(),
                description: Some("Skip this command".to_string()),
            },
            CodeUiInteractionOption {
                id: "abort".to_string(),
                label: "Abort".to_string(),
                description: Some("Cancel this tool run immediately".to_string()),
            },
        ],
        status: CodeUiInteractionStatus::Pending,
        metadata: exec_approval_request_to_metadata(request),
        requested_at: Utc::now(),
        resolved_at: None,
    }
}

fn exec_approval_request_to_metadata(request: &ExecApprovalRequest) -> serde_json::Value {
    serde_json::json!({
        "command": request.command,
        "cwd": request.cwd.display().to_string(),
        "reason": request.reason,
        "is_retry": request.is_retry,
        "sandbox_label": request.sandbox_label,
        "network_access": request.network_access,
        "writable_roots": request
            .writable_roots
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>(),
        "cache_disabled_reason": request.cache_disabled_reason,
    })
}

fn review_decision_from_interaction_response(
    response: CodeUiInteractionResponse,
) -> anyhow::Result<ReviewDecision> {
    let approved = response
        .approved
        .or(match response.selected_option.as_deref() {
            Some(option) if option.eq_ignore_ascii_case("approve") => Some(true),
            Some(option) if option.eq_ignore_ascii_case("allow") => Some(true),
            Some(option) if option.eq_ignore_ascii_case("approve_all") => Some(true),
            Some(option) if option.eq_ignore_ascii_case("yes") => Some(true),
            Some(option) if option.eq_ignore_ascii_case("deny") => Some(false),
            Some(option) if option.eq_ignore_ascii_case("decline") => Some(false),
            Some(option) if option.eq_ignore_ascii_case("no") => Some(false),
            Some(option) if option.eq_ignore_ascii_case("abort") => {
                return Ok(ReviewDecision::Abort)
            }
            _ => None,
        })
        .ok_or_else(|| anyhow!("Exec approvals require an explicit decision"))?;

    if !approved {
        return Ok(ReviewDecision::Denied);
    }

    match response.apply_to_future {
        Some(CodeUiApplyToFuture::AcceptAll) => Ok(ReviewDecision::ApprovedForAllCommands),
        Some(CodeUiApplyToFuture::DeclineAll) => Ok(ReviewDecision::Denied),
        Some(CodeUiApplyToFuture::No) | None => Ok(ReviewDecision::Approved),
    }
}

fn user_input_response_from_code_ui_request(
    questions: &[UserInputQuestion],
    response: CodeUiInteractionResponse,
) -> anyhow::Result<UserInputResponse> {
    if let Some((question_id, answers)) = response
        .answers
        .into_iter()
        .find(|(_, answers)| !answers.is_empty())
    {
        return Ok(UserInputResponse {
            answers: [(question_id, UserInputAnswer { answers })]
                .into_iter()
                .collect::<HashMap<_, _>>(),
        });
    }

    let question = questions
        .first()
        .ok_or_else(|| anyhow!("User input request contains no questions"))?;

    let mut values = Vec::new();
    if let Some(selected) = response.selected_option {
        if !selected.is_empty() {
            values.push(selected);
        }
    }
    if let Some(note) = response.note.as_deref() {
        let note = note.trim();
        if !note.is_empty() {
            values.push(format!("user_note: {note}"));
        }
    }

    if values.is_empty() {
        if let Some(approved) = response.approved {
            values.push(if approved { "yes".to_string() } else { "no".to_string() });
        }
    }

    if values.is_empty() {
        return Err(anyhow!("User input response must include answers"));
    }

    Ok(UserInputResponse {
        answers: [(question.id.clone(), UserInputAnswer { answers: values })]
            .into_iter()
            .collect::<HashMap<_, _>>(),
    })
}

/// Observer that streams text deltas into the live snapshot transcript so the
/// browser sees the assistant's reply build up as it arrives.
struct HeadlessTurnObserver {
    session: Arc<CodeUiSession>,
    assistant_entry_id: String,
}

impl super::super::agent::runtime::tool_loop::ToolLoopObserver for HeadlessTurnObserver {
    fn on_model_stream_event(&mut self, event: &CompletionStreamEvent) {
        if let CompletionStreamEvent::TextDelta { delta, .. } = event {
            if delta.is_empty() {
                return;
            }
            let session = self.session.clone();
            let entry_id = self.assistant_entry_id.clone();
            let delta = delta.clone();
            tokio::spawn(async move {
                session.append_assistant_delta(&entry_id, &delta).await;
            });
        }
    }

    fn on_model_usage_recorded(&mut self, _usage: &CompletionUsageSummary, _wall_clock_ms: u64) {
        // Phase 3 follow-up: persist usage rows + show them in the Settings tab.
    }
}
