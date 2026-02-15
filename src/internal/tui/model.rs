//! Model management for the application.

use crate::internal::ai::completion::CompletionModel;

/// Enum representing all supported AI models.
#[derive(Debug, Clone)]
pub enum ModelType {
    /// Gemini model
    Gemini(crate::internal::ai::providers::gemini::completion::CompletionModel),
    /// OpenAI model
    Openai(crate::internal::ai::providers::openai::completion::CompletionModel),
    /// Anthropic model
    Anthropic(crate::internal::ai::providers::anthropic::completion::CompletionModel),
    /// DeepSeek model
    Deepseek(crate::internal::ai::providers::deepseek::completion::CompletionModel),
    /// Zhipu model
    Zhipu(crate::internal::ai::providers::zhipu::completion::CompletionModel),
}

impl CompletionModel for ModelType {
    type Response = Box<dyn std::any::Any + Send + Sync>;

    async fn completion(
        &self,
        request: crate::internal::ai::completion::CompletionRequest,
    ) -> Result<
        crate::internal::ai::completion::CompletionResponse<Self::Response>,
        crate::internal::ai::completion::CompletionError,
    > {
        match self {
            ModelType::Gemini(model) => model.completion(request).await.map(|r| {
                crate::internal::ai::completion::CompletionResponse {
                    content: r.content,
                    raw_response: Box::new(r.raw_response) as Self::Response,
                }
            }),
            ModelType::Openai(model) => model.completion(request).await.map(|r| {
                crate::internal::ai::completion::CompletionResponse {
                    content: r.content,
                    raw_response: Box::new(r.raw_response) as Self::Response,
                }
            }),
            ModelType::Anthropic(model) => model.completion(request).await.map(|r| {
                crate::internal::ai::completion::CompletionResponse {
                    content: r.content,
                    raw_response: Box::new(r.raw_response) as Self::Response,
                }
            }),
            ModelType::Deepseek(model) => model.completion(request).await.map(|r| {
                crate::internal::ai::completion::CompletionResponse {
                    content: r.content,
                    raw_response: Box::new(r.raw_response) as Self::Response,
                }
            }),
            ModelType::Zhipu(model) => model.completion(request).await.map(|r| {
                crate::internal::ai::completion::CompletionResponse {
                    content: r.content,
                    raw_response: Box::new(r.raw_response) as Self::Response,
                }
            }),
        }
    }
}

impl ModelType {
    /// Get the name of the current model.
    pub fn name(&self) -> String {
        match self {
            ModelType::Gemini(model) => model.model_name().to_string(),
            ModelType::Openai(model) => model.model_name().to_string(),
            ModelType::Anthropic(model) => model.model_name().to_string(),
            ModelType::Deepseek(model) => model.model_name().to_string(),
            ModelType::Zhipu(model) => model.model_name().to_string(),
        }
    }

    /// Get the provider of the current model.
    pub fn provider(&self) -> String {
        match self {
            ModelType::Gemini(_) => "gemini".to_string(),
            ModelType::Openai(_) => "openai".to_string(),
            ModelType::Anthropic(_) => "anthropic".to_string(),
            ModelType::Deepseek(_) => "deepseek".to_string(),
            ModelType::Zhipu(_) => "zhipu".to_string(),
        }
    }
}

/// Supported models for each provider.
pub const SUPPORTED_MODELS: &[(&str, &str, &str)] = &[
    ("gemini", "gemini-2.5-flash", "Google Gemini 2.5 Flash"),
    ("gemini", "gemini-2.5-pro", "Google Gemini 2.5 Pro"),
    ("openai", "gpt-4o-mini", "OpenAI GPT-4o Mini"),
    ("openai", "gpt-4o", "OpenAI GPT-4o"),
    (
        "anthropic",
        "claude-3.5-sonnet",
        "Anthropic Claude 3.5 Sonnet",
    ),
    ("anthropic", "claude-3-opus", "Anthropic Claude 3 Opus"),
    ("deepseek", "deepseek-chat", "DeepSeek Chat"),
    ("zhipu", "glm-5", "Zhipu GLM-5"),
    ("zhipu", "glm-4", "Zhipu GLM-4"),
    ("zhipu", "glm-4-flash", "Zhipu GLM-4 Flash"),
];

/// Get all supported models.
pub fn get_supported_models() -> Vec<(&'static str, &'static str, &'static str)> {
    SUPPORTED_MODELS.to_vec()
}

/// Get supported models for a specific provider.
#[allow(dead_code)]
pub fn get_models_for_provider(provider: &str) -> Vec<(&'static str, &'static str)> {
    SUPPORTED_MODELS
        .iter()
        .filter(|(p, _, _)| *p == provider)
        .map(|(_, model, desc)| (*model, *desc))
        .collect()
}
