use std::{env, sync::Arc};

use async_trait::async_trait;
use dagrs::{
    Action, Content, DefaultNode, EnvVar, Graph, InChannels, Node, NodeTable, OutChannels, Output,
};
use libra::internal::ai::{
    agent::AgentBuilder, node_adapter::AgentAction, providers::gemini::Client,
};

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
/// cargo test test_gemini_agent_execution
/// ```
#[tokio::test]
async fn test_gemini_agent_execution() {
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
