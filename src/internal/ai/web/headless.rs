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
//! # Out of scope for v0 (tracked as future work)
//!
//! - IntentSpec / Plan workflow integration. The TUI's Phase 0/1 review loop
//!   is deeply coupled to the ratatui [`crate::internal::tui::app::App`]; this
//!   runtime treats every browser submit as a single direct turn instead.
//! - `request_user_input` / `approval` interactions surfaced as
//!   [`crate::internal::ai::web::code_ui::CodeUiInteractionRequest`]s. The
//!   current observer ignores those tools.
//! - Multi-turn conversation history persistence via `SessionStore`.
//!
//! These follow-ups are explicitly called out in
//! `docs/improvement/web.md` and will land in subsequent phases.

use std::sync::Arc;

use anyhow::anyhow;
use async_trait::async_trait;
use chrono::Utc;
use tokio::{sync::Mutex, task::JoinHandle};

use super::code_ui::{
    CodeUiCapabilities, CodeUiCommandAdapter, CodeUiInteractionResponse, CodeUiReadModel,
    CodeUiSession, CodeUiSessionStatus, CodeUiTranscriptEntry, CodeUiTranscriptEntryKind,
};
use crate::internal::ai::{
    agent::runtime::run_tool_loop_with_history_and_observer,
    completion::{
        CompletionError, CompletionModel, CompletionStreamEvent, CompletionUsage,
        CompletionUsageSummary, Message,
    },
    tools::ToolRegistry,
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
        interactive_approvals: false,
        structured_questions: false,
        provider_session_resume: false,
    }
}

/// Adapter that runs an agent tool loop in response to browser-driven messages.
///
/// Generic over a [`CompletionModel`] so each provider (Ollama, OpenAI, Gemini,
/// …) can plug in its own client. The model is held inside an `Arc<Mutex<…>>`
/// so the spawned turn task can take exclusive access while the next submit
/// waits in the queue.
pub struct HeadlessCodeRuntime<M: CompletionModel + 'static> {
    session: Arc<CodeUiSession>,
    capabilities: CodeUiCapabilities,
    /// Conversation history accumulated across turns.
    history: Arc<Mutex<Vec<Message>>>,
    model: Arc<M>,
    registry: Arc<ToolRegistry>,
    config_factory:
        Arc<dyn Fn() -> super::super::agent::runtime::tool_loop::ToolLoopConfig + Send + Sync>,
    /// Currently-running turn task (if any). `cancel_turn` aborts whatever is
    /// pinned here.
    in_flight: Arc<Mutex<Option<JoinHandle<()>>>>,
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
        config_factory: Arc<
            dyn Fn() -> super::super::agent::runtime::tool_loop::ToolLoopConfig + Send + Sync,
        >,
    ) -> Arc<Self> {
        Arc::new(Self {
            session,
            capabilities,
            history: Arc::new(Mutex::new(Vec::new())),
            model: Arc::new(model),
            registry,
            config_factory,
            in_flight: Arc::new(Mutex::new(None)),
        })
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

        // Reject overlapping submits — the in-flight turn must complete or be
        // cancelled before the next user prompt is accepted, mirroring the
        // TUI's serialized turn semantics.
        {
            let in_flight = self.in_flight.lock().await;
            if in_flight.as_ref().is_some_and(|task| !task.is_finished()) {
                return Err(anyhow!(
                    "A turn is already running; cancel it or wait for the assistant to finish before sending another message"
                ));
            }
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
        let in_flight = self.in_flight.clone();
        let user_text = text;

        let task = tokio::spawn(async move {
            let mut observer = HeadlessTurnObserver {
                session: session.clone(),
                assistant_entry_id: assistant_entry_id.clone(),
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
                        &assistant_entry_id,
                        &turn.final_text,
                        false,
                    )
                    .await;
                    session.set_status(CodeUiSessionStatus::Idle).await;
                }
                Err(error) => {
                    let message = format_completion_error(&error);
                    finalize_assistant_entry(&session, &assistant_entry_id, &message, true).await;
                    session.set_status(CodeUiSessionStatus::Error).await;
                }
            }

            // Drop the join handle once the turn finishes so the next submit
            // can claim the in_flight slot.
            let mut slot = in_flight.lock().await;
            *slot = None;
        });

        let mut slot = self.in_flight.lock().await;
        *slot = Some(task);
        Ok(())
    }

    async fn respond_interaction(
        &self,
        _interaction_id: &str,
        _response: CodeUiInteractionResponse,
    ) -> anyhow::Result<()> {
        // Phase 3 v0 surface — interactions are not yet routed to the headless
        // runtime; the browser only sees plain text turns.
        Err(anyhow!(
            "Interactive approvals are not yet supported by the headless web runtime; configure --provider codex or --browser-control loopback in TUI mode"
        ))
    }

    async fn cancel_turn(&self) -> anyhow::Result<()> {
        let mut slot = self.in_flight.lock().await;
        if let Some(task) = slot.take()
            && !task.is_finished()
        {
            task.abort();
        }
        self.session.set_status(CodeUiSessionStatus::Idle).await;
        Ok(())
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        let mut slot = self.in_flight.lock().await;
        if let Some(task) = slot.take() {
            task.abort();
        }
        Ok(())
    }
}

// `CodeUiProviderAdapter` is automatically implemented for any `T` that
// satisfies `CodeUiReadModel + CodeUiCommandAdapter` via the blanket impl in
// `code_ui.rs`. `Arc<HeadlessCodeRuntime<M>>` picks that up directly because
// `HeadlessCodeRuntime` itself implements both halves.

/// Replace the streaming assistant entry with the finalized text, mark the
/// streaming flag false, and stamp the right status.
async fn finalize_assistant_entry(
    session: &Arc<CodeUiSession>,
    entry_id: &str,
    text: &str,
    is_error: bool,
) {
    let entry_id = entry_id.to_string();
    let text = text.to_string();
    let status = if is_error { "error" } else { "completed" };
    session
        .mutate("session_updated", |snapshot| {
            if let Some(entry) = snapshot.transcript.iter_mut().find(|e| e.id == entry_id) {
                entry.content = Some(text.clone());
                entry.status = Some(status.to_string());
                entry.streaming = false;
                entry.updated_at = Utc::now();
            }
        })
        .await;
}

fn format_completion_error(error: &CompletionError) -> String {
    format!("Agent turn failed: {error}")
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
