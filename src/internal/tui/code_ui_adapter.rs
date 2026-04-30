//! TUI-mode `CodeUiCommandAdapter` bridge.
//!
//! This module implements the [`CodeUiCommandAdapter`] trait for the default Libra
//! TUI. HTTP write requests from automation clients are translated into
//! [`TuiControlCommand`] messages and sent over an unbounded channel to the App
//! main loop. The App owns turn id assignment, snapshot mutation, and interaction
//! lifecycle; the adapter never mutates snapshot state directly.
//!
//! Boundary with [`CodexCodeUiAdapter`]: `CodexCodeUiAdapter` (in
//! `src/internal/ai/codex/mod.rs`) is used **only** for `--web-only --provider codex`,
//! where it speaks directly to the Codex app-server WebSocket. In TUI mode,
//! `--provider codex` uses the default Libra TUI (`run_tui_with_managed_code_runtime`)
//! and routes automation writes through this `TuiCodeUiAdapter`, so Codex
//! app-server acts purely as a managed execution backend.

use std::{sync::Arc, time::Duration};

use anyhow::{Context, anyhow};
use async_trait::async_trait;
use tokio::sync::{mpsc::UnboundedSender, oneshot};

use super::control::{TuiControlCommand, TuiControlError};
use crate::internal::ai::web::code_ui::{
    CodeUiCapabilities, CodeUiCommandAdapter, CodeUiInteractionResponse, CodeUiInteractionStatus,
    CodeUiReadModel, CodeUiSession,
};

const TUI_CONTROL_ACK_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct TuiCodeUiAdapter {
    session: Arc<CodeUiSession>,
    capabilities: CodeUiCapabilities,
    control_tx: UnboundedSender<TuiControlCommand>,
}

impl TuiCodeUiAdapter {
    pub fn new(
        session: Arc<CodeUiSession>,
        capabilities: CodeUiCapabilities,
        control_tx: UnboundedSender<TuiControlCommand>,
    ) -> Arc<Self> {
        Arc::new(Self {
            session,
            capabilities,
            control_tx,
        })
    }

    async fn wait_for_ack(
        ack_rx: oneshot::Receiver<Result<(), TuiControlError>>,
    ) -> anyhow::Result<()> {
        tokio::time::timeout(TUI_CONTROL_ACK_TIMEOUT, ack_rx)
            .await
            .context("timed out waiting for TUI control acknowledgement")?
            .map_err(|_| anyhow!("TUI control channel closed before acknowledgement"))?
            .map_err(anyhow::Error::new)
    }
}

#[async_trait]
impl CodeUiReadModel for TuiCodeUiAdapter {
    fn session(&self) -> Arc<CodeUiSession> {
        self.session.clone()
    }
}

#[async_trait]
impl CodeUiCommandAdapter for TuiCodeUiAdapter {
    fn capabilities(&self) -> CodeUiCapabilities {
        self.capabilities.clone()
    }

    async fn submit_message(&self, text: String) -> anyhow::Result<()> {
        let (ack, ack_rx) = oneshot::channel();
        self.control_tx
            .send(TuiControlCommand::SubmitMessage { text, ack })
            .map_err(|_| anyhow!("TUI control channel is closed"))?;
        Self::wait_for_ack(ack_rx).await
    }

    async fn respond_interaction(
        &self,
        interaction_id: &str,
        response: CodeUiInteractionResponse,
    ) -> anyhow::Result<()> {
        let snapshot = self.session.snapshot().await;
        let is_pending = snapshot.interactions.iter().any(|interaction| {
            interaction.id == interaction_id
                && interaction.status == CodeUiInteractionStatus::Pending
        });
        if !is_pending {
            return Err(anyhow::Error::new(TuiControlError::InteractionNotActive));
        }

        let (ack, ack_rx) = oneshot::channel();
        self.control_tx
            .send(TuiControlCommand::RespondInteraction {
                interaction_id: interaction_id.to_string(),
                response,
                ack,
            })
            .map_err(|_| anyhow!("TUI control channel is closed"))?;
        Self::wait_for_ack(ack_rx).await
    }

    async fn cancel_turn(&self) -> anyhow::Result<()> {
        let (ack, ack_rx) = oneshot::channel();
        self.control_tx
            .send(TuiControlCommand::CancelCurrentTurn { ack })
            .map_err(|_| anyhow!("TUI control channel is closed"))?;
        Self::wait_for_ack(ack_rx).await
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use tokio::sync::mpsc;

    use super::*;
    use crate::internal::ai::web::code_ui::{
        CodeUiInteractionKind, CodeUiInteractionRequest, CodeUiProviderInfo, CodeUiSessionStatus,
        initial_snapshot,
    };

    fn test_adapter() -> (
        Arc<TuiCodeUiAdapter>,
        mpsc::UnboundedReceiver<TuiControlCommand>,
    ) {
        let session = CodeUiSession::new(initial_snapshot(
            "/tmp/libra",
            CodeUiProviderInfo {
                provider: "test".to_string(),
                model: Some("test-model".to_string()),
                mode: Some("tui".to_string()),
                managed: false,
            },
            CodeUiCapabilities {
                message_input: true,
                interactive_approvals: true,
                ..CodeUiCapabilities::default()
            },
        ));
        let (tx, rx) = mpsc::unbounded_channel();
        (
            TuiCodeUiAdapter::new(session, CodeUiCapabilities::default(), tx),
            rx,
        )
    }

    #[tokio::test]
    async fn submit_message_sends_control_command_and_waits_for_ack() {
        let (adapter, mut rx) = test_adapter();
        let submit = tokio::spawn(async move { adapter.submit_message("hello".to_string()).await });

        let command = rx.recv().await.expect("control command should be sent");
        match command {
            TuiControlCommand::SubmitMessage { text, ack } => {
                assert_eq!(text, "hello");
                ack.send(Ok(())).expect("ack receiver should be live");
            }
            _ => panic!("unexpected command"),
        }

        submit.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn respond_interaction_rejects_non_pending_interaction_before_app_channel() {
        let (adapter, mut rx) = test_adapter();

        let error = adapter
            .respond_interaction("missing", CodeUiInteractionResponse::default())
            .await
            .unwrap_err();

        assert!(error.downcast_ref::<TuiControlError>().is_some());
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn respond_interaction_sends_pending_interaction_to_app() {
        let (adapter, mut rx) = test_adapter();
        adapter
            .session
            .upsert_interaction(CodeUiInteractionRequest {
                id: "approval-1".to_string(),
                kind: CodeUiInteractionKind::Approval,
                title: Some("Approval".to_string()),
                description: None,
                prompt: None,
                options: Vec::new(),
                status: CodeUiInteractionStatus::Pending,
                metadata: serde_json::json!({}),
                requested_at: Utc::now(),
                resolved_at: None,
            })
            .await;
        adapter
            .session
            .set_status(CodeUiSessionStatus::AwaitingInteraction)
            .await;

        let adapter_for_task = adapter.clone();
        let respond = tokio::spawn(async move {
            adapter_for_task
                .respond_interaction("approval-1", CodeUiInteractionResponse::default())
                .await
        });

        let command = rx.recv().await.expect("control command should be sent");
        match command {
            TuiControlCommand::RespondInteraction {
                interaction_id,
                ack,
                ..
            } => {
                assert_eq!(interaction_id, "approval-1");
                ack.send(Ok(())).expect("ack receiver should be live");
            }
            _ => panic!("unexpected command"),
        }

        respond.await.unwrap().unwrap();
    }
}
