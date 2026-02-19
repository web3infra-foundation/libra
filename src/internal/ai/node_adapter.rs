use std::sync::Arc;

use async_trait::async_trait;
use dagrs::{Action, Content, EnvVar, InChannels, OutChannels, Output};

use crate::internal::ai::{
    agent::{Agent, ToolLoopConfig, run_tool_loop},
    completion::{CompletionModel, Prompt},
    tools::ToolRegistry,
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
                out_channels.broadcast(content.clone()).await;
                Output::Out(Some(content))
            }
            Err(e) => {
                tracing::error!("Agent Execution Error: {}", e);
                Output::Err(e.to_string())
            }
        }
    }
}

/// An Action adapter that runs the iterative tool-calling agent loop inside a DAG node.
pub struct ToolLoopAction<M: CompletionModel + 'static> {
    model: M,
    registry: ToolRegistry,
    config: ToolLoopConfig,
}

impl<M: CompletionModel> ToolLoopAction<M> {
    pub fn new(
        model: M,
        registry: ToolRegistry,
        preamble: Option<String>,
        temperature: Option<f64>,
        max_steps: usize,
    ) -> Self {
        Self {
            model,
            registry,
            config: ToolLoopConfig {
                preamble,
                temperature,
                max_steps,
                hook_runner: None,
                allowed_tools: None,
            },
        }
    }
}

#[async_trait]
impl<M: CompletionModel> Action for ToolLoopAction<M> {
    async fn run(
        &self,
        in_channels: &mut InChannels,
        out_channels: &mut OutChannels,
        _env: Arc<EnvVar>,
    ) -> Output {
        // Collect upstream string outputs into one prompt, consistent with AgentAction.
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

        let prompt = inputs.join("\n\n");

        match run_tool_loop(&self.model, prompt, &self.registry, self.config.clone()).await {
            Ok(resp) => {
                let content = Content::new(resp);
                out_channels.broadcast(content.clone()).await;
                Output::Out(Some(content))
            }
            Err(e) => {
                tracing::error!("Agent Tool Loop Error: {}", e);
                Output::Err(e.to_string())
            }
        }
    }
}
