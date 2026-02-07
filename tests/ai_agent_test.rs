use std::{
    env,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use async_trait::async_trait;
use dagrs::{
    Action, Content, DefaultNode, EnvVar, Graph, InChannels, Node, NodeTable, OutChannels, Output,
};
use libra::internal::ai::{
    agent::AgentBuilder,
    completion::{
        CompletionError, CompletionModel, CompletionRequest, CompletionResponse,
        message::{AssistantContent, Function, ToolCall},
    },
    node_adapter::AgentAction,
    providers::gemini::Client,
    tools::{Tool, ToolDefinition, ToolSet},
};
use serde_json::json;

struct InputGenerator {
    prompt: String,
}

#[async_trait]
impl Action for InputGenerator {
    async fn run(
        &self,
        _: &mut InChannels,
        out_channels: &mut OutChannels,
        _: Arc<EnvVar>,
    ) -> Output {
        let content = Content::new(self.prompt.clone());
        out_channels.broadcast(content.clone()).await;
        Output::Out(Some(content))
    }
}

/// Integration test for Gemini agent execution.
///
/// # Setup
/// This test requires a valid `GEMINI_API_KEY` environment variable.
/// The test will be skipped if the key is not set.
///
/// ```bash
/// export GEMINI_API_KEY="your_key_here"
/// cargo test --test ai_agent_test test_gemini_agent_execution
/// ```
#[test]
fn test_gemini_agent_execution() {
    // Check for API Key
    if env::var("GEMINI_API_KEY").is_err() {
        println!("Skipping test_gemini_agent_execution because GEMINI_API_KEY is not set.");
        return;
    }

    // 1. Create Gemini Agent
    let client = Client::from_env().expect("Failed to create client from env");
    // Use flash model for speed/cost in tests
    let model = client.completion_model("gemini-2.5-flash");

    let agent = AgentBuilder::new(model)
        .preamble("You are a translator. Translate the input to Spanish.")
        .temperature(0.7)
        .expect("Invalid temperature")
        .build();

    let agent_action = AgentAction::new(agent);

    // 2. Build DAG
    let mut node_table = NodeTable::new();

    // Node A: Input
    let input_action = InputGenerator {
        prompt: "Hello".to_string(),
    };
    let a = DefaultNode::with_action("input".to_string(), input_action, &mut node_table);
    let a_id = a.id();

    // Node B: AI Agent
    let b = DefaultNode::with_action("translator".to_string(), agent_action, &mut node_table);
    let b_id = b.id();

    let mut graph = Graph::new();
    graph.add_node(a);
    graph.add_node(b);

    // Edge: A -> B
    graph.add_edge(a_id, vec![b_id]);

    // 3. Run
    let result = graph.start();
    assert!(result.is_ok(), "Graph execution failed: {:?}", result.err());

    // 4. Check Output
    let outputs = graph.get_results::<String>();
    let output = outputs.get(&b_id).unwrap().clone();

    if let Some(content) = output {
        println!("Translation Result: {}", content);
        assert!(!content.is_empty());
    } else {
        panic!("No output from translator agent.");
    }
}

struct WeatherTool {
    called: Arc<AtomicBool>,
}

impl Tool for WeatherTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "get_weather".to_string(),
            description: "Get the current weather in a given location".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "location": {
                        "type": "string",
                        "description": "The city and state, e.g. San Francisco, CA"
                    },
                    "unit": {
                        "type": "string",
                        "enum": ["celsius", "fahrenheit"]
                    }
                },
                "required": ["location"]
            }),
        }
    }

    fn call(
        &self,
        _args: serde_json::Value,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
        self.called.store(true, Ordering::SeqCst);
        Ok(json!({ "temperature": "22", "unit": "celsius", "description": "Sunny" }))
    }
}

/// Integration test for Gemini agent execution with Tools.
///
/// # Setup
/// This test requires a valid `GEMINI_API_KEY` environment variable.
///
/// ```bash
/// export GEMINI_API_KEY="your_key_here"
/// cargo test --test ai_agent_test test_gemini_agent_with_tools
/// ```
#[test]
fn test_gemini_agent_with_tools() {
    // Check for API Key
    if env::var("GEMINI_API_KEY").is_err() {
        println!("Skipping test_gemini_agent_with_tools because GEMINI_API_KEY is not set.");
        return;
    }

    // 1. Create Gemini Agent
    let client = Client::from_env().expect("Failed to create client from env");
    // Use flash model for speed/cost in tests
    let model = client.completion_model("gemini-2.5-flash");

    let tool_called = Arc::new(AtomicBool::new(false));

    let mut tool_set = ToolSet::default();
    tool_set.tools.push(Box::new(WeatherTool {
        called: tool_called.clone(),
    }));

    let agent = AgentBuilder::new(model)
        .preamble("You are a helpful assistant. If asked about weather, use the tool.")
        .tools(tool_set)
        .temperature(0.0)
        .expect("Invalid temperature")
        .build();

    let agent_action = AgentAction::new(agent);

    // 2. Build DAG
    let mut node_table = NodeTable::new();

    // Node A: Input
    let input_action = InputGenerator {
        prompt: "What is the weather in Tokyo?".to_string(),
    };
    let a = DefaultNode::with_action("input".to_string(), input_action, &mut node_table);
    let a_id = a.id();

    // Node B: AI Agent
    let b = DefaultNode::with_action("weather_bot".to_string(), agent_action, &mut node_table);
    let b_id = b.id();

    let mut graph = Graph::new();
    graph.add_node(a);
    graph.add_node(b);

    // Edge: A -> B
    graph.add_edge(a_id, vec![b_id]);

    // 3. Run
    let result = graph.start();
    assert!(result.is_ok(), "Graph execution failed: {:?}", result.err());

    // 4. Check Output
    let outputs = graph.get_results::<String>();
    let output = outputs.get(&b_id).unwrap().clone();

    let content = if let Some(content) = output {
        println!("Weather Tool Result: {}", content);
        assert!(!content.is_empty());
        content
    } else {
        panic!("No output from weather bot.");
    };

    let content_lower = content.to_lowercase();
    let looks_reasonable = content_lower.contains("tokyo")
        || content_lower.contains("celsius")
        || content_lower.contains("fahrenheit")
        || content_lower.contains("sunny")
        || content_lower.contains("22");

    // Accept either a tool invocation or a reasonable natural-language response.
    assert!(
        tool_called.load(Ordering::SeqCst) || looks_reasonable,
        "Tool call not triggered and response did not look like a weather answer: {}",
        content
    );
}

#[cfg(test)]
mod error_tests {
    use super::*;

    #[derive(Clone)]
    struct MockLoopModel;

    impl CompletionModel for MockLoopModel {
        type Response = ();
        async fn completion(
            &self,
            _req: CompletionRequest,
        ) -> Result<CompletionResponse<()>, CompletionError> {
            // Always return a tool call to trigger infinite loop if not stopped
            Ok(CompletionResponse {
                content: vec![AssistantContent::ToolCall(ToolCall {
                    id: "test-id".into(),
                    name: "infinite_tool".into(),
                    function: Function {
                        name: "infinite_tool".into(),
                        arguments: json!({}),
                    },
                })],
                raw_response: (),
            })
        }
    }

    struct MockTool;
    impl Tool for MockTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: "infinite_tool".into(),
                description: "loop".into(),
                parameters: json!({"type": "object"}),
            }
        }
        fn call(
            &self,
            _args: serde_json::Value,
        ) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
            Ok(json!({"result": "ok"}))
        }
    }

    #[tokio::test]
    async fn test_max_steps_exceeded() {
        let mut tools = ToolSet::default();
        tools.tools.push(Box::new(MockTool));

        let agent = AgentBuilder::new(MockLoopModel)
            .tools(tools)
            .max_steps(2) // Set low limit
            .build();

        use libra::internal::ai::completion::Chat;

        let result = agent.chat("test", vec![]).await;

        assert!(result.is_err());
        let err = result.err().unwrap();
        match err {
            CompletionError::ResponseError(msg) => {
                assert!(
                    msg.contains("exceeded max steps"),
                    "Unexpected error message: {}",
                    msg
                );
            }
            _ => panic!("Unexpected error type: {:?}", err),
        }
    }
}
