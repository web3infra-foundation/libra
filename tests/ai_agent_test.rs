//! L3 integration tests for the AI agent DAG execution and tool-use pipeline.
//!
//! Exercises end-to-end `dagrs` graph execution where one node feeds a prompt into a
//! DeepSeek-backed `AgentAction`, optionally with registered tools. These tests live
//! at the highest test layer because they reach a live LLM endpoint.
//!
//! **Layer:** L3 — gated on `DEEPSEEK_API_KEY` being set in the environment.
//! Sourcing `.env.test` before `cargo test --all` is the documented activation
//! path; the GitHub Actions L3 nightly job injects the same secret. With the
//! key set the tests are expected to pass — failure here represents a real
//! provider-client or runtime regression.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use async_trait::async_trait;
use dagrs::{
    Action, Content, DefaultNode, EnvVar, Graph, InChannels, Node, NodeTable, OutChannels, Output,
};
use libra::internal::ai::{
    agent::AgentBuilder,
    client::CompletionClient,
    node_adapter::AgentAction,
    providers::deepseek::Client,
    tools::{Tool, ToolDefinition, ToolSet},
};
use serde_json::json;

/// L3 model used by every DeepSeek-backed integration test in this crate.
///
/// `deepseek-v4-flash` is the cheap / fast tier that matches what
/// `.env.test`-backed local runs and the GitHub Actions L3 nightly job pay
/// for. Tests must not silently switch to a costlier tier (e.g.
/// `deepseek-v4-pro`) without a deliberate per-test override.
const DEEPSEEK_TEST_MODEL: &str = "deepseek-v4-flash";

/// Return `true` when the DeepSeek live gate is satisfied — i.e. a non-empty
/// `DEEPSEEK_API_KEY` is in the process environment. Sourcing `.env.test`
/// before `cargo test --all` is the documented way to enable L3 AI tests.
fn deepseek_live_enabled() -> bool {
    std::env::var("DEEPSEEK_API_KEY").is_ok_and(|value| !value.trim().is_empty())
}

/// Trivial `Action` that broadcasts a fixed prompt as the source node of the DAG.
///
/// Used as the upstream node feeding the LLM-backed agent under test. Holding the
/// prompt as owned `String` keeps it self-contained inside the spawned task.
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

/// Integration test for DeepSeek agent execution.
///
/// Scenario: builds a two-node DAG where an `InputGenerator` feeds a prompt into a
/// DeepSeek-backed `AgentAction`, runs the graph against the live DeepSeek API, and
/// asserts that a non-empty translation comes back. This exercises the boundary
/// between the agent runtime and the provider client.
///
/// Boundary: skipped when `DEEPSEEK_API_KEY` is unset. With the key set the
/// test reaches `https://api.deepseek.com/v1/chat/completions` and is
/// expected to pass.
///
/// # Setup
///
/// ```bash
/// source .env.test && cargo test --test ai_agent_test test_deepseek_agent_execution
/// ```
#[tokio::test]
async fn test_deepseek_agent_execution() {
    if !deepseek_live_enabled() {
        eprintln!("skipped (set DEEPSEEK_API_KEY to run the DeepSeek L3 gate)");
        return;
    }

    // 1. Create DeepSeek Agent
    let client = Client::from_env().expect("DEEPSEEK_API_KEY missing despite gate");
    let model = client.completion_model(DEEPSEEK_TEST_MODEL);

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
    graph.add_node(a).expect("add input node");
    graph.add_node(b).expect("add translator node");

    // Edge: A -> B
    graph.add_edge(a_id, vec![b_id]).expect("add edge");

    // 3. Run
    let result = graph.async_start().await;
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

/// Test fixture tool that records whether the agent invoked it.
///
/// The `called` flag is shared with the test harness via `Arc<AtomicBool>` so the
/// assertion can detect whether the model actually emitted a function call instead of
/// answering directly in natural language.
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

/// Integration test for DeepSeek agent execution with Tools.
///
/// Scenario: registers a fixture `WeatherTool`, drives the agent with a prompt that
/// would naturally call it, and asserts the response either invokes the tool or at
/// minimum produces a plausible weather-shaped answer. This is intentionally lenient
/// because tool-call routing is non-deterministic across provider revisions — the
/// test exists to guard the wiring (tool spec is reachable, response shape is
/// parseable), not the model's behavior.
///
/// Boundary: skipped when `DEEPSEEK_API_KEY` is unset, matching
/// [`test_deepseek_agent_execution`].
///
/// # Setup
///
/// ```bash
/// source .env.test && cargo test --test ai_agent_test test_deepseek_agent_with_tools
/// ```
#[tokio::test]
async fn test_deepseek_agent_with_tools() {
    if !deepseek_live_enabled() {
        eprintln!("skipped (set DEEPSEEK_API_KEY to run the DeepSeek L3 gate)");
        return;
    }

    // 1. Create DeepSeek Agent
    let client = Client::from_env().expect("DEEPSEEK_API_KEY missing despite gate");
    let model = client.completion_model(DEEPSEEK_TEST_MODEL);

    let tool_called = Arc::new(AtomicBool::new(false));

    let mut tool_set = ToolSet::default();
    tool_set.tools.push(std::sync::Arc::new(WeatherTool {
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
    graph.add_node(a).expect("add input node");
    graph.add_node(b).expect("add weather node");

    // Edge: A -> B
    graph.add_edge(a_id, vec![b_id]).expect("add edge");

    // 3. Run
    let result = graph.async_start().await;
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
