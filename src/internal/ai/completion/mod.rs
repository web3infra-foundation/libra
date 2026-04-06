pub mod message;
pub mod request;
pub mod retry;
pub mod throttle;

use std::future::Future;

pub use message::{
    AssistantContent, Function, Message, MessageError, OneOrMany, Text, ToolCall, ToolResult,
    UserContent,
};
pub use request::{CompletionRequest, CompletionResponse};
pub use retry::{
    CompletionRetryEvent, CompletionRetryObserver, CompletionRetryPolicy, RetryingCompletionModel,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
pub use throttle::ThrottledCompletionModel;

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

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CompletionUsageSummary {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

impl CompletionUsageSummary {
    pub fn merge(&mut self, other: &Self) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.cost_usd = match (self.cost_usd, other.cost_usd) {
            (Some(left), Some(right)) => Some(left + right),
            (Some(left), None) => Some(left),
            (None, Some(right)) => Some(right),
            (None, None) => None,
        };
    }

    pub fn is_zero(&self) -> bool {
        self.input_tokens == 0 && self.output_tokens == 0 && self.cost_usd.is_none()
    }
}

pub trait CompletionUsage: Send + Sync {
    fn usage_summary(&self) -> Option<CompletionUsageSummary>;
}

impl CompletionUsage for () {
    fn usage_summary(&self) -> Option<CompletionUsageSummary> {
        None
    }
}

impl CompletionUsage for serde_json::Value {
    fn usage_summary(&self) -> Option<CompletionUsageSummary> {
        None
    }
}

pub trait CompletionModel: Clone + Send + Sync {
    type Response: Send + Sync;

    fn completion(
        &self,
        request: CompletionRequest,
    ) -> impl Future<Output = Result<CompletionResponse<Self::Response>, CompletionError>> + Send;

    /// Optional method to set run ID for linking to workflow objects.
    /// Default implementation does nothing.
    fn set_run_id(&self, _run_id: String) {}
}

pub trait Prompt: Send + Sync {
    fn prompt(
        &self,
        prompt: impl Into<Message> + Send,
    ) -> impl Future<Output = Result<String, CompletionError>> + Send;
}

pub trait Chat: Send + Sync {
    fn chat(
        &self,
        prompt: impl Into<Message> + Send,
        chat_history: Vec<Message>,
    ) -> impl Future<Output = Result<String, CompletionError>> + Send;
}
