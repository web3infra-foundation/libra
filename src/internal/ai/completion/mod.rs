pub mod message;
pub mod request;

use std::future::Future;

pub use message::{AssistantContent, Message, MessageError, UserContent};
pub use request::{CompletionRequest, CompletionResponse};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CompletionError {
    #[error("HttpError: {0}")]
    HttpError(#[from] reqwest::Error),

    #[error("JsonError: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("RequestError: {0}")]
    RequestError(#[from] Box<dyn std::error::Error + Send + Sync + 'static>),

    #[error("ProviderError: {0}")]
    ProviderError(String),

    #[error("ResponseError: {0}")]
    ResponseError(String),

    #[error("Feature not implemented: {0}")]
    NotImplemented(String),
}

/// A trait representing a model capable of generating completions.
///
/// This is the core abstraction for different AI providers (e.g., Gemini, OpenAI).
/// Implementors must provide a way to handle a `CompletionRequest` and return a `CompletionResponse`.
pub trait CompletionModel: Clone + Send + Sync {
    type Response: Send + Sync;

    /// Generates a completion for the given request.
    fn completion(
        &self,
        request: CompletionRequest,
    ) -> impl Future<Output = Result<CompletionResponse<Self::Response>, CompletionError>> + Send;
}

/// A trait for simple text-based prompting.
///
/// This trait provides a high-level interface for sending a message and getting a text response,
/// abstracting away the details of the underlying request/response structures.
pub trait Prompt: Send + Sync {
    /// Sends a prompt to the model and returns the generated text.
    fn prompt(
        &self,
        prompt: impl Into<Message> + Send,
    ) -> impl Future<Output = Result<String, CompletionError>> + Send;
}

/// A trait for chat-based interactions.
///
/// This trait supports sending a message along with a history of previous messages,
/// allowing for stateful conversations (managed by the caller).
pub trait Chat: Send + Sync {
    /// Sends a prompt with conversation history to the model and returns the generated text.
    fn chat(
        &self,
        prompt: impl Into<Message> + Send,
        chat_history: Vec<Message>,
    ) -> impl Future<Output = Result<String, CompletionError>> + Send;
}
