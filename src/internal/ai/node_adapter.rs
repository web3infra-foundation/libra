//! DAG node adapters for AI agents.
//!
//! This module provides [`Action`] adapters that bridge the AI agent system with the
//! `dagrs` DAG execution framework. These adapters allow AI agents to participate as
//! nodes in a directed acyclic graph (DAG), enabling orchestration of multi-step
//! AI workflows.
//!
//! # Adapters
//!
//! - [`AgentAction`]: Wraps a single [`Agent`] for one-shot prompt-response execution
//!   within a DAG node.
//! - [`ToolLoopAction`]: Wraps the iterative tool-calling loop ([`run_tool_loop`]) within
//!   a DAG node, allowing the agent to invoke tools repeatedly until a final answer is produced.
//!
//! # Data Flow
//!
//! Both adapters follow the same input/output pattern:
//! 1. **Input**: Collect string outputs from all upstream nodes and concatenate them
//!    with `"\n\n"` as a separator to form a single prompt.
//! 2. **Execution**: Run the agent (or tool loop) with the assembled prompt.
//! 3. **Output**: Broadcast the agent's response to all downstream nodes.

use std::sync::Arc;

use async_trait::async_trait;
use dagrs::{Action, Content, EnvVar, InChannels, OutChannels, Output};

use crate::internal::ai::{
    agent::{Agent, ToolLoopConfig, run_tool_loop},
    completion::{CompletionModel, Prompt},
    tools::ToolRegistry,
};

/// An [`Action`] adapter that wraps an AI [`Agent`] for use in a DAG node.
///
/// This adapter bridges the gap between `dagrs::Action` and the AI `Agent`.
/// It automatically handles:
/// 1. Reading input from upstream nodes (as Prompt).
///    - If there are multiple upstream nodes, their outputs are concatenated
///      with newlines (`"\n\n"`) to form a single prompt.
/// 2. Invoking the Agent with the assembled prompt.
/// 3. Broadcasting the Agent's response to downstream nodes.
///
/// # Type Parameters
///
/// * `M` - The [`CompletionModel`] implementation used by the agent.
pub struct AgentAction<M: CompletionModel + 'static> {
    /// The wrapped AI Agent instance.
    agent: Agent<M>,
}

impl<M: CompletionModel> AgentAction<M> {
    /// Creates a new `AgentAction` adapter wrapping the given agent.
    ///
    /// # Arguments
    ///
    /// * `agent` - The configured [`Agent`] instance to wrap.
    pub fn new(agent: Agent<M>) -> Self {
        Self { agent }
    }
}

#[async_trait]
impl<M: CompletionModel> Action for AgentAction<M> {
    /// Executes the agent within the DAG node lifecycle.
    ///
    /// This method performs the following steps:
    /// 1. Collects string outputs from all upstream nodes via `in_channels`.
    /// 2. Concatenates them into a single prompt separated by `"\n\n"`.
    /// 3. Sends the prompt to the wrapped agent.
    /// 4. On success, broadcasts the response to all downstream nodes via `out_channels`.
    /// 5. On failure, logs the error and returns `Output::Err`.
    ///
    /// # Arguments
    ///
    /// * `in_channels` - Channels for receiving input from upstream DAG nodes.
    /// * `out_channels` - Channels for sending output to downstream DAG nodes.
    /// * `_env` - Shared environment variables (unused by this adapter).
    async fn run(
        &self,
        in_channels: &mut InChannels,
        out_channels: &mut OutChannels,
        _env: Arc<EnvVar>,
    ) -> Output {
        // Step 1: Collect inputs from all upstream nodes
        let ids = in_channels.get_sender_ids();
        let mut inputs = Vec::new();

        for id in ids {
            match in_channels.recv_from(&id).await {
                Ok(content) => {
                    // Attempt to extract a String from the upstream content
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

        // Concatenate all upstream outputs into a single prompt
        let input = inputs.join("\n\n");

        // Step 2: Run the agent with the assembled prompt
        match self.agent.prompt(input).await {
            Ok(resp) => {
                // Broadcast the successful response to all downstream nodes
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

/// An [`Action`] adapter that runs the iterative tool-calling agent loop inside a DAG node.
///
/// Unlike [`AgentAction`], which performs a single prompt-response cycle, this adapter
/// invokes [`run_tool_loop`], allowing the agent to call tools repeatedly in a loop
/// until it produces a final answer or reaches the maximum number of steps.
///
/// # Type Parameters
///
/// * `M` - The [`CompletionModel`] implementation used by the tool loop.
///
/// # Fields
///
/// * `model` - The language model used for generating completions.
/// * `registry` - The registry of tools available to the agent during the loop.
/// * `config` - Configuration for the tool loop (preamble, temperature, max steps, etc.).
pub struct ToolLoopAction<M: CompletionModel + 'static> {
    /// The language model used for generating completions.
    model: M,
    /// The registry of tools available to the agent during the tool loop.
    registry: ToolRegistry,
    /// Configuration controlling the behavior of the tool loop.
    config: ToolLoopConfig,
}

impl<M: CompletionModel> ToolLoopAction<M> {
    /// Creates a new `ToolLoopAction` adapter.
    ///
    /// # Arguments
    ///
    /// * `model` - The language model instance for generating completions.
    /// * `registry` - The tool registry containing all available tools.
    /// * `preamble` - An optional system preamble prepended to every prompt.
    /// * `temperature` - An optional temperature value controlling response randomness.
    /// * `max_steps` - An optional limit on the number of tool-calling iterations.
    pub fn new(
        model: M,
        registry: ToolRegistry,
        preamble: Option<String>,
        temperature: Option<f64>,
        max_steps: Option<usize>,
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
    /// Executes the tool loop within the DAG node lifecycle.
    ///
    /// Follows the same input collection pattern as [`AgentAction::run`]:
    /// collects and concatenates upstream outputs, then runs the tool loop
    /// instead of a simple agent prompt.
    ///
    /// # Arguments
    ///
    /// * `in_channels` - Channels for receiving input from upstream DAG nodes.
    /// * `out_channels` - Channels for sending output to downstream DAG nodes.
    /// * `_env` - Shared environment variables (unused by this adapter).
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
                    // Attempt to extract a String from the upstream content
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

        // Concatenate all upstream outputs into a single prompt
        let prompt = inputs.join("\n\n");

        // Run the iterative tool-calling loop with the assembled prompt
        match run_tool_loop(&self.model, prompt, &self.registry, self.config.clone()).await {
            Ok(resp) => {
                // Broadcast the successful response to all downstream nodes
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
