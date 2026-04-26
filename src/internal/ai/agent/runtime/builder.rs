//! Fluent builder for [`super::Agent`].
//!
//! Construction goes through [`AgentBuilder`] so that knobs that need validation
//! (currently only the sampling temperature) reject bad input before the agent is
//! observable. The builder is intentionally consuming (`mut self` returns) to mirror
//! the rest of the codebase's builder style.

use std::sync::Arc;

use super::Agent;
use crate::internal::ai::{
    completion::CompletionModel,
    tools::{Tool, ToolRegistry, ToolSet},
};

/// A builder for configuring and creating AI Agent instances.
///
/// Mirrors the field set of [`Agent`] but stores the model by value so it can be
/// `Arc`-wrapped exactly once at `build` time. The builder owns its inputs and is
/// not `Clone` because the model itself need not be `Clone`.
pub struct AgentBuilder<M: CompletionModel> {
    model: M,
    preamble: Option<String>,
    temperature: Option<f64>,
    tools: ToolSet,
}

impl<M: CompletionModel> AgentBuilder<M> {
    /// Creates a new AgentBuilder with the specified CompletionModel.
    ///
    /// All optional knobs default to `None`/empty so the resulting agent works without
    /// any further configuration calls.
    pub fn new(model: M) -> Self {
        Self {
            model,
            preamble: None,
            temperature: None,
            tools: ToolSet::default(),
        }
    }

    /// Sets the preamble (system prompt) for the agent.
    ///
    /// Replaces any previously configured preamble. The string is later forwarded to
    /// every `CompletionRequest` as the system message.
    pub fn preamble(mut self, preamble: impl Into<String>) -> Self {
        self.preamble = Some(preamble.into());
        self
    }

    /// Sets the tools for the agent.
    ///
    /// Replaces the existing tool set wholesale. To add a single tool, use
    /// [`Self::tool`] instead.
    pub fn tools(mut self, tools: ToolSet) -> Self {
        self.tools = tools;
        self
    }

    /// Add a single tool to the agent.
    ///
    /// Functional scope: pushes the tool onto the existing set. Order is preserved,
    /// which matters when the model needs to disambiguate ties between similarly
    /// described tools.
    pub fn tool(mut self, tool: impl Tool + 'static) -> Self {
        self.tools.tools.push(std::sync::Arc::new(tool));
        self
    }

    /// Add tools from a ToolRegistry.
    ///
    /// Currently a no-op kept around for source compatibility while the migration to
    /// `ToolSet` is in flight. New code should call [`Self::tools`] or [`Self::tool`].
    #[deprecated(
        note = "Not yet implemented: ToolRegistry is not yet converted into ToolSet. Use tools(...) or tool(...) instead."
    )]
    pub fn with_registry(self, _registry: &ToolRegistry) -> Self {
        self
    }

    /// Sets the temperature for the agent's responses (0.0 to 2.0).
    ///
    /// Functional scope: stores the validated value and forwards it on every request.
    ///
    /// Boundary conditions:
    /// - Returns `Err(String)` describing the violation when the value is outside
    ///   `[0.0, 2.0]`. The closed range matches what every supported provider accepts;
    ///   higher values are rejected client-side rather than risking a 400 response.
    ///
    /// See: `tests::test_agent_builder_temperature_validation`.
    pub fn temperature(mut self, temperature: f64) -> Result<Self, String> {
        if !(0.0..=2.0).contains(&temperature) {
            return Err(format!(
                "Temperature must be between 0.0 and 2.0, got {}",
                temperature
            ));
        }
        self.temperature = Some(temperature);
        Ok(self)
    }

    /// Builds and returns the configured Agent instance.
    ///
    /// Wraps the model in an `Arc` so the resulting [`Agent`] is cheap to clone for
    /// concurrent use (e.g. spawning multiple chat tasks against the same model).
    pub fn build(self) -> Agent<M> {
        Agent {
            model: Arc::new(self.model),
            preamble: self.preamble,
            temperature: self.temperature,
            tools: self.tools,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AgentBuilder;
    use crate::internal::ai::{
        completion::{CompletionError, CompletionModel, CompletionRequest, CompletionResponse},
        tools::ToolSet,
    };

    #[derive(Clone, Debug)]
    struct MockModel;

    impl CompletionModel for MockModel {
        type Response = ();

        async fn completion(
            &self,
            _request: CompletionRequest,
        ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
            use crate::internal::ai::completion::message::{AssistantContent, Text};
            Ok(CompletionResponse {
                content: vec![AssistantContent::Text(Text {
                    text: "mock response".to_string(),
                })],
                reasoning_content: None,
                raw_response: (),
            })
        }
    }

    /// Scenario: standard happy-path build with preamble and an in-range temperature.
    #[test]
    fn test_agent_builder_configuration() {
        let model = MockModel;
        let _agent = AgentBuilder::new(model)
            .preamble("system prompt")
            .temperature(0.5)
            .expect("Valid temperature")
            .build();
    }

    /// Scenario: temperature must accept boundary values (`0.0`, `2.0`) and reject
    /// values just outside the closed range.
    #[test]
    fn test_agent_builder_temperature_validation() {
        // Test valid temperatures
        let builder = AgentBuilder::new(MockModel);
        assert!(builder.new_with_temp(0.5).is_ok());

        let builder = AgentBuilder::new(MockModel);
        assert!(builder.new_with_temp(0.0).is_ok());

        let builder = AgentBuilder::new(MockModel);
        assert!(builder.new_with_temp(2.0).is_ok());

        // Test invalid temperatures
        let builder_low = AgentBuilder::new(MockModel);
        assert!(builder_low.temperature(-0.1).is_err());

        let builder_high = AgentBuilder::new(MockModel);
        assert!(builder_high.temperature(2.1).is_err());
    }

    // Helper to reuse builder for test simplicity, though real builder consumes self
    impl AgentBuilder<MockModel> {
        fn new_with_temp(self, temp: f64) -> Result<Self, String> {
            self.temperature(temp)
        }
    }

    /// Scenario: assigning an empty `ToolSet` builds successfully (the agent simply
    /// runs without tools).
    #[test]
    fn test_agent_builder_tools() {
        let model = MockModel;
        let tools = ToolSet::default();
        let _agent = AgentBuilder::new(model).tools(tools).build();
    }
}
