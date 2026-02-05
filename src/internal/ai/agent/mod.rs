use crate::internal::ai::{
    completion::{Chat, CompletionError, CompletionModel, CompletionRequest, Message, Prompt},
    tools::ToolSet,
};

pub mod builder;
pub use builder::AgentBuilder;

pub mod chat;
pub use chat::ChatAgent;

/// An AI Agent that manages interactions with a CompletionModel.
///
/// This is a **stateless** agent (also known as a Simple Agent). It handles configuration
/// (preamble, tools, temperature) and requests, but it does not maintain conversation history
/// between calls.
///
/// For a stateful agent that remembers context, use [`ChatAgent`].
///
/// The Agent is responsible for:
/// - Maintaining configuration (temperature, preamble/system prompt).
/// - Managing tools (optional).
/// - Constructing requests to the underlying model.
///
/// It implements the `Prompt` and `Chat` traits for easy interaction.
pub struct Agent<M: CompletionModel> {
    /// The underlying completion model (e.g., Gemini, OpenAI).
    model: M,
    /// System prompt or preamble to set the agent's behavior context.
    preamble: Option<String>,
    /// Sampling temperature (0.0 to 1.0). Higher values mean more creativity.
    temperature: Option<f64>,
    /// Set of tools available to the agent.
    /// Tools available to the agent (reserved for future tool-calling support).
    #[allow(dead_code)]
    tools: ToolSet,
}

impl<M: CompletionModel> Agent<M> {
    /// Creates a new Agent with the given model.
    ///
    /// # Arguments
    /// * `model` - The completion model instance.
    pub fn new(model: M) -> Self {
        Self {
            model,
            preamble: None,
            temperature: None,
            tools: ToolSet::default(),
        }
    }
}

impl<M: CompletionModel> Prompt for Agent<M> {
    async fn prompt(&self, prompt: impl Into<Message> + Send) -> Result<String, CompletionError> {
        let msg = prompt.into();
        let tools = self.tools.tools.iter().map(|t| t.definition()).collect();

        let request = CompletionRequest {
            preamble: self.preamble.clone(),
            chat_history: vec![msg],
            temperature: self.temperature,
            tools,
            ..Default::default()
        };

        let response = self.model.completion(request).await?;
        // Extract text content for backward compatibility
        // If there are multiple content items, join text ones.
        // For tools, this simple Prompt trait might not be enough, but we stick to text return for now.
        let text_response = response
            .content
            .iter()
            .filter_map(|c| match c {
                crate::internal::ai::completion::message::AssistantContent::Text(t) => {
                    Some(t.text.clone())
                }
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        if text_response.is_empty() && !response.content.is_empty() {
            // If no text but has other content (e.g. tool call), return a placeholder or JSON representation
            // For now, let's return a debug string if no text
            return Ok(format!("(Non-text response: {:?})", response.content));
        }

        Ok(text_response)
    }
}

impl<M: CompletionModel> Chat for Agent<M> {
    async fn chat(
        &self,
        prompt: impl Into<Message> + Send,
        mut chat_history: Vec<Message>,
    ) -> Result<String, CompletionError> {
        let msg = prompt.into();
        chat_history.push(msg);
        let tools = self.tools.tools.iter().map(|t| t.definition()).collect();

        let request = CompletionRequest {
            preamble: self.preamble.clone(),
            chat_history,
            temperature: self.temperature,
            tools,
            ..Default::default()
        };

        let response = self.model.completion(request).await?;

        let text_response = response
            .content
            .iter()
            .filter_map(|c| match c {
                crate::internal::ai::completion::message::AssistantContent::Text(t) => {
                    Some(t.text.clone())
                }
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        if text_response.is_empty() && !response.content.is_empty() {
            return Ok(format!("(Non-text response: {:?})", response.content));
        }

        Ok(text_response)
    }
}
