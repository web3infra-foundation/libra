//! Stateful conversation wrapper around [`super::Agent`].
//!
//! While [`super::Agent`] is intentionally stateless (it is reused by the multi-agent
//! plan executor where every step needs an isolated context), [`ChatAgent`] is the
//! type that interactive callers — the TUI, MCP, and `libra code` — own across many
//! turns. It records each user/assistant turn so subsequent calls implicitly include
//! the running conversation.

use super::Agent;
use crate::internal::ai::completion::{CompletionError, CompletionModel, Message};

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
    /// Functional scope:
    /// 1. Adds the user's message to the history.
    /// 2. Calls the underlying agent to generate a response using the full history.
    /// 3. Adds the agent's response to the history.
    ///
    /// Boundary conditions:
    /// - On a `CompletionError` the user message is *retained* in the history but no
    ///   assistant turn is appended. The next `chat()` call will therefore retry with
    ///   the same user message at the tail; callers that want to drop it must pop it
    ///   explicitly.
    /// - The whole history is cloned per call because [`Agent::run_with_history`]
    ///   takes ownership; the cost is acceptable for chat-sized turn counts.
    ///
    /// # Arguments
    /// * `prompt` - The user's input message.
    ///
    /// See: `tests::test_chat_agent_maintains_history`.
    pub async fn chat(
        &mut self,
        prompt: impl Into<String> + Send,
    ) -> Result<String, CompletionError> {
        let user_msg = Message::user(prompt.into());

        // Update history with user message first
        self.history.push(user_msg);

        // Run the agent with the current history.
        // We must clone the history because the agent takes ownership of the context for the request.
        let response = self.agent.run_with_history(self.history.clone()).await?;

        // Update history with assistant response
        self.history.push(Message::assistant(response.clone()));

        Ok(response)
    }

    /// Returns a reference to the current conversation history.
    ///
    /// Note: The history grows with each turn. For long-running conversations,
    /// consider monitoring the length and clearing it if it becomes too large
    /// to avoid token limit issues or excessive memory usage.
    pub fn history(&self) -> &[Message] {
        &self.history
    }

    /// Clears the conversation history.
    ///
    /// Use this to reset the conversation context. After this call the chat agent
    /// behaves identically to a freshly constructed one with the same underlying
    /// `Agent`.
    pub fn clear_history(&mut self) {
        self.history.clear();
    }

    /// Clone the inner agent for background execution.
    ///
    /// Functional scope: returns a clone of the wrapped [`Agent`]. Because the agent
    /// holds its model behind an `Arc`, the clone shares the same model handle and
    /// network client. Use this together with [`Self::update_history`] to drive the
    /// agent from a background task while still letting the foreground update the
    /// canonical history once the task finishes.
    pub fn clone_agent(&self) -> Agent<M> {
        self.agent.clone()
    }

    /// Update the history after a response is complete.
    ///
    /// Functional scope: appends the original user message and the produced assistant
    /// response to the canonical history.
    ///
    /// Boundary conditions: the caller must ensure ordering — typically this is called
    /// exactly once after a background task that used [`Self::clone_agent`] returns.
    /// Calling it twice for the same turn would duplicate the messages.
    pub fn update_history(&mut self, user_msg: String, assistant_response: String) {
        self.history.push(Message::user(user_msg));
        self.history.push(Message::assistant(assistant_response));
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
                Message::User {
                    content: OneOrMany::One(UserContent::Text(t)),
                } => format!("Echo: {}", t.text),
                _ => "Unknown".to_string(),
            };

            Ok(CompletionResponse {
                content: vec![AssistantContent::Text(Text {
                    text: response_text,
                })],
                reasoning_content: None,
                raw_response: (),
            })
        }
    }

    /// Scenario: two consecutive chat turns each grow the history by exactly two
    /// messages (user + assistant) — verifies the bookkeeping in `chat`.
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

    /// Scenario: `clear_history` empties the buffer, returning the agent to a virgin
    /// state.
    #[tokio::test]
    async fn test_clear_history() {
        let agent = Agent::new(MockModel);
        let mut chat_agent = ChatAgent::new(agent);

        chat_agent.chat("Hello").await.unwrap();
        assert!(!chat_agent.history().is_empty());

        chat_agent.clear_history();
        assert!(chat_agent.history().is_empty());
    }

    #[derive(Clone)]
    struct AlwaysErrorModel;
    impl CompletionModel for AlwaysErrorModel {
        type Response = ();
        async fn completion(
            &self,
            _request: CompletionRequest,
        ) -> Result<CompletionResponse<()>, CompletionError> {
            Err(CompletionError::ProviderError("simulated".to_string()))
        }
    }

    /// Documented error-path contract: when the underlying agent returns a
    /// `CompletionError`, the user message must remain in the history (no
    /// assistant turn appended). Pin this so a future refactor that pops the
    /// user message on error gets caught here.
    #[tokio::test]
    async fn chat_error_retains_user_message_but_appends_no_assistant_turn() {
        let agent = Agent::new(AlwaysErrorModel);
        let mut chat_agent = ChatAgent::new(agent);

        let result = chat_agent.chat("first attempt").await;
        assert!(result.is_err());
        assert_eq!(
            chat_agent.history().len(),
            1,
            "history must contain exactly the retained user message; got {:?}",
            chat_agent.history(),
        );
        match &chat_agent.history()[0] {
            Message::User {
                content: OneOrMany::One(UserContent::Text(t)),
            } => assert_eq!(t.text, "first attempt"),
            other => panic!("expected User text message, got {other:?}"),
        }
    }

    /// `clone_agent()` returns an independent agent handle (the inner model
    /// holds via Arc, so the underlying state is shared, but the cloned
    /// agent does not carry the chat history). Pin so callers can spawn
    /// background tasks without contaminating the foreground history.
    #[tokio::test]
    async fn clone_agent_returns_independent_handle_without_history() {
        let agent = Agent::new(MockModel);
        let mut chat_agent = ChatAgent::new(agent);

        chat_agent.chat("seed").await.unwrap();
        assert_eq!(chat_agent.history().len(), 2);

        let cloned = chat_agent.clone_agent();
        // The cloned agent has no notion of the foreground chat history —
        // it's a fresh Agent handle ready for background dispatch.
        // We can't directly inspect its private state but we can verify
        // it produces an independent response.
        let response = cloned
            .run_with_history(vec![Message::user("hello")])
            .await
            .unwrap();
        assert_eq!(response, "Echo: hello");

        // The foreground chat history must be unaffected by the clone +
        // background dispatch.
        assert_eq!(chat_agent.history().len(), 2);
    }

    /// `update_history(user, assistant)` appends both messages in order.
    /// This is the contract a background task uses to commit its result
    /// back into the foreground chat after completing via `clone_agent()`.
    #[tokio::test]
    async fn update_history_appends_user_and_assistant_in_order() {
        let agent = Agent::new(MockModel);
        let mut chat_agent = ChatAgent::new(agent);

        chat_agent.update_history(
            "background prompt".to_string(),
            "background response".to_string(),
        );

        assert_eq!(chat_agent.history().len(), 2);
        match &chat_agent.history()[0] {
            Message::User {
                content: OneOrMany::One(UserContent::Text(t)),
            } => assert_eq!(t.text, "background prompt"),
            other => panic!("expected User, got {other:?}"),
        }
        match &chat_agent.history()[1] {
            Message::Assistant { content, .. } => match content {
                OneOrMany::One(AssistantContent::Text(t)) => {
                    assert_eq!(t.text, "background response");
                }
                other => panic!("expected One(Text), got {other:?}"),
            },
            other => panic!("expected Assistant, got {other:?}"),
        }
    }

    /// `history()` returns a borrowed slice. The caller can read but not
    /// mutate the underlying buffer; pin so a future refactor doesn't
    /// accidentally surface `&mut [Message]`.
    #[tokio::test]
    async fn history_returns_borrowed_slice_for_read_only_access() {
        let agent = Agent::new(MockModel);
        let mut chat_agent = ChatAgent::new(agent);

        chat_agent.chat("hi").await.unwrap();
        // The returned slice must support .len() and indexing.
        let slice: &[Message] = chat_agent.history();
        assert_eq!(slice.len(), 2);
    }
}
