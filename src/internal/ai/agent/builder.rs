use crate::internal::ai::{agent::Agent, completion::CompletionModel, tools::ToolSet};

/// A builder for configuring and creating AI Agent instances.
pub struct AgentBuilder<M: CompletionModel> {
    model: M,
    preamble: Option<String>,
    temperature: Option<f64>,
    tools: ToolSet,
}

impl<M: CompletionModel> AgentBuilder<M> {
    /// Creates a new AgentBuilder with the specified CompletionModel.
    pub fn new(model: M) -> Self {
        Self {
            model,
            preamble: None,
            temperature: None,
            tools: ToolSet::default(),
        }
    }

    /// Sets the preamble (system prompt) for the agent.
    pub fn preamble(mut self, preamble: impl Into<String>) -> Self {
        self.preamble = Some(preamble.into());
        self
    }

    /// Sets the tools for the agent.
    pub fn tools(mut self, tools: ToolSet) -> Self {
        self.tools = tools;
        self
    }

    /// Sets the temperature for the agent's responses.
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
    pub fn build(self) -> Agent<M> {
        Agent {
            model: self.model,
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
                raw_response: (),
            })
        }
    }

    #[test]
    fn test_agent_builder_configuration() {
        let model = MockModel;
        let _agent = AgentBuilder::new(model)
            .preamble("system prompt")
            .temperature(0.5)
            .expect("Valid temperature")
            .build();
    }

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

    #[test]
    fn test_agent_builder_tools() {
        let model = MockModel;
        let tools = ToolSet::default();
        let _agent = AgentBuilder::new(model).tools(tools).build();
    }
}
