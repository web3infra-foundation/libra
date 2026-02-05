use std::sync::Arc;

use async_trait::async_trait;
use dagrs::{Action, Content, EnvVar, InChannels, OutChannels, Output};

use crate::internal::ai::{
    agent::Agent,
    completion::{CompletionModel, Prompt},
};

/// An Action adapter that wraps an AI Agent for use in a DAG node.
///
/// This adapter bridges the gap between `dagrs::Action` and the AI `Agent`.
/// It automatically handles:
/// 1. Reading input from upstream nodes (as Prompt).
///    - If there are multiple upstream nodes, their outputs are concatenated
///      with newlines ("\n\n") to form a single prompt.
/// 2. Invoking the Agent.
/// 3. Broadcasting the Agent's response to downstream nodes.
///
/// # Type Parameters
/// * `M` - The CompletionModel implementation used by the agent.
pub struct AgentAction<M: CompletionModel + 'static> {
    /// The wrapped AI Agent instance.
    agent: Agent<M>,
}

impl<M: CompletionModel> AgentAction<M> {
    /// Creates a new AgentAction adapter.
    ///
    /// # Arguments
    /// * `agent` - The configured Agent instance to wrap.
    pub fn new(agent: Agent<M>) -> Self {
        Self { agent }
    }
}

#[async_trait]
impl<M: CompletionModel> Action for AgentAction<M> {
    async fn run(
        &self,
        in_channels: &mut InChannels,
        out_channels: &mut OutChannels,
        _env: Arc<EnvVar>,
    ) -> Output {
        // 1. Get Input
        let ids = in_channels.get_sender_ids();
        let mut inputs = Vec::new();

        for id in ids {
            match in_channels.recv_from(&id).await {
                Ok(content) => {
                    if let Some(text) = content.get::<String>() {
                        inputs.push(text.clone());
                    } else {
                        tracing::warn!(
                            "Received content from upstream {:?} is not a String. Defaulting to empty.",
                            id
                        );
                    }
                }
                Err(e) => {
                    let error_msg =
                        format!("Failed to receive input from upstream {:?}: {:?}", id, e);
                    tracing::error!("{}", error_msg);
                    return Output::Err(error_msg);
                }
            }
        }

        let input = inputs.join("\n\n");

        // 2. Run Agent
        match self.agent.prompt(input).await {
            Ok(resp) => {
                let content = Content::new(resp);
                out_channels.broadcast(content).await;
                Output::Out(None)
            }
            Err(e) => {
                tracing::error!("Agent Execution Error: {}", e);
                Output::Err(e.to_string())
            }
        }
    }
}
