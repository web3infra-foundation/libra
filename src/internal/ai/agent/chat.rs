use super::Agent;
use crate::internal::ai::completion::{Chat, CompletionError, CompletionModel, Message};

/// A stateful agent that maintains conversation history.
///
/// `ChatAgent` wraps a standard `Agent` and adds memory capabilities by storing
/// the conversation history locally. It is designed for multi-turn conversations
/// where context needs to be preserved.
///
/// # Example
///
/// ```rust,no_run
/// # use libra::internal::ai::agent::{Agent, ChatAgent};
/// # use libra::internal::ai::completion::CompletionModel;
/// # async fn example<M: CompletionModel>(model: M) {
/// let agent = Agent::new(model);
/// let mut chat_agent = ChatAgent::new(agent);
///
/// let response = chat_agent.chat("Hello").await.unwrap();
/// let response2 = chat_agent.chat("My name is Jack").await.unwrap();
/// # }
/// ```
pub struct ChatAgent<M: CompletionModel> {
    /// The underlying stateless agent used for completion generation.
    agent: Agent<M>,
    /// The history of the conversation.
    history: Vec<Message>,
}

impl<M: CompletionModel> ChatAgent<M> {
    /// Creates a new ChatAgent from an existing Agent.
    ///
    /// # Arguments
    /// * `agent` - The base agent configuration to use.
    pub fn new(agent: Agent<M>) -> Self {
        Self {
            agent,
            history: Vec::new(),
        }
    }

    /// Sends a message to the agent and gets a response, updating the history.
    ///
    /// This method:
    /// 1. Adds the user's message to the history.
    /// 2. Calls the underlying agent to generate a response using the full history.
    /// 3. Adds the agent's response to the history.
    ///
    /// # Arguments
    /// * `prompt` - The user's input message.
    pub async fn chat(
        &mut self,
        prompt: impl Into<String> + Send,
    ) -> Result<String, CompletionError> {
        let user_msg = Message::user(prompt.into());

        // Delegate to the underlying agent's chat implementation.
        // The underlying agent handles the request construction using the provided history
        // and the new user message.
        let response = Chat::chat(&self.agent, user_msg.clone(), self.history.clone()).await?;

        // Update history with user message and assistant response
        self.history.push(user_msg);
        self.history.push(Message::assistant(response.clone()));

        Ok(response)
    }

    /// Returns a reference to the current conversation history.
    pub fn history(&self) -> &[Message] {
        &self.history
    }

    /// Clears the conversation history.
    pub fn clear_history(&mut self) {
        self.history.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::completion::{
        CompletionRequest, CompletionResponse, Message,
        message::{AssistantContent, OneOrMany, Text, UserContent},
    };

    #[derive(Clone)]
    struct MockModel;

    impl CompletionModel for MockModel {
        type Response = ();

        async fn completion(
            &self,
            request: CompletionRequest,
        ) -> Result<CompletionResponse<()>, CompletionError> {
            let last_msg = request.chat_history.last().unwrap();
            let response_text = match last_msg {
                Message::User { content } => match content {
                    OneOrMany::One(UserContent::Text(t)) => format!("Echo: {}", t.text),
                    _ => "Unknown".to_string(),
                },
                _ => "Unknown".to_string(),
            };

            Ok(CompletionResponse {
                content: vec![AssistantContent::Text(Text {
                    text: response_text,
                })],
                raw_response: (),
            })
        }
    }

    #[tokio::test]
    async fn test_chat_agent_maintains_history() {
        let agent = Agent::new(MockModel);
        let mut chat_agent = ChatAgent::new(agent);

        let resp1 = chat_agent.chat("Hello").await.unwrap();
        assert_eq!(resp1, "Echo: Hello");
        assert_eq!(chat_agent.history().len(), 2); // User + Assistant

        let resp2 = chat_agent.chat("World").await.unwrap();
        assert_eq!(resp2, "Echo: World");
        assert_eq!(chat_agent.history().len(), 4); // User + Assistant + User + Assistant
    }

    #[tokio::test]
    async fn test_clear_history() {
        let agent = Agent::new(MockModel);
        let mut chat_agent = ChatAgent::new(agent);

        chat_agent.chat("Hello").await.unwrap();
        assert!(!chat_agent.history().is_empty());

        chat_agent.clear_history();
        assert!(chat_agent.history().is_empty());
    }
}
